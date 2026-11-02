//! Plays q0bo's greeting and tutorial with real clicks on his buttons and
//! saves screenshots along the way.
//! Usage: q0bo_shot <out dir> [paper left top right bottom]

use std::path::PathBuf;

use eframe::App;
use egui::{Event, Key, Modifiers, PointerButton, Pos2, Rect};
use q0editor::app::Action;
use q0editor::settings::Settings;
use q0editor::EditorApp;

type Step = Box<dyn FnMut(&mut EditorApp, &egui::Context, &mut Vec<Event>)>;

struct ShotApp {
    editor: EditorApp,
    out: PathBuf,
    steps: Vec<(f64, Option<String>, Step)>,
    start: Option<f64>,
    waiting: Option<String>,
    events: Vec<Event>,
}

fn click(events: &mut Vec<Event>, at: Pos2) {
    events.push(Event::PointerMoved(at));
    for pressed in [true, false] {
        events.push(Event::PointerButton {
            pos: at,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        });
    }
}

/// Clicks a bubble button twice, apart: if he was still typing, the first
/// click only finishes the line. A spare second click lands on the next,
/// freshly started line and just finishes that one.
fn press(events: &mut Vec<Event>, at: Pos2) {
    click(events, at);
    for _ in 0..20 {
        events.push(Event::PointerMoved(at));
    }
    click(events, at);
}

fn key(events: &mut Vec<Event>, key: Key) {
    for pressed in [true, false] {
        events.push(Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: Modifiers::NONE,
        });
    }
}

fn drag(events: &mut Vec<Event>, from: Pos2, to: Pos2) {
    events.push(Event::PointerMoved(from));
    events.push(Event::PointerButton {
        pos: from,
        button: PointerButton::Primary,
        pressed: true,
        modifiers: Modifiers::NONE,
    });
    for k in 1..=12 {
        events.push(Event::PointerMoved(from + (to - from) * (k as f32 / 12.0)));
    }
    events.push(Event::PointerButton {
        pos: to,
        button: PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::NONE,
    });
}

/// The middle of a button in q0bo's bubble.
fn bubble_button(ctx: &egui::Context, labels: &[&str], index: usize) -> Pos2 {
    let rect = ctx
        .memory(|memory| memory.area_rect(egui::Id::new("q0bo_bubble")))
        .expect("q0bo's bubble");
    let char_w = ctx.fonts(|fonts| fonts.glyph_width(&egui::FontId::monospace(13.0), 'x'));
    let width = |label: &str| (label.chars().count() + 4) as f32 * char_w + 14.0;
    let mut x = rect.left() + 14.0;
    for label in &labels[..index] {
        x += width(label) + 8.0;
    }
    Pos2::new(x + width(labels[index]) * 0.5, rect.bottom() - 10.0 - 12.0)
}

impl App for ShotApp {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        self.editor.update(ctx, frame);
        let now = ctx.input(|input| input.time);
        let start = *self.start.get_or_insert(now);
        let shots: Vec<_> = ctx.input(|input| {
            input
                .raw
                .events
                .iter()
                .filter_map(|event| match event {
                    Event::Screenshot { image, .. } => Some(image.clone()),
                    _ => None,
                })
                .collect()
        });
        for image in shots {
            if let Some(name) = self.waiting.take() {
                let [w, h] = image.size;
                let bytes: Vec<u8> = image.pixels.iter().flat_map(|c| c.to_array()).collect();
                let path = self.out.join(format!("{name}.png"));
                image::save_buffer(&path, &bytes, w as u32, h as u32, image::ColorType::Rgba8)
                    .expect("save");
                println!("saved {} ({})", path.display(), self.editor.session.status);
            }
        }
        if self.waiting.is_none() {
            if let Some(index) = self
                .steps
                .iter()
                .position(|(at, _, _)| *at <= now - start)
            {
                let (_, shot, mut step) = self.steps.remove(index);
                step(&mut self.editor, ctx, &mut self.events);
                if let Some(name) = shot {
                    self.waiting = Some(name);
                    ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot);
                }
            }
        }
        if self.steps.is_empty() && self.waiting.is_none() {
            self.editor.state.dirty = false;
            self.editor.close_requested = true;
            std::process::exit(0);
        }
        ctx.request_repaint();
    }

    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw: &mut egui::RawInput) {
        if !self.events.is_empty() {
            raw.events.push(self.events.remove(0));
        }
    }
}

