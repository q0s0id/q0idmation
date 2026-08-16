use std::collections::HashSet;
use std::fs::File;
use std::io::{BufReader, Cursor};
use std::path::Path;

use q0s_format::v2::{Asset, AudioClip, ProjectV2, Target};
use rodio::{Decoder, OutputStream, OutputStreamHandle, Sink, Source};

pub const EDITOR_AUDIO_SAMPLE_RATE: u32 = 48_000;
pub const EDITOR_AUDIO_CHANNELS: u16 = 2;
const MAX_IMPORTED_Q0V_BYTES: usize = 240 * 1024 * 1024;
const MAX_WAVEFORM_BINS: usize = 2048;

#[derive(Debug, Clone)]
pub struct AudioWaveform {
    pub bins: Vec<(f32, f32)>,
    pub samples_per_channel: u64,
    pub channels: u16,
    pub sample_rate: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioClipRef {
    pub clip_idx: usize,
    pub asset_id: u16,
    pub start_frame: u16,
    pub end_frame_exclusive: u16,
}

pub fn is_supported_audio_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            ["wav", "mp3", "ogg", "flac"]
                .iter()
                .any(|candidate| extension.eq_ignore_ascii_case(candidate))
        })
}

pub fn q0v_is_audio_only_bytes(bytes: &[u8]) -> bool {
    q0video::q0v::probe_header(bytes)
        .ok()
        .is_some_and(|header| header.spec.audio && !header.spec.video)
}

pub fn asset_is_audio_only(project: &ProjectV2, asset_id: u16) -> bool {
    project
        .assets
        .iter()
        .find(|asset| asset.id() == asset_id)
        .and_then(|asset| match asset {
            Asset::Q0v(media) => Some(q0v_is_audio_only_bytes(&media.bytes)),
            _ => None,
        })
        .unwrap_or(false)
}

pub fn library_item_is_audio_only(project: &ProjectV2, item: crate::state::LibraryItem) -> bool {
    match item {
        crate::state::LibraryItem::Asset(asset_id) => asset_is_audio_only(project, asset_id),
        crate::state::LibraryItem::Q0rg(_) => false,
    }
}

pub fn transcode_audio_to_q0v(path: &Path) -> Result<Vec<u8>, String> {
    let file = File::open(path).map_err(|error| format!("open audio file: {error}"))?;
    let decoder = Decoder::new(BufReader::new(file))
        .map_err(|error| format!("decode audio file: {error}"))?;
    let input_channels = decoder.channels();
    let input_sample_rate = decoder.sample_rate();
    if input_channels == 0 || input_sample_rate == 0 {
        return Err("audio decoder returned an invalid stream format".to_string());
    }

    // The project media ceiling applies to the final q0v PCM payload. Keep the
    // temporary decode bounded too, so a malformed or gigantic file cannot make
    // the import worker allocate without limit before conversion.
    let max_input_samples = MAX_IMPORTED_Q0V_BYTES / std::mem::size_of::<i16>();
    let mut input = Vec::new();
    for sample in decoder {
        if input.len() >= max_input_samples {
            return Err("audio import exceeds the 240 MiB project media limit".to_string());
        }
        input.push(sample);
    }
    if input.is_empty() {
        return Err("audio file contains no decodable samples".to_string());
    }

    let pcm = resample_to_editor_stereo(&input, input_channels, input_sample_rate)?;
    let frames = pcm.len() / usize::from(EDITOR_AUDIO_CHANNELS);
    let timeline_frames = u32::try_from(frames)
        .map_err(|_| "audio is too long for the q0v timeline clock".to_string())?;
    if timeline_frames == 0 {
        return Err("audio file is shorter than one output sample".to_string());
    }

    let spec = q0video::q0v::Q0vSpec {
        width: 0,
        height: 0,
        fps: EDITOR_AUDIO_SAMPLE_RATE,
        timeline_frames,
        video: false,
        audio: true,
        audio_sample_rate: EDITOR_AUDIO_SAMPLE_RATE,
        audio_channels: EDITOR_AUDIO_CHANNELS,
    };
    let mut writer = q0video::q0v::Q0vWriter::new(Cursor::new(Vec::new()), spec)?;
    writer.write_audio_pcm_i16(&pcm)?;
    let bytes = writer.finish()?.into_inner();
    if bytes.len() > MAX_IMPORTED_Q0V_BYTES {
        return Err("audio import exceeds the 240 MiB project media limit".to_string());
    }
    let parsed = q0video::q0v::Q0vFile::parse(bytes.clone())?;
    if parsed.spec != spec || parsed.audio_samples_per_channel != u64::from(timeline_frames) {
        return Err("audio q0v parse-back changed the stream specification".to_string());
    }
    Ok(bytes)
}

