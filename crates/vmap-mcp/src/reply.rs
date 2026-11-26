//! What a tool hands back: text, and sometimes a picture.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use base64::Engine;
use rmcp::model::{CallToolResult, ContentBlock};
use serde_json::Value;

/// A picture as RGBA bytes, row by row from the top.
pub struct Picture {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Picture {
    pub fn new(width: u32, height: u32, rgba: Vec<u8>) -> Self {
        Self { width, height, rgba }
    }

    /// A picture filled with one colour.
    pub fn filled(width: u32, height: u32, color: [u8; 3]) -> Self {
        let mut rgba = Vec::with_capacity((width * height * 4) as usize);
        for _ in 0..width * height {
            rgba.extend_from_slice(&[color[0], color[1], color[2], 255]);
        }
        Self { width, height, rgba }
    }

    pub fn put(&mut self, x: u32, y: u32, color: [u8; 3]) {
        if x < self.width && y < self.height {
            let i = ((y * self.width + x) * 4) as usize;
            self.rgba[i..i + 3].copy_from_slice(&color);
        }
    }

    /// Blends a colour over a pixel.
    pub fn blend(&mut self, x: u32, y: u32, color: [u8; 4]) {
        if x < self.width && y < self.height {
            let i = ((y * self.width + x) * 4) as usize;
            let a = color[3] as u32;
            for c in 0..3 {
                let under = self.rgba[i + c] as u32;
                self.rgba[i + c] = ((color[c] as u32 * a + under * (255 - a)) / 255) as u8;
            }
        }
    }

    fn rgb(&self) -> Vec<u8> {
        self.rgba.chunks_exact(4).flat_map(|p| [p[0], p[1], p[2]]).collect()
    }

    /// Encodes the picture as an opaque PNG.
    pub fn png(&self) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        let encoder = image::codecs::png::PngEncoder::new(&mut out);
        image::ImageEncoder::write_image(encoder, &self.rgb(), self.width, self.height, image::ExtendedColorType::Rgb8)
            .context("encoding the picture")?;
        Ok(out)
    }

    fn jpeg(&self, quality: u8) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality)
            .encode(&self.rgb(), self.width, self.height, image::ExtendedColorType::Rgb8)
            .context("encoding the picture")?;
        Ok(out)
    }

    /// The picture as it is sent to the client: the PNG while that is
    /// small, otherwise a JPEG. A detailed view of a large map makes a PNG
    /// of more than a megabyte, which some clients refuse in an answer.
    pub fn to_send(&self, png: Vec<u8>) -> Result<SentImage> {
        if png.len() <= SEND_LIMIT {
            return Ok(SentImage { bytes: png, mime: "image/png" });
        }
        let mut bytes = self.jpeg(90)?;
        if bytes.len() > SEND_LIMIT {
            bytes = self.jpeg(75)?;
        }
        Ok(SentImage { bytes, mime: "image/jpeg" })
    }
}

/// Largest picture, in bytes, that is sent as PNG.
const SEND_LIMIT: usize = 480 * 1024;

/// An encoded picture on its way to the client.
pub struct SentImage {
    pub bytes: Vec<u8>,
    pub mime: &'static str,
}

impl SentImage {
    pub fn png(bytes: Vec<u8>) -> Self {
        Self { bytes, mime: "image/png" }
    }

    pub fn is_lossy(&self) -> bool {
        self.mime != "image/png"
    }
}

/// The answer of a tool.
pub struct Reply {
    pub text: String,
    pub image: Option<SentImage>,
}

impl Reply {
    pub fn text(text: impl Into<String>) -> Self {
        Self { text: text.into(), image: None }
    }

    pub fn json(value: &Value) -> Self {
        Self::text(to_text(value))
    }

    pub fn into_result(self) -> CallToolResult {
        let mut content = Vec::new();
        if let Some(image) = self.image {
            content.push(ContentBlock::image(base64::engine::general_purpose::STANDARD.encode(image.bytes), image.mime));
        }
        content.push(ContentBlock::text(self.text));
        CallToolResult::success(content)
    }
}

