//! Static NPY/NPZ capture reader (descriptor tier).
//!
//! NumPy captures (`.npy` single arrays, `.npz` ZIP archives of `.npy`
//! members) are the standard currency of numerical experiments. This
//! reader inventories them data-only: member names, dtypes, shapes, byte
//! order and layout, with exact payload spans for stored members and
//! honest bounded extents for compressed ones. Object arrays are never
//! unpickled; structured dtypes stay visible as unsupported descriptors.

use super::zip::{parse_zip, ZipEntry};
use super::{
    encoding_decode_supported, Extent, Finding, FormatInventory, Severity, TensorEntry, Validity,
};
use crate::nn::budget::Budget;
use crate::nn::error::NnError;
use crate::nn::source::BoundedFile;

const NPY_MAGIC: [u8; 6] = [0x93, b'N', b'U', b'M', b'P', b'Y'];

/// A parsed NPY header.
struct NpyHeader {
    /// Canonical encoding id (e.g. `numpy.float32`) and element width.
    encoding: &'static str,
    element_size: u64,
    shape: Vec<u64>,
    /// C-order (False in the header) or Fortran-order (True).
    fortran_order: bool,
    /// Header total byte length (magic through padded dict).
    header_len: u64,
    findings: Vec<Finding>,
}

fn u16le(buf: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([buf[at], buf[at + 1]])
}

fn u32le(buf: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([buf[at], buf[at + 1], buf[at + 2], buf[at + 3]])
}

/// Map a numpy descr string to (encoding, element size, decode supported).
fn descr_encoding(descr: &str) -> Option<(&'static str, u64)> {
    Some(match descr {
        "<f2" => ("numpy.float16", 2),
        "<f4" => ("numpy.float32", 4),
        "<f8" => ("numpy.float64", 8),
        "<i1" => ("numpy.int8", 1),
        "<i2" => ("numpy.int16", 2),
        "<i4" => ("numpy.int32", 4),
        "<i8" => ("numpy.int64", 8),
        "<u1" => ("numpy.uint8", 1),
        "|b1" => ("numpy.bool", 1),
        "|u1" => ("numpy.uint8", 1),
        "|i1" => ("numpy.int8", 1),
        _ => return None,
    })
}

/// Extract a scalar string value for a key from the Python-dict-literal
/// header text (e.g. 'descr'). Returns the raw quoted content.
fn header_string(header: &str, key: &str) -> Option<String> {
    let marker = format!("'{key}':");
    let at = header
        .find(&marker)
        .or_else(|| header.find(&format!("\"{key}\":")))?;
    let rest = header[at + marker.len()..].trim_start();
    // Structured dtypes carry a bracketed value: capture it verbatim.
    if let Some(bracketed) = rest.strip_prefix('[') {
        let end = bracketed.find(']')?;
        return Some(format!("[{}]", &bracketed[..end]));
    }
    let rest = rest.strip_prefix('\'').or_else(|| rest.strip_prefix('"'))?;
    let end = rest.find('\'').or_else(|| rest.find('"'))?;
    Some(rest[..end].to_string())
}

/// Extract the shape tuple as integers.
fn header_shape(header: &str) -> Option<Vec<u64>> {
    let marker = "'shape':";
    let at = header.find(marker)?;
    let rest = header[at + marker.len()..].trim_start();
    let rest = rest.strip_prefix('(')?;
    let end = rest.find(')')?;
    let inner = &rest[..end];
    if inner.trim().is_empty() {
        return Some(Vec::new());
    }
    inner
        .split(',')
        .filter(|part| !part.trim().is_empty())
        .map(|part| part.trim().parse::<u64>().ok())
        .collect()
}

/// Extract the fortran_order boolean.
fn header_fortran(header: &str) -> Option<bool> {
    let marker = "'fortran_order':";
    let at = header.find(marker)?;
    let rest = header[at + marker.len()..].trim_start();
    if rest.starts_with("True") {
        Some(true)
    } else if rest.starts_with("False") {
        Some(false)
    } else {
        None
    }
}

