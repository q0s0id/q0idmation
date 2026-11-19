//! The editor's own mouse pointers.
//!
//! Every pointer state the editor uses has a vector drawing here. egui paints
//! it on the topmost layer at the pointer while the system cursor hides.
//! Colours come from the theme (`Theme::cursor_colors`), so they follow every
//! palette and `.q7s` file.
//!
//! Motion is driven by events: a pointer plays when it appears, when the
//! button goes down or comes up, and while the mouse moves, then rests. An
//! idle editor draws no extra frames for its cursor. Each state moves in its
//! own way (the pen dips, the bucket pours, the crosshair locks on); a few
//! hold still because precision reads better that way.
//!
//! Also here: a number field dragged into the window's edge carries on from
//! the opposite edge, as in Blender. That moves the system cursor, which only
//! Windows does here; elsewhere the drag stops at the edge as before.

use std::collections::BTreeSet;
use std::f32::consts::{FRAC_PI_2, FRAC_PI_4, PI, TAU};

use egui::epaint::Mesh;
use egui::{
    pos2, vec2, Color32, Context, CursorIcon, Id, LayerId, Order, Pos2, Response, Sense, Shape,
    Stroke, Ui, Vec2,
};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// States

/// A pointer the editor draws for itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CursorKind {
    Arrow,
    Link,
    Text,
    Grab,
    Grabbing,
    Move,
    Crosshair,
    ResizeH,
    ResizeV,
    ResizeNwSe,
    ResizeNeSw,
    Rotate,
    SkewH,
    SkewV,
    Scrub,
    Pen,
    Pencil,
    Bucket,
    Eyedropper,
    Knife,
    Lasso,
    Line,
    Rect,
    Oval,
    LibraryDrop,
    PenClose,
    ScreenPick,
    FrameDrag,
    ParentLink,
    KnifeCut,
    SelectAdd,
    SelectRemove,
    CopyDrag,
    Pivot,
    Snap,
    Locked,
}

impl CursorKind {
    /// One of each switch in Settings, in the order shown there.
    pub(crate) const STATES: [Self; 20] = [
        Self::Arrow,
        Self::Link,
        Self::Text,
        Self::Grab,
        Self::Grabbing,
        Self::Move,
        Self::Crosshair,
        Self::ResizeH,
        Self::ResizeV,
        Self::ResizeNwSe,
        Self::Rotate,
        Self::SkewH,
        Self::Scrub,
        Self::Pen,
        Self::Pencil,
        Self::Bucket,
        Self::Eyedropper,
        Self::Knife,
        Self::Lasso,
        Self::Rect,
    ];

    /// Pointers for what a press or drag will do right there, which no
    /// system cursor stands for. Settings lists them on their own.
    pub(crate) const ACTIONS: [Self; 12] = [
        Self::LibraryDrop,
        Self::PenClose,
        Self::ScreenPick,
        Self::FrameDrag,
        Self::ParentLink,
        Self::KnifeCut,
        Self::SelectAdd,
        Self::SelectRemove,
        Self::CopyDrag,
        Self::Pivot,
        Self::Snap,
        Self::Locked,
    ];

    /// The name a state is switched off by in the settings file. Variants of
    /// one state (the two diagonals, the shape tools) share theirs.
    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::Arrow => "arrow",
            Self::Link => "link",
            Self::Text => "text",
            Self::Grab => "grab",
            Self::Grabbing => "grabbing",
            Self::Move => "move",
            Self::Crosshair => "crosshair",
            Self::ResizeH => "resize_h",
            Self::ResizeV => "resize_v",
            Self::ResizeNwSe | Self::ResizeNeSw => "resize_diagonal",
            Self::Rotate => "rotate",
            Self::SkewH | Self::SkewV => "skew",
            Self::Scrub => "number_drag",
            Self::Pen => "pen",
            Self::Pencil => "pencil",
            Self::Bucket => "bucket",
            Self::Eyedropper => "eyedropper",
            Self::Knife => "knife",
            Self::Lasso => "lasso",
            Self::Line | Self::Rect | Self::Oval => "shapes",
            Self::LibraryDrop => "library_drop",
            Self::PenClose => "pen_close",
            Self::ScreenPick => "screen_pick",
            Self::FrameDrag => "frame_drag",
            Self::ParentLink => "parent_link",
            Self::KnifeCut => "knife_cut",
            Self::SelectAdd => "select_add",
            Self::SelectRemove => "select_remove",
            Self::CopyDrag => "copy_drag",
            Self::Pivot => "pivot",
            Self::Snap => "snap",
            Self::Locked => "locked",
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Arrow => "Pointer",
            Self::Link => "Buttons and links",
            Self::Text => "Text",
            Self::Grab => "Grab",
            Self::Grabbing => "Grabbing",
            Self::Move => "Move",
            Self::Crosshair => "Crosshair",
            Self::ResizeH => "Resize sideways",
            Self::ResizeV => "Resize up and down",
            Self::ResizeNwSe | Self::ResizeNeSw => "Resize a corner",
            Self::Rotate => "Rotate",
            Self::SkewH | Self::SkewV => "Skew",
            Self::Scrub => "Number drag",
            Self::Pen => "Pen",
            Self::Pencil => "Pencil",
            Self::Bucket => "Paint bucket",
            Self::Eyedropper => "Eyedropper",
            Self::Knife => "Knife",
            Self::Lasso => "Lasso",
            Self::Line | Self::Rect | Self::Oval => "Line, rectangle, oval",
            Self::LibraryDrop => "Drop from the Library",
            Self::PenClose => "Close the path",
            Self::ScreenPick => "Pick from the screen",
            Self::FrameDrag => "Move frames",
            Self::ParentLink => "Link to a parent",
            Self::KnifeCut => "Cutting",
            Self::SelectAdd => "Add to the selection",
            Self::SelectRemove => "Take from the selection",
            Self::CopyDrag => "Alt-drag copy",
            Self::Pivot => "Transform anchor",
            Self::Snap => "Snapping",
            Self::Locked => "Locked layer",
        }
    }

    /// Where it shows and how it moves.
    pub(crate) fn hint(self) -> &'static str {
        match self {
            Self::Arrow => "The idle pointer. Trails a little behind fast moves and springs back; dips when you click.",
            Self::Link => "Over buttons and links. Wags once on arrival; the finger presses in while the button is down.",
            Self::Text => "In text fields and the Text tool. Unfolds from the middle, then holds still.",
            Self::Grab => "Over things you can pick up, and the Hand tool. The fingers fan open one by one.",
            Self::Grabbing => "While dragging something. Squeezes on the grab and stretches along fast drags.",
            Self::Move => "Moving pivots, emitters and 3D objects. The arrowheads pop out in turn; the leading one reaches ahead.",
            Self::Crosshair => "Precise picks, bones and gizmos. Locks on with a quarter turn, then holds still.",
            Self::ResizeH => "Panel edges, timeline columns and side handles. Stretches elastically on arrival.",
            Self::ResizeV => "Top and bottom handles and row edges. Stretches elastically on arrival.",
            Self::ResizeNwSe | Self::ResizeNeSw => "Corner handles and zooming the camera. The corner bracket slides into place.",
            Self::Rotate => "Rotate zones of the transform box, cameras and lights. Draws its arc on arrival and turns as you drag.",
            Self::SkewH | Self::SkewV => "Skew zones on the edges of the transform box. Leans into the shear as you drag.",
            Self::Scrub => "Dragging a number field. The chevrons run like a belt in the direction you drag.",
            Self::Pen => "The Pen tool. Dips in like a nib into ink and leaves an ink dot when you press.",
            Self::Pencil => "The Pencil tool. Swings into hand and tilts with the stroke.",
            Self::Bucket => "The Paint Bucket. Still until you click; then it pours a drop and splashes.",
            Self::Eyedropper => "The Eyedropper and screen colour picks. Holds the fill colour; a press draws more up the tube.",
            Self::Knife => "The Knife. Still, with a glint along the edge as you cut.",
            Self::Lasso => "The Lasso. The loop spins once on arrival and its rope runs as you draw.",
            Self::Line | Self::Rect | Self::Oval => "The Line, Rectangle and Oval tools: a crosshair with a badge of the shape that pops in.",
            Self::LibraryDrop => "Dragging a Library item over the stage or the timeline: letting go places it there. Its card drops in, hangs from the pointer and swings as you move.",
            Self::PenClose => "The Pen over the first point of the path you are drawing: a click closes the shape. A ring draws itself shut around the nib, then pulses once.",
            Self::ScreenPick => "Dragging the Eyedropper, or a colour button off its panel: a loupe holding the colour under it, live. It zooms in and its notches turn as you sweep.",
            Self::FrameDrag => "Dragging selected frames along the timeline: a keyframe diamond with ghosts trailing the way it came.",
            Self::ParentLink => "Dragging a layer's parent square onto another layer: two chain links snap together and swing with the drag.",
            Self::KnifeCut => "The Knife while it cuts: the blade leans back against the cut and leaves a slash behind; sparks fly as it bites.",
            Self::SelectAdd => "Shift over something not yet selected: a click adds it. The plus spins into its badge.",
            Self::SelectRemove => "Shift over something already selected: a click takes it out. The badge drops in and the minus wipes open.",
            Self::CopyDrag => "Alt over a selected object: dragging makes a copy. The copy slides off the stack and peels away the way you drag.",
            Self::Pivot => "Alt over the transform anchor, and while moving it: a target whose satellite orbits once, and again as you drag.",
            Self::Snap => "Drawing or moving onto something the magnet catches: the point lands on it. The magnet clicks in with a spark.",
            Self::Locked => "A drawing tool over a layer that is locked, hidden or a folder: nothing will be drawn. The shackle drops shut; a click rattles it.",
        }
    }

    /// The system cursor that stands in for it when it is switched off.
    pub(crate) fn system(self) -> CursorIcon {
        match self {
            Self::Arrow => CursorIcon::Default,
            Self::Link => CursorIcon::PointingHand,
            Self::Text => CursorIcon::Text,
            Self::Grab => CursorIcon::Grab,
            Self::Grabbing => CursorIcon::Grabbing,
            Self::Move => CursorIcon::Move,
            Self::ResizeH | Self::Scrub => CursorIcon::ResizeHorizontal,
            Self::ResizeV => CursorIcon::ResizeVertical,
            Self::SkewH => CursorIcon::ResizeColumn,
            Self::SkewV => CursorIcon::ResizeRow,
            Self::ResizeNwSe => CursorIcon::ResizeNwSe,
            Self::ResizeNeSw => CursorIcon::ResizeNeSw,
            Self::Rotate => CursorIcon::Alias,
            Self::Crosshair
            | Self::Pen
            | Self::Pencil
            | Self::Bucket
            | Self::Eyedropper
            | Self::Knife
            | Self::Lasso
            | Self::Line
            | Self::Rect
            | Self::Oval
            | Self::PenClose
            | Self::ScreenPick
            | Self::KnifeCut
            | Self::Snap => CursorIcon::Crosshair,
            Self::LibraryDrop | Self::FrameDrag | Self::ParentLink => CursorIcon::Grabbing,
            Self::SelectAdd | Self::SelectRemove => CursorIcon::Default,
            Self::CopyDrag => CursorIcon::Copy,
            Self::Pivot => CursorIcon::Move,
            Self::Locked => CursorIcon::NotAllowed,
        }
    }

    /// The drawn pointer for a system cursor the editor or egui asked for.
    /// `Alias` is the editor's rotate cursor (cameras, lights, physics).
    fn from_icon(icon: CursorIcon) -> Option<Self> {
        use CursorIcon as I;
        Some(match icon {
            I::Default => Self::Arrow,
            I::PointingHand => Self::Link,
            I::Text | I::VerticalText => Self::Text,
            I::Grab => Self::Grab,
            I::Grabbing => Self::Grabbing,
            I::Move | I::AllScroll => Self::Move,
            I::Crosshair | I::Cell => Self::Crosshair,
            I::ResizeHorizontal | I::ResizeEast | I::ResizeWest | I::ResizeColumn => Self::ResizeH,
            I::ResizeVertical | I::ResizeNorth | I::ResizeSouth | I::ResizeRow => Self::ResizeV,
            I::ResizeNwSe | I::ResizeNorthWest | I::ResizeSouthEast => Self::ResizeNwSe,
            I::ResizeNeSw | I::ResizeNorthEast | I::ResizeSouthWest => Self::ResizeNeSw,
            I::Alias => Self::Rotate,
            _ => return None,
        })
    }

    /// How long its arrival plays, in seconds. Zero for the still ones.
    fn entry_secs(self) -> f32 {
        match self {
            Self::Arrow | Self::Scrub | Self::Bucket | Self::Knife | Self::KnifeCut => 0.0,
            Self::Text | Self::Crosshair | Self::Line | Self::Rect | Self::Oval => 0.32,
            Self::ResizeNwSe | Self::ResizeNeSw | Self::Grabbing => 0.4,
            Self::Grab | Self::Rotate | Self::SkewH | Self::SkewV | Self::Pen => 0.45,
            Self::Link | Self::Move | Self::Pencil => 0.55,
            Self::ResizeH | Self::ResizeV | Self::Lasso => 0.65,
            Self::Eyedropper => 0.7,
            Self::FrameDrag | Self::ScreenPick | Self::SelectAdd | Self::SelectRemove => 0.36,
            Self::CopyDrag | Self::Snap | Self::ParentLink | Self::Locked => 0.48,
            Self::LibraryDrop | Self::Pivot => 0.65,
            Self::PenClose => 0.8,
        }
    }

    /// Roughly the middle of the drawing, from the hotspot, for previews.
    fn preview_center(self) -> Vec2 {
        match self {
            Self::Arrow => vec2(6.0, 9.5),
            Self::Link => vec2(2.4, 10.4),
            Self::Grab | Self::Grabbing => vec2(0.0, 2.0),
            Self::Pen => vec2(5.4, -6.4),
            Self::Pencil => vec2(5.6, -8.0),
            Self::Bucket => vec2(6.4, -6.4),
            Self::Eyedropper => vec2(6.2, -6.6),
            Self::Knife => vec2(5.0, -8.6),
            Self::Lasso => vec2(5.2, -5.4),
            Self::Line | Self::Rect | Self::Oval => vec2(3.0, 3.0),
            Self::LibraryDrop => vec2(5.4, 10.8),
            Self::PenClose => vec2(4.0, -5.0),
            Self::ScreenPick => vec2(3.2, 3.2),
            Self::ParentLink => vec2(5.4, -5.4),
            Self::KnifeCut => vec2(5.0, -8.6),
            Self::SelectAdd | Self::SelectRemove | Self::Locked => vec2(6.6, 10.4),
            Self::CopyDrag => vec2(7.6, 10.4),
            Self::Snap => vec2(4.0, -4.0),
            _ => Vec2::ZERO,
        }
    }
}

