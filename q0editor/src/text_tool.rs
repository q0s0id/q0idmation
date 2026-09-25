//! Editable text stored as vector outlines. The source, size and font live in the
//! existing q1s asset-name field; old readers still render the vector paths.

use egui::{Color32, Context, PointerButton, Response};
use q0s_format::transform::Affine;
use q0s_format::v2::{
    Anchor, Asset, Path, Placement, Rgba, Target, Transform2D, Tween, Vec2, VectorAsset,
};
use ttf_parser::{Face, OutlineBuilder};

use crate::{app::EditorApp, state::Selection};

const SOURCE_PREFIX: &str = "\u{1f}q0text:1:";
const FONT_SOURCE_PREFIX: &str = "\u{1f}q0text:2:";
const MAX_CHARS: usize = 240;

#[derive(serde::Serialize, serde::Deserialize)]
struct TextSource {
    text: String,
    size: f32,
    font: String,
}

#[derive(Debug, Clone)]
pub struct TextDraft {
    pub asset_id: Option<u16>,
    pub origin: Vec2,
    pub placement: Option<crate::state::PlacementRef>,
    pub q0rg_id: u16,
    pub layer_id: u16,
    pub frame: u16,
    pub transform: Affine,
    pub text: String,
    pub size: f32,
    pub font: String,
    pub font_query: String,
    pub font_picker_open: bool,
    pub font_previews: std::collections::HashMap<String, Option<egui::Mesh>>,
    pub color: Rgba,
    pub focus_requested: bool,
}

impl Default for TextDraft {
    fn default() -> Self {
        Self {
            asset_id: None,
            origin: Vec2::new(0.0, 0.0),
            placement: None,
            q0rg_id: 1,
            layer_id: 1,
            frame: 0,
            transform: Affine::IDENTITY,
            text: String::new(),
            size: 48.0,
            font: default_font(),
            font_query: default_font(),
            font_picker_open: false,
            font_previews: Default::default(),
            color: Rgba {
                r: 0,
                g: 0,
                b: 0,
                a: 255,
            },
            focus_requested: true,
        }
    }
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
    let source = decode_text_source(name)?;
    Some((source.text, source.size))
}

fn decode_text_source(name: &str) -> Option<TextSource> {
    if let Some(json) = name.strip_prefix(FONT_SOURCE_PREFIX) {
        let source: TextSource = serde_json::from_str(json).ok()?;
        return ((8.0..=512.0).contains(&source.size)
            && source.text.chars().count() <= MAX_CHARS
            && source.font.len() <= 1024)
            .then_some(source);
    }
    let encoded = name.strip_prefix(SOURCE_PREFIX)?;
    let (size, hex) = encoded.split_once(':')?;
    let size = size.parse::<u16>().ok()?;
    if !(8..=512).contains(&size)
        || !hex.is_ascii()
        || hex.len() % 2 != 0
        || hex.len() > MAX_CHARS * 8
    {
        return None;
    }
    let bytes: Vec<u8> = (0..hex.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&hex[index..index + 2], 16).ok())
        .collect::<Option<_>>()?;
    let text = String::from_utf8(bytes).ok()?;
    (text.chars().count() <= MAX_CHARS).then_some(TextSource {
        text,
        size: f32::from(size),
        font: default_font(),
    })
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

fn draft_for_asset(app: &EditorApp, asset_id: u16) -> Option<TextDraft> {
    let source = decode_text_source(app.state.project.asset_names.get(&asset_id)?)?;
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
    let selected = match app.session.selection {
        Selection::Placement {
            q0rg_id,
            layer_id,
            placement_idx,
        } => Some(crate::state::PlacementRef {
            q0rg_id,
            layer_id,
            placement_idx,
        }),
        _ => None,
    };
    let placement = selected.filter(|r| {
        app.state
            .project
            .q0rgs
            .iter()
            .find(|q| q.q0rg_id == r.q0rg_id)
            .and_then(|q| q.layers.iter().find(|l| l.layer_id == r.layer_id))
            .and_then(|l| l.placements.get(r.placement_idx))
            .is_some_and(|p| p.target == Target::Asset(asset_id))
    });
    let transform = placement
        .and_then(|r| {
            app.state
                .project
                .q0rgs
                .iter()
                .find(|q| q.q0rg_id == r.q0rg_id)
                .and_then(|q| q.layers.iter().find(|l| l.layer_id == r.layer_id))
                .and_then(|l| {
                    crate::render::active_placements_at(l, app.session.current_frame)
                        .into_iter()
                        .find(|(i, _)| *i == r.placement_idx)
                })
                .map(|(_, t)| Affine::from_transform(t))
        })
        .unwrap_or(Affine::IDENTITY);
    Some(TextDraft {
        asset_id: Some(asset_id),
        origin: transform.apply(Vec2::new(0.0, 0.0)),
        placement,
        q0rg_id: app.session.current_q0rg_id,
        layer_id: placement.map_or(app.session.current_layer_id, |p| p.layer_id),
        frame: app.session.current_frame,
        transform,
        text: source.text,
        size: source.size,
        font_query: source.font.clone(),
        font: source.font,
        color,
        ..Default::default()
    })
}

