//! Headless import/save diagnostics; never opens an editor window.
use std::path::PathBuf;
use std::time::{Duration, Instant};

fn verify_audio_export(
    project: &q0s_format::v2::ProjectV2,
    output: &std::path::Path,
) -> Result<(), Box<dyn std::error::Error>> {
    use q0editor::q0enc::{ExportFormat, JobState, Q0EncState, Q0vStreams};
    for q0rg in project.q0rgs.iter().filter(|q| {
        project
            .audio_clips
            .iter()
            .any(|clip| clip.q0rg_id == q.q0rg_id)
    }) {
        let mut state = Q0EncState::default();
        state.open_for_project(project, None);
        state.select_format(ExportFormat::Q0v, None);
        state.source_q0rg_id = q0rg.q0rg_id;
        state.q0v_streams = Q0vStreams {
            video: false,
            audio: true,
        };
        state.output_text = output
            .join(format!("timeline-{}-audio.q0v", q0rg.q0rg_id))
            .display()
            .to_string();
        state.enqueue(project)?;
        state.start_queue();
        let started = Instant::now();
        while state.queue_running && started.elapsed() < Duration::from_secs(30) {
            state.poll_events();
            std::thread::sleep(Duration::from_millis(5));
        }
        state.poll_events();
        if !matches!(state.queue[0].state, JobState::Completed) {
            return Err(format!("audio export failed: {:?}", state.queue[0].state).into());
        }
        let bytes = std::fs::read(&state.queue[0].output_path)?;
        let view = q0video::q0v::audio_view(&bytes)?;
        let peak = view
            .pcm_le_bytes()
            .as_chunks::<2>()
            .0
            .iter()
            .map(|raw| i16::from_le_bytes([raw[0], raw[1]]).unsigned_abs())
            .max()
            .unwrap_or(0);
        if peak == 0 {
            return Err(format!("silent timeline {}", q0rg.q0rg_id).into());
        }
        println!(
            "audio_export q0rg={} samples={} rate={} channels={} peak={peak}",
            q0rg.name,
            view.samples_per_channel,
            view.spec.audio_sample_rate,
            view.spec.audio_channels
        );
        std::fs::write(
            output.join(format!("timeline-{}-audio.pcm", q0rg.q0rg_id)),
            view.pcm_le_bytes(),
        )?;
    }
    Ok(())
}

