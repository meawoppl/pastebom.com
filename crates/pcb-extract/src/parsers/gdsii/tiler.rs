//! GDSII tile pipeline — stage 2/3: pyramid geometry + per-layer tile rendering.
//!
//! Turns placed records into a slippy-map tile pyramid: per zoom level, per
//! `(x, y)` tile, one SVG per layer. Sub-pixel features are culled by on-tile
//! area and replaced with an annotated AABB overlay (`__lod`). Coordinates are
//! GDSII-native Y-up in world space; each tile's SVG flips Y once in its
//! `viewBox`, so tile math stays sign-clean.

use std::collections::HashMap;
use std::fmt::Write;
use std::io::Write as _;

use crate::svg;

use super::tile::{Geom, PlacedRecord, RecordKind, WorldBox};

/// Tile edge length in pixels.
pub const TILE_PX: u32 = 512;
/// Minimum on-tile pixel area for a record to be drawn; smaller features are
/// dropped to the LOD overlay.
pub const LOD_MIN_PX: f64 = 2.0;
/// Overlay bucket grid (cells per tile edge) for merging omitted records.
const OVERLAY_CELLS: i64 = 16;

/// Slippy-map pyramid geometry over a design extent.
///
/// Level 0 fits the whole design in one `TILE_PX` tile; each deeper level
/// doubles linear resolution (quadruples tile count). The pyramid is anchored
/// to a `root_span` square at `(bounds.minx, bounds.miny)`.
#[derive(Debug, Clone, Copy)]
pub struct Pyramid {
    pub bounds: WorldBox,
    pub tile_px: u32,
    pub levels: u32,
    pub root_span: i64,
}

impl Pyramid {
    /// Build a pyramid for `bounds`, deep enough that the finest level reaches
    /// roughly `res_min_nm_per_px` nanometers per pixel.
    pub fn new(bounds: WorldBox, res_min_nm_per_px: f64) -> Self {
        let w = (bounds.maxx - bounds.minx).max(1);
        let h = (bounds.maxy - bounds.miny).max(1);
        let root_span = ceil_pow2(w.max(h));
        let target = (root_span as f64 / (TILE_PX as f64 * res_min_nm_per_px.max(1e-9))).max(1.0);
        let levels = 1 + target.log2().ceil().max(0.0) as u32;
        Self {
            bounds,
            tile_px: TILE_PX,
            levels,
            root_span,
        }
    }

    /// World-nm edge length of a tile at level `z`.
    pub fn span(&self, z: u32) -> i64 {
        (self.root_span >> z).max(1)
    }

    /// World nanometers per pixel at level `z`.
    pub fn res(&self, z: u32) -> f64 {
        self.span(z) as f64 / self.tile_px as f64
    }

    /// Number of tiles per axis at level `z`.
    pub fn tiles_per_axis(&self, z: u32) -> u32 {
        1u32 << z
    }

    /// World AABB of tile `(z, x, y)`. Rows grow downward: `y = 0` is the top
    /// (highest world Y).
    pub fn tile_world_box(&self, z: u32, x: u32, y: u32) -> WorldBox {
        let span = self.span(z);
        let top = self.bounds.miny + self.root_span;
        let minx = self.bounds.minx + x as i64 * span;
        let maxy = top - y as i64 * span;
        WorldBox {
            minx,
            miny: maxy - span,
            maxx: minx + span,
            maxy,
        }
    }
}

fn ceil_pow2(n: i64) -> i64 {
    let mut p = 1i64;
    while p < n {
        p <<= 1;
    }
    p
}

// ─── Level-of-detail ────────────────────────────────────────────────────────

/// Partition records into those worth drawing at resolution `res_nm_per_px`
/// and those whose on-tile footprint is below [`LOD_MIN_PX`]. Text labels are
/// always kept (they carry their own legibility threshold elsewhere).
pub fn lod_partition<'a>(
    recs: &[&'a PlacedRecord],
    res_nm_per_px: f64,
) -> (Vec<&'a PlacedRecord>, Vec<&'a PlacedRecord>) {
    let inv = 1.0 / (res_nm_per_px * res_nm_per_px);
    let mut kept = Vec::new();
    let mut omitted = Vec::new();
    for &r in recs {
        if r.kind == RecordKind::Text {
            kept.push(r);
            continue;
        }
        let bw = (r.bbox.maxx - r.bbox.minx) as f64;
        let bh = (r.bbox.maxy - r.bbox.miny) as f64;
        if bw * bh * inv >= LOD_MIN_PX {
            kept.push(r);
        } else {
            omitted.push(r);
        }
    }
    (kept, omitted)
}

