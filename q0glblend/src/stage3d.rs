//! 3D on a stage drawn with egui: placements drawn by the 3D renderer are
//! gathered while the stage is walked and drawn together, so they hide each
//! other by depth, the moment something flat is drawn after them. The
//! editor and q0player both walk their stage through this.

use std::cell::Cell;
use std::sync::Arc;

use egui::Painter;
use q0s_format::runtime_scene::RuntimeSceneState;
use q0s_format::three::draw::{DrawItem, Frame3d, Image, PlaneDraw, View3d};
use q0s_format::three::frame::{self, FlatCut, SceneLook};
use q0s_format::three::scene::{lift, CameraPose};
use q0s_format::three::Mat4;
use q0s_format::transform::Affine;
use q0s_format::v2::{Asset, Placement, PlacementFx, ProjectV2, Target};

thread_local! {
    /// The project revision the stage is drawn at, so pictures of symbols
    /// placed in 3D can be kept between frames.
    static REVISION: Cell<Option<u64>> = const { Cell::new(None) };
    /// The editor's free 3D view, while the stage is drawn through it.
    static VIEW: Cell<Option<FreeView>> = const { Cell::new(None) };
}

/// Looking at a stage from a camera of the editor's own instead of the
/// scene's: everything on it is drawn in 3D, flat drawings as cards lying
/// where the scene camera would show them, on the paper.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FreeView {
    pub camera: CameraPose,
    pub orthographic: bool,
}

/// Draws through `view` until dropped, then puts back what was there.
pub struct FreeViewGuard(Option<FreeView>);

impl FreeViewGuard {
    pub fn set(view: Option<FreeView>) -> Self {
        Self(VIEW.with(|cell| cell.replace(view)))
    }
}

impl Drop for FreeViewGuard {
    fn drop(&mut self) {
        VIEW.with(|cell| cell.set(self.0));
    }
}

/// The free view the stage is being drawn through, if any.
pub fn free_view() -> Option<FreeView> {
    VIEW.with(Cell::get)
}

/// Runs `draw` with the project's revision known to the 3D pass. Without
/// one, symbols placed in 3D are drawn afresh every frame.
pub fn with_revision<R>(revision: Option<u64>, draw: impl FnOnce() -> R) -> R {
    let previous = REVISION.with(|cell| cell.replace(revision));
    let out = draw();
    REVISION.with(|cell| cell.set(previous));
    out
}

pub struct Stage3d {
    look: Option<SceneLook>,
    view: Option<View3d>,
    /// The root's own transform undone: stage space from a parent.
    root_inverse: Affine,
    items: Vec<DrawItem>,
    flat: FlatCut,
    /// Inside a symbol on a layer whose flat art is lit.
    pub inside_lit: bool,
    /// Seen through a free view: every placement is drawn in 3D.
    everything: bool,
    /// Liquids on this frame, drawn when their layer comes round.
    liquids: Vec<(u16, DrawItem)>,
}

impl Stage3d {
    pub const OFF: Self = Self {
        look: None,
        view: None,
        root_inverse: Affine::IDENTITY,
        items: Vec::new(),
        flat: FlatCut::OFF,
        inside_lit: false,
        everything: false,
        liquids: Vec::new(),
    };

    /// The 3D pass of a drawing of `q0rg_id`. `root` is the transform the
    /// timeline is drawn through (a 2D camera, say) and `to_screen` takes
    /// stage space to screen points. Does nothing when the project has no
    /// 3D.
    pub fn new(
        project: &ProjectV2,
        q0rg_id: u16,
        frame: u16,
        root: Affine,
        to_screen: Affine,
        scene: &RuntimeSceneState,
    ) -> Self {
        let free = free_view();
        if free.is_none() && !frame::project_uses_3d(project) {
            return Self::OFF;
        }
        let local_frame = project
            .q0rgs
            .iter()
            .find(|q0rg| q0rg.q0rg_id == q0rg_id)
            .map_or(0, |q0rg| {
                if q0rg.frame_count > 0 {
                    frame % q0rg.frame_count
                } else {
                    0
                }
            });
        let mut look = frame::scene_look(project, q0rg_id, local_frame, |instance| {
            q0s_format::raster::runtime_instance_matrix(
                project,
                q0rg_id,
                local_frame,
                instance,
                scene,
            )
        });
        if let Some(free) = free {
            look.camera = free.camera;
            look.orthographic = free.orthographic;
        }
        // The GPU pass maps stage space onto its own target; the output
        // size here is only a placeholder.
        let view = look.view(project, Affine::compose(to_screen, root), [1, 1]);
        let mut items = Vec::new();
        if free.is_some() && q0rg_id == project.meta.entry_q0rg_id {
            items.push(paper(project));
        }
        Self {
            flat: look.flat_cut(),
            look: Some(look),
            view: Some(view),
            root_inverse: root.inverse().unwrap_or(Affine::IDENTITY),
            items,
            inside_lit: false,
            everything: free.is_some(),
            liquids: frame::liquid_items(project, q0rg_id, local_frame),
        }
    }

