//! Parsing utilities for binfiddle.
//!
//! This module provides functions for parsing:
//! - Range specifications (e.g., "10..20", "0x100..", "..0xFF")
//! - Input data in various formats (hex, dec, oct, bin, ascii)
//! - Search patterns for the search command
/// src/utils/parsing.rs
use crate::error::{BinfiddleError, Result};

/// Parses a range specification string into start and optional end indices.
///
/// # Range Syntax
/// - `"10"` → Single byte at index 10 (returns `(10, Some(11))`)
/// - `"10..20"` → Bytes 10-19 (returns `(10, Some(20))`)
/// - `"..20"` → Bytes 0-19 (returns `(0, Some(20))`)
/// - `"10.."` → Bytes 10 to end (returns `(10, None)`)
/// - `".."` → Entire data (returns `(0, None)`)
/// - `"0x100..0x200"` → Hex indices (returns `(256, Some(512))`)
///
/// # Arguments
/// * `range` - The range specification string
/// * `data_len` - The length of the data (for validation)
///
/// # Returns
/// A tuple of (start, Option<end>) where end is exclusive.
///
/// # Errors
/// Returns `BinfiddleError::Parse` if the range string is malformed.
/// Returns `BinfiddleError::InvalidRange` if indices are out of bounds.
pub fn parse_range(range: &str, data_len: usize) -> Result<(usize, Option<usize>)> {
    let range = range.trim();

    // Check if it's a slice (contains "..")
    if range.contains("..") {
        let parts: Vec<&str> = range.split("..").collect();
        if parts.len() != 2 {
            return Err(BinfiddleError::Parse(format!(
                "Invalid range format: '{}'. Expected 'start..end', '..end', 'start..', or '..'",
                range
            )));
        }

        let start = if parts[0].is_empty() {
            0
        } else {
            parse_number(parts[0])?
        };

        let end = if parts[1].is_empty() {
            None
        } else {
            Some(parse_number(parts[1])?)
        };

        // Validate bounds
        if start > data_len {
            return Err(BinfiddleError::InvalidRange(format!(
                "Start index {} exceeds data length {}",
                start, data_len
            )));
        }

        if let Some(e) = end {
            if e > data_len {
                return Err(BinfiddleError::InvalidRange(format!(
                    "End index {} exceeds data length {}",
                    e, data_len
                )));
            }
            if start >= e {
                return Err(BinfiddleError::InvalidRange(format!(
                    "Start index {} must be less than end index {}",
                    start, e
                )));
            }
        }

        Ok((start, end))
    } else {
        // Single index - treat as single byte
        let index = parse_number(range)?;
        if index >= data_len {
            return Err(BinfiddleError::InvalidRange(format!(
                "Index {} out of bounds (data length: {})",
                index, data_len
            )));
        }
        Ok((index, Some(index + 1)))
    }
}

/// Parses a number from a string, supporting decimal and hexadecimal formats.
///
/// # Supported Formats
/// - Decimal: `"123"`, `"0"`, `"255"`
/// - Hexadecimal: `"0x1F"`, `"0X1f"`, `"01F"` (leading zero implies hex)
///
/// # Arguments
/// * `s` - The string to parse
///
/// # Returns
/// The parsed number as `usize`.
///
/// # Errors
/// Returns `BinfiddleError::Parse` if the string cannot be parsed.
fn parse_number(s: &str) -> Result<usize> {
    let s = s.trim();
    if s.is_empty() {
        return Err(BinfiddleError::Parse("Empty number string".to_string()));
    }

    // Check for hex prefix
    if s.starts_with("0x") || s.starts_with("0X") {
        usize::from_str_radix(&s[2..], 16).map_err(|e| {
            BinfiddleError::Parse(format!("Invalid hexadecimal number '{}': {}", s, e))
        })
    } else if s.starts_with('0') && s.len() > 1 && s.chars().skip(1).all(|c| c.is_ascii_hexdigit())
    {
        // Leading zero with hex digits implies hex (e.g., "0100" = 256)
        usize::from_str_radix(&s[1..], 16).map_err(|e| {
            BinfiddleError::Parse(format!("Invalid hexadecimal number '{}': {}", s, e))
        })
    } else {
        s.parse::<usize>()
            .map_err(|e| BinfiddleError::Parse(format!("Invalid decimal number '{}': {}", s, e)))
    }
}

