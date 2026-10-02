//! Scene-to-screen transform: uniform scale, rotation, optional horizontal
//! mirror (bottom-side view), optional Y flip (for y-up scenes), translation.
//!
//! `screen = t + scale * F * R(rotation) * p`, where `F = diag(±1, ±1)` holds
//! the mirror and Y flip. Screen units are CSS pixels; [`View::canvas_transform`]
//! folds in the device pixel ratio.

use crate::scene::{BBox, Point, Scene};

pub const MIN_SCALE: f64 = 1e-3;
pub const MAX_SCALE: f64 = 1e5;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct View {
    /// CSS pixels per scene unit (millimetre).
    pub scale: f64,
    /// Screen position of the scene origin, in CSS pixels.
    pub tx: f64,
    pub ty: f64,
    /// Rotation in radians applied in scene axes before mirroring (the same
    /// sense as `Prim::Arc` angles: from +x toward +y).
    pub rotation: f64,
    /// Mirror X, e.g. to show a board as seen from the bottom.
    pub mirrored: bool,
    /// Flip Y so +y points up the screen (scenes with `y_down == false`).
    pub y_up: bool,
}

impl Default for View {
    fn default() -> Self {
        Self {
            scale: 1.0,
            tx: 0.0,
            ty: 0.0,
            rotation: 0.0,
            mirrored: false,
            y_up: false,
        }
    }
}

impl View {
    /// A view with the axis convention of `scene` (flips Y for y-up scenes).
    pub fn for_scene(scene: &Scene) -> Self {
        Self {
            y_up: !scene.y_down,
            ..Self::default()
        }
    }

    /// Linear part `[a, b, c, d]` mapping scene vectors to screen vectors:
    /// `sx = a*x + c*y`, `sy = b*x + d*y`.
    pub fn linear(&self) -> [f64; 4] {
        let (s, c) = self.rotation.sin_cos();
        let fx = if self.mirrored { -1.0 } else { 1.0 };
        let fy = if self.y_up { -1.0 } else { 1.0 };
        [
            self.scale * fx * c,
            self.scale * fy * s,
            -self.scale * fx * s,
            self.scale * fy * c,
        ]
    }

    pub fn to_screen(&self, p: Point) -> Point {
        let [a, b, c, d] = self.linear();
        [a * p[0] + c * p[1] + self.tx, b * p[0] + d * p[1] + self.ty]
    }

    pub fn to_world(&self, s: Point) -> Point {
        let [a, b, c, d] = self.linear();
        let det = a * d - b * c;
        let x = s[0] - self.tx;
        let y = s[1] - self.ty;
        [(d * x - c * y) / det, (-b * x + a * y) / det]
    }

    /// Scene units per CSS pixel (for tolerances and minimum line widths).
    pub fn world_per_px(&self) -> f64 {
        1.0 / self.scale
    }

    /// Canvas 2D transform `[a, b, c, d, e, f]` for a backing store scaled by `dpr`.
    pub fn canvas_transform(&self, dpr: f64) -> [f64; 6] {
        let [a, b, c, d] = self.linear();
        [
            a * dpr,
            b * dpr,
            c * dpr,
            d * dpr,
            self.tx * dpr,
            self.ty * dpr,
        ]
    }

    /// Screen-space bounds of `bbox` under the current linear part (ignoring translation).
    fn projected_extent(&self, bbox: &BBox) -> BBox {
        let [a, b, c, d] = self.linear();
        let mut out = BBox::EMPTY;
        for p in [
            bbox.min,
            [bbox.max[0], bbox.min[1]],
            bbox.max,
            [bbox.min[0], bbox.max[1]],
        ] {
            out.include([a * p[0] + c * p[1], b * p[0] + d * p[1]]);
        }
        out
    }

    /// Fit `bbox` inside a `width` x `height` viewport with `padding` CSS
    /// pixels on each side, honouring the current rotation and mirroring.
    pub fn fit(&mut self, bbox: &BBox, width: f64, height: f64, padding: f64) {
        if bbox.is_empty() {
            self.scale = 1.0;
            self.tx = width / 2.0;
            self.ty = height / 2.0;
            return;
        }
        let unit = View {
            scale: 1.0,
            ..*self
        };
        let ext = unit.projected_extent(bbox);
        let bw = ext.width().max(1e-6);
        let bh = ext.height().max(1e-6);
        let avail_w = (width - 2.0 * padding).max(1.0);
        let avail_h = (height - 2.0 * padding).max(1.0);
        self.scale = (avail_w / bw).min(avail_h / bh).clamp(MIN_SCALE, MAX_SCALE);
        self.tx = 0.0;
        self.ty = 0.0;
        let centre = self.to_screen([
            (bbox.min[0] + bbox.max[0]) / 2.0,
            (bbox.min[1] + bbox.max[1]) / 2.0,
        ]);
        self.tx = width / 2.0 - centre[0];
        self.ty = height / 2.0 - centre[1];
    }

    /// Zoom by `factor`, keeping the scene point under screen `(px, py)` fixed.
    pub fn zoom_at(&mut self, px: f64, py: f64, factor: f64) {
        let new_scale = (self.scale * factor).clamp(MIN_SCALE, MAX_SCALE);
        let applied = new_scale / self.scale;
        self.tx = px - (px - self.tx) * applied;
        self.ty = py - (py - self.ty) * applied;
        self.scale = new_scale;
    }

    pub fn pan(&mut self, dx: f64, dy: f64) {
        self.tx += dx;
        self.ty += dy;
    }

