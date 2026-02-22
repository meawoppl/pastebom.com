use super::commands::{BoardSide, CopperSide, FileFunction};

/// What role a Gerber file plays in the board stackup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GerberLayerType {
    CopperTop,
    CopperBottom,
    CopperInner(String),
    SilkscreenTop,
    SilkscreenBottom,
    SolderMaskTop,
    SolderMaskBottom,
    BoardOutline,
    Drills,
    Unknown,
}

/// Identify layer type from a Gerber X2 FileFunction attribute.
pub fn identify_from_x2(func: &FileFunction) -> GerberLayerType {
    match func {
        FileFunction::Copper {
            side, layer_num, ..
        } => match side {
            CopperSide::Top => GerberLayerType::CopperTop,
            CopperSide::Bottom => GerberLayerType::CopperBottom,
            CopperSide::Inner => GerberLayerType::CopperInner(format!("In{layer_num}")),
        },
        FileFunction::Legend { side } => match side {
            BoardSide::Top => GerberLayerType::SilkscreenTop,
            BoardSide::Bottom => GerberLayerType::SilkscreenBottom,
        },
        FileFunction::SolderMask { side } => match side {
            BoardSide::Top => GerberLayerType::SolderMaskTop,
            BoardSide::Bottom => GerberLayerType::SolderMaskBottom,
        },
        FileFunction::Profile => GerberLayerType::BoardOutline,
        _ => GerberLayerType::Unknown,
    }
}

/// Identify layer type from filename patterns.
///
/// Handles conventions from Altium/Protel, KiCad, and Eagle.
/// All comparisons are case-insensitive.
pub fn identify_from_filename(filename: &str) -> GerberLayerType {
    // Extract just the filename (strip directory path)
    let name = filename
        .rsplit('/')
        .next()
        .unwrap_or(filename)
        .rsplit('\\')
        .next()
        .unwrap_or(filename);
    let lower = name.to_lowercase();

    // Try extension-based matching first (Altium/Protel conventions)
    if let Some(ext) = lower.rsplit('.').next() {
        match ext {
            // Copper
            "gtl" => return GerberLayerType::CopperTop,
            "gbl" => return GerberLayerType::CopperBottom,
            "g1" | "g2" | "g3" | "g4" | "g5" | "g6" | "g7" | "g8" => {
                let num = &ext[1..]; // strip 'g' prefix
                return GerberLayerType::CopperInner(format!("In{num}"));
            }
            // Silkscreen
            "gto" => return GerberLayerType::SilkscreenTop,
            "gbo" => return GerberLayerType::SilkscreenBottom,
            // Solder mask
            "gts" => return GerberLayerType::SolderMaskTop,
            "gbs" => return GerberLayerType::SolderMaskBottom,
            // Board outline
            "gko" => return GerberLayerType::BoardOutline,
            // Eagle extensions
            "cmp" => return GerberLayerType::CopperTop,
            "sol" => return GerberLayerType::CopperBottom,
            "plc" => return GerberLayerType::SilkscreenTop,
            "pls" => return GerberLayerType::SilkscreenBottom,
            "stc" => return GerberLayerType::SolderMaskTop,
            "sts" => return GerberLayerType::SolderMaskBottom,
            "dim" => return GerberLayerType::BoardOutline,
            _ => {}
        }
    }

    // KiCad naming patterns (case-insensitive substring matching)
    if lower.contains("f_cu") || lower.contains("f.cu") || lower.contains("front_cu") {
        return GerberLayerType::CopperTop;
    }
    if lower.contains("b_cu") || lower.contains("b.cu") || lower.contains("back_cu") {
        return GerberLayerType::CopperBottom;
    }
    // KiCad inner copper: In1_Cu, In2_Cu, etc.
    if let Some(inner) = extract_kicad_inner(&lower) {
        return GerberLayerType::CopperInner(inner);
    }
    if lower.contains("f_silks")
        || lower.contains("f.silks")
        || lower.contains("f_silkscreen")
        || lower.contains("front_silk")
    {
        return GerberLayerType::SilkscreenTop;
    }
    if lower.contains("b_silks")
        || lower.contains("b.silks")
        || lower.contains("b_silkscreen")
        || lower.contains("back_silk")
    {
        return GerberLayerType::SilkscreenBottom;
    }
    if lower.contains("f_mask") || lower.contains("f.mask") || lower.contains("front_mask") {
        return GerberLayerType::SolderMaskTop;
    }
    if lower.contains("b_mask") || lower.contains("b.mask") || lower.contains("back_mask") {
        return GerberLayerType::SolderMaskBottom;
    }
    if lower.contains("edge_cuts") || lower.contains("edge.cuts") || lower.contains("boardoutline")
    {
        return GerberLayerType::BoardOutline;
    }

    // EasyEDA naming
    if lower.contains("toplayer") {
        return GerberLayerType::CopperTop;
    }
    if lower.contains("bottomlayer") {
        return GerberLayerType::CopperBottom;
    }
    if lower.contains("topsilk") {
        return GerberLayerType::SilkscreenTop;
    }
    if lower.contains("bottomsilk") {
        return GerberLayerType::SilkscreenBottom;
    }
    if lower.contains("topsoldermask") {
        return GerberLayerType::SolderMaskTop;
    }
    if lower.contains("bottomsoldermask") {
        return GerberLayerType::SolderMaskBottom;
    }

    // Generic patterns
    if lower.contains("top") && lower.contains("copper") {
        return GerberLayerType::CopperTop;
    }
    if lower.contains("bottom") && lower.contains("copper") {
        return GerberLayerType::CopperBottom;
    }
    if lower.contains("silkscreen") || lower.contains("silk") {
        if lower.contains("top") || lower.contains("front") {
            return GerberLayerType::SilkscreenTop;
        }
        if lower.contains("bottom") || lower.contains("back") {
            return GerberLayerType::SilkscreenBottom;
        }
    }
    if lower.contains("soldermask") || (lower.contains("solder") && lower.contains("mask")) {
        if lower.contains("top") || lower.contains("front") {
            return GerberLayerType::SolderMaskTop;
        }
        if lower.contains("bottom") || lower.contains("back") {
            return GerberLayerType::SolderMaskBottom;
        }
    }
    if lower.contains("outline") || lower.contains("profile") {
        return GerberLayerType::BoardOutline;
    }

    GerberLayerType::Unknown
}

