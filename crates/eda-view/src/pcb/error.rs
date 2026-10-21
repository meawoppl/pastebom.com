use thiserror::Error;

#[derive(Error, Debug)]
pub enum ExtractError {
    #[error("unsupported file format: {0}")]
    UnsupportedFormat(String),

    #[error("file appears to be a macOS resource fork (AppleDouble), not a valid PCB file")]
    MacosResourceFork,

    #[error("parse error: {0}")]
    ParseError(String),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("ZIP error: {0}")]
    Zip(#[from] zip::result::ZipError),

    #[error("decompression bomb detected: decompressed content exceeds safe size limit")]
    DecompressionBomb,
}
