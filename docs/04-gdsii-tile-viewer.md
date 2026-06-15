# Work Unit 4: GDSII Tile Viewer (streamed records → BSP index → SVG tile pyramid → WASM viewer)

A separate viewing path for GDSII (`.gds`/`.gds2`) layouts. GDSII is an IC
mask-layout format, not a PCB format, so it does **not** belong on the existing
`PcbData` → BOM viewer path. Instead of one monolithic render, GDSII gets a
**map-tile / slippy-map** treatment: the layout is sliced into a pyramid of
pre-rendered SVG tiles (one SVG per layer per tile), and a zoomable WASM viewer
stacks the per-layer tiles like map overlays.

> **Status:** design. Nothing here is built yet. This document specifies the
> data flow, the on-disk/S3 artifact layout, the reuse boundary against the
> existing code, and a milestone plan.

---

## Motivation

The current viewer ([Work Unit 2](02-interactive-viewer.md)) loads a single
`PcbData` JSON blob over `/b/{id}/data` and draws every primitive onto four
stacked HTML5 canvases. That model assumes a *board*: thousands of primitives,
a handful of copper layers, one screenful of content.

GDSII breaks every one of those assumptions:

| Property | PCB (today) | GDSII (this work unit) |
|---|---|---|
| Element count | 10²–10⁴ | 10⁵–10⁸ (full chips) |
| Layers | ~4–32, fixed semantics | arbitrary `(layer, datatype)` pairs, hundreds |
| Hierarchy | flat footprints | deep SREF/AREF cell instancing, huge array fan-out |
| Zoom range | fit-to-screen + a few × | many decades (full reticle → single transistor) |
| Render strategy | one canvas pass | impossible to draw at once; must tile + LOD |

The existing GDSII parser (`crates/pcb-extract/src/parsers/gdsii.rs`,
`pub fn parse(data, opts) -> Result<PcbData, ExtractError>`, line 1138)
**flattens** the cell hierarchy into `PcbData` and hard-caps at
`MAX_FLATTEN_ELEMENTS = 500_000` (line 97), `MAX_AREF_INSTANCES = 1_000_000`
(line 101), `MAX_FOOTPRINTS = 5_000` (line 104). It also discards everything a
mask layout cares about: **datatype** is parsed then ignored, **TEXT** is
dropped, hierarchy collapses to footprints, and layer numbers are squashed into
PCB layer names via `layer_name()` (`0 → "F"`, `1 → "B"`, …). That mapping is
fine for "is this roughly a board," useless for a real layout viewer.

