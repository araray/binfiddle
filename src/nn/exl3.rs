//! EXL3 trellis decode (BF-001 codec tier).
//!
//! Transcribed from turboderp-org/exllamav3 (exllamav3_ext/quant/
//! exl3_dq.cuh + codebook.cuh + reconstruct.cu), pinned at
//! 151539c77abc7ab7425d30da7a4e8e3c5c154e7b. See the GLM lab and its
//! independent Python reference for reproducible qualification.
//! trellis[k_group][n_group][16*bits u16]; each 16x16 tile uses the
//! tensor-core lane permutation in exl3_lib/quantize.py. Codes are packed
//! MSB-first into little-endian u32 words. A codebook input is the 16-bit
//! window ENDING at its last code bit, wrapping inside the tile.
//! Logical weights live in 128-point Hadamard blocks:
//! W[N,K] = diag(svh) · H · Wq · H · diag(suh). Block arithmetic uses
//! f32, not the intermediate fp16 rounding of a particular GPU kernel.

use super::budget::Budget;
use super::catalog::{Catalog, CatalogTensor};
use super::error::NnError;
use super::format::npy::ArrayReader;
use super::json::Json;
use super::report::ResultEnvelope;

/// mcg codebook selector magic (0xCBAC1FED as u32).
pub const MCG_MAGIC: u32 = 0xCBAC1FED;

/// fp16 bit pattern → f32.
fn f16_bits(bits: u16) -> f32 {
    super::codec::f16_to_f64(bits) as f32
}

/// 3INST mcg codebook (cb 1): x * 0xCBAC1FED, then
/// (x & 0x8fff8fff) ^ 0x3b603b60, then fp16(hi) + fp16(lo).
pub fn decode_mcg(window: u16) -> f32 {
    let mut x = (window as u32).wrapping_mul(MCG_MAGIC);
    x = (x & 0x8fff_8fff) ^ 0x3b60_3b60;
    let sum = f16_bits((x >> 16) as u16) + f16_bits(x as u16);
    // CUDA __hadd rounds the codebook sum to binary16 before promotion.
    f16_bits(super::edit::f64_to_f16_bits(sum as f64))
}

/// One trellis tile (16×16 weights, `16*bits` u16 words, wrapped).
pub struct Tile<'a> {
    words: &'a [u16],
    bits: u32,
}

impl<'a> Tile<'a> {
    pub fn new(words: &'a [u16], bits: u32) -> Result<Tile<'a>, NnError> {
        if !(1..=8).contains(&bits) {
            return Err(NnError::InvalidRequest {
                message: "EXL3 mcg supports integer bitrates 1..8".to_string(),
            });
        }
        if words.len() != (16 * bits) as usize {
            return Err(NnError::MalformedInput {
                detail: format!(
                    "trellis tile has {} u16 words; 16*bits = {} expected",
                    words.len(),
                    16 * bits
                ),
            });
        }
        Ok(Tile { words, bits })
    }

    /// Decoded value for encoded lane index t (0..255), NOT a row-major index.
    pub fn value(&self, t: u32) -> f32 {
        let end = (t + 1 + 256) * self.bits;
        let first = (end - 16) / 32;
        let last = (end - 1) / 32;
        let shift = (last + 1) * 32 - end;
        let word = |i: u32| {
            let i = (i % (self.bits * 8)) as usize * 2;
            self.words[i] as u32 | ((self.words[i + 1] as u32) << 16)
        };
        let pair = ((word(first) as u64) << 32) | word(last) as u64;
        let window = (pair >> shift) & 0xffff;
        decode_mcg(window as u16)
    }
}

/// Inverse of upstream tensor_core_perm: [k_local, n_local] -> lane index.
fn lane_index(n: u64, k: u64) -> u32 {
    let (n, k) = (n % 16, k % 16);
    let lane = (n % 8) * 4 + (k % 8) / 2;
    (lane * 8 + (n / 8) * 4 + (k / 8) * 2 + k % 2) as u32
}

/// The 128-point Sylvester Hadamard row i, col j: (-1)^popcount(i & j)/√128.
fn had(i: usize, j: usize) -> f32 {
    if i == 0 {
        return 1.0 / (128.0f32).sqrt();
    }
    let sign = if (i & j).count_ones().is_multiple_of(2) {
        1.0
    } else {
        -1.0
    };
    sign / (128.0f32).sqrt()
}

pub struct Exl3Tensors<'a> {
    pub trellis: &'a CatalogTensor,
    pub suh: &'a CatalogTensor,
    pub svh: &'a CatalogTensor,
    pub mcg: &'a CatalogTensor,
    pub bits: u32,
    pub size_n: u64,
    pub size_k: u64,
}

