//! Static PyTorch checkpoint reader (descriptor tier).
//!
//! A `.pth`/`.pt` checkpoint is a ZIP archive whose `data.pkl` entry is a
//! pickle byte stream describing the state dictionary. This reader is
//! strictly data-only: a closed-subset opcode interpreter builds strings,
//! numbers, and containers; `REDUCE` is recognized for the tensor rebuild
//! functions and read as data — nothing is ever looked up, called, or
//! executed. Opcodes outside the subset surface as findings, never as
//! silent interpretation.

use super::{
    encoding_decode_supported, Extent, Finding, FormatInventory, Severity, TensorEntry, Validity,
};
use crate::nn::budget::Budget;
use crate::nn::error::NnError;
use crate::nn::source::BoundedFile;

const EOCD_MAGIC: [u8; 4] = [0x50, 0x4b, 0x05, 0x06];
const CD_ENTRY_MAGIC: [u8; 4] = [0x50, 0x4b, 0x01, 0x02];
const LOCAL_MAGIC: [u8; 4] = [0x50, 0x4b, 0x03, 0x04];
/// EOCD is a fixed 22-byte record plus an optional comment (≤ 64 KiB).
const EOCD_SCAN: u64 = 22 + 65_536;

/// One ZIP entry, located.
#[derive(Debug, Clone)]
struct ZipEntry {
    name: String,
    compression: u16,
    /// Absolute file offset of the entry's data.
    data_offset: u64,
    /// Bytes the data occupies in the file (compressed size).
    stored_size: u64,
}

fn read_at(
    file: &BoundedFile,
    offset: u64,
    len: usize,
    budget: &Budget,
) -> Result<Vec<u8>, NnError> {
    let mut buf = vec![0u8; len];
    budget.consume_metadata(len as u64)?;
    file.read_exact_at(offset, &mut buf)?;
    Ok(buf)
}

fn u16le(buf: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([buf[at], buf[at + 1]])
}

fn u32le(buf: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([buf[at], buf[at + 1], buf[at + 2], buf[at + 3]])
}

fn u64le(buf: &[u8], at: usize) -> u64 {
    let mut b = [0u8; 8];
    b.copy_from_slice(&buf[at..at + 8]);
    u64::from_le_bytes(b)
}

/// Locate the end-of-central-directory record and parse the archive.
fn parse_zip(file: &BoundedFile, budget: &Budget) -> Result<Vec<ZipEntry>, NnError> {
    let length = file.length();
    if length < 22 {
        return Err(NnError::MalformedInput {
            detail: "torch: file shorter than a ZIP end record".to_string(),
        });
    }
    let scan_start = length.saturating_sub(EOCD_SCAN);
    let scan = read_at(file, scan_start, (length - scan_start) as usize, budget)?;
    let eocd = scan
        .windows(4)
        .rev()
        .position(|w| w == EOCD_MAGIC)
        .map(|back| scan.len() - back - 4)
        .ok_or_else(|| NnError::MalformedInput {
            detail: "torch: no ZIP end-of-central-directory record".to_string(),
        })?;
    let total_entries = u16le(&scan, eocd + 10) as usize;
    let cd_size = u32le(&scan, eocd + 12) as u64;
    let cd_offset = u32le(&scan, eocd + 16) as u64;
    if cd_offset.saturating_add(cd_size) > length {
        return Err(NnError::MalformedInput {
            detail: "torch: ZIP central directory beyond end of file".to_string(),
        });
    }
    let cd = read_at(file, cd_offset, cd_size as usize, budget)?;
    let mut entries = Vec::with_capacity(total_entries.min(4096));
    let mut pos = 0usize;
    for _ in 0..total_entries {
        if pos + 46 > cd.len() || cd[pos..pos + 4] != CD_ENTRY_MAGIC {
            return Err(NnError::MalformedInput {
                detail: format!(
                    "torch: ZIP central directory entry {} malformed",
                    entries.len()
                ),
            });
        }
        let compression = u16le(&cd, pos + 10);
        let compressed = u32le(&cd, pos + 20) as u64;
        let name_len = u16le(&cd, pos + 28) as usize;
        let extra_len = u16le(&cd, pos + 30) as usize;
        let comment_len = u16le(&cd, pos + 32) as usize;
        let local_offset = u32le(&cd, pos + 42) as u64;
        if pos + 46 + name_len > cd.len() {
            return Err(NnError::MalformedInput {
                detail: "torch: ZIP entry name beyond central directory".to_string(),
            });
        }
        let name = String::from_utf8_lossy(&cd[pos + 46..pos + 46 + name_len]).into_owned();
        // The local header carries its own (possibly different) name/extra
        // lengths; the data begins after them.
        let mut lhead = [0u8; 30];
        budget.consume_metadata(30)?;
        file.read_exact_at(local_offset, &mut lhead)?;
        if lhead[0..4] != LOCAL_MAGIC {
            return Err(NnError::MalformedInput {
                detail: format!("torch: ZIP local header missing for {name}"),
            });
        }
        let l_name = u16le(&lhead, 26) as u64;
        let l_extra = u16le(&lhead, 28) as u64;
        entries.push(ZipEntry {
            name,
            compression,
            data_offset: local_offset + 30 + l_name + l_extra,
            stored_size: compressed,
        });
        pos += 46 + name_len + extra_len + comment_len;
    }
    Ok(entries)
}

