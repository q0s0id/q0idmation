//! What q0lang's layer questions see: everything drawn on a layer as filled
//! outlines in stage space, and whether two drawings meet.
//!
//! Fills count where the renderer fills them (closed paths, non-zero rule),
//! strokes as the stroked shape, bitmaps and videos as their rectangles.
//! Colour does not matter, so a fully transparent shape is an invisible wall.
//! Placement effects such as glow do not count; masks do not cut.

use std::collections::HashMap;
use std::sync::Arc;

use crate::geom::{flatten_path_within, stroke_polygons};
use crate::raster::active_placement_states_at;
use crate::runtime_scene::RuntimeSceneState;
use crate::transform::Affine;
use crate::v2::{
    Asset, InstanceKey, Layer, LayerKind, ProjectV2, Target, Vec2, VectorAsset,
    MAX_Q0RG_NESTING_DEPTH,
};

/// How far a flattened curve may stray from the curve, in local units.
const CURVE_TOLERANCE: f32 = 0.35;
const CURVE_MAX_STEPS: usize = 24;

/// An axis-aligned box.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bounds {
    pub min: Vec2,
    pub max: Vec2,
}

impl Bounds {
    fn of(points: &[Vec2]) -> Option<Self> {
        let first = *points.first()?;
        let mut bounds = Self {
            min: first,
            max: first,
        };
        for point in &points[1..] {
            bounds.min.x = bounds.min.x.min(point.x);
            bounds.min.y = bounds.min.y.min(point.y);
            bounds.max.x = bounds.max.x.max(point.x);
            bounds.max.y = bounds.max.y.max(point.y);
        }
        Some(bounds)
    }

    pub fn union(self, other: Self) -> Self {
        Self {
            min: Vec2::new(self.min.x.min(other.min.x), self.min.y.min(other.min.y)),
            max: Vec2::new(self.max.x.max(other.max.x), self.max.y.max(other.max.y)),
        }
    }

    pub fn meets(self, other: Self) -> bool {
        self.min.x <= other.max.x
            && other.min.x <= self.max.x
            && self.min.y <= other.max.y
            && other.min.y <= self.max.y
    }

    pub fn expand(self, by: f32) -> Self {
        Self {
            min: Vec2::new(self.min.x - by, self.min.y - by),
            max: Vec2::new(self.max.x + by, self.max.y + by),
        }
    }

    pub fn translate(self, dx: f32, dy: f32) -> Self {
        Self {
            min: Vec2::new(self.min.x + dx, self.min.y + dy),
            max: Vec2::new(self.max.x + dx, self.max.y + dy),
        }
    }

    fn contains(self, point: Vec2) -> bool {
        point.x >= self.min.x
            && point.x <= self.max.x
            && point.y >= self.min.y
            && point.y <= self.max.y
    }

    fn transform(self, affine: &Affine) -> Self {
        let corners = [
            affine.apply(self.min),
            affine.apply(Vec2::new(self.max.x, self.min.y)),
            affine.apply(self.max),
            affine.apply(Vec2::new(self.min.x, self.max.y)),
        ];
        Self::of(&corners).expect("four corners")
    }
}

/// A closed polygon and its box.
#[derive(Debug, Clone)]
struct Outline {
    points: Vec<Vec2>,
    bounds: Bounds,
}

impl Outline {
    fn new(points: Vec<Vec2>) -> Option<Self> {
        if points.len() < 2 || points.iter().any(|point| !point.is_finite()) {
            return None;
        }
        let bounds = Bounds::of(&points)?;
        Some(Self { points, bounds })
    }

    fn edges(&self) -> impl Iterator<Item = (Vec2, Vec2)> + '_ {
        let n = self.points.len();
        (0..n).map(move |i| (self.points[i], self.points[(i + 1) % n]))
    }

    /// Non-zero winding contribution of this outline around `point`.
    fn winding(&self, point: Vec2) -> i32 {
        if !self.bounds.contains(point) || self.points.len() < 3 {
            return 0;
        }
        let mut winding = 0;
        for (a, b) in self.edges() {
            if a.y <= point.y {
                if b.y > point.y && cross(a, b, point) > 0.0 {
                    winding += 1;
                }
            } else if b.y <= point.y && cross(a, b, point) < 0.0 {
                winding -= 1;
            }
        }
        winding
    }
}