/// Writes a PNG where a tool was asked to. Only `.png` files, and never
/// inside the folders of the open game: this tool reads games, it does not
/// change them.
pub fn save_png(path: &str, png: &[u8], protected: &[PathBuf]) -> Result<String> {
    let path = Path::new(path);
    if !path.extension().is_some_and(|e| e.eq_ignore_ascii_case("png")) {
        bail!("save_to must name a .png file");
    }
    if !path.is_absolute() {
        bail!("save_to must be an absolute path");
    }
    let parent = path.parent().context("save_to has no folder")?;
    let parent = parent.canonicalize().with_context(|| format!("the folder {} does not exist", parent.display()))?;
    for folder in protected.iter().filter_map(|f| f.canonicalize().ok()) {
        if parent.starts_with(&folder) {
            bail!("save_to points into the folder of the open game, which is never written to");
        }
    }
    let file = parent.join(path.file_name().context("save_to has no file name")?);
    std::fs::write(&file, png).with_context(|| format!("writing {}", file.display()))?;
    Ok(file.display().to_string().trim_start_matches(r"\\?\").to_string())
}

/// JSON laid out for reading: a value stays on one line while it is short,
/// and is broken into one entry per line when it is not.
pub fn to_text(value: &Value) -> String {
    let mut out = String::new();
    write_value(value, 0, &mut out);
    out
}

const LINE: usize = 110;
/// A record of plain values (an entity, a file, a zone) may be this long
/// before it is broken up: lists of records read best one per line.
const RECORD: usize = 420;

fn is_record(value: &Value) -> bool {
    let plain = |v: &Value| !v.is_object() && !v.is_array();
    match value {
        Value::Object(map) => map.values().all(|v| match v {
            Value::Array(items) => items.iter().all(plain),
            Value::Object(inner) => inner.len() <= 4 && inner.values().all(plain),
            other => plain(other),
        }),
        _ => false,
    }
}

fn write_value(value: &Value, depth: usize, out: &mut String) {
    let compact = compact(value);
    let container = matches!(value, Value::Object(m) if !m.is_empty()) || matches!(value, Value::Array(a) if !a.is_empty());
    let fits = compact.len() + depth * 2 <= if is_record(value) { RECORD } else { LINE };
    if !container || fits {
        out.push_str(&compact);
        return;
    }
    let pad = "  ".repeat(depth + 1);
    match value {
        Value::Object(map) => {
            out.push_str("{\n");
            for (i, (key, v)) in map.iter().enumerate() {
                out.push_str(&pad);
                out.push_str(&Value::String(key.clone()).to_string());
                out.push_str(": ");
                write_value(v, depth + 1, out);
                out.push_str(if i + 1 < map.len() { ",\n" } else { "\n" });
            }
            out.push_str(&"  ".repeat(depth));
            out.push('}');
        }
        Value::Array(items) => {
            out.push_str("[\n");
            for (i, v) in items.iter().enumerate() {
                out.push_str(&pad);
                write_value(v, depth + 1, out);
                out.push_str(if i + 1 < items.len() { ",\n" } else { "\n" });
            }
            out.push_str(&"  ".repeat(depth));
            out.push(']');
        }
        _ => unreachable!("only containers are broken into lines"),
    }
}

fn compact(value: &Value) -> String {
    match value {
        Value::Object(map) => {
            let parts: Vec<String> =
                map.iter().map(|(k, v)| format!("{}: {}", Value::String(k.clone()), compact(v))).collect();
            format!("{{{}}}", parts.join(", "))
        }
        Value::Array(items) => {
            let parts: Vec<String> = items.iter().map(compact).collect();
            format!("[{}]", parts.join(", "))
        }
        other => other.to_string(),
    }
}

/// Rounds for display: positions do not need seven decimals.
pub fn round(v: f32, decimals: i32) -> f64 {
    let f = 10f64.powi(decimals);
    (v as f64 * f).round() / f
}

pub fn vec3(v: glam::Vec3) -> Value {
    serde_json::json!([round(v.x, 1), round(v.y, 1), round(v.z, 1)])
}

/// Parses `#rrggbb` (or `rrggbb`) into red, green and blue.
pub fn parse_color(text: &str) -> Result<[u8; 3]> {
    let hex = text.trim().trim_start_matches('#');
    if hex.len() != 6 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        bail!("a colour is written as #rrggbb, for example #202830; got \"{text}\"");
    }
    let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).expect("checked above");
    Ok([byte(0), byte(2), byte(4)])
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn short_values_stay_on_one_line_and_long_ones_are_broken_up() {
        assert_eq!(to_text(&json!({"a": 1, "b": [1, 2]})), r#"{"a": 1, "b": [1, 2]}"#);
        let long: Vec<Value> = (0..12).map(|i| json!({"index": i, "label": "an entry of a list"})).collect();
        let text = to_text(&json!({ "items": long }));
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 16, "{text}");
        assert_eq!(lines[2], r#"    {"index": 0, "label": "an entry of a list"},"#);
        // Whatever the layout, it stays valid JSON.
        assert!(serde_json::from_str::<Value>(&text).is_ok());
    }

    #[test]
    fn colours_are_read_from_hex() {
        assert_eq!(parse_color("#20FF08").unwrap(), [0x20, 0xFF, 0x08]);
        assert!(parse_color("sky blue").is_err());
    }
}
