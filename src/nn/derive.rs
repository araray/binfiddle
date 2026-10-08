//! Data-only derived numerical views (BF-008).
//!
//! A small closed expression language evaluates derived quantities over
//! catalog tensors (typically NumPy capture members): arithmetic, log,
//! exp, sqrt, abs. Nothing executes; inputs are byte-addressed through
//! the bounded ArrayReader and decoded under their declared dtypes; the
//! result is written as a fresh .npy with a canonical provenance sidecar
//! recording the formula, input identities, evaluation dtype, and domain
//! violations. Derived values are labeled derived — never observed.

use super::budget::Budget;
use super::catalog::{Catalog, CatalogTensor};
use super::error::NnError;
use super::format::npy::ArrayReader;
use super::json::Json;
use super::report::ResultEnvelope;
use std::collections::BTreeMap;
use std::path::Path;

/// One parsed token.
#[derive(Debug, Clone, PartialEq)]
enum Token {
    Ident(String),
    Number(f64),
    Op(char),
    Func(&'static str),
    LParen,
    RParen,
    Comma,
}

fn tokenize(text: &str) -> Result<Vec<Token>, NnError> {
    let mut out = Vec::new();
    let bytes: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if c.is_ascii_alphabetic() || c == '_' {
            let start = i;
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == '_') {
                i += 1;
            }
            let word: String = bytes[start..i].iter().collect();
            out.push(match word.as_str() {
                "log" | "ln" => Token::Func("log"),
                "exp" => Token::Func("exp"),
                "sqrt" => Token::Func("sqrt"),
                "abs" => Token::Func("abs"),
                _ => Token::Ident(word),
            });
            continue;
        }
        if c.is_ascii_digit() || (c == '.' && i + 1 < bytes.len() && bytes[i + 1].is_ascii_digit())
        {
            let start = i;
            while i < bytes.len()
                && (bytes[i].is_ascii_digit()
                    || bytes[i] == '.'
                    || bytes[i] == 'e'
                    || bytes[i] == 'E'
                    || ((bytes[i] == '+' || bytes[i] == '-')
                        && i > start
                        && (bytes[i - 1] == 'e' || bytes[i - 1] == 'E')))
            {
                i += 1;
            }
            let word: String = bytes[start..i].iter().collect();
            let value: f64 = word.parse().map_err(|_| NnError::InvalidRequest {
                message: format!("bad number literal {word}"),
            })?;
            out.push(Token::Number(value));
            continue;
        }
        match c {
            '+' | '-' | '*' | '/' => out.push(Token::Op(c)),
            '(' => out.push(Token::LParen),
            ')' => out.push(Token::RParen),
            ',' => out.push(Token::Comma),
            other => {
                return Err(NnError::InvalidRequest {
                    message: format!("unsupported character {other:?} in expression"),
                })
            }
        }
        i += 1;
    }
    Ok(out)
}

/// Parsed expression tree.
#[derive(Debug, Clone)]
enum Expr {
    Ident(String),
    Number(f64),
    Unary(Box<Expr>),
    Binary(char, Box<Expr>, Box<Expr>),
    Call(&'static str, Box<Expr>),
}

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }
    fn next(&mut self) -> Option<Token> {
        let t = self.tokens.get(self.pos).cloned();
        if t.is_some() {
            self.pos += 1;
        }
        t
    }
    fn parse_expr(&mut self) -> Result<Expr, NnError> {
        let mut left = self.parse_term()?;
        while let Some(Token::Op(op @ ('+' | '-'))) = self.peek() {
            let op = *op;
            self.next();
            let right = self.parse_term()?;
            left = Expr::Binary(op, Box::new(left), Box::new(right));
        }
        Ok(left)
    }
    fn parse_term(&mut self) -> Result<Expr, NnError> {
        let mut left = self.parse_unary()?;
        while let Some(Token::Op(op @ ('*' | '/'))) = self.peek() {
            let op = *op;
            self.next();
            let right = self.parse_unary()?;
            left = Expr::Binary(op, Box::new(left), Box::new(right));
        }
        Ok(left)
    }
    fn parse_unary(&mut self) -> Result<Expr, NnError> {
        if let Some(Token::Op('-')) = self.peek() {
            self.next();
            let inner = self.parse_unary()?;
            return Ok(Expr::Unary(Box::new(inner)));
        }
        self.parse_atom()
    }
    fn parse_atom(&mut self) -> Result<Expr, NnError> {
        match self.next() {
            Some(Token::Number(v)) => Ok(Expr::Number(v)),
            Some(Token::Ident(name)) => Ok(Expr::Ident(name)),
            Some(Token::Func(f)) => {
                if self.next() != Some(Token::LParen) {
                    return Err(NnError::InvalidRequest {
                        message: format!("function {f} needs parenthesis"),
                    });
                }
                let inner = self.parse_expr()?;
                if self.next() != Some(Token::RParen) {
                    return Err(NnError::InvalidRequest {
                        message: format!("unbalanced parenthesis after {f}"),
                    });
                }
                Ok(Expr::Call(f, Box::new(inner)))
            }
            Some(Token::LParen) => {
                let inner = self.parse_expr()?;
                if self.next() != Some(Token::RParen) {
                    return Err(NnError::InvalidRequest {
                        message: "unbalanced parenthesis".to_string(),
                    });
                }
                Ok(inner)
            }
            other => Err(NnError::InvalidRequest {
                message: format!("unexpected token in expression: {other:?}"),
            }),
        }
    }
}

