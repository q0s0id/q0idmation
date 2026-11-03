//! Where everyone else in the room is, each in their own colour: their frame
//! on the timeline (and on the layer they are on), who is here on the
//! timeline bar, and an outline with their name on what they just changed.
use super::*;
use egui::{vec2, Align, Color32, Layout, Pos2, Rect, RichText, Stroke};
use q0online::ResourceKey;

/// How long a remote change stays outlined on the stage, then fades.
const EDIT_GLOW: Duration = Duration::from_millis(3500);
const EDIT_FADE: Duration = Duration::from_millis(900);
const KEEP_EDITS: usize = 24;

/// Someone's colour: the same on every screen, since it comes from their id.
pub(crate) fn person_colour(user_id: i64) -> Color32 {
    chat::name_colour(user_id, true)
}

/// A participant on the timeline you are looking at.
pub(crate) struct Marker {
    pub user_id: i64,
    pub layer_id: u16,
    pub frame: u16,
    pub colour: Color32,
}

#[derive(Clone, Debug)]
pub(super) struct RemoteEdit {
    pub(super) user_id: i64,
    q0rg_id: u16,
    layer_id: u16,
    at: Instant,
}

impl Online {
    /// Everyone else present on timeline `q0rg_id` of the room tab.
    pub(crate) fn markers(&self, tab: Option<u64>, q0rg_id: u16) -> Vec<Marker> {
        let me = self.actor();
        let Some(room) = self.room.as_ref().filter(|room| Some(room.tab_id) == tab) else {
            return Vec::new();
        };
        room.presence
            .iter()
            .filter(|row| Some(row.user_id) != me && row.presence.q0rg_id == Some(q0rg_id))
            .filter_map(|row| {
                Some(Marker {
                    user_id: row.user_id,
                    layer_id: row.presence.layer_id?,
                    frame: row.presence.frame.unwrap_or(0),
                    colour: person_colour(row.user_id),
                })
            })
            .collect()
    }

    /// Layers other people changed in these events, so the stage can show it.
    pub(super) fn note_remote_edits(&mut self, events: &[RoomEvent]) {
        let me = self.actor();
        let received = self
            .room
            .as_ref()
            .map_or(0, |room| room.received_revision());
        let now = Instant::now();
        for event in events
            .iter()
            .filter(|e| e.revision > received && Some(e.actor) != me)
        {
            for change in &event.patch.changes {
                if let ResourceKey::Layer { q0rg_id, layer_id } = change.key {
                    self.edits.retain(|edit| {
                        !(edit.q0rg_id == q0rg_id
                            && edit.layer_id == layer_id
                            && edit.user_id == event.actor)
                    });
                    self.edits.push(RemoteEdit {
                        user_id: event.actor,
                        q0rg_id,
                        layer_id,
                        at: now,
                    });
                }
            }
        }
        let extra = self.edits.len().saturating_sub(KEEP_EDITS);
        self.edits.drain(..extra);
    }
}

/// Outlines what others just changed on the current timeline, with their
/// name, in their colour, like team editors in games.
pub(crate) fn paint_stage_edits(
    app: &EditorApp,
    painter: &egui::Painter,
    view: &crate::render::StageView,
) {
    let online = &app.online;
    if online.edits.is_empty() || !online.editing_tab(app.active_project_tab_id()) {
        return;
    }
    let now = Instant::now();
    let q0rg_id = app.session.current_q0rg_id;
    let frame = app.session.current_frame;
    let mut lit = false;
    for edit in online.edits.iter().filter(|e| e.q0rg_id == q0rg_id) {
        let age = now.saturating_duration_since(edit.at);
        if age >= EDIT_GLOW + EDIT_FADE {
            continue;
        }
        lit = true;
        let fade = if age <= EDIT_GLOW {
            1.0
        } else {
            1.0 - (age - EDIT_GLOW).as_secs_f32() / EDIT_FADE.as_secs_f32()
        };
        let Some((min_x, min_y, max_x, max_y)) =
            crate::tools::layer_world_bounds(&app.state.project, q0rg_id, edit.layer_id, frame)
        else {
            continue;
        };
        let rect = view
            .stage_bounds_to_screen(
                q0s_format::v2::Vec2::new(min_x, min_y),
                q0s_format::v2::Vec2::new(max_x, max_y),
            )
            .expand(3.0);
        let colour = person_colour(edit.user_id).gamma_multiply(fade);
        painter.rect_stroke(rect, 3.0, Stroke::new(2.0_f32, colour));
        painter.rect_filled(rect, 3.0, colour.gamma_multiply(0.08));
        let name = online.person(edit.user_id).username;
        let font = egui::FontId::proportional(12.0);
        let galley = painter.layout_no_wrap(name, font, crate::theme::on_accent(colour));
        let tag = Rect::from_min_size(
            Pos2::new(rect.left(), rect.top() - galley.size().y - 4.0),
            galley.size() + vec2(10.0, 4.0),
        );
        painter.rect_filled(tag, 3.0, colour);
        painter.galley(tag.min + vec2(5.0, 2.0), galley, Color32::WHITE);
    }
    if lit {
        painter
            .ctx()
            .request_repaint_after(Duration::from_millis(50));
    }
}

