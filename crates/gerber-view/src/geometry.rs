//! Compiles parsed Gerber drawings into a small set of batched path operations:
//! one fill path plus one stroke path per distinct line width.
//!
//! Every closed fill contour is emitted with the same (positive shoelace) winding,
//! so the whole layer can be filled in one call with the nonzero rule and
//! overlapping flashes union instead of cancelling. Region cut-ins keep their
//! internal orientation, so holes drawn that way stay open.

use std::collections::BTreeMap;
use std::f64::consts::TAU;

use pcb_extract::types::Drawing;

#[derive(Debug, Clone, PartialEq)]
pub enum PathOp {
    MoveTo(f64, f64),
    LineTo(f64, f64),
    /// Clockwise (in Y-down screen space) arc from angle `a0` to `a1`, radians.
    Arc {
        cx: f64,
        cy: f64,
        r: f64,
        a0: f64,
        a1: f64,
    },
    BezierTo {
        c1: [f64; 2],
        c2: [f64; 2],
        end: [f64; 2],
    },
    Close,
}

/// Batched geometry for one polarity of one layer.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct CompiledPaths {
    pub fill: Vec<PathOp>,
    /// Stroke paths keyed by line width in millimetres. Width 0 means hairline.
    pub strokes: Vec<(f64, Vec<PathOp>)>,
}

impl CompiledPaths {
    pub fn is_empty(&self) -> bool {
        self.fill.is_empty() && self.strokes.is_empty()
    }
}

pub fn compile(drawings: &[Drawing]) -> CompiledPaths {
    let mut fill = Vec::new();
    let mut strokes: BTreeMap<i64, (f64, Vec<PathOp>)> = BTreeMap::new();

    for d in drawings {
        match d {
            Drawing::Segment { start, end, width } => {
                let ops = stroke_group(&mut strokes, *width);
                ops.push(PathOp::MoveTo(start[0], start[1]));
                ops.push(PathOp::LineTo(end[0], end[1]));
            }
            Drawing::Arc {
                start,
                radius,
                startangle,
                endangle,
                width,
            } => {
                let (a0, a1) = (startangle.to_radians(), endangle.to_radians());
                let ops = stroke_group(&mut strokes, *width);
                ops.push(PathOp::MoveTo(
                    start[0] + radius * a0.cos(),
                    start[1] + radius * a0.sin(),
                ));
                ops.push(PathOp::Arc {
                    cx: start[0],
                    cy: start[1],
                    r: *radius,
                    a0,
                    a1,
                });
            }
            Drawing::Curve {
                start,
                end,
                cpa,
                cpb,
                width,
            } => {
                let ops = stroke_group(&mut strokes, *width);
                ops.push(PathOp::MoveTo(start[0], start[1]));
                ops.push(PathOp::BezierTo {
                    c1: *cpa,
                    c2: *cpb,
                    end: *end,
                });
            }
            Drawing::Circle {
                start,
                radius,
                width,
                filled,
            } => {
                let ops = if filled.is_some_and(|f| f != 0) {
                    &mut fill
                } else {
                    stroke_group(&mut strokes, *width)
                };
                push_circle(ops, start[0], start[1], *radius);
            }
            Drawing::Rect { start, end, width } => {
                let ring = [
                    [start[0], start[1]],
                    [end[0], start[1]],
                    [end[0], end[1]],
                    [start[0], end[1]],
                ];
                // Zero-width rects are aperture flashes; others are outlines.
                if *width == 0.0 {
                    push_ring(&mut fill, &ring, true);
                } else {
                    push_ring(stroke_group(&mut strokes, *width), &ring, false);
                }
            }
            Drawing::Polygon {
                pos,
                angle,
                polygons,
                filled,
                width,
            } => {
                let (sin, cos) = (-angle).to_radians().sin_cos();
                let place = |p: &[f64; 2]| {
                    [
                        pos[0] + p[0] * cos - p[1] * sin,
                        pos[1] + p[0] * sin + p[1] * cos,
                    ]
                };
                let is_filled = filled.is_none_or(|f| f != 0);
                for poly in polygons.iter().filter(|p| p.len() >= 2) {
                    let ring: Vec<[f64; 2]> = poly.iter().map(place).collect();
                    if is_filled {
                        push_ring(&mut fill, &ring, true);
                    } else {
                        push_ring(stroke_group(&mut strokes, *width), &ring, false);
                    }
                }
            }
        }
    }

    CompiledPaths {
        fill,
        strokes: strokes.into_values().collect(),
    }
}

