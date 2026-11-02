//! The "ask q0bo" tab: type a question or pick a topic and he explains it
//! the simple way.

use egui::{vec2, Color32, FontId, Key, RichText, Sense, Ui};

use super::bubble;
use super::face::{self, Mood, Pose, RED};
use crate::app::EditorApp;

/// What the tab remembers.
#[derive(Default)]
pub(crate) struct Ask {
    query: String,
    topic: Option<usize>,
    said_at: f64,
    /// Something to say that is not a topic: a shrug, or a reaction to a poke.
    quip: Option<(&'static str, Mood)>,
    poked_at: f64,
    pokes: usize,
    /// The tab as a window, on phones where nothing docks.
    pub(crate) window_open: bool,
}

impl Ask {
    /// Opens on a topic, for when someone else sends the user here.
    pub(crate) fn greet(&mut self, now: f64) {
        self.topic = None;
        self.quip = None;
        self.said_at = now;
    }
}

struct Topic {
    group: &'static str,
    title: &'static str,
    /// Words people might use asking about it, besides the title's.
    words: &'static str,
    mood: Mood,
    say: &'static str,
}

const GROUPS: [&str; 5] = [
    "basics",
    "animation",
    "organizing",
    "fancy stuff",
    "files & sharing",
];

const TOPICS: &[Topic] = &[
    Topic {
        group: "basics",
        title: "drawing stuff",
        words: "draw drawing brush pencil pen line shape shapes rectangle square box oval circle ellipse sketch curve",
        mood: Mood::Happy,
        say: "pick a tool on teh left, then drag on teh paper:\n\n• brush «({brush})» or pencil «({pencil})» = freehand\n• line «({line})», rectangle «({rect})», oval «({oval})» = neat shapes\n• pen «({pen})» = click dots 4 smooth curvy paths\n\nhold shift while dragging a shape 4 a perfect square/circle",
    },
    Topic {
        group: "basics",
        title: "colors",
        words: "color colour colors colours fill stroke outline paint bucket eyedropper picker thickness width",
        mood: Mood::Love,
        say: "with a drawing tool picked (or a shape selected), look at teh properties panel on teh right:\n\n• stroke = teh outline (color + thickness)\n• fill = teh inside\n\nteh paint bucket «({bucket})» fills a shape when u click it. teh eyedropper «({eyedropper})» copies a color off teh stage",
    },
    Topic {
        group: "basics",
        title: "moving & resizing",
        words: "move moving select selection arrow resize scale size rotate rotation turn spin drag nudge transform handles flip",
        mood: Mood::Cool,
        say: "teh arrow tool «({select})»:\n\n• drag a thing = move it\n• drag teh corner handles = resize\n• drag just outside a corner = rotate\n«• arrow keys nudge it (shift = tiny steps)\n»\nsubselect «({subselect})» moves single points of a shape instead",
    },
    Topic {
        group: "basics",
        title: "undo",
        words: "undo redo mistake oops back revert",
        mood: Mood::Wink,
        say: "messed up? no biggie.\n\n«{undo} = undo, {redo} = redo. »also in teh Edit menu. it remembers a LOT so go wild lol",
    },
    Topic {
        group: "basics",
        title: "zoom & looking around",
        words: "zoom zooming pan panning scroll view navigate look around closer",
        mood: Mood::Talk,
        say: "«• mouse wheel = zoom\n• hold space or H and drag = slide teh view (middle mouse works 2)\n• {zoomreset} = fit it back\n»teh View menu has zoom in/out 2. on a phone: pinch n drag w/ 2 fingers",
    },
    Topic {
        group: "basics",
        title: "snapping",
        words: "snap snapping magnet align line up grid",
        mood: Mood::Smug,
        say: "teh magnet button above teh stage «(or {snap})» turns snapping on. then points stick 2 other points, line ends n teh middle of teh stage.\n\nhold shift 2 drag without snapping",
    },
    Topic {
        group: "animation",
        title: "frames & keyframes",
        words: "frame frames keyframe keyframes key keys blank timeline insert remove hold extend",
        mood: Mood::Talk,
        say: "teh timeline = ur animation, 1 lil box per frame.\n\n• keyframe «({key})» = a frame where u decide what's there. it starts as a copy of teh last 1\n• blank keyframe «({blank})» = an empty 1, draw fresh\n• frame «({frame})» = just holds teh last keyframe longer\n«• {clearkey} clears a keyframe, {removeframe} removes a frame»",
    },
    Topic {
        group: "animation",
        title: "motion tweens",
        words: "tween tweens tweening motion slide move animate movement inbetween interpolate",
        mood: Mood::Excited,
        say: "makes a thing slide, spin or grow on its own:\n\n1. keyframe 1: put ur thing somewhere\n2. a later keyframe «({key})»: move it\n3. click a frame between them, right-click > create motion tween «(or {tween})»\n\nthat's what i did w/ my ball in teh tutorial ;)",
    },
    Topic {
        group: "animation",
        title: "shape tweens",
        words: "shape tween morph morphing transform into become",
        mood: Mood::Surprised,
        say: "morphs 1 drawing into another (like a square into a circle):\n\n1. draw on a keyframe\n2. make a later keyframe and draw what it should become\n3. click a frame between > create shape tween «({shape})»\n\nteh span turns green. motion tweens = objects, shape tweens = drawings",
    },
    Topic {
        group: "animation",
        title: "easing",
        words: "ease easing speed slow fast smooth bounce curve accelerate",
        mood: Mood::Smug,
        say: "easing = speed changes, so stuff doesn't move like a robot.\n\nclick a frame inside a tween, then in teh properties panel pick an ease (in, out, bouncy...) or hit \"edit curve...\" 2 bend it urself",
    },
    Topic {
        group: "animation",
        title: "onion skin",
        words: "onion skin ghost ghosts previous next frames trace",
        mood: Mood::Wink,
        say: "teh onion button in teh timeline shows see-thru ghosts of teh frames before n after, so u can line up ur drawings.\n\nteh numbers next 2 it = how many frames of ghosts",
    },
    Topic {
        group: "animation",
        title: "playing & jumping around",
        words: "play playing stop pause playback preview watch scrub first last step jump",
        mood: Mood::Excited,
        say: "«• {play} = play / stop\n• {first} / {last} = jump 2 start / end\n• left/right arrows = 1 frame (when nothing's selected)\n»• click any frame in teh timeline 2 jump there",
    },
    Topic {
        group: "animation",
        title: "fast preview",
        words: "fast preview test run player watch full",
        mood: Mood::Cool,
        say: "View > Fast Preview «({preview})» plays ur whole animation in q0player, exactly like it'll look exported. scripts run 2!\n\n«{previewscene} = just this scene»",
    },
    Topic {
        group: "animation",
        title: "record mode",
        words: "record recording puppet hold held drag every frame live",
        mood: Mood::Excited,
        say: "teh record button in teh timeline: while it's on, dragging an object keys EVERY frame as u go. like puppeteering!!\n\nturn it off when ur done or u'll key everything lol",
    },
    Topic {
        group: "organizing",
        title: "layers",
        words: "layer layers folder folders hide show lock unlock order front back above below stack",
        mood: Mood::Talk,
        say: "layers = see-thru sheets stacked up. top ones cover bottom ones.\n\n• teh + button in teh timeline adds 1\n• u draw on teh highlighted layer\n• eye = hide it, lock = can't touch it\n• folders keep big projects tidy\n• drag layers up n down 2 restack",
    },
    Topic {
        group: "organizing",
        title: "masks",
        words: "mask masks masking clip clipping window reveal hide cut",
        mood: Mood::Thinking,
        say: "a mask is like a window: only teh parts of teh layer under it that r INSIDE teh mask's shapes show up.\n\nselect a layer, hit \"add a mask\" in teh timeline, then draw a shape on teh mask layer",
    },
    Topic {
        group: "organizing",
        title: "symbols (q0rgs)",
        words: "symbol symbols q0rg q0rgs reuse reusable instance instances clip group convert nested character",
        mood: Mood::Excited,
        say: "a symbol (we call em q0rgs) = a reusable thing with its OWN timeline. like a character that blinks by itself.\n\n• select stuff > convert 2 symbol «({symbol})»\n• double-click it 2 edit inside\n• it lives in teh library: drag it out as many times as u want. change 1, they all change",
    },
    Topic {
        group: "organizing",
        title: "library",
        words: "library asset assets items import folder rename delete reuse",
        mood: Mood::Happy,
        say: "teh library (right side) keeps ur symbols, pictures, sounds, videos n 3d models.\n\n• drag stuff from it onto teh stage\n• right-click 2 rename or delete\n• folders keep it tidy",
    },
    Topic {
        group: "organizing",
        title: "panels & layouts",
        words: "panel panels layout layouts workspace dock tab tabs window rearrange move float",
        mood: Mood::Smug,
        say: "drag a panel's tab around 2 rearrange: drop it on an edge 2 split, away from targets 2 float it.\n\nteh Layout menu (top right) saves ur setups n brings back teh default. i live in a panel 2 btw :3",
    },
    Topic {
        group: "fancy stuff",
        title: "bones & rigging",
        words: "bone bones rig rigging skeleton puppet ik joint joints pose posing bind character",
        mood: Mood::Cool,
        say: "make puppets!!\n\n1. rig tool «({rig})», hit \"create rig\"\n2. switch it 2 \"add bone\" and drag 2 draw bones\n3. select a drawing, \"bind selected object\" glues it 2 a bone\n4. \"pose\" mode: drag bones around. keyframes remember teh poses",
    },
    Topic {
        group: "fancy stuff",
        title: "camera",
        words: "camera cam zoom pan film shot follow view",
        mood: Mood::Cool,
        say: "teh camera button in teh timeline turns on a camera track. key it like a layer 2 pan, zoom n turn teh view over time. like filming ur scene",
    },
    Topic {
        group: "fancy stuff",
        title: "3d models",
        words: "3d model models mesh fbx gltf glb obj blender import studio sculpt",
        mood: Mood::Excited,
        say: "yes we have 3d!! B)\n\n• File > Import 3D Model: fbx, gltf, glb, obj\n• File > New 3D Model opens teh model editor (shapes, bones, materials, animation)\n• teh 3D button (top bar) = camera, models n 3d lights\n\n3d lights light up ur 2d drawings 2. kinda sick ngl",
    },
    Topic {
        group: "fancy stuff",
        title: "lights",
        words: "light lights lighting lamp shadow shadows glow dark",
        mood: Mood::Love,
        say: "turn it on in Settings > Extra tools > Light editor, then hit Light in teh top bar.\n\ndarken teh scene, drop lamps on it, drag em around. moody vibes unlocked",
    },
    Topic {
        group: "fancy stuff",
        title: "particles",
        words: "particle particles smoke fire sparks rain snow confetti effect effects emitter",
        mood: Mood::Excited,
        say: "Settings > Extra tools > Particle system turns it on, then Particles in teh top bar.\n\nsmoke, sparks, rain... confetti like mine!!",
    },
    Topic {
        group: "fancy stuff",
        title: "physics",
        words: "physics gravity fall falling bounce collide collision rope ropes liquid water simulate",
        mood: Mood::Dizzy,
        say: "Settings > Extra tools > Physics editor turns it on, then Physics in teh top bar.\n\nmake stuff fall, bounce n swing on ropes. physics made me dizzy once",
    },
    Topic {
        group: "fancy stuff",
        title: "scripts (q0lang)",
        words: "script scripts scripting code coding q0lang program programming interactive game games button buttons logic",
        mood: Mood::Smug,
        say: "q0lang = our lil code language 4 interactive stuff: buttons, games, logic.\n\n• View > Script Editor «({script})»\n• frames can have their own scripts 2\n• \"simple\" mode shows code as blocks if u don't wanna type\n\nscripts run in fast preview n q0player",
    },
    Topic {
        group: "fancy stuff",
        title: "text",
        words: "text type typing font fonts letters words title",
        mood: Mood::Talk,
        say: "text tool «({text})»: click teh stage n type. font, size n color r in teh properties panel",
    },
    Topic {
        group: "files & sharing",
        title: "saving",
        words: "save saving saved file files project open autosave crash recover recovery lost q1s",
        mood: Mood::Cool,
        say: "File > Save «({save})» saves ur project, File > Save As «({saveas})» makes a copy.\n\nif q0editor ever crashes, Home offers ur unsaved work back (Settings > Autosave)",
    },
    Topic {
        group: "files & sharing",
        title: "exporting",
        words: "export exporting render video mp4 gif png image images share sharing q0enc q0s q0v sequence",
        mood: Mood::Excited,
        say: "File > Export «({export})» opens q0enc, teh export workshop:\n\n• mp4 = video\n• gif = 4 memes\n• png sequence / image = pictures\n• q0s = plays in q0player, scripts n all\n• q0v = our own video format",
    },
    Topic {
        group: "files & sharing",
        title: "importing",
        words: "import importing picture pictures image images photo sound sounds audio music video videos media drop",
        mood: Mood::Happy,
        say: "File > Import Media to Library 4 pictures, sounds n videos. or just drag files onto teh window!!\n\n3d models: File > Import 3D Model",
    },
    Topic {
        group: "files & sharing",
        title: "flash files",
        words: "flash fla xfl animate adobe old convert q0fla",
        mood: Mood::Love,
        say: "got old .fla files? q0fla (top bar) opens FLA n XFL in q0editor. symbols, timelines n tweens come along.\n\nwelcome home, old projects <3",
    },
    Topic {
        group: "files & sharing",
        title: "shortcuts & settings",
        words: "shortcut shortcuts hotkey hotkeys keys keyboard settings theme themes color dark light preferences",
        mood: Mood::Wink,
        say: "Settings (top right):\n\n• Keyboard shortcuts > manage shortcuts... = change any key\n• Appearance = pick a theme, every panel follows it\n• Extra tools = physics, particles n lights",
    },
    Topic {
        group: "files & sharing",
        title: "plugins",
        words: "plugin plugins addon addons extension extensions mod mods",
        mood: Mood::Smug,
        say: "teh Plugins menu (top) has add-ons that bring new tools n panels. made by other ppl. or by u!",
    },
    Topic {
        group: "files & sharing",
        title: "who r u?",
        words: "q0bo who you mascot robot bot hello hi hey name about",
        mood: Mood::Wink,
        say: "i'm q0bo!! i live in ur editor :3\n\nlikes: keyframes, teh color red, dial-up noises\ndislikes: crashes, unsaved work\na/s/l? 404/bot/ur pc lol",
    },
];

const HELLO: &str = "sup!! ask me anything about q0editor. type a question up there or pick a topic below :3";
const SHRUG: &str = "uhh idk that 1 :( try other words? or pick a topic below";
const QUIPS: [(&str, Mood); 6] = [
    ("hey that tickles lol", Mood::Happy),
    ("*boop*", Mood::Wink),
    ("y u poke me o_O", Mood::Surprised),
    ("stop it, i'm ticklish!!1", Mood::Excited),
    ("brb rebooting... jk", Mood::Smug),
    ("ok now i'm dizzy. happy?", Mood::Dizzy),
];

/// Words that say nothing about the topic.
const FILLER: [&str; 24] = [
    "how", "do", "to", "the", "teh", "an", "is", "what", "can", "my", "in", "of", "it", "and",
    "or", "you", "where", "with", "does", "use", "get", "make", "this", "that",
];

/// How well a topic answers a question. A word counts most in the title,
/// and a whole word more than the start of one ("tween" in "tweens").
fn score(topic: &Topic, query: &str) -> usize {
    let query = query.to_lowercase();
    let best = |known: &mut dyn Iterator<Item = &str>, word: &str, exact: usize, near: usize| {
        known
            .map(|known| {
                if known == word {
                    exact
                } else if word.len() >= 4
                    && known.len() >= 4
                    && (known.starts_with(word) || word.starts_with(known))
                {
                    near
                } else {
                    0
                }
            })
            .max()
            .unwrap_or(0)
    };
    query
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| word.len() >= 2 && !FILLER.contains(word))
        .map(|word| {
            best(
                &mut topic.title.split(|c: char| !c.is_alphanumeric()),
                word,
                4,
                2,
            ) + best(&mut topic.words.split_whitespace(), word, 3, 1)
        })
        .sum()
}

