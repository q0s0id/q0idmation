//! H.264/MP4 output: the frame spec and NV12 conversion every platform's
//! encoder shares, and the Windows Media Foundation encoder.

#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;
#[cfg(windows)]
use std::path::Path;
#[cfg(windows)]
use std::ptr;

#[cfg(windows)]
use windows::core::PCWSTR;
#[cfg(windows)]
use windows::Win32::Media::MediaFoundation::*;
#[cfg(windows)]
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};

#[cfg(windows)]
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
        if u64::from(self.width) * u64::from(self.height) > u64::from(u32::MAX) * 2 / 3 {
            return Err("mp4 sample buffer exceeds the media foundation size limit".to_string());
        }
        if self.bitrate < 64_000 {
            return Err("mp4 bitrate is too low".to_string());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mp4AudioSpec {
    pub sample_rate: u32,
    pub channels: u16,
    pub bitrate: u32,
}

impl Default for Mp4AudioSpec {
    fn default() -> Self {
        Self {
            sample_rate: 48_000,
            channels: 2,
            bitrate: 160_000,
        }
    }
}

impl Mp4AudioSpec {
    pub fn validate(self) -> Result<(), String> {
        if !matches!(self.sample_rate, 44_100 | 48_000)
            || !matches!(self.channels, 1 | 2)
            || !matches!(self.bitrate, 96_000 | 128_000 | 160_000 | 192_000)
        {
            return Err("unsupported mp4 AAC audio format".into());
        }
        Ok(())
    }
}

#[cfg(windows)]
struct MediaFoundationSession {
    com_initialized: bool,
    mf_started: bool,
}

#[cfg(windows)]
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

#[cfg(windows)]
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

#[cfg(windows)]
pub struct Mp4Encoder {
    writer: IMFSinkWriter,
    stream_index: u32,
    spec: Mp4Spec,
    frame_index: u64,
    audio: Option<(u32, Mp4AudioSpec)>,
    audio_samples_written: u64,
    finalized: bool,
    _session: MediaFoundationSession,
}

#[cfg(windows)]
impl Mp4Encoder {
    pub fn create(path: &Path, spec: Mp4Spec) -> Result<Self, String> {
        Self::create_with_audio(path, spec, None)
    }

    pub fn create_with_audio(
        path: &Path,
        spec: Mp4Spec,
        audio: Option<Mp4AudioSpec>,
    ) -> Result<Self, String> {
        spec.validate()?;
        if let Some(audio) = audio {
            audio.validate()?;
        }
        // Allow installed hardware MFTs, but retain software support for
        // machines/drivers that cannot encode this particular output format.
        Self::create_with_hardware(path, spec, audio, true)
            .or_else(|_| Self::create_with_hardware(path, spec, audio, false))
    }

    fn create_with_hardware(
        path: &Path,
        spec: Mp4Spec,
        audio: Option<Mp4AudioSpec>,
        hardware: bool,
    ) -> Result<Self, String> {
        let session = MediaFoundationSession::start()?;
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let mut attributes = None;
        unsafe { MFCreateAttributes(&mut attributes, 1) }
            .map_err(|error| format!("create mp4 writer attributes: {error}"))?;
        let attributes = attributes.ok_or("missing mp4 writer attributes")?;
        unsafe {
            attributes.SetUINT32(
                &MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS,
                u32::from(hardware),
            )
        }
        .map_err(|error| format!("set mp4 hardware encoding: {error}"))?;
        let writer = unsafe {
            MFCreateSinkWriterFromURL(PCWSTR(wide.as_ptr()), None::<&IMFByteStream>, &attributes)
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
            .map_err(|error| format!("create nv12 input type: {error}"))?;
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
            // Some hardware encoders default to B frames and shift video PTS
            // by one frame while AAC starts at zero. Disable reordering for
            // movies with audio so both presentation clocks really start at zero.
            let video_parameters = if audio.is_some() {
                let mut parameters = None;
                MFCreateAttributes(&mut parameters, 1)
                    .map_err(|error| format!("create video encoder parameters: {error}"))?;
                let parameters = parameters.ok_or("missing video encoder parameters")?;
                parameters
                    .SetUINT32(&CODECAPI_AVEncMPVDefaultBPictureCount, 0)
                    .map_err(|error| format!("disable video timestamp reordering: {error}"))?;
                Some(parameters)
            } else {
                None
            };
            writer
                .SetInputMediaType(stream_index, &input_type, video_parameters.as_ref())
                .map_err(|error| format!("connect nv12 to h.264 encoder: {error}"))?;
        }
        let audio = audio
            .map(|spec| add_audio_stream(&writer, spec).map(|index| (index, spec)))
            .transpose()?;
        unsafe {
            writer
                .BeginWriting()
                .map_err(|error| format!("begin mp4 writing: {error}"))?;
        }

        Ok(Self {
            writer,
            stream_index,
            spec,
            frame_index: 0,
            audio,
            audio_samples_written: 0,
            finalized: false,
            _session: session,
        })
    }

    pub fn write_rgba_frame(&mut self, rgba: &[u8]) -> Result<(), String> {
        let nv12 = rgba_to_nv12(rgba, self.spec.width, self.spec.height)?;
        self.write_nv12_frame(&nv12)
    }

    /// Submit a prepared frame. Conversion may run on rendering workers;
    /// the COM writer stays on its owning thread and receives frames in order.
    pub fn write_nv12_frame(&mut self, nv12: &[u8]) -> Result<(), String> {
        if nv12.len() as u64 != u64::from(self.spec.width) * u64::from(self.spec.height) * 3 / 2 {
            return Err("nv12 frame length does not match mp4 dimensions".to_string());
        }
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

    /// Audio and video use integer sample clocks, with both streams starting at zero.
    pub fn write_audio_pcm_i16(&mut self, interleaved: &[i16]) -> Result<(), String> {
        let (stream, spec) = self.audio.ok_or("mp4 has no audio stream")?;
        if !interleaved.len().is_multiple_of(usize::from(spec.channels)) {
            return Err("mp4 audio is not aligned to whole channel frames".into());
        }
        if interleaved.is_empty() {
            return Ok(());
        }
        let bytes: Vec<u8> = interleaved
            .iter()
            .flat_map(|sample| sample.to_le_bytes())
            .collect();
        let length = u32::try_from(bytes.len()).map_err(|_| "mp4 audio sample is too large")?;
        let frames = interleaved.len() as u64 / u64::from(spec.channels);
        let start =
            self.audio_samples_written * MEDIA_TICKS_PER_SECOND / u64::from(spec.sample_rate);
        let end = (self.audio_samples_written + frames) * MEDIA_TICKS_PER_SECOND
            / u64::from(spec.sample_rate);
        unsafe {
            let buffer = MFCreateMemoryBuffer(length)
                .map_err(|error| format!("allocate audio buffer: {error}"))?;
            let mut destination = ptr::null_mut();
            buffer
                .Lock(&mut destination, None, None)
                .map_err(|error| format!("lock audio buffer: {error}"))?;
            ptr::copy_nonoverlapping(bytes.as_ptr(), destination, bytes.len());
            buffer
                .Unlock()
                .map_err(|error| format!("unlock audio buffer: {error}"))?;
            buffer
                .SetCurrentLength(length)
                .map_err(|error| format!("set audio buffer length: {error}"))?;
            let sample =
                MFCreateSample().map_err(|error| format!("create audio sample: {error}"))?;
            sample
                .AddBuffer(&buffer)
                .map_err(|error| format!("attach audio buffer: {error}"))?;
            sample
                .SetSampleTime(start as i64)
                .map_err(|error| format!("set audio time: {error}"))?;
            sample
                .SetSampleDuration((end - start) as i64)
                .map_err(|error| format!("set audio duration: {error}"))?;
            self.writer
                .WriteSample(stream, &sample)
                .map_err(|error| format!("write AAC sample: {error}"))?;
        }
        self.audio_samples_written += frames;
        Ok(())
    }
}

#[cfg(windows)]
fn add_audio_stream(writer: &IMFSinkWriter, spec: Mp4AudioSpec) -> Result<u32, String> {
    unsafe {
        let output = MFCreateMediaType().map_err(|error| format!("create AAC type: {error}"))?;
        output
            .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)
            .map_err(|error| error.to_string())?;
        output
            .SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_AAC)
            .map_err(|error| error.to_string())?;
        output
            .SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, spec.sample_rate)
            .map_err(|error| error.to_string())?;
        output
            .SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, u32::from(spec.channels))
            .map_err(|error| error.to_string())?;
        output
            .SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)
            .map_err(|error| error.to_string())?;
        output
            .SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, spec.bitrate / 8)
            .map_err(|error| error.to_string())?;
        output
            .SetUINT32(&MF_MT_AAC_PAYLOAD_TYPE, 0)
            .map_err(|error| error.to_string())?;
        let index = writer
            .AddStream(&output)
            .map_err(|error| format!("add AAC stream: {error}"))?;
        let input = MFCreateMediaType().map_err(|error| format!("create PCM type: {error}"))?;
        input
            .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)
            .map_err(|error| error.to_string())?;
        input
            .SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM)
            .map_err(|error| error.to_string())?;
        input
            .SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, spec.sample_rate)
            .map_err(|error| error.to_string())?;
        input
            .SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, u32::from(spec.channels))
            .map_err(|error| error.to_string())?;
        input
            .SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)
            .map_err(|error| error.to_string())?;
        let alignment = u32::from(spec.channels) * 2;
        input
            .SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, alignment)
            .map_err(|error| error.to_string())?;
        input
            .SetUINT32(
                &MF_MT_AUDIO_AVG_BYTES_PER_SECOND,
                spec.sample_rate * alignment,
            )
            .map_err(|error| error.to_string())?;
        writer
            .SetInputMediaType(index, &input, None::<&IMFAttributes>)
            .map_err(|error| format!("connect PCM to AAC: {error}"))?;
        Ok(index)
    }
}

