//! Headless pictures of the online panel for design review. They run the real
//! panel code in an egui context and rasterize its meshes in software, so no
//! window opens. Ignored by default:
//! `Q0EDITOR_ONLINE_SHOTS=<dir> cargo test -p q0editor --lib online::shots -- --ignored`
use super::ui::Tab;
use super::*;
use egui::epaint::{ClippedPrimitive, ImageData, Primitive};
use egui::Color32;
use q0online::{Grant, Scope};
use q0s_format::v2::{Layer, LayerKey, LayerKind, LayerMetadata, LibraryItemReference, Q0rg};

#[test]
fn online_panel_idle_layout_does_not_grow() {
    let mut failures = Vec::new();
    for tab in [Tab::Rooms, Tab::Friends, Tab::Room] {
        for notice in [false, true] {
            let mut app = EditorApp::default();
            let mut online = signed_in(1);
            room(&mut online, &mut app, 1);
            online.view.tab = tab;
            if notice {
                online.say("your unshared change is saved for recovery. open a local copy when you need it; you can keep working in the room.",Tone::Warning);
            } else {
                online.notice = None;
            }
            let ctx = Context::default();
            crate::theme::install(&ctx);
            let _ = crate::q0lang::fonts::install(&ctx, &app.settings.q0lang_font_name);
            app.settings.theme.apply(&ctx);
            let mut settled = None;
            let mut last = egui::Rect::NOTHING;
            for frame in 0..48 {
                let _ = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(1000.0, 800.0),
                        )),
                        time: Some(frame as f64 / 60.0),
                        ..Default::default()
                    },
                    |ctx| online.show_panel(&mut app, ctx),
                );
                last = ctx
                    .memory(|memory| memory.area_rect(egui::Id::new("q0editor_online")))
                    .unwrap();
                if frame == 8 {
                    settled = Some(last);
                }
                if [0, 8, 47].contains(&frame) {
                    eprintln!(
                        "online layout tab={} notice={notice} frame={frame}: {last:?}",
                        tab as u8
                    );
                }
            }
            let settled = settled.unwrap();
            if (last.width() - settled.width()).abs() > 0.5
                || (last.height() - settled.height()).abs() > 0.5
            {
                failures.push(format!(
                    "tab={} notice={notice} settled={settled:?} last={last:?}",
                    tab as u8
                ));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "idle panel keeps growing: {failures:?}"
    );
}

