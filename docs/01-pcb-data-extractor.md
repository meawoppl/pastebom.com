# Work Unit 1: PCB Data Extractor

A Rust library and CLI tool that parses PCB design files and emits the InteractiveHtmlBom `pcbdata` JSON format. This replaces InteractiveHtmlBom's Python `ecad/` parsers with a standalone, dependency-free binary.

## Motivation

The current pastebom.com depends on InteractiveHtmlBom's Python parsers, which in turn depend on KiCad's `pcbnew` C++ module for `.kicad_pcb` support. This means:

- The Docker image must use `kicad/kicad:9.0` as a base (~1.5GB)
- KiCad's Python bindings are fragile across versions
- The parsers are tightly coupled to InteractiveHtmlBom's internal APIs
- No standalone CLI exists for extracting PCB data

A Rust extractor eliminates the KiCad system dependency by parsing `.kicad_pcb` files directly (they are S-expression text, not binary). It also provides a clean library boundary for the Axum backend.

## Supported Input Formats

| Format | Extension(s) | Source Format | Reference Parser |
|--------|-------------|---------------|-----------------|
| KiCad 5-9 | `.kicad_pcb` | S-expression text | `ecad/kicad.py` via `pcbnew` API |
| EasyEDA | `.json` | JSON | `ecad/easyeda.py` |
| Eagle/Fusion360 | `.brd`, `.fbrd` | XML | `ecad/fusion_eagle.py` |

## CLI Interface

```
pcb-extract <INPUT> [OPTIONS]

Arguments:
  <INPUT>  Path to PCB file (.kicad_pcb, .json, .brd, .fbrd)

Options:
  -o, --output <FILE>        Output JSON file (default: stdout)
  -f, --format <FORMAT>      Force input format: kicad|easyeda|eagle (default: auto-detect by extension)
      --include-tracks       Include copper tracks in output
      --include-nets         Include net names in output
      --pretty               Pretty-print JSON output
  -h, --help                 Print help
  -V, --version              Print version
```

## Library API

```rust
pub enum PcbFormat {
    KiCad,
    EasyEda,
    Eagle,
}

pub struct ExtractOptions {
    pub include_tracks: bool,
    pub include_nets: bool,
}

/// Auto-detect format from extension and parse.
pub fn extract(path: &Path, opts: &ExtractOptions) -> Result<PcbData, ExtractError>;

/// Parse from bytes with explicit format.
pub fn extract_bytes(data: &[u8], format: PcbFormat, opts: &ExtractOptions) -> Result<PcbData, ExtractError>;

/// The PcbData struct serializes to the exact InteractiveHtmlBom pcbdata JSON schema.
#[derive(Serialize)]
pub struct PcbData { /* see Output Schema below */ }
```

## Output Schema (pcbdata JSON)

The output must be byte-for-byte JSON-equivalent to what InteractiveHtmlBom produces. This is the contract that the viewer (work unit 2) consumes.

### Top-Level Object

```json
{
  "edges_bbox": { "minx": f64, "miny": f64, "maxx": f64, "maxy": f64 },
  "edges": [ Drawing, ... ],
  "drawings": {
    "silkscreen": { "F": [Drawing, ...], "B": [Drawing, ...] },
    "fabrication": { "F": [Drawing, ...], "B": [Drawing, ...] }
  },
  "footprints": [ Footprint, ... ],
  "metadata": { "title": str, "revision": str, "company": str, "date": str },
  "tracks": { "F": [Track, ...], "B": [Track, ...] },       // if include_tracks
  "zones": { "F": [Zone, ...], "B": [Zone, ...] },           // if include_tracks
  "nets": [ str, ... ],                                       // if include_nets
  "font_data": { char: { "w": f64, "l": [[[f64,f64], ...], ...] }, ... }  // KiCad only
}
```

All coordinates are in mm. Origin is top-left, Y grows downward. Floats are rounded to 6 decimal places in the final JSON.

### Drawing Types

Every drawing object has a `"type"` field:

**Segment:**
```json
{ "type": "segment", "start": [x, y], "end": [x, y], "width": f64 }
```

**Rectangle:**
```json
{ "type": "rect", "start": [x, y], "end": [x, y], "width": f64 }
```

**Circle:**
```json
{ "type": "circle", "start": [cx, cy], "radius": f64, "width": f64, "filled": 0|1 }
```

**Arc (angle form):**
```json
{ "type": "arc", "start": [cx, cy], "radius": f64, "startangle": f64, "endangle": f64, "width": f64 }
```

