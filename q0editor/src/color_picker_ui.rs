//! The colour picker's face, used by every colour box in the editor.
//!
//! On top: the old and new colour side by side (click the old one to go
//! back), the hex code, the square/wheel switch and the eyedropper. Then a
//! big saturation/brightness field, either a rounded square with capsule hue
//! and opacity sliders, or a hue ring around the square. Below, three tabs:
//! channel sliders whose tracks show where each value leads (RGB, HSV or
//! HSL), colour harmonies built from the current colour, and swatches
//! (recently used, saved, and the colours already in the project).
//!
//! The field, ring and tracks are small textures drawn on the CPU and shown
//! with egui's anti-aliased rounded rectangles; each is redrawn only when the
//! values it depends on change. Recent and saved colours, and the picker's
//! own choices (square or wheel, open tab), are kept in `colors.json` beside
//! the editor's settings.

use std::hash::{Hash, Hasher};
use std::sync::{Mutex, OnceLock};

use egui::epaint::RectShape;
use egui::{
    pos2, vec2, Align2, Color32, ColorImage, Context, FontId, Id, Painter, Pos2, Rect, Response,
    Rounding, Sense, Stroke, TextureHandle, TextureOptions, Ui, Vec2,
};
use serde::{Deserialize, Serialize};

/// How many recently used colours are remembered.
const RECENT: usize = 16;
/// How many of the project's colours are offered.
const PROJECT_COLORS: usize = 24;

// ---------------------------------------------------------------- colour maths

/// Hue, saturation and value (each 0..=1) to sRGB, in the values people see
/// rather than linear light, as every paint program's picker does.
pub(crate) fn hsv_to_rgb([h, s, v]: [f32; 3]) -> [f32; 3] {
    let h = h.rem_euclid(1.0) * 6.0;
    let sector = h.floor();
    let f = h - sector;
    let p = v * (1.0 - s);
    let q = v * (1.0 - s * f);
    let t = v * (1.0 - s * (1.0 - f));
    match sector as i32 {
        0 => [v, t, p],
        1 => [q, v, p],
        2 => [p, v, t],
        3 => [p, q, v],
        4 => [t, p, v],
        _ => [v, p, q],
    }
}

pub(crate) fn rgb_to_hsv([r, g, b]: [f32; 3]) -> [f32; 3] {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let h = if d <= 0.0 {
        0.0
    } else if max == r {
        ((g - b) / d).rem_euclid(6.0) / 6.0
    } else if max == g {
        ((b - r) / d + 2.0) / 6.0
    } else {
        ((r - g) / d + 4.0) / 6.0
    };
    let s = if max <= 0.0 { 0.0 } else { d / max };
    [h, s, max]
}

fn hsv_to_hsl([h, s, v]: [f32; 3]) -> [f32; 3] {
    let l = v * (1.0 - s / 2.0);
    let s = if l <= 0.0 || l >= 1.0 {
        0.0
    } else {
        (v - l) / l.min(1.0 - l)
    };
    [h, s, l]
}

fn hsl_to_hsv([h, s, l]: [f32; 3]) -> [f32; 3] {
    let v = l + s * l.min(1.0 - l);
    let s = if v <= 0.0 { 0.0 } else { 2.0 * (1.0 - l / v) };
    [h, s, v]
}

pub(crate) fn byte(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}

pub(crate) fn color_of(rgb: [f32; 3], alpha: f32) -> Color32 {
    Color32::from_rgba_unmultiplied(byte(rgb[0]), byte(rgb[1]), byte(rgb[2]), byte(alpha))
}

pub(crate) fn rgb_of(color: Color32) -> [f32; 3] {
    let [r, g, b, _] = color.to_srgba_unmultiplied();
    [r, g, b].map(|channel| f32::from(channel) / 255.0)
}

fn opaque(color: Color32) -> Color32 {
    let [r, g, b, _] = color.to_srgba_unmultiplied();
    Color32::from_rgb(r, g, b)
}

/// `#RRGGBB`, or `#RRGGBBAA` when the colour is see-through and may be.
/// `Color32` is premultiplied, so a nearly clear colour has lost its RGB:
/// the picker's own codes come from `hex_of_values` instead.
fn hex_of(color: Color32, with_alpha: bool) -> String {
    let [r, g, b, a] = color.to_srgba_unmultiplied();
    hex_of_values([r, g, b], a, with_alpha)
}

pub(crate) fn hex_of_values([r, g, b]: [u8; 3], a: u8, with_alpha: bool) -> String {
    if with_alpha && a < 255 {
        format!("{r:02X}{g:02X}{b:02X}{a:02X}")
    } else {
        format!("{r:02X}{g:02X}{b:02X}")
    }
}

/// A typed hex code: 3, 6 or 8 digits, with or without `#`. Without its own
/// alpha it keeps `alpha`.
pub(crate) fn parse_hex(text: &str, alpha: u8) -> Option<Color32> {
    let digits: String = text
        .trim()
        .trim_start_matches('#')
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    if !digits.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let pair = |i: usize| u8::from_str_radix(&digits[i..i + 2], 16).ok();
    match digits.len() {
        3 => {
            let one = |i: usize| u8::from_str_radix(&digits[i..=i], 16).ok().map(|v| v * 17);
            Some(Color32::from_rgba_unmultiplied(
                one(0)?,
                one(1)?,
                one(2)?,
                alpha,
            ))
        }
        6 => Some(Color32::from_rgba_unmultiplied(
            pair(0)?,
            pair(2)?,
            pair(4)?,
            alpha,
        )),
        8 => Some(Color32::from_rgba_unmultiplied(
            pair(0)?,
            pair(2)?,
            pair(4)?,
            pair(6)?,
        )),
        _ => None,
    }
}

// ---------------------------------------------------------------- preferences

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
enum Tab {
    #[default]
    Sliders,
    Harmony,
    Swatches,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
enum Channels {
    #[default]
    Rgb,
    Hsv,
    Hsl,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
enum Harmony {
    #[default]
    Complement,
    Analogous,
    Triad,
    Split,
    Square,
    Shades,
}

impl Harmony {
    const ALL: [Harmony; 6] = [
        Harmony::Complement,
        Harmony::Analogous,
        Harmony::Triad,
        Harmony::Split,
        Harmony::Square,
        Harmony::Shades,
    ];

    fn label(self) -> &'static str {
        match self {
            Harmony::Complement => "Complement",
            Harmony::Analogous => "Analogous",
            Harmony::Triad => "Triad",
            Harmony::Split => "Split",
            Harmony::Square => "Square",
            Harmony::Shades => "Shades",
        }
    }

    fn hint(self) -> &'static str {
        match self {
            Harmony::Complement => "The opposite hue, with tints of both",
            Harmony::Analogous => "Neighbouring hues, 30° apart",
            Harmony::Triad => "Three hues evenly round the wheel",
            Harmony::Split => "The two hues either side of the opposite one",
            Harmony::Square => "Four hues evenly round the wheel",
            Harmony::Shades => "The same hue, darker and lighter",
        }
    }

    /// The colours this rule makes from `hsv`, the current colour among them.
    fn colors(self, [h, s, v]: [f32; 3]) -> Vec<[f32; 3]> {
        let turn = |offset: f32| [h + offset, s, v];
        match self {
            Harmony::Complement => vec![
                [h, s * 0.45, (v + (1.0 - v) * 0.5)],
                [h, s, v],
                turn(0.5),
                [h + 0.5, s * 0.45, (v + (1.0 - v) * 0.5)],
                [h + 0.5, s, v * 0.6],
            ],
            Harmony::Analogous => vec![
                turn(-2.0 / 12.0),
                turn(-1.0 / 12.0),
                [h, s, v],
                turn(1.0 / 12.0),
                turn(2.0 / 12.0),
            ],
            Harmony::Triad => vec![
                [h, s, v],
                turn(1.0 / 3.0),
                turn(2.0 / 3.0),
                [h + 1.0 / 3.0, s * 0.5, v],
                [h + 2.0 / 3.0, s * 0.5, v],
            ],
            Harmony::Split => vec![
                [h, s, v],
                turn(5.0 / 12.0),
                turn(7.0 / 12.0),
                [h + 5.0 / 12.0, s * 0.5, v],
                [h + 7.0 / 12.0, s * 0.5, v],
            ],
            Harmony::Square => vec![[h, s, v], turn(0.25), turn(0.5), turn(0.75)],
            Harmony::Shades => vec![
                [h, s, v * 0.3],
                [h, s, v * 0.62],
                [h, s, v],
                [h, s * 0.65, v + (1.0 - v) * 0.45],
                [h, s * 0.3, v + (1.0 - v) * 0.8],
            ],
        }
        .into_iter()
        .map(|[h, s, v]| [h.rem_euclid(1.0), s.clamp(0.0, 1.0), v.clamp(0.0, 1.0)])
        .collect()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct Prefs {
    recent: Vec<[u8; 4]>,
    saved: Vec<[u8; 4]>,
    wheel: bool,
    tab: Tab,
    channels: Channels,
    harmony: Harmony,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            recent: Vec::new(),
            saved: starter_palette(),
            wheel: false,
            tab: Tab::default(),
            channels: Channels::default(),
            harmony: Harmony::default(),
        }
    }
}