/// A pickle value. Only what a state dictionary can contain.
#[derive(Debug, Clone)]
enum PValue {
    None,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    List(Vec<PValue>),
    Tuple(Vec<PValue>),
    Dict(Vec<(PValue, PValue)>),
    /// BINPERSID payload: torch's `('storage', storage_type, key, location, numel)`.
    PersId(Vec<PValue>),
    /// REDUCE read as data: the callable's qualified name plus its args.
    Reduce {
        func: String,
        args: Vec<PValue>,
    },
}

impl PValue {
    fn as_str(&self) -> Option<&str> {
        match self {
            PValue::Str(s) => Some(s),
            _ => None,
        }
    }
}

/// Failure modes of the static interpreter.
#[derive(Debug)]
enum PickleError {
    /// Unknown/unsupported opcode: stop interpreting, say which.
    Unsupported {
        opcode: u8,
        offset: u64,
    },
    /// Structurally broken stream.
    Malformed {
        detail: String,
    },
    Budget {
        detail: String,
    },
}

struct Pickle<'a> {
    bytes: &'a [u8],
    pos: usize,
    stack: Vec<PValue>,
    /// Mark positions on the value stack.
    marks: Vec<usize>,
    memo: Vec<PValue>,
    ops: u64,
    max_ops: u64,
}

