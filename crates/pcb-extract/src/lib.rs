pub mod bom;
pub mod error;
pub mod parsers;
pub mod thumbnail;
pub mod types;

use error::ExtractError;
use serde::{Deserialize, Serialize};
use std::path::Path;
use types::PcbData;

/// Maximum total decompressed size for archive contents (500 MB).
/// Prevents ZIP/tar bomb attacks where a small compressed file expands to exhaust memory.
pub const MAX_DECOMPRESSED_BYTES: u64 = 500 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PcbFormat {
    KiCad,
    EasyEda,
    Eagle,
    Altium,
    Gerber,
    Gdsii,
    #[serde(rename = "odbpp")]
    OdbPlusPlus,
}

#[derive(Debug, Clone, Default)]
pub struct ExtractOptions {
    pub include_tracks: bool,
    pub include_nets: bool,
}

/// Detect format from file extension alone (no content inspection).
pub fn detect_format(path: &Path) -> Option<PcbFormat> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase());

    match ext.as_deref() {
        Some("kicad_pcb") => Some(PcbFormat::KiCad),
        Some("json") => Some(PcbFormat::EasyEda),
        Some("brd") | Some("fbrd") => Some(PcbFormat::Eagle),
        Some("pcbdoc") => Some(PcbFormat::Altium),
        Some("tgz") => Some(PcbFormat::OdbPlusPlus),
        Some("zip") => Some(PcbFormat::Gerber),
        Some("gz") => {
            // Handle .tar.gz double extension
            let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
            if stem.ends_with(".tar") {
                Some(PcbFormat::OdbPlusPlus)
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Detect format using both filename and file content.
///
/// For unambiguous extensions, returns immediately.
/// For `.zip` archives, peeks inside to distinguish Gerber from ODB++.
pub fn detect_format_with_content(path: &Path, data: &[u8]) -> Option<PcbFormat> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase());

    match ext.as_deref() {
        Some("zip") => {
            if zip_contains_odbpp(data) {
                Some(PcbFormat::OdbPlusPlus)
            } else {
                Some(PcbFormat::Gerber)
            }
        }
        Some("tgz") => Some(PcbFormat::OdbPlusPlus),
        Some("gz") => {
            let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
            if stem.ends_with(".tar") {
                Some(PcbFormat::OdbPlusPlus)
            } else {
                None
            }
        }
        _ => detect_format(path),
    }
}

/// Check if a ZIP archive contains ODB++ structure (matrix/matrix file).
fn zip_contains_odbpp(data: &[u8]) -> bool {
    let reader = std::io::Cursor::new(data);
    let Ok(archive) = zip::ZipArchive::new(reader) else {
        return false;
    };
    let found = archive
        .file_names()
        .any(|name| name.ends_with("/matrix/matrix") || name == "matrix/matrix");
    found
}

/// Auto-detect format from extension + content and parse.
pub fn extract(path: &Path, opts: &ExtractOptions) -> Result<PcbData, ExtractError> {
    let data = std::fs::read(path)?;
    let format = detect_format_with_content(path, &data).ok_or_else(|| {
        ExtractError::UnsupportedFormat(
            path.extension()
                .and_then(|e| e.to_str())
                .unwrap_or("(none)")
                .to_string(),
        )
    })?;
    extract_bytes(&data, format, opts)
}

/// AppleDouble magic number found at the start of macOS resource fork files.
const APPLE_DOUBLE_MAGIC: [u8; 4] = [0x00, 0x05, 0x16, 0x07];

/// Parse from bytes with explicit format.
pub fn extract_bytes(
    data: &[u8],
    format: PcbFormat,
    opts: &ExtractOptions,
) -> Result<PcbData, ExtractError> {
    if data.starts_with(&APPLE_DOUBLE_MAGIC) {
        return Err(ExtractError::MacosResourceFork);
    }

    let mut pcbdata = match format {
        PcbFormat::KiCad => parsers::kicad::parse(data, opts),
        PcbFormat::EasyEda => parsers::easyeda::parse(data, opts),
        PcbFormat::Eagle => parsers::eagle::parse(data, opts),
        PcbFormat::Altium => parsers::altium::parse(data, opts),
        PcbFormat::Gerber => parsers::gerber::parse(data, opts),
        PcbFormat::Gdsii => parsers::gdsii::parse(data, opts),
        PcbFormat::OdbPlusPlus => parsers::odbpp::parse(data, opts),
    }?;
    pcbdata.format = Some(format);
    pcbdata.parser_version = Some(env!("CARGO_PKG_VERSION").to_string());
    Ok(pcbdata)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_detect_format_extensions() {
        assert_eq!(
            detect_format(Path::new("board.kicad_pcb")),
            Some(PcbFormat::KiCad)
        );
        assert_eq!(
            detect_format(Path::new("board.zip")),
            Some(PcbFormat::Gerber)
        );
        assert_eq!(
            detect_format(Path::new("board.tgz")),
            Some(PcbFormat::OdbPlusPlus)
        );
        assert_eq!(
            detect_format(Path::new("board.tar.gz")),
            Some(PcbFormat::OdbPlusPlus)
        );
        assert_eq!(detect_format(Path::new("board.xyz")), None);
    }

    #[test]
    fn test_detect_tar_gz_double_extension() {
        let path = PathBuf::from("my_board.tar.gz");
        assert_eq!(detect_format(&path), Some(PcbFormat::OdbPlusPlus));
    }

    #[test]
    fn test_detect_plain_gz_not_matched() {
        let path = PathBuf::from("compressed.gz");
        assert_eq!(detect_format(&path), None);
    }

    #[test]
    fn test_reject_macos_resource_fork() {
        let mut data = vec![0x00, 0x05, 0x16, 0x07];
        data.extend_from_slice(b"Mac OS X        ");

        let opts = ExtractOptions::default();
        let result = extract_bytes(&data, PcbFormat::KiCad, &opts);

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, ExtractError::MacosResourceFork),
            "expected MacosResourceFork error, got: {err}"
        );
        assert!(
            err.to_string().contains("macOS resource fork"),
            "error message should mention macOS resource fork"
        );
    }
}
