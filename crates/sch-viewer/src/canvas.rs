//! Yew host for a `vector_view::Scene` on a Canvas2D: pan/zoom/pinch input,
//! tap picking, fitting and redraw on resize.
use std::{cell::RefCell, collections::HashSet, rc::Rc};

use gloo_events::EventListener;
use vector_view::{
    hit::HitIndex,
    input::{Input, InputEvent, InputOutcome},
    render::{clear, draw_cached, PathCache},
    style::{Highlight, LayerVisibility, Rgba, Theme, ViewState},
    view::View,
    BBox, ItemId, Scene,
};
use wasm_bindgen::{closure::Closure, JsCast};
use web_sys::{
    CanvasRenderingContext2d, HtmlCanvasElement, PointerEvent, ResizeObserver, WheelEvent,
};
use yew::prelude::*;

/// Padding left around a fit, in CSS pixels.
const FIT_PAD: f64 = 24.0;
/// Pick tolerance, in CSS pixels.
const PICK_PX: f64 = 6.0;

/// A request to frame `bbox`; bump `seq` to repeat the same request.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Focus {
    pub seq: u32,
    pub bbox: BBox,
}

#[derive(Properties, PartialEq)]
pub struct SceneCanvasProps {
    pub scene: Rc<Scene>,
    #[prop_or_default]
    pub highlight: Highlight,
    #[prop_or_default]
    pub visibility: LayerVisibility,
    #[prop_or_default]
    pub hidden_items: Rc<HashSet<ItemId>>,
    #[prop_or_default]
    pub theme: Theme,
    #[prop_or_default]
    pub background: Option<Rgba>,
    /// Box framed on first draw, on resize until the user moves the camera,
    /// and by the Fit button; the scene bbox when unset.
    #[prop_or_default]
    pub fit_bbox: Option<BBox>,
    #[prop_or_default]
    pub focus: Option<Focus>,
    #[prop_or_default]
    pub on_select: Callback<Option<ItemId>>,
}

struct Runtime {
    state: ViewState,
    input: Input,
    hit: HitIndex,
    cache: PathCache,
    fit_bbox: Option<BBox>,
    /// The view needs a fit before the next draw.
    needs_fit: bool,
    /// The view came from an automatic fit, so resizes refit it.
    auto_fit: bool,
    size: (f64, f64),
}

impl Runtime {
    fn new(scene: &Scene) -> Self {
        Self {
            state: ViewState::new(View::for_scene(scene)),
            input: Input::default(),
            hit: HitIndex::new(scene),
            cache: PathCache::new(),
            fit_bbox: None,
            needs_fit: true,
            auto_fit: true,
            size: (0.0, 0.0),
        }
    }
}

/// Size the backing store to the laid-out canvas; `None` while it has no box.
fn layout(c: &HtmlCanvasElement) -> Option<(f64, f64, f64)> {
    if c.client_width() <= 0 || c.client_height() <= 0 {
        return None;
    }
    let w = f64::from(c.client_width());
    let h = f64::from(c.client_height());
    let d = web_sys::window().map_or(1.0, |x| x.device_pixel_ratio().clamp(1.0, 3.0));
    c.set_width((w * d).round() as u32);
    c.set_height((h * d).round() as u32);
    Some((w, h, d))
}

fn redraw(c: &HtmlCanvasElement, scene: &Scene, r: &mut Runtime) {
    let Some(ctx) = c
        .get_context("2d")
        .ok()
        .flatten()
        .and_then(|x| x.dyn_into::<CanvasRenderingContext2d>().ok())
    else {
        return;
    };
    // Never fit against a collapsed canvas: that shrinks the scene to a dot.
    let Some((w, h, d)) = layout(c) else {
        return;
    };
    let resized = r.size != (w, h);
    r.size = (w, h);
    r.state.dpr = d;
    if r.needs_fit || (r.auto_fit && resized) {
        let bbox = r.fit_bbox.unwrap_or(scene.bbox);
        r.state.view.fit(&bbox, w, h, FIT_PAD);
        r.needs_fit = false;
    }
    clear(
        &ctx,
        f64::from(c.width()),
        f64::from(c.height()),
        r.state.background,
    );
    draw_cached(scene, &ctx, &r.state, &mut r.cache);
}

