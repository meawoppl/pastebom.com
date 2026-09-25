//! Embeddable Gerber / Excellon viewer.
//!
//! Parses fabrication files with `pcb-extract` and renders them to a canvas with
//! pan, zoom, per-layer visibility, colour, and top/bottom views. It has no
//! framework or application dependencies: a host page mounts it into any element.
//!
//! ```js
//! import init, { GerberViewer } from "./gerber_view.js";
//! await init();
//! const viewer = new GerberViewer({ controls: true });
//! viewer.mount(document.getElementById("gerbers"));
//! const project = await viewer.setSources([{ url: "/fab/board-F_Cu.gbr" }]);
//! ```

mod controls;
mod geometry;
mod scene;
mod sources;
mod style;
mod view;

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::{Rc, Weak};

use js_sys::{Array, Function, Promise, Reflect};
use pcb_extract::parsers::gerber::layers::{LayerFunction, LayerSide};
use pcb_extract::parsers::gerber::{board_bbox, parse_source, GerberLayer, GerberProject};
use pcb_extract::types::BBox;
use serde::{Deserialize, Serialize};
use serde_wasm_bindgen::Serializer;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::{future_to_promise, JsFuture};
use web_sys::{
    AddEventListenerOptions, CanvasRenderingContext2d, Element, Event, EventTarget,
    HtmlCanvasElement, HtmlElement, HtmlInputElement, PointerEvent, WheelEvent,
};

use crate::scene::{paint_order, LayerPaths};
use crate::style::{layer_label, LayerStyle, DEFAULT_BACKGROUND};
use crate::view::View;

/// Ctrl+wheel delta (in pixels) that zooms by a factor of e.
const WHEEL_ZOOM_PIXELS: f64 = 400.0;

/// Boards whose full render takes longer than this preview pan/zoom from a snapshot.
const FRAME_BUDGET_MS: f64 = 12.0;

/// Quiet period after the last pan/zoom before a heavy board is re-rendered sharply.
const IDLE_RENDER_DELAY_MS: i32 = 150;

#[derive(Debug, Clone, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Options {
    /// Canvas background colour (any CSS colour).
    background: String,
    /// Show the built-in layer panel.
    controls: bool,
    /// Padding around the board when fitting, in CSS pixels.
    padding: f64,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            background: DEFAULT_BACKGROUND.to_string(),
            controls: false,
            padding: 16.0,
        }
    }
}

struct Layer {
    layer: GerberLayer,
    /// Label from the layer's kind alone, e.g. "Top copper".
    kind_label: String,
    /// Displayed label: `kind_label`, plus the file name when several layers share it.
    label: String,
    style: LayerStyle,
    paths: LayerPaths,
}

impl Layer {
    /// Match on the full source name, the displayed label, or the kind label
    /// ("Top copper" matches every top copper layer).
    fn matches(&self, key: &str) -> bool {
        self.layer.name == key
            || self.label.eq_ignore_ascii_case(key)
            || self.kind_label.eq_ignore_ascii_case(key)
    }
}

/// Give layers that share a kind label a file-name suffix so they can be told apart.
fn disambiguate_labels(layers: &mut [Layer]) {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for layer in layers.iter() {
        *counts.entry(layer.kind_label.clone()).or_default() += 1;
    }
    for layer in layers.iter_mut() {
        layer.label = if counts[&layer.kind_label] > 1 {
            let file = layer
                .layer
                .name
                .rsplit(['/', '\\'])
                .next()
                .unwrap_or_default();
            if file == layer.kind_label {
                file.to_string()
            } else {
                format!("{} ({file})", layer.kind_label)
            }
        } else {
            layer.kind_label.clone()
        };
    }
}

type Listener = Closure<dyn FnMut(Event)>;

