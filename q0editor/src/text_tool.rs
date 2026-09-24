//! Editable text stored as vector outlines. The source and size live in the
//! existing q1s asset-name field; old readers still render the vector paths.

use egui::{Color32, Context, PointerButton, Response, RichText};
use q0s_format::v2::{
    Anchor, Asset, Path, Placement, Rgba, Target, Transform2D, Tween, Vec2, VectorAsset,
};
use ttf_parser::{Face, OutlineBuilder};

use crate::{app::EditorApp, state::Selection};

const SOURCE_PREFIX: &str = "\u{1f}q0text:1:";
const MAX_CHARS: usize = 240;

#[derive(Debug, Clone)]
pub struct TextDraft {
    pub asset_id: Option<u16>,
    pub origin: Vec2,
    pub text: String,
    pub size: f32,
    pub color: Rgba,
    pub focus_requested: bool,
}

pub fn encode_source(text: &str, size: f32) -> String {
    let mut encoded = format!("{SOURCE_PREFIX}{}:", size.round() as u16);
    for byte in text.bytes() {
        use std::fmt::Write;
        let _ = write!(&mut encoded, "{byte:02x}");
    }
    encoded
}

pub fn decode_source(name: &str) -> Option<(String, f32)> {
    let encoded = name.strip_prefix(SOURCE_PREFIX)?;
    let (size, hex) = encoded.split_once(':')?;
    let size = size.parse::<u16>().ok()?;
    if !(8..=512).contains(&size) || hex.len() % 2 != 0 || hex.len() > MAX_CHARS * 8 {
        return None;
    }
    let bytes: Vec<u8> = (0..hex.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&hex[index..index + 2], 16).ok())
        .collect::<Option<_>>()?;
    let text = String::from_utf8(bytes).ok()?;
    (text.chars().count() <= MAX_CHARS).then_some((text, f32::from(size)))
}

pub fn display_name(name: &str) -> Option<String> {
    let (text, _) = decode_source(name)?;
    let first: String = text.lines().next().unwrap_or("").chars().take(30).collect();
    Some(format!("Text: {first}"))
}

pub fn selected_asset_id(app: &EditorApp, selection: &Selection) -> Option<u16> {
    let Selection::Placement {
        q0rg_id,
        layer_id,
        placement_idx,
    } = *selection
    else {
        return None;
    };
    let target = app
        .state
        .project
        .q0rgs
        .iter()
        .find(|q| q.q0rg_id == q0rg_id)?
        .layers
        .iter()
        .find(|l| l.layer_id == layer_id)?
        .placements
        .get(placement_idx)?
        .target;
    let Target::Asset(asset_id) = target else {
        return None;
    };
    app.state
        .project
        .asset_names
        .get(&asset_id)
        .and_then(|name| decode_source(name))
        .map(|_| asset_id)
}

pub fn begin_edit(app: &mut EditorApp, asset_id: u16) {
    let Some((text, size)) = app
        .state
        .project
        .asset_names
        .get(&asset_id)
        .and_then(|name| decode_source(name))
    else {
        return;
    };
    let color = app
        .state
        .project
        .assets
        .iter()
        .find_map(|asset| match asset {
            Asset::Vector(vector) if vector.asset_id == asset_id => vector.fill,
            _ => None,
        })
        .unwrap_or(app.session.stroke_color);
    app.session.text_editor = Some(TextDraft {
        asset_id: Some(asset_id),
        origin: Vec2::new(0.0, 0.0),
        text,
        size,
        color,
        focus_requested: true,
    });
}

pub fn handle_stage(app: &mut EditorApp, response: &Response, cursor: Option<Vec2>) {
    if !response.clicked_by(PointerButton::Primary) {
        return;
    }
    let Some(point) = cursor else { return };
    if point.x < 0.0
        || point.y < 0.0
        || point.x > f32::from(app.state.project.meta.stage_width)
        || point.y > f32::from(app.state.project.meta.stage_height)
    {
        return;
    }
    if let Some(selection) = crate::tools::selection_at_point_pub(
        &app.state.project,
        app.session.current_q0rg_id,
        app.session.current_frame,
        point,
    ) {
        if let Some(asset_id) = selected_asset_id(app, &selection) {
            app.session.selection = selection;
            begin_edit(app, asset_id);
            return;
        }
    }
    app.session.text_editor = Some(TextDraft {
        asset_id: None,
        origin: point,
        text: String::new(),
        size: 48.0,
        color: app.session.fill_color.unwrap_or(app.session.stroke_color),
        focus_requested: true,
    });
}

