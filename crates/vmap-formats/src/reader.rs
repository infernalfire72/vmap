use crate::error::{FormatError, Result};

/// Bounds-checked little-endian cursor over a byte slice.
#[derive(Clone)]
pub struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    pub fn at(data: &'a [u8], pos: usize) -> Self {
        Self { data, pos }
    }

    pub fn pos(&self) -> usize {
        self.pos
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    pub fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    pub fn seek(&mut self, pos: usize) {
        self.pos = pos;
    }

    pub fn skip(&mut self, n: usize) {
        self.pos += n;
    }

    pub fn data(&self) -> &'a [u8] {
        self.data
    }

    pub fn bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.pos.checked_add(n).filter(|&e| e <= self.data.len());
        match end {
            Some(end) => {
                let s = &self.data[self.pos..end];
                self.pos = end;
                Ok(s)
            }
            None => Err(FormatError::Eof { offset: self.pos, needed: n }),
        }
    }

    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.bytes(1)?[0])
    }

    pub fn i8(&mut self) -> Result<i8> {
        Ok(self.u8()? as i8)
    }

    pub fn u16(&mut self) -> Result<u16> {
        let b = self.bytes(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    pub fn i16(&mut self) -> Result<i16> {
        Ok(self.u16()? as i16)
    }

    pub fn u32(&mut self) -> Result<u32> {
        let b = self.bytes(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn i32(&mut self) -> Result<i32> {
        Ok(self.u32()? as i32)
    }

    /// Signed 16-bit fixed point with 12 fractional bits.
    pub fn fx16(&mut self) -> Result<f32> {
        Ok(self.i16()? as f32 / 4096.0)
    }

    /// Signed 32-bit fixed point with 12 fractional bits.
    pub fn fx32(&mut self) -> Result<f32> {
        Ok(self.i32()? as f32 / 4096.0)
    }

    pub fn vec3_fx32(&mut self) -> Result<glam::Vec3> {
        Ok(glam::Vec3::new(self.fx32()?, self.fx32()?, self.fx32()?))
    }

    /// Fixed-width string, trimmed at the first NUL.
    pub fn str_padded(&mut self, n: usize) -> Result<String> {
        let b = self.bytes(n)?;
        let end = b.iter().position(|&c| c == 0).unwrap_or(n);
        Ok(String::from_utf8_lossy(&b[..end]).into_owned())
    }

    pub fn expect_magic(&mut self, magic: &[u8]) -> Result<()> {
        let found = self.bytes(magic.len())?;
        if found == magic {
            Ok(())
        } else {
            Err(FormatError::BadMagic {
                expected: String::from_utf8_lossy(magic).into_owned(),
                found: String::from_utf8_lossy(found).into_owned(),
            })
        }
    }
}

pub fn u16_at(data: &[u8], pos: usize) -> Result<u16> {
    Reader::at(data, pos).u16()
}

pub fn u32_at(data: &[u8], pos: usize) -> Result<u32> {
    Reader::at(data, pos).u32()
}
