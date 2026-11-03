//! room chat and direct messages: what arrived, what you are typing, and the
//! game-style overlay that shows new lines over the canvas even while the
//! online panel is closed.
use super::*;
use egui::{vec2, Align2, Color32, RichText};
use std::collections::VecDeque;

/// Room lines kept in memory; the server keeps more.
const KEEP_ROOM_LINES: usize = 200;
/// Lines the overlay remembers for its expanded view.
const KEEP_FEED: usize = 60;
/// How long a new line stays over the canvas, and how long it takes to fade.
const SHOW_FOR: Duration = Duration::from_secs(10);
const FADE_FOR: Duration = Duration::from_millis(1600);
/// Lines over the canvas while you are not typing.
const OVERLAY_LINES: usize = 6;
/// Lines over the canvas while the chat input is open.
const OVERLAY_HISTORY: usize = 14;
pub(super) const MAX_ROOM_CHARS: usize = 400;
pub(super) const MAX_DM_CHARS: usize = 2000;

#[derive(Clone, Debug, Deserialize)]
pub struct ChatMessage {
    pub id: i64,
    pub user_id: i64,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub created_at: i64,
}
#[derive(Clone, Debug, Deserialize)]
pub(super) struct DirectMessage {
    pub(super) id: i64,
    pub(super) from: i64,
    pub(super) to: i64,
    #[serde(default)]
    pub(super) body: String,
    #[serde(default)]
    pub(super) created_at: i64,
}
impl DirectMessage {
    pub(super) fn other(&self, me: Option<i64>) -> i64 {
        if Some(self.from) == me {
            self.to
        } else {
            self.from
        }
    }
}
#[derive(Clone, Debug, Deserialize)]
pub(super) struct Conversation {
    pub(super) user_id: i64,
    #[serde(default)]
    pub(super) last: Option<DirectMessage>,
    #[serde(default)]
    pub(super) unread: i64,
    #[serde(default)]
    pub(super) blocked: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum LineKind {
    Room,
    /// A direct message to or from the person in `user_id`.
    DmIn,
    DmOut,
    /// Something the overlay says itself, such as a message that failed.
    Notice,
}
#[derive(Clone, Debug)]
pub(super) struct Line {
    kind: LineKind,
    user_id: i64,
    body: String,
    at: Instant,
}

#[derive(Default)]
pub(super) struct Chat {
    /// This room's conversation, oldest first.
    pub(super) room: Vec<ChatMessage>,
    /// The recent conversation arrives with the first poll; it fills the
    /// panel's history but never floods the overlay.
    room_primed: bool,
    /// Everything new, for the overlay: room lines, messages and notices.
    pub(super) feed: VecDeque<Line>,
    pub(super) input_open: bool,
    focus_input: bool,
    pub(super) draft: String,
    /// Who `/r` answers: the last person who messaged you.
    reply_to: Option<i64>,
    pub(super) conversations: Vec<Conversation>,
    pub(super) conversations_loaded: bool,
    pub(super) unread: i64,
    pub(super) threads: HashMap<i64, Vec<DirectMessage>>,
    /// The conversation open in the panel's Messages tab.
    pub(super) thread_with: Option<i64>,
    pub(super) blocked_with: BTreeSet<i64>,
    pub(super) dm_draft: String,
    pub(super) room_draft: String,
    pub(super) find: String,
    /// The conversation list was asked for and has not arrived yet.
    pub(super) requested: bool,
}

impl Chat {
    pub(super) fn left_room(&mut self) {
        self.room.clear();
        self.room_primed = false;
        self.feed.retain(|line| line.kind != LineKind::Room);
    }
    fn push(&mut self, kind: LineKind, user_id: i64, body: impl Into<String>) {
        self.feed.push_back(Line {
            kind,
            user_id,
            body: body.into(),
            at: Instant::now(),
        });
        while self.feed.len() > KEEP_FEED {
            self.feed.pop_front();
        }
    }
    pub(super) fn notice(&mut self, text: impl Into<String>) {
        self.push(LineKind::Notice, 0, text);
    }
    /// The newest message the overlay should still show, for repaint timing.
    fn visible_until(&self) -> Option<Instant> {
        self.feed
            .back()
            .map(|line| line.at + SHOW_FOR + FADE_FOR)
            .filter(|until| *until > Instant::now())
    }
}

/// Single-line text for room chat: line breaks become spaces.
pub(super) fn room_body(text: &str) -> Option<String> {
    let body = text
        .split(['\n', '\r'])
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let body: String = body
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_ROOM_CHARS)
        .collect();
    (!body.is_empty()).then_some(body)
}
pub(super) fn dm_body(text: &str) -> Option<String> {
    let body: String = text
        .trim()
        .replace("\r\n", "\n")
        .chars()
        .filter(|c| *c == '\n' || !c.is_control())
        .take(MAX_DM_CHARS)
        .collect();
    (!body.trim().is_empty()).then_some(body)
}