struct Canvas {
    width: usize,
    height: usize,
    pixels: Vec<[f32; 4]>,
    textures: HashMap<egui::TextureId, (usize, Vec<Color32>)>,
}
impl Canvas {
    fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            pixels: vec![[0.0, 0.0, 0.0, 1.0]; width * height],
            textures: HashMap::new(),
        }
    }
    fn textures(&mut self, delta: &egui::TexturesDelta) {
        for (id, change) in &delta.set {
            let (width, pixels): (usize, Vec<Color32>) = match &change.image {
                ImageData::Color(image) => (image.width(), image.pixels.clone()),
                ImageData::Font(font) => (font.width(), font.srgba_pixels(None).collect()),
            };
            match change.pos {
                None => {
                    self.textures.insert(*id, (width, pixels));
                }
                Some([x, y]) => {
                    let texture = self.textures.get_mut(id).expect("partial texture update");
                    for (row, line) in pixels.chunks(width).enumerate() {
                        let start = (y + row) * texture.0 + x;
                        texture.1[start..start + line.len()].copy_from_slice(line);
                    }
                }
            }
        }
    }
    fn paint(&mut self, primitives: &[ClippedPrimitive]) {
        let unit = |c: Color32| c.to_array().map(|v| f32::from(v) / 255.0);
        for primitive in primitives {
            let Primitive::Mesh(mesh) = &primitive.primitive else {
                continue;
            };
            let Some((tex_width, texels)) = self.textures.get(&mesh.texture_id) else {
                continue;
            };
            let tex_height = texels.len() / tex_width;
            let clip = primitive.clip_rect;
            let x_min = clip.min.x.max(0.0) as i32;
            let y_min = clip.min.y.max(0.0) as i32;
            let x_max = clip.max.x.min(self.width as f32) as i32;
            let y_max = clip.max.y.min(self.height as f32) as i32;
            for triangle in mesh.indices.as_chunks::<3>().0.iter() {
                let v = [0, 1, 2].map(|i| &mesh.vertices[triangle[i] as usize]);
                let edge = |a: egui::Pos2, b: egui::Pos2, p: egui::Pos2| {
                    (b.x - a.x) * (p.y - a.y) - (b.y - a.y) * (p.x - a.x)
                };
                let area = edge(v[0].pos, v[1].pos, v[2].pos);
                if area.abs() < 1e-6 {
                    continue;
                }
                let left = v.iter().map(|v| v.pos.x).fold(f32::MAX, f32::min).floor() as i32;
                let right = v.iter().map(|v| v.pos.x).fold(f32::MIN, f32::max).ceil() as i32;
                let top = v.iter().map(|v| v.pos.y).fold(f32::MAX, f32::min).floor() as i32;
                let bottom = v.iter().map(|v| v.pos.y).fold(f32::MIN, f32::max).ceil() as i32;
                for y in top.max(y_min)..bottom.min(y_max) {
                    for x in left.max(x_min)..right.min(x_max) {
                        let p = egui::pos2(x as f32 + 0.5, y as f32 + 0.5);
                        let w0 = edge(v[1].pos, v[2].pos, p) / area;
                        let w1 = edge(v[2].pos, v[0].pos, p) / area;
                        let w2 = 1.0 - w0 - w1;
                        if w0 < -1e-4 || w1 < -1e-4 || w2 < -1e-4 {
                            continue;
                        }
                        let u = w0 * v[0].uv.x + w1 * v[1].uv.x + w2 * v[2].uv.x;
                        let t = w0 * v[0].uv.y + w1 * v[1].uv.y + w2 * v[2].uv.y;
                        let tx = ((u * *tex_width as f32) as usize).min(tex_width - 1);
                        let ty = ((t * tex_height as f32) as usize).min(tex_height - 1);
                        let texel = unit(texels[ty * tex_width + tx]);
                        let colors = v.map(|v| unit(v.color));
                        let dst = &mut self.pixels[y as usize * self.width + x as usize];
                        let src: [f32; 4] = std::array::from_fn(|i| {
                            (w0 * colors[0][i] + w1 * colors[1][i] + w2 * colors[2][i]) * texel[i]
                        });
                        for i in 0..4 {
                            dst[i] = src[i] + dst[i] * (1.0 - src[3]);
                        }
                    }
                }
            }
        }
    }
    fn save(&self, path: &std::path::Path) {
        let bytes: Vec<u8> = self
            .pixels
            .iter()
            .flat_map(|p| p.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8))
            .collect();
        image::save_buffer(
            path,
            &bytes,
            self.width as u32,
            self.height as u32,
            image::ColorType::Rgba8,
        )
        .expect("save panel picture");
    }
}

