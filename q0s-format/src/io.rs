use crate::error::Error;

pub(crate) struct Cursor<'a> {
    data: &'a [u8],
    offset: usize,
}

impl<'a> Cursor<'a> {
    pub(crate) fn new(data: &'a [u8]) -> Self {
        Self { data, offset: 0 }
    }

    pub(crate) fn read_exact(&mut self, count: usize) -> Result<&'a [u8], Error> {
        let end = self.offset.saturating_add(count);
        if end > self.data.len() {
            return Err(Error::UnexpectedEof {
                offset: self.offset,
                needed: count,
                available: self.data.len().saturating_sub(self.offset),
            });
        }
        let out = &self.data[self.offset..end];
        self.offset = end;
        Ok(out)
    }

    pub(crate) fn read_u8(&mut self) -> Result<u8, Error> {
        Ok(self.read_exact(1)?[0])
    }

    pub(crate) fn read_u16(&mut self) -> Result<u16, Error> {
        let b = self.read_exact(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    pub(crate) fn read_u32(&mut self) -> Result<u32, Error> {
        let b = self.read_exact(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub(crate) fn read_i16(&mut self) -> Result<i16, Error> {
        let b = self.read_exact(2)?;
        Ok(i16::from_le_bytes([b[0], b[1]]))
    }

    pub(crate) fn read_f32(&mut self) -> Result<f32, Error> {
        let b = self.read_exact(4)?;
        Ok(f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub(crate) fn read_string_u16(&mut self) -> Result<String, Error> {
        let len = usize::from(self.read_u16()?);
        let bytes = self.read_exact(len)?;
        let text = std::str::from_utf8(bytes).map_err(|_| Error::InvalidUtf8)?;
        Ok(text.to_owned())
    }

    pub(crate) fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.offset)
    }

    /// A LEB128 unsigned number of up to 32 bits.
    pub(crate) fn read_varint(&mut self) -> Result<u32, Error> {
        let mut value = 0_u32;
        for shift in (0..35).step_by(7) {
            let byte = self.read_u8()?;
            let bits = u32::from(byte & 0x7f);
            if shift == 28 && bits > 0x0f {
                return Err(Error::Validation("varint exceeds 32 bits"));
            }
            value |= bits << shift;
            if byte & 0x80 == 0 {
                return Ok(value);
            }
        }
        Err(Error::Validation("varint exceeds 32 bits"))
    }

    /// A zigzag-coded signed number, small either side of zero.
    pub(crate) fn read_zigzag(&mut self) -> Result<i32, Error> {
        let value = self.read_varint()?;
        Ok((value >> 1) as i32 ^ -((value & 1) as i32))
    }

    pub(crate) fn finish(self) -> Result<(), Error> {
        let remaining = self.remaining();
        if remaining == 0 {
            Ok(())
        } else {
            Err(Error::TrailingBytes {
                offset: self.offset,
                remaining,
            })
        }
    }
}

pub(crate) fn write_varint(out: &mut Vec<u8>, mut value: u32) {
    while value >= 0x80 {
        out.push((value as u8 & 0x7f) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

pub(crate) fn write_zigzag(out: &mut Vec<u8>, value: i32) {
    write_varint(out, ((value << 1) ^ (value >> 31)) as u32);
}

pub(crate) fn write_string_u16(out: &mut Vec<u8>, text: &str) -> Result<(), Error> {
    let bytes = text.as_bytes();
    let len =
        u16::try_from(bytes.len()).map_err(|_| Error::Overflow("string length exceeds u16"))?;
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(bytes);
    Ok(())
}