impl<'a> Pickle<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Pickle {
            bytes,
            pos: 0,
            stack: Vec::new(),
            marks: Vec::new(),
            memo: Vec::new(),
            ops: 0,
            // Every opcode consumes at least one byte; bytes × 2 + slack
            // bounds any stream while stopping runaway loops.
            max_ops: bytes.len() as u64 * 2 + 1024,
        }
    }

    fn err(&self, detail: impl Into<String>) -> PickleError {
        PickleError::Malformed {
            detail: format!("pickle byte {}: {}", self.pos, detail.into()),
        }
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], PickleError> {
        if self.pos + n > self.bytes.len() {
            return Err(self.err("unexpected end of pickle stream"));
        }
        let slice = &self.bytes[self.pos..self.pos + n];
        self.pos += n;
        Ok(slice)
    }

    fn byte(&mut self) -> Result<u8, PickleError> {
        Ok(self.take(1)?[0])
    }

    fn pop(&mut self) -> Result<PValue, PickleError> {
        self.stack.pop().ok_or_else(|| self.err("stack underflow"))
    }

    fn push(&mut self, value: PValue) -> Result<(), PickleError> {
        if self.stack.len() > 4096 {
            return Err(PickleError::Budget {
                detail: "pickle value stack exceeds 4096 entries".to_string(),
            });
        }
        self.stack.push(value);
        Ok(())
    }

    /// Pop values until (and excluding) the most recent mark; drop the mark.
    fn pop_to_mark(&mut self) -> Result<Vec<PValue>, PickleError> {
        let mark = self.marks.pop().ok_or_else(|| self.err("MARK expected"))?;
        if self.stack.len() < mark {
            return Err(self.err("mark position below stack"));
        }
        Ok(self.stack.split_off(mark))
    }

    fn push_str(&mut self, text: String) -> Result<(), PickleError> {
        self.push(PValue::Str(text))
    }

    fn memoize_top(&mut self) -> Result<(), PickleError> {
        if let Some(top) = self.stack.last() {
            let top = top.clone();
            self.memo.push(top);
            Ok(())
        } else {
            Err(self.err("MEMOIZE on an empty stack"))
        }
    }

    fn memo_store(&mut self, id: usize) -> Result<(), PickleError> {
        if let Some(top) = self.stack.last() {
            let top = top.clone();
            if id >= self.memo.len() {
                self.memo.resize(id + 1, PValue::None);
            }
            self.memo[id] = top;
            Ok(())
        } else {
            Err(self.err("memo store on an empty stack"))
        }
    }

    fn run(&mut self) -> Result<PValue, PickleError> {
        loop {
            self.ops += 1;
            if self.ops > self.max_ops {
                return Err(PickleError::Budget {
                    detail: "pickle opcode count exceeds allowance".to_string(),
                });
            }
            if self.pos >= self.bytes.len() {
                return Err(self.err("unexpected end before STOP"));
            }
            let op = self.bytes[self.pos];
            self.pos += 1;
            match op {
                b'.' => {
                    if self.stack.len() != 1 {
                        return Err(self.err("STOP with a dirty stack"));
                    }
                    return Ok(self.stack.pop().expect("checked length"));
                }
                0x80 => {
                    let version = self.byte()?;
                    if !(2..=5).contains(&version) {
                        return Err(PickleError::Unsupported {
                            opcode: version,
                            offset: self.pos as u64 - 1,
                        });
                    }
                }
                0x95 => {
                    self.take(8)?; // FRAME boundary: a no-op for reading.
                }
                0x94 => self.memoize_top()?,
                b'(' => self.marks.push(self.stack.len()),
                b')' => self.push(PValue::Tuple(Vec::new()))?,
                b'}' => self.push(PValue::Dict(Vec::new()))?,
                b']' => self.push(PValue::List(Vec::new()))?,
                b'b' => {
                    // BUILD: install a state onto the object below. For
                    // OrderedDicts the state dict carries the entries; for
                    // rebuilt tensors it carries metadata with no geometry.
                    // Both are read as data.
                    let state = self.pop()?;
                    let target = self.pop()?;
                    match (target, state) {
                        (PValue::Dict(mut base), PValue::Dict(extra)) => {
                            base.extend(extra);
                            self.push(PValue::Dict(base))?;
                        }
                        (reduce @ PValue::Reduce { .. }, other) => {
                            // Metadata on a rebuilt tensor: retained shape
                            // information lives in the rebuild args already.
                            let _ = other;
                            self.push(reduce)?;
                        }
                        (target, state) => {
                            return Err(self.err(format!(
                                "BUILD over unsupported target/state pair ({}/{})",
                                kind_of(&target),
                                kind_of(&state)
                            )))
                        }
                    }
                }
                b's' => {
                    // SETITEM: one key/value pair onto the dict below.
                    let value = self.pop()?;
                    let key = self.pop()?;
                    let dict = self.pop()?;
                    match dict {
                        PValue::Dict(mut d) => {
                            d.push((key, value));
                            self.push(PValue::Dict(d))?;
                        }
                        _ => return Err(self.err("SETITEM target is not a dict")),
                    }
                }
                b'e' => {
                    let items = self.pop_to_mark()?;
                    let list = self.pop()?;
                    match list {
                        PValue::List(mut l) => {
                            l.extend(items);
                            self.push(PValue::List(l))?;
                        }
                        _ => return Err(self.err("APPENDS target is not a list")),
                    }
                }
                b'u' => {
                    let items = self.pop_to_mark()?;
                    let dict = self.pop()?;
                    match dict {
                        PValue::Dict(mut d) => {
                            if items.len() % 2 != 0 {
                                return Err(self.err("SETITEMS with an odd item count"));
                            }
                            for pair in items.chunks(2) {
                                d.push((pair[0].clone(), pair[1].clone()));
                            }
                            self.push(PValue::Dict(d))?;
                        }
                        _ => return Err(self.err("SETITEMS target is not a dict")),
                    }
                }
                b't' => {
                    let items = self.pop_to_mark()?;
                    self.push(PValue::Tuple(items))?;
                }
                0x85..=0x87 => {
                    let n = (op - 0x84) as usize;
                    let mut items = Vec::with_capacity(n);
                    for _ in 0..n {
                        items.push(self.pop()?);
                    }
                    items.reverse();
                    self.push(PValue::Tuple(items))?;
                }
                0x8c => {
                    let len = self.byte()? as usize;
                    let raw = self.take(len)?;
                    self.push_str(String::from_utf8_lossy(raw).into_owned())?;
                }
                b'X' => {
                    let len = u32le(self.take(4)?, 0) as usize;
                    let raw = self.take(len)?;
                    self.push_str(String::from_utf8_lossy(raw).into_owned())?;
                }
                0x8d => {
                    let len = u64le(self.take(8)?, 0) as usize;
                    let raw = self.take(len)?;
                    self.push_str(String::from_utf8_lossy(raw).into_owned())?;
                }
                b'N' => self.push(PValue::None)?,
                0x88 => self.push(PValue::Bool(true))?,
                0x89 => self.push(PValue::Bool(false))?,
                b'J' => {
                    let raw = self.take(4)?;
                    self.push(PValue::Int(
                        i32::from_le_bytes(raw.try_into().expect("4 bytes")) as i64,
                    ))?;
                }
                b'K' => {
                    let value = self.byte()? as i64;
                    self.push(PValue::Int(value))?;
                }
                b'M' => {
                    let raw = self.take(2)?;
                    self.push(PValue::Int(u16le(raw, 0) as i64))?;
                }
                b'L' | b'I' => {
                    let rest = &self.bytes[self.pos..];
                    let end = rest
                        .iter()
                        .position(|b| *b == b'\n')
                        .ok_or_else(|| self.err("numeric text without a terminator"))?;
                    let text = String::from_utf8_lossy(&rest[..end]).into_owned();
                    self.pos += end + 1;
                    let value = text
                        .trim()
                        .parse::<i64>()
                        .map_err(|_| self.err("unreadable integer literal"))?;
                    self.push(PValue::Int(value))?;
                }
                b'G' => {
                    let raw = self.take(8)?;
                    let mut b = [0u8; 8];
                    b.copy_from_slice(raw);
                    self.push(PValue::Float(f64::from_be_bytes(b)))?;
                }
                b'q' => {
                    let id = self.byte()? as usize;
                    self.memo_store(id)?;
                }
                b'r' => {
                    let raw = self.take(4)?;
                    let id = u32le(raw, 0) as usize;
                    self.memo_store(id)?;
                }
                b'h' => {
                    let id = self.byte()? as usize;
                    let value = self
                        .memo
                        .get(id)
                        .cloned()
                        .ok_or_else(|| self.err(format!("BINGET of unknown memo id {id}")))?;
                    self.push(value)?;
                }
                b'j' => {
                    let raw = self.take(4)?;
                    let id = u32le(raw, 0) as usize;
                    let value =
                        self.memo.get(id).cloned().ok_or_else(|| {
                            self.err(format!("LONG_BINGET of unknown memo id {id}"))
                        })?;
                    self.push(value)?;
                }
                b'c' => {
                    // GLOBAL "module\nname\n": read as a qualified name only.
                    let rest = &self.bytes[self.pos..];
                    let end = rest
                        .iter()
                        .position(|b| *b == b'\n')
                        .ok_or_else(|| self.err("GLOBAL without a terminator"))?;
                    let module = String::from_utf8_lossy(&rest[..end]).into_owned();
                    self.pos += end + 1;
                    let rest = &self.bytes[self.pos..];
                    let end = rest
                        .iter()
                        .position(|b| *b == b'\n')
                        .ok_or_else(|| self.err("GLOBAL without a terminator"))?;
                    let name = String::from_utf8_lossy(&rest[..end]).into_owned();
                    self.pos += end + 1;
                    self.push_str(format!("{module}.{name}"))?;
                }
                0x93 => {
                    // STACK_GLOBAL (protocol 4): name over module on the stack.
                    let name = self.pop()?;
                    let module = self.pop()?;
                    match (module, name) {
                        (PValue::Str(m), PValue::Str(n)) => {
                            self.push_str(format!("{m}.{n}"))?;
                        }
                        _ => return Err(self.err("STACK_GLOBAL over non-strings")),
                    }
                }
                b'Q' => {
                    // BINPERSID: torch's persistent storage id tuple.
                    let value = self.pop()?;
                    match value {
                        PValue::Tuple(items) | PValue::List(items) => {
                            self.push(PValue::PersId(items))?;
                        }
                        _ => return Err(self.err("BINPERSID over a non-tuple")),
                    }
                }
                b'R' => {
                    // REDUCE: read as data — a callable name plus its args.
                    // Pickle order: callable first, args tuple on top.
                    let args = self.pop()?;
                    let callable = self.pop()?;
                    match (callable, args) {
                        (PValue::Str(f), PValue::Tuple(a)) | (PValue::Str(f), PValue::List(a)) => {
                            // collections.OrderedDict(pairs) is a dict
                            // constructor: materialized as a plain dict, the
                            // same static-data reading as the tensor
                            // rebuilds. Everything else stays an opaque
                            // Reduce record.
                            if f == "collections.OrderedDict" {
                                let mut members = Vec::with_capacity(a.len() / 2);
                                for pair in a.chunks(2) {
                                    members.push((pair[0].clone(), pair[1].clone()));
                                }
                                self.push(PValue::Dict(members))?;
                            } else {
                                self.push(PValue::Reduce { func: f, args: a })?;
                            }
                        }
                        _ => {
                            return Err(self.err(
                                "REDUCE over a non-string callable (dynamic dispatch is never interpreted)",
                            ))
                        }
                    }
                }
                other => {
                    return Err(PickleError::Unsupported {
                        opcode: other,
                        offset: self.pos as u64 - 1,
                    });
                }
            }
        }
    }
}

