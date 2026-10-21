//! Reproducible q0enc benchmark. Run in release mode; arguments are
//! output-directory width height frames supersampling [project.q1s].
use q0editor::q0enc::{ExportFormat, JobState, Q0EncState};
use q0s_format::v2::*;
use std::{path::PathBuf, time::Instant};

fn sample_project(frames: u16) -> ProjectV2 {
    let mut project = q0editor::state::default_project();
    project.meta.stage_width = 640;
    project.meta.stage_height = 360;
    project.meta.fps = 24;
    project.meta.name = "mp4 export benchmark".into();
    let scene = &mut project.q0rgs[0];
    scene.frame_count = frames;
    scene.layers[0].placements.clear();
    for i in 0..12u16 {
        project.assets.push(Asset::Vector(VectorAsset {
            asset_id: i + 1,
            paths: vec![Path {
                anchors: [(0.0, 0.0), (100.0, 0.0), (100.0, 80.0), (0.0, 80.0)]
                    .map(|(x, y)| Anchor {
                        point: Vec2::new(x, y),
                        in_handle: None,
                        out_handle: None,
                    })
                    .to_vec(),
                closed: true,
            }],
            fill: Some(Rgba {
                r: (i * 19) as u8,
                g: 130,
                b: 220,
                a: if i % 3 == 0 { 150 } else { 255 },
            }),
            stroke: None,
        }));
        for (frame, shift) in [(0, 0.0), (frames - 1, 65.0)] {
            scene.layers[0].placements.push(Placement {
                pivot: None,
                instance_id: u32::from(i + 1),
                frame,
                target: Target::Asset(i + 1),
                transform: Transform2D {
                    tx: f32::from(i % 4) * 140.0 + shift,
                    ty: f32::from(i / 4) * 110.0,
                    ..Transform2D::IDENTITY
                },
                tween: if frame == 0 {
                    Tween::Linear {
                        to_frame: frames - 1,
                    }
                } else {
                    Tween::None
                },
                fx: PlacementFx::default(),
            });
        }
    }
    project
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    let output = PathBuf::from(args.get(1).ok_or("missing output directory")?);
    std::fs::create_dir_all(&output)?;
    let width: u32 = args.get(2).map(String::as_str).unwrap_or("3840").parse()?;
    let height: u32 = args.get(3).map(String::as_str).unwrap_or("2160").parse()?;
    let frames: u16 = args.get(4).map(String::as_str).unwrap_or("48").parse()?;
    let ss: u8 = args.get(5).map(String::as_str).unwrap_or("2").parse()?;
    let project = if let Some(path) = args.get(6) {
        q0editor::file_io::load_project(std::path::Path::new(path))
            .map_err(|error| error.to_string())?
    } else {
        sample_project(frames)
    };
    q0editor::file_io::save_project(&output.join("source.q1s"), &project)
        .map_err(|error| error.to_string())?;
    let bg = project.meta.background_color;
    let render = |frame| {
        q0s_format::raster::rasterize_q0rg_frame_scaled(
            &project,
            project.meta.entry_q0rg_id,
            frame,
            width,
            height,
            u32::from(ss),
            [bg.r, bg.g, bg.b, 255],
        )
    };
    let rgba = render(0);
    image::save_buffer(
        output.join("frame.png"),
        &rgba,
        width,
        height,
        image::ColorType::Rgba8,
    )?;
    let start = Instant::now();
    let mut checksum = 0u64;
    for frame in 0..frames {
        let pixels = render(frame);
        checksum = checksum.wrapping_add(pixels.iter().map(|v| u64::from(*v)).sum::<u64>());
        std::hint::black_box(pixels);
    }
    let render_time = start.elapsed();
    println!(
        "render_ms={:.1} frames={frames} size={width}x{height} ss={ss} checksum={checksum}",
        render_time.as_secs_f64() * 1000.0
    );
    #[cfg(windows)]
    {
        let start = Instant::now();
        let mut encoder = q0video::mp4::Mp4Encoder::create(
            &output.join("encode-only.mp4"),
            q0video::mp4::Mp4Spec {
                width,
                height,
                fps: u32::from(project.meta.fps),
                bitrate: 20_000_000,
            },
        )?;
        let create = start.elapsed();
        for _ in 0..frames {
            encoder.write_rgba_frame(&rgba)?;
        }
        let write = start.elapsed() - create;
        encoder.finish()?;
        println!(
            "encode_ms={:.1} create_ms={:.1} write_ms={:.1} finalize_ms={:.1}",
            start.elapsed().as_secs_f64() * 1000.0,
            create.as_secs_f64() * 1000.0,
            write.as_secs_f64() * 1000.0,
            (start.elapsed() - create - write).as_secs_f64() * 1000.0
        );
    }
    let mut queue = Q0EncState::default();
    queue.open_for_project(&project, None);
    queue.select_format(ExportFormat::Mp4, None);
    queue.entire_timeline = false;
    queue.first_frame = 0;
    queue.last_frame = frames - 1;
    queue.width = width;
    queue.height = height;
    queue.supersampling = ss;
    queue.mp4_bitrate = 20_000_000;
    queue.output_text = output.join("q0enc.mp4").display().to_string();
    queue.enqueue(&project)?;
    let start = Instant::now();
    queue.start_queue();
    loop {
        queue.poll_events();
        if queue.queue[0].state.is_finished() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(queue.queue[0].state, JobState::Completed);
    println!("q0enc_ms={:.1}", start.elapsed().as_secs_f64() * 1000.0);
    Ok(())
}
