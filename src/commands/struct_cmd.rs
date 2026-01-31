//! Structure parsing command for interpreting binary data using templates.
//!
//! This module provides functionality to parse binary data according to
//! predefined structural templates in YAML format.
//!
//! # Template Format
//!
//! Templates are defined in YAML with the following structure:
//! ```yaml
//! name: MyStructure
//! endian: little  # or 'big'
//! fields:
//!   - name: magic
//!     offset: 0x00
//!     size: 4
//!     type: hex_string
//!     assert: "7f454c46"
//!   - name: version
//!     offset: 0x04
//!     size: 2
//!     type: u16
//!     enum:
//!       1: v1.0
//!       2: v2.0
//! ```
//!
//! # Supported Types
//!
//! - `u8`, `u16`, `u32`, `u64` - Unsigned integers
//! - `i8`, `i16`, `i32`, `i64` - Signed integers
//! - `hex_string` - Raw bytes displayed as hex
//! - `string` - ASCII/UTF-8 string (null-terminated or fixed length)
//! - `bytes` - Raw byte array
//!
//! # Examples
//!
//! ```bash
//! # Parse a binary file using a template
//! binfiddle struct elf_header.yaml -i /bin/ls
//!
//! # Get a specific field value
//! binfiddle struct elf_header.yaml -i /bin/ls --get e_entry
//!
//! # Output as JSON
//! binfiddle struct elf_header.yaml -i /bin/ls --format json
//!
//! # List all fields in template
//! binfiddle struct elf_header.yaml --list-fields
//! ```

use crate::error::{BinfiddleError, Result};
use byteorder::{BigEndian, ByteOrder, LittleEndian};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Endianness for multi-byte field interpretation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Endianness {
    /// Little-endian byte order (least significant byte first)
    #[default]
    Little,
    /// Big-endian byte order (most significant byte first)
    Big,
}

impl Endianness {
    /// Parses an endianness string.
    pub fn from_str(s: &str) -> Result<Self> {
        match s.to_lowercase().as_str() {
            "little" | "le" | "little-endian" | "littleendian" => Ok(Endianness::Little),
            "big" | "be" | "big-endian" | "bigendian" => Ok(Endianness::Big),
            _ => Err(BinfiddleError::Parse(format!(
                "Invalid endianness '{}': expected 'little' or 'big'",
                s
            ))),
        }
    }
}

/// Output format for struct command results.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StructOutputFormat {
    /// Human-readable table format
    #[default]
    Human,
    /// JSON format
    Json,
    /// YAML format
    Yaml,
}

impl StructOutputFormat {
    /// Parses an output format string.
    pub fn from_str(s: &str) -> Result<Self> {
        match s.to_lowercase().as_str() {
            "human" | "table" | "text" => Ok(StructOutputFormat::Human),
            "json" => Ok(StructOutputFormat::Json),
            "yaml" | "yml" => Ok(StructOutputFormat::Yaml),
            _ => Err(BinfiddleError::Parse(format!(
                "Invalid output format '{}': expected 'human', 'json', or 'yaml'",
                s
            ))),
        }
    }
}

/// Field data type for template fields.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FieldType {
    /// Unsigned 8-bit integer
    U8,
    /// Unsigned 16-bit integer
    U16,
    /// Unsigned 32-bit integer
    U32,
    /// Unsigned 64-bit integer
    U64,
    /// Signed 8-bit integer
    I8,
    /// Signed 16-bit integer
    I16,
    /// Signed 32-bit integer
    I32,
    /// Signed 64-bit integer
    I64,
    /// Hexadecimal string representation of bytes
    HexString,
    /// ASCII/UTF-8 string (null-terminated or fixed length)
    String,
    /// Raw byte array
    Bytes,
}

impl Default for FieldType {
    fn default() -> Self {
        FieldType::Bytes
    }
}

impl FieldType {
    /// Returns the expected size for this field type, if fixed.
    pub fn expected_size(&self) -> Option<usize> {
        match self {
            FieldType::U8 | FieldType::I8 => Some(1),
            FieldType::U16 | FieldType::I16 => Some(2),
            FieldType::U32 | FieldType::I32 => Some(4),
            FieldType::U64 | FieldType::I64 => Some(8),
            _ => None, // Variable size types
        }
    }

