/// src/lib.rs
pub mod commands;
pub mod error;
pub mod process_memory;
pub mod utils;

use std::io::Read;

pub use commands::{
    chain::ChainExecutor, parse_encoding, parse_ignore_ranges, AnalysisType, AnalyzeCommand,
    AnalyzeConfig, AnalyzeOutputFormat, BitfieldDefinition, BomMode, Command, ConvertCommand,
    ConvertConfig, DiffCommand, DiffConfig, DiffEntry, DiffFormat, EditCommand, EditOperation,
    Endianness, ErrorMode, FieldDefinition, FieldType, NewlineMode, ParsedField, ParsedStruct,
    PatchCommand, PatchConfig, PatchEntry, PatchResult, ReadCommand, SearchCommand, SearchConfig,
    StructCommand, StructConfig, StructOutputFormat, StructTemplate, ValueOrExpression,
    WriteCommand,
};
pub use error::{BinfiddleError, Result};
pub use utils::parsing::validate_search_pattern;
pub use utils::parsing::SearchPattern;
pub use utils::{display, parsing};
pub use utils::{
    display_bytes, display_bytes_with_offset, parse_bit_input, parse_input, parse_range,
};

/// Color output mode for terminal display.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ColorMode {
    /// Always use colors, even if output is not a terminal
    Always,
    /// Automatically detect if terminal supports colors
    #[default]
    Auto,
    /// Never use colors
    Never,
}

/// Represents the input source for binary data
#[derive(Debug, Clone)]
pub enum BinarySource {
    File(std::path::PathBuf),
    Stdin,
    MemoryAddress(usize),
    /// Read memory from the current process via `/proc/self/mem`.
    ProcessSelf {
        address: u64,
        size: u64,
        fill_mode: process_memory::FillMode,
    },
    /// Read memory from another process via `/proc/<pid>/mem`.
    Process {
        pid: u32,
        address: u64,
        size: u64,
        fill_mode: process_memory::FillMode,
    },
    RawData(Vec<u8>),
}

/// Internal backing storage for [`BinaryData`].
///
/// File-backed data is mapped read-only with `memmap2`. Mutation methods lazily
/// copy the mapped contents into an owned `Vec<u8>` before modifying them.
#[derive(Debug)]
enum DataBacking {
    Owned(Vec<u8>),
    Mmap(memmap2::Mmap),
}

impl DataBacking {
    fn as_bytes(&self) -> &[u8] {
        match self {
            DataBacking::Owned(v) => v.as_slice(),
            DataBacking::Mmap(m) => m.as_ref(),
        }
    }

    fn len(&self) -> usize {
        self.as_bytes().len()
    }

    fn is_empty(&self) -> bool {
        self.as_bytes().is_empty()
    }
}

impl AsRef<[u8]> for DataBacking {
    fn as_ref(&self) -> &[u8] {
        self.as_bytes()
    }
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
    data: DataBacking,
    chunk_size: usize, // in bits
    width: usize,      // chunks per line
    source: BinarySource,
}

impl BinaryData {
    pub fn new(source: BinarySource, chunk_size: usize, width: usize) -> Result<Self> {
        let data = match &source {
            BinarySource::File(path) => {
                let file = std::fs::File::open(path)?;
                let metadata = file.metadata()?;
                if metadata.len() == 0 {
                    // memmap2 cannot map an empty file on all platforms, so keep it owned.
                    DataBacking::Owned(Vec::new())
                } else {
                    // Safety: the file is opened read-only. The mapping is only read
                    // until a mutation method is called, at which point the mapped
                    // region is copied into an owned Vec before any writes occur.
                    DataBacking::Mmap(unsafe { memmap2::Mmap::map(&file)? })
                }
            }
            BinarySource::Stdin => {
                let mut buf = Vec::new();
                std::io::stdin().read_to_end(&mut buf)?;
                DataBacking::Owned(buf)
            }
            BinarySource::MemoryAddress(_addr) => {
                // Platform-specific implementation would go here
                return Err(BinfiddleError::UnsupportedOperation(
                    "Memory address access not yet implemented".to_string(),
                ));
            }
            BinarySource::ProcessSelf {
                address,
                size,
                fill_mode,
            } => DataBacking::Owned(process_memory::read_process_memory_sparse(
                0, *address, *size, *fill_mode,
            )?),
            BinarySource::Process {
                pid,
                address,
                size,
                fill_mode,
            } => DataBacking::Owned(process_memory::read_process_memory_sparse(
                *pid, *address, *size, *fill_mode,
            )?),
            BinarySource::RawData(data) => DataBacking::Owned(data.clone()),
        };

        if chunk_size == 0 || (!data.is_empty() && chunk_size > data.len() * 8) {
            return Err(BinfiddleError::InvalidChunkSize(chunk_size));
        }

        Ok(Self {
            data,
            chunk_size,
            width,
            source,
        })
    }

    /// Returns the original source used to load this data.
    pub fn source(&self) -> &BinarySource {
        &self.source
    }

    pub fn get_width(&self) -> usize {
        self.width
    }

