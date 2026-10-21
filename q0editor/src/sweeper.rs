//! q0sweeper: a minesweeper hidden in the credits, drawn the way the old
//! one was. Grey bevels, red LED counters, blocky coloured numbers, and the
//! q0editor demon where the smiley used to be.
//!
//! The way in: in the credits, knock on the "q0" mark six times, then six
//! more, then six more, a short breath between each six. Every six grows
//! the mark a horn; the third opens the game. Unlocked, it stays under Help.

use std::time::{Duration, Instant};

use egui::{pos2, vec2, Color32, Context, Id, Painter, Pos2, Rect, Sense, Stroke, Ui};
use serde::{Deserialize, Serialize};

use crate::app::EditorApp;

// ------------------------------------------------------------------ settings

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Level {
    #[default]
    Beginner,
    Intermediate,
    Expert,
}

impl Level {
    pub const ALL: [Self; 3] = [Self::Beginner, Self::Intermediate, Self::Expert];

    /// Columns, rows and mines.
    pub fn size(self) -> (usize, usize, usize) {
        match self {
            Self::Beginner => (9, 9, 10),
            Self::Intermediate => (16, 16, 40),
            Self::Expert => (30, 16, 99),
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Beginner => "Beginner",
            Self::Intermediate => "Intermediate",
            Self::Expert => "Expert",
        }
    }

