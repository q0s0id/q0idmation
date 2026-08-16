//! Single-instance document forwarding for q0editor.
//!
//! The first editor process owns a user-specific loopback TCP port. Later
//! launches forward the document path to that process and exit. The listener
//! never opens files itself: it only enqueues a request that `EditorApp`
//! consumes on the UI thread, so all normal tab/dirty/import guards stay in
//! one place.

use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const PROTOCOL_MAGIC: &[u8; 16] = b"q0editor-open-v1";
const REQUEST_OPEN_DOCUMENT: u8 = 1;
const REQUEST_ACTIVATE: u8 = 2;
const ACK_OK: u8 = 0xA5;
const MAX_PATH_BYTES: usize = 64 * 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_millis(700);
const IO_TIMEOUT: Duration = Duration::from_millis(700);
const INSTANCE_PORT_BASE: u16 = 39_000;
const INSTANCE_PORT_SPAN: u32 = 20_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExternalOpenRequest {
    pub path: Option<PathBuf>,
}

#[derive(Clone, Default)]
pub struct ExternalOpenInbox {
    queue: Arc<Mutex<VecDeque<ExternalOpenRequest>>>,
    repaint_ctx: Arc<Mutex<Option<egui::Context>>>,
}

impl ExternalOpenInbox {
    pub fn attach_context(&self, ctx: &egui::Context) {
        if let Ok(mut repaint_ctx) = self.repaint_ctx.lock() {
            *repaint_ctx = Some(ctx.clone());
        }
    }

    pub fn drain(&self) -> Vec<ExternalOpenRequest> {
        let Ok(mut queue) = self.queue.lock() else {
            return Vec::new();
        };
        queue.drain(..).collect()
    }

    fn push(&self, request: ExternalOpenRequest) {
        if let Ok(mut queue) = self.queue.lock() {
            queue.push_back(request);
        }
        if let Ok(repaint_ctx) = self.repaint_ctx.lock() {
            if let Some(ctx) = repaint_ctx.as_ref() {
                ctx.request_repaint();
            }
        }
    }
}

pub struct PrimaryInstance {
    listener: TcpListener,
    inbox: ExternalOpenInbox,
}

pub enum LaunchDisposition {
    Primary(PrimaryInstance),
    Forwarded,
    Secondary,
}

impl PrimaryInstance {
    pub fn start(self) -> ExternalOpenInbox {
        let inbox = self.inbox.clone();
        let server_inbox = self.inbox;
        let listener = self.listener;
        let _ = std::thread::Builder::new()
            .name("q0editor-instance-ipc".to_string())
            .spawn(move || serve(listener, server_inbox));
        inbox
    }
}

/// Claim the editor's user-specific loopback endpoint. If another q0editor
/// already owns it, forward the path (or an activate-only request) and tell
/// the caller to exit. A foreign port owner never prevents q0editor startup:
/// failed handshakes fall back to a normal secondary window.
pub fn claim_or_forward(path: Option<PathBuf>) -> LaunchDisposition {
    let addr = instance_addr();
    match TcpListener::bind(addr) {
        Ok(listener) => LaunchDisposition::Primary(PrimaryInstance {
            listener,
            inbox: ExternalOpenInbox::default(),
        }),
        Err(error) if error.kind() == io::ErrorKind::AddrInUse => {
            if forward_to_addr(addr.into(), path.as_deref()).is_ok() {
                LaunchDisposition::Forwarded
            } else {
                LaunchDisposition::Secondary
            }
        }
        Err(_) => LaunchDisposition::Secondary,
    }
}

fn serve(listener: TcpListener, inbox: ExternalOpenInbox) {
    for incoming in listener.incoming() {
        let Ok(mut stream) = incoming else {
            continue;
        };
        let _ = stream.set_read_timeout(Some(IO_TIMEOUT));
        let _ = stream.set_write_timeout(Some(IO_TIMEOUT));
        let _ = handle_connection(&mut stream, &inbox);
    }
}

