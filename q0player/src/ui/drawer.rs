//! The slide-out panels at the movie's sides. Tucked away, a drawer leaves
//! the movie the whole window. Resting the pointer at its edge for a moment
//! brings up a faint handle, a click there slides the drawer out over the
//! movie, and a click on the movie tucks it back.

use std::time::{Duration, Instant};

use egui::{pos2, vec2, Color32, Context, Id, Rect, Rounding, Sense, Stroke, Ui};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

/// How long the pointer rests at an edge before its handle shows.
const DWELL: Duration = Duration::from_millis(450);

pub struct Drawer {
    side: Side,
    pub open: bool,
    resting_since: Option<Instant>,
}

impl Drawer {
    pub fn new(side: Side) -> Self {
        Self {
            side,
            open: false,
            resting_since: None,
        }
    }

    /// Which way is into the movie.
    fn inward(&self) -> f32 {
        match self.side {
            Side::Left => 1.0,
            Side::Right => -1.0,
        }
    }

    /// The strip along `area`'s edge that a click opens the drawer from.
    fn edge(&self, area: Rect, touch: bool) -> Rect {
        let width = if touch { 22.0 } else { 14.0 };
        match self.side {
            Side::Left => Rect::from_min_max(area.min, pos2(area.left() + width, area.bottom())),
            Side::Right => Rect::from_min_max(pos2(area.right() - width, area.top()), area.max),
        }
    }

    /// Where the drawer is, `slide` of the way out.
    fn panel(&self, area: Rect, width: f32, slide: f32) -> Rect {
        let left = match self.side {
            Side::Left => area.left() - width * (1.0 - slide),
            Side::Right => area.right() - width * slide,
        };
        Rect::from_min_size(pos2(left, area.top()), vec2(width, area.height()))
    }

    /// Shows the drawer's edge, and the drawer itself over `area` while it
    /// is out, filled by `contents`.
    pub fn show(
        &mut self,
        ctx: &Context,
        area: Rect,
        width: f32,
        touch: bool,
        contents: impl FnOnce(&mut Ui),
    ) {
        let id = Id::new(("q0player_drawer", self.side == Side::Left));
        let width = width.min(area.width() * 0.85).max(1.0);
        let edge = self.edge(area, touch);
        let now = Instant::now();
        let pointer = ctx.input(|input| input.pointer.hover_pos());
        if !self.open && pointer.is_some_and(|point| edge.contains(point)) {
            let since = *self.resting_since.get_or_insert(now);
            let rested = now.duration_since(since);
            if rested < DWELL {
                ctx.request_repaint_after(DWELL - rested);
            }
        } else {
            self.resting_since = None;
        }
        let rested = self
            .resting_since
            .is_some_and(|since| now.duration_since(since) >= DWELL);
        let peek = ctx.animate_bool_with_time(id.with("peek"), rested, 0.25);
        let slide = ctx.animate_bool_with_time(id.with("open"), self.open, 0.2);
        // Eases out: quick to leave the edge, gentle to settle.
        let slide = 1.0 - (1.0 - slide).powi(3);

        if !self.open {
            egui::Area::new(id.with("edge"))
                .fixed_pos(edge.min)
                .constrain(false)
                .show(ctx, |ui| {
                    let response = ui
                        .allocate_exact_size(edge.size(), Sense::click())
                        .1
                        .on_hover_cursor(if rested {
                            egui::CursorIcon::PointingHand
                        } else {
                            egui::CursorIcon::Default
                        });
                    if peek > 0.0 {
                        self.paint_handle(ui, edge, peek);
                    }
                    if response.clicked() {
                        self.open = true;
                        self.resting_since = None;
                    }
                });
        }

        if slide > 0.001 {
            let panel = self.panel(area, width, slide);
            egui::Area::new(id.with("panel"))
                .fixed_pos(panel.min)
                .constrain(false)
                .show(ctx, |ui| {
                    ui.set_clip_rect(panel.intersect(area).expand2(vec2(40.0, 0.0)));
                    let shadow = egui::epaint::Shadow {
                        offset: vec2(6.0 * self.inward(), 0.0),
                        blur: 24.0,
                        spread: 0.0,
                        color: Color32::from_black_alpha((110.0 * slide) as u8),
                    };
                    egui::Frame::none()
                        .fill(ui.visuals().panel_fill)
                        .inner_margin(egui::Margin::symmetric(10.0, 8.0))
                        .shadow(shadow)
                        .show(ui, |ui| {
                            ui.set_width(width - 20.0);
                            ui.set_height(area.height() - 16.0);
                            contents(ui);
                        });
                });
            // A press on the movie tucks the drawer back. Presses outside
            // the movie, on menus and the transport, leave it be.
            let pressed = ctx.input(|input| {
                input
                    .pointer
                    .press_origin()
                    .filter(|_| input.pointer.any_pressed())
            });
            if self.open
                && pressed.is_some_and(|point| area.contains(point) && !panel.contains(point))
            {
                self.open = false;
            }
        }
    }

    /// The faint handle that shows when the pointer rests at the edge.
    fn paint_handle(&self, ui: &Ui, edge: Rect, peek: f32) {
        let painter = ui.painter();
        let ink = ui.visuals().strong_text_color();
        let inward = self.inward();
        let outer = if inward > 0.0 {
            edge.left()
        } else {
            edge.right()
        };
        // A glow along the edge, fading into the movie.
        for step in 0..8 {
            let reach = 26.0 - step as f32 * 3.0;
            let band = Rect::from_x_y_ranges(
                if inward > 0.0 {
                    outer..=outer + reach
                } else {
                    outer - reach..=outer
                },
                edge.y_range(),
            );
            painter.rect_filled(band, Rounding::ZERO, ink.gamma_multiply(0.012 * peek));
        }
        let centre = pos2(outer + inward * 7.0, edge.center().y);
        painter.rect_filled(
            Rect::from_center_size(centre, vec2(4.0, 58.0)),
            Rounding::same(2.0),
            ink.gamma_multiply(0.55 * peek),
        );
        let tip = centre + vec2(inward * 13.0, 0.0);
        let back = tip - vec2(inward * 5.0, 0.0);
        painter.add(egui::Shape::line(
            vec![back + vec2(0.0, -6.0), tip, back + vec2(0.0, 6.0)],
            Stroke::new(1.8_f32, ink.gamma_multiply(0.7 * peek)),
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drawers_slide_in_from_their_own_side() {
        let area = Rect::from_min_size(pos2(0.0, 20.0), vec2(800.0, 500.0));
        let left = Drawer::new(Side::Left);
        assert_eq!(left.panel(area, 200.0, 0.0).right(), 0.0, "tucked away");
        assert_eq!(left.panel(area, 200.0, 1.0).left(), 0.0, "all the way out");
        let right = Drawer::new(Side::Right);
        assert_eq!(right.panel(area, 200.0, 0.0).left(), 800.0);
        assert_eq!(right.panel(area, 200.0, 1.0).right(), 800.0);
        assert!(left.edge(area, false).max.x < 20.0);
        assert!(right.edge(area, false).min.x > 780.0);
    }
}
