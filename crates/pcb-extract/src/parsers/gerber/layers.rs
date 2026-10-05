use serde::Serialize;

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
    SolderPasteTop,
    SolderPasteBottom,
    BoardOutline,
    Drills,
    /// A declared non-fabrication layer: documentation, courtyard, fab/assembly
    /// drawings, adhesive, user layers.
    Other,
    Unknown,
}

/// Side-independent function of a fabrication layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LayerFunction {
    Copper,
    Silkscreen,
    SolderMask,
    SolderPaste,
    Outline,
    Drill,
    Other,
    Unknown,
}

/// Which side of the board a fabrication layer belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LayerSide {
    Top,
    Bottom,
    Inner,
}

impl GerberLayerType {
    pub fn function(&self) -> LayerFunction {
        match self {
            Self::CopperTop | Self::CopperBottom | Self::CopperInner(_) => LayerFunction::Copper,
            Self::SilkscreenTop | Self::SilkscreenBottom => LayerFunction::Silkscreen,
            Self::SolderMaskTop | Self::SolderMaskBottom => LayerFunction::SolderMask,
            Self::SolderPasteTop | Self::SolderPasteBottom => LayerFunction::SolderPaste,
            Self::BoardOutline => LayerFunction::Outline,
            Self::Drills => LayerFunction::Drill,
            Self::Other => LayerFunction::Other,
            Self::Unknown => LayerFunction::Unknown,
        }
    }

    pub fn side(&self) -> Option<LayerSide> {
        match self {
            Self::CopperTop | Self::SilkscreenTop | Self::SolderMaskTop | Self::SolderPasteTop => {
                Some(LayerSide::Top)
            }
            Self::CopperBottom
            | Self::SilkscreenBottom
            | Self::SolderMaskBottom
            | Self::SolderPasteBottom => Some(LayerSide::Bottom),
            Self::CopperInner(_) => Some(LayerSide::Inner),
            Self::BoardOutline | Self::Drills | Self::Other | Self::Unknown => None,
        }
    }

    /// Inner copper layer name such as `In1`, if this is an inner copper layer.
    pub fn inner_name(&self) -> Option<&str> {
        match self {
            Self::CopperInner(name) => Some(name),
            _ => None,
        }
    }

    /// Back-to-front order for compositing layers in a single view.
    pub fn stack_order(&self) -> u8 {
        match self {
            Self::SolderPasteBottom => 0,
            Self::SilkscreenBottom => 1,
            Self::SolderMaskBottom => 2,
            Self::CopperBottom => 3,
            Self::CopperInner(_) => 4,
            Self::Other | Self::Unknown => 5,
            Self::CopperTop => 6,
            Self::SolderMaskTop => 7,
            Self::SilkscreenTop => 8,
            Self::SolderPasteTop => 9,
            Self::Drills => 10,
            Self::BoardOutline => 11,
        }
    }
}

