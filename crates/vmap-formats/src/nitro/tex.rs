//! `TEX0` texture blocks and decoding of the seven DS texture formats.

use crate::error::Result;
use crate::nitro::{read_dict, NitroFile};
use crate::reader::Reader;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TexFormat {
    None,
    /// 3-bit alpha, 5-bit palette index.
    A3I5,
    Pal4,
    Pal16,
    Pal256,
    /// 4x4 block compression.
    Cmpr,
    /// 5-bit alpha, 3-bit palette index.
    A5I3,
    /// 15-bit colour plus one alpha bit.
    Direct,
}

impl TexFormat {
    pub fn from_bits(v: u32) -> Self {
        match v & 7 {
            1 => Self::A3I5,
            2 => Self::Pal4,
            3 => Self::Pal16,
            4 => Self::Pal256,
            5 => Self::Cmpr,
            6 => Self::A5I3,
            7 => Self::Direct,
            _ => Self::None,
        }
    }

    pub fn bits_per_pixel(self) -> usize {
        match self {
            Self::None => 0,
            Self::A3I5 | Self::A5I3 | Self::Pal256 => 8,
            Self::Pal4 | Self::Cmpr => 2,
            Self::Pal16 => 4,
            Self::Direct => 16,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::A3I5 => "A3I5",
            Self::Pal4 => "4-colour",
            Self::Pal16 => "16-colour",
            Self::Pal256 => "256-colour",
            Self::Cmpr => "4x4 compressed",
            Self::A5I3 => "A5I3",
            Self::Direct => "direct colour",
        }
    }

    /// Formats whose texels carry more than one bit of alpha.
    pub fn is_translucent(self) -> bool {
        matches!(self, Self::A3I5 | Self::A5I3)
    }
}

/// Decoded image, 8-bit RGBA, rows top to bottom.
#[derive(Debug, Clone)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Image {
    pub fn new(width: u32, height: u32) -> Self {
        Self { width, height, rgba: vec![0; (width * height * 4) as usize] }
    }

    /// True if any texel is neither fully opaque nor fully transparent.
    pub fn has_partial_alpha(&self) -> bool {
        self.rgba.chunks_exact(4).any(|p| p[3] != 0 && p[3] != 255)
    }

    pub fn has_transparency(&self) -> bool {
        self.rgba.chunks_exact(4).any(|p| p[3] != 255)
    }
}

#[derive(Debug, Clone)]
pub struct Texture {
    pub name: String,
    pub format: TexFormat,
    pub width: u32,
    pub height: u32,
    /// Palette index 0 is transparent (paletted formats only).
    pub color0_transparent: bool,
    /// Raw `TEXIMAGE_PARAM` word.
    pub params: u32,
    pub data: Vec<u8>,
    /// Per-block palette info, 4x4 compressed textures only.
    pub cmpr_info: Vec<u16>,
}

#[derive(Debug, Clone)]
pub struct Palette {
    pub name: String,
    /// Index of the first colour inside the palette block.
    pub first_color: usize,
    /// Number of colours up to the next palette (or the end of the block).
    pub color_count: usize,
}

#[derive(Debug, Clone, Default)]
pub struct Tex0 {
    pub textures: Vec<Texture>,
    pub palettes: Vec<Palette>,
    /// All palette colours of the block, BGR555.
    pub colors: Vec<u16>,
}

impl Tex0 {
    /// Parses a standalone `BTX0` file.
    pub fn from_btx(data: &[u8]) -> Result<Self> {
        let file = NitroFile::parse(data)?;
        match file.block(b"TEX0") {
            Some(b) => Self::parse(b),
            None => crate::error::invalid("file has no TEX0 block"),
        }
    }

