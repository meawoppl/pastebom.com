//! `vector-view`: a reified 2D vector scene and (in later modules) a minimal
//! viewer core over it.
//!
//! Producers (kicadmium's backend for KiCad PCBs/schematics/footprints/symbols,
//! pastebom's `pcb-extract` adapter, Gerber layers) do all format-specific
//! work: coordinate transforms, rotation, text-to-stroke conversion, layer
//! assignment. A [`scene::Scene`] is therefore plain geometry plus pick ids and
//! properties, and viewers stay small and format-agnostic.

pub mod scene;

pub use scene::*;
