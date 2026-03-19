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

/// Formats bytes with hex address prefixes on each line, xxd-style.
///
/// Produces output like:
/// ```text
/// 0x00000000: 7f 45 4c 46 02 01 01 00 00 00 00 00 00 00 00 00  |.ELF............|
/// 0x00000010: 03 00 3e 00 01 00 00 00 40 10 00 00 00 00 00 00  |..>.....@.......|
/// ```
///
/// # Arguments
/// * `bytes` - The raw bytes to display
/// * `format` - The output format (hex, dec, oct, bin)
/// * `chunk_size` - Number of bits per display chunk (1-64)
/// * `width` - Number of chunks per line (must be > 0)
/// * `base_offset` - Starting address for the first line
/// * `show_ascii` - Whether to show ASCII sidebar (only for hex format with 8-bit chunks)
///
/// # Returns
/// A formatted string with address prefixes on each line.
pub fn display_bytes_with_offset(
    bytes: &[u8],
    format: &str,
    chunk_size: usize,
    width: usize,
    base_offset: usize,
    show_ascii: bool,
) -> Result<String> {
    if bytes.is_empty() {
        return Ok(String::new());
    }

    // Width must be > 0 for offset display to make sense
    let effective_width = if width == 0 { 16 } else { width };

    // Calculate bytes per line based on chunk_size and width
    // Each chunk is chunk_size bits, width chunks per line
    let bits_per_line = effective_width * chunk_size;
    let bytes_per_line = (bits_per_line + 7) / 8; // ceiling division

    // Determine address width based on max offset
    let max_addr = base_offset + bytes.len();
    let addr_width = if max_addr > 0xFFFF_FFFF {
        16 // 64-bit addresses
    } else if max_addr > 0xFFFF {
        8 // 32-bit addresses
    } else {
        4 // 16-bit addresses (minimum)
    };

    let show_sidebar = show_ascii && format == "hex" && chunk_size == 8;

    let mut output = String::new();

    for (line_idx, chunk) in bytes.chunks(bytes_per_line).enumerate() {
        let line_offset = base_offset + line_idx * bytes_per_line;

        // Address prefix
        if !output.is_empty() {
            output.push('\n');
        }
        output.push_str(&format!("0x{:0width$x}: ", line_offset, width = addr_width));

        // Format the line data (width=0 to prevent internal wrapping)
        let line_str = display_bytes(chunk, format, chunk_size, 0)?;
        output.push_str(&line_str);

        // Pad short last line to align ASCII sidebar
        if show_sidebar && chunk.len() < bytes_per_line {
            // Calculate expected formatted width for a full line
            let full_line_str = display_bytes(&vec![0u8; bytes_per_line], format, chunk_size, 0)?;
            let padding = full_line_str.len().saturating_sub(line_str.len());
            for _ in 0..padding {
                output.push(' ');
            }
        }

        // ASCII sidebar
        if show_sidebar {
            output.push_str("  |");
            for &byte in chunk {
                if byte >= 0x20 && byte <= 0x7E {
                    output.push(byte as char);
                } else {
                    output.push('.');
                }
            }
            output.push('|');
        }
    }

    Ok(output)
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

    for (i, &byte) in bytes.iter().enumerate() {
        let ch = if byte >= 0x20 && byte <= 0x7E {
            byte as char
        } else {
            '.'
        };
        output.push(ch);
        chars_on_line += 1;

        // Handle line wrapping (don't add newline after last byte)
        if width > 0 && chars_on_line >= width && i + 1 < bytes.len() {
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

// ANSI color codes for terminal output
const ANSI_RESET: &str = "\x1b[0m";
const ANSI_BOLD_RED: &str = "\x1b[1;31m";
const ANSI_BOLD_GREEN: &str = "\x1b[1;32m";
const ANSI_CYAN: &str = "\x1b[36m";
const ANSI_DIM: &str = "\x1b[2m";

/// Formats a single match result with color highlighting.
///
/// # Arguments
/// * `offset` - The byte offset where the match was found
/// * `data` - The matched data
/// * `format` - The output format
/// * `chunk_size` - Bits per display chunk
///
/// # Returns
/// A formatted string with ANSI color codes showing offset and matched data.
pub fn format_match_colored(
    offset: usize,
    data: &[u8],
    format: &str,
    chunk_size: usize,
) -> Result<String> {
    let formatted_data = display_bytes(data, format, chunk_size, 0)?;
    Ok(format!(
        "{}0x{:08x}{}: {}{}{}",
        ANSI_CYAN, offset, ANSI_RESET, ANSI_BOLD_RED, formatted_data, ANSI_RESET
    ))
}

/// Formats a match with context bytes, using color to highlight the match.
///
/// The output shows the match in red/bold, with context bytes in dim.
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
/// A formatted string with ANSI color codes for terminal display.
pub fn format_match_with_context_colored(
    offset: usize,
    match_data: &[u8],
    before_context: &[u8],
    after_context: &[u8],
    format: &str,
    chunk_size: usize,
) -> Result<String> {
    let mut output = String::new();

    output.push_str(&format!(
        "{}Match at 0x{:08x}{}:\n",
        ANSI_BOLD_GREEN, offset, ANSI_RESET
    ));

    if !before_context.is_empty() {
        let before_fmt = display_bytes(before_context, format, chunk_size, 0)?;
        output.push_str(&format!(
            "  Before: {}{}{}\n",
            ANSI_DIM, before_fmt, ANSI_RESET
        ));
    }

    let match_fmt = display_bytes(match_data, format, chunk_size, 0)?;
    output.push_str(&format!(
        "  Match:  {}{}{}\n",
        ANSI_BOLD_RED, match_fmt, ANSI_RESET
    ));

    if !after_context.is_empty() {
        let after_fmt = display_bytes(after_context, format, chunk_size, 0)?;
        output.push_str(&format!(
            "  After:  {}{}{}",
            ANSI_DIM, after_fmt, ANSI_RESET
        ));
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

    #[test]
    fn test_format_match_colored_contains_ansi() {
        let data = vec![0xDE, 0xAD];
        let output = format_match_colored(256, &data, "hex", 8).unwrap();

        // Should contain ANSI escape codes
        assert!(output.contains("\x1b["), "Output should contain ANSI codes");
        assert!(
            output.contains(ANSI_RESET),
            "Output should contain reset code"
        );
        assert!(
            output.contains(ANSI_CYAN),
            "Output should contain cyan for offset"
        );
        assert!(
            output.contains(ANSI_BOLD_RED),
            "Output should contain red for match"
        );

        // Should still contain the actual data
        assert!(output.contains("00000100"), "Output should contain offset");
        assert!(output.contains("de ad"), "Output should contain match data");
    }

    #[test]
    fn test_format_match_with_context_colored() {
        let match_data = vec![0xBE, 0xEF];
        let before = vec![0xDE, 0xAD];
        let after = vec![0xCA, 0xFE];

        let output =
            format_match_with_context_colored(2, &match_data, &before, &after, "hex", 8).unwrap();

        // Should contain ANSI codes
        assert!(
            output.contains(ANSI_BOLD_GREEN),
            "Should have green for header"
        );
        assert!(output.contains(ANSI_DIM), "Should have dim for context");
        assert!(output.contains(ANSI_BOLD_RED), "Should have red for match");
        assert!(output.contains(ANSI_RESET), "Should have reset codes");

        // Should contain the data
        assert!(output.contains("Match at"));
        assert!(output.contains("Before:"));
        assert!(output.contains("Match:"));
        assert!(output.contains("After:"));
        assert!(output.contains("de ad")); // before context
        assert!(output.contains("be ef")); // match
        assert!(output.contains("ca fe")); // after context
    }

    #[test]
    fn test_format_match_no_color_no_ansi() {
        let data = vec![0xDE, 0xAD];
        let output = format_match(256, &data, "hex", 8).unwrap();

        // Non-colored version should NOT contain ANSI escape codes
        assert!(
            !output.contains("\x1b["),
            "Non-colored output should not contain ANSI codes"
        );
    }

    // === Tests for display_bytes_with_offset ===

    #[test]
    fn test_offset_display_basic() {
        let bytes = vec![0x7F, 0x45, 0x4C, 0x46];
        let output = display_bytes_with_offset(&bytes, "hex", 8, 16, 0, false).unwrap();
        assert!(output.starts_with("0x"), "Should start with address prefix");
        assert!(output.contains("7f 45 4c 46"), "Should contain hex data");
    }

    #[test]
    fn test_offset_display_multiline() {
        // 8 bytes with width=4 → should produce 2 lines
        let bytes: Vec<u8> = (0..8).collect();
        let output = display_bytes_with_offset(&bytes, "hex", 8, 4, 0, false).unwrap();
        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(
            lines.len(),
            2,
            "Should produce 2 lines for 8 bytes at width=4"
        );
        assert!(
            lines[0].contains("0x0000:"),
            "First line should start at offset 0"
        );
        assert!(
            lines[1].contains("0x0004:"),
            "Second line should start at offset 4"
        );
    }

    #[test]
    fn test_offset_display_with_base_offset() {
        let bytes = vec![0xDE, 0xAD];
        let output = display_bytes_with_offset(&bytes, "hex", 8, 16, 0x100, false).unwrap();
        assert!(output.contains("0x0100:"), "Should show base offset 0x100");
    }

    #[test]
    fn test_offset_display_with_ascii_sidebar() {
        let bytes = b"Hello\x00World".to_vec();
        let output = display_bytes_with_offset(&bytes, "hex", 8, 16, 0, true).unwrap();
        assert!(
            output.contains("|Hello.World|"),
            "Should show ASCII sidebar with . for non-printable"
        );
    }

    #[test]
    fn test_offset_display_ascii_sidebar_only_for_hex_8bit() {
        // ASCII sidebar should only appear for hex format with 8-bit chunks
        let bytes = vec![0x48, 0x69]; // "Hi"
        let output_hex = display_bytes_with_offset(&bytes, "hex", 8, 16, 0, true).unwrap();
        assert!(
            output_hex.contains("|Hi|"),
            "Hex 8-bit should show ASCII sidebar"
        );

        let output_dec = display_bytes_with_offset(&bytes, "dec", 8, 16, 0, true).unwrap();
        assert!(
            !output_dec.contains("|"),
            "Dec format should not show ASCII sidebar"
        );
    }

    #[test]
    fn test_offset_display_empty() {
        let output = display_bytes_with_offset(&[], "hex", 8, 16, 0, false).unwrap();
        assert_eq!(output, "", "Empty bytes should produce empty output");
    }

    #[test]
    fn test_offset_display_large_address() {
        let bytes = vec![0xFF];
        // Address > 0xFFFF should use 8-digit width
        let output = display_bytes_with_offset(&bytes, "hex", 8, 16, 0x10000, false).unwrap();
        assert!(
            output.contains("0x00010000:"),
            "Should use 8-digit address for offset > 0xFFFF"
        );
    }

    #[test]
    fn test_offset_display_short_last_line_padding() {
        // 5 bytes with width=4 → line 1 has 4 bytes, line 2 has 1 byte
        // ASCII sidebar on line 2 should be aligned
        let bytes = vec![0x41, 0x42, 0x43, 0x44, 0x45]; // "ABCDE"
        let output = display_bytes_with_offset(&bytes, "hex", 8, 4, 0, true).unwrap();
        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("|ABCD|"), "First line sidebar");
        assert!(lines[1].contains("|E|"), "Second line sidebar");
    }

    // === Tests for ASCII wrapping fix ===

    #[test]
    fn test_ascii_wrapping_exact_width() {
        // Exactly width bytes — should NOT have trailing newline
        let bytes = vec![0x41, 0x42, 0x43, 0x44]; // "ABCD"
        let output = display_bytes(&bytes, "ascii", 8, 4).unwrap();
        assert_eq!(
            output, "ABCD",
            "Exact width should not have trailing newline"
        );
    }

    #[test]
    fn test_ascii_wrapping_double_width() {
        // Exactly 2x width bytes — should have one internal newline, no trailing
        let bytes = b"ABCDEFGH".to_vec();
        let output = display_bytes(&bytes, "ascii", 8, 4).unwrap();
        assert_eq!(output, "ABCD\nEFGH", "Double width should have one newline");
    }
}
