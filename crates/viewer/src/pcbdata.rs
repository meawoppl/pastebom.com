//! The parts of the board JSON the BOM/UI side needs. Board geometry is read
//! separately into `pcb_extract::types::PcbData` and drawn through
//! vector-view (see `render.rs`); geometry fields are skipped here.

use serde::de::IgnoredAny;
use serde::Deserialize;
use std::collections::HashMap;

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct PcbData {
    pub metadata: Metadata,
    #[serde(default)]
    pub bom: Option<BomData>,
    #[serde(default)]
    pub ibom_version: Option<String>,
    #[serde(default)]
    pub tracks: Option<LayerKeys>,
    #[serde(default)]
    pub zones: Option<LayerKeys>,
    #[serde(default)]
    pub nets: Option<Vec<String>>,
}

/// Which copper layers a per-layer map has, without parsing the contents.
#[derive(Debug, Clone, Deserialize)]
pub struct LayerKeys(HashMap<String, IgnoredAny>);

impl LayerKeys {
    /// Inner layer names (everything but "F" and "B"), sorted.
    pub fn inner_layer_names(&self) -> Vec<&String> {
        let mut names: Vec<&String> = self.0.keys().filter(|k| *k != "F" && *k != "B").collect();
        names.sort();
        names
    }
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
