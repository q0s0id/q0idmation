use std::io::{Seek, SeekFrom, Write};

pub const MAGIC: [u8; 4] = *b"Q0V\0";
pub const VERSION: u16 = 1;
pub const MEDIA_TICKS_PER_SECOND: u64 = 10_000_000;
const HEADER_SIZE: u64 = 80;
const INDEX_ENTRY_SIZE: u64 = 24;
const FLAG_VIDEO: u16 = 1;
const FLAG_AUDIO: u16 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Q0vSpec {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub timeline_frames: u32,
    pub video: bool,
    pub audio: bool,
    pub audio_sample_rate: u32,
    pub audio_channels: u16,
}

impl Q0vSpec {
    pub fn validate(self) -> Result<(), String> {
        if !self.video && !self.audio {
            return Err("q0v must contain video, audio, or both".to_string());
        }
        if self.fps == 0 || self.timeline_frames == 0 {
            return Err("q0v timeline must have a positive fps and length".to_string());
        }
        if self.video && (self.width == 0 || self.height == 0) {
            return Err("q0v video dimensions must be positive".to_string());
        }
        if self.audio && (self.audio_sample_rate == 0 || self.audio_channels == 0) {
            return Err("q0v audio format must be positive".to_string());
        }
        Ok(())
    }

    pub fn duration_ticks(self) -> u64 {
        u64::from(self.timeline_frames).saturating_mul(MEDIA_TICKS_PER_SECOND)
            / u64::from(self.fps.max(1))
    }

    pub fn expected_audio_samples_per_channel(self) -> u64 {
        u64::from(self.timeline_frames)
            .saturating_mul(u64::from(self.audio_sample_rate))
            .div_ceil(u64::from(self.fps.max(1)))
    }

