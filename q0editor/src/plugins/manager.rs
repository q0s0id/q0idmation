//! The Plugins window: cards to switch plugins on and off, each plugin's
//! page and settings, and the question before installing one.

use egui::{vec2, Align2, Color32, Context, FontId, Rect, RichText, Sense, Stroke, Ui};
use q0ex::{ExprKind, SettingDecl, SettingKind, Value};

use super::{LineKind, PendingInstall};
use crate::app::EditorApp;
use crate::ui::FitToScreen;

/// A setting's value before anyone changed it.
pub(crate) fn default_value(decl: &SettingDecl) -> Value {
    fn literal(kind: &ExprKind) -> Option<Value> {
        Some(match kind {
            ExprKind::Null => Value::Null,
            ExprKind::Bool(value) => Value::Bool(*value),
            ExprKind::Num(value, unit) => {
                Value::Num(value * unit.map_or(1.0, q0ex::ast::Unit::scale))
            }
            ExprKind::Str(text) => Value::str(text),
            ExprKind::Unary(q0ex::ast::UnOp::Neg, inner) => match literal(&inner.kind)? {
                Value::Num(value) => Value::Num(-value),
                _ => return None,
            },
            ExprKind::List(items) => Value::list(
                items
                    .iter()
                    .map(|item| literal(&item.kind).unwrap_or_default())
                    .collect(),
            ),
            _ => return None,
        })
    }
    if let Some(value) = decl.default.as_ref().and_then(|expr| literal(&expr.kind)) {
        return value;
    }
    match decl.kind {
        SettingKind::Slider | SettingKind::Number => Value::Num(decl.min.unwrap_or(0.0)),
        SettingKind::Toggle => Value::Bool(false),
        SettingKind::Text => Value::str(""),
        SettingKind::Color => Value::str("#ffffff"),
        SettingKind::Choice => decl
            .options
            .first()
            .and_then(|option| literal(&option.kind))
            .unwrap_or_default(),
        SettingKind::Button | SettingKind::Heading | SettingKind::Note => Value::Null,
    }
}

/// An on/off switch that slides.
pub(crate) fn switch(ui: &mut Ui, on: &mut bool, accent: Color32) -> egui::Response {
    let size = vec2(38.0, 21.0);
    let (rect, mut response) = ui.allocate_exact_size(size, Sense::click());
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    let t = ui.ctx().animate_bool_with_time(response.id, *on, 0.16);
    let off = ui.visuals().widgets.inactive.bg_fill;
    let fill = lerp_color(off, accent, t);
    let painter = ui.painter_at(rect.expand(2.0));
    painter.rect_filled(rect, rect.height() / 2.0, fill);
    if response.hovered() {
        painter.rect_stroke(
            rect,
            rect.height() / 2.0,
            Stroke::new(1.0_f32, accent.gamma_multiply(0.7)),
        );
    }
    let radius = rect.height() / 2.0 - 3.0;
    let x = egui::lerp(
        (rect.left() + radius + 3.0)..=(rect.right() - radius - 3.0),
        t,
    );
    let knob = egui::pos2(x, rect.center().y);
    painter.circle_filled(knob + vec2(0.0, 1.0), radius, Color32::from_black_alpha(50));
    painter.circle_filled(knob, radius, Color32::WHITE);
    response
}

fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let mix = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round() as u8;
    Color32::from_rgb(mix(a.r(), b.r()), mix(a.g(), b.g()), mix(a.b(), b.b()))
}

/// The plugin's picture, or its initial on its colour.
fn thumbnail(
    ui: &mut Ui,
    texture: Option<egui::TextureHandle>,
    name: &str,
    accent: Color32,
    size: f32,
) {
    let (rect, _) = ui.allocate_exact_size(vec2(size, size), Sense::hover());
    let rounding = size * 0.2;
    match texture {
        Some(texture) => {
            ui.painter()
                .rect_filled(rect, rounding, ui.visuals().extreme_bg_color);
            let image = egui::Image::new((texture.id(), vec2(size, size))).rounding(rounding);
            image.paint_at(ui, rect);
        }
        None => {
            let painter = ui.painter();
            painter.rect_filled(rect, rounding, accent.gamma_multiply(0.85));
            painter.rect_filled(
                Rect::from_min_max(rect.center(), rect.max),
                egui::Rounding {
                    se: rounding,
                    ..Default::default()
                },
                Color32::from_black_alpha(30),
            );
            let initial = name.chars().find(|c| c.is_alphanumeric()).unwrap_or('?');
            painter.text(
                rect.center(),
                Align2::CENTER_CENTER,
                initial.to_uppercase().to_string(),
                FontId::proportional(size * 0.5),
                crate::theme::on_accent(accent),
            );
        }
    }
}