/// Parses input data from various formats into raw bytes.
///
/// # Supported Formats
/// - `hex`: Hexadecimal pairs, with or without spaces (e.g., "DEADBEEF" or "DE AD BE EF")
/// - `dec`: Space-separated decimal values 0-255 (e.g., "222 173 190 239")
/// - `oct`: Space-separated octal values 0-377 (e.g., "336 255 276 357")
/// - `bin`: Space-separated binary strings, ≤8 chars each (e.g., "11011110 10101101")
/// - `ascii`: Raw ASCII string converted to bytes
///
/// # Arguments
/// * `input` - The input string
/// * `format` - The format identifier
///
/// # Returns
/// A vector of bytes.
///
/// # Errors
/// Returns `BinfiddleError::Parse` if the input is malformed for the given format.
/// Returns `BinfiddleError::InvalidInput` if the format is unknown.
pub fn parse_input(input: &str, format: &str) -> Result<Vec<u8>> {
    match format.to_lowercase().as_str() {
        "hex" => parse_hex_input(input),
        "dec" => parse_dec_input(input),
        "oct" => parse_oct_input(input),
        "bin" => parse_bin_input(input),
        "ascii" => Ok(input.as_bytes().to_vec()),
        _ => Err(BinfiddleError::InvalidInput(format!(
            "Unknown input format: '{}'. Supported: hex, dec, oct, bin, ascii",
            format
        ))),
    }
}

/// Parses hexadecimal input into bytes.
///
/// Accepts hex strings with or without spaces/separators.
/// Examples: "DEADBEEF", "DE AD BE EF", "de-ad-be-ef"
fn parse_hex_input(input: &str) -> Result<Vec<u8>> {
    // Remove common separators and whitespace
    let cleaned: String = input.chars().filter(|c| c.is_ascii_hexdigit()).collect();

    if cleaned.len() % 2 != 0 {
        return Err(BinfiddleError::Parse(format!(
            "Hex input must have even number of digits, got {} digits",
            cleaned.len()
        )));
    }

    hex::decode(&cleaned).map_err(|e| BinfiddleError::Parse(format!("Invalid hex input: {}", e)))
}

/// Parses decimal input into bytes.
///
/// Expects space-separated decimal values, each 0-255.
fn parse_dec_input(input: &str) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    for part in input.split_whitespace() {
        let value: u16 = part.parse().map_err(|e| {
            BinfiddleError::Parse(format!("Invalid decimal value '{}': {}", part, e))
        })?;
        if value > 255 {
            return Err(BinfiddleError::Parse(format!(
                "Decimal value {} exceeds byte range (0-255)",
                value
            )));
        }
        bytes.push(value as u8);
    }
    if bytes.is_empty() {
        return Err(BinfiddleError::Parse("Empty decimal input".to_string()));
    }
    Ok(bytes)
}

/// Parses octal input into bytes.
///
/// Expects space-separated octal values, each 0-377.
fn parse_oct_input(input: &str) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    for part in input.split_whitespace() {
        let value = u16::from_str_radix(part, 8)
            .map_err(|e| BinfiddleError::Parse(format!("Invalid octal value '{}': {}", part, e)))?;
        if value > 255 {
            return Err(BinfiddleError::Parse(format!(
                "Octal value {} (decimal {}) exceeds byte range (0-377 octal)",
                part, value
            )));
        }
        bytes.push(value as u8);
    }
    if bytes.is_empty() {
        return Err(BinfiddleError::Parse("Empty octal input".to_string()));
    }
    Ok(bytes)
}

/// Parses binary input into bytes.
///
/// Expects space-separated binary strings, each ≤8 characters.
fn parse_bin_input(input: &str) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    for part in input.split_whitespace() {
        if part.len() > 8 {
            return Err(BinfiddleError::Parse(format!(
                "Binary value '{}' exceeds 8 bits",
                part
            )));
        }
        let value = u8::from_str_radix(part, 2).map_err(|e| {
            BinfiddleError::Parse(format!("Invalid binary value '{}': {}", part, e))
        })?;
        bytes.push(value);
    }
    if bytes.is_empty() {
        return Err(BinfiddleError::Parse("Empty binary input".to_string()));
    }
    Ok(bytes)
}