// ---------------------------------------------------------------------------
// Preferences

/// The pointer switches on the Appearance page.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct CursorSettings {
    /// Draw the editor's own pointers instead of the system ones.
    pub custom: bool,
    /// Let them move. Off, every pointer holds still.
    pub animated: bool,
    /// Size, 1 being about a system cursor's.
    pub size: f32,
    /// A number field dragged into the window's edge carries on from the
    /// opposite edge.
    pub wrap_number_drags: bool,
    /// The states left to the system cursor, by `CursorKind::key`. New
    /// states start drawn.
    pub system: BTreeSet<String>,
}

impl Default for CursorSettings {
    fn default() -> Self {
        Self {
            custom: true,
            animated: true,
            size: 1.0,
            wrap_number_drags: true,
            system: BTreeSet::new(),
        }
    }
}

impl CursorSettings {
    pub const SIZE_RANGE: std::ops::RangeInclusive<f32> = 0.75..=1.75;

    /// Whether `kind` is drawn by the editor.
    pub(crate) fn shows(&self, kind: CursorKind) -> bool {
        self.custom && self.state_on(kind)
    }

    /// The state's own switch, whatever the main one says.
    pub(crate) fn state_on(&self, kind: CursorKind) -> bool {
        !self.system.contains(kind.key())
    }

    pub(crate) fn set_state(&mut self, kind: CursorKind, on: bool) {
        if on {
            self.system.remove(kind.key());
        } else {
            self.system.insert(kind.key().to_string());
        }
    }

    fn scale(&self) -> f32 {
        let size = if self.size.is_finite() {
            self.size
        } else {
            1.0
        };
        size.clamp(*Self::SIZE_RANGE.start(), *Self::SIZE_RANGE.end())
    }
}

// ---------------------------------------------------------------------------
// Asking for a pointer

const REQUEST_ID: &str = "q0_cursor_request";
const HIDE_ID: &str = "q0_cursor_hide";
const LAYER_ID: &str = "q0_cursor_layer";

#[derive(Clone, Copy)]
struct Request {
    kind: CursorKind,
    icon: CursorIcon,
    sample: Option<Color32>,
}

/// Show `kind` this frame. Its system stand-in is set right away, so a later
/// widget that sets another cursor still wins, as with `set_cursor_icon`.
pub(crate) fn request(ctx: &Context, kind: CursorKind) {
    request_with_sample(ctx, kind, None);
}

/// `request`, with the colour an eyedropper shows in its tube.
pub(crate) fn request_with_sample(ctx: &Context, kind: CursorKind, sample: Option<Color32>) {
    let icon = kind.system();
    ctx.set_cursor_icon(icon);
    ctx.data_mut(|data| {
        data.insert_temp(Id::new(REQUEST_ID), Request { kind, icon, sample });
    });
}

// ---------------------------------------------------------------------------
// Colours

/// The colours a pointer is drawn in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Palette {
    fill: Color32,
    outline: Color32,
    accent: Color32,
    shadow: Color32,
}

impl Palette {
    pub(crate) fn from_theme(theme: &crate::settings::Theme) -> Self {
        let colors = theme.cursor_colors();
        let shadow = (colors.shadow.min(100) as f32 * 1.9).round() as u8;
        Self {
            fill: colors.fill.to_color32(),
            outline: colors.outline.to_color32(),
            accent: colors.accent.to_color32(),
            shadow: Color32::from_black_alpha(shadow),
        }
    }

    fn faded(self, k: f32) -> Self {
        Self {
            fill: self.fill.gamma_multiply(k),
            outline: self.outline.gamma_multiply(k),
            accent: self.accent.gamma_multiply(k),
            shadow: self.shadow.gamma_multiply(k),
        }
    }
}

fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgba_premultiplied(
        l(a.r(), b.r()),
        l(a.g(), b.g()),
        l(a.b(), b.b()),
        l(a.a(), b.a()),
    )
}

// ---------------------------------------------------------------------------
// Motion

/// What a pointer reacts to, all in seconds and points.
#[derive(Clone, Copy, Debug)]
struct Motion {
    /// Since this pointer appeared.
    enter: f32,
    /// Since the primary button last went down.
    pressed: f32,
    /// The button is down now.
    held: bool,
    /// Since the button last came up.
    release: f32,
    /// The pointer's velocity, followed by a spring that overshoots a little.
    lean: Vec2,
    /// Distance moved with the button down.
    travel: f32,
    /// Sideways distance moved with the button down, signed.
    sweep: f32,
    /// The colour an eyedropper holds.
    sample: Option<Color32>,
}

impl Motion {
    const REST: Self = Self {
        enter: 100.0,
        pressed: 100.0,
        held: false,
        release: 100.0,
        lean: Vec2::ZERO,
        travel: 0.0,
        sweep: 0.0,
        sample: None,
    };

    /// 0 at rest, 1 while the button is down; after release it springs back
    /// through a small overshoot.
    fn press(&self) -> f32 {
        if self.held {
            ease_out(clamp01(self.pressed / 0.09))
        } else if self.release < 0.7 && self.pressed < 100.0 {
            (-9.0 * self.release).exp() * (17.0 * self.release).cos()
        } else {
            0.0
        }
    }

    /// The motion a preview plays on a loop: arrive, sway, press, let go.
    fn demo(time: f64) -> Self {
        const CYCLE: f32 = 2.8;
        const DOWN: f32 = 1.15;
        const UP: f32 = 1.85;
        let t = (time % CYCLE as f64) as f32;
        let seconds = time as f32;
        let held = (DOWN..UP).contains(&t);
        let since = |at: f32| if t >= at { t - at } else { t + CYCLE - at };
        let sway = pulse(t - 0.55, 1.9) * (seconds * 3.3).sin() * 520.0;
        Self {
            enter: t,
            pressed: since(DOWN),
            held,
            release: if held { 100.0 } else { since(UP) },
            lean: vec2(sway, sway * 0.25),
            travel: seconds * 80.0,
            sweep: (seconds * 1.4).sin() * 120.0,
            sample: None,
        }
    }
}

fn clamp01(t: f32) -> f32 {
    t.clamp(0.0, 1.0)
}

fn ease_out(t: f32) -> f32 {
    let u = 1.0 - clamp01(t);
    1.0 - u * u * u
}

/// Ease out past the end and settle back.
fn ease_back(t: f32) -> f32 {
    const C1: f32 = 1.70158;
    let u = clamp01(t) - 1.0;
    1.0 + (C1 + 1.0) * u * u * u + C1 * u * u
}

/// A swing that starts at 0, swings out and dies away.
fn wobble(t: f32, frequency: f32, decay: f32) -> f32 {
    if t < 0.0 {
        return 0.0;
    }
    (-decay * t).exp() * (frequency * t).sin()
}

/// One hump from 0 up to 1 and back over `duration`.
fn pulse(t: f32, duration: f32) -> f32 {
    if (0.0..duration).contains(&t) {
        (PI * t / duration).sin()
    } else {
        0.0
    }
}

// ---------------------------------------------------------------------------
// Drawing

/// A 2D affine map: `p' = (a x + c y, b x + d y) + t`.
#[derive(Clone, Copy, Debug)]
struct Xf {
    a: f32,
    b: f32,
    c: f32,
    d: f32,
    t: Vec2,
}

impl Xf {
    const ID: Self = Self {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 1.0,
        t: Vec2::ZERO,
    };

    fn rotate(angle: f32) -> Self {
        let (s, c) = angle.sin_cos();
        Self {
            a: c,
            b: s,
            c: -s,
            d: c,
            t: Vec2::ZERO,
        }
    }

    fn scale(sx: f32, sy: f32) -> Self {
        Self {
            a: sx,
            d: sy,
            ..Self::ID
        }
    }

    fn shear_x(k: f32) -> Self {
        Self { c: k, ..Self::ID }
    }

    fn translate(t: Vec2) -> Self {
        Self { t, ..Self::ID }
    }

    /// Stretch by `k` along `direction` and squeeze across it to keep area.
    fn stretch(direction: Vec2, k: f32) -> Self {
        let angle = direction.y.atan2(direction.x);
        Self::rotate(-angle)
            .then(Self::scale(k, 1.0 / k))
            .then(Self::rotate(angle))
    }

    fn apply(self, v: Vec2) -> Vec2 {
        vec2(self.a * v.x + self.c * v.y, self.b * v.x + self.d * v.y) + self.t
    }

    /// `self` first, then `outer`.
    fn then(self, outer: Self) -> Self {
        Self {
            a: outer.a * self.a + outer.c * self.b,
            b: outer.b * self.a + outer.d * self.b,
            c: outer.a * self.c + outer.c * self.d,
            d: outer.b * self.c + outer.d * self.d,
            t: outer.apply(self.t),
        }
    }
}

fn v(x: f32, y: f32) -> Vec2 {
    vec2(x, y)
}

fn lerp(a: Vec2, b: Vec2, t: f32) -> Vec2 {
    a + (b - a) * t
}

/// Collects one pointer's shapes. Drawings are in points at size 1, with the
/// hotspot at the origin.
struct Canvas {
    shapes: Vec<Shape>,
    origin: Pos2,
    scale: f32,
    xf: Xf,
    pal: Palette,
    /// The shadow pass: everything in the shadow colour, details left out.
    shadow: bool,
    /// Outline width in screen points.
    line: f32,
}

