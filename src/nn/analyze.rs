//! Numerical inspection over catalog tensors.
//!
//! Statistics are evidence about examined values under a declared method:
//! every result carries a coverage record stating the mode (metadata, sample,
//! full), how many elements were examined, and why the scan stopped. Finite
//! aggregates use Welford updates; norms use a scaled sum of squares so huge
//! magnitudes cannot overflow; non-finite values are counted by category and
//! never silently folded into means. Histograms declare their bin edges and
//! edge-inclusion rules. Reference comparisons report alignment and
//! zero-denominator policies instead of inventing numbers.

use super::budget::Budget;
use super::catalog::{Catalog, CatalogTensor};
use super::codec::{
    layout_for_encoding, q4_0_decode_block, ScalarCodec, ScalarValue, TensorLayout,
};
use super::error::NnError;
use super::json::Json;
use super::report::{Diagnostic, DiagnosticLevel, ResultEnvelope};
use super::source::BoundedFile;
use std::path::Path;

/// Scan chunk for scalar element streams.
const ELEMENT_CHUNK_BYTES: usize = 256 * 1024;
/// Elements buffered for histogram construction (bounded memory).
const HISTOGRAM_ELEMENT_CAP: u64 = 1_000_000;
/// Default sample size in elements.
pub const DEFAULT_SAMPLE_SIZE: u64 = 10_000;

/// Access mode for one analysis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanMode {
    /// Descriptors only; no payload bytes are read.
    Metadata,
    /// Deterministic uniform element sample (seeded).
    Sample,
    /// Full scan within the budget; partial on exhaustion.
    Full,
}

impl ScanMode {
    pub fn as_str(self) -> &'static str {
        match self {
            ScanMode::Metadata => "metadata",
            ScanMode::Sample => "sample",
            ScanMode::Full => "full",
        }
    }
}

/// Streaming statistics accumulator.
#[derive(Debug)]
pub struct StreamingStats {
    count: u64,
    mean: f64,
    m2: f64,
    nan_count: u64,
    pos_inf: u64,
    neg_inf: u64,
    pos_zero: u64,
    neg_zero: u64,
    abs_sum: f64,
    min: Option<(f64, Vec<u64>)>,
    max: Option<(f64, Vec<u64>)>,
    min_ties: u64,
    max_ties: u64,
    /// Tri-state constancy tracker. A plain `Option<f64>` cannot distinguish
    /// "no observations yet" from "proven non-constant", which would let a
    /// varying sequence re-seed as constant after every differing value.
    constant: Constancy,
    /// Scaled sum of squares: sum of (x * 2^-scale)².
    l2_scale: i32,
    l2_sum: f64,
}

/// Constancy of the observed finite population.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Constancy {
    /// No finite value observed yet.
    Empty,
    /// Every observed finite value has been this value.
    Constant(f64),
    /// At least two distinct finite values observed.
    Varies,
}

impl Default for StreamingStats {
    fn default() -> Self {
        StreamingStats {
            count: 0,
            mean: 0.0,
            m2: 0.0,
            nan_count: 0,
            pos_inf: 0,
            neg_inf: 0,
            pos_zero: 0,
            neg_zero: 0,
            abs_sum: 0.0,
            min: None,
            max: None,
            min_ties: 0,
            max_ties: 0,
            constant: Constancy::Empty,
            l2_scale: 0,
            l2_sum: 0.0,
        }
    }
}

impl StreamingStats {
    /// Current mean (only meaningful when count > 0).
    pub fn mean_value(&self) -> f64 {
        self.mean
    }

    /// Current M2 (sum of squared deviations).
    pub fn m2_value(&self) -> f64 {
        self.m2
    }

    /// Observe one decoded value at a linear coordinate.
    pub fn update(&mut self, value: f64, coordinate: &[u64]) {
        if value.is_nan() {
            self.nan_count += 1;
            return;
        }
        if value == f64::INFINITY {
            self.pos_inf += 1;
            return;
        }
        if value == f64::NEG_INFINITY {
            self.neg_inf += 1;
            return;
        }
        if value == 0.0 {
            if value.is_sign_negative() {
                self.neg_zero += 1;
            } else {
                self.pos_zero += 1;
            }
        }
        self.count += 1;
        // Welford update.
        let delta = value - self.mean;
        self.mean += delta / self.count as f64;
        self.m2 += delta * (value - self.mean);
        self.abs_sum += value.abs();
        // Constant-population tracking.
        self.constant = match self.constant {
            Constancy::Empty => Constancy::Constant(value),
            Constancy::Constant(c) if c == value => Constancy::Constant(c),
            _ => Constancy::Varies,
        };
        // Min/max with a deterministic first-occurrence tie rule.
        match &self.min {
            None => self.min = Some((value, coordinate.to_vec())),
            Some((m, _)) => {
                if value < *m {
                    self.min = Some((value, coordinate.to_vec()));
                } else if value == *m {
                    self.min_ties += 1;
                }
            }
        }
        match &self.max {
            None => self.max = Some((value, coordinate.to_vec())),
            Some((m, _)) => {
                if value > *m {
                    self.max = Some((value, coordinate.to_vec()));
                } else if value == *m {
                    self.max_ties += 1;
                }
            }
        }
        self.push_square(value);
    }

    /// Add x² to the scaled sum of squares without overflow or underflow.
    ///
    /// The running scale is the largest value exponent seen, so every scaled
    /// term lies in (0, 2] and its square in (0, 4]: the sum is bounded by
    /// 4·count for any population that fits in u64, at any magnitude.
    fn push_square(&mut self, x: f64) {
        if x == 0.0 {
            return;
        }
        // Exponent of x (frexp-style): x = m · 2^e with 0.5 ≤ |m| < 1, so e
        // is floor(log2|x|).
        let e = x.abs().log2().floor() as i32;
        if self.count == 1 || e > self.l2_scale {
            if self.l2_sum > 0.0 {
                // Rescale the existing sum to the new, larger exponent.
                let shift = e - self.l2_scale;
                self.l2_sum *= 2f64.powi(-2 * shift);
            }
            self.l2_scale = e;
        }
        let term = x * 2f64.powi(-self.l2_scale);
        self.l2_sum += term * term;
    }

    /// L2 (Frobenius) norm over observed finite values. The scale factor is
    /// applied in two halves so the reconstruction cannot overflow even when
    /// the true norm is near the floating-point limits.
    pub fn l2_norm(&self) -> Option<f64> {
        if self.count == 0 {
            return None;
        }
        let root = self.l2_sum.sqrt();
        let half_a = self.l2_scale / 2;
        let half_b = self.l2_scale - half_a;
        Some((root * 2f64.powi(half_a)) * 2f64.powi(half_b))
    }