fn project() -> ProjectV2 {
    let layer = |id: u16, name: &str, keys: bool| Layer {
        layer_id: id,
        timeline_frame_count: None,
        name: name.into(),
        explicit_keyframes: if keys { vec![0] } else { Vec::new() },
        placements: Vec::new(),
    };
    let mut project = crate::state::default_project();
    project.meta.name = "rooftop chase".into();
    project.meta.entry_q0rg_id = 1;
    project.q0rgs = vec![
        Q0rg {
            q0rg_id: 1,
            name: "main".into(),
            frame_count: 48,
            script: String::new(),
            layers: vec![
                layer(1, "sky", true),
                layer(5, "rooftops", true),
                layer(4, "ada shadow", true),
                layer(3, "ada legs", true),
                layer(2, "ada torso", true),
                layer(11, "ada", false),
                layer(10, "characters", false),
                layer(6, "camera notes", true),
            ],
        },
        Q0rg {
            q0rg_id: 2,
            name: "ada run cycle".into(),
            frame_count: 12,
            script: String::new(),
            layers: vec![layer(1, "body", true), layer(2, "face", true)],
        },
    ];
    let folder = |parent| LayerMetadata {
        kind: LayerKind::Folder,
        parent_folder_id: parent,
        ..Default::default()
    };
    let child = |parent| LayerMetadata {
        parent_folder_id: Some(parent),
        ..Default::default()
    };
    project.layer_metadata = [
        (LayerKey::new(1, 10), folder(None)),
        (LayerKey::new(1, 11), folder(Some(10))),
        (LayerKey::new(1, 2), child(11)),
        (LayerKey::new(1, 3), child(11)),
        (LayerKey::new(1, 4), child(10)),
    ]
    .into_iter()
    .collect();
    project.runtime.library_folders = vec![
        "backgrounds".into(),
        "characters".into(),
        "characters/ada".into(),
        "characters/bram".into(),
    ];
    project
        .runtime
        .library_item_folders
        .insert(LibraryItemReference::Q0rg(2), "characters/ada".into());
    project
}
fn person(id: i64, name: &str, seen: i64) -> Person {
    Person {
        id,
        username: name.into(),
        pfp_url: String::new(),
        last_seen: seen,
    }
}
fn signed_in(me: i64) -> Online {
    let now = unix_now();
    let mut online = Online {
        open: true,
        ..Online::default()
    };
    online.user = Some(person(
        me,
        ["", "ada_test", "bram_test", "cleo_test"][me as usize],
        now,
    ));
    online.lists_at = Some(Instant::now());
    for p in [
        person(1, "ada_test", now - 30),
        person(2, "bram_test", now - 100),
        person(3, "cleo_test", now - 7200),
        person(4, "dmitri", now - 90_000),
    ] {
        online.people.insert(p.id, p);
    }
    online.friends = vec![
        FriendRow {
            person: person(2, "bram_test", now - 100),
            status: "accepted".into(),
            since: now - 9000,
        },
        FriendRow {
            person: person(3, "cleo_test", now - 7200),
            status: "accepted".into(),
            since: now - 9000,
        },
        FriendRow {
            person: person(4, "dmitri", now - 90_000),
            status: "incoming".into(),
            since: now - 600,
        },
    ];
    online
}
fn room(online: &mut Online, app: &mut EditorApp, me: i64) {
    let project = project();
    let tab = app.open_online_project(project.clone()).unwrap();
    app.session.current_q0rg_id = 1;
    app.session.current_layer_id = if me == 1 { 6 } else { 3 };
    let layer = |l| Grant {
        scope: Scope::LayerFolder {
            q0rg_id: 1,
            layer_id: l,
        },
    };
    online.room = Some(RoomSession {
        id: "893c07e8a83b11bc3bb37bf6f870f049".into(),
        name: "rooftop chase".into(),
        owner_id: 1,
        tab_id: tab,
        revision: 42,
        confirmed: project,
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
                grants: vec![layer(11)],
                banned: false,
            },
            Member {
                user_id: 3,
                rank: Rank::Viewer,
                grants: Vec::new(),
                banned: false,
            },
        ],
        policy: RoomPolicy {
            editor_grants: vec![Grant {
                scope: Scope::Layer {
                    q0rg_id: 1,
                    layer_id: 6,
                },
            }],
            ..RoomPolicy::default()
        },
        presence: vec![
            PresenceRow {
                user_id: 2,
                presence: Presence {
                    q0rg_id: Some(1),
                    layer_id: Some(2),
                    frame: Some(11),
                    x: None,
                    y: None,
                },
            },
            PresenceRow {
                user_id: 1,
                presence: Presence {
                    q0rg_id: Some(1),
                    layer_id: Some(6),
                    frame: Some(0),
                    x: None,
                    y: None,
                },
            },
        ],
        invites: Vec::new(),
        pending: None,
        events: BTreeMap::new(),
        local_revision: app.state.revision(),
        changed_at: None,
        history: None,
        last_presence: Instant::now(),
        sent_presence: None,
        sync_job: None,
        rejection: None,
        read_only: None,
    });
    online.view.joined_room();
}

