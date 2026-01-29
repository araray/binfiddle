//! Search command implementation for binfiddle.
//!
//! This module provides pattern matching capabilities for binary data,
//! supporting exact byte sequences, regular expressions, and mask patterns.
//!
//! # Performance
//!
//! For large files (>1MB), the search operations can use parallel processing
//! via rayon to improve throughput on multi-core systems.
/// src/commands/search.rs
use super::Command;
use crate::error::{BinfiddleError, Result};
use crate::utils::display::{format_match, format_match_with_context, format_match_colored, format_match_with_context_colored};
use crate::utils::parsing::SearchPattern;
use crate::{BinaryData, ColorMode};

use memchr::memmem;
use rayon::prelude::*;
use regex::bytes::Regex;

/// Minimum file size (in bytes) to trigger parallel search.
/// Below this threshold, sequential search is typically faster due to parallelization overhead.
const PARALLEL_THRESHOLD: usize = 1024 * 1024; // 1 MB

/// Chunk size for parallel search operations.
/// Each worker processes chunks of this size.
const PARALLEL_CHUNK_SIZE: usize = 256 * 1024; // 256 KB

/// Configuration for search operations.
#[derive(Debug, Clone)]
pub struct SearchConfig {
    /// The parsed search pattern
    pub pattern: SearchPattern,
    /// Output format (hex, dec, oct, bin, ascii)
    pub format: String,
    /// Chunk size for display
    pub chunk_size: usize,
    /// Find all matches (vs. first only)
    pub find_all: bool,
    /// Only output match count
    pub count_only: bool,
    /// Only output offsets (no data)
    pub offsets_only: bool,
    /// Context bytes to show before/after match
    pub context: usize,
    /// Prevent overlapping matches
    pub no_overlap: bool,
    /// Color output mode
    pub color: ColorMode,
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            pattern: SearchPattern::Exact(vec![]),
            format: "hex".to_string(),
            chunk_size: 8,
            find_all: false,
            count_only: false,
            offsets_only: false,
            context: 0,
            no_overlap: false,
            color: ColorMode::Auto,
        }
    }
}

/// Represents a search match result.
#[derive(Debug, Clone)]
pub struct SearchMatch {
    /// Byte offset where the match was found
    pub offset: usize,
    /// The matched bytes
    pub data: Vec<u8>,
}

/// Search command structure.
pub struct SearchCommand {
    config: SearchConfig,
}

impl SearchCommand {
    /// Creates a new SearchCommand with the given configuration.
    pub fn new(config: SearchConfig) -> Self {
        Self { config }
    }

    /// Performs the search operation on the given data.
    ///
    /// # Arguments
    /// * `data` - The binary data to search (borrowed immutably for read)
    ///
    /// # Returns
    /// A vector of SearchMatch results.
    pub fn search(&self, data: &[u8]) -> Result<Vec<SearchMatch>> {
        match &self.config.pattern {
            SearchPattern::Exact(needle) => self.search_exact(data, needle),
            SearchPattern::Regex(pattern) => self.search_regex(data, pattern),
            SearchPattern::Mask(mask) => self.search_mask(data, mask),
        }
    }

    /// Performs exact byte sequence search using memchr for efficiency.
    fn search_exact(&self, haystack: &[u8], needle: &[u8]) -> Result<Vec<SearchMatch>> {
        if needle.is_empty() {
            return Err(BinfiddleError::InvalidInput(
                "Search pattern cannot be empty".to_string(),
            ));
        }

        let finder = memmem::Finder::new(needle);
        let mut matches = Vec::new();
        let mut search_start = 0;

        while search_start < haystack.len() {
            match finder.find(&haystack[search_start..]) {
                Some(relative_offset) => {
                    let absolute_offset = search_start + relative_offset;
                    matches.push(SearchMatch {
                        offset: absolute_offset,
                        data: needle.to_vec(),
                    });

                    if !self.config.find_all {
                        break;
                    }

                    // Advance search position
                    search_start = if self.config.no_overlap {
                        absolute_offset + needle.len()
                    } else {
                        absolute_offset + 1
                    };
                }
                None => break,
            }
        }

        Ok(matches)
    }