    /// Finalized statistics.
    pub fn summary(&self) -> StatsSummary {
        StatsSummary {
            finite_count: self.count,
            nan_count: self.nan_count,
            pos_inf: self.pos_inf,
            neg_inf: self.neg_inf,
            pos_zero: self.pos_zero,
            neg_zero: self.neg_zero,
            mean: (self.count > 0).then_some(self.mean),
            population_variance: (self.count > 0).then_some(self.m2 / self.count as f64),
            sample_variance: (self.count > 1).then_some(self.m2 / (self.count - 1) as f64),
            abs_sum: (self.count > 0).then_some(self.abs_sum),
            l2_norm: self.l2_norm(),
            min: self.min.clone(),
            max: self.max.clone(),
            min_ties: self.min_ties,
            max_ties: self.max_ties,
            constant: match self.constant {
                Constancy::Constant(c) => Some(c),
                _ => None,
            },
        }
    }
}

/// Finalized statistics for a population.
#[derive(Debug, Clone, PartialEq)]
pub struct StatsSummary {
    pub finite_count: u64,
    pub nan_count: u64,
    pub pos_inf: u64,
    pub neg_inf: u64,
    pub pos_zero: u64,
    pub neg_zero: u64,
    pub mean: Option<f64>,
    pub population_variance: Option<f64>,
    pub sample_variance: Option<f64>,
    pub abs_sum: Option<f64>,
    pub l2_norm: Option<f64>,
    pub min: Option<(f64, Vec<u64>)>,
    pub max: Option<(f64, Vec<u64>)>,
    pub min_ties: u64,
    pub max_ties: u64,
    pub constant: Option<f64>,
}

/// Reference-error metrics with explicit zero-denominator policies.
#[derive(Debug, Clone, PartialEq)]
pub struct ErrorMetrics {
    pub compared: u64,
    pub mae: Option<f64>,
    pub rmse: Option<f64>,
    pub max_abs_error: Option<f64>,
    /// ||e||₂ / ||x||₂ when the reference norm is positive.
    pub relative_l2: Option<f64>,
    /// Exact equality observed when both norms are zero.
    pub exact_equality: bool,
    /// Reference norm was zero with a nonzero error: relative measures are
    /// undefined, not zero.
    pub relative_undefined: bool,
    pub nonfinite_mismatches: u64,
}

/// Compute error metrics from streamed (reference, candidate) pairs.
pub fn error_metrics(pairs: &[(f64, f64)]) -> ErrorMetrics {
    let mut stats = StreamingStats::default();
    let mut ref_stats = StreamingStats::default();
    let mut compared: u64 = 0;
    let mut nonfinite = 0u64;
    for &(x, y) in pairs {
        let x_ok = x.is_finite();
        let y_ok = y.is_finite();
        if !x_ok || !y_ok {
            if x_ok != y_ok {
                nonfinite += 1;
            }
            continue;
        }
        compared += 1;
        let e = y - x;
        stats.update(e.abs(), &[]);
        ref_stats.update(x, &[]);
    }
    let exact_equality =
        compared > 0 && stats.l2_norm() == Some(0.0) && ref_stats.l2_norm() == Some(0.0);
    let relative_undefined = compared > 0
        && ref_stats.l2_norm() == Some(0.0)
        && stats.l2_norm().is_some_and(|n| n > 0.0);
    ErrorMetrics {
        compared,
        mae: (compared > 0).then_some(stats_mean(&stats)),
        rmse: (compared > 0).then_some({
            let mean = stats_mean(&stats);
            let second_moment = stats_m2(&stats) / compared as f64 + mean * mean;
            second_moment.sqrt()
        }),
        max_abs_error: stats.max.as_ref().map(|(m, _)| *m),
        relative_l2: match (stats.l2_norm(), ref_stats.l2_norm()) {
            (Some(e), Some(r)) if r > 0.0 => Some(e / r),
            _ => None,
        },
        exact_equality,
        relative_undefined,
        nonfinite_mismatches: nonfinite,
    }
}

fn stats_mean(stats: &StreamingStats) -> f64 {
    stats.mean_value()
}

fn stats_m2(stats: &StreamingStats) -> f64 {
    stats.m2_value()
}

/// Histogram over finite values with declared edges. Bins are half-open
/// `[lower, upper)`; the final bin is closed `[lower, upper]`.
#[derive(Debug, Clone, PartialEq)]
pub struct Histogram {
    pub edges: Vec<f64>,
    pub counts: Vec<u64>,
    pub out_of_range: u64,
}

impl Histogram {
    /// Build `bins` equal-width bins spanning the observed min..max.
    pub fn from_values(values: &[f64], bins: usize) -> Histogram {
        if bins == 0 || values.is_empty() {
            return Histogram {
                edges: Vec::new(),
                counts: vec![0; bins],
                out_of_range: 0,
            };
        }
        let mut min = f64::INFINITY;
        let mut max = f64::NEG_INFINITY;
        for &v in values {
            min = min.min(v);
            max = max.max(v);
        }
        let mut edges = Vec::with_capacity(bins + 1);
        if min == max {
            // Constant population collapses to a single closed bin.
            edges.push(min);
            edges.push(max);
            let mut counts = vec![0u64; 1];
            counts[0] = values.len() as u64;
            return Histogram {
                edges,
                counts,
                out_of_range: 0,
            };
        }
        let width = (max - min) / bins as f64;
        for i in 0..=bins {
            edges.push(min + width * i as f64);
        }
        edges[bins] = max; // exact upper edge
        let mut counts = vec![0u64; bins];
        let mut out_of_range = 0u64;
        for &v in values {
            let idx_f = ((v - min) / width).floor() as i64;
            let idx = idx_f.clamp(0, bins as i64 - 1) as usize;
            // The clamped assignment keeps values within [min, max] in-range
            // by construction; the counter exists for future declared-edge
            // histograms.
            if v < min || v > max {
                out_of_range += 1;
            } else {
                counts[idx] += 1;
            }
        }
        Histogram {
            edges,
            counts,
            out_of_range,
        }
    }
}

/// Coverage record for one analysis.
#[derive(Debug, Clone)]
pub struct Coverage {
    pub mode: ScanMode,
    pub eligible_elements: u64,
    pub examined_elements: u64,
    pub fetched_bytes: u64,
    pub seed: Option<u64>,
    pub sample_size: Option<u64>,
    pub termination: &'static str, // "completed" | "budget_exhausted" | "metadata_only"
}

/// One analysis finding.
#[derive(Debug, Clone)]
pub struct Finding {
    pub code: &'static str,
    pub severity: DiagnosticLevel,
    pub message: String,
}