/// The saved swatches someone sees before saving any of their own.
fn starter_palette() -> Vec<[u8; 4]> {
    // TODO(human): the colours every new q0editor user starts with, as
    // 0xRRGGBB. These twelve are a placeholder: greys, then a rainbow.
    [
        0x000000, 0x5F5F6E, 0xB4B4C0, 0xFFFFFF, 0xE8344E, 0xF28A2E, 0xF7D046, 0x5BC25A, 0x2FB3C9,
        0x3D6FE0, 0x8A4FD8, 0xF06AB4,
    ]
    .into_iter()
    .map(|rgb: u32| [(rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8, 255])
    .collect()
}

fn prefs_cell() -> &'static Mutex<Prefs> {
    static PREFS: OnceLock<Mutex<Prefs>> = OnceLock::new();
    PREFS.get_or_init(|| Mutex::new(load_prefs()))
}

/// Beside settings.json; none under tests, like the settings themselves.
fn prefs_path() -> Option<std::path::PathBuf> {
    crate::settings::Settings::path().map(|path| path.with_file_name("colors.json"))
}

fn load_prefs() -> Prefs {
    prefs_path()
        .and_then(|path| std::fs::read(path).ok())
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn prefs() -> Prefs {
    prefs_cell()
        .lock()
        .map(|prefs| prefs.clone())
        .unwrap_or_default()
}

fn store_prefs(prefs: Prefs) {
    let changed = match prefs_cell().lock() {
        Ok(mut slot) if *slot != prefs => {
            *slot = prefs.clone();
            true
        }
        _ => false,
    };
    if !changed {
        return;
    }
    if let Some(path) = prefs_path() {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(json) = serde_json::to_vec_pretty(&prefs) {
            let _ = std::fs::write(path, json);
        }
    }
}

fn push_recent(prefs: &mut Prefs, color: Color32) {
    let rgba = color.to_srgba_unmultiplied();
    prefs.recent.retain(|existing| *existing != rgba);
    prefs.recent.insert(0, rgba);
    prefs.recent.truncate(RECENT);
}

/// The saved swatches, as unmultiplied RGBA: the same list as the
/// picker's Swatches tab, kept in `colors.json`.
pub(crate) fn saved_swatches() -> Vec<[u8; 4]> {
    prefs().saved
}

/// Adds a colour to the saved swatches, unless it is there already.
pub(crate) fn save_swatch(rgba: [u8; 4]) {
    let mut prefs = prefs();
    if !prefs.saved.contains(&rgba) {
        prefs.saved.push(rgba);
        store_prefs(prefs);
    }
}

/// Removes the saved swatch at `index`.
pub(crate) fn forget_swatch(index: usize) {
    let mut prefs = prefs();
    if index < prefs.saved.len() {
        prefs.saved.remove(index);
        store_prefs(prefs);
    }
}

/// "No colour": white with a red line corner to corner, the way paint
/// programs have always shown it.
pub(crate) fn paint_none(painter: &Painter, rect: Rect, rounding: f32) {
    painter.rect_filled(rect, rounding, Color32::WHITE);
    let inset = rect.shrink((rect.width().min(rect.height()) * 0.08).max(1.0));
    painter.line_segment(
        [inset.left_bottom(), inset.right_top()],
        Stroke::new(
            (rect.width().min(rect.height()) * 0.09).clamp(1.25, 2.5),
            Color32::from_rgb(0xE0, 0x23, 0x2E),
        ),
    );
}

// ---------------------------------------------------------------- per-popup state

/// What one picker remembers while it is open: the colour as hue, saturation
/// and value (so a grey keeps its hue and black its saturation), the colour it
/// opened with, and the hex text being typed.
#[derive(Clone, Debug)]
struct Memory {
    hsv: [f32; 3],
    alpha: f32,
    shown: Color32,
    original: Color32,
    hex: String,
    hex_editing: bool,
}

impl Memory {
    fn new(color: Color32, with_alpha: bool) -> Self {
        let mut memory = Self {
            hsv: [0.0, 0.0, 0.0],
            alpha: 1.0,
            shown: color,
            original: color,
            hex: String::new(),
            hex_editing: false,
        };
        memory.sync(color, with_alpha);
        memory
    }

    /// Takes on a colour set from outside (or typed), keeping the hue of a
    /// grey and the saturation of black.
    fn sync(&mut self, color: Color32, with_alpha: bool) {
        let [h, s, v] = rgb_to_hsv(rgb_of(color));
        if v > 1.0e-4 {
            if s > 1.0e-4 {
                self.hsv[0] = h;
            }
            self.hsv[1] = s;
        }
        self.hsv[2] = v;
        self.alpha = f32::from(color.to_srgba_unmultiplied()[3]) / 255.0;
        self.shown = color;
        if !self.hex_editing {
            self.hex = self.hex(with_alpha);
        }
    }

    /// The hex code of the values themselves, not of the premultiplied colour.
    fn hex(&self, with_alpha: bool) -> String {
        hex_of_values(hsv_to_rgb(self.hsv).map(byte), byte(self.alpha), with_alpha)
    }

    fn color(&self) -> Color32 {
        color_of(hsv_to_rgb(self.hsv), self.alpha)
    }

    /// Sets hue, saturation and value; true when the colour changed.
    fn set_hsv(&mut self, hsv: [f32; 3], with_alpha: bool) -> bool {
        self.hsv = [
            hsv[0].rem_euclid(1.0),
            hsv[1].clamp(0.0, 1.0),
            hsv[2].clamp(0.0, 1.0),
        ];
        self.commit(with_alpha)
    }

    fn commit(&mut self, with_alpha: bool) -> bool {
        let color = self.color();
        let changed = color != self.shown;
        self.shown = color;
        if !self.hex_editing {
            self.hex = self.hex(with_alpha);
        }
        changed
    }
}

fn registry_id(owner: Id) -> Id {
    owner.with("color_picker_ui_keys")
}

/// Ends the pickers a popup opened: a colour that changed joins the recent
/// ones. Call when the popup is closed.
pub(crate) fn finish(ctx: &Context, owner: Id) {
    let Some(keys) = ctx.data_mut(|data| data.remove_temp::<Vec<Id>>(registry_id(owner))) else {
        return;
    };
    let mut prefs = prefs();
    let mut changed = false;
    for key in keys {
        let memory = ctx.data_mut(|data| {
            let memory = data.get_temp::<Memory>(key);
            data.remove::<Memory>(key);
            memory
        });
        if let Some(memory) = memory {
            if memory.shown != memory.original {
                push_recent(&mut prefs, memory.shown);
                changed = true;
            }
        }
    }
    if changed {
        store_prefs(prefs);
    }
}

/// Whether a popup has an open picker that still needs `finish`.
pub(crate) fn has_open(ctx: &Context, owner: Id) -> bool {
    ctx.data(|data| data.get_temp::<Vec<Id>>(registry_id(owner)).is_some())
}

// ---------------------------------------------------------------- project colours

fn project_colors_id() -> Id {
    Id::new("color_picker_ui_project_colors")
}

/// The colours the project's drawings use most, for the Swatches tab. Cheap
/// to call every frame: it counts again only when `key` changes.
pub(crate) fn note_project(
    ctx: &Context,
    project: &q0s_format::v2::ProjectV2,
    key: (Option<u64>, u64),
) {
    let current =
        ctx.data(|data| data.get_temp::<((Option<u64>, u64), Vec<Color32>)>(project_colors_id()));
    if current.as_ref().is_some_and(|(seen, _)| *seen == key) {
        return;
    }
    let mut counts: std::collections::HashMap<[u8; 4], usize> = std::collections::HashMap::new();
    let mut add = |rgba: q0s_format::v2::Rgba| {
        if rgba.a > 0 {
            *counts.entry([rgba.r, rgba.g, rgba.b, rgba.a]).or_default() += 1;
        }
    };
    for asset in &project.assets {
        if let q0s_format::v2::Asset::Vector(vector) = asset {
            if let Some(fill) = vector.fill {
                add(fill);
            }
            if let Some(stroke) = &vector.stroke {
                add(stroke.color);
            }
        }
    }
    let mut colors: Vec<([u8; 4], usize)> = counts.into_iter().collect();
    colors.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let colors: Vec<Color32> = colors
        .into_iter()
        .take(PROJECT_COLORS)
        .map(|([r, g, b, a], _)| Color32::from_rgba_unmultiplied(r, g, b, a))
        .collect();
    ctx.data_mut(|data| data.insert_temp(project_colors_id(), (key, colors)));
}

fn project_colors(ctx: &Context) -> Vec<Color32> {
    ctx.data(|data| data.get_temp::<((Option<u64>, u64), Vec<Color32>)>(project_colors_id()))
        .map(|(_, colors)| colors)
        .unwrap_or_default()
}

// ---------------------------------------------------------------- drawing helpers

/// Sizes, larger on touch screens.
#[derive(Clone, Copy)]
struct Metrics {
    width: f32,
    field: f32,
    bar: f32,
    track: f32,
    thumb: f32,
    swatch: f32,
    number: f32,
}

fn metrics() -> Metrics {
    if crate::touch::ENABLED {
        Metrics {
            width: 300.0,
            field: 190.0,
            bar: 20.0,
            track: 14.0,
            thumb: 11.0,
            swatch: 30.0,
            number: 54.0,
        }
    } else {
        Metrics {
            width: 256.0,
            field: 164.0,
            bar: 13.0,
            track: 9.0,
            thumb: 7.5,
            swatch: 21.0,
            number: 46.0,
        }
    }
}

/// The popup's content width, for the gradient editor around the picker.
pub(crate) fn width() -> f32 {
    metrics().width
}

pub(crate) fn hash_of(values: &[f32]) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for value in values {
        ((value * 4096.0).round() as i64).hash(&mut hasher);
    }
    hasher.finish()
}