fn resample_to_editor_stereo(
    input: &[i16],
    input_channels: u16,
    input_sample_rate: u32,
) -> Result<Vec<i16>, String> {
    let channels = usize::from(input_channels);
    if channels == 0 || !input.len().is_multiple_of(channels) {
        return Err("decoded audio is not aligned to whole channel frames".to_string());
    }
    let input_frames = input.len() / channels;
    if input_frames == 0 {
        return Ok(Vec::new());
    }
    let output_frames = (u64::try_from(input_frames)
        .unwrap_or(u64::MAX)
        .saturating_mul(u64::from(EDITOR_AUDIO_SAMPLE_RATE))
        .div_ceil(u64::from(input_sample_rate))) as usize;
    let max_output_frames =
        MAX_IMPORTED_Q0V_BYTES / std::mem::size_of::<i16>() / usize::from(EDITOR_AUDIO_CHANNELS);
    if output_frames > max_output_frames {
        return Err("audio import exceeds the 240 MiB project media limit".to_string());
    }

    let mut output = Vec::with_capacity(output_frames.saturating_mul(2));
    for out_frame in 0..output_frames {
        let numerator = (out_frame as u64).saturating_mul(u64::from(input_sample_rate));
        let source_index = (numerator / u64::from(EDITOR_AUDIO_SAMPLE_RATE)) as usize;
        let remainder = numerator % u64::from(EDITOR_AUDIO_SAMPLE_RATE);
        let fraction = remainder as f32 / EDITOR_AUDIO_SAMPLE_RATE as f32;
        let next_index = (source_index + 1).min(input_frames - 1);
        for output_channel in 0..2 {
            let input_channel = if channels == 1 {
                0
            } else {
                output_channel.min(channels - 1)
            };
            let a = input[source_index.min(input_frames - 1) * channels + input_channel] as f32;
            let b = input[next_index * channels + input_channel] as f32;
            let value = a + (b - a) * fraction;
            output.push(value.round().clamp(i16::MIN as f32, i16::MAX as f32) as i16);
        }
    }
    Ok(output)
}

#[derive(Debug)]
pub struct AudioWaveformBuilder {
    waveform: AudioWaveform,
    source_frames: usize,
    next_bin: usize,
}

impl AudioWaveformBuilder {
    pub fn new(bytes: &[u8]) -> Option<Self> {
        let audio = q0video::q0v::audio_view(bytes).ok()?;
        if audio.spec.audio_channels == 0 {
            return None;
        }
        let channels = usize::from(audio.spec.audio_channels);
        let source_frames = audio.pcm_le_bytes().len() / 2 / channels;
        let bin_count = source_frames.clamp(1, MAX_WAVEFORM_BINS);
        Some(Self {
            waveform: AudioWaveform {
                bins: if source_frames == 0 {
                    Vec::new()
                } else {
                    vec![(0.0, 0.0); bin_count]
                },
                samples_per_channel: audio.samples_per_channel,
                channels: audio.spec.audio_channels,
                sample_rate: audio.spec.audio_sample_rate,
            },
            source_frames,
            next_bin: 0,
        })
    }

    pub fn waveform(&self) -> &AudioWaveform {
        &self.waveform
    }

    pub fn is_complete(&self) -> bool {
        self.next_bin >= self.waveform.bins.len()
    }

