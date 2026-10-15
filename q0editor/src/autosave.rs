//! Background recovery snapshots of unsaved documents.
//!
//! Each running editor owns one locked session directory below
//! `<local data>/q0editor/recovery/`. Dirty documents are written there on a
//! timer, off the UI thread, through the same verified path as a normal save.
//! Saving, discarding or closing a document removes its snapshot. A session
//! directory whose lock is free on a later launch belonged to an editor that
//! did not exit cleanly; its snapshots are offered on Home.
use std::collections::{HashMap, VecDeque};
use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use q0s_format::v2::ProjectV2;
use serde::{Deserialize, Serialize};

use crate::app::EditorApp;
use crate::file_io;

pub const DEFAULT_INTERVAL_SECS: u32 = 120;
pub const MIN_INTERVAL_SECS: u32 = 15;
pub const MAX_INTERVAL_SECS: u32 = 3600;

const MANIFEST_VERSION: u32 = 1;
const MAX_MANIFEST_BYTES: u64 = 64 * 1024;
const MAX_RECOVERY_ENTRIES: usize = 64;
const LOCK_FILE: &str = "session.lock";
const POLL_INTERVAL: Duration = Duration::from_millis(250);
const EXIT_WAIT: Duration = Duration::from_secs(10);
/// A pass that is slow to write is repeated proportionally less often, so a
/// very large project cannot keep one core and the disk permanently busy.
const WRITE_COST_BACKOFF: u32 = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum SnapshotKind {
    Project,
    Q0lang,
}

