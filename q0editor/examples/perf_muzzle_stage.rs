use egui::{pos2, Rect};
use q0editor::render::{render_stage, StageView, TextureCache};
use std::time::Instant;

fn traverse(
    project: &q0s_format::v2::ProjectV2,
    q0rg_id: u16,
    frame: u16,
    depth: u8,
    counts: &mut (usize, usize, usize),
) {
    if depth > 8 {
        return;
    }
    let Some(q) = project.q0rgs.iter().find(|q| q.q0rg_id == q0rg_id) else {
        return;
    };
    let local = if q.frame_count > 0 {
        frame % q.frame_count
    } else {
        0
    };
    counts.0 += 1;
    for layer in &q.layers {
        if !project.layer_is_visible(q0rg_id, layer.layer_id) {
            continue;
        }
        let states = q0s_format::raster::active_placement_states_at(layer, local);
        counts.1 += states.len();
        for state in states {
            let Some(placement) = layer.placements.get(state.index) else {
                continue;
            };
            match placement.target {
                q0s_format::v2::Target::Asset(_) => counts.2 += 1,
                q0s_format::v2::Target::Q0rg(child) if child != q0rg_id => {
                    let child_frame = q0s_format::v2::resolve_q0rg_playback_frame(
                        project,
                        child,
                        local,
                        placement.frame,
                        state.fx.q0rg_playback,
                    );
                    traverse(project, child, child_frame, depth + 1, counts);
                }
                _ => {}
            }
        }
    }
}

fn fx_stats(
    project: &q0s_format::v2::ProjectV2,
    q0rg_id: u16,
    frame: u16,
    depth: u8,
    stats: &mut [usize; 10],
) {
    if depth > 8 {
        return;
    }
    let Some(q) = project.q0rgs.iter().find(|q| q.q0rg_id == q0rg_id) else {
        return;
    };
    let local = if q.frame_count > 0 {
        frame % q.frame_count
    } else {
        0
    };
    for layer in &q.layers {
        if !project.layer_is_visible(q0rg_id, layer.layer_id) {
            continue;
        }
        for state in q0s_format::raster::active_placement_states_at(layer, local) {
            let Some(p) = layer.placements.get(state.index) else {
                continue;
            };
            stats[0] += 1;
            if state.fx.is_identity() {
                stats[1] += 1;
            }
            if state.fx.opacity < 0.999_999 {
                stats[2] += 1;
                if matches!(p.target, q0s_format::v2::Target::Q0rg(_)) {
                    stats[8] += 1
                } else {
                    stats[9] += 1
                }
            }
            if state.fx.blend_mode != q0s_format::v2::BlendMode::Normal {
                stats[3] += 1;
            }
            if state
                .fx
                .blur
                .is_some_and(|v| v.radius_x.max(v.radius_y) > 1e-6)
            {
                stats[4] += 1;
            }
            if state.fx.glow.is_some() {
                stats[5] += 1;
            }
            if state.fx.shadow.is_some() {
                stats[6] += 1;
            }
            if let q0s_format::v2::Target::Q0rg(child) = p.target {
                stats[7] += 1;
                if child != q0rg_id {
                    let cf = q0s_format::v2::resolve_q0rg_playback_frame(
                        project,
                        child,
                        local,
                        p.frame,
                        state.fx.q0rg_playback,
                    );
                    fx_stats(project, child, cf, depth + 1, stats);
                }
            }
        }
    }
}

