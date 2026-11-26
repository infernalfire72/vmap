//! Per-area texture swap animations (flowers, sea foam, ...). Each area file
//! holds several animations; every one brings its own texture set and swaps
//! the texture of all map materials that use one of its textures.

use crate::error::{invalid, FormatError, Result};
use crate::nitro::Tex0;
use crate::reader::Reader;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AreaPatternKey {
    /// Frame from which this texture is shown.
    pub frame: u16,
    /// Index into the animation's own textures.
    pub texture: u8,
    /// Index into the animation's own palettes.
    pub palette: u8,
}

#[derive(Debug, Clone)]
pub struct AreaPattern {
    pub frame_count: u32,
    /// Key frames per animated texture.
    pub groups: Vec<Vec<AreaPatternKey>>,
    pub textures: Tex0,
}

/// Reads an area's pattern animation file.
pub fn parse_area_patterns(data: &[u8]) -> Result<Vec<AreaPattern>> {
    let mut r = Reader::new(data);
    let count = r.u32()? as usize;
    if count > 256 {
        return invalid(format!("{count} pattern animations is not plausible"));
    }
    let offsets: Vec<usize> = (0..count * 2 + 1).map(|_| r.u32().map(|o| o as usize)).collect::<Result<_>>()?;
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let (keys_at, tex_at, end) = (offsets[i * 2], offsets[i * 2 + 1], offsets[i * 2 + 2]);
        let keys = data.get(keys_at..tex_at).ok_or_else(|| FormatError::Invalid("pattern animation lies outside the file".into()))?;
        let tex = data.get(tex_at..end.max(tex_at).min(data.len())).ok_or_else(|| FormatError::Invalid("textures lie outside the file".into()))?;
        let (frame_count, groups) = parse_keys(keys)?;
        out.push(AreaPattern { frame_count, groups, textures: Tex0::from_btx(tex)? });
    }
    Ok(out)
}

fn parse_keys(data: &[u8]) -> Result<(u32, Vec<Vec<AreaPatternKey>>)> {
    let mut r = Reader::new(data);
    let align = |r: &mut Reader| r.seek((r.pos() + 3) & !3);
    let count = r.u32()? as usize;
    let frames: Vec<u16> = (0..count).map(|_| r.u16()).collect::<Result<_>>()?;
    align(&mut r);
    let textures = r.bytes(count)?.to_vec();
    align(&mut r);
    let palettes = r.bytes(count)?.to_vec();
    align(&mut r);
    let target_count = r.u32()? as usize;
    let mut starts: Vec<usize> = r.bytes(target_count)?.iter().map(|&b| b as usize).collect();
    starts.push(count);
    align(&mut r);
    let frame_count = r.u32()?;

    let keys: Vec<AreaPatternKey> =
        (0..count).map(|i| AreaPatternKey { frame: frames[i], texture: textures[i], palette: palettes[i] }).collect();
    let groups = starts
        .windows(2)
        .map(|w| keys.get(w[0]..w[1].max(w[0]).min(count)).map(<[_]>::to_vec).unwrap_or_default())
        .collect();
    Ok((frame_count, groups))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_frames_split_into_groups() {
        let mut d = Vec::new();
        d.extend_from_slice(&3u32.to_le_bytes());
        for f in [0u16, 8, 0] {
            d.extend_from_slice(&f.to_le_bytes());
        }
        d.extend_from_slice(&[0, 0]); // pad to 4
        d.extend_from_slice(&[0, 1, 2, 0]); // textures + pad
        d.extend_from_slice(&[5, 6, 7, 0]); // palettes + pad
        d.extend_from_slice(&2u32.to_le_bytes());
        d.extend_from_slice(&[0, 2, 0, 0]); // group starts + pad
        d.extend_from_slice(&16u32.to_le_bytes());
        let (frames, groups) = parse_keys(&d).unwrap();
        assert_eq!(frames, 16);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0], vec![
            AreaPatternKey { frame: 0, texture: 0, palette: 5 },
            AreaPatternKey { frame: 8, texture: 1, palette: 6 },
        ]);
        assert_eq!(groups[1], vec![AreaPatternKey { frame: 0, texture: 2, palette: 7 }]);
    }
}
