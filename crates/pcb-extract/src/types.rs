use serde::ser::{SerializeMap, Serializer};
use serde::Serialize;
use std::collections::HashMap;

/// Round a float to N decimal places.
pub fn round_f64(v: f64, places: u32) -> f64 {
    let factor = 10f64.powi(places as i32);
    (v * factor).round() / factor
}

/// Wrapper that rounds f64 to 6 decimal places on serialization.
fn serialize_f64_rounded<S: Serializer>(v: &f64, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_f64(round_f64(*v, 6))
}

fn serialize_point<S: Serializer>(p: &[f64; 2], s: S) -> Result<S::Ok, S::Error> {
    let rounded = [round_f64(p[0], 6), round_f64(p[1], 6)];
    rounded.serialize(s)
}

fn serialize_opt_f64_rounded<S: Serializer>(v: &Option<f64>, s: S) -> Result<S::Ok, S::Error> {
    match v {
        Some(val) => s.serialize_some(&round_f64(*val, 6)),
        None => s.serialize_none(),
    }
}

fn serialize_opt_point<S: Serializer>(p: &Option<[f64; 2]>, s: S) -> Result<S::Ok, S::Error> {
    match p {
        Some(pt) => {
            let rounded = [round_f64(pt[0], 6), round_f64(pt[1], 6)];
            s.serialize_some(&rounded)
        }
        None => s.serialize_none(),
    }
}

// ─── Top-level PcbData ───────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct PcbData {
    pub edges_bbox: BBox,
    pub edges: Vec<Drawing>,
    pub drawings: Drawings,
    pub footprints: Vec<Footprint>,
    pub metadata: Metadata,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bom: Option<BomData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ibom_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tracks: Option<LayerData<Vec<Track>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub zones: Option<LayerData<Vec<Zone>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nets: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub font_data: Option<FontData>,
}

// ─── Bounding Box ────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct BBox {
    #[serde(serialize_with = "serialize_f64_rounded")]
    pub minx: f64,
    #[serde(serialize_with = "serialize_f64_rounded")]
    pub miny: f64,
    #[serde(serialize_with = "serialize_f64_rounded")]
    pub maxx: f64,
    #[serde(serialize_with = "serialize_f64_rounded")]
    pub maxy: f64,
}

impl BBox {
    pub fn empty() -> Self {
        Self {
            minx: f64::INFINITY,
            miny: f64::INFINITY,
            maxx: f64::NEG_INFINITY,
            maxy: f64::NEG_INFINITY,
        }
    }

    pub fn expand_point(&mut self, x: f64, y: f64) {
        self.minx = self.minx.min(x);
        self.miny = self.miny.min(y);
        self.maxx = self.maxx.max(x);
        self.maxy = self.maxy.max(y);
    }
}

// ─── Drawings container ──────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct Drawings {
    pub silkscreen: LayerData<Vec<Drawing>>,
    pub fabrication: LayerData<Vec<Drawing>>,
}

/// Front/Back/Inner layer data.
#[derive(Debug, Clone, Serialize)]
pub struct LayerData<T> {
    #[serde(rename = "F")]
    pub front: T,
    #[serde(rename = "B")]
    pub back: T,
    #[serde(flatten, skip_serializing_if = "HashMap::is_empty")]
    pub inner: HashMap<String, T>,
}

// ─── Drawing types ───────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Drawing {
    Segment {
        #[serde(serialize_with = "serialize_point")]
        start: [f64; 2],
        #[serde(serialize_with = "serialize_point")]
        end: [f64; 2],
        #[serde(serialize_with = "serialize_f64_rounded")]
        width: f64,
    },
    Rect {
        #[serde(serialize_with = "serialize_point")]
        start: [f64; 2],
        #[serde(serialize_with = "serialize_point")]
        end: [f64; 2],
        #[serde(serialize_with = "serialize_f64_rounded")]
        width: f64,
    },
    Circle {
        #[serde(serialize_with = "serialize_point")]
        start: [f64; 2],
        #[serde(serialize_with = "serialize_f64_rounded")]
        radius: f64,
        #[serde(serialize_with = "serialize_f64_rounded")]
        width: f64,
        #[serde(skip_serializing_if = "Option::is_none")]
        filled: Option<u8>,
    },
    Arc {
        #[serde(serialize_with = "serialize_point")]
        start: [f64; 2],
        #[serde(serialize_with = "serialize_f64_rounded")]
        radius: f64,
        #[serde(serialize_with = "serialize_f64_rounded")]
        startangle: f64,
        #[serde(serialize_with = "serialize_f64_rounded")]
        endangle: f64,
        #[serde(serialize_with = "serialize_f64_rounded")]
        width: f64,
    },
    Curve {
        #[serde(serialize_with = "serialize_point")]
        start: [f64; 2],
        #[serde(serialize_with = "serialize_point")]
        end: [f64; 2],
        #[serde(serialize_with = "serialize_point")]
        cpa: [f64; 2],
        #[serde(serialize_with = "serialize_point")]
        cpb: [f64; 2],
        #[serde(serialize_with = "serialize_f64_rounded")]
        width: f64,
    },
    Polygon {
        #[serde(serialize_with = "serialize_point")]
        pos: [f64; 2],
        #[serde(serialize_with = "serialize_f64_rounded")]
        angle: f64,
        polygons: Vec<Vec<[f64; 2]>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        filled: Option<u8>,
        #[serde(serialize_with = "serialize_f64_rounded")]
        width: f64,
    },
}

/// Text drawing — not tagged with "type" since ibom outputs bare objects.
#[derive(Debug, Clone, Serialize)]
pub struct TextDrawing {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub svgpath: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thickness: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "ref")]
    pub is_ref: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub val: Option<u8>,
    // Stroke font fields
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pos: Option<[f64; 2]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub justify: Option<[i8; 2]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub angle: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attr: Option<Vec<String>>,
}