/// Topics that answer a question, best first.
fn matches(query: &str) -> Vec<usize> {
    let mut found: Vec<(usize, usize)> = TOPICS
        .iter()
        .enumerate()
        .map(|(index, topic)| (score(topic, query), index))
        .filter(|(score, _)| *score > 0)
        .collect();
    found.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    found.into_iter().map(|(_, index)| index).collect()
}

/// The tab's contents.
pub(crate) fn panel(app: &mut EditorApp, ui: &mut Ui) {
    let now = ui.input(|input| input.time);
    let mut ask = std::mem::take(&mut app.mascot.ask);
    let touch = crate::touch::ENABLED;

    let (line, mood) = match (ask.quip, ask.topic) {
        (Some((quip, mood)), _) => (quip.to_string(), mood),
        (None, Some(index)) => (
            bubble::fill(TOPICS[index].say, &app.settings.hotkeys, touch),
            TOPICS[index].mood,
        ),
        (None, None) => (HELLO.to_string(), Mood::Happy),
    };
    let typed = ((now - ask.said_at) as f32) * bubble::TYPING * 1.6;
    let typing = !bubble::finished(&line, typed);

    egui::ScrollArea::vertical()
        .id_source("ask_q0bo")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.add_space(6.0);
            let wide = ui.available_width() > 360.0;
            let mut layout = |ui: &mut Ui| {
                let size = vec2(92.0, 112.0);
                let (rect, response) = ui.allocate_exact_size(size, Sense::click());
                let since_poke = (now - ask.poked_at) as f32;
                let hop = if since_poke < 0.5 {
                    (since_poke / 0.5 * std::f32::consts::PI).sin()
                } else {
                    0.0
                };
                let look = ui
                    .ctx()
                    .pointer_hover_pos()
                    .map_or(egui::Vec2::ZERO, |pointer| {
                        ((pointer - rect.center()) / 220.0).clamp(vec2(-1.0, -1.0), vec2(1.0, 1.0))
                    });
                face::paint(
                    ui.painter(),
                    rect.center_bottom() - vec2(0.0, 4.0),
                    rect.height() - 8.0,
                    &Pose {
                        mood: if typing && mood == Mood::Happy { Mood::Talk } else { mood },
                        talking: typing,
                        look,
                        hop,
                        ..Pose::default()
                    },
                    now,
                );
                if response
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .on_hover_text("poke q0bo")
                    .clicked()
                {
                    ask.quip = Some(QUIPS[ask.pokes % QUIPS.len()]);
                    ask.pokes += 1;
                    ask.poked_at = now;
                    ask.said_at = now;
                }
                let face_at = rect.center() - vec2(0.0, rect.height() * 0.12);
                ui.vertical(|ui| {
                    let said = bubble::inline(ui, &line, typed, &[], face_at);
                    if said.hurry {
                        ask.said_at = now - 1000.0;
                    }
                });
            };
            if wide {
                ui.horizontal_top(|ui| layout(ui));
            } else {
                ui.vertical(|ui| layout(ui));
            }

            ui.add_space(10.0);
            ui.horizontal(|ui| {
                let field = ui.add(
                    egui::TextEdit::singleline(&mut ask.query)
                        .hint_text("ask me stuff... like \"how do i tween\"")
                        .font(FontId::monospace(13.0))
                        .desired_width((ui.available_width() - 90.0).max(80.0)),
                );
                let entered = field.lost_focus() && ui.input(|input| input.key_pressed(Key::Enter));
                if bubble::retro_button(ui, "ask", true).clicked() || entered {
                    ask.said_at = now;
                    ask.quip = None;
                    match matches(&ask.query).first() {
                        Some(best) => ask.topic = Some(*best),
                        None => {
                            ask.topic = None;
                            ask.quip = Some((SHRUG, Mood::Sad));
                        }
                    }
                }
            });
            let query = ask.query.trim();
            if !query.is_empty() {
                let found = matches(query);
                ui.add_space(4.0);
                ui.horizontal_wrapped(|ui| {
                    if found.is_empty() {
                        ui.weak("no topic matches yet");
                    } else {
                        ui.weak("maybe:");
                    }
                    for index in found.into_iter().take(4) {
                        if chip(ui, TOPICS[index].title, ask.topic == Some(index)).clicked() {
                            ask.topic = Some(index);
                            ask.quip = None;
                            ask.said_at = now;
                        }
                    }
                });
            }

            ui.add_space(8.0);
            for group in GROUPS {
                ui.add_space(4.0);
                ui.label(
                    RichText::new(group)
                        .font(FontId::monospace(12.0))
                        .color(RED)
                        .strong(),
                );
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                    for (index, topic) in TOPICS.iter().enumerate() {
                        if topic.group == group
                            && chip(ui, topic.title, ask.topic == Some(index) && ask.quip.is_none())
                                .clicked()
                        {
                            ask.topic = Some(index);
                            ask.quip = None;
                            ask.said_at = now;
                        }
                    }
                });
            }
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if bubble::retro_button(ui, "replay teh tutorial", false).clicked() {
                    app.queue(crate::app::Action::StartTutorial);
                }
            });
            ui.add_space(8.0);
        });

    if typing || (now - ask.poked_at) < 0.6 {
        ui.ctx().request_repaint();
    } else if ui.rect_contains_pointer(ui.max_rect()) {
        // He breathes, blinks and watches the pointer while it is near him;
        // otherwise he holds still and the editor can idle.
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(80));
    }
    app.mascot.ask = ask;
}