/// Resolve the four sibling fields for one projection from a catalog.
pub fn resolve<'a>(catalog: &'a Catalog, trellis_name: &str) -> Result<Exl3Tensors<'a>, NnError> {
    let base = trellis_name
        .strip_suffix(".trellis")
        .ok_or_else(|| NnError::InvalidRequest {
            message: format!("{trellis_name} is not a .trellis field"),
        })?;
    let find = |suffix: &str| -> Result<&'a CatalogTensor, NnError> {
        catalog
            .tensors
            .iter()
            .find(|t| t.original_name == format!("{base}.{suffix}"))
            .ok_or_else(|| NnError::SourceMissing {
                detail: format!("EXL3 field {base}.{suffix} not in the catalog"),
            })
    };
    let trellis = find("trellis")?;
    let suh = find("suh")?;
    let svh = find("svh")?;
    let mcg = find("mcg")?;
    if trellis.shape.len() != 3 {
        return Err(NnError::MalformedInput {
            detail: format!("trellis must be 3-axis, got {:?}", trellis.shape),
        });
    }
    let bits = trellis.shape[2] / 16;
    if bits == 0 || trellis.shape[2] % 16 != 0 {
        return Err(NnError::MalformedInput {
            detail: format!(
                "trellis inner axis {} is not a positive multiple of 16",
                trellis.shape[2]
            ),
        });
    }
    let (k_groups, n_groups) = (trellis.shape[0], trellis.shape[1]);
    Ok(Exl3Tensors {
        trellis,
        suh,
        svh,
        mcg,
        bits: bits as u32,
        size_n: n_groups * 16,
        size_k: k_groups * 16,
    })
}

fn load_field(
    catalog: &Catalog,
    tensor: &CatalogTensor,
    budget: &Budget,
) -> Result<Vec<u8>, NnError> {
    let source = catalog.resolve_source(&tensor.source_id)?;
    let path = catalog.resolve_path(&source.path);
    let _width = match super::codec::layout_for_encoding(&tensor.encoding) {
        super::codec::TensorLayout::Scalar(codec) => codec.width(),
        _ => {
            return Err(NnError::CodecUnsupported {
                codec: tensor.encoding.clone(),
                operation: "exl3 field load".to_string(),
                reason: "field is not scalar-decodable".to_string(),
            })
        }
    };
    let len = tensor
        .payload_length
        .ok_or_else(|| NnError::InvalidRequest {
            message: format!("field {} has a bounded extent", tensor.original_name),
        })?;
    // NumPy captures read through the bounded ArrayReader; container-backed
    // fields read their exact payload span directly.
    if tensor.encoding.starts_with("numpy.") {
        let reader = ArrayReader::open(&path, tensor, budget)?;
        reader.read_all(len, budget)
    } else {
        let file = crate::nn::source::BoundedFile::open(&path)?;
        let mut buf = vec![0u8; len as usize];
        budget.consume_source_read(len)?;
        file.read_exact_at(tensor.payload_start, &mut buf)?;
        Ok(buf)
    }
}

/// Read the mcg selector and verify it names the mcg codebook.
pub fn check_mcg(catalog: &Catalog, ex: &Exl3Tensors, budget: &Budget) -> Result<(), NnError> {
    let bytes = load_field(catalog, ex.mcg, budget)?;
    let value = u32::from_le_bytes(
        bytes
            .get(0..4)
            .ok_or_else(|| NnError::MalformedInput {
                detail: "mcg field is empty".to_string(),
            })?
            .try_into()
            .unwrap(),
    );
    if value != MCG_MAGIC {
        return Err(NnError::FormatUnsupported {
            format: "exl3".to_string(),
            reason: format!(
                "mcg selector 0x{value:08x} is not the mcg codebook (0x{MCG_MAGIC:08x}); \
                 this variant is not decodable by this codec"
            ),
        });
    }
    Ok(())
}

