//! Binary diff command for comparing two binary files.
//!
//! This module provides functionality to compare two binary files and
//! display their differences in various formats.
//!
//! # Output Formats
//!
//! * `simple` - One line per difference: `Offset: 0xXX != 0xYY`
//! * `unified` - Unified diff format with context, like text diffs
//! * `side-by-side` - Two-column comparison view
//! * `patch` - Machine-readable format for the `patch` command
//!
//! # Examples
//!
//! ```bash
//! # Simple diff showing byte differences
//! binfiddle diff file1.bin file2.bin --format simple
//!
//! # Unified diff with 3 lines of context
//! binfiddle diff file1.bin file2.bin --format unified --context 3
//!
//! # Generate a patch file
//! binfiddle diff original.bin modified.bin --format patch > changes.patch
//! ```

use crate::error::{BinfiddleError, Result};
use crate::ColorMode;

// ANSI color codes for terminal output
const ANSI_RESET: &str = "\x1b[0m";
const ANSI_BOLD_RED: &str = "\x1b[1;31m";
const ANSI_BOLD_GREEN: &str = "\x1b[1;32m";
const ANSI_BOLD_YELLOW: &str = "\x1b[1;33m";
const ANSI_CYAN: &str = "\x1b[36m";
const ANSI_DIM: &str = "\x1b[2m";
const ANSI_MAGENTA: &str = "\x1b[35m";

/// Output format for diff results.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DiffFormat {
    /// Simple format: one line per difference showing offset and differing bytes
    #[default]
    Simple,
    /// Unified format: similar to text unified diff, with context lines
    Unified,
    /// Side-by-side format: two column view comparing both files
    SideBySide,
    /// Patch format: machine-readable format for applying patches
    Patch,
    /// Summary format: statistical overview without byte-level details
    Summary,
}

impl DiffFormat {
    /// Parse a format string into a DiffFormat enum.
    ///
    /// # Arguments
    /// * `s` - Format string to parse
    ///
    /// # Returns
    /// The corresponding DiffFormat variant.
    ///
    /// # Errors
    /// Returns `InvalidInput` if the format string is not recognized.
    pub fn from_str(s: &str) -> Result<Self> {
        match s.to_lowercase().as_str() {
            "simple" => Ok(DiffFormat::Simple),
            "unified" => Ok(DiffFormat::Unified),
            "side-by-side" | "sidebyside" | "side" => Ok(DiffFormat::SideBySide),
            "patch" => Ok(DiffFormat::Patch),
            "summary" => Ok(DiffFormat::Summary),
            "auto" => Ok(DiffFormat::Simple), // Placeholder - actual selection happens in main.rs
            _ => Err(BinfiddleError::InvalidInput(format!(
                "Unknown diff format: '{}'. Supported: simple, unified, side-by-side, patch, summary, auto",
                s
            ))),
        }
    }

    /// Automatically selects the best format based on difference density.
    ///
    /// # Arguments
    /// * `total_diffs` - Number of differences found
    /// * `file_size` - Size of the larger file
    ///
    /// # Returns
    /// The recommended format for the given difference density.
    pub fn auto_select(total_diffs: usize, file_size: usize) -> Self {
        if file_size == 0 || total_diffs == 0 {
            return DiffFormat::Simple;
        }

        let diff_ratio = total_diffs as f64 / file_size as f64;

        match diff_ratio {
            r if r < 0.01 => DiffFormat::Simple,  // <1% different: show all
            r if r < 0.50 => DiffFormat::Unified, // <50%: grouped hunks
            _ => DiffFormat::Summary,             // >=50%: summary only
        }
    }
}

/// Represents a single difference between two files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffEntry {
    /// Byte offset where the difference occurs
    pub offset: usize,
    /// Byte value in the first file (None if offset is past EOF)
    pub byte1: Option<u8>,
    /// Byte value in the second file (None if offset is past EOF)
    pub byte2: Option<u8>,
}

impl DiffEntry {
    /// Creates a new diff entry.
    pub fn new(offset: usize, byte1: Option<u8>, byte2: Option<u8>) -> Self {
        Self {
            offset,
            byte1,
            byte2,
        }
    }

    /// Returns true if this represents a byte that only exists in file1.
    pub fn is_deletion(&self) -> bool {
        self.byte1.is_some() && self.byte2.is_none()
    }

    /// Returns true if this represents a byte that only exists in file2.
    pub fn is_addition(&self) -> bool {
        self.byte1.is_none() && self.byte2.is_some()
    }

    /// Returns true if this represents a byte that differs between files.
    pub fn is_change(&self) -> bool {
        self.byte1.is_some() && self.byte2.is_some()
    }
}

/// Configuration for the diff command.
#[derive(Debug, Clone)]
pub struct DiffConfig {
    /// Output format for differences
    pub format: DiffFormat,
    /// Number of context bytes/lines to show (for unified format)
    pub context: usize,
    /// Color output mode
    pub color: ColorMode,
    /// Offset ranges to ignore during comparison
    pub ignore_ranges: Vec<(usize, usize)>,
    /// Bytes per line in output (for unified and side-by-side)
    pub width: usize,
}

impl Default for DiffConfig {
    fn default() -> Self {
        Self {
            format: DiffFormat::Simple,
            context: 3,
            color: ColorMode::Auto,
            ignore_ranges: Vec::new(),
            width: 16,
        }
    }
}

/// The diff command implementation.
#[derive(Debug)]
pub struct DiffCommand {
    config: DiffConfig,
}

impl DiffCommand {
    /// Creates a new DiffCommand with the given configuration.
    pub fn new(config: DiffConfig) -> Self {
        Self { config }
    }

