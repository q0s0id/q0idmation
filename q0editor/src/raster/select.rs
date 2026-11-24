//! Raster selection: rectangle and lasso, and moving, sizing and turning
//! the pixels they take.
//!
//! A selection is a list of shapes in the drawing's pixels, each adding or
//! taking away. Dragging inside it lifts the pixels off the drawing into a
//! floating piece (one undo step from here to letting go), which follows
//! the handles until it is let go: Enter, Escape, another tool, another
//! frame or layer, or a click outside.

use egui::{
    vec2, Color32, Context, Key, Modifiers, Painter, PointerButton, Pos2, Rect, Response, Shape,
    Stroke,
};
use q0s_format::pixels::{bilinear, PixelRect, RasterAsset};
use q0s_format::transform::Affine;
use q0s_format::v2::Vec2;

use crate::app::EditorApp;
use crate::cursors::CursorKind;
use crate::render::StageView;

use super::{CanvasRef, RasterTool};

/// The id the floating piece is drawn under; no project asset has it.
const FLOATING_ID: u16 = u16::MAX;
/// Handle size on screen.
const HANDLE: f32 = 7.0;

/// One shape of a selection, in drawing pixels.
#[derive(Clone, Debug, PartialEq)]
struct Outline {
    points: Vec<(f32, f32)>,
    add: bool,
}

/// What is selected, on which drawing.
#[derive(Clone, Debug)]
struct Selection {
    canvas: CanvasRef,
    frame: u16,
    shapes: Vec<Outline>,
}

/// Pixels lifted off the drawing.
struct Floating {
    canvas: CanvasRef,
    frame: u16,
    /// The pixels as they were, their top left at (0, 0).
    piece: RasterAsset,
    size: (i32, i32),
    /// The piece's own pixels to the drawing's.
    transform: Affine,
    /// What the history looked like after the lift: if it changes (an undo),
    /// the piece is forgotten rather than put back.
    history_mark: u64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Handle {
    Corner(usize),
    Turn(usize),
    Inside,
}

enum Drag {
    Marquee {
        from: (f32, f32),
        to: (f32, f32),
        add: bool,
    },
    Lasso {
        points: Vec<(f32, f32)>,
        add: bool,
    },
    Transform {
        handle: Handle,
        start: Affine,
        /// Pointer at the press, in drawing pixels.
        grab: (f32, f32),
    },
}

/// The selection tools' state.
#[derive(Default)]
pub struct SelectionState {
    selection: Option<Selection>,
    floating: Option<Floating>,
    drag: Option<Drag>,
}

impl SelectionState {
    pub fn is_empty(&self) -> bool {
        self.selection.is_none() && self.floating.is_none()
    }
}

/// Whether pixels are lifted and waiting to be let go.
pub fn floating(app: &EditorApp) -> bool {
    app.session.raster.selection.floating.is_some()
}

fn polygon_contains(points: &[(f32, f32)], x: f32, y: f32) -> bool {
    let mut inside = false;
    let mut j = points.len().wrapping_sub(1);
    for i in 0..points.len() {
        let (xi, yi) = points[i];
        let (xj, yj) = points[j];
        if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
            inside = !inside;
        }
        j = i;
    }
    inside
}

impl Selection {
    fn bounds(&self) -> Option<PixelRect> {
        let mut rect: Option<PixelRect> = None;
        for shape in self.shapes.iter().filter(|shape| shape.add) {
            for &(x, y) in &shape.points {
                let p = PixelRect::new(
                    x.floor() as i32,
                    y.floor() as i32,
                    x.ceil() as i32 + 1,
                    y.ceil() as i32 + 1,
                );
                rect = Some(rect.map_or(p, |r| r.union(p)));
            }
        }
        rect
    }

    fn contains(&self, x: f32, y: f32) -> bool {
        let mut inside = false;
        for shape in &self.shapes {
            if polygon_contains(&shape.points, x, y) {
                inside = shape.add;
            }
        }
        inside
    }

    /// Which pixels of `rect` are selected (by their centres).
    fn mask(&self, rect: PixelRect) -> Vec<bool> {
        let mut mask = Vec::with_capacity((rect.width() * rect.height()) as usize);
        for y in rect.y0..rect.y1 {
            for x in rect.x0..rect.x1 {
                mask.push(self.contains(x as f32 + 0.5, y as f32 + 0.5));
            }
        }
        mask
    }
}

