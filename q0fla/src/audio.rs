use std::collections::HashMap;
use std::io::Cursor;

use q0s_format::v2::{Asset, AudioClip, LibraryItemReference, ProjectV2, Q0vAsset};
use q0video::q0v::{Q0vSpec, Q0vWriter};
use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;

use crate::{attr, ImportError};

pub(crate) fn is_media_name(name: &str) -> bool {
    name.rsplit('.').next().is_some_and(|ext| {
        ["ogg", "mp3", "flac", "wav", "aif", "aiff"]
            .iter()
            .any(|supported| ext.eq_ignore_ascii_case(supported))
    })
}

fn reference(name: &str) -> String {
    name.replace('\\', "/").to_lowercase()
}

#[derive(Debug, Clone)]
pub(crate) struct RawSound {
    name: String,
    sync: String,
    in_point: u64,
    out_point: Option<u64>,
    loops: u32,
    envelope: Vec<(u64, f32, f32)>,
}

fn number(
    reader: &Reader<&[u8]>,
    element: &BytesStart<'_>,
    key: &[u8],
    default: u64,
) -> Result<u64, ImportError> {
    attr(reader, element, key)?.map_or(Ok(default), |value| {
        value.parse().map_err(|_| {
            ImportError::Invalid(format!(
                "invalid sound {} '{value}'",
                String::from_utf8_lossy(key)
            ))
        })
    })
}

impl RawSound {
    pub(crate) fn read(
        reader: &Reader<&[u8]>,
        element: &BytesStart<'_>,
    ) -> Result<Option<Self>, ImportError> {
        let Some(name) = attr(reader, element, b"soundName")?.filter(|name| !name.is_empty())
        else {
            return Ok(None);
        };
        Ok(Some(Self {
            name,
            sync: attr(reader, element, b"soundSync")?
                .unwrap_or_else(|| "event".into())
                .to_ascii_lowercase(),
            in_point: number(reader, element, b"inPoint44", 0)?,
            out_point: attr(reader, element, b"outPoint44")?
                .map(|value| {
                    value
                        .parse()
                        .map_err(|_| ImportError::Invalid(format!("invalid outPoint44 '{value}'")))
                })
                .transpose()?,
            loops: u32::try_from(number(reader, element, b"soundLoop", 1)?)
                .unwrap_or(u32::MAX)
                .max(1),
            envelope: Vec::new(),
        }))
    }

    pub(crate) fn envelope_point(
        &mut self,
        reader: &Reader<&[u8]>,
        element: &BytesStart<'_>,
    ) -> Result<(), ImportError> {
        self.envelope.push((
            number(reader, element, b"mark44", 0)?,
            number(reader, element, b"level0", 32768)?.min(32768) as f32 / 32768.0,
            number(reader, element, b"level1", 32768)?.min(32768) as f32 / 32768.0,
        ));
        Ok(())
    }
}

pub(crate) struct ImportedSound {
    asset_id: u16,
    samples: u64,
    sample_rate: u32,
}

pub(crate) type SoundMap = HashMap<String, ImportedSound>;

fn wrap_audio(
    source: &[u8],
    encoded: bool,
    rate: u32,
    channels: u16,
    samples: u64,
    pcm: &[u8],
) -> Result<Vec<u8>, String> {
    let spec = Q0vSpec {
        width: 0,
        height: 0,
        fps: rate,
        timeline_frames: u32::try_from(samples)
            .map_err(|_| "sound exceeds the q0v timeline limit")?,
        video: false,
        audio: true,
        audio_sample_rate: rate,
        audio_channels: channels,
    };
    let mut writer = Q0vWriter::new_compressed(Cursor::new(Vec::new()), spec)?;
    if encoded {
        writer.write_audio_encoded(source, samples)?;
    } else {
        for chunk in pcm.chunks(16_384 * usize::from(channels)) {
            let samples: Vec<i16> = chunk
                .as_chunks::<2>()
                .0
                .iter()
                .map(|pair| i16::from_le_bytes([pair[0], pair[1]]))
                .collect();
            writer.write_audio_pcm_i16(&samples)?;
        }
    }
    let bytes = writer.finish()?.into_inner();
    q0video::q0v::validate_bytes(&bytes)?;
    Ok(bytes)
}

