//! The tutorial: q0bo opens a practice project and plays through the basics
//! in it with the user. He draws, moves and animates his own ball with his
//! own mouse pointer, then waits while the user does the same with theirs.

use egui::{pos2, vec2, Color32, Context, Painter, Pos2, Rect, Shape, Stroke};
use q0s_format::geom::{CapShape, JoinShape};
use q0s_format::v2::{
    Asset, Layer, Path as VPath, Placement, ProjectV2, Rgba, Stroke as VStroke, Target,
    Transform2D, Tween, Vec2 as StageVec, VectorAsset,
};

use super::face::{Mood, INK, RED};
use super::{fx, Spot};
use crate::app::{Action, EditorApp};
use crate::hotkeys::ShortcutCommand;
use crate::state::{Selection, TimelineSelection, Tool};
use crate::workspace::Panel;

/// q0bo's layer and the user's, in the practice project.
pub(crate) const MINE: u16 = 1;
pub(crate) const YOURS: u16 = 2;
const LAST_FRAME: u16 = 23;
/// Half the size of the ball he draws, in stage units.
const BALL_RADIUS: f32 = 38.0;

/// Something q0bo does with his pointer.
#[derive(Clone, Copy, Debug)]
enum Act {
    /// Click a tool button.
    Tool(Tool),
    /// Drag out his ball with the oval tool, corner to corner, in stage units.
    Ball([f32; 2], [f32; 2]),
    /// Grab his ball on the keyframe at a frame and drag its middle to a point.
    Drag(u16, [f32; 2]),
    /// Click his layer's row in the timeline at a frame.
    Cell(u16),
    /// Press a key.
    Press(Press),
    /// Bring the pointer home and put it away.
    Rest,
}

#[derive(Clone, Copy, Debug)]
enum Press {
    Keyframe,
    Tween,
    Play,
}

/// What the user is asked to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Goal {
    Draw,
    Move,
    /// Stop the playback and start it again.
    Replay,
}

