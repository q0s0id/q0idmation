use std::path::{Path, PathBuf};

#[cfg(windows)]
use std::io::Cursor;
#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;
#[cfg(windows)]
use std::ptr;

#[cfg(windows)]
use image::ImageEncoder;
#[cfg(windows)]
use windows::core::PCWSTR;
#[cfg(windows)]
use windows::Win32::Media::MediaFoundation::*;
#[cfg(windows)]
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};

#[cfg(windows)]
use crate::q0v::{Q0vSpec, Q0vWriter};

/// Decode an ordinary MP4 into the portable q0v container used by q0editor
/// and q0player. The first implementation imports the video stream; audio is
/// deliberately not fabricated or silently replaced with silence.
#[cfg(windows)]
pub fn transcode_mp4_to_q0v(path: &Path) -> Result<Vec<u8>, String> {
    let session = MediaFoundationSession::start()?;
    let reader = create_video_reader(path)?;
    let stream = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;

    let output_type = unsafe { MFCreateMediaType() }
        .map_err(|error| format!("create mp4 decode media type: {error}"))?;
    unsafe {
        output_type
            .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
            .map_err(|error| format!("set mp4 decode major type: {error}"))?;
        output_type
            .SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)
            .map_err(|error| format!("set mp4 decode rgb32 subtype: {error}"))?;
        reader
            .SetCurrentMediaType(stream, None, &output_type)
            .map_err(|error| format!("connect mp4 decoder to rgb32: {error}"))?;
    }

    let current_type = unsafe { reader.GetCurrentMediaType(stream) }
        .map_err(|error| format!("query decoded mp4 format: {error}"))?;
    let (width, height) = unpack_ratio(
        unsafe { current_type.GetUINT64(&MF_MT_FRAME_SIZE) }
            .map_err(|error| format!("query decoded mp4 dimensions: {error}"))?,
    );
    if width == 0 || height == 0 {
        return Err("decoded mp4 dimensions are zero".to_string());
    }
    let (fps_numerator, fps_denominator) = unsafe { current_type.GetUINT64(&MF_MT_FRAME_RATE) }
        .map(unpack_ratio)
        .unwrap_or((30, 1));
    let fps = rounded_fps(fps_numerator, fps_denominator)?;
    let stride = unsafe { current_type.GetUINT32(&MF_MT_DEFAULT_STRIDE) }
        .map(|value| value as i32)
        .unwrap_or_else(|_| width.saturating_mul(4) as i32);

    const MAX_Q0V_PAYLOAD_BYTES: usize = 240 * 1024 * 1024;
    let mut decoded_frames = Vec::<DecodedFrame>::new();
    let mut total_payload_bytes = 0_usize;
    loop {
        let mut flags = 0_u32;
        let mut timestamp = 0_i64;
        let mut sample = None;
        unsafe {
            reader
                .ReadSample(
                    stream,
                    0,
                    None,
                    Some(&mut flags),
                    Some(&mut timestamp),
                    Some(&mut sample),
                )
                .map_err(|error| format!("decode mp4 sample: {error}"))?;
        }
        if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
            break;
        }
        let Some(sample) = sample else {
            continue;
        };
        let rgba = sample_to_rgba(&sample, width, height, stride)?;
        let png = encode_png(&rgba, width, height)?;
        total_payload_bytes = total_payload_bytes
            .checked_add(png.len())
            .ok_or_else(|| "converted q0v size overflows".to_string())?;
        if total_payload_bytes > MAX_Q0V_PAYLOAD_BYTES {
            return Err("converted q0v would exceed the 240 MiB project media limit".to_string());
        }
        decoded_frames.push(DecodedFrame { timestamp, png });
    }
    drop(reader);
    drop(session);

    if decoded_frames.is_empty() {
        return Err("mp4 contains no decodable video frames".to_string());
    }
    let timeline_frames = u32::try_from(decoded_frames.len())
        .map_err(|_| "mp4 contains too many frames for q0v".to_string())?;
    let spec = Q0vSpec {
        width,
        height,
        fps,
        timeline_frames,
        video: true,
        audio: false,
        audio_sample_rate: 0,
        audio_channels: 0,
    };
    let first_timestamp = decoded_frames
        .first()
        .map(|frame| frame.timestamp.max(0) as u64)
        .unwrap_or(0);
    let mut writer = Q0vWriter::new(Cursor::new(Vec::new()), spec)?;
    for frame in decoded_frames {
        let timestamp = frame.timestamp.max(0) as u64;
        writer.write_video_frame(timestamp.saturating_sub(first_timestamp), &frame.png)?;
    }
    Ok(writer.finish()?.into_inner())
}

