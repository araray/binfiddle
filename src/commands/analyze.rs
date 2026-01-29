//! Analyze command implementation for binfiddle.
//!
//! This module provides statistical analysis capabilities for binary data,
//! including Shannon entropy calculation, byte frequency histograms, and
//! Index of Coincidence computation.
//!
//! # Analysis Types
//!
//! - **Entropy**: Measures randomness/disorder in data (0.0 = uniform, 8.0 = max random for bytes)
//! - **Histogram**: Frequency distribution of byte values
//! - **IC (Index of Coincidence)**: Useful for cryptanalysis, measures non-uniformity
//!
//! # Entropy Interpretation
//!
//! | Entropy Range | Typical Content |
//! |---------------|-----------------|
//! | 0.0 - 1.0 | Highly repetitive (null bytes, single value) |
//! | 1.0 - 4.0 | Text, code, structured data |
//! | 4.0 - 6.0 | Mixed content, some patterns |
//! | 6.0 - 7.5 | Compressed data |
//! | 7.5 - 8.0 | Encrypted or highly random data |

use super::Command;
use crate::error::{BinfiddleError, Result};
use crate::BinaryData;

/// Analysis type to perform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnalysisType {
    /// Shannon entropy calculation
    Entropy,
    /// Byte frequency histogram
    Histogram,
    /// Index of Coincidence (useful for cryptanalysis)
    IndexOfCoincidence,
}

impl AnalysisType {
    /// Parses an analysis type from a string.
    pub fn from_str(s: &str) -> Result<Self> {
        match s.to_lowercase().as_str() {
            "entropy" => Ok(AnalysisType::Entropy),
            "histogram" | "hist" => Ok(AnalysisType::Histogram),
            "ic" | "ioc" | "index-of-coincidence" => Ok(AnalysisType::IndexOfCoincidence),
            _ => Err(BinfiddleError::InvalidInput(format!(
                "Unknown analysis type: '{}'. Valid types: entropy, histogram, ic",
                s
            ))),
        }
    }
}

/// Output format for analysis results.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    /// Human-readable text format
    Human,
    /// Comma-separated values
    Csv,
    /// JSON format
    Json,
}

impl OutputFormat {
    /// Parses an output format from a string.
    pub fn from_str(s: &str) -> Result<Self> {
        match s.to_lowercase().as_str() {
            "human" | "text" => Ok(OutputFormat::Human),
            "csv" => Ok(OutputFormat::Csv),
            "json" => Ok(OutputFormat::Json),
            _ => Err(BinfiddleError::InvalidInput(format!(
                "Unknown output format: '{}'. Valid formats: human, csv, json",
                s
            ))),
        }
    }
}

/// Configuration for analysis operations.
#[derive(Debug, Clone)]
pub struct AnalyzeConfig {
    /// Type of analysis to perform
    pub analysis_type: AnalysisType,
    /// Block size for block-based analysis (0 = entire file)
    pub block_size: usize,
    /// Output format
    pub format: OutputFormat,
    /// Optional range restriction (start, end)
    pub range: Option<(usize, usize)>,
}

impl Default for AnalyzeConfig {
    fn default() -> Self {
        Self {
            analysis_type: AnalysisType::Entropy,
            block_size: 256,
            format: OutputFormat::Human,
            range: None,
        }
    }
}

/// Result of entropy analysis for a single block.
#[derive(Debug, Clone)]
pub struct EntropyResult {
    /// Block offset (0 if whole-file analysis)
    pub offset: usize,
    /// Block size in bytes
    pub size: usize,
    /// Shannon entropy value (0.0 - 8.0 for byte data)
    pub entropy: f64,
}

/// Byte frequency entry for histogram.
#[derive(Debug, Clone)]
pub struct ByteFrequency {
    /// Byte value (0-255)
    pub byte_value: u8,
    /// Occurrence count
    pub count: usize,
    /// Frequency as percentage (0.0 - 100.0)
    pub percentage: f64,
}

