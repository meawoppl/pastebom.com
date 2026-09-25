# gerber-view

Embeddable Gerber / Excellon viewer compiled to WebAssembly. It uses the same
Gerber parser as pastebom.com (`pcb-extract`) and draws to a `<canvas>` with
pan, zoom, per-layer visibility and colour, and top/bottom views. There are no
framework dependencies and no pastebom.com app state. Mount it into any element.

## Build

```bash
cd crates/gerber-view
wasm-pack build --release --target web --out-dir pkg
```

`pkg/` then contains an ES module (`gerber_view.js`), the wasm binary, and
TypeScript definitions (`gerber_view.d.ts`). Use `--target bundler` instead to
consume it from webpack/Vite as an npm package.

Prebuilt copies are also available:
- pastebom.com serves the bundle at `https://pastebom.com/gerber-view/pkg/`
  (`gerber_view.js`, `gerber_view_bg.wasm`, `gerber_view.d.ts`), with CORS
  enabled, and a demo at `https://pastebom.com/gerber-view/`.
- Every CI run uploads a `gerber-view` artifact with `pkg/`, `examples/`, and
  this README.

## Example

`examples/index.html` is a plain HTML page with a file picker and drag-and-drop.
Serve the crate directory and open it:

```bash
python3 -m http.server -d crates/gerber-view 8000
# http://localhost:8000/examples/index.html
# http://localhost:8000/examples/index.html?src=/fab/board-F_Cu.gbr,/fab/board.drl
```

## Usage

```js
import init, { GerberViewer, parseSources } from "./pkg/gerber_view.js";

await init();

const viewer = new GerberViewer({ controls: true });   // all options optional
viewer.mount(document.getElementById("gerbers"));     // element needs a size

const project = await viewer.setSources([
  { url: "/api/kicad/file?path=fab/board-F_Cu.gbr" },  // name taken from ?path=
  { name: "board.drl", url: "/files/1234" },            // or given explicitly
  { name: "board-B_Cu.gbr", content: gerberText },      // string, ArrayBuffer, typed array, Blob
  fileInput.files[0],                                   // File; a zip is expanded
]);

viewer.setLayerVisibility("Bottom copper", false);     // by label or source name
viewer.setSide("bottom");
viewer.onChange((project) => renderMyLayerList(project.layers));
```

### Constructor options

| Option       | Default   | Description                                  |
|--------------|-----------|----------------------------------------------|
| `background` | `#12161c` | Canvas background (any CSS colour)           |
| `controls`   | `false`   | Show the built-in layer panel                |
| `padding`    | `16`      | Padding around the board when fitting, in px |

### Methods

| Method | Description |
|---|---|
| `mount(element)` | Render into `element`, filling it and following its size (`ResizeObserver`). |
| `setSources(sources) → Promise<Project>` | Replace all layers. Layers appear as each source loads. |
| `addSources(sources) → Promise<Project>` | Add layers to the current set. |
| `clear()` | Remove all layers. Pending loads from earlier calls are discarded. |
| `setLayerVisibility(layer, visible) → bool` | `layer` is a source name or a label. A kind label (`"Top copper"`) matches every layer of that kind. |
| `setLayerColor(layer, cssColor) → bool` | |
| `setLayerOpacity(layer, 0..1) → bool` | |
| `setSide("top" \| "bottom")` / `side()` | The bottom view is mirrored with the stack reversed. |
| `fit()` | Fit the board. The view keeps fitting on resize until the user pans or zooms. |
| `resize()` | Re-read the container size (only needed without `ResizeObserver`). |
| `project() → Project` | Current summary (see below). |
| `exportProject() → string` | Full parsed geometry as JSON. |
| `onChange(callback \| null)` | Called with the summary when layers, styles or the side change. |
| `destroy()` | Unmount and drop all layers. Call `free()` afterwards to release wasm memory. |

`parseSources(sources) → Promise` parses without rendering and resolves with
`{ layers, bbox, warnings, skipped }`, including full geometry.

