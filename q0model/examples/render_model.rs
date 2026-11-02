//! Imports a model file and draws it with the software renderer.
//! Usage: render_model <model file> <out.png> [clip seconds...]

use std::sync::Arc;

use q0s_format::three::draw::{push_model, Frame3d, View3d};
use q0s_format::three::frame::model_output_bounds;
use q0s_format::three::scene::{CameraPose, LightRig, Space3d};
use q0s_format::three::{Mat4, Quat, V3};
use q0s_format::transform::Affine;

fn main() {
    let mut args = std::env::args().skip(1);
    let input = args.next().expect("model file");
    let output = args.next().expect("out png");
    let times: Vec<f32> = args.filter_map(|a| a.parse().ok()).collect();
    let times = if times.is_empty() { vec![0.0] } else { times };
    let started = std::time::Instant::now();
    let imported = q0model::import_path(std::path::Path::new(&input), 300.0).expect("import");
    let model = Arc::new(imported.model);
    println!(
        "{}: {} meshes, {} verts, {} tris, {} nodes, {} clips {:?}, {} textures, import {:?}",
        imported.name,
        model.meshes.len(),
        model.vertex_count(),
        model.triangle_count(),
        model.nodes.len(),
        model.clips.len(),
        model
            .clips
            .iter()
            .map(|c| (c.name.clone(), c.duration))
            .collect::<Vec<_>>(),
        model.textures.len(),
        started.elapsed()
    );
    for w in &imported.warnings {
        println!("warning: {w}");
    }
    if std::env::var("DEBUG_JOINTS").is_ok() {
        debug_joints(&model);
    }
    let (w, h) = (400u32, 400u32);
    let camera = CameraPose::stage(w as u16, h as u16);
    let mut sheet = vec![0u8; (w * h * 4) as usize * times.len()];
    for (i, time) in times.iter().enumerate() {
        let space = Space3d {
            clip: if model.clips.is_empty() || *time < 0.0 {
                q0s_format::three::NO_CLIP
            } else {
                0
            },
            clip_time: *time,
            clip_speed: 0.0,
            ..Space3d::default()
        };
        let world = q0s_format::three::draw::pose_model(&model, &space, 0.0, 24, None);
        let view = View3d::new(
            camera,
            false,
            [w as f32, h as f32],
            Affine::IDENTITY,
            [w, h],
        );
        // Centre the model: measure, then shift.
        let probe = Mat4::from_quat(Quat::from_euler_deg(0.0, 30.0, 0.0));
        let bounds =
            model_output_bounds(&model, &probe, &world, &view).unwrap_or([0.0, 0.0, 0.0, 0.0]);
        let shift = V3::new(
            w as f32 * 0.5 - (bounds[0] + bounds[2]) * 0.5,
            h as f32 * 0.5 - (bounds[1] + bounds[3]) * 0.5,
            0.0,
        );
        let placement = Mat4::translation(shift) * probe;
        let mut items = Vec::new();
        push_model(&mut items, &model, &placement, &world, 1.0, true);
        let frame = Frame3d {
            view,
            rig: LightRig::default_for(&camera),
            items,
            overlay: None,
            clear: None,
        };
        let t = std::time::Instant::now();
        let pixels = q0s_format::three::soft::render(&frame, 2);
        println!("frame {time}s rendered in {:?}", t.elapsed());
        for y in 0..h as usize {
            for x in 0..w as usize {
                let src = (y * w as usize + x) * 4;
                let dst = (y * (w as usize * times.len()) + i * w as usize + x) * 4;
                // Over a light grey card.
                let a = u32::from(pixels[src + 3]);
                for c in 0..3 {
                    sheet[dst + c] =
                        (u32::from(pixels[src + c]) + 230 * (255 - a) / 255).min(255) as u8;
                }
                sheet[dst + 3] = 255;
            }
        }
    }
    image::save_buffer(
        &output,
        &sheet,
        w * times.len() as u32,
        h,
        image::ColorType::Rgba8,
    )
    .expect("save");
    println!("saved {output}");
}

#[allow(dead_code)]
fn debug_joints(model: &q0s_format::three::ModelData) {
    let rest = model.rest_world();
    let posed = model.world_matrices(&model.sample_clip(Some(0), 0.0));
    let mesh = &model.meshes[0];
    let skin = mesh.skin.as_ref().unwrap();
    for (j, (node, inverse)) in skin
        .joints
        .iter()
        .zip(&skin.inverse_bind)
        .enumerate()
        .take(8)
    {
        let n = usize::from(*node);
        let a = (rest[n] * *inverse).translation_part();
        let b = (posed[n] * *inverse).translation_part();
        println!(
            "joint {j} {} rest-joint {:?} posed-joint {:?} rest-world {:?} posed-world {:?} local-t {:?}",
            model.nodes[n].name,
            a,
            b,
            rest[n].translation_part(),
            posed[n].translation_part(),
            model.nodes[n].translation
        );
    }
}