    /// Performs regex-based search using the regex crate's bytes module.
    fn search_regex(&self, haystack: &[u8], pattern: &str) -> Result<Vec<SearchMatch>> {
        let regex = Regex::new(pattern)
            .map_err(|e| BinfiddleError::Parse(format!("Invalid regex pattern: {}", e)))?;

        let mut matches = Vec::new();
        let mut search_start = 0;

        for mat in regex.find_iter(haystack) {
            // Skip if we're using no_overlap and this overlaps with previous
            if self.config.no_overlap && mat.start() < search_start {
                continue;
            }

            matches.push(SearchMatch {
                offset: mat.start(),
                data: mat.as_bytes().to_vec(),
            });

            if !self.config.find_all {
                break;
            }

            if self.config.no_overlap {
                search_start = mat.end();
            }
        }

        Ok(matches)
    }

    /// Performs mask-based search with wildcard support.
    fn search_mask(&self, haystack: &[u8], mask: &[Option<u8>]) -> Result<Vec<SearchMatch>> {
        if mask.is_empty() {
            return Err(BinfiddleError::InvalidInput(
                "Mask pattern cannot be empty".to_string(),
            ));
        }

        let mask_len = mask.len();
        let mut matches = Vec::new();

        if haystack.len() < mask_len {
            return Ok(matches);
        }

        let mut pos = 0;
        while pos <= haystack.len() - mask_len {
            if self.matches_mask(&haystack[pos..pos + mask_len], mask) {
                matches.push(SearchMatch {
                    offset: pos,
                    data: haystack[pos..pos + mask_len].to_vec(),
                });

                if !self.config.find_all {
                    break;
                }

                pos = if self.config.no_overlap {
                    pos + mask_len
                } else {
                    pos + 1
                };
            } else {
                pos += 1;
            }
        }

        Ok(matches)
    }

    /// Checks if a byte slice matches a mask pattern.
    fn matches_mask(&self, data: &[u8], mask: &[Option<u8>]) -> bool {
        if data.len() != mask.len() {
            return false;
        }

        for (byte, mask_byte) in data.iter().zip(mask.iter()) {
            match mask_byte {
                Some(expected) if byte != expected => return false,
                _ => {} // None = wildcard, matches anything
            }
        }

        true
    }

    /// Performs parallel search on large data sets.
    ///
    /// This method automatically uses parallel processing for data larger than
    /// `PARALLEL_THRESHOLD` (1MB). For smaller data, it falls back to sequential search.
    ///
    /// # Arguments
    /// * `data` - The binary data to search
    ///
    /// # Returns
    /// A vector of SearchMatch results, sorted by offset.
    pub fn search_parallel(&self, data: &[u8]) -> Result<Vec<SearchMatch>> {
        // Fall back to sequential for small data or when not finding all matches
        if data.len() < PARALLEL_THRESHOLD || !self.config.find_all {
            return self.search(data);
        }

        match &self.config.pattern {
            SearchPattern::Exact(needle) => self.search_exact_parallel(data, needle),
            SearchPattern::Mask(mask) => self.search_mask_parallel(data, mask),
            // Regex doesn't parallelize well due to internal state
            SearchPattern::Regex(pattern) => self.search_regex(data, pattern),
        }
    }

