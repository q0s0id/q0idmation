//! Windows Media Foundation H.264/MP4 encoder.

use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::ptr;

use windows::core::PCWSTR;
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};

use crate::q0v::MEDIA_TICKS_PER_SECOND;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mp4Spec {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub bitrate: u32,
}

impl Mp4Spec {
    pub fn validate(self) -> Result<(), String> {
        if self.width == 0 || self.height == 0 || self.fps == 0 {
            return Err("mp4 dimensions and fps must be positive".to_string());
        }
        if !self.width.is_multiple_of(2) || !self.height.is_multiple_of(2) {
            return Err("h.264 output dimensions must be even".to_string());
        }
        if self.bitrate < 64_000 {
            return Err("mp4 bitrate is too low".to_string());
        }
        Ok(())
    }
}

struct MediaFoundationSession {
    com_initialized: bool,
    mf_started: bool,
}

impl MediaFoundationSession {
    fn start() -> Result<Self, String> {
        unsafe {
            CoInitializeEx(None, COINIT_MULTITHREADED)
                .ok()
                .map_err(|error| format!("initialize com: {error}"))?;
            let mut session = Self {
                com_initialized: true,
                mf_started: false,
            };
            MFStartup(MF_VERSION, MFSTARTUP_FULL)
                .map_err(|error| format!("start media foundation: {error}"))?;
            session.mf_started = true;
            Ok(session)
        }
    }
}

impl Drop for MediaFoundationSession {
    fn drop(&mut self) {
        unsafe {
            if self.mf_started {
                let _ = MFShutdown();
            }
            if self.com_initialized {
                CoUninitialize();
            }
        }
    }
}

pub struct Mp4Encoder {
    writer: IMFSinkWriter,
    stream_index: u32,
    spec: Mp4Spec,
    frame_index: u64,
    finalized: bool,
    _session: MediaFoundationSession,
}

