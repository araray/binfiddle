use crate::error::{BinfiddleError, Result};

pub fn display_bytes(data: &[u8], format: &str, chunk_size: usize) -> Result<String> {
    if chunk_size == 0 {
        return Err(BinfiddleError::InvalidInput(
            "Chunk size must be greater than 0".to_string(),
        ));
    }

    match format.to_lowercase().as_str() {
        "hex" => {
            if chunk_size % 8 == 0 {
                // Byte-aligned chunks - use standard hex encoding
                let chunks = data.chunks(chunk_size / 8);
                let hex_strs: Vec<String> = chunks.map(hex::encode).collect();
                Ok(hex_strs.join(" "))
            } else {
                // Bit-level chunks
                let bits: String = data
                    .iter()
                    .flat_map(|&b| (0..8).rev().map(move |i| ((b >> i) & 1).to_string()))
                    .collect();
                let chunks: Vec<String> = bits
                    .as_bytes()
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
                    .collect();
                Ok(chunks.join(" "))
            }
        }
        "dec" => {
            let bits: String = data
                .iter()
                .flat_map(|&b| (0..8).rev().map(move |i| ((b >> i) & 1).to_string()))
                .collect();
            let chunks: Vec<String> = bits
                .as_bytes()
                .chunks(chunk_size)
                .map(|c| {
                    let chunk_str = String::from_utf8_lossy(c).to_string();
                    u64::from_str_radix(&chunk_str, 2)
                        .map(|n| n.to_string())
                        .unwrap_or_else(|_| "?".to_string())
                })
                .collect();
            Ok(chunks.join(" "))
        }
        "oct" => {
            let bits: String = data
                .iter()
                .flat_map(|&b| (0..8).rev().map(move |i| ((b >> i) & 1).to_string()))
                .collect();
            let chunks: Vec<String> = bits
                .as_bytes()
                .chunks(chunk_size)
                .map(|c| {
                    let chunk_str = String::from_utf8_lossy(c).to_string();
                    u64::from_str_radix(&chunk_str, 2)
                        .map(|n| format!("{:o}", n))
                        .unwrap_or_else(|_| "?".to_string())
                })
                .collect();
            Ok(chunks.join(" "))
        }
        "bin" => {
            let bits: String = data
                .iter()
                .flat_map(|&b| (0..8).rev().map(move |i| ((b >> i) & 1).to_string()))
                .collect();
            let chunks: Vec<String> = bits
                .as_bytes()
                .chunks(chunk_size)
                .map(|c| String::from_utf8_lossy(c).to_string())
                .collect();
            Ok(chunks.join(" "))
        }
        "ascii" => {
            if chunk_size == 8 {
                Ok(String::from_utf8_lossy(data).to_string())
            } else {
                Err(BinfiddleError::InvalidInput(
                    "ASCII output only supported for 8-bit chunks".to_string(),
                ))
            }
        }
        _ => Err(BinfiddleError::InvalidInput(format!(
            "Unsupported display format: {}",
            format
        ))),
    }
}
