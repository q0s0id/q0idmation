//! Discord Rich Presence: "Editing <project> · frame 12 of 240" under the
//! person's name in Discord, while q0editor runs and Discord is open.
//!
//! Discord's desktop app listens on a local pipe (`discord-ipc-0` to `-9`).
//! A frame on it is an opcode and a length, both little-endian u32, then
//! that much JSON: a handshake with the application id, then one
//! `SET_ACTIVITY` per change. A thread of its own does the talking, so a
//! Discord that is closed, slow or restarting never holds up the editor.
//! Discord takes a few updates every twenty seconds, so they are spaced out.

use std::io::{Read, Write};
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::app::EditorApp;

/// The q0editor application on Discord's developer portal. Empty until one
/// is made; Settings can hold another, for testing.
pub(crate) const APPLICATION_ID: &str = "";
/// The fewest seconds between two updates sent to Discord.
const SPACING: Duration = Duration::from_secs(15);
const RETRY: Duration = Duration::from_secs(20);

/// What Discord shows.
#[derive(Debug, Clone, PartialEq)]
struct Activity {
    details: String,
    state: String,
}

enum Message {
    /// Show this, for the application with this id, since this time.
    Set {
        application: String,
        started: u64,
        activity: Activity,
    },
    Clear,
}

#[derive(Default)]
pub(crate) struct Presence {
    tx: Option<mpsc::Sender<Message>>,
    /// What was last sent, and when.
    sent: Option<(Option<Activity>, Instant)>,
    /// Waiting to be sent once the spacing allows.
    pending: Option<Option<Activity>>,
    started: u64,
}

/// What the editor is doing, as Discord should say it.
fn activity(app: &EditorApp) -> Activity {
    if app.home_visible() || !app.has_active_project() {
        return Activity {
            details: "On the home screen".into(),
            state: String::new(),
        };
    }
    let project = &app.state.project;
    let name = project.meta.name.trim();
    let details = if app.settings.discord_show_project && !name.is_empty() {
        format!("Editing {name}")
    } else {
        "Animating".into()
    };
    let frames = project
        .q0rgs
        .iter()
        .find(|q0rg| q0rg.q0rg_id == app.session.current_q0rg_id)
        .map_or(1, |q0rg| q0rg.frame_count);
    let mut state = if app.session.playing {
        "Watching it play".to_string()
    } else {
        format!(
            "Frame {} of {frames}",
            u32::from(app.session.current_frame) + 1
        )
    };
    if let Some(people) = app.online.room_size() {
        state.push_str(&format!(" · in a room of {people}"));
    }
    Activity { details, state }
}

impl Presence {
    /// Called every frame: works out what to show, and sends it when it
    /// changed and Discord may take another update.
    pub(crate) fn update(app: &mut EditorApp) {
        let application = if app.settings.discord_application_id.trim().is_empty() {
            APPLICATION_ID.to_string()
        } else {
            app.settings.discord_application_id.trim().to_string()
        };
        let wanted =
            (app.settings.discord_presence && !application.is_empty()).then(|| activity(app));
        let presence = &mut app.discord;
        if presence.tx.is_none() {
            if wanted.is_none() {
                return;
            }
            presence.tx = Some(start());
            presence.started = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |since| since.as_secs());
        }
        if presence.sent.as_ref().map(|(sent, _)| sent) != Some(&wanted)
            && presence.pending.as_ref() != Some(&wanted)
        {
            presence.pending = Some(wanted);
        }
        let ready = presence
            .sent
            .as_ref()
            .is_none_or(|(_, at)| at.elapsed() >= SPACING);
        if !ready {
            return;
        }
        let Some(next) = presence.pending.take() else {
            return;
        };
        let message = match &next {
            Some(activity) => Message::Set {
                application,
                started: presence.started,
                activity: Activity {
                    details: clip(&activity.details),
                    state: clip(&activity.state),
                },
            },
            None => Message::Clear,
        };
        if presence
            .tx
            .as_ref()
            .is_some_and(|tx| tx.send(message).is_err())
        {
            presence.tx = None;
        }
        presence.sent = Some((next, Instant::now()));
    }
}

/// Discord takes two to 128 characters a line.
fn clip(text: &str) -> String {
    let mut text: String = text.chars().take(128).collect();
    while text.chars().count() < 2 {
        text.push(' ');
    }
    text
}