    fn index(self) -> usize {
        match self {
            Self::Beginner => 0,
            Self::Intermediate => 1,
            Self::Expert => 2,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Record {
    pub seconds: u32,
    pub name: String,
}

/// What q0sweeper keeps between runs.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SweeperSettings {
    /// Found by knocking in the credits; until then it does not exist.
    pub unlocked: bool,
    pub level: Level,
    /// Right clicks cycle through a question mark too.
    pub marks: bool,
    /// Best time per level, beginner first.
    pub best: [Option<Record>; 3],
}

// --------------------------------------------------------------------- board

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cover {
    Hidden,
    Flag,
    Question,
    Open,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cell {
    pub mine: bool,
    /// Mines among the eight neighbours.
    pub around: u8,
    pub cover: Cover,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// No click yet; the mines are laid on the first one.
    Ready,
    Playing,
    Won,
    Lost,
}

/// The rules, with nothing on screen.
#[derive(Debug, Clone)]
pub struct Board {
    pub width: usize,
    pub height: usize,
    pub mines: usize,
    cells: Vec<Cell>,
    pub phase: Phase,
    /// The mine that went off.
    pub blown: Option<(usize, usize)>,
    seed: u64,
}

/// A cell that opened, and how many steps of flood it took to reach.
pub type Opened = ((usize, usize), u32);

impl Board {
    pub fn new(width: usize, height: usize, mines: usize, seed: u64) -> Self {
        let width = width.max(1);
        let height = height.max(1);
        Self {
            width,
            height,
            mines: mines.min(width * height - 1),
            cells: vec![
                Cell {
                    mine: false,
                    around: 0,
                    cover: Cover::Hidden,
                };
                width * height
            ],
            phase: Phase::Ready,
            blown: None,
            seed: seed | 1,
        }
    }

    pub fn cell(&self, x: usize, y: usize) -> Cell {
        self.cells[y * self.width + x]
    }

    fn cell_mut(&mut self, x: usize, y: usize) -> &mut Cell {
        &mut self.cells[y * self.width + x]
    }

    fn neighbours(&self, x: usize, y: usize) -> Vec<(usize, usize)> {
        let mut around = Vec::with_capacity(8);
        for dy in -1_i32..=1 {
            for dx in -1_i32..=1 {
                if dx == 0 && dy == 0 {
                    continue;
                }
                let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                if nx >= 0 && ny >= 0 && (nx as usize) < self.width && (ny as usize) < self.height {
                    around.push((nx as usize, ny as usize));
                }
            }
        }
        around
    }

    fn random(&mut self) -> u64 {
        // xorshift64*
        self.seed ^= self.seed >> 12;
        self.seed ^= self.seed << 25;
        self.seed ^= self.seed >> 27;
        self.seed.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Lays the mines anywhere but around the first click, so the first
    /// click always opens a patch.
    fn lay_mines(&mut self, safe_x: usize, safe_y: usize) {
        let near = |x: usize, y: usize| x.abs_diff(safe_x) <= 1 && y.abs_diff(safe_y) <= 1;
        let mut free = (0..self.height)
            .flat_map(|y| (0..self.width).map(move |x| (x, y)))
            .filter(|&(x, y)| !near(x, y))
            .collect::<Vec<_>>();
        if free.len() < self.mines {
            free = (0..self.height)
                .flat_map(|y| (0..self.width).map(move |x| (x, y)))
                .filter(|&(x, y)| (x, y) != (safe_x, safe_y))
                .collect();
        }
        for index in 0..self.mines.min(free.len()) {
            let pick = index + (self.random() % (free.len() - index) as u64) as usize;
            free.swap(index, pick);
            let (x, y) = free[index];
            self.cell_mut(x, y).mine = true;
        }
        for y in 0..self.height {
            for x in 0..self.width {
                let around = self
                    .neighbours(x, y)
                    .into_iter()
                    .filter(|&(nx, ny)| self.cell(nx, ny).mine)
                    .count() as u8;
                self.cell_mut(x, y).around = around;
            }
        }
    }

    pub fn flags(&self) -> usize {
        self.cells
            .iter()
            .filter(|cell| cell.cover == Cover::Flag)
            .count()
    }

    pub fn finished(&self) -> bool {
        matches!(self.phase, Phase::Won | Phase::Lost)
    }

    /// Opens (x, y), flooding outward from empty cells; returns what opened.
    pub fn open(&mut self, x: usize, y: usize) -> Vec<Opened> {
        if self.finished() || x >= self.width || y >= self.height {
            return Vec::new();
        }
        if !matches!(self.cell(x, y).cover, Cover::Hidden | Cover::Question) {
            return Vec::new();
        }
        if self.phase == Phase::Ready {
            self.lay_mines(x, y);
            self.phase = Phase::Playing;
        }
        if self.cell(x, y).mine {
            self.cell_mut(x, y).cover = Cover::Open;
            self.phase = Phase::Lost;
            self.blown = Some((x, y));
            return vec![((x, y), 0)];
        }
        let mut opened = Vec::new();
        let mut queue = std::collections::VecDeque::from([((x, y), 0_u32)]);
        self.cell_mut(x, y).cover = Cover::Open;
        while let Some(((cx, cy), ring)) = queue.pop_front() {
            opened.push(((cx, cy), ring));
            if self.cell(cx, cy).around != 0 {
                continue;
            }
            for (nx, ny) in self.neighbours(cx, cy) {
                let cell = self.cell(nx, ny);
                if !cell.mine && matches!(cell.cover, Cover::Hidden | Cover::Question) {
                    self.cell_mut(nx, ny).cover = Cover::Open;
                    queue.push_back(((nx, ny), ring + 1));
                }
            }
        }
        self.settle();
        opened
    }

    /// Clicking an open number whose flags are all placed opens the rest
    /// around it.
    pub fn chord(&mut self, x: usize, y: usize) -> Vec<Opened> {
        if self.finished() || x >= self.width || y >= self.height {
            return Vec::new();
        }
        let cell = self.cell(x, y);
        if cell.cover != Cover::Open || cell.around == 0 {
            return Vec::new();
        }
        let around = self.neighbours(x, y);
        let flags = around
            .iter()
            .filter(|&&(nx, ny)| self.cell(nx, ny).cover == Cover::Flag)
            .count();
        if flags != usize::from(cell.around) {
            return Vec::new();
        }
        let mut opened = Vec::new();
        for (nx, ny) in around {
            if matches!(self.cell(nx, ny).cover, Cover::Hidden | Cover::Question) {
                opened.extend(self.open(nx, ny));
                if self.phase == Phase::Lost {
                    break;
                }
            }
        }
        opened
    }

    /// Right click: a flag, then (with marks on) a question, then nothing.
    pub fn mark(&mut self, x: usize, y: usize, marks: bool) {
        if self.finished() || x >= self.width || y >= self.height {
            return;
        }
        let cell = self.cell_mut(x, y);
        cell.cover = match cell.cover {
            Cover::Hidden => Cover::Flag,
            Cover::Flag if marks => Cover::Question,
            Cover::Flag | Cover::Question => Cover::Hidden,
            Cover::Open => Cover::Open,
        };
    }

    /// Won once every safe cell is open; the mines then get their flags.
    fn settle(&mut self) {
        if self.phase != Phase::Playing {
            return;
        }
        if self
            .cells
            .iter()
            .all(|cell| cell.mine || cell.cover == Cover::Open)
        {
            self.phase = Phase::Won;
            for cell in &mut self.cells {
                if cell.mine {
                    cell.cover = Cover::Flag;
                }
            }
        }
    }
}

// ------------------------------------------------------------- secret knock

/// Knocks closer together than this belong to one six.
const KNOCK_GROUP_GAP: f32 = 0.42;
/// A longer breath than this between sixes starts over.
const KNOCK_MAX_PAUSE: f32 = 2.4;

/// The secret knock on the credits' "q0" mark: three sixes.
#[derive(Debug, Default)]
pub struct Knock {
    /// Knocks in the six going on now.
    group: u32,
    /// Sixes done; each grew a horn.
    sixes: u32,
    last: Option<Instant>,
    /// When each horn grew, for its little pop.
    grown_at: [Option<Instant>; 2],
    /// Recent knocks, for their ripples.
    ripples: Vec<Instant>,
    hover_since: Option<Instant>,
}

impl Knock {
    /// A knock; true when it completes the third six.
    pub fn knock(&mut self, now: Instant) -> bool {
        self.settle(now);
        self.group += 1;
        self.last = Some(now);
        self.ripples
            .retain(|at| now.duration_since(*at).as_secs_f32() < 0.4);
        self.ripples.push(now);
        if self.group > 6 {
            self.reset();
            return false;
        }
        if self.sixes == 2 && self.group == 6 {
            self.reset();
            return true;
        }
        false
    }

    /// Closes a six once the knocking pauses, and forgets a sequence left
    /// alone too long.
    pub fn settle(&mut self, now: Instant) {
        let Some(last) = self.last else {
            return;
        };
        let quiet = now.duration_since(last).as_secs_f32();
        if self.group > 0 && quiet > KNOCK_GROUP_GAP {
            if self.group == 6 && self.sixes < 2 {
                self.grown_at[self.sixes as usize] = Some(now);
                self.sixes += 1;
            } else {
                self.sixes = 0;
                self.grown_at = [None; 2];
            }
            self.group = 0;
        }
        if self.group == 0 && self.sixes > 0 && quiet > KNOCK_MAX_PAUSE {
            self.sixes = 0;
            self.grown_at = [None; 2];
        }
    }

    fn reset(&mut self) {
        self.group = 0;
        self.sixes = 0;
        self.grown_at = [None; 2];
    }

    fn busy(&self) -> bool {
        self.group > 0 || self.sixes > 0 || !self.ripples.is_empty()
    }
}

/// Listens for the knock on `mark`, the credits' "q0" square, and draws
/// its horns and ripples. True when the knock opens the game.
pub(crate) fn knock_on_mark(knock: &mut Knock, ui: &mut Ui, mark: Rect) -> bool {
    let now = Instant::now();
    let response = ui.interact(mark, Id::new("q0sweeper_knock"), Sense::click());
    knock.settle(now);
    let mut opened = false;
    if response.clicked() {
        opened = knock.knock(now);
    }
    if response.hovered() {
        let since = *knock.hover_since.get_or_insert(now);
        if now.duration_since(since) > Duration::from_secs(7) {
            response.on_hover_text("...knock. six, six, six.");
        } else {
            ui.ctx().request_repaint_after(Duration::from_millis(500));
        }
    } else {
        knock.hover_since = None;
    }
    let painter = ui.painter().clone();
    // Horns that grew from finished sixes.
    for (index, grown) in knock.grown_at.iter().enumerate() {
        let Some(grown) = grown else {
            continue;
        };
        let t = (now.duration_since(*grown).as_secs_f32() / 0.22).clamp(0.0, 1.0);
        let pop = 1.0 + (1.0 - t) * 0.6 * (t * std::f32::consts::PI).sin();
        let side = if index == 0 { -1.0 } else { 1.0 };
        let base = if index == 0 {
            mark.left_top() + vec2(7.0, 2.0)
        } else {
            mark.right_top() + vec2(-7.0, 2.0)
        };
        let height = 13.0 * t * pop;
        let tip = base + vec2(side * 5.0 * t, -height);
        painter.add(egui::Shape::convex_polygon(
            vec![base + vec2(-4.0, 0.0), tip, base + vec2(4.0, 0.0)],
            DEMON,
            Stroke::new(1.0_f32, DEMON_DARK),
        ));
    }
    for at in &knock.ripples {
        let t = now.duration_since(*at).as_secs_f32() / 0.35;
        if t < 1.0 {
            painter.rect_stroke(
                mark.expand(2.0 + 9.0 * t),
                9.0,
                Stroke::new(
                    2.0 * (1.0 - t),
                    Color32::from_rgba_unmultiplied(170, 90, 255, (200.0 * (1.0 - t)) as u8),
                ),
            );
        }
    }
    knock
        .ripples
        .retain(|at| now.duration_since(*at).as_secs_f32() < 0.35);
    if knock.busy() {
        ui.ctx().request_repaint_after(Duration::from_millis(16));
    }
    opened
}

// --------------------------------------------------------------------- game

/// A press on the field that has not been let go yet.
#[derive(Debug, Clone, Copy)]
struct Hold {
    /// Both buttons or the middle one: opens around a number.
    chord: bool,
    started: Instant,
    start_cell: Option<(usize, usize)>,
    /// A long touch already flagged; letting go does nothing more.
    marked: bool,
}

pub struct Sweeper {
    pub open: bool,
    level: Level,
    board: Board,
    started: Option<Instant>,
    /// Seconds on the clock when the game ended.
    final_seconds: Option<u32>,
    ended_at: Option<Instant>,
    /// When each cell's cover lifts on screen, for the ripple.
    shown_at: Vec<Option<Instant>>,
    hold: Option<Hold>,
    face_held: bool,
    booted_at: Instant,
    /// A best time waiting for its name.
    record_entry: Option<String>,
    best_times_open: bool,
    about_open: bool,
    /// The last press landed on the game window: F2 is the game's.
    focused: bool,
    pub knock: Knock,
}

impl Default for Sweeper {
    fn default() -> Self {
        Self::new(Level::Beginner)
    }
}

fn seed() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0x9e37_79b9, |since| since.as_nanos() as u64)
}

impl Sweeper {
    fn new(level: Level) -> Self {
        let (width, height, mines) = level.size();
        Self {
            open: false,
            level,
            board: Board::new(width, height, mines, seed()),
            started: None,
            final_seconds: None,
            ended_at: None,
            shown_at: vec![None; width * height],
            hold: None,
            face_held: false,
            booted_at: Instant::now(),
            record_entry: None,
            best_times_open: false,
            about_open: false,
            focused: false,
            knock: Knock::default(),
        }
    }

    /// Opens the window on a fresh game, with its power-on flicker.
    pub fn show(&mut self, level: Level) {
        self.restart(level);
        self.open = true;
        self.focused = true;
        self.booted_at = Instant::now();
    }

    fn restart(&mut self, level: Level) {
        let knock = std::mem::take(&mut self.knock);
        let (open, focused) = (self.open, self.focused);
        let dialogs = (self.best_times_open, self.about_open);
        *self = Self::new(level);
        self.knock = knock;
        (self.open, self.focused) = (open, focused);
        (self.best_times_open, self.about_open) = dialogs;
    }

    fn seconds(&self, now: Instant) -> u32 {
        if let Some(seconds) = self.final_seconds {
            return seconds;
        }
        self.started.map_or(0, |started| {
            (now.duration_since(started).as_secs() as u32 + 1).min(999)
        })
    }

    /// Plays out a click's result: the ripple of cells opening, the clock,
    /// and the ending.
    fn after(&mut self, opened: Vec<Opened>, now: Instant, settings: &SweeperSettings) {
        if self.started.is_none() && self.board.phase != Phase::Ready {
            self.started = Some(now);
        }
        for ((x, y), ring) in opened {
            let delay = Duration::from_millis(u64::from(ring.min(40)) * 14);
            self.shown_at[y * self.board.width + x] = Some(now + delay);
        }
        if self.board.finished() && self.ended_at.is_none() {
            self.ended_at = Some(now);
            self.final_seconds = Some(self.seconds(now));
            if self.board.phase == Phase::Won {
                let seconds = self.final_seconds.unwrap_or(999);
                let best = &settings.best[self.level.index()];
                if best.as_ref().is_none_or(|best| seconds < best.seconds) {
                    self.record_entry = Some(String::new());
                }
            }
        }
    }

    /// Whether the cover of (x, y) has lifted on screen yet.
    fn shows_open(&self, x: usize, y: usize, now: Instant) -> bool {
        self.board.cell(x, y).cover == Cover::Open
            && self.shown_at[y * self.board.width + x].is_none_or(|at| now >= at)
    }

    /// How far into the ending's reveal `(x, y)` is: mines of a lost game
    /// show one ring at a time from the one that went off.
    fn revealed_in_ending(&self, x: usize, y: usize, now: Instant) -> bool {
        let Some(ended) = self.ended_at else {
            return false;
        };
        let distance = match (self.board.phase, self.board.blown) {
            (Phase::Lost, Some((bx, by))) => x.abs_diff(bx).max(y.abs_diff(by)),
            _ => (x + y) / 2,
        };
        now >= ended + Duration::from_millis(distance.min(40) as u64 * 30)
    }

    fn animating(&self, now: Instant) -> bool {
        now.duration_since(self.booted_at) < Duration::from_millis(500)
            || self.shown_at.iter().flatten().any(|at| now < *at)
            || self
                .ended_at
                .is_some_and(|ended| now.duration_since(ended) < Duration::from_millis(1600))
    }
}

// ------------------------------------------------------------------ drawing

const GREY: Color32 = Color32::from_rgb(192, 192, 192);
const DARK: Color32 = Color32::from_rgb(128, 128, 128);
const LIGHT: Color32 = Color32::from_rgb(223, 223, 223);
const WHITE: Color32 = Color32::WHITE;
const BLACK: Color32 = Color32::BLACK;
const RED: Color32 = Color32::from_rgb(255, 0, 0);
const LED_OFF: Color32 = Color32::from_rgb(64, 0, 0);
const DEMON: Color32 = Color32::from_rgb(157, 78, 255);
const DEMON_DARK: Color32 = Color32::from_rgb(106, 43, 190);
const DEMON_FACE: Color32 = Color32::from_rgb(28, 12, 42);
const DEMON_LIGHT: Color32 = Color32::from_rgb(226, 196, 255);
const TITLE_LEFT: Color32 = Color32::from_rgb(0, 0, 128);
const TITLE_RIGHT: Color32 = Color32::from_rgb(16, 132, 208);

fn number_color(around: u8) -> Color32 {
    match around {
        1 => Color32::from_rgb(0, 0, 255),
        2 => Color32::from_rgb(0, 128, 0),
        3 => Color32::from_rgb(255, 0, 0),
        4 => Color32::from_rgb(0, 0, 128),
        5 => Color32::from_rgb(128, 0, 0),
        6 => Color32::from_rgb(0, 128, 128),
        7 => Color32::from_rgb(0, 0, 0),
        _ => Color32::from_rgb(128, 128, 128),
    }
}

/// The classic blocky digits, 8 by 10 art pixels.
const DIGITS: [[&str; 10]; 8] = [
    [
        "...##...", "..###...", ".####...", "...##...", "...##...", "...##...", "...##...",
        "...##...", ".######.", ".######.",
    ],
    [
        ".######.", "########", "##....##", ".....###", "...####.", ".####...", "###.....",
        "##......", "########", "########",
    ],
    [
        "#######.", "########", "......##", "......##", "..#####.", "..#####.", "......##",
        "......##", "########", "#######.",
    ],
    [
        "...####.", "..#####.", ".###.##.", "###..##.", "##...##.", "########", "########",
        ".....##.", ".....##.", ".....##.",
    ],
    [
        "########", "########", "##......", "##......", "#######.", "########", "......##",
        "......##", "########", "#######.",
    ],
    [
        ".######.", "########", "##......", "##......", "#######.", "########", "##....##",
        "##....##", "########", ".######.",
    ],
    [
        "########", "########", "......##", ".....###", "....###.", "...###..", "..###...",
        "..##....", "..##....", "..##....",
    ],
    [
        ".######.", "########", "##....##", "##....##", ".######.", ".######.", "##....##",
        "##....##", "########", ".######.",
    ],
];

/// 13 by 13: `#` black, `w` white shine.
const MINE: [&str; 13] = [
    "......#......",
    "......#......",
    "..#.#####.#..",
    "...#######...",
    "..##ww#####..",
    "..##ww#####..",
    "#############",
    "..#########..",
    "..#########..",
    "...#######...",
    "..#.#####.#..",
    "......#......",
    "......#......",
];

/// 10 by 11: `r` red, `#` black.
const FLAG: [&str; 11] = [
    ".....rr...",
    "...rrrr...",
    "..rrrrr...",
    "...rrrr...",
    ".....rr...",
    "......#...",
    "......#...",
    "....####..",
    "..########",
    "..########",
    "..........",
];

const QUESTION: [&str; 10] = [
    "..####..", ".##..##.", ".##..##.", ".....##.", "....##..", "...##...", "...##...", "........",
    "...##...", "...##...",
];

/// One of the mascot's little `q` lollipops, 8 by 10: `w` is its shine.
const LOLLIPOP: [&str; 10] = [
    "..####..", ".##ww###", "##w..###", "##....##", "##....##", ".##..###", "..######", "......##",
    "......##", "......##",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Face {
    Calm,
    Oh,
    Dead,
    Cool,
}

/// Paints in art pixels: `origin` is the top left on screen, `px` the size
/// of one art pixel in points.
#[derive(Clone, Copy)]
struct Canvas<'a> {
    painter: &'a Painter,
    origin: Pos2,
    px: f32,
}

impl Canvas<'_> {
    fn at(&self, x: f32, y: f32) -> Pos2 {
        pos2(self.origin.x + x * self.px, self.origin.y + y * self.px)
    }

    fn rect(&self, x: f32, y: f32, w: f32, h: f32, color: Color32) {
        if w <= 0.0 || h <= 0.0 {
            return;
        }
        self.painter.rect_filled(
            Rect::from_min_size(self.at(x, y), vec2(w * self.px, h * self.px)),
            0.0,
            color,
        );
    }

    /// A bevel `depth` pixels deep: `light` on the top and left, `dark` on
    /// the bottom and right, meeting on the diagonal.
    #[allow(clippy::too_many_arguments)]
    fn bevel(&self, x: f32, y: f32, w: f32, h: f32, depth: f32, light: Color32, dark: Color32) {
        for i in 0..depth as i32 {
            let i = i as f32;
            self.rect(x, y + i, w - i, 1.0, light);
            self.rect(x + i, y, 1.0, h - i, light);
            self.rect(x + i + 1.0, y + h - 1.0 - i, w - i - 1.0, 1.0, dark);
            self.rect(x + w - 1.0 - i, y + i + 1.0, 1.0, h - i - 1.0, dark);
        }
    }

    fn raised(&self, x: f32, y: f32, w: f32, h: f32, depth: f32) {
        self.bevel(x, y, w, h, depth, WHITE, DARK);
    }

    fn sunken(&self, x: f32, y: f32, w: f32, h: f32, depth: f32) {
        self.bevel(x, y, w, h, depth, DARK, WHITE);
    }

    fn sprite(&self, x: f32, y: f32, rows: &[&str], color: impl Fn(char) -> Option<Color32>) {
        for (row, line) in rows.iter().enumerate() {
            // Runs of one colour paint as one rectangle.
            let chars = line.chars().collect::<Vec<_>>();
            let mut column = 0;
            while column < chars.len() {
                let Some(paint) = color(chars[column]) else {
                    column += 1;
                    continue;
                };
                let start = column;
                while column < chars.len() && color(chars[column]) == Some(paint) {
                    column += 1;
                }
                self.rect(
                    x + start as f32,
                    y + row as f32,
                    (column - start) as f32,
                    1.0,
                    paint,
                );
            }
        }
    }

    /// One 7-segment LED digit, 13 by 23; `None` is a minus sign.
    fn led_digit(&self, x: f32, y: f32, digit: Option<u8>) {
        const SEGMENTS: [[bool; 7]; 10] = [
            // a, b, c, d, e, f, g
            [true, true, true, true, true, true, false],
            [false, true, true, false, false, false, false],
            [true, true, false, true, true, false, true],
            [true, true, true, true, false, false, true],
            [false, true, true, false, false, true, true],
            [true, false, true, true, false, true, true],
            [true, false, true, true, true, true, true],
            [true, true, true, false, false, false, false],
            [true, true, true, true, true, true, true],
            [true, true, true, true, false, true, true],
        ];
        let lit = match digit {
            Some(digit) => SEGMENTS[usize::from(digit.min(9))],
            None => [false, false, false, false, false, false, true],
        };
        let paint = |on: bool| if on { RED } else { LED_OFF };
        // Horizontal bars taper by a pixel at each end, row by row.
        let horizontal = |row: f32, rows: [(f32, f32); 3], on: bool| {
            for (i, (from, to)) in rows.iter().enumerate() {
                self.rect(
                    x + from,
                    y + row + i as f32,
                    to - from + 1.0,
                    1.0,
                    paint(on),
                );
            }
        };
        let vertical = |column: f32, rows: [(f32, f32); 3], step: f32, on: bool| {
            for (i, (from, to)) in rows.iter().enumerate() {
                self.rect(
                    x + column + step * i as f32,
                    y + from,
                    1.0,
                    to - from + 1.0,
                    paint(on),
                );
            }
        };
        horizontal(1.0, [(2.0, 10.0), (3.0, 9.0), (4.0, 8.0)], lit[0]);
        horizontal(10.0, [(3.0, 9.0), (2.0, 10.0), (3.0, 9.0)], lit[6]);
        horizontal(19.0, [(4.0, 8.0), (3.0, 9.0), (2.0, 10.0)], lit[3]);
        let upper = [(2.0, 10.0), (3.0, 9.0), (4.0, 8.0)];
        let lower = [(12.0, 20.0), (13.0, 19.0), (14.0, 18.0)];
        vertical(1.0, upper, 1.0, lit[5]);
        vertical(11.0, upper, -1.0, lit[1]);
        vertical(1.0, lower, 1.0, lit[4]);
        vertical(11.0, lower, -1.0, lit[2]);
    }

    /// A sunken three-digit LED counter, 41 by 25.
    fn led(&self, x: f32, y: f32, value: i32, lit: bool) {
        self.sunken(x, y, 41.0, 25.0, 1.0);
        self.rect(x + 1.0, y + 1.0, 39.0, 23.0, BLACK);
        let value = value.clamp(-99, 999);
        let digits: [Option<u8>; 3] = if value < 0 {
            let v = value.unsigned_abs();
            [None, Some((v / 10 % 10) as u8), Some((v % 10) as u8)]
        } else {
            let v = value as u32;
            [
                Some((v / 100 % 10) as u8),
                Some((v / 10 % 10) as u8),
                Some((v % 10) as u8),
            ]
        };
        for (index, digit) in digits.into_iter().enumerate() {
            if lit {
                self.led_digit(x + 1.0 + index as f32 * 13.0, y + 1.0, digit);
            } else {
                self.led_digit(x + 1.0 + index as f32 * 13.0, y + 1.0, Some(8));
                // Dark, as if switched off for a blink.
                self.rect(
                    x + 1.0 + index as f32 * 13.0,
                    y + 1.0,
                    13.0,
                    23.0,
                    Color32::from_rgba_unmultiplied(0, 0, 0, 200),
                );
            }
        }
    }

    /// The q0editor demon, 20 by 20, wearing `face`.
    fn demon(&self, x: f32, y: f32, face: Face) {
        for py in 0..20 {
            for px in 0..20 {
                if let Some(color) = demon_pixel(px, py, face) {
                    self.rect(x + px as f32, y + py as f32, 1.0, 1.0, color);
                }
            }
        }
    }
}

/// One pixel of the demon: a purple ring of a head with horns, a heart on
/// its forehead, and the face of the moment.
fn demon_pixel(x: i32, y: i32, face: Face) -> Option<Color32> {
    let mirror = if x >= 10 { 19 - x } else { x };
    // Horns, tip first.
    let horn = match y {
        0 => mirror == 2,
        1 => (2..=3).contains(&mirror),
        2 => (2..=4).contains(&mirror),
        3 => (3..=5).contains(&mirror),
        4 => (3..=6).contains(&mirror),
        _ => false,
    };
    let (cx, cy) = (10.0, 10.5);
    let (fx, fy) = (x as f32 + 0.5 - cx, y as f32 + 0.5 - cy);
    let outer = (fx / 8.6).powi(2) + (fy / 8.2).powi(2) <= 1.0;
    let inner = (fx / 6.4).powi(2) + (fy / 6.0).powi(2) <= 1.0;
    if horn && !inner {
        return Some(DEMON);
    }
    if !outer {
        return None;
    }
    if !inner {
        return Some(if y >= 13 { DEMON_DARK } else { DEMON });
    }
    // The heart on the forehead.
    let heart = match y {
        6 => mirror == 8,
        7 => (8..=9).contains(&mirror),
        8 => mirror == 9,
        _ => false,
    };
    if heart {
        return Some(DEMON);
    }
    // A glint on the left lens of the sunglasses.
    if face == Face::Cool && (x, y) == (6, 11) {
        return Some(WHITE);
    }
    let feature = match face {
        Face::Calm => match y {
            10 | 11 => mirror == 6 || mirror == 7,
            13 => mirror == 6,
            14 => (7..=9).contains(&mirror),
            _ => false,
        },
        Face::Oh => match y {
            9..=11 => mirror == 6 || mirror == 7,
            13 | 15 => mirror == 9,
            14 => mirror == 8,
            _ => false,
        },
        Face::Dead => match y {
            9 | 11 => mirror == 5 || mirror == 7,
            10 => mirror == 6,
            13 => (7..=9).contains(&mirror),
            14 => mirror == 6,
            _ => false,
        },
        Face::Cool => match y {
            10 => (4..=9).contains(&mirror),
            11 => (5..=7).contains(&mirror),
            13 => mirror == 6,
            14 => (7..=9).contains(&mirror),
            _ => false,
        },
    };
    if feature {
        return Some(match (face, y) {
            (Face::Cool, 10 | 11) => BLACK,
            _ => DEMON_LIGHT,
        });
    }
    Some(DEMON_FACE)
}

// ------------------------------------------------------------------ window

/// F2 deals a new game while the game has the focus. Taken before the rest
/// of the editor looks at the keys, since the library renames on F2.
pub(crate) fn take_keys(app: &mut EditorApp, ctx: &Context) {
    if app.sweeper.open
        && app.sweeper.focused
        && ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::F2))
    {
        let level = app.sweeper.level;
        app.sweeper.restart(level);
    }
}