impl Canvas {
    fn new(origin: Pos2, scale: f32, pal: Palette, shadow: bool) -> Self {
        Self {
            shapes: Vec::new(),
            origin,
            scale,
            xf: Xf::ID,
            pal,
            shadow,
            line: (1.2 * scale).max(1.0),
        }
    }

    fn at(&self, p: Vec2) -> Pos2 {
        self.origin + self.xf.apply(p) * self.scale
    }

    fn map(&self, points: &[Vec2]) -> Vec<Pos2> {
        points.iter().map(|&p| self.at(p)).collect()
    }

    fn ink(&self, color: Color32) -> Color32 {
        if self.shadow {
            self.pal.shadow
        } else {
            color
        }
    }

    /// Draw with `xf` applied before the current transform.
    fn with(&mut self, xf: Xf, draw: impl FnOnce(&mut Self)) {
        let outer = self.xf;
        self.xf = xf.then(outer);
        draw(self);
        self.xf = outer;
    }

    /// A filled shape with an outline. It may be concave.
    fn body(&mut self, points: &[Vec2], fill: Color32) {
        let points = self.map(points);
        let fill = self.ink(fill);
        let outline = Stroke::new(self.line, self.ink(self.pal.outline));
        fill_polygon(&mut self.shapes, &points, fill);
        self.shapes.push(Shape::closed_line(points, outline));
    }

    /// A filled shape without an outline, for things inside a body.
    fn flat(&mut self, points: &[Vec2], fill: Color32) {
        if !self.shadow {
            let points = self.map(points);
            fill_polygon(&mut self.shapes, &points, fill);
        }
    }

    /// Outlined lines with round ends. All the outlines go under all the
    /// colours, so lines that meet read as one shape.
    fn lines(&mut self, lines: &[(&[Vec2], f32, Color32)]) {
        let mapped: Vec<(Vec<Pos2>, f32, Color32)> = lines
            .iter()
            .filter(|(points, _, _)| points.len() >= 2)
            .map(|(points, width, color)| (self.map(points), width * self.scale, *color))
            .collect();
        let outline = self.ink(self.pal.outline);
        for (points, width, _) in &mapped {
            let reach = width * 0.5 + self.line;
            self.shapes.push(Shape::line(
                points.clone(),
                Stroke::new(width + 2.0 * self.line, outline),
            ));
            for end in [points[0], points[points.len() - 1]] {
                self.shapes.push(Shape::circle_filled(end, reach, outline));
            }
        }
        if self.shadow {
            return;
        }
        for (points, width, color) in mapped {
            let (first, last) = (points[0], points[points.len() - 1]);
            self.shapes
                .push(Shape::line(points, Stroke::new(width, color)));
            for end in [first, last] {
                self.shapes
                    .push(Shape::circle_filled(end, width * 0.5, color));
            }
        }
    }

    fn line(&mut self, points: &[Vec2], width: f32, color: Color32) {
        self.lines(&[(points, width, color)]);
    }

    /// A thin line with no outline, for creases and details.
    fn hairline(&mut self, points: &[Vec2], width: f32, color: Color32) {
        if !self.shadow && points.len() >= 2 {
            let points = self.map(points);
            self.shapes
                .push(Shape::line(points, Stroke::new(width * self.scale, color)));
        }
    }

    /// An outlined dot.
    fn dot(&mut self, center: Vec2, radius: f32, color: Color32) {
        let center = self.at(center);
        let radius = radius * self.scale;
        self.shapes
            .push(Shape::circle_filled(center, radius, self.ink(color)));
        self.shapes.push(Shape::circle_stroke(
            center,
            radius,
            Stroke::new(self.line, self.ink(self.pal.outline)),
        ));
    }

    /// A dot with no outline.
    fn spot(&mut self, center: Vec2, radius: f32, color: Color32) {
        if !self.shadow {
            let center = self.at(center);
            self.shapes
                .push(Shape::circle_filled(center, radius * self.scale, color));
        }
    }

    /// A thin circle with no outline: ripples and splashes.
    fn ripple(&mut self, center: Vec2, radius: f32, width: f32, color: Color32) {
        if !self.shadow && radius > 0.0 {
            let center = self.at(center);
            self.shapes.push(Shape::circle_stroke(
                center,
                radius * self.scale,
                Stroke::new(width * self.scale, color),
            ));
        }
    }
}

fn fill_polygon(shapes: &mut Vec<Shape>, points: &[Pos2], color: Color32) {
    if points.len() < 3 || color.a() == 0 {
        return;
    }
    let mut mesh = Mesh::default();
    for &p in points {
        mesh.colored_vertex(p, color);
    }
    for [a, b, c] in triangulate(points) {
        mesh.add_triangle(a, b, c);
    }
    shapes.push(Shape::mesh(mesh));
}

/// Ear clipping for a simple polygon of either winding. egui only fills
/// convex paths, and an arrow or a hand is not convex.
fn triangulate(points: &[Pos2]) -> Vec<[u32; 3]> {
    let n = points.len();
    if n < 3 {
        return Vec::new();
    }
    let twice_area: f32 = (0..n)
        .map(|i| {
            let (p, q) = (points[i], points[(i + 1) % n]);
            p.x * q.y - q.x * p.y
        })
        .sum();
    let sign = if twice_area >= 0.0 { 1.0 } else { -1.0 };
    let turn =
        |a: Pos2, b: Pos2, c: Pos2| ((b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x)) * sign;
    let mut left: Vec<usize> = (0..n).collect();
    let mut out = Vec::with_capacity(n - 2);
    while left.len() > 3 {
        let m = left.len();
        let ear = (0..m).find(|&i| {
            let (ia, ib, ic) = (left[(i + m - 1) % m], left[i], left[(i + 1) % m]);
            let (a, b, c) = (points[ia], points[ib], points[ic]);
            turn(a, b, c) > 1.0e-6
                && !left.iter().any(|&j| {
                    j != ia
                        && j != ib
                        && j != ic
                        && turn(a, b, points[j]) >= 0.0
                        && turn(b, c, points[j]) >= 0.0
                        && turn(c, a, points[j]) >= 0.0
                })
        });
        let Some(i) = ear else {
            break;
        };
        out.push([
            left[(i + m - 1) % m] as u32,
            left[i] as u32,
            left[(i + 1) % m] as u32,
        ]);
        left.remove(i);
    }
    // Whatever is left (three corners, or a degenerate rest) as a fan.
    for k in 1..left.len().saturating_sub(1) {
        out.push([left[0] as u32, left[k] as u32, left[k + 1] as u32]);
    }
    out
}

fn arc(center: Vec2, rx: f32, ry: f32, from: f32, to: f32, steps: usize) -> Vec<Vec2> {
    (0..=steps)
        .map(|i| {
            let a = from + (to - from) * i as f32 / steps as f32;
            center + vec2(a.cos() * rx, a.sin() * ry)
        })
        .collect()
}

/// A triangle head whose tip is at `tip`, pointing along `direction`.
fn head(tip: Vec2, direction: Vec2, length: f32, half_width: f32) -> [Vec2; 3] {
    let d = direction.normalized();
    let n = vec2(-d.y, d.x);
    let base = tip - d * length;
    [tip, base + n * half_width, base - n * half_width]
}

/// Paint `kind` with its hotspot at `at`.
fn paint(
    shapes: &mut Vec<Shape>,
    kind: CursorKind,
    at: Pos2,
    scale: f32,
    pal: Palette,
    m: &Motion,
) {
    if pal.shadow.a() > 0 {
        let mut shadow = Canvas::new(at + vec2(0.5, 1.4) * scale, scale, pal, true);
        draw(&mut shadow, kind, m);
        shapes.extend(shadow.shapes);
    }
    let mut canvas = Canvas::new(at, scale, pal, false);
    draw(&mut canvas, kind, m);
    shapes.extend(canvas.shapes);
}

fn draw(c: &mut Canvas, kind: CursorKind, m: &Motion) {
    match kind {
        CursorKind::Arrow => arrow(c, m),
        CursorKind::Link => link(c, m),
        CursorKind::Text => text(c, m),
        CursorKind::Grab => grab(c, m),
        CursorKind::Grabbing => grabbing(c, m),
        CursorKind::Move => move_arrows(c, m),
        CursorKind::Crosshair => crosshair(c, m),
        CursorKind::ResizeH => resize(c, m, 0.0),
        CursorKind::ResizeV => resize(c, m, FRAC_PI_2),
        CursorKind::ResizeNwSe => resize_corner(c, m, FRAC_PI_4),
        CursorKind::ResizeNeSw => resize_corner(c, m, -FRAC_PI_4),
        CursorKind::Rotate => rotate(c, m),
        CursorKind::SkewH => skew(c, m, 0.0),
        CursorKind::SkewV => skew(c, m, FRAC_PI_2),
        CursorKind::Scrub => scrub(c, m),
        CursorKind::Pen => pen(c, m),
        CursorKind::Pencil => pencil(c, m),
        CursorKind::Bucket => bucket(c, m),
        CursorKind::Eyedropper => eyedropper(c, m),
        CursorKind::Knife => knife(c, m),
        CursorKind::Lasso => lasso(c, m),
        CursorKind::Line | CursorKind::Rect | CursorKind::Oval => shape_tool(c, m, kind),
        CursorKind::LibraryDrop => library_drop(c, m),
        CursorKind::PenClose => pen_close(c, m),
        CursorKind::ScreenPick => screen_pick(c, m),
        CursorKind::FrameDrag => frame_drag(c, m),
        CursorKind::ParentLink => parent_link(c, m),
        CursorKind::KnifeCut => knife_cut(c, m),
        CursorKind::SelectAdd => select_add(c, m),
        CursorKind::SelectRemove => select_remove(c, m),
        CursorKind::CopyDrag => copy_drag(c, m),
        CursorKind::Pivot => pivot(c, m),
        CursorKind::Snap => snap(c, m),
        CursorKind::Locked => locked(c, m),
    }
}

// ---------------------------------------------------------------------------
// The pointers

const ARROW: [Vec2; 7] = [
    vec2(0.0, 0.0),
    vec2(0.0, 16.6),
    vec2(4.0, 12.9),
    vec2(6.8, 18.8),
    vec2(9.5, 17.5),
    vec2(6.8, 11.8),
    vec2(12.0, 11.6),
];

/// Trails behind fast moves like a flag and springs back; a click presses
/// it toward its tip and sends a ripple out.
fn arrow(c: &mut Canvas, m: &Motion) {
    let trail = (m.lean.x * 0.00042).clamp(-0.2, 0.2);
    let squash = 1.0 - 0.1 * m.press();
    c.with(Xf::scale(squash, squash).then(Xf::rotate(trail)), |c| {
        c.body(&ARROW, c.pal.fill);
        // A sliver of the accent along the inner edge.
        c.flat(
            &[v(1.3, 3.4), v(1.3, 12.9), v(3.6, 10.8)],
            c.pal.accent.gamma_multiply(0.85),
        );
    });
    let t = m.pressed / 0.45;
    if t < 1.0 {
        let color = c.pal.accent.gamma_multiply(1.0 - t);
        c.ripple(
            Vec2::ZERO,
            2.0 + 11.0 * ease_out(t),
            1.6 * (1.0 - t) + 0.4,
            color,
        );
    }
}

const HAND: [Vec2; 31] = [
    vec2(-2.0, 1.8),
    vec2(-1.6, 0.5),
    vec2(-0.7, -0.1),
    vec2(0.7, -0.1),
    vec2(1.6, 0.5),
    vec2(2.0, 1.8),
    vec2(2.0, 7.4),
    vec2(2.4, 6.7),
    vec2(3.4, 6.3),
    vec2(4.4, 6.6),
    vec2(5.0, 7.6),
    vec2(5.5, 7.1),
    vec2(6.6, 7.0),
    vec2(7.4, 7.5),
    vec2(7.8, 8.6),
    vec2(8.3, 8.3),
    vec2(9.3, 8.4),
    vec2(10.0, 9.2),
    vec2(10.2, 10.4),
    vec2(10.2, 13.6),
    vec2(9.4, 16.6),
    vec2(7.8, 18.6),
    vec2(7.8, 20.4),
    vec2(-0.2, 20.4),
    vec2(-0.2, 18.8),
    vec2(-2.2, 16.6),
    vec2(-4.8, 13.2),
    vec2(-5.4, 11.6),
    vec2(-4.9, 10.5),
    vec2(-3.7, 10.4),
    vec2(-2.0, 12.2),
];