/// Parses bit-level input (placeholder for future bit-level operations).
///
/// Currently delegates to `parse_input` for byte-aligned formats.
pub fn parse_bit_input(input: &str, format: &str) -> Result<Vec<u8>> {
    // For now, delegate to standard parsing
    // Future: support bit-level specifications like "0x10:3" (byte 0x10, bit 3)
    parse_input(input, format)
}

/// Parses a search pattern based on the input format.
///
/// This function handles pattern parsing specifically for the search command,
/// supporting additional formats like regex and mask patterns.
///
/// # Supported Formats
/// - `hex`: Hexadecimal byte pattern (e.g., "DE AD BE EF")
/// - `ascii`: Literal string pattern
/// - `dec`: Decimal byte sequence
/// - `oct`: Octal byte sequence
/// - `bin`: Binary byte sequence
/// - `regex`: Regular expression pattern for byte matching
/// - `hex-regex`: Hex-based regex pattern (e.g., "[0-9A-F]{2}FF")
/// - `mask`: Hex pattern with wildcards (e.g., "DE ?? BE EF")
///
/// # Returns
/// For non-regex/mask formats: `SearchPattern::Exact(Vec<u8>)`
/// For regex: `SearchPattern::Regex(String)` - the regex pattern string
/// For mask: `SearchPattern::Mask(...)` - parsed mask pattern
pub fn parse_search_pattern(input: &str, format: &str) -> Result<SearchPattern> {
    match format.to_lowercase().as_str() {
        "hex" => Ok(SearchPattern::Exact(parse_hex_input(input)?)),
        "ascii" => Ok(SearchPattern::Exact(input.as_bytes().to_vec())),
        "dec" => Ok(SearchPattern::Exact(parse_dec_input(input)?)),
        "oct" => Ok(SearchPattern::Exact(parse_oct_input(input)?)),
        "bin" => Ok(SearchPattern::Exact(parse_bin_input(input)?)),
        "regex" => Ok(SearchPattern::Regex(input.to_string())),
        "hex-regex" | "hexregex" => parse_hex_regex_pattern(input),
        "mask" => parse_mask_pattern(input),
        _ => Err(BinfiddleError::InvalidInput(format!(
            "Unknown search pattern format: '{}'. Supported: hex, ascii, dec, oct, bin, regex, hex-regex, mask",
            format
        ))),
    }
}

/// Represents a parsed search pattern.
#[derive(Debug, Clone)]
pub enum SearchPattern {
    /// Exact byte sequence to match
    Exact(Vec<u8>),
    /// Regular expression pattern (string form, compiled later)
    Regex(String),
    /// Mask pattern with wildcards
    /// Each element is either Some(byte) for exact match or None for wildcard
    Mask(Vec<Option<u8>>),
}

/// Parses a mask pattern with wildcards (e.g., "DE ?? BE EF").
///
/// Wildcards can be represented as "??" or "XX" (case-insensitive).
fn parse_mask_pattern(input: &str) -> Result<SearchPattern> {
    let mut pattern = Vec::new();

    // Split by whitespace or process character pairs
    let cleaned: String = input
        .chars()
        .filter(|c| c.is_ascii_hexdigit() || *c == '?' || *c == 'x' || *c == 'X')
        .collect();

    if cleaned.len() % 2 != 0 {
        return Err(BinfiddleError::Parse(format!(
            "Mask pattern must have pairs of characters, got {} characters",
            cleaned.len()
        )));
    }

    let mut chars = cleaned.chars().peekable();
    while let (Some(c1), Some(c2)) = (chars.next(), chars.next()) {
        let pair = format!("{}{}", c1, c2).to_uppercase();
        if pair == "??" || pair == "XX" {
            pattern.push(None); // Wildcard
        } else {
            let byte = u8::from_str_radix(&pair, 16).map_err(|e| {
                BinfiddleError::Parse(format!("Invalid mask byte '{}': {}", pair, e))
            })?;
            pattern.push(Some(byte));
        }
    }

    if pattern.is_empty() {
        return Err(BinfiddleError::Parse("Empty mask pattern".to_string()));
    }

    Ok(SearchPattern::Mask(pattern))
}

