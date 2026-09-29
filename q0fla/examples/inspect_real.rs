fn main() {
    let path =
        std::path::Path::new(r"F:\Users\q0s0id\Downloads\eastlys-muzzle-flash-v0.1-reused.fla");
    let imported = q0fla::import_fla(path).expect("import eastly fla");
    println!(
        "q0rgs={} assets={} warnings={}",
        imported.project.q0rgs.len(),
        imported.project.assets.len(),
        imported.report.warnings.len()
    );
    let tween_total = imported
        .project
        .q0rgs
        .iter()
        .flat_map(|q| &q.layers)
        .flat_map(|l| &l.placements)
        .filter(|p| !matches!(p.tween, q0s_format::v2::Tween::None))
        .count();
    println!("tweens={tween_total}");
    let bytes = q0s_format::v2::write(&imported.project).expect("write q1s-v2");
    let parsed = q0s_format::v2::parse(&bytes).expect("parse q1s-v2");
    let parsed_tweens = parsed
        .q0rgs
        .iter()
        .flat_map(|q| &q.layers)
        .flat_map(|l| &l.placements)
        .filter(|p| !matches!(p.tween, q0s_format::v2::Tween::None))
        .count();
    println!(
        "roundtrip bytes={} tweens={parsed_tweens} wire_eq={}",
        bytes.len(),
        q0s_format::v2::wire_equivalent(&imported.project, &parsed)
    );
    let smoke = imported
        .project
        .q0rgs
        .iter()
        .find(|q| q.name.ends_with("SmokeAnimated1"))
        .unwrap();
    for f in [0u16, 1, 12, 23, 24, 25, 26] {
        let states: Vec<_> = smoke
            .layers
            .iter()
            .flat_map(|l| q0s_format::raster::active_placement_states_at(l, f))
            .collect();
        println!(
            "smoke1 frame={f} states={} moving={:?}",
            states.len(),
            states
                .iter()
                .map(|s| (s.transform.tx, s.transform.ty))
                .collect::<Vec<_>>()
        );
    }
    for q in &imported.project.q0rgs {
        let low = q.name.to_ascii_lowercase();
        if low.contains("muzzleanimated")
            || low.contains("smokeanimated")
            || low.contains("muzzlespark")
        {
            let placements = q.layers.iter().flat_map(|l| &l.placements).count();
            let tweens = q
                .layers
                .iter()
                .flat_map(|l| &l.placements)
                .filter(|p| !matches!(p.tween, q0s_format::v2::Tween::None))
                .count();
            println!(
                "q0rg id={} name={:?} frames={} placements={} tweens={}",
                q.q0rg_id, q.name, q.frame_count, placements, tweens
            );
            for l in &q.layers {
                let lt = l
                    .placements
                    .iter()
                    .filter(|p| !matches!(p.tween, q0s_format::v2::Tween::None))
                    .count();
                if lt > 0 {
                    println!(
                        "  layer {:?} keyframes={:?} tweens={lt}",
                        l.name, l.explicit_keyframes
                    );
                    for p in &l.placements {
                        if !matches!(p.tween, q0s_format::v2::Tween::None) {
                            println!(
                                "    f={} target={:?} tween={:?} tx={:.2} ty={:.2}",
                                p.frame, p.target, p.tween, p.transform.tx, p.transform.ty
                            );
                        }
                    }
                }
            }
        }
    }
    for a in &imported.project.assets {
        if let q0s_format::v2::Asset::Bitmap(b) = a {
            let name = imported
                .project
                .asset_names
                .get(&b.asset_id)
                .map(String::as_str)
                .unwrap_or("");
            if name.to_ascii_lowercase().contains("spark")
                || name.to_ascii_lowercase().contains("bitmap 5")
                || name.to_ascii_lowercase().contains("bitmap 6")
                || name.to_ascii_lowercase().contains("bitmap 7")
                || name.to_ascii_lowercase().contains("bitmap 9")
                || name.to_ascii_lowercase().contains("bitmap 10")
                || name.to_ascii_lowercase().contains("bitmap 11")
                || name.to_ascii_lowercase().contains("bitmap 12")
                || name.to_ascii_lowercase().contains("bitmap 17")
                || name.to_ascii_lowercase().contains("bitmap 18")
            {
                let nonzero = b.rgba.chunks_exact(4).filter(|p| p[3] != 0).count();
                let maxa = b.rgba.chunks_exact(4).map(|p| p[3]).max().unwrap_or(0);
                println!(
                    "bitmap id={} name={:?} {}x{} nonzero={}/{} maxa={}",
                    b.asset_id,
                    name,
                    b.width,
                    b.height,
                    nonzero,
                    b.rgba.len() / 4,
                    maxa
                );
            }
        }
    }
    let stage = &imported.project.q0rgs[0];
    println!(
        "stage frames={} layers={}",
        stage.frame_count,
        stage.layers.len()
    );
    for l in &stage.layers {
        for p in &l.placements {
            if let q0s_format::v2::Target::Q0rg(id) = p.target {
                let n = imported
                    .project
                    .q0rgs
                    .iter()
                    .find(|q| q.q0rg_id == id)
                    .map(|q| q.name.as_str())
                    .unwrap_or("?");
                if n.to_ascii_lowercase().contains("muzzle") {
                    println!(
                        "stage placement f={} layer={:?} child={} {:?} playback={:?}",
                        p.frame, l.name, id, n, p.fx.q0rg_playback
                    );
                }
            }
        }
    }
    for w in &imported.report.warnings {
        let l = w.to_ascii_lowercase();
        if l.contains("bitmap")
            || l.contains("spark")
            || l.contains("tween")
            || l.contains("muzzle")
        {
            println!("warning: {w}");
        }
    }
}
