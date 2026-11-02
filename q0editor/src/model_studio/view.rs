//! The studio viewport's insides: what is drawn over the model, picking,
//! box picking, and the changes that follow the mouse.

use egui::{Pos2, Rect};
use q0s_format::three::draw::{Overlay, View3d};
use q0s_format::three::model::mesh_matrix;
use q0s_format::three::{Mat4, V3};

use super::{
    about, flip, rotation_about, Constraint, Element, Hit, Mode, ModelStudio, Op, OpKind, Start,
    Tool,
};

#[derive(Debug, Clone, Copy)]
pub struct Palette {
    pub background: [u8; 4],
    pub grid: [u8; 4],
    pub grid_major: [u8; 4],
    pub wire: [u8; 4],
    pub selected: [u8; 4],
    pub hover: [u8; 4],
    pub vertex: [u8; 4],
    pub face_tint: [u8; 4],
    pub hover_tint: [u8; 4],
}

impl Palette {
    pub fn dark() -> Self {
        Self {
            background: [48, 50, 57, 255],
            grid: [255, 255, 255, 30],
            grid_major: [255, 255, 255, 72],
            wire: [12, 12, 16, 150],
            selected: [255, 150, 40, 255],
            hover: [255, 255, 255, 220],
            vertex: [20, 20, 24, 255],
            face_tint: [255, 150, 40, 80],
            hover_tint: [255, 255, 255, 40],
        }
    }
}

/// The floor: fine squares around the middle of the view, every tenth line
/// reaching far past what the camera sees, all fading out toward the edge
/// so there is no hard border to bump into whichever way the camera turns.
fn floor(studio: &ModelStudio, palette: &Palette, overlay: &mut Overlay) {
    let step = studio.grid_step();
    let major = step * 10.0;
    let distance = studio.orbit.distance.max(studio.size() * 0.5);
    let centre = studio.orbit.target;
    let (cx, cz) = (
        (centre.x / major).round() * major,
        (centre.z / major).round() * major,
    );
    let minor_reach = (distance * 1.6 / major).ceil().max(1.0) * major;
    let major_reach = (distance * 9.0 / major).ceil().max(2.0) * major;
    let mut line = |a: V3, b: V3, colour: [u8; 4], reach: f32, pieces: usize| {
        for k in 0..pieces {
            let (t0, t1) = (k as f32 / pieces as f32, (k + 1) as f32 / pieces as f32);
            let (p0, p1) = (a + (b - a) * t0, a + (b - a) * t1);
            let mid = (p0 + p1) * 0.5;
            let r = ((mid.x - cx).powi(2) + (mid.z - cz).powi(2)).sqrt() / reach;
            let fade = (1.0 - r * r).max(0.0);
            if fade > 0.01 {
                let mut c = colour;
                c[3] = (f32::from(c[3]) * fade) as u8;
                overlay.lines.push(([flip(p0), flip(p1)], c));
            }
        }
    };
    let minor = (minor_reach / step).round() as i32;
    for i in -minor..=minor {
        if i % 10 == 0 {
            continue;
        }
        let o = i as f32 * step;
        line(
            V3::new(cx + o, 0.0, cz - minor_reach),
            V3::new(cx + o, 0.0, cz + minor_reach),
            palette.grid,
            minor_reach,
            8,
        );
        line(
            V3::new(cx - minor_reach, 0.0, cz + o),
            V3::new(cx + minor_reach, 0.0, cz + o),
            palette.grid,
            minor_reach,
            8,
        );
    }
    let count = (major_reach / major).round() as i32;
    for i in -count..=count {
        let o = i as f32 * major;
        if (cx + o).abs() > step * 0.5 {
            line(
                V3::new(cx + o, 0.0, cz - major_reach),
                V3::new(cx + o, 0.0, cz + major_reach),
                palette.grid_major,
                major_reach,
                16,
            );
        }
        if (cz + o).abs() > step * 0.5 {
            line(
                V3::new(cx - major_reach, 0.0, cz + o),
                V3::new(cx + major_reach, 0.0, cz + o),
                palette.grid_major,
                major_reach,
                16,
            );
        }
    }
    // The model's own axes through the origin: red x, blue z.
    line(
        V3::new(cx - major_reach, 0.0, 0.0),
        V3::new(cx + major_reach, 0.0, 0.0),
        [235, 80, 80, 210],
        major_reach,
        24,
    );
    line(
        V3::new(0.0, 0.0, cz - major_reach),
        V3::new(0.0, 0.0, cz + major_reach),
        [90, 140, 245, 210],
        major_reach,
        24,
    );
}