impl SnapshotKind {
    fn extension(self) -> &'static str {
        match self {
            Self::Project => "q1s",
            Self::Q0lang => "q0l",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum SnapshotOrigin {
    /// Written by the timer from a quiescent frame.
    Autosave,
    /// Written while the editor was already failing. It is newer than the
    /// timed snapshot but may capture a half-applied edit, so both are kept.
    Crash,
}

impl SnapshotOrigin {
    fn tag(self) -> &'static str {
        match self {
            Self::Autosave => "autosave",
            Self::Crash => "crash",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Manifest {
    version: u32,
    kind: SnapshotKind,
    origin: SnapshotOrigin,
    title: String,
    #[serde(default)]
    original_path: Option<String>,
    saved_unix_secs: u64,
    data_file: String,
}

/// What the editor reports about one open document tab.
pub(crate) struct DocumentInfo {
    pub tab_id: u64,
    pub kind: SnapshotKind,
    pub title: String,
    pub path: Option<PathBuf>,
    pub dirty: bool,
    /// Changes whenever the document content may have changed.
    pub stamp: u64,
}

pub(crate) enum Payload {
    Project(Box<ProjectV2>),
    Q0lang(String),
}

#[derive(Debug, Clone)]
pub(crate) struct RecoveryEntry {
    manifest: Manifest,
    data_path: PathBuf,
    manifest_path: PathBuf,
}

impl RecoveryEntry {
    pub(crate) fn kind(&self) -> SnapshotKind {
        self.manifest.kind
    }

    pub(crate) fn title(&self) -> &str {
        &self.manifest.title
    }

    pub(crate) fn data_path(&self) -> &Path {
        &self.data_path
    }

    pub(crate) fn original_path(&self) -> Option<PathBuf> {
        self.manifest.original_path.as_deref().map(PathBuf::from)
    }

    pub(crate) fn key(&self) -> &Path {
        &self.manifest_path
    }

    /// "autosave, 4 min ago" or "at crash, just now".
    pub(crate) fn detail(&self) -> String {
        let origin = match self.manifest.origin {
            SnapshotOrigin::Autosave => "autosave",
            SnapshotOrigin::Crash => "at crash",
        };
        format!(
            "{origin}, {}",
            age_label(unix_now().saturating_sub(self.manifest.saved_unix_secs))
        )
    }

    fn remove_files(&self) {
        let _ = fs::remove_file(&self.data_path);
        let _ = fs::remove_file(&self.manifest_path);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DiscardTarget {
    One(PathBuf),
    All,
}

struct SessionDir {
    dir: PathBuf,
    /// Held for as long as this process is responsible for the directory.
    lock: Option<File>,
}

impl SessionDir {
    fn has_manifests(&self) -> bool {
        fs::read_dir(&self.dir).is_ok_and(|entries| {
            entries
                .flatten()
                .any(|entry| entry.path().extension().is_some_and(|ext| ext == "json"))
        })
    }

    /// Delete the directory once no snapshot in it can still be recovered.
    fn remove_if_unused(&mut self) -> bool {
        if self.has_manifests() {
            return false;
        }
        self.lock = None;
        if let Ok(entries) = fs::read_dir(&self.dir) {
            for entry in entries.flatten() {
                let _ = fs::remove_file(entry.path());
            }
        }
        let _ = fs::remove_dir(&self.dir);
        true
    }
}

struct Tracked {
    stamp: u64,
    stem: String,
    data_file: String,
}

struct Pending {
    tab_id: u64,
    tracked: Tracked,
    receiver: Receiver<Result<Duration, String>>,
}

pub(crate) struct Autosave {
    root: Option<PathBuf>,
    session: Option<SessionDir>,
    scanned: bool,
    tracked: HashMap<u64, Tracked>,
    queue: VecDeque<u64>,
    pending: Option<Pending>,
    next_due: Option<Instant>,
    pass_cost: Duration,
    last_error: Option<String>,
    last_snapshot: Option<Instant>,
    recovered: Vec<RecoveryEntry>,
    /// Recovered documents that are open but not yet saved or discarded. Their
    /// snapshot stays on disk in case the recovered project crashes again.
    opened: Vec<(u64, RecoveryEntry)>,
    adopted: Vec<SessionDir>,
    pub(crate) discard_armed: Option<DiscardTarget>,
}

impl Default for Autosave {
    fn default() -> Self {
        Self::with_root(default_root())
    }
}

/// Unit tests construct many editors; none of them may touch the user's real
/// recovery directory.
#[cfg(test)]
fn default_root() -> Option<PathBuf> {
    None
}

#[cfg(not(test))]
fn default_root() -> Option<PathBuf> {
    Some(dirs::data_local_dir()?.join("q0editor").join("recovery"))
}

impl Autosave {
    pub(crate) fn with_root(root: Option<PathBuf>) -> Self {
        Self {
            root,
            session: None,
            scanned: false,
            tracked: HashMap::new(),
            queue: VecDeque::new(),
            pending: None,
            next_due: None,
            pass_cost: Duration::ZERO,
            last_error: None,
            last_snapshot: None,
            recovered: Vec::new(),
            opened: Vec::new(),
            adopted: Vec::new(),
            discard_armed: None,
        }
    }

    pub(crate) fn recovered(&self) -> &[RecoveryEntry] {
        &self.recovered
    }

    pub(crate) fn last_snapshot_age(&self) -> Option<Duration> {
        self.last_snapshot.map(|at| at.elapsed())
    }

    pub(crate) fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    /// The recovered entry now lives in an open tab; keep its files until that
    /// tab is saved or discarded.
    pub(crate) fn mark_recovered_opened(&mut self, index: usize, tab_id: u64) {
        if index < self.recovered.len() {
            let entry = self.recovered.remove(index);
            self.opened.push((tab_id, entry));
        }
        self.discard_armed = None;
    }

    pub(crate) fn discard_recovered(&mut self, index: usize) {
        if index < self.recovered.len() {
            self.recovered.remove(index).remove_files();
        }
        self.discard_armed = None;
        self.prune_adopted();
    }

    pub(crate) fn discard_all_recovered(&mut self) {
        for entry in self.recovered.drain(..) {
            entry.remove_files();
        }
        self.discard_armed = None;
        self.prune_adopted();
    }

    fn prune_adopted(&mut self) {
        self.adopted
            .retain_mut(|session| !session.remove_if_unused());
    }

    fn scan_once(&mut self) -> usize {
        if self.scanned {
            return 0;
        }
        self.scanned = true;
        let Some(root) = self.root.clone() else {
            return 0;
        };
        let own = self.session.as_ref().map(|session| session.dir.clone());
        let (entries, adopted) = scan(&root, own.as_deref());
        self.recovered = entries;
        self.adopted = adopted;
        self.recovered.len()
    }

    fn ensure_session(&mut self) -> io::Result<PathBuf> {
        if let Some(session) = &self.session {
            return Ok(session.dir.clone());
        }
        let root = self.root.clone().ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, "recovery directory is unavailable")
        })?;
        let session = claim_session(&root)?;
        let dir = session.dir.clone();
        self.session = Some(session);
        Ok(dir)
    }

    fn report_error(&mut self, status: &mut String, error: String) {
        // The same failure would otherwise overwrite the status every pass.
        if self.last_error.as_deref() != Some(error.as_str()) {
            *status = format!("autosave failed: {error}");
            self.last_error = Some(error);
        }
    }

    fn finish_pass_if_idle(&mut self, interval: Duration) {
        if self.queue.is_empty() && self.pending.is_none() {
            let backoff = self.pass_cost.saturating_mul(WRITE_COST_BACKOFF);
            self.next_due = Some(Instant::now() + interval.max(backoff));
            self.pass_cost = Duration::ZERO;
        }
    }
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

fn age_label(seconds: u64) -> String {
    match seconds {
        0..=89 => "just now".to_string(),
        90..=5399 => format!("{} min ago", (seconds + 30) / 60),
        5400..=172_799 => format!("{} h ago", (seconds + 1800) / 3600),
        _ => format!("{} days ago", seconds / 86_400),
    }
}

fn claim_session(root: &Path) -> io::Result<SessionDir> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis());
    let dir = root.join(format!("{}-{millis}", std::process::id()));
    fs::create_dir_all(&dir)?;
    let lock = open_lock(&dir)?;
    try_lock_exclusive(&lock)
        .map_err(|error| io::Error::other(format!("could not lock recovery session: {error}")))?;
    Ok(SessionDir {
        dir,
        lock: Some(lock),
    })
}

fn open_lock(dir: &Path) -> io::Result<File> {
    OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join(LOCK_FILE))
}

#[cfg(not(target_os = "android"))]
fn try_lock_exclusive(file: &File) -> io::Result<()> {
    file.try_lock().map_err(io::Error::from)
}

/// std reports file locking as unsupported on Android, although the kernel
/// implements `flock` like any other Linux.
#[cfg(target_os = "android")]
fn try_lock_exclusive(file: &File) -> io::Result<()> {
    use std::os::fd::AsRawFd;

    // SAFETY: `flock` only reads the descriptor, which `file` keeps open.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// Collect the snapshots of every session that no running editor owns.
fn scan(root: &Path, own: Option<&Path>) -> (Vec<RecoveryEntry>, Vec<SessionDir>) {
    let mut entries = Vec::new();
    let mut adopted = Vec::new();
    let Ok(sessions) = fs::read_dir(root) else {
        return (entries, adopted);
    };
    for session in sessions.flatten() {
        let dir = session.path();
        if !dir.is_dir() || own == Some(dir.as_path()) {
            continue;
        }
        // A held lock means another editor instance is alive and still owns
        // these snapshots.
        let Ok(lock) = open_lock(&dir) else { continue };
        if try_lock_exclusive(&lock).is_err() {
            continue;
        }
        let mut session = SessionDir {
            dir,
            lock: Some(lock),
        };
        let before = entries.len();
        if let Ok(files) = fs::read_dir(&session.dir) {
            for file in files.flatten() {
                if entries.len() >= MAX_RECOVERY_ENTRIES {
                    break;
                }
                let path = file.path();
                if path.extension().is_some_and(|ext| ext == "json") {
                    entries.extend(read_entry(&session.dir, &path));
                }
            }
        }
        if entries.len() == before && session.remove_if_unused() {
            continue;
        }
        adopted.push(session);
    }
    entries.sort_by(|left, right| {
        right
            .manifest
            .saved_unix_secs
            .cmp(&left.manifest.saved_unix_secs)
    });
    (entries, adopted)
}

fn read_entry(dir: &Path, manifest_path: &Path) -> Option<RecoveryEntry> {
    if fs::metadata(manifest_path).ok()?.len() > MAX_MANIFEST_BYTES {
        return None;
    }
    let manifest: Manifest = serde_json::from_slice(&fs::read(manifest_path).ok()?).ok()?;
    if manifest.version != MANIFEST_VERSION {
        return None;
    }
    // The data file must be a plain name inside the session directory.
    let name = Path::new(&manifest.data_file);
    if name.file_name() != Some(name.as_os_str()) {
        return None;
    }
    let data_path = dir.join(name);
    if !data_path.is_file() {
        // Nothing left to recover; do not keep the session alive for it.
        let _ = fs::remove_file(manifest_path);
        return None;
    }
    Some(RecoveryEntry {
        manifest,
        data_path,
        manifest_path: manifest_path.to_path_buf(),
    })
}

fn snapshot_names(tab_id: u64, kind: SnapshotKind, origin: SnapshotOrigin) -> (String, String) {
    let stem = format!("tab-{tab_id}.{}", origin.tag());
    let data_file = format!("{stem}.{}", kind.extension());
    (stem, data_file)
}

fn manifest_for(document: &DocumentInfo, origin: SnapshotOrigin, data_file: &str) -> Manifest {
    Manifest {
        version: MANIFEST_VERSION,
        kind: document.kind,
        origin,
        title: document.title.clone(),
        // A path that is not valid Unicode cannot round-trip through JSON;
        // such a document is recovered as an untitled one.
        original_path: document
            .path
            .as_deref()
            .and_then(Path::to_str)
            .map(str::to_owned),
        saved_unix_secs: unix_now(),
        data_file: data_file.to_string(),
    }
}

/// The manifest is written last, so its presence implies complete data.
fn write_snapshot(
    dir: &Path,
    stem: &str,
    manifest: &Manifest,
    payload: &Payload,
) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|error| error.to_string())?;
    let data_path = dir.join(&manifest.data_file);
    match payload {
        Payload::Project(project) => {
            file_io::save_project(&data_path, project).map_err(|error| error.to_string())?
        }
        Payload::Q0lang(text) => file_io::write_bytes_atomic(&data_path, text.as_bytes())
            .map_err(|error| error.to_string())?,
    }
    let bytes = serde_json::to_vec_pretty(manifest).map_err(|error| error.to_string())?;
    file_io::write_bytes_atomic(&dir.join(format!("{stem}.json")), &bytes)
        .map_err(|error| error.to_string())
}