    /// Resolve a q0v video frame from elapsed host-timeline frames. The media
    /// starts at placement time, preserves its own fps, and holds its final
    /// frame after the clip ends. Looping is an explicit future clip property,
    /// never an accidental renderer default.
    pub fn video_frame_for_host_frame(
        self,
        elapsed_host_frames: u32,
        host_fps: u32,
    ) -> Option<usize> {
        if !self.video || self.timeline_frames == 0 || host_fps == 0 {
            return None;
        }
        let source = u64::from(elapsed_host_frames).saturating_mul(u64::from(self.fps))
            / u64::from(host_fps);
        Some(source.min(u64::from(self.timeline_frames - 1)) as usize)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Q0vHeader {
    pub spec: Q0vSpec,
    pub audio_samples_per_channel: u64,
}

/// Read only the fixed q0v header. This deliberately avoids cloning or scanning
/// media payloads and is intended for editor hot paths such as library labels
/// and timeline layout. Full file validation still belongs to `Q0vFile::parse`.
pub fn probe_header(bytes: &[u8]) -> Result<Q0vHeader, String> {
    if bytes.len() < HEADER_SIZE as usize {
        return Err("q0v is truncated before its header".to_string());
    }
    if bytes[0..4] != MAGIC {
        return Err("q0v magic is invalid".to_string());
    }
    let version = read_u16(bytes, 4)?;
    if version != VERSION {
        return Err(format!("unsupported q0v version {version}"));
    }
    let flags = read_u16(bytes, 6)?;
    if flags & !(FLAG_VIDEO | FLAG_AUDIO) != 0 {
        return Err("q0v contains unknown stream flags".to_string());
    }
    let video = flags & FLAG_VIDEO != 0;
    let audio = flags & FLAG_AUDIO != 0;
    let spec = Q0vSpec {
        width: read_u32(bytes, 8)?,
        height: read_u32(bytes, 12)?,
        fps: read_u32(bytes, 16)?,
        timeline_frames: read_u32(bytes, 20)?,
        video,
        audio,
        audio_sample_rate: read_u32(bytes, 28)?,
        audio_channels: read_u16(bytes, 32)?,
    };
    spec.validate()?;
    let audio_bits = read_u16(bytes, 34)?;
    if audio && audio_bits != 16 {
        return Err("q0v audio must be signed 16-bit PCM".to_string());
    }
    if !audio && audio_bits != 0 {
        return Err("q0v has audio sample bits without audio".to_string());
    }
    let header_size = read_u32(bytes, 68)?;
    if header_size != HEADER_SIZE as u32 {
        return Err("q0v header size is invalid".to_string());
    }
    Ok(Q0vHeader {
        spec,
        audio_samples_per_channel: read_u64(bytes, 40)?,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameIndex {
    pub pts_ticks: u64,
    pub offset: u64,
    pub length: u32,
}

pub struct Q0vWriter<W: Write + Seek> {
    writer: W,
    spec: Q0vSpec,
    frames: Vec<FrameIndex>,
    audio_offset: Option<u64>,
    audio_samples_per_channel: u64,
    finished: bool,
}

impl<W: Write + Seek> Q0vWriter<W> {
    pub fn new(mut writer: W, spec: Q0vSpec) -> Result<Self, String> {
        spec.validate()?;
        writer
            .write_all(&vec![0_u8; HEADER_SIZE as usize])
            .map_err(|error| format!("write q0v header placeholder: {error}"))?;
        Ok(Self {
            writer,
            spec,
            frames: Vec::with_capacity(if spec.video {
                spec.timeline_frames as usize
            } else {
                0
            }),
            audio_offset: None,
            audio_samples_per_channel: 0,
            finished: false,
        })
    }

    pub fn write_video_frame(&mut self, pts_ticks: u64, png: &[u8]) -> Result<(), String> {
        if !self.spec.video {
            return Err("q0v has no video stream".to_string());
        }
        if self.audio_offset.is_some() {
            return Err("q0v video frames must be written before audio".to_string());
        }
        if self.frames.len() >= self.spec.timeline_frames as usize {
            return Err("q0v received more video frames than its timeline length".to_string());
        }
        if png.len() > u32::MAX as usize {
            return Err("q0v frame payload is too large".to_string());
        }
        let offset = self
            .writer
            .stream_position()
            .map_err(|error| format!("query q0v frame offset: {error}"))?;
        self.writer
            .write_all(png)
            .map_err(|error| format!("write q0v frame: {error}"))?;
        self.frames.push(FrameIndex {
            pts_ticks,
            offset,
            length: png.len() as u32,
        });
        Ok(())
    }

    pub fn write_audio_pcm_i16(&mut self, interleaved: &[i16]) -> Result<(), String> {
        if !self.spec.audio {
            return Err("q0v has no audio stream".to_string());
        }
        let channels = usize::from(self.spec.audio_channels);
        if !interleaved.len().is_multiple_of(channels) {
            return Err("q0v audio buffer is not aligned to whole channel frames".to_string());
        }
        if self.audio_offset.is_none() {
            self.audio_offset = Some(
                self.writer
                    .stream_position()
                    .map_err(|error| format!("query q0v audio offset: {error}"))?,
            );
        }
        let mut bytes = Vec::with_capacity(interleaved.len() * 2);
        for sample in interleaved {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        self.writer
            .write_all(&bytes)
            .map_err(|error| format!("write q0v audio: {error}"))?;
        self.audio_samples_per_channel = self
            .audio_samples_per_channel
            .saturating_add((interleaved.len() / channels) as u64);
        Ok(())
    }

    pub fn write_silence(&mut self, samples_per_channel: u64) -> Result<(), String> {
        if !self.spec.audio {
            return Err("q0v has no audio stream".to_string());
        }
        const CHUNK_FRAMES: usize = 4096;
        let channels = usize::from(self.spec.audio_channels);
        let chunk = vec![0_i16; CHUNK_FRAMES * channels];
        let mut remaining = samples_per_channel;
        while remaining > 0 {
            let frames = remaining.min(CHUNK_FRAMES as u64) as usize;
            self.write_audio_pcm_i16(&chunk[..frames * channels])?;
            remaining -= frames as u64;
        }
        if samples_per_channel == 0 && self.audio_offset.is_none() {
            self.audio_offset = Some(
                self.writer
                    .stream_position()
                    .map_err(|error| format!("query empty q0v audio offset: {error}"))?,
            );
        }
        Ok(())
    }

    pub fn finish(mut self) -> Result<W, String> {
        if self.finished {
            return Err("q0v writer was already finished".to_string());
        }
        if self.spec.video && self.frames.len() != self.spec.timeline_frames as usize {
            return Err(format!(
                "q0v expected {} video frames but received {}",
                self.spec.timeline_frames,
                self.frames.len()
            ));
        }
        if self.spec.audio && self.audio_offset.is_none() {
            self.write_silence(0)?;
        }
        let index_offset = self
            .writer
            .stream_position()
            .map_err(|error| format!("query q0v index offset: {error}"))?;
        for frame in &self.frames {
            write_u64(&mut self.writer, frame.pts_ticks)?;
            write_u64(&mut self.writer, frame.offset)?;
            write_u32(&mut self.writer, frame.length)?;
            write_u32(&mut self.writer, 0)?;
        }
        let end = self
            .writer
            .stream_position()
            .map_err(|error| format!("query q0v end: {error}"))?;
        self.writer
            .seek(SeekFrom::Start(0))
            .map_err(|error| format!("seek q0v header: {error}"))?;
        self.writer
            .write_all(&MAGIC)
            .map_err(|error| format!("write q0v magic: {error}"))?;
        write_u16(&mut self.writer, VERSION)?;
        let flags = if self.spec.video { FLAG_VIDEO } else { 0 }
            | if self.spec.audio { FLAG_AUDIO } else { 0 };
        write_u16(&mut self.writer, flags)?;
        write_u32(&mut self.writer, self.spec.width)?;
        write_u32(&mut self.writer, self.spec.height)?;
        write_u32(&mut self.writer, self.spec.fps)?;
        write_u32(&mut self.writer, self.spec.timeline_frames)?;
        write_u32(&mut self.writer, self.frames.len() as u32)?;
        write_u32(
            &mut self.writer,
            if self.spec.audio {
                self.spec.audio_sample_rate
            } else {
                0
            },
        )?;
        write_u16(
            &mut self.writer,
            if self.spec.audio {
                self.spec.audio_channels
            } else {
                0
            },
        )?;
        write_u16(&mut self.writer, if self.spec.audio { 16 } else { 0 })?;
        write_u32(&mut self.writer, 0)?;
        write_u64(&mut self.writer, self.audio_samples_per_channel)?;
        write_u64(&mut self.writer, self.audio_offset.unwrap_or(0))?;
        write_u64(&mut self.writer, index_offset)?;
        write_u32(&mut self.writer, self.frames.len() as u32)?;
        write_u32(&mut self.writer, HEADER_SIZE as u32)?;
        write_u64(&mut self.writer, 0)?;
        self.writer
            .seek(SeekFrom::Start(end))
            .map_err(|error| format!("restore q0v end: {error}"))?;
        self.writer
            .flush()
            .map_err(|error| format!("flush q0v: {error}"))?;
        self.finished = true;
        Ok(self.writer)
    }
}

#[derive(Debug)]
struct ParsedQ0vLayout {
    spec: Q0vSpec,
    frames: Vec<FrameIndex>,
    audio_samples_per_channel: u64,
    audio_offset: usize,
    audio_length: usize,
}

fn parse_layout(bytes: &[u8]) -> Result<ParsedQ0vLayout, String> {
    if bytes.len() < HEADER_SIZE as usize {
        return Err("q0v is truncated before its header".to_string());
    }
    if bytes[0..4] != MAGIC {
        return Err("q0v magic is invalid".to_string());
    }
    let version = read_u16(bytes, 4)?;
    if version != VERSION {
        return Err(format!("unsupported q0v version {version}"));
    }
    let flags = read_u16(bytes, 6)?;
    if flags & !(FLAG_VIDEO | FLAG_AUDIO) != 0 {
        return Err("q0v contains unknown stream flags".to_string());
    }
    let video = flags & FLAG_VIDEO != 0;
    let audio = flags & FLAG_AUDIO != 0;
    let spec = Q0vSpec {
        width: read_u32(bytes, 8)?,
        height: read_u32(bytes, 12)?,
        fps: read_u32(bytes, 16)?,
        timeline_frames: read_u32(bytes, 20)?,
        video,
        audio,
        audio_sample_rate: read_u32(bytes, 28)?,
        audio_channels: read_u16(bytes, 32)?,
    };
    spec.validate()?;
    let video_frames = read_u32(bytes, 24)?;
    let audio_bits = read_u16(bytes, 34)?;
    if audio && audio_bits != 16 {
        return Err("q0v audio must be signed 16-bit pcm".to_string());
    }
    if !audio && (spec.audio_sample_rate != 0 || spec.audio_channels != 0 || audio_bits != 0) {
        return Err("q0v has audio metadata without an audio stream".to_string());
    }
    let audio_samples_per_channel = read_u64(bytes, 40)?;
    let audio_offset = read_u64(bytes, 48)?;
    let index_offset = read_u64(bytes, 56)?;
    let index_entries = read_u32(bytes, 64)?;
    let header_size = read_u32(bytes, 68)?;
    if header_size != HEADER_SIZE as u32 {
        return Err("q0v header size is invalid".to_string());
    }
    if index_entries != video_frames {
        return Err("q0v video frame count and index count disagree".to_string());
    }
    if video && video_frames != spec.timeline_frames {
        return Err("q0v video stream does not cover the full timeline".to_string());
    }
    if !video && video_frames != 0 {
        return Err("q0v has indexed frames without a video stream".to_string());
    }
    let index_end = index_offset
        .checked_add(u64::from(index_entries).saturating_mul(INDEX_ENTRY_SIZE))
        .ok_or_else(|| "q0v index overflows the file".to_string())?;
    if index_offset < HEADER_SIZE || index_end != bytes.len() as u64 {
        return Err("q0v index bounds or trailing bytes are invalid".to_string());
    }
    let audio_length = if audio {
        let length = audio_samples_per_channel
            .checked_mul(u64::from(spec.audio_channels))
            .and_then(|samples| samples.checked_mul(2))
            .ok_or_else(|| "q0v audio length overflows".to_string())?;
        if audio_offset < HEADER_SIZE || audio_offset.saturating_add(length) != index_offset {
            return Err("q0v audio payload bounds are invalid".to_string());
        }
        length
    } else {
        if audio_offset != 0 || audio_samples_per_channel != 0 {
            return Err("q0v has audio payload metadata without audio".to_string());
        }
        0
    };

    let mut frames: Vec<FrameIndex> = Vec::with_capacity(index_entries as usize);
    for index in 0..index_entries {
        let base = index_offset as usize + index as usize * INDEX_ENTRY_SIZE as usize;
        let frame = FrameIndex {
            pts_ticks: read_u64(bytes, base)?,
            offset: read_u64(bytes, base + 8)?,
            length: read_u32(bytes, base + 16)?,
        };
        let end = frame
            .offset
            .checked_add(u64::from(frame.length))
            .ok_or_else(|| "q0v frame payload overflows".to_string())?;
        let payload_limit = if audio { audio_offset } else { index_offset };
        if frame.offset < HEADER_SIZE || end > payload_limit {
            return Err("q0v frame payload bounds are invalid".to_string());
        }
        if let Some(previous) = frames.last() {
            if frame.pts_ticks < previous.pts_ticks {
                return Err("q0v frame timestamps are not monotonic".to_string());
            }
        }
        frames.push(frame);
    }
    Ok(ParsedQ0vLayout {
        spec,
        frames,
        audio_samples_per_channel,
        audio_offset: audio_offset as usize,
        audio_length: audio_length as usize,
    })
}

/// Validate the complete q0v container without taking ownership of or copying
/// its media payload. This is intentionally as strict as `Q0vFile::parse`.
pub fn validate_bytes(bytes: &[u8]) -> Result<(), String> {
    parse_layout(bytes).map(|_| ())
}

#[derive(Debug, Clone, Copy)]
pub struct Q0vAudioView<'a> {
    pub spec: Q0vSpec,
    pub samples_per_channel: u64,
    pcm_le_bytes: &'a [u8],
}

impl<'a> Q0vAudioView<'a> {
    pub fn pcm_le_bytes(self) -> &'a [u8] {
        self.pcm_le_bytes
    }
}

/// Borrow the validated PCM payload directly from an existing q0v byte slice.
/// Audio-only projects use this for waveform work without duplicating the song.
pub fn audio_view(bytes: &[u8]) -> Result<Q0vAudioView<'_>, String> {
    let layout = parse_layout(bytes)?;
    if !layout.spec.audio {
        return Err("q0v has no audio stream".to_string());
    }
    let end = layout
        .audio_offset
        .checked_add(layout.audio_length)
        .ok_or_else(|| "q0v audio payload overflows usize".to_string())?;
    Ok(Q0vAudioView {
        spec: layout.spec,
        samples_per_channel: layout.audio_samples_per_channel,
        pcm_le_bytes: &bytes[layout.audio_offset..end],
    })
}

#[derive(Debug, Clone)]
pub struct Q0vFile {
    bytes: Vec<u8>,
    pub spec: Q0vSpec,
    pub frames: Vec<FrameIndex>,
    pub audio_samples_per_channel: u64,
    audio_offset: usize,
    audio_length: usize,
}

impl Q0vFile {
    pub fn parse(bytes: Vec<u8>) -> Result<Self, String> {
        let layout = parse_layout(&bytes)?;
        Ok(Self {
            bytes,
            spec: layout.spec,
            frames: layout.frames,
            audio_samples_per_channel: layout.audio_samples_per_channel,
            audio_offset: layout.audio_offset,
            audio_length: layout.audio_length,
        })
    }

    pub fn frame_png(&self, index: usize) -> Option<&[u8]> {
        let frame = self.frames.get(index)?;
        let start = frame.offset as usize;
        let end = start.checked_add(frame.length as usize)?;
        self.bytes.get(start..end)
    }

    pub fn decode_frame_rgba(&self, index: usize) -> Result<Vec<u8>, String> {
        let payload = self
            .frame_png(index)
            .ok_or_else(|| "q0v frame index is out of range".to_string())?;
        let decoded = image::load_from_memory_with_format(payload, image::ImageFormat::Png)
            .map_err(|error| format!("decode q0v frame png: {error}"))?
            .to_rgba8();
        if decoded.dimensions() != (self.spec.width, self.spec.height) {
            return Err("q0v frame dimensions disagree with the container".to_string());
        }
        Ok(decoded.into_raw())
    }

    pub fn audio_pcm_le_bytes(&self) -> &[u8] {
        if !self.spec.audio {
            return &[];
        }
        &self.bytes[self.audio_offset..self.audio_offset + self.audio_length]
    }
}

fn write_u16(writer: &mut impl Write, value: u16) -> Result<(), String> {
    writer
        .write_all(&value.to_le_bytes())
        .map_err(|error| format!("write q0v u16: {error}"))
}
fn write_u32(writer: &mut impl Write, value: u32) -> Result<(), String> {
    writer
        .write_all(&value.to_le_bytes())
        .map_err(|error| format!("write q0v u32: {error}"))
}
fn write_u64(writer: &mut impl Write, value: u64) -> Result<(), String> {
    writer
        .write_all(&value.to_le_bytes())
        .map_err(|error| format!("write q0v u64: {error}"))
}
fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, String> {
    let raw = bytes
        .get(offset..offset + 2)
        .ok_or_else(|| "q0v is truncated".to_string())?;
    Ok(u16::from_le_bytes([raw[0], raw[1]]))
}
fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, String> {
    let raw = bytes
        .get(offset..offset + 4)
        .ok_or_else(|| "q0v is truncated".to_string())?;
    Ok(u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]))
}
fn read_u64(bytes: &[u8], offset: usize) -> Result<u64, String> {
    let raw = bytes
        .get(offset..offset + 8)
        .ok_or_else(|| "q0v is truncated".to_string())?;
    Ok(u64::from_le_bytes([
        raw[0], raw[1], raw[2], raw[3], raw[4], raw[5], raw[6], raw[7],
    ]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::ImageEncoder;
    use std::io::Cursor;

    fn png(color: [u8; 4]) -> Vec<u8> {
        let mut bytes = Vec::new();
        image::codecs::png::PngEncoder::new(&mut bytes)
            .write_image(&color, 1, 1, image::ColorType::Rgba8)
            .expect("encode png");
        bytes
    }

    #[test]
    fn q0v_round_trips_video_and_audio_with_random_access() {
        let spec = Q0vSpec {
            width: 1,
            height: 1,
            fps: 24,
            timeline_frames: 2,
            video: true,
            audio: true,
            audio_sample_rate: 48_000,
            audio_channels: 2,
        };
        let mut writer = Q0vWriter::new(Cursor::new(Vec::new()), spec).expect("writer");
        writer.write_video_frame(0, &png([255, 0, 0, 255])).unwrap();
        writer
            .write_video_frame(MEDIA_TICKS_PER_SECOND / 24, &png([0, 255, 0, 255]))
            .unwrap();
        writer.write_audio_pcm_i16(&[1, -1, 2, -2]).unwrap();
        let cursor = writer.finish().expect("finish");
        let bytes = cursor.into_inner();
        validate_bytes(&bytes).expect("borrowed validation");
        let audio = audio_view(&bytes).expect("borrowed audio view");
        assert_eq!(audio.spec, spec);
        assert_eq!(audio.samples_per_channel, 2);
        assert_eq!(audio.pcm_le_bytes(), &[1, 0, 255, 255, 2, 0, 254, 255]);
        let parsed = Q0vFile::parse(bytes).expect("parse");
        assert_eq!(parsed.spec, spec);
        assert_eq!(parsed.frames.len(), 2);
        assert_eq!(parsed.decode_frame_rgba(1).unwrap(), vec![0, 255, 0, 255]);
        assert_eq!(
            parsed.audio_pcm_le_bytes(),
            &[1, 0, 255, 255, 2, 0, 254, 255]
        );
    }

    #[test]
    fn q0v_rejects_trailing_bytes_and_empty_stream_sets() {
        let invalid = Q0vSpec {
            width: 1,
            height: 1,
            fps: 24,
            timeline_frames: 1,
            video: false,
            audio: false,
            audio_sample_rate: 48_000,
            audio_channels: 2,
        };
        assert!(invalid.validate().is_err());

        let spec = Q0vSpec {
            video: true,
            audio: false,
            ..invalid
        };
        let mut writer = Q0vWriter::new(Cursor::new(Vec::new()), spec).unwrap();
        writer.write_video_frame(0, &png([0, 0, 0, 255])).unwrap();
        let mut bytes = writer.finish().unwrap().into_inner();
        bytes.push(9);
        assert!(validate_bytes(&bytes).is_err());
        assert!(Q0vFile::parse(bytes).is_err());
    }

    #[test]
    fn video_frame_mapping_uses_host_fps_without_fixed_step_drift() {
        let spec = Q0vSpec {
            width: 1,
            height: 1,
            fps: 30,
            timeline_frames: 3,
            video: true,
            audio: false,
            audio_sample_rate: 0,
            audio_channels: 0,
        };
        assert_eq!(spec.video_frame_for_host_frame(0, 60), Some(0));
        assert_eq!(spec.video_frame_for_host_frame(1, 60), Some(0));
        assert_eq!(spec.video_frame_for_host_frame(2, 60), Some(1));
        assert_eq!(spec.video_frame_for_host_frame(5, 60), Some(2));
        assert_eq!(spec.video_frame_for_host_frame(600, 60), Some(2));
    }

    #[test]
    fn header_probe_reads_stream_metadata_without_owning_payload() {
        let spec = Q0vSpec {
            width: 0,
            height: 0,
            fps: 48_000,
            timeline_frames: 4,
            video: false,
            audio: true,
            audio_sample_rate: 48_000,
            audio_channels: 2,
        };
        let mut writer = Q0vWriter::new(std::io::Cursor::new(Vec::new()), spec).unwrap();
        writer
            .write_audio_pcm_i16(&[1, -1, 2, -2, 3, -3, 4, -4])
            .unwrap();
        let bytes = writer.finish().unwrap().into_inner();
        let header = probe_header(&bytes).unwrap();
        assert_eq!(header.spec, spec);
        assert_eq!(header.audio_samples_per_channel, 4);
    }

    #[test]
    fn audio_duration_uses_the_same_rational_timeline_clock() {
        let spec = Q0vSpec {
            width: 0,
            height: 0,
            fps: 30,
            timeline_frames: 61,
            video: false,
            audio: true,
            audio_sample_rate: 48_000,
            audio_channels: 2,
        };
        assert_eq!(spec.expected_audio_samples_per_channel(), 97_600);
        assert_eq!(spec.duration_ticks(), 20_333_333);
    }
}