#[cfg(windows)]
impl Drop for Mp4Encoder {
    fn drop(&mut self) {
        if !self.finalized {
            // Sink Writer cleanup is handled by COM object release. We avoid
            // finalizing an explicitly cancelled or failed file.
        }
    }
}

pub fn rgba_to_nv12(rgba: &[u8], width: u32, height: u32) -> Result<Vec<u8>, String> {
    if width == 0 || height == 0 || !width.is_multiple_of(2) || !height.is_multiple_of(2) {
        return Err("nv12 conversion requires even dimensions".to_string());
    }
    let pixels = (width as usize)
        .checked_mul(height as usize)
        .ok_or("nv12 dimensions overflow")?;
    if pixels.checked_mul(4) != Some(rgba.len()) {
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

#[cfg(windows)]
const fn pack_ratio(numerator: u32, denominator: u32) -> u64 {
    ((numerator as u64) << 32) | denominator as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nv12_conversion_preserves_luma_and_averages_each_chroma_block() {
        let rgba = [
            255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255,
        ];
        assert_eq!(
            rgba_to_nv12(&rgba, 2, 2).unwrap(),
            vec![82, 144, 41, 235, 128, 128]
        );
        assert!(rgba_to_nv12(&rgba[..12], 2, 2).is_err());
        assert!(rgba_to_nv12(&[], 0, 0).is_err());
        assert!(rgba_to_nv12(&rgba, 1, 4).is_err());
    }

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

    #[cfg(windows)]
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

    #[cfg(windows)]
    #[test]
    fn media_foundation_muxes_aac_alongside_video() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("q0video-aac-{}-{nonce}.mp4", std::process::id()));
        let mut encoder = Mp4Encoder::create_with_audio(
            &path,
            Mp4Spec {
                width: 64,
                height: 64,
                fps: 24,
                bitrate: 500_000,
            },
            Some(Mp4AudioSpec::default()),
        )
        .unwrap();
        for frame in 0..24 {
            encoder
                .write_rgba_frame(&[255, 0, 0, 255].repeat(64 * 64))
                .unwrap();
            let pcm: Vec<i16> = (frame * 2000..(frame + 1) * 2000)
                .flat_map(|sample| {
                    let value = ((sample as f32 * 440.0 * std::f32::consts::TAU / 48_000.0).sin()
                        * 12000.0) as i16;
                    [value, -value]
                })
                .collect();
            encoder.write_audio_pcm_i16(&pcm).unwrap();
        }
        encoder.finish().unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert!(bytes.windows(4).any(|data| data == b"soun"));
        assert!(bytes.windows(4).any(|data| data == b"mp4a"));
        assert!(bytes.windows(4).any(|data| data == b"avc1"));
        {
            let _session = MediaFoundationSession::start().unwrap();
            let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
            unsafe {
                let reader =
                    MFCreateSourceReaderFromURL(PCWSTR(wide.as_ptr()), None::<&IMFAttributes>)
                        .unwrap();
                let mut timestamp = -1;
                let mut sample = None;
                let mut video_flags = 0;
                reader
                    .ReadSample(
                        MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32,
                        0,
                        None,
                        Some(&mut video_flags),
                        Some(&mut timestamp),
                        Some(&mut sample),
                    )
                    .unwrap();
                assert!(sample.is_some());
                assert_eq!(
                    timestamp, 0,
                    "video and audio must start on the same presentation clock"
                );
                let audio_stream = MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32;
                let pcm_type = MFCreateMediaType().unwrap();
                pcm_type
                    .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)
                    .unwrap();
                pcm_type
                    .SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM)
                    .unwrap();
                pcm_type
                    .SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)
                    .unwrap();
                reader
                    .SetCurrentMediaType(audio_stream, None, &pcm_type)
                    .unwrap();
                let mut decoded = Vec::<i16>::new();
                loop {
                    let mut flags = 0;
                    let mut sample = None;
                    reader
                        .ReadSample(
                            audio_stream,
                            0,
                            None,
                            Some(&mut flags),
                            Some(&mut timestamp),
                            Some(&mut sample),
                        )
                        .unwrap();
                    if let Some(sample) = sample {
                        if decoded.is_empty() {
                            assert_eq!(timestamp, 0);
                        }
                        let buffer = sample.ConvertToContiguousBuffer().unwrap();
                        let mut pointer = ptr::null_mut();
                        let mut length = 0;
                        buffer.Lock(&mut pointer, None, Some(&mut length)).unwrap();
                        let data = std::slice::from_raw_parts(pointer, length as usize);
                        decoded.extend(
                            data.as_chunks::<2>()
                                .0
                                .iter()
                                .map(|raw| i16::from_le_bytes([raw[0], raw[1]])),
                        );
                        buffer.Unlock().unwrap();
                    }
                    if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                        break;
                    }
                }
                assert!(decoded.len() >= 48_000 * 2);
                assert!(
                    decoded.iter().any(|sample| sample.unsigned_abs() > 8_000),
                    "AAC must decode to audible audio"
                );
                let error: f64 = decoded
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|pair| (f64::from(pair[0]) + f64::from(pair[1])).abs())
                    .sum::<f64>()
                    / decoded.len() as f64;
                assert!(
                    error < 100.0,
                    "stereo polarity must survive encoding: {error}"
                );
            }
        }
        std::fs::remove_file(path).unwrap();
    }
}