#[cfg(not(windows))]
pub fn transcode_mp4_to_q0v(_path: &Path) -> Result<Vec<u8>, String> {
    Err("mp4 to q0v conversion is currently available on Windows only".to_string())
}
/// Start MP4 decoding on a fresh worker thread. Media Foundation requires a
/// predictable COM apartment; GUI threads may already be initialized as STA,
/// so importing directly on the caller thread can fail with
/// `RPC_E_CHANGED_MODE`.
pub fn spawn_mp4_to_q0v(
    path: PathBuf,
) -> Result<std::sync::mpsc::Receiver<Result<Vec<u8>, String>>, String> {
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("q0video-mp4-import".to_string())
        .spawn(move || {
            let result = transcode_mp4_to_q0v(&path);
            let _ = sender.send(result);
        })
        .map_err(|error| format!("start mp4 import worker: {error}"))?;
    Ok(receiver)
}

#[cfg(windows)]
struct DecodedFrame {
    timestamp: i64,
    png: Vec<u8>,
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
                .map_err(|error| format!("initialize com for mp4 import: {error}"))?;
            let mut session = Self {
                com_initialized: true,
                mf_started: false,
            };
            MFStartup(MF_VERSION, MFSTARTUP_FULL)
                .map_err(|error| format!("start media foundation for mp4 import: {error}"))?;
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
fn create_video_reader(path: &Path) -> Result<IMFSourceReader, String> {
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut attributes = None;
    unsafe { MFCreateAttributes(&mut attributes, 2) }
        .map_err(|error| format!("create mp4 reader attributes: {error}"))?;
    let attributes =
        attributes.ok_or_else(|| "media foundation returned no reader attributes".to_string())?;
    unsafe {
        attributes
            .SetUINT32(&MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, 1)
            .map_err(|error| format!("enable mp4 video processing: {error}"))?;
        let reader = MFCreateSourceReaderFromURL(PCWSTR(wide.as_ptr()), &attributes)
            .map_err(|error| format!("open mp4 for decoding: {error}"))?;
        reader
            .SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false)
            .map_err(|error| format!("disable unused mp4 streams: {error}"))?;
        reader
            .SetStreamSelection(MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32, true)
            .map_err(|error| format!("select mp4 video stream: {error}"))?;
        Ok(reader)
    }
}

#[cfg(windows)]
fn sample_to_rgba(
    sample: &IMFSample,
    width: u32,
    height: u32,
    stride: i32,
) -> Result<Vec<u8>, String> {
    let buffer = unsafe { sample.ConvertToContiguousBuffer() }
        .map_err(|error| format!("flatten decoded mp4 sample: {error}"))?;
    let mut pointer = ptr::null_mut();
    let mut current_length = 0_u32;
    unsafe {
        buffer
            .Lock(&mut pointer, None, Some(&mut current_length))
            .map_err(|error| format!("lock decoded mp4 sample: {error}"))?;
    }
    let result = (|| {
        let row_bytes = usize::try_from(width)
            .ok()
            .and_then(|width| width.checked_mul(4))
            .ok_or_else(|| "decoded mp4 row size overflows".to_string())?;
        let source_stride = stride.unsigned_abs() as usize;
        if source_stride < row_bytes {
            return Err("decoded mp4 stride is smaller than one rgb32 row".to_string());
        }
        let required = source_stride
            .checked_mul(height as usize)
            .ok_or_else(|| "decoded mp4 sample size overflows".to_string())?;
        if (current_length as usize) < required || pointer.is_null() {
            return Err("decoded mp4 sample is truncated".to_string());
        }
        let source = unsafe { std::slice::from_raw_parts(pointer, current_length as usize) };
        let mut rgba = vec![0_u8; row_bytes * height as usize];
        for y in 0..height as usize {
            // Source Reader's RGB32 transform exposes scanlines in display
            // order. `MF_MT_DEFAULT_STRIDE` describes padding, not a request
            // for an extra DIB-style vertical flip here.
            let source_row = &source[y * source_stride..y * source_stride + row_bytes];
            let destination_row = &mut rgba[y * row_bytes..(y + 1) * row_bytes];
            for (source_pixel, destination_pixel) in source_row
                .chunks_exact(4)
                .zip(destination_row.chunks_exact_mut(4))
            {
                destination_pixel[0] = source_pixel[2];
                destination_pixel[1] = source_pixel[1];
                destination_pixel[2] = source_pixel[0];
                destination_pixel[3] = 255;
            }
        }
        Ok(rgba)
    })();
    unsafe {
        let _ = buffer.Unlock();
    }
    result
}

