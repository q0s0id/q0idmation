use std::path::PathBuf;
use std::{
    collections::{HashMap, HashSet},
    time::Instant,
};

pub use q0s_format::geom::CapShape;
use q0s_format::transform::Affine;
use q0s_format::v2::{
    Anchor, Layer, LayerMetadata, Path as VPath, Placement, ProjectMeta, ProjectScene, ProjectV2,
    Q0rg, Rgba, RigChannel, RigPoseBlendMode, Target, Transform2D, Vec2, VectorAppearance,
    VectorAsset,
};

use crate::advanced_brush::{AdvancedBrushSettings, AdvancedBrushStroke, BrushMode};
use crate::brush::{BrushSettings, BrushStroke};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LibraryItem {
    Q0rg(u16),
    Asset(u16),
}

/// Drag-and-drop identity for a library row. `LibraryItem` ids are only unique
/// within one project, so cross-project drags must carry their source tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LibraryDragPayload {
    pub source_tab_id: u64,
    pub item: LibraryItem,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryRename {
    pub item: LibraryItem,
    pub draft: String,
    pub focus_requested: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayerRename {
    pub q0rg_id: u16,
    pub layer_id: u16,
    pub draft: String,
    pub focus_requested: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameLabelEdit {
    pub q0rg_id: u16,
    pub frame: u16,
    pub draft: String,
    pub focus_requested: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SceneRename {
    pub scene_id: u16,
    pub draft: String,
    pub focus_requested: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimelineLayerDrag {
    pub q0rg_id: u16,
    pub selection: TimelineLayerSelection,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AppearanceTransformSnapshot {
    pub asset_id: u16,
    pub field_transform: Affine,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimelineFrameDrag {
    pub q0rg_id: u16,
    pub selection: TimelineSelection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimelineRigKeySelection {
    pub q0rg_id: u16,
    pub layer_id: u16,
    pub frame: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CameraTimelineSelection {
    pub anchor_frame: u16,
    pub focus_frame: u16,
}

impl CameraTimelineSelection {
    pub const fn single(frame: u16) -> Self {
        Self {
            anchor_frame: frame,
            focus_frame: frame,
        }
    }

    pub const fn first_frame(self) -> u16 {
        if self.anchor_frame < self.focus_frame {
            self.anchor_frame
        } else {
            self.focus_frame
        }
    }

    pub const fn last_frame(self) -> u16 {
        if self.anchor_frame > self.focus_frame {
            self.anchor_frame
        } else {
            self.focus_frame
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CameraFrameDrag {
    pub selection: CameraTimelineSelection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimelineLayerSelection {
    pub anchor_layer_id: u16,
    pub focus_layer_id: u16,
}

impl TimelineLayerSelection {
    pub const fn single(layer_id: u16) -> Self {
        Self {
            anchor_layer_id: layer_id,
            focus_layer_id: layer_id,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Select,
    Lasso,
    Hand,
    Subselect,
    Pen,
    Pencil,
    Brush,
    Eraser,
    Line,
    Rectangle,
    Oval,
    Bucket,
    Eyedropper,
    Text,
    Rig,
}

impl Tool {
    pub const ALL: [Tool; 15] = [
        Tool::Select,
        Tool::Lasso,
        Tool::Hand,
        Tool::Subselect,
        Tool::Pen,
        Tool::Pencil,
        Tool::Brush,
        Tool::Eraser,
        Tool::Line,
        Tool::Rectangle,
        Tool::Oval,
        Tool::Bucket,
        Tool::Eyedropper,
        Tool::Text,
        Tool::Rig,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Tool::Select => "Select",
            Tool::Lasso => "Lasso",
            Tool::Hand => "Hand",
            Tool::Subselect => "Subselect",
            Tool::Pen => "Pen",
            Tool::Pencil => "Pencil",
            Tool::Brush => "Brush",
            Tool::Eraser => "Eraser",
            Tool::Line => "Line",
            Tool::Rectangle => "Rectangle",
            Tool::Oval => "Oval",
            Tool::Bucket => "Paint Bucket",
            Tool::Eyedropper => "Eyedropper",
            Tool::Text => "Text",
            Tool::Rig => "Rig",
        }
    }

    pub fn glyph(self) -> &'static str {
        match self {
            Tool::Select => "V",
            Tool::Lasso => "Q",
            Tool::Hand => "H",
            Tool::Subselect => "A",
            Tool::Pen => "P",
            Tool::Pencil => "Y",
            Tool::Brush => "B",
            Tool::Eraser => "E",
            Tool::Line => "N",
            Tool::Rectangle => "R",
            Tool::Oval => "O",
            Tool::Bucket => "K",
            Tool::Eyedropper => "I",
            Tool::Text => "T",
            Tool::Rig => "G",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlacementRef {
    pub q0rg_id: u16,
    pub layer_id: u16,
    pub placement_idx: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct PathRef {
    pub q0rg_id: u16,
    pub layer_id: u16,
    pub placement_idx: usize,
    pub path_idx: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnchorHandleKind {
    In,
    Out,
}

/// Editor-only reference to one authored stroke segment. This never reaches
/// q1s: once edited, the segment is materialized as ordinary open raw paths.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StrokeSegmentRef {
    pub path: PathRef,
    pub segment_idx: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Selection {
    None,
    Asset(u16),
    Q0rg(u16),
    Placement {
        q0rg_id: u16,
        layer_id: u16,
        placement_idx: usize,
    },
    /// One editable raw-graphics contour inside a vector placement. Flash
    /// treats fills/lines as selectable pieces even though they share one
    /// drawing surface; this preserves that distinction without turning each
    /// brush gesture into a separate display object.
    Path {
        q0rg_id: u16,
        layer_id: u16,
        placement_idx: usize,
        path_idx: usize,
    },
    /// Marquee selection of several raw-graphics contours. They can share one
    /// Placement/asset; selection still remains path-granular.
    Paths(Vec<PathRef>),
    /// Flash-style raw fill/outline selection. Whole raw strokes stay path-granular,
    /// while authored sides of closed stroked shapes stay segment-granular so a
    /// Shift-selected fill plus one outline side cannot silently become the whole
    /// outline. objects keeps display-object selection separate from raw parts.
    RawParts {
        fills: Vec<PathRef>,
        strokes: Vec<PathRef>,
        stroke_segments: Vec<StrokeSegmentRef>,
        objects: Vec<PlacementRef>,
    },
    /// One authored segment of a closed raw outline. Selection itself is
    /// non-destructive; the first edit cuts it into ordinary open raw paths.
    StrokeSegment(StrokeSegmentRef),
    /// V-tool marquee selection of only part of one raw contour. The selected
    /// boundary anchors move together and reshape the fill, matching Animate's
    /// partial raw-shape selection rather than Subselect's whole-path editing.
    PathPoints {
        path: PathRef,
        anchor_indices: Vec<usize>,
        bounds_min: Vec2,
        bounds_max: Vec2,
    },
    /// Subselect's authored anchor/Bezier-point selection. This is deliberately
    /// distinct from `PathPoints`: leaving Subselect promotes this to the whole
    /// raw path instead of reinterpreting anchor points as a partial fill area.
    SubselectPoints {
        path: PathRef,
        anchor_indices: Vec<usize>,
        bounds_min: Vec2,
        bounds_max: Vec2,
    },
    /// Non-destructive V marquee or lasso over raw drawing surfaces.
    /// Geometry is not split until the user actually drags the selection.
    RawArea {
        /// Identity raw drawing surfaces intersected by the selection region.
        placements: Vec<PlacementRef>,
        /// Display objects intersected by the same region.
        objects: Vec<PlacementRef>,
        bounds_min: Vec2,
        bounds_max: Vec2,
        /// Freehand clip for a lasso selection; None uses the rectangular bounds.
        clip_region: Option<geo::MultiPolygon<f64>>,
    },
    /// Materialized mixed free-transform selection. Raw marquee fragments are
    /// cut into path-granular geometry only when the user starts moving or
    /// transforming them; display objects remain ordinary placements.
    Mixed {
        paths: Vec<PathRef>,
        objects: Vec<PlacementRef>,
    },
    /// Marquee result: 2+ placements selected at once. Single-element marquee
    /// hits collapse back to `Selection::Placement` so existing single-select
    /// code paths keep working unchanged.
    Multi(Vec<PlacementRef>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimelineSelection {
    pub anchor_layer_id: u16,
    pub anchor_frame: u16,
    pub focus_layer_id: u16,
    pub focus_frame: u16,
}

impl TimelineSelection {
    pub const fn single(layer_id: u16, frame: u16) -> Self {
        Self {
            anchor_layer_id: layer_id,
            anchor_frame: frame,
            focus_layer_id: layer_id,
            focus_frame: frame,
        }
    }
}

#[derive(Debug, Clone)]
pub struct TimelineFrameClipboardRow {
    /// Original layer. Paste uses this only to distinguish a temporal copy on
    /// the same track from a spatial copy into another layer.
    pub source_layer_id: u16,
    /// Relative frame offsets that were actually selected. Ordinary range
    /// selections store every offset; ctrl-selected cells can stay sparse.
    pub selected_frames: Vec<u16>,
    pub explicit_keyframes: Vec<u16>,
    pub placements: Vec<Placement>,
    /// Frame scripts are timeline data, not display objects. Frames are stored
    /// relative to the copied range, exactly like placements/keyframe markers.
    pub frame_scripts: Vec<q0s_format::v2::FrameScript>,
}

#[derive(Debug, Clone)]
pub struct TimelineFrameClipboard {
    pub width: u16,
    pub rows: Vec<TimelineFrameClipboardRow>,
    /// Rig keys are included only when the frame selection spans every drawable
    /// layer in the q0rg. That makes whole-character time edits coherent without
    /// making a one-layer copy unexpectedly rewrite the character rig.
    pub rig_channels: Vec<RigChannel>,
}

#[derive(Debug, Clone)]
pub struct TimelineLayerClipboard {
    pub layers: Vec<Layer>,
    pub metadata: Vec<(u16, LayerMetadata)>,
    pub frame_scripts: Vec<q0s_format::v2::FrameScript>,
}

#[derive(Debug, Clone)]
pub enum TimelineClipboard {
    Frames(TimelineFrameClipboard),
    Layers(TimelineLayerClipboard),
}

#[derive(Debug, Clone)]
pub struct RawVectorClipboard {
    pub vector: VectorAsset,
    /// Appearance is part of raw artwork, not renderer-only state. When present
    /// it carries a frozen material source/post-material clip with the snapshot.
    pub appearance: Option<VectorAppearance>,
}

#[derive(Debug, Clone, Default)]
pub struct ClipboardPayload {
    /// Display-object placements. Targets remain project-local, matching the
    /// old clipboard contract, while transforms and ordering are preserved.
    pub placements: Vec<Placement>,
    /// Standalone raw vector snapshots. Paste assigns fresh asset ids and
    /// keeps them as identity raw-graphics placements.
    pub raw_vectors: Vec<RawVectorClipboard>,
    /// Center of the copied artwork group in stage coordinates. All pasted
    /// artwork receives one shared delta from this point, so mixed raw/q0rg
    /// selections keep their relative spacing instead of collapsing together.
    pub artwork_center: Option<Vec2>,
    /// Timeline cells or complete layer/folder blocks. This stays project-local
    /// for the same reason as placement targets: ids refer to the open project.
    pub timeline: Option<TimelineClipboard>,
}

impl ClipboardPayload {
    pub(crate) fn validate_for_q0rg(
        &self,
        project: &ProjectV2,
        q0rg_id: u16,
    ) -> Result<(), q0s_format::Error> {
        match &self.timeline {
            Some(TimelineClipboard::Frames(frames)) => q0s_format::v2::validate_q0rg_insertions(
                project,
                q0rg_id,
                frames
                    .rows
                    .iter()
                    .flat_map(|row| &row.placements)
                    .map(|p| p.target),
            ),
            Some(TimelineClipboard::Layers(layers)) => q0s_format::v2::validate_q0rg_insertions(
                project,
                q0rg_id,
                layers
                    .layers
                    .iter()
                    .flat_map(|layer| &layer.placements)
                    .map(|p| p.target),
            ),
            None => q0s_format::v2::validate_q0rg_insertions(
                project,
                q0rg_id,
                self.placements.iter().map(|p| p.target),
            ),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.placements.is_empty() && self.raw_vectors.is_empty() && self.timeline.is_none()
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RigMode {
    #[default]
    Simple,
    Pro,
}

impl RigMode {
    pub const ALL: [Self; 2] = [Self::Simple, Self::Pro];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Simple => "Simple",
            Self::Pro => "Pro",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RigEditMode {
    #[default]
    Pose,
    AddBone,
}

impl RigEditMode {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Pose => "Pose",
            Self::AddBone => "Add bone",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RigBoneDrag {
    pub node_id: u16,
    pub center_world: Vec2,
    pub parent_world_angle: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RigControlDrag {
    pub control_id: u16,
    pub start_cursor: Vec2,
    pub start_x: f32,
    pub start_y: f32,
    pub start_value: f32,
}

pub struct ProjectState {
    pub project: ProjectV2,
    pub file_path: Option<PathBuf>,
    pub dirty: bool,
    /// Advances whenever edits can affect rendered or cached project data.
    pub(crate) revision: u64,
    /// Advances when render caches need a full compatibility check. Live
    /// artwork transforms update their affected cache entries directly.
    pub(crate) render_revision: u64,
    /// Advances when library rows (names, kinds, uses, or item count) can change.
    pub(crate) library_revision: u64,
}

impl ProjectState {
    pub fn new_default() -> Self {
        Self {
            project: default_project(),
            file_path: None,
            dirty: false,
            revision: 0,
            render_revision: 0,
            library_revision: 0,
        }
    }

    pub fn replace(&mut self, mut project: ProjectV2, path: Option<PathBuf>) {
        let repaired = crate::timeline_edit::sync_project_folder_timelines(&mut project);
        self.project = project;
        self.file_path = path;
        self.dirty = repaired;
        self.revision = self.revision.wrapping_add(1);
        self.render_revision = self.render_revision.wrapping_add(1);
        self.library_revision = self.library_revision.wrapping_add(1);
    }

    pub fn mark_dirty(&mut self) {
        self.mark_render_dirty();
        self.library_revision = self.library_revision.wrapping_add(1);
    }

    /// Invalidate revision-scoped render state without rebuilding library rows.
    /// Effect edits need this: resolved layer frames also cache PlacementFx.
    pub fn mark_render_dirty(&mut self) {
        self.mark_artwork_dirty();
        self.render_revision = self.render_revision.wrapping_add(1);
    }

    /// Mark artwork edits whose render caches are refreshed separately.
    /// Keeps render_revision stable for incremental drag/brush updates. Model
    /// changes such as PlacementFx edits must use mark_render_dirty instead.
    pub fn mark_artwork_dirty(&mut self) {
        self.dirty = true;
        self.revision = self.revision.wrapping_add(1);
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn render_revision(&self) -> u64 {
        self.render_revision
    }

    pub fn library_revision(&self) -> u64 {
        self.library_revision
    }

    pub fn title(&self) -> String {
        let path = self
            .file_path
            .as_ref()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| {
                let name = self.project.meta.name.trim();
                let name = if name.is_empty() { "untitled" } else { name };
                if name.to_ascii_lowercase().ends_with(".q1s") {
                    name.to_string()
                } else {
                    format!("{name}.q1s")
                }
            });
        let dirty = if self.dirty { "*" } else { "" };
        format!("{}{} - q0editor", truncate_label(&path, 72), dirty)
    }
}

fn truncate_label(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let keep = max_chars.saturating_sub(3);
    let mut shortened: String = text.chars().take(keep).collect();
    shortened.push_str("...");
    shortened
}

/// Identifies one of the 8 transform handles around a selection bounding box.
/// Corner handles resize both axes; edge handles resize one axis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Handle {
    TopLeft,
    TopRight,
    BottomRight,
    BottomLeft,
    MidTop,
    MidRight,
    MidBottom,
    MidLeft,
}

impl Handle {
    pub fn is_corner(self) -> bool {
        matches!(
            self,
            Handle::TopLeft | Handle::TopRight | Handle::BottomRight | Handle::BottomLeft
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransformEdge {
    Top,
    Right,
    Bottom,
    Left,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TransformPivot {
    pub selection: Selection,
    pub point: Vec2,
}

#[derive(Debug, Clone, Copy)]
pub enum GroupTransformOperation {
    Move {
        start_cursor: Vec2,
    },
    Scale {
        handle: Handle,
        start_bounds: (f32, f32, f32, f32),
    },
    Rotate {
        center: Vec2,
        start_angle: f32,
    },
    Skew {
        edge: TransformEdge,
        start_bounds: (f32, f32, f32, f32),
        start_cursor: Vec2,
    },
}

#[derive(Debug, Clone)]
pub enum ToolState {
    Idle,
    LassoDrawing {
        points: Vec<Vec2>,
    },
    PenDrawing {
        anchors: Vec<Anchor>,
    },
    FreehandDrawing {
        points: Vec<Vec2>,
    },
    BrushDrawing {
        stroke: BrushStroke,
    },
    AdvancedBrushDrawing {
        stroke: AdvancedBrushStroke,
    },
    EraserDrawing {
        stroke: BrushStroke,
    },
    PrimitiveDrawing {
        start: Vec2,
        end: Vec2,
    },
    DraggingPlacement {
        q0rg_id: u16,
        layer_id: u16,
        placement_idx: usize,
        start_cursor: Vec2,
        cursor_offset: Vec2,
        pivot_cursor_offset: Option<Vec2>,
    },
    /// Move one raw vector contour without moving the containing Placement.
    DraggingPath {
        q0rg_id: u16,
        layer_id: u16,
        placement_idx: usize,
        path_idx: usize,
        start_cursor: Vec2,
        start_path: VPath,
    },
    DraggingPaths {
        refs: Vec<PathRef>,
        start_cursor: Vec2,
        start_paths: std::sync::Arc<Vec<VPath>>,
        start_appearances: std::sync::Arc<Vec<AppearanceTransformSnapshot>>,
        start_pivot: Option<Vec2>,
    },
    /// Axis-scale one connected raw-graphics selection without turning the
    /// containing raw placement into a selectable display object.
    DraggingRawHandle {
        refs: Vec<PathRef>,
        start_paths: std::sync::Arc<Vec<VPath>>,
        start_appearances: std::sync::Arc<Vec<AppearanceTransformSnapshot>>,
        handle: Handle,
        start_bounds: (f32, f32, f32, f32),
        start_pivot: Vec2,
    },
    DraggingRawRotate {
        refs: Vec<PathRef>,
        start_paths: std::sync::Arc<Vec<VPath>>,
        start_appearances: std::sync::Arc<Vec<AppearanceTransformSnapshot>>,
        center: Vec2,
        start_angle: f32,
    },
    DraggingRawSkew {
        refs: Vec<PathRef>,
        start_paths: std::sync::Arc<Vec<VPath>>,
        start_appearances: std::sync::Arc<Vec<AppearanceTransformSnapshot>>,
        edge: TransformEdge,
        start_bounds: (f32, f32, f32, f32),
        start_cursor: Vec2,
        start_pivot: Vec2,
    },
    DraggingPathPoints {
        path: PathRef,
        anchor_indices: Vec<usize>,
        start_cursor: Vec2,
        start_path: VPath,
        start_bounds_min: Vec2,
        start_bounds_max: Vec2,
    },
    /// Drags one end of a selected straight line while the other end stays:
    /// moving, turning and stretching the line in one gesture.
    DraggingLineEnd {
        path: PathRef,
        anchor_idx: usize,
        start_cursor: Vec2,
        start_path: VPath,
    },
    /// Direct-selection drag of one authored Bezier control handle. The source
    /// path snapshot keeps each pointer tick absolute, so dragging cannot
    /// accumulate numeric drift or mutate the opposite handle accidentally.
    DraggingSubselectHandle {
        path: PathRef,
        anchor_idx: usize,
        handle: AnchorHandleKind,
        start_path: VPath,
    },
    DraggingCurve {
        path: PathRef,
        segment_idx: usize,
        t: f32,
        start_cursor: Vec2,
        start_path: VPath,
    },
    /// Active while the user drags a transform handle around the selection.
    /// `start_*` snapshots are captured on drag-start so each frame's update
    /// can be computed from the original geometry, not the previous tick.
    DraggingHandle {
        q0rg_id: u16,
        layer_id: u16,
        placement_idx: usize,
        handle: Handle,
        start_transform: Transform2D,
        /// Local-space AABB of the placement's content (asset / q0rg).
        start_local_bbox: (f32, f32, f32, f32),
        /// World-space AABB at drag start (kept for compatibility/status).
        start_world_bbox: (f32, f32, f32, f32),
        pivot_local: Option<Vec2>,
    },
    DraggingPlacementRotate {
        q0rg_id: u16,
        layer_id: u16,
        placement_idx: usize,
        start_transform: Transform2D,
        center_local: Vec2,
        center_world: Vec2,
        start_angle: f32,
    },
    DraggingPlacementSkew {
        q0rg_id: u16,
        layer_id: u16,
        placement_idx: usize,
        edge: TransformEdge,
        start_transform: Transform2D,
        start_local_bbox: (f32, f32, f32, f32),
        start_cursor_local: Vec2,
        pivot_local: Option<Vec2>,
    },
    DraggingGroup {
        refs: Vec<PathRef>,
        start_paths: std::sync::Arc<Vec<VPath>>,
        start_appearances: std::sync::Arc<Vec<AppearanceTransformSnapshot>>,
        objects: Vec<PlacementRef>,
        start_transforms: Vec<Transform2D>,
        operation: GroupTransformOperation,
        start_pivot: Vec2,
    },
    DraggingTransformPivot {
        selection: Selection,
    },
    /// Rubber-band rectangle from `start` to the current cursor position.
    /// Active while the user drags Select tool over empty stage. On drag-stop
    /// the rect's contents are committed as a `Selection::Placement` (single
    /// hit) or `Selection::Multi` (2+ hits).
    Marquee {
        start: Vec2,
    },
}

impl ToolState {
    pub fn is_drawing(&self) -> bool {
        matches!(
            self,
            ToolState::PenDrawing { .. }
                | ToolState::FreehandDrawing { .. }
                | ToolState::BrushDrawing { .. }
                | ToolState::EraserDrawing { .. }
                | ToolState::PrimitiveDrawing { .. }
        )
    }
}

/// Mutually exclusive onion preview and range editing, with a shared ruler range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OnionSettings {
    pub enabled: bool,
    pub edit_frames: bool,
    pub all_unlocked_layers: bool,
    pub before: u16,
    pub after: u16,
}

impl Default for OnionSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            edit_frames: false,
            all_unlocked_layers: false,
            before: 2,
            after: 2,
        }
    }
}

/// Stage viewport: zoom is a multiplier on top of the auto-fit scale (1.0 = fit
/// to canvas), pan is a screen-space offset in pixels relative to the canvas
/// centre.  `panning` flips on while the user holds middle mouse to drag.
#[derive(Debug, Clone, Copy)]
pub struct Viewport {
    pub zoom: f32,
    pub pan: Vec2,
    pub panning: bool,
    /// Active hand interaction: permanent Hand tool or temporary Space override.
    pub hand_active: bool,
    pub touch: TouchGesture,
    /// Zoom and pan a zoom command is gliding towards; any hands-on
    /// navigation stops it where it is.
    pub glide: Option<(f32, Vec2)>,
}

/// Several fingers on the canvas at once, from the first touch to the last
/// release.
#[derive(Debug, Clone, Copy, Default)]
pub struct TouchGesture {
    pub active: bool,
    pub started: f64,
    /// Where the fingers are centred now; zooming keeps this stage point put.
    pub center: egui::Pos2,
    pub most_fingers: usize,
    /// How far the fingers moved in points, with zooming counted as movement.
    pub travel: f32,
}

impl Viewport {
    pub fn identity() -> Self {
        Self {
            zoom: 1.0,
            pan: Vec2::new(0.0, 0.0),
            panning: false,
            hand_active: false,
            touch: TouchGesture::default(),
            glide: None,
        }
    }

    /// Zooms by `factor` around the middle of the view, gliding there.
    /// Presses in quick succession add up. Returns the zoom it heads for.
    pub fn glide_zoom(&mut self, factor: f32) -> f32 {
        let (zoom, pan) = self.glide.unwrap_or((self.zoom, self.pan));
        let target = (zoom * factor).clamp(0.05, 32.0);
        // The stage point in the middle of the view stays put when the pan,
        // measured in screen pixels, grows with the zoom.
        let keep = target / zoom;
        self.glide = Some((target, Vec2::new(pan.x * keep, pan.y * keep)));
        target
    }

    /// Glides back to the fitted, centred view.
    pub fn glide_home(&mut self) {
        self.glide = Some((1.0, Vec2::new(0.0, 0.0)));
    }

    /// Moves one frame of `dt` seconds along a glide. True while it still
    /// has a way to go.
    pub fn step_glide(&mut self, dt: f32) -> bool {
        let Some((zoom, pan)) = self.glide else {
            return false;
        };
        // Eases out over roughly a sixth of a second at any frame rate.
        let t = 1.0 - (-dt.clamp(0.001, 0.05) * 18.0).exp();
        self.zoom = (self.zoom.ln() + (zoom.ln() - self.zoom.ln()) * t).exp();
        self.pan = Vec2::new(
            self.pan.x + (pan.x - self.pan.x) * t,
            self.pan.y + (pan.y - self.pan.y) * t,
        );
        let arrived = (self.zoom / zoom - 1.0).abs() < 0.002
            && (pan.x - self.pan.x).abs() < 0.5
            && (pan.y - self.pan.y).abs() < 0.5;
        if arrived {
            self.zoom = zoom;
            self.pan = pan;
            self.glide = None;
        }
        !arrived
    }
}

impl Default for Viewport {
    fn default() -> Self {
        Self::identity()
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum BrushLibraryFilter {
    #[default]
    All,
    Builtin,
    Created,
}

impl BrushLibraryFilter {
    pub const ALL: [Self; 3] = [Self::All, Self::Builtin, Self::Created];

    pub const fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Builtin => "Builtin",
            Self::Created => "Created",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrushSizePreview {
    Size,
    MinimumSize,
}

#[derive(Debug, Clone)]
pub struct FrameScriptEditor {
    pub q0rg_id: u16,
    pub layer_id: u16,
    pub frame: u16,
    pub source: String,
}

pub struct Session {
    pub current_q0rg_id: u16,
    pub current_layer_id: u16,
    pub current_frame: u16,
    /// Timeline-only cursor beyond the current q0rg tail. Stage tools keep using
    /// current_frame until F5/F6/F7 materialises this virtual destination.
    pub pending_timeline_frame: Option<u16>,
    /// Rectangular frame-cell selection in the timeline, separate from stage selection.
    pub timeline_selection: Option<TimelineSelection>,
    /// Individually ctrl-selected frame cells. The q0rg id keeps selections
    /// from a previously open scene from appearing in the current timeline.
    pub timeline_cell_selection: Option<(u16, Vec<(u16, u16)>)>,
    /// View-only Animate-style wireframe display for the active stage.
    pub outline_mode: bool,
    /// Per-layer wireframe display, keyed by q0rg and layer id.
    pub outline_layers: HashSet<(u16, u16)>,
    /// Editor-only mask-source visibility. Hiding a source keeps its clipping
    /// effect active and leaves the source layer editable.
    pub hidden_mask_art_layers: HashSet<(u64, u16, u16)>,
    /// Selected layer rows for layer clipboard operations.
    pub timeline_layer_selection: Option<TimelineLayerSelection>,
    /// One-shot timeline scroll request from selecting a stage object.
    pub timeline_scroll_to_layer: Option<(u16, u16)>,
    pub timeline_scroll_to_frame: Option<(u16, u16)>,
    /// Project-level camera track selection. Kept separate from layer ids so
    /// the camera never becomes selectable artwork.
    pub camera_track_selected: bool,
    /// Frame-cell selection inside the camera row. It mirrors ordinary
    /// timeline selection without inventing a fake layer id.
    pub camera_timeline_selection: Option<CameraTimelineSelection>,
    pub current_tool: Tool,
    pub tool_state: ToolState,
    pub selection: Selection,
    pub transform_pivot: Option<TransformPivot>,
    pub marquee_base_selection: Option<Selection>,
    /// One history step for a completed selection gesture, never per pointer move.
    pub selection_history_gesture: Option<(HistoryPivotState, bool)>,
    pub(crate) frame_edit_gesture: Option<crate::frame_edit::Gesture>,
    pub(crate) frame_edit_selection: Vec<crate::frame_edit::FrameRef>,
    #[allow(clippy::type_complexity)]
    pub symbol_edit_click: Option<(u16, Option<(u16, usize)>, f64, egui::Pos2)>,
    pub timeline_horizontal_scroll: f32,
    pub preview_loop_ranges: HashMap<u16, (bool, u16, u16)>,
    pub show_animation_preview: bool,
    pub animation_preview_frame: u16,
    pub animation_preview_playing: bool,
    pub animation_preview_tick: Instant,
    pub breadcrumb: Vec<u16>,
    /// Parent-chain context for Animate-style Edit In Place. Each entry maps
    /// one active q0rg into document-stage coordinates and remembers the frame
    /// and instance through which it was opened.
    pub edit_in_place: Vec<EditInPlaceContext>,
    /// Transient Library-panel filter and sort state.
    pub library_search: String,
    pub library_sort_ascending: bool,
    pub library_rename: Option<LibraryRename>,
    pub text_editor: Option<crate::text_tool::TextDraft>,
    pub(crate) font_audit: crate::font_replacement::ProjectFontAudit,
    pub text_defaults: crate::text_tool::TextDraft,
    pub text_inspector: Option<crate::text_tool::TextDraft>,
    pub text_canvas_click_consumed: bool,
    pub layer_rename: Option<LayerRename>,
    pub frame_label_edit: Option<FrameLabelEdit>,
    pub scene_rename: Option<SceneRename>,
    pub timeline_layer_drag: Option<TimelineLayerDrag>,
    pub timeline_frame_drag: Option<TimelineFrameDrag>,
    pub timeline_rig_key_selection: Option<TimelineRigKeySelection>,
    pub timeline_rig_key_drag: Option<TimelineRigKeySelection>,
    pub camera_frame_drag: Option<CameraFrameDrag>,
    pub playing: bool,
    /// Record motion while objects are held: a keyframe at every frame tick.
    pub record_held: bool,
    /// When the last recorded keyframe was taken, in input time.
    pub last_held_key: Option<f64>,
    /// Whether this hold already recorded a keyframe, and so an undo step.
    pub held_run_keyed: bool,
    pub last_tick: Instant,
    /// Persistent q0lang state for editor playback preview. Scrubbing does not
    /// execute it; playback/frame-runtime transitions do.
    pub preview_q0lang_runtime: q0s_format::q0lang::runtime::Runtime,
    pub preview_q0lang_initialized: bool,
    pub preview_script_diagnostics: Vec<q0s_format::q0lang::runtime::RuntimeDiagnostic>,
    pub preview_frame_script_entries: u64,
    pub preview_audio_resync_needed: bool,
    pub audio_scrubbing: bool,
    pub status: String,
    pub stroke_color: Rgba,
    pub stroke_width: f32,
    pub fill_color: Option<Rgba>,
    pub brush: BrushSettings,
    pub brush_mode: BrushMode,
    pub advanced_brush: AdvancedBrushSettings,
    /// Temporary centre-stage nib preview while the user edits brush size controls.
    pub brush_size_preview: Option<BrushSizePreview>,
    /// Draft name used by the Advanced preset library.
    pub advanced_brush_preset_name: String,
    /// Original custom preset name being edited, so rename/update/delete act on
    /// one library entry instead of accidentally duplicating it.
    pub advanced_brush_selected_preset: Option<String>,
    /// Transient source filter for the unified Advanced brush library.
    pub advanced_brush_library_filter: BrushLibraryFilter,
    /// Independent classic eraser size while brush/eraser sync is disabled.
    pub eraser_size: f32,
    /// Cap shape applied to brush strokes when committing them, and to
    /// stroke-to-fill conversions in the right-click menu. Per-session, so
    /// the user picks once and forgets.
    pub brush_cap: CapShape,
    /// How new strokes turn their corners.
    pub brush_join: q0s_format::geom::JoinShape,
    /// Rigging uses one project/runtime model; this is only the editor presentation mode.
    pub rig_mode: RigMode,
    pub rig_edit_mode: RigEditMode,
    /// Physics editor window and preview; hidden unless enabled in Settings.
    pub physics: crate::physics::PhysicsSession,
    /// Particle editor window; hidden unless enabled in Settings.
    pub particles: crate::particles::ParticleSession,
    /// Light editor window; hidden unless enabled in Settings.
    pub lights: crate::lights::LightSession,
    /// The background preview both windows share.
    pub sim_preview: crate::sim_preview::SimPreview,
    pub rig_auto_key: bool,
    pub rig_selected_node: Option<u16>,
    pub rig_selected_control: Option<u16>,
    pub rig_pose_blend_weight: f32,
    pub rig_pose_blend_mode: RigPoseBlendMode,
    pub rig_mirror_partner_node: Option<u16>,
    pub rig_mirror_partner_control: Option<u16>,
    pub rig_variant_add_target: Option<Target>,
    pub rig_pending_bone_start: Option<Vec2>,
    pub rig_bone_drag: Option<RigBoneDrag>,
    pub rig_control_drag: Option<RigControlDrag>,
    pub show_credits: bool,
    pub credits_opened_at: Instant,
    pub show_settings: bool,
    pub show_shortcuts: bool,
    pub shortcut_capture: Option<crate::hotkeys::ShortcutCommand>,
    pub shortcut_feedback: Option<String>,
    pub easing_editor: Option<crate::easing::EasingEditorState>,
    pub tween_warning: Option<String>,
    pub viewport: Viewport,
    /// Clipboard for display objects, raw graphics, or a mixed selection.
    pub clipboard: Option<ClipboardPayload>,
    /// Last pointer position observed over the stage canvas, expressed in stage
    /// coordinates. Keyboard paste uses this even though shortcut handling runs
    /// before the immediate-mode stage widget is rebuilt for the current frame.
    pub stage_cursor: Option<Vec2>,
    pub onion: OnionSettings,
    /// True while the q0lang script editor window is open. Driven by
    /// `Action::OpenQ0langEditor` / the close button on the window itself.
    pub show_q0lang_editor: bool,
    /// Which q0rg's `script` field the editor is bound to. `None` falls
    /// back to `current_q0rg_id` when the window opens, so F9 always does
    /// the right thing without first selecting a q0rg.
    pub q0lang_target: Option<u16>,
    /// "Armed" = the next script-text mutation should snapshot history.
    /// Flips on whenever the script TextEdit gains focus (start of a new
    /// edit session) and back off after the first snapshot, so a long
    /// burst of typing collapses to a single undo entry.
    pub q0lang_edit_armed: bool,
    /// Non-modal editor for code attached to one concrete timeline cell.
    pub frame_script_editor: Option<FrameScriptEditor>,
    /// Selection and inline rename state for the linked project graph panel.
    pub project_graph_selected: Option<u16>,
    pub project_graph_rename: Option<(u16, String)>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EditInPlaceContext {
    pub parent_q0rg_id: u16,
    pub parent_frame: u16,
    pub layer_id: u16,
    pub placement_idx: usize,
    pub parent_to_stage: Affine,
    pub child_to_stage: Affine,
}

impl Session {
    pub fn history_pivot_state(&self) -> HistoryPivotState {
        HistoryPivotState {
            q0rg_id: self.current_q0rg_id,
            frame: self.current_frame,
            layer_id: self.current_layer_id,
            selection: self.selection.clone(),
            timeline_selection: self.timeline_selection,
            timeline_cell_selection: self.timeline_cell_selection.clone(),
            timeline_layer_selection: self.timeline_layer_selection,
            camera_track_selected: self.camera_track_selected,
            camera_timeline_selection: self.camera_timeline_selection,
            pivot: self
                .transform_pivot
                .as_ref()
                .filter(|pivot| pivot.selection == self.selection)
                .cloned(),
        }
    }

    pub fn restore_history_pivot(&mut self, state: Option<HistoryPivotState>, project: &ProjectV2) {
        let Some(state) = state else {
            self.transform_pivot = None;
            return;
        };
        self.current_q0rg_id = state.q0rg_id;
        self.current_layer_id = state.layer_id;
        self.current_frame = state.frame;
        self.pending_timeline_frame = None;
        self.selection = state.selection;
        self.timeline_selection = state.timeline_selection;
        self.timeline_cell_selection = state.timeline_cell_selection;
        self.timeline_layer_selection = state.timeline_layer_selection;
        self.camera_track_selected = state.camera_track_selected;
        self.camera_timeline_selection = state.camera_timeline_selection;
        self.tool_state = ToolState::Idle;
        self.marquee_base_selection = None;
        self.selection_history_gesture = None;
        self.frame_edit_gesture = None;
        self.frame_edit_selection.clear();
        self.symbol_edit_click = None;
        self.reconcile_with(project);
        self.transform_pivot = state
            .pivot
            .filter(|pivot| pivot.selection == self.selection);
        self.timeline_scroll_to_layer = Some((self.current_q0rg_id, self.current_layer_id));
    }

    pub fn timeline_history_position(&self) -> TimelineHistoryPosition {
        TimelineHistoryPosition {
            q0rg_id: self.current_q0rg_id,
            layer_id: self.current_layer_id,
            frame: self.current_frame,
            camera_track_selected: self.camera_track_selected,
        }
    }

    pub fn restore_timeline_history_position(&mut self, position: TimelineHistoryPosition) {
        // Undo does not enter a different symbol or replace its edit-in-place context.
        if self.current_q0rg_id != position.q0rg_id {
            return;
        }
        self.current_q0rg_id = position.q0rg_id;
        self.current_layer_id = position.layer_id;
        self.current_frame = position.frame;
        self.camera_track_selected = position.camera_track_selected;
        self.pending_timeline_frame = None;
        self.timeline_selection =
            Some(TimelineSelection::single(position.layer_id, position.frame));
        self.timeline_cell_selection = None;
        self.timeline_layer_selection = None;
        self.camera_timeline_selection = None;
        self.timeline_scroll_to_layer = Some((position.q0rg_id, position.layer_id));
        self.timeline_scroll_to_frame = Some((position.q0rg_id, position.frame));
    }

    pub fn reset_q0lang_preview(&mut self) {
        self.preview_q0lang_runtime = q0s_format::q0lang::runtime::Runtime::new();
        self.preview_q0lang_initialized = false;
        self.preview_script_diagnostics.clear();
        self.preview_frame_script_entries = 0;
        self.preview_audio_resync_needed = false;
    }

    pub fn for_project(project: &ProjectV2) -> Self {
        let entry = project.meta.entry_q0rg_id;
        let layer_id = project
            .q0rgs
            .iter()
            .find(|q| q.q0rg_id == entry)
            .and_then(|q| {
                q.layers
                    .iter()
                    .find(|layer| !project.layer_is_folder(entry, layer.layer_id))
                    .or_else(|| q.layers.first())
            })
            .map(|layer| layer.layer_id)
            .unwrap_or(1);
        Self {
            current_q0rg_id: entry,
            current_layer_id: layer_id,
            current_frame: 0,
            pending_timeline_frame: None,
            timeline_selection: None,
            timeline_cell_selection: None,
            outline_mode: false,
            outline_layers: HashSet::new(),
            hidden_mask_art_layers: HashSet::new(),
            timeline_layer_selection: None,
            timeline_scroll_to_layer: None,
            timeline_scroll_to_frame: None,
            camera_track_selected: false,
            camera_timeline_selection: None,
            current_tool: Tool::Select,
            tool_state: ToolState::Idle,
            selection: Selection::None,
            transform_pivot: None,
            marquee_base_selection: None,
            selection_history_gesture: None,
            frame_edit_gesture: None,
            frame_edit_selection: Vec::new(),
            symbol_edit_click: None,
            timeline_horizontal_scroll: 0.0,
            preview_loop_ranges: HashMap::new(),
            show_animation_preview: false,
            animation_preview_frame: 0,
            animation_preview_playing: false,
            animation_preview_tick: Instant::now(),
            breadcrumb: Vec::new(),
            edit_in_place: Vec::new(),
            library_search: String::new(),
            library_sort_ascending: true,
            text_editor: None,
            font_audit: Default::default(),
            text_defaults: Default::default(),
            text_inspector: None,
            text_canvas_click_consumed: false,
            library_rename: None,
            layer_rename: None,
            frame_label_edit: None,
            scene_rename: None,
            timeline_layer_drag: None,
            timeline_frame_drag: None,
            timeline_rig_key_selection: None,
            timeline_rig_key_drag: None,
            camera_frame_drag: None,
            playing: false,
            record_held: false,
            last_held_key: None,
            held_run_keyed: false,
            last_tick: Instant::now(),
            preview_q0lang_runtime: q0s_format::q0lang::runtime::Runtime::new(),
            preview_q0lang_initialized: false,
            preview_script_diagnostics: Vec::new(),
            preview_frame_script_entries: 0,
            preview_audio_resync_needed: false,
            audio_scrubbing: true,
            status: "ready".to_string(),
            stroke_color: Rgba {
                r: 0,
                g: 0,
                b: 0,
                a: 255,
            },
            stroke_width: 1.0,
            fill_color: None,
            brush: BrushSettings::default(),
            brush_mode: BrushMode::Classic,
            advanced_brush: AdvancedBrushSettings::default(),
            brush_size_preview: None,
            advanced_brush_preset_name: "My Brush".to_string(),
            advanced_brush_selected_preset: None,
            advanced_brush_library_filter: BrushLibraryFilter::All,
            eraser_size: 18.0,
            brush_cap: CapShape::Round,
            brush_join: q0s_format::geom::JoinShape::Round,
            rig_mode: RigMode::Simple,
            rig_edit_mode: RigEditMode::Pose,
            physics: Default::default(),
            particles: Default::default(),
            lights: Default::default(),
            sim_preview: Default::default(),
            rig_auto_key: true,
            rig_selected_node: None,
            rig_selected_control: None,
            rig_pose_blend_weight: 0.5,
            rig_pose_blend_mode: RigPoseBlendMode::Override,
            rig_mirror_partner_node: None,
            rig_mirror_partner_control: None,
            rig_variant_add_target: None,
            rig_pending_bone_start: None,
            rig_bone_drag: None,
            rig_control_drag: None,
            show_credits: false,
            credits_opened_at: Instant::now(),
            show_settings: false,
            show_shortcuts: false,
            shortcut_capture: None,
            shortcut_feedback: None,
            easing_editor: None,
            tween_warning: None,
            viewport: Viewport::identity(),
            clipboard: None,
            stage_cursor: None,
            onion: OnionSettings::default(),
            show_q0lang_editor: false,
            q0lang_target: None,
            q0lang_edit_armed: true,
            frame_script_editor: None,
            project_graph_selected: None,
            project_graph_rename: None,
        }
    }

    pub fn timeline_frame(&self) -> u16 {
        self.pending_timeline_frame.unwrap_or(self.current_frame)
    }

    pub fn set_timeline_frame(&mut self, frame: u16, frame_count: u16) {
        if frame < frame_count {
            self.current_frame = frame;
            self.pending_timeline_frame = None;
        } else {
            self.pending_timeline_frame = Some(frame);
        }
    }

    /// Make sure cached selectors still point at valid model state. Called after
    /// undo/redo — the project might have lost q0rgs, layers, placements etc.
    /// Reconcile transient selection across a project-history boundary.
    ///
    /// History stores `ProjectV2` and transform-pivot metadata. Path and placement
    /// references are index-based, so blindly carrying them across undo/redo
    /// can reinterpret an old selection as a different contour (notably a hole)
    /// after topology or target changes. Preserve references when their structural
    /// identity is still the same, even if editable geometry/transform properties
    /// changed, and drop them when the backing model object is no longer compatible.
    pub fn reconcile_after_history(&mut self, from: &ProjectV2, to: &ProjectV2) {
        self.selection = history_stable_selection(from, to, self.selection.clone());
        self.reconcile_with(to);
    }

    pub fn reconcile_with(&mut self, project: &ProjectV2) {
        let previous_q0rg_id = self.current_q0rg_id;
        self.breadcrumb
            .retain(|id| project.q0rgs.iter().any(|q0rg| q0rg.q0rg_id == *id));
        if !project
            .q0rgs
            .iter()
            .any(|q| q.q0rg_id == self.current_q0rg_id)
        {
            self.current_q0rg_id = project.meta.entry_q0rg_id;
            self.breadcrumb.clear();
        }
        if let Some(q) = project
            .q0rgs
            .iter()
            .find(|q| q.q0rg_id == self.current_q0rg_id)
        {
            if !q
                .layers
                .iter()
                .any(|layer| layer.layer_id == self.current_layer_id)
            {
                self.current_layer_id = q
                    .layers
                    .iter()
                    .find(|layer| !project.layer_is_folder(q.q0rg_id, layer.layer_id))
                    .or_else(|| q.layers.first())
                    .map(|layer| layer.layer_id)
                    .unwrap_or(1);
            }
            self.current_frame = self.current_frame.min(q.frame_count.saturating_sub(1));
            if previous_q0rg_id != self.current_q0rg_id {
                self.pending_timeline_frame = None;
                self.timeline_selection = None;
                self.timeline_cell_selection = None;
                self.timeline_layer_selection = None;
                self.camera_track_selected = false;
                self.camera_timeline_selection = None;
                self.camera_frame_drag = None;
                self.timeline_frame_drag = None;
                self.reset_q0lang_preview();
            } else if let Some(selection) = self.timeline_selection {
                let layers_exist = q
                    .layers
                    .iter()
                    .any(|layer| layer.layer_id == selection.anchor_layer_id)
                    && q.layers
                        .iter()
                        .any(|layer| layer.layer_id == selection.focus_layer_id);
                let selectable_last = self
                    .pending_timeline_frame
                    .unwrap_or_else(|| q.frame_count.saturating_sub(1))
                    .max(q.frame_count.saturating_sub(1));
                let frames_exist = selection.anchor_frame <= selectable_last
                    && selection.focus_frame <= selectable_last;
                if !layers_exist || !frames_exist {
                    self.timeline_selection = None;
                }
            }
            let mut clear_cell_selection = false;
            if let Some((selected_q0rg_id, cells)) = &mut self.timeline_cell_selection {
                if *selected_q0rg_id != self.current_q0rg_id {
                    clear_cell_selection = true;
                } else {
                    let selectable_last = self
                        .pending_timeline_frame
                        .unwrap_or_else(|| q.frame_count.saturating_sub(1))
                        .max(q.frame_count.saturating_sub(1));
                    cells.retain(|(layer_id, frame)| {
                        *frame <= selectable_last
                            && q.layers.iter().any(|layer| {
                                layer.layer_id == *layer_id
                                    && !project.layer_is_folder(q.q0rg_id, *layer_id)
                            })
                    });
                    clear_cell_selection = cells.is_empty();
                }
            }
            if clear_cell_selection {
                self.timeline_cell_selection = None;
            }
        } else {
            self.timeline_selection = None;
            self.timeline_cell_selection = None;
            self.pending_timeline_frame = None;
            self.camera_track_selected = false;
        }
        if self.current_q0rg_id != project.meta.entry_q0rg_id || !project.runtime.camera.enabled {
            self.camera_track_selected = false;
            self.camera_timeline_selection = None;
            self.camera_frame_drag = None;
        }
        if self.timeline_layer_selection.is_some_and(|selection| {
            !project
                .q0rgs
                .iter()
                .find(|q0rg| q0rg.q0rg_id == self.current_q0rg_id)
                .is_some_and(|q0rg| {
                    q0rg.layers
                        .iter()
                        .any(|layer| layer.layer_id == selection.anchor_layer_id)
                        && q0rg
                            .layers
                            .iter()
                            .any(|layer| layer.layer_id == selection.focus_layer_id)
                })
        }) {
            self.timeline_layer_selection = None;
        }
        if self.timeline_frame_drag.is_some_and(|drag| {
            drag.q0rg_id != self.current_q0rg_id
                || !project
                    .q0rgs
                    .iter()
                    .find(|q0rg| q0rg.q0rg_id == drag.q0rg_id)
                    .is_some_and(|q0rg| {
                        q0rg.layers
                            .iter()
                            .any(|layer| layer.layer_id == drag.selection.anchor_layer_id)
                            && q0rg
                                .layers
                                .iter()
                                .any(|layer| layer.layer_id == drag.selection.focus_layer_id)
                    })
        }) {
            self.timeline_frame_drag = None;
        }
        if self.timeline_layer_drag.is_some_and(|drag| {
            drag.q0rg_id != self.current_q0rg_id
                || !project
                    .q0rgs
                    .iter()
                    .find(|q0rg| q0rg.q0rg_id == drag.q0rg_id)
                    .is_some_and(|q0rg| {
                        q0rg.layers
                            .iter()
                            .any(|layer| layer.layer_id == drag.selection.anchor_layer_id)
                            && q0rg
                                .layers
                                .iter()
                                .any(|layer| layer.layer_id == drag.selection.focus_layer_id)
                    })
        }) {
            self.timeline_layer_drag = None;
        }
        match self.selection {
            Selection::Asset(id) => {
                if !project.assets.iter().any(|a| a.id() == id) {
                    self.selection = Selection::None;
                }
            }
            Selection::Q0rg(id) => {
                if !project.q0rgs.iter().any(|q| q.q0rg_id == id) {
                    self.selection = Selection::None;
                }
            }
            Selection::Placement {
                q0rg_id,
                layer_id,
                placement_idx,
            } => {
                let exists = project
                    .q0rgs
                    .iter()
                    .find(|q| q.q0rg_id == q0rg_id)
                    .and_then(|q| q.layers.iter().find(|l| l.layer_id == layer_id))
                    .and_then(|l| l.placements.get(placement_idx))
                    .is_some();
                if !exists {
                    self.selection = Selection::None;
                }
            }
            Selection::Path {
                q0rg_id,
                layer_id,
                placement_idx,
                path_idx,
            } => {
                let exists = project
                    .q0rgs
                    .iter()
                    .find(|q| q.q0rg_id == q0rg_id)
                    .and_then(|q| q.layers.iter().find(|l| l.layer_id == layer_id))
                    .and_then(|l| l.placements.get(placement_idx))
                    .and_then(|placement| match placement.target {
                        q0s_format::v2::Target::Asset(asset_id) => {
                            project.assets.iter().find(|asset| asset.id() == asset_id)
                        }
                        q0s_format::v2::Target::Q0rg(_) => None,
                    })
                    .and_then(|asset| match asset {
                        q0s_format::v2::Asset::Vector(vector) => vector.paths.get(path_idx),
                        q0s_format::v2::Asset::Bitmap(_)
                        | q0s_format::v2::Asset::Q0v(_)
                        | q0s_format::v2::Asset::Rig(_) => None,
                    })
                    .is_some();
                if !exists {
                    self.selection = Selection::None;
                }
            }
            Selection::Paths(ref refs) => {
                let kept: Vec<PathRef> = refs
                    .iter()
                    .copied()
                    .filter(|r| path_ref_exists(project, *r))
                    .collect();
                self.selection = collapse_path_refs(kept);
            }
            Selection::RawParts {
                ref mut fills,
                ref mut strokes,
                ref mut stroke_segments,
                ref mut objects,
            } => {
                fills.retain(|reference| path_ref_exists(project, *reference));
                strokes.retain(|reference| path_ref_exists(project, *reference));
                stroke_segments.retain(|segment| {
                    path_segment_count(project, segment.path)
                        .is_some_and(|count| segment.segment_idx < count)
                });
                objects.retain(|reference| placement_ref_exists(project, *reference));
                if fills.is_empty()
                    && strokes.is_empty()
                    && stroke_segments.is_empty()
                    && objects.is_empty()
                {
                    self.selection = Selection::None;
                }
            }
            Selection::StrokeSegment(segment) => {
                if path_segment_count(project, segment.path)
                    .is_none_or(|count| segment.segment_idx >= count)
                {
                    self.selection = Selection::None;
                }
            }
            Selection::PathPoints {
                path,
                ref mut anchor_indices,
                ..
            } => {
                if !path_ref_exists(project, path) {
                    self.selection = Selection::None;
                } else if let Some(anchor_count) = path_anchor_count(project, path) {
                    anchor_indices.retain(|index| *index < anchor_count);
                    if anchor_indices.is_empty() {
                        self.selection = Selection::None;
                    }
                }
            }
            Selection::SubselectPoints {
                path,
                ref mut anchor_indices,
                ..
            } => {
                if !path_ref_exists(project, path) {
                    self.selection = Selection::None;
                } else if let Some(anchor_count) = path_anchor_count(project, path) {
                    // An empty Subselect point list intentionally means "show all
                    // authored anchors, none selected"; unlike partial V-selection,
                    // it remains a valid active contour.
                    anchor_indices.retain(|index| *index < anchor_count);
                }
            }
            Selection::RawArea {
                ref mut placements,
                ref mut objects,
                ..
            } => {
                let exists = |r: &PlacementRef| {
                    project
                        .q0rgs
                        .iter()
                        .find(|q| q.q0rg_id == r.q0rg_id)
                        .and_then(|q| q.layers.iter().find(|layer| layer.layer_id == r.layer_id))
                        .and_then(|layer| layer.placements.get(r.placement_idx))
                        .is_some()
                };
                placements.retain(&exists);
                objects.retain(exists);
                if placements.is_empty() && objects.is_empty() {
                    self.selection = Selection::None;
                }
            }
            Selection::Mixed {
                ref paths,
                ref objects,
            } => {
                let kept_paths: Vec<PathRef> = paths
                    .iter()
                    .copied()
                    .filter(|reference| path_ref_exists(project, *reference))
                    .collect();
                let kept_objects: Vec<PlacementRef> = objects
                    .iter()
                    .copied()
                    .filter(|reference| placement_ref_exists(project, *reference))
                    .collect();
                self.selection = collapse_mixed_selection(kept_paths, kept_objects);
            }
            Selection::Multi(ref refs) => {
                let kept: Vec<PlacementRef> = refs
                    .iter()
                    .copied()
                    .filter(|reference| placement_ref_exists(project, *reference))
                    .collect();
                self.selection = match kept.len() {
                    0 => Selection::None,
                    1 => Selection::Placement {
                        q0rg_id: kept[0].q0rg_id,
                        layer_id: kept[0].layer_id,
                        placement_idx: kept[0].placement_idx,
                    },
                    _ => Selection::Multi(kept),
                };
            }
            Selection::None => {}
        }
        self.tool_state = ToolState::Idle;
        self.rig_pending_bone_start = None;
        self.rig_bone_drag = None;
        self.rig_control_drag = None;
        if let Some(rig) = q0s_format::rig::rig_for_q0rg(project, self.current_q0rg_id) {
            if self
                .rig_selected_node
                .is_some_and(|id| !rig.nodes.iter().any(|node| node.node_id == id))
            {
                self.rig_selected_node = None;
            }
            if self
                .rig_selected_control
                .is_some_and(|id| !rig.controls.iter().any(|control| control.control_id == id))
            {
                self.rig_selected_control = None;
            }
        } else {
            self.rig_selected_node = None;
            self.rig_selected_control = None;
        }
        self.playing = false;
        // If the script editor is bound to a now-gone q0rg, clear the
        // pin and let the next open snap to `current_q0rg_id`.
        if let Some(id) = self.q0lang_target {
            if !project.q0rgs.iter().any(|q| q.q0rg_id == id) {
                self.q0lang_target = None;
            }
        }
        self.clear_inactive_frame_selection(project);
    }

    pub fn clear_inactive_frame_selection(&mut self, project: &ProjectV2) {
        if self.onion.edit_frames {
            crate::frame_edit::refresh_session(self, project);
            return;
        }
        let frame = self.current_frame;
        let current_q0rg_id = self.current_q0rg_id;
        let placement_on_current_frame = |r: PlacementRef| {
            r.q0rg_id == current_q0rg_id
                && project
                    .q0rgs
                    .iter()
                    .find(|q| q.q0rg_id == r.q0rg_id)
                    .and_then(|q| q.layers.iter().find(|l| l.layer_id == r.layer_id))
                    .map(|layer| {
                        crate::render::placement_is_active_at(layer, r.placement_idx, frame)
                    })
                    .unwrap_or(false)
        };

        let selection_is_already_valid = match &self.selection {
            Selection::Placement {
                q0rg_id,
                layer_id,
                placement_idx,
            } => placement_on_current_frame(PlacementRef {
                q0rg_id: *q0rg_id,
                layer_id: *layer_id,
                placement_idx: *placement_idx,
            }),
            Selection::Path {
                q0rg_id,
                layer_id,
                placement_idx,
                path_idx,
            } => {
                let path = PathRef {
                    q0rg_id: *q0rg_id,
                    layer_id: *layer_id,
                    placement_idx: *placement_idx,
                    path_idx: *path_idx,
                };
                placement_on_current_frame(PlacementRef {
                    q0rg_id: *q0rg_id,
                    layer_id: *layer_id,
                    placement_idx: *placement_idx,
                }) && path_ref_exists(project, path)
            }
            Selection::Paths(refs) => {
                !refs.is_empty()
                    && refs.iter().all(|r| {
                        placement_on_current_frame(PlacementRef {
                            q0rg_id: r.q0rg_id,
                            layer_id: r.layer_id,
                            placement_idx: r.placement_idx,
                        }) && path_ref_exists(project, *r)
                    })
            }
            Selection::PathPoints { path, .. } | Selection::SubselectPoints { path, .. } => {
                placement_on_current_frame(PlacementRef {
                    q0rg_id: path.q0rg_id,
                    layer_id: path.layer_id,
                    placement_idx: path.placement_idx,
                }) && path_ref_exists(project, *path)
            }
            Selection::RawArea {
                placements,
                objects,
                ..
            } => {
                // RawArea with no raw surfaces intentionally normalises to
                // Placement/Multi below; otherwise an unchanged area can stay put.
                !placements.is_empty()
                    && placements
                        .iter()
                        .chain(objects)
                        .all(|placement| placement_on_current_frame(*placement))
            }
            Selection::Multi(refs) => {
                refs.len() >= 2
                    && refs
                        .iter()
                        .all(|placement| placement_on_current_frame(*placement))
            }
            // These variants were not frame-normalised by the old match either.
            // Returning here preserves that behaviour without cloning their payloads.
            Selection::None
            | Selection::Asset(_)
            | Selection::Q0rg(_)
            | Selection::RawParts { .. }
            | Selection::StrokeSegment(_)
            | Selection::Mixed { .. } => true,
        };
        if selection_is_already_valid {
            return;
        }

        let next = match self.selection.clone() {
            Selection::Placement {
                q0rg_id,
                layer_id,
                placement_idx,
            } => {
                let r = PlacementRef {
                    q0rg_id,
                    layer_id,
                    placement_idx,
                };
                if placement_on_current_frame(r) {
                    Selection::Placement {
                        q0rg_id,
                        layer_id,
                        placement_idx,
                    }
                } else {
                    Selection::None
                }
            }
            Selection::Path {
                q0rg_id,
                layer_id,
                placement_idx,
                path_idx,
            } => {
                let r = PlacementRef {
                    q0rg_id,
                    layer_id,
                    placement_idx,
                };
                if placement_on_current_frame(r) {
                    Selection::Path {
                        q0rg_id,
                        layer_id,
                        placement_idx,
                        path_idx,
                    }
                } else {
                    Selection::None
                }
            }
            Selection::Paths(refs) => {
                let kept: Vec<PathRef> = refs
                    .into_iter()
                    .filter(|r| {
                        placement_on_current_frame(PlacementRef {
                            q0rg_id: r.q0rg_id,
                            layer_id: r.layer_id,
                            placement_idx: r.placement_idx,
                        }) && path_ref_exists(project, *r)
                    })
                    .collect();
                collapse_path_refs(kept)
            }
            Selection::PathPoints {
                path,
                anchor_indices,
                bounds_min,
                bounds_max,
            } => {
                let placement = PlacementRef {
                    q0rg_id: path.q0rg_id,
                    layer_id: path.layer_id,
                    placement_idx: path.placement_idx,
                };
                if placement_on_current_frame(placement) && path_ref_exists(project, path) {
                    Selection::PathPoints {
                        path,
                        anchor_indices,
                        bounds_min,
                        bounds_max,
                    }
                } else {
                    Selection::None
                }
            }
            Selection::SubselectPoints {
                path,
                anchor_indices,
                bounds_min,
                bounds_max,
            } => {
                let placement = PlacementRef {
                    q0rg_id: path.q0rg_id,
                    layer_id: path.layer_id,
                    placement_idx: path.placement_idx,
                };
                if placement_on_current_frame(placement) && path_ref_exists(project, path) {
                    Selection::SubselectPoints {
                        path,
                        anchor_indices,
                        bounds_min,
                        bounds_max,
                    }
                } else {
                    Selection::None
                }
            }
            Selection::RawArea {
                placements,
                objects,
                bounds_min,
                bounds_max,
                clip_region,
            } => {
                let kept_raw: Vec<PlacementRef> = placements
                    .into_iter()
                    .filter(|placement| placement_on_current_frame(*placement))
                    .collect();
                let kept_objects: Vec<PlacementRef> = objects
                    .into_iter()
                    .filter(|placement| placement_on_current_frame(*placement))
                    .collect();
                if kept_raw.is_empty() && kept_objects.is_empty() {
                    Selection::None
                } else if kept_raw.is_empty() {
                    match kept_objects.len() {
                        1 => Selection::Placement {
                            q0rg_id: kept_objects[0].q0rg_id,
                            layer_id: kept_objects[0].layer_id,
                            placement_idx: kept_objects[0].placement_idx,
                        },
                        _ => Selection::Multi(kept_objects),
                    }
                } else {
                    Selection::RawArea {
                        placements: kept_raw,
                        objects: kept_objects,
                        bounds_min,
                        bounds_max,
                        clip_region,
                    }
                }
            }
            Selection::Multi(refs) => {
                let kept: Vec<PlacementRef> = refs
                    .into_iter()
                    .filter(|r| placement_on_current_frame(*r))
                    .collect();
                match kept.len() {
                    0 => Selection::None,
                    1 => Selection::Placement {
                        q0rg_id: kept[0].q0rg_id,
                        layer_id: kept[0].layer_id,
                        placement_idx: kept[0].placement_idx,
                    },
                    _ => Selection::Multi(kept),
                }
            }
            other => other,
        };

        if next != self.selection {
            self.selection = next;
            self.tool_state = ToolState::Idle;
        }
    }
}

fn placement_at(project: &ProjectV2, reference: PlacementRef) -> Option<&Placement> {
    project
        .q0rgs
        .iter()
        .find(|q0rg| q0rg.q0rg_id == reference.q0rg_id)
        .and_then(|q0rg| {
            q0rg.layers
                .iter()
                .find(|layer| layer.layer_id == reference.layer_id)
        })
        .and_then(|layer| layer.placements.get(reference.placement_idx))
}

fn path_at(project: &ProjectV2, reference: PathRef) -> Option<&VPath> {
    let placement = placement_at(
        project,
        PlacementRef {
            q0rg_id: reference.q0rg_id,
            layer_id: reference.layer_id,
            placement_idx: reference.placement_idx,
        },
    )?;
    let q0s_format::v2::Target::Asset(asset_id) = placement.target else {
        return None;
    };
    let asset = project.assets.iter().find(|asset| asset.id() == asset_id)?;
    let q0s_format::v2::Asset::Vector(vector) = asset else {
        return None;
    };
    vector.paths.get(reference.path_idx)
}

fn stable_placement_ref(from: &ProjectV2, to: &ProjectV2, reference: PlacementRef) -> bool {
    let (Some(before), Some(after)) = (placement_at(from, reference), placement_at(to, reference))
    else {
        return false;
    };
    if before == after {
        return true;
    }
    // Transform, tween and FX are mutable properties of one placement, not its
    // identity. Undoing those edits should keep the object selected. A changed
    // frame/target, however, means this index can now refer to a different model
    // object (for example after Break Apart), so carrying selection would be unsafe.
    before.frame == after.frame && before.target == after.target
}

fn stable_vector_path_structure(from: &ProjectV2, to: &ProjectV2, reference: PathRef) -> bool {
    let placement = PlacementRef {
        q0rg_id: reference.q0rg_id,
        layer_id: reference.layer_id,
        placement_idx: reference.placement_idx,
    };
    let (Some(before_placement), Some(after_placement)) =
        (placement_at(from, placement), placement_at(to, placement))
    else {
        return false;
    };
    let (
        q0s_format::v2::Target::Asset(before_asset_id),
        q0s_format::v2::Target::Asset(after_asset_id),
    ) = (before_placement.target, after_placement.target)
    else {
        return false;
    };
    if before_asset_id != after_asset_id {
        return false;
    }
    let before = from.assets.iter().find_map(|asset| match asset {
        q0s_format::v2::Asset::Vector(vector) if vector.asset_id == before_asset_id => Some(vector),
        _ => None,
    });
    let after = to.assets.iter().find_map(|asset| match asset {
        q0s_format::v2::Asset::Vector(vector) if vector.asset_id == after_asset_id => Some(vector),
        _ => None,
    });
    let (Some(before), Some(after)) = (before, after) else {
        return false;
    };
    if reference.path_idx >= before.paths.len() || before.paths.len() != after.paths.len() {
        return false;
    }
    before.paths.iter().zip(&after.paths).all(|(left, right)| {
        left.closed == right.closed
            && left.anchors.len() == right.anchors.len()
            && left.anchors.iter().zip(&right.anchors).all(|(a, b)| {
                a.in_handle.is_some() == b.in_handle.is_some()
                    && a.out_handle.is_some() == b.out_handle.is_some()
            })
    })
}

fn stable_path_ref(from: &ProjectV2, to: &ProjectV2, reference: PathRef) -> bool {
    let placement = PlacementRef {
        q0rg_id: reference.q0rg_id,
        layer_id: reference.layer_id,
        placement_idx: reference.placement_idx,
    };
    if !stable_placement_ref(from, to, placement) {
        return false;
    }
    matches!(
        (path_at(from, reference), path_at(to, reference)),
        (Some(before), Some(after)) if before == after
    ) || stable_vector_path_structure(from, to, reference)
}

fn history_stable_selection(from: &ProjectV2, to: &ProjectV2, selection: Selection) -> Selection {
    match selection {
        Selection::Placement {
            q0rg_id,
            layer_id,
            placement_idx,
        } => {
            let reference = PlacementRef {
                q0rg_id,
                layer_id,
                placement_idx,
            };
            if stable_placement_ref(from, to, reference) {
                Selection::Placement {
                    q0rg_id,
                    layer_id,
                    placement_idx,
                }
            } else {
                Selection::None
            }
        }
        Selection::Path {
            q0rg_id,
            layer_id,
            placement_idx,
            path_idx,
        } => {
            let reference = PathRef {
                q0rg_id,
                layer_id,
                placement_idx,
                path_idx,
            };
            if stable_path_ref(from, to, reference) {
                Selection::Path {
                    q0rg_id,
                    layer_id,
                    placement_idx,
                    path_idx,
                }
            } else {
                Selection::None
            }
        }
        Selection::Paths(refs) => collapse_path_refs(
            refs.into_iter()
                .filter(|reference| stable_path_ref(from, to, *reference))
                .collect(),
        ),
        Selection::PathPoints {
            path,
            anchor_indices,
            bounds_min,
            bounds_max,
        } => {
            if stable_path_ref(from, to, path) {
                Selection::PathPoints {
                    path,
                    anchor_indices,
                    bounds_min,
                    bounds_max,
                }
            } else {
                Selection::None
            }
        }
        Selection::SubselectPoints {
            path,
            anchor_indices,
            bounds_min,
            bounds_max,
        } => {
            if stable_path_ref(from, to, path) {
                Selection::SubselectPoints {
                    path,
                    anchor_indices,
                    bounds_min,
                    bounds_max,
                }
            } else {
                Selection::None
            }
        }
        Selection::RawArea {
            placements,
            objects,
            bounds_min,
            bounds_max,
            clip_region,
        } => {
            let stable_raw: Vec<PlacementRef> = placements
                .iter()
                .copied()
                .filter(|reference| stable_placement_ref(from, to, *reference))
                .collect();
            let stable_objects: Vec<PlacementRef> = objects
                .iter()
                .copied()
                .filter(|reference| stable_placement_ref(from, to, *reference))
                .collect();
            if stable_raw.len() == placements.len() && stable_objects.len() == objects.len() {
                Selection::RawArea {
                    placements: stable_raw,
                    objects: stable_objects,
                    bounds_min,
                    bounds_max,
                    clip_region,
                }
            } else {
                Selection::None
            }
        }
        Selection::Mixed { paths, objects } => {
            let stable_paths: Vec<PathRef> = paths
                .into_iter()
                .filter(|reference| stable_path_ref(from, to, *reference))
                .collect();
            let stable_objects: Vec<PlacementRef> = objects
                .into_iter()
                .filter(|reference| stable_placement_ref(from, to, *reference))
                .collect();
            collapse_mixed_selection(stable_paths, stable_objects)
        }
        Selection::Multi(refs) => {
            let stable: Vec<PlacementRef> = refs
                .into_iter()
                .filter(|reference| stable_placement_ref(from, to, *reference))
                .collect();
            match stable.len() {
                0 => Selection::None,
                1 => Selection::Placement {
                    q0rg_id: stable[0].q0rg_id,
                    layer_id: stable[0].layer_id,
                    placement_idx: stable[0].placement_idx,
                },
                _ => Selection::Multi(stable),
            }
        }
        other => other,
    }
}

fn path_ref_exists(project: &ProjectV2, r: PathRef) -> bool {
    project
        .q0rgs
        .iter()
        .find(|q| q.q0rg_id == r.q0rg_id)
        .and_then(|q| q.layers.iter().find(|layer| layer.layer_id == r.layer_id))
        .and_then(|layer| layer.placements.get(r.placement_idx))
        .and_then(|placement| match placement.target {
            q0s_format::v2::Target::Asset(asset_id) => {
                project.assets.iter().find(|asset| asset.id() == asset_id)
            }
            q0s_format::v2::Target::Q0rg(_) => None,
        })
        .and_then(|asset| match asset {
            q0s_format::v2::Asset::Vector(vector) => vector.paths.get(r.path_idx),
            q0s_format::v2::Asset::Bitmap(_)
            | q0s_format::v2::Asset::Q0v(_)
            | q0s_format::v2::Asset::Rig(_) => None,
        })
        .is_some()
}

fn path_anchor_count(project: &ProjectV2, r: PathRef) -> Option<usize> {
    project
        .q0rgs
        .iter()
        .find(|q| q.q0rg_id == r.q0rg_id)
        .and_then(|q| q.layers.iter().find(|layer| layer.layer_id == r.layer_id))
        .and_then(|layer| layer.placements.get(r.placement_idx))
        .and_then(|placement| match placement.target {
            q0s_format::v2::Target::Asset(asset_id) => {
                project.assets.iter().find(|asset| asset.id() == asset_id)
            }
            q0s_format::v2::Target::Q0rg(_) => None,
        })
        .and_then(|asset| match asset {
            q0s_format::v2::Asset::Vector(vector) => vector.paths.get(r.path_idx),
            q0s_format::v2::Asset::Bitmap(_)
            | q0s_format::v2::Asset::Q0v(_)
            | q0s_format::v2::Asset::Rig(_) => None,
        })
        .map(|path| path.anchors.len())
}

fn path_segment_count(project: &ProjectV2, r: PathRef) -> Option<usize> {
    project
        .q0rgs
        .iter()
        .find(|q| q.q0rg_id == r.q0rg_id)
        .and_then(|q| q.layers.iter().find(|layer| layer.layer_id == r.layer_id))
        .and_then(|layer| layer.placements.get(r.placement_idx))
        .and_then(|placement| match placement.target {
            q0s_format::v2::Target::Asset(asset_id) => {
                project.assets.iter().find(|asset| asset.id() == asset_id)
            }
            q0s_format::v2::Target::Q0rg(_) => None,
        })
        .and_then(|asset| match asset {
            q0s_format::v2::Asset::Vector(vector) => vector.paths.get(r.path_idx),
            q0s_format::v2::Asset::Bitmap(_)
            | q0s_format::v2::Asset::Q0v(_)
            | q0s_format::v2::Asset::Rig(_) => None,
        })
        .map(|path| {
            if path.anchors.len() < 2 {
                0
            } else if path.closed {
                path.anchors.len()
            } else {
                path.anchors.len() - 1
            }
        })
}

fn placement_ref_exists(project: &ProjectV2, reference: PlacementRef) -> bool {
    project
        .q0rgs
        .iter()
        .find(|q0rg| q0rg.q0rg_id == reference.q0rg_id)
        .and_then(|q0rg| {
            q0rg.layers
                .iter()
                .find(|layer| layer.layer_id == reference.layer_id)
        })
        .and_then(|layer| layer.placements.get(reference.placement_idx))
        .is_some()
}

fn collapse_path_refs(refs: Vec<PathRef>) -> Selection {
    match refs.len() {
        0 => Selection::None,
        1 => Selection::Path {
            q0rg_id: refs[0].q0rg_id,
            layer_id: refs[0].layer_id,
            placement_idx: refs[0].placement_idx,
            path_idx: refs[0].path_idx,
        },
        _ => Selection::Paths(refs),
    }
}

fn collapse_mixed_selection(paths: Vec<PathRef>, objects: Vec<PlacementRef>) -> Selection {
    if !paths.is_empty() && !objects.is_empty() {
        return Selection::Mixed { paths, objects };
    }
    if !paths.is_empty() {
        return collapse_path_refs(paths);
    }
    match objects.len() {
        0 => Selection::None,
        1 => Selection::Placement {
            q0rg_id: objects[0].q0rg_id,
            layer_id: objects[0].layer_id,
            placement_idx: objects[0].placement_idx,
        },
        _ => Selection::Multi(objects),
    }
}

/// Snapshot-based undo/redo stack. Each snapshot is a deep clone of the project,
/// which is acceptable for the current scale (vector shapes are tiny). Capped to
/// avoid unbounded memory growth on long sessions.
const HISTORY_CAP: usize = 64;

#[derive(Clone, Copy)]
pub struct TimelineHistoryPosition {
    pub q0rg_id: u16,
    pub layer_id: u16,
    pub frame: u16,
    pub camera_track_selected: bool,
}

#[derive(Clone, PartialEq)]
pub struct HistoryPivotState {
    pub(crate) q0rg_id: u16,
    frame: u16,
    layer_id: u16,
    selection: Selection,
    timeline_selection: Option<TimelineSelection>,
    timeline_cell_selection: Option<(u16, Vec<(u16, u16)>)>,
    timeline_layer_selection: Option<TimelineLayerSelection>,
    camera_track_selected: bool,
    camera_timeline_selection: Option<CameraTimelineSelection>,
    pivot: Option<TransformPivot>,
}

impl HistoryPivotState {
    pub(crate) fn same_selection(&self, other: &Self) -> bool {
        self.selection == other.selection
            && self.timeline_selection == other.timeline_selection
            && self.timeline_cell_selection == other.timeline_cell_selection
            && self.timeline_layer_selection == other.timeline_layer_selection
            && self.camera_track_selected == other.camera_track_selected
            && self.camera_timeline_selection == other.camera_timeline_selection
            && self.pivot == other.pivot
    }
}

struct HistoryEntry {
    // Selection-only entries do not copy or modify project artwork.
    project: Option<ProjectV2>,
    timeline_position: Option<TimelineHistoryPosition>,
    pivot_state: Option<HistoryPivotState>,
}

pub struct History {
    undo: Vec<HistoryEntry>,
    redo: Vec<HistoryEntry>,
    snapshot_revision: u64,
}

impl History {
    pub fn new() -> Self {
        Self {
            undo: Vec::new(),
            redo: Vec::new(),
            snapshot_revision: 0,
        }
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn snapshot_revision(&self) -> u64 {
        self.snapshot_revision
    }

    pub fn undo_timeline_position(&self) -> Option<TimelineHistoryPosition> {
        self.undo.last().and_then(|entry| entry.timeline_position)
    }

    pub fn redo_timeline_position(&self) -> Option<TimelineHistoryPosition> {
        self.redo.last().and_then(|entry| entry.timeline_position)
    }

    pub fn undo_pivot_state(&self) -> Option<HistoryPivotState> {
        self.undo.last().and_then(|entry| entry.pivot_state.clone())
    }

    pub fn redo_pivot_state(&self) -> Option<HistoryPivotState> {
        self.redo.last().and_then(|entry| entry.pivot_state.clone())
    }

    pub fn set_undo_pivot_state(&mut self, state: HistoryPivotState) {
        if let Some(entry) = self.undo.last_mut() {
            entry.pivot_state = Some(state);
        }
    }

    pub fn set_redo_pivot_state(&mut self, state: HistoryPivotState) {
        if let Some(entry) = self.redo.last_mut() {
            entry.pivot_state = Some(state);
        }
    }

    pub fn set_undo_timeline_position(&mut self, position: TimelineHistoryPosition) {
        if let Some(entry) = self.undo.last_mut() {
            entry.timeline_position = Some(position);
        }
    }

    pub fn set_redo_timeline_position(&mut self, position: TimelineHistoryPosition) {
        if let Some(entry) = self.redo.last_mut() {
            entry.timeline_position = Some(position);
        }
    }

    pub fn undo_changes_project(&self) -> bool {
        self.undo
            .last()
            .is_some_and(|entry| entry.project.is_some())
    }

    pub fn redo_changes_project(&self) -> bool {
        self.redo
            .last()
            .is_some_and(|entry| entry.project.is_some())
    }

    pub fn snapshot_selection(&mut self, state: HistoryPivotState) {
        self.undo.push(HistoryEntry {
            project: None,
            timeline_position: None,
            pivot_state: Some(state),
        });
        self.snapshot_revision = self.snapshot_revision.wrapping_add(1);
        if self.undo.len() > HISTORY_CAP {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    /// Record the project's CURRENT state as the "before" of the upcoming change.
    /// Clears any redo branch (standard undo semantics). Skips pushing if the
    /// current state already matches the top of the stack — this lets callers
    /// safely snapshot on every `.changed()` from a DragValue without bloating
    /// the stack with duplicates of the same intermediate value.
    pub fn snapshot(&mut self, current: &ProjectV2) {
        self.snapshot_inner(current, None);
    }

    pub fn snapshot_with_session(&mut self, current: &ProjectV2, session: &Session) {
        self.snapshot_inner(current, Some(session.history_pivot_state()));
    }

    fn snapshot_inner(&mut self, current: &ProjectV2, pivot_state: Option<HistoryPivotState>) {
        if self.undo.last().is_some_and(|entry| {
            entry.project.as_ref() == Some(current) && entry.pivot_state == pivot_state
        }) {
            return;
        }
        self.undo.push(HistoryEntry {
            project: Some(current.clone()),
            timeline_position: None,
            pivot_state,
        });
        self.snapshot_revision = self.snapshot_revision.wrapping_add(1);
        if self.undo.len() > HISTORY_CAP {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    /// Pop the latest "before"; move `current` onto the redo stack.
    #[allow(clippy::result_large_err)]
    pub fn pop_undo(
        &mut self,
        current: ProjectV2,
        reconcile: impl FnOnce(&ProjectV2, &ProjectV2),
    ) -> Result<ProjectV2, ProjectV2> {
        let Some(prior) = self.undo.pop() else {
            return Err(current);
        };
        self.snapshot_revision = self.snapshot_revision.wrapping_add(1);
        let (restored, redo_project) = if let Some(project) = prior.project {
            reconcile(&current, &project);
            (project, Some(current))
        } else {
            reconcile(&current, &current);
            (current, None)
        };
        self.redo.push(HistoryEntry {
            project: redo_project,
            timeline_position: None,
            pivot_state: None,
        });
        Ok(restored)
    }

    /// Pop the latest "after" off redo; move `current` back onto undo.
    #[allow(clippy::result_large_err)]
    pub fn pop_redo(
        &mut self,
        current: ProjectV2,
        reconcile: impl FnOnce(&ProjectV2, &ProjectV2),
    ) -> Result<ProjectV2, ProjectV2> {
        let Some(next) = self.redo.pop() else {
            return Err(current);
        };
        self.snapshot_revision = self.snapshot_revision.wrapping_add(1);
        let (restored, undo_project) = if let Some(project) = next.project {
            reconcile(&current, &project);
            (project, Some(current))
        } else {
            reconcile(&current, &current);
            (current, None)
        };
        self.undo.push(HistoryEntry {
            project: undo_project,
            timeline_position: None,
            pivot_state: None,
        });
        Ok(restored)
    }
}

impl Default for History {
    fn default() -> Self {
        Self::new()
    }
}

pub const DEFAULT_STAGE_WIDTH: u16 = 640;
pub const DEFAULT_STAGE_HEIGHT: u16 = 480;

pub fn default_project() -> ProjectV2 {
    ProjectV2 {
        meta: ProjectMeta {
            name: "untitled".to_string(),
            fps: 24,
            stage_width: DEFAULT_STAGE_WIDTH,
            stage_height: DEFAULT_STAGE_HEIGHT,
            background_color: q0s_format::v2::Rgba {
                r: 255,
                g: 255,
                b: 255,
                a: 255,
            },
            entry_q0rg_id: 1,
        },
        assets: Vec::new(),
        asset_names: std::collections::HashMap::new(),
        asset_appearances: std::collections::HashMap::new(),
        layer_metadata: std::collections::HashMap::new(),
        audio_clips: Vec::new(),
        runtime: q0s_format::v2::ProjectRuntimeData {
            scenes: vec![ProjectScene {
                scene_id: 1,
                q0rg_id: 1,
                name: "Stage".to_string(),
            }],
            ..Default::default()
        },
        q0rgs: vec![Q0rg {
            q0rg_id: 1,
            name: "Stage".to_string(),
            frame_count: 24,
            script: String::new(),
            layers: vec![Layer {
                layer_id: 1,
                timeline_frame_count: None,
                name: "Layer 1".to_string(),
                explicit_keyframes: vec![0],
                placements: Vec::new(),
            }],
        }],
    }
}

#[cfg(test)]
mod default_project_tests {
    use super::{
        default_project, History, ProjectState, DEFAULT_STAGE_HEIGHT, DEFAULT_STAGE_WIDTH,
    };
    use q0s_format::v2::{Asset, Placement, Target, Transform2D, Tween, VectorAsset};

    #[test]
    fn new_project_starts_with_a_real_blank_keyframe() {
        let project = default_project();
        let q0rg = &project.q0rgs[0];
        let layer = &q0rg.layers[0];
        assert_eq!(q0rg.frame_count, 24);
        assert_eq!(layer.keyframe_frames(), vec![0]);
        assert!(layer.is_blank_keyframe(0));
        assert!(crate::render::active_placements_at(layer, 23).is_empty());
    }

    #[test]
    fn new_project_uses_the_classic_640_by_480_stage() {
        let project = default_project();
        assert_eq!(project.meta.stage_width, DEFAULT_STAGE_WIDTH);
        assert_eq!(project.meta.stage_height, DEFAULT_STAGE_HEIGHT);
        assert_eq!(
            u32::from(project.meta.stage_width) * 3,
            u32::from(project.meta.stage_height) * 4,
            "the default stage must keep a true 4:3 aspect ratio"
        );
    }

    #[test]
    fn artwork_motion_skips_structural_cache_revisions() {
        let mut state = ProjectState::new_default();
        state.mark_artwork_dirty();
        assert_eq!(state.revision(), 1);
        assert_eq!(state.render_revision(), 0);
        assert_eq!(state.library_revision(), 0);

        state.mark_dirty();
        assert_eq!(state.revision(), 2);
        assert_eq!(state.render_revision(), 1);
        assert_eq!(state.library_revision(), 1);
    }

    #[test]
    fn undo_redo_preserves_rig_structure_and_instance_identity() {
        let mut before = default_project();
        before.assets.push(Asset::Vector(VectorAsset {
            asset_id: 7,
            paths: Vec::new(),
            fill: None,
            stroke: None,
        }));
        before.q0rgs[0].layers[0].push_placement(Placement {
            pivot: None,
            instance_id: 41,
            frame: 0,
            target: Target::Asset(7),
            transform: Transform2D::IDENTITY,
            tween: Tween::None,
            fx: Default::default(),
        });
        crate::rigging::ensure_rig(&mut before, 1).expect("create rig");
        let bone = crate::rigging::add_bone(
            &mut before,
            1,
            None,
            q0s_format::v2::Vec2::new(0.0, 0.0),
            q0s_format::v2::Vec2::new(20.0, 0.0),
            0,
        )
        .expect("bone");
        let selection = super::Selection::Placement {
            q0rg_id: 1,
            layer_id: 1,
            placement_idx: 0,
        };
        crate::rigging::bind_selected_placement_to_node(&mut before, &selection, bone, 0)
            .expect("bind");
        let stable_id = before.q0rgs[0].layers[0].placements[0].instance_id;
        assert_eq!(stable_id, 41);

        let mut history = History::new();
        history.snapshot(&before);
        let mut after = before.clone();
        crate::rigging::set_node_rotation(&mut after, 1, bone, 6, 0.75, true);
        let undone = history
            .pop_undo(after.clone(), |_, _| {})
            .expect("undo snapshot");
        assert_eq!(undone, before);
        assert_eq!(
            undone.q0rgs[0].layers[0].placements[0].instance_id,
            stable_id
        );
        let redone = history.pop_redo(undone, |_, _| {}).expect("redo snapshot");
        assert_eq!(redone, after);
        assert_eq!(
            redone.q0rgs[0].layers[0].placements[0].instance_id,
            stable_id
        );
        let rig = q0s_format::rig::rig_for_q0rg(&redone, 1).expect("rig after redo");
        assert_eq!(
            rig.nodes[0].binding.expect("binding").instance_id,
            stable_id
        );
    }
}
