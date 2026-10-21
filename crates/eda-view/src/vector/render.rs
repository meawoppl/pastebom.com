//! Canvas2D renderer (feature `canvas`).
//!
//! [`draw`] paints a [`Scene`] into a `CanvasRenderingContext2d` as resolved
//! by a [`ViewState`]: layers in paint order, filled areas with even-odd holes,
//! round-capped strokes (tracks, arcs, stroked text), drill holes, highlight
//! dimming, and `destination-out` erase layers. It does not clear the canvas
//! unless `ViewState::background` is set; call [`clear`] first, or compose
//! several draws (e.g. a highlight overlay canvas).
//!
//! With `ViewState::passes` each pass is drawn opaque into an offscreen
//! canvas and composited at its alpha, so translucent layers blend as one
//! unit. `ViewState::overlay` draws a highlight over everything the same way.

use crate::scene::{Item, Point, Prim, Scene};
use crate::style::{css_rgba, ItemPaint, Rgba, ViewState};
use std::collections::HashMap;
use std::f64::consts::TAU;
use wasm_bindgen::JsCast;
use web_sys::{CanvasRenderingContext2d, CanvasWindingRule, HtmlCanvasElement, Path2d};

/// Polygons with at least this many vertices get a cached `Path2d`.
const CACHE_MIN_POINTS: usize = 24;

/// Anything paths can be built into: the context's current path or a `Path2d`.
trait Sink {
    fn move_to(&self, x: f64, y: f64);
    fn line_to(&self, x: f64, y: f64);
    fn arc(&self, x: f64, y: f64, r: f64, a0: f64, a1: f64);
    fn close(&self);
}

impl Sink for CanvasRenderingContext2d {
    fn move_to(&self, x: f64, y: f64) {
        CanvasRenderingContext2d::move_to(self, x, y)
    }
    fn line_to(&self, x: f64, y: f64) {
        CanvasRenderingContext2d::line_to(self, x, y)
    }
    fn arc(&self, x: f64, y: f64, r: f64, a0: f64, a1: f64) {
        // Only fails for negative radii, which `r.max(0.0)` rules out.
        let _ = CanvasRenderingContext2d::arc(self, x, y, r.max(0.0), a0, a1);
    }
    fn close(&self) {
        self.close_path()
    }
}

impl Sink for Path2d {
    fn move_to(&self, x: f64, y: f64) {
        Path2d::move_to(self, x, y)
    }
    fn line_to(&self, x: f64, y: f64) {
        Path2d::line_to(self, x, y)
    }
    fn arc(&self, x: f64, y: f64, r: f64, a0: f64, a1: f64) {
        let _ = Path2d::arc(self, x, y, r.max(0.0), a0, a1);
    }
    fn close(&self) {
        self.close_path()
    }
}

fn ring(sink: &dyn Sink, pts: &[Point], closed: bool) {
    if let Some((first, rest)) = pts.split_first() {
        sink.move_to(first[0], first[1]);
        if rest.is_empty() {
            // A lone point: zero-length segment so round caps draw a dot.
            sink.line_to(first[0], first[1]);
        }
        for p in rest {
            sink.line_to(p[0], p[1]);
        }
        if closed {
            sink.close();
        }
    }
}

fn circle(sink: &dyn Sink, c: Point, r: f64) {
    sink.move_to(c[0] + r, c[1]);
    sink.arc(c[0], c[1], r, 0.0, TAU);
    sink.close();
}

/// Oblong (stadium) of `size` centred at `c`, rotated by `rot` radians.
fn oblong(sink: &dyn Sink, c: Point, size: [f64; 2], rot: f64) {
    let (w, h) = (size[0] / 2.0, size[1] / 2.0);
    let (s, co) = rot.sin_cos();
    let at = |x: f64, y: f64| [c[0] + x * co - y * s, c[1] + x * s + y * co];
    if (w - h).abs() < 1e-12 {
        circle(sink, c, w);
        return;
    }
    let (r, a, b, a0) = if w > h {
        (h, at(-(w - h), 0.0), at(w - h, 0.0), rot + TAU / 4.0)
    } else {
        (w, at(0.0, -(h - w)), at(0.0, h - w), rot + TAU / 2.0)
    };
    // Cap around `a`, then around `b`, half a turn each.
    let start = [a[0] + r * a0.cos(), a[1] + r * a0.sin()];
    sink.move_to(start[0], start[1]);
    sink.arc(a[0], a[1], r, a0, a0 + TAU / 2.0);
    sink.arc(b[0], b[1], r, a0 + TAU / 2.0, a0 + TAU);
    sink.close();
}