**Arc (SVG path form):**
```json
{ "type": "arc", "svgpath": "M ... A ...", "width": f64 }
```

**Bezier Curve:**
```json
{ "type": "curve", "start": [x, y], "end": [x, y], "cpa": [x, y], "cpb": [x, y], "width": f64 }
```

**Polygon (point array form):**
```json
{ "type": "polygon", "pos": [x, y], "angle": f64, "polygons": [[[x,y], ...], ...], "filled": 0|1, "width": f64 }
```

**Polygon (SVG path form):**
```json
{ "type": "polygon", "svgpath": "M ... Z", "filled": 0|1, "width": f64 }
```

**Text (SVG path form):**
```json
{ "svgpath": "M ...", "thickness": f64, "ref": 0|1, "val": 0|1 }
```

**Text (stroke font form, KiCad only):**
```json
{
  "pos": [x, y], "text": str, "height": f64, "width": f64,
  "justify": [h, v], "thickness": f64, "angle": f64,
  "attr": ["mirrored"|"italic"|"bold", ...]
}
```
- `justify`: `[-1|0|1, -1|0|1]` for left/center/right, top/center/bottom
- `ref`: 1 if reference designator text
- `val`: 1 if value text

### Footprint Object

```json
{
  "ref": "U1",
  "center": [x, y],
  "bbox": {
    "pos": [x, y],
    "relpos": [rx, ry],
    "size": [w, h],
    "angle": f64
  },
  "pads": [ Pad, ... ],
  "drawings": [ { "layer": "F"|"B", "drawing": Drawing } ],
  "layer": "F"|"B"
}
```

### Pad Object

```json
{
  "layers": ["F"] | ["F", "B"],
  "pos": [x, y],
  "size": [w, h],
  "shape": "rect"|"circle"|"oval"|"roundrect"|"chamfrect"|"custom",
  "type": "smd"|"th",
  "angle": f64,
  "pin1": 0|1,
  "net": "NET_NAME",
  "offset": [x, y],
  "radius": f64,                    // roundrect/chamfrect
  "chamfpos": int,                  // bitmask for chamfrect
  "chamfratio": f64,                // chamfrect
  "drillshape": "circle"|"oblong"|"rect",  // th only
  "drillsize": [w, h],             // th only
  "svgpath": str,                   // custom shape
  "polygons": [[[x,y], ...], ...]   // custom shape
}
```

### Track Object

```json
{ "start": [x, y], "end": [x, y], "width": f64, "net": str }
```

Via (rendered as track with drill):
```json
{ "start": [x, y], "end": [x, y], "width": f64, "net": str, "drillsize": f64 }
```

Arc track:
```json
{ "center": [x, y], "startangle": f64, "endangle": f64, "radius": f64, "width": f64, "net": str }
```

### Zone Object

```json
{ "polygons": [[[x,y], ...], ...], "width": f64, "net": str }
```

or SVG form:
```json
{ "svgpath": "M ... Z", "net": str, "fillrule": "evenodd"|"nonzero" }
```

### Font Data (KiCad only)

```json
{ "A": { "w": f64, "l": [[[x,y], [x,y], ...], ...] }, ... }
```

Each character maps to width `w` and array of polylines `l`.

## KiCad S-Expression Parser

The `.kicad_pcb` format is a Lisp-like S-expression. No C++ dependency is needed.

### Format Example

```
(kicad_pcb (version 20221018) (generator pcbnew)
  (general (thickness 1.6) (drawings 4) (tracks 127))
  (layers
    (0 "F.Cu" signal)
    (31 "B.Cu" signal)
    (36 "B.SilkS" user "B.Silkscreen"))
  (setup ...)
  (net 0 "")
  (net 1 "GND")
  (footprint "R_0402_1005Metric" (layer "F.Cu")
    (at 100.5 50.3 90)
    (fp_text reference "R1" (at 0 -1.2) (layer "F.SilkS")
      (effects (font (size 1 1) (thickness 0.15))))
    (pad "1" smd roundrect (at -0.51 0 90) (size 0.54 0.64)
      (layers "F.Cu" "F.Paste" "F.Mask")
      (roundrect_rratio 0.25)
      (net 1 "GND")))
  (segment (start 100.5 50.3) (end 105.2 50.3) (width 0.25) (layer "F.Cu") (net 1))
  (zone (net 1) (net_name "GND") (layer "F.Cu")
    (filled_polygon (layer "F.Cu")
      (pts (xy 0 0) (xy 100 0) (xy 100 80) (xy 0 80)))))
```