/// Decoded Hadamard-domain value Wq[n][k].
pub fn wq_value(
    catalog: &Catalog,
    ex: &Exl3Tensors,
    n: u64,
    k: u64,
    budget: &Budget,
) -> Result<f32, NnError> {
    if n >= ex.size_n || k >= ex.size_k {
        return Err(NnError::InvalidRequest {
            message: format!(
                "({n}, {k}) outside the projection [{}, {}]",
                ex.size_n, ex.size_k
            ),
        });
    }
    let (kg, ng) = ((k / 16) as usize, (n / 16) as usize);
    let words_per_tile = (16 * ex.bits) as usize;
    let bytes = load_field(catalog, ex.trellis, budget)?;
    // Whole-field load: locate the tile by flat index.
    let flat = (kg * ex.trellis.shape[1] as usize + ng) * words_per_tile * 2;
    let mut tile_bytes: Vec<u16> = Vec::with_capacity(words_per_tile);
    let mut i = flat;
    while i + 1 < flat + words_per_tile * 2 {
        tile_bytes.push(u16::from_le_bytes([bytes[i], bytes[i + 1]]));
        i += 2;
    }
    let tile = Tile::new(&tile_bytes, ex.bits)?;
    let t = lane_index(n, k);
    Ok(tile.value(t))
}

/// Logical 128×128 block W = diag(svh)·H·Wq·H·diag(suh) for block
/// coordinates (n0, k0) that are multiples of 128.
pub fn logical_block_summary(
    catalog: &Catalog,
    ex: &Exl3Tensors,
    n0: u64,
    k0: u64,
    budget: &Budget,
) -> Result<BlockSummary, NnError> {
    if !ex.size_n.is_multiple_of(128) || !ex.size_k.is_multiple_of(128) {
        return Err(NnError::InvalidRequest {
            message: format!(
                "extents [{}, {}] are not 128-blocked; Hadamard structure unresolved",
                ex.size_n, ex.size_k
            ),
        });
    }
    if !n0.is_multiple_of(128)
        || !k0.is_multiple_of(128)
        || n0 + 128 > ex.size_n
        || k0 + 128 > ex.size_k
    {
        return Err(NnError::InvalidRequest {
            message: format!(
                "block origin ({n0}, {k0}) must be a 128 multiple inside the projection"
            ),
        });
    }
    // Load the whole trellis once (bounded by the field size).
    let bytes = load_field(catalog, ex.trellis, budget)?;
    let mut words: Vec<u16> = Vec::with_capacity(bytes.len() / 2);
    let mut i = 0;
    while i + 1 < bytes.len() {
        words.push(u16::from_le_bytes([bytes[i], bytes[i + 1]]));
        i += 2;
    }
    let words_per_tile = (16 * ex.bits) as usize;
    let suh_bytes = load_field(catalog, ex.suh, budget)?;
    let svh_bytes = load_field(catalog, ex.svh, budget)?;
    let f16_at = |b: &[u8], i: u64| -> f32 {
        let o = (i * 2) as usize;
        f16_bits(u16::from_le_bytes([b[o], b[o + 1]]))
    };

    // Wq rows n0..n0+128, cols k0..k0+128
    let mut wq = vec![0f32; 128 * 128];
    for ni in 0..128u64 {
        let n = n0 + ni;
        for ki in 0..128u64 {
            let k = k0 + ki;
            let flat = ((k / 16) * ex.trellis.shape[1] + n / 16) as usize * words_per_tile;
            let tile = Tile::new(&words[flat..flat + words_per_tile], ex.bits)?;
            wq[(ni * 128 + ki) as usize] = tile.value(lane_index(n, k));
        }
    }
    // Upstream reconstructs [K,N]; our public coordinates are [N,K].
    // W = diag(svh) · H · Wq · H · diag(suh): the scales are outside H.
    let mut w2 = vec![0f32; 128 * 128]; // Wq · H
    for i in 0..128 {
        for j in 0..128 {
            let mut acc = 0f32;
            for m in 0..128 {
                acc += wq[i * 128 + m] * had(m, j);
            }
            w2[i * 128 + j] = acc;
        }
    }
    let mut w = vec![0f32; 128 * 128];
    let mut mean = 0f32;
    let mut m2 = 0f32;
    let mut absmax = 0f32;
    let mut count = 0u64;
    for i in 0..128 {
        let s = f16_at(&svh_bytes, n0 + i as u64);
        for j in 0..128 {
            let mut acc = 0f32;
            for m in 0..128 {
                acc += had(i, m) * w2[m * 128 + j];
            }
            acc *= s * f16_at(&suh_bytes, k0 + j as u64);
            mean += acc;
            m2 += acc * acc;
            absmax = absmax.max(acc.abs());
            w[i * 128 + j] = acc;
            count += 1;
        }
    }
    let mean = mean / count as f32;
    let var = (m2 / count as f32 - mean * mean).max(0.0);
    Ok(BlockSummary {
        n0,
        k0,
        mean,
        var,
        absmax,
        w,
    })
}

