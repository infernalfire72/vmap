use thiserror::Error;

#[derive(Debug, Error)]
pub enum FormatError {
    #[error("unexpected end of data at offset {offset:#x} (needed {needed} more bytes)")]
    Eof { offset: usize, needed: usize },
    #[error("bad magic: expected {expected:?}, found {found:?}")]
    BadMagic { expected: String, found: String },
    #[error("{0}")]
    Invalid(String),
}

pub type Result<T> = std::result::Result<T, FormatError>;

pub(crate) fn invalid<T>(msg: impl Into<String>) -> Result<T> {
    Err(FormatError::Invalid(msg.into()))
}