/// Wags once on arrival; the finger presses in while the button is down.
fn link(c: &mut Canvas, m: &Motion) {
    let wag = 0.2 * wobble(m.enter, 24.0, 6.5);
    let press = m.press().max(0.0) * 1.7;
    let hand: Vec<Vec2> = HAND
        .iter()
        .map(|&p| {
            if p.y < 7.4 {
                p + vec2(0.0, press * (1.0 - p.y.max(0.0) / 7.4))
            } else {
                p
            }
        })
        .collect();
    c.with(Xf::rotate(wag), |c| {
        c.body(&hand, c.pal.fill);
        let crease = c.pal.outline;
        c.hairline(&[v(2.0, 7.4), v(2.0, 10.6)], 1.0, crease);
        c.hairline(&[v(5.0, 7.6), v(5.0, 10.8)], 1.0, crease);
        c.hairline(&[v(7.8, 8.6), v(7.8, 11.2)], 1.0, crease);
        c.body(
            &[v(-0.7, 18.4), v(8.3, 18.4), v(8.3, 21.2), v(-0.7, 21.2)],
            c.pal.accent,
        );
    });
}

/// Unfolds from the middle: the stem grows, then the serifs open.
fn text(c: &mut Canvas, m: &Motion) {
    let grow = ease_back(m.enter / 0.16);
    let open = ease_out((m.enter - 0.08) / 0.2);
    let h = 7.6 * grow;
    let serif = |y: f32, s: f32| {
        vec![
            v(-3.0 * open, y - 1.0 * s * open),
            v(-open, y - 0.55 * s * open),
            v(0.0, y),
            v(open, y - 0.55 * s * open),
            v(3.0 * open, y - 1.0 * s * open),
        ]
    };
    let fill = c.pal.fill;
    let stem = [v(0.0, -h), v(0.0, h)];
    let top = serif(-h, 1.0);
    let bottom = serif(h, -1.0);
    c.lines(&[
        (&stem[..], 1.5, fill),
        (&top[..], 1.3, fill),
        (&bottom[..], 1.3, fill),
    ]);
    let tick = 1.3 * open;
    c.hairline(&[v(-tick, 0.0), v(tick, 0.0)], 1.2, c.pal.accent);
}

const PALM: [Vec2; 8] = [
    vec2(-5.2, -2.6),
    vec2(5.2, -2.6),
    vec2(5.8, 1.0),
    vec2(4.8, 5.6),
    vec2(2.6, 7.6),
    vec2(-2.6, 7.6),
    vec2(-4.8, 5.6),
    vec2(-5.8, 1.0),
];

const CUFF: [Vec2; 4] = [
    vec2(-3.2, 7.0),
    vec2(3.2, 7.0),
    vec2(3.2, 9.6),
    vec2(-3.2, 9.6),
];

/// The fingers fan open one by one, little finger last.
fn grab(c: &mut Canvas, m: &Motion) {
    const BASES: [f32; 4] = [-3.4, -1.15, 1.15, 3.4];
    const ANGLES: [f32; 4] = [-0.27, -0.09, 0.09, 0.27];
    const LENGTHS: [f32; 4] = [6.2, 7.6, 7.2, 5.8];
    let fill = c.pal.fill;
    let fingers: Vec<[Vec2; 2]> = (0..4)
        .map(|i| {
            let s = ease_back((m.enter - 0.045 * i as f32) / 0.24);
            let angle = ANGLES[i] * (0.2 + 0.8 * s);
            let length = LENGTHS[i] * (0.7 + 0.3 * s);
            let base = v(BASES[i], -1.6);
            [base, base + vec2(angle.sin(), -angle.cos()) * length]
        })
        .collect();
    let thumb_open = ease_back((m.enter - 0.2) / 0.24);
    let thumb_angle = -0.7 - 0.45 * thumb_open;
    let thumb = [
        v(-4.4, 2.6),
        v(-4.4, 2.6) + vec2(thumb_angle.sin(), -thumb_angle.cos()) * 5.2,
    ];
    let mut lines: Vec<(&[Vec2], f32, Color32)> =
        fingers.iter().map(|f| (&f[..], 2.6, fill)).collect();
    lines.push((&thumb[..], 2.6, fill));
    c.lines(&lines);
    c.body(&PALM, fill);
    c.body(&CUFF, c.pal.accent);
}

/// Squeezes on the grab and stretches along fast drags.
fn grabbing(c: &mut Canvas, m: &Motion) {
    let squeeze = 0.17 * wobble(m.enter, 20.0, 8.0);
    let mut xf = Xf::scale(1.0 + squeeze, 1.0 - squeeze);
    let speed = m.lean.length();
    if speed > 1.0 {
        let k = 1.0 + (speed * 0.00032).min(0.2);
        xf = xf.then(Xf::stretch(m.lean / speed, k));
    }
    let fill = c.pal.fill;
    c.with(xf, |c| {
        c.body(
            &[
                v(-5.4, -1.2),
                v(-4.6, -3.4),
                v(4.6, -3.4),
                v(5.4, -1.2),
                v(5.6, 3.0),
                v(4.6, 6.2),
                v(2.6, 7.6),
                v(-2.6, 7.6),
                v(-4.6, 6.2),
                v(-5.6, 3.0),
            ],
            fill,
        );
        let crease = c.pal.outline;
        for x in [-2.6, 0.0, 2.6] {
            c.hairline(&[v(x, -3.4), v(x, -0.6)], 1.0, crease);
        }
        c.hairline(&[v(-4.8, -0.6), v(4.8, -0.6)], 1.0, crease);
        c.line(&[v(-4.9, 2.6), v(0.6, 1.0)], 2.4, fill);
        c.body(&CUFF, c.pal.accent);
    });
}

/// The four heads pop out in turn, clockwise from the top; while dragging
/// the one facing the way you go reaches ahead.
fn move_arrows(c: &mut Canvas, m: &Motion) {
    const WAYS: [Vec2; 4] = [
        vec2(0.0, -1.0),
        vec2(1.0, 0.0),
        vec2(0.0, 1.0),
        vec2(-1.0, 0.0),
    ];
    let speed = m.lean.length();
    let reach: Vec<f32> = WAYS
        .iter()
        .enumerate()
        .map(|(i, way)| {
            let pop = 2.6 * pulse(m.enter - 0.06 * i as f32, 0.24);
            let lead = if m.held && speed > 1.0 {
                2.4 * way.dot(m.lean / speed).max(0.0) * (speed / 500.0).min(1.0)
            } else {
                0.0
            };
            pop + lead
        })
        .collect();
    let fill = c.pal.fill;
    let across = [v(-6.4 - reach[3], 0.0), v(6.4 + reach[1], 0.0)];
    let down = [v(0.0, -6.4 - reach[0]), v(0.0, 6.4 + reach[2])];
    c.lines(&[(&across[..], 1.5, fill), (&down[..], 1.5, fill)]);
    for (way, reach) in WAYS.iter().zip(&reach) {
        c.body(&head(*way * (10.0 + reach), *way, 4.2, 3.2), fill);
    }
    c.dot(Vec2::ZERO, 1.6, c.pal.accent);
}

/// Locks on: the arms close in with a quarter turn, then it holds still.
/// The centre swells while the button is down.
fn crosshair(c: &mut Canvas, m: &Motion) {
    let lock = ease_out(m.enter / 0.24);
    let gap = 5.0 * (1.0 - lock);
    let turn = FRAC_PI_4 * (1.0 - lock);
    let fill = c.pal.fill;
    c.with(Xf::rotate(turn), |c| {
        let arms: Vec<[Vec2; 2]> = [v(1.0, 0.0), v(-1.0, 0.0), v(0.0, 1.0), v(0.0, -1.0)]
            .iter()
            .map(|&d| [d * (2.8 + gap), d * (9.2 + gap)])
            .collect();
        let lines: Vec<(&[Vec2], f32, Color32)> =
            arms.iter().map(|arm| (&arm[..], 1.4, fill)).collect();
        c.lines(&lines);
    });
    c.dot(Vec2::ZERO, 1.15 + 0.7 * m.press().max(0.0), c.pal.accent);
}

/// The double arrow of a side handle, turned by `angle`. Stretches like
/// elastic on arrival; while dragging the leading head reaches out.
fn resize(c: &mut Canvas, m: &Motion, angle: f32) {
    let axis = vec2(angle.cos(), angle.sin());
    let bounce = 2.6 * wobble(m.enter, 17.0, 5.5);
    let along = if m.held {
        m.lean.dot(axis) * 0.0045
    } else {
        0.0
    };
    let ahead = bounce + along.clamp(0.0, 2.6);
    let behind = bounce + (-along).clamp(0.0, 2.6);
    let fill = c.pal.fill;
    c.with(Xf::rotate(angle), |c| {
        c.line(&[v(-6.0 - behind, 0.0), v(6.0 + ahead, 0.0)], 1.5, fill);
        c.body(&head(v(10.2 + ahead, 0.0), v(1.0, 0.0), 4.2, 3.4), fill);
        c.body(&head(v(-10.2 - behind, 0.0), v(-1.0, 0.0), 4.2, 3.4), fill);
        // The edge being dragged.
        c.line(&[v(0.0, -3.8), v(0.0, 3.8)], 1.6, c.pal.accent);
    });
}

/// The double arrow of a corner handle. Its corner bracket slides into the
/// corner on arrival.
fn resize_corner(c: &mut Canvas, m: &Motion, angle: f32) {
    let settle = ease_back(m.enter / 0.3);
    let fill = c.pal.fill;
    c.with(Xf::rotate(angle), |c| {
        c.line(&[v(-6.0, 0.0), v(6.0, 0.0)], 1.5, fill);
        c.body(&head(v(10.2, 0.0), v(1.0, 0.0), 4.2, 3.4), fill);
        c.body(&head(v(-10.2, 0.0), v(-1.0, 0.0), 4.2, 3.4), fill);
    });
    // A corner of the box, sliding in along the diagonal.
    let slide = (1.0 - settle) * 3.2;
    c.with(
        Xf::translate(vec2(-slide, 0.0)).then(Xf::rotate(angle - FRAC_PI_4)),
        |c| {
            let bracket = [v(-1.4, 3.4), v(1.9, 1.9), v(3.4, -1.4)];
            c.line(&bracket, 1.5, c.pal.accent);
        },
    );
}

/// Draws its arc on arrival; while dragging the arc turns with the pointer.
fn rotate(c: &mut Canvas, m: &Motion) {
    let spin = if m.held { m.sweep * 0.03 } else { 0.0 };
    let sweep = 1.55 * PI * ease_out(m.enter / 0.34).max(0.05);
    let start = -0.9 * PI + spin;
    let end = start + sweep;
    let radius = 7.4;
    let fill = c.pal.fill;
    let points = arc(Vec2::ZERO, radius, radius, start, end, 30);
    c.line(&points, 1.7, fill);
    let tangent = vec2(-end.sin(), end.cos());
    let tip = vec2(end.cos(), end.sin()) * radius + tangent * 3.2;
    c.body(&head(tip, tangent, 4.6, 3.3), fill);
    c.dot(Vec2::ZERO, 1.5, c.pal.accent);
}

