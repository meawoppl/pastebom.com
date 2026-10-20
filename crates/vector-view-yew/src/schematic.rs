//! Schematic viewer over a `vector_view::Scene` produced by `schematic-extract`
//! (or any producer following its page/symbol conventions).
//!
//! One sheet is shown at a time. The host owns all I/O: it supplies the scene
//! (replacing it on live refresh keeps the open sheet and its camera), and
//! receives page, selection, camera and cross-probe events through callbacks.
//! The component never fetches, and never reads or writes the URL.
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    rc::Rc,
};

use vector_view::{
    schematic,
    style::{Highlight, LayerVisibility, Rgba, Theme},
    view::View,
    BBox, GroupId, GroupKind, ItemId, NetId, Prop, Role, Scene,
};
use web_sys::{HtmlInputElement, HtmlSelectElement};
use yew::prelude::*;

use crate::canvas::{Focus, VectorSceneCanvas};
use crate::search::{find, ViewerSearch};

/// Canvas background matching the default (Tokyo Night) scene palette.
pub const DEFAULT_BACKGROUND: Rgba = [0x1a, 0x1b, 0x26, 255];
/// Selection colour; everything else dims.
pub const DEFAULT_SELECTION: Rgba = [0xff, 0x9e, 0x64, 255];
/// Root sheet path.
const ROOT: &str = "/";

/// Default styles for the classes this component renders. Hosts with their
/// own stylesheet (kicadmium) can skip it.
pub const DEFAULT_CSS: &str = include_str!("schematic.css");

/// Ask the view to select and frame a symbol by reference. Bump `seq` to
/// repeat a request for the same reference.
#[derive(Debug, Clone, PartialEq)]
pub struct Reveal {
    pub seq: u32,
    pub reference: String,
}

#[derive(Properties, PartialEq)]
pub struct SchematicViewProps {
    pub scene: Rc<Scene>,
    /// Sheet to show. When set the host controls the sheet (update it from
    /// `on_page`); unset, the view keeps its own, starting at the first sheet.
    #[prop_or_default]
    pub page: Option<AttrValue>,
    /// The open sheet changed (picker, parent button, "Open sheet", search
    /// jump, reveal).
    #[prop_or_default]
    pub on_page: Callback<String>,
    #[prop_or_default]
    pub reveal: Option<Reveal>,
    /// Selected symbol reference; `None` when cleared or not a symbol.
    #[prop_or_default]
    pub on_select_ref: Callback<Option<String>>,
    /// Selected item, of any kind.
    #[prop_or_default]
    pub on_select: Callback<Option<ItemId>>,
    /// When set, symbol properties offer this action (e.g. show on the PCB).
    #[prop_or_default]
    pub on_cross_probe: Option<Callback<String>>,
    #[prop_or(AttrValue::Static("Show on PCB"))]
    pub cross_probe_label: AttrValue,
    /// Camera for a sheet opened for the first time; later visits and scene
    /// refreshes keep the camera last used on that sheet.
    #[prop_or_default]
    pub initial_view: Option<(String, View)>,
    #[prop_or_default]
    pub on_view: Callback<(String, View)>,
    #[prop_or(true)]
    pub show_search: bool,
    #[prop_or(true)]
    pub show_layers: bool,
    #[prop_or_default]
    pub theme: Option<Theme>,
    #[prop_or(Some(DEFAULT_BACKGROUND))]
    pub background: Option<Rgba>,
    /// Extra toolbar content supplied by the host.
    #[prop_or_default]
    pub children: Html,
}

fn prop<'a>(props: &'a [Prop], key: &str) -> Option<&'a str> {
    props
        .iter()
        .find(|p| p.key == key)
        .map(|p| p.value.as_str())
}

fn page_of(props: &[Prop]) -> &str {
    prop(props, "page").unwrap_or(ROOT)
}

/// Lookups derived once per scene.
struct Index {
    pages: Vec<schematic::Page>,
    /// Items hidden while a sheet is open (everything on other sheets).
    hidden: HashMap<String, Rc<HashSet<ItemId>>>,
    /// Upper-cased symbol reference to its groups, one per placed unit.
    refs: HashMap<String, Vec<GroupId>>,
    group_bbox: HashMap<GroupId, BBox>,
    warnings: Vec<String>,
}