/// Grid, wires, points, picked and hovered parts. Bones and gizmos are
/// drawn over the picture by the viewport itself.
pub fn overlay(studio: &ModelStudio, world: &[Mat4], palette: &Palette) -> Overlay {
    let mut overlay = Overlay {
        point_size: 7.0,
        on_top: studio.xray,
        ..Overlay::default()
    };
    if studio.show_grid {
        floor(studio, palette, &mut overlay);
    }

    let idle = studio.op.is_none();
    for (index, mesh) in studio.model.meshes.iter().enumerate() {
        if mesh.hidden {
            continue;
        }
        let editing = studio.mode == Mode::Edit && index == studio.mesh;
        let picked = studio.mode == Mode::Object && studio.objects.contains(&index);
        let hovered = studio.mode == Mode::Object && studio.hover == Hit::Part(index) && idle;
        if !editing && !picked && !hovered && !studio.wire {
            continue;
        }
        let m = super::FLIP * mesh_matrix(mesh, world);
        let colour = if picked {
            palette.selected
        } else if hovered {
            palette.hover
        } else {
            palette.wire
        };
        for [a, b] in q0model::ops::edges(mesh) {
            let chosen = editing
                && match studio.element {
                    Element::Vertex => studio.verts.contains(&a) && studio.verts.contains(&b),
                    Element::Edge => studio.edges.contains(&[a.min(b), a.max(b)]),
                    Element::Face => false,
                };
            let lit = editing && idle && studio.hover == Hit::Edge([a.min(b), a.max(b)]);
            let c = if chosen {
                palette.selected
            } else if lit {
                palette.hover
            } else {
                colour
            };
            overlay.lines.push((
                [
                    m.transform_point(mesh.positions[a as usize]),
                    m.transform_point(mesh.positions[b as usize]),
                ],
                c,
            ));
        }
        let tint = |face: usize, c: [u8; 4], out: &mut Overlay| {
            let Some(face) = mesh.faces.get(face) else {
                return;
            };
            let verts = mesh.face_verts(face);
            for k in 1..verts.len().saturating_sub(1) {
                out.triangles.push((
                    [
                        m.transform_point(mesh.positions[verts[0] as usize]),
                        m.transform_point(mesh.positions[verts[k] as usize]),
                        m.transform_point(mesh.positions[verts[k + 1] as usize]),
                    ],
                    c,
                ));
            }
        };
        if hovered && studio.tool == Tool::Paint {
            for f in 0..mesh.faces.len() {
                tint(f, palette.hover_tint, &mut overlay);
            }
        }
        if !editing {
            continue;
        }
        match studio.element {
            Element::Vertex => {
                for (v, p) in mesh.positions.iter().enumerate() {
                    let c = if studio.verts.contains(&(v as u32)) {
                        palette.selected
                    } else if idle && studio.hover == Hit::Vertex(v as u32) {
                        palette.hover
                    } else {
                        palette.vertex
                    };
                    overlay.points.push((m.transform_point(*p), c));
                }
            }
            Element::Face => {
                for (f, face) in mesh.faces.iter().enumerate() {
                    let c = if studio.faces.contains(&f) {
                        palette.selected
                    } else {
                        palette.vertex
                    };
                    overlay
                        .points
                        .push((m.transform_point(mesh.face_center(face)), c));
                }
            }
            Element::Edge => {}
        }
        for f in studio.working_faces() {
            tint(f, palette.face_tint, &mut overlay);
        }
        if let Hit::Face(f) = studio.hover {
            if idle {
                tint(f, palette.hover_tint, &mut overlay);
            }
        }
    }
    overlay
}

/// Möller–Trumbore, both sides.
pub fn ray_triangle(origin: V3, dir: V3, a: V3, b: V3, c: V3) -> Option<f32> {
    let e1 = b - a;
    let e2 = c - a;
    let p = dir.cross(e2);
    let det = e1.dot(p);
    if det.abs() < 1.0e-12 {
        return None;
    }
    let inv = 1.0 / det;
    let s = origin - a;
    let u = s.dot(p) * inv;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = s.cross(e1);
    let v = dir.dot(q) * inv;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = e2.dot(q) * inv;
    (t > 1.0e-6).then_some(t)
}

/// A ray's first meeting with a face: part, face, distance and point.
pub type FaceHit = (usize, usize, f32, V3);