fn polygon(sink: &dyn Sink, outer: &[Point], holes: &[Vec<Point>]) {
    ring(sink, outer, true);
    for h in holes {
        ring(sink, h, true);
    }
}

/// Cached `Path2d`s for large polygons (zones), valid for one scene, plus
/// the offscreen canvas used for pass compositing.
#[derive(Default)]
pub struct PathCache {
    key: Option<(usize, usize)>,
    paths: HashMap<usize, Path2d>,
    scratch: Option<Scratch>,
}

impl PathCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Drop everything (call when the scene changes in place).
    pub fn clear(&mut self) {
        self.key = None;
        self.paths.clear();
    }

    fn sync(&mut self, scene: &Scene) {
        let key = (scene.items.as_ptr() as usize, scene.items.len());
        if self.key != Some(key) {
            self.paths.clear();
            self.key = Some(key);
        }
    }
}

/// Offscreen canvas matching the target's backing-store size.
struct Scratch {
    canvas: HtmlCanvasElement,
    ctx: CanvasRenderingContext2d,
}

impl Scratch {
    fn for_target(target: &CanvasRenderingContext2d, reuse: Option<Scratch>) -> Option<Scratch> {
        let main = target.canvas()?;
        let scratch = match reuse {
            Some(s) => s,
            None => {
                let doc = main.owner_document()?;
                let canvas: HtmlCanvasElement =
                    doc.create_element("canvas").ok()?.dyn_into().ok()?;
                let ctx: CanvasRenderingContext2d =
                    canvas.get_context("2d").ok()??.dyn_into().ok()?;
                Scratch { canvas, ctx }
            }
        };
        if scratch.canvas.width() != main.width() {
            scratch.canvas.set_width(main.width());
        }
        if scratch.canvas.height() != main.height() {
            scratch.canvas.set_height(main.height());
        }
        Some(scratch)
    }

    fn clear(&self) {
        let _ = self.ctx.set_transform(1.0, 0.0, 0.0, 1.0, 0.0, 0.0);
        self.ctx.clear_rect(
            0.0,
            0.0,
            self.canvas.width() as f64,
            self.canvas.height() as f64,
        );
    }

    /// Composite onto `target` at `alpha` with an identity transform.
    fn composite(&self, target: &CanvasRenderingContext2d, alpha: f64) {
        target.save();
        let _ = target.set_transform(1.0, 0.0, 0.0, 1.0, 0.0, 0.0);
        let _ = target.set_global_composite_operation("source-over");
        target.set_global_alpha(alpha.clamp(0.0, 1.0));
        let _ = target.draw_image_with_html_canvas_element(&self.canvas, 0.0, 0.0);
        target.restore();
    }
}

/// Stroke batching: consecutive strokes sharing width and paint go into one path.
#[derive(PartialEq, Clone)]
struct StrokeKey {
    width: f64,
    color: String,
}

struct Painter<'a> {
    ctx: &'a CanvasRenderingContext2d,
    open: Option<StrokeKey>,
}

impl Painter<'_> {
    fn flush(&mut self) {
        if self.open.take().is_some() {
            self.ctx.stroke();
        }
    }

    /// Begin (or continue) a batched stroke; returns the context to path into.
    fn stroke(&mut self, width: f64, color: &str) -> &CanvasRenderingContext2d {
        let key = StrokeKey {
            width,
            color: color.to_string(),
        };
        if self.open.as_ref() != Some(&key) {
            self.flush();
            self.ctx.set_line_width(width);
            self.ctx.set_stroke_style_str(color);
            self.ctx.begin_path();
            self.open = Some(key);
        }
        self.ctx
    }

    fn fill_now(&mut self, color: &str, build: impl FnOnce(&dyn Sink)) {
        self.flush();
        self.ctx.set_fill_style_str(color);
        self.ctx.begin_path();
        build(self.ctx);
        self.ctx
            .fill_with_canvas_winding_rule(CanvasWindingRule::Evenodd);
    }

    fn stroke_now(&mut self, width: f64, color: &str, build: impl FnOnce(&dyn Sink)) {
        let ctx = self.stroke(width, color);
        build(ctx);
    }
}

