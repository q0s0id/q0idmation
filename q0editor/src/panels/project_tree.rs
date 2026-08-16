use q0s_format::v2::{ProjectDependencyKind, ProjectDependencyNode, ProjectDependencySource};

use crate::app::{Action, EditorApp};

pub fn render(app: &mut EditorApp, ui: &mut egui::Ui) {
    let theme = app.settings.theme.clone();
    let node_count = app.state.project.runtime.project_graph.nodes.len();

    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.label(
                egui::RichText::new("Project dependencies")
                    .strong()
                    .size(18.0),
            );
            ui.label(
                egui::RichText::new("linked q0s movies and q0lang sources")
                    .small()
                    .color(theme.text_dim.to_color32()),
            );
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
            ui.label(
                egui::RichText::new(format!("{node_count} linked"))
                    .small()
                    .color(theme.text_dim.to_color32()),
            );
        });
    });
    ui.add_space(8.0);

    let selected = app.session.project_graph_selected;
    let parent_for_new = selected;
    ui.horizontal_wrapped(|ui| {
        if ui
            .button("Link existing...")
            .on_hover_text("Link a .q0s, .q0l or .q0lang file")
            .clicked()
        {
            if let Some(path) = rfd::FileDialog::new()
                .add_filter("q0 project dependency", &["q0s", "q0l", "q0lang"])
                .pick_file()
            {
                app.queue(Action::AddProjectDependency(path, parent_for_new));
            }
        }
        if ui
            .button("New q0lang source...")
            .on_hover_text("Create a .q0l file and link it here")
            .clicked()
        {
            let suggested = app
                .state
                .file_path
                .as_deref()
                .and_then(std::path::Path::parent)
                .map(|base| base.join("script.q0l"));
            if let Some(path) = crate::file_io::pick_q0lang_save_path(suggested.as_deref()) {
                match crate::file_io::save_q0lang_document(&path, "") {
                    Ok(()) => app.queue(Action::AddProjectDependency(path, parent_for_new)),
                    Err(error) => {
                        app.session.status = format!("could not create q0lang source: {error}")
                    }
                }
            }
        }
    });

    ui.add_space(10.0);
    render_root(app, ui, selected);
    ui.add_space(8.0);

    let nodes = app.state.project.runtime.project_graph.nodes.clone();
    render_tree(app, ui, &nodes);

    ui.add_space(8.0);
    render_selection_details(app, ui, &nodes);
}

fn render_root(app: &mut EditorApp, ui: &mut egui::Ui, selected: Option<u16>) {
    let theme = app.settings.theme.clone();
    let root_name = if app.state.project.meta.name.trim().is_empty() {
        "untitled"
    } else {
        &app.state.project.meta.name
    };
    let path = app
        .state
        .file_path
        .as_ref()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "unsaved project".to_string());

    let response = egui::Frame::group(ui.style())
        .fill(theme.panel.to_color32())
        .rounding(egui::Rounding::same(6.0))
        .inner_margin(egui::Margin::symmetric(10.0, 8.0))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("ROOT")
                        .monospace()
                        .small()
                        .color(theme.accent.to_color32()),
                );
                ui.vertical(|ui| {
                    ui.label(egui::RichText::new(root_name).strong());
                    ui.label(
                        egui::RichText::new(path)
                            .small()
                            .color(theme.text_dim.to_color32()),
                    );
                });
            });
        })
        .response
        .interact(egui::Sense::click());

    if selected.is_none() {
        ui.painter().rect_stroke(
            response.rect,
            6.0,
            egui::Stroke::new(1.0_f32, theme.accent.to_color32()),
        );
    }
    if response.clicked() {
        app.session.project_graph_selected = None;
        app.session.project_graph_rename = None;
    }
}

