//! EDA file parsing and vector rendering in one crate, with each part behind a
//! cargo feature so a wasm frontend links only what it draws.
//!
//! | module | feature | contents |
//! |---|---|---|
//! | [`vector`] | always | Reified 2D scene, camera, input, hit testing, styling, SVG export, schematic metadata accessors; Canvas2D renderer with `canvas` |
//! | `components` | `yew` | Yew components: `VectorSceneCanvas`, scene search, `SchematicView` |
//!
//! Enabling only `yew` (or `canvas`) never compiles any parser or font code.

pub mod vector;

#[cfg(feature = "yew")]
pub mod components;

#[cfg(feature = "kicad")]
pub mod kicad;

#[cfg(feature = "strokes")]
pub mod strokes;

#[cfg(feature = "schematic")]
pub mod schematic;

#[cfg(feature = "pcb")]
pub mod pcb;

#[cfg(feature = "gerber-view")]
pub mod gerber_view;