/// Where the ray through a screen point first meets a face. `only` limits
/// it to one part.
pub fn pick_face(
    studio: &ModelStudio,
    view: &View3d,
    at: Pos2,
    only: Option<usize>,
) -> Option<FaceHit> {
    let (origin, direction) = studio.ray(view, at)?;
    let world = studio.world();
    let mut best: Option<FaceHit> = None;
    for (index, mesh) in studio.model.meshes.iter().enumerate() {
        if mesh.hidden || only.is_some_and(|o| o != index) {
            continue;
        }
        let m = mesh_matrix(mesh, &world);
        let points: Vec<V3> = mesh
            .positions
            .iter()
            .map(|p| m.transform_point(*p))
            .collect();
        for (f, face) in mesh.faces.iter().enumerate() {
            let verts = mesh.face_verts(face);
            for k in 1..verts.len().saturating_sub(1) {
                let (a, b, c) = (
                    points[verts[0] as usize],
                    points[verts[k] as usize],
                    points[verts[k + 1] as usize],
                );
                if let Some(t) = ray_triangle(origin, direction, a, b, c) {
                    if best.is_none_or(|(_, _, d, _)| t < d) {
                        best = Some((index, f, t, origin + direction * t));
                    }
                }
            }
        }
    }
    best
}

fn screen(view: &View3d, p: V3) -> Option<Pos2> {
    view.project(flip(p)).map(|[x, y, _]| Pos2::new(x, y))
}

