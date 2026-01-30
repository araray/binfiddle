//! # Convert Command
//!
//! Provides text encoding conversion, line ending normalization, and BOM handling
//! for binary data containing text. This command enables `binfiddle` to correctly
//! interpret, display, and transform data based on common text encodings.
//!
//! ## Supported Encodings
//!
//! - UTF-8 (default)
//! - UTF-16LE (Little Endian)
//! - UTF-16BE (Big Endian)
//! - Latin-1 / ISO-8859-1
//! - Windows-1252
//!
//! ## Newline Modes
//!
//! - `unix`: LF (`\n`) - Linux, macOS
//! - `windows`: CRLF (`\r\n`) - Windows
//! - `mac`: CR (`\r`) - Classic Mac OS
//! - `keep`: Preserve original line endings
//!
//! ## BOM Handling
//!
//! - `add`: Add BOM appropriate for target encoding
//! - `remove`: Strip any existing BOM
//! - `keep`: Preserve existing BOM (or absence thereof)
//!
//! ## Error Modes
//!
//! - `strict`: Fail on any encoding/decoding error
//! - `replace`: Replace invalid characters with U+FFFD (replacement character)
//! - `ignore`: Skip invalid byte sequences

use crate::{BinfiddleError, Result};
use encoding_rs::{Encoding, UTF_16BE, UTF_16LE, UTF_8, WINDOWS_1252};

// ============================================================================
// BOM Constants
// ============================================================================

/// UTF-8 Byte Order Mark
const UTF8_BOM: &[u8] = &[0xEF, 0xBB, 0xBF];

/// UTF-16 Big Endian Byte Order Mark
const UTF16_BE_BOM: &[u8] = &[0xFE, 0xFF];

/// UTF-16 Little Endian Byte Order Mark
const UTF16_LE_BOM: &[u8] = &[0xFF, 0xFE];

// ============================================================================
// Encoding Name Parsing
// ============================================================================

/// Parse an encoding name string to an `encoding_rs::Encoding` reference.
///
/// # Supported Encodings
///
/// | Input String | Encoding |
/// |--------------|----------|
/// | `utf-8`, `utf8` | UTF-8 |
/// | `utf-16le`, `utf16le` | UTF-16 Little Endian |
/// | `utf-16be`, `utf16be` | UTF-16 Big Endian |
/// | `latin-1`, `latin1`, `iso-8859-1` | Windows-1252 (superset of Latin-1) |
/// | `windows-1252`, `cp1252` | Windows-1252 |
///
/// # Errors
///
/// Returns `InvalidInput` if the encoding name is not recognized.
///
/// # Examples
///
/// ```ignore
/// use binfiddle::commands::convert::parse_encoding;
/// let encoding = parse_encoding("utf-16le").unwrap();
/// assert_eq!(encoding.name(), "UTF-16LE");
/// ```
pub fn parse_encoding(name: &str) -> Result<&'static Encoding> {
    match name.to_lowercase().as_str() {
        "utf-8" | "utf8" => Ok(UTF_8),
        "utf-16le" | "utf16le" => Ok(UTF_16LE),
        "utf-16be" | "utf16be" => Ok(UTF_16BE),
        // Latin-1 is a subset of Windows-1252, which encoding_rs provides
        "latin-1" | "latin1" | "iso-8859-1" => Ok(WINDOWS_1252),
        "windows-1252" | "cp1252" => Ok(WINDOWS_1252),
        _ => Err(BinfiddleError::InvalidInput(format!(
            "Unsupported encoding: '{}'. Supported: utf-8, utf-16le, utf-16be, latin-1, windows-1252",
            name
        ))),
    }
}

// ============================================================================
// Configuration Types
// ============================================================================

/// Newline conversion mode.
///
/// Specifies how line endings should be converted during text transformation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NewlineMode {
    /// Unix-style: LF (`\n`)
    Unix,
    /// Windows-style: CRLF (`\r\n`)
    Windows,
    /// Classic Mac OS-style: CR (`\r`)
    Mac,
    /// Keep original line endings unchanged
    #[default]
    Keep,
}

