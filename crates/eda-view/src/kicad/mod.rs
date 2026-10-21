//! Reusable KiCad schematic models and s-expression parsing.
//!
//! Parse in-memory KiCad files with [`sexp::parse`] and construct a
//! [`schema::schematic::Schematic`] with `new`. Parsing and model
//! inspection work on native targets and WebAssembly. File loading, saving,
//! library discovery, and editing helpers that generate identifiers require a
//! native operating system.
//!
//! Extracted from KiCadmium's MIT-licensed `kct` schema and s-expression modules.
//! See `NOTICE.md` and `LICENSE-kicad-tools` for provenance.

mod fsutil;
pub mod schema;
pub mod sexp;

pub type Result<T> = anyhow::Result<T>;
