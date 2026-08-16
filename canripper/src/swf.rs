use std::io::{Cursor, Read};

use flate2::read::ZlibDecoder;

const MAX_SWF_BYTES: usize = 512 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwfCompression {
    None,
    Zlib,
    Lzma,
}

impl SwfCompression {
    pub const fn label(self) -> &'static str {
        match self {
            Self::None => "uncompressed",
            Self::Zlib => "zlib",
            Self::Lzma => "lzma",
        }
    }
}

#[derive(Debug, Clone)]
pub struct SwfTag {
    pub index: usize,
    pub code: u16,
    pub name: &'static str,
    pub payload_offset: usize,
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct SwfMovie {
    pub compression: SwfCompression,
    pub version: u8,
    pub declared_file_length: u32,
    pub source_length: usize,
    pub width_px: f32,
    pub height_px: f32,
    pub frame_rate: f32,
    pub frame_count: u16,
    pub tags: Vec<SwfTag>,
    pub normalized_fws: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct EmbeddedSwf {
    pub offset: usize,
    pub movie: SwfMovie,
}

pub fn looks_like_swf(bytes: &[u8]) -> bool {
    bytes.len() >= 8 && matches!(&bytes[0..3], b"FWS" | b"CWS" | b"ZWS")
}

pub fn parse_swf(bytes: &[u8]) -> Result<SwfMovie, String> {
    parse_swf_inner(bytes, false)
}

pub fn find_projector_swf(bytes: &[u8]) -> Result<EmbeddedSwf, String> {
    if !looks_like_windows_pe(bytes) {
        return Err("not a Windows PE executable".to_string());
    }

    let mut candidates = Vec::new();
    for offset in 0..=bytes.len().saturating_sub(8) {
        if matches!(&bytes[offset..offset + 3], b"FWS" | b"CWS" | b"ZWS") {
            candidates.push(offset);
        }
    }

    for offset in candidates.into_iter().rev() {
        let candidate = &bytes[offset..];
        let version = candidate[3];
        if version == 0 || version > 64 {
            continue;
        }
        if let Ok(movie) = parse_swf_inner(candidate, true) {
            return Ok(EmbeddedSwf { offset, movie });
        }
    }

    Err("no valid embedded SWF was found in the executable".to_string())
}

fn parse_swf_inner(bytes: &[u8], allow_trailing: bool) -> Result<SwfMovie, String> {
    if bytes.len() < 8 {
        return Err("SWF header is truncated".to_string());
    }

    let compression = match &bytes[0..3] {
        b"FWS" => SwfCompression::None,
        b"CWS" => SwfCompression::Zlib,
        b"ZWS" => SwfCompression::Lzma,
        _ => return Err("SWF signature must be FWS, CWS, or ZWS".to_string()),
    };
    let version = bytes[3];
    if version == 0 || version > 64 {
        return Err(format!("implausible SWF version {version}"));
    }
    let declared_file_length = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
    let declared_len = usize::try_from(declared_file_length)
        .map_err(|_| "SWF declared length does not fit this platform".to_string())?;
    if !(8..=MAX_SWF_BYTES).contains(&declared_len) {
        return Err(format!(
            "SWF declared length {declared_len} is outside the safe limit"
        ));
    }

    let (normalized_fws, source_length) = match compression {
        SwfCompression::None => {
            if bytes.len() < declared_len {
                return Err("uncompressed SWF is shorter than its declared length".to_string());
            }
            if !allow_trailing && bytes.len() != declared_len {
                return Err("uncompressed SWF has trailing bytes".to_string());
            }
            (bytes[..declared_len].to_vec(), declared_len)
        }
        SwfCompression::Zlib => {
            let expected_body = declared_len - 8;
            let mut decoder = ZlibDecoder::new(&bytes[8..]);
            let mut body = Vec::with_capacity(expected_body.min(8 * 1024 * 1024));
            decoder
                .by_ref()
                .take((expected_body as u64) + 1)
                .read_to_end(&mut body)
                .map_err(|error| format!("decompress CWS zlib stream: {error}"))?;
            if body.len() != expected_body {
                return Err(format!(
                    "CWS expands to {} bytes, expected {expected_body}",
                    body.len()
                ));
            }
            let compressed_len = usize::try_from(decoder.total_in())
                .map_err(|_| "compressed CWS length overflow".to_string())?;
            let source_length = 8usize
                .checked_add(compressed_len)
                .ok_or_else(|| "compressed CWS length overflow".to_string())?;
            if source_length > bytes.len() {
                return Err("CWS decoder consumed beyond input".to_string());
            }
            if !allow_trailing && source_length != bytes.len() {
                return Err("CWS has trailing bytes".to_string());
            }

            let mut normalized = Vec::with_capacity(declared_len);
            normalized.extend_from_slice(b"FWS");
            normalized.push(version);
            normalized.extend_from_slice(&declared_file_length.to_le_bytes());
            normalized.extend_from_slice(&body);
            (normalized, source_length)
        }
        SwfCompression::Lzma => {
            if version < 13 {
                return Err(format!("ZWS/LZMA requires SWF 13+, got version {version}"));
            }
            if bytes.len() < 17 {
                return Err("ZWS header is truncated".to_string());
            }
            let compressed_len = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
            let compressed_len = usize::try_from(compressed_len)
                .map_err(|_| "ZWS compressed length does not fit this platform".to_string())?;
            let source_length = 17usize
                .checked_add(compressed_len)
                .ok_or_else(|| "ZWS compressed length overflow".to_string())?;
            if source_length > bytes.len() {
                return Err("ZWS compressed payload is truncated".to_string());
            }
            if !allow_trailing && source_length != bytes.len() {
                return Err("ZWS has trailing bytes".to_string());
            }

            let expected_body = declared_len - 8;
            let mut alone = Vec::with_capacity(13 + compressed_len);
            alone.extend_from_slice(&bytes[12..17]);
            alone.extend_from_slice(&(expected_body as u64).to_le_bytes());
            alone.extend_from_slice(&bytes[17..source_length]);
            let mut body = Vec::with_capacity(expected_body.min(8 * 1024 * 1024));
            lzma_rs::lzma_decompress(&mut Cursor::new(alone), &mut body)
                .map_err(|error| format!("decompress ZWS LZMA stream: {error}"))?;
            if body.len() != expected_body {
                return Err(format!(
                    "ZWS expands to {} bytes, expected {expected_body}",
                    body.len()
                ));
            }

            let mut normalized = Vec::with_capacity(declared_len);
            normalized.extend_from_slice(b"FWS");
            normalized.push(version);
            normalized.extend_from_slice(&declared_file_length.to_le_bytes());
            normalized.extend_from_slice(&body);
            (normalized, source_length)
        }
    };

    let (rect_bytes, width_px, height_px) = parse_rect(&normalized_fws[8..])?;
    let timeline_offset = 8 + rect_bytes;
    if normalized_fws.len() < timeline_offset + 4 {
        return Err("SWF timeline header is truncated".to_string());
    }
    let frame_rate_raw = u16::from_le_bytes([
        normalized_fws[timeline_offset],
        normalized_fws[timeline_offset + 1],
    ]);
    let frame_rate = f32::from(frame_rate_raw) / 256.0;
    let frame_count = u16::from_le_bytes([
        normalized_fws[timeline_offset + 2],
        normalized_fws[timeline_offset + 3],
    ]);
    let tags = parse_tags(&normalized_fws, timeline_offset + 4)?;

    Ok(SwfMovie {
        compression,
        version,
        declared_file_length,
        source_length,
        width_px,
        height_px,
        frame_rate,
        frame_count,
        tags,
        normalized_fws,
    })
}

fn looks_like_windows_pe(bytes: &[u8]) -> bool {
    if bytes.len() < 0x40 || &bytes[0..2] != b"MZ" {
        return false;
    }
    let pe_offset = u32::from_le_bytes(bytes[0x3c..0x40].try_into().unwrap());
    let Ok(pe_offset) = usize::try_from(pe_offset) else {
        return false;
    };
    let Some(end) = pe_offset.checked_add(4) else {
        return false;
    };
    end <= bytes.len() && &bytes[pe_offset..end] == b"PE\0\0"
}

fn parse_rect(bytes: &[u8]) -> Result<(usize, f32, f32), String> {
    if bytes.is_empty() {
        return Err("SWF RECT is missing".to_string());
    }
    let mut reader = BitReader::new(bytes);
    let nbits = reader.read_unsigned(5)? as usize;
    if nbits == 0 || nbits > 31 {
        return Err(format!("invalid SWF RECT bit width {nbits}"));
    }
    let xmin = reader.read_signed(nbits)?;
    let xmax = reader.read_signed(nbits)?;
    let ymin = reader.read_signed(nbits)?;
    let ymax = reader.read_signed(nbits)?;
    let consumed = reader.bytes_consumed();
    let width_px = (xmax - xmin) as f32 / 20.0;
    let height_px = (ymax - ymin) as f32 / 20.0;
    if width_px < 0.0 || height_px < 0.0 {
        return Err("SWF RECT has inverted bounds".to_string());
    }
    Ok((consumed, width_px, height_px))
}

fn parse_tags(bytes: &[u8], mut offset: usize) -> Result<Vec<SwfTag>, String> {
    let mut tags = Vec::new();
    let mut saw_end = false;
    while offset < bytes.len() {
        if bytes.len() - offset < 2 {
            return Err("SWF tag header is truncated".to_string());
        }
        let record = u16::from_le_bytes([bytes[offset], bytes[offset + 1]]);
        offset += 2;
        let code = record >> 6;
        let short_len = usize::from(record & 0x3f);
        let payload_len = if short_len == 0x3f {
            if bytes.len() - offset < 4 {
                return Err("SWF long tag length is truncated".to_string());
            }
            let len = u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap());
            offset += 4;
            usize::try_from(len).map_err(|_| "SWF tag length overflow".to_string())?
        } else {
            short_len
        };
        let end = offset
            .checked_add(payload_len)
            .ok_or_else(|| "SWF tag payload length overflow".to_string())?;
        if end > bytes.len() {
            return Err(format!("SWF tag {code} payload is truncated"));
        }
        let index = tags.len();
        tags.push(SwfTag {
            index,
            code,
            name: tag_name(code),
            payload_offset: offset,
            payload: bytes[offset..end].to_vec(),
        });
        offset = end;
        if code == 0 {
            saw_end = true;
            break;
        }
    }
    if !saw_end {
        return Err("SWF is missing its End tag".to_string());
    }
    Ok(tags)
}

pub fn tag_name(code: u16) -> &'static str {
    match code {
        0 => "End",
        1 => "ShowFrame",
        2 => "DefineShape",
        4 => "PlaceObject",
        5 => "RemoveObject",
        6 => "DefineBits",
        7 => "DefineButton",
        8 => "JPEGTables",
        9 => "SetBackgroundColor",
        10 => "DefineFont",
        11 => "DefineText",
        12 => "DoAction",
        13 => "DefineFontInfo",
        14 => "DefineSound",
        15 => "StartSound",
        17 => "DefineButtonSound",
        18 => "SoundStreamHead",
        19 => "SoundStreamBlock",
        20 => "DefineBitsLossless",
        21 => "DefineBitsJPEG2",
        22 => "DefineShape2",
        23 => "DefineButtonCxform",
        24 => "Protect",
        26 => "PlaceObject2",
        28 => "RemoveObject2",
        32 => "DefineShape3",
        33 => "DefineText2",
        34 => "DefineButton2",
        35 => "DefineBitsJPEG3",
        36 => "DefineBitsLossless2",
        37 => "DefineEditText",
        39 => "DefineSprite",
        43 => "FrameLabel",
        45 => "SoundStreamHead2",
        46 => "DefineMorphShape",
        48 => "DefineFont2",
        56 => "ExportAssets",
        57 => "ImportAssets",
        58 => "EnableDebugger",
        59 => "DoInitAction",
        60 => "DefineVideoStream",
        61 => "VideoFrame",
        62 => "DefineFontInfo2",
        64 => "EnableDebugger2",
        65 => "ScriptLimits",
        66 => "SetTabIndex",
        69 => "FileAttributes",
        70 => "PlaceObject3",
        71 => "ImportAssets2",
        73 => "DefineFontAlignZones",
        74 => "CSMTextSettings",
        75 => "DefineFont3",
        76 => "SymbolClass",
        77 => "Metadata",
        78 => "DefineScalingGrid",
        82 => "DoABC",
        83 => "DefineShape4",
        84 => "DefineMorphShape2",
        86 => "DefineSceneAndFrameLabelData",
        87 => "DefineBinaryData",
        88 => "DefineFontName",
        89 => "StartSound2",
        90 => "DefineBitsJPEG4",
        91 => "DefineFont4",
        _ => "Unknown",
    }
}

