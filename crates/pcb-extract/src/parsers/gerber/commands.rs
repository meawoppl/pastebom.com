use crate::error::ExtractError;

use super::coord::{CoordinateFormat, Units};
use super::lexer::GerberToken;

/// Aperture shape template from an %AD command.
#[derive(Debug, Clone, PartialEq)]
pub enum ApertureTemplate {
    Circle {
        diameter: f64,
    },
    Rectangle {
        x_size: f64,
        y_size: f64,
    },
    Obround {
        x_size: f64,
        y_size: f64,
    },
    Polygon {
        outer_diameter: f64,
        num_vertices: u32,
        rotation: f64,
    },
    /// Reference to a user-defined aperture macro.
    Macro {
        name: String,
        params: Vec<f64>,
    },
}

/// Layer polarity from %LP command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Polarity {
    Dark,
    Clear,
}

/// Board side for non-copper layers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoardSide {
    Top,
    Bottom,
}

/// Copper layer side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopperSide {
    Top,
    Bottom,
    Inner,
}

/// Parsed Gerber X2 FileFunction attribute.
#[derive(Debug, Clone, PartialEq)]
pub enum FileFunction {
    Copper { layer_num: u32, side: CopperSide },
    Legend { side: BoardSide },
    SolderMask { side: BoardSide },
    Paste { side: BoardSide },
    Profile,
    Other(String),
}

/// A fully parsed Gerber command.
#[derive(Debug, Clone, PartialEq)]
pub enum GerberCommand {
    /// %FS - Format specification
    FormatSpec(CoordinateFormat),
    /// %MO - Unit mode
    Units(Units),
    /// %AD - Aperture definition
    ApertureDefine {
        code: u32,
        template: ApertureTemplate,
    },
    /// Dnn (n >= 10) - Select aperture
    SelectAperture(u32),
    /// D01 - Interpolate (draw)
    Interpolate {
        x: Option<i64>,
        y: Option<i64>,
        i: Option<i64>,
        j: Option<i64>,
    },
    /// D02 - Move
    Move { x: Option<i64>, y: Option<i64> },
    /// D03 - Flash
    Flash { x: Option<i64>, y: Option<i64> },
    /// G01 - Linear interpolation mode
    LinearMode,
    /// G02 - Clockwise circular interpolation
    ClockwiseArcMode,
    /// G03 - Counter-clockwise circular interpolation
    CounterClockwiseArcMode,
    /// G36 - Begin region
    RegionBegin,
    /// G37 - End region
    RegionEnd,
    /// G74 - Single quadrant arc mode
    SingleQuadrant,
    /// G75 - Multi quadrant arc mode
    MultiQuadrant,
    /// %LP - Layer polarity
    Polarity(Polarity),
    /// %TF.FileFunction - Gerber X2 file function attribute
    FileFunction(FileFunction),
    /// %AM - Aperture macro definition
    MacroDefine { name: String, body: Vec<String> },
    /// %SR - Step-and-repeat block.
    /// When x_repeat=1 AND y_repeat=1 this closes (or resets) any open SR block.
    /// Otherwise it opens a new block that will be tiled x_repeat × y_repeat times
    /// with x_step / y_step spacing (in file units, mm or inch).
    StepRepeat {
        x_repeat: u32,
        y_repeat: u32,
        x_step: f64,
        y_step: f64,
    },
    /// %MI - Image mirroring (deprecated but still in legacy files)
    /// A=true mirrors about the Y-axis (flips X), B=true mirrors about the X-axis (flips Y).
    ImageMirror { a: bool, b: bool },
    /// %SF - Image scaling (deprecated but still in legacy files)
    /// a scales the X axis, b scales the Y axis.
    ImageScale { a: f64, b: f64 },
    /// M02 - End of file
    EndOfFile,
}

