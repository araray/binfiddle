//! Strict JSON for the NN wire subset, plus RFC 8785 canonical serialization.
//!
//! The NN wire subset is number-free: integers travel as canonical decimal
//! strings and non-finite or precision-sensitive values travel as objects with
//! an explicit encoding. Bare JSON numbers are therefore rejected by
//! [`Json::parse_strict`], and the canonical serializer never emits them.
//!
//! Foreign artifacts (container headers) do use JSON integers. Those are read
//! with [`Json::parse_foreign`], which produces [`Json::Number`] values for
//! non-negative integers only (fractional and exponent forms are rejected as
//! unsupported for the artifact formats being read). `Number` is a read-only
//! convenience for foreign data: canonical serialization refuses it, so a
//! number can never leak into a wire record.
//!
//! Both parsers reject duplicate object keys (they must be caught before
//! schema validation) and enforce explicit depth and node-count limits.

use super::error::NnError;

/// A parsed JSON value. `Number` appears only through `parse_foreign`.
#[derive(Debug, Clone, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Str(String),
    /// Non-negative integer in canonical decimal form, from foreign JSON.
    Number(String),
    Array(Vec<Json>),
    /// Object members in first-appearance order; duplicates are rejected by the
    /// parser and by the canonical serializer.
    Object(Vec<(String, Json)>),
}

impl Json {
    /// Build an object from pairs, rejecting duplicate keys eagerly.
    pub fn object(pairs: Vec<(impl Into<String>, Json)>) -> Result<Json, NnError> {
        let mut members: Vec<(String, Json)> = Vec::with_capacity(pairs.len());
        for (key, value) in pairs {
            let key = key.into();
            if members.iter().any(|(k, _)| *k == key) {
                return Err(NnError::WireSyntax {
                    detail: format!("duplicate object key {}", super::error::brief(&key)),
                });
            }
            members.push((key, value));
        }
        Ok(Json::Object(members))
    }

    /// Look up a member of an object.
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Object(members) => members.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    /// Look up an item of an array by position.
    pub fn at(&self, index: usize) -> Option<&Json> {
        match self {
            Json::Array(items) => items.get(index),
            _ => None,
        }
    }

    /// String value of a `Str`, or `None` for other variants.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }

    /// Items of an `Array`, or `None` for other variants.
    pub fn as_array(&self) -> Option<&[Json]> {
        match self {
            Json::Array(items) => Some(items),
            _ => None,
        }
    }

    /// Parse a complete document from a UTF-8 string with the given limits.
    /// Trailing non-whitespace content after the top-level value is an error.
    /// Bare numbers are rejected (NN wire subset).
    pub fn parse_strict(input: &str, limits: ParseLimits) -> Result<Json, NnError> {
        Self::parse_with_mode(input, limits, false)
    }

    /// Parse a foreign JSON document (container headers). Non-negative
    /// integers are accepted and represented as [`Json::Number`]; fractional
    /// and exponent forms are rejected because no supported artifact format
    /// uses them, and negative numbers likewise.
    pub fn parse_foreign(input: &str, limits: ParseLimits) -> Result<Json, NnError> {
        Self::parse_with_mode(input, limits, true)
    }

    fn parse_with_mode(
        input: &str,
        limits: ParseLimits,
        allow_numbers: bool,
    ) -> Result<Json, NnError> {
        let mut parser = Parser {
            bytes: input.as_bytes(),
            pos: 0,
            limits,
            depth: 0,
            nodes: 0,
            allow_numbers,
        };
        parser.skip_ws();
        let value = parser.parse_value()?;
        parser.skip_ws();
        if parser.pos != parser.bytes.len() {
            return Err(parser.error("trailing content after top-level value"));
        }
        Ok(value)
    }

    /// For a [`Json::Number`], return its value as `u64`. Returns `None` for
    /// other variants or out-of-range digits.
    pub fn as_number_u64(&self) -> Option<u64> {
        match self {
            Json::Number(digits) => digits.parse::<u64>().ok(),
            _ => None,
        }
    }