/// Two opposite arrows on either side of a slanted accent line. On arrival
/// they slide apart while the line leans; while dragging they shear with the
/// pointer.
fn skew(c: &mut Canvas, m: &Motion, angle: f32) {
    let settle = ease_back(m.enter / 0.32);
    let axis = vec2(angle.cos(), angle.sin());
    let drag = if m.held {
        (m.lean.dot(axis) * 0.0035).clamp(-2.2, 2.2)
    } else {
        0.0
    };
    let d = -2.4 * (1.0 - settle) + drag;
    let fill = c.pal.fill;
    c.with(Xf::rotate(angle), |c| {
        c.with(Xf::shear_x(-0.38 * settle - drag * 0.08), |c| {
            c.line(&[v(0.0, 5.0), v(0.0, -5.0)], 1.5, c.pal.accent);
        });
        c.line(&[v(-7.0 + d, -3.4), v(5.0 + d, -3.4)], 1.5, fill);
        c.body(&head(v(9.0 + d, -3.4), v(1.0, 0.0), 4.0, 3.0), fill);
        c.line(&[v(7.0 - d, 3.4), v(-5.0 - d, 3.4)], 1.5, fill);
        c.body(&head(v(-9.0 - d, 3.4), v(-1.0, 0.0), 4.0, 3.0), fill);
    });
}

/// A value pill between chevrons. While dragging the chevrons run outward
/// like a belt, and the pill leans the way you drag.
fn scrub(c: &mut Canvas, m: &Motion) {
    let flow = if m.held {
        (m.sweep.abs() * 0.07).rem_euclid(1.0)
    } else {
        0.0
    };
    let lean = if m.held {
        (m.lean.x * 0.0025).clamp(-1.3, 1.3)
    } else {
        0.0
    };
    let fill = c.pal.fill;
    let chevrons: Vec<[Vec2; 3]> = [-1.0_f32, 1.0]
        .iter()
        .flat_map(|&side| {
            (0..2).map(move |i| {
                let x = side * (4.6 + 2.9 * ((i as f32 + flow) % 2.0));
                [v(x - side * 1.8, -2.8), v(x, 0.0), v(x - side * 1.8, 2.8)]
            })
        })
        .collect();
    let lines: Vec<(&[Vec2], f32, Color32)> =
        chevrons.iter().map(|ch| (&ch[..], 1.35, fill)).collect();
    c.lines(&lines);
    let pill = arc(v(lean, -2.9), 1.7, 1.7, PI, TAU, 6)
        .into_iter()
        .chain(arc(v(lean, 2.9), 1.7, 1.7, 0.0, PI, 6))
        .collect::<Vec<_>>();
    c.body(&pill, c.pal.accent);
}

/// The Pen: dips in like a nib into ink, overshooting a touch, and leaves an
/// ink dot under the tip while the button is down.
fn pen(c: &mut Canvas, m: &Motion) {
    let t = m.enter / 0.38;
    let lift = (1.0 - ease_back(t)) * 5.0;
    let tilt = 0.7 + 0.35 * (1.0 - ease_out(t));
    pen_body(c, lift, tilt);
    let ink = if m.held {
        ease_out(m.pressed / 0.12)
    } else {
        1.0 - clamp01(m.release / 0.5)
    };
    if ink > 0.0 && m.pressed < 100.0 {
        let accent = c.pal.accent;
        c.spot(Vec2::ZERO, 1.9 * ink.max(0.3), accent.gamma_multiply(ink));
    }
}

/// The pen itself, lifted along its axis by `lift` and leaning by `tilt`.
fn pen_body(c: &mut Canvas, lift: f32, tilt: f32) {
    let fill = c.pal.fill;
    let accent = c.pal.accent;
    let outline = c.pal.outline;
    c.with(
        Xf::translate(vec2(0.0, -lift)).then(Xf::rotate(tilt)),
        |c| {
            c.body(
                &[v(-2.5, -10.2), v(2.5, -10.2), v(2.2, -17.2), v(-2.2, -17.2)],
                fill,
            );
            c.body(
                &[v(-3.0, -8.4), v(3.0, -8.4), v(3.0, -10.4), v(-3.0, -10.4)],
                accent,
            );
            c.body(
                &[
                    v(0.0, 0.3),
                    v(-2.9, -6.2),
                    v(-2.6, -8.6),
                    v(2.6, -8.6),
                    v(2.9, -6.2),
                ],
                fill,
            );
            c.hairline(&[v(0.0, -0.9), v(0.0, -5.0)], 0.9, outline);
            c.spot(v(0.0, -5.6), 0.85, outline);
        },
    );
}

/// The Pencil: swings into hand on arrival and tilts with the stroke.
fn pencil(c: &mut Canvas, m: &Motion) {
    let swing = 0.55 * (1.0 - ease_back(m.enter / 0.42));
    let lean = if m.held {
        (m.lean.x * 0.00035).clamp(-0.2, 0.2)
    } else {
        0.0
    };
    let fill = c.pal.fill;
    let accent = c.pal.accent;
    let outline = c.pal.outline;
    c.with(Xf::rotate(0.62 + swing + lean), |c| {
        c.body(
            &[
                v(-2.6, -16.8),
                v(2.6, -16.8),
                v(2.6, -18.6),
                v(1.6, -19.6),
                v(-1.6, -19.6),
                v(-2.6, -18.6),
            ],
            mix(accent, fill, 0.6),
        );
        c.body(
            &[v(-2.6, -15.2), v(2.6, -15.2), v(2.6, -16.9), v(-2.6, -16.9)],
            fill,
        );
        c.body(
            &[v(-2.6, -5.6), v(2.6, -5.6), v(2.6, -15.3), v(-2.6, -15.3)],
            accent,
        );
        c.hairline(&[v(0.9, -5.8), v(0.9, -15.1)], 0.9, mix(accent, fill, 0.45));
        c.body(
            &[v(-1.0, -2.0), v(-2.6, -5.6), v(2.6, -5.6), v(1.0, -2.0)],
            fill,
        );
        c.body(&[v(0.0, 0.2), v(-1.0, -2.0), v(1.0, -2.0)], outline);
    });
}

/// The Paint Bucket, tipped so its lip drips onto the hotspot. Still until
/// you click; then it tips further, a drop falls and splashes.
fn bucket(c: &mut Canvas, m: &Motion) {
    let pour = pulse(m.pressed, 0.5);
    let fill = c.pal.fill;
    let accent = c.pal.accent;
    let tilt = -0.8 - 0.22 * pour;
    let center = v(8.4, -6.6);
    c.with(Xf::rotate(tilt).then(Xf::translate(center)), |c| {
        let handle = arc(v(0.0, -4.4), 4.6, 4.6, PI, TAU, 14);
        c.line(&handle, 1.0, fill);
        c.body(
            &[v(-4.6, -4.4), v(4.6, -4.4), v(3.6, 4.4), v(-3.6, 4.4)],
            fill,
        );
        let paint = arc(v(0.0, -4.4), 4.6, 1.5, 0.0, TAU, 16);
        c.body(&paint, accent);
    });
    // The lip the paint runs over, after the tilt.
    let lip = center + Xf::rotate(tilt).apply(v(-4.6, -4.4));
    c.line(&[lip, v(0.5, -3.4)], 1.5, accent);
    let mut drop: Vec<Vec2> = arc(v(0.0, -1.7), 1.7, 1.7, -0.25 * PI, 1.25 * PI, 10);
    drop.push(v(0.4, -4.6));
    c.body(&drop, accent);
    let t = m.pressed / 0.42;
    if t < 1.0 {
        let falling = lerp(lip, v(0.0, 4.0), ease_out(t) * t.sqrt());
        c.spot(falling, 1.5 * (1.0 - t * 0.6), accent);
    }
    let splash = (m.pressed - 0.22) / 0.42;
    if (0.0..1.0).contains(&splash) {
        let color = accent.gamma_multiply(1.0 - splash);
        let rx = 1.5 + 6.0 * ease_out(splash);
        let ring = arc(Vec2::ZERO, rx, rx * 0.36, 0.0, TAU, 20);
        c.hairline(&ring, 1.1, color);
    }
}

/// The Eyedropper. Its tube holds the fill colour. A press squeezes the
/// bulb and, let go, draws the colour up the tube before it drains back.
fn eyedropper(c: &mut Canvas, m: &Motion) {
    let squeeze = m.press().max(0.0);
    let rise = if m.held || m.pressed >= 100.0 {
        0.0
    } else {
        ease_out(m.release / 0.28) * (1.0 - ease_out((m.release - 0.55) / 0.55))
    };
    let level = (0.42 - 0.14 * squeeze + 0.44 * rise).clamp(0.08, 0.95);
    let fill = c.pal.fill;
    let accent = c.pal.accent;
    let liquid = m.sample.unwrap_or(accent);
    let bulb = 2.3 * (1.0 - 0.28 * squeeze);
    c.with(Xf::rotate(0.78), |c| {
        let mut top = arc(v(0.0, -16.2), bulb, 2.1, 0.0, -PI, 10);
        top.insert(0, v(bulb, -12.6));
        top.push(v(-bulb, -12.6));
        c.body(&top, accent);
        c.body(
            &[v(-2.4, -11.0), v(2.4, -11.0), v(2.4, -12.7), v(-2.4, -12.7)],
            fill,
        );
        c.body(
            &[
                v(0.0, 0.2),
                v(-0.8, -1.8),
                v(-1.5, -4.0),
                v(-1.5, -11.0),
                v(1.5, -11.0),
                v(1.5, -4.0),
                v(0.8, -1.8),
            ],
            fill,
        );
        let surface = 2.4 + level * 7.8;
        c.flat(
            &[
                v(0.0, -0.7),
                v(-0.75, -2.4),
                v(-0.8, -surface),
                v(0.8, -surface),
                v(0.75, -2.4),
            ],
            liquid,
        );
        let bubble = m.enter / 0.6;
        if bubble < 1.0 {
            let y = -2.6 - (surface - 2.6) * ease_out(bubble);
            c.ripple(v(0.1, y), 0.5, 0.6, fill.gamma_multiply(1.0 - bubble * 0.5));
        }
    });
}

/// The Knife: still, with a glint running up the edge when you press.
fn knife(c: &mut Canvas, m: &Motion) {
    knife_body(c, 0.55, m.pressed / 0.32);
}

/// The knife turned by `angle`, with a glint `glint` of the way up its edge
/// (none outside 0..1).
fn knife_body(c: &mut Canvas, angle: f32, glint: f32) {
    let fill = c.pal.fill;
    let accent = c.pal.accent;
    let outline = c.pal.outline;
    const EDGE: [Vec2; 4] = [
        vec2(0.0, 0.0),
        vec2(2.6, -4.6),
        vec2(3.0, -9.0),
        vec2(2.9, -11.6),
    ];
    c.with(Xf::rotate(angle), |c| {
        c.body(
            &[
                v(-0.9, -12.8),
                v(2.9, -12.8),
                v(2.7, -19.6),
                v(1.0, -20.4),
                v(-0.7, -19.6),
            ],
            accent,
        );
        c.spot(v(1.0, -15.0), 0.6, fill);
        c.spot(v(1.0, -17.8), 0.6, fill);
        c.body(
            &[v(-1.4, -11.4), v(3.4, -11.4), v(3.4, -12.8), v(-1.4, -12.8)],
            outline,
        );
        c.body(
            &[
                EDGE[0],
                EDGE[1],
                EDGE[2],
                EDGE[3],
                v(-0.9, -11.6),
                v(-0.9, -3.4),
            ],
            fill,
        );
        let t = glint;
        if (0.0..1.0).contains(&t) {
            let along = |s: f32| {
                let s = clamp01(s) * 3.0;
                let i = (s as usize).min(2);
                lerp(EDGE[i], EDGE[i + 1], s - i as f32)
            };
            let shine = mix(fill, Color32::WHITE, 0.6).gamma_multiply(pulse(t, 1.0));
            c.hairline(&[along(t - 0.12), along(t + 0.12)], 1.3, shine);
        }
    });
}