fn q0rg_plain_support(
    project: &q0s_format::v2::ProjectV2,
    qid: u16,
    frame: u16,
    depth: u8,
) -> bool {
    if depth > 8 {
        return false;
    }
    let Some(q) = project.q0rgs.iter().find(|q| q.q0rg_id == qid) else {
        return false;
    };
    let local = if q.frame_count > 0 {
        frame % q.frame_count
    } else {
        0
    };
    for l in &q.layers {
        if !project.layer_is_visible(qid, l.layer_id) {
            continue;
        }
        for st in q0s_format::raster::active_placement_states_at(l, local) {
            if !st.fx.is_identity() {
                return false;
            }
            let Some(p) = l.placements.get(st.index) else {
                return false;
            };
            match p.target {
                q0s_format::v2::Target::Asset(id) => {
                    let Some(a) = project.assets.iter().find(|a| match a {
                        q0s_format::v2::Asset::Bitmap(v) => v.asset_id == id,
                        q0s_format::v2::Asset::Vector(v) => v.asset_id == id,
                        q0s_format::v2::Asset::Q0v(v) => v.asset_id == id,
                        q0s_format::v2::Asset::Rig(v) => v.asset_id == id,
                    }) else {
                        return false;
                    };
                    match a {
                        q0s_format::v2::Asset::Vector(v) => {
                            if project.asset_appearances.contains_key(&id) || v.stroke.is_some() {
                                return false;
                            }
                        }
                        _ => return false,
                    }
                }
                q0s_format::v2::Target::Q0rg(child) if child != qid => {
                    let cf = q0s_format::v2::resolve_q0rg_playback_frame(
                        project,
                        child,
                        local,
                        p.frame,
                        st.fx.q0rg_playback,
                    );
                    if !q0rg_plain_support(project, child, cf, depth + 1) {
                        return false;
                    }
                }
                _ => return false,
            }
        }
    }
    true
}
fn opacity_support_stats(
    project: &q0s_format::v2::ProjectV2,
    qid: u16,
    frame: u16,
    depth: u8,
    stats: &mut [usize; 3],
) {
    if depth > 8 {
        return;
    }
    let Some(q) = project.q0rgs.iter().find(|q| q.q0rg_id == qid) else {
        return;
    };
    let local = if q.frame_count > 0 {
        frame % q.frame_count
    } else {
        0
    };
    for l in &q.layers {
        if !project.layer_is_visible(qid, l.layer_id) {
            continue;
        }
        for st in q0s_format::raster::active_placement_states_at(l, local) {
            let Some(p) = l.placements.get(st.index) else {
                continue;
            };
            if st.fx.opacity < 0.999_999 {
                if let q0s_format::v2::Target::Q0rg(child) = p.target {
                    stats[0] += 1;
                    let cf = q0s_format::v2::resolve_q0rg_playback_frame(
                        project,
                        child,
                        local,
                        p.frame,
                        st.fx.q0rg_playback,
                    );
                    if q0rg_plain_support(project, child, cf, depth + 1) {
                        stats[1] += 1
                    } else {
                        stats[2] += 1
                    }
                }
            }
            if let q0s_format::v2::Target::Q0rg(child) = p.target {
                if child != qid {
                    let cf = q0s_format::v2::resolve_q0rg_playback_frame(
                        project,
                        child,
                        local,
                        p.frame,
                        st.fx.q0rg_playback,
                    );
                    opacity_support_stats(project, child, cf, depth + 1, stats);
                }
            }
        }
    }
}

fn q0rg_complex_flags(
    project: &q0s_format::v2::ProjectV2,
    qid: u16,
    frame: u16,
    depth: u8,
    flags: &mut [usize; 7],
) {
    if depth > 8 {
        flags[6] += 1;
        return;
    }
    let Some(q) = project.q0rgs.iter().find(|q| q.q0rg_id == qid) else {
        return;
    };
    let local = if q.frame_count > 0 {
        frame % q.frame_count
    } else {
        0
    };
    for l in &q.layers {
        if !project.layer_is_visible(qid, l.layer_id) {
            continue;
        }
        for st in q0s_format::raster::active_placement_states_at(l, local) {
            let Some(p) = l.placements.get(st.index) else {
                continue;
            };
            if !st.fx.is_identity() {
                flags[0] += 1;
            }
            match p.target {
                q0s_format::v2::Target::Asset(id) => {
                    if let Some(a) = project.assets.iter().find(|a| match a {
                        q0s_format::v2::Asset::Bitmap(v) => v.asset_id == id,
                        q0s_format::v2::Asset::Vector(v) => v.asset_id == id,
                        q0s_format::v2::Asset::Q0v(v) => v.asset_id == id,
                        q0s_format::v2::Asset::Rig(v) => v.asset_id == id,
                    }) {
                        match a {
                            q0s_format::v2::Asset::Vector(v) => {
                                if v.stroke.is_some() {
                                    flags[1] += 1;
                                }
                                if let Some(ap) = project.asset_appearances.get(&id) {
                                    if ap.fill_gradient.is_some() {
                                        flags[2] += 1;
                                    }
                                    if !matches!(ap.material, q0s_format::v2::VectorMaterial::Solid)
                                    {
                                        flags[3] += 1;
                                    }
                                    if !ap.erase_mask.is_empty()
                                        || !ap.material_source.is_empty()
                                        || !ap.clip_mask.is_empty()
                                    {
                                        flags[4] += 1;
                                    }
                                }
                            }
                            _ => flags[5] += 1,
                        }
                    }
                }
                q0s_format::v2::Target::Q0rg(child) if child != qid => {
                    let cf = q0s_format::v2::resolve_q0rg_playback_frame(
                        project,
                        child,
                        local,
                        p.frame,
                        st.fx.q0rg_playback,
                    );
                    q0rg_complex_flags(project, child, cf, depth + 1, flags);
                }
                _ => {}
            }
        }
    }
}
fn opacity_complex_reasons(
    project: &q0s_format::v2::ProjectV2,
    qid: u16,
    frame: u16,
    depth: u8,
    totals: &mut [usize; 7],
) {
    if depth > 8 {
        return;
    }
    let Some(q) = project.q0rgs.iter().find(|q| q.q0rg_id == qid) else {
        return;
    };
    let local = if q.frame_count > 0 {
        frame % q.frame_count
    } else {
        0
    };
    for l in &q.layers {
        if !project.layer_is_visible(qid, l.layer_id) {
            continue;
        }
        for st in q0s_format::raster::active_placement_states_at(l, local) {
            let Some(p) = l.placements.get(st.index) else {
                continue;
            };
            if st.fx.opacity < 0.999_999 {
                if let q0s_format::v2::Target::Q0rg(child) = p.target {
                    let cf = q0s_format::v2::resolve_q0rg_playback_frame(
                        project,
                        child,
                        local,
                        p.frame,
                        st.fx.q0rg_playback,
                    );
                    if !q0rg_plain_support(project, child, cf, depth + 1) {
                        let mut f = [0usize; 7];
                        q0rg_complex_flags(project, child, cf, depth + 1, &mut f);
                        for i in 0..7 {
                            if f[i] > 0 {
                                totals[i] += 1;
                            }
                        }
                    }
                }
            }
            if let q0s_format::v2::Target::Q0rg(child) = p.target {
                if child != qid {
                    let cf = q0s_format::v2::resolve_q0rg_playback_frame(
                        project,
                        child,
                        local,
                        p.frame,
                        st.fx.q0rg_playback,
                    );
                    opacity_complex_reasons(project, child, cf, depth + 1, totals);
                }
            }
        }
    }
}