    /// Serialize to the canonical (RFC 8785 subset) form: object keys sorted by
    /// UTF-16 code units, JCS string escaping, no insignificant whitespace.
    /// Duplicate keys anywhere in the value are an error. `Number` values
    /// cannot be canonicalized (the wire subset is number-free).
    pub fn to_canonical(&self) -> Result<String, NnError> {
        let mut out = String::new();
        write_canonical(self, &mut out)?;
        Ok(out)
    }

    /// Canonical SHA-256 digest of this value (lowercase hex).
    pub fn canonical_digest(&self) -> Result<String, NnError> {
        let canonical = self.to_canonical()?;
        use sha2::{Digest, Sha256};
        let digest = Sha256::digest(canonical.as_bytes());
        Ok(hex::encode(digest))
    }
}

impl From<&str> for Json {
    fn from(value: &str) -> Self {
        Json::Str(value.to_string())
    }
}

impl From<String> for Json {
    fn from(value: String) -> Self {
        Json::Str(value)
    }
}

impl From<bool> for Json {
    fn from(value: bool) -> Self {
        Json::Bool(value)
    }
}

impl From<Vec<Json>> for Json {
    fn from(value: Vec<Json>) -> Self {
        Json::Array(value)
    }
}

/// Parser limits. These are hard safety boundaries, not tuning hints.
#[derive(Debug, Clone, Copy)]
pub struct ParseLimits {
    /// Maximum object/array nesting depth.
    pub max_depth: usize,
    /// Maximum number of parsed values (including containers).
    pub max_nodes: usize,
}

impl Default for ParseLimits {
    fn default() -> Self {
        ParseLimits {
            max_depth: 64,
            max_nodes: 1_000_000,
        }
    }
}

struct Parser<'a> {
    bytes: &'a [u8],
    pos: usize,
    limits: ParseLimits,
    depth: usize,
    nodes: usize,
    allow_numbers: bool,
}

impl<'a> Parser<'a> {
    fn error(&self, message: &str) -> NnError {
        NnError::WireSyntax {
            detail: format!("JSON parse error at byte {}: {}", self.pos, message),
        }
    }