impl Mp4Encoder {
    pub fn create(path: &Path, spec: Mp4Spec) -> Result<Self, String> {
        spec.validate()?;
        let session = MediaFoundationSession::start()?;
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let writer = unsafe {
            MFCreateSinkWriterFromURL(
                PCWSTR(wide.as_ptr()),
                None::<&IMFByteStream>,
                None::<&IMFAttributes>,
            )
            .map_err(|error| format!("create mp4 sink writer: {error}"))?
        };

        let output_type = unsafe { MFCreateMediaType() }
            .map_err(|error| format!("create h.264 output type: {error}"))?;
        unsafe {
            output_type
                .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
                .map_err(|error| format!("set mp4 major type: {error}"))?;
            output_type
                .SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)
                .map_err(|error| format!("set mp4 h.264 subtype: {error}"))?;
            output_type
                .SetUINT32(&MF_MT_AVG_BITRATE, spec.bitrate)
                .map_err(|error| format!("set mp4 bitrate: {error}"))?;
            output_type
                .SetUINT32(&MF_MT_MPEG2_PROFILE, eAVEncH264VProfile_Main.0 as u32)
                .map_err(|error| format!("set mp4 h.264 profile: {error}"))?;
            output_type
                .SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)
                .map_err(|error| format!("set mp4 progressive mode: {error}"))?;
            output_type
                .SetUINT64(&MF_MT_FRAME_SIZE, pack_ratio(spec.width, spec.height))
                .map_err(|error| format!("set mp4 frame size: {error}"))?;
            output_type
                .SetUINT64(&MF_MT_FRAME_RATE, pack_ratio(spec.fps, 1))
                .map_err(|error| format!("set mp4 frame rate: {error}"))?;
            output_type
                .SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack_ratio(1, 1))
                .map_err(|error| format!("set mp4 pixel aspect ratio: {error}"))?;
        }
        let stream_index = unsafe { writer.AddStream(&output_type) }
            .map_err(|error| format!("add h.264 stream: {error}"))?;

        let input_type = unsafe { MFCreateMediaType() }
            .map_err(|error| format!("create bgra input type: {error}"))?;
        unsafe {
            input_type
                .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
                .map_err(|error| format!("set input major type: {error}"))?;
            input_type
                .SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)
                .map_err(|error| format!("set input nv12 subtype: {error}"))?;
            input_type
                .SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)
                .map_err(|error| format!("set input progressive mode: {error}"))?;
            input_type
                .SetUINT64(&MF_MT_FRAME_SIZE, pack_ratio(spec.width, spec.height))
                .map_err(|error| format!("set input frame size: {error}"))?;
            input_type
                .SetUINT64(&MF_MT_FRAME_RATE, pack_ratio(spec.fps, 1))
                .map_err(|error| format!("set input frame rate: {error}"))?;
            input_type
                .SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack_ratio(1, 1))
                .map_err(|error| format!("set input pixel aspect ratio: {error}"))?;
            let sample_size = spec.width.saturating_mul(spec.height).saturating_mul(3) / 2;
            input_type
                .SetUINT32(&MF_MT_DEFAULT_STRIDE, spec.width)
                .map_err(|error| format!("set input nv12 stride: {error}"))?;
            input_type
                .SetUINT32(&MF_MT_FIXED_SIZE_SAMPLES, 1)
                .map_err(|error| format!("set fixed nv12 samples: {error}"))?;
            input_type
                .SetUINT32(&MF_MT_ALL_SAMPLES_INDEPENDENT, 1)
                .map_err(|error| format!("set independent nv12 samples: {error}"))?;
            input_type
                .SetUINT32(&MF_MT_SAMPLE_SIZE, sample_size)
                .map_err(|error| format!("set nv12 sample size: {error}"))?;
            writer
                .SetInputMediaType(stream_index, &input_type, None::<&IMFAttributes>)
                .map_err(|error| format!("connect nv12 to h.264 encoder: {error}"))?;
            writer
                .BeginWriting()
                .map_err(|error| format!("begin mp4 writing: {error}"))?;
        }

        Ok(Self {
            writer,
            stream_index,
            spec,
            frame_index: 0,
            finalized: false,
            _session: session,
        })
    }

    pub fn write_rgba_frame(&mut self, rgba: &[u8]) -> Result<(), String> {
        let expected = u64::from(self.spec.width)
            .checked_mul(u64::from(self.spec.height))
            .and_then(|pixels| pixels.checked_mul(4))
            .ok_or_else(|| "mp4 frame size overflows".to_string())?;
        if rgba.len() as u64 != expected {
            return Err("rgba frame length does not match mp4 dimensions".to_string());
        }
        let nv12 = rgba_to_nv12(rgba, self.spec.width, self.spec.height)?;
        let length = u32::try_from(nv12.len()).map_err(|_| "mp4 frame is too large".to_string())?;
        let buffer = unsafe { MFCreateMemoryBuffer(length) }
            .map_err(|error| format!("allocate mp4 sample buffer: {error}"))?;
        let mut destination = ptr::null_mut();
        unsafe {
            buffer
                .Lock(&mut destination, None, None)
                .map_err(|error| format!("lock mp4 sample buffer: {error}"))?;
            ptr::copy_nonoverlapping(nv12.as_ptr(), destination, nv12.len());
            buffer
                .Unlock()
                .map_err(|error| format!("unlock mp4 sample buffer: {error}"))?;
            buffer
                .SetCurrentLength(length)
                .map_err(|error| format!("set mp4 sample length: {error}"))?;
        }
        let sample =
            unsafe { MFCreateSample() }.map_err(|error| format!("create mp4 sample: {error}"))?;
        let start =
            self.frame_index.saturating_mul(MEDIA_TICKS_PER_SECOND) / u64::from(self.spec.fps);
        let end = self
            .frame_index
            .saturating_add(1)
            .saturating_mul(MEDIA_TICKS_PER_SECOND)
            / u64::from(self.spec.fps);
        unsafe {
            sample
                .AddBuffer(&buffer)
                .map_err(|error| format!("attach mp4 sample buffer: {error}"))?;
            sample
                .SetSampleTime(start as i64)
                .map_err(|error| format!("set mp4 sample time: {error}"))?;
            sample
                .SetSampleDuration(end.saturating_sub(start) as i64)
                .map_err(|error| format!("set mp4 sample duration: {error}"))?;
            self.writer
                .WriteSample(self.stream_index, &sample)
                .map_err(|error| format!("write h.264 sample: {error}"))?;
        }
        self.frame_index = self.frame_index.saturating_add(1);
        Ok(())
    }

    pub fn finish(mut self) -> Result<(), String> {
        unsafe {
            self.writer
                .Finalize()
                .map_err(|error| format!("finalize mp4: {error}"))?;
        }
        self.finalized = true;
        Ok(())
    }
}

