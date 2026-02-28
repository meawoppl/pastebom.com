mod pcbdata;
mod render;
mod state;

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use gloo::events::EventListener;
use wasm_bindgen::JsCast;
use web_sys::{HtmlCanvasElement, HtmlElement, HtmlInputElement, Path2d};
use yew::prelude::*;

use pcbdata::*;
use render::*;
use state::*;

fn main() {
    wasm_logger::init(wasm_logger::Config::default());
    yew::Renderer::<App>::new().render();
}

// ─── App State ──────────────────────────────────────────────────────

struct ViewerState {
    canvases: LayerCanvases,
    colors: Colors,
    path_cache: PathCache,
    zone_cache: HashMap<String, Path2d>,
    pointer_states: HashMap<i32, PointerState>,
}

struct PointerState {
    distance_travelled: f64,
    last_x: f64,
    last_y: f64,
    down_time: f64,
}

impl ViewerState {
    fn redraw(
        &mut self,
        data: &PcbData,
        settings: &Settings,
        hl: &[usize],
        mf: &HashSet<usize>,
        hn: &Option<String>,
    ) {
        let ViewerState {
            ref mut canvases,
            ref colors,
            ref mut path_cache,
            ref mut zone_cache,
            ..
        } = *self;
        render::redraw_canvas(
            canvases, data, colors, settings, hl, mf, hn, path_cache, zone_cache,
        );
    }

    fn redraw_highlights(
        &mut self,
        data: &PcbData,
        settings: &Settings,
        hl: &[usize],
        mf: &HashSet<usize>,
        hn: &Option<String>,
    ) {
        let ViewerState {
            ref mut canvases,
            ref colors,
            ref mut path_cache,
            ref mut zone_cache,
            ..
        } = *self;
        render::prepare_layer(canvases, settings);
        render::draw_highlights_on_layer(
            canvases, data, colors, settings, hl, mf, hn, path_cache, zone_cache,
        );
    }
}

// ─── App Component ──────────────────────────────────────────────────

