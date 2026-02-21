pub mod bom;
pub mod error;
pub mod parsers;
pub mod types;

use error::ExtractError;
use std::path::Path;
use types::PcbData;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PcbFormat {
    KiCad,
    EasyEda,
    Eagle,
    Altium,
}

#[derive(Debug, Clone, Default)]
pub struct ExtractOptions {
    pub include_tracks: bool,
    pub include_nets: bool,
}

/// Detect format from file extension.
pub fn detect_format(path: &Path) -> Option<PcbFormat> {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .as_deref()
    {
        Some("kicad_pcb") => Some(PcbFormat::KiCad),
        Some("json") => Some(PcbFormat::EasyEda),
        Some("brd") | Some("fbrd") => Some(PcbFormat::Eagle),
        Some("pcbdoc") => Some(PcbFormat::Altium),
        _ => None,
    }
}

/// Auto-detect format from extension and parse.
pub fn extract(path: &Path, opts: &ExtractOptions) -> Result<PcbData, ExtractError> {
    let format = detect_format(path).ok_or_else(|| {
        ExtractError::UnsupportedFormat(
            path.extension()
                .and_then(|e| e.to_str())
                .unwrap_or("(none)")
                .to_string(),
        )
    })?;
    let data = std::fs::read(path)?;
    extract_bytes(&data, format, opts)
}

/// Parse from bytes with explicit format.
pub fn extract_bytes(
    data: &[u8],
    format: PcbFormat,
    opts: &ExtractOptions,
) -> Result<PcbData, ExtractError> {
    match format {
        PcbFormat::KiCad => parsers::kicad::parse(data, opts),
        PcbFormat::EasyEda => parsers::easyeda::parse(data, opts),
        PcbFormat::Eagle => parsers::eagle::parse(data, opts),
        PcbFormat::Altium => parsers::altium::parse(data, opts),
    }
}