/// Result of one analysis.
pub struct AnalysisResult {
    pub tensor_id: String,
    pub name: String,
    pub encoding: String,
    pub shape: Vec<u64>,
    pub element_count: u64,
    pub coverage: Coverage,
    pub stats: Option<StatsSummary>,
    pub histogram: Option<Histogram>,
    pub blocks: Vec<super::codec::Q4Block>,
    pub block_range: Vec<(u64, u64)>,
    pub reference: Option<ErrorMetrics>,
    pub findings: Vec<Finding>,
    pub notes: Vec<String>,
}

/// Deterministic sample index stream: hash(seed, k).
fn sample_index(seed: u64, k: u64) -> u64 {
    // SplitMix64 over (seed, k).
    let mut z = seed
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add(k.wrapping_mul(0xBF58_476D_1CE4_E5B9));
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Seed derived from the user seed and the tensor identity (stable across
/// runs and shard orderings).
fn tensor_seed(user_seed: u64, tensor_id: &str) -> u64 {
    let mut z = user_seed;
    for byte in tensor_id.bytes() {
        z = z.wrapping_mul(31).wrapping_add(byte as u64);
    }
    sample_index(z, 0x5EED)
}

/// Run one analysis over a catalog tensor.
#[allow(clippy::too_many_arguments)]
pub fn analyze_tensor(
    catalog: &Catalog,
    tensor: &CatalogTensor,
    mode: ScanMode,
    seed: u64,
    sample_size: u64,
    histogram_bins: usize,
    block_view: u64,
    reference: Option<&Path>,
    reference_width: u32,
    budget: &Budget,
) -> Result<AnalysisResult, NnError> {
    let layout = layout_for_encoding(&tensor.encoding);
    let source = catalog.resolve_source(&tensor.source_id)?;
    if source.path.is_empty() {
        return Err(NnError::InvalidRequest {
            message: "source locator path is unavailable; re-discover with the same root"
                .to_string(),
        });
    }
    let reader = BoundedFile::open(Path::new(&source.path))?;
    reader.verify_length(Path::new(&source.path))?;

    let findings: Vec<Finding> = Vec::new();
    let notes: Vec<String> = Vec::new();

    let eligible = tensor.element_count;
    let mut result = AnalysisResult {
        tensor_id: tensor.id.clone(),
        name: tensor.original_name.clone(),
        encoding: tensor.encoding.clone(),
        shape: tensor.shape.clone(),
        element_count: eligible,
        coverage: Coverage {
            mode,
            eligible_elements: eligible,
            examined_elements: 0,
            fetched_bytes: 0,
            seed: None,
            sample_size: None,
            termination: "metadata_only",
        },
        stats: None,
        histogram: None,
        blocks: Vec::new(),
        block_range: Vec::new(),
        reference: None,
        findings,
        notes,
    };
    match mode {
        ScanMode::Metadata => {
            result.notes.push(
                "metadata mode reads no payload bytes; no numerical observations are claimed"
                    .to_string(),
            );
            return Ok(result);
        }
        ScanMode::Sample | ScanMode::Full => {}
    }

    if tensor.payload_length.is_none() {
        return Err(NnError::InvalidRequest {
            message: format!(
                "tensor {} has only a bounded extent; numerical analysis requires exact extents",
                tensor.original_name
            ),
        });
    }

    match layout {
        TensorLayout::Scalar(codec) => {
            analyze_scalar(
                &reader,
                tensor,
                codec,
                mode,
                seed,
                sample_size,
                histogram_bins,
                reference,
                reference_width,
                budget,
                &mut result,
            )?;
        }
        TensorLayout::Q4_0 => {
            analyze_q4_0(
                &reader,
                tensor,
                mode,
                seed,
                sample_size,
                block_view,
                budget,
                &mut result,
            )?;
        }
        TensorLayout::Unknown => {
            return Err(NnError::CodecUnsupported {
                codec: tensor.encoding.clone(),
                operation: "numerical analysis".to_string(),
                reason: "no qualified numeric decoder for this encoding".to_string(),
            });
        }
    }

    // Findings from the gathered statistics.
    if let Some(stats) = &result.stats {
        let nonfinite = stats.nan_count + stats.pos_inf + stats.neg_inf;
        if nonfinite > 0 {
            result.findings.push(Finding {
                code: "NONFINITE_OBSERVED",
                severity: DiagnosticLevel::Warning,
                message: format!(
                    "{nonfinite} of {} examined values are non-finite (NaN {}, +Inf {}, -Inf {})",
                    stats.finite_count + nonfinite,
                    stats.nan_count,
                    stats.pos_inf,
                    stats.neg_inf
                ),
            });
        }
        if result.coverage.mode == ScanMode::Sample && result.coverage.examined_elements > 0 {
            result.findings.push(Finding {
                code: "SAMPLE_INCOMPLETE",
                severity: DiagnosticLevel::Info,
                message: format!(
                    "observations cover {} of {} elements; statements apply to the sample only",
                    result.coverage.examined_elements, result.coverage.eligible_elements
                ),
            });
        }
        if stats.finite_count > 0 && stats.constant.is_some() {
            result.findings.push(Finding {
                code: "CONSTANT_VALUES",
                severity: DiagnosticLevel::Info,
                message: "every examined value is identical".to_string(),
            });
        }
    }
    if result.coverage.termination == "budget_exhausted" {
        result.findings.push(Finding {
            code: "SCAN_TRUNCATED",
            severity: DiagnosticLevel::Warning,
            message: "the scan stopped early because a resource budget was exhausted; coverage is partial"
                .to_string(),
        });
    }
    Ok(result)
}

#[allow(clippy::too_many_arguments)]
fn analyze_scalar(
    reader: &BoundedFile,
    tensor: &CatalogTensor,
    codec: ScalarCodec,
    mode: ScanMode,
    seed: u64,
    sample_size: u64,
    histogram_bins: usize,
    reference: Option<&Path>,
    reference_width: u32,
    budget: &Budget,
    result: &mut AnalysisResult,
) -> Result<(), NnError> {
    let width = codec.width();
    let stats = StreamingStats::default();
    let mut collector = ScanCollector::new(
        stats,
        histogram_bins,
        eligible_for_histogram(mode, tensor.element_count),
    );
    let mut reference_stream = match reference {
        Some(path) => Some(ReferenceStream::open(
            path,
            reference_width,
            tensor.element_count,
        )?),
        None => None,
    };

    match mode {
        ScanMode::Full => {
            let mut buffer = vec![0u8; ELEMENT_CHUNK_BYTES / width as usize * width as usize];
            let mut linear: u64 = 0;
            let mut remaining = tensor.element_count;
            'scan: while remaining > 0 {
                let take = ((buffer.len() as u64) / width).min(remaining) as usize;
                let byte_len = take * width as usize;
                budget.consume_source_read(byte_len as u64)?;
                reader.read_exact_at(
                    tensor.payload_start + linear * width,
                    &mut buffer[..byte_len],
                )?;
                result.coverage.fetched_bytes += byte_len as u64;
                for i in 0..take {
                    let bytes = &buffer[i * width as usize..(i + 1) * width as usize];
                    let value = match codec.decode(bytes)?.value {
                        ScalarValue::Float(f) => f,
                        ScalarValue::Int(v) => v as f64,
                        ScalarValue::Uint(v) => v as f64,
                        ScalarValue::Bool(v) => (v as u8) as f64,
                    };
                    let coordinate = crate::nn::address::coordinate_from_linear(
                        &tensor.shape,
                        linear + i as u64,
                    )
                    .unwrap_or_default();
                    let ref_value = reference_stream
                        .as_mut()
                        .map(|stream| stream.next())
                        .transpose()?;
                    collector.push(value, coordinate, ref_value, linear + i as u64);
                }
                linear += take as u64;
                remaining -= take as u64;
                result.coverage.examined_elements = collector.examined;
                if let Err(err) = budget.checkpoint() {
                    if matches!(err, NnError::BudgetExceeded { .. }) {
                        result.coverage.termination = "budget_exhausted";
                        break 'scan;
                    }
                    return Err(err);
                }
            }
            if result.coverage.termination != "budget_exhausted" {
                result.coverage.termination = "completed";
            }
        }
        ScanMode::Sample => {
            let size = sample_size.min(tensor.element_count).max(1);
            let t_seed = tensor_seed(seed, &tensor.id);
            let mut indices: Vec<u64> = (0..size)
                .map(|k| sample_index(t_seed, k) % tensor.element_count.max(1))
                .collect();
            indices.sort_unstable();
            indices.dedup();
            result.coverage.seed = Some(seed);
            result.coverage.sample_size = Some(indices.len() as u64);
            let mut element = [0u8; 16];
            for index in indices {
                budget.consume_source_read(width)?;
                reader.read_exact_at(
                    tensor.payload_start + index * width,
                    &mut element[..width as usize],
                )?;
                result.coverage.fetched_bytes += width;
                let value = match codec.decode(&element[..width as usize])?.value {
                    ScalarValue::Float(f) => f,
                    ScalarValue::Int(v) => v as f64,
                    ScalarValue::Uint(v) => v as f64,
                    ScalarValue::Bool(v) => (v as u8) as f64,
                };
                let coordinate = crate::nn::address::coordinate_from_linear(&tensor.shape, index)
                    .unwrap_or_default();
                let ref_value = reference_stream
                    .as_mut()
                    .map(|stream| stream.at(index))
                    .transpose()?;
                collector.push(value, coordinate, ref_value, index);
                if let Err(err) = budget.checkpoint() {
                    if matches!(err, NnError::BudgetExceeded { .. }) {
                        result.coverage.termination = "budget_exhausted";
                        break;
                    }
                    return Err(err);
                }
            }
            if result.coverage.termination != "budget_exhausted" {
                result.coverage.termination = "completed";
            }
        }
        ScanMode::Metadata => unreachable!("handled by the caller"),
    }

    finish_collector(collector, result);
    Ok(())
}

fn eligible_for_histogram(mode: ScanMode, element_count: u64) -> bool {
    mode == ScanMode::Sample || element_count <= HISTOGRAM_ELEMENT_CAP
}

#[allow(clippy::too_many_arguments)]
fn analyze_q4_0(
    reader: &BoundedFile,
    tensor: &CatalogTensor,
    mode: ScanMode,
    seed: u64,
    sample_size: u64,
    block_view: u64,
    budget: &Budget,
    result: &mut AnalysisResult,
) -> Result<(), NnError> {
    if tensor.shape.len() != 2 {
        return Err(NnError::InvalidRequest {
            message: "Q4_0 analysis expects a two-axis tensor".to_string(),
        });
    }
    let blocks_total = tensor.element_count / 32;
    let stats = StreamingStats::default();
    let mut collector = ScanCollector::new(stats, 0, false);
    let mut block = [0u8; 18];
    let mut block_index: u64 = 0;

    let blocks_to_scan: Vec<u64> = match mode {
        ScanMode::Full => (0..blocks_total).collect(),
        ScanMode::Sample => {
            let size = (sample_size / 32).max(1).min(blocks_total);
            let t_seed = tensor_seed(seed, &tensor.id);
            let mut indices: Vec<u64> = (0..size)
                .map(|k| sample_index(t_seed, k) % blocks_total.max(1))
                .collect();
            indices.sort_unstable();
            indices.dedup();
            result.coverage.seed = Some(seed);
            result.coverage.sample_size = Some(indices.len() as u64 * 32);
            indices
        }
        ScanMode::Metadata => unreachable!("handled by the caller"),
    };

    for scan_block in blocks_to_scan {
        block_index = scan_block;
        budget.consume_source_read(18)?;
        reader.read_exact_at(tensor.payload_start + scan_block * 18, &mut block)?;
        result.coverage.fetched_bytes += 18;
        let decoded = q4_0_decode_block(&block)?;
        if (result.blocks.len() as u64) < block_view {
            result.blocks.push(decoded.clone());
            result
                .block_range
                .push((scan_block * 32, scan_block * 32 + 31));
        }
        for (u, value) in decoded.values.iter().enumerate() {
            let linear = scan_block * 32 + u as u64;
            let coordinate = crate::nn::address::coordinate_from_linear(&tensor.shape, linear)
                .unwrap_or_default();
            collector.push(*value, coordinate, None, linear);
        }
        if let Err(err) = budget.checkpoint() {
            if matches!(err, NnError::BudgetExceeded { .. }) {
                result.coverage.termination = "budget_exhausted";
                break;
            }
            return Err(err);
        }
    }
    if result.coverage.termination != "budget_exhausted" {
        result.coverage.termination = "completed";
    }
    let _ = block_index;
    finish_collector(collector, result);
    Ok(())
}

/// Bundles the streaming accumulator with optional histogram and reference
/// pair collection.
struct ScanCollector {
    stats: StreamingStats,
    pairs: Vec<(f64, f64)>,
    values: Vec<f64>,
    collect_values: bool,
    collect_pairs: bool,
    examined: u64,
    histogram_bins: usize,
}

impl ScanCollector {
    fn new(stats: StreamingStats, histogram_bins: usize, collect_values: bool) -> ScanCollector {
        ScanCollector {
            stats,
            pairs: Vec::new(),
            values: Vec::new(),
            collect_values,
            collect_pairs: false,
            examined: 0,
            histogram_bins,
        }
    }

    fn push(&mut self, value: f64, coordinate: Vec<u64>, reference: Option<f64>, _linear: u64) {
        self.stats.update(value, &coordinate);
        self.examined += 1;
        if let Some(x) = reference {
            self.collect_pairs = true;
            self.pairs.push((x, value));
        }
        if self.collect_values && value.is_finite() {
            self.values.push(value);
        }
    }
}

fn finish_collector(collector: ScanCollector, result: &mut AnalysisResult) {
    result.coverage.examined_elements = collector.examined;
    result.stats = Some(collector.stats.summary());
    if !collector.pairs.is_empty() || collector.collect_pairs {
        result.reference = Some(error_metrics(&collector.pairs));
    } else {
        result.reference = None;
    }
    if collector.histogram_bins > 0 {
        if collector.values.is_empty() && collector.collect_values {
            result
                .notes
                .push("histogram unavailable: no finite values were collected".to_string());
        } else if !collector.collect_values {
            result
                .notes
                .push(format!("histogram unavailable: populations above {HISTOGRAM_ELEMENT_CAP} elements are not buffered for binning in this version"));
        } else if let Some(histogram) = {
            let values = &collector.values;
            (!values.is_empty()).then(|| Histogram::from_values(values, collector.histogram_bins))
        } {
            result.histogram = Some(histogram);
        }
    }
}

/// Streaming reference reader aligned by element index.
struct ReferenceStream {
    reader: BoundedFile,
    width: u32,
    element_count: u64,
    cursor: u64,
    buffer: Vec<u8>,
    buffer_start: u64,
    buffer_len: u64,
    codec: ScalarCodec,
}

impl ReferenceStream {
    fn open(path: &Path, width: u32, element_count: u64) -> Result<ReferenceStream, NnError> {
        let reader = BoundedFile::open(path)?;
        let codec = match width {
            4 => ScalarCodec::F32,
            8 => ScalarCodec::F64,
            other => {
                return Err(NnError::InvalidRequest {
                    message: format!("unsupported reference width {other} (use 4 or 8)"),
                })
            }
        };
        if reader.length() != element_count * width as u64 {
            return Err(NnError::InvalidRequest {
                message: format!(
                    "reference has {} bytes but the tensor has {} elements of width {width}",
                    reader.length(),
                    element_count
                ),
            });
        }
        Ok(ReferenceStream {
            reader,
            width,
            element_count,
            cursor: 0,
            buffer: vec![0u8; ELEMENT_CHUNK_BYTES],
            buffer_start: u64::MAX,
            buffer_len: 0,
            codec,
        })
    }

    fn fetch(&mut self, index: u64) -> Result<(), NnError> {
        if index >= self.element_count {
            return Err(NnError::InvalidRequest {
                message: "reference index out of range".to_string(),
            });
        }
        let capacity = (self.buffer.len() as u64) / self.width as u64;
        if index >= self.buffer_start && index < self.buffer_start.saturating_add(self.buffer_len) {
            return Ok(());
        }
        self.buffer_start = index - (index % capacity.max(1));
        let take = capacity.min(self.element_count - self.buffer_start);
        self.reader.read_exact_at(
            self.buffer_start * self.width as u64,
            &mut self.buffer[..(take * self.width as u64) as usize],
        )?;
        self.buffer_len = take;
        Ok(())
    }

    fn at(&mut self, index: u64) -> Result<f64, NnError> {
        self.fetch(index)?;
        self.value_at(index)
    }

    fn next(&mut self) -> Result<f64, NnError> {
        let index = self.cursor;
        self.at(index)
    }

    fn value_at(&mut self, index: u64) -> Result<f64, NnError> {
        let offset = (index - self.buffer_start) * self.width as u64;
        let bytes = &self.buffer[offset as usize..(offset + self.width as u64) as usize];
        let value = self.codec.decode(bytes)?.value;
        self.cursor = index + 1;
        Ok(match value {
            ScalarValue::Float(f) => f,
            ScalarValue::Int(v) => v as f64,
            ScalarValue::Uint(v) => v as f64,
            ScalarValue::Bool(v) => (v as u8) as f64,
        })
    }
}

// ---- rendering ----

fn optional_number(value: Option<f64>) -> Json {
    match value {
        Some(v) if v.is_finite() => Json::Str(format_number(v)),
        Some(_) => Json::Null, // non-finite aggregates render as null + category counts
        None => Json::Null,
    }
}

fn format_number(v: f64) -> String {
    format!("{v}")
}

/// Build the analysis envelope.
pub fn analysis_envelope(
    catalog: &Catalog,
    result: &AnalysisResult,
) -> Result<ResultEnvelope, NnError> {
    let stats_json = match &result.stats {
        None => Json::Null,
        Some(stats) => Json::object(vec![
            ("finite_count", Json::Str(stats.finite_count.to_string())),
            ("nan_count", Json::Str(stats.nan_count.to_string())),
            ("pos_inf", Json::Str(stats.pos_inf.to_string())),
            ("neg_inf", Json::Str(stats.neg_inf.to_string())),
            ("pos_zero", Json::Str(stats.pos_zero.to_string())),
            ("neg_zero", Json::Str(stats.neg_zero.to_string())),
            ("mean", optional_number(stats.mean)),
            (
                "population_variance",
                optional_number(stats.population_variance),
            ),
            ("sample_variance", optional_number(stats.sample_variance)),
            ("abs_sum", optional_number(stats.abs_sum)),
            ("l2_norm", optional_number(stats.l2_norm)),
            (
                "min",
                match &stats.min {
                    Some((v, c)) => Json::object(vec![
                        ("value", Json::Str(format_number(*v))),
                        (
                            "coordinate",
                            Json::Array(c.iter().map(|i| Json::Str(i.to_string())).collect()),
                        ),
                        ("ties", Json::Str(stats.min_ties.to_string())),
                    ])?,
                    None => Json::Null,
                },
            ),
            (
                "max",
                match &stats.max {
                    Some((v, c)) => Json::object(vec![
                        ("value", Json::Str(format_number(*v))),
                        (
                            "coordinate",
                            Json::Array(c.iter().map(|i| Json::Str(i.to_string())).collect()),
                        ),
                        ("ties", Json::Str(stats.max_ties.to_string())),
                    ])?,
                    None => Json::Null,
                },
            ),
            (
                "constant",
                match stats.constant {
                    Some(v) => Json::Str(format_number(v)),
                    None => Json::Bool(false),
                },
            ),
        ])?,
    };
    let histogram_json = match &result.histogram {
        None => Json::Null,
        Some(h) => Json::object(vec![
            (
                "edges",
                Json::Array(
                    h.edges
                        .iter()
                        .map(|e| Json::Str(format_number(*e)))
                        .collect(),
                ),
            ),
            (
                "counts",
                Json::Array(h.counts.iter().map(|c| Json::Str(c.to_string())).collect()),
            ),
            ("out_of_range", Json::Str(h.out_of_range.to_string())),
            (
                "rule",
                Json::Str("half-open [lower, upper); final bin closed".to_string()),
            ),
        ])?,
    };
    let blocks_json: Vec<Json> = result
        .blocks
        .iter()
        .zip(&result.block_range)
        .map(|(block, range)| {
            Json::object(vec![
                (
                    "element_range",
                    Json::Str(format!("{}..{}", range.0, range.1 + 1)),
                ),
                ("scale", Json::Str(format_number(block.scale))),
                ("scale_raw", Json::Str(block.scale_raw.clone())),
                (
                    "values",
                    Json::Array(
                        block
                            .values
                            .iter()
                            .map(|v| Json::Str(format_number(*v)))
                            .collect(),
                    ),
                ),
            ])
        })
        .collect::<Result<Vec<_>, _>>()?;
    let reference_json = match &result.reference {
        None => Json::Null,
        Some(m) => Json::object(vec![
            ("compared", Json::Str(m.compared.to_string())),
            ("mae", optional_number(m.mae)),
            ("rmse", optional_number(m.rmse)),
            ("max_abs_error", optional_number(m.max_abs_error)),
            ("relative_l2", optional_number(m.relative_l2)),
            ("exact_equality", Json::Bool(m.exact_equality)),
            ("relative_undefined", Json::Bool(m.relative_undefined)),
            (
                "nonfinite_mismatches",
                Json::Str(m.nonfinite_mismatches.to_string()),
            ),
        ])?,
    };
    let findings: Vec<Json> = result
        .findings
        .iter()
        .map(|f| {
            Json::object(vec![
                ("code", Json::Str(f.code.to_string())),
                ("severity", Json::Str(f.severity.as_str().to_string())),
                ("message", Json::Str(f.message.clone())),
            ])
        })
        .collect::<Result<Vec<_>, _>>()?;
    let semantic = Json::object(vec![
        ("catalog_id", Json::Str(catalog.id()?)),
        ("tensor_id", Json::Str(result.tensor_id.clone())),
        ("name", Json::Str(result.name.clone())),
        ("encoding", Json::Str(result.encoding.clone())),
        (
            "shape",
            Json::Array(
                result
                    .shape
                    .iter()
                    .map(|d| Json::Str(d.to_string()))
                    .collect(),
            ),
        ),
        ("elements", Json::Str(result.element_count.to_string())),
        (
            "coverage",
            Json::object(vec![
                ("mode", Json::Str(result.coverage.mode.as_str().to_string())),
                (
                    "eligible_elements",
                    Json::Str(result.coverage.eligible_elements.to_string()),
                ),
                (
                    "examined_elements",
                    Json::Str(result.coverage.examined_elements.to_string()),
                ),
                (
                    "fetched_bytes",
                    Json::Str(result.coverage.fetched_bytes.to_string()),
                ),
                (
                    "seed",
                    match result.coverage.seed {
                        Some(s) => Json::Str(s.to_string()),
                        None => Json::Null,
                    },
                ),
                (
                    "sample_size",
                    match result.coverage.sample_size {
                        Some(s) => Json::Str(s.to_string()),
                        None => Json::Null,
                    },
                ),
                (
                    "termination",
                    Json::Str(result.coverage.termination.to_string()),
                ),
            ])?,
        ),
        ("stats", stats_json),
        ("histogram", histogram_json),
        (
            "blocks",
            if blocks_json.is_empty() {
                Json::Null
            } else {
                Json::Array(blocks_json)
            },
        ),
        ("reference", reference_json),
        ("findings", Json::Array(findings)),
        (
            "notes",
            Json::Array(result.notes.iter().cloned().map(Json::Str).collect()),
        ),
    ])?;
    let complete =
        result.coverage.termination == "completed" || result.coverage.mode == ScanMode::Metadata;
    let mut envelope = ResultEnvelope::new("analyze").with_semantic(semantic);
    if !complete {
        envelope = envelope.with_coverage(false, vec!["scan terminated early".to_string()]);
    }
    Ok(envelope)
}

/// Human-readable analysis output.
pub fn analysis_text(result: &AnalysisResult) -> String {
    let mut out = String::new();
    out.push_str(&format!("analyze {} ({})\n", result.name, result.encoding));
    out.push_str(&format!(
        "  coverage: mode {} — {} of {} elements examined, {} bytes read ({})\n",
        result.coverage.mode.as_str(),
        result.coverage.examined_elements,
        result.coverage.eligible_elements,
        result.coverage.fetched_bytes,
        result.coverage.termination
    ));
    if let Some(stats) = &result.stats {
        out.push_str(&format!("  finite:   {}\n", stats.finite_count));
        if stats.nan_count + stats.pos_inf + stats.neg_inf > 0 {
            out.push_str(&format!(
                "  non-finite: NaN {}, +Inf {}, -Inf {}\n",
                stats.nan_count, stats.pos_inf, stats.neg_inf
            ));
        }
        if let Some(mean) = stats.mean {
            out.push_str(&format!("  mean:     {mean}\n"));
        }
        if let Some(pv) = stats.population_variance {
            out.push_str(&format!("  variance: {pv} (population)\n"));
        }
        if let Some(sv) = stats.sample_variance {
            out.push_str(&format!("            {sv} (sample)\n"));
        }
        if let Some((min, coord)) = &stats.min {
            out.push_str(&format!(
                "  min:      {min} at {:?} (+{} ties)\n",
                coord, stats.min_ties
            ));
        }
        if let Some((max, coord)) = &stats.max {
            out.push_str(&format!(
                "  max:      {max} at {:?} (+{} ties)\n",
                coord, stats.max_ties
            ));
        }
        if let Some(norm) = stats.l2_norm {
            out.push_str(&format!("  L2 norm:  {norm}\n"));
        }
        if stats.constant.is_some() && stats.finite_count > 0 {
            out.push_str("  note:     population is constant\n");
        }
    }
    if let Some(histogram) = &result.histogram {
        out.push_str(&format!(
            "  histogram: {} bins over [{}, {}]\n",
            histogram.counts.len(),
            histogram.edges.first().copied().unwrap_or(0.0),
            histogram.edges.last().copied().unwrap_or(0.0)
        ));
        for (i, count) in histogram.counts.iter().enumerate() {
            out.push_str(&format!(
                "    [{:.6}, {:.6}){end}: {count}\n",
                histogram.edges[i],
                histogram.edges[i + 1],
                end = if i + 1 == histogram.counts.len() {
                    "]"
                } else {
                    ""
                }
            ));
        }
    }
    for (block, range) in result.blocks.iter().zip(&result.block_range) {
        out.push_str(&format!(
            "  block {}: scale {} (raw {}), first values {}, {}, {}\n",
            range.0 / 32,
            block.scale,
            block.scale_raw,
            block.values[0],
            block.values[1],
            block.values[16]
        ));
    }
    if let Some(metrics) = &result.reference {
        out.push_str(&format!(
            "  reference: {} pairs compared\n",
            metrics.compared
        ));
        if let Some(mae) = metrics.mae {
            out.push_str(&format!("    MAE:  {mae}\n"));
        }
        if let Some(rmse) = metrics.rmse {
            out.push_str(&format!("    RMSE: {rmse}\n"));
        }
        if let Some(maxe) = metrics.max_abs_error {
            out.push_str(&format!("    maxAE: {maxe}\n"));
        }
        match metrics.relative_l2 {
            Some(rel) => out.push_str(&format!("    relative L2: {rel}\n")),
            None if metrics.exact_equality => {
                out.push_str("    relative L2: undefined (both norms zero; exact equality)\n")
            }
            None if metrics.relative_undefined => out.push_str(
                "    relative L2: undefined (reference norm is zero with nonzero error)\n",
            ),
            None => {}
        }
    }
    for finding in &result.findings {
        out.push_str(&format!(
            "  finding [{}] {}: {}\n",
            finding.severity.as_str(),
            finding.code,
            finding.message
        ));
    }
    for note in &result.notes {
        out.push_str(&format!("  note: {note}\n"));
    }
    out
}

/// Convert a `ResultEnvelope` to diagnostics (used by the CLI for --fail-on
/// style policies in later milestones).
pub fn findings_as_diagnostics(result: &AnalysisResult) -> Vec<Diagnostic> {
    result
        .findings
        .iter()
        .map(|f| Diagnostic::new(f.code, f.severity, f.message.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(stats: &mut StreamingStats, values: &[f64]) {
        for (i, v) in values.iter().enumerate() {
            stats.update(*v, &[i as u64]);
        }
    }

    /// Reference vector: [1,2,3,4] → mean 2.5, population variance 1.25,
    /// sample variance 5/3.
    #[test]
    fn welford_reference_vector() {
        let mut stats = StreamingStats::default();
        feed(&mut stats, &[1.0, 2.0, 3.0, 4.0]);
        let summary = stats.summary();
        assert_eq!(summary.finite_count, 4);
        let mean = summary.mean.unwrap();
        let pop = summary.population_variance.unwrap();
        let sample = summary.sample_variance.unwrap();
        assert!((mean - 2.5).abs() < 1e-12, "{mean}");
        assert!((pop - 1.25).abs() < 1e-12, "{pop}");
        assert!((sample - 5.0 / 3.0).abs() < 1e-12, "{sample}");
        assert_eq!(summary.min.as_ref().unwrap().0, 1.0);
        assert_eq!(summary.max.as_ref().unwrap().0, 4.0);
        assert_eq!(summary.min.as_ref().unwrap().1, vec![0]);
        assert_eq!(summary.max.as_ref().unwrap().1, vec![3]);
    }

    #[test]
    fn empty_and_single_populations_have_no_invented_values() {
        let empty = StreamingStats::default().summary();
        assert_eq!(empty.finite_count, 0);
        assert!(empty.mean.is_none());
        assert!(empty.population_variance.is_none());
        assert!(empty.sample_variance.is_none());
        assert!(empty.l2_norm.is_none());

        let mut single = StreamingStats::default();
        single.update(7.0, &[]);
        let summary = single.summary();
        assert_eq!(summary.finite_count, 1);
        assert_eq!(summary.mean, Some(7.0));
        assert_eq!(summary.population_variance, Some(0.0));
        assert!(summary.sample_variance.is_none(), "one degree of freedom");
    }

    #[test]
    fn l2_norm_is_overflow_safe() {
        let mut huge = StreamingStats::default();
        feed(&mut huge, &[3e200, 4e200]);
        let norm = huge.l2_norm().unwrap();
        assert!((norm - 5e200).abs() < 5e187, "{norm}");

        let mut tiny = StreamingStats::default();
        feed(&mut tiny, &[3e-200, 4e-200]);
        let norm = tiny.l2_norm().unwrap();
        assert!((norm - 5e-200).abs() < 5e-213, "{norm}");

        let mut plain = StreamingStats::default();
        feed(&mut plain, &[3.0, 4.0]);
        assert!((plain.l2_norm().unwrap() - 5.0).abs() < 1e-12);
    }

    #[test]
    fn nonfinite_categories_are_counted_not_averaged() {
        let mut stats = StreamingStats::default();
        feed(
            &mut stats,
            &[
                1.0,
                f64::NAN,
                f64::INFINITY,
                f64::NEG_INFINITY,
                0.0,
                -0.0,
                2.0,
            ],
        );
        let summary = stats.summary();
        assert_eq!(summary.finite_count, 4, "zeros are finite values");
        assert_eq!(summary.nan_count, 1);
        assert_eq!(summary.pos_inf, 1);
        assert_eq!(summary.neg_inf, 1);
        assert_eq!(summary.pos_zero, 1);
        assert_eq!(summary.neg_zero, 1);
        assert!((summary.mean.unwrap() - 0.75).abs() < 1e-12);
    }

    #[test]
    fn tie_rules_report_counts() {
        let mut stats = StreamingStats::default();
        feed(&mut stats, &[5.0, 1.0, 5.0, 1.0, 5.0]);
        let summary = stats.summary();
        assert_eq!(summary.max.as_ref().unwrap().1, vec![0], "first occurrence");
        assert_eq!(summary.max_ties, 2);
        assert_eq!(summary.min.as_ref().unwrap().1, vec![1]);
        assert_eq!(summary.min_ties, 1);
    }

    /// A varying sequence must never be reported constant, no matter where
    /// its equal-adjacent pairs fall (regression: the old Option-based
    /// tracker re-seeded Some(value) after every differing value, so
    /// [1, 2, 3] and any sequence ending in an equal pair read as constant).
    #[test]
    fn varying_populations_are_never_constant() {
        for values in [
            vec![1.0, 2.0, 3.0],
            vec![1.0, 2.0, 2.0],
            vec![1.0, 1.0, 2.0],
            vec![0.0, 0.0, 1.0, 1.0],
            vec![-1.0, -1.0, -1.0, 0.5],
        ] {
            let mut stats = StreamingStats::default();
            feed(&mut stats, &values);
            let summary = stats.summary();
            assert!(
                summary.constant.is_none(),
                "{values:?} must not be reported constant"
            );
        }
    }

    #[test]
    fn uniform_populations_are_constant() {
        let mut stats = StreamingStats::default();
        feed(&mut stats, &[0.25, 0.25, 0.25]);
        assert_eq!(stats.summary().constant, Some(0.25));

        // Non-finite values do not participate in the finite-constancy claim.
        let mut with_nan = StreamingStats::default();
        feed(&mut with_nan, &[0.25, f64::NAN, 0.25]);
        assert_eq!(with_nan.summary().constant, Some(0.25));
    }

    /// Reference vector: x = [0,2], y = [1,0] → MAE 1.5, RMSE √2.5,
    /// maxAE 2, relative L2 √5/2.
    #[test]
    fn error_metrics_reference_vector() {
        let pairs = [(0.0, 1.0), (2.0, 0.0)];
        let metrics = error_metrics(&pairs);
        assert_eq!(metrics.compared, 2);
        assert!((metrics.mae.unwrap() - 1.5).abs() < 1e-12);
        assert!((metrics.rmse.unwrap() - 2.5f64.sqrt()).abs() < 1e-12);
        assert!((metrics.max_abs_error.unwrap() - 2.0).abs() < 1e-12);
        assert!((metrics.relative_l2.unwrap() - 5.0f64.sqrt() / 2.0).abs() < 1e-12);
    }

    #[test]
    fn zero_denominator_policies() {
        // Both zero: exact equality, relative measure undefined-but-equal.
        let metrics = error_metrics(&[(0.0, 0.0), (0.0, 0.0)]);
        assert!(metrics.exact_equality);
        assert!(metrics.relative_l2.is_none());
        assert!(!metrics.relative_undefined);

        // Zero reference with nonzero error: undefined, never a fake 0%.
        let metrics = error_metrics(&[(0.0, 1.0)]);
        assert!(metrics.relative_undefined);
        assert!(metrics.relative_l2.is_none());
        assert!((metrics.mae.unwrap() - 1.0).abs() < 1e-12);

        // Empty: not applicable.
        let metrics = error_metrics(&[]);
        assert_eq!(metrics.compared, 0);
        assert!(metrics.mae.is_none());
    }

    #[test]
    fn nonfinite_mismatches_are_counted_separately() {
        let metrics = error_metrics(&[(1.0, f64::NAN), (2.0, 2.0)]);
        assert_eq!(metrics.nonfinite_mismatches, 1);
        assert_eq!(metrics.compared, 1);
        assert!(metrics.mae.is_some());
    }

    #[test]
    fn histogram_declares_edges_and_counts() {
        let histogram = Histogram::from_values(&[1.0, 2.0, 3.0, 4.0], 4);
        assert_eq!(histogram.counts, vec![1, 1, 1, 1]);
        assert_eq!(histogram.edges.len(), 5);
        assert_eq!(histogram.out_of_range, 0);

        let constant = Histogram::from_values(&[7.0, 7.0, 7.0], 4);
        assert_eq!(constant.counts, vec![3]);
        assert_eq!(constant.edges, vec![7.0, 7.0]);
    }

    #[test]
    fn sampling_is_deterministic_and_seed_sensitive() {
        let a = sample_index(17, 0);
        let b = sample_index(17, 0);
        assert_eq!(a, b);
        assert_ne!(a, sample_index(18, 0));
        // Tensor-scoped seeds differ per tensor id.
        assert_ne!(tensor_seed(17, "tensor:aaa"), tensor_seed(17, "tensor:bbb"));
    }

    #[test]
    fn analysis_envelope_renders_coverage_and_stats() {
        let result = AnalysisResult {
            tensor_id: "tensor:x".to_string(),
            name: "w".to_string(),
            encoding: "safetensors.F32".to_string(),
            shape: vec![2, 2],
            element_count: 4,
            coverage: Coverage {
                mode: ScanMode::Full,
                eligible_elements: 4,
                examined_elements: 4,
                fetched_bytes: 16,
                seed: None,
                sample_size: None,
                termination: "completed",
            },
            stats: Some({
                let mut stats = StreamingStats::default();
                feed(&mut stats, &[1.0, 2.0, 3.0, 4.0]);
                stats.summary()
            }),
            histogram: None,
            blocks: Vec::new(),
            block_range: Vec::new(),
            reference: None,
            findings: Vec::new(),
            notes: Vec::new(),
        };
        // Minimal catalog from a real fixture for envelope rendering.
        let dir = tempfile::tempdir().unwrap();
        let header = r#"{"w":{"dtype":"F32","shape":[2,2],"data_offsets":[0,16]}}"#;
        let mut data = Vec::new();
        data.extend_from_slice(&(header.len() as u64).to_le_bytes());
        data.extend_from_slice(header.as_bytes());
        data.extend_from_slice(&[0u8; 16]);
        std::fs::write(dir.path().join("m.safetensors"), data).unwrap();
        let report = crate::nn::discover::discover(
            &dir.path().join("m.safetensors"),
            &crate::nn::discover::DiscoverOptions::default(),
            &crate::nn::budget::Budget::new(
                crate::nn::budget::BudgetCaps::default(),
                None,
                crate::nn::cancel::CancellationToken::new(),
            ),
        )
        .unwrap();
        let catalog = crate::nn::catalog::Catalog::from_discovery(&report).unwrap();
        let envelope = analysis_envelope(&catalog, &result).unwrap();
        let text = envelope.to_json_string().unwrap();
        let l2 = result
            .stats
            .as_ref()
            .and_then(|s| s.l2_norm)
            .map(|n| format!("{n}"))
            .unwrap_or_default();
        assert!(text.contains("\"operation\":\"analyze\""));
        assert!(text.contains("\"mode\":\"full\""));
        assert!(text.contains("\"population_variance\":\"1.25\""));
        assert!(text.contains("\"sample_variance\":\"1.666"));
        assert!(
            text.contains(&format!("\"l2_norm\":\"{l2}\"")),
            "l2 rendered as {l2} but not found in envelope"
        );
        let human = analysis_text(&result);
        assert!(human.contains("4 of 4 elements examined"));
        assert!(human.contains("variance: 1.25 (population)"));
    }
}