/// Outlines filled together with the non-zero rule: one fill with its
/// holes, or one stroke.
#[derive(Debug, Clone)]
pub struct Region {
    outlines: Vec<Outline>,
    pub bounds: Bounds,
    /// Which object of the drawing this belongs to; see [`Drawing::objects`].
    pub object: u32,
}

impl Region {
    fn new(outlines: Vec<Outline>) -> Option<Self> {
        let bounds = outlines
            .iter()
            .map(|outline| outline.bounds)
            .reduce(Bounds::union)?;
        Some(Self {
            outlines,
            bounds,
            object: 0,
        })
    }

    /// A filled rectangle.
    pub fn rect(left: f32, top: f32, width: f32, height: f32) -> Option<Self> {
        let points = vec![
            Vec2::new(left, top),
            Vec2::new(left + width, top),
            Vec2::new(left + width, top + height),
            Vec2::new(left, top + height),
        ];
        Self::new(vec![Outline::new(points)?])
    }

    fn transformed(&self, affine: &Affine) -> Self {
        let outlines = self
            .outlines
            .iter()
            .filter_map(|outline| {
                Outline::new(
                    outline
                        .points
                        .iter()
                        .map(|point| affine.apply(*point))
                        .collect(),
                )
            })
            .collect::<Vec<_>>();
        let bounds = outlines
            .iter()
            .map(|outline| outline.bounds)
            .reduce(Bounds::union)
            .unwrap_or(Bounds {
                min: Vec2::new(f32::INFINITY, f32::INFINITY),
                max: Vec2::new(f32::NEG_INFINITY, f32::NEG_INFINITY),
            });
        Self {
            outlines,
            bounds,
            object: self.object,
        }
    }

    fn translated(&self, dx: f32, dy: f32) -> Self {
        Self {
            outlines: self
                .outlines
                .iter()
                .map(|outline| Outline {
                    points: outline
                        .points
                        .iter()
                        .map(|point| Vec2::new(point.x + dx, point.y + dy))
                        .collect(),
                    bounds: outline.bounds.translate(dx, dy),
                })
                .collect(),
            bounds: self.bounds.translate(dx, dy),
            object: self.object,
        }
    }

    pub fn contains(&self, point: Vec2) -> bool {
        self.bounds.contains(point)
            && self
                .outlines
                .iter()
                .map(|outline| outline.winding(point))
                .sum::<i32>()
                != 0
    }

    pub fn meets_circle(&self, centre: Vec2, radius: f32) -> bool {
        let reach = self.bounds.expand(radius);
        if !reach.contains(centre) {
            return false;
        }
        if self.contains(centre) {
            return true;
        }
        let radius2 = radius * radius;
        self.outlines.iter().any(|outline| {
            outline.bounds.expand(radius).contains(centre)
                && outline
                    .edges()
                    .any(|(a, b)| distance2_to_segment(centre, a, b) <= radius2)
        })
    }

    pub fn meets(&self, other: &Region) -> bool {
        if !self.bounds.meets(other.bounds) {
            return false;
        }
        for mine in &self.outlines {
            if !mine.bounds.meets(other.bounds) {
                continue;
            }
            for theirs in &other.outlines {
                if mine.bounds.meets(theirs.bounds) && outlines_cross(mine, theirs) {
                    return true;
                }
            }
        }
        // No edges cross: one is inside the other, or they are apart.
        self.outlines
            .iter()
            .any(|outline| other.contains(outline.points[0]))
            || other
                .outlines
                .iter()
                .any(|outline| self.contains(outline.points[0]))
    }
}

