# kicad-strokes

Reusable KiCad text layout as font-free polylines in millimetres. `to_strokes`
accepts a `TextSpec`; `TextStrokes::into_prim` converts the result to the shared
`vector-view` scene model. Native and `wasm32-unknown-unknown` builds use the same
layout implementation without a browser, font engine, or KiCad installation.

The implementation and regression fixtures originate in
`agent-portal-plugins/kicadmium/kicad-strokes` at commit
`5b9c3372482dbf52c3a971abc8c6ad8f8d586847`, where the crate is MIT licensed.
The bundled NewStroke font by Vladimir Uryvaev is the 2015 CC0 release;
its source archive, checksums, attribution, and import rules are recorded in
[the provenance record](docs/newstroke-provenance.md). The generator lives in
`examples/gen_glyphs.rs` and a regression test pins the glyph table checksum.

Run `cargo test -p kicad-strokes` for layout and KiCad SVG fixture comparisons.
