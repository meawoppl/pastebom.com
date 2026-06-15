//! Small SVG path/geometry helpers shared by the SVG render paths.
//!
//! Lifted out of `thumbnail.rs` so the thumbnail generator and the GDSII tile
//! renderer emit identical `d`-strings, view boxes, and stroke widths instead
//! of each re-implementing them (and drifting).

use std::fmt::Write;

use crate::types::BBox;

/// Open polyline `d`-string: `M x y L x y …` with 4-decimal coordinates.
/// Points are emitted as given (apply any rotation/offset before calling).
pub fn polyline_to_d(pts: &[[f64; 2]]) -> String {
    let mut d = String::with_capacity(pts.len() * 18);
    for (i, p) in pts.iter().enumerate() {
        if i == 0 {
            let _ = write!(d, "M{:.4} {:.4}", p[0], p[1]);
        } else {
            let _ = write!(d, "L{:.4} {:.4}", p[0], p[1]);
        }
    }
    d
}

/// Closed multi-ring polygon `d`-string: each ring rendered as `M…L…Z` and
/// concatenated (use with `fill-rule="evenodd"` for holes). Rings with fewer
/// than two points are skipped.
pub fn poly_to_d(rings: &[Vec<[f64; 2]>]) -> String {
    let mut d = String::new();
    for ring in rings {
        if ring.len() < 2 {
            continue;
        }
        d.push_str(&polyline_to_d(ring));
        d.push('Z');
    }
    d
}

/// Clamp a stroke width to a minimum visible value so hairline strokes don't
/// vanish at small scales.
pub fn stroke_w(w: f64) -> f64 {
    if w < 0.1 {
        0.1
    } else {
        w
    }
}

/// SVG `viewBox` tuple `(x, y, w, h)` for a bbox expanded by `margin` on all
/// sides.
pub fn view_box(b: &BBox, margin: f64) -> (f64, f64, f64, f64) {
    (
        b.minx - margin,
        b.miny - margin,
        (b.maxx - b.minx) + 2.0 * margin,
        (b.maxy - b.miny) + 2.0 * margin,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn polyline_format() {
        let d = polyline_to_d(&[[0.0, 0.0], [1.5, 2.0]]);
        assert_eq!(d, "M0.0000 0.0000L1.5000 2.0000");
    }

    #[test]
    fn poly_closes_each_ring() {
        let d = poly_to_d(&[vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]]]);
        assert_eq!(d, "M0.0000 0.0000L1.0000 0.0000L0.0000 1.0000Z");
        // sub-2-point rings are skipped
        assert_eq!(poly_to_d(&[vec![[0.0, 0.0]]]), "");
    }

    #[test]
    fn stroke_clamps_below_min() {
        assert_eq!(stroke_w(0.0), 0.1);
        assert_eq!(stroke_w(0.05), 0.1);
        assert_eq!(stroke_w(0.3), 0.3);
    }

    #[test]
    fn view_box_expands_by_margin() {
        let b = BBox {
            minx: 0.0,
            miny: 0.0,
            maxx: 10.0,
            maxy: 4.0,
        };
        assert_eq!(view_box(&b, 2.0), (-2.0, -2.0, 14.0, 8.0));
    }
}