    pub fn read_range(&self, start: usize, end: Option<usize>) -> Result<Chunk> {
        let bytes = self.data.as_bytes();
        let end = end.unwrap_or(bytes.len());
        if start >= bytes.len() || end > bytes.len() || start >= end {
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

        Chunk::new(bytes[start..end].to_vec(), effective_chunk_size)
    }

    /// Ensures the backing storage is an owned `Vec<u8>`.
    ///
    /// This is called lazily by mutation methods so that read-only operations on
    /// file-backed data can keep using the memory-mapped region.
    fn ensure_owned(&mut self) {
        if matches!(&self.data, DataBacking::Mmap(_)) {
            let old = std::mem::replace(&mut self.data, DataBacking::Owned(Vec::new()));
            if let DataBacking::Mmap(mmap) = old {
                self.data = DataBacking::Owned(mmap.as_ref().to_vec());
            }
        }
    }

    pub fn write_range(&mut self, start: usize, data: &[u8]) -> Result<()> {
        self.ensure_owned();
        let bytes = match &mut self.data {
            DataBacking::Owned(v) => v.as_mut_slice(),
            DataBacking::Mmap(_) => unreachable!("ensure_owned just converted Mmap to Owned"),
        };

        if start + data.len() > bytes.len() {
            return Err(BinfiddleError::InvalidRange(
                "Write operation would exceed data bounds".to_string(),
            ));
        }

        bytes[start..start + data.len()].copy_from_slice(data);
        Ok(())
    }

    pub fn insert_data(&mut self, position: usize, data: &[u8]) -> Result<()> {
        self.ensure_owned();
        let vec = match &mut self.data {
            DataBacking::Owned(v) => v,
            DataBacking::Mmap(_) => unreachable!("ensure_owned just converted Mmap to Owned"),
        };

        if position > vec.len() {
            return Err(BinfiddleError::InvalidRange(
                "Insert position out of bounds".to_string(),
            ));
        }

        vec.splice(position..position, data.iter().cloned());
        Ok(())
    }

    pub fn remove_range(&mut self, start: usize, end: usize) -> Result<()> {
        self.ensure_owned();
        let vec = match &mut self.data {
            DataBacking::Owned(v) => v,
            DataBacking::Mmap(_) => unreachable!("ensure_owned just converted Mmap to Owned"),
        };

        if start >= vec.len() || end > vec.len() || start >= end {
            return Err(BinfiddleError::InvalidRange(format!(
                "Invalid range [{}, {})",
                start, end
            )));
        }

        vec.drain(start..end);
        Ok(())
    }

    pub fn get_chunk_size(&self) -> usize {
        self.chunk_size
    }

    pub fn set_chunk_size(&mut self, chunk_size: usize) -> Result<()> {
        if chunk_size == 0 || (!self.data.is_empty() && chunk_size > self.data.len() * 8) {
            return Err(BinfiddleError::InvalidChunkSize(chunk_size));
        }
        self.chunk_size = chunk_size;
        Ok(())
    }
}

impl BinaryData {
    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// Returns an immutable view of the underlying bytes.
    ///
    /// For file-backed data this is a slice into the memory-mapped region, so
    /// callers do not trigger a full copy of the file.
    pub fn as_bytes(&self) -> &[u8] {
        self.data.as_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

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

    #[test]
    fn test_read_process_self() {
        // A static with a known magic value so we can look it up in /proc/self/mem.
        static TEST_DATA: [u8; 8] = *b"BINFIDL!";
        let address = &TEST_DATA as *const _ as u64;

        let data = BinaryData::new(
            BinarySource::ProcessSelf {
                address,
                size: TEST_DATA.len() as u64,
                fill_mode: process_memory::FillMode::Error,
            },
            8,
            16,
        )
        .expect("should read /proc/self/mem");

        assert_eq!(
            data.read_range(0, Some(TEST_DATA.len()))
                .unwrap()
                .get_bytes(),
            &TEST_DATA[..]
        );
    }

    #[test]
    fn test_read_process_by_pid() {
        static TEST_DATA: [u8; 8] = *b"PIDMEM!!";
        let address = &TEST_DATA as *const _ as u64;

        let data = BinaryData::new(
            BinarySource::Process {
                pid: std::process::id(),
                address,
                size: TEST_DATA.len() as u64,
                fill_mode: process_memory::FillMode::Error,
            },
            8,
            16,
        )
        .expect("should read /proc/<pid>/mem");

        assert_eq!(
            data.read_range(0, Some(TEST_DATA.len()))
                .unwrap()
                .get_bytes(),
            &TEST_DATA[..]
        );
    }

    #[test]
    fn test_file_backed_binarydata_reads_via_mmap() {
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        tmp.write_all(b"hello mmap").unwrap();
        let path = tmp.path().to_path_buf();

        let data = BinaryData::new(BinarySource::File(path), 8, 16).unwrap();
        assert_eq!(data.len(), 10);
        assert_eq!(data.read_range(0, Some(5)).unwrap().get_bytes(), b"hello");
        assert_eq!(data.read_range(6, Some(10)).unwrap().get_bytes(), b"mmap");
    }

    #[test]
    fn test_empty_file_binarydata() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let path = tmp.path().to_path_buf();

        let data = BinaryData::new(BinarySource::File(path), 8, 16).unwrap();
        assert_eq!(data.len(), 0);
        assert!(data.is_empty());
    }

    #[test]
    fn test_file_backed_mutation_converts_to_owned() {
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        tmp.write_all(b"ABCD").unwrap();
        let path = tmp.path().to_path_buf();

        let mut data = BinaryData::new(BinarySource::File(path), 8, 16).unwrap();
        data.write_range(0, b"XYZ").unwrap();
        assert_eq!(data.read_range(0, Some(4)).unwrap().get_bytes(), b"XYZD");

        // Insert/remove also work after the conversion.
        data.insert_data(4, b"!").unwrap();
        assert_eq!(data.read_range(0, None).unwrap().get_bytes(), b"XYZD!");

        data.remove_range(3, 4).unwrap();
        assert_eq!(data.read_range(0, None).unwrap().get_bytes(), b"XYZ!");
    }
}