/// Parses a hex-regex pattern and converts it to a byte-level regex.
///
/// Hex-regex allows users to write regex patterns using hex notation.
///
/// # Examples
/// - `"FF"` → `"\xFF"` (literal byte 0xFF)
/// - `"[0-9A-F]{2}"` → `"[\x00-\xFF]"` (any byte)
/// - `"FF.{2}EF"` → `"\xFF..\xEF"` (0xFF, any 2 bytes, 0xEF)
/// - `"(DE|AD)"` → `"(\xDE|\xAD)"` (0xDE or 0xAD)
///
/// # Syntax
/// - Hex pairs (00-FF): Literal byte values
/// - `.`: Any single byte (kept as-is)
/// - `*`, `+`, `?`, `{n,m}`: Quantifiers (kept as-is)
/// - `[...]`: Character class - hex digits converted to byte ranges
/// - `(...)`: Grouping (kept as-is)
/// - `|`: Alternation (kept as-is)
/// - `^`, `$`: Anchors (kept as-is)
/// - `\xHH`: Already escaped hex (kept as-is)
///
/// # Returns
/// A `SearchPattern::Regex` with the converted byte-level pattern
fn parse_hex_regex_pattern(input: &str) -> Result<SearchPattern> {
    let mut result = String::new();
    let mut chars = input.chars().peekable();

    while let Some(ch) = chars.next() {
        match ch {
            // Regex operators - keep as-is
            '.' | '*' | '+' | '?' | '^' | '$' | '|' | '(' | ')' => {
                result.push(ch);
            }

            // Quantifiers - keep as-is
            '{' => {
                result.push(ch);
                // Copy everything until closing }
                while let Some(&next_ch) = chars.peek() {
                    result.push(chars.next().unwrap());
                    if next_ch == '}' {
                        break;
                    }
                }
            }

            // Character classes - need special handling for hex
            '[' => {
                result.push('[');
                let class_content = parse_hex_char_class(&mut chars)?;
                result.push_str(&class_content);
                result.push(']');
                // Consume the closing ']' from the iterator
                // (parse_hex_char_class stops AT the ']' but doesn't consume it)
                chars.next();
            }

            // Backslash - might be \xHH or other escape
            '\\' => {
                if let Some(&next_ch) = chars.peek() {
                    if next_ch == 'x' {
                        // Already escaped hex - keep as-is
                        result.push('\\');
                        result.push(chars.next().unwrap()); // 'x'
                                                            // Copy next two hex digits
                        for _ in 0..2 {
                            if let Some(hex_digit) = chars.next() {
                                result.push(hex_digit);
                            }
                        }
                    } else {
                        // Other escape sequence - keep as-is
                        result.push('\\');
                        result.push(chars.next().unwrap());
                    }
                } else {
                    result.push('\\');
                }
            }

            // Whitespace - skip (hex patterns can have spaces)
            ' ' | '\t' | '\n' | '\r' => {
                // Skip whitespace
            }

            // Potential hex digit - check if it's a hex pair
            _ if ch.is_ascii_hexdigit() => {
                // Look ahead for second hex digit
                if let Some(&next_ch) = chars.peek() {
                    if next_ch.is_ascii_hexdigit() {
                        // We have a hex pair - convert to \xHH
                        let hex_pair = format!("{}{}", ch, chars.next().unwrap());
                        let byte_value = u8::from_str_radix(&hex_pair, 16).map_err(|e| {
                            BinfiddleError::Parse(format!("Invalid hex pair '{}': {}", hex_pair, e))
                        })?;
                        result.push_str(&format!("\\x{:02X}", byte_value));
                    } else {
                        // Single hex digit followed by non-hex - error
                        return Err(BinfiddleError::Parse(format!(
                            "Incomplete hex pair: '{}' followed by '{}'",
                            ch, next_ch
                        )));
                    }
                } else {
                    // Single hex digit at end - error
                    return Err(BinfiddleError::Parse(format!(
                        "Incomplete hex pair at end: '{}'",
                        ch
                    )));
                }
            }

            // Unexpected character
            _ => {
                return Err(BinfiddleError::Parse(format!(
                    "Unexpected character '{}' in hex-regex pattern",
                    ch
                )));
            }
        }
    }

    Ok(SearchPattern::Regex(result))
}

