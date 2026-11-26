//! DS geometry engine display lists: four command bytes packed into a word,
//! followed by the parameter words of each command in turn.

/// Number of 32-bit parameters a geometry command takes.
pub fn param_count(cmd: u8) -> Option<usize> {
    Some(match cmd {
        0x00 => 0,
        0x10 => 1,
        0x11 => 0,
        0x12 | 0x13 | 0x14 => 1,
        0x15 => 0,
        0x16 => 16,
        0x17 => 12,
        0x18 => 16,
        0x19 => 12,
        0x1A => 9,
        0x1B | 0x1C => 3,
        0x20 | 0x21 | 0x22 => 1,
        0x23 => 2,
        0x24..=0x28 => 1,
        0x29 | 0x2A | 0x2B => 1,
        0x30 | 0x31 | 0x32 | 0x33 => 1,
        0x34 => 32,
        0x40 => 1,
        0x41 => 0,
        0x50 | 0x60 => 1,
        0x70 => 3,
        0x71 => 2,
        0x72 => 1,
        _ => return None,
    })
}

/// Calls `f(command, params)` for every command in a packed display list.
/// Stops quietly at the first malformed command.
pub fn for_each_command(dl: &[u8], mut f: impl FnMut(u8, &[u32])) {
    let word = |i: usize| -> Option<u32> {
        dl.get(i..i + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    };
    let mut params = [0u32; 32];
    let mut pos = 0;
    while let Some(packed) = word(pos) {
        pos += 4;
        for k in 0..4 {
            let cmd = (packed >> (k * 8)) as u8;
            let Some(n) = param_count(cmd) else { return };
            for p in params.iter_mut().take(n) {
                match word(pos) {
                    Some(v) => *p = v,
                    None => return,
                }
                pos += 4;
            }
            if cmd != 0 {
                f(cmd, &params[..n]);
            }
        }
    }
}

pub const PRIM_TRIANGLES: u32 = 0;
pub const PRIM_QUADS: u32 = 1;
pub const PRIM_TRIANGLE_STRIP: u32 = 2;
pub const PRIM_QUAD_STRIP: u32 = 3;

/// Sign-extends the low `bits` bits of `v`.
pub fn sext(v: u32, bits: u32) -> i32 {
    let shift = 32 - bits;
    ((v << shift) as i32) >> shift
}