/// Parse a token stream into a sequence of Gerber commands.
pub fn parse_commands(tokens: &[GerberToken]) -> Result<Vec<GerberCommand>, ExtractError> {
    let mut commands = Vec::new();
    let mut macro_name: Option<String> = None;
    let mut macro_body: Vec<String> = Vec::new();

    for token in tokens {
        match token {
            GerberToken::Extended(content) => {
                // Check if this starts a new macro definition
                if content.starts_with("AM") && content.len() > 2 {
                    // Flush any previous macro
                    if let Some(name) = macro_name.take() {
                        commands.push(GerberCommand::MacroDefine {
                            name,
                            body: std::mem::take(&mut macro_body),
                        });
                    }
                    macro_name = Some(content[2..].to_string());
                    macro_body.clear();
                    continue;
                }

                // If we're inside a macro, collect body lines
                if macro_name.is_some() {
                    // Body lines are primitive definitions (start with a digit) or comments
                    let trimmed = content.trim();
                    if trimmed.starts_with(|c: char| c.is_ascii_digit()) || trimmed.starts_with('$')
                    {
                        macro_body.push(trimmed.to_string());
                        continue;
                    }
                    // Non-body extended token ends the macro
                    let name = macro_name.take().unwrap();
                    commands.push(GerberCommand::MacroDefine {
                        name,
                        body: std::mem::take(&mut macro_body),
                    });
                }

                if let Some(cmd) = parse_extended(content)? {
                    commands.push(cmd);
                }
            }
            GerberToken::Word(word) => {
                // A word token ends any open macro definition
                if let Some(name) = macro_name.take() {
                    commands.push(GerberCommand::MacroDefine {
                        name,
                        body: std::mem::take(&mut macro_body),
                    });
                }

                let cmds = parse_word(word)?;
                commands.extend(cmds);
            }
        }
    }

    // Flush any remaining macro
    if let Some(name) = macro_name.take() {
        commands.push(GerberCommand::MacroDefine {
            name,
            body: macro_body,
        });
    }

    Ok(commands)
}

/// Parse an extended command (content between % delimiters).
fn parse_extended(content: &str) -> Result<Option<GerberCommand>, ExtractError> {
    if content.starts_with("FS") {
        return Ok(Some(parse_format_spec(content)?));
    }
    if content == "MOMM" {
        return Ok(Some(GerberCommand::Units(Units::Millimeters)));
    }
    if content == "MOIN" {
        return Ok(Some(GerberCommand::Units(Units::Inches)));
    }
    if content.starts_with("AD") {
        return Ok(Some(parse_aperture_define(content)?));
    }
    if content == "LPD" {
        return Ok(Some(GerberCommand::Polarity(Polarity::Dark)));
    }
    if content == "LPC" {
        return Ok(Some(GerberCommand::Polarity(Polarity::Clear)));
    }
    if content.starts_with("TF.FileFunction,") {
        return Ok(Some(parse_file_function(content)?));
    }
    if content.starts_with("SR") {
        return Ok(Some(parse_step_repeat(content)?));
    }
    if content.starts_with("MI") {
        return Ok(Some(parse_image_mirror(content)?));
    }
    if content.starts_with("SF") {
        return Ok(Some(parse_image_scale(content)?));
    }
    // Skip other extended commands (AM, AB, TF, TA, TD, etc.)
    Ok(None)
}