    /// Build at most `max_bins` exact peak bins. Runtime cost is therefore
    /// bounded by a small fixed slice of the song instead of its full length.
    pub fn advance(&mut self, bytes: &[u8], max_bins: usize) -> bool {
        if self.is_complete() || max_bins == 0 {
            return self.is_complete();
        }
        let Ok(audio) = q0video::q0v::audio_view(bytes) else {
            return false;
        };
        if audio.spec.audio_channels != self.waveform.channels
            || audio.samples_per_channel != self.waveform.samples_per_channel
        {
            return false;
        }
        let channels = usize::from(self.waveform.channels);
        let pcm = audio.pcm_le_bytes();
        let bin_count = self.waveform.bins.len();
        let end_bin = self.next_bin.saturating_add(max_bins).min(bin_count);
        for bin in self.next_bin..end_bin {
            let start_frame = bin.saturating_mul(self.source_frames) / bin_count;
            let end_frame = (bin + 1).saturating_mul(self.source_frames) / bin_count;
            let mut min = 1.0_f32;
            let mut max = -1.0_f32;
            for frame in start_frame..end_frame {
                for channel in 0..channels {
                    let sample_index = (frame * channels + channel) * 2;
                    let sample = i16::from_le_bytes([pcm[sample_index], pcm[sample_index + 1]])
                        as f32
                        / 32768.0;
                    min = min.min(sample);
                    max = max.max(sample);
                }
            }
            self.waveform.bins[bin] = if min <= max { (min, max) } else { (0.0, 0.0) };
        }
        self.next_bin = end_bin;
        self.is_complete()
    }
}

pub fn waveform_from_q0v_bytes(bytes: &[u8]) -> Option<AudioWaveform> {
    let mut builder = AudioWaveformBuilder::new(bytes)?;
    builder.advance(bytes, usize::MAX);
    Some(builder.waveform)
}

pub fn audio_clip_ref(project: &ProjectV2, clip_idx: usize) -> Option<AudioClipRef> {
    let clip = *project.audio_clips.get(clip_idx)?;
    let end_frame_exclusive = q0s_format::v2::audio_clip_end_frame(project, clip)?;
    Some(AudioClipRef {
        clip_idx,
        asset_id: clip.asset_id,
        start_frame: clip.start_frame,
        end_frame_exclusive,
    })
}

pub fn audio_clip_at_frame(
    project: &ProjectV2,
    q0rg_id: u16,
    layer_id: u16,
    frame: u16,
) -> Option<AudioClipRef> {
    project
        .audio_clips
        .iter()
        .enumerate()
        .filter(|(_, clip)| clip.q0rg_id == q0rg_id && clip.layer_id == layer_id)
        .filter_map(|(clip_idx, _)| audio_clip_ref(project, clip_idx))
        .filter(|clip| clip.start_frame <= frame && frame < clip.end_frame_exclusive)
        .max_by_key(|clip| clip.start_frame)
}

pub fn audio_range_intersects(
    project: &ProjectV2,
    q0rg_id: u16,
    layer_id: u16,
    start_frame: u16,
    end_frame_exclusive: u16,
) -> bool {
    project
        .audio_clips
        .iter()
        .enumerate()
        .any(|(clip_idx, clip)| {
            clip.q0rg_id == q0rg_id
                && clip.layer_id == layer_id
                && audio_clip_ref(project, clip_idx).is_some_and(|resolved| {
                    start_frame < resolved.end_frame_exclusive
                        && resolved.start_frame < end_frame_exclusive
                })
        })
}

pub fn visual_content_conflicts_with_audio_range(
    project: &ProjectV2,
    q0rg_id: u16,
    layer_id: u16,
    start_frame: u16,
    end_frame_exclusive: u16,
) -> bool {
    let Some(layer) = project
        .q0rgs
        .iter()
        .find(|q0rg| q0rg.q0rg_id == q0rg_id)
        .and_then(|q0rg| q0rg.layers.iter().find(|layer| layer.layer_id == layer_id))
    else {
        return true;
    };
    if layer
        .keyframe_frames()
        .into_iter()
        .any(|frame| (start_frame..end_frame_exclusive).contains(&frame))
    {
        return true;
    }
    if let Some(previous_key) = layer
        .keyframe_frames()
        .into_iter()
        .filter(|frame| *frame < start_frame)
        .max()
    {
        if layer
            .placements
            .iter()
            .any(|placement| placement.frame == previous_key)
        {
            return true;
        }
    }
    layer
        .placements
        .iter()
        .any(|placement| (start_frame..end_frame_exclusive).contains(&placement.frame))
}

