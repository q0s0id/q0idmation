//! Your own pointers: pixel drawings made in the cursor maker (Settings >
//! Appearance > Cursors), one per pointer state, each with its hotspot.
//! They hold still: no sway, no animation, drawn crisp on the screen's
//! pixels.

use std::collections::BTreeMap;

use egui::{Rect, Ui};

use super::*;

/// Every pointer state, in the order the maker lists them.
pub(crate) const EVERY: [CursorKind; 41] = [
    CursorKind::Arrow,
    CursorKind::Subselect,
    CursorKind::Link,
    CursorKind::Text,
    CursorKind::Grab,
    CursorKind::Grabbing,
    CursorKind::Move,
    CursorKind::Crosshair,
    CursorKind::ResizeH,
    CursorKind::ResizeV,
    CursorKind::ResizeNwSe,
    CursorKind::ResizeNeSw,
    CursorKind::Rotate,
    CursorKind::SkewH,
    CursorKind::SkewV,
    CursorKind::Scrub,
    CursorKind::Pen,
    CursorKind::Pencil,
    CursorKind::Bucket,
    CursorKind::Eyedropper,
    CursorKind::Knife,
    CursorKind::Lasso,
    CursorKind::Line,
    CursorKind::Rect,
    CursorKind::Oval,
    CursorKind::RasterBrush,
    CursorKind::RasterEraser,
    CursorKind::Smudge,
    CursorKind::LibraryDrop,
    CursorKind::PenClose,
    CursorKind::ScreenPick,
    CursorKind::FrameDrag,
    CursorKind::ParentLink,
    CursorKind::ParentUnlink,
    CursorKind::KnifeCut,
    CursorKind::SelectAdd,
    CursorKind::SelectRemove,
    CursorKind::CopyDrag,
    CursorKind::Pivot,
    CursorKind::Snap,
    CursorKind::Locked,
];

/// The name a drawing is kept under: one per state, the diagonals and the
/// shape tools each their own.
pub(crate) fn key(kind: CursorKind) -> String {
    format!("{kind:?}")
}

/// The largest drawing, in pixels each way.
pub(crate) const MAX_SIDE: usize = 64;
pub(crate) const SIDES: [usize; 4] = [16, 24, 32, 48];

/// A pointer you drew.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PixelCursor {
    pub(crate) width: u8,
    pub(crate) height: u8,
    /// The pixel that points.
    pub(crate) hotspot: [u8; 2],
    /// Straight RGBA, row by row.
    #[serde(with = "rgba_text")]
    pub(crate) rgba: Vec<u8>,
}

/// Pixels as base64 in the settings file.
mod rgba_text {
    use base64::Engine;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(rgba: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&base64::engine::general_purpose::STANDARD.encode(rgba))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
        let text = String::deserialize(deserializer)?;
        base64::engine::general_purpose::STANDARD
            .decode(text.as_bytes())
            .map_err(serde::de::Error::custom)
    }
}

impl PixelCursor {
    pub(crate) fn blank(side: usize) -> Self {
        let side = side.clamp(4, MAX_SIDE);
        Self {
            width: side as u8,
            height: side as u8,
            hotspot: [0, 0],
            rgba: vec![0; side * side * 4],
        }
    }

    pub(crate) fn is_valid(&self) -> bool {
        let (w, h) = (usize::from(self.width), usize::from(self.height));
        w > 0
            && h > 0
            && w <= MAX_SIDE
            && h <= MAX_SIDE
            && self.rgba.len() == w * h * 4
            && usize::from(self.hotspot[0]) < w
            && usize::from(self.hotspot[1]) < h
    }

    fn index(&self, x: usize, y: usize) -> Option<usize> {
        (x < usize::from(self.width) && y < usize::from(self.height))
            .then(|| (y * usize::from(self.width) + x) * 4)
    }

    pub(crate) fn pixel(&self, x: usize, y: usize) -> Color32 {
        self.index(x, y).map_or(Color32::TRANSPARENT, |i| {
            Color32::from_rgba_unmultiplied(
                self.rgba[i],
                self.rgba[i + 1],
                self.rgba[i + 2],
                self.rgba[i + 3],
            )
        })
    }

