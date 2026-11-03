//! q0editor online controller: account, friends, rooms and document synchronization.
//! the panel lives in `ui`; transport is replaceable; authorization lives in q0online.
mod chat;
#[cfg(test)]
mod freeze;
mod presence;
#[cfg(test)]
mod shots;
mod sync;
mod transport;
mod ui;
use crate::app::{Action, EditorApp};
use crate::state::{History, ToolState};
pub(crate) use chat::CANVAS_RECT;
use egui::Context;
pub(crate) use presence::{paint_stage_edits, paint_timeline_markers, timeline_people};
use q0online::{Member, OperationRequest, Presence, ProjectPatch, Rank, RoomEvent, RoomPolicy};
use q0s_format::v2::ProjectV2;
use serde::Deserialize;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use transport::{Command, FriendAction, Reply, Transport};

/// Public account details shared by the website and editor.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
pub struct Person {
    #[serde(default)]
    pub id: i64,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub pfp_url: String,
    #[serde(default)]
    pub last_seen: i64,
}
#[derive(Clone, Deserialize)]
pub struct RoomSnapshot {
    #[serde(deserialize_with = "string_id")]
    pub id: String,
    pub name: String,
    pub owner_id: i64,
    pub revision: u64,
    pub project: ProjectV2,
    #[serde(deserialize_with = "member_list")]
    pub members: Vec<Member>,
    pub policy: RoomPolicy,
    pub protocol_version: u32,
    #[serde(default)]
    pub people: BTreeMap<String, Person>,
}
#[derive(Deserialize)]
pub struct EventBatch {
    #[serde(default)]
    pub events: Vec<RoomEvent>,
    pub revision: u64,
    #[serde(default, deserialize_with = "member_list")]
    pub members: Vec<Member>,
    #[serde(default)]
    pub policy: RoomPolicy,
    #[serde(default)]
    pub presence: Vec<Value>,
    #[serde(default)]
    pub people: BTreeMap<String, Person>,
    /// Room chat newer than the poll's cursor; absent from servers without chat.
    #[serde(default)]
    pub chat: Vec<chat::ChatMessage>,
}
/// Where another participant is looking. Presence is transient and never saved.
#[derive(Clone, Debug, Deserialize)]
pub struct PresenceRow {
    pub user_id: i64,
    #[serde(default)]
    pub presence: Presence,
}
#[derive(Clone, Deserialize)]
struct FriendRow {
    #[serde(flatten)]
    person: Person,
    #[serde(default)]
    status: String,
    #[serde(default)]
    since: i64,
}
#[derive(Clone, Deserialize)]
struct BlockedRow {
    #[serde(flatten)]
    person: Person,
}
fn viewer_rank() -> Rank {
    Rank::Viewer
}
#[derive(Clone, Deserialize)]
struct RoomRow {
    #[serde(deserialize_with = "string_id")]
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    owner_id: i64,
    #[serde(default)]
    owner_name: String,
    #[serde(default = "viewer_rank")]
    rank: Rank,
    #[serde(default)]
    closed: bool,
    #[serde(default)]
    updated_at: i64,
    #[serde(default)]
    member_count: i64,
}
#[derive(Clone, Deserialize)]
struct InviteRow {
    #[serde(deserialize_with = "string_id")]
    id: String,
    #[serde(default)]
    room_id: String,
    #[serde(default)]
    room_name: String,
    #[serde(default)]
    inviter_id: i64,
    #[serde(default)]
    inviter_name: String,
    #[serde(default)]
    inviter_pfp_url: String,
    #[serde(default)]
    member: Option<Member>,
    #[serde(default)]
    expires_at: i64,
}
#[derive(Clone, Deserialize)]
struct RoomInviteRow {
    #[serde(deserialize_with = "string_id")]
    id: String,
    invitee_id: i64,
    #[serde(default)]
    invitee_name: String,
    #[serde(default)]
    invitee_pfp_url: String,
    #[serde(default)]
    inviter_id: i64,
    #[serde(default)]
    inviter_name: String,
    #[serde(default = "viewer_rank")]
    rank: Rank,
    #[serde(default)]
    expires_at: i64,
}
fn string_id<'de, D: serde::Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    let v = Value::deserialize(d)?;
    match v {
        Value::String(s) => Ok(s),
        Value::Number(n) => Ok(n.to_string()),
        _ => Err(serde::de::Error::custom("invalid room id")),
    }
}
fn member_list<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<Member>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Members {
        List(Vec<Member>),
        Map(BTreeMap<String, Member>),
    }
    Ok(match Members::deserialize(d)? {
        Members::List(v) => v,
        Members::Map(v) => v.into_values().collect(),
    })
}
/// List rows are decoded one by one so a newer server field never hides a list.
fn rows<T: serde::de::DeserializeOwned>(value: &Value, key: &str) -> Vec<T> {
    value
        .get(key)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| serde_json::from_value(item.clone()).ok())
                .collect()
        })
        .unwrap_or_default()
}
fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tone {
    Info,
    Success,
    Warning,
    Error,
}
#[derive(Clone, Debug)]
struct Notice {
    text: String,
    tone: Tone,
}
#[derive(Clone)]
struct Login {
    code: String,
    url: String,
    expires: Instant,
}
enum Avatar {
    Requested,
    Ready(egui::TextureHandle),
    Missing,
}
#[derive(Clone, Copy)]
struct Follow {
    q0rg_id: u16,
    frame: u16,
    navigated: bool,
}
#[derive(Clone)]
struct Pending {
    id: String,
    sent_project: ProjectV2,
}
pub struct RoomSession {
    id: String,
    name: String,
    owner_id: i64,
    tab_id: u64,
    revision: u64,
    confirmed: ProjectV2,
    members: Vec<Member>,
    policy: RoomPolicy,
    presence: Vec<PresenceRow>,
    invites: Vec<RoomInviteRow>,
    pending: Option<Pending>,
    events: BTreeMap<u64, RoomEvent>,
    local_revision: u64,
    changed_at: Option<Instant>,
    history: Option<bool>,
    last_presence: Instant,
    sent_presence: Option<(u16, u16, u16)>,
    sync_job: Option<sync::Job>,
    rejection: Option<String>,
    /// Layers this participant may not edit, shown and enforced as locked.
    read_only: Option<ReadOnly>,
    /// Layers you locked yourself. In a room a lock is a view preference
    /// like outlines: it is never sent, so it never locks anyone else.
    local_locks: BTreeSet<(u16, u16)>,
}
/// The read-only overlay and what it was computed from.
struct ReadOnly {
    revision: u64,
    member: Option<Member>,
    policy: RoomPolicy,
    local: BTreeSet<(u16, u16)>,
    layers: Option<std::sync::Arc<std::collections::HashSet<(u16, u16)>>>,
}
impl RoomSession {
    /// Layers this participant cannot edit, from the server's own grant
    /// matching. Recomputed only when membership, policy or revision changes.
    fn read_only_layers(
        &mut self,
        me: Option<i64>,
    ) -> Option<std::sync::Arc<std::collections::HashSet<(u16, u16)>>> {
        let member = self.member(me).cloned();
        let fresh = self.read_only.as_ref().is_some_and(|cached| {
            cached.revision == self.revision
                && cached.member == member
                && cached.policy == self.policy
                && cached.local == self.local_locks
        });
        if !fresh {
            let project = &self.confirmed;
            let mut locked: std::collections::HashSet<(u16, u16)> = project
                .q0rgs
                .iter()
                .flat_map(|q| q.layers.iter().map(move |l| (q.q0rg_id, l.layer_id)))
                .filter(|(q, l)| {
                    member.as_ref().is_none_or(|m| {
                        !q0online::layer_access(project, &self.policy, m, *q, *l).allowed()
                    })
                })
                .collect();
            locked.extend(self.local_locks.iter().copied());
            self.read_only = Some(ReadOnly {
                revision: self.revision,
                member: member.clone(),
                policy: self.policy.clone(),
                local: self.local_locks.clone(),
                layers: (!locked.is_empty()).then(|| std::sync::Arc::new(locked)),
            });
        }
        self.read_only
            .as_ref()
            .and_then(|cached| cached.layers.clone())
    }
    fn member(&self, user: Option<i64>) -> Option<&Member> {
        self.members.iter().find(|m| Some(m.user_id) == user)
    }
    fn received_revision(&self) -> u64 {
        let mut revision = self.revision;
        while let Some(next) = revision.checked_add(1) {
            if !self.events.contains_key(&next) {
                break;
            }
            revision = next;
        }
        revision
    }
}
pub struct Online {
    pub open: bool,
    server: String,
    transport: Option<Transport>,
    user: Option<Person>,
    login: Option<Login>,
    login_ended: Option<String>,
    pub(crate) room: Option<RoomSession>,
    pending_join: Option<RoomSnapshot>,
    pending_resync: Option<RoomSnapshot>,
    rooms: Vec<RoomRow>,
    friends: Vec<FriendRow>,
    blocked: Vec<Person>,
    invites: Vec<InviteRow>,
    lists_at: Option<Instant>,
    skew: i64,
    people: BTreeMap<i64, Person>,
    results: Option<Vec<Person>>,
    notice: Option<Notice>,
    recovery: Option<u64>,
    rejected_drafts: Vec<ProjectV2>,
    busy: BTreeSet<String>,
    /// Set while the room connection is failing; cleared when it recovers.
    connection: Option<String>,
    avatars: HashMap<i64, Avatar>,
    follow: Option<Follow>,
    counter: u64,
    view: ui::View,
    chat: chat::Chat,
    /// What other people changed lately, outlined on the stage.
    edits: Vec<presence::RemoteEdit>,
    /// Loads attachment previews into textures as they arrive.
    preview_ctx: Context,
}
/// q0id.com, or a development server named by `Q0EDITOR_ONLINE_SERVER`.
/// Read once: `tick` builds a default controller on every frame.
fn default_server() -> &'static str {
    static SERVER: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    SERVER.get_or_init(|| {
        std::env::var("Q0EDITOR_ONLINE_SERVER")
            .ok()
            .and_then(|server| validate_server(&server).ok())
            .unwrap_or_else(|| "https://q0id.com".into())
    })
}
impl Default for Online {
    fn default() -> Self {
        Self {
            open: false,
            server: default_server().into(),
            transport: None,
            user: None,
            login: None,
            login_ended: None,
            room: None,
            pending_join: None,
            pending_resync: None,
            rooms: Vec::new(),
            friends: Vec::new(),
            blocked: Vec::new(),
            invites: Vec::new(),
            lists_at: None,
            skew: 0,
            people: BTreeMap::new(),
            results: None,
            notice: None,
            recovery: None,
            rejected_drafts: Vec::new(),
            busy: BTreeSet::new(),
            connection: None,
            avatars: HashMap::new(),
            follow: None,
            counter: 0,
            view: ui::View::default(),
            chat: chat::Chat::default(),
            edits: Vec::new(),
            preview_ctx: Context::default(),
        }
    }
}
impl Online {
    pub fn connected(&self) -> bool {
        self.room.is_some()
    }
    /// The lock button in a room tab: locks or unlocks the layer for you
    /// only. `None` when `tab` is not the room's document.
    pub(crate) fn toggle_local_lock(
        &mut self,
        tab: Option<u64>,
        q0rg_id: u16,
        layer_id: u16,
    ) -> Option<bool> {
        let room = self.room.as_mut().filter(|room| Some(room.tab_id) == tab)?;
        let key = (q0rg_id, layer_id);
        let locked = !room.local_locks.remove(&key);
        if locked {
            room.local_locks.insert(key);
        }
        Some(locked)
    }
    pub(crate) fn locally_locked(&self, q0rg_id: u16, layer_id: u16) -> bool {
        self.room
            .as_ref()
            .is_some_and(|room| room.local_locks.contains(&(q0rg_id, layer_id)))
    }
    pub(crate) fn editing_tab(&self, tab: Option<u64>) -> bool {
        self.room
            .as_ref()
            .is_some_and(|room| Some(room.tab_id) == tab)
    }
    fn remember_rejected(&mut self, app: &mut EditorApp, project: ProjectV2) {
        if self.rejected_drafts.last() == Some(&project) {
            return;
        }
        app.autosave.preserve_online_change(project.clone());
        // every copy is already on disk in recovery; memory keeps a few to open
        const KEPT_IN_MEMORY: usize = 3;
        if self.rejected_drafts.len() >= KEPT_IN_MEMORY {
            self.rejected_drafts.remove(0);
        }
        self.rejected_drafts.push(project);
        self.view.recovery_hidden = false;
    }
    pub fn request_history(&mut self, redo: bool) {
        if let Some(room) = &mut self.room {
            room.history = Some(redo);
            self.say(
                if redo {
                    "redoing your latest undone edit"
                } else {
                    "undoing your latest edit"
                },
                Tone::Info,
            );
        }
    }
    fn actor(&self) -> Option<i64> {
        self.user.as_ref().map(|user| user.id)
    }
    fn my_member(&self) -> Option<&Member> {
        self.room.as_ref()?.member(self.actor())
    }
    fn say(&mut self, text: impl Into<String>, tone: Tone) {
        self.notice = Some(Notice {
            text: text.into(),
            tone,
        });
    }
    fn busy(&self, key: &str) -> bool {
        self.busy.contains(key)
    }
    fn send(&mut self, c: Command) {
        if let Some(t) = &self.transport {
            if let Some(key) = c.busy_key() {
                self.busy.insert(key);
            }
            t.send(c)
        }
    }
    fn server_now(&self) -> i64 {
        unix_now() + self.skew
    }
    fn learn(&mut self, people: impl IntoIterator<Item = Person>) {
        for person in people {
            if person.id > 0 && !person.username.is_empty() {
                // a new picture replaces the cached one; a partial row keeps it
                let known = self.people.get(&person.id);
                let pfp = if person.pfp_url.is_empty() {
                    known.map(|p| p.pfp_url.clone()).unwrap_or_default()
                } else {
                    person.pfp_url.clone()
                };
                if known.is_some_and(|p| p.pfp_url != pfp) {
                    self.avatars.remove(&person.id);
                }
                let last_seen = person.last_seen.max(known.map_or(0, |p| p.last_seen));
                self.people.insert(
                    person.id,
                    Person {
                        pfp_url: pfp,
                        last_seen,
                        ..person
                    },
                );
            }
        }
    }
    /// A real username whenever the server has told us one.
    fn person(&self, id: i64) -> Person {
        self.people.get(&id).cloned().unwrap_or_else(|| Person {
            id,
            username: format!("member #{id}"),
            ..Person::default()
        })
    }
    fn next_id(&mut self) -> String {
        self.counter = self.counter.wrapping_add(1);
        format!(
            "desktop-{}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos(),
            self.counter
        )
    }
    fn ensure_transport(&mut self, ctx: &Context) -> bool {
        if self.transport.is_some() {
            return true;
        }
        match validate_server(&self.server) {
            Ok(server) => {
                self.server = server.clone();
                self.transport = Some(Transport::new(server, ctx.clone()));
                true
            }
            Err(e) => {
                self.say(e, Tone::Error);
                false
            }
        }
    }
    fn start_sign_in(&mut self, ctx: &Context) {
        self.login_ended = None;
        if self.ensure_transport(ctx) {
            self.send(Command::Login);
        }
    }
    /// Cancelling drops the worker, so a late browser approval is never used.
    fn cancel_sign_in(&mut self) {
        self.transport = None;
        self.login = None;
        self.login_ended = None;
        self.busy.clear();
        self.notice = None;
    }
    pub(crate) fn shutdown(&mut self) {
        if self.room.is_some() {
            self.leave();
        }
        if let Some(transport) = self.transport.take() {
            transport.shutdown();
        }
    }
    fn leave(&mut self) {
        self.room = None;
        q0s_format::v2::set_read_only_layers(None);
        self.pending_join = None;
        self.pending_resync = None;
        self.follow = None;
        self.connection = None;
        self.view.left_room();
        self.chat.left_room();
        self.edits.clear();
        self.say(
            "you left the room. its document stays open as a local copy",
            Tone::Info,
        );
        self.send(Command::LeaveRoom);
    }
    fn signed_out(&mut self, message: &str, tone: Tone) {
        if self.room.is_some() {
            self.leave();
        }
        self.user = None;
        self.login = None;
        self.rooms.clear();
        self.friends.clear();
        self.blocked.clear();
        self.invites.clear();
        self.results = None;
        self.lists_at = None;
        self.busy.clear();
        self.view = ui::View::default();
        self.chat = chat::Chat::default();
        self.say(message, tone);
    }
    fn refresh(&mut self) {
        if !self.busy("refresh") {
            self.send(Command::Refresh);
        }
    }
    fn request_avatar(&mut self, id: i64) {
        if self.avatars.contains_key(&id) || self.transport.is_none() {
            return;
        }
        let url = self.person(id).pfp_url;
        if url.is_empty() {
            self.avatars.insert(id, Avatar::Missing);
            return;
        }
        self.avatars.insert(id, Avatar::Requested);
        self.send(Command::Avatar { user_id: id, url });
    }

