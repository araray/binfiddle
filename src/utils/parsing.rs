use crate::error::{BinfiddleError, Result};

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

pub fn parse_input(input: &str, format: &str) -> Result<Vec<u8>> {
    match format.to_lowercase().as_str() {
        "hex" => hex::decode(input).map_err(|e| BinfiddleError::Parse(e.to_string())),
        "dec" => input
            .split_whitespace()
            .map(|s| {
                s.parse::<u8>()
                    .map_err(|e| BinfiddleError::Parse(e.to_string()))
            })
            .collect(),
        "oct" => input
            .split_whitespace()
            .map(|s| u8::from_str_radix(s, 8).map_err(|e| BinfiddleError::Parse(e.to_string())))
            .collect(),
        "bin" => input
            .split_whitespace()
            .map(|s| u8::from_str_radix(s, 2).map_err(|e| BinfiddleError::Parse(e.to_string())))
            .collect(),
        _ => Err(BinfiddleError::InvalidInput(format!(
            "Unsupported input format: {}",
            format
        ))),
    }
}

pub fn parse_bit_input(input: &str, format: &str, chunk_size: usize) -> Result<Vec<u8>> {
    if chunk_size == 0 || chunk_size > 64 {
        return Err(BinfiddleError::InvalidInput(
            "Chunk size must be between 1 and 64 bits".to_string(),
        ));
    }

    match format.to_lowercase().as_str() {
        "bin" => {
            let clean_input: String = input.chars().filter(|c| !c.is_whitespace()).collect();

            if clean_input.is_empty() {
                return Ok(Vec::new());
            }

            if clean_input.chars().any(|c| c != '0' && c != '1') {
                return Err(BinfiddleError::Parse(
                    "Binary input contains non-binary characters".to_string(),
                ));
            }

            let padding = (chunk_size - (clean_input.len() % chunk_size)) % chunk_size;
            let padded_input = format!(
                "{:0<width$}",
                clean_input,
                width = clean_input.len() + padding
            );

            let mut result = Vec::new();
            for chunk in padded_input.chars().collect::<Vec<_>>().chunks(chunk_size) {
                let binary_str: String = chunk.iter().collect();
                let value = u64::from_str_radix(&binary_str, 2)
                    .map_err(|e| BinfiddleError::Parse(e.to_string()))?;

                let byte_count = (chunk_size + 7) / 8;
                let bytes = value.to_be_bytes();
                result.extend_from_slice(&bytes[bytes.len() - byte_count..]);
            }
            Ok(result)
        }
        "hex" => {
            let bytes = hex::decode(input).map_err(|e| BinfiddleError::Parse(e.to_string()))?;

            if chunk_size == 8 {
                return Ok(bytes);
            }

            let bit_str: String = bytes
                .iter()
                .flat_map(|&b| {
                    (0..8)
                        .rev()
                        .map(move |i| (((b >> i) & 1) as u8).to_string())
                })
                .collect();

            parse_bit_input(&bit_str, "bin", chunk_size)
        }
        "dec" => {
            let numbers: Vec<&str> = input.split_whitespace().collect();
            let mut result = Vec::new();

            for num_str in numbers {
                let value = num_str
                    .parse::<u64>()
                    .map_err(|e| BinfiddleError::Parse(e.to_string()))?;

                if value >= (1 << chunk_size) {
                    return Err(BinfiddleError::Parse(format!(
                        "Value {} exceeds chunk size of {} bits",
                        value, chunk_size
                    )));
                }

                let byte_count = (chunk_size + 7) / 8;
                let bytes = value.to_be_bytes();
                result.extend_from_slice(&bytes[bytes.len() - byte_count..]);
            }
            Ok(result)
        }
        "oct" => {
            let numbers: Vec<&str> = input.split_whitespace().collect();
            let mut result = Vec::new();

            for num_str in numbers {
                let value = u64::from_str_radix(num_str, 8)
                    .map_err(|e| BinfiddleError::Parse(e.to_string()))?;

                if value >= (1 << chunk_size) {
                    return Err(BinfiddleError::Parse(format!(
                        "Value {} exceeds chunk size of {} bits",
                        value, chunk_size
                    )));
                }

                let byte_count = (chunk_size + 7) / 8;
                let bytes = value.to_be_bytes();
                result.extend_from_slice(&bytes[bytes.len() - byte_count..]);
            }
            Ok(result)
        }
        _ => Err(BinfiddleError::InvalidInput(format!(
            "Unsupported input format for bit parsing: {}",
            format
        ))),
    }
}

fn parse_index(index_str: &str, max_len: usize) -> Result<usize> {
    let index = if index_str.starts_with('0') && index_str.len() > 1 {
        usize::from_str_radix(&index_str[1..], 16).map_err(|e| {
            BinfiddleError::Parse(format!("Failed to parse hex index '{}': {}", index_str, e))
        })?
    } else {
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
