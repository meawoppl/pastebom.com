# schematic-extract

Reusable, filesystem-free KiCad schematic rendering. Parses modern
`.kicad_sch` files and their embedded symbols into a `eda_view::vector::Scene`.
No KiCad installation, browser, fonts, or network access is required.

```rust
let scene = eda_view::schematic::parse(source)?;
let svg = scene.to_svg(&eda_view::vector::svg::SvgOptions {
    background: Some([0x1a, 0x1b, 0x26, 255]),
    ..Default::default()
});
```

For hierarchy, call `parse_project(root, &HashMap<String, Vec<u8>>)`. Map keys
are project-relative paths using `/`. Child files resolve relative to their
parent, including `../` within the supplied project. Escaping or recursive
child references are skipped with warning metadata while their sheet frames
and other reachable sheets remain visible. Invalid root paths still return
errors. Unrelated invalid file-map keys are ignored; normalized collisions
return errors only if the root or a reachable sheet uses that path. Syntax
and numeric-validation errors include the affected filename.
Repeated sheet instances render
separately. Missing child files or embedded symbols produce `warning` scene
metadata so a standalone root upload can still be viewed. Limits are 64 levels,
512 pages, and 128 nested s-expressions.

`find_root(&files)` selects the likely root: a schematic beside a matching
`.kicad_pro` or `.kicad_pcb` first, otherwise a schematic not referenced as a
child. Ties prefer shallower paths, then lexical order. Resource forks and
`__MACOSX` metadata are ignored. `parse_files(&files)` discovers and parses in
one call; use explicit `parse_project` when the caller knows the intended root.

Coordinates are millimetres, Y down. Sixteen layers distinguish wires, buses,
symbol outlines/fills, pins, pin text, fields, labels, power, no-connects, sheets,
notes, graphics, and page/title frames. Text uses NewStroke polylines, shared
with the Canvas and SVG renderers. Multi-unit and alternate-body symbols select
the matching body and common graphics.

## Consumer contract

- Every item has a `page` property. Items belonging to a symbol link to its
  group by `item.group`.
- Symbol groups have `GroupKind::Symbol`, label equal to their reference, and
  properties `reference`, `value`, `footprint`, `page`, `uuid`, `lib_id`, `unit`,
  `dnp`, `in_bom`, and `on_board`. Boolean values are `true` or `false` strings.
- Sheet groups have `filename`, `child_page`, `page`, and `uuid` properties.
- Scene metadata has `root`, `page:{page}:file`, `page:{page}:parent` (children),
  `page:{page}:paper`, and `page:{page}:size` (`width,height`). Root page is `/`.
- Select one page's item IDs with `SvgOptions.item_ids`; exporting all items
  overlays pages because each retains its native page coordinates.
- `vector-view` supplies Canvas rendering, hit testing, layer filtering, and
  SVG export with stable layer/item/group/net attributes.

The scene is a drawing model, not an electrical netlist. Wire endpoint labels
and hidden power-input symbol names are propagated for selection; full
cross-sheet electrical connectivity is not inferred. Embedded raster images, custom
`.kicad_wks` templates, legacy `.sch` files, and external symbol libraries are
not supported. Sheet pins and global-label outlines currently use simplified
geometry. Symbol instance paths select per-sheet reference and unit overrides.
Net identities are scoped to a page, so same-named local nets on different
sheets cannot accidentally highlight each other.

Native preview example:

```sh
cargo run -p eda-view --features schematic --example schematic_render -- path/root.kicad_sch output.svg
```

KiCadmium can replace its backend-specific schematic renderer with
`parse_project`, retaining its project revision checks and supplying file
contents itself. Scene properties and layer IDs preserve its viewer contract.

## Host configuration and reusable APIs

`parse_with`, `parse_project_with`, and `parse_files_with` take an
`&ExtractOptions` after the usual arguments. `Palette` exposes all sixteen
schematic RGBA colours and defaults to Tokyo Night. `frame_label: Some(...)`
adds a host label in the title block; the default is unbranded. KiCadmium uses
`Some("kicadmium".into())` to preserve its drawing frame.

`symbol_scene(&LibrarySymbol, &Palette)` renders an already-resolved library
model. `symbol_scene_for_unit(&SExp, unit, body_style, &Palette)` selects common
and unit/body-specific graphics from a resolved symbol expression before
flattening it. `LibrarySymbol`, `SExp`, and `kicad_schema` are re-exported so
consumers use the same model types. Resolve library inheritance before calling
the unit-specific expression API.

`eda_view::vector::schematic` supplies typed `Page` and `Symbol` records and
`pages`, `page_box`, `page_item_ids`, `symbols`, `parent`, `child_page`, and
`warnings` accessors. It is re-exported here as `schematic`. Browser consumers
can depend only on `vector-view`, without linking parsing or glyph data.
Page accessors preserve metadata traversal order; symbol records represent
placed units, so several records can share a reference.

See [the parity report](docs/kicadmium-parity.md) for the capture-corpus
assertions, documented rendering improvements, and model deduplication gaps.
See [packaging](docs/packaging.md) for dry-run verification without publishing.

## Provenance

Rendering modules were extracted from `meawoppl/agent-portal-plugins`,
`kicadmium/backend/src/{schematic_view,schematic_drawing,symbol_graphics}.rs`,
revision `5b9c3372482dbf52c3a971abc8c6ad8f8d586847` (MIT). Parsing models are in
`kicad-schema`, with the original kicad-tools notice retained there. The
NewStroke font and its provenance are retained in `kicad-strokes`.