    /// Parses a `TEX0` block; `blk` starts at the block magic.
    pub fn parse(blk: &[u8]) -> Result<Self> {
        let mut r = Reader::new(blk);
        r.expect_magic(b"TEX0")?;
        let _size = r.u32()?;

        let _rt = r.u32()?;
        let _tex_size = (r.u16()? as usize) << 3;
        let tex_dict = r.u16()? as usize;
        let _flags = r.u16()?;
        r.skip(2);
        let tex_data = r.u32()? as usize;

        let _rt = r.u32()?;
        let _cmpr_size = (r.u16()? as usize) << 3;
        let _cmpr_dict = r.u16()?;
        let _flags = r.u16()?;
        r.skip(2);
        let cmpr_data = r.u32()? as usize;
        let cmpr_info = r.u32()? as usize;

        let _rt = r.u32()?;
        let pal_size = (r.u16()? as usize) << 3;
        let _flags = r.u16()?;
        let pal_dict = r.u16()? as usize;
        r.skip(2);
        let pal_data = r.u32()? as usize;

        // Palette colours are read as one array: compressed textures index
        // past the end of "their" palette.
        let pal_end = (pal_data + pal_size).min(blk.len());
        let mut colors = Vec::with_capacity(pal_size / 2);
        let mut pr = Reader::at(blk, pal_data);
        while pr.pos() + 2 <= pal_end {
            colors.push(pr.u16()?);
        }

        let mut textures = Vec::new();
        for e in read_dict(blk, tex_dict)? {
            let params = e.u32(0);
            let format = TexFormat::from_bits(params >> 26);
            let width = 8u32 << ((params >> 20) & 7);
            let height = 8u32 << ((params >> 23) & 7);
            let offset = ((params & 0xFFFF) as usize) << 3;
            let size = (width * height) as usize * format.bits_per_pixel() / 8;
            let base = if format == TexFormat::Cmpr { cmpr_data } else { tex_data };
            let start = (base + offset).min(blk.len());
            let end = (start + size).min(blk.len());
            let data = blk[start..end].to_vec();

            let mut info = Vec::new();
            if format == TexFormat::Cmpr {
                let blocks = (width * height / 16) as usize;
                let mut ir = Reader::at(blk, cmpr_info + offset / 2);
                for _ in 0..blocks {
                    match ir.u16() {
                        Ok(v) => info.push(v),
                        Err(_) => break,
                    }
                }
            }
            textures.push(Texture {
                name: e.name,
                format,
                width,
                height,
                color0_transparent: (params >> 29) & 1 == 1,
                params,
                data,
                cmpr_info: info,
            });
        }

        let pal_entries = read_dict(blk, pal_dict)?;
        let starts: Vec<usize> = pal_entries.iter().map(|e| ((e.u32(0) & 0xFFFF) as usize) << 3).collect();
        let mut palettes = Vec::with_capacity(pal_entries.len());
        for (e, &start) in pal_entries.iter().zip(&starts) {
            let next = starts.iter().copied().filter(|&o| o > start).min().unwrap_or(pal_size);
            palettes.push(Palette {
                name: e.name.clone(),
                first_color: start / 2,
                color_count: next.saturating_sub(start) / 2,
            });
        }

        Ok(Self { textures, palettes, colors })
    }

    pub fn find_texture(&self, name: &str) -> Option<usize> {
        self.textures.iter().position(|t| t.name == name)
    }

    pub fn find_palette(&self, name: &str) -> Option<usize> {
        self.palettes.iter().position(|p| p.name == name)
    }

    /// Picks the palette a texture most likely belongs to when no material
    /// names one: same name, the `_pl` suffix convention, or the only palette.
    pub fn guess_palette(&self, texture: usize) -> Option<usize> {
        let name = &self.textures.get(texture)?.name;
        self.find_palette(name)
            .or_else(|| self.find_palette(&format!("{name}_pl")))
            .or_else(|| {
                self.palettes
                    .iter()
                    .position(|p| p.name.strip_suffix("_pl").is_some_and(|base| name.starts_with(base)))
            })
            .or_else(|| (!self.palettes.is_empty()).then_some(0))
    }

    /// Decodes a texture with the given palette to RGBA.
    pub fn decode(&self, texture: usize, palette: Option<usize>) -> Option<Image> {
        let tex = self.textures.get(texture)?;
        let first = palette.and_then(|p| self.palettes.get(p)).map(|p| p.first_color).unwrap_or(0);
        let colors = self.colors.get(first..).unwrap_or(&[]);
        Some(decode_texture(tex, colors))
    }

    pub fn decode_by_name(&self, texture: &str, palette: Option<&str>) -> Option<Image> {
        let t = self.find_texture(texture)?;
        let p = match palette {
            Some(name) => self.find_palette(name).or_else(|| self.guess_palette(t)),
            None => self.guess_palette(t),
        };
        self.decode(t, p)
    }
}

/// Expands BGR555 to 8-bit RGB.
pub fn rgb555(c: u16) -> [u8; 3] {
    let e = |v: u16| ((v << 3) | (v >> 2)) as u8;
    [e(c & 31), e((c >> 5) & 31), e((c >> 10) & 31)]
}

