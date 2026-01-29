# src/utils/parsing.rs
//! Parsing utilities for binfiddle.
//!
//! This module provides functions for parsing:
//! - Range specifications (e.g., "10..20", "0x100..", "..0xFF")
//! - Input data in various formats (hex, dec, oct, bin, ascii)
//! - Search patterns for the search command

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
        "mask" => parse_mask_pattern(input),
        _ => Err(BinfiddleError::InvalidInput(format!(
            "Unknown search pattern format: '{}'. Supported: hex, ascii, dec, oct, bin, regex, mask",
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
}
