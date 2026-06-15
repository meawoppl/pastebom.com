//! GDSII tile pipeline — stage 4: tile-set driver + manifest.
//!
//! Ties the stream (stage 1), the BSP index (stage 2), and the renderer
//! (stage 3) together: given a `.gds` byte buffer it produces a [`Manifest`],
//! the serialized index, and the eager (zoomed-out) tile blobs. The server
//! persists these and serves them; the WASM viewer reads the manifest first,
//! then fetches tiles on demand.

use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};

use crate::error::ExtractError;

use super::bsp::BspIndex;
use super::tile::{stream_records, InstancedArray, PlacedRecord, WorldBox};
use super::tiler::{
    lod_partition, render_layer_tile, render_overlay, svgz, Pyramid, LOD_MIN_PX, TILE_PX,
};

/// Finest resolution the deepest pyramid level targets, in nm/px.
const RES_MIN_NM_PER_PX: f64 = 1.0;

// ─── Manifest (the viewer/tiler contract; served as JSON) ────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub id: String,
    pub units: Units,
    pub extent_nm: Extent,
    pub tile_px: u32,
    pub zoom: Zoom,
    pub lod_min_px: f64,
    pub layers: Vec<LayerInfo>,
    pub generated: Generated,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Units {
    pub nm_per_world: f64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Extent {
    pub minx: i64,
    pub miny: i64,
    pub maxx: i64,
    pub maxy: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Zoom {
    pub min: u32,
    pub max: u32,
    pub res_nm_per_px: Vec<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LayerInfo {
    /// `"{layer}/{datatype}"`, or `"__lod"` for the synthetic overlay layer.
    pub key: String,
    pub layer: i16,
    pub datatype: i16,
    pub name: String,
    pub count: u64,
    pub bbox_nm: Extent,
    pub default_color: String,
    pub default_order: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Generated {
    pub mode: String,
    pub eager_max_z: u32,
}

/// The synthetic overlay layer key (its tiles carry "(n hidden)" annotations).
pub const LOD_KEY: &str = "__lod";

// ─── Driver output ───────────────────────────────────────────────────────────

/// Everything to persist for one ingested GDSII file.
pub struct TileSet {
    pub manifest: Manifest,
    /// Serialized [`TileIndex`] (cache so re-tiling needn't re-parse).
    pub index_bytes: Vec<u8>,
    /// Eager tile blobs: `(path under tiles/, gzipped-SVG body)`.
    /// Path is `"{z}/{x}/{y}/{key}.svgz"`.
    pub tiles: Vec<(String, Vec<u8>)>,
}

/// The cached spatial index for a GDSII view: the BSP over flat records plus
/// the unexpanded large arrays. Serialized to `gdsii/{id}/index.bin` so tiles
/// (including deep-zoom, on demand) can be rendered without re-parsing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TileIndex {
    pub bsp: BspIndex,
    pub arrays: Vec<InstancedArray>,
}

impl TileIndex {
    pub fn to_bytes(&self) -> Result<Vec<u8>, postcard::Error> {
        postcard::to_allocvec(self)
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, postcard::Error> {
        postcard::from_bytes(bytes)
    }
}

fn ext(b: &WorldBox) -> Extent {
    Extent {
        minx: b.minx,
        miny: b.miny,
        maxx: b.maxx,
        maxy: b.maxy,
    }
}

/// Deterministic per-layer color (no GDSII layer-map): hash the key into a
/// small palette so the same layer always gets the same hue.
fn palette_color(layer: i16, datatype: i16) -> String {
    const PALETTE: [&str; 12] = [
        "#7aa2f7", "#9ece6a", "#e0af68", "#bb9af7", "#7dcfff", "#f7768e", "#2ac3de", "#ff9e64",
        "#73daca", "#b4f9f8", "#c0caf5", "#ff007c",
    ];
    let h = (layer as i64)
        .wrapping_mul(31)
        .wrapping_add(datatype as i64);
    let idx = h.rem_euclid(PALETTE.len() as i64) as usize;
    PALETTE[idx].to_string()
}

/// Build the tile set for a GDSII byte buffer. `eager_max_z` bounds how many
/// zoomed-out levels are pre-rendered; deeper levels are generated on demand.
pub fn build_tileset(id: &str, data: &[u8], eager_max_z: u32) -> Result<TileSet, ExtractError> {
    let stream = stream_records(data)?;
    let records = stream.records;
    let arrays = stream.arrays;

    // Group by (layer, datatype) for per-layer counts and extents — counting
    // both flat records and the (unmaterialized) instances of large arrays.
    let mut groups: BTreeMap<(i16, i16), (u64, WorldBox)> = BTreeMap::new();
    let mut bounds = WorldBox::empty();
    for r in &records {
        bounds.union(&r.bbox);
        let e = groups
            .entry((r.layer, r.datatype))
            .or_insert((0, WorldBox::empty()));
        e.0 += 1;
        e.1.union(&r.bbox);
    }
    for arr in &arrays {
        bounds.union(&arr.bbox);
        let instances = arr.cols as u64 * arr.rows as u64;
        for cr in &arr.child {
            let e = groups
                .entry((cr.layer, cr.datatype))
                .or_insert((0, WorldBox::empty()));
            e.0 += instances;
            e.1.union(&arr.bbox);
        }
    }
    if bounds.is_empty() {
        bounds = WorldBox {
            minx: 0,
            miny: 0,
            maxx: 1,
            maxy: 1,
        };
    }

    let pyramid = Pyramid::new(bounds, RES_MIN_NM_PER_PX);
    let max_z = pyramid.levels.saturating_sub(1);
    let res_nm_per_px: Vec<f64> = (0..pyramid.levels).map(|z| pyramid.res(z)).collect();

    let index = TileIndex {
        bsp: BspIndex::build(records),
        arrays,
    };
    let index_bytes = index
        .to_bytes()
        .map_err(|e| ExtractError::ParseError(format!("GDSII tile index serialize: {e}")))?;

    // Manifest layers: one per (layer, datatype) plus the synthetic LOD overlay.
    let mut layers = Vec::new();
    for (order, (&(layer, datatype), &(count, bbox))) in groups.iter().enumerate() {
        layers.push(LayerInfo {
            key: format!("{layer}/{datatype}"),
            layer,
            datatype,
            name: format!("L{layer}/{datatype}"),
            count,
            bbox_nm: ext(&bbox),
            default_color: palette_color(layer, datatype),
            default_order: order as u32,
        });
    }
    layers.push(LayerInfo {
        key: LOD_KEY.to_string(),
        layer: 0,
        datatype: 0,
        name: "omitted (LOD)".to_string(),
        count: 0,
        bbox_nm: ext(&bounds),
        default_color: "#f7768e".to_string(),
        default_order: layers.len() as u32,
    });

    // Eager tiles for the zoomed-out levels.
    let eager = eager_max_z.min(max_z);
    let mut tiles = Vec::new();
    for z in 0..=eager {
        let n = pyramid.tiles_per_axis(z);
        for ty in 0..n {
            for tx in 0..n {
                tiles.extend(render_tile_inner(&pyramid, &bounds, &index, z, tx, ty));
            }
        }
    }

    let manifest = Manifest {
        id: id.to_string(),
        units: Units { nm_per_world: 1.0 },
        extent_nm: ext(&bounds),
        tile_px: TILE_PX,
        zoom: Zoom {
            min: 0,
            max: max_z,
            res_nm_per_px,
        },
        lod_min_px: LOD_MIN_PX,
        layers,
        generated: Generated {
            mode: "eager".to_string(),
            eager_max_z: eager,
        },
    };

    Ok(TileSet {
        manifest,
        index_bytes,
        tiles,
    })
}

/// Render every layer's blob for a single tile `(z, x, y)` on demand, given the
/// design `bounds` (from the manifest) and the cached index. Returns
/// `(path, svgz)` pairs (`{z}/{x}/{y}/{key}.svgz`); empty if the tile holds
/// nothing. The pyramid is reconstructed deterministically from `bounds`, so
/// these match what [`build_tileset`] would have produced eagerly.
pub fn render_tile(
    bounds: WorldBox,
    index: &TileIndex,
    z: u32,
    x: u32,
    y: u32,
) -> Vec<(String, Vec<u8>)> {
    let pyramid = Pyramid::new(bounds, RES_MIN_NM_PER_PX);
    render_tile_inner(&pyramid, &bounds, index, z, x, y)
}

fn render_tile_inner(
    pyramid: &Pyramid,
    bounds: &WorldBox,
    index: &TileIndex,
    z: u32,
    tx: u32,
    ty: u32,
) -> Vec<(String, Vec<u8>)> {
    let tbox = pyramid.tile_world_box(z, tx, ty);
    let mut out = Vec::new();
    if !tbox.intersects(bounds) {
        return out;
    }

    // Candidate geometry for this tile: flat records from the BSP (cloned) plus
    // the instances of any large array whose extent overlaps the tile.
    let mut by_layer: HashMap<(i16, i16), Vec<PlacedRecord>> = HashMap::new();
    for r in index.bsp.query_records(&tbox) {
        by_layer
            .entry((r.layer, r.datatype))
            .or_default()
            .push(r.clone());
    }
    for arr in &index.arrays {
        if !arr.bbox.intersects(&tbox) {
            continue;
        }
        for rec in arr.expand_for_tile(&tbox) {
            by_layer
                .entry((rec.layer, rec.datatype))
                .or_default()
                .push(rec);
        }
    }
    if by_layer.is_empty() {
        return out;
    }

    let res_z = pyramid.res(z);
    let mut keys: Vec<(i16, i16)> = by_layer.keys().copied().collect();
    keys.sort_unstable();
    let mut all_omitted: Vec<&PlacedRecord> = Vec::new();
    for key in &keys {
        let refs: Vec<&PlacedRecord> = by_layer[key].iter().collect();
        let (kept, omitted) = lod_partition(&refs, res_z);
        all_omitted.extend(omitted);
        if kept.is_empty() {
            continue;
        }
        let color = palette_color(key.0, key.1);
        let svg = render_layer_tile(&kept, &tbox, &color);
        out.push((
            format!("{z}/{tx}/{ty}/{}_{}.svgz", key.0, key.1),
            svgz(&svg),
        ));
    }
    if !all_omitted.is_empty() {
        let svg = render_overlay(&all_omitted, &tbox);
        out.push((format!("{z}/{tx}/{ty}/{LOD_KEY}.svgz"), svgz(&svg)));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    // Minimal GDSII byte-stream builder (mirrors the tile-module tests).
    fn frame(rt: u8, dt: u8, payload: &[u8]) -> Vec<u8> {
        let len = (4 + payload.len()) as u16;
        let mut v = Vec::new();
        v.write_all(&len.to_be_bytes()).unwrap();
        v.push(rt);
        v.push(dt);
        v.extend_from_slice(payload);
        v
    }
    fn no_data(rt: u8) -> Vec<u8> {
        frame(rt, 0x00, &[])
    }
    fn i16r(rt: u8, vals: &[i16]) -> Vec<u8> {
        let mut p = Vec::new();
        for &x in vals {
            p.extend_from_slice(&x.to_be_bytes());
        }
        frame(rt, 0x02, &p)
    }
    fn i32r(rt: u8, vals: &[i32]) -> Vec<u8> {
        let mut p = Vec::new();
        for &x in vals {
            p.extend_from_slice(&x.to_be_bytes());
        }
        frame(rt, 0x03, &p)
    }
    fn f64r(rt: u8, vals: &[f64]) -> Vec<u8> {
        let mut p = Vec::new();
        for &x in vals {
            p.extend_from_slice(&f64_to_gds(x));
        }
        frame(rt, 0x05, &p)
    }
    fn ascii(rt: u8, s: &str) -> Vec<u8> {
        let mut p = s.as_bytes().to_vec();
        if p.len() % 2 == 1 {
            p.push(0);
        }
        frame(rt, 0x06, &p)
    }
    fn f64_to_gds(value: f64) -> [u8; 8] {
        if value == 0.0 {
            return [0u8; 8];
        }
        let sign = if value < 0.0 { 1u8 } else { 0u8 };
        let mut v = value.abs();
        let mut exponent = 64i32;
        while v >= 1.0 {
            v /= 16.0;
            exponent += 1;
        }
        while v < 1.0 / 16.0 {
            v *= 16.0;
            exponent -= 1;
        }
        let mantissa = (v * (1u64 << 56) as f64) as u64;
        let mut b = [0u8; 8];
        b[0] = (sign << 7) | (exponent as u8 & 0x7F);
        for (i, byte) in b.iter_mut().enumerate().skip(1) {
            *byte = ((mantissa >> (8 * (7 - i))) & 0xFF) as u8;
        }
        b
    }

    // record types
    const HEADER: u8 = 0x00;
    const BGNLIB: u8 = 0x01;
    const LIBNAME: u8 = 0x02;
    const UNITS: u8 = 0x03;
    const ENDLIB: u8 = 0x04;
    const BGNSTR: u8 = 0x05;
    const STRNAME: u8 = 0x06;
    const ENDSTR: u8 = 0x07;
    const BOUNDARY: u8 = 0x08;
    const LAYER: u8 = 0x0D;
    const DATATYPE: u8 = 0x0E;
    const XY: u8 = 0x10;
    const ENDEL: u8 = 0x11;

    fn square(layer: i16, dt: i16, x0: i32, y0: i32, side: i32) -> Vec<u8> {
        let mut g = Vec::new();
        g.extend(no_data(BOUNDARY));
        g.extend(i16r(LAYER, &[layer]));
        g.extend(i16r(DATATYPE, &[dt]));
        g.extend(i32r(
            XY,
            &[
                x0,
                y0,
                x0 + side,
                y0,
                x0 + side,
                y0 + side,
                x0,
                y0 + side,
                x0,
                y0,
            ],
        ));
        g.extend(no_data(ENDEL));
        g
    }

    /// One cell, two layers (1/0 and 2/0), each a 1000nm square offset apart.
    fn two_layer_file() -> Vec<u8> {
        let mut g = Vec::new();
        g.extend(i16r(HEADER, &[600]));
        g.extend(i16r(BGNLIB, &[0; 12]));
        g.extend(ascii(LIBNAME, "TOP"));
        g.extend(f64r(UNITS, &[1e-3, 1e-9])); // 1 nm/DBU
        g.extend(i16r(BGNSTR, &[0; 12]));
        g.extend(ascii(STRNAME, "TOP"));
        g.extend(square(1, 0, 0, 0, 1000));
        g.extend(square(2, 0, 2000, 2000, 1000));
        g.extend(no_data(ENDSTR));
        g.extend(no_data(ENDLIB));
        g
    }

    #[test]
    fn manifest_has_layers_and_lod() {
        let ts = build_tileset("abc", &two_layer_file(), 4).unwrap();
        let keys: Vec<&str> = ts.manifest.layers.iter().map(|l| l.key.as_str()).collect();
        assert!(keys.contains(&"1/0"));
        assert!(keys.contains(&"2/0"));
        assert!(keys.contains(&"__lod"));
        assert_eq!(ts.manifest.id, "abc");
        assert_eq!(ts.manifest.tile_px, TILE_PX);
        // extent covers both squares: x 0..3000, y 0..3000
        assert_eq!(ts.manifest.extent_nm.minx, 0);
        assert_eq!(ts.manifest.extent_nm.maxx, 3000);
        // one res entry per level
        assert_eq!(
            ts.manifest.zoom.res_nm_per_px.len() as u32,
            ts.manifest.zoom.max + 1
        );
    }

    #[test]
    fn emits_level0_tiles_and_serializes_round_trip() {
        let ts = build_tileset("abc", &two_layer_file(), 0).unwrap();
        // At least one level-0 tile per real layer.
        assert!(ts.tiles.iter().any(|(p, _)| p.starts_with("0/0/0/1_0")));
        assert!(ts.tiles.iter().any(|(p, _)| p.starts_with("0/0/0/2_0")));
        // tiles are gzip
        for (_, body) in &ts.tiles {
            assert!(body.len() >= 2 && body[0] == 0x1f && body[1] == 0x8b);
        }
        // index round-trips and answers a query over the full extent.
        let idx = TileIndex::from_bytes(&ts.index_bytes).unwrap();
        let all = idx.bsp.query(&WorldBox {
            minx: 0,
            miny: 0,
            maxx: 3000,
            maxy: 3000,
        });
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn manifest_serializes_to_expected_json_shape() {
        let ts = build_tileset("abc", &two_layer_file(), 0).unwrap();
        let json = serde_json::to_string(&ts.manifest).unwrap();
        for field in [
            "\"id\"",
            "\"units\"",
            "nm_per_world",
            "extent_nm",
            "tile_px",
            "res_nm_per_px",
            "lod_min_px",
            "default_color",
            "default_order",
            "eager_max_z",
        ] {
            assert!(json.contains(field), "manifest JSON missing {field}");
        }
    }

    #[test]
    fn empty_design_does_not_panic() {
        let mut g = Vec::new();
        g.extend(i16r(HEADER, &[600]));
        g.extend(i16r(BGNLIB, &[0; 12]));
        g.extend(ascii(LIBNAME, "T"));
        g.extend(f64r(UNITS, &[1e-3, 1e-9]));
        g.extend(i16r(BGNSTR, &[0; 12]));
        g.extend(ascii(STRNAME, "T"));
        g.extend(no_data(ENDSTR));
        g.extend(no_data(ENDLIB));
        let ts = build_tileset("e", &g, 4).unwrap();
        // only the synthetic __lod layer
        assert_eq!(ts.manifest.layers.len(), 1);
        assert!(ts.tiles.is_empty());
    }
}