fn start() -> mpsc::Sender<Message> {
    let (tx, rx) = mpsc::channel::<Message>();
    let spawned = std::thread::Builder::new()
        .name("discord presence".into())
        .spawn(move || run(rx));
    if let Err(error) = spawned {
        eprintln!("could not start Discord presence: {error}");
    }
    tx
}

/// The thread: keeps a pipe to Discord open while there is something to
/// show, opening it again after Discord restarts.
fn run(rx: mpsc::Receiver<Message>) {
    let mut pipe: Option<(Box<dyn Pipe>, String)> = None;
    let mut last: Option<(String, u64, Activity)> = None;
    let mut next_try = Instant::now();
    loop {
        let message = match rx.recv_timeout(RETRY) {
            Ok(message) => Some(message),
            Err(mpsc::RecvTimeoutError::Timeout) => None,
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        };
        match message {
            Some(Message::Set {
                application,
                started,
                activity,
            }) => last = Some((application, started, activity)),
            Some(Message::Clear) => {
                last = None;
                if let Some((mut open, _)) = pipe.take() {
                    let _ = send_activity(open.as_mut(), None, 0);
                }
                continue;
            }
            None if pipe.is_some() || last.is_none() => continue,
            None => {}
        }
        let Some((application, started, activity)) = last.clone() else {
            continue;
        };
        if pipe.as_ref().is_some_and(|(_, id)| *id != application) {
            pipe = None;
        }
        if pipe.is_none() {
            if Instant::now() < next_try {
                continue;
            }
            next_try = Instant::now() + RETRY;
            pipe = connect(&application).map(|open| (open, application.clone()));
        }
        if let Some((open, _)) = pipe.as_mut() {
            if send_activity(open.as_mut(), Some(&activity), started).is_err() {
                // Discord went away; the next update or retry opens it again.
                pipe = None;
            }
        }
    }
}

trait Pipe: Read + Write + Send {}
impl<T: Read + Write + Send> Pipe for T {}

fn connect(application: &str) -> Option<Box<dyn Pipe>> {
    for n in 0..10 {
        let Some(mut pipe) = open_pipe(n) else {
            continue;
        };
        let hello = json!({"v": 1, "client_id": application});
        if write_frame(pipe.as_mut(), 0, &hello).is_ok() && read_frame(pipe.as_mut()).is_ok() {
            return Some(pipe);
        }
    }
    None
}

#[cfg(windows)]
fn open_pipe(n: u32) -> Option<Box<dyn Pipe>> {
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(format!(r"\\.\pipe\discord-ipc-{n}"))
        .ok()
        .map(|file| Box::new(file) as Box<dyn Pipe>)
}

#[cfg(all(unix, not(target_os = "android")))]
fn open_pipe(n: u32) -> Option<Box<dyn Pipe>> {
    let dir = ["XDG_RUNTIME_DIR", "TMPDIR", "TMP", "TEMP"]
        .iter()
        .find_map(|name| std::env::var(name).ok())
        .unwrap_or_else(|| "/tmp".into());
    let stream = std::os::unix::net::UnixStream::connect(format!("{dir}/discord-ipc-{n}")).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    Some(Box::new(stream))
}

/// Phones have no Discord pipe.
#[cfg(not(any(windows, all(unix, not(target_os = "android")))))]
fn open_pipe(_: u32) -> Option<Box<dyn Pipe>> {
    None
}

fn send_activity(
    pipe: &mut dyn Pipe,
    activity: Option<&Activity>,
    started: u64,
) -> std::io::Result<()> {
    let activity = activity.map(|activity| {
        let mut shown = json!({
            "details": activity.details,
            "timestamps": {"start": started},
            "assets": {
                "large_image": "q0editor",
                "large_text": format!("q0editor {}", env!("CARGO_PKG_VERSION")),
            },
            "buttons": [{"label": "q0id.com", "url": "https://q0id.com"}],
        });
        if !activity.state.trim().is_empty() {
            shown["state"] = Value::String(activity.state.clone());
        }
        shown
    });
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos());
    let frame = json!({
        "cmd": "SET_ACTIVITY",
        "args": {"pid": std::process::id(), "activity": activity},
        "nonce": nonce.to_string(),
    });
    write_frame(pipe, 1, &frame)?;
    read_frame(pipe).map(|_| ())
}