    /// Parallel exact byte sequence search.
    ///
    /// Splits the data into chunks and searches each chunk in parallel.
    /// Handles boundary matches by including overlap regions between chunks.
    fn search_exact_parallel(&self, haystack: &[u8], needle: &[u8]) -> Result<Vec<SearchMatch>> {
        if needle.is_empty() {
            return Err(BinfiddleError::InvalidInput(
                "Search pattern cannot be empty".to_string(),
            ));
        }

        let needle_len = needle.len();
        let overlap = needle_len.saturating_sub(1);

        // Create chunk boundaries with overlap to catch matches spanning chunks
        let chunks: Vec<(usize, usize)> = (0..haystack.len())
            .step_by(PARALLEL_CHUNK_SIZE)
            .map(|start| {
                let end = (start + PARALLEL_CHUNK_SIZE + overlap).min(haystack.len());
                (start, end)
            })
            .collect();

        // Search chunks in parallel
        let finder = memmem::Finder::new(needle);
        let all_matches: Vec<Vec<SearchMatch>> = chunks
            .par_iter()
            .map(|(chunk_start, chunk_end)| {
                let chunk = &haystack[*chunk_start..*chunk_end];
                let mut chunk_matches = Vec::new();
                let mut search_start = 0;

                while search_start < chunk.len() {
                    match finder.find(&chunk[search_start..]) {
                        Some(relative_offset) => {
                            let absolute_offset = chunk_start + search_start + relative_offset;

                            // Only include if this match starts within our primary chunk region
                            // (not in the overlap region that belongs to the next chunk)
                            let primary_end = (*chunk_start + PARALLEL_CHUNK_SIZE).min(haystack.len());
                            if absolute_offset < primary_end || *chunk_end == haystack.len() {
                                chunk_matches.push(SearchMatch {
                                    offset: absolute_offset,
                                    data: needle.to_vec(),
                                });
                            }

                            search_start = if self.config.no_overlap {
                                search_start + relative_offset + needle_len
                            } else {
                                search_start + relative_offset + 1
                            };
                        }
                        None => break,
                    }
                }

                chunk_matches
            })
            .collect();

        // Merge and sort results
        let mut matches: Vec<SearchMatch> = all_matches.into_iter().flatten().collect();
        matches.sort_by_key(|m| m.offset);

        // Remove duplicates (from overlap regions)
        matches.dedup_by_key(|m| m.offset);

        // Apply no_overlap filter if needed (post-merge)
        if self.config.no_overlap {
            let mut filtered = Vec::with_capacity(matches.len());
            let mut last_end = 0usize;
            for m in matches {
                if m.offset >= last_end {
                    last_end = m.offset + m.data.len();
                    filtered.push(m);
                }
            }
            matches = filtered;
        }

        Ok(matches)
    }

    /// Parallel mask-based search with wildcard support.
    fn search_mask_parallel(&self, haystack: &[u8], mask: &[Option<u8>]) -> Result<Vec<SearchMatch>> {
        if mask.is_empty() {
            return Err(BinfiddleError::InvalidInput(
                "Mask pattern cannot be empty".to_string(),
            ));
        }

        let mask_len = mask.len();
        if haystack.len() < mask_len {
            return Ok(Vec::new());
        }

        let overlap = mask_len.saturating_sub(1);

        // Create chunk boundaries with overlap
        let chunks: Vec<(usize, usize)> = (0..haystack.len())
            .step_by(PARALLEL_CHUNK_SIZE)
            .map(|start| {
                let end = (start + PARALLEL_CHUNK_SIZE + overlap).min(haystack.len());
                (start, end)
            })
            .collect();

        // Search chunks in parallel
        let all_matches: Vec<Vec<SearchMatch>> = chunks
            .par_iter()
            .map(|(chunk_start, chunk_end)| {
                let chunk = &haystack[*chunk_start..*chunk_end];
                let mut chunk_matches = Vec::new();

                if chunk.len() < mask_len {
                    return chunk_matches;
                }

                let mut pos = 0;
                while pos <= chunk.len() - mask_len {
                    if self.matches_mask(&chunk[pos..pos + mask_len], mask) {
                        let absolute_offset = chunk_start + pos;

                        // Only include if within primary chunk region
                        let primary_end = (*chunk_start + PARALLEL_CHUNK_SIZE).min(haystack.len());
                        if absolute_offset < primary_end || *chunk_end == haystack.len() {
                            chunk_matches.push(SearchMatch {
                                offset: absolute_offset,
                                data: chunk[pos..pos + mask_len].to_vec(),
                            });
                        }

                        pos = if self.config.no_overlap {
                            pos + mask_len
                        } else {
                            pos + 1
                        };
                    } else {
                        pos += 1;
                    }
                }

                chunk_matches
            })
            .collect();

        // Merge and sort results
        let mut matches: Vec<SearchMatch> = all_matches.into_iter().flatten().collect();
        matches.sort_by_key(|m| m.offset);
        matches.dedup_by_key(|m| m.offset);

        // Apply no_overlap filter if needed
        if self.config.no_overlap {
            let mut filtered = Vec::with_capacity(matches.len());
            let mut last_end = 0usize;
            for m in matches {
                if m.offset >= last_end {
                    last_end = m.offset + m.data.len();
                    filtered.push(m);
                }
            }
            matches = filtered;
        }

        Ok(matches)
    }