    /// Switch mirroring, flipping about the vertical centre line of a
    /// `width`-wide viewport so the scene point at the centre stays put.
    pub fn set_mirrored(&mut self, mirrored: bool, width: f64) {
        if self.mirrored != mirrored {
            self.tx = width - self.tx;
            self.mirrored = mirrored;
        }
    }

    /// Set the rotation, keeping the scene point under screen `pivot` fixed.
    pub fn set_rotation(&mut self, rotation: f64, pivot: Point) {
        let w = self.to_world(pivot);
        self.rotation = rotation;
        self.tx = 0.0;
        self.ty = 0.0;
        let s = self.to_screen(w);
        self.tx = pivot[0] - s[0];
        self.ty = pivot[1] - s[1];
    }

    /// The scene-space rectangle visible in a `width` x `height` viewport.
    pub fn visible_bbox(&self, width: f64, height: f64) -> BBox {
        let mut b = BBox::EMPTY;
        for s in [[0.0, 0.0], [width, 0.0], [width, height], [0.0, height]] {
            b.include(self.to_world(s));
        }
        b
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Point, b: Point) -> bool {
        (a[0] - b[0]).abs() < 1e-9 && (a[1] - b[1]).abs() < 1e-9
    }

    fn board() -> BBox {
        BBox {
            min: [10.0, -30.0],
            max: [60.0, -10.0],
        }
    }

    #[test]
    fn fit_centres_and_scales_to_limiting_axis() {
        let mut v = View::default();
        v.fit(&board(), 600.0, 400.0, 50.0);
        // Width-limited: 500px / 50mm = 10 px/mm (height would allow 15).
        assert!((v.scale - 10.0).abs() < 1e-9);
        assert!(close(v.to_screen([35.0, -20.0]), [300.0, 200.0]));
        assert!(close(v.to_screen([10.0, -20.0]), [50.0, 200.0]));
    }

    #[test]
    fn fit_accounts_for_rotation() {
        let mut v = View {
            rotation: std::f64::consts::FRAC_PI_2,
            ..View::default()
        };
        v.fit(&board(), 600.0, 400.0, 50.0);
        // Rotated 90deg the board is 20 wide, 50 tall: height-limited 300/50.
        assert!((v.scale - 6.0).abs() < 1e-9);
        assert!(close(v.to_screen([35.0, -20.0]), [300.0, 200.0]));
    }

    #[test]
    fn round_trip_screen_world_all_flags() {
        for mirrored in [false, true] {
            for y_up in [false, true] {
                let mut v = View {
                    rotation: 0.7,
                    mirrored,
                    y_up,
                    ..View::default()
                };
                v.fit(&board(), 800.0, 600.0, 20.0);
                let s = v.to_screen([12.5, -17.0]);
                assert!(close(v.to_world(s), [12.5, -17.0]));
            }
        }
    }

    #[test]
    fn zoom_keeps_cursor_point_fixed() {
        let mut v = View::default();
        v.fit(&board(), 800.0, 600.0, 20.0);
        let before = v.to_world([123.0, 456.0]);
        v.zoom_at(123.0, 456.0, 2.5);
        assert!(close(v.to_world([123.0, 456.0]), before));
    }

    #[test]
    fn zoom_is_clamped() {
        let mut v = View::default();
        v.zoom_at(0.0, 0.0, 1e12);
        assert_eq!(v.scale, MAX_SCALE);
        v.zoom_at(0.0, 0.0, 1e-20);
        assert_eq!(v.scale, MIN_SCALE);
    }

    #[test]
    fn mirror_flips_about_viewport_centre() {
        let mut v = View::default();
        v.fit(&board(), 600.0, 400.0, 50.0);
        v.set_mirrored(true, 600.0);
        assert!(close(v.to_screen([35.0, -20.0]), [300.0, 200.0]));
        assert!(close(v.to_screen([10.0, -20.0]), [550.0, 200.0]));
        v.set_mirrored(false, 600.0);
        assert!(close(v.to_screen([10.0, -20.0]), [50.0, 200.0]));
    }

    #[test]
    fn y_up_flips_vertical() {
        let mut v = View {
            y_up: true,
            ..View::default()
        };
        v.fit(&board(), 600.0, 400.0, 50.0);
        let top = v.to_screen([35.0, -10.0]);
        let bottom = v.to_screen([35.0, -30.0]);
        assert!(top[1] < bottom[1]);
    }

    #[test]
    fn rotation_keeps_pivot_fixed() {
        let mut v = View::default();
        v.fit(&board(), 600.0, 400.0, 50.0);
        let w = v.to_world([300.0, 200.0]);
        v.set_rotation(1.0, [300.0, 200.0]);
        assert!(close(v.to_world([300.0, 200.0]), w));
        // Positive rotation turns +x toward +y (clockwise on a y-down screen).
        let a = v.to_screen([0.0, 0.0]);
        let b = v.to_screen([1.0, 0.0]);
        assert!(b[1] > a[1]);
    }

    #[test]
    fn canvas_transform_includes_mirror_and_dpr() {
        let v = View {
            scale: 4.0,
            tx: 10.0,
            ty: 20.0,
            mirrored: true,
            ..View::default()
        };
        assert_eq!(v.canvas_transform(2.0), [-8.0, 0.0, -0.0, 8.0, 20.0, 40.0]);
    }

    #[test]
    fn visible_bbox_inverts_viewport() {
        let mut v = View::default();
        v.fit(&board(), 600.0, 400.0, 50.0);
        let b = v.visible_bbox(600.0, 400.0);
        assert!(b.contains([10.0, -20.0]) && b.contains([60.0, -20.0]));
        assert!(!b.contains([0.0, -20.0]));
    }
}