/// Convert the short-lived v19 editor representation where audio was stored as
/// an ordinary display placement. This runs once on load so files made by that
/// build keep their sound while audio disappears from stage/keyframe semantics.
pub fn migrate_legacy_audio_placements(project: &mut ProjectV2) -> usize {
    let audio_assets = project
        .assets
        .iter()
        .filter_map(|asset| match asset {
            Asset::Q0v(media) if q0v_is_audio_only_bytes(&media.bytes) => Some(media.asset_id),
            _ => None,
        })
        .collect::<HashSet<_>>();
    if audio_assets.is_empty() {
        return 0;
    }
    let mut migrated = Vec::new();
    for q0rg in &mut project.q0rgs {
        for layer in &mut q0rg.layers {
            let mut visual = Vec::with_capacity(layer.placements.len());
            for placement in layer.placements.drain(..) {
                let Target::Asset(asset_id) = placement.target else {
                    visual.push(placement);
                    continue;
                };
                if !audio_assets.contains(&asset_id) {
                    visual.push(placement);
                    continue;
                }
                migrated.push(AudioClip {
                    q0rg_id: q0rg.q0rg_id,
                    layer_id: layer.layer_id,
                    start_frame: placement.frame,
                    asset_id,
                    gain: placement.fx.audio_gain.clamp(0.0, 4.0),
                    muted: placement.fx.audio_muted,
                });
            }
            layer.placements = visual;
        }
    }
    let count = migrated.len();
    for clip in migrated {
        if !project.audio_clips.iter().any(|current| {
            current.q0rg_id == clip.q0rg_id
                && current.layer_id == clip.layer_id
                && current.start_frame == clip.start_frame
                && current.asset_id == clip.asset_id
        }) {
            project.audio_clips.push(clip);
        }
    }
    count
}

const PLAYBACK_CHUNK_SAMPLE_FRAMES: usize = EDITOR_AUDIO_SAMPLE_RATE as usize / 4;
const PLAYBACK_QUEUE_CHUNKS: usize = 3;

pub fn has_audible_timeline_audio(project: &ProjectV2, q0rg_id: u16) -> bool {
    project
        .audio_clips
        .iter()
        .filter(|clip| clip.q0rg_id == q0rg_id && !clip.muted && clip.gain > 0.0)
        .any(|clip| {
            project
                .assets
                .iter()
                .find(|asset| asset.id() == clip.asset_id)
                .and_then(|asset| match asset {
                    Asset::Q0v(media) => q0video::q0v::probe_header(&media.bytes).ok(),
                    _ => None,
                })
                .is_some_and(|header| header.spec.audio && !header.spec.video)
        })
}

fn timeline_total_sample_frames(project: &ProjectV2, q0rg_id: u16) -> Option<u64> {
    let q0rg = project.q0rgs.iter().find(|q0rg| q0rg.q0rg_id == q0rg_id)?;
    let host_fps = u64::from(project.meta.fps.max(1));
    Some(
        u64::from(q0rg.frame_count.max(1))
            .saturating_mul(u64::from(EDITOR_AUDIO_SAMPLE_RATE))
            .div_ceil(host_fps)
            .max(1),
    )
}

fn pcm_sample(view: q0video::q0v::Q0vAudioView<'_>, frame: u64, channel: u16) -> f32 {
    if frame >= view.samples_per_channel || view.spec.audio_channels == 0 {
        return 0.0;
    }
    let source_channel = if view.spec.audio_channels == 1 {
        0
    } else {
        channel.min(view.spec.audio_channels - 1)
    };
    let channels = u64::from(view.spec.audio_channels);
    let sample_index = frame
        .saturating_mul(channels)
        .saturating_add(u64::from(source_channel));
    let byte_index = match usize::try_from(sample_index.saturating_mul(2)) {
        Ok(index) => index,
        Err(_) => return 0.0,
    };
    let pcm = view.pcm_le_bytes();
    let Some(raw) = pcm.get(byte_index..byte_index.saturating_add(2)) else {
        return 0.0;
    };
    i16::from_le_bytes([raw[0], raw[1]]) as f32 / 32768.0
}

