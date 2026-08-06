use crate::error::Error;
use crate::model::Movie;

pub fn write_q0s(movie: &Movie) -> Result<Vec<u8>, Error> {
    let placements_total: usize = movie.placements_by_frame.iter().map(Vec::len).sum();
    let record_count_usize = 1 + movie.bitmaps.len() + placements_total;
    let record_count = u16::try_from(record_count_usize)
        .map_err(|_| Error::Overflow("record_count exceeds u16"))?;

    let mut out = Vec::new();
    out.extend_from_slice(b"Q0S\0");
    out.extend_from_slice(&movie.header.version.to_le_bytes());
    out.extend_from_slice(&movie.header.fps.to_le_bytes());
    out.extend_from_slice(&movie.header.frame_count.to_le_bytes());
    out.extend_from_slice(&record_count.to_le_bytes());

    out.push(1);
    out.extend_from_slice(&[
        movie.background.r,
        movie.background.g,
        movie.background.b,
        movie.background.a,
    ]);

    let mut bitmap_ids: Vec<_> = movie.bitmaps.keys().copied().collect();
    bitmap_ids.sort_unstable();
    for bitmap_id in bitmap_ids {
        let bitmap = &movie.bitmaps[&bitmap_id];
        let payload_len = u32::try_from(bitmap.rgba.len())
            .map_err(|_| Error::Overflow("bitmap payload length exceeds u32"))?;

        out.push(2);
        out.extend_from_slice(&bitmap.id.to_le_bytes());
        out.extend_from_slice(&bitmap.width.to_le_bytes());
        out.extend_from_slice(&bitmap.height.to_le_bytes());
        out.extend_from_slice(&payload_len.to_le_bytes());
        out.extend_from_slice(&bitmap.rgba);
    }

    for (frame_idx, placements) in movie.placements_by_frame.iter().enumerate() {
        let frame =
            u16::try_from(frame_idx).map_err(|_| Error::Overflow("frame index exceeds u16"))?;
        for placement in placements {
            if !placement.scale_x.is_finite() || !placement.scale_y.is_finite() {
                return Err(Error::Validation("placement scale must be finite"));
            }
            if placement.scale_x <= 0.0 || placement.scale_y <= 0.0 {
                return Err(Error::Validation("placement scale must be positive"));
            }
            out.push(3);
            out.extend_from_slice(&frame.to_le_bytes());
            out.extend_from_slice(&placement.bitmap_id.to_le_bytes());
            out.extend_from_slice(&placement.x.to_le_bytes());
            out.extend_from_slice(&placement.y.to_le_bytes());
            out.extend_from_slice(&placement.scale_x.to_le_bytes());
            out.extend_from_slice(&placement.scale_y.to_le_bytes());
        }
    }

    Ok(out)
}
