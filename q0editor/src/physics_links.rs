//! Links in the physics editor: ropes, chains, springs, hinges and welds
//! that tie one object to another or to the stage. Their part of the
//! physics window, their handles on the stage, and the drawings a bake
//! leaves of the ropes, chains and springs that show.

use egui::{Color32, Painter, RichText, Stroke, Ui};
use q0physics::BodyPose;
use q0s_format::physics::{
    BodyKind, LinkKind, LinkLook, PhysicsBody, PhysicsLink, MAX_LINK_SEGMENTS,
};
use q0s_format::v2::{Placement, ProjectV2, Rgba, Target, Transform2D, Tween, Vec2};

use crate::app::EditorApp;
use crate::physics::{Edit, Simulation};
use crate::render::StageView;
use crate::state::RigMode;

/// A link end being dragged on the stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LinkDrag {
    link_id: u16,
    end: End,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum End {
    From,
    To,
}

/// Ready-made links the window offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Preset {
    Rope,
    Chain,
    Spring,
    Hinge,
    Glue,
}

impl Preset {
    const ALL: [Self; 5] = [
        Self::Rope,
        Self::Chain,
        Self::Spring,
        Self::Hinge,
        Self::Glue,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Rope => "Rope",
            Self::Chain => "Chain",
            Self::Spring => "Spring",
            Self::Hinge => "Hinge",
            Self::Glue => "Glue",
        }
    }

    fn hint(self) -> &'static str {
        match self {
            Self::Rope => "a light rope it dangles and swings on",
            Self::Chain => "a heavy chain of links, drawn link by link",
            Self::Spring => "a springy coil it bounces on",
            Self::Hinge => "pinned where they meet; it swings around the pin",
            Self::Glue => "stuck where it is; it wobbles along like jelly",
        }
    }

    fn kind(self) -> LinkKind {
        match self {
            Self::Rope | Self::Chain => LinkKind::Rope,
            Self::Spring => LinkKind::Spring,
            Self::Hinge => LinkKind::Hinge,
            Self::Glue => LinkKind::Weld,
        }
    }

    fn apply(self, link: &mut PhysicsLink) {
        if self == Self::Chain {
            link.look = LinkLook::Chain;
            link.segments = 16;
            link.mass = 1.0;
            link.width = 6.0;
            link.damping = 0.1;
            link.color = Rgba {
                r: 120,
                g: 122,
                b: 130,
                a: 255,
            };
        }
        if self == Self::Spring {
            link.width = 6.0;
            link.color = Rgba {
                r: 110,
                g: 115,
                b: 125,
                a: 255,
            };
        }
    }
}

fn kind_label(kind: LinkKind, mode: RigMode) -> &'static str {
    match (kind, mode) {
        (LinkKind::Rope, _) => "Rope",
        (LinkKind::Spring, _) => "Spring",
        (LinkKind::Hinge, _) => "Hinge",
        (LinkKind::Weld, RigMode::Simple) => "Glue",
        (LinkKind::Weld, RigMode::Pro) => "Weld",
    }
}

fn look_label(look: LinkLook) -> &'static str {
    match look {
        LinkLook::Hidden => "Hidden",
        LinkLook::Line => "Line",
        LinkLook::Chain => "Chain",
        LinkLook::Coil => "Coil",
    }
}

/// What the rigidity slider means for `kind`, and a hint.
fn stiffness_label(kind: LinkKind) -> (&'static str, &'static str) {
    match kind {
        LinkKind::Rope => ("Stiffness", "0 is a floppy string, 1 a stiff wire"),
        LinkKind::Spring => ("Strength", "how hard it pulls back to its length"),
        LinkKind::Hinge => (
            "Springs back",
            "0 swings freely; more pulls it back to how it hangs now",
        ),
        LinkKind::Weld => ("Rigidity", "0 wobbles like jelly, 1 holds solid"),
    }
}

/// How hard a link may be pulled before it snaps, for Simple mode.
const SNAP_PRESETS: [(&str, f32); 3] = [("Never", 0.0), ("Strong", 6000.0), ("Weak", 1500.0)];

fn pose_point(pose: BodyPose, [x, y]: [f32; 2]) -> [f32; 2] {
    let (sin, cos) = pose.angle.sin_cos();
    [
        pose.center[0] + cos * x - sin * y,
        pose.center[1] + sin * x + cos * y,
    ]
}

fn local_point(pose: BodyPose, [x, y]: [f32; 2]) -> [f32; 2] {
    let (sin, cos) = pose.angle.sin_cos();
    let (dx, dy) = (x - pose.center[0], y - pose.center[1]);
    [cos * dx + sin * dy, -sin * dx + cos * dy]
}

fn distance(a: [f32; 2], b: [f32; 2]) -> f32 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