So this is a genuinely different *path*, sharing low-level primitives (the GDSII
record reader, the SVG-`d`-string helpers, the `BBox` type, the viewer's
pan/zoom transform, the server's pull-through tile cache) but not the
`PcbData` model.

---

## Goals / non-goals

**Goals**

- Stream a `.gds`/`.gds2` file into a layer-aware, **lossless-enough** record
  set: keep `(layer, datatype)`, geometry, and per-record extent.
- Build a **BSP spatial index** over record extents for fast tile range-queries.
- Generate a **tile pyramid**: per zoom level, per tile, **one SVG per layer**.
- **Level-of-detail (LOD) culling**: drop features whose on-tile footprint is
  below a pixel-area threshold; replace them with an **AABB overlay** layer that
  draws bounding boxes annotated `(n sub-records not rendered)`.
- A **WASM viewer** that reads a manifest, lets the user **toggle and reorder
  layers**, and renders the per-layer SVG tiles **stacked** with pan/zoom/pinch.

**Non-goals (for v1)**

- DRC/LVS, netlist extraction, or any electrical interpretation.
- Editing. This is read-only.
- Perfect GDSII fidelity (custom path end-caps, every PROPATTR, etc.). We render
  BOUNDARY, PATH, BOX, NODE, TEXT, and resolved SREF/AREF geometry.
- Client-side parsing of the raw `.gds`. Parsing + tiling happen server-side;
  the browser only consumes tiles + manifest.

---

## Architecture overview

```
                        ┌──────────────────────────────────────────────┐
   upload .gds  ───────►│                 Axum server                   │
                        │                                               │
                        │  POST /upload  (now accepts .gds/.gds2)        │
                        │       │                                        │
                        │       ▼                                        │
                        │  ┌─────────────────────────────────────────┐  │
                        │  │  gdsii-tile crate (NEW)                  │  │
                        │  │                                          │  │
                        │  │  1. stream_records()  ── reuse record    │  │
                        │  │       reader from gdsii.rs               │  │
                        │  │            │  PlacedRecord{layer,dt,      │  │
                        │  │            │     geom, bbox}              │  │
                        │  │            ▼                             │  │
                        │  │  2. BspIndex::build(records)             │  │
                        │  │            │                             │  │
                        │  │            ▼                             │  │
                        │  │  3. tile pyramid generation             │  │
                        │  │       per z / x / y / layer → SVG        │  │
                        │  │       + LOD cull → omitted AABB overlay  │  │
                        │  │            │                             │  │
                        │  │            ▼                             │  │
                        │  │  4. manifest.json (layers, extent, zoom) │  │
                        │  └─────────────────────────────────────────┘  │
                        │       │                                        │
                        │       ▼                                        │
                        │  S3 / filesystem storage                       │
                        │     gdsii/{id}/raw.gds                         │
                        │     gdsii/{id}/index.bin        (BSP, cached)  │
                        │     gdsii/{id}/manifest.json                   │
                        │     gdsii/{id}/tiles/{z}/{x}/{y}/{layer}.svgz  │
                        │                                                │
                        │  GET /g/{id}              viewer shell (WASM)  │
                        │  GET /g/{id}/manifest.json                     │
                        │  GET /g/{id}/tiles/{z}/{x}/{y}/{layer}.svgz    │
                        └──────────────────────────────────────────────┘
                                          ▲
                                          │ fetch manifest, then tiles on demand
                        ┌─────────────────┴──────────────────────────────┐
                        │            gds-viewer  (Yew/WASM, NEW)          │
                        │   • reuse Transform + pan/zoom/pinch handlers   │
                        │   • layer panel: reorder (drag) + toggle + α    │
                        │   • stacked per-layer tile grids                │
                        │   • LOD overlay: "(n sub-records not rendered)" │
                        └────────────────────────────────────────────────┘
```

Four pipeline stages (server) + one consumer (browser). Stages 1–2 run once per
upload; stage 3 is **hybrid** (low-zoom eager, high-zoom on-demand + cached,
exactly like the existing `thumb.svg` pull-through cache); stage 4 streams the
artifacts.

---

## Coordinate systems and the tiling scheme

GDSII stores integer **database units** (DBU). The `UNITS` record carries two
`f64`s: `user_units_per_db_unit` and `meters_per_db_unit` (parsed today in
`gdsii.rs` ~lines 663–674, default `(0.001, 1e-9)` ⇒ 1 nm DBU). We keep geometry
in **i64 DBU** internally for exact integer math (i32 can overflow on stitched
reticles; promote on read), and expose **nanometers** as the canonical
"world" unit in the manifest so the viewer never juggles per-file scale factors.

We use a **slippy-map pyramid** keyed `(z, x, y)`:

- Let the layout extent be `W × H` in world units (nm), origin at the
  bottom-left of the design AABB. (Note the existing thumbnail/parse path
  Y-flips with `xy_to_mm` negating Y; for tiles we keep GDSII's native Y-up in
  world space and flip once, in the SVG `viewBox`, so tile math stays sign-clean.)
- **Tile pixel size** `TILE_PX = 512`. Tiles are square in *pixels*; their world
  span depends on zoom.
- Choose the number of levels `Z` so that the **base level `z = 0`** fits the
  whole design in a single `TILE_PX` tile (the "thumbnail" zoom), and the
  **deepest level `z = Z-1`** reaches ~1 world-unit-per-pixel (transistor scale).

World span of one tile at level `z`:

```
span(z) = ceil_pow2(max(W, H)) / 2^z          // world units per tile edge
res(z)  = span(z) / TILE_PX                    // world units per pixel
```

Tile address of a world point `(wx, wy)` at level `z`:

```
x = floor(wx / span(z))
y = floor((H_pad - wy) / span(z))             // y inverted for screen-down rows
```

Number of levels:

```
Z = 1 + ceil(log2( ceil_pow2(max(W,H)) / (TILE_PX * res_min) ))
res_min = 1.0   // 1 nm/px at the deepest zoom; configurable per manifest
```

This is the standard web-map relationship: each level doubles linear resolution,
quadruples tile count. We store it explicitly in the manifest so the viewer
never recomputes pyramid geometry from heuristics.

The per-tile **SVG `viewBox`** maps the tile's world AABB directly into a
`0 0 TILE_PX TILE_PX` user space, with a single `transform="matrix(...)"` (or a
`viewBox` with flipped Y) doing the world→pixel mapping — i.e. the exact pattern
already in `thumbnail.rs` (`viewBox="{vx} {vy} {vw} {vh}"`, lines ~30–40), just
parameterized per tile instead of per board.

---

## Stage 1 — Streaming parse + per-record extents

### 1.1 Reuse the record reader, not `parse()`

`gdsii.rs` already contains a correct low-level GDSII reader: the `Record` /
`RecordData` types (lines ~107–120), the binary record loop (~198–247), and the
excess-64 IBM float decoder (~162–185). **Refactor these out of `gdsii.rs` into a
shared `pcb_extract::parsers::gdsii::reader` submodule** so both the existing
`parse()` and the new tiler consume the same byte-level reader. This is a pure
extraction (no behavior change) and keeps the two paths from drifting — same
spirit as the CLAUDE.md "viewer and thumbnail are two paths over one model" note.

What the tiler needs that `parse()` throws away:

- **`datatype`** alongside `layer` (currently parsed and dropped, ~line 677).
- **TEXT** elements (currently dropped, ~lines 640–650).
- **BOX** and **NODE** (currently recognized but unhandled).
- **No PCB layer-name squashing** — keep raw `(layer: i16, datatype: i16)`.

### 1.2 The placed-record stream

We resolve hierarchy by walking SREF/AREF, but **emit a flat stream of placed
records** rather than building a `Vec<GdsStructure>` tree and then a `PcbData`.
The existing `flatten_structure()` (~lines 950–1135) and `compute_structure_bbox()`
(~lines 800–948) already implement the transform stack (mirror via `strans`
bit `0x8000`, `mag`, `angle`, recursion depth 64). We reuse that **transform
algebra** but change the sink: instead of pushing into `FlattenOutput`, push
into a streaming consumer that computes an extent per record and feeds the index
builder.

```rust
/// One fully-placed (instanced) geometry element in world units (nm).
pub struct PlacedRecord {
    pub layer: i16,
    pub datatype: i16,
    pub kind: RecordKind,
    pub bbox: WorldBox,          // axis-aligned extent, i64 nm
    pub geom: Geom,              // resolved coordinates, i64 nm
}

pub enum RecordKind { Boundary, Path, Box, Node, Text }

pub enum Geom {
    /// closed polygon ring(s)
    Poly { rings: Vec<Vec<[i64; 2]>> },
    /// center-line + half-width (PATH); end-cap from pathtype
    Path { pts: Vec<[i64; 2]>, half_width: i64, pathtype: u8 },
    /// label anchor + string (TEXT)
    Label { at: [i64; 2], text: String, mag: f64, angle: f64 },
}

pub struct WorldBox { pub minx: i64, pub miny: i64, pub maxx: i64, pub maxy: i64 }
```

`WorldBox` is the integer twin of the existing `BBox` (`types.rs` lines 73–143,
with `expand_point` / `from_drawings`). We keep `BBox` for the float SVG side and
convert at the boundary; the index stays in exact integers.

### 1.3 The instancing problem (and why we cap differently)

`parse()`'s 500k element cap exists to protect the BOM path. The tiler can't
accept that cap — a real chip *is* tens of millions of polygons. Two strategies,
chosen per-file by a size heuristic recorded in the manifest:

1. **Eager flatten (small/medium, < ~5M elements).** Walk once, emit all
   `PlacedRecord`s, build the index in memory, persist it to
   `gdsii/{id}/index.bin`. Simple; this is the v1 default.

2. **Lazy AREF expansion (large).** Keep array references *unexpanded* in the
   index as a single node carrying `(cols, rows, pitch, child_cell_bbox)`, and
   expand instances **only for the tiles a query touches**, during tile
   generation. This bounds memory to the unique-cell geometry plus the index,
   not the full instance count. The transform stack from `flatten_structure()`
   is reused per-expansion. (v2 — design the `Geom` enum and BSP node to allow an
   `InstancedRef` variant now so we don't repaginate later.)

Either way we **raise/parameterize** `MAX_FLATTEN_ELEMENTS` for the tile path via
`ExtractOptions`-style config, and back the index with disk/S3 rather than
holding everything resident.

### 1.4 The BSP index

The user asked specifically for a **BSP tree** over record extents. For
axis-aligned rectangles the practical form is a **k-d-style BSP**: recursively
split the world AABB by an axis-aligned plane (alternating X/Y, cut at the median
of record centroids), descend until a node holds ≤ `LEAF_CAP` records or
`max_depth` is hit.

```rust
pub struct BspIndex {
    pub bounds: WorldBox,
    pub nodes: Vec<BspNode>,     // arena; node 0 is root
    pub records: Vec<PlacedRecord>,
}

pub enum BspNode {
    Leaf { record_ids: Vec<u32> },
    Split {
        axis: Axis,              // X | Y
        cut: i64,                // world coordinate of the plane
        below: u32,              // child node index (< cut)
        above: u32,              // child node index (>= cut)
    },
}
```

Records that straddle the cut plane are referenced from **both** children
(loose-BSP style) so a single AABB range-query never misses a straddler. The
trade-off (some records visited by multiple leaves) is bounded by choosing cuts
at centroid medians and is cheap relative to SVG emission.

**Tile query** = AABB range query: descend, pruning subtrees whose `bounds`
don't intersect the tile box, collect record ids (dedup via a visited bitset
per query). This is `O(log n + k)` for `k` hits — the whole point of the index.

The index is serialized (`bincode`/`postcard`) to `gdsii/{id}/index.bin` so tile
regeneration (cache eviction, new zoom levels) doesn't re-parse the `.gds`.

---

## Stage 2 — Tile pyramid generation

For each level `z`, for each tile `(x, y)` that overlaps the design extent:

```
for layer in layers_present:
    records = bsp.query(tile_world_box).filter(|r| r.layer_key == layer)
    (kept, omitted) = lod_partition(records, z)        // §LOD below
    svg = render_layer_tile(kept, tile_world_box, layer.style)
    if !omitted.is_empty():
        # omitted go to a synthetic overlay "layer" for this tile
        overlay[tile].extend(omitted_aabbs_with_counts)
    store("gdsii/{id}/tiles/{z}/{x}/{y}/{layer}.svgz", gzip(svg))
# after all real layers for the tile:
store(".../{z}/{x}/{y}/__lod.svgz", render_overlay(overlay[tile]))
```

"Each tile is N layers" (the user's framing) falls straight out: a tile address
`(z,x,y)` resolves to *N* sibling SVGs, one per visible layer, plus one synthetic
`__lod` overlay. The viewer stacks them.

### 2.1 Reusing the SVG path tooling

`thumbnail.rs` already builds SVG by `write!`-ing into a `String` (no SVG crate)
and contains exactly the helpers we need, currently inline:

- polygon → `M…L…Z` `d`-string with rotation/offset (render_drawings, ~142–178),
- segment/`<line>`, circle/`<circle>`, rect/`<rect>`, arc `<path A>` (~86–199),
- `viewBox` + aspect math (~14–40),
- `fill-rule="evenodd"`, stroke-width clamp `if w < 0.1 { 0.1 }`.

**Extract these into `pcb_extract::svg` as reusable free functions** so both the
thumbnail and the tiler call them (again, the "don't let the two render paths
drift" rule from CLAUDE.md):

```rust
// pcb_extract::svg  (NEW module, lifted verbatim from thumbnail.rs internals)
pub fn poly_to_d(rings: &[Vec<[f64; 2]>]) -> String;          // M/L/Z, multi-ring
pub fn polyline_to_d(pts: &[[f64; 2]]) -> String;             // M/L (open path)
pub fn view_box(b: &BBox, margin: f64) -> (f64, f64, f64, f64);
pub fn stroke_w(w: f64) -> f64;                               // clamp
```

`render_layer_tile()` then becomes a thin loop over `PlacedRecord`s for one
layer, emitting one `<path>`/`<polyline>` each into a tile-sized SVG:

```rust
fn render_layer_tile(recs: &[&PlacedRecord], tile: WorldBox, style: &LayerStyle) -> String {
    let mut s = String::with_capacity(16 * 1024);
    let (vx, vy, vw, vh) = tile_viewbox(tile);   // world AABB → flipped viewBox
    write!(s, r#"<svg xmlns="http://www.w3.org/2000/svg" width="{TILE_PX}" height="{TILE_PX}" viewBox="{vx} {vy} {vw} {vh}">"#).unwrap();
    for r in recs {
        match &r.geom {
            Geom::Poly { rings } => write!(s, r#"<path d="{}" fill="{}" fill-rule="evenodd"/>"#, svg::poly_to_d(&to_f64(rings)), style.fill).unwrap(),
            Geom::Path { pts, half_width, .. } => write!(s, r#"<path d="{}" fill="none" stroke="{}" stroke-width="{}" stroke-linecap="round"/>"#, svg::polyline_to_d(&to_f64(pts)), style.stroke, svg::stroke_w(2.0 * half_width_world(*half_width))).unwrap(),
            Geom::Label { at, text, .. } => write!(s, r#"<text x="{}" y="{}" font-size="{}" fill="{}">{}</text>"#, at[0], at[1], style.label_px, style.fill, xml_escape(text)).unwrap(),
        }
    }
    s.push_str("</svg>");
    s
}
```

Tiles are stored **gzip-compressed** (`.svgz`) — SVG path text compresses
~5–10×, and browsers accept `Content-Encoding: gzip` natively. Empty tiles
(no records on that layer) are **not stored**; the manifest's per-layer coverage
lets the viewer skip requests that would 404.

### 2.2 Level-of-detail culling (the AABB overlay)

The user's instinct — "computed by tile image area" — is exactly right. At a
given level a record projects to a pixel footprint; if it's smaller than a
threshold it's invisible noise that bloats the tile. Define, per record per tile:

```
px_area(r, z) = (r.bbox area in world units) / res(z)^2     // res(z) = world units / px
keep  ⟺  px_area(r, z) >= LOD_MIN_PX     (e.g. 1.0–4.0 px²)
```

So at the **top of the pyramid** (large `res(z)`), only the big shapes survive;
deep in the pyramid (small `res(z)`) everything survives — which matches "the top
level tiles would otherwise feature all records." Tiny records aren't silently
dropped: they accumulate into the `__lod` overlay as **merged AABB rectangles**
with a count, e.g.

```
┌───────────────┐
│  ▢ (1,284 sub-records not rendered)
│   zoom in to resolve
└───────────────┘
```

Merging: bin omitted records into a coarse grid (say `TILE_PX / 16` cells),
union their AABBs per cell, and emit one annotated rectangle per non-empty cell.
This bounds overlay size and gives the user a spatial sense of "there's dense
stuff here, keep zooming." The overlay rectangle is drawn dashed/translucent and
its `<title>` / label carries the count. This both motivates the z-scale clipping
*and* makes it discoverable.

```rust
struct OmittedCell { bbox: WorldBox, count: u32 }

fn render_overlay(cells: &[OmittedCell], tile: WorldBox) -> String {
    // dashed rect + "(n sub-records not rendered)" label per cell
}
```

### 2.3 Eager vs on-demand generation

Mirror the existing `thumb.svg` **pull-through cache** (routes.rs ~403–462: try
cache key → on miss render → fire-and-forget `put_object` → return with
`cache-control: max-age=86400`):

- **Eager at ingest:** levels `0 .. K` (the zoomed-out levels — few tiles, high
  reuse) plus the manifest and `index.bin`. `K` chosen so eager tile count stays
  in the low thousands.
- **On demand:** deeper levels rendered the first time a tile is requested, then
  cached in S3. The index makes a single-tile render cheap (range query + a few
  hundred `<path>`s), so first-paint latency on deep zoom is acceptable.

Generation is CPU-bound and embarrassingly parallel across tiles → `rayon` over
the tile list, inside `tokio::task::spawn_blocking` so we don't stall the async
runtime (the upload handler already offloads parsing this way).

---

## Stage 3 — Manifest

The viewer fetches this **first**; it's the contract between tiler and viewer.

```jsonc
// GET /g/{id}/manifest.json
{
  "id": "a1b2c3",
  "units": { "nm_per_world": 1.0 },         // world unit = nm
  "extent_nm": { "minx": 0, "miny": 0, "maxx": 4200000, "maxy": 3100000 },
  "tile_px": 512,
  "zoom": { "min": 0, "max": 14, "res_nm_per_px": [ /* per level */ ] },
  "lod_min_px": 2.0,
  "layers": [
    {
      "key": "10/0",                         // "{layer}/{datatype}"
      "layer": 10, "datatype": 0,
      "name": "metal1",                      // from layer map if provided, else "L10/0"
      "count": 184223,
      "bbox_nm": { "minx": 0, "miny": 0, "maxx": 4200000, "maxy": 3100000 },
      "default_color": "#7aa2f7",
      "default_order": 3,                    // initial z-stack order (editable)
      "coverage_bits_url": "tiles/coverage/10_0.bin"   // optional: which tiles are non-empty
    }
    // …N layers… plus a synthetic { "key": "__lod", "name": "omitted (LOD)" }
  ],
  "generated": { "mode": "eager", "eager_max_z": 6 }
}
```

Layer **names/colors**: GDSII files don't self-describe layer semantics. Support
an optional sidecar **layer map** (the common `.layermap` / KLayout `.lyp`
conventions, or a small JSON) supplied at upload; absent that, fall back to
`L{layer}/{datatype}` names and a deterministic palette (hash the key → hue).
The viewer treats `name`, `color`, `order`, and `visible` as user-editable and
persists overrides (see Stage 4).

---

## Stage 4 — The WASM tile viewer (`gds-viewer`)

A **new Yew app** (new crate `crates/gds-viewer`, or a feature-gated mode in the
existing viewer). It is *not* the canvas BOM renderer; it's a tile compositor.
But it reuses the existing viewer's interaction core.

### 4.1 Reuse from `crates/viewer/src/render.rs`

- **`Transform`** (lines 14–34: `x, y, s, panx, pany, zoom`) — the pan/zoom
  state. We extend it with an integer **base level** selector so continuous
  `zoom` maps to a discrete tile level `z = clamp(round(log2(zoom)) + base, …)`
  plus a fractional CSS scale for smoothness between levels.
- **Wheel handler** (~298–329): cursor-centered zoom (`m = 1.1^(-Δ/40)`, pan
  compensation). Reuse verbatim; only the redraw target changes.
- **Pointer handlers** (~362–541): single-pointer pan, two-pointer pinch-zoom
  with centroid, right-click reset. Reuse verbatim.
- **`screen_to_board()` / hit-scan** (~1518–1701): repurpose for "what world
  coordinate / which cell is under the cursor" (future: click a cell → zoom to
  its bbox).

### 4.2 Tile composition

Rather than four fixed canvases, the viewer maintains a **stack of layer
surfaces** ordered by the user's layer order. Two viable render backends:

1. **DOM `<img>` tiles** (recommended v1): one absolutely-positioned
   `<div class="layer">` per visible layer, each containing the `<img
   src=".../{z}/{x}/{y}/{key}.svgz">` tiles for the current viewport, positioned
   by a CSS `transform: matrix(...)` derived from `Transform`. Browser handles
   SVG rasterization and caching; layer opacity/visibility/reorder are pure CSS.
   This is the classic slippy-map approach and the least code.
2. **Canvas `drawImage` of decoded SVGs**: more control (custom blending,
   highlight), more work. Defer to v2 if DOM compositing isn't enough.

Viewport tile set: from `Transform` + level `z`, compute the visible world AABB,
map to the `(x,y)` tile range, request those tiles per visible layer (skipping
empty ones per `coverage`), keep a small LRU of `HtmlImageElement`s. Prefetch one
ring beyond the viewport and the parent level for instant zoom-out.

### 4.3 Layer panel — toggle + **reorder** + style

The user requirement: *"confirm the layer names/order (should be resortable in
the UI) and render the svg tiles stacked."* The panel is a draggable list (HTML5
drag-and-drop or pointer-reorder) where each row is:

```
≡  ☑  ▢#7aa2f7  metal1 (10/0)         184,223   ─────●──  (opacity)
↑drag ↑vis ↑color ↑name(editable)     count     opacity slider
```

Reordering rewrites the CSS `z-index` / DOM order of the layer surfaces → the
SVG stack reorders live, no refetch. The `__lod` overlay is itself a toggleable
layer (default on), so users can hide the "(n not rendered)" annotations.

### 4.4 Persistence

Reuse the `state.rs` localStorage pattern (`read_storage`/`write_storage`/
`init_settings`, prefixed key). Persist per-design (`gds:{id}:`): layer order,
per-layer `{visible, color, opacity, name-override}`, last `Transform`
(pan/zoom), and `lod_overlay_visible`. Same mechanism the PCB viewer already uses
for `hiddenLayers`, `boardRotation`, etc.

---

## Server routes & storage

### Routes (add to `crates/server/src/routes.rs::router`)

| Method | Path | Handler | Notes |
|---|---|---|---|
| `GET` | `/g/{id}` | `get_gds_viewer` | serves the `gds-viewer` WASM shell (mirror `get_bom`, lines 347–366) |
| `GET` | `/g/{id}/manifest.json` | `get_gds_manifest` | from `gdsii/{id}/manifest.json` |
| `GET` | `/g/{id}/tiles/{z}/{x}/{y}/{key}` | `get_gds_tile` | pull-through cache; `key` = `"{layer}_{dt}"` or `"__lod"`; `.svgz` body, `Content-Encoding: gzip` |
| `GET` | `/g/{id}/raw.gds` | `get_gds_raw` | optional: download original |