/// Result of Index of Coincidence analysis.
#[derive(Debug, Clone)]
pub struct IcResult {
    /// Block offset (0 if whole-file analysis)
    pub offset: usize,
    /// Block size in bytes
    pub size: usize,
    /// Index of Coincidence value
    pub ic: f64,
}

/// Analyze command structure.
pub struct AnalyzeCommand {
    config: AnalyzeConfig,
}

impl AnalyzeCommand {
    /// Creates a new AnalyzeCommand with the given configuration.
    pub fn new(config: AnalyzeConfig) -> Self {
        Self { config }
    }

    /// Calculates Shannon entropy for a byte slice.
    ///
    /// Shannon entropy formula: H(X) = -Σ P(x_i) * log2(P(x_i))
    ///
    /// # Arguments
    /// * `data` - The byte data to analyze
    ///
    /// # Returns
    /// Entropy value between 0.0 (completely uniform) and 8.0 (maximally random)
    pub fn calculate_entropy(data: &[u8]) -> f64 {
        if data.is_empty() {
            return 0.0;
        }

        // Count byte frequencies
        let mut frequencies = [0usize; 256];
        for &byte in data {
            frequencies[byte as usize] += 1;
        }

        let len = data.len() as f64;
        let mut entropy = 0.0;

        for &count in &frequencies {
            if count > 0 {
                let probability = count as f64 / len;
                entropy -= probability * probability.log2();
            }
        }

        entropy
    }

    /// Calculates byte frequency histogram for a byte slice.
    ///
    /// # Arguments
    /// * `data` - The byte data to analyze
    ///
    /// # Returns
    /// Vector of ByteFrequency entries for non-zero counts, sorted by count descending
    pub fn calculate_histogram(data: &[u8]) -> Vec<ByteFrequency> {
        if data.is_empty() {
            return Vec::new();
        }

        let mut frequencies = [0usize; 256];
        for &byte in data {
            frequencies[byte as usize] += 1;
        }

        let len = data.len() as f64;
        let mut histogram: Vec<ByteFrequency> = frequencies
            .iter()
            .enumerate()
            .filter(|(_, &count)| count > 0)
            .map(|(byte_val, &count)| ByteFrequency {
                byte_value: byte_val as u8,
                count,
                percentage: (count as f64 / len) * 100.0,
            })
            .collect();

        // Sort by count descending
        histogram.sort_by(|a, b| b.count.cmp(&a.count));

        histogram
    }

    /// Calculates full histogram including zero counts (for visualization).
    ///
    /// # Arguments
    /// * `data` - The byte data to analyze
    ///
    /// # Returns
    /// Vector of 256 ByteFrequency entries (one per byte value)
    pub fn calculate_full_histogram(data: &[u8]) -> Vec<ByteFrequency> {
        let mut frequencies = [0usize; 256];
        for &byte in data {
            frequencies[byte as usize] += 1;
        }

        let len = if data.is_empty() { 1.0 } else { data.len() as f64 };

        (0u16..256)
            .map(|byte_val| {
                let count = frequencies[byte_val as usize];
                ByteFrequency {
                    byte_value: byte_val as u8,
                    count,
                    percentage: (count as f64 / len) * 100.0,
                }
            })
            .collect()
    }

    /// Calculates Index of Coincidence for a byte slice.
    ///
    /// IC formula: IC = Σ n_i(n_i - 1) / (N(N - 1))
    /// where n_i is the count of byte value i, and N is the total length.
    ///
    /// For random data: IC ≈ 1/256 ≈ 0.0039
    /// For English text (bytes): IC ≈ 0.0667
    ///
    /// # Arguments
    /// * `data` - The byte data to analyze
    ///
    /// # Returns
    /// Index of Coincidence value
    pub fn calculate_ic(data: &[u8]) -> f64 {
        if data.len() < 2 {
            return 0.0;
        }

        let mut frequencies = [0usize; 256];
        for &byte in data {
            frequencies[byte as usize] += 1;
        }

        let n = data.len() as f64;
        let mut numerator = 0.0;

        for &count in &frequencies {
            if count > 1 {
                numerator += (count as f64) * (count as f64 - 1.0);
            }
        }

        numerator / (n * (n - 1.0))
    }

