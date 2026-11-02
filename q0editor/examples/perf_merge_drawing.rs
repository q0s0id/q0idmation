//! Headless merge + repaint benchmark on a detailed fill with many holes.
use std::time::Instant;

use q0editor::{app::EditorApp, brush, render};
use q0s_format::v2::{
    Anchor, Asset, Path, Placement, Rgba, Target, Transform2D, Tween, Vec2, VectorAsset,
};

fn repaint(app: &mut EditorApp) {
    let ctx = egui::Context::default();
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(900.0, 700.0));
    let view = render::StageView {
        origin: rect.min,
        scale: 1.0,
        stage_rect: rect,
        edit_transform: q0s_format::transform::Affine::IDENTITY,
    };
    let _ = ctx.run(
        egui::RawInput {
            screen_rect: Some(rect),
            ..Default::default()
        },
        |ctx| {
            let painter = ctx.layer_painter(egui::LayerId::background());
            render::render_stage(
                &painter,
                &app.state.project,
                1,
                0,
                &view,
                &mut app.textures,
                ctx,
            );
        },
    );
}

fn main() {
    println!(
        "profile={}",
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        }
    );
    for vertices in [2048, 8192] {
        for same_colour in [true, false] {
            for dab in [true, false] {
                let mut app = EditorApp::default();
                let paint = brush::BrushSettings {
                    size: 14.0,
                    smoothing: 50,
                    ..Default::default()
                };
                let mut paths = vec![Path {
                    anchors: (0..vertices)
                        .map(|index| {
                            let angle = std::f32::consts::TAU * index as f32 / vertices as f32;
                            let radius = 260.0 + (angle * 37.0).sin() * 5.0;
                            Anchor {
                                point: Vec2::new(
                                    300.0 + angle.cos() * radius,
                                    300.0 + angle.sin() * radius,
                                ),
                                in_handle: None,
                                out_handle: None,
                            }
                        })
                        .collect(),
                    closed: true,
                }];
                for row in -6..=6 {
                    for column in -6..=6 {
                        let dx = column as f32 * 30.0;
                        let dy = row as f32 * 30.0;
                        if dx.hypot(dy) > 205.0 {
                            continue;
                        }
                        paths.push(Path {
                            anchors: (0..32)
                                .map(|index| {
                                    let angle = -std::f32::consts::TAU * index as f32 / 32.0;
                                    Anchor {
                                        point: Vec2::new(
                                            300.0 + dx + angle.cos() * 6.0,
                                            300.0 + dy + angle.sin() * 6.0,
                                        ),
                                        in_handle: None,
                                        out_handle: None,
                                    }
                                })
                                .collect(),
                            closed: true,
                        });
                    }
                }
                let anchors: usize = paths.iter().map(|path| path.anchors.len()).sum();
                app.state.project.assets = vec![Asset::Vector(VectorAsset {
                    asset_id: 1,
                    paths,
                    fill: Some(if same_colour {
                        paint.color
                    } else {
                        Rgba {
                            r: 40,
                            g: 80,
                            b: 200,
                            a: 255,
                        }
                    }),
                    stroke: None,
                })];
                app.state.project.q0rgs[0].layers[0].placements = vec![Placement {
                    pivot: None,
                    instance_id: 0,
                    frame: 0,
                    target: Target::Asset(1),
                    transform: Transform2D::IDENTITY,
                    tween: Tween::None,
                    fx: Default::default(),
                }];
                repaint(&mut app);
                let start = if dab {
                    Vec2::new(560.0, 300.0)
                } else {
                    Vec2::new(550.0, 300.0)
                };
                let mut stroke = brush::brush_begin(paint, brush::BrushSample::mouse(start));
                if !dab {
                    brush::brush_add_sample(
                        &mut stroke,
                        paint,
                        brush::BrushSample::mouse(Vec2::new(580.0, 306.0)),
                    );
                }
                let region = brush::brush_finish(stroke, paint);
                let started = Instant::now();
                brush::commit_brush_region(&mut app, region, paint);
                let commit = started.elapsed();
                let started = Instant::now();
                repaint(&mut app);
                let render = started.elapsed();
                println!("anchors={anchors} same_colour={same_colour} dab={dab}: merge={commit:?} repaint={render:?} total={:?}",commit+render);
            }
        }
    }
}
