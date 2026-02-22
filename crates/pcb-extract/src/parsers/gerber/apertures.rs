use std::collections::HashMap;

use super::commands::ApertureTemplate;

/// An aperture in the aperture table.
#[derive(Debug, Clone)]
pub struct Aperture {
    pub template: ApertureTemplate,
}

/// Aperture table built from %AD commands.
#[derive(Debug, Default)]
pub struct ApertureTable {
    apertures: HashMap<u32, Aperture>,
}

impl ApertureTable {
    pub fn define(&mut self, code: u32, template: ApertureTemplate) {
        self.apertures.insert(code, Aperture { template });
    }

    pub fn get(&self, code: u32) -> Option<&Aperture> {
        self.apertures.get(&code)
    }

    /// Get the effective stroke width for the current aperture when used for D01 draws.
    /// For circles, this is the diameter. For rectangles/obrounds, it's the minimum dimension.
    pub fn stroke_width(&self, code: u32) -> f64 {
        match self.apertures.get(&code) {
            Some(ap) => match &ap.template {
                ApertureTemplate::Circle { diameter } => *diameter,
                ApertureTemplate::Rectangle { x_size, y_size } => x_size.min(*y_size),
                ApertureTemplate::Obround { x_size, y_size } => x_size.min(*y_size),
                ApertureTemplate::Polygon { outer_diameter, .. } => *outer_diameter,
            },
            None => 0.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_define_and_get() {
        let mut table = ApertureTable::default();
        table.define(10, ApertureTemplate::Circle { diameter: 0.5 });
        let ap = table.get(10).unwrap();
        assert!(
            matches!(ap.template, ApertureTemplate::Circle { diameter } if (diameter - 0.5).abs() < 1e-9)
        );
    }

    #[test]
    fn test_get_missing() {
        let table = ApertureTable::default();
        assert!(table.get(10).is_none());
    }

    #[test]
    fn test_stroke_width_circle() {
        let mut table = ApertureTable::default();
        table.define(10, ApertureTemplate::Circle { diameter: 0.254 });
        assert!((table.stroke_width(10) - 0.254).abs() < 1e-9);
    }

    #[test]
    fn test_stroke_width_rect() {
        let mut table = ApertureTable::default();
        table.define(
            11,
            ApertureTemplate::Rectangle {
                x_size: 0.5,
                y_size: 0.3,
            },
        );
        assert!((table.stroke_width(11) - 0.3).abs() < 1e-9);
    }

    #[test]
    fn test_stroke_width_missing() {
        let table = ApertureTable::default();
        assert!((table.stroke_width(99)).abs() < 1e-9);
    }
}
