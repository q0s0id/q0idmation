//! background transport for the desktop online panel. credentials live only in memory.
use super::{EventBatch, RoomSnapshot};
use egui::Context;
use q0online::{Member, OperationRequest, Rank, RoomEvent, RoomPolicy};
use q0s_format::v2::ProjectV2;
use serde_json::{json, Value};
use std::io::Read;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    mpsc, Arc, Mutex,
};
use std::time::{Duration, Instant};

const MAX_RESPONSE: u64 = 128 * 1024 * 1024;
const MAX_AVATAR: u64 = 2 * 1024 * 1024;
const AVATAR_SIZE: u32 = 64;
const AVATAR_WORKERS: usize = 3;
#[derive(Clone, Default)]
struct Credentials {
    access: String,
    refresh: String,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FriendAction {
    Request,
    Accept,
    Decline,
    /// Removes a friendship or cancels an outgoing request.
    Remove,
    Block,
    Unblock,
}
#[derive(Clone)]
pub enum Command {
    Login,
    Logout,
    LeaveRoom,
    Refresh,
    Search {
        query: String,
    },
    Create {
        name: String,
        // boxed: one whole project would size every command on the channel
        project: Box<ProjectV2>,
    },
    Join {
        id: String,
    },
    Submit {
        room: String,
        request: OperationRequest,
    },
    History {
        room: String,
        operation_id: String,
        redo: bool,
    },
    Invite {
        room: String,
        user_id: i64,
        rank: Rank,
    },
    RoomInvites {
        room: String,
    },
    RevokeInvite {
        room: String,
        id: String,
    },
    Member {
        room: String,
        member: Member,
    },
    Ban {
        room: String,
        user_id: i64,
        banned: bool,
    },
    Policy {
        room: String,
        policy: RoomPolicy,
    },
    CloseRoom {
        room: String,
        closed: bool,
    },
    DeleteRoom {
        room: String,
    },
    Friend {
        user_id: i64,
        action: FriendAction,
    },
    AcceptInvite {
        id: String,
    },
    DeclineInvite {
        id: String,
    },
    Presence {
        room: String,
        q0rg_id: u16,
        layer_id: u16,
        frame: u16,
    },
    Avatar {
        user_id: i64,
        url: String,
    },
    /// A line in the room chat.
    Say {
        room: String,
        body: String,
    },
    Conversations,
    Thread {
        user_id: i64,
    },
    SendDm {
        user_id: i64,
        body: String,
    },
    ReadDm {
        user_id: i64,
    },
}
impl Command {
    /// Chat never waits behind a room download, an edit or a list refresh.
    fn is_chat(&self) -> bool {
        matches!(
            self,
            Self::Say { .. }
                | Self::Conversations
                | Self::Thread { .. }
                | Self::SendDm { .. }
                | Self::ReadDm { .. }
        )
    }
    /// Requests the panel shows as pending. Every keyed command answers with
    /// `Reply::Finished(key)` after its result, success or failure.
    pub fn busy_key(&self) -> Option<String> {
        Some(match self {
            Self::Login => "login".into(),
            Self::Logout => "logout".into(),
            Self::Refresh => "refresh".into(),
            Self::Search { .. } => "search".into(),
            // one room opens at a time, whichever way it was chosen
            Self::Create { .. } | Self::Join { .. } | Self::AcceptInvite { .. } => "join".into(),
            Self::DeclineInvite { id } => format!("decline:{id}"),
            Self::Invite { user_id, .. } => format!("invite:{user_id}"),
            Self::RevokeInvite { id, .. } => format!("revoke:{id}"),
            Self::Member { member, .. } => format!("member:{}", member.user_id),
            Self::Ban { user_id, .. } => format!("ban:{user_id}"),
            Self::Policy { .. } => "policy".into(),
            Self::CloseRoom { .. } => "close".into(),
            Self::DeleteRoom { .. } => "delete-room".into(),
            Self::Friend { user_id, .. } => format!("friend:{user_id}"),
            Self::LeaveRoom
            | Self::Submit { .. }
            | Self::History { .. }
            | Self::RoomInvites { .. }
            | Self::Presence { .. }
            | Self::Avatar { .. }
            | Self::Say { .. }
            | Self::Conversations
            | Self::Thread { .. }
            | Self::SendDm { .. }
            | Self::ReadDm { .. } => return None,
        })
    }
}
pub enum Reply {
    Login {
        user_code: String,
        verification_uri: String,
        expires_in: u64,
    },
    /// The browser request ended without a sign-in: denied, expired or failed.
    LoginEnded {
        message: String,
    },
    SignedIn(Value),
    SignedOut,
    People(Value),
    Lists {
        rooms: Value,
        friends: Value,
        invites: Value,
    },
    RoomInvites {
        room: String,
        invites: Value,
    },
    Snapshot(Box<RoomSnapshot>),
    Event {
        room: String,
        event: RoomEvent,
    },
    Events {
        room: String,
        batch: EventBatch,
    },
    Failure {
        message: String,
        status: u16,
        operation: bool,
        denied: bool,
    },
    /// Long-poll health. Transient failures retry quietly instead of
    /// replacing the panel's message every few seconds.
    Connection {
        ok: bool,
        message: String,
    },
    Notice(String),
    RoomClosed {
        room: String,
        closed: bool,
    },
    RoomDeleted {
        room: String,
    },
    Avatar {
        user_id: i64,
        image: Option<egui::ColorImage>,
    },
    Finished(String),
    ChatSent {
        room: String,
        message: Value,
    },
    /// A chat line or message that was not delivered, said where you typed it.
    ChatFailed(String),
    /// New direct messages to or from you, from the inbox long poll.
    Inbox(Value),
    Conversations(Value),
    Thread {
        user_id: i64,
        value: Value,
    },
    DmSent(Value),
}
#[derive(Debug)]
struct Failure {
    status: u16,
    code: String,
    message: String,
}
impl Failure {
    /// Readable text for the panel; raw codes stay out of user messages.
    fn text(&self) -> String {
        match (self.status, self.code.as_str()) {
            (0, "connection") => {
                "can’t reach the q0id server. check your connection; q0editor keeps trying".into()
            }
            (_, "invalid_response" | "protocol") => {
                "the server sent something this editor doesn’t understand. q0editor may need an update"
                    .into()
            }
            (401, _) => "your q0id sign-in expired. sign in again to continue".into(),
            (429, _) => "too many requests at once. wait a moment and try again".into(),
            (500..=599, _) => "q0id couldn’t finish that request. try again shortly".into(),
            _ if self.message.is_empty() => "the request failed".into(),
            _ => self.message.clone(),
        }
    }
}
fn request(
    agent: &ureq::Agent,
    base: &str,
    creds: &Arc<Mutex<Credentials>>,
    method: &str,
    path: &str,
    body: Option<Value>,
    auth: bool,
) -> Result<Value, Failure> {
    request_inner(agent, base, creds, method, path, body, auth, true)
}
#[allow(clippy::too_many_arguments)]
fn request_inner(
    agent: &ureq::Agent,
    base: &str,
    creds: &Arc<Mutex<Credentials>>,
    method: &str,
    path: &str,
    body: Option<Value>,
    auth: bool,
    refresh: bool,
) -> Result<Value, Failure> {
    let mut req = agent
        .request(method, &format!("{base}{path}"))
        .set("Accept", "application/json");
    let original_token = if auth {
        creds
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .access
            .clone()
    } else {
        String::new()
    };
    if auth {
        let token = original_token.clone();
        if token.is_empty() {
            return Err(Failure {
                status: 401,
                code: "signed_out".into(),
                message: "sign in to q0id first".into(),
            });
        }
        req = req.set("Authorization", &format!("Bearer {token}"));
    }
    let response = match body.as_ref() {
        Some(value) => req
            .set("Content-Type", "application/json")
            .send_string(&value.to_string()),
        None => req.call(),
    };
    let (status, response) = match response {
        Ok(r) => (r.status(), r),
        Err(ureq::Error::Status(s, r)) => (s, r),
        Err(e) => {
            return Err(Failure {
                status: 0,
                code: "connection".into(),
                message: e.to_string(),
            })
        }
    };
    let mut bytes = Vec::new();
    response
        .into_reader()
        .take(MAX_RESPONSE + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| Failure {
            status: 0,
            code: "connection".into(),
            message: e.to_string(),
        })?;
    if bytes.len() as u64 > MAX_RESPONSE {
        return Err(Failure {
            status: 413,
            code: "too_large".into(),
            message: "the room is larger than this editor’s online project limit".into(),
        });
    }
    let value: Value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).map_err(|_| Failure {
            status,
            code: "invalid_response".into(),
            message: "the server did not return online api data".into(),
        })?
    };
    if status == 401 && auth && refresh {
        // Both worker threads share this lock, so a refresh token is rotated
        // once even when a long poll and an edit expire together.
        let mut current = creds.lock().unwrap_or_else(|e| e.into_inner());
        if current.access == original_token && !current.refresh.is_empty() {
            let tokens = request_inner(
                agent,
                base,
                creds,
                "POST",
                "/api/v1/auth/refresh",
                Some(json!({"refresh_token":current.refresh})),
                false,
                false,
            )?;
            let access = tokens["access_token"].as_str().unwrap_or("");
            let refresh_token = tokens["refresh_token"].as_str().unwrap_or("");
            if !access.is_empty() && !refresh_token.is_empty() {
                current.access = access.into();
                current.refresh = refresh_token.into();
            }
        }
        drop(current);
        return request_inner(agent, base, creds, method, path, body, auth, false);
    }
    if status >= 400 {
        return Err(decode_failure(status, &value));
    }

    Ok(value)
}
fn decode_failure(status: u16, value: &Value) -> Failure {
    let detail = value
        .get("error")
        .filter(|v| v.is_object())
        .unwrap_or(value);
    Failure {
        status,
        code: detail
            .get("code")
            .and_then(Value::as_str)
            .or_else(|| value.get("error").and_then(Value::as_str))
            .unwrap_or("")
            .into(),
        message: detail
            .get("message")
            .and_then(Value::as_str)
            .or_else(|| value.get("error").and_then(Value::as_str))
            .unwrap_or("request failed")
            .into(),
    }
}
fn decode<T: serde::de::DeserializeOwned>(value: Value) -> Result<T, Failure> {
    serde_json::from_value(value).map_err(|e| Failure {
        status: 0,
        code: "protocol".into(),
        message: e.to_string(),
    })
}
fn id_path(id: &str) -> Result<&str, Failure> {
    if !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        Ok(id)
    } else {
        Err(Failure {
            status: 400,
            code: "invalid_room".into(),
            message: "that isn’t a room code. room codes are 32 letters and digits".into(),
        })
    }
}
fn room_path(room: &str, suffix: &str) -> Result<String, Failure> {
    Ok(format!("/api/v1/rooms/{}{suffix}", id_path(room)?))
}
fn event(value: Value) -> Result<RoomEvent, Failure> {
    let value = value.get("event").cloned().unwrap_or(value);
    decode(value)
}
fn snapshot(value: Value) -> Result<RoomSnapshot, Failure> {
    let value = value.get("room").cloned().unwrap_or(value);
    decode(value)
}
fn send(tx: &mpsc::Sender<Reply>, ctx: &Context, reply: Reply) {
    if tx.send(reply).is_ok() {
        ctx.request_repaint();
    }
}
/// Rooms the server has closed, forgotten or forbidden end the session.
fn room_gone(status: u16) -> bool {
    matches!(status, 403 | 404 | 410)
}
struct Poll {
    stop: Arc<AtomicBool>,
    revision: Arc<AtomicU64>,
}
impl Drop for Poll {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}
fn start_poll(
    base: String,
    creds: Arc<Mutex<Credentials>>,
    room: &RoomSnapshot,
    tx: mpsc::Sender<Reply>,
    ctx: Context,
    quit: Arc<AtomicBool>,
) -> Poll {
    let stop = Arc::new(AtomicBool::new(false));
    let revision = Arc::new(AtomicU64::new(room.revision));
    let thread_stop = stop.clone();
    let thread_revision = revision.clone();
    let id = room.id.clone();
    std::thread::spawn(move || {
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(5))
            .timeout_read(Duration::from_secs(25))
            .build();
        let mut healthy = true;
        let mut failures = 0u32;
        // 0 asks for the recent conversation once; then only what is new
        let mut chat_since = 0i64;
        while !thread_stop.load(Ordering::Acquire) && !quit.load(Ordering::Acquire) {
            let path = match room_path(
                &id,
                &format!(
                    "/events?since={}&wait=15&chat_since={chat_since}",
                    thread_revision.load(Ordering::Acquire)
                ),
            ) {
                Ok(p) => p,
                Err(_) => break,
            };
            match request(&agent, &base, &creds, "GET", &path, None, true)
                .and_then(decode::<EventBatch>)
            {
                Ok(batch) => {
                    if thread_stop.load(Ordering::Acquire) {
                        break;
                    }
                    if let Some(last) = batch.chat.iter().map(|line| line.id).max() {
                        chat_since = chat_since.max(last);
                    }
                    if !healthy {
                        healthy = true;
                        failures = 0;
                        send(
                            &tx,
                            &ctx,
                            Reply::Connection {
                                ok: true,
                                message: String::new(),
                            },
                        );
                    }
                    send(
                        &tx,
                        &ctx,
                        Reply::Events {
                            room: id.clone(),
                            batch,
                        },
                    );
                }
                Err(e) if e.status == 409 => {
                    if let Ok(path) = room_path(&id, "") {
                        match request(&agent, &base, &creds, "GET", &path, None, true)
                            .and_then(snapshot)
                        {
                            Ok(room) => send(&tx, &ctx, Reply::Snapshot(Box::new(room))),
                            Err(e) => send(
                                &tx,
                                &ctx,
                                Reply::Failure {
                                    message: e.text(),
                                    status: e.status,
                                    operation: false,
                                    denied: room_gone(e.status),
                                },
                            ),
                        }
                    }
                }
                Err(e) if room_gone(e.status) || e.status == 401 => {
                    if !thread_stop.load(Ordering::Acquire) {
                        send(
                            &tx,
                            &ctx,
                            Reply::Failure {
                                message: e.text(),
                                status: e.status,
                                operation: false,
                                denied: room_gone(e.status),
                            },
                        );
                    }
                    break;
                }
                Err(e) => {
                    healthy = false;
                    failures = failures.saturating_add(1);
                    if !thread_stop.load(Ordering::Acquire) {
                        send(
                            &tx,
                            &ctx,
                            Reply::Connection {
                                ok: false,
                                message: e.text(),
                            },
                        );
                    }
                    // back off gently while the server or network is away
                    let wait = 2u64.saturating_mul(u64::from(failures.min(5)));
                    let until = Instant::now() + Duration::from_secs(wait);
                    while Instant::now() < until && !thread_stop.load(Ordering::Acquire) {
                        std::thread::sleep(Duration::from_millis(100));
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(150));
        }
    });
    Poll { stop, revision }
}

/// Direct messages for the signed-in account, followed from sign-in until
/// sign-out. It starts at the newest message, so history is never replayed.
fn start_inbox(
    base: String,
    creds: Arc<Mutex<Credentials>>,
    tx: mpsc::Sender<Reply>,
    ctx: Context,
    quit: Arc<AtomicBool>,
) -> Poll {
    let stop = Arc::new(AtomicBool::new(false));
    let thread_stop = stop.clone();
    std::thread::spawn(move || {
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(5))
            .timeout_read(Duration::from_secs(30))
            .build();
        let mut cursor: Option<i64> = None;
        let mut failures = 0u64;
        while !thread_stop.load(Ordering::Acquire) && !quit.load(Ordering::Acquire) {
            let path = match cursor {
                Some(since) => format!("/api/v1/messages/feed?since={since}&wait=20"),
                None => "/api/v1/messages/feed".to_string(),
            };
            match request(&agent, &base, &creds, "GET", &path, None, true) {
                Ok(value) => {
                    failures = 0;
                    if thread_stop.load(Ordering::Acquire) {
                        break;
                    }
                    cursor = value["cursor"].as_i64().or(cursor);
                    send(&tx, &ctx, Reply::Inbox(value));
                }
                // signed out, or a server without messages: nothing to follow
                Err(e) if matches!(e.status, 401 | 404) => break,
                Err(_) => {
                    failures = (failures + 1).min(5);
                    let until = Instant::now() + Duration::from_secs(3 * failures);
                    while Instant::now() < until && !thread_stop.load(Ordering::Acquire) {
                        std::thread::sleep(Duration::from_millis(100));
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    });
    Poll {
        stop,
        revision: Arc::new(AtomicU64::new(0)),
    }
}

/// Room chat and direct messages, on their own thread.
fn chat_worker(
    base: String,
    credentials: Arc<Mutex<Credentials>>,
    commands: mpsc::Receiver<Command>,
    replies: mpsc::Sender<Reply>,
    ctx: Context,
    stopped: Arc<AtomicBool>,
) {
    std::thread::spawn(move || {
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(5))
            .timeout_read(Duration::from_secs(20))
            .build();
        while !stopped.load(Ordering::Acquire) {
            let command = match commands.recv_timeout(Duration::from_millis(200)) {
                Ok(command) => command,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            };
            let call = |method: &str, path: &str, body: Option<Value>| {
                request(&agent, &base, &credentials, method, path, body, true)
            };
            let result: Result<Option<Reply>, Failure> = (|| match command {
                Command::Say { room, body } => {
                    let v = call(
                        "POST",
                        &room_path(&room, "/chat")?,
                        Some(json!({ "body": body })),
                    )?;
                    Ok(Some(Reply::ChatSent {
                        room,
                        message: v["message"].clone(),
                    }))
                }
                Command::Conversations => Ok(Some(Reply::Conversations(call(
                    "GET",
                    "/api/v1/messages",
                    None,
                )?))),
                Command::Thread { user_id } => Ok(Some(Reply::Thread {
                    user_id,
                    value: call("GET", &format!("/api/v1/messages/{user_id}"), None)?,
                })),
                Command::SendDm { user_id, body } => {
                    let v = call(
                        "POST",
                        &format!("/api/v1/messages/{user_id}"),
                        Some(json!({ "body": body })),
                    )?;
                    Ok(Some(Reply::DmSent(v["message"].clone())))
                }
                Command::ReadDm { user_id } => {
                    call(
                        "POST",
                        &format!("/api/v1/messages/{user_id}/read"),
                        Some(json!({})),
                    )?;
                    Ok(None)
                }
                _ => Ok(None),
            })();
            match result {
                Ok(Some(reply)) => send(&replies, &ctx, reply),
                Ok(None) => {}
                Err(e) if e.status == 404 && e.code == "invalid_response" => send(
                    &replies,
                    &ctx,
                    Reply::ChatFailed(
                        "this server doesn’t have chat yet. it arrives with the next q0id update"
                            .into(),
                    ),
                ),
                Err(e) => send(&replies, &ctx, Reply::ChatFailed(e.text())),
            }
        }
    });
}

/// Avatars are optional decoration: fetched from the chosen server only, size
/// limited, decoded off the ui thread, and silently replaced by initials.
fn fetch_avatar(base: &str, url: &str) -> Option<egui::ColorImage> {
    if !url.starts_with('/') || url.starts_with("//") || url.ends_with(".svg") {
        return None;
    }
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(5))
        .timeout_read(Duration::from_secs(10))
        .build();
    let response = agent.get(&format!("{base}{url}")).call().ok()?;
    let mut bytes = Vec::new();
    response
        .into_reader()
        .take(MAX_AVATAR + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() as u64 > MAX_AVATAR {
        return None;
    }
    let image = image::load_from_memory(&bytes)
        .ok()?
        .thumbnail_exact(AVATAR_SIZE, AVATAR_SIZE)
        .to_rgba8();
    Some(egui::ColorImage::from_rgba_unmultiplied(
        [image.width() as usize, image.height() as usize],
        image.as_raw(),
    ))
}

pub struct Transport {
    tx: mpsc::Sender<Command>,
    chats: mpsc::Sender<Command>,
    /// Room edits, undo/redo and presence: never queued behind list refreshes,
    /// searches or invitations on the general worker.
    edits: mpsc::Sender<Command>,
    pub rx: mpsc::Receiver<Reply>,
    quit: Arc<AtomicBool>,
    seen: Arc<Mutex<Option<Arc<AtomicU64>>>>,
    finished: mpsc::Receiver<()>,
}
impl Drop for Transport {
    /// Never blocks: cancelling sign-in or changing servers must not freeze
    /// the editor while a worker finishes a long request.
    fn drop(&mut self) {
        self.quit.store(true, Ordering::Release);
    }
}
impl Transport {
    pub fn new(base: String, ctx: Context) -> Self {
        let (tx, commands) = mpsc::channel();
        let (replies, rx) = mpsc::channel();
        let quit = Arc::new(AtomicBool::new(false));
        let stopped = quit.clone();
        let seen = Arc::new(Mutex::new(None));
        let worker_seen = seen.clone();
        let (finished_tx, finished) = mpsc::channel();
        let shared_credentials = Arc::new(Mutex::new(Credentials::default()));
        let (edits_tx, edits) = mpsc::channel();
        edit_worker(
            base.clone(),
            shared_credentials.clone(),
            edits,
            replies.clone(),
            ctx.clone(),
            quit.clone(),
        );
        let (chats_tx, chats) = mpsc::channel();
        chat_worker(
            base.clone(),
            shared_credentials.clone(),
            chats,
            replies.clone(),
            ctx.clone(),
            quit.clone(),
        );
        std::thread::spawn(move || {
            let agent = ureq::AgentBuilder::new()
                .timeout_connect(Duration::from_secs(5))
                .timeout_read(Duration::from_secs(30))
                .build();
            let credentials = shared_credentials;
            let avatar_workers = Arc::new(AtomicUsize::new(0));
            let mut avatar_queue: Vec<(i64, String)> = Vec::new();
            let mut login: Option<(String, Instant, Instant, u64)> = None;
            let mut poll: Option<Poll> = None;
            let mut inbox: Option<Poll> = None;
            let mut active_room: Option<String> = None;
            while !stopped.load(Ordering::Acquire) {
                let received = commands.recv_timeout(Duration::from_millis(200));
                match received {
                    Ok(Command::Avatar { user_id, url }) => {
                        avatar_queue.push((user_id, url));
                    }
                    Ok(command) => {
                        let busy = command.busy_key();
                        let result: Result<Option<Reply>, Failure> = (|| match command {
                            Command::Login => {
                                let v = request(
                                    &agent,
                                    &base,
                                    &credentials,
                                    "POST",
                                    "/api/v1/auth/device",
                                    Some(json!({"client_name":"q0editor"})),
                                    false,
                                )?;
                                let code = v["device_code"]
                                    .as_str()
                                    .ok_or_else(|| Failure {
                                        status: 0,
                                        code: "protocol".into(),
                                        message: "missing device code".into(),
                                    })?
                                    .to_string();
                                let interval = v["interval"].as_u64().unwrap_or(5).max(1);
                                let expires = v["expires_in"].as_u64().unwrap_or(600).min(1800);
                                login = Some((
                                    code,
                                    Instant::now() + Duration::from_secs(expires),
                                    Instant::now() + Duration::from_secs(interval),
                                    interval,
                                ));
                                Ok(Some(Reply::Login {
                                    user_code: v["user_code"].as_str().unwrap_or("").into(),
                                    verification_uri: verification_url(
                                        &base,
                                        v["verification_uri_complete"]
                                            .as_str()
                                            .or_else(|| v["verification_uri"].as_str())
                                            .unwrap_or(""),
                                    ),
                                    expires_in: expires,
                                }))
                            }
                            Command::LeaveRoom => {
                                poll = None;
                                if let Some(room) = active_room.take() {
                                    leave_request(&base, &credentials, &room);
                                }
                                *worker_seen.lock().unwrap_or_else(|e| e.into_inner()) = None;
                                Ok(None)
                            }
                            Command::Logout => {
                                poll = None;
                                inbox = None;
                                if let Some(room) = active_room.take() {
                                    leave_request(&base, &credentials, &room);
                                }
                                let _ = request(
                                    &agent,
                                    &base,
                                    &credentials,
                                    "POST",
                                    "/api/v1/auth/logout",
                                    Some(json!({})),
                                    true,
                                );
                                login = None;
                                *credentials.lock().unwrap_or_else(|e| e.into_inner()) =
                                    Credentials::default();
                                Ok(Some(Reply::SignedOut))
                            }
                            Command::Refresh => {
                                let rooms = request(
                                    &agent,
                                    &base,
                                    &credentials,
                                    "GET",
                                    "/api/v1/rooms",
                                    None,
                                    true,
                                )?;
                                let friends = request(
                                    &agent,
                                    &base,
                                    &credentials,
                                    "GET",
                                    "/api/v1/friends",
                                    None,
                                    true,
                                )?;
                                let invites = request(
                                    &agent,
                                    &base,
                                    &credentials,
                                    "GET",
                                    "/api/v1/invites",
                                    None,
                                    true,
                                )?;
                                Ok(Some(Reply::Lists {
                                    rooms,
                                    friends,
                                    invites,
                                }))
                            }
                            Command::Search { query } => {
                                let query = query
                                    .bytes()
                                    .map(|b| {
                                        if b.is_ascii_alphanumeric() || b == b'_' {
                                            char::from(b).to_string()
                                        } else {
                                            format!("%{b:02X}")
                                        }
                                    })
                                    .collect::<String>();
                                Ok(Some(Reply::People(request(
                                    &agent,
                                    &base,
                                    &credentials,
                                    "GET",
                                    &format!("/api/v1/people/search?q={query}"),
                                    None,
                                    true,
                                )?)))
                            }
                            Command::Create { name, project } => {
                                let room = snapshot(request(
                                    &agent,
                                    &base,
                                    &credentials,
                                    "POST",
                                    "/api/v1/rooms",
                                    Some(json!({"name":name,"project":project})),
                                    true,
                                )?)?;
                                if active_room.as_deref() != Some(room.id.as_str()) {
                                    if let Some(previous) = active_room.take() {
                                        leave_request(&base, &credentials, &previous);
                                    }
                                }
                                active_room = Some(room.id.clone());
                                poll = Some(start_poll(
                                    base.clone(),
                                    credentials.clone(),
                                    &room,
                                    replies.clone(),
                                    ctx.clone(),
                                    stopped.clone(),
                                ));
                                *worker_seen.lock().unwrap_or_else(|e| e.into_inner()) =
                                    poll.as_ref().map(|p| p.revision.clone());
                                Ok(Some(Reply::Snapshot(Box::new(room))))
                            }
                            Command::Join { id } => {
                                let room = snapshot(request(
                                    &agent,
                                    &base,
                                    &credentials,
                                    "GET",
                                    &room_path(&id, "")?,
                                    None,
                                    true,
                                )?)?;
                                if active_room.as_deref() != Some(room.id.as_str()) {
                                    if let Some(previous) = active_room.take() {
                                        leave_request(&base, &credentials, &previous);
                                    }
                                }
                                active_room = Some(room.id.clone());
                                poll = Some(start_poll(
                                    base.clone(),
                                    credentials.clone(),
                                    &room,
                                    replies.clone(),
                                    ctx.clone(),
                                    stopped.clone(),
                                ));
                                *worker_seen.lock().unwrap_or_else(|e| e.into_inner()) =
                                    poll.as_ref().map(|p| p.revision.clone());
                                Ok(Some(Reply::Snapshot(Box::new(room))))
                            }
                            Command::DeleteRoom { room } => {
                                request(
                                    &agent,
                                    &base,
                                    &credentials,
                                    "DELETE",
                                    &room_path(&room, "")?,
                                    None,
                                    true,
                                )?;
                                if active_room.as_ref() == Some(&room) {
                                    poll = None;
                                    active_room = None;
                                }
                                Ok(Some(Reply::RoomDeleted { room }))
                            }
                            // room edits and chat run on their own workers
                            Command::Submit { .. }
                            | Command::History { .. }
                            | Command::Presence { .. }
                            | Command::Say { .. }
                            | Command::Conversations
                            | Command::Thread { .. }
                            | Command::SendDm { .. }
                            | Command::ReadDm { .. } => Ok(None),
                            Command::Invite {
                                room,
                                user_id,
                                rank,
                            } => {
                                request(
                                    &agent,
                                    &base,
                                    &credentials,
                                    "POST",
                                    &room_path(&room, "/invites")?,
                                    Some(json!({"user_id":user_id,"rank":rank,"grants":[]})),
                                    true,
                                )?;
                                let invites = request(
                                    &agent,
                                    &base,
                                    &credentials,
                                    "GET",
                                    &room_path(&room, "/invites")?,
                                    None,
                                    true,
                                )
                                .unwrap_or(Value::Null);
                                send(&replies, &ctx, Reply::RoomInvites { room, invites });
                                Ok(Some(Reply::Notice(
                                    "invitation sent. it waits in their inbox for 7 days".into(),
                                )))
                            }
                            Command::RoomInvites { room } => {
                                let invites = request(
                                    &agent,
                                    &base,
                                    &credentials,
                                    "GET",
                                    &room_path(&room, "/invites")?,
                                    None,
                                    true,
                                )?;
                                Ok(Some(Reply::RoomInvites { room, invites }))
                            }
                            Command::RevokeInvite { room, id } => {
                                request(
                                    &agent,
                                    &base,
                                    &credentials,
                                    "POST",
                                    &format!("/api/v1/invites/{}/revoke", id_path(&id)?),
                                    Some(json!({})),
                                    true,
                                )?;
                                let invites = request(
                                    &agent,
                                    &base,
                                    &credentials,
                                    "GET",
                                    &room_path(&room, "/invites")?,
                                    None,
                                    true,
                                )
                                .unwrap_or(Value::Null);
                                send(&replies, &ctx, Reply::RoomInvites { room, invites });
                                Ok(Some(Reply::Notice("invitation withdrawn".into())))
                            }
                            Command::Member { room, member } => {
                                request(
                                    &agent,
                                    &base,
                                    &credentials,
                                    "POST",
                                    &room_path(&room, "/members?view=summary")?,
                                    Some(serde_json::to_value(member).unwrap()),
                                    true,
                                )?;
                                Ok(Some(Reply::Notice("access saved".into())))
                            }
                            Command::Ban {
                                room,
                                user_id,
                                banned,
                            } => {
                                request(
                                    &agent,
                                    &base,
                                    &credentials,
                                    "POST",
                                    &room_path(&room, "/ban?view=summary")?,
                                    Some(json!({"user_id":user_id,"banned":banned})),
                                    true,
                                )?;
                                Ok(Some(Reply::Notice(
                                    if banned {
                                        "participant banned. they were disconnected and can’t rejoin"
                                    } else {
                                        "participant unbanned. they can open the room again"
                                    }
                                    .into(),
                                )))
                            }
                            Command::Policy { room, policy } => {
                                request(
                                    &agent,
                                    &base,
                                    &credentials,
                                    "POST",
                                    &room_path(&room, "/policy?view=summary")?,
                                    Some(serde_json::to_value(policy).unwrap()),
                                    true,
                                )?;
                                Ok(Some(Reply::Notice("room settings saved".into())))
                            }
                            Command::CloseRoom { room, closed } => {
                                request(
                                    &agent,
                                    &base,
                                    &credentials,
                                    "POST",
                                    &room_path(&room, "/close")?,
                                    Some(json!({"closed":closed})),
                                    true,
                                )?;
                                Ok(Some(Reply::RoomClosed { room, closed }))
                            }
                            Command::Friend { user_id, action } => {
                                let (path, body, done) = match action {
                                    FriendAction::Request => (
                                        "/api/v1/friends/request".to_string(),
                                        json!({"user_id":user_id}),
                                        "friend request sent",
                                    ),
                                    FriendAction::Accept => (
                                        format!("/api/v1/friends/{user_id}/respond"),
                                        json!({"accept":true}),
                                        "friend request accepted",
                                    ),
                                    FriendAction::Decline => (
                                        format!("/api/v1/friends/{user_id}/respond"),
                                        json!({"accept":false}),
                                        "friend request declined",
                                    ),
                                    FriendAction::Remove => (
                                        format!("/api/v1/friends/{user_id}/remove"),
                                        json!({}),
                                        "removed",
                                    ),
                                    FriendAction::Block => (
                                        format!("/api/v1/friends/{user_id}/block"),
                                        json!({}),
                                        "blocked. they can’t find you, invite you or send requests",
                                    ),
                                    FriendAction::Unblock => (
                                        format!("/api/v1/friends/{user_id}/unblock"),
                                        json!({}),
                                        "unblocked",
                                    ),
                                };
                                let v = request(
                                    &agent,
                                    &base,
                                    &credentials,
                                    "POST",
                                    &path,
                                    Some(body),
                                    true,
                                )?;
                                // requesting someone who already asked you makes you friends
                                let done = if action == FriendAction::Request
                                    && v["status"].as_str() == Some("accepted")
                                {
                                    "you are friends now"
                                } else {
                                    done
                                };
                                Ok(Some(Reply::Notice(done.into())))
                            }
                            Command::AcceptInvite { id } => {
                                let id = id_path(&id)?;
                                let room = snapshot(request(
                                    &agent,
                                    &base,
                                    &credentials,
                                    "POST",
                                    &format!("/api/v1/invites/{id}/accept"),
                                    Some(json!({})),
                                    true,
                                )?)?;
                                if active_room.as_deref() != Some(room.id.as_str()) {
                                    if let Some(previous) = active_room.take() {
                                        leave_request(&base, &credentials, &previous);
                                    }
                                }
                                active_room = Some(room.id.clone());
                                poll = Some(start_poll(
                                    base.clone(),
                                    credentials.clone(),
                                    &room,
                                    replies.clone(),
                                    ctx.clone(),
                                    stopped.clone(),
                                ));
                                *worker_seen.lock().unwrap_or_else(|e| e.into_inner()) =
                                    poll.as_ref().map(|p| p.revision.clone());
                                Ok(Some(Reply::Snapshot(Box::new(room))))
                            }
                            Command::DeclineInvite { id } => {
                                request(
                                    &agent,
                                    &base,
                                    &credentials,
                                    "POST",
                                    &format!("/api/v1/invites/{}/decline", id_path(&id)?),
                                    Some(json!({})),
                                    true,
                                )?;
                                Ok(Some(Reply::Notice("invitation declined".into())))
                            }
                            Command::Avatar { .. } => Ok(None),
                        })();
                        match result {
                            Ok(Some(reply)) => send(&replies, &ctx, reply),
                            Ok(None) => {}
                            Err(e) => send(
                                &replies,
                                &ctx,
                                Reply::Failure {
                                    message: e.text(),
                                    status: e.status,
                                    operation: false,
                                    denied: false,
                                },
                            ),
                        }
                        if let Some(key) = busy {
                            send(&replies, &ctx, Reply::Finished(key));
                        }
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                }
                while avatar_workers.load(Ordering::Acquire) < AVATAR_WORKERS {
                    let Some((user_id, url)) = avatar_queue.pop() else {
                        break;
                    };
                    avatar_workers.fetch_add(1, Ordering::AcqRel);
                    let (base, replies, ctx, workers) = (
                        base.clone(),
                        replies.clone(),
                        ctx.clone(),
                        avatar_workers.clone(),
                    );
                    std::thread::spawn(move || {
                        let image = fetch_avatar(&base, &url);
                        send(&replies, &ctx, Reply::Avatar { user_id, image });
                        workers.fetch_sub(1, Ordering::AcqRel);
                    });
                }
                if login
                    .as_ref()
                    .is_some_and(|(_, _, next, _)| *next <= Instant::now())
                {
                    let (code, expires, _, interval) = login.take().unwrap();
                    if expires <= Instant::now() {
                        send(
                            &replies,
                            &ctx,
                            Reply::LoginEnded {
                                message: "the sign-in request expired before it was approved"
                                    .into(),
                            },
                        );
                        continue;
                    }
                    match request(
                        &agent,
                        &base,
                        &credentials,
                        "POST",
                        "/api/v1/auth/device/token",
                        Some(json!({"device_code":code})),
                        false,
                    ) {
                        Ok(v) => {
                            let access = v["access_token"].as_str().unwrap_or("").to_string();
                            let refresh = v["refresh_token"].as_str().unwrap_or("").to_string();
                            if access.is_empty() {
                                send(
                                    &replies,
                                    &ctx,
                                    Reply::LoginEnded {
                                        message: "the server approved the sign-in but sent no credentials"
                                            .into(),
                                    },
                                );
                            } else {
                                *credentials.lock().unwrap_or_else(|e| e.into_inner()) =
                                    Credentials { access, refresh };
                                match request(
                                    &agent,
                                    &base,
                                    &credentials,
                                    "GET",
                                    "/api/v1/auth/me",
                                    None,
                                    true,
                                ) {
                                    Ok(user) => {
                                        send(&replies, &ctx, Reply::SignedIn(user));
                                        inbox = Some(start_inbox(
                                            base.clone(),
                                            credentials.clone(),
                                            replies.clone(),
                                            ctx.clone(),
                                            stopped.clone(),
                                        ));
                                    }
                                    Err(e) => send(
                                        &replies,
                                        &ctx,
                                        Reply::LoginEnded { message: e.text() },
                                    ),
                                }
                            }
                        }
                        Err(e) if e.code == "authorization_pending" || e.code == "slow_down" => {
                            let next_interval = if e.code == "slow_down" {
                                interval + 5
                            } else {
                                interval
                            };
                            login = Some((
                                code,
                                expires,
                                Instant::now() + Duration::from_secs(next_interval),
                                next_interval,
                            ));
                        }
                        Err(e) if e.status == 0 && e.code == "connection" => {
                            // a dropped connection is not an answer; keep waiting
                            login = Some((
                                code,
                                expires,
                                Instant::now() + Duration::from_secs(interval.max(5)),
                                interval,
                            ));
                        }
                        Err(e) => {
                            let message = match e.code.as_str() {
                                "access_denied" => {
                                    "the sign-in was declined in the browser".to_string()
                                }
                                "expired_token" => {
                                    "the sign-in request expired before it was approved".into()
                                }
                                "invalid_grant" => "this sign-in request was already used".into(),
                                _ => e.text(),
                            };
                            send(&replies, &ctx, Reply::LoginEnded { message });
                        }
                    }
                }
            }
            drop(poll);
            drop(inbox);
            if let Some(room) = active_room {
                leave_request(&base, &credentials, &room);
            }
            let _ = finished_tx.send(());
        });
        Self {
            tx,
            chats: chats_tx,
            edits: edits_tx,
            rx,
            quit,
            seen,
            finished,
        }
    }
    /// On exit: stop, and give the worker a moment to tell the room you left.
    pub fn shutdown(self) {
        self.quit.store(true, Ordering::Release);
        let _ = self.finished.recv_timeout(Duration::from_millis(1200));
    }
    pub fn send(&self, command: Command) {
        let _ = match command {
            Command::Submit { .. } | Command::History { .. } | Command::Presence { .. } => {
                self.edits.send(command)
            }
            command if command.is_chat() => self.chats.send(command),
            command => self.tx.send(command),
        };
    }
    pub fn seen(&self, revision: u64) {
        if let Some(seen) = self.seen.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            seen.store(revision, Ordering::Release);
        }
    }
}

fn leave_request(base: &str, credentials: &Arc<Mutex<Credentials>>, room: &str) {
    let Ok(path) = room_path(room, "/leave") else {
        return;
    };
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_millis(800))
        .build();
    let _ = request(
        &agent,
        base,
        credentials,
        "POST",
        &path,
        Some(json!({})),
        true,
    );
}

/// Sends room edits, personal undo/redo and presence. An uncertain failure
/// (connection, 5xx, 429) retries the same operation id, which the server's
/// receipts make idempotent.
fn edit_worker(
    base: String,
    credentials: Arc<Mutex<Credentials>>,
    edits: mpsc::Receiver<Command>,
    replies: mpsc::Sender<Reply>,
    ctx: Context,
    stopped: Arc<AtomicBool>,
) {
    std::thread::spawn(move || {
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(5))
            .timeout_read(Duration::from_secs(30))
            .build();
        let mut retry: Option<(Command, Instant)> = None;
        while !stopped.load(Ordering::Acquire) {
            let received = match &retry {
                Some((_, at)) if *at <= Instant::now() => Ok(retry.take().unwrap().0),
                _ => edits.recv_timeout(Duration::from_millis(200)),
            };
            let command = match received {
                Ok(command) => command,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            };
            let result: Result<Option<Reply>, Failure> = (|| match &command {
                Command::Submit { room, request: op } => {
                    let v = request(
                        &agent,
                        &base,
                        &credentials,
                        "POST",
                        &room_path(room, "/operations")?,
                        Some(serde_json::to_value(op).unwrap()),
                        true,
                    )?;
                    Ok(Some(Reply::Event {
                        room: room.clone(),
                        event: event(v)?,
                    }))
                }
                Command::History {
                    room,
                    operation_id,
                    redo,
                } => {
                    let v = request(
                        &agent,
                        &base,
                        &credentials,
                        "POST",
                        &room_path(room, if *redo { "/redo" } else { "/undo" })?,
                        Some(json!({"operation_id":operation_id})),
                        true,
                    )?;
                    Ok(Some(Reply::Event {
                        room: room.clone(),
                        event: event(v)?,
                    }))
                }
                Command::Presence {
                    room,
                    q0rg_id,
                    layer_id,
                    frame,
                } => {
                    request(
                        &agent,
                        &base,
                        &credentials,
                        "POST",
                        &room_path(room, "/presence")?,
                        Some(
                            json!({"cursor":{"q0rg_id":q0rg_id,"layer_id":layer_id,"frame":frame}}),
                        ),
                        true,
                    )?;
                    Ok(None)
                }
                _ => Ok(None),
            })();
            let operation = !matches!(command, Command::Presence { .. });
            match result {
                Ok(Some(reply)) => send(&replies, &ctx, reply),
                Ok(None) => {}
                Err(e)
                    if operation
                        && ((e.status == 0 && e.code == "connection")
                            || e.status >= 500
                            || e.status == 429) =>
                {
                    send(
                        &replies,
                        &ctx,
                        Reply::Connection {
                            ok: false,
                            message: format!(
                                "{}. your edit is kept and will be sent again",
                                e.text()
                            ),
                        },
                    );
                    retry = Some((command, Instant::now() + Duration::from_secs(3)));
                }
                // presence is best effort; the long poll reports lost access
                Err(_) if !operation => {}
                Err(e) => send(
                    &replies,
                    &ctx,
                    Reply::Failure {
                        message: e.text(),
                        status: e.status,
                        operation: true,
                        denied: false,
                    },
                ),
            }
        }
    });
}

fn verification_url(base: &str, uri: &str) -> String {
    if uri.starts_with('/') && !uri.starts_with("//") {
        format!("{base}{uri}")
    } else if uri.starts_with(&format!("{base}/")) {
        uri.into()
    } else {
        format!("{base}/online/authorize")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn device_pending_and_room_conflict_codes_are_preserved() {
        let pending = decode_failure(
            400,
            &json!({"error":"authorization_pending","message":"waiting"}),
        );
        assert_eq!(pending.code, "authorization_pending");
        let conflict = decode_failure(
            409,
            &json!({"error":{"code":"conflict","message":"changed"}}),
        );
        assert_eq!(conflict.code, "conflict");
        assert_eq!(conflict.message, "changed");
    }
    #[test]
    fn authorization_links_stay_on_the_chosen_origin() {
        assert_eq!(
            verification_url("http://127.0.0.1:8080", "/online/authorize?user_code=abc"),
            "http://127.0.0.1:8080/online/authorize?user_code=abc"
        );
        assert_eq!(
            verification_url("https://q0id.com", "//evil.invalid/approve"),
            "https://q0id.com/online/authorize"
        );
        assert_eq!(
            verification_url("https://q0id.com", "https://q0id.com.evil.invalid/approve"),
            "https://q0id.com/online/authorize"
        );
    }
    #[test]
    fn failures_read_as_sentences_without_raw_codes() {
        let offline = Failure {
            status: 0,
            code: "connection".into(),
            message: "io: connection refused (os error 10061)".into(),
        };
        assert!(!offline.text().contains("os error"));
        let forbidden = decode_failure(
            403,
            &json!({"error":"forbidden","message":"your room permissions do not cover the layer “sky” in “main”"}),
        );
        assert!(forbidden.text().starts_with("your room permissions"));
        assert!(!forbidden.text().contains("forbidden:"));
    }
    #[test]
    fn avatars_only_load_from_the_chosen_server() {
        assert!(fetch_avatar("https://q0id.com", "https://elsewhere.invalid/a.png").is_none());
        assert!(fetch_avatar("https://q0id.com", "//elsewhere.invalid/a.png").is_none());
        assert!(fetch_avatar("https://q0id.com", "/assets/img/pfp-default.svg").is_none());
    }
    #[test]
    fn every_user_action_reports_when_it_finishes() {
        assert_eq!(
            Command::Friend {
                user_id: 4,
                action: FriendAction::Block
            }
            .busy_key()
            .as_deref(),
            Some("friend:4")
        );
        assert_eq!(
            Command::AcceptInvite { id: "x".into() }.busy_key(),
            Command::Join { id: "y".into() }.busy_key(),
            "joining any room disables every join control"
        );
        assert!(Command::Presence {
            room: "r".into(),
            q0rg_id: 1,
            layer_id: 1,
            frame: 0
        }
        .busy_key()
        .is_none());
    }

    /// End-to-end check against a running q0id server, approving the device
    /// request the way a signed-in browser does. Ignored by default:
    /// Q0EDITOR_ONLINE_E2E=http://127.0.0.1:8099 Q0EDITOR_ONLINE_E2E_COOKIE=<q0sid>
    /// Q0EDITOR_ONLINE_E2E_ROOM=<room the account owns>
    #[test]
    #[ignore = "needs a running q0id server and a browser session cookie"]
    fn live_server_sign_in_lists_join_and_permissions() {
        let (Ok(base), Ok(cookie), Ok(room)) = (
            std::env::var("Q0EDITOR_ONLINE_E2E"),
            std::env::var("Q0EDITOR_ONLINE_E2E_COOKIE"),
            std::env::var("Q0EDITOR_ONLINE_E2E_ROOM"),
        ) else {
            return;
        };
        let ctx = Context::default();
        let transport = Transport::new(base.clone(), ctx);
        let next = |what: &str| -> Reply {
            let deadline = Instant::now() + Duration::from_secs(40);
            loop {
                match transport.rx.recv_timeout(Duration::from_millis(200)) {
                    Ok(
                        Reply::Finished(_)
                        | Reply::Avatar { .. }
                        | Reply::Inbox(_)
                        | Reply::Connection { ok: true, .. },
                    ) => {}
                    Ok(Reply::Failure { message, .. }) => panic!("{what}: {message}"),
                    Ok(Reply::LoginEnded { message }) => panic!("{what}: {message}"),
                    Ok(reply) => return reply,
                    Err(_) if Instant::now() < deadline => {}
                    Err(_) => panic!("{what}: timed out"),
                }
            }
        };
        transport.send(Command::Login);
        let Reply::Login {
            user_code,
            verification_uri,
            ..
        } = next("device request")
        else {
            panic!("expected a device code");
        };
        assert!(verification_uri.starts_with(&format!("{base}/online/authorize?user_code=")));
        // the browser: open the page (never approves by itself), then press approve
        let agent = ureq::AgentBuilder::new().redirects(0).build();
        let page = agent
            .get(&verification_uri)
            .set("Cookie", &format!("q0sid={cookie}"))
            .call()
            .unwrap()
            .into_string()
            .unwrap();
        let confirmation = page
            .split("name=\"confirmation\" value=\"")
            .nth(1)
            .and_then(|rest| rest.split('"').next())
            .expect("confirmation field");
        let done = agent
            .post(&format!("{base}/online/authorize"))
            .set("Cookie", &format!("q0sid={cookie}"))
            .set("Origin", &base)
            .send_form(&[
                ("user_code", user_code.as_str()),
                ("confirmation", confirmation),
                ("decision", "approve"),
            ])
            .unwrap()
            .into_string()
            .unwrap();
        assert!(done.contains("is connected"));
        let Reply::SignedIn(user) = next("token poll") else {
            panic!("expected sign-in");
        };
        let me = user["id"].as_i64().unwrap();
        transport.send(Command::Refresh);
        let Reply::Lists { rooms, friends, .. } = next("lists") else {
            panic!("expected lists");
        };
        assert!(rooms["rooms"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["id"] == room.as_str()));
        assert!(friends.get("blocked").is_some());
        transport.send(Command::Join { id: room.clone() });
        let Reply::Snapshot(snapshot) = next("join") else {
            panic!("expected a room snapshot");
        };
        assert_eq!(
            snapshot.owner_id, me,
            "the e2e room must belong to this account"
        );
        assert!(snapshot.people.values().all(|p| !p.username.is_empty()));
        transport.send(Command::Presence {
            room: room.clone(),
            q0rg_id: 1,
            layer_id: 6,
            frame: 3,
        });
        let presence = loop {
            if let Reply::Events { batch, .. } = next("events") {
                if let Some(row) = batch
                    .presence
                    .iter()
                    .find(|row| row["user_id"] == me && row["presence"]["frame"] == 3)
                {
                    break row.clone();
                }
            }
        };
        assert_eq!(presence["presence"]["layer_id"], 6);
        let other = snapshot
            .members
            .iter()
            .find(|m| m.user_id != me && m.rank != Rank::Owner)
            .cloned()
            .expect("the e2e room needs a second member");
        transport.send(Command::Member {
            room: room.clone(),
            member: other.clone(),
        });
        let Reply::Notice(saved) = next("member update") else {
            panic!("expected a saved notice");
        };
        assert!(saved.contains("saved"));
        transport.send(Command::RoomInvites { room: room.clone() });
        loop {
            if let Reply::RoomInvites { invites, .. } = next("room invites") {
                assert!(invites["invites"].is_array());
                break;
            }
        }
        transport.send(Command::LeaveRoom);
        transport.send(Command::Logout);
        loop {
            if matches!(next("logout"), Reply::SignedOut) {
                break;
            }
        }
    }

    /// The editor's chat paths against a running server: room history and
    /// live lines through the room poll, sending, and direct messages both
    /// ways. `Q0EDITOR_ONLINE_E2E_FRIEND_COOKIE` is a second account that is
    /// a member of the room.
    #[test]
    #[ignore = "needs a running q0id server, a room and two browser session cookies"]
    fn live_server_room_chat_and_direct_messages() {
        let (Ok(base), Ok(cookie), Ok(room), Ok(friend)) = (
            std::env::var("Q0EDITOR_ONLINE_E2E"),
            std::env::var("Q0EDITOR_ONLINE_E2E_COOKIE"),
            std::env::var("Q0EDITOR_ONLINE_E2E_ROOM"),
            std::env::var("Q0EDITOR_ONLINE_E2E_FRIEND_COOKIE"),
        ) else {
            return;
        };
        let transport = Transport::new(base.clone(), Context::default());
        let wait = |what: &str, wanted: &mut dyn FnMut(&Reply) -> bool| {
            let deadline = Instant::now() + Duration::from_secs(40);
            loop {
                match transport.rx.recv_timeout(Duration::from_millis(200)) {
                    Ok(Reply::Failure { message, .. }) => panic!("{what}: {message}"),
                    Ok(Reply::ChatFailed(message)) => panic!("{what}: {message}"),
                    Ok(Reply::LoginEnded { message }) => panic!("{what}: {message}"),
                    Ok(reply) if wanted(&reply) => return reply,
                    Ok(_) => {}
                    Err(_) if Instant::now() < deadline => {}
                    Err(_) => panic!("{what}: timed out"),
                }
            }
        };
        let browser = |cookie: &str, path: &str, body: Value| {
            ureq::post(&format!("{base}{path}"))
                .set("Cookie", &format!("q0sid={cookie}"))
                .set("Origin", &base)
                .set("Content-Type", "application/json")
                .send_string(&body.to_string())
                .ok()
                .and_then(|r| r.into_string().ok())
                .and_then(|text| serde_json::from_str::<Value>(&text).ok())
                .expect("the friend's request failed")
        };
        transport.send(Command::Login);
        let Reply::Login {
            user_code,
            verification_uri,
            ..
        } = wait("device request", &mut |r| matches!(r, Reply::Login { .. }))
        else {
            unreachable!()
        };
        let agent = ureq::AgentBuilder::new().redirects(0).build();
        let page = agent
            .get(&verification_uri)
            .set("Cookie", &format!("q0sid={cookie}"))
            .call()
            .unwrap()
            .into_string()
            .unwrap();
        let confirmation = page
            .split("name=\"confirmation\" value=\"")
            .nth(1)
            .and_then(|rest| rest.split('"').next())
            .expect("confirmation field");
        agent
            .post(&format!("{base}/online/authorize"))
            .set("Cookie", &format!("q0sid={cookie}"))
            .set("Origin", &base)
            .send_form(&[
                ("user_code", user_code.as_str()),
                ("confirmation", confirmation),
                ("decision", "approve"),
            ])
            .unwrap();
        let Reply::SignedIn(user) = wait("sign in", &mut |r| matches!(r, Reply::SignedIn(_)))
        else {
            unreachable!()
        };
        let me = user["id"].as_i64().unwrap();

        transport.send(Command::Join { id: room.clone() });
        wait("join", &mut |r| matches!(r, Reply::Snapshot(_)));
        // the first poll brings the recent conversation
        let mut seen = Vec::new();
        wait("chat history", &mut |r| match r {
            Reply::Events { batch, .. } => {
                seen.extend(batch.chat.iter().map(|line| line.id));
                !batch.chat.is_empty()
            }
            _ => false,
        });
        let said = browser(
            &friend,
            &format!("/api/v1/rooms/{room}/chat"),
            json!({"body":"live from the friend"}),
        );
        let friend_id = said["message"]["user_id"].as_i64().unwrap();
        let Reply::Events { batch, .. } = wait(
            "live chat",
            &mut |r| matches!(r, Reply::Events { batch, .. } if !batch.chat.is_empty()),
        ) else {
            unreachable!()
        };
        assert_eq!(batch.chat.len(), 1, "only the new line arrives");
        assert_eq!(batch.chat[0].body, "live from the friend");
        assert!(!seen.contains(&batch.chat[0].id));

        transport.send(Command::Say {
            room: room.clone(),
            body: "hello from q0editor".into(),
        });
        let Reply::ChatSent { message, .. } =
            wait("say", &mut |r| matches!(r, Reply::ChatSent { .. }))
        else {
            unreachable!()
        };
        assert_eq!(message["user_id"], me);

        browser(
            &friend,
            &format!("/api/v1/messages/{me}"),
            json!({"body":"psst, a direct message"}),
        );
        wait("inbox", &mut |r| match r {
            Reply::Inbox(value) => value["messages"]
                .as_array()
                .is_some_and(|m| m.iter().any(|m| m["body"] == "psst, a direct message")),
            _ => false,
        });
        transport.send(Command::Thread { user_id: friend_id });
        let Reply::Thread { value, .. } =
            wait("thread", &mut |r| matches!(r, Reply::Thread { .. }))
        else {
            unreachable!()
        };
        assert!(!value["messages"].as_array().unwrap().is_empty());
        transport.send(Command::SendDm {
            user_id: friend_id,
            body: "got it".into(),
        });
        let Reply::DmSent(message) = wait("send dm", &mut |r| matches!(r, Reply::DmSent(_))) else {
            unreachable!()
        };
        assert_eq!(message["to"], friend_id);
        transport.send(Command::LeaveRoom);
    }
}
