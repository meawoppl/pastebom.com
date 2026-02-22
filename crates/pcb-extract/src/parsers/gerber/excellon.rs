use std::collections::HashMap;

use crate::types::Drawing;

/// Units used in the drill file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExcellonUnits {
    Metric,
    Inches,
}

/// Zero suppression mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ZeroSuppression {
    Trailing,
    Leading,
}

/// Coordinate format: how many integer and decimal digits.
#[derive(Debug, Clone, Copy)]
struct CoordFormat {
    integer: u8,
    decimal: u8,
}

/// A tool definition: tool number → diameter in file units.
#[derive(Debug, Clone)]
struct ToolDef {
    diameter_mm: f64,
}

/// Parse an Excellon drill file into a list of Drawing::Circle primitives.
///
/// Each drill hit becomes a filled circle at the hit position with radius = tool_diameter / 2.
/// Returns None if the content doesn't look like an Excellon file.
pub fn parse_excellon(content: &str) -> Option<Vec<Drawing>> {
    // Quick check: Excellon files typically start with M48 or contain it in the header
    let trimmed = content.trim();
    if !trimmed.starts_with("M48") && !trimmed.contains("M48") {
        return None;
    }

    let mut units = ExcellonUnits::Metric;
    let mut zero_sup = ZeroSuppression::Trailing;
    let mut format = CoordFormat {
        integer: 3,
        decimal: 3,
    };
    let mut tools: HashMap<u32, ToolDef> = HashMap::new();
    let mut current_tool: Option<u32> = None;
    let mut drawings: Vec<Drawing> = Vec::new();
    let mut in_header = false;
    let mut saw_header = false;

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with(';') {
            continue;
        }

        // Header start
        if line == "M48" {
            in_header = true;
            saw_header = true;
            continue;
        }

        // Header end markers
        if line == "%" || line == "M95" {
            in_header = false;
            continue;
        }

        // End of file
        if line == "M30" || line == "M00" {
            break;
        }

        if in_header {
            parse_header_line(line, &mut units, &mut zero_sup, &mut format, &mut tools);
        } else {
            parse_body_line(
                line,
                &mut current_tool,
                &tools,
                units,
                zero_sup,
                format,
                &mut drawings,
            );
        }
    }

    // If we never saw a proper header and found nothing, this wasn't an Excellon file
    if !saw_header && tools.is_empty() && drawings.is_empty() {
        return None;
    }

    Some(drawings)
}

fn parse_header_line(
    line: &str,
    units: &mut ExcellonUnits,
    zero_sup: &mut ZeroSuppression,
    format: &mut CoordFormat,
    tools: &mut HashMap<u32, ToolDef>,
) {
    // Units and format: "METRIC,TZ,000.000" or "INCH,LZ" or "M71" / "M72"
    let upper = line.to_uppercase();

    if upper.starts_with("METRIC") || upper == "M71" {
        *units = ExcellonUnits::Metric;
        parse_format_options(&upper, zero_sup, format);
        return;
    }
    if upper.starts_with("INCH") || upper == "M72" {
        *units = ExcellonUnits::Inches;
        parse_format_options(&upper, zero_sup, format);
        return;
    }

    // Tool definitions: T01C0.300 or T1C0.3
    if let Some(rest) = upper.strip_prefix('T') {
        if let Some(c_pos) = rest.find('C') {
            let tool_num_str = &rest[..c_pos];
            let diameter_str = &rest[c_pos + 1..];
            if let (Ok(tool_num), Ok(diameter)) =
                (tool_num_str.parse::<u32>(), diameter_str.parse::<f64>())
            {
                let diameter_mm = match *units {
                    ExcellonUnits::Metric => diameter,
                    ExcellonUnits::Inches => diameter * 25.4,
                };
                tools.insert(tool_num, ToolDef { diameter_mm });
            }
        }
    }
}

fn parse_format_options(line: &str, zero_sup: &mut ZeroSuppression, format: &mut CoordFormat) {
    // Parse comma-separated options like "METRIC,TZ,000.000"
    for part in line.split(',') {
        let part = part.trim();
        match part {
            "TZ" => *zero_sup = ZeroSuppression::Trailing,
            "LZ" => *zero_sup = ZeroSuppression::Leading,
            _ => {
                // Try to parse coordinate format like "000.000" or "00.0000"
                if part.contains('.') && part.chars().all(|c| c == '0' || c == '.') {
                    if let Some(dot_pos) = part.find('.') {
                        let int_digits = dot_pos as u8;
                        let dec_digits = (part.len() - dot_pos - 1) as u8;
                        if int_digits > 0 && dec_digits > 0 {
                            *format = CoordFormat {
                                integer: int_digits,
                                decimal: dec_digits,
                            };
                        }
                    }
                }
            }
        }
    }
}

