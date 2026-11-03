//! Frame budget of a room document under real editor input: the whole editor
//! frame runs headlessly (stage, tools, autosave, online tick) while a
//! selection box is dragged, moved, and remote edits arrive. Ignored by default:
//! `cargo test --release -p q0editor --lib online::freeze -- --ignored --nocapture`
use super::*;
use q0s_format::v2::{
    Anchor, Asset, Path, Placement, Rgba, Target, Transform2D, Tween, Vec2, VectorAsset,
};

fn circle(id: u16, anchors: usize, radius: f32) -> Asset {
    Asset::Vector(VectorAsset {
        asset_id: id,
        fill: Some(Rgba {
            r: (id * 37 % 255) as u8,
            g: 80,
            b: 160,
            a: 255,
        }),
        stroke: None,
        paths: vec![Path {
            closed: true,
            anchors: (0..anchors)
                .map(|i| {
                    let a = std::f32::consts::TAU * i as f32 / anchors as f32;
                    Anchor {
                        point: Vec2::new(a.cos() * radius, a.sin() * radius),
                        in_handle: None,
                        out_handle: None,
                    }
                })
                .collect(),
        }],
    })
}

/// A busy page: `count` filled shapes spread over the stage on one layer.
fn busy_project(count: usize, anchors: usize, radius: f32) -> ProjectV2 {
    let mut project = crate::state::default_project();
    let (w, h) = (
        f32::from(project.meta.stage_width),
        f32::from(project.meta.stage_height),
    );
    let columns = (count as f32).sqrt().ceil() as usize;
    for index in 0..count {
        let id = index as u16 + 1;
        project.assets.push(circle(id, anchors, radius));
        let (col, row) = (index % columns, index / columns);
        let x = 20.0 + (w - 40.0) * col as f32 / columns as f32;
        let y = 20.0 + (h - 40.0) * row as f32 / columns as f32;
        project.q0rgs[0].layers[0].placements.push(Placement {
            instance_id: 0,
            frame: 0,
            target: Target::Asset(id),
            transform: Transform2D {
                tx: x,
                ty: y,
                ..Transform2D::IDENTITY
            },
            pivot: None,
            tween: Tween::None,
            fx: Default::default(),
        });
    }
    project
}

struct Driver {
    ctx: Context,
    app: EditorApp,
    screen: egui::Rect,
    time: f64,
    slowest: (f64, &'static str),
}
impl Driver {
    fn frame(&mut self, events: Vec<egui::Event>, label: &'static str) -> egui::FullOutput {
        self.time += 1.0 / 60.0;
        let started = Instant::now();
        let app = &mut self.app;
        let output = self.ctx.run(
            egui::RawInput {
                screen_rect: Some(self.screen),
                time: Some(self.time),
                events,
                ..Default::default()
            },
            |ctx| app.update_headless(ctx),
        );
        let ms = started.elapsed().as_secs_f64() * 1000.0;
        if ms > self.slowest.0 {
            self.slowest = (ms, label);
        }
        output
    }
    fn stage_rect(&mut self) -> egui::Rect {
        let output = self.frame(Vec::new(), "idle");
        let mut best = egui::Rect::NOTHING;
        for clipped in &output.shapes {
            if let egui::Shape::Rect(rect) = &clipped.shape {
                let larger = !best.is_positive() || rect.rect.area() > best.area();
                if rect.fill == crate::theme::STAGE_BG && larger {
                    best = rect.rect;
                }
            }
        }
        if !best.is_positive() {
            let mut rects: Vec<(f32, String)> = output
                .shapes
                .iter()
                .filter_map(|c| match &c.shape {
                    egui::Shape::Rect(r) => {
                        Some((r.rect.area(), format!("{:?} {:?}", r.rect, r.fill)))
                    }
                    egui::Shape::Mesh(m) => Some((
                        0.0,
                        format!("mesh {} verts tex {:?}", m.vertices.len(), m.texture_id),
                    )),
                    _ => None,
                })
                .collect();
            rects.sort_by(|a, b| b.0.total_cmp(&a.0));
            panic!(
                "stage not found; home={} shapes={} biggest={:?}",
                self.app.home_visible(),
                output.shapes.len(),
                &rects[..rects.len().min(8)]
            );
        }
        best
    }
    fn drag(&mut self, from: egui::Pos2, to: egui::Pos2, steps: usize, label: &'static str) {
        let button = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::default(),
        };
        self.frame(vec![egui::Event::PointerMoved(from)], label);
        self.frame(vec![button(from, true)], label);
        for step in 1..=steps {
            let t = step as f32 / steps as f32;
            self.frame(vec![egui::Event::PointerMoved(from.lerp(to, t))], label);
        }
        self.frame(vec![button(to, false)], label);
    }
    fn idle(&mut self, frames: usize, label: &'static str) {
        for _ in 0..frames {
            self.frame(Vec::new(), label);
            std::thread::sleep(Duration::from_millis(16));
        }
    }
}