fn offset(c: &HtmlCanvasElement, x: i32, y: i32) -> [f64; 2] {
    let b = c.get_bounding_client_rect();
    [f64::from(x) - b.left(), f64::from(y) - b.top()]
}

fn apply(
    c: &HtmlCanvasElement,
    scene: &Scene,
    r: &mut Runtime,
    e: InputEvent,
    select: &Callback<Option<ItemId>>,
) {
    match r.input.handle(&e, &mut r.state.view) {
        InputOutcome::None => {}
        InputOutcome::ViewChanged => {
            r.auto_fit = false;
            redraw(c, scene, r);
        }
        InputOutcome::Reset => {
            r.needs_fit = true;
            r.auto_fit = true;
            redraw(c, scene, r);
        }
        InputOutcome::Tap { x, y, .. } => {
            let p = r.state.view.to_world([x, y]);
            let tol = PICK_PX * r.state.view.world_per_px();
            select.emit(r.hit.hit_test_view(scene, &r.state, p, tol));
        }
    }
}

fn pointer(
    canvas: NodeRef,
    scene: Rc<Scene>,
    runtime: Rc<RefCell<Runtime>>,
    select: Callback<Option<ItemId>>,
    make: impl Fn(&PointerEvent, [f64; 2]) -> InputEvent + 'static,
) -> Callback<PointerEvent> {
    Callback::from(move |e: PointerEvent| {
        e.prevent_default();
        let Some(c) = canvas.cast::<HtmlCanvasElement>() else {
            return;
        };
        if e.type_() == "pointerdown" {
            let _ = c.set_pointer_capture(e.pointer_id());
        }
        let p = offset(&c, e.client_x(), e.client_y());
        apply(&c, &scene, &mut runtime.borrow_mut(), make(&e, p), &select);
    })
}

