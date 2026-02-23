use serde::Deserialize;
use std::collections::HashMap;

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct PcbData {
    pub edges_bbox: BBox,
    pub edges: Vec<Drawing>,
    pub drawings: Drawings,
    pub footprints: Vec<Footprint>,
    pub metadata: Metadata,
    #[serde(default)]
    pub bom: Option<BomData>,
    #[serde(default)]
    pub ibom_version: Option<String>,
    #[serde(default)]
    pub tracks: Option<LayerData<Vec<Track>>>,
    #[serde(default)]
    pub copper_pads: Option<LayerData<Vec<Drawing>>>,
    #[serde(default)]
    pub zones: Option<LayerData<Vec<Zone>>>,
    #[serde(default)]
    pub nets: Option<Vec<String>>,
    #[serde(default)]
    pub font_data: Option<FontData>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BBox {
    pub minx: f64,
    pub miny: f64,
    pub maxx: f64,
    pub maxy: f64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Drawings {
    pub silkscreen: LayerData<Vec<Drawing>>,
    pub fabrication: LayerData<Vec<Drawing>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LayerData<T> {
    #[serde(rename = "F")]
    pub front: T,
    #[serde(rename = "B")]
    pub back: T,
    #[serde(flatten, default)]
    pub inner: HashMap<String, T>,
}

impl<T> LayerData<T> {
    pub fn get(&self, layer: &str) -> Option<&T> {
        match layer {
            "F" => Some(&self.front),
            "B" => Some(&self.back),
            name => self.inner.get(name),
        }
    }

    pub fn inner_layer_names(&self) -> Vec<&String> {
        let mut names: Vec<&String> = self.inner.keys().collect();
        names.sort();
        names
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Drawing {
    Segment {
        start: [f64; 2],
        end: [f64; 2],
        width: f64,
    },
    Rect {
        start: [f64; 2],
        end: [f64; 2],
        width: f64,
    },
    Circle {
        start: [f64; 2],
        radius: f64,
        width: f64,
        #[serde(default)]
        filled: Option<u8>,
    },
    Arc {
        start: [f64; 2],
        radius: f64,
        startangle: f64,
        endangle: f64,
        width: f64,
    },
    Curve {
        start: [f64; 2],
        end: [f64; 2],
        cpa: [f64; 2],
        cpb: [f64; 2],
        width: f64,
    },
    Polygon {
        pos: [f64; 2],
        angle: f64,
        polygons: Vec<Vec<[f64; 2]>>,
        #[serde(default)]
        filled: Option<u8>,
        width: f64,
    },
}

#[derive(Debug, Clone, Deserialize)]
pub struct TextDrawing {
    #[serde(default)]
    pub svgpath: Option<String>,
    #[serde(default)]
    pub thickness: Option<f64>,
    #[serde(default, rename = "ref")]
    pub is_ref: Option<u8>,
    #[serde(default)]
    pub val: Option<u8>,
    #[serde(default)]
    pub fillrule: Option<String>,
    #[serde(default)]
    pub pos: Option<[f64; 2]>,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub height: Option<f64>,
    #[serde(default)]
    pub width: Option<f64>,
    #[serde(default)]
    pub justify: Option<[f64; 2]>,
    #[serde(default)]
    pub angle: Option<f64>,
    #[serde(default)]
    pub attr: Option<Vec<String>>,
    #[serde(default)]
    pub polygons: Option<Vec<Vec<[f64; 2]>>>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum FootprintDrawingItem {
    Shape(Drawing),
    Text(TextDrawing),
}

#[derive(Debug, Clone, Deserialize)]
pub struct FootprintDrawing {
    pub layer: String,
    pub drawing: FootprintDrawingItem,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct Footprint {
    #[serde(rename = "ref")]
    pub ref_: String,
    pub center: [f64; 2],
    pub bbox: FootprintBBox,
    pub pads: Vec<Pad>,
    pub drawings: Vec<FootprintDrawing>,
    pub layer: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FootprintBBox {
    pub pos: [f64; 2],
    pub relpos: [f64; 2],
    pub size: [f64; 2],
    pub angle: f64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Pad {
    pub layers: Vec<String>,
    pub pos: [f64; 2],
    pub size: [f64; 2],
    pub shape: String,
    #[serde(rename = "type")]
    pub pad_type: String,
    #[serde(default)]
    pub angle: Option<f64>,
    #[serde(default)]
    pub pin1: Option<u8>,
    #[serde(default)]
    pub net: Option<String>,
    #[serde(default)]
    pub offset: Option<[f64; 2]>,
    #[serde(default)]
    pub radius: Option<f64>,
    #[serde(default)]
    pub chamfpos: Option<u8>,
    #[serde(default)]
    pub chamfratio: Option<f64>,
    #[serde(default)]
    pub drillshape: Option<String>,
    #[serde(default)]
    pub drillsize: Option<[f64; 2]>,
    #[serde(default)]
    pub svgpath: Option<String>,
    #[serde(default)]
    pub polygons: Option<Vec<Vec<[f64; 2]>>>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum Track {
    Arc {
        center: [f64; 2],
        startangle: f64,
        endangle: f64,
        radius: f64,
        width: f64,
        #[serde(default)]
        net: Option<String>,
    },
    Segment {
        start: [f64; 2],
        end: [f64; 2],
        width: f64,
        #[serde(default)]
        net: Option<String>,
        #[serde(default)]
        drillsize: Option<f64>,
    },
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct Zone {
    #[serde(default)]
    pub polygons: Option<Vec<Vec<[f64; 2]>>>,
    #[serde(default)]
    pub svgpath: Option<String>,
    #[serde(default)]
    pub width: Option<f64>,
    #[serde(default)]
    pub net: Option<String>,
    #[serde(default)]
    pub fillrule: Option<String>,
}

pub type FontData = HashMap<String, GlyphData>;

#[derive(Debug, Clone, Deserialize)]
pub struct GlyphData {
    pub w: f64,
    pub l: Vec<Vec<[f64; 2]>>,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct Metadata {
    pub title: String,
    pub revision: String,
    pub company: String,
    pub date: String,
}

/// BOM ref entry: (reference_designator, footprint_index)
pub type BomRef = (String, usize);
/// BOM group: a list of refs that share the same value+footprint
pub type BomGroup = Vec<BomRef>;

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct BomData {
    pub both: Vec<BomGroup>,
    #[serde(rename = "F")]
    pub front: Vec<BomGroup>,
    #[serde(rename = "B")]
    pub back: Vec<BomGroup>,
    pub skipped: Vec<usize>,
    pub fields: HashMap<String, Vec<serde_json::Value>>,
}
