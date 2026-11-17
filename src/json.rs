//! A JSON reader that keeps the line of every value and of every field, for `.fsm` files: their
//! problems are reported with their lines. Strict JSON (RFC 8259), plus a byte order mark at the start, which is
//! skipped. A field given twice in one object is an error.

use std::fmt::Write as _;

#[derive(Debug, Clone, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Array(Vec<Node>),
    Object(Vec<Field>),
}

/// A value and the line it starts on (1-based).
#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    pub value: Json,
    pub line: usize,
}

/// A field of an object: its name, the line of the name, its value.
#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    pub key: String,
    pub line: usize,
    pub value: Node,
}

/// A syntax error and its line.
#[derive(Debug, Clone, PartialEq)]
pub struct SyntaxError {
    pub line: usize,
    pub message: String,
}

impl Node {
    pub fn str(&self) -> Option<&str> {
        match &self.value {
            Json::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn fields(&self) -> Option<&[Field]> {
        match &self.value {
            Json::Object(f) => Some(f),
            _ => None,
        }
    }

    pub fn items(&self) -> Option<&[Node]> {
        match &self.value {
            Json::Array(a) => Some(a),
            _ => None,
        }
    }

    pub fn number(&self) -> Option<f64> {
        match &self.value {
            Json::Number(n) => Some(*n),
            _ => None,
        }
    }

    /// The field `key` of an object.
    pub fn get(&self, key: &str) -> Option<&Field> {
        self.fields()?.iter().find(|f| f.key == key)
    }
}

struct Parser<'a> {
    s: &'a [u8],
    text: &'a str,
    i: usize,
    line: usize,
}

/// How a character is shown in a message: itself, or `\uXXXX` for control and invisible characters.
fn show(c: char) -> String {
    if c.is_control() || c == '\u{feff}' { format!("\\u{:04x}", c as u32) } else { c.to_string() }
}

impl Parser<'_> {
    fn error<T>(&self, message: impl Into<String>) -> Result<T, SyntaxError> {
        Err(SyntaxError { line: self.line, message: message.into() })
    }

    fn unexpected<T>(&self) -> Result<T, SyntaxError> {
        match self.text[self.i..].chars().next() {
            None => self.error("Unexpected end of the file."),
            Some(c) => self.error(format!("Unexpected character '{}'.", show(c))),
        }
    }

    fn space(&mut self) {
        while self.i < self.s.len() {
            match self.s[self.i] {
                b'\n' => self.line += 1,
                b' ' | b'\t' | b'\r' => {}
                _ => return,
            }
            self.i += 1;
        }
    }

    fn value(&mut self, depth: usize) -> Result<Node, SyntaxError> {
        if depth > 200 {
            return self.error("The document is nested too deeply.");
        }
        self.space();
        let line = self.line;
        let value = match self.s.get(self.i) {
            None => return self.unexpected(),
            Some(b'{') => {
                self.i += 1;
                let mut fields: Vec<Field> = Vec::new();
                self.space();
                if self.s.get(self.i) == Some(&b'}') {
                    self.i += 1;
                } else {
                    loop {
                        self.space();
                        if self.s.get(self.i) != Some(&b'"') {
                            return self.unexpected();
                        }
                        let key_line = self.line;
                        let key = self.string()?;
                        if fields.iter().any(|f| f.key == key) {
                            return Err(SyntaxError {
                                line: key_line,
                                message: format!("Field '{key}' is given twice."),
                            });
                        }
                        self.space();
                        if self.s.get(self.i) != Some(&b':') {
                            return self.unexpected();
                        }
                        self.i += 1;
                        let value = self.value(depth + 1)?;
                        fields.push(Field { key, line: key_line, value });
                        self.space();
                        match self.s.get(self.i) {
                            Some(b',') => self.i += 1,
                            Some(b'}') => {
                                self.i += 1;
                                break;
                            }
                            _ => return self.unexpected(),
                        }
                    }
                }
                Json::Object(fields)
            }
            Some(b'[') => {
                self.i += 1;
                let mut items = Vec::new();
                self.space();
                if self.s.get(self.i) == Some(&b']') {
                    self.i += 1;
                } else {
                    loop {
                        items.push(self.value(depth + 1)?);
                        self.space();
                        match self.s.get(self.i) {
                            Some(b',') => self.i += 1,
                            Some(b']') => {
                                self.i += 1;
                                break;
                            }
                            _ => return self.unexpected(),
                        }
                    }
                }
                Json::Array(items)
            }
            Some(b'"') => Json::String(self.string()?),
            Some(b't') => self.word("true", Json::Bool(true))?,
            Some(b'f') => self.word("false", Json::Bool(false))?,
            Some(b'n') => self.word("null", Json::Null)?,
            Some(b'-' | b'0'..=b'9') => Json::Number(self.number()?),
            Some(_) => return self.unexpected(),
        };
        Ok(Node { value, line })
    }