/// The Lasso: the loop spins once on arrival, and its dashes run along
/// while you draw.
fn lasso(c: &mut Canvas, m: &Motion) {
    const DASHES: usize = 9;
    let spin = ease_out(m.enter / 0.6) * DASHES as f32 + if m.held { m.travel * 0.05 } else { 0.0 };
    let center = v(6.4, -7.0);
    let tilt = Xf::rotate(-0.35);
    let on_loop = |a: f32| center + tilt.apply(vec2(a.cos() * 5.8, a.sin() * 3.7));
    let step = TAU / DASHES as f32;
    let dashes: Vec<Vec<Vec2>> = (0..DASHES)
        .map(|i| {
            let from = (i as f32 + spin) * step;
            (0..=4)
                .map(|k| on_loop(from + step * 0.62 * k as f32 / 4.0))
                .collect()
        })
        .collect();
    let start = on_loop(2.35);
    let rope: Vec<Vec2> = (0..=8)
        .map(|k| {
            let t = k as f32 / 8.0;
            let bend = v(1.2, -2.2);
            start * (1.0 - t) * (1.0 - t) + bend * 2.0 * t * (1.0 - t)
        })
        .collect();
    let fill = c.pal.fill;
    let mut lines: Vec<(&[Vec2], f32, Color32)> =
        dashes.iter().map(|d| (&d[..], 1.5, fill)).collect();
    lines.push((&rope[..], 1.4, fill));
    c.lines(&lines);
    c.dot(Vec2::ZERO, 1.3, c.pal.accent);
}

/// A plain crosshair with a badge of the shape the tool draws, which pops in
/// after the crosshair arrives.
fn shape_tool(c: &mut Canvas, m: &Motion, kind: CursorKind) {
    let fill = c.pal.fill;
    let accent = c.pal.accent;
    let arms: Vec<[Vec2; 2]> = [v(1.0, 0.0), v(-1.0, 0.0), v(0.0, 1.0), v(0.0, -1.0)]
        .iter()
        .map(|&d| [d * 2.4, d * 7.4])
        .collect();
    let lines: Vec<(&[Vec2], f32, Color32)> =
        arms.iter().map(|arm| (&arm[..], 1.3, fill)).collect();
    c.lines(&lines);
    let pop = ease_back((m.enter - 0.06) / 0.24);
    if pop <= 0.0 {
        return;
    }
    c.with(
        Xf::scale(pop, pop).then(Xf::translate(v(8.2, 8.0))),
        |c| match kind {
            CursorKind::Line => {
                c.line(&[v(-3.0, 2.4), v(3.0, -2.4)], 1.3, accent);
                c.dot(v(-3.0, 2.4), 0.9, fill);
                c.dot(v(3.0, -2.4), 0.9, fill);
            }
            CursorKind::Oval => {
                let ring = arc(Vec2::ZERO, 3.3, 2.5, 0.0, TAU, 20);
                c.line(&ring, 1.3, accent);
            }
            _ => {
                let square = [
                    v(-3.1, -2.4),
                    v(3.1, -2.4),
                    v(3.1, 2.4),
                    v(-3.1, 2.4),
                    v(-3.1, -2.4),
                ];
                c.line(&square, 1.3, accent);
            }
        },
    );
}

// ---------------------------------------------------------------------------
// Action pointers

/// The idle arrow a size down, for pointers that carry something.
fn small_arrow(c: &mut Canvas) {
    c.with(Xf::scale(0.82, 0.82), |c| c.body(&ARROW, c.pal.fill));
}

/// A round accent badge at `at`, grown to `size` (0 hidden, 1 full).
fn badge(c: &mut Canvas, at: Vec2, size: f32, draw: impl FnOnce(&mut Canvas)) {
    if size <= 0.0 {
        return;
    }
    c.with(Xf::scale(size, size).then(Xf::translate(at)), |c| {
        let disc = arc(Vec2::ZERO, 4.3, 4.3, 0.0, TAU, 20);
        c.body(&disc, c.pal.accent);
        draw(c);
    });
}

/// A closed ellipse turned by `angle`.
fn ellipse(center: Vec2, rx: f32, ry: f32, angle: f32) -> Vec<Vec2> {
    let turn = Xf::rotate(angle);
    arc(Vec2::ZERO, rx, ry, 0.0, TAU, 24)
        .into_iter()
        .map(|p| center + turn.apply(p))
        .collect()
}

/// Dragging a Library item: its card drops in on a string from the pointer
/// and swings like a pendulum as you move.
fn library_drop(c: &mut Canvas, m: &Motion) {
    small_arrow(c);
    let swing = (-m.lean.x * 0.0011).clamp(-0.5, 0.5) + 0.35 * wobble(m.enter, 13.0, 5.0);
    let drop = (1.0 - ease_back(m.enter / 0.3)) * -6.0;
    let fill = c.pal.fill;
    let accent = c.pal.accent;
    c.with(
        Xf::rotate(swing).then(Xf::translate(v(6.4, 11.0 + drop))),
        |c| {
            c.line(&[v(0.0, 0.0), v(0.0, 2.6)], 0.9, fill);
            c.body(
                &[v(-4.2, 2.4), v(4.2, 2.4), v(4.2, 10.4), v(-4.2, 10.4)],
                accent,
            );
            c.body(&[v(0.0, 4.0), v(2.4, 6.4), v(0.0, 8.8), v(-2.4, 6.4)], fill);
        },
    );
}

/// The Pen over the path's first point: a ring draws itself shut around the
/// nib, then pulses out once.
fn pen_close(c: &mut Canvas, m: &Motion) {
    pen_body(c, 0.0, 0.7);
    let accent = c.pal.accent;
    let sweep = TAU * ease_out(m.enter / 0.36).max(0.04);
    let ring = arc(Vec2::ZERO, 3.6, 3.6, -FRAC_PI_2, -FRAC_PI_2 + sweep, 24);
    c.line(&ring, 1.4, accent);
    let t = (m.enter - 0.36) / 0.4;
    if (0.0..1.0).contains(&t) {
        c.ripple(
            Vec2::ZERO,
            3.6 + 5.0 * ease_out(t),
            1.2 * (1.0 - t) + 0.3,
            accent.gamma_multiply(1.0 - t),
        );
    }
}

/// Picking a colour off the screen: a loupe ring that holds the colour under
/// it, live. It zooms in on arrival and its notches turn as you sweep.
///
/// The screen is read where the pointer is, and this pointer is painted
/// there too. Anything drawn over the hotspot would be read back as the
/// colour (the loupe's own marks flickered in, inverted), so the middle of
/// the ring stays empty, shadow included, with room for a fast sweep.
fn screen_pick(c: &mut Canvas, m: &Motion) {
    const CLEAR: f32 = 6.0;
    let zoom = ease_back(m.enter / 0.26).max(0.2);
    let turn = m.travel * 0.02 + (1.0 - ease_out(m.enter / 0.36)) * 1.4;
    let fill = c.pal.fill;
    let outline = c.pal.outline;
    let color = m.sample.unwrap_or(c.pal.accent);
    // Grows outward from the clear middle, never into it.
    let band = 2.8 * zoom;
    let radius = CLEAR + 1.6 + band * 0.5;
    let ring = arc(Vec2::ZERO, radius, radius, 0.0, TAU, 40);
    let outer = radius + band * 0.5;
    c.line(
        &[v(0.72, 0.72) * outer, v(0.72, 0.72) * (outer + 5.4)],
        2.6,
        fill,
    );
    c.line(&ring, band, color);
    for k in 0..6 {
        let a = turn + k as f32 * TAU / 6.0;
        let d = vec2(a.cos(), a.sin());
        c.hairline(&[d * (radius - band * 0.5), d * outer], 0.9, outline);
    }
}

/// Moving frames on the timeline: a keyframe diamond that pops in, with
/// ghosts trailing the way it came.
fn frame_drag(c: &mut Canvas, m: &Motion) {
    let pop = ease_back(m.enter / 0.24).max(0.1);
    let speed = m.lean.length();
    let reach = (speed / 600.0).min(1.0);
    let back = if speed > 1.0 {
        -m.lean / speed
    } else {
        v(-1.0, 0.0)
    };
    let diamond = |s: f32| {
        [
            v(0.0, -4.6 * s),
            v(4.0 * s, 0.0),
            v(0.0, 4.6 * s),
            v(-4.0 * s, 0.0),
        ]
    };
    let fill = c.pal.fill;
    let accent = c.pal.accent;
    if reach > 0.05 {
        for (step, strength) in [(2.0_f32, 0.3_f32), (1.0, 0.55)] {
            c.with(Xf::translate(back * 4.6 * step * reach), |c| {
                c.flat(&diamond(0.8), accent.gamma_multiply(strength * reach));
            });
        }
    }
    c.line(&[v(-7.6, -2.4), v(-9.4, 0.0), v(-7.6, 2.4)], 1.2, fill);
    c.line(&[v(7.6, -2.4), v(9.4, 0.0), v(7.6, 2.4)], 1.2, fill);
    c.with(Xf::scale(pop, pop), |c| c.body(&diamond(1.0), accent));
}

/// Linking a layer to a parent: two chain links snap together on arrival
/// and the chain swings with the drag from its hook at the hotspot.
fn parent_link(c: &mut Canvas, m: &Motion) {
    let joined = ease_back(m.enter / 0.3);
    let swing = (m.lean.x * 0.0009).clamp(-0.45, 0.45) + 0.25 * wobble(m.enter - 0.2, 20.0, 8.0);
    let fill = c.pal.fill;
    let accent = c.pal.accent;
    c.with(Xf::rotate(swing), |c| {
        let near = ellipse(v(3.4, -3.4), 3.6, 2.1, -FRAC_PI_4);
        let far = ellipse(
            v(7.9, -7.9) + v(3.0, -3.0) * (1.0 - joined),
            3.6,
            1.0,
            -FRAC_PI_4,
        );
        c.lines(&[(&near[..], 1.5, fill), (&far[..], 1.5, accent)]);
    });
    c.dot(Vec2::ZERO, 1.2, accent);
}

/// The Knife mid-cut: the blade leans back against the way it cuts and
/// leaves a slash behind; sparks fly as it bites.
fn knife_cut(c: &mut Canvas, m: &Motion) {
    let speed = m.lean.length();
    let accent = c.pal.accent;
    if speed > 1.0 {
        let back = -m.lean / speed;
        let length = 15.0 * (speed / 400.0).min(1.0);
        for k in 0..3 {
            let from = 1.6 + length * k as f32 / 3.0;
            let to = from + length / 3.0 * 0.62;
            let fade = 1.0 - k as f32 / 3.0;
            c.hairline(&[back * from, back * to], 1.4, accent.gamma_multiply(fade));
        }
    }
    let angle = 0.55 - (m.lean.x * 0.0007).clamp(-0.5, 0.5);
    knife_body(c, angle, -1.0);
    let t = m.pressed / 0.3;
    if (0.0..1.0).contains(&t) {
        for d in [v(-0.8, 0.6), v(0.3, 1.0), v(1.0, 0.2)] {
            c.spot(d * (2.0 + 6.0 * ease_out(t)), 0.9 * (1.0 - t) + 0.2, accent);
        }
    }
}

/// Shift over something unselected: the plus spins into its badge.
fn select_add(c: &mut Canvas, m: &Motion) {
    small_arrow(c);
    let size = ease_back(m.enter / 0.26);
    let spin = FRAC_PI_2 * (1.0 - ease_out(m.enter / 0.32));
    badge(c, v(10.6, 13.8), size, |c| {
        let fill = c.pal.fill;
        c.with(Xf::rotate(spin), |c| {
            c.hairline(&[v(-2.2, 0.0), v(2.2, 0.0)], 1.5, fill);
            c.hairline(&[v(0.0, -2.2), v(0.0, 2.2)], 1.5, fill);
        });
    });
}

/// Shift over something selected: the badge drops in and the minus wipes
/// open from its middle.
fn select_remove(c: &mut Canvas, m: &Motion) {
    small_arrow(c);
    let drop = (1.0 - ease_back(m.enter / 0.28)) * -4.0;
    let wipe = 2.3 * ease_out((m.enter - 0.1) / 0.22);
    badge(c, v(10.6, 13.8 + drop), 1.0, |c| {
        if wipe > 0.05 {
            let fill = c.pal.fill;
            c.hairline(&[v(-wipe, 0.0), v(wipe, 0.0)], 1.6, fill);
        }
    });
}