/// What the overlay input means: game chat conventions.
#[derive(Debug, PartialEq, Eq)]
enum Typed {
    Room(String),
    Reply(String),
    Whisper(String, String),
}
fn parse_typed(text: &str) -> Option<Typed> {
    let text = text.trim();
    let command = |names: &[&str]| {
        names.iter().find_map(|name| {
            text.strip_prefix(name)
                .filter(|rest| rest.is_empty() || rest.starts_with(' '))
                .map(str::trim_start)
        })
    };
    if let Some(rest) = command(&["/r", "/reply"]) {
        return Some(Typed::Reply(rest.to_owned()));
    }
    if let Some(rest) = command(&["/w", "/msg", "/dm", "/whisper"]) {
        let (name, body) = rest.split_once(' ').unwrap_or((rest, ""));
        return Some(Typed::Whisper(
            name.trim_start_matches('@').to_owned(),
            body.trim().to_owned(),
        ));
    }
    (!text.is_empty()).then(|| Typed::Room(text.to_owned()))
}

/// A steady colour per person, readable on dark backgrounds or, with
/// `dark` false, on light ones.
pub(super) fn name_colour(user_id: i64, dark: bool) -> Color32 {
    let hue = ((user_id.unsigned_abs().wrapping_mul(2654435761) % 360) as f32) / 360.0;
    let (saturation, value) = if dark { (0.55, 1.0) } else { (0.75, 0.55) };
    egui::ecolor::Hsva::new(hue, saturation, value, 1.0).into()
}

