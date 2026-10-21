//! Imports FLA files and reports what came in: errors, warnings, counts and
//! timing, plus a render of the stage's first frame and contact sheets of
//! every symbol, so broken artwork shows at a glance.
//!
//! cargo run --release -p q0editor --example fla_survey -- OUT_DIR FILE.fla...

use std::path::{Path, PathBuf};
use std::time::Instant;

use q0s_format::v2::{Asset, Layer, Placement, ProjectV2, Q0rg, Target, Transform2D, Tween};

const CELL: f32 = 160.0;
const COLUMNS: usize = 10;
const PER_SHEET: usize = 80;

fn save_png(path: &Path, rgba: &[u8], width: u32, height: u32) {
    image::save_buffer(path, rgba, width, height, image::ColorType::Rgba8)
        .unwrap_or_else(|error| eprintln!("could not save {}: {error}", path.display()));
}

fn main() {
    let mut args = std::env::args().skip(1);
    let out = PathBuf::from(args.next().expect("an output directory"));
    std::fs::create_dir_all(&out).expect("output directory");
    let only_report = std::env::var_os("NO_RENDER").is_some();
    for file in args {
        let path = PathBuf::from(&file);
        let stem = path
            .file_stem()
            .map(|stem| {
                stem.to_string_lossy()
                    .replace([' ', '\'', '&', '(', ')'], "_")
            })
            .unwrap_or_default();
        println!("=== {file}");
        let started = Instant::now();
        let imported = match std::panic::catch_unwind(|| q0fla::import_path(&path)) {
            Ok(Ok(imported)) => imported,
            Ok(Err(error)) => {
                println!("  ERROR after {:?}: {error}", started.elapsed());
                continue;
            }
            Err(_) => {
                println!("  PANIC after {:?}", started.elapsed());
                continue;
            }
        };
        println!(
            "  imported in {:?}: {}",
            started.elapsed(),
            imported.report.summary()
        );
        let project = &imported.project;
        let mut kinds = [0usize; 4];
        for asset in &project.assets {
            kinds[match asset {
                Asset::Vector(_) => 0,
                Asset::Bitmap(_) => 1,
                Asset::Q0v(_) => 2,
                Asset::Rig(_) => 3,
            }] += 1;
        }
        let placements = project
            .q0rgs
            .iter()
            .flat_map(|q0rg| &q0rg.layers)
            .map(|layer| layer.placements.len())
            .sum::<usize>();
        println!(
            "  stage {}x{} @{}fps, {} timelines, {} vectors, {} bitmaps, {} placements",
            project.meta.stage_width,
            project.meta.stage_height,
            project.meta.fps,
            project.q0rgs.len(),
            kinds[0],
            kinds[1],
            placements
        );
        let entry = project.meta.entry_q0rg_id;
        if let Some(main) = project.q0rgs.iter().find(|q0rg| q0rg.q0rg_id == entry) {
            println!(
                "  main timeline {:?}: {} frames, {} layers, {} placements",
                main.name,
                main.frame_count,
                main.layers.len(),
                main.layers
                    .iter()
                    .map(|l| l.placements.len())
                    .sum::<usize>()
            );
        }
        let missing = imported.missing_fonts();
        if !missing.is_empty() {
            println!(
                "  missing fonts: {:?}",
                missing.iter().map(|f| &f.name).collect::<Vec<_>>()
            );
        }
        let _ = std::fs::write(
            out.join(format!("{stem}_warnings.txt")),
            imported.report.warnings.join(
                "
",
            ),
        );
        let mut counts = std::collections::BTreeMap::<String, usize>::new();
        for warning in &imported.report.warnings {
            let key = warning.chars().take(70).collect::<String>();
            *counts.entry(key).or_default() += 1;
        }
        for (warning, count) in counts.iter().take(25) {
            println!("  warn x{count}: {warning}");
        }
        if counts.len() > 25 {
            println!("  ... {} more kinds of warning", counts.len() - 25);
        }
        if only_report {
            continue;
        }
        let started = Instant::now();
        let (w, h) = (
            u32::from(project.meta.stage_width.max(1)).min(1600),
            u32::from(project.meta.stage_height.max(1)).min(1600),
        );
        let stage = q0s_format::raster::rasterize_q0rg_frame(
            project,
            entry,
            0,
            w,
            h,
            1,
            [255, 255, 255, 255],
        );
        save_png(&out.join(format!("{stem}_stage.png")), &stage, w, h);
        println!("  stage rendered in {:?}", started.elapsed());
        if let Some(debug) = std::env::var("DEBUG_FRAME")
            .ok()
            .and_then(|v| v.parse::<u16>().ok())
        {
            list_big(project, entry, debug);
        }
        // Every main timeline frame, framed around what is on it.
        let frames = project
            .q0rgs
            .iter()
            .find(|q0rg| q0rg.q0rg_id == entry)
            .map_or(1, |q0rg| q0rg.frame_count)
            .min(24);
        for frame in 0..frames {
            let Some([min_x, min_y, max_x, max_y]) = frame_bounds(project, entry, frame) else {
                continue;
            };
            let (bw, bh) = ((max_x - min_x).max(1.0), (max_y - min_y).max(1.0));
            let scale = (1600.0 / bw).min(1600.0 / bh).min(4.0);
            let (w, h) = (
                (bw * scale).ceil() as u32 + 2,
                (bh * scale).ceil() as u32 + 2,
            );
            println!(
                "  frame {frame}: x {min_x:.0}..{max_x:.0}, y {min_y:.0}..{max_y:.0}, scale {scale:.2}"
            );
            let root = q0s_format::transform::Affine {
                a11: scale,
                a12: 0.0,
                a21: 0.0,
                a22: scale,
                tx: -min_x * scale + 1.0,
                ty: -min_y * scale + 1.0,
            };
            let rgba = q0s_format::raster::rasterize_q0rg_frame_with_root_transform(
                project,
                entry,
                frame,
                w,
                h,
                1,
                [255, 255, 255, 255],
                root,
                &q0s_format::rig::RigRuntimeOverrides::new(),
                &q0s_format::runtime_scene::RuntimeSceneState::new(),
            );
            save_png(&out.join(format!("{stem}_f{frame:02}.png")), &rgba, w, h);
        }
        contact_sheets(project, &out, &stem);
    }
}