fn parse(text: &str) -> Result<(Expr, Vec<String>), NnError> {
    let tokens = tokenize(text)?;
    if tokens.is_empty() {
        return Err(NnError::InvalidRequest {
            message: "empty expression".to_string(),
        });
    }
    let mut parser = Parser { tokens, pos: 0 };
    let expr = parser.parse_expr()?;
    if parser.pos != parser.tokens.len() {
        return Err(NnError::InvalidRequest {
            message: "trailing tokens after expression".to_string(),
        });
    }
    let mut inputs = Vec::new();
    collect_idents(&expr, &mut inputs);
    inputs.sort();
    inputs.dedup();
    Ok((expr, inputs))
}

fn collect_idents(expr: &Expr, out: &mut Vec<String>) {
    match expr {
        Expr::Ident(name) => out.push(name.clone()),
        Expr::Number(_) => {}
        Expr::Unary(a) | Expr::Call(_, a) => collect_idents(a, out),
        Expr::Binary(_, a, b) => {
            collect_idents(a, out);
            collect_idents(b, out);
        }
    }
}

/// Domain-violation tally per operation kind.
#[derive(Default)]
struct DomainTally {
    log_nonpositive: u64,
    sqrt_negative: u64,
    div_zero: u64,
}

impl DomainTally {
    fn total(&self) -> u64 {
        self.log_nonpositive + self.sqrt_negative + self.div_zero
    }
    fn lines(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.log_nonpositive > 0 {
            out.push(format!(
                "log of non-positive input: {}",
                self.log_nonpositive
            ));
        }
        if self.sqrt_negative > 0 {
            out.push(format!("sqrt of negative input: {}", self.sqrt_negative));
        }
        if self.div_zero > 0 {
            out.push(format!("division by zero: {}", self.div_zero));
        }
        out
    }
}

fn eval(
    expr: &Expr,
    values: &BTreeMap<String, Vec<f64>>,
    tally: &mut DomainTally,
) -> Result<Vec<f64>, NnError> {
    match expr {
        Expr::Number(v) => {
            // Scalars broadcast over the first input's length (validated
            // equal beforehand); a pure-constant expression is rejected.
            let len =
                values
                    .values()
                    .next()
                    .map(|v| v.len())
                    .ok_or_else(|| NnError::InvalidRequest {
                        message: "expression needs at least one named input".to_string(),
                    })?;
            Ok(vec![*v; len])
        }
        Expr::Ident(name) => values
            .get(name)
            .cloned()
            .ok_or_else(|| NnError::InvalidRequest {
                // Unreachable: inputs are pre-resolved.
                message: format!("unresolved input {name}"),
            }),
        Expr::Unary(a) => eval(a, values, tally).map(|v| v.into_iter().map(|x| -x).collect()),
        Expr::Binary(op, a, b) => {
            let left = eval(a, values, tally)?;
            let right = eval(b, values, tally)?;
            let mut out = Vec::with_capacity(left.len());
            for (l, r) in left.iter().zip(right.iter()) {
                out.push(match op {
                    '+' => l + r,
                    '-' => l - r,
                    '*' => l * r,
                    '/' => {
                        if *r == 0.0 {
                            tally.div_zero += 1;
                            f64::NAN
                        } else {
                            l / r
                        }
                    }
                    other => unreachable!("op {other}"),
                });
            }
            Ok(out)
        }
        Expr::Call(fname, a) => {
            let inner = eval(a, values, tally)?;
            let mut out = Vec::with_capacity(inner.len());
            for x in inner {
                out.push(match *fname {
                    "log" => {
                        if x <= 0.0 {
                            tally.log_nonpositive += 1;
                            f64::NAN
                        } else {
                            x.ln()
                        }
                    }
                    "exp" => x.exp(),
                    "sqrt" => {
                        if x < 0.0 {
                            tally.sqrt_negative += 1;
                            f64::NAN
                        } else {
                            x.sqrt()
                        }
                    }
                    "abs" => x.abs(),
                    other => unreachable!("func {other}"),
                });
            }
            Ok(out)
        }
    }
}

