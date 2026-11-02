//! Editing commands: what the action buttons, keys and the right-click
//! menu do to the model, each one undo step.

use std::collections::BTreeSet;

use egui::Pos2;
use q0model::ops::{self, Axis, Projection};
use q0s_format::three::model::{mesh_matrix, Node};
use q0s_format::three::{Mat4, Quat, V3};

use super::{view, Constraint, Element, Mode, ModelStudio, OpKind};

/// Editing commands without a mouse drag.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Command {
    Fill,
    Merge,
    Weld,
    Delete,
    DeleteObjects,
    Duplicate,
    Linked,
    SelectAll,
    SelectNone,
    Subdivide,
    Smooth,
    Triangulate,
    Flip,
    FixNormals,
    ShadeSmooth,
    ShadeFlat,
    Mirror(Axis),
    Uvs(Projection),
    KeyBone,
    DeleteKey,
    AddBone,
    DeleteBone,
    Bind,
    Unbind,
    Join,
    KeyAll,
}

pub fn label_of(command: Command) -> &'static str {
    match command {
        Command::Fill => "Fill (F)",
        Command::Merge => "Merge (M)",
        Command::Weld => "Weld close points",
        Command::Delete => "Delete (X)",
        Command::DeleteObjects => "Delete parts (X)",
        Command::Duplicate => "Duplicate (Shift D)",
        Command::Linked => "Pick linked (L)",
        Command::SelectAll => "Pick all (A)",
        Command::SelectNone => "Pick none (Alt A)",
        Command::Subdivide => "Subdivide",
        Command::Smooth => "Smooth (subdivide round)",
        Command::Triangulate => "Triangulate",
        Command::Flip => "Flip faces",
        Command::FixNormals => "Faces outward",
        Command::ShadeSmooth => "Shade smooth",
        Command::ShadeFlat => "Shade flat",
        Command::Mirror(Axis::X) => "Mirror x",
        Command::Mirror(Axis::Y) => "Mirror y",
        Command::Mirror(Axis::Z) => "Mirror z",
        Command::Uvs(Projection::Box) => "Box",
        Command::Uvs(Projection::Front) => "Front",
        Command::Uvs(Projection::Cylinder) => "Around",
        Command::KeyBone => "Key bone (K)",
        Command::DeleteKey => "Delete key",
        Command::AddBone => "Add bone",
        Command::DeleteBone => "Delete bone",
        Command::Bind => "Bind parts to bones",
        Command::Unbind => "Unbind",
        Command::Join => "Join parts",
        Command::KeyAll => "Key all bones",
    }
}