struct Dom {
    root: HtmlElement,
    canvas: HtmlCanvasElement,
    ctx: CanvasRenderingContext2d,
    scratch: HtmlCanvasElement,
    scratch_ctx: CanvasRenderingContext2d,
    panel: Option<HtmlElement>,
    listeners: Vec<(EventTarget, &'static str, Listener)>,
    resize_observer: Option<JsValue>,
    resize_callback: Option<Closure<dyn FnMut()>>,
    /// Copy of the last full-quality frame and the view it was drawn with. Boards
    /// too heavy to redraw every frame pan and zoom by transforming this copy.
    snapshot: HtmlCanvasElement,
    snapshot_ctx: CanvasRenderingContext2d,
    snapshot_view: Option<View>,
    /// Duration of the last full render, in milliseconds.
    full_render_ms: f64,
    frame_callback: Option<Closure<dyn FnMut()>>,
    frame_id: Option<i32>,
    idle_callback: Option<Closure<dyn FnMut()>>,
    idle_id: Option<i32>,
    /// Canvas size in CSS pixels.
    width: f64,
    height: f64,
    dpr: f64,
}

impl Dom {
    fn listen(
        &mut self,
        target: &EventTarget,
        event: &'static str,
        passive: bool,
        handler: impl FnMut(Event) + 'static,
    ) -> Result<(), JsValue> {
        let closure = Listener::new(handler);
        let opts = AddEventListenerOptions::new();
        opts.set_passive(passive);
        target.add_event_listener_with_callback_and_add_event_listener_options(
            event,
            closure.as_ref().unchecked_ref(),
            &opts,
        )?;
        self.listeners.push((target.clone(), event, closure));
        Ok(())
    }

    fn teardown(self) {
        for (target, event, closure) in &self.listeners {
            let _ =
                target.remove_event_listener_with_callback(event, closure.as_ref().unchecked_ref());
        }
        if let Some(observer) = &self.resize_observer {
            let _ = call_method(observer, "disconnect", &[]);
        }
        if let Some(window) = web_sys::window() {
            if let Some(id) = self.frame_id {
                let _ = window.cancel_animation_frame(id);
            }
            if let Some(id) = self.idle_id {
                window.clear_timeout_with_handle(id);
            }
        }
        self.root.remove();
    }
}

#[derive(Default)]
struct Inner {
    options: Options,
    layers: Vec<Layer>,
    warnings: Vec<String>,
    skipped: Vec<String>,
    bbox: Option<BBox>,
    view: View,
    from_bottom: bool,
    /// Refit whenever geometry or size changes, until the user pans or zooms.
    auto_fit: bool,
    /// Incremented by `setSources`/`clear` so loads from a superseded call are dropped.
    generation: u32,
    drag: Option<(i32, f64, f64)>,
    /// The next frame must be a full render rather than a snapshot preview.
    needs_full_render: bool,
    on_change: Option<Function>,
    dom: Option<Dom>,
}

impl Inner {
    fn add_source(&mut self, name: &str, data: &[u8]) {
        let parsed = parse_source(name, data);
        self.warnings.extend(parsed.warnings);
        self.skipped.extend(parsed.skipped);
        for layer in parsed.layers {
            let paths = match LayerPaths::build(&layer) {
                Ok(paths) => paths,
                Err(e) => {
                    self.warnings
                        .push(format!("{}: {}", layer.name, js_error_text(&e)));
                    continue;
                }
            };
            let order = layer.layer_type.stack_order();
            let pos = self
                .layers
                .partition_point(|l| l.layer.layer_type.stack_order() <= order);
            let kind_label = layer_label(&layer.layer_type, &layer.name);
            self.layers.insert(
                pos,
                Layer {
                    label: kind_label.clone(),
                    kind_label,
                    style: LayerStyle::default_for(&layer.layer_type, &self.options.background),
                    paths,
                    layer,
                },
            );
        }
        disambiguate_labels(&mut self.layers);
        self.bbox = board_bbox(self.layers.iter().map(|l| &l.layer));
        self.refresh();
    }

    fn clear(&mut self) {
        self.layers.clear();
        self.warnings.clear();
        self.skipped.clear();
        self.bbox = None;
        self.auto_fit = true;
        self.generation = self.generation.wrapping_add(1);
        self.refresh();
    }

    /// Match the canvas backing stores to the root element's current size.
    fn sync_size(&mut self) {
        let Some(dom) = self.dom.as_mut() else {
            return;
        };
        let dpr = web_sys::window()
            .map(|w| w.device_pixel_ratio())
            .unwrap_or(1.0);
        let width = f64::from(dom.root.client_width());
        let height = f64::from(dom.root.client_height());
        if width == dom.width && height == dom.height && dpr == dom.dpr {
            return;
        }
        dom.width = width;
        dom.height = height;
        dom.dpr = dpr;
        let (bw, bh) = ((width * dpr).round() as u32, (height * dpr).round() as u32);
        for canvas in [&dom.canvas, &dom.scratch, &dom.snapshot] {
            canvas.set_width(bw);
            canvas.set_height(bh);
        }
        dom.snapshot_view = None;
    }