fn status_pill(ui: &mut Ui, running: bool, error: bool) {
    let (text, color) = if error {
        ("error", Color32::from_rgb(235, 87, 87))
    } else if running {
        ("on", Color32::from_rgb(76, 194, 120))
    } else {
        ("off", ui.visuals().weak_text_color())
    };
    let galley = ui
        .painter()
        .layout_no_wrap(text.into(), FontId::proportional(10.5), color);
    let (rect, _) = ui.allocate_exact_size(galley.size() + vec2(14.0, 6.0), Sense::hover());
    ui.painter()
        .rect_filled(rect, rect.height() / 2.0, color.gamma_multiply(0.18));
    ui.painter()
        .galley(rect.min + vec2(7.0, 3.0), galley, color);
}

pub(super) fn render(app: &mut EditorApp, ctx: &Context) {
    if let Some(pending) = app.plugins.pending_install.take() {
        install_prompt(app, ctx, pending);
    }
    if !app.plugins.manager_open {
        return;
    }
    let mut open = true;
    let screen = ctx.screen_rect();
    egui::Window::new("Plugins")
        .id(egui::Id::new("q0editor_plugins_window"))
        .open(&mut open)
        .default_size([620.0, 560.0])
        .default_pos(screen.center() - vec2(310.0, 290.0))
        .min_size([380.0, 320.0])
        .collapsible(false)
        .resizable(true)
        .fit_to_screen(ctx)
        .show(ctx, |ui| {
            if !app.settings.plugins.warned {
                warning(app, ui);
            }
            match app.plugins.page.clone() {
                Some(id) => match app.plugins.index_of(&id) {
                    Some(index) => page(app, ui, index),
                    None => app.plugins.page = None,
                },
                None => list(app, ui),
            }
        });
    if !open {
        app.plugins.manager_open = false;
    }
}

fn warning(app: &mut EditorApp, ui: &mut Ui) {
    egui::Frame::none()
        .fill(Color32::from_rgb(235, 170, 40).gamma_multiply(0.16))
        .stroke(Stroke::new(
            1.0_f32,
            Color32::from_rgb(235, 170, 40).gamma_multiply(0.6),
        ))
        .rounding(8.0)
        .inner_margin(egui::Margin::same(10.0))
        .show(ui, |ui| {
            ui.label(RichText::new("Plugins can do anything").strong());
            ui.label(
                "A plugin can change every part of q0editor, your projects and your files. \
                 Add only plugins you trust, like the checked ones from the #plugins channel. \
                 Compare a plugin's fingerprint with the one posted there before installing it.",
            );
            if ui.button("Got it").clicked() {
                app.settings.plugins.warned = true;
                app.settings.save();
            }
        });
    ui.add_space(8.0);
}