/// Parse %FS command. Example: `FSLAX24Y24`
fn parse_format_spec(content: &str) -> Result<GerberCommand, ExtractError> {
    // Expected format: FS[LA|LT|TA|TI]X<n><m>Y<n><m>
    let s = &content[2..]; // skip "FS"

    // Skip L/T (zero suppression) and A/I (absolute/incremental) chars
    let s = s.trim_start_matches(['L', 'T', 'A', 'I']);

    let x_pos = s
        .find('X')
        .ok_or_else(|| ExtractError::ParseError("FS: missing X".into()))?;
    let y_pos = s
        .find('Y')
        .ok_or_else(|| ExtractError::ParseError("FS: missing Y".into()))?;

    let x_part = &s[x_pos + 1..y_pos];
    let y_part = &s[y_pos + 1..];

    if x_part.len() < 2 || y_part.len() < 2 {
        return Err(ExtractError::ParseError(format!(
            "FS: invalid format digits: X={x_part} Y={y_part}"
        )));
    }

    let x_integer = x_part[..x_part.len() - 1]
        .parse::<u8>()
        .map_err(|_| ExtractError::ParseError(format!("FS: bad X integer: {x_part}")))?;
    let x_decimal = x_part[x_part.len() - 1..]
        .parse::<u8>()
        .map_err(|_| ExtractError::ParseError(format!("FS: bad X decimal: {x_part}")))?;
    let y_integer = y_part[..y_part.len() - 1]
        .parse::<u8>()
        .map_err(|_| ExtractError::ParseError(format!("FS: bad Y integer: {y_part}")))?;
    let y_decimal = y_part[y_part.len() - 1..]
        .parse::<u8>()
        .map_err(|_| ExtractError::ParseError(format!("FS: bad Y decimal: {y_part}")))?;

    Ok(GerberCommand::FormatSpec(CoordinateFormat {
        x_integer,
        x_decimal,
        y_integer,
        y_decimal,
    }))
}

/// Parse %AD command. Example: `ADD10C,0.020` or `ADD11R,0.040X0.020`
fn parse_aperture_define(content: &str) -> Result<GerberCommand, ExtractError> {
    let s = &content[2..]; // skip "AD"

    // Must start with D followed by aperture code
    if !s.starts_with('D') {
        return Err(ExtractError::ParseError(format!(
            "AD: expected D, got: {s}"
        )));
    }
    let s = &s[1..]; // skip 'D'

    // Find where the code ends and the template type begins
    let type_pos = s
        .find(|c: char| c.is_ascii_alphabetic())
        .ok_or_else(|| ExtractError::ParseError(format!("AD: no template type in: {s}")))?;

    let code: u32 = s[..type_pos]
        .parse()
        .map_err(|_| ExtractError::ParseError(format!("AD: bad aperture code: {s}")))?;

    let rest = &s[type_pos..];
    let template = parse_aperture_template(rest)?;

    Ok(GerberCommand::ApertureDefine { code, template })
}

/// Parse aperture template. Example: `C,0.020` or `R,0.040X0.020`
fn parse_aperture_template(s: &str) -> Result<ApertureTemplate, ExtractError> {
    let (type_char, params_str) = if let Some(comma_pos) = s.find(',') {
        (&s[..comma_pos], &s[comma_pos + 1..])
    } else {
        (s, "")
    };

    let params: Vec<f64> = if params_str.is_empty() {
        Vec::new()
    } else {
        params_str
            .split('X')
            .map(|p| {
                p.parse::<f64>()
                    .map_err(|_| ExtractError::ParseError(format!("AD: bad param: {p}")))
            })
            .collect::<Result<Vec<_>, _>>()?
    };

    match type_char {
        "C" => {
            let diameter = params
                .first()
                .copied()
                .ok_or_else(|| ExtractError::ParseError("AD C: missing diameter".into()))?;
            Ok(ApertureTemplate::Circle { diameter })
        }
        "R" => {
            if params.len() < 2 {
                return Err(ExtractError::ParseError(
                    "AD R: need x_size and y_size".into(),
                ));
            }
            Ok(ApertureTemplate::Rectangle {
                x_size: params[0],
                y_size: params[1],
            })
        }
        "O" => {
            if params.len() < 2 {
                return Err(ExtractError::ParseError(
                    "AD O: need x_size and y_size".into(),
                ));
            }
            Ok(ApertureTemplate::Obround {
                x_size: params[0],
                y_size: params[1],
            })
        }
        "P" => {
            if params.len() < 2 {
                return Err(ExtractError::ParseError(
                    "AD P: need diameter and num_vertices".into(),
                ));
            }
            Ok(ApertureTemplate::Polygon {
                outer_diameter: params[0],
                num_vertices: params[1] as u32,
                rotation: params.get(2).copied().unwrap_or(0.0),
            })
        }
        _ => {
            // Aperture macro reference: type_char is the macro name, params are passed through
            Ok(ApertureTemplate::Macro {
                name: type_char.to_string(),
                params,
            })
        }
    }
}

