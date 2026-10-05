//! Declarative model packs.
//!
//! A pack teaches the generic catalog what a particular architecture's tensor
//! names mean. Packs are pure data: a YAML manifest with configuration
//! parameters, tensor-name patterns with captures, expected shapes written as
//! integer expressions over those parameters, and component kinds that carry
//! architecture-specific layout mechanics. Nothing in a pack executes.
//!
//! Recognition matches catalog tensors against the patterns, evaluates the
//! expected shapes, and keeps contradictions as first-class findings — a
//! name-match with a wrong shape never silently becomes a component, and an
//! unmatched tensor never disappears. Components are virtual views over the
//! same stable tensor identities the catalog already carries.

use super::catalog::Catalog;
use super::error::NnError;
use super::id::{compute_id, IdKind};
use super::json::Json;
use std::collections::BTreeMap;
use std::path::Path;

/// A tensor-name binding: pattern with captures, component template, kind,
/// and expected shape expressions per axis.
#[derive(Debug, Clone, PartialEq)]
pub struct Binding {
    pub pattern: String,
    pub component: String,
    pub kind: BindingKind,
    pub shape: Vec<String>,
}

/// Component kinds carrying layout mechanics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindingKind {
    /// Ordinary dense projection (no special view).
    Dense,
    /// Fused query/gate projection `[2HD, d]` (per-head interleaving).
    QueryGate,
    /// Zero-centered RMSNorm weight (`effective = 1 + stored`).
    NormZeroCentered,
    /// GatedDeltaNet grouped `qkvz` projection `[Nk*P, d]`.
    LinearQkvzGroups,
    /// GatedDeltaNet grouped `ba` projection `[2Nv, d]`.
    LinearBaGroups,
    /// MLP gate projection `[m, d]` (rows are intermediate channels).
    MlpGate,
    /// MLP up projection `[m, d]` (rows are intermediate channels).
    MlpUp,
    /// MLP down projection `[d, m]` (columns are intermediate channels).
    MlpDown,
}

impl BindingKind {
    pub fn as_str(self) -> &'static str {
        match self {
            BindingKind::Dense => "dense",
            BindingKind::QueryGate => "query_gate",
            BindingKind::NormZeroCentered => "norm_zero_centered",
            BindingKind::LinearQkvzGroups => "linear_qkvz_groups",
            BindingKind::LinearBaGroups => "linear_ba_groups",
            BindingKind::MlpGate => "mlp_gate",
            BindingKind::MlpUp => "mlp_up",
            BindingKind::MlpDown => "mlp_down",
        }
    }

    fn parse(text: &str) -> Result<BindingKind, NnError> {
        match text {
            "dense" => Ok(BindingKind::Dense),
            "query_gate" => Ok(BindingKind::QueryGate),
            "norm_zero_centered" => Ok(BindingKind::NormZeroCentered),
            "linear_qkvz_groups" => Ok(BindingKind::LinearQkvzGroups),
            "linear_ba_groups" => Ok(BindingKind::LinearBaGroups),
            "mlp_gate" => Ok(BindingKind::MlpGate),
            "mlp_up" => Ok(BindingKind::MlpUp),
            "mlp_down" => Ok(BindingKind::MlpDown),
            other => Err(NnError::MalformedInput {
                detail: format!("unknown binding kind {other}"),
            }),
        }
    }
}

/// A declarative model pack.
#[derive(Debug, Clone, PartialEq)]
pub struct Pack {
    pub id_name: String,
    pub version: String,
    pub description: String,
    /// Integer parameters referenced by shape expressions.
    pub config: BTreeMap<String, u64>,
    pub bindings: Vec<Binding>,
    /// Explicit full-attention layer indices, when declared.
    pub layer_types_explicit: Option<Vec<bool>>,
    /// Fallback interval: layer i is full attention when (i+1) % interval == 0.
    pub full_attention_interval: Option<u64>,
}

impl Pack {
    /// Parse a pack from YAML text.
    pub fn parse(text: &str) -> Result<Pack, NnError> {
        let value: serde_yaml::Value =
            serde_yaml::from_str(text).map_err(|e| NnError::MalformedInput {
                detail: format!("pack YAML: {e}"),
            })?;
        if value.get("schema").and_then(|v| v.as_str()) != Some("binfiddle.nn.pack/v1") {
            return Err(NnError::MalformedInput {
                detail: "not a binfiddle model pack (schema mismatch)".to_string(),
            });
        }
        let id_name = value
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| NnError::MalformedInput {
                detail: "pack is missing id".to_string(),
            })?
            .to_string();
        let version = value
            .get("version")
            .and_then(|v| v.as_str())
            .unwrap_or("0")
            .to_string();
        let description = value
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let mut config = BTreeMap::new();
        if let Some(mapping) = value.get("config").and_then(|v| v.as_mapping()) {
            for (key, val) in mapping {
                let key = key.as_str().ok_or_else(|| NnError::MalformedInput {
                    detail: "config keys must be strings".to_string(),
                })?;
                let number = val.as_u64().ok_or_else(|| NnError::MalformedInput {
                    detail: format!("config parameter {key} must be a non-negative integer"),
                })?;
                config.insert(key.to_string(), number);
            }
        }