/// BGR555 to linear-ish float RGB in `0..=1`.
pub fn rgb555_f32(c: u16) -> [f32; 3] {
    [(c & 31) as f32 / 31.0, ((c >> 5) & 31) as f32 / 31.0, ((c >> 10) & 31) as f32 / 31.0]
}

fn decode_texture(tex: &Texture, colors: &[u16]) -> Image {
    let (w, h) = (tex.width as usize, tex.height as usize);
    let mut img = Image::new(tex.width, tex.height);
    let pal = |i: usize| -> [u8; 3] { colors.get(i).map(|&c| rgb555(c)).unwrap_or([0, 0, 0]) };
    let mut put = |i: usize, rgb: [u8; 3], a: u8| {
        if let Some(px) = img.rgba.get_mut(i * 4..i * 4 + 4) {
            px.copy_from_slice(&[rgb[0], rgb[1], rgb[2], a]);
        }
    };

    match tex.format {
        TexFormat::None => {}
        TexFormat::A3I5 => {
            for (i, &b) in tex.data.iter().take(w * h).enumerate() {
                let a = (b >> 5) as u32;
                put(i, pal((b & 31) as usize), ((a << 5) | (a << 2) | (a >> 1)) as u8);
            }
        }
        TexFormat::A5I3 => {
            for (i, &b) in tex.data.iter().take(w * h).enumerate() {
                let a = (b >> 3) as u32;
                put(i, pal((b & 7) as usize), ((a << 3) | (a >> 2)) as u8);
            }
        }
        TexFormat::Pal4 | TexFormat::Pal16 | TexFormat::Pal256 => {
            let bpp = tex.format.bits_per_pixel();
            let mask = (1usize << bpp) - 1;
            for i in 0..w * h {
                let bit = i * bpp;
                let Some(&byte) = tex.data.get(bit / 8) else { break };
                let idx = (byte as usize >> (bit % 8)) & mask;
                let a = if idx == 0 && tex.color0_transparent { 0 } else { 255 };
                put(i, pal(idx), a);
            }
        }
        TexFormat::Direct => {
            for i in 0..w * h {
                let Some(b) = tex.data.get(i * 2..i * 2 + 2) else { break };
                let c = u16::from_le_bytes([b[0], b[1]]);
                put(i, rgb555(c), if c & 0x8000 != 0 { 255 } else { 0 });
            }
        }
        TexFormat::Cmpr => {
            let blocks_x = w / 4;
            for (bi, &info) in tex.cmpr_info.iter().enumerate() {
                let Some(b) = tex.data.get(bi * 4..bi * 4 + 4) else { break };
                let bits = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
                let base = ((info & 0x3FFF) as usize) << 1;
                let c0 = pal(base);
                let c1 = pal(base + 1);
                let mix = |a: [u8; 3], b: [u8; 3], wa: u32, wb: u32| -> [u8; 3] {
                    let d = wa + wb;
                    [
                        ((a[0] as u32 * wa + b[0] as u32 * wb) / d) as u8,
                        ((a[1] as u32 * wa + b[1] as u32 * wb) / d) as u8,
                        ((a[2] as u32 * wa + b[2] as u32 * wb) / d) as u8,
                    ]
                };
                let table: [([u8; 3], u8); 4] = match info >> 14 {
                    0 => [(c0, 255), (c1, 255), (pal(base + 2), 255), ([0, 0, 0], 0)],
                    1 => [(c0, 255), (c1, 255), (mix(c0, c1, 1, 1), 255), ([0, 0, 0], 0)],
                    2 => [(c0, 255), (c1, 255), (pal(base + 2), 255), (pal(base + 3), 255)],
                    _ => [(c0, 255), (c1, 255), (mix(c0, c1, 5, 3), 255), (mix(c0, c1, 3, 5), 255)],
                };
                let (bx, by) = (bi % blocks_x.max(1), bi / blocks_x.max(1));
                for py in 0..4 {
                    for px in 0..4 {
                        let sel = (bits >> ((py * 4 + px) * 2)) & 3;
                        let (rgb, a) = table[sel as usize];
                        let (x, y) = (bx * 4 + px, by * 4 + py);
                        if x < w && y < h {
                            put(y * w + x, rgb, a);
                        }
                    }
                }
            }
        }
    }
    img
}