    fn skip_ws(&mut self) {
        while let Some(&b) = self.bytes.get(self.pos) {
            if matches!(b, b' ' | b'\t' | b'\n' | b'\r') {
                self.pos += 1;
            } else {
                break;
            }
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn bump(&mut self) -> Option<u8> {
        let b = self.peek();
        if b.is_some() {
            self.pos += 1;
        }
        b
    }

    fn expect(&mut self, expected: u8) -> Result<(), NnError> {
        if self.peek() == Some(expected) {
            self.pos += 1;
            Ok(())
        } else {
            Err(self.error(&format!("expected '{}'", expected as char)))
        }
    }

    fn count_node(&mut self) -> Result<(), NnError> {
        self.nodes = self
            .nodes
            .checked_add(1)
            .ok_or_else(|| self.error("node counter overflow"))?;
        if self.nodes > self.limits.max_nodes {
            return Err(self.error("node count exceeds limit"));
        }
        Ok(())
    }

    fn parse_value(&mut self) -> Result<Json, NnError> {
        self.count_node()?;
        match self.peek() {
            Some(b'{') => self.parse_object(),
            Some(b'[') => self.parse_array(),
            Some(b'"') => Ok(Json::Str(self.parse_string()?)),
            Some(b't') => self.parse_literal("true", Json::Bool(true)),
            Some(b'f') => self.parse_literal("false", Json::Bool(false)),
            Some(b'n') => self.parse_literal("null", Json::Null),
            Some(b'0'..=b'9') if self.allow_numbers => self.parse_foreign_number(),
            Some(b'-') | Some(b'0'..=b'9') => Err(self.error(
                "JSON numbers are not part of the NN wire subset; encode integers as decimal strings",
            )),
            Some(_) => Err(self.error("unexpected character")),
            None => Err(self.error("unexpected end of input")),
        }
    }

    /// Parse a JSON integer literal in foreign mode: non-negative, canonical
    /// digits (JSON forbids leading zeros), no fraction or exponent. Those
    /// restrictions cover every supported artifact header format.
    fn parse_foreign_number(&mut self) -> Result<Json, NnError> {
        let start = self.pos;
        if self.peek() == Some(b'0') {
            self.pos += 1;
            if let Some(b'0'..=b'9') = self.peek() {
                return Err(self.error("leading zeros are not valid JSON numbers"));
            }
        } else {
            while let Some(b'0'..=b'9') = self.peek() {
                self.pos += 1;
            }
        }
        match self.peek() {
            Some(b'.') | Some(b'e') | Some(b'E') => {
                Err(self
                    .error("fractional or exponent numbers are unsupported in artifact headers"))
            }
            _ => Ok(Json::Number(
                std::str::from_utf8(&self.bytes[start..self.pos])
                    .map_err(|_| self.error("invalid UTF-8 in number"))?
                    .to_string(),
            )),
        }
    }

    fn parse_literal(&mut self, text: &str, value: Json) -> Result<Json, NnError> {
        if self.bytes[self.pos..].starts_with(text.as_bytes()) {
            self.pos += text.len();
            Ok(value)
        } else {
            Err(self.error(&format!("invalid literal (expected '{text}')")))
        }
    }

    fn parse_object(&mut self) -> Result<Json, NnError> {
        self.depth += 1;
        if self.depth > self.limits.max_depth {
            return Err(self.error("nesting depth exceeds limit"));
        }
        self.expect(b'{')?;
        let mut members: Vec<(String, Json)> = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            self.depth -= 1;
            return Ok(Json::Object(members));
        }
        loop {
            self.skip_ws();
            let key = self.parse_string()?;
            self.skip_ws();
            self.expect(b':')?;
            self.skip_ws();
            let value = self.parse_value()?;
            if members.iter().any(|(k, _)| *k == key) {
                self.pos -= 1;
                return Err(self.error(&format!(
                    "duplicate object key {}",
                    super::error::brief(&key)
                )));
            }
            members.push((key, value));
            self.skip_ws();
            match self.bump() {
                Some(b',') => continue,
                Some(b'}') => break,
                _ => {
                    self.pos -= 1;
                    return Err(self.error("expected ',' or '}' in object"));
                }
            }
        }
        self.depth -= 1;
        Ok(Json::Object(members))
    }

    fn parse_array(&mut self) -> Result<Json, NnError> {
        self.depth += 1;
        if self.depth > self.limits.max_depth {
            return Err(self.error("nesting depth exceeds limit"));
        }
        self.expect(b'[')?;
        let mut items: Vec<Json> = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b']') {
            self.pos += 1;
            self.depth -= 1;
            return Ok(Json::Array(items));
        }
        loop {
            self.skip_ws();
            items.push(self.parse_value()?);
            self.skip_ws();
            match self.bump() {
                Some(b',') => continue,
                Some(b']') => break,
                _ => {
                    self.pos -= 1;
                    return Err(self.error("expected ',' or ']' in array"));
                }
            }
        }
        self.depth -= 1;
        Ok(Json::Array(items))
    }

