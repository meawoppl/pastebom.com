/// S-expression parser for KiCad files.
///
/// Grammar:
///   sexpr  = '(' atom_or_sexpr* ')'
///   atom   = string | number | symbol
///   string = '"' [^"]* '"'  (with escape handling)
///   number = [-]?[0-9]+[.[0-9]*]?
///   symbol = [^ \t\n\r()"]+

#[derive(Debug, Clone, PartialEq)]
pub enum SExpr {
    List(Vec<SExpr>),
    Atom(String),
}

impl SExpr {
    /// Get the first atom in a list (the "tag" or "name").
    pub fn tag(&self) -> Option<&str> {
        match self {
            SExpr::List(items) => items.first().and_then(|item| match item {
                SExpr::Atom(s) => Some(s.as_str()),
                _ => None,
            }),
            _ => None,
        }
    }

    /// Get list children (everything after the tag).
    pub fn children(&self) -> &[SExpr] {
        match self {
            SExpr::List(items) if !items.is_empty() => &items[1..],
            _ => &[],
        }
    }

    /// Get all items including tag.
    pub fn items(&self) -> &[SExpr] {
        match self {
            SExpr::List(items) => items,
            _ => &[],
        }
    }

    /// Find a child list with the given tag.
    pub fn find(&self, tag: &str) -> Option<&SExpr> {
        self.children().iter().find(|c| c.tag() == Some(tag))
    }

    /// Find all child lists with the given tag.
    pub fn find_all(&self, tag: &str) -> Vec<&SExpr> {
        self.children()
            .iter()
            .filter(|c| c.tag() == Some(tag))
            .collect()
    }

    /// Get the value of a simple (tag value) node.
    pub fn value(&self, tag: &str) -> Option<&str> {
        self.find(tag).and_then(|node| {
            node.children().first().and_then(|v| match v {
                SExpr::Atom(s) => Some(s.as_str()),
                _ => None,
            })
        })
    }

    /// Get the atom value (if this is an atom).
    pub fn as_atom(&self) -> Option<&str> {
        match self {
            SExpr::Atom(s) => Some(s.as_str()),
            _ => None,
        }
    }

    /// Parse a float from a simple (tag value) node.
    pub fn value_f64(&self, tag: &str) -> Option<f64> {
        self.value(tag).and_then(|v| v.parse().ok())
    }

    /// Parse an int from a simple (tag value) node.
    pub fn value_i64(&self, tag: &str) -> Option<i64> {
        self.value(tag).and_then(|v| v.parse().ok())
    }

    /// Get the nth atom child (0-indexed from children, i.e., after the tag).
    pub fn atom_at(&self, index: usize) -> Option<&str> {
        self.children().get(index).and_then(|v| v.as_atom())
    }

    /// Get the nth child as f64.
    pub fn f64_at(&self, index: usize) -> Option<f64> {
        self.atom_at(index).and_then(|v| v.parse().ok())
    }
}

struct Parser<'a> {
    input: &'a [u8],
    pos: usize,
}

impl<'a> Parser<'a> {
    fn new(input: &'a [u8]) -> Self {
        Self { input, pos: 0 }
    }

    fn skip_whitespace(&mut self) {
        while self.pos < self.input.len() {
            match self.input[self.pos] {
                b' ' | b'\t' | b'\n' | b'\r' => self.pos += 1,
                _ => break,
            }
        }
    }

    fn peek(&self) -> Option<u8> {
        self.input.get(self.pos).copied()
    }

    fn parse_string(&mut self) -> String {
        // Skip opening quote
        self.pos += 1;
        let start = self.pos;
        let mut result = String::new();
        while self.pos < self.input.len() {
            match self.input[self.pos] {
                b'"' => {
                    if result.is_empty() {
                        result = String::from_utf8_lossy(&self.input[start..self.pos]).into_owned();
                    }
                    self.pos += 1;
                    return result;
                }
                b'\\' => {
                    // Handle escape
                    if result.is_empty() {
                        result = String::from_utf8_lossy(&self.input[start..self.pos]).into_owned();
                    }
                    self.pos += 1;
                    if self.pos < self.input.len() {
                        result.push(self.input[self.pos] as char);
                        self.pos += 1;
                    }
                }
                _ => {
                    if !result.is_empty() {
                        result.push(self.input[self.pos] as char);
                    }
                    self.pos += 1;
                }
            }
        }
        result
    }

    fn parse_symbol(&mut self) -> String {
        let start = self.pos;
        while self.pos < self.input.len() {
            match self.input[self.pos] {
                b' ' | b'\t' | b'\n' | b'\r' | b'(' | b')' | b'"' => break,
                _ => self.pos += 1,
            }
        }
        String::from_utf8_lossy(&self.input[start..self.pos]).into_owned()
    }

    fn parse_sexpr(&mut self) -> Option<SExpr> {
        self.skip_whitespace();
        match self.peek()? {
            b'(' => {
                self.pos += 1;
                let mut items = Vec::new();
                loop {
                    self.skip_whitespace();
                    match self.peek() {
                        Some(b')') => {
                            self.pos += 1;
                            break;
                        }
                        None => break,
                        _ => {
                            if let Some(expr) = self.parse_sexpr() {
                                items.push(expr);
                            }
                        }
                    }
                }
                Some(SExpr::List(items))
            }
            b'"' => Some(SExpr::Atom(self.parse_string())),
            b')' => None,
            _ => Some(SExpr::Atom(self.parse_symbol())),
        }
    }
}

/// Parse an S-expression from bytes.
pub fn parse(input: &[u8]) -> Result<SExpr, String> {
    let mut parser = Parser::new(input);
    parser
        .parse_sexpr()
        .ok_or_else(|| "empty input".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_list() {
        let result = parse(b"(hello world)").unwrap();
        assert_eq!(result.tag(), Some("hello"));
        assert_eq!(result.atom_at(0), Some("world"));
    }

    #[test]
    fn test_nested() {
        let result = parse(b"(a (b 1) (c 2))").unwrap();
        assert_eq!(result.tag(), Some("a"));
        assert_eq!(result.value("b"), Some("1"));
        assert_eq!(result.value("c"), Some("2"));
    }

    #[test]
    fn test_string() {
        let result = parse(b"(layer \"F.Cu\")").unwrap();
        assert_eq!(result.value("layer"), None);
        assert_eq!(result.tag(), Some("layer"));
        assert_eq!(result.atom_at(0), Some("F.Cu"));
    }

    #[test]
    fn test_float() {
        let result = parse(b"(at 100.5 50.3 90)").unwrap();
        assert_eq!(result.f64_at(0), Some(100.5));
        assert_eq!(result.f64_at(1), Some(50.3));
        assert_eq!(result.f64_at(2), Some(90.0));
    }

    #[test]
    fn test_find_all() {
        let result = parse(b"(root (net 0 \"\") (net 1 \"GND\") (net 2 \"VCC\"))").unwrap();
        let nets = result.find_all("net");
        assert_eq!(nets.len(), 3);
    }
}