    fn word(&mut self, w: &str, v: Json) -> Result<Json, SyntaxError> {
        if self.text[self.i..].starts_with(w) {
            self.i += w.len();
            Ok(v)
        } else {
            self.unexpected()
        }
    }

    fn number(&mut self) -> Result<f64, SyntaxError> {
        let start = self.i;
        let digit = |p: &Parser, i: usize| p.s.get(i).is_some_and(|b| b.is_ascii_digit());
        if self.s[self.i] == b'-' {
            self.i += 1;
        }
        if !digit(self, self.i) {
            return self.error("Invalid number.");
        }
        if self.s[self.i] == b'0' {
            self.i += 1;
            if digit(self, self.i) {
                return self.error("Invalid number.");
            }
        } else {
            while digit(self, self.i) {
                self.i += 1;
            }
        }
        if self.s.get(self.i) == Some(&b'.') {
            self.i += 1;
            if !digit(self, self.i) {
                return self.error("Invalid number.");
            }
            while digit(self, self.i) {
                self.i += 1;
            }
        }
        if matches!(self.s.get(self.i), Some(b'e' | b'E')) {
            self.i += 1;
            if matches!(self.s.get(self.i), Some(b'+' | b'-')) {
                self.i += 1;
            }
            if !digit(self, self.i) {
                return self.error("Invalid number.");
            }
            while digit(self, self.i) {
                self.i += 1;
            }
        }
        match self.text[start..self.i].parse::<f64>() {
            Ok(n) if n.is_finite() => Ok(n),
            _ => self.error("Invalid number."),
        }
    }

    fn hex4(&mut self) -> Result<u32, SyntaxError> {
        let h = self.text.get(self.i..self.i + 4).filter(|h| h.bytes().all(|b| b.is_ascii_hexdigit()));
        match h {
            Some(h) => {
                self.i += 4;
                Ok(u32::from_str_radix(h, 16).unwrap())
            }
            None => self.error("Invalid escape in a text."),
        }
    }

    fn string(&mut self) -> Result<String, SyntaxError> {
        self.i += 1; // the opening quote
        let mut out = String::new();
        loop {
            let Some(c) = self.text[self.i..].chars().next() else {
                return self.error("Unexpected end of the file in a text.");
            };
            match c {
                '"' => {
                    self.i += 1;
                    return Ok(out);
                }
                '\\' => {
                    self.i += 1;
                    let e = self.s.get(self.i).copied();
                    self.i += 1;
                    match e {
                        Some(b'"') => out.push('"'),
                        Some(b'\\') => out.push('\\'),
                        Some(b'/') => out.push('/'),
                        Some(b'b') => out.push('\u{8}'),
                        Some(b'f') => out.push('\u{c}'),
                        Some(b'n') => out.push('\n'),
                        Some(b'r') => out.push('\r'),
                        Some(b't') => out.push('\t'),
                        Some(b'u') => {
                            let u = self.hex4()?;
                            let code = if (0xd800..0xdc00).contains(&u) {
                                // A surrogate pair, or nothing.
                                if !self.text[self.i..].starts_with("\\u") {
                                    return self.error("Invalid escape in a text.");
                                }
                                self.i += 2;
                                let low = self.hex4()?;
                                if !(0xdc00..0xe000).contains(&low) {
                                    return self.error("Invalid escape in a text.");
                                }
                                0x10000 + ((u - 0xd800) << 10) + (low - 0xdc00)
                            } else {
                                u
                            };
                            match char::from_u32(code) {
                                Some(ch) => out.push(ch),
                                None => return self.error("Invalid escape in a text."),
                            }
                        }
                        _ => return self.error("Invalid escape in a text."),
                    }
                }
                c if (c as u32) < 0x20 => {
                    return self.error(format!("Control character '{}' in a text: write it as an escape.", show(c)));
                }
                c => {
                    out.push(c);
                    self.i += c.len_utf8();
                }
            }
        }
    }
}

/// Parses a JSON document.
pub fn parse(text: &str) -> Result<Node, SyntaxError> {
    let mut p = Parser { s: text.as_bytes(), text, i: 0, line: 1 };
    if text.starts_with('\u{feff}') {
        p.i = 3;
    }
    let node = p.value(0)?;
    p.space();
    if p.i < p.s.len() {
        return p.unexpected();
    }
    Ok(node)
}

