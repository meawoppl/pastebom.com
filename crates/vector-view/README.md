# vector-view

Reusable, serializable vector scenes for PCB, schematic, Gerber, footprint, and
symbol viewers. Geometry, layers, item identities, groups, nets, and properties
are independent of source formats and frontend frameworks. Coordinates use
millimetres; the scene declares its Y-axis convention. Text arrives as strokes.

The default build provides scenes, hit testing, pan/zoom transforms, interaction
state, styling, and standalone SVG export without browser dependencies. Enable
`canvas` for the `web-sys` Canvas2D renderer. Both configurations support WASM.

The `schematic` module provides typed sheet and placed-symbol metadata,
page bounds/item filters, parent/child navigation, and producer warnings.
It has no KiCad parser or font dependency. Page order follows the producer's
hierarchy traversal; missing legacy properties have documented defaults.

```rust
use vector_view::{Scene, SceneKind};
use vector_view::svg::SvgOptions;

let scene = Scene::new(SceneKind::Schematic, true);
let svg = scene.to_svg(&SvgOptions::default());
assert!(svg.starts_with("<svg"));
```

SVG export uses the scene's layer colors and visibility and emits layers in
paint order. Each group retains `data-layer`, `data-name`, and `data-item`
attributes, plus optional group and net identifiers. `SvgOptions` can select
layers and item ids (for a single document page), crop to a bounding box, add a
margin, and draw a background. All geometry and stroked text are embedded;
there are no fonts, scripts, or external references. Polygon holes use even-odd
fill. SVG export does not apply Canvas-specific view state or compositing.

Run `cargo test -p vector-view` and
`cargo check -p vector-view --target wasm32-unknown-unknown` to validate the
shared core.
