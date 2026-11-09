//! A small, dependency-free JSON parser.
//!
//! `parse` is strict JSON (used for `--configFile`). `parse_lenient` additionally
//! accepts single-quoted strings, unquoted keys and trailing commas, which is what
//! people write inside `%%{init: {...}}%%` directives.

use std::fmt;

/// A parsed JSON value. Objects keep insertion order; lookups return the *last*
/// occurrence of a key, matching `JSON.parse` semantics.
#[derive(Debug, Clone, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Array(Vec<Json>),
    Object(Vec<(String, Json)>),
}

impl Json {
    /// Look up a key in an object.
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Object(pairs) => pairs.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::String(s) => Some(s),
            _ => None,
        }
    }

    /// Numbers, or strings holding numbers / CSS pixel values ("16px").
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Json::Number(n) => Some(*n),
            Json::String(s) => s.trim().trim_end_matches("px").trim().parse().ok(),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Json::Bool(b) => Some(*b),
            Json::String(s) if s == "true" => Some(true),
            Json::String(s) if s == "false" => Some(false),
            _ => None,
        }
    }

    pub fn as_object(&self) -> Option<&[(String, Json)]> {
        match self {
            Json::Object(p) => Some(p),
            _ => None,
        }
    }

    /// Render scalar values as strings (used for theme variables).
    pub fn to_scalar_string(&self) -> Option<String> {
        match self {
            Json::String(s) => Some(s.clone()),
            Json::Number(n) => Some(format_number(*n)),
            Json::Bool(b) => Some(b.to_string()),
            _ => None,
        }
    }
}

fn format_number(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e15 {
        format!("{}", n as i64)
    } else {
        n.to_string()
    }
}

/// Parse error with a byte offset.
#[derive(Debug, Clone, PartialEq)]
pub struct JsonError {
    pub offset: usize,
    pub message: String,
}

impl fmt::Display for JsonError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} at position {}", self.message, self.offset)
    }
}

impl std::error::Error for JsonError {}

/// Strict JSON.
pub fn parse(input: &str) -> Result<Json, JsonError> {
    Parser {
        s: input.as_bytes(),
        src: input,
        pos: 0,
        lenient: false,
    }
    .document()
}

/// JSON5-ish relaxed syntax for directives.
pub fn parse_lenient(input: &str) -> Result<Json, JsonError> {
    Parser {
        s: input.as_bytes(),
        src: input,
        pos: 0,
        lenient: true,
    }
    .document()
}

struct Parser<'a> {
    s: &'a [u8],
    src: &'a str,
    pos: usize,
    lenient: bool,
}

