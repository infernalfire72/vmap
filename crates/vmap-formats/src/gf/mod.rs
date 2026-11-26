//! Game Freak's own small containers.

use std::ops::Range;

use crate::error::{invalid, Result};
use crate::reader::Reader;

/// A "GF container": two magic letters, a file count and an offset table.
/// Map chunks (`WB`, ...) and building bundles (`AB`) use this layout.
#[derive(Debug, Clone)]
pub struct GfContainer {
    pub magic: String,
    pub files: Vec<Range<usize>>,
}

impl GfContainer {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let mut r = Reader::new(data);
        let magic = r.str_padded(2)?;
        let count = r.u16()? as usize;
        let mut offsets = Vec::with_capacity(count + 1);
        for _ in 0..=count {
            offsets.push(r.u32()? as usize);
        }
        let mut files = Vec::with_capacity(count);
        for w in offsets.windows(2) {
            if w[1] < w[0] || w[1] > data.len() {
                return invalid(format!("container '{magic}' has an offset outside the file"));
            }
            files.push(w[0]..w[1]);
        }
        Ok(Self { magic, files })
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

    pub fn last<'a>(&self, data: &'a [u8]) -> Option<&'a [u8]> {
        self.files.last().map(|r| &data[r.clone()])
    }
}
