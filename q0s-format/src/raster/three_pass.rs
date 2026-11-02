//! 3D in the software renderer. While a frame is drawn, placements drawn in
//! 3D gather in a group for the buffer they belong to; the group is drawn
//! by the software 3D renderer and laid over the buffer the moment
//! something flat is drawn after it, so layer order holds and runs of 3D
//! hide each other by depth.
//!
//! Each buffer the frame is drawn into (the frame itself, a faded layer, a
//! tile with effects) has its own group, kept on a stack.

use std::cell::{Cell, RefCell};

use crate::runtime_scene::RuntimeSceneState;
use crate::three::draw::{DrawItem, Frame3d};
use crate::three::frame::{self, FlatCut, SceneLook};
use crate::three::scene::lift;
use crate::transform::Affine;
use crate::v2::{Asset, Placement, PlacementFx, ProjectV2, Target};

struct Group {
    buffer: usize,
    w: u32,
    h: u32,
    /// Stage space to this buffer's pixels.
    to_output: Affine,
    look: SceneLook,
    items: Vec<DrawItem>,
    flat: FlatCut,
    /// The root layer being drawn has its flat art lit.
    layer_lit: bool,
    /// Liquids on this frame, drawn when their layer comes round.
    liquids: Vec<(u16, DrawItem)>,
}

thread_local! {
    static STACK: RefCell<Vec<Group>> = const { RefCell::new(Vec::new()) };
    /// Drawing a symbol's picture for a 3D card: its 3D shows flat-on.
    static CARD: Cell<u32> = const { Cell::new(0) };
}

/// Runs `draw` as the drawing of a card's picture.
pub(crate) fn as_card<R>(draw: impl FnOnce() -> R) -> R {
    CARD.with(|c| c.set(c.get() + 1));
    let out = draw();
    CARD.with(|c| c.set(c.get() - 1));
    out
}

fn ptr(buffer: &[u8]) -> usize {
    buffer.as_ptr() as usize
}

/// Starts the 3D pass of a whole frame drawn into `buffer` through `root`
/// (stage to pixels). Returns whether there is one, to [`end`] later.
#[allow(clippy::too_many_arguments)]
pub(crate) fn begin_root(
    project: &ProjectV2,
    q0rg_id: u16,
    local_frame: u16,
    root: Affine,
    buffer: &[u8],
    w: u32,
    h: u32,
    scene_state: &RuntimeSceneState,
) -> bool {
    if !frame::project_uses_3d(project) {
        return false;
    }
    let mut look = frame::scene_look(project, q0rg_id, local_frame, |instance| {
        super::runtime_instance_matrix(project, q0rg_id, local_frame, instance, scene_state)
    });
    if CARD.with(Cell::get) > 0 {
        // Inside a card the picture is flat-on: the stage camera without
        // perspective, so the symbol looks as it does in 2D.
        look.camera = crate::three::scene::CameraPose::stage(
            project.meta.stage_width,
            project.meta.stage_height,
        );
        look.orthographic = true;
        look.light_flat = None;
    }
    let liquids = if CARD.with(Cell::get) > 0 {
        Vec::new()
    } else {
        frame::liquid_items(project, q0rg_id, local_frame)
    };
    let group = Group {
        buffer: ptr(buffer),
        w,
        h,
        to_output: root,
        flat: look.flat_cut(),
        look,
        items: Vec::new(),
        layer_lit: false,
        liquids,
    };
    STACK.with(|stack| stack.borrow_mut().push(group));
    true
}

/// Starts a group for another buffer drawn like the current one, shifted
/// by `(dx, dy)` pixels (a tile cut from it). Returns whether one began.
pub(crate) fn begin_scope(buffer: &[u8], w: u32, h: u32, dx: f32, dy: f32) -> bool {
    STACK.with(|stack| {
        let mut stack = stack.borrow_mut();
        let Some(top) = stack.last() else {
            return false;
        };
        let group = Group {
            buffer: ptr(buffer),
            w,
            h,
            to_output: Affine::compose(
                Affine {
                    tx: -dx,
                    ty: -dy,
                    ..Affine::IDENTITY
                },
                top.to_output,
            ),
            look: top.look.clone(),
            items: Vec::new(),
            flat: FlatCut::OFF,
            layer_lit: top.layer_lit,
            liquids: Vec::new(),
        };
        stack.push(group);
        true
    })
}