fn remove_snapshot(dir: &Path, tracked: &Tracked) {
    let _ = fs::remove_file(dir.join(format!("{}.json", tracked.stem)));
    let _ = fs::remove_file(dir.join(&tracked.data_file));
}

fn poll_pending(app: &mut EditorApp, interval: Duration) {
    let Some(pending) = app.autosave.pending.as_ref() else {
        return;
    };
    let result = match pending.receiver.try_recv() {
        Ok(result) => result,
        Err(TryRecvError::Empty) => return,
        Err(TryRecvError::Disconnected) => {
            Err("worker stopped before the snapshot was written".to_string())
        }
    };
    let pending = app.autosave.pending.take().expect("pending autosave");
    complete_pending(app, pending, result, interval);
}

fn complete_pending(
    app: &mut EditorApp,
    pending: Pending,
    result: Result<Duration, String>,
    interval: Duration,
) {
    match result {
        Ok(elapsed) => {
            let autosave = &mut app.autosave;
            autosave.tracked.insert(pending.tab_id, pending.tracked);
            autosave.pass_cost += elapsed;
            autosave.last_error = None;
            autosave.last_snapshot = Some(Instant::now());
        }
        Err(error) => app.autosave.report_error(&mut app.session.status, error),
    }
    app.autosave.finish_pass_if_idle(interval);
}