`get_gds_tile` follows `get_thumb_svg` exactly (routes.rs ~403–462): validate id,
try `s3.get_object(tile_key)`, on miss load `index.bin` (+ raw for lazy AREF),
render the one tile via `spawn_blocking`, fire-and-forget `put_object`, return
with long `cache-control`. **`validate_id`** (already used by every handler)
guards against path traversal in `{id}`; `{z}/{x}/{y}/{key}` are parsed as typed
ints / a small regex, never interpolated raw.

### Upload path changes

`is_supported_format()` (routes.rs lines 100–102) currently **rejects**
`.gds`/`.gds2`. Change: route GDSII to the **tile ingest** instead of the
`PcbData` path.

```rust
// routes.rs upload(): branch on detected format
let format = pcb_extract::detect_format_with_content(path, &data)?;
if format == PcbFormat::Gdsii {
    // store raw, kick off stage 1+2 (eager portion) in spawn_blocking,
    // write manifest + index + eager tiles, return /g/{id}
} else {
    // existing PcbData path → /b/{id}
}
```

`detect_format_with_content` and `PcbFormat::Gdsii` already exist
(`main.rs` line 39, `routes.rs` line 267). The `pastebom.com-tester` discovery
agent already collects candidate `.gds` paths into `gdsii_files.json` — those are
ready-made ingest fixtures for load-testing.