fn shoot(
    dir: &std::path::Path,
    name: &str,
    size: [f32; 2],
    theme: &str,
    online: &mut Online,
    app: &mut EditorApp,
) {
    let preset = crate::settings::Theme::PRESETS
        .iter()
        .find(|p| p.name == theme)
        .expect("theme preset");
    app.settings.theme = crate::settings::Theme::from_builtin(preset);
    let ctx = Context::default();
    // the editor's own fonts and system fallbacks, as at startup
    let _ = crate::q0lang::fonts::install(&ctx, &app.settings.q0lang_font_name);
    app.settings.theme.apply(&ctx);
    let mut canvas = Canvas::new(size[0] as usize, size[1] as usize);
    let mut last = None;
    for _ in 0..4 {
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size.into())),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.label(egui::RichText::new("stage").weak());
                });
                online.show_panel(app, ctx);
            },
        );
        canvas.textures(&output.textures_delta);
        last = Some(output);
    }
    let output = last.unwrap();
    let primitives = ctx.tessellate(output.shapes, output.pixels_per_point);
    canvas.paint(&primitives);
    canvas.save(&dir.join(format!("{name}.png")));
}

#[test]
#[ignore = "writes review pictures; set Q0EDITOR_ONLINE_SHOTS"]
fn panel_pictures() {
    let Some(dir) = std::env::var_os("Q0EDITOR_ONLINE_SHOTS") else {
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    std::fs::create_dir_all(&dir).unwrap();
    let dark = "Dark (default)";
    let size = [460.0, 780.0];

    let mut app = EditorApp::default();
    let mut online = Online {
        open: true,
        ..Online::default()
    };
    shoot(&dir, "01-signed-out", size, dark, &mut online, &mut app);

    online.login = Some(Login {
        code: "221a8c8514".into(),
        url: "https://q0id.com/online/authorize?user_code=221a8c8514".into(),
        expires: Instant::now() + Duration::from_secs(512),
    });
    shoot(
        &dir,
        "02-waiting-for-approval",
        size,
        dark,
        &mut online,
        &mut app,
    );

    online.login = None;
    online.login_ended = Some("the sign-in was declined in the browser".into());
    shoot(
        &dir,
        "03-sign-in-declined",
        size,
        dark,
        &mut online,
        &mut app,
    );

    let mut app = EditorApp::default();
    app.open_online_project(project());
    let mut online = signed_in(1);
    let now = unix_now();
    online.invites = vec![InviteRow {
        id: "i1".into(),
        room_id: "invited-room".into(),
        room_name: "storyboard jam".into(),
        inviter_id: 3,
        inviter_name: "cleo_test".into(),
        inviter_pfp_url: String::new(),
        member: Some(Member {
            user_id: 1,
            rank: Rank::Editor,
            grants: Vec::new(),
            banned: false,
        }),
        expires_at: now + 6 * 86_400,
    }];
    online.rooms = vec![
        RoomRow {
            id: "893c07e8a83b11bc3bb37bf6f870f049".into(),
            name: "rooftop chase".into(),
            owner_id: 1,
            owner_name: "ada_test".into(),
            rank: Rank::Owner,
            closed: false,
            updated_at: now - 120,
            member_count: 3,
        },
        RoomRow {
            id: "c7e15c23a7b09098894b0f3531a1a765".into(),
            name: "bram's sketchbook".into(),
            owner_id: 2,
            owner_name: "bram_test".into(),
            rank: Rank::Viewer,
            closed: false,
            updated_at: now - 7200,
            member_count: 2,
        },
        RoomRow {
            id: "0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f".into(),
            name: "old intro".into(),
            owner_id: 1,
            owner_name: "ada_test".into(),
            rank: Rank::Owner,
            closed: true,
            updated_at: now - 900_000,
            member_count: 1,
        },
    ];
    shoot(&dir, "04-rooms", size, dark, &mut online, &mut app);

    online.view.tab = Tab::Friends;
    online.view.search = "cl".into();
    online.results = Some(vec![person(3, "cleo_test", 0), person(5, "clementine", 0)]);
    online.blocked = vec![person(9, "noisy_neighbour", 0)];
    shoot(&dir, "05-friends", size, dark, &mut online, &mut app);

    let mut app = EditorApp::default();
    let mut online = signed_in(1);
    room(&mut online, &mut app, 1);
    online.say(
        "joined “rooftop chase”. it opened in its own tab; your other documents are untouched",
        Tone::Success,
    );
    shoot(
        &dir,
        "06-room-owner",
        [460.0, 1100.0],
        dark,
        &mut online,
        &mut app,
    );

    online.view.expanded = Some(2);
    online.notice = None;
    shoot(
        &dir,
        "07-room-owner-editing-bram",
        [460.0, 1300.0],
        dark,
        &mut online,
        &mut app,
    );

    let mut app = EditorApp::default();
    let mut online = signed_in(2);
    room(&mut online, &mut app, 2);
    online.recovery = Some(99);
    online.connection =
        Some("can’t reach the q0id server. check your connection; q0editor keeps trying".into());
    online.say("your room permissions do not cover the layer “sky” in “main”. your change was kept in a recovered tab and the room copy was restored", Tone::Error);
    shoot(
        &dir,
        "08-room-editor-recovery",
        [460.0, 1000.0],
        dark,
        &mut online,
        &mut app,
    );

    let mut app = EditorApp::default();
    let mut online = signed_in(1);
    room(&mut online, &mut app, 1);
    online.view.expanded = Some(2);
    shoot(
        &dir,
        "09-room-owner-light-narrow",
        [330.0, 1300.0],
        "Light",
        &mut online,
        &mut app,
    );

    // a room created before editors could draw by default
    let mut app = EditorApp::default();
    let mut online = signed_in(1);
    room(&mut online, &mut app, 1);
    let session = online.room.as_mut().unwrap();
    session.policy.editor_grants.clear();
    session.members[1].grants.clear();
    online.notice = None;
    shoot(
        &dir,
        "10-room-owner-stuck-editor",
        [460.0, 1000.0],
        dark,
        &mut online,
        &mut app,
    );
}

fn find_text(shapes: &[egui::epaint::ClippedShape], wanted: &str) -> Option<egui::Pos2> {
    fn walk(shape: &egui::Shape, wanted: &str) -> Option<egui::Pos2> {
        match shape {
            egui::Shape::Text(text) if text.galley.job.text == wanted => {
                Some(text.pos + text.galley.size() * 0.5)
            }
            egui::Shape::Vec(shapes) => shapes.iter().find_map(|s| walk(s, wanted)),
            _ => None,
        }
    }
    shapes.iter().find_map(|s| walk(&s.shape, wanted))
}

/// Open each dropdown in the panel and keep it open: frames must stay short
/// and the window must not grow.
#[test]
fn online_panel_dropdowns_stay_fast_and_still() {
    for (label, offset, expanded) in [("Join as", 70.0, None), ("Rank", 60.0, Some(2))] {
        let mut app = EditorApp::default();
        let mut online = signed_in(1);
        room(&mut online, &mut app, 1);
        online.view.tab = Tab::Room;
        online.view.expanded = expanded;
        online.notice = None;
        let ctx = Context::default();
        crate::theme::install(&ctx);
        app.settings.theme.apply(&ctx);
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1000.0, 1400.0));
        let mut time = 0.0;
        let mut run = |online: &mut Online, app: &mut EditorApp, events: Vec<egui::Event>| {
            time += 1.0 / 60.0;
            let started = Instant::now();
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(screen),
                    time: Some(time),
                    events,
                    ..Default::default()
                },
                |ctx| online.show_panel(app, ctx),
            );
            (output, started.elapsed())
        };
        let mut shapes = Vec::new();
        for _ in 0..6 {
            shapes = run(&mut online, &mut app, Vec::new()).0.shapes;
        }
        let at = find_text(&shapes, label).unwrap_or_else(|| panic!("{label} not drawn"))
            + egui::vec2(offset, 0.0);
        let click = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::default(),
        };
        run(
            &mut online,
            &mut app,
            vec![egui::Event::PointerMoved(at), click(true)],
        );
        run(&mut online, &mut app, vec![click(false)]);
        let before = ctx.memory(|m| m.area_rect(egui::Id::new("q0editor_online")));
        let mut slowest = Duration::ZERO;
        for _ in 0..120 {
            slowest = slowest.max(run(&mut online, &mut app, Vec::new()).1);
        }
        let after = ctx.memory(|m| m.area_rect(egui::Id::new("q0editor_online")));
        let open = ctx.memory(|m| m.any_popup_open());
        eprintln!("{label}: popup open={open} slowest={slowest:?} window {before:?} -> {after:?}");
        assert!(open, "{label} dropdown did not open");
        assert!(
            slowest < Duration::from_millis(250),
            "{label}: slow frame {slowest:?}"
        );
        assert_eq!(
            before, after,
            "{label}: the window moved or grew with the dropdown open"
        );
    }
}