pub fn render_dialog(app: &mut EditorApp, ctx: &Context) {
    let Some(mut draft) = app.session.text_editor.take() else {
        return;
    };
    let mut open = true;
    let mut apply = false;
    let mut cancel = false;
    egui::Window::new(if draft.asset_id.is_some() {
        "Edit text"
    } else {
        "Add text"
    })
    .open(&mut open)
    .collapsible(false)
    .resizable(false)
    .default_width(380.0)
    .show(ctx, |ui| {
        ui.label(RichText::new("Type on the stage").size(20.0).strong());
        ui.label(
            RichText::new("The text remains editable and exports as vector outlines.")
                .small()
                .color(app.settings.theme.text_dim.to_color32()),
        );
        ui.add_space(10.0);
        let entry = ui.add(
            egui::TextEdit::multiline(&mut draft.text)
                .desired_width(360.0)
                .desired_rows(4)
                .hint_text("Your text..."),
        );
        if draft.focus_requested {
            entry.request_focus();
            draft.focus_requested = false;
        }
        ui.horizontal(|ui| {
            ui.label("Size");
            ui.add(
                egui::DragValue::new(&mut draft.size)
                    .clamp_range(8.0..=512.0)
                    .suffix(" px"),
            );
            ui.label("Color");
            let mut color = Color32::from_rgba_unmultiplied(
                draft.color.r,
                draft.color.g,
                draft.color.b,
                draft.color.a,
            );
            if ui.color_edit_button_srgba(&mut color).changed() {
                draft.color = Rgba {
                    r: color.r(),
                    g: color.g(),
                    b: color.b(),
                    a: color.a(),
                };
            }
        });
        if draft.text.chars().count() > MAX_CHARS {
            ui.colored_label(
                ui.visuals().error_fg_color,
                format!("Limit: {MAX_CHARS} characters"),
            );
        }
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    !draft.text.trim().is_empty() && draft.text.chars().count() <= MAX_CHARS,
                    egui::Button::new("Apply text").fill(app.settings.theme.accent.to_color32()),
                )
                .clicked()
            {
                apply = true;
            }
            if ui.button("Cancel").clicked() {
                cancel = true;
            }
        });
    });
    if apply {
        if let Err(error) = commit(app, &draft) {
            app.session.status = error;
            app.session.text_editor = Some(draft);
        }
    } else if open && !cancel {
        app.session.text_editor = Some(draft);
    }
}

fn font_bytes() -> Option<Vec<u8>> {
    ["Segoe UI", "Tahoma", "Arial", "Verdana"]
        .iter()
        .find_map(|name| crate::q0lang::fonts::load_font_bytes(name))
}

fn commit(app: &mut EditorApp, draft: &TextDraft) -> Result<(), String> {
    let bytes = font_bytes().ok_or("No compatible system font found for text outlines")?;
    let paths = outline_text(&bytes, &draft.text, draft.size)?;
    if paths.is_empty() {
        return Err("The chosen font has no outlines for this text".into());
    }
    let source = encode_source(&draft.text, draft.size);
    if let Some(asset_id) = draft.asset_id {
        let Some(index) = app
            .state
            .project
            .assets
            .iter()
            .position(|asset| matches!(asset, Asset::Vector(v) if v.asset_id == asset_id))
        else {
            return Err("Text object no longer exists".into());
        };
        app.history.snapshot(&app.state.project);
        if let Asset::Vector(vector) = &mut app.state.project.assets[index] {
            vector.paths = paths;
            vector.fill = Some(draft.color);
        }
        app.state.project.asset_names.insert(asset_id, source);
        app.textures.invalidate_asset(asset_id);
        app.state.mark_dirty();
        app.session.status = "Text updated".into();
    } else {
        let q0rg_id = app.session.current_q0rg_id;
        let layer_id = app.session.current_layer_id;
        let frame = app.session.current_frame;
        if app.state.project.layer_is_folder(q0rg_id, layer_id)
            || app.state.project.layer_is_locked(q0rg_id, layer_id)
            || !app.state.project.layer_is_visible(q0rg_id, layer_id)
        {
            return Err("Choose an unlocked drawing layer for text".into());
        }
        app.history.snapshot(&app.state.project);
        crate::tools::materialize_layer_keyframe_for_edit(
            &mut app.state.project,
            q0rg_id,
            layer_id,
            frame,
        )
        .ok_or("Cannot add text to this timeline layer")?;
        let asset_id = crate::tools::next_asset_id_pub(&app.state.project);
        app.state.project.assets.push(Asset::Vector(VectorAsset {
            asset_id,
            paths,
            fill: Some(draft.color),
            stroke: None,
        }));
        app.state.project.asset_names.insert(asset_id, source);
        let layer = app
            .state
            .project
            .q0rgs
            .iter_mut()
            .find(|q| q.q0rg_id == q0rg_id)
            .and_then(|q| q.layers.iter_mut().find(|l| l.layer_id == layer_id))
            .ok_or("Drawing layer no longer exists")?;
        let placement_idx = layer.placements.len();
        layer.placements.push(Placement {
            instance_id: 0,
            frame,
            target: Target::Asset(asset_id),
            transform: Transform2D {
                tx: draft.origin.x,
                ty: draft.origin.y,
                ..Transform2D::IDENTITY
            },
            tween: Tween::None,
            fx: Default::default(),
        });
        app.session.selection = Selection::Placement {
            q0rg_id,
            layer_id,
            placement_idx,
        };
        app.state.mark_dirty();
        app.session.status = "Text added".into();
    }
    Ok(())
}