fn write_frame(pipe: &mut dyn Pipe, opcode: u32, body: &Value) -> std::io::Result<()> {
    let body = serde_json::to_vec(body)?;
    let mut frame = Vec::with_capacity(8 + body.len());
    frame.extend_from_slice(&opcode.to_le_bytes());
    frame.extend_from_slice(&(body.len() as u32).to_le_bytes());
    frame.extend_from_slice(&body);
    pipe.write_all(&frame)?;
    pipe.flush()
}

/// One frame back from Discord. A close frame (opcode 2) is an error.
fn read_frame(pipe: &mut dyn Pipe) -> std::io::Result<(u32, Value)> {
    let mut head = [0_u8; 8];
    pipe.read_exact(&mut head)?;
    let opcode = u32::from_le_bytes([head[0], head[1], head[2], head[3]]);
    let length = u32::from_le_bytes([head[4], head[5], head[6], head[7]]) as usize;
    if length > 1 << 20 {
        return Err(std::io::Error::other(
            "Discord sent a frame too big to be one",
        ));
    }
    let mut body = vec![0; length];
    pipe.read_exact(&mut body)?;
    let value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    if opcode == 2 {
        return Err(std::io::Error::other(format!(
            "Discord closed the pipe: {value}"
        )));
    }
    Ok((opcode, value))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Discord's side of a pipe: answers each frame with an empty one.
    struct FakeDiscord {
        written: Vec<u8>,
        answers: std::io::Cursor<Vec<u8>>,
    }

    impl Read for FakeDiscord {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            self.answers.read(buf)
        }
    }

    impl Write for FakeDiscord {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.written.extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn an_activity_goes_out_as_one_discord_frame() {
        let mut answer = Vec::new();
        answer.extend_from_slice(&1_u32.to_le_bytes());
        answer.extend_from_slice(&2_u32.to_le_bytes());
        answer.extend_from_slice(b"{}");
        let mut pipe = FakeDiscord {
            written: Vec::new(),
            answers: std::io::Cursor::new(answer),
        };
        let activity = Activity {
            details: "Editing Kobolt".into(),
            state: "Frame 12 of 240".into(),
        };
        send_activity(&mut pipe, Some(&activity), 1_700_000_000).unwrap();
        let written = &pipe.written;
        assert_eq!(u32::from_le_bytes(written[0..4].try_into().unwrap()), 1);
        let length = u32::from_le_bytes(written[4..8].try_into().unwrap()) as usize;
        assert_eq!(length, written.len() - 8);
        let body: Value = serde_json::from_slice(&written[8..]).unwrap();
        assert_eq!(body["cmd"], "SET_ACTIVITY");
        assert_eq!(body["args"]["activity"]["details"], "Editing Kobolt");
        assert_eq!(body["args"]["activity"]["state"], "Frame 12 of 240");
        assert_eq!(
            body["args"]["activity"]["timestamps"]["start"],
            1_700_000_000
        );
    }

    /// Talks to the Discord running on this machine with a made-up id:
    /// it should answer the handshake with a close saying the id is wrong.
    #[test]
    #[ignore = "needs the Discord app running"]
    fn the_real_discord_answers_a_handshake() {
        let mut pipe = open_pipe(0).expect("Discord is not running");
        write_frame(pipe.as_mut(), 0, &json!({"v": 1, "client_id": "1"})).unwrap();
        let answer = read_frame(pipe.as_mut());
        println!("{answer:?}");
        assert!(answer.unwrap_err().to_string().contains("Client"));
    }

    #[test]
    fn a_close_from_discord_is_an_error() {
        let mut answer = Vec::new();
        answer.extend_from_slice(&2_u32.to_le_bytes());
        let body = br#"{"code":4000,"message":"Invalid Client ID"}"#;
        answer.extend_from_slice(&(body.len() as u32).to_le_bytes());
        answer.extend_from_slice(body);
        let mut pipe = FakeDiscord {
            written: Vec::new(),
            answers: std::io::Cursor::new(answer),
        };
        assert!(read_frame(&mut pipe).is_err());
        assert_eq!(clip("x"), "x ");
        assert_eq!(clip(&"y".repeat(300)).chars().count(), 128);
    }
}
