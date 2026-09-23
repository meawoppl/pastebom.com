//! Canvas rendering of compiled layer geometry.

use pcb_extract::parsers::gerber::layers::GerberLayerType;
use pcb_extract::parsers::gerber::GerberLayer;
use wasm_bindgen::JsValue;
use web_sys::{CanvasRenderingContext2d, CanvasWindingRule, Path2d};

use crate::geometry::{compile, CompiledPaths, PathOp};

/// Browser-side paths for one polarity of a layer.
pub struct Paths {
    fill: Option<Path2d>,
    strokes: Vec<(f64, Path2d)>,
}

impl Paths {
    fn build(compiled: &CompiledPaths) -> Result<Self, JsValue> {
        let fill = if compiled.fill.is_empty() {
            None
        } else {
            Some(to_path2d(&compiled.fill)?)
        };
        let strokes = compiled
            .strokes
            .iter()
            .map(|(w, ops)| Ok((*w, to_path2d(ops)?)))
            .collect::<Result<_, JsValue>>()?;
        Ok(Self { fill, strokes })
    }

    fn draw(&self, ctx: &CanvasRenderingContext2d, min_width: f64) {
        if let Some(fill) = &self.fill {
            ctx.fill_with_path_2d_and_winding(fill, CanvasWindingRule::Nonzero);
        }
        for (width, path) in &self.strokes {
            ctx.set_line_width(width.max(min_width));
            ctx.stroke_with_path(path);
        }
    }
}

/// Cached geometry for one layer: dark shapes plus clear-polarity shapes.
pub struct LayerPaths {
    dark: Paths,
    clear: Option<Paths>,
}

impl LayerPaths {
    pub fn build(layer: &GerberLayer) -> Result<Self, JsValue> {
        let clear = compile(&layer.clear_drawings);
        Ok(Self {
            dark: Paths::build(&compile(&layer.drawings))?,
            clear: if clear.is_empty() {
                None
            } else {
                Some(Paths::build(&clear)?)
            },
        })
    }

    /// Draw the layer in `color`, then erase its clear-polarity shapes.
    /// `min_width` is the thinnest stroke to draw, in board units.
    pub fn draw(&self, ctx: &CanvasRenderingContext2d, color: &str, min_width: f64) {
        ctx.set_line_cap("round");
        ctx.set_line_join("round");
        ctx.set_fill_style_str(color);
        ctx.set_stroke_style_str(color);
        self.dark.draw(ctx, min_width);

        if let Some(clear) = &self.clear {
            // Only fails for unknown operation names; this one is always valid.
            let _ = ctx.set_global_composite_operation("destination-out");
            ctx.set_fill_style_str("#000");
            ctx.set_stroke_style_str("#000");
            clear.draw(ctx, min_width);
            let _ = ctx.set_global_composite_operation("source-over");
        }
    }
}

fn to_path2d(ops: &[PathOp]) -> Result<Path2d, JsValue> {
    let path = Path2d::new()?;
    for op in ops {
        match *op {
            PathOp::MoveTo(x, y) => path.move_to(x, y),
            PathOp::LineTo(x, y) => path.line_to(x, y),
            PathOp::Arc { cx, cy, r, a0, a1 } => path.arc(cx, cy, r.max(0.0), a0, a1)?,
            PathOp::BezierTo { c1, c2, end } => {
                path.bezier_curve_to(c1[0], c1[1], c2[0], c2[1], end[0], end[1])
            }
            PathOp::Close => path.close_path(),
        }
    }
    Ok(path)
}

/// Paint order for a layer. Viewed from the bottom, the stack is reversed, but
/// drills and the board outline always stay on top.
pub fn paint_order(layer_type: &GerberLayerType, from_bottom: bool) -> i32 {
    let order = layer_type.stack_order() as i32;
    let pinned_top = matches!(
        layer_type,
        GerberLayerType::Drills | GerberLayerType::BoardOutline
    );
    if from_bottom && !pinned_top {
        -order
    } else {
        order
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bottom_view_reverses_stack_but_keeps_drills_and_outline_on_top() {
        let top = GerberLayerType::CopperTop;
        let bottom = GerberLayerType::CopperBottom;
        assert!(paint_order(&top, false) > paint_order(&bottom, false));
        assert!(paint_order(&top, true) < paint_order(&bottom, true));
        for pinned in [GerberLayerType::Drills, GerberLayerType::BoardOutline] {
            assert!(paint_order(&pinned, true) > paint_order(&bottom, true));
            assert!(paint_order(&pinned, false) > paint_order(&top, false));
        }
    }
}