    pub(crate) fn set(&mut self, x: usize, y: usize, color: Color32) {
        if let Some(i) = self.index(x, y) {
            let [r, g, b, a] = color.to_srgba_unmultiplied();
            self.rgba[i..i + 4].copy_from_slice(&[r, g, b, a]);
        }
    }

    /// Floods the area of one colour under `(x, y)` with `color`.
    pub(crate) fn fill(&mut self, x: usize, y: usize, color: Color32) {
        let Some(start) = self.index(x, y) else {
            return;
        };
        let target: [u8; 4] = self.rgba[start..start + 4].try_into().expect("four bytes");
        let [r, g, b, a] = color.to_srgba_unmultiplied();
        if target == [r, g, b, a] {
            return;
        }
        let (w, h) = (usize::from(self.width), usize::from(self.height));
        let mut stack = vec![(x, y)];
        while let Some((x, y)) = stack.pop() {
            let Some(i) = self.index(x, y) else {
                continue;
            };
            if self.rgba[i..i + 4] != target {
                continue;
            }
            self.rgba[i..i + 4].copy_from_slice(&[r, g, b, a]);
            if x > 0 {
                stack.push((x - 1, y));
            }
            if x + 1 < w {
                stack.push((x + 1, y));
            }
            if y > 0 {
                stack.push((x, y - 1));
            }
            if y + 1 < h {
                stack.push((x, y + 1));
            }
        }
    }

    /// Mirrored left to right, its hotspot with it.
    pub(crate) fn mirrored(&self) -> Self {
        let mut out = self.clone();
        let w = usize::from(self.width);
        for y in 0..usize::from(self.height) {
            for x in 0..w {
                out.set(w - 1 - x, y, self.pixel(x, y));
            }
        }
        out.hotspot[0] = (w - 1 - usize::from(self.hotspot[0])) as u8;
        out
    }

    /// The same drawing on a canvas `side` pixels square, kept at the top
    /// left.
    pub(crate) fn resized(&self, side: usize) -> Self {
        let mut out = Self::blank(side);
        for y in 0..usize::from(out.height).min(usize::from(self.height)) {
            for x in 0..usize::from(out.width).min(usize::from(self.width)) {
                out.set(x, y, self.pixel(x, y));
            }
        }
        out.hotspot = [
            self.hotspot[0].min(out.width - 1),
            self.hotspot[1].min(out.height - 1),
        ];
        out
    }

    /// A retro pack sprite, to start a drawing from.
    pub(crate) fn from_sprite(sprite: &super::packs::Sprite) -> Self {
        let width = sprite.width.clamp(1, MAX_SIDE);
        let height = sprite.height.clamp(1, MAX_SIDE);
        let mut out = Self {
            width: width as u8,
            height: height as u8,
            hotspot: [
                sprite.hotspot.0.clamp(0, width as i32 - 1) as u8,
                sprite.hotspot.1.clamp(0, height as i32 - 1) as u8,
            ],
            rgba: vec![0; width * height * 4],
        };
        for y in 0..height {
            for x in 0..width {
                if let Some(c) = sprite.cells[y * sprite.width + x] {
                    out.set(x, y, c);
                }
            }
        }
        out
    }

    /// An image file's pixels, shrunk to fit when bigger than the maker
    /// allows.
    pub(crate) fn from_image(image: &image::RgbaImage) -> Self {
        let (w, h) = image.dimensions();
        let image = if w as usize > MAX_SIDE || h as usize > MAX_SIDE {
            let k = MAX_SIDE as f32 / w.max(h) as f32;
            image::imageops::resize(
                image,
                ((w as f32 * k).round() as u32).max(1),
                ((h as f32 * k).round() as u32).max(1),
                image::imageops::FilterType::Triangle,
            )
        } else {
            image.clone()
        };
        let (w, h) = image.dimensions();
        Self {
            width: w as u8,
            height: h as u8,
            hotspot: [0, 0],
            rgba: image.into_raw(),
        }
    }

    fn sprite(&self) -> super::packs::Sprite {
        let (w, h) = (usize::from(self.width), usize::from(self.height));
        let cells = (0..w * h)
            .map(|i| {
                let c = self.pixel(i % w, i / w);
                (c.a() > 0).then_some(c)
            })
            .collect();
        super::packs::Sprite {
            width: w,
            height: h,
            hotspot: (i32::from(self.hotspot[0]), i32::from(self.hotspot[1])),
            cells,
            glow: vec![None; w * h],
        }
    }
}

