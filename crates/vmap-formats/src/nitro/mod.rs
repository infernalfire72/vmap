//! Nitro System 3D resources: models (`BMD0`), textures (`BTX0`) and the
//! animation files that go with them.

pub mod anim;
pub mod eval;
pub mod gx;
pub mod mdl;
pub mod tex;

use crate::error::Result;
use crate::reader::Reader;

pub use eval::{evaluate, GeoBatch, GeoVertex, Geometry};
pub use mdl::{Material, Mdl0, Model, Node, Shape};
pub use tex::{Image, Tex0, TexFormat};

/// A Nitro resource file split into its blocks.
#[derive(Debug, Clone)]
pub struct NitroFile<'a> {
    /// File magic, e.g. `BMD0`.
    pub magic: [u8; 4],
    /// Each block as (magic, bytes starting at the block magic).
    pub blocks: Vec<([u8; 4], &'a [u8])>,
}

impl<'a> NitroFile<'a> {
    pub fn parse(data: &'a [u8]) -> Result<Self> {
        let mut r = Reader::new(data);
        let m = r.bytes(4)?;
        let magic = [m[0], m[1], m[2], m[3]];
        let _bom = r.u16()?;
        let _version = r.u16()?;
        let _file_size = r.u32()?;
        let _header_size = r.u16()?;
        let count = r.u16()? as usize;
        let mut offsets = Vec::with_capacity(count);
        for _ in 0..count {
            offsets.push(r.u32()? as usize);
        }
        let mut blocks = Vec::with_capacity(count);
        for (i, &off) in offsets.iter().enumerate() {
            let end = offsets.get(i + 1).copied().filter(|&e| e > off).unwrap_or(data.len());
            if off + 8 > data.len() || end > data.len() {
                continue;
            }
            let b = &data[off..end];
            blocks.push(([b[0], b[1], b[2], b[3]], b));
        }
        Ok(Self { magic, blocks })
    }

    pub fn block(&self, magic: &[u8; 4]) -> Option<&'a [u8]> {
        self.blocks.iter().find(|(m, _)| m == magic).map(|(_, b)| *b)
    }

    /// Four-letter magic of a file, if it looks like a Nitro resource.
    pub fn sniff(data: &[u8]) -> Option<[u8; 4]> {
        if data.len() < 16 || data[4] != 0xFF || data[5] != 0xFE {
            return None;
        }
        let m = [data[0], data[1], data[2], data[3]];
        m.iter().all(|c| c.is_ascii_alphanumeric()).then_some(m)
    }
}

/// One named entry of a Nitro dictionary together with its fixed-size record.
#[derive(Debug, Clone)]
pub struct DictEntry<'a> {
    pub name: String,
    pub data: &'a [u8],
}

impl DictEntry<'_> {
    pub fn u32(&self, word: usize) -> u32 {
        let o = word * 4;
        match self.data.get(o..o + 4) {
            Some(b) => u32::from_le_bytes([b[0], b[1], b[2], b[3]]),
            None => 0,
        }
    }
}

/// Reads the name dictionary that starts at `pos`.
pub fn read_dict(data: &[u8], pos: usize) -> Result<Vec<DictEntry<'_>>> {
    let mut r = Reader::at(data, pos);
    let _revision = r.u8()?;
    let count = r.u8()? as usize;
    let _size = r.u16()?;
    let _nodes_offset = r.u16()?;
    let entries_offset = r.u16()? as usize;

    let entries_start = pos + entries_offset;
    r.seek(entries_start);
    let entry_size = r.u16()? as usize;
    let names_offset = r.u16()? as usize;

    let mut records = Vec::with_capacity(count);
    for _ in 0..count {
        records.push(r.bytes(entry_size)?);
    }
    r.seek(entries_start + names_offset);
    let mut out = Vec::with_capacity(count);
    for data in records {
        out.push(DictEntry { name: r.str_padded(16)?, data });
    }
    Ok(out)
}