    /// Formats and outputs search results.
    pub fn format_results(&self, data: &[u8], matches: &[SearchMatch]) -> Result<String> {
        if self.config.count_only {
            return Ok(format!("{}", matches.len()));
        }

        // Determine if we should use color
        let use_color = match self.config.color {
            ColorMode::Always => true,
            ColorMode::Never => false,
            ColorMode::Auto => atty::is(atty::Stream::Stdout),
        };

        let mut output = String::new();

        for (i, m) in matches.iter().enumerate() {
            if i > 0 {
                output.push('\n');
            }

            if self.config.offsets_only {
                output.push_str(&format!("0x{:08x}", m.offset));
            } else if self.config.context > 0 {
                // Extract context bytes
                let before_start = m.offset.saturating_sub(self.config.context);
                let after_end = (m.offset + m.data.len() + self.config.context).min(data.len());

                let before = &data[before_start..m.offset];
                let after = &data[m.offset + m.data.len()..after_end];

                let formatted = if use_color {
                    format_match_with_context_colored(
                        m.offset,
                        &m.data,
                        before,
                        after,
                        &self.config.format,
                        self.config.chunk_size,
                    )?
                } else {
                    format_match_with_context(
                        m.offset,
                        &m.data,
                        before,
                        after,
                        &self.config.format,
                        self.config.chunk_size,
                    )?
                };
                output.push_str(&formatted);
            } else {
                let formatted = if use_color {
                    format_match_colored(
                        m.offset,
                        &m.data,
                        &self.config.format,
                        self.config.chunk_size,
                    )?
                } else {
                    format_match(
                        m.offset,
                        &m.data,
                        &self.config.format,
                        self.config.chunk_size,
                    )?
                };
                output.push_str(&formatted);
            }
        }

        Ok(output)
    }
}

impl Command for SearchCommand {
    fn execute(&self, data: &mut BinaryData) -> Result<()> {
        // Read all data for searching
        let chunk = data.read_range(0, None)?;
        let bytes = chunk.get_bytes();

        // Perform search
        let matches = self.search(bytes)?;

        // Report results
        if matches.is_empty() {
            eprintln!("No matches found");
        } else {
            let output = self.format_results(bytes, &matches)?;
            println!("{}", output);
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_config(pattern: SearchPattern) -> SearchConfig {
        SearchConfig {
            pattern,
            format: "hex".to_string(),
            chunk_size: 8,
            find_all: true,
            count_only: false,
            offsets_only: false,
            context: 0,
            no_overlap: false,
            color: ColorMode::Never, // Disable color in tests for predictable output
        }
    }

    #[test]
    fn test_exact_search_single_match() {
        let config = make_config(SearchPattern::Exact(vec![0xBE, 0xEF]));
        let cmd = SearchCommand::new(config);
        let data = vec![0xDE, 0xAD, 0xBE, 0xEF, 0xCA, 0xFE];

        let matches = cmd.search(&data).unwrap();
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].offset, 2);
        assert_eq!(matches[0].data, vec![0xBE, 0xEF]);
    }

