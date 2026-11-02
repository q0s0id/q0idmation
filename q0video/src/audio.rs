//! Portable decoding of the original compressed audio embedded in q0v v2.
use std::io::{Cursor, ErrorKind};

use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::errors::Error;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

pub const MAX_DECODED_AUDIO_BYTES: usize = 240 * 1024 * 1024;

pub struct DecodedAudio {
    pub sample_rate: u32,
    pub channels: u16,
    pub pcm_le_bytes: Vec<u8>,
}

impl DecodedAudio {
    pub fn samples_per_channel(&self) -> u64 {
        self.pcm_le_bytes.len() as u64 / 2 / u64::from(self.channels)
    }
}

pub fn decode_source(bytes: &[u8]) -> Result<DecodedAudio, String> {
    let stream = MediaSourceStream::new(Box::new(Cursor::new(bytes.to_vec())), Default::default());
    let options = FormatOptions {
        enable_gapless: true,
        ..Default::default()
    };
    let mut format = symphonia::default::get_probe()
        .format(&Hint::new(), stream, &options, &MetadataOptions::default())
        .map_err(|error| format!("probe embedded audio: {error}"))?
        .format;
    let track = format.default_track().ok_or("audio has no default track")?;
    let track_id = track.id;
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|error| format!("create audio decoder: {error}"))?;
    let mut output = DecodedAudio {
        sample_rate: 0,
        channels: 0,
        pcm_le_bytes: Vec::new(),
    };
    loop {
        let packet = match format.next_packet() {
            Ok(packet) => packet,
            Err(Error::IoError(error)) if error.kind() == ErrorKind::UnexpectedEof => break,
            Err(error) => return Err(format!("read embedded audio: {error}")),
        };
        if packet.track_id() != track_id {
            continue;
        }
        let decoded = decoder
            .decode(&packet)
            .map_err(|error| format!("decode embedded audio: {error}"))?;
        let spec = *decoded.spec();
        let channels =
            u16::try_from(spec.channels.count()).map_err(|_| "too many audio channels")?;
        if spec.rate == 0 || channels == 0 {
            return Err("audio decoder returned an invalid format".into());
        }
        if output.sample_rate == 0 {
            output.sample_rate = spec.rate;
            output.channels = channels;
        } else if output.sample_rate != spec.rate || output.channels != channels {
            return Err("audio changes sample rate or channel count midstream".into());
        }
        let mut samples = SampleBuffer::<i16>::new(decoded.capacity() as u64, spec);
        samples.copy_interleaved_ref(decoded);
        let byte_count = samples.len().checked_mul(2).ok_or("audio size overflows")?;
        if output.pcm_le_bytes.len().saturating_add(byte_count) > MAX_DECODED_AUDIO_BYTES {
            return Err("decoded audio exceeds the 240 MiB media limit".into());
        }
        output.pcm_le_bytes.extend(
            samples
                .samples()
                .iter()
                .flat_map(|sample| sample.to_le_bytes()),
        );
    }
    if output.pcm_le_bytes.is_empty() {
        return Err("audio contains no decodable samples".into());
    }
    Ok(output)
}