    fn receive_sign_in_request(
        &mut self,
        ctx: &Context,
        code: String,
        url: String,
        expires_in: u64,
    ) {
        // Receiving a code is not enough: the original button promises to
        // start browser sign-in, so emit the native platform action once.
        ctx.open_url(egui::OpenUrl::new_tab(url.clone()));
        self.login = Some(Login {
            code,
            url,
            expires: Instant::now() + Duration::from_secs(expires_in),
        });
        self.login_ended = None;
        self.notice = None;
    }

    fn handle(&mut self, _app: &mut EditorApp, ctx: &Context, reply: Reply) {
        match reply {
            Reply::Login {
                user_code,
                verification_uri,
                expires_in,
            } => {
                self.receive_sign_in_request(ctx, user_code, verification_uri, expires_in);
            }
            Reply::LoginEnded { message } => {
                self.login = None;
                self.login_ended = Some(message);
            }
            Reply::SignedIn(user) => {
                let user: Person =
                    serde_json::from_value(user.get("user").cloned().unwrap_or(user))
                        .unwrap_or_default();
                self.learn([user.clone()]);
                self.say(format!("signed in as {}", user.username), Tone::Success);
                self.user = Some(user);
                self.login = None;
                self.login_ended = None;
                self.refresh();
            }
            Reply::People(people) => {
                let found: Vec<Person> = rows(&people, "users");
                self.learn(found.clone());
                self.results = Some(found);
            }
            Reply::SignedOut => self.signed_out("signed out of q0id in this editor", Tone::Info),
            Reply::Lists {
                rooms: room_list,
                friends,
                invites,
            } => {
                if let Some(now) = friends.get("now").and_then(Value::as_i64) {
                    self.skew = now - unix_now();
                }
                self.rooms = rows(&room_list, "rooms");
                self.friends = rows(&friends, "friends");
                self.blocked = rows::<BlockedRow>(&friends, "blocked")
                    .into_iter()
                    .map(|row| row.person)
                    .collect();
                self.invites = rows(&invites, "invites");
                self.invites.retain(|invite| {
                    !self.rooms.iter().any(|room| room.id == invite.room_id)
                        && !self
                            .room
                            .as_ref()
                            .is_some_and(|room| room.id == invite.room_id)
                        && !self
                            .pending_join
                            .as_ref()
                            .is_some_and(|room| room.id == invite.room_id)
                });
                let mut known: Vec<Person> =
                    self.friends.iter().map(|f| f.person.clone()).collect();
                known.extend(self.blocked.iter().cloned());
                known.extend(self.invites.iter().map(|i| Person {
                    id: i.inviter_id,
                    username: i.inviter_name.clone(),
                    pfp_url: i.inviter_pfp_url.clone(),
                    last_seen: 0,
                }));
                self.learn(known);
                self.lists_at = Some(Instant::now());
            }
            Reply::RoomInvites { room, invites } => {
                let mut list: Vec<RoomInviteRow> = rows(&invites, "invites");
                self.learn(list.iter().map(|i| Person {
                    id: i.invitee_id,
                    username: i.invitee_name.clone(),
                    pfp_url: i.invitee_pfp_url.clone(),
                    last_seen: 0,
                }));
                if let Some(session) = self.room.as_mut().filter(|r| r.id == room) {
                    list.retain(|invite| {
                        !session
                            .members
                            .iter()
                            .any(|member| member.user_id == invite.invitee_id)
                    });
                    session.invites = list;
                }
            }
            Reply::Snapshot(snapshot) => {
                let snapshot = *snapshot;
                if snapshot.protocol_version != q0online::PROTOCOL_VERSION {
                    self.say(
                        "this room needs a different version of q0editor. update the editor to join it",
                        Tone::Error,
                    );
                    self.send(Command::LeaveRoom);
                    return;
                }
                self.learn(snapshot.people.values().cloned());
                self.invites.retain(|invite| invite.room_id != snapshot.id);
                if self.room.as_ref().is_some_and(|r| r.id == snapshot.id) {
                    self.pending_resync = Some(snapshot);
                } else {
                    self.pending_join = Some(snapshot);
                    self.open = true;
                }
            }
            Reply::Event { room, event } => {
                if let Some(session) = self.room.as_mut().filter(|r| r.id == room) {
                    if event.revision > session.revision {
                        session.events.insert(event.revision, event);
                    }
                }
            }
            Reply::Events { room, mut batch } => {
                self.learn(batch.people.values().cloned());
                let chat = std::mem::take(&mut batch.chat);
                self.room_chat_arrived(&room, chat);
                if self.room.as_ref().is_some_and(|session| session.id == room) {
                    self.note_remote_edits(&batch.events);
                }
                if let Some(session) = self.room.as_mut().filter(|r| r.id == room) {
                    session.members = batch.members;
                    session.invites.retain(|invite| {
                        !session
                            .members
                            .iter()
                            .any(|member| member.user_id == invite.invitee_id)
                    });
                    session.policy = batch.policy;
                    session.presence = batch
                        .presence
                        .into_iter()
                        .filter_map(|row| serde_json::from_value(row).ok())
                        .collect();
                    for event in batch.events {
                        if event.revision > session.revision {
                            session.events.insert(event.revision, event);
                        }
                    }
                    if let Some(transport) = &self.transport {
                        // Submit replies and polling can arrive out of order.
                        // Never advance past an event which has not arrived yet.
                        transport.seen(session.received_revision());
                    }
                    if batch.revision < session.revision {
                        self.say(
                            "the room went back in time on the server. leave and join again to resync",
                            Tone::Warning,
                        );
                    }
                }
            }
            Reply::Failure {
                message,
                status,
                operation,
                denied,
            } => {
                if status == 401 {
                    self.signed_out(&message, Tone::Warning);
                    return;
                }
                self.say(message, Tone::Error);
                if operation {
                    if let Some(room) = &mut self.room {
                        room.rejection = Some(
                            self.notice
                                .as_ref()
                                .map(|n| n.text.clone())
                                .unwrap_or_default(),
                        );
                        room.sync_job = None;
                    }
                }
                if denied && !operation && self.room.is_some() {
                    // The room tab keeps the latest local state, so nothing is lost.
                    self.room = None;
                    self.pending_resync = None;
                    self.connection = None;
                    self.view.left_room();
                    self.chat.left_room();
                    self.send(Command::LeaveRoom);
                    if let Some(notice) = &mut self.notice {
                        notice.text.push_str(
                            ". your access ended; the document stays open as a local copy",
                        );
                    }
                    self.refresh();
                }
            }
            Reply::Connection { ok, message } => {
                self.connection = (!ok).then_some(message);
            }
            Reply::Notice(notice) => {
                self.say(notice, Tone::Success);
                self.refresh();
            }
            Reply::RoomClosed { room, closed } => {
                if closed && self.room.as_ref().is_some_and(|r| r.id == room) {
                    self.leave();
                    self.say(
                        "the room is closed for everyone. your document stays open as a local copy",
                        Tone::Success,
                    );
                } else {
                    self.say(
                        if closed {
                            "the room is closed"
                        } else {
                            "the room is open again"
                        },
                        Tone::Success,
                    );
                }
                self.refresh();
            }
            Reply::RoomDeleted { room } => {
                if self.room.as_ref().is_some_and(|session| session.id == room) {
                    self.leave();
                }
                self.rooms.retain(|row| row.id != room);
                self.invites.retain(|invite| invite.room_id != room);
                self.say(
                    "the room was deleted. local document copies stay open",
                    Tone::Success,
                );
                self.refresh();
            }
            Reply::Avatar { user_id, image } => {
                let avatar = match image {
                    Some(image) => Avatar::Ready(ctx.load_texture(
                        format!("q0id-avatar-{user_id}"),
                        image,
                        egui::TextureOptions::LINEAR,
                    )),
                    None => Avatar::Missing,
                };
                self.avatars.insert(user_id, avatar);
            }
            Reply::Finished(key) => {
                self.busy.remove(&key);
            }
            reply @ (Reply::ChatSent { .. }
            | Reply::ChatFailed(_)
            | Reply::Inbox(_)
            | Reply::Conversations(_)
            | Reply::Thread { .. }
            | Reply::DmSent(_)
            | Reply::Preview { .. }) => self.handle_chat(reply),
        }
    }

