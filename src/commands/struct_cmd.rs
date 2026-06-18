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

use super::struct_bits::{byte_span, read_bits, sign_extend, BitAddress};
use super::struct_expr::{eval_to_bool, eval_to_i128, resolve_size_or_offset, EvalContext};

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

impl std::str::FromStr for Endianness {
    type Err = BinfiddleError;

    /// Parses an endianness string.
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
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

impl std::str::FromStr for StructOutputFormat {
    type Err = BinfiddleError;

    /// Parses an output format string.
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
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
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize, Serialize)]
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
    #[default]
    Bytes,
    /// Computed value from an expression
    Computed,
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

    /// Returns the maximum bit width this type can hold, if it supports bit-level parsing.
    pub fn max_bit_width(&self) -> Option<u8> {
        match self {
            FieldType::U8 | FieldType::I8 => Some(8),
            FieldType::U16 | FieldType::I16 => Some(16),
            FieldType::U32 | FieldType::I32 => Some(32),
            FieldType::U64 | FieldType::I64 => Some(64),
            _ => None,
        }
    }
}

impl std::str::FromStr for FieldType {
    type Err = BinfiddleError;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
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
            "computed" | "calc" => Ok(FieldType::Computed),
            _ => Err(BinfiddleError::Parse(format!(
                "Invalid field type '{}': expected u8, u16, u32, u64, i8, i16, i32, i64, \
                 hex_string, string, bytes, or computed",
                s
            ))),
        }
    }
}

/// A literal value or an expression string.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum ValueOrExpression {
    /// A fixed numeric value.
    Number(usize),
    /// An expression such as `$@prev_end + 4` or `$filename_length`.
    Expression(String),
}

impl ValueOrExpression {
    /// Returns the expression string if this is an expression.
    pub fn as_expr(&self) -> Option<&str> {
        match self {
            ValueOrExpression::Expression(s) => Some(s),
            _ => None,
        }
    }

    /// Resolves to a usize using the given evaluation context.
    pub fn resolve(&self, ctx: &EvalContext) -> Result<usize> {
        match self {
            ValueOrExpression::Number(n) => Ok(*n),
            ValueOrExpression::Expression(s) => resolve_size_or_offset(s, ctx),
        }
    }
}

/// Definition of a bitfield within an integer field.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct BitfieldDefinition {
    /// Name of the bitfield.
    pub name: String,
    /// Bit range, e.g. `0` or `2..5`.
    #[serde(deserialize_with = "deserialize_bit_range")]
    pub bits: (u8, u8),
    /// Interpretation type: `bool`, `u8`, `u16`, `u32`, `u64`.
    #[serde(rename = "type", default = "default_bitfield_type")]
    pub field_type: String,
    /// Optional description.
    #[serde(default)]
    pub description: Option<String>,
}

fn default_bitfield_type() -> String {
    "bool".to_string()
}

/// Deserialize a bit range from either a single number or `start..end`.
fn deserialize_bit_range<'de, D>(deserializer: D) -> std::result::Result<(u8, u8), D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::Error;

    #[derive(Deserialize)]
    #[serde(untagged)]
    enum BitRangeValue {
        Single(u8),
        String(String),
    }

    match BitRangeValue::deserialize(deserializer)? {
        BitRangeValue::Single(n) => Ok((n, n)),
        BitRangeValue::String(s) => {
            let s = s.trim();
            if let Some((start, end)) = s.split_once("..") {
                let start = start
                    .trim()
                    .parse::<u8>()
                    .map_err(|e| D::Error::custom(format!("Invalid bit start: {}", e)))?;
                let end = end
                    .trim()
                    .parse::<u8>()
                    .map_err(|e| D::Error::custom(format!("Invalid bit end: {}", e)))?;
                if start > end {
                    return Err(D::Error::custom("Bit range start must be <= end"));
                }
                Ok((start, end))
            } else {
                let n = s
                    .parse::<u8>()
                    .map_err(|e| D::Error::custom(format!("Invalid bit index: {}", e)))?;
                Ok((n, n))
            }
        }
    }
}

