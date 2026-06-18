//! Hash command implementation for binfiddle.
//!
//! Computes common digests over binary data, either for the whole input or
//! block-by-block. Supports MD5, SHA-1, SHA-256, BLAKE3, CRC32, and xxhash64,
//! with hex or base64 output encoding. A streaming mode is available for
//! incremental hashing of files that do not fit in memory.

use super::Command;
use crate::error::{BinfiddleError, Result};
use crate::BinaryData;
use sha2::Digest;
use std::io::Read;

/// Hash algorithm to compute.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HashAlgorithm {
    /// MD5 (128-bit)
    Md5,
    /// SHA-1 (160-bit)
    Sha1,
    /// SHA-256 (256-bit)
    Sha256,
    /// BLAKE3 (256-bit)
    Blake3,
    /// CRC32/IEEE (32-bit)
    Crc32,
    /// xxhash64 (64-bit, seed 0)
    Xxhash64,
}

impl std::str::FromStr for HashAlgorithm {
    type Err = BinfiddleError;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "md5" => Ok(HashAlgorithm::Md5),
            "sha1" | "sha-1" => Ok(HashAlgorithm::Sha1),
            "sha256" | "sha-256" => Ok(HashAlgorithm::Sha256),
            "blake3" | "blake3-256" => Ok(HashAlgorithm::Blake3),
            "crc32" | "crc-32" => Ok(HashAlgorithm::Crc32),
            "xxhash64" | "xxh64" => Ok(HashAlgorithm::Xxhash64),
            _ => Err(BinfiddleError::InvalidInput(format!(
                "Unknown hash algorithm: '{}'. Valid: md5, sha1, sha256, blake3, crc32, xxhash64",
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
    /// Base64 (standard, no padding)
    Base64,
}

impl std::str::FromStr for HashOutputFormat {
    type Err = BinfiddleError;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "hex" => Ok(HashOutputFormat::Hex),
            "base64" => Ok(HashOutputFormat::Base64),
            _ => Err(BinfiddleError::InvalidInput(format!(
                "Unknown hash output format: '{}'. Valid: hex, base64",
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

/// Internal incremental hasher used by the streaming hash path.
enum IncrementalHasher {
    Md5(md5::Context),
    Sha1(sha1::Sha1),
    Sha256(sha2::Sha256),
    Blake3(Box<blake3::Hasher>),
    Crc32(crc32fast::Hasher),
    Xxhash64(xxhash_rust::xxh64::Xxh64),
}

impl IncrementalHasher {
    fn update(&mut self, data: &[u8]) {
        match self {
            IncrementalHasher::Md5(ctx) => ctx.consume(data),
            IncrementalHasher::Sha1(h) => {
                h.update(data);
            }
            IncrementalHasher::Sha256(h) => {
                h.update(data);
            }
            IncrementalHasher::Blake3(h) => {
                h.update(data);
            }

            IncrementalHasher::Crc32(h) => h.update(data),
            IncrementalHasher::Xxhash64(h) => h.update(data),
        }
    }

    fn finalize(self) -> Vec<u8> {
        match self {
            IncrementalHasher::Md5(ctx) => ctx.compute().0.to_vec(),
            IncrementalHasher::Sha1(h) => h.finalize().to_vec(),
            IncrementalHasher::Sha256(h) => h.finalize().to_vec(),
            IncrementalHasher::Blake3(h) => h.finalize().as_bytes().to_vec(),
            IncrementalHasher::Crc32(h) => h.finalize().to_be_bytes().to_vec(),
            IncrementalHasher::Xxhash64(h) => h.digest().to_be_bytes().to_vec(),
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

    fn format_bytes(&self, bytes: &[u8]) -> String {
        match self.config.output_format {
            HashOutputFormat::Hex => bytes.iter().map(|b| format!("{:02x}", b)).collect(),
            HashOutputFormat::Base64 => {
                base64::Engine::encode(&base64::engine::general_purpose::STANDARD, bytes)
            }
        }
    }

    fn new_incremental_hasher(&self) -> IncrementalHasher {
        match self.config.algorithm {
            HashAlgorithm::Md5 => IncrementalHasher::Md5(md5::Context::new()),
            HashAlgorithm::Sha1 => IncrementalHasher::Sha1(sha1::Sha1::new()),
            HashAlgorithm::Sha256 => IncrementalHasher::Sha256(sha2::Sha256::new()),
            HashAlgorithm::Blake3 => IncrementalHasher::Blake3(Box::new(blake3::Hasher::new())),
            HashAlgorithm::Crc32 => IncrementalHasher::Crc32(crc32fast::Hasher::new()),
            HashAlgorithm::Xxhash64 => {
                IncrementalHasher::Xxhash64(xxhash_rust::xxh64::Xxh64::new(0))
            }
        }
    }

    fn digest_bytes(&self, data: &[u8]) -> Vec<u8> {
        match self.config.algorithm {
            HashAlgorithm::Md5 => md5::compute(data).0.to_vec(),
            HashAlgorithm::Sha1 => {
                let mut h = sha1::Sha1::new();
                h.update(data);
                h.finalize().to_vec()
            }
            HashAlgorithm::Sha256 => {
                use sha2::{Digest, Sha256};
                let mut h = Sha256::new();
                h.update(data);
                h.finalize().to_vec()
            }
            HashAlgorithm::Blake3 => blake3::hash(data).as_bytes().to_vec(),
            HashAlgorithm::Crc32 => crc32fast::hash(data).to_be_bytes().to_vec(),
            HashAlgorithm::Xxhash64 => xxhash_rust::xxh64::xxh64(data, 0).to_be_bytes().to_vec(),
        }
    }

    /// Computes the digest of a byte slice.
    pub fn digest(&self, data: &[u8]) -> String {
        self.format_bytes(&self.digest_bytes(data))
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

    /// Streams a hash over a reader, avoiding loading the whole input.
    ///
    /// If `block_size` is 0, the input is hashed incrementally and a single
    /// digest is returned. Otherwise, each block-sized chunk is hashed
    /// independently and output with its offset.
    pub fn compute_stream<R: Read>(&self, mut reader: R, read_chunk_size: usize) -> Result<String> {
        if self.config.block_size == 0 {
            let mut hasher = self.new_incremental_hasher();
            let mut buffer = vec![0u8; read_chunk_size];
            loop {
                let n = reader.read(&mut buffer)?;
                if n == 0 {
                    break;
                }
                hasher.update(&buffer[..n]);
            }
            let digest = hasher.finalize();
            return Ok(self.format_bytes(&digest));
        }

        let mut buffer = vec![0u8; self.config.block_size];
        let mut offset = 0usize;
        let mut output = String::new();
        loop {
            let mut pos = 0usize;
            while pos < self.config.block_size {
                match reader.read(&mut buffer[pos..self.config.block_size]) {
                    Ok(0) => break,
                    Ok(n) => pos += n,
                    Err(e) => return Err(e.into()),
                }
            }
            if pos == 0 {
                break;
            }
            if !output.is_empty() {
                output.push('\n');
            }
            output.push_str(&format!(
                "0x{:08x}: {}",
                offset,
                self.format_bytes(&self.digest_bytes(&buffer[..pos]))
            ));
            offset += pos;
        }

        Ok(output)
    }
}

impl Command for HashCommand {
    fn execute(&self, _data: &mut BinaryData) -> Result<()> {
        // The CLI driver calls compute() or compute_stream() directly so that
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
    fn test_sha1_known_value() {
        let cmd = HashCommand::new(HashConfig {
            algorithm: HashAlgorithm::Sha1,
            ..Default::default()
        });
        assert_eq!(
            cmd.digest(b"hello"),
            "aaf4c61ddcc5e8a2dabede0f3b482cd9aea9434d"
        );
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
    fn test_xxhash64_known_value() {
        let cmd = HashCommand::new(HashConfig {
            algorithm: HashAlgorithm::Xxhash64,
            ..Default::default()
        });
        // xxhash64 with seed 0 of "hello" is 0x26c7827d889f6da3
        assert_eq!(cmd.digest(b"hello"), "26c7827d889f6da3");
    }

    #[test]
    fn test_base64_output() {
        let cmd = HashCommand::new(HashConfig {
            algorithm: HashAlgorithm::Md5,
            output_format: HashOutputFormat::Base64,
            ..Default::default()
        });
        assert_eq!(cmd.digest(b"hello"), "XUFAKrxLKna5cZ2REBfFkg==");
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
    fn test_stream_whole_file_matches_digest() {
        let cmd = HashCommand::new(HashConfig {
            algorithm: HashAlgorithm::Sha256,
            ..Default::default()
        });
        let data = b"hello world";
        let streamed = cmd.compute_stream(&data[..], 4).unwrap();
        assert_eq!(streamed, cmd.digest(data));
    }

    #[test]
    fn test_stream_block_hashing() {
        let cmd = HashCommand::new(HashConfig {
            algorithm: HashAlgorithm::Crc32,
            block_size: 3,
            ..Default::default()
        });
        let streamed = cmd.compute_stream(&b"123456789"[..], 5).unwrap();
        assert_eq!(streamed, cmd.compute(b"123456789").unwrap());
    }

    #[test]
    fn test_algorithm_parse() {
        assert_eq!(
            "sha256".parse::<HashAlgorithm>().unwrap(),
            HashAlgorithm::Sha256
        );
        assert_eq!(
            "xxhash64".parse::<HashAlgorithm>().unwrap(),
            HashAlgorithm::Xxhash64
        );
        assert!("unknown".parse::<HashAlgorithm>().is_err());
    }

    #[test]
    fn test_output_format_parse() {
        assert_eq!(
            "base64".parse::<HashOutputFormat>().unwrap(),
            HashOutputFormat::Base64
        );
        assert!("hex".parse::<HashOutputFormat>().is_ok());
        assert!("unknown".parse::<HashOutputFormat>().is_err());
    }
}