    fn open_pending_join(&mut self, app: &mut EditorApp) {
        let Some(room) = self.pending_join.take() else {
            return;
        };
        match app.open_online_project(room.project.clone()) {
            Some(tab) => {
                self.say(
                    format!(
                        "joined “{}”. it opened in its own tab; your other documents are untouched",
                        room.name
                    ),
                    Tone::Success,
                );
                self.room = Some(RoomSession {
                    id: room.id.clone(),
                    name: room.name,
                    owner_id: room.owner_id,
                    tab_id: tab,
                    revision: room.revision,
                    confirmed: room.project,
                    members: room.members,
                    policy: room.policy,
                    presence: Vec::new(),
                    invites: Vec::new(),
                    pending: None,
                    events: BTreeMap::new(),
                    local_revision: app.state.revision(),
                    changed_at: None,
                    history: None,
                    last_presence: Instant::now() - Duration::from_secs(60),
                    sent_presence: None,
                    sync_job: None,
                    rejection: None,
                    read_only: None,
                    local_locks: BTreeSet::new(),
                });
                app.history = History::new();
                self.recovery = None;
                self.connection = None;
                self.view.joined_room();
                self.send(Command::RoomInvites { room: room.id });
                self.refresh();
            }
            None => {
                // An import or export holds the document; the panel offers a retry
                // instead of copying the room project on every idle frame.
                self.pending_join = Some(room);
                self.view.hold_join = true;
                self.say(
                    "the room is ready, but another task is using the document. open it when that finishes",
                    Tone::Warning,
                );
            }
        }
    }