/// The hitbox pose and half size of object `instance_id` on `frame`: its
/// body's, or the plain fit of an object that is no body yet.
fn hitbox(app: &EditorApp, instance_id: u32, frame: u16) -> Option<(BodyPose, [f32; 2])> {
    let q0rg_id = app.session.current_q0rg_id;
    let fallback = PhysicsBody::new(q0rg_id, instance_id);
    let body = app
        .state
        .project
        .runtime
        .physics
        .body(q0rg_id, instance_id)
        .unwrap_or(&fallback);
    let (fit, pose, _) = crate::physics::cached_collider(app, body, frame)?;
    let scale = [
        body.shape_scale * body.size[0],
        body.shape_scale * body.size[1],
    ];
    Some((
        pose,
        [
            fit.fit.half_extents[0] * scale[0],
            fit.fit.half_extents[1] * scale[1],
        ],
    ))
}

/// Where the ends of `link` are on the stage on `frame`.
fn link_ends(app: &EditorApp, link: &PhysicsLink, frame: u16) -> Option<([f32; 2], [f32; 2])> {
    let (to_pose, _) = hitbox(app, link.to, frame)?;
    let to_point = pose_point(to_pose, link.to_anchor);
    let from_point = if !link.kind.spans() {
        to_point
    } else if link.from == 0 {
        link.from_anchor
    } else {
        pose_point(hitbox(app, link.from, frame)?.0, link.from_anchor)
    };
    Some((from_point, to_point))
}

/// The point of a box `half` wide and tall around the origin nearest
/// `point`.
fn nearest_on_box(point: [f32; 2], half: [f32; 2]) -> [f32; 2] {
    [
        point[0].clamp(-half[0], half[0]),
        point[1].clamp(-half[1], half[1]),
    ]
}

/// Ties the selected objects with a new link: the higher of two holds the
/// lower, and a single object hangs from the stage. Objects that are no
/// bodies yet join the physics, the holder moving as animated and the tied
/// one falling. Returns the new link's id.
fn tie_selected(app: &mut EditorApp, preset: Preset) -> Result<u16, String> {
    let q0rg_id = app.session.current_q0rg_id;
    let frame = app.session.current_frame;
    let refs =
        crate::selection_edit::selected_object_refs(&app.state.project, &app.session.selection)
            .into_iter()
            .filter(|reference| reference.q0rg_id == q0rg_id)
            .collect::<Vec<_>>();
    let mut ids = Vec::new();
    for reference in &refs {
        if let Some((instance_id, _)) = crate::rigging::ensure_selected_track_instance_id(
            &mut app.state.project,
            q0rg_id,
            reference.layer_id,
            reference.placement_idx,
            frame,
        ) {
            if !ids.contains(&instance_id) {
                ids.push(instance_id);
            }
        }
        if ids.len() == 2 {
            break;
        }
    }
    let mut ends = ids
        .iter()
        .filter_map(|id| hitbox(app, *id, frame).map(|(pose, half)| (*id, pose, half)))
        .collect::<Vec<_>>();
    if ends.is_empty() {
        return Err("select one object to hang from the stage, or two to tie together".to_string());
    }
    // The higher one holds; stage y points down.
    ends.sort_by(|a, b| a.1.center[1].total_cmp(&b.1.center[1]));
    let (to, to_pose, to_half) = *ends.last().expect("checked above");
    let holder = (ends.len() == 2).then(|| ends[0]);
    let kind = preset.kind();
    let physics = &mut app.state.project.runtime.physics;
    let link_id = physics.next_link_id(q0rg_id);
    let mut link = PhysicsLink::new(q0rg_id, link_id, kind, holder.map_or(0, |end| end.0), to);
    preset.apply(&mut link);
    match holder {
        Some((from, from_pose, from_half)) => {
            if physics.body(q0rg_id, from).is_none() {
                let mut body = PhysicsBody::new(q0rg_id, from);
                body.kind = BodyKind::Animated;
                physics.bodies.push(body);
            }
            // Each end where its object faces the other.
            link.to_anchor = nearest_on_box(local_point(to_pose, from_pose.center), to_half);
            link.from_anchor = nearest_on_box(local_point(from_pose, to_pose.center), from_half);
            let gap = distance(
                pose_point(from_pose, link.from_anchor),
                pose_point(to_pose, link.to_anchor),
            );
            link.length = gap.max(10.0);
        }
        None => {
            link.to_anchor = [0.0, -to_half[1]];
            let top = pose_point(to_pose, link.to_anchor);
            link.from_anchor = [top[0], top[1] - 80.0];
            link.length = 80.0;
        }
    }
    if physics.body(q0rg_id, to).is_none() {
        physics.bodies.push(PhysicsBody::new(q0rg_id, to));
    }
    physics.links.push(link);
    physics.sanitize();
    Ok(link_id)
}