/// Parse one NPY member header from an in-memory prefix of the payload.
fn parse_npy_header_bytes(payload: &[u8]) -> Result<Option<NpyHeader>, NnError> {
    if payload.len() < 10 || payload[..6] != NPY_MAGIC {
        return Ok(None); // not an NPY payload — caller decides
    }
    let version = [payload[6], payload[7]];
    if version[0] > 3 {
        return Err(NnError::MalformedInput {
            detail: format!("npy: unsupported major version {}", version[0]),
        });
    }
    let (len_width, len_at): (usize, usize) = if version[0] == 1 {
        (2, 8)
    } else {
        (4, 12) // v2/v3: 4-byte length preceded by 2 padding bytes
    };
    if payload.len() < len_at + len_width {
        return Err(NnError::MalformedInput {
            detail: "npy: header truncated before the dictionary length".to_string(),
        });
    }
    let len_bytes = &payload[len_at..len_at + len_width];
    let dict_len = if len_width == 2 {
        u16le(len_bytes, 0) as u64
    } else {
        u32le(len_bytes, 0) as u64
    } as usize;
    let dict_start = len_at + len_width;
    if payload.len() < dict_start + dict_len {
        return Err(NnError::MalformedInput {
            detail: "npy: header dictionary truncated".to_string(),
        });
    }
    let dict = &payload[dict_start..dict_start + dict_len];
    let text = String::from_utf8_lossy(dict).into_owned();
    let mut findings = Vec::new();
    let descr = header_string(&text, "descr").ok_or_else(|| NnError::MalformedInput {
        detail: "npy: header carries no descr".to_string(),
    })?;
    let shape = header_shape(&text).ok_or_else(|| NnError::MalformedInput {
        detail: "npy: header carries no shape".to_string(),
    })?;
    let fortran_order = header_fortran(&text).ok_or_else(|| NnError::MalformedInput {
        detail: "npy: header carries no fortran_order".to_string(),
    })?;
    let (encoding, element_size) = match descr_encoding(&descr) {
        Some(pair) => pair,
        None => {
            findings.push(Finding::warning(
                "NPY_UNSUPPORTED_DTYPE",
                format!("descr {descr} has no qualified scalar codec; descriptor retained"),
            ));
            ("numpy.unknown", 0)
        }
    };
    if fortran_order {
        findings.push(Finding::warning(
            "NPY_FORTRAN_ORDER",
            "fortran_order storage is not C-contiguous; extent bounded to the member payload",
        ));
    }
    let header_len = (dict_start + dict_len) as u64;
    Ok(Some(NpyHeader {
        encoding,
        element_size,
        shape,
        fortran_order,
        header_len,
        findings,
    }))
}

/// Parse one NPY member header starting at a file offset (stored members).
fn parse_npy_header(
    file: &BoundedFile,
    offset: u64,
    budget: &Budget,
) -> Result<Option<NpyHeader>, NnError> {
    // 10 bytes identify the format; then peek the length fields.
    let prefix = super::zip::read_at(file, offset, 16, budget)?;
    if prefix.len() < 10 || prefix[..6] != NPY_MAGIC {
        return Ok(None);
    }
    let len_width = if prefix[6] == 1 { 2 } else { 4 };
    let len_at = if prefix[6] == 1 { 8 } else { 12 };
    if prefix.len() < len_at + len_width {
        return Err(NnError::MalformedInput {
            detail: "npy: header truncated before the dictionary length".to_string(),
        });
    }
    let dict_len = if len_width == 2 {
        u16le(&prefix, len_at) as u64
    } else {
        u32le(&prefix, len_at) as u64
    };
    let total = len_at as u64 + len_width as u64 + dict_len;
    let payload = super::zip::read_at(file, offset, total as usize, budget)?;
    parse_npy_header_bytes(&payload)
}

/// Bounded DEFLATE prefix decompression: enough of the member to read its
/// NPY header. Pure data decoding — nothing executes; compressed bytes
/// read and decompressed bytes produced are both budget-charged.
fn deflate_prefix(
    file: &BoundedFile,
    entry: &ZipEntry,
    budget: &Budget,
) -> Result<(Vec<u8>, bool), NnError> {
    use std::io::Read as _;
    const COMPRESSED_CAP: u64 = 4 * 1024 * 1024;
    const DECOMPRESSED_CAP: usize = 256 * 1024;
    let take = entry.stored_size.min(COMPRESSED_CAP);
    let compressed = super::zip::read_at(file, entry.data_offset, take as usize, budget)?;
    budget.consume_metadata(take)?;
    let mut decoder = flate2::read::DeflateDecoder::new(&compressed[..]);
    let mut out = Vec::with_capacity(4096);
    let mut chunk = [0u8; 8192];
    let mut complete = true;
    loop {
        match decoder.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                out.extend_from_slice(&chunk[..n]);
                budget.consume_metadata(n as u64)?;
                if out.len() >= DECOMPRESSED_CAP {
                    complete = false;
                    break;
                }
            }
            Err(err) => {
                return Err(NnError::MalformedInput {
                    detail: format!("npz: member {} failed to decompress: {err}", entry.name),
                })
            }
        }
    }
    Ok((out, complete))
}