/// Stroke ops for `width`, grouped by width rounded to a nanometre.
fn stroke_group(strokes: &mut BTreeMap<i64, (f64, Vec<PathOp>)>, width: f64) -> &mut Vec<PathOp> {
    let key = (width * 1e6).round() as i64;
    &mut strokes.entry(key).or_insert_with(|| (width, Vec::new())).1
}

fn push_circle(ops: &mut Vec<PathOp>, cx: f64, cy: f64, r: f64) {
    ops.push(PathOp::MoveTo(cx + r, cy));
    ops.push(PathOp::Arc {
        cx,
        cy,
        r,
        a0: 0.0,
        a1: TAU,
    });
    ops.push(PathOp::Close);
}

/// Twice the signed area of a ring (positive for the canvas' clockwise-on-screen winding).
fn signed_area2(ring: &[[f64; 2]]) -> f64 {
    let n = ring.len();
    (0..n)
        .map(|i| {
            let (a, b) = (ring[i], ring[(i + 1) % n]);
            a[0] * b[1] - b[0] * a[1]
        })
        .sum()
}

fn push_ring(ops: &mut Vec<PathOp>, ring: &[[f64; 2]], normalize: bool) {
    let reversed = normalize && signed_area2(ring) < 0.0;
    let mut points: Box<dyn Iterator<Item = &[f64; 2]>> = if reversed {
        Box::new(ring.iter().rev())
    } else {
        Box::new(ring.iter())
    };
    if let Some(first) = points.next() {
        ops.push(PathOp::MoveTo(first[0], first[1]));
        ops.extend(points.map(|p| PathOp::LineTo(p[0], p[1])));
        ops.push(PathOp::Close);
    }
}

/// Serialise ops as SVG path data, so a browser `Path2D` can be built in one call
/// rather than one wasm-to-JS call per segment.
///
/// Canvas-style arcs become SVG endpoint arcs. The pen is assumed to already sit
/// at the arc's start point, which `compile` guarantees.
pub fn to_svg_path(ops: &[PathOp]) -> String {
    use std::fmt::Write;

    let mut d = String::with_capacity(ops.len() * 24);
    for op in ops {
        // Writing to a String cannot fail.
        let _ = match *op {
            PathOp::MoveTo(x, y) => write!(d, "M{}", Pt(x, y)),
            PathOp::LineTo(x, y) => write!(d, "L{}", Pt(x, y)),
            PathOp::BezierTo { c1, c2, end } => write!(
                d,
                "C{} {} {}",
                Pt(c1[0], c1[1]),
                Pt(c2[0], c2[1]),
                Pt(end[0], end[1])
            ),
            PathOp::Close => write!(d, "Z"),
            PathOp::Arc { cx, cy, r, a0, a1 } => {
                let at = |a: f64| Pt(cx + r * a.cos(), cy + r * a.sin());
                let r = Num(r);
                let sweep = arc_sweep(a0, a1);
                if sweep >= TAU - 1e-9 {
                    // SVG cannot draw a full circle as one arc; use two halves.
                    write!(
                        d,
                        "A{r} {r} 0 0 1 {}A{r} {r} 0 0 1 {}",
                        at(a0 + TAU / 2.0),
                        at(a0)
                    )
                } else if sweep > 0.0 {
                    let large = u8::from(sweep > TAU / 2.0);
                    write!(d, "A{r} {r} 0 {large} 1 {}", at(a0 + sweep))
                } else {
                    Ok(())
                }
            }
        };
    }
    d
}

/// Sweep of a canvas `arc(a0, a1)` drawn with increasing angle, in `[0, 2π]`.
fn arc_sweep(a0: f64, a1: f64) -> f64 {
    let raw = a1 - a0;
    if raw >= TAU {
        TAU
    } else {
        raw.rem_euclid(TAU)
    }
}

/// Coordinate formatted to 0.1 µm, which is plenty for fabrication data in mm.
struct Num(f64);

impl std::fmt::Display for Num {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        let v = (self.0 * 1e4).round() / 1e4;
        // Avoid "-0".
        write!(f, "{}", if v == 0.0 { 0.0 } else { v })
    }
}

struct Pt(f64, f64);

