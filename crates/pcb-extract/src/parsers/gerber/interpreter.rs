use std::f64::consts::PI;

use log::warn;

use crate::error::ExtractError;
use crate::types::Drawing;

use super::apertures::ApertureTable;
use super::commands::{ApertureTemplate, GerberCommand, Polarity};
use super::coord::CoordinateConverter;
use super::macros::{self, MacroTable};

/// Output from interpreting a single Gerber file.
#[derive(Debug, Default)]
pub struct GerberLayerOutput {
    pub drawings: Vec<Drawing>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InterpolationMode {
    Linear,
    ClockwiseArc,
    CounterClockwiseArc,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QuadrantMode {
    Single,
    Multi,
}

/// Gerber state machine. Walks commands and produces Drawing primitives.
struct Interpreter {
    x: i64,
    y: i64,
    aperture: u32,
    interpolation: InterpolationMode,
    quadrant: QuadrantMode,
    region_active: bool,
    region_points: Vec<[f64; 2]>,
    region_contours: Vec<Vec<[f64; 2]>>,
    polarity: Polarity,
    converter: CoordinateConverter,
    apertures: ApertureTable,
    macro_table: MacroTable,
    drawings: Vec<Drawing>,
    /// Step-and-repeat: index into `drawings` where the current SR block started,
    /// plus the repeat counts and steps (in mm) for replication on block close.
    sr_block_start: Option<usize>,
    sr_x_repeat: u32,
    sr_y_repeat: u32,
    sr_x_step: f64,
    sr_y_step: f64,
}

impl Interpreter {
    fn new() -> Self {
        Self {
            x: 0,
            y: 0,
            aperture: 0,
            interpolation: InterpolationMode::Linear,
            quadrant: QuadrantMode::Multi, // Modern default per spec
            region_active: false,
            region_points: Vec::new(),
            region_contours: Vec::new(),
            polarity: Polarity::Dark,
            converter: CoordinateConverter::default(),
            apertures: ApertureTable::default(),
            macro_table: MacroTable::default(),
            drawings: Vec::new(),
            sr_block_start: None,
            sr_x_repeat: 1,
            sr_y_repeat: 1,
            sr_x_step: 0.0,
            sr_y_step: 0.0,
        }
    }

    fn process(&mut self, cmd: &GerberCommand) {
        match cmd {
            GerberCommand::FormatSpec(fmt) => {
                self.converter.format = fmt.clone();
            }
            GerberCommand::Units(units) => {
                self.converter.units = *units;
            }
            GerberCommand::ApertureDefine { code, template } => {
                self.apertures.define(*code, template.clone());
            }
            GerberCommand::SelectAperture(code) => {
                self.aperture = *code;
            }
            GerberCommand::LinearMode => {
                self.interpolation = InterpolationMode::Linear;
            }
            GerberCommand::ClockwiseArcMode => {
                self.interpolation = InterpolationMode::ClockwiseArc;
            }
            GerberCommand::CounterClockwiseArcMode => {
                self.interpolation = InterpolationMode::CounterClockwiseArc;
            }
            GerberCommand::SingleQuadrant => {
                self.quadrant = QuadrantMode::Single;
            }
            GerberCommand::MultiQuadrant => {
                self.quadrant = QuadrantMode::Multi;
            }
            GerberCommand::Polarity(p) => {
                self.polarity = *p;
            }
            GerberCommand::MacroDefine { name, body } => {
                if let Ok(primitives) = macros::parse_macro_body(body) {
                    self.macro_table.define(
                        name.clone(),
                        macros::ApertureMacro {
                            name: name.clone(),
                            primitives,
                        },
                    );
                }
            }
            GerberCommand::RegionBegin => {
                self.region_active = true;
                self.region_points.clear();
                self.region_contours.clear();
            }
            GerberCommand::RegionEnd => {
                self.flush_region_end();
                self.region_active = false;
            }
            GerberCommand::Interpolate { x, y, i, j } => {
                let old_x = self.x;
                let old_y = self.y;
                if let Some(nx) = x {
                    self.x = *nx;
                }
                if let Some(ny) = y {
                    self.y = *ny;
                }
                self.do_interpolate(old_x, old_y, *i, *j);
            }
            GerberCommand::Move { x, y } => {
                // In region mode, a D02 closes the current contour and starts a new one
                if self.region_active && !self.region_points.is_empty() {
                    let points = std::mem::take(&mut self.region_points);
                    if points.len() >= 3 {
                        self.region_contours.push(points);
                    }
                }
                if let Some(nx) = x {
                    self.x = *nx;
                }
                if let Some(ny) = y {
                    self.y = *ny;
                }
                // In region mode, start a new contour at the new position
                if self.region_active {
                    let px = self.converter.to_mm(self.x, true);
                    let py = self.converter.to_mm(self.y, false);
                    self.region_points.push([px, py]);
                }
            }
            GerberCommand::Flash { x, y } => {
                if let Some(nx) = x {
                    self.x = *nx;
                }
                if let Some(ny) = y {
                    self.y = *ny;
                }
                self.do_flash();
            }
            GerberCommand::StepRepeat {
                x_repeat,
                y_repeat,
                x_step,
                y_step,
            } => {
                // Close any open SR block first, replicating its drawings.
                self.close_sr_block();

                if *x_repeat > 1 || *y_repeat > 1 {
                    // Open a new SR block.
                    // The step values in the file are in file units (mm or inch).
                    // Since we read them as f64 directly, convert inch→mm if needed.
                    let step_x_mm = if self.converter.units == super::coord::Units::Inches {
                        x_step * 25.4
                    } else {
                        *x_step
                    };
                    let step_y_mm = if self.converter.units == super::coord::Units::Inches {
                        y_step * 25.4
                    } else {
                        *y_step
                    };
                    self.sr_block_start = Some(self.drawings.len());
                    self.sr_x_repeat = *x_repeat;
                    self.sr_y_repeat = *y_repeat;
                    self.sr_x_step = step_x_mm;
                    self.sr_y_step = step_y_mm;
                }
                // x_repeat=1, y_repeat=1 was already closed above; nothing left to do.
            }
            GerberCommand::EndOfFile | GerberCommand::FileFunction(_) => {}
        }
    }

    /// Close an open step-and-repeat block: replicate block drawings at each grid position.
    fn close_sr_block(&mut self) {
        let Some(start) = self.sr_block_start.take() else {
            return;
        };

        let block: Vec<Drawing> = self.drawings[start..].to_vec();

        for yi in 0..self.sr_y_repeat {
            for xi in 0..self.sr_x_repeat {
                if xi == 0 && yi == 0 {
                    continue; // original position already drawn
                }
                let dx = xi as f64 * self.sr_x_step;
                let dy = yi as f64 * self.sr_y_step;
                for d in &block {
                    self.drawings.push(offset_drawing(d, dx, dy));
                }
            }
        }

        // Reset SR state to defaults.
        self.sr_x_repeat = 1;
        self.sr_y_repeat = 1;
        self.sr_x_step = 0.0;
        self.sr_y_step = 0.0;
    }

    fn do_interpolate(&mut self, old_x: i64, old_y: i64, i: Option<i64>, j: Option<i64>) {
        // Skip clear polarity for now
        if self.polarity == Polarity::Clear {
            if self.region_active {
                let px = self.converter.to_mm(self.x, true);
                let py = self.converter.to_mm(self.y, false);
                self.region_points.push([px, py]);
            }
            return;
        }

        let x1 = self.converter.to_mm(old_x, true);
        let y1 = self.converter.to_mm(old_y, false);
        let x2 = self.converter.to_mm(self.x, true);
        let y2 = self.converter.to_mm(self.y, false);

        if self.region_active {
            // In region mode, just collect points
            if self.region_points.is_empty() {
                self.region_points.push([x1, y1]);
            }
            // For arcs in region mode, approximate with line segments
            if self.interpolation != InterpolationMode::Linear && (i.is_some() || j.is_some()) {
                let arc_points = self.compute_arc_points(old_x, old_y, i, j);
                // Skip first point (it's the current position)
                for pt in arc_points.into_iter().skip(1) {
                    self.region_points.push(pt);
                }
            } else {
                self.region_points.push([x2, y2]);
            }
            return;
        }

        let width = self.apertures.stroke_width(self.aperture);

        match self.interpolation {
            InterpolationMode::Linear => {
                self.drawings.push(Drawing::Segment {
                    start: [x1, y1],
                    end: [x2, y2],
                    width,
                });
            }
            InterpolationMode::ClockwiseArc | InterpolationMode::CounterClockwiseArc => {
                if let Some(arc) = self.compute_arc_drawing(old_x, old_y, i, j, width) {
                    self.drawings.push(arc);
                }
            }
        }
    }

    fn do_flash(&mut self) {
        if self.polarity == Polarity::Clear {
            return;
        }

        let px = self.converter.to_mm(self.x, true);
        let py = self.converter.to_mm(self.y, false);

        let aperture_code = self.aperture;
        if let Some(ap) = self.apertures.get(aperture_code) {
            match &ap.template {
                ApertureTemplate::Circle { diameter } => {
                    let r = diameter / 2.0;
                    self.drawings.push(Drawing::Circle {
                        start: [px, py],
                        radius: r,
                        width: 0.0,
                        filled: Some(1),
                    });
                }
                ApertureTemplate::Rectangle { x_size, y_size } => {
                    let half_x = x_size / 2.0;
                    let half_y = y_size / 2.0;
                    self.drawings.push(Drawing::Rect {
                        start: [px - half_x, py - half_y],
                        end: [px + half_x, py + half_y],
                        width: 0.0,
                    });
                }
                ApertureTemplate::Obround { x_size, y_size } => {
                    // Approximate obround as a rectangle (close enough for rendering)
                    let half_x = x_size / 2.0;
                    let half_y = y_size / 2.0;
                    self.drawings.push(Drawing::Rect {
                        start: [px - half_x, py - half_y],
                        end: [px + half_x, py + half_y],
                        width: 0.0,
                    });
                }
                ApertureTemplate::Polygon {
                    outer_diameter,
                    num_vertices,
                    rotation,
                } => {
                    let r = outer_diameter / 2.0;
                    let n = *num_vertices as usize;
                    let rot_rad = rotation.to_radians();
                    let mut points = Vec::with_capacity(n);
                    for k in 0..n {
                        let angle = rot_rad + 2.0 * PI * (k as f64) / (n as f64);
                        points.push([px + r * angle.cos(), py + r * angle.sin()]);
                    }
                    self.drawings.push(Drawing::Polygon {
                        pos: [px, py],
                        angle: 0.0,
                        polygons: vec![points],
                        filled: Some(1),
                        width: 0.0,
                    });
                }
                ApertureTemplate::Macro { name, params } => {
                    if let Some(mac) = self.macro_table.get(name) {
                        let macro_drawings = macros::evaluate_macro(mac, params, px, py);
                        self.drawings.extend(macro_drawings);
                    } else {
                        warn!("Gerber: D03 flash with undefined macro aperture '{name}'");
                    }
                }
            }
        } else {
            warn!("Gerber: D03 flash with undefined aperture D{aperture_code}");
        }
    }

    /// Compute an Arc drawing from I,J offsets.
    fn compute_arc_drawing(
        &self,
        old_x: i64,
        old_y: i64,
        i: Option<i64>,
        j: Option<i64>,
        width: f64,
    ) -> Option<Drawing> {
        let i_val = i.unwrap_or(0);
        let j_val = j.unwrap_or(0);

        let x1 = self.converter.to_mm(old_x, true);
        let y1 = self.converter.to_mm(old_y, false);
        let x2 = self.converter.to_mm(self.x, true);
        let y2 = self.converter.to_mm(self.y, false);

        // I,J are offsets from start point to center
        let cx = x1 + self.converter.to_mm(i_val, true);
        let cy = y1 + self.converter.to_mm(j_val, false);

        let radius = ((x1 - cx).powi(2) + (y1 - cy).powi(2)).sqrt();
        if radius < 1e-9 {
            return None;
        }

        let mut start_angle = (y1 - cy).atan2(x1 - cx).to_degrees();
        let mut end_angle = (y2 - cy).atan2(x2 - cx).to_degrees();

        // Normalize to 0..360
        if start_angle < 0.0 {
            start_angle += 360.0;
        }
        if end_angle < 0.0 {
            end_angle += 360.0;
        }

        // CW arcs go from start_angle decreasing to end_angle
        // CCW arcs go from start_angle increasing to end_angle
        // The Drawing::Arc type uses startangle/endangle in CCW direction.
        if self.interpolation == InterpolationMode::ClockwiseArc {
            std::mem::swap(&mut start_angle, &mut end_angle);
        }

        Some(Drawing::Arc {
            start: [cx, cy],
            radius,
            startangle: start_angle,
            endangle: end_angle,
            width,
        })
    }

    /// Compute arc points for region approximation.
    fn compute_arc_points(
        &self,
        old_x: i64,
        old_y: i64,
        i: Option<i64>,
        j: Option<i64>,
    ) -> Vec<[f64; 2]> {
        let i_val = i.unwrap_or(0);
        let j_val = j.unwrap_or(0);

        let x1 = self.converter.to_mm(old_x, true);
        let y1 = self.converter.to_mm(old_y, false);
        let x2 = self.converter.to_mm(self.x, true);
        let y2 = self.converter.to_mm(self.y, false);

        let cx = x1 + self.converter.to_mm(i_val, true);
        let cy = y1 + self.converter.to_mm(j_val, false);

        let radius = ((x1 - cx).powi(2) + (y1 - cy).powi(2)).sqrt();
        if radius < 1e-9 {
            return vec![[x1, y1], [x2, y2]];
        }

        let start_angle = (y1 - cy).atan2(x1 - cx);
        let mut end_angle = (y2 - cy).atan2(x2 - cx);

        let is_cw = self.interpolation == InterpolationMode::ClockwiseArc;

        // Ensure correct sweep direction
        if is_cw {
            if end_angle >= start_angle {
                end_angle -= 2.0 * PI;
            }
        } else if end_angle <= start_angle {
            end_angle += 2.0 * PI;
        }

        let sweep = (end_angle - start_angle).abs();
        let num_segments = ((sweep / (PI / 18.0)).ceil() as usize).max(2); // ~10 deg per segment

        let mut points = Vec::with_capacity(num_segments + 1);
        for k in 0..=num_segments {
            let t = k as f64 / num_segments as f64;
            let angle = start_angle + t * (end_angle - start_angle);
            points.push([cx + radius * angle.cos(), cy + radius * angle.sin()]);
        }

        points
    }

    /// Flush all collected region contours as a single multi-ring polygon.
    /// Called on RegionEnd (G37) or EOF.
    fn flush_region_end(&mut self) {
        // Save any in-progress contour
        if self.region_points.len() >= 3 {
            let points = std::mem::take(&mut self.region_points);
            self.region_contours.push(points);
        } else {
            self.region_points.clear();
        }

        if !self.region_contours.is_empty() && self.polarity == Polarity::Dark {
            let contours = std::mem::take(&mut self.region_contours);
            self.drawings.push(Drawing::Polygon {
                pos: [0.0, 0.0],
                angle: 0.0,
                polygons: contours,
                filled: Some(1),
                width: 0.0,
            });
        } else {
            self.region_contours.clear();
        }
    }
}

/// Translate all coordinate points in a Drawing by (dx, dy).
///
/// Used when replicating step-and-repeat blocks and aperture blocks.
pub(crate) fn offset_drawing(d: &Drawing, dx: f64, dy: f64) -> Drawing {
    match d {
        Drawing::Segment { start, end, width } => Drawing::Segment {
            start: [start[0] + dx, start[1] + dy],
            end: [end[0] + dx, end[1] + dy],
            width: *width,
        },
        Drawing::Rect { start, end, width } => Drawing::Rect {
            start: [start[0] + dx, start[1] + dy],
            end: [end[0] + dx, end[1] + dy],
            width: *width,
        },
        Drawing::Circle {
            start,
            radius,
            width,
            filled,
        } => Drawing::Circle {
            start: [start[0] + dx, start[1] + dy],
            radius: *radius,
            width: *width,
            filled: *filled,
        },
        Drawing::Arc {
            start,
            radius,
            startangle,
            endangle,
            width,
        } => Drawing::Arc {
            start: [start[0] + dx, start[1] + dy],
            radius: *radius,
            startangle: *startangle,
            endangle: *endangle,
            width: *width,
        },
        Drawing::Curve {
            start,
            end,
            cpa,
            cpb,
            width,
        } => Drawing::Curve {
            start: [start[0] + dx, start[1] + dy],
            end: [end[0] + dx, end[1] + dy],
            cpa: [cpa[0] + dx, cpa[1] + dy],
            cpb: [cpb[0] + dx, cpb[1] + dy],
            width: *width,
        },
        Drawing::Polygon {
            pos,
            angle,
            polygons,
            filled,
            width,
        } => Drawing::Polygon {
            pos: *pos,
            angle: *angle,
            polygons: polygons
                .iter()
                .map(|ring| ring.iter().map(|pt| [pt[0] + dx, pt[1] + dy]).collect())
                .collect(),
            filled: *filled,
            width: *width,
        },
    }
}

/// Interpret a sequence of Gerber commands into drawing primitives.
pub fn interpret(commands: &[GerberCommand]) -> Result<GerberLayerOutput, ExtractError> {
    let mut interp = Interpreter::new();

    for cmd in commands {
        interp.process(cmd);
    }

    // Flush any remaining region
    if interp.region_active {
        interp.flush_region_end();
    }

    // Close any unterminated SR block (some files omit the closing %SR%)
    interp.close_sr_block();

    Ok(GerberLayerOutput {
        drawings: interp.drawings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parsers::gerber::commands::ApertureTemplate;
    use crate::parsers::gerber::coord::{CoordinateFormat, Units};

    /// Helper: create a basic command sequence with format spec and aperture.
    fn setup_commands() -> Vec<GerberCommand> {
        vec![
            GerberCommand::FormatSpec(CoordinateFormat {
                x_integer: 2,
                x_decimal: 4,
                y_integer: 2,
                y_decimal: 4,
            }),
            GerberCommand::Units(Units::Millimeters),
            GerberCommand::ApertureDefine {
                code: 10,
                template: ApertureTemplate::Circle { diameter: 0.1 },
            },
            GerberCommand::SelectAperture(10),
            GerberCommand::LinearMode,
        ]
    }

    #[test]
    fn test_linear_segment() {
        let mut cmds = setup_commands();
        cmds.push(GerberCommand::Move {
            x: Some(0),
            y: Some(0),
        });
        cmds.push(GerberCommand::Interpolate {
            x: Some(10000), // 1.0 mm
            y: Some(0),
            i: None,
            j: None,
        });

        let output = interpret(&cmds).unwrap();
        assert_eq!(output.drawings.len(), 1);
        match &output.drawings[0] {
            Drawing::Segment { start, end, width } => {
                assert!((start[0]).abs() < 1e-6);
                assert!((start[1]).abs() < 1e-6);
                assert!((end[0] - 1.0).abs() < 1e-6);
                assert!((end[1]).abs() < 1e-6);
                assert!((*width - 0.1).abs() < 1e-6);
            }
            other => panic!("expected Segment, got: {other:?}"),
        }
    }

    #[test]
    fn test_flash_circle() {
        let mut cmds = setup_commands();
        cmds.push(GerberCommand::Flash {
            x: Some(10000),
            y: Some(20000),
        });

        let output = interpret(&cmds).unwrap();
        assert_eq!(output.drawings.len(), 1);
        match &output.drawings[0] {
            Drawing::Circle {
                start,
                radius,
                filled,
                ..
            } => {
                assert!((start[0] - 1.0).abs() < 1e-6);
                assert!((start[1] - 2.0).abs() < 1e-6);
                assert!((*radius - 0.05).abs() < 1e-6);
                assert_eq!(*filled, Some(1));
            }
            other => panic!("expected Circle, got: {other:?}"),
        }
    }

    #[test]
    fn test_flash_rectangle() {
        let mut cmds = vec![
            GerberCommand::FormatSpec(CoordinateFormat::default()),
            GerberCommand::Units(Units::Millimeters),
            GerberCommand::ApertureDefine {
                code: 11,
                template: ApertureTemplate::Rectangle {
                    x_size: 0.5,
                    y_size: 0.3,
                },
            },
            GerberCommand::SelectAperture(11),
        ];
        cmds.push(GerberCommand::Flash {
            x: Some(10000),
            y: Some(10000),
        });

        let output = interpret(&cmds).unwrap();
        assert_eq!(output.drawings.len(), 1);
        match &output.drawings[0] {
            Drawing::Rect { start, end, .. } => {
                // Center at (1.0, 1.0), rect half-sizes (0.25, 0.15)
                assert!((start[0] - 0.75).abs() < 1e-6);
                assert!((start[1] - 0.85).abs() < 1e-6);
                assert!((end[0] - 1.25).abs() < 1e-6);
                assert!((end[1] - 1.15).abs() < 1e-6);
            }
            other => panic!("expected Rect, got: {other:?}"),
        }
    }

    #[test]
    fn test_region_polygon() {
        let mut cmds = setup_commands();
        cmds.extend([
            GerberCommand::RegionBegin,
            GerberCommand::Move {
                x: Some(0),
                y: Some(0),
            },
            GerberCommand::Interpolate {
                x: Some(10000),
                y: Some(0),
                i: None,
                j: None,
            },
            GerberCommand::Interpolate {
                x: Some(10000),
                y: Some(10000),
                i: None,
                j: None,
            },
            GerberCommand::Interpolate {
                x: Some(0),
                y: Some(10000),
                i: None,
                j: None,
            },
            GerberCommand::Interpolate {
                x: Some(0),
                y: Some(0),
                i: None,
                j: None,
            },
            GerberCommand::RegionEnd,
        ]);

        let output = interpret(&cmds).unwrap();
        assert_eq!(output.drawings.len(), 1);
        match &output.drawings[0] {
            Drawing::Polygon {
                polygons, filled, ..
            } => {
                assert_eq!(*filled, Some(1));
                assert_eq!(polygons.len(), 1);
                assert_eq!(polygons[0].len(), 5); // 4 corners + closing
            }
            other => panic!("expected Polygon, got: {other:?}"),
        }
    }

    #[test]
    fn test_coordinate_persistence() {
        // D01 without X or Y should reuse previous value
        let mut cmds = setup_commands();
        cmds.extend([
            GerberCommand::Move {
                x: Some(10000),
                y: Some(20000),
            },
            GerberCommand::Interpolate {
                x: Some(30000),
                y: None, // Y should stay at 20000
                i: None,
                j: None,
            },
        ]);

        let output = interpret(&cmds).unwrap();
        assert_eq!(output.drawings.len(), 1);
        match &output.drawings[0] {
            Drawing::Segment { start, end, .. } => {
                assert!((start[0] - 1.0).abs() < 1e-6);
                assert!((start[1] - 2.0).abs() < 1e-6);
                assert!((end[0] - 3.0).abs() < 1e-6);
                assert!((end[1] - 2.0).abs() < 1e-6); // Y persisted
            }
            other => panic!("expected Segment, got: {other:?}"),
        }
    }

    #[test]
    fn test_clear_polarity_skipped() {
        let mut cmds = setup_commands();
        cmds.extend([
            GerberCommand::Polarity(Polarity::Clear),
            GerberCommand::Move {
                x: Some(0),
                y: Some(0),
            },
            GerberCommand::Interpolate {
                x: Some(10000),
                y: Some(0),
                i: None,
                j: None,
            },
            GerberCommand::Flash {
                x: Some(20000),
                y: Some(0),
            },
        ]);

        let output = interpret(&cmds).unwrap();
        assert!(output.drawings.is_empty());
    }

    #[test]
    fn test_multiple_segments() {
        let mut cmds = setup_commands();
        cmds.extend([
            GerberCommand::Move {
                x: Some(0),
                y: Some(0),
            },
            GerberCommand::Interpolate {
                x: Some(10000),
                y: Some(0),
                i: None,
                j: None,
            },
            GerberCommand::Interpolate {
                x: Some(10000),
                y: Some(10000),
                i: None,
                j: None,
            },
            GerberCommand::Interpolate {
                x: Some(0),
                y: Some(10000),
                i: None,
                j: None,
            },
        ]);

        let output = interpret(&cmds).unwrap();
        assert_eq!(output.drawings.len(), 3);
    }

    #[test]
    fn test_inches_conversion() {
        let cmds = vec![
            GerberCommand::FormatSpec(CoordinateFormat {
                x_integer: 2,
                x_decimal: 4,
                y_integer: 2,
                y_decimal: 4,
            }),
            GerberCommand::Units(Units::Inches),
            GerberCommand::ApertureDefine {
                code: 10,
                template: ApertureTemplate::Circle { diameter: 0.01 }, // 0.01 inches
            },
            GerberCommand::SelectAperture(10),
            GerberCommand::LinearMode,
            GerberCommand::Move {
                x: Some(0),
                y: Some(0),
            },
            GerberCommand::Interpolate {
                x: Some(10000), // 1.0000 inches = 25.4 mm
                y: Some(0),
                i: None,
                j: None,
            },
        ];

        let output = interpret(&cmds).unwrap();
        assert_eq!(output.drawings.len(), 1);
        match &output.drawings[0] {
            Drawing::Segment { end, .. } => {
                assert!((end[0] - 25.4).abs() < 1e-4);
            }
            other => panic!("expected Segment, got: {other:?}"),
        }
    }

    #[test]
    fn test_region_multi_contour() {
        // A region with two contours (outer boundary + hole) should produce
        // a single Drawing::Polygon with two rings.
        let mut cmds = setup_commands();
        cmds.extend([
            GerberCommand::RegionBegin,
            // Outer contour: 10x10mm square
            GerberCommand::Move {
                x: Some(0),
                y: Some(0),
            },
            GerberCommand::Interpolate {
                x: Some(100000),
                y: Some(0),
                i: None,
                j: None,
            },
            GerberCommand::Interpolate {
                x: Some(100000),
                y: Some(100000),
                i: None,
                j: None,
            },
            GerberCommand::Interpolate {
                x: Some(0),
                y: Some(100000),
                i: None,
                j: None,
            },
            GerberCommand::Interpolate {
                x: Some(0),
                y: Some(0),
                i: None,
                j: None,
            },
            // D02 starts a new contour (hole): inner 5x5mm square
            GerberCommand::Move {
                x: Some(20000),
                y: Some(20000),
            },
            GerberCommand::Interpolate {
                x: Some(80000),
                y: Some(20000),
                i: None,
                j: None,
            },
            GerberCommand::Interpolate {
                x: Some(80000),
                y: Some(80000),
                i: None,
                j: None,
            },
            GerberCommand::Interpolate {
                x: Some(20000),
                y: Some(80000),
                i: None,
                j: None,
            },
            GerberCommand::Interpolate {
                x: Some(20000),
                y: Some(20000),
                i: None,
                j: None,
            },
            GerberCommand::RegionEnd,
        ]);

        let output = interpret(&cmds).unwrap();
        assert_eq!(output.drawings.len(), 1);
        match &output.drawings[0] {
            Drawing::Polygon { polygons, .. } => {
                // Should have 2 rings (outer + hole) in a single polygon
                assert_eq!(polygons.len(), 2);
                assert_eq!(polygons[0].len(), 5); // outer: 4 corners + closing
                assert_eq!(polygons[1].len(), 5); // hole: 4 corners + closing
            }
            other => panic!("expected Polygon, got: {other:?}"),
        }
    }

    #[test]
    fn test_flash_macro_aperture() {
        let mut cmds = vec![
            GerberCommand::FormatSpec(CoordinateFormat::default()),
            GerberCommand::Units(Units::Millimeters),
            // Define a macro with a single circle primitive
            GerberCommand::MacroDefine {
                name: "MYCIRC".to_string(),
                body: vec!["1,1,$1,0,0".to_string()],
            },
            // Define aperture using the macro with param 0.5
            GerberCommand::ApertureDefine {
                code: 20,
                template: ApertureTemplate::Macro {
                    name: "MYCIRC".to_string(),
                    params: vec![0.5],
                },
            },
            GerberCommand::SelectAperture(20),
        ];
        cmds.push(GerberCommand::Flash {
            x: Some(10000),
            y: Some(20000),
        });

        let output = interpret(&cmds).unwrap();
        assert_eq!(output.drawings.len(), 1);
        match &output.drawings[0] {
            Drawing::Circle { start, radius, .. } => {
                assert!((start[0] - 1.0).abs() < 1e-6);
                assert!((start[1] - 2.0).abs() < 1e-6);
                assert!((*radius - 0.25).abs() < 1e-6); // diameter 0.5 / 2
            }
            other => panic!("expected Circle, got: {other:?}"),
        }
    }

    #[test]
    fn test_step_repeat_2x2() {
        // Draw one segment inside a 2×2 SR block with 3mm X step and 4mm Y step.
        // Expected: 4 copies of the segment at (0,0), (3,0), (0,4), (3,4) offsets.
        let mut cmds = setup_commands();
        cmds.extend([
            GerberCommand::StepRepeat {
                x_repeat: 2,
                y_repeat: 2,
                x_step: 3.0,
                y_step: 4.0,
            },
            GerberCommand::Move {
                x: Some(0),
                y: Some(0),
            },
            GerberCommand::Interpolate {
                x: Some(10000), // 1 mm
                y: Some(0),
                i: None,
                j: None,
            },
            // Close the block
            GerberCommand::StepRepeat {
                x_repeat: 1,
                y_repeat: 1,
                x_step: 0.0,
                y_step: 0.0,
            },
        ]);

        let output = interpret(&cmds).unwrap();
        assert_eq!(output.drawings.len(), 4, "2×2 SR should produce 4 drawings");

        // Collect all segment starts and ends
        let mut starts: Vec<[f64; 2]> = output
            .drawings
            .iter()
            .filter_map(|d| {
                if let Drawing::Segment { start, .. } = d {
                    Some(*start)
                } else {
                    None
                }
            })
            .collect();
        starts.sort_by(|a, b| {
            a[0].partial_cmp(&b[0])
                .unwrap()
                .then(a[1].partial_cmp(&b[1]).unwrap())
        });

        let expected = [[0.0, 0.0], [0.0, 4.0], [3.0, 0.0], [3.0, 4.0]];
        for (got, exp) in starts.iter().zip(expected.iter()) {
            assert!(
                (got[0] - exp[0]).abs() < 1e-6,
                "start x: got {} exp {}",
                got[0],
                exp[0]
            );
            assert!(
                (got[1] - exp[1]).abs() < 1e-6,
                "start y: got {} exp {}",
                got[1],
                exp[1]
            );
        }
    }

    #[test]
    fn test_step_repeat_implicit_close_at_eof() {
        // SR block not explicitly closed — should be closed at EOF.
        let mut cmds = setup_commands();
        cmds.extend([
            GerberCommand::StepRepeat {
                x_repeat: 3,
                y_repeat: 1,
                x_step: 2.0,
                y_step: 0.0,
            },
            GerberCommand::Flash {
                x: Some(0),
                y: Some(0),
            },
        ]);

        let output = interpret(&cmds).unwrap();
        assert_eq!(
            output.drawings.len(),
            3,
            "implicit close should replicate 3×1"
        );
    }
}