struct GlyphPaths {
    paths: Vec<Path>,
    current: Vec<Anchor>,
    x: f32,
    baseline: f32,
    scale: f32,
}

impl GlyphPaths {
    fn point(&self, x: f32, y: f32) -> Vec2 {
        Vec2::new(self.x + x * self.scale, self.baseline - y * self.scale)
    }
    fn flush(&mut self) {
        if self.current.len() >= 2 {
            self.paths.push(Path {
                anchors: std::mem::take(&mut self.current),
                closed: true,
            });
        } else {
            self.current.clear();
        }
    }
}

impl OutlineBuilder for GlyphPaths {
    fn move_to(&mut self, x: f32, y: f32) {
        self.flush();
        let point = self.point(x, y);
        self.current.push(Anchor {
            point,
            in_handle: None,
            out_handle: None,
        });
    }
    fn line_to(&mut self, x: f32, y: f32) {
        let point = self.point(x, y);
        self.current.push(Anchor {
            point,
            in_handle: None,
            out_handle: None,
        });
    }
    fn quad_to(&mut self, cx: f32, cy: f32, x: f32, y: f32) {
        let control = self.point(cx, cy);
        let end = self.point(x, y);
        if let Some(last) = self.current.last_mut() {
            last.out_handle = Some(Vec2::new(
                last.point.x + (control.x - last.point.x) * (2.0 / 3.0),
                last.point.y + (control.y - last.point.y) * (2.0 / 3.0),
            ));
        }
        self.current.push(Anchor {
            point: end,
            in_handle: Some(Vec2::new(
                end.x + (control.x - end.x) * (2.0 / 3.0),
                end.y + (control.y - end.y) * (2.0 / 3.0),
            )),
            out_handle: None,
        });
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let first = self.point(x1, y1);
        let second = self.point(x2, y2);
        let end = self.point(x, y);
        if let Some(last) = self.current.last_mut() {
            last.out_handle = Some(first);
        }
        self.current.push(Anchor {
            point: end,
            in_handle: Some(second),
            out_handle: None,
        });
    }
    fn close(&mut self) {
        if self.current.len() >= 3 {
            let first = self.current[0].point;
            let last = self.current.last().expect("contour endpoint");
            if (first.x - last.point.x).abs() < 0.001 && (first.y - last.point.y).abs() < 0.001 {
                let closing = self.current.pop().expect("contour endpoint");
                self.current[0].in_handle = closing.in_handle;
            }
        }
        self.flush();
    }
}

fn outline_text(font: &[u8], text: &str, size: f32) -> Result<Vec<Path>, String> {
    let face = Face::parse(font, 0).map_err(|_| "Unable to read the system font")?;
    let scale = size / f32::from(face.units_per_em());
    let mut builder = GlyphPaths {
        paths: Vec::new(),
        current: Vec::new(),
        x: 0.0,
        baseline: size,
        scale,
    };
    for character in text.chars() {
        if character == '\n' {
            builder.x = 0.0;
            builder.baseline += size * 1.25;
            continue;
        }
        if let Some(glyph) = face.glyph_index(character) {
            face.outline_glyph(glyph, &mut builder);
            builder.x += f32::from(
                face.glyph_hor_advance(glyph)
                    .unwrap_or(face.units_per_em() / 2),
            ) * scale;
        } else {
            builder.x += size * 0.5;
        }
    }
    builder.flush();
    Ok(builder.paths)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn text_source_round_trips_unicode_and_newlines() {
        let source = encode_source("Привет\nq0e", 48.0);
        assert_eq!(decode_source(&source), Some(("Привет\nq0e".into(), 48.0)));
        assert_eq!(display_name(&source), Some("Text: Привет".into()));
    }

    #[test]
    fn vector_text_survives_project_save_and_remains_editable() {
        let Some(_) = font_bytes() else { return }; // Windows system font; outline logic is exercised on supported hosts.
        let mut app = EditorApp::default();
        let draft = TextDraft {
            asset_id: None,
            origin: Vec2::new(24.0, 42.0),
            text: "Hello q0e".into(),
            size: 48.0,
            color: Rgba {
                r: 20,
                g: 30,
                b: 40,
                a: 255,
            },
            focus_requested: false,
        };
        commit(&mut app, &draft).expect("insert outlined text");
        let selected =
            selected_asset_id(&app, &app.session.selection).expect("editable selected text");
        let bytes = q0s_format::v2::write(&app.state.project).expect("write q1s");
        let project = q0s_format::v2::parse(&bytes).expect("read q1s");
        assert_eq!(
            decode_source(&project.asset_names[&selected]),
            Some((draft.text, 48.0))
        );
        assert!(
            matches!(project.assets.iter().find(|asset| asset.id() == selected),
            Some(Asset::Vector(vector)) if !vector.paths.is_empty() && vector.fill == Some(draft.color))
        );
    }
}