fn list(app: &mut EditorApp, ui: &mut Ui) {
    let accent = app.settings.theme.accent.to_color32();
    ui.horizontal(|ui| {
        ui.label(RichText::new("Plugins").size(20.0).strong());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if crate::theme::primary_button(ui, "Add plugin...", accent).clicked() {
                app.queue(crate::app::Action::InstallPlugin);
            }
            if ui.button("Plugin Studio").clicked() {
                app.plugins.studio.open = true;
            }
            if let Some(dir) = super::plugin_dir() {
                #[cfg(not(target_os = "android"))]
                if ui
                    .button("Folder")
                    .on_hover_text(dir.display().to_string())
                    .clicked()
                {
                    let _ = std::fs::create_dir_all(&dir);
                    ui.ctx()
                        .open_url(egui::OpenUrl::new_tab(format!("file:///{}", dir.display())));
                }
                #[cfg(target_os = "android")]
                let _ = dir;
            }
        });
    });
    ui.label(
        RichText::new(
            "Plugins are .qx files written in q0ex. Drop one on the window, or add it from a file.",
        )
        .small()
        .color(ui.visuals().weak_text_color()),
    );
    ui.add_space(8.0);
    if app.plugins.list.is_empty() {
        ui.add_space(30.0);
        ui.vertical_centered(|ui| {
            ui.label(RichText::new("No plugins yet").size(17.0).strong());
            ui.label("Add one from a file, or make your own in the Plugin Studio.");
            ui.add_space(6.0);
            if ui.button("Make a plugin").clicked() {
                app.plugins.studio.open = true;
                app.plugins.studio.new_requested = true;
            }
        });
        return;
    }
    let ctx = ui.ctx().clone();
    egui::ScrollArea::vertical()
        .id_source("plugin_cards")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let mut toggled = None;
            let mut opened = None;
            for index in 0..app.plugins.list.len() {
                let texture = app.plugins.list[index].thumbnail(&ctx);
                let plugin = &app.plugins.list[index];
                let accent = plugin.accent();
                let running = plugin.running();
                let failed = plugin.error.is_some() || plugin.parse_error.is_some();
                let meta = plugin.meta.clone();
                let id = egui::Id::new(("plugin-card", &meta.id));
                let hovered_before = ui
                    .ctx()
                    .data(|data| data.get_temp::<bool>(id).unwrap_or(false));
                let lift = ui
                    .ctx()
                    .animate_bool_with_time(id.with("lift"), hovered_before, 0.12);
                let frame = egui::Frame::none()
                    .fill(lerp_color(
                        ui.visuals().faint_bg_color,
                        ui.visuals().widgets.hovered.bg_fill,
                        lift * 0.6,
                    ))
                    .stroke(Stroke::new(
                        1.0_f32,
                        lerp_color(
                            ui.visuals().widgets.noninteractive.bg_stroke.color,
                            accent,
                            lift,
                        ),
                    ))
                    .rounding(12.0)
                    .inner_margin(egui::Margin::same(12.0));
                let shown = frame.show(ui, |ui| {
                    ui.horizontal(|ui| {
                        thumbnail(ui, texture, &meta.name, accent, 56.0);
                        ui.add_space(6.0);
                        ui.vertical(|ui| {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(&meta.name).size(16.0).strong());
                                status_pill(ui, running, failed);
                            });
                            let by = match (meta.author.is_empty(), meta.version.is_empty()) {
                                (false, false) => format!("by {} · v{}", meta.author, meta.version),
                                (false, true) => format!("by {}", meta.author),
                                (true, false) => format!("v{}", meta.version),
                                (true, true) => String::new(),
                            };
                            if !by.is_empty() {
                                ui.label(
                                    RichText::new(by)
                                        .small()
                                        .color(ui.visuals().weak_text_color()),
                                );
                            }
                            if !meta.summary.is_empty() {
                                ui.label(&meta.summary);
                            }
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let mut on = running;
                            if switch(ui, &mut on, accent).changed() {
                                toggled = Some((index, on));
                            }
                        });
                    });
                });
                let response = ui.interact(shown.response.rect, id.with("click"), Sense::click());
                ui.ctx()
                    .data_mut(|data| data.insert_temp(id, response.hovered()));
                if response.clicked() && toggled.is_none() {
                    opened = Some(meta.id.clone());
                }
                if response.hovered() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
                ui.add_space(6.0);
            }
            if let Some((index, on)) = toggled {
                if on {
                    super::enable(app, &ctx, index);
                } else {
                    super::disable(app, &ctx, index);
                }
            } else if let Some(id) = opened {
                app.plugins.page = Some(id);
            }
        });
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    About,
    Settings,
    Code,
}