    #[test]
    fn test_exact_search_multiple_matches() {
        let config = make_config(SearchPattern::Exact(vec![0x00]));
        let cmd = SearchCommand::new(config);
        let data = vec![0x00, 0x01, 0x00, 0x02, 0x00];

        let matches = cmd.search(&data).unwrap();
        assert_eq!(matches.len(), 3);
        assert_eq!(matches[0].offset, 0);
        assert_eq!(matches[1].offset, 2);
        assert_eq!(matches[2].offset, 4);
    }

    #[test]
    fn test_exact_search_no_overlap() {
        let mut config = make_config(SearchPattern::Exact(vec![0x00, 0x00]));
        config.no_overlap = true;
        let cmd = SearchCommand::new(config);
        let data = vec![0x00, 0x00, 0x00, 0x00];

        let matches = cmd.search(&data).unwrap();
        assert_eq!(matches.len(), 2); // [0,1] and [2,3], not [1,2]
        assert_eq!(matches[0].offset, 0);
        assert_eq!(matches[1].offset, 2);
    }

    #[test]
    fn test_regex_search() {
        // Use ASCII pattern for reliable testing
        // Pattern: "AB" followed by any byte
        let config = make_config(SearchPattern::Regex(r"AB.".to_string()));
        let cmd = SearchCommand::new(config);
        let data = b"ABCDEF_ABXYZ".to_vec();

        let matches = cmd.search(&data).unwrap();
        assert_eq!(matches.len(), 2);
        assert_eq!(matches[0].offset, 0);
        assert_eq!(matches[0].data, b"ABC".to_vec());
        assert_eq!(matches[1].offset, 7);
        assert_eq!(matches[1].data, b"ABX".to_vec());
    }