/// The old grey look, for the game window, its menus and its dialogs.
fn vintage_style(base: &egui::Style) -> egui::Style {
    let mut style = base.clone();
    let mut visuals = egui::Visuals::light();
    visuals.window_fill = GREY;
    visuals.panel_fill = GREY;
    visuals.faint_bg_color = GREY;
    visuals.extreme_bg_color = WHITE;
    visuals.window_rounding = 0.0.into();
    visuals.menu_rounding = 0.0.into();
    visuals.window_shadow = egui::epaint::Shadow::NONE;
    visuals.popup_shadow = egui::epaint::Shadow::NONE;
    visuals.window_stroke = Stroke::new(1.0_f32, BLACK);
    visuals.selection.bg_fill = TITLE_LEFT;
    visuals.selection.stroke = Stroke::new(1.0_f32, WHITE);
    let widgets = &mut visuals.widgets;
    for (state, fill, text) in [
        (&mut widgets.noninteractive, GREY, BLACK),
        (&mut widgets.inactive, GREY, BLACK),
        (&mut widgets.hovered, TITLE_LEFT, WHITE),
        (&mut widgets.active, TITLE_LEFT, WHITE),
        (&mut widgets.open, TITLE_LEFT, WHITE),
    ] {
        state.weak_bg_fill = fill;
        state.bg_fill = if fill == GREY { WHITE } else { fill };
        state.fg_stroke = Stroke::new(1.0_f32, text);
        state.bg_stroke = Stroke::new(1.0_f32, DARK);
        state.rounding = 0.0.into();
        state.expansion = 0.0;
    }
    widgets.noninteractive.bg_fill = GREY;
    style.visuals = visuals;
    style
}