    /// Parses a field type from string (for manual parsing if needed).
    pub fn from_str(s: &str) -> Result<Self> {
        match s.to_lowercase().as_str() {
            "u8" | "uint8" | "byte" => Ok(FieldType::U8),
            "u16" | "uint16" | "word" | "ushort" => Ok(FieldType::U16),
            "u32" | "uint32" | "dword" | "uint" => Ok(FieldType::U32),
            "u64" | "uint64" | "qword" | "ulong" => Ok(FieldType::U64),
            "i8" | "int8" | "sbyte" => Ok(FieldType::I8),
            "i16" | "int16" | "short" => Ok(FieldType::I16),
            "i32" | "int32" | "int" => Ok(FieldType::I32),
            "i64" | "int64" | "long" => Ok(FieldType::I64),
            "hex_string" | "hexstring" | "hex" => Ok(FieldType::HexString),
            "string" | "str" | "ascii" | "utf8" => Ok(FieldType::String),
            "bytes" | "raw" | "data" => Ok(FieldType::Bytes),
            _ => Err(BinfiddleError::Parse(format!(
                "Invalid field type '{}': expected u8, u16, u32, u64, i8, i16, i32, i64, \
                 hex_string, string, or bytes",
                s
            ))),
        }
    }
}

/// Represents a single field definition in a template.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FieldDefinition {
    /// Field name (identifier)
    pub name: String,

    /// Byte offset from start of structure
    #[serde(deserialize_with = "deserialize_offset")]
    pub offset: usize,

    /// Size in bytes
    pub size: usize,

    /// Data type for interpretation
    #[serde(rename = "type", default)]
    pub field_type: FieldType,

    /// Optional assertion value (hex string without prefix)
    #[serde(default)]
    pub assert: Option<String>,

    /// Optional enum mapping for integer values
    #[serde(default)]
    pub r#enum: Option<HashMap<String, String>>,

    /// Optional description/comment
    #[serde(default)]
    pub description: Option<String>,

    /// Optional display format override (hex, dec, bin, oct)
    #[serde(default)]
    pub display: Option<String>,
}

/// Custom deserializer for offset that handles hex strings.
fn deserialize_offset<'de, D>(deserializer: D) -> std::result::Result<usize, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::Error;

    #[derive(Deserialize)]
    #[serde(untagged)]
    enum OffsetValue {
        Number(usize),
        String(String),
    }

    match OffsetValue::deserialize(deserializer)? {
        OffsetValue::Number(n) => Ok(n),
        OffsetValue::String(s) => {
            let s = s.trim();
            if s.starts_with("0x") || s.starts_with("0X") {
                usize::from_str_radix(&s[2..], 16).map_err(|e| D::Error::custom(e.to_string()))
            } else {
                s.parse::<usize>().map_err(|e| D::Error::custom(e.to_string()))
            }
        }
    }
}

/// Represents a complete structure template.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct StructTemplate {
    /// Template/structure name
    pub name: String,

    /// Byte order for multi-byte fields
    #[serde(default)]
    pub endian: Endianness,

    /// List of field definitions
    pub fields: Vec<FieldDefinition>,

    /// Optional description of the structure
    #[serde(default)]
    pub description: Option<String>,
}

impl StructTemplate {
    /// Loads a template from YAML string.
    pub fn from_yaml(yaml: &str) -> Result<Self> {
        serde_yaml::from_str(yaml).map_err(|e| {
            BinfiddleError::Parse(format!("Failed to parse template YAML: {}", e))
        })
    }

    /// Loads a template from a file path.
    pub fn from_file(path: &str) -> Result<Self> {
        let content = std::fs::read_to_string(path).map_err(|e| {
            BinfiddleError::Io(std::io::Error::new(
                e.kind(),
                format!("Failed to read template file '{}': {}", path, e),
            ))
        })?;
        Self::from_yaml(&content)
    }

    /// Returns the total size covered by this template.
    pub fn total_size(&self) -> usize {
        self.fields
            .iter()
            .map(|f| f.offset + f.size)
            .max()
            .unwrap_or(0)
    }

    /// Gets a field definition by name.
    pub fn get_field(&self, name: &str) -> Option<&FieldDefinition> {
        self.fields.iter().find(|f| f.name == name)
    }

    /// Validates the template for consistency.
    pub fn validate(&self) -> Result<()> {
        // Check for duplicate field names
        let mut seen_names = std::collections::HashSet::new();
        for field in &self.fields {
            if !seen_names.insert(&field.name) {
                return Err(BinfiddleError::Parse(format!(
                    "Duplicate field name '{}' in template",
                    field.name
                )));
            }
        }

        // Validate field type sizes
        for field in &self.fields {
            if let Some(expected) = field.field_type.expected_size() {
                if field.size != expected {
                    return Err(BinfiddleError::Parse(format!(
                        "Field '{}' has type {:?} which requires {} bytes, but size is {}",
                        field.name, field.field_type, expected, field.size
                    )));
                }
            }
        }

        Ok(())
    }
}