### Storage keys (reuse `S3Client`: `get_object`/`put_object`/`list_objects`/`delete_object`, `crates/server/src/s3.rs` lines 62–197)

```
gdsii/{id}/raw.gds                          original upload
gdsii/{id}/index.bin                        serialized BspIndex (+ records)
gdsii/{id}/manifest.json                    layer/zoom/extent contract
gdsii/{id}/tiles/{z}/{x}/{y}/{layer}_{dt}.svgz   per-layer tile
gdsii/{id}/tiles/{z}/{x}/{y}/__lod.svgz          LOD overlay tile
gdsii/{id}/tiles/coverage/{layer}_{dt}.bin       optional non-empty-tile bitset
```

The dual-backend `S3Client` (S3 in prod, filesystem locally) works unchanged for
all of these.

---

## Reuse map (what's borrowed vs new)

| Concern | Reuse | New |
|---|---|---|
| GDSII byte/record reading | `gdsii.rs` `Record`/`RecordData`, record loop, excess-64 float (extract to `reader` submodule) | `PlacedRecord` stream sink (keeps datatype/text/box/node) |
| Instancing transforms | `flatten_structure` / `compute_structure_bbox` transform algebra (mirror/mag/angle/depth) | streaming consumer; lazy AREF variant for huge files |
| Extents | `BBox` (`types.rs` 73–143) for the float side | integer `WorldBox`; `BspIndex` spatial index |
| SVG `d`-strings | `thumbnail.rs` polygon/line/circle/arc/viewBox/stroke-clamp (extract to `pcb_extract::svg`) | per-tile `viewBox`, per-layer tile assembly, `.svgz`, LOD overlay |
| Pan / zoom / pinch | `render.rs` `Transform`, wheel & pointer handlers, `screen_to_board` | discrete level selection, tile-grid compositor, layer reorder UI |
| Settings persistence | `state.rs` localStorage read/write/init pattern | `gds:{id}:` keys (order, color, opacity, transform) |
| Tile serving | `get_thumb_svg` pull-through cache pattern; `S3Client` API; `validate_id` | `/g/{id}/...` routes, typed `{z}/{x}/{y}/{key}` parsing |
| Format detection | `detect_format_with_content`, `PcbFormat::Gdsii` | upload branch to tile ingest |
| Test inputs | `crates/pcb-extract/test-fixtures/*.gds`; `gdsii_files.json` corpus | golden-tile + index unit tests |