/// A texture painted pixel by pixel at screen resolution, kept until `key`
/// or the size changes. `pixel` gets the position in points within `size`.
pub(crate) fn texture(
    ctx: &Context,
    id: Id,
    key: u64,
    size: Vec2,
    pixel: impl Fn(f32, f32) -> Color32,
) -> TextureHandle {
    let scale = ctx.pixels_per_point();
    let pixels = [
        ((size.x * scale).round() as usize).max(1),
        ((size.y * scale).round() as usize).max(1),
    ];
    let cached = ctx.data(|data| data.get_temp::<(u64, [usize; 2], TextureHandle)>(id));
    if let Some((seen, seen_pixels, handle)) = &cached {
        if *seen == key && *seen_pixels == pixels {
            return handle.clone();
        }
    }
    let mut image = ColorImage::new(pixels, Color32::TRANSPARENT);
    for y in 0..pixels[1] {
        for x in 0..pixels[0] {
            image.pixels[y * pixels[0] + x] =
                pixel((x as f32 + 0.5) / scale, (y as f32 + 0.5) / scale);
        }
    }
    let handle = match cached {
        Some((_, _, mut handle)) => {
            handle.set(image, TextureOptions::LINEAR);
            handle
        }
        None => ctx.load_texture(format!("{id:?}"), image, TextureOptions::LINEAR),
    };
    ctx.data_mut(|data| data.insert_temp(id, (key, pixels, handle.clone())));
    handle
}

/// A texture in a rounded rectangle, its edges anti-aliased.
pub(crate) fn image(
    painter: &Painter,
    rect: Rect,
    rounding: impl Into<Rounding>,
    handle: &TextureHandle,
) {
    let mut shape = RectShape::filled(rect, rounding, Color32::WHITE);
    shape.fill_texture_id = handle.id();
    shape.uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
    painter.add(shape);
}

fn checker_at(x: f32, y: f32) -> Color32 {
    let cell = 4.0;
    if ((x / cell).floor() as i32 + (y / cell).floor() as i32) % 2 == 0 {
        Color32::from_gray(214)
    } else {
        Color32::from_gray(150)
    }
}

/// `over` on the checkerboard at (x, y).
fn over_checker(over: Color32, x: f32, y: f32) -> Color32 {
    let [r, g, b, a] = over.to_srgba_unmultiplied();
    let under = checker_at(x, y);
    let t = f32::from(a) / 255.0;
    let mix =
        |top: u8, bottom: u8| (f32::from(top) * t + f32::from(bottom) * (1.0 - t)).round() as u8;
    Color32::from_rgb(mix(r, under.r()), mix(g, under.g()), mix(b, under.b()))
}

/// A colour tile: the colour over a checkerboard when it is see-through.
pub(crate) fn tile(ui: &Ui, rect: Rect, rounding: f32, color: Color32) {
    let painter = ui.painter();
    if color.a() < 255 {
        let checker = texture(
            ui.ctx(),
            Id::new((
                "color_picker_ui_checker",
                rect.size().x as i32,
                rect.size().y as i32,
            )),
            0,
            rect.size(),
            checker_at,
        );
        image(painter, rect, rounding, &checker);
    }
    painter.rect_filled(rect, rounding, color);
}

/// A slider thumb: a soft shadow, a white ring and the colour inside.
pub(crate) fn thumb(painter: &Painter, center: Pos2, radius: f32, fill: Color32) {
    painter.circle_filled(
        center + vec2(0.0, 1.0),
        radius + 1.5,
        Color32::from_black_alpha(55),
    );
    painter.circle_filled(center, radius, Color32::WHITE);
    painter.circle_filled(center, (radius - 2.25).max(1.0), opaque(fill));
    painter.circle_stroke(
        center,
        radius,
        Stroke::new(1.0_f32, Color32::from_black_alpha(80)),
    );
}

pub(crate) fn outline(ui: &Ui) -> Stroke {
    Stroke::new(1.0_f32, ui.visuals().widgets.noninteractive.bg_stroke.color)
}

// ---------------------------------------------------------------- small icons

fn paint_eyedropper(painter: &Painter, rect: Rect, color: Color32) {
    let c = rect.center();
    let s = rect.width().min(rect.height()) / 19.0;
    let at = |x: f32, y: f32| c + vec2(x * s, y * s);
    painter.line_segment([at(-6.0, 6.0), at(-1.0, 1.0)], Stroke::new(1.6 * s, color));
    painter.line_segment([at(-1.5, 1.5), at(3.0, -3.0)], Stroke::new(3.2 * s, color));
    painter.circle_filled(at(4.2, -4.2), 2.6 * s, color);
    painter.line_segment([at(0.5, -5.0), at(5.0, -0.5)], Stroke::new(1.6 * s, color));
}

fn paint_square_icon(painter: &Painter, rect: Rect, color: Color32) {
    let r = Rect::from_center_size(rect.center(), vec2(12.0, 12.0));
    painter.rect_stroke(r, 2.5_f32, Stroke::new(1.5_f32, color));
    painter.circle_stroke(
        r.center() + vec2(1.5, -1.5),
        2.2,
        Stroke::new(1.2_f32, color),
    );
}

fn paint_wheel_icon(painter: &Painter, rect: Rect, color: Color32) {
    painter.circle_stroke(rect.center(), 6.0, Stroke::new(1.5_f32, color));
    painter.rect_stroke(
        Rect::from_center_size(rect.center(), vec2(5.0, 5.0)),
        1.0,
        Stroke::new(1.2_f32, color),
    );
}

fn paint_close_icon(painter: &Painter, rect: Rect, color: Color32) {
    let c = rect.center();
    let d = 4.0;
    let stroke = Stroke::new(1.6_f32, color);
    painter.line_segment([c + vec2(-d, -d), c + vec2(d, d)], stroke);
    painter.line_segment([c + vec2(-d, d), c + vec2(d, -d)], stroke);
}

fn paint_plus_icon(painter: &Painter, rect: Rect, color: Color32) {
    let c = rect.center();
    let d = rect.width().min(rect.height()) * 0.26;
    let stroke = Stroke::new(1.8_f32, color);
    painter.line_segment([c + vec2(-d, 0.0), c + vec2(d, 0.0)], stroke);
    painter.line_segment([c + vec2(0.0, -d), c + vec2(0.0, d)], stroke);
}

/// Whether pickers show the hue wheel rather than the square: one choice,
/// remembered, for every picker and the Color panel.
pub(crate) fn prefers_wheel() -> bool {
    prefs().wheel
}

#[cfg(test)]
pub(crate) fn set_prefers_wheel(wheel: bool) {
    let mut prefs = prefs();
    prefs.wheel = wheel;
    store_prefs(prefs);
}