/// The window, while it is open.
pub(crate) fn render_window(app: &mut EditorApp, ctx: &Context) {
    if !app.settings.sweeper.unlocked || !app.sweeper.open {
        return;
    }
    let now = Instant::now();
    let editor_style = ctx.style();
    ctx.set_style(vintage_style(&editor_style));
    render_windows(app, ctx, now);
    ctx.set_style(editor_style);
    if app.sweeper.animating(now) {
        ctx.request_repaint();
    } else if app.sweeper.board.phase == Phase::Playing {
        ctx.request_repaint_after(Duration::from_millis(200));
    }
}

fn render_windows(app: &mut EditorApp, ctx: &Context, now: Instant) {
    let (columns, rows) = (app.sweeper.board.width, app.sweeper.board.height);
    let art = vec2(16.0 * columns as f32 + 24.0, 16.0 * rows as f32 + 67.0);
    let ppp = ctx.pixels_per_point();
    let screen = ctx.screen_rect();
    let fits = ((screen.width() - 40.0) * ppp / art.x).min((screen.height() - 140.0) * ppp / art.y);
    let physical = fits.floor().clamp(1.0, (2.0 * ppp).round().max(1.0));
    let px = physical / ppp;
    let mut open = true;
    let frame = egui::Frame::none()
        .fill(GREY)
        .inner_margin(egui::Margin::same(3.0 * px));
    let shown = egui::Window::new("q0sweeper")
        .id(Id::new("q0sweeper_window"))
        .title_bar(false)
        .resizable(false)
        .collapsible(false)
        .frame(frame)
        .default_pos(screen.center() - art * px * 0.5)
        .show(ctx, |ui| {
            // Nothing in here may keep the window wider than the board, or
            // it would not shrink back after an expert game.
            ui.set_max_width(art.x * px);
            ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
            title_bar(ui, art.x * px, &mut open);
            menu_bar(app, ui);
            game(app, ui, now, art, px);
            record_prompt(app, ui);
        });
    if let Some(shown) = shown {
        let rect = shown.response.rect;
        // The raised frame every window had then.
        let painter = ctx.layer_painter(shown.response.layer_id);
        let frame = Canvas {
            painter: &painter,
            origin: rect.min,
            px,
        };
        let (w, h) = (rect.width() / px, rect.height() / px);
        frame.bevel(0.0, 0.0, w, h, 1.0, LIGHT, BLACK);
        frame.bevel(1.0, 1.0, w - 2.0, h - 2.0, 1.0, WHITE, DARK);
        let pressed = ctx.input(|input| {
            input
                .pointer
                .any_pressed()
                .then(|| input.pointer.interact_pos())
                .flatten()
        });
        if let Some(pressed) = pressed {
            app.sweeper.focused = rect.contains(pressed);
        }
    }
    if !open {
        app.sweeper.open = false;
    }
    best_times_window(app, ctx);
    about_window(app, ctx);
}

