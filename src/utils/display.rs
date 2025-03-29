use crate::error::{BinfiddleError, Result};

pub fn display_bytes(data: &[u8], format: &str, chunk_size: usize, width: usize) -> Result<String> {
    if chunk_size == 0 {
        return Err(BinfiddleError::InvalidInput(
            "Chunk size must be greater than 0".to_string(),
        ));
    }

    // First convert to a vector of string representations
    let chunks = match format.to_lowercase().as_str() {
        "hex" => {
            if chunk_size % 8 == 0 {
                // Byte-aligned chunks
                data.chunks(chunk_size / 8)
                    .map(hex::encode)
                    .collect::<Vec<_>>()
            } else {
                // Bit-level chunks
                let bits: String = data
                    .iter()
                    .flat_map(|&b| (0..8).rev().map(move |i| ((b >> i) & 1).to_string()))
                    .collect();
                bits.as_bytes()
                    .chunks(chunk_size)
                    .map(|c| {
                        let chunk_str = String::from_utf8_lossy(c).to_string();
                        if chunk_str.len() == chunk_size {
                            format!(
                                "{:0>width$x}",
                                u64::from_str_radix(&chunk_str, 2).unwrap_or(0),
                                width = (chunk_size + 3) / 4
                            )
                        } else {
                            chunk_str
                        }
                    })
                    .collect()
            }
        }
        "dec" => {
            let bits: String = data
                .iter()
                .flat_map(|&b| (0..8).rev().map(move |i| ((b >> i) & 1).to_string()))
                .collect();
            bits.as_bytes()
                .chunks(chunk_size)
                .map(|c| {
                    let chunk_str = String::from_utf8_lossy(c).to_string();
                    u64::from_str_radix(&chunk_str, 2)
                        .map(|n| n.to_string())
                        .unwrap_or_else(|_| "?".to_string())
                })
                .collect()
        }
        "oct" => {
            let bits: String = data
                .iter()
                .flat_map(|&b| (0..8).rev().map(move |i| ((b >> i) & 1).to_string()))
                .collect();
            bits.as_bytes()
                .chunks(chunk_size)
                .map(|c| {
                    let chunk_str = String::from_utf8_lossy(c).to_string();
                    u64::from_str_radix(&chunk_str, 2)
                        .map(|n| format!("{:o}", n))
                        .unwrap_or_else(|_| "?".to_string())
                })
                .collect()
        }
        "bin" => {
            let bits: String = data
                .iter()
                .flat_map(|&b| (0..8).rev().map(move |i| ((b >> i) & 1).to_string()))
                .collect();
            bits.as_bytes()
                .chunks(chunk_size)
                .map(|c| String::from_utf8_lossy(c).to_string())
                .collect()
        }
        "ascii" => {
            if chunk_size == 8 {
                data.iter()
                    .map(|b| {
                        if b.is_ascii_graphic() || *b == b' ' {
                            format!("{}", *b as char)
                        } else {
                            ".".to_string()
                        }
                    })
                    .collect()
            } else {
                return Err(BinfiddleError::InvalidInput(
                    "ASCII output only supported for 8-bit chunks".to_string(),
                ));
            }
        }
        _ => {
            return Err(BinfiddleError::InvalidInput(format!(
                "Unsupported display format: {}",
                format
            )))
        }
    };

    // Handle width = 0 (no line breaks)
    if width == 0 {
        return Ok(chunks.join(" "));
    }

    // Group chunks into lines
    let mut result = String::new();
    for line in chunks.chunks(width) {
        if !result.is_empty() {
            result.push('\n');
        }
        result.push_str(&line.join(" "));
    }

    Ok(result)
}
