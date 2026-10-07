//! Bounded look-ahead decoding for native timeline video textures.
//! Workers prepare pixels only; GPU uploads stay on the UI thread.
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex, OnceLock, Weak};

use egui::ColorImage;
use q0video::q0v::Q0vFile;

const MAX_FRAMES: usize = 8;
const MAX_BYTES: usize = 64 * 1024 * 1024;
type FrameKey = (usize, usize);

struct DecodeJob {
    media: Arc<Q0vFile>,
    frame: usize,
    active: Weak<AtomicBool>,
    result: mpsc::SyncSender<Option<ColorImage>>,
}

fn decode_sender() -> Option<&'static mpsc::SyncSender<DecodeJob>> {
    static SENDER: OnceLock<Option<mpsc::SyncSender<DecodeJob>>> = OnceLock::new();
    SENDER
        .get_or_init(|| {
            let (sender, receiver) = mpsc::sync_channel::<DecodeJob>(MAX_FRAMES);
            let receiver = Arc::new(Mutex::new(receiver));
            let workers = std::thread::available_parallelism()
                .map_or(1, |count| count.get().saturating_sub(1).clamp(1, 4));
            let mut started = 0;
            for worker in 0..workers {
                let receiver = Arc::clone(&receiver);
                if std::thread::Builder::new()
                    .name(format!("q0e-video-decode-{worker}"))
                    .spawn(move || loop {
                        let job = match receiver.lock() {
                            Ok(receiver) => match receiver.recv() {
                                Ok(job) => job,
                                Err(_) => break,
                            },
                            Err(_) => break,
                        };
                        if !job
                            .active
                            .upgrade()
                            .is_some_and(|active| active.load(Ordering::Relaxed))
                        {
                            continue;
                        }
                        let image = job.media.decode_frame_rgba(job.frame).ok().map(|rgba| {
                            ColorImage::from_rgba_unmultiplied(
                                [
                                    job.media.spec.width as usize,
                                    job.media.spec.height as usize,
                                ],
                                &rgba,
                            )
                        });
                        let _ = job.result.try_send(image);
                    })
                    .is_ok()
                {
                    started += 1;
                }
            }
            (started > 0).then_some(sender)
        })
        .as_ref()
}

struct PendingFrame {
    result: mpsc::Receiver<Option<ColorImage>>,
    active: Arc<AtomicBool>,
    bytes: usize,
}

impl Drop for PendingFrame {
    fn drop(&mut self) {
        self.active.store(false, Ordering::Relaxed);
    }
}

struct ReadyFrame {
    image: Option<ColorImage>,
    last_used: u64,
}

#[derive(Default)]
pub(crate) struct VideoPrefetch {
    pending: HashMap<FrameKey, PendingFrame>,
    ready: HashMap<FrameKey, ReadyFrame>,
    serial: u64,
}

impl VideoPrefetch {
    pub(crate) fn retain_sources(&mut self, ids: &HashSet<usize>) {
        self.pending.retain(|(asset, _), _| ids.contains(asset));
        self.ready.retain(|(asset, _), _| ids.contains(asset));
    }

    pub(crate) fn poll(&mut self) {
        let mut complete = Vec::new();
        self.pending
            .retain(|key, pending| match pending.result.try_recv() {
                Ok(image) => {
                    complete.push((*key, image));
                    false
                }
                Err(mpsc::TryRecvError::Empty) => true,
                Err(mpsc::TryRecvError::Disconnected) => false,
            });
        for (key, image) in complete {
            self.serial = self.serial.wrapping_add(1);
            self.ready.insert(
                key,
                ReadyFrame {
                    image,
                    last_used: self.serial,
                },
            );
        }
    }

    pub(crate) fn take(&mut self, asset: usize, frame: usize) -> Option<Option<ColorImage>> {
        self.ready.remove(&(asset, frame)).map(|ready| ready.image)
    }

    fn reserved_bytes(&self) -> usize {
        self.pending
            .values()
            .map(|frame| frame.bytes)
            .sum::<usize>()
            + self
                .ready
                .values()
                .filter_map(|frame| frame.image.as_ref())
                .map(|image| image.pixels.len() * std::mem::size_of::<egui::Color32>())
                .sum::<usize>()
    }

    pub(crate) fn request(&mut self, asset: usize, frame: usize, media: &Arc<Q0vFile>) {
        let key = (asset, frame);
        if self.pending.contains_key(&key) || self.ready.contains_key(&key) {
            return;
        }
        let Some(bytes) = u64::from(media.spec.width)
            .checked_mul(u64::from(media.spec.height))
            .and_then(|pixels| pixels.checked_mul(4))
        else {
            return;
        };
        let Ok(bytes) = usize::try_from(bytes) else {
            return;
        };
        if bytes > MAX_BYTES {
            return;
        }
        while self.pending.len() + self.ready.len() >= MAX_FRAMES
            || self.reserved_bytes().saturating_add(bytes) > MAX_BYTES
        {
            let Some(oldest) = self
                .ready
                .iter()
                .min_by_key(|(_, frame)| frame.last_used)
                .map(|(key, _)| *key)
            else {
                return;
            };
            self.ready.remove(&oldest);
        }
        let Some(sender) = decode_sender() else {
            return;
        };
        let (result, receiver) = mpsc::sync_channel(1);
        let active = Arc::new(AtomicBool::new(true));
        let job = DecodeJob {
            media: Arc::clone(media),
            frame,
            active: Arc::downgrade(&active),
            result,
        };
        if sender.try_send(job).is_ok() {
            self.pending.insert(
                key,
                PendingFrame {
                    result: receiver,
                    active,
                    bytes,
                },
            );
        }
    }

    /// The horizon is bounded by decoded bytes as well as frame count. Reserve
    /// no more than a few host frames, even for a high source-frame rate.
    pub(crate) fn look_ahead(media: &Q0vFile) -> usize {
        let Some(bytes) = u64::from(media.spec.width)
            .checked_mul(u64::from(media.spec.height))
            .and_then(|pixels| pixels.checked_mul(4))
        else {
            return 0;
        };
        let memory_frames = (MAX_BYTES as u64).checked_div(bytes).unwrap_or(0);
        memory_frames.min(4).min(MAX_FRAMES as u64) as usize
    }
}
