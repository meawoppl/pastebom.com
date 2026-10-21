//! A reified 2D vector scene and a minimal viewer core over it. Always
//! available; it depends only on `serde`.
//!
//! Producers (the `schematic` extractor, kicadmium's backend for KiCad
//! PCBs/footprints/symbols, Gerber layers) do all format-specific
//! work: coordinate transforms, rotation, text-to-stroke conversion, layer
//! assignment. A [`scene::Scene`] is therefore plain geometry plus pick ids and
//! properties, and viewers stay small and format-agnostic.
//!
//! - [`view`]: pan/zoom/fit/rotate/mirror transform.
//! - [`input`]: wheel, drag, pinch and tap as a pure state machine.
//! - [`hit`]: hit testing with role priority and a grid index.
//! - [`style`]: visibility, paint order, highlight and theme resolution.
//! - [`svg`]: standalone SVG export.
//! - [`schematic`]: typed accessors for schematic page/symbol metadata.
//! - `render` (feature `canvas`): Canvas2D drawing.

pub mod hit;
pub mod input;
pub mod scene;
pub mod schematic;
pub mod style;
pub mod svg;
pub mod view;

#[cfg(feature = "canvas")]
pub mod render;

pub use scene::*;
