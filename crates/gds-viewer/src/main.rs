mod manifest;
mod state;
mod transform;

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use wasm_bindgen::JsCast;
use web_sys::{HtmlElement, HtmlInputElement};
use yew::prelude::*;

use manifest::{Layer, Manifest};
use state::{init_view_state, storage_prefix, write_view_state, LayerState, ViewState};
use transform::{Pyramid, Transform};

fn main() {
    wasm_logger::init(wasm_logger::Config::default());
    yew::Renderer::<App>::new().render();
}

/// Parse the board id from a `/g/{id}` (optionally `/g/{id}/...`) path.
fn id_from_path(pathname: &str) -> Option<String> {
    let mut parts = pathname.trim_start_matches('/').split('/');
    if parts.next()? != "g" {
        return None;
    }
    let id = parts.next()?;
    if id.is_empty() {
        None
    } else {
        Some(id.to_string())
    }
}

/// Imperative pan/zoom state held outside Yew so high-frequency pointer/wheel
/// events mutate it without a re-render per event; we trigger redraws via a
/// counter when the visible tile set may have changed.
struct Interaction {
    transform: Transform,
    pointers: HashMap<i32, PointerState>,
    view_w: f64,
    view_h: f64,
}

struct PointerState {
    last_x: f64,
    last_y: f64,
}