    /// Compares two byte slices and returns a list of differences.
    ///
    /// # Arguments
    /// * `data1` - First file's data
    /// * `data2` - Second file's data
    ///
    /// # Returns
    /// A vector of DiffEntry structs representing each difference.
    pub fn compare(&self, data1: &[u8], data2: &[u8]) -> Vec<DiffEntry> {
        let max_len = data1.len().max(data2.len());
        let mut differences = Vec::new();

        for offset in 0..max_len {
            // Skip ignored ranges
            if self.is_ignored(offset) {
                continue;
            }

            let byte1 = data1.get(offset).copied();
            let byte2 = data2.get(offset).copied();

            if byte1 != byte2 {
                differences.push(DiffEntry::new(offset, byte1, byte2));
            }
        }

        differences
    }

    /// Checks if an offset falls within any ignored range.
    fn is_ignored(&self, offset: usize) -> bool {
        self.config
            .ignore_ranges
            .iter()
            .any(|(start, end)| offset >= *start && offset < *end)
    }

    /// Formats the diff results according to the configured format.
    ///
    /// # Arguments
    /// * `data1` - First file's data
    /// * `data2` - Second file's data
    /// * `differences` - List of differences found
    /// * `file1_name` - Name/path of first file (for headers)
    /// * `file2_name` - Name/path of second file (for headers)
    ///
    /// # Returns
    /// A formatted string representation of the differences.
    pub fn format_diff(
        &self,
        data1: &[u8],
        data2: &[u8],
        differences: &[DiffEntry],
        file1_name: &str,
        file2_name: &str,
    ) -> Result<String> {
        let use_color = self.should_use_color();

        match self.config.format {
            DiffFormat::Simple => self.format_simple(differences, use_color),
            DiffFormat::Unified => {
                self.format_unified(data1, data2, differences, file1_name, file2_name, use_color)
            }
            DiffFormat::SideBySide => self.format_side_by_side(
                data1,
                data2,
                differences,
                file1_name,
                file2_name,
                use_color,
            ),
            DiffFormat::Patch => self.format_patch(differences, file1_name, file2_name),
            DiffFormat::Summary => {
                self.format_summary(data1, data2, differences, file1_name, file2_name)
            }
        }
    }

    /// Determines if color output should be used.
    fn should_use_color(&self) -> bool {
        match self.config.color {
            ColorMode::Always => true,
            ColorMode::Never => false,
            ColorMode::Auto => atty::is(atty::Stream::Stdout),
        }
    }

    /// Formats differences in simple format.
    ///
    /// Output: `0x00000100: 0xDE != 0xAD`
    fn format_simple(&self, differences: &[DiffEntry], use_color: bool) -> Result<String> {
        let mut output = String::new();

        for diff in differences {
            let line = if use_color {
                self.format_simple_entry_colored(diff)
            } else {
                self.format_simple_entry(diff)
            };
            output.push_str(&line);
            output.push('\n');
        }

        // Remove trailing newline
        if output.ends_with('\n') {
            output.pop();
        }

        Ok(output)
    }

    /// Formats a single diff entry in simple format (no color).
    fn format_simple_entry(&self, diff: &DiffEntry) -> String {
        let byte1_str = diff
            .byte1
            .map(|b| format!("0x{:02x}", b))
            .unwrap_or_else(|| "EOF".to_string());
        let byte2_str = diff
            .byte2
            .map(|b| format!("0x{:02x}", b))
            .unwrap_or_else(|| "EOF".to_string());

        format!("0x{:08x}: {} != {}", diff.offset, byte1_str, byte2_str)
    }

    /// Formats a single diff entry in simple format with color.
    fn format_simple_entry_colored(&self, diff: &DiffEntry) -> String {
        let byte1_str = diff
            .byte1
            .map(|b| format!("{}0x{:02x}{}", ANSI_BOLD_RED, b, ANSI_RESET))
            .unwrap_or_else(|| format!("{}EOF{}", ANSI_DIM, ANSI_RESET));
        let byte2_str = diff
            .byte2
            .map(|b| format!("{}0x{:02x}{}", ANSI_BOLD_GREEN, b, ANSI_RESET))
            .unwrap_or_else(|| format!("{}EOF{}", ANSI_DIM, ANSI_RESET));

        format!(
            "{}0x{:08x}{}: {} != {}",
            ANSI_CYAN, diff.offset, ANSI_RESET, byte1_str, byte2_str
        )
    }

    /// Formats differences in unified diff format.
    ///
    /// Shows context around each difference, similar to `diff -u` for text.
    fn format_unified(
        &self,
        data1: &[u8],
        data2: &[u8],
        differences: &[DiffEntry],
        file1_name: &str,
        file2_name: &str,
        use_color: bool,
    ) -> Result<String> {
        if differences.is_empty() {
            return Ok(String::new());
        }

        let mut output = String::new();

        // Header
        if use_color {
            output.push_str(&format!(
                "{}--- {}{}\n",
                ANSI_BOLD_RED, file1_name, ANSI_RESET
            ));
            output.push_str(&format!(
                "{}+++ {}{}\n",
                ANSI_BOLD_GREEN, file2_name, ANSI_RESET
            ));
        } else {
            output.push_str(&format!("--- {}\n", file1_name));
            output.push_str(&format!("+++ {}\n", file2_name));
        }

        // Group consecutive differences into hunks (returns indices)
        let hunks = self.group_into_hunks(differences);

        for hunk_indices in hunks {
            // Collect the actual DiffEntry references for this hunk
            let hunk: Vec<&DiffEntry> = hunk_indices.iter().map(|&i| &differences[i]).collect();
            let hunk_output = self.format_unified_hunk(data1, data2, &hunk, use_color)?;
            output.push_str(&hunk_output);
        }

        // Remove trailing newline
        if output.ends_with('\n') {
            output.pop();
        }

        Ok(output)
    }

