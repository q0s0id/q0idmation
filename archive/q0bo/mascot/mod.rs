//! q0bo, the editor's mascot. He says hi on the very first start, plays
//! through a tutorial project with the user (drawing and moving real things
//! in it with his own pointer), and answers questions in the "ask q0bo" tab.

mod ask;
mod bubble;
mod face;
mod fx;
mod tutorial;

use std::collections::HashMap;

use egui::{pos2, vec2, Color32, Context, CursorIcon, Id, LayerId, Order, Pos2, Rect, Sense, Vec2};
use serde::{Deserialize, Serialize};

use crate::app::EditorApp;
use crate::state::Tool;
use crate::workspace::Panel;
use face::{Arm, Mood, Pose};

pub(crate) use ask::panel as ask_panel;

/// What q0bo remembers between runs.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct MascotSettings {
    /// He has said hello. He does that once, on the very first start.
    pub greeted: bool,
    /// The tutorial was played to its end.
    pub tutorial_done: bool,
}

/// Places q0bo can point at, noted by the panels as they draw themselves.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Spot {
    /// The whole canvas around the stage.
    Stage,
    /// The stage's paper.
    Paper,
    Panel(Panel),
    Tool(Tool),
    /// The cell of frame 0 in a layer's timeline row.
    TimelineRow(u16),
}

#[derive(Clone, Default)]
struct Spots(HashMap<Spot, (u64, Rect)>);

fn spots_id() -> Id {
    Id::new("q0bo_spots")
}

/// Notes where something is on screen this frame.
pub(crate) fn mark(ctx: &Context, spot: Spot, rect: Rect) {
    let frame = ctx.frame_nr();
    ctx.data_mut(|data| {
        data.get_temp_mut_or_default::<Spots>(spots_id())
            .0
            .insert(spot, (frame, rect));
    });
}

/// Where something was drawn this frame or the last one.
pub(crate) fn spot(ctx: &Context, spot: Spot) -> Option<Rect> {
    let frame = ctx.frame_nr();
    ctx.data(|data| {
        data.get_temp::<Spots>(spots_id())
            .and_then(|spots| spots.0.get(&spot).copied())
    })
    .filter(|(seen, _)| seen + 1 >= frame)
    .map(|(_, rect)| rect)
}

#[derive(Default)]
pub(crate) struct Mascot {
    scene: Option<Scene>,
    pub(crate) ask: ask::Ask,
    buddy: Buddy,
    party: fx::Party,
    /// The time of the latest frame, for things started between frames.
    now: f64,
    /// Something on him is moving fast this frame: typing, a show, a hop.
    lively: bool,
}

enum Scene {
    Hello,
    Tutorial(Box<tutorial::Tutorial>),
    Bye(&'static str, Mood),
    /// Popping out of sight since this time.
    Leaving(f64),
}

/// How he moves around on screen.
#[derive(Default)]
struct Buddy {
    /// When he popped up.
    arrived: f64,
    /// When his current line started, outside the tutorial.
    said_at: f64,
    /// Where he has been dragged to, away from his usual spot.
    offset: Vec2,
    hopped: f64,
    poked: f64,
    pokes: usize,
}

impl Buddy {
    fn arrive(&mut self, now: f64) {
        self.arrived = now;
        self.said_at = now + 0.35;
    }

    /// His size while popping in: overshoots a little, then settles.
    fn scale(&self, now: f64) -> f32 {
        let k = (((now - self.arrived) / 0.4) as f32).clamp(0.0, 1.0) - 1.0;
        1.0 + 2.7 * k * k * k + 1.7 * k * k
    }

    fn hop(&self, now: f64) -> f32 {
        let cheer = ((now - self.hopped) / 0.9) as f32;
        let poke = ((now - self.poked) / 0.4) as f32;
        let bounce = |k: f32, height: f32| {
            if (0.0..1.0).contains(&k) {
                (k * std::f32::consts::PI * 2.0).sin().abs() * height * (1.0 - k)
            } else {
                0.0
            }
        };
        bounce(cheer, 1.0).max(bounce(poke, 0.6))
    }