/// Parse %TF.FileFunction command.
fn parse_file_function(content: &str) -> Result<GerberCommand, ExtractError> {
    let parts: Vec<&str> = content
        .strip_prefix("TF.FileFunction,")
        .unwrap_or("")
        .split(',')
        .collect();

    let func = match parts.first().copied() {
        Some("Copper") => {
            let layer_num = parts
                .get(1)
                .and_then(|s| s.strip_prefix('L'))
                .and_then(|s| s.parse::<u32>().ok())
                .unwrap_or(1);
            let side = match parts.get(2).copied() {
                Some("Top") => CopperSide::Top,
                Some("Bot") | Some("Bottom") => CopperSide::Bottom,
                Some("Inr") | Some("Inner") => CopperSide::Inner,
                _ => {
                    if layer_num == 1 {
                        CopperSide::Top
                    } else {
                        CopperSide::Inner
                    }
                }
            };
            FileFunction::Copper { layer_num, side }
        }
        Some("Legend") => {
            let side = parse_board_side(parts.get(1).copied());
            FileFunction::Legend { side }
        }
        Some("Soldermask") => {
            let side = parse_board_side(parts.get(1).copied());
            FileFunction::SolderMask { side }
        }
        Some("Paste") => {
            let side = parse_board_side(parts.get(1).copied());
            FileFunction::Paste { side }
        }
        Some("Profile") => FileFunction::Profile,
        Some(other) => FileFunction::Other(other.to_string()),
        None => FileFunction::Other(String::new()),
    };

    Ok(GerberCommand::FileFunction(func))
}

fn parse_board_side(s: Option<&str>) -> BoardSide {
    match s {
        Some("Bot") | Some("Bottom") => BoardSide::Bottom,
        _ => BoardSide::Top,
    }
}

/// Parse %SR command.  Example: `SRX3Y2I5.0J10.0` or bare `SR` (close/reset).
fn parse_step_repeat(content: &str) -> Result<GerberCommand, ExtractError> {
    let s = &content[2..]; // skip "SR"
    if s.is_empty() {
        // Bare %SR% — closes the current block (or resets to defaults).
        return Ok(GerberCommand::StepRepeat {
            x_repeat: 1,
            y_repeat: 1,
            x_step: 0.0,
            y_step: 0.0,
        });
    }
    // Parse X<n>Y<n>I<f>J<f> — all fields optional, defaults are 1/1/0/0.
    let x_repeat = parse_sr_uint(s, 'X').unwrap_or(1);
    let y_repeat = parse_sr_uint(s, 'Y').unwrap_or(1);
    let x_step = parse_sr_float(s, 'I').unwrap_or(0.0);
    let y_step = parse_sr_float(s, 'J').unwrap_or(0.0);
    Ok(GerberCommand::StepRepeat {
        x_repeat,
        y_repeat,
        x_step,
        y_step,
    })
}

/// Extract the unsigned integer after a given key letter in a SR parameter string.
fn parse_sr_uint(s: &str, key: char) -> Option<u32> {
    let pos = s.find(key)?;
    let after = &s[pos + 1..];
    let end = after
        .find(|c: char| c.is_alphabetic())
        .unwrap_or(after.len());
    after[..end].parse().ok()
}

/// Extract the float after a given key letter in a SR parameter string.
fn parse_sr_float(s: &str, key: char) -> Option<f64> {
    let pos = s.find(key)?;
    let after = &s[pos + 1..];
    let end = after
        .find(|c: char| c.is_alphabetic())
        .unwrap_or(after.len());
    after[..end].parse().ok()
}