#[function_component(App)]
fn app() -> Html {
    let manifest = use_state(|| None::<Rc<Manifest>>);
    let pyramid = use_state(|| None::<Rc<Pyramid>>);
    let view = use_state(|| None::<ViewState>);
    let prefix = use_state(String::new);
    let error = use_state(|| None::<String>);
    // Bumped to force a re-render of the tile grid after pan/zoom.
    let redraw = use_state(|| 0u64);
    let interaction: UseStateHandle<Rc<RefCell<Interaction>>> = use_state(|| {
        Rc::new(RefCell::new(Interaction {
            transform: Transform::default(),
            pointers: HashMap::new(),
            view_w: 1.0,
            view_h: 1.0,
        }))
    });
    // Index of the layer row currently being dragged (HTML5 DnD reorder).
    let drag_src: UseStateHandle<Option<usize>> = use_state(|| None);
    // Keys of tiles whose image has finished loading; drives the fade-in so a
    // tile only becomes visible once its pixels arrive, not when it mounts.
    let loaded: UseStateHandle<Rc<RefCell<HashSet<String>>>> =
        use_state(|| Rc::new(RefCell::new(HashSet::new())));

    // ── Fetch manifest once ─────────────────────────────────────────
    {
        let manifest = manifest.clone();
        let pyramid = pyramid.clone();
        let view = view.clone();
        let prefix = prefix.clone();
        let error = error.clone();
        let interaction = interaction.clone();
        let redraw = redraw.clone();
        use_effect_with((), move |_| {
            wasm_bindgen_futures::spawn_local(async move {
                let window = web_sys::window().unwrap();
                let pathname = window.location().pathname().unwrap_or_default();
                let Some(id) = id_from_path(&pathname) else {
                    error.set(Some("No board id in URL".to_string()));
                    return;
                };
                let url = format!("/g/{}/manifest.json", id);
                match gloo::net::http::Request::get(&url).send().await {
                    Ok(resp) if resp.ok() => match resp.text().await {
                        Ok(text) => match serde_json::from_str::<Manifest>(&text) {
                            Ok(m) => {
                                let pfx = storage_prefix(&m.id);
                                let vs = init_view_state(&m, &pfx);
                                interaction.borrow_mut().transform = vs.transform();
                                let pyr = Pyramid::from_manifest(&m);
                                prefix.set(pfx);
                                view.set(Some(vs));
                                pyramid.set(Some(Rc::new(pyr)));
                                manifest.set(Some(Rc::new(m)));
                                redraw.set(1);
                            }
                            Err(e) => error.set(Some(format!("Bad manifest: {}", e))),
                        },
                        Err(e) => error.set(Some(format!("Read error: {}", e))),
                    },
                    Ok(resp) => error.set(Some(format!("Manifest not found ({})", resp.status()))),
                    Err(e) => error.set(Some(format!("Network error: {}", e))),
                }
            });
            || ()
        });
    }

    // ── Track viewport size ─────────────────────────────────────────
    {
        let interaction = interaction.clone();
        let redraw = redraw.clone();
        use_effect_with((), move |_| {
            let update = {
                let interaction = interaction.clone();
                let redraw = redraw.clone();
                move || {
                    if let Some(el) = web_sys::window()
                        .and_then(|w| w.document())
                        .and_then(|d| d.get_element_by_id("gds-map"))
                    {
                        let el: HtmlElement = el.dyn_into().unwrap();
                        let mut it = interaction.borrow_mut();
                        it.view_w = el.client_width() as f64;
                        it.view_h = el.client_height() as f64;
                        drop(it);
                        redraw.set(*redraw + 1);
                    }
                }
            };
            update();
            let listener = gloo::events::EventListener::new(
                &web_sys::window().unwrap(),
                "resize",
                move |_| update(),
            );
            move || drop(listener)
        });
    }

    // ── Persist view state (debounced via redraw dependency) ────────
    {
        let view = view.clone();
        let prefix = prefix.clone();
        use_effect_with((*redraw, (*view).clone()), move |_| {
            if let Some(ref vs) = *view {
                if !prefix.is_empty() {
                    write_view_state(vs, &prefix);
                }
            }
            || ()
        });
    }

    let Some(m) = (*manifest).clone() else {
        let msg = (*error).clone().unwrap_or_else(|| "Loading…".to_string());
        let class = if error.is_some() {
            "gds-message error"
        } else {
            "gds-message"
        };
        return html! { <div id="app"><div id="gds-map" class="gds-map"><div class={class}>{msg}</div></div></div> };
    };
    let pyr = (*pyramid).clone().unwrap();
    let vs = (*view).clone().unwrap();

    // ── Event handlers ──────────────────────────────────────────────
    let on_wheel = {
        let interaction = interaction.clone();
        let view = view.clone();
        let redraw = redraw.clone();
        Callback::from(move |e: WheelEvent| {
            e.prevent_default();
            let mut it = interaction.borrow_mut();
            let mut delta = e.delta_y();
            if e.delta_mode() == 1 {
                delta *= 30.0;
            } else if e.delta_mode() == 2 {
                delta *= 300.0;
            }
            let mm = (1.1f64).powf(-delta / 40.0).clamp(0.5, 2.0);
            it.transform.zoom *= mm;
            // Cursor-centred: keep the point under the cursor fixed.
            it.transform.panx =
                e.offset_x() as f64 - (e.offset_x() as f64 - it.transform.panx) * mm;
            it.transform.pany =
                e.offset_y() as f64 - (e.offset_y() as f64 - it.transform.pany) * mm;
            let t = it.transform;
            drop(it);
            persist_transform(&view, &t);
            redraw.set(*redraw + 1);
        })
    };

    let on_pointerdown = {
        let interaction = interaction.clone();
        Callback::from(move |e: PointerEvent| {
            e.prevent_default();
            if let Some(el) = e.target().and_then(|t| t.dyn_into::<HtmlElement>().ok()) {
                let _ = el.set_pointer_capture(e.pointer_id());
            }
            interaction.borrow_mut().pointers.insert(
                e.pointer_id(),
                PointerState {
                    last_x: e.offset_x() as f64,
                    last_y: e.offset_y() as f64,
                },
            );
        })
    };

    let on_pointermove = {
        let interaction = interaction.clone();
        let view = view.clone();
        let redraw = redraw.clone();
        Callback::from(move |e: PointerEvent| {
            let mut it = interaction.borrow_mut();
            if !it.pointers.contains_key(&e.pointer_id()) {
                return;
            }
            e.prevent_default();
            let count = it.pointers.len();
            if count == 2 {
                let other_id = *it
                    .pointers
                    .keys()
                    .find(|&&id| id != e.pointer_id())
                    .unwrap();
                let (ox, oy) = {
                    let o = &it.pointers[&other_id];
                    (o.last_x, o.last_y)
                };
                let (cx, cy) = {
                    let c = &it.pointers[&e.pointer_id()];
                    (c.last_x, c.last_y)
                };
                let nx = e.offset_x() as f64;
                let ny = e.offset_y() as f64;
                let old_mid_x = (cx + ox) / 2.0;
                let old_mid_y = (cy + oy) / 2.0;
                let new_mid_x = (nx + ox) / 2.0;
                let new_mid_y = (ny + oy) / 2.0;
                let old_dist = ((cx - ox).powi(2) + (cy - oy).powi(2)).sqrt();
                let new_dist = ((nx - ox).powi(2) + (ny - oy).powi(2)).sqrt();
                it.transform.panx += new_mid_x - old_mid_x;
                it.transform.pany += new_mid_y - old_mid_y;
                if old_dist > 1.0 && new_dist > 1.0 {
                    let scale = (new_dist / old_dist).clamp(0.5, 2.0);
                    it.transform.zoom *= scale;
                    it.transform.panx = new_mid_x - (new_mid_x - it.transform.panx) * scale;
                    it.transform.pany = new_mid_y - (new_mid_y - it.transform.pany) * scale;
                }
                if let Some(p) = it.pointers.get_mut(&e.pointer_id()) {
                    p.last_x = nx;
                    p.last_y = ny;
                }
            } else if count == 1 {
                let (dx, dy) = {
                    let p = &it.pointers[&e.pointer_id()];
                    (
                        e.offset_x() as f64 - p.last_x,
                        e.offset_y() as f64 - p.last_y,
                    )
                };
                it.transform.panx += dx;
                it.transform.pany += dy;
                if let Some(p) = it.pointers.get_mut(&e.pointer_id()) {
                    p.last_x = e.offset_x() as f64;
                    p.last_y = e.offset_y() as f64;
                }
            }
            let t = it.transform;
            drop(it);
            persist_transform(&view, &t);
            redraw.set(*redraw + 1);
        })
    };

    let on_pointerup = {
        let interaction = interaction.clone();
        let view = view.clone();
        let redraw = redraw.clone();
        Callback::from(move |e: PointerEvent| {
            let mut it = interaction.borrow_mut();
            if e.button() == 2 {
                it.transform = Transform::default();
                let t = it.transform;
                it.pointers.remove(&e.pointer_id());
                drop(it);
                persist_transform(&view, &t);
                redraw.set(*redraw + 1);
                return;
            }
            it.pointers.remove(&e.pointer_id());
        })
    };

    let on_pointercancel = {
        let interaction = interaction.clone();
        Callback::from(move |e: PointerEvent| {
            interaction.borrow_mut().pointers.remove(&e.pointer_id());
        })
    };

    let on_contextmenu = Callback::from(|e: MouseEvent| e.prevent_default());

    // ── Compose layer surfaces ──────────────────────────────────────
    let (panx, pany, zoom, view_w, view_h) = {
        let it = interaction.borrow();
        (
            it.transform.panx,
            it.transform.pany,
            it.transform.zoom,
            it.view_w,
            it.view_h,
        )
    };
    let z = pyr.level_for_zoom(zoom);

    let layer_lookup: HashMap<String, &Layer> =
        m.layers.iter().map(|l| (l.tile_key(), l)).collect();

    let loaded_set = (*loaded).clone();
    let surfaces: Html = vs
        .layers
        .iter()
        .enumerate()
        .filter(|(_, l)| l.visible)
        .map(|(idx, l)| {
            render_surface(
                &m.id,
                l,
                idx,
                &pyr,
                z,
                zoom,
                panx,
                pany,
                view_w,
                view_h,
                &loaded_set,
                &redraw,
            )
        })
        .collect();

    // ── Layer panel ─────────────────────────────────────────────────
    let panel = render_panel(&vs, &view, &layer_lookup, &redraw, &drag_src);

    // Prefer the manifest's per-level resolution when present, else derive it.
    let res = m
        .zoom
        .res_nm_per_px
        .get(z as usize)
        .copied()
        .unwrap_or_else(|| pyr.res(z));
    let status = format!(
        "z {} · {:.0}×{:.0} nm · {:.3} nm/px · zoom {:.2}",
        z, pyr.extent_w, pyr.extent_h, res, zoom
    );

    html! {
        <div id="app">
            <div
                id="gds-map"
                class="gds-map"
                onwheel={on_wheel}
                onpointerdown={on_pointerdown}
                onpointermove={on_pointermove}
                onpointerup={on_pointerup}
                onpointercancel={on_pointercancel}
                oncontextmenu={on_contextmenu}
            >
                { surfaces }
                <div class="gds-status">{ status }</div>
            </div>
            { panel }
        </div>
    }
}