/// The links part of the physics window.
pub(crate) fn render_links(
    app: &mut EditorApp,
    ui: &mut Ui,
    q0rg_id: u16,
    mode: RigMode,
    dim: Color32,
) {
    ui.horizontal_wrapped(|ui| {
        ui.label("Tie selected with");
        for preset in Preset::ALL {
            if ui
                .small_button(preset.label())
                .on_hover_text(preset.hint())
                .clicked()
            {
                let before = app.state.project.clone();
                match tie_selected(app, preset) {
                    Ok(link_id) => {
                        app.history.snapshot_with_session(&before, &app.session);
                        app.state.mark_dirty();
                        app.session.physics.selected_link = Some(link_id);
                        app.session.status = format!("{} added", preset.label().to_lowercase());
                    }
                    Err(error) => {
                        app.state.project = before;
                        app.session.status = error;
                    }
                }
            }
        }
    });
    let links = app
        .state
        .project
        .runtime
        .physics
        .links
        .iter()
        .filter(|link| link.q0rg_id == q0rg_id)
        .map(|link| {
            (
                link.link_id,
                link.name.clone(),
                link.kind,
                link.from,
                link.to,
            )
        })
        .collect::<Vec<_>>();
    if links.is_empty() {
        ui.label(
            RichText::new(
                "Select two objects and pick how to tie them: the higher one holds the lower. Select one to hang it from the stage.",
            )
            .small()
            .color(dim),
        );
        return;
    }
    if app
        .session
        .physics
        .selected_link
        .is_none_or(|selected| !links.iter().any(|(id, ..)| *id == selected))
    {
        app.session.physics.selected_link = Some(links[0].0);
    }
    let mut remove = None;
    for (link_id, name, kind, from, to) in &links {
        ui.horizontal(|ui| {
            let selected = app.session.physics.selected_link == Some(*link_id);
            let from_name = if *from == 0 {
                "the stage".to_string()
            } else {
                crate::physics::object_name(app, *from)
            };
            let label = format!(
                "{name} · {}: {from_name} → {}",
                kind_label(*kind, mode),
                crate::physics::object_name(app, *to)
            );
            if ui.selectable_label(selected, label).clicked() {
                app.session.physics.selected_link = Some(*link_id);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("Remove").clicked() {
                    remove = Some(*link_id);
                }
            });
        });
    }
    if let Some(link_id) = remove {
        app.history
            .snapshot_with_session(&app.state.project, &app.session);
        app.state
            .project
            .runtime
            .physics
            .links
            .retain(|link| !(link.q0rg_id == q0rg_id && link.link_id == link_id));
        app.state.mark_dirty();
        return;
    }
    let Some(link_id) = app.session.physics.selected_link else {
        return;
    };
    let Some(mut link) = app
        .state
        .project
        .runtime
        .physics
        .link(q0rg_id, link_id)
        .cloned()
    else {
        return;
    };
    let gap = link_ends(app, &link, app.session.current_frame).map(|(a, b)| distance(a, b));
    let from_name = if link.from == 0 {
        "the stage".to_string()
    } else {
        crate::physics::object_name(app, link.from)
    };
    let to_name = crate::physics::object_name(app, link.to);
    ui.add_space(4.0);
    let mut edit = Edit::default();
    egui::Grid::new(("physics_link", mode == RigMode::Pro))
        .num_columns(2)
        .spacing([10.0, 6.0])
        .show(ui, |ui| {
            ui.label("Name");
            edit.text(ui, &mut link.name, 150.0);
            ui.end_row();
            ui.label("Ties");
            ui.horizontal_wrapped(|ui| {
                ui.label(format!("{to_name} to {from_name}"));
                if link.from != 0
                    && ui
                        .small_button("Swap")
                        .on_hover_text("The other object holds")
                        .clicked()
                {
                    std::mem::swap(&mut link.from, &mut link.to);
                    std::mem::swap(&mut link.from_anchor, &mut link.to_anchor);
                    edit.snapshot = true;
                    edit.changed = true;
                }
            });
            ui.end_row();
            ui.label("Kind");
            ui.horizontal_wrapped(|ui| {
                for kind in LinkKind::ALL {
                    edit.choice(ui, &mut link.kind, kind, kind_label(kind, mode));
                }
            });
            ui.end_row();
            if link.kind.spans() {
                ui.label("Length");
                ui.horizontal(|ui| {
                    edit.drag(ui, &mut link.length, 0.5, 1.0..=100_000.0, " px");
                    if ui
                        .add_enabled(gap.is_some(), egui::Button::new("Fit").small())
                        .on_hover_text("As long as the gap between its ends on this frame")
                        .clicked()
                    {
                        if let Some(gap) = gap {
                            link.length = gap.max(1.0);
                            edit.snapshot = true;
                            edit.changed = true;
                        }
                    }
                });
                ui.end_row();
            }
            let (label, hint) = stiffness_label(link.kind);
            ui.label(label).on_hover_text(hint);
            edit.slider(ui, &mut link.stiffness, 0.0..=1.0, false);
            ui.end_row();
            ui.label("Calms down")
                .on_hover_text("0 wobbles and swings on and on; 1 settles at once");
            edit.slider(ui, &mut link.damping, 0.0..=1.0, false);
            ui.end_row();
            match link.kind {
                LinkKind::Rope => {
                    ui.label("Stretch")
                        .on_hover_text("0 holds its length; more gives like a bungee");
                    edit.slider(ui, &mut link.stretch, 0.0..=1.0, false);
                    ui.end_row();
                    ui.label("Pieces").on_hover_text(
                        "How many pieces it bends in; none keeps it a straight tether",
                    );
                    if mode == RigMode::Simple {
                        ui.horizontal_wrapped(|ui| {
                            for (label, pieces) in
                                [("None", 0), ("Few", 5), ("Some", 10), ("Many", 20)]
                            {
                                edit.choice(ui, &mut link.segments, pieces, label);
                            }
                        });
                    } else {
                        let response = ui.add(
                            egui::DragValue::new(&mut link.segments)
                                .clamp_range(0..=MAX_LINK_SEGMENTS),
                        );
                        edit.note(&response);
                    }
                    ui.end_row();
                    ui.label("Weight");
                    edit.drag(ui, &mut link.mass, 0.01, 0.001..=100_000.0, "");
                    ui.end_row();
                }
                LinkKind::Hinge => {
                    ui.label("Swings at most")
                        .on_hover_text("Degrees either way from how it hangs now; 0 is free");
                    edit.drag(ui, &mut link.swing_limit, 1.0, 0.0..=180.0, "°");
                    ui.end_row();
                }
                LinkKind::Spring | LinkKind::Weld => {}
            }
            if link.kind.spans() {
                ui.label("Looks like");
                ui.horizontal_wrapped(|ui| {
                    for look in LinkLook::ALL {
                        edit.choice(ui, &mut link.look, look, look_label(look));
                    }
                });
                ui.end_row();
                if link.look != LinkLook::Hidden {
                    ui.label("Colour");
                    edit.color(ui, &mut link.color);
                    ui.end_row();
                    ui.label("Thickness");
                    edit.drag(ui, &mut link.width, 0.1, 0.1..=500.0, " px");
                    ui.end_row();
                }
            }
            match mode {
                RigMode::Simple => {
                    ui.label("Snaps");
                    ui.horizontal(|ui| {
                        for (label, force) in SNAP_PRESETS {
                            edit.choice(ui, &mut link.break_force, force, label);
                        }
                    });
                    ui.end_row();
                }
                RigMode::Pro => {
                    ui.label("Snaps at").on_hover_text(
                        "The pull it takes, 0 never. About 1000 holds a body of mass 1 against Earth's gravity.",
                    );
                    edit.drag(ui, &mut link.break_force, 10.0, 0.0..=1.0e9, "");
                    ui.end_row();
                    ui.label("");
                    edit.checkbox(ui, &mut link.collide, "The tied objects bump into each other");
                    ui.end_row();
                    if link.kind == LinkKind::Rope {
                        ui.label("Rope collides with");
                        crate::physics::group_number(ui, &mut edit, &mut link.group);
                        ui.end_row();
                    }
                }
            }
        });
    ui.label(
        RichText::new(if link.kind.spans() {
            "Drag its ends on the stage to tie it elsewhere."
        } else {
            "Drag the pin on the stage to move where they are held together."
        })
        .small()
        .color(dim),
    );
    if edit.changed || edit.snapshot {
        crate::physics::apply_edit(app, &edit, |project| {
            if let Some(stored) = project.runtime.physics.link_mut(q0rg_id, link_id) {
                *stored = link.clone();
                project.runtime.physics.sanitize();
            }
        });
    }
}