/// Runs a command on the studio's model, as one undo step.
pub fn run(studio: &mut ModelStudio, command: Command) {
    match command {
        Command::SelectAll => {
            match studio.mode {
                Mode::Object => studio.objects = (0..studio.model.meshes.len()).collect(),
                Mode::Edit => {
                    let Some(mesh) = studio.model.meshes.get(studio.mesh) else {
                        return;
                    };
                    studio.verts = (0..mesh.positions.len() as u32).collect();
                    studio.edges = ops::edges(mesh).into_iter().collect();
                    studio.faces = (0..mesh.faces.len()).collect();
                }
                Mode::Pose => {}
            }
            return;
        }
        Command::SelectNone => {
            studio.objects.clear();
            studio.clear_elements();
            studio.bone = None;
            return;
        }
        Command::Linked => {
            let Some(mesh) = studio.model.meshes.get(studio.mesh) else {
                return;
            };
            let faces = ops::linked_faces(
                mesh,
                &studio
                    .working_faces()
                    .into_iter()
                    .chain(seed_faces(studio))
                    .collect(),
            );
            studio.faces = faces.clone();
            studio.verts = ops::face_verts(mesh, &faces);
            studio.edges = ops::edges(mesh)
                .into_iter()
                .filter(|[a, b]| studio.verts.contains(a) && studio.verts.contains(b))
                .collect();
            return;
        }
        _ => {}
    }
    studio.snapshot();
    let mesh_index = studio.mesh;
    let faces = studio.working_faces();
    let verts = studio.moving_verts();
    let all_faces: BTreeSet<usize> = studio
        .model
        .meshes
        .get(mesh_index)
        .map(|m| (0..m.faces.len()).collect())
        .unwrap_or_default();
    let faces_or_all = if faces.is_empty() {
        all_faces.clone()
    } else {
        faces.clone()
    };
    let mut message = String::new();
    match command {
        Command::Fill => {
            if let Some(mesh) = studio.model.meshes.get_mut(mesh_index) {
                match ops::fill(mesh, &verts) {
                    Some(face) => {
                        studio.faces = BTreeSet::from([face]);
                        message = "filled".into();
                    }
                    None => message = "pick three or more points to fill".into(),
                }
            }
        }
        Command::Merge => {
            if let Some(mesh) = studio.model.meshes.get_mut(mesh_index) {
                if verts.len() >= 2 {
                    ops::merge_verts(mesh, &verts);
                    studio.clear_elements();
                    message = format!("merged {} points", verts.len());
                }
            }
        }
        Command::Weld => {
            let size = studio.size();
            if let Some(mesh) = studio.model.meshes.get_mut(mesh_index) {
                let n = ops::merge_by_distance(mesh, size * 1.0e-4);
                studio.clear_elements();
                message = format!("welded {n} points");
            }
        }
        Command::Delete => {
            if let Some(mesh) = studio.model.meshes.get_mut(mesh_index) {
                match studio.element {
                    Element::Face => ops::delete_faces(mesh, &faces),
                    _ => ops::delete_verts(mesh, &verts),
                }
                studio.clear_elements();
            }
        }
        Command::DeleteObjects => {
            let doomed: Vec<usize> = studio.objects.iter().rev().copied().collect();
            for m in doomed {
                if m < studio.model.meshes.len() {
                    studio.model.meshes.remove(m);
                }
            }
            studio.objects.clear();
            studio.mesh = 0;
        }
        Command::Duplicate => match studio.mode {
            Mode::Edit => {
                if let Some(mesh) = studio.model.meshes.get_mut(mesh_index) {
                    let copies = ops::duplicate_faces(mesh, &faces);
                    studio.faces = copies.clone();
                    studio.verts = ops::face_verts(mesh, &copies);
                    studio.edges.clear();
                    studio.element = Element::Face;
                }
            }
            Mode::Object => {
                let mut fresh = BTreeSet::new();
                for m in studio.objects.clone() {
                    let mut copy = studio.model.meshes[m].clone();
                    copy.name = format!("{} copy", copy.name);
                    if let Some(node) = copy.node {
                        let mut n = studio.model.nodes[usize::from(node)].clone();
                        n.name = copy.name.clone();
                        studio.model.nodes.push(n);
                        copy.node = Some((studio.model.nodes.len() - 1) as u16);
                    }
                    studio.model.meshes.push(copy);
                    fresh.insert(studio.model.meshes.len() - 1);
                }
                studio.objects = fresh;
            }
            Mode::Pose => {}
        },
        Command::Subdivide | Command::Smooth => {
            if let Some(mesh) = studio.model.meshes.get_mut(mesh_index) {
                let smooth = command == Command::Smooth;
                let target = if smooth {
                    all_faces.clone()
                } else {
                    faces_or_all.clone()
                };
                let new = ops::subdivide(mesh, &target, smooth);
                studio.faces = new;
                studio.verts.clear();
                studio.edges.clear();
                message = format!("{} faces", mesh.faces.len());
            }
        }
        Command::Triangulate => {
            if let Some(mesh) = studio.model.meshes.get_mut(mesh_index) {
                ops::triangulate(mesh, &faces_or_all);
                studio.clear_elements();
            }
        }
        Command::Flip => {
            if let Some(mesh) = studio.model.meshes.get_mut(mesh_index) {
                ops::flip_faces(mesh, &faces_or_all);
            }
        }
        Command::FixNormals => {
            if let Some(mesh) = studio.model.meshes.get_mut(mesh_index) {
                ops::fix_normals(mesh);
            }
        }
        Command::ShadeSmooth | Command::ShadeFlat => {
            let smooth = command == Command::ShadeSmooth;
            let targets: Vec<usize> = match studio.mode {
                Mode::Object => studio.objects.iter().copied().collect(),
                _ => vec![mesh_index],
            };
            for m in targets {
                if let Some(mesh) = studio.model.meshes.get_mut(m) {
                    let set = if studio.mode == Mode::Edit && !faces.is_empty() {
                        faces.clone()
                    } else {
                        (0..mesh.faces.len()).collect()
                    };
                    ops::set_smooth(mesh, &set, smooth);
                }
            }
        }
        Command::Mirror(axis) => {
            if let Some(mesh) = studio.model.meshes.get_mut(mesh_index) {
                ops::mirror(mesh, axis);
                studio.clear_elements();
            }
        }
        Command::Uvs(projection) => {
            if let Some(mesh) = studio.model.meshes.get_mut(mesh_index) {
                ops::project_uvs(mesh, &faces_or_all, projection);
                message = "texture positions set".into();
            }
        }
        Command::KeyBone => {
            if let Some(bone) = studio.bone {
                let local = studio.locals()[usize::from(bone)];
                studio.key_bone(bone, local);
                message = format!("keyed at {:.2}s", studio.time);
            }
        }
        Command::DeleteKey => {
            if let (Some(bone), Some(clip)) = (studio.bone, studio.clip) {
                let time = studio.time;
                if let Some(clip) = studio.model.clips.get_mut(clip) {
                    for channel in clip.channels.iter_mut().filter(|c| c.node == bone) {
                        if let Some(k) =
                            channel.times.iter().position(|t| (t - time).abs() < 1.0e-3)
                        {
                            channel.remove_key(k);
                        }
                    }
                    clip.channels.retain(|c| !c.times.is_empty());
                }
            }
        }
        Command::AddBone => {
            let world = studio.model.rest_world();
            let length = studio.size() * 0.25;
            let (parent, head) = match studio.bone {
                Some(b) => {
                    let node = &studio.model.nodes[usize::from(b)];
                    (Some(b), V3::new(0.0, node.length.max(length * 0.2), 0.0))
                }
                None => (
                    None,
                    studio.selection_centre().unwrap_or(studio.orbit.target),
                ),
            };
            let mut node = Node::new(format!(
                "bone{}",
                studio.model.nodes.iter().filter(|n| n.bone).count() + 1
            ));
            node.bone = true;
            node.length = length;
            node.parent = parent;
            node.translation = head;
            let _ = world;
            studio.model.nodes.push(node);
            studio.bone = Some((studio.model.nodes.len() - 1) as u16);
            studio.show_bones = true;
            message = "bone added: move it with G, turn it with R".into();
        }
        Command::DeleteBone => {
            if let Some(bone) = studio.bone {
                remove_node(studio, bone);
                studio.bone = None;
            }
        }
        Command::Bind => {
            let bones: Vec<u16> = studio
                .model
                .nodes
                .iter()
                .enumerate()
                .filter(|(_, n)| n.bone)
                .map(|(i, _)| i as u16)
                .collect();
            if bones.is_empty() {
                message = "add bones first".into();
            } else {
                let targets: Vec<usize> = if studio.objects.is_empty() {
                    (0..studio.model.meshes.len()).collect()
                } else {
                    studio.objects.iter().copied().collect()
                };
                for m in &targets {
                    ops::auto_weights(&mut studio.model, *m, &bones);
                }
                message = format!("{} part(s) follow {} bone(s)", targets.len(), bones.len());
            }
        }
        Command::Unbind => {
            for mesh in &mut studio.model.meshes {
                mesh.skin = None;
            }
        }
        Command::Join => {
            let picked: Vec<usize> = studio.objects.iter().copied().collect();
            if picked.len() >= 2 {
                let world = studio.model.rest_world();
                let first = picked[0];
                let base = mesh_matrix(&studio.model.meshes[first], &world);
                let base_inverse = base.inverse().unwrap_or(Mat4::IDENTITY);
                for &m in picked.iter().skip(1) {
                    let other = studio.model.meshes[m].clone();
                    let into = base_inverse * mesh_matrix(&other, &world);
                    let target = &mut studio.model.meshes[first];
                    let offset = target.positions.len() as u32;
                    target
                        .positions
                        .extend(other.positions.iter().map(|p| into.transform_point(*p)));
                    let mut polys = ops::polys(target);
                    for p in ops::polys(&other) {
                        polys.push(ops::Poly {
                            verts: p.verts.iter().map(|v| v + offset).collect(),
                            ..p
                        });
                    }
                    ops::set_polys(target, polys);
                    target.skin = None;
                    if !target.colors.is_empty() || !other.colors.is_empty() {
                        target.colors.clear();
                    }
                }
                for &m in picked.iter().skip(1).rev() {
                    studio.model.meshes.remove(m);
                }
                studio.objects = BTreeSet::from([first]);
            }
        }
        Command::KeyAll => {
            let locals = studio.locals();
            let bones: Vec<u16> = studio
                .model
                .nodes
                .iter()
                .enumerate()
                .filter(|(_, n)| n.bone)
                .map(|(i, _)| i as u16)
                .collect();
            for bone in &bones {
                studio.key_bone(*bone, locals[usize::from(*bone)]);
            }
            message = format!("keyed {} bones at {:.2}s", bones.len(), studio.time);
        }
        Command::SelectAll | Command::SelectNone | Command::Linked => {}
    }
    studio.model.sanitize();
    studio.touch();
    if !message.is_empty() {
        studio.status = message;
    }
}

