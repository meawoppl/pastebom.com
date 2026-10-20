//! Interactive schematic viewer for `/b/{id}/schematic`: loads the stored
//! `vector_view::Scene` and shows one sheet at a time with selection,
//! reference search, net highlighting and layer toggles.
mod canvas;

use std::{
    collections::{HashMap, HashSet},
    rc::Rc,
};

use canvas::{Focus, SceneCanvas};
use vector_view::{
    style::{Highlight, LayerVisibility, Rgba, Theme},
    BBox, GroupId, GroupKind, ItemId, Prop, Role, Scene, SCENE_VERSION,
};
use wasm_bindgen_futures::spawn_local;
use web_sys::{HtmlInputElement, HtmlSelectElement};
use yew::prelude::*;

/// Canvas background (Tokyo Night); layer colours come from the scene.
const BACKGROUND: Rgba = [0x1a, 0x1b, 0x26, 255];
/// Selected items draw in orange while the rest dims.
const SELECTION: Rgba = [0xff, 0x9e, 0x64, 255];

fn main() {
    yew::Renderer::<App>::new().render();
}

/// Board id from `/b/{id}/schematic`.
fn board_id() -> Option<String> {
    let path = web_sys::window()?.location().pathname().ok()?;
    let mut parts = path.trim_matches('/').split('/');
    (parts.next()? == "b").then_some(())?;
    parts.next().map(str::to_owned)
}

/// `?ref=` from the page URL.
fn query_ref() -> Option<String> {
    let search = web_sys::window()?.location().search().ok()?;
    let params = web_sys::UrlSearchParams::new_with_str(&search).ok()?;
    params.get("ref").filter(|r| !r.is_empty())
}

/// Reflect the selected reference in the URL so the view can be shared.
fn set_query_ref(reference: Option<&str>) {
    let Some(window) = web_sys::window() else {
        return;
    };
    let Ok(path) = window.location().pathname() else {
        return;
    };
    let url = match reference {
        Some(r) => format!("{path}?ref={}", js_sys::encode_uri_component(r)),
        None => path,
    };
    if let Ok(history) = window.history() {
        let _ = history.replace_state_with_url(&wasm_bindgen::JsValue::NULL, "", Some(&url));
    }
}

fn prop<'a>(props: &'a [Prop], key: &str) -> Option<&'a str> {
    props
        .iter()
        .find(|p| p.key == key)
        .map(|p| p.value.as_str())
}

fn item_page(props: &[Prop]) -> &str {
    prop(props, "page").unwrap_or("/")
}

/// Sheets in traversal order: `(page path, file)` from `page:{path}:file`.
fn pages(scene: &Scene) -> Vec<(String, String)> {
    scene
        .meta
        .iter()
        .filter_map(|p| {
            let path = p.key.strip_prefix("page:")?.strip_suffix(":file")?;
            Some((path.to_owned(), p.value.clone()))
        })
        .collect()
}

/// The sheet's paper area (`page:{path}:size` = "w,h" in mm).
fn page_box(scene: &Scene, page: &str) -> Option<BBox> {
    let (w, h) = prop(&scene.meta, &format!("page:{page}:size"))?.split_once(',')?;
    Some(BBox {
        min: [0.0, 0.0],
        max: [w.parse().ok()?, h.parse().ok()?],
    })
}

/// Derived lookups built once per scene.
struct Index {
    pages: Vec<(String, String)>,
    /// Items hidden while a page is shown (everything on other pages).
    hidden: HashMap<String, Rc<HashSet<ItemId>>>,
    /// Symbol reference (upper-cased) to its groups, one per placed unit.
    refs: HashMap<String, Vec<GroupId>>,
    group_bbox: HashMap<GroupId, BBox>,
    warnings: Vec<String>,
}