fn sample_audio_view_at_editor_frame(
    view: q0video::q0v::Q0vAudioView<'_>,
    output_frame: u64,
    output_channel: u16,
) -> f32 {
    let input_rate = u64::from(view.spec.audio_sample_rate.max(1));
    let output_rate = u64::from(EDITOR_AUDIO_SAMPLE_RATE);
    let numerator = output_frame.saturating_mul(input_rate);
    let source_frame = numerator / output_rate;
    if source_frame >= view.samples_per_channel {
        return 0.0;
    }
    let next_frame = source_frame
        .saturating_add(1)
        .min(view.samples_per_channel.saturating_sub(1));
    let fraction = (numerator % output_rate) as f32 / EDITOR_AUDIO_SAMPLE_RATE as f32;
    let a = pcm_sample(view, source_frame, output_channel);
    let b = pcm_sample(view, next_frame, output_channel);
    a + (b - a) * fraction
}

fn audio_view_duration_at_editor_rate(view: q0video::q0v::Q0vAudioView<'_>) -> u64 {
    view.samples_per_channel
        .saturating_mul(u64::from(EDITOR_AUDIO_SAMPLE_RATE))
        .div_ceil(u64::from(view.spec.audio_sample_rate.max(1)))
}

fn mix_timeline_audio_chunk(
    project: &ProjectV2,
    q0rg_id: u16,
    start_sample_frame: u64,
    sample_frames: usize,
) -> Option<Vec<f32>> {
    let q0rg = project.q0rgs.iter().find(|q0rg| q0rg.q0rg_id == q0rg_id)?;
    let host_fps = u64::from(project.meta.fps.max(1));
    let total_sample_frames = timeline_total_sample_frames(project, q0rg_id)?;
    let mut output =
        vec![0.0_f32; sample_frames.saturating_mul(usize::from(EDITOR_AUDIO_CHANNELS))];
    let mut found_audio = false;

    for clip in project
        .audio_clips
        .iter()
        .filter(|clip| clip.q0rg_id == q0rg_id && !clip.muted && clip.gain > 0.0)
    {
        let Some(Asset::Q0v(media)) = project
            .assets
            .iter()
            .find(|asset| asset.id() == clip.asset_id)
        else {
            continue;
        };
        let Ok(view) = q0video::q0v::audio_view(&media.bytes) else {
            continue;
        };
        if view.spec.video || !view.spec.audio {
            continue;
        }
        found_audio = true;
        let clip_start = u64::from(clip.start_frame)
            .saturating_mul(u64::from(EDITOR_AUDIO_SAMPLE_RATE))
            / host_fps;
        let boundary_frame =
            q0s_format::v2::audio_clip_end_frame(project, *clip).unwrap_or(q0rg.frame_count);
        let boundary_sample = u64::from(boundary_frame)
            .saturating_mul(u64::from(EDITOR_AUDIO_SAMPLE_RATE))
            / host_fps;
        let clip_end = clip_start
            .saturating_add(audio_view_duration_at_editor_rate(view))
            .min(boundary_sample)
            .min(total_sample_frames);
        if clip_end <= clip_start {
            continue;
        }
        let gain = clip.gain.clamp(0.0, 4.0);
        for output_frame in 0..sample_frames {
            let timeline_frame =
                start_sample_frame.saturating_add(output_frame as u64) % total_sample_frames;
            if timeline_frame < clip_start || timeline_frame >= clip_end {
                continue;
            }
            let local_frame = timeline_frame - clip_start;
            let base = output_frame * usize::from(EDITOR_AUDIO_CHANNELS);
            for channel in 0..EDITOR_AUDIO_CHANNELS {
                output[base + usize::from(channel)] +=
                    sample_audio_view_at_editor_frame(view, local_frame, channel) * gain;
            }
        }
    }
    if !found_audio {
        return None;
    }
    for sample in &mut output {
        *sample = sample.clamp(-1.0, 1.0);
    }
    Some(output)
}