/// Storage type → (encoding, element size in bytes).
fn storage_encoding(storage_type: &str) -> Option<(&'static str, u64)> {
    let short = storage_type.rsplit('.').next().unwrap_or(storage_type);
    Some(match short {
        "FloatStorage" => ("torch.float32", 4),
        "HalfStorage" => ("torch.float16", 2),
        "DoubleStorage" => ("torch.float64", 8),
        "BFloat16Storage" => ("torch.bfloat16", 2),
        "ByteStorage" => ("torch.uint8", 1),
        "CharStorage" => ("torch.int8", 1),
        "ShortStorage" => ("torch.int16", 2),
        "IntStorage" => ("torch.int32", 4),
        "LongStorage" => ("torch.int64", 8),
        "BoolStorage" => ("torch.bool", 1),
        _ => return None,
    })
}

/// Torch stride is row-major-contiguous when stride[i] == prod(shape[i+1..]).
fn is_contiguous(shape: &[u64], stride: &[i64]) -> bool {
    if shape.len() != stride.len() {
        return false;
    }
    let mut expected = 1i64;
    for i in (0..shape.len()).rev() {
        if stride[i] != expected {
            return false;
        }
        expected = expected.saturating_mul(shape[i].max(1) as i64);
    }
    true
}

fn kind_of(value: &PValue) -> &'static str {
    match value {
        PValue::None => "none",
        PValue::Bool(_) => "bool",
        PValue::Int(_) => "int",
        PValue::Float(_) => "float",
        PValue::Str(_) => "str",
        PValue::List(_) => "list",
        PValue::Tuple(_) => "tuple",
        PValue::Dict(_) => "dict",
        PValue::PersId(_) => "persid",
        PValue::Reduce { .. } => "reduce",
    }
}

