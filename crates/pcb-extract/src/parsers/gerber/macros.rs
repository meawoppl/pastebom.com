use std::collections::HashMap;
use std::f64::consts::PI;

use crate::error::ExtractError;
use crate::types::Drawing;

/// A single primitive within an aperture macro definition.
#[derive(Debug, Clone, PartialEq)]
pub enum MacroPrimitive {
    /// Code 0: Comment (ignored during evaluation)
    Comment,
    /// Code 1: Circle
    Circle {
        exposure: Expr,
        diameter: Expr,
        center_x: Expr,
        center_y: Expr,
        rotation: Option<Expr>,
    },
    /// Code 20 (or 2): Vector line
    VectorLine {
        exposure: Expr,
        width: Expr,
        start_x: Expr,
        start_y: Expr,
        end_x: Expr,
        end_y: Expr,
        rotation: Expr,
    },
    /// Code 21: Center line (rectangle by center)
    CenterLine {
        exposure: Expr,
        width: Expr,
        height: Expr,
        center_x: Expr,
        center_y: Expr,
        rotation: Expr,
    },
    /// Code 4: Outline (arbitrary polygon)
    Outline {
        exposure: Expr,
        num_points: Expr,
        points: Vec<Expr>, // pairs of (x, y) coordinates
        rotation: Expr,
    },
    /// Code 5: Regular polygon
    Polygon {
        exposure: Expr,
        num_vertices: Expr,
        center_x: Expr,
        center_y: Expr,
        diameter: Expr,
        rotation: Expr,
    },
    /// Code 7: Thermal (ring with gaps)
    Thermal {
        center_x: Expr,
        center_y: Expr,
        outer_diameter: Expr,
        inner_diameter: Expr,
        gap_thickness: Expr,
        rotation: Expr,
    },
}

/// Expression node for macro parameter evaluation.
/// Supports: literals, variable references ($1, $2, ...), and arithmetic.
#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Literal(f64),
    Variable(u32), // $1 = Variable(1)
    Add(Box<Expr>, Box<Expr>),
    Sub(Box<Expr>, Box<Expr>),
    Mul(Box<Expr>, Box<Expr>),
    Div(Box<Expr>, Box<Expr>),
}

impl Expr {
    /// Evaluate the expression with the given parameter bindings.
    pub fn eval(&self, params: &[f64]) -> f64 {
        match self {
            Expr::Literal(v) => *v,
            Expr::Variable(idx) => {
                if *idx == 0 || *idx as usize > params.len() {
                    0.0
                } else {
                    params[*idx as usize - 1]
                }
            }
            Expr::Add(a, b) => a.eval(params) + b.eval(params),
            Expr::Sub(a, b) => a.eval(params) - b.eval(params),
            Expr::Mul(a, b) => a.eval(params) * b.eval(params),
            Expr::Div(a, b) => {
                let denom = b.eval(params);
                if denom.abs() < 1e-15 {
                    0.0
                } else {
                    a.eval(params) / denom
                }
            }
        }
    }
}

/// An aperture macro definition (from %AM...% blocks).
#[derive(Debug, Clone)]
pub struct ApertureMacro {
    pub name: String,
    pub primitives: Vec<MacroPrimitive>,
}

/// Table of macro definitions, keyed by name.
#[derive(Debug, Default)]
pub struct MacroTable {
    macros: HashMap<String, ApertureMacro>,
}

impl MacroTable {
    pub fn define(&mut self, name: String, mac: ApertureMacro) {
        self.macros.insert(name, mac);
    }

    pub fn get(&self, name: &str) -> Option<&ApertureMacro> {
        self.macros.get(name)
    }
}

// ─── Expression Parser ──────────────────────────────────────────────

/// Parse a Gerber macro expression string into an Expr tree.
/// Gerber uses 'x' or 'X' for multiplication (not '*' which is the statement terminator).
pub fn parse_expr(s: &str) -> Result<Expr, ExtractError> {
    let s = s.trim();
    if s.is_empty() {
        return Ok(Expr::Literal(0.0));
    }
    let tokens = tokenize_expr(s)?;
    let (expr, rest) = parse_add_sub(&tokens)?;
    if !rest.is_empty() {
        return Err(ExtractError::ParseError(format!(
            "AM expr: unexpected tokens after expression: {s}"
        )));
    }
    Ok(expr)
}