fn cross(a: Vec2, b: Vec2, c: Vec2) -> f32 {
    (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x)
}

fn distance2_to_segment(point: Vec2, a: Vec2, b: Vec2) -> f32 {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let length2 = dx * dx + dy * dy;
    let t = if length2 > 0.0 {
        (((point.x - a.x) * dx + (point.y - a.y) * dy) / length2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let (x, y) = (a.x + dx * t - point.x, a.y + dy * t - point.y);
    x * x + y * y
}

fn segments_meet(a: Vec2, b: Vec2, c: Vec2, d: Vec2) -> bool {
    if a.x.max(b.x) < c.x.min(d.x)
        || c.x.max(d.x) < a.x.min(b.x)
        || a.y.max(b.y) < c.y.min(d.y)
        || c.y.max(d.y) < a.y.min(b.y)
    {
        return false;
    }
    let d1 = cross(c, d, a);
    let d2 = cross(c, d, b);
    let d3 = cross(a, b, c);
    let d4 = cross(a, b, d);
    if ((d1 > 0.0 && d2 < 0.0) || (d1 < 0.0 && d2 > 0.0))
        && ((d3 > 0.0 && d4 < 0.0) || (d3 < 0.0 && d4 > 0.0))
    {
        return true;
    }
    // Touching and lying along one another count too.
    let on = |p: Vec2, q: Vec2, r: Vec2, side: f32| {
        side == 0.0
            && r.x >= p.x.min(q.x)
            && r.x <= p.x.max(q.x)
            && r.y >= p.y.min(q.y)
            && r.y <= p.y.max(q.y)
    };
    on(c, d, a, d1) || on(c, d, b, d2) || on(a, b, c, d3) || on(a, b, d, d4)
}

fn outlines_cross(a: &Outline, b: &Outline) -> bool {
    for (p, q) in a.edges() {
        let edge = Bounds {
            min: Vec2::new(p.x.min(q.x), p.y.min(q.y)),
            max: Vec2::new(p.x.max(q.x), p.y.max(q.y)),
        };
        if !edge.meets(b.bounds) {
            continue;
        }
        if b.edges().any(|(r, s)| segments_meet(p, q, r, s)) {
            return true;
        }
    }
    false
}

/// Everything one or more layers draw, or one placed object, in stage space.
#[derive(Debug, Clone, Default)]
pub struct Drawing {
    pub regions: Vec<Region>,
    /// What each object of the drawing is called: its instance name, or
    /// "drawing" for unnamed artwork. Regions point here with `object`.
    pub objects: Vec<String>,
}

impl Drawing {
    pub fn bounds(&self) -> Option<Bounds> {
        self.regions
            .iter()
            .map(|region| region.bounds)
            .reduce(Bounds::union)
    }

    pub fn is_empty(&self) -> bool {
        self.regions.is_empty()
    }

    pub fn translated(&self, dx: f32, dy: f32) -> Self {
        if dx == 0.0 && dy == 0.0 {
            return self.clone();
        }
        Self {
            regions: self
                .regions
                .iter()
                .map(|region| region.translated(dx, dy))
                .collect(),
            objects: self.objects.clone(),
        }
    }

    /// The first object of this drawing that `other` meets.
    pub fn first_met_by(&self, other: &Drawing) -> Option<&str> {
        let reach = other.bounds()?;
        self.regions
            .iter()
            .filter(|region| region.bounds.meets(reach))
            .find(|region| other.regions.iter().any(|theirs| region.meets(theirs)))
            .map(|region| self.object_name(region.object))
    }

    pub fn meets(&self, other: &Drawing) -> bool {
        self.first_met_by(other).is_some()
    }

    pub fn object_at(&self, point: Vec2) -> Option<&str> {
        self.regions
            .iter()
            .find(|region| region.contains(point))
            .map(|region| self.object_name(region.object))
    }

    pub fn object_in_circle(&self, centre: Vec2, radius: f32) -> Option<&str> {
        self.regions
            .iter()
            .find(|region| region.meets_circle(centre, radius))
            .map(|region| self.object_name(region.object))
    }

    fn object_name(&self, object: u32) -> &str {
        self.objects
            .get(object as usize)
            .map_or("drawing", String::as_str)
    }
}

/// Outlines of the project's artwork in its own coordinates, worked out once.
/// Build a new one for a new project.
#[derive(Debug, Default)]
pub struct CollisionCache {
    assets: HashMap<u16, Arc<Vec<Region>>>,
}

impl CollisionCache {
    pub fn new() -> Self {
        Self::default()
    }

    fn asset(&mut self, project: &ProjectV2, asset_id: u16) -> Arc<Vec<Region>> {
        if let Some(regions) = self.assets.get(&asset_id) {
            return Arc::clone(regions);
        }
        let regions = Arc::new(
            project
                .assets
                .iter()
                .find(|asset| asset.id() == asset_id)
                .map(asset_regions)
                .unwrap_or_default(),
        );
        self.assets.insert(asset_id, Arc::clone(&regions));
        regions
    }
}

fn asset_regions(asset: &Asset) -> Vec<Region> {
    match asset {
        Asset::Vector(vector) => vector_regions(vector),
        Asset::Model(model) => model
            .flat_bounds()
            .and_then(|(min, max)| Region::rect(min.x, min.y, max.x - min.x, max.y - min.y))
            .into_iter()
            .collect(),
        Asset::Bitmap(bitmap) => {
            Region::rect(0.0, 0.0, f32::from(bitmap.width), f32::from(bitmap.height))
                .into_iter()
                .collect()
        }
        Asset::Q0v(video) => q0video::q0v::probe_header(&video.bytes)
            .ok()
            .filter(|media| media.spec.video)
            .and_then(|media| {
                Region::rect(0.0, 0.0, media.spec.width as f32, media.spec.height as f32)
            })
            .into_iter()
            .collect(),
        Asset::Rig(_) => Vec::new(),
    }
}

/// A vector as the renderer fills it: its closed paths together, then each
/// path's stroke.
pub fn vector_regions(vector: &VectorAsset) -> Vec<Region> {
    let mut regions = Vec::new();
    if vector.fill.is_some() {
        let outlines = vector
            .paths
            .iter()
            .filter(|path| path.closed)
            .filter_map(|path| {
                let points = flatten_path_within(path, CURVE_TOLERANCE, CURVE_MAX_STEPS);
                (points.len() >= 3).then_some(points).and_then(Outline::new)
            })
            .collect::<Vec<_>>();
        regions.extend(Region::new(outlines));
    }
    if let Some(stroke) = &vector.stroke {
        for path in &vector.paths {
            let points = flatten_path_within(path, CURVE_TOLERANCE, CURVE_MAX_STEPS);
            let half = stroke.width * 0.5;
            let outlines = if points.len() >= 2 {
                stroke_polygons(&points, path.closed, half, stroke.cap, stroke.join)
                    .into_iter()
                    .filter_map(Outline::new)
                    .collect::<Vec<_>>()
            } else {
                // A single click of the pen: a dot as wide as the stroke.
                points
                    .first()
                    .and_then(|centre| Outline::new(dot(*centre, half)))
                    .into_iter()
                    .collect()
            };
            regions.extend(Region::new(outlines));
        }
    }
    regions
}

fn dot(centre: Vec2, radius: f32) -> Vec<Vec2> {
    (0..12)
        .map(|step| {
            let angle = step as f32 * std::f32::consts::TAU / 12.0;
            Vec2::new(
                centre.x + radius * angle.cos(),
                centre.y + radius * angle.sin(),
            )
        })
        .collect()
}

/// The layers of q0rg `q0rg_id` that `name` picks: a layer by name (case
/// does not matter when no name matches exactly), a folder with everything
/// in it, or the `row`th row of the editor's timeline counted from the top.
pub fn find_layers(
    project: &ProjectV2,
    q0rg_id: u16,
    name: Option<&str>,
    row: Option<u16>,
) -> Vec<u16> {
    let Some(q0rg) = project.q0rgs.iter().find(|q0rg| q0rg.q0rg_id == q0rg_id) else {
        return Vec::new();
    };
    let Some(picked) = find_layer(project, q0rg_id, name, row) else {
        return Vec::new();
    };
    if project.layer_is_folder(q0rg_id, picked) {
        q0rg.layers
            .iter()
            .filter(|layer| project.layer_is_descendant_of(q0rg_id, layer.layer_id, picked))
            .map(|layer| layer.layer_id)
            .collect()
    } else {
        vec![picked]
    }
}

/// The one layer, or folder, that `name` or `row` picks; see [`find_layers`].
pub fn find_layer(
    project: &ProjectV2,
    q0rg_id: u16,
    name: Option<&str>,
    row: Option<u16>,
) -> Option<u16> {
    let q0rg = project.q0rgs.iter().find(|q0rg| q0rg.q0rg_id == q0rg_id)?;
    let picked = match (name, row) {
        (Some(name), _) => q0rg
            .layers
            .iter()
            .find(|layer| layer.name == name)
            .or_else(|| {
                q0rg.layers
                    .iter()
                    .find(|layer| layer.name.eq_ignore_ascii_case(name))
            }),
        (None, Some(row)) if row >= 1 => q0rg
            .layers
            .len()
            .checked_sub(usize::from(row))
            .and_then(|index| q0rg.layers.get(index)),
        _ => None,
    };
    picked.map(|layer| layer.layer_id)
}

/// Everything layers `layer_ids` of q0rg `q0rg_id` draw at `frame`, as
/// `scene` (that q0rg's runtime state) has it. Each object placed on the
/// layers is one object of the drawing. With `within`, only artwork that
/// reaches into that box is worked out; `skip` leaves one placed object out,
/// so a thing on the layer it asks about never touches itself.
#[allow(clippy::too_many_arguments)]
pub fn layer_drawing(
    project: &ProjectV2,
    q0rg_id: u16,
    frame: u16,
    layer_ids: &[u16],
    scene: &RuntimeSceneState,
    cache: &mut CollisionCache,
    within: Option<Bounds>,
    skip: u32,
) -> Drawing {
    let mut walk = Walk {
        project,
        cache,
        within,
        skip,
        drawing: Drawing::default(),
    };
    walk.timeline(
        q0rg_id,
        frame,
        Affine::IDENTITY,
        scene,
        0,
        Pick::Layers(layer_ids),
    );
    walk.drawing
}

/// What the placed object `instance_id` of q0rg `q0rg_id` draws at `frame`,
/// all of it one object.
pub fn instance_drawing(
    project: &ProjectV2,
    q0rg_id: u16,
    frame: u16,
    instance_id: u32,
    scene: &RuntimeSceneState,
    cache: &mut CollisionCache,
) -> Drawing {
    let mut walk = Walk {
        project,
        cache,
        within: None,
        skip: 0,
        drawing: Drawing::default(),
    };
    if instance_id != 0 {
        walk.timeline(
            q0rg_id,
            frame,
            Affine::IDENTITY,
            scene,
            0,
            Pick::Instance(instance_id),
        );
    }
    walk.drawing
}

/// Where the placed object `instance_id` of q0rg `q0rg_id` stands at
/// `frame`: its `name.x` and `name.y`.
pub fn instance_origin(
    project: &ProjectV2,
    q0rg_id: u16,
    frame: u16,
    instance_id: u32,
    scene: &RuntimeSceneState,
) -> Option<Vec2> {
    if instance_id == 0 {
        return None;
    }
    let q0rg = project.q0rgs.iter().find(|q0rg| q0rg.q0rg_id == q0rg_id)?;
    let local_frame = if q0rg.frame_count > 0 {
        frame % q0rg.frame_count
    } else {
        0
    };
    let rig_pose = crate::rig::rig_for_q0rg(project, q0rg_id).map(|rig| {
        crate::rig::evaluate_rig(
            rig,
            f32::from(local_frame),
            scene
                .rig_overrides
                .get(&q0rg_id)
                .map(Vec::as_slice)
                .unwrap_or(&[]),
        )
    });
    for layer in &q0rg.layers {
        if !layer
            .placements
            .iter()
            .any(|placement| placement.instance_id == instance_id)
        {
            continue;
        }
        for active in active_placement_states_at(layer, local_frame) {
            if layer.placements.get(active.index)?.instance_id != instance_id {
                continue;
            }
            let authored = rig_pose
                .as_ref()
                .and_then(|pose| pose.binding_transform(instance_id))
                .unwrap_or_else(|| Affine::from_transform(active.transform));
            let placed = scene.effective_affine(InstanceKey::new(q0rg_id, instance_id), authored);
            return Some(Vec2::new(placed.tx, placed.ty));
        }
    }
    None
}

/// How many objects layers `layer_ids` show at `frame`.
pub fn layer_object_count(
    project: &ProjectV2,
    q0rg_id: u16,
    frame: u16,
    layer_ids: &[u16],
) -> usize {
    let Some(q0rg) = project.q0rgs.iter().find(|q0rg| q0rg.q0rg_id == q0rg_id) else {
        return 0;
    };
    let local_frame = if q0rg.frame_count > 0 {
        frame % q0rg.frame_count
    } else {
        0
    };
    q0rg.layers
        .iter()
        .filter(|layer| layer_ids.contains(&layer.layer_id))
        .map(|layer| active_placement_states_at(layer, local_frame).len())
        .sum()
}

#[derive(Clone, Copy)]
enum Pick<'a> {
    /// Every visible drawing layer.
    All,
    Layers(&'a [u16]),
    Instance(u32),
}

struct Walk<'p, 'c> {
    project: &'p ProjectV2,
    cache: &'c mut CollisionCache,
    within: Option<Bounds>,
    /// A top-level object to leave out; 0 leaves nothing out.
    skip: u32,
    drawing: Drawing,
}

impl Walk<'_, '_> {
    /// The object a region belongs to: a new one for each top-level object,
    /// the object being walked below that.
    fn object(&mut self, q0rg_id: u16, instance_id: u32) -> u32 {
        let name = self
            .project
            .runtime
            .instance_names
            .get(&InstanceKey::new(q0rg_id, instance_id))
            .cloned()
            .unwrap_or_else(|| "drawing".to_string());
        self.drawing.objects.push(name);
        (self.drawing.objects.len() - 1) as u32
    }

    fn push(&mut self, regions: &[Region], affine: &Affine, object: u32) {
        for region in regions {
            let reach = region.bounds.transform(affine);
            if self.within.is_some_and(|within| !within.meets(reach)) {
                continue;
            }
            let mut placed = region.transformed(affine);
            placed.object = object;
            if !placed.outlines.is_empty() {
                self.drawing.regions.push(placed);
            }
        }
    }

    fn timeline(
        &mut self,
        q0rg_id: u16,
        frame: u16,
        parent: Affine,
        scene: &RuntimeSceneState,
        depth: u8,
        pick: Pick<'_>,
    ) {
        self.timeline_as(q0rg_id, frame, parent, scene, depth, pick, None);
    }

    #[allow(clippy::too_many_arguments)]
    fn timeline_as(
        &mut self,
        q0rg_id: u16,
        frame: u16,
        parent: Affine,
        scene: &RuntimeSceneState,
        depth: u8,
        pick: Pick<'_>,
        object: Option<u32>,
    ) {
        if depth > MAX_Q0RG_NESTING_DEPTH {
            return;
        }
        let project = self.project;
        let Some(q0rg) = project.q0rgs.iter().find(|q0rg| q0rg.q0rg_id == q0rg_id) else {
            return;
        };
        let local_frame = if q0rg.frame_count > 0 {
            frame % q0rg.frame_count
        } else {
            0
        };
        for layer in &q0rg.layers {
            let kind = project.layer_metadata(q0rg_id, layer.layer_id).kind;
            let wanted = match pick {
                Pick::All | Pick::Instance(_) => kind == LayerKind::Normal,
                Pick::Layers(ids) => ids.contains(&layer.layer_id) && kind != LayerKind::Folder,
            };
            if !wanted {
                continue;
            }
            if let Pick::Instance(instance_id) = pick {
                if !layer
                    .placements
                    .iter()
                    .any(|placement| placement.instance_id == instance_id)
                {
                    continue;
                }
            }
            let look = scene.layer_look(project, q0rg_id, layer.layer_id);
            if !look.visible {
                continue;
            }
            self.layer(
                q0rg_id,
                layer,
                local_frame,
                look.shift(parent),
                scene,
                depth,
                pick,
                object,
            );
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn layer(
        &mut self,
        q0rg_id: u16,
        layer: &Layer,
        local_frame: u16,
        parent: Affine,
        scene: &RuntimeSceneState,
        depth: u8,
        pick: Pick<'_>,
        object: Option<u32>,
    ) {
        let project = self.project;
        let rig = crate::rig::rig_for_q0rg(project, q0rg_id);
        let rig_pose = rig.map(|rig| {
            crate::rig::evaluate_rig(
                rig,
                f32::from(local_frame),
                scene
                    .rig_overrides
                    .get(&q0rg_id)
                    .map(Vec::as_slice)
                    .unwrap_or(&[]),
            )
        });
        let morph = crate::shape_tween::project_layer_morph(project, layer, local_frame);
        for active in active_placement_states_at(layer, local_frame) {
            let Some(placement) = layer.placements.get(active.index) else {
                continue;
            };
            if let Pick::Instance(instance_id) = pick {
                if placement.instance_id != instance_id {
                    continue;
                }
            }
            if depth == 0 && self.skip != 0 && placement.instance_id == self.skip {
                continue;
            }
            let object = match object {
                Some(object) => object,
                None => self.object(q0rg_id, placement.instance_id),
            };
            let authored = rig_pose
                .as_ref()
                .and_then(|pose| pose.binding_transform(placement.instance_id))
                .unwrap_or_else(|| Affine::from_transform(active.transform));
            let key = InstanceKey::new(q0rg_id, placement.instance_id);
            let composed = Affine::compose(parent, scene.effective_affine(key, authored));
            if let Some(shapes) = morph
                .as_ref()
                .and_then(|morph| morph.for_placement(active.index))
            {
                for shape in shapes {
                    self.push(&vector_regions(shape), &composed, object);
                }
                continue;
            }
            let target = match (rig, rig_pose.as_ref()) {
                (Some(rig), Some(pose)) => crate::rig::resolved_variant_target(
                    rig,
                    pose,
                    placement.instance_id,
                    placement.target,
                ),
                _ => placement.target,
            };
            match target {
                Target::Asset(asset_id) => {
                    let deformed = match (rig, rig_pose.as_ref()) {
                        (Some(rig), Some(pose)) => project
                            .assets
                            .iter()
                            .find_map(|asset| match asset {
                                Asset::Vector(vector) if vector.asset_id == asset_id => {
                                    Some(vector)
                                }
                                _ => None,
                            })
                            .and_then(|vector| {
                                crate::rig::deform_vector_for_instance(
                                    rig,
                                    pose,
                                    placement.instance_id,
                                    vector,
                                )
                            }),
                        _ => None,
                    };
                    match deformed {
                        Some(vector) => self.push(&vector_regions(&vector), &composed, object),
                        None => {
                            let regions = self.cache.asset(project, asset_id);
                            self.push(&regions, &composed, object);
                        }
                    }
                }
                Target::Q0rg(child_id) if child_id != q0rg_id => {
                    let fallback = crate::v2::resolve_q0rg_playback_frame(
                        project,
                        child_id,
                        local_frame,
                        active.playback_start_frame,
                        active.fx.q0rg_playback,
                    );
                    let (child_frame, child_scene) = scene.child_context(key, fallback);
                    self.timeline_as(
                        child_id,
                        child_frame,
                        composed,
                        child_scene,
                        depth + 1,
                        Pick::All,
                        Some(object),
                    );
                }
                Target::Q0rg(_) => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::v2::{Anchor, Path, Stroke};

    fn square(left: f32, top: f32, size: f32) -> Path {
        let point = |x: f32, y: f32| Anchor {
            point: Vec2::new(x, y),
            in_handle: None,
            out_handle: None,
        };
        Path {
            anchors: vec![
                point(left, top),
                point(left + size, top),
                point(left + size, top + size),
                point(left, top + size),
            ],
            closed: true,
        }
    }

    fn filled(paths: Vec<Path>) -> VectorAsset {
        VectorAsset {
            asset_id: 1,
            paths,
            fill: Some(crate::v2::Rgba {
                r: 255,
                g: 0,
                b: 0,
                a: 255,
            }),
            stroke: None,
        }
    }

    fn drawing(regions: Vec<Region>) -> Drawing {
        Drawing {
            regions,
            objects: vec!["thing".into()],
        }
    }

    #[test]
    fn fills_meet_when_they_overlap_or_one_holds_the_other() {
        let big = drawing(vector_regions(&filled(vec![square(0.0, 0.0, 100.0)])));
        let inside = drawing(vector_regions(&filled(vec![square(40.0, 40.0, 10.0)])));
        let across = drawing(vector_regions(&filled(vec![square(90.0, 90.0, 30.0)])));
        let apart = drawing(vector_regions(&filled(vec![square(150.0, 0.0, 10.0)])));
        assert!(big.meets(&inside));
        assert!(inside.meets(&big));
        assert!(big.meets(&across));
        assert!(!big.meets(&apart));
    }

    #[test]
    fn a_hole_is_empty_and_an_outline_only_blocks_along_its_line() {
        // A square with a square hole: the outer path one way round, the
        // hole the other.
        let mut hole = square(30.0, 30.0, 40.0);
        hole.anchors.reverse();
        let ring = drawing(vector_regions(&filled(vec![square(0.0, 0.0, 100.0), hole])));
        let in_hole = drawing(vector_regions(&filled(vec![square(45.0, 45.0, 10.0)])));
        assert!(!ring.meets(&in_hole));
        assert!(ring.object_at(Vec2::new(10.0, 10.0)).is_some());
        assert!(ring.object_at(Vec2::new(50.0, 50.0)).is_none());

        let mut box_line = filled(vec![square(0.0, 0.0, 100.0)]);
        box_line.fill = None;
        box_line.stroke = Some(Stroke {
            color: crate::v2::Rgba {
                r: 255,
                g: 255,
                b: 255,
                a: 255,
            },
            width: 4.0,
            cap: crate::geom::CapShape::Round,
            join: crate::geom::JoinShape::Miter,
        });
        let walls = drawing(vector_regions(&box_line));
        assert!(!walls.meets(&in_hole));
        let at_wall = drawing(vector_regions(&filled(vec![square(95.0, 40.0, 10.0)])));
        assert!(walls.meets(&at_wall));
        assert!(walls.object_in_circle(Vec2::new(50.0, 1.0), 2.0).is_some());
        assert!(walls
            .object_in_circle(Vec2::new(50.0, 50.0), 10.0)
            .is_none());
    }

    #[test]
    fn a_moved_drawing_meets_what_it_moved_onto() {
        let wall = drawing(vector_regions(&filled(vec![square(100.0, 0.0, 10.0)])));
        let soul = drawing(vector_regions(&filled(vec![square(0.0, 0.0, 10.0)])));
        assert!(!wall.meets(&soul));
        assert!(wall.meets(&soul.translated(95.0, 0.0)));
        assert!(!wall.meets(&soul.translated(85.0, 0.0)));
    }
}