    /// A poke changes his face for a moment.
    fn mood(&self, now: f64, mood: Mood) -> Mood {
        if now - self.poked < 1.0 {
            if self.pokes >= 5 {
                Mood::Dizzy
            } else {
                Mood::Surprised
            }
        } else {
            mood
        }
    }
}

const HELLO: &str = "hiii!! i'm q0bo :3\nwelcome 2 q0editor!!\n\nwanna learn teh basics w/ me? i'll draw stuff with u. takes like 3 min";
const NO_THANKS: &str =
    "np, ur a pro i guess B)\n\nif u ever get stuck: Help > ask q0bo. cya o/";
const QUIT: &str = "aw ok :( ...jk np!!\n\nur practice project stays open. find me in Help > ask q0bo";
const DONE: &str = "kthxbai!! <3\n\nHelp > ask q0bo if u ever need me";
const GONE: &str = "u closed teh tutorial :( no worries!!\n\nHelp > ask q0bo if u need me";
const CHEERS: [&str; 5] = ["+1337 xp", "gg", "w00t", "lvl up!!", "pwned it"];

/// Runs q0bo for this frame: the greeting, the tutorial or his goodbye.
pub(crate) fn update(app: &mut EditorApp, ctx: &Context) {
    let now = ctx.input(|input| input.time);
    app.mascot.now = now;
    ask::window(app, ctx);
    if app.model_studio.is_some() {
        return;
    }
    if app.mascot.scene.is_none() && !app.settings.mascot.greeted {
        app.mascot.scene = Some(Scene::Hello);
        app.mascot.buddy.arrive(now);
    }
    app.mascot.lively = false;
    if let Some(scene) = app.mascot.scene.take() {
        app.mascot.scene = run(app, ctx, scene, now);
    }
    let top = ctx.layer_painter(LayerId::new(Order::Tooltip, Id::new("q0bo_party")));
    app.mascot.party.paint(&top, now);
    let buddy = &app.mascot.buddy;
    let settling = now - buddy.arrived < 1.0 || now - buddy.hopped < 1.0 || now - buddy.poked < 1.2;
    if app.mascot.lively || settling || app.mascot.party.busy() {
        ctx.request_repaint();
    } else if app.mascot.scene.is_some() {
        // Standing around he only breathes and blinks: a few frames a second
        // is plenty, and leaves the editor idle otherwise.
        ctx.request_repaint_after(std::time::Duration::from_millis(120));
    }
}

/// Starts the tutorial in a new practice project.
pub(crate) fn start_tutorial(app: &mut EditorApp) {
    let now = app.mascot.now;
    app.mascot.scene = begin_tutorial(app, now);
}

/// Shows the "ask q0bo" tab, or a window with it where nothing docks.
pub(crate) fn open_ask(app: &mut EditorApp) {
    app.mascot.ask.greet(app.mascot.now);
    if crate::touch::ENABLED || app.home_visible() || !app.has_active_project() {
        app.mascot.ask.window_open = true;
    } else {
        app.workspace_layout.show(Panel::Helper);
    }
}

fn begin_tutorial(app: &mut EditorApp, now: f64) -> Option<Scene> {
    let Some(tab) = app.open_new_document(tutorial::project()) else {
        app.session.status = "q0bo could not open the practice project".to_string();
        return None;
    };
    app.session.current_layer_id = tutorial::YOURS;
    app.session.status = "tutorial with q0bo".to_string();
    if !matches!(app.mascot.scene, Some(Scene::Hello | Scene::Tutorial(_))) {
        app.mascot.buddy.arrive(now);
    }
    app.mascot.buddy.offset = Vec2::ZERO;
    Some(Scene::Tutorial(Box::new(tutorial::Tutorial::new(tab, now))))
}

fn greeted(app: &mut EditorApp) {
    if !app.settings.mascot.greeted {
        app.settings.mascot.greeted = true;
        app.settings.save();
    }
}

fn run(app: &mut EditorApp, ctx: &Context, scene: Scene, now: f64) -> Option<Scene> {
    match scene {
        Scene::Hello => {
            let screen = ctx.screen_rect();
            let narrow = screen.width() < 700.0;
            let height = if narrow { 150.0 } else { 190.0 };
            let feet = if app.home_visible() || !app.has_active_project() {
                let middle = if narrow {
                    pos2(screen.center().x, screen.bottom() - 30.0)
                } else {
                    pos2(screen.center().x + 200.0, screen.center().y + 170.0)
                };
                place(&app.mascot.buddy, ctx, middle)
            } else {
                standing(app, ctx, height)
            };
            // Dim the editor behind him: this is his moment.
            let fade = (((now - app.mascot.buddy.arrived) / 0.4) as f32).clamp(0.0, 1.0);
            ctx.layer_painter(LayerId::new(Order::Middle, Id::new("q0bo_dim")))
                .rect_filled(screen, 0.0, Color32::from_black_alpha((120.0 * fade) as u8));
            let typed = ((now - app.mascot.buddy.said_at) as f32) * bubble::TYPING;
            let typing = !bubble::finished(HELLO, typed);
            app.mascot.lively = true;
            let arm = if now - app.mascot.buddy.arrived < 2.6 {
                Arm::Wave
            } else {
                Arm::Rest
            };
            let rect = show(app, ctx, feet, height, Mood::Happy, typing, arm, None, now);
            let said = bubble::speech(
                ctx,
                Id::new("q0bo_bubble"),
                rect,
                HELLO,
                typed,
                &["yea let's go!!", "nah, skip"],
                true,
                None,
            );
            if said.hurry {
                app.mascot.buddy.said_at = now - 1000.0;
            }
            match said.clicked {
                Some(0) => {
                    greeted(app);
                    begin_tutorial(app, now).or(Some(Scene::Hello))
                }
                Some(_) => {
                    greeted(app);
                    Some(bye(app, NO_THANKS, Mood::Cool, now))
                }
                None => Some(Scene::Hello),
            }
        }
        Scene::Tutorial(tutorial) => run_tutorial(app, ctx, tutorial, now),
        Scene::Bye(line, mood) => {
            let height = 130.0;
            let feet = standing(app, ctx, height);
            let typed = ((now - app.mascot.buddy.said_at) as f32) * bubble::TYPING;
            let typing = !bubble::finished(line, typed);
            app.mascot.lively = true;
            let arm = if typing { Arm::Rest } else { Arm::Wave };
            let rect = show(app, ctx, feet, height, mood, typing, arm, None, now);
            let said =
                bubble::speech(ctx, Id::new("q0bo_bubble"), rect, line, typed, &["k"], true, None);
            if said.hurry {
                app.mascot.buddy.said_at = now - 1000.0;
            }
            if said.clicked.is_some() || now - app.mascot.buddy.said_at > 14.0 {
                Some(Scene::Leaving(now))
            } else {
                Some(Scene::Bye(line, mood))
            }
        }
        Scene::Leaving(since) => {
            app.mascot.lively = true;
            let k = ((now - since) / 0.3) as f32;
            if k >= 1.0 {
                return None;
            }
            let height = 130.0;
            let feet = standing(app, ctx, height);
            let painter = ctx.layer_painter(LayerId::new(Order::Foreground, Id::new("q0bo")));
            face::paint(
                &painter,
                feet,
                height,
                &Pose {
                    mood: Mood::Wink,
                    arm: Arm::Wave,
                    hop: k,
                    scale: 1.0 - k * k,
                    ..Pose::default()
                },
                now,
            );
            Some(Scene::Leaving(since))
        }
    }
}

fn bye(app: &mut EditorApp, line: &'static str, mood: Mood, now: f64) -> Scene {
    app.mascot.buddy.said_at = now;
    Scene::Bye(line, mood)
}

fn run_tutorial(
    app: &mut EditorApp,
    ctx: &Context,
    mut tutorial: Box<tutorial::Tutorial>,
    now: f64,
) -> Option<Scene> {
    if !tutorial.alive(app) {
        return Some(bye(app, GONE, Mood::Sad, now));
    }
    let height = 118.0;
    let feet = standing(app, ctx, height);
    let home = feet + vec2(-height * 0.42, -height * 0.4);
    let scene = tutorial.frame(app, ctx, now, home);
    if scene.hop || scene.cheer {
        app.mascot.buddy.hopped = now;
    }
    if scene.cheer {
        app.mascot
            .party
            .burst(feet - vec2(0.0, height), now, CHEERS[(now as usize) % CHEERS.len()]);
    }
    let screen = ctx.screen_rect();
    let fx_painter = ctx.layer_painter(LayerId::new(Order::Middle, Id::new("q0bo_fx")));
    if let Some(rect) = scene.point.and_then(|point| spot(ctx, point)) {
        fx::highlight(&fx_painter, rect, now, screen);
    }
    let typing = !bubble::finished(&scene.line, scene.typed);
    app.mascot.lively = typing || tutorial.busy() || tutorial.hand.shown > 0.0 || scene.point.is_some();
    let hand = (tutorial.hand.shown > 0.3).then_some(tutorial.hand.pos);
    let arm = match hand {
        Some(at) if tutorial.busy() => Arm::Point(at - (feet - vec2(0.0, height * 0.5))),
        _ if scene.buttons.first() == Some(&"open ask q0bo") => Arm::Wave,
        _ => Arm::Rest,
    };
    let rect = show(app, ctx, feet, height, scene.mood, typing, arm, hand, now);
    let said = bubble::speech(
        ctx,
        Id::new("q0bo_bubble"),
        rect,
        &scene.line,
        scene.typed,
        &scene.buttons,
        scene.primary,
        scene.corner,
    );
    let top = ctx.layer_painter(LayerId::new(Order::Tooltip, Id::new("q0bo_hand")));
    tutorial.paint(app, ctx, &top, now);
    if said.hurry {
        tutorial.hurry(now);
    }
    if said.corner {
        return Some(bye(app, QUIT, Mood::Wink, now));
    }
    if let Some(button) = said.clicked {
        match tutorial.press(app, button, now) {
            tutorial::Outcome::Going => {}
            tutorial::Outcome::Finished { ask } => {
                app.settings.mascot.tutorial_done = true;
                app.settings.save();
                if ask {
                    open_ask(app);
                    return Some(Scene::Leaving(now));
                }
                return Some(bye(app, DONE, Mood::Love, now));
            }
            tutorial::Outcome::Skipped => return Some(bye(app, QUIT, Mood::Wink, now)),
        }
    }
    Some(Scene::Tutorial(tutorial))
}

/// Where his feet go in the editor: the stage's bottom-right corner, or the
/// window's when there is no stage in sight.
fn standing(app: &EditorApp, ctx: &Context, height: f32) -> Pos2 {
    let screen = ctx.screen_rect();
    let usual = match spot(ctx, Spot::Stage) {
        Some(stage) if !app.home_visible() && stage.width() > height * 2.0 => {
            pos2(stage.right() - height * 0.6, stage.bottom() - 12.0)
        }
        _ => pos2(screen.right() - height * 0.7, screen.bottom() - 24.0),
    };
    place(&app.mascot.buddy, ctx, usual)
}

/// His usual spot moved by wherever he was dragged, kept on screen.
fn place(buddy: &Buddy, ctx: &Context, usual: Pos2) -> Pos2 {
    let screen = ctx.screen_rect();
    let at = usual + buddy.offset;
    pos2(
        within(at.x, screen.left() + 60.0, screen.right() - 60.0),
        within(at.y, screen.top() + 150.0, screen.bottom() - 4.0),
    )
}

/// `value` kept between `low` and `high`, or halfway when there is no room.
pub(crate) fn within(value: f32, low: f32, high: f32) -> f32 {
    if low > high {
        (low + high) * 0.5
    } else {
        value.clamp(low, high)
    }
}

/// Draws him, lets him be dragged around and poked; returns where he is.
#[allow(clippy::too_many_arguments)]
fn show(
    app: &mut EditorApp,
    ctx: &Context,
    feet: Pos2,
    height: f32,
    mood: Mood,
    talking: bool,
    arm: Arm,
    watching: Option<Pos2>,
    now: f64,
) -> Rect {
    let rect = face::bounds(feet, height);
    let eyes = rect.center() - vec2(0.0, height * 0.12);
    let look = watching
        .or_else(|| ctx.pointer_hover_pos())
        .map_or(Vec2::ZERO, |target| {
            ((target - eyes) / 260.0).clamp(vec2(-1.0, -1.0), vec2(1.0, 1.0))
        });
    let buddy = &mut app.mascot.buddy;
    let pose = Pose {
        mood: buddy.mood(now, if talking && mood == Mood::Happy { Mood::Talk } else { mood }),
        talking,
        look,
        arm,
        hop: buddy.hop(now),
        scale: buddy.scale(now),
    };
    let response = egui::Area::new(Id::new("q0bo"))
        .order(Order::Foreground)
        .fixed_pos(rect.min)
        .show(ctx, |ui| {
            let (body, response) = ui.allocate_exact_size(rect.size(), Sense::click_and_drag());
            face::paint(ui.painter(), body.center_bottom(), height, &pose, now);
            response
        })
        .inner;
    if response.dragged() {
        buddy.offset += response.drag_delta();
    }
    if response.clicked() {
        buddy.pokes = if now - buddy.poked < 3.0 { buddy.pokes + 1 } else { 1 };
        buddy.poked = now;
    }
    let cursor = if response.dragged() {
        CursorIcon::Grabbing
    } else {
        CursorIcon::Grab
    };
    response
        .on_hover_cursor(cursor)
        .on_hover_text("q0bo! drag him around, or poke him");
    rect
}
