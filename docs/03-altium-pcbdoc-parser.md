# Work Unit 3: Altium .PcbDoc Parser

A Rust parser for Altium Designer `.PcbDoc` files, implemented as an additional format in the pcb-extract library (work unit 1). Outputs the same pcbdata JSON schema consumed by the viewer.

## Motivation

Altium Designer is the most widely used commercial PCB EDA tool. Supporting `.PcbDoc` import would make pastebom.com useful to a much larger audience. No existing open-source Rust implementation exists.

KiCad has a mature Altium importer written in C++ (~8000 lines across several files). This is the best reference for understanding the binary format and mapping Altium concepts to a generic PCB data model.

## File Format Overview

Altium `.PcbDoc` files are **OLE2 Compound File Binary (CFB)** containers — the same format as `.doc` and `.xls` files. Magic bytes: `D0 CF 11 E0 A1 B1 1A E1`.

Inside the CFB container is a directory tree of streams, each representing a category of PCB objects.

### Container Structure

```
Root Entry/
├── FileHeader/                  # File version info
├── Board6/
│   ├── Header                   # Record count (uint32)
│   └── Data                     # Board outline, stackup, layer config
├── Components6/
│   ├── Header
│   └── Data                     # Component placements
├── ComponentBodies6/
│   ├── Header
│   └── Data                     # 3D body definitions
├── Nets6/
│   ├── Header
│   └── Data                     # Net definitions
├── Classes6/
│   ├── Header
│   └── Data                     # Net classes, diff pairs
├── Rules6/
│   ├── Header
│   └── Data                     # Design rules
├── Tracks6/
│   ├── Header
│   └── Data                     # Track segments
├── Arcs6/
│   ├── Header
│   └── Data                     # Arc segments
├── Pads6/
│   ├── Header
│   └── Data                     # Pads (SMD + TH)
├── Vias6/
│   ├── Header
│   └── Data                     # Vias
├── Fills6/
│   ├── Header
│   └── Data                     # Solid fills
├── Regions6/
│   ├── Header
│   └── Data                     # Polygon regions / zone fills
├── Texts6/
│   ├── Header
│   └── Data                     # Text objects
├── Dimensions6/
│   ├── Header
│   └── Data                     # Dimension annotations
├── Polygons6/
│   ├── Header
│   └── Data                     # Polygon pour definitions
├── WideStrings6/
│   └── Data                     # UTF-16 string table
└── Models/
    ├── Header
    └── Data                     # 3D model data (compressed STEP)
```

### Stream Priority

Not all streams are needed for BOM generation. Required streams:

| Stream | Priority | Reason |
|--------|----------|--------|
| Board6 | Required | Board outline, layer definitions |
| Components6 | Required | Component placements (ref, value, footprint, position) |
| Nets6 | Required | Net names |
| Tracks6 | Required | Track segments (if include_tracks) |
| Arcs6 | Required | Arc segments |
| Pads6 | Required | Pad definitions |
| Vias6 | Required | Via definitions (if include_tracks) |
| Fills6 | Optional | Solid copper fills |
| Regions6 | Optional | Zone fills |
| Texts6 | Optional | Text objects (silkscreen, fab) |
| Polygons6 | Optional | Polygon pour outlines |
| WideStrings6 | Required | UTF-16 string references |
| Classes6 | Ignored | Not needed for BOM |
| Rules6 | Ignored | Not needed for BOM |
| Dimensions6 | Ignored | Not needed for BOM |
| ComponentBodies6 | Ignored | 3D data not needed |
| Models | Ignored | 3D data not needed |

## Record Encoding

Altium uses two encoding schemes depending on the stream.

### Text Property Records

Used by: Board6, Components6, Nets6, Classes6, Rules6, Polygons6, Dimensions6

Format:
```
[4 bytes: record length (little-endian uint32)]
[record_length bytes: pipe-delimited key=value pairs, null-terminated]
```

Example raw bytes (decoded to ASCII):
```
|RECORD=Board|LAYER=TOP|...|\0
```

