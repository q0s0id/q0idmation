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

pub(crate) fn write_string_u16(out: &mut Vec<u8>, text: &str) -> Result<(), Error> {
    let bytes = text.as_bytes();
    let len =
        u16::try_from(bytes.len()).map_err(|_| Error::Overflow("string length exceeds u16"))?;
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(bytes);
    Ok(())
}