    fn follow_step(&mut self, app: &mut EditorApp) {
        let Some(target) = self.follow.take() else {
            return;
        };
        if app.session.current_q0rg_id == target.q0rg_id {
            let last = app
                .state
                .project
                .q0rgs
                .iter()
                .find(|q| q.q0rg_id == target.q0rg_id)
                .map(|q| q.frame_count.saturating_sub(1))
                .unwrap_or(0);
            app.session.current_frame = target.frame.min(last);
            app.session.pending_timeline_frame = None;
        } else if !target.navigated
            && app
                .state
                .project
                .q0rgs
                .iter()
                .any(|q| q.q0rg_id == target.q0rg_id)
        {
            match app
                .session
                .breadcrumb
                .iter()
                .position(|q| *q == target.q0rg_id)
            {
                Some(depth) => app.queue(Action::BreadcrumbJumpTo(depth)),
                None => app.queue(Action::EnterQ0rg(target.q0rg_id)),
            }
            self.follow = Some(Follow {
                navigated: true,
                ..target
            });
        } else {
            self.say("that timeline isn’t reachable from here", Tone::Warning);
        }
    }
}

fn validate_server(server: &str) -> Result<String, &'static str> {
    let value = server.trim().trim_end_matches('/');
    if value.contains(['?', '#', '@']) {
        return Err("enter the server origin without a path, query or user information");
    }
    let host = if let Some(host) = value.strip_prefix("https://") {
        host
    } else if let Some(host) = value.strip_prefix("http://") {
        if !(host == "localhost"
            || host.starts_with("localhost:")
            || host == "127.0.0.1"
            || host.starts_with("127.0.0.1:")
            || host == "[::1]"
            || host.starts_with("[::1]:"))
        {
            return Err(
                "online connections use https; http is allowed only for localhost development",
            );
        }
        host
    } else {
        return Err("enter an https server address");
    };
    if host.is_empty() || host.contains('/') || host.chars().any(char::is_whitespace) {
        return Err("enter the server origin without a path");
    }
    Ok(value.into())
}

const LIST_REFRESH: Duration = Duration::from_secs(60);
const PRESENCE_HEARTBEAT: Duration = Duration::from_secs(20);