/// The square and wheel buttons: which field the pickers show.
pub(crate) fn field_buttons(ui: &mut Ui) {
    let mut prefs = prefs();
    let before = prefs.wheel;
    if icon_button(ui, prefs.wheel, "Hue wheel", paint_wheel_icon).clicked() {
        prefs.wheel = true;
    }
    if icon_button(ui, !prefs.wheel, "Square", paint_square_icon).clicked() {
        prefs.wheel = false;
    }
    if prefs.wheel != before {
        store_prefs(prefs);
    }
}

/// The side of the small icon buttons.
pub(crate) fn icon_button_side() -> f32 {
    if crate::touch::ENABLED {
        32.0
    } else {
        24.0
    }
}

/// A small square button with a painted icon.
fn icon_button(
    ui: &mut Ui,
    selected: bool,
    hint: &str,
    paint: impl Fn(&Painter, Rect, Color32),
) -> Response {
    let side = icon_button_side();
    let (rect, response) = ui.allocate_exact_size(vec2(side, side), Sense::click());
    let visuals = ui.style().interact_selectable(&response, selected);
    if selected || response.hovered() || response.has_focus() {
        ui.painter()
            .rect(rect, 6.0_f32, visuals.weak_bg_fill, visuals.bg_stroke);
    }
    paint(ui.painter(), rect, visuals.fg_stroke.color);
    response.on_hover_text(hint)
}

/// Equal-width segments, one of them on.
pub(crate) fn segmented<T: Copy + PartialEq>(
    ui: &mut Ui,
    width: f32,
    value: &mut T,
    options: &[(T, &str)],
) -> bool {
    segmented_enabled(ui, width, value, options, |_| None)
}

/// `segmented`, where `why_off` names the options that cannot be chosen
/// now, and says why on hover.
pub(crate) fn segmented_enabled<T: Copy + PartialEq>(
    ui: &mut Ui,
    width: f32,
    value: &mut T,
    options: &[(T, &str)],
    why_off: impl Fn(T) -> Option<&'static str>,
) -> bool {
    let height = if crate::touch::ENABLED { 30.0 } else { 22.0 };
    let (rect, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    let visuals = ui.visuals().clone();
    ui.painter().rect(
        rect,
        height * 0.5,
        visuals.extreme_bg_color,
        Stroke::new(1.0_f32, visuals.widgets.noninteractive.bg_stroke.color),
    );
    let step = rect.width() / options.len() as f32;
    let mut changed = false;
    for (index, (option, label)) in options.iter().enumerate() {
        let cell = Rect::from_min_size(
            pos2(rect.left() + step * index as f32, rect.top()),
            vec2(step, height),
        )
        .shrink(2.0);
        let off = why_off(*option);
        let response = ui.interact(
            cell,
            ui.id().with(("segment", index, *label)),
            if off.is_some() {
                Sense::hover()
            } else {
                Sense::click()
            },
        );
        let on = *value == *option;
        if on {
            ui.painter()
                .rect_filled(cell, cell.height() * 0.5, visuals.selection.bg_fill);
        } else if response.hovered() && off.is_none() {
            ui.painter().rect_filled(
                cell,
                cell.height() * 0.5,
                visuals.widgets.hovered.weak_bg_fill,
            );
        }
        let text_color = if on {
            visuals.selection.stroke.color
        } else if off.is_some() {
            visuals.weak_text_color().gamma_multiply(0.6)
        } else {
            visuals.text_color()
        };
        let response = match off {
            Some(why) => response.on_hover_text(why),
            None => response,
        };
        ui.painter().text(
            cell.center(),
            Align2::CENTER_CENTER,
            *label,
            FontId::proportional(if crate::touch::ENABLED { 14.0 } else { 12.0 }),
            text_color,
        );
        if response.clicked() && !on {
            *value = *option;
            changed = true;
        }
    }
    changed
}

/// A rounded ramp of `color(t)` left to right, over a checkerboard where it
/// is see-through. `id` names its texture.
pub(crate) fn ramp(ui: &Ui, rect: Rect, rounding: f32, id: Id, color: impl Fn(f32) -> Color32) {
    let samples: Vec<f32> = (0..=24)
        .flat_map(|i| {
            let [r, g, b, a] = color(i as f32 / 24.0).to_srgba_unmultiplied();
            [r, g, b, a].map(f32::from)
        })
        .collect();
    let width = rect.width();
    let handle = texture(ui.ctx(), id, hash_of(&samples), rect.size(), |x, y| {
        over_checker(color((x / width).clamp(0.0, 1.0)), x, y)
    });
    image(ui.painter(), rect, rounding, &handle);
    ui.painter().rect_stroke(rect, rounding, outline(ui));
}

/// The ring a hovered tile gets.
pub(crate) fn hover_ring(ui: &Ui, rect: Rect, rounding: f32) {
    ui.painter().rect_stroke(
        rect.expand(2.0),
        rounding + 2.0,
        Stroke::new(1.5_f32, ui.visuals().strong_text_color()),
    );
}

/// A gradient stop under its bar: a round pin pointing up, its colour
/// inside a white ring; the selected one wears an accent ring as well.
pub(crate) fn stop_pin(
    painter: &Painter,
    center: Pos2,
    radius: f32,
    color: Color32,
    selected: bool,
    hovered: bool,
    accent: Color32,
) {
    let tip = center - vec2(0.0, radius + 4.5);
    painter.add(egui::Shape::convex_polygon(
        vec![
            tip,
            center + vec2(radius * 0.62, -radius * 0.55),
            center + vec2(-radius * 0.62, -radius * 0.55),
        ],
        Color32::WHITE,
        Stroke::NONE,
    ));
    painter.circle_filled(
        center + vec2(0.0, 1.0),
        radius + 1.5,
        Color32::from_black_alpha(60),
    );
    painter.circle_filled(center, radius, Color32::WHITE);
    let inner = radius - 2.25;
    if color.a() < 255 {
        painter.circle_filled(center, inner, Color32::from_gray(180));
        painter.circle_filled(
            center + vec2(-inner * 0.35, -inner * 0.35),
            inner * 0.45,
            Color32::from_gray(130),
        );
        painter.circle_filled(
            center + vec2(inner * 0.35, inner * 0.35),
            inner * 0.45,
            Color32::from_gray(130),
        );
    }
    painter.circle_filled(center, inner, color);
    painter.circle_stroke(
        center,
        radius,
        Stroke::new(1.0_f32, Color32::from_black_alpha(80)),
    );
    if selected {
        painter.circle_stroke(center, radius + 2.5, Stroke::new(2.0_f32, accent));
    } else if hovered {
        painter.circle_stroke(
            center,
            radius + 2.0,
            Stroke::new(1.0_f32, Color32::from_white_alpha(160)),
        );
    }
}

/// A small button with an arrow pointing `degrees` clockwise from right.
pub(crate) fn arrow_button(ui: &mut Ui, degrees: f32, selected: bool) -> Response {
    let side = if crate::touch::ENABLED { 30.0 } else { 22.0 };
    let (rect, response) = ui.allocate_exact_size(vec2(side, side), Sense::click());
    let visuals = ui.style().interact_selectable(&response, selected);
    if selected || response.hovered() {
        ui.painter()
            .rect(rect, 5.0_f32, visuals.weak_bg_fill, visuals.bg_stroke);
    }
    let (sin, cos) = degrees.to_radians().sin_cos();
    let along = vec2(cos, sin);
    let across = vec2(-sin, cos);
    let c = rect.center();
    let reach = side * 0.28;
    let stroke = Stroke::new(1.6_f32, visuals.fg_stroke.color);
    let head = c + along * reach;
    ui.painter().line_segment([c - along * reach, head], stroke);
    ui.painter()
        .line_segment([head, head - along * 4.0 + across * 3.5], stroke);
    ui.painter()
        .line_segment([head, head - along * 4.0 - across * 3.5], stroke);
    response
}

/// The popup's close button.
pub(crate) fn close_button(ui: &mut Ui) -> Response {
    icon_button(ui, false, "Close (Esc)", paint_close_icon)
}

/// The colour under a drag on a horizontal capsule: 0..=1 between its caps.
fn capsule_value(rect: Rect, x: f32) -> f32 {
    let inset = rect.height() * 0.5;
    ((x - rect.left() - inset) / (rect.width() - inset * 2.0).max(1.0)).clamp(0.0, 1.0)
}

fn capsule_x(rect: Rect, value: f32) -> f32 {
    let inset = rect.height() * 0.5;
    rect.left() + inset + value.clamp(0.0, 1.0) * (rect.width() - inset * 2.0)
}

/// A capsule slider whose track is painted by `track(t)` (t 0..=1 between
/// the caps); true when `value` changed.
fn capsule(
    ui: &mut Ui,
    id: Id,
    key: u64,
    size: Vec2,
    value: &mut f32,
    see_through: bool,
    track: impl Fn(f32) -> Color32,
) -> bool {
    let (rect, response) = ui.allocate_exact_size(size, Sense::click_and_drag());
    let inset = size.y * 0.5;
    let width = size.x;
    let handle = texture(ui.ctx(), id, key, size, |x, y| {
        let t = ((x - inset) / (width - inset * 2.0).max(1.0)).clamp(0.0, 1.0);
        let color = track(t);
        if see_through {
            over_checker(color, x, y)
        } else {
            color
        }
    });
    image(ui.painter(), rect, inset, &handle);
    ui.painter().rect_stroke(rect, inset, outline(ui));
    let mut changed = false;
    if response.is_pointer_button_down_on() {
        if let Some(pointer) = response.interact_pointer_pos() {
            let next = capsule_value(rect, pointer.x);
            changed = (next - *value).abs() > f32::EPSILON;
            *value = next;
        }
    }
    let center = pos2(capsule_x(rect, *value), rect.center().y);
    let radius = (size.y * 0.5 + 2.5).max(metrics().thumb * 0.8);
    thumb(ui.painter(), center, radius, track(*value));
    changed
}

// ---------------------------------------------------------------- the picker

/// What the picker did this frame.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Outcome {
    /// The colour changed.
    pub(crate) changed: bool,
    /// The close button was pressed.
    pub(crate) close: bool,
}