/// Represents a parsed field value.
#[derive(Debug, Clone, Serialize)]
pub struct ParsedField {
    /// Field name
    pub name: String,
    /// Offset in data
    pub offset: usize,
    /// Size in bytes
    pub size: usize,
    /// Raw bytes
    #[serde(skip)]
    pub raw_bytes: Vec<u8>,
    /// Interpreted value as string
    pub value: String,
    /// Numeric value (if applicable)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub numeric_value: Option<i128>,
    /// Enum name (if matched)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enum_name: Option<String>,
    /// Assertion passed
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assertion_passed: Option<bool>,
    /// Description (if any)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// Result of parsing a structure.
#[derive(Debug, Clone, Serialize)]
pub struct ParsedStruct {
    /// Template name
    pub name: String,
    /// Template description
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Parsed fields
    pub fields: Vec<ParsedField>,
    /// Whether all assertions passed
    pub all_assertions_passed: bool,
}

/// Configuration for struct parsing.
#[derive(Debug, Clone, Default)]
pub struct StructConfig {
    /// Output format
    pub format: StructOutputFormat,
    /// Only get specific field(s)
    pub get_fields: Vec<String>,
    /// List fields only (don't parse data)
    pub list_fields: bool,
}

/// The struct command implementation.
#[derive(Debug)]
pub struct StructCommand {
    config: StructConfig,
}

impl StructCommand {
    /// Creates a new StructCommand with the given configuration.
    pub fn new(config: StructConfig) -> Self {
        Self { config }
    }

    /// Parses binary data according to a template.
    ///
    /// # Arguments
    /// * `data` - The binary data to parse
    /// * `template` - The structure template to apply
    ///
    /// # Returns
    /// A ParsedStruct containing all field values.
    pub fn parse(&self, data: &[u8], template: &StructTemplate) -> Result<ParsedStruct> {
        // Validate template first
        template.validate()?;

        let mut fields = Vec::new();
        let mut all_assertions_passed = true;

        for field_def in &template.fields {
            // Check if we should skip this field (if get_fields is specified)
            if !self.config.get_fields.is_empty()
                && !self.config.get_fields.contains(&field_def.name)
            {
                continue;
            }

            let parsed = self.parse_field(data, field_def, template.endian)?;

            // Check assertion
            if let Some(passed) = parsed.assertion_passed {
                if !passed {
                    all_assertions_passed = false;
                }
            }

            fields.push(parsed);
        }

        Ok(ParsedStruct {
            name: template.name.clone(),
            description: template.description.clone(),
            fields,
            all_assertions_passed,
        })
    }

    /// Parses a single field from binary data.
    fn parse_field(
        &self,
        data: &[u8],
        field: &FieldDefinition,
        endian: Endianness,
    ) -> Result<ParsedField> {
        // Bounds check
        if field.offset + field.size > data.len() {
            return Err(BinfiddleError::InvalidRange(format!(
                "Field '{}' at offset 0x{:x} with size {} exceeds data length {}",
                field.name,
                field.offset,
                field.size,
                data.len()
            )));
        }

        let raw_bytes = data[field.offset..field.offset + field.size].to_vec();

        // Parse based on type
        let (value, numeric_value) = self.interpret_field(&raw_bytes, &field.field_type, endian)?;

        // Check enum mapping
        let enum_name = if let (Some(ref enum_map), Some(num)) = (&field.r#enum, numeric_value) {
            enum_map.get(&num.to_string()).cloned()
        } else {
            None
        };

        // Check assertion
        let assertion_passed = field.assert.as_ref().map(|expected| {
            let expected_bytes = self.parse_assert_value(expected);
            expected_bytes.as_ref() == Some(&raw_bytes)
        });

        // Format the display value
        let display_value = if let Some(ref enum_name) = enum_name {
            format!("{} ({})", value, enum_name)
        } else {
            value
        };

        Ok(ParsedField {
            name: field.name.clone(),
            offset: field.offset,
            size: field.size,
            raw_bytes,
            value: display_value,
            numeric_value,
            enum_name,
            assertion_passed,
            description: field.description.clone(),
        })
    }