enum Then {
    /// Talk, maybe pointing at something, until the user says next.
    Next(Option<Spot>),
    /// Do these, then say this in this mood.
    Show(&'static [Act], Mood, &'static str),
    /// Wait for the user to do it, pointing at something, then cheer.
    Try(Goal, Option<Spot>, &'static str),
    /// The goodbye, with a way into the ask tab.
    End,
}

struct Step {
    mood: Mood,
    say: &'static str,
    then: Then,
}

const STEPS: &[Step] = &[
    Step {
        mood: Mood::Excited,
        say: "yay!! i made us a practice project called \"tutorial\". mess it up all u want, it's 4 learning lol\n\ni even made us 2 layers: 1 4 me, 1 4 u. so we dont bump elbows",
        then: Then::Next(None),
    },
    Step {
        mood: Mood::Talk,
        say: "this white paper is ur stage. whatever's on it = what ppl see in ur animation.\n\nstuff off teh paper is hidden. like a ninja",
        then: Then::Next(Some(Spot::Paper)),
    },
    Step {
        mood: Mood::Focus,
        say: "watch me draw a ball. *cracks knuckles*",
        then: Then::Show(
            &[
                Act::Tool(Tool::Oval),
                Act::Ball([112.0, 262.0], [188.0, 338.0]),
                Act::Rest,
            ],
            Mood::Smug,
            "ta-daaa!! a ball. 10/10 art, i know",
        ),
    },
    Step {
        mood: Mood::Excited,
        say: "ur turn!! click teh rectangle tool «(or press {rect})», then drag on teh paper 2 make a box",
        then: Then::Try(
            Goal::Draw,
            Some(Spot::Tool(Tool::Rectangle)),
            "w00t!! nice box!! ur a natural fr",
        ),
    },
    Step {
        mood: Mood::Talk,
        say: "now moving stuff, w/ teh arrow tool «({select})»:\n1. drag a box around a thing 2 select ALL of it\n2. drag it\nlike dis:",
        then: Then::Show(
            &[
                Act::Tool(Tool::Select),
                Act::Drag(0, [130.0, 150.0]),
                Act::Rest,
            ],
            Mood::Cool,
            "ez. drag = move. that's literally it B)",
        ),
    },
    Step {
        mood: Mood::Happy,
        say: "ur turn: drag a box around ur box 2 select it, then drag it somewhere else",
        then: Then::Try(
            Goal::Move,
            Some(Spot::Tool(Tool::Select)),
            "yesss. look at u go",
        ),
    },
    Step {
        mood: Mood::Talk,
        say: "ok now teh fun part: ANIMATION!!\n\ndown there is teh timeline. each lil box = 1 frame. frames play super fast 1 after another, like a flipbook",
        then: Then::Next(Some(Spot::Panel(Panel::Timeline))),
    },
    Step {
        mood: Mood::Focus,
        say: "a keyframe = a frame where u decide where stuff is. lemme make 1 on teh last frame «({key})»",
        then: Then::Show(
            &[
                Act::Cell(LAST_FRAME),
                Act::Press(Press::Keyframe),
                Act::Rest,
            ],
            Mood::Happy,
            "see teh new dot in my row? that's my keyframe",
        ),
    },
    Step {
        mood: Mood::Focus,
        say: "on that keyframe i move my ball 2 teh other side...",
        then: Then::Show(
            &[Act::Drag(LAST_FRAME, [500.0, 150.0]), Act::Rest],
            Mood::Talk,
            "so now: ball on teh left at teh start, ball on teh right at teh end",
        ),
    },
    Step {
        mood: Mood::Focus,
        say: "a tween fills in teh frames between 2 keyframes. right-click a frame between them > create motion tween «({tween})»",
        then: Then::Show(
            &[Act::Cell(10), Act::Press(Press::Tween), Act::Rest],
            Mood::Happy,
            "teh frames between got filled in. now watch...",
        ),
    },
    Step {
        mood: Mood::Excited,
        say: "playing it! «({play})»",
        then: Then::Show(
            &[Act::Press(Press::Play), Act::Rest],
            Mood::Excited,
            "WHEEEE!! it moves!! it's ALIVE",
        ),
    },
    Step {
        mood: Mood::Happy,
        say: "ur turn!! stop it, then play it again «({play})». or use teh play button in teh timeline",
        then: Then::Try(
            Goal::Replay,
            Some(Spot::Panel(Panel::Timeline)),
            "*happy beeps* that's an animation and U played it",
        ),
    },
    Step {
        mood: Mood::Wink,
        say: "pro tip: click any frame in teh timeline 2 jump there n see that moment",
        then: Then::Next(Some(Spot::Panel(Panel::Timeline))),
    },
    Step {
        mood: Mood::Cool,
        say: "last thing: ur stuff isn't saved yet!!\n\nFile > Save «({save})» saves it. File > Export «({export})» makes a video or a gif 4 ur friends",
        then: Then::Next(None),
    },
    Step {
        mood: Mood::Love,
        say: "and that's teh basics!! u r now officially 1337 B)\n\nwanna learn more? layers, symbols, bones, 3d, scripts... ask me anything in teh \"ask q0bo\" tab. i'll be chillin there",
        then: Then::End,
    },
];

/// What he says when the user takes their time.
const SNOOZE: &str = "zzz... oh! ur still there? take ur time lol. or hit skip if ur stuck";

/// How the tutorial left off this frame.
pub(crate) enum Outcome {
    Going,
    Finished { ask: bool },
    Skipped,
}

/// One act being played out.
#[derive(Clone, Debug)]
struct Run {
    act: usize,
    started: f64,
    /// Where the pointer was when the act began.
    from: Pos2,
    pressed: bool,
    released: bool,
    /// The corner a selection box was dragged from, in stage units.
    marquee: Option<[f32; 2]>,
    /// Where the ball's middle was when he grabbed it.
    grabbed: Option<[f32; 2]>,
}

impl Run {
    fn new(started: f64, from: Pos2) -> Self {
        Self {
            act: 0,
            started,
            from,
            pressed: false,
            released: false,
            marquee: None,
            grabbed: None,
        }
    }
}

/// The project as it was when the user was asked to do something.
#[derive(Clone, Debug)]
struct Base {
    assets: usize,
    placements: usize,
    transforms: Vec<(u16, usize, Transform2D)>,
    shapes: Vec<(u16, Vec<VPath>)>,
    stopped: bool,
}

impl Base {
    fn of(app: &EditorApp) -> Self {
        let project = &app.state.project;
        let mut transforms = Vec::new();
        for layer in stage_layers(project) {
            for (index, placement) in layer.placements.iter().enumerate() {
                transforms.push((layer.layer_id, index, placement.transform));
            }
        }
        Self {
            assets: project.assets.len(),
            placements: transforms.len(),
            transforms,
            shapes: project
                .assets
                .iter()
                .filter_map(|asset| match asset {
                    Asset::Vector(vector) => Some((vector.asset_id, vector.paths.clone())),
                    _ => None,
                })
                .collect(),
            stopped: !app.session.playing,
        }
    }

