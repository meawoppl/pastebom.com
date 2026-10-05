//! Board-to-screen transform: uniform scale, translation, and an optional
//! horizontal mirror for viewing the board from the bottom side.

use pcb_extract::types::BBox;

const MIN_SCALE: f64 = 1e-3;
const MAX_SCALE: f64 = 1e5;

/// Maps board coordinates (millimetres, Y-down) to CSS pixels in the canvas.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct View {
    /// CSS pixels per millimetre.
    pub scale: f64,
    /// Screen position of the board origin, in CSS pixels.
    pub tx: f64,
    pub ty: f64,
    /// Mirror X to show the board as seen from the bottom.
    pub mirrored: bool,
}

impl Default for View {
    fn default() -> Self {
        Self {
            scale: 1.0,
            tx: 0.0,
            ty: 0.0,
            mirrored: false,
        }
    }
}

impl View {
    fn sx(&self) -> f64 {
        if self.mirrored {
            -self.scale
        } else {
            self.scale
        }
    }

    /// Canvas 2D transform `[a, b, c, d, e, f]` for a backing store scaled by `dpr`.
    pub fn canvas_transform(&self, dpr: f64) -> [f64; 6] {
        [
            self.sx() * dpr,
            0.0,
            0.0,
            self.scale * dpr,
            self.tx * dpr,
            self.ty * dpr,
        ]
    }

    /// Fit `bbox` inside a `width` x `height` viewport with `padding` CSS pixels on each side.
    pub fn fit(&mut self, bbox: &BBox, width: f64, height: f64, padding: f64) {
        let bw = (bbox.maxx - bbox.minx).max(1e-6);
        let bh = (bbox.maxy - bbox.miny).max(1e-6);
        let avail_w = (width - 2.0 * padding).max(1.0);
        let avail_h = (height - 2.0 * padding).max(1.0);
        self.scale = (avail_w / bw).min(avail_h / bh).clamp(MIN_SCALE, MAX_SCALE);

        let cx = (bbox.minx + bbox.maxx) / 2.0;
        let cy = (bbox.miny + bbox.maxy) / 2.0;
        self.tx = width / 2.0 - cx * self.sx();
        self.ty = height / 2.0 - cy * self.scale;
    }

    /// Zoom by `factor`, keeping the board point under screen position `(px, py)` fixed.
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

    /// Switch between top and bottom views, mirroring about the viewport centre line.
    pub fn set_mirrored(&mut self, mirrored: bool, width: f64) {
        if self.mirrored != mirrored {
            self.tx = width - self.tx;
            self.mirrored = mirrored;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    impl View {
        fn to_screen(self, x: f64, y: f64) -> (f64, f64) {
            (x * self.sx() + self.tx, y * self.scale + self.ty)
        }

        fn to_board(self, px: f64, py: f64) -> (f64, f64) {
            ((px - self.tx) / self.sx(), (py - self.ty) / self.scale)
        }
    }

    fn close(a: (f64, f64), b: (f64, f64)) -> bool {
        (a.0 - b.0).abs() < 1e-9 && (a.1 - b.1).abs() < 1e-9
    }

    fn board() -> BBox {
        BBox {
            minx: 10.0,
            miny: -30.0,
            maxx: 60.0,
            maxy: -10.0,
        }
    }

    #[test]
    fn fit_centres_and_scales_to_limiting_axis() {
        let mut v = View::default();
        v.fit(&board(), 600.0, 400.0, 50.0);
        // Width-limited: 500px / 50mm = 10 px/mm (height would allow 15).
        assert!((v.scale - 10.0).abs() < 1e-9);
        assert!(close(v.to_screen(35.0, -20.0), (300.0, 200.0)));
        assert!(close(v.to_screen(10.0, -20.0), (50.0, 200.0)));
    }

    #[test]
    fn round_trip_screen_board() {
        let mut v = View::default();
        v.fit(&board(), 800.0, 600.0, 20.0);
        v.mirrored = true;
        let (px, py) = v.to_screen(12.5, -17.0);
        assert!(close(v.to_board(px, py), (12.5, -17.0)));
    }

    #[test]
    fn zoom_keeps_cursor_point_fixed() {
        let mut v = View::default();
        v.fit(&board(), 800.0, 600.0, 20.0);
        let before = v.to_board(123.0, 456.0);
        v.zoom_at(123.0, 456.0, 2.5);
        assert!(close(v.to_board(123.0, 456.0), before));
    }

    #[test]
    fn mirror_flips_about_viewport_centre() {
        let mut v = View::default();
        v.fit(&board(), 600.0, 400.0, 50.0);
        v.set_mirrored(true, 600.0);
        // The board centre stays at the viewport centre; the left edge moves right.
        assert!(close(v.to_screen(35.0, -20.0), (300.0, 200.0)));
        assert!(close(v.to_screen(10.0, -20.0), (550.0, 200.0)));
        v.set_mirrored(false, 600.0);
        assert!(close(v.to_screen(10.0, -20.0), (50.0, 200.0)));
    }

    #[test]
    fn canvas_transform_includes_mirror_and_dpr() {
        let v = View {
            scale: 4.0,
            tx: 10.0,
            ty: 20.0,
            mirrored: true,
        };
        assert_eq!(v.canvas_transform(2.0), [-8.0, 0.0, 0.0, 8.0, 20.0, 40.0]);
    }
}
