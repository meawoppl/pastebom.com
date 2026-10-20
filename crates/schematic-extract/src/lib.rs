//! KiCad schematics to layered vector scenes, independent of servers and browsers.
//!
//! Coordinates are millimetres, with Y pointing down. Embedded `lib_symbols`
//! supply symbol geometry; no KiCad installation or system libraries are needed.

mod discovery;
mod pin_graphics;
mod scene;
mod schematic_drawing;
mod symbol_graphics;
mod symbol_scene;

use std::collections::HashMap;

pub use kicad_schema;
pub use kicad_schema::{schema::library::LibrarySymbol, sexp::SExp};
pub use scene::palette::{self, Palette};
pub use symbol_scene::{symbol_scene, symbol_scene_for_unit};
pub use vector_view;
pub use vector_view::schematic;

/// Rendering choices shared by project and standalone schematic extraction.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExtractOptions {
    pub palette: Palette,
    /// Optional application label in the bottom-left title-block cell.
    pub frame_label: Option<String>,
}

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
    parse_files_with(files, &ExtractOptions::default())
}

/// Discover and render a project with explicit rendering options.
pub fn parse_files_with(
    files: &HashMap<String, Vec<u8>>,
    options: &ExtractOptions,
) -> Result<vector_view::Scene, Error> {
    let root =
        find_root(files).ok_or_else(|| Error(anyhow::anyhow!("no KiCad schematic files found")))?;
    parse_project_with(&root, files, options)
}

/// Parse one standalone `.kicad_sch`. Missing subsheets are reported in scene
/// metadata (`warning`) so a root sheet remains viewable when uploaded alone.
pub fn parse(source: &str) -> Result<vector_view::Scene, Error> {
    parse_with(source, &ExtractOptions::default())
}

/// Render a standalone schematic with explicit rendering options.
pub fn parse_with(source: &str, options: &ExtractOptions) -> Result<vector_view::Scene, Error> {
    parse_project_with(
        "schematic.kicad_sch",
        &HashMap::from([("schematic.kicad_sch".into(), source.as_bytes().to_vec())]),
        options,
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
    parse_project_with(root, files, &ExtractOptions::default())
}

/// Render an in-memory schematic hierarchy with explicit rendering options.
pub fn parse_project_with(
    root: &str,
    files: &HashMap<String, Vec<u8>>,
    options: &ExtractOptions,
) -> Result<vector_view::Scene, Error> {
    scene::parse_project(root, files, options).map_err(Error::from)
}