#[derive(Debug, Clone)]
enum ExprToken {
    Num(f64),
    Var(u32),
    Plus,
    Minus,
    Mul,
    Div,
    LParen,
    RParen,
}

fn tokenize_expr(s: &str) -> Result<Vec<ExprToken>, ExtractError> {
    let mut tokens = Vec::new();
    let mut chars = s.chars().peekable();

    while let Some(&ch) = chars.peek() {
        match ch {
            ' ' | '\t' => {
                chars.next();
            }
            '+' => {
                chars.next();
                tokens.push(ExprToken::Plus);
            }
            '-' => {
                chars.next();
                // Negative number if preceded by operator or at start
                let is_unary = matches!(
                    tokens.last(),
                    None | Some(ExprToken::Plus)
                        | Some(ExprToken::Minus)
                        | Some(ExprToken::Mul)
                        | Some(ExprToken::Div)
                        | Some(ExprToken::LParen)
                );
                if is_unary
                    && chars
                        .peek()
                        .is_some_and(|c| c.is_ascii_digit() || *c == '.')
                {
                    let mut num_str = String::from('-');
                    while chars
                        .peek()
                        .is_some_and(|c| c.is_ascii_digit() || *c == '.')
                    {
                        num_str.push(chars.next().unwrap());
                    }
                    let val: f64 = num_str.parse().map_err(|_| {
                        ExtractError::ParseError(format!("AM expr: bad number: {num_str}"))
                    })?;
                    tokens.push(ExprToken::Num(val));
                } else {
                    tokens.push(ExprToken::Minus);
                }
            }
            'x' | 'X' => {
                chars.next();
                tokens.push(ExprToken::Mul);
            }
            '/' => {
                chars.next();
                tokens.push(ExprToken::Div);
            }
            '(' => {
                chars.next();
                tokens.push(ExprToken::LParen);
            }
            ')' => {
                chars.next();
                tokens.push(ExprToken::RParen);
            }
            '$' => {
                chars.next(); // consume '$'
                let mut num_str = String::new();
                while chars.peek().is_some_and(|c| c.is_ascii_digit()) {
                    num_str.push(chars.next().unwrap());
                }
                let idx: u32 = num_str.parse().map_err(|_| {
                    ExtractError::ParseError(format!("AM expr: bad variable: ${num_str}"))
                })?;
                tokens.push(ExprToken::Var(idx));
            }
            c if c.is_ascii_digit() || c == '.' => {
                let mut num_str = String::new();
                while chars
                    .peek()
                    .is_some_and(|c| c.is_ascii_digit() || *c == '.')
                {
                    num_str.push(chars.next().unwrap());
                }
                let val: f64 = num_str.parse().map_err(|_| {
                    ExtractError::ParseError(format!("AM expr: bad number: {num_str}"))
                })?;
                tokens.push(ExprToken::Num(val));
            }
            _ => {
                return Err(ExtractError::ParseError(format!(
                    "AM expr: unexpected char '{ch}' in: {s}"
                )));
            }
        }
    }

    Ok(tokens)
}

// Recursive descent: add/sub -> mul/div -> atom
fn parse_add_sub(tokens: &[ExprToken]) -> Result<(Expr, &[ExprToken]), ExtractError> {
    let (mut left, mut rest) = parse_mul_div(tokens)?;
    loop {
        match rest.first() {
            Some(ExprToken::Plus) => {
                let (right, r) = parse_mul_div(&rest[1..])?;
                left = Expr::Add(Box::new(left), Box::new(right));
                rest = r;
            }
            Some(ExprToken::Minus) => {
                let (right, r) = parse_mul_div(&rest[1..])?;
                left = Expr::Sub(Box::new(left), Box::new(right));
                rest = r;
            }
            _ => break,
        }
    }
    Ok((left, rest))
}