/// The piece's corners in drawing pixels, clockwise from the top left.
fn piece_corners(floating: &Floating) -> [Vec2; 4] {
    let (w, h) = (floating.size.0 as f32, floating.size.1 as f32);
    [(0.0, 0.0), (w, 0.0), (w, h), (0.0, h)].map(|(x, y)| floating.transform.apply(Vec2::new(x, y)))
}

fn to_screen(canvas: &CanvasRef, view: &StageView, x: f32, y: f32) -> Pos2 {
    let stage = canvas
        .to_local
        .inverse()
        .map_or(Vec2::new(x, y), |world| world.apply(Vec2::new(x, y)));
    view.stage_to_screen(stage)
}

/// The handle under `pointer`, if any.
fn handle_at(floating: &Floating, view: &StageView, pointer: Pos2) -> Option<Handle> {
    let corners = piece_corners(floating).map(|c| to_screen(&floating.canvas, view, c.x, c.y));
    for (i, corner) in corners.iter().enumerate() {
        if corner.distance(pointer) <= HANDLE + 3.0 {
            return Some(Handle::Corner(i));
        }
    }
    let centre = corners
        .iter()
        .fold(Pos2::ZERO, |a, c| a + c.to_vec2() / 4.0);
    for (i, corner) in corners.iter().enumerate() {
        let out = (*corner - centre).normalized() * 16.0;
        if (*corner + out).distance(pointer) <= 14.0 {
            return Some(Handle::Turn(i));
        }
    }
    let local = floating.transform.inverse().map(|inverse| {
        let d = floating.canvas.local(view.screen_to_stage(pointer));
        inverse.apply(Vec2::new(d.0, d.1))
    })?;
    (local.x >= 0.0
        && local.y >= 0.0
        && local.x <= floating.size.0 as f32
        && local.y <= floating.size.1 as f32)
        .then_some(Handle::Inside)
}

/// The pointer the selection tools show at `pointer`.
pub(crate) fn pointer_kind(
    app: &EditorApp,
    view: &StageView,
    pointer: Pos2,
    modifiers: Modifiers,
    lasso: bool,
) -> CursorKind {
    let state = &app.session.raster.selection;
    if let Some(floating) = &state.floating {
        match handle_at(floating, view, pointer) {
            Some(Handle::Corner(i)) => {
                return if i % 2 == 0 {
                    CursorKind::ResizeNwSe
                } else {
                    CursorKind::ResizeNeSw
                };
            }
            Some(Handle::Turn(_)) => return CursorKind::Rotate,
            Some(Handle::Inside) => return CursorKind::Move,
            None => {}
        }
    }
    if let Some(selection) = &state.selection {
        let (x, y) = selection.canvas.local(view.screen_to_stage(pointer));
        if selection.contains(x, y) {
            return CursorKind::Move;
        }
    }
    if modifiers.shift {
        CursorKind::SelectAdd
    } else if modifiers.alt {
        CursorKind::SelectRemove
    } else if lasso {
        CursorKind::Lasso
    } else {
        CursorKind::Crosshair
    }
}

/// Lifts the selected pixels into a floating piece. Takes the undo snapshot.
fn lift(app: &mut EditorApp) -> bool {
    let Some(selection) = app.session.raster.selection.selection.clone() else {
        return false;
    };
    let Some(bounds) = selection.bounds() else {
        return false;
    };
    let Some(drawing) = super::raster_ref(&app.state.project, selection.canvas.asset_id) else {
        return false;
    };
    let mask = selection.mask(bounds);
    let mut pixels = drawing.compose(bounds);
    let mut left = pixels.clone();
    for (i, selected) in mask.iter().enumerate() {
        if *selected {
            left[i * 4..i * 4 + 4].fill(0);
        } else {
            pixels[i * 4..i * 4 + 4].fill(0);
        }
    }
    if pixels.as_chunks::<4>().0.iter().all(|p| p[3] == 0) {
        app.session.status = "nothing in the selection to move".to_string();
        return false;
    }
    app.history
        .snapshot_with_session(&app.state.project, &app.session);
    if let Some(drawing) = super::raster_mut(&mut app.state.project, selection.canvas.asset_id) {
        drawing.put(bounds, &left);
    }
    app.state.mark_dirty();
    let size = (bounds.width(), bounds.height());
    let piece =
        RasterAsset::from_pixels(FLOATING_ID, PixelRect::new(0, 0, size.0, size.1), &pixels);
    app.session.raster.selection.floating = Some(Floating {
        canvas: selection.canvas,
        frame: selection.frame,
        piece,
        size,
        transform: Affine {
            tx: bounds.x0 as f32,
            ty: bounds.y0 as f32,
            ..Affine::IDENTITY
        },
        history_mark: app.history.snapshot_revision(),
    });
    true
}

