//! Default colours, opacity, and visibility for each kind of fabrication layer.

use pcb_extract::parsers::gerber::layers::{GerberLayerType, LayerFunction, LayerSide};
use serde::Serialize;

pub const DEFAULT_BACKGROUND: &str = "#12161c";

const INNER_COPPER: [&str; 6] = [
    "#c678dd", "#56b6c2", "#98c379", "#e06c75", "#61afef", "#d19a66",
];

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LayerStyle {
    pub color: String,
    pub opacity: f64,
    pub visible: bool,
}

impl LayerStyle {
    fn new(color: &str, opacity: f64, visible: bool) -> Self {
        Self {
            color: color.to_string(),
            opacity,
            visible,
        }
    }

    /// Default style for a layer. Solder mask and paste are hidden by default since
    /// they obscure copper; drills take the background colour so they read as holes.
    pub fn default_for(layer_type: &GerberLayerType, background: &str) -> Self {
        match layer_type {
            GerberLayerType::CopperTop => Self::new("#d8a03c", 0.85, true),
            GerberLayerType::CopperBottom => Self::new("#4f8fd6", 0.85, true),
            GerberLayerType::CopperInner(name) => {
                let n: usize = name
                    .trim_start_matches(|c: char| !c.is_ascii_digit())
                    .parse()
                    .unwrap_or(1);
                let color = INNER_COPPER[n.saturating_sub(1) % INNER_COPPER.len()];
                Self::new(color, 0.7, true)
            }
            GerberLayerType::SolderMaskTop => Self::new("#2f9e5b", 0.45, false),
            GerberLayerType::SolderMaskBottom => Self::new("#2f7f9e", 0.45, false),
            GerberLayerType::SolderPasteTop => Self::new("#b8b8b8", 0.8, false),
            GerberLayerType::SolderPasteBottom => Self::new("#8c8ca0", 0.8, false),
            GerberLayerType::SilkscreenTop => Self::new("#f2f2f2", 0.95, true),
            GerberLayerType::SilkscreenBottom => Self::new("#c9c3e6", 0.8, true),
            GerberLayerType::BoardOutline => Self::new("#e8d44d", 1.0, true),
            GerberLayerType::Drills => Self::new(background, 1.0, true),
            GerberLayerType::Other => Self::new("#9aa4b2", 0.6, false),
            GerberLayerType::Unknown => Self::new("#e5c07b", 0.6, true),
        }
    }
}

/// Short human-readable label such as "Top copper" or "In2 copper". Layers without
/// a known fabrication function are labelled with their file name.
pub fn layer_label(layer_type: &GerberLayerType, source_name: &str) -> String {
    let function = match layer_type.function() {
        LayerFunction::Copper => "copper",
        LayerFunction::Silkscreen => "silkscreen",
        LayerFunction::SolderMask => "solder mask",
        LayerFunction::SolderPaste => "paste",
        LayerFunction::Outline => return "Board outline".to_string(),
        LayerFunction::Drill => return "Drills".to_string(),
        LayerFunction::Other | LayerFunction::Unknown => {
            let file = source_name
                .rsplit(['/', '\\'])
                .next()
                .unwrap_or(source_name);
            return file.to_string();
        }
    };
    let side = match layer_type {
        GerberLayerType::CopperInner(name) => name.as_str(),
        _ if layer_type.side() == Some(LayerSide::Top) => "Top",
        _ => "Bottom",
    };
    format!("{side} {function}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inner_layers_get_distinct_colours() {
        let a = LayerStyle::default_for(&GerberLayerType::CopperInner("In1".into()), "#000");
        let b = LayerStyle::default_for(&GerberLayerType::CopperInner("In2".into()), "#000");
        assert_ne!(a.color, b.color);
    }

    #[test]
    fn drills_use_background_and_mask_hidden() {
        let drill = LayerStyle::default_for(&GerberLayerType::Drills, "#abcdef");
        assert_eq!(drill.color, "#abcdef");
        assert!(!LayerStyle::default_for(&GerberLayerType::SolderMaskTop, "#000").visible);
    }

    #[test]
    fn labels() {
        let label = |t: GerberLayerType| layer_label(&t, "fab.zip/gerbers/b.gbr");
        assert_eq!(label(GerberLayerType::CopperTop), "Top copper");
        assert_eq!(
            label(GerberLayerType::SilkscreenBottom),
            "Bottom silkscreen"
        );
        assert_eq!(
            label(GerberLayerType::CopperInner("In3".into())),
            "In3 copper"
        );
        assert_eq!(label(GerberLayerType::BoardOutline), "Board outline");
        assert_eq!(label(GerberLayerType::Unknown), "b.gbr");
        assert_eq!(label(GerberLayerType::Other), "b.gbr");
    }
}