        let mut bindings = Vec::new();
        if let Some(list) = value.get("bindings").and_then(|v| v.as_sequence()) {
            for entry in list {
                let pattern = entry
                    .get("pattern")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| NnError::MalformedInput {
                        detail: "binding is missing pattern".to_string(),
                    })?
                    .to_string();
                let component = entry
                    .get("component")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| NnError::MalformedInput {
                        detail: format!("binding {pattern} is missing component"),
                    })?
                    .to_string();
                let kind = BindingKind::parse(
                    entry
                        .get("kind")
                        .and_then(|v| v.as_str())
                        .unwrap_or("dense"),
                )?;
                let shape = entry
                    .get("shape")
                    .and_then(|v| v.as_sequence())
                    .map(|axes| {
                        axes.iter()
                            .filter_map(|a| a.as_str().map(str::to_string))
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                // Patterns and expressions must parse before the pack loads.
                validate_pattern(&pattern)?;
                for axis in &shape {
                    parse_expression(axis)?;
                }
                bindings.push(Binding {
                    pattern,
                    component,
                    kind,
                    shape,
                });
            }
        }
        if bindings.is_empty() {
            return Err(NnError::MalformedInput {
                detail: "pack declares no bindings".to_string(),
            });
        }

        let mut layer_types_explicit = None;
        if let Some(list) = value.get("layer_types").and_then(|v| v.as_sequence()) {
            let flags = list
                .iter()
                .map(|v| {
                    v.as_str().and_then(|s| match s {
                        "full_attention" => Some(true),
                        "linear_attention" => Some(false),
                        _ => None,
                    })
                })
                .collect::<Option<Vec<bool>>>()
                .ok_or_else(|| NnError::MalformedInput {
                    detail: "layer_types entries must be full_attention or linear_attention"
                        .to_string(),
                })?;
            layer_types_explicit = Some(flags);
        }
        let full_attention_interval = value
            .get("full_attention_interval")
            .and_then(|v| v.as_u64());

        Ok(Pack {
            id_name,
            version,
            description,
            config,
            bindings,
            layer_types_explicit,
            full_attention_interval,
        })
    }

    /// Load from a file (a pack.yaml, or a directory containing one).
    pub fn load(path: &Path) -> Result<Pack, NnError> {
        let file = if path.is_dir() {
            path.join("pack.yaml")
        } else {
            path.to_path_buf()
        };
        let text = std::fs::read_to_string(&file).map_err(|_| NnError::SourceMissing {
            detail: format!("no pack file at {}", file.display()),
        })?;
        Pack::parse(&text)
    }

    /// Canonical semantic record (config sorted by key; bindings in order).
    pub fn semantic(&self) -> Result<Json, NnError> {
        let config = Json::Object(
            self.config
                .iter()
                .map(|(k, v)| (k.clone(), Json::Str(v.to_string())))
                .collect(),
        );
        let bindings = self
            .bindings
            .iter()
            .map(|b| {
                Json::object(vec![
                    ("pattern", Json::Str(b.pattern.clone())),
                    ("component", Json::Str(b.component.clone())),
                    ("kind", Json::Str(b.kind.as_str().to_string())),
                    (
                        "shape",
                        Json::Array(b.shape.iter().cloned().map(Json::Str).collect()),
                    ),
                ])
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut pairs = vec![
            (
                "schema",
                Json::Str("binfiddle.nn.pack-semantic/v1".to_string()),
            ),
            ("id", Json::Str(self.id_name.clone())),
            ("version", Json::Str(self.version.clone())),
            ("config", config),
            ("bindings", Json::Array(bindings)),
        ];
        if let Some(flags) = &self.layer_types_explicit {
            pairs.push((
                "layer_types",
                Json::Array(
                    flags
                        .iter()
                        .map(|f| {
                            Json::Str(
                                f.then_some("full_attention")
                                    .unwrap_or("linear_attention")
                                    .to_string(),
                            )
                        })
                        .collect(),
                ),
            ));
        }
        if let Some(interval) = self.full_attention_interval {
            pairs.push(("full_attention_interval", Json::Str(interval.to_string())));
        }
        Json::object(pairs)
    }

    /// Domain-separated pack identifier over the canonical semantic record.
    pub fn pack_id(&self) -> Result<String, NnError> {
        compute_id(IdKind::Pack, &self.semantic()?)
    }

    /// Evaluate a shape expression against the pack configuration.
    pub fn eval(&self, expression: &str) -> Result<u64, NnError> {
        let parsed = parse_expression(expression)?;
        eval_expression(&parsed, &self.config)
    }

    /// The full-attention layer schedule: explicit flags when declared, the
    /// interval fallback otherwise (layer i is full attention when
    /// (i+1) % interval == 0).
    pub fn full_attention_layers(&self) -> Result<Vec<usize>, NnError> {
        if let Some(flags) = &self.layer_types_explicit {
            return Ok(flags
                .iter()
                .enumerate()
                .filter(|(_, f)| **f)
                .map(|(i, _)| i)
                .collect());
        }
        let interval = self
            .full_attention_interval
            .ok_or_else(|| NnError::InvalidRequest {
                message: "pack declares neither layer_types nor full_attention_interval"
                    .to_string(),
            })?;
        if interval == 0 {
            return Err(NnError::MalformedInput {
                detail: "full_attention_interval must be positive".to_string(),
            });
        }
        let layers = self.config.get("num_layers").copied().unwrap_or(0) as usize;
        Ok((0..layers)
            .filter(|i| (i + 1) % interval as usize == 0)
            .collect())
    }
}

// ---- name patterns ----

/// A compiled name pattern: literal segments and `{capture}` holes.
#[derive(Debug, Clone, PartialEq)]
pub enum PatternToken {
    Literal(String),
    Capture(String),
}

fn validate_pattern(pattern: &str) -> Result<Vec<PatternToken>, NnError> {
    let tokens = compile_pattern(pattern)?;
    if tokens.is_empty() {
        return Err(NnError::MalformedInput {
            detail: format!("empty pattern {pattern}"),
        });
    }
    for pair in tokens.windows(2) {
        if matches!(pair[0], PatternToken::Capture(_))
            && matches!(pair[1], PatternToken::Capture(_))
        {
            return Err(NnError::MalformedInput {
                detail: format!("adjacent captures in pattern {pattern} are ambiguous"),
            });
        }
    }
    Ok(tokens)
}

fn compile_pattern(pattern: &str) -> Result<Vec<PatternToken>, NnError> {
    let mut tokens = Vec::new();
    let mut literal = String::new();
    let mut chars = pattern.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '{' {
            let mut name = String::new();
            let mut closed = false;
            for inner in chars.by_ref() {
                if inner == '}' {
                    closed = true;
                    break;
                }
                name.push(inner);
            }
            if !closed || name.is_empty() {
                return Err(NnError::MalformedInput {
                    detail: format!("malformed capture in pattern {pattern}"),
                });
            }
            if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                return Err(NnError::MalformedInput {
                    detail: format!("capture name {name} must be alphanumeric"),
                });
            }
            if !literal.is_empty() {
                tokens.push(PatternToken::Literal(std::mem::take(&mut literal)));
            }
            tokens.push(PatternToken::Capture(name));
        } else {
            literal.push(ch);
        }
    }
    if !literal.is_empty() {
        tokens.push(PatternToken::Literal(literal));
    }
    Ok(tokens)
}

