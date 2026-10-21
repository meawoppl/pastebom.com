use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq)]
pub enum LayerCategory {
    CopperF,
    CopperB,
    CopperInner,
    SilkF,
    SilkB,
    FabF,
    FabB,
    Other,
}

pub struct LayerMap {
    /// Maps V6 layer ID -> category
    categories: HashMap<u8, LayerCategory>,
    /// Mechanical layer mechkind mappings from Board6
    mech_kinds: HashMap<u8, String>,
}

impl LayerMap {
    pub fn side(&self, layer_id: u8) -> &'static str {
        match self.category(layer_id) {
            LayerCategory::CopperF | LayerCategory::SilkF | LayerCategory::FabF => "F",
            LayerCategory::CopperB | LayerCategory::SilkB | LayerCategory::FabB => "B",
            _ => "F",
        }
    }

    /// Return a layer name string for inner copper layers (IDs 2-30).
    /// Uses KiCad-compatible naming: "In1.Cu", "In2.Cu", etc.
    pub fn inner_layer_name(&self, layer_id: u8) -> String {
        format!("In{}.Cu", layer_id - 1)
    }

    pub fn category(&self, layer_id: u8) -> LayerCategory {
        if let Some(cat) = self.categories.get(&layer_id) {
            return cat.clone();
        }
        // V6 standard layer mapping
        match layer_id {
            1 => LayerCategory::CopperF,
            2..=30 => LayerCategory::CopperInner,
            32 => LayerCategory::CopperB,
            33 => LayerCategory::SilkF,
            34 => LayerCategory::SilkB,
            74 => LayerCategory::CopperF, // Multi-layer, treat as front
            57..=72 => {
                // Mechanical layers - check mechkind
                if let Some(kind) = self.mech_kinds.get(&layer_id) {
                    match kind.to_uppercase().as_str() {
                        "ASSEMBLY_TOP" | "COURTYARD_TOP" => LayerCategory::FabF,
                        "ASSEMBLY_BOTTOM" | "COURTYARD_BOTTOM" => LayerCategory::FabB,
                        _ => LayerCategory::Other,
                    }
                } else {
                    LayerCategory::Other
                }
            }
            _ => LayerCategory::Other,
        }
    }
}

pub fn build_layer_map(board_records: &[HashMap<String, String>]) -> LayerMap {
    let mut mech_kinds = HashMap::new();

    // Parse mechkind from board records
    if let Some(board) = board_records.first() {
        for i in 1..=32 {
            let key = format!("LAYERV7_{}MECHKIND", i);
            if let Some(kind) = board.get(&key) {
                // Mechanical layers start at V6 ID 57
                let layer_id = 56 + i as u8;
                mech_kinds.insert(layer_id, kind.clone());
            }
        }
    }

    LayerMap {
        categories: HashMap::new(),
        mech_kinds,
    }
}