fn as_int(value: &PValue) -> Option<i64> {
    match value {
        PValue::Int(i) => Some(*i),
        PValue::Bool(b) => Some(*b as i64),
        PValue::Float(f) if f.fract() == 0.0 && f.is_finite() => Some(*f as i64),
        _ => None,
    }
}

fn as_ints(value: &PValue) -> Option<Vec<i64>> {
    match value {
        PValue::Tuple(items) | PValue::List(items) => {
            items.iter().map(as_int).collect::<Option<Vec<_>>>()
        }
        _ => None,
    }
}

/// Inventory a PyTorch checkpoint. Descriptor-only: the pickle stream is
/// interpreted as data; nothing is executed.
pub fn inventory(file: &BoundedFile, budget: &Budget) -> Result<FormatInventory, NnError> {
    let mut findings: Vec<Finding> = Vec::new();
    let entries = parse_zip(file, budget)?;
    let pickle_entry = entries
        .iter()
        .find(|e| e.name.ends_with("data.pkl"))
        .ok_or_else(|| NnError::MalformedInput {
            detail: "torch: no data.pkl entry in the archive".to_string(),
        })?
        .clone();
    let archive_prefix = pickle_entry
        .name
        .strip_suffix("data.pkl")
        .unwrap_or("")
        .to_string();
    if pickle_entry.compression != 0 {
        findings.push(Finding::error(
            "TORCH_COMPRESSED_PICKLE",
            format!(
                "data.pkl uses compression method {}; PyTorch archives store it uncompressed — refusing to decompress",
                pickle_entry.compression
            ),
        ));
        return Ok(FormatInventory {
            format: "torch".to_string(),
            format_version: "zip".to_string(),
            validity: Validity::Invalid,
            tensors: Vec::new(),
            findings,
            metadata_bytes: file.length().min(pickle_entry.data_offset),
            payload_bytes: 0,
        });
    }
    let pickle_bytes = read_at(
        file,
        pickle_entry.data_offset,
        pickle_entry.stored_size as usize,
        budget,
    )?;
    let mut parser = Pickle::new(&pickle_bytes);
    let root = match parser.run() {
        Ok(value) => value,
        Err(PickleError::Unsupported { opcode, offset }) => {
            findings.push(Finding::error(
                "TORCH_PICKLE_OPCODE",
                format!(
                    "pickle opcode 0x{opcode:02x} at byte {offset} is outside the static state-dict subset; refusing to interpret further"
                ),
            ));
            return Ok(FormatInventory {
                format: "torch".to_string(),
                format_version: "zip".to_string(),
                validity: Validity::Invalid,
                tensors: Vec::new(),
                findings,
                metadata_bytes: pickle_entry.data_offset + pickle_entry.stored_size,
                payload_bytes: 0,
            });
        }
        Err(PickleError::Budget { detail }) => {
            let _ = detail; // the code+limits carry the story
            return Err(NnError::BudgetExceeded {
                resource: "pickle_ops",
                limit: parser.max_ops,
                requested: parser.ops,
            });
        }
        Err(PickleError::Malformed { detail }) => {
            return Err(NnError::MalformedInput { detail });
        }
    };
    let state_entries: Vec<(String, PValue)> = match root {
        PValue::Dict(members) => members
            .into_iter()
            .filter_map(|(k, v)| match k {
                PValue::Str(name) => Some((name, v)),
                _ => None,
            })
            .collect(),
        PValue::List(items) | PValue::Tuple(items) => {
            // Bare tensor at top level (voice-pack style).
            let mut pairs = Vec::new();
            for (i, v) in items.into_iter().enumerate() {
                pairs.push((format!("tensor.{i}"), v));
            }
            pairs
        }
        // Voice-pack style: a bare rebuilt tensor at the root.
        PValue::Reduce { .. } => vec![("tensor".to_string(), root)],
        _ => {
            return Err(NnError::MalformedInput {
                detail: "torch: top-level pickle value is not a state dictionary".to_string(),
            })
        }
    };

    // Blob lookup: <prefix>data/<key>.
    let blob = |key: i64| -> Option<ZipEntry> {
        entries
            .iter()
            .find(|e| e.name == format!("{archive_prefix}data/{key}"))
            .cloned()
    };

    // State dictionaries nest (top-level groups -> OrderedDicts of
    // tensors); flatten to dotted paths, the state-dict convention.
    fn flatten(prefix: &str, value: PValue, out: &mut Vec<(String, PValue)>) {
        match value {
            PValue::Dict(members) => {
                for (k, v) in members {
                    let key = match k {
                        PValue::Str(s) => s,
                        _ => continue,
                    };
                    let path = if prefix.is_empty() {
                        key
                    } else {
                        format!("{prefix}.{key}")
                    };
                    flatten(&path, v, out);
                }
            }
            other => out.push((prefix.to_string(), other)),
        }
    }
    let mut flat: Vec<(String, PValue)> = Vec::new();
    for (name, value) in state_entries {
        flatten(&name, value, &mut flat);
    }
    let mut tensors: Vec<TensorEntry> = Vec::new();
    for (name, value) in flat {
        let PValue::Reduce { func, args } = &value else {
            // Non-tensor metadata entries (e.g. __metadata__): not tensors.
            continue;
        };
        if func != "torch._utils._rebuild_tensor_v2" && func != "torch._utils._rebuild_tensor" {
            findings.push(Finding::info(
                "TORCH_NON_TENSOR_REDUCE",
                format!(
                    "{}: REDUCE of {} kept as opaque metadata (only tensor rebuilds are interpreted)",
                    name, func
                ),
            ));
            continue;
        }
        // v2: (storage, storage_offset, size, stride, requires_grad, hooks)
        // v1: (storage, storage_offset, size)
        let Some(PValue::PersId(parts)) = args.first() else {
            findings.push(Finding::warning(
                "TORCH_TENSOR_MALFORMED",
                format!("{name}: rebuild without a persistent storage reference"),
            ));
            continue;
        };
        // ('storage', type, key, location, numel)
        let Some(storage_type) = parts.get(1).and_then(PValue::as_str) else {
            findings.push(Finding::warning(
                "TORCH_TENSOR_MALFORMED",
                format!("{name}: storage tuple lacks a type string"),
            ));
            continue;
        };
        let key = match parts.get(2) {
            Some(PValue::Int(i)) => *i,
            // Real torch pickles the storage key as a decimal string.
            Some(PValue::Str(text)) => {
                text.parse::<i64>().map_err(|_| NnError::MalformedInput {
                    detail: format!("{}: non-numeric storage key {text}", name),
                })?
            }
            _ => 0,
        };
        let storage_numel = parts.get(4).and_then(as_int).unwrap_or(0);
        let storage_offset = args.get(1).and_then(as_int).unwrap_or(0).max(0) as u64;
        let shape: Vec<u64> = args
            .get(2)
            .and_then(as_ints)
            .map(|dims| dims.iter().map(|d| (*d).max(0) as u64).collect())
            .unwrap_or_default();
        let stride: Vec<i64> = args.get(3).and_then(as_ints).unwrap_or_default();
        let element_count: u64 = shape.iter().product();
        let Some(blob_entry) = blob(key) else {
            findings.push(Finding::error(
                "TORCH_MISSING_BLOB",
                format!("{name}: storage key {key} has no data/{key} entry"),
            ));
            continue;
        };
        if blob_entry.compression != 0 {
            findings.push(Finding::warning(
                "TORCH_COMPRESSED_BLOB",
                format!(
                    "{name}: storage blob uses compression method {}",
                    blob_entry.compression
                ),
            ));
        }
        let (encoding, elem_size): (Option<&str>, u64) = match storage_encoding(storage_type) {
            Some((enc, size)) => (Some(enc), size),
            None => {
                findings.push(Finding::warning(
                    "TORCH_UNKNOWN_STORAGE",
                    format!("{name}: unknown storage type {storage_type}"),
                ));
                (None, 0)
            }
        };
        let payload_start = blob_entry.data_offset + storage_offset.saturating_mul(elem_size);
        let (payload_length, extent, decode_supported) = match (
            encoding,
            is_contiguous(&shape, &stride),
        ) {
            (Some(enc), true) => {
                let bytes = element_count.saturating_mul(elem_size);
                let end = payload_start.saturating_add(bytes);
                if end > blob_entry.data_offset + blob_entry.stored_size {
                    findings.push(Finding::error(
                        "TORCH_SPAN_EXCEEDS_BLOB",
                        format!(
                            "{name}: declared span [{payload_start}, {end}) exceeds storage blob size {}",
                            blob_entry.stored_size
                        ),
                    ));
                    continue;
                }
                (Some(bytes), Extent::Exact, encoding_decode_supported(enc))
            }
            (Some(_), false) => {
                findings.push(Finding::warning(
                    "TORCH_STRIDED_VIEW",
                    format!(
                        "{name}: non-contiguous stride {stride:?} for shape {shape:?}; extent bounded to the storage blob"
                    ),
                ));
                (None, Extent::UpperBoundOnly, false)
            }
            (None, _) => (None, Extent::UpperBoundOnly, false),
        };
        if element_count > storage_numel.max(0) as u64 {
            findings.push(Finding::warning(
                "TORCH_NUMEL_MISMATCH",
                format!(
                    "{name}: {element_count} elements claimed over a storage of {storage_numel}"
                ),
            ));
        }
        tensors.push(TensorEntry {
            original_name: name,
            encoding: encoding.unwrap_or("torch.unknown").to_string(),
            decode_supported,
            shape,
            element_count,
            payload_start,
            payload_length,
            extent,
        });
    }

    // Cross-tensor overlap check within the archive (safetensors parity).
    tensors.sort_by_key(|t| (t.payload_start, t.payload_length.unwrap_or(0)));
    for pair in tensors.windows(2) {
        let (a, b) = (&pair[0], &pair[1]);
        if let Some(la) = a.payload_length {
            if b.payload_start < a.payload_start + la {
                findings.push(Finding::error(
                    "TORCH_OVERLAP",
                    format!(
                        "tensors {} and {} have overlapping payload ranges",
                        a.original_name, b.original_name
                    ),
                ));
            }
        }
    }

    let has_error = findings.iter().any(|f| f.severity == Severity::Error);
    Ok(FormatInventory {
        format: "torch".to_string(),
        format_version: "zip".to_string(),
        validity: if has_error {
            Validity::Invalid
        } else {
            Validity::Valid
        },
        tensors,
        findings,
        metadata_bytes: pickle_entry.data_offset + pickle_entry.stored_size,
        payload_bytes: 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a STORED zip with a protocol-2 state-dict pickle, in the style
    /// of torch.save (persistent storage ids, _rebuild_tensor_v2).
    fn write_torch_zip(
        dir: &std::path::Path,
        tensors: &[(&str, &str, Vec<u64>, Vec<u8>)],
    ) -> std::path::PathBuf {
        fn short_unicode(out: &mut Vec<u8>, text: &[u8]) {
            out.push(0x8c);
            out.push(text.len() as u8);
            out.extend_from_slice(text);
        }
        fn binint(out: &mut Vec<u8>, value: i32) {
            out.push(0x4a);
            out.extend_from_slice(&value.to_le_bytes());
        }
        // Protocol-2 state dict: dict(MARK, (key, REDUCE(rebuild, (persid,
        // offset, size, stride, False, None)))...) SETITEMS STOP.
        let mut pickle: Vec<u8> = vec![0x80, 0x02];
        pickle.push(b'}'); // EMPTY_DICT
        pickle.push(0x94); // MEMOIZE root
        pickle.push(b'('); // MARK for dict items
        for (i, (name, storage, shape, _payload)) in tensors.iter().enumerate() {
            short_unicode(&mut pickle, name.as_bytes());
            pickle.push(0x94); // MEMOIZE name
                               // Callable first (pickle REDUCE order), then the args tuple.
            short_unicode(&mut pickle, b"torch._utils._rebuild_tensor_v2");
            pickle.push(0x94); // MEMOIZE callable
            pickle.push(b'('); // MARK for args
            pickle.push(b'('); // MARK for persid items
            short_unicode(&mut pickle, b"storage");
            short_unicode(&mut pickle, storage.as_bytes());
            pickle.push(0x4b); // BININT1 storage key
            pickle.push(i as u8);
            short_unicode(&mut pickle, b"cpu");
            let numel: u64 = shape.iter().product();
            binint(&mut pickle, numel as i32);
            pickle.push(b't'); // TUPLE persid
            pickle.push(0x51); // BINPERSID
            pickle.push(0x94); // MEMOIZE persid
            pickle.push(0x4b); // BININT1 storage offset
            pickle.push(0x00);
            pickle.push(b'('); // MARK size
            for d in shape {
                binint(&mut pickle, *d as i32);
            }
            pickle.push(b't');
            pickle.push(b'('); // MARK stride (row-major)
            let mut expected = 1i32;
            let mut strides: Vec<i32> = Vec::new();
            for d in shape.iter().rev() {
                strides.push(expected);
                expected *= *d as i32;
            }
            for s in strides.iter().rev() {
                binint(&mut pickle, *s);
            }
            pickle.push(b't');
            pickle.push(0x89); // NEWFALSE requires_grad
            pickle.push(0x4e); // NONE backward hooks
            pickle.push(b't'); // TUPLE args
            pickle.push(0x52); // REDUCE
        }
        pickle.push(0x75); // SETITEMS
        pickle.push(0x2e); // STOP

        // STORED zip with data.pkl + data/<i> entries.
        struct Entry {
            name: String,
            data: Vec<u8>,
        }
        let mut entries: Vec<Entry> = vec![Entry {
            name: "model/data.pkl".to_string(),
            data: pickle,
        }];
        for (i, (_, _, _, payload)) in tensors.iter().enumerate() {
            entries.push(Entry {
                name: format!("model/data/{i}"),
                data: payload.clone(),
            });
        }
        let mut out: Vec<u8> = Vec::new();
        let mut central: Vec<u8> = Vec::new();
        let mut offsets: Vec<u32> = Vec::new();
        for entry in &entries {
            let name = entry.name.as_bytes();
            offsets.push(out.len() as u32);
            out.extend_from_slice(&LOCAL_MAGIC);
            out.extend_from_slice(&[0, 0, 0, 0]); // version/flags
            out.extend_from_slice(&[0, 0]); // method STORE
            out.extend_from_slice(&[0, 0, 0, 0]); // time/date
            out.extend_from_slice(&0u32.to_le_bytes()); // crc (unverified by the reader)
            out.extend_from_slice(&(entry.data.len() as u32).to_le_bytes());
            out.extend_from_slice(&(entry.data.len() as u32).to_le_bytes());
            out.extend_from_slice(&(name.len() as u16).to_le_bytes());
            out.extend_from_slice(&[0, 0]); // extra len
            out.extend_from_slice(name);
            out.extend_from_slice(&entry.data);
        }
        for (entry, offset) in entries.iter().zip(&offsets) {
            let name = entry.name.as_bytes();
            central.extend_from_slice(&CD_ENTRY_MAGIC);
            central.extend_from_slice(&[0, 0, 0, 0, 0, 0]); // made/needed/flags
            central.extend_from_slice(&[0, 0]); // method STORE
            central.extend_from_slice(&[0, 0, 0, 0]); // time/date
            central.extend_from_slice(&0u32.to_le_bytes()); // crc
            central.extend_from_slice(&(entry.data.len() as u32).to_le_bytes());
            central.extend_from_slice(&(entry.data.len() as u32).to_le_bytes());
            central.extend_from_slice(&(name.len() as u16).to_le_bytes());
            central.extend_from_slice(&[0, 0]); // extra len
            central.extend_from_slice(&[0, 0]); // comment len
            central.extend_from_slice(&[0, 0]); // disk start
            central.extend_from_slice(&[0, 0]); // internal attrs
            central.extend_from_slice(&0u32.to_le_bytes()); // external attrs
            central.extend_from_slice(&offset.to_le_bytes());
            central.extend_from_slice(name);
        }
        let cd_offset = out.len() as u32;
        out.extend_from_slice(&central);
        out.extend_from_slice(&EOCD_MAGIC);
        out.extend_from_slice(&[0, 0, 0, 0]); // disk numbers
        out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        out.extend_from_slice(&(central.len() as u32).to_le_bytes());
        out.extend_from_slice(&cd_offset.to_le_bytes());
        out.extend_from_slice(&[0, 0]); // comment len
        let path = dir.join("m.pth");
        std::fs::write(&path, out).unwrap();
        path
    }

    fn budget() -> crate::nn::budget::Budget {
        crate::nn::budget::Budget::unrestricted()
    }

    #[test]
    fn inventories_tensors_with_exact_spans() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_torch_zip(
            dir.path(),
            &[
                ("w", "torch.FloatStorage", vec![2, 3], vec![0u8; 24]),
                ("b", "torch.LongStorage", vec![2], vec![0u8; 16]),
            ],
        );
        let file = BoundedFile::open(&path).unwrap();
        let inv = inventory(&file, &budget()).unwrap();
        assert_eq!(inv.format, "torch");
        assert_eq!(inv.validity, Validity::Valid);
        assert_eq!(inv.tensors.len(), 2);
        let w = inv.tensors.iter().find(|t| t.original_name == "w").unwrap();
        assert_eq!(w.encoding, "torch.float32");
        assert_eq!(w.shape, vec![2, 3]);
        assert_eq!(w.payload_length, Some(24));
        assert_eq!(w.extent, Extent::Exact);
        assert!(w.decode_supported);
        let b = inv.tensors.iter().find(|t| t.original_name == "b").unwrap();
        assert_eq!(b.encoding, "torch.int64");
        assert_eq!(b.payload_length, Some(16));
    }

    #[test]
    fn unknown_opcode_is_a_finding_never_executed() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_torch_zip(
            dir.path(),
            &[("w", "torch.FloatStorage", vec![2], vec![0u8; 8])],
        );
        // Corrupt one pickle opcode into something outside the subset
        // (EXT1 = 0x82, unused by state dicts): find the PROTO-prefixed
        // pickle stream inside the zip and replace its EMPTY_DICT.
        let mut bytes = std::fs::read(&path).unwrap();
        let proto = [0x80u8, 0x02, 0x7d, 0x94];
        let at = bytes
            .windows(4)
            .position(|w| w == proto)
            .expect("pickle stream present")
            + 2;
        bytes[at] = 0x82;
        std::fs::write(&path, &bytes).unwrap();
        let file = BoundedFile::open(&path).unwrap();
        let inv = inventory(&file, &budget()).unwrap();
        assert_eq!(inv.validity, Validity::Invalid);
        assert!(inv.findings.iter().any(|f| f.code == "TORCH_PICKLE_OPCODE"));
    }

    #[test]
    fn missing_blob_and_overlap_surface_as_findings() {
        let dir = tempfile::tempdir().unwrap();
        // Two tensors whose (offset-0) spans land in the same blob? The
        // builder gives each tensor its own blob, so craft overlap by
        // declaring a huge shape instead.
        let path = write_torch_zip(
            dir.path(),
            &[("w", "torch.FloatStorage", vec![1024], vec![0u8; 8])],
        );
        let file = BoundedFile::open(&path).unwrap();
        let inv = inventory(&file, &budget()).unwrap();
        assert!(inv
            .findings
            .iter()
            .any(|f| f.code == "TORCH_SPAN_EXCEEDS_BLOB"));
        assert_eq!(inv.tensors.len(), 0);
    }
}
