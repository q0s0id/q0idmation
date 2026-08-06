use std::collections::HashMap;

use crate::error::Error;
use crate::io::Cursor;
use crate::model::{Background, Bitmap, Header, Movie, Placement, MAGIC, SUPPORTED_VERSION};

const TAG_SET_BACKGROUND: u8 = 1;
const TAG_DEFINE_BITMAP: u8 = 2;
const TAG_PLACE_BITMAP: u8 = 3;

pub fn parse_q0s(bytes: &[u8]) -> Result<Movie, Error> {
    let mut c = Cursor::new(bytes);

    let magic = c.read_exact(4)?;
    let mut magic_arr = [0_u8; 4];
    magic_arr.copy_from_slice(magic);
    if magic_arr != MAGIC {
        return Err(Error::InvalidMagic(magic_arr));
    }

    let version = c.read_u16()?;
    if version != SUPPORTED_VERSION {
        return Err(Error::UnsupportedVersion(version));
    }

    let fps = c.read_u16()?;
    let frame_count = c.read_u16()?;
    let record_count = c.read_u16()?;

    if frame_count == 0 {
        return Err(Error::Validation("frame_count must be > 0"));
    }
    if fps == 0 {
        return Err(Error::Validation("fps must be > 0"));
    }

    let header = Header {
        version,
        fps,
        frame_count,
    };

    let mut background = Background {
        r: 0,
        g: 0,
        b: 0,
        a: 255,
    };
    let mut bitmaps: HashMap<u16, Bitmap> = HashMap::new();
    let mut placements_by_frame = vec![Vec::<Placement>::new(); usize::from(frame_count)];

    for _ in 0..record_count {
        let tag = c.read_u8()?;
        match tag {
            TAG_SET_BACKGROUND => {
                let rgba = c.read_exact(4)?;
                background = Background {
                    r: rgba[0],
                    g: rgba[1],
                    b: rgba[2],
                    a: rgba[3],
                };
            }
            TAG_DEFINE_BITMAP => {
                let id = c.read_u16()?;
                let width = c.read_u16()?;
                let height = c.read_u16()?;
                let len = usize::try_from(c.read_u32()?).map_err(|_| Error::InvalidRecord {
                    tag,
                    reason: "bitmap payload length does not fit usize",
                })?;

                let expected = usize::from(width)
                    .saturating_mul(usize::from(height))
                    .saturating_mul(4);
                if len != expected {
                    return Err(Error::InvalidRecord {
                        tag,
                        reason: "bitmap payload length mismatches dimensions",
                    });
                }
                let rgba = c.read_exact(len)?.to_vec();

                bitmaps.insert(
                    id,
                    Bitmap {
                        id,
                        width,
                        height,
                        rgba,
                    },
                );
            }
            TAG_PLACE_BITMAP => {
                let frame = c.read_u16()?;
                let bitmap_id = c.read_u16()?;
                let x = c.read_i16()?;
                let y = c.read_i16()?;
                let scale_x = c.read_f32()?;
                let scale_y = c.read_f32()?;

                if !scale_x.is_finite() || !scale_y.is_finite() {
                    return Err(Error::InvalidRecord {
                        tag,
                        reason: "scale must be finite",
                    });
                }
                if scale_x <= 0.0 || scale_y <= 0.0 {
                    return Err(Error::InvalidRecord {
                        tag,
                        reason: "scale must be positive",
                    });
                }
                if usize::from(frame) >= placements_by_frame.len() {
                    return Err(Error::InvalidRecord {
                        tag,
                        reason: "placement frame is out of bounds",
                    });
                }

                placements_by_frame[usize::from(frame)].push(Placement {
                    frame,
                    bitmap_id,
                    x,
                    y,
                    scale_x,
                    scale_y,
                });
            }
            _ => return Err(Error::InvalidTag(tag)),
        }
    }

    c.finish()?;

    Ok(Movie {
        header,
        background,
        bitmaps,
        placements_by_frame,
    })
}
