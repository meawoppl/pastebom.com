//! Manifest contract consumed from `GET /g/{id}/manifest.json`.
//!
//! The server side is built in parallel against the same spec, so every field
//! is optional-tolerant: missing pieces fall back to sane defaults rather than
//! failing the whole fetch.

use serde::Deserialize;

/// Synthetic layer key for the level-of-detail overlay.
pub const LOD_KEY: &str = "__lod";

#[derive(Clone, Debug, Deserialize)]
pub struct Manifest {
    pub id: String,
    // Parsed for contract completeness; tile geometry is already world-space.
    #[serde(default)]
    #[allow(dead_code)]
    pub units: Units,
    pub extent_nm: Extent,
    #[serde(default = "default_tile_px")]
    pub tile_px: u32,
    pub zoom: Zoom,
    // Retained from the manifest contract; the LOD overlay is a normal toggleable
    // layer in the viewer, so this server-side threshold is informational here.
    #[serde(default = "default_lod_min_px")]
    #[allow(dead_code)]
    pub lod_min_px: f64,
    #[serde(default)]
    pub layers: Vec<Layer>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Units {
    // World unit = nm by default; tile geometry is already in world units, so
    // this scale is retained for completeness rather than applied client-side.
    #[serde(default = "default_nm_per_world")]
    #[allow(dead_code)]
    pub nm_per_world: f64,
}

impl Default for Units {
    fn default() -> Self {
        Self {
            nm_per_world: default_nm_per_world(),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize)]
pub struct Extent {
    pub minx: f64,
    pub miny: f64,
    pub maxx: f64,
    pub maxy: f64,
}

impl Extent {
    pub fn width(&self) -> f64 {
        (self.maxx - self.minx).max(1.0)
    }

    pub fn height(&self) -> f64 {
        (self.maxy - self.miny).max(1.0)
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct Zoom {
    pub min: u32,
    pub max: u32,
    #[serde(default)]
    pub res_nm_per_px: Vec<f64>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Layer {
    pub key: String,
    #[serde(default)]
    pub layer: i64,
    #[serde(default)]
    pub datatype: i64,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub count: u64,
    // Per-layer extent from the contract; reserved for a future "zoom to layer".
    #[serde(default)]
    #[allow(dead_code)]
    pub bbox_nm: Option<Extent>,
    #[serde(default)]
    pub default_color: Option<String>,
    #[serde(default)]
    pub default_order: Option<i64>,
}

impl Layer {
    /// True for the synthetic level-of-detail overlay layer.
    pub fn is_lod(&self) -> bool {
        self.key == LOD_KEY
    }

    /// Tile-URL key: `"{layer}_{datatype}"` for real layers, `"__lod"` for the
    /// overlay. The manifest's `key` uses a slash (`"10/0"`) for display.
    pub fn tile_key(&self) -> String {
        if self.is_lod() {
            LOD_KEY.to_string()
        } else {
            format!("{}_{}", self.layer, self.datatype)
        }
    }

    /// Display name: manifest name if present, else `L{layer}/{datatype}`.
    pub fn display_name(&self) -> String {
        if let Some(ref n) = self.name {
            if !n.is_empty() {
                return n.clone();
            }
        }
        if self.is_lod() {
            "omitted (LOD)".to_string()
        } else {
            format!("L{}/{}", self.layer, self.datatype)
        }
    }

    /// Color: manifest default if present, else a deterministic palette colour
    /// derived from the key's hash.
    pub fn default_color(&self) -> String {
        if let Some(ref c) = self.default_color {
            if !c.is_empty() {
                return c.clone();
            }
        }
        palette_color(&self.key)
    }
}

fn default_tile_px() -> u32 {
    512
}

fn default_lod_min_px() -> f64 {
    2.0
}

fn default_nm_per_world() -> f64 {
    1.0
}

/// Deterministic palette: hash the layer key into a hue, fixed S/L.
pub fn palette_color(key: &str) -> String {
    // FNV-1a over the key bytes gives a stable, well-spread hash.
    let mut h: u64 = 0xcbf29ce484222325;
    for b in key.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    let hue = (h % 360) as f64;
    hsl_to_hex(hue, 0.62, 0.62)
}

fn hsl_to_hex(h: f64, s: f64, l: f64) -> String {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let hp = h / 60.0;
    let x = c * (1.0 - (hp % 2.0 - 1.0).abs());
    let (r1, g1, b1) = match hp as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c / 2.0;
    let to_byte = |v: f64| ((v + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    format!("#{:02x}{:02x}{:02x}", to_byte(r1), to_byte(g1), to_byte(b1))
}
