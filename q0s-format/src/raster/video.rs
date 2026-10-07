//! Bounded source/frame reuse for software masks, filters and export.
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{Arc, Weak};

use crate::v2::{BitmapAsset, Q0vAsset};

struct IndexedVideo {
    source: Weak<Vec<u8>>,
    media: q0video::q0v::Q0vFile,
    last_used: u64,
}

struct DecodedFrame {
    bitmap: Arc<BitmapAsset>,
    last_used: u64,
}

#[derive(Default)]
struct VideoCache {
    sources: HashMap<usize, IndexedVideo>,
    frames: HashMap<(usize, usize), DecodedFrame>,
    serial: u64,
}

thread_local! {
    // Rendering/export workers do not contend over PNG decoding or pixel data.
    static CACHE: RefCell<VideoCache> = RefCell::new(VideoCache::default());
}

pub(super) fn frame(
    video: &Q0vAsset,
    elapsed_host_frames: u16,
    host_fps: u16,
) -> Option<Arc<BitmapAsset>> {
    CACHE.with(|cache| {
        cache
            .borrow_mut()
            .frame(video, elapsed_host_frames, host_fps)
    })
}

impl VideoCache {
    fn frame(
        &mut self,
        video: &Q0vAsset,
        elapsed_host_frames: u16,
        host_fps: u16,
    ) -> Option<Arc<BitmapAsset>> {
        let spec = q0video::q0v::probe_header(&video.bytes).ok()?.spec;
        if !spec.video || spec.width > u32::from(u16::MAX) || spec.height > u32::from(u16::MAX) {
            return None;
        }
        self.serial = self.serial.wrapping_add(1);
        // The parsed file holds one strong reference. Remove clips whose project
        // and other consumers have released the immutable embedded source.
        self.sources
            .retain(|_, source| source.source.strong_count() > 1);
        self.frames
            .retain(|(source, _), _| self.sources.contains_key(source));
        let source_id = Arc::as_ptr(&video.bytes) as usize;
        if !self.sources.contains_key(&source_id) {
            let media = q0video::q0v::Q0vFile::parse_shared(video.bytes.clone()).ok()?;
            // Bound indexing even when a container has a damaged PNG frame.
            while self.sources.len() >= 8 {
                let oldest = self
                    .sources
                    .iter()
                    .min_by_key(|(_, source)| source.last_used)
                    .map(|(key, _)| *key)?;
                self.sources.remove(&oldest);
                self.frames.retain(|(source, _), _| *source != oldest);
            }
            self.sources.insert(
                source_id,
                IndexedVideo {
                    source: Arc::downgrade(&video.bytes),
                    media,
                    last_used: self.serial,
                },
            );
        }
        let source = self.sources.get_mut(&source_id)?;
        source.last_used = self.serial;
        let frame_index = source.media.spec.video_frame_for_host_frame(
            u32::from(elapsed_host_frames),
            u32::from(host_fps.max(1)),
        )?;
        let key = (source_id, frame_index);
        if let std::collections::hash_map::Entry::Vacant(entry) = self.frames.entry(key) {
            let width = u16::try_from(source.media.spec.width).ok()?;
            let height = u16::try_from(source.media.spec.height).ok()?;
            let rgba = source.media.decode_frame_rgba(frame_index).ok()?;
            entry.insert(DecodedFrame {
                bitmap: Arc::new(BitmapAsset {
                    asset_id: video.asset_id,
                    width,
                    height,
                    rgba,
                }),
                last_used: self.serial,
            });
        }
        let cached = self.frames.get_mut(&key)?;
        cached.last_used = self.serial;
        let bitmap = cached.bitmap.clone();
        let mut bytes: usize = self
            .frames
            .values()
            .map(|frame| frame.bitmap.rgba.len())
            .sum();
        while (bytes > 128 * 1024 * 1024 && self.frames.len() > 1) || self.frames.len() > 64 {
            let oldest = self
                .frames
                .iter()
                .min_by_key(|(_, frame)| frame.last_used)
                .map(|(key, _)| *key)?;
            if let Some(removed) = self.frames.remove(&oldest) {
                bytes = bytes.saturating_sub(removed.bitmap.rgba.len());
            }
        }
        Some(bitmap)
    }
}
