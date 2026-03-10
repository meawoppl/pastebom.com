use super::symbols;
use crate::types::Drawing;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Unit {
    Inch,
    Mm,
}

impl Unit {
    /// Convert a coordinate value to millimeters.
    pub fn coord_to_mm(&self, v: f64) -> f64 {
        match self {
            Unit::Inch => v * 25.4,
            Unit::Mm => v,
        }
    }

    /// Convert a symbol dimension (mils or microns) to millimeters.
    pub fn symbol_dim_to_mm(&self, v: f64) -> f64 {
        match self {
            Unit::Inch => v * 0.0254,
            Unit::Mm => v * 0.001,
        }
    }
}

/// A pad flash from a feature file with position and resolved size.
#[derive(Debug, Clone)]
pub struct PadFeature {
    pub x_mm: f64,
    pub y_mm: f64,
    pub width_mm: f64,
    pub height_mm: f64,
    pub shape: String,
}

/// Parsed feature file data.
#[derive(Debug)]
pub struct FeatureData {
    pub unit: Unit,
    pub drawings: Vec<Drawing>,
    pub pads: Vec<PadFeature>,
    pub zones: Vec<Vec<Vec<[f64; 2]>>>,
}

pub fn parse_features(content: &str) -> FeatureData {
    let mut unit = Unit::Inch;
    let mut sym_table: Vec<String> = Vec::new();
    let mut drawings: Vec<Drawing> = Vec::new();
    let mut pads: Vec<PadFeature> = Vec::new();
    let mut zones: Vec<Vec<Vec<[f64; 2]>>> = Vec::new();

    // Surface parsing state
    let mut in_surface = false;
    let mut surface_polys: Vec<Vec<[f64; 2]>> = Vec::new();
    let mut current_poly: Vec<[f64; 2]> = Vec::new();

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        if let Some(rest) = line.strip_prefix("UNITS=") {
            unit = match rest.trim() {
                "MM" => Unit::Mm,
                _ => Unit::Inch,
            };
            continue;
        }

        if line.starts_with('@') || line.starts_with('&') || line.starts_with("ID=") {
            continue;
        }

        if line.starts_with("F ") {
            continue;
        }

        // Symbol table: $N name
        if let Some(rest) = line.strip_prefix('$') {
            if let Some((_idx, name)) = rest.split_once(' ') {
                let name = name.split_whitespace().next().unwrap_or(name);
                let idx: usize = _idx.parse().unwrap_or(sym_table.len());
                while sym_table.len() <= idx {
                    sym_table.push(String::new());
                }
                sym_table[idx] = name.to_string();
            }
            continue;
        }

        // Surface handling
        if in_surface {
            if line.starts_with("SE") {
                if !surface_polys.is_empty() {
                    let converted: Vec<Vec<[f64; 2]>> = surface_polys
                        .iter()
                        .map(|poly| {
                            poly.iter()
                                .map(|p| [unit.coord_to_mm(p[0]), unit.coord_to_mm(p[1])])
                                .collect()
                        })
                        .collect();

                    zones.push(converted.clone());

                    drawings.push(Drawing::Polygon {
                        pos: [0.0, 0.0],
                        angle: 0.0,
                        polygons: converted,
                        filled: Some(1),
                        width: 0.0,
                    });
                }
                surface_polys.clear();
                in_surface = false;
                continue;
            }
            if line.starts_with("OB") {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 3 {
                    current_poly.clear();
                    if let (Ok(x), Ok(y)) = (parts[1].parse::<f64>(), parts[2].parse::<f64>()) {
                        current_poly.push([x, y]);
                    }
                }
                continue;
            }
            if line.starts_with("OS") {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 3 {
                    if let (Ok(x), Ok(y)) = (parts[1].parse::<f64>(), parts[2].parse::<f64>()) {
                        current_poly.push([x, y]);
                    }
                }
                continue;
            }
            if line.starts_with("OC") {
                // Arc segment - approximate with line segments
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 5 {
                    if let (Ok(xe), Ok(ye), Ok(xc), Ok(yc)) = (
                        parts[1].parse::<f64>(),
                        parts[2].parse::<f64>(),
                        parts[3].parse::<f64>(),
                        parts[4].parse::<f64>(),
                    ) {
                        let cw = parts.len() >= 6 && parts[5] == "Y";
                        if let Some(&[xs, ys]) = current_poly.last() {
                            let arc_points = approximate_arc(xs, ys, xe, ye, xc, yc, cw);
                            for pt in arc_points.into_iter().skip(1) {
                                current_poly.push(pt);
                            }
                        } else {
                            current_poly.push([xe, ye]);
                        }
                    }
                }
                continue;
            }
            if line == "OE" {
                if current_poly.len() >= 3 {
                    surface_polys.push(current_poly.clone());
                }
                current_poly.clear();
                continue;
            }
            continue;
        }

        // Parse feature records
        let (record, _attrs) = line.split_once(';').unwrap_or((line, ""));
        let parts: Vec<&str> = record.split_whitespace().collect();
        if parts.is_empty() {
            continue;
        }

        match parts[0] {
            "L" if parts.len() >= 7 => {
                if let (Ok(xs), Ok(ys), Ok(xe), Ok(ye), Ok(sym_idx)) = (
                    parts[1].parse::<f64>(),
                    parts[2].parse::<f64>(),
                    parts[3].parse::<f64>(),
                    parts[4].parse::<f64>(),
                    parts[5].parse::<usize>(),
                ) {
                    let width = symbol_width(&sym_table, sym_idx, &unit);
                    drawings.push(Drawing::Segment {
                        start: [unit.coord_to_mm(xs), unit.coord_to_mm(ys)],
                        end: [unit.coord_to_mm(xe), unit.coord_to_mm(ye)],
                        width,
                    });
                }
            }
            "A" if parts.len() >= 10 => {
                if let (Ok(xs), Ok(ys), Ok(xe), Ok(ye), Ok(xc), Ok(yc), Ok(sym_idx)) = (
                    parts[1].parse::<f64>(),
                    parts[2].parse::<f64>(),
                    parts[3].parse::<f64>(),
                    parts[4].parse::<f64>(),
                    parts[5].parse::<f64>(),
                    parts[6].parse::<f64>(),
                    parts[7].parse::<usize>(),
                ) {
                    let width = symbol_width(&sym_table, sym_idx, &unit);
                    let start_angle = (ys - yc).atan2(xs - xc).to_degrees();
                    let end_angle = (ye - yc).atan2(xe - xc).to_degrees();
                    let r = ((xs - xc).powi(2) + (ys - yc).powi(2)).sqrt();
                    drawings.push(Drawing::Arc {
                        start: [unit.coord_to_mm(xc), unit.coord_to_mm(yc)],
                        radius: unit.coord_to_mm(r),
                        startangle: start_angle,
                        endangle: end_angle,
                        width,
                    });
                }
            }
            "P" if parts.len() >= 7 => {
                if let (Ok(x), Ok(y), Ok(sym_idx)) = (
                    parts[1].parse::<f64>(),
                    parts[2].parse::<f64>(),
                    parts[3].parse::<usize>(),
                ) {
                    if let Some(info) = sym_table
                        .get(sym_idx)
                        .and_then(|n| symbols::parse_symbol_name(n))
                    {
                        let w = unit.symbol_dim_to_mm(info.width);
                        let h = unit.symbol_dim_to_mm(info.height);
                        let cx = unit.coord_to_mm(x);
                        let cy = unit.coord_to_mm(y);

                        let (shape_name, drawing) = match info.shape {
                            symbols::SymbolShape::Round => {
                                let r = w / 2.0;
                                (
                                    "circle",
                                    Drawing::Circle {
                                        start: [cx, cy],
                                        radius: r,
                                        width: 0.0,
                                        filled: Some(1),
                                    },
                                )
                            }
                            symbols::SymbolShape::Oval => {
                                let hw = w / 2.0;
                                let hh = h / 2.0;
                                (
                                    "oval",
                                    Drawing::Rect {
                                        start: [cx - hw, cy - hh],
                                        end: [cx + hw, cy + hh],
                                        width: 0.0,
                                    },
                                )
                            }
                            _ => {
                                let hw = w / 2.0;
                                let hh = h / 2.0;
                                (
                                    "rect",
                                    Drawing::Rect {
                                        start: [cx - hw, cy - hh],
                                        end: [cx + hw, cy + hh],
                                        width: 0.0,
                                    },
                                )
                            }
                        };

                        pads.push(PadFeature {
                            x_mm: cx,
                            y_mm: cy,
                            width_mm: w,
                            height_mm: h,
                            shape: shape_name.to_string(),
                        });
                        drawings.push(drawing);
                    }
                }
            }
            "S" => {
                in_surface = true;
                surface_polys.clear();
                current_poly.clear();
            }
            "T" => {
                // Text records - skip
            }
            _ => {}
        }
    }

    FeatureData {
        unit,
        drawings,
        pads,
        zones,
    }
}