/// Match a tensor name against a pattern; captures must be decimal integers
/// (they index layers/heads/families).
pub fn match_pattern(tokens: &[PatternToken], name: &str) -> Option<BTreeMap<String, u64>> {
    let mut captures = BTreeMap::new();
    let mut rest = name;
    for (index, token) in tokens.iter().enumerate() {
        match token {
            PatternToken::Literal(lit) => {
                if index == tokens.len() - 1 {
                    // Trailing literal must consume the remainder exactly.
                    if rest != lit.as_str() {
                        return None;
                    }
                    return Some(captures);
                }
                let pos = rest.find(lit.as_str())?;
                if pos != 0 {
                    return None;
                }
                rest = &rest[lit.len()..];
            }
            PatternToken::Capture(name_key) => {
                // The capture runs until the next literal (or the end).
                let next_literal = tokens.get(index + 1).and_then(|t| match t {
                    PatternToken::Literal(l) => Some(l.clone()),
                    PatternToken::Capture(_) => None,
                });
                let value = match &next_literal {
                    Some(next) => {
                        let pos = rest.find(next.as_str())?;
                        if pos == 0 {
                            return None; // empty capture
                        }
                        let value = &rest[..pos];
                        rest = &rest[pos..];
                        value
                    }
                    None => rest,
                };
                // Last token and it is a capture: the remainder is the value.
                if index == tokens.len() - 1 {
                    if value.is_empty() {
                        return None;
                    }
                    let parsed: u64 = value.parse().ok()?;
                    captures.insert(name_key.clone(), parsed);
                    return Some(captures);
                }
                // Adjacent captures are rejected structurally at
                // validation time; here a next literal always exists.
                let parsed: u64 = value.parse().ok()?;
                captures.insert(name_key.clone(), parsed);
            }
        }
    }
    // Trailing literal already returned; reaching here means a capture ended
    // the pattern but consumed nothing.
    match tokens.last() {
        Some(PatternToken::Literal(_)) => None,
        _ => None,
    }
}

// ---- expressions ----

/// A parsed integer expression.
#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Number(u64),
    Ident(String),
    Add(Box<Expr>, Box<Expr>),
    Sub(Box<Expr>, Box<Expr>),
    Mul(Box<Expr>, Box<Expr>),
    Div(Box<Expr>, Box<Expr>), // exact division only
}

pub fn parse_expression(text: &str) -> Result<Expr, NnError> {
    let mut parser = ExprParser {
        bytes: text.as_bytes(),
        pos: 0,
    };
    parser.skip_ws();
    let expr = parser.parse_add()?;
    parser.skip_ws();
    if parser.pos != parser.bytes.len() {
        return Err(NnError::MalformedInput {
            detail: format!("trailing characters in expression {text}"),
        });
    }
    Ok(expr)
}