/// The colour links show in on the stage.
fn link_color(link: &PhysicsLink) -> Color32 {
    if link.look == LinkLook::Hidden || !link.kind.spans() {
        Color32::from_rgb(255, 196, 60)
    } else {
        let Rgba { r, g, b, .. } = link.color;
        // Dark ropes would vanish on dark artwork; lift them a little.
        let lift = |value: u8| value.max(70);
        Color32::from_rgb(lift(r), lift(g), lift(b))
    }
}

/// Links on the stage while the physics window is open: where they tie,
/// and while previewing, any that the movie does not draw.
pub(crate) fn paint_links(
    app: &EditorApp,
    painter: &Painter,
    view: &StageView,
    simulation: Option<&Simulation>,
) {
    let q0rg_id = app.session.current_q0rg_id;
    let frame = app.session.current_frame;
    let selected = app.session.physics.selected_link;
    let to_screen = |point: [f32; 2]| view.stage_to_screen(Vec2::new(point[0], point[1]));
    for link in app
        .state
        .project
        .runtime
        .physics
        .links
        .iter()
        .filter(|link| link.q0rg_id == q0rg_id)
    {
        let color = link_color(link);
        let is_selected = selected == Some(link.link_id);
        if let Some(simulation) = simulation {
            if link.is_drawn() {
                continue;
            }
            let Some(lines) = simulation
                .links
                .iter()
                .find(|track| track.link_id == link.link_id)
                .and_then(|track| {
                    let offset = frame.checked_sub(simulation.start_frame)?;
                    track.frames.get(usize::from(offset))
                })
            else {
                continue;
            };
            for line in lines {
                let points = line.iter().map(|point| to_screen(*point)).collect();
                painter.add(egui::Shape::line(points, Stroke::new(1.5_f32, color)));
            }
            continue;
        }
        let Some((from, to)) = link_ends(app, link, frame) else {
            continue;
        };
        let (from_at, to_at) = (to_screen(from), to_screen(to));
        let width = if is_selected { 2.5_f32 } else { 1.5_f32 };
        match link.kind {
            LinkKind::Rope | LinkKind::Spring => {
                let stroke = Stroke::new(width, color);
                if link.look == LinkLook::Hidden {
                    painter.extend(egui::Shape::dashed_line(
                        &[from_at, to_at],
                        stroke,
                        6.0,
                        4.0,
                    ));
                } else {
                    painter.line_segment([from_at, to_at], stroke);
                }
                if link.from == 0 {
                    // A pin in the stage.
                    painter.circle_stroke(from_at, 4.0, Stroke::new(1.5_f32, color));
                    painter.line_segment(
                        [
                            from_at - egui::vec2(6.0, 0.0),
                            from_at + egui::vec2(6.0, 0.0),
                        ],
                        Stroke::new(1.5_f32, color),
                    );
                }
            }
            LinkKind::Hinge => {
                painter.circle_stroke(to_at, 5.0, Stroke::new(width, color));
                painter.circle_filled(to_at, 1.8, color);
            }
            LinkKind::Weld => {
                painter.rect_stroke(
                    egui::Rect::from_center_size(to_at, egui::vec2(9.0, 9.0)),
                    1.0,
                    Stroke::new(width, color),
                );
            }
        }
        if is_selected {
            for (end, at) in handle_points(link, from, to) {
                let at = to_screen(at);
                let fill = if end == End::From {
                    Color32::from_black_alpha(160)
                } else {
                    color
                };
                painter.circle(at, 5.0, fill, Stroke::new(1.5_f32, color));
            }
        }
    }
}