/// How the picker sits in its popup.
pub(crate) struct Frame<'a> {
    /// The popup, whose closing ends the picker (see `finish`).
    pub(crate) owner: Id,
    /// This picker within the popup: one per gradient stop, say.
    pub(crate) key: Id,
    /// Whether the colour has an opacity to edit.
    pub(crate) alpha: bool,
    /// Whether to show a close button.
    pub(crate) closable: bool,
    /// On touch screens, whether the next tap outside takes its colour.
    pub(crate) armed: Option<&'a mut bool>,
}

/// The whole picker for `color`.
pub(crate) fn picker(ui: &mut Ui, color: &mut Color32, frame: Frame<'_>) -> Outcome {
    let Frame {
        owner,
        key,
        alpha: with_alpha,
        closable,
        mut armed,
    } = frame;
    let m = metrics();
    let ctx = ui.ctx().clone();
    // Remember this picker so closing the popup can file its colour.
    ctx.data_mut(|data| {
        let keys = data.get_temp_mut_or_default::<Vec<Id>>(registry_id(owner));
        if !keys.contains(&key) {
            keys.push(key);
        }
    });
    let mut memory = ctx
        .data(|data| data.get_temp::<Memory>(key))
        .unwrap_or_else(|| Memory::new(*color, with_alpha));
    if memory.shown != *color {
        memory.sync(*color, with_alpha);
    }
    let mut prefs = prefs();
    let prefs_before = prefs.clone();
    let mut outcome = Outcome::default();

    ui.vertical(|ui| {
        ui.set_width(m.width);
        ui.spacing_mut().item_spacing = vec2(6.0, 7.0);

        // ---- header: old | new, hex, square/wheel, eyedropper, close
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let pill = vec2(if crate::touch::ENABLED { 76.0 } else { 62.0 }, if crate::touch::ENABLED { 32.0 } else { 24.0 });
            let (rect, response) = ui.allocate_exact_size(pill, Sense::click());
            let half = rect.width() * 0.5;
            let old_rect = Rect::from_min_size(rect.min, vec2(half, rect.height()));
            let new_rect = Rect::from_min_max(pos2(rect.left() + half, rect.top()), rect.max);
            let radius = 7.0;
            let checker = texture(ui.ctx(), Id::new("color_picker_ui_pill"), 0, rect.size(), checker_at);
            image(ui.painter(), rect, radius, &checker);
            ui.painter().rect_filled(
                old_rect,
                Rounding { nw: radius, sw: radius, ne: 0.0, se: 0.0 },
                memory.original,
            );
            ui.painter().rect_filled(
                new_rect,
                Rounding { nw: 0.0, sw: 0.0, ne: radius, se: radius },
                memory.shown,
            );
            ui.painter().line_segment(
                [old_rect.right_top(), old_rect.right_bottom()],
                Stroke::new(1.0_f32, Color32::from_black_alpha(90)),
            );
            ui.painter().rect_stroke(rect, radius, outline(ui));
            let over_old = response
                .hover_pos()
                .is_some_and(|pointer| old_rect.contains(pointer));
            if over_old && memory.original != memory.shown {
                ui.painter().rect_stroke(
                    old_rect.shrink(1.5),
                    Rounding { nw: radius - 1.5, sw: radius - 1.5, ne: 0.0, se: 0.0 },
                    Stroke::new(1.5_f32, ui.visuals().strong_text_color()),
                );
            }
            let response = response.on_hover_text("Left: the colour it had · right: the new one. Click the left half to go back.");
            if response.clicked() && over_old && memory.original != memory.shown {
                memory.sync(memory.original, with_alpha);
                outcome.changed = true;
            }

            // Hex code.
            ui.label(
                egui::RichText::new("#")
                    .monospace()
                    .color(ui.visuals().weak_text_color()),
            );
            let hex = ui.add(
                egui::TextEdit::singleline(&mut memory.hex)
                    .font(egui::TextStyle::Monospace)
                    .desired_width(if crate::touch::ENABLED { 86.0 } else { 70.0 })
                    .char_limit(9)
                    .hint_text("RRGGBB"),
            )
            .on_hover_text("Hex code: 3, 6 or 8 digits (the last two are opacity)");
            memory.hex_editing = hex.has_focus();
            if hex.changed() {
                let alpha = byte(memory.alpha);
                if let Some(parsed) = parse_hex(&memory.hex, alpha) {
                    let parsed = if with_alpha { parsed } else { opaque(parsed) };
                    if parsed != memory.shown {
                        memory.sync(parsed, with_alpha);
                        outcome.changed = true;
                    }
                }
            }
            if hex.lost_focus() {
                memory.hex_editing = false;
                memory.hex = memory.hex(with_alpha);
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                if closable
                    && icon_button(ui, false, "Close (Esc)", paint_close_icon).clicked()
                {
                    outcome.close = true;
                }
                match armed.take() {
                    Some(armed) => {
                        if icon_button(ui, *armed, "Pick from the screen: then tap anywhere", paint_eyedropper)
                            .clicked()
                        {
                            *armed = !*armed;
                        }
                    }
                    None => {
                        icon_button(
                            ui,
                            false,
                            "Pick from the screen: click anywhere outside this palette, even in other windows",
                            paint_eyedropper,
                        );
                    }
                }
                if icon_button(ui, prefs.wheel, "Hue wheel", paint_wheel_icon).clicked() {
                    prefs.wheel = true;
                }
                if icon_button(ui, !prefs.wheel, "Square", paint_square_icon).clicked() {
                    prefs.wheel = false;
                }
            });
        });

        // ---- the field
        if prefs.wheel {
            outcome.changed |= wheel(ui, key, &mut memory, with_alpha, m);
        } else {
            outcome.changed |= square(ui, &mut memory, with_alpha, m);
            let mut hue = memory.hsv[0];
            let [_, s, v] = memory.hsv;
            if capsule(
                ui,
                Id::new("color_picker_ui_hue"),
                0,
                vec2(m.width, m.bar),
                &mut hue,
                false,
                |t| color_of(hsv_to_rgb([t, 1.0, 1.0]), 1.0),
            ) {
                outcome.changed |= memory.set_hsv([hue, s, v], with_alpha);
            }
        }
        if with_alpha {
            let rgb = hsv_to_rgb(memory.hsv);
            let mut alpha = memory.alpha;
            if capsule(
                ui,
                Id::new("color_picker_ui_alpha"),
                hash_of(&rgb),
                vec2(m.width, m.bar),
                &mut alpha,
                true,
                |t| color_of(rgb, t),
            ) {
                memory.alpha = alpha;
                outcome.changed |= memory.commit(with_alpha);
            }
        }

        // ---- tabs
        ui.add_space(2.0);
        segmented(
            ui,
            m.width,
            &mut prefs.tab,
            &[
                (Tab::Sliders, "Sliders"),
                (Tab::Harmony, "Harmony"),
                (Tab::Swatches, "Swatches"),
            ],
        );
        match prefs.tab {
            Tab::Sliders => outcome.changed |= sliders(ui, &mut memory, &mut prefs, with_alpha, m),
            Tab::Harmony => outcome.changed |= harmony(ui, &mut memory, &mut prefs, with_alpha, m),
            Tab::Swatches => outcome.changed |= swatches(ui, &mut memory, &mut prefs, with_alpha, m),
        }
    });

    if outcome.changed {
        *color = memory.shown;
    }
    ctx.data_mut(|data| data.insert_temp(key, memory));
    if prefs != prefs_before {
        store_prefs(prefs);
    }
    outcome
}