/// Parse %MI command.  Example: `MIA1B0` (mirror X only).
fn parse_image_mirror(content: &str) -> Result<GerberCommand, ExtractError> {
    let s = &content[2..]; // skip "MI"
    let a = s
        .find('A')
        .and_then(|p| s[p + 1..].chars().next())
        .map(|c| c == '1')
        .unwrap_or(false);
    let b = s
        .find('B')
        .and_then(|p| s[p + 1..].chars().next())
        .map(|c| c == '1')
        .unwrap_or(false);
    Ok(GerberCommand::ImageMirror { a, b })
}

/// Parse %SF command.  Example: `SFA1.5B2.0`.
fn parse_image_scale(content: &str) -> Result<GerberCommand, ExtractError> {
    let s = &content[2..]; // skip "SF"
    let a = parse_ab_float(s, 'A').unwrap_or(1.0);
    let b = parse_ab_float(s, 'B').unwrap_or(1.0);
    Ok(GerberCommand::ImageScale { a, b })
}

/// Extract the float value after a given letter key in a "A<val>B<val>" string.
fn parse_ab_float(s: &str, key: char) -> Option<f64> {
    let pos = s.find(key)?;
    let after = &s[pos + 1..];
    let end = after
        .find(|c: char| c.is_alphabetic())
        .unwrap_or(after.len());
    after[..end].parse().ok()
}

/// Parse a word command (e.g., "D10", "X100Y200D01", "G01", "M02").
///
/// A single word may contain an embedded G-code prefix (e.g., "G01X100Y200D01").
fn parse_word(word: &str) -> Result<Vec<GerberCommand>, ExtractError> {
    let mut commands = Vec::new();
    let mut remaining = word;

    // Handle leading G-code if present
    if remaining.starts_with('G') || remaining.starts_with('g') {
        let g_end = remaining[1..]
            .find(|c: char| !c.is_ascii_digit())
            .map(|i| i + 1)
            .unwrap_or(remaining.len());
        let g_code = &remaining[..g_end];
        if let Some(cmd) = parse_g_code(g_code) {
            commands.push(cmd);
        }
        remaining = &remaining[g_end..];
        if remaining.is_empty() {
            return Ok(commands);
        }
    }

    // Handle M-code
    if remaining.starts_with('M') || remaining.starts_with('m') {
        let code = remaining[1..].parse::<u32>().unwrap_or(0);
        if code == 2 {
            commands.push(GerberCommand::EndOfFile);
        }
        return Ok(commands);
    }

    // Parse coordinate/D-code word: optional X, Y, I, J values followed by D code
    let mut x: Option<i64> = None;
    let mut y: Option<i64> = None;
    let mut i: Option<i64> = None;
    let mut j: Option<i64> = None;
    let mut d_code: Option<u32> = None;

    let s = remaining;
    let mut pos = 0;
    let bytes = s.as_bytes();

    while pos < bytes.len() {
        let key = bytes[pos] as char;
        pos += 1;

        match key.to_ascii_uppercase() {
            'X' | 'Y' | 'I' | 'J' => {
                let start = pos;
                // Read optional sign and digits
                if pos < bytes.len() && (bytes[pos] == b'+' || bytes[pos] == b'-') {
                    pos += 1;
                }
                while pos < bytes.len() && bytes[pos].is_ascii_digit() {
                    pos += 1;
                }
                let val: i64 = s[start..pos]
                    .parse()
                    .map_err(|_| ExtractError::ParseError(format!("bad coord in: {word}")))?;
                match key.to_ascii_uppercase() {
                    'X' => x = Some(val),
                    'Y' => y = Some(val),
                    'I' => i = Some(val),
                    'J' => j = Some(val),
                    _ => unreachable!(),
                }
            }
            'D' | 'd' => {
                let start = pos;
                while pos < bytes.len() && bytes[pos].is_ascii_digit() {
                    pos += 1;
                }
                d_code = Some(
                    s[start..pos]
                        .parse()
                        .map_err(|_| ExtractError::ParseError(format!("bad D-code in: {word}")))?,
                );
            }
            _ => {
                // Skip unknown characters
            }
        }
    }

    // Emit command based on D-code
    match d_code {
        Some(1) => commands.push(GerberCommand::Interpolate { x, y, i, j }),
        Some(2) => commands.push(GerberCommand::Move { x, y }),
        Some(3) => commands.push(GerberCommand::Flash { x, y }),
        Some(code) if code >= 10 => commands.push(GerberCommand::SelectAperture(code)),
        _ => {
            // Bare coordinates without D-code: treat as D01 (interpolate)
            // per Gerber spec, the D-code from the previous command persists
            if x.is_some() || y.is_some() {
                commands.push(GerberCommand::Interpolate { x, y, i, j });
            }
        }
    }

    Ok(commands)
}

