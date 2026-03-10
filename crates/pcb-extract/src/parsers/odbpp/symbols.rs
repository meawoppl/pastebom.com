/// Standard ODB++ symbol name parser.
/// Symbol dimensions are in mils (UNITS=INCH) or microns (UNITS=MM).

#[derive(Debug, Clone)]
pub struct SymbolInfo {
    pub shape: SymbolShape,
    /// Width in symbol units (mils or microns).
    pub width: f64,
    /// Height in symbol units (mils or microns).
    pub height: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SymbolShape {
    Round,
    Square,
    Rect,
    Oval,
    Diamond,
    Octagon,
}

/// Parse a standard ODB++ symbol name into dimensions.
/// Returns None for custom/user-defined symbols.
pub fn parse_symbol_name(name: &str) -> Option<SymbolInfo> {
    // Strip any trailing suffixes like "+1" (mirrored variant)
    let name = name.split('+').next().unwrap_or(name);

    if let Some(rest) = name.strip_prefix("rect") {
        // rect<w>x<h>, optionally rect<w>x<h>xr<r> or rect<w>x<h>xc<c>
        let dims = rest.split('x').collect::<Vec<_>>();
        if dims.len() >= 2 {
            let w: f64 = dims[0].parse().ok()?;
            let h: f64 = dims[1].parse().ok()?;
            return Some(SymbolInfo {
                shape: SymbolShape::Rect,
                width: w,
                height: h,
            });
        }
    } else if let Some(rest) = name.strip_prefix("oval") {
        let dims = rest.split('x').collect::<Vec<_>>();
        if dims.len() >= 2 {
            let w: f64 = dims[0].parse().ok()?;
            let h: f64 = dims[1].parse().ok()?;
            return Some(SymbolInfo {
                shape: SymbolShape::Oval,
                width: w,
                height: h,
            });
        }
    } else if let Some(rest) = name.strip_prefix("di") {
        let dims = rest.split('x').collect::<Vec<_>>();
        if dims.len() >= 2 {
            let w: f64 = dims[0].parse().ok()?;
            let h: f64 = dims[1].parse().ok()?;
            return Some(SymbolInfo {
                shape: SymbolShape::Diamond,
                width: w,
                height: h,
            });
        }
    } else if let Some(rest) = name.strip_prefix("oct") {
        let dims = rest.split('x').collect::<Vec<_>>();
        if dims.len() >= 2 {
            let w: f64 = dims[0].parse().ok()?;
            let h: f64 = dims[1].parse().ok()?;
            return Some(SymbolInfo {
                shape: SymbolShape::Octagon,
                width: w,
                height: h,
            });
        }
    } else if let Some(rest) = name.strip_prefix("donut_r") {
        let dims = rest.split('x').collect::<Vec<_>>();
        if dims.len() >= 2 {
            let od: f64 = dims[0].parse().ok()?;
            return Some(SymbolInfo {
                shape: SymbolShape::Round,
                width: od,
                height: od,
            });
        }
    } else if let Some(rest) = name.strip_prefix('r') {
        if let Ok(d) = rest.parse::<f64>() {
            return Some(SymbolInfo {
                shape: SymbolShape::Round,
                width: d,
                height: d,
            });
        }
    } else if let Some(rest) = name.strip_prefix('s') {
        if let Ok(s) = rest.parse::<f64>() {
            return Some(SymbolInfo {
                shape: SymbolShape::Square,
                width: s,
                height: s,
            });
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_round() {
        let s = parse_symbol_name("r25").unwrap();
        assert_eq!(s.shape, SymbolShape::Round);
        assert!((s.width - 25.0).abs() < 1e-6);
    }

    #[test]
    fn test_square() {
        let s = parse_symbol_name("s40").unwrap();
        assert_eq!(s.shape, SymbolShape::Square);
        assert!((s.width - 40.0).abs() < 1e-6);
    }

    #[test]
    fn test_rect() {
        let s = parse_symbol_name("rect20x60").unwrap();
        assert_eq!(s.shape, SymbolShape::Rect);
        assert!((s.width - 20.0).abs() < 1e-6);
        assert!((s.height - 60.0).abs() < 1e-6);
    }

    #[test]
    fn test_rect_rounded() {
        let s = parse_symbol_name("rect20x60xr5").unwrap();
        assert_eq!(s.shape, SymbolShape::Rect);
        assert!((s.width - 20.0).abs() < 1e-6);
    }

    #[test]
    fn test_oval() {
        let s = parse_symbol_name("oval141.73x62.99").unwrap();
        assert_eq!(s.shape, SymbolShape::Oval);
        assert!((s.width - 141.73).abs() < 1e-6);
    }

    #[test]
    fn test_round_decimal() {
        let s = parse_symbol_name("r0.5").unwrap();
        assert_eq!(s.shape, SymbolShape::Round);
        assert!((s.width - 0.5).abs() < 1e-6);
    }

    #[test]
    fn test_custom_returns_none() {
        assert!(parse_symbol_name("shp_ind_mss6132t").is_none());
        assert!(parse_symbol_name("s038_030").is_none());
    }
}