/// Alt over a selected object: a stack of two cards. The copy slides off on
/// arrival and peels away the way you drag.
fn copy_drag(c: &mut Canvas, m: &Motion) {
    small_arrow(c);
    let spread = ease_back(m.enter / 0.3);
    let mut peel = if m.held { m.lean * 0.006 } else { Vec2::ZERO };
    if peel.length() > 3.0 {
        peel = peel.normalized() * 3.0;
    }
    let card = |o: Vec2| [o, o + v(6.4, 0.0), o + v(6.4, 7.6), o + v(0.0, 7.6)];
    let base = v(8.6, 11.6);
    let copy = base + v(2.2, 2.2) * spread + peel;
    c.body(&card(base), c.pal.fill);
    c.body(&card(copy), c.pal.accent);
    let center = copy + v(3.2, 3.8);
    let fill = c.pal.fill;
    c.hairline(&[center - v(1.7, 0.0), center + v(1.7, 0.0)], 1.3, fill);
    c.hairline(&[center - v(0.0, 1.7), center + v(0.0, 1.7)], 1.3, fill);
}

/// The transform anchor: a target whose satellite orbits once on arrival
/// and again as you move the anchor.
fn pivot(c: &mut Canvas, m: &Motion) {
    let fill = c.pal.fill;
    let accent = c.pal.accent;
    let ring = arc(Vec2::ZERO, 4.4, 4.4, 0.0, TAU, 24);
    let arms: Vec<[Vec2; 2]> = [v(1.0, 0.0), v(-1.0, 0.0), v(0.0, 1.0), v(0.0, -1.0)]
        .iter()
        .map(|&d| [d * 4.4, d * 8.0])
        .collect();
    let mut lines: Vec<(&[Vec2], f32, Color32)> =
        arms.iter().map(|arm| (&arm[..], 1.3, fill)).collect();
    lines.push((&ring[..], 1.4, fill));
    c.lines(&lines);
    c.dot(Vec2::ZERO, 1.3, accent);
    let orbit =
        -FRAC_PI_2 + TAU * ease_out(m.enter / 0.6) + if m.held { m.travel * 0.04 } else { 0.0 };
    c.dot(vec2(orbit.cos(), orbit.sin()) * 6.4, 1.25, accent);
}

/// Something to snap to under the pointer: a magnet clicks in toward the
/// hotspot and a spark jumps the gap.
fn snap(c: &mut Canvas, m: &Motion) {
    let pull = (1.0 - ease_out(m.enter / 0.16)) * 3.4;
    let fill = c.pal.fill;
    let accent = c.pal.accent;
    c.with(
        Xf::rotate(FRAC_PI_4).then(Xf::translate(v(5.6 + pull, -5.6 - pull))),
        |c| {
            let mut horseshoe = vec![v(-3.2, 0.6)];
            horseshoe.extend(arc(v(0.0, -2.6), 3.2, 3.2, PI, TAU, 14));
            horseshoe.push(v(3.2, 0.6));
            c.line(&horseshoe, 2.4, fill);
            let poles = [[v(-3.2, -0.4), v(-3.2, 1.4)], [v(3.2, -0.4), v(3.2, 1.4)]];
            c.lines(&[(&poles[0][..], 2.4, accent), (&poles[1][..], 2.4, accent)]);
        },
    );
    let spark = pulse(m.enter - 0.1, 0.3);
    if spark > 0.0 {
        for a in [0.55_f32, FRAC_PI_4, 1.02] {
            let d = vec2(a.cos(), -a.sin());
            c.hairline(
                &[d * 1.6, d * (2.2 + 2.4 * spark)],
                1.1,
                accent.gamma_multiply(spark),
            );
        }
    }
    c.dot(Vec2::ZERO, 1.2, accent);
}

/// A tool over a layer it cannot draw on: a padlock whose shackle drops shut
/// on arrival; a click rattles it.
fn locked(c: &mut Canvas, m: &Motion) {
    small_arrow(c);
    let rattle = 1.8 * wobble(m.pressed, 38.0, 7.0);
    let open = (1.0 - ease_back(m.enter / 0.3)) * -2.6;
    let fill = c.pal.fill;
    let accent = c.pal.accent;
    let outline = c.pal.outline;
    c.with(Xf::translate(v(10.6 + rattle, 12.4)), |c| {
        let mut shackle = vec![v(-2.4, 0.6 + open)];
        shackle.extend(arc(v(0.0, -2.2 + open), 2.4, 2.4, PI, TAU, 12));
        shackle.push(v(2.4, 0.6 + open));
        c.line(&shackle, 1.4, fill);
        c.body(
            &[v(-3.8, 0.0), v(3.8, 0.0), v(3.8, 6.2), v(-3.8, 6.2)],
            accent,
        );
        c.spot(v(0.0, 2.4), 0.95, outline);
        c.hairline(&[v(0.0, 2.6), v(0.0, 4.4)], 0.9, outline);
    });
}

// ---------------------------------------------------------------------------
// Each frame

/// Pointer state kept between frames.
#[derive(Debug)]
pub(crate) struct CursorRuntime {
    shown: Option<CursorKind>,
    shown_since: f64,
    pressed_at: f64,
    released_at: f64,
    held: bool,
    last: Option<(Pos2, f64)>,
    velocity: Vec2,
    lean: Vec2,
    lean_speed: Vec2,
    travel: f32,
    sweep: f32,
    wrap: Wrap,
    /// The end-of-frame hook that keeps the system cursor hidden is in.
    hooked: bool,
}

impl Default for CursorRuntime {
    fn default() -> Self {
        Self {
            shown: None,
            shown_since: 0.0,
            pressed_at: f64::NEG_INFINITY,
            released_at: f64::NEG_INFINITY,
            held: false,
            last: None,
            velocity: Vec2::ZERO,
            lean: Vec2::ZERO,
            lean_speed: Vec2::ZERO,
            travel: 0.0,
            sweep: 0.0,
            wrap: Wrap::default(),
            hooked: false,
        }
    }
}

impl CursorRuntime {
    /// Before egui reads this frame's input: keep a wrapped number drag's
    /// pointer continuous.
    pub(crate) fn rewrite_input(&mut self, raw: &mut egui::RawInput) {
        self.wrap.rewrite(raw);
    }

    /// After every widget has had its say: wrap number drags, then draw the
    /// pointer the frame ended on.
    pub(crate) fn finish_frame(
        &mut self,
        ctx: &Context,
        prefs: &CursorSettings,
        theme: &crate::settings::Theme,
    ) {
        if !self.hooked {
            // egui's drag and drop sets its own cursor after the frame is
            // built. This runs after it and hides the system cursor again
            // while one is drawn.
            self.hooked = true;
            ctx.on_end_frame(
                "q0_cursor",
                std::sync::Arc::new(|ctx: &Context| {
                    if ctx.data(|d| d.get_temp::<bool>(Id::new(HIDE_ID)).unwrap_or(false)) {
                        ctx.set_cursor_icon(CursorIcon::None);
                    }
                }),
            );
        }
        ctx.data_mut(|data| data.insert_temp(Id::new(HIDE_ID), false));
        let request = ctx.data_mut(|data| {
            let id = Id::new(REQUEST_ID);
            let request = data.get_temp::<Request>(id);
            data.remove::<Request>(id);
            request
        });
        let now = ctx.input(|i| i.time);
        let (hover, primary_down) =
            ctx.input(|i| (i.pointer.hover_pos(), i.pointer.primary_down()));

        // Number fields keep their full-precision value in egui's memory
        // while they are dragged. That is how a drag is told to be one.
        let dragged = ctx.dragged_id();
        if !primary_down || dragged.is_none() {
            if !primary_down {
                // A release the input rewrite missed (focus lost mid-drag).
                self.wrap.shift = Vec2::ZERO;
                self.wrap.pending = None;
            }
            self.wrap.drag = None;
        } else if dragged != self.wrap.drag {
            self.wrap.drag = dragged.filter(|&id| ctx.data(|d| d.get_temp::<f64>(id)).is_some());
        }
        let scrubbing = self.wrap.drag.is_some();
        let real = hover.map(|p| p - self.wrap.applied);
        if prefs.wrap_number_drags && scrubbing && primary_down {
            if let Some(real) = real {
                self.wrap.wrap_at_edges(ctx, real);
            }
        }

        if primary_down && !self.held {
            self.pressed_at = now;
            self.held = true;
            self.travel = 0.0;
            self.sweep = 0.0;
        } else if !primary_down && self.held {
            self.released_at = now;
            self.held = false;
        }

        let icon = ctx.output(|o| o.cursor_icon);
        let mut kind = match request {
            Some(request) if request.icon == icon => Some(request.kind),
            // egui is about to show its grabbing hand for a drag and drop.
            _ if egui::DragAndDrop::has_any_payload(ctx) => Some(CursorKind::Grabbing),
            _ => CursorKind::from_icon(icon),
        };
        if scrubbing {
            kind = Some(CursorKind::Scrub);
        }
        let kind = kind.filter(|&kind| prefs.shows(kind) && !crate::touch::ENABLED);
        let (Some(kind), Some(at)) = (kind, real) else {
            self.shown = None;
            self.last = None;
            return;
        };
        if self.shown != Some(kind) {
            self.shown = Some(kind);
            self.shown_since = now;
        }
        // Velocity from the position egui saw, which stays continuous
        // through a wrap.
        let seen = hover.unwrap_or(at);
        self.follow(seen, now);

        let motion = if prefs.animated {
            Motion {
                enter: (now - self.shown_since) as f32,
                pressed: ((now - self.pressed_at) as f32).min(100.0),
                held: self.held,
                release: if self.held {
                    100.0
                } else {
                    ((now - self.released_at) as f32).min(100.0)
                },
                lean: self.lean,
                travel: self.travel,
                sweep: self.sweep,
                sample: request.and_then(|r| r.sample),
            }
        } else {
            Motion {
                sample: request.and_then(|r| r.sample),
                ..Motion::REST
            }
        };

        ctx.set_cursor_icon(CursorIcon::None);
        ctx.data_mut(|data| data.insert_temp(Id::new(HIDE_ID), true));
        let mut shapes = Vec::new();
        paint(
            &mut shapes,
            kind,
            at,
            prefs.scale(),
            Palette::from_theme(theme),
            &motion,
        );
        ctx.layer_painter(LayerId::new(Order::Debug, Id::new(LAYER_ID)))
            .extend(shapes);

        let settling = motion.enter < kind.entry_secs()
            || motion.pressed < 1.3
            || motion.release < 1.3
            || self.velocity.length() > 2.0
            || self.lean.length() > 2.0
            || self.lean_speed.length() > 6.0;
        if prefs.animated && settling {
            ctx.request_repaint();
        }
    }

    /// Track the pointer's velocity and the spring that follows it.
    fn follow(&mut self, pos: Pos2, now: f64) {
        let Some((last, then)) = self.last.replace((pos, now)) else {
            return;
        };
        let dt = ((now - then) as f32).clamp(0.0, 0.25);
        if dt <= 0.0 {
            return;
        }
        let moved = pos - last;
        if self.held {
            self.travel = (self.travel + moved.length()) % 10_000.0;
            self.sweep = (self.sweep + moved.x) % 10_000.0;
        }
        let instant = moved / dt.max(1.0 / 240.0);
        self.velocity = lerp(self.velocity, instant, (dt * 20.0).min(1.0));
        if self.velocity.length() < 0.5 {
            self.velocity = Vec2::ZERO;
        }
        // A spring a little under critical damping: it catches up, swings
        // past once, and settles.
        const STIFFNESS: f32 = 150.0;
        let damping = 2.0 * STIFFNESS.sqrt() * 0.6;
        let steps = (dt * 240.0).ceil().max(1.0) as usize;
        let h = dt / steps as f32;
        for _ in 0..steps {
            let force = (self.velocity - self.lean) * STIFFNESS - self.lean_speed * damping;
            self.lean_speed += force * h;
            self.lean += self.lean_speed * h;
        }
        if self.lean.length() < 0.5 && self.lean_speed.length() < 1.0 && self.velocity == Vec2::ZERO
        {
            self.lean = Vec2::ZERO;
            self.lean_speed = Vec2::ZERO;
        }
    }
}