fn decode_media(
    reader: &Reader<&[u8]>,
    element: &BytesStart<'_>,
    binaries: &HashMap<String, &[u8]>,
) -> Result<(Vec<u8>, u64, u32), String> {
    let mut source_error = None;
    let href = attr(reader, element, b"href")
        .map_err(|error| error.to_string())?
        .unwrap_or_default();
    let path = if reference(&href).starts_with("library/") {
        reference(&href)
    } else {
        format!("library/{}", reference(&href))
    };
    if let Some(source) = binaries.get(&path) {
        match q0video::audio::decode_source(source) {
            Ok(decoded) => {
                let samples = decoded.samples_per_channel();
                let encoded = source.starts_with(b"OggS")
                    || source.starts_with(b"fLaC")
                    || path.ends_with(".mp3");
                let bytes = wrap_audio(
                    source,
                    encoded,
                    decoded.sample_rate,
                    decoded.channels,
                    samples,
                    &decoded.pcm_le_bytes,
                )?;
                return Ok((bytes, samples, decoded.sample_rate));
            }
            Err(error) => source_error = Some(error),
        }
    }
    let data_href = attr(reader, element, b"soundDataHRef")
        .map_err(|error| error.to_string())?
        .unwrap_or_default();
    let pcm = binaries
        .get(&format!("bin/{}", reference(&data_href)))
        .ok_or_else(|| {
            source_error
                .unwrap_or_else(|| format!("embedded audio '{href}' and '{data_href}' are missing"))
        })?;
    let format = attr(reader, element, b"format")
        .map_err(|error| error.to_string())?
        .unwrap_or_default()
        .to_ascii_lowercase();
    if !format.contains("16bit") {
        return Err(format!("unsupported embedded PCM format '{format}'"));
    }
    let rate = match format.split_whitespace().next() {
        Some("44khz") => 44_100,
        Some("22khz") => 22_050,
        Some("11khz") => 11_025,
        Some("48khz") => 48_000,
        _ => return Err(format!("unsupported embedded PCM sample rate '{format}'")),
    };
    let channels = if format.contains("stereo") {
        2
    } else if format.contains("mono") {
        1
    } else {
        return Err(format!("missing embedded PCM channel layout '{format}'"));
    };
    let samples = number(reader, element, b"sampleCount", 0).map_err(|error| error.to_string())?;
    if samples == 0
        || samples.checked_mul(u64::from(channels) * 2) != Some(pcm.len() as u64)
        || pcm.len() > q0video::audio::MAX_DECODED_AUDIO_BYTES
    {
        return Err("embedded PCM length does not match its sound metadata".into());
    }
    Ok((
        wrap_audio(&[], false, rate, channels, samples, pcm)?,
        samples,
        rate,
    ))
}

