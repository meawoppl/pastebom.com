//! Per-design persistence in localStorage, mirroring the PCB viewer's
//! `read_storage`/`write_storage`/`init_*` pattern under a `gds:{id}:` prefix.

use serde::{Deserialize, Serialize};

use crate::manifest::Manifest;
use crate::transform::Transform;

/// Editable, persisted state for one layer surface.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LayerState {
    /// Tile-URL key (`"{layer}_{datatype}"` or `"__lod"`).
    pub key: String,
    pub visible: bool,
    pub color: String,
    pub opacity: f64,
    /// User name override; falls back to the manifest/derived name when empty.
    #[serde(default)]
    pub name: String,
}

/// Whole-design persisted settings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ViewState {
    /// Layers in user z-order (front of the vec = bottom of the stack).
    pub layers: Vec<LayerState>,
    pub panx: f64,
    pub pany: f64,
    pub zoom: f64,
}

impl ViewState {
    /// Build default state from the manifest, ordered by `default_order` then key.
    pub fn from_manifest(m: &Manifest) -> Self {
        let mut layers: Vec<(i64, LayerState)> = m
            .layers
            .iter()
            .enumerate()
            .map(|(i, l)| {
                let order = l.default_order.unwrap_or(i as i64);
                (
                    order,
                    LayerState {
                        key: l.tile_key(),
                        visible: true,
                        color: l.default_color(),
                        opacity: 1.0,
                        name: String::new(),
                    },
                )
            })
            .collect();
        layers.sort_by_key(|(o, _)| *o);
        Self {
            layers: layers.into_iter().map(|(_, l)| l).collect(),
            panx: 0.0,
            pany: 0.0,
            zoom: 1.0,
        }
    }

    pub fn transform(&self) -> Transform {
        Transform {
            panx: self.panx,
            pany: self.pany,
            zoom: self.zoom,
        }
    }

    pub fn set_transform(&mut self, t: &Transform) {
        self.panx = t.panx;
        self.pany = t.pany;
        self.zoom = t.zoom;
    }
}

pub fn storage_prefix(id: &str) -> String {
    format!("gds:{}:", id)
}

pub fn read_storage(key: &str, prefix: &str) -> Option<String> {
    let window = web_sys::window()?;
    let storage = window.local_storage().ok()??;
    storage.get_item(&format!("{}{}", prefix, key)).ok()?
}

pub fn write_storage(key: &str, value: &str, prefix: &str) {
    if let Some(window) = web_sys::window() {
        if let Ok(Some(storage)) = window.local_storage() {
            let _ = storage.set_item(&format!("{}{}", prefix, key), value);
        }
    }
}

/// Load persisted state for a design, falling back to manifest defaults.
///
/// Persisted layers are merged against the manifest by key: any new layers in
/// the manifest are appended, and any persisted layers no longer present are
/// dropped, so a re-tile with different layers stays consistent.
pub fn init_view_state(m: &Manifest, prefix: &str) -> ViewState {
    let defaults = ViewState::from_manifest(m);
    let Some(raw) = read_storage("view", prefix) else {
        return defaults;
    };
    let Ok(stored) = serde_json::from_str::<ViewState>(&raw) else {
        return defaults;
    };

    let valid_keys: Vec<String> = defaults.layers.iter().map(|l| l.key.clone()).collect();
    let mut merged: Vec<LayerState> = stored
        .layers
        .into_iter()
        .filter(|l| valid_keys.contains(&l.key))
        .collect();
    for d in &defaults.layers {
        if !merged.iter().any(|l| l.key == d.key) {
            merged.push(d.clone());
        }
    }

    ViewState {
        layers: merged,
        panx: stored.panx,
        pany: stored.pany,
        zoom: if stored.zoom > 0.0 { stored.zoom } else { 1.0 },
    }
}

pub fn write_view_state(state: &ViewState, prefix: &str) {
    if let Ok(s) = serde_json::to_string(state) {
        write_storage("view", &s, prefix);
    }
}