Sources can be a single item or any iterable (Array, `FileList`). Each item is
a `File`, `{ name, content }`, or `{ name?, url }`. URLs are loaded with
`fetch()`, so they must be same-origin or CORS-enabled. When `name` is omitted,
it comes from a `path`/`file`/`filename`/`name` query parameter, or from the
last path segment. The name matters because it is used to identify the layer
when a file has no Gerber X2 `FileFunction` attribute. Zip archives are
expanded, and their entries are named `archive.zip/inner/path.gbr`.

Files are recognised by content, not extension, so hosts shouldn't filter by
extension. Pass every fabrication-output file, and non-Gerber ones will land
in `skipped`. Extensions vary widely: `.gbr`, Protel `.gtl`/`.gbs`/`.gm1`,
numbered inner copper `.g1`/`.g2`/..., `.art`, `.drl`, `.xln`, `.txt`.

A source that fails to load or parse never rejects the promise. Diagnostics
read `"<file>: <reason>"` and come in two severities:
- `warnings` are problems worth showing: fetch failures, files that look like
  Gerber but fail to parse, and layers whose function can't be identified.
- `skipped` is informational: files that aren't fabrication data (logs, PDFs,
  job files), and files with nothing to draw (empty paste or silkscreen
  layers, drill files with no holes).

### Project summary

```ts
interface Project {
  layers: Layer[];            // back-to-front stacking order
  bbox: BBox | null;          // outline extents, else copper, else all geometry
  warnings: string[];
  skipped: string[];
  side: "top" | "bottom";
}
interface Layer {
  name: string;               // source name
  label: string;              // e.g. "Top copper", "In2 copper", "Drills"; the file name for
                              // other/unknown layers; "(file)" is appended when labels collide
  function: "copper" | "silkscreen" | "solder_mask" | "solder_paste" | "outline" | "drill"
          | "other" | "unknown";
  side: "top" | "bottom" | "inner" | null;
  inner: string | null;       // "In1", "In2", ... for inner copper
  color: string;
  opacity: number;
  visible: boolean;
  drawingCount: number;
  clearDrawingCount: number;
  bbox: BBox | null;
}
interface BBox { minx: number; miny: number; maxx: number; maxy: number }
```

Coordinates are in millimetres with Y pointing down, so Gerber Y is negated.

### Interaction

Drag, scroll, or two-finger swipe to pan. Pinch, or hold ctrl/⌘ while using
the wheel, to zoom about the cursor. This matches the main pastebom viewer.
Double-click to fit.
The built-in panel toggles layers and switches between the top and bottom
views. Elements use `gv-*` class names (`gv-root`, `gv-canvas`, `gv-controls`,
`gv-layer`, `gv-swatch`, `gv-button`) so hosts can restyle them.

### Layer detection

A layer's function comes from the Gerber X2 `FileFunction` attribute when
present. Both the `%TF...%` form and KiCad's `G04 #@! TF...` comment form are
read. Otherwise the filename is checked against these conventions:
- Protel/Altium extensions (`.GTL`, `.GBS`, `.G1`, ...)
- KiCad names (`F_Cu`, `B_Mask`, `In1_Cu`, `Edge_Cuts`, ...)
- Cadence Allegro artwork (`l1_top.art`, `l3.art`, `masktop.art`, `silkbot.art`)
- Eagle and EasyEDA conventions

Documentation layers are classified as `other` and hidden by default, with no
warning. These are KiCad `User`/`Courtyard`/`Fab`/`Adhesive`/`Margin`,
non-fabrication X2 functions, and Allegro `fab`/`assy` drawings. Anything else
is still shown, as an `unknown` layer labelled with its file name and a
warning. Solder mask and paste are hidden by default.

Known gap: Altium- and Allegro-style Excellon drill files (`T1F00S00C...`
tool definitions, modal coordinates) don't parse yet, so they appear in
`skipped`. KiCad and Eagle drill files work.