impl NewlineMode {
    /// Parse a newline mode from a string.
    ///
    /// # Arguments
    ///
    /// * `s` - Mode string (case-insensitive)
    ///
    /// # Returns
    ///
    /// The corresponding `NewlineMode`.
    ///
    /// # Errors
    ///
    /// Returns `InvalidInput` if the mode string is not recognized.
    pub fn from_str(s: &str) -> Result<Self> {
        match s.to_lowercase().as_str() {
            "unix" | "lf" => Ok(NewlineMode::Unix),
            "windows" | "crlf" | "dos" => Ok(NewlineMode::Windows),
            "mac" | "cr" => Ok(NewlineMode::Mac),
            "keep" | "preserve" => Ok(NewlineMode::Keep),
            _ => Err(BinfiddleError::InvalidInput(format!(
                "Unknown newline mode: '{}'. Supported: unix, windows, mac, keep",
                s
            ))),
        }
    }

    /// Get the newline byte sequence for this mode.
    pub fn as_bytes(&self) -> &'static [u8] {
        match self {
            NewlineMode::Unix => b"\n",
            NewlineMode::Windows => b"\r\n",
            NewlineMode::Mac => b"\r",
            NewlineMode::Keep => b"\n", // Fallback, shouldn't be used directly
        }
    }
}

/// BOM (Byte Order Mark) handling mode.
///
/// Specifies how BOMs should be handled during encoding conversion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BomMode {
    /// Add a BOM appropriate for the target encoding
    Add,
    /// Remove any existing BOM
    Remove,
    /// Keep existing BOM (or absence thereof)
    #[default]
    Keep,
}

impl BomMode {
    /// Parse a BOM mode from a string.
    ///
    /// # Arguments
    ///
    /// * `s` - Mode string (case-insensitive)
    ///
    /// # Returns
    ///
    /// The corresponding `BomMode`.
    ///
    /// # Errors
    ///
    /// Returns `InvalidInput` if the mode string is not recognized.
    pub fn from_str(s: &str) -> Result<Self> {
        match s.to_lowercase().as_str() {
            "add" | "yes" | "true" => Ok(BomMode::Add),
            "remove" | "strip" | "no" | "false" => Ok(BomMode::Remove),
            "keep" | "preserve" => Ok(BomMode::Keep),
            _ => Err(BinfiddleError::InvalidInput(format!(
                "Unknown BOM mode: '{}'. Supported: add, remove, keep",
                s
            ))),
        }
    }
}

/// Error handling mode for encoding/decoding operations.
///
/// Specifies how to handle invalid byte sequences during conversion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ErrorMode {
    /// Fail immediately on any encoding/decoding error
    Strict,
    /// Replace invalid characters with U+FFFD (replacement character)
    #[default]
    Replace,
    /// Skip invalid byte sequences (may lose data)
    Ignore,
}

impl ErrorMode {
    /// Parse an error mode from a string.
    ///
    /// # Arguments
    ///
    /// * `s` - Mode string (case-insensitive)
    ///
    /// # Returns
    ///
    /// The corresponding `ErrorMode`.
    ///
    /// # Errors
    ///
    /// Returns `InvalidInput` if the mode string is not recognized.
    pub fn from_str(s: &str) -> Result<Self> {
        match s.to_lowercase().as_str() {
            "strict" | "error" | "fail" => Ok(ErrorMode::Strict),
            "replace" | "substitute" => Ok(ErrorMode::Replace),
            "ignore" | "skip" => Ok(ErrorMode::Ignore),
            _ => Err(BinfiddleError::InvalidInput(format!(
                "Unknown error mode: '{}'. Supported: strict, replace, ignore",
                s
            ))),
        }
    }
}

/// Configuration for the convert command.
#[derive(Debug, Clone)]
pub struct ConvertConfig {
    /// Source encoding (default: UTF-8)
    pub from_encoding: &'static Encoding,
    /// Target encoding (default: UTF-8)
    pub to_encoding: &'static Encoding,
    /// Newline conversion mode
    pub newlines: NewlineMode,
    /// BOM handling mode
    pub bom: BomMode,
    /// Error handling mode
    pub on_error: ErrorMode,
}

impl Default for ConvertConfig {
    fn default() -> Self {
        Self {
            from_encoding: UTF_8,
            to_encoding: UTF_8,
            newlines: NewlineMode::Keep,
            bom: BomMode::Keep,
            on_error: ErrorMode::Replace,
        }
    }
}

// ============================================================================
// Convert Command Implementation
// ============================================================================