/// The tab as a floating window, for layouts without docking.
pub(crate) fn window(app: &mut EditorApp, ctx: &egui::Context) {
    if !app.mascot.ask.window_open {
        return;
    }
    let mut open = true;
    egui::Window::new("ask q0bo")
        .open(&mut open)
        .default_width(380.0)
        .default_height(520.0)
        .resizable(true)
        .show(ctx, |ui| panel(app, ui));
    if !open {
        app.mascot.ask.window_open = false;
    }
}

fn chip(ui: &mut Ui, label: &str, selected: bool) -> egui::Response {
    let text = RichText::new(label).font(FontId::monospace(12.0));
    let text = if selected {
        text.color(Color32::WHITE)
    } else {
        text
    };
    let button = egui::Button::new(text)
        .rounding(10.0)
        .fill(if selected {
            RED
        } else {
            ui.visuals().widgets.inactive.weak_bg_fill
        });
    ui.add(button)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn best(question: &str) -> &'static str {
        matches(question)
            .first()
            .map_or("(nothing)", |index| TOPICS[*index].title)
    }

    #[test]
    fn questions_find_their_topics() {
        assert_eq!(best("how do i tween"), "motion tweens");
        assert_eq!(best("how do i make a symbol"), "symbols (q0rgs)");
        assert_eq!(best("import fbx"), "3d models");
        assert_eq!(best("export a gif"), "exporting");
        assert_eq!(best("bones"), "bones & rigging");
        assert_eq!(best("how do i change the colour"), "colors");
        assert_eq!(best("how do i morph shapes"), "shape tweens");
        assert_eq!(best("who are u"), "who r u?");
        assert_eq!(best("asdfgh"), "(nothing)");
    }

    #[test]
    fn every_topic_is_in_a_group_short_and_fully_filled_in() {
        let hotkeys = crate::hotkeys::HotkeySettings::default();
        for topic in TOPICS {
            assert!(GROUPS.contains(&topic.group), "{}", topic.title);
            assert!(topic.say.chars().count() < 420, "{} is too long", topic.title);
            for touch in [false, true] {
                let said = bubble::fill(topic.say, &hotkeys, touch);
                assert!(
                    !said.contains(&['{', '}', '\u{ab}', '\u{bb}'][..]),
                    "{}: {said}",
                    topic.title
                );
            }
        }
    }
}