/// Puts a floating piece down and clears the selection.
pub fn let_go(app: &mut EditorApp) {
    let state = std::mem::take(&mut app.session.raster.selection);
    let Some(floating) = state.floating else {
        return;
    };
    if app.history.snapshot_revision() != floating.history_mark {
        return;
    }
    if let Some(drawing) = super::raster_mut(&mut app.state.project, floating.canvas.asset_id) {
        place(drawing, &floating);
        app.state.mark_dirty();
        app.session.status = "pixels placed".to_string();
    }
}

/// Composites the floating piece into `drawing` through its transform.
fn place(drawing: &mut RasterAsset, floating: &Floating) {
    let t = floating.transform;
    let (w, h) = floating.size;
    let source = floating.piece.compose(PixelRect::new(0, 0, w, h));
    let exact = (t.a11 - 1.0).abs() < 1e-4
        && (t.a22 - 1.0).abs() < 1e-4
        && t.a12.abs() < 1e-4
        && t.a21.abs() < 1e-4;
    let corners = piece_corners(floating);
    let min_x = corners
        .iter()
        .map(|c| c.x)
        .fold(f32::INFINITY, f32::min)
        .floor() as i32;
    let min_y = corners
        .iter()
        .map(|c| c.y)
        .fold(f32::INFINITY, f32::min)
        .floor() as i32;
    let max_x = corners
        .iter()
        .map(|c| c.x)
        .fold(f32::NEG_INFINITY, f32::max)
        .ceil() as i32;
    let max_y = corners
        .iter()
        .map(|c| c.y)
        .fold(f32::NEG_INFINITY, f32::max)
        .ceil() as i32;
    let rect = PixelRect::new(min_x, min_y, max_x + 1, max_y + 1);
    let Some(inverse) = t.inverse() else {
        return;
    };
    let mut out = drawing.compose(rect);
    let width = rect.width();
    // Shrinking a lot: from a smaller copy, so it stays smooth.
    let shrink = t.uniform_scale();
    let (mut pixels, mut pw, mut ph, mut step) = (source, w as usize, h as usize, 1.0_f32);
    while shrink * step < 0.5 && pw > 1 && ph > 1 && !exact {
        let (next, nw, nh) = q0s_format::pixels::halve(&pixels, pw, ph);
        pixels = next;
        pw = nw;
        ph = nh;
        step *= 2.0;
    }
    for y in rect.y0..rect.y1 {
        for x in rect.x0..rect.x1 {
            let local = inverse.apply(Vec2::new(x as f32 + 0.5, y as f32 + 0.5));
            let src = if exact {
                let (sx, sy) = ((local.x).floor() as i64, (local.y).floor() as i64);
                if sx < 0 || sy < 0 || sx >= pw as i64 || sy >= ph as i64 {
                    continue;
                }
                let i = (sy as usize * pw + sx as usize) * 4;
                [pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3]]
            } else {
                bilinear(&pixels, pw, ph, local.x / step - 0.5, local.y / step - 0.5)
            };
            if src[3] == 0 {
                continue;
            }
            let i = (((y - rect.y0) * width + (x - rect.x0)) * 4) as usize;
            let keep = 255 - u32::from(src[3]);
            for c in 0..4 {
                out[i + c] =
                    (u32::from(src[c]) + (u32::from(out[i + c]) * keep + 127) / 255).min(255) as u8;
            }
        }
    }
    drawing.put(rect, &out);
}

/// Pixels copied from a raster selection.
#[derive(Clone)]
pub struct Clip {
    piece: RasterAsset,
    size: (i32, i32),
    /// Where they sat in the drawing they came from.
    transform: Affine,
}