impl std::fmt::Display for Pt {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{} {}", Num(self.0), Num(self.1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ring_points(ops: &[PathOp]) -> Vec<[f64; 2]> {
        ops.iter()
            .filter_map(|op| match op {
                PathOp::MoveTo(x, y) | PathOp::LineTo(x, y) => Some([*x, *y]),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn strokes_grouped_by_width() {
        let seg = |w: f64| Drawing::Segment {
            start: [0.0, 0.0],
            end: [1.0, 0.0],
            width: w,
        };
        let out = compile(&[seg(0.2), seg(0.1), seg(0.2)]);
        assert!(out.fill.is_empty());
        let widths: Vec<f64> = out.strokes.iter().map(|(w, _)| *w).collect();
        assert_eq!(widths, [0.1, 0.2]);
        assert_eq!(out.strokes[1].1.len(), 4);
    }

    #[test]
    fn fill_rings_share_winding() {
        let cw = vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        let ccw: Vec<[f64; 2]> = cw.iter().rev().copied().collect();
        let poly = |pts: Vec<[f64; 2]>| Drawing::Polygon {
            pos: [0.0, 0.0],
            angle: 0.0,
            polygons: vec![pts],
            filled: Some(1),
            width: 0.0,
        };
        let out = compile(&[poly(cw), poly(ccw)]);
        let pts = ring_points(&out.fill);
        assert_eq!(pts.len(), 8);
        assert!(signed_area2(&pts[..4]) > 0.0);
        assert!(signed_area2(&pts[4..]) > 0.0);
    }

    #[test]
    fn flashed_rect_fills_outline_rect_strokes() {
        let flash = Drawing::Rect {
            start: [0.0, 0.0],
            end: [2.0, 1.0],
            width: 0.0,
        };
        let outline = Drawing::Rect {
            start: [0.0, 0.0],
            end: [2.0, 1.0],
            width: 0.1,
        };
        let out = compile(&[flash, outline]);
        assert_eq!(out.fill.len(), 5);
        assert_eq!(out.strokes.len(), 1);
    }

    #[test]
    fn polygon_placement_applies_pos_and_angle() {
        let d = Drawing::Polygon {
            pos: [10.0, 5.0],
            angle: 90.0,
            polygons: vec![vec![[1.0, 0.0], [0.0, 0.0], [0.0, 1.0]]],
            filled: Some(0),
            width: 0.05,
        };
        let out = compile(&[d]);
        let pts = ring_points(&out.strokes[0].1);
        // Rotating by -90deg (Y-down screen) maps (1,0) -> (0,-1).
        assert!((pts[0][0] - 10.0).abs() < 1e-9 && (pts[0][1] - 4.0).abs() < 1e-9);
    }

    #[test]
    fn arc_starts_on_circumference() {
        let d = Drawing::Arc {
            start: [0.0, 0.0],
            radius: 2.0,
            startangle: 90.0,
            endangle: 180.0,
            width: 0.1,
        };
        let out = compile(&[d]);
        match out.strokes[0].1[0] {
            PathOp::MoveTo(x, y) => assert!(x.abs() < 1e-9 && (y - 2.0).abs() < 1e-9),
            ref other => panic!("expected MoveTo, got {other:?}"),
        }
    }

    #[test]
    fn svg_path_lines_and_close() {
        let ops = [
            PathOp::MoveTo(0.0, -0.0),
            PathOp::LineTo(1.23456789, 2.0),
            PathOp::Close,
        ];
        assert_eq!(to_svg_path(&ops), "M0 0L1.2346 2Z");
    }

    #[test]
    fn svg_path_full_circle_is_two_half_arcs() {
        let mut ops = Vec::new();
        push_circle(&mut ops, 1.0, 1.0, 0.5);
        assert_eq!(
            to_svg_path(&ops),
            "M1.5 1A0.5 0.5 0 0 1 0.5 1A0.5 0.5 0 0 1 1.5 1Z"
        );
    }

    #[test]
    fn svg_path_partial_arcs_pick_large_flag_and_wrap() {
        let arc = |a0: f64, a1: f64| {
            to_svg_path(&[PathOp::Arc {
                cx: 0.0,
                cy: 0.0,
                r: 1.0,
                a0: a0.to_radians(),
                a1: a1.to_radians(),
            }])
        };
        assert_eq!(arc(0.0, 90.0), "A1 1 0 0 1 0 1");
        assert_eq!(arc(0.0, 270.0), "A1 1 0 1 1 0 -1");
        // Canvas semantics: 350deg -> 10deg sweeps 20deg forward, ending at 10deg.
        let wrapped = arc(350.0, 10.0);
        assert!(wrapped.starts_with("A1 1 0 0 1 0.9848 0.1736"), "{wrapped}");
        assert_eq!(arc(45.0, 45.0), "");
    }

    #[test]
    fn filled_circles_go_to_fill() {
        let d = Drawing::Circle {
            start: [1.0, 1.0],
            radius: 0.5,
            width: 0.0,
            filled: Some(1),
        };
        let out = compile(&[d]);
        assert_eq!(out.fill.len(), 3);
        assert!(out.strokes.is_empty());
    }
}