Parsing:
1. Read 4-byte little-endian length
2. Read that many bytes as ASCII/Latin-1
3. Split on `|` to get key=value pairs
4. Parse values: integers, floats (Altium uses `.` decimal), booleans (`T`/`F`), strings

```rust
fn parse_text_record(data: &[u8]) -> Result<HashMap<String, String>> {
    let len = u32::from_le_bytes(data[0..4].try_into()?) as usize;
    let record = &data[4..4 + len];
    let text = std::str::from_utf8(record)?.trim_end_matches('\0');
    let mut props = HashMap::new();
    for pair in text.split('|').filter(|s| !s.is_empty()) {
        if let Some((key, value)) = pair.split_once('=') {
            props.insert(key.to_uppercase(), value.to_string());
        }
    }
    Ok(props)
}
```

### Binary Subrecord Format

Used by: Tracks6, Arcs6, Pads6, Vias6, Fills6, Regions6, Texts6, ComponentBodies6

Format:
```
[1 byte: record type tag]
[4 bytes: subrecord length (little-endian uint32)]
[subrecord_length bytes: binary data]
```

A single logical record may consist of multiple subrecords. For example, a Pad record has:
- Subrecord 0: pad name (variable length string)
- Subrecord 1: pad geometry (fixed layout binary)
- Subrecord 2: (optional) size-and-shape override data

```rust
fn parse_binary_subrecords(data: &[u8]) -> Result<Vec<(u8, &[u8])>> {
    let mut records = Vec::new();
    let mut offset = 0;
    while offset < data.len() {
        let record_type = data[offset];
        offset += 1;
        let len = u32::from_le_bytes(data[offset..offset + 4].try_into()?) as usize;
        offset += 4;
        records.push((record_type, &data[offset..offset + len]));
        offset += len;
    }
    Ok(records)
}
```

## Coordinate System

Altium stores coordinates as **32-bit signed integers in units of 1/10000 mil** (= 0.000254 mm = 254 nm).

Conversion to mm:
```rust
fn altium_to_mm(units: i32) -> f64 {
    units as f64 * 0.0000254
}
```

Altium Y-axis points **upward** (mathematical convention). KiCad/InteractiveHtmlBom Y-axis points **downward** (screen convention). Negate Y when converting:

```rust
fn convert_point(x: i32, y: i32) -> (f64, f64) {
    (altium_to_mm(x), -altium_to_mm(y))
}
```

Angles in Altium are stored as **64-bit IEEE 754 doubles** in degrees, counterclockwise from the positive X-axis.

## Binary Field Layouts

These byte-offset tables are derived from KiCad's `altium_parser_pcb.cpp` and cross-referenced with `altium2kicad/convertpcb.pl`.

### Track Record (Tracks6)

Single subrecord per track:

| Offset | Size | Type | Field |
|--------|------|------|-------|
| 0 | 1 | u8 | Layer ID |
| 1 | 2 | u16 | (reserved) |
| 3 | 2 | u16 | Net ID |
| 5 | 2 | u16 | (reserved) |
| 7 | 2 | u16 | Component ID (0xFFFF = free) |
| 9 | 4 | i32 | (reserved) |
| 13 | 4 | i32 | Start X |
| 17 | 4 | i32 | Start Y |
| 21 | 4 | i32 | End X |
| 25 | 4 | i32 | End Y |
| 29 | 4 | i32 | Width |

Total fixed size: 33 bytes (may vary by Altium version).

### Arc Record (Arcs6)

| Offset | Size | Type | Field |
|--------|------|------|-------|
| 0 | 1 | u8 | Layer ID |
| 1 | 2 | u16 | (reserved) |
| 3 | 2 | u16 | Net ID |
| 5 | 2 | u16 | (reserved) |
| 7 | 2 | u16 | Component ID |
| 9 | 4 | i32 | (reserved) |
| 13 | 4 | i32 | Center X |
| 17 | 4 | i32 | Center Y |
| 21 | 4 | i32 | Radius |
| 25 | 8 | f64 | Start Angle (degrees) |
| 33 | 8 | f64 | End Angle (degrees) |
| 41 | 4 | i32 | Width |

