//! Display utilities for binfiddle.
//!
//! This module provides functions for formatting binary data into
//! human-readable representations in various formats.
/// src/utils/display.rs
use crate::error::{BinfiddleError, Result};

/// Formats bytes for display in the specified format.
///
/// # Arguments
/// * `bytes` - The raw bytes to display
/// * `format` - The output format (hex, dec, oct, bin, ascii)
/// * `chunk_size` - Number of bits per display chunk (1-64)
/// * `width` - Number of chunks per line (0 = no wrapping)
///
/// # Returns
/// A formatted string representation of the bytes.
///
/// # Errors
/// Returns `BinfiddleError::InvalidInput` if format is unknown or
/// if ASCII is requested with non-8-bit chunk size.
///
/// # Examples
/// ```ignore
/// let bytes = vec![0xDE, 0xAD, 0xBE, 0xEF];
/// let output = display_bytes(&bytes, "hex", 8, 16)?;
/// // Output: "de ad be ef"
/// ```
pub fn display_bytes(
    bytes: &[u8],
    format: &str,
    chunk_size: usize,
    width: usize,
) -> Result<String> {
    if bytes.is_empty() {
        return Ok(String::new());
    }

    match format.to_lowercase().as_str() {
        "hex" => format_chunked(bytes, chunk_size, width, format_chunk_hex),
        "dec" => format_chunked(bytes, chunk_size, width, format_chunk_dec),
        "oct" => format_chunked(bytes, chunk_size, width, format_chunk_oct),
        "bin" => format_chunked(bytes, chunk_size, width, format_chunk_bin),
        "ascii" => {
            if chunk_size != 8 {
                return Err(BinfiddleError::InvalidInput(
                    "ASCII output only supported for 8-bit chunks".to_string(),
                ));
            }
            format_ascii(bytes, width)
        }
        _ => Err(BinfiddleError::InvalidInput(format!(
            "Unknown output format: '{}'. Supported: hex, dec, oct, bin, ascii",
            format
        ))),
    }
}

/// Generic chunked formatting function.
///
/// This function handles the common logic of:
/// 1. Extracting bits from the byte stream according to chunk_size
/// 2. Applying the format function to each chunk
/// 3. Applying line width wrapping
fn format_chunked<F>(bytes: &[u8], chunk_size: usize, width: usize, format_fn: F) -> Result<String>
where
    F: Fn(u64, usize) -> String,
{
    let mut output = String::new();
    let total_bits = bytes.len() * 8;
    let mut bit_offset = 0;
    let mut chunks_on_line = 0;

    while bit_offset < total_bits {
        // Calculate how many bits we can extract for this chunk
        let bits_remaining = total_bits - bit_offset;
        let bits_to_extract = chunk_size.min(bits_remaining);

        // Extract the chunk value
        let value = extract_bits(bytes, bit_offset, bits_to_extract);

        // Format the chunk
        let formatted = format_fn(value, bits_to_extract);

        // Add separator if not first chunk on line
        if chunks_on_line > 0 {
            output.push(' ');
        }
        output.push_str(&formatted);

        chunks_on_line += 1;
        bit_offset += chunk_size;

        // Handle line wrapping
        if width > 0 && chunks_on_line >= width && bit_offset < total_bits {
            output.push('\n');
            chunks_on_line = 0;
        }
    }

    Ok(output)
}

/// Extracts a value from a bit stream.
///
/// # Arguments
/// * `bytes` - The byte array to extract from
/// * `bit_offset` - Starting bit position (0-indexed from the start)
/// * `bit_count` - Number of bits to extract (1-64)
///
/// # Returns
/// The extracted bits as a u64 value.
fn extract_bits(bytes: &[u8], bit_offset: usize, bit_count: usize) -> u64 {
    if bit_count == 0 || bit_count > 64 {
        return 0;
    }

    let mut value: u64 = 0;
    let mut bits_collected = 0;

    while bits_collected < bit_count {
        let current_bit_pos = bit_offset + bits_collected;
        let byte_index = current_bit_pos / 8;
        let bit_in_byte = 7 - (current_bit_pos % 8); // MSB first

        if byte_index >= bytes.len() {
            break;
        }

        let bit = ((bytes[byte_index] >> bit_in_byte) & 1) as u64;
        value = (value << 1) | bit;
        bits_collected += 1;
    }

    value
}

/// Formats a chunk value as hexadecimal.
fn format_chunk_hex(value: u64, bit_count: usize) -> String {
    let hex_digits = (bit_count + 3) / 4; // Ceiling division
    format!("{:0width$x}", value, width = hex_digits)
}

/// Formats a chunk value as decimal.
fn format_chunk_dec(value: u64, _bit_count: usize) -> String {
    format!("{}", value)
}

/// Formats a chunk value as octal.
fn format_chunk_oct(value: u64, _bit_count: usize) -> String {
    format!("{:o}", value)
}

/// Formats a chunk value as binary.
fn format_chunk_bin(value: u64, bit_count: usize) -> String {
    format!("{:0width$b}", value, width = bit_count)
}