struct BitReader<'a> {
    bytes: &'a [u8],
    bit: usize,
}

impl<'a> BitReader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, bit: 0 }
    }

    fn read_unsigned(&mut self, count: usize) -> Result<u32, String> {
        if count > 32 {
            return Err("bit field is wider than 32 bits".to_string());
        }
        let mut value = 0u32;
        for _ in 0..count {
            let byte_index = self.bit / 8;
            let bit_index = 7 - (self.bit % 8);
            let byte = *self
                .bytes
                .get(byte_index)
                .ok_or_else(|| "bit field is truncated".to_string())?;
            value = (value << 1) | u32::from((byte >> bit_index) & 1);
            self.bit += 1;
        }
        Ok(value)
    }

    fn read_signed(&mut self, count: usize) -> Result<i32, String> {
        let raw = self.read_unsigned(count)?;
        let shift = 32usize.saturating_sub(count);
        Ok(((raw << shift) as i32) >> shift)
    }

    fn bytes_consumed(&self) -> usize {
        self.bit.div_ceil(8)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::{write::ZlibEncoder, Compression};
    use std::io::Write;

    fn minimal_fws() -> Vec<u8> {
        // RECT: nbits=1, all four bounds zero => 9 bits => two bytes.
        let mut bytes = b"FWS\x09\x00\x00\x00\x00".to_vec();
        bytes.extend_from_slice(&[0x08, 0x00]);
        bytes.extend_from_slice(&(24u16 << 8).to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&0u16.to_le_bytes());
        let len = u32::try_from(bytes.len()).unwrap();
        bytes[4..8].copy_from_slice(&len.to_le_bytes());
        bytes
    }

    #[test]
    fn parses_minimal_uncompressed_swf() {
        let movie = parse_swf(&minimal_fws()).expect("parse fws");
        assert_eq!(movie.version, 9);
        assert_eq!(movie.frame_rate, 24.0);
        assert_eq!(movie.frame_count, 1);
        assert_eq!(movie.tags.len(), 1);
        assert_eq!(movie.tags[0].code, 0);
    }

    #[test]
    fn parses_zlib_swf_to_same_normalized_bytes() {
        let fws = minimal_fws();
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&fws[8..]).unwrap();
        let compressed = encoder.finish().unwrap();
        let mut cws = b"CWS".to_vec();
        cws.push(fws[3]);
        cws.extend_from_slice(&fws[4..8]);
        cws.extend_from_slice(&compressed);

        let movie = parse_swf(&cws).expect("parse cws");
        assert_eq!(movie.compression, SwfCompression::Zlib);
        assert_eq!(movie.normalized_fws, fws);
    }

    #[test]
    fn parses_lzma_swf_to_same_normalized_bytes() {
        let mut fws = minimal_fws();
        fws[3] = 13;
        let mut alone = Vec::new();
        lzma_rs::lzma_compress(&mut Cursor::new(&fws[8..]), &mut alone).expect("compress lzma");
        assert!(alone.len() >= 13);
        let compressed = &alone[13..];

        let mut zws = b"ZWS".to_vec();
        zws.push(fws[3]);
        zws.extend_from_slice(&fws[4..8]);
        zws.extend_from_slice(&u32::try_from(compressed.len()).unwrap().to_le_bytes());
        zws.extend_from_slice(&alone[0..5]);
        zws.extend_from_slice(compressed);

        let movie = parse_swf(&zws).expect("parse zws");
        assert_eq!(movie.compression, SwfCompression::Lzma);
        assert_eq!(movie.normalized_fws, fws);
    }

    #[test]
    fn finds_embedded_projector_swf_after_real_pe_signature() {
        let fws = minimal_fws();
        let mut exe = vec![0u8; 0x84];
        exe[0..2].copy_from_slice(b"MZ");
        exe[0x3c..0x40].copy_from_slice(&0x80u32.to_le_bytes());
        exe[0x80..0x84].copy_from_slice(b"PE\0\0");
        let swf_offset = exe.len();
        exe.extend_from_slice(&fws);
        exe.extend_from_slice(b"trailer");
        let embedded = find_projector_swf(&exe).expect("embedded swf");
        assert_eq!(embedded.offset, swf_offset);
        assert_eq!(embedded.movie.normalized_fws, fws);
    }

    #[test]
    fn mz_prefix_without_pe_header_is_not_a_projector() {
        let mut fake = b"MZnot-a-pe".to_vec();
        fake.extend_from_slice(&minimal_fws());
        let error = find_projector_swf(&fake).expect_err("reject fake executable");
        assert!(error.contains("not a Windows PE"));
    }
}