    /// Interprets raw bytes according to field type.
    fn interpret_field(
        &self,
        bytes: &[u8],
        field_type: &FieldType,
        endian: Endianness,
    ) -> Result<(String, Option<i128>)> {
        match field_type {
            FieldType::U8 => {
                let val = bytes[0] as u64;
                Ok((format!("{}", val), Some(val as i128)))
            }
            FieldType::I8 => {
                let val = bytes[0] as i8;
                Ok((format!("{}", val), Some(val as i128)))
            }
            FieldType::U16 => {
                let val = match endian {
                    Endianness::Little => LittleEndian::read_u16(bytes),
                    Endianness::Big => BigEndian::read_u16(bytes),
                };
                Ok((format!("{}", val), Some(val as i128)))
            }
            FieldType::I16 => {
                let val = match endian {
                    Endianness::Little => LittleEndian::read_i16(bytes),
                    Endianness::Big => BigEndian::read_i16(bytes),
                };
                Ok((format!("{}", val), Some(val as i128)))
            }
            FieldType::U32 => {
                let val = match endian {
                    Endianness::Little => LittleEndian::read_u32(bytes),
                    Endianness::Big => BigEndian::read_u32(bytes),
                };
                Ok((format!("{}", val), Some(val as i128)))
            }
            FieldType::I32 => {
                let val = match endian {
                    Endianness::Little => LittleEndian::read_i32(bytes),
                    Endianness::Big => BigEndian::read_i32(bytes),
                };
                Ok((format!("{}", val), Some(val as i128)))
            }
            FieldType::U64 => {
                let val = match endian {
                    Endianness::Little => LittleEndian::read_u64(bytes),
                    Endianness::Big => BigEndian::read_u64(bytes),
                };
                Ok((format!("{}", val), Some(val as i128)))
            }
            FieldType::I64 => {
                let val = match endian {
                    Endianness::Little => LittleEndian::read_i64(bytes),
                    Endianness::Big => BigEndian::read_i64(bytes),
                };
                Ok((format!("{}", val), Some(val as i128)))
            }
            FieldType::HexString => {
                let hex_str = bytes
                    .iter()
                    .map(|b| format!("{:02x}", b))
                    .collect::<Vec<_>>()
                    .join(" ");
                Ok((hex_str, None))
            }
            FieldType::String => {
                // Find null terminator or use full length
                let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
                let s = String::from_utf8_lossy(&bytes[..end]).to_string();
                Ok((format!("\"{}\"", s), None))
            }
            FieldType::Bytes => {
                let hex_str = bytes
                    .iter()
                    .map(|b| format!("{:02x}", b))
                    .collect::<Vec<_>>()
                    .join(" ");
                Ok((hex_str, None))
            }
        }
    }

    /// Parses an assertion value (hex string) to bytes.
    fn parse_assert_value(&self, value: &str) -> Option<Vec<u8>> {
        let value = value.trim();
        // Remove 0x prefix if present
        let hex_str = if value.starts_with("0x") || value.starts_with("0X") {
            &value[2..]
        } else {
            value
        };
        // Remove spaces
        let hex_str: String = hex_str.chars().filter(|c| !c.is_whitespace()).collect();
        hex::decode(&hex_str).ok()
    }

    /// Formats the parsed structure for output.
    pub fn format_output(&self, parsed: &ParsedStruct) -> Result<String> {
        match self.config.format {
            StructOutputFormat::Human => Ok(self.format_human(parsed)),
            StructOutputFormat::Json => self.format_json(parsed),
            StructOutputFormat::Yaml => self.format_yaml(parsed),
        }
    }

    /// Formats output in human-readable table format.
    fn format_human(&self, parsed: &ParsedStruct) -> String {
        let mut output = String::new();

        // Header
        output.push_str(&format!("Structure: {}\n", parsed.name));
        if let Some(ref desc) = parsed.description {
            output.push_str(&format!("Description: {}\n", desc));
        }
        output.push_str(&format!(
            "Assertions: {}\n",
            if parsed.all_assertions_passed {
                "✓ All passed"
            } else {
                "✗ Some failed"
            }
        ));
        output.push('\n');

        // Calculate column widths
        let name_width = parsed
            .fields
            .iter()
            .map(|f| f.name.len())
            .max()
            .unwrap_or(4)
            .max(4);
        let offset_width = 10; // "0x00000000"
        let size_width = 4;
        let value_width = parsed
            .fields
            .iter()
            .map(|f| f.value.len())
            .max()
            .unwrap_or(5)
            .max(5);

        // Table header
        output.push_str(&format!(
            "{:<name_width$}  {:>offset_width$}  {:>size_width$}  {:<value_width$}  Status\n",
            "Name",
            "Offset",
            "Size",
            "Value",
            name_width = name_width,
            offset_width = offset_width,
            size_width = size_width,
            value_width = value_width
        ));
        output.push_str(&format!(
            "{:-<name_width$}  {:-<offset_width$}  {:-<size_width$}  {:-<value_width$}  ------\n",
            "",
            "",
            "",
            "",
            name_width = name_width,
            offset_width = offset_width,
            size_width = size_width,
            value_width = value_width
        ));

        // Table rows
        for field in &parsed.fields {
            let status = match field.assertion_passed {
                Some(true) => "✓",
                Some(false) => "✗ FAIL",
                None => "",
            };

            output.push_str(&format!(
                "{:<name_width$}  0x{:08x}  {:>size_width$}  {:<value_width$}  {}\n",
                field.name,
                field.offset,
                field.size,
                field.value,
                status,
                name_width = name_width,
                size_width = size_width,
                value_width = value_width
            ));
        }

        output
    }

