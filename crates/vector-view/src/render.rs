//! Canvas2D renderer (feature `canvas`).
//!
//! [`draw`] paints a [`Scene`] into a `CanvasRenderingContext2d` as resolved
//! by a [`ViewState`]: layers in paint order, filled areas with even-odd holes,
//! round-capped strokes (tracks, arcs, stroked text), drill holes, highlight
//! dimming, and `destination-out` erase layers. It does not clear the canvas;
//! call [`clear`] first, or compose several passes (e.g. a highlight overlay).

use crate::scene::{Item, Point, Prim, Scene};
use crate::style::{css_rgba, Rgba, ViewState};
use std::collections::HashMap;
use std::f64::consts::TAU;
use web_sys::{CanvasRenderingContext2d, CanvasWindingRule, Path2d};

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

/// Cached `Path2d`s for large polygons (zones), valid for one scene.
#[derive(Default)]
pub struct PathCache {
    key: Option<(usize, usize)>,
    paths: HashMap<usize, Path2d>,
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
    draw_inner(scene, ctx, state, None);
}

/// Draw `scene`, reusing `Path2d`s for large polygons across frames.
pub fn draw_cached(
    scene: &Scene,
    ctx: &CanvasRenderingContext2d,
    state: &ViewState,
    cache: &mut PathCache,
) {
    cache.sync(scene);
    draw_inner(scene, ctx, state, Some(cache));
}

fn draw_inner(
    scene: &Scene,
    ctx: &CanvasRenderingContext2d,
    state: &ViewState,
    mut cache: Option<&mut PathCache>,
) {
    let order = state.paint_order(scene);
    if order.is_empty() {
        return;
    }
    let mut by_layer: HashMap<u16, Vec<usize>> = HashMap::new();
    for (i, item) in scene.items.iter().enumerate() {
        by_layer.entry(item.layer).or_default().push(i);
    }

    ctx.save();
    let [a, b, c, d, e, f] = state.view.canvas_transform(state.dpr);
    let _ = ctx.set_transform(a, b, c, d, e, f);
    ctx.set_line_cap("round");
    ctx.set_line_join("round");
    let min_w = state.min_line_world();
    let outline_w = (state.min_line_px * 3.0).max(1.0) * state.view.world_per_px();
    let mut painter = Painter { ctx, open: None };

    for layer in order {
        let Some(items) = by_layer.get(&layer.id) else {
            continue;
        };
        let erase = state.erase_layers.contains(&layer.id);
        let _ = ctx.set_global_composite_operation(if erase {
            "destination-out"
        } else {
            "source-over"
        });
        let mut alpha_now = f64::NAN;
        for &i in items {
            let item = &scene.items[i];
            let Some(paint) = state.item_paint(layer, item) else {
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
            let cached = cache.as_deref_mut();
            draw_item(
                &mut painter,
                item,
                i,
                &stroke,
                &fill,
                min_w,
                paint.outline.then_some(outline_w),
                cached,
            );
        }
        painter.flush();
    }
    let _ = ctx.set_global_composite_operation("source-over");
    ctx.restore();
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