/// Drop the snapshots of documents that were saved, discarded or closed.
fn release_clean(app: &mut EditorApp) {
    let in_flight = app.autosave.pending.as_ref().map(|pending| pending.tab_id);
    let clean = app
        .autosave
        .tracked
        .keys()
        .copied()
        .filter(|tab_id| Some(*tab_id) != in_flight && !app.document_tab_dirty(*tab_id))
        .collect::<Vec<_>>();
    for tab_id in clean {
        if let (Some(tracked), Some(session)) =
            (app.autosave.tracked.remove(&tab_id), &app.autosave.session)
        {
            remove_snapshot(&session.dir, &tracked);
        }
    }

    let mut index = 0;
    let mut removed = false;
    while index < app.autosave.opened.len() {
        if app.document_tab_dirty(app.autosave.opened[index].0) {
            index += 1;
        } else {
            app.autosave.opened.remove(index).1.remove_files();
            removed = true;
        }
    }
    if removed {
        app.autosave.prune_adopted();
    }
}

fn interval_of(app: &EditorApp) -> Duration {
    Duration::from_secs(u64::from(
        app.settings
            .autosave_interval_secs
            .clamp(MIN_INTERVAL_SECS, MAX_INTERVAL_SECS),
    ))
}

fn start_snapshot(app: &mut EditorApp, tab_id: u64) -> Result<(), String> {
    let Some((document, payload)) = app.autosave_snapshot(tab_id) else {
        return Ok(());
    };
    if !document.dirty {
        return Ok(());
    }
    let dir = app
        .autosave
        .ensure_session()
        .map_err(|error| error.to_string())?;
    let (stem, data_file) = snapshot_names(tab_id, document.kind, SnapshotOrigin::Autosave);
    let manifest = manifest_for(&document, SnapshotOrigin::Autosave, &data_file);
    let (sender, receiver) = mpsc::channel();
    let worker_stem = stem.clone();
    std::thread::Builder::new()
        .name("q0editor-autosave".to_string())
        .spawn(move || {
            let started = Instant::now();
            let result =
                write_snapshot(&dir, &worker_stem, &manifest, &payload).map(|()| started.elapsed());
            let _ = sender.send(result);
        })
        .map_err(|error| format!("could not start the autosave worker: {error}"))?;
    app.autosave.pending = Some(Pending {
        tab_id,
        tracked: Tracked {
            stamp: document.stamp,
            stem,
            data_file,
        },
        receiver,
    });
    Ok(())
}