### Via Record (Vias6)

| Offset | Size | Type | Field |
|--------|------|------|-------|
| 0-12 | — | — | (header, reserved) |
| 13 | 4 | i32 | X position |
| 17 | 4 | i32 | Y position |
| 21 | 4 | i32 | Diameter |
| 25 | 4 | i32 | Hole size |
| 29 | 1 | u8 | Start layer |
| 30 | 1 | u8 | End layer |

### Pad Record (Pads6) — Multi-Subrecord

**Subrecord 0: Pad name**
Variable-length string (the pad designator, e.g., "1", "A1").

**Subrecord 1: Pad geometry** (at least 100+ bytes)

| Offset | Size | Type | Field |
|--------|------|------|-------|
| 0 | 1 | u8 | Layer ID |
| 1 | 6 | — | Flags |
| 7 | 2 | u16 | Net ID |
| 9 | 4 | — | (reserved) |
| 13 | 2 | u16 | Component ID |
| 15 | 4 | — | (reserved) |
| 19 | 4 | — | (reserved) |
| 23 | 4 | i32 | X position |
| 27 | 4 | i32 | Y position |
| 31 | 4 | i32 | Top X size |
| 35 | 4 | i32 | Top Y size |
| 39 | 4 | i32 | Mid X size |
| 43 | 4 | i32 | Mid Y size |
| 47 | 4 | i32 | Bottom X size |
| 51 | 4 | i32 | Bottom Y size |
| 55 | 4 | i32 | Hole size |
| 59 | 1 | u8 | Top shape (1=round, 2=rect, 3=octagonal, 9=roundrect) |
| 60 | 1 | u8 | Mid shape |
| 61 | 1 | u8 | Bottom shape |
| 62 | 8 | f64 | Rotation (degrees) |
| 70 | 1 | bool | Is plated |
| 71... | — | — | Paste/mask expansions, etc. |

**Subrecord 2: Size-and-Shape (optional)**
Contains per-layer shape overrides, corner radius, chamfer ratios. Present only when pads use roundrect or chamfered shapes.

| Offset | Size | Field |
|--------|------|-------|
| 0-28 | — | Per-layer shape overrides |
| 29+ | varies | Corner radius values per layer (4 bytes each) |
| ... | varies | Chamfer ratios per layer |

### Fill Record (Fills6)

| Offset | Size | Type | Field |
|--------|------|------|-------|
| 0 | 1 | u8 | Layer ID |
| 1-12 | — | — | Header fields |
| 13 | 4 | i32 | X1 (corner 1) |
| 17 | 4 | i32 | Y1 |
| 21 | 4 | i32 | X2 (corner 2) |
| 25 | 4 | i32 | Y2 |
| 29 | 8 | f64 | Rotation (degrees) |

### Text Record (Texts6)

**Subrecord 0: Text properties**

| Offset | Size | Type | Field |
|--------|------|------|-------|
| 0 | 1 | u8 | Layer ID |
| 1-12 | — | — | Header |
| 13 | 4 | i32 | X position |
| 17 | 4 | i32 | Y position |
| 21 | 4 | i32 | Height |
| 25 | 8 | f64 | Rotation |
| 33 | 1 | bool | Mirrored |
| 34 | 4 | i32 | Width (stroke thickness) |
| ... | — | — | Font, justification fields |

**Subrecord 1: Text string**
Variable-length text content. May reference WideStrings6 for Unicode text.

### Component Record (Components6) — Text Properties

Key fields:
```
RECORD=Component
PATTERN=<footprint name>
DESIGNITEMID=<library ref>
CURRENTPARTID=<part number>
SOURCEDESIGNATOR=<reference, e.g., "U1">
SOURCELIBRARY=<library name>
X=<x position in 1/10000 mil>
Y=<y position in 1/10000 mil>
ROTATION=<angle in degrees>
LAYER=<layer name>
```