/// Formats bytes as ASCII characters.
///
/// Printable ASCII characters (0x20-0x7E) are shown as-is.
/// Non-printable characters are replaced with '.'.
fn format_ascii(bytes: &[u8], width: usize) -> Result<String> {
    let mut output = String::new();
    let mut chars_on_line = 0;

    for &byte in bytes {
        let ch = if byte >= 0x20 && byte <= 0x7E {
            byte as char
        } else {
            '.'
        };
        output.push(ch);
        chars_on_line += 1;

        // Handle line wrapping
        if width > 0 && chars_on_line >= width && chars_on_line < bytes.len() {
            output.push('\n');
            chars_on_line = 0;
        }
    }

    Ok(output)
}

/// Formats a single match result for search output.
///
/// # Arguments
/// * `offset` - The byte offset where the match was found
/// * `data` - The matched data
/// * `format` - The output format
/// * `chunk_size` - Bits per display chunk
///
/// # Returns
/// A formatted string showing offset and matched data.
pub fn format_match(offset: usize, data: &[u8], format: &str, chunk_size: usize) -> Result<String> {
    let formatted_data = display_bytes(data, format, chunk_size, 0)?;
    Ok(format!("0x{:08x}: {}", offset, formatted_data))
}

/// Formats a match with context bytes before and after.
///
/// # Arguments
/// * `offset` - The byte offset where the match was found
/// * `match_data` - The matched data
/// * `before_context` - Bytes before the match (may be empty)
/// * `after_context` - Bytes after the match (may be empty)
/// * `format` - The output format
/// * `chunk_size` - Bits per display chunk
///
/// # Returns
/// A formatted string showing context, match, and position info.
pub fn format_match_with_context(
    offset: usize,
    match_data: &[u8],
    before_context: &[u8],
    after_context: &[u8],
    format: &str,
    chunk_size: usize,
) -> Result<String> {
    let mut output = String::new();

    // Calculate actual start offset (accounting for before context)
    // Note: reserved for future use when implementing inline context display
    let _display_offset = offset.saturating_sub(before_context.len());

    output.push_str(&format!("Match at 0x{:08x}:\n", offset));

    if !before_context.is_empty() {
        let before_fmt = display_bytes(before_context, format, chunk_size, 0)?;
        output.push_str(&format!("  Before: {}\n", before_fmt));
    }

    let match_fmt = display_bytes(match_data, format, chunk_size, 0)?;
    output.push_str(&format!("  Match:  {}\n", match_fmt));

    if !after_context.is_empty() {
        let after_fmt = display_bytes(after_context, format, chunk_size, 0)?;
        output.push_str(&format!("  After:  {}", after_fmt));
    }

    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_display_hex_basic() {
        let bytes = vec![0xDE, 0xAD, 0xBE, 0xEF];
        let output = display_bytes(&bytes, "hex", 8, 16).unwrap();
        assert_eq!(output, "de ad be ef");
    }

    #[test]
    fn test_display_hex_4bit_chunks() {
        let bytes = vec![0xDE, 0xAD];
        let output = display_bytes(&bytes, "hex", 4, 16).unwrap();
        assert_eq!(output, "d e a d");
    }

    #[test]
    fn test_display_hex_16bit_chunks() {
        let bytes = vec![0xDE, 0xAD, 0xBE, 0xEF];
        let output = display_bytes(&bytes, "hex", 16, 16).unwrap();
        assert_eq!(output, "dead beef");
    }

    #[test]
    fn test_display_dec() {
        let bytes = vec![0xDE, 0xAD, 0xBE, 0xEF];
        let output = display_bytes(&bytes, "dec", 8, 16).unwrap();
        assert_eq!(output, "222 173 190 239");
    }

    #[test]
    fn test_display_bin() {
        let bytes = vec![0xDE, 0xAD];
        let output = display_bytes(&bytes, "bin", 8, 16).unwrap();
        assert_eq!(output, "11011110 10101101");
    }

    #[test]
    fn test_display_ascii_printable() {
        let bytes = b"Hello".to_vec();
        let output = display_bytes(&bytes, "ascii", 8, 16).unwrap();
        assert_eq!(output, "Hello");
    }

    #[test]
    fn test_display_ascii_non_printable() {
        let bytes = vec![0x00, 0x48, 0x65, 0x6C, 0x6C, 0x6F, 0x00];
        let output = display_bytes(&bytes, "ascii", 8, 16).unwrap();
        assert_eq!(output, ".Hello.");
    }

    #[test]
    fn test_display_with_width() {
        let bytes = vec![0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07];
        let output = display_bytes(&bytes, "hex", 8, 4).unwrap();
        assert_eq!(output, "00 01 02 03\n04 05 06 07");
    }

    #[test]
    fn test_extract_bits() {
        let bytes = vec![0b10110100, 0b01101001];

        // First 4 bits should be 1011 = 11
        assert_eq!(extract_bits(&bytes, 0, 4), 0b1011);

        // Bits 4-7 should be 0100 = 4
        assert_eq!(extract_bits(&bytes, 4, 4), 0b0100);

        // Full first byte
        assert_eq!(extract_bits(&bytes, 0, 8), 0b10110100);
    }

    #[test]
    fn test_format_match() {
        let data = vec![0xDE, 0xAD];
        let output = format_match(256, &data, "hex", 8).unwrap();
        assert_eq!(output, "0x00000100: de ad");
    }
}