---

## Performance, limits, and failure modes

- **Memory**: eager mode resident set ≈ `records · sizeof(PlacedRecord)` + index
  arena. Budget it; above the threshold switch to lazy-AREF (§1.3). Persist
  `index.bin` so we never hold both the parse scratch and the index long-term.
- **Tile explosion**: deepest level can be millions of tiles. We **never**
  pre-generate deep levels — only eager levels `0..K` + on-demand. Coverage
  bitsets prevent requesting/storing empty tiles (sparse layouts are mostly
  empty at depth).
- **Pathological AREF** (e.g. `cols*rows` in the billions): keep the existing
  guard spirit (`MAX_AREF_INSTANCES`) but as a *lazy* gate — never materialize an
  array that a tile doesn't intersect.
- **Degenerate geometry** (self-intersecting boundaries, zero-width paths): SVG
  `fill-rule="evenodd"` + stroke clamp handle these the way the thumbnail already
  does; no special-casing in the pipeline.
- **Caps surfaced, not silent**: if any cap is hit (lazy gate refuses an array,
  a level is skipped), record it in the manifest (`"warnings": [...]`) and show a
  viewer banner — same principle the discovery agent follows (never silently
  truncate coverage).
- **Cache invalidation**: tiles are keyed by `id` only; a re-tile (algorithm
  change) bumps a `tiler_version` prefix in the key so old tiles age out
  naturally rather than needing a purge.