    fn met(&mut self, goal: Goal, app: &EditorApp) -> bool {
        let project = &app.state.project;
        match goal {
            Goal::Draw => {
                project.assets.len() > self.assets
                    || stage_layers(project)
                        .map(|layer| layer.placements.len())
                        .sum::<usize>()
                        > self.placements
            }
            Goal::Move => {
                let moved = self.transforms.iter().any(|(layer_id, index, before)| {
                    stage_layers(project)
                        .find(|layer| layer.layer_id == *layer_id)
                        .and_then(|layer| layer.placements.get(*index))
                        .is_some_and(|placement| placement.transform != *before)
                });
                let reshaped = self.shapes.iter().any(|(asset_id, paths)| {
                    project.assets.iter().any(|asset| match asset {
                        Asset::Vector(vector) => {
                            vector.asset_id == *asset_id && vector.paths != *paths
                        }
                        _ => false,
                    })
                });
                moved || reshaped
            }
            Goal::Replay => {
                if !app.session.playing {
                    self.stopped = true;
                }
                self.stopped && app.session.playing
            }
        }
    }
}

fn stage_layers(project: &ProjectV2) -> impl Iterator<Item = &Layer> {
    project
        .q0rgs
        .iter()
        .filter(move |q0rg| q0rg.q0rg_id == project.meta.entry_q0rg_id)
        .flat_map(|q0rg| q0rg.layers.iter())
}

/// The practice project: the default stage with a layer for each of them.
pub(crate) fn project() -> ProjectV2 {
    let mut project = crate::state::default_project();
    project.meta.name = "tutorial".to_string();
    let stage = &mut project.q0rgs[0];
    stage.layers[0].layer_id = MINE;
    stage.layers[0].name = "q0bo's stuff".to_string();
    stage.layers.push(Layer {
        layer_id: YOURS,
        timeline_frame_count: None,
        name: "ur stuff".to_string(),
        explicit_keyframes: vec![0],
        placements: Vec::new(),
    });
    project
}

pub(crate) struct Tutorial {
    pub(crate) tab: u64,
    step: usize,
    /// The step has moved on to its second line: the show is over or the
    /// user did it.
    after: bool,
    said_at: f64,
    run: Option<Run>,
    base: Option<Base>,
    ball: Option<u16>,
    pub(crate) hand: fx::Hand,
    /// A key on screen and when it went down.
    key: Option<(String, f64)>,
    /// The user wandered off to another tab or Home.
    away: bool,
}

/// What the tutorial wants q0bo to look like and say this frame.
pub(crate) struct Scene {
    pub mood: Mood,
    pub line: String,
    pub typed: f32,
    pub buttons: Vec<&'static str>,
    /// The first button is the thing to press, not a way out.
    pub primary: bool,
    pub corner: Option<&'static str>,
    pub point: Option<Spot>,
    /// The user did it, or the animation came alive: confetti.
    pub cheer: bool,
    /// He finished showing something: a proud hop.
    pub hop: bool,
}

impl Tutorial {
    pub(crate) fn new(tab: u64, now: f64) -> Self {
        Self {
            tab,
            step: 0,
            after: false,
            said_at: now,
            run: None,
            base: None,
            ball: None,
            hand: fx::Hand::default(),
            key: None,
            away: false,
        }
    }

    pub(crate) fn hurry(&mut self, now: f64) {
        self.said_at = now - 1000.0;
    }

    /// Whether his pointer is out doing something.
    pub(crate) fn busy(&self) -> bool {
        self.run.is_some()
    }