/// The convert command for encoding and line ending transformations.
///
/// # Example
///
/// ```ignore
/// use binfiddle::commands::convert::{ConvertCommand, ConvertConfig, NewlineMode};
///
/// // Convert UTF-16LE to UTF-8 with Unix line endings
/// let config = ConvertConfig {
///     from_encoding: encoding_rs::UTF_16LE,
///     to_encoding: encoding_rs::UTF_8,
///     newlines: NewlineMode::Unix,
///     ..Default::default()
/// };
///
/// let cmd = ConvertCommand::new(config);
/// let output = cmd.convert(&input_bytes)?;
/// ```
pub struct ConvertCommand {
    config: ConvertConfig,
}

impl ConvertCommand {
    /// Creates a new convert command with the given configuration.
    ///
    /// # Arguments
    ///
    /// * `config` - Conversion configuration
    pub fn new(config: ConvertConfig) -> Self {
        Self { config }
    }

    /// Converts the input data according to the configuration.
    ///
    /// The conversion pipeline is:
    /// 1. Strip BOM from input (if present)
    /// 2. Decode from source encoding to String
    /// 3. Convert line endings (if configured)
    /// 4. Encode to target encoding
    /// 5. Add/remove BOM (if configured)
    ///
    /// # Arguments
    ///
    /// * `input` - Input bytes to convert
    ///
    /// # Returns
    ///
    /// Converted bytes.
    ///
    /// # Errors
    ///
    /// Returns error if:
    /// - Decoding fails and error mode is `Strict`
    /// - Encoding fails and error mode is `Strict`
    pub fn convert(&self, input: &[u8]) -> Result<Vec<u8>> {
        // Step 1: Strip any existing BOM and remember if we had one
        let (data_without_bom, had_bom) = self.strip_bom(input);

        // Step 2: Decode from source encoding
        let decoded = self.decode(data_without_bom)?;

        // Step 3: Convert line endings
        let newlines_converted = self.convert_newlines(&decoded);

        // Step 4: Encode to target encoding
        let encoded = self.encode(&newlines_converted)?;

        // Step 5: Handle BOM for output
        let output = self.apply_bom_mode(&encoded, had_bom);

        Ok(output)
    }

