# vector-view-yew

Yew (0.23, CSR) components for [`vector-view`](../vector-view) scenes. They are
shared by pastebom.com's schematic page and the kicadmium PCB workbench.

- **`VectorSceneCanvas`**: format-independent Canvas2D host for any `Scene`
  (PCB, schematic, footprint, symbol). It handles wheel/drag/pinch navigation
  with a multi-finger tap guard, tap picking, layer visibility, hidden items,
  compositing passes, overlays and outline items. It also supports mirroring,
  a dot grid, fitting (`fit_bbox`, `fit_fill`), camera restore and reporting
  (`initial_view`, `on_view`), `Fit`/`Reset` commands and `focus` requests.
  The current camera is published on the canvas as `data-view`, read with
  `view_of`, so overlays such as annotation tools can map CSS pixels to scene
  units.
- **`search`**: `find(scene, query)` matches component references, values and
  net names (exact match wins, otherwise a literal case-insensitive substring),
  plus a compact `ViewerSearch` field.
- **`SchematicView`**: shows one sheet at a time for scenes from
  [`schematic-extract`](../schematic-extract). It has:
  - a sheet picker in hierarchy order, a parent button, and "Open sheet" on
    sheet frames;
  - component/net search that jumps to the first sheet with a match, and
    Enter selects a reference;
  - selection that highlights every unit of a symbol, a properties panel, and
    net highlighting;
  - layer toggles and a producer warning banner;
  - a camera remembered per sheet.

  Replacing the `scene` prop (live refresh) keeps the open sheet and its camera.

## Host contract

`SchematicView` does no I/O. It never fetches and never touches the URL.
The host:

- passes `scene: Rc<Scene>` and replaces it on refresh;
- optionally controls the sheet with `page` + `on_page`;
- asks for a symbol with `reveal: Some(Reveal { seq, reference })` (deep
  links, cross-probe in) and hears selections via `on_select_ref`/`on_select`;
- gets a properties-panel action by setting `on_cross_probe` (label
  `cross_probe_label`);
- persists cameras with `on_view((page, view))` and `initial_view`;
- adds toolbar content as children.

The root element carries `data-schematic-page` with the open sheet path, next
to the canvas `data-view`. Annotation tools can record which sheet a region
belongs to.

Class names (`schematicv`, `schematicv-tools`, `schematicv-pages`,
`pcbv-props`, `viewer-search`, …) match kicadmium's stylesheet. Hosts without
one can inject `schematic::DEFAULT_CSS`.

```rust
use std::rc::Rc;
use vector_view_yew::{schematic::DEFAULT_CSS, Reveal, SchematicView};
use yew::prelude::*;

#[derive(Properties, PartialEq)]
struct Props { scene: Rc<vector_view_yew::vector_view::Scene> }

#[function_component(Page)]
fn page(props: &Props) -> Html {
    let on_probe = Callback::from(|reference: String| {
        // e.g. switch to the PCB tab and select `reference`
    });
    html! {
        <>
            <style>{DEFAULT_CSS}</style>
            <SchematicView scene={props.scene.clone()}
                reveal={Some(Reveal { seq: 0, reference: "U1".into() })}
                on_cross_probe={Some(on_probe)} />
        </>
    }
}
```

## Moving kicadmium onto it

- `frontend/src/vector_scene.rs` → `vector_view_yew::canvas` (same component
  name and props, plus `focus`).
- `view_input.rs` → `vector_view_yew::input`.
- `viewer_search.rs` → `vector_view_yew::search` (adds an optional `onsubmit`).
- `schematic_view.rs` keeps only fetching keyed on project/revision and
  renders `SchematicView`.

License: MIT. The canvas, input and search modules derive from kicadmium
(`meawoppl/agent-portal-plugins`, MIT).