fn tensor_entry(
    name: String,
    header: &NpyHeader,
    payload_start: u64,
    stored_span: Option<u64>,
    archive_note: Option<String>,
) -> TensorEntry {
    let element_count: u64 = header.shape.iter().product();
    let (payload_length, extent, decode_supported) =
        if header.element_size == 0 || header.fortran_order || header.encoding == "numpy.unknown" {
            (None, Extent::UpperBoundOnly, false)
        } else {
            let bytes = element_count.saturating_mul(header.element_size);
            // The declared bytes must fit whatever storage backs the member.
            if let Some(span) = stored_span {
                if bytes > span {
                    return TensorEntry {
                        original_name: name.clone(),
                        encoding: header.encoding.to_string(),
                        decode_supported: false,
                        shape: header.shape.clone(),
                        element_count,
                        payload_start,
                        payload_length: None,
                        extent: Extent::UpperBoundOnly,
                    };
                }
            }
            (
                Some(bytes),
                Extent::Exact,
                encoding_decode_supported(header.encoding),
            )
        };
    let _ = archive_note; // archive locations surface as inventory findings
    TensorEntry {
        original_name: name,
        encoding: header.encoding.to_string(),
        decode_supported,
        shape: header.shape.clone(),
        element_count,
        payload_start,
        payload_length,
        extent,
    }
}

fn report(
    format: &str,
    tensors: Vec<TensorEntry>,
    mut findings: Vec<Finding>,
    metadata_bytes: u64,
) -> FormatInventory {
    let has_error = findings.iter().any(|f| f.severity == Severity::Error);
    findings.retain(|_| true);
    FormatInventory {
        format: format.to_string(),
        format_version: "1".to_string(),
        validity: if has_error {
            Validity::Invalid
        } else {
            Validity::Valid
        },
        tensors,
        findings,
        metadata_bytes,
        payload_bytes: 0,
    }
}

/// Inventory a bare `.npy` file.
pub fn inventory_npy(file: &BoundedFile, budget: &Budget) -> Result<FormatInventory, NnError> {
    let header = parse_npy_header(file, 0, budget)?.ok_or_else(|| NnError::MalformedInput {
        detail: "npy: magic mismatch".to_string(),
    })?;
    let mut findings = header.findings.clone();
    let length = file.length();
    let element_count: u64 = header.shape.iter().product();
    let declared = element_count.saturating_mul(header.element_size.max(1));
    let available = length.saturating_sub(header.header_len);
    if header.element_size > 0 && declared > available {
        findings.push(Finding::error(
            "NPY_SIZE_MISMATCH",
            format!("payload declares {declared} bytes but only {available} remain in the file"),
        ));
    }
    let tensors = vec![tensor_entry(
        "array".to_string(),
        &header,
        header.header_len,
        Some(length.saturating_sub(header.header_len)),
        None,
    )];
    Ok(report("numpy", tensors, findings, header.header_len))
}

