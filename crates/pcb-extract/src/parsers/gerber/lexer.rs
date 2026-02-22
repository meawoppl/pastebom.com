/// Tokens produced by the Gerber lexer.
#[derive(Debug, Clone, PartialEq)]
pub enum GerberToken {
    /// Extended command block (contents between `%` delimiters).
    /// Example: `"FSLAX24Y24"`, `"ADD10C,0.020"`, `"LPD"`
    Extended(String),
    /// A word command terminated by `*`.
    /// Example: `"D10"`, `"X100Y200D01"`, `"G01"`, `"M02"`
    Word(String),
}

/// Tokenize a Gerber file into a sequence of tokens.
///
/// Gerber uses `*` as a statement terminator and `%...*%` for extended commands.
/// Comments start with `G04` and end with `*`.
pub fn tokenize(input: &str) -> Vec<GerberToken> {
    let mut tokens = Vec::new();
    let mut chars = input.chars().peekable();

    while let Some(&ch) = chars.peek() {
        match ch {
            '%' => {
                chars.next(); // consume '%'
                              // Read until closing '%', collecting extended command blocks
                let mut block = String::new();
                loop {
                    match chars.peek() {
                        Some(&'%') => {
                            chars.next(); // consume closing '%'
                                          // Emit any remaining content as a block
                            let trimmed = block.trim().to_string();
                            if !trimmed.is_empty() && !is_comment(&trimmed) {
                                tokens.push(GerberToken::Extended(trimmed));
                            }
                            break;
                        }
                        Some(&'*') => {
                            chars.next(); // consume '*'
                                          // End of one extended command within the block
                            let trimmed = block.trim().to_string();
                            if !trimmed.is_empty() && !is_comment(&trimmed) {
                                tokens.push(GerberToken::Extended(trimmed));
                            }
                            block.clear();
                        }
                        Some(&c) => {
                            chars.next();
                            if c != '\n' && c != '\r' {
                                block.push(c);
                            }
                        }
                        None => break, // EOF inside extended block
                    }
                }
            }
            '\n' | '\r' | ' ' | '\t' => {
                chars.next(); // skip whitespace
            }
            _ => {
                // Read a word command until '*'
                let mut word = String::new();
                while let Some(&c) = chars.peek() {
                    if c == '*' {
                        chars.next(); // consume '*'
                        break;
                    }
                    if c == '%' {
                        break; // don't consume, let outer loop handle
                    }
                    chars.next();
                    if c != '\n' && c != '\r' {
                        word.push(c);
                    }
                }
                let trimmed = word.trim().to_string();
                if !trimmed.is_empty() && !is_comment(&trimmed) {
                    tokens.push(GerberToken::Word(trimmed));
                }
            }
        }
    }

    tokens
}

/// Check if a command is a G04 comment.
fn is_comment(s: &str) -> bool {
    s.starts_with("G04") || s.starts_with("G4")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_commands() {
        let input = "G01*\nD10*\nX100Y200D01*\nM02*\n";
        let tokens = tokenize(input);
        assert_eq!(
            tokens,
            vec![
                GerberToken::Word("G01".into()),
                GerberToken::Word("D10".into()),
                GerberToken::Word("X100Y200D01".into()),
                GerberToken::Word("M02".into()),
            ]
        );
    }

    #[test]
    fn test_extended_commands() {
        let input = "%FSLAX24Y24*%\n%MOMM*%\n%ADD10C,0.020*%\n";
        let tokens = tokenize(input);
        assert_eq!(
            tokens,
            vec![
                GerberToken::Extended("FSLAX24Y24".into()),
                GerberToken::Extended("MOMM".into()),
                GerberToken::Extended("ADD10C,0.020".into()),
            ]
        );
    }

    #[test]
    fn test_comment_skipped() {
        let input = "G04 This is a comment*\nD10*\n";
        let tokens = tokenize(input);
        assert_eq!(tokens, vec![GerberToken::Word("D10".into())]);
    }

    #[test]
    fn test_extended_comment_skipped() {
        let input = "%G04 Comment in extended block*%\n%MOMM*%\n";
        let tokens = tokenize(input);
        assert_eq!(tokens, vec![GerberToken::Extended("MOMM".into())]);
    }

    #[test]
    fn test_multiple_extended_in_one_block() {
        // Some files put multiple extended commands in one % block
        let input = "%FSLAX24Y24*MOMM*%\n";
        let tokens = tokenize(input);
        assert_eq!(
            tokens,
            vec![
                GerberToken::Extended("FSLAX24Y24".into()),
                GerberToken::Extended("MOMM".into()),
            ]
        );
    }

    #[test]
    fn test_mixed_content() {
        let input =
            "%FSLAX24Y24*%\n%MOMM*%\n%ADD10C,0.100*%\nG01*\nD10*\nX0Y0D02*\nX10000Y0D01*\nM02*\n";
        let tokens = tokenize(input);
        assert_eq!(
            tokens,
            vec![
                GerberToken::Extended("FSLAX24Y24".into()),
                GerberToken::Extended("MOMM".into()),
                GerberToken::Extended("ADD10C,0.100".into()),
                GerberToken::Word("G01".into()),
                GerberToken::Word("D10".into()),
                GerberToken::Word("X0Y0D02".into()),
                GerberToken::Word("X10000Y0D01".into()),
                GerberToken::Word("M02".into()),
            ]
        );
    }

    #[test]
    fn test_empty_input() {
        assert_eq!(tokenize(""), Vec::<GerberToken>::new());
    }

    #[test]
    fn test_whitespace_only() {
        assert_eq!(tokenize("  \n\r\t  "), Vec::<GerberToken>::new());
    }

    #[test]
    fn test_x2_attributes() {
        let input = "%TF.FileFunction,Copper,L1,Top*%\n%TF.FilePolarity,Positive*%\n";
        let tokens = tokenize(input);
        assert_eq!(
            tokens,
            vec![
                GerberToken::Extended("TF.FileFunction,Copper,L1,Top".into()),
                GerberToken::Extended("TF.FilePolarity,Positive".into()),
            ]
        );
    }
}
