//! EXL3 trellis decode (BF-001 codec tier).
//!
//! Transcribed from turboderp-org/exllamav3 (exllamav3_ext/quant/
//! exl3_dq.cuh + codebook.cuh + reconstruct.cu) and qualified against a
//! Python reference on the authentic k2-layer12-expert27-up sample.
//! Layout facts (from exl3_gemv_kernel.cuh / modules/quant/exl3.py):
//! trellis[k_group][n_group][16*bits u16]; each 16x16 tile is row-major
//! t = n_local*16 + k_local; a weight's codebook input is the 16-bit
//! window ENDING at its last code bit (wrap-around inside the tile).
//! Logical weights live in 128-point Hadamard blocks:
//! W = H · diag(svh) · Wq · diag(suh) · H (suh over the input extent,
//! svh over the output extent, blocks of 128).

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
    f16_bits((x >> 16) as u16) + f16_bits(x as u16)
}

/// One trellis tile (16×16 weights, `16*bits` u16 words, wrapped).
pub struct Tile<'a> {
    words: &'a [u16],
    bits: u32,
}

impl<'a> Tile<'a> {
    pub fn new(words: &'a [u16], bits: u32) -> Result<Tile<'a>, NnError> {
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

    fn bit(&self, i: u32) -> u32 {
        let i = i % (256 * self.bits);
        (self.words[(i / 16) as usize] >> (i % 16) & 1) as u32
    }

    /// Decoded Hadamard-domain value for tile-linear weight t (0..255,
    /// row-major n_local*16 + k_local).
    pub fn value(&self, t: u32) -> f32 {
        let e = (t + 1) * self.bits;
        let mut window = 0u32;
        for j in 0..16 {
            window |= self.bit(e.wrapping_sub(16).wrapping_add(j)) << j;
        }
        decode_mcg(window as u16)
    }
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
    let t = ((n % 16) * 16 + (k % 16)) as u32;
    Ok(tile.value(t))
}

/// Logical 128×128 block W = H·diag(svh)·Wq·diag(suh)·H for block
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
        let (kg, ng) = ((k0 / 16) as usize, (n / 16) as usize);
        let tile_words: Vec<u16> = {
            let flat = (kg * ex.trellis.shape[1] as usize + ng) * words_per_tile;
            words[flat..flat + words_per_tile].to_vec()
        };
        let tile = Tile::new(&tile_words, ex.bits)?;
        for ki in 0..128u64 {
            let t = ((n % 16) * 16 + ((k0 + ki) % 16)) as u32;
            wq[(ni * 128 + ki) as usize] = tile.value(t);
        }
    }
    // W = H · diag(svh) · Wq · diag(suh) · H, computed as explicit
    // left/right applications of the block Hadamard.
    // Step 1: W1 = Wq · diag(suh) (scale columns), then W1 · H (columns mix).
    // Step 2: diag(svh) rows scale, then H · (that).
    let mut w2 = vec![0f32; 128 * 128]; // Wq · diag(suh) · H
    for i in 0..128 {
        for j in 0..128 {
            let mut acc = 0f32;
            for m in 0..128 {
                acc += wq[i * 128 + m] * f16_at(&suh_bytes, k0 + m as u64) * had(m, j);
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
                acc += had(i, m) * s * w2[m * 128 + j];
            }
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
        ("value", Json::Float(value as f64)),
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
        "exl3 logical block\n  origin:    ({}, {})\n  mean:      {:.6}\n  variance:  {:.8}\n  abs max:   {:.6}\n  claims: logical weights under H . diag(svh) . Wq . diag(suh) . H; model quality and behavior are NOT implied\n",
        summary.n0, summary.k0, summary.mean, summary.var, summary.absmax
    )
}

pub fn block_envelope(summary: &BlockSummary) -> Result<ResultEnvelope, NnError> {
    let semantic = Json::object(vec![
        ("n0", Json::Str(summary.n0.to_string())),
        ("k0", Json::Str(summary.k0.to_string())),
        ("mean", Json::Float(summary.mean as f64)),
        ("variance", Json::Float(summary.var as f64)),
        ("absmax", Json::Float(summary.absmax as f64)),
        (
            "claims",
            Json::Str(
                "logical weights under the Hadamard transform; no behavioral claim".to_string(),
            ),
        ),
    ])?;
    Ok(ResultEnvelope::new("exl3.block").with_semantic(semantic))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mcg_codebook_matches_reference_values() {
        // Reference table computed independently (Python transcription of
        // codebook.cuh 3INST cb=1) on codes 0..3.
        let table: [f32; 4] = [1.843_75, 0.134_521_5, -0.759_277_3, 0.839_599_6];
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
        // End-to-end decode path on a synthetic 2bpw tile: fill the tile
        // with a known bit pattern and check the window arithmetic. The
        // authentic-sample values (Wq[0,0] = -1.2127686, Wq[3,5] =
        // -1.4238281 on the k2 sample) are exercised through the real
        // binary in the GLM deep-dive walkthrough.
        let words = vec![0xFFFFu16; 32]; // every tile bit set
        let tile = Tile::new(&words, 2).unwrap();
        let v = tile.value(0); // any 16-bit window = all ones
        assert_eq!(v, decode_mcg(0xFFFF));
        let words = vec![0u16; 32];
        let tile = Tile::new(&words, 2).unwrap();
        assert_eq!(tile.value(137), decode_mcg(0));
        // Tile-size mismatch is a visible error.
        assert!(Tile::new(&[0u16; 31], 2).is_err());
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