impl Parser<'_> {
    fn err<T>(&self, msg: &str) -> Result<T, JsonError> {
        Err(JsonError {
            offset: self.pos,
            message: msg.to_string(),
        })
    }

    fn document(mut self) -> Result<Json, JsonError> {
        self.ws();
        let v = self.value(0)?;
        self.ws();
        if self.pos != self.s.len() {
            return self.err("Unexpected trailing characters");
        }
        Ok(v)
    }

    fn peek(&self) -> Option<u8> {
        self.s.get(self.pos).copied()
    }

    fn ws(&mut self) {
        while let Some(c) = self.peek() {
            if c.is_ascii_whitespace() {
                self.pos += 1;
            } else {
                break;
            }
        }
    }

    fn value(&mut self, depth: usize) -> Result<Json, JsonError> {
        if depth > 128 {
            return self.err("Nesting too deep");
        }
        match self.peek() {
            Some(b'{') => self.object(depth),
            Some(b'[') => self.array(depth),
            Some(b'"') => self.string(b'"').map(Json::String),
            Some(b'\'') if self.lenient => self.string(b'\'').map(Json::String),
            Some(b't') => self.literal("true", Json::Bool(true)),
            Some(b'f') => self.literal("false", Json::Bool(false)),
            Some(b'n') => self.literal("null", Json::Null),
            Some(c) if c == b'-' || c.is_ascii_digit() || (self.lenient && c == b'.') => {
                self.number()
            }
            Some(_) => self.err("Unexpected character"),
            None => self.err("Unexpected end of input"),
        }
    }

    fn literal(&mut self, word: &str, v: Json) -> Result<Json, JsonError> {
        if self.src[self.pos..].starts_with(word) {
            self.pos += word.len();
            Ok(v)
        } else {
            self.err("Invalid literal")
        }
    }

    fn number(&mut self) -> Result<Json, JsonError> {
        let start = self.pos;
        while let Some(c) = self.peek() {
            if c.is_ascii_digit() || matches!(c, b'-' | b'+' | b'.' | b'e' | b'E') {
                self.pos += 1;
            } else {
                break;
            }
        }
        self.src[start..self.pos]
            .parse::<f64>()
            .map(Json::Number)
            .or_else(|_| self.err("Invalid number"))
    }

    fn string(&mut self, quote: u8) -> Result<String, JsonError> {
        self.pos += 1;
        let mut out = String::new();
        loop {
            let rest = &self.src[self.pos..];
            let Some(c) = rest.chars().next() else {
                return self.err("Unterminated string");
            };
            self.pos += c.len_utf8();
            match c {
                c if c as u32 == quote as u32 => return Ok(out),
                '\\' => {
                    let Some(e) = self.peek() else {
                        return self.err("Bad escape");
                    };
                    self.pos += 1;
                    match e {
                        b'"' => out.push('"'),
                        b'\'' => out.push('\''),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let cp = self.hex4()?;
                            let ch = if (0xD800..0xDC00).contains(&cp)
                                && self.src[self.pos..].starts_with("\\u")
                            {
                                self.pos += 2;
                                let lo = self.hex4()?;
                                char::from_u32(
                                    0x10000
                                        + ((cp - 0xD800) << 10)
                                        + (lo.wrapping_sub(0xDC00) & 0x3FF),
                                )
                            } else {
                                char::from_u32(cp)
                            };
                            out.push(ch.unwrap_or('\u{FFFD}'));
                        }
                        _ => return self.err("Bad escape"),
                    }
                }
                '\n' if !self.lenient => return self.err("Unescaped newline in string"),
                c => out.push(c),
            }
        }
    }

    fn hex4(&mut self) -> Result<u32, JsonError> {
        let h = self.src.get(self.pos..self.pos + 4).ok_or(JsonError {
            offset: self.pos,
            message: "Bad unicode escape".into(),
        })?;
        let v = u32::from_str_radix(h, 16).map_err(|_| JsonError {
            offset: self.pos,
            message: "Bad unicode escape".into(),
        })?;
        self.pos += 4;
        Ok(v)
    }

    fn key(&mut self) -> Result<String, JsonError> {
        match self.peek() {
            Some(b'"') => self.string(b'"'),
            Some(b'\'') if self.lenient => self.string(b'\''),
            Some(c) if self.lenient && (c.is_ascii_alphabetic() || c == b'_' || c == b'$') => {
                let start = self.pos;
                while let Some(c) = self.peek() {
                    if c.is_ascii_alphanumeric() || c == b'_' || c == b'$' || c == b'-' {
                        self.pos += 1;
                    } else {
                        break;
                    }
                }
                Ok(self.src[start..self.pos].to_string())
            }
            _ => self.err("Expected object key"),
        }
    }

    fn object(&mut self, depth: usize) -> Result<Json, JsonError> {
        self.pos += 1;
        let mut pairs = Vec::new();
        self.ws();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            return Ok(Json::Object(pairs));
        }
        loop {
            self.ws();
            if self.lenient && self.peek() == Some(b'}') {
                self.pos += 1;
                return Ok(Json::Object(pairs));
            }
            let k = self.key()?;
            self.ws();
            if self.peek() != Some(b':') {
                return self.err("Expected ':'");
            }
            self.pos += 1;
            self.ws();
            let v = self.value(depth + 1)?;
            pairs.push((k, v));
            self.ws();
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b'}') => {
                    self.pos += 1;
                    return Ok(Json::Object(pairs));
                }
                _ => return self.err("Expected ',' or '}'"),
            }
        }
    }

    fn array(&mut self, depth: usize) -> Result<Json, JsonError> {
        self.pos += 1;
        let mut items = Vec::new();
        self.ws();
        if self.peek() == Some(b']') {
            self.pos += 1;
            return Ok(Json::Array(items));
        }
        loop {
            self.ws();
            if self.lenient && self.peek() == Some(b']') {
                self.pos += 1;
                return Ok(Json::Array(items));
            }
            items.push(self.value(depth + 1)?);
            self.ws();
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b']') => {
                    self.pos += 1;
                    return Ok(Json::Array(items));
                }
                _ => return self.err("Expected ',' or ']'"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strict_roundtrip() {
        let v = parse(r#"{"theme":"dark","n":1.5,"a":[true,null],"s":"é\n"}"#).unwrap();
        assert_eq!(v.get("theme").unwrap().as_str(), Some("dark"));
        assert_eq!(v.get("n").unwrap().as_f64(), Some(1.5));
        assert_eq!(v.get("s").unwrap().as_str(), Some("é\n"));
    }

    #[test]
    fn strict_rejects_single_quotes() {
        assert!(parse("{'a':1}").is_err());
        assert!(parse("{\"a\":1,}").is_err());
    }

    #[test]
    fn lenient_accepts_directive_style() {
        let v =
            parse_lenient("{ 'theme': 'forest', themeVariables: { primaryColor: '#ff0000', }, }")
                .unwrap();
        assert_eq!(v.get("theme").unwrap().as_str(), Some("forest"));
        assert_eq!(
            v.get("themeVariables")
                .unwrap()
                .get("primaryColor")
                .unwrap()
                .as_str(),
            Some("#ff0000")
        );
    }

    #[test]
    fn last_key_wins() {
        let v = parse(r#"{"a":1,"a":2}"#).unwrap();
        assert_eq!(v.get("a").unwrap().as_f64(), Some(2.0));
    }
}
