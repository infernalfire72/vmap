//! Game text files: encrypted UTF-16 strings, grouped in sections.

use crate::error::Result;
use crate::reader::Reader;

const TERMINATOR: u16 = 0xFFFF;
const NEWLINE: u16 = 0xFFFE;
const VARIABLE: u16 = 0xF000;
const BEGIN_9BIT: u16 = 0xF100;

/// Characters 256.. of the compressed 9-bit alphabet.
const COMPRESSED_SPECIALS: &str = "ŒœŞş‘“”„…①②③④⑤⑥⑦⑧⑨⑩⑪⑫⑬⑭⑮⑯⑰⑱⑲⑳⑴⑵⑶⑷⑸⑹⑺⑻⑼⑽⑾⑿⒀⒁⒂⒃⒄⒅⒆⒇･";

/// Decoded text file: one list of lines per section (usually one section).
#[derive(Debug, Clone, Default)]
pub struct TextFile {
    pub sections: Vec<Vec<String>>,
}

impl TextFile {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let mut r = Reader::new(data);
        let section_count = r.u16()? as usize;
        let line_count = r.u16()? as usize;
        let _section_size = r.u32()?;
        let _reserved = r.u32()?;
        let mut offsets = Vec::with_capacity(section_count);
        for _ in 0..section_count {
            offsets.push(r.u32()? as usize);
        }

        let mut sections = Vec::with_capacity(section_count);
        for start in offsets {
            let mut lines = Vec::with_capacity(line_count);
            for l in 0..line_count {
                let mut e = Reader::at(data, start + 4 + l * 8);
                let offset = e.u32()? as usize;
                let chars = e.u16()? as usize;
                let mut s = Reader::at(data, start + offset);
                let mut key = ((l as u32 + 3) * 0x2983) as u16;
                let mut words = Vec::with_capacity(chars);
                for _ in 0..chars {
                    words.push(s.u16()? ^ key);
                    key = key.rotate_left(3);
                }
                lines.push(decode_line(&words));
            }
            sections.push(lines);
        }
        Ok(Self { sections })
    }

    pub fn lines(&self) -> &[String] {
        self.sections.first().map(|s| s.as_slice()).unwrap_or(&[])
    }
}

/// Turns decrypted 16-bit units into a string. Variables become `[VAR ...]`.
fn decode_line(words: &[u16]) -> String {
    let mut out = String::new();
    let mut i = 0;
    // 9-bit mode state.
    let mut nine = false;
    let (mut buf, mut bits) = (0u32, 0u32);

    let mut next = |i: &mut usize, nine: &mut bool| -> Option<u16> {
        if !*nine {
            let w = *words.get(*i)?;
            *i += 1;
            return Some(w);
        }
        while bits < 9 {
            buf |= (*words.get(*i)? as u32) << bits;
            *i += 1;
            bits += 16;
        }
        let c = (buf & 0x1FF) as u16;
        buf >>= 9;
        bits -= 9;
        if c == 0x1FF {
            *nine = false;
            buf = 0;
            bits = 0;
            // The terminator of the compressed run; the caller reads on.
            return Some(0x1FF);
        }
        Some(c)
    };

    while let Some(c) = next(&mut i, &mut nine) {
        if nine || c == 0x1FF {
            if c == 0x1FF {
                continue;
            }
            if c >= 256 {
                if let Some(ch) = COMPRESSED_SPECIALS.chars().nth(c as usize - 256) {
                    out.push(ch);
                }
            } else {
                out.push(char::from_u32(c as u32).unwrap_or('?'));
            }
            continue;
        }
        match c {
            TERMINATOR => break,
            NEWLINE => out.push('\n'),
            BEGIN_9BIT => nine = true,
            VARIABLE => {
                let code = next(&mut i, &mut nine).unwrap_or(0);
                let argc = next(&mut i, &mut nine).unwrap_or(0) as usize;
                let mut args = Vec::new();
                for _ in 0..argc.min(16) {
                    args.push(next(&mut i, &mut nine).unwrap_or(0).to_string());
                }
                match code {
                    0xBE00 => out.push('\n'),
                    0xBE01 => out.push('\n'),
                    _ => {
                        out.push_str(&format!("[VAR {code:04X}"));
                        if !args.is_empty() {
                            out.push_str(&format!("({})", args.join(",")));
                        }
                        out.push(']');
                    }
                }
            }
            _ => out.push(char::from_u32(c as u32).unwrap_or('?')),
        }
    }
    out
}