/// Every symbol and drawing of `project`, fitted into cells of a grid.
fn contact_sheets(project: &ProjectV2, out: &Path, stem: &str) {
    let entry = project.meta.entry_q0rg_id;
    let mut targets = project
        .q0rgs
        .iter()
        .filter(|q0rg| q0rg.q0rg_id != entry)
        .map(|q0rg| (Target::Q0rg(q0rg.q0rg_id), q0rg.name.clone()))
        .collect::<Vec<_>>();
    if targets.is_empty() {
        targets = project
            .assets
            .iter()
            .map(|asset| (Target::Asset(asset.id()), format!("asset {}", asset.id())))
            .collect();
    }
    let mut names = Vec::new();
    for (sheet, chunk) in targets.chunks(PER_SHEET).enumerate() {
        let mut copy = project.clone();
        let sheet_id = copy
            .q0rgs
            .iter()
            .map(|q0rg| q0rg.q0rg_id)
            .max()
            .unwrap_or(0)
            + 1;
        let mut placements = Vec::new();
        for (index, (target, name)) in chunk.iter().enumerate() {
            names.push(format!("sheet {sheet} cell {index}: {name}"));
            let Some((min_x, min_y, max_x, max_y)) =
                q0editor::render::target_local_bbox(project, *target, None)
            else {
                continue;
            };
            let (bw, bh) = ((max_x - min_x).max(1.0), (max_y - min_y).max(1.0));
            let scale = (CELL * 0.9 / bw).min(CELL * 0.9 / bh).min(4.0);
            if !scale.is_finite() {
                continue;
            }
            let column = (index % COLUMNS) as f32;
            let row = (index / COLUMNS) as f32;
            let (cx, cy) = ((column + 0.5) * CELL, (row + 0.5) * CELL);
            placements.push(Placement {
                instance_id: 0,
                frame: 0,
                target: *target,
                transform: Transform2D {
                    tx: cx - (min_x + max_x) * 0.5 * scale,
                    ty: cy - (min_y + max_y) * 0.5 * scale,
                    sx: scale,
                    sy: scale,
                    ..Transform2D::IDENTITY
                },
                pivot: None,
                tween: Tween::None,
                fx: Default::default(),
            });
        }
        copy.q0rgs.push(Q0rg {
            q0rg_id: sheet_id,
            name: "sheet".into(),
            frame_count: 1,
            script: String::new(),
            layers: vec![Layer {
                layer_id: 1,
                timeline_frame_count: None,
                name: "sheet".into(),
                explicit_keyframes: vec![0],
                placements,
            }],
        });
        let rows = chunk.len().div_ceil(COLUMNS);
        let (w, h) = ((COLUMNS as f32 * CELL) as u32, (rows as f32 * CELL) as u32);
        let started = Instant::now();
        let rgba = q0s_format::raster::rasterize_q0rg_frame(
            &copy,
            sheet_id,
            0,
            w,
            h,
            1,
            [235, 235, 235, 255],
        );
        save_png(&out.join(format!("{stem}_sheet{sheet}.png")), &rgba, w, h);
        println!(
            "  sheet {sheet} ({} items) rendered in {:?}",
            chunk.len(),
            started.elapsed()
        );
    }
    let _ = std::fs::write(out.join(format!("{stem}_sheets.txt")), names.join("\n"));
}