/// Faces under the picked points, so "linked" works from points too.
fn seed_faces(studio: &ModelStudio) -> BTreeSet<usize> {
    let Some(mesh) = studio.model.meshes.get(studio.mesh) else {
        return BTreeSet::new();
    };
    let verts = studio.moving_verts();
    mesh.faces
        .iter()
        .enumerate()
        .filter(|(_, f)| mesh.face_verts(f).iter().any(|v| verts.contains(v)))
        .map(|(i, _)| i)
        .collect()
}

/// Removes a node, giving its children its parent and renumbering every
/// reference.
fn remove_node(studio: &mut ModelStudio, node: u16) {
    let model = &mut studio.model;
    let index = usize::from(node);
    if index >= model.nodes.len() {
        return;
    }
    let parent = model.nodes[index].parent;
    let world = model.rest_world();
    // Children keep their place in the world.
    for i in 0..model.nodes.len() {
        if model.nodes[i].parent == Some(node) {
            let parent_world = parent
                .map(|p| world[usize::from(p)])
                .unwrap_or(Mat4::IDENTITY);
            let local = parent_world.inverse().unwrap_or(Mat4::IDENTITY) * world[i];
            let (t, r, s) = local.decompose();
            let child = &mut model.nodes[i];
            child.translation = t;
            child.rotation = r;
            child.scale = s;
            child.parent = parent;
        }
    }
    model.nodes.remove(index);
    let renumber = |n: u16| if n > node { n - 1 } else { n };
    for n in &mut model.nodes {
        n.parent = n.parent.map(renumber);
    }
    for mesh in &mut model.meshes {
        if mesh.node == Some(node) {
            mesh.node = parent;
        }
        mesh.node = mesh.node.map(renumber);
        if let Some(skin) = &mut mesh.skin {
            if skin.joints.contains(&node) {
                mesh.skin = None;
            } else {
                for j in &mut skin.joints {
                    *j = renumber(*j);
                }
            }
        }
    }
    for clip in &mut model.clips {
        clip.channels.retain(|c| c.node != node);
        for c in &mut clip.channels {
            c.node = renumber(c.node);
        }
    }
}