    /// Plays this frame of the tutorial and says what to show. `home` is
    /// where his own hand is, the pointer's resting place.
    pub(crate) fn frame(
        &mut self,
        app: &mut EditorApp,
        ctx: &Context,
        now: f64,
        home: Pos2,
    ) -> Scene {
        let here = app.active_project_tab_id() == Some(self.tab) && !app.home_visible();
        if !here {
            self.away = true;
            self.hand.shown = (self.hand.shown - 0.1).max(0.0);
            return Scene {
                mood: Mood::Thinking,
                line: "psst!! ur tutorial is in teh \"tutorial\" tab. wanna go back?".into(),
                typed: 999.0,
                buttons: vec!["take me back", "end tutorial"],
                primary: true,
                corner: None,
                point: None,
                cheer: false,
                hop: false,
            };
        }
        if self.away {
            self.away = false;
            self.said_at = now - 1000.0;
        }
        let step = &STEPS[self.step];
        let mut cheer = false;
        let mut hop = false;
        match &step.then {
            Then::Show(acts, ..) if !self.after => {
                if self.run.is_none() {
                    // Give the line a moment before the pointer sets off.
                    self.run = Some(Run::new(now + 0.9, home));
                    self.hand.pos = home;
                }
                if self.play(app, ctx, acts, now, home) {
                    self.run = None;
                    self.after = true;
                    self.said_at = now;
                    hop = true;
                    cheer = matches!(step.then, Then::Show(_, Mood::Excited, _));
                }
            }
            Then::Try(goal, ..) if !self.after => {
                let base = self.base.get_or_insert_with(|| Base::of(app));
                if base.met(*goal, app) {
                    self.after = true;
                    self.said_at = now;
                    cheer = true;
                }
            }
            _ => {}
        }
        if self.run.is_none() {
            self.hand.shown = (self.hand.shown - 0.08).max(0.0);
        }

        let touch = crate::touch::ENABLED;
        let (mood, line, buttons, point) = match &step.then {
            Then::Next(point) => (step.mood, step.say, vec!["next >>"], *point),
            Then::Show(_, mood, after) if self.after => (*mood, *after, vec!["next >>"], None),
            Then::Show(..) => (step.mood, step.say, Vec::new(), None),
            Then::Try(_, _, cheer) if self.after => (Mood::Excited, *cheer, vec!["next >>"], None),
            // A long wait on the user sends him to sleep, still pointing.
            Then::Try(_, point, _) if now - self.said_at > 40.0 => {
                (Mood::Sleepy, SNOOZE, vec!["skip this 1"], *point)
            }
            Then::Try(_, point, _) => (step.mood, step.say, vec!["skip this 1"], *point),
            Then::End => (step.mood, step.say, vec!["open ask q0bo", "kthxbai"], None),
        };
        Scene {
            mood,
            line: super::bubble::fill(line, &app.settings.hotkeys, touch),
            typed: ((now - self.said_at) as f32) * super::bubble::TYPING,
            primary: buttons.first() != Some(&"skip this 1"),
            buttons,
            corner: Some("[x] skip tutorial"),
            point,
            cheer,
            hop,
        }
    }

    /// Acts on a button in his bubble.
    pub(crate) fn press(
        &mut self,
        app: &mut EditorApp,
        button: usize,
        now: f64,
    ) -> Outcome {
        if self.away {
            return if button == 0 {
                app.switch_to_document(self.tab);
                Outcome::Going
            } else {
                Outcome::Skipped
            };
        }
        if matches!(STEPS[self.step].then, Then::End) {
            return Outcome::Finished { ask: button == 0 };
        }
        if self.step + 1 >= STEPS.len() {
            return Outcome::Finished { ask: false };
        }
        self.step += 1;
        self.after = false;
        self.said_at = now;
        self.run = None;
        self.base = None;
        self.key = None;
        match STEPS[self.step].then {
            Then::Try(Goal::Draw | Goal::Move, ..) | Then::Show(..) => {
                stop(app);
                if let Then::Try(..) = STEPS[self.step].then {
                    // Their drawing goes on their own layer, filled so it can
                    // be grabbed anywhere inside.
                    if app.session.fill_color.is_none() {
                        app.session.fill_color = Some(Rgba {
                            r: 0xFF,
                            g: 0xC8,
                            b: 0x3D,
                            a: 255,
                        });
                    }
                    app.session.current_layer_id = YOURS;
                    app.session.current_frame = 0;
                    app.session.timeline_selection = None;
                    app.session.selection = Selection::None;
                }
            }
            _ => {}
        }
        Outcome::Going
    }

    /// Whether the tutorial's tab is still open.
    pub(crate) fn alive(&self, app: &EditorApp) -> bool {
        app.document_tab_exists(self.tab)
    }