pub fn segment_distance(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let t = if ab.length_sq() > 0.0 {
        ((p - a).dot(ab) / ab.length_sq()).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (a + ab * t).distance(p)
}

/// Is a model-space point in plain sight, not behind a face?
fn visible(studio: &ModelStudio, view: &View3d, p: V3, hit: Option<FaceHit>) -> bool {
    if studio.xray || view.orthographic {
        return true;
    }
    let Some((_, _, t, _)) = hit else { return true };
    let eye = flip(view.camera.position);
    (p - eye).length() <= t + studio.size() * 0.004
}

/// The nearest edge of the active part on screen, with how far along it the
/// point lies (0 at its first end).
pub fn nearest_edge(
    studio: &ModelStudio,
    view: &View3d,
    at: Pos2,
    reach: f32,
) -> Option<([u32; 2], f32)> {
    let mesh = studio.model.meshes.get(studio.mesh)?;
    let m = mesh_matrix(mesh, &studio.world());
    let hit = pick_face(studio, view, at, None);
    let mut best: Option<([u32; 2], f32, f32)> = None;
    for [a, b] in q0model::ops::edges(mesh) {
        let (pa, pb) = (
            m.transform_point(mesh.positions[a as usize]),
            m.transform_point(mesh.positions[b as usize]),
        );
        let (Some(sa), Some(sb)) = (screen(view, pa), screen(view, pb)) else {
            continue;
        };
        let d = segment_distance(at, sa, sb);
        if d < reach
            && best.is_none_or(|(_, _, bd)| d < bd)
            && visible(studio, view, (pa + pb) * 0.5, hit)
        {
            let ab = sb - sa;
            let t = if ab.length_sq() > 0.0 {
                ((at - sa).dot(ab) / ab.length_sq()).clamp(0.0, 1.0)
            } else {
                0.5
            };
            best = Some(([a, b], t, d));
        }
    }
    best.map(|(e, t, _)| (e, t))
}

/// What a click at `at` would pick.
pub fn pick_at(studio: &ModelStudio, view: &View3d, at: Pos2) -> Hit {
    let world = studio.world();
    match studio.mode {
        Mode::Object => {
            pick_face(studio, view, at, None).map_or(Hit::None, |(m, _, _, _)| Hit::Part(m))
        }
        Mode::Pose => {
            let mut best: Option<(u16, f32)> = None;
            for (index, node) in studio.model.nodes.iter().enumerate() {
                if !node.bone {
                    continue;
                }
                let head = world[index].translation_part();
                let tail = world[index].transform_point(V3::new(0.0, node.length.max(1.0e-4), 0.0));
                if let (Some(a), Some(b)) = (screen(view, head), screen(view, tail)) {
                    let d = segment_distance(at, a, b);
                    if d < 14.0 && best.is_none_or(|(_, bd)| d < bd) {
                        best = Some((index as u16, d));
                    }
                }
            }
            best.map_or(Hit::None, |(b, _)| Hit::Bone(b))
        }
        Mode::Edit => {
            let Some(mesh) = studio.model.meshes.get(studio.mesh) else {
                return Hit::None;
            };
            let m = mesh_matrix(mesh, &world);
            match studio.element {
                Element::Vertex => {
                    let hit = pick_face(studio, view, at, None);
                    let mut best: Option<(u32, f32)> = None;
                    for (v, p) in mesh.positions.iter().enumerate() {
                        let p = m.transform_point(*p);
                        let Some(s) = screen(view, p) else { continue };
                        let d = s.distance(at);
                        if d < 14.0
                            && best.is_none_or(|(_, bd)| d < bd)
                            && visible(studio, view, p, hit)
                        {
                            best = Some((v as u32, d));
                        }
                    }
                    best.map_or(Hit::None, |(v, _)| Hit::Vertex(v))
                }
                Element::Edge => nearest_edge(studio, view, at, 10.0)
                    .map_or(Hit::None, |([a, b], _)| Hit::Edge([a.min(b), a.max(b)])),
                Element::Face => pick_face(studio, view, at, Some(studio.mesh))
                    .map_or(Hit::None, |(_, f, _, _)| Hit::Face(f)),
            }
        }
    }
}

/// Picks `hit`: alone, or toggled in the selection with `add`.
pub fn select(studio: &mut ModelStudio, hit: Hit, add: bool) {
    fn toggle<T: Ord + Copy>(set: &mut std::collections::BTreeSet<T>, item: T, add: bool) {
        if !add {
            set.clear();
            set.insert(item);
        } else if !set.remove(&item) {
            set.insert(item);
        }
    }
    match hit {
        Hit::None => {
            if !add {
                studio.objects.clear();
                studio.clear_elements();
                if studio.mode == Mode::Pose {
                    studio.bone = None;
                }
            }
        }
        Hit::Part(m) => {
            toggle(&mut studio.objects, m, add);
            studio.mesh = m;
        }
        Hit::Vertex(v) => toggle(&mut studio.verts, v, add),
        Hit::Edge(e) => toggle(&mut studio.edges, e, add),
        Hit::Face(f) => toggle(&mut studio.faces, f, add),
        Hit::Bone(b) => studio.bone = Some(b),
    }
}

/// A coarse depth picture of the visible faces, for telling which points a
/// box takes.
struct Depth {
    rect: Rect,
    w: usize,
    h: usize,
    z: Vec<f32>,
}

impl Depth {
    fn build(studio: &ModelStudio, view: &View3d) -> Self {
        let rect = studio.viewport;
        let w = ((rect.width() / 2.0) as usize).max(1);
        let h = ((rect.height() / 2.0) as usize).max(1);
        let mut z = vec![f32::INFINITY; w * h];
        let world = studio.world();
        let sx = w as f32 / rect.width().max(1.0);
        let sy = h as f32 / rect.height().max(1.0);
        for mesh in studio.model.meshes.iter().filter(|m| !m.hidden) {
            let m = mesh_matrix(mesh, &world);
            let projected: Vec<Option<[f32; 3]>> = mesh
                .positions
                .iter()
                .map(|p| {
                    view.project(flip(m.transform_point(*p)))
                        .map(|[x, y, d]| [(x - rect.min.x) * sx, (y - rect.min.y) * sy, d])
                })
                .collect();
            for face in &mesh.faces {
                let verts = mesh.face_verts(face);
                for k in 1..verts.len().saturating_sub(1) {
                    let (Some(a), Some(b), Some(c)) = (
                        projected[verts[0] as usize],
                        projected[verts[k] as usize],
                        projected[verts[k + 1] as usize],
                    ) else {
                        continue;
                    };
                    let area = (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
                    if area.abs() < 1.0e-9 {
                        continue;
                    }
                    let x0 = a[0].min(b[0]).min(c[0]).floor().max(0.0) as usize;
                    let x1 = (a[0].max(b[0]).max(c[0]).ceil().max(0.0) as usize).min(w - 1);
                    let y0 = a[1].min(b[1]).min(c[1]).floor().max(0.0) as usize;
                    let y1 = (a[1].max(b[1]).max(c[1]).ceil().max(0.0) as usize).min(h - 1);
                    for y in y0..=y1 {
                        for x in x0..=x1 {
                            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                            let w0 = ((b[0] - px) * (c[1] - py) - (b[1] - py) * (c[0] - px)) / area;
                            let w1 = ((c[0] - px) * (a[1] - py) - (c[1] - py) * (a[0] - px)) / area;
                            let w2 = 1.0 - w0 - w1;
                            if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                                continue;
                            }
                            let inv = w0 / a[2] + w1 / b[2] + w2 / c[2];
                            let d = 1.0 / inv.max(1.0e-12);
                            let slot = &mut z[y * w + x];
                            if d < *slot {
                                *slot = d;
                            }
                        }
                    }
                }
            }
        }
        Self { rect, w, h, z }
    }

    fn sees(&self, x: f32, y: f32, depth: f32, slack: f32) -> bool {
        let px = ((x - self.rect.min.x) * self.w as f32 / self.rect.width().max(1.0)) as isize;
        let py = ((y - self.rect.min.y) * self.h as f32 / self.rect.height().max(1.0)) as isize;
        for dy in -1..=1 {
            for dx in -1..=1 {
                let (x, y) = (px + dx, py + dy);
                if x < 0 || y < 0 || x as usize >= self.w || y as usize >= self.h {
                    continue;
                }
                if depth <= self.z[y as usize * self.w + x as usize] + slack {
                    return true;
                }
            }
        }
        false
    }
}

/// Picks everything inside a screen rectangle.
pub fn box_select(studio: &mut ModelStudio, view: &View3d, area: Rect, add: bool) {
    let world = studio.world();
    let depth = (!studio.xray && studio.mode == Mode::Edit).then(|| Depth::build(studio, view));
    let slack = studio.size() * 0.01;
    let inside = |p: V3| -> bool {
        let Some([x, y, d]) = view.project(flip(p)) else {
            return false;
        };
        area.contains(Pos2::new(x, y))
            && depth
                .as_ref()
                .is_none_or(|depth| depth.sees(x, y, d, slack))
    };
    match studio.mode {
        Mode::Object => {
            if !add {
                studio.objects.clear();
            }
            for (index, mesh) in studio.model.meshes.iter().enumerate() {
                let m = mesh_matrix(mesh, &world);
                if let Some((lo, hi)) = mesh.bounds() {
                    if !mesh.hidden && inside(m.transform_point((lo + hi) * 0.5)) {
                        studio.objects.insert(index);
                    }
                }
            }
            if let Some(first) = studio.objects.iter().next() {
                studio.mesh = *first;
            }
        }
        Mode::Pose => {
            for (index, node) in studio.model.nodes.iter().enumerate() {
                if node.bone && inside(world[index].translation_part()) {
                    studio.bone = Some(index as u16);
                }
            }
        }
        Mode::Edit => {
            let Some(mesh) = studio.model.meshes.get(studio.mesh) else {
                return;
            };
            let m = mesh_matrix(mesh, &world);
            let points: Vec<bool> = mesh
                .positions
                .iter()
                .map(|p| inside(m.transform_point(*p)))
                .collect();
            match studio.element {
                Element::Vertex => {
                    if !add {
                        studio.verts.clear();
                    }
                    studio
                        .verts
                        .extend((0..points.len()).filter(|v| points[*v]).map(|v| v as u32));
                }
                Element::Edge => {
                    if !add {
                        studio.edges.clear();
                    }
                    for [a, b] in q0model::ops::edges(mesh) {
                        if points[a as usize] && points[b as usize] {
                            studio.edges.insert([a, b]);
                        }
                    }
                }
                Element::Face => {
                    if !add {
                        studio.faces.clear();
                    }
                    for (f, face) in mesh.faces.iter().enumerate() {
                        if mesh.face_verts(face).iter().all(|v| points[*v as usize]) {
                            studio.faces.insert(f);
                        }
                    }
                }
            }
        }
    }
}

/// Starts a change of the selection following the mouse from `mouse`.
pub fn begin(
    studio: &mut ModelStudio,
    kind: OpKind,
    mouse: Pos2,
    constraint: Constraint,
    label: &str,
) -> bool {
    let Some(pivot) = studio.selection_centre() else {
        studio.status = "pick something first".into();
        return false;
    };
    let before = Box::new(studio.model.clone());
    let start = match studio.mode {
        Mode::Edit if kind == OpKind::Inset => {
            let faces = studio.working_faces();
            if faces.is_empty() {
                studio.status = "pick faces to inset".into();
                return false;
            }
            Start::Mesh {
                mesh: studio.mesh,
                faces,
                original: Box::new(studio.model.meshes[studio.mesh].clone()),
            }
        }
        Mode::Edit => {
            let Some(mesh) = studio.model.meshes.get(studio.mesh) else {
                return false;
            };
            let list: Vec<(u32, V3)> = studio
                .moving_verts()
                .into_iter()
                .filter_map(|v| mesh.positions.get(v as usize).map(|p| (v, *p)))
                .collect();
            if list.is_empty() {
                return false;
            }
            Start::Verts {
                mesh: studio.mesh,
                list,
            }
        }
        Mode::Object => {
            let objects: Vec<usize> = studio.objects.iter().copied().collect();
            let nodes: Vec<u16> = objects
                .into_iter()
                .filter_map(|m| studio.ensure_node(m))
                .collect();
            let locals = studio.model.rest_locals();
            Start::Nodes {
                list: nodes
                    .into_iter()
                    .map(|n| (n, locals[usize::from(n)]))
                    .collect(),
            }
        }
        Mode::Pose => {
            let Some(bone) = studio.bone else {
                return false;
            };
            let locals = studio.locals();
            Start::Nodes {
                list: vec![(bone, locals[usize::from(bone)])],
            }
        }
    };
    studio.snapshot();
    studio.op = Some(Op {
        kind,
        constraint,
        label: label.to_string(),
        start_mouse: mouse,
        pivot,
        start,
        before,
        fresh: false,
        dragging: false,
        value: String::new(),
    });
    true
}

/// Starts sliding a loop cut through `edge` of the active part.
pub fn begin_loop_cut(studio: &mut ModelStudio, edge: [u32; 2], mouse: Pos2) -> bool {
    let Some(mesh) = studio.model.meshes.get(studio.mesh) else {
        return false;
    };
    let (_, quads) = q0model::ops::edge_ring(mesh, edge);
    if quads.is_empty() {
        studio.status = "a loop cut needs a strip of four-sided faces".into();
        return false;
    }
    let original = Box::new(mesh.clone());
    let before = Box::new(studio.model.clone());
    studio.snapshot();
    studio.op = Some(Op {
        kind: OpKind::LoopCut,
        constraint: Constraint::Free,
        label: String::new(),
        start_mouse: mouse,
        pivot: V3::ZERO,
        start: Start::Loop {
            mesh: studio.mesh,
            edge,
            original,
        },
        before,
        fresh: false,
        dragging: true,
        value: String::new(),
    });
    true
}

/// Where the mouse ray meets the plane through `point` with normal `n`.
fn on_plane(origin: V3, dir: V3, point: V3, n: V3) -> Option<V3> {
    let denom = dir.dot(n);
    if denom.abs() < 1.0e-6 {
        return None;
    }
    let t = (point - origin).dot(n) / denom;
    Some(origin + dir * t)
}

/// The distance along `axis` (through `point`) closest to the mouse ray.
fn along_axis(origin: V3, dir: V3, point: V3, axis: V3) -> Option<f32> {
    let w0 = point - origin;
    let b = axis.dot(dir);
    let denom = 1.0 - b * b;
    if denom.abs() < 1.0e-5 {
        return None;
    }
    Some((b * dir.dot(w0) - axis.dot(w0)) / denom)
}

fn snap(value: f32, step: f32) -> f32 {
    (value / step).round() * step
}

/// The model-space change the mouse asks for now.
fn op_matrix(studio: &mut ModelStudio, view: &View3d, mouse: Pos2, snapping: bool) -> Mat4 {
    let Some(op) = studio.op.clone() else {
        return Mat4::IDENTITY;
    };
    let step = studio.grid_step();
    let angle_step = studio.settings.angle_step.max(1.0);
    let Some(pivot_screen) = studio.screen(view, op.pivot) else {
        return Mat4::IDENTITY;
    };
    let rays = (studio.ray(view, op.start_mouse), studio.ray(view, mouse));
    let mut value = String::new();
    let m = match op.kind {
        OpKind::Grab | OpKind::Inset | OpKind::LoopCut => {
            let forward = studio.orbit.forward();
            let delta = match (op.constraint, rays) {
                (Constraint::Axis(axis), (Some((o0, d0)), Some((o1, d1)))) => {
                    match (
                        along_axis(o0, d0, op.pivot, axis),
                        along_axis(o1, d1, op.pivot, axis),
                    ) {
                        (Some(s0), Some(s1)) => {
                            let mut s = s1 - s0;
                            if snapping {
                                s = snap(s, step);
                            }
                            value = format!("{s:+.3}");
                            axis * s
                        }
                        _ => V3::ZERO,
                    }
                }
                (constraint, (Some((o0, d0)), Some((o1, d1)))) => {
                    let n = match constraint {
                        Constraint::Plane(n) => n,
                        _ => forward,
                    };
                    match (on_plane(o0, d0, op.pivot, n), on_plane(o1, d1, op.pivot, n)) {
                        (Some(a), Some(b)) => {
                            let mut d = b - a;
                            if snapping {
                                d = V3::new(snap(d.x, step), snap(d.y, step), snap(d.z, step));
                            }
                            value = format!("{:+.3}, {:+.3}, {:+.3}", d.x, d.y, d.z);
                            d
                        }
                        _ => V3::ZERO,
                    }
                }
                _ => V3::ZERO,
            };
            Mat4::translation(delta)
        }
        OpKind::Rotate => {
            let a = op.start_mouse - pivot_screen;
            let b = mouse - pivot_screen;
            if a.length() < 2.0 || b.length() < 2.0 {
                return Mat4::IDENTITY;
            }
            let mut degrees = -(b.y.atan2(b.x) - a.y.atan2(a.x)).to_degrees();
            degrees = (degrees + 540.0).rem_euclid(360.0) - 180.0;
            let toward_viewer = -studio.orbit.forward();
            let axis = match op.constraint {
                Constraint::Axis(axis) | Constraint::Plane(axis) => {
                    if axis.dot(toward_viewer) < 0.0 {
                        degrees = -degrees;
                    }
                    axis
                }
                Constraint::Free => toward_viewer,
            };
            if snapping {
                degrees = snap(degrees, angle_step);
            }
            value = format!("{degrees:+.1}°");
            about(op.pivot, rotation_about(axis, degrees))
        }
        OpKind::Scale => {
            let a = (op.start_mouse - pivot_screen).length().max(4.0);
            let mut k = (mouse - pivot_screen).length() / a;
            if snapping {
                k = snap(k, 0.1).max(0.1);
            }
            value = format!("×{k:.2}");
            let m = match op.constraint {
                Constraint::Axis(axis) => {
                    let mut m = Mat4::IDENTITY;
                    let a = [axis.x, axis.y, axis.z];
                    for r in 0..3 {
                        for c in 0..3 {
                            let v = if r == c { 1.0 } else { 0.0 } + (k - 1.0) * a[r] * a[c];
                            m.set(r, c, v);
                        }
                    }
                    m
                }
                Constraint::Plane(n) => {
                    let mut m = Mat4::IDENTITY;
                    let a = [n.x, n.y, n.z];
                    for r in 0..3 {
                        for c in 0..3 {
                            let identity = if r == c { 1.0 } else { 0.0 };
                            m.set(r, c, k * identity + (1.0 - k) * a[r] * a[c]);
                        }
                    }
                    m
                }
                Constraint::Free => Mat4::scale(V3::splat(k)),
            };
            about(op.pivot, m)
        }
    };
    if let Some(op) = &mut studio.op {
        op.value = value;
    }
    m
}

/// Follows the mouse with the running change.
pub fn update(studio: &mut ModelStudio, view: &View3d, mouse: Pos2, snapping: bool) {
    let Some(op) = studio.op.clone() else { return };
    if let Start::Loop {
        mesh,
        edge,
        original,
    } = &op.start
    {
        let world = studio.model.rest_world();
        let m = mesh_matrix(original, &world);
        let (pa, pb) = (
            m.transform_point(original.positions[edge[0] as usize]),
            m.transform_point(original.positions[edge[1] as usize]),
        );
        let t = match (studio.screen(view, pa), studio.screen(view, pb)) {
            (Some(a), Some(b)) if (b - a).length_sq() > 1.0 => {
                ((mouse - a).dot(b - a) / (b - a).length_sq()).clamp(0.05, 0.95)
            }
            _ => 0.5,
        };
        let t = if snapping {
            snap(t, 0.25).clamp(0.05, 0.95)
        } else {
            t
        };
        let mut fresh = (**original).clone();
        let new = q0model::ops::loop_cut(&mut fresh, *edge, t);
        studio.model.meshes[*mesh] = fresh;
        studio.element = Element::Edge;
        studio.verts.clear();
        studio.faces.clear();
        studio.edges = q0model::ops::edges(&studio.model.meshes[*mesh])
            .into_iter()
            .filter(|[a, b]| new.contains(a) && new.contains(b))
            .collect();
        if let Some(op) = &mut studio.op {
            op.value = format!("{:.0}% along", t * 100.0);
        }
        studio.touch();
        return;
    }
    let t = op_matrix(studio, view, mouse, snapping);
    match &op.start {
        Start::Verts { mesh, list } => {
            let world = studio.model.rest_world();
            let Some(target) = studio.model.meshes.get(*mesh) else {
                return;
            };
            let m = mesh_matrix(target, &world);
            let inverse = m.inverse().unwrap_or(Mat4::IDENTITY);
            let full = inverse * t * m;
            let target = &mut studio.model.meshes[*mesh];
            for (v, original) in list {
                if let Some(p) = target.positions.get_mut(*v as usize) {
                    *p = full.transform_point(*original);
                }
            }
            target.corner_normals.clear();
        }
        Start::Mesh {
            mesh,
            faces,
            original,
        } => {
            let amount = ((mouse.x - op.start_mouse.x).abs() / 240.0).clamp(0.0, 0.95);
            let amount = if snapping { snap(amount, 0.05) } else { amount };
            let mut fresh = (**original).clone();
            q0model::ops::inset_faces(&mut fresh, faces, amount);
            studio.model.meshes[*mesh] = fresh;
            if let Some(op) = &mut studio.op {
                op.value = format!("{:.0}%", amount * 100.0);
            }
        }
        Start::Nodes { list } => {
            let pose = studio.mode == Mode::Pose && !studio.rest_edit;
            let mut locals = if pose {
                studio.model.sample_clip(studio.clip, studio.time)
            } else {
                studio.model.rest_locals()
            };
            for (node, original) in list {
                locals[usize::from(*node)] = *original;
            }
            let world = studio.model.world_matrices(&locals);
            for (node, original) in list {
                let n = usize::from(*node);
                let parent = studio.model.nodes[n]
                    .parent
                    .map(|p| world[usize::from(p)])
                    .unwrap_or(Mat4::IDENTITY);
                let global = parent * original.matrix();
                let moved = parent.inverse().unwrap_or(Mat4::IDENTITY) * t * global;
                let (translation, rotation, scale) = moved.decompose();
                let local = q0s_format::three::Trs {
                    translation,
                    rotation,
                    scale,
                };
                if pose {
                    studio.key_bone(*node, local);
                } else {
                    let target = &mut studio.model.nodes[n];
                    target.translation = translation;
                    target.rotation = rotation;
                    target.scale = scale;
                }
            }
        }
        Start::Loop { .. } => {}
    }
    studio.touch();
}

pub fn confirm(studio: &mut ModelStudio) {
    if studio.op.take().is_some() {
        studio.model.sanitize();
        studio.touch();
    }
}

pub fn cancel(studio: &mut ModelStudio) {
    if let Some(op) = studio.op.take() {
        studio.model = *op.before;
        studio.undo.pop();
        studio.touch();
    }
}

/// Points the camera at the selection, or the whole model.
pub fn frame_on(studio: &mut ModelStudio, everything: bool) {
    let centre = if everything {
        None
    } else {
        studio.selection_centre()
    };
    let (lo, hi) = studio
        .model
        .bounds()
        .unwrap_or((V3::splat(-1.0), V3::splat(1.0)));
    match centre {
        Some(c) => {
            studio.orbit.target = c;
            studio.orbit.distance = studio
                .orbit
                .distance
                .min(studio.size() * 1.5)
                .max(studio.size() * 0.15);
        }
        None => {
            studio.orbit.target = (lo + hi) * 0.5;
            studio.orbit.distance = (hi - lo).max_elem().max(1.0e-3) * 2.4;
        }
    }
}

/// Where a click on the floor or on a surface lands, in model space: a
/// surface point, or the floor (y = 0), or the camera's target.
pub fn place_point(studio: &ModelStudio, view: &View3d, at: Pos2) -> V3 {
    if let Some((_, _, _, point)) = pick_face(studio, view, at, None) {
        return point;
    }
    let Some((o, d)) = studio.ray(view, at) else {
        return studio.orbit.target;
    };
    match on_plane(o, d, V3::ZERO, V3::Y) {
        Some(p) if (p - o).dot(d) > 0.0 => p,
        _ => studio.orbit.target,
    }
}

/// Where the mouse ray meets the plane through `point` facing the camera.
pub fn on_view_plane(studio: &ModelStudio, view: &View3d, at: Pos2, point: V3) -> Option<V3> {
    let (o, d) = studio.ray(view, at)?;
    on_plane(o, d, point, studio.orbit.forward())
}

/// The screen line of the loop a cut through `edge` at `t` would make.
pub fn loop_preview(studio: &ModelStudio, view: &View3d, edge: [u32; 2], t: f32) -> Vec<[Pos2; 2]> {
    let Some(mesh) = studio.model.meshes.get(studio.mesh) else {
        return Vec::new();
    };
    let m = mesh_matrix(mesh, &studio.world());
    let (ring, quads) = q0model::ops::edge_ring(mesh, edge);
    if quads.is_empty() {
        return Vec::new();
    }
    // Each ring edge runs the same way as the hovered one; `t` is measured
    // along the hovered edge as it is stored.
    let t = if ring.contains(&edge) { t } else { 1.0 - t };
    let points: Vec<Option<Pos2>> = ring
        .iter()
        .map(|[a, b]| {
            let p =
                m.transform_point(mesh.positions[*a as usize].lerp(mesh.positions[*b as usize], t));
            studio.screen(view, p)
        })
        .collect();
    let mut lines = Vec::new();
    for pair in points.windows(2) {
        if let [Some(a), Some(b)] = pair {
            lines.push([*a, *b]);
        }
    }
    if ring.len() == quads.len() {
        if let (Some(Some(a)), Some(Some(b))) = (points.last(), points.first()) {
            lines.push([*a, *b]);
        }
    }
    lines
}

/// The part and face under a screen point, for painting.
pub fn face_under(studio: &ModelStudio, view: &View3d, at: Pos2) -> Option<(usize, usize)> {
    let only = (studio.mode == Mode::Edit).then_some(studio.mesh);
    pick_face(studio, view, at, only).map(|(m, f, _, _)| (m, f))
}