### Parsing Strategy

Use a recursive descent parser (or `nom` combinators) for the S-expression grammar:

```
sexpr     = '(' atom sexpr* ')'
atom      = string | number | symbol
string    = '"' [^"]* '"'
number    = [-]?[0-9]+[.[0-9]*]?
symbol    = [a-zA-Z_][a-zA-Z0-9_.-]*
```

Then walk the parsed tree to extract:
1. `layers` → build layer ID to name mapping
2. `net` declarations → net list
3. `footprint` nodes → footprints with pads and drawings
4. `gr_line`, `gr_arc`, `gr_circle`, `gr_rect`, `gr_curve`, `gr_poly` → board drawings, grouped by layer
5. `segment`, `arc` (top-level) → tracks
6. `zone` with `filled_polygon` → zones
7. `setup` → title block metadata

### Layer Mapping

KiCad layer names to InteractiveHtmlBom sides:

| KiCad Layer | Side | Category |
|-------------|------|----------|
| `F.Cu` | `"F"` | pads/tracks |
| `B.Cu` | `"B"` | pads/tracks |
| `F.SilkS` / `F.Silkscreen` | `"F"` | silkscreen |
| `B.SilkS` / `B.Silkscreen` | `"B"` | silkscreen |
| `F.Fab` / `F.Fabrication` | `"F"` | fabrication |
| `B.Fab` / `B.Fabrication` | `"B"` | fabrication |
| `Edge.Cuts` | — | board edges |

Note: KiCad 5 used short names (`F.SilkS`), KiCad 6+ uses long names (`F.Silkscreen`). Both must be handled.

### Coordinate Transforms

KiCad stores footprint-local coordinates relative to the footprint's `(at x y angle)`. Pads and drawings within a footprint must be rotated by the footprint angle and translated to absolute coordinates to match InteractiveHtmlBom output.

```
absolute_pos = footprint_pos + rotate(local_pos, footprint_angle)
```

### Text Rendering

InteractiveHtmlBom's KiCad parser uses `pcbnew`'s C++ text-to-path API to convert text to SVG paths. Without `pcbnew`, we have two options:

1. **Stroke font rendering (preferred):** Emit text as stroke font objects (`pos`, `text`, `height`, `width`, `justify`, etc.) and let the viewer render them using `font_data`. This is what InteractiveHtmlBom does for KiCad text when `kicad_text_formatting` is enabled.

2. **Glyph lookup:** Bundle KiCad's stroke font data (`newstroke` font from the `kicad-symbols` repo) and render text to SVG paths at extraction time.

Option 1 is simpler and matches the existing viewer behavior. The `font_data` object containing KiCad's `newstroke` glyph definitions should be emitted alongside pcbdata.

### KiCad Stroke Font Data

Source: https://gitlab.com/kicad/libraries/kicad-symbols/-/blob/master/kicad_sym (the `newstroke` font is actually compiled into KiCad).

The font is a Hershey-derived vector font. Each glyph is defined as a series of polylines with coordinates in a normalized space. The font data file can be bundled as a static asset in the Rust binary.

Canonical source for glyph data: https://gitlab.com/kicad/code/kicad/-/blob/master/common/font/newstroke_font.cpp

## EasyEDA Parser

EasyEDA exports PCB data as JSON. The parser reads this directly.

### Source Reference
- `InteractiveHtmlBom/ecad/easyeda.py` — the Python reference implementation

### Key Structures

EasyEDA JSON contains a `layers` array, a `shape` field per component, and component definitions with footprints. The parser must:

1. Parse canvas dimensions and coordinate system
2. Extract board outline from edge layer shapes
3. Parse component placements (PAD, TRACK, ARC, CIRCLE, RECT, SOLIDREGION, etc.)
4. Map EasyEDA layer IDs to front/back
5. Convert EasyEDA coordinate units (mils) to mm

### EasyEDA Layer Mapping

| EasyEDA Layer ID | InteractiveHtmlBom Side |
|-----------------|----------------------|
| 1 | `"F"` (top copper) |
| 2 | `"B"` (bottom copper) |
| 3 | `"F"` (top silkscreen) |
| 4 | `"B"` (bottom silkscreen) |
| 5 | `"F"` (top paste) |
| 6 | `"B"` (bottom paste) |
| 10 | board edge |