/// Persist the latest transform into the React-style state handle so it survives
/// reloads. We mutate a clone and set it back; cheap relative to a tile render.
fn persist_transform(view: &UseStateHandle<Option<ViewState>>, t: &Transform) {
    if let Some(mut vs) = (**view).clone() {
        vs.set_transform(t);
        view.set(Some(vs));
    }
}

/// Render one layer's surface div with its visible `<img>` tiles. Tiles that
/// 404 are hidden via an onerror handler so missing tiles simply aren't drawn.
///
/// To keep zooming smooth we render the level *below* the active one underneath
/// it. Tiles are keyed by level, so when the active level changes the previous
/// level's already-loaded tiles become the coarse backing set with their DOM
/// elements intact — they stay on screen, filling the gap while the finer tiles
/// fade in on top, instead of the whole map blanking out.
#[allow(clippy::too_many_arguments)]
fn render_surface(
    id: &str,
    layer: &LayerState,
    z_index: usize,
    pyr: &Pyramid,
    z: u32,
    zoom: f64,
    panx: f64,
    pany: f64,
    view_w: f64,
    view_h: f64,
    loaded: &Rc<RefCell<HashSet<String>>>,
    redraw: &UseStateHandle<u64>,
) -> Html {
    let style = format!(
        "z-index:{}; opacity:{};",
        z_index,
        layer.opacity.clamp(0.0, 1.0)
    );

    let mut tiles: Vec<Html> = Vec::new();
    // Coarser backing level first, then the active level. Each level carries an
    // explicit z-index (= its level number) so the finer level always paints on
    // top regardless of DOM order. Skip the backing level at the coarsest zoom.
    if z > pyr.min_z {
        emit_level_tiles(
            &mut tiles,
            id,
            layer,
            pyr,
            z - 1,
            zoom,
            panx,
            pany,
            view_w,
            view_h,
            loaded,
            redraw,
        );
    }
    emit_level_tiles(
        &mut tiles, id, layer, pyr, z, zoom, panx, pany, view_w, view_h, loaded, redraw,
    );

    html! {
        <div class="gds-surface" style={style}>
            { tiles }
        </div>
    }
}