impl Index {
    fn new(scene: &Scene) -> Self {
        let pages = schematic::pages(scene);
        let hidden = pages
            .iter()
            .map(|page| {
                let shown = schematic::page_item_ids(scene, &page.path);
                let h = scene
                    .items
                    .iter()
                    .map(|i| i.id)
                    .filter(|id| !shown.contains(id))
                    .collect();
                (page.path.clone(), Rc::new(h))
            })
            .collect();
        let mut group_bbox: HashMap<GroupId, BBox> = HashMap::new();
        for item in &scene.items {
            if let Some(g) = item.group {
                group_bbox
                    .entry(g)
                    .or_insert(BBox::EMPTY)
                    .union(&item.prim.bbox());
            }
        }
        let mut refs: HashMap<String, Vec<GroupId>> = HashMap::new();
        for s in schematic::symbols(scene) {
            if !s.reference.is_empty() {
                refs.entry(s.reference.to_uppercase())
                    .or_default()
                    .push(s.group);
            }
        }
        let warnings = schematic::warnings(scene)
            .into_iter()
            .map(str::to_owned)
            .collect();
        Self {
            pages,
            hidden,
            refs,
            group_bbox,
            warnings,
        }
    }

    fn has_page(&self, page: &str) -> bool {
        self.pages.iter().any(|p| p.path == page)
    }