    /// A pass that does nothing, for drawings that never hold 3D.
    pub fn off() -> Self {
        Self::OFF
    }

    pub fn active(&self) -> bool {
        self.look.is_some()
    }

    /// Is the next root layer's flat art lit by the 3D lights?
    pub fn next_layer(&mut self, layer_id: u16) -> bool {
        if !self.liquids.is_empty() {
            let (here, later): (Vec<_>, Vec<_>) = std::mem::take(&mut self.liquids)
                .into_iter()
                .partition(|(layer, _)| *layer == layer_id);
            self.liquids = later;
            self.items.extend(here.into_iter().map(|(_, item)| item));
        }
        self.flat.layer(layer_id)
    }

    /// Should this placement be drawn in 3D?
    pub fn takes(&self, asset: Option<&Asset>, fx: &PlacementFx, lit_flat: bool) -> bool {
        self.active()
            && (self.everything
                || frame::is_3d(asset, fx)
                || (lit_flat
                    && matches!(
                        asset,
                        Some(Asset::Vector(_) | Asset::Bitmap(_) | Asset::Q0v(_))
                    )))
    }

    /// Adds a placement drawn in 3D. `parent` is where its timeline sits
    /// (through the root); `local` its own transform.
    #[allow(clippy::too_many_arguments)]
    pub fn push(
        &mut self,
        project: &ProjectV2,
        q0rg_id: u16,
        placement: &Placement,
        asset: Option<&Asset>,
        parent: Affine,
        local: Affine,
        fx: PlacementFx,
        local_frame: u16,
        playback_start_frame: u16,
        lit_flat: bool,
        opacity: f32,
    ) {
        let Some(view) = self.view else {
            return;
        };
        let stage_parent = Affine::compose(self.root_inverse, parent);
        let matrix =
            frame::placement_space_matrix(&lift(stage_parent), local, &fx, placement.pivot);
        let space = fx.three.unwrap_or_default();
        let elapsed = local_frame.saturating_sub(playback_start_frame);
        match (placement.target, asset) {
            (_, Some(Asset::Model(model))) => frame::model_items(
                project,
                &model.data,
                &matrix,
                &space,
                f32::from(elapsed),
                Some((q0rg_id, placement.instance_id, local_frame)),
                opacity,
                &mut self.items,
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
                    self.items.push(item);
                }
            }
            (Target::Q0rg(child_id), None) => {
                let child_frame = q0s_format::v2::resolve_q0rg_playback_frame(
                    project,
                    child_id,
                    local_frame,
                    playback_start_frame,
                    fx.q0rg_playback,
                );
                let revision = REVISION.with(Cell::get);
                if let Some(mut item) = frame::symbol_plane(
                    project,
                    child_id,
                    child_frame,
                    matrix,
                    &view,
                    opacity,
                    space.lit,
                    revision,
                    &RuntimeSceneState::new(),
                ) {
                    // A flat symbol seen from aside keeps its place in the
                    // layer order, as on the stage.
                    if let (true, None, DrawItem::Plane(plane)) =
                        (self.everything, fx.three, &mut item)
                    {
                        plane.layered = true;
                    }
                    self.items.push(item);
                }
            }
            _ => {}
        }
    }

    /// Draws what has gathered.
    pub fn flush(&mut self, painter: &Painter) {
        if self.items.is_empty() {
            return;
        }
        let (Some(look), Some(view)) = (&self.look, self.view) else {
            self.items.clear();
            return;
        };
        let frame = Frame3d {
            view,
            rig: look.rig.clone(),
            items: std::mem::take(&mut self.items),
            overlay: None,
            clear: None,
        };
        crate::paint_frame3d(painter, painter.clip_rect(), Arc::new(frame));
    }
}

/// The stage's paper as a card at depth 0, in its background colour.
fn paper(project: &ProjectV2) -> DrawItem {
    let bg = project.meta.background_color;
    let a = f32::from(bg.a) / 255.0;
    let premultiplied = |c: u8| (f32::from(c) * a).round() as u8;
    DrawItem::Plane(PlaneDraw {
        image: Arc::new(Image {
            width: 1,
            height: 1,
            rgba: Arc::new(vec![
                premultiplied(bg.r),
                premultiplied(bg.g),
                premultiplied(bg.b),
                bg.a,
            ]),
        }),
        rect: [
            0.0,
            0.0,
            f32::from(project.meta.stage_width),
            f32::from(project.meta.stage_height),
        ],
        matrix: Mat4::IDENTITY,
        opacity: 1.0,
        lit: false,
        layered: true,
    })
}