pub struct BlockSummary {
    pub n0: u64,
    pub k0: u64,
    pub mean: f32,
    pub var: f32,
    pub absmax: f32,
    pub w: Vec<f32>,
}

pub fn decode_text(
    trellis_name: &str,
    n: u64,
    k: u64,
    value: f32,
    size_n: u64,
    size_k: u64,
    bits: u32,
) -> String {
    format!(
        "exl3 decode\n  field:     {trellis_name}\n  logical:   Wq[{n}, {k}] (Hadamard domain, projection [{size_n}, {size_k}], {bits} bpw)\n  value:     {value}\n  decode dependencies: the 16-bit window ending at the weight's code, inside its 16x16 tile\n  claims: dequantized storage value under the mcg codebook; model quality and behavior are NOT implied\n"
    )
}

pub fn decode_envelope(
    trellis_name: &str,
    n: u64,
    k: u64,
    value: f32,
) -> Result<ResultEnvelope, NnError> {
    let semantic = Json::object(vec![
        ("field", Json::Str(trellis_name.to_string())),
        ("n", Json::Str(n.to_string())),
        ("k", Json::Str(k.to_string())),
        ("value", Json::Str((value as f64).to_string())),
        (
            "claims",
            Json::Str(
                "dequantized storage value; no model-quality or behavioral claim".to_string(),
            ),
        ),
    ])?;
    Ok(ResultEnvelope::new("exl3.decode").with_semantic(semantic))
}

pub fn block_text(summary: &BlockSummary) -> String {
    format!(
        "exl3 logical block\n  origin:    ({}, {})\n  mean:      {:.6}\n  variance:  {:.8}\n  abs max:   {:.6}\n  claims: f32 logical weights under diag(svh) . H . Wq . H . diag(suh); no GPU-rounding parity, model-quality or behavioral claim\n",
        summary.n0, summary.k0, summary.mean, summary.var, summary.absmax
    )
}