struct ExprParser<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl ExprParser<'_> {
    fn skip_ws(&mut self) {
        while matches!(self.bytes.get(self.pos), Some(b' ' | b'\t')) {
            self.pos += 1;
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn parse_add(&mut self) -> Result<Expr, NnError> {
        let mut left = self.parse_mul()?;
        loop {
            self.skip_ws();
            match self.peek() {
                Some(b'+') => {
                    self.pos += 1;
                    let right = self.parse_mul()?;
                    left = Expr::Add(Box::new(left), Box::new(right));
                }
                Some(b'-') => {
                    self.pos += 1;
                    let right = self.parse_mul()?;
                    left = Expr::Sub(Box::new(left), Box::new(right));
                }
                _ => return Ok(left),
            }
        }
    }

    fn parse_mul(&mut self) -> Result<Expr, NnError> {
        let mut left = self.parse_atom()?;
        loop {
            self.skip_ws();
            match self.peek() {
                Some(b'*') => {
                    self.pos += 1;
                    let right = self.parse_atom()?;
                    left = Expr::Mul(Box::new(left), Box::new(right));
                }
                Some(b'/') => {
                    self.pos += 1;
                    let right = self.parse_atom()?;
                    left = Expr::Div(Box::new(left), Box::new(right));
                }
                _ => return Ok(left),
            }
        }
    }

    fn parse_atom(&mut self) -> Result<Expr, NnError> {
        self.skip_ws();
        match self.peek() {
            Some(b'(') => {
                self.pos += 1;
                let inner = self.parse_add()?;
                self.skip_ws();
                if self.peek() != Some(b')') {
                    return Err(NnError::MalformedInput {
                        detail: "expected ')' in expression".to_string(),
                    });
                }
                self.pos += 1;
                Ok(inner)
            }
            Some(b'0'..=b'9') => {
                let start = self.pos;
                while matches!(self.peek(), Some(b'0'..=b'9')) {
                    self.pos += 1;
                }
                let text = std::str::from_utf8(&self.bytes[start..self.pos]).map_err(|_| {
                    NnError::MalformedInput {
                        detail: "bad number".to_string(),
                    }
                })?;
                Ok(Expr::Number(text.parse().map_err(|_| {
                    NnError::MalformedInput {
                        detail: format!("number {text} out of range"),
                    }
                })?))
            }
            Some(b'a'..=b'z' | b'A'..=b'Z' | b'_') => {
                let start = self.pos;
                while matches!(
                    self.peek(),
                    Some(b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_')
                ) {
                    self.pos += 1;
                }
                let name = std::str::from_utf8(&self.bytes[start..self.pos]).map_err(|_| {
                    NnError::MalformedInput {
                        detail: "bad identifier".to_string(),
                    }
                })?;
                Ok(Expr::Ident(name.to_string()))
            }
            other => Err(NnError::MalformedInput {
                detail: format!("unexpected {:?} in expression", other.map(|b| b as char)),
            }),
        }
    }
}

pub fn eval_expression(expr: &Expr, params: &BTreeMap<String, u64>) -> Result<u64, NnError> {
    match expr {
        Expr::Number(n) => Ok(*n),
        Expr::Ident(name) => params
            .get(name)
            .copied()
            .ok_or_else(|| NnError::MalformedInput {
                detail: format!("unknown parameter {name}"),
            }),
        Expr::Add(a, b) => eval_expression(a, params)?
            .checked_add(eval_expression(b, params)?)
            .ok_or_else(overflow),
        Expr::Sub(a, b) => {
            let (a, b) = (eval_expression(a, params)?, eval_expression(b, params)?);
            a.checked_sub(b).ok_or_else(|| NnError::MalformedInput {
                detail: format!("negative shape result ({a} - {b})"),
            })
        }
        Expr::Mul(a, b) => eval_expression(a, params)?
            .checked_mul(eval_expression(b, params)?)
            .ok_or_else(overflow),
        Expr::Div(a, b) => {
            let (a, b) = (eval_expression(a, params)?, eval_expression(b, params)?);
            if b == 0 {
                return Err(NnError::MalformedInput {
                    detail: "division by zero in shape expression".to_string(),
                });
            }
            if a % b != 0 {
                return Err(NnError::MalformedInput {
                    detail: format!("inexact division {a}/{b} in shape expression (a fractional value/value-head ratio is invalid for this layout)"),
                });
            }
            Ok(a / b)
        }
    }
}

fn overflow() -> NnError {
    NnError::MalformedInput {
        detail: "arithmetic overflow in shape expression".to_string(),
    }
}

// ---- reference mechanics (parameter-driven, per binding kind) ----

/// B.4: per-head rows of a fused `[2HD, d]` query/gate projection. Head `h`
/// owns query rows `[2Dh, 2Dh+D)` and gate rows `[2Dh+D, 2Dh+2D)`.
pub fn query_gate_head_rows(head: u64, head_dim: u64) -> ((u64, u64), (u64, u64)) {
    let base = 2 * head_dim * head;
    (
        (base, base + head_dim),
        (base + head_dim, base + 2 * head_dim),
    )
}

/// B.5 parameters for the grouped GatedDeltaNet projections.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinearGroupParams {
    pub key_heads: u64,
    pub value_heads: u64,
    pub key_head_dim: u64,
    pub value_head_dim: u64,
}

impl LinearGroupParams {
    /// R = value_heads / key_heads (must divide exactly for this layout).
    pub fn ratio(&self) -> Result<u64, NnError> {
        if self.key_heads == 0 || !self.value_heads.is_multiple_of(self.key_heads) {
            return Err(NnError::MalformedInput {
                detail: format!(
                    "value_heads {} is not a positive multiple of key_heads {}",
                    self.value_heads, self.key_heads
                ),
            });
        }
        Ok(self.value_heads / self.key_heads)
    }

    /// P = 2*Dk + 2*R*Dv: the projected width per key group.
    pub fn group_width(&self) -> Result<u64, NnError> {
        let r = self.ratio()?;
        Ok(2 * self.key_head_dim + 2 * r * self.value_head_dim)
    }

    /// B.5: row ranges of group `g` inside the `[Nk*P, d]` projection.
    pub fn group_rows(&self, group: u64) -> Result<LinearGroupRows, NnError> {
        let r = self.ratio()?;
        let p = self.group_width()?;
        let start = group * p;
        Ok(LinearGroupRows {
            query: (start, start + self.key_head_dim),
            key: (start + self.key_head_dim, start + 2 * self.key_head_dim),
            value: (
                start + 2 * self.key_head_dim,
                start + 2 * self.key_head_dim + r * self.value_head_dim,
            ),
            gate_z: (
                start + 2 * self.key_head_dim + r * self.value_head_dim,
                start + p,
            ),
        })
    }

    /// B.5: the `ba` projection is grouped by key group with width `2R`:
    /// the group's `b` rows come first, then its `a` rows.
    pub fn ba_group_rows(&self, group: u64) -> Result<BaGroupRows, NnError> {
        let r = self.ratio()?;
        let width = 2 * r;
        let start = group * width;
        Ok(BaGroupRows {
            b: (start, start + r),
            a: (start + r, start + width),
        })
    }
}

/// B.5: the `b` then `a` row ranges of one `ba` group.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BaGroupRows {
    pub b: (u64, u64),
    pub a: (u64, u64),
}

/// B.5 row ranges within one key group.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinearGroupRows {
    pub query: (u64, u64),
    pub key: (u64, u64),
    pub value: (u64, u64),
    pub gate_z: (u64, u64),
}

/// B.7: the zero-centered normalization lens maps stored `w` to the
/// effective scale `gamma = 1 + w`.
pub fn norm_effective_scale(stored: f64) -> f64 {
    1.0 + stored
}