/// Custom deserializer for `ValueOrExpression`.
fn deserialize_value_or_expression<'de, D>(
    deserializer: D,
) -> std::result::Result<Option<ValueOrExpression>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::Error;

    #[derive(Deserialize)]
    #[serde(untagged)]
    enum RawValue {
        Number(usize),
        String(String),
    }

    match Option::<RawValue>::deserialize(deserializer)? {
        None => Ok(None),
        Some(RawValue::Number(n)) => Ok(Some(ValueOrExpression::Number(n))),
        Some(RawValue::String(s)) => {
            let s = s.trim();
            if s.is_empty() {
                return Err(D::Error::custom("Empty offset/size expression"));
            }
            if s.starts_with('$') {
                Ok(Some(ValueOrExpression::Expression(s.to_string())))
            } else {
                // Try to parse as hex/decimal literal
                let n = if s.starts_with("0x") || s.starts_with("0X") {
                    usize::from_str_radix(&s[2..], 16).map_err(|e| {
                        D::Error::custom(format!("Invalid hex value '{}': {}", s, e))
                    })?
                } else {
                    s.parse::<usize>()
                        .map_err(|e| D::Error::custom(format!("Invalid number '{}': {}", s, e)))?
                };
                Ok(Some(ValueOrExpression::Number(n)))
            }
        }
    }
}

/// Custom deserializer for an optional `ValueOrExpression`.
fn deserialize_optional_value_or_expression<'de, D>(
    deserializer: D,
) -> std::result::Result<Option<ValueOrExpression>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    deserialize_value_or_expression(deserializer)
}