pub fn block_envelope(summary: &BlockSummary) -> Result<ResultEnvelope, NnError> {
    let semantic = Json::object(vec![
        ("n0", Json::Str(summary.n0.to_string())),
        ("k0", Json::Str(summary.k0.to_string())),
        ("mean", Json::Str((summary.mean as f64).to_string())),
        ("variance", Json::Str((summary.var as f64).to_string())),
        ("absmax", Json::Str((summary.absmax as f64).to_string())),
        (
            "claims",
            Json::Str(
                "f32 logical weights under diag(svh) . H . Wq . H . diag(suh); no GPU-rounding parity or behavioral claim".to_string(),
            ),
        ),
    ])?;
    Ok(ResultEnvelope::new("exl3.block").with_semantic(semantic))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_serialize_floats_as_number_free_wire_values() {
        let decode = decode_envelope("p.trellis", 3, 5, 0.540_527_34)
            .unwrap()
            .to_json_string()
            .unwrap();
        assert!(decode.contains("\"value\":\"0.54052734375\""));
        let block = block_envelope(&BlockSummary {
            n0: 0,
            k0: 0,
            mean: 0.5,
            var: 0.25,
            absmax: 1.0,
            w: vec![],
        })
        .unwrap()
        .to_json_string()
        .unwrap();
        assert!(block.contains("\"variance\":\"0.25\""));
    }

    #[test]
    fn mcg_codebook_matches_reference_values() {
        // Reference table computed independently (Python transcription of
        // codebook.cuh 3INST cb=1) on codes 0..3.
        let table: [f32; 4] = [1.843_75, 0.134_521_5, -0.759_277_3, 0.839_843_75];
        for (code, expected) in table.iter().enumerate() {
            let got = decode_mcg(code as u16);
            assert!(
                (got - expected).abs() < 1e-6,
                "code {code}: {got} != {expected}"
            );
        }
    }

    #[test]
    fn tile_window_decode_matches_reference() {
        // Uniform tiles are useful corners, but cannot detect reversed
        // bit order or an incorrect tensor-core permutation (tested below).
        let words = vec![0xFFFFu16; 32]; // every tile bit set
        let tile = Tile::new(&words, 2).unwrap();
        let v = tile.value(0); // any 16-bit window = all ones
        assert_eq!(v, decode_mcg(0xFFFF));
        let words = vec![0u16; 32];
        let tile = Tile::new(&words, 2).unwrap();
        assert_eq!(tile.value(137), decode_mcg(0));
        // Tile-size mismatch is a visible error.
        assert!(Tile::new(&[0u16; 31], 2).is_err());
        assert!(Tile::new(&[], 0).is_err());
        assert!(Tile::new(&[0u16; 144], 9).is_err());
    }

    #[test]
    fn packed_windows_follow_msb_stream_including_wrap_for_all_integer_bitrates() {
        for bits in 1..=8 {
            // Independent encoder: append each code MSB-first, then store
            // groups of 32 stream bits as little-endian u32s (pack.cu).
            let mut stream = Vec::new();
            for t in 0..256u32 {
                let code = (t * 17 + (t / 7) * 11 + 3) & ((1 << bits) - 1);
                for bit in (0..bits).rev() {
                    stream.push((code >> bit) & 1);
                }
            }
            let words: Vec<u16> = stream
                .as_chunks::<32>()
                .0
                .iter()
                .flat_map(|chunk| {
                    let word = chunk.iter().fold(0u32, |v, b| (v << 1) | b);
                    [word as u16, (word >> 16) as u16]
                })
                .collect();
            let tile = Tile::new(&words, bits).unwrap();
            for t in 0..256 {
                let end = (t + 1) * bits as usize;
                let mut window = 0u16;
                for j in 0..16 {
                    let index = (end + stream.len() - 16 + j) % stream.len();
                    window = (window << 1) | stream[index] as u16;
                }
                assert_eq!(tile.value(t as u32), decode_mcg(window), "K={bits}, t={t}");
            }
        }
    }

    #[test]
    fn lane_index_inverts_upstream_tensor_core_permutation() {
        let mut seen = [false; 256];
        for lane in 0..32u64 {
            let r = (lane % 4) * 2;
            let c = lane / 4;
            let coords = [
                (r, c),
                (r + 1, c),
                (r + 8, c),
                (r + 9, c),
                (r, c + 8),
                (r + 1, c + 8),
                (r + 8, c + 8),
                (r + 9, c + 8),
            ];
            for (i, (k, n)) in coords.into_iter().enumerate() {
                let index = lane_index(n, k) as usize;
                assert_eq!(index, lane as usize * 8 + i);
                assert!(!seen[index]);
                seen[index] = true;
            }
        }
        assert!(seen.iter().all(|v| *v));
    }

    #[test]
    fn logical_block_crosses_all_tiles_and_scales_outside_the_hadamards() {
        use crate::nn::discover::{discover, DiscoverOptions};

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("exl3.safetensors");
        let mut bytes = Vec::new();
        let mut entries = Vec::new();
        let mut append = |suffix: &str, dtype: &str, shape: &str, payload: &[u8]| {
            let start = bytes.len();
            bytes.extend_from_slice(payload);
            entries.push(format!(
                "\"p.{suffix}\":{{\"dtype\":\"{dtype}\",\"shape\":{shape},\"data_offsets\":[{start},{}]}}",
                bytes.len()
            ));
        };
        let mut trellis = Vec::new();
        for kg in 0..16 {
            for ng in 0..16 {
                let byte = if (kg + 3 * ng) % 5 < 2 { 0 } else { 255 };
                trellis.extend(std::iter::repeat_n(byte, 128));
            }
        }
        let su: Vec<u16> = (0..256).map(|i| [0x3800, 0xb400, 0x3c00][i % 3]).collect();
        let sv: Vec<u16> = (0..256)
            .map(|i| [0x3400, 0xbc00, 0x3800, 0xb800][i % 4])
            .collect();
        append("trellis", "I16", "[16,16,64]", &trellis);
        append(
            "suh",
            "F16",
            "[256]",
            &su.iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<_>>(),
        );
        append(
            "svh",
            "F16",
            "[256]",
            &sv.iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<_>>(),
        );
        append("mcg", "I32", "[1]", &MCG_MAGIC.to_le_bytes());
        let header = format!("{{{}}}", entries.join(","));
        let mut file = (header.len() as u64).to_le_bytes().to_vec();
        file.extend_from_slice(header.as_bytes());
        file.extend(bytes);
        std::fs::write(&path, file).unwrap();
        let budget = Budget::with_default_caps();
        let report = discover(&path, &DiscoverOptions::default(), &budget).unwrap();
        let catalog = Catalog::from_discovery(&report).unwrap();
        let ex = resolve(&catalog, "p.trellis").unwrap();

        // An independent f64 butterfly transform, compared element by
        // element against the production f32 matrix multiplication.
        fn fwht(values: &mut [f64]) {
            let mut step = 1;
            while step < values.len() {
                for base in (0..values.len()).step_by(2 * step) {
                    for j in 0..step {
                        let (a, b) = (values[base + j], values[base + j + step]);
                        values[base + j] = a + b;
                        values[base + j + step] = a - b;
                    }
                }
                step *= 2;
            }
            let norm = (values.len() as f64).sqrt();
            for v in values {
                *v /= norm;
            }
        }
        for (n0, k0) in [(0, 0), (128, 128)] {
            let mut expected = vec![0.0f64; 128 * 128];
            for n in 0..128 {
                for k in 0..128 {
                    let window = if ((k0 + k) / 16 + 3 * ((n0 + n) / 16)) % 5 < 2 {
                        0
                    } else {
                        0xffff
                    };
                    expected[n * 128 + k] = decode_mcg(window) as f64;
                }
                fwht(&mut expected[n * 128..(n + 1) * 128]);
            }
            for k in 0..128 {
                let mut column: Vec<f64> = (0..128).map(|n| expected[n * 128 + k]).collect();
                fwht(&mut column);
                for n in 0..128 {
                    expected[n * 128 + k] =
                        column[n] * f16_bits(sv[n0 + n]) as f64 * f16_bits(su[k0 + k]) as f64;
                }
            }
            let actual =
                logical_block_summary(&catalog, &ex, n0 as u64, k0 as u64, &budget).unwrap();
            for (i, (a, e)) in actual.w.iter().zip(&expected).enumerate() {
                assert!(
                    (*a as f64 - e).abs() < 2e-4,
                    "origin {n0},{k0}; element {i}: {a} != {e}"
                );
            }
        }
    }

    #[test]
    fn hadamard_is_orthonormal_at_corners() {
        let n = (128.0f32).sqrt();
        assert!((had(0, 0) - 1.0 / n).abs() < 1e-6);
        assert!((had(1, 0) - 1.0 / n).abs() < 1e-6);
        assert!((had(1, 1) + 1.0 / n).abs() < 1e-6);
        assert!((had(3, 5) + 1.0 / n).abs() < 1e-6); // popcount(3&5)=1 -> -
        assert!((had(5, 7) - 1.0 / n).abs() < 1e-6); // popcount(5&7)=2 -> +
    }
}