fn main() {
    let path =
        std::path::Path::new(r"F:\Users\q0s0id\Downloads\eastlys-muzzle-flash-v0.1-reused.fla");
    let imported = q0fla::import_fla(path).unwrap();
    for warning in &imported.report.warnings {
        if warning.to_ascii_lowercase().contains("bitmap")
            || warning.to_ascii_lowercase().contains("spark")
            || warning.to_ascii_lowercase().contains("tween")
        {
            eprintln!("import_warning: {warning}");
        }
    }
    let project = imported.project;
    for asset in &project.assets {
        if let q0s_format::v2::Asset::Bitmap(bitmap) = asset {
            let name = project
                .asset_names
                .get(&bitmap.asset_id)
                .map(String::as_str)
                .unwrap_or("");
            let lower = name.to_ascii_lowercase();
            if lower.contains("bitmap 5")
                || lower.contains("bitmap 6")
                || lower.contains("bitmap 7")
                || lower.contains("bitmap 9")
                || lower.contains("bitmap 10")
                || lower.contains("bitmap 11")
                || lower.contains("bitmap 12")
                || lower.contains("bitmap 17")
                || lower.contains("bitmap 18")
                || lower.contains("spark")
            {
                let mut nonzero_alpha = 0usize;
                let mut opaque = 0usize;
                let mut alpha_max = 0u8;
                for px in bitmap.rgba.chunks_exact(4) {
                    alpha_max = alpha_max.max(px[3]);
                    nonzero_alpha += usize::from(px[3] != 0);
                    opaque += usize::from(px[3] == 255);
                }
                println!(
                    "bitmap_asset id={} name={:?} {}x{} alpha_nonzero={}/{} opaque={} max={}",
                    bitmap.asset_id,
                    name,
                    bitmap.width,
                    bitmap.height,
                    nonzero_alpha,
                    usize::from(bitmap.width) * usize::from(bitmap.height),
                    opaque,
                    alpha_max
                );
            }
        }
    }
    let tween_total = project
        .q0rgs
        .iter()
        .flat_map(|q| &q.layers)
        .flat_map(|l| &l.placements)
        .filter(|p| !matches!(p.tween, q0s_format::v2::Tween::None))
        .count();
    let eased_total = project
        .q0rgs
        .iter()
        .flat_map(|q| &q.layers)
        .flat_map(|l| &l.placements)
        .filter(|p| matches!(p.tween, q0s_format::v2::Tween::Eased { .. }))
        .count();
    println!(
        "imported_tweens total={} eased={}",
        tween_total, eased_total
    );
    for q in &project.q0rgs {
        if q.name.contains("SmokeAnimated1") || q.name.contains("bullet-smoke-animated") {
            let tweens = q
                .layers
                .iter()
                .flat_map(|l| &l.placements)
                .filter(|p| !matches!(p.tween, q0s_format::v2::Tween::None))
                .count();
            println!(
                "q0rg_tweens name={:?} frames={} tweens={}",
                q.name, q.frame_count, tweens
            );
        }
    }
    let mut stats = [0usize; 10];
    fx_stats(&project, 1, 0, 0, &mut stats);
    println!("fx_stats total={} identity={} opacity={} blend={} blur={} glow={} shadow={} q0rg={} opacity_q0rg={} opacity_asset={}",stats[0],stats[1],stats[2],stats[3],stats[4],stats[5],stats[6],stats[7],stats[8],stats[9]);
    let mut os = [0usize; 3];
    opacity_support_stats(&project, 1, 0, 0, &mut os);
    println!(
        "opacity q0rg={} plain_gpu_supported={} complex={}",
        os[0], os[1], os[2]
    );
    let mut reasons = [0usize; 7];
    opacity_complex_reasons(&project, 1, 0, 0, &mut reasons);
    println!(
        "complex reasons nested_fx={} stroke={} gradient={} softhalo={} masks={} nonvector={} depth={}",
        reasons[0], reasons[1], reasons[2], reasons[3], reasons[4], reasons[5], reasons[6]
    );
    for _ in 0..5 {
        let t = Instant::now();
        let mut counts = (0usize, 0usize, 0usize);
        traverse(&project, 1, 0, 0, &mut counts);
        println!(
            "traverse_ms={:.3} q0rg_visits={} active_placements={} asset_visits={}",
            t.elapsed().as_secs_f64() * 1000.0,
            counts.0,
            counts.1,
            counts.2
        );
    }
    let ctx = egui::Context::default();
    let screen = Rect::from_min_max(pos2(0.0, 0.0), pos2(1500.0, 1000.0));
    let view = StageView {
        origin: pos2(100.0, 100.0),
        scale: 1.0,
        stage_rect: screen,
    };
    let mut cache = TextureCache::default();
    let mut ts = Vec::new();
    let mut shapes = 0;
    for i in 0..25 {
        let mut ms = 0.0;
        let out = ctx.run(
            egui::RawInput {
                screen_rect: Some(screen),
                ..Default::default()
            },
            |ctx| {
                let p = ctx.layer_painter(egui::LayerId::new(
                    egui::Order::Middle,
                    egui::Id::new(("m", i)),
                ));
                let t = Instant::now();
                render_stage(&p, &project, 1, 0, &view, &mut cache, ctx);
                ms = t.elapsed().as_secs_f64() * 1000.0;
            },
        );
        shapes = out.shapes.len();
        ts.push(ms);
    }
    let w = &ts[5..];
    println!("shapes={shapes} times={ts:?}");
    println!(
        "warm avg={:.3} min={:.3} max={:.3}",
        w.iter().sum::<f64>() / w.len() as f64,
        w.iter().copied().fold(f64::INFINITY, f64::min),
        w.iter().copied().fold(0.0, f64::max)
    );
    let stage_frames = project
        .q0rgs
        .iter()
        .find(|q| q.q0rg_id == project.meta.entry_q0rg_id)
        .map(|q| q.frame_count)
        .unwrap_or(1);
    let mut animated = Vec::new();
    let mut max_callbacks = 0usize;
    for frame in 0..stage_frames {
        let mut ms = 0.0;
        let out = ctx.run(
            egui::RawInput {
                screen_rect: Some(screen),
                ..Default::default()
            },
            |ctx| {
                let p = ctx.layer_painter(egui::LayerId::new(
                    egui::Order::Middle,
                    egui::Id::new(("animated-muzzle", frame)),
                ));
                let t = Instant::now();
                render_stage(&p, &project, 1, frame, &view, &mut cache, ctx);
                ms = t.elapsed().as_secs_f64() * 1000.0;
            },
        );
        let callbacks = out
            .shapes
            .iter()
            .filter(|shape| matches!(shape.shape, egui::Shape::Callback(_)))
            .count();
        max_callbacks = max_callbacks.max(callbacks);
        animated.push((frame, ms, out.shapes.len(), callbacks));
    }
    let avg = animated.iter().map(|v| v.1).sum::<f64>() / animated.len().max(1) as f64;
    let worst = animated.iter().max_by(|a, b| a.1.total_cmp(&b.1)).copied();
    println!(
        "animated_frames={} cpu_avg_ms={:.3} worst={:?} max_callbacks={}",
        stage_frames, avg, worst, max_callbacks
    );
    println!("animated={animated:?}");
}
