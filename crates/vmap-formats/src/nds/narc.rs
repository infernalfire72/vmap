//! NARC archives: numbered files packed into one blob. Almost all Gen V game
//! data lives in NARCs addressed as `a/X/Y/Z`.

use std::ops::Range;

use crate::error::{invalid, Result};
use crate::reader::Reader;

/// Parsed file table of a NARC. Borrow the archive bytes separately.
#[derive(Debug, Clone, Default)]
pub struct Narc {
    /// Byte range of each member inside the archive.
    pub files: Vec<Range<usize>>,
}

impl Narc {
    pub fn is_narc(data: &[u8]) -> bool {
        data.len() >= 16 && &data[0..4] == b"NARC"
    }

    pub fn parse(data: &[u8]) -> Result<Self> {
        let mut r = Reader::new(data);
        r.expect_magic(b"NARC")?;
        let _bom = r.u16()?;
        let _version = r.u16()?;
        let _file_size = r.u32()?;
        let header_size = r.u16()? as usize;
        let _block_count = r.u16()?;

        // File allocation block.
        r.seek(header_size);
        let fatb_start = r.pos();
        r.expect_magic(b"BTAF")?;
        let fatb_size = r.u32()? as usize;
        let count = r.u32()? as usize & 0xFFFF;
        let mut entries = Vec::with_capacity(count);
        for _ in 0..count {
            let start = r.u32()? as usize;
            let end = r.u32()? as usize;
            entries.push((start, end));
        }

        // File name block (ignored: Gen V archives are index-addressed).
        r.seek(fatb_start + fatb_size);
        let fntb_start = r.pos();
        r.expect_magic(b"BTNF")?;
        let fntb_size = r.u32()? as usize;

        // File image block.
        r.seek(fntb_start + fntb_size);
        r.expect_magic(b"GMIF")?;
        let _fimg_size = r.u32()?;
        let base = r.pos();

        let mut files = Vec::with_capacity(count);
        for (start, end) in entries {
            let (s, e) = (base + start, base + end);
            if e < s || e > data.len() {
                return invalid("NARC member lies outside the archive");
            }
            files.push(s..e);
        }
        Ok(Self { files })
    }

    pub fn len(&self) -> usize {
        self.files.len()
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    pub fn get<'a>(&self, data: &'a [u8], index: usize) -> Option<&'a [u8]> {
        self.files.get(index).map(|r| &data[r.clone()])
    }
}