    fn parse_string(&mut self) -> Result<String, NnError> {
        self.expect(b'"')?;
        let mut out = String::new();
        loop {
            let start = self.pos;
            // Fast path: consume a run of plain bytes.
            while let Some(b) = self.peek() {
                if b == b'"' || b == b'\\' || b < 0x20 {
                    break;
                }
                self.pos += 1;
            }
            if self.pos > start {
                // Input is a &str, so this range is valid UTF-8.
                out.push_str(
                    std::str::from_utf8(&self.bytes[start..self.pos])
                        .map_err(|_| self.error("invalid UTF-8 in string"))?,
                );
            }
            match self.bump() {
                Some(b'"') => return Ok(out),
                Some(b'\\') => match self.bump() {
                    Some(b'"') => out.push('"'),
                    Some(b'\\') => out.push('\\'),
                    Some(b'/') => out.push('/'),
                    Some(b'b') => out.push('\u{0008}'),
                    Some(b'f') => out.push('\u{000C}'),
                    Some(b'n') => out.push('\n'),
                    Some(b'r') => out.push('\r'),
                    Some(b't') => out.push('\t'),
                    Some(b'u') => {
                        let first = self.parse_hex4()?;
                        let code = if (0xD800..=0xDBFF).contains(&first) {
                            // High surrogate: must pair with a low surrogate.
                            if self.peek() != Some(b'\\') {
                                return Err(self.error("lone high surrogate in string"));
                            }
                            self.pos += 1;
                            if self.peek() != Some(b'u') {
                                return Err(self.error("lone high surrogate in string"));
                            }
                            self.pos += 1;
                            let second = self.parse_hex4()?;
                            if !(0xDC00..=0xDFFF).contains(&second) {
                                return Err(self.error("invalid surrogate pair in string"));
                            }
                            0x10000 + ((first - 0xD800) << 10) + (second - 0xDC00)
                        } else if (0xDC00..=0xDFFF).contains(&first) {
                            return Err(self.error("lone low surrogate in string"));
                        } else {
                            first
                        };
                        match char::from_u32(code) {
                            Some(ch) => out.push(ch),
                            None => return Err(self.error("invalid code point in string")),
                        }
                    }
                    _ => {
                        self.pos -= 1;
                        return Err(self.error("invalid escape sequence in string"));
                    }
                },
                Some(b) if b < 0x20 => {
                    self.pos -= 1;
                    return Err(self.error("unescaped control character in string"));
                }
                Some(_) => unreachable!("plain bytes are consumed by the fast path"),
                None => return Err(self.error("unterminated string")),
            }
        }
    }

    fn parse_hex4(&mut self) -> Result<u32, NnError> {
        let mut value: u32 = 0;
        for _ in 0..4 {
            let digit = self
                .bump()
                .and_then(|b| (b as char).to_digit(16))
                .ok_or_else(|| NnError::WireSyntax {
                    detail: format!(
                        "JSON parse error at byte {}: \\u escape requires four hex digits",
                        self.pos
                    ),
                })?;
            value = value * 16 + digit;
        }
        Ok(value)
    }
}

fn write_canonical(value: &Json, out: &mut String) -> Result<(), NnError> {
    match value {
        Json::Null => out.push_str("null"),
        Json::Bool(true) => out.push_str("true"),
        Json::Bool(false) => out.push_str("false"),
        Json::Str(s) => write_canonical_string(s, out),
        Json::Number(digits) => {
            return Err(NnError::WireSyntax {
                detail: format!(
                    "foreign number {digits} cannot be canonicalized; convert it to a decimal string first"
                ),
            });
        }
        Json::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_canonical(item, out)?;
            }
            out.push(']');
        }
        Json::Object(members) => {
            let mut indexed: Vec<&(String, Json)> = members.iter().collect();
            if members.len() > 1 {
                // Reject duplicate keys before canonicalization.
                indexed.sort_by(|a, b| cmp_utf16(&a.0, &b.0));
                for window in indexed.windows(2) {
                    if window[0].0 == window[1].0 {
                        return Err(NnError::WireSyntax {
                            detail: format!(
                                "duplicate object key {}",
                                super::error::brief(&window[0].0)
                            ),
                        });
                    }
                }
            }
            out.push('{');
            for (i, (key, item)) in indexed.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_canonical_string(key, out);
                out.push(':');
                write_canonical(item, out)?;
            }
            out.push('}');
        }
    }
    Ok(())
}

/// Compare two strings by their UTF-16 code unit sequences (JCS key order).
fn cmp_utf16(a: &str, b: &str) -> std::cmp::Ordering {
    a.encode_utf16().cmp(b.encode_utf16())
}