    fn fit(&mut self) {
        if let (Some(bbox), Some(dom)) = (&self.bbox, &self.dom) {
            if dom.width > 0.0 && dom.height > 0.0 {
                self.view
                    .fit(bbox, dom.width, dom.height, self.options.padding);
            }
        }
    }

    fn set_side(&mut self, from_bottom: bool) {
        let width = self.dom.as_ref().map_or(0.0, |d| d.width);
        self.from_bottom = from_bottom;
        self.view.set_mirrored(from_bottom, width);
        self.refresh();
    }

    fn refresh(&mut self) {
        if self.auto_fit {
            self.fit();
        }
        self.invalidate();
        self.update_panel();
    }

    /// Content or styling changed: schedule a full-quality render.
    fn invalidate(&mut self) {
        self.needs_full_render = true;
        self.schedule_frame();
    }

    /// The view moved interactively. Heavy boards preview by transforming the last
    /// full frame and re-render at full quality once interaction pauses.
    fn view_changed(&mut self) {
        let can_preview = self.dom.as_ref().is_some_and(|d| {
            d.full_render_ms > FRAME_BUDGET_MS
                && d.snapshot_view
                    .is_some_and(|v| v.mirrored == self.view.mirrored)
        });
        if can_preview {
            self.schedule_frame();
            self.schedule_idle_render();
        } else {
            self.invalidate();
        }
    }

    fn schedule_frame(&mut self) {
        let Some(dom) = self.dom.as_mut() else {
            return;
        };
        if dom.frame_id.is_some() {
            return;
        }
        if let (Some(window), Some(callback)) = (web_sys::window(), &dom.frame_callback) {
            dom.frame_id = window
                .request_animation_frame(callback.as_ref().unchecked_ref())
                .ok();
        }
    }

    fn schedule_idle_render(&mut self) {
        let Some(dom) = self.dom.as_mut() else {
            return;
        };
        let (Some(window), Some(callback)) = (web_sys::window(), &dom.idle_callback) else {
            return;
        };
        if let Some(id) = dom.idle_id.take() {
            window.clear_timeout_with_handle(id);
        }
        dom.idle_id = window
            .set_timeout_with_callback_and_timeout_and_arguments_0(
                callback.as_ref().unchecked_ref(),
                IDLE_RENDER_DELAY_MS,
            )
            .ok();
    }

    fn on_idle(&mut self) {
        if let Some(dom) = self.dom.as_mut() {
            dom.idle_id = None;
        }
        self.invalidate();
    }

    fn on_frame(&mut self) {
        let full = self.needs_full_render;
        let Some(dom) = self.dom.as_mut() else {
            return;
        };
        dom.frame_id = None;
        let result = if full || dom.snapshot_view.is_none() {
            self.needs_full_render = false;
            self.render_full()
        } else {
            self.render_preview()
        };
        if let Err(e) = result {
            web_sys::console::error_2(&"gerber-view: render failed".into(), &e);
        }
    }

    /// Draw the last full frame, scaled and shifted from its view to the current one.
    fn render_preview(&self) -> Result<(), JsValue> {
        let Some(dom) = &self.dom else {
            return Ok(());
        };
        let Some(snap) = dom.snapshot_view else {
            return Ok(());
        };
        let (w, h) = (
            f64::from(dom.canvas.width()),
            f64::from(dom.canvas.height()),
        );
        // A board point at snapshot screen position p lands at (p - t_snap) * k + t.
        let k = self.view.scale / snap.scale;
        let dx = (self.view.tx - snap.tx * k) * dom.dpr;
        let dy = (self.view.ty - snap.ty * k) * dom.dpr;

        let ctx = &dom.ctx;
        ctx.set_transform(1.0, 0.0, 0.0, 1.0, 0.0, 0.0)?;
        ctx.set_global_alpha(1.0);
        ctx.set_fill_style_str(&self.options.background);
        ctx.fill_rect(0.0, 0.0, w, h);
        ctx.draw_image_with_html_canvas_element_and_dw_and_dh(&dom.snapshot, dx, dy, w * k, h * k)
    }

    fn render_full(&mut self) -> Result<(), JsValue> {
        let started = js_sys::Date::now();
        self.draw_layers()?;
        let view = self.view;
        let Some(dom) = self.dom.as_mut() else {
            return Ok(());
        };
        dom.snapshot_ctx.clear_rect(
            0.0,
            0.0,
            f64::from(dom.snapshot.width()),
            f64::from(dom.snapshot.height()),
        );
        dom.snapshot_ctx
            .draw_image_with_html_canvas_element(&dom.canvas, 0.0, 0.0)?;
        dom.snapshot_view = Some(view);
        dom.full_render_ms = js_sys::Date::now() - started;
        Ok(())
    }

