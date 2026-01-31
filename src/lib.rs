/// src/lib.rs
pub mod commands;
pub mod error;
pub mod utils;

use std::io::Read;

pub use commands::{
    parse_encoding, parse_ignore_ranges, AnalysisType, AnalyzeCommand, AnalyzeConfig,
    AnalyzeOutputFormat, BomMode, Command, ConvertCommand, ConvertConfig, DiffCommand, DiffConfig,
    DiffEntry, DiffFormat, EditCommand, EditOperation, ErrorMode, NewlineMode, PatchCommand,
    PatchConfig, PatchEntry, PatchResult, ReadCommand, SearchCommand, SearchConfig, WriteCommand,
};
pub use error::{BinfiddleError, Result};
pub use utils::parsing::SearchPattern;
pub use utils::{display, parsing};
pub use utils::{display_bytes, parse_bit_input, parse_input, parse_range};

/// Color output mode for terminal display.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorMode {
    /// Always use colors, even if output is not a terminal
    Always,
    /// Automatically detect if terminal supports colors
    Auto,
    /// Never use colors
    Never,
}

impl Default for ColorMode {
    fn default() -> Self {
        ColorMode::Auto
    }
}

/// Represents the input source for binary data
pub enum BinarySource {
    File(std::path::PathBuf),
    Stdin,
    MemoryAddress(usize),
    RawData(Vec<u8>),
}

/// Represents a chunk of data with configurable bit length
pub struct Chunk {
    data: Vec<u8>,
    bit_length: usize,
}

impl Chunk {
    pub fn new(data: Vec<u8>, bit_length: usize) -> Result<Self> {
        if bit_length == 0 || bit_length > data.len() * 8 {
            Err(BinfiddleError::InvalidChunkSize(bit_length))
        } else {
            Ok(Self { data, bit_length })
        }
    }

    pub fn get_bytes(&self) -> &[u8] {
        &self.data
    }

    pub fn get_bit_length(&self) -> usize {
        self.bit_length
    }
}

/// Main struct representing the binary data being manipulated
pub struct BinaryData {
    data: Vec<u8>,
    chunk_size: usize, // in bits
    width: usize,      // chunks per line
}

impl BinaryData {
    pub fn new(source: BinarySource, chunk_size: usize, width: usize) -> Result<Self> {
        let data = match source {
            BinarySource::File(path) => std::fs::read(path)?,
            BinarySource::Stdin => {
                let mut buf = Vec::new();
                std::io::stdin().read_to_end(&mut buf)?;
                buf
            }
            BinarySource::MemoryAddress(_addr) => {
                // Platform-specific implementation would go here
                return Err(BinfiddleError::UnsupportedOperation(
                    "Memory address access not yet implemented".to_string(),
                ));
            }
            BinarySource::RawData(data) => data,
        };

        if chunk_size == 0 || (data.len() > 0 && chunk_size > data.len() * 8) {
            return Err(BinfiddleError::InvalidChunkSize(chunk_size));
        }

        Ok(Self {
            data,
            chunk_size,
            width,
        })
    }

    pub fn get_width(&self) -> usize {
        self.width
    }

    pub fn read_range(&self, start: usize, end: Option<usize>) -> Result<Chunk> {
        let end = end.unwrap_or(self.data.len());
        if start >= self.data.len() || end > self.data.len() || start >= end {
            return Err(BinfiddleError::InvalidRange(format!(
                "Invalid range [{}, {})",
                start, end
            )));
        }

        // Calculate how many full chunks we can get from this range
        let bit_length = (end - start) * 8;
        let effective_chunk_size = if self.chunk_size > bit_length {
            bit_length // Return all available bits if chunk size is larger than available data
        } else {
            self.chunk_size
        };

        Chunk::new(self.data[start..end].to_vec(), effective_chunk_size)
    }

    pub fn write_range(&mut self, start: usize, data: &[u8]) -> Result<()> {
        if start + data.len() > self.data.len() {
            return Err(BinfiddleError::InvalidRange(
                "Write operation would exceed data bounds".to_string(),
            ));
        }

        self.data[start..start + data.len()].copy_from_slice(data);
        Ok(())
    }

    pub fn insert_data(&mut self, position: usize, data: &[u8]) -> Result<()> {
        if position > self.data.len() {
            return Err(BinfiddleError::InvalidRange(
                "Insert position out of bounds".to_string(),
            ));
        }

        self.data.splice(position..position, data.iter().cloned());
        Ok(())
    }