/// The blue title bar of the old days, with its little close box.
fn title_bar(ui: &mut Ui, width: f32, open: &mut bool) {
    let (rect, _) = ui.allocate_exact_size(vec2(width, 20.0), Sense::hover());
    let painter = ui.painter_at(rect);
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(rect.left_top(), TITLE_LEFT);
    mesh.colored_vertex(rect.right_top(), TITLE_RIGHT);
    mesh.colored_vertex(rect.right_bottom(), TITLE_RIGHT);
    mesh.colored_vertex(rect.left_bottom(), TITLE_LEFT);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    painter.add(egui::Shape::mesh(mesh));
    let icon = Canvas {
        painter: &painter,
        origin: rect.left_top() + vec2(3.0, 2.0),
        px: 0.8,
    };
    icon.demon(0.0, 0.0, Face::Calm);
    painter.text(
        rect.left_center() + vec2(22.0, 0.0),
        egui::Align2::LEFT_CENTER,
        "q0sweeper",
        egui::FontId::proportional(13.0),
        WHITE,
    );
    let close = Rect::from_min_size(rect.right_top() + vec2(-19.0, 3.0), vec2(16.0, 14.0));
    let response = ui.interact(close, Id::new("q0sweeper_close"), Sense::click());
    let canvas = Canvas {
        painter: &painter,
        origin: close.min,
        px: 1.0,
    };
    canvas.rect(0.0, 0.0, 16.0, 14.0, GREY);
    if response.is_pointer_button_down_on() {
        canvas.sunken(0.0, 0.0, 16.0, 14.0, 1.0);
    } else {
        canvas.raised(0.0, 0.0, 16.0, 14.0, 1.0);
    }
    for i in 0..6 {
        let i = i as f32;
        canvas.rect(4.0 + i, 3.0 + i, 2.0, 1.0, BLACK);
        canvas.rect(9.0 - i, 3.0 + i, 2.0, 1.0, BLACK);
    }
    if response.clicked() {
        *open = false;
    }
}