/// The ends of `link` that can be dragged, and where they are.
fn handle_points(link: &PhysicsLink, from: [f32; 2], to: [f32; 2]) -> Vec<(End, [f32; 2])> {
    if link.kind.spans() {
        vec![(End::From, from), (End::To, to)]
    } else {
        vec![(End::To, to)]
    }
}

/// Drags the ends of the selected link on the stage. Returns whether the
/// pointer is busy with them.
pub(crate) fn handle_stage(
    app: &mut EditorApp,
    response: &egui::Response,
    cursor: Option<Vec2>,
    view: &StageView,
) -> bool {
    let q0rg_id = app.session.current_q0rg_id;
    let frame = app.session.current_frame;
    let Some(link) = app
        .session
        .physics
        .selected_link
        .and_then(|id| app.state.project.runtime.physics.link(q0rg_id, id))
        .cloned()
    else {
        app.session.physics.link_drag = None;
        return false;
    };
    let Some((from, to)) = link_ends(app, &link, frame) else {
        return false;
    };
    let near = |screen: egui::Pos2| {
        handle_points(&link, from, to)
            .into_iter()
            .map(|(end, at)| {
                (
                    end,
                    view.stage_to_screen(Vec2::new(at[0], at[1]))
                        .distance(screen),
                )
            })
            .filter(|(_, distance)| *distance <= crate::physics::HANDLE_REACH)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(end, _)| end)
    };
    if response.drag_started_by(egui::PointerButton::Primary) {
        let press = response
            .ctx
            .input(|input| input.pointer.press_origin())
            .or_else(|| response.interact_pointer_pos());
        if let Some(end) = press.and_then(near) {
            app.history
                .snapshot_with_session(&app.state.project, &app.session);
            app.session.physics.link_drag = Some(LinkDrag {
                link_id: link.link_id,
                end,
            });
            return true;
        }
        return false;
    }
    let Some(drag) = app.session.physics.link_drag else {
        if response.hover_pos().and_then(near).is_some() {
            response.ctx.set_cursor_icon(egui::CursorIcon::Grab);
        }
        return false;
    };
    if drag.link_id != link.link_id || response.drag_stopped() || !response.dragged() {
        app.session.physics.link_drag = None;
        return true;
    }
    let Some(cursor) = cursor else {
        return true;
    };
    let snap = response.ctx.input(|input| input.modifiers.shift);
    let point = [cursor.x, cursor.y];
    let anchor = match drag.end {
        End::From if link.from == 0 => Some(if snap { point.map(f32::round) } else { point }),
        End::From => hitbox(app, link.from, frame)
            .map(|(pose, half)| snapped(local_point(pose, point), half, snap)),
        End::To => hitbox(app, link.to, frame)
            .map(|(pose, half)| snapped(local_point(pose, point), half, snap)),
    };
    let Some(anchor) = anchor else {
        return true;
    };
    let mut changed = link.clone();
    match drag.end {
        End::From => changed.from_anchor = anchor,
        End::To => changed.to_anchor = anchor,
    }
    if changed != link {
        if let Some(stored) = app
            .state
            .project
            .runtime
            .physics
            .link_mut(q0rg_id, link.link_id)
        {
            *stored = changed;
        }
        app.state.project.runtime.physics.sanitize();
        app.state.mark_dirty();
    }
    true
}

