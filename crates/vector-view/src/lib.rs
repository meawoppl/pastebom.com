//! `vector-view`: a reified 2D vector scene and a minimal viewer core over it.
//!
//! Producers (kicadmium's backend for KiCad PCBs/schematics/footprints/symbols,
//! pastebom's `pcb-extract` adapter, Gerber layers) do all format-specific
//! work: coordinate transforms, rotation, text-to-stroke conversion, layer
//! assignment. A [`scene::Scene`] is therefore plain geometry plus pick ids and
//! properties, and viewers stay small and format-agnostic.
//!
//! - [`view`]: pan/zoom/fit/rotate/mirror transform.
//! - [`input`]: wheel, drag, pinch and tap as a pure state machine.
//! - [`hit`]: hit testing with role priority and a grid index.
//! - [`style`]: visibility, paint order, highlight and theme resolution.
//! - `render` (feature `canvas`): Canvas2D drawing.

pub mod hit;
pub mod input;
pub mod scene;
pub mod style;
pub mod view;

#[cfg(feature = "canvas")]
pub mod render;

pub use scene::*;