/// The editor's Copy and Cut while there is a raster selection. True when
/// it was one.
pub(crate) fn route_copy(app: &mut EditorApp, cut: bool) -> bool {
    if !app.session.raster.active() || app.session.raster.selection.is_empty() {
        app.session.raster.clip_newest = false;
        return false;
    }
    let clip = if let Some(floating) = &app.session.raster.selection.floating {
        Some(Clip {
            piece: floating.piece.clone(),
            size: floating.size,
            transform: floating.transform,
        })
    } else {
        app.session
            .raster
            .selection
            .selection
            .clone()
            .and_then(|selection| {
                let bounds = selection.bounds()?;
                let drawing = super::raster_ref(&app.state.project, selection.canvas.asset_id)?;
                let mask = selection.mask(bounds);
                let mut pixels = drawing.compose(bounds);
                for (i, selected) in mask.iter().enumerate() {
                    if !selected {
                        pixels[i * 4..i * 4 + 4].fill(0);
                    }
                }
                let size = (bounds.width(), bounds.height());
                Some(Clip {
                    piece: RasterAsset::from_pixels(
                        FLOATING_ID,
                        PixelRect::new(0, 0, size.0, size.1),
                        &pixels,
                    ),
                    size,
                    transform: Affine {
                        tx: bounds.x0 as f32,
                        ty: bounds.y0 as f32,
                        ..Affine::IDENTITY
                    },
                })
            })
    };
    let Some(clip) = clip.filter(|clip| !clip.piece.is_empty()) else {
        app.session.status = "nothing in the selection to copy".to_string();
        return true;
    };
    app.session.raster.clipboard = Some(clip);
    app.session.raster.clip_newest = true;
    if cut {
        delete(app);
        app.session.status = "pixels cut".to_string();
    } else {
        app.session.status = "pixels copied".to_string();
    }
    true
}

/// The editor's Paste, when the newest copy is raster pixels and the
/// current layer is a raster layer: they come in where they were, floating,
/// ready to move.
pub(crate) fn route_paste(app: &mut EditorApp) -> bool {
    if !app.session.raster.clip_newest
        || !super::is_raster_layer(
            &app.state.project,
            app.session.current_q0rg_id,
            app.session.current_layer_id,
        )
    {
        return false;
    }
    let Some(clip) = app.session.raster.clipboard.clone() else {
        return false;
    };
    let_go(app);
    if let Err(why) = super::check_layer(app) {
        app.session.status = why.message().to_string();
        return true;
    }
    app.history
        .snapshot_with_session(&app.state.project, &app.session);
    let canvas = match super::prepare_canvas(app) {
        Ok(canvas) => canvas,
        Err(why) => {
            app.session.status = why.message().to_string();
            return true;
        }
    };
    app.state.mark_dirty();
    if !matches!(
        app.session.raster.tool,
        Some(RasterTool::Marquee | RasterTool::Lasso)
    ) {
        super::pick_tool(app, RasterTool::Marquee);
    }
    app.session.raster.selection.floating = Some(Floating {
        canvas,
        frame: app.session.current_frame,
        piece: clip.piece,
        size: clip.size,
        transform: clip.transform,
        history_mark: app.history.snapshot_revision(),
    });
    app.session.status = "pixels pasted: drag to place them, Enter to put them down".to_string();
    true
}

/// The editor's Delete while there is a raster selection.
pub(crate) fn route_delete(app: &mut EditorApp) -> bool {
    if !app.session.raster.active() || app.session.raster.selection.is_empty() {
        return false;
    }
    delete(app);
    true
}

/// The editor's Select All while a raster selection tool is picked.
pub(crate) fn route_select_all(app: &mut EditorApp) -> bool {
    if !matches!(
        app.session.raster.tool,
        Some(RasterTool::Marquee | RasterTool::Lasso)
    ) {
        return false;
    }
    select_all(app);
    true
}

/// Clears the selected pixels, or drops a floating piece.
pub(crate) fn delete(app: &mut EditorApp) {
    if app.session.raster.selection.floating.take().is_some() {
        app.session.raster.selection = SelectionState::default();
        app.state.mark_dirty();
        app.session.status = "pixels deleted".to_string();
        return;
    }
    let Some(selection) = app.session.raster.selection.selection.take() else {
        return;
    };
    let Some(bounds) = selection.bounds() else {
        return;
    };
    app.history
        .snapshot_with_session(&app.state.project, &app.session);
    if let Some(drawing) = super::raster_mut(&mut app.state.project, selection.canvas.asset_id) {
        let mask = selection.mask(bounds);
        let mut pixels = drawing.compose(bounds);
        for (i, selected) in mask.iter().enumerate() {
            if *selected {
                pixels[i * 4..i * 4 + 4].fill(0);
            }
        }
        drawing.put(bounds, &pixels);
    }
    app.state.mark_dirty();
    app.session.status = "selection cleared".to_string();
}

