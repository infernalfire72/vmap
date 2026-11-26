//! Nintendo DS ROM image (`.nds`): header, file allocation table, file name
//! table and the ARM9 overlay table.

use std::collections::HashMap;
use std::ops::Range;

use crate::error::{invalid, Result};
use crate::reader::Reader;

#[derive(Debug, Clone)]
pub struct RomHeader {
    pub title: String,
    /// Four characters, e.g. `IRBO` for Pokemon Black (USA/Europe).
    pub game_code: String,
    pub maker_code: String,
    pub arm9_offset: u32,
    pub arm9_entry: u32,
    pub arm9_ram_addr: u32,
    pub arm9_size: u32,
    pub arm7_offset: u32,
    pub arm7_size: u32,
    pub fnt_offset: u32,
    pub fnt_size: u32,
    pub fat_offset: u32,
    pub fat_size: u32,
    pub arm9_overlay_offset: u32,
    pub arm9_overlay_size: u32,
}

impl RomHeader {
    pub const SIZE: usize = 0x200;

    pub fn parse(data: &[u8]) -> Result<Self> {
        let mut r = Reader::new(data);
        let title = r.str_padded(12)?;
        let game_code = r.str_padded(4)?;
        let maker_code = r.str_padded(2)?;
        r.seek(0x20);
        let arm9_offset = r.u32()?;
        let arm9_entry = r.u32()?;
        let arm9_ram_addr = r.u32()?;
        let arm9_size = r.u32()?;
        let arm7_offset = r.u32()?;
        let _arm7_entry = r.u32()?;
        let _arm7_ram_addr = r.u32()?;
        let arm7_size = r.u32()?;
        let fnt_offset = r.u32()?;
        let fnt_size = r.u32()?;
        let fat_offset = r.u32()?;
        let fat_size = r.u32()?;
        let arm9_overlay_offset = r.u32()?;
        let arm9_overlay_size = r.u32()?;
        Ok(Self {
            title,
            game_code,
            maker_code,
            arm9_offset,
            arm9_entry,
            arm9_ram_addr,
            arm9_size,
            arm7_offset,
            arm7_size,
            fnt_offset,
            fnt_size,
            fat_offset,
            fat_size,
            arm9_overlay_offset,
            arm9_overlay_size,
        })
    }
}

/// One entry of the ARM9 overlay table (`y9.bin`).
#[derive(Debug, Clone, Copy)]
pub struct OverlayEntry {
    pub id: u32,
    pub ram_addr: u32,
    pub ram_size: u32,
    pub bss_size: u32,
    pub file_id: u32,
    pub compressed: bool,
}

impl OverlayEntry {
    pub const SIZE: usize = 32;

    pub fn parse_table(data: &[u8]) -> Result<Vec<Self>> {
        let mut out = Vec::with_capacity(data.len() / Self::SIZE);
        let mut r = Reader::new(data);
        while r.remaining() >= Self::SIZE {
            let id = r.u32()?;
            let ram_addr = r.u32()?;
            let ram_size = r.u32()?;
            let bss_size = r.u32()?;
            let _init_start = r.u32()?;
            let _init_end = r.u32()?;
            let file_id = r.u32()?;
            let flags = r.u32()?;
            out.push(Self {
                id,
                ram_addr,
                ram_size,
                bss_size,
                file_id,
                compressed: (flags >> 24) & 1 == 1,
            });
        }
        Ok(out)
    }
}

/// Index of everything inside a ROM image. Holds byte ranges only, so the
/// caller decides how the image itself is kept in memory (for example mapped).
#[derive(Debug, Clone)]
pub struct RomIndex {
    pub header: RomHeader,
    /// Lower-case path (`a/0/0/8`) to byte range in the image.
    pub files: HashMap<String, Range<usize>>,
    /// Byte ranges by file ID (overlays are addressed this way).
    pub file_ranges: Vec<Range<usize>>,
    pub overlays: Vec<OverlayEntry>,
}

impl RomIndex {
    pub fn parse(rom: &[u8]) -> Result<Self> {
        let header = RomHeader::parse(rom)?;
        let fat = slice(rom, header.fat_offset, header.fat_size, "file allocation table")?;
        let fnt = slice(rom, header.fnt_offset, header.fnt_size, "file name table")?;

        let mut file_ranges = Vec::with_capacity(fat.len() / 8);
        let mut r = Reader::new(fat);
        while r.remaining() >= 8 {
            let start = r.u32()? as usize;
            let end = r.u32()? as usize;
            if end < start || end > rom.len() {
                file_ranges.push(0..0);
            } else {
                file_ranges.push(start..end);
            }
        }

        let mut files = HashMap::new();
        walk_fnt(fnt, 0xF000, String::new(), &file_ranges, &mut files, 0)?;

        let overlays = if header.arm9_overlay_size > 0 {
            OverlayEntry::parse_table(slice(
                rom,
                header.arm9_overlay_offset,
                header.arm9_overlay_size,
                "overlay table",
            )?)?
        } else {
            Vec::new()
        };

        Ok(Self { header, files, file_ranges, overlays })
    }
}

fn slice<'a>(rom: &'a [u8], offset: u32, size: u32, what: &str) -> Result<&'a [u8]> {
    let (o, s) = (offset as usize, size as usize);
    match o.checked_add(s) {
        Some(end) if end <= rom.len() => Ok(&rom[o..end]),
        _ => invalid(format!("{what} lies outside the ROM image")),
    }
}

fn walk_fnt(
    fnt: &[u8],
    dir_id: u16,
    prefix: String,
    ranges: &[Range<usize>],
    out: &mut HashMap<String, Range<usize>>,
    depth: usize,
) -> Result<()> {
    if depth > 32 {
        return invalid("file name table is nested too deeply");
    }
    let mut main = Reader::at(fnt, (dir_id as usize & 0xFFF) * 8);
    let sub_offset = main.u32()? as usize;
    let mut file_id = main.u16()? as usize;

    let mut r = Reader::at(fnt, sub_offset);
    loop {
        let kind = r.u8()?;
        if kind == 0 {
            break;
        }
        let name_len = (kind & 0x7F) as usize;
        let name = String::from_utf8_lossy(r.bytes(name_len)?).into_owned();
        let path = if prefix.is_empty() { name } else { format!("{prefix}/{name}") };
        if kind & 0x80 != 0 {
            let child = r.u16()?;
            walk_fnt(fnt, child, path, ranges, out, depth + 1)?;
        } else {
            if let Some(range) = ranges.get(file_id) {
                out.insert(path.to_ascii_lowercase(), range.clone());
            }
            file_id += 1;
        }
    }
    Ok(())
}
