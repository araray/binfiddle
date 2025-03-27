use crate::error::Result;

pub fn display_bytes(data: &[u8], format: &str, chunk_size: usize) -> Result<String> {
    match format.to_lowercase().as_str() {
        "hex" => Ok(hex::encode(data)),
        "dec" => Ok(data.iter().map(|b| format!("{} ", b)).collect()),
        "oct" => Ok(data.iter().map(|b| format!("{:o} ", b)).collect()),
        "bin" => Ok(data
            .iter()
            .map(|b| format!("{:08b} ", b))
            .collect::<String>()),
        "ascii" => Ok(String::from_utf8_lossy(data).to_string()),
        "chunked" => {
            let mut result = String::new();
            for chunk in data.chunks((chunk_size + 7) / 8) {
                let bits: String = chunk
                    .iter()
                    .flat_map(|&b| (0..8).rev().map(move |i| ((b >> i) & 1).to_string()))
                    .take(chunk_size)
                    .collect();
                result.push_str(&format!("{} ", bits));
            }
            Ok(result.trim_end().to_string())
        }
        _ => Err(crate::error::BinfiddleError::InvalidInput(format!(
            "Unsupported display format: {}",
            format
        ))),
    }
}