    /// Performs entropy analysis on the data.
    pub fn analyze_entropy(&self, data: &[u8]) -> Vec<EntropyResult> {
        let block_size = if self.config.block_size == 0 {
            data.len()
        } else {
            self.config.block_size
        };

        if data.is_empty() {
            return vec![EntropyResult {
                offset: 0,
                size: 0,
                entropy: 0.0,
            }];
        }

        data.chunks(block_size)
            .enumerate()
            .map(|(i, chunk)| EntropyResult {
                offset: i * block_size,
                size: chunk.len(),
                entropy: Self::calculate_entropy(chunk),
            })
            .collect()
    }

    /// Performs IC analysis on the data.
    pub fn analyze_ic(&self, data: &[u8]) -> Vec<IcResult> {
        let block_size = if self.config.block_size == 0 {
            data.len()
        } else {
            self.config.block_size
        };

        if data.is_empty() {
            return vec![IcResult {
                offset: 0,
                size: 0,
                ic: 0.0,
            }];
        }

        data.chunks(block_size)
            .enumerate()
            .map(|(i, chunk)| IcResult {
                offset: i * block_size,
                size: chunk.len(),
                ic: Self::calculate_ic(chunk),
            })
            .collect()
    }

    /// Formats entropy results according to the configured output format.
    pub fn format_entropy_results(&self, results: &[EntropyResult]) -> String {
        match self.config.format {
            OutputFormat::Human => self.format_entropy_human(results),
            OutputFormat::Csv => self.format_entropy_csv(results),
            OutputFormat::Json => self.format_entropy_json(results),
        }
    }

    fn format_entropy_human(&self, results: &[EntropyResult]) -> String {
        let mut output = String::new();

        // Summary statistics
        if results.len() > 1 {
            let entropies: Vec<f64> = results.iter().map(|r| r.entropy).collect();
            let min = entropies.iter().cloned().fold(f64::INFINITY, f64::min);
            let max = entropies.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
            let avg = entropies.iter().sum::<f64>() / entropies.len() as f64;

            output.push_str("=== Entropy Analysis ===\n");
            output.push_str(&format!("Blocks: {}\n", results.len()));
            output.push_str(&format!("Block size: {} bytes\n", self.config.block_size));
            output.push_str(&format!("Min entropy: {:.4} bits/byte\n", min));
            output.push_str(&format!("Max entropy: {:.4} bits/byte\n", max));
            output.push_str(&format!("Avg entropy: {:.4} bits/byte\n", avg));
            output.push_str("\n--- Block Details ---\n");
        } else {
            output.push_str("=== Entropy Analysis ===\n");
        }

        for result in results {
            let interpretation = interpret_entropy(result.entropy);
            if results.len() > 1 {
                output.push_str(&format!(
                    "Offset 0x{:08x}: {:.4} bits/byte ({})\n",
                    result.offset, result.entropy, interpretation
                ));
            } else {
                output.push_str(&format!("Size: {} bytes\n", result.size));
                output.push_str(&format!("Entropy: {:.4} bits/byte\n", result.entropy));
                output.push_str(&format!("Interpretation: {}\n", interpretation));
            }
        }

        output
    }

    fn format_entropy_csv(&self, results: &[EntropyResult]) -> String {
        let mut output = String::from("offset,size,entropy\n");
        for result in results {
            output.push_str(&format!(
                "{},{},{:.6}\n",
                result.offset, result.size, result.entropy
            ));
        }
        output
    }

