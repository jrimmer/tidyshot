//! Minimal JSON value, parser, and serializer.
//!
//! The build is offline and no JSON crate is vendored, so the recording
//! journal, checkpoint, and sidecar metadata use this small implementation.
//! It covers the subset those files need: objects, arrays, strings, numbers,
//! booleans, and null.

use std::collections::BTreeMap;
use std::fmt;

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Array(Vec<Value>),
    Object(BTreeMap<String, Value>),
}

impl Value {
    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(s) => Some(s),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Number(n) => Some(*n),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// Python truthiness: null, zero, and empty strings, arrays and objects
    /// are false; every other value is true.
    #[must_use]
    pub fn is_truthy(&self) -> bool {
        match self {
            Value::Null => false,
            Value::Bool(b) => *b,
            Value::Number(n) => *n != 0.0,
            Value::String(s) => !s.is_empty(),
            Value::Array(a) => !a.is_empty(),
            Value::Object(o) => !o.is_empty(),
        }
    }

    #[must_use]
    pub fn as_array(&self) -> Option<&[Value]> {
        match self {
            Value::Array(a) => Some(a),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_object(&self) -> Option<&BTreeMap<String, Value>> {
        match self {
            Value::Object(o) => Some(o),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_object_mut(&mut self) -> Option<&mut BTreeMap<String, Value>> {
        match self {
            Value::Object(o) => Some(o),
            _ => None,
        }
    }

    /// Look up a single object key, as Python's `dict.get` does. A key that
    /// contains a literal `.` is matched exactly rather than split as a path.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.as_object()?.get(key)
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_string_compact())
    }
}

#[derive(Debug)]
pub struct JsonError {
    message: String,
    offset: usize,
}

impl JsonError {
    fn new(message: impl Into<String>, offset: usize) -> Self {
        Self {
            message: message.into(),
            offset,
        }
    }
}

impl fmt::Display for JsonError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} at byte {}", self.message, self.offset)
    }
}

impl std::error::Error for JsonError {}

impl Value {
    /// Serialize without whitespace, the form the checkpoint journal uses.
    #[must_use]
    pub fn to_string_compact(&self) -> String {
        let mut out = String::new();
        write_value(self, &mut out, None);
        out
    }

    /// Serialize with two-space indentation, matching the sidecar metadata.
    #[must_use]
    pub fn to_string_pretty(&self) -> String {
        let mut out = String::new();
        write_value(self, &mut out, Some(0));
        out
    }
}

fn write_value(value: &Value, out: &mut String, indent: Option<usize>) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(true) => out.push_str("true"),
        Value::Bool(false) => out.push_str("false"),
        Value::Number(n) => out.push_str(&format_number(*n)),
        Value::String(s) => write_string(s, out),
        Value::Array(items) => {
            if items.is_empty() {
                out.push_str("[]");
                return;
            }
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                if let Some(depth) = indent {
                    out.push('\n');
                    out.push_str(&"  ".repeat(depth + 1));
                    write_value(item, out, Some(depth + 1));
                } else {
                    write_value(item, out, None);
                }
            }
            if let Some(depth) = indent {
                out.push('\n');
                out.push_str(&"  ".repeat(depth));
            }
            out.push(']');
        }
        Value::Object(entries) => {
            if entries.is_empty() {
                out.push_str("{}");
                return;
            }
            out.push('{');
            for (index, (key, item)) in entries.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                if let Some(depth) = indent {
                    out.push('\n');
                    out.push_str(&"  ".repeat(depth + 1));
                    write_string(key, out);
                    out.push_str(": ");
                    write_value(item, out, Some(depth + 1));
                } else {
                    write_string(key, out);
                    out.push(':');
                    write_value(item, out, None);
                }
            }
            if let Some(depth) = indent {
                out.push('\n');
                out.push_str(&"  ".repeat(depth));
            }
            out.push('}');
        }
    }
}