fn menu_bar(app: &mut EditorApp, ui: &mut Ui) {
    egui::menu::bar(ui, |ui| {
        ui.spacing_mut().item_spacing = vec2(8.0, 4.0);
        ui.menu_button("Game", |ui| {
            if ui
                .add(egui::Button::new("New").shortcut_text("F2"))
                .clicked()
            {
                let level = app.sweeper.level;
                app.sweeper.restart(level);
                ui.close_menu();
            }
            ui.separator();
            for level in Level::ALL {
                if ui
                    .radio(app.sweeper.level == level, level.label())
                    .clicked()
                {
                    app.settings.sweeper.level = level;
                    app.settings.save();
                    app.sweeper.restart(level);
                    ui.close_menu();
                }
            }
            ui.separator();
            let mut marks = app.settings.sweeper.marks;
            if ui.checkbox(&mut marks, "Marks (?)").clicked() {
                app.settings.sweeper.marks = marks;
                app.settings.save();
                ui.close_menu();
            }
            ui.separator();
            if ui.button("Best Times...").clicked() {
                app.sweeper.best_times_open = true;
                ui.close_menu();
            }
            ui.separator();
            if ui.button("Exit").clicked() {
                app.sweeper.open = false;
                ui.close_menu();
            }
        });
        ui.menu_button("Help", |ui| {
            if ui.button("About q0sweeper...").clicked() {
                app.sweeper.about_open = true;
                ui.close_menu();
            }
        });
    });
}

/// The board itself: counters, the demon, the field, and every click.
fn game(app: &mut EditorApp, ui: &mut Ui, now: Instant, art: egui::Vec2, px: f32) {
    let ppp = ui.ctx().pixels_per_point();
    let (rect, response) = ui.allocate_exact_size(art * px, Sense::click_and_drag());
    let origin = pos2(
        (rect.min.x * ppp).round() / ppp,
        (rect.min.y * ppp).round() / ppp,
    );
    let marks = app.settings.sweeper.marks;
    let sweeper = &mut app.sweeper;
    let (columns, rows) = (sweeper.board.width, sweeper.board.height);

    // A little shudder when a mine goes off.
    let shake = match (sweeper.board.phase, sweeper.ended_at) {
        (Phase::Lost, Some(ended)) => {
            let t = now.duration_since(ended).as_secs_f32();
            if t < 0.3 {
                ((t * 90.0).sin() * 2.0 * (1.0 - t / 0.3)).round()
            } else {
                0.0
            }
        }
        _ => 0.0,
    };
    let to_art = |point: Pos2| ((point.x - origin.x) / px, (point.y - origin.y) / px);
    let cell_at = |point: Pos2| -> Option<(usize, usize)> {
        let (x, y) = to_art(point);
        let (cx, cy) = ((x - 12.0 - shake) / 16.0, (y - 55.0) / 16.0);
        (cx >= 0.0 && cy >= 0.0 && (cx as usize) < columns && (cy as usize) < rows)
            .then_some((cx as usize, cy as usize))
    };
    let face_x = (art.x - 26.0) / 2.0;
    let on_face = |point: Pos2| {
        let (x, y) = to_art(point);
        (face_x..face_x + 26.0).contains(&x) && (15.0..41.0).contains(&y)
    };

    // ---------------------------------------------------------------- input
    let (pointer, primary_down, secondary_down, primary_pressed, secondary_pressed) =
        ui.input(|input| {
            (
                input.pointer.interact_pos(),
                input.pointer.button_down(egui::PointerButton::Primary),
                input.pointer.button_down(egui::PointerButton::Secondary),
                input.pointer.button_pressed(egui::PointerButton::Primary),
                input.pointer.button_pressed(egui::PointerButton::Secondary),
            )
        });
    let (middle_pressed, primary_released, secondary_released, middle_released) =
        ui.input(|input| {
            (
                input.pointer.button_pressed(egui::PointerButton::Middle),
                input.pointer.button_released(egui::PointerButton::Primary),
                input
                    .pointer
                    .button_released(egui::PointerButton::Secondary),
                input.pointer.button_released(egui::PointerButton::Middle),
            )
        });
    let inside = pointer.is_some_and(|point| rect.contains(point)) && response.hovered();
    let under = pointer.and_then(cell_at);
    let playable = !sweeper.board.finished() && sweeper.record_entry.is_none();
    if primary_pressed && inside {
        if pointer.is_some_and(on_face) {
            sweeper.face_held = true;
        } else if playable && under.is_some() {
            sweeper.hold = Some(Hold {
                chord: secondary_down,
                started: now,
                start_cell: under,
                marked: false,
            });
        }
    }
    if secondary_pressed && inside && playable {
        if let Some(hold) = sweeper.hold.as_mut() {
            hold.chord = true;
        } else if let Some((x, y)) = under {
            sweeper.board.mark(x, y, marks);
        }
    }
    if middle_pressed && inside && playable && under.is_some() {
        sweeper.hold = Some(Hold {
            chord: true,
            started: now,
            start_cell: under,
            marked: false,
        });
    }
    // On a touch screen a long press plants a flag.
    if crate::touch::ENABLED {
        if let Some(hold) = sweeper.hold.as_mut() {
            if !hold.chord
                && !hold.marked
                && primary_down
                && under == hold.start_cell
                && now.duration_since(hold.started) > Duration::from_millis(380)
            {
                if let Some((x, y)) = under {
                    if sweeper.board.cell(x, y).cover != Cover::Open {
                        sweeper.board.mark(x, y, marks);
                        hold.marked = true;
                    }
                }
            }
            if !hold.marked {
                ui.ctx().request_repaint_after(Duration::from_millis(50));
            }
        }
    }
    if sweeper.face_held && primary_released {
        if pointer.is_some_and(on_face) {
            let level = sweeper.level;
            sweeper.restart(level);
        }
        sweeper.face_held = false;
    }
    if let Some(hold) = sweeper.hold {
        let let_go = primary_released || middle_released || (hold.chord && secondary_released);
        if let_go {
            sweeper.hold = None;
            if !hold.marked {
                if let Some((x, y)) = under {
                    let opened = if hold.chord || sweeper.board.cell(x, y).cover == Cover::Open {
                        sweeper.board.chord(x, y)
                    } else {
                        sweeper.board.open(x, y)
                    };
                    sweeper.after(opened, now, &app.settings.sweeper);
                }
            }
        } else if !primary_down && !secondary_down {
            sweeper.hold = None;
        }
    }
    let sweeper = &app.sweeper;

    // -------------------------------------------------------------- drawing
    let painter = ui.painter_at(rect);
    let canvas = Canvas {
        painter: &painter,
        origin,
        px,
    };
    canvas.rect(0.0, 0.0, art.x, art.y, GREY);
    canvas.raised(0.0, 0.0, art.x, art.y, 3.0);
    // The panel with the counters and the demon.
    canvas.sunken(9.0, 9.0, art.x - 18.0, 37.0, 2.0);
    let mines_left = sweeper.board.mines as i32 - sweeper.board.flags() as i32;
    canvas.led(16.0, 15.0, mines_left, true);
    let seconds = sweeper.seconds(now);
    // The clock blinks a few times on a win.
    let blink = sweeper.board.phase == Phase::Won
        && sweeper.ended_at.is_some_and(|ended| {
            let t = now.duration_since(ended).as_secs_f32();
            t < 1.2 && ((t / 0.15) as i32) % 2 == 1
        });
    canvas.led(art.x - 16.0 - 41.0, 15.0, seconds as i32, !blink);
    // The mascot's two little lollipops float beside the button.
    let bob = |phase: f32| {
        let t = now.duration_since(sweeper.booted_at).as_secs_f32() * 2.2 + phase;
        let won = sweeper.board.phase == Phase::Won;
        let height = if won { 3.0 } else { 1.0 };
        -((t.sin() * 0.5 + 0.5) * height).round()
    };
    let lollipop = |c: char| match c {
        '#' => Some(DEMON),
        'w' => Some(DEMON_LIGHT),
        _ => None,
    };
    canvas.sprite(face_x - 11.0, 23.0 + bob(0.0), &LOLLIPOP, lollipop);
    canvas.sprite(face_x + 29.0, 23.0 + bob(1.7), &LOLLIPOP, lollipop);
    let face = if sweeper.board.phase == Phase::Lost {
        Face::Dead
    } else if sweeper.board.phase == Phase::Won {
        Face::Cool
    } else if sweeper.hold.is_some() && playable {
        Face::Oh
    } else {
        Face::Calm
    };
    canvas.rect(face_x, 15.0, 26.0, 26.0, DARK);
    canvas.rect(face_x + 1.0, 16.0, 24.0, 24.0, GREY);
    let pressed = sweeper.face_held && pointer.is_some_and(on_face);
    if pressed {
        canvas.rect(face_x + 1.0, 16.0, 24.0, 1.0, DARK);
        canvas.rect(face_x + 1.0, 16.0, 1.0, 24.0, DARK);
        canvas.demon(face_x + 3.0, 19.0, face);
    } else {
        canvas.raised(face_x + 1.0, 16.0, 24.0, 24.0, 2.0);
        canvas.demon(face_x + 2.0, 18.0, face);
    }

    // The field.
    canvas.sunken(
        9.0,
        52.0,
        16.0 * columns as f32 + 6.0,
        16.0 * rows as f32 + 6.0,
        3.0,
    );
    let field = Canvas {
        painter: &painter,
        origin: canvas.at(shake, 0.0),
        px,
    };
    let pressed_cells = match (sweeper.hold, under) {
        (Some(hold), Some((x, y))) if !hold.marked => {
            let mut cells = vec![(x, y)];
            if hold.chord || sweeper.board.cell(x, y).cover == Cover::Open {
                cells.extend(sweeper.board.neighbours(x, y));
            }
            cells
        }
        _ => Vec::new(),
    };
    for y in 0..rows {
        for x in 0..columns {
            let (cx, cy) = (12.0 + 16.0 * x as f32, 55.0 + 16.0 * y as f32);
            let cell = sweeper.board.cell(x, y);
            let ending = sweeper.board.finished() && sweeper.revealed_in_ending(x, y, now);
            let open_now = sweeper.shows_open(x, y, now);
            let lost = sweeper.board.phase == Phase::Lost;
            let shows_mine = lost && cell.mine && cell.cover != Cover::Flag && ending;
            let wrong_flag = lost && !cell.mine && cell.cover == Cover::Flag && ending;
            let blown = sweeper.board.blown == Some((x, y));
            let pressed_here = pressed_cells.contains(&(x, y))
                && matches!(cell.cover, Cover::Hidden | Cover::Question);
            if (open_now && !cell.mine) || shows_mine || wrong_flag || blown || pressed_here {
                field.rect(cx, cy, 16.0, 16.0, if blown { RED } else { GREY });
                field.rect(cx, cy, 16.0, 1.0, DARK);
                field.rect(cx, cy, 1.0, 16.0, DARK);
                if blown || shows_mine || wrong_flag {
                    field.sprite(cx + 2.0, cy + 2.0, &MINE, |c| match c {
                        '#' => Some(BLACK),
                        'w' => Some(WHITE),
                        _ => None,
                    });
                    if wrong_flag {
                        for i in 0..11 {
                            let i = i as f32;
                            field.rect(cx + 3.0 + i, cy + 3.0 + i, 1.5, 1.0, RED);
                            field.rect(cx + 13.0 - i, cy + 3.0 + i, 1.5, 1.0, RED);
                        }
                    }
                } else if open_now && cell.around > 0 {
                    let color = number_color(cell.around);
                    field.sprite(
                        cx + 4.0,
                        cy + 3.0,
                        &DIGITS[usize::from(cell.around - 1)],
                        |c| (c == '#').then_some(color),
                    );
                }
                continue;
            }
            field.rect(cx, cy, 16.0, 16.0, GREY);
            field.raised(cx, cy, 16.0, 16.0, 2.0);
            // A won game plants its flags one after another.
            let flag_shows = cell.cover == Cover::Flag
                && (sweeper.board.phase != Phase::Won || !cell.mine || ending);
            if flag_shows {
                field.sprite(cx + 3.0, cy + 3.0, &FLAG, |c| match c {
                    'r' => Some(RED),
                    '#' => Some(BLACK),
                    _ => None,
                });
            } else if cell.cover == Cover::Question {
                field.sprite(cx + 4.0, cy + 3.0, &QUESTION, |c| {
                    (c == '#').then_some(BLACK)
                });
            }
        }
    }

    // A power-on wipe, the way an old screen woke up.
    let boot = now.duration_since(sweeper.booted_at).as_secs_f32() / 0.45;
    if boot < 1.0 {
        let line = (boot * art.y).round();
        canvas.rect(0.0, line, art.x, art.y - line, Color32::from_rgb(8, 8, 8));
        canvas.rect(0.0, line, art.x, 1.0, Color32::from_rgb(200, 255, 200));
    }
}