    /// Formats output as JSON.
    fn format_json(&self, parsed: &ParsedStruct) -> Result<String> {
        serde_json::to_string_pretty(parsed)
            .map_err(|e| BinfiddleError::Parse(format!("Failed to serialize to JSON: {}", e)))
    }

    /// Formats output as YAML.
    fn format_yaml(&self, parsed: &ParsedStruct) -> Result<String> {
        serde_yaml::to_string(parsed)
            .map_err(|e| BinfiddleError::Parse(format!("Failed to serialize to YAML: {}", e)))
    }

    /// Lists all fields in a template (without parsing data).
    pub fn list_fields(&self, template: &StructTemplate) -> String {
        let mut output = String::new();

        output.push_str(&format!("Template: {}\n", template.name));
        if let Some(ref desc) = template.description {
            output.push_str(&format!("Description: {}\n", desc));
        }
        output.push_str(&format!("Endianness: {:?}\n", template.endian));
        output.push_str(&format!("Total size: {} bytes\n", template.total_size()));
        output.push_str(&format!("Fields: {}\n\n", template.fields.len()));

        // Calculate column widths
        let name_width = template
            .fields
            .iter()
            .map(|f| f.name.len())
            .max()
            .unwrap_or(4)
            .max(4);
        let type_width = 10;

        // Header
        output.push_str(&format!(
            "{:<name_width$}  {:>10}  {:>4}  {:<type_width$}  Description\n",
            "Name",
            "Offset",
            "Size",
            "Type",
            name_width = name_width,
            type_width = type_width
        ));
        output.push_str(&format!(
            "{:-<name_width$}  {:-<10}  {:-<4}  {:-<type_width$}  -----------\n",
            "",
            "",
            "",
            "",
            name_width = name_width,
            type_width = type_width
        ));

        // Fields
        for field in &template.fields {
            let type_str = format!("{:?}", field.field_type);
            let desc = field.description.as_deref().unwrap_or("-");
            output.push_str(&format!(
                "{:<name_width$}  0x{:08x}  {:>4}  {:<type_width$}  {}\n",
                field.name,
                field.offset,
                field.size,
                type_str,
                desc,
                name_width = name_width,
                type_width = type_width
            ));
        }

        output
    }