/// Represents a single field definition in a template.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FieldDefinition {
    /// Field name (identifier)
    pub name: String,

    /// Byte offset from start of structure
    #[serde(default, deserialize_with = "deserialize_value_or_expression")]
    pub offset: Option<ValueOrExpression>,

    /// Size in bytes (optional for computed/array fields or when bit_size is used)
    #[serde(default, deserialize_with = "deserialize_optional_value_or_expression")]
    pub size: Option<ValueOrExpression>,

    /// Bit offset inside the byte at `offset` (0-7). Optional; defaults to 0.
    #[serde(default)]
    pub bit_offset: Option<u8>,

    /// Size in bits (1-64). When present, the field is read at bit precision
    /// and `size` is ignored.
    #[serde(default)]
    pub bit_size: Option<u8>,

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

    /// Conditional expression; field is skipped if false.
    #[serde(default)]
    pub when: Option<String>,

    /// For arrays: expression evaluating to element count.
    #[serde(default)]
    pub count: Option<String>,

    /// For arrays: template file to parse for each element.
    #[serde(default)]
    pub element_template: Option<String>,

    /// For computed fields: expression yielding the value.
    #[serde(default)]
    pub value: Option<String>,

    /// For integer fields: bitfield definitions.
    #[serde(default)]
    pub bitfields: Option<Vec<BitfieldDefinition>>,
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
        serde_yaml::from_str(yaml)
            .map_err(|e| BinfiddleError::Parse(format!("Failed to parse template YAML: {}", e)))
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

    /// Returns the total size covered by this template using literal values only.
    pub fn total_size(&self) -> usize {
        self.fields
            .iter()
            .filter_map(|f| {
                let offset = match &f.offset {
                    Some(ValueOrExpression::Number(n)) => *n,
                    _ => return None,
                };
                let size = match &f.size {
                    Some(ValueOrExpression::Number(n)) => *n,
                    _ => return None,
                };
                Some(offset + size)
            })
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

        // Validate field type sizes for literal sizes
        for field in &self.fields {
            if let Some(expected) = field.field_type.expected_size() {
                if let Some(ValueOrExpression::Number(size)) = &field.size {
                    if *size != expected {
                        return Err(BinfiddleError::Parse(format!(
                            "Field '{}' has type {:?} which requires {} bytes, but size is {}",
                            field.name, field.field_type, expected, size
                        )));
                    }
                }
            }

            // Validate bit-level constraints
            if let Some(bit_offset) = field.bit_offset {
                if bit_offset >= 8 {
                    return Err(BinfiddleError::Parse(format!(
                        "Field '{}' has invalid bit_offset {}; must be 0-7",
                        field.name, bit_offset
                    )));
                }
            }
            if let Some(bit_size) = field.bit_size {
                if bit_size == 0 || bit_size > 64 {
                    return Err(BinfiddleError::Parse(format!(
                        "Field '{}' has invalid bit_size {}; must be 1-64",
                        field.name, bit_size
                    )));
                }
                if let Some(max_bits) = field.field_type.max_bit_width() {
                    if bit_size > max_bits {
                        return Err(BinfiddleError::Parse(format!(
                            "Field '{}' has type {:?} which supports at most {} bits, but bit_size is {}",
                            field.name, field.field_type, max_bits, bit_size
                        )));
                    }
                } else {
                    return Err(BinfiddleError::Parse(format!(
                        "Field '{}' has type {:?} which does not support bit-level parsing",
                        field.name, field.field_type
                    )));
                }

                if field.count.is_some() {
                    return Err(BinfiddleError::Parse(format!(
                        "Field '{}' cannot use bit_size with counted arrays",
                        field.name
                    )));
                }
                if field.field_type == FieldType::Computed {
                    return Err(BinfiddleError::Parse(format!(
                        "Field '{}' cannot use bit_size with computed type",
                        field.name
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
    /// Offset in data (start byte)
    pub offset: usize,
    /// Size in bytes (total bytes spanned)
    pub size: usize,
    /// Bit offset inside the start byte, if this is a bit-level field.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bit_offset: Option<u8>,
    /// Size in bits, if this is a bit-level field.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bit_size: Option<u8>,
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
        let ctx = EvalContext::new(data.len(), 0);
        self.parse_with_context(data, template, ctx)
    }

    /// Parses binary data using an existing evaluation context.
    fn parse_with_context(
        &self,
        data: &[u8],
        template: &StructTemplate,
        mut ctx: EvalContext,
    ) -> Result<ParsedStruct> {
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

            // Evaluate conditional
            if let Some(when_expr) = &field_def.when {
                if !eval_to_bool(when_expr, &ctx)? {
                    continue;
                }
            }

            // Resolve offset: explicit or $@prev_end
            let offset = match &field_def.offset {
                Some(expr) => expr.resolve(&ctx)?,
                None => ctx.prev_end(),
            };
            ctx.set_current_offset(offset);

            // Parse the field (which may produce multiple parsed fields for arrays/bitfields)
            let mut parsed_fields =
                self.parse_field_at(data, field_def, offset, template.endian, &ctx)?;

            // Update context with numeric values from all parsed fields and end offset
            if let Some(main) = parsed_fields.first() {
                ctx.set_prev_end(main.offset + main.size);
            }
            for parsed in &parsed_fields {
                if let Some(numeric) = parsed.numeric_value {
                    ctx.set_variable(parsed.name.clone(), numeric);
                }
            }

            // Check assertions
            for parsed in &parsed_fields {
                if let Some(passed) = parsed.assertion_passed {
                    if !passed {
                        all_assertions_passed = false;
                    }
                }
            }

            fields.append(&mut parsed_fields);
        }

        Ok(ParsedStruct {
            name: template.name.clone(),
            description: template.description.clone(),
            fields,
            all_assertions_passed,
        })
    }

    /// Parses a field at the given offset, handling arrays, computed fields,
    /// bitfields, and normal data fields.
    fn parse_field_at(
        &self,
        data: &[u8],
        field: &FieldDefinition,
        offset: usize,
        endian: Endianness,
        ctx: &EvalContext,
    ) -> Result<Vec<ParsedField>> {
        // Handle counted arrays
        if let Some(count_expr) = &field.count {
            let count = eval_to_i128(count_expr, ctx)?;
            if count < 0 {
                return Err(BinfiddleError::InvalidRange(format!(
                    "Array count for field '{}' evaluated to negative value {}",
                    field.name, count
                )));
            }
            let count = count as usize;
            return self.parse_array(data, field, offset, count, endian, ctx);
        }

        // Handle computed fields
        if field.field_type == FieldType::Computed {
            let value_expr = field.value.as_ref().ok_or_else(|| {
                BinfiddleError::Parse(format!(
                    "Computed field '{}' requires a 'value' expression",
                    field.name
                ))
            })?;
            let value = eval_to_i128(value_expr, ctx)?;
            let parsed = ParsedField {
                name: field.name.clone(),
                offset,
                size: 0,
                bit_offset: None,
                bit_size: None,
                raw_bytes: Vec::new(),
                value: format!("{}", value),
                numeric_value: Some(value),
                enum_name: None,
                assertion_passed: None,
                description: field.description.clone(),
            };
            return Ok(vec![parsed]);
        }

        // Determine if this is a bit-level field.
        let bit_offset_val = field.bit_offset.unwrap_or(0);
        let (value, numeric_value, raw_bytes, size, bit_size_opt) = if let Some(bit_size) =
            field.bit_size
        {
            let bit_start = BitAddress::new(offset, bit_offset_val).flat();
            let span = byte_span(bit_offset_val, bit_size);
            if offset + span > data.len() {
                return Err(BinfiddleError::InvalidRange(format!(
                    "Field '{}' at bit offset {} with bit size {} exceeds data length {} bits",
                    field.name,
                    bit_start,
                    bit_size,
                    data.len() * 8
                )));
            }
            let raw = data[offset..offset + span].to_vec();
            let bits_value = read_bits(data, bit_start, bit_size as usize, endian);
            let (value, numeric) = self.interpret_bits(bits_value, &field.field_type, bit_size)?;
            (value, numeric, raw, span, Some(bit_size))
        } else {
            // Resolve size for normal byte-aligned fields
            let size = match &field.size {
                Some(expr) => expr.resolve(ctx)?,
                None => {
                    return Err(BinfiddleError::Parse(format!(
                        "Field '{}' is missing required 'size'",
                        field.name
                    )))
                }
            };

            // Bounds check
            if offset + size > data.len() {
                return Err(BinfiddleError::InvalidRange(format!(
                    "Field '{}' at offset 0x{:x} with size {} exceeds data length {}",
                    field.name,
                    offset,
                    size,
                    data.len()
                )));
            }

            let raw_bytes = data[offset..offset + size].to_vec();

            // Parse based on type
            let (value, numeric_value) =
                self.interpret_field(&raw_bytes, &field.field_type, endian)?;
            (value, numeric_value, raw_bytes, size, None)
        };

        // Check enum mapping
        let enum_name = if let (Some(ref enum_map), Some(num)) = (&field.r#enum, numeric_value) {
            enum_map.get(&num.to_string()).cloned()
        } else {
            None
        };

        // Check assertion (only for byte-aligned fields; raw bytes include extra bits for bit fields)
        let assertion_passed = if bit_size_opt.is_some() {
            None
        } else {
            field.assert.as_ref().map(|expected| {
                let expected_bytes = self.parse_assert_value(expected);
                expected_bytes.as_ref() == Some(&raw_bytes)
            })
        };

        // Format the display value
        let display_value = if let Some(ref enum_name) = enum_name {
            format!("{} ({})", value, enum_name)
        } else {
            value
        };

        let main_field = ParsedField {
            name: field.name.clone(),
            offset,
            size,
            bit_offset: field.bit_offset,
            bit_size: bit_size_opt,
            raw_bytes,
            value: display_value,
            numeric_value,
            enum_name,
            assertion_passed,
            description: field.description.clone(),
        };

        let mut result = vec![main_field.clone()];

        // Extract bitfields if defined and we have a numeric value
        if let (Some(bitfields), Some(numeric)) = (&field.bitfields, numeric_value) {
            let max_bit = bit_size_opt.unwrap_or((size * 8) as u8);
            let mut bitfield_fields =
                self.parse_bitfields(&field.name, offset, size, max_bit, numeric, bitfields);
            result.append(&mut bitfield_fields);
        }

        Ok(result)
    }

    /// Parses a counted array of elements.
    fn parse_array(
        &self,
        data: &[u8],
        field: &FieldDefinition,
        start_offset: usize,
        count: usize,
        endian: Endianness,
        ctx: &EvalContext,
    ) -> Result<Vec<ParsedField>> {
        let mut result = Vec::new();

        if let Some(template_name) = &field.element_template {
            // Load external template for each element
            let element_template = StructTemplate::from_file(template_name)?;
            let element_size = element_template.total_size();
            if element_size == 0 {
                return Err(BinfiddleError::Parse(format!(
                    "Array field '{}' uses element template '{}' with zero size",
                    field.name, template_name
                )));
            }

            for i in 0..count {
                let element_offset = start_offset + i * element_size;
                if element_offset + element_size > data.len() {
                    return Err(BinfiddleError::InvalidRange(format!(
                        "Array '{}' element {} at offset 0x{:x} with size {} exceeds data length {}",
                        field.name,
                        i,
                        element_offset,
                        element_size,
                        data.len()
                    )));
                }

                let element_ctx = ctx.child_with_base(element_offset);
                let parsed_element =
                    self.parse_with_context(data, &element_template, element_ctx)?;
                for parsed_field in parsed_element.fields {
                    let indexed_name = format!("{}[{}].{}", field.name, i, parsed_field.name);
                    let mut indexed = parsed_field;
                    indexed.name = indexed_name;
                    result.push(indexed);
                }
            }
        } else {
            // Array of identical simple fields
            let size = match &field.size {
                Some(expr) => expr.resolve(ctx)?,
                None => {
                    return Err(BinfiddleError::Parse(format!(
                        "Array field '{}' without element_template requires 'size'",
                        field.name
                    )))
                }
            };

            for i in 0..count {
                let element_offset = start_offset + i * size;
                if element_offset + size > data.len() {
                    return Err(BinfiddleError::InvalidRange(format!(
                        "Array '{}' element {} at offset 0x{:x} with size {} exceeds data length {}",
                        field.name,
                        i,
                        element_offset,
                        size,
                        data.len()
                    )));
                }

                let raw_bytes = data[element_offset..element_offset + size].to_vec();
                let (value, numeric_value) =
                    self.interpret_field(&raw_bytes, &field.field_type, endian)?;

                let field_name = format!("{}[{}]", field.name, i);
                result.push(ParsedField {
                    name: field_name,
                    offset: element_offset,
                    size,
                    bit_offset: None,
                    bit_size: None,
                    raw_bytes,
                    value,
                    numeric_value,
                    enum_name: None,
                    assertion_passed: None,
                    description: field.description.clone(),
                });
            }
        }

        Ok(result)
    }

    /// Extracts bitfield values from an integer field.
    fn parse_bitfields(
        &self,
        parent_name: &str,
        parent_offset: usize,
        _parent_size: usize,
        max_bit: u8,
        value: i128,
        bitfields: &[BitfieldDefinition],
    ) -> Vec<ParsedField> {
        let mut result = Vec::new();

        for bf in bitfields {
            let (start, end) = bf.bits;
            if end >= max_bit {
                // Skip out-of-range bitfields silently; validation could be stricter
                continue;
            }
            let width = (end - start + 1) as u32;
            let mask = ((1i128 << width) - 1) << start;
            let raw = ((value as u128 & mask as u128) >> start) as i128;

            let (display, numeric) = match bf.field_type.to_lowercase().as_str() {
                "bool" | "boolean" => (
                    if raw != 0 {
                        "true".to_string()
                    } else {
                        "false".to_string()
                    },
                    Some(raw),
                ),
                "u8" | "u16" | "u32" | "u64" | "i8" | "i16" | "i32" | "i64" => {
                    (format!("{}", raw), Some(raw))
                }
                _ => (format!("{}", raw), Some(raw)),
            };

            let name = format!("{}.{}", parent_name, bf.name);
            result.push(ParsedField {
                name,
                offset: parent_offset,
                size: 0,
                bit_offset: None,
                bit_size: None,
                raw_bytes: Vec::new(),
                value: display,
                numeric_value: numeric,
                enum_name: None,
                assertion_passed: None,
                description: bf.description.clone(),
            });
        }

        result
    }

    /// Interprets a bit-level value according to field type.
    fn interpret_bits(
        &self,
        value: u64,
        field_type: &FieldType,
        bit_width: u8,
    ) -> Result<(String, Option<i128>)> {
        match field_type {
            FieldType::U8 | FieldType::U16 | FieldType::U32 | FieldType::U64 => {
                Ok((format!("{}", value), Some(value as i128)))
            }
            FieldType::I8 | FieldType::I16 | FieldType::I32 | FieldType::I64 => {
                let signed = sign_extend(value, bit_width as usize)?;
                Ok((format!("{}", signed), Some(signed as i128)))
            }
            _ => Err(BinfiddleError::Parse(format!(
                "Type {:?} does not support bit-level parsing",
                field_type
            ))),
        }
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
            FieldType::Computed => Err(BinfiddleError::Parse(
                "Computed fields should not be interpreted as raw bytes".to_string(),
            )),
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
        let offset_width = parsed
            .fields
            .iter()
            .map(|f| {
                if f.bit_size.is_some() {
                    "0x00000000.0".len()
                } else {
                    "0x00000000".len()
                }
            })
            .max()
            .unwrap_or(10)
            .max(10);
        let size_width = parsed
            .fields
            .iter()
            .map(|f| {
                if let Some(bit_size) = f.bit_size {
                    format!("{}b", bit_size).len()
                } else {
                    format!("{}", f.size).len()
                }
            })
            .max()
            .unwrap_or(4)
            .max(4);
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

            let (offset_str, size_str) = if let Some(bit_size) = field.bit_size {
                (
                    format!("0x{:08x}.{} ", field.offset, field.bit_offset.unwrap_or(0)),
                    format!("{}b", bit_size),
                )
            } else {
                (
                    format!("0x{:08x}  ", field.offset),
                    format!("{}", field.size),
                )
            };

            output.push_str(&format!(
                "{:<name_width$}  {:>offset_width$}  {:>size_width$}  {:<value_width$}  {}\n",
                field.name,
                offset_str,
                size_str,
                field.value,
                status,
                name_width = name_width,
                offset_width = offset_width,
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
            let offset_str = {
                let base = field
                    .offset
                    .as_ref()
                    .map(|v| match v {
                        ValueOrExpression::Number(n) => format!("0x{:08x}", n),
                        ValueOrExpression::Expression(s) => s.clone(),
                    })
                    .unwrap_or_else(|| "auto".to_string());
                if let Some(bit_offset) = field.bit_offset {
                    format!("{}.{} ", base, bit_offset)
                } else {
                    base
                }
            };
            let size_str = if let Some(bit_size) = field.bit_size {
                format!("{}b", bit_size)
            } else {
                field
                    .size
                    .as_ref()
                    .map(|v| match v {
                        ValueOrExpression::Number(n) => format!("{}", n),
                        ValueOrExpression::Expression(s) => s.clone(),
                    })
                    .unwrap_or_else(|| "-".to_string())
            };
            output.push_str(&format!(
                "{:<name_width$}  {:>10}  {:>4}  {:<type_width$}  {}\n",
                field.name,
                offset_str,
                size_str,
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

    fn literal_offset(offset: &Option<ValueOrExpression>) -> usize {
        match offset {
            Some(ValueOrExpression::Number(n)) => *n,
            _ => panic!("Expected literal offset"),
        }
    }

    fn literal_size(size: &Option<ValueOrExpression>) -> usize {
        match size {
            Some(ValueOrExpression::Number(n)) => *n,
            _ => panic!("Expected literal size"),
        }
    }

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
        assert_eq!(literal_offset(&template.fields[0].offset), 0);
        assert_eq!(literal_size(&template.fields[0].size), 4);
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
        assert_eq!(literal_offset(&template.fields[0].offset), 0x100);
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
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("exceeds data length"));
    }

    #[test]
    fn test_endianness_from_str() {
        assert_eq!("little".parse::<Endianness>().unwrap(), Endianness::Little);
        assert_eq!("big".parse::<Endianness>().unwrap(), Endianness::Big);
        assert_eq!("LE".parse::<Endianness>().unwrap(), Endianness::Little);
        assert_eq!("BE".parse::<Endianness>().unwrap(), Endianness::Big);
        assert!("invalid".parse::<Endianness>().is_err());
    }

    #[test]
    fn test_output_format_from_str() {
        assert_eq!(
            "human".parse::<StructOutputFormat>().unwrap(),
            StructOutputFormat::Human
        );
        assert_eq!(
            "json".parse::<StructOutputFormat>().unwrap(),
            StructOutputFormat::Json
        );
        assert_eq!(
            "yaml".parse::<StructOutputFormat>().unwrap(),
            StructOutputFormat::Yaml
        );
        assert!("invalid".parse::<StructOutputFormat>().is_err());
    }

    #[test]
    fn test_field_type_from_str() {
        assert_eq!("u8".parse::<FieldType>().unwrap(), FieldType::U8);
        assert_eq!("u16".parse::<FieldType>().unwrap(), FieldType::U16);
        assert_eq!("uint32".parse::<FieldType>().unwrap(), FieldType::U32);
        assert_eq!(
            "hex_string".parse::<FieldType>().unwrap(),
            FieldType::HexString
        );
        assert_eq!("string".parse::<FieldType>().unwrap(), FieldType::String);
        assert!("invalid".parse::<FieldType>().is_err());
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
        assert_eq!(literal_offset(&field.offset), 0x10);
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

    #[test]
    fn test_field_reference_in_size() {
        let yaml = r#"
name: Dynamic String
endian: little
fields:
  - name: length
    offset: 0
    size: 2
    type: u16
  - name: data
    offset: 2
    size: $length
    type: bytes
"#;
        let template = StructTemplate::from_yaml(yaml).unwrap();
        let mut data = vec![0u8; 6];
        data[0..2].copy_from_slice(&4u16.to_le_bytes());
        data[2..6].copy_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF]);

        let cmd = StructCommand::new(StructConfig::default());
        let result = cmd.parse(&data, &template).unwrap();

        assert_eq!(result.fields.len(), 2);
        assert_eq!(result.fields[0].numeric_value, Some(4));
        assert_eq!(result.fields[1].name, "data");
        assert_eq!(result.fields[1].size, 4);
        assert_eq!(result.fields[1].value, "de ad be ef");
    }

    #[test]
    fn test_computed_field() {
        let yaml = r#"
name: Computed Test
fields:
  - name: a
    offset: 0
    size: 1
    type: u8
  - name: b
    offset: 1
    size: 1
    type: u8
  - name: sum
    type: computed
    value: $a + $b
"#;
        let template = StructTemplate::from_yaml(yaml).unwrap();
        let data = vec![10u8, 32u8];

        let cmd = StructCommand::new(StructConfig::default());
        let result = cmd.parse(&data, &template).unwrap();

        assert_eq!(result.fields.len(), 3);
        assert_eq!(result.fields[2].name, "sum");
        assert_eq!(result.fields[2].value, "42");
        assert_eq!(result.fields[2].numeric_value, Some(42));
    }

    #[test]
    fn test_conditional_field_with_bitfield_reference() {
        let yaml = r#"
name: Conditional Bitfield Test
endian: big
fields:
  - name: flags
    offset: 0
    size: 1
    type: u8
    bitfields:
      - name: urg
        bits: 5
        type: bool
  - name: urgent_pointer
    offset: 1
    size: 1
    type: u8
    when: $flags.urg == 1
"#;
        let template = StructTemplate::from_yaml(yaml).unwrap();

        // URG not set -> skip urgent_pointer
        let data1 = vec![0x00u8, 0xFF];
        let cmd = StructCommand::new(StructConfig::default());
        let result1 = cmd.parse(&data1, &template).unwrap();
        assert_eq!(result1.fields.len(), 2); // flags + flags.urg

        // URG set -> parse urgent_pointer
        let data2 = vec![0x20u8, 0xAB];
        let result2 = cmd.parse(&data2, &template).unwrap();
        assert_eq!(result2.fields.len(), 3);
        assert_eq!(result2.fields[2].name, "urgent_pointer");
        assert_eq!(result2.fields[2].numeric_value, Some(0xAB));
    }

    #[test]
    fn test_conditional_field() {
        let yaml = r#"
name: Conditional Test
fields:
  - name: version
    offset: 0
    size: 1
    type: u8
  - name: extra
    offset: 1
    size: 1
    type: u8
    when: $version >= 2
"#;
        let template = StructTemplate::from_yaml(yaml).unwrap();

        // version == 1: extra field should be skipped
        let data1 = vec![1u8, 0xFF];
        let cmd = StructCommand::new(StructConfig::default());
        let result1 = cmd.parse(&data1, &template).unwrap();
        assert_eq!(result1.fields.len(), 1);

        // version == 2: extra field should be parsed
        let data2 = vec![2u8, 0xAB];
        let result2 = cmd.parse(&data2, &template).unwrap();
        assert_eq!(result2.fields.len(), 2);
        assert_eq!(result2.fields[1].numeric_value, Some(0xAB));
    }

    #[test]
    fn test_bitfield_extraction() {
        let yaml = r#"
name: Bitfield Test
endian: little
fields:
  - name: flags
    offset: 0
    size: 2
    type: u16
    bitfields:
      - name: is_compressed
        bits: 0
        type: bool
      - name: compression_level
        bits: 2..5
        type: u8
"#;
        let template = StructTemplate::from_yaml(yaml).unwrap();
        // value = 0x0031 -> bit 0 = 1, bits 2-5 = 0b1100 (12)
        let data = vec![0x31u8, 0x00u8];

        let cmd = StructCommand::new(StructConfig::default());
        let result = cmd.parse(&data, &template).unwrap();

        assert_eq!(result.fields.len(), 3);
        let is_compressed = &result.fields[1];
        assert_eq!(is_compressed.name, "flags.is_compressed");
        assert_eq!(is_compressed.value, "true");

        let level = &result.fields[2];
        assert_eq!(level.name, "flags.compression_level");
        assert_eq!(level.numeric_value, Some(12));
    }

    #[test]
    fn test_counted_array_of_simple_fields() {
        let yaml = r#"
name: Array Test
endian: little
fields:
  - name: count
    offset: 0
    size: 1
    type: u8
  - name: values
    offset: 1
    size: 1
    type: u8
    count: $count
"#;
        let template = StructTemplate::from_yaml(yaml).unwrap();
        let data = vec![3u8, 0xAA, 0xBB, 0xCC];

        let cmd = StructCommand::new(StructConfig::default());
        let result = cmd.parse(&data, &template).unwrap();

        assert_eq!(result.fields.len(), 4); // count + 3 array elements
        assert_eq!(result.fields[1].name, "values[0]");
        assert_eq!(result.fields[1].numeric_value, Some(0xAA));
        assert_eq!(result.fields[3].name, "values[2]");
        assert_eq!(result.fields[3].numeric_value, Some(0xCC));
    }

    #[test]
    fn test_auto_offset_with_prev_end() {
        let yaml = r#"
name: Auto Offset Test
fields:
  - name: first
    offset: 0
    size: 2
    type: u16
  - name: second
    size: 1
    type: u8
"#;
        let template = StructTemplate::from_yaml(yaml).unwrap();
        let data = vec![0x00, 0x00, 0xAB];

        let cmd = StructCommand::new(StructConfig::default());
        let result = cmd.parse(&data, &template).unwrap();

        assert_eq!(result.fields[1].offset, 2);
        assert_eq!(result.fields[1].numeric_value, Some(0xAB));
    }

    #[test]
    fn test_bit_level_field_big_endian() {
        let yaml = r#"
name: Bit-level BE Test
endian: big
fields:
  - name: nibble
    offset: 0
    bit_offset: 0
    bit_size: 4
    type: u8
"#;
        let template = StructTemplate::from_yaml(yaml).unwrap();
        let data = vec![0xA0u8];

        let cmd = StructCommand::new(StructConfig::default());
        let result = cmd.parse(&data, &template).unwrap();

        assert_eq!(result.fields.len(), 1);
        assert_eq!(result.fields[0].name, "nibble");
        assert_eq!(result.fields[0].numeric_value, Some(0xA));
        assert_eq!(result.fields[0].bit_size, Some(4));
        assert_eq!(result.fields[0].bit_offset, Some(0));
    }

    #[test]
    fn test_bit_level_field_little_endian() {
        let yaml = r#"
name: Bit-level LE Test
endian: little
fields:
  - name: nibble
    offset: 0
    bit_offset: 0
    bit_size: 4
    type: u8
"#;
        let template = StructTemplate::from_yaml(yaml).unwrap();
        let data = vec![0x0Au8];

        let cmd = StructCommand::new(StructConfig::default());
        let result = cmd.parse(&data, &template).unwrap();

        assert_eq!(result.fields[0].numeric_value, Some(0xA));
    }

    #[test]
    fn test_bit_level_field_across_bytes() {
        let yaml = r#"
name: Bit-level Across Bytes Test
endian: big
fields:
  - name: twelve_bits
    offset: 0
    bit_offset: 4
    bit_size: 12
    type: u16
"#;
        let template = StructTemplate::from_yaml(yaml).unwrap();
        // Bytes: 0x12 0x34
        // Big-endian bit stream starting at bit 4:
        // 0001 0010 0011 0100
        // skip first 4 bits -> 0010 0011 0100 = 0x234
        let data = vec![0x12u8, 0x34u8];

        let cmd = StructCommand::new(StructConfig::default());
        let result = cmd.parse(&data, &template).unwrap();

        assert_eq!(result.fields[0].numeric_value, Some(0x234));
        assert_eq!(result.fields[0].size, 2);
    }

    #[test]
    fn test_bit_level_signed_field() {
        let yaml = r#"
name: Bit-level Signed Test
endian: big
fields:
  - name: signed_nibble
    offset: 0
    bit_offset: 0
    bit_size: 4
    type: i8
"#;
        let template = StructTemplate::from_yaml(yaml).unwrap();
        // 0x80 => top 4 bits are 1000b = -8 in 4-bit two's complement
        let data = vec![0x80u8];

        let cmd = StructCommand::new(StructConfig::default());
        let result = cmd.parse(&data, &template).unwrap();

        assert_eq!(result.fields[0].numeric_value, Some(-8));
    }

    #[test]
    fn test_bit_level_with_bitfields() {
        let yaml = r#"
name: Bit-level With Bitfields Test
endian: big
fields:
  - name: packed
    offset: 0
    bit_size: 8
    type: u8
    bitfields:
      - name: low
        bits: 0..3
        type: u8
      - name: high
        bits: 3..7
        type: u8
"#;
        let template = StructTemplate::from_yaml(yaml).unwrap();
        // 0xA7 = 1010 0111
        // bits 0..3 = 0111 = 7
        // bits 3..7 = 10100 = 20
        let data = vec![0xA7u8];

        let cmd = StructCommand::new(StructConfig::default());
        let result = cmd.parse(&data, &template).unwrap();

        assert_eq!(result.fields[0].numeric_value, Some(0xA7));
        assert_eq!(result.fields[1].name, "packed.low");
        assert_eq!(result.fields[1].numeric_value, Some(7));
        assert_eq!(result.fields[2].name, "packed.high");
        assert_eq!(result.fields[2].numeric_value, Some(20));
    }

    #[test]
    fn test_bit_level_validation_rejects_bad_bit_offset() {
        let yaml = r#"
name: Bad Bit Offset Test
fields:
  - name: bad
    offset: 0
    bit_offset: 8
    bit_size: 1
    type: u8
"#;
        assert!(StructTemplate::from_yaml(yaml).unwrap().validate().is_err());
    }

    #[test]
    fn test_bit_level_validation_rejects_zero_bit_size() {
        let yaml = r#"
name: Bad Bit Size Test
fields:
  - name: bad
    offset: 0
    bit_size: 0
    type: u8
"#;
        assert!(StructTemplate::from_yaml(yaml).unwrap().validate().is_err());
    }

    #[test]
    fn test_bit_level_validation_rejects_too_large_bit_size_for_type() {
        let yaml = r#"
name: Too Large Bit Size Test
fields:
  - name: bad
    offset: 0
    bit_size: 9
    type: u8
"#;
        assert!(StructTemplate::from_yaml(yaml).unwrap().validate().is_err());
    }

    #[test]
    fn test_bit_level_backwards_compatibility() {
        // Existing byte-aligned templates should still work unchanged.
        let yaml = r#"
name: Byte-aligned Test
endian: little
fields:
  - name: byte_value
    offset: 0
    size: 1
    type: u8
"#;
        let template = StructTemplate::from_yaml(yaml).unwrap();
        let data = vec![0x42u8];

        let cmd = StructCommand::new(StructConfig::default());
        let result = cmd.parse(&data, &template).unwrap();

        assert_eq!(result.fields[0].numeric_value, Some(0x42));
        assert_eq!(result.fields[0].bit_size, None);
    }
}
