use super::features::Unit;
use std::collections::HashMap;

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct OdbComponent {
    pub pkg_ref: u32,
    pub x: f64,
    pub y: f64,
    pub rotation: f64,
    pub mirror: bool,
    pub ref_des: String,
    pub properties: HashMap<String, String>,
    pub pins: Vec<OdbPin>,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct OdbPin {
    pub num: u32,
    pub x: f64,
    pub y: f64,
    pub rotation: f64,
    pub mirror: bool,
    pub net_num: i32,
}

pub fn parse_components(content: &str) -> (Unit, Vec<OdbComponent>) {
    let mut unit = Unit::Inch;
    let mut components = Vec::new();
    let mut current: Option<OdbComponent> = None;

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

        if line.starts_with("ID=") || line.starts_with('@') || line.starts_with('&') {
            continue;
        }

        if line.starts_with("CMP ") {
            // Save previous component
            if let Some(comp) = current.take() {
                components.push(comp);
            }

            // Parse: CMP <pkg_ref> <x> <y> <rotation> <mirror> <comp_name> <part_name> ;attrs
            let (record, _attrs) = line.split_once(';').unwrap_or((line, ""));
            let parts: Vec<&str> = record.split_whitespace().collect();
            if parts.len() >= 7 {
                current = Some(OdbComponent {
                    pkg_ref: parts[1].parse().unwrap_or(0),
                    x: parts[2].parse().unwrap_or(0.0),
                    y: parts[3].parse().unwrap_or(0.0),
                    rotation: parts[4].parse().unwrap_or(0.0),
                    mirror: parts[5] == "Y",
                    ref_des: parts[6].to_string(),
                    properties: HashMap::new(),
                    pins: Vec::new(),
                });
            }
        } else if let Some(rest) = line.strip_prefix("PRP ") {
            if let Some(comp) = current.as_mut() {
                if let Some((key, val_quoted)) = rest.split_once(' ') {
                    let val = val_quoted
                        .trim()
                        .trim_start_matches('\'')
                        .trim_end_matches('\'')
                        .trim()
                        .to_string();
                    comp.properties.insert(key.to_string(), val);
                }
            }
        } else if line.starts_with("TOP ") {
            // TOP <pin_num> <x> <y> <rotation> <mirror> <net_num> <subnet_num> <toeprint>
            if let Some(comp) = current.as_mut() {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 7 {
                    comp.pins.push(OdbPin {
                        num: parts[1].parse().unwrap_or(0),
                        x: parts[2].parse().unwrap_or(0.0),
                        y: parts[3].parse().unwrap_or(0.0),
                        rotation: parts[4].parse().unwrap_or(0.0),
                        mirror: parts[5] == "Y",
                        net_num: parts[6].parse().unwrap_or(-1),
                    });
                }
            }
        }
    }

    // Don't forget the last component
    if let Some(comp) = current {
        components.push(comp);
    }

    (unit, components)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_components() {
        let content = r#"UNITS=INCH
ID=525
#
#Component attribute names
#
@0 .comp_height

# CMP 0
CMP 1 0.225 -0.275 90.0 N JAP ??? ;1=0.151180;ID=532
PRP MANUFACTURER 'ROSENBERGER'
PRP VALUE '59S2AQ-40MT5-Z_1'
PRP PART_NAME '59S2AQ-40MT5'
TOP 0 0.06772 -0.275 90.0 N 47 0 1
TOP 1 0.09134 -0.39311 90.0 N 2 0 2
#
# CMP 1
CMP 2 0.275 -0.7 270.0 N J8 ??? ;1=0.505910;ID=536
PRP VALUE 'E6S201-40MT5-Z'
TOP 0 0.275 -0.7 270.0 N 41 0 1
"#;
        let (unit, comps) = parse_components(content);
        assert_eq!(unit, Unit::Inch);
        assert_eq!(comps.len(), 2);

        assert_eq!(comps[0].ref_des, "JAP");
        assert_eq!(comps[0].rotation, 90.0);
        assert!(!comps[0].mirror);
        assert_eq!(comps[0].pins.len(), 2);
        assert_eq!(
            comps[0].properties.get("VALUE").map(|s| s.as_str()),
            Some("59S2AQ-40MT5-Z_1")
        );
        assert_eq!(
            comps[0].properties.get("MANUFACTURER").map(|s| s.as_str()),
            Some("ROSENBERGER")
        );

        assert_eq!(comps[1].ref_des, "J8");
        assert_eq!(comps[1].pins.len(), 1);
    }
}