/// Clear the whole backing store (`width_px` x `height_px` device pixels),
/// optionally filling it with `background`.
pub fn clear(
    ctx: &CanvasRenderingContext2d,
    width_px: f64,
    height_px: f64,
    background: Option<Rgba>,
) {
    ctx.save();
    let _ = ctx.set_transform(1.0, 0.0, 0.0, 1.0, 0.0, 0.0);
    match background {
        Some(c) => {
            ctx.set_fill_style_str(&css_rgba(c, 1.0));
            ctx.fill_rect(0.0, 0.0, width_px, height_px);
        }
        None => ctx.clear_rect(0.0, 0.0, width_px, height_px),
    }
    ctx.restore();
}

/// Draw `scene` without caching.
pub fn draw(scene: &Scene, ctx: &CanvasRenderingContext2d, state: &ViewState) {
    let mut cache = PathCache::new();
    draw_inner(scene, ctx, state, &mut cache, false);
}

/// Draw `scene`, reusing `Path2d`s for large polygons (and the compositing
/// canvas) across frames.
pub fn draw_cached(
    scene: &Scene,
    ctx: &CanvasRenderingContext2d,
    state: &ViewState,
    cache: &mut PathCache,
) {
    cache.sync(scene);
    draw_inner(scene, ctx, state, cache, true);
}

/// Shared per-frame drawing parameters.
struct Frame<'a> {
    scene: &'a Scene,
    state: &'a ViewState,
    by_layer: HashMap<u16, Vec<usize>>,
    min_w: f64,
    outline_w: f64,
}

fn set_view(ctx: &CanvasRenderingContext2d, state: &ViewState) {
    let [a, b, c, d, e, f] = state.view.canvas_transform(state.dpr);
    let _ = ctx.set_transform(a, b, c, d, e, f);
    ctx.set_line_cap("round");
    ctx.set_line_join("round");
}

fn draw_background(ctx: &CanvasRenderingContext2d, state: &ViewState) {
    let Some(canvas) = ctx.canvas() else { return };
    let (w, h) = (canvas.width() as f64, canvas.height() as f64);
    if let Some(bg) = state.background {
        clear(ctx, w, h, Some(bg));
    }
    let Some(grid) = state.grid else { return };
    let view = &state.view;
    let mut step = grid.spacing(view.scale);
    let vis = view.visible_bbox(w / state.dpr, h / state.dpr);
    // Coarsen rather than draw an unreasonable number of dots.
    while (vis.width() / step + 1.0) * (vis.height() / step + 1.0) > 40_000.0 {
        step *= 10.0;
    }
    let dot = grid.dot_px * view.world_per_px();
    ctx.save();
    set_view(ctx, state);
    ctx.set_fill_style_str(&css_rgba(grid.color, 1.0));
    ctx.begin_path();
    let (x0, y0) = ((vis.min[0] / step).floor(), (vis.min[1] / step).floor());
    let (x1, y1) = ((vis.max[0] / step).ceil(), (vis.max[1] / step).ceil());
    let mut y = y0;
    while y <= y1 {
        let mut x = x0;
        while x <= x1 {
            ctx.rect(x * step - dot / 2.0, y * step - dot / 2.0, dot, dot);
            x += 1.0;
        }
        y += 1.0;
    }
    ctx.fill();
    ctx.restore();
}