    /// Gets a single field value as a simple string (for --get mode).
    pub fn get_field_value(&self, parsed: &ParsedStruct, field_name: &str) -> Option<String> {
        parsed
            .fields
            .iter()
            .find(|f| f.name == field_name)
            .map(|f| f.value.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ELF_TEMPLATE_YAML: &str = r#"
name: ELF Header
description: ELF file header structure
endian: little
fields:
  - name: e_ident_magic
    offset: 0x00
    size: 4
    type: hex_string
    assert: "7f454c46"
    description: "ELF magic number"
  - name: e_ident_class
    offset: 0x04
    size: 1
    type: u8
    description: "32/64-bit flag"
    enum:
      "1": "32-bit"
      "2": "64-bit"
  - name: e_ident_data
    offset: 0x05
    size: 1
    type: u8
    description: "Endianness"
    enum:
      "1": "Little Endian"
      "2": "Big Endian"
  - name: e_type
    offset: 0x10
    size: 2
    type: u16
    description: "Object file type"
    enum:
      "1": "Relocatable"
      "2": "Executable"
      "3": "Shared object"
      "4": "Core file"
"#;

    // Mock ELF header (64-bit little endian executable)
    fn mock_elf_header() -> Vec<u8> {
        let mut data = vec![0u8; 64];
        // Magic: 0x7f 'E' 'L' 'F'
        data[0] = 0x7f;
        data[1] = 0x45;
        data[2] = 0x4c;
        data[3] = 0x46;
        // Class: 64-bit (2)
        data[4] = 0x02;
        // Data: Little endian (1)
        data[5] = 0x01;
        // e_type at offset 0x10: Executable (2) in little endian
        data[0x10] = 0x02;
        data[0x11] = 0x00;
        data
    }

    #[test]
    fn test_parse_template_yaml() {
        let template = StructTemplate::from_yaml(ELF_TEMPLATE_YAML).unwrap();
        assert_eq!(template.name, "ELF Header");
        assert_eq!(template.endian, Endianness::Little);
        assert_eq!(template.fields.len(), 4);
        assert_eq!(template.fields[0].name, "e_ident_magic");
        assert_eq!(template.fields[0].offset, 0);
        assert_eq!(template.fields[0].size, 4);
    }

    #[test]
    fn test_parse_template_with_hex_offset() {
        let yaml = r#"
name: Test
fields:
  - name: field1
    offset: 0x100
    size: 4
    type: u32
"#;
        let template = StructTemplate::from_yaml(yaml).unwrap();
        assert_eq!(template.fields[0].offset, 0x100);
    }

    #[test]
    fn test_validate_template_duplicate_names() {
        let yaml = r#"
name: Test
fields:
  - name: duplicate
    offset: 0
    size: 4
    type: u32
  - name: duplicate
    offset: 4
    size: 4
    type: u32
"#;
        let template = StructTemplate::from_yaml(yaml).unwrap();
        let result = template.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Duplicate"));
    }

    #[test]
    fn test_validate_template_size_mismatch() {
        let yaml = r#"
name: Test
fields:
  - name: bad_field
    offset: 0
    size: 2
    type: u32
"#;
        let template = StructTemplate::from_yaml(yaml).unwrap();
        let result = template.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("requires 4 bytes"));
    }

    #[test]
    fn test_parse_elf_header() {
        let template = StructTemplate::from_yaml(ELF_TEMPLATE_YAML).unwrap();
        let data = mock_elf_header();

        let config = StructConfig::default();
        let cmd = StructCommand::new(config);
        let result = cmd.parse(&data, &template).unwrap();

        assert_eq!(result.name, "ELF Header");
        assert!(result.all_assertions_passed);
        assert_eq!(result.fields.len(), 4);

        // Check magic number
        let magic = &result.fields[0];
        assert_eq!(magic.name, "e_ident_magic");
        assert_eq!(magic.value, "7f 45 4c 46");
        assert_eq!(magic.assertion_passed, Some(true));

        // Check class (64-bit)
        let class = &result.fields[1];
        assert_eq!(class.name, "e_ident_class");
        assert_eq!(class.numeric_value, Some(2));
        assert_eq!(class.enum_name, Some("64-bit".to_string()));

        // Check e_type (Executable)
        let etype = &result.fields[3];
        assert_eq!(etype.name, "e_type");
        assert_eq!(etype.numeric_value, Some(2));
        assert_eq!(etype.enum_name, Some("Executable".to_string()));
    }

    #[test]
    fn test_parse_with_failed_assertion() {
        let yaml = r#"
name: Test
fields:
  - name: magic
    offset: 0
    size: 4
    type: hex_string
    assert: "deadbeef"
"#;
        let template = StructTemplate::from_yaml(yaml).unwrap();
        let data = vec![0x00, 0x11, 0x22, 0x33];

        let config = StructConfig::default();
        let cmd = StructCommand::new(config);
        let result = cmd.parse(&data, &template).unwrap();

        assert!(!result.all_assertions_passed);
        assert_eq!(result.fields[0].assertion_passed, Some(false));
    }

    #[test]
    fn test_get_specific_field() {
        let template = StructTemplate::from_yaml(ELF_TEMPLATE_YAML).unwrap();
        let data = mock_elf_header();

        let config = StructConfig {
            get_fields: vec!["e_type".to_string()],
            ..Default::default()
        };
        let cmd = StructCommand::new(config);
        let result = cmd.parse(&data, &template).unwrap();

        assert_eq!(result.fields.len(), 1);
        assert_eq!(result.fields[0].name, "e_type");
    }

    #[test]
    fn test_format_human_output() {
        let template = StructTemplate::from_yaml(ELF_TEMPLATE_YAML).unwrap();
        let data = mock_elf_header();

        let config = StructConfig::default();
        let cmd = StructCommand::new(config);
        let parsed = cmd.parse(&data, &template).unwrap();
        let output = cmd.format_output(&parsed).unwrap();

        assert!(output.contains("Structure: ELF Header"));
        assert!(output.contains("e_ident_magic"));
        assert!(output.contains("7f 45 4c 46"));
        assert!(output.contains("✓"));
    }

    #[test]
    fn test_format_json_output() {
        let template = StructTemplate::from_yaml(ELF_TEMPLATE_YAML).unwrap();
        let data = mock_elf_header();

        let config = StructConfig {
            format: StructOutputFormat::Json,
            ..Default::default()
        };
        let cmd = StructCommand::new(config);
        let parsed = cmd.parse(&data, &template).unwrap();
        let output = cmd.format_output(&parsed).unwrap();

        assert!(output.contains("\"name\": \"ELF Header\""));
        assert!(output.contains("\"e_ident_magic\""));
    }

    #[test]
    fn test_format_yaml_output() {
        let template = StructTemplate::from_yaml(ELF_TEMPLATE_YAML).unwrap();
        let data = mock_elf_header();

        let config = StructConfig {
            format: StructOutputFormat::Yaml,
            ..Default::default()
        };
        let cmd = StructCommand::new(config);
        let parsed = cmd.parse(&data, &template).unwrap();
        let output = cmd.format_output(&parsed).unwrap();

        assert!(output.contains("name: ELF Header"));
        assert!(output.contains("e_ident_magic"));
    }

    #[test]
    fn test_list_fields() {
        let template = StructTemplate::from_yaml(ELF_TEMPLATE_YAML).unwrap();

        let config = StructConfig::default();
        let cmd = StructCommand::new(config);
        let output = cmd.list_fields(&template);

        assert!(output.contains("Template: ELF Header"));
        assert!(output.contains("Fields: 4"));
        assert!(output.contains("e_ident_magic"));
        assert!(output.contains("HexString"));
    }

    #[test]
    fn test_big_endian_parsing() {
        let yaml = r#"
name: Big Endian Test
endian: big
fields:
  - name: value
    offset: 0
    size: 4
    type: u32
"#;
        let template = StructTemplate::from_yaml(yaml).unwrap();
        // 0xDEADBEEF in big endian
        let data = vec![0xDE, 0xAD, 0xBE, 0xEF];

        let config = StructConfig::default();
        let cmd = StructCommand::new(config);
        let result = cmd.parse(&data, &template).unwrap();

        assert_eq!(result.fields[0].numeric_value, Some(0xDEADBEEF));
    }

    #[test]
    fn test_little_endian_parsing() {
        let yaml = r#"
name: Little Endian Test
endian: little
fields:
  - name: value
    offset: 0
    size: 4
    type: u32
"#;
        let template = StructTemplate::from_yaml(yaml).unwrap();
        // 0xDEADBEEF in little endian
        let data = vec![0xEF, 0xBE, 0xAD, 0xDE];

        let config = StructConfig::default();
        let cmd = StructCommand::new(config);
        let result = cmd.parse(&data, &template).unwrap();

        assert_eq!(result.fields[0].numeric_value, Some(0xDEADBEEF));
    }

    #[test]
    fn test_string_field_null_terminated() {
        let yaml = r#"
name: String Test
fields:
  - name: message
    offset: 0
    size: 10
    type: string
"#;
        let template = StructTemplate::from_yaml(yaml).unwrap();
        let mut data = vec![0u8; 10];
        data[0..5].copy_from_slice(b"Hello");
        data[5] = 0; // Null terminator

        let config = StructConfig::default();
        let cmd = StructCommand::new(config);
        let result = cmd.parse(&data, &template).unwrap();

        assert_eq!(result.fields[0].value, "\"Hello\"");
    }

    #[test]
    fn test_signed_integer_parsing() {
        let yaml = r#"
name: Signed Test
endian: little
fields:
  - name: negative
    offset: 0
    size: 4
    type: i32
"#;
        let template = StructTemplate::from_yaml(yaml).unwrap();
        // -1 in little endian
        let data = vec![0xFF, 0xFF, 0xFF, 0xFF];

        let config = StructConfig::default();
        let cmd = StructCommand::new(config);
        let result = cmd.parse(&data, &template).unwrap();

        assert_eq!(result.fields[0].numeric_value, Some(-1));
        assert_eq!(result.fields[0].value, "-1");
    }

    #[test]
    fn test_out_of_bounds_field() {
        let yaml = r#"
name: Test
fields:
  - name: oob
    offset: 100
    size: 4
    type: u32
"#;
        let template = StructTemplate::from_yaml(yaml).unwrap();
        let data = vec![0u8; 10]; // Only 10 bytes

        let config = StructConfig::default();
        let cmd = StructCommand::new(config);
        let result = cmd.parse(&data, &template);

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("exceeds data length"));
    }