fn parse_mul_div(tokens: &[ExprToken]) -> Result<(Expr, &[ExprToken]), ExtractError> {
    let (mut left, mut rest) = parse_atom(tokens)?;
    loop {
        match rest.first() {
            Some(ExprToken::Mul) => {
                let (right, r) = parse_atom(&rest[1..])?;
                left = Expr::Mul(Box::new(left), Box::new(right));
                rest = r;
            }
            Some(ExprToken::Div) => {
                let (right, r) = parse_atom(&rest[1..])?;
                left = Expr::Div(Box::new(left), Box::new(right));
                rest = r;
            }
            _ => break,
        }
    }
    Ok((left, rest))
}

fn parse_atom(tokens: &[ExprToken]) -> Result<(Expr, &[ExprToken]), ExtractError> {
    match tokens.first() {
        Some(ExprToken::Num(v)) => Ok((Expr::Literal(*v), &tokens[1..])),
        Some(ExprToken::Var(idx)) => Ok((Expr::Variable(*idx), &tokens[1..])),
        Some(ExprToken::LParen) => {
            let (expr, rest) = parse_add_sub(&tokens[1..])?;
            match rest.first() {
                Some(ExprToken::RParen) => Ok((expr, &rest[1..])),
                _ => Err(ExtractError::ParseError(
                    "AM expr: missing closing paren".into(),
                )),
            }
        }
        _ => Err(ExtractError::ParseError(
            "AM expr: unexpected end of expression".into(),
        )),
    }
}

// ─── Macro Primitive Parser ─────────────────────────────────────────

/// Parse the body lines of an aperture macro into primitives.
/// Each line is a comma-separated list like "5,1,8,0,0,1.08239X$1,22.5"
pub fn parse_macro_body(lines: &[String]) -> Result<Vec<MacroPrimitive>, ExtractError> {
    let mut primitives = Vec::new();

    for line in lines {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        // Comment lines start with "0 "
        if trimmed.starts_with("0 ") || trimmed == "0" {
            primitives.push(MacroPrimitive::Comment);
            continue;
        }

        let parts: Vec<&str> = trimmed.split(',').collect();
        if parts.is_empty() {
            continue;
        }

        let code: u32 = parts[0].trim().parse().map_err(|_| {
            ExtractError::ParseError(format!("AM: bad primitive code: {}", parts[0]))
        })?;

        let exprs: Vec<Expr> = parts[1..]
            .iter()
            .map(|p| parse_expr(p))
            .collect::<Result<Vec<_>, _>>()?;

        let prim = match code {
            1 => {
                // Circle: exposure, diameter, center_x, center_y [, rotation]
                if exprs.len() < 4 {
                    return Err(ExtractError::ParseError(
                        "AM circle: need at least 4 params".into(),
                    ));
                }
                MacroPrimitive::Circle {
                    exposure: exprs[0].clone(),
                    diameter: exprs[1].clone(),
                    center_x: exprs[2].clone(),
                    center_y: exprs[3].clone(),
                    rotation: exprs.get(4).cloned(),
                }
            }
            2 | 20 => {
                // Vector line: exposure, width, start_x, start_y, end_x, end_y, rotation
                if exprs.len() < 7 {
                    return Err(ExtractError::ParseError(
                        "AM vector line: need 7 params".into(),
                    ));
                }
                MacroPrimitive::VectorLine {
                    exposure: exprs[0].clone(),
                    width: exprs[1].clone(),
                    start_x: exprs[2].clone(),
                    start_y: exprs[3].clone(),
                    end_x: exprs[4].clone(),
                    end_y: exprs[5].clone(),
                    rotation: exprs[6].clone(),
                }
            }
            21 => {
                // Center line: exposure, width, height, center_x, center_y, rotation
                if exprs.len() < 6 {
                    return Err(ExtractError::ParseError(
                        "AM center line: need 6 params".into(),
                    ));
                }
                MacroPrimitive::CenterLine {
                    exposure: exprs[0].clone(),
                    width: exprs[1].clone(),
                    height: exprs[2].clone(),
                    center_x: exprs[3].clone(),
                    center_y: exprs[4].clone(),
                    rotation: exprs[5].clone(),
                }
            }
            4 => {
                // Outline: exposure, n_vertices, x0, y0, x1, y1, ..., rotation
                if exprs.len() < 2 {
                    return Err(ExtractError::ParseError(
                        "AM outline: need at least 2 params".into(),
                    ));
                }
                // The points and rotation are all in exprs[2..].
                // We can't know the exact count until evaluation, so store them all.
                MacroPrimitive::Outline {
                    exposure: exprs[0].clone(),
                    num_points: exprs[1].clone(),
                    points: exprs[2..].to_vec(),
                    rotation: exprs.last().cloned().unwrap_or(Expr::Literal(0.0)),
                }
            }
            5 => {
                // Polygon: exposure, n_vertices, center_x, center_y, diameter, rotation
                if exprs.len() < 6 {
                    return Err(ExtractError::ParseError("AM polygon: need 6 params".into()));
                }
                MacroPrimitive::Polygon {
                    exposure: exprs[0].clone(),
                    num_vertices: exprs[1].clone(),
                    center_x: exprs[2].clone(),
                    center_y: exprs[3].clone(),
                    diameter: exprs[4].clone(),
                    rotation: exprs[5].clone(),
                }
            }
            7 => {
                // Thermal: center_x, center_y, outer_d, inner_d, gap, rotation
                if exprs.len() < 6 {
                    return Err(ExtractError::ParseError("AM thermal: need 6 params".into()));
                }
                MacroPrimitive::Thermal {
                    center_x: exprs[0].clone(),
                    center_y: exprs[1].clone(),
                    outer_diameter: exprs[2].clone(),
                    inner_diameter: exprs[3].clone(),
                    gap_thickness: exprs[4].clone(),
                    rotation: exprs[5].clone(),
                }
            }
            _ => {
                // Unknown primitive code — skip
                continue;
            }
        };

        primitives.push(prim);
    }

    Ok(primitives)
}

