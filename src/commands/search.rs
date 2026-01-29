//! Search command implementation for binfiddle.
//!
//! This module provides pattern matching capabilities for binary data,
//! supporting exact byte sequences, regular expressions, and mask patterns.

use super::Command;
use crate::error::{BinfiddleError, Result};
use crate::utils::display::{format_match, format_match_with_context};
use crate::utils::parsing::SearchPattern;
use crate::BinaryData;

use memchr::memmem;
use regex::bytes::Regex;

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

    /// Formats and outputs search results.
    pub fn format_results(&self, data: &[u8], matches: &[SearchMatch]) -> Result<String> {
        if self.config.count_only {
            return Ok(format!("{}", matches.len()));
        }

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

                let formatted = format_match_with_context(
                    m.offset,
                    &m.data,
                    before,
                    after,
                    &self.config.format,
                    self.config.chunk_size,
                )?;
                output.push_str(&formatted);
            } else {
                let formatted = format_match(
                    m.offset,
                    &m.data,
                    &self.config.format,
                    self.config.chunk_size,
                )?;
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
}