/// Inventory an `.npz` archive of `.npy` members.
pub fn inventory_npz(file: &BoundedFile, budget: &Budget) -> Result<FormatInventory, NnError> {
    let entries = parse_zip(file, budget)?;
    let mut tensors = Vec::new();
    let mut findings = Vec::new();
    let mut metadata_bytes = 0u64;
    for entry in &entries {
        let ZipEntry {
            name,
            compression,
            data_offset,
            stored_size,
            uncompressed_size,
        } = entry;
        if !name.ends_with(".npy") {
            findings.push(Finding::info(
                "NPZ_NON_NPY_MEMBER",
                format!("{name} is not an .npy member; retained as archive evidence"),
            ));
            metadata_bytes += stored_size;
            continue;
        }
        let member_name = name.trim_end_matches(".npy").to_string();
        let (header, decompressed_prefix_complete) = if *compression == 8 {
            let (prefix, complete) = deflate_prefix(file, entry, budget)?;
            match parse_npy_header_bytes(&prefix)? {
                Some(header) => (header, complete),
                None => {
                    findings.push(Finding::warning(
                        "NPZ_MEMBER_NOT_NPY",
                        format!("{name} lacks the NPY magic despite its extension"),
                    ));
                    continue;
                }
            }
        } else {
            match parse_npy_header(file, *data_offset, budget)? {
                Some(header) => (header, true),
                None => {
                    findings.push(Finding::warning(
                        "NPZ_MEMBER_NOT_NPY",
                        format!("{name} lacks the NPY magic despite its extension"),
                    ));
                    continue;
                }
            }
        };
        if !decompressed_prefix_complete {
            findings.push(Finding::info(
                "NPZ_PREFIX_ONLY",
                format!("{member_name}: header read from a bounded decompressed prefix"),
            ));
        }
        metadata_bytes += header.header_len;
        let payload_start = data_offset + header.header_len;
        match *compression {
            0 => {
                // STORED: payload bytes sit at file-qualified offsets.
                let available = stored_size.saturating_sub(header.header_len);
                for finding in &header.findings {
                    findings.push(Finding {
                        code: format!("NPY_{}", finding_code_suffix(finding)),
                        severity: finding.severity,
                        message: format!("{member_name}: {}", finding.message),
                    });
                }
                tensors.push(tensor_entry(
                    member_name,
                    &header,
                    payload_start,
                    Some(available),
                    None,
                ));
            }
            8 => {
                // DEFLATED: payload addresses exist only in the decompressed
                // stream. The archive location is known exactly; the NPY
                // payload is bounded, never claimed as file-qualified bytes.
                findings.push(Finding::info(
                    "NPZ_COMPRESSED_MEMBER",
                    format!(
                        "{member_name}: deflate-compressed; archive span [{data_offset}, {}) is exact, array extents are logical (decompressed) coordinates",
                        data_offset + stored_size
                    ),
                ));
                let element_count: u64 = header.shape.iter().product();
                let declared = element_count.saturating_mul(header.element_size);
                if header.element_size > 0 && declared > *uncompressed_size {
                    findings.push(Finding::error(
                        "NPY_SIZE_MISMATCH",
                        format!(
                            "{member_name}: declares {declared} payload bytes over a decompressed member of {uncompressed_size}"
                        ),
                    ));
                }
                for finding in &header.findings {
                    findings.push(Finding {
                        code: format!("NPY_{}", finding_code_suffix(finding)),
                        severity: finding.severity,
                        message: format!("{member_name}: {}", finding.message),
                    });
                }
                // Exact extent only when the decompressed size equals the
                // declared size AND the dtype/decode path is honest; the
                // payload_start points at the compressed data, so exact
                // file-qualified spans are impossible — bounded extent with
                // the archive location recorded in the finding above.
                tensors.push(TensorEntry {
                    original_name: member_name,
                    encoding: header.encoding.to_string(),
                    decode_supported: false,
                    shape: header.shape.clone(),
                    element_count,
                    payload_start: *data_offset,
                    payload_length: None,
                    extent: Extent::UpperBoundOnly,
                });
            }
            other => {
                findings.push(Finding::warning(
                    "NPZ_UNSUPPORTED_COMPRESSION",
                    format!("{member_name}: compression method {other} is not supported"),
                ));
            }
        }
    }
    if tensors.is_empty() && findings.iter().all(|f| f.severity != Severity::Error) {
        findings.push(Finding::error(
            "NPZ_NO_ARRAYS",
            "the archive contains no readable .npy members".to_string(),
        ));
    }
    Ok(report("numpy-archive", tensors, findings, metadata_bytes))
}