// ─── Macro Evaluation (flash-time) ──────────────────────────────────

/// Evaluate an aperture macro at a given flash position, producing Drawing primitives.
pub fn evaluate_macro(
    mac: &ApertureMacro,
    params: &[f64],
    flash_x: f64,
    flash_y: f64,
) -> Vec<Drawing> {
    let mut drawings = Vec::new();

    for prim in &mac.primitives {
        match prim {
            MacroPrimitive::Comment => {}
            MacroPrimitive::Circle {
                exposure,
                diameter,
                center_x,
                center_y,
                rotation,
            } => {
                let exp = exposure.eval(params);
                if exp < 0.5 {
                    continue; // clear exposure — skip for now
                }
                let d = diameter.eval(params);
                let cx = center_x.eval(params);
                let cy = center_y.eval(params);
                let rot = rotation.as_ref().map(|r| r.eval(params)).unwrap_or(0.0);

                let (rx, ry) = rotate_point(cx, cy, rot);
                drawings.push(Drawing::Circle {
                    start: [flash_x + rx, flash_y + ry],
                    radius: d.abs() / 2.0,
                    width: 0.0,
                    filled: Some(1),
                });
            }
            MacroPrimitive::VectorLine {
                exposure,
                width,
                start_x,
                start_y,
                end_x,
                end_y,
                rotation,
            } => {
                let exp = exposure.eval(params);
                if exp < 0.5 {
                    continue;
                }
                let w = width.eval(params);
                let sx = start_x.eval(params);
                let sy = start_y.eval(params);
                let ex = end_x.eval(params);
                let ey = end_y.eval(params);
                let rot = rotation.eval(params);

                let (rsx, rsy) = rotate_point(sx, sy, rot);
                let (rex, rey) = rotate_point(ex, ey, rot);
                drawings.push(Drawing::Segment {
                    start: [flash_x + rsx, flash_y + rsy],
                    end: [flash_x + rex, flash_y + rey],
                    width: w,
                });
            }
            MacroPrimitive::CenterLine {
                exposure,
                width,
                height,
                center_x,
                center_y,
                rotation,
            } => {
                let exp = exposure.eval(params);
                if exp < 0.5 {
                    continue;
                }
                let w = width.eval(params);
                let h = height.eval(params);
                let cx = center_x.eval(params);
                let cy = center_y.eval(params);
                let rot = rotation.eval(params);

                // Build rectangle corners, rotate, then translate
                let hw = w / 2.0;
                let hh = h / 2.0;
                let corners = [
                    (cx - hw, cy - hh),
                    (cx + hw, cy - hh),
                    (cx + hw, cy + hh),
                    (cx - hw, cy + hh),
                ];
                let points: Vec<[f64; 2]> = corners
                    .iter()
                    .map(|&(px, py)| {
                        let (rx, ry) = rotate_point(px, py, rot);
                        [flash_x + rx, flash_y + ry]
                    })
                    .collect();

                drawings.push(Drawing::Polygon {
                    pos: [0.0, 0.0],
                    angle: 0.0,
                    polygons: vec![points],
                    filled: Some(1),
                    width: 0.0,
                });
            }
            MacroPrimitive::Outline {
                exposure,
                num_points,
                points: point_exprs,
                rotation: _,
            } => {
                let exp = exposure.eval(params);
                if exp < 0.5 {
                    continue;
                }
                let n = num_points.eval(params) as usize;
                // point_exprs contains pairs of (x, y) coordinates followed by rotation.
                // Total coordinate values = (n+1) * 2, then rotation is the last element.
                let coord_count = (n + 1) * 2;
                if point_exprs.len() < coord_count + 1 {
                    continue; // malformed
                }

                let rot = point_exprs[coord_count].eval(params);
                let mut pts = Vec::with_capacity(n + 1);
                for k in 0..=n {
                    let px = point_exprs[k * 2].eval(params);
                    let py = point_exprs[k * 2 + 1].eval(params);
                    let (rx, ry) = rotate_point(px, py, rot);
                    pts.push([flash_x + rx, flash_y + ry]);
                }

                drawings.push(Drawing::Polygon {
                    pos: [0.0, 0.0],
                    angle: 0.0,
                    polygons: vec![pts],
                    filled: Some(1),
                    width: 0.0,
                });
            }
            MacroPrimitive::Polygon {
                exposure,
                num_vertices,
                center_x,
                center_y,
                diameter,
                rotation,
            } => {
                let exp = exposure.eval(params);
                if exp < 0.5 {
                    continue;
                }
                let n = num_vertices.eval(params) as usize;
                let cx = center_x.eval(params);
                let cy = center_y.eval(params);
                let d = diameter.eval(params);
                let rot = rotation.eval(params);
                let r = d / 2.0;

                let rot_rad = rot.to_radians();
                let mut pts = Vec::with_capacity(n);
                for k in 0..n {
                    let angle = rot_rad + 2.0 * PI * (k as f64) / (n as f64);
                    let px = cx + r * angle.cos();
                    let py = cy + r * angle.sin();
                    let (rx, ry) = rotate_point(px, py, 0.0); // rotation already in angle
                    pts.push([flash_x + rx, flash_y + ry]);
                }

                drawings.push(Drawing::Polygon {
                    pos: [0.0, 0.0],
                    angle: 0.0,
                    polygons: vec![pts],
                    filled: Some(1),
                    width: 0.0,
                });
            }
            MacroPrimitive::Thermal {
                center_x,
                center_y,
                outer_diameter,
                inner_diameter,
                gap_thickness,
                rotation,
            } => {
                // Thermal: a ring (annulus) with four 90° gap cuts at the rotation angle.
                // Render each of the four solid arc segments as a Drawing::Arc whose
                // stroke width equals the ring thickness — this gives perfectly smooth
                // curves with zero polygon approximation error.
                let cx = center_x.eval(params);
                let cy = center_y.eval(params);
                let od = outer_diameter.eval(params);
                let id = inner_diameter.eval(params);
                let gap = gap_thickness.eval(params);
                let rot = rotation.eval(params);

                let outer_r = od / 2.0;
                let inner_r = id / 2.0;
                let ring_width = outer_r - inner_r;
                let mid_r = (outer_r + inner_r) / 2.0;

                if mid_r < 1e-9 || ring_width < 1e-9 {
                    continue;
                }

                // Half-angle subtended by the gap at the mid-radius.
                // Clamp argument to [-1, 1] to guard against numerical overshoot.
                let gap_half_angle = ((gap / (2.0 * mid_r)).clamp(-1.0, 1.0)).asin();
                let rot_rad = rot.to_radians();

                // Emit one Drawing::Arc per quadrant, each trimmed by the gap.
                for quadrant in 0..4u32 {
                    let base = rot_rad + (quadrant as f64) * PI / 2.0;
                    let arc_start_rad = base + gap_half_angle;
                    let arc_end_rad = base + PI / 2.0 - gap_half_angle;

                    if arc_end_rad <= arc_start_rad {
                        continue;
                    }

                    drawings.push(Drawing::Arc {
                        start: [flash_x + cx, flash_y + cy],
                        radius: mid_r,
                        startangle: arc_start_rad.to_degrees(),
                        endangle: arc_end_rad.to_degrees(),
                        width: ring_width,
                    });
                }
            }
        }
    }

    drawings
}

