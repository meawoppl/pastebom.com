use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub canvas_layout: String, // "F", "FB", "B"
    pub bom_layout: String,    // "bom-only", "left-right", "top-bottom"
    pub bom_mode: String,      // "grouped", "ungrouped", "netlist"
    pub dark_mode: bool,
    pub highlight_pin1: String, // "none", "all", "selected"
    pub redraw_on_drag: bool,
    pub board_rotation: f64,
    pub offset_back_rotation: bool,
    pub render_pads: bool,
    pub render_references: bool,
    pub render_values: bool,
    pub render_silkscreen: bool,
    pub render_fabrication: bool,
    pub render_tracks: bool,
    pub render_zones: bool,
    pub render_dnp_outline: bool,
    pub checkboxes: Vec<String>,
    pub checkbox_stored_refs: HashMap<String, String>,
    pub mark_when_checked: String,
    pub hidden_columns: Vec<String>,
    pub column_order: Vec<String>,
    pub net_colors: HashMap<String, String>,
    pub highlight_row_on_click: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            canvas_layout: "FB".to_string(),
            bom_layout: "left-right".to_string(),
            bom_mode: "grouped".to_string(),
            dark_mode: false,
            highlight_pin1: "none".to_string(),
            redraw_on_drag: true,
            board_rotation: 0.0,
            offset_back_rotation: false,
            render_pads: true,
            render_references: true,
            render_values: true,
            render_silkscreen: true,
            render_fabrication: true,
            render_tracks: true,
            render_zones: true,
            render_dnp_outline: false,
            checkboxes: vec!["Sourced".to_string(), "Placed".to_string()],
            checkbox_stored_refs: HashMap::new(),
            mark_when_checked: String::new(),
            hidden_columns: Vec::new(),
            column_order: Vec::new(),
            net_colors: HashMap::new(),
            highlight_row_on_click: false,
        }
    }
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

pub fn storage_prefix(title: &str, revision: &str) -> String {
    format!("KiCad_HTML_BOM__{}__{}__#", title, revision)
}

pub fn init_settings(prefix: &str) -> Settings {
    let mut s = Settings::default();

    if let Some(v) = read_storage("bomlayout", prefix) {
        if ["bom-only", "left-right", "top-bottom"].contains(&v.as_str()) {
            s.bom_layout = v;
        }
    }
    if let Some(v) = read_storage("bommode", prefix) {
        if ["grouped", "ungrouped", "netlist"].contains(&v.as_str()) {
            s.bom_mode = v;
        }
    }
    if let Some(v) = read_storage("canvaslayout", prefix) {
        if ["F", "FB", "B"].contains(&v.as_str()) {
            s.canvas_layout = v;
        }
    }
    if let Some(v) = read_storage("darkmode", prefix) {
        s.dark_mode = v == "true";
    }
    if let Some(v) = read_storage("highlightpin1", prefix) {
        s.highlight_pin1 = v;
    }
    if let Some(v) = read_storage("padsVisible", prefix) {
        s.render_pads = v != "false";
    }
    if let Some(v) = read_storage("fabricationVisible", prefix) {
        s.render_fabrication = v == "true";
    }
    if let Some(v) = read_storage("silkscreenVisible", prefix) {
        s.render_silkscreen = v != "false";
    }
    if let Some(v) = read_storage("referencesVisible", prefix) {
        s.render_references = v != "false";
    }
    if let Some(v) = read_storage("valuesVisible", prefix) {
        s.render_values = v != "false";
    }
    if let Some(v) = read_storage("tracksVisible", prefix) {
        s.render_tracks = v != "false";
    }
    if let Some(v) = read_storage("zonesVisible", prefix) {
        s.render_zones = v != "false";
    }
    if let Some(v) = read_storage("redrawOnDrag", prefix) {
        s.redraw_on_drag = v != "false";
    }
    if let Some(v) = read_storage("boardRotation", prefix) {
        if let Ok(r) = v.parse::<f64>() {
            s.board_rotation = r;
        }
    }
    if let Some(v) = read_storage("offsetBackRotation", prefix) {
        s.offset_back_rotation = v == "true";
    }
    if let Some(v) = read_storage("netColors", prefix) {
        if let Ok(colors) = serde_json::from_str(&v) {
            s.net_colors = colors;
        }
    }
    if let Some(v) = read_storage("hiddenColumns", prefix) {
        if let Ok(cols) = serde_json::from_str::<Vec<String>>(&v) {
            s.hidden_columns = cols;
        }
    }
    if let Some(v) = read_storage("highlightRowOnClick", prefix) {
        s.highlight_row_on_click = v == "true";
    }

    s
}
