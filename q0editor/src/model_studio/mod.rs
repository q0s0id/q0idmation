//! The model studio: build, colour, rig and animate a 3D model, then use it
//! on the stage. It edits a copy of the model and writes it back to the
//! library on Save, as one undo step of the project.
//!
//! Model space is y-up. The viewport shows it through the same 3D renderer
//! as the stage, flipped into stage space (y down, z away).
//!
//! Work happens with tools picked on the left (select, move, rotate,
//! scale, add, extrude, inset, loop cut, paint, bones), handled through
//! on-screen gizmos, with every tool also on a key.

use std::collections::BTreeSet;
use std::sync::Arc;

use egui::{Pos2, Rect};
use q0model::prim::Primitive;
use q0s_format::three::draw::{Frame3d, View3d};
use q0s_format::three::model::{mesh_matrix, ChannelTarget, Clip, ModelData, Node, Trs};
use q0s_format::three::scene::{CameraPose, Light3dKind, LightRig, LitLight3d};
use q0s_format::three::{Mat4, Quat, V3};
use q0s_format::transform::Affine;

use crate::app::EditorApp;

mod commands;
pub(crate) mod gizmo;
mod input;
mod panels;
#[cfg(test)]
mod tests;
mod view;

pub use panels::render;

/// Model space to the viewport's stage space and back (its own inverse).
pub(crate) fn flip(p: V3) -> V3 {
    V3::new(p.x, -p.y, -p.z)
}

const FLIP: Mat4 = Mat4 {
    m: [
        1.0, 0.0, 0.0, 0.0, 0.0, -1.0, 0.0, 0.0, 0.0, 0.0, -1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
    ],
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Whole parts: move, turn and size meshes.
    Object,
    /// Points, edges and faces of one part.
    Edit,
    /// Bones: pose them, key them, draw them, shape the skeleton.
    Pose,
}

impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Object => "Parts",
            Self::Edit => "Shape",
            Self::Pose => "Bones",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Element {
    Vertex,
    Edge,
    Face,
}

/// What the mouse does in the viewport.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Select,
    Move,
    Rotate,
    Scale,
    AddShape,
    Extrude,
    Inset,
    LoopCut,
    Paint,
    DrawBone,
}