/// Rotate a point (x, y) around the origin by the given angle in degrees.
fn rotate_point(x: f64, y: f64, angle_deg: f64) -> (f64, f64) {
    if angle_deg.abs() < 1e-9 {
        return (x, y);
    }
    let rad = angle_deg.to_radians();
    let cos_a = rad.cos();
    let sin_a = rad.sin();
    (x * cos_a - y * sin_a, x * sin_a + y * cos_a)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_expr_literal() {
        let expr = parse_expr("42.5").unwrap();
        assert!((expr.eval(&[]) - 42.5).abs() < 1e-9);
    }

    #[test]
    fn test_expr_variable() {
        let expr = parse_expr("$1").unwrap();
        assert!((expr.eval(&[3.0]) - 3.0).abs() < 1e-9);
    }

    #[test]
    fn test_expr_multiply() {
        let expr = parse_expr("1.08239X$1").unwrap();
        assert!((expr.eval(&[0.1]) - 0.108239).abs() < 1e-9);
    }

    #[test]
    fn test_expr_add_sub() {
        let expr = parse_expr("$1+$2-1.0").unwrap();
        assert!((expr.eval(&[3.0, 5.0]) - 7.0).abs() < 1e-9);
    }

    #[test]
    fn test_expr_precedence() {
        // 2 + 3 * 4 = 14, not 20
        let expr = parse_expr("2+3x4").unwrap();
        assert!((expr.eval(&[]) - 14.0).abs() < 1e-9);
    }

    #[test]
    fn test_expr_parentheses() {
        let expr = parse_expr("(2+3)x4").unwrap();
        assert!((expr.eval(&[]) - 20.0).abs() < 1e-9);
    }

    #[test]
    fn test_expr_negative() {
        let expr = parse_expr("-1.5").unwrap();
        assert!((expr.eval(&[]) - (-1.5)).abs() < 1e-9);
    }

    #[test]
    fn test_parse_polygon_primitive() {
        let lines = vec!["5,1,8,0,0,1.08239X$1,22.5".to_string()];
        let prims = parse_macro_body(&lines).unwrap();
        assert_eq!(prims.len(), 1);
        assert!(matches!(prims[0], MacroPrimitive::Polygon { .. }));
    }

    #[test]
    fn test_parse_circle_primitive() {
        let lines = vec!["1,1,0.5,0,0".to_string()];
        let prims = parse_macro_body(&lines).unwrap();
        assert_eq!(prims.len(), 1);
        assert!(matches!(prims[0], MacroPrimitive::Circle { .. }));
    }

    #[test]
    fn test_parse_center_line() {
        let lines = vec!["21,1,0.5,0.3,0,0,0".to_string()];
        let prims = parse_macro_body(&lines).unwrap();
        assert_eq!(prims.len(), 1);
        assert!(matches!(prims[0], MacroPrimitive::CenterLine { .. }));
    }

    #[test]
    fn test_evaluate_circle_macro() {
        let mac = ApertureMacro {
            name: "TEST".to_string(),
            primitives: vec![MacroPrimitive::Circle {
                exposure: Expr::Literal(1.0),
                diameter: Expr::Variable(1),
                center_x: Expr::Literal(0.0),
                center_y: Expr::Literal(0.0),
                rotation: None,
            }],
        };
        let drawings = evaluate_macro(&mac, &[0.5], 10.0, 20.0);
        assert_eq!(drawings.len(), 1);
        match &drawings[0] {
            Drawing::Circle { start, radius, .. } => {
                assert!((start[0] - 10.0).abs() < 1e-6);
                assert!((start[1] - 20.0).abs() < 1e-6);
                assert!((*radius - 0.25).abs() < 1e-6);
            }
            other => panic!("expected Circle, got: {other:?}"),
        }
    }

    #[test]
    fn test_evaluate_polygon_macro() {
        // The OC8 macro from EAGLE
        let mac = ApertureMacro {
            name: "OC8".to_string(),
            primitives: vec![MacroPrimitive::Polygon {
                exposure: Expr::Literal(1.0),
                num_vertices: Expr::Literal(8.0),
                center_x: Expr::Literal(0.0),
                center_y: Expr::Literal(0.0),
                diameter: Expr::Mul(
                    Box::new(Expr::Literal(1.08239)),
                    Box::new(Expr::Variable(1)),
                ),
                rotation: Expr::Literal(22.5),
            }],
        };
        let drawings = evaluate_macro(&mac, &[1.0], 5.0, 5.0);
        assert_eq!(drawings.len(), 1);
        match &drawings[0] {
            Drawing::Polygon { polygons, .. } => {
                assert_eq!(polygons.len(), 1);
                assert_eq!(polygons[0].len(), 8);
            }
            other => panic!("expected Polygon, got: {other:?}"),
        }
    }

    #[test]
    fn test_evaluate_clear_exposure_skipped() {
        let mac = ApertureMacro {
            name: "TEST".to_string(),
            primitives: vec![MacroPrimitive::Circle {
                exposure: Expr::Literal(0.0), // clear
                diameter: Expr::Literal(1.0),
                center_x: Expr::Literal(0.0),
                center_y: Expr::Literal(0.0),
                rotation: None,
            }],
        };
        let drawings = evaluate_macro(&mac, &[], 0.0, 0.0);
        assert!(drawings.is_empty());
    }

    #[test]
    fn test_rotate_point_zero() {
        let (x, y) = rotate_point(1.0, 0.0, 0.0);
        assert!((x - 1.0).abs() < 1e-9);
        assert!(y.abs() < 1e-9);
    }

    #[test]
    fn test_rotate_point_90() {
        let (x, y) = rotate_point(1.0, 0.0, 90.0);
        assert!(x.abs() < 1e-9);
        assert!((y - 1.0).abs() < 1e-9);
    }

    #[test]
    fn test_evaluate_thermal_macro() {
        // Thermal: outer_d=2.0, inner_d=1.0, gap=0.2, rotation=0
        // ring_width = 0.5, mid_r = 0.75
        // gap_half_angle = asin(0.1 / 0.75) ≈ 7.66°
        // Each quadrant arc spans from (0 + 7.66°) to (90 - 7.66°) ≈ 74.7°
        // Four such arcs should be emitted as Drawing::Arc
        let mac = ApertureMacro {
            name: "THERMAL".to_string(),
            primitives: vec![MacroPrimitive::Thermal {
                center_x: Expr::Literal(0.0),
                center_y: Expr::Literal(0.0),
                outer_diameter: Expr::Literal(2.0),
                inner_diameter: Expr::Literal(1.0),
                gap_thickness: Expr::Literal(0.2),
                rotation: Expr::Literal(0.0),
            }],
        };
        let drawings = evaluate_macro(&mac, &[], 0.0, 0.0);
        assert_eq!(drawings.len(), 4, "expected 4 arc segments for thermal");
        for d in &drawings {
            match d {
                Drawing::Arc {
                    start,
                    radius,
                    width,
                    startangle,
                    endangle,
                } => {
                    assert!((*radius - 0.75).abs() < 1e-6, "mid-radius should be 0.75");
                    assert!((*width - 0.5).abs() < 1e-6, "ring width should be 0.5");
                    assert!(start[0].abs() < 1e-9);
                    assert!(start[1].abs() < 1e-9);
                    assert!(*endangle > *startangle, "arc should sweep forward");
                    let span = endangle - startangle;
                    assert!(span < 90.0, "each quadrant arc must be < 90°");
                    assert!(span > 0.0, "arc span must be positive");
                }
                other => panic!("expected Drawing::Arc for thermal, got: {other:?}"),
            }
        }
    }
}