Component records own child objects (tracks, pads, arcs, texts) via the Component ID field in those records. Component ID 0xFFFF means the object is free (not part of a component).

### Board Record (Board6) — Text Properties

Key fields:
```
RECORD=Board
SHEETHEIGHT=<height>
SHEETWIDTH=<width>
LAYERV7_<n>NAME=<name>
LAYERV7_<n>COPTHICK=<thickness>
LAYERV7_<n>MECHKIND=<mechanical layer kind>
V7_LAYERPAIRCOUNT=<count>
```

Also contains board outline vertices:
```
KIND=0 (board outline)
V7_LAYER=<edge cuts layer>
VCOUNT=<vertex count>
VX0=... VY0=... VX1=... VY1=...
```

## Layer System

Altium has three generations of layer IDs. All must be handled.

### V6 Layer IDs (8-bit, legacy)

| ID | Name |
|----|------|
| 1 | Top Copper |
| 2-30 | Inner Copper 1-29 |
| 31 | Keep Out |
| 32 | Bottom Copper |
| 33 | Top Overlay (Silkscreen) |
| 34 | Bottom Overlay (Silkscreen) |
| 35 | Top Paste |
| 36 | Bottom Paste |
| 37 | Top Solder Mask |
| 38 | Bottom Solder Mask |
| 39-54 | Internal Planes 1-16 |
| 55 | Drill Guide |
| 56 | Drill Drawing |
| 57-72 | Mechanical 1-16 |
| 74 | Multi-Layer (all copper) |

### V7 Layer IDs (32-bit)

Base `0x01000000`:
```
0x01000000 + V6_ID = V7 equivalent
```

Extended mechanical layers:
```
0x01020000 + N = Extended Mechanical N
```

### V8 Layer IDs (32-bit)

Additional base `0x01030000` for even more extended layers.

### Mapping to InteractiveHtmlBom Sides

| Altium Layer | pcbdata Side | Category |
|-------------|-------------|----------|
| Top Copper (1) | `"F"` | pads, tracks |
| Bottom Copper (32) | `"B"` | pads, tracks |
| Top Overlay (33) | `"F"` | silkscreen |
| Bottom Overlay (34) | `"B"` | silkscreen |
| Multi-Layer (74) | `"F"` + `"B"` | pads (both sides) |
| Mechanical (57+) | depends on `mechkind` | see below |

### Mechanical Layer Mapping

Altium's mechanical layers have a `mechkind` property (from Board6 stackup):

| mechkind | InteractiveHtmlBom Mapping |
|----------|--------------------------|
| `ASSEMBLY_TOP` | `"F"` fabrication |
| `ASSEMBLY_BOTTOM` | `"B"` fabrication |
| `COURTYARD_TOP` | `"F"` fabrication |
| `COURTYARD_BOTTOM` | `"B"` fabrication |
| Other / unknown | Ignored |

When `mechkind` is not set (older Altium files), mechanical layers are ignored.

## WideStrings6 — Unicode String Table

Some text records reference UTF-16 strings stored in a separate stream. The WideStrings6 stream contains:

```
[4 bytes: string count]
For each string:
  [4 bytes: string ID]
  [4 bytes: string length in UTF-16 code units]
  [N*2 bytes: UTF-16LE encoded string]
```

Text records with a `WIDESTRING_INDEX` property reference into this table.

## Parse Flow

Following KiCad's proven parse order:

```rust
fn parse_pcbdoc(path: &Path, opts: &ExtractOptions) -> Result<PcbData> {
    let mut cfb = cfb::CompoundFile::open(path)?;

    // 1. Parse string table first (needed by other streams)
    let wide_strings = parse_wide_strings(&mut cfb)?;

    // 2. Parse board config (layer definitions, outline)
    let board = parse_board6(&mut cfb)?;
    let layer_map = build_layer_map(&board);

    // 3. Parse components (need these before assigning child objects)
    let components = parse_components6(&mut cfb, &wide_strings)?;

    // 4. Parse nets
    let nets = parse_nets6(&mut cfb)?;

    // 5. Parse all geometry objects
    let tracks = parse_tracks6(&mut cfb)?;
    let arcs = parse_arcs6(&mut cfb)?;
    let pads = parse_pads6(&mut cfb)?;
    let vias = parse_vias6(&mut cfb)?;
    let fills = parse_fills6(&mut cfb)?;
    let regions = parse_regions6(&mut cfb)?;
    let texts = parse_texts6(&mut cfb, &wide_strings)?;

    // 6. Assign child objects to components via component_id
    let footprints = build_footprints(&components, &tracks, &arcs, &pads, &fills, &texts, &layer_map);

    // 7. Extract board edges from Board6 outline
    let edges = extract_board_edges(&board);
    let edges_bbox = compute_bbox(&edges);

    // 8. Separate drawings by layer (silkscreen, fabrication)
    let drawings = categorize_drawings(&tracks, &arcs, &fills, &texts, &layer_map);

    // 9. Build tracks/zones if requested
    let (track_data, zone_data) = if opts.include_tracks {
        build_tracks_and_zones(&tracks, &arcs, &vias, &regions, &fills, &nets, &layer_map)
    } else {
        (None, None)
    };

    // 10. Build net list if requested
    let net_names = if opts.include_nets {
        Some(nets.iter().map(|n| n.name.clone()).collect())
    } else {
        None
    };

    Ok(PcbData {
        edges_bbox,
        edges,
        drawings,
        footprints,
        metadata: extract_metadata(&board),
        tracks: track_data,
        zones: zone_data,
        nets: net_names,
        font_data: None,  // Altium doesn't use KiCad's stroke font
    })
}
```

## Building Footprints from Components

Altium doesn't store footprints as self-contained units like KiCad. Instead, component records define placements, and child objects (pads, tracks, arcs, etc.) reference their parent component via a `component_id` field.

To build InteractiveHtmlBom footprints:

1. For each component record, collect all child objects where `component_id == component_index`
2. Transform child object coordinates from absolute to component-relative
3. Build the pad array from Pads6 records belonging to this component
4. Build the drawings array from Tracks6/Arcs6/Texts6 on silkscreen/fab layers
5. Compute bounding box from all child objects
6. Emit a footprint with `ref`, `center`, `bbox`, `pads`, `drawings`, `layer`

```rust
fn build_footprints(
    components: &[AltiumComponent],
    tracks: &[AltiumTrack],
    arcs: &[AltiumArc],
    pads: &[AltiumPad],
    fills: &[AltiumFill],
    texts: &[AltiumText],
    layer_map: &LayerMap,
) -> Vec<Footprint> {
    components.iter().enumerate().map(|(idx, comp)| {
        let comp_pads: Vec<_> = pads.iter()
            .filter(|p| p.component_id == idx as u16)
            .map(|p| convert_pad(p, comp, layer_map))
            .collect();

        let comp_drawings: Vec<_> = tracks.iter()
            .filter(|t| t.component_id == idx as u16)
            .filter_map(|t| convert_track_drawing(t, comp, layer_map))
            .chain(arcs.iter()
                .filter(|a| a.component_id == idx as u16)
                .filter_map(|a| convert_arc_drawing(a, comp, layer_map)))
            .chain(texts.iter()
                .filter(|t| t.component_id == idx as u16)
                .filter_map(|t| convert_text_drawing(t, comp, layer_map)))
            .collect();

        let bbox = compute_footprint_bbox(&comp_pads, &comp_drawings, comp);

        Footprint {
            ref_: comp.designator.clone(),
            center: convert_point(comp.x, comp.y),
            bbox,
            pads: comp_pads,
            drawings: comp_drawings,
            layer: layer_map.side(comp.layer),
        }
    }).collect()
}
```

## Pad Shape Mapping

| Altium Shape ID | Altium Name | pcbdata Shape |
|----------------|-------------|---------------|
| 1 | Round | `"circle"` |
| 2 | Rectangular | `"rect"` |
| 3 | Octagonal | `"polygon"` (8-sided) |
| 9 | Rounded Rectangle | `"roundrect"` |