/// The saturation/brightness square with its round thumb.
fn square(ui: &mut Ui, memory: &mut Memory, with_alpha: bool, m: Metrics) -> bool {
    let size = vec2(m.width, m.field);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click_and_drag());
    let hue = memory.hsv[0];
    let handle = texture(
        ui.ctx(),
        Id::new("color_picker_ui_square"),
        hash_of(&[hue]),
        size,
        |x, y| color_of(hsv_to_rgb([hue, x / size.x, 1.0 - y / size.y]), 1.0),
    );
    image(ui.painter(), rect, 10.0_f32, &handle);
    ui.painter().rect_stroke(rect, 10.0_f32, outline(ui));
    let mut changed = false;
    if response.is_pointer_button_down_on() {
        if let Some(pointer) = response.interact_pointer_pos() {
            let s = ((pointer.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
            let v = 1.0 - ((pointer.y - rect.top()) / rect.height()).clamp(0.0, 1.0);
            changed = memory.set_hsv([hue, s, v], with_alpha);
        }
    }
    let center = pos2(
        rect.left() + memory.hsv[1] * rect.width(),
        rect.top() + (1.0 - memory.hsv[2]) * rect.height(),
    );
    thumb(ui.painter(), center, m.thumb + 1.0, memory.shown);
    changed
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum WheelPart {
    Ring,
    Square,
}

/// A hue ring with the saturation/brightness square inside it.
fn wheel(ui: &mut Ui, key: Id, memory: &mut Memory, with_alpha: bool, m: Metrics) -> bool {
    let side = m.field + 44.0;
    let (row, _) = ui.allocate_exact_size(vec2(m.width, side), Sense::hover());
    let rect = Rect::from_center_size(row.center(), vec2(side, side));
    match hue_wheel(ui, key, rect, memory.hsv, m.thumb + 0.5) {
        Some(next) => memory.set_hsv(next, with_alpha),
        None => false,
    }
}

/// A hue ring with the saturation/brightness square inside it, as big as
/// fits in `rect`, for the colour `hsv`. `key` names its state and
/// textures. Gives the new hue, saturation and value while it is dragged:
/// on the ring the hue changes, in the square the other two.
pub(crate) fn hue_wheel(
    ui: &mut Ui,
    key: Id,
    rect: Rect,
    hsv: [f32; 3],
    thumb_radius: f32,
) -> Option<[f32; 3]> {
    let side = rect.width().min(rect.height()).max(40.0);
    let rect = Rect::from_center_size(rect.center(), vec2(side, side));
    let response = ui.interact(rect, key.with("wheel"), Sense::click_and_drag());
    let outer = side * 0.5;
    let band = (if crate::touch::ENABLED {
        22.0
    } else {
        17.0_f32
    })
    .min(side * 0.13);
    let inner = outer - band;
    let center = rect.center();
    // Red at the top, the hues going clockwise.
    let angle_hue = |dx: f32, dy: f32| (dx.atan2(-dy) / std::f32::consts::TAU).rem_euclid(1.0);
    let ring = texture(
        ui.ctx(),
        key.with("wheel_ring"),
        hash_of(&[outer, inner]),
        rect.size(),
        |x, y| {
            let (dx, dy) = (x - outer, y - outer);
            let distance = (dx * dx + dy * dy).sqrt();
            let coverage =
                (distance - inner + 0.5).clamp(0.0, 1.0) * (outer - distance + 0.5).clamp(0.0, 1.0);
            if coverage <= 0.0 {
                return Color32::TRANSPARENT;
            }
            color_of(hsv_to_rgb([angle_hue(dx, dy), 1.0, 1.0]), coverage)
        },
    );
    image(ui.painter(), rect, 0.0_f32, &ring);
    let stroke = outline(ui);
    ui.painter().circle_stroke(center, outer - 0.5, stroke);
    ui.painter().circle_stroke(center, inner + 0.5, stroke);

    let half = (inner - 7.0).max(4.0) * std::f32::consts::FRAC_1_SQRT_2;
    let square_rect = Rect::from_center_size(center, vec2(half * 2.0, half * 2.0));
    let hue = hsv[0];
    let inside = texture(
        ui.ctx(),
        key.with("wheel_square"),
        hash_of(&[hue]),
        square_rect.size(),
        |x, y| {
            color_of(
                hsv_to_rgb([hue, x / (half * 2.0), 1.0 - y / (half * 2.0)]),
                1.0,
            )
        },
    );
    image(ui.painter(), square_rect, 6.0_f32, &inside);
    ui.painter().rect_stroke(square_rect, 6.0_f32, stroke);

    let drag_id = key.with("wheel_part");
    let mut next = None;
    if response.drag_started() || response.clicked() || response.is_pointer_button_down_on() {
        if let Some(pointer) = response.interact_pointer_pos() {
            let part = ui
                .data(|data| data.get_temp::<WheelPart>(drag_id))
                .filter(|_| !response.drag_started() && !response.clicked())
                .unwrap_or_else(|| {
                    if pointer.distance(center) >= inner - 2.0 {
                        WheelPart::Ring
                    } else {
                        WheelPart::Square
                    }
                });
            ui.data_mut(|data| data.insert_temp(drag_id, part));
            let [h, s, v] = hsv;
            next = Some(match part {
                WheelPart::Ring => [angle_hue(pointer.x - center.x, pointer.y - center.y), s, v],
                WheelPart::Square => [
                    h,
                    ((pointer.x - square_rect.left()) / square_rect.width()).clamp(0.0, 1.0),
                    1.0 - ((pointer.y - square_rect.top()) / square_rect.height()).clamp(0.0, 1.0),
                ],
            });
        }
    }
    if !response.is_pointer_button_down_on() {
        ui.data_mut(|data| data.remove::<WheelPart>(drag_id));
    }

    // The thumbs, where the colour is now.
    let [h, s, v] = next.unwrap_or(hsv);
    let angle = h * std::f32::consts::TAU;
    let on_ring = center + vec2(angle.sin(), -angle.cos()) * (inner + outer) * 0.5;
    thumb(
        ui.painter(),
        on_ring,
        thumb_radius,
        color_of(hsv_to_rgb([h, 1.0, 1.0]), 1.0),
    );
    let in_square = pos2(
        square_rect.left() + s * square_rect.width(),
        square_rect.top() + (1.0 - v) * square_rect.height(),
    );
    thumb(
        ui.painter(),
        in_square,
        thumb_radius,
        color_of(hsv_to_rgb([h, s, v]), 1.0),
    );
    next
}

/// The Sliders tab: three channels (and opacity) with live tracks.
fn sliders(
    ui: &mut Ui,
    memory: &mut Memory,
    prefs: &mut Prefs,
    with_alpha: bool,
    m: Metrics,
) -> bool {
    segmented(
        ui,
        m.width * 0.55,
        &mut prefs.channels,
        &[
            (Channels::Rgb, "RGB"),
            (Channels::Hsv, "HSV"),
            (Channels::Hsl, "HSL"),
        ],
    );
    let mut changed = false;
    let rgb = hsv_to_rgb(memory.hsv);
    let hsl = hsv_to_hsl(memory.hsv);
    // (label, value 0..=1, shown maximum, unit, track colour for t)
    type Track = Box<dyn Fn(f32) -> Color32>;
    let rows: Vec<(&str, f32, f32, &str, Track)> = match prefs.channels {
        Channels::Rgb => (0..3)
            .map(|i| {
                let base = rgb;
                let track: Track = Box::new(move |t| {
                    let mut c = base;
                    c[i] = t;
                    color_of(c, 1.0)
                });
                (["R", "G", "B"][i], rgb[i], 255.0, "", track)
            })
            .collect(),
        Channels::Hsv => (0..3)
            .map(|i| {
                let base = memory.hsv;
                let track: Track = Box::new(move |t| {
                    let mut c = base;
                    c[i] = t;
                    if i == 0 {
                        c[1] = c[1].max(0.85);
                        c[2] = c[2].max(0.85);
                    }
                    color_of(hsv_to_rgb(c), 1.0)
                });
                (
                    ["H", "S", "V"][i],
                    memory.hsv[i],
                    [360.0, 100.0, 100.0][i],
                    ["°", "%", "%"][i],
                    track,
                )
            })
            .collect(),
        Channels::Hsl => (0..3)
            .map(|i| {
                let base = hsl;
                let track: Track = Box::new(move |t| {
                    let mut c = base;
                    c[i] = t;
                    if i == 0 {
                        c[1] = c[1].max(0.85);
                        c[2] = 0.5;
                    }
                    color_of(hsv_to_rgb(hsl_to_hsv(c)), 1.0)
                });
                (
                    ["H", "S", "L"][i],
                    hsl[i],
                    [360.0, 100.0, 100.0][i],
                    ["°", "%", "%"][i],
                    track,
                )
            })
            .collect(),
    };
    ui.spacing_mut().item_spacing.y = 4.0;
    let label_width = 14.0;
    let track_width = m.width - label_width - m.number - 12.0;
    for (index, (label, value, maximum, unit, track)) in rows.into_iter().enumerate() {
        let mut next = value;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let (label_rect, _) =
                ui.allocate_exact_size(vec2(label_width, m.bar.max(18.0)), Sense::hover());
            ui.painter().text(
                label_rect.center(),
                Align2::CENTER_CENTER,
                label,
                FontId::monospace(12.0),
                ui.visuals().weak_text_color(),
            );
            let key = hash_of(&[
                rgb[0],
                rgb[1],
                rgb[2],
                memory.hsv[0],
                prefs_channel_code(prefs.channels),
            ]);
            // The capsule moves `next`; the number below can too.
            capsule(
                ui,
                Id::new(("color_picker_ui_channel", index)),
                key,
                vec2(track_width, m.track),
                &mut next,
                false,
                &*track,
            );
            let mut shown = next * maximum;
            let response = ui.add_sized(
                vec2(m.number, 20.0),
                egui::DragValue::new(&mut shown)
                    .clamp_range(0.0..=maximum)
                    .speed(maximum / 255.0)
                    .max_decimals(0)
                    .suffix(unit),
            );
            if response.changed() {
                next = shown / maximum;
            }
        });
        if (next - value).abs() > 1.0e-6 {
            changed |= set_channel(memory, prefs.channels, index, next, with_alpha);
        }
    }
    if with_alpha {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let (label_rect, _) = ui.allocate_exact_size(vec2(label_width, 18.0), Sense::hover());
            ui.painter().text(
                label_rect.center(),
                Align2::CENTER_CENTER,
                "A",
                FontId::monospace(12.0),
                ui.visuals().weak_text_color(),
            );
            let (rest, _) = ui.allocate_exact_size(vec2(track_width, 18.0), Sense::hover());
            ui.painter().text(
                rest.left_center(),
                Align2::LEFT_CENTER,
                "Opacity",
                FontId::proportional(11.0),
                ui.visuals().weak_text_color(),
            );
            let mut percent = memory.alpha * 100.0;
            if ui
                .add_sized(
                    vec2(m.number, 20.0),
                    egui::DragValue::new(&mut percent)
                        .clamp_range(0.0..=100.0)
                        .speed(0.5)
                        .max_decimals(0)
                        .suffix("%"),
                )
                .on_hover_text("Opacity")
                .changed()
            {
                memory.alpha = percent / 100.0;
                changed |= memory.commit(with_alpha);
            }
        });
    }
    changed
}