    fn format_entropy_json(&self, results: &[EntropyResult]) -> String {
        let blocks: Vec<String> = results
            .iter()
            .map(|r| {
                format!(
                    r#"{{"offset":{},"size":{},"entropy":{:.6}}}"#,
                    r.offset, r.size, r.entropy
                )
            })
            .collect();

        if results.len() == 1 {
            blocks[0].clone()
        } else {
            format!(r#"{{"blocks":[{}]}}"#, blocks.join(","))
        }
    }

    /// Formats histogram results according to the configured output format.
    pub fn format_histogram_results(&self, histogram: &[ByteFrequency], total_bytes: usize) -> String {
        match self.config.format {
            OutputFormat::Human => self.format_histogram_human(histogram, total_bytes),
            OutputFormat::Csv => self.format_histogram_csv(histogram),
            OutputFormat::Json => self.format_histogram_json(histogram, total_bytes),
        }
    }

    fn format_histogram_human(&self, histogram: &[ByteFrequency], total_bytes: usize) -> String {
        let mut output = String::new();
        output.push_str("=== Byte Frequency Histogram ===\n");
        output.push_str(&format!("Total bytes: {}\n", total_bytes));
        output.push_str(&format!("Unique byte values: {}\n\n", histogram.len()));

        // Show top 20 most frequent bytes
        output.push_str("Top 20 most frequent bytes:\n");
        output.push_str("Byte   Hex   Count      Percentage  Bar\n");
        output.push_str("─────────────────────────────────────────\n");

        for entry in histogram.iter().take(20) {
            let bar_len = (entry.percentage / 2.0).round() as usize;
            let bar: String = "█".repeat(bar_len.min(25));
            let printable = if entry.byte_value.is_ascii_graphic() || entry.byte_value == b' ' {
                format!("'{}'", entry.byte_value as char)
            } else {
                "   ".to_string()
            };
            output.push_str(&format!(
                "{:3}  0x{:02x} {:>10}  {:>6.2}%     {}\n",
                printable, entry.byte_value, entry.count, entry.percentage, bar
            ));
        }

        output
    }

    fn format_histogram_csv(&self, histogram: &[ByteFrequency]) -> String {
        let mut output = String::from("byte_value,hex,count,percentage\n");
        for entry in histogram {
            output.push_str(&format!(
                "{},0x{:02x},{},{:.4}\n",
                entry.byte_value, entry.byte_value, entry.count, entry.percentage
            ));
        }
        output
    }

    fn format_histogram_json(&self, histogram: &[ByteFrequency], total_bytes: usize) -> String {
        let entries: Vec<String> = histogram
            .iter()
            .map(|e| {
                format!(
                    r#"{{"byte":{},"hex":"0x{:02x}","count":{},"percentage":{:.4}}}"#,
                    e.byte_value, e.byte_value, e.count, e.percentage
                )
            })
            .collect();

        format!(
            r#"{{"total_bytes":{},"unique_values":{},"frequencies":[{}]}}"#,
            total_bytes,
            histogram.len(),
            entries.join(",")
        )
    }

    /// Formats IC results according to the configured output format.
    pub fn format_ic_results(&self, results: &[IcResult]) -> String {
        match self.config.format {
            OutputFormat::Human => self.format_ic_human(results),
            OutputFormat::Csv => self.format_ic_csv(results),
            OutputFormat::Json => self.format_ic_json(results),
        }
    }

    fn format_ic_human(&self, results: &[IcResult]) -> String {
        let mut output = String::new();

        if results.len() > 1 {
            let ics: Vec<f64> = results.iter().map(|r| r.ic).collect();
            let min = ics.iter().cloned().fold(f64::INFINITY, f64::min);
            let max = ics.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
            let avg = ics.iter().sum::<f64>() / ics.len() as f64;

            output.push_str("=== Index of Coincidence Analysis ===\n");
            output.push_str(&format!("Blocks: {}\n", results.len()));
            output.push_str(&format!("Block size: {} bytes\n", self.config.block_size));
            output.push_str(&format!("Min IC: {:.6}\n", min));
            output.push_str(&format!("Max IC: {:.6}\n", max));
            output.push_str(&format!("Avg IC: {:.6}\n", avg));
            output.push_str("\nReference values:\n");
            output.push_str("  Random data:  ~0.0039 (1/256)\n");
            output.push_str("  English text: ~0.0667\n");
            output.push_str("\n--- Block Details ---\n");
        } else {
            output.push_str("=== Index of Coincidence Analysis ===\n");
        }

        for result in results {
            let interpretation = interpret_ic(result.ic);
            if results.len() > 1 {
                output.push_str(&format!(
                    "Offset 0x{:08x}: {:.6} ({})\n",
                    result.offset, result.ic, interpretation
                ));
            } else {
                output.push_str(&format!("Size: {} bytes\n", result.size));
                output.push_str(&format!("IC: {:.6}\n", result.ic));
                output.push_str(&format!("Interpretation: {}\n", interpretation));
                output.push_str("\nReference values:\n");
                output.push_str("  Random data:  ~0.0039 (1/256)\n");
                output.push_str("  English text: ~0.0667\n");
            }
        }

        output
    }

    fn format_ic_csv(&self, results: &[IcResult]) -> String {
        let mut output = String::from("offset,size,ic\n");
        for result in results {
            output.push_str(&format!(
                "{},{},{:.8}\n",
                result.offset, result.size, result.ic
            ));
        }
        output
    }

    fn format_ic_json(&self, results: &[IcResult]) -> String {
        let blocks: Vec<String> = results
            .iter()
            .map(|r| {
                format!(
                    r#"{{"offset":{},"size":{},"ic":{:.8}}}"#,
                    r.offset, r.size, r.ic
                )
            })
            .collect();

        if results.len() == 1 {
            blocks[0].clone()
        } else {
            format!(r#"{{"blocks":[{}]}}"#, blocks.join(","))
        }
    }

    /// Executes the analysis and returns formatted output.
    pub fn analyze(&self, data: &[u8]) -> Result<String> {
        // Apply range restriction if specified
        let data = if let Some((start, end)) = self.config.range {
            if start >= data.len() || end > data.len() || start >= end {
                return Err(BinfiddleError::InvalidRange(format!(
                    "Invalid range [{}, {}) for data of length {}",
                    start, end, data.len()
                )));
            }
            &data[start..end]
        } else {
            data
        };

        match self.config.analysis_type {
            AnalysisType::Entropy => {
                let results = self.analyze_entropy(data);
                Ok(self.format_entropy_results(&results))
            }
            AnalysisType::Histogram => {
                let histogram = Self::calculate_histogram(data);
                Ok(self.format_histogram_results(&histogram, data.len()))
            }
            AnalysisType::IndexOfCoincidence => {
                let results = self.analyze_ic(data);
                Ok(self.format_ic_results(&results))
            }
        }
    }
}

/// Interprets an entropy value and returns a human-readable description.
fn interpret_entropy(entropy: f64) -> &'static str {
    if entropy < 1.0 {
        "highly repetitive/uniform"
    } else if entropy < 4.0 {
        "structured data/text/code"
    } else if entropy < 6.0 {
        "mixed content"
    } else if entropy < 7.5 {
        "likely compressed"
    } else {
        "encrypted or random"
    }
}

/// Interprets an IC value and returns a human-readable description.
fn interpret_ic(ic: f64) -> &'static str {
    if ic < 0.006 {
        "random/encrypted"
    } else if ic < 0.02 {
        "possibly compressed"
    } else if ic < 0.05 {
        "structured binary"
    } else {
        "text-like patterns"
    }
}