fn load_input(
    catalog: &Catalog,
    tensor: &CatalogTensor,
    budget: &Budget,
) -> Result<Vec<f64>, NnError> {
    let source = catalog.resolve_source(&tensor.source_id)?;
    let path = catalog.resolve_path(&source.path);
    let reader = ArrayReader::open(&path, tensor, budget)?;
    let layout = super::codec::layout_for_encoding(&tensor.encoding);
    let super::codec::TensorLayout::Scalar(codec) = layout else {
        return Err(NnError::CodecUnsupported {
            codec: tensor.encoding.clone(),
            operation: "derive input".to_string(),
            reason: "derived views need scalar-decodable inputs".to_string(),
        });
    };
    let width = codec.width();
    let bytes = reader.read_all(tensor.element_count * width, budget)?;
    let mut out = Vec::with_capacity(tensor.element_count as usize);
    for chunk in bytes.chunks(width as usize) {
        let scalar = codec.decode(chunk)?;
        out.push(match scalar.value {
            super::codec::ScalarValue::Float(f) => f,
            super::codec::ScalarValue::Int(v) => v as f64,
            super::codec::ScalarValue::Uint(v) => v as f64,
            super::codec::ScalarValue::Bool(v) => (v as u8) as f64,
        });
    }
    Ok(out)
}

/// Write a little-endian C-order NPY (v1) file.
fn write_npy(path: &Path, values: &[f64], shape: &[u64], f32_out: bool) -> Result<(), NnError> {
    let mut payload: Vec<u8> = Vec::with_capacity(values.len() * if f32_out { 4 } else { 8 });
    for v in values {
        if f32_out {
            payload.extend_from_slice(&(*v as f32).to_le_bytes());
        } else {
            payload.extend_from_slice(&v.to_le_bytes());
        }
    }
    let dims: Vec<String> = shape.iter().map(|d| d.to_string()).collect();
    // numpy requires the trailing comma for 1-D tuples ((5,) not (5));
    // trailing commas are accepted at any rank.
    let header_text = format!(
        "{{'descr': '{}', 'fortran_order': False, 'shape': ({},), }}",
        if f32_out { "<f4" } else { "<f8" },
        dims.join(",")
    );
    let mut header = Vec::new();
    header.extend_from_slice(b"\x93NUMPY");
    header.extend_from_slice(&[1, 0]);
    // Pad the dictionary to a 64-byte total alignment.
    let mut dict_len = header_text.len();
    let unpadded = 10 + dict_len;
    if unpadded % 64 != 0 {
        dict_len += 64 - (unpadded % 64);
    }
    header.extend_from_slice(&(dict_len as u16).to_le_bytes());
    header.extend_from_slice(header_text.as_bytes());
    while header.len() < 10 + dict_len {
        header.push(b' ');
    }
    let mut out = header;
    out.extend_from_slice(&payload);
    std::fs::write(path, out).map_err(NnError::Io)?;
    Ok(())
}

pub struct DeriveReceipt {
    pub output_path: std::path::PathBuf,
    pub sidecar_path: std::path::PathBuf,
    pub elements: u64,
    pub domain_violations: u64,
    pub inputs: Vec<String>,
}

pub fn derive_text(receipt: &DeriveReceipt) -> String {
    let mut out = String::from("derived view\n");
    out.push_str(&format!("  output:    {}\n", receipt.output_path.display()));
    out.push_str(&format!(
        "  sidecar:   {}\n",
        receipt.sidecar_path.display()
    ));
    out.push_str(&format!("  inputs:    {}\n", receipt.inputs.join(", ")));
    out.push_str(&format!("  elements:  {}\n", receipt.elements));
    if receipt.domain_violations > 0 {
        out.push_str(&format!(
            "  domain violations written as NaN: {}\n",
            receipt.domain_violations
        ));
    } else {
        out.push_str("  domain violations: none\n");
    }
    out.push_str("  claims: derived values under the stated formula and dtypes; not observed device values\n");
    out
}