/// Draws your pointer: one screen pixel (times the size setting) a pixel,
/// still.
pub(super) fn paint(shapes: &mut Vec<Shape>, cursor: &PixelCursor, at: Pos2, scale: f32) {
    if !cursor.is_valid() {
        return;
    }
    let cell = super::packs::cell_size(scale / super::packs::pixels_per_point().max(0.5));
    super::packs::paint_cells(shapes, &cursor.sprite(), at, cell, false, |_, c| c);
}

// ---------------------------------------------------------------------------
// The cursor maker

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pen {
    Draw,
    Erase,
    Fill,
    Pick,
    Hotspot,
}

#[derive(Clone, Copy)]
struct MakerState {
    kind: usize,
    pen: Pen,
    color: Color32,
    side: usize,
}

impl Default for MakerState {
    fn default() -> Self {
        Self {
            kind: 0,
            pen: Pen::Draw,
            color: Color32::BLACK,
            side: 24,
        }
    }
}

/// The cursor maker, for Settings. Returns true when a drawing changed and
/// should be saved.
pub(crate) fn maker(
    ui: &mut Ui,
    own: &mut BTreeMap<String, PixelCursor>,
    theme: &crate::settings::Theme,
) -> bool {
    let id = ui.id().with("cursor_maker");
    let mut state = ui
        .data(|d| d.get_temp::<MakerState>(id))
        .unwrap_or_default();
    let mut changed = false;
    let kind = EVERY[state.kind.min(EVERY.len() - 1)];
    ui.horizontal_wrapped(|ui| {
        egui::ComboBox::from_id_source(id.with("kind"))
            .selected_text(kind.label())
            .width(190.0)
            .show_ui(ui, |ui| {
                for (index, kind) in EVERY.iter().enumerate() {
                    let drawn = own.contains_key(&key(*kind));
                    let label = if drawn {
                        format!("{}  \u{2022}", kind.label())
                    } else {
                        kind.label().to_string()
                    };
                    ui.selectable_value(&mut state.kind, index, label);
                }
            });
        ui.label(
            egui::RichText::new(if own.contains_key(&key(kind)) {
                "drawn"
            } else {
                "not drawn: shows the standard one"
            })
            .small()
            .weak(),
        );
    });
    let kind = EVERY[state.kind.min(EVERY.len() - 1)];
    let name = key(kind);
    ui.horizontal_wrapped(|ui| {
        crate::color_picker_ui::segmented(
            ui,
            250.0,
            &mut state.pen,
            &[
                (Pen::Draw, "Draw"),
                (Pen::Erase, "Erase"),
                (Pen::Fill, "Fill"),
                (Pen::Pick, "Pick"),
                (Pen::Hotspot, "Hotspot"),
            ],
        );
        egui::color_picker::color_edit_button_srgba(
            ui,
            &mut state.color,
            egui::color_picker::Alpha::OnlyBlend,
        );
    });
    ui.horizontal_wrapped(|ui| {
        let current_side = own
            .get(&name)
            .map(|c| usize::from(c.width.max(c.height)))
            .unwrap_or(state.side);
        egui::ComboBox::from_id_source(id.with("side"))
            .selected_text(format!("{current_side} px"))
            .width(70.0)
            .show_ui(ui, |ui| {
                for side in SIDES {
                    if ui
                        .selectable_label(current_side == side, format!("{side} px"))
                        .clicked()
                    {
                        state.side = side;
                        if let Some(cursor) = own.get_mut(&name) {
                            *cursor = cursor.resized(side);
                            changed = true;
                        }
                    }
                }
            });
        let pal = Palette::from_theme(theme);
        if ui
            .button("Start from Retro")
            .on_hover_text("Copy the Retro pack's pixels for this pointer, to change as you like")
            .clicked()
        {
            let sprite = super::packs::retro_sprite(kind, pal, &Motion::REST);
            own.insert(name.clone(), PixelCursor::from_sprite(&sprite));
            changed = true;
        }
        #[cfg(not(target_os = "android"))]
        if ui
            .button("Import image\u{2026}")
            .on_hover_text("A PNG, GIF, BMP or WebP; bigger than 64 px is shrunk to fit")
            .clicked()
        {
            let picked = crate::platform::FileDialog::new()
                .add_filter("Image", &["png", "gif", "bmp", "webp"])
                .pick_file();
            if let Some(path) = picked {
                if let Ok(image) = image::open(&path) {
                    own.insert(name.clone(), PixelCursor::from_image(&image.to_rgba8()));
                    changed = true;
                }
            }
        }
        if own.contains_key(&name) {
            if ui.button("Mirror").clicked() {
                if let Some(cursor) = own.get_mut(&name) {
                    *cursor = cursor.mirrored();
                    changed = true;
                }
            }
            if ui.button("Clear").clicked() {
                if let Some(cursor) = own.get_mut(&name) {
                    *cursor = PixelCursor::blank(usize::from(cursor.width.max(cursor.height)));
                    changed = true;
                }
            }
            if ui
                .button("Remove")
                .on_hover_text("Back to the standard pointer for this state")
                .clicked()
            {
                own.remove(&name);
                changed = true;
            }
        }
    });
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        changed |= canvas(ui, own, &name, &mut state);
        ui.add_space(8.0);
        ui.vertical(|ui| {
            ui.label(egui::RichText::new("Real size").small().weak());
            let (rect, _) = ui.allocate_exact_size(vec2(72.0, 72.0), Sense::hover());
            ui.painter()
                .rect_filled(rect, 6.0, ui.visuals().extreme_bg_color);
            if let Some(cursor) = own.get(&name) {
                let mut shapes = Vec::new();
                super::packs::set_pixels_per_point(ui.ctx().pixels_per_point());
                let centre = rect.center()
                    - vec2(
                        f32::from(cursor.width) * 0.5 - f32::from(cursor.hotspot[0]),
                        f32::from(cursor.height) * 0.5 - f32::from(cursor.hotspot[1]),
                    ) / ui.ctx().pixels_per_point();
                paint(&mut shapes, cursor, centre, 1.0);
                ui.painter().with_clip_rect(rect).extend(shapes);
            }
            ui.label(
                egui::RichText::new(
                    "Left button paints, right button erases. The red cross is where it points.",
                )
                .small()
                .weak(),
            );
        });
    });
    ui.data_mut(|d| d.insert_temp(id, state));
    changed
}