#[function_component(App)]
fn app() -> Html {
    let pcbdata: UseStateHandle<Option<Rc<PcbData>>> = use_state(|| None);
    let settings = use_state(Settings::default);
    let highlighted_footprints: UseStateHandle<Vec<usize>> = use_state(Vec::new);
    let highlighted_net: UseStateHandle<Option<String>> = use_state(|| None);
    let marked_footprints: UseStateHandle<HashSet<usize>> = use_state(HashSet::new);
    let filter = use_state(String::new);
    let current_row: UseStateHandle<Option<String>> = use_state(|| None);
    let loading = use_state(|| true);
    let error: UseStateHandle<Option<String>> = use_state(|| None);
    let viewer_state: UseStateHandle<Option<Rc<RefCell<ViewerState>>>> = use_state(|| None);
    let storage_prefix_str = use_state(String::new);
    let redraw_trigger = use_state(|| 0u32);
    let board_flipped = use_state(|| false);
    let is_mobile = web_sys::window()
        .and_then(|w| w.inner_width().ok())
        .and_then(|v| v.as_f64())
        .map(|w| w < 768.0)
        .unwrap_or(false);
    let bom_sidebar_open = use_state(move || !is_mobile);
    let view_sidebar_open = use_state(move || !is_mobile);
    let upload_filename: UseStateHandle<Option<String>> = use_state(|| None);

    // Fetch pcbdata on mount
    {
        let pcbdata = pcbdata.clone();
        let settings = settings.clone();
        let loading = loading.clone();
        let error = error.clone();
        let storage_prefix_str = storage_prefix_str.clone();
        let upload_filename = upload_filename.clone();

        use_effect_with((), move |_| {
            wasm_bindgen_futures::spawn_local(async move {
                let window = web_sys::window().unwrap();
                let pathname = window.location().pathname().unwrap_or_default();

                // Fetch upload metadata (filename) in parallel
                let meta_url = format!("{}/meta", pathname);
                if let Ok(meta_resp) = gloo::net::http::Request::get(&meta_url).send().await {
                    if let Ok(text) = meta_resp.text().await {
                        if let Ok(meta) = serde_json::from_str::<serde_json::Value>(&text) {
                            if let Some(name) = meta.get("filename").and_then(|v| v.as_str()) {
                                upload_filename.set(Some(name.to_string()));
                            }
                        }
                    }
                }

                let data_url = format!("{}/data", pathname);

                match gloo::net::http::Request::get(&data_url).send().await {
                    Ok(resp) => {
                        if resp.ok() {
                            match resp.text().await {
                                Ok(text) => match serde_json::from_str::<PcbData>(&text) {
                                    Ok(data) => {
                                        let prefix = storage_prefix(
                                            &data.metadata.title,
                                            &data.metadata.revision,
                                        );
                                        let s = init_settings(&prefix);
                                        storage_prefix_str.set(prefix);
                                        settings.set(s);
                                        pcbdata.set(Some(Rc::new(data)));
                                        loading.set(false);
                                    }
                                    Err(e) => {
                                        error.set(Some(format!("Failed to parse data: {}", e)));
                                        loading.set(false);
                                    }
                                },
                                Err(e) => {
                                    error.set(Some(format!("Failed to read response: {}", e)));
                                    loading.set(false);
                                }
                            }
                        } else {
                            error.set(Some(format!("BOM not found ({})", resp.status())));
                            loading.set(false);
                        }
                    }
                    Err(e) => {
                        error.set(Some(format!("Network error: {}", e)));
                        loading.set(false);
                    }
                }
            });
            || ()
        });
    }

    // Initialize canvases after pcbdata is loaded
    {
        let pcbdata = pcbdata.clone();
        let settings = settings.clone();
        let viewer_state = viewer_state.clone();
        let highlighted_footprints = highlighted_footprints.clone();
        let highlighted_net = highlighted_net.clone();
        let marked_footprints = marked_footprints.clone();
        let redraw_trigger = redraw_trigger.clone();
        let board_flipped = board_flipped.clone();

        use_effect_with(
            (pcbdata.is_some(), *redraw_trigger, *board_flipped),
            move |_| {
                if let Some(ref data) = *pcbdata {
                    let layer_name = if *board_flipped { "B" } else { "F" };

                    let state = if viewer_state.is_none() {
                        let document = web_sys::window().unwrap().document().unwrap();

                        let get_canvas = |id: &str| -> HtmlCanvasElement {
                            document
                                .get_element_by_id(id)
                                .unwrap()
                                .dyn_into::<HtmlCanvasElement>()
                                .unwrap()
                        };

                        let topmostdiv = document.get_element_by_id("topmostdiv").unwrap();
                        let colors = Colors::from_element(&topmostdiv);

                        let canvases = LayerCanvases {
                            bg: get_canvas("bg"),
                            fab: get_canvas("fab"),
                            silk: get_canvas("slk"),
                            highlight: get_canvas("hl"),
                            layer: layer_name.to_string(),
                            transform: Transform::default(),
                        };

                        let vs = Rc::new(RefCell::new(ViewerState {
                            canvases,
                            colors,
                            path_cache: PathCache::new(),
                            zone_cache: HashMap::new(),
                            pointer_states: HashMap::new(),
                        }));

                        viewer_state.set(Some(vs.clone()));
                        vs
                    } else {
                        let vs = viewer_state.as_ref().unwrap().clone();
                        vs.borrow_mut().canvases.layer = layer_name.to_string();
                        vs
                    };

                    let mut vs = state.borrow_mut();

                    // Update colors on dark mode change
                    if let Some(document) = web_sys::window().and_then(|w| w.document()) {
                        if let Some(el) = document.get_element_by_id("topmostdiv") {
                            vs.colors = Colors::from_element(&el);
                        }
                    }

                    // Resize and redraw
                    let dpr = web_sys::window()
                        .map(|w| w.device_pixel_ratio())
                        .unwrap_or(1.0);

                    if let Some(document) = web_sys::window().and_then(|w| w.document()) {
                        if let Some(el) = document.get_element_by_id("canvascontainer") {
                            let el: HtmlElement = el.dyn_into().unwrap();
                            let width = el.client_width() as f64 * dpr;
                            let height = el.client_height() as f64 * dpr;
                            if width > 0.0 && height > 0.0 {
                                recalc_layer_scale(
                                    &mut vs.canvases,
                                    width,
                                    height,
                                    data,
                                    &settings,
                                );
                            }
                        }
                    }

                    let hl = (*highlighted_footprints).clone();
                    let hn = (*highlighted_net).clone();
                    let mf = (*marked_footprints).clone();

                    vs.redraw(data, &settings, &hl, &mf, &hn);
                }
                || ()
            },
        );
    }

    // Window resize handler
    {
        let redraw_trigger = redraw_trigger.clone();
        use_effect_with((), move |_| {
            let listener = EventListener::new(&web_sys::window().unwrap(), "resize", move |_| {
                redraw_trigger.set(*redraw_trigger + 1);
            });
            move || drop(listener)
        });
    }

    // Canvas event handlers
    let on_canvas_wheel = {
        let viewer_state = viewer_state.clone();
        let pcbdata = pcbdata.clone();
        let settings = settings.clone();
        let highlighted_footprints = highlighted_footprints.clone();
        let highlighted_net = highlighted_net.clone();
        let marked_footprints = marked_footprints.clone();

        Callback::from(move |e: WheelEvent| {
            e.prevent_default();
            if let (Some(state), Some(data)) = ((*viewer_state).as_ref(), (*pcbdata).as_ref()) {
                let mut vs = state.borrow_mut();

                let mut wheeldelta = e.delta_y();
                if e.delta_mode() == 1 {
                    wheeldelta *= 30.0;
                } else if e.delta_mode() == 2 {
                    wheeldelta *= 300.0;
                }
                let m = (1.1f64).powf(-wheeldelta / 40.0).clamp(0.5, 2.0);
                let dpr = web_sys::window()
                    .map(|w| w.device_pixel_ratio())
                    .unwrap_or(1.0);

                vs.canvases.transform.zoom *= m;
                let zoomd = (1.0 - m) / vs.canvases.transform.zoom;
                vs.canvases.transform.panx += dpr * e.offset_x() as f64 * zoomd;
                vs.canvases.transform.pany += dpr * e.offset_y() as f64 * zoomd;

                let hl = (*highlighted_footprints).clone();
                let hn = (*highlighted_net).clone();
                let mf = (*marked_footprints).clone();
                vs.redraw(data, &settings, &hl, &mf, &hn);
            }
        })
    };

    let on_canvas_pointerdown = {
        let viewer_state = viewer_state.clone();
        Callback::from(move |e: PointerEvent| {
            e.prevent_default();
            if let Some(canvas) = e.target().and_then(|t| t.dyn_into::<HtmlElement>().ok()) {
                let _ = canvas.set_pointer_capture(e.pointer_id());
            }
            if let Some(ref state) = *viewer_state {
                let mut vs = state.borrow_mut();
                vs.pointer_states.insert(
                    e.pointer_id(),
                    PointerState {
                        distance_travelled: 0.0,
                        last_x: e.offset_x() as f64,
                        last_y: e.offset_y() as f64,
                        down_time: js_sys::Date::now(),
                    },
                );
            }
        })
    };

    let on_canvas_pointermove = {
        let viewer_state = viewer_state.clone();
        let pcbdata = pcbdata.clone();
        let settings = settings.clone();
        let highlighted_footprints = highlighted_footprints.clone();
        let highlighted_net = highlighted_net.clone();
        let marked_footprints = marked_footprints.clone();

        Callback::from(move |e: PointerEvent| {
            if let (Some(state), Some(data)) = ((*viewer_state).as_ref(), (*pcbdata).as_ref()) {
                let mut vs = state.borrow_mut();
                if !vs.pointer_states.contains_key(&e.pointer_id()) {
                    return;
                }
                e.prevent_default();

                {
                    let ViewerState {
                        ref mut canvases,
                        ref mut pointer_states,
                        ..
                    } = *vs;
                    let pointer_count = pointer_states.len();

                    let dpr = web_sys::window()
                        .map(|w| w.device_pixel_ratio())
                        .unwrap_or(1.0);

                    if pointer_count == 2 {
                        // Pinch-to-zoom + simultaneous pan by centroid movement
                        let other_id = *pointer_states
                            .keys()
                            .find(|&&id| id != e.pointer_id())
                            .unwrap();
                        let other = pointer_states.get(&other_id).unwrap();
                        let cur = pointer_states.get(&e.pointer_id()).unwrap();

                        let old_mid_x = (cur.last_x + other.last_x) / 2.0;
                        let old_mid_y = (cur.last_y + other.last_y) / 2.0;
                        let new_mid_x = (e.offset_x() as f64 + other.last_x) / 2.0;
                        let new_mid_y = (e.offset_y() as f64 + other.last_y) / 2.0;

                        let old_dist = ((cur.last_x - other.last_x).powi(2)
                            + (cur.last_y - other.last_y).powi(2))
                        .sqrt();
                        let new_dist = ((e.offset_x() as f64 - other.last_x).powi(2)
                            + (e.offset_y() as f64 - other.last_y).powi(2))
                        .sqrt();

                        // Pan by centroid movement (before zoom so units are consistent)
                        canvases.transform.panx +=
                            dpr * (new_mid_x - old_mid_x) / canvases.transform.zoom;
                        canvases.transform.pany +=
                            dpr * (new_mid_y - old_mid_y) / canvases.transform.zoom;

                        // Zoom around new centroid
                        if old_dist > 1.0 && new_dist > 1.0 {
                            let scale = (new_dist / old_dist).clamp(0.5, 2.0);
                            canvases.transform.zoom *= scale;
                            let zoomd = (1.0 - scale) / canvases.transform.zoom;
                            canvases.transform.panx += dpr * new_mid_x * zoomd;
                            canvases.transform.pany += dpr * new_mid_y * zoomd;
                        }

                        let ptr = pointer_states.get_mut(&e.pointer_id()).unwrap();
                        ptr.distance_travelled += 100.0; // prevent click detection
                        ptr.last_x = e.offset_x() as f64;
                        ptr.last_y = e.offset_y() as f64;
                    } else if pointer_count == 1 {
                        let ptr = pointer_states.get_mut(&e.pointer_id()).unwrap();
                        let dx = e.offset_x() as f64 - ptr.last_x;
                        let dy = e.offset_y() as f64 - ptr.last_y;
                        ptr.distance_travelled += dx.abs() + dy.abs();

                        canvases.transform.panx += dpr * dx / canvases.transform.zoom;
                        canvases.transform.pany += dpr * dy / canvases.transform.zoom;

                        ptr.last_x = e.offset_x() as f64;
                        ptr.last_y = e.offset_y() as f64;
                    }
                }

                if settings.redraw_on_drag {
                    let hl = (*highlighted_footprints).clone();
                    let hn = (*highlighted_net).clone();
                    let mf = (*marked_footprints).clone();
                    vs.redraw(data, &settings, &hl, &mf, &hn);
                }
            }
        })
    };

    let on_canvas_pointerup = {
        let viewer_state = viewer_state.clone();
        let pcbdata = pcbdata.clone();
        let settings = settings.clone();
        let highlighted_footprints = highlighted_footprints.clone();
        let highlighted_net = highlighted_net.clone();
        let marked_footprints = marked_footprints.clone();
        let current_row = current_row.clone();

        Callback::from(move |e: PointerEvent| {
            if let (Some(state), Some(data)) = ((*viewer_state).as_ref(), (*pcbdata).as_ref()) {
                let mut vs = state.borrow_mut();

                if e.button() == 2 {
                    vs.canvases.transform.panx = 0.0;
                    vs.canvases.transform.pany = 0.0;
                    vs.canvases.transform.zoom = 1.0;
                    let hl = (*highlighted_footprints).clone();
                    let hn = (*highlighted_net).clone();
                    let mf = (*marked_footprints).clone();
                    vs.redraw(data, &settings, &hl, &mf, &hn);
                    vs.pointer_states.remove(&e.pointer_id());
                    return;
                }

                let was_click = if let Some(ptr) = vs.pointer_states.get(&e.pointer_id()) {
                    ptr.distance_travelled < 10.0 && js_sys::Date::now() - ptr.down_time <= 500.0
                } else {
                    false
                };

                if was_click && e.button() == 0 {
                    let layer_str = vs.canvases.layer.clone();
                    let board_pt = screen_to_board(
                        e.offset_x() as f64,
                        e.offset_y() as f64,
                        &vs.canvases.transform,
                        &layer_str,
                        &settings,
                    );

                    if data.nets.is_some() {
                        let net =
                            net_hit_scan(&layer_str, board_pt[0], board_pt[1], data, &settings);
                        if net != *highlighted_net {
                            highlighted_net.set(net.clone());
                            highlighted_footprints.set(Vec::new());
                            current_row.set(None);
                        }
                    }
                    if highlighted_net.is_none() {
                        let fps = bbox_hit_scan(&layer_str, board_pt[0], board_pt[1], data);
                        if !fps.is_empty() {
                            highlighted_footprints.set(fps);
                            highlighted_net.set(None);
                        }
                    }
                } else if !settings.redraw_on_drag {
                    let hl = (*highlighted_footprints).clone();
                    let hn = (*highlighted_net).clone();
                    let mf = (*marked_footprints).clone();
                    vs.redraw(data, &settings, &hl, &mf, &hn);
                }

                vs.pointer_states.remove(&e.pointer_id());
            }
        })
    };

    let on_canvas_pointercancel = {
        let viewer_state = viewer_state.clone();
        Callback::from(move |e: PointerEvent| {
            if let Some(ref state) = *viewer_state {
                let mut vs = state.borrow_mut();
                vs.pointer_states.remove(&e.pointer_id());
            }
        })
    };

    // Redraw only highlight layers when highlight state changes
    {
        let viewer_state = viewer_state.clone();
        let pcbdata = pcbdata.clone();
        let settings = settings.clone();
        let highlighted_footprints = highlighted_footprints.clone();
        let highlighted_net = highlighted_net.clone();
        let marked_footprints = marked_footprints.clone();
        let hl = (*highlighted_footprints).clone();
        let hn = (*highlighted_net).clone();
        use_effect_with((hl, hn), move |_| {
            if let (Some(state), Some(data)) = ((*viewer_state).as_ref(), (*pcbdata).as_ref()) {
                let mut vs = state.borrow_mut();
                let hl = (*highlighted_footprints).clone();
                let hn = (*highlighted_net).clone();
                let mf = (*marked_footprints).clone();
                vs.redraw_highlights(data, &settings, &hl, &mf, &hn);
            }
            || ()
        });
    }

    // ─── Settings callbacks ─────────────────────────────────────────

    let toggle_dark_mode = {
        let settings = settings.clone();
        let storage_prefix_str = storage_prefix_str.clone();
        let redraw_trigger = redraw_trigger.clone();
        Callback::from(move |_| {
            let mut s = (*settings).clone();
            s.dark_mode = !s.dark_mode;
            write_storage("darkmode", &s.dark_mode.to_string(), &storage_prefix_str);
            settings.set(s);
            let rt = redraw_trigger.clone();
            gloo::timers::callback::Timeout::new(50, move || {
                rt.set(*rt + 1);
            })
            .forget();
        })
    };

    let toggle_setting = {
        let settings = settings.clone();
        let storage_prefix_str = storage_prefix_str.clone();
        let redraw_trigger = redraw_trigger.clone();
        Callback::from(move |(key, value): (String, bool)| {
            let mut s = (*settings).clone();
            match key.as_str() {
                "pads" => {
                    s.render_pads = value;
                    write_storage("padsVisible", &value.to_string(), &storage_prefix_str);
                }
                "references" => {
                    s.render_references = value;
                    write_storage("referencesVisible", &value.to_string(), &storage_prefix_str);
                }
                "values" => {
                    s.render_values = value;
                    write_storage("valuesVisible", &value.to_string(), &storage_prefix_str);
                }
                "fabrication" => {
                    s.render_fabrication = value;
                    write_storage(
                        "fabricationVisible",
                        &value.to_string(),
                        &storage_prefix_str,
                    );
                }
                "silkscreen" => {
                    s.render_silkscreen = value;
                    write_storage("silkscreenVisible", &value.to_string(), &storage_prefix_str);
                }
                "tracks" => {
                    s.render_tracks = value;
                    write_storage("tracksVisible", &value.to_string(), &storage_prefix_str);
                }
                "zones" => {
                    s.render_zones = value;
                    write_storage("zonesVisible", &value.to_string(), &storage_prefix_str);
                }
                "dnp" => {
                    s.render_dnp_outline = value;
                    write_storage("dnpOutline", &value.to_string(), &storage_prefix_str);
                }
                "redraw_on_drag" => {
                    s.redraw_on_drag = value;
                    write_storage("redrawOnDrag", &value.to_string(), &storage_prefix_str);
                }
                "offset_back_rotation" => {
                    s.offset_back_rotation = value;
                    write_storage(
                        "offsetBackRotation",
                        &value.to_string(),
                        &storage_prefix_str,
                    );
                }
                "highlight_row_on_click" => {
                    s.highlight_row_on_click = value;
                    write_storage(
                        "highlightRowOnClick",
                        &value.to_string(),
                        &storage_prefix_str,
                    );
                }
                _ => {}
            }
            settings.set(s);
            redraw_trigger.set(*redraw_trigger + 1);
        })
    };

    let set_bom_mode = {
        let settings = settings.clone();
        let storage_prefix_str = storage_prefix_str.clone();
        let highlighted_footprints = highlighted_footprints.clone();
        let highlighted_net = highlighted_net.clone();
        let current_row = current_row.clone();
        Callback::from(move |mode: String| {
            let mut s = (*settings).clone();
            if mode != s.bom_mode {
                highlighted_footprints.set(Vec::new());
                highlighted_net.set(None);
                current_row.set(None);
            }
            s.bom_mode = mode.clone();
            write_storage("bommode", &mode, &storage_prefix_str);
            settings.set(s);
        })
    };

    let set_board_rotation = {
        let settings = settings.clone();
        let storage_prefix_str = storage_prefix_str.clone();
        let redraw_trigger = redraw_trigger.clone();
        Callback::from(move |value: i32| {
            let mut s = (*settings).clone();
            s.board_rotation = (value * 5) as f64;
            write_storage(
                "boardRotation",
                &s.board_rotation.to_string(),
                &storage_prefix_str,
            );
            settings.set(s);
            redraw_trigger.set(*redraw_trigger + 1);
        })
    };

    let set_highlight_pin1 = {
        let settings = settings.clone();
        let storage_prefix_str = storage_prefix_str.clone();
        let redraw_trigger = redraw_trigger.clone();
        Callback::from(move |value: String| {
            let mut s = (*settings).clone();
            s.highlight_pin1 = value.clone();
            write_storage("highlightpin1", &value, &storage_prefix_str);
            settings.set(s);
            redraw_trigger.set(*redraw_trigger + 1);
        })
    };

    let on_filter_change = {
        let filter = filter.clone();
        Callback::from(move |e: InputEvent| {
            let input: HtmlInputElement = e.target_unchecked_into();
            filter.set(input.value().to_lowercase());
        })
    };

    // Flip board callback
    let on_flip = {
        let board_flipped = board_flipped.clone();
        let viewer_state = viewer_state.clone();
        Callback::from(move |_: MouseEvent| {
            board_flipped.set(!*board_flipped);
            if let Some(ref state) = *viewer_state {
                let mut vs = state.borrow_mut();
                // Adjust panx to keep the same board point at viewport center.
                // The back view mirrors x, so we need:
                //   panx_new = width*(1/zoom - 1) - panx_old
                let dpr = web_sys::window()
                    .map(|w| w.device_pixel_ratio())
                    .unwrap_or(1.0);
                if let Some(document) = web_sys::window().and_then(|w| w.document()) {
                    if let Some(el) = document.get_element_by_id("canvascontainer") {
                        let el: HtmlElement = el.dyn_into::<HtmlElement>().unwrap();
                        let width = el.client_width() as f64 * dpr;
                        let zoom = vs.canvases.transform.zoom;
                        vs.canvases.transform.panx =
                            width * (1.0 / zoom - 1.0) - vs.canvases.transform.panx;
                    }
                }
            }
        })
    };

    // ─── BOM row click/hover handler ────────────────────────────────

    let on_bom_row_highlight = {
        let highlighted_footprints = highlighted_footprints.clone();
        let highlighted_net = highlighted_net.clone();
        let current_row = current_row.clone();
        let redraw_trigger = redraw_trigger.clone();
        Callback::from(
            move |(row_id, refs, net): (String, Option<Vec<BomRef>>, Option<String>)| {
                current_row.set(Some(row_id));
                if let Some(refs) = refs {
                    highlighted_footprints.set(refs.iter().map(|r| r.1).collect());
                } else {
                    highlighted_footprints.set(Vec::new());
                }
                highlighted_net.set(net);
                redraw_trigger.set(*redraw_trigger + 1);
            },
        )
    };

    // ─── Render ─────────────────────────────────────────────────────

    if *loading {
        return html! {
            <div style="display: flex; justify-content: center; align-items: center; height: 100vh; font-family: sans-serif; font-size: 24px;">
                {"Loading BOM..."}
            </div>
        };
    }

    if let Some(ref err) = *error {
        return html! {
            <div style="display: flex; justify-content: center; align-items: center; height: 100vh; font-family: sans-serif; color: red; font-size: 18px;">
                {err}
            </div>
        };
    }

    let data = match &*pcbdata {
        Some(d) => d.clone(),
        None => return html! { <div>{"No data"}</div> },
    };

    let has_nets = data.nets.is_some();
    let has_tracks = data.tracks.is_some();

    let bom_entries = get_bom_entries(&data, &settings, &filter);

    let dark_class = if settings.dark_mode { "dark" } else { "" };

    let oncontextmenu = Callback::from(|e: MouseEvent| e.prevent_default());

    let layer_label = if *board_flipped { "Back" } else { "Front" };
    let layer_prefix = if *board_flipped { "B" } else { "F" };

    html! {
        <div id="topmostdiv" class={classes!("topmostdiv", dark_class)}>
            // ─── Fullscreen canvas ─────────────────────────────
            <div id="canvascontainer"
                onwheel={on_canvas_wheel}
                onpointerdown={on_canvas_pointerdown}
                onpointermove={on_canvas_pointermove}
                onpointerup={on_canvas_pointerup}
                onpointercancel={on_canvas_pointercancel}
                oncontextmenu={oncontextmenu}>
                <canvas id="bg" style="position: absolute; left: 0; top: 0; z-index: 0;"></canvas>
                <canvas id="fab" style="position: absolute; left: 0; top: 0; z-index: 1;"></canvas>
                <canvas id="slk" style="position: absolute; left: 0; top: 0; z-index: 2;"></canvas>
                <canvas id="hl" style="position: absolute; left: 0; top: 0; z-index: 3;"></canvas>
            </div>

            // ─── Flip button ───────────────────────────────────
            <button class="flip-btn" onclick={on_flip}>{layer_label}</button>

            // ─── BOM sidebar (left) ────────────────────────────
            if *bom_sidebar_open {
                <div class="sidebar bom-sidebar">
                    <div class="sidebar-header">
                        <div>
                            <div class="sidebar-title">{
                                if let Some(ref name) = *upload_filename {
                                    name.clone()
                                } else {
                                    data.metadata.title.clone()
                                }
                            }</div>
                            <div class="sidebar-subtitle">
                                if upload_filename.is_some() && !data.metadata.title.is_empty() {
                                    {format!("{} ", &data.metadata.title)}
                                }
                                {format!("Rev: {}", &data.metadata.revision)}
                                if !data.metadata.date.is_empty() {
                                    {format!(" | {}", &data.metadata.date)}
                                }
                            </div>
                        </div>
                        <button class="sidebar-close" onclick={{
                            let s = bom_sidebar_open.clone();
                            Callback::from(move |_: MouseEvent| s.set(false))
                        }}>{"‹"}</button>
                    </div>
                    <div class="sidebar-controls">
                        <div class="button-container">
                            <button id="bom-grouped-btn"
                                class={classes!("left-most-button", (settings.bom_mode == "grouped").then_some("depressed"))}
                                onclick={{let s = set_bom_mode.clone(); Callback::from(move |_| s.emit("grouped".into()))}}
                            ></button>
                            <button id="bom-ungrouped-btn"
                                class={classes!(if has_nets { "middle-button" } else { "right-most-button" },
                                    (settings.bom_mode == "ungrouped").then_some("depressed"))}
                                onclick={{let s = set_bom_mode.clone(); Callback::from(move |_| s.emit("ungrouped".into()))}}
                            ></button>
                            if has_nets {
                                <button id="bom-netlist-btn"
                                    class={classes!("right-most-button", (settings.bom_mode == "netlist").then_some("depressed"))}
                                    onclick={{let s = set_bom_mode.clone(); Callback::from(move |_| s.emit("netlist".into()))}}
                                ></button>
                            }
                        </div>
                    </div>
                    <div class="sidebar-filter-container">
                        <input class="sidebar-filter" type="text"
                            placeholder="Filter" oninput={on_filter_change} />
                    </div>
                    <div class="sidebar-table-container">
                        <table class="bom" id="bomtable">
                            <thead id="bomhead">
                                <tr>
                                    <th class="numCol">{"#"}</th>
                                    if settings.bom_mode == "netlist" {
                                        <th>{"Net name"}</th>
                                    } else {
                                        <th>{"References"}</th>
                                        {for data.bom.as_ref().map(|_| {
                                            let fields: Vec<&str> = vec!["Value", "Footprint"];
                                            fields.into_iter().map(|f| html! { <th>{f}</th> }).collect::<Html>()
                                        })}
                                        if settings.bom_mode == "grouped" {
                                            <th class="quantity">{"Qty"}</th>
                                        }
                                    }
                                </tr>
                            </thead>
                            <tbody id="bombody">
                                {for bom_entries.iter().enumerate().map(|(idx, entry)| {
                                    let row_id = format!("bomrow{}", idx + 1);
                                    let is_highlighted = (*current_row).as_deref() == Some(row_id.as_str());

                                    let handler = {
                                        let row_id = row_id.clone();
                                        let entry = entry.clone();
                                        let cb = on_bom_row_highlight.clone();
                                        match entry {
                                            BomEntry::Component { refs, .. } => {
                                                let refs2 = refs.clone();
                                                Callback::from(move |_: MouseEvent| {
                                                    cb.emit((row_id.clone(), Some(refs2.clone()), None));
                                                })
                                            }
                                            BomEntry::Net { name, .. } => {
                                                let name2 = name.clone();
                                                Callback::from(move |_: MouseEvent| {
                                                    cb.emit((row_id.clone(), None, Some(name2.clone())));
                                                })
                                            }
                                        }
                                    };

                                    html! {
                                        <tr id={row_id}
                                            class={classes!(is_highlighted.then_some("highlighted"))}
                                            onmousedown={handler}
                                        >
                                            <td>{idx + 1}</td>
                                            {match entry {
                                                BomEntry::Component { refs, fields } => html! {
                                                    <>
                                                        <td>{refs.iter().map(|r| r.0.as_str()).collect::<Vec<_>>().join(", ")}</td>
                                                        {for fields.iter().map(|f| html! { <td>{f}</td> })}
                                                        if settings.bom_mode == "grouped" {
                                                            <td>{refs.len()}</td>
                                                        }
                                                    </>
                                                },
                                                BomEntry::Net { name } => html! {
                                                    <td>{if name.is_empty() { "<no net>" } else { &name }}</td>
                                                },
                                            }}
                                        </tr>
                                    }
                                })}
                            </tbody>
                        </table>
                    </div>
                </div>
            } else {
                <button class="sidebar-tab left-tab" onclick={{
                    let s = bom_sidebar_open.clone();
                    Callback::from(move |_: MouseEvent| s.set(true))
                }}>{"›"}</button>
            }

            // ─── View sidebar (right) ──────────────────────────
            if *view_sidebar_open {
                <div class="sidebar view-sidebar">
                    <div class="sidebar-header">
                        <span class="sidebar-title">{"View"}</span>
                        <button class="sidebar-close" onclick={{
                            let s = view_sidebar_open.clone();
                            Callback::from(move |_: MouseEvent| s.set(false))
                        }}>{"›"}</button>
                    </div>
                    <div class="sidebar-settings">
                        // ─── Layer color key ──────────────────────────
                        <div class="layer-key">
                            <div class="layer-key-title">{format!("Layers ({})", layer_label)}</div>
                            <div class="layer-key-item">
                                <span class="layer-swatch" style="background: var(--pad-color);"></span>
                                <span>{format!("{}.Cu (pads)", layer_prefix)}</span>
                            </div>
                            if has_tracks {
                                <div class="layer-key-item">
                                    <span class="layer-swatch" style={format!("background: var(--track-color-{});", if *board_flipped { "back" } else { "front" })}></span>
                                    <span>{format!("{}.Cu (tracks)", layer_prefix)}</span>
                                </div>
                                <div class="layer-key-item">
                                    <span class="layer-swatch" style={format!("background: var(--zone-color-{});", if *board_flipped { "back" } else { "front" })}></span>
                                    <span>{format!("{}.Cu (zones)", layer_prefix)}</span>
                                </div>
                            }
                            <div class="layer-key-item">
                                <span class="layer-swatch" style="background: var(--silkscreen-edge-color);"></span>
                                <span>{format!("{}.SilkS", layer_prefix)}</span>
                            </div>
                            <div class="layer-key-item">
                                <span class="layer-swatch" style="background: var(--fabrication-edge-color);"></span>
                                <span>{format!("{}.Fab", layer_prefix)}</span>
                            </div>
                            <div class="layer-key-item">
                                <span class="layer-swatch" style="background: var(--pcb-edge-color);"></span>
                                <span>{"Edge.Cuts"}</span>
                            </div>
                        </div>
                        // ─── Settings ─────────────────────────────────
                        <SettingCheckbox label="Dark mode" checked={settings.dark_mode}
                            on_change={toggle_dark_mode.clone()} is_top={true} />
                        <SettingCheckbox label="Silkscreen" checked={settings.render_silkscreen}
                            on_change={{let ts = toggle_setting.clone(); let v = settings.render_silkscreen; Callback::from(move |_| ts.emit(("silkscreen".into(), !v)))}}
                            is_top={false} />
                        <SettingCheckbox label="Fab layer" checked={settings.render_fabrication}
                            on_change={{let ts = toggle_setting.clone(); let v = settings.render_fabrication; Callback::from(move |_| ts.emit(("fabrication".into(), !v)))}}
                            is_top={false} />
                        <SettingCheckbox label="References" checked={settings.render_references}
                            on_change={{let ts = toggle_setting.clone(); let v = settings.render_references; Callback::from(move |_| ts.emit(("references".into(), !v)))}}
                            is_top={false} />
                        <SettingCheckbox label="Values" checked={settings.render_values}
                            on_change={{let ts = toggle_setting.clone(); let v = settings.render_values; Callback::from(move |_| ts.emit(("values".into(), !v)))}}
                            is_top={false} />
                        if has_tracks {
                            <SettingCheckbox label="Tracks" checked={settings.render_tracks}
                                on_change={{let ts = toggle_setting.clone(); let v = settings.render_tracks; Callback::from(move |_| ts.emit(("tracks".into(), !v)))}}
                                is_top={false} />
                            <SettingCheckbox label="Zones" checked={settings.render_zones}
                                on_change={{let ts = toggle_setting.clone(); let v = settings.render_zones; Callback::from(move |_| ts.emit(("zones".into(), !v)))}}
                                is_top={false} />
                        }
                        <SettingCheckbox label="Pads" checked={settings.render_pads}
                            on_change={{let ts = toggle_setting.clone(); let v = settings.render_pads; Callback::from(move |_| ts.emit(("pads".into(), !v)))}}
                            is_top={false} />
                        <SettingCheckbox label="Redraw on drag" checked={settings.redraw_on_drag}
                            on_change={{let ts = toggle_setting.clone(); let v = settings.redraw_on_drag; Callback::from(move |_| ts.emit(("redraw_on_drag".into(), !v)))}}
                            is_top={false} />
                        <label class="menu-label">
                            <span>{"Board rotation"}</span>
                            <span style="float: right">
                                <span>{format!("{}°", settings.board_rotation as i32)}</span>
                            </span>
                            <input type="range" class="slider" min="-36" max="36"
                                value={(settings.board_rotation as i32 / 5).to_string()}
                                oninput={{
                                    let sbr = set_board_rotation.clone();
                                    Callback::from(move |e: InputEvent| {
                                        let input: HtmlInputElement = e.target_unchecked_into();
                                        if let Ok(v) = input.value().parse::<i32>() {
                                            sbr.emit(v);
                                        }
                                    })
                                }}
                            />
                        </label>
                        <label class="menu-label">
                            {"Highlight first pin "}
                            <div class="flexbox">
                                {for ["none", "all", "selected"].iter().map(|v| {
                                    let shp = set_highlight_pin1.clone();
                                    let val = v.to_string();
                                    let checked = settings.highlight_pin1 == *v;
                                    html! {
                                        <label>
                                            <input type="radio" name="highlightpin1"
                                                value={val.clone()} {checked}
                                                onchange={{
                                                    let val = val.clone();
                                                    Callback::from(move |_| shp.emit(val.clone()))
                                                }}
                                            />
                                            {v.chars().next().unwrap().to_uppercase().to_string()}{&v[1..]}
                                        </label>
                                    }
                                })}
                            </div>
                        </label>
                    </div>
                </div>
            } else {
                <button class="sidebar-tab right-tab" onclick={{
                    let s = view_sidebar_open.clone();
                    Callback::from(move |_: MouseEvent| s.set(true))
                }}>{"‹"}</button>
            }
        </div>
    }
}