/// A text as JSON writes it: minimal escapes (`\"`, `\\`, `\n`, `\t`, `\uXXXX` for other control characters), every
/// other character as it is.
pub fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A number as JavaScript's `String(n)` writes it (finite).
pub fn number(n: f64) -> String {
    if n == 0.0 {
        return "0".to_string();
    }
    if n.abs() >= 1e21 || n.abs() < 1e-6 {
        // Exponential, the shortest digits as Rust's: `1e+21`, `1.5e-7`.
        let s = format!("{n:e}");
        return match s.split_once('e') {
            Some((m, e)) if !e.starts_with('-') => format!("{m}e+{e}"),
            _ => s,
        };
    }
    if n.fract() == 0.0 {
        return format!("{n:.0}");
    }
    // Rust's shortest representation is JavaScript's between 1e-6 and 1e21.
    format!("{n}")
}

/// The value without its lines, compact, with the fields of every object sorted by name: what the model hash is
/// computed from, so that indentation, field order and escapes don't change it.
pub fn canonical(node: &Node, out: &mut String) {
    match &node.value {
        Json::Null => out.push_str("null"),
        Json::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Json::Number(n) => out.push_str(&number(*n)),
        Json::String(s) => out.push_str(&quote(s)),
        Json::Array(items) => {
            out.push('[');
            for (i, x) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                canonical(x, out);
            }
            out.push(']');
        }
        Json::Object(fields) => {
            let mut sorted: Vec<&Field> = fields.iter().collect();
            sorted.sort_by(|a, b| a.key.cmp(&b.key));
            out.push('{');
            for (i, f) in sorted.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&quote(&f.key));
                out.push(':');
                canonical(&f.value, out);
            }
            out.push('}');
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn err(text: &str) -> (usize, String) {
        let e = parse(text).unwrap_err();
        (e.line, e.message)
    }

    #[test]
    fn values_and_lines() {
        let n = parse("\u{feff}{\n  \"a\": [1, -2.5e1, true, null],\r\n  \"b\": \"x\\u00e9\\ud83d\\ude00\\n\"\n}\n")
            .unwrap();
        assert_eq!(n.line, 1);
        let a = n.get("a").unwrap();
        assert_eq!(a.line, 2);
        assert_eq!(a.value.items().unwrap()[1].number(), Some(-25.0));
        let b = n.get("b").unwrap();
        assert_eq!((b.line, b.value.line), (3, 3));
        assert_eq!(b.value.str(), Some("xé😀\n"));
    }

    #[test]
    fn errors() {
        assert_eq!(err(""), (1, "Unexpected end of the file.".into()));
        assert_eq!(err("{\n\"a\": 1,\n}"), (3, "Unexpected character '}'.".into()));
        assert_eq!(err("{\"a\": 01}"), (1, "Invalid number.".into()));
        assert_eq!(err("{\"a\": 1, \"a\": 2}"), (1, "Field 'a' is given twice.".into()));
        assert_eq!(err("\"\\x\""), (1, "Invalid escape in a text.".into()));
        assert_eq!(err("\"\\ud800\""), (1, "Invalid escape in a text.".into()));
        assert_eq!(err("\"a\nb\""), (1, "Control character '\\u000a' in a text: write it as an escape.".into()));
        assert_eq!(err("\"ab"), (1, "Unexpected end of the file in a text.".into()));
        assert_eq!(err("{} x"), (1, "Unexpected character 'x'.".into()));
        assert_eq!(err("[1e400]"), (1, "Invalid number.".into()));
        assert_eq!(err("{\n\n'a'"), (3, "Unexpected character '''.".into()));
    }

    #[test]
    fn quotes_and_numbers() {
        assert_eq!(quote("a\"b\\c\nd\te\u{1}é/"), "\"a\\\"b\\\\c\\nd\\te\\u0001é/\"");
        assert_eq!(number(12.0), "12");
        assert_eq!(number(-0.5), "-0.5");
        assert_eq!(number(0.1 + 0.2), "0.30000000000000004");
        // JavaScript's String(n) from 1e21 up and below 1e-6.
        assert_eq!(number(1e21), "1e+21");
        assert_eq!(number(-1.5e300), "-1.5e+300");
        assert_eq!(number(1e20), "100000000000000000000");
        assert_eq!(number(0.000001), "0.000001");
        assert_eq!(number(1.5e-7), "1.5e-7");
        let mut c = String::new();
        canonical(&parse("{ \"b\": [1, \"x\"], \"a\": {} }").unwrap(), &mut c);
        assert_eq!(c, "{\"a\":{},\"b\":[1,\"x\"]}");
    }
}
