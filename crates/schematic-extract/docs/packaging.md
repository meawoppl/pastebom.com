# Packaging the shared libraries

`vector-view`, `kicad-schema`, `kicad-strokes`, and `schematic-extract` have
independent `0.1.0` package versions. The website keeps its workspace release
version. Path dependencies also specify registry versions.

With a Cargo version supporting multi-package publishing, verify all unpublished
siblings together, without uploading:

```sh
cargo publish --dry-run -p vector-view -p kicad-schema -p kicad-strokes -p schematic-extract
```

Cargo stages sibling packages in a temporary registry and checks their packaged
contents. Do not add `--workspace`: that also selects unrelated website crates.
For the reusable Yew frontend, add `-p vector-view-yew` after integrating it.

Inspect file lists with `cargo package --list -p NAME`. The packages explicitly
include source, README, licenses, provenance, docs, examples, and regression
tests. No build output or external golden capture data belongs in the archives.

Rust code uses the bundled MIT license. kicad-schema retains the original
kicad-tools copyright notice. kicad-strokes also includes the NewStroke CC0
license and its documented source provenance. Actual registry publication is
a separate action; these commands only validate readiness.
