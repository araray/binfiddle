use crate::error::{BinfiddleError, Result};

pub fn parse_input(input: &str, format: &str) -> Result<Vec<u8>> {
    match format.to_lowercase().as_str() {
        "hex" => hex::decode(input).map_err(|e| BinfiddleError::Parse(e.to_string())),
        "dec" => {
            let bytes: Result<Vec<u8>> = input
                .split_whitespace()
                .map(|s| {
                    s.parse::<u8>()
                        .map_err(|e| BinfiddleError::Parse(e.to_string()))
                })
                .collect();
            bytes
        }
        _ => Err(BinfiddleError::InvalidInput(format!(
            "Unsupported input format: {}",
            format
        ))),
    }
}

pub fn parse_range(range_str: &str, max_len: usize) -> Result<(usize, Option<usize>)> {
    if range_str.is_empty() {
        return Ok((0, None));
    }

    let parts: Vec<&str> = range_str.split("..").collect();
    match parts.len() {
        1 => {
            let start = parse_index(parts[0], max_len)?;
            Ok((start, Some(start + 1)))
        }
        2 => {
            let start = if parts[0].is_empty() {
                0
            } else {
                parse_index(parts[0], max_len)?
            };

            let end = if parts[1].is_empty() {
                None
            } else {
                Some(parse_index(parts[1], max_len)?)
            };

            Ok((start, end))
        }
        _ => Err(BinfiddleError::Parse(
            "Invalid range format. Use 'start..end' or 'index'".to_string(),
        )),
    }
}

fn parse_index(index_str: &str, max_len: usize) -> Result<usize> {
    let index = if index_str.starts_with('0') && index_str.len() > 1 {
        // Hexadecimal
        usize::from_str_radix(&index_str[1..], 16).map_err(|e| {
            BinfiddleError::Parse(format!("Failed to parse hex index '{}': {}", index_str, e))
        })?
    } else {
        // Decimal
        index_str.parse().map_err(|e| {
            BinfiddleError::Parse(format!("Failed to parse index '{}': {}", index_str, e))
        })?
    };

    if index > max_len {
        Err(BinfiddleError::InvalidRange(format!(
            "Index {} out of bounds (max {})",
            index, max_len
        )))
    } else {
        Ok(index)
    }
}
