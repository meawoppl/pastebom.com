//! Yew components for [`crate::vector`] scenes (feature `yew`).
//!
//! - [`VectorSceneCanvas`]: format-independent Canvas2D scene host (PCB,
//!   schematic, footprint, symbol): pan/zoom/pinch, tap picking, layer
//!   visibility, compositing passes, overlays, fitting, camera reporting.
//! - [`search`]: component/net lookup over a scene and a compact search field.
//! - [`SchematicView`]: one-sheet-at-a-time schematic viewer with sheet
//!   navigation, search, selection, net highlight, layer toggles and
//!   cross-probe hooks. All I/O stays with the host.

pub mod canvas;
pub mod input;
pub mod schematic;
pub mod search;

pub use canvas::{view_of, Focus, VectorSceneCanvas, VectorSceneCanvasProps, VectorSceneCommand};
pub use schematic::{Reveal, SchematicView, SchematicViewProps};