fn page(app: &mut EditorApp, ui: &mut Ui, index: usize) {
    let ctx = ui.ctx().clone();
    let texture = app.plugins.list[index].thumbnail(&ctx);
    let plugin = &app.plugins.list[index];
    let meta = plugin.meta.clone();
    let accent = plugin.accent();
    let running = plugin.running();
    let error = plugin
        .error
        .clone()
        .or_else(|| plugin.parse_error.as_ref().map(ToString::to_string));
    let has_settings = plugin.settings.iter().any(|decl| decl.kind.has_key());
    let fingerprint = plugin.fingerprint.clone();
    let tab_id = egui::Id::new(("plugin-tab", &meta.id));
    let mut tab =
        ctx.data(|data| data.get_temp::<u8>(tab_id))
            .map_or(Tab::About, |tab| match tab {
                1 => Tab::Settings,
                2 => Tab::Code,
                _ => Tab::About,
            });

    if ui.button("← All plugins").clicked() {
        app.plugins.page = None;
        return;
    }
    ui.add_space(6.0);
    // The banner: picture, name, switch.
    egui::Frame::none()
        .fill(accent.gamma_multiply(0.12))
        .rounding(14.0)
        .inner_margin(egui::Margin::same(14.0))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                thumbnail(ui, texture, &meta.name, accent, 84.0);
                ui.add_space(8.0);
                ui.vertical(|ui| {
                    ui.label(RichText::new(&meta.name).size(22.0).strong());
                    let mut by = Vec::new();
                    if !meta.author.is_empty() {
                        by.push(format!("by {}", meta.author));
                    }
                    if !meta.version.is_empty() {
                        by.push(format!("version {}", meta.version));
                    }
                    if !by.is_empty() {
                        ui.label(by.join(" · "));
                    }
                    if !meta.summary.is_empty() {
                        ui.label(RichText::new(&meta.summary).italics());
                    }
                    ui.label(
                        RichText::new(format!("id {} · fingerprint {fingerprint}", meta.id))
                            .small()
                            .monospace()
                            .color(ui.visuals().weak_text_color()),
                    );
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                    let mut on = running;
                    if switch(ui, &mut on, accent).changed() {
                        if on {
                            super::enable(app, &ctx, index);
                        } else {
                            super::disable(app, &ctx, index);
                        }
                    }
                    ui.label(if running { "On" } else { "Off" });
                });
            });
        });
    if let Some(error) = &error {
        ui.add_space(6.0);
        egui::Frame::none()
            .fill(Color32::from_rgb(235, 87, 87).gamma_multiply(0.14))
            .rounding(8.0)
            .inner_margin(egui::Margin::same(10.0))
            .show(ui, |ui| {
                ui.label(RichText::new("It stopped with an error").strong());
                ui.label(RichText::new(error).monospace());
                ui.horizontal(|ui| {
                    if ui.button("Try again").clicked() {
                        super::enable(app, &ctx, index);
                    }
                    if ui.button("Open in Studio").clicked() {
                        app.plugins.studio.open_plugin(&meta.id);
                    }
                });
            });
    }
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        for (value, label) in [
            (Tab::About, "About"),
            (Tab::Settings, "Settings"),
            (Tab::Code, "More"),
        ] {
            if value == Tab::Settings && !has_settings {
                continue;
            }
            if ui.selectable_label(tab == value, label).clicked() {
                tab = value;
            }
        }
    });
    ui.separator();
    ctx.data_mut(|data| {
        data.insert_temp(
            tab_id,
            match tab {
                Tab::About => 0u8,
                Tab::Settings => 1,
                Tab::Code => 2,
            },
        )
    });
    egui::ScrollArea::vertical()
        .id_source(("plugin-page", &meta.id))
        .auto_shrink([false, false])
        .show(ui, |ui| match tab {
            Tab::About => {
                let doc = app.plugins.list[index].doc.clone();
                let markup = doc.as_ref().map(q0ex::PluginDoc::page).unwrap_or_default();
                if markup.trim().is_empty() {
                    ui.label(RichText::new("This plugin has no page.").italics());
                } else {
                    super::page::show(ui, &markup, doc.as_ref(), &meta.id);
                }
            }
            Tab::Settings => settings_page(app, ui, index),
            Tab::Code => more(app, ui, index),
        });
}