/// Runs after normal editor actions, with every network operation on a worker.
pub fn tick(app: &mut EditorApp, ctx: &Context) {
    if !app.online.open && app.online.transport.is_none() && !app.online.connected() {
        return;
    }
    let mut online = std::mem::take(&mut app.online);
    online.preview_ctx = ctx.clone();
    if online
        .room
        .as_ref()
        .is_some_and(|room| app.project_for_document_tab(room.tab_id).is_none())
    {
        online.leave();
    }
    online.show_panel(app, ctx);
    online.show_overlay(app, ctx);
    let mut replies = Vec::new();
    if let Some(transport) = &online.transport {
        while let Ok(reply) = transport.rx.try_recv() {
            replies.push(reply);
        }
    }
    for reply in replies {
        online.handle(app, ctx, reply);
    }
    if online.user.is_some()
        && online.open
        && online
            .lists_at
            .is_some_and(|at| at.elapsed() >= LIST_REFRESH)
    {
        online.lists_at = Some(Instant::now());
        online.refresh();
    }
    let idle = !ctx.input(|input| input.pointer.any_down())
        && matches!(app.session.tool_state, ToolState::Idle);
    if idle && online.pending_join.is_some() && online.room.is_none() && !online.view.hold_join {
        online.open_pending_join(app);
    }
    let active = online.editing_tab(app.active_project_tab_id());
    // Layers you can't edit behave as locked while the room tab is active:
    // tools refuse them up front instead of letting you draw and then
    // taking the stroke back. Other documents never see the overlay.
    let me = online.actor();
    let overlay = match online.room.as_mut() {
        Some(room) if active => room.read_only_layers(me),
        _ => None,
    };
    q0s_format::v2::set_read_only_layers(overlay);
    let mut drafts = Vec::new();
    let mut notice = None;
    let mut resync = None;
    let mut outgoing = Vec::new();
    if idle && active {
        online.follow_step(app);
        if let Some(snapshot) = online.pending_resync.take() {
            online.learn(snapshot.people.values().cloned());
            if let Some(room) = online.room.as_mut().filter(|room| room.id == snapshot.id) {
                if app.state.project != room.confirmed {
                    drafts.push(app.state.project.clone());
                }
                room.confirmed = snapshot.project;
                room.revision = snapshot.revision;
                room.members = snapshot.members;
                room.policy = snapshot.policy;
                room.events.clear();
                room.pending = None;
                room.sync_job = None;
                room.rejection = None;
                room.changed_at = None;
                replace_project(app, room.confirmed.clone());
                room.local_revision = app.state.revision();
                if let Some(transport) = &online.transport {
                    transport.seen(room.revision);
                }
            }
        }
        let actor = online.actor();
        let operation_id = online.next_id();
        if let Some(room) = &mut online.room {
            if let Some(message) = room.rejection.take() {
                room.sync_job = None;
                if room.pending.take().is_some() {
                    drafts.push(app.state.project.clone());
                    replace_project(app, room.confirmed.clone());
                    room.local_revision = app.state.revision();
                    room.changed_at = None;
                    room.history = None;
                    notice = Some((format!("{message}. your unshared change is saved; open a local copy when you need it"), Tone::Warning));
                }
            }
            if room.local_revision != app.state.revision() {
                room.local_revision = app.state.revision();
                room.changed_at = Some(Instant::now());
            }
            let completed = room.sync_job.as_ref().and_then(|job| {
                use std::sync::mpsc::TryRecvError;
                match job.rx.try_recv() {
                    Ok(result) => Some((job.local_revision, job.room_revision, result)),
                    Err(TryRecvError::Empty) => None,
                    Err(TryRecvError::Disconnected) => Some((
                        job.local_revision,
                        job.room_revision,
                        Err("the sync worker stopped".into()),
                    )),
                }
            });
            if let Some((local_revision, room_revision, result)) = completed {
                room.sync_job = None;
                // A finished job never overwrites edits made since its snapshot.
                // Events stay queued until a matching result is committed.
                if local_revision == app.state.revision() && room_revision == room.revision {
                    match result {
                        Ok(result) => {
                            room.confirmed = result.confirmed;
                            room.revision = result.revision;
                            room.pending = result.pending;
                            room.events.retain(|revision, _| *revision > room.revision);
                            if let Some(rejected) = result.rejected {
                                drafts.push(rejected);
                                notice = Some(("the same resource changed remotely. your unshared version is saved and can be opened as a local copy".into(),Tone::Warning));
                            }
                            replace_project(app, result.project);
                            room.local_revision = app.state.revision();
                            if result.acknowledged {
                                app.history = History::new();
                            }
                            let newer_events = room.events.contains_key(&(room.revision + 1));
                            if let Some(patch) = result.patch.filter(|_| !newer_events) {
                                room.changed_at = None;
                                if !patch.is_empty() {
                                    let allowed = room.members.iter().any(|member| {
                                        Some(member.user_id) == actor
                                            && !member.banned
                                            && member.rank != Rank::Viewer
                                    });
                                    if allowed {
                                        room.pending = Some(Pending {
                                            id: operation_id.clone(),
                                            sent_project: app.state.project.clone(),
                                        });
                                        outgoing.push(Command::Submit {
                                            room: room.id.clone(),
                                            request: OperationRequest {
                                                operation_id: operation_id.clone(),
                                                base_revision: room.revision,
                                                patch,
                                            },
                                        });
                                    } else {
                                        drafts.push(app.state.project.clone());
                                        replace_project(app, room.confirmed.clone());
                                        room.local_revision = app.state.revision();
                                        room.history = None;
                                        notice = Some(("you can only watch this room. your unshared change is saved and can be opened as a local copy".into(),Tone::Warning));
                                    }
                                } else if let Some(redo) = room.history.take() {
                                    outgoing.push(Command::History {
                                        room: room.id.clone(),
                                        operation_id: operation_id.clone(),
                                        redo,
                                    });
                                }
                            }
                            if app.state.project != room.confirmed && room.pending.is_none() {
                                room.changed_at
                                    .get_or_insert(Instant::now() - Duration::from_millis(180));
                            }
                        }
                        Err(error) => {
                            room.changed_at = None;
                            resync = Some(room.id.clone());
                            notice = Some((
                                format!("room state needs a resync ({error}); reloading it"),
                                Tone::Warning,
                            ));
                        }
                    }
                }
            }
            let prepare = room.pending.is_none()
                && (room
                    .changed_at
                    .is_some_and(|at| at.elapsed() >= Duration::from_millis(180))
                    || room.history.is_some());
            if room.sync_job.is_none()
                && (prepare || room.events.contains_key(&(room.revision + 1)))
                && resync.is_none()
            {
                room.sync_job = Some(sync::Job::start(
                    room,
                    app.state.project.clone(),
                    app.state.revision(),
                    prepare,
                    ctx,
                ));
            }
        }
    }
    // Inactive room tabs keep their connection and queue document events. They
    // never read or replace the unrelated document currently being edited.
    if let Some(room) = &mut online.room {
        let place = if active {
            (
                app.session.current_q0rg_id,
                app.session.current_layer_id,
                app.session.current_frame,
            )
        } else {
            room.sent_presence
                .unwrap_or((room.confirmed.meta.entry_q0rg_id, 0, 0))
        };
        let moved = room.sent_presence != Some(place)
            && room.last_presence.elapsed() >= Duration::from_millis(750);
        if moved || room.last_presence.elapsed() >= PRESENCE_HEARTBEAT {
            room.last_presence = Instant::now();
            room.sent_presence = Some(place);
            outgoing.push(Command::Presence {
                room: room.id.clone(),
                q0rg_id: place.0,
                layer_id: place.1,
                frame: place.2,
            });
        }
    }
    for project in drafts {
        online.remember_rejected(app, project);
    }
    if let Some((text, tone)) = notice {
        online.say(text, tone);
    }
    if let Some(id) = resync {
        online.send(Command::Join { id });
    }
    for command in outgoing {
        online.send(command);
    }
    if online.room.is_some() || online.login.is_some() || !online.busy.is_empty() {
        ctx.request_repaint_after(Duration::from_millis(100));
    }
    app.online = online;
}