fn finding_code_suffix(finding: &Finding) -> String {
    finding
        .code
        .strip_prefix("NPY_")
        .unwrap_or(&finding.code)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn budget() -> crate::nn::budget::Budget {
        crate::nn::budget::Budget::unrestricted()
    }

    fn write_npy(path: &std::path::Path, header: &str, payload: &[u8]) {
        let mut out = Vec::new();
        out.extend_from_slice(&NPY_MAGIC);
        out.extend_from_slice(&[1, 0]);
        out.extend_from_slice(&(header.len() as u16).to_le_bytes());
        out.extend_from_slice(header.as_bytes());
        while out.len() % 64 != 0 {
            out.push(b' ');
        }
        out.extend_from_slice(payload);
        std::fs::write(path, out).unwrap();
    }

    #[test]
    fn inventories_bare_npy_with_exact_extent() {
        let dir = tempfile::tempdir().unwrap();
        write_npy(
            &dir.path().join("a.npy"),
            "{'descr': '<f4', 'fortran_order': False, 'shape': (2, 2), }",
            &[0u8; 16],
        );
        let file = BoundedFile::open(&dir.path().join("a.npy")).unwrap();
        let inv = inventory_npy(&file, &budget()).unwrap();
        assert_eq!(inv.format, "numpy");
        assert_eq!(inv.tensors.len(), 1);
        let t = &inv.tensors[0];
        assert_eq!(t.encoding, "numpy.float32");
        assert_eq!(t.shape, vec![2, 2]);
        assert_eq!(t.payload_length, Some(16));
        assert_eq!(t.extent, Extent::Exact);
        assert!(t.decode_supported);
    }

    #[test]
    fn one_element_shape_and_unsupported_dtypes() {
        let dir = tempfile::tempdir().unwrap();
        // (3,) — the trailing comma of a 1-tuple.
        write_npy(
            &dir.path().join("one.npy"),
            "{'descr': '<f2', 'fortran_order': False, 'shape': (3,), }",
            &[0u8; 6],
        );
        let file = BoundedFile::open(&dir.path().join("one.npy")).unwrap();
        let inv = inventory_npy(&file, &budget()).unwrap();
        assert_eq!(inv.tensors[0].shape, vec![3]);
        assert_eq!(inv.tensors[0].encoding, "numpy.float16");

        // Structured dtype: descriptor retained, no codec, honest.
        write_npy(
            &dir.path().join("rec.npy"),
            "{'descr': [('a', '<f4'), ('b', '<i2')], 'fortran_order': False, 'shape': (2,), }",
            &[0u8; 12],
        );
        let file = BoundedFile::open(&dir.path().join("rec.npy")).unwrap();
        let inv = inventory_npy(&file, &budget()).unwrap();
        assert_eq!(inv.tensors[0].encoding, "numpy.unknown");
        assert!(!inv.tensors[0].decode_supported);
        assert!(inv
            .findings
            .iter()
            .any(|f| f.code == "NPY_UNSUPPORTED_DTYPE"));

        // Fortran order: bounded extent + finding.
        write_npy(
            &dir.path().join("f.npy"),
            "{'descr': '<f4', 'fortran_order': True, 'shape': (2, 3), }",
            &[0u8; 24],
        );
        let file = BoundedFile::open(&dir.path().join("f.npy")).unwrap();
        let inv = inventory_npy(&file, &budget()).unwrap();
        assert_eq!(inv.tensors[0].extent, Extent::UpperBoundOnly);
        assert!(inv.findings.iter().any(|f| f.code == "NPY_FORTRAN_ORDER"));

        // Declared payload exceeds the file.
        write_npy(
            &dir.path().join("big.npy"),
            "{'descr': '<f8', 'fortran_order': False, 'shape': (1024,), }",
            &[0u8; 8],
        );
        let file = BoundedFile::open(&dir.path().join("big.npy")).unwrap();
        let inv = inventory_npy(&file, &budget()).unwrap();
        assert!(inv.findings.iter().any(|f| f.code == "NPY_SIZE_MISMATCH"));
    }

    #[test]
    fn npz_stored_exact_and_deflated_logical() {
        // Build with python? No: hand-build a minimal zip with one STORED
        // npy member and one DEFLATED member.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cap.npz");
        let mut npy: Vec<u8> = Vec::new();
        npy.extend_from_slice(&NPY_MAGIC);
        npy.extend_from_slice(&[1, 0]);
        let header = "{'descr': '<f4', 'fortran_order': False, 'shape': (4,), }";
        npy.extend_from_slice(&(header.len() as u16).to_le_bytes());
        npy.extend_from_slice(header.as_bytes());
        npy.extend_from_slice(&[0u8; 16]);

        let mut out: Vec<u8> = Vec::new();
        let mut central: Vec<u8> = Vec::new();
        let mut offsets = Vec::new();
        // Local header: sig(4) ver/flags/method/time/date(12) crc(4)
        // csize(4) usize(4) namelen(2) extralen(2) = 30 bytes, then name.
        let mut local_header =
            |out: &mut Vec<u8>, name: &str, method: u16, csize: u32, usize_: u32| {
                offsets.push(out.len() as u32);
                out.extend_from_slice(&crate::nn::format::zip::LOCAL_MAGIC);
                out.extend_from_slice(&[0; 4]); // ver/flags
                out.extend_from_slice(&method.to_le_bytes());
                out.extend_from_slice(&[0; 4]); // time/date
                out.extend_from_slice(&0u32.to_le_bytes()); // crc
                out.extend_from_slice(&csize.to_le_bytes());
                out.extend_from_slice(&usize_.to_le_bytes());
                out.extend_from_slice(&(name.len() as u16).to_le_bytes());
                out.extend_from_slice(&0u16.to_le_bytes());
                out.extend_from_slice(name.as_bytes());
            };
        // STORED member "a.npy"
        local_header(&mut out, "a.npy", 0, npy.len() as u32, npy.len() as u32);
        out.extend_from_slice(&npy);
        // DEFLATED member "b.npy": deflate the same payload.
        use std::io::Write as _;
        let mut enc =
            flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
        enc.write_all(&npy).unwrap();
        let deflated = enc.finish().unwrap();
        local_header(
            &mut out,
            "b.npy",
            8,
            deflated.len() as u32,
            npy.len() as u32,
        );
        out.extend_from_slice(&deflated);
        // Central directory: sig(4) made/needed/flags/method/time/date(12)
        // crc(4) csize(4) usize(4) namelen(2) extra/comment/disk/iattr(8)
        // eattr(4) offset(4) = 46 bytes, then name.
        for ((name, method, csize, usize_), offset) in [
            ("a.npy", 0u16, npy.len() as u32, npy.len() as u32),
            ("b.npy", 8u16, deflated.len() as u32, npy.len() as u32),
        ]
        .iter()
        .zip(&offsets)
        {
            central.extend_from_slice(&crate::nn::format::zip::CD_ENTRY_MAGIC);
            central.extend_from_slice(&[0; 6]); // made/needed/flags
            central.extend_from_slice(&method.to_le_bytes());
            central.extend_from_slice(&[0; 4]); // time/date
            central.extend_from_slice(&0u32.to_le_bytes()); // crc
            central.extend_from_slice(&csize.to_le_bytes());
            central.extend_from_slice(&usize_.to_le_bytes());
            central.extend_from_slice(&(name.len() as u16).to_le_bytes());
            central.extend_from_slice(&[0; 8]);
            central.extend_from_slice(&0u32.to_le_bytes()); // eattr
            central.extend_from_slice(&offset.to_le_bytes());
            central.extend_from_slice(name.as_bytes());
        }
        let cd_offset = out.len() as u32;
        out.extend_from_slice(&central);
        out.extend_from_slice(&crate::nn::format::zip::EOCD_MAGIC);
        out.extend_from_slice(&[0; 4]);
        out.extend_from_slice(&2u16.to_le_bytes());
        out.extend_from_slice(&2u16.to_le_bytes());
        out.extend_from_slice(&(central.len() as u32).to_le_bytes());
        out.extend_from_slice(&cd_offset.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        std::fs::write(&path, out).unwrap();

        let file = BoundedFile::open(&path).unwrap();
        let inv = inventory_npz(&file, &budget()).unwrap();
        assert_eq!(inv.tensors.len(), 2);
        let a = inv.tensors.iter().find(|t| t.original_name == "a").unwrap();
        assert_eq!(a.extent, Extent::Exact);
        assert_eq!(a.payload_length, Some(16));
        assert!(a.decode_supported);
        let b = inv.tensors.iter().find(|t| t.original_name == "b").unwrap();
        assert_eq!(b.extent, Extent::UpperBoundOnly);
        assert!(!b.decode_supported); // decompressed coordinates, not file bytes
        assert!(inv
            .findings
            .iter()
            .any(|f| f.code == "NPZ_COMPRESSED_MEMBER" && f.message.contains("b:")));
        assert_eq!(b.shape, vec![4]);
        assert_eq!(b.encoding, "numpy.float32");
    }
}