/// The people on this timeline, each a colour-ringed picture on the
/// timeline bar. Clicking one jumps to their frame.
pub(crate) fn timeline_people(app: &mut EditorApp, ui: &mut egui::Ui) {
    let tab = app.active_project_tab_id();
    let markers = app.online.markers(tab, app.session.current_q0rg_id);
    let online = &mut app.online;
    let everyone: Vec<(i64, Option<(u16, u16)>)> = match online.room.as_ref() {
        Some(room) if Some(room.tab_id) == tab => room
            .presence
            .iter()
            .filter(|row| Some(row.user_id) != online.user.as_ref().map(|u| u.id))
            .map(|row| {
                (
                    row.user_id,
                    row.presence
                        .q0rg_id
                        .map(|q0rg| (q0rg, row.presence.frame.unwrap_or(0))),
                )
            })
            .collect(),
        _ => return,
    };
    if everyone.is_empty() {
        return;
    }
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        for (user_id, place) in everyone {
            let person = online.person(user_id);
            let colour = person_colour(user_id);
            let size = 20.0;
            let (rect, response) = ui.allocate_exact_size(vec2(size, size), egui::Sense::click());
            let painter = ui.painter();
            match online.avatars.get(&user_id) {
                Some(Avatar::Ready(texture)) => {
                    egui::Image::new(egui::load::SizedTexture::new(texture.id(), rect.size()))
                        .rounding(size * 0.5)
                        .paint_at(ui, rect.shrink(1.5));
                }
                found => {
                    painter.circle_filled(
                        rect.center(),
                        size * 0.5 - 1.5,
                        colour.gamma_multiply(0.55),
                    );
                    let initial = person
                        .username
                        .chars()
                        .find(|c| c.is_alphanumeric())
                        .map(|c| c.to_uppercase().to_string())
                        .unwrap_or_else(|| "?".into());
                    painter.text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        initial,
                        egui::FontId::proportional(11.0),
                        Color32::WHITE,
                    );
                    if found.is_none() {
                        online.request_avatar(user_id);
                    }
                }
            }
            ui.painter().circle_stroke(
                rect.center(),
                size * 0.5 - 0.5,
                Stroke::new(2.0_f32, colour),
            );
            let here = markers.iter().find(|m| m.user_id == user_id);
            let detail = match (here, place) {
                (Some(marker), _) => format!("frame {} · click to go there", marker.frame + 1),
                (None, Some(_)) => "on another timeline · click to go there".to_owned(),
                (None, None) => "in the room".to_owned(),
            };
            let response = response.on_hover_ui(|ui| {
                ui.label(RichText::new(&person.username).strong().color(colour));
                ui.label(RichText::new(&detail).small());
            });
            if response.clicked() {
                if let Some((q0rg_id, frame)) = place {
                    online.follow = Some(Follow {
                        q0rg_id,
                        frame,
                        navigated: false,
                    });
                }
            }
        }
        ui.label(RichText::new("here:").small().weak());
    });
}

/// Their frame on the timeline: a column wash in their colour and a solid
/// cell on the layer they are on.
pub(crate) fn paint_timeline_markers(
    painter: &egui::Painter,
    markers: &[Marker],
    layer_ids: &[u16],
    base_x: f32,
    first_row_y: f32,
    row_h: f32,
    frame_w: f32,
) {
    let rows = layer_ids.len() as f32;
    for marker in markers {
        let x0 = base_x + f32::from(marker.frame) * frame_w;
        let column = Rect::from_min_size(Pos2::new(x0, first_row_y), vec2(frame_w, rows * row_h));
        painter.rect_filled(column, 0.0, marker.colour.gamma_multiply(0.12));
        if let Some(row) = layer_ids.iter().position(|id| *id == marker.layer_id) {
            let cell = Rect::from_min_size(
                Pos2::new(x0, first_row_y + row as f32 * row_h),
                vec2(frame_w, row_h),
            )
            .shrink(1.0);
            painter.rect_filled(cell, 2.0, marker.colour.gamma_multiply(0.45));
            painter.rect_stroke(cell, 2.0, Stroke::new(1.5_f32, marker.colour));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_are_steady_per_person_and_differ_between_people() {
        assert_eq!(person_colour(7), person_colour(7));
        assert_ne!(person_colour(7), person_colour(8));
    }
}