// ---- recognition ----

/// One recognized component: a virtual view over member tensors.
#[derive(Debug, Clone)]
pub struct RecognizedComponent {
    pub path: String,
    pub kind: BindingKind,
    pub tensor_names: Vec<String>,
    pub tensor_ids: Vec<String>,
    /// Capture values that instantiated the component template.
    pub captures: BTreeMap<String, u64>,
}

/// A contradiction: the name matched but the shape disagrees.
#[derive(Debug, Clone)]
pub struct Contradiction {
    pub tensor_name: String,
    pub pattern: String,
    pub component: String,
    pub expected_shape: Vec<u64>,
    pub actual_shape: Vec<u64>,
}

/// Recognition outcome for one catalog under one pack.
#[derive(Debug, Clone)]
pub struct Recognition {
    pub pack_id: String,
    pub pack_name: String,
    pub components: Vec<RecognizedComponent>,
    pub contradictions: Vec<Contradiction>,
    /// Tensor names that matched no pattern.
    pub unassigned: Vec<String>,
}

impl Recognition {
    /// Match every catalog tensor against the pack.
    pub fn recognize(pack: &Pack, catalog: &Catalog) -> Result<Recognition, NnError> {
        let compiled: Vec<(Vec<PatternToken>, &Binding)> = pack
            .bindings
            .iter()
            .map(|b| {
                Ok::<(Vec<PatternToken>, &Binding), NnError>((compile_pattern(&b.pattern)?, b))
            })
            .collect::<Result<Vec<_>, _>>()?;
        // Every binding's shape expressions must evaluate now.
        for binding in &pack.bindings {
            for axis in &binding.shape {
                pack.eval(axis)?;
            }
        }

        let mut components: BTreeMap<String, RecognizedComponent> = BTreeMap::new();
        let mut contradictions = Vec::new();
        let mut unassigned = Vec::new();

        for tensor in &catalog.tensors {
            let mut matched = false;
            for (tokens, binding) in &compiled {
                let Some(captures) = match_pattern(tokens, &tensor.original_name) else {
                    continue;
                };
                matched = true;
                // Expected shape from expressions.
                let expected: Vec<u64> = binding
                    .shape
                    .iter()
                    .map(|axis| pack.eval(axis))
                    .collect::<Result<Vec<_>, _>>()?;
                if expected != tensor.shape {
                    contradictions.push(Contradiction {
                        tensor_name: tensor.original_name.clone(),
                        pattern: binding.pattern.clone(),
                        component: binding.component.clone(),
                        expected_shape: expected,
                        actual_shape: tensor.shape.clone(),
                    });
                    break;
                }
                // Instantiate the component path with captures.
                let path = instantiate(&binding.component, &captures);
                components
                    .entry(path.clone())
                    .and_modify(|existing| {
                        existing.tensor_names.push(tensor.original_name.clone());
                        existing.tensor_ids.push(tensor.id.clone());
                    })
                    .or_insert_with(|| RecognizedComponent {
                        path,
                        kind: binding.kind,
                        tensor_names: vec![tensor.original_name.clone()],
                        tensor_ids: vec![tensor.id.clone()],
                        captures: captures.clone(),
                    });
                break;
            }
            if !matched {
                unassigned.push(tensor.original_name.clone());
            }
        }

        Ok(Recognition {
            pack_id: pack.pack_id()?,
            pack_name: pack.id_name.clone(),
            components: components.into_values().collect(),
            contradictions,
            unassigned,
        })
    }

    /// Look up one component by exact path.
    pub fn component(&self, path: &str) -> Option<&RecognizedComponent> {
        self.components.iter().find(|c| c.path == path)
    }
}

fn instantiate(template: &str, captures: &BTreeMap<String, u64>) -> String {
    let mut out = template.to_string();
    for (key, value) in captures {
        out = out.replace(&format!("{{{key}}}"), &value.to_string());
    }
    out
}

// ---- rendering surfaces for ls/show ----

use super::report::ResultEnvelope;

/// Architecture-view envelope: components, contradictions, unassigned.
pub fn architecture_envelope(
    recognition: &Recognition,
    pack: &Pack,
    catalog: &Catalog,
) -> Result<ResultEnvelope, NnError> {
    let components = recognition
        .components
        .iter()
        .map(|c| {
            Json::object(vec![
                ("path", Json::Str(c.path.clone())),
                ("kind", Json::Str(c.kind.as_str().to_string())),
                (
                    "tensors",
                    Json::Array(c.tensor_names.iter().cloned().map(Json::Str).collect()),
                ),
                (
                    "tensor_ids",
                    Json::Array(c.tensor_ids.iter().cloned().map(Json::Str).collect()),
                ),
            ])
        })
        .collect::<Result<Vec<_>, _>>()?;
    let contradictions = recognition
        .contradictions
        .iter()
        .map(|c| {
            Json::object(vec![
                ("tensor", Json::Str(c.tensor_name.clone())),
                ("pattern", Json::Str(c.pattern.clone())),
                (
                    "expected_shape",
                    Json::Array(
                        c.expected_shape
                            .iter()
                            .map(|d| Json::Str(d.to_string()))
                            .collect(),
                    ),
                ),
                (
                    "actual_shape",
                    Json::Array(
                        c.actual_shape
                            .iter()
                            .map(|d| Json::Str(d.to_string()))
                            .collect(),
                    ),
                ),
            ])
        })
        .collect::<Result<Vec<_>, _>>()?;
    let semantic = Json::object(vec![
        ("pack_id", Json::Str(recognition.pack_id.clone())),
        ("pack", Json::Str(recognition.pack_name.clone())),
        ("catalog_id", Json::Str(catalog.id()?)),
        (
            "full_attention_layers",
            match pack.full_attention_layers() {
                Ok(layers) => {
                    Json::Array(layers.iter().map(|l| Json::Str(l.to_string())).collect())
                }
                Err(_) => Json::Null,
            },
        ),
        ("components", Json::Array(components)),
        ("contradictions", Json::Array(contradictions)),
        (
            "unassigned",
            Json::Array(
                recognition
                    .unassigned
                    .iter()
                    .cloned()
                    .map(Json::Str)
                    .collect(),
            ),
        ),
        (
            "counts",
            Json::object(vec![
                (
                    "components",
                    Json::Str(recognition.components.len().to_string()),
                ),
                (
                    "contradictions",
                    Json::Str(recognition.contradictions.len().to_string()),
                ),
                (
                    "unassigned",
                    Json::Str(recognition.unassigned.len().to_string()),
                ),
            ])?,
        ),
    ])?;
    let complete = recognition.contradictions.is_empty() && recognition.unassigned.is_empty();
    let mut envelope = ResultEnvelope::new("ls").with_semantic(semantic);
    if !complete {
        envelope = envelope.with_coverage(
            false,
            vec![format!(
                "{} contradictions, {} unassigned tensors",
                recognition.contradictions.len(),
                recognition.unassigned.len()
            )],
        );
    }
    Ok(envelope)
}