    /// Strip BOM from input if present.
    ///
    /// Returns the data without BOM and a flag indicating if BOM was present.
    fn strip_bom<'a>(&self, input: &'a [u8]) -> (&'a [u8], bool) {
        // Check for UTF-8 BOM
        if input.starts_with(UTF8_BOM) {
            return (&input[UTF8_BOM.len()..], true);
        }
        // Check for UTF-16 BE BOM
        if input.starts_with(UTF16_BE_BOM) {
            return (&input[UTF16_BE_BOM.len()..], true);
        }
        // Check for UTF-16 LE BOM
        if input.starts_with(UTF16_LE_BOM) {
            return (&input[UTF16_LE_BOM.len()..], true);
        }
        (input, false)
    }

    /// Decode input bytes from source encoding to String.
    fn decode(&self, input: &[u8]) -> Result<String> {
        let (decoded, _encoding_used, had_errors) = self.config.from_encoding.decode(input);

        match self.config.on_error {
            ErrorMode::Strict if had_errors => Err(BinfiddleError::Parse(format!(
                "Decoding error: input contains invalid sequences for {} encoding",
                self.config.from_encoding.name()
            ))),
            ErrorMode::Ignore if had_errors => {
                // Filter out replacement characters for ignore mode
                Ok(decoded.replace('\u{FFFD}', ""))
            }
            _ => Ok(decoded.into_owned()),
        }
    }

    /// Convert line endings in the string.
    fn convert_newlines(&self, text: &str) -> String {
        match self.config.newlines {
            NewlineMode::Keep => text.to_string(),
            NewlineMode::Unix => {
                // Convert all line endings to LF
                // Order matters: CRLF first, then standalone CR
                text.replace("\r\n", "\n").replace('\r', "\n")
            }
            NewlineMode::Windows => {
                // Convert all to CRLF
                // First normalize to LF, then convert to CRLF
                let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
                normalized.replace('\n', "\r\n")
            }
            NewlineMode::Mac => {
                // Convert all to CR
                text.replace("\r\n", "\r").replace('\n', "\r")
            }
        }
    }

    /// Encode string to target encoding.
    fn encode(&self, text: &str) -> Result<Vec<u8>> {
        // encoding_rs doesn't support encoding TO UTF-16, only FROM UTF-16
        // So we need to handle UTF-16 encoding manually
        if self.config.to_encoding == UTF_16LE {
            return Ok(self.encode_utf16le(text));
        } else if self.config.to_encoding == UTF_16BE {
            return Ok(self.encode_utf16be(text));
        }

        // For other encodings, use encoding_rs
        let (encoded, _encoding_used, had_errors) = self.config.to_encoding.encode(text);

        match self.config.on_error {
            ErrorMode::Strict if had_errors => Err(BinfiddleError::Parse(format!(
                "Encoding error: text contains characters that cannot be represented in {} encoding",
                self.config.to_encoding.name()
            ))),
            _ => Ok(encoded.into_owned()),
        }
    }

    /// Apply BOM mode to the encoded output.
    fn apply_bom_mode(&self, data: &[u8], had_bom: bool) -> Vec<u8> {
        match self.config.bom {
            BomMode::Add => {
                let mut output = self.get_bom_for_encoding().to_vec();
                output.extend_from_slice(data);
                output
            }
            BomMode::Remove => data.to_vec(),
            BomMode::Keep => {
                if had_bom {
                    let mut output = self.get_bom_for_encoding().to_vec();
                    output.extend_from_slice(data);
                    output
                } else {
                    data.to_vec()
                }
            }
        }
    }

    /// Get the appropriate BOM for the target encoding.
    fn get_bom_for_encoding(&self) -> &'static [u8] {
        if self.config.to_encoding == UTF_8 {
            UTF8_BOM
        } else if self.config.to_encoding == UTF_16BE {
            UTF16_BE_BOM
        } else if self.config.to_encoding == UTF_16LE {
            UTF16_LE_BOM
        } else {
            // No BOM for other encodings
            &[]
        }
    }

    /// Returns information about the conversion that will be performed.
    ///
    /// Useful for `--dry-run` or verbose mode.
    pub fn describe(&self) -> String {
        format!(
            "Convert: {} → {}, newlines: {:?}, bom: {:?}, on_error: {:?}",
            self.config.from_encoding.name(),
            self.config.to_encoding.name(),
            self.config.newlines,
            self.config.bom,
            self.config.on_error
        )
    }

    /// Encode a string to UTF-16LE bytes.
    ///
    /// encoding_rs doesn't support encoding TO UTF-16, so we implement it manually.
    fn encode_utf16le(&self, text: &str) -> Vec<u8> {
        let mut result = Vec::with_capacity(text.len() * 2);
        for code_unit in text.encode_utf16() {
            result.extend_from_slice(&code_unit.to_le_bytes());
        }
        result
    }

    /// Encode a string to UTF-16BE bytes.
    ///
    /// encoding_rs doesn't support encoding TO UTF-16, so we implement it manually.
    fn encode_utf16be(&self, text: &str) -> Vec<u8> {
        let mut result = Vec::with_capacity(text.len() * 2);
        for code_unit in text.encode_utf16() {
            result.extend_from_slice(&code_unit.to_be_bytes());
        }
        result
    }
}

// ============================================================================
// Utility Functions
// ============================================================================

/// Detect the encoding of input data based on BOM.
///
/// Returns `None` if no BOM is found.
///
/// # Arguments
///
/// * `data` - Input bytes to check
///
/// # Returns
///
/// The detected encoding, or `None` if no BOM is present.
pub fn detect_bom_encoding(data: &[u8]) -> Option<&'static Encoding> {
    if data.starts_with(UTF8_BOM) {
        Some(UTF_8)
    } else if data.starts_with(UTF16_BE_BOM) {
        Some(UTF_16BE)
    } else if data.starts_with(UTF16_LE_BOM) {
        Some(UTF_16LE)
    } else {
        None
    }
}