/// Append the `<img>` tiles for one pyramid level `z` covering the viewport.
/// Tiles start transparent and fade in (CSS transition on `opacity`) only once
/// their image has actually loaded — tracked in `loaded` and reflected in the
/// inline style so pan re-renders never reset the fade.
#[allow(clippy::too_many_arguments)]
fn emit_level_tiles(
    out: &mut Vec<Html>,
    id: &str,
    layer: &LayerState,
    pyr: &Pyramid,
    z: u32,
    zoom: f64,
    panx: f64,
    pany: f64,
    view_w: f64,
    view_h: f64,
    loaded: &Rc<RefCell<HashSet<String>>>,
    redraw: &UseStateHandle<u64>,
) {
    let frac = pyr.frac_scale(zoom, z);
    let tile_screen = pyr.tile_px * frac;
    let range = pyr.visible_tiles(z, panx, pany, frac, view_w, view_h);

    // Prefetch one ring beyond the viewport for snappier panning.
    let n = pyr.tiles_per_axis(z) as i64;
    let gx0 = (range.x0 - 1).max(0);
    let gy0 = (range.y0 - 1).max(0);
    let gx1 = (range.x1 + 1).min(n - 1);
    let gy1 = (range.y1 + 1).min(n - 1);
    for ty in gy0..=gy1 {
        for tx in gx0..=gx1 {
            let left = panx + tx as f64 * tile_screen;
            let top = pany + ty as f64 * tile_screen;
            let src = format!("/g/{}/tiles/{}/{}/{}/{}.svgz", id, z, tx, ty, layer.key);
            let key = format!("{}:{}:{}:{}", z, tx, ty, layer.key);
            // Loaded (incl. browser-cached) tiles render opaque immediately; the
            // rest stay transparent until onload flips them, animating the fade.
            let opacity = if loaded.borrow().contains(&key) {
                1.0
            } else {
                0.0
            };
            let tile_style = format!(
                "left:{:.2}px; top:{:.2}px; width:{:.2}px; height:{:.2}px; z-index:{}; opacity:{};",
                left, top, tile_screen, tile_screen, z, opacity
            );
            let onload = {
                let loaded = loaded.clone();
                let redraw = redraw.clone();
                let key = key.clone();
                Callback::from(move |_: Event| {
                    if loaded.borrow_mut().insert(key.clone()) {
                        redraw.set(*redraw + 1);
                    }
                })
            };
            let onerror = Callback::from(|e: Event| {
                if let Some(img) = e.target().and_then(|t| t.dyn_into::<HtmlElement>().ok()) {
                    let _ = img.style().set_property("visibility", "hidden");
                }
            });
            out.push(html! {
                <img
                    key={key}
                    class="gds-tile"
                    src={src}
                    style={tile_style}
                    {onload}
                    {onerror}
                    draggable="false"
                />
            });
        }
    }
}