    /// Draws his pointer, any key he pressed and the shapes he is dragging out.
    pub(crate) fn paint(&self, app: &EditorApp, ctx: &Context, painter: &Painter, now: f64) {
        if let Some(run) = &self.run {
            if let Then::Show(acts, ..) = &STEPS[self.step].then {
                if let Some(Act::Ball(a, b)) = acts.get(run.act) {
                    let t = (now - run.started) as f32;
                    if run.pressed && !run.released {
                        if let (Some(a), Some(b)) = (
                            stage_point(app, ctx, *a),
                            stage_point(app, ctx, lerp(*a, *b, fx::smooth((t - 0.6) / 0.9))),
                        ) {
                            let rect = Rect::from_two_pos(a, b);
                            let clip = super::spot(ctx, Spot::Stage).unwrap_or(ctx.screen_rect());
                            let painter = painter.with_clip_rect(clip);
                            painter.add(Shape::ellipse_filled(
                                rect.center(),
                                rect.size() * 0.5,
                                RED.gamma_multiply(0.35),
                            ));
                            painter.add(Shape::ellipse_stroke(
                                rect.center(),
                                rect.size() * 0.5,
                                Stroke::new(1.5_f32, INK),
                            ));
                        }
                    }
                }
            }
        }
        if let Some(run) = &self.run {
            if let (Some(corner), false) = (run.marquee, run.pressed) {
                if let Some(a) = stage_point(app, ctx, corner) {
                    if self.hand.down {
                        let rect = Rect::from_two_pos(a, self.hand.pos);
                        let clip = super::spot(ctx, Spot::Stage).unwrap_or(ctx.screen_rect());
                        let painter = painter.with_clip_rect(clip);
                        painter.rect_filled(rect, 0.0, RED.gamma_multiply(0.08));
                        painter.extend(Shape::dashed_line(
                            &[
                                rect.left_top(),
                                rect.right_top(),
                                rect.right_bottom(),
                                rect.left_bottom(),
                                rect.left_top(),
                            ],
                            Stroke::new(1.0_f32, INK),
                            4.0,
                            3.0,
                        ));
                    }
                }
            }
        }
        if let Some((label, when)) = &self.key {
            let age = (now - when) as f32;
            let at = if self.hand.shown > 0.2 {
                self.hand.pos + vec2(0.0, -44.0)
            } else {
                self.hand.pos
            };
            fx::keycap(painter, at, label, age);
        }
        self.hand.paint(painter, now);
    }

    /// Plays the show's acts; true once they are all done.
    fn play(
        &mut self,
        app: &mut EditorApp,
        ctx: &Context,
        acts: &[Act],
        now: f64,
        home: Pos2,
    ) -> bool {
        let Some(mut run) = self.run.take() else {
            return true;
        };
        let done = loop {
            let Some(act) = acts.get(run.act).copied() else {
                break true;
            };
            let t = (now - run.started) as f32;
            if t < 0.0 {
                break false;
            }
            if !matches!(act, Act::Rest) {
                self.hand.shown = (self.hand.shown + 0.12).min(1.0);
            }
            if !self.act(app, ctx, act, &mut run, t, now, home) {
                break false;
            }
            // That one is over: the next starts now, from where the pointer is.
            let next = run.act + 1;
            run = Run::new(now, self.hand.pos);
            run.act = next;
        };
        self.run = Some(run);
        done
    }

