# Packaging eda-view

The single `eda-view` package has an independent `0.1.0` version. The website
keeps its workspace release version. Parsers, renderers, and components are
selected through Cargo features; default features are empty.

Verify the package without uploading:

```sh
cargo publish --dry-run -p eda-view
cargo package --list -p eda-view
```

The archive includes source, documentation, licenses, and provenance. Test fixtures,
build output, and external golden captures remain outside the archive.
Feature-gated regression tests and golden examples run from the repository.

Rust code uses the bundled MIT license. The KiCad schema retains the original
kicad-tools copyright notice. NewStroke also includes its CC0 license and
documented source provenance. Actual registry publication is a separate action.