fn render_tree(app: &mut EditorApp, ui: &mut egui::Ui, nodes: &[ProjectDependencyNode]) {
    let theme = app.settings.theme.clone();
    let rows = flattened_nodes(nodes);
    let tree_height = (ui.available_height() - 190.0).clamp(150.0, 360.0);

    egui::Frame::group(ui.style())
        .rounding(egui::Rounding::same(6.0))
        .inner_margin(egui::Margin::symmetric(8.0, 8.0))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Tree").strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new("double-click to open")
                            .small()
                            .color(theme.text_dim.to_color32()),
                    );
                });
            });
            ui.separator();

            if rows.is_empty() {
                ui.allocate_ui_with_layout(
                    egui::vec2(ui.available_width(), 80.0),
                    egui::Layout::centered_and_justified(egui::Direction::TopDown),
                    |ui| {
                        ui.label(
                            egui::RichText::new("No linked files yet")
                                .color(theme.text_dim.to_color32()),
                        );
                    },
                );
                return;
            }

            egui::ScrollArea::vertical()
                .id_source("project_tree_scroll")
                .max_height(tree_height)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for (depth, node) in rows {
                        render_node_row(app, ui, depth, &node);
                    }
                });
        });
}

fn render_node_row(
    app: &mut EditorApp,
    ui: &mut egui::Ui,
    depth: usize,
    node: &ProjectDependencyNode,
) {
    let theme = app.settings.theme.clone();
    let resolved = app.resolve_project_dependency_path(node.node_id);
    let missing = matches!(node.source, ProjectDependencySource::External(_))
        && resolved.as_ref().is_none_or(|path| !path.is_file());
    let selected = app.session.project_graph_selected == Some(node.node_id);
    let kind = match node.kind {
        ProjectDependencyKind::Movie => "Q0S",
        ProjectDependencyKind::Q0lang => "Q0L",
    };
    let badge = if missing {
        "missing"
    } else {
        match node.source {
            ProjectDependencySource::External(_) => "external",
            ProjectDependencySource::Embedded(_) => "embedded",
        }
    };
    let source_label = match &node.source {
        ProjectDependencySource::External(stored) => {
            let resolved = resolved
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| stored.clone());
            format!("external\n{resolved}")
        }
        ProjectDependencySource::Embedded(bytes) => {
            format!("embedded\n{} bytes", bytes.len())
        }
    };

    let row = ui
        .horizontal(|ui| {
            ui.add_space((depth as f32) * 18.0);
            ui.label(
                egui::RichText::new(kind)
                    .monospace()
                    .small()
                    .color(theme.accent.to_color32()),
            );
            let name = ui.selectable_label(selected, egui::RichText::new(&node.alias).strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(egui::RichText::new(badge).small().color(if missing {
                    theme.syntax_signal.to_color32()
                } else {
                    theme.text_dim.to_color32()
                }));
            });
            name
        })
        .inner;

    let row = row.on_hover_text(source_label);
    if row.clicked() {
        app.session.project_graph_selected = Some(node.node_id);
        app.session.project_graph_rename = None;
    }
    if row.double_clicked() {
        app.queue(Action::OpenProjectDependency(node.node_id));
    }
}