fn write_canonical_string(s: &str, out: &mut String) {
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{0008}' => out.push_str("\\b"),
            '\u{000C}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(input: &str) -> Result<Json, NnError> {
        Json::parse_strict(input, ParseLimits::default())
    }

    #[test]
    fn parses_the_full_subset() {
        let text = r#"{"schema":"binfiddle.nn.result/v1","ok":true,"none":null,"list":["a","b",{"count":"2"}]}"#;
        let value = parse(text).unwrap();
        assert_eq!(
            value.get("schema"),
            Some(&Json::Str("binfiddle.nn.result/v1".into()))
        );
        assert_eq!(value.get("ok"), Some(&Json::Bool(true)));
        assert_eq!(value.get("none"), Some(&Json::Null));
        match value.get("list") {
            Some(Json::Array(items)) => assert_eq!(items.len(), 3),
            other => panic!("unexpected list: {other:?}"),
        }
        assert_eq!(
            value.get("list").unwrap().at(2).unwrap().get("count"),
            Some(&Json::Str("2".into()))
        );
    }

    #[test]
    fn rejects_bare_numbers() {
        assert!(parse("1").is_err());
        assert!(parse("-1.5e3").is_err());
        assert!(parse(r#"{"a":0}"#).is_err());
    }

    #[test]
    fn rejects_duplicate_keys() {
        let err = parse(r#"{"a":"1","a":"2"}"#).unwrap_err();
        assert_eq!(err.code().as_str(), "WIRE_SYNTAX");
        assert!(err.to_string().contains("duplicate object key"));
    }

    #[test]
    fn rejects_trailing_content() {
        assert!(parse(r#"{"a":"1"} extra"#).is_err());
        assert!(parse(r#""a" "b""#).is_err());
    }

    #[test]
    fn rejects_malformed_documents() {
        assert!(parse("").is_err());
        assert!(parse("{").is_err());
        assert!(parse(r#"{"a"}"#).is_err());
        assert!(parse(r#"["a",]"#).is_err());
        assert!(parse(r#"tru"#).is_err());
        assert!(parse(r#""unterminated"#).is_err());
    }

    #[test]
    fn rejects_bad_escapes_and_control_chars() {
        assert!(parse(r#""\x41""#).is_err());
        assert!(parse(r#""\u00""#).is_err());
        assert!(parse("\"a\nb\"").is_err());
        assert!(parse(r#""\ud800""#).is_err());
        assert!(parse(r#""\udc00\ud800""#).is_err());
    }

    #[test]
    fn accepts_surrogate_pairs_and_escapes() {
        let value = parse(r#""😀ok""#).unwrap();
        assert_eq!(value, Json::Str("😀ok".to_string()));
        let value = parse(r#""A\/B""#).unwrap();
        assert_eq!(value, Json::Str("A/B".to_string()));
    }

    #[test]
    fn enforces_depth_limit() {
        let limits = ParseLimits {
            max_depth: 4,
            max_nodes: 1000,
        };
        assert!(Json::parse_strict("[[[[\"x\"]]]]", limits).is_ok());
        assert!(Json::parse_strict("[[[[[\"x\"]]]]]", limits).is_err());
    }

    #[test]
    fn enforces_node_limit() {
        let limits = ParseLimits {
            max_depth: 64,
            max_nodes: 3,
        };
        assert!(Json::parse_strict("[\"a\",\"b\"]", limits).is_ok());
        assert!(Json::parse_strict("[\"a\",\"b\",\"c\"]", limits).is_err());
    }

    #[test]
    fn canonical_sorts_keys_by_utf16() {
        let value = Json::object(vec![
            ("b", Json::Str("1".into())),
            ("a", Json::Str("2".into())),
        ])
        .unwrap();
        assert_eq!(value.to_canonical().unwrap(), r#"{"a":"2","b":"1"}"#);

        // U+10000 encodes as D800 DC00 in UTF-16, which sorts before U+FFFF.
        let astral_first =
            Json::object(vec![("\u{FFFF}", Json::Null), ("\u{10000}", Json::Null)]).unwrap();
        assert_eq!(
            astral_first.to_canonical().unwrap(),
            "{\"\u{10000}\":null,\"\u{FFFF}\":null}"
        );
    }

    #[test]
    fn canonical_escapes_control_characters() {
        let value = Json::Str("a\u{0001}b\"c\\d\ne\u{001F}".to_string());
        assert_eq!(
            value.to_canonical().unwrap(),
            "\"a\\u0001b\\\"c\\\\d\\ne\\u001f\""
        );
    }

    #[test]
    fn canonical_round_trips_through_parser() {
        let value = Json::object(vec![
            ("schema", Json::Str("binfiddle.nn.result/v1".into())),
            (
                "nested",
                Json::Array(vec![Json::Bool(true), Json::Null, Json::Str("é😀".into())]),
            ),
            ("z-last", Json::Object(vec![])),
        ])
        .unwrap();
        let canonical = value.to_canonical().unwrap();
        let reparsed = parse(&canonical).unwrap();
        assert_eq!(reparsed.to_canonical().unwrap(), canonical);
    }

    #[test]
    fn canonical_rejects_duplicate_keys() {
        let dup = Json::Object(vec![
            ("a".to_string(), Json::Null),
            ("a".to_string(), Json::Null),
        ]);
        assert!(dup.to_canonical().is_err());
    }

    #[test]
    fn builder_rejects_duplicate_keys() {
        assert!(Json::object(vec![("a", Json::Null), ("a", Json::Null)]).is_err());
    }

    #[test]
    fn canonical_digest_is_stable() {
        let value = Json::object(vec![("a", Json::Str("1".into()))]).unwrap();
        let d1 = value.canonical_digest().unwrap();
        let d2 = value.canonical_digest().unwrap();
        assert_eq!(d1, d2);
        assert_eq!(d1.len(), 64);
        assert!(d1
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
    }

    #[test]
    fn foreign_mode_accepts_non_negative_integers_only() {
        let value = Json::parse_foreign(
            r#"{"shape":[2,3],"off":[0,48],"name":"t"}"#,
            ParseLimits::default(),
        )
        .unwrap();
        let shape = value.get("shape").unwrap();
        assert_eq!(shape.at(0).unwrap().as_number_u64(), Some(2));
        assert_eq!(shape.at(1).unwrap().as_number_u64(), Some(3));
        assert_eq!(shape.at(0), Some(&Json::Number("2".to_string())));

        assert!(Json::parse_foreign("-1", ParseLimits::default()).is_err());
        assert!(Json::parse_foreign("1.5", ParseLimits::default()).is_err());
        assert!(Json::parse_foreign("1e3", ParseLimits::default()).is_err());
        assert!(Json::parse_foreign("01", ParseLimits::default()).is_err());
        // Strict mode still rejects integers.
        assert!(Json::parse_strict("2", ParseLimits::default()).is_err());
        assert!(Json::parse_strict(r#"{"a":2}"#, ParseLimits::default()).is_err());
    }

    #[test]
    fn foreign_numbers_cannot_be_canonicalized() {
        let value = Json::parse_foreign("12", ParseLimits::default()).unwrap();
        assert!(value.to_canonical().is_err());
        // Converted to a decimal string, it serializes fine.
        let converted = Json::Str(value.as_number_u64().unwrap().to_string());
        assert_eq!(converted.to_canonical().unwrap(), "\"12\"");
    }

    #[test]
    fn as_number_u64_rejects_other_variants() {
        assert_eq!(Json::Str("12".into()).as_number_u64(), None);
        assert_eq!(Json::Null.as_number_u64(), None);
    }

    #[test]
    fn canonical_digest_matches_reference_vector() {
        // SHA-256 of the exact canonical bytes {"a":"1","b":"2"},
        // computed independently with sha256sum.
        let value = Json::object(vec![
            ("b", Json::Str("2".into())),
            ("a", Json::Str("1".into())),
        ])
        .unwrap();
        assert_eq!(
            value.canonical_digest().unwrap(),
            "21f76dfbfe6dfe21f762080ef484112cf2952974cef30741fd1931e1c6d92112"
        );
    }
}