/// With `snap`, `local` jumps to the nearest of the hitbox's middle, edge
/// middles and corners.
fn snapped(local: [f32; 2], half: [f32; 2], snap: bool) -> [f32; 2] {
    if !snap {
        return local;
    }
    let mut best = [0.0, 0.0];
    for x in [-half[0], 0.0, half[0]] {
        for y in [-half[1], 0.0, half[1]] {
            if distance([x, y], local) < distance(best, local) {
                best = [x, y];
            }
        }
    }
    best
}

/// Attachment positions on the *baked* body, including frames interpolated
/// between sparse simulation keys. Evaluate only the active cell and its tween
/// destinations, so long animations never rescan all keys for every frame.
fn drawn_anchors(
    project: &ProjectV2,
    simulation: &Simulation,
    instance_id: u32,
    anchor: [f32; 2],
    frames: usize,
) -> Vec<Option<[f32; 2]>> {
    use std::collections::{BTreeMap, BTreeSet};
    if instance_id == 0 {
        return vec![Some(anchor); frames];
    }
    let missing = || vec![None; frames];
    let Some(track) = simulation
        .tracks
        .iter()
        .find(|track| track.instance_id == instance_id)
    else {
        return missing();
    };
    let Some(layer) = project
        .q0rgs
        .iter()
        .find(|q0rg| q0rg.q0rg_id == simulation.q0rg_id)
        .and_then(|q0rg| {
            q0rg.layers
                .iter()
                .find(|layer| layer.layer_id == track.start.layer_id)
        })
    else {
        return missing();
    };
    let keys = layer.keyframe_frames();
    let mut cells = BTreeMap::<u16, Vec<&Placement>>::new();
    for placement in &layer.placements {
        cells.entry(placement.frame).or_default().push(placement);
    }
    let mut active_key = None;
    let mut sample = q0s_format::v2::Layer {
        layer_id: layer.layer_id,
        timeline_frame_count: layer.timeline_frame_count,
        name: String::new(),
        explicit_keyframes: Vec::new(),
        placements: Vec::new(),
    };
    (0..frames)
        .map(|offset| {
            let frame = simulation
                .start_frame
                .checked_add(u16::try_from(offset).ok()?)?;
            let key = keys.get(keys.partition_point(|key| *key <= frame).checked_sub(1)?)?;
            if active_key != Some(*key) {
                active_key = Some(*key);
                sample.explicit_keyframes.clear();
                sample.explicit_keyframes.push(*key);
                sample.placements.clear();
                if let Some(current) = cells.get(key) {
                    let destinations = current
                        .iter()
                        .filter_map(|placement| placement.tween.to_frame())
                        .filter(|to_frame| *to_frame > *key)
                        .collect::<BTreeSet<_>>();
                    sample
                        .placements
                        .extend(current.iter().map(|placement| (*placement).clone()));
                    for destination in destinations {
                        if let Some(placements) = cells.get(&destination) {
                            sample
                                .placements
                                .extend(placements.iter().map(|placement| (*placement).clone()));
                        }
                    }
                }
            }
            let transform = q0s_format::raster::active_placements_at(&sample, frame)
                .into_iter()
                .find(|(index, _)| sample.placements[*index].instance_id == instance_id)?
                .1;
            Some(q0physics::attachment_point(
                track.start.transform,
                track.start_pose,
                q0s_format::transform::Affine::from_transform(transform),
                anchor,
            ))
        })
        .collect()
}