fn prefs_channel_code(channels: Channels) -> f32 {
    match channels {
        Channels::Rgb => 1.0,
        Channels::Hsv => 2.0,
        Channels::Hsl => 3.0,
    }
}

fn set_channel(
    memory: &mut Memory,
    channels: Channels,
    index: usize,
    value: f32,
    with_alpha: bool,
) -> bool {
    match channels {
        Channels::Rgb => {
            let mut rgb = hsv_to_rgb(memory.hsv);
            rgb[index] = value;
            let [h, s, v] = rgb_to_hsv(rgb);
            let mut hsv = memory.hsv;
            if v > 1.0e-4 {
                if s > 1.0e-4 {
                    hsv[0] = h;
                }
                hsv[1] = s;
            }
            hsv[2] = v;
            memory.set_hsv(hsv, with_alpha)
        }
        Channels::Hsv => {
            let mut hsv = memory.hsv;
            hsv[index] = value;
            memory.set_hsv(hsv, with_alpha)
        }
        Channels::Hsl => {
            let mut hsl = hsv_to_hsl(memory.hsv);
            hsl[index] = value;
            let mut hsv = hsl_to_hsv(hsl);
            // Keep the hue of a grey, as everywhere else.
            hsv[0] = hsl[0];
            memory.set_hsv(hsv, with_alpha)
        }
    }
}

/// A row of clickable colour tiles; returns the one clicked.
fn tile_row(
    ui: &mut Ui,
    colors: &[Color32],
    size: Vec2,
    current: Color32,
    hint: &str,
) -> Option<(usize, bool)> {
    tile_row_with_add(ui, colors, size, current, hint, None)
}

/// `tile_row`, ending in a dashed + tile that sets `add` when clicked.
fn tile_row_with_add(
    ui: &mut Ui,
    colors: &[Color32],
    size: Vec2,
    current: Color32,
    hint: &str,
    add: Option<&mut bool>,
) -> Option<(usize, bool)> {
    let mut picked = None;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(5.0, 5.0);
        for (index, color) in colors.iter().enumerate() {
            let (rect, response) = ui.allocate_exact_size(size, Sense::click());
            tile(ui, rect, 5.0, *color);
            ui.painter().rect_stroke(
                rect,
                5.0_f32,
                Stroke::new(1.0_f32, Color32::from_black_alpha(70)),
            );
            if *color == current {
                ui.painter().circle_filled(
                    rect.center(),
                    2.5,
                    if rgb_of(*color).iter().sum::<f32>() > 1.6 {
                        Color32::from_black_alpha(160)
                    } else {
                        Color32::from_white_alpha(220)
                    },
                );
            }
            if response.hovered() {
                ui.painter().rect_stroke(
                    rect.expand(2.0),
                    6.5_f32,
                    Stroke::new(1.5_f32, ui.visuals().strong_text_color()),
                );
            }
            let response = response.on_hover_text(format!("#{} · {hint}", hex_of(*color, true)));
            if response.clicked() {
                picked = Some((index, false));
            }
            if response.secondary_clicked() {
                picked = Some((index, true));
            }
        }
        if let Some(add) = add {
            let (rect, response) = ui.allocate_exact_size(size, Sense::click());
            let color = if response.hovered() {
                ui.visuals().strong_text_color()
            } else {
                ui.visuals().weak_text_color()
            };
            // Dashes round the tile.
            let r = rect.shrink(0.5);
            let corners = [
                r.left_top(),
                r.right_top(),
                r.right_bottom(),
                r.left_bottom(),
            ];
            for side in 0..4 {
                let (a, b) = (corners[side], corners[(side + 1) % 4]);
                let steps = 4;
                for step in 0..steps {
                    let t0 = (step as f32 + 0.2) / steps as f32;
                    let t1 = (step as f32 + 0.75) / steps as f32;
                    ui.painter().line_segment(
                        [a + (b - a) * t0, a + (b - a) * t1],
                        Stroke::new(1.0_f32, color),
                    );
                }
            }
            paint_plus_icon(ui.painter(), rect, color);
            if response.on_hover_text("Save the current colour").clicked() {
                *add = true;
            }
        }
    });
    picked
}

fn section_label(ui: &mut Ui, text: &str) {
    ui.label(
        egui::RichText::new(text)
            .small()
            .color(ui.visuals().weak_text_color()),
    );
}