impl Tool {
    pub fn label(self) -> &'static str {
        match self {
            Self::Select => "Select",
            Self::Move => "Move",
            Self::Rotate => "Rotate",
            Self::Scale => "Scale",
            Self::AddShape => "Add shape",
            Self::Extrude => "Extrude",
            Self::Inset => "Inset",
            Self::LoopCut => "Loop cut",
            Self::Paint => "Paint",
            Self::DrawBone => "Draw bones",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Self::Select => "Q",
            Self::Move => "W",
            Self::Rotate => "E",
            Self::Scale => "R",
            Self::AddShape => "Shift A",
            Self::Extrude => "T",
            Self::Inset => "I",
            Self::LoopCut => "C",
            Self::Paint => "P",
            Self::DrawBone => "B",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            Self::Select => "Click to pick, Shift to add, drag a box around things",
            Self::Move => "Drag an arrow to move along it, a square to slide in its plane, the ring in the middle to move freely",
            Self::Rotate => "Drag a ring to turn about its axis, the outer ring to turn toward you",
            Self::Scale => "Drag a handle to stretch along it, the middle to size evenly",
            Self::AddShape => "Click on the floor or on a surface to put the shape there",
            Self::Extrude => "Drag the arrow to pull the picked faces out",
            Self::Inset => "Drag sideways to shrink the picked faces into a new ring",
            Self::LoopCut => "Point at an edge to see the cut, click (or drag to slide it) to cut a new loop",
            Self::Paint => "Click or drag over faces to give them the material picked above",
            Self::DrawBone => "Drag to draw a bone; the next one grows from its tip",
        }
    }

    /// The tools of a mode, in toolbar order.
    pub fn of(mode: Mode) -> &'static [Tool] {
        match mode {
            Mode::Object => &[
                Self::Select,
                Self::Move,
                Self::Rotate,
                Self::Scale,
                Self::AddShape,
                Self::Paint,
            ],
            Mode::Edit => &[
                Self::Select,
                Self::Move,
                Self::Rotate,
                Self::Scale,
                Self::Extrude,
                Self::Inset,
                Self::LoopCut,
                Self::Paint,
                Self::AddShape,
            ],
            Mode::Pose => &[Self::Select, Self::Move, Self::Rotate, Self::DrawBone],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Orientation {
    /// The model's own axes.
    Global,
    /// The picked part's or bone's axes.
    Local,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Settings {
    pub snap: bool,
    /// Degrees per snap step when turning.
    pub angle_step: f32,
    pub orientation: Orientation,
    pub shape: Primitive,
    pub detail: u32,
    /// Size of added shapes in model units; `None` fits them to the view.
    pub shape_size: Option<f32>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            snap: false,
            angle_step: 15.0,
            orientation: Orientation::Global,
            shape: Primitive::Cube,
            detail: 12,
            shape_size: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Orbit {
    pub target: V3,
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
    pub ortho: bool,
}

impl Orbit {
    pub fn eye(&self) -> V3 {
        let (sy, cy) = self.yaw.to_radians().sin_cos();
        let (sp, cp) = self.pitch.to_radians().sin_cos();
        self.target + V3::new(cp * sy, sp, cp * cy) * self.distance
    }

    pub fn pose(&self, height: f32) -> CameraPose {
        CameraPose {
            position: flip(self.eye()),
            target: flip(self.target),
            roll: 0.0,
            zoom: height.max(1.0) * 0.5 / (22.5f32.to_radians()).tan(),
        }
    }

    /// The direction the camera looks, in model space.
    pub fn forward(&self) -> V3 {
        (self.target - self.eye()).normalized()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpKind {
    Grab,
    Rotate,
    Scale,
    Inset,
    LoopCut,
}

/// What a change may move along.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Constraint {
    Free,
    /// Along a model-space direction.
    Axis(V3),
    /// Within the plane with this normal.
    Plane(V3),
}

/// What an operation started from, to redo it from each mouse move.
#[derive(Debug, Clone)]
enum Start {
    Verts {
        mesh: usize,
        list: Vec<(u32, V3)>,
    },
    Nodes {
        list: Vec<(u16, Trs)>,
    },
    Mesh {
        mesh: usize,
        faces: BTreeSet<usize>,
        original: Box<q0s_format::three::Mesh>,
    },
    Loop {
        mesh: usize,
        edge: [u32; 2],
        original: Box<q0s_format::three::Mesh>,
    },
}

/// A change following the mouse: a gizmo drag, a key-started move, an
/// inset or a loop cut being slid.
#[derive(Debug, Clone)]
pub struct Op {
    pub kind: OpKind,
    pub constraint: Constraint,
    /// Shown while it runs: "x", "xy plane"...
    pub label: String,
    start_mouse: Pos2,
    /// The middle of what moves, in model space.
    pivot: V3,
    start: Start,
    /// For undo and cancel.
    before: Box<ModelData>,
    /// Started from a key: measure from where the mouse first lands.
    pub fresh: bool,
    /// Ends when the mouse button is let go (a drag), not on a click.
    pub dragging: bool,
    /// What it does right now, for the readout.
    pub value: String,
}

/// What is under the mouse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    None,
    Part(usize),
    Vertex(u32),
    Edge([u32; 2]),
    Face(usize),
    Bone(u16),
}

pub struct ModelStudio {
    pub asset_id: u16,
    pub name: String,
    pub model: ModelData,
    revision: u64,
    display: Option<(u64, Arc<ModelData>)>,
    undo: Vec<ModelData>,
    redo: Vec<ModelData>,
    pub orbit: Orbit,
    pub mode: Mode,
    pub element: Element,
    pub tool: Tool,
    pub settings: Settings,
    pub mesh: usize,
    pub objects: BTreeSet<usize>,
    pub verts: BTreeSet<u32>,
    pub edges: BTreeSet<[u32; 2]>,
    pub faces: BTreeSet<usize>,
    pub bone: Option<u16>,
    pub op: Option<Op>,
    pub hover: Hit,
    pub clip: Option<usize>,
    pub time: f32,
    pub playing: bool,
    pub looping: bool,
    /// Bones mode shapes the skeleton instead of keying the clip.
    pub rest_edit: bool,
    pub wire: bool,
    pub xray: bool,
    pub show_bones: bool,
    pub show_grid: bool,
    pub material: usize,
    pub status: String,
    pub(crate) box_select: Option<(Pos2, Pos2)>,
    pub viewport: Rect,
    pub(crate) changed: bool,
    pub(crate) wants_texture: bool,
    /// The gizmo handle under the mouse and the one being dragged.
    pub(crate) handle_hover: Option<gizmo::Handle>,
    pub(crate) handle_drag: Option<gizmo::Handle>,
    /// A paint stroke is going: one undo step for all of it.
    pub(crate) painting: bool,
    /// Drawing a bone: where it starts (model space) and its parent.
    pub(crate) bone_start: Option<(V3, Option<u16>)>,
    /// Where the loop cut tool would cut: the hovered edge and how far
    /// along it.
    pub(crate) loop_preview: Option<([u32; 2], f32)>,
    pub show_help: bool,
    /// Asking whether to save before closing.
    pub(crate) confirm_close: bool,
}

impl ModelStudio {
    pub fn new(asset_id: u16, name: String, model: ModelData) -> Self {
        let (lo, hi) = model.bounds().unwrap_or((V3::splat(-1.0), V3::splat(1.0)));
        let size = (hi - lo).max_elem().max(1.0e-3);
        let has_bones = model.nodes.iter().any(|n| n.bone);
        Self {
            asset_id,
            name,
            orbit: Orbit {
                target: (lo + hi) * 0.5,
                yaw: 30.0,
                pitch: 20.0,
                distance: size * 3.4,
                ortho: false,
            },
            mode: Mode::Object,
            element: Element::Vertex,
            tool: Tool::Select,
            settings: Settings::default(),
            mesh: 0,
            objects: BTreeSet::new(),
            verts: BTreeSet::new(),
            edges: BTreeSet::new(),
            faces: BTreeSet::new(),
            bone: None,
            op: None,
            hover: Hit::None,
            clip: (!model.clips.is_empty()).then_some(0),
            time: 0.0,
            playing: false,
            looping: true,
            rest_edit: false,
            wire: false,
            xray: false,
            show_bones: has_bones,
            show_grid: true,
            material: 0,
            status: String::new(),
            box_select: None,
            viewport: Rect::NOTHING,
            changed: false,
            wants_texture: false,
            handle_hover: None,
            handle_drag: None,
            painting: false,
            bone_start: None,
            loop_preview: None,
            show_help: false,
            confirm_close: false,
            model,
            revision: 1,
            display: None,
            undo: Vec::new(),
            redo: Vec::new(),
        }
    }

    /// The model as drawn this frame, rebuilt after edits.
    pub fn display(&mut self) -> Arc<ModelData> {
        match &self.display {
            Some((revision, model)) if *revision == self.revision => model.clone(),
            _ => {
                let model = Arc::new(self.model.clone());
                self.display = Some((self.revision, model.clone()));
                model
            }
        }
    }

    /// Something changed the model: draw it afresh.
    pub fn touch(&mut self) {
        self.revision += 1;
        self.changed = true;
    }

    /// Remembers the model before an edit.
    pub fn snapshot(&mut self) {
        self.undo.push(self.model.clone());
        if self.undo.len() > 80 {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    pub fn undo(&mut self) {
        if let Some(previous) = self.undo.pop() {
            self.redo.push(std::mem::replace(&mut self.model, previous));
            self.after_history();
        }
    }

    pub fn redo(&mut self) {
        if let Some(next) = self.redo.pop() {
            self.undo.push(std::mem::replace(&mut self.model, next));
            self.after_history();
        }
    }

    fn after_history(&mut self) {
        self.op = None;
        self.model.sanitize();
        self.mesh = self.mesh.min(self.model.meshes.len().saturating_sub(1));
        self.objects.retain(|m| *m < self.model.meshes.len());
        self.clear_elements();
        self.bone = self
            .bone
            .filter(|b| usize::from(*b) < self.model.nodes.len());
        self.clip = self.clip.filter(|c| *c < self.model.clips.len());
        self.touch();
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn clear_elements(&mut self) {
        self.verts.clear();
        self.edges.clear();
        self.faces.clear();
    }

    /// Switches mode, keeping a sensible tool and selection.
    pub fn set_mode(&mut self, mode: Mode) {
        if mode == Mode::Edit {
            if let Some(first) = self.objects.iter().next() {
                self.mesh = *first;
            }
            if self.model.meshes.is_empty() {
                self.status = "add a shape first".into();
                return;
            }
        }
        if mode == Mode::Object && self.mode == Mode::Edit {
            self.objects = BTreeSet::from([self.mesh]);
        }
        self.mode = mode;
        if !Tool::of(mode).contains(&self.tool) {
            self.tool = Tool::Select;
        }
        if mode == Mode::Pose {
            self.show_bones = true;
        }
        self.op = None;
        self.touch();
    }

    /// Vertices of the active part that an edit moves.
    pub fn moving_verts(&self) -> BTreeSet<u32> {
        let Some(mesh) = self.model.meshes.get(self.mesh) else {
            return BTreeSet::new();
        };
        match self.element {
            Element::Vertex => self.verts.clone(),
            Element::Edge => self.edges.iter().flat_map(|e| e.iter().copied()).collect(),
            Element::Face => q0model::ops::face_verts(mesh, &self.faces),
        }
    }

    /// Faces an edit works on: picked faces, or faces whose points are all
    /// picked.
    pub fn working_faces(&self) -> BTreeSet<usize> {
        let Some(mesh) = self.model.meshes.get(self.mesh) else {
            return BTreeSet::new();
        };
        match self.element {
            Element::Face => self.faces.clone(),
            _ => q0model::ops::faces_of_verts(mesh, &self.moving_verts()),
        }
    }

    pub fn has_selection(&self) -> bool {
        match self.mode {
            Mode::Object => !self.objects.is_empty(),
            Mode::Edit => {
                !self.verts.is_empty() || !self.edges.is_empty() || !self.faces.is_empty()
            }
            Mode::Pose => self.bone.is_some(),
        }
    }

    /// Local transforms of every node right now: the clip at the playhead
    /// in Bones mode, the rest pose otherwise.
    pub fn locals(&self) -> Vec<Trs> {
        if (self.mode == Mode::Pose && !self.rest_edit) || self.playing {
            self.model.sample_clip(self.clip, self.time)
        } else {
            self.model.rest_locals()
        }
    }

    pub fn world(&self) -> Vec<Mat4> {
        self.model.world_matrices(&self.locals())
    }

    /// The viewport's camera and projection.
    pub fn view(&self, rect: Rect) -> View3d {
        let pose = self.orbit.pose(rect.height());
        View3d::new(
            pose,
            self.orbit.ortho,
            [rect.width(), rect.height()],
            Affine {
                tx: rect.min.x,
                ty: rect.min.y,
                ..Affine::IDENTITY
            },
            [rect.width().max(1.0) as u32, rect.height().max(1.0) as u32],
        )
    }

    /// Screen position of a model-space point.
    pub fn screen(&self, view: &View3d, p: V3) -> Option<Pos2> {
        view.project(flip(p)).map(|[x, y, _]| Pos2::new(x, y))
    }

    /// A model-space ray through a screen point.
    pub fn ray(&self, view: &View3d, at: Pos2) -> Option<(V3, V3)> {
        view.ray(at.x, at.y).map(|(o, d)| (flip(o), flip(d)))
    }

    /// The frame to draw: the model lit by a soft studio light, with the
    /// editing overlays.
    pub fn frame(&mut self, rect: Rect, colors: &view::Palette) -> Frame3d {
        let view = self.view(rect);
        let model = self.display();
        let world = self.world();
        let placement = Mat4::scale(V3::splat(1.0 / model.scale.max(1.0e-6)));
        let mut items = Vec::new();
        q0s_format::three::draw::push_model(&mut items, &model, &placement, &world, 1.0, true);
        // The camera's own key and fill, plus a light from straight above so
        // tops read brighter than sides whichever way the camera turns.
        let mut rig = LightRig::default_for(&view.camera);
        rig.ambient = [0.2, 0.21, 0.25];
        for (light, strength) in rig.lights.iter_mut().zip([0.68, 0.2]) {
            light.color = [strength; 3];
        }
        rig.lights.push(LitLight3d {
            kind: Light3dKind::Sun,
            position: V3::ZERO,
            direction: flip(-V3::Y),
            color: [0.24, 0.24, 0.27],
            range: 0.0,
            cos_inner: 1.0,
            cos_outer: 1.0,
            shadows: false,
        });
        let overlay = view::overlay(self, &world, colors);
        Frame3d {
            view,
            rig,
            items,
            overlay: Some(Arc::new(overlay)),
            clear: Some(colors.background),
        }
    }

    /// Size of the model, for steps and camera moves.
    pub fn size(&self) -> f32 {
        self.model
            .bounds()
            .map(|(lo, hi)| (hi - lo).max_elem())
            .filter(|s| *s > 1.0e-6)
            .unwrap_or(1.0)
    }

    /// The floor grid's step, also the snapping step: a power of ten, a
    /// tenth to a hundredth of how far the camera is from what it looks at.
    pub fn grid_step(&self) -> f32 {
        let span = self.orbit.distance.max(1.0e-4);
        10f32.powf(span.log10().floor() - 1.0)
    }

    /// How big added shapes are, in model units.
    pub fn shape_size(&self) -> f32 {
        self.settings
            .shape_size
            .unwrap_or_else(|| nice(self.orbit.distance * 0.3))
    }

    /// Makes sure part `index` has its own node, and returns it.
    pub fn ensure_node(&mut self, index: usize) -> Option<u16> {
        let mesh = self.model.meshes.get(index)?;
        if let Some(node) = mesh.node {
            return Some(node);
        }
        let mut node = Node::new(mesh.name.clone());
        node.parent = None;
        self.model.nodes.push(node);
        let id = (self.model.nodes.len() - 1) as u16;
        self.model.meshes[index].node = Some(id);
        Some(id)
    }

    /// Keys a bone's local transform at the playhead, making a clip if
    /// there is none.
    pub fn key_bone(&mut self, bone: u16, local: Trs) {
        if self.model.clips.is_empty() {
            self.model.clips.push(Clip {
                name: "Clip 1".into(),
                duration: 1.0,
                channels: Vec::new(),
            });
            self.clip = Some(0);
        }
        let clip_index = self.clip.unwrap_or(0).min(self.model.clips.len() - 1);
        self.clip = Some(clip_index);
        let time = self.time;
        let clip = &mut self.model.clips[clip_index];
        let r = local.rotation;
        clip.channel_mut(bone, ChannelTarget::Rotation)
            .set_key(time, &[r.x, r.y, r.z, r.w]);
        let t = local.translation;
        clip.channel_mut(bone, ChannelTarget::Translation)
            .set_key(time, &[t.x, t.y, t.z]);
        let s = local.scale;
        clip.channel_mut(bone, ChannelTarget::Scale)
            .set_key(time, &[s.x, s.y, s.z]);
        clip.duration = clip.duration.max(time);
    }

    /// Puts a picture file on the current material.
    pub fn load_texture(&mut self, path: &std::path::Path) {
        match std::fs::read(path)
            .ok()
            .and_then(|bytes| image::load_from_memory(&bytes).ok())
        {
            Some(picture) => {
                self.snapshot();
                let picture = picture.to_rgba8();
                let name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("picture")
                    .to_string();
                self.model.textures.push(q0s_format::three::Texture::new(
                    name,
                    picture.width(),
                    picture.height(),
                    picture.into_raw(),
                ));
                let texture = (self.model.textures.len() - 1) as u16;
                if let Some(material) = self.model.materials.get_mut(self.material) {
                    material.texture = Some(texture);
                    material.color = q0s_format::v2::Rgba {
                        r: 255,
                        g: 255,
                        b: 255,
                        a: material.color.a,
                    };
                }
                self.touch();
            }
            None => self.status = "that picture could not be read".into(),
        }
    }

    /// The middle of the selection in model space.
    pub fn selection_centre(&self) -> Option<V3> {
        let world = self.world();
        match self.mode {
            Mode::Edit => {
                let mesh = self.model.meshes.get(self.mesh)?;
                let verts = self.moving_verts();
                if verts.is_empty() {
                    return None;
                }
                let m = mesh_matrix(mesh, &world);
                let sum = verts
                    .iter()
                    .filter_map(|v| mesh.positions.get(*v as usize))
                    .fold(V3::ZERO, |s, p| s + m.transform_point(*p));
                Some(sum / verts.len() as f32)
            }
            Mode::Object => {
                if self.objects.is_empty() {
                    return None;
                }
                let mut sum = V3::ZERO;
                let mut n = 0.0;
                for index in &self.objects {
                    let Some(mesh) = self.model.meshes.get(*index) else {
                        continue;
                    };
                    let m = mesh_matrix(mesh, &world);
                    if let Some((lo, hi)) = mesh.bounds() {
                        sum += m.transform_point((lo + hi) * 0.5);
                        n += 1.0;
                    }
                }
                (n > 0.0).then(|| sum / n)
            }
            Mode::Pose => self
                .bone
                .and_then(|b| world.get(usize::from(b)))
                .map(Mat4::translation_part),
        }
    }

    /// The gizmo's axes in model space: the model's, or the picked thing's
    /// own.
    pub fn axes(&self) -> [V3; 3] {
        let global = [V3::X, V3::Y, V3::Z];
        if self.settings.orientation == Orientation::Global {
            return global;
        }
        let world = self.world();
        let node = match self.mode {
            Mode::Pose => self.bone,
            Mode::Object => self
                .objects
                .iter()
                .next()
                .and_then(|m| self.model.meshes.get(*m)?.node),
            Mode::Edit => self.model.meshes.get(self.mesh).and_then(|m| m.node),
        };
        match node.and_then(|n| world.get(usize::from(n))) {
            Some(m) => [
                m.column(0).normalized(),
                m.column(1).normalized(),
                m.column(2).normalized(),
            ],
            None => global,
        }
    }
}

/// Opens the model studio on library model `asset_id`.
pub fn open(app: &mut EditorApp, asset_id: u16) {
    let Some(data) = app
        .state
        .project
        .assets
        .iter()
        .find_map(|asset| match asset {
            q0s_format::v2::Asset::Model(model) if model.asset_id == asset_id => {
                Some(model.data.clone())
            }
            _ => None,
        })
    else {
        app.session.status = "that model is no longer in the library".into();
        return;
    };
    let name = app
        .state
        .project
        .asset_names
        .get(&asset_id)
        .cloned()
        .unwrap_or_else(|| format!("Model {asset_id}"));
    app.model_studio = Some(Box::new(ModelStudio::new(asset_id, name, (*data).clone())));
}

/// Writes the studio's model back to the library.
pub fn apply(app: &mut EditorApp) {
    let Some(studio) = app.model_studio.as_mut() else {
        return;
    };
    let asset_id = studio.asset_id;
    let mut model = studio.model.clone();
    model.sanitize();
    let name = studio.name.clone();
    studio.changed = false;
    app.history
        .snapshot_with_session(&app.state.project, &app.session);
    for asset in &mut app.state.project.assets {
        if let q0s_format::v2::Asset::Model(stored) = asset {
            if stored.asset_id == asset_id {
                stored.data = Arc::new(model);
                break;
            }
        }
    }
    if !name.trim().is_empty() {
        app.state.project.asset_names.insert(asset_id, name);
    }
    app.state.mark_dirty();
    app.textures.invalidate();
    app.session.status = "model saved to the library".into();
}

/// Rounds down to 1, 2 or 5 times a power of ten.
fn nice(value: f32) -> f32 {
    let value = value.max(1.0e-6);
    let power = 10f32.powf(value.log10().floor());
    let mantissa = value / power;
    power
        * if mantissa >= 5.0 {
            5.0
        } else if mantissa >= 2.0 {
            2.0
        } else {
            1.0
        }
}

/// Turns model-space matrix `m` about `pivot`.
pub(crate) fn about(pivot: V3, m: Mat4) -> Mat4 {
    Mat4::translation(pivot) * m * Mat4::translation(-pivot)
}

pub(crate) fn rotation_about(axis: V3, degrees: f32) -> Mat4 {
    Mat4::from_quat(Quat::from_axis_angle(axis, degrees.to_radians()))
}