    /// Plays one act at `t` seconds in; true once it is over.
    #[allow(clippy::too_many_arguments)]
    fn act(
        &mut self,
        app: &mut EditorApp,
        ctx: &Context,
        act: Act,
        run: &mut Run,
        t: f32,
        now: f64,
        home: Pos2,
    ) -> bool {
        const FLY: f32 = 0.6;
        match act {
            Act::Tool(tool) => {
                let Some(target) = super::spot(ctx, Spot::Tool(tool)).map(|rect| rect.center())
                else {
                    app.handle(ctx, Action::SelectTool(tool));
                    return true;
                };
                self.hand.pos = fx::swoop(run.from, target, t / FLY);
                if t >= FLY && !run.pressed {
                    run.pressed = true;
                    self.hand.press(now);
                    app.handle(ctx, Action::SelectTool(tool));
                }
                if t >= FLY + 0.2 {
                    self.hand.down = false;
                }
                t >= FLY + 0.45
            }
            Act::Ball(a, b) => {
                let Some(start) = stage_point(app, ctx, a) else {
                    self.ball = add_ball(app, a, b);
                    return true;
                };
                if t < FLY {
                    self.hand.pos = fx::swoop(run.from, start, t / FLY);
                    return false;
                }
                if !run.pressed {
                    run.pressed = true;
                    self.hand.pos = start;
                    self.hand.press(now);
                }
                let drag = ((t - FLY) / 0.9).min(1.0);
                if let Some(point) = stage_point(app, ctx, lerp(a, b, fx::smooth(drag))) {
                    self.hand.pos = point;
                }
                if drag >= 1.0 && !run.released {
                    run.released = true;
                    self.hand.down = false;
                    self.ball = add_ball(app, a, b);
                }
                t >= FLY + 1.2
            }
            Act::Drag(frame, to) => {
                // Like the user has to: a selection box around the ball
                // first, so all of it comes along, then the drag itself.
                const BOX: f32 = 0.6;
                const REACH: f32 = 0.35;
                const MOVE: f32 = 1.0;
                let Some(index) = ball_at(app, self.ball, frame) else {
                    return true;
                };
                let Some(centre) = ball_centre(app, index) else {
                    return true;
                };
                let margin = BALL_RADIUS + 16.0;
                let from_corner = [centre[0] - margin, centre[1] - margin];
                let to_corner = [centre[0] + margin, centre[1] + margin];
                if t < FLY {
                    if let Some(target) = stage_point(app, ctx, from_corner) {
                        self.hand.pos = fx::swoop(run.from, target, t / FLY);
                    }
                    return false;
                }
                let entry = app.state.project.meta.entry_q0rg_id;
                if t < FLY + BOX {
                    if run.marquee.is_none() {
                        run.marquee = Some(from_corner);
                        self.hand.press(now);
                        app.session.current_layer_id = MINE;
                        app.session.current_frame = frame;
                        app.session.timeline_selection = None;
                        app.session.selection = Selection::None;
                    }
                    let corner = lerp(from_corner, to_corner, fx::smooth((t - FLY) / BOX));
                    if let Some(point) = stage_point(app, ctx, corner) {
                        self.hand.pos = point;
                    }
                    return false;
                }
                if run.marquee.is_some() && !run.pressed && self.hand.down {
                    self.hand.down = false;
                    app.session.selection = Selection::Placement {
                        q0rg_id: entry,
                        layer_id: MINE,
                        placement_idx: index,
                    };
                }
                if t < FLY + BOX + REACH {
                    if let (Some(a), Some(b)) =
                        (stage_point(app, ctx, to_corner), stage_point(app, ctx, centre))
                    {
                        self.hand.pos = fx::swoop(a, b, (t - FLY - BOX) / REACH);
                    }
                    return false;
                }
                if !run.pressed {
                    run.pressed = true;
                    self.hand.press(now);
                    app.history
                        .snapshot_with_session(&app.state.project, &app.session);
                    app.session.selection = Selection::Placement {
                        q0rg_id: entry,
                        layer_id: MINE,
                        placement_idx: index,
                    };
                    run.grabbed = Some(centre);
                }
                let drag = ((t - FLY - BOX - REACH) / MOVE).min(1.0);
                let at = lerp(run.grabbed.unwrap_or(centre), to, fx::smooth(drag));
                if !run.released {
                    if let Some(transform) = ball_transform(app, index) {
                        transform.tx = at[0];
                        transform.ty = at[1];
                    }
                    app.mark_placement_transforms_changed(&[(entry, MINE, index)]);
                    if drag >= 1.0 {
                        run.released = true;
                        self.hand.down = false;
                        app.state.mark_dirty();
                    }
                }
                if let Some(point) = stage_point(app, ctx, at) {
                    self.hand.pos = point;
                }
                t >= FLY + BOX + REACH + MOVE + 0.3
            }
            Act::Cell(frame) => {
                let Some(target) = cell_point(ctx, frame) else {
                    pick_cell(app, frame);
                    return true;
                };
                self.hand.pos = fx::swoop(run.from, target, t / FLY);
                if t >= FLY && !run.pressed {
                    run.pressed = true;
                    self.hand.press(now);
                    pick_cell(app, frame);
                }
                if t >= FLY + 0.2 {
                    self.hand.down = false;
                }
                t >= FLY + 0.4
            }
            Act::Press(press) => {
                if !run.pressed {
                    run.pressed = true;
                    let (command, name) = match press {
                        Press::Keyframe => (ShortcutCommand::InsertKeyframe, "keyframe"),
                        Press::Tween => (ShortcutCommand::MotionTween, "motion tween"),
                        Press::Play => (ShortcutCommand::PlaybackToggle, "play"),
                    };
                    let label = if crate::touch::ENABLED {
                        name.to_string()
                    } else {
                        super::bubble::key(&app.settings.hotkeys, command)
                    };
                    self.key = Some((label, now));
                }
                if t >= 0.15 && !run.released {
                    run.released = true;
                    match press {
                        Press::Keyframe => app.handle(ctx, Action::InsertKeyframe),
                        Press::Tween => app.handle(ctx, Action::CreateMotionTween),
                        Press::Play => {
                            app.session.selection = Selection::None;
                            app.session.current_frame = 0;
                            if !app.session.playing {
                                app.handle(ctx, Action::TogglePlay);
                            }
                        }
                    }
                }
                t >= 1.1
            }
            Act::Rest => {
                self.hand.pos = fx::swoop(run.from, home, t / 0.5);
                self.hand.shown = (1.0 - t / 0.5).clamp(0.0, 1.0);
                if t >= 0.5 {
                    self.hand.pos = home;
                    app.session.selection = Selection::None;
                }
                t >= 0.5
            }
        }
    }
}

fn ball_centre(app: &EditorApp, index: usize) -> Option<[f32; 2]> {
    stage_layers(&app.state.project)
        .find(|layer| layer.layer_id == MINE)?
        .placements
        .get(index)
        .map(|placement| [placement.transform.tx, placement.transform.ty])
}

fn lerp(a: [f32; 2], b: [f32; 2], t: f32) -> [f32; 2] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]
}