fn write_string(value: &str, out: &mut String) {
    out.push('"');
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Render numbers without a trailing `.0` for integral values, so a file's
/// `version: 1` does not churn between reads and writes.
fn format_number(n: f64) -> String {
    if n.is_finite() && n.fract() == 0.0 && n.abs() < 9e15 {
        format!("{}", n as i64)
    } else {
        format!("{n}")
    }
}

/// Parse a complete JSON document, rejecting trailing content.
pub fn parse(input: &str) -> Result<Value, JsonError> {
    let mut parser = Parser {
        bytes: input.as_bytes(),
        pos: 0,
    };
    parser.skip_whitespace();
    let value = parser.parse_value()?;
    parser.skip_whitespace();
    if parser.pos != parser.bytes.len() {
        return Err(JsonError::new("trailing content", parser.pos));
    }
    Ok(value)
}

struct Parser<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl Parser<'_> {
    fn skip_whitespace(&mut self) {
        while let Some(&b) = self.bytes.get(self.pos) {
            if b == b' ' || b == b'\t' || b == b'\n' || b == b'\r' {
                self.pos += 1;
            } else {
                break;
            }
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn parse_value(&mut self) -> Result<Value, JsonError> {
        match self.peek() {
            Some(b'{') => self.parse_object(),
            Some(b'[') => self.parse_array(),
            Some(b'"') => Ok(Value::String(self.parse_string()?)),
            Some(b't') => self.parse_literal("true", Value::Bool(true)),
            Some(b'f') => self.parse_literal("false", Value::Bool(false)),
            Some(b'n') => self.parse_literal("null", Value::Null),
            Some(b'-' | b'0'..=b'9') => self.parse_number(),
            Some(_) => Err(JsonError::new("unexpected character", self.pos)),
            None => Err(JsonError::new("unexpected end of input", self.pos)),
        }
    }

    fn parse_literal(&mut self, text: &str, value: Value) -> Result<Value, JsonError> {
        if self.bytes[self.pos..].starts_with(text.as_bytes()) {
            self.pos += text.len();
            Ok(value)
        } else {
            Err(JsonError::new("invalid literal", self.pos))
        }
    }

    fn parse_object(&mut self) -> Result<Value, JsonError> {
        self.pos += 1; // {
        let mut entries = BTreeMap::new();
        self.skip_whitespace();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            return Ok(Value::Object(entries));
        }
        loop {
            self.skip_whitespace();
            let key = self.parse_string()?;
            self.skip_whitespace();
            if self.peek() != Some(b':') {
                return Err(JsonError::new("expected ':'", self.pos));
            }
            self.pos += 1;
            self.skip_whitespace();
            let value = self.parse_value()?;
            entries.insert(key, value);
            self.skip_whitespace();
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b'}') => {
                    self.pos += 1;
                    return Ok(Value::Object(entries));
                }
                _ => return Err(JsonError::new("expected ',' or '}'", self.pos)),
            }
        }
    }

    fn parse_array(&mut self) -> Result<Value, JsonError> {
        self.pos += 1; // [
        let mut items = Vec::new();
        self.skip_whitespace();
        if self.peek() == Some(b']') {
            self.pos += 1;
            return Ok(Value::Array(items));
        }
        loop {
            self.skip_whitespace();
            items.push(self.parse_value()?);
            self.skip_whitespace();
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b']') => {
                    self.pos += 1;
                    return Ok(Value::Array(items));
                }
                _ => return Err(JsonError::new("expected ',' or ']'", self.pos)),
            }
        }
    }

    fn parse_string(&mut self) -> Result<String, JsonError> {
        if self.peek() != Some(b'"') {
            return Err(JsonError::new("expected string", self.pos));
        }
        self.pos += 1;
        let mut out = String::new();
        loop {
            match self.peek() {
                None => return Err(JsonError::new("unterminated string", self.pos)),
                Some(b'"') => {
                    self.pos += 1;
                    return Ok(out);
                }
                Some(b'\\') => {
                    self.pos += 1;
                    let escape = self
                        .peek()
                        .ok_or_else(|| JsonError::new("unterminated escape", self.pos))?;
                    self.pos += 1;
                    match escape {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{0008}'),
                        b'f' => out.push('\u{000c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let first = self.parse_hex4()?;
                            if (0xD800..=0xDBFF).contains(&first) {
                                // A high surrogate must be followed by a low one.
                                if self.peek() != Some(b'\\') {
                                    return Err(JsonError::new("lone high surrogate", self.pos));
                                }
                                self.pos += 1;
                                if self.peek() != Some(b'u') {
                                    return Err(JsonError::new(
                                        "high surrogate without low surrogate",
                                        self.pos,
                                    ));
                                }
                                self.pos += 1;
                                let second = self.parse_hex4()?;
                                if !(0xDC00..=0xDFFF).contains(&second) {
                                    return Err(JsonError::new("invalid low surrogate", self.pos));
                                }
                                let code = 0x1_0000 + ((first - 0xD800) << 10) + (second - 0xDC00);
                                let c = char::from_u32(code).ok_or_else(|| {
                                    JsonError::new("invalid code point", self.pos)
                                })?;
                                out.push(c);
                            } else {
                                let c = char::from_u32(first).ok_or_else(|| {
                                    JsonError::new("invalid code point", self.pos)
                                })?;
                                out.push(c);
                            }
                        }
                        _ => return Err(JsonError::new("invalid escape", self.pos)),
                    }
                }
                Some(_) => {
                    let rest = std::str::from_utf8(&self.bytes[self.pos..])
                        .map_err(|_| JsonError::new("invalid utf-8", self.pos))?;
                    let c = rest
                        .chars()
                        .next()
                        .ok_or_else(|| JsonError::new("invalid utf-8", self.pos))?;
                    out.push(c);
                    self.pos += c.len_utf8();
                }
            }
        }
    }

    fn parse_hex4(&mut self) -> Result<u32, JsonError> {
        if self.pos + 4 > self.bytes.len() {
            return Err(JsonError::new("truncated unicode escape", self.pos));
        }
        let hex = std::str::from_utf8(&self.bytes[self.pos..self.pos + 4])
            .map_err(|_| JsonError::new("invalid unicode escape", self.pos))?;
        let code = u32::from_str_radix(hex, 16)
            .map_err(|_| JsonError::new("invalid unicode escape", self.pos))?;
        self.pos += 4;
        Ok(code)
    }

    fn parse_number(&mut self) -> Result<Value, JsonError> {
        let start = self.pos;
        if self.peek() == Some(b'-') {
            self.pos += 1;
        }
        while let Some(b) = self.peek() {
            if b.is_ascii_digit() || b == b'.' || b == b'e' || b == b'E' || b == b'+' || b == b'-' {
                self.pos += 1;
            } else {
                break;
            }
        }
        let text = std::str::from_utf8(&self.bytes[start..self.pos])
            .map_err(|_| JsonError::new("invalid number", start))?;
        text.parse::<f64>()
            .map(Value::Number)
            .map_err(|_| JsonError::new("invalid number", start))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_nested_documents() {
        let source = r#"{"version":1,"cursor":[{"t":0.2,"x":0.1}],"flag":true,"none":null}"#;
        let value = parse(source).unwrap();
        assert_eq!(value.get("version").unwrap().as_f64(), Some(1.0));
        assert_eq!(value.get("cursor").unwrap().as_array().unwrap().len(), 1);
        assert_eq!(
            parse(&value.to_string_compact()).unwrap(),
            parse(source).unwrap()
        );
    }

    #[test]
    fn integral_numbers_do_not_grow_a_decimal_tail() {
        let value = parse("1").unwrap();
        assert_eq!(value.to_string_compact(), "1");
    }

    #[test]
    fn rejects_trailing_content() {
        assert!(parse("{} {}").is_err());
    }

    #[test]
    fn literal_dots_in_keys_are_not_paths() {
        let value = parse(r#"{"a.b":1}"#).unwrap();
        assert_eq!(value.get("a.b").unwrap().as_f64(), Some(1.0));
    }

    #[test]
    fn parses_escapes() {
        let value = parse(r#""a\nb\u0041""#).unwrap();
        assert_eq!(value.as_str(), Some("a\nbA"));
    }

    #[test]
    fn parses_surrogate_pairs() {
        assert_eq!(parse(r#""\uD83D\uDE00""#).unwrap().as_str(), Some("😀"));
        assert_eq!(parse(r#""\ud83d\ude00""#).unwrap().as_str(), Some("😀"));
        assert_eq!(parse(r#""\uD83D\uDE00a""#).unwrap().as_str(), Some("😀a"));
        // A lone or malformed surrogate is rejected, not silently dropped.
        assert!(parse(r#""\uD83D""#).is_err());
        assert!(parse(r#""\uD83D\u0041""#).is_err());
    }
}