/// Parse a G-code string.
fn parse_g_code(s: &str) -> Option<GerberCommand> {
    let code: u32 = s[1..].parse().ok()?;
    match code {
        1 => Some(GerberCommand::LinearMode),
        2 => Some(GerberCommand::ClockwiseArcMode),
        3 => Some(GerberCommand::CounterClockwiseArcMode),
        36 => Some(GerberCommand::RegionBegin),
        37 => Some(GerberCommand::RegionEnd),
        74 => Some(GerberCommand::SingleQuadrant),
        75 => Some(GerberCommand::MultiQuadrant),
        _ => None, // G01, G54, G70, G71, etc. — deprecated or handled elsewhere
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parsers::gerber::lexer::tokenize;

    fn parse(input: &str) -> Vec<GerberCommand> {
        let tokens = tokenize(input);
        parse_commands(&tokens).unwrap()
    }

    #[test]
    fn test_format_spec() {
        let cmds = parse("%FSLAX24Y24*%\n");
        assert_eq!(cmds.len(), 1);
        match &cmds[0] {
            GerberCommand::FormatSpec(fmt) => {
                assert_eq!(fmt.x_integer, 2);
                assert_eq!(fmt.x_decimal, 4);
                assert_eq!(fmt.y_integer, 2);
                assert_eq!(fmt.y_decimal, 4);
            }
            other => panic!("expected FormatSpec, got: {other:?}"),
        }
    }

    #[test]
    fn test_format_spec_35() {
        let cmds = parse("%FSLAX35Y35*%\n");
        match &cmds[0] {
            GerberCommand::FormatSpec(fmt) => {
                assert_eq!(fmt.x_integer, 3);
                assert_eq!(fmt.x_decimal, 5);
            }
            other => panic!("expected FormatSpec, got: {other:?}"),
        }
    }

    #[test]
    fn test_units() {
        assert_eq!(
            parse("%MOMM*%\n"),
            vec![GerberCommand::Units(Units::Millimeters)]
        );
        assert_eq!(
            parse("%MOIN*%\n"),
            vec![GerberCommand::Units(Units::Inches)]
        );
    }

    #[test]
    fn test_aperture_define_circle() {
        let cmds = parse("%ADD10C,0.020*%\n");
        assert_eq!(
            cmds,
            vec![GerberCommand::ApertureDefine {
                code: 10,
                template: ApertureTemplate::Circle { diameter: 0.020 },
            }]
        );
    }

    #[test]
    fn test_aperture_define_rectangle() {
        let cmds = parse("%ADD11R,0.040X0.020*%\n");
        assert_eq!(
            cmds,
            vec![GerberCommand::ApertureDefine {
                code: 11,
                template: ApertureTemplate::Rectangle {
                    x_size: 0.040,
                    y_size: 0.020,
                },
            }]
        );
    }

    #[test]
    fn test_aperture_define_obround() {
        let cmds = parse("%ADD12O,0.050X0.030*%\n");
        assert_eq!(
            cmds,
            vec![GerberCommand::ApertureDefine {
                code: 12,
                template: ApertureTemplate::Obround {
                    x_size: 0.050,
                    y_size: 0.030,
                },
            }]
        );
    }

    #[test]
    fn test_aperture_define_polygon() {
        let cmds = parse("%ADD13P,0.080X6*%\n");
        assert_eq!(
            cmds,
            vec![GerberCommand::ApertureDefine {
                code: 13,
                template: ApertureTemplate::Polygon {
                    outer_diameter: 0.080,
                    num_vertices: 6,
                    rotation: 0.0,
                },
            }]
        );
    }

    #[test]
    fn test_select_aperture() {
        let cmds = parse("D10*\n");
        assert_eq!(cmds, vec![GerberCommand::SelectAperture(10)]);
    }

    #[test]
    fn test_interpolate() {
        let cmds = parse("X100Y200D01*\n");
        assert_eq!(
            cmds,
            vec![GerberCommand::Interpolate {
                x: Some(100),
                y: Some(200),
                i: None,
                j: None,
            }]
        );
    }

    #[test]
    fn test_move() {
        let cmds = parse("X100Y200D02*\n");
        assert_eq!(
            cmds,
            vec![GerberCommand::Move {
                x: Some(100),
                y: Some(200),
            }]
        );
    }

    #[test]
    fn test_flash() {
        let cmds = parse("X100Y200D03*\n");
        assert_eq!(
            cmds,
            vec![GerberCommand::Flash {
                x: Some(100),
                y: Some(200),
            }]
        );
    }

    #[test]
    fn test_g_codes() {
        assert_eq!(parse("G01*\n"), vec![GerberCommand::LinearMode]);
        assert_eq!(parse("G02*\n"), vec![GerberCommand::ClockwiseArcMode]);
        assert_eq!(
            parse("G03*\n"),
            vec![GerberCommand::CounterClockwiseArcMode]
        );
        assert_eq!(parse("G36*\n"), vec![GerberCommand::RegionBegin]);
        assert_eq!(parse("G37*\n"), vec![GerberCommand::RegionEnd]);
        assert_eq!(parse("G74*\n"), vec![GerberCommand::SingleQuadrant]);
        assert_eq!(parse("G75*\n"), vec![GerberCommand::MultiQuadrant]);
    }

    #[test]
    fn test_end_of_file() {
        assert_eq!(parse("M02*\n"), vec![GerberCommand::EndOfFile]);
    }

    #[test]
    fn test_polarity() {
        assert_eq!(
            parse("%LPD*%\n"),
            vec![GerberCommand::Polarity(Polarity::Dark)]
        );
        assert_eq!(
            parse("%LPC*%\n"),
            vec![GerberCommand::Polarity(Polarity::Clear)]
        );
    }

    #[test]
    fn test_file_function_copper_top() {
        let cmds = parse("%TF.FileFunction,Copper,L1,Top*%\n");
        assert_eq!(
            cmds,
            vec![GerberCommand::FileFunction(FileFunction::Copper {
                layer_num: 1,
                side: CopperSide::Top,
            })]
        );
    }

    #[test]
    fn test_file_function_legend_bottom() {
        let cmds = parse("%TF.FileFunction,Legend,Bot*%\n");
        assert_eq!(
            cmds,
            vec![GerberCommand::FileFunction(FileFunction::Legend {
                side: BoardSide::Bottom,
            })]
        );
    }

    #[test]
    fn test_file_function_profile() {
        let cmds = parse("%TF.FileFunction,Profile*%\n");
        assert_eq!(
            cmds,
            vec![GerberCommand::FileFunction(FileFunction::Profile)]
        );
    }

    #[test]
    fn test_negative_coords() {
        let cmds = parse("X-100Y-200D01*\n");
        assert_eq!(
            cmds,
            vec![GerberCommand::Interpolate {
                x: Some(-100),
                y: Some(-200),
                i: None,
                j: None,
            }]
        );
    }

    #[test]
    fn test_arc_with_ij() {
        let cmds = parse("X200Y100I50J-30D01*\n");
        assert_eq!(
            cmds,
            vec![GerberCommand::Interpolate {
                x: Some(200),
                y: Some(100),
                i: Some(50),
                j: Some(-30),
            }]
        );
    }

    #[test]
    fn test_g_code_with_coords() {
        // G01X100Y200D01 — G-code prefix followed by coordinates and D-code
        let cmds = parse("G01X100Y200D01*\n");
        assert_eq!(
            cmds,
            vec![
                GerberCommand::LinearMode,
                GerberCommand::Interpolate {
                    x: Some(100),
                    y: Some(200),
                    i: None,
                    j: None,
                },
            ]
        );
    }

    #[test]
    fn test_macro_define() {
        let cmds = parse("%AMOC8*5,1,8,0,0,1.08239X$1,22.5*%\n");
        assert_eq!(cmds.len(), 1);
        match &cmds[0] {
            GerberCommand::MacroDefine { name, body } => {
                assert_eq!(name, "OC8");
                assert_eq!(body.len(), 1);
                assert_eq!(body[0], "5,1,8,0,0,1.08239X$1,22.5");
            }
            other => panic!("expected MacroDefine, got: {other:?}"),
        }
    }

    #[test]
    fn test_macro_ad_reference() {
        let cmds = parse("%ADD22OC8,0.1*%\n");
        assert_eq!(cmds.len(), 1);
        match &cmds[0] {
            GerberCommand::ApertureDefine { code, template } => {
                assert_eq!(*code, 22);
                match template {
                    ApertureTemplate::Macro { name, params } => {
                        assert_eq!(name, "OC8");
                        assert_eq!(params.len(), 1);
                        assert!((params[0] - 0.1).abs() < 1e-9);
                    }
                    other => panic!("expected Macro template, got: {other:?}"),
                }
            }
            other => panic!("expected ApertureDefine, got: {other:?}"),
        }
    }

    #[test]
    fn test_macro_multi_line() {
        // Macro with multiple primitives
        let cmds = parse("%AMTEST*1,1,0.5,0,0*21,1,0.3,0.1,0,0,0*%\n");
        assert_eq!(cmds.len(), 1);
        match &cmds[0] {
            GerberCommand::MacroDefine { name, body } => {
                assert_eq!(name, "TEST");
                assert_eq!(body.len(), 2);
            }
            other => panic!("expected MacroDefine, got: {other:?}"),
        }
    }

    #[test]
    fn test_step_repeat_parse() {
        let cmds = parse("%SRX3Y2I5.0J10.0*%\n");
        assert_eq!(
            cmds,
            vec![GerberCommand::StepRepeat {
                x_repeat: 3,
                y_repeat: 2,
                x_step: 5.0,
                y_step: 10.0,
            }]
        );
        // Bare %SR% = close/reset
        let cmds = parse("%SR*%\n");
        assert_eq!(
            cmds,
            vec![GerberCommand::StepRepeat {
                x_repeat: 1,
                y_repeat: 1,
                x_step: 0.0,
                y_step: 0.0,
            }]
        );
    }

    #[test]
    fn test_image_mirror() {
        let cmds = parse("%MIA1B0*%\n");
        assert_eq!(cmds, vec![GerberCommand::ImageMirror { a: true, b: false }]);
        let cmds = parse("%MIA0B1*%\n");
        assert_eq!(cmds, vec![GerberCommand::ImageMirror { a: false, b: true }]);
        let cmds = parse("%MIA0B0*%\n");
        assert_eq!(
            cmds,
            vec![GerberCommand::ImageMirror { a: false, b: false }]
        );
    }

    #[test]
    fn test_image_scale() {
        let cmds = parse("%SFA2.0B1.5*%\n");
        assert_eq!(cmds.len(), 1);
        match &cmds[0] {
            GerberCommand::ImageScale { a, b } => {
                assert!((*a - 2.0).abs() < 1e-9);
                assert!((*b - 1.5).abs() < 1e-9);
            }
            other => panic!("expected ImageScale, got: {other:?}"),
        }
        // Default scale (no-op)
        let cmds = parse("%SFA1.0B1.0*%\n");
        assert_eq!(cmds, vec![GerberCommand::ImageScale { a: 1.0, b: 1.0 }]);
    }
}
