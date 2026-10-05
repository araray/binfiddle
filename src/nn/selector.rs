//! Component selector grammar.
//!
//! The baseline grammar is deliberately small:
//!
//! ```text
//! selector     := segment ("." segment)*
//! segment      := identifier indexer?
//! indexer      := "[" index_spec "]"
//! index_spec   := "*" | uint | uint ":" uint | uint ("," uint)+
//! identifier   := [A-Za-z_][A-Za-z0-9_]*
//! uint         := "0" | [1-9][0-9]*
//! ```
//!
//! Indices are zero-based and ranges are half-open. Negative indices, omitted
//! range endpoints, slice steps, and arbitrary expressions are not part of the
//! grammar; names outside the restricted alphabet are addressed through exact
//! original-name or full-identifier routes instead of being rewritten.

use super::error::NnError;
use std::fmt;

/// One index specification inside brackets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IndexSpec {
    /// `*` — all elements of the family.
    All,
    /// A single zero-based index.
    Single(u64),
    /// A half-open range `[start, end)`.
    Range(u64, u64),
    /// An explicit list of indices, in the given order.
    List(Vec<u64>),
}

/// One selector segment: a name with an optional indexer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    pub name: String,
    pub index: Option<IndexSpec>,
}

/// A parsed selector expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selector {
    pub segments: Vec<Segment>,
}

impl fmt::Display for Selector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut text = String::new();
        for (position, segment) in self.segments.iter().enumerate() {
            if position > 0 {
                text.push('.');
            }
            text.push_str(&segment.name);
            match &segment.index {
                Some(IndexSpec::All) => text.push_str("[*]"),
                Some(IndexSpec::Single(i)) => text.push_str(&format!("[{i}]")),
                Some(IndexSpec::Range(a, b)) => text.push_str(&format!("[{a}:{b}]")),
                Some(IndexSpec::List(items)) => {
                    text.push('[');
                    for (i, item) in items.iter().enumerate() {
                        if i > 0 {
                            text.push(',');
                        }
                        text.push_str(&item.to_string());
                    }
                    text.push(']');
                }
                None => {}
            }
        }
        f.write_str(&text)
    }
}

impl Selector {
    /// Parse a selector expression. Errors carry `WIRE_SYNTAX` with the
    /// offending position and reason; nothing is silently coerced.
    pub fn parse(input: &str) -> Result<Selector, NnError> {
        let bytes = input.as_bytes();
        let mut pos: usize = 0;
        let mut segments: Vec<Segment> = Vec::new();

        if input.is_empty() {
            return Err(selector_error(input, 0, "selector cannot be empty"));
        }

        loop {
            // identifier
            let name_start = pos;
            while pos < bytes.len() && is_identifier_byte(bytes[pos], pos == name_start) {
                pos += 1;
            }
            let name_len = pos - name_start;
            if name_len == 0 {
                return Err(selector_error(
                    input,
                    pos,
                    "expected an identifier [A-Za-z_][A-Za-z0-9_]*",
                ));
            }
            let name = input[name_start..pos].to_string();
            if is_reserved(&name) {
                return Err(selector_error(
                    input,
                    name_start,
                    &format!("'{name}' is a reserved word and cannot be a segment name"),
                ));
            }

            // optional indexer
            let index = if pos < bytes.len() && bytes[pos] == b'[' {
                pos += 1;
                let spec = parse_index_spec(input, &mut pos)?;
                if pos >= bytes.len() || bytes[pos] != b']' {
                    return Err(selector_error(input, pos.min(input.len()), "expected ']'"));
                }
                pos += 1;
                Some(spec)
            } else {
                None
            };

            segments.push(Segment { name, index });

            if pos == bytes.len() {
                break;
            }
            if bytes[pos] == b'.' {
                pos += 1;
                if pos == bytes.len() {
                    return Err(selector_error(input, pos, "selector cannot end with '.'"));
                }
                continue;
            }
            return Err(selector_error(
                input,
                pos,
                &format!(
                    "unexpected character {:?}",
                    input[pos..].chars().next().unwrap_or('?')
                ),
            ));
        }

        Ok(Selector { segments })
    }
}

fn is_identifier_byte(b: u8, first: bool) -> bool {
    match b {
        b'a'..=b'z' | b'A'..=b'Z' | b'_' => true,
        b'0'..=b'9' => !first,
        _ => false,
    }
}

/// Reserved words are rejected as segment names so future grammar versions can
/// attach meaning to them without reinterpreting existing saved selections.
fn is_reserved(name: &str) -> bool {
    matches!(name, "true" | "false" | "null" | "self" | "super")
}