/// Draws what the top group gathered into `buffer`, if it is the group's.
pub(crate) fn flush(project: &ProjectV2, buffer: &mut [u8]) {
    let frame = STACK.with(|stack| {
        let mut stack = stack.borrow_mut();
        let top = stack.last_mut()?;
        if top.items.is_empty() || top.buffer != ptr(buffer) {
            return None;
        }
        Some(Frame3d {
            view: top.look.view(project, top.to_output, [top.w, top.h]),
            rig: top.look.rig.clone(),
            items: std::mem::take(&mut top.items),
            overlay: None,
            clear: None,
        })
    });
    if let Some(frame) = frame {
        let pixels = crate::three::soft::render(&frame, 2);
        frame::composite_over(buffer, &pixels);
    }
}

/// Draws and drops the top group.
pub(crate) fn end(project: &ProjectV2, buffer: &mut [u8]) {
    flush(project, buffer);
    STACK.with(|stack| {
        stack.borrow_mut().pop();
    });
}

/// The next root layer: is its flat art lit by the 3D lights?
pub(crate) fn next_layer(layer_id: u16) -> bool {
    STACK.with(|stack| {
        let mut stack = stack.borrow_mut();
        let Some(top) = stack.last_mut() else {
            return false;
        };
        if !top.liquids.is_empty() {
            let (here, later): (Vec<_>, Vec<_>) = std::mem::take(&mut top.liquids)
                .into_iter()
                .partition(|(layer, _)| *layer == layer_id);
            top.liquids = later;
            top.items.extend(here.into_iter().map(|(_, item)| item));
        }
        top.layer_lit = top.flat.layer(layer_id);
        top.layer_lit
    })
}

/// Takes a placement into the 3D pass if it is drawn in 3D. `parent` is
/// where its timeline sits in this buffer's pixels and `local` its own
/// transform. Returns true when the placement is taken.
#[allow(clippy::too_many_arguments)]
pub(crate) fn take(
    project: &ProjectV2,
    q0rg_id: u16,
    placement: &Placement,
    fx: &PlacementFx,
    parent: Affine,
    local: Affine,
    local_frame: u16,
    playback_start_frame: u16,
    buffer: &[u8],
) -> bool {
    STACK.with(|stack| {
        let mut stack = stack.borrow_mut();
        let Some(top) = stack.last_mut() else {
            return false;
        };
        if top.buffer != ptr(buffer) {
            return false;
        }
        let asset = match placement.target {
            Target::Asset(id) => project.assets.iter().find(|asset| asset.id() == id),
            Target::Q0rg(_) => None,
        };
        let lit_flat = top.layer_lit
            && matches!(
                asset,
                Some(Asset::Vector(_) | Asset::Bitmap(_) | Asset::Q0v(_))
            );
        if !frame::is_3d(asset, fx) && !lit_flat {
            return false;
        }
        let Some(back) = top.to_output.inverse() else {
            return false;
        };
        let stage_parent = Affine::compose(back, parent);
        let matrix = frame::placement_space_matrix(&lift(stage_parent), local, fx, placement.pivot);
        let space = fx.three.unwrap_or_default();
        let elapsed = local_frame.saturating_sub(playback_start_frame);
        let view = top.look.view(project, top.to_output, [top.w, top.h]);
        let opacity = fx.opacity.clamp(0.0, 1.0);
        match (placement.target, asset) {
            (_, Some(Asset::Model(model))) => frame::model_items(
                project,
                &model.data,
                &matrix,
                &space,
                f32::from(elapsed),
                Some((q0rg_id, placement.instance_id, local_frame)),
                opacity,
                &mut top.items,
            ),
            (Target::Asset(_), Some(asset)) => {
                if let Some(item) = frame::asset_plane(
                    project,
                    asset,
                    matrix,
                    &view,
                    elapsed,
                    opacity,
                    space.lit || lit_flat,
                    fx.three.is_none(),
                ) {
                    top.items.push(item);
                }
            }
            (Target::Q0rg(child_id), None) => {
                let child_frame = crate::v2::resolve_q0rg_playback_frame(
                    project,
                    child_id,
                    local_frame,
                    playback_start_frame,
                    fx.q0rg_playback,
                );
                // The card's picture is drawn by a nested frame drawing,
                // which takes the stack too: let go of it meanwhile.
                drop(stack);
                let item = frame::symbol_plane(
                    project,
                    child_id,
                    child_frame,
                    matrix,
                    &view,
                    opacity,
                    space.lit,
                    None,
                    &RuntimeSceneState::new(),
                );
                STACK.with(|stack| {
                    if let (Some(item), Some(top)) = (item, stack.borrow_mut().last_mut()) {
                        top.items.push(item);
                    }
                });
                return true;
            }
            _ => return false,
        }
        true
    })
}