fn render_panel(
    vs: &ViewState,
    view: &UseStateHandle<Option<ViewState>>,
    lookup: &HashMap<String, &Layer>,
    redraw: &UseStateHandle<u64>,
    drag_src: &UseStateHandle<Option<usize>>,
) -> Html {
    let rows: Html = vs
        .layers
        .iter()
        .enumerate()
        .map(|(i, l)| {
            let display = if l.name.is_empty() {
                lookup
                    .get(&l.key)
                    .map(|m| m.display_name())
                    .unwrap_or_else(|| l.key.clone())
            } else {
                l.name.clone()
            };
            let count = lookup.get(&l.key).map(|m| m.count).unwrap_or(0);

            let on_toggle = layer_mutator(view, redraw, i, |ls| ls.visible = !ls.visible);
            let on_color = {
                let view = view.clone();
                let redraw = redraw.clone();
                Callback::from(move |e: Event| {
                    if let Some(inp) = e
                        .target()
                        .and_then(|t| t.dyn_into::<HtmlInputElement>().ok())
                    {
                        let val = inp.value();
                        mutate_layer(&view, &redraw, i, |ls| ls.color = val.clone());
                    }
                })
            };
            let on_name = {
                let view = view.clone();
                let redraw = redraw.clone();
                Callback::from(move |e: InputEvent| {
                    if let Some(inp) = e
                        .target()
                        .and_then(|t| t.dyn_into::<HtmlInputElement>().ok())
                    {
                        let val = inp.value();
                        mutate_layer(&view, &redraw, i, |ls| ls.name = val.clone());
                    }
                })
            };
            let on_opacity = {
                let view = view.clone();
                let redraw = redraw.clone();
                Callback::from(move |e: InputEvent| {
                    if let Some(inp) = e
                        .target()
                        .and_then(|t| t.dyn_into::<HtmlInputElement>().ok())
                    {
                        if let Ok(v) = inp.value().parse::<f64>() {
                            mutate_layer(&view, &redraw, i, |ls| ls.opacity = v / 100.0);
                        }
                    }
                })
            };

            let ondragstart = {
                let drag_src = drag_src.clone();
                Callback::from(move |_: DragEvent| drag_src.set(Some(i)))
            };
            let ondragover = Callback::from(|e: DragEvent| e.prevent_default());
            let ondrop = {
                let drag_src = drag_src.clone();
                let view = view.clone();
                let redraw = redraw.clone();
                Callback::from(move |e: DragEvent| {
                    e.prevent_default();
                    if let Some(from) = *drag_src {
                        reorder_layer(&view, &redraw, from, i);
                    }
                    drag_src.set(None);
                })
            };

            html! {
                <div class="gds-row" draggable="true" {ondragstart} {ondragover} {ondrop}>
                    <span class="gds-handle">{ "≡" }</span>
                    <input
                        type="checkbox"
                        checked={l.visible}
                        onchange={on_toggle}
                    />
                    <input
                        type="color"
                        class="gds-swatch"
                        value={l.color.clone()}
                        onchange={on_color}
                    />
                    <input
                        type="text"
                        class="gds-name"
                        value={display}
                        oninput={on_name}
                    />
                    <span class="gds-count">{ format_count(count) }</span>
                    <input
                        type="range"
                        class="gds-opacity"
                        min="0"
                        max="100"
                        value={((l.opacity * 100.0).round() as i64).to_string()}
                        oninput={on_opacity}
                    />
                </div>
            }
        })
        .collect();

    let show_all = {
        let view = view.clone();
        let redraw = redraw.clone();
        Callback::from(move |_: MouseEvent| set_all_visible(&view, &redraw, true))
    };
    let hide_all = {
        let view = view.clone();
        let redraw = redraw.clone();
        Callback::from(move |_: MouseEvent| set_all_visible(&view, &redraw, false))
    };

    html! {
        <div class="gds-panel">
            <div class="gds-panel-head">
                <h1>{ "Layers" }</h1>
                <div class="gds-allnone">
                    <button type="button" class="gds-btn" onclick={show_all}>{ "All" }</button>
                    <button type="button" class="gds-btn" onclick={hide_all}>{ "None" }</button>
                </div>
            </div>
            { rows }
        </div>
    }
}