/// Draws every shown link of `simulation` on a layer of its own, a drawing
/// per frame, replacing what an earlier bake drew over the same frames.
pub(crate) fn bake_links(
    project: &mut ProjectV2,
    simulation: &Simulation,
) -> Result<usize, String> {
    use crate::sim_bake;
    let q0rg_id = simulation.q0rg_id;
    let start = simulation.start_frame;
    let mut ids = sim_bake::AssetIds::new(project);
    let mut drawn = 0;
    for track in &simulation.links {
        let Some(link) = project
            .runtime
            .physics
            .link(q0rg_id, track.link_id)
            .cloned()
            .filter(PhysicsLink::is_drawn)
        else {
            continue;
        };
        // Bodies are already keyed by physics::bake. Use those rendered keys,
        // not the original per-step pose; a sparse tween can be far away under
        // tension. Keep its rope attached without forcing extra object keys.
        let from = drawn_anchors(
            project,
            simulation,
            link.from,
            link.from_anchor,
            track.frames.len(),
        );
        let to = drawn_anchors(
            project,
            simulation,
            link.to,
            link.to_anchor,
            track.frames.len(),
        );
        let layer_id = sim_bake::effect_layer(project, q0rg_id, link.layer_id, &link.name)?;
        if let Some(stored) = project.runtime.physics.link_mut(q0rg_id, link.link_id) {
            stored.layer_id = Some(layer_id);
        }
        let last = start + (track.frames.len().max(1) - 1) as u16;
        let folder = format!("{}/{}", sim_bake::PHYSICS_FOLDER, link.name);
        sim_bake::clear_range(
            project,
            q0rg_id,
            layer_id,
            start,
            last,
            sim_bake::PHYSICS_FOLDER,
        );
        for (offset, lines) in track.frames.iter().enumerate() {
            let frame = start + offset as u16;
            let lines = q0physics::links::fit_link_lines(
                lines,
                track.attached_ends.get(offset).map_or(&[], Vec::as_slice),
                from[offset],
                to[offset],
            );
            let strokes = q0physics::links::link_strokes(&lines, link.look, link.width);
            // One drawing per line thickness: a chain's rings and edges
            // differ.
            let mut widths = Vec::<f32>::new();
            for stroke in &strokes {
                if !widths.contains(&stroke.width) {
                    widths.push(stroke.width);
                }
            }
            let mut placements = Vec::new();
            for width in widths {
                let paths = strokes
                    .iter()
                    .filter(|stroke| stroke.width == width)
                    .map(|stroke| {
                        let mut path = sim_bake::polygon_path(&stroke.points);
                        path.closed = stroke.closed;
                        path
                    })
                    .collect();
                let asset_id = sim_bake::add_line_drawing(
                    project,
                    &mut ids,
                    paths,
                    q0s_format::v2::Stroke {
                        color: link.color,
                        width,
                        cap: q0s_format::geom::CapShape::Round,
                        join: q0s_format::geom::JoinShape::Round,
                    },
                    format!("{} {}", link.name, u32::from(frame) + 1),
                    &folder,
                )?;
                placements.push(Placement {
                    pivot: None,
                    instance_id: 0,
                    frame,
                    target: Target::Asset(asset_id),
                    transform: Transform2D::IDENTITY,
                    tween: Tween::None,
                    fx: Default::default(),
                });
            }
            sim_bake::key_frame(project, q0rg_id, layer_id, frame, placements);
            drawn += 1;
        }
        sim_bake::reach_frame(project, q0rg_id, layer_id, last);
    }
    Ok(drawn)
}