/// The canvas a real editor frame would have, with the online panel closed
/// or open beside it, and the chat overlay on top.
fn shoot_canvas(
    dir: &std::path::Path,
    name: &str,
    size: [f32; 2],
    online: &mut Online,
    app: &mut EditorApp,
) {
    let ctx = Context::default();
    let _ = crate::q0lang::fonts::install(&ctx, &app.settings.q0lang_font_name);
    app.settings.theme.apply(&ctx);
    let mut canvas = Canvas::new(size[0] as usize, size[1] as usize);
    let mut last = None;
    for _ in 0..4 {
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size.into())),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let rect = ui.max_rect();
                    ui.painter()
                        .rect_filled(rect, 0.0, app.settings.theme.canvas_bg.to_color32());
                    let stage = egui::Rect::from_center_size(
                        rect.center(),
                        egui::vec2(rect.width() * 0.8, rect.height() * 0.7),
                    );
                    ui.painter().rect_filled(stage, 0.0, Color32::WHITE);
                    // a sketch, so the overlay is judged against artwork
                    for (i, color) in [
                        Color32::from_rgb(40, 40, 60),
                        Color32::from_rgb(200, 60, 60),
                    ]
                    .into_iter()
                    .enumerate()
                    {
                        ui.painter().circle_filled(
                            stage.left_bottom() + egui::vec2(120.0 + 90.0 * i as f32, -110.0),
                            70.0,
                            color,
                        );
                    }
                    ctx.data_mut(|d| d.insert_temp(egui::Id::new(super::CANVAS_RECT), rect));
                });
                online.show_panel(app, ctx);
                online.show_overlay(app, ctx);
            },
        );
        canvas.textures(&output.textures_delta);
        last = Some(output);
    }
    let output = last.unwrap();
    let primitives = ctx.tessellate(output.shapes, output.pixels_per_point);
    canvas.paint(&primitives);
    canvas.save(&dir.join(format!("{name}.png")));
}