    /// Groups differences into hunks based on context overlap.
    /// Returns indices into the differences slice.
    /// Groups differences into hunks with smart gap merging.
    ///
    /// Uses adaptive gap thresholds based on hunk size to balance
    /// readability with context preservation:
    /// - Small gaps (<16 bytes): Always merge
    /// - Large hunks (>100 diffs): Merge up to 256 byte gaps
    /// - Default: Merge if <64 bytes apart
    fn group_into_hunks(&self, differences: &[DiffEntry]) -> Vec<Vec<usize>> {
        if differences.is_empty() {
            return Vec::new();
        }

        let mut hunks: Vec<Vec<usize>> = Vec::new();
        let mut current_hunk: Vec<usize> = Vec::new();

        for (idx, diff) in differences.iter().enumerate() {
            if current_hunk.is_empty() {
                current_hunk.push(idx);
            } else {
                let last_idx = *current_hunk.last().unwrap();
                let last_offset = differences[last_idx].offset;
                let gap = diff.offset.saturating_sub(last_offset);

                // Smart gap threshold based on hunk characteristics
                let should_merge = match (current_hunk.len(), gap) {
                    // Always merge very close differences
                    (_, g) if g <= 16 => true,
                    // For large hunks, allow bigger gaps to avoid fragmentation
                    (n, g) if n > 100 && g <= 256 => true,
                    // For medium hunks, use moderate threshold
                    (n, g) if n > 20 && g <= 128 => true,
                    // Default: merge if reasonably close
                    (_, g) if g <= 64 => true,
                    // Also respect original context-based merging
                    (_, g) if g <= 2 * self.config.context + 1 => true,
                    _ => false,
                };

                if should_merge {
                    current_hunk.push(idx);
                } else {
                    // Start a new hunk
                    hunks.push(current_hunk);
                    current_hunk = vec![idx];
                }
            }
        }

        if !current_hunk.is_empty() {
            hunks.push(current_hunk);
        }

        hunks
    }

    /// Formats a single hunk in unified format.
    fn format_unified_hunk(
        &self,
        data1: &[u8],
        data2: &[u8],
        hunk: &[&DiffEntry],
        use_color: bool,
    ) -> Result<String> {
        if hunk.is_empty() {
            return Ok(String::new());
        }

        let mut output = String::new();
        let context = self.config.context;
        let width = self.config.width;

        // Calculate hunk boundaries
        let first_offset = hunk.first().unwrap().offset;
        let last_offset = hunk.last().unwrap().offset;

        let start = first_offset.saturating_sub(context);
        let end1 = (last_offset + context + 1).min(data1.len());
        let end2 = (last_offset + context + 1).min(data2.len());
        let end = end1.max(end2);

        // Hunk header
        let hunk_header = format!(
            "@@ -0x{:x},0x{:x} +0x{:x},0x{:x} @@",
            start,
            end1.saturating_sub(start),
            start,
            end2.saturating_sub(start)
        );

        if use_color {
            output.push_str(&format!("{}{}{}\n", ANSI_MAGENTA, hunk_header, ANSI_RESET));
        } else {
            output.push_str(&hunk_header);
            output.push('\n');
        }

        // Create a set of difference offsets for quick lookup
        let diff_offsets: std::collections::HashSet<usize> =
            hunk.iter().map(|d| d.offset).collect();

        // Output lines
        let mut offset = start;
        while offset < end {
            let line_end = (offset + width).min(end);

            // Check if this line contains any differences
            let has_diff = (offset..line_end).any(|o| diff_offsets.contains(&o));

            if has_diff {
                // Output file1's view of this line
                self.format_unified_line(
                    &mut output,
                    data1,
                    offset,
                    line_end,
                    '-',
                    use_color,
                    &diff_offsets,
                )?;
                // Output file2's view of this line
                self.format_unified_line(
                    &mut output,
                    data2,
                    offset,
                    line_end,
                    '+',
                    use_color,
                    &diff_offsets,
                )?;
            } else {
                // Context line (same in both files)
                self.format_unified_line(
                    &mut output,
                    data1,
                    offset,
                    line_end,
                    ' ',
                    use_color,
                    &diff_offsets,
                )?;
            }

            offset = line_end;
        }

        Ok(output)
    }