## Eagle/Fusion360 Parser

Eagle uses XML format (`.brd`). Fusion360 uses the same format with `.fbrd` extension.

### Source Reference
- `InteractiveHtmlBom/ecad/fusion_eagle.py` — the Python reference implementation

### Key Structures

```xml
<eagle>
  <drawing>
    <board>
      <plain> ... </plain>           <!-- Board outline, drawings -->
      <signals> ... </signals>       <!-- Nets with wires/vias -->
      <elements> ... </elements>     <!-- Component placements -->
      <libraries> ... </libraries>   <!-- Footprint definitions -->
    </board>
  </drawing>
</eagle>
```

### Parsing Strategy

Use `roxmltree` for XML parsing:

1. Parse `<libraries>` to build a footprint library (package name → pads + drawings)
2. Parse `<elements>` for component placements (x, y, rotation, mirror, reference, value)
3. Combine footprint library with placements to produce absolute-coordinate footprints
4. Parse `<plain>` for board edge (layer 20) and silkscreen/fabrication drawings
5. Parse `<signals>` for tracks and vias (if `include_tracks`)

### Eagle Layer Mapping

| Eagle Layer | InteractiveHtmlBom |
|------------|-------------------|
| 1 (Top) | `"F"` copper |
| 16 (Bottom) | `"B"` copper |
| 20 (Dimension) | edges |
| 21 (tPlace) | `"F"` silkscreen |
| 22 (bPlace) | `"B"` silkscreen |
| 25 (tNames) | `"F"` silkscreen text |
| 26 (bNames) | `"B"` silkscreen text |
| 27 (tValues) | `"F"` fabrication text |
| 28 (bValues) | `"B"` fabrication text |
| 51 (tDocu) | `"F"` fabrication |
| 52 (bDocu) | `"B"` fabrication |

### Eagle Coordinate System

Eagle uses mm or mils depending on the `<grid>` element. Coordinates must be normalized to mm. Rotation is specified as `R0`, `R90`, `R180`, `R270`, or `MR0` etc. for mirrored.

## BOM Generation

The extractor also needs to produce the `bom` field. This logic lives in `InteractiveHtmlBom/core/ibom.py`:

```json
{
  "bom": {
    "both": [ [["C1", 0], ["C2", 5]], ... ],
    "F": [ ... ],
    "B": [ ... ],
    "skipped": [3, 7],
    "fields": { "0": ["100nF", "C0402"], "1": ["10k", "R0402"] }
  }
}
```

### Grouping Logic

1. Collect all components with `ref`, `val`, `footprint`, `layer`, `extra_fields`
2. Skip components matching blacklist patterns or marked virtual/DNP
3. Group remaining components by `(val, footprint)` (or configured `group_fields`)
4. Sort groups by reference designator prefix using `component_sort_order`
5. Produce `both` (all), `F` (front only), `B` (back only) tables
6. Populate `fields` map: footprint index → field values in `show_fields` order
7. Populate `skipped` array with indices of skipped components

## Crate Dependencies

| Crate | Purpose | Version |
|-------|---------|---------|
| `serde` + `serde_json` | JSON serialization | latest |
| `nom` | S-expression parser for KiCad | 7.x |
| `roxmltree` | XML parser for Eagle | latest |
| `clap` | CLI argument parsing | 4.x |
| `thiserror` | Error types | latest |
| `log` + `env_logger` | Logging | latest |

## Project Structure

```
pcb-extract/
├── Cargo.toml
├── src/
│   ├── main.rs              # CLI entry point
│   ├── lib.rs               # Public API
│   ├── error.rs             # Error types
│   ├── types.rs             # PcbData, Footprint, Pad, Drawing, etc.
│   ├── bom.rs               # BOM generation logic
│   ├── parsers/
│   │   ├── mod.rs           # Format detection, parser trait
│   │   ├── kicad.rs         # KiCad S-expression parser
│   │   ├── kicad_sexpr.rs   # Low-level S-expr tokenizer/parser
│   │   ├── easyeda.rs       # EasyEDA JSON parser
│   │   └── eagle.rs         # Eagle XML parser
│   └── font/
│       ├── mod.rs           # Stroke font data types
│       └── newstroke.rs     # Compiled glyph data from KiCad
└── tests/
    ├── fixtures/            # Sample PCB files for each format
    ├── kicad_tests.rs
    ├── easyeda_tests.rs
    ├── eagle_tests.rs
    └── snapshot_tests.rs    # JSON snapshot comparison tests
```