    fn first_page(&self) -> String {
        self.pages
            .first()
            .map_or_else(|| ROOT.to_owned(), |p| p.path.clone())
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Selection {
    None,
    Item(ItemId),
    Net(NetId),
}

#[function_component(SchematicView)]
pub fn schematic_view(props: &SchematicViewProps) -> Html {
    let index = use_memo(props.scene.clone(), |scene| Index::new(scene));
    let own_page = use_state(|| None::<String>);
    let selection = use_state(|| Selection::None);
    let query = use_state(String::new);
    let focus = use_state(|| None::<Focus>);
    let visibility = use_state(LayerVisibility::default);
    let layers_open = use_state(|| false);
    // Last camera per sheet; survives scene refreshes and sheet switches.
    let views = use_mut_ref(HashMap::<String, View>::new);

    // The open sheet: the host's, else ours, else the first; kept across
    // scene refreshes while it still exists.
    let wanted = props
        .page
        .as_ref()
        .map(|p| p.to_string())
        .or_else(|| (*own_page).clone());
    let current = wanted
        .filter(|p| index.has_page(p))
        .unwrap_or_else(|| index.first_page());

    let open_page = {
        let own_page = own_page.clone();
        let on_page = props.on_page.clone();
        let focus = focus.clone();
        Callback::from(move |p: String| {
            // The canvas remounts per sheet; a kept focus would replay there.
            focus.set(None);
            own_page.set(Some(p.clone()));
            on_page.emit(p);
        })
    };

    // A new scene invalidates item ids; keep the query, drop the selection.
    {
        let selection = selection.clone();
        use_effect_with(props.scene.clone(), move |_| selection.set(Selection::None));
    }

    let select_ref = {
        let scene = props.scene.clone();
        let index = index.clone();
        let selection = selection.clone();
        let focus = focus.clone();
        let open_page = open_page.clone();
        let on_select_ref = props.on_select_ref.clone();
        let current = current.clone();
        Rc::new(move |reference: &str| -> bool {
            let Some(units) = index.refs.get(&reference.trim().to_uppercase()) else {
                return false;
            };
            // Prefer a unit on the open sheet, else the first placed.
            let Some(group) = units
                .iter()
                .filter_map(|g| scene.group(*g))
                .find(|g| page_of(&g.props) == current)
                .or_else(|| units.first().and_then(|g| scene.group(*g)))
            else {
                return false;
            };
            let page = page_of(&group.props).to_owned();
            if page != current {
                open_page.emit(page);
            }
            if let Some(item) = scene.items.iter().find(|i| i.group == Some(group.id)) {
                selection.set(Selection::Item(item.id));
            }
            if let Some(bbox) = index.group_bbox.get(&group.id) {
                let seq = focus.as_ref().map_or(0, |f| f.seq.wrapping_add(1));
                focus.set(Some(Focus { seq, bbox: *bbox }));
            }
            on_select_ref.emit(Some(group.label.clone()));
            true
        })
    };

    {
        let select_ref = select_ref.clone();
        use_effect_with(props.reveal.clone(), move |reveal| {
            if let Some(r) = reveal {
                select_ref(&r.reference);
            }
        });
    }

    let scene = &props.scene;
    let found = find(scene, &query);
    let hidden = index
        .hidden
        .get(&current)
        .cloned()
        .unwrap_or_else(|| Rc::new(HashSet::new()));
    let visible_matches = found.items.iter().filter(|id| !hidden.contains(id)).count();
    let summary = if found.labels.is_empty() {
        "No matches".to_owned()
    } else {
        format!(
            "{} match{} · {} items on sheet",
            found.labels.len(),
            if found.labels.len() == 1 { "" } else { "es" },
            visible_matches
        )
    };

    let selected_item = match *selection {
        Selection::Item(id) => scene.items.iter().find(|i| i.id == id),
        _ => None,
    };
    let selected_group = selected_item
        .and_then(|i| i.group)
        .and_then(|g| scene.group(g));
    let searching = !query.trim().is_empty();
    let highlight = if searching {
        if found.items.is_empty() {
            Highlight::None
        } else {
            Highlight::Items(found.items.clone())
        }
    } else {
        match (*selection, selected_group) {
            (Selection::Net(n), _) => Highlight::Net(n),
            // Every unit of a multi-unit symbol lights up together.
            (Selection::Item(_), Some(g)) if g.kind == GroupKind::Symbol => Highlight::Group(
                index
                    .refs
                    .get(&g.label.to_uppercase())
                    .map_or_else(|| HashSet::from([g.id]), |u| u.iter().copied().collect()),
            ),
            (Selection::Item(_), Some(g)) => Highlight::Group(HashSet::from([g.id])),
            (Selection::Item(id), None) => Highlight::Items(HashSet::from([id])),
            (Selection::None, _) => Highlight::None,
        }
    };

    let on_query = {
        let query = query.clone();
        let selection = selection.clone();
        let scene = scene.clone();
        let current = current.clone();
        let open_page = open_page.clone();
        Callback::from(move |text: String| {
            // Jump to the first sheet with a match when the open one has none.
            let found = find(&scene, &text);
            let here = scene
                .items
                .iter()
                .any(|i| found.items.contains(&i.id) && page_of(&i.props) == current);
            if !here {
                if let Some(i) = scene.items.iter().find(|i| found.items.contains(&i.id)) {
                    open_page.emit(page_of(&i.props).to_owned());
                }
            }
            selection.set(Selection::None);
            query.set(text);
        })
    };
    let on_submit = {
        let select_ref = select_ref.clone();
        let query = query.clone();
        Callback::from(move |text: String| {
            if select_ref(&text) {
                query.set(String::new());
            }
        })
    };

    let on_canvas_select = {
        let selection = selection.clone();
        let scene = scene.clone();
        let on_select = props.on_select.clone();
        let on_select_ref = props.on_select_ref.clone();
        Callback::from(move |id: Option<ItemId>| {
            let reference = id
                .and_then(|id| scene.items.iter().find(|i| i.id == id))
                .and_then(|i| i.group)
                .and_then(|g| scene.group(g))
                .filter(|g| g.kind == GroupKind::Symbol)
                .map(|g| g.label.clone());
            selection.set(id.map_or(Selection::None, Selection::Item));
            on_select.emit(id);
            on_select_ref.emit(reference);
        })
    };
    let on_view = {
        let views = views.clone();
        let on_view = props.on_view.clone();
        let current = current.clone();
        Callback::from(move |view: View| {
            views.borrow_mut().insert(current.clone(), view);
            on_view.emit((current.clone(), view));
        })
    };
    let initial_view = remembered_view(&views, &current, props.initial_view.as_ref());

    let picker = (index.pages.len() > 1).then(|| {
        let open_page = open_page.clone();
        let onchange = Callback::from(move |e: Event| {
            open_page.emit(e.target_unchecked_into::<HtmlSelectElement>().value())
        });
        html! {
            <select class="schematicv-pages" aria-label="Schematic sheet" {onchange}>
                { for index.pages.iter().enumerate().map(|(i, p)| html! {
                    <option value={p.path.clone()} selected={p.path == current}>{format!("{}. {} ({})", i + 1, p.path, p.file)}</option>
                }) }
            </select>
        }
    });
    let parent = schematic::parent(scene, &current).map(|p| {
        let open_page = open_page.clone();
        let p = p.to_owned();
        html! {
            <button class="schematicv-parent" onclick={Callback::from(move |_| open_page.emit(p.clone()))}>{"↑ Parent"}</button>
        }
    });
    let search = props.show_search.then(|| {
        html! { <ViewerSearch query={(*query).clone()} onchange={on_query} {summary} onsubmit={on_submit} /> }
    });
    let layers_button = props.show_layers.then(|| {
        let layers_open = layers_open.clone();
        html! {
            <button class="schematicv-layers-toggle" aria-expanded={layers_open.to_string()}
                onclick={Callback::from(move |_| layers_open.set(!*layers_open))}>{"Layers"}</button>
        }
    });
    let layers_panel = (props.show_layers && *layers_open).then(|| {
        html! {
            <div class="schematicv-layers">
                { for scene.layers.iter().map(|layer| {
                    let checked = visibility.is_visible(layer);
                    let visibility = visibility.clone();
                    let id = layer.id;
                    let [r, g, b, _] = layer.color;
                    let onchange = Callback::from(move |e: Event| {
                        let mut v = (*visibility).clone();
                        v.set(id, e.target_unchecked_into::<HtmlInputElement>().checked());
                        visibility.set(v);
                    });
                    html! {
                        <label>
                            <input type="checkbox" {checked} {onchange} />
                            <span class="schematicv-swatch" style={format!("background:rgb({r},{g},{b})")} />
                            {&layer.name}
                        </label>
                    }
                }) }
            </div>
        }
    });
    let warnings = (!index.warnings.is_empty()).then(|| {
        html! {
            <div class="schematicv-warnings" role="status">
                { for index.warnings.iter().map(|w| html! { <div>{w}</div> }) }
            </div>
        }
    });

    let properties = selected_item.map(|item| {
        let close = {
            let selection = selection.clone();
            let on_select = props.on_select.clone();
            let on_select_ref = props.on_select_ref.clone();
            Callback::from(move |_| {
                selection.set(Selection::None);
                on_select.emit(None);
                on_select_ref.emit(None);
            })
        };
        let child = (item.role == Role::SheetFrame)
            .then_some(selected_group)
            .flatten()
            .and_then(|g| schematic::child_page(scene, g.id))
            .map(|p| {
                let open_page = open_page.clone();
                let p = p.to_owned();
                html! { <button onclick={Callback::from(move |_| open_page.emit(p.clone()))}>{"Open sheet"}</button> }
            });
        let net = item.net.and_then(|n| scene.net(n)).map(|net| {
            let selection = selection.clone();
            let id = net.id;
            html! {
                <button onclick={Callback::from(move |_| selection.set(Selection::Net(id)))}>
                    {format!("Highlight net {}", net.name)}
                </button>
            }
        });
        let probe = props
            .on_cross_probe
            .as_ref()
            .zip(selected_group.filter(|g| g.kind == GroupKind::Symbol))
            .map(|(cb, g)| {
                let cb = cb.clone();
                let reference = g.label.clone();
                html! { <button onclick={Callback::from(move |_| cb.emit(reference.clone()))}>{props.cross_probe_label.clone()}</button> }
            });
        html! {
            <aside class="pcbv-props" aria-label="Schematic selection properties">
                <div class="pcbv-props-head">
                    <strong>{selected_group.map_or_else(|| format!("Item {}", item.id), |g| g.label.clone())}</strong>
                    {child}{net}{probe}
                    <button onclick={close}>{"Close"}</button>
                </div>
                <dl>{ for property_rows(scene, item, selected_group).into_iter().map(|(k, v)| html! { <><dt>{k}</dt><dd>{v}</dd></> }) }</dl>
            </aside>
        }
    });

    let theme = props.theme.clone().unwrap_or_else(|| Theme {
        highlight: Some(DEFAULT_SELECTION),
        ..Default::default()
    });
    html! {
        <div class="schematicv" data-schematic-page={current.clone()}>
            <div class="schematicv-tools">
                {search}{picker}{parent}{layers_button}{props.children.clone()}
            </div>
            {warnings}
            <VectorSceneCanvas
                key={current.clone()}
                scene={scene.clone()}
                {highlight}
                on_select={on_canvas_select}
                visibility={(*visibility).clone()}
                hidden_items={(*hidden).clone()}
                fit_bbox={schematic::page_box(scene, &current)}
                {theme}
                background={props.background}
                {initial_view}
                {on_view}
                focus={*focus}
            />
            {layers_panel}
            {properties}
        </div>
    }
}

/// The camera to restore on `page`: the last one used there, else the host's
/// initial camera for that sheet.
fn remembered_view(
    views: &Rc<RefCell<HashMap<String, View>>>,
    page: &str,
    initial: Option<&(String, View)>,
) -> Option<View> {
    views
        .borrow()
        .get(page)
        .copied()
        .or_else(|| initial.filter(|(p, _)| p == page).map(|(_, v)| *v))
}

/// Symbol properties first, then the item's own, then role/net/group.
fn property_rows(
    scene: &Scene,
    item: &vector_view::Item,
    group: Option<&vector_view::Group>,
) -> Vec<(String, String)> {
    let mut rows: Vec<(String, String)> = Vec::new();
    for p in group
        .into_iter()
        .flat_map(|g| g.props.iter())
        .chain(&item.props)
    {
        if !rows.iter().any(|(k, _)| *k == p.key) {
            rows.push((p.key.clone(), p.value.clone()));
        }
    }
    rows.push(("role".into(), format!("{:?}", item.role)));
    if let Some(net) = item.net.and_then(|n| scene.net(n)) {
        rows.push(("net".into(), net.name.clone()));
    }
    if let Some(g) = group {
        rows.push(("group".into(), g.label.clone()));
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use vector_view::{Group, Item, Prim, SceneKind};

    fn item(id: ItemId, page: &str, group: Option<GroupId>) -> Item {
        Item {
            id,
            layer: 0,
            role: Role::SymbolBody,
            prim: Prim::Circle {
                center: [f64::from(id), 0.0],
                radius: 1.0,
                fill: false,
                stroke: 0.1,
            },
            net: None,
            group,
            props: vec![Prop::new("page", page)],
        }
    }

    fn symbol(id: GroupId, reference: &str, page: &str) -> Group {
        Group {
            id,
            kind: GroupKind::Symbol,
            label: reference.into(),
            props: vec![Prop::new("reference", reference), Prop::new("page", page)],
        }
    }

    fn two_sheet_scene() -> Scene {
        let mut s = Scene::new(SceneKind::Schematic, true);
        s.meta = vec![
            Prop::new("page:/:file", "root.kicad_sch"),
            Prop::new("page:/:size", "297,210"),
            Prop::new("page:/sub:file", "sub.kicad_sch"),
            Prop::new("page:/sub:parent", "/"),
            Prop::new("warning", "missing.kicad_sch not supplied"),
        ];
        // U1 has a unit on each sheet.
        s.groups = vec![
            symbol(1, "U1", "/"),
            symbol(2, "U1", "/sub"),
            symbol(3, "R1", "/sub"),
        ];
        s.items = vec![
            item(1, "/", Some(1)),
            item(2, "/sub", Some(2)),
            item(3, "/sub", Some(3)),
        ];
        s
    }

    #[test]
    fn index_splits_items_by_sheet() {
        let index = Index::new(&two_sheet_scene());
        let pages: Vec<_> = index
            .pages
            .iter()
            .map(|p| (p.path.as_str(), p.file.as_str()))
            .collect();
        assert_eq!(pages, [("/", "root.kicad_sch"), ("/sub", "sub.kicad_sch")]);
        assert_eq!(*index.hidden["/"], HashSet::from([2, 3]));
        assert_eq!(*index.hidden["/sub"], HashSet::from([1]));
        assert_eq!(index.warnings, ["missing.kicad_sch not supplied"]);
        assert_eq!(index.first_page(), "/");
        assert!(index.has_page("/sub") && !index.has_page("/nope"));
    }

    #[test]
    fn references_collect_every_unit_case_insensitively() {
        let index = Index::new(&two_sheet_scene());
        assert_eq!(index.refs["U1"], [1, 2]);
        assert_eq!(index.refs["R1"], [3]);
        assert!(!index.refs.contains_key("u1"));
        assert!(index.group_bbox.contains_key(&2));
    }

    #[test]
    fn remembered_camera_wins_over_initial() {
        let views = Rc::new(RefCell::new(HashMap::new()));
        let initial = (
            "/".to_owned(),
            View {
                scale: 2.0,
                ..View::default()
            },
        );
        assert_eq!(
            remembered_view(&views, "/", Some(&initial)),
            Some(initial.1)
        );
        assert_eq!(remembered_view(&views, "/sub", Some(&initial)), None);
        let used = View {
            scale: 7.0,
            ..View::default()
        };
        views.borrow_mut().insert("/".to_owned(), used);
        assert_eq!(remembered_view(&views, "/", Some(&initial)), Some(used));
    }

    #[test]
    fn property_rows_put_symbol_fields_first_without_duplicates() {
        let s = two_sheet_scene();
        let rows = property_rows(&s, &s.items[0], s.group(1));
        let keys: Vec<_> = rows.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, ["reference", "page", "role", "group"]);
    }
}