    /// Formats a single line in unified diff format.
    fn format_unified_line(
        &self,
        output: &mut String,
        data: &[u8],
        start: usize,
        end: usize,
        marker: char,
        use_color: bool,
        diff_offsets: &std::collections::HashSet<usize>,
    ) -> Result<()> {
        // Line marker
        let marker_colored = if use_color {
            match marker {
                '-' => format!("{}{}{}", ANSI_BOLD_RED, marker, ANSI_RESET),
                '+' => format!("{}{}{}", ANSI_BOLD_GREEN, marker, ANSI_RESET),
                _ => marker.to_string(),
            }
        } else {
            marker.to_string()
        };

        output.push_str(&marker_colored);
        output.push_str(&format!("0x{:08x}: ", start));

        // Hex bytes
        for offset in start..end {
            if offset >= data.len() {
                if use_color {
                    output.push_str(&format!("{}--{} ", ANSI_DIM, ANSI_RESET));
                } else {
                    output.push_str("-- ");
                }
            } else {
                let byte = data[offset];
                let is_diff = diff_offsets.contains(&offset);

                if use_color && is_diff {
                    let color = match marker {
                        '-' => ANSI_BOLD_RED,
                        '+' => ANSI_BOLD_GREEN,
                        _ => ANSI_BOLD_YELLOW,
                    };
                    output.push_str(&format!("{}{:02x}{} ", color, byte, ANSI_RESET));
                } else {
                    output.push_str(&format!("{:02x} ", byte));
                }
            }
        }

        // ASCII representation
        output.push_str(" |");
        for offset in start..end {
            if offset >= data.len() {
                output.push(' ');
            } else {
                let byte = data[offset];
                let ch = if byte >= 0x20 && byte <= 0x7E {
                    byte as char
                } else {
                    '.'
                };

                if use_color && diff_offsets.contains(&offset) {
                    let color = match marker {
                        '-' => ANSI_BOLD_RED,
                        '+' => ANSI_BOLD_GREEN,
                        _ => ANSI_BOLD_YELLOW,
                    };
                    output.push_str(&format!("{}{}{}", color, ch, ANSI_RESET));
                } else {
                    output.push(ch);
                }
            }
        }
        output.push_str("|\n");

        Ok(())
    }

    /// Formats differences in side-by-side format.
    fn format_side_by_side(
        &self,
        data1: &[u8],
        data2: &[u8],
        differences: &[DiffEntry],
        file1_name: &str,
        file2_name: &str,
        use_color: bool,
    ) -> Result<String> {
        if differences.is_empty() {
            return Ok(String::new());
        }

        let mut output = String::new();
        let width = self.config.width;

        // Create diff offset set for quick lookup
        let diff_offsets: std::collections::HashSet<usize> =
            differences.iter().map(|d| d.offset).collect();

        // Header
        let half_width = width * 3 + 12; // Approximate width of each side
        if use_color {
            output.push_str(&format!(
                "{}{:<width$}{} | {}{:<width$}{}\n",
                ANSI_BOLD_RED,
                file1_name,
                ANSI_RESET,
                ANSI_BOLD_GREEN,
                file2_name,
                ANSI_RESET,
                width = half_width
            ));
        } else {
            output.push_str(&format!(
                "{:<width$} | {:<width$}\n",
                file1_name,
                file2_name,
                width = half_width
            ));
        }
        output.push_str(&format!(
            "{:-<width$}-+-{:-<width$}\n",
            "",
            "",
            width = half_width
        ));

        // Group differences into display regions (returns indices)
        let hunks = self.group_into_hunks(differences);

        for hunk_indices in hunks {
            // Collect the actual DiffEntry references for this hunk
            let hunk: Vec<&DiffEntry> = hunk_indices.iter().map(|&i| &differences[i]).collect();

            let first_offset = hunk.first().unwrap().offset;
            let last_offset = hunk.last().unwrap().offset;

            let start = first_offset.saturating_sub(self.config.context);
            let end1 = (last_offset + self.config.context + 1).min(data1.len());
            let end2 = (last_offset + self.config.context + 1).min(data2.len());
            let end = end1.max(end2);

            // Align to width boundary
            let start = (start / width) * width;

            let mut offset = start;
            while offset < end {
                let line_end = (offset + width).min(end);
                let has_diff = (offset..line_end).any(|o| diff_offsets.contains(&o));

                let left = self.format_side_line(
                    data1,
                    offset,
                    line_end,
                    use_color,
                    has_diff,
                    &diff_offsets,
                    true,
                )?;
                let right = self.format_side_line(
                    data2,
                    offset,
                    line_end,
                    use_color,
                    has_diff,
                    &diff_offsets,
                    false,
                )?;

                let separator = if has_diff {
                    if use_color {
                        format!(" {}!{} ", ANSI_BOLD_YELLOW, ANSI_RESET)
                    } else {
                        " ! ".to_string()
                    }
                } else {
                    " | ".to_string()
                };

                output.push_str(&left);
                output.push_str(&separator);
                output.push_str(&right);
                output.push('\n');

                offset = line_end;
            }

            output.push('\n');
        }

        // Remove trailing newlines
        while output.ends_with('\n') {
            output.pop();
        }

        Ok(output)
    }

    /// Formats one side of a side-by-side line.
    fn format_side_line(
        &self,
        data: &[u8],
        start: usize,
        end: usize,
        use_color: bool,
        _has_diff: bool,
        diff_offsets: &std::collections::HashSet<usize>,
        is_left: bool,
    ) -> Result<String> {
        let mut output = String::new();

        // Offset
        output.push_str(&format!("0x{:08x}: ", start));

        // Hex bytes
        for offset in start..end {
            if offset >= data.len() {
                output.push_str("   ");
            } else {
                let byte = data[offset];
                let is_diff = diff_offsets.contains(&offset);

                if use_color && is_diff {
                    let color = if is_left {
                        ANSI_BOLD_RED
                    } else {
                        ANSI_BOLD_GREEN
                    };
                    output.push_str(&format!("{}{:02x}{} ", color, byte, ANSI_RESET));
                } else {
                    output.push_str(&format!("{:02x} ", byte));
                }
            }
        }

        Ok(output)
    }