/// Stage input for the selection tools.
pub fn handle(
    app: &mut EditorApp,
    response: &Response,
    view: &StageView,
    ctx: &Context,
    tool: RasterTool,
) {
    // An undo while pixels float takes them back where they were.
    if let Some(floating) = &app.session.raster.selection.floating {
        if app.history.snapshot_revision() != floating.history_mark {
            app.session.raster.selection = SelectionState::default();
        }
    }
    // Another frame or layer lets go.
    let here = (
        app.session.current_q0rg_id,
        app.session.current_layer_id,
        app.session.current_frame,
    );
    let elsewhere =
        |canvas: &CanvasRef, frame: u16| (canvas.q0rg_id, canvas.layer_id, frame) != here;
    let state = &app.session.raster.selection;
    let moved_away = state
        .floating
        .as_ref()
        .is_some_and(|f| elsewhere(&f.canvas, f.frame))
        || state
            .selection
            .as_ref()
            .is_some_and(|s| elsewhere(&s.canvas, s.frame));
    if moved_away {
        let_go(app);
    }
    // Delete, Select All, Copy, Cut and Paste come as the editor's own
    // commands (`route_*`), so their keys stay whatever the user set.
    if !ctx.wants_keyboard_input() {
        let (enter, escape) = ctx.input(|input| {
            (
                input.key_pressed(Key::Enter),
                input.key_pressed(Key::Escape),
            )
        });
        if enter || escape {
            let_go(app);
        }
    }
    let (shift, alt) = ctx.input(|input| (input.modifiers.shift, input.modifiers.alt));
    let pointer = response
        .interact_pointer_pos()
        .or_else(|| response.hover_pos());
    if response.drag_started_by(PointerButton::Primary) {
        let Some(press) = ctx.input(|input| input.pointer.press_origin()) else {
            return;
        };
        begin_drag(app, view, press, tool, shift, alt);
    }
    if response.dragged_by(PointerButton::Primary) {
        if let Some(pointer) = pointer {
            continue_drag(app, view, pointer, shift);
            ctx.request_repaint();
        }
    }
    if response.drag_stopped() {
        end_drag(app);
    }
    if response.clicked() {
        // A click outside lets go of everything.
        let inside = pointer.is_some_and(|pointer| {
            let state = &app.session.raster.selection;
            state
                .floating
                .as_ref()
                .is_some_and(|f| handle_at(f, view, pointer).is_some())
                || state.selection.as_ref().is_some_and(|s| {
                    let (x, y) = s.canvas.local(view.screen_to_stage(pointer));
                    s.contains(x, y)
                })
        });
        if !inside {
            let_go(app);
        }
    }
    if !app.session.raster.selection.is_empty() {
        // Marching ants march.
        ctx.request_repaint_after(std::time::Duration::from_millis(60));
    }
}

pub(crate) fn select_all(app: &mut EditorApp) {
    let_go(app);
    let Ok(Some(canvas)) = super::find_canvas(app) else {
        return;
    };
    let Some(bounds) =
        super::raster_ref(&app.state.project, canvas.asset_id).and_then(RasterAsset::exact_bounds)
    else {
        return;
    };
    let (x0, y0, x1, y1) = (
        bounds.x0 as f32,
        bounds.y0 as f32,
        bounds.x1 as f32,
        bounds.y1 as f32,
    );
    app.session.raster.selection.selection = Some(Selection {
        canvas,
        frame: app.session.current_frame,
        shapes: vec![Outline {
            points: vec![(x0, y0), (x1, y0), (x1, y1), (x0, y1)],
            add: true,
        }],
    });
}