impl Index {
    fn new(scene: &Scene) -> Self {
        let pages = pages(scene);
        let mut on_page: HashMap<&str, Vec<ItemId>> = HashMap::new();
        let mut group_bbox: HashMap<GroupId, BBox> = HashMap::new();
        for item in &scene.items {
            on_page
                .entry(item_page(&item.props))
                .or_default()
                .push(item.id);
            if let Some(g) = item.group {
                group_bbox
                    .entry(g)
                    .or_insert(BBox::EMPTY)
                    .union(&item.prim.bbox());
            }
        }
        let all: HashSet<ItemId> = scene.items.iter().map(|i| i.id).collect();
        let hidden = pages
            .iter()
            .map(|(page, _)| {
                let mut h = all.clone();
                for id in on_page.get(page.as_str()).into_iter().flatten() {
                    h.remove(id);
                }
                (page.clone(), Rc::new(h))
            })
            .collect();
        let mut refs: HashMap<String, Vec<GroupId>> = HashMap::new();
        for g in &scene.groups {
            if g.kind == GroupKind::Symbol && !g.label.is_empty() {
                refs.entry(g.label.to_uppercase()).or_default().push(g.id);
            }
        }
        let warnings = scene
            .meta
            .iter()
            .filter(|p| p.key == "warning")
            .map(|p| p.value.clone())
            .collect();
        Self {
            pages,
            hidden,
            refs,
            group_bbox,
            warnings,
        }
    }
}

#[derive(Clone, PartialEq)]
enum Selection {
    None,
    Item(ItemId),
    Net(u32),
}

#[derive(Clone)]
enum Load {
    Loading,
    Ready(Rc<Scene>, Rc<Index>),
    Error(String),
}

/// Board metadata relevant to the viewer chrome, from `/b/{id}/meta`.
#[derive(Clone, PartialEq)]
struct Meta {
    filename: String,
    has_pcb: bool,
}

fn parse_meta(v: &serde_json::Value) -> Meta {
    Meta {
        filename: v["filename"].as_str().unwrap_or_default().to_owned(),
        has_pcb: v["has_pcb"].as_bool().unwrap_or(true),
    }
}

async fn fetch_json(url: &str) -> Result<serde_json::Value, String> {
    let resp = gloo_net::http::Request::get(url)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.ok() {
        return Err(format!("{} ({})", resp.status_text(), resp.status()));
    }
    resp.json().await.map_err(|e| e.to_string())
}

async fn fetch_scene(id: &str) -> Result<Scene, String> {
    let value = fetch_json(&format!("/b/{id}/schematic.json")).await?;
    let scene: Scene = serde_json::from_value(value).map_err(|e| e.to_string())?;
    if scene.version != SCENE_VERSION {
        return Err(format!(
            "unsupported scene version {} (viewer expects {SCENE_VERSION})",
            scene.version
        ));
    }
    Ok(scene)
}