    /// Formats differences in patch format.
    ///
    /// Patch format: `OFFSET:OLD_HEX:NEW_HEX`
    /// This format can be used with the `patch` command to apply changes.
    fn format_patch(
        &self,
        differences: &[DiffEntry],
        file1_name: &str,
        file2_name: &str,
    ) -> Result<String> {
        let mut output = String::new();

        // Header comment
        output.push_str(&format!("# binfiddle patch file\n"));
        output.push_str(&format!("# source: {}\n", file1_name));
        output.push_str(&format!("# target: {}\n", file2_name));
        output.push_str(&format!("# format: OFFSET:OLD_HEX:NEW_HEX\n"));
        output.push_str(&format!("# differences: {}\n", differences.len()));
        output.push_str("#\n");

        for diff in differences {
            let old_hex = diff
                .byte1
                .map(|b| format!("{:02x}", b))
                .unwrap_or_else(|| "".to_string());
            let new_hex = diff
                .byte2
                .map(|b| format!("{:02x}", b))
                .unwrap_or_else(|| "".to_string());

            output.push_str(&format!("0x{:08x}:{}:{}\n", diff.offset, old_hex, new_hex));
        }

        // Remove trailing newline
        if output.ends_with('\n') {
            output.pop();
        }

        Ok(output)
    }

    /// Returns a summary of the differences.
    pub fn summary(&self, differences: &[DiffEntry], data1_len: usize, data2_len: usize) -> String {
        let changes = differences.iter().filter(|d| d.is_change()).count();
        let deletions = differences.iter().filter(|d| d.is_deletion()).count();
        let additions = differences.iter().filter(|d| d.is_addition()).count();

        format!(
            "{} difference(s): {} changed, {} deleted, {} added (file1: {} bytes, file2: {} bytes)",
            differences.len(),
            changes,
            deletions,
            additions,
            data1_len,
            data2_len
        )
    }

    /// Formats a byte count with appropriate units.
    fn format_size(bytes: usize) -> String {
        if bytes < 1024 {
            format!("{} bytes", bytes)
        } else if bytes < 1024 * 1024 {
            format!("{:.1} KB", bytes as f64 / 1024.0)
        } else {
            format!("{:.2} MB", bytes as f64 / (1024.0 * 1024.0))
        }
    }

    /// Creates a density bar visualization.
    fn density_bar(density: f64, width: usize) -> String {
        let filled = (density * width as f64).round() as usize;
        let filled = filled.min(width);
        let mut bar = String::with_capacity(width);
        for i in 0..width {
            bar.push(if i < filled { '█' } else { '░' });
        }
        bar
    }

    /// Categorizes differences into changed, deleted, and added.
    fn categorize_differences(&self, differences: &[DiffEntry]) -> (usize, usize, usize) {
        let mut changed = 0;
        let mut deleted = 0;
        let mut added = 0;

        for diff in differences {
            if diff.is_change() {
                changed += 1;
            } else if diff.is_deletion() {
                deleted += 1;
            } else if diff.is_addition() {
                added += 1;
            }
        }

        (changed, deleted, added)
    }

    /// Formats differences as a summary with statistics.
    fn format_summary(
        &self,
        data1: &[u8],
        data2: &[u8],
        differences: &[DiffEntry],
        file1_name: &str,
        file2_name: &str,
    ) -> Result<String> {
        let mut output = String::new();

        // Header
        output.push_str("Binary Diff Summary\n");
        output.push_str("===================\n\n");

        // File information
        output.push_str(&format!(
            "File 1: {} ({})\n",
            file1_name,
            Self::format_size(data1.len())
        ));
        output.push_str(&format!(
            "File 2: {} ({})\n\n",
            file2_name,
            Self::format_size(data2.len())
        ));

        // Calculate statistics
        let (changed, deleted, added) = self.categorize_differences(differences);
        let total_diffs = differences.len();
        let max_size = data1.len().max(data2.len());

        if max_size == 0 {
            output.push_str("Both files are empty\n");
            return Ok(output);
        }

        // Overview section
        output.push_str("Overview:\n");
        output.push_str(&format!(
            "  Total differences: {:>10} bytes ({:>5.1}% of file)\n",
            total_diffs,
            (total_diffs as f64 / max_size as f64) * 100.0
        ));
        output.push_str(&format!(
            "  Changed bytes:     {:>10}       ({:>5.1}%)\n",
            changed,
            (changed as f64 / max_size as f64) * 100.0
        ));

        if deleted > 0 {
            output.push_str(&format!(
                "  Deleted bytes:     {:>10}       ({:>5.1}%) - file1 larger\n",
                deleted,
                (deleted as f64 / data1.len() as f64) * 100.0
            ));
        }

        if added > 0 {
            output.push_str(&format!(
                "  Added bytes:       {:>10}       ({:>5.1}%) - file2 larger\n",
                added,
                (added as f64 / data2.len() as f64) * 100.0
            ));
        }

        let size_diff = data2.len() as i64 - data1.len() as i64;
        if size_diff != 0 {
            let sign = if size_diff > 0 { "+" } else { "-" };
            output.push_str(&format!(
                "  File size change:  {:>10} bytes ({}{:>4.1}%)\n",
                size_diff.abs(),
                sign,
                (size_diff.abs() as f64 / data1.len() as f64) * 100.0
            ));
        }

        output.push('\n');

        // Assessment
        output.push_str("Assessment:\n");
        let diff_percent = (total_diffs as f64 / max_size as f64) * 100.0;
        if diff_percent > 80.0 {
            output.push_str("  Files are substantially different (>80% changed)\n");
            output.push_str("  Likely: Major version update, recompilation, or different builds\n");
        } else if diff_percent > 50.0 {
            output.push_str("  Files have major differences (50-80% changed)\n");
            output.push_str("  Likely: Significant refactoring or feature additions\n");
        } else if diff_percent > 10.0 {
            output.push_str("  Files have moderate differences (10-50% changed)\n");
            output.push_str("  Likely: Bug fixes, minor updates, or targeted changes\n");
        } else if diff_percent > 0.0 {
            output.push_str("  Files have minor differences (<10% changed)\n");
            output.push_str("  Likely: Patch, hotfix, or configuration change\n");
        } else {
            output.push_str("  Files are identical\n");
        }
        output.push('\n');

        // Suggestions (only if differences exist)
        if total_diffs > 0 {
            output.push_str("Suggestions:\n");
            output.push_str("  --format unified      : View grouped changes with context\n");
            output.push_str("  --format patch        : Generate machine-readable patch\n");
            output.push_str("  --format side-by-side : Two-column hex comparison\n");
        }

        Ok(output)
    }
}