/// Parse the profile file into board edge drawings.
pub fn parse_profile(content: &str) -> (Unit, Vec<Drawing>) {
    let data = parse_features(content);
    (data.unit, data.drawings)
}

fn symbol_width(sym_table: &[String], idx: usize, unit: &Unit) -> f64 {
    sym_table
        .get(idx)
        .and_then(|name| symbols::parse_symbol_name(name))
        .map(|info| unit.symbol_dim_to_mm(info.width))
        .unwrap_or(0.0)
}

/// Approximate an arc from (xs,ys) to (xe,ye) with center (xc,yc) as line segments.
fn approximate_arc(
    xs: f64,
    ys: f64,
    xe: f64,
    ye: f64,
    xc: f64,
    yc: f64,
    clockwise: bool,
) -> Vec<[f64; 2]> {
    let r = ((xs - xc).powi(2) + (ys - yc).powi(2)).sqrt();
    if r < 1e-9 {
        return vec![[xs, ys], [xe, ye]];
    }

    let start_angle = (ys - yc).atan2(xs - xc);
    let mut end_angle = (ye - yc).atan2(xe - xc);

    if clockwise {
        if end_angle >= start_angle {
            end_angle -= 2.0 * std::f64::consts::PI;
        }
    } else if end_angle <= start_angle {
        end_angle += 2.0 * std::f64::consts::PI;
    }

    let sweep = (end_angle - start_angle).abs();
    let n_segments = (sweep / (std::f64::consts::PI / 18.0)).ceil().max(2.0) as usize;

    let mut points = Vec::with_capacity(n_segments + 1);
    for i in 0..=n_segments {
        let t = i as f64 / n_segments as f64;
        let angle = start_angle + t * (end_angle - start_angle);
        points.push([xc + r * angle.cos(), yc + r * angle.sin()]);
    }

    points
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_profile() {
        let content = r#"UNITS=INCH
ID=6
#
#Num Features
#
F 1

#
#Layer features
#
S P 0;;ID=157339
OB 0 -1.25 I
OS 0 1.25
OS 3.93 1.25
OS 3.93 -1.25
OS 0 -1.25
OE
SE
"#;
        let (unit, drawings) = parse_profile(content);
        assert_eq!(unit, Unit::Inch);
        assert_eq!(drawings.len(), 1);
        match &drawings[0] {
            Drawing::Polygon { polygons, .. } => {
                assert_eq!(polygons.len(), 1);
                assert_eq!(polygons[0].len(), 5);
            }
            _ => panic!("expected polygon"),
        }
    }

    #[test]
    fn test_parse_line_features() {
        let content = r#"UNITS=INCH
$0 r0
$1 r25

L 0 1.25 3.93 1.25 1 P 0;;ID=123
"#;
        let data = parse_features(content);
        assert_eq!(data.drawings.len(), 1);
        match &data.drawings[0] {
            Drawing::Segment { start, end, width } => {
                assert!((start[0] - 0.0).abs() < 0.001);
                assert!((start[1] - 1.25 * 25.4).abs() < 0.01);
                assert!((end[0] - 3.93 * 25.4).abs() < 0.01);
                assert!((width - 25.0 * 0.0254).abs() < 0.001);
            }
            _ => panic!("expected segment"),
        }
    }

    #[test]
    fn test_parse_pad_features() {
        let content = r#"UNITS=INCH
$0 r50
$1 rect20x60

P 1.0 0.5 0 P 0 8 0;;ID=100
P 2.0 1.0 1 P 0 8 0;;ID=101
"#;
        let data = parse_features(content);
        assert_eq!(data.pads.len(), 2);
        assert_eq!(data.pads[0].shape, "circle");
        assert!((data.pads[0].width_mm - 50.0 * 0.0254).abs() < 0.001);
        assert_eq!(data.pads[1].shape, "rect");
        assert!((data.pads[1].width_mm - 20.0 * 0.0254).abs() < 0.001);
        assert!((data.pads[1].height_mm - 60.0 * 0.0254).abs() < 0.001);
    }

    #[test]
    fn test_arc_approximation() {
        let points = approximate_arc(1.0, 0.0, 0.0, 1.0, 0.0, 0.0, false);
        assert!(points.len() >= 3);
        // First point should be near (1,0)
        assert!((points[0][0] - 1.0).abs() < 0.01);
        assert!((points[0][1] - 0.0).abs() < 0.01);
        // Last point should be near (0,1)
        let last = points.last().unwrap();
        assert!((last[0] - 0.0).abs() < 0.01);
        assert!((last[1] - 1.0).abs() < 0.01);
    }
}
