//! Pin shafts and electrical-style decorations in sheet coordinates.

use vector_view::Prim;

const WIDTH: f64 = 0.2;
const MARKER_RADIUS: f64 = 0.5;

/// Draw a pin from its electrical connection to the symbol body.
///
/// Marker geometry follows the pin direction, including arbitrary rotations.
/// Bubbles occupy the outside of the body and clocks point into the body.
/// Unknown styles retain the ordinary shaft. Zero-length pins have no oriented
/// decorations, and short pins scale their markers to fit the available shaft.
pub(crate) fn primitives(at: [f64; 2], end: [f64; 2], shape: &str) -> Vec<Prim> {
    let delta = [end[0] - at[0], end[1] - at[1]];
    let length = delta[0].hypot(delta[1]);
    let shaft = |tip| Prim::Polyline {
        points: vec![at, tip],
        width: WIDTH,
    };
    if !length.is_finite() || length <= f64::EPSILON {
        return vec![shaft(end)];
    }

    let direction = [delta[0] / length, delta[1] / length];
    let radius = MARKER_RADIUS.min(length / 2.0);
    // Positive x lies inside the body; negative y lies above a rightward pin.
    let point = |x: f64, y: f64| {
        [
            end[0] + x * direction[0] - y * direction[1],
            end[1] + x * direction[1] + y * direction[0],
        ]
    };
    let path = |vertices: &[[f64; 2]]| Prim::Polyline {
        points: vertices.iter().map(|p| point(p[0], p[1])).collect(),
        width: WIDTH,
    };

    let inverted = matches!(shape, "inverted" | "inverted_clock");
    let mut result = vec![shaft(if inverted {
        point(-2.0 * radius, 0.0)
    } else {
        end
    })];

    if inverted {
        result.push(Prim::Circle {
            center: point(-radius, 0.0),
            radius,
            fill: false,
            stroke: WIDTH,
        });
    }
    if matches!(
        shape,
        "clock" | "inverted_clock" | "clock_low" | "edge_clock_high"
    ) {
        result.push(path(&[[0.0, -radius], [radius, 0.0], [0.0, radius]]));
    }
    if matches!(shape, "input_low" | "clock_low" | "edge_clock_high") {
        result.push(path(&[
            [-2.0 * radius, 0.0],
            [-2.0 * radius, -2.0 * radius],
            [0.0, 0.0],
        ]));
    }
    if shape == "output_low" {
        result.push(path(&[[0.0, -2.0 * radius], [-2.0 * radius, 0.0]]));
    }
    if shape == "non_logic" {
        result.push(path(&[[-radius, -radius], [radius, radius]]));
        result.push(path(&[[-radius, radius], [radius, -radius]]));
    }
    result
}

#[cfg(test)]
mod tests {
    use super::primitives;
    use vector_view::Prim;

    fn points(prim: &Prim) -> &[[f64; 2]] {
        match prim {
            Prim::Polyline { points, .. } => points,
            _ => panic!("expected a polyline"),
        }
    }

    #[test]
    fn inverted_clock_keeps_bubble_clear_and_clock_inside_body() {
        let drawn = primitives([0.0, 0.0], [4.0, 0.0], "inverted_clock");
        assert_eq!(points(&drawn[0]), [[0.0, 0.0], [3.0, 0.0]]);
        assert!(matches!(
            drawn[1],
            Prim::Circle {
                center: [3.5, 0.0],
                radius: 0.5,
                fill: false,
                ..
            }
        ));
        assert_eq!(points(&drawn[2]), [[4.0, -0.5], [4.5, 0.0], [4.0, 0.5]]);
    }

    #[test]
    fn decorations_rotate_with_the_pin() {
        let styles = [
            "line",
            "inverted",
            "clock",
            "inverted_clock",
            "input_low",
            "clock_low",
            "output_low",
            "edge_clock_high",
            "non_logic",
        ];
        for style in styles {
            let horizontal = primitives([0.0, 0.0], [4.0, 0.0], style);
            let vertical = primitives([0.0, 0.0], [0.0, 4.0], style);
            assert_eq!(horizontal.len(), vertical.len());
            for (original, rotated) in horizontal.iter().zip(&vertical) {
                match (original, rotated) {
                    (Prim::Polyline { points: a, .. }, Prim::Polyline { points: b, .. }) => {
                        assert_eq!(a.len(), b.len());
                        for (a, b) in a.iter().zip(b) {
                            assert_eq!([-a[1], a[0]], *b, "{style}");
                        }
                    }
                    (
                        Prim::Circle {
                            center: a,
                            radius: ra,
                            ..
                        },
                        Prim::Circle {
                            center: b,
                            radius: rb,
                            ..
                        },
                    ) => {
                        assert_eq!([-a[1], a[0]], *b);
                        assert_eq!(ra, rb);
                    }
                    _ => panic!("unexpected primitive"),
                }
            }
        }
    }

    #[test]
    fn low_markers_and_nonlogic_are_distinct() {
        let start = [0.0, 0.0];
        let end = [4.0, 0.0];
        let input = primitives(start, end, "input_low");
        let output = primitives(start, end, "output_low");
        assert_eq!(points(&input[1]).len(), 3);
        assert_eq!(points(&output[1]).len(), 2);
        assert_ne!(input, output);
        assert_eq!(primitives(start, end, "clock_low").len(), 3);
        assert_eq!(primitives(start, end, "non_logic").len(), 3);
        assert_eq!(
            primitives(start, end, "unknown"),
            primitives(start, end, "line")
        );
    }

    #[test]
    fn zero_and_short_pins_do_not_reverse_the_shaft() {
        let zero = primitives([2.0, 3.0], [2.0, 3.0], "inverted_clock");
        assert_eq!(zero.len(), 1);
        let short = primitives([0.0, 0.0], [0.2, 0.0], "inverted");
        assert_eq!(points(&short[0]), [[0.0, 0.0], [0.0, 0.0]]);
        assert!(matches!(short[1], Prim::Circle { radius: 0.1, .. }));
    }
}