#[function_component(App)]
fn app() -> Html {
    let id = use_memo((), |_| board_id().unwrap_or_default());
    let load = use_state(|| Load::Loading);
    let meta = use_state(|| None::<Meta>);
    let page = use_state(|| "/".to_owned());
    let selection = use_state(|| Selection::None);
    let focus = use_state(|| None::<Focus>);
    let visibility = use_state(LayerVisibility::default);
    let show_layers = use_state(|| false);
    let search_error = use_state(|| false);

    // Select a symbol by reference: open its sheet and frame it.
    let goto_ref = {
        let load = load.clone();
        let page = page.clone();
        let selection = selection.clone();
        let focus = focus.clone();
        let search_error = search_error.clone();
        Callback::from(move |reference: String| {
            let Load::Ready(scene, index) = &*load else {
                return;
            };
            let Some(units) = index.refs.get(&reference.trim().to_uppercase()) else {
                search_error.set(true);
                return;
            };
            search_error.set(false);
            // Prefer a unit on the open sheet, else the first placed.
            let Some(group) = units
                .iter()
                .filter_map(|g| scene.group(*g))
                .find(|g| item_page(&g.props) == page.as_str())
                .or_else(|| units.first().and_then(|g| scene.group(*g)))
            else {
                return;
            };
            let gid = group.id;
            page.set(item_page(&group.props).to_owned());
            if let Some(item) = scene.items.iter().find(|i| i.group == Some(gid)) {
                selection.set(Selection::Item(item.id));
            }
            if let Some(bbox) = index.group_bbox.get(&gid) {
                let seq = focus.as_ref().map_or(0, |f| f.seq + 1);
                focus.set(Some(Focus { seq, bbox: *bbox }));
            }
            set_query_ref(Some(&group.label));
        })
    };

    {
        let id = id.clone();
        let load = load.clone();
        let meta = meta.clone();
        let page = page.clone();
        use_effect_with((), move |_| {
            spawn_local(async move {
                if let Ok(v) = fetch_json(&format!("/b/{id}/meta")).await {
                    meta.set(Some(parse_meta(&v)));
                }
                match fetch_scene(&id).await {
                    Ok(scene) => {
                        let index = Index::new(&scene);
                        if let Some((first, _)) = index.pages.first() {
                            page.set(first.clone());
                        }
                        load.set(Load::Ready(Rc::new(scene), Rc::new(index)));
                    }
                    Err(e) => load.set(Load::Error(e)),
                }
            });
        });
    }

    // Apply `?ref=` once the scene is ready.
    {
        let goto_ref = goto_ref.clone();
        let ready = matches!(&*load, Load::Ready(..));
        use_effect_with(ready, move |ready| {
            if *ready {
                if let Some(r) = query_ref() {
                    goto_ref.emit(r);
                }
            }
        });
    }

    let (scene, index) = match &*load {
        Load::Loading => return html! { <div class="status">{"Loading schematic…"}</div> },
        Load::Error(e) => {
            return html! { <div class="status error">{format!("Could not load schematic: {e}")}</div> }
        }
        Load::Ready(scene, index) => (scene.clone(), index.clone()),
    };

    let current = (*page).clone();
    let hidden = index
        .hidden
        .get(&current)
        .cloned()
        .unwrap_or_else(|| Rc::new(HashSet::new()));
    let fit_bbox = page_box(&scene, &current);

    let selected_item = match &*selection {
        Selection::Item(id) => scene.items.iter().find(|i| i.id == *id),
        _ => None,
    };
    let selected_group = selected_item
        .and_then(|i| i.group)
        .and_then(|g| scene.group(g));
    let highlight = match (&*selection, selected_group) {
        (Selection::Net(n), _) => Highlight::Net(*n),
        // Every unit of a multi-unit symbol lights up together.
        (Selection::Item(_), Some(g)) => Highlight::Group(
            index
                .refs
                .get(&g.label.to_uppercase())
                .filter(|_| g.kind == GroupKind::Symbol)
                .map_or_else(|| HashSet::from([g.id]), |u| u.iter().copied().collect()),
        ),
        (Selection::Item(id), None) => Highlight::Items(HashSet::from([*id])),
        (Selection::None, _) => Highlight::None,
    };

    let open_page = {
        let page = page.clone();
        let selection = selection.clone();
        let focus = focus.clone();
        Callback::from(move |p: String| {
            // The canvas remounts per sheet; a kept focus would replay there.
            focus.set(None);
            selection.set(Selection::None);
            set_query_ref(None);
            page.set(p);
        })
    };
    let on_select = {
        let selection = selection.clone();
        let scene = scene.clone();
        Callback::from(move |id: Option<ItemId>| {
            let reference = id
                .and_then(|id| scene.items.iter().find(|i| i.id == id))
                .and_then(|i| i.group)
                .and_then(|g| scene.group(g))
                .filter(|g| g.kind == GroupKind::Symbol)
                .map(|g| g.label.clone());
            set_query_ref(reference.as_deref());
            selection.set(id.map_or(Selection::None, Selection::Item));
        })
    };

    let page_picker = if index.pages.len() > 1 {
        let open_page = open_page.clone();
        let onchange = Callback::from(move |e: Event| {
            open_page.emit(e.target_unchecked_into::<HtmlSelectElement>().value())
        });
        html! {
            <select class="pages" aria-label="Sheet" {onchange}>
                { for index.pages.iter().enumerate().map(|(i, (p, file))| html! {
                    <option value={p.clone()} selected={*p == current}>
                        {format!("{}. {p} ({file})", i + 1)}
                    </option>
                }) }
            </select>
        }
    } else {
        Html::default()
    };

    let search = {
        let goto_ref = goto_ref.clone();
        let onkeydown = Callback::from(move |e: KeyboardEvent| {
            if e.key() == "Enter" {
                goto_ref.emit(e.target_unchecked_into::<HtmlInputElement>().value());
            }
        });
        let class = if *search_error {
            "search miss"
        } else {
            "search"
        };
        html! { <input {class} type="search" placeholder="Find reference (U1)" {onkeydown} /> }
    };

    let layers_toggle = {
        let show_layers = show_layers.clone();
        Callback::from(move |_| show_layers.set(!*show_layers))
    };
    let layers_panel = if *show_layers {
        html! {
            <div class="layers">
                { for scene.layers.iter().map(|layer| {
                    let visibility = visibility.clone();
                    let id = layer.id;
                    let checked = visibility.is_visible(layer);
                    let [r, g, b, _] = layer.color;
                    let onchange = Callback::from(move |e: Event| {
                        let mut v = (*visibility).clone();
                        v.set(id, e.target_unchecked_into::<HtmlInputElement>().checked());
                        visibility.set(v);
                    });
                    html! {
                        <label>
                            <input type="checkbox" {checked} {onchange} />
                            <span class="swatch" style={format!("background:rgb({r},{g},{b})")} />
                            {&layer.name}
                        </label>
                    }
                }) }
            </div>
        }
    } else {
        Html::default()
    };

    let pcb_link = match &*meta {
        Some(m) if !m.has_pcb => Html::default(),
        _ => {
            let href = match selected_group.filter(|g| g.kind == GroupKind::Symbol) {
                Some(g) => format!("/b/{id}#ref={}", js_sys::encode_uri_component(&g.label)),
                None => format!("/b/{id}"),
            };
            html! { <a class="button" {href}>{"PCB / BOM"}</a> }
        }
    };
    let title = meta
        .as_ref()
        .map(|m| m.filename.clone())
        .unwrap_or_else(|| "Schematic".into());

    let warnings = if index.warnings.is_empty() {
        Html::default()
    } else {
        html! {
            <div class="warnings">
                { for index.warnings.iter().map(|w| html! { <div>{w}</div> }) }
            </div>
        }
    };

    let properties = selected_item.map(|item| {
        let mut rows: Vec<(String, String)> = Vec::new();
        if let Some(g) = selected_group {
            rows.extend(g.props.iter().map(|p| (p.key.clone(), p.value.clone())));
        }
        for p in &item.props {
            if !rows.iter().any(|(k, _)| *k == p.key) {
                rows.push((p.key.clone(), p.value.clone()));
            }
        }
        rows.retain(|(k, _)| k != "uuid" && k != "at" && k != "rotation");
        rows.push(("role".into(), format!("{:?}", item.role)));
        let net = item.net.and_then(|n| scene.net(n)).map(|net| {
            let selection = selection.clone();
            let id = net.id;
            html! {
                <button onclick={Callback::from(move |_| selection.set(Selection::Net(id)))}>
                    {format!("Highlight net {}", net.name)}
                </button>
            }
        });
        let child = (item.role == Role::SheetFrame)
            .then_some(selected_group)
            .flatten()
            .and_then(|g| prop(&g.props, "child_page"))
            .map(|p| {
                let open_page = open_page.clone();
                let p = p.to_owned();
                html! { <button onclick={Callback::from(move |_| open_page.emit(p.clone()))}>{"Open sheet"}</button> }
            });
        let heading = selected_group
            .map(|g| g.label.clone())
            .unwrap_or_else(|| format!("{:?}", item.role));
        let close = {
            let selection = selection.clone();
            Callback::from(move |_| {
                set_query_ref(None);
                selection.set(Selection::None)
            })
        };
        html! {
            <aside class="props" aria-label="Selection">
                <div class="props-head">
                    <strong>{heading}</strong>
                    <button onclick={close}>{"×"}</button>
                </div>
                <dl>
                    { for rows.iter().map(|(k, v)| html! { <><dt>{k}</dt><dd>{v}</dd></> }) }
                </dl>
                <div class="props-actions">{child.unwrap_or_default()}{net.unwrap_or_default()}</div>
            </aside>
        }
    });

    // Parent sheet, for an "up" button on child pages.
    let parent = prop(&scene.meta, &format!("page:{current}:parent")).map(str::to_owned);
    let up = parent.map(|p| {
        let open_page = open_page.clone();
        html! { <button onclick={Callback::from(move |_| open_page.emit(p.clone()))}>{"↑ Parent"}</button> }
    });

    let theme = Theme {
        highlight: Some(SELECTION),
        ..Default::default()
    };

    html! {
        <div class="app">
            <header>
                <a class="brand" href="/">{"pastebom"}</a>
                <span class="title">{title}</span>
                {page_picker}
                {up.unwrap_or_default()}
                <span class="spacer" />
                {search}
                <button onclick={layers_toggle}>{"Layers"}</button>
                {pcb_link}
            </header>
            {warnings}
            <main>
                <SceneCanvas
                    key={current.clone()}
                    scene={scene.clone()}
                    {highlight}
                    visibility={(*visibility).clone()}
                    hidden_items={hidden}
                    {theme}
                    background={Some(BACKGROUND)}
                    {fit_bbox}
                    focus={*focus}
                    {on_select}
                />
                {layers_panel}
                {properties.unwrap_or_default()}
            </main>
        </div>
    }
}