/// The plugin's settings, laid out like q0editor's own.
pub(crate) fn settings_page(app: &mut EditorApp, ui: &mut Ui, index: usize) {
    let ctx = ui.ctx().clone();
    let plugin = &app.plugins.list[index];
    let id = plugin.meta.id.clone();
    let accent = plugin.accent();
    let decls = plugin.settings.clone();
    let mut changed: Vec<(String, Value)> = Vec::new();
    let mut pressed: Vec<String> = Vec::new();
    let mut first = true;
    for decl in &decls {
        match decl.kind {
            SettingKind::Heading => {
                if !first {
                    ui.add_space(10.0);
                }
                crate::theme::section_heading(ui, &decl.label, decl.help.as_deref().unwrap_or(""));
            }
            SettingKind::Note => {
                ui.label(RichText::new(&decl.label).color(ui.visuals().weak_text_color()));
            }
            SettingKind::Button => {
                let response = ui.add(egui::Button::new(&decl.label).min_size(vec2(140.0, 26.0)));
                let response = match &decl.help {
                    Some(help) => response.on_hover_text(help),
                    None => response,
                };
                if response.clicked() {
                    pressed.push(decl.key.clone());
                }
            }
            _ => {
                let stored = app
                    .settings
                    .plugins
                    .values
                    .get(&id)
                    .and_then(|values| values.get(&decl.key))
                    .map(Value::from_json);
                let current = stored.unwrap_or_else(|| default_value(decl));
                if let Some(value) = setting_row(ui, decl, &current, accent) {
                    changed.push((decl.key.clone(), value));
                }
            }
        }
        first = false;
    }
    ui.add_space(12.0);
    if ui.button("Reset to defaults").clicked() {
        app.settings.plugins.values.remove(&id);
        app.settings.save();
        for decl in decls.iter().filter(|decl| decl.kind.has_key()) {
            changed.push((decl.key.clone(), default_value(decl)));
        }
        changed.retain(|_| false);
        super::dispatch_arg(app, &ctx, &id, "setting", "", vec![]);
    }
    if !changed.is_empty() {
        let values = app.settings.plugins.values.entry(id.clone()).or_default();
        for (key, value) in &changed {
            values.insert(key.clone(), value.to_json());
        }
        app.plugins
            .settings_dirty
            .get_or_insert(std::time::Instant::now());
        for (key, value) in changed {
            super::dispatch_arg(app, &ctx, &id, "setting", &key, vec![value]);
        }
    }
    for key in pressed {
        super::dispatch_arg(app, &ctx, &id, "setting", &key, vec![]);
    }
}

/// One setting's row: its label, then its control. Returns a new value.
fn setting_row(ui: &mut Ui, decl: &SettingDecl, current: &Value, accent: Color32) -> Option<Value> {
    let mut out = None;
    ui.horizontal(|ui| {
        let label = ui.add_sized(
            vec2(170.0_f32.min(ui.available_width() * 0.45), 22.0),
            egui::Label::new(&decl.label).truncate(true),
        );
        if let Some(help) = &decl.help {
            label.on_hover_text(help);
        }
        match decl.kind {
            SettingKind::Slider => {
                let mut value = current.as_num().unwrap_or(0.0);
                let (low, high) = (decl.min.unwrap_or(0.0), decl.max.unwrap_or(100.0));
                let mut slider = egui::Slider::new(&mut value, low.min(high)..=high.max(low));
                if let Some(step) = decl.step {
                    slider = slider.step_by(step);
                }
                if ui.add(slider).changed() {
                    out = Some(Value::Num(value));
                }
            }
            SettingKind::Number => {
                let mut value = current.as_num().unwrap_or(0.0);
                let mut drag = egui::DragValue::new(&mut value).speed(decl.step.unwrap_or(0.1));
                if let (Some(low), Some(high)) = (decl.min, decl.max) {
                    drag = drag.clamp_range(low.min(high)..=high.max(low));
                }
                if ui.add(drag).changed() {
                    out = Some(Value::Num(value));
                }
            }
            SettingKind::Toggle => {
                let mut on = current.truthy();
                if switch(ui, &mut on, accent).changed() {
                    out = Some(Value::Bool(on));
                }
            }
            SettingKind::Text => {
                let mut text = current.to_text();
                if matches!(current, Value::Null) {
                    text.clear();
                }
                if ui.text_edit_singleline(&mut text).changed() {
                    out = Some(Value::str(text));
                }
            }
            SettingKind::Color => {
                let [r, g, b, a] = current
                    .as_str()
                    .and_then(q0ex::stdlib::parse_color)
                    .unwrap_or([255, 255, 255, 255]);
                let mut color = Color32::from_rgba_unmultiplied(r, g, b, a);
                if ui.color_edit_button_srgba(&mut color).changed() {
                    let [r, g, b, a] = color.to_srgba_unmultiplied();
                    out = Some(Value::str(q0ex::stdlib::color_text([r, g, b, a])));
                }
            }
            SettingKind::Choice => {
                let options = decl
                    .options
                    .iter()
                    .map(|option| match &option.kind {
                        ExprKind::Str(text) => Value::str(text),
                        ExprKind::Num(value, _) => Value::Num(*value),
                        ExprKind::Bool(value) => Value::Bool(*value),
                        _ => Value::Null,
                    })
                    .collect::<Vec<_>>();
                egui::ComboBox::from_id_source(("plugin-choice", &decl.key))
                    .selected_text(current.to_text())
                    .show_ui(ui, |ui| {
                        for option in options {
                            if ui
                                .selectable_label(option.equals(current), option.to_text())
                                .clicked()
                            {
                                out = Some(option);
                            }
                        }
                    });
            }
            _ => {}
        }
    });
    out
}