/// Draw `items` (indices) of one plan entry onto `ctx`.
fn draw_items(
    frame: &Frame,
    ctx: &CanvasRenderingContext2d,
    items: &[usize],
    erase: bool,
    paint: &dyn Fn(&Item) -> Option<ItemPaint>,
    mut cache: Option<&mut PathCache>,
) {
    let _ = ctx.set_global_composite_operation(if erase {
        "destination-out"
    } else {
        "source-over"
    });
    let mut painter = Painter { ctx, open: None };
    let mut alpha_now = f64::NAN;
    for &i in items {
        let item = &frame.scene.items[i];
        let Some(paint) = paint(item) else {
            continue;
        };
        let (alpha, stroke_c, fill_c) = if erase {
            (1.0, [0, 0, 0, 255], [0, 0, 0, 255])
        } else {
            (paint.alpha, paint.stroke, paint.fill)
        };
        if alpha <= 0.0 {
            continue;
        }
        if alpha != alpha_now {
            painter.flush();
            ctx.set_global_alpha(alpha);
            alpha_now = alpha;
        }
        let stroke = css_rgba(stroke_c, 1.0);
        let fill = css_rgba(fill_c, 1.0);
        draw_item(
            &mut painter,
            item,
            i,
            &stroke,
            &fill,
            frame.min_w,
            paint.outline.then_some(frame.outline_w),
            cache.as_deref_mut(),
        );
    }
    painter.flush();
    let _ = ctx.set_global_composite_operation("source-over");
    ctx.set_global_alpha(1.0);
}

/// Draw `items` at `alpha` as one unit: opaque into the scratch canvas, then
/// composited. Falls back to direct drawing at full opacity.
fn draw_composited(
    frame: &Frame,
    ctx: &CanvasRenderingContext2d,
    items: &[usize],
    alpha: f64,
    paint: &dyn Fn(&Item) -> Option<ItemPaint>,
    cache: &mut PathCache,
) {
    if items.is_empty() || alpha <= 0.0 {
        return;
    }
    if alpha >= 1.0 {
        draw_items(frame, ctx, items, false, paint, Some(cache));
        return;
    }
    let Some(scratch) = Scratch::for_target(ctx, cache.scratch.take()) else {
        // No document (e.g. a worker): approximate with per-item alpha.
        let dimmed = |item: &Item| {
            paint(item).map(|mut p| {
                p.alpha *= alpha;
                p
            })
        };
        draw_items(frame, ctx, items, false, &dimmed, Some(cache));
        return;
    };
    scratch.clear();
    set_view(&scratch.ctx, frame.state);
    draw_items(frame, &scratch.ctx, items, false, paint, Some(cache));
    scratch.composite(ctx, alpha);
    cache.scratch = Some(scratch);
}