fn parse_body_line(
    line: &str,
    current_tool: &mut Option<u32>,
    tools: &HashMap<u32, ToolDef>,
    units: ExcellonUnits,
    zero_sup: ZeroSuppression,
    format: CoordFormat,
    drawings: &mut Vec<Drawing>,
) {
    let upper = line.to_uppercase();

    // Tool selection: T01 or T1 (without C parameter = selection, not definition)
    if upper.starts_with('T') && !upper.contains('C') {
        let num_str: String = upper[1..]
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        if let Ok(num) = num_str.parse::<u32>() {
            *current_tool = Some(num);
        }
        return;
    }

    // Coordinate line: X14.478Y10.541 or X14478Y10541
    if upper.starts_with('X') || upper.starts_with('Y') {
        let tool = match current_tool.and_then(|t| tools.get(&t)) {
            Some(t) => t,
            None => return,
        };

        if let Some((x, y)) = parse_coordinate_line(&upper, units, zero_sup, format) {
            drawings.push(Drawing::Circle {
                start: [x, y],
                radius: tool.diameter_mm / 2.0,
                width: 0.0,
                filled: Some(1),
            });
        }
    }
}

fn parse_coordinate_line(
    line: &str,
    units: ExcellonUnits,
    zero_sup: ZeroSuppression,
    format: CoordFormat,
) -> Option<(f64, f64)> {
    let mut x_str: Option<&str> = None;
    let mut y_str: Option<&str> = None;

    let mut i = 0;
    let chars: Vec<char> = line.chars().collect();
    while i < chars.len() {
        match chars[i] {
            'X' => {
                let start = i + 1;
                let end = find_next_letter(&chars, start);
                x_str = Some(&line[start..end]);
                i = end;
            }
            'Y' => {
                let start = i + 1;
                let end = find_next_letter(&chars, start);
                y_str = Some(&line[start..end]);
                i = end;
            }
            _ => i += 1,
        }
    }

    let x = parse_coord_value(x_str?, units, zero_sup, format)?;
    let y = parse_coord_value(y_str?, units, zero_sup, format)?;
    Some((x, y))
}

fn find_next_letter(chars: &[char], start: usize) -> usize {
    for (i, ch) in chars.iter().enumerate().skip(start) {
        if ch.is_ascii_alphabetic() {
            return i;
        }
    }
    chars.len()
}