fn begin_drag(
    app: &mut EditorApp,
    view: &StageView,
    press: Pos2,
    tool: RasterTool,
    shift: bool,
    alt: bool,
) {
    if let Some(floating) = &app.session.raster.selection.floating {
        if let Some(handle) = handle_at(floating, view, press) {
            let grab = floating.canvas.local(view.screen_to_stage(press));
            let start = floating.transform;
            app.session.raster.selection.drag = Some(Drag::Transform {
                handle,
                start,
                grab,
            });
            return;
        }
        let_go(app);
    }
    let canvas = match super::find_canvas(app) {
        Ok(Some(canvas)) => canvas,
        Ok(None) => {
            app.session.status = "nothing drawn on this frame to select".to_string();
            return;
        }
        Err(why) => {
            app.session.status = why.message().to_string();
            return;
        }
    };
    let at = canvas.local(view.screen_to_stage(press));
    // Inside the selection: lift and move it.
    let inside = !shift
        && !alt
        && app
            .session
            .raster
            .selection
            .selection
            .as_ref()
            .is_some_and(|s| s.canvas.asset_id == canvas.asset_id && s.contains(at.0, at.1));
    if inside {
        if app
            .state
            .project
            .layer_is_locked(canvas.q0rg_id, canvas.layer_id)
        {
            app.session.status = super::NoCanvas::Locked.message().to_string();
            return;
        }
        if lift(app) {
            let start = app
                .session
                .raster
                .selection
                .floating
                .as_ref()
                .map(|f| f.transform);
            if let Some(start) = start {
                app.session.raster.selection.drag = Some(Drag::Transform {
                    handle: Handle::Inside,
                    start,
                    grab: at,
                });
            }
        }
        return;
    }
    let fresh = !(shift || alt)
        || app
            .session
            .raster
            .selection
            .selection
            .as_ref()
            .is_none_or(|s| s.canvas.asset_id != canvas.asset_id);
    if fresh {
        app.session.raster.selection.selection = None;
    }
    let add = !alt;
    app.session.raster.selection.drag = Some(match tool {
        RasterTool::Lasso => Drag::Lasso {
            points: vec![at],
            add,
        },
        _ => Drag::Marquee {
            from: at,
            to: at,
            add,
        },
    });
    if app.session.raster.selection.selection.is_none() {
        app.session.raster.selection.selection = Some(Selection {
            canvas,
            frame: app.session.current_frame,
            shapes: Vec::new(),
        });
    }
}

fn continue_drag(app: &mut EditorApp, view: &StageView, pointer: Pos2, shift: bool) {
    let state = &mut app.session.raster.selection;
    let canvas = state
        .floating
        .as_ref()
        .map(|f| f.canvas)
        .or_else(|| state.selection.as_ref().map(|s| s.canvas));
    let Some(canvas) = canvas else {
        return;
    };
    let at = canvas.local(view.screen_to_stage(pointer));
    match state.drag.as_mut() {
        Some(Drag::Marquee { to, .. }) => *to = at,
        Some(Drag::Lasso { points, .. }) => {
            if points
                .last()
                .is_none_or(|last| (last.0 - at.0).hypot(last.1 - at.1) >= 1.0)
            {
                points.push(at);
            }
        }
        Some(Drag::Transform {
            handle,
            start,
            grab,
        }) => {
            let Some(floating) = state.floating.as_mut() else {
                return;
            };
            floating.transform = transformed(*handle, *start, *grab, at, floating.size, shift);
        }
        None => {}
    }
}

/// The piece's transform after dragging `handle` from `grab` to `at`.
fn transformed(
    handle: Handle,
    start: Affine,
    grab: (f32, f32),
    at: (f32, f32),
    size: (i32, i32),
    keep_aspect: bool,
) -> Affine {
    let (w, h) = (size.0 as f32, size.1 as f32);
    let corner_local = |i: usize| match i {
        0 => Vec2::new(0.0, 0.0),
        1 => Vec2::new(w, 0.0),
        2 => Vec2::new(w, h),
        _ => Vec2::new(0.0, h),
    };
    match handle {
        Handle::Inside => {
            // Whole pixels, so a moved drawing stays crisp.
            let dx = (at.0 - grab.0).round();
            let dy = (at.1 - grab.1).round();
            Affine {
                tx: start.tx + dx,
                ty: start.ty + dy,
                ..start
            }
        }
        Handle::Corner(i) => {
            // Size about the opposite corner, in the piece's own axes.
            let anchor_local = corner_local((i + 2) % 4);
            let Some(inverse) = start.inverse() else {
                return start;
            };
            let moving = corner_local(i);
            let now = inverse.apply(Vec2::new(at.0, at.1));
            let mut sx = (now.x - anchor_local.x) / (moving.x - anchor_local.x);
            let mut sy = (now.y - anchor_local.y) / (moving.y - anchor_local.y);
            if !sx.is_finite() || sx.abs() < 0.01 {
                sx = 0.01_f32.copysign(if sx.is_finite() { sx } else { 1.0 });
            }
            if !sy.is_finite() || sy.abs() < 0.01 {
                sy = 0.01_f32.copysign(if sy.is_finite() { sy } else { 1.0 });
            }
            if keep_aspect {
                let s = sx.abs().max(sy.abs());
                sx = s.copysign(sx);
                sy = s.copysign(sy);
            }
            // Local scale about the anchor, then the old placement.
            let local = Affine {
                a11: sx,
                a12: 0.0,
                a21: 0.0,
                a22: sy,
                tx: anchor_local.x - anchor_local.x * sx,
                ty: anchor_local.y - anchor_local.y * sy,
            };
            Affine::compose(start, local)
        }
        Handle::Turn(_) => {
            let centre = start.apply(Vec2::new(w * 0.5, h * 0.5));
            let a0 = (grab.1 - centre.y).atan2(grab.0 - centre.x);
            let a1 = (at.1 - centre.y).atan2(at.0 - centre.x);
            let mut angle = a1 - a0;
            if keep_aspect {
                let step = std::f32::consts::PI / 12.0;
                angle = (angle / step).round() * step;
            }
            let (sin, cos) = angle.sin_cos();
            let turn = Affine {
                a11: cos,
                a12: -sin,
                a21: sin,
                a22: cos,
                tx: centre.x - cos * centre.x + sin * centre.y,
                ty: centre.y - sin * centre.x - cos * centre.y,
            };
            Affine::compose(turn, start)
        }
    }
}

