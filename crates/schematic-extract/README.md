# schematic-extract

Reusable, filesystem-free KiCad schematic rendering. Parses modern
`.kicad_sch` files and their embedded symbols into a `vector_view::Scene`.
No KiCad installation, browser, fonts, or network access is required.

```rust
let scene = schematic_extract::parse(source)?;
let svg = scene.to_svg(&schematic_extract::vector_view::svg::SvgOptions {
    background: Some([0x1a, 0x1b, 0x26, 255]),
    ..Default::default()
});
```

For hierarchy, call `parse_project(root, &HashMap<String, Vec<u8>>)`. Map keys
are project-relative paths using `/`. Child files resolve relative to their
parent, including `../` within the supplied project. Paths escaping the project
and recursive hierarchies return errors. Repeated sheet instances render
separately. Missing child files or embedded symbols produce `warning` scene
metadata so a standalone root upload can still be viewed. Limits are 64 levels,
512 pages, and 128 nested s-expressions.

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
are propagated for selection; full cross-sheet electrical connectivity and
implicit power connectivity are not inferred. Embedded raster images, custom
`.kicad_wks` templates, legacy `.sch` files, and external symbol libraries are
not supported. Sheet pins and decorative pin shapes currently use simplified
geometry. Instance-specific reference overrides in repeated-sheet projects
are not yet applied.

Native preview example:

```sh
cargo run -p schematic-extract --example render -- path/root.kicad_sch output.svg
```

KiCadmium can replace its backend-specific schematic renderer with
`parse_project`, retaining its project revision checks and supplying file
contents itself. Scene properties and layer IDs preserve its viewer contract.

## Provenance

Rendering modules were extracted from `meawoppl/agent-portal-plugins`,
`kicadmium/backend/src/{schematic_view,schematic_drawing,symbol_graphics}.rs`,
revision `5b9c3372482dbf52c3a971abc8c6ad8f8d586847` (MIT). Parsing models are in
`kicad-schema`, with the original kicad-tools notice retained there. The
NewStroke font and its provenance are retained in `kicad-strokes`.
