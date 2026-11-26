//! Pure parsers for Nintendo DS ("Nitro") and Generation V Pokemon formats.
//!
//! Everything in this crate works on byte slices and returns plain data.
//! There is no file access, no GPU code and no UI code here, so the same
//! parsers serve the desktop viewer, the headless renderer and the MCP server.

pub mod error;
pub mod gen5;
pub mod gf;
pub mod nds;
pub mod nitro;
pub mod reader;

pub use error::{FormatError, Result};
pub use reader::Reader;
