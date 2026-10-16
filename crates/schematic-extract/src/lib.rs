//! KiCad schematics to layered vector scenes, independent of servers and browsers.
//!
//! Coordinates are millimetres, with Y pointing down. Embedded `lib_symbols`
//! supply symbol geometry; no KiCad installation or system libraries are needed.

mod discovery;
mod pin_graphics;
mod scene;
mod schematic_drawing;
mod symbol_graphics;

use std::collections::HashMap;

pub use vector_view;

/// An invalid schematic or unresolved project input.
#[derive(Debug, thiserror::Error)]
#[error("{0:#}")]
pub struct Error(#[from] anyhow::Error);

/// Select the most likely project root deterministically. Prefer a schematic
/// beside a matching `.kicad_pro` or `.kicad_pcb`; otherwise prefer files not
/// referenced as children. Ties use path depth and then lexical order.
/// macOS resource forks and `__MACOSX` metadata are ignored.
pub fn find_root(files: &HashMap<String, Vec<u8>>) -> Option<String> {
    discovery::find_root(files)
}

/// Discover a root and parse a collection of project files.
pub fn parse_files(files: &HashMap<String, Vec<u8>>) -> Result<vector_view::Scene, Error> {
    let root =
        find_root(files).ok_or_else(|| Error(anyhow::anyhow!("no KiCad schematic files found")))?;
    parse_project(&root, files)
}

/// Parse one standalone `.kicad_sch`. Missing subsheets are reported in scene
/// metadata (`warning`) so a root sheet remains viewable when uploaded alone.
pub fn parse(source: &str) -> Result<vector_view::Scene, Error> {
    parse_project(
        "schematic.kicad_sch",
        &HashMap::from([("schematic.kicad_sch".into(), source.as_bytes().to_vec())]),
    )
}

/// Parse a schematic hierarchy from an in-memory project.
///
/// Keys use relative slash-separated paths. Child paths resolve relative to
/// their parent file; paths escaping the project are rejected. Repeated sheet
/// instances are rendered separately; recursive references are rejected.
pub fn parse_project(
    root: &str,
    files: &HashMap<String, Vec<u8>>,
) -> Result<vector_view::Scene, Error> {
    scene::parse_project(root, files).map_err(Error::from)
}
