//! Headless import, q0v and MP4 regression probe. Arguments: audio-file output-directory.
use q0editor::q0enc::{ExportFormat, JobState, Q0EncState, Q0vStreams};
use q0s_format::v2::{Asset, AudioClip, Q0vAsset};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let source = PathBuf::from(args.next().ok_or("missing audio path")?);
    let output = PathBuf::from(args.next().ok_or("missing output directory")?);
    std::fs::create_dir_all(&output)?;
    let bytes = q0editor::audio::transcode_audio_to_q0v(&source)?;
    let media = q0video::q0v::Q0vFile::parse(bytes.clone())?;
    println!(
        "original_bytes={} imported_q0v_bytes={} samples={} rate={} channels={} codec={:?}",
        std::fs::metadata(&source)?.len(),
        bytes.len(),
        media.audio_samples_per_channel,
        media.spec.audio_sample_rate,
        media.spec.audio_channels,
        media.audio_codec()
    );
    std::fs::write(output.join("imported.q0v"), &bytes)?;
    let mut project = q0editor::state::default_project();
    project.meta.stage_width = 64;
    project.meta.stage_height = 64;
    project.meta.fps = 24;
    project.q0rgs[0].frame_count = 72;
    project.assets.clear();
    project.q0rgs[0].layers[0].placements.clear();
    project.assets.push(Asset::Q0v(Q0vAsset {
        asset_id: 77,
        bytes: bytes.into(),
    }));
    project.audio_clips.push(AudioClip {
        q0rg_id: project.meta.entry_q0rg_id,
        layer_id: project.q0rgs[0].layers[0].layer_id,
        asset_id: 77,
        start_frame: 12,
        source_offset_samples: 4800,
        gain: 0.5,
        pan: -1.0,
        fade_in_ms: 100,
        fade_out_ms: 100,
        duration_frames: Some(36),
        ..Default::default()
    });
    let mut muted_layer = project.q0rgs[0].layers[0].clone();
    muted_layer.layer_id += 1;
    muted_layer.name = "muted audio".into();
    let muted_layer_id = muted_layer.layer_id;
    project.q0rgs[0].layers.push(muted_layer);
    project.audio_clips.push(AudioClip {
        muted: true,
        gain: 4.0,
        layer_id: muted_layer_id,
        ..project.audio_clips[0]
    });
    let project_path = output.join("source.q1s");
    q0editor::file_io::save_project(&project_path, &project).map_err(|error| error.to_string())?;
    let project =
        q0editor::file_io::load_project(&project_path).map_err(|error| error.to_string())?;
    for format in [ExportFormat::Mp4, ExportFormat::Q0v] {
        let mut state = Q0EncState::default();
        state.open_for_project(&project, None);
        state.select_format(format, None);
        state.width = 64;
        state.height = 64;
        state.supersampling = 1;
        state.entire_timeline = false;
        state.first_frame = 6;
        state.last_frame = 65;
        state.q0v_streams = Q0vStreams {
            video: true,
            audio: true,
        };
        state.output_text = output
            .join(format!("timeline.{}", format.extension()))
            .display()
            .to_string();
        state.enqueue(&project)?;
        state.start_queue();
        let started = Instant::now();
        while state.queue_running && started.elapsed() < Duration::from_secs(120) {
            state.poll_events();
            std::thread::sleep(Duration::from_millis(5));
        }
        state.poll_events();
        if !matches!(state.queue[0].state, JobState::Completed) {
            return Err(format!("export failed: {:?}", state.queue[0].state).into());
        }
        let path = &state.queue[0].output_path;
        println!(
            "export={} bytes={} elapsed_ms={:.1}",
            path.display(),
            std::fs::metadata(path)?.len(),
            started.elapsed().as_secs_f64() * 1000.0
        );
        if format == ExportFormat::Q0v {
            let file = q0video::q0v::Q0vFile::parse(std::fs::read(path)?)?;
            std::fs::write(output.join("expected.pcm"), file.audio_pcm_le_bytes()?)?;
            println!(
                "exported_audio_samples={} codec={:?}",
                file.audio_samples_per_channel,
                file.audio_codec()
            );
        }
    }
    Ok(())
}