/// "You have the fastest time" with its name box, as it used to ask.
fn record_prompt(app: &mut EditorApp, ui: &mut Ui) {
    let Some(name) = app.sweeper.record_entry.as_mut() else {
        return;
    };
    let level = app.sweeper.level;
    let mut done = false;
    egui::Frame::none()
        .fill(GREY)
        .inner_margin(egui::Margin::same(10.0))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
            ui.label(format!(
                "You have the fastest time for {} level. Please type your name:",
                level.label().to_lowercase()
            ));
            let field = ui.add(egui::TextEdit::singleline(name).desired_width(180.0));
            let entered =
                field.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
            if ui.button("OK").clicked() || entered {
                done = true;
            } else if !field.has_focus() {
                field.request_focus();
            }
        });
    if done {
        let name = app.sweeper.record_entry.take().unwrap_or_default();
        let name = if name.trim().is_empty() {
            "Anonymous".to_string()
        } else {
            name.trim().chars().take(32).collect()
        };
        if let Some(seconds) = app.sweeper.final_seconds {
            app.settings.sweeper.best[level.index()] = Some(Record { seconds, name });
            app.settings.save();
            app.sweeper.best_times_open = true;
        }
    }
}

fn vintage_window<'a>(title: &'a str, open: &'a mut bool) -> egui::Window<'a> {
    egui::Window::new(title)
        .open(open)
        .collapsible(false)
        .resizable(false)
}

fn best_times_window(app: &mut EditorApp, ctx: &Context) {
    let mut open = app.sweeper.best_times_open;
    let mut reset = false;
    vintage_window("Fastest Mine Sweepers", &mut open).show(ctx, |ui| {
        egui::Grid::new("q0sweeper_best")
            .spacing([18.0, 6.0])
            .show(ui, |ui| {
                for level in Level::ALL {
                    ui.label(format!("{}:", level.label()));
                    match &app.settings.sweeper.best[level.index()] {
                        Some(record) => {
                            ui.label(format!("{} seconds", record.seconds));
                            ui.label(&record.name);
                        }
                        None => {
                            ui.label("999 seconds");
                            ui.label("Anonymous");
                        }
                    }
                    ui.end_row();
                }
            });
        ui.add_space(6.0);
        if ui.button("Reset Scores").clicked() {
            reset = true;
        }
    });
    if reset {
        app.settings.sweeper.best = Default::default();
        app.settings.save();
    }
    app.sweeper.best_times_open = open;
}

