//! Hash command implementation for binfiddle.
//!
//! Computes common digests over binary data, either for the whole input or
//! block-by-block. Supported algorithms: MD5, SHA-256, BLAKE3, and CRC32.

use super::Command;
use crate::error::{BinfiddleError, Result};
use crate::BinaryData;

/// Hash algorithm to compute.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HashAlgorithm {
    /// MD5 (128-bit)
    Md5,
    /// SHA-256 (256-bit)
    Sha256,
    /// BLAKE3 (256-bit)
    Blake3,
    /// CRC32/IEEE (32-bit)
    Crc32,
}

impl std::str::FromStr for HashAlgorithm {
    type Err = BinfiddleError;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "md5" => Ok(HashAlgorithm::Md5),
            "sha256" | "sha-256" => Ok(HashAlgorithm::Sha256),
            "blake3" | "blake3-256" => Ok(HashAlgorithm::Blake3),
            "crc32" | "crc-32" => Ok(HashAlgorithm::Crc32),
            _ => Err(BinfiddleError::InvalidInput(format!(
                "Unknown hash algorithm: '{}'. Valid: md5, sha256, blake3, crc32",
                s
            ))),
        }
    }
}

/// Output encoding for the digest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HashOutputFormat {
    /// Lowercase hexadecimal
    Hex,
}

impl std::str::FromStr for HashOutputFormat {
    type Err = BinfiddleError;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "hex" => Ok(HashOutputFormat::Hex),
            _ => Err(BinfiddleError::InvalidInput(format!(
                "Unknown hash output format: '{}'. Valid: hex",
                s
            ))),
        }
    }
}

/// Configuration for hash operations.
#[derive(Debug, Clone)]
pub struct HashConfig {
    /// Hash algorithm to use
    pub algorithm: HashAlgorithm,
    /// Output encoding
    pub output_format: HashOutputFormat,
    /// Block size for block-based hashing (0 = whole file)
    pub block_size: usize,
}

impl Default for HashConfig {
    fn default() -> Self {
        Self {
            algorithm: HashAlgorithm::Sha256,
            output_format: HashOutputFormat::Hex,
            block_size: 0,
        }
    }
}

/// Hash command structure.
pub struct HashCommand {
    config: HashConfig,
}

impl HashCommand {
    /// Creates a new HashCommand with the given configuration.
    pub fn new(config: HashConfig) -> Self {
        Self { config }
    }

    /// Computes the digest of a byte slice.
    pub fn digest(&self, data: &[u8]) -> String {
        match self.config.algorithm {
            HashAlgorithm::Md5 => format!("{:x}", md5::compute(data)),
            HashAlgorithm::Sha256 => {
                use sha2::{Digest, Sha256};
                let mut hasher = Sha256::new();
                hasher.update(data);
                format!("{:x}", hasher.finalize())
            }
            HashAlgorithm::Blake3 => blake3::hash(data).to_hex().to_string(),
            HashAlgorithm::Crc32 => format!("{:08x}", crc32fast::hash(data)),
        }
    }

    /// Computes the hash over the given data and formats the result.
    ///
    /// If `block_size` is 0, the whole input is hashed once. Otherwise, the
    /// input is split into non-overlapping blocks of that size and each block
    /// is hashed separately.
    pub fn compute(&self, data: &[u8]) -> Result<String> {
        if self.config.block_size == 0 {
            return Ok(self.digest(data));
        }

        let mut output = String::new();
        for (i, chunk) in data.chunks(self.config.block_size).enumerate() {
            if i > 0 {
                output.push('\n');
            }
            let offset = i * self.config.block_size;
            output.push_str(&format!("0x{:08x}: {}", offset, self.digest(chunk)));
        }

        Ok(output)
    }
}

impl Command for HashCommand {
    fn execute(&self, _data: &mut BinaryData) -> Result<()> {
        // The CLI driver calls compute() directly with the byte slice so that
        // it can avoid a full copy for file-backed data.
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_md5_empty() {
        let cmd = HashCommand::new(HashConfig {
            algorithm: HashAlgorithm::Md5,
            ..Default::default()
        });
        assert_eq!(cmd.digest(b""), "d41d8cd98f00b204e9800998ecf8427e");
    }

    #[test]
    fn test_sha256_known_value() {
        let cmd = HashCommand::new(HashConfig {
            algorithm: HashAlgorithm::Sha256,
            ..Default::default()
        });
        assert_eq!(
            cmd.digest(b"hello"),
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }

    #[test]
    fn test_blake3_known_value() {
        let cmd = HashCommand::new(HashConfig {
            algorithm: HashAlgorithm::Blake3,
            ..Default::default()
        });
        assert_eq!(
            cmd.digest(b"hello"),
            "ea8f163db38682925e4491c5e58d4bb3506ef8c14eb78a86e908c5624a67200f"
        );
    }

    #[test]
    fn test_crc32_known_value() {
        let cmd = HashCommand::new(HashConfig {
            algorithm: HashAlgorithm::Crc32,
            ..Default::default()
        });
        // CRC32 of the bytes "123456789" is 0xcbf43926
        assert_eq!(cmd.digest(b"123456789"), "cbf43926");
    }

    #[test]
    fn test_block_hashing() {
        let cmd = HashCommand::new(HashConfig {
            algorithm: HashAlgorithm::Crc32,
            block_size: 3,
            ..Default::default()
        });
        let output = cmd.compute(b"123456789").unwrap();
        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(lines.len(), 3);
        assert!(lines[0].starts_with("0x00000000:"));
        assert!(lines[1].starts_with("0x00000003:"));
        assert!(lines[2].starts_with("0x00000006:"));
    }

    #[test]
    fn test_algorithm_parse() {
        assert_eq!(
            "sha256".parse::<HashAlgorithm>().unwrap(),
            HashAlgorithm::Sha256
        );
        assert_eq!(
            "SHA-256".parse::<HashAlgorithm>().unwrap(),
            HashAlgorithm::Sha256
        );
        assert!("unknown".parse::<HashAlgorithm>().is_err());
    }
}