pub struct EditorAudioPlayback {
    _stream: OutputStream,
    handle: OutputStreamHandle,
    sink: Option<Sink>,
    q0rg_id: Option<u16>,
    next_sample_frame: u64,
    total_sample_frames: u64,
}

impl EditorAudioPlayback {
    pub fn new() -> Result<Self, String> {
        let (stream, handle) = OutputStream::try_default()
            .map_err(|error| format!("open default audio output: {error}"))?;
        Ok(Self {
            _stream: stream,
            handle,
            sink: None,
            q0rg_id: None,
            next_sample_frame: 0,
            total_sample_frames: 0,
        })
    }

    pub fn sync(
        &mut self,
        project: &ProjectV2,
        q0rg_id: u16,
        start_frame: u16,
        playing: bool,
    ) -> Result<(), String> {
        self.stop();
        if !has_audible_timeline_audio(project, q0rg_id) {
            return Ok(());
        }
        let Some(total_sample_frames) = timeline_total_sample_frames(project, q0rg_id) else {
            return Ok(());
        };
        let host_fps = u64::from(project.meta.fps.max(1));
        let start_sample_frame =
            u64::from(start_frame).saturating_mul(u64::from(EDITOR_AUDIO_SAMPLE_RATE)) / host_fps;
        let sink = Sink::try_new(&self.handle)
            .map_err(|error| format!("create editor audio sink: {error}"))?;
        if !playing {
            sink.pause();
        }
        self.sink = Some(sink);
        self.q0rg_id = Some(q0rg_id);
        self.next_sample_frame = start_sample_frame % total_sample_frames;
        self.total_sample_frames = total_sample_frames;
        self.pump(project, q0rg_id);
        Ok(())
    }

    pub fn pump(&mut self, project: &ProjectV2, q0rg_id: u16) {
        if self.q0rg_id != Some(q0rg_id) || self.total_sample_frames == 0 {
            return;
        }
        if self
            .sink
            .as_ref()
            .is_none_or(|sink| sink.len() >= PLAYBACK_QUEUE_CHUNKS)
        {
            return;
        }
        let start = self.next_sample_frame;
        let Some(samples) =
            mix_timeline_audio_chunk(project, q0rg_id, start, PLAYBACK_CHUNK_SAMPLE_FRAMES)
        else {
            return;
        };
        let Some(sink) = self.sink.as_ref() else {
            return;
        };
        sink.append(rodio::buffer::SamplesBuffer::new(
            EDITOR_AUDIO_CHANNELS,
            EDITOR_AUDIO_SAMPLE_RATE,
            samples,
        ));
        self.next_sample_frame =
            start.saturating_add(PLAYBACK_CHUNK_SAMPLE_FRAMES as u64) % self.total_sample_frames;
    }

    pub fn set_playing(&self, playing: bool) {
        let Some(sink) = self.sink.as_ref() else {
            return;
        };
        if playing {
            sink.play();
        } else {
            sink.pause();
        }
    }