pub fn derive_envelope(receipt: &DeriveReceipt) -> Result<ResultEnvelope, NnError> {
    let semantic = Json::object(vec![
        (
            "output",
            Json::Str(receipt.output_path.display().to_string()),
        ),
        (
            "sidecar",
            Json::Str(receipt.sidecar_path.display().to_string()),
        ),
        ("elements", Json::Str(receipt.elements.to_string())),
        (
            "domain_violations",
            Json::Str(receipt.domain_violations.to_string()),
        ),
        (
            "inputs",
            Json::Array(receipt.inputs.iter().cloned().map(Json::Str).collect()),
        ),
    ])?;
    Ok(ResultEnvelope::new("derive").with_semantic(semantic))
}

/// Evaluate `expression` over catalog tensors named by its identifiers and
/// write the derived array (+ provenance sidecar). Inputs must share one
/// shape; domain violations refuse by default and become NaN with
/// `allow_domain_violations`.
pub fn derive(
    catalog: &Catalog,
    expression: &str,
    out_npy: &Path,
    out_sidecar: &Path,
    output_f32: bool,
    allow_domain_violations: bool,
    budget: &Budget,
) -> Result<DeriveReceipt, NnError> {
    let (expr, input_names) = parse(expression)?;
    if input_names.is_empty() {
        return Err(NnError::InvalidRequest {
            message: "expression references no named tensor".to_string(),
        });
    }
    let mut values: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    let mut shape: Option<Vec<u64>> = None;
    let mut input_records = Vec::new();
    for name in &input_names {
        let matches: Vec<&CatalogTensor> = catalog
            .tensors
            .iter()
            .filter(|t| t.original_name == *name)
            .collect();
        let tensor = match matches.as_slice() {
            [one] => *one,
            [] => {
                return Err(NnError::SourceMissing {
                    detail: format!("input tensor {name} not in the catalog"),
                })
            }
            _ => {
                return Err(NnError::AmbiguousBinding {
                    detail: format!("input tensor {name} matches {} tensors", matches.len()),
                })
            }
        };
        if shape.is_none() {
            shape = Some(tensor.shape.clone());
        } else if shape != Some(tensor.shape.clone()) {
            return Err(NnError::InvalidRequest {
                message: format!(
                    "input {name} has shape {:?}; all inputs must share one shape {:?}",
                    tensor.shape, shape
                ),
            });
        }
        let value = load_input(catalog, tensor, budget)?;
        input_records.push((
            name.clone(),
            tensor.encoding.clone(),
            tensor.shape.clone(),
            tensor.element_count,
            tensor.id.clone(),
        ));
        values.insert(name.clone(), value);
    }
    let shape = shape.unwrap_or_default();
    let mut tally = DomainTally::default();
    let result = eval(&expr, &values, &mut tally)?;
    if tally.total() > 0 && !allow_domain_violations {
        return Err(NnError::InvalidRequest {
            message: format!(
                "expression hits domain violations ({}); pass --allow-domain-violations to write NaN at those positions",
                tally.lines().join("; ")
            ),
        });
    }
    let nan_count = result.iter().filter(|v| v.is_nan()).count() as u64;
    write_npy(out_npy, &result, &shape, output_f32)?;
    let output_sha = sha256_of(out_npy)?;
    let sidecar = Json::object(vec![
        ("schema", Json::Str("binfiddle.nn.derive-sidecar/v1".to_string())),
        ("expression", Json::Str(expression.to_string())),
        (
            "inputs",
            Json::Array(
                input_records
                    .iter()
                    .map(|(name, encoding, shape, count, id)| {
                        Json::object(vec![
                            ("name", Json::Str(name.clone())),
                            ("encoding", Json::Str(encoding.clone())),
                            (
                                "shape",
                                Json::Array(shape.iter().cloned().map(|d| Json::Str(d.to_string())).collect()),
                            ),
                            ("elements", Json::Str(count.to_string())),
                            ("tensor_id", Json::Str(id.clone())),
                        ])
                    })
                    .collect::<Result<Vec<_>, _>>()?,
            ),
        ),
        ("evaluation_dtype", Json::Str("float64".to_string())),
        (
            "output_dtype",
            Json::Str(if output_f32 { "float32".to_string() } else { "float64".to_string() }),
        ),
        ("output_sha256", Json::Str(output_sha)),
        ("domain_violations", Json::Str(tally.total().to_string())),
        ("nan_elements", Json::Str(nan_count.to_string())),
        (
            "claims",
            Json::Str(
                "derived under the stated formula; values are model results only with the stated assumptions; not observed device values"
                    .to_string(),
            ),
        ),
    ])?;
    std::fs::write(out_sidecar, sidecar.to_canonical()?.as_bytes()).map_err(NnError::Io)?;
    Ok(DeriveReceipt {
        output_path: out_npy.to_path_buf(),
        sidecar_path: out_sidecar.to_path_buf(),
        elements: result.len() as u64,
        domain_violations: tally.total(),
        inputs: input_names,
    })
}