pub fn begin_edit(app: &mut EditorApp, asset_id: u16) {
    if let Some(draft) = draft_for_asset(app, asset_id) {
        if app
            .state
            .project
            .layer_is_locked(draft.q0rg_id, draft.layer_id)
        {
            return;
        }
        app.session.text_editor = Some(draft);
    }
}

pub fn handle_stage(app: &mut EditorApp, response: &Response, cursor: Option<Vec2>) {
    if !response.clicked_by(PointerButton::Primary) || app.session.text_editor.is_some() {
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
    let mut draft = app.session.text_defaults.clone();
    draft.origin = point;
    draft.transform = Affine {
        tx: point.x,
        ty: point.y,
        ..Affine::IDENTITY
    };
    draft.q0rg_id = app.session.current_q0rg_id;
    draft.layer_id = app.session.current_layer_id;
    draft.frame = app.session.current_frame;
    draft.focus_requested = true;
    app.session.selection = Selection::None;
    app.session.text_editor = Some(draft);
    response.ctx.request_repaint();
}

/// Commit one editing gesture; Escape simply drops the uncommitted draft.
pub(crate) fn finish_edit(app: &mut EditorApp) -> bool {
    let Some(draft) = app.session.text_editor.take() else {
        return true;
    };
    if draft.text.trim().is_empty() {
        if let Some(reference) = draft.placement {
            if app
                .state
                .project
                .layer_is_locked(reference.q0rg_id, reference.layer_id)
            {
                app.session.text_editor = Some(draft);
                return false;
            }
            app.history.snapshot(&app.state.project);
            let Some(placement_idx) = crate::tools::materialize_placement_keyframe_for_edit(
                &mut app.state.project,
                reference.q0rg_id,
                reference.layer_id,
                reference.placement_idx,
                draft.frame,
            ) else {
                return true;
            };
            if let Some(layer) = app
                .state
                .project
                .q0rgs
                .iter_mut()
                .find(|q| q.q0rg_id == reference.q0rg_id)
                .and_then(|q| {
                    q.layers
                        .iter_mut()
                        .find(|l| l.layer_id == reference.layer_id)
                })
            {
                if layer
                    .placements
                    .get(placement_idx)
                    .is_some_and(|p| Some(p.target) == draft.asset_id.map(Target::Asset))
                {
                    layer.placements.remove(placement_idx);
                    app.textures.invalidate();
                    app.state.mark_dirty();
                    app.session.selection = Selection::None;
                }
            }
        }
        return true;
    }
    if let Err(error) = commit(app, &draft) {
        app.session.status = error;
        app.session.text_editor = Some(draft);
        return false;
    }
    app.session.text_defaults.font = draft.font.clone();
    app.session.text_defaults.font_query = draft.font;
    app.session.text_defaults.size = draft.size;
    app.session.text_defaults.color = draft.color;
    true
}

fn canvas_font(ctx: &Context, name: &str) -> egui::FontFamily {
    let family = egui::FontFamily::Name("q0text_canvas".into());
    let id = egui::Id::new("q0text_canvas_font");
    let same = ctx.data(|d| d.get_temp::<String>(id).as_deref() == Some(name));
    let registered = ctx.fonts(|f| f.lock().fonts.definitions().families.contains_key(&family));
    if same && registered {
        return family;
    }
    if let Some((bytes, index)) = font_bytes(name) {
        if let Ok(face) = Face::parse(&bytes, index) {
            let em = f32::from(face.units_per_em());
            ctx.data_mut(|d| {
                d.insert_temp(
                    id.with("metrics"),
                    [
                        f32::from(face.ascender()) / em,
                        (f32::from(face.ascender()) - f32::from(face.descender())) / em,
                    ],
                )
            });
        }
        let mut definitions = ctx.fonts(|f| f.lock().fonts.definitions().clone());
        let mut data = egui::FontData::from_owned(bytes);
        data.index = index;
        definitions.font_data.insert("q0text_canvas".into(), data);
        let mut chain = vec!["q0text_canvas".into()];
        chain.extend(
            definitions
                .families
                .get(&egui::FontFamily::Proportional)
                .cloned()
                .unwrap_or_default(),
        );
        definitions.families.insert(family.clone(), chain);
        ctx.set_fonts(definitions);
        ctx.data_mut(|d| d.insert_temp(id, name.to_owned()));
        ctx.request_repaint();
    }
    if registered {
        family
    } else {
        egui::FontFamily::Proportional
    }
}

/// Inline canvas editing owns the pointer until a click outside its bounds.
pub(crate) fn render_canvas(
    app: &mut EditorApp,
    ui: &mut egui::Ui,
    view: &crate::render::StageView,
) -> bool {
    if app.session.text_canvas_click_consumed {
        if !ui.input(|i| i.pointer.primary_down()) {
            app.session.text_canvas_click_consumed = false;
        }
        return true;
    }
    let Some(mut draft) = app.session.text_editor.take() else {
        return false;
    };
    let scale = view.scale * draft.transform.uniform_scale().max(0.01);
    let mut origin = egui::pos2(
        view.origin.x + draft.origin.x * view.scale,
        view.origin.y + draft.origin.y * view.scale,
    );
    let family = canvas_font(ui.ctx(), &draft.font);
    let metrics = ui
        .ctx()
        .data(|d| d.get_temp::<[f32; 2]>(egui::Id::new("q0text_canvas_font").with("metrics")))
        .unwrap_or([1.0, 1.0]);
    origin.y += draft.size * scale * (1.0 - metrics[0]);
    let font = egui::FontId::new((draft.size * scale * metrics[1]).clamp(4.0, 1024.0), family);
    let color =
        Color32::from_rgba_unmultiplied(draft.color.r, draft.color.g, draft.color.b, draft.color.a);
    let text_width = ui.fonts(|fonts| {
        fonts
            .layout_no_wrap(draft.text.clone(), font.clone(), color)
            .size()
            .x
    });
    let width = (text_width + 24.0)
        .max(150.0)
        .min((ui.clip_rect().right() - origin.x).max(150.0));
    let rect = egui::Rect::from_min_size(origin, egui::vec2(width, font.size * 1.5));
    let mut entry_rect = rect;
    let mut editor_id = egui::Id::NULL;
    let mut editor_had_focus = false;
    let line_height = draft.size * scale * 1.25;
    let mut layouter = |ui: &egui::Ui, text: &str, _width: f32| {
        let mut job =
            egui::text::LayoutJob::simple(text.to_owned(), font.clone(), color, f32::INFINITY);
        for section in &mut job.sections {
            section.format.line_height = Some(line_height);
        }
        ui.fonts(|f| f.layout_job(job))
    };
    let focus_requested = draft.focus_requested;
    ui.allocate_ui_at_rect(rect, |ui| {
        ui.visuals_mut().extreme_bg_color = Color32::from_white_alpha(235);
        let response = ui.add(
            egui::TextEdit::multiline(&mut draft.text)
                .id_source("canvas_text_entry")
                .font(font.clone())
                .layouter(&mut layouter)
                .text_color(color)
                .desired_width(width)
                .desired_rows(1)
                .char_limit(MAX_CHARS)
                .margin(egui::Vec2::ZERO),
        );
        entry_rect = response.rect;
        editor_id = response.id;
        editor_had_focus = response.has_focus() || response.lost_focus();
        if draft.focus_requested {
            response.request_focus();
            draft.focus_requested = false;
        }
        ui.painter().rect_stroke(
            response.rect,
            0.0,
            egui::Stroke::new(1.0_f32, app.settings.theme.accent.to_color32()),
        );
    });
    let escape = editor_had_focus && ui.input(|i| i.key_pressed(egui::Key::Escape));
    let properties_rect = ui
        .ctx()
        .data(|d| d.get_temp::<egui::Rect>(egui::Id::new("text_properties_rect")));
    let apply = !focus_requested
        && (ui.input(|i| i.modifiers.command && i.key_pressed(egui::Key::Enter))
            || ui.input(|i| {
                i.pointer.primary_pressed()
                    && i.pointer.interact_pos().is_some_and(|p| {
                        !entry_rect.contains(p)
                            && !properties_rect.is_some_and(|rect| rect.contains(p))
                    })
            })
            || draft.q0rg_id != app.session.current_q0rg_id
            || draft.frame != app.session.current_frame);
    if !escape {
        app.session.text_editor = Some(draft);
    }
    if apply {
        app.session.text_canvas_click_consumed = ui.input(|i| i.pointer.primary_down());
        finish_edit(app);
    }
    if escape || apply {
        ui.memory_mut(|m| m.surrender_focus(editor_id));
        ui.ctx().request_repaint();
    }
    true
}

pub(crate) fn render_properties(app: &mut EditorApp, ui: &mut egui::Ui) -> bool {
    if let Some(mut draft) = app.session.text_editor.take() {
        text_controls(ui, &mut draft);
        app.session.text_editor = Some(draft);
        return true;
    }
    let asset_id =
        selected_asset_id(app, &app.session.selection).or_else(|| match app.session.selection {
            Selection::Asset(id)
                if app
                    .state
                    .project
                    .asset_names
                    .get(&id)
                    .and_then(|n| decode_source(n))
                    .is_some() =>
            {
                Some(id)
            }
            _ => None,
        });
    if let Some(id) = asset_id {
        let fresh = draft_for_asset(app, id).unwrap();
        let mut draft = app
            .session
            .text_inspector
            .take()
            .filter(|d| {
                d.asset_id == Some(id)
                    && d.placement == fresh.placement
                    && d.frame == fresh.frame
                    && d.q0rg_id == fresh.q0rg_id
                    && d.transform == fresh.transform
                    && d.text == fresh.text
                    && d.size == fresh.size
                    && d.font == fresh.font
                    && d.color == fresh.color
            })
            .unwrap_or(fresh);
        let before = (draft.font.clone(), draft.size, draft.color);
        text_controls(ui, &mut draft);
        if before != (draft.font.clone(), draft.size, draft.color) {
            if let Err(error) = commit(app, &draft) {
                app.session.status = error;
            }
        }
        if draft.placement.is_some() && ui.button("Edit on canvas").clicked() {
            begin_edit(app, id);
        }
        app.session.text_inspector = Some(draft);
        return true;
    }
    app.session.text_inspector = None;
    if app.session.current_tool == crate::state::Tool::Text {
        let mut defaults = std::mem::take(&mut app.session.text_defaults);
        text_controls(ui, &mut defaults);
        app.session.text_defaults = defaults;
        return true;
    }
    false
}

fn text_controls(ui: &mut egui::Ui, draft: &mut TextDraft) {
    // Keep editing while the user adjusts the text's own inspector controls.
    ui.ctx()
        .data_mut(|d| d.insert_temp(egui::Id::new("text_properties_rect"), ui.clip_rect()));
    ui.label(egui::RichText::new("Text").strong());
    font_picker(ui, draft);
    ui.horizontal(|ui| {
        ui.label("Size");
        ui.add(
            egui::DragValue::new(&mut draft.size)
                .clamp_range(8.0..=512.0)
                .suffix(" px"),
        );
    });
    ui.horizontal(|ui| {
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
    ui.label(egui::RichText::new("Click outside the text on the canvas to finish.").small());
}

fn default_font() -> String {
    static DEFAULT: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    DEFAULT
        .get_or_init(|| {
            ["Segoe UI", "Tahoma", "Arial", "Verdana"]
                .iter()
                .find(|name| crate::q0lang::fonts::load_font_bytes(name).is_some())
                .copied()
                .or_else(|| {
                    crate::text_fonts::installed()?
                        .first()
                        .map(|f| f.name.as_str())
                })
                .unwrap_or("Segoe UI")
                .to_owned()
        })
        .clone()
}

fn font_bytes(name: &str) -> Option<(Vec<u8>, u32)> {
    if let Some(font) = crate::text_fonts::find(name) {
        return std::fs::read(&font.path)
            .ok()
            .map(|bytes| (bytes, font.index));
    }
    crate::q0lang::fonts::load_font_bytes(name).map(|bytes| (bytes, 0))
}

fn font_picker(ui: &mut egui::Ui, draft: &mut TextDraft) {
    ui.label("Font");
    let mut output = egui::TextEdit::singleline(&mut draft.font_query)
        .desired_width(ui.available_width())
        .hint_text("Search installed fonts...")
        .show(ui);
    let response = output.response;
    if response.gained_focus() {
        output
            .state
            .cursor
            .set_char_range(Some(egui::text::CCursorRange::two(
                egui::text::CCursor::new(0),
                egui::text::CCursor::new(draft.font_query.chars().count()),
            )));
        output.state.store(ui.ctx(), response.id);
    }
    if response.gained_focus() || response.clicked() || response.changed() {
        draft.font_picker_open = true;
    }
    let Some(fonts) = crate::text_fonts::installed() else {
        ui.label("Loading installed fonts...");
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(100));
        return;
    };
    if !draft.font_picker_open {
        return;
    }
    let query = if draft.font_query == draft.font {
        String::new()
    } else {
        draft.font_query.to_lowercase()
    };
    let matches: Vec<_> = fonts
        .iter()
        .filter(|f| f.name.to_lowercase().contains(&query))
        .collect();
    let mut selected = None;
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.set_min_width(ui.available_width().max(160.0));
        if matches.is_empty() {
            ui.label("No matching installed fonts");
        }
        egui::ScrollArea::vertical()
            .id_source("text_font_results")
            .max_height(200.0)
            .show_rows(ui, 30.0, matches.len(), |ui, rows| {
                for row in rows {
                    let font = matches[row];
                    let (rect, response) = ui.allocate_exact_size(
                        egui::vec2(ui.available_width(), 30.0),
                        egui::Sense::click(),
                    );
                    if response.hovered() || draft.font == font.name {
                        ui.painter()
                            .rect_filled(rect, 3.0, ui.visuals().widgets.hovered.bg_fill);
                    }
                    let name_rect =
                        egui::Rect::from_min_max(rect.min, egui::pos2(rect.center().x, rect.max.y));
                    ui.painter().with_clip_rect(name_rect).text(
                        rect.left_center() + egui::vec2(5.0, 0.0),
                        egui::Align2::LEFT_CENTER,
                        &font.name,
                        egui::FontId::proportional(14.0),
                        ui.visuals().text_color(),
                    );
                    let preview =
                        draft
                            .font_previews
                            .entry(font.name.clone())
                            .or_insert_with(|| {
                                let bytes = std::fs::read(&font.path).ok()?;
                                let paths =
                                    outline_text(&bytes, font.index, "Lorem ipsum.", 20.0).ok()?;
                                crate::render::text_preview_mesh(paths)
                            });
                    if let Some(mesh) = preview {
                        let mut mesh = mesh.clone();
                        let bounds = mesh.calc_bounds();
                        let scale = ((rect.width() * 0.48 - 8.0) / bounds.width().max(1.0))
                            .min(22.0 / bounds.height().max(1.0))
                            .min(1.0);
                        for vertex in &mut mesh.vertices {
                            vertex.pos = egui::pos2(
                                rect.center().x + 8.0,
                                rect.center().y - bounds.height() * scale * 0.5,
                            ) + (vertex.pos - bounds.min) * scale;
                            vertex.color = ui.visuals().text_color();
                        }
                        ui.painter()
                            .with_clip_rect(rect)
                            .add(egui::Shape::Mesh(mesh));
                    }
                    if response.clicked() {
                        selected = Some(font.name.clone());
                    }
                }
            });
    });
    if response.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
        selected = matches
            .iter()
            .find(|font| font.name.eq_ignore_ascii_case(&draft.font_query))
            .or_else(|| matches.first())
            .map(|font| font.name.clone());
    }
    if let Some(name) = selected {
        draft.font_query = name.clone();
        draft.font = name;
        draft.font_picker_open = false;
        response.surrender_focus();
    } else if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        draft.font_query = draft.font.clone();
        draft.font_picker_open = false;
        response.surrender_focus();
    }
}