/// Identify layer type from a Gerber X2 FileFunction attribute.
pub fn identify_from_x2(func: &FileFunction) -> GerberLayerType {
    match func {
        FileFunction::Copper {
            side, layer_num, ..
        } => match side {
            CopperSide::Top => GerberLayerType::CopperTop,
            CopperSide::Bottom => GerberLayerType::CopperBottom,
            // X2 numbers physical layers from the top (L1), so L2 is the first inner
            // layer, which KiCad and Altium call In1.
            CopperSide::Inner => {
                GerberLayerType::CopperInner(format!("In{}", layer_num.saturating_sub(1).max(1)))
            }
        },
        FileFunction::Legend { side } => match side {
            BoardSide::Top => GerberLayerType::SilkscreenTop,
            BoardSide::Bottom => GerberLayerType::SilkscreenBottom,
        },
        FileFunction::SolderMask { side } => match side {
            BoardSide::Top => GerberLayerType::SolderMaskTop,
            BoardSide::Bottom => GerberLayerType::SolderMaskBottom,
        },
        FileFunction::Paste { side } => match side {
            BoardSide::Top => GerberLayerType::SolderPasteTop,
            BoardSide::Bottom => GerberLayerType::SolderPasteBottom,
        },
        FileFunction::Profile => GerberLayerType::BoardOutline,
        FileFunction::Other(function) => match function.as_str() {
            "" => GerberLayerType::Unknown,
            // Drill and rout data written as Gerber.
            "Plated" | "NonPlated" => GerberLayerType::Drills,
            // Any other declared function (Other, Glue, AssemblyDrawing, Keep-out, ...)
            // is documentation rather than fabrication artwork.
            _ => GerberLayerType::Other,
        },
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
            // Solder paste
            "gtp" => return GerberLayerType::SolderPasteTop,
            "gbp" => return GerberLayerType::SolderPasteBottom,
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
    if lower.contains("f_paste") || lower.contains("f.paste") || lower.contains("front_paste") {
        return GerberLayerType::SolderPasteTop;
    }
    if lower.contains("b_paste") || lower.contains("b.paste") || lower.contains("back_paste") {
        return GerberLayerType::SolderPasteBottom;
    }
    if lower.contains("edge_cuts") || lower.contains("edge.cuts") || lower.contains("boardoutline")
    {
        return GerberLayerType::BoardOutline;
    }
    // KiCad documentation layers: courtyard, fab, adhesive, margin, user drawings.
    const KICAD_NON_FAB: [&str; 9] = [
        "courtyard",
        "crtyd",
        "_fab",
        ".fab",
        "_adhes",
        ".adhes",
        "margin",
        "_user",
        ".user",
    ];
    if KICAD_NON_FAB.iter().any(|p| lower.contains(p)) || lower.contains("user_") {
        return GerberLayerType::Other;
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
    let side = name_side(&lower);
    if lower.contains("copper") {
        match side {
            Some(LayerSide::Top) => return GerberLayerType::CopperTop,
            Some(LayerSide::Bottom) => return GerberLayerType::CopperBottom,
            _ => {}
        }
    }
    let by_side = |top, bottom| match side {
        Some(LayerSide::Top) => Some(top),
        Some(LayerSide::Bottom) => Some(bottom),
        _ => None,
    };
    if lower.contains("silk") {
        if let Some(layer) = by_side(
            GerberLayerType::SilkscreenTop,
            GerberLayerType::SilkscreenBottom,
        ) {
            return layer;
        }
    }
    if lower.contains("mask") {
        if let Some(layer) = by_side(
            GerberLayerType::SolderMaskTop,
            GerberLayerType::SolderMaskBottom,
        ) {
            return layer;
        }
    }
    if lower.contains("paste") {
        if let Some(layer) = by_side(
            GerberLayerType::SolderPasteTop,
            GerberLayerType::SolderPasteBottom,
        ) {
            return layer;
        }
    }
    if lower.contains("outline") || lower.contains("profile") {
        return GerberLayerType::BoardOutline;
    }
    if let Some(layer) = numbered_copper(&lower) {
        return layer;
    }
    // Allegro fabrication/assembly drawings.
    let stem = lower.rsplit_once('.').map_or(lower.as_str(), |(s, _)| s);
    if stem == "fab" || stem.starts_with("fabnote") || stem.starts_with("assy") {
        return GerberLayerType::Other;
    }

    GerberLayerType::Unknown
}

/// Board side named in a filename: "top"/"front" or "bottom"/"bot"/"back".
fn name_side(lower: &str) -> Option<LayerSide> {
    if lower.contains("top") || lower.contains("front") {
        Some(LayerSide::Top)
    } else if lower.contains("bot") || lower.contains("back") {
        Some(LayerSide::Bottom)
    } else {
        None
    }
}

/// Allegro-style numbered copper artwork: `l1_top.art`, `l3.art`, `l6_bottom.art`,
/// `layer2.gbr`. L1 is the top, and unsuffixed Ln (n > 1) is inner layer In(n-1).
fn numbered_copper(lower: &str) -> Option<GerberLayerType> {
    let stem = lower.rsplit_once('.').map_or(lower, |(s, _)| s);
    let rest = stem
        .strip_prefix("layer")
        .or_else(|| stem.strip_prefix('l'))?;
    let digits = rest.len() - rest.trim_start_matches(|c: char| c.is_ascii_digit()).len();
    let n: u32 = rest[..digits].parse().ok()?;
    let suffix = rest[digits..].trim_start_matches(['_', '-']);
    match (suffix, name_side(suffix)) {
        (_, Some(LayerSide::Top)) => Some(GerberLayerType::CopperTop),
        (_, Some(LayerSide::Bottom)) => Some(GerberLayerType::CopperBottom),
        ("", _) if n == 1 => Some(GerberLayerType::CopperTop),
        ("", _) => Some(GerberLayerType::CopperInner(format!("In{}", n - 1))),
        _ => None,
    }
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
        // Physical layer 3 is the second inner layer.
        assert_eq!(
            identify_from_x2(&func),
            GerberLayerType::CopperInner("In2".into())
        );
    }

    #[test]
    fn test_x2_other_functions() {
        for f in ["Other", "Glue", "AssemblyDrawing", "Drillmap"] {
            assert_eq!(
                identify_from_x2(&FileFunction::Other(f.into())),
                GerberLayerType::Other,
                "{f}"
            );
        }
        for f in ["Plated", "NonPlated"] {
            assert_eq!(
                identify_from_x2(&FileFunction::Other(f.into())),
                GerberLayerType::Drills
            );
        }
        assert_eq!(
            identify_from_x2(&FileFunction::Other(String::new())),
            GerberLayerType::Unknown
        );
    }

    #[test]
    fn test_kicad_documentation_layers() {
        for name in [
            "board-F_Courtyard.gbr",
            "board-B_Fab.gbr",
            "board-F_Adhes.gbr",
            "board-Margin.gbr",
            "board-User_2.gbr",
            "board-Dwgs_User.gbr",
        ] {
            assert_eq!(
                identify_from_filename(name),
                GerberLayerType::Other,
                "{name}"
            );
        }
    }

    #[test]
    fn test_allegro_artwork_names() {
        let cases = [
            ("l1_top.art", GerberLayerType::CopperTop),
            ("l6_bottom.art", GerberLayerType::CopperBottom),
            ("L2.art", GerberLayerType::CopperInner("In1".into())),
            ("l5.art", GerberLayerType::CopperInner("In4".into())),
            ("masktop.art", GerberLayerType::SolderMaskTop),
            ("maskbot.art", GerberLayerType::SolderMaskBottom),
            ("silkbot2.art", GerberLayerType::SilkscreenBottom),
            ("pastetop.art", GerberLayerType::SolderPasteTop),
            ("fab.art", GerberLayerType::Other),
            ("fabnotes.art", GerberLayerType::Other),
            ("logo.art", GerberLayerType::Unknown),
        ];
        for (name, expected) in cases {
            assert_eq!(identify_from_filename(name), expected, "{name}");
        }
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

    #[test]
    fn test_x2_paste() {
        let func = FileFunction::Paste {
            side: BoardSide::Bottom,
        };
        assert_eq!(identify_from_x2(&func), GerberLayerType::SolderPasteBottom);
    }

    #[test]
    fn test_paste_filenames() {
        assert_eq!(
            identify_from_filename("board.GTP"),
            GerberLayerType::SolderPasteTop
        );
        assert_eq!(
            identify_from_filename("board.gbp"),
            GerberLayerType::SolderPasteBottom
        );
        assert_eq!(
            identify_from_filename("board-F_Paste.gbr"),
            GerberLayerType::SolderPasteTop
        );
        assert_eq!(
            identify_from_filename("board-B_Paste.gbr"),
            GerberLayerType::SolderPasteBottom
        );
    }

    #[test]
    fn test_function_and_side() {
        let inner = GerberLayerType::CopperInner("In2".into());
        assert_eq!(inner.function(), LayerFunction::Copper);
        assert_eq!(inner.side(), Some(LayerSide::Inner));
        assert_eq!(inner.inner_name(), Some("In2"));

        let mask = GerberLayerType::SolderMaskBottom;
        assert_eq!(mask.function(), LayerFunction::SolderMask);
        assert_eq!(mask.side(), Some(LayerSide::Bottom));
        assert_eq!(mask.inner_name(), None);

        assert_eq!(GerberLayerType::Drills.side(), None);
        assert!(
            GerberLayerType::CopperTop.stack_order() > GerberLayerType::CopperBottom.stack_order()
        );
        assert!(
            GerberLayerType::BoardOutline.stack_order()
                > GerberLayerType::SilkscreenTop.stack_order()
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