/// Get the BOM length for a given encoding.
///
/// Returns 0 if the encoding doesn't have a standard BOM.
pub fn bom_length(encoding: &'static Encoding) -> usize {
    if encoding == UTF_8 {
        UTF8_BOM.len()
    } else if encoding == UTF_16BE || encoding == UTF_16LE {
        UTF16_BE_BOM.len() // Both are 2 bytes
    } else {
        0
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ------------------------------------------------------------------------
    // Encoding Parsing Tests
    // ------------------------------------------------------------------------

    #[test]
    fn test_parse_encoding_utf8() {
        assert_eq!(parse_encoding("utf-8").unwrap(), UTF_8);
        assert_eq!(parse_encoding("UTF-8").unwrap(), UTF_8);
        assert_eq!(parse_encoding("utf8").unwrap(), UTF_8);
    }

    #[test]
    fn test_parse_encoding_utf16() {
        assert_eq!(parse_encoding("utf-16le").unwrap(), UTF_16LE);
        assert_eq!(parse_encoding("UTF-16LE").unwrap(), UTF_16LE);
        assert_eq!(parse_encoding("utf16le").unwrap(), UTF_16LE);
        assert_eq!(parse_encoding("utf-16be").unwrap(), UTF_16BE);
        assert_eq!(parse_encoding("utf16be").unwrap(), UTF_16BE);
    }

    #[test]
    fn test_parse_encoding_latin1() {
        assert_eq!(parse_encoding("latin-1").unwrap(), WINDOWS_1252);
        assert_eq!(parse_encoding("latin1").unwrap(), WINDOWS_1252);
        assert_eq!(parse_encoding("iso-8859-1").unwrap(), WINDOWS_1252);
    }

    #[test]
    fn test_parse_encoding_windows1252() {
        assert_eq!(parse_encoding("windows-1252").unwrap(), WINDOWS_1252);
        assert_eq!(parse_encoding("cp1252").unwrap(), WINDOWS_1252);
    }

    #[test]
    fn test_parse_encoding_invalid() {
        assert!(parse_encoding("invalid").is_err());
        assert!(parse_encoding("utf-7").is_err());
    }

    // ------------------------------------------------------------------------
    // NewlineMode Tests
    // ------------------------------------------------------------------------

    #[test]
    fn test_newline_mode_from_str() {
        assert_eq!(NewlineMode::from_str("unix").unwrap(), NewlineMode::Unix);
        assert_eq!(NewlineMode::from_str("lf").unwrap(), NewlineMode::Unix);
        assert_eq!(
            NewlineMode::from_str("windows").unwrap(),
            NewlineMode::Windows
        );
        assert_eq!(NewlineMode::from_str("crlf").unwrap(), NewlineMode::Windows);
        assert_eq!(NewlineMode::from_str("mac").unwrap(), NewlineMode::Mac);
        assert_eq!(NewlineMode::from_str("cr").unwrap(), NewlineMode::Mac);
        assert_eq!(NewlineMode::from_str("keep").unwrap(), NewlineMode::Keep);
    }

    #[test]
    fn test_newline_mode_invalid() {
        assert!(NewlineMode::from_str("invalid").is_err());
    }

    // ------------------------------------------------------------------------
    // BomMode Tests
    // ------------------------------------------------------------------------

    #[test]
    fn test_bom_mode_from_str() {
        assert_eq!(BomMode::from_str("add").unwrap(), BomMode::Add);
        assert_eq!(BomMode::from_str("remove").unwrap(), BomMode::Remove);
        assert_eq!(BomMode::from_str("keep").unwrap(), BomMode::Keep);
        assert_eq!(BomMode::from_str("strip").unwrap(), BomMode::Remove);
    }

    #[test]
    fn test_bom_mode_invalid() {
        assert!(BomMode::from_str("invalid").is_err());
    }

    // ------------------------------------------------------------------------
    // ErrorMode Tests
    // ------------------------------------------------------------------------

    #[test]
    fn test_error_mode_from_str() {
        assert_eq!(ErrorMode::from_str("strict").unwrap(), ErrorMode::Strict);
        assert_eq!(ErrorMode::from_str("replace").unwrap(), ErrorMode::Replace);
        assert_eq!(ErrorMode::from_str("ignore").unwrap(), ErrorMode::Ignore);
    }

    #[test]
    fn test_error_mode_invalid() {
        assert!(ErrorMode::from_str("invalid").is_err());
    }

    // ------------------------------------------------------------------------
    // UTF-8 Identity Conversion Tests
    // ------------------------------------------------------------------------

    #[test]
    fn test_convert_utf8_identity() {
        let config = ConvertConfig::default();
        let cmd = ConvertCommand::new(config);

        let input = b"Hello, World!";
        let output = cmd.convert(input).unwrap();

        assert_eq!(output, input);
    }

    #[test]
    fn test_convert_utf8_with_unicode() {
        let config = ConvertConfig::default();
        let cmd = ConvertCommand::new(config);

        let input = "Hello, 世界! 🌍".as_bytes();
        let output = cmd.convert(input).unwrap();

        assert_eq!(output, input);
    }

    // ------------------------------------------------------------------------
    // UTF-8 to UTF-16 Conversion Tests
    // ------------------------------------------------------------------------

    #[test]
    fn test_convert_utf8_to_utf16le() {
        let config = ConvertConfig {
            from_encoding: UTF_8,
            to_encoding: UTF_16LE,
            ..Default::default()
        };
        let cmd = ConvertCommand::new(config);

        let input = b"AB";
        let output = cmd.convert(input).unwrap();

        // 'A' = 0x41, 'B' = 0x42 in ASCII/UTF-8
        // UTF-16LE: 'A' = 0x41 0x00, 'B' = 0x42 0x00
        assert_eq!(output, &[0x41, 0x00, 0x42, 0x00]);
    }

    #[test]
    fn test_convert_utf8_to_utf16be() {
        let config = ConvertConfig {
            from_encoding: UTF_8,
            to_encoding: UTF_16BE,
            ..Default::default()
        };
        let cmd = ConvertCommand::new(config);

        let input = b"AB";
        let output = cmd.convert(input).unwrap();

        // UTF-16BE: 'A' = 0x00 0x41, 'B' = 0x00 0x42
        assert_eq!(output, &[0x00, 0x41, 0x00, 0x42]);
    }

    // ------------------------------------------------------------------------
    // UTF-16 to UTF-8 Conversion Tests
    // ------------------------------------------------------------------------

    #[test]
    fn test_convert_utf16le_to_utf8() {
        let config = ConvertConfig {
            from_encoding: UTF_16LE,
            to_encoding: UTF_8,
            ..Default::default()
        };
        let cmd = ConvertCommand::new(config);

        // UTF-16LE for "AB"
        let input = &[0x41, 0x00, 0x42, 0x00];
        let output = cmd.convert(input).unwrap();

        assert_eq!(output, b"AB");
    }

    #[test]
    fn test_convert_utf16be_to_utf8() {
        let config = ConvertConfig {
            from_encoding: UTF_16BE,
            to_encoding: UTF_8,
            ..Default::default()
        };
        let cmd = ConvertCommand::new(config);

        // UTF-16BE for "AB"
        let input = &[0x00, 0x41, 0x00, 0x42];
        let output = cmd.convert(input).unwrap();

        assert_eq!(output, b"AB");
    }

    // ------------------------------------------------------------------------
    // Newline Conversion Tests
    // ------------------------------------------------------------------------

    #[test]
    fn test_convert_newlines_to_unix() {
        let config = ConvertConfig {
            newlines: NewlineMode::Unix,
            ..Default::default()
        };
        let cmd = ConvertCommand::new(config);

        // Windows line endings (CRLF)
        let input = b"line1\r\nline2\r\nline3";
        let output = cmd.convert(input).unwrap();
        assert_eq!(output, b"line1\nline2\nline3");

        // Mac line endings (CR)
        let input = b"line1\rline2\rline3";
        let output = cmd.convert(input).unwrap();
        assert_eq!(output, b"line1\nline2\nline3");

        // Mixed line endings
        let input = b"line1\r\nline2\rline3\n";
        let output = cmd.convert(input).unwrap();
        assert_eq!(output, b"line1\nline2\nline3\n");
    }

    #[test]
    fn test_convert_newlines_to_windows() {
        let config = ConvertConfig {
            newlines: NewlineMode::Windows,
            ..Default::default()
        };
        let cmd = ConvertCommand::new(config);

        // Unix line endings (LF)
        let input = b"line1\nline2\nline3";
        let output = cmd.convert(input).unwrap();
        assert_eq!(output, b"line1\r\nline2\r\nline3");

        // Already Windows
        let input = b"line1\r\nline2\r\nline3";
        let output = cmd.convert(input).unwrap();
        assert_eq!(output, b"line1\r\nline2\r\nline3");
    }

    #[test]
    fn test_convert_newlines_to_mac() {
        let config = ConvertConfig {
            newlines: NewlineMode::Mac,
            ..Default::default()
        };
        let cmd = ConvertCommand::new(config);

        // Unix line endings
        let input = b"line1\nline2\nline3";
        let output = cmd.convert(input).unwrap();
        assert_eq!(output, b"line1\rline2\rline3");
    }

    #[test]
    fn test_convert_newlines_keep() {
        let config = ConvertConfig {
            newlines: NewlineMode::Keep,
            ..Default::default()
        };
        let cmd = ConvertCommand::new(config);

        // Mixed line endings should be preserved
        let input = b"line1\r\nline2\nline3\r";
        let output = cmd.convert(input).unwrap();
        assert_eq!(output, input.as_ref());
    }

    // ------------------------------------------------------------------------
    // BOM Handling Tests
    // ------------------------------------------------------------------------

    #[test]
    fn test_bom_add_utf8() {
        let config = ConvertConfig {
            bom: BomMode::Add,
            ..Default::default()
        };
        let cmd = ConvertCommand::new(config);

        let input = b"Hello";
        let output = cmd.convert(input).unwrap();

        assert!(output.starts_with(UTF8_BOM));
        assert_eq!(&output[UTF8_BOM.len()..], b"Hello");
    }

    #[test]
    fn test_bom_add_utf16le() {
        let config = ConvertConfig {
            to_encoding: UTF_16LE,
            bom: BomMode::Add,
            ..Default::default()
        };
        let cmd = ConvertCommand::new(config);

        let input = b"A";
        let output = cmd.convert(input).unwrap();

        assert!(output.starts_with(UTF16_LE_BOM));
    }

    #[test]
    fn test_bom_remove() {
        let config = ConvertConfig {
            bom: BomMode::Remove,
            ..Default::default()
        };
        let cmd = ConvertCommand::new(config);

        // Input with UTF-8 BOM
        let mut input = UTF8_BOM.to_vec();
        input.extend_from_slice(b"Hello");

        let output = cmd.convert(&input).unwrap();

        assert!(!output.starts_with(UTF8_BOM));
        assert_eq!(output, b"Hello");
    }

    #[test]
    fn test_bom_keep_with_bom() {
        let config = ConvertConfig {
            bom: BomMode::Keep,
            ..Default::default()
        };
        let cmd = ConvertCommand::new(config);

        // Input with UTF-8 BOM
        let mut input = UTF8_BOM.to_vec();
        input.extend_from_slice(b"Hello");

        let output = cmd.convert(&input).unwrap();

        assert!(output.starts_with(UTF8_BOM));
        assert_eq!(&output[UTF8_BOM.len()..], b"Hello");
    }

    #[test]
    fn test_bom_keep_without_bom() {
        let config = ConvertConfig {
            bom: BomMode::Keep,
            ..Default::default()
        };
        let cmd = ConvertCommand::new(config);

        let input = b"Hello";
        let output = cmd.convert(input).unwrap();

        assert!(!output.starts_with(UTF8_BOM));
        assert_eq!(output, b"Hello");
    }

    // ------------------------------------------------------------------------
    // Error Mode Tests
    // ------------------------------------------------------------------------

    #[test]
    fn test_error_mode_replace() {
        let config = ConvertConfig {
            from_encoding: UTF_8,
            on_error: ErrorMode::Replace,
            ..Default::default()
        };
        let cmd = ConvertCommand::new(config);

        // Invalid UTF-8 sequence (0xFF is not valid in UTF-8)
        let input = &[0x48, 0x65, 0x6C, 0xFF, 0x6F]; // "Hel\xFFo"
        let output = cmd.convert(input).unwrap();

        // Should contain the replacement character
        let output_str = String::from_utf8(output).unwrap();
        assert!(output_str.contains('\u{FFFD}'));
    }

    #[test]
    fn test_error_mode_strict() {
        let config = ConvertConfig {
            from_encoding: UTF_8,
            on_error: ErrorMode::Strict,
            ..Default::default()
        };
        let cmd = ConvertCommand::new(config);

        // Invalid UTF-8 sequence
        let input = &[0x48, 0x65, 0x6C, 0xFF, 0x6F]; // "Hel\xFFo"
        let result = cmd.convert(input);

        assert!(result.is_err());
    }

    #[test]
    fn test_error_mode_ignore() {
        let config = ConvertConfig {
            from_encoding: UTF_8,
            on_error: ErrorMode::Ignore,
            ..Default::default()
        };
        let cmd = ConvertCommand::new(config);

        // Invalid UTF-8 sequence
        let input = &[0x48, 0x65, 0x6C, 0xFF, 0x6F]; // "Hel\xFFo"
        let output = cmd.convert(input).unwrap();

        // Should NOT contain replacement character (filtered out)
        let output_str = String::from_utf8(output).unwrap();
        assert!(!output_str.contains('\u{FFFD}'));
        // The invalid byte is replaced by decoder, then we filter out FFFD
        assert!(output_str.contains("Hel"));
        assert!(output_str.contains('o'));
    }

    // ------------------------------------------------------------------------
    // Combined Conversion Tests
    // ------------------------------------------------------------------------

    #[test]
    fn test_utf16le_with_bom_to_utf8_unix() {
        let config = ConvertConfig {
            from_encoding: UTF_16LE,
            to_encoding: UTF_8,
            newlines: NewlineMode::Unix,
            bom: BomMode::Remove,
            ..Default::default()
        };
        let cmd = ConvertCommand::new(config);

        // UTF-16LE BOM + "A\r\n"
        let input = &[
            0xFF, 0xFE, // BOM
            0x41, 0x00, // 'A'
            0x0D, 0x00, // '\r'
            0x0A, 0x00, // '\n'
        ];
        let output = cmd.convert(input).unwrap();

        assert_eq!(output, b"A\n");
    }

    // ------------------------------------------------------------------------
    // BOM Detection Tests
    // ------------------------------------------------------------------------

    #[test]
    fn test_detect_bom_encoding_utf8() {
        let mut data = UTF8_BOM.to_vec();
        data.extend_from_slice(b"Hello");
        assert_eq!(detect_bom_encoding(&data), Some(UTF_8));
    }

    #[test]
    fn test_detect_bom_encoding_utf16le() {
        let mut data = UTF16_LE_BOM.to_vec();
        data.extend_from_slice(&[0x41, 0x00]);
        assert_eq!(detect_bom_encoding(&data), Some(UTF_16LE));
    }

    #[test]
    fn test_detect_bom_encoding_utf16be() {
        let mut data = UTF16_BE_BOM.to_vec();
        data.extend_from_slice(&[0x00, 0x41]);
        assert_eq!(detect_bom_encoding(&data), Some(UTF_16BE));
    }

    #[test]
    fn test_detect_bom_encoding_none() {
        let data = b"Hello";
        assert_eq!(detect_bom_encoding(data), None);
    }

    // ------------------------------------------------------------------------
    // Edge Cases
    // ------------------------------------------------------------------------

    #[test]
    fn test_empty_input() {
        let config = ConvertConfig::default();
        let cmd = ConvertCommand::new(config);

        let input = b"";
        let output = cmd.convert(input).unwrap();
        assert!(output.is_empty());
    }

    #[test]
    fn test_empty_input_with_bom_add() {
        let config = ConvertConfig {
            bom: BomMode::Add,
            ..Default::default()
        };
        let cmd = ConvertCommand::new(config);

        let input = b"";
        let output = cmd.convert(input).unwrap();
        assert_eq!(output, UTF8_BOM);
    }

    #[test]
    fn test_describe() {
        let config = ConvertConfig {
            from_encoding: UTF_16LE,
            to_encoding: UTF_8,
            newlines: NewlineMode::Unix,
            bom: BomMode::Remove,
            on_error: ErrorMode::Strict,
        };
        let cmd = ConvertCommand::new(config);

        let desc = cmd.describe();
        assert!(desc.contains("UTF-16LE"));
        assert!(desc.contains("UTF-8"));
        assert!(desc.contains("Unix"));
        assert!(desc.contains("Remove"));
        assert!(desc.contains("Strict"));
    }

    // ------------------------------------------------------------------------
    // Latin-1 / Windows-1252 Tests
    // ------------------------------------------------------------------------

    #[test]
    fn test_convert_windows1252_to_utf8() {
        let config = ConvertConfig {
            from_encoding: WINDOWS_1252,
            to_encoding: UTF_8,
            ..Default::default()
        };
        let cmd = ConvertCommand::new(config);

        // Windows-1252 specific characters:
        // 0x80 = € (Euro sign), 0x92 = ' (right single quote)
        let input = &[0x48, 0x65, 0x6C, 0x6C, 0x6F, 0x80]; // "Hello€"
        let output = cmd.convert(input).unwrap();

        let output_str = String::from_utf8(output).unwrap();
        assert_eq!(output_str, "Hello€");
    }

    // ------------------------------------------------------------------------
    // Large Data Tests
    // ------------------------------------------------------------------------

    #[test]
    fn test_large_conversion() {
        let config = ConvertConfig {
            newlines: NewlineMode::Unix,
            ..Default::default()
        };
        let cmd = ConvertCommand::new(config);

        // Create a large input with Windows line endings
        let line = "This is a test line with some content.\r\n";
        let input: String = line.repeat(10000);

        let output = cmd.convert(input.as_bytes()).unwrap();

        // Verify all CRLF were converted to LF
        assert!(!output.windows(2).any(|w| w == b"\r\n"));
        // Verify we have the expected number of LF
        let lf_count = output.iter().filter(|&&b| b == b'\n').count();
        assert_eq!(lf_count, 10000);
    }
}