fn handle_connection(stream: &mut TcpStream, inbox: &ExternalOpenInbox) -> io::Result<()> {
    let mut magic = [0_u8; PROTOCOL_MAGIC.len()];
    stream.read_exact(&mut magic)?;
    if &magic != PROTOCOL_MAGIC {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "not a q0editor instance request",
        ));
    }

    let mut kind = [0_u8; 1];
    stream.read_exact(&mut kind)?;
    let mut len_bytes = [0_u8; 4];
    stream.read_exact(&mut len_bytes)?;
    let len = u32::from_le_bytes(len_bytes) as usize;
    if len > MAX_PATH_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "forwarded document path is too large",
        ));
    }

    let request = match kind[0] {
        REQUEST_ACTIVATE if len == 0 => ExternalOpenRequest { path: None },
        REQUEST_OPEN_DOCUMENT if len > 0 => {
            let mut bytes = vec![0_u8; len];
            stream.read_exact(&mut bytes)?;
            let path = String::from_utf8(bytes).map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidData, "document path is not utf-8")
            })?;
            ExternalOpenRequest {
                path: Some(PathBuf::from(path)),
            }
        }
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid q0editor instance request",
            ))
        }
    };

    inbox.push(request);
    stream.write_all(&[ACK_OK])?;
    stream.flush()?;
    Ok(())
}

fn forward_to_addr(addr: SocketAddr, path: Option<&Path>) -> io::Result<()> {
    let mut stream = TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT)?;
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(IO_TIMEOUT))?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;

    let (kind, payload) = if let Some(path) = path {
        let payload = path.to_string_lossy().into_owned().into_bytes();
        if payload.is_empty() || payload.len() > MAX_PATH_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "document path cannot be forwarded",
            ));
        }
        (REQUEST_OPEN_DOCUMENT, payload)
    } else {
        (REQUEST_ACTIVATE, Vec::new())
    };

    stream.write_all(PROTOCOL_MAGIC)?;
    stream.write_all(&[kind])?;
    stream.write_all(&(payload.len() as u32).to_le_bytes())?;
    stream.write_all(&payload)?;
    stream.flush()?;

    let mut ack = [0_u8; 1];
    stream.read_exact(&mut ack)?;
    if ack[0] != ACK_OK {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "existing process did not acknowledge q0editor protocol",
        ));
    }
    Ok(())
}

fn instance_addr() -> SocketAddrV4 {
    let identity = dirs::config_dir().unwrap_or_else(|| PathBuf::from("q0editor"));
    let port = port_for_identity(&identity);
    SocketAddrV4::new(Ipv4Addr::LOCALHOST, port)
}

fn port_for_identity(identity: &Path) -> u16 {
    // Stable FNV-1a rather than DefaultHasher, whose output is intentionally
    // not a cross-version protocol guarantee.
    let mut hash = 2_166_136_261_u32;
    for byte in identity.to_string_lossy().to_lowercase().bytes() {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(16_777_619);
    }
    INSTANCE_PORT_BASE + (hash % INSTANCE_PORT_SPAN) as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_launch_forwards_unicode_document_to_primary_inbox() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind test listener");
        let addr = listener.local_addr().expect("listener address");
        let inbox = ExternalOpenInbox::default();
        let server_inbox = inbox.clone();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept forwarded document");
            handle_connection(&mut stream, &server_inbox).expect("handle forwarded document");
        });

        let path = PathBuf::from(r"C:\projects\???? ? ?????????\script.q0lang");
        forward_to_addr(addr, Some(&path)).expect("forward document");
        server.join().expect("join test server");

        assert_eq!(
            inbox.drain(),
            vec![ExternalOpenRequest { path: Some(path) }]
        );
    }

    #[test]
    fn activate_only_request_has_no_fake_document_path() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind test listener");
        let addr = listener.local_addr().expect("listener address");
        let inbox = ExternalOpenInbox::default();
        let server_inbox = inbox.clone();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept activation");
            handle_connection(&mut stream, &server_inbox).expect("handle activation");
        });

        forward_to_addr(addr, None).expect("forward activation");
        server.join().expect("join test server");
        assert_eq!(inbox.drain(), vec![ExternalOpenRequest { path: None }]);
    }

    #[test]
    fn user_identity_maps_into_non_ephemeral_port_range() {
        let port = port_for_identity(Path::new(r"C:\Users\q0s0id\AppData\Roaming"));
        assert!(
            (INSTANCE_PORT_BASE..INSTANCE_PORT_BASE + INSTANCE_PORT_SPAN as u16).contains(&port)
        );
    }
}