/// Called once per frame. Everything here is cheap unless a snapshot is due.
pub(crate) fn tick(app: &mut EditorApp, ctx: &egui::Context) {
    if app.autosave.root.is_none() {
        return;
    }
    let found = app.autosave.scan_once();
    if found > 0 {
        app.session.status = format!(
            "recovered {found} unsaved document{} from a previous session; see Home",
            if found == 1 { "" } else { "s" }
        );
    }
    let interval = interval_of(app);
    poll_pending(app, interval);
    release_clean(app);
    if !app.settings.autosave_enabled {
        app.autosave.queue.clear();
        app.autosave.next_due = None;
        return;
    }
    if app.autosave.pending.is_some() {
        ctx.request_repaint_after(POLL_INTERVAL);
        return;
    }

    let now = Instant::now();
    if app.autosave.queue.is_empty() {
        let due = *app.autosave.next_due.get_or_insert(now + interval);
        if now < due {
            if app.has_dirty_documents() {
                // egui only repaints on input; make sure the timer still fires.
                ctx.request_repaint_after(due - now);
            }
            return;
        }
        let changed = app
            .autosave_documents()
            .into_iter()
            .filter(|document| {
                document.dirty
                    && app
                        .autosave
                        .tracked
                        .get(&document.tab_id)
                        .is_none_or(|tracked| tracked.stamp != document.stamp)
            })
            .map(|document| document.tab_id)
            .collect::<VecDeque<_>>();
        if changed.is_empty() {
            app.autosave.next_due = Some(now + interval);
            return;
        }
        app.autosave.queue = changed;
    }

    // Never clone the project in the middle of a stroke, drag or playback.
    if app.session.playing || ctx.input(|input| input.pointer.any_down()) {
        ctx.request_repaint_after(POLL_INTERVAL);
        return;
    }
    while let Some(tab_id) = app.autosave.queue.pop_front() {
        match start_snapshot(app, tab_id) {
            Ok(()) if app.autosave.pending.is_some() => {
                ctx.request_repaint_after(POLL_INTERVAL);
                return;
            }
            Ok(()) => {}
            Err(error) => {
                app.autosave.queue.clear();
                app.autosave.report_error(&mut app.session.status, error);
            }
        }
    }
    app.autosave.finish_pass_if_idle(interval);
}

/// Write whatever is newer than the last timed snapshot, synchronously. Called
/// after the UI thread panicked, just before the panic is allowed to continue.
pub(crate) fn emergency_flush(app: &mut EditorApp) {
    if app.autosave.root.is_none() || !app.settings.autosave_enabled {
        return;
    }
    for document in app.autosave_documents() {
        let current = app
            .autosave
            .tracked
            .get(&document.tab_id)
            .is_some_and(|tracked| tracked.stamp == document.stamp);
        if !document.dirty || current {
            continue;
        }
        let Some((document, payload)) = app.autosave_snapshot(document.tab_id) else {
            continue;
        };
        let Ok(dir) = app.autosave.ensure_session() else {
            return;
        };
        let (stem, data_file) =
            snapshot_names(document.tab_id, document.kind, SnapshotOrigin::Crash);
        let manifest = manifest_for(&document, SnapshotOrigin::Crash, &data_file);
        let _ = write_snapshot(&dir, &stem, &manifest, &payload);
    }
}

