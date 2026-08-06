use std::collections::HashMap;

pub const MAGIC: [u8; 4] = *b"Q0S\0";
pub const SUPPORTED_VERSION: u16 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    pub version: u16,
    pub fps: u16,
    pub frame_count: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bitmap {
    pub id: u16,
    pub width: u16,
    pub height: u16,
    pub rgba: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Placement {
    pub frame: u16,
    pub bitmap_id: u16,
    pub x: i16,
    pub y: i16,
    pub scale_x: f32,
    pub scale_y: f32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Background {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Movie {
    pub header: Header,
    pub background: Background,
    pub bitmaps: HashMap<u16, Bitmap>,
    pub placements_by_frame: Vec<Vec<Placement>>,
}