fn end_drag(app: &mut EditorApp) {
    let state = &mut app.session.raster.selection;
    let Some(drag) = state.drag.take() else {
        return;
    };
    let shape = match drag {
        Drag::Marquee { from, to, add, .. } => {
            let (x0, x1) = (from.0.min(to.0).floor(), from.0.max(to.0).ceil());
            let (y0, y1) = (from.1.min(to.1).floor(), from.1.max(to.1).ceil());
            if x1 - x0 < 1.0 || y1 - y0 < 1.0 {
                None
            } else {
                Some(Outline {
                    points: vec![(x0, y0), (x1, y0), (x1, y1), (x0, y1)],
                    add,
                })
            }
        }
        Drag::Lasso { points, add, .. } => (points.len() >= 3).then_some(Outline { points, add }),
        Drag::Transform { .. } => {
            app.session.status = "Enter places the pixels; Escape too".to_string();
            return;
        }
    };
    if let Some(selection) = state.selection.as_mut() {
        if let Some(shape) = shape {
            selection.shapes.push(shape);
        }
        if selection.shapes.is_empty() || selection.bounds().is_none() {
            state.selection = None;
        }
    }
}

/// Draws the selection's marching ants, a floating piece and its handles.
pub fn draw(app: &mut EditorApp, painter: &Painter, view: &StageView) {
    let ctx = painter.ctx().clone();
    let time = ctx.input(|input| input.time);
    let phase = (time * 12.0) as f32 % 8.0;
    let state = &app.session.raster.selection;
    let mut outlines: Vec<(CanvasRef, Vec<(f32, f32)>)> = Vec::new();
    if let Some(selection) = &state.selection {
        if state.floating.is_none() {
            for shape in &selection.shapes {
                outlines.push((selection.canvas, shape.points.clone()));
            }
        }
    }
    match &state.drag {
        Some(Drag::Marquee { from, to, .. }) => {
            if let Some(selection) = &state.selection {
                outlines.push((
                    selection.canvas,
                    vec![
                        (from.0, from.1),
                        (to.0, from.1),
                        (to.0, to.1),
                        (from.0, to.1),
                    ],
                ));
            }
        }
        Some(Drag::Lasso { points, .. }) => {
            if let Some(selection) = &state.selection {
                outlines.push((selection.canvas, points.clone()));
            }
        }
        _ => {}
    }
    for (canvas, points) in &outlines {
        let screen: Vec<Pos2> = points
            .iter()
            .map(|(x, y)| to_screen(canvas, view, *x, *y))
            .collect();
        ants(painter, &screen, phase);
    }
    let Some(floating) = &state.floating else {
        return;
    };
    // The piece itself, over the stage.
    let world = floating
        .canvas
        .to_local
        .inverse()
        .unwrap_or(Affine::IDENTITY);
    let transform = Affine::compose(world, floating.transform);
    let meshes = super::stage::meshes(
        &mut app.textures,
        &ctx,
        &floating.piece,
        transform,
        view,
        Color32::WHITE,
    );
    for mesh in meshes {
        painter.add(Shape::mesh(mesh));
    }
    let corners = piece_corners(floating).map(|c| to_screen(&floating.canvas, view, c.x, c.y));
    ants(painter, &corners, phase);
    let accent = Color32::from_rgb(70, 160, 255);
    for corner in corners {
        let rect = Rect::from_center_size(corner, vec2(HANDLE, HANDLE));
        painter.rect_filled(rect, 1.5, Color32::WHITE);
        painter.rect_stroke(rect, 1.5, Stroke::new(1.0_f32, accent));
    }
    let centre = corners
        .iter()
        .fold(Pos2::ZERO, |a, c| a + c.to_vec2() / 4.0);
    painter.circle_stroke(centre, 3.5, Stroke::new(1.0_f32, accent));
}