    pub fn remove_range(&mut self, start: usize, end: usize) -> Result<()> {
        if start >= self.data.len() || end > self.data.len() || start >= end {
            return Err(BinfiddleError::InvalidRange(format!(
                "Invalid range [{}, {})",
                start, end
            )));
        }

        self.data.drain(start..end);
        Ok(())
    }

    pub fn get_chunk_size(&self) -> usize {
        self.chunk_size
    }

    pub fn set_chunk_size(&mut self, chunk_size: usize) -> Result<()> {
        if chunk_size == 0 || (self.data.len() > 0 && chunk_size > self.data.len() * 8) {
            return Err(BinfiddleError::InvalidChunkSize(chunk_size));
        }
        self.chunk_size = chunk_size;
        Ok(())
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_binarydata_with_default_chunk_size() {
        // This is the exact scenario that was failing
        let result = BinaryData::new(BinarySource::RawData(Vec::new()), 8, 16);
        assert!(
            result.is_ok(),
            "Empty BinaryData with chunk_size=8 should be valid"
        );

        let data = result.unwrap();
        assert_eq!(data.len(), 0);
        assert_eq!(data.get_chunk_size(), 8);
        assert_eq!(data.get_width(), 16);
    }

    #[test]
    fn test_empty_binarydata_with_various_chunk_sizes() {
        // Test that empty data works with various chunk sizes
        for chunk_size in [1, 4, 8, 16, 32, 64] {
            let result = BinaryData::new(BinarySource::RawData(Vec::new()), chunk_size, 16);
            assert!(
                result.is_ok(),
                "Empty BinaryData should work with chunk_size={}",
                chunk_size
            );
        }
    }

    #[test]
    fn test_empty_binarydata_rejects_zero_chunk_size() {
        // Zero chunk size should still be rejected, even for empty data
        let result = BinaryData::new(BinarySource::RawData(Vec::new()), 0, 16);
        assert!(result.is_err(), "chunk_size=0 should always be invalid");

        match result {
            Err(BinfiddleError::InvalidChunkSize(size)) => {
                assert_eq!(size, 0);
            }
            _ => panic!("Expected InvalidChunkSize error"),
        }
    }

    #[test]
    fn test_non_empty_binarydata_chunk_size_validation() {
        // For non-empty data, chunk_size should still be limited by data size
        let data = vec![0xDE, 0xAD]; // 2 bytes = 16 bits

        // Valid: chunk_size <= 16
        assert!(BinaryData::new(BinarySource::RawData(data.clone()), 8, 16).is_ok());
        assert!(BinaryData::new(BinarySource::RawData(data.clone()), 16, 16).is_ok());

        // Invalid: chunk_size > 16
        let result = BinaryData::new(BinarySource::RawData(data.clone()), 17, 16);
        assert!(
            result.is_err(),
            "chunk_size=17 should be invalid for 2-byte data"
        );

        match result {
            Err(BinfiddleError::InvalidChunkSize(size)) => {
                assert_eq!(size, 17);
            }
            _ => panic!("Expected InvalidChunkSize error"),
        }
    }

    #[test]
    fn test_set_chunk_size_on_empty_data() {
        // Test that set_chunk_size also works on empty data
        let mut data = BinaryData::new(BinarySource::RawData(Vec::new()), 8, 16).unwrap();

        // Should be able to change chunk_size on empty data
        assert!(data.set_chunk_size(16).is_ok());
        assert_eq!(data.get_chunk_size(), 16);

        assert!(data.set_chunk_size(32).is_ok());
        assert_eq!(data.get_chunk_size(), 32);

        // Zero should still be rejected
        assert!(data.set_chunk_size(0).is_err());
    }

    #[test]
    fn test_set_chunk_size_on_non_empty_data() {
        // Test that set_chunk_size still validates against data size for non-empty data
        let mut data = BinaryData::new(
            BinarySource::RawData(vec![0xDE, 0xAD]), // 2 bytes = 16 bits
            8,
            16,
        )
        .unwrap();

        // Valid sizes
        assert!(data.set_chunk_size(8).is_ok());
        assert!(data.set_chunk_size(16).is_ok());

        // Invalid: too large
        assert!(data.set_chunk_size(17).is_err());
        assert!(data.set_chunk_size(100).is_err());
    }

    #[test]
    fn test_diff_command_scenario() {
        // Simulate the exact scenario from the diff command
        // where a dummy BinaryData is created with default settings
        let dummy_data = BinaryData::new(
            BinarySource::RawData(Vec::new()),
            8,  // Default CLI chunk_size
            16, // Default CLI width
        );

        assert!(
            dummy_data.is_ok(),
            "Diff command's dummy BinaryData creation should succeed"
        );
    }
}