#[test]
#[ignore = "writes review pictures; set Q0EDITOR_ONLINE_SHOTS"]
fn chat_pictures() {
    let Some(dir) = std::env::var_os("Q0EDITOR_ONLINE_SHOTS") else {
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    std::fs::create_dir_all(&dir).unwrap();
    let now = unix_now();
    let mut app = EditorApp::default();
    let mut online = signed_in(1);
    room(&mut online, &mut app, 1);
    let line = |id: i64, user_id: i64, body: &str| chat::ChatMessage {
        id,
        user_id,
        body: body.into(),
        created_at: now - 300 + id * 20,
    };
    online.room_chat_arrived(
        &online.room.as_ref().unwrap().id.clone(),
        vec![
            line(1, 2, "pushed the run cycle to frame 24"),
            line(2, 1, "nice! the arms look way better"),
            line(
                3,
                3,
                "can someone unlock the sky layer for me? i only have viewer",
            ),
        ],
    );
    let id = online.room.as_ref().unwrap().id.clone();
    online.room_chat_arrived(&id, vec![line(4, 2, "on it, give me a sec")]);
    online.room_chat_arrived(
        &id,
        vec![line(
            5,
            1,
            "@cleo_test you're an editor now, the sky folder is yours. careful with the clouds, they're tweened",
        )],
    );
    online.notice = None;
    online.view.tab = Tab::Chat;
    shoot(
        &dir,
        "11-room-chat",
        [460.0, 780.0],
        "Dark (default)",
        &mut online,
        &mut app,
    );

    online.chat.conversations = vec![
        chat::Conversation {
            user_id: 2,
            last: Some(chat::DirectMessage {
                id: 9,
                from: 2,
                to: 1,
                body: "want to do the fight scene together tomorrow?".into(),
                created_at: now - 400,
            }),
            unread: 2,
            blocked: false,
        },
        chat::Conversation {
            user_id: 4,
            last: Some(chat::DirectMessage {
                id: 3,
                from: 1,
                to: 4,
                body: "thanks for the brush preset!".into(),
                created_at: now - 90_000,
            }),
            unread: 0,
            blocked: false,
        },
    ];
    online.chat.conversations_loaded = true;
    online.chat.unread = 2;
    online.view.tab = Tab::Messages;
    shoot(
        &dir,
        "12-messages",
        [460.0, 780.0],
        "Dark (default)",
        &mut online,
        &mut app,
    );

    let dm = |id: i64, from: i64, body: &str, ago: i64| chat::DirectMessage {
        id,
        from,
        to: if from == 1 { 2 } else { 1 },
        body: body.into(),
        created_at: now - ago,
    };
    online.chat.threads.insert(
        2,
        vec![
            dm(5, 2, "yo, saw your rooftop chase room", 3000),
            dm(6, 1, "haha yeah, still blocking it out", 2900),
            dm(7, 1, "the jump at frame 40 needs work", 2890),
            dm(
                8,
                2,
                "i can take the fight scene if you want.\nsend me the layers you want me on",
                500,
            ),
            dm(9, 2, "want to do the fight scene together tomorrow?", 400),
        ],
    );
    online.chat.thread_with = Some(2);
    online.chat.dm_draft = "sure, 6pm?".into();
    shoot(
        &dir,
        "13-thread",
        [460.0, 780.0],
        "Dark (default)",
        &mut online,
        &mut app,
    );
    shoot(
        &dir,
        "14-thread-light",
        [400.0, 780.0],
        "Light",
        &mut online,
        &mut app,
    );

    // the overlay over the canvas with the panel closed, like in a game
    online.open = false;
    online.chat.feed.clear();
    online.room_chat_arrived(&id, vec![line(6, 2, "frame 40 is fixed, check it")]);
    online.room_chat_arrived(&id, vec![line(7, 3, "looks sick")]);
    online.dm_arrived(
        dm(10, 4, "hey, are you free to review my walk cycle?", 5),
        true,
    );
    shoot_canvas(&dir, "15-overlay", [1100.0, 700.0], &mut online, &mut app);
    online.chat.input_open = true;
    online.chat.draft = "/r sure, send it over".into();
    shoot_canvas(
        &dir,
        "16-overlay-typing",
        [1100.0, 700.0],
        &mut online,
        &mut app,
    );
    online.chat.input_open = false;
    online.chat.feed.clear();
    shoot_canvas(
        &dir,
        "17-overlay-idle-pill",
        [1100.0, 700.0],
        &mut online,
        &mut app,
    );
}