/// Human-readable architecture view.
pub fn architecture_text(recognition: &Recognition, pack: &Pack) -> String {
    let mut out = format!("architecture view (pack {})\n", recognition.pack_name);
    if let Ok(layers) = pack.full_attention_layers() {
        let layers_text = layers
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        out.push_str(&format!("  full-attention layers: [{layers_text}]\n"));
    }
    out.push_str("  components:\n");
    for component in &recognition.components {
        out.push_str(&format!(
            "    {} [{}] <- {}\n",
            component.path,
            component.kind.as_str(),
            component.tensor_names.join(", ")
        ));
    }
    if !recognition.contradictions.is_empty() {
        out.push_str("  contradictions:\n");
        for c in &recognition.contradictions {
            out.push_str(&format!(
                "    {} matched {} but shape {:?} != {:?}\n",
                c.tensor_name, c.pattern, c.actual_shape, c.expected_shape
            ));
        }
    }
    if !recognition.unassigned.is_empty() {
        out.push_str(&format!(
            "  unassigned tensors: {}\n",
            recognition.unassigned.join(", ")
        ));
    }
    out
}

/// Component detail record with kind-specific layout mechanics.
pub struct ComponentDetail {
    pub path: String,
    pub kind: BindingKind,
    pub tensor_names: Vec<String>,
    pub tensor_ids: Vec<String>,
    /// Kind-specific row maps (query/gate heads, linear groups).
    pub maps: Vec<String>,
}

/// Inspect one recognized component under its pack semantics.
pub fn component_detail(
    recognition: &Recognition,
    pack: &Pack,
    component_path: &str,
) -> Result<ComponentDetail, NnError> {
    let component = recognition.component(component_path).ok_or_else(|| {
        NnError::SourceMissing {
            detail: format!(
                "no component {component_path} in pack {}; list components with nn ls --view architecture",
                recognition.pack_name
            ),
        }
    })?;
    let mut maps = Vec::new();
    match component.kind {
        BindingKind::QueryGate => {
            let heads = pack.config.get("num_attention_heads").copied().unwrap_or(0);
            let head_dim = pack.config.get("head_dim").copied().unwrap_or(0);
            for h in 0..heads.min(8) {
                let ((qs, qe), (gs, ge)) = query_gate_head_rows(h, head_dim);
                maps.push(format!(
                    "head {h}: query rows [{qs}, {qe}), gate rows [{gs}, {ge})"
                ));
            }
            if heads > 8 {
                maps.push(format!("... and {} more heads", heads - 8));
            }
        }
        BindingKind::LinearQkvzGroups => {
            let params = linear_params(pack)?;
            for g in 0..params.key_heads.min(8) {
                let rows = params.group_rows(g)?;
                maps.push(format!(
                    "group {g}: q [{}, {}) k [{}, {}) v [{}, {}) z [{}, {})",
                    rows.query.0,
                    rows.query.1,
                    rows.key.0,
                    rows.key.1,
                    rows.value.0,
                    rows.value.1,
                    rows.gate_z.0,
                    rows.gate_z.1
                ));
            }
            maps.push(
                "convolution note: q/k/v channels are re-flattened after projection; these row maps do NOT describe convolution weights"
                    .to_string(),
            );
        }
        BindingKind::LinearBaGroups => {
            let params = linear_params(pack)?;
            for g in 0..params.key_heads.min(8) {
                let rows = params.ba_group_rows(g)?;
                let ((bs, be), (as_, ae)) = (rows.b, rows.a);
                maps.push(format!(
                    "group {g}: b rows [{bs}, {be}), a rows [{as_}, {ae})"
                ));
            }
        }
        BindingKind::NormZeroCentered => {
            maps.push("effective scale = 1 + stored weight (gamma = 1 + w)".to_string());
        }
        BindingKind::MlpGate | BindingKind::MlpUp => {
            maps.push("rows are intermediate channels (pruning coordinates)".to_string());
        }
        BindingKind::MlpDown => {
            maps.push("columns are intermediate channels (pruning coordinates)".to_string());
        }
        BindingKind::Dense => {}
    }
    Ok(ComponentDetail {
        path: component.path.clone(),
        kind: component.kind,
        tensor_names: component.tensor_names.clone(),
        tensor_ids: component.tensor_ids.clone(),
        maps,
    })
}