fn replace_project(app: &mut EditorApp, project: ProjectV2) {
    if app.state.project == project {
        return;
    }
    let before = std::mem::replace(&mut app.state.project, project);
    // Reconciliation is built for undo, which stops playback. Someone else's
    // edit must not: keep playing while the room updates underneath.
    let playing = app.session.playing;
    app.session
        .reconcile_after_history(&before, &app.state.project);
    app.session.playing = playing;
    let old_assets: HashMap<_, _> = before
        .assets
        .iter()
        .map(|asset| (asset.id(), asset))
        .collect();
    let new_assets: HashMap<_, _> = app
        .state
        .project
        .assets
        .iter()
        .map(|asset| (asset.id(), asset))
        .collect();
    let changed_assets: BTreeSet<_> = old_assets
        .keys()
        .chain(new_assets.keys())
        .copied()
        .filter(|id| {
            old_assets.get(id) != new_assets.get(id)
                || before.asset_appearances.get(id) != app.state.project.asset_appearances.get(id)
        })
        .collect();
    app.textures.invalidate_assets(changed_assets);
    app.state.mark_dirty();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn regression_room_app() -> EditorApp {
        let mut app = EditorApp::default();
        let project = crate::state::default_project();
        let mut online = Online {
            user: Some(Person {
                id: 1,
                username: "owner".into(),
                ..Default::default()
            }),
            ..Online::default()
        };
        online.pending_join = Some(RoomSnapshot {
            id: "test-room".into(),
            name: "room".into(),
            owner_id: 1,
            revision: 0,
            project,
            members: vec![Member {
                user_id: 1,
                rank: Rank::Owner,
                grants: Vec::new(),
                banned: false,
            }],
            policy: RoomPolicy::default(),
            protocol_version: q0online::PROTOCOL_VERSION,
            people: BTreeMap::new(),
        });
        online.open_pending_join(&mut app);
        app.online = online;
        app
    }

    fn regression_tick(app: &mut EditorApp) {
        let ctx = Context::default();
        let _ = ctx.run(Default::default(), |ctx| tick(app, ctx));
    }

    fn with_stroke(project: &ProjectV2, size: f32) -> ProjectV2 {
        use q0s_format::v2::{
            Anchor, Asset, Path, Placement, Rgba, Target, Transform2D, Tween, Vec2, VectorAsset,
        };
        let mut next = project.clone();
        let id = next.assets.iter().map(Asset::id).max().unwrap_or(0) + 1;
        next.assets.push(Asset::Vector(VectorAsset {
            asset_id: id,
            fill: Some(Rgba {
                r: 9,
                g: 9,
                b: 9,
                a: 255,
            }),
            stroke: None,
            paths: vec![Path {
                closed: true,
                anchors: [[0., 0.], [size, 0.], [size, size]]
                    .into_iter()
                    .map(|[x, y]| Anchor {
                        point: Vec2::new(x, y),
                        in_handle: None,
                        out_handle: None,
                    })
                    .collect(),
            }],
        }));
        next.q0rgs[0].layers[0].placements.push(Placement {
            instance_id: 0,
            frame: 0,
            target: Target::Asset(id),
            transform: Transform2D::IDENTITY,
            pivot: None,
            tween: Tween::None,
            fx: Default::default(),
        });
        next
    }

    #[test]
    fn strokes_drawn_at_once_on_one_layer_merge_instead_of_being_set_aside() {
        let base = crate::state::default_project();
        let mut room = q0online::RoomState::new(1, base.clone()).unwrap();
        room.set_member(
            1,
            Member {
                user_id: 2,
                rank: Rank::Editor,
                grants: Vec::new(),
                banned: false,
            },
        )
        .unwrap();
        let theirs = with_stroke(&base, 7.0);
        room.submit(
            2,
            OperationRequest {
                operation_id: "theirs".into(),
                base_revision: 0,
                patch: ProjectPatch::between(&base, &theirs).unwrap(),
            },
        )
        .unwrap();
        // my unshared stroke: same layer, same frame, same next asset id
        let mine = with_stroke(&base, 9.0);
        let outcome = sync::calculate(
            base.clone(),
            mine,
            0,
            None,
            room.events_since(0).unwrap(),
            true,
        )
        .unwrap();
        assert!(outcome.rejected.is_none(), "nothing is set aside");
        assert_eq!(outcome.project.assets.len(), base.assets.len() + 2);
        assert_eq!(
            outcome.project.q0rgs[0].layers[0].placements.len(),
            base.q0rgs[0].layers[0].placements.len() + 2
        );
        let patch = outcome.patch.expect("my stroke is ready to send");
        assert!(!patch.is_empty());
        // and the room accepts it on top of theirs
        room.submit(
            1,
            OperationRequest {
                operation_id: "mine".into(),
                base_revision: 1,
                patch,
            },
        )
        .unwrap();
        assert_eq!(room.project, outcome.project);
    }

    #[test]
    fn layers_you_cannot_edit_are_locked_only_while_the_room_tab_is_active() {
        let mut app = regression_room_app();
        let (q0rg, layer) = {
            let room = app.online.room.as_mut().unwrap();
            room.owner_id = 9;
            room.policy.editor_grants.clear();
            room.members = vec![
                Member {
                    user_id: 9,
                    rank: Rank::Owner,
                    grants: Vec::new(),
                    banned: false,
                },
                Member {
                    user_id: 1,
                    rank: Rank::Editor,
                    grants: Vec::new(),
                    banned: false,
                },
            ];
            let q = &room.confirmed.q0rgs[0];
            (q.q0rg_id, q.layers[0].layer_id)
        };
        regression_tick(&mut app);
        assert!(
            app.state.project.layer_is_locked(q0rg, layer),
            "nothing assigned yet"
        );
        app.online.room.as_mut().unwrap().members[1].grants = vec![q0online::Grant {
            scope: q0online::Scope::Layer {
                q0rg_id: q0rg,
                layer_id: layer,
            },
        }];
        regression_tick(&mut app);
        assert!(
            !app.state.project.layer_is_locked(q0rg, layer),
            "assigned layer is editable"
        );
        app.online.room.as_mut().unwrap().members[1].grants.clear();
        regression_tick(&mut app);
        let local = app
            .open_online_project(crate::state::default_project())
            .unwrap();
        regression_tick(&mut app);
        assert_eq!(app.active_project_tab_id(), Some(local));
        assert!(
            !app.state.project.layer_is_locked(q0rg, layer),
            "another document never inherits the room's locks"
        );
    }

    #[test]
    fn a_lock_in_a_room_is_yours_alone() {
        let mut app = regression_room_app();
        regression_tick(&mut app);
        let (q0rg, layer) = (app.session.current_q0rg_id, app.session.current_layer_id);
        let before = app.state.project.clone();
        app.handle(&Context::default(), Action::ToggleLayerLock(q0rg, layer));
        regression_tick(&mut app);
        assert_eq!(app.state.project, before, "a room lock is never sent");
        assert!(app.state.project.layer_is_locked(q0rg, layer));
        assert!(app.online.locally_locked(q0rg, layer));
        app.handle(&Context::default(), Action::ToggleLayerLock(q0rg, layer));
        regression_tick(&mut app);
        assert!(!app.state.project.layer_is_locked(q0rg, layer));
        assert_eq!(app.state.project, before);
    }

    #[test]
    fn playback_keeps_running_while_someone_else_edits() {
        let mut app = regression_room_app();
        let base = app.online.room.as_ref().unwrap().confirmed.clone();
        let mut state = q0online::RoomState::new(1, base.clone()).unwrap();
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
        let theirs = with_stroke(&base, 5.0);
        let event = state
            .submit(
                2,
                OperationRequest {
                    operation_id: "theirs".into(),
                    base_revision: 0,
                    patch: ProjectPatch::between(&base, &theirs).unwrap(),
                },
            )
            .unwrap();
        app.online.room.as_mut().unwrap().events.insert(1, event);
        app.session.playing = true;
        for _ in 0..200 {
            regression_tick(&mut app);
            if app.online.room.as_ref().unwrap().revision == 1 {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(
            app.online.room.as_ref().unwrap().revision,
            1,
            "the edit arrived"
        );
        assert_eq!(app.state.project.assets.len(), theirs.assets.len());
        assert!(app.session.playing, "a remote edit must not stop playback");
    }

    #[test]
    fn other_peoples_strokes_are_outlined_and_their_frame_is_marked() {
        let mut app = regression_room_app();
        let base = app.online.room.as_ref().unwrap().confirmed.clone();
        let mut state = q0online::RoomState::new(1, base.clone()).unwrap();
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
        let request = |id: &str, size| OperationRequest {
            operation_id: id.into(),
            base_revision: 0,
            patch: ProjectPatch::between(&base, &with_stroke(&base, size)).unwrap(),
        };
        let theirs = state.submit(2, request("theirs", 5.0)).unwrap();
        let mine = state.submit(1, request("mine", 7.0)).unwrap();
        app.online.note_remote_edits(&[theirs.clone(), mine]);
        assert_eq!(
            app.online.edits.len(),
            1,
            "only someone else's change glows"
        );
        assert_eq!(app.online.edits[0].user_id, 2);
        // the same event arriving again does not count twice
        app.online.note_remote_edits(&[theirs]);
        assert_eq!(app.online.edits.len(), 1);

        let room = app.online.room.as_mut().unwrap();
        let q0rg_id = room.confirmed.meta.entry_q0rg_id;
        for user_id in [1, 2] {
            room.presence.push(PresenceRow {
                user_id,
                presence: Presence {
                    q0rg_id: Some(q0rg_id),
                    layer_id: Some(1),
                    frame: Some(4),
                    x: None,
                    y: None,
                },
            });
        }
        let tab = Some(room.tab_id);
        let markers = app.online.markers(tab, q0rg_id);
        assert_eq!(markers.len(), 1, "you are not marked for yourself");
        assert_eq!((markers[0].user_id, markers[0].frame), (2, 4));
        assert!(app.online.markers(Some(9999), q0rg_id).is_empty());
    }

    #[test]
    fn regression_rejection_does_not_create_document_tabs() {
        let mut app = regression_room_app();
        let tabs = app.autosave_documents().len();
        let original = app.online.room.as_ref().unwrap().confirmed.clone();
        app.state.project.meta.name = "my rejected title".into();
        app.state.mark_dirty();
        let mut online = std::mem::take(&mut app.online);
        online.room.as_mut().unwrap().pending = Some(Pending {
            id: "op".into(),
            sent_project: app.state.project.clone(),
        });
        online.handle(
            &mut app,
            &Context::default(),
            Reply::Failure {
                message: "not allowed".into(),
                status: 403,
                operation: true,
                denied: false,
            },
        );
        app.online = online;
        regression_tick(&mut app);
        assert_eq!(app.autosave_documents().len(), tabs);
        assert_eq!(app.state.project, original);
        assert_eq!(app.online.rejected_drafts[0].meta.name, "my rejected title");
    }

    #[test]
    fn regression_room_connection_allows_switching_and_closing_other_tabs() {
        let mut app = regression_room_app();
        let room_tab = app.active_project_tab_id().unwrap();
        let mut local = crate::state::default_project();
        local.meta.name = "other document".into();
        let local_tab = app
            .open_online_project(local)
            .expect("a connection is not an import/export lock");
        regression_tick(&mut app);
        assert!(app.online.connected());
        assert_eq!(app.state.project.meta.name, "other document");
        assert!(app.online.room.as_ref().unwrap().sync_job.is_none());
        app.state.dirty = false;
        app.handle(&Context::default(), Action::CloseDocumentTab(local_tab));
        assert_eq!(app.active_project_tab_id(), Some(room_tab));
        assert!(app.online.connected());
    }

    #[test]
    fn regression_stale_worker_result_cannot_overwrite_newer_local_edit() {
        let mut app = regression_room_app();
        let room = app.online.room.as_mut().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let result = sync::calculate(
            room.confirmed.clone(),
            room.confirmed.clone(),
            0,
            None,
            Vec::new(),
            false,
        )
        .unwrap();
        tx.send(Ok(result)).unwrap();
        room.sync_job = Some(sync::Job {
            local_revision: app.state.revision(),
            room_revision: 0,
            rx,
        });
        app.state.project.meta.name = "newer edit".into();
        app.state.mark_dirty();
        regression_tick(&mut app);
        assert_eq!(app.state.project.meta.name, "newer edit");
        assert_eq!(app.online.room.as_ref().unwrap().revision, 0);
    }

    #[test]
    fn regression_ack_keeps_edits_made_after_submission() {
        let before = crate::state::default_project();
        let mut sent = before.clone();
        sent.meta.name = "accepted title".into();
        let mut local = sent.clone();
        local.q0rgs[0].layers[0].name = "later local edit".into();
        let mut server = q0online::RoomState::new(1, before.clone()).unwrap();
        let event = server
            .submit(
                1,
                OperationRequest {
                    operation_id: "op".into(),
                    base_revision: 0,
                    patch: ProjectPatch::between(&before, &sent).unwrap(),
                },
            )
            .unwrap();
        let result = sync::calculate(
            before,
            local.clone(),
            0,
            Some(Pending {
                id: "op".into(),
                sent_project: sent.clone(),
            }),
            vec![event],
            false,
        )
        .unwrap();
        assert_eq!(result.confirmed, sent);
        assert_eq!(result.project, local);
        assert!(result.pending.is_none());
        assert!(result.rejected.is_none());
    }

    #[test]
    fn regression_poll_acknowledgement_never_skips_missing_foreign_events() {
        let mut app = regression_room_app();
        let room = app.online.room.as_mut().unwrap();
        let mut server = q0online::RoomState::new(1, room.confirmed.clone()).unwrap();
        let mut events = Vec::new();
        for index in 1..=3 {
            let mut project = server.project.clone();
            project.meta.name = format!("edit {index}");
            events.push(
                server
                    .submit(
                        1,
                        OperationRequest {
                            operation_id: format!("op{index}"),
                            base_revision: server.revision,
                            patch: ProjectPatch::between(&server.project, &project).unwrap(),
                        },
                    )
                    .unwrap(),
            );
        }
        room.events.insert(3, events[2].clone());
        room.events.insert(1, events[0].clone());
        assert_eq!(room.received_revision(), 1);
        room.events.insert(2, events[1].clone());
        assert_eq!(room.received_revision(), 3);
    }

    const SIGN_IN_CODE: &str = "0a12bc34de";
    const SIGN_IN_URL: &str = "https://q0id.com/online/authorize?user_code=0a12bc34de";

    fn sign_in_frame(
        ctx: &Context,
        online: &mut Online,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        let modifiers = events
            .iter()
            .find_map(|event| match event {
                egui::Event::Key { modifiers, .. } => Some(*modifiers),
                _ => None,
            })
            .unwrap_or_default();
        ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(800.0, 600.0),
                )),
                events,
                modifiers,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| online.sign_in_controls(ui));
            },
        )
    }

    fn text_center(shape: &egui::Shape, wanted: &str) -> Option<egui::Pos2> {
        match shape {
            egui::Shape::Text(text) if text.galley.job.text == wanted => {
                Some(text.pos + text.galley.size() * 0.5)
            }
            egui::Shape::Vec(shapes) => shapes.iter().find_map(|s| text_center(s, wanted)),
            _ => None,
        }
    }

    fn click_sign_in_text(ctx: &Context, online: &mut Online, label: &str) -> egui::FullOutput {
        let frame = sign_in_frame(ctx, online, Vec::new());
        let pos = frame
            .shapes
            .iter()
            .find_map(|shape| text_center(&shape.shape, label))
            .unwrap_or_else(|| panic!("sign-in control was not rendered: {label}"));
        let button = |pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::default(),
        };
        let _ = sign_in_frame(
            ctx,
            online,
            vec![egui::Event::PointerMoved(pos), button(true)],
        );
        sign_in_frame(
            ctx,
            online,
            vec![egui::Event::PointerMoved(pos), button(false)],
        )
    }

    fn pending_sign_in() -> Online {
        Online {
            login: Some(Login {
                code: SIGN_IN_CODE.into(),
                url: SIGN_IN_URL.into(),
                expires: Instant::now() + Duration::from_secs(600),
            }),
            ..Online::default()
        }
    }

    #[test]
    fn sign_in_reply_opens_the_native_browser_once() {
        let ctx = Context::default();
        let mut online = Online::default();
        let output = ctx.run(Default::default(), |ctx| {
            online.receive_sign_in_request(ctx, SIGN_IN_CODE.into(), SIGN_IN_URL.into(), 600);
        });
        let opened = output
            .platform_output
            .open_url
            .expect("browser launch request");
        assert_eq!(opened.url, SIGN_IN_URL);
        assert!(opened.new_tab);
        assert_eq!(online.login.as_ref().unwrap().code, SIGN_IN_CODE);
        for _ in 0..3 {
            assert!(
                sign_in_frame(&ctx, &mut online, Vec::new())
                    .platform_output
                    .open_url
                    .is_none(),
                "an idle repaint must not open another browser tab"
            );
        }
    }

    #[test]
    fn copy_buttons_emit_the_exact_code_and_sign_in_link() {
        let ctx = Context::default();
        let mut online = pending_sign_in();
        for (button, expected) in [
            ("Copy code", SIGN_IN_CODE),
            ("Copy sign-in link", SIGN_IN_URL),
        ] {
            let output = click_sign_in_text(&ctx, &mut online, button);
            assert_eq!(output.platform_output.copied_text, expected);
            assert!(output.platform_output.open_url.is_none());
            assert!(online.notice.as_ref().unwrap().text.contains("copied"));
        }
    }

    #[test]
    fn open_browser_button_retries_the_same_sign_in_request() {
        let ctx = Context::default();
        let mut online = pending_sign_in();
        let output = click_sign_in_text(&ctx, &mut online, "Open browser again");
        assert_eq!(output.platform_output.open_url.unwrap().url, SIGN_IN_URL);
        assert_eq!(online.login.as_ref().unwrap().code, SIGN_IN_CODE);
    }

    #[test]
    fn confirmation_code_supports_select_all_and_keyboard_copy() {
        let ctx = Context::default();
        let mut online = pending_sign_in();
        let _ = click_sign_in_text(&ctx, &mut online, SIGN_IN_CODE);
        let _ = sign_in_frame(
            &ctx,
            &mut online,
            vec![egui::Event::Key {
                key: egui::Key::A,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers {
                    ctrl: true,
                    command: true,
                    ..Default::default()
                },
            }],
        );
        let output = sign_in_frame(&ctx, &mut online, vec![egui::Event::Copy]);
        assert_eq!(output.platform_output.copied_text, SIGN_IN_CODE);
    }

    #[test]
    fn ended_sign_in_offers_a_retry_instead_of_a_stale_code() {
        let mut online = pending_sign_in();
        let mut app = EditorApp::default();
        let ctx = Context::default();
        online.handle(
            &mut app,
            &ctx,
            Reply::LoginEnded {
                message: "the sign-in was declined in the browser".into(),
            },
        );
        assert!(
            online.login.is_none(),
            "a declined code must not stay on screen"
        );
        assert!(online.login_ended.as_deref().unwrap().contains("declined"));
    }

    #[test]
    fn server_address_rejects_credentials_and_plain_remote_http() {
        assert!(validate_server("https://q0id.com/").is_ok());
        assert!(validate_server("http://127.0.0.1:8080").is_ok());
        assert!(validate_server("http://evil.invalid").is_err());
        assert!(validate_server("https://name:secret@q0id.com").is_err());
        assert!(validate_server("https://q0id.com/not-an-origin").is_err());
    }
    #[test]
    fn member_snapshot_accepts_browser_lists_and_core_maps() {
        #[derive(Deserialize)]
        struct Wrapper {
            #[serde(deserialize_with = "member_list")]
            members: Vec<Member>,
        }
        let list:Wrapper=serde_json::from_value(serde_json::json!({"members":[{"user_id":7,"rank":"viewer","grants":[],"banned":false}]})).unwrap();
        let map:Wrapper=serde_json::from_value(serde_json::json!({"members":{"7":{"user_id":7,"rank":"viewer","grants":[],"banned":false}}})).unwrap();
        assert_eq!(list.members, map.members);
    }
    #[test]
    fn names_come_from_every_response_and_unknown_rows_are_skipped() {
        let mut online = Online::default();
        let mut app = EditorApp::default();
        let ctx = Context::default();
        online.handle(
            &mut app,
            &ctx,
            Reply::Lists {
                rooms: serde_json::json!({"rooms":[
                    {"id":"a1","name":"walk cycle","owner_id":2,"owner_name":"bram","rank":"editor","closed":false,"updated_at":5,"member_count":3,"new_field":true},
                    {"broken":"row"}]}),
                friends: serde_json::json!({"friends":[{"id":2,"username":"bram","pfp_url":"/files/b.png","status":"accepted","last_seen":1,"since":1}],
                    "blocked":[{"id":9,"username":"noisy","pfp_url":"","since":1}],"now":100}),
                invites: serde_json::json!({"invites":[{"id":"i1","room_id":"r1","room_name":"intro","inviter_id":3,"inviter_name":"cleo",
                    "inviter_pfp_url":"","member":{"user_id":1,"rank":"editor","grants":[],"banned":false},"expires_at":200}]}),
            },
        );
        assert_eq!(
            online.rooms.len(),
            1,
            "a malformed row must not hide the list"
        );
        assert_eq!(online.person(2).username, "bram");
        assert_eq!(online.person(3).username, "cleo");
        assert_eq!(online.blocked[0].username, "noisy");
        assert_eq!(
            online.invites[0].member.as_ref().unwrap().rank,
            Rank::Editor
        );
        assert_eq!(online.person(77).username, "member #77");
    }
    #[test]
    fn remote_disjoint_edit_rebases_unsent_local_work() {
        let before = crate::state::default_project();
        let mut local = before.clone();
        local.meta.name = "my title".into();
        let mut remote = before.clone();
        remote.q0rgs[0].layers[0].name = "their layer".into();
        let local_patch = ProjectPatch::between(&before, &local).unwrap();
        let event_patch = ProjectPatch::between(&before, &remote).unwrap();
        let mut confirmed = before;
        event_patch.apply(&mut confirmed).unwrap();
        local_patch.apply(&mut confirmed).unwrap();
        assert_eq!(confirmed.meta.name, "my title");
        assert_eq!(confirmed.q0rgs[0].layers[0].name, "their layer");
    }
    #[test]
    fn joining_and_rejected_drafts_preserve_local_documents() {
        let mut app = EditorApp::default();
        let mut first = crate::state::default_project();
        first.meta.name = "unsaved local".into();
        let first_id = app.open_online_project(first.clone()).unwrap();
        let mut room = crate::state::default_project();
        room.meta.name = "room".into();
        let room_id = app.open_online_project(room.clone()).unwrap();
        assert_ne!(room_id, first_id);
        assert_eq!(
            app.project_for_document_tab(first_id).unwrap().meta.name,
            "unsaved local"
        );
        let mut rejected = room;
        rejected.meta.name = "my rejected edit".into();
        let recovered = app.preserve_online_draft(None, rejected);
        assert_eq!(app.active_project_tab_id(), Some(room_id));
        assert!(app.document_tab_dirty(recovered));
        assert_eq!(
            app.project_for_document_tab(recovered).unwrap().meta.name,
            "my rejected edit"
        );
        assert_eq!(app.project_for_document_tab(first_id), Some(&first));
    }
    #[test]
    fn a_joined_room_opens_in_its_own_tab_and_ending_access_keeps_it() {
        let mut app = EditorApp::default();
        let mut local = crate::state::default_project();
        local.meta.name = "my unsaved work".into();
        let local_tab = app.open_online_project(local.clone()).unwrap();
        let mut online = Online {
            user: Some(Person {
                id: 1,
                username: "ada".into(),
                ..Person::default()
            }),
            ..Online::default()
        };
        let mut project = crate::state::default_project();
        project.meta.name = "shared".into();
        online.pending_join = Some(RoomSnapshot {
            id: "0123456789abcdef0123456789abcdef".into(),
            name: "shared".into(),
            owner_id: 2,
            revision: 4,
            project: project.clone(),
            members: vec![Member::viewer(1)],
            policy: RoomPolicy::default(),
            protocol_version: q0online::PROTOCOL_VERSION,
            people: BTreeMap::new(),
        });
        online.open_pending_join(&mut app);
        let room_tab = online.room.as_ref().unwrap().tab_id;
        assert_ne!(room_tab, local_tab);
        assert_eq!(app.project_for_document_tab(local_tab), Some(&local));
        let ctx = Context::default();
        online.handle(
            &mut app,
            &ctx,
            Reply::Failure {
                message: "you are not a member of this room".into(),
                status: 403,
                operation: false,
                denied: true,
            },
        );
        assert!(online.room.is_none());
        assert_eq!(app.active_project_tab_id(), Some(room_tab));
        assert_eq!(app.state.project.meta.name, "shared");
        assert!(online.notice.as_ref().unwrap().text.contains("local copy"));
    }
}