/// The editing grid. True when a stroke on it finished.
fn canvas(
    ui: &mut Ui,
    own: &mut BTreeMap<String, PixelCursor>,
    name: &str,
    state: &mut MakerState,
) -> bool {
    let side_px = (ui.available_width() - 100.0).clamp(160.0, 300.0);
    let (rect, response) = ui.allocate_exact_size(vec2(side_px, side_px), Sense::click_and_drag());
    let (w, h) = own.get(name).map_or((state.side, state.side), |c| {
        (usize::from(c.width), usize::from(c.height))
    });
    let cell = side_px / w.max(h) as f32;
    let painter = ui.painter_at(rect);
    // A checkerboard behind clear pixels.
    for y in 0..h {
        for x in 0..w {
            let r = Rect::from_min_size(
                rect.min + vec2(x as f32 * cell, y as f32 * cell),
                vec2(cell, cell),
            );
            let light = (x + y) % 2 == 0;
            painter.rect_filled(
                r,
                0.0,
                if light {
                    Color32::from_gray(205)
                } else {
                    Color32::from_gray(170)
                },
            );
        }
    }
    if let Some(cursor) = own.get(name) {
        for y in 0..h {
            for x in 0..w {
                let c = cursor.pixel(x, y);
                if c.a() > 0 {
                    let r = Rect::from_min_size(
                        rect.min + vec2(x as f32 * cell, y as f32 * cell),
                        vec2(cell, cell),
                    );
                    painter.rect_filled(r, 0.0, c);
                }
            }
        }
        let hot = rect.min
            + vec2(
                (f32::from(cursor.hotspot[0]) + 0.5) * cell,
                (f32::from(cursor.hotspot[1]) + 0.5) * cell,
            );
        let red = Stroke::new(1.5_f32, Color32::from_rgb(230, 40, 40));
        painter.line_segment([hot - vec2(cell, 0.0), hot + vec2(cell, 0.0)], red);
        painter.line_segment([hot - vec2(0.0, cell), hot + vec2(0.0, cell)], red);
    }
    let at = response.hover_pos().filter(|p| rect.contains(*p)).map(|p| {
        let local = p - rect.min;
        ((local.x / cell) as usize, (local.y / cell) as usize)
    });
    if let Some((x, y)) = at {
        let r = Rect::from_min_size(
            rect.min + vec2(x as f32 * cell, y as f32 * cell),
            vec2(cell, cell),
        );
        painter.rect_stroke(
            r,
            0.0,
            Stroke::new(1.0_f32, ui.visuals().strong_text_color()),
        );
    }
    let (primary, secondary) = ui.input(|i| (i.pointer.primary_down(), i.pointer.secondary_down()));
    let active = response.is_pointer_button_down_on() || response.dragged() || response.clicked();
    if let (Some((x, y)), true) = (at, active && (primary || secondary || response.clicked())) {
        let cursor = own
            .entry(name.to_string())
            .or_insert_with(|| PixelCursor::blank(state.side));
        let erase = secondary || state.pen == Pen::Erase;
        match state.pen {
            _ if erase => cursor.set(x, y, Color32::TRANSPARENT),
            Pen::Draw => cursor.set(x, y, state.color),
            Pen::Fill => cursor.fill(x, y, state.color),
            Pen::Pick => state.color = cursor.pixel(x, y),
            Pen::Hotspot => cursor.hotspot = [x as u8, y as u8],
            Pen::Erase => {}
        }
        ui.ctx().request_repaint();
    }
    // Saved when the button comes up, not every frame of a stroke.
    response.drag_stopped() || response.clicked() || response.secondary_clicked()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_state_is_listed_once() {
        let mut keys: Vec<String> = EVERY.iter().map(|k| key(*k)).collect();
        keys.sort();
        keys.dedup();
        assert_eq!(keys.len(), EVERY.len());
        for kind in CursorKind::STATES.iter().chain(&CursorKind::ACTIONS) {
            assert!(EVERY.contains(kind), "{kind:?}");
        }
    }

    #[test]
    fn a_drawing_survives_the_settings_file() {
        let mut cursor = PixelCursor::blank(16);
        cursor.set(3, 4, Color32::from_rgb(200, 10, 20));
        cursor.hotspot = [3, 4];
        let text = serde_json::to_string(&cursor).unwrap();
        let back: PixelCursor = serde_json::from_str(&text).unwrap();
        assert_eq!(back, cursor);
        assert!(back.is_valid());
        assert_eq!(back.pixel(3, 4), Color32::from_rgb(200, 10, 20));
    }

    #[test]
    fn fill_mirror_and_resize_keep_the_drawing_sound() {
        let mut cursor = PixelCursor::blank(16);
        for x in 0..16 {
            cursor.set(x, 8, Color32::BLACK);
        }
        cursor.fill(0, 0, Color32::RED);
        assert_eq!(cursor.pixel(15, 7), Color32::RED);
        assert_eq!(cursor.pixel(0, 9), Color32::TRANSPARENT);
        cursor.hotspot = [2, 5];
        let mirrored = cursor.mirrored();
        assert_eq!(mirrored.hotspot, [13, 5]);
        let small = cursor.resized(8);
        assert!(small.is_valid());
        assert_eq!(small.width, 8);
    }

    #[test]
    fn big_images_are_shrunk_to_fit() {
        let image = image::RgbaImage::from_pixel(200, 100, image::Rgba([1, 2, 3, 255]));
        let cursor = PixelCursor::from_image(&image);
        assert!(cursor.is_valid());
        assert_eq!(usize::from(cursor.width), MAX_SIDE);
        assert_eq!(cursor.height, 32);
    }

    #[test]
    fn a_retro_sprite_starts_a_drawing() {
        let pal = Palette::from_theme(&crate::settings::Theme::default());
        let sprite = super::super::packs::retro_sprite(CursorKind::Arrow, pal, &Motion::REST);
        let cursor = PixelCursor::from_sprite(&sprite);
        assert!(cursor.is_valid());
        assert_eq!(cursor.hotspot, [0, 0]);
        assert!(cursor.pixel(0, 0).a() > 0);
    }
}