fn more(app: &mut EditorApp, ui: &mut Ui, index: usize) {
    let ctx = ui.ctx().clone();
    let plugin = &app.plugins.list[index];
    let id = plugin.meta.id.clone();
    let path = plugin.path.clone();
    ui.label(RichText::new("Where it lives").strong());
    ui.label(
        RichText::new(path.display().to_string())
            .monospace()
            .small(),
    );
    ui.add_space(8.0);
    ui.horizontal_wrapped(|ui| {
        if ui.button("Open in Plugin Studio").clicked() {
            app.plugins.studio.open_plugin(&id);
        }
        #[cfg(not(target_os = "android"))]
        if ui.button("Save a copy...").clicked() {
            let source = app.plugins.list[index].source.clone();
            let name = super::file_name_for(&id);
            if let Some(target) = rfd::FileDialog::new()
                .set_file_name(&name)
                .add_filter("q0ex plugin", &q0ex::EXTENSIONS)
                .save_file()
            {
                match std::fs::write(&target, source) {
                    Ok(()) => app.session.status = format!("saved a copy as {}", target.display()),
                    Err(error) => app.session.status = format!("could not save the copy: {error}"),
                }
            }
        }
        let remove = ui.add(egui::Button::new(
            RichText::new("Remove").color(Color32::from_rgb(235, 87, 87)),
        ));
        if remove.clicked() {
            ui.ctx()
                .data_mut(|data| data.insert_temp(egui::Id::new(("plugin-remove", &id)), true));
        }
    });
    let asking = ctx.data(|data| {
        data.get_temp::<bool>(egui::Id::new(("plugin-remove", &id)))
            .unwrap_or(false)
    });
    if asking {
        ui.add_space(6.0);
        egui::Frame::none()
            .fill(ui.visuals().faint_bg_color)
            .rounding(8.0)
            .inner_margin(egui::Margin::same(10.0))
            .show(ui, |ui| {
                ui.label("Remove this plugin and its saved values?");
                ui.horizontal(|ui| {
                    if ui.button("Remove it").clicked() {
                        ctx.data_mut(|data| {
                            data.remove::<bool>(egui::Id::new(("plugin-remove", &id)))
                        });
                        super::uninstall(app, &ctx, index);
                    }
                    if ui.button("Keep it").clicked() {
                        ctx.data_mut(|data| {
                            data.remove::<bool>(egui::Id::new(("plugin-remove", &id)))
                        });
                    }
                });
            });
    }
    ui.add_space(10.0);
    ui.label(RichText::new("Console").strong());
    let name = app
        .plugins
        .list
        .get(index)
        .map(|plugin| plugin.meta.name.clone())
        .unwrap_or_default();
    console(ui, app, Some(&name));
}