/// A drawing that can be either a shape or text (footprint drawings use this).
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum FootprintDrawingItem {
    Shape(Drawing),
    Text(TextDrawing),
}

#[derive(Debug, Clone, Serialize)]
pub struct FootprintDrawing {
    pub layer: String,
    pub drawing: FootprintDrawingItem,
}

// ─── Footprint ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct Footprint {
    #[serde(rename = "ref")]
    pub ref_: String,
    #[serde(serialize_with = "serialize_point")]
    pub center: [f64; 2],
    pub bbox: FootprintBBox,
    pub pads: Vec<Pad>,
    pub drawings: Vec<FootprintDrawing>,
    pub layer: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct FootprintBBox {
    #[serde(serialize_with = "serialize_point")]
    pub pos: [f64; 2],
    #[serde(serialize_with = "serialize_point")]
    pub relpos: [f64; 2],
    #[serde(serialize_with = "serialize_point")]
    pub size: [f64; 2],
    #[serde(serialize_with = "serialize_f64_rounded")]
    pub angle: f64,
}

// ─── Pad ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct Pad {
    pub layers: Vec<String>,
    #[serde(serialize_with = "serialize_point")]
    pub pos: [f64; 2],
    #[serde(serialize_with = "serialize_point")]
    pub size: [f64; 2],
    pub shape: String,
    #[serde(rename = "type")]
    pub pad_type: String,
    #[serde(
        serialize_with = "serialize_opt_f64_rounded",
        skip_serializing_if = "Option::is_none"
    )]
    pub angle: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pin1: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub net: Option<String>,
    #[serde(
        serialize_with = "serialize_opt_point",
        skip_serializing_if = "Option::is_none"
    )]
    pub offset: Option<[f64; 2]>,
    #[serde(
        serialize_with = "serialize_opt_f64_rounded",
        skip_serializing_if = "Option::is_none"
    )]
    pub radius: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chamfpos: Option<u8>,
    #[serde(
        serialize_with = "serialize_opt_f64_rounded",
        skip_serializing_if = "Option::is_none"
    )]
    pub chamfratio: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub drillshape: Option<String>,
    #[serde(
        serialize_with = "serialize_opt_point",
        skip_serializing_if = "Option::is_none"
    )]
    pub drillsize: Option<[f64; 2]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub svgpath: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub polygons: Option<Vec<Vec<[f64; 2]>>>,
}

// ─── Track ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum Track {
    Segment {
        #[serde(serialize_with = "serialize_point")]
        start: [f64; 2],
        #[serde(serialize_with = "serialize_point")]
        end: [f64; 2],
        #[serde(serialize_with = "serialize_f64_rounded")]
        width: f64,
        #[serde(skip_serializing_if = "Option::is_none")]
        net: Option<String>,
        #[serde(
            serialize_with = "serialize_opt_f64_rounded",
            skip_serializing_if = "Option::is_none"
        )]
        drillsize: Option<f64>,
    },
    Arc {
        #[serde(serialize_with = "serialize_point")]
        center: [f64; 2],
        #[serde(serialize_with = "serialize_f64_rounded")]
        startangle: f64,
        #[serde(serialize_with = "serialize_f64_rounded")]
        endangle: f64,
        #[serde(serialize_with = "serialize_f64_rounded")]
        radius: f64,
        #[serde(serialize_with = "serialize_f64_rounded")]
        width: f64,
        #[serde(skip_serializing_if = "Option::is_none")]
        net: Option<String>,
    },
}

// ─── Zone ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct Zone {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub polygons: Option<Vec<Vec<[f64; 2]>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub svgpath: Option<String>,
    #[serde(
        serialize_with = "serialize_opt_f64_rounded",
        skip_serializing_if = "Option::is_none"
    )]
    pub width: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub net: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fillrule: Option<String>,
}

// ─── Font data ───────────────────────────────────────────────────────

pub type FontData = HashMap<String, GlyphData>;

#[derive(Debug, Clone, Serialize)]
pub struct GlyphData {
    pub w: f64,
    pub l: Vec<Vec<[f64; 2]>>,
}

// ─── Metadata ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct Metadata {
    pub title: String,
    pub revision: String,
    pub company: String,
    pub date: String,
}

// ─── BOM data ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct BomData {
    pub both: Vec<Vec<(String, usize)>>,
    #[serde(rename = "F")]
    pub front: Vec<Vec<(String, usize)>>,
    #[serde(rename = "B")]
    pub back: Vec<Vec<(String, usize)>>,
    pub skipped: Vec<usize>,
    pub fields: BomFields,
}

/// Map of footprint index (as string) to field values.
#[derive(Debug, Clone)]
pub struct BomFields(pub HashMap<String, Vec<String>>);

impl Serialize for BomFields {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        let mut keys: Vec<_> = self.0.keys().collect();
        keys.sort_by_key(|k| k.parse::<usize>().unwrap_or(0));
        for key in keys {
            map.serialize_entry(key, &self.0[key])?;
        }
        map.end()
    }
}

// ─── Side helper ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Front,
    Back,
}

impl Side {
    pub fn as_str(&self) -> &'static str {
        match self {
            Side::Front => "F",
            Side::Back => "B",
        }
    }
}

// ─── Component (used internally for BOM generation) ──────────────────

#[derive(Debug, Clone)]
pub struct Component {
    pub ref_: String,
    pub val: String,
    pub footprint_name: String,
    pub layer: Side,
    pub footprint_index: usize,
    pub extra_fields: HashMap<String, String>,
    pub attr: Option<String>,
}
