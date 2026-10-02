mod pcbdata;
mod render;
mod state;

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

use gloo::events::EventListener;
use vector_view::input::{Input, InputConfig, InputEvent as ViewEvent, InputOutcome, WheelMode};
use vector_view::render::PathCache;
use wasm_bindgen::JsCast;
use web_sys::{HtmlCanvasElement, HtmlElement, HtmlInputElement};
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
    canvases: Canvases,
    colors: Colors,
    board: Rc<Board>,
    viewport: Viewport,
    input: Input,
    cache: PathCache,
    side: &'static str,
}

impl ViewerState {
    fn frame<'a>(
        &'a self,
        data: &'a PcbData,
        settings: &'a Settings,
        hl: &'a [usize],
        mf: &'a HashSet<usize>,
        hn: &Option<String>,
        dnp: &'a HashSet<usize>,
    ) -> Frame<'a> {
        let _ = data;
        Frame {
            board: &self.board,
            colors: &self.colors,
            settings,
            side: self.side,
            highlighted_footprints: hl,
            marked_footprints: mf,
            highlighted_net: hn.as_deref().and_then(|n| self.board.net_id(n)),
            dnp,
        }
    }

    fn redraw(
        &mut self,
        data: &PcbData,
        settings: &Settings,
        hl: &[usize],
        mf: &HashSet<usize>,
        hn: &Option<String>,
    ) {
        let dnp = dnp_set(data);
        let mut cache = std::mem::take(&mut self.cache);
        {
            let frame = self.frame(data, settings, hl, mf, hn, &dnp);
            frame.draw_background(&self.canvases, &self.viewport, &mut cache);
            frame.draw_highlights(&self.canvases, &self.viewport, &mut cache);
        }
        self.cache = cache;
    }

    fn redraw_highlights(
        &mut self,
        data: &PcbData,
        settings: &Settings,
        hl: &[usize],
        mf: &HashSet<usize>,
        hn: &Option<String>,
    ) {
        let dnp = dnp_set(data);
        let mut cache = std::mem::take(&mut self.cache);
        {
            let frame = self.frame(data, settings, hl, mf, hn, &dnp);
            frame.draw_highlights(&self.canvases, &self.viewport, &mut cache);
        }
        self.cache = cache;
    }
}

fn dnp_set(data: &PcbData) -> HashSet<usize> {
    data.bom
        .as_ref()
        .map(|b| b.skipped.iter().copied().collect())
        .unwrap_or_default()
}

/// Board geometry for the canvas, built from the same JSON as the BOM data.
fn build_board(text: &str) -> Board {
    match serde_json::from_str::<pcb_extract::types::PcbData>(text) {
        Ok(pcb) => Board::new(pcb_extract::scene::to_scene(&pcb)),
        Err(e) => {
            log::error!("board geometry unavailable: {e}");
            Board::new(vector_view::Scene::new(vector_view::SceneKind::Pcb, true))
        }
    }
}

fn now_ms() -> f64 {
    js_sys::Date::now()
}

// ─── App Component ──────────────────────────────────────────────────