/// Where a stage point is on screen, while the stage is in sight.
fn stage_point(app: &EditorApp, ctx: &Context, point: [f32; 2]) -> Option<Pos2> {
    let paper = super::spot(ctx, Spot::Paper)?;
    let scale = paper.width() / f32::from(app.state.project.meta.stage_width.max(1));
    Some(paper.min + vec2(point[0], point[1]) * scale)
}

/// The middle of a frame's cell in q0bo's timeline row.
fn cell_point(ctx: &Context, frame: u16) -> Option<Pos2> {
    let first = super::spot(ctx, Spot::TimelineRow(MINE))?;
    let centre = first.center() + vec2(first.width() * f32::from(frame), 0.0);
    let panel = super::spot(ctx, Spot::Panel(Panel::Timeline)).unwrap_or(ctx.screen_rect());
    Some(pos2(
        super::within(centre.x, panel.left() + 8.0, panel.right() - 8.0),
        super::within(centre.y, panel.top() + 8.0, panel.bottom() - 8.0),
    ))
}

/// The ball's placement on the keyframe at `frame`, by index.
fn ball_at(app: &EditorApp, ball: Option<u16>, frame: u16) -> Option<usize> {
    let ball = ball?;
    stage_layers(&app.state.project)
        .find(|layer| layer.layer_id == MINE)?
        .placements
        .iter()
        .position(|placement| placement.frame == frame && placement.target == Target::Asset(ball))
}

fn ball_transform(app: &mut EditorApp, index: usize) -> Option<&mut Transform2D> {
    let entry = app.state.project.meta.entry_q0rg_id;
    app.state
        .project
        .q0rgs
        .iter_mut()
        .find(|q0rg| q0rg.q0rg_id == entry)?
        .layers
        .iter_mut()
        .find(|layer| layer.layer_id == MINE)?
        .placements
        .get_mut(index)
        .map(|placement| &mut placement.transform)
}

/// Draws his ball into the project: a red oval with an ink outline, centred
/// on its placement so moving it is moving the placement.
fn add_ball(app: &mut EditorApp, a: [f32; 2], b: [f32; 2]) -> Option<u16> {
    let radius = StageVec::new((b[0] - a[0]).abs() * 0.5, (b[1] - a[1]).abs() * 0.5);
    let path = crate::tools::oval_path(StageVec::new(-radius.x, -radius.y), radius)?;
    let entry = app.state.project.meta.entry_q0rg_id;
    let has_layer = stage_layers(&app.state.project).any(|layer| layer.layer_id == MINE);
    if !has_layer {
        return None;
    }
    app.history
        .snapshot_with_session(&app.state.project, &app.session);
    let asset_id = crate::tools::next_asset_id_pub(&app.state.project);
    let colour = |c: Color32| Rgba {
        r: c.r(),
        g: c.g(),
        b: c.b(),
        a: 255,
    };
    app.state.project.assets.push(Asset::Vector(VectorAsset {
        asset_id,
        paths: vec![path],
        fill: Some(colour(RED)),
        stroke: Some(VStroke {
            color: colour(INK),
            width: 3.0,
            cap: CapShape::Round,
            join: JoinShape::Round,
        }),
    }));
    let layer = app
        .state
        .project
        .q0rgs
        .iter_mut()
        .find(|q0rg| q0rg.q0rg_id == entry)?
        .layers
        .iter_mut()
        .find(|layer| layer.layer_id == MINE)?;
    layer.push_placement(Placement {
        pivot: None,
        instance_id: 0,
        frame: 0,
        target: Target::Asset(asset_id),
        transform: Transform2D {
            tx: (a[0] + b[0]) * 0.5,
            ty: (a[1] + b[1]) * 0.5,
            ..Transform2D::IDENTITY
        },
        tween: Tween::None,
        fx: Default::default(),
    });
    app.state.mark_dirty();
    app.session.status = "q0bo drew a ball".to_string();
    Some(asset_id)
}

/// Clicks a frame in his own row, the way a user picks one.
fn pick_cell(app: &mut EditorApp, frame: u16) {
    app.session.playing = false;
    app.session.current_layer_id = MINE;
    app.session.current_frame = frame;
    app.session.pending_timeline_frame = None;
    app.session.selection = Selection::None;
    app.session.timeline_cell_selection = None;
    app.session.timeline_selection = Some(TimelineSelection::single(MINE, frame));
}