    #[test]
    fn test_mask_search() {
        let config = make_config(SearchPattern::Mask(vec![
            Some(0xDE),
            None, // wildcard
            Some(0xBE),
            None, // wildcard
        ]));
        let cmd = SearchCommand::new(config);
        let data = vec![0x00, 0xDE, 0xAD, 0xBE, 0xEF, 0x00];

        let matches = cmd.search(&data).unwrap();
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].offset, 1);
        assert_eq!(matches[0].data, vec![0xDE, 0xAD, 0xBE, 0xEF]);
    }

    #[test]
    fn test_mask_search_multiple() {
        let config = make_config(SearchPattern::Mask(vec![None, Some(0x00)]));
        let cmd = SearchCommand::new(config);
        let data = vec![0xAA, 0x00, 0xBB, 0x00, 0xCC, 0x00];

        let matches = cmd.search(&data).unwrap();
        assert_eq!(matches.len(), 3);
    }

    #[test]
    fn test_empty_pattern_error() {
        let config = make_config(SearchPattern::Exact(vec![]));
        let cmd = SearchCommand::new(config);
        let data = vec![0x00, 0x01, 0x02];

        assert!(cmd.search(&data).is_err());
    }

    #[test]
    fn test_count_only() {
        let mut config = make_config(SearchPattern::Exact(vec![0x00]));
        config.count_only = true;
        let cmd = SearchCommand::new(config);
        let data = vec![0x00, 0x01, 0x00, 0x02, 0x00];

        let matches = cmd.search(&data).unwrap();
        let output = cmd.format_results(&data, &matches).unwrap();
        assert_eq!(output, "3");
    }

    #[test]
    fn test_offsets_only() {
        let mut config = make_config(SearchPattern::Exact(vec![0x00]));
        config.offsets_only = true;
        let cmd = SearchCommand::new(config);
        let data = vec![0x00, 0x01, 0x00];

        let matches = cmd.search(&data).unwrap();
        let output = cmd.format_results(&data, &matches).unwrap();
        assert!(output.contains("0x00000000"));
        assert!(output.contains("0x00000002"));
    }

    #[test]
    fn test_colored_output() {
        let mut config = make_config(SearchPattern::Exact(vec![0xBE, 0xEF]));
        config.color = ColorMode::Always;
        let cmd = SearchCommand::new(config);
        let data = vec![0xDE, 0xAD, 0xBE, 0xEF, 0xCA, 0xFE];

        let matches = cmd.search(&data).unwrap();
        let output = cmd.format_results(&data, &matches).unwrap();
        
        // Should contain ANSI escape codes when color is Always
        assert!(output.contains("\x1b["), "Colored output should contain ANSI codes");
        // Should still contain the actual data
        assert!(output.contains("be ef"), "Output should contain match data");
    }

    #[test]
    fn test_colored_output_with_context() {
        let mut config = make_config(SearchPattern::Exact(vec![0xBE, 0xEF]));
        config.color = ColorMode::Always;
        config.context = 2;
        let cmd = SearchCommand::new(config);
        let data = vec![0xDE, 0xAD, 0xBE, 0xEF, 0xCA, 0xFE];

        let matches = cmd.search(&data).unwrap();
        let output = cmd.format_results(&data, &matches).unwrap();
        
        // Should contain ANSI escape codes
        assert!(output.contains("\x1b["), "Colored context output should contain ANSI codes");
        // Should contain context labels
        assert!(output.contains("Before:"), "Should show before context");
        assert!(output.contains("Match:"), "Should show match");
        assert!(output.contains("After:"), "Should show after context");
    }

    #[test]
    fn test_no_color_output() {
        let mut config = make_config(SearchPattern::Exact(vec![0xBE, 0xEF]));
        config.color = ColorMode::Never;
        let cmd = SearchCommand::new(config);
        let data = vec![0xDE, 0xAD, 0xBE, 0xEF, 0xCA, 0xFE];

        let matches = cmd.search(&data).unwrap();
        let output = cmd.format_results(&data, &matches).unwrap();
        
        // Should NOT contain ANSI escape codes when color is Never
        assert!(!output.contains("\x1b["), "No-color output should not contain ANSI codes");
        // Should still contain the actual data
        assert!(output.contains("be ef"), "Output should contain match data");
    }

    // ===== Parallel Search Tests =====

    #[test]
    fn test_parallel_exact_search_small_data() {
        // Small data should still work with parallel interface (falls back to sequential)
        let config = make_config(SearchPattern::Exact(vec![0xDE, 0xAD]));
        let cmd = SearchCommand::new(config);
        let data = vec![0xDE, 0xAD, 0xBE, 0xEF, 0xDE, 0xAD];

        let matches = cmd.search_parallel(&data).unwrap();
        assert_eq!(matches.len(), 2);
        assert_eq!(matches[0].offset, 0);
        assert_eq!(matches[1].offset, 4);
    }

    #[test]
    fn test_parallel_exact_search_large_data() {
        // Create data larger than PARALLEL_THRESHOLD
        let mut data = vec![0x00u8; 2 * 1024 * 1024]; // 2MB
        // Insert pattern at known locations
        let pattern = [0xDE, 0xAD, 0xBE, 0xEF];
        for offset in [0, 100_000, 500_000, 1_000_000, 1_500_000, 2_097_148] {
            if offset + pattern.len() <= data.len() {
                data[offset..offset + pattern.len()].copy_from_slice(&pattern);
            }
        }

        let config = make_config(SearchPattern::Exact(pattern.to_vec()));
        let cmd = SearchCommand::new(config);

        let matches = cmd.search_parallel(&data).unwrap();
        assert_eq!(matches.len(), 6);
        assert_eq!(matches[0].offset, 0);
        assert_eq!(matches[1].offset, 100_000);
        assert_eq!(matches[2].offset, 500_000);
        assert_eq!(matches[3].offset, 1_000_000);
        assert_eq!(matches[4].offset, 1_500_000);
        assert_eq!(matches[5].offset, 2_097_148);
    }

    #[test]
    fn test_parallel_mask_search_large_data() {
        // Create data larger than PARALLEL_THRESHOLD
        let mut data = vec![0x00u8; 2 * 1024 * 1024]; // 2MB
        // Insert pattern at known locations
        let pattern = [0xDE, 0xAD, 0xBE, 0xEF];
        for offset in [0, 256_000, 512_000, 1_024_000] {
            if offset + pattern.len() <= data.len() {
                data[offset..offset + pattern.len()].copy_from_slice(&pattern);
            }
        }

        // Mask: DE ?? BE EF
        let mask = vec![Some(0xDE), None, Some(0xBE), Some(0xEF)];
        let config = make_config(SearchPattern::Mask(mask));
        let cmd = SearchCommand::new(config);

        let matches = cmd.search_parallel(&data).unwrap();
        assert_eq!(matches.len(), 4);
        assert_eq!(matches[0].offset, 0);
        assert_eq!(matches[1].offset, 256_000);
        assert_eq!(matches[2].offset, 512_000);
        assert_eq!(matches[3].offset, 1_024_000);
    }

    #[test]
    fn test_parallel_boundary_match() {
        // Test pattern that spans chunk boundaries
        // PARALLEL_CHUNK_SIZE is 256KB, so put pattern at boundary
        let chunk_boundary = 256 * 1024;
        let mut data = vec![0x00u8; 2 * 1024 * 1024]; // 2MB
        
        // Pattern spanning the chunk boundary
        let pattern = [0xDE, 0xAD, 0xBE, 0xEF];
        let boundary_offset = chunk_boundary - 2; // Pattern starts 2 bytes before boundary
        data[boundary_offset..boundary_offset + pattern.len()].copy_from_slice(&pattern);

        let config = make_config(SearchPattern::Exact(pattern.to_vec()));
        let cmd = SearchCommand::new(config);

        let matches = cmd.search_parallel(&data).unwrap();
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].offset, boundary_offset);
    }

    #[test]
    fn test_parallel_no_overlap() {
        // Test no_overlap with parallel search
        let mut data = vec![0x00u8; 2 * 1024 * 1024]; // 2MB
        // Create overlapping patterns: 00 00 00 00 at multiple locations
        let overlapping_start = 1_000_000;
        for i in 0..8 {
            data[overlapping_start + i] = 0x00;
        }

        let mut config = make_config(SearchPattern::Exact(vec![0x00, 0x00]));
        config.no_overlap = true;
        let cmd = SearchCommand::new(config);

        let matches = cmd.search_parallel(&data).unwrap();
        
        // With no_overlap, consecutive 00 00 patterns should not overlap
        // Check that matches don't overlap
        for i in 1..matches.len() {
            assert!(
                matches[i].offset >= matches[i-1].offset + matches[i-1].data.len(),
                "Matches should not overlap"
            );
        }
    }

    #[test]
    fn test_parallel_consistency_with_sequential() {
        // Verify parallel and sequential produce same results
        let mut data = vec![0x00u8; 2 * 1024 * 1024]; // 2MB
        let pattern = [0xCA, 0xFE, 0xBA, 0xBE];
        
        // Insert patterns at various locations
        for i in 0..100 {
            let offset = i * 20_000;
            if offset + pattern.len() <= data.len() {
                data[offset..offset + pattern.len()].copy_from_slice(&pattern);
            }
        }

        let config = make_config(SearchPattern::Exact(pattern.to_vec()));
        let cmd = SearchCommand::new(config);

        let sequential = cmd.search(&data).unwrap();
        let parallel = cmd.search_parallel(&data).unwrap();

        assert_eq!(sequential.len(), parallel.len(), "Match count should be equal");
        for (seq, par) in sequential.iter().zip(parallel.iter()) {
            assert_eq!(seq.offset, par.offset, "Offsets should match");
            assert_eq!(seq.data, par.data, "Data should match");
        }
    }
}