fn main() -> eframe::Result<()> {
    let mut args = std::env::args().skip(1);
    let out = PathBuf::from(args.next().expect("out dir"));
    let numbers: Vec<f32> = args.filter_map(|arg| arg.parse().ok()).collect();
    let paper = (numbers.len() >= 4).then(|| {
        Rect::from_min_max(Pos2::new(numbers[0], numbers[1]), Pos2::new(numbers[2], numbers[3]))
    });
    let topics: Vec<Pos2> = numbers
        .get(4..)
        .unwrap_or_default()
        .chunks_exact(2)
        .map(|xy| Pos2::new(xy[0], xy[1]))
        .collect();
    std::fs::create_dir_all(&out).unwrap();
    let mut editor = EditorApp::default();
    editor.settings = Settings::default();

    let mut steps: Vec<(f64, Option<String>, Step)> = Vec::new();
    let mut t = 0.0;
    let mut at = |dt: f64, shot: Option<&str>, f: Step, steps: &mut Vec<(f64, Option<String>, Step)>| {
        t += dt;
        steps.push((t, shot.map(str::to_string), f));
    };
    let nothing = || -> Step { Box::new(|_, _, _| {}) };
    // Twice: a first click on a line still being typed only finishes it.
    let next = || -> Step {
        Box::new(|_, ctx, events| {
            let at = bubble_button(ctx, &["next >>"], 0);
            press(events, at);
        })
    };
    // Stage units to screen, once the paper's place is known.
    let stage = move |x: f32, y: f32| -> Pos2 {
        let paper = paper.unwrap_or(Rect::from_min_size(Pos2::new(247.0, 172.0), egui::vec2(668.0, 501.0)));
        paper.min + egui::vec2(x, y) * (paper.width() / 640.0)
    };

    at(4.5, Some("01_hello"), nothing(), &mut steps);
    at(
        0.3,
        None,
        Box::new(|_, ctx, events| {
            press(events, bubble_button(ctx, &["yea let's go!!", "nah, skip"], 0))
        }),
        &mut steps,
    );
    at(3.0, Some("02_intro"), nothing(), &mut steps);
    at(0.2, None, next(), &mut steps);
    at(2.8, Some("03_stage"), nothing(), &mut steps);
    at(0.2, None, next(), &mut steps);
    // His show: the line, then the oval tool, then the drag.
    at(3.0, Some("04_drawing"), nothing(), &mut steps);
    at(2.2, Some("05_tada"), nothing(), &mut steps);
    at(0.2, None, next(), &mut steps);
    at(2.0, Some("06_your_turn"), nothing(), &mut steps);
    at(
        0.2,
        None,
        Box::new(|editor, _, _| editor.queue(Action::SelectTool(q0editor::state::Tool::Rectangle))),
        &mut steps,
    );
    at(
        0.3,
        None,
        Box::new(move |_, _, events| drag(events, stage(330.0, 260.0), stage(440.0, 350.0))),
        &mut steps,
    );
    at(0.6, Some("07_cheer"), nothing(), &mut steps);
    at(1.2, None, next(), &mut steps);
    at(2.9, Some("08_selecting"), nothing(), &mut steps);
    at(1.2, Some("08_moving"), nothing(), &mut steps);
    at(2.3, None, next(), &mut steps);
    at(
        1.8,
        None,
        Box::new(move |_, _, events| drag(events, stage(300.0, 225.0), stage(470.0, 385.0))),
        &mut steps,
    );
    at(
        0.6,
        None,
        Box::new(move |_, _, events| drag(events, stage(385.0, 272.0), stage(300.0, 180.0))),
        &mut steps,
    );
    at(0.6, Some("08b_moved"), nothing(), &mut steps);
    at(1.0, None, next(), &mut steps);
    at(2.6, Some("09_timeline"), nothing(), &mut steps);
    at(0.2, None, next(), &mut steps);
    at(2.2, Some("10_keyframe"), nothing(), &mut steps);
    at(2.3, None, next(), &mut steps);
    at(3.0, Some("11_move_on_key"), nothing(), &mut steps);
    at(2.6, None, next(), &mut steps);
    at(4.4, Some("12_tween"), nothing(), &mut steps);
    at(0.2, None, next(), &mut steps);
    at(2.9, Some("13_playing"), nothing(), &mut steps);
    at(0.4, None, next(), &mut steps);
    at(1.0, None, Box::new(|_, _, events| key(events, Key::Enter)), &mut steps);
    at(0.6, None, Box::new(|_, _, events| key(events, Key::Enter)), &mut steps);
    at(0.4, Some("14_replay"), nothing(), &mut steps);
    at(1.2, None, next(), &mut steps);
    at(1.6, None, next(), &mut steps);
    at(2.4, None, next(), &mut steps);
    at(3.0, Some("15_end"), nothing(), &mut steps);
    at(
        0.2,
        None,
        Box::new(|_, ctx, events| {
            press(events, bubble_button(ctx, &["open ask q0bo", "kthxbai"], 0))
        }),
        &mut steps,
    );
    at(2.0, Some("16_ask"), nothing(), &mut steps);
    for (index, topic) in topics.iter().enumerate() {
        let topic = *topic;
        at(0.3, None, Box::new(move |_, _, events| click(events, topic)), &mut steps);
        at(2.6, Some(["17_ask_topic", "18_ask_topic"][index.min(1)]), nothing(), &mut steps);
    }

    // A phone-sized window: only the greeting and the first steps.
    let narrow = std::env::var_os("Q0BO_NARROW").is_some();
    if narrow {
        steps.truncate(6);
    }
    let size = if narrow { [430.0, 880.0] } else { [1440.0, 900.0] };
    eframe::run_native(
        "q0editor q0bo",
        eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default().with_inner_size(size),
            multisampling: 4,
            ..Default::default()
        },
        Box::new(move |_cc| {
            Box::new(ShotApp {
                editor,
                out,
                steps,
                start: None,
                waiting: None,
                events: Vec::new(),
            })
        }),
    )
}