    fn draw_layers(&self) -> Result<(), JsValue> {
        let Some(dom) = &self.dom else {
            return Ok(());
        };
        let (w, h) = (
            f64::from(dom.canvas.width()),
            f64::from(dom.canvas.height()),
        );
        let ctx = &dom.ctx;
        ctx.set_transform(1.0, 0.0, 0.0, 1.0, 0.0, 0.0)?;
        ctx.set_global_alpha(1.0);
        ctx.set_fill_style_str(&self.options.background);
        ctx.fill_rect(0.0, 0.0, w, h);

        let [a, b, c, d, e, f] = self.view.canvas_transform(dom.dpr);
        // Thinnest stroke drawn: one CSS pixel, in board units.
        let min_width = 1.0 / self.view.scale;

        let mut visible: Vec<&Layer> = self.layers.iter().filter(|l| l.style.visible).collect();
        visible.sort_by_key(|l| paint_order(&l.layer.layer_type, self.from_bottom));

        let sctx = &dom.scratch_ctx;
        for layer in visible {
            // Each layer is drawn opaque into the scratch canvas, then composited with
            // its opacity, so overlapping shapes within a layer don't double up.
            sctx.set_transform(1.0, 0.0, 0.0, 1.0, 0.0, 0.0)?;
            sctx.clear_rect(0.0, 0.0, w, h);
            sctx.set_transform(a, b, c, d, e, f)?;
            layer.paths.draw(sctx, &layer.style.color, min_width);

            ctx.set_global_alpha(layer.style.opacity.clamp(0.0, 1.0));
            ctx.draw_image_with_html_canvas_element(&dom.scratch, 0.0, 0.0)?;
        }
        ctx.set_global_alpha(1.0);
        Ok(())
    }

    fn update_panel(&self) {
        let Some(panel) = self.dom.as_ref().and_then(|d| d.panel.as_ref()) else {
            return;
        };
        let rows: Vec<controls::PanelRow> = self
            .layers
            .iter()
            .map(|l| controls::PanelRow {
                label: &l.label,
                name: &l.layer.name,
                color: &l.style.color,
                visible: l.style.visible,
            })
            .collect();
        panel.set_inner_html(&controls::panel_html(&rows, self.from_bottom));
    }

    /// Apply `f` to every layer matching `key`; returns whether any matched.
    fn update_layers(&mut self, key: &str, f: impl Fn(&mut LayerStyle)) -> bool {
        let mut found = false;
        for layer in self.layers.iter_mut().filter(|l| l.matches(key)) {
            f(&mut layer.style);
            found = true;
        }
        if found {
            self.invalidate();
            self.update_panel();
        }
        found
    }