fn room_app(project: ProjectV2) -> EditorApp {
    let mut app = EditorApp::default();
    let mut online = Online {
        user: Some(Person {
            id: 1,
            username: "owner".into(),
            ..Person::default()
        }),
        ..Online::default()
    };
    online.pending_join = Some(RoomSnapshot {
        id: "0123456789abcdef0123456789abcdef".into(),
        name: "busy".into(),
        owner_id: 1,
        revision: 0,
        project,
        members: vec![
            Member {
                user_id: 1,
                rank: Rank::Owner,
                grants: Vec::new(),
                banned: false,
            },
            Member {
                user_id: 2,
                rank: Rank::Editor,
                grants: Vec::new(),
                banned: false,
            },
        ],
        policy: RoomPolicy::default(),
        protocol_version: q0online::PROTOCOL_VERSION,
        people: BTreeMap::new(),
    });
    online.open_pending_join(&mut app);
    app.online = online;
    app
}

#[test]
#[ignore = "frame timing; run explicitly with --release"]
fn selection_box_in_a_room_keeps_frames_short() {
    let cases = [
        (false, 600, 48, 9.0, false),
        (true, 600, 48, 9.0, false),
        (false, 60, 3000, 60.0, false),
        (true, 60, 3000, 60.0, false),
        // a large embedded bitmap: every whole-document JSON round trip on the
        // ui thread used to cost seconds with media like this
        (true, 300, 48, 9.0, true),
    ];
    for (in_room, shapes, anchors, radius, bitmap) in cases {
        let mut project = busy_project(shapes, anchors, radius);
        if bitmap {
            let id = project.assets.iter().map(Asset::id).max().unwrap_or(0) + 1;
            project
                .assets
                .push(Asset::Bitmap(q0s_format::v2::BitmapAsset {
                    asset_id: id,
                    width: 2048,
                    height: 2048,
                    rgba: std::sync::Arc::new(vec![200; 2048 * 2048 * 4]),
                }));
            project.q0rgs[0].layers[0].placements.insert(
                0,
                Placement {
                    instance_id: 0,
                    frame: 0,
                    target: Target::Asset(id),
                    transform: Transform2D::IDENTITY,
                    pivot: None,
                    tween: Tween::None,
                    fx: Default::default(),
                },
            );
        }
        let app = if in_room {
            room_app(project.clone())
        } else {
            let mut app = EditorApp::default();
            app.open_online_project(project.clone()).unwrap();
            app
        };
        let mut driver = Driver {
            ctx: Context::default(),
            app,
            screen: egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 1000.0)),
            time: 0.0,
            slowest: (0.0, ""),
        };
        driver.app.session.current_tool = crate::state::Tool::Select;
        driver.idle(5, "warm up");
        let stage = driver.stage_rect();
        let at = |x: f32, y: f32| stage.min + egui::vec2(stage.width() * x, stage.height() * y);
        let started = Instant::now();
        // a selection box over a third of the shapes
        driver.drag(at(0.01, 0.01), at(0.6, 0.6), 30, "selection box");
        driver.idle(20, "after selection");
        // move the selection
        driver.drag(at(0.3, 0.3), at(0.45, 0.4), 30, "move selection");
        driver.idle(30, "after move");
        // someone else draws, several times, while the selection is still active
        for round in 0..(if in_room { 4 } else { 0 }) {
            let room = driver.app.online.room.as_ref().unwrap();
            let mut state = q0online::RoomState::new(1, room.confirmed.clone()).unwrap();
            state
                .set_member(
                    1,
                    Member {
                        user_id: 2,
                        rank: Rank::Editor,
                        grants: Vec::new(),
                        banned: false,
                    },
                )
                .unwrap();
            let base = state.project.clone();
            let mut theirs = base.clone();
            let id = theirs.assets.iter().map(Asset::id).max().unwrap() + 1;
            theirs.assets.push(circle(id, 48, 12.0));
            theirs.q0rgs[0].layers[0].placements.push(Placement {
                instance_id: 0,
                frame: 0,
                target: Target::Asset(id),
                transform: Transform2D::IDENTITY,
                pivot: None,
                tween: Tween::None,
                fx: Default::default(),
            });
            let event = state
                .submit(
                    2,
                    OperationRequest {
                        operation_id: format!("theirs-{round}"),
                        base_revision: 0,
                        patch: ProjectPatch::between(&base, &theirs).unwrap(),
                    },
                )
                .unwrap();
            let room = driver.app.online.room.as_mut().unwrap();
            // the poll would deliver it after whatever this editor sent
            let revision = room.revision + 1;
            room.events
                .insert(revision, RoomEvent { revision, ..event });
            driver.idle(40, "remote edit arrives");
        }
        eprintln!(
            "in_room={in_room} bitmap={bitmap} shapes={shapes}x{anchors}: total {:.0} ms, slowest frame {:.1} ms during {}",
            started.elapsed().as_secs_f64() * 1000.0,
            driver.slowest.0,
            driver.slowest.1
        );
    }
}