impl Online {
    /// The chat key: the overlay input in a room or after a message, the
    /// panel's Messages tab otherwise. Signed out it does nothing, so the key
    /// never surprises someone who doesn't use online.
    pub fn open_chat(&mut self) {
        if self.user.is_none() {
            return;
        }
        if self.room.is_none() && self.chat.reply_to.is_none() {
            self.open = true;
            self.view.tab = ui::Tab::Messages;
            return;
        }
        self.chat.input_open = true;
        self.chat.focus_input = true;
    }
    pub(super) fn room_chat_arrived(&mut self, room: &str, mut lines: Vec<ChatMessage>) {
        if !self.room.as_ref().is_some_and(|session| session.id == room) {
            return;
        }
        lines.retain(|line| !self.chat.room.iter().any(|known| known.id == line.id));
        lines.sort_by_key(|line| line.id);
        let primed = self.chat.room_primed;
        self.chat.room_primed = true;
        for line in &lines {
            if primed {
                self.chat
                    .push(LineKind::Room, line.user_id, line.body.clone());
            }
        }
        self.chat.room.extend(lines);
        self.chat.room.sort_by_key(|line| line.id);
        let extra = self.chat.room.len().saturating_sub(KEEP_ROOM_LINES);
        self.chat.room.drain(..extra);
    }
    pub(super) fn dm_arrived(&mut self, message: DirectMessage, announce: bool) {
        let me = self.actor();
        let other = message.other(me);
        let incoming = Some(message.to) == me && message.from != message.to;
        if let Some(thread) = self.chat.threads.get_mut(&other) {
            if !thread.iter().any(|known| known.id == message.id) {
                thread.push(message.clone());
                thread.sort_by_key(|m| m.id);
            }
        }
        match self
            .chat
            .conversations
            .iter_mut()
            .find(|c| c.user_id == other)
        {
            Some(row) => {
                if row.last.as_ref().is_none_or(|last| last.id < message.id) {
                    row.last = Some(message.clone());
                }
            }
            None => self.chat.conversations.push(Conversation {
                user_id: other,
                last: Some(message.clone()),
                unread: 0,
                blocked: false,
            }),
        }
        self.chat
            .conversations
            .sort_by_key(|c| std::cmp::Reverse(c.last.as_ref().map_or(0, |m| m.id)));
        if incoming {
            self.chat.reply_to = Some(other);
            let reading = self.open
                && self.view.tab == ui::Tab::Messages
                && self.chat.thread_with == Some(other);
            if reading {
                self.send(Command::ReadDm { user_id: other });
            } else if let Some(row) = self
                .chat
                .conversations
                .iter_mut()
                .find(|c| c.user_id == other)
            {
                row.unread += 1;
            }
        }
        if announce {
            let kind = if incoming {
                LineKind::DmIn
            } else {
                LineKind::DmOut
            };
            self.chat.push(kind, other, message.body);
        }
    }
    pub(super) fn handle_chat(&mut self, reply: Reply) {
        match reply {
            Reply::ChatSent { room, message } => {
                if let Ok(line) = serde_json::from_value::<ChatMessage>(message) {
                    self.room_chat_arrived(&room, vec![line]);
                }
            }
            Reply::ChatFailed(text) => {
                self.chat.notice(text.clone());
                self.say(text, Tone::Error);
            }
            Reply::Inbox(value) => {
                self.learn(
                    value
                        .get("people")
                        .and_then(Value::as_object)
                        .into_iter()
                        .flat_map(|people| people.values())
                        .filter_map(|p| serde_json::from_value::<Person>(p.clone()).ok()),
                );
                for message in rows::<DirectMessage>(&value, "messages") {
                    self.dm_arrived(message, true);
                }
                if let Some(unread) = value.get("unread").and_then(Value::as_i64) {
                    self.chat.unread = unread;
                }
            }
            Reply::Conversations(value) => {
                self.learn(
                    value
                        .get("people")
                        .and_then(Value::as_object)
                        .into_iter()
                        .flat_map(|people| people.values())
                        .filter_map(|p| serde_json::from_value::<Person>(p.clone()).ok()),
                );
                self.chat.conversations = rows(&value, "conversations");
                self.chat.conversations_loaded = true;
                self.chat.requested = false;
                self.chat.blocked_with = self
                    .chat
                    .conversations
                    .iter()
                    .filter(|c| c.blocked)
                    .map(|c| c.user_id)
                    .collect();
                if let Some(unread) = value.get("unread").and_then(Value::as_i64) {
                    self.chat.unread = unread;
                }
            }
            Reply::Thread { user_id, value } => {
                self.learn(
                    value
                        .get("people")
                        .and_then(Value::as_object)
                        .into_iter()
                        .flat_map(|people| people.values())
                        .filter_map(|p| serde_json::from_value::<Person>(p.clone()).ok()),
                );
                let messages = rows::<DirectMessage>(&value, "messages");
                if value.get("blocked").and_then(Value::as_bool) == Some(true) {
                    self.chat.blocked_with.insert(user_id);
                } else {
                    self.chat.blocked_with.remove(&user_id);
                }
                // opening a conversation reads it
                if let Some(row) = self
                    .chat
                    .conversations
                    .iter_mut()
                    .find(|c| c.user_id == user_id)
                {
                    self.chat.unread = (self.chat.unread - row.unread).max(0);
                    row.unread = 0;
                }
                self.chat.threads.insert(user_id, messages);
            }
            Reply::DmSent(message) => {
                if let Ok(message) = serde_json::from_value::<DirectMessage>(message) {
                    self.dm_arrived(message, false);
                }
            }
            _ => {}
        }
    }
    pub(super) fn send_room_chat(&mut self, text: &str) -> bool {
        let Some(room) = self.room.as_ref().map(|room| room.id.clone()) else {
            self.chat
                .notice("you’re not in a room. /w name message writes to someone directly");
            return false;
        };
        let Some(body) = room_body(text) else {
            return false;
        };
        self.send(Command::Say { room, body });
        true
    }
    pub(super) fn send_dm(&mut self, user_id: i64, text: &str) -> bool {
        let Some(body) = dm_body(text) else {
            return false;
        };
        self.chat.reply_to.get_or_insert(user_id);
        self.send(Command::SendDm { user_id, body });
        true
    }
    pub(super) fn open_thread(&mut self, user_id: i64) {
        self.chat.thread_with = Some(user_id);
        self.chat.dm_draft.clear();
        self.send(Command::Thread { user_id });
    }
    fn submit_overlay(&mut self) {
        let text = std::mem::take(&mut self.chat.draft);
        match parse_typed(&text) {
            None => {}
            Some(Typed::Room(body)) => {
                if self.room.is_some() {
                    self.send_room_chat(&body);
                } else if let Some(to) = self.chat.reply_to {
                    self.send_dm(to, &body);
                } else {
                    self.chat
                        .notice("you’re not in a room. /w name message writes to someone directly");
                }
            }
            Some(Typed::Reply(body)) => match self.chat.reply_to {
                Some(to) if !body.is_empty() => {
                    self.send_dm(to, &body);
                }
                Some(_) => self.chat.notice("/r needs a message after it"),
                None => self.chat.notice("nobody has messaged you yet"),
            },
            Some(Typed::Whisper(name, body)) => {
                let found = self
                    .people
                    .values()
                    .find(|p| p.username.eq_ignore_ascii_case(&name))
                    .map(|p| p.id);
                match found {
                    _ if name.is_empty() => self.chat.notice("/w name message"),
                    Some(id) if Some(id) == self.actor() => self.chat.notice("that’s you"),
                    Some(_) if body.is_empty() => {
                        self.chat.notice("/w needs a message after the name")
                    }
                    Some(id) => {
                        self.send_dm(id, &body);
                    }
                    None => self.chat.notice(format!(
                        "q0editor doesn’t know “{name}” yet. find them under Online → Messages"
                    )),
                }
            }
        }
    }