    pub fn stop(&mut self) {
        if let Some(sink) = self.sink.take() {
            sink.stop();
        }
        self.q0rg_id = None;
        self.next_sample_frame = 0;
        self.total_sample_frames = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use q0s_format::v2::{AudioClip, Layer, ProjectMeta, Q0rg, Q0vAsset};
    use std::collections::HashMap;

    fn audio_q0v(samples: &[i16], sample_rate: u32, channels: u16) -> Vec<u8> {
        let frames = samples.len() / usize::from(channels);
        let spec = q0video::q0v::Q0vSpec {
            width: 0,
            height: 0,
            fps: sample_rate,
            timeline_frames: frames as u32,
            video: false,
            audio: true,
            audio_sample_rate: sample_rate,
            audio_channels: channels,
        };
        let mut writer = q0video::q0v::Q0vWriter::new(Cursor::new(Vec::new()), spec).unwrap();
        writer.write_audio_pcm_i16(samples).unwrap();
        writer.finish().unwrap().into_inner()
    }

    fn project_with_audio(bytes: Vec<u8>, start_frame: u16) -> ProjectV2 {
        ProjectV2 {
            meta: ProjectMeta {
                name: "audio-test".to_string(),
                fps: 24,
                stage_width: 640,
                stage_height: 480,
                entry_q0rg_id: 1,
            },
            assets: vec![Asset::Q0v(Q0vAsset { asset_id: 7, bytes })],
            asset_names: HashMap::new(),
            asset_appearances: HashMap::new(),
            layer_metadata: HashMap::new(),
            audio_clips: vec![AudioClip {
                q0rg_id: 1,
                layer_id: 1,
                start_frame,
                asset_id: 7,
                gain: 1.0,
                muted: false,
            }],
            runtime: Default::default(),
            q0rgs: vec![Q0rg {
                q0rg_id: 1,
                name: "Stage".to_string(),
                frame_count: 48,
                script: String::new(),
                layers: vec![Layer {
                    layer_id: 1,
                    name: "sound".to_string(),
                    explicit_keyframes: Vec::new(),
                    placements: Vec::new(),
                }],
            }],
        }
    }

    #[test]
    fn recognizes_supported_audio_extensions_case_insensitively() {
        for name in ["voice.wav", "VOICE.MP3", "ambience.OgG", "music.FLAC"] {
            assert!(is_supported_audio_path(Path::new(name)), "{name}");
        }
        assert!(!is_supported_audio_path(Path::new("not-audio.png")));
    }

    #[test]
    fn wav_import_decodes_to_audio_only_q0v() {
        let path =
            std::env::temp_dir().join(format!("q0editor-audio-import-{}.wav", std::process::id()));
        let samples = [0_i16, 12_000, -12_000, 24_000, -24_000, 0];
        let mut wav = Vec::new();
        let data_bytes = (samples.len() * 2) as u32;
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + data_bytes).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16_u32.to_le_bytes());
        wav.extend_from_slice(&1_u16.to_le_bytes());
        wav.extend_from_slice(&1_u16.to_le_bytes());
        wav.extend_from_slice(&24_000_u32.to_le_bytes());
        wav.extend_from_slice(&(24_000_u32 * 2).to_le_bytes());
        wav.extend_from_slice(&2_u16.to_le_bytes());
        wav.extend_from_slice(&16_u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&data_bytes.to_le_bytes());
        for sample in samples {
            wav.extend_from_slice(&sample.to_le_bytes());
        }
        std::fs::write(&path, wav).expect("write wav fixture");
        let bytes = transcode_audio_to_q0v(&path).expect("transcode wav");
        let media = q0video::q0v::Q0vFile::parse(bytes).expect("parse imported q0v");
        assert!(media.spec.audio);
        assert!(!media.spec.video);
        assert_eq!(media.spec.audio_sample_rate, EDITOR_AUDIO_SAMPLE_RATE);
        assert_eq!(media.spec.audio_channels, EDITOR_AUDIO_CHANNELS);
        assert!(media.audio_samples_per_channel > 0);
        std::fs::remove_file(path).expect("cleanup wav fixture");
    }

    #[test]
    fn audio_only_q0v_is_not_a_visual_asset() {
        let bytes = audio_q0v(&[0, 0, 1000, -1000], 48_000, 2);
        assert!(q0v_is_audio_only_bytes(&bytes));
        let project = project_with_audio(bytes, 0);
        assert!(asset_is_audio_only(&project, 7));
    }

    #[test]
    fn timeline_chunk_starts_audio_at_its_timeline_frame() {
        // 2 stereo frames at 48 kHz, placed at project frame 1. At 24 fps
        // that means 2000 silent sample frames before the clip begins.
        let bytes = audio_q0v(&[16_384, -16_384, 8192, -8192], 48_000, 2);
        let project = project_with_audio(bytes, 1);
        let chunk = mix_timeline_audio_chunk(&project, 1, 0, 2002).unwrap();
        assert!(chunk[..2000 * 2].iter().all(|sample| *sample == 0.0));
        assert!((chunk[2000 * 2] - 0.5).abs() < 0.001);
        assert!((chunk[2000 * 2 + 1] + 0.5).abs() < 0.001);
    }

    #[test]
    fn timeline_playback_chunk_stays_bounded_for_long_audio() {
        let frames = EDITOR_AUDIO_SAMPLE_RATE as usize * 2;
        let mut pcm = Vec::with_capacity(frames * 2);
        for _ in 0..frames {
            pcm.extend_from_slice(&[4096, -4096]);
        }
        let project = project_with_audio(audio_q0v(&pcm, 48_000, 2), 0);
        let chunk = mix_timeline_audio_chunk(&project, 1, 0, PLAYBACK_CHUNK_SAMPLE_FRAMES).unwrap();
        assert_eq!(
            chunk.len(),
            PLAYBACK_CHUNK_SAMPLE_FRAMES * usize::from(EDITOR_AUDIO_CHANNELS)
        );
        assert!(chunk.len() < pcm.len() / 4);
    }

    #[test]
    fn muted_timeline_audio_is_rejected_before_playback_setup() {
        let bytes = audio_q0v(&[16_384, -16_384], 48_000, 2);
        let mut project = project_with_audio(bytes, 0);
        assert!(has_audible_timeline_audio(&project, 1));
        project.audio_clips[0].muted = true;
        assert!(!has_audible_timeline_audio(&project, 1));
    }

    #[test]
    fn waveform_builder_limits_each_repaint_to_requested_bins() {
        let mut pcm = Vec::with_capacity(8_192 * 2);
        for _ in 0..8_192 {
            pcm.extend_from_slice(&[-16_384, 16_384]);
        }
        let bytes = audio_q0v(&pcm, 48_000, 2);
        let mut builder = AudioWaveformBuilder::new(&bytes).expect("waveform builder");
        assert_eq!(builder.next_bin, 0);
        assert!(!builder.is_complete());

        assert!(!builder.advance(&bytes, 3));
        assert_eq!(builder.next_bin, 3);
        assert!(builder.waveform.bins[..3]
            .iter()
            .all(|(min, max)| *min <= -0.49 && *max >= 0.49));
        assert_eq!(builder.waveform.bins[3], (0.0, 0.0));

        assert!(builder.advance(&bytes, usize::MAX));
        assert!(builder.is_complete());
        assert!(builder
            .waveform
            .bins
            .iter()
            .all(|(min, max)| *min <= -0.49 && *max >= 0.49));
    }

    #[test]
    fn waveform_preserves_positive_and_negative_peaks() {
        let bytes = audio_q0v(&[-32768, 32767, 0, 0, -8192, 16_384, 0, 0], 48_000, 2);
        let waveform = waveform_from_q0v_bytes(&bytes).unwrap();
        assert!(!waveform.bins.is_empty());
        assert!(waveform.bins.iter().any(|(min, _)| *min <= -0.99));
        assert!(waveform.bins.iter().any(|(_, max)| *max >= 0.99));
    }

    #[test]
    fn legacy_audio_placement_migrates_to_timeline_clip_with_fx() {
        let bytes = audio_q0v(&vec![0; 4_800 * 2], 48_000, 2);
        let mut project = project_with_audio(bytes, 3);
        project.audio_clips.clear();
        let fx = q0s_format::v2::PlacementFx {
            audio_gain: 0.42,
            audio_muted: true,
            ..Default::default()
        };
        project.q0rgs[0].layers[0]
            .placements
            .push(q0s_format::v2::Placement {
                instance_id: 0,
                frame: 3,
                target: Target::Asset(7),
                transform: q0s_format::v2::Transform2D::IDENTITY,
                tween: q0s_format::v2::Tween::None,
                fx,
            });

        assert_eq!(migrate_legacy_audio_placements(&mut project), 1);
        assert!(project.q0rgs[0].layers[0].placements.is_empty());
        assert_eq!(project.audio_clips.len(), 1);
        let clip = project.audio_clips[0];
        assert_eq!(clip.q0rg_id, 1);
        assert_eq!(clip.layer_id, 1);
        assert_eq!(clip.start_frame, 3);
        assert_eq!(clip.asset_id, 7);
        assert!((clip.gain - 0.42).abs() < 1.0e-6);
        assert!(clip.muted);
        q0s_format::v2::validate(&project).expect("migrated audio project");
    }
}