/// Parses the content of a character class in hex-regex notation.
///
/// Converts hex digit ranges to byte ranges.
/// Examples:
/// - `"0-9A-F"` → `"\x00-\x09\x0A-\x0F\x41-\x46\x61-\x66"` (matches '0'-'9', 'A'-'F', 'a'-'f')
/// - `"00-FF"` → `"\x00-\xFF"` (any byte)
/// - `"^00"` → `"^\x00"` (not NULL)
fn parse_hex_char_class(chars: &mut std::iter::Peekable<std::str::Chars>) -> Result<String> {
    let mut result = String::new();

    while let Some(&ch) = chars.peek() {
        if ch == ']' {
            // End of character class
            break;
        }

        let current_char = chars.next().unwrap();

        match current_char {
            // Negation at start
            '^' if result.is_empty() => {
                result.push('^');
            }

            // Range operator - just pass it through
            '-' => {
                result.push('-');
            }

            // Hex digit - might be part of hex pair or literal
            _ if current_char.is_ascii_hexdigit() => {
                // Check if next char is also hex digit (making a hex pair)
                if let Some(&next_ch) = chars.peek() {
                    if next_ch != ']' && next_ch != '-' && next_ch.is_ascii_hexdigit() {
                        // Hex pair - convert to \xHH
                        let hex_pair = format!("{}{}", current_char, chars.next().unwrap());
                        let byte_value = u8::from_str_radix(&hex_pair, 16).map_err(|e| {
                            BinfiddleError::Parse(format!(
                                "Invalid hex pair in char class '{}': {}",
                                hex_pair, e
                            ))
                        })?;
                        result.push_str(&format!("\\x{:02X}", byte_value));
                    } else {
                        // Single hex digit - treat as literal ASCII character
                        result.push(current_char);
                    }
                } else {
                    // Single hex digit at end - treat as literal
                    result.push(current_char);
                }
            }

            // Other characters - keep as-is (for ASCII ranges like A-Z)
            _ => {
                result.push(current_char);
            }
        }
    }

    Ok(result)
}

/// Validates a search pattern and returns helpful warnings if the pattern
/// might be using the wrong input format.
///
/// This helps users catch common mistakes like:
/// - Using `regex` for hex patterns (e.g., "ff" to find 0xFF)
/// - Using `hex` for ASCII text (e.g., searching for "ERROR" in hex mode)
/// - Patterns that look malformed for the chosen format
///
/// # Returns
/// A vector of warning messages. Empty if no issues detected.
pub fn validate_search_pattern(pattern: &str, format: &str) -> Vec<String> {
    let mut warnings = Vec::new();

    match format.to_lowercase().as_str() {
        "regex" => {
            // Check if pattern looks like it should be hex
            if is_likely_hex_pattern(pattern) {
                warnings.push(format!(
                    "⚠️  Pattern '{}' looks like hex bytes, but you're using --input-format regex.\n\
                    In regex mode, 'ff' matches ASCII 'f' (byte 0x66), NOT hex 0xFF.\n\
                    Did you mean: --input-format hex  or  --input-format hex-regex?",
                    pattern
                ));
            }
        }

        "hex" => {
            // Check if pattern looks like it should be ASCII
            if is_likely_ascii_pattern(pattern) {
                warnings.push(format!(
                    "⚠️  Pattern '{}' contains ASCII letters. In hex mode, each pair is a byte.\n\
                    Did you mean: --input-format ascii?",
                    pattern
                ));
            }

            // Check for common regex syntax in hex mode
            if contains_regex_operators(pattern) {
                warnings.push(format!(
                    "⚠️  Pattern '{}' contains regex operators ([], (), *, +).\n\
                    These are treated as literal characters in hex mode.\n\
                    Did you mean: --input-format regex  or  --input-format hex-regex?",
                    pattern
                ));
            }
        }

        "hex-regex" | "hexregex" => {
            // Check if pattern looks like plain hex (no regex operators)
            if is_plain_hex_only(pattern) {
                warnings.push(format!(
                    "💡 Pattern '{}' has no regex operators. Use --input-format hex for better performance.",
                    pattern
                ));
            }
        }

        "ascii" => {
            // Check if pattern contains non-printable characters or looks like hex
            if pattern.len() <= 4 && pattern.chars().all(|c| c.is_ascii_hexdigit()) {
                warnings.push(format!(
                    "⚠️  Pattern '{}' looks like hex bytes but using ASCII mode.\n\
                    This searches for literal string '{}', not hex bytes.\n\
                    Did you mean: --input-format hex?",
                    pattern, pattern
                ));
            }
        }

        "mask" => {
            // Check if wildcards are present
            if !pattern.contains("??")
                && !pattern.contains("XX")
                && !pattern.to_uppercase().contains("XX")
            {
                warnings.push(format!(
                    "💡 Pattern '{}' has no wildcards (?? or XX).\n\
                    Did you mean: --input-format hex?",
                    pattern
                ));
            }
        }

        _ => {}
    }

    warnings
}