    /// Recent chat over the canvas, like a game: new lines show for a few
    /// seconds and fade; the chat key opens an input with the history above.
    pub(super) fn show_overlay(&mut self, app: &EditorApp, ctx: &Context) {
        if self.user.is_none() {
            self.chat.input_open = false;
            return;
        }
        let canvas = ctx
            .data(|d| d.get_temp::<egui::Rect>(egui::Id::new(CANVAS_RECT)))
            .filter(|rect| rect.is_positive())
            .unwrap_or_else(|| ctx.screen_rect().shrink(40.0));
        let width = (canvas.width() - 24.0).clamp(160.0, 460.0);
        let anchor = canvas.left_bottom() + vec2(12.0, -12.0);
        let now = Instant::now();
        let typing = self.chat.input_open;
        let lines: Vec<(Line, f32)> = if typing {
            let skip = self.chat.feed.len().saturating_sub(OVERLAY_HISTORY);
            self.chat
                .feed
                .iter()
                .skip(skip)
                .map(|l| (l.clone(), 1.0))
                .collect()
        } else {
            let fresh: Vec<_> = self
                .chat
                .feed
                .iter()
                .filter_map(|line| {
                    let age = now.saturating_duration_since(line.at);
                    if age >= SHOW_FOR + FADE_FOR {
                        return None;
                    }
                    let fade = if age <= SHOW_FOR {
                        1.0
                    } else {
                        1.0 - (age - SHOW_FOR).as_secs_f32() / FADE_FOR.as_secs_f32()
                    };
                    Some((line.clone(), fade))
                })
                .collect();
            let skip = fresh.len().saturating_sub(OVERLAY_LINES);
            fresh.into_iter().skip(skip).collect()
        };
        if let Some(until) = self.chat.visible_until() {
            // keep fading without input; a frame a tenth of a second is plenty
            ctx.request_repaint_after(
                until
                    .saturating_duration_since(now)
                    .min(Duration::from_millis(100)),
            );
        }
        let accent = app.settings.theme.accent.to_color32();
        let me = self.actor();
        let in_room = self.room.is_some();
        let mut submit = false;
        let mut close = false;
        let mut open = false;
        let room_name = self.room.as_ref().map(|room| room.name.clone());
        if !lines.is_empty() || typing {
            egui::Area::new(egui::Id::new("q0editor_chat_overlay"))
                .order(egui::Order::Foreground)
                .pivot(Align2::LEFT_BOTTOM)
                .fixed_pos(anchor)
                .interactable(typing)
                .show(ctx, |ui| {
                    ui.set_max_width(width);
                    ui.spacing_mut().item_spacing.y = 3.0;
                    if !typing {
                        for (line, fade) in &lines {
                            self.overlay_line(ui, line, *fade, me, accent, width, true);
                        }
                        return;
                    }
                    // the open chat: a dark console, the same on every theme
                    let white = Color32::from_gray(236);
                    let dim = Color32::from_gray(150);
                    egui::Frame::none()
                        .fill(Color32::from_rgba_unmultiplied(12, 12, 16, 222))
                        .stroke(egui::Stroke::new(1.0_f32, Color32::from_white_alpha(24)))
                        .rounding(10.0)
                        .inner_margin(egui::Margin::same(10.0))
                        .show(ui, |ui| {
                            ui.set_width(width - 20.0);
                            ui.horizontal(|ui| {
                                let (dot, _) =
                                    ui.allocate_exact_size(vec2(8.0, 8.0), egui::Sense::hover());
                                ui.painter().circle_filled(dot.center(), 4.0, accent);
                                let (title, place) = match &room_name {
                                    Some(name) => ("ROOM CHAT", name.clone()),
                                    None => ("MESSAGES", String::new()),
                                };
                                ui.label(RichText::new(title).small().strong().color(white));
                                ui.label(RichText::new(place).small().color(dim));
                            });
                            ui.add_space(2.0);
                            if lines.is_empty() {
                                ui.label(
                                    RichText::new(if in_room {
                                        "nothing said yet. say hi"
                                    } else {
                                        "no messages yet"
                                    })
                                    .italics()
                                    .color(dim),
                                );
                            }
                            egui::ScrollArea::vertical()
                                .max_height(280.0)
                                .stick_to_bottom(true)
                                .show(ui, |ui| {
                                    for (line, _) in &lines {
                                        self.overlay_line(
                                            ui,
                                            line,
                                            1.0,
                                            me,
                                            accent,
                                            width - 20.0,
                                            false,
                                        );
                                    }
                                });
                            ui.add_space(6.0);
                            let edit = ui
                                .horizontal(|ui| {
                                    ui.label(RichText::new("›").strong().size(18.0).color(accent));
                                    let visuals = ui.visuals_mut();
                                    visuals.extreme_bg_color = Color32::from_black_alpha(170);
                                    visuals.override_text_color = Some(white);
                                    visuals.selection.stroke = egui::Stroke::new(1.0_f32, accent);
                                    visuals.widgets.inactive.bg_stroke =
                                        egui::Stroke::new(1.0_f32, Color32::from_white_alpha(30));
                                    visuals.widgets.hovered.bg_stroke =
                                        egui::Stroke::new(1.0_f32, Color32::from_white_alpha(60));
                                    let hint = if in_room {
                                        "say something · /r reply · /w name text"
                                    } else {
                                        "reply · /w name text"
                                    };
                                    ui.add(
                                        egui::TextEdit::singleline(&mut self.chat.draft)
                                            .id_source("q0editor_chat_input")
                                            .hint_text(RichText::new(hint).color(dim))
                                            .char_limit(MAX_DM_CHARS)
                                            .margin(vec2(8.0, 5.0))
                                            .desired_width(f32::INFINITY),
                                    )
                                })
                                .inner;
                            if self.chat.focus_input {
                                // after the widget: the key that opened chat
                                // never lands in the text
                                edit.request_focus();
                                self.chat.focus_input = false;
                            } else if edit.lost_focus() {
                                if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                                    submit = true;
                                }
                                close = true;
                            }
                            ui.label(
                                RichText::new("Enter sends · Esc closes")
                                    .small()
                                    .color(Color32::from_gray(115)),
                            );
                        });
                });
        }
        // a quiet way in while you're in a room and nothing is showing
        if in_room && !typing && lines.is_empty() {
            let shortcut = app
                .settings
                .hotkeys
                .shortcuts(crate::hotkeys::ShortcutCommand::OpenChat)
                .first()
                .map(|chord| chord.display());
            egui::Area::new(egui::Id::new("q0editor_chat_pill"))
                .order(egui::Order::Foreground)
                .pivot(Align2::LEFT_BOTTOM)
                .fixed_pos(anchor)
                .show(ctx, |ui| {
                    let text = match &shortcut {
                        Some(key) => format!("chat  {key}"),
                        None => "chat".into(),
                    };
                    let pill = egui::Button::new(
                        RichText::new(text).small().color(Color32::from_gray(225)),
                    )
                    .fill(Color32::from_black_alpha(90))
                    .stroke(egui::Stroke::new(1.0_f32, Color32::from_white_alpha(28)))
                    .rounding(10.0);
                    if ui
                        .add(pill)
                        .on_hover_text("Talk to everyone in the room")
                        .clicked()
                    {
                        open = true;
                    }
                });
        }
        if submit {
            self.submit_overlay();
        }
        if close {
            self.chat.input_open = false;
        }
        if open {
            self.open_chat();
        }
    }

    /// One chat line, painted: a stripe in the speaker's colour, the name,
    /// the text, and over the canvas a dark backing so it reads on any art.
    #[allow(clippy::too_many_arguments)]
    fn overlay_line(
        &self,
        ui: &mut egui::Ui,
        line: &Line,
        fade: f32,
        me: Option<i64>,
        accent: Color32,
        width: f32,
        backed: bool,
    ) {
        let fade = fade.clamp(0.0, 1.0);
        let alpha = |c: Color32| c.gamma_multiply(fade);
        let mut job = egui::text::LayoutJob::default();
        let font = egui::TextStyle::Body.resolve(ui.style());
        let mut part = |text: &str, color: Color32, background: Color32| {
            job.append(
                text,
                0.0,
                egui::TextFormat {
                    font_id: font.clone(),
                    color: alpha(color),
                    background: alpha(background),
                    ..Default::default()
                },
            );
        };
        let none = Color32::TRANSPARENT;
        let white = Color32::from_gray(240);
        let soft = Color32::from_gray(140);
        let whisper = Color32::from_rgb(255, 208, 232);
        let stripe = match line.kind {
            LineKind::Room => {
                let name = if Some(line.user_id) == me {
                    "you".to_owned()
                } else {
                    self.person(line.user_id).username
                };
                part(&name, name_colour(line.user_id, true), none);
                part("  ", soft, none);
                part(&line.body, white, none);
                name_colour(line.user_id, true)
            }
            LineKind::DmIn | LineKind::DmOut => {
                let name = self.person(line.user_id).username;
                part(" DM ", crate::theme::on_accent(accent), accent);
                part(
                    if line.kind == LineKind::DmIn {
                        "  from "
                    } else {
                        "  to "
                    },
                    soft,
                    none,
                );
                part(&name, name_colour(line.user_id, true), none);
                part("  ", soft, none);
                part(&line.body, whisper, none);
                accent
            }
            LineKind::Notice => {
                let amber = Color32::from_rgb(255, 196, 120);
                part(&line.body, amber, none);
                amber
            }
        };
        let pad = vec2(9.0, 4.0);
        let bar = 3.0;
        job.wrap.max_width = (width - pad.x * 2.0 - bar).max(40.0);
        let galley = ui.fonts(|fonts| fonts.layout_job(job));
        let size = galley.size() + pad * 2.0 + vec2(bar, 0.0);
        let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
        if !ui.is_rect_visible(rect) {
            return;
        }
        let painter = ui.painter();
        if backed {
            let shadow = rect.translate(vec2(0.0, 2.0)).expand(1.0);
            painter.rect_filled(shadow, 8.0, Color32::from_black_alpha((50.0 * fade) as u8));
            let fill = if matches!(line.kind, LineKind::DmIn | LineKind::DmOut) {
                // a whisper glows in the theme colour
                let mix = |dark: u8, tint: u8| {
                    (f32::from(dark) + (f32::from(tint) - f32::from(dark)) * 0.22) as u8
                };
                Color32::from_rgb(
                    mix(14, accent.r()),
                    mix(10, accent.g()),
                    mix(16, accent.b()),
                )
            } else {
                Color32::from_rgb(12, 12, 16)
            };
            painter.rect_filled(rect, 7.0, alpha(fill.gamma_multiply(0.86)));
        }
        let bar_rect = egui::Rect::from_min_size(
            rect.min + vec2(0.0, if backed { 0.0 } else { 3.0 }),
            vec2(bar, rect.height() - if backed { 0.0 } else { 6.0 }),
        );
        painter.rect_filled(
            bar_rect,
            egui::Rounding {
                nw: 7.0,
                sw: 7.0,
                ne: 0.0,
                se: 0.0,
            },
            alpha(stripe),
        );
        painter.galley(rect.min + vec2(bar, 0.0) + pad, galley, white);
    }
}

