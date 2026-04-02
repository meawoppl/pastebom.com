mod components;
mod features;
mod matrix;
mod symbols;

use crate::bom::{generate_bom, BomConfig};
use crate::error::ExtractError;
use crate::types::*;
use crate::ExtractOptions;
use std::collections::HashMap;
use std::io::Read;

use features::{PadFeature, Unit};
use matrix::{LayerContext, LayerType};

/// Parse an ODB++ archive (.tgz or .zip) into PcbData.
pub fn parse(data: &[u8], opts: &ExtractOptions) -> Result<PcbData, ExtractError> {
    let files = extract_archive(data)?;
    let job_root = find_job_root(&files)?;

    // Parse the matrix to understand layers
    let matrix_path = format!("{job_root}/matrix/matrix");
    let matrix_content = get_file(&files, &matrix_path)?;
    let mtx = matrix::parse_matrix(&matrix_content);

    // Find the step name
    let step_name = mtx
        .steps
        .first()
        .map(|s| s.name.clone())
        .unwrap_or_else(|| find_step_name(&files, &job_root));
    let step_path = format!("{job_root}/steps/{}", step_name.to_lowercase());

    // Parse board profile (outline)
    let profile_path = format!("{step_path}/profile");
    let (_profile_unit, profile_drawings) = match get_file(&files, &profile_path) {
        Ok(content) => features::parse_profile(&content),
        Err(_) => (Unit::Inch, Vec::new()),
    };

    // Determine board edges from profile, or fall back to outline layer
    let mut edges = profile_drawings;
    if edges.is_empty() {
        for layer in &mtx.layers {
            if layer.layer_type == LayerType::Rout {
                let feat_path =
                    format!("{step_path}/layers/{}/features", layer.name.to_lowercase());
                if let Ok(content) = get_file(&files, &feat_path) {
                    let feat_data = features::parse_features(&content);
                    edges = feat_data.drawings;
                    if !edges.is_empty() {
                        break;
                    }
                }
            }
        }
    }

    let edges_bbox = BBox::from_drawings(&edges);

    // Parse silkscreen layers
    let mut silk_front = Vec::new();
    let mut silk_back = Vec::new();
    for layer in &mtx.layers {
        if layer.layer_type == LayerType::SilkScreen && layer.context == LayerContext::Board {
            let feat_path = format!("{step_path}/layers/{}/features", layer.name.to_lowercase());
            if let Ok(content) = get_file(&files, &feat_path) {
                let feat_data = features::parse_features(&content);
                let name_upper = layer.name.to_uppercase();
                if name_upper.contains("TOP") || name_upper == "SST" {
                    silk_front = feat_data.drawings;
                } else if name_upper.contains("BOT") || name_upper == "SSB" {
                    silk_back = feat_data.drawings;
                }
            }
        }
    }

    // Parse fabrication/document layers
    let mut fab_front = Vec::new();
    let mut fab_back = Vec::new();
    for layer in &mtx.layers {
        if layer.layer_type == LayerType::Document {
            let feat_path = format!("{step_path}/layers/{}/features", layer.name.to_lowercase());
            if let Ok(content) = get_file(&files, &feat_path) {
                let feat_data = features::parse_features(&content);
                if !feat_data.drawings.is_empty() {
                    let name_upper = layer.name.to_uppercase();
                    if name_upper.contains("BOT") || name_upper.contains("BOTTOM") {
                        fab_back = feat_data.drawings;
                    } else {
                        // Default to front fab
                        if fab_front.is_empty() {
                            fab_front = feat_data.drawings;
                        }
                    }
                }
            }
        }
    }

    // Parse EDA data for net names
    let mut net_names: Vec<String> = Vec::new();
    let eda_path = format!("{step_path}/eda/data");
    if let Ok(eda_content) = get_file(&files, &eda_path) {
        net_names = parse_eda_net_names(&eda_content);
    }

    // Parse copper features from top/bottom to get pad sizes for component matching
    let signal_layers: Vec<_> = mtx
        .layers
        .iter()
        .filter(|l| {
            (l.layer_type == LayerType::Signal || l.layer_type == LayerType::PowerGround)
                && l.context == LayerContext::Board
        })
        .collect();
    let first_signal = signal_layers.first().map(|l| l.name.to_uppercase());
    let last_signal = signal_layers.last().map(|l| l.name.to_uppercase());

    // Pre-parse top/bottom copper for pad matching
    let top_copper_pads = first_signal
        .as_ref()
        .and_then(|name| {
            let feat_path = format!("{step_path}/layers/{}/features", name.to_lowercase());
            get_file(&files, &feat_path).ok()
        })
        .map(|content| features::parse_features(&content).pads)
        .unwrap_or_default();

    let bot_copper_pads = last_signal
        .as_ref()
        .and_then(|name| {
            let feat_path = format!("{step_path}/layers/{}/features", name.to_lowercase());
            get_file(&files, &feat_path).ok()
        })
        .map(|content| features::parse_features(&content).pads)
        .unwrap_or_default();

    // Parse drill layer for through-hole info
    let mut drill_positions: HashMap<(i64, i64), f64> = HashMap::new();
    for layer in &mtx.layers {
        if layer.layer_type == LayerType::Drill {
            let feat_path = format!("{step_path}/layers/{}/features", layer.name.to_lowercase());
            if let Ok(content) = get_file(&files, &feat_path) {
                let feat_data = features::parse_features(&content);
                for pad in &feat_data.pads {
                    // Key by rounded position for matching
                    let key = (
                        (pad.x_mm * 1000.0).round() as i64,
                        (pad.y_mm * 1000.0).round() as i64,
                    );
                    drill_positions.insert(key, pad.width_mm);
                }
            }
        }
    }

    // Parse components from comp_+_top and comp_+_bot
    let mut all_footprints = Vec::new();
    let mut all_components = Vec::new();

    for layer in &mtx.layers {
        if layer.layer_type != LayerType::Component {
            continue;
        }
        let comp_path = format!(
            "{step_path}/layers/{}/components",
            layer.name.to_lowercase()
        );
        let comp_content = match get_file(&files, &comp_path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let name_upper = layer.name.to_uppercase();
        let side = if name_upper.contains("TOP") {
            Side::Front
        } else {
            Side::Back
        };
        let side_str = side.as_str().to_string();
        let copper_pads = if side == Side::Front {
            &top_copper_pads
        } else {
            &bot_copper_pads
        };

        let (unit, odb_components) = components::parse_components(&comp_content);

        for odb_comp in &odb_components {
            let fp_index = all_footprints.len();
            let cx = unit.coord_to_mm(odb_comp.x);
            let cy = -unit.coord_to_mm(odb_comp.y);

            let mut fp_pads = Vec::new();
            let mut pad_bbox = BBox::empty();
            let rot_rad = -odb_comp.rotation.to_radians();

            for (pin_idx, pin) in odb_comp.pins.iter().enumerate() {
                let px = unit.coord_to_mm(pin.x);
                let py = -unit.coord_to_mm(pin.y);

                // Compute position relative to component center, un-rotated
                let dx = px - cx;
                let dy = py - cy;
                let rel_x = dx * rot_rad.cos() - dy * rot_rad.sin();
                let rel_y = dx * rot_rad.sin() + dy * rot_rad.cos();
                let rel_x = if odb_comp.mirror { -rel_x } else { rel_x };

                let net_name = if pin.net_num >= 0 {
                    net_names.get(pin.net_num as usize).cloned()
                } else {
                    None
                };

                // Match pad from copper features by position
                let matched = find_nearest_pad(copper_pads, px, py, 0.1);

                // Check for drill at this position
                let drill_key = ((px * 1000.0).round() as i64, (py * 1000.0).round() as i64);
                let drill_size = drill_positions.get(&drill_key).copied();

                let (pad_w, pad_h, shape, pad_type) = if let Some(mp) = &matched {
                    let pt = if drill_size.is_some() { "th" } else { "smd" };
                    (mp.width_mm, mp.height_mm, mp.shape.clone(), pt.to_string())
                } else if drill_size.is_some() {
                    (1.0, 1.0, "circle".to_string(), "th".to_string())
                } else {
                    (0.5, 0.5, "circle".to_string(), "smd".to_string())
                };

                pad_bbox.expand_point(rel_x - pad_w / 2.0, rel_y - pad_h / 2.0);
                pad_bbox.expand_point(rel_x + pad_w / 2.0, rel_y + pad_h / 2.0);

                let pad_layers = if drill_size.is_some() {
                    vec!["F".to_string(), "B".to_string()]
                } else {
                    vec![side_str.clone()]
                };

                fp_pads.push(Pad {
                    layers: pad_layers,
                    pos: [px, py],
                    size: [pad_w, pad_h],
                    shape,
                    pad_type,
                    angle: Some(odb_comp.rotation),
                    pin1: if pin_idx == 0 { Some(1) } else { None },
                    net: net_name,
                    offset: None,
                    radius: None,
                    chamfpos: None,
                    chamfratio: None,
                    drillshape: drill_size.map(|_| "circle".to_string()),
                    drillsize: drill_size.map(|d| [d, d]),
                    svgpath: None,
                    polygons: None,
                });
            }

            if fp_pads.is_empty() {
                pad_bbox.expand_point(-0.5, -0.5);
                pad_bbox.expand_point(0.5, 0.5);
            }

            let bbox_w = pad_bbox.maxx - pad_bbox.minx;
            let bbox_h = pad_bbox.maxy - pad_bbox.miny;

            all_footprints.push(Footprint {
                ref_: odb_comp.ref_des.clone(),
                center: [cx, cy],
                bbox: FootprintBBox {
                    pos: [cx, cy],
                    relpos: [pad_bbox.minx, pad_bbox.miny],
                    size: [bbox_w, bbox_h],
                    angle: odb_comp.rotation,
                },
                pads: fp_pads,
                drawings: Vec::new(),
                layer: side_str.clone(),
            });

            // Build BOM component data
            let value = odb_comp
                .properties
                .get("VALUE")
                .cloned()
                .unwrap_or_default();
            let footprint_name = odb_comp
                .properties
                .get("PART_NAME")
                .cloned()
                .unwrap_or_default();

            let mut extra_fields = HashMap::new();
            for (k, v) in &odb_comp.properties {
                match k.as_str() {
                    "MANUFACTURER" => {
                        extra_fields.insert("Manufacturer".to_string(), v.clone());
                    }
                    "MFG_PART_NUMBER" => {
                        extra_fields.insert("MPN".to_string(), v.clone());
                    }
                    "DESCRIPTION" => {
                        extra_fields.insert("Description".to_string(), v.clone());
                    }
                    _ => {}
                }
            }

            let skip = odb_comp
                .properties
                .get("BOM")
                .map(|v| v != "Y")
                .unwrap_or(false);

            all_components.push(Component {
                ref_: odb_comp.ref_des.clone(),
                val: value,
                footprint_name,
                layer: side,
                footprint_index: fp_index,
                extra_fields,
                attr: if skip {
                    Some("virtual".to_string())
                } else {
                    None
                },
            });
        }
    }

    // Parse copper tracks, pads, and zones if requested
    let mut tracks = None;
    let mut copper_pads_out = None;
    let mut zones_out = None;

    if opts.include_tracks {
        let mut front_tracks = Vec::new();
        let mut back_tracks = Vec::new();
        let mut inner_tracks: HashMap<String, Vec<Track>> = HashMap::new();

        let mut front_pads_draw = Vec::new();
        let mut back_pads_draw = Vec::new();
        let mut inner_pads: HashMap<String, Vec<Drawing>> = HashMap::new();

        let mut front_zones = Vec::new();
        let mut back_zones = Vec::new();
        let mut inner_zones: HashMap<String, Vec<Zone>> = HashMap::new();

        for layer in &signal_layers {
            let feat_path = format!("{step_path}/layers/{}/features", layer.name.to_lowercase());
            let content = match get_file(&files, &feat_path) {
                Ok(c) => c,
                Err(_) => continue,
            };
            let feat_data = features::parse_features(&content);

            let mut layer_tracks = Vec::new();
            let mut layer_pads = Vec::new();

            for d in feat_data.drawings {
                match d {
                    Drawing::Segment { start, end, width } => {
                        layer_tracks.push(Track::Segment {
                            start,
                            end,
                            width,
                            net: None,
                            drillsize: None,
                        });
                    }
                    Drawing::Arc {
                        start,
                        radius,
                        startangle,
                        endangle,
                        width,
                    } => {
                        layer_tracks.push(Track::Arc {
                            center: start,
                            startangle,
                            endangle,
                            radius,
                            width,
                            net: None,
                        });
                    }
                    Drawing::Circle { .. } | Drawing::Rect { .. } => {
                        layer_pads.push(d);
                    }
                    Drawing::Polygon { .. } => {
                        // Polygons already handled via zones
                    }
                    _ => {}
                }
            }

            let layer_zones: Vec<Zone> = feat_data
                .zones
                .into_iter()
                .map(|polys| Zone {
                    polygons: Some(polys),
                    svgpath: None,
                    width: None,
                    net: None,
                    fillrule: None,
                })
                .collect();

            let name_upper = layer.name.to_uppercase();
            if Some(&name_upper) == first_signal.as_ref() {
                front_tracks = layer_tracks;
                front_pads_draw = layer_pads;
                front_zones = layer_zones;
            } else if Some(&name_upper) == last_signal.as_ref() {
                back_tracks = layer_tracks;
                back_pads_draw = layer_pads;
                back_zones = layer_zones;
            } else {
                inner_tracks.insert(layer.name.clone(), layer_tracks);
                inner_pads.insert(layer.name.clone(), layer_pads);
                inner_zones.insert(layer.name.clone(), layer_zones);
            }
        }

        tracks = Some(LayerData {
            front: front_tracks,
            back: back_tracks,
            inner: inner_tracks,
        });

        copper_pads_out = Some(LayerData {
            front: front_pads_draw,
            back: back_pads_draw,
            inner: inner_pads,
        });

        zones_out = Some(LayerData {
            front: front_zones,
            back: back_zones,
            inner: inner_zones,
        });
    }

    // Generate BOM
    let bom_config = BomConfig {
        fields: vec![
            "Value".to_string(),
            "Footprint".to_string(),
            "Manufacturer".to_string(),
            "MPN".to_string(),
            "Description".to_string(),
        ],
        ..BomConfig::default()
    };
    let bom = Some(generate_bom(&all_footprints, &all_components, &bom_config));

    let title = find_title(&files, &job_root);

    let nets = if opts.include_nets && !net_names.is_empty() {
        Some(net_names)
    } else {
        None
    };

    Ok(PcbData {
        edges_bbox,
        edges,
        drawings: Drawings {
            silkscreen: LayerData {
                front: silk_front,
                back: silk_back,
                inner: HashMap::new(),
            },
            fabrication: LayerData {
                front: fab_front,
                back: fab_back,
                inner: HashMap::new(),
            },
        },
        footprints: all_footprints,
        metadata: Metadata {
            title,
            revision: String::new(),
            company: String::new(),
            date: String::new(),
        },
        bom,
        ibom_version: None,
        tracks,
        copper_pads: copper_pads_out,
        zones: zones_out,
        nets,
        font_data: None,
    })
}

/// Find the nearest copper pad within tolerance (mm) of a given position.
fn find_nearest_pad(pads: &[PadFeature], x_mm: f64, y_mm: f64, tol: f64) -> Option<PadFeature> {
    let mut best: Option<(f64, &PadFeature)> = None;
    for pad in pads {
        let dist = ((pad.x_mm - x_mm).powi(2) + (pad.y_mm - y_mm).powi(2)).sqrt();
        if dist <= tol && (best.is_none() || dist < best.unwrap().0) {
            best = Some((dist, pad));
        }
    }
    best.map(|(_, p)| p.clone())
}

/// Try extracting as .tgz first, then fall back to .zip.
fn extract_archive(data: &[u8]) -> Result<HashMap<String, Vec<u8>>, ExtractError> {
    let limit = crate::MAX_DECOMPRESSED_BYTES;
    extract_tar_gz(data, limit).or_else(|_| extract_zip(data, limit))
}

/// Extract all files from a .zip archive into memory.
fn extract_zip(
    data: &[u8],
    max_decompressed: u64,
) -> Result<HashMap<String, Vec<u8>>, ExtractError> {
    let reader = std::io::Cursor::new(data);
    let mut archive = zip::ZipArchive::new(reader)
        .map_err(|e| ExtractError::ParseError(format!("Failed to read ZIP archive: {e}")))?;
    let mut files = HashMap::new();
    let mut total_decompressed: u64 = 0;

    for i in 0..archive.len() {
        let entry = archive
            .by_index(i)
            .map_err(|e| ExtractError::ParseError(format!("Failed to read ZIP entry: {e}")))?;
        if entry.is_file() {
            let name = entry.name().to_string();
            let remaining = max_decompressed.saturating_sub(total_decompressed);
            let mut content = Vec::new();
            entry
                .take(remaining + 1)
                .read_to_end(&mut content)
                .map_err(|e| {
                    ExtractError::ParseError(format!("Failed to read ZIP entry content: {e}"))
                })?;
            total_decompressed += content.len() as u64;
            if total_decompressed > max_decompressed {
                return Err(ExtractError::DecompressionBomb);
            }
            files.insert(name, content);
        }
    }

    Ok(files)
}

/// Extract all files from a .tgz archive into memory.
fn extract_tar_gz(
    data: &[u8],
    max_decompressed: u64,
) -> Result<HashMap<String, Vec<u8>>, ExtractError> {
    let gz = flate2::read::GzDecoder::new(data);
    let mut archive = tar::Archive::new(gz);
    let mut files = HashMap::new();
    let mut total_decompressed: u64 = 0;

    for entry in archive
        .entries()
        .map_err(|e| ExtractError::ParseError(format!("Failed to read tar.gz archive: {e}")))?
    {
        let mut entry = entry
            .map_err(|e| ExtractError::ParseError(format!("Failed to read tar entry: {e}")))?;
        let path = entry
            .path()
            .map_err(|e| ExtractError::ParseError(format!("Invalid tar path: {e}")))?
            .to_string_lossy()
            .to_string();

        if entry.header().entry_type().is_file() {
            let remaining = max_decompressed.saturating_sub(total_decompressed);
            let mut content = Vec::new();
            (&mut entry)
                .take(remaining + 1)
                .read_to_end(&mut content)
                .map_err(|e| {
                    ExtractError::ParseError(format!("Failed to read tar entry content: {e}"))
                })?;
            total_decompressed += content.len() as u64;
            if total_decompressed > max_decompressed {
                return Err(ExtractError::DecompressionBomb);
            }
            files.insert(path, content);
        }
    }

    Ok(files)
}

/// Find the job root directory (first path component).
fn find_job_root(files: &HashMap<String, Vec<u8>>) -> Result<String, ExtractError> {
    for path in files.keys() {
        if path.contains("/matrix/matrix") {
            let root = path
                .strip_suffix("/matrix/matrix")
                .unwrap_or("")
                .to_string();
            return Ok(root);
        }
    }
    Err(ExtractError::ParseError(
        "No matrix/matrix file found - not a valid ODB++ archive".to_string(),
    ))
}

/// Find step name by looking at directory structure.
fn find_step_name(files: &HashMap<String, Vec<u8>>, job_root: &str) -> String {
    let prefix = format!("{job_root}/steps/");
    for path in files.keys() {
        if let Some(rest) = path.strip_prefix(&prefix) {
            if let Some(step) = rest.split('/').next() {
                return step.to_string();
            }
        }
    }
    "pcb".to_string()
}

/// Get a file's content as UTF-8 string, trying case-insensitive matching.
fn get_file(files: &HashMap<String, Vec<u8>>, path: &str) -> Result<String, ExtractError> {
    if let Some(data) = files.get(path) {
        return Ok(String::from_utf8_lossy(data).to_string());
    }

    let lower = path.to_lowercase();
    for (k, v) in files {
        if k.to_lowercase() == lower {
            return Ok(String::from_utf8_lossy(v).to_string());
        }
    }

    Err(ExtractError::ParseError(format!(
        "File not found in archive: {path}"
    )))
}

/// Parse EDA data file for net names.
fn parse_eda_net_names(content: &str) -> Vec<String> {
    let mut nets = Vec::new();

    for line in content.lines() {
        let line = line.trim();
        if line.starts_with("NET ") {
            let (record, _) = line.split_once(';').unwrap_or((line, ""));
            let parts: Vec<&str> = record.split_whitespace().collect();
            if parts.len() >= 2 {
                let name = parts[1].trim_start_matches('$').to_string();
                nets.push(name);
            }
        }
    }

    nets
}

/// Extract title from misc/info or job name.
fn find_title(files: &HashMap<String, Vec<u8>>, job_root: &str) -> String {
    let job_name_path = format!("{job_root}/misc/job_name");
    if let Ok(content) = get_file(files, &job_name_path) {
        let name = content.trim().to_string();
        if !name.is_empty() {
            return name;
        }
    }

    job_root.rsplit('/').next().unwrap_or("ODB++").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_abs_diff_eq;

    #[test]
    fn test_compute_bbox() {
        let edges = vec![Drawing::Segment {
            start: [0.0, 0.0],
            end: [100.0, 50.0],
            width: 0.0,
        }];
        let bbox = BBox::from_drawings(&edges);
        assert_abs_diff_eq!(bbox.minx, 0.0, epsilon = 1e-6);
        assert_abs_diff_eq!(bbox.miny, 0.0, epsilon = 1e-6);
        assert_abs_diff_eq!(bbox.maxx, 100.0, epsilon = 1e-6);
        assert_abs_diff_eq!(bbox.maxy, 50.0, epsilon = 1e-6);
    }

    #[test]
    fn test_find_nearest_pad() {
        let pads = vec![
            PadFeature {
                x_mm: 10.0,
                y_mm: 20.0,
                width_mm: 1.0,
                height_mm: 1.0,
                shape: "circle".to_string(),
            },
            PadFeature {
                x_mm: 15.0,
                y_mm: 25.0,
                width_mm: 2.0,
                height_mm: 3.0,
                shape: "rect".to_string(),
            },
        ];

        let found = find_nearest_pad(&pads, 10.01, 20.01, 0.1);
        assert!(found.is_some());
        assert_eq!(found.unwrap().shape, "circle");

        let found = find_nearest_pad(&pads, 100.0, 200.0, 0.1);
        assert!(found.is_none());
    }
}
