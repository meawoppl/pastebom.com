//! wasm-pack entry point for the `gerber_view` JS package. The viewer lives in
//! `eda_view::gerber_view`; its wasm-bindgen exports become this package's API.

pub use eda_view::gerber_view::*;