fn linear_params(pack: &Pack) -> Result<LinearGroupParams, NnError> {
    let get = |name: &str| pack.config.get(name).copied().unwrap_or(0);
    Ok(LinearGroupParams {
        key_heads: get("num_key_heads"),
        value_heads: get("num_value_heads"),
        key_head_dim: get("key_head_dim"),
        value_head_dim: get("value_head_dim"),
    })
}

/// Component envelope.
pub fn component_envelope(
    detail: &ComponentDetail,
    recognition: &Recognition,
) -> Result<ResultEnvelope, NnError> {
    let semantic = Json::object(vec![
        ("pack_id", Json::Str(recognition.pack_id.clone())),
        ("path", Json::Str(detail.path.clone())),
        ("kind", Json::Str(detail.kind.as_str().to_string())),
        (
            "tensors",
            Json::Array(detail.tensor_names.iter().cloned().map(Json::Str).collect()),
        ),
        (
            "tensor_ids",
            Json::Array(detail.tensor_ids.iter().cloned().map(Json::Str).collect()),
        ),
        (
            "layout_maps",
            Json::Array(detail.maps.iter().cloned().map(Json::Str).collect()),
        ),
    ])?;
    Ok(ResultEnvelope::new("show").with_semantic(semantic))
}

/// Component text card.
pub fn component_text(detail: &ComponentDetail) -> String {
    let mut out = format!("component {}\n", detail.path);
    out.push_str(&format!("  kind:    {}\n", detail.kind.as_str()));
    for name in &detail.tensor_names {
        out.push_str(&format!("  tensor:  {name}\n"));
    }
    for map in &detail.maps {
        out.push_str(&format!("  {map}\n"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- B.4: query/gate interleaving ----

    #[test]
    fn b4_query_gate_rows_reference_vector() {
        // H=2, D=3: query rows and gate rows per head.
        assert_eq!(query_gate_head_rows(0, 3), ((0, 3), (3, 6)));
        assert_eq!(query_gate_head_rows(1, 3), ((6, 9), (9, 12)));
        // A global-half split would select rows [0,6) as queries; the
        // per-head split is 0,1,2 and 6,7,8 instead.
        // Query row starts across heads.
        let starts: Vec<u64> = [0u64, 1]
            .iter()
            .map(|h| query_gate_head_rows(*h, 3).0 .0)
            .collect();
        assert_eq!(starts, vec![0, 6]);
    }

    // ---- B.5: GatedDeltaNet group maps ----

    #[test]
    fn b5_group_rows_reference_vector() {
        let params = LinearGroupParams {
            key_heads: 2,
            value_heads: 4,
            key_head_dim: 3,
            value_head_dim: 2,
        };
        assert_eq!(params.ratio().unwrap(), 2);
        assert_eq!(params.group_width().unwrap(), 14); // P = 2*3 + 2*2*2
        let g0 = params.group_rows(0).unwrap();
        assert_eq!(g0.query, (0, 3));
        assert_eq!(g0.key, (3, 6));
        assert_eq!(g0.value, (6, 10));
        assert_eq!(g0.gate_z, (10, 14));
        let g1 = params.group_rows(1).unwrap();
        assert_eq!(g1.query, (14, 17));
        assert_eq!(g1.key, (17, 20));
        assert_eq!(g1.value, (20, 24));
        assert_eq!(g1.gate_z, (24, 28));
        // ba groups: width 4, b then a.
        assert_eq!(
            params.ba_group_rows(0).unwrap(),
            BaGroupRows {
                b: (0, 2),
                a: (2, 4)
            }
        );
        assert_eq!(
            params.ba_group_rows(1).unwrap(),
            BaGroupRows {
                b: (4, 6),
                a: (6, 8)
            }
        );
        // Total projection widths.
        assert_eq!(2 * params.group_width().unwrap(), 28);
        assert_eq!(2 * params.value_heads, 8);
    }

    #[test]
    fn b5_fractional_ratio_rejected() {
        let params = LinearGroupParams {
            key_heads: 3,
            value_heads: 4,
            key_head_dim: 3,
            value_head_dim: 2,
        };
        assert!(params.ratio().is_err());
        assert!(params.group_width().is_err());
    }

    // ---- B.6: layer schedule ----

    #[test]
    fn b6_layer_schedule_reference_vector() {
        let mut pack = Pack {
            id_name: "test".into(),
            version: "1".into(),
            description: String::new(),
            config: [("num_layers".to_string(), 8)].into_iter().collect(),
            bindings: vec![Binding {
                pattern: "w".into(),
                component: "w".into(),
                kind: BindingKind::Dense,
                shape: vec!["1".into()],
            }],
            layer_types_explicit: None,
            full_attention_interval: Some(4),
        };
        // Interval fallback: (i+1) % 4 == 0 → [3, 7]. Not i % 4 == 0.
        assert_eq!(pack.full_attention_layers().unwrap(), vec![3, 7]);
        // Explicit schedule wins.
        pack.layer_types_explicit =
            Some(vec![true, false, false, false, true, false, false, false]);
        assert_eq!(pack.full_attention_layers().unwrap(), vec![0, 4]);
        // Zero interval is malformed.
        pack.full_attention_interval = Some(0);
        pack.layer_types_explicit = None;
        assert!(pack.full_attention_layers().is_err());
    }

    // ---- B.7: zero-centered norm ----

    #[test]
    fn b7_norm_effective_scale_reference_vector() {
        // stored [-1, 0, 0.5] → effective [0, 1, 1.5].
        assert_eq!(norm_effective_scale(-1.0), 0.0);
        assert_eq!(norm_effective_scale(0.0), 1.0);
        assert_eq!(norm_effective_scale(0.5), 1.5);
    }

    // ---- expressions ----

    #[test]
    fn expressions_evaluate_with_checked_math() {
        let params: BTreeMap<String, u64> = [
            ("H".to_string(), 2),
            ("D".to_string(), 3),
            ("d".to_string(), 4),
        ]
        .into_iter()
        .collect();
        assert_eq!(
            eval_expression(&parse_expression("2*H*D").unwrap(), &params).unwrap(),
            12
        );
        assert_eq!(
            eval_expression(&parse_expression("(H + D) * 2").unwrap(), &params).unwrap(),
            10
        );
        assert_eq!(
            eval_expression(&parse_expression("8 / 4").unwrap(), &params).unwrap(),
            2
        );
        assert!(parse_expression("2**H").is_err());
        assert!(parse_expression("").is_err());
        assert!(parse_expression("2 + ").is_err());
        assert!(eval_expression(&parse_expression("X").unwrap(), &params).is_err());
        assert!(eval_expression(&parse_expression("3 / 2").unwrap(), &params).is_err());
        assert!(eval_expression(&parse_expression("2 - 3").unwrap(), &params).is_err());
        assert!(eval_expression(
            &parse_expression("18446744073709551615 * 2").unwrap(),
            &params
        )
        .is_err());
    }

    // ---- patterns ----

    #[test]
    fn patterns_capture_decimal_segments() {
        let tokens = validate_pattern("model.layers.{layer}.self_attn.q_proj.weight").unwrap();
        let captures = match_pattern(&tokens, "model.layers.12.self_attn.q_proj.weight").unwrap();
        assert_eq!(captures.get("layer"), Some(&12));
        assert!(match_pattern(&tokens, "model.layers.12.self_attn.k_proj.weight").is_none());
        assert!(match_pattern(&tokens, "model.layers.x.self_attn.q_proj.weight").is_none());
        assert!(match_pattern(&tokens, "model.layers..self_attn.q_proj.weight").is_none());

        let trailing = validate_pattern("prefix.{n}").unwrap();
        assert_eq!(
            match_pattern(&trailing, "prefix.42").unwrap().get("n"),
            Some(&42)
        );
        assert!(match_pattern(&trailing, "prefix.42x").is_none());
        assert!(validate_pattern("bad{unclosed").is_err());
        assert!(validate_pattern("empty{}").is_err());
        assert!(validate_pattern("{a}{b}").is_err());
    }

    // ---- pack parsing and identity ----

    fn reference_pack_yaml() -> String {
        r#"schema: binfiddle.nn.pack/v1
id: qwen3-next.reference
version: "1.0.0"
description: worked reference profile
config:
  hidden_size: 4
  num_layers: 8
  num_attention_heads: 2
  head_dim: 3
  num_key_heads: 2
  num_value_heads: 4
  key_head_dim: 3
  value_head_dim: 2
bindings:
  - pattern: "model.layers.{layer}.self_attn.q_proj.weight"
    component: "decoder.layers[{layer}].attention.query_gate"
    kind: query_gate
    shape: ["2*num_attention_heads*head_dim", "hidden_size"]
  - pattern: "model.layers.{layer}.self_attn.o_proj.weight"
    component: "decoder.layers[{layer}].attention.output"
    kind: dense
    shape: ["hidden_size", "num_attention_heads*head_dim"]
  - pattern: "model.layers.{layer}.input_layernorm.weight"
    component: "decoder.layers[{layer}].input_norm"
    kind: norm_zero_centered
    shape: ["hidden_size"]
  - pattern: "model.layers.{layer}.linear_attn.in_proj.weight"
    component: "decoder.layers[{layer}].linear_attention.in_projections"
    kind: linear_qkvz_groups
    shape: ["num_key_heads*(2*key_head_dim + 2*(num_value_heads/num_key_heads)*value_head_dim)", "hidden_size"]
  - pattern: "model.layers.{layer}.linear_attn.ba_proj.weight"
    component: "decoder.layers[{layer}].linear_attention.ba"
    kind: linear_ba_groups
    shape: ["2*num_value_heads", "hidden_size"]
full_attention_interval: 4
"#
        .to_string()
    }

    #[test]
    fn pack_parses_and_has_stable_identity() {
        let pack = Pack::parse(&reference_pack_yaml()).unwrap();
        assert_eq!(pack.id_name, "qwen3-next.reference");
        assert_eq!(pack.bindings.len(), 5);
        assert!(pack.pack_id().unwrap().starts_with("pack:"));
        // Same text parses to the same id.
        let again = Pack::parse(&reference_pack_yaml()).unwrap();
        assert_eq!(pack.pack_id().unwrap(), again.pack_id().unwrap());
        // Shape expressions evaluate against the (B-vector) config.
        assert_eq!(pack.eval("2*num_attention_heads*head_dim").unwrap(), 12);
        assert_eq!(
            pack.eval(
                "num_key_heads*(2*key_head_dim + 2*(num_value_heads/num_key_heads)*value_head_dim)"
            )
            .unwrap(),
            28
        );
        assert_eq!(pack.eval("2*num_value_heads").unwrap(), 8);
        assert_eq!(pack.full_attention_layers().unwrap(), vec![3, 7]);
    }

    #[test]
    fn pack_rejects_bad_schema_and_expressions() {
        assert!(Pack::parse("schema: other").is_err());
        assert!(
            Pack::parse(&reference_pack_yaml().replace("binfiddle.nn.pack/v1", "nope")).is_err()
        );
        let bad_expr = reference_pack_yaml().replace("\"hidden_size\"]", "\"hidden_size + *2\"]");
        assert!(Pack::parse(&bad_expr).is_err());
        let unknown_param = reference_pack_yaml().replace("\"hidden_size\"]", "\"not_a_param\"]");
        // Parsing succeeds (expression is syntactically valid); evaluation
        // fails during recognition. Direct eval catches it:
        let pack = Pack::parse(&unknown_param).unwrap();
        assert!(pack.eval("not_a_param").is_err());
    }
}