/// Pulls the picked faces out, then moves them along their normal with the
/// mouse. Returns whether it started.
pub fn extrude(studio: &mut ModelStudio, mouse: Pos2, dragging: bool) -> bool {
    let faces = studio.working_faces();
    if faces.is_empty() {
        studio.status = "pick faces to extrude".into();
        return false;
    }
    let before = studio.model.clone();
    let mesh_index = studio.mesh;
    let normal = {
        let mesh = &studio.model.meshes[mesh_index];
        let world = studio.model.rest_world();
        let m = mesh_matrix(mesh, &world);
        faces
            .iter()
            .fold(V3::ZERO, |s, f| {
                s + m.transform_vector(mesh.face_normal(&mesh.faces[*f]))
            })
            .normalized()
    };
    let top = ops::extrude_faces(&mut studio.model.meshes[mesh_index], &faces, 0.0);
    studio.faces = top.clone();
    studio.verts = ops::face_verts(&studio.model.meshes[mesh_index], &top);
    studio.edges.clear();
    studio.element = Element::Face;
    studio.touch();
    let constraint = if normal == V3::ZERO {
        Constraint::Free
    } else {
        Constraint::Axis(normal)
    };
    if !view::begin(studio, OpKind::Grab, mouse, constraint, "out") {
        studio.model = before;
        return false;
    }
    // One undo step back to before the extrusion.
    studio.undo.pop();
    studio.undo.push(before.clone());
    if let Some(op) = &mut studio.op {
        *op.before = before;
        op.dragging = dragging;
    }
    true
}