fn parse_index_spec(input: &str, pos: &mut usize) -> Result<IndexSpec, NnError> {
    let bytes = input.as_bytes();
    if *pos < bytes.len() && bytes[*pos] == b'*' {
        *pos += 1;
        return Ok(IndexSpec::All);
    }

    let first = parse_uint(input, pos)?;
    if *pos < bytes.len() && bytes[*pos] == b':' {
        *pos += 1;
        let second = parse_uint(input, pos)?;
        if second <= first {
            return Err(selector_error(
                input,
                *pos,
                &format!("range [{first}:{second}) must have end after start"),
            ));
        }
        return Ok(IndexSpec::Range(first, second));
    }
    if *pos < bytes.len() && bytes[*pos] == b',' {
        let mut items = vec![first];
        while *pos < bytes.len() && bytes[*pos] == b',' {
            *pos += 1;
            items.push(parse_uint(input, pos)?);
        }
        for pair in items.windows(2) {
            if pair[0] == pair[1] {
                return Err(selector_error(
                    input,
                    *pos,
                    &format!("duplicate index {} in list", pair[0]),
                ));
            }
        }
        return Ok(IndexSpec::List(items));
    }
    Ok(IndexSpec::Single(first))
}

fn parse_uint(input: &str, pos: &mut usize) -> Result<u64, NnError> {
    let bytes = input.as_bytes();
    let start = *pos;
    if *pos >= bytes.len() || !bytes[*pos].is_ascii_digit() {
        return Err(selector_error(
            input,
            *pos,
            "expected a non-negative integer",
        ));
    }
    if bytes[*pos] == b'0' && *pos + 1 < bytes.len() && bytes[*pos + 1].is_ascii_digit() {
        return Err(selector_error(
            input,
            *pos,
            "indices cannot have leading zeros",
        ));
    }
    while *pos < bytes.len() && bytes[*pos].is_ascii_digit() {
        *pos += 1;
    }
    input[start..*pos]
        .parse::<u64>()
        .map_err(|_| selector_error(input, start, "index out of u64 range"))
}

fn selector_error(input: &str, pos: usize, reason: &str) -> NnError {
    NnError::WireSyntax {
        detail: format!(
            "selector parse error at byte {}: {} (in {})",
            pos,
            reason,
            super::error::brief(input)
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(input: &str) -> Selector {
        Selector::parse(input).unwrap_or_else(|e| panic!("{input}: {e}"))
    }

    fn err(input: &str) -> NnError {
        Selector::parse(input).expect_err(input)
    }

    #[test]
    fn parses_plain_paths() {
        assert_eq!(
            ok("model"),
            Selector {
                segments: vec![Segment {
                    name: "model".into(),
                    index: None
                }]
            }
        );
        let s = ok("decoder.layers[12].mlp");
        assert_eq!(s.segments.len(), 3);
        assert_eq!(s.segments[1].index, Some(IndexSpec::Single(12)));
    }

    #[test]
    fn parses_all_indexer_forms() {
        assert_eq!(ok("layers[*]").segments[0].index, Some(IndexSpec::All));
        assert_eq!(
            ok("layers[8:16]").segments[0].index,
            Some(IndexSpec::Range(8, 16))
        );
        assert_eq!(
            ok("layers[4,9,17]").segments[0].index,
            Some(IndexSpec::List(vec![4, 9, 17]))
        );
        assert_eq!(ok("x[0]").segments[0].index, Some(IndexSpec::Single(0)));
    }

    #[test]
    fn rejects_negative_and_fancy_indices() {
        for bad in [
            "layers[-1]",
            "layers[1:]",
            "layers[:2]",
            "layers[::2]",
            "layers[1:2:3]",
            "layers[]",
            "layers[01]",
            "layers[1,]",
            "layers[*2]",
            "layers[1 ,2]",
        ] {
            let e = err(bad);
            assert_eq!(e.code().as_str(), "WIRE_SYNTAX", "input {bad}");
        }
    }

    #[test]
    fn rejects_bad_identifiers_and_structure() {
        for bad in [
            "",
            ".a",
            "a.",
            "a..b",
            "9layers",
            "lay ers",
            "layers[0",
            "layers0]",
            "layers]",
            "a[b]",
            "café",
            "layers[18446744073709551616]",
        ] {
            let e = err(bad);
            assert_eq!(e.code().as_str(), "WIRE_SYNTAX", "input {bad:?}");
        }
    }

    #[test]
    fn rejects_reserved_words() {
        assert!(Selector::parse("true").is_err());
        assert!(Selector::parse("a.self").is_err());
        // Reserved words remain legal inside longer names.
        assert!(Selector::parse("self_attention").is_ok());
    }

    #[test]
    fn rejects_empty_and_backwards_ranges_and_duplicate_list_entries() {
        assert!(Selector::parse("layers[3:3]").is_err());
        assert!(Selector::parse("layers[5:2]").is_err());
        assert!(Selector::parse("layers[2,2]").is_err());
    }

    #[test]
    fn display_round_trips() {
        for text in [
            "model",
            "decoder.layers[12]",
            "decoder.layers[8:16].mlp",
            "decoder.layers[4,9,17]",
            "layers[*].attention",
        ] {
            assert_eq!(ok(text).to_string(), text);
        }
    }
}