/// The console, for one plugin or for all.
pub(crate) fn console(ui: &mut Ui, app: &EditorApp, only: Option<&str>) {
    let lines = app
        .plugins
        .console
        .iter()
        .filter(|line| only.is_none_or(|name| line.plugin == name))
        .collect::<Vec<_>>();
    egui::Frame::none()
        .fill(ui.visuals().extreme_bg_color)
        .rounding(6.0)
        .inner_margin(egui::Margin::same(8.0))
        .show(ui, |ui| {
            egui::ScrollArea::vertical()
                .id_source(("plugin-console", only.unwrap_or("all")))
                .max_height(180.0)
                .stick_to_bottom(true)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    if lines.is_empty() {
                        ui.label(
                            RichText::new("nothing yet")
                                .italics()
                                .color(ui.visuals().weak_text_color()),
                        );
                    }
                    for line in lines {
                        let color = match line.kind {
                            LineKind::Error => Color32::from_rgb(235, 87, 87),
                            LineKind::Say => ui.visuals().hyperlink_color,
                            LineKind::Info => ui.visuals().weak_text_color(),
                            LineKind::Print => ui.visuals().text_color(),
                        };
                        ui.horizontal_wrapped(|ui| {
                            if only.is_none() {
                                ui.label(
                                    RichText::new(format!("{}:", line.plugin))
                                        .monospace()
                                        .small()
                                        .strong(),
                                );
                            }
                            ui.label(
                                RichText::new(&line.text)
                                    .monospace()
                                    .size(12.0)
                                    .color(color),
                            );
                        });
                    }
                });
        });
}

fn install_prompt(app: &mut EditorApp, ctx: &Context, pending: PendingInstall) {
    let mut decision: Option<bool> = None;
    let accent = pending.plugin.accent();
    let meta = pending.plugin.meta.clone();
    let texture = pending
        .plugin
        .doc
        .as_ref()
        .and_then(q0ex::PluginDoc::thumbnail)
        .and_then(|bytes| super::cached_texture(ctx, &format!("install-{}", meta.id), &bytes));
    let screen = ctx.screen_rect();
    egui::Window::new("Install this plugin?")
        .id(egui::Id::new("q0editor_plugin_install"))
        .collapsible(false)
        .resizable(false)
        .default_pos(screen.center() - vec2(200.0, 160.0))
        .default_width(400.0)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                thumbnail(ui, texture, &meta.name, accent, 64.0);
                ui.vertical(|ui| {
                    ui.label(RichText::new(&meta.name).size(18.0).strong());
                    if !meta.author.is_empty() {
                        ui.label(format!("by {}", meta.author));
                    }
                    if !meta.version.is_empty() {
                        ui.label(format!("version {}", meta.version));
                    }
                });
            });
            if !meta.summary.is_empty() {
                ui.add_space(4.0);
                ui.label(&meta.summary);
            }
            ui.add_space(6.0);
            ui.label(
                RichText::new(format!("fingerprint  {}", pending.plugin.fingerprint))
                    .monospace()
                    .strong(),
            );
            if let Some(version) = &pending.replaces {
                let text = if version.is_empty() {
                    "It replaces the installed plugin with the same id.".to_string()
                } else {
                    format!("It replaces the installed version {version}.")
                };
                ui.label(RichText::new(text).color(Color32::from_rgb(235, 170, 40)));
            }
            if let Some(error) = &pending.plugin.parse_error {
                ui.label(
                    RichText::new(format!(
                        "Its code has a mistake and will not run yet: {error}"
                    ))
                    .color(Color32::from_rgb(235, 87, 87)),
                );
            }
            ui.add_space(6.0);
            ui.label(
                RichText::new(
                    "A plugin can change anything in q0editor and your files. Install it only if \
                     you trust where it came from.",
                )
                .small()
                .color(ui.visuals().weak_text_color()),
            );
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if crate::theme::primary_button(ui, "Install", accent).clicked() {
                    decision = Some(true);
                }
                if ui.button("Cancel").clicked() {
                    decision = Some(false);
                }
            });
        });
    match decision {
        Some(true) => {
            let name = pending.plugin.meta.name.clone();
            super::install(app, ctx, pending);
            app.plugins
                .log(&name, LineKind::Info, "installed from a file");
        }
        Some(false) => {}
        None => app.plugins.pending_install = Some(pending),
    }
}