/// Checks if a pattern likely represents hex bytes.
fn is_likely_hex_pattern(pattern: &str) -> bool {
    let cleaned: String = pattern.chars().filter(|c| !c.is_whitespace()).collect();

    // Short, all-hex patterns
    if cleaned.len() <= 8 && cleaned.chars().all(|c| c.is_ascii_hexdigit()) {
        return true;
    }

    // Contains common hex byte sequences
    let hex_markers = ["ff", "00", "dead", "beef", "cafe", "babe", "face", "fade"];
    let lower = cleaned.to_lowercase();
    hex_markers.iter().any(|marker| lower.contains(marker))
}

/// Checks if a pattern likely represents ASCII text.
fn is_likely_ascii_pattern(pattern: &str) -> bool {
    // Check for common English words
    let common_words = [
        "ERROR", "error", "Warning", "PASSWORD", "password", "USER", "user", "PATH", "path",
    ];
    if common_words.iter().any(|word| pattern.contains(word)) {
        return true;
    }

    // Long pattern with mixed case
    if pattern.len() > 8 {
        let has_upper = pattern.chars().any(|c| c.is_uppercase());
        let has_lower = pattern.chars().any(|c| c.is_lowercase());
        if has_upper && has_lower {
            return true;
        }
    }

    false
}

/// Checks if pattern contains regex operators.
fn contains_regex_operators(pattern: &str) -> bool {
    pattern
        .chars()
        .any(|c| matches!(c, '[' | ']' | '(' | ')' | '*' | '+' | '?' | '{' | '}' | '|'))
}

