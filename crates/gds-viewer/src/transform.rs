//! Pan/zoom transform and slippy-map pyramid geometry.
//!
//! `Transform` mirrors the PCB viewer's pan/zoom state (`panx`/`pany`/`zoom`),
//! reusing the same cursor-centred wheel and pointer math. On top of it we map
//! a continuous `zoom` onto a discrete tile level plus a fractional CSS scale.

use crate::manifest::Manifest;

/// Continuous pan/zoom state, in CSS pixels at the base (level-0) scale.
///
/// `panx`/`pany` translate the world surface; `zoom` is the continuous scale.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transform {
    pub panx: f64,
    pub pany: f64,
    pub zoom: f64,
}

impl Default for Transform {
    fn default() -> Self {
        Self {
            panx: 0.0,
            pany: 0.0,
            zoom: 1.0,
        }
    }
}

/// Pyramid geometry derived once from the manifest.
#[derive(Clone, Debug)]
pub struct Pyramid {
    pub tile_px: f64,
    /// Square world span (nm) covered by the whole design at level 0.
    pub world_span: f64,
    pub min_z: u32,
    pub max_z: u32,
    pub extent_w: f64,
    pub extent_h: f64,
}

impl Pyramid {
    pub fn from_manifest(m: &Manifest) -> Self {
        let w = m.extent_nm.width();
        let h = m.extent_nm.height();
        // Level 0 fits the whole design in a single tile: the world edge is the
        // next power of two above max(W, H) so deeper levels bisect cleanly.
        let world_span = ceil_pow2(w.max(h));
        Self {
            tile_px: m.tile_px.max(1) as f64,
            world_span,
            min_z: m.zoom.min,
            max_z: m.zoom.max.max(m.zoom.min),
            extent_w: w,
            extent_h: h,
        }
    }

    /// World units per pixel at level `z`.
    pub fn res(&self, z: u32) -> f64 {
        self.span(z) / self.tile_px
    }

    /// World units along one tile edge at level `z`.
    pub fn span(&self, z: u32) -> f64 {
        self.world_span / (1u64 << z) as f64
    }

    /// Number of tiles per axis at level `z`.
    pub fn tiles_per_axis(&self, z: u32) -> u32 {
        1u32 << z
    }

    /// Map a continuous zoom (CSS scale relative to level-0 fit) to a discrete
    /// tile level. `base` aligns zoom == 1.0 to the level whose resolution shows
    /// the whole design at the viewport's base scale.
    pub fn level_for_zoom(&self, zoom: f64) -> u32 {
        let z = (zoom.max(1e-6).log2().round() as i64) + self.min_z as i64;
        z.clamp(self.min_z as i64, self.max_z as i64) as u32
    }

    /// Fractional CSS scale to apply on top of the chosen level so continuous
    /// zoom stays smooth between discrete levels.
    pub fn frac_scale(&self, zoom: f64, z: u32) -> f64 {
        // The chosen level covers 2^(z - min_z) of the level-0 fit.
        let level_zoom = 2f64.powi((z as i32) - (self.min_z as i32));
        (zoom / level_zoom).max(1e-6)
    }
}

/// Round `v` up to the nearest power of two (as an f64 world span).
fn ceil_pow2(v: f64) -> f64 {
    if v <= 1.0 {
        return 1.0;
    }
    2f64.powf(v.log2().ceil())
}

/// Inclusive tile (x, y) range covering a viewport, given level `z`.
pub struct TileRange {
    pub x0: i64,
    pub y0: i64,
    pub x1: i64,
    pub y1: i64,
}

impl Pyramid {
    /// Compute the visible tile range for a viewport.
    ///
    /// `panx`/`pany`/`scale` describe the CSS placement of the level-0 surface:
    /// world point (0,0) of the surface maps to screen (panx, pany) and one
    /// world tile edge at level `z` maps to `tile_px * scale` screen pixels.
    pub fn visible_tiles(
        &self,
        z: u32,
        panx: f64,
        pany: f64,
        scale: f64,
        view_w: f64,
        view_h: f64,
    ) -> TileRange {
        let tile_screen = self.tile_px * scale;
        if tile_screen <= 0.0 {
            return TileRange {
                x0: 0,
                y0: 0,
                x1: 0,
                y1: 0,
            };
        }
        let n = self.tiles_per_axis(z) as i64;
        let x0 = ((-panx) / tile_screen).floor() as i64;
        let y0 = ((-pany) / tile_screen).floor() as i64;
        let x1 = ((view_w - panx) / tile_screen).floor() as i64;
        let y1 = ((view_h - pany) / tile_screen).floor() as i64;
        TileRange {
            x0: x0.clamp(0, n - 1),
            y0: y0.clamp(0, n - 1),
            x1: x1.clamp(0, n - 1),
            y1: y1.clamp(0, n - 1),
        }
    }
}