// ─── Helper Components ──────────────────────────────────────────────

#[derive(Properties, PartialEq)]
struct SettingCheckboxProps {
    label: String,
    checked: bool,
    on_change: Callback<()>,
    #[prop_or(false)]
    is_top: bool,
}

#[function_component(SettingCheckbox)]
fn setting_checkbox(props: &SettingCheckboxProps) -> Html {
    let onclick = {
        let cb = props.on_change.clone();
        Callback::from(move |_: MouseEvent| cb.emit(()))
    };
    html! {
        <label class={classes!("menu-label", props.is_top.then_some("menu-label-top"))}>
            <input type="checkbox" checked={props.checked} onclick={onclick} />
            {&props.label}
        </label>
    }
}

// ─── BOM Data Helpers ───────────────────────────────────────────────

#[derive(Clone)]
enum BomEntry {
    Component {
        refs: Vec<BomRef>,
        fields: Vec<String>,
    },
    Net {
        name: String,
    },
}

fn get_bom_entries(data: &PcbData, settings: &Settings, filter: &str) -> Vec<BomEntry> {
    if settings.bom_mode == "netlist" {
        if let Some(ref nets) = data.nets {
            return nets
                .iter()
                .filter(|n| filter.is_empty() || n.to_lowercase().contains(filter))
                .map(|n| BomEntry::Net { name: n.clone() })
                .collect();
        }
        return Vec::new();
    }

    let bom = match &data.bom {
        Some(b) => b,
        None => return Vec::new(),
    };

    let groups = &bom.both;

    let mut entries: Vec<BomEntry> = if settings.bom_mode == "ungrouped" {
        groups
            .iter()
            .flat_map(|group| {
                group.iter().map(|ref_| {
                    let fields = get_fields_for_ref(ref_.1, bom);
                    BomEntry::Component {
                        refs: vec![ref_.clone()],
                        fields,
                    }
                })
            })
            .collect()
    } else {
        groups
            .iter()
            .map(|group| {
                let fields = if let Some(first) = group.first() {
                    get_fields_for_ref(first.1, bom)
                } else {
                    Vec::new()
                };
                BomEntry::Component {
                    refs: group.clone(),
                    fields,
                }
            })
            .collect()
    };

    if !filter.is_empty() {
        entries.retain(|e| match e {
            BomEntry::Component { refs, fields } => {
                refs.iter().any(|r| r.0.to_lowercase().contains(filter))
                    || fields.iter().any(|f| f.to_lowercase().contains(filter))
            }
            BomEntry::Net { name } => name.to_lowercase().contains(filter),
        });
    }

    entries
}

fn get_fields_for_ref(fp_idx: usize, bom: &BomData) -> Vec<String> {
    let key = fp_idx.to_string();
    if let Some(fields) = bom.fields.get(&key) {
        fields
            .iter()
            .map(|v| match v {
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            })
            .collect()
    } else {
        Vec::new()
    }
}