/// Checks if a pattern is plain hex with no regex operators.
fn is_plain_hex_only(pattern: &str) -> bool {
    let cleaned: String = pattern.chars().filter(|c| !c.is_whitespace()).collect();

    // Has regex operators?
    if contains_regex_operators(&cleaned)
        || cleaned.contains('.')
        || cleaned.contains('^')
        || cleaned.contains('$')
    {
        return false;
    }

    // All remaining chars are hex digits?
    cleaned.chars().all(|c| c.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_range_single_index() {
        let (start, end) = parse_range("10", 100).unwrap();
        assert_eq!(start, 10);
        assert_eq!(end, Some(11));
    }

    #[test]
    fn test_parse_range_full_range() {
        let (start, end) = parse_range("10..20", 100).unwrap();
        assert_eq!(start, 10);
        assert_eq!(end, Some(20));
    }

    #[test]
    fn test_parse_range_open_end() {
        let (start, end) = parse_range("10..", 100).unwrap();
        assert_eq!(start, 10);
        assert_eq!(end, None);
    }

    #[test]
    fn test_parse_range_open_start() {
        let (start, end) = parse_range("..20", 100).unwrap();
        assert_eq!(start, 0);
        assert_eq!(end, Some(20));
    }

    #[test]
    fn test_parse_range_entire() {
        let (start, end) = parse_range("..", 100).unwrap();
        assert_eq!(start, 0);
        assert_eq!(end, None);
    }

    #[test]
    fn test_parse_range_hex() {
        let (start, end) = parse_range("0x10..0x20", 100).unwrap();
        assert_eq!(start, 16);
        assert_eq!(end, Some(32));
    }

    #[test]
    fn test_parse_hex_input() {
        let bytes = parse_hex_input("DEADBEEF").unwrap();
        assert_eq!(bytes, vec![0xDE, 0xAD, 0xBE, 0xEF]);

        let bytes = parse_hex_input("DE AD BE EF").unwrap();
        assert_eq!(bytes, vec![0xDE, 0xAD, 0xBE, 0xEF]);
    }

    #[test]
    fn test_parse_dec_input() {
        let bytes = parse_dec_input("222 173 190 239").unwrap();
        assert_eq!(bytes, vec![0xDE, 0xAD, 0xBE, 0xEF]);
    }

    #[test]
    fn test_parse_oct_input() {
        let bytes = parse_oct_input("336 255 276 357").unwrap();
        assert_eq!(bytes, vec![0xDE, 0xAD, 0xBE, 0xEF]);
    }

    #[test]
    fn test_parse_bin_input() {
        let bytes = parse_bin_input("11011110 10101101").unwrap();
        assert_eq!(bytes, vec![0xDE, 0xAD]);
    }

    #[test]
    fn test_parse_mask_pattern() {
        let pattern = parse_mask_pattern("DE ?? BE EF").unwrap();
        if let SearchPattern::Mask(mask) = pattern {
            assert_eq!(mask, vec![Some(0xDE), None, Some(0xBE), Some(0xEF)]);
        } else {
            panic!("Expected Mask pattern");
        }
    }

    // ===== Hex-Regex Pattern Tests =====

    #[test]
    fn test_parse_hex_regex_literal_bytes() {
        let pattern = parse_hex_regex_pattern("DEADBEEF").unwrap();
        if let SearchPattern::Regex(regex) = pattern {
            assert_eq!(regex, "\\xDE\\xAD\\xBE\\xEF");
        } else {
            panic!("Expected Regex pattern");
        }
    }

    #[test]
    fn test_parse_hex_regex_with_dot() {
        let pattern = parse_hex_regex_pattern("FF..EF").unwrap();
        if let SearchPattern::Regex(regex) = pattern {
            assert_eq!(regex, "\\xFF..\\xEF");
        } else {
            panic!("Expected Regex pattern");
        }
    }

    #[test]
    fn test_parse_hex_regex_with_quantifiers() {
        let pattern = parse_hex_regex_pattern("AA+").unwrap();
        if let SearchPattern::Regex(regex) = pattern {
            assert_eq!(regex, "\\xAA+");
        } else {
            panic!("Expected Regex pattern");
        }

        let pattern = parse_hex_regex_pattern("BB*").unwrap();
        if let SearchPattern::Regex(regex) = pattern {
            assert_eq!(regex, "\\xBB*");
        } else {
            panic!("Expected Regex pattern");
        }

        let pattern = parse_hex_regex_pattern("CC{2,4}").unwrap();
        if let SearchPattern::Regex(regex) = pattern {
            assert_eq!(regex, "\\xCC{2,4}");
        } else {
            panic!("Expected Regex pattern");
        }
    }

    #[test]
    fn test_parse_hex_regex_with_alternation() {
        let pattern = parse_hex_regex_pattern("(DE|AD)").unwrap();
        if let SearchPattern::Regex(regex) = pattern {
            assert_eq!(regex, "(\\xDE|\\xAD)");
        } else {
            panic!("Expected Regex pattern");
        }
    }

    #[test]
    fn test_parse_hex_regex_any_byte() {
        let pattern = parse_hex_regex_pattern(".").unwrap();
        if let SearchPattern::Regex(regex) = pattern {
            assert_eq!(regex, ".");
        } else {
            panic!("Expected Regex pattern");
        }
    }

    #[test]
    fn test_parse_hex_regex_char_class_hex_range() {
        let pattern = parse_hex_regex_pattern("[00-FF]").unwrap();
        if let SearchPattern::Regex(regex) = pattern {
            assert_eq!(regex, "[\\x00-\\xFF]");
        } else {
            panic!("Expected Regex pattern");
        }
    }

    #[test]
    fn test_parse_hex_regex_char_class_negation() {
        let pattern = parse_hex_regex_pattern("[^00]").unwrap();
        if let SearchPattern::Regex(regex) = pattern {
            assert_eq!(regex, "[^\\x00]");
        } else {
            panic!("Expected Regex pattern");
        }
    }

    #[test]
    fn test_parse_hex_regex_char_class_ascii_range() {
        let pattern = parse_hex_regex_pattern("[A-Z]").unwrap();
        if let SearchPattern::Regex(regex) = pattern {
            assert_eq!(regex, "[A-Z]");
        } else {
            panic!("Expected Regex pattern");
        }
    }

    #[test]
    fn test_parse_hex_regex_with_spaces() {
        let pattern = parse_hex_regex_pattern("DE AD BE EF").unwrap();
        if let SearchPattern::Regex(regex) = pattern {
            assert_eq!(regex, "\\xDE\\xAD\\xBE\\xEF");
        } else {
            panic!("Expected Regex pattern");
        }
    }

    #[test]
    fn test_parse_hex_regex_already_escaped() {
        let pattern = parse_hex_regex_pattern("\\xFF\\x00").unwrap();
        if let SearchPattern::Regex(regex) = pattern {
            assert_eq!(regex, "\\xFF\\x00");
        } else {
            panic!("Expected Regex pattern");
        }
    }

    #[test]
    fn test_parse_hex_regex_complex_pattern() {
        // Find ELF-like headers: 7F followed by any byte, then 4C 46
        let pattern = parse_hex_regex_pattern("7F.4C46").unwrap();
        if let SearchPattern::Regex(regex) = pattern {
            assert_eq!(regex, "\\x7F.\\x4C\\x46");
        } else {
            panic!("Expected Regex pattern");
        }
    }

    #[test]
    fn test_parse_hex_regex_incomplete_pair_error() {
        let result = parse_hex_regex_pattern("DEA");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_search_pattern_hex_regex() {
        let pattern = parse_search_pattern("DEADBEEF", "hex-regex").unwrap();
        if let SearchPattern::Regex(regex) = pattern {
            assert_eq!(regex, "\\xDE\\xAD\\xBE\\xEF");
        } else {
            panic!("Expected Regex pattern");
        }

        // Also test the alternative spelling
        let pattern = parse_search_pattern("CAFEBABE", "hexregex").unwrap();
        if let SearchPattern::Regex(regex) = pattern {
            assert_eq!(regex, "\\xCA\\xFE\\xBA\\xBE");
        } else {
            panic!("Expected Regex pattern");
        }
    }

    // ===== Validation Tests =====

    #[test]
    fn test_validate_regex_with_hex_pattern() {
        let warnings = validate_search_pattern("ff", "regex");
        assert!(!warnings.is_empty());
        assert!(warnings[0].contains("hex bytes"));
    }

    #[test]
    fn test_validate_hex_with_ascii_pattern() {
        let warnings = validate_search_pattern("ERROR", "hex");
        assert!(!warnings.is_empty());
        assert!(warnings[0].contains("ASCII"));
    }

    #[test]
    fn test_validate_hex_with_regex_operators() {
        let warnings = validate_search_pattern("[0-9A-F]{2}", "hex");
        assert!(!warnings.is_empty());
        assert!(warnings[0].contains("regex operators"));
    }

    #[test]
    fn test_validate_mask_without_wildcards() {
        let warnings = validate_search_pattern("DEADBEEF", "mask");
        assert!(!warnings.is_empty());
        assert!(warnings[0].contains("wildcards"));
    }

    #[test]
    fn test_validate_ascii_looks_like_hex() {
        let warnings = validate_search_pattern("DEAD", "ascii");
        assert!(!warnings.is_empty());
        assert!(warnings[0].contains("hex bytes"));
    }

    #[test]
    fn test_validate_hex_regex_no_operators() {
        let warnings = validate_search_pattern("DEADBEEF", "hex-regex");
        assert!(!warnings.is_empty());
        assert!(warnings[0].contains("performance"));
    }

    #[test]
    fn test_validate_correct_usage_no_warnings() {
        // Proper hex usage
        assert!(validate_search_pattern("DEADBEEF", "hex").is_empty());
        // Proper ASCII usage
        assert!(validate_search_pattern("Hello World", "ascii").is_empty());
        // Proper regex usage (character classes)
        assert!(validate_search_pattern("[A-Z]+", "regex").is_empty());
        // Proper mask usage
        assert!(validate_search_pattern("DE ?? EF", "mask").is_empty());
        // Proper hex-regex usage
        assert!(validate_search_pattern("FF.{2}", "hex-regex").is_empty());
    }
}