/// Extract KiCad inner copper layer name (e.g., "In1_Cu" -> "In1").
fn extract_kicad_inner(lower: &str) -> Option<String> {
    // Match patterns like "in1_cu", "in2_cu", "in1.cu"
    for sep in ["_cu", ".cu"] {
        if let Some(pos) = lower.find(sep) {
            let before = &lower[..pos];
            // Look for "inN" pattern
            if let Some(in_pos) = before.rfind("in") {
                let num_str = &before[in_pos + 2..];
                if let Ok(n) = num_str.parse::<u32>() {
                    return Some(format!("In{n}"));
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- X2 attribute tests ---

    #[test]
    fn test_x2_copper_top() {
        let func = FileFunction::Copper {
            layer_num: 1,
            side: CopperSide::Top,
        };
        assert_eq!(identify_from_x2(&func), GerberLayerType::CopperTop);
    }

    #[test]
    fn test_x2_copper_bottom() {
        let func = FileFunction::Copper {
            layer_num: 2,
            side: CopperSide::Bottom,
        };
        assert_eq!(identify_from_x2(&func), GerberLayerType::CopperBottom);
    }

    #[test]
    fn test_x2_copper_inner() {
        let func = FileFunction::Copper {
            layer_num: 3,
            side: CopperSide::Inner,
        };
        assert_eq!(
            identify_from_x2(&func),
            GerberLayerType::CopperInner("In3".into())
        );
    }

    #[test]
    fn test_x2_legend_top() {
        let func = FileFunction::Legend {
            side: BoardSide::Top,
        };
        assert_eq!(identify_from_x2(&func), GerberLayerType::SilkscreenTop);
    }

    #[test]
    fn test_x2_profile() {
        assert_eq!(
            identify_from_x2(&FileFunction::Profile),
            GerberLayerType::BoardOutline
        );
    }

    // --- Altium/Protel extension tests ---

    #[test]
    fn test_altium_extensions() {
        assert_eq!(
            identify_from_filename("board.GTL"),
            GerberLayerType::CopperTop
        );
        assert_eq!(
            identify_from_filename("board.GBL"),
            GerberLayerType::CopperBottom
        );
        assert_eq!(
            identify_from_filename("board.GTO"),
            GerberLayerType::SilkscreenTop
        );
        assert_eq!(
            identify_from_filename("board.GBO"),
            GerberLayerType::SilkscreenBottom
        );
        assert_eq!(
            identify_from_filename("board.GTS"),
            GerberLayerType::SolderMaskTop
        );
        assert_eq!(
            identify_from_filename("board.GBS"),
            GerberLayerType::SolderMaskBottom
        );
        assert_eq!(
            identify_from_filename("board.GKO"),
            GerberLayerType::BoardOutline
        );
    }

    #[test]
    fn test_altium_case_insensitive() {
        assert_eq!(
            identify_from_filename("BOARD.gtl"),
            GerberLayerType::CopperTop
        );
        assert_eq!(
            identify_from_filename("Board.Gbl"),
            GerberLayerType::CopperBottom
        );
    }

    #[test]
    fn test_altium_inner_layers() {
        assert_eq!(
            identify_from_filename("board.G1"),
            GerberLayerType::CopperInner("In1".into())
        );
        assert_eq!(
            identify_from_filename("board.G2"),
            GerberLayerType::CopperInner("In2".into())
        );
    }

    // --- KiCad naming tests ---

    #[test]
    fn test_kicad_naming() {
        assert_eq!(
            identify_from_filename("board-F_Cu.gbr"),
            GerberLayerType::CopperTop
        );
        assert_eq!(
            identify_from_filename("board-B_Cu.gbr"),
            GerberLayerType::CopperBottom
        );
        assert_eq!(
            identify_from_filename("board-F_SilkS.gbr"),
            GerberLayerType::SilkscreenTop
        );
        assert_eq!(
            identify_from_filename("board-B_SilkS.gbr"),
            GerberLayerType::SilkscreenBottom
        );
        assert_eq!(
            identify_from_filename("board-Edge_Cuts.gbr"),
            GerberLayerType::BoardOutline
        );
        assert_eq!(
            identify_from_filename("board-F_Mask.gbr"),
            GerberLayerType::SolderMaskTop
        );
    }

    #[test]
    fn test_kicad_inner_copper() {
        assert_eq!(
            identify_from_filename("board-In1_Cu.gbr"),
            GerberLayerType::CopperInner("In1".into())
        );
        assert_eq!(
            identify_from_filename("board-In2_Cu.gbr"),
            GerberLayerType::CopperInner("In2".into())
        );
    }

    // --- Eagle naming tests ---

    #[test]
    fn test_eagle_extensions() {
        assert_eq!(
            identify_from_filename("board.cmp"),
            GerberLayerType::CopperTop
        );
        assert_eq!(
            identify_from_filename("board.sol"),
            GerberLayerType::CopperBottom
        );
        assert_eq!(
            identify_from_filename("board.plc"),
            GerberLayerType::SilkscreenTop
        );
    }

    // --- EasyEDA naming tests ---

    #[test]
    fn test_easyeda_naming() {
        assert_eq!(
            identify_from_filename("Gerber_TopLayer.GTL"),
            GerberLayerType::CopperTop
        );
        assert_eq!(
            identify_from_filename("Gerber_BottomLayer.GBL"),
            GerberLayerType::CopperBottom
        );
        assert_eq!(
            identify_from_filename("Gerber_TopSilkLayer.GTO"),
            GerberLayerType::SilkscreenTop
        );
    }

    // --- Generic naming patterns (EAGLE CAM output style) ---

    #[test]
    fn test_generic_silkscreen_naming() {
        assert_eq!(
            identify_from_filename("silkscreen_top.gbr"),
            GerberLayerType::SilkscreenTop
        );
        assert_eq!(
            identify_from_filename("silkscreen_bottom.gbr"),
            GerberLayerType::SilkscreenBottom
        );
        assert_eq!(
            identify_from_filename("GerberFiles/silkscreen_top.gbr"),
            GerberLayerType::SilkscreenTop
        );
    }

    #[test]
    fn test_generic_soldermask_naming() {
        assert_eq!(
            identify_from_filename("soldermask_top.gbr"),
            GerberLayerType::SolderMaskTop
        );
        assert_eq!(
            identify_from_filename("soldermask_bottom.gbr"),
            GerberLayerType::SolderMaskBottom
        );
    }

    #[test]
    fn test_generic_copper_naming() {
        assert_eq!(
            identify_from_filename("copper_top.gbr"),
            GerberLayerType::CopperTop
        );
        assert_eq!(
            identify_from_filename("copper_bottom.gbr"),
            GerberLayerType::CopperBottom
        );
    }

    #[test]
    fn test_generic_profile_naming() {
        assert_eq!(
            identify_from_filename("profile.gbr"),
            GerberLayerType::BoardOutline
        );
    }

    // --- Unknown tests ---

    #[test]
    fn test_unknown_file() {
        assert_eq!(
            identify_from_filename("readme.txt"),
            GerberLayerType::Unknown
        );
        assert_eq!(
            identify_from_filename("drill.drl"),
            GerberLayerType::Unknown
        );
    }

    // --- Path handling ---

    #[test]
    fn test_strips_directory_path() {
        assert_eq!(
            identify_from_filename("gerbers/board.GTL"),
            GerberLayerType::CopperTop
        );
        assert_eq!(
            identify_from_filename("output/copper/board-F_Cu.gbr"),
            GerberLayerType::CopperTop
        );
    }
}