/// Adds the tool's shape at `at` (model space): a new part, or more of the
/// part being shaped.
pub fn add_shape_at(studio: &mut ModelStudio, at: V3) {
    studio.snapshot();
    let size = studio.shape_size();
    let primitive = studio.settings.shape;
    let mut mesh = primitive.build(size, studio.settings.detail);
    // Shapes sit on what they are put on, not half sunk into it.
    let lift = if primitive == q0model::prim::Primitive::Plane {
        0.0
    } else {
        size * 0.5
    };
    let at = at + V3::new(0.0, lift, 0.0);
    match studio.mode {
        Mode::Edit if studio.mesh < studio.model.meshes.len() => {
            let world = studio.model.rest_world();
            let target = &mut studio.model.meshes[studio.mesh];
            let inverse = mesh_matrix(target, &world)
                .inverse()
                .unwrap_or(Mat4::IDENTITY);
            let offset = target.positions.len() as u32;
            target.positions.extend(
                mesh.positions
                    .iter()
                    .map(|p| inverse.transform_point(*p + at)),
            );
            let mut polys = ops::polys(target);
            let first_new = polys.len();
            for p in ops::polys(&mesh) {
                polys.push(ops::Poly {
                    verts: p.verts.iter().map(|v| v + offset).collect(),
                    ..p
                });
            }
            ops::set_polys(target, polys);
            studio.faces = (first_new..target.faces.len()).collect();
            studio.verts = ops::face_verts(target, &studio.faces);
            studio.element = Element::Face;
        }
        _ => {
            let mut node = Node::new(primitive.label());
            node.translation = at;
            studio.model.nodes.push(node);
            mesh.node = Some((studio.model.nodes.len() - 1) as u16);
            mesh.name = format!("{} {}", primitive.label(), studio.model.meshes.len() + 1);
            studio.model.meshes.push(mesh);
            studio.mesh = studio.model.meshes.len() - 1;
            studio.objects = BTreeSet::from([studio.mesh]);
            if studio.mode == Mode::Pose {
                studio.set_mode(Mode::Object);
            }
        }
    }
    studio.model.sanitize();
    studio.touch();
    studio.status = format!("added a {}", primitive.label().to_lowercase());
}

/// Gives a face (Shape mode) or a whole part (Parts mode) the current
/// material.
pub fn paint(studio: &mut ModelStudio, part: usize, face: usize) {
    let material = studio
        .material
        .min(studio.model.materials.len().saturating_sub(1)) as u16;
    let whole = studio.mode == Mode::Object;
    let Some(mesh) = studio.model.meshes.get_mut(part) else {
        return;
    };
    let faces: BTreeSet<usize> = if whole {
        (0..mesh.faces.len()).collect()
    } else {
        BTreeSet::from([face])
    };
    if faces
        .iter()
        .all(|f| mesh.faces.get(*f).is_some_and(|x| x.material == material))
    {
        return;
    }
    ops::set_material(mesh, &faces, material);
    studio.touch();
}

/// Adds a bone from `head` to `tail` (model space, rest pose) under
/// `parent`, and picks it.
pub fn add_bone(studio: &mut ModelStudio, head: V3, tail: V3, parent: Option<u16>) {
    let length = (tail - head).length();
    if length < studio.size() * 0.005 {
        return;
    }
    studio.snapshot();
    let world = studio.model.rest_world();
    let parent_world = parent
        .and_then(|p| world.get(usize::from(p)).copied())
        .unwrap_or(Mat4::IDENTITY);
    let turn = Quat::between(V3::Y, (tail - head).normalized());
    let global = Mat4::translation(head) * Mat4::from_quat(turn);
    let local = parent_world.inverse().unwrap_or(Mat4::IDENTITY) * global;
    let (translation, rotation, _) = local.decompose();
    let count = studio.model.nodes.iter().filter(|n| n.bone).count();
    let mut node = Node::new(format!("bone{}", count + 1));
    node.bone = true;
    node.parent = parent;
    node.translation = translation;
    node.rotation = rotation;
    node.length = length;
    studio.model.nodes.push(node);
    studio.bone = Some((studio.model.nodes.len() - 1) as u16);
    studio.show_bones = true;
    studio.touch();
}

/// Where the next drawn bone starts: the picked bone's tip, or `fallback`.
pub fn bone_start(studio: &ModelStudio, fallback: V3) -> (V3, Option<u16>) {
    let world = studio.model.rest_world();
    match studio.bone {
        Some(b) => {
            let node = &studio.model.nodes[usize::from(b)];
            let tip = world[usize::from(b)].transform_point(V3::new(0.0, node.length, 0.0));
            (tip, Some(b))
        }
        None => (fallback, None),
    }
}