/// Clean exit: every document was saved or discarded, so nothing of this
/// session needs to survive. Snapshots of still-dirty documents are kept.
pub(crate) fn shutdown(app: &mut EditorApp) {
    if let Some(pending) = app.autosave.pending.take() {
        let result = pending
            .receiver
            .recv_timeout(EXIT_WAIT)
            .unwrap_or_else(|_| Err("autosave did not finish before exit".to_string()));
        let interval = interval_of(app);
        complete_pending(app, pending, result, interval);
    }
    release_clean(app);
    if app.autosave.tracked.is_empty() {
        if let Some(mut session) = app.autosave.session.take() {
            if !session.remove_if_unused() {
                app.autosave.session = Some(session);
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    pub(crate) struct TestDir(pub(crate) PathBuf);

    impl TestDir {
        pub(crate) fn new(label: &str) -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "q0editor-autosave-{label}-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).expect("create test recovery root");
            Self(path)
        }

        fn sessions(&self) -> Vec<PathBuf> {
            fs::read_dir(&self.0)
                .into_iter()
                .flatten()
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| path.is_dir())
                .collect()
        }

        /// Names of every snapshot file in every session, sorted.
        pub(crate) fn files(&self) -> Vec<String> {
            let mut files = Vec::new();
            for session in self.sessions() {
                for file in fs::read_dir(session).into_iter().flatten().flatten() {
                    let name = file.file_name().to_string_lossy().into_owned();
                    if name != LOCK_FILE {
                        files.push(name);
                    }
                }
            }
            files.sort();
            files
        }

        pub(crate) fn file(&self, name: &str) -> PathBuf {
            self.sessions()
                .into_iter()
                .map(|session| session.join(name))
                .find(|path| path.is_file())
                .unwrap_or_else(|| panic!("no snapshot file named {name}"))
        }

        pub(crate) fn session_count(&self) -> usize {
            self.sessions().len()
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// Point the editor at a private recovery root with autosave switched on.
    pub(crate) fn enable(app: &mut EditorApp, root: &TestDir) {
        app.autosave = Autosave::with_root(Some(root.0.clone()));
        app.settings.autosave_enabled = true;
        app.settings.autosave_interval_secs = MIN_INTERVAL_SECS;
    }

    /// What a script tab leaves behind when its editor dies without cleanup.
    pub(crate) fn leave_crashed_script(root: &TestDir, text: &str) {
        let session = claim_session(&root.0).expect("claim session");
        let document = DocumentInfo {
            tab_id: 1,
            kind: SnapshotKind::Q0lang,
            title: "intro.q0l".to_string(),
            path: Some(root.0.join("intro.q0l")),
            dirty: true,
            stamp: 1,
        };
        let (stem, data_file) =
            snapshot_names(document.tab_id, document.kind, SnapshotOrigin::Autosave);
        let manifest = manifest_for(&document, SnapshotOrigin::Autosave, &data_file);
        write_snapshot(
            &session.dir,
            &stem,
            &manifest,
            &Payload::Q0lang(text.to_string()),
        )
        .expect("write snapshot");
    }

    /// Make the timer fire now and wait for the background writes.
    pub(crate) fn run_pass(app: &mut EditorApp, ctx: &egui::Context) {
        tick(app, ctx);
        app.autosave.next_due = Some(Instant::now());
        for _ in 0..2000 {
            tick(app, ctx);
            if app.autosave.pending.is_none() && app.autosave.queue.is_empty() {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("autosave pass did not finish");
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::TestDir;
    use super::*;

    fn document(tab_id: u64, kind: SnapshotKind, path: Option<&str>) -> DocumentInfo {
        DocumentInfo {
            tab_id,
            kind,
            title: format!("doc {tab_id}"),
            path: path.map(PathBuf::from),
            dirty: true,
            stamp: 1,
        }
    }

    fn write(session: &SessionDir, document: &DocumentInfo, origin: SnapshotOrigin, text: &str) {
        let (stem, data_file) = snapshot_names(document.tab_id, document.kind, origin);
        let manifest = manifest_for(document, origin, &data_file);
        write_snapshot(
            &session.dir,
            &stem,
            &manifest,
            &Payload::Q0lang(text.to_string()),
        )
        .expect("write snapshot");
    }

    #[test]
    fn snapshots_of_a_live_session_are_not_offered() {
        let root = TestDir::new("live");
        let session = claim_session(&root.0).expect("claim session");
        write(
            &session,
            &document(1, SnapshotKind::Q0lang, Some("a.q0l")),
            SnapshotOrigin::Autosave,
            "live",
        );

        let (entries, adopted) = scan(&root.0, None);
        assert!(entries.is_empty());
        assert!(adopted.is_empty());
        assert!(session.dir.join("tab-1.autosave.q0l").is_file());
    }

    #[test]
    fn snapshots_of_a_dead_session_are_recovered_and_then_removed() {
        let root = TestDir::new("dead");
        let session = claim_session(&root.0).expect("claim session");
        let dir = session.dir.clone();
        write(
            &session,
            &document(4, SnapshotKind::Q0lang, Some("scripts/a.q0l")),
            SnapshotOrigin::Autosave,
            "timed",
        );
        write(
            &session,
            &document(4, SnapshotKind::Q0lang, Some("scripts/a.q0l")),
            SnapshotOrigin::Crash,
            "at crash",
        );
        drop(session);

        let mut autosave = Autosave::with_root(Some(root.0.clone()));
        assert_eq!(autosave.scan_once(), 2);
        assert_eq!(autosave.scan_once(), 0, "the scan runs once per launch");
        let entry = &autosave.recovered()[0];
        assert_eq!(entry.kind(), SnapshotKind::Q0lang);
        assert_eq!(entry.original_path(), Some(PathBuf::from("scripts/a.q0l")));
        assert!(fs::read_to_string(entry.data_path()).is_ok());

        // A second editor starting now must not offer the same snapshots.
        let (entries, _) = scan(&root.0, None);
        assert!(entries.is_empty());

        autosave.discard_recovered(0);
        assert!(dir.is_dir(), "one snapshot is still recoverable");
        autosave.discard_all_recovered();
        assert!(autosave.recovered().is_empty());
        assert!(!dir.exists(), "an emptied session directory is removed");
    }

    #[test]
    fn opened_entries_keep_their_files() {
        let root = TestDir::new("opened");
        let session = claim_session(&root.0).expect("claim session");
        write(
            &session,
            &document(2, SnapshotKind::Q0lang, None),
            SnapshotOrigin::Autosave,
            "text",
        );
        drop(session);

        let mut autosave = Autosave::with_root(Some(root.0.clone()));
        assert_eq!(autosave.scan_once(), 1);
        let data = autosave.recovered()[0].data_path().to_path_buf();
        autosave.mark_recovered_opened(0, 9);
        assert!(autosave.recovered().is_empty());
        assert!(data.is_file());
    }

    #[test]
    fn manifests_cannot_point_outside_their_session() {
        let root = TestDir::new("escape");
        let session = claim_session(&root.0).expect("claim session");
        let outside = root.0.join("outside.q1s");
        fs::write(&outside, b"not a snapshot").expect("write outside file");
        let mut manifest = manifest_for(
            &document(1, SnapshotKind::Project, None),
            SnapshotOrigin::Autosave,
            "../outside.q1s",
        );
        let path = session.dir.join("tab-1.autosave.json");
        fs::write(&path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        assert!(read_entry(&session.dir, &path).is_none());

        manifest.data_file = "missing.q1s".to_string();
        fs::write(&path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        assert!(read_entry(&session.dir, &path).is_none());
        assert!(
            !path.exists(),
            "a manifest without data is removed, not kept forever"
        );
        assert!(outside.is_file());
    }

    #[test]
    fn empty_dead_sessions_are_cleaned_up() {
        let root = TestDir::new("empty");
        let dir = claim_session(&root.0).expect("claim session").dir;
        let (entries, adopted) = scan(&root.0, None);
        assert!(entries.is_empty());
        assert!(adopted.is_empty());
        assert!(!dir.exists());
    }

    #[test]
    fn ages_read_naturally() {
        assert_eq!(age_label(20), "just now");
        assert_eq!(age_label(240), "4 min ago");
        assert_eq!(age_label(7200), "2 h ago");
        assert_eq!(age_label(86_400 * 3), "3 days ago");
    }
}
