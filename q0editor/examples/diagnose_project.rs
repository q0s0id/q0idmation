use q0s_format::v2::{self, Asset};
use std::{path::PathBuf, time::Instant};

fn main() {
    let mut args = std::env::args_os().skip(1);
    let path = PathBuf::from(args.next().expect("project path"));
    let output = PathBuf::from(args.next().expect("output directory"));
    std::fs::create_dir_all(&output).unwrap();
    let bytes = std::fs::read(&path).unwrap();
    println!(
        "bytes={} version={}",
        bytes.len(),
        u16::from_le_bytes([bytes[4], bytes[5]])
    );
    let started = Instant::now();
    let project = match q0editor::file_io::load_project(&path) {
        Ok(project) => project,
        Err(error) => {
            println!("load_error={error:?}\nmessage={error}");
            std::process::exit(2);
        }
    };
    println!(
        "load_ms={:.3} meta={:?}",
        started.elapsed().as_secs_f64() * 1000.0,
        project.meta
    );
    println!(
        "assets={} q0rgs={} appearances={} layer_metadata={} audio={} frame_scripts={}",
        project.assets.len(),
        project.q0rgs.len(),
        project.asset_appearances.len(),
        project.layer_metadata.len(),
        project.audio_clips.len(),
        project.runtime.frame_scripts.len()
    );
    println!("validation={:?}", v2::validate(&project));
    for asset in &project.assets {
        let name = project.asset_names.get(&asset.id());
        match asset {
            Asset::Vector(vector) => println!(
                "vector={} paths={} anchors={} fill={:?} stroke={:?} name={name:?}",
                vector.asset_id,
                vector.paths.len(),
                vector
                    .paths
                    .iter()
                    .map(|path| path.anchors.len())
                    .sum::<usize>(),
                vector.fill,
                vector.stroke
            ),
            _ => println!(
                "asset={} kind={}",
                asset.id(),
                match asset {
                    Asset::Bitmap(_) => "bitmap",
                    Asset::Q0v(_) => "q0v",
                    Asset::Rig(_) => "rig",
                    _ => unreachable!(),
                }
            ),
        }
    }
    for q0rg in &project.q0rgs {
        println!(
            "q0rg={} name={:?} frames={} layers={} script_bytes={}",
            q0rg.q0rg_id,
            q0rg.name,
            q0rg.frame_count,
            q0rg.layers.len(),
            q0rg.script.len()
        );
        for layer in &q0rg.layers {
            println!(
                "  layer={} name={:?} placements={} keys={:?} length={:?}",
                layer.layer_id,
                layer.name,
                layer.placements.len(),
                layer.explicit_keyframes,
                layer.timeline_frame_count
            );
            for placement in &layer.placements {
                println!("    frame={} target={:?} instance={} transform={:?} pivot={:?} tween={:?} fx={:?}", placement.frame, placement.target, placement.instance_id, placement.transform, placement.pivot, placement.tween, placement.fx);
            }
        }
    }
    let started = Instant::now();
    match v2::write(&project) {
        Ok(encoded) => match v2::parse(&encoded) {
            Ok(decoded) => println!(
                "roundtrip_equivalent={} canonical_bytes={} roundtrip_ms={:.3}",
                v2::wire_equivalent(&project, &decoded),
                encoded.len(),
                started.elapsed().as_secs_f64() * 1000.0
            ),
            Err(error) => println!("roundtrip_parse_error={error:?}"),
        },
        Err(error) => println!("write_error={error:?}"),
    }
    std::fs::write(
        output.join("project.json"),
        serde_json::to_vec_pretty(&project).unwrap(),
    )
    .unwrap();
}