/// The stage area what `q0rg_id` shows on `frame` covers.
fn frame_bounds(project: &ProjectV2, q0rg_id: u16, frame: u16) -> Option<[f32; 4]> {
    let q0rg = project.q0rgs.iter().find(|q0rg| q0rg.q0rg_id == q0rg_id)?;
    let mut bounds: Option<[f32; 4]> = None;
    for layer in &q0rg.layers {
        if !project.layer_is_visible(q0rg_id, layer.layer_id) {
            continue;
        }
        for state in q0s_format::raster::active_placement_states_at(layer, frame) {
            let Some(placement) = layer.placements.get(state.index) else {
                continue;
            };
            let Some((x0, y0, x1, y1)) =
                q0editor::render::target_local_bbox(project, placement.target, None)
            else {
                continue;
            };
            let affine = q0s_format::transform::Affine::from_transform(state.transform);
            for (x, y) in [(x0, y0), (x1, y0), (x1, y1), (x0, y1)] {
                let point = affine.apply(q0s_format::v2::Vec2::new(x, y));
                let b = bounds.get_or_insert([point.x, point.y, point.x, point.y]);
                b[0] = b[0].min(point.x);
                b[1] = b[1].min(point.y);
                b[2] = b[2].max(point.x);
                b[3] = b[3].max(point.y);
            }
        }
    }
    bounds
}

/// The biggest things on `frame`, by area, with where they are.
fn list_big(project: &ProjectV2, q0rg_id: u16, frame: u16) {
    let Some(q0rg) = project.q0rgs.iter().find(|q0rg| q0rg.q0rg_id == q0rg_id) else {
        return;
    };
    let mut rows = Vec::new();
    for layer in &q0rg.layers {
        for state in q0s_format::raster::active_placement_states_at(layer, frame) {
            let placement = &layer.placements[state.index];
            let Some((x0, y0, x1, y1)) =
                q0editor::render::target_local_bbox(project, placement.target, None)
            else {
                continue;
            };
            let affine = q0s_format::transform::Affine::from_transform(state.transform);
            let a = affine.apply(q0s_format::v2::Vec2::new(x0, y0));
            let b = affine.apply(q0s_format::v2::Vec2::new(x1, y1));
            let name = match placement.target {
                Target::Q0rg(id) => project
                    .q0rgs
                    .iter()
                    .find(|q| q.q0rg_id == id)
                    .map_or(String::new(), |q| format!("symbol {id} {:?}", q.name)),
                Target::Asset(id) => format!("asset {id}"),
            };
            rows.push((
                (b.x - a.x).abs() * (b.y - a.y).abs(),
                name,
                a,
                b,
                layer.name.clone(),
                state.transform,
            ));
        }
    }
    rows.sort_by(|a, b| b.0.total_cmp(&a.0));
    for (area, name, a, b, layer, transform) in rows.iter().take(8) {
        println!("    big {area:.0}: {name} on {layer:?} from ({:.0},{:.0}) to ({:.0},{:.0}) {transform:?}", a.x, a.y, b.x, b.y);
    }
}
