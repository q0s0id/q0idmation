//! Small real .q0s files for testing the browser port, without a server account.
use q0s_format::v2::*;
use std::path::PathBuf;

fn project(frames: u16) -> ProjectV2 {
    ProjectV2 {
        meta: ProjectMeta {
            name: "browser fixture".into(),
            fps: 24,
            stage_width: 320,
            stage_height: 200,
            background_color: Rgba {
                r: 244,
                g: 236,
                b: 218,
                a: 255,
            },
            entry_q0rg_id: 1,
        },
        assets: vec![Asset::Vector(VectorAsset {
            asset_id: 1,
            fill: Some(Rgba {
                r: 205,
                g: 62,
                b: 43,
                a: 255,
            }),
            stroke: None,
            paths: vec![Path {
                closed: true,
                anchors: [[0., 0.], [30., 0.], [30., 30.], [0., 30.]]
                    .map(|[x, y]| Anchor {
                        point: Vec2::new(x, y),
                        in_handle: None,
                        out_handle: None,
                    })
                    .to_vec(),
            }],
        })],
        asset_names: Default::default(),
        asset_appearances: Default::default(),
        layer_metadata: Default::default(),
        audio_clips: Vec::new(),
        runtime: Default::default(),
        q0rgs: vec![Q0rg {
            q0rg_id: 1,
            name: "stage".into(),
            frame_count: frames,
            script: String::new(),
            layers: vec![Layer {
                layer_id: 1,
                name: "player".into(),
                timeline_frame_count: None,
                explicit_keyframes: vec![0],
                placements: vec![Placement {
                    pivot: None,
                    instance_id: 1,
                    frame: 0,
                    target: Target::Asset(1),
                    transform: Transform2D {
                        tx: 30.,
                        ty: 170.,
                        ..Transform2D::IDENTITY
                    },
                    tween: Tween::None,
                    fx: Default::default(),
                }],
            }],
        }],
    }
}

fn main() {
    let output = PathBuf::from(std::env::args_os().nth(1).expect("output folder"));
    std::fs::create_dir_all(&output).unwrap();
    let mut animation = project(48);
    let mut end = animation.q0rgs[0].layers[0].placements[0].clone();
    end.frame = 47;
    end.transform.tx = 260.;
    end.transform.ty = 35.;
    animation.q0rgs[0].layers[0].placements[0].tween = Tween::Linear { to_frame: 47 };
    animation.q0rgs[0].layers[0].placements.push(end);
    animation.q0rgs[0].layers[0].explicit_keyframes.push(47);
    let mut writer = q0video::q0v::Q0vWriter::new(
        std::io::Cursor::new(Vec::new()),
        q0video::q0v::Q0vSpec {
            width: 0,
            height: 0,
            fps: 24,
            timeline_frames: 48,
            video: false,
            audio: true,
            audio_sample_rate: 48000,
            audio_channels: 1,
        },
    )
    .unwrap();
    let tone = (0..96000)
        .map(|i| ((i as f32 / 48000. * 440. * std::f32::consts::TAU).sin() * 3000.) as i16)
        .collect::<Vec<_>>();
    writer.write_audio_pcm_i16(&tone).unwrap();
    animation.assets.push(Asset::Q0v(Q0vAsset {
        asset_id: 2,
        bytes: writer.finish().unwrap().into_inner().into(),
    }));
    animation.q0rgs[0].layers.push(Layer {
        layer_id: 2,
        name: "audio".into(),
        timeline_frame_count: None,
        explicit_keyframes: Vec::new(),
        placements: Vec::new(),
    });
    animation.audio_clips.push(AudioClip {
        q0rg_id: 1,
        layer_id: 2,
        asset_id: 2,
        gain: 1.0,
        ..Default::default()
    });
    std::fs::write(
        output.join("animation.q0s"),
        q0s_format::q0s_v2::write_q0s_v2(&animation).unwrap(),
    )
    .unwrap();

    let mut game = project(1);
    game.runtime
        .instance_names
        .insert(InstanceKey::new(1, 1), "player".into());
    // Use the exact controller from the regression test.
    let library = include_str!("../src/lib.rs");
    let controller = library
        .split("const GAME_CONTROLLER: &str = r#\"")
        .nth(1)
        .unwrap()
        .split("\"#;")
        .next()
        .unwrap();
    game.q0rgs[0].script = controller.into();
    game.runtime.frame_scripts.push(FrameScript {
        q0rg_id: 1,
        layer_id: 1,
        frame: 0,
        source: "vy = update(vy)\n".into(),
    });
    std::fs::write(
        output.join("game.q0s"),
        q0s_format::q0s_v2::write_q0s_v2(&game).unwrap(),
    )
    .unwrap();

    let mut mouse = project(1);
    mouse
        .runtime
        .instance_names
        .insert(InstanceKey::new(1, 1), "player".into());
    mouse.q0rgs[0].script = "import q0.mouse\nimport q0.scene\npb func move target {\n  player.x = mouse.x\n  player.y = mouse.y\n}\nlisten mouse.click move\n".into();
    std::fs::write(
        output.join("mouse.q0s"),
        q0s_format::q0s_v2::write_q0s_v2(&mouse).unwrap(),
    )
    .unwrap();

    let mut expected = Vec::new();
    for (name, frame) in [("animation", 0), ("animation", 24), ("game", 0)] {
        let mut player = q0player::Player::from_bytes(
            &std::fs::read(output.join(format!("{name}.q0s"))).unwrap(),
        )
        .unwrap();
        player.seek_to_frame(frame);
        let project = player.project_v2().unwrap();
        let pixels = q0s_format::raster::rasterize_q0rg_frame_with_runtime_state(
            project,
            project.meta.entry_q0rg_id,
            frame as u16,
            320,
            200,
            1,
            [255; 4],
            player.rig_runtime_overrides(),
            player.scene_runtime(),
        );
        let hash = pixels.iter().fold(2166136261_u32, |hash, byte| {
            (hash ^ u32::from(*byte)).wrapping_mul(16777619)
        });
        expected.push(
            serde_json::json!({ "file": format!("{name}.q0s"), "frame": frame, "hash": hash }),
        );
    }
    std::fs::write(
        output.join("expected.json"),
        serde_json::to_vec_pretty(&expected).unwrap(),
    )
    .unwrap();
    println!("browser fixtures written to {}", output.display());
}