fn sha256_of(path: &Path) -> Result<String, NnError> {
    use sha2::Digest;
    let bytes = std::fs::read(path).map_err(NnError::Io)?;
    Ok(hex::encode(sha2::Sha256::digest(&bytes)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_collects_inputs() {
        let (expr, inputs) = parse("log(p) - log(2 - p)").unwrap();
        assert_eq!(inputs, vec!["p".to_string()]);
        match &expr {
            Expr::Binary('-', a, b) => {
                assert!(matches!(&**a, Expr::Call("log", _)));
                assert!(matches!(&**b, Expr::Call("log", _)));
            }
            other => panic!("unexpected {other:?}"),
        }
        assert!(parse("post - expected_post").is_ok());
        assert!(parse("1 + ").is_err());
        assert!(parse("foo($)").is_err());
    }

    #[test]
    fn eval_reference_vector_logit() {
        // p in (0,2): log(p) - log(2-p) matches independent computation.
        let (expr, _) = parse("log(p) - log(2 - p)").unwrap();
        let p: Vec<f64> = (1..=8).map(|i| i as f64 / 8.0).collect();
        let mut values = BTreeMap::new();
        values.insert("p".to_string(), p.clone());
        let mut tally = DomainTally::default();
        let out = eval(&expr, &values, &mut tally).unwrap();
        assert_eq!(tally.total(), 0);
        for (got, x) in out.iter().zip(p.iter()) {
            let expected = x.ln() - (2.0 - x).ln();
            assert!((got - expected).abs() < 1e-12);
        }
    }

    #[test]
    fn domain_violations_are_counted() {
        let (expr, _) = parse("log(p)").unwrap();
        let mut values = BTreeMap::new();
        values.insert("p".to_string(), vec![0.5, 0.0, -1.0, 2.0]);
        let mut tally = DomainTally::default();
        let out = eval(&expr, &values, &mut tally).unwrap();
        assert_eq!(tally.log_nonpositive, 2);
        assert!(out[0].is_finite() && out[3].is_finite());
        assert!(out[1].is_nan() && out[2].is_nan());

        let (expr, _) = parse("p / q").unwrap();
        let mut values = BTreeMap::new();
        values.insert("p".to_string(), vec![1.0, 2.0]);
        values.insert("q".to_string(), vec![2.0, 0.0]);
        let mut tally = DomainTally::default();
        let out = eval(&expr, &values, &mut tally).unwrap();
        assert_eq!(tally.div_zero, 1);
        assert!((out[0] - 0.5).abs() < 1e-12 && out[1].is_nan());
    }

    #[test]
    fn npy_writer_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.npy");
        write_npy(&path, &[1.0, 2.5, -3.0], &[3], false).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(&bytes[..6], b"\x93NUMPY");
        // Header padded to 64.
        let dict_len = u16::from_le_bytes([bytes[8], bytes[9]]) as usize;
        assert_eq!((10 + dict_len) % 64, 0);
        let payload = &bytes[10 + dict_len..];
        assert_eq!(payload.len(), 24);
        let v0 = f64::from_le_bytes(payload[..8].try_into().unwrap());
        assert_eq!(v0, 1.0);
        // And the file re-discovers through the npy tier.
        let file = crate::nn::source::BoundedFile::open(&path).unwrap();
        let inv = crate::nn::format::npy::inventory_npy(&file, &Budget::unrestricted()).unwrap();
        assert_eq!(inv.tensors[0].encoding, "numpy.float64");
        assert_eq!(inv.tensors[0].shape, vec![3]);
        assert_eq!(inv.tensors[0].payload_length, Some(24));
    }
}