// ─── SVG tile rendering ──────────────────────────────────────────────────────

fn tile_open(tile: &WorldBox) -> String {
    let vx = tile.minx;
    let vy = -tile.maxy; // Y flip: world (x,y) -> (x,-y) under scale(1,-1)
    let vw = tile.maxx - tile.minx;
    let vh = tile.maxy - tile.miny;
    format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{TILE_PX}" height="{TILE_PX}" viewBox="{vx} {vy} {vw} {vh}"><g transform="scale(1,-1)">"#
    )
}

fn to_f64_rings(rings: &[Vec<[i64; 2]>]) -> Vec<Vec<[f64; 2]>> {
    rings
        .iter()
        .map(|r| r.iter().map(|p| [p[0] as f64, p[1] as f64]).collect())
        .collect()
}

/// Render one layer's kept records for a tile into a standalone SVG string.
pub fn render_layer_tile(recs: &[&PlacedRecord], tile: &WorldBox, color: &str) -> String {
    let mut s = tile_open(tile);
    for r in recs {
        match &r.geom {
            Geom::Poly { rings } => {
                let d = svg::poly_to_d(&to_f64_rings(rings));
                if !d.is_empty() {
                    let _ = write!(s, r#"<path d="{d}" fill="{color}" fill-rule="evenodd"/>"#);
                }
            }
            Geom::Path {
                pts, half_width, ..
            } => {
                let f: Vec<[f64; 2]> = pts.iter().map(|p| [p[0] as f64, p[1] as f64]).collect();
                let d = svg::polyline_to_d(&f);
                let w = ((2 * half_width).max(1)) as f64;
                let _ = write!(
                    s,
                    r#"<path d="{d}" fill="none" stroke="{color}" stroke-width="{w}" stroke-linecap="round"/>"#
                );
            }
            Geom::Label { at, text, mag, .. } => {
                // Counter-flip so text is upright inside the scale(1,-1) group.
                let fs = (mag * 1000.0).max(1.0);
                let _ = write!(
                    s,
                    r#"<text transform="translate({} {}) scale(1,-1)" font-size="{fs}" fill="{color}">{}</text>"#,
                    at[0],
                    at[1],
                    xml_escape(text)
                );
            }
        }
    }
    s.push_str("</g></svg>");
    s
}

/// Render the LOD overlay tile: omitted records merged into a coarse grid of
/// dashed AABB rectangles, each annotated with how many records it hides.
pub fn render_overlay(omitted: &[&PlacedRecord], tile: &WorldBox) -> String {
    let mut s = tile_open(tile);
    let lod = "#f7768e";
    if !omitted.is_empty() {
        let span = (tile.maxx - tile.minx).max(1);
        let cell = (span / OVERLAY_CELLS).max(1);
        let mut cells: HashMap<(i64, i64), (WorldBox, u32)> = HashMap::new();
        for &r in omitted {
            let cx = ((r.bbox.minx - tile.minx) / cell).clamp(0, OVERLAY_CELLS - 1);
            let cy = ((r.bbox.miny - tile.miny) / cell).clamp(0, OVERLAY_CELLS - 1);
            let entry = cells.entry((cx, cy)).or_insert((WorldBox::empty(), 0));
            entry.0.union(&r.bbox);
            entry.1 += 1;
        }
        // Deterministic emission order for stable output.
        let mut keys: Vec<_> = cells.keys().copied().collect();
        keys.sort_unstable();
        for k in keys {
            let (b, count) = &cells[&k];
            // Clip the merged box to the tile so the overlay stays in-bounds.
            let minx = b.minx.max(tile.minx);
            let miny = b.miny.max(tile.miny);
            let maxx = b.maxx.min(tile.maxx);
            let maxy = b.maxy.min(tile.maxy);
            let (w, h) = (maxx - minx, maxy - miny);
            if w <= 0 || h <= 0 {
                continue;
            }
            let sw = (span / 256).max(1);
            let _ = write!(
                s,
                r#"<rect x="{minx}" y="{miny}" width="{w}" height="{h}" fill="none" stroke="{lod}" stroke-width="{sw}" stroke-dasharray="{dash} {dash}" opacity="0.7"/>"#,
                dash = (span / 64).max(1),
            );
            let fs = (h as f64 / 6.0).max(1.0);
            let _ = write!(
                s,
                r#"<text transform="translate({tx} {ty}) scale(1,-1)" font-size="{fs}" fill="{lod}">{count} hidden</text>"#,
                tx = minx,
                ty = maxy,
            );
        }
    }
    s.push_str("</g></svg>");
    s
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Gzip an SVG string into a `.svgz` body (served with `Content-Encoding: gzip`).
pub fn svgz(svg: &str) -> Vec<u8> {
    let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    let _ = enc.write_all(svg.as_bytes());
    enc.finish().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::read::GzDecoder;
    use std::io::Read;

    fn poly(layer: i16, minx: i64, miny: i64, maxx: i64, maxy: i64) -> PlacedRecord {
        PlacedRecord {
            layer,
            datatype: 0,
            kind: RecordKind::Boundary,
            bbox: WorldBox {
                minx,
                miny,
                maxx,
                maxy,
            },
            geom: Geom::Poly {
                rings: vec![vec![[minx, miny], [maxx, miny], [maxx, maxy], [minx, maxy]]],
            },
        }
    }

    #[test]
    fn pyramid_level0_covers_design_in_one_tile() {
        let b = WorldBox {
            minx: 0,
            miny: 0,
            maxx: 3000,
            maxy: 2000,
        };
        let p = Pyramid::new(b, 1.0);
        assert_eq!(p.tiles_per_axis(0), 1);
        let t0 = p.tile_world_box(0, 0, 0);
        // The single level-0 tile spans the root_span square covering the design.
        assert!(t0.minx <= b.minx && t0.maxx >= b.maxx);
        assert!(t0.miny <= b.miny && t0.maxy >= b.maxy);
    }

    #[test]
    fn pyramid_resolution_halves_each_level() {
        let b = WorldBox {
            minx: 0,
            miny: 0,
            maxx: 4096,
            maxy: 4096,
        };
        let p = Pyramid::new(b, 1.0);
        assert!((p.res(0) - 2.0 * p.res(1)).abs() < 1e-9);
        assert_eq!(p.span(1), p.span(0) / 2);
    }

    #[test]
    fn tiles_partition_their_level() {
        let b = WorldBox {
            minx: 0,
            miny: 0,
            maxx: 4096,
            maxy: 4096,
        };
        let p = Pyramid::new(b, 1.0);
        // At z=1 there are 2x2 tiles; adjacent tiles abut exactly.
        let a = p.tile_world_box(1, 0, 0);
        let r = p.tile_world_box(1, 1, 0);
        assert_eq!(a.maxx, r.minx);
        let below = p.tile_world_box(1, 0, 1);
        assert_eq!(a.miny, below.maxy);
    }

    #[test]
    fn render_emits_path_and_flips_y() {
        let recs = [&poly(5, 0, 0, 100, 100)];
        let tile = WorldBox {
            minx: 0,
            miny: 0,
            maxx: 512,
            maxy: 512,
        };
        let s = render_layer_tile(&recs, &tile, "#7aa2f7");
        assert!(s.contains("<path"));
        assert!(s.contains("fill=\"#7aa2f7\""));
        assert!(s.contains(r#"transform="scale(1,-1)""#));
        // viewBox y is negated maxy.
        assert!(s.contains("viewBox=\"0 -512 512 512\""));
    }

    #[test]
    fn lod_drops_subpixel_records() {
        // res = 10 nm/px. A 5x5nm box => 0.25 px^2 < 2 => omitted.
        // A 100x100nm box => 100 px^2 => kept.
        let big = poly(1, 0, 0, 100, 100);
        let tiny = poly(1, 0, 0, 5, 5);
        let recs = [&big, &tiny];
        let (kept, omitted) = lod_partition(&recs, 10.0);
        assert_eq!(kept.len(), 1);
        assert_eq!(omitted.len(), 1);
        assert_eq!(kept[0].bbox.maxx, 100);
    }

    #[test]
    fn overlay_reports_hidden_count() {
        let tile = WorldBox {
            minx: 0,
            miny: 0,
            maxx: 1600,
            maxy: 1600,
        };
        let a = poly(1, 10, 10, 12, 12);
        let b = poly(1, 20, 20, 22, 22);
        let omitted = [&a, &b];
        let s = render_overlay(&omitted, &tile);
        assert!(s.contains("hidden"));
        assert!(s.contains("<rect"));
    }

    #[test]
    fn svgz_round_trips() {
        let svg = render_layer_tile(
            &[&poly(1, 0, 0, 10, 10)],
            &WorldBox {
                minx: 0,
                miny: 0,
                maxx: 100,
                maxy: 100,
            },
            "#fff",
        );
        let gz = svgz(&svg);
        assert!(
            gz.len() >= 2 && gz[0] == 0x1f && gz[1] == 0x8b,
            "gzip magic"
        );
        let mut d = GzDecoder::new(&gz[..]);
        let mut out = String::new();
        d.read_to_string(&mut out).unwrap();
        assert_eq!(out, svg);
    }
}