pub(crate) fn import_media(
    xml: &[u8],
    binaries: &HashMap<String, &[u8]>,
    project: &mut ProjectV2,
    next_asset: &mut u16,
    warnings: &mut Vec<String>,
) -> Result<SoundMap, ImportError> {
    let mut sounds = SoundMap::new();
    let mut reader = Reader::from_reader(xml);
    loop {
        match reader
            .read_event()
            .map_err(|error| ImportError::Xml(error.to_string()))?
        {
            Event::Start(element) | Event::Empty(element)
                if element.name().as_ref() == b"DOMSoundItem" =>
            {
                let name = attr(&reader, &element, b"name")?
                    .unwrap_or_default()
                    .replace('\\', "/");
                match decode_media(&reader, &element, binaries) {
                    Ok((bytes, samples, sample_rate)) => {
                        let asset_id = *next_asset;
                        *next_asset = next_asset.checked_add(1).ok_or_else(|| {
                            ImportError::Invalid("too many sound assets in fla".into())
                        })?;
                        project.assets.push(Asset::Q0v(Q0vAsset {
                            asset_id,
                            bytes: bytes.into(),
                        }));
                        project.asset_names.insert(
                            asset_id,
                            name.rsplit('/').next().unwrap_or(&name).to_string(),
                        );
                        if let Some((folder, _)) = name.rsplit_once('/') {
                            if let Some(folder) =
                                crate::xfl_content::ensure_library_folder_paths(project, folder)
                            {
                                project
                                    .runtime
                                    .library_item_folders
                                    .insert(LibraryItemReference::Asset(asset_id), folder);
                            }
                        }
                        sounds.insert(
                            reference(&name),
                            ImportedSound {
                                asset_id,
                                samples,
                                sample_rate,
                            },
                        );
                    }
                    Err(error) => {
                        warnings.push(format!("sound '{name}' could not be imported: {error}"))
                    }
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(sounds)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn append_clip(
    raw: &RawSound,
    sounds: &SoundMap,
    project: &mut ProjectV2,
    q0rg_id: u16,
    layer_id: u16,
    start_frame: u16,
    duration: u16,
    frame_count: u16,
    warnings: &mut Vec<String>,
) {
    let Some(sound) = sounds.get(&reference(&raw.name)) else {
        warnings.push(format!(
            "timeline sound '{}' is missing; its clip was skipped",
            raw.name
        ));
        return;
    };
    if raw.sync == "stop" {
        for clip in &mut project.audio_clips {
            if clip.q0rg_id == q0rg_id
                && clip.layer_id == layer_id
                && clip.asset_id == sound.asset_id
                && clip.start_frame < start_frame
            {
                clip.duration_frames = Some(
                    clip.duration_frames
                        .unwrap_or(u16::MAX)
                        .min(start_frame - clip.start_frame),
                );
            }
        }
        return;
    }
    if raw.loops > 1 {
        warnings.push(format!(
            "sound '{}' repeats {} times; only its first pass is imported",
            raw.name, raw.loops
        ));
    }
    let (left, right) = raw
        .envelope
        .first()
        .map(|point| (point.1, point.2))
        .unwrap_or((1.0, 1.0));
    if raw
        .envelope
        .iter()
        .any(|point| point.1 != left || point.2 != right)
    {
        warnings.push(format!(
            "sound '{}' has an animated volume envelope; its initial volume is used",
            raw.name
        ));
    }
    let offset = raw.in_point.saturating_mul(48_000) / 44_100;
    let end_44 = raw
        .out_point
        .unwrap_or_else(|| sound.samples.saturating_mul(44_100) / u64::from(sound.sample_rate));
    let source_frames = ((end_44.saturating_sub(raw.in_point) as f64 / 44_100.0)
        * f64::from(project.meta.fps))
    .ceil()
    .clamp(0.0, f64::from(u16::MAX)) as u16;
    let duration = if raw.sync == "stream" {
        duration.min(source_frames)
    } else {
        source_frames
    }
    .min(frame_count.saturating_sub(start_frame));
    if duration == 0 {
        return;
    }
    // Balance in the native mixer attenuates one side while keeping the other.
    let gain = left.max(right);
    let pan = if gain == 0.0 {
        0.0
    } else if right >= left {
        1.0 - left / gain
    } else {
        right / gain - 1.0
    };
    project.audio_clips.push(AudioClip {
        q0rg_id,
        layer_id,
        start_frame,
        asset_id: sound.asset_id,
        gain,
        pan,
        source_offset_samples: offset,
        duration_frames: Some(duration),
        muted: false,
        fade_in_ms: 0,
        fade_out_ms: 0,
    });
}