Altium octagonal pads have no direct pcbdata equivalent. Emit as a polygon with 8 vertices computed from the pad size and a 45-degree chamfer.

For rounded rectangle pads, the corner radius comes from the Size-and-Shape subrecord (subrecord 2). If absent, default to a small radius.

## Text Handling

Altium text is rendered as SVG paths in the pcbdata output (same as InteractiveHtmlBom's non-KiCad parsers). Since we don't have Altium's fonts, we need to:

1. Parse text position, size, rotation, mirror, justification
2. Emit as a stroke font text object (if we bundle a suitable font), or
3. Emit as positioned text metadata and let the viewer render it

For initial implementation, emit text as `{"svgpath": "...", "ref": 1}` placeholders. Full text-to-path rendering can be added later by bundling a stroke font and implementing glyph layout.

Pragmatic approach: for reference designators and values (the most important texts), use simple bounding-box rectangles as placeholders. The BOM table still shows the correct ref/value from component metadata.

## Board Outline Extraction

The Board6 record contains board outline vertices:

```
KIND=0
VCOUNT=N
VX0=... VY0=... VX1=... VY1=... SA0=... (start angle for arcs)
```

Each vertex pair defines a segment. If `SA` (start angle) is nonzero, the segment is an arc. Convert to pcbdata edge drawings:

- Straight segments → `{"type": "segment", "start": [x0,y0], "end": [x1,y1], "width": 0.05}`
- Arc segments → `{"type": "arc", ...}` with center computed from endpoints and angle

## Crate Dependencies

| Crate | Purpose | Version |
|-------|---------|---------|
| `cfb` | OLE2/CFB container parsing | 0.14+ |
| `nom` | Binary record parsing | 7.x |
| `encoding_rs` | Text encoding (Latin-1 to UTF-8) | latest |

The `cfb` crate provides:
```rust
use cfb::CompoundFile;

let mut file = CompoundFile::open("board.PcbDoc")?;

// List all streams
for entry in file.walk() {
    println!("{}: {} bytes", entry.path().display(), entry.len());
}

// Read a specific stream
let mut stream = file.open_stream("/Board6/Data")?;
let mut data = Vec::new();
stream.read_to_end(&mut data)?;
```

## Project Structure (within pcb-extract)

```
pcb-extract/src/parsers/
├── altium/
│   ├── mod.rs          # AltiumParser, public parse function
│   ├── cfb.rs          # CFB container opening, stream reading
│   ├── records.rs      # Text property + binary subrecord parsing
│   ├── board.rs        # Board6 parsing (outline, layers)
│   ├── components.rs   # Components6 parsing
│   ├── nets.rs         # Nets6 parsing
│   ├── tracks.rs       # Tracks6 binary parsing
│   ├── arcs.rs         # Arcs6 binary parsing
│   ├── pads.rs         # Pads6 binary parsing (multi-subrecord)
│   ├── vias.rs         # Vias6 binary parsing
│   ├── fills.rs        # Fills6 binary parsing
│   ├── regions.rs      # Regions6 binary parsing
│   ├── texts.rs        # Texts6 binary parsing + WideStrings6
│   ├── layers.rs       # V6/V7/V8 layer ID mapping
│   └── types.rs        # Altium-specific intermediate types
```

## Testing Strategy

### Test Fixtures

Altium .PcbDoc files for testing:

1. **Simple 2-layer board**: minimal board with a few components, tracks, vias
2. **Multi-layer board**: 4+ layer board with blind/buried vias
3. **Board with arcs**: curved tracks, arc board outline
4. **Complex pads**: rounded rect, octagonal, custom shapes
5. **Unicode text**: board with non-ASCII text (tests WideStrings6)

Sources for test files:
- KiCad's test suite: https://gitlab.com/kicad/code/kicad/-/tree/master/qa/data/pcbnew/plugins/altium
- Open-source hardware projects with Altium sources
- Manually created minimal .PcbDoc files

### Unit Tests

- CFB stream reading
- Text property record parsing
- Binary subrecord parsing for each object type
- Coordinate conversion (1/10000 mil → mm, Y-axis flip)
- Layer ID mapping (V6 → V7 → side)
- Pad shape conversion
- Board outline extraction (straight + arc segments)

### Cross-Validation

For boards that KiCad can import:
1. Import .PcbDoc in KiCad GUI → save as .kicad_pcb
2. Run pcb-extract on both the .PcbDoc and the .kicad_pcb
3. Compare pcbdata output — footprint positions, pad sizes, and board outline should match within floating-point tolerance

### Fuzzing

The binary parser should be fuzz-tested to ensure it doesn't panic on malformed input:
```rust
// Using cargo-fuzz
fuzz_target!(|data: &[u8]| {
    let _ = parse_track_record(data);
    let _ = parse_pad_record(data);
    let _ = parse_text_properties(data);
});
```

## Risks and Unknowns

1. **Altium version variations.** Binary field layouts may shift between Altium versions (14 through 24+). KiCad's parser handles this with version detection from FileHeader. We should do the same.

2. **Pad Size-and-Shape subrecord.** This subrecord has a complex, version-dependent layout. It's needed for roundrect corner radius and chamfer ratios. Initial implementation can default to approximate values and improve later.

3. **Region/zone fills.** Altium stores pre-filled zone polygons in Regions6, which is a complex format with multiple polygon outlines and holes. Getting this right is hard. Initial implementation can skip zones or emit simplified outlines.

4. **Text rendering.** Without Altium's proprietary fonts, text rendering will be approximate. Reference designators and values are available from component metadata, so the BOM table works regardless.

5. **Embedded models and compressed data.** Some Altium streams use zlib compression or contain embedded 3D models. These are not needed for BOM generation and can be safely ignored.

6. **OLE2 edge cases.** Very large .PcbDoc files may use the CFB format's extended allocation table. The `cfb` crate handles this, but it's a potential source of parsing failures on exotic files.

7. **Circuit Studio / Circuit Maker variants.** Files with `.CSPcbDoc` and `.CMPcbDoc` extensions use the same CFB format with minor differences. Supporting these is a stretch goal.

## Source References

| Resource | URL | Notes |
|----------|-----|-------|
| KiCad Altium plugin (C++) | https://gitlab.com/kicad/code/kicad/-/tree/master/pcbnew/plugins/altium | Primary reference for binary layouts |
| KiCad altium_parser_pcb.h | https://gitlab.com/kicad/code/kicad/-/blob/master/pcbnew/plugins/altium/altium_parser_pcb.h | Struct definitions |
| KiCad altium_parser_pcb.cpp | https://gitlab.com/kicad/code/kicad/-/blob/master/pcbnew/plugins/altium/altium_parser_pcb.cpp | Binary parsing implementation |
| KiCad altium_pcb.cpp | https://gitlab.com/kicad/code/kicad/-/blob/master/pcbnew/plugins/altium/altium_pcb.cpp | High-level import orchestration |
| KiCad altium_pcb.h | https://gitlab.com/kicad/code/kicad/-/blob/master/pcbnew/plugins/altium/altium_pcb.h | Class definitions |
| KiCad Altium dev docs | https://dev-docs.kicad.org/en/import-formats/altium/index.html | Best single-page reference |
| python-altium format.md | https://github.com/vadmium/python-altium/blob/master/format.md | Excellent format documentation |
| altium2kicad convertpcb.pl | https://github.com/thesourcerer8/altium2kicad/blob/master/convertpcb.pl | Byte-offset tables |
| altium Rust crate | https://crates.io/crates/altium | Alpha, no PcbDoc support (SchLib/SchDoc only) |
| cfb Rust crate | https://crates.io/crates/cfb | OLE2/CFB container parsing |
| KiCad test fixtures | https://gitlab.com/kicad/code/kicad/-/tree/master/qa/data/pcbnew/plugins/altium | Sample .PcbDoc files |
| OLE2/CFB specification | https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-cfb/ | Microsoft's CFB spec |