/// The layers earlier bakes drew the links of `q0rg_id` on.
pub(crate) fn baked_layers(project: &ProjectV2, q0rg_id: u16) -> Vec<u16> {
    project
        .runtime
        .physics
        .links
        .iter()
        .filter(|link| link.q0rg_id == q0rg_id)
        .filter_map(|link| link.layer_id)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{PlacementRef, Selection};
    use q0s_format::physics::PhysicsScene;
    use q0s_format::v2::{Anchor, Asset, Layer, Path, ProjectMeta, Q0rg, VectorAsset};

    /// Two 20 px squares, one above the other, each on its own layer.
    fn app_with_two_squares() -> EditorApp {
        let corner = |x: f32, y: f32| Anchor {
            point: Vec2::new(x, y),
            in_handle: None,
            out_handle: None,
        };
        let placed = |instance_id: u32, y: f32| Placement {
            pivot: None,
            instance_id,
            frame: 0,
            target: Target::Asset(1),
            transform: Transform2D {
                tx: 200.0,
                ty: y,
                ..Transform2D::IDENTITY
            },
            tween: Tween::None,
            fx: Default::default(),
        };
        let layer = |layer_id: u16, placement: Placement| Layer {
            layer_id,
            timeline_frame_count: None,
            name: format!("layer {layer_id}"),
            explicit_keyframes: vec![0],
            placements: vec![placement],
        };
        let mut project = ProjectV2 {
            meta: ProjectMeta {
                name: "links".into(),
                fps: 24,
                stage_width: 400,
                stage_height: 300,
                background_color: Rgba {
                    r: 255,
                    g: 255,
                    b: 255,
                    a: 255,
                },
                entry_q0rg_id: 1,
            },
            assets: vec![Asset::Vector(VectorAsset {
                asset_id: 1,
                paths: vec![Path {
                    anchors: vec![
                        corner(-10.0, -10.0),
                        corner(10.0, -10.0),
                        corner(10.0, 10.0),
                        corner(-10.0, 10.0),
                    ],
                    closed: true,
                }],
                fill: Some(Rgba {
                    r: 0,
                    g: 0,
                    b: 0,
                    a: 255,
                }),
                stroke: None,
            })],
            asset_names: Default::default(),
            asset_appearances: Default::default(),
            layer_metadata: Default::default(),
            audio_clips: Vec::new(),
            runtime: Default::default(),
            q0rgs: vec![Q0rg {
                q0rg_id: 1,
                name: "Stage".into(),
                frame_count: 48,
                script: String::new(),
                layers: vec![layer(1, placed(7, 40.0)), layer(2, placed(8, 140.0))],
            }],
        };
        project.runtime.physics.scenes.push(PhysicsScene::new(1));
        let mut app = EditorApp::default();
        app.state.project = project;
        app.session.current_q0rg_id = 1;
        app.session.current_frame = 0;
        app
    }

    fn select(app: &mut EditorApp, layers: &[u16]) {
        app.session.selection = Selection::Multi(
            layers
                .iter()
                .map(|layer_id| PlacementRef {
                    q0rg_id: 1,
                    layer_id: *layer_id,
                    placement_idx: 0,
                })
                .collect(),
        );
    }

    fn render(app: &mut EditorApp) {
        let ctx = egui::Context::default();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            crate::physics::render_window(app, ctx);
        });
    }

    #[test]
    fn tying_two_objects_hangs_the_lower_from_the_higher() {
        let mut app = app_with_two_squares();
        // Picked bottom first: the higher one still holds.
        select(&mut app, &[2, 1]);
        let link_id = tie_selected(&mut app, Preset::Chain).unwrap();
        let physics = &app.state.project.runtime.physics;
        let link = physics.link(1, link_id).unwrap();
        assert_eq!((link.from, link.to), (7, 8));
        assert_eq!(link.kind, LinkKind::Rope);
        assert_eq!(link.look, LinkLook::Chain);
        assert_eq!(link.from_anchor, [0.0, 10.0], "the holder's bottom edge");
        assert_eq!(link.to_anchor, [0.0, -10.0], "the tied one's top edge");
        assert!((link.length - 80.0).abs() < 0.01);
        assert_eq!(physics.body(1, 7).unwrap().kind, BodyKind::Animated);
        assert_eq!(physics.body(1, 8).unwrap().kind, BodyKind::Dynamic);

        // The window draws the link in both modes.
        app.settings.physics_enabled = true;
        app.session.physics.open = true;
        app.session.physics.selected_link = Some(link_id);
        for mode in RigMode::ALL {
            app.session.physics.mode = mode;
            render(&mut app);
        }

        // It simulates: the chain holds the falling square below the holder.
        let simulation = crate::physics::simulate(&app.state.project, 1).unwrap();
        let hang = simulation.tracks[1].poses.last().unwrap().center[1];
        assert!(hang < 145.0, "held up by the chain: {hang}");
    }

    #[test]
    fn one_object_hangs_from_the_stage_and_a_hinge_pins_its_top() {
        let mut app = app_with_two_squares();
        select(&mut app, &[2]);
        let rope = tie_selected(&mut app, Preset::Rope).unwrap();
        let link = app.state.project.runtime.physics.link(1, rope).unwrap();
        assert_eq!(link.from, 0, "tied to the stage");
        assert_eq!(link.from_anchor, [200.0, 50.0], "80 px above its top");
        let hinge = tie_selected(&mut app, Preset::Hinge).unwrap();
        let link = app.state.project.runtime.physics.link(1, hinge).unwrap();
        assert_eq!(link.kind, LinkKind::Hinge);
        assert_eq!(link.to_anchor, [0.0, -10.0]);
        select(&mut app, &[]);
        assert!(tie_selected(&mut app, Preset::Rope).is_err());
    }
}