#[function_component(SceneCanvas)]
pub fn scene_canvas(props: &SceneCanvasProps) -> Html {
    let canvas = use_node_ref();
    let runtime = use_mut_ref(|| Runtime::new(&props.scene));

    // A new scene rebuilds the runtime (hit index and path cache are per scene).
    {
        let runtime = runtime.clone();
        use_effect_with(props.scene.clone(), move |scene| {
            *runtime.borrow_mut() = Runtime::new(scene);
        });
    }

    // Drawing configuration; a changed fit box refits.
    {
        let canvas = canvas.clone();
        let runtime = runtime.clone();
        let config = (
            props.scene.clone(),
            props.highlight.clone(),
            props.visibility.clone(),
            props.hidden_items.clone(),
            props.theme.clone(),
            props.background,
            props.fit_bbox,
        );
        use_effect_with(config, move |config| {
            let (scene, highlight, visibility, hidden, theme, background, fit_bbox) = config;
            let mut r = runtime.borrow_mut();
            r.state.highlight = highlight.clone();
            r.state.visibility = visibility.clone();
            r.state.hidden_items = (**hidden).clone();
            r.state.theme = theme.clone();
            r.state.background = *background;
            if r.fit_bbox != *fit_bbox {
                r.fit_bbox = *fit_bbox;
                r.needs_fit = true;
                r.auto_fit = true;
            }
            if let Some(c) = canvas.cast::<HtmlCanvasElement>() {
                redraw(&c, scene, &mut r);
            }
        });
    }

    // Frame a requested box (search hits, cross-probe).
    {
        let canvas = canvas.clone();
        let runtime = runtime.clone();
        let scene = props.scene.clone();
        use_effect_with(props.focus, move |focus| {
            let Some(focus) = focus else { return };
            let Some(c) = canvas.cast::<HtmlCanvasElement>() else {
                return;
            };
            let mut r = runtime.borrow_mut();
            let (w, h) = (
                f64::from(c.client_width().max(1)),
                f64::from(c.client_height().max(1)),
            );
            let mut bbox = focus.bbox;
            let pad = (bbox.width().max(bbox.height()) * 0.6).max(5.0);
            bbox.min = [bbox.min[0] - pad, bbox.min[1] - pad];
            bbox.max = [bbox.max[0] + pad, bbox.max[1] + pad];
            r.state.view.fit(&bbox, w, h, FIT_PAD);
            r.needs_fit = false;
            r.auto_fit = false;
            redraw(&c, &scene, &mut r);
        });
    }

    // Track the canvas box itself as well as the window.
    {
        let canvas = canvas.clone();
        let runtime = runtime.clone();
        let scene = props.scene.clone();
        use_effect_with(props.scene.clone(), move |_| {
            let element = canvas.cast::<HtmlCanvasElement>();
            let callback = Closure::<dyn FnMut()>::new({
                let canvas = canvas.clone();
                move || {
                    if let (Some(c), Ok(mut r)) =
                        (canvas.cast::<HtmlCanvasElement>(), runtime.try_borrow_mut())
                    {
                        redraw(&c, &scene, &mut r);
                    }
                }
            });
            let observer = ResizeObserver::new(callback.as_ref().unchecked_ref()).ok();
            if let (Some(observer), Some(element)) = (&observer, &element) {
                observer.observe(element);
            }
            let listener = web_sys::window().map(|w| {
                let f = callback
                    .as_ref()
                    .unchecked_ref::<js_sys::Function>()
                    .clone();
                EventListener::new(&w, "resize", move |_| {
                    let _ = f.call0(&wasm_bindgen::JsValue::NULL);
                })
            });
            move || {
                if let Some(observer) = observer {
                    observer.disconnect();
                }
                drop(listener);
                drop(callback);
            }
        });
    }

    let scene = &props.scene;
    let select = &props.on_select;
    let down = pointer(
        canvas.clone(),
        scene.clone(),
        runtime.clone(),
        select.clone(),
        |e, p| InputEvent::PointerDown {
            id: e.pointer_id(),
            x: p[0],
            y: p[1],
            button: e.button(),
            time_ms: e.time_stamp(),
        },
    );
    let moved = pointer(
        canvas.clone(),
        scene.clone(),
        runtime.clone(),
        select.clone(),
        |e, p| InputEvent::PointerMove {
            id: e.pointer_id(),
            x: p[0],
            y: p[1],
        },
    );
    let up = pointer(
        canvas.clone(),
        scene.clone(),
        runtime.clone(),
        select.clone(),
        |e, p| InputEvent::PointerUp {
            id: e.pointer_id(),
            x: p[0],
            y: p[1],
            button: e.button(),
            time_ms: e.time_stamp(),
        },
    );
    let cancel = {
        let runtime = runtime.clone();
        Callback::from(move |e: PointerEvent| {
            let mut r = runtime.borrow_mut();
            let mut v = r.state.view;
            let _ = r
                .input
                .handle(&InputEvent::PointerCancel { id: e.pointer_id() }, &mut v);
            r.state.view = v;
        })
    };
    let wheel = {
        let canvas = canvas.clone();
        let scene = scene.clone();
        let runtime = runtime.clone();
        let select = select.clone();
        Callback::from(move |e: WheelEvent| {
            e.prevent_default();
            let Some(c) = canvas.cast::<HtmlCanvasElement>() else {
                return;
            };
            let p = offset(&c, e.client_x(), e.client_y());
            let ev = InputEvent::Wheel {
                x: p[0],
                y: p[1],
                dx: e.delta_x(),
                dy: e.delta_y(),
                delta_mode: e.delta_mode(),
                ctrl: e.ctrl_key(),
            };
            apply(&c, &scene, &mut runtime.borrow_mut(), ev, &select);
        })
    };
    let fit = {
        let canvas = canvas.clone();
        let scene = scene.clone();
        let runtime = runtime.clone();
        Callback::from(move |_| {
            if let Some(c) = canvas.cast::<HtmlCanvasElement>() {
                let mut r = runtime.borrow_mut();
                r.needs_fit = true;
                r.auto_fit = true;
                redraw(&c, &scene, &mut r);
            }
        })
    };

    html! {
        <div class="scene">
            <canvas
                ref={canvas}
                class="scene-canvas"
                aria-label="Schematic"
                onpointerdown={down}
                onpointermove={moved}
                onpointerup={up}
                onpointercancel={cancel}
                onwheel={wheel}
            />
            <button class="scene-fit" onclick={fit}>{"Fit"}</button>
        </div>
    }
}
