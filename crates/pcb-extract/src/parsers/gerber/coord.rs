/// Coordinate format from the %FS (Format Specification) command.
///
/// Example: `%FSLAX24Y24*%` means leading-zero suppression, absolute mode,
/// 2 integer digits + 4 decimal digits for both X and Y.
#[derive(Debug, Clone, PartialEq)]
pub struct CoordinateFormat {
    pub x_integer: u8,
    pub x_decimal: u8,
    pub y_integer: u8,
    pub y_decimal: u8,
}

impl Default for CoordinateFormat {
    fn default() -> Self {
        // Common default: 2.4 format (FSLAX24Y24)
        Self {
            x_integer: 2,
            x_decimal: 4,
            y_integer: 2,
            y_decimal: 4,
        }
    }
}

/// Unit system from the %MO command.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Units {
    #[default]
    Millimeters,
    Inches,
}

/// Converts raw Gerber integer coordinates to millimeters.
#[derive(Debug, Clone, Default)]
pub struct CoordinateConverter {
    pub format: CoordinateFormat,
    pub units: Units,
}

impl CoordinateConverter {
    /// Convert a raw Gerber coordinate integer to mm.
    ///
    /// The raw value is an integer where the last N digits are the decimal part,
    /// as specified by the format. For example, with X24 format, the value 1234567
    /// means 123.4567 in the file's units.
    pub fn to_mm(&self, raw: i64, is_x: bool) -> f64 {
        let decimal_digits = if is_x {
            self.format.x_decimal
        } else {
            self.format.y_decimal
        };
        let divisor = 10f64.powi(decimal_digits as i32);
        let value = raw as f64 / divisor;
        match self.units {
            Units::Millimeters => value,
            Units::Inches => value * 25.4,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_format_mm() {
        let conv = CoordinateConverter::default();
        // FSLAX24Y24, MM: raw 10000 = 1.0000 mm
        assert!((conv.to_mm(10000, true) - 1.0).abs() < 1e-9);
        assert!((conv.to_mm(10000, false) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn test_negative_coordinate() {
        let conv = CoordinateConverter::default();
        assert!((conv.to_mm(-25000, true) - (-2.5)).abs() < 1e-9);
    }

    #[test]
    fn test_inches_to_mm() {
        let conv = CoordinateConverter {
            format: CoordinateFormat {
                x_integer: 2,
                x_decimal: 4,
                y_integer: 2,
                y_decimal: 4,
            },
            units: Units::Inches,
        };
        // raw 10000 = 1.0000 inches = 25.4 mm
        assert!((conv.to_mm(10000, true) - 25.4).abs() < 1e-9);
    }

    #[test]
    fn test_format_3_5() {
        let conv = CoordinateConverter {
            format: CoordinateFormat {
                x_integer: 3,
                x_decimal: 5,
                y_integer: 3,
                y_decimal: 5,
            },
            units: Units::Millimeters,
        };
        // raw 100000 = 1.00000 mm
        assert!((conv.to_mm(100000, true) - 1.0).abs() < 1e-9);
        // raw 1234567 = 12.34567 mm
        assert!((conv.to_mm(1234567, true) - 12.34567).abs() < 1e-9);
    }

    #[test]
    fn test_zero() {
        let conv = CoordinateConverter::default();
        assert!((conv.to_mm(0, true)).abs() < 1e-9);
    }

    #[test]
    fn test_format_2_5_inches() {
        let conv = CoordinateConverter {
            format: CoordinateFormat {
                x_integer: 2,
                x_decimal: 5,
                y_integer: 2,
                y_decimal: 5,
            },
            units: Units::Inches,
        };
        // raw 100000 = 1.00000 inches = 25.4 mm
        assert!((conv.to_mm(100000, true) - 25.4).abs() < 1e-9);
    }
}