---

## Implementation plan (milestones / PRs)

Small, focused PRs (per the project commit style), each independently testable:

1. **Extract shared GDSII reader** — lift `Record`/`RecordData`/record-loop/float
   decode out of `gdsii.rs` into a `reader` submodule; `parse()` now calls it.
   Pure refactor, existing tests stay green.
2. **Extract `pcb_extract::svg`** — lift the `d`-string / viewBox / stroke
   helpers out of `thumbnail.rs`; thumbnail now calls them. Golden-SVG tests
   unchanged.
3. **`gdsii-tile` crate: record stream + `WorldBox`** — `PlacedRecord` stream
   over the shared reader, datatype/text/box/node preserved, per-record extent.
   Unit-test against `test-fixtures/*.gds`.
4. **`BspIndex`** — build + AABB range query, straddler handling, (de)serialize.
   Property tests: every record is returned by a query of its own bbox; query
   results ⊇ brute-force filter.
5. **Tile renderer + LOD** — `render_layer_tile`, `render_overlay`, the
   `px_area` cull, gzip, coverage bitsets. Golden-tile tests at a couple levels.
6. **Manifest + ingest wiring** — upload branch for `Gdsii`, eager generation in
   `spawn_blocking`, write manifest/index/eager tiles.
7. **Server routes** — `/g/{id}`, `/g/{id}/manifest.json`,
   `/g/{id}/tiles/...` with pull-through cache; reuse `validate_id`.