fn commit(app: &mut EditorApp, draft: &TextDraft) -> Result<(), String> {
    if draft.text.chars().count() > MAX_CHARS {
        return Err(format!("Limit: {MAX_CHARS} characters"));
    }
    if draft.placement.is_some()
        && (app
            .state
            .project
            .layer_is_locked(draft.q0rg_id, draft.layer_id)
            || !app
                .state
                .project
                .layer_is_visible(draft.q0rg_id, draft.layer_id))
    {
        return Err("Choose an unlocked, visible text layer".into());
    }
    let (bytes, index) = font_bytes(&draft.font).ok_or_else(|| {
        format!(
            "Font '{}' is unavailable. Choose an installed font.",
            draft.font
        )
    })?;
    let paths = outline_text(&bytes, index, &draft.text, draft.size)?;
    if paths.is_empty() {
        return Err("The chosen font has no outlines for this text".into());
    }
    let source = format!(
        "{FONT_SOURCE_PREFIX}{}",
        serde_json::to_string(&TextSource {
            text: draft.text.clone(),
            size: draft.size,
            font: draft.font.clone(),
        })
        .map_err(|e| e.to_string())?
    );
    if let Some(mut asset_id) = draft.asset_id {
        let Some(mut index) = app
            .state
            .project
            .assets
            .iter()
            .position(|asset| matches!(asset, Asset::Vector(v) if v.asset_id == asset_id))
        else {
            return Err("Text object no longer exists".into());
        };
        if let Asset::Vector(vector) = &app.state.project.assets[index] {
            if vector.paths == paths
                && vector.fill == Some(draft.color)
                && app
                    .state
                    .project
                    .asset_names
                    .get(&asset_id)
                    .and_then(|name| decode_text_source(name))
                    .is_some_and(|s| {
                        s.text == draft.text && s.font == draft.font && s.size == draft.size
                    })
            {
                return Ok(());
            }
        }
        app.history.snapshot(&app.state.project);
        if let Some(reference) = draft.placement {
            let placement_idx = crate::tools::materialize_placement_keyframe_for_edit(
                &mut app.state.project,
                reference.q0rg_id,
                reference.layer_id,
                reference.placement_idx,
                draft.frame,
            )
            .ok_or("Text placement no longer exists")?;
            let references = app
                .state
                .project
                .q0rgs
                .iter()
                .flat_map(|q| &q.layers)
                .flat_map(|l| &l.placements)
                .filter(|p| p.target == Target::Asset(asset_id))
                .count();
            if references > 1 {
                let new_id = crate::tools::next_asset_id_pub(&app.state.project);
                let Asset::Vector(mut vector) = app.state.project.assets[index].clone() else {
                    unreachable!()
                };
                vector.asset_id = new_id;
                index = app.state.project.assets.len();
                app.state.project.assets.push(Asset::Vector(vector));
                #[cfg(feature = "appearance-mask-eraser")]
                crate::appearance::clone_asset_appearance(&mut app.state.project, asset_id, new_id);
                app.state
                    .project
                    .q0rgs
                    .iter_mut()
                    .find(|q| q.q0rg_id == reference.q0rg_id)
                    .unwrap()
                    .layers
                    .iter_mut()
                    .find(|l| l.layer_id == reference.layer_id)
                    .unwrap()
                    .placements[placement_idx]
                    .target = Target::Asset(new_id);
                asset_id = new_id;
            }
            app.session.selection = Selection::Placement {
                q0rg_id: reference.q0rg_id,
                layer_id: reference.layer_id,
                placement_idx,
            };
        }
        if let Asset::Vector(vector) = &mut app.state.project.assets[index] {
            vector.paths = paths;
            vector.fill = Some(draft.color);
        }
        app.state.project.asset_names.insert(asset_id, source);
        app.textures.invalidate_asset(asset_id);
        app.state.mark_dirty();
        app.session.status = "Text updated".into();
    } else {
        let q0rg_id = draft.q0rg_id;
        let layer_id = draft.layer_id;
        let frame = draft.frame;
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

fn outline_text(font: &[u8], index: u32, text: &str, size: f32) -> Result<Vec<Path>, String> {
    let face = Face::parse(font, index).map_err(|_| "Unable to read the system font")?;
    let scale = size / f32::from(face.units_per_em());
    let mut builder = GlyphPaths {
        paths: Vec::new(),
        current: Vec::new(),
        x: 0.0,
        baseline: size,
        scale,
    };
    let mut previous_glyph = None;
    for character in text.chars() {
        if character == '\n' {
            builder.x = 0.0;
            builder.baseline += size * 1.25;
            previous_glyph = None;
            continue;
        }
        if let Some(glyph) = face.glyph_index(character) {
            if let (Some(previous), Some(kern)) = (previous_glyph, face.tables().kern) {
                for subtable in kern.subtables {
                    if subtable.horizontal
                        && !subtable.variable
                        && !subtable.has_cross_stream
                        && !subtable.has_state_machine
                    {
                        builder.x +=
                            f32::from(subtable.glyphs_kerning(previous, glyph).unwrap_or(0))
                                * scale;
                    }
                }
            }
            face.outline_glyph(glyph, &mut builder);
            previous_glyph = Some(glyph);
            builder.x += f32::from(
                face.glyph_hor_advance(glyph)
                    .unwrap_or(face.units_per_em() / 2),
            ) * scale;
        } else {
            previous_glyph = None;
            builder.x += size * 0.5;
        }
    }
    builder.flush();
    Ok(builder.paths)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn canvas_frame(app: &mut EditorApp, ctx: &Context, events: Vec<egui::Event>) {
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(600.0, 400.0),
                )),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default()
                    .frame(egui::Frame::none())
                    .show(ctx, |ui| {
                        let (rect, response) = ui.allocate_exact_size(
                            egui::vec2(600.0, 400.0),
                            egui::Sense::click_and_drag(),
                        );
                        let view = crate::render::StageView {
                            origin: egui::Pos2::ZERO,
                            scale: 1.0,
                            stage_rect: rect,
                        };
                        if !render_canvas(app, ui, &view) {
                            handle_stage(
                                app,
                                &response,
                                response.hover_pos().map(|p| Vec2::new(p.x, p.y)),
                            );
                        }
                    });
            },
        );
    }

    #[test]
    fn canvas_text_commits_once_on_outside_click_and_remains_editable() {
        if font_bytes(&default_font()).is_none() {
            return;
        }
        let mut app = EditorApp::default();
        app.session.text_editor = Some(TextDraft {
            origin: Vec2::new(30.0, 40.0),
            ..Default::default()
        });
        let ctx = Context::default();
        canvas_frame(&mut app, &ctx, vec![]);
        canvas_frame(
            &mut app,
            &ctx,
            vec![egui::Event::Text("hello\nworld".into())],
        );
        assert_eq!(
            app.session.text_editor.as_ref().unwrap().text,
            "hello\nworld"
        );
        assert!(
            app.state.project.assets.is_empty(),
            "typing should remain one uncommitted gesture"
        );
        let outside = egui::pos2(500.0, 300.0);
        for pressed in [true, false] {
            canvas_frame(
                &mut app,
                &ctx,
                vec![
                    egui::Event::PointerMoved(outside),
                    egui::Event::PointerButton {
                        pos: outside,
                        button: PointerButton::Primary,
                        pressed,
                        modifiers: Default::default(),
                    },
                ],
            );
        }
        assert!(
            app.session.text_editor.is_none(),
            "the apply click must not start a second text box"
        );
        assert_eq!(app.state.project.assets.len(), 1);
        let id = selected_asset_id(&app, &app.session.selection).unwrap();
        assert_eq!(
            decode_source(&app.state.project.asset_names[&id])
                .unwrap()
                .0,
            "hello\nworld"
        );
        begin_edit(&mut app, id);
        assert_eq!(
            app.session.text_editor.as_ref().unwrap().origin,
            Vec2::new(30.0, 40.0)
        );
        canvas_frame(&mut app, &ctx, vec![]);
        canvas_frame(
            &mut app,
            &ctx,
            vec![egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Default::default(),
            }],
        );
        assert!(app.session.text_editor.is_none());
        assert_eq!(app.state.project.assets.len(), 1);
    }

    #[test]
    fn editing_held_text_preserves_the_previous_frame() {
        if font_bytes(&default_font()).is_none() {
            return;
        }
        let mut app = EditorApp::default();
        commit(
            &mut app,
            &TextDraft {
                text: "first".into(),
                ..Default::default()
            },
        )
        .unwrap();
        let id = selected_asset_id(&app, &app.session.selection).unwrap();
        app.session.current_frame = 3;
        begin_edit(&mut app, id);
        app.session.text_editor.as_mut().unwrap().text = "later".into();
        assert!(finish_edit(&mut app));
        let edited_id = selected_asset_id(&app, &app.session.selection).unwrap();
        assert_ne!(id, edited_id);
        assert_eq!(
            decode_source(&app.state.project.asset_names[&id])
                .unwrap()
                .0,
            "first"
        );
        assert_eq!(
            decode_source(&app.state.project.asset_names[&edited_id])
                .unwrap()
                .0,
            "later"
        );
    }

    #[test]
    fn empty_new_canvas_text_does_not_create_an_asset_or_history_entry() {
        let mut app = EditorApp::default();
        let before = app.state.project.clone();
        app.session.text_editor = Some(TextDraft::default());
        assert!(finish_edit(&mut app));
        assert_eq!(app.state.project, before);
    }

    #[test]
    fn text_source_round_trips_unicode_and_newlines() {
        let source = encode_source("Привет\nq0e", 48.0);
        assert_eq!(decode_source(&source), Some(("Привет\nq0e".into(), 48.0)));
        assert_eq!(display_name(&source), Some("Text: Привет".into()));
    }

    #[test]
    fn vector_text_survives_project_save_and_remains_editable() {
        let Some(_) = font_bytes(&default_font()) else {
            return;
        }; // Windows system font; outline logic is exercised on supported hosts.
        let mut app = EditorApp::default();
        let draft = TextDraft {
            asset_id: None,
            origin: Vec2::new(24.0, 42.0),
            text: "Hello q0e".into(),
            size: 48.0,
            font: default_font(),
            font_query: default_font(),
            font_picker_open: false,
            font_previews: Default::default(),
            color: Rgba {
                r: 20,
                g: 30,
                b: 40,
                a: 255,
            },
            focus_requested: false,
            ..Default::default()
        };
        commit(&mut app, &draft).expect("insert outlined text");
        let selected =
            selected_asset_id(&app, &app.session.selection).expect("editable selected text");
        let bytes = q0s_format::v2::write(&app.state.project).expect("write q1s");
        let project = q0s_format::v2::parse(&bytes).expect("read q1s");
        assert_eq!(
            decode_source(&project.asset_names[&selected]),
            Some((draft.text.clone(), 48.0))
        );
        assert!(
            matches!(project.assets.iter().find(|asset| asset.id() == selected),
            Some(Asset::Vector(vector)) if !vector.paths.is_empty() && vector.fill == Some(draft.color))
        );
        assert_eq!(
            decode_text_source(&project.asset_names[&selected])
                .unwrap()
                .font,
            draft.font
        );
        app.state.project = project;
        begin_edit(&mut app, selected);
        let mut edited = app.session.text_editor.take().expect("reopen text");
        assert_eq!(edited.font, draft.font);
        let original = app
            .state
            .project
            .assets
            .iter()
            .find(|a| a.id() == selected)
            .unwrap()
            .clone();
        edited.font = "Times New Roman".into();
        if font_bytes(&edited.font).is_some() {
            commit(&mut app, &edited).expect("change font");
            assert_ne!(
                app.state
                    .project
                    .assets
                    .iter()
                    .find(|a| a.id() == selected)
                    .unwrap(),
                &original
            );
            begin_edit(&mut app, selected);
            assert_eq!(
                app.session.text_editor.as_ref().unwrap().font,
                "Times New Roman"
            );
        }
    }

    #[test]
    fn malformed_text_metadata_is_rejected_without_panicking() {
        assert!(decode_source("\u{1f}q0text:1:48:а1").is_none());
        assert!(
            decode_source("\u{1f}q0text:2:{\"text\":\"hello\",\"size\":0,\"font\":\"Arial\"}")
                .is_none()
        );
    }
}