/// The Harmony tab: colours that go with this one, by a chosen rule.
fn harmony(
    ui: &mut Ui,
    memory: &mut Memory,
    prefs: &mut Prefs,
    with_alpha: bool,
    m: Metrics,
) -> bool {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
        for rule in Harmony::ALL {
            if ui
                .selectable_label(prefs.harmony == rule, rule.label())
                .on_hover_text(rule.hint())
                .clicked()
            {
                prefs.harmony = rule;
            }
        }
    });
    let colors: Vec<Color32> = prefs
        .harmony
        .colors(memory.hsv)
        .into_iter()
        .map(|hsv| color_of(hsv_to_rgb(hsv), memory.alpha))
        .collect();
    let count = colors.len() as f32;
    let gap = 6.0;
    let width = (m.width - gap * (count - 1.0)) / count;
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = gap;
        for color in &colors {
            let (rect, response) =
                ui.allocate_exact_size(vec2(width, m.swatch * 1.7), Sense::click());
            tile(ui, rect, 7.0, *color);
            ui.painter().rect_stroke(
                rect,
                7.0_f32,
                Stroke::new(1.0_f32, Color32::from_black_alpha(70)),
            );
            if *color == memory.shown {
                ui.painter().rect_stroke(
                    rect.shrink(3.0),
                    5.0_f32,
                    Stroke::new(2.0_f32, Color32::from_white_alpha(230)),
                );
            }
            if response.hovered() {
                ui.painter().rect_stroke(
                    rect.expand(2.0),
                    8.5_f32,
                    Stroke::new(1.5_f32, ui.visuals().strong_text_color()),
                );
            }
            ui.painter().text(
                rect.center_bottom() + vec2(0.0, -4.0),
                Align2::CENTER_BOTTOM,
                hex_of(*color, false),
                FontId::monospace(if crate::touch::ENABLED { 9.5 } else { 8.0 }),
                if rgb_of(*color).iter().sum::<f32>() > 1.6 {
                    Color32::from_black_alpha(170)
                } else {
                    Color32::from_white_alpha(210)
                },
            );
            if response.on_hover_text("Use this colour").clicked() && *color != memory.shown {
                memory.sync(*color, with_alpha);
                changed = true;
            }
        }
    });
    if ui
        .small_button("Save these to my swatches")
        .on_hover_text("Adds the colours above to Saved in the Swatches tab")
        .clicked()
    {
        for color in colors.iter().rev() {
            let rgba = color.to_srgba_unmultiplied();
            if !prefs.saved.contains(&rgba) {
                prefs.saved.push(rgba);
            }
        }
    }
    changed
}

/// The Swatches tab: recent colours, saved ones and the project's.
fn swatches(
    ui: &mut Ui,
    memory: &mut Memory,
    prefs: &mut Prefs,
    with_alpha: bool,
    m: Metrics,
) -> bool {
    let size = vec2(m.swatch, m.swatch);
    let mut changed = false;
    let mut apply = |memory: &mut Memory, color: Color32| {
        let color = if with_alpha { color } else { opaque(color) };
        if color != memory.shown {
            memory.sync(color, with_alpha);
            changed = true;
        }
    };
    let as_colors = |list: &[[u8; 4]]| -> Vec<Color32> {
        list.iter()
            .map(|[r, g, b, a]| Color32::from_rgba_unmultiplied(*r, *g, *b, *a))
            .collect()
    };

    section_label(ui, "Recent");
    if prefs.recent.is_empty() {
        ui.label(
            egui::RichText::new("Colours you pick show up here.")
                .small()
                .italics()
                .color(ui.visuals().weak_text_color()),
        );
    } else if let Some((index, remove)) = tile_row(
        ui,
        &as_colors(&prefs.recent),
        size,
        memory.shown,
        "right-click to forget",
    ) {
        if remove {
            prefs.recent.remove(index);
        } else {
            apply(memory, as_colors(&prefs.recent)[index]);
        }
    }

    section_label(ui, "Saved");
    let rgba = memory.shown.to_srgba_unmultiplied();
    let already = prefs.saved.contains(&rgba);
    let mut add = false;
    if let Some((index, remove)) = tile_row_with_add(
        ui,
        &as_colors(&prefs.saved),
        size,
        memory.shown,
        "right-click to remove",
        (!already).then_some(&mut add),
    ) {
        if remove {
            prefs.saved.remove(index);
        } else {
            apply(memory, as_colors(&prefs.saved)[index]);
        }
    }
    if add {
        prefs.saved.push(rgba);
    }

    let project = project_colors(ui.ctx());
    if !project.is_empty() {
        section_label(ui, "In this project");
        if let Some((index, remove)) =
            tile_row(ui, &project, size, memory.shown, "used in this project")
        {
            if !remove {
                apply(memory, project[index]);
            }
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hsv_and_hsl_go_there_and_back() {
        for rgb in [
            [1.0, 0.0, 0.0],
            [0.2, 0.6, 0.9],
            [0.5, 0.5, 0.5],
            [0.0, 0.0, 0.0],
            [1.0, 1.0, 1.0],
            [0.9, 0.4, 0.1],
        ] {
            let hsv = rgb_to_hsv(rgb);
            let back = hsv_to_rgb(hsv);
            let again = hsv_to_rgb(hsl_to_hsv(hsv_to_hsl(hsv)));
            for i in 0..3 {
                assert!((back[i] - rgb[i]).abs() < 1.0e-4, "{rgb:?} -> {back:?}");
                assert!((again[i] - rgb[i]).abs() < 1.0e-4, "{rgb:?} -> {again:?}");
            }
        }
    }

    #[test]
    fn hex_codes_read_like_people_type_them() {
        assert_eq!(
            parse_hex("#f80", 255),
            Some(Color32::from_rgb(0xFF, 0x88, 0x00))
        );
        assert_eq!(
            parse_hex(" 3D6FE0 ", 255),
            Some(Color32::from_rgb(0x3D, 0x6F, 0xE0))
        );
        assert_eq!(
            parse_hex("3D6FE080", 255),
            Some(Color32::from_rgba_unmultiplied(0x3D, 0x6F, 0xE0, 0x80))
        );
        assert_eq!(parse_hex("12345", 255), None);
        assert_eq!(parse_hex("zzzzzz", 255), None);
        assert_eq!(hex_of(Color32::from_rgb(1, 2, 3), true), "010203");
        assert_eq!(hex_of_values([1, 2, 3], 4, true), "01020304");
        assert_eq!(hex_of_values([1, 2, 3], 4, false), "010203");
        // A nearly clear colour keeps its code while the picker holds it.
        let mut memory = Memory::new(Color32::from_rgb(240, 106, 180), true);
        memory.alpha = 4.0 / 255.0;
        memory.commit(true);
        assert_eq!(memory.hex, "F06AB404");
    }

    #[test]
    fn a_grey_keeps_the_hue_it_had() {
        let mut memory = Memory::new(Color32::from_rgb(200, 40, 40), false);
        let red_hue = memory.hsv[0];
        memory.sync(Color32::from_gray(128), false);
        assert_eq!(memory.hsv[0], red_hue);
        memory.sync(Color32::BLACK, false);
        assert_eq!(memory.hsv[0], red_hue);
        assert!(memory.hsv[1] < 1.0e-4, "the grey's saturation stays");
    }

    #[test]
    fn every_harmony_holds_the_colour_it_came_from() {
        let hsv = [0.6, 0.7, 0.8];
        for rule in Harmony::ALL {
            let colors = rule.colors(hsv);
            assert!(colors.len() >= 4, "{rule:?}");
            assert!(
                colors.iter().any(|c| (c[0] - hsv[0]).abs() < 1.0e-6
                    && (c[1] - hsv[1]).abs() < 1.0e-6
                    && (c[2] - hsv[2]).abs() < 1.0e-6),
                "{rule:?}"
            );
        }
    }

    #[test]
    fn channel_edits_land_where_the_slider_says() {
        let mut memory = Memory::new(Color32::from_rgb(200, 40, 40), false);
        // Red down to nothing on a red: black keeps its red hue.
        assert!(set_channel(&mut memory, Channels::Rgb, 0, 0.0, false));
        assert_eq!(opaque(memory.shown), Color32::from_rgb(0, 40, 40));
        // Lightness to the top is white whatever the hue.
        set_channel(&mut memory, Channels::Hsl, 2, 1.0, false);
        assert_eq!(opaque(memory.shown), Color32::WHITE);
        let hue = memory.hsv[0];
        set_channel(&mut memory, Channels::Hsl, 2, 0.5, false);
        assert!(
            (memory.hsv[0] - hue).abs() < 1.0e-6,
            "white keeps the hue it had"
        );
        // Value in HSV.
        set_channel(&mut memory, Channels::Hsv, 2, 0.0, false);
        assert_eq!(opaque(memory.shown), Color32::BLACK);
    }

    #[test]
    fn recent_colours_lead_with_the_newest_and_never_repeat() {
        let mut prefs = Prefs::default();
        push_recent(&mut prefs, Color32::RED);
        push_recent(&mut prefs, Color32::BLUE);
        push_recent(&mut prefs, Color32::RED);
        assert_eq!(prefs.recent[0], Color32::RED.to_srgba_unmultiplied());
        assert_eq!(prefs.recent.len(), 2);
        for i in 0..40 {
            push_recent(&mut prefs, Color32::from_gray(i));
        }
        assert_eq!(prefs.recent.len(), RECENT);
    }
}