impl Drop for Mp4Encoder {
    fn drop(&mut self) {
        if !self.finalized {
            // Sink Writer cleanup is handled by COM object release. We avoid
            // finalizing an explicitly cancelled or failed file.
        }
    }
}

fn rgba_to_nv12(rgba: &[u8], width: u32, height: u32) -> Result<Vec<u8>, String> {
    if !width.is_multiple_of(2) || !height.is_multiple_of(2) {
        return Err("nv12 conversion requires even dimensions".to_string());
    }
    let pixels = width as usize * height as usize;
    if rgba.len() != pixels * 4 {
        return Err("rgba buffer length does not match nv12 dimensions".to_string());
    }
    let mut output = vec![0_u8; pixels + pixels / 2];
    let width_usize = width as usize;
    for y in 0..height as usize {
        for x in 0..width_usize {
            let source = (y * width_usize + x) * 4;
            let r = i32::from(rgba[source]);
            let g = i32::from(rgba[source + 1]);
            let b = i32::from(rgba[source + 2]);
            output[y * width_usize + x] = clamp_u8(((66 * r + 129 * g + 25 * b + 128) >> 8) + 16);
        }
    }
    let uv_start = pixels;
    for y in (0..height as usize).step_by(2) {
        for x in (0..width_usize).step_by(2) {
            let mut sum_u = 0_i32;
            let mut sum_v = 0_i32;
            for dy in 0..2 {
                for dx in 0..2 {
                    let source = ((y + dy) * width_usize + x + dx) * 4;
                    let r = i32::from(rgba[source]);
                    let g = i32::from(rgba[source + 1]);
                    let b = i32::from(rgba[source + 2]);
                    sum_u += ((-38 * r - 74 * g + 112 * b + 128) >> 8) + 128;
                    sum_v += ((112 * r - 94 * g - 18 * b + 128) >> 8) + 128;
                }
            }
            let uv = uv_start + (y / 2) * width_usize + x;
            output[uv] = clamp_u8((sum_u + 2) / 4);
            output[uv + 1] = clamp_u8((sum_v + 2) / 4);
        }
    }
    Ok(output)
}

fn clamp_u8(value: i32) -> u8 {
    value.clamp(0, 255) as u8
}

const fn pack_ratio(numerator: u32, denominator: u32) -> u64 {
    ((numerator as u64) << 32) | denominator as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_odd_h264_dimensions() {
        assert!(Mp4Spec {
            width: 3,
            height: 2,
            fps: 24,
            bitrate: 1_000_000,
        }
        .validate()
        .is_err());
    }

    #[test]
    fn media_foundation_writes_a_real_mp4() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("q0video-mf-{}-{nonce}.mp4", std::process::id()));
        let spec = Mp4Spec {
            width: 64,
            height: 64,
            fps: 24,
            bitrate: 500_000,
        };
        let mut encoder = Mp4Encoder::create(&path, spec).expect("create media foundation mp4");
        encoder
            .write_rgba_frame(&[255, 0, 0, 255].repeat(64 * 64))
            .expect("red frame");
        encoder
            .write_rgba_frame(&[0, 255, 0, 255].repeat(64 * 64))
            .expect("green frame");
        encoder.finish().expect("finalize media foundation mp4");
        let bytes = std::fs::read(&path).expect("read mp4");
        assert!(bytes.len() > 32);
        assert!(bytes[..bytes.len().min(64)]
            .windows(4)
            .any(|window| window == b"ftyp"));
        std::fs::remove_file(path).expect("cleanup mp4");
    }
}