impl Command for AnalyzeCommand {
    fn execute(&self, data: &mut BinaryData) -> Result<()> {
        let chunk = data.read_range(0, None)?;
        let bytes = chunk.get_bytes();

        let output = self.analyze(bytes)?;
        println!("{}", output);

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_entropy_uniform_data() {
        // All same bytes -> entropy should be 0
        let data = vec![0x00u8; 1000];
        let entropy = AnalyzeCommand::calculate_entropy(&data);
        assert!((entropy - 0.0).abs() < 0.001);
    }

    #[test]
    fn test_entropy_two_values() {
        // Equal distribution of two values -> entropy = 1.0
        let mut data = vec![0x00u8; 500];
        data.extend(vec![0xFFu8; 500]);
        let entropy = AnalyzeCommand::calculate_entropy(&data);
        assert!((entropy - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_entropy_random_data() {
        // All 256 values equally distributed -> entropy = 8.0
        let mut data = Vec::with_capacity(256 * 100);
        for _ in 0..100 {
            for byte in 0u8..=255 {
                data.push(byte);
            }
        }
        let entropy = AnalyzeCommand::calculate_entropy(&data);
        assert!((entropy - 8.0).abs() < 0.001);
    }

    #[test]
    fn test_entropy_empty_data() {
        let data: Vec<u8> = Vec::new();
        let entropy = AnalyzeCommand::calculate_entropy(&data);
        assert_eq!(entropy, 0.0);
    }

    #[test]
    fn test_histogram_basic() {
        let data = vec![0x00, 0x00, 0x00, 0x01, 0x01, 0xFF];
        let histogram = AnalyzeCommand::calculate_histogram(&data);

        assert_eq!(histogram.len(), 3); // 3 unique values
        assert_eq!(histogram[0].byte_value, 0x00); // Most frequent first
        assert_eq!(histogram[0].count, 3);
        assert_eq!(histogram[1].byte_value, 0x01);
        assert_eq!(histogram[1].count, 2);
        assert_eq!(histogram[2].byte_value, 0xFF);
        assert_eq!(histogram[2].count, 1);
    }

    #[test]
    fn test_histogram_percentages() {
        let data = vec![0x00, 0x00, 0x01, 0x01];
        let histogram = AnalyzeCommand::calculate_histogram(&data);

        assert!((histogram[0].percentage - 50.0).abs() < 0.001);
        assert!((histogram[1].percentage - 50.0).abs() < 0.001);
    }

    #[test]
    fn test_histogram_empty() {
        let data: Vec<u8> = Vec::new();
        let histogram = AnalyzeCommand::calculate_histogram(&data);
        assert!(histogram.is_empty());
    }

    #[test]
    fn test_ic_uniform_data() {
        // All same bytes -> IC = 1.0 (maximum)
        let data = vec![0x00u8; 1000];
        let ic = AnalyzeCommand::calculate_ic(&data);
        assert!((ic - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_ic_random_data() {
        // Perfectly distributed -> IC ≈ 1/256 ≈ 0.0039
        let mut data = Vec::with_capacity(256 * 100);
        for _ in 0..100 {
            for byte in 0u8..=255 {
                data.push(byte);
            }
        }
        let ic = AnalyzeCommand::calculate_ic(&data);
        assert!((ic - (1.0 / 256.0)).abs() < 0.001);
    }

    #[test]
    fn test_ic_empty_data() {
        let data: Vec<u8> = Vec::new();
        let ic = AnalyzeCommand::calculate_ic(&data);
        assert_eq!(ic, 0.0);
    }

    #[test]
    fn test_block_analysis() {
        // Create data with different entropy regions
        let mut data = Vec::new();
        data.extend(vec![0x00u8; 256]); // Low entropy block
        for byte in 0u8..=255 {
            // High entropy block
            data.push(byte);
        }

        let config = AnalyzeConfig {
            analysis_type: AnalysisType::Entropy,
            block_size: 256,
            format: OutputFormat::Human,
            range: None,
        };
        let cmd = AnalyzeCommand::new(config);
        let results = cmd.analyze_entropy(&data);

        assert_eq!(results.len(), 2);
        assert!(results[0].entropy < 1.0); // First block is uniform
        assert!(results[1].entropy > 7.0); // Second block is high entropy
    }

    #[test]
    fn test_csv_output_format() {
        let data = vec![0x00u8; 256];
        let config = AnalyzeConfig {
            analysis_type: AnalysisType::Entropy,
            block_size: 0, // Whole file
            format: OutputFormat::Csv,
            range: None,
        };
        let cmd = AnalyzeCommand::new(config);
        let output = cmd.analyze(&data).unwrap();

        assert!(output.starts_with("offset,size,entropy\n"));
        assert!(output.contains("0,256,"));
    }

    #[test]
    fn test_json_output_format() {
        let data = vec![0x00u8; 256];
        let config = AnalyzeConfig {
            analysis_type: AnalysisType::Entropy,
            block_size: 0,
            format: OutputFormat::Json,
            range: None,
        };
        let cmd = AnalyzeCommand::new(config);
        let output = cmd.analyze(&data).unwrap();

        assert!(output.contains(r#""offset":0"#));
        assert!(output.contains(r#""size":256"#));
        assert!(output.contains(r#""entropy":"#));
    }

    #[test]
    fn test_analysis_type_parsing() {
        assert!(matches!(
            AnalysisType::from_str("entropy").unwrap(),
            AnalysisType::Entropy
        ));
        assert!(matches!(
            AnalysisType::from_str("HISTOGRAM").unwrap(),
            AnalysisType::Histogram
        ));
        assert!(matches!(
            AnalysisType::from_str("ic").unwrap(),
            AnalysisType::IndexOfCoincidence
        ));
        assert!(matches!(
            AnalysisType::from_str("ioc").unwrap(),
            AnalysisType::IndexOfCoincidence
        ));
        assert!(AnalysisType::from_str("invalid").is_err());
    }

    #[test]
    fn test_output_format_parsing() {
        assert!(matches!(
            OutputFormat::from_str("human").unwrap(),
            OutputFormat::Human
        ));
        assert!(matches!(
            OutputFormat::from_str("CSV").unwrap(),
            OutputFormat::Csv
        ));
        assert!(matches!(
            OutputFormat::from_str("json").unwrap(),
            OutputFormat::Json
        ));
        assert!(OutputFormat::from_str("xml").is_err());
    }

    #[test]
    fn test_range_restriction() {
        let data = vec![0x00, 0x00, 0xFF, 0xFF, 0xFF, 0xFF];
        let config = AnalyzeConfig {
            analysis_type: AnalysisType::Entropy,
            block_size: 0,
            format: OutputFormat::Json,
            range: Some((2, 6)), // Only analyze the 0xFF bytes
        };
        let cmd = AnalyzeCommand::new(config);
        let results = cmd.analyze_entropy(&data[2..6]);

        assert_eq!(results.len(), 1);
        assert!(results[0].entropy < 0.001); // All same value
    }

    #[test]
    fn test_interpret_entropy() {
        assert_eq!(interpret_entropy(0.5), "highly repetitive/uniform");
        assert_eq!(interpret_entropy(3.0), "structured data/text/code");
        assert_eq!(interpret_entropy(5.0), "mixed content");
        assert_eq!(interpret_entropy(7.0), "likely compressed");
        assert_eq!(interpret_entropy(7.9), "encrypted or random");
    }

    #[test]
    fn test_interpret_ic() {
        assert_eq!(interpret_ic(0.003), "random/encrypted");
        assert_eq!(interpret_ic(0.01), "possibly compressed");
        assert_eq!(interpret_ic(0.03), "structured binary");
        assert_eq!(interpret_ic(0.07), "text-like patterns");
    }
}