// ---------------------------------------------------------------------------
// Wrapping number drags

/// A jump of the system cursor that the input has not shown yet.
#[derive(Clone, Copy, Debug)]
struct Jump {
    from: Pos2,
    to: Pos2,
    shift_before: Vec2,
}

/// The offset between where the mouse really is and where egui is told it
/// is, while a number drag wraps around the window.
#[derive(Debug, Default)]
struct Wrap {
    /// The number field being dragged.
    drag: Option<Id>,
    /// Added to the real position before egui sees it.
    shift: Vec2,
    /// The shift on the newest position egui saw.
    applied: Vec2,
    pending: Option<Jump>,
}

impl Wrap {
    /// Moves recorded before the jump still carry the old shift; the first
    /// one near where the cursor was sent switches to the new one.
    fn shift_for(&mut self, pos: Pos2) -> Vec2 {
        if let Some(jump) = self.pending {
            if pos.distance(jump.to) < pos.distance(jump.from) {
                self.pending = None;
            } else {
                return jump.shift_before;
            }
        }
        self.shift
    }

    fn rewrite(&mut self, raw: &mut egui::RawInput) {
        let mut released = false;
        for event in &mut raw.events {
            match event {
                egui::Event::PointerMoved(pos) => {
                    let shift = self.shift_for(*pos);
                    *pos += shift;
                    self.applied = shift;
                }
                egui::Event::PointerButton {
                    pos,
                    button,
                    pressed,
                    ..
                } => {
                    let shift = self.shift_for(*pos);
                    *pos += shift;
                    self.applied = shift;
                    released |= *button == egui::PointerButton::Primary && !*pressed;
                }
                _ => {}
            }
        }
        if released {
            // egui sees the release where the drag got to; the next move
            // brings its pointer back to the real one.
            let drag = self.drag;
            *self = Self {
                drag,
                applied: self.applied,
                ..Self::default()
            };
        }
    }

    /// Send the system cursor to the opposite edge when it reaches one.
    fn wrap_at_edges(&mut self, ctx: &Context, real: Pos2) {
        if self.pending.is_some() {
            return;
        }
        const INSET: f32 = 3.0;
        let area = ctx.screen_rect().shrink(INSET);
        if area.width() < 40.0 || area.height() < 40.0 || area.contains(real) {
            return;
        }
        let wrap = |p: f32, min: f32, max: f32| {
            if p < min || p > max {
                min + 1.0 + (p - min - 1.0).rem_euclid(max - min - 2.0)
            } else {
                p
            }
        };
        let to = pos2(
            wrap(real.x, area.left(), area.right()),
            wrap(real.y, area.top(), area.bottom()),
        );
        if to.distance(real) < 1.0 || !move_system_cursor(ctx, to) {
            return;
        }
        self.pending = Some(Jump {
            from: real,
            to,
            shift_before: self.shift,
        });
        self.shift += real - to;
    }
}

/// Put the system cursor at `to`, in points from the window's corner.
#[cfg(windows)]
fn move_system_cursor(ctx: &Context, to: Pos2) -> bool {
    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::Graphics::Gdi::ClientToScreen;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetActiveWindow;
    use windows_sys::Win32::UI::WindowsAndMessaging::SetCursorPos;

    let scale = ctx.pixels_per_point();
    let mut point = POINT {
        x: (to.x * scale).round() as i32,
        y: (to.y * scale).round() as i32,
    };
    // SAFETY: plain Win32 calls on this thread's own active window, with a
    // POINT that lives on the stack for the duration of the call.
    unsafe {
        let window = GetActiveWindow();
        if window.is_null() || ClientToScreen(window, &mut point) == 0 {
            return false;
        }
        SetCursorPos(point.x, point.y) != 0
    }
}

#[cfg(not(windows))]
fn move_system_cursor(_ctx: &Context, _to: Pos2) -> bool {
    false
}

// ---------------------------------------------------------------------------
// Settings previews

/// A small stage playing `kind` on a loop. Hovering it shows the real pointer
/// for that state.
pub(crate) fn preview(
    ui: &mut Ui,
    kind: CursorKind,
    prefs: &CursorSettings,
    theme: &crate::settings::Theme,
) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(58.0, 40.0), Sense::hover());
    if !ui.is_rect_visible(rect) {
        return response;
    }
    let visuals = ui.visuals();
    let back = if response.hovered() {
        visuals.widgets.hovered.weak_bg_fill
    } else {
        visuals.extreme_bg_color
    };
    ui.painter()
        .rect(rect, 6.0, back, visuals.widgets.noninteractive.bg_stroke);
    let on = prefs.shows(kind);
    let pal = Palette::from_theme(theme);
    let pal = if on { pal } else { pal.faded(0.35) };
    let time = ui.input(|i| i.time);
    let motion = if prefs.animated && on {
        Motion::demo(time)
    } else {
        Motion::REST
    };
    let scale = 1.05;
    let at = rect.center() - kind.preview_center() * scale;
    let mut shapes = Vec::new();
    paint(&mut shapes, kind, at, scale, pal, &motion);
    ui.painter().with_clip_rect(rect.shrink(1.0)).extend(shapes);
    if prefs.animated && on {
        ui.ctx().request_repaint();
    }
    if response.hovered() {
        request(ui.ctx(), kind);
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::Rect;

    fn shapes_for(kind: CursorKind, rect: Rect) -> Vec<Shape> {
        let mut shapes = Vec::new();
        let pal = Palette::from_theme(&crate::settings::Theme::default());
        paint(
            &mut shapes,
            kind,
            rect.center(),
            1.0,
            pal,
            &Motion::demo(1.3),
        );
        shapes
    }

    const ALL: [CursorKind; 36] = [
        CursorKind::Arrow,
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
        CursorKind::LibraryDrop,
        CursorKind::PenClose,
        CursorKind::ScreenPick,
        CursorKind::FrameDrag,
        CursorKind::ParentLink,
        CursorKind::KnifeCut,
        CursorKind::SelectAdd,
        CursorKind::SelectRemove,
        CursorKind::CopyDrag,
        CursorKind::Pivot,
        CursorKind::Snap,
        CursorKind::Locked,
    ];

    fn area(points: &[Pos2], triangles: &[[u32; 3]]) -> f32 {
        triangles
            .iter()
            .map(|&[a, b, c]| {
                let (a, b, c) = (points[a as usize], points[b as usize], points[c as usize]);
                ((b - a).x * (c - a).y - (b - a).y * (c - a).x).abs() * 0.5
            })
            .sum()
    }

    #[test]
    fn concave_outlines_are_filled_exactly() {
        for outline in [&ARROW[..], &HAND[..]] {
            let points: Vec<Pos2> = outline.iter().map(|p| p.to_pos2()).collect();
            let n = points.len();
            let exact = (0..n)
                .map(|i| {
                    let (p, q) = (points[i], points[(i + 1) % n]);
                    p.x * q.y - q.x * p.y
                })
                .sum::<f32>()
                .abs()
                * 0.5;
            let triangles = triangulate(&points);
            assert_eq!(triangles.len(), n - 2);
            assert!((area(&points, &triangles) - exact).abs() < 1.0e-3);
            // Reversed winding fills the same.
            let reversed: Vec<Pos2> = points.iter().rev().copied().collect();
            assert!((area(&reversed, &triangulate(&reversed)) - exact).abs() < 1.0e-3);
        }
    }

    #[test]
    fn every_pointer_draws_finite_shapes_near_its_hotspot() {
        let rect = Rect::from_center_size(pos2(100.0, 100.0), vec2(80.0, 80.0));
        for kind in ALL {
            let shapes = shapes_for(kind, rect);
            assert!(!shapes.is_empty(), "{kind:?} drew nothing");
            for shape in &shapes {
                let bounds = shape.visual_bounding_rect();
                assert!(
                    !bounds.is_positive() || rect.expand(10.0).contains_rect(bounds),
                    "{kind:?} drew outside its box: {bounds:?}"
                );
            }
        }
    }

    #[test]
    fn the_screen_pick_loupe_leaves_the_sampled_pixel_alone() {
        let at = pos2(100.0, 100.0);
        let pal = Palette::from_theme(&crate::settings::Theme::default());
        for time in [0.05, 0.3, 1.3, 2.0] {
            let mut shapes = Vec::new();
            paint(
                &mut shapes,
                CursorKind::ScreenPick,
                at,
                1.75,
                pal,
                &Motion::demo(time),
            );
            for shape in &shapes {
                let bounds = shape.visual_bounding_rect();
                if let Shape::Path(path) = shape {
                    for p in &path.points {
                        assert!(
                            p.distance(at) > 4.0,
                            "a point of the loupe sits on the pixel it reads"
                        );
                    }
                } else if bounds.is_positive() {
                    assert!(!bounds.shrink(0.5).contains(at) || matches!(shape, Shape::Path(_)));
                }
            }
        }
    }

    #[test]
    fn every_state_has_a_switch_and_every_variant_a_state() {
        let keys: BTreeSet<&str> = CursorKind::STATES
            .iter()
            .chain(&CursorKind::ACTIONS)
            .map(|k| k.key())
            .collect();
        assert_eq!(
            keys.len(),
            CursorKind::STATES.len() + CursorKind::ACTIONS.len(),
            "two switches share a key"
        );
        for kind in ALL {
            assert!(keys.contains(kind.key()), "{kind:?} has no switch");
        }
    }

    #[test]
    fn system_cursors_map_back_to_their_pointer() {
        for kind in [
            CursorKind::Arrow,
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
        ] {
            assert_eq!(CursorKind::from_icon(kind.system()), Some(kind));
        }
        assert_eq!(CursorKind::from_icon(CursorIcon::None), None);
    }

    #[test]
    fn switched_off_states_and_the_main_switch_fall_back_to_the_system() {
        let mut prefs = CursorSettings::default();
        assert!(prefs.shows(CursorKind::ResizeNeSw));
        prefs.set_state(CursorKind::ResizeNwSe, false);
        assert!(!prefs.shows(CursorKind::ResizeNeSw));
        assert!(prefs.shows(CursorKind::Arrow));
        prefs.custom = false;
        assert!(!prefs.shows(CursorKind::Arrow));
        assert!(prefs.state_on(CursorKind::Arrow));
        let old: CursorSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(old, CursorSettings::default());
    }

    #[test]
    fn a_wrap_keeps_the_drag_continuous_until_the_button_comes_up() {
        let mut wrap = Wrap {
            drag: Some(Id::new("number")),
            ..Wrap::default()
        };
        // The cursor reached the right edge at x 798 and was sent to x 4.
        wrap.pending = Some(Jump {
            from: pos2(798.0, 50.0),
            to: pos2(4.0, 50.0),
            shift_before: Vec2::ZERO,
        });
        wrap.shift = vec2(794.0, 0.0);
        let mut raw = egui::RawInput {
            events: vec![
                // Recorded before the jump: no shift yet.
                egui::Event::PointerMoved(pos2(799.0, 50.0)),
                // After it: carried on past the old edge.
                egui::Event::PointerMoved(pos2(6.0, 50.0)),
            ],
            ..Default::default()
        };
        wrap.rewrite(&mut raw);
        let seen: Vec<Pos2> = raw
            .events
            .iter()
            .filter_map(|e| match e {
                egui::Event::PointerMoved(p) => Some(*p),
                _ => None,
            })
            .collect();
        assert_eq!(seen, vec![pos2(799.0, 50.0), pos2(800.0, 50.0)]);
        assert!(wrap.pending.is_none());
        let mut raw = egui::RawInput {
            events: vec![egui::Event::PointerButton {
                pos: pos2(10.0, 50.0),
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        };
        wrap.rewrite(&mut raw);
        assert!(matches!(
            raw.events[0],
            egui::Event::PointerButton { pos, .. } if pos == pos2(804.0, 50.0)
        ));
        assert_eq!(wrap.shift, Vec2::ZERO);
    }
}