    #[test]
    fn test_endianness_from_str() {
        assert_eq!(Endianness::from_str("little").unwrap(), Endianness::Little);
        assert_eq!(Endianness::from_str("big").unwrap(), Endianness::Big);
        assert_eq!(Endianness::from_str("LE").unwrap(), Endianness::Little);
        assert_eq!(Endianness::from_str("BE").unwrap(), Endianness::Big);
        assert!(Endianness::from_str("invalid").is_err());
    }

    #[test]
    fn test_output_format_from_str() {
        assert_eq!(
            StructOutputFormat::from_str("human").unwrap(),
            StructOutputFormat::Human
        );
        assert_eq!(
            StructOutputFormat::from_str("json").unwrap(),
            StructOutputFormat::Json
        );
        assert_eq!(
            StructOutputFormat::from_str("yaml").unwrap(),
            StructOutputFormat::Yaml
        );
        assert!(StructOutputFormat::from_str("invalid").is_err());
    }

    #[test]
    fn test_field_type_from_str() {
        assert_eq!(FieldType::from_str("u8").unwrap(), FieldType::U8);
        assert_eq!(FieldType::from_str("u16").unwrap(), FieldType::U16);
        assert_eq!(FieldType::from_str("uint32").unwrap(), FieldType::U32);
        assert_eq!(FieldType::from_str("hex_string").unwrap(), FieldType::HexString);
        assert_eq!(FieldType::from_str("string").unwrap(), FieldType::String);
        assert!(FieldType::from_str("invalid").is_err());
    }

