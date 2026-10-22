/// Parser for the ODB++ matrix file, which defines the layer stack.

#[derive(Debug, Clone, PartialEq)]
pub enum LayerType {
    Signal,
    PowerGround,
    Mixed,
    SolderMask,
    SolderPaste,
    SilkScreen,
    Drill,
    Rout,
    Document,
    Component,
    Dielectric,
    Mask,
}

#[derive(Debug, Clone, PartialEq)]
pub enum LayerContext {
    Board,
    Misc,
}

#[derive(Debug, Clone)]
pub struct MatrixLayer {
    pub name: String,
    pub layer_type: LayerType,
    pub row: u32,
    pub context: LayerContext,
}

#[derive(Debug, Clone)]
pub struct MatrixStep {
    pub name: String,
}

#[derive(Debug)]
pub struct Matrix {
    pub steps: Vec<MatrixStep>,
    pub layers: Vec<MatrixLayer>,
}

pub fn parse_matrix(content: &str) -> Matrix {
    let mut steps = Vec::new();
    let mut layers = Vec::new();

    let mut in_block: Option<&str> = None;
    let mut name = String::new();
    let mut layer_type = LayerType::Signal;
    let mut row = 0u32;
    let mut context = LayerContext::Board;

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        if line == "}" {
            match in_block {
                Some("STEP") => {
                    steps.push(MatrixStep { name: name.clone() });
                }
                Some("LAYER") => {
                    layers.push(MatrixLayer {
                        name: name.clone(),
                        layer_type: layer_type.clone(),
                        row,
                        context: context.clone(),
                    });
                }
                _ => {}
            }
            in_block = None;
            name.clear();
            continue;
        }

        if line.starts_with("STEP") && line.contains('{') {
            in_block = Some("STEP");
            continue;
        }
        if line.starts_with("LAYER") && line.contains('{') {
            in_block = Some("LAYER");
            continue;
        }

        if in_block.is_some() {
            if let Some((key, val)) = line.split_once('=') {
                let key = key.trim();
                let val = val.trim();
                match key {
                    "NAME" => name = val.to_string(),
                    "ROW" => row = val.parse().unwrap_or(0),
                    "TYPE" => {
                        layer_type = match val {
                            "SIGNAL" => LayerType::Signal,
                            "POWER_GROUND" => LayerType::PowerGround,
                            "MIXED" => LayerType::Mixed,
                            "SOLDER_MASK" => LayerType::SolderMask,
                            "SOLDER_PASTE" => LayerType::SolderPaste,
                            "SILK_SCREEN" => LayerType::SilkScreen,
                            "DRILL" => LayerType::Drill,
                            "ROUT" => LayerType::Rout,
                            "DOCUMENT" => LayerType::Document,
                            "COMPONENT" => LayerType::Component,
                            "DIELECTRIC" => LayerType::Dielectric,
                            "MASK" => LayerType::Mask,
                            _ => LayerType::Document,
                        };
                    }
                    "CONTEXT" => {
                        context = match val {
                            "BOARD" => LayerContext::Board,
                            _ => LayerContext::Misc,
                        };
                    }
                    _ => {}
                }
            }
        }
    }

    layers.sort_by_key(|l| l.row);

    Matrix { steps, layers }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_matrix() {
        let input = r#"
STEP {
    COL=1
    NAME=pcb
}

LAYER {
    ROW=1
    CONTEXT=BOARD
    TYPE=COMPONENT
    NAME=COMP_+_TOP
    POLARITY=POSITIVE
}

LAYER {
    ROW=2
    CONTEXT=BOARD
    TYPE=SILK_SCREEN
    NAME=SST
    POLARITY=POSITIVE
}

LAYER {
    ROW=3
    CONTEXT=BOARD
    TYPE=SIGNAL
    NAME=TOP
    POLARITY=POSITIVE
}
"#;
        let m = parse_matrix(input);
        assert_eq!(m.steps.len(), 1);
        assert_eq!(m.steps[0].name, "pcb");
        assert_eq!(m.layers.len(), 3);
        assert_eq!(m.layers[0].name, "COMP_+_TOP");
        assert_eq!(m.layers[0].layer_type, LayerType::Component);
        assert_eq!(m.layers[1].name, "SST");
        assert_eq!(m.layers[1].layer_type, LayerType::SilkScreen);
        assert_eq!(m.layers[2].name, "TOP");
        assert_eq!(m.layers[2].layer_type, LayerType::Signal);
    }
}