    fn summary(&self) -> Result<JsValue, JsValue> {
        let summary = ProjectSummary {
            layers: self
                .layers
                .iter()
                .map(|l| LayerSummary {
                    name: &l.layer.name,
                    label: &l.label,
                    function: l.layer.function,
                    side: l.layer.side,
                    inner: l.layer.inner.as_deref(),
                    color: &l.style.color,
                    opacity: l.style.opacity,
                    visible: l.style.visible,
                    drawing_count: l.layer.drawings.len(),
                    clear_drawing_count: l.layer.clear_drawings.len(),
                    bbox: l.layer.bbox.as_ref(),
                })
                .collect(),
            bbox: self.bbox.as_ref(),
            warnings: &self.warnings,
            skipped: &self.skipped,
            side: if self.from_bottom { "bottom" } else { "top" },
        };
        Ok(summary.serialize(&Serializer::json_compatible())?)
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ProjectSummary<'a> {
    layers: Vec<LayerSummary<'a>>,
    bbox: Option<&'a BBox>,
    warnings: &'a [String],
    skipped: &'a [String],
    side: &'static str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LayerSummary<'a> {
    name: &'a str,
    label: &'a str,
    function: LayerFunction,
    side: Option<LayerSide>,
    inner: Option<&'a str>,
    color: &'a str,
    opacity: f64,
    visible: bool,
    drawing_count: usize,
    clear_drawing_count: usize,
    bbox: Option<&'a BBox>,
}

/// Call the `onChange` callback with a fresh summary. Must be called with no
/// outstanding borrow of `inner`, since the callback may call back into the viewer.
fn notify(inner: &Rc<RefCell<Inner>>) {
    let (callback, summary) = {
        let state = inner.borrow();
        (state.on_change.clone(), state.summary())
    };
    if let (Some(callback), Ok(summary)) = (callback, summary) {
        if let Err(e) = callback.call1(&JsValue::NULL, &summary) {
            web_sys::console::error_2(&"gerber-view: onChange callback threw".into(), &e);
        }
    }
}

fn call_method(target: &JsValue, name: &str, args: &[&JsValue]) -> Result<JsValue, JsValue> {
    let method: Function = Reflect::get(target, &name.into())?.dyn_into()?;
    let args: Array = args.iter().copied().collect();
    method.apply(target, &args)
}

fn js_error_text(e: &JsValue) -> String {
    e.as_string()
        .or_else(|| e.dyn_ref::<js_sys::Error>().map(|e| e.message().into()))
        .unwrap_or_else(|| format!("{e:?}"))
}

/// Embeddable Gerber viewer. Create one, `mount` it into an element with a size,
/// then give it sources with `setSources`.
#[wasm_bindgen]
pub struct GerberViewer {
    inner: Rc<RefCell<Inner>>,
}

#[wasm_bindgen]
impl GerberViewer {
    /// Options (all optional): `{ background: "#12161c", controls: false, padding: 16 }`.
    #[wasm_bindgen(constructor)]
    pub fn new(options: JsValue) -> Result<GerberViewer, JsValue> {
        let options: Options = if options.is_undefined() || options.is_null() {
            Options::default()
        } else {
            serde_wasm_bindgen::from_value(options)?
        };
        Ok(Self {
            inner: Rc::new(RefCell::new(Inner {
                options,
                auto_fit: true,
                ..Default::default()
            })),
        })
    }

    /// Render into `container`, which should have a non-zero size. The viewer fills
    /// it and tracks its size. Mounting again moves the viewer to the new container.
    pub fn mount(&self, container: HtmlElement) -> Result<(), JsValue> {
        self.unmount();
        let document = container
            .owner_document()
            .ok_or("container is not in a document")?;
        let create = |tag: &str| -> Result<HtmlElement, JsValue> {
            Ok(document.create_element(tag)?.dyn_into()?)
        };

        let root = create("div")?;
        root.set_class_name("gv-root");
        root.style()
            .set_css_text("position:relative;width:100%;height:100%;overflow:hidden;");
        let canvas: HtmlCanvasElement = create("canvas")?.dyn_into()?;
        canvas.set_class_name("gv-canvas");
        canvas
            .style()
            .set_css_text("display:block;width:100%;height:100%;touch-action:none;cursor:grab;");
        root.append_child(&canvas)?;
        let scratch: HtmlCanvasElement = create("canvas")?.dyn_into()?;
        let snapshot: HtmlCanvasElement = create("canvas")?.dyn_into()?;

        let panel = if self.inner.borrow().options.controls {
            let panel = create("div")?;
            panel.set_class_name("gv-controls");
            panel.style().set_css_text(controls::PANEL_STYLE);
            root.append_child(&panel)?;
            Some(panel)
        } else {
            None
        };
        container.append_child(&root)?;

        let context = |c: &HtmlCanvasElement| -> Result<CanvasRenderingContext2d, JsValue> {
            Ok(c.get_context("2d")?
                .ok_or("canvas 2d context unavailable")?
                .dyn_into()?)
        };
        let weak_frame = Rc::downgrade(&self.inner);
        let weak_idle = Rc::downgrade(&self.inner);
        let mut dom = Dom {
            ctx: context(&canvas)?,
            scratch_ctx: context(&scratch)?,
            snapshot_ctx: context(&snapshot)?,
            root,
            canvas,
            scratch,
            snapshot,
            snapshot_view: None,
            full_render_ms: 0.0,
            frame_callback: Some(Closure::new(move || {
                with_state(&weak_frame, Inner::on_frame);
            })),
            frame_id: None,
            idle_callback: Some(Closure::new(move || {
                with_state(&weak_idle, Inner::on_idle);
            })),
            idle_id: None,
            panel,
            listeners: Vec::new(),
            resize_observer: None,
            resize_callback: None,
            width: 0.0,
            height: 0.0,
            dpr: 0.0,
        };
        self.attach_listeners(&mut dom)?;

        let mut state = self.inner.borrow_mut();
        state.dom = Some(dom);
        state.sync_size();
        state.refresh();
        Ok(())
    }

    /// Replace all layers with `sources`: a File, `{ name, content }`, `{ name?, url }`,
    /// or an iterable of those (Array, FileList). Layers appear as each source finishes
    /// loading. Resolves with the project summary once every source has been handled;
    /// per-source failures are reported in `warnings` rather than rejecting.
    #[wasm_bindgen(js_name = setSources)]
    pub fn set_sources(&self, sources: JsValue) -> Promise {
        self.inner.borrow_mut().clear();
        notify(&self.inner);
        self.load(sources)
    }

    /// Add `sources` to the current layers. Accepts the same forms as `setSources`.
    #[wasm_bindgen(js_name = addSources)]
    pub fn add_sources(&self, sources: JsValue) -> Promise {
        self.load(sources)
    }

    /// Remove all layers and diagnostics. Pending loads from earlier calls are discarded.
    pub fn clear(&self) {
        self.inner.borrow_mut().clear();
        notify(&self.inner);
    }

    /// Show or hide layers by source name or label (e.g. "Top copper").
    /// Returns whether any layer matched.
    #[wasm_bindgen(js_name = setLayerVisibility)]
    pub fn set_layer_visibility(&self, layer: &str, visible: bool) -> bool {
        self.update_layers(layer, |s| s.visible = visible)
    }

    /// Set a layer's colour (any CSS colour) by source name or label.
    #[wasm_bindgen(js_name = setLayerColor)]
    pub fn set_layer_color(&self, layer: &str, color: &str) -> bool {
        self.update_layers(layer, |s| s.color = color.to_string())
    }

    /// Set a layer's opacity (0 to 1) by source name or label.
    #[wasm_bindgen(js_name = setLayerOpacity)]
    pub fn set_layer_opacity(&self, layer: &str, opacity: f64) -> bool {
        self.update_layers(layer, |s| s.opacity = opacity.clamp(0.0, 1.0))
    }

    /// View the board from the `"top"` or `"bottom"` (mirrored, stack reversed).
    #[wasm_bindgen(js_name = setSide)]
    pub fn set_side(&self, side: &str) -> Result<(), JsValue> {
        let from_bottom = match side {
            "top" => false,
            "bottom" => true,
            other => {
                return Err(format!("side must be \"top\" or \"bottom\", got {other:?}").into())
            }
        };
        self.inner.borrow_mut().set_side(from_bottom);
        notify(&self.inner);
        Ok(())
    }

    /// Current viewing side: `"top"` or `"bottom"`.
    pub fn side(&self) -> String {
        if self.inner.borrow().from_bottom {
            "bottom"
        } else {
            "top"
        }
        .to_string()
    }

    /// Fit the board in the viewport. The view keeps refitting on resize and new
    /// layers until the user pans or zooms.
    pub fn fit(&self) {
        let mut state = self.inner.borrow_mut();
        state.auto_fit = true;
        state.refresh();
    }

    /// Re-read the container size. Only needed where `ResizeObserver` is unavailable
    /// and the container changes size without a window resize.
    pub fn resize(&self) {
        let mut state = self.inner.borrow_mut();
        state.sync_size();
        state.refresh();
    }

    /// Summary of the loaded project: layers (name, label, function, side, colour,
    /// visibility, counts, bbox), board bbox in millimetres, `warnings` (problems),
    /// `skipped` (non-fabrication or empty files), and view side.
    pub fn project(&self) -> Result<JsValue, JsValue> {
        self.inner.borrow().summary()
    }

    /// Full parsed geometry as a JSON string, in the same shape `parseSources` returns.
    #[wasm_bindgen(js_name = exportProject)]
    pub fn export_project(&self) -> Result<String, JsValue> {
        let state = self.inner.borrow();
        let project = GerberProject {
            layers: state.layers.iter().map(|l| l.layer.clone()).collect(),
            bbox: state.bbox.clone(),
            warnings: state.warnings.clone(),
            skipped: state.skipped.clone(),
        };
        serde_json::to_string(&project).map_err(|e| e.to_string().into())
    }

    /// Register a callback invoked with the project summary whenever layers, styles,
    /// or the viewing side change. Pass `null` to remove it.
    #[wasm_bindgen(js_name = onChange)]
    pub fn on_change(&self, callback: Option<Function>) {
        self.inner.borrow_mut().on_change = callback;
    }

    /// Unmount from the DOM and drop all layers. Call `free()` afterwards to release
    /// the WebAssembly memory held by this object.
    pub fn destroy(&self) {
        self.unmount();
        let mut state = self.inner.borrow_mut();
        state.clear();
        state.on_change = None;
    }
}

impl GerberViewer {
    fn unmount(&self) {
        let dom = self.inner.borrow_mut().dom.take();
        if let Some(dom) = dom {
            dom.teardown();
        }
    }

    fn update_layers(&self, key: &str, f: impl Fn(&mut LayerStyle)) -> bool {
        let found = self.inner.borrow_mut().update_layers(key, f);
        if found {
            notify(&self.inner);
        }
        found
    }

    fn load(&self, sources: JsValue) -> Promise {
        let list = match sources::to_list(&sources) {
            Ok(list) => list,
            Err(e) => return Promise::reject(&e),
        };
        let generation = self.inner.borrow().generation;

        let pending: Array = list
            .into_iter()
            .map(|source| {
                let weak = Rc::downgrade(&self.inner);
                JsValue::from(future_to_promise(async move {
                    let loaded = sources::load(source).await;
                    if let Some(inner) = weak.upgrade() {
                        let current = {
                            let mut state = inner.borrow_mut();
                            let current = state.generation == generation;
                            if current {
                                match loaded {
                                    Ok((name, bytes)) => state.add_source(&name, &bytes),
                                    Err(e) => state.warnings.push(e),
                                }
                            }
                            current
                        };
                        if current {
                            notify(&inner);
                        }
                    }
                    Ok(JsValue::UNDEFINED)
                }))
            })
            .collect();

        let weak = Rc::downgrade(&self.inner);
        future_to_promise(async move {
            JsFuture::from(Promise::all(&pending)).await?;
            let inner = weak.upgrade().ok_or("viewer was dropped")?;
            let summary = inner.borrow().summary();
            summary
        })
    }

    fn attach_listeners(&self, dom: &mut Dom) -> Result<(), JsValue> {
        let canvas: EventTarget = dom.canvas.clone().into();

        let weak = Rc::downgrade(&self.inner);
        dom.listen(&canvas, "pointerdown", true, move |e| {
            let e: &PointerEvent = e.unchecked_ref();
            if e.button() != 0 {
                return;
            }
            with_state(&weak, |s| {
                s.drag = Some((
                    e.pointer_id(),
                    f64::from(e.client_x()),
                    f64::from(e.client_y()),
                ));
                if let Some(dom) = &s.dom {
                    let _ = dom.canvas.set_pointer_capture(e.pointer_id());
                    let _ = dom.canvas.style().set_property("cursor", "grabbing");
                }
            });
        })?;

        let weak = Rc::downgrade(&self.inner);
        dom.listen(&canvas, "pointermove", true, move |e| {
            let e: &PointerEvent = e.unchecked_ref();
            with_state(&weak, |s| {
                if let Some((id, x, y)) = s.drag {
                    if id == e.pointer_id() {
                        let (nx, ny) = (f64::from(e.client_x()), f64::from(e.client_y()));
                        s.view.pan(nx - x, ny - y);
                        s.drag = Some((id, nx, ny));
                        s.auto_fit = false;
                        s.view_changed();
                    }
                }
            });
        })?;

        for event in ["pointerup", "pointercancel"] {
            let weak = Rc::downgrade(&self.inner);
            dom.listen(&canvas, event, true, move |_| {
                with_state(&weak, |s| {
                    if s.drag.take().is_some() && !s.auto_fit {
                        // Sharpen immediately rather than waiting for the idle timer.
                        s.invalidate();
                    }
                    if let Some(dom) = &s.dom {
                        let _ = dom.canvas.style().set_property("cursor", "grab");
                    }
                });
            })?;
        }

        let weak = Rc::downgrade(&self.inner);
        dom.listen(&canvas, "wheel", false, move |e| {
            e.prevent_default();
            let e: &WheelEvent = e.unchecked_ref();
            with_state(&weak, |s| {
                let unit = match e.delta_mode() {
                    WheelEvent::DOM_DELTA_LINE => 16.0,
                    WheelEvent::DOM_DELTA_PAGE => s.dom.as_ref().map_or(800.0, |d| d.height),
                    _ => 1.0,
                };
                // Like the pastebom viewer: pinch (reported as ctrl+wheel) or
                // ctrl/cmd+wheel zooms, plain wheel or two-finger scroll pans.
                if e.ctrl_key() || e.meta_key() {
                    let factor = (-e.delta_y() * unit / WHEEL_ZOOM_PIXELS)
                        .exp()
                        .clamp(0.5, 2.0);
                    s.view
                        .zoom_at(f64::from(e.offset_x()), f64::from(e.offset_y()), factor);
                } else {
                    s.view.pan(-e.delta_x() * unit, -e.delta_y() * unit);
                }
                s.auto_fit = false;
                s.view_changed();
            });
        })?;

        let weak = Rc::downgrade(&self.inner);
        dom.listen(&canvas, "dblclick", true, move |_| {
            with_state(&weak, |s| {
                s.auto_fit = true;
                s.refresh();
            });
        })?;

        if let Some(panel) = dom.panel.clone() {
            let panel: EventTarget = panel.into();

            let weak = Rc::downgrade(&self.inner);
            dom.listen(&panel, "change", true, move |e| {
                let Some(input) = e
                    .target()
                    .and_then(|t| t.dyn_into::<HtmlInputElement>().ok())
                else {
                    return;
                };
                let Some(index) = input
                    .get_attribute("data-gv-layer")
                    .and_then(|i| i.parse::<usize>().ok())
                else {
                    return;
                };
                let changed = with_state(&weak, |s| {
                    let layer = s.layers.get_mut(index)?;
                    layer.style.visible = input.checked();
                    s.invalidate();
                    s.update_panel();
                    Some(())
                });
                if changed.flatten().is_some() {
                    if let Some(inner) = weak.upgrade() {
                        notify(&inner);
                    }
                }
            })?;

            let weak = Rc::downgrade(&self.inner);
            dom.listen(&panel, "click", true, move |e| {
                let Some(action) = e
                    .target()
                    .and_then(|t| t.dyn_into::<Element>().ok())
                    .and_then(|el| el.closest("[data-gv-action]").ok().flatten())
                    .and_then(|el| el.get_attribute("data-gv-action"))
                else {
                    return;
                };
                with_state(&weak, |s| match action.as_str() {
                    "side" => s.set_side(!s.from_bottom),
                    "fit" => {
                        s.auto_fit = true;
                        s.refresh();
                    }
                    _ => {}
                });
                if action == "side" {
                    if let Some(inner) = weak.upgrade() {
                        notify(&inner);
                    }
                }
            })?;
        }

        self.observe_resize(dom)
    }

    /// Track the root element's size with `ResizeObserver`, falling back to window resize.
    fn observe_resize(&self, dom: &mut Dom) -> Result<(), JsValue> {
        let window = web_sys::window().ok_or("no window")?;
        let weak = Rc::downgrade(&self.inner);
        let on_resize = move || {
            with_state(&weak, |s| {
                s.sync_size();
                s.refresh();
            });
        };

        let ctor = Reflect::get(&window, &"ResizeObserver".into())?;
        if let Some(ctor) = ctor.dyn_ref::<Function>() {
            let callback = Closure::<dyn FnMut()>::new(on_resize);
            let observer = Reflect::construct(ctor, &Array::of1(callback.as_ref()))?;
            call_method(&observer, "observe", &[dom.root.as_ref()])?;
            dom.resize_observer = Some(observer);
            dom.resize_callback = Some(callback);
        } else {
            dom.listen(&window.into(), "resize", true, move |_| on_resize())?;
        }
        Ok(())
    }
}

/// Run `f` on the viewer state if it is still alive and not already borrowed.
fn with_state<R>(weak: &Weak<RefCell<Inner>>, f: impl FnOnce(&mut Inner) -> R) -> Option<R> {
    let inner = weak.upgrade()?;
    let mut state = inner.try_borrow_mut().ok()?;
    Some(f(&mut state))
}

/// Parse sources without rendering. Resolves with `{ layers, bbox, warnings, skipped }`, where
/// each layer has `name`, `function`, `side`, `inner`, `drawings`, `clear_drawings`,
/// and `bbox`. Coordinates are millimetres with Y pointing down.
#[wasm_bindgen(js_name = parseSources)]
pub fn parse_sources(sources: JsValue) -> Promise {
    future_to_promise(async move {
        let mut project = GerberProject::default();
        for source in sources::to_list(&sources)? {
            match sources::load(source).await {
                Ok((name, bytes)) => project.add_source(&name, &bytes),
                Err(e) => project.warnings.push(e),
            }
        }
        Ok(project.serialize(&Serializer::json_compatible())?)
    })
}