8. **`gds-viewer` WASM — compositor** — manifest fetch, tile grid, reuse
   `Transform`/handlers, DOM `<img>` stack.
9. **`gds-viewer` WASM — layer panel** — toggle/reorder/color/opacity + LOD
   overlay toggle + localStorage persistence.
10. **Lazy AREF expansion** (v2) — `InstancedRef` index node + per-tile
    expansion for chip-scale files.

---

## Open questions

- **Layer naming source of truth.** Ship a built-in heuristic palette for v1;
  add `.lyp`/`.layermap` sidecar upload in a follow-up? (Leaning: heuristic now,
  sidecar later.)
- **`TILE_PX` = 256 vs 512.** 512 → fewer requests, larger tiles; 256 → finer
  cache granularity. Start at 512, make it a manifest field so it's tunable
  without a viewer change.
- **`.svgz` vs raster tiles.** SVG keeps crispness at fractional zoom and is tiny
  for sparse layers, but dense metal-fill tiles could rival raster size; consider
  a per-tile fallback to PNG/WebP above an element-count threshold (manifest flag
  per tile). Defer measuring until stage 5.
- **Continuous vs snapped zoom.** Snap to discrete levels (simplest) or CSS-scale
  between levels for smoothness? Start snapped; add fractional scale if it feels
  janky.
- **One crate or two for the viewer.** New `crates/gds-viewer` (clean separation,
  another Trunk target) vs a mode inside `crates/viewer` (shared build, shared
  `Transform`). Leaning new crate to avoid bloating the BOM viewer bundle.