fn parse_coord_value(
    s: &str,
    units: ExcellonUnits,
    zero_sup: ZeroSuppression,
    format: CoordFormat,
) -> Option<f64> {
    if s.is_empty() {
        return None;
    }

    let value = if s.contains('.') {
        // Explicit decimal point — parse directly
        s.parse::<f64>().ok()?
    } else {
        // No decimal point — interpret based on format and zero suppression
        let negative = s.starts_with('-');
        let digits: String = s.chars().filter(|c| c.is_ascii_digit()).collect();
        if digits.is_empty() {
            return None;
        }

        let total_digits = (format.integer + format.decimal) as usize;
        let mut padded = digits;
        match zero_sup {
            ZeroSuppression::Trailing | ZeroSuppression::Leading => {
                // Both modes pad on the left. Eagle (and most real-world tools) declare TZ
                // but omit leading zeros too, so the coordinate digits are always right-aligned
                // against the decimal point — pad left to restore.
                while padded.len() < total_digits {
                    padded.insert(0, '0');
                }
            }
        }

        let raw: i64 = padded.parse().ok()?;
        let divisor = 10f64.powi(format.decimal as i32);
        let val = raw as f64 / divisor;
        if negative {
            -val
        } else {
            val
        }
    };

    // Convert to mm
    match units {
        ExcellonUnits::Metric => Some(value),
        ExcellonUnits::Inches => Some(value * 25.4),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_excellon() {
        let content = "\
M48
METRIC,TZ,000.000
T11C0.300
T12C0.800
%
T11
X14.478Y10.541
X14.478Y12.191
T12
X15.000Y10.000
M30
";
        let drawings = parse_excellon(content).unwrap();
        assert_eq!(drawings.len(), 3);

        // First drill hit: T11 (0.3mm diameter = 0.15mm radius)
        match &drawings[0] {
            Drawing::Circle {
                start,
                radius,
                filled,
                ..
            } => {
                assert!((start[0] - 14.478).abs() < 1e-6);
                assert!((start[1] - 10.541).abs() < 1e-6);
                assert!((radius - 0.15).abs() < 1e-6);
                assert_eq!(*filled, Some(1));
            }
            _ => panic!("Expected Circle"),
        }

        // Third drill hit: T12 (0.8mm diameter = 0.4mm radius)
        match &drawings[2] {
            Drawing::Circle { start, radius, .. } => {
                assert!((start[0] - 15.0).abs() < 1e-6);
                assert!((start[1] - 10.0).abs() < 1e-6);
                assert!((radius - 0.4).abs() < 1e-6);
            }
            _ => panic!("Expected Circle"),
        }
    }

    #[test]
    fn test_inch_units() {
        let content = "\
M48
INCH,TZ
T01C0.010
%
T01
X1.000Y1.000
M30
";
        let drawings = parse_excellon(content).unwrap();
        assert_eq!(drawings.len(), 1);
        match &drawings[0] {
            Drawing::Circle { start, radius, .. } => {
                // 1.0 inch = 25.4mm
                assert!((start[0] - 25.4).abs() < 1e-3);
                assert!((start[1] - 25.4).abs() < 1e-3);
                // 0.010 inch diameter = 0.254mm diameter = 0.127mm radius
                assert!((radius - 0.127).abs() < 1e-3);
            }
            _ => panic!("Expected Circle"),
        }
    }

    #[test]
    fn test_no_decimal_trailing_zero_suppressed() {
        let content = "\
M48
METRIC,TZ,000.000
T01C0.500
%
T01
X14478Y10541
M30
";
        let drawings = parse_excellon(content).unwrap();
        assert_eq!(drawings.len(), 1);
        match &drawings[0] {
            Drawing::Circle { start, .. } => {
                // TZ (trailing zeros suppressed): Eagle and most real-world tools
                // omit leading zeros even in TZ mode, so digits are right-aligned
                // against the decimal point. Pad left to 6 digits:
                // "14478" → "014478" → 14.478mm
                assert!((start[0] - 14.478).abs() < 1e-3);
                assert!((start[1] - 10.541).abs() < 1e-3);
            }
            _ => panic!("Expected Circle"),
        }
    }

    #[test]
    fn test_leading_zero_suppression() {
        let content = "\
M48
METRIC,LZ,000.000
T01C0.500
%
T01
X14478Y10541
M30
";
        let drawings = parse_excellon(content).unwrap();
        assert_eq!(drawings.len(), 1);
        match &drawings[0] {
            Drawing::Circle { start, .. } => {
                // LZ (leading zeros suppressed): pad left to 6 digits
                // "14478" → "014478" → 014.478 = 14.478mm
                assert!((start[0] - 14.478).abs() < 1e-3);
                assert!((start[1] - 10.541).abs() < 1e-3);
            }
            _ => panic!("Expected Circle"),
        }
    }

    #[test]
    fn test_not_excellon() {
        assert!(parse_excellon("random text without M48").is_none());
    }

    #[test]
    fn test_empty_drill_file() {
        let content = "\
M48
METRIC,TZ,000.000
%
M30
";
        // Valid Excellon but no drill hits — returns empty vec
        let drawings = parse_excellon(content).unwrap();
        assert!(drawings.is_empty());
    }

    #[test]
    fn test_m71_m72_units() {
        let content = "\
M48
M71
T01C0.500
%
T01
X10.000Y20.000
M30
";
        let drawings = parse_excellon(content).unwrap();
        assert_eq!(drawings.len(), 1);
        match &drawings[0] {
            Drawing::Circle { start, .. } => {
                assert!((start[0] - 10.0).abs() < 1e-6);
                assert!((start[1] - 20.0).abs() < 1e-6);
            }
            _ => panic!("Expected Circle"),
        }
    }

    #[test]
    fn test_multiple_tools() {
        let content = "\
M48
METRIC,TZ,000.000
T01C0.300
T02C0.800
T03C1.000
%
T01
X1.000Y1.000
X2.000Y2.000
T02
X3.000Y3.000
T03
X4.000Y4.000
X5.000Y5.000
M30
";
        let drawings = parse_excellon(content).unwrap();
        assert_eq!(drawings.len(), 5);

        // T01 hits should have 0.15mm radius
        match &drawings[0] {
            Drawing::Circle { radius, .. } => assert!((radius - 0.15).abs() < 1e-6),
            _ => panic!("Expected Circle"),
        }
        // T02 hit should have 0.4mm radius
        match &drawings[2] {
            Drawing::Circle { radius, .. } => assert!((radius - 0.4).abs() < 1e-6),
            _ => panic!("Expected Circle"),
        }
        // T03 hits should have 0.5mm radius
        match &drawings[3] {
            Drawing::Circle { radius, .. } => assert!((radius - 0.5).abs() < 1e-6),
            _ => panic!("Expected Circle"),
        }
    }

    #[test]
    fn test_eagle_tz_leading_zeros_dropped() {
        // Eagle generates METRIC,TZ files but drops leading zeros, so "4572" means
        // 4.572mm (not 457.200mm). Verify small coordinates decode correctly.
        let content = "\
M48
;GenerationSoftware,Autodesk,EAGLE,9.7.0*%
FMAT,2
ICI,OFF
METRIC,TZ,000.000
T1C4.300
%
G90
M71
T1
X4572Y4572
X135128Y58928
M30
";
        let drawings = parse_excellon(content).unwrap();
        assert_eq!(drawings.len(), 2);
        match &drawings[0] {
            Drawing::Circle { start, radius, .. } => {
                assert!((start[0] - 4.572).abs() < 1e-3, "x={}", start[0]);
                assert!((start[1] - 4.572).abs() < 1e-3, "y={}", start[1]);
                assert!((radius - 2.15).abs() < 1e-3);
            }
            _ => panic!("Expected Circle"),
        }
        match &drawings[1] {
            Drawing::Circle { start, .. } => {
                assert!((start[0] - 135.128).abs() < 1e-3, "x={}", start[0]);
                assert!((start[1] - 58.928).abs() < 1e-3, "y={}", start[1]);
            }
            _ => panic!("Expected Circle"),
        }
    }
}