    #[test]
    fn test_template_total_size() {
        let template = StructTemplate::from_yaml(ELF_TEMPLATE_YAML).unwrap();
        // Last field is e_type at offset 0x10 with size 2
        assert_eq!(template.total_size(), 0x12);
    }

    #[test]
    fn test_template_get_field() {
        let template = StructTemplate::from_yaml(ELF_TEMPLATE_YAML).unwrap();
        let field = template.get_field("e_type").unwrap();
        assert_eq!(field.offset, 0x10);
        assert!(template.get_field("nonexistent").is_none());
    }

    #[test]
    fn test_get_field_value() {
        let template = StructTemplate::from_yaml(ELF_TEMPLATE_YAML).unwrap();
        let data = mock_elf_header();

        let config = StructConfig::default();
        let cmd = StructCommand::new(config);
        let parsed = cmd.parse(&data, &template).unwrap();

        let value = cmd.get_field_value(&parsed, "e_ident_class");
        assert!(value.is_some());
        assert!(value.unwrap().contains("2")); // 64-bit = 2

        assert!(cmd.get_field_value(&parsed, "nonexistent").is_none());
    }

    #[test]
    fn test_assert_with_hex_prefix() {
        let yaml = r#"
name: Test
fields:
  - name: magic
    offset: 0
    size: 2
    type: hex_string
    assert: "0xdead"
"#;
        let template = StructTemplate::from_yaml(yaml).unwrap();
        let data = vec![0xde, 0xad];

        let config = StructConfig::default();
        let cmd = StructCommand::new(config);
        let result = cmd.parse(&data, &template).unwrap();

        assert!(result.all_assertions_passed);
    }

    #[test]
    fn test_assert_with_spaces() {
        let yaml = r#"
name: Test
fields:
  - name: magic
    offset: 0
    size: 4
    type: hex_string
    assert: "de ad be ef"
"#;
        let template = StructTemplate::from_yaml(yaml).unwrap();
        let data = vec![0xde, 0xad, 0xbe, 0xef];

        let config = StructConfig::default();
        let cmd = StructCommand::new(config);
        let result = cmd.parse(&data, &template).unwrap();

        assert!(result.all_assertions_passed);
    }

    #[test]
    fn test_all_integer_types() {
        let yaml = r#"
name: Integer Types Test
endian: little
fields:
  - name: u8_val
    offset: 0
    size: 1
    type: u8
  - name: i8_val
    offset: 1
    size: 1
    type: i8
  - name: u16_val
    offset: 2
    size: 2
    type: u16
  - name: i16_val
    offset: 4
    size: 2
    type: i16
  - name: u32_val
    offset: 6
    size: 4
    type: u32
  - name: i32_val
    offset: 10
    size: 4
    type: i32
  - name: u64_val
    offset: 14
    size: 8
    type: u64
  - name: i64_val
    offset: 22
    size: 8
    type: i64
"#;
        let template = StructTemplate::from_yaml(yaml).unwrap();
        let mut data = vec![0u8; 30];
        data[0] = 255; // u8
        data[1] = 0xFF; // i8 = -1
        data[2..4].copy_from_slice(&1000u16.to_le_bytes()); // u16
        data[4..6].copy_from_slice(&(-100i16).to_le_bytes()); // i16
        data[6..10].copy_from_slice(&100000u32.to_le_bytes()); // u32
        data[10..14].copy_from_slice(&(-50000i32).to_le_bytes()); // i32
        data[14..22].copy_from_slice(&1000000000000u64.to_le_bytes()); // u64
        data[22..30].copy_from_slice(&(-500000000000i64).to_le_bytes()); // i64

        let config = StructConfig::default();
        let cmd = StructCommand::new(config);
        let result = cmd.parse(&data, &template).unwrap();

        assert_eq!(result.fields[0].numeric_value, Some(255));
        assert_eq!(result.fields[1].numeric_value, Some(-1));
        assert_eq!(result.fields[2].numeric_value, Some(1000));
        assert_eq!(result.fields[3].numeric_value, Some(-100));
        assert_eq!(result.fields[4].numeric_value, Some(100000));
        assert_eq!(result.fields[5].numeric_value, Some(-50000));
        assert_eq!(result.fields[6].numeric_value, Some(1000000000000));
        assert_eq!(result.fields[7].numeric_value, Some(-500000000000));
    }
}
