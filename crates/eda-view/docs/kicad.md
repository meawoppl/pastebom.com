# kicad-schema

Reusable KiCad schematic and symbol-library models, extracted from KiCadmium's
`kct` crate. Depends only on `anyhow` and `serde`.

```rust
use eda_view::kicad::{schema::schematic::Schematic, sexp};

let tree = sexp::parse("(kicad_sch (version 20231120) (uuid \"example\"))")?;
let schematic = Schematic::new(tree, None)?;
assert!(schematic.symbols().is_empty());
# Ok::<(), anyhow::Error>(())
```

`sexp` preserves numeric spelling and quoted strings and provides tree queries,
edits, and serialization. `schema` provides symbols, embedded library graphics,
wires, junctions, labels, and hierarchical sheet references. The model preserves
unrecognized nodes in the underlying tree.

In-memory parsing and inspection support `wasm32-unknown-unknown`. Filesystem
loading/saving, library discovery, and identifier-generating editing helpers
require a native operating system. Callers supply linked sheet contents; the
parser does not load hierarchy files automatically.

Parsing rejects nesting beyond 128 levels and non-finite numeric atoms, including
floating-point overflow. Applications should impose their own input size limit.

See [NOTICE.md](NOTICE.md) for provenance and [LICENSE-kicad-tools](LICENSE-kicad-tools)
for the retained MIT license.
