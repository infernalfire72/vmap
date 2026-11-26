//! Decompression for the schemes found in DS games: forward LZ77 (types 0x10
//! and 0x11) for data files and backward LZ ("BLZ") for ARM9 code and overlays.

use crate::error::{invalid, Result};

/// Returns true if `data` starts with an LZ10 or LZ11 header whose declared
/// size is plausible.
pub fn looks_lz(data: &[u8]) -> bool {
    if data.len() < 5 {
        return false;
    }
    let size = u32::from_le_bytes([data[1], data[2], data[3], 0]) as usize;
    matches!(data[0], 0x10 | 0x11) && size > 0 && size >= data.len() / 2
}

/// Decompresses LZ10 or LZ11 data.
pub fn lz_decompress(data: &[u8]) -> Result<Vec<u8>> {
    if data.len() < 4 {
        return invalid("LZ data is too short");
    }
    let kind = data[0];
    let mut size = u32::from_le_bytes([data[1], data[2], data[3], 0]) as usize;
    let mut pos = 4;
    if size == 0 {
        if data.len() < 8 {
            return invalid("LZ data is too short");
        }
        size = u32::from_le_bytes([data[4], data[5], data[6], data[7]]) as usize;
        pos = 8;
    }
    let mut out = Vec::with_capacity(size);
    let next = |pos: &mut usize| -> Result<u8> {
        let b = *data.get(*pos).ok_or_else(|| crate::FormatError::Eof { offset: *pos, needed: 1 })?;
        *pos += 1;
        Ok(b)
    };

    while out.len() < size {
        let flags = next(&mut pos)?;
        for bit in (0..8).rev() {
            if out.len() >= size {
                break;
            }
            if flags >> bit & 1 == 0 {
                out.push(next(&mut pos)?);
                continue;
            }
            let (len, disp) = match kind {
                0x10 => {
                    let a = next(&mut pos)? as usize;
                    let b = next(&mut pos)? as usize;
                    ((a >> 4) + 3, ((a & 0xF) << 8 | b) + 1)
                }
                0x11 => {
                    let a = next(&mut pos)? as usize;
                    match a >> 4 {
                        0 => {
                            let b = next(&mut pos)? as usize;
                            let c = next(&mut pos)? as usize;
                            (((a & 0xF) << 4 | b >> 4) + 0x11, ((b & 0xF) << 8 | c) + 1)
                        }
                        1 => {
                            let b = next(&mut pos)? as usize;
                            let c = next(&mut pos)? as usize;
                            let d = next(&mut pos)? as usize;
                            (
                                ((a & 0xF) << 12 | b << 4 | c >> 4) + 0x111,
                                ((c & 0xF) << 8 | d) + 1,
                            )
                        }
                        n => {
                            let b = next(&mut pos)? as usize;
                            (n + 1, ((a & 0xF) << 8 | b) + 1)
                        }
                    }
                }
                other => return invalid(format!("unknown LZ type {other:#x}")),
            };
            if disp > out.len() {
                return invalid("LZ back-reference points before the start of the output");
            }
            for _ in 0..len {
                if out.len() >= size {
                    break;
                }
                out.push(out[out.len() - disp]);
            }
        }
    }
    Ok(out)
}

/// Decompresses backward-LZ code (ARM9 binary and overlays). Data without a
/// valid footer is returned unchanged.
pub fn blz_decompress(data: &[u8]) -> Vec<u8> {
    let n = data.len();
    if n < 8 {
        return data.to_vec();
    }
    let inc_len = u32::from_le_bytes([data[n - 4], data[n - 3], data[n - 2], data[n - 1]]) as usize;
    let footer = u32::from_le_bytes([data[n - 8], data[n - 7], data[n - 6], data[n - 5]]);
    let header_len = (footer >> 24) as usize;
    let enc_len = (footer & 0x00FF_FFFF) as usize;
    if inc_len == 0 || header_len < 8 || header_len > enc_len || enc_len > n {
        return data.to_vec();
    }

    let keep = n - enc_len;
    let mut out = vec![0u8; n + inc_len];
    out[..keep].copy_from_slice(&data[..keep]);

    let mut src = n - header_len; // one past the last compressed byte
    let mut dst = n + inc_len;
    while src > keep && dst > keep {
        src -= 1;
        let flags = data[src];
        for bit in (0..8).rev() {
            if src <= keep || dst <= keep {
                break;
            }
            if flags >> bit & 1 == 0 {
                src -= 1;
                dst -= 1;
                out[dst] = data[src];
            } else {
                if src < keep + 2 {
                    return out;
                }
                src -= 2;
                let pair = (data[src + 1] as usize) << 8 | data[src] as usize;
                let len = (pair >> 12) + 3;
                let disp = (pair & 0xFFF) + 3;
                for _ in 0..len {
                    if dst <= keep {
                        break;
                    }
                    dst -= 1;
                    let from = dst + disp;
                    out[dst] = if from < out.len() { out[from] } else { 0 };
                }
            }
        }
    }
    out
}