fn stop(app: &mut EditorApp) {
    app.session.playing = false;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layer(app: &EditorApp, layer_id: u16) -> &Layer {
        stage_layers(&app.state.project)
            .find(|layer| layer.layer_id == layer_id)
            .expect("layer")
    }

    /// Plays frames until the step offers a button to press.
    fn settle(tutorial: &mut Tutorial, app: &mut EditorApp, ctx: &Context, now: &mut f64) -> Scene {
        for _ in 0..2000 {
            *now += 1.0 / 30.0;
            let scene = tutorial.frame(app, ctx, *now, Pos2::ZERO);
            if !scene.buttons.is_empty() && super::super::bubble::finished(&scene.line, scene.typed)
            {
                return scene;
            }
        }
        panic!("step {} never settled", tutorial.step);
    }

    #[test]
    fn the_tutorial_draws_keys_and_tweens_a_real_ball() {
        let ctx = Context::default();
        let mut app = EditorApp::default();
        let tab = app.open_new_document(project()).expect("tutorial tab");
        let mut tutorial = Tutorial::new(tab, 0.0);
        let mut now = 0.0;
        let mut finished = false;
        for _ in 0..STEPS.len() * 2 {
            settle(&mut tutorial, &mut app, &ctx, &mut now);
            if let Then::Try(goal, ..) = STEPS[tutorial.step].then {
                assert!(!tutorial.after, "step {} was done before the user did it", tutorial.step);
                match goal {
                    Goal::Draw => {
                        assert_eq!(app.session.current_layer_id, YOURS);
                        let path = crate::tools::oval_path(
                            StageVec::new(300.0, 100.0),
                            StageVec::new(380.0, 160.0),
                        )
                        .expect("path");
                        let id = crate::tools::next_asset_id_pub(&app.state.project);
                        app.state.project.assets.push(Asset::Vector(VectorAsset {
                            asset_id: id,
                            paths: vec![path],
                            fill: None,
                            stroke: None,
                        }));
                        app.state.project.q0rgs[0].layers[1].push_placement(Placement {
                            pivot: None,
                            instance_id: 0,
                            frame: 0,
                            target: Target::Asset(id),
                            transform: Transform2D::IDENTITY,
                            tween: Tween::None,
                            fx: Default::default(),
                        });
                    }
                    Goal::Move => app.state.project.q0rgs[0].layers[1].placements[0].transform.tx += 40.0,
                    Goal::Replay => {
                        app.session.playing = false;
                        tutorial.frame(&mut app, &ctx, now, Pos2::ZERO);
                        app.session.playing = true;
                    }
                }
                let scene = settle(&mut tutorial, &mut app, &ctx, &mut now);
                assert!(tutorial.after, "step {} missed the user doing it", tutorial.step);
                assert_eq!(scene.mood, Mood::Excited);
            }
            match tutorial.press(&mut app, 0, now) {
                Outcome::Going => {}
                Outcome::Finished { ask } => {
                    assert!(ask);
                    finished = true;
                    break;
                }
                Outcome::Skipped => panic!("skipped"),
            }
        }
        assert!(finished, "the tutorial never reached its end");

        let ball = tutorial.ball.expect("q0bo drew his ball");
        let mine = layer(&app, MINE);
        let start = mine
            .placements
            .iter()
            .find(|placement| placement.frame == 0 && placement.target == Target::Asset(ball))
            .expect("ball on frame 0");
        let end = mine
            .placements
            .iter()
            .find(|placement| placement.frame == LAST_FRAME && placement.target == Target::Asset(ball))
            .expect("ball keyed on the last frame");
        assert_eq!(start.tween, Tween::Linear { to_frame: LAST_FRAME });
        assert_eq!((start.transform.tx, start.transform.ty), (130.0, 150.0));
        assert_eq!((end.transform.tx, end.transform.ty), (500.0, 150.0));
        assert_eq!(layer(&app, YOURS).placements.len(), 1);
        assert!(app.history.can_undo());
    }

    #[test]
    fn leaving_the_tab_pauses_and_offers_the_way_back() {
        let ctx = Context::default();
        let mut app = EditorApp::default();
        let tab = app.open_new_document(project()).expect("tutorial tab");
        let mut tutorial = Tutorial::new(tab, 0.0);
        app.open_new_document(crate::state::default_project())
            .expect("another tab");
        let scene = tutorial.frame(&mut app, &ctx, 1.0, Pos2::ZERO);
        assert_eq!(scene.buttons, vec!["take me back", "end tutorial"]);
        assert!(matches!(tutorial.press(&mut app, 0, 1.0), Outcome::Going));
        assert_eq!(app.active_project_tab_id(), Some(tab));
        let scene = tutorial.frame(&mut app, &ctx, 1.1, Pos2::ZERO);
        assert_eq!(scene.buttons, vec!["next >>"]);
        assert_eq!(tutorial.step, 0);
    }
}