#[cfg(windows)]
fn encode_png(rgba: &[u8], width: u32, height: u32) -> Result<Vec<u8>, String> {
    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new(&mut png)
        .write_image(rgba, width, height, image::ColorType::Rgba8)
        .map_err(|error| format!("encode imported q0v frame: {error}"))?;
    Ok(png)
}

#[cfg(windows)]
fn unpack_ratio(value: u64) -> (u32, u32) {
    ((value >> 32) as u32, value as u32)
}

#[cfg(windows)]
fn rounded_fps(numerator: u32, denominator: u32) -> Result<u32, String> {
    if numerator == 0 || denominator == 0 {
        return Err("decoded mp4 frame rate is invalid".to_string());
    }
    let rounded = (u64::from(numerator) + u64::from(denominator) / 2) / u64::from(denominator);
    u32::try_from(rounded.clamp(1, 240))
        .map_err(|_| "decoded mp4 frame rate does not fit q0v".to_string())
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use crate::mp4::{Mp4Encoder, Mp4Spec};

    #[test]
    fn media_foundation_mp4_import_preserves_color_and_orientation() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "q0video импорт с пробелом {}-{nonce}.mp4",
            std::process::id()
        ));
        let mut encoder = Mp4Encoder::create(
            &path,
            Mp4Spec {
                width: 64,
                height: 64,
                fps: 24,
                bitrate: 500_000,
            },
        )
        .expect("create fixture mp4");
        let mut frame = vec![0_u8; 64 * 64 * 4];
        for y in 0..64 {
            for x in 0..64 {
                let index = (y * 64 + x) * 4;
                frame[index..index + 4].copy_from_slice(if y < 32 {
                    &[255, 0, 0, 255]
                } else {
                    &[0, 0, 255, 255]
                });
            }
        }
        encoder.write_rgba_frame(&frame).expect("fixture frame");
        encoder.finish().expect("finish fixture mp4");

        let bytes = transcode_mp4_to_q0v(&path).expect("transcode fixture mp4");
        let media = crate::q0v::Q0vFile::parse(bytes).expect("parse imported q0v");
        let decoded = media.decode_frame_rgba(0).expect("decode imported frame");
        let top = &decoded[0..4];
        let bottom = &decoded[((63 * 64) * 4)..((63 * 64) * 4 + 4)];
        assert!(top[0] > top[2], "top row should stay red: {top:?}");
        assert!(
            bottom[2] > bottom[0],
            "bottom row should stay blue: {bottom:?}"
        );
        std::fs::remove_file(path).expect("cleanup fixture mp4");
    }

    #[test]
    fn mp4_worker_uses_mta_even_when_the_caller_is_sta() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "q0video-sta-caller-{}-{nonce}.mp4",
            std::process::id()
        ));
        let mut encoder = Mp4Encoder::create(
            &path,
            Mp4Spec {
                width: 64,
                height: 64,
                fps: 24,
                bitrate: 500_000,
            },
        )
        .expect("create STA fixture mp4");
        encoder
            .write_rgba_frame(&[32, 64, 192, 255].repeat(64 * 64))
            .expect("write STA fixture frame");
        encoder.finish().expect("finish STA fixture mp4");

        let worker_path = path.clone();
        let bytes = std::thread::spawn(move || {
            unsafe {
                windows::Win32::System::Com::CoInitializeEx(
                    None,
                    windows::Win32::System::Com::COINIT_APARTMENTTHREADED,
                )
                .ok()
                .expect("initialize dedicated STA caller");
            }
            let receiver = spawn_mp4_to_q0v(worker_path).expect("spawn MTA import worker");
            let result = receiver
                .recv_timeout(std::time::Duration::from_secs(10))
                .expect("receive worker result");
            unsafe {
                windows::Win32::System::Com::CoUninitialize();
            }
            result
        })
        .join()
        .expect("join STA caller")
        .expect("transcode from STA caller");

        let media = crate::q0v::Q0vFile::parse(bytes).expect("parse worker q0v");
        assert_eq!(media.spec.timeline_frames, 1);
        std::fs::remove_file(path).expect("cleanup STA fixture mp4");
    }
}
