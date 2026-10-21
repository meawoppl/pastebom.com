# eda-view

EDA file parsing and vector rendering in one crate. It covers KiCad
schematics, PCB and Gerber parsers, a reified 2D scene with SVG and Canvas2D
renderers, and Yew/wasm viewer components. Each part sits behind a cargo
feature, so a browser bundle links only what it draws.

pastebom.com uses it for uploads and viewers. The kicadmium PCB workbench uses
it for its schematic, PCB, library and Gerber views.

## Features

| feature | module | what you get |
|---|---|---|
| *(always)* | `vector` | `Scene` (layers, items, nets, groups, props), camera `view`, pointer `input` state machine, `hit` testing, `style` (visibility, passes, highlight, theme), `svg` export, `schematic` page/symbol accessors. Depends only on `serde`. |
| `canvas` | `vector::render` | Canvas2D renderer (web-sys). |
| `yew` | `components` | Yew 0.23 components: `VectorSceneCanvas`, `search`, `SchematicView`. Implies `canvas`. |
| `kicad` | `kicad` | Lossless KiCad s-expression parser plus schematic, symbol and library models. |
| `strokes` | `strokes` | KiCad NewStroke font: text to polylines, so viewers need no fonts. |
| `schematic` | `schematic` | `.kicad_sch` hierarchy to `Scene`: `parse_files`, `parse_project`, `find_root`, `ExtractOptions`, `Palette`, `symbol_scene`. Implies `kicad` and `strokes`. Filesystem-free. |
| `pcb` | `pcb` | PCB/BOM extraction: KiCad, EasyEDA, Eagle, Altium, Gerber/Excellon, ODB++, GDSII, plus an SVG thumbnail. |
| `gerber-view` | `gerber_view` | Embeddable, framework-free Gerber viewer with a wasm-bindgen JS API (`GerberViewer`). Implies `pcb`. |
| `cli` | bin `pcb-extract` | Command-line PCB extractor. Implies `pcb`. |

Nothing is enabled by default. Typical sets:

```toml
# server: parse uploads, render scenes
eda-view = { version = "0.1", features = ["schematic", "pcb"] }
# wasm frontend: draw scenes; no parser or font code is compiled
eda-view = { version = "0.1", features = ["yew"] }
```

`yew` and `canvas` never enable `kicad`, `strokes` or `schematic`. Parsing
stays on the server, and the browser receives `Scene` JSON.

## Schematic viewer contract

`components::SchematicView` does no I/O. It never fetches and never touches
the URL. The host:

- passes `scene: Rc<Scene>` and replaces it on live refresh (the open sheet
  and its camera are kept);
- optionally controls the sheet with `page` + `on_page`;
- requests a symbol with `reveal` and hears selections via
  `on_select_ref`/`on_select`;
- gets a properties-panel action with `on_cross_probe`;
- persists cameras with `on_view`/`initial_view`;
- adds toolbar content as children.

By default the behaviour, labels and class names match kicadmium's schematic
tab. `symbol_selection` opts into whole-symbol selection, as pastebom uses it.
The root carries `data-schematic-page`, and the canvas publishes its camera as
`data-view` (read with `components::view_of`). Annotation tools can map pixels
to scene millimetres on the right sheet. Hosts without kicadmium's
stylesheet can inject `components::schematic::DEFAULT_CSS`.

## License

MIT (see `LICENSE`). The KiCad models derive from kicad-tools (MIT, see
`LICENSE-kicad-tools` and `NOTICE.md`). The NewStroke font data is CC0 (see
`LICENSE-CC0`).
