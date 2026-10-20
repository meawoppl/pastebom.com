# KiCadmium parity and model reuse

## Capture and reproduction

The 2026-10-20 corpus contains 32 projects: seven bp-test boards and 25 KiCadmium
fixture roots, captured from installed backend revision `25ed8137`.
`MANIFEST.json` records source paths and response hashes. `LIBRARY_SOURCES.json`
records the exact source endpoint content used for the library previews.
The external corpus is not included in these packages.

```sh
cargo run -p schematic-extract --example golden_scenes -- CORPUS OUTPUT
python3 crates/schematic-extract/tests/compare_golden.py CORPUS OUTPUT
```

The renderer supplies `frame_label = "kicadmium"`. The comparison fails on
unclassified deltas. Numeric geometry comparisons allow 1e-9 mm absolute error
to accommodate floating-point serialization; library previews compare raw bytes.

## Structural assertions

All 32 schematic scenes pass these assertions:

- Scene version, kind, axis direction, and all 16 layer definitions match.
- All 2,226 existing groups retain their exact numeric IDs, kinds, labels,
  and every original property value. Every symbol also supplies reference,
  value, footprint, lib_id, unit, dnp, in_bom, on_board, page, and uuid with
  validated types. Reference equals the group label.
- All 29,780 existing items preserve their role, page/UUID identity, symbol
  group membership, resolved net name, and original property values.
- Item/group/net IDs are unique and every membership points to an existing
  record. Each net ID belongs to one page. There are 375 item net-ID remaps
  from page scoping, with zero net-name changes.
- Every original page metadata key/value and root path is retained. The host
  owns `revision` and adds its current content hash after extraction.
- Original title-block text is retained. Only five sheet-number labels change
  to reflect two correctly rendered repeated-sheet instances; their values
  are asserted against traversal order and the actual page count.
- All 73 captured library-symbol previews are byte-for-byte identical.

The shared library does not produce PCB or footprint scenes. Those producers
are unchanged by this extraction; byte parity of their adapted backend endpoints
must be checked by the KiCadmium integration against the captured PCB/footprint
goldens, rather than inferred from schematic tests.

## Rendering deltas

| Class | Count | Meaning |
|---|---:|---|
| Unchanged primitive geometry | 26,329 | Existing items, including all unchanged frame text |
| Text effects | 3,440 | Source font size, justification, and rotation are honored instead of fixed centered text |
| Equivalent polygon winding | 6 | Same closed polygon edges; all rectangle corners transformed independently |
| Repeated-sheet numbering | 5 | Title-block sheet number/total now includes every instance |
| Added sheet fields | 23 | Visible sheet names and filenames |
| Added sheet pins | 7 | Hierarchical sheet connection geometry |
| Added sheet-pin labels | 7 | Text belonging to those pins |
| Added repeated-sheet pages/items | 2 / 99 | Instances previously omitted by file-level deduplication |

Added geometry on existing pages is asserted to belong to sheet groups.
No existing item is removed. Rendering deltas preserve the semantic contract;
they do not require a compatibility rendering mode.

The comparison exposed an upstream power-symbol naming change made after the
initial extraction. Hidden `power_in` pins now seed net names; `PWR_FLAG` does
not. Conflicting labels preserve KiCadmium's ordered wire propagation. In
`fx-board05_sync_drift--bldc_controller`, this retains `GND` instead of the
different `ISENSE_A-` result from unordered propagation. An ordered affected-edge
queue preserves this behavior without repeatedly scanning every wire.

## Model deduplication audit

Current KiCadmium source audited at `d75a64ee6003df62c428ec96c6b6c6dc897d6b5d`.
The schematic, library, symbol, wire, and label modules expose all public model
names needed by `backend/src/{schematic_view,schematic_drawing,symbol_graphics,
library_scene}.rs`. Rendering can use `schematic-extract` directly, retaining
host filesystem safety, revision retries, and HTTP state. The shared
`kicad-strokes` also covers PCB text callers, allowing the duplicate copy to go.

Broader kct replacement has these gaps:

- kct's BOM, hierarchy, hierarchy validation, field geometry, instances,
  physical identity, and PCB modules are outside `kicad-schema`.
- kct BOM/hierarchy/field-geometry modules access extracted `pub(crate)`
  helpers; editing and validation also access private parser tokens. They need
  small public operations or local adapters before kct reexports shared models.
- Backend library inventory uses its separate `crate::sexp::Sexp` parser.
  Its lists retain the head atom, unlike the shared `SExp`; this is not a
  drop-in type alias and needs an independent inventory migration.
- Backend BOM export uses `kicad-cli sch export bom`; missing kct BOM models
  do not block that current backend path.

Migrate rendering imports together, then make kct reexport the shared parser
and five model modules while keeping automation, PCB, hierarchy, and BOM logic
in kct. This avoids two incompatible SExp/model types without expanding the
reusable crate into the entire automation application.