fn differences(a: &serde_json::Value, b: &serde_json::Value, path: &str, found: &mut usize) {
    if a == b || *found >= 20 {
        return;
    }
    match (a, b) {
        (serde_json::Value::Object(a), serde_json::Value::Object(b)) => {
            let keys: std::collections::BTreeSet<_> = a.keys().chain(b.keys()).collect();
            for key in keys {
                differences(
                    &a.get(key).cloned().unwrap_or_default(),
                    &b.get(key).cloned().unwrap_or_default(),
                    &format!("{path}/{key}"),
                    found,
                );
            }
        }
        (serde_json::Value::Array(a), serde_json::Value::Array(b)) if a.len() == b.len() => {
            for (index, (a, b)) in a.iter().zip(b).enumerate() {
                differences(a, b, &format!("{path}/{index}"), found);
            }
        }
        _ => {
            println!(
                "difference {path}: {} -> {}",
                a.to_string().chars().take(180).collect::<String>(),
                b.to_string().chars().take(180).collect::<String>()
            );
            *found += 1;
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let source = PathBuf::from(args.next().expect("fla/xfl path"));
    let output = PathBuf::from(args.next().expect("output directory"));
    std::fs::create_dir_all(&output)?;
    let imported = q0fla::import_path(&source)?;
    println!("{}", imported.report.summary());
    for warning in &imported.report.warnings {
        println!("warning: {warning}");
    }
    let mut canonical = imported.project.clone();
    canonical.assets.sort_by_key(q0s_format::v2::Asset::id);
    canonical.q0rgs.sort_by_key(|q| q.q0rg_id);
    canonical.runtime.scenes = canonical.scenes();
    canonical.runtime.library_folders.sort();
    for q in &mut canonical.q0rgs {
        for layer in &mut q.layers {
            layer.placements.sort_by_key(|p| p.frame);
            layer.explicit_keyframes.sort_unstable();
            if layer.timeline_frame_count == Some(q.frame_count) {
                layer.timeline_frame_count = None;
            }
        }
    }
    let bytes = q0s_format::v2::write(&canonical)?;
    let parsed = q0s_format::v2::parse(&bytes)?;
    println!(
        "assets={} symbols={} audio_clips={} tinted={} bytes={} equivalent={}",
        canonical.assets.len(),
        canonical.q0rgs.len(),
        canonical.audio_clips.len(),
        canonical
            .q0rgs
            .iter()
            .flat_map(|q| &q.layers)
            .flat_map(|l| &l.placements)
            .filter(|p| p.fx.tint.is_some())
            .count(),
        bytes.len(),
        q0s_format::v2::wire_equivalent(&canonical, &parsed)
    );
    if !q0s_format::v2::wire_equivalent(&canonical, &parsed) {
        differences(
            &serde_json::to_value(&canonical)?,
            &serde_json::to_value(&parsed)?,
            "",
            &mut 0,
        );
        return Err("q1s changed the canonical project".into());
    }
    let path = output.join("flag-band.q1s");
    q0editor::file_io::save_project(&path, &imported.project).map_err(|error| error.to_string())?;
    let project = q0editor::file_io::load_project(&path).map_err(|error| error.to_string())?;
    if !q0s_format::v2::wire_equivalent(&imported.project, &project) {
        return Err("editor reopen changed the project".into());
    }
    println!("saved_and_reopened={}", path.display());
    let again = output.join("flag-band-resaved.q1s");
    q0editor::file_io::save_project(&again, &project).map_err(|error| error.to_string())?;
    if std::fs::read(&path)? != std::fs::read(&again)? {
        return Err("repeated save changed q1s bytes".into());
    }
    for asset in &project.assets {
        if let q0s_format::v2::Asset::Q0v(media) = asset {
            let view = q0video::q0v::audio_view(&media.bytes)?;
            println!(
                "audio_asset={} name={:?} samples={} rate={} channels={}",
                media.asset_id,
                project.asset_names.get(&media.asset_id),
                view.samples_per_channel,
                view.spec.audio_sample_rate,
                view.spec.audio_channels
            );
            std::fs::write(
                output.join(format!("asset-{}.q0v", media.asset_id)),
                media.bytes.as_slice(),
            )?;
        }
    }
    for clip in &project.audio_clips {
        println!(
            "clip q0rg={} layer={} start={} duration={:?} asset={}",
            clip.q0rg_id, clip.layer_id, clip.start_frame, clip.duration_frames, clip.asset_id
        );
    }
    for q0rg in project
        .q0rgs
        .iter()
        .filter(|q| q.name == "France Animation Tint")
    {
        let mut untinted = project.clone();
        for layer in &mut untinted
            .q0rgs
            .iter_mut()
            .find(|q| q.q0rg_id == q0rg.q0rg_id)
            .unwrap()
            .layers
        {
            for placement in &mut layer.placements {
                placement.fx.tint = None;
            }
        }
        for frame in [0, 20, 44, 65] {
            let image = q0s_format::raster::rasterize_q0rg_frame(
                &project,
                q0rg.q0rg_id,
                frame,
                1080,
                800,
                1,
                [51, 51, 51, 255],
            );
            let baseline = q0s_format::raster::rasterize_q0rg_frame(
                &untinted,
                q0rg.q0rg_id,
                frame,
                1080,
                800,
                1,
                [51, 51, 51, 255],
            );
            let changed = image
                .as_chunks::<4>()
                .0
                .iter()
                .zip(baseline.as_chunks::<4>().0.iter())
                .filter(|(a, b)| a != b)
                .count();
            println!("tint_render frame={frame} changed_pixels={changed}");
            if changed == 0 {
                return Err(format!("tint has no visible effect on frame {frame}").into());
            }
            image::save_buffer(
                output.join(format!("tint-{frame}.png")),
                &image,
                1080,
                800,
                image::ColorType::Rgba8,
            )?;
        }
    }
    verify_audio_export(&project, &output)?;
    Ok(())
}