/// Where the stage panel drew the canvas this frame; the overlay sits on it.
pub(crate) const CANVAS_RECT: &str = "q0editor_canvas_rect";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_lines_follow_game_chat_conventions() {
        assert_eq!(parse_typed("  hi all "), Some(Typed::Room("hi all".into())));
        assert_eq!(
            parse_typed("/r  thanks"),
            Some(Typed::Reply("thanks".into()))
        );
        assert_eq!(
            parse_typed("/w @Ekar look at layer 3"),
            Some(Typed::Whisper("Ekar".into(), "look at layer 3".into()))
        );
        assert_eq!(
            parse_typed("/msg walibite"),
            Some(Typed::Whisper("walibite".into(), String::new()))
        );
        // a word that only starts like a command is just text
        assert_eq!(
            parse_typed("/rainbow"),
            Some(Typed::Room("/rainbow".into()))
        );
        assert_eq!(parse_typed("   "), None);
    }

    #[test]
    fn bodies_are_trimmed_and_bounded() {
        assert_eq!(room_body(" a\r\n b \n"), Some("a b".into()));
        assert_eq!(room_body(" \n "), None);
        assert_eq!(room_body(&"x".repeat(900)).unwrap().len(), MAX_ROOM_CHARS);
        assert_eq!(dm_body(" two\r\nlines "), Some("two\nlines".into()));
        assert_eq!(dm_body("\u{7}"), None);
    }

    fn signed_in() -> Online {
        Online {
            user: Some(Person {
                id: 1,
                username: "me".into(),
                ..Person::default()
            }),
            ..Online::default()
        }
    }

    #[test]
    fn the_first_room_page_is_history_and_later_lines_show_over_the_canvas() {
        let mut online = signed_in();
        online.room = Some(RoomSession {
            id: "room".into(),
            name: "room".into(),
            owner_id: 1,
            tab_id: 1,
            revision: 0,
            confirmed: crate::state::default_project(),
            members: Vec::new(),
            policy: RoomPolicy::default(),
            presence: Vec::new(),
            invites: Vec::new(),
            pending: None,
            events: BTreeMap::new(),
            local_revision: 0,
            changed_at: None,
            history: None,
            last_presence: Instant::now(),
            sent_presence: None,
            sync_job: None,
            rejection: None,
            read_only: None,
        });
        let line = |id: i64| ChatMessage {
            id,
            user_id: 2,
            body: format!("line {id}"),
            created_at: 0,
        };
        online.room_chat_arrived("room", vec![line(2), line(1)]);
        assert_eq!(
            online.chat.room.iter().map(|l| l.id).collect::<Vec<_>>(),
            [1, 2]
        );
        assert!(
            online.chat.feed.is_empty(),
            "history must not flood the overlay"
        );
        online.room_chat_arrived("room", vec![line(2), line(3)]);
        assert_eq!(online.chat.room.len(), 3, "duplicates are dropped");
        assert_eq!(online.chat.feed.len(), 1);
        // another room's lines are ignored
        online.room_chat_arrived("elsewhere", vec![line(9)]);
        assert_eq!(online.chat.room.len(), 3);
        online.leave();
        assert!(online.chat.room.is_empty() && online.chat.feed.is_empty());
    }

    #[test]
    fn incoming_messages_count_as_unread_and_set_the_reply_target() {
        let mut online = signed_in();
        let dm = |id: i64, from: i64, to: i64| DirectMessage {
            id,
            from,
            to,
            body: "hey".into(),
            created_at: 0,
        };
        online.dm_arrived(dm(1, 7, 1), true);
        online.dm_arrived(dm(2, 1, 8), true);
        assert_eq!(online.chat.reply_to, Some(7));
        assert_eq!(online.chat.conversations[0].user_id, 8, "newest first");
        assert_eq!(online.chat.conversations[1].unread, 1);
        assert_eq!(
            online.chat.conversations[0].unread, 0,
            "your own message is read"
        );
        assert_eq!(online.chat.feed.len(), 2);
        online.chat.draft = "/r ok".into();
        online.submit_overlay();
        assert!(online.chat.draft.is_empty());
    }
}