## Testing Strategy

### Snapshot Tests

The primary validation strategy: parse a known input file, compare the output JSON against a "golden" snapshot captured from InteractiveHtmlBom's Python output for the same file.

```rust
#[test]
fn kicad_sample_matches_ibom_output() {
    let result = extract(Path::new("tests/fixtures/sample.kicad_pcb"), &default_opts());
    let expected: Value = serde_json::from_str(include_str!("fixtures/sample.pcbdata.json"));
    assert_json_eq!(result.to_value(), expected);
}
```

To generate golden snapshots:
1. Run InteractiveHtmlBom's Python parser on each fixture file
2. Capture the `pcbdata` dict as JSON
3. Store in `tests/fixtures/`

### Fixture Files

Collect sample PCB files covering:
- Simple 2-layer board (KiCad 5, 6, 7, 8, 9)
- Board with curved edges, arcs, beziers
- Board with custom pad shapes
- Board with zones and tracks
- EasyEDA export
- Eagle export
- Fusion360 `.fbrd` export

### Unit Tests

- S-expression tokenizer and parser
- Coordinate transforms (rotation, translation)
- Layer mapping
- BOM grouping and sorting
- Each pad shape type

## Risks and Unknowns

1. **KiCad version drift.** KiCad's S-expression format evolves across versions. Need to test against KiCad 5 through 9 files. The format is forward-compatible but new fields are added.

2. **Text rendering fidelity.** Without `pcbnew`'s C++ text layout, some text positioning may differ slightly (multi-line text, tab stops, overbar). Acceptable for initial release.

3. **Custom pad shapes.** KiCad 6+ supports custom pad shapes defined as polygons or anchored primitives. These produce `"shape": "custom"` with `svgpath` or `polygons`. Full fidelity requires implementing KiCad's pad primitive combination logic.

4. **EasyEDA format versions.** EasyEDA has evolved its JSON format. The reference parser handles specific known structures; new versions may introduce breaking changes.

5. **Zone tessellation.** InteractiveHtmlBom uses KiCad's `pcbnew` to get filled zone polygons (the result of DRC zone filling). Without `pcbnew`, we can only output zone outlines, not the filled copper. This is a known limitation — zones will appear as outlines rather than filled regions. This is acceptable because zone filling is a computationally expensive operation that depends on DRC rules.

## Source References

| Resource | URL |
|----------|-----|
| InteractiveHtmlBom repo | https://github.com/openscopeproject/InteractiveHtmlBom |
| InteractiveHtmlBom DATAFORMAT.md | https://github.com/openscopeproject/InteractiveHtmlBom/blob/master/DATAFORMAT.md |
| InteractiveHtmlBom ecad/kicad.py | https://github.com/openscopeproject/InteractiveHtmlBom/blob/master/InteractiveHtmlBom/ecad/kicad.py |
| InteractiveHtmlBom ecad/easyeda.py | https://github.com/openscopeproject/InteractiveHtmlBom/blob/master/InteractiveHtmlBom/ecad/easyeda.py |
| InteractiveHtmlBom ecad/fusion_eagle.py | https://github.com/openscopeproject/InteractiveHtmlBom/blob/master/InteractiveHtmlBom/ecad/fusion_eagle.py |
| InteractiveHtmlBom ecad/common.py | https://github.com/openscopeproject/InteractiveHtmlBom/blob/master/InteractiveHtmlBom/ecad/common.py |
| InteractiveHtmlBom core/ibom.py | https://github.com/openscopeproject/InteractiveHtmlBom/blob/master/InteractiveHtmlBom/core/ibom.py |
| InteractiveHtmlBom generic JSON schema | https://github.com/openscopeproject/InteractiveHtmlBom/blob/master/InteractiveHtmlBom/ecad/schema/genericjsonpcbdata_v1.schema |
| KiCad S-expression file format docs | https://dev-docs.kicad.org/en/file-formats/sexpr-intro/ |
| KiCad PCB file format | https://dev-docs.kicad.org/en/file-formats/sexpr-pcb/ |
| KiCad newstroke font source | https://gitlab.com/kicad/code/kicad/-/blob/master/common/font/newstroke_font.cpp |
| Eagle XML file format | https://web.archive.org/web/2024/https://www.autodesk.com/eagle-library/ |
| nom crate | https://docs.rs/nom/latest |
| roxmltree crate | https://docs.rs/roxmltree/latest |