fn about_window(app: &mut EditorApp, ctx: &Context) {
    let mut open = app.sweeper.about_open;
    vintage_window("About q0sweeper", &mut open).show(ctx, |ui| {
        let (rect, _) = ui.allocate_exact_size(vec2(40.0, 40.0), Sense::hover());
        let painter = ui.painter_at(rect);
        Canvas {
            painter: &painter,
            origin: rect.min,
            px: 2.0,
        }
        .demon(0.0, 0.0, Face::Cool);
        ui.label("q0sweeper");
        ui.label("You knocked six times, six times, six times.");
        ui.label("Left click opens, right click flags. Both, or the middle");
        ui.label("button, opens around a number whose flags are all placed.");
    });
    app.sweeper.about_open = open;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open_count(board: &Board) -> usize {
        (0..board.height)
            .flat_map(|y| (0..board.width).map(move |x| (x, y)))
            .filter(|&(x, y)| board.cell(x, y).cover == Cover::Open)
            .count()
    }

    #[test]
    fn the_first_click_is_always_safe_and_opens_a_patch() {
        for seed in 1..200 {
            let mut board = Board::new(9, 9, 10, seed);
            let opened = board.open(4, 4);
            assert_ne!(board.phase, Phase::Lost, "seed {seed}");
            assert!(
                opened.len() >= 9,
                "a zero opens its whole ring: seed {seed}"
            );
            assert_eq!(opened.len(), open_count(&board), "seed {seed}");
            let mines = (0..9)
                .flat_map(|y| (0..9).map(move |x| (x, y)))
                .filter(|&(x, y)| board.cell(x, y).mine)
                .count();
            assert_eq!(mines, 10);
        }
    }

    #[test]
    fn the_numbers_count_their_neighbouring_mines() {
        let mut board = Board::new(16, 16, 40, 7);
        board.open(0, 0);
        for y in 0..16 {
            for x in 0..16 {
                let expected = board
                    .neighbours(x, y)
                    .into_iter()
                    .filter(|&(nx, ny)| board.cell(nx, ny).mine)
                    .count() as u8;
                assert_eq!(board.cell(x, y).around, expected);
            }
        }
    }

    #[test]
    fn a_flood_stops_at_numbers_and_never_opens_a_flag() {
        let mut board = Board::new(30, 16, 99, 42);
        board.open(15, 8);
        for y in 0..16 {
            for x in 0..30 {
                let cell = board.cell(x, y);
                if cell.cover == Cover::Open {
                    assert!(!cell.mine);
                }
            }
        }
        // Every open zero has all its neighbours open.
        for y in 0..16 {
            for x in 0..30 {
                let cell = board.cell(x, y);
                if cell.cover == Cover::Open && cell.around == 0 {
                    for (nx, ny) in board.neighbours(x, y) {
                        assert_eq!(board.cell(nx, ny).cover, Cover::Open);
                    }
                }
            }
        }
        let mut flagged = Board::new(9, 9, 10, 3);
        flagged.open(4, 4);
        let hidden = (0..9)
            .flat_map(|y| (0..9).map(move |x| (x, y)))
            .find(|&(x, y)| flagged.cell(x, y).cover == Cover::Hidden && !flagged.cell(x, y).mine);
        if let Some((x, y)) = hidden {
            flagged.mark(x, y, false);
            flagged.open(x, y);
            assert_eq!(
                flagged.cell(x, y).cover,
                Cover::Flag,
                "a flag guards its cell"
            );
        }
    }

    #[test]
    fn marks_cycle_through_flag_question_and_back() {
        let mut board = Board::new(9, 9, 10, 1);
        board.mark(0, 0, true);
        assert_eq!(board.cell(0, 0).cover, Cover::Flag);
        assert_eq!(board.flags(), 1);
        board.mark(0, 0, true);
        assert_eq!(board.cell(0, 0).cover, Cover::Question);
        board.mark(0, 0, true);
        assert_eq!(board.cell(0, 0).cover, Cover::Hidden);
        board.mark(0, 0, false);
        board.mark(0, 0, false);
        assert_eq!(
            board.cell(0, 0).cover,
            Cover::Hidden,
            "no question without marks"
        );
        assert_eq!(board.phase, Phase::Ready, "flags do not start the game");
    }

    #[test]
    fn opening_every_safe_cell_wins_and_flags_the_mines() {
        let mut board = Board::new(9, 9, 10, 9);
        board.open(4, 4);
        for y in 0..9 {
            for x in 0..9 {
                if !board.cell(x, y).mine {
                    board.open(x, y);
                }
            }
        }
        assert_eq!(board.phase, Phase::Won);
        assert_eq!(board.flags(), 10);
    }

    #[test]
    fn a_mine_ends_the_game_and_a_chord_opens_around_a_finished_number() {
        let mut board = Board::new(9, 9, 10, 5);
        board.open(4, 4);
        // Find an open number, flag its mines, and chord it.
        let number = (0..9)
            .flat_map(|y| (0..9).map(move |x| (x, y)))
            .find(|&(x, y)| {
                let cell = board.cell(x, y);
                cell.cover == Cover::Open && cell.around > 0
            })
            .expect("an open number");
        for (nx, ny) in board.neighbours(number.0, number.1) {
            if board.cell(nx, ny).mine && board.cell(nx, ny).cover != Cover::Flag {
                board.mark(nx, ny, false);
            }
        }
        board.chord(number.0, number.1);
        assert_ne!(board.phase, Phase::Lost, "correct flags make a safe chord");
        for (nx, ny) in board.neighbours(number.0, number.1) {
            let cell = board.cell(nx, ny);
            assert!(cell.mine || cell.cover == Cover::Open);
        }
        let mine = (0..9)
            .flat_map(|y| (0..9).map(move |x| (x, y)))
            .find(|&(x, y)| board.cell(x, y).mine && board.cell(x, y).cover != Cover::Flag);
        if let Some((x, y)) = mine {
            board.open(x, y);
            assert_eq!(board.phase, Phase::Lost);
            assert_eq!(board.blown, Some((x, y)));
        }
    }

    #[test]
    fn three_sixes_of_knocks_open_the_game_and_a_slip_starts_over() {
        let start = Instant::now();
        let mut knock = Knock::default();
        let mut at = start;
        let knock_six = |knock: &mut Knock, at: &mut Instant| {
            let mut opened = false;
            for _ in 0..6 {
                *at += Duration::from_millis(150);
                opened |= knock.knock(*at);
            }
            *at += Duration::from_millis(800);
            knock.settle(*at);
            opened
        };
        assert!(!knock_six(&mut knock, &mut at));
        assert_eq!(knock.sixes, 1, "a horn grows");
        assert!(!knock_six(&mut knock, &mut at));
        assert_eq!(knock.sixes, 2);
        assert!(knock_six(&mut knock, &mut at), "the third six opens it");

        // Five knocks instead of six take the horns away.
        let mut knock = Knock::default();
        let mut at = start;
        assert!(!knock_six(&mut knock, &mut at));
        for _ in 0..5 {
            at += Duration::from_millis(150);
            knock.knock(at);
        }
        at += Duration::from_millis(800);
        knock.settle(at);
        assert_eq!(knock.sixes, 0);

        // Waiting too long between sixes starts over too.
        let mut knock = Knock::default();
        let mut at = start;
        knock_six(&mut knock, &mut at);
        at += Duration::from_secs(4);
        knock.settle(at);
        assert_eq!(knock.sixes, 0);
    }

    #[test]
    fn the_demon_is_symmetric_and_every_face_differs() {
        let picture = |face: Face| {
            (0..20)
                .flat_map(|y| (0..20).map(move |x| demon_pixel(x, y, face)))
                .collect::<Vec<_>>()
        };
        for y in 0..20 {
            for x in 0..20 {
                assert_eq!(
                    demon_pixel(x, y, Face::Calm),
                    demon_pixel(19 - x, y, Face::Calm),
                    "{x},{y}"
                );
            }
        }
        let faces = [Face::Calm, Face::Oh, Face::Dead, Face::Cool].map(picture);
        for a in 0..4 {
            for b in a + 1..4 {
                assert_ne!(faces[a], faces[b]);
            }
        }
    }
}