/// A dashed black and white outline whose dashes crawl along it.
fn ants(painter: &Painter, points: &[Pos2], phase: f32) {
    if points.len() < 2 {
        return;
    }
    let mut closed = points.to_vec();
    closed.push(points[0]);
    painter.add(Shape::line(
        closed.clone(),
        Stroke::new(1.0_f32, Color32::from_black_alpha(200)),
    ));
    let dash = 4.0;
    let mut along = phase;
    for pair in closed.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        let length = a.distance(b);
        if length < 0.01 {
            continue;
        }
        let dir = (b - a) / length;
        let mut t = 0.0;
        while t < length {
            let cycle = (along + t) % (dash * 2.0);
            let run = if cycle < dash {
                dash - cycle
            } else {
                dash * 2.0 - cycle
            };
            let end = (t + run).min(length);
            if cycle < dash {
                painter.line_segment(
                    [a + dir * t, a + dir * end],
                    Stroke::new(1.0_f32, Color32::WHITE),
                );
            }
            t = end + 0.001;
        }
        along += length;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn canvas() -> CanvasRef {
        CanvasRef {
            q0rg_id: 1,
            layer_id: 1,
            asset_id: 1,
            to_local: Affine::IDENTITY,
        }
    }

    #[test]
    fn shapes_add_and_take_away() {
        let selection = Selection {
            canvas: canvas(),
            frame: 0,
            shapes: vec![
                Outline {
                    points: vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)],
                    add: true,
                },
                Outline {
                    points: vec![(4.0, 4.0), (6.0, 4.0), (6.0, 6.0), (4.0, 6.0)],
                    add: false,
                },
            ],
        };
        assert!(selection.contains(1.0, 1.0));
        assert!(!selection.contains(5.0, 5.0));
        assert!(!selection.contains(11.0, 5.0));
        let mask = selection.mask(PixelRect::new(0, 0, 10, 10));
        assert_eq!(mask.iter().filter(|m| **m).count(), 96);
    }

    #[test]
    fn moving_by_whole_pixels_keeps_pixels_exact() {
        let mut drawing = RasterAsset::new(1);
        let piece = RasterAsset::from_pixels(
            FLOATING_ID,
            PixelRect::new(0, 0, 2, 1),
            &[255, 0, 0, 255, 0, 0, 255, 255],
        );
        let floating = Floating {
            canvas: canvas(),
            frame: 0,
            piece,
            size: (2, 1),
            transform: transformed(
                Handle::Inside,
                Affine::IDENTITY,
                (0.0, 0.0),
                (30.4, 70.6),
                (2, 1),
                false,
            ),
            history_mark: 0,
        };
        place(&mut drawing, &floating);
        assert_eq!(drawing.pixel(30, 71), [255, 0, 0, 255]);
        assert_eq!(drawing.pixel(31, 71), [0, 0, 255, 255]);
        assert_eq!(drawing.pixel(32, 71), [0, 0, 0, 0]);
    }

    #[test]
    fn a_corner_drag_sizes_about_the_opposite_corner() {
        let t = transformed(
            Handle::Corner(2),
            Affine::IDENTITY,
            (10.0, 10.0),
            (20.0, 30.0),
            (10, 10),
            false,
        );
        let far = t.apply(Vec2::new(10.0, 10.0));
        let anchor = t.apply(Vec2::new(0.0, 0.0));
        assert!((far.x - 20.0).abs() < 1e-3 && (far.y - 30.0).abs() < 1e-3);
        assert!(anchor.x.abs() < 1e-3 && anchor.y.abs() < 1e-3);
        let even = transformed(
            Handle::Corner(2),
            Affine::IDENTITY,
            (10.0, 10.0),
            (20.0, 30.0),
            (10, 10),
            true,
        );
        let corner = even.apply(Vec2::new(10.0, 10.0));
        assert!((corner.x - 30.0).abs() < 1e-3 && (corner.y - 30.0).abs() < 1e-3);
    }

    #[test]
    fn a_turn_keeps_the_centre() {
        let t = transformed(
            Handle::Turn(1),
            Affine::IDENTITY,
            (20.0, 5.0),
            (5.0, 20.0),
            (10, 10),
            false,
        );
        let centre = t.apply(Vec2::new(5.0, 5.0));
        assert!((centre.x - 5.0).abs() < 1e-3 && (centre.y - 5.0).abs() < 1e-3);
    }
}