/// Parse a comma-separated list of ranges to ignore.
///
/// # Arguments
/// * `ignore_str` - Comma-separated ranges like "0x0..0x10,0x100..0x200"
///
/// # Returns
/// A vector of (start, end) tuples.
pub fn parse_ignore_ranges(ignore_str: &str) -> Result<Vec<(usize, usize)>> {
    if ignore_str.is_empty() {
        return Ok(Vec::new());
    }

    let mut ranges = Vec::new();

    for part in ignore_str.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }

        let (start, end) = crate::utils::parsing::parse_range(part, usize::MAX)?;
        let end = end.unwrap_or(start + 1);
        ranges.push((start, end));
    }

    Ok(ranges)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_diff_format_parsing() {
        assert_eq!(DiffFormat::from_str("simple").unwrap(), DiffFormat::Simple);
        assert_eq!(
            DiffFormat::from_str("unified").unwrap(),
            DiffFormat::Unified
        );
        assert_eq!(
            DiffFormat::from_str("side-by-side").unwrap(),
            DiffFormat::SideBySide
        );
        assert_eq!(
            DiffFormat::from_str("sidebyside").unwrap(),
            DiffFormat::SideBySide
        );
        assert_eq!(DiffFormat::from_str("patch").unwrap(), DiffFormat::Patch);
        assert_eq!(
            DiffFormat::from_str("summary").unwrap(),
            DiffFormat::Summary
        );
        assert_eq!(DiffFormat::from_str("auto").unwrap(), DiffFormat::Simple); // Placeholder
        assert!(DiffFormat::from_str("invalid").is_err());
    }

    #[test]
    fn test_auto_select_format() {
        // Less than 1% different -> Simple
        assert_eq!(DiffFormat::auto_select(50, 10000), DiffFormat::Simple);

        // Between 1% and 50% -> Unified
        assert_eq!(DiffFormat::auto_select(3000, 10000), DiffFormat::Unified);

        // More than 50% -> Summary
        assert_eq!(DiffFormat::auto_select(8000, 10000), DiffFormat::Summary);

        // Edge cases
        assert_eq!(DiffFormat::auto_select(0, 10000), DiffFormat::Simple);

        assert_eq!(DiffFormat::auto_select(100, 0), DiffFormat::Simple);
    }

    #[test]
    fn test_format_summary() {
        let config = DiffConfig {
            format: DiffFormat::Summary,
            color: ColorMode::Never,
            ..Default::default()
        };
        let cmd = DiffCommand::new(config);

        let data1 = vec![0x00; 1000];
        let mut data2 = vec![0x00; 1000];
        // Change 80% of bytes
        for i in 0..800 {
            data2[i] = 0xFF;
        }

        let differences = cmd.compare(&data1, &data2);
        let output = cmd
            .format_summary(&data1, &data2, &differences, "file1", "file2")
            .unwrap();

        assert!(output.contains("Binary Diff Summary"));
        assert!(output.contains("file1"));
        assert!(output.contains("file2"));
        assert!(output.contains("Total differences"));
        assert!(output.contains("Assessment"));
        assert!(differences.len() == 800);
    }

    #[test]
    fn test_format_summary_identical_files() {
        let config = DiffConfig {
            format: DiffFormat::Summary,
            ..Default::default()
        };
        let cmd = DiffCommand::new(config);

        let data = vec![0xDE, 0xAD, 0xBE, 0xEF];
        let differences = cmd.compare(&data, &data);

        let output = cmd
            .format_summary(&data, &data, &differences, "file1", "file2")
            .unwrap();

        assert!(output.contains("Files are identical"));
    }

    #[test]
    fn test_compare_identical() {
        let config = DiffConfig::default();
        let cmd = DiffCommand::new(config);

        let data = vec![0xDE, 0xAD, 0xBE, 0xEF];
        let differences = cmd.compare(&data, &data);

        assert!(differences.is_empty());
    }

    #[test]
    fn test_compare_single_difference() {
        let config = DiffConfig::default();
        let cmd = DiffCommand::new(config);

        let data1 = vec![0xDE, 0xAD, 0xBE, 0xEF];
        let data2 = vec![0xDE, 0xAD, 0x00, 0xEF];
        let differences = cmd.compare(&data1, &data2);

        assert_eq!(differences.len(), 1);
        assert_eq!(differences[0].offset, 2);
        assert_eq!(differences[0].byte1, Some(0xBE));
        assert_eq!(differences[0].byte2, Some(0x00));
    }

    #[test]
    fn test_compare_multiple_differences() {
        let config = DiffConfig::default();
        let cmd = DiffCommand::new(config);

        let data1 = vec![0x00, 0x11, 0x22, 0x33];
        let data2 = vec![0xFF, 0x11, 0x22, 0xCC];
        let differences = cmd.compare(&data1, &data2);

        assert_eq!(differences.len(), 2);
        assert_eq!(differences[0].offset, 0);
        assert_eq!(differences[1].offset, 3);
    }

    #[test]
    fn test_compare_different_lengths_file1_longer() {
        let config = DiffConfig::default();
        let cmd = DiffCommand::new(config);

        let data1 = vec![0xDE, 0xAD, 0xBE, 0xEF];
        let data2 = vec![0xDE, 0xAD];
        let differences = cmd.compare(&data1, &data2);

        assert_eq!(differences.len(), 2);
        assert_eq!(differences[0].offset, 2);
        assert_eq!(differences[0].byte1, Some(0xBE));
        assert_eq!(differences[0].byte2, None);
        assert!(differences[0].is_deletion());
    }

    #[test]
    fn test_compare_different_lengths_file2_longer() {
        let config = DiffConfig::default();
        let cmd = DiffCommand::new(config);

        let data1 = vec![0xDE, 0xAD];
        let data2 = vec![0xDE, 0xAD, 0xBE, 0xEF];
        let differences = cmd.compare(&data1, &data2);

        assert_eq!(differences.len(), 2);
        assert_eq!(differences[0].offset, 2);
        assert_eq!(differences[0].byte1, None);
        assert_eq!(differences[0].byte2, Some(0xBE));
        assert!(differences[0].is_addition());
    }

    #[test]
    fn test_compare_with_ignore_range() {
        let config = DiffConfig {
            ignore_ranges: vec![(1, 3)], // Ignore bytes 1 and 2
            ..Default::default()
        };
        let cmd = DiffCommand::new(config);

        let data1 = vec![0x00, 0x11, 0x22, 0x33];
        let data2 = vec![0x00, 0xFF, 0xFF, 0x33];
        let differences = cmd.compare(&data1, &data2);

        // Bytes 1 and 2 differ but are ignored
        assert!(differences.is_empty());
    }

    #[test]
    fn test_format_simple() {
        let config = DiffConfig {
            format: DiffFormat::Simple,
            color: ColorMode::Never,
            ..Default::default()
        };
        let cmd = DiffCommand::new(config);

        let data1 = vec![0xDE, 0xAD];
        let data2 = vec![0xDE, 0x00];
        let differences = cmd.compare(&data1, &data2);

        let output = cmd
            .format_diff(&data1, &data2, &differences, "file1", "file2")
            .unwrap();

        assert!(output.contains("0x00000001:"));
        assert!(output.contains("0xad"));
        assert!(output.contains("0x00"));
        assert!(output.contains("!="));
    }

    #[test]
    fn test_format_simple_eof() {
        let config = DiffConfig {
            format: DiffFormat::Simple,
            color: ColorMode::Never,
            ..Default::default()
        };
        let cmd = DiffCommand::new(config);

        let data1 = vec![0xDE, 0xAD, 0xBE];
        let data2 = vec![0xDE, 0xAD];
        let differences = cmd.compare(&data1, &data2);

        let output = cmd
            .format_diff(&data1, &data2, &differences, "file1", "file2")
            .unwrap();

        assert!(output.contains("EOF"));
    }

    #[test]
    fn test_format_patch() {
        let config = DiffConfig {
            format: DiffFormat::Patch,
            color: ColorMode::Never,
            ..Default::default()
        };
        let cmd = DiffCommand::new(config);

        let data1 = vec![0x00, 0x11, 0x22];
        let data2 = vec![0xFF, 0x11, 0x33];
        let differences = cmd.compare(&data1, &data2);

        let output = cmd
            .format_diff(&data1, &data2, &differences, "original.bin", "modified.bin")
            .unwrap();

        assert!(output.contains("# binfiddle patch file"));
        assert!(output.contains("# source: original.bin"));
        assert!(output.contains("0x00000000:00:ff"));
        assert!(output.contains("0x00000002:22:33"));
    }

    #[test]
    fn test_format_unified_header() {
        let config = DiffConfig {
            format: DiffFormat::Unified,
            color: ColorMode::Never,
            context: 2,
            ..Default::default()
        };
        let cmd = DiffCommand::new(config);

        let data1 = vec![0x00, 0x11, 0x22, 0x33];
        let data2 = vec![0x00, 0xFF, 0x22, 0x33];
        let differences = cmd.compare(&data1, &data2);

        let output = cmd
            .format_diff(&data1, &data2, &differences, "file1.bin", "file2.bin")
            .unwrap();

        assert!(output.contains("--- file1.bin"));
        assert!(output.contains("+++ file2.bin"));
        assert!(output.contains("@@"));
    }

    #[test]
    fn test_diff_entry_types() {
        let change = DiffEntry::new(0, Some(0xAA), Some(0xBB));
        assert!(change.is_change());
        assert!(!change.is_deletion());
        assert!(!change.is_addition());

        let deletion = DiffEntry::new(1, Some(0xCC), None);
        assert!(!deletion.is_change());
        assert!(deletion.is_deletion());
        assert!(!deletion.is_addition());

        let addition = DiffEntry::new(2, None, Some(0xDD));
        assert!(!addition.is_change());
        assert!(!addition.is_deletion());
        assert!(addition.is_addition());
    }

    #[test]
    fn test_summary() {
        let config = DiffConfig::default();
        let cmd = DiffCommand::new(config);

        let differences = vec![
            DiffEntry::new(0, Some(0xAA), Some(0xBB)), // change
            DiffEntry::new(1, Some(0xCC), None),       // deletion
            DiffEntry::new(2, None, Some(0xDD)),       // addition
        ];

        let summary = cmd.summary(&differences, 100, 100);
        assert!(summary.contains("3 difference(s)"));
        assert!(summary.contains("1 changed"));
        assert!(summary.contains("1 deleted"));
        assert!(summary.contains("1 added"));
    }

    #[test]
    fn test_parse_ignore_ranges() {
        let ranges = parse_ignore_ranges("0x0..0x10,0x100..0x200").unwrap();
        assert_eq!(ranges.len(), 2);
        assert_eq!(ranges[0], (0, 16));
        assert_eq!(ranges[1], (256, 512));
    }

    #[test]
    fn test_parse_ignore_ranges_empty() {
        let ranges = parse_ignore_ranges("").unwrap();
        assert!(ranges.is_empty());
    }

    #[test]
    fn test_colored_simple_contains_ansi() {
        let config = DiffConfig {
            format: DiffFormat::Simple,
            color: ColorMode::Always,
            ..Default::default()
        };
        let cmd = DiffCommand::new(config);

        let data1 = vec![0xDE, 0xAD];
        let data2 = vec![0xDE, 0x00];
        let differences = cmd.compare(&data1, &data2);

        let output = cmd
            .format_diff(&data1, &data2, &differences, "file1", "file2")
            .unwrap();

        assert!(output.contains("\x1b["), "Should contain ANSI escape codes");
    }

    #[test]
    fn test_no_color_no_ansi() {
        let config = DiffConfig {
            format: DiffFormat::Simple,
            color: ColorMode::Never,
            ..Default::default()
        };
        let cmd = DiffCommand::new(config);

        let data1 = vec![0xDE, 0xAD];
        let data2 = vec![0xDE, 0x00];
        let differences = cmd.compare(&data1, &data2);

        let output = cmd
            .format_diff(&data1, &data2, &differences, "file1", "file2")
            .unwrap();

        assert!(
            !output.contains("\x1b["),
            "Should not contain ANSI escape codes"
        );
    }

    #[test]
    fn test_empty_files() {
        let config = DiffConfig::default();
        let cmd = DiffCommand::new(config);

        let data1: Vec<u8> = vec![];
        let data2: Vec<u8> = vec![];
        let differences = cmd.compare(&data1, &data2);

        assert!(differences.is_empty());
    }

    #[test]
    fn test_one_empty_file() {
        let config = DiffConfig::default();
        let cmd = DiffCommand::new(config);

        let data1: Vec<u8> = vec![];
        let data2 = vec![0xDE, 0xAD];
        let differences = cmd.compare(&data1, &data2);

        assert_eq!(differences.len(), 2);
        assert!(differences[0].is_addition());
        assert!(differences[1].is_addition());
    }

    #[test]
    fn test_hunk_grouping() {
        let config = DiffConfig {
            context: 2,
            ..Default::default()
        };
        let cmd = DiffCommand::new(config);

        // Differences at offset 0, 1, and 100 should create two hunks
        let differences = vec![
            DiffEntry::new(0, Some(0xAA), Some(0xBB)),
            DiffEntry::new(1, Some(0xCC), Some(0xDD)),
            DiffEntry::new(100, Some(0xEE), Some(0xFF)),
        ];

        let hunks = cmd.group_into_hunks(&differences);
        assert_eq!(hunks.len(), 2);
        assert_eq!(hunks[0].len(), 2); // First two close together (indices 0, 1)
        assert_eq!(hunks[1].len(), 1); // Last one separate (index 2)
        assert_eq!(hunks[0], vec![0, 1]);
        assert_eq!(hunks[1], vec![2]);
    }

    #[test]
    fn test_hunk_grouping_smart_gaps() {
        let config = DiffConfig::default();
        let cmd = DiffCommand::new(config);

        // Test small gaps (<16 bytes) - should always merge
        let differences = vec![
            DiffEntry::new(0, Some(0xAA), Some(0xBB)),
            DiffEntry::new(10, Some(0xCC), Some(0xDD)),
            DiffEntry::new(20, Some(0xEE), Some(0xFF)),
        ];

        let hunks = cmd.group_into_hunks(&differences);
        assert_eq!(hunks.len(), 1, "Small gaps should create single hunk");

        // Test medium gaps (20-64 bytes) - should merge
        let differences = vec![
            DiffEntry::new(0, Some(0xAA), Some(0xBB)),
            DiffEntry::new(50, Some(0xCC), Some(0xDD)),
        ];

        let hunks = cmd.group_into_hunks(&differences);
        assert_eq!(hunks.len(), 1, "Medium gaps should merge");

        // Test large gaps (>256 bytes) - should split (unless large hunk)
        let differences = vec![
            DiffEntry::new(0, Some(0xAA), Some(0xBB)),
            DiffEntry::new(500, Some(0xCC), Some(0xDD)),
        ];

        let hunks = cmd.group_into_hunks(&differences);
        assert_eq!(
            hunks.len(),
            2,
            "Large gaps should split into separate hunks"
        );
    }

    #[test]
    fn test_hunk_grouping_large_hunks() {
        let config = DiffConfig::default();
        let cmd = DiffCommand::new(config);

        // Create a large hunk (>100 differences) with 200-byte gaps
        // These should merge even though gap is large
        let mut differences = Vec::new();
        for i in 0..120 {
            differences.push(DiffEntry::new(i * 5, Some(0xAA), Some(0xBB)));
        }

        let hunks = cmd.group_into_hunks(&differences);
        // With smart grouping, this should create fewer hunks
        // than naive 2*context grouping would
        assert!(
            hunks.len() < 10,
            "Smart grouping should reduce hunk fragmentation"
        );
    }
}