fn render_selection_details(
    app: &mut EditorApp,
    ui: &mut egui::Ui,
    nodes: &[ProjectDependencyNode],
) {
    let theme = app.settings.theme.clone();
    let Some(node_id) = app.session.project_graph_selected else {
        egui::Frame::group(ui.style())
            .rounding(egui::Rounding::same(6.0))
            .inner_margin(egui::Margin::symmetric(10.0, 10.0))
            .show(ui, |ui| {
                ui.label(
                    egui::RichText::new(
                        "Select a linked file to edit its alias, parent or source.",
                    )
                    .small()
                    .color(theme.text_dim.to_color32()),
                );
            });
        return;
    };

    let Some(node) = nodes.iter().find(|node| node.node_id == node_id).cloned() else {
        app.session.project_graph_selected = None;
        return;
    };

    egui::Frame::group(ui.style())
        .fill(theme.panel.to_color32())
        .rounding(egui::Rounding::same(6.0))
        .inner_margin(egui::Margin::symmetric(10.0, 10.0))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(&node.alias).strong().size(15.0));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let kind = match node.kind {
                        ProjectDependencyKind::Movie => "q0s movie",
                        ProjectDependencyKind::Q0lang => "q0lang source",
                    };
                    ui.label(
                        egui::RichText::new(kind)
                            .small()
                            .color(theme.text_dim.to_color32()),
                    );
                });
            });
            ui.add_space(5.0);

            ui.horizontal_wrapped(|ui| {
                if ui.button("Open").clicked() {
                    app.queue(Action::OpenProjectDependency(node_id));
                }
                if ui.button("Rename").clicked() {
                    app.session.project_graph_rename = Some((node_id, node.alias.clone()));
                }
                if matches!(node.source, ProjectDependencySource::External(_)) {
                    if ui.button("Relink...").clicked() {
                        let extensions: &[&str] = match node.kind {
                            ProjectDependencyKind::Movie => &["q0s"],
                            ProjectDependencyKind::Q0lang => &["q0l", "q0lang"],
                        };
                        if let Some(path) = rfd::FileDialog::new()
                            .add_filter("replacement dependency", extensions)
                            .pick_file()
                        {
                            app.queue(Action::RelinkProjectDependency(node_id, path));
                        }
                    }
                    if ui
                        .button("Embed")
                        .on_hover_text("Store a copy inside this project")
                        .clicked()
                    {
                        app.queue(Action::EmbedProjectDependency(node_id));
                    }
                }
                if ui
                    .button("Remove")
                    .on_hover_text("Remove this node and its children")
                    .clicked()
                {
                    app.queue(Action::RemoveProjectDependency(node_id));
                }
            });

            if let Some((rename_id, mut alias)) = app.session.project_graph_rename.clone() {
                if rename_id == node_id {
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.label("Alias");
                        let response = ui.add_sized(
                            [220.0, 24.0],
                            egui::TextEdit::singleline(&mut alias).hint_text("q0lang alias"),
                        );
                        let enter = response.lost_focus()
                            && ui.input(|input| input.key_pressed(egui::Key::Enter));
                        if ui.button("Apply").clicked() || enter {
                            app.queue(Action::RenameProjectDependency(node_id, alias.clone()));
                            app.session.project_graph_rename = None;
                        } else if ui.button("Cancel").clicked() {
                            app.session.project_graph_rename = None;
                        } else {
                            app.session.project_graph_rename = Some((rename_id, alias));
                        }
                    });
                }
            }

            ui.add_space(6.0);
            let mut parent = node.parent_node_id;
            let old_parent = parent;
            egui::ComboBox::from_label("Parent")
                .selected_text(
                    parent
                        .and_then(|id| nodes.iter().find(|candidate| candidate.node_id == id))
                        .map(|candidate| candidate.alias.as_str())
                        .unwrap_or("project root"),
                )
                .width(220.0)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut parent, None, "project root");
                    for candidate in nodes {
                        if candidate.node_id != node_id {
                            ui.selectable_value(
                                &mut parent,
                                Some(candidate.node_id),
                                &candidate.alias,
                            );
                        }
                    }
                });
            if parent != old_parent {
                app.queue(Action::ReparentProjectDependency(node_id, parent));
            }
        });
}

fn flattened_nodes(nodes: &[ProjectDependencyNode]) -> Vec<(usize, ProjectDependencyNode)> {
    fn visit(
        nodes: &[ProjectDependencyNode],
        parent: Option<u16>,
        depth: usize,
        seen: &mut std::collections::HashSet<u16>,
        output: &mut Vec<(usize, ProjectDependencyNode)>,
    ) {
        if depth > 64 {
            return;
        }
        for node in nodes.iter().filter(|node| node.parent_node_id == parent) {
            if !seen.insert(node.node_id) {
                continue;
            }
            output.push((depth, node.clone()));
            visit(nodes, Some(node.node_id), depth + 1, seen, output);
        }
    }
    let mut output = Vec::with_capacity(nodes.len());
    let mut seen = std::collections::HashSet::new();
    visit(nodes, None, 0, &mut seen, &mut output);
    for node in nodes {
        if seen.insert(node.node_id) {
            output.push((0, node.clone()));
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flattened_project_tree_keeps_children_below_their_parent() {
        let nodes = vec![
            ProjectDependencyNode {
                node_id: 2,
                parent_node_id: Some(1),
                alias: "child".into(),
                kind: ProjectDependencyKind::Q0lang,
                source: ProjectDependencySource::External("child.q0l".into()),
            },
            ProjectDependencyNode {
                node_id: 1,
                parent_node_id: None,
                alias: "root".into(),
                kind: ProjectDependencyKind::Movie,
                source: ProjectDependencySource::External("root.q0s".into()),
            },
        ];
        let rows = flattened_nodes(&nodes);
        assert_eq!(
            rows.iter()
                .map(|(depth, node)| (*depth, node.node_id))
                .collect::<Vec<_>>(),
            vec![(0, 1), (1, 2)]
        );
    }
}