/// Build an onchange callback that mutates layer `i` in place.
fn layer_mutator<F: Fn(&mut LayerState) + 'static>(
    view: &UseStateHandle<Option<ViewState>>,
    redraw: &UseStateHandle<u64>,
    i: usize,
    f: F,
) -> Callback<Event> {
    let view = view.clone();
    let redraw = redraw.clone();
    Callback::from(move |_: Event| {
        mutate_layer(&view, &redraw, i, &f);
    })
}

fn mutate_layer<F: Fn(&mut LayerState)>(
    view: &UseStateHandle<Option<ViewState>>,
    redraw: &UseStateHandle<u64>,
    i: usize,
    f: F,
) {
    if let Some(mut vs) = (**view).clone() {
        if let Some(ls) = vs.layers.get_mut(i) {
            f(ls);
            view.set(Some(vs));
            redraw.set(**redraw + 1);
        }
    }
}

/// Show or hide every layer at once (the panel's All / None control).
fn set_all_visible(
    view: &UseStateHandle<Option<ViewState>>,
    redraw: &UseStateHandle<u64>,
    visible: bool,
) {
    if let Some(mut vs) = (**view).clone() {
        for ls in &mut vs.layers {
            ls.visible = visible;
        }
        view.set(Some(vs));
        redraw.set(**redraw + 1);
    }
}

fn reorder_layer(
    view: &UseStateHandle<Option<ViewState>>,
    redraw: &UseStateHandle<u64>,
    from: usize,
    to: usize,
) {
    if from == to {
        return;
    }
    if let Some(mut vs) = (**view).clone() {
        if from < vs.layers.len() && to < vs.layers.len() {
            let item = vs.layers.remove(from);
            vs.layers.insert(to, item);
            view.set(Some(vs));
            redraw.set(**redraw + 1);
        }
    }
}

fn format_count(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    let bytes = s.as_bytes();
    for (i, c) in bytes.iter().enumerate() {
        if i > 0 && (bytes.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(*c as char);
    }
    out
}