fn draw_inner(
    scene: &Scene,
    ctx: &CanvasRenderingContext2d,
    state: &ViewState,
    cache: &mut PathCache,
    keep_scratch: bool,
) {
    draw_background(ctx, state);
    let plan = state.paint_plan(scene);
    if plan.is_empty() {
        return;
    }
    let mut by_layer: HashMap<u16, Vec<usize>> = HashMap::new();
    for (i, item) in scene.items.iter().enumerate() {
        by_layer.entry(item.layer).or_default().push(i);
    }
    let frame = Frame {
        scene,
        state,
        by_layer,
        min_w: state.min_line_world(),
        outline_w: (state.min_line_px * 3.0).max(1.0) * state.view.world_per_px(),
    };

    ctx.save();
    set_view(ctx, state);
    for entry in &plan {
        let Some(all) = frame.by_layer.get(&entry.layer.id) else {
            continue;
        };
        let layer = entry.layer;
        let erase = state.erase_layers.contains(&layer.id);
        if !entry.composited {
            let paint = |item: &Item| state.item_paint(layer, item);
            draw_items(&frame, ctx, all, erase, &paint, Some(cache));
            continue;
        }
        let items: Vec<usize> = all
            .iter()
            .copied()
            .filter(|&i| entry.matches(&scene.items[i]))
            .collect();
        let paint = |item: &Item| state.item_paint_with(layer, item, 1.0);
        if erase {
            draw_items(&frame, ctx, &items, true, &paint, Some(cache));
        } else {
            draw_composited(&frame, ctx, &items, entry.alpha, &paint, cache);
        }
    }

    if let Some(overlay) = &state.overlay {
        if overlay.highlight.is_active() {
            let mut lit: Vec<usize> = Vec::new();
            for entry in &plan {
                if state.erase_layers.contains(&entry.layer.id) {
                    continue;
                }
                if let Some(all) = frame.by_layer.get(&entry.layer.id) {
                    lit.extend(all.iter().copied().filter(|&i| {
                        let item = &scene.items[i];
                        entry.matches(item)
                            && overlay.highlight.contains(item)
                            && !state.hidden_items.contains(&item.id)
                    }));
                }
            }
            lit.sort_unstable();
            lit.dedup();
            let color = overlay.color;
            let hole = state.theme.hole;
            let paint = |item: &Item| {
                let c = if item.role == crate::scene::Role::Hole {
                    hole.unwrap_or(color)
                } else {
                    color
                };
                Some(ItemPaint {
                    stroke: c,
                    fill: c,
                    alpha: 1.0,
                    outline: state.outline_items.contains(&item.id),
                })
            };
            draw_composited(&frame, ctx, &lit, overlay.alpha, &paint, cache);
        }
    }
    ctx.restore();
    if !keep_scratch {
        cache.scratch = None;
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_item(
    p: &mut Painter,
    item: &Item,
    index: usize,
    stroke: &str,
    fill: &str,
    min_w: f64,
    outline: Option<f64>,
    cache: Option<&mut PathCache>,
) {
    let w = |width: f64| width.max(min_w);
    match &item.prim {
        Prim::Polyline { points, width } => {
            p.stroke_now(w(*width), stroke, |s| ring(s, points, false));
        }
        Prim::Strokes { strokes, width } => {
            p.stroke_now(w(*width), stroke, |s| {
                for st in strokes {
                    ring(s, st, false);
                }
            });
        }
        Prim::Arc {
            center,
            radius,
            start,
            end,
            width,
        } => {
            p.stroke_now(w(*width), stroke, |s| {
                s.move_to(
                    center[0] + radius * start.cos(),
                    center[1] + radius * start.sin(),
                );
                s.arc(center[0], center[1], *radius, *start, *end);
            });
        }
        Prim::Circle {
            center,
            radius,
            fill: filled,
            stroke: sw,
        } => {
            if *filled && outline.is_none() {
                p.fill_now(fill, |s| circle(s, *center, *radius));
            }
            let line = match (*filled, outline) {
                (_, Some(o)) => Some(o.max(*sw)),
                (true, None) => (*sw > 0.0).then_some(*sw),
                (false, None) => Some(w(*sw)),
            };
            if let Some(lw) = line {
                p.stroke_now(lw, stroke, |s| circle(s, *center, *radius));
            }
        }
        Prim::Polygon {
            outer,
            holes,
            fill: filled,
            stroke: sw,
        } => {
            let big = outer.len() + holes.iter().map(Vec::len).sum::<usize>() >= CACHE_MIN_POINTS;
            if let (true, Some(cache)) = (big, cache) {
                let path = cache.paths.entry(index).or_insert_with(|| {
                    let path = Path2d::new().expect("Path2d constructor");
                    polygon(&path, outer, holes);
                    path
                });
                p.flush();
                if *filled && outline.is_none() {
                    p.ctx.set_fill_style_str(fill);
                    p.ctx
                        .fill_with_path_2d_and_winding(path, CanvasWindingRule::Evenodd);
                }
                let line = match (*filled, outline) {
                    (_, Some(o)) => Some(o.max(*sw)),
                    (true, None) => (*sw > 0.0).then_some(*sw),
                    (false, None) => Some(w(*sw)),
                };
                if let Some(lw) = line {
                    p.ctx.set_line_width(lw);
                    p.ctx.set_stroke_style_str(stroke);
                    p.ctx.stroke_with_path(path);
                }
                return;
            }
            if *filled && outline.is_none() {
                p.fill_now(fill, |s| polygon(s, outer, holes));
            }
            let line = match (*filled, outline) {
                (_, Some(o)) => Some(o.max(*sw)),
                (true, None) => (*sw > 0.0).then_some(*sw),
                (false, None) => Some(w(*sw)),
            };
            if let Some(lw) = line {
                p.stroke_now(lw, stroke, |s| polygon(s, outer, holes));
            }
        }
        Prim::Hole {
            center,
            size,
            rotation,
        } => match outline {
            None => p.fill_now(fill, |s| oblong(s, *center, *size, *rotation)),
            Some(o) => p.stroke_now(o, stroke, |s| oblong(s, *center, *size, *rotation)),
        },
    }
}