#[function_component(App)]
fn app() -> Html {
    let pcbdata: UseStateHandle<Option<Rc<PcbData>>> = use_state(|| None);
    let board: UseStateHandle<Option<Rc<Board>>> = use_state(|| None);
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
        let board = board.clone();
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
                                        board.set(Some(Rc::new(build_board(&text))));
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
        let board = board.clone();
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
                if let (Some(data), Some(board)) = (&*pcbdata, &*board) {
                    let side = if *board_flipped { "B" } else { "F" };

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

                        let canvases = Canvases {
                            bg: get_canvas("bg"),
                            fab: get_canvas("fab"),
                            silk: get_canvas("slk"),
                            highlight: get_canvas("hl"),
                        };

                        let vs = Rc::new(RefCell::new(ViewerState {
                            canvases,
                            colors,
                            board: board.clone(),
                            viewport: Viewport::default(),
                            input: Input::new(InputConfig {
                                wheel: WheelMode::PanUnlessCtrl,
                                ..InputConfig::default()
                            }),
                            cache: PathCache::new(),
                            side,
                        }));

                        viewer_state.set(Some(vs.clone()));
                        vs
                    } else {
                        viewer_state.as_ref().unwrap().clone()
                    };

                    let mut vs = state.borrow_mut();
                    if vs.side != side {
                        // Keep the board point at the viewport centre when flipping.
                        vs.viewport.flip_pan();
                        vs.side = side;
                    }

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
                            let width = el.client_width() as f64;
                            let height = el.client_height() as f64;
                            if width > 0.0 && height > 0.0 {
                                let flipped = *board_flipped;
                                let ViewerState {
                                    ref canvases,
                                    ref mut viewport,
                                    ref board,
                                    ..
                                } = *vs;
                                viewport.dpr = dpr;
                                viewport.refit(board, width, height, &settings, flipped);
                                canvases.resize(width, height, dpr);
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

    // Window resize handler. Re-registered when redraw_trigger changes so the
    // closure always captures the current value (otherwise it would forever
    // set 1 and stop triggering redraws after the first resize).
    {
        let redraw_trigger = redraw_trigger.clone();
        use_effect_with(*redraw_trigger, move |_| {
            let listener = EventListener::new(&web_sys::window().unwrap(), "resize", move |_| {
                redraw_trigger.set(*redraw_trigger + 1);
            });
            move || drop(listener)
        });
    }

    // Canvas event handlers: DOM events become vector-view input events.
    let on_canvas_input = {
        let viewer_state = viewer_state.clone();
        let pcbdata = pcbdata.clone();
        let settings = settings.clone();
        let highlighted_footprints = highlighted_footprints.clone();
        let highlighted_net = highlighted_net.clone();
        let marked_footprints = marked_footprints.clone();
        let current_row = current_row.clone();
        let filter = filter.clone();

        Callback::from(move |ev: ViewEvent| {
            let (Some(state), Some(data)) = ((*viewer_state).as_ref(), (*pcbdata).as_ref()) else {
                return;
            };
            let mut vs = state.borrow_mut();
            let outcome = {
                let ViewerState {
                    ref mut input,
                    ref mut viewport,
                    ..
                } = *vs;
                input.handle(&ev, &mut viewport.user)
            };
            let redraw = |vs: &mut ViewerState| {
                let hl = (*highlighted_footprints).clone();
                let hn = (*highlighted_net).clone();
                let mf = (*marked_footprints).clone();
                vs.redraw(data, &settings, &hl, &mf, &hn);
            };
            match outcome {
                InputOutcome::ViewChanged => {
                    let wheel = matches!(ev, ViewEvent::Wheel { .. });
                    if wheel || settings.redraw_on_drag {
                        redraw(&mut vs);
                    }
                }
                InputOutcome::Reset => {
                    vs.viewport.reset_user();
                    redraw(&mut vs);
                }
                InputOutcome::Tap { x, y, button: 0 } => {
                    // Decide net vs. component selection from the locally
                    // computed hit, not the deferred highlighted_net handle.
                    match pick(
                        &vs.board,
                        &vs.viewport,
                        x,
                        y,
                        vs.side,
                        &settings,
                        data.nets.is_some(),
                    ) {
                        Pick::Net(net) => {
                            // A net is under the cursor — highlight it.
                            if Some(&net) != highlighted_net.as_ref() {
                                highlighted_net.set(Some(net));
                                highlighted_footprints.set(Vec::new());
                                current_row.set(None);
                            }
                        }
                        Pick::Footprints(fps) => {
                            // Find matching BOM row for the clicked component
                            let bom_entries =
                                get_bom_entries(data, &settings, &filter.to_lowercase());
                            let row_id = fps.first().and_then(|&fp_idx| {
                                bom_entries.iter().find_map(|entry| {
                                    if let BomEntry::Component { refs, .. } = entry {
                                        if refs.iter().any(|r| r.1 == fp_idx) {
                                            Some(bom_row_id(entry))
                                        } else {
                                            None
                                        }
                                    } else {
                                        None
                                    }
                                })
                            });
                            current_row.set(row_id);
                            highlighted_footprints.set(fps);
                            highlighted_net.set(None);
                        }
                        Pick::Nothing => {
                            if highlighted_net.is_some() {
                                // Clicked empty space with a net highlighted — clear it.
                                highlighted_net.set(None);
                                highlighted_footprints.set(Vec::new());
                                current_row.set(None);
                            }
                        }
                    }
                }
                InputOutcome::Tap { .. } | InputOutcome::None => {
                    let up = matches!(ev, ViewEvent::PointerUp { .. });
                    if up && !settings.redraw_on_drag {
                        redraw(&mut vs);
                    }
                }
            }
        })
    };

    let on_canvas_wheel = {
        let on_input = on_canvas_input.clone();
        Callback::from(move |e: WheelEvent| {
            e.prevent_default();
            on_input.emit(ViewEvent::Wheel {
                x: e.offset_x() as f64,
                y: e.offset_y() as f64,
                dx: e.delta_x(),
                dy: e.delta_y(),
                delta_mode: e.delta_mode(),
                ctrl: e.ctrl_key(),
            });
        })
    };

    let on_canvas_pointerdown = {
        let on_input = on_canvas_input.clone();
        Callback::from(move |e: PointerEvent| {
            e.prevent_default();
            if let Some(canvas) = e.target().and_then(|t| t.dyn_into::<HtmlElement>().ok()) {
                let _ = canvas.set_pointer_capture(e.pointer_id());
            }
            on_input.emit(ViewEvent::PointerDown {
                id: e.pointer_id(),
                x: e.offset_x() as f64,
                y: e.offset_y() as f64,
                button: e.button(),
                time_ms: now_ms(),
            });
        })
    };

    let on_canvas_pointermove = {
        let on_input = on_canvas_input.clone();
        let viewer_state = viewer_state.clone();
        Callback::from(move |e: PointerEvent| {
            let pressed = (*viewer_state)
                .as_ref()
                .is_some_and(|s| s.borrow().input.active_pointers() > 0);
            if !pressed {
                return;
            }
            e.prevent_default();
            on_input.emit(ViewEvent::PointerMove {
                id: e.pointer_id(),
                x: e.offset_x() as f64,
                y: e.offset_y() as f64,
            });
        })
    };

    let on_canvas_pointerup = {
        let on_input = on_canvas_input.clone();
        Callback::from(move |e: PointerEvent| {
            on_input.emit(ViewEvent::PointerUp {
                id: e.pointer_id(),
                x: e.offset_x() as f64,
                y: e.offset_y() as f64,
                button: e.button(),
                time_ms: now_ms(),
            });
        })
    };

    let on_canvas_pointercancel = {
        let on_input = on_canvas_input.clone();
        Callback::from(move |e: PointerEvent| {
            on_input.emit(ViewEvent::PointerCancel { id: e.pointer_id() });
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

    // Scroll BOM table to the current row when it changes
    {
        let row = (*current_row).clone();
        use_effect_with(row, move |row| {
            if let Some(id) = row {
                if let Some(window) = web_sys::window() {
                    if let Some(doc) = window.document() {
                        if let Some(el) = doc.get_element_by_id(id) {
                            el.scroll_into_view_with_bool(false);
                        }
                    }
                }
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
                "edge_cuts" => {
                    s.render_edge_cuts = value;
                    write_storage("edgeCutsVisible", &value.to_string(), &storage_prefix_str);
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

    let toggle_layer = {
        let settings = settings.clone();
        let storage_prefix_str = storage_prefix_str.clone();
        let redraw_trigger = redraw_trigger.clone();
        Callback::from(move |layer_name: String| {
            let mut s = (*settings).clone();
            if s.hidden_layers.contains(&layer_name) {
                s.hidden_layers.remove(&layer_name);
            } else {
                s.hidden_layers.insert(layer_name);
            }
            let layers_vec: Vec<&String> = s.hidden_layers.iter().collect();
            if let Ok(json) = serde_json::to_string(&layers_vec) {
                write_storage("hiddenLayers", &json, &storage_prefix_str);
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
        // The canvas effect mirrors the pan so the centred board point stays put.
        Callback::from(move |_: MouseEvent| {
            board_flipped.set(!*board_flipped);
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
    let inner_layer_names: Vec<String> = {
        use std::collections::BTreeSet;
        let mut names: BTreeSet<String> = BTreeSet::new();
        if let Some(ref t) = data.tracks {
            names.extend(t.inner_layer_names().into_iter().cloned());
        }
        if let Some(ref z) = data.zones {
            names.extend(z.inner_layer_names().into_iter().cloned());
        }
        names.into_iter().collect()
    };

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

            // ─── Pull tabs (outside sidebars for mobile accessibility) ──
                <button class={classes!("pull-tab", "pull-tab-left", (*bom_sidebar_open).then_some("tab-open"))} onclick={{
                    let s = bom_sidebar_open.clone();
                    let open = *bom_sidebar_open;
                    Callback::from(move |_: MouseEvent| s.set(!open))
                }}>{if *bom_sidebar_open { "\u{2039}" } else { "\u{203a}" }}</button>
                <button class={classes!("pull-tab", "pull-tab-right", (*view_sidebar_open).then_some("tab-open"))} onclick={{
                    let s = view_sidebar_open.clone();
                    let open = *view_sidebar_open;
                    Callback::from(move |_: MouseEvent| s.set(!open))
                }}>{if *view_sidebar_open { "\u{203a}" } else { "\u{2039}" }}</button>

            // ─── BOM sidebar (left) ────────────────────────────
                <div class={classes!("sidebar", "bom-sidebar", (!*bom_sidebar_open).then_some("sidebar-closed"))}>
                    <div class="sidebar-header">
                        <a class="back-btn" href="/" title="Back to PasteBOM" aria-label="Back to PasteBOM">{"\u{2039}"}</a>
                        <div class="sidebar-header-titles">
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
                                    let row_id = bom_row_id(entry);
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

            // ─── View sidebar (right) ──────────────────────────
                <div class={classes!("sidebar", "view-sidebar", (!*view_sidebar_open).then_some("sidebar-closed"))}>
                    <div class="sidebar-header">
                        <span class="sidebar-title">{"View"}</span>
                    </div>
                    <div class="sidebar-settings">
                        // ─── Layer color key ──────────────────────────
                        <div class="layer-key">
                            <div class="layer-key-title">{format!("Layers ({})", layer_label)}</div>
                            <LayerToggle
                                label={format!("{}.Cu (pads)", layer_prefix)}
                                color="var(--pad-color)"
                                checked={settings.render_pads}
                                on_change={{let ts = toggle_setting.clone(); let v = settings.render_pads; Callback::from(move |_| ts.emit(("pads".into(), !v)))}}
                            />
                            if has_tracks {
                                <LayerToggle
                                    label={format!("{}.Cu (tracks)", layer_prefix)}
                                    color={format!("var(--track-color-{})", if *board_flipped { "back" } else { "front" })}
                                    checked={settings.render_tracks}
                                    on_change={{let ts = toggle_setting.clone(); let v = settings.render_tracks; Callback::from(move |_| ts.emit(("tracks".into(), !v)))}}
                                />
                                <LayerToggle
                                    label={format!("{}.Cu (zones)", layer_prefix)}
                                    color={format!("var(--zone-color-{})", if *board_flipped { "back" } else { "front" })}
                                    checked={settings.render_zones}
                                    on_change={{let ts = toggle_setting.clone(); let v = settings.render_zones; Callback::from(move |_| ts.emit(("zones".into(), !v)))}}
                                />
                                {for inner_layer_names.iter().map(|name| {
                                    let tl = toggle_layer.clone();
                                    let n = name.clone();
                                    let visible = !settings.hidden_layers.contains(name);
                                    let color = format!("var(--track-color-{})", if *board_flipped { "back" } else { "front" });
                                    html! {
                                        <LayerToggle
                                            label={name.clone()}
                                            {color}
                                            checked={visible}
                                            on_change={Callback::from(move |_| tl.emit(n.clone()))}
                                            opacity={0.25}
                                        />
                                    }
                                })}
                            }
                            <LayerToggle
                                label={format!("{}.SilkS", layer_prefix)}
                                color="var(--silkscreen-edge-color)"
                                checked={settings.render_silkscreen}
                                on_change={{let ts = toggle_setting.clone(); let v = settings.render_silkscreen; Callback::from(move |_| ts.emit(("silkscreen".into(), !v)))}}
                            />
                            <LayerToggle
                                label={format!("{}.Fab", layer_prefix)}
                                color="var(--fabrication-edge-color)"
                                checked={settings.render_fabrication}
                                on_change={{let ts = toggle_setting.clone(); let v = settings.render_fabrication; Callback::from(move |_| ts.emit(("fabrication".into(), !v)))}}
                            />
                            <LayerToggle
                                label="Edge.Cuts"
                                color="var(--pcb-edge-color)"
                                checked={settings.render_edge_cuts}
                                on_change={{let ts = toggle_setting.clone(); let v = settings.render_edge_cuts; Callback::from(move |_| ts.emit(("edge_cuts".into(), !v)))}}
                            />
                        </div>
                        // ─── Settings ─────────────────────────────────
                        <SettingCheckbox label="Dark mode" checked={settings.dark_mode}
                            on_change={toggle_dark_mode.clone()} is_top={true} />
                        <SettingCheckbox label="References" checked={settings.render_references}
                            on_change={{let ts = toggle_setting.clone(); let v = settings.render_references; Callback::from(move |_| ts.emit(("references".into(), !v)))}}
                            is_top={false} />
                        <SettingCheckbox label="Values" checked={settings.render_values}
                            on_change={{let ts = toggle_setting.clone(); let v = settings.render_values; Callback::from(move |_| ts.emit(("values".into(), !v)))}}
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

            // ─── Version badge ───────────────────────────────────
            <span class="version-badge">{concat!("v", env!("CARGO_PKG_VERSION"))}</span>
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

#[derive(Properties, PartialEq)]
struct LayerToggleProps {
    label: AttrValue,
    color: AttrValue,
    checked: bool,
    on_change: Callback<()>,
    #[prop_or(1.0)]
    opacity: f64,
}

#[function_component(LayerToggle)]
fn layer_toggle(props: &LayerToggleProps) -> Html {
    let onclick = {
        let cb = props.on_change.clone();
        Callback::from(move |_: MouseEvent| cb.emit(()))
    };
    let swatch_style = if props.opacity < 1.0 {
        format!("background: {}; opacity: {};", props.color, props.opacity)
    } else {
        format!("background: {};", props.color)
    };
    html! {
        <label class="layer-toggle">
            <input type="checkbox" checked={props.checked} onclick={onclick} />
            <span class="layer-swatch" style={swatch_style}></span>
            <span class="layer-toggle-label">{&props.label}</span>
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

/// Stable DOM id for a BOM row, derived from the entry's identity rather than
/// its position in the (filterable) list, so highlighting survives search edits.
fn bom_row_id(entry: &BomEntry) -> String {
    match entry {
        BomEntry::Component { refs, .. } => {
            let mut idxs: Vec<usize> = refs.iter().map(|r| r.1).collect();
            idxs.sort_unstable();
            let key = idxs
                .iter()
                .map(|i| i.to_string())
                .collect::<Vec<_>>()
                .join("_");
            format!("bomrow-c{key}")
        }
        BomEntry::Net { name } => format!("bomrow-n{name}"),
    }
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
