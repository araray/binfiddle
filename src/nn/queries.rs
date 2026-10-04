//! Catalog listing queries: filters, deterministic ordering, bounded
//! pagination, and rendering for `nn ls`.
//!
//! Filters compose; ordering is explicit; pagination is bounded by a hard
//! limit; unknown payload sizes sort separately; total counts are always
//! reported so partial listings cannot masquerade as complete ones.

use super::catalog::{Catalog, CatalogTensor};
use super::error::NnError;
use super::format::Extent;
use super::json::Json;
use super::report::ResultEnvelope;
use std::cmp::Ordering;

/// Hard ceiling for one listing page.
pub const MAX_LIMIT: usize = 10_000;
/// Default page size.
pub const DEFAULT_LIMIT: usize = 1_000;

/// Listing filters.
#[derive(Debug, Clone, Default)]
pub struct ListFilters {
    /// Exact encoding id match (e.g. `safetensors.F32`).
    pub encoding: Option<String>,
    /// Source scope: unique id prefix or exact path.
    pub source: Option<String>,
    /// Bounded regular expression over original tensor names.
    pub name_regex: Option<String>,
}

/// Sort policies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortField {
    /// Default: source order, then original name.
    Name,
    /// Payload bytes, descending; unknown sizes sort last.
    Bytes,
}

impl SortField {
    pub fn as_str(self) -> &'static str {
        match self {
            SortField::Name => "name",
            SortField::Bytes => "bytes",
        }
    }
}

/// One page of a listing.
#[derive(Debug)]
pub struct ListPage {
    pub total_matches: usize,
    pub offset: usize,
    pub limit: usize,
    pub items: Vec<usize>, // indexes into catalog.tensors (or sources)
}

/// Validate a limit/offset pair.
pub fn clamp_pagination(limit: Option<usize>, offset: usize) -> Result<(usize, usize), NnError> {
    let limit = limit.unwrap_or(DEFAULT_LIMIT);
    if limit == 0 || limit > MAX_LIMIT {
        return Err(NnError::InvalidRequest {
            message: format!("list limit must be between 1 and {}", MAX_LIMIT),
        });
    }
    Ok((limit, offset))
}

/// Filter tensors. The name regex compiles once; failures are usage errors.
fn tensor_matches(
    catalog: &Catalog,
    tensor: &CatalogTensor,
    filters: &ListFilters,
    regex: &Option<regex::Regex>,
    source_id: &Option<String>,
) -> bool {
    if let Some(encoding) = &filters.encoding {
        if &tensor.encoding != encoding {
            return false;
        }
    }
    if let Some(source) = source_id {
        if &tensor.source_id != source {
            return false;
        }
    }
    if let Some(regex) = regex {
        if !regex.is_match(&tensor.original_name) {
            return false;
        }
    }
    let _ = catalog;
    true
}

/// List tensors with filters, sorting, and pagination.
pub fn list_tensors(
    catalog: &Catalog,
    filters: &ListFilters,
    sort: SortField,
    limit: usize,
    offset: usize,
) -> Result<ListPage, NnError> {
    let regex = compile_name_regex(filters.name_regex.as_deref())?;
    let source_id = match &filters.source {
        Some(scope) => Some(catalog.resolve_source(scope)?.id.clone()),
        None => None,
    };
    let mut indexes: Vec<usize> = catalog
        .tensors
        .iter()
        .enumerate()
        .filter(|(_, t)| tensor_matches(catalog, t, filters, &regex, &source_id))
        .map(|(i, _)| i)
        .collect();

    match sort {
        SortField::Name => {
            // Discovery order (source, then directory order) is already
            // stable; ties fall back to it via sort_by_key on position.
            indexes.sort_by(|&a, &b| {
                let ta = &catalog.tensors[a];
                let tb = &catalog.tensors[b];
                (&ta.source_id, &ta.original_name, a).cmp(&(&tb.source_id, &tb.original_name, b))
            });
        }
        SortField::Bytes => indexes.sort_by(|&a, &b| {
            let ta = &catalog.tensors[a];
            let tb = &catalog.tensors[b];
            match (ta.payload_length, tb.payload_length) {
                (Some(x), Some(y)) => y.cmp(&x),
                (Some(_), None) => Ordering::Less,
                (None, Some(_)) => Ordering::Greater,
                (None, None) => Ordering::Equal,
            }
            .then_with(|| {
                (&ta.source_id, &ta.original_name, a).cmp(&(&tb.source_id, &tb.original_name, b))
            })
        }),
    }

    let total = indexes.len();
    let items = indexes
        .into_iter()
        .skip(offset)
        .take(limit)
        .collect::<Vec<_>>();
    Ok(ListPage {
        total_matches: total,
        offset,
        limit,
        items,
    })
}

fn compile_name_regex(pattern: Option<&str>) -> Result<Option<regex::Regex>, NnError> {
    match pattern {
        Some(pattern) => {
            regex::Regex::new(pattern)
                .map(Some)
                .map_err(|e| NnError::InvalidRequest {
                    message: format!("invalid name regex: {e}"),
                })
        }
        None => Ok(None),
    }
}

/// Tensor record as JSON.
pub fn tensor_record(tensor: &CatalogTensor) -> Result<Json, NnError> {
    Json::object(vec![
        ("id", Json::Str(tensor.id.clone())),
        ("source_id", Json::Str(tensor.source_id.clone())),
        ("name", Json::Str(tensor.original_name.clone())),
        ("encoding", Json::Str(tensor.encoding.clone())),
        (
            "shape",
            Json::Array(
                tensor
                    .shape
                    .iter()
                    .map(|d| Json::Str(d.to_string()))
                    .collect(),
            ),
        ),
        ("elements", Json::Str(tensor.element_count.to_string())),
        ("payload_start", Json::Str(tensor.payload_start.to_string())),
        (
            "payload_length",
            match tensor.payload_length {
                Some(len) => Json::Str(len.to_string()),
                None => Json::Null,
            },
        ),
        (
            "extent",
            Json::Str(
                match tensor.extent {
                    Extent::Exact => "exact",
                    Extent::UpperBoundOnly => "upper_bound_only",
                }
                .to_string(),
            ),
        ),
        ("decode_supported", Json::Bool(tensor.decode_supported)),
    ])
}

/// Build the `ls` result envelope for the tensors view.
pub fn tensors_envelope(
    catalog: &Catalog,
    page: &ListPage,
    filters: &ListFilters,
    sort: SortField,
) -> Result<ResultEnvelope, NnError> {
    let items = page
        .items
        .iter()
        .map(|&i| tensor_record(&catalog.tensors[i]))
        .collect::<Result<Vec<_>, _>>()?;
    let mut filter_pairs: Vec<(&str, Json)> = Vec::new();
    if let Some(encoding) = &filters.encoding {
        filter_pairs.push(("encoding", Json::Str(encoding.clone())));
    }
    if let Some(source) = &filters.source {
        filter_pairs.push(("source", Json::Str(source.clone())));
    }
    if let Some(regex) = &filters.name_regex {
        filter_pairs.push(("name_regex", Json::Str(regex.clone())));
    }
    let semantic = Json::object(vec![
        ("view", Json::Str("tensors".to_string())),
        ("catalog_id", Json::Str(catalog.id()?)),
        (
            "filters",
            Json::Object(
                filter_pairs
                    .into_iter()
                    .map(|(k, v)| (k.to_string(), v))
                    .collect(),
            ),
        ),
        ("sort", Json::Str(sort.as_str().to_string())),
        (
            "pagination",
            Json::object(vec![
                ("total_matches", Json::Str(page.total_matches.to_string())),
                ("offset", Json::Str(page.offset.to_string())),
                ("limit", Json::Str(page.limit.to_string())),
                ("returned", Json::Str(page.items.len().to_string())),
            ])?,
        ),
        ("items", Json::Array(items)),
    ])?;
    let envelope = ResultEnvelope::new("ls").with_semantic(semantic);
    Ok(envelope)
}

/// Build the `ls` result envelope for the sources view.
pub fn sources_envelope(catalog: &Catalog) -> Result<ResultEnvelope, NnError> {
    let mut items = Vec::with_capacity(catalog.sources.len());
    for source in &catalog.sources {
        let tensor_count = catalog
            .tensors
            .iter()
            .filter(|t| t.source_id == source.id)
            .count();
        items.push(Json::object(vec![
            ("id", Json::Str(source.id.clone())),
            ("path", Json::Str(source.path.clone())),
            ("outcome", Json::Str(source.outcome.as_str().to_string())),
            (
                "format",
                match &source.format {
                    Some(format) => Json::Str(format.clone()),
                    None => Json::Null,
                },
            ),
            ("tensors", Json::Str(tensor_count.to_string())),
        ])?);
    }
    for member in &catalog.unresolved {
        items.push(Json::object(vec![
            ("id", Json::Null),
            ("path", Json::Str(member.path.clone())),
            ("outcome", Json::Str(member.outcome.as_str().to_string())),
            ("format", Json::Null),
            ("tensors", Json::Str("0".to_string())),
        ])?);
    }
    let semantic = Json::object(vec![
        ("view", Json::Str("sources".to_string())),
        ("catalog_id", Json::Str(catalog.id()?)),
        ("items", Json::Array(items)),
    ])?;
    Ok(ResultEnvelope::new("ls").with_semantic(semantic))
}

/// Human-readable listing for the tensors view.
pub fn tensors_text(catalog: &Catalog, page: &ListPage) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "{:<4} {:<28} {:<18} {:<14} {:<12} {}\n",
        "#", "name", "encoding", "elements", "bytes", "tensor id"
    ));
    for (row, &index) in page.items.iter().enumerate() {
        let tensor = &catalog.tensors[index];
        let shape = tensor
            .shape
            .iter()
            .map(|d| d.to_string())
            .collect::<Vec<_>>()
            .join("x");
        out.push_str(&format!(
            "{:<4} {:<28} {:<18} {:<14} {:<12} {}\n",
            page.offset + row + 1,
            truncate(&tensor.original_name, 28),
            truncate(&tensor.encoding, 18),
            format!("{} [{}]", tensor.element_count, shape),
            tensor
                .payload_length
                .map(|l| l.to_string())
                .unwrap_or_else(|| "?".to_string()),
            &tensor.id[..24.min(tensor.id.len())]
        ));
    }
    out.push_str(&format!(
        "\n{} of {} tensors (offset {}, limit {})\n",
        page.items.len(),
        page.total_matches,
        page.offset,
        page.limit
    ));
    out
}

/// Human-readable listing for the sources view.
pub fn sources_text(catalog: &Catalog) -> String {
    let mut out = String::from("sources:\n");
    for source in &catalog.sources {
        out.push_str(&format!(
            "  {} [{}] tensors in {} ({})\n",
            if source.path.is_empty() {
                source.id.as_str()
            } else {
                &source.path
            },
            source.outcome.as_str(),
            source.format.as_deref().unwrap_or("unknown format"),
            &source.id[..20.min(source.id.len())]
        ));
    }
    for member in &catalog.unresolved {
        out.push_str(&format!(
            "  {} [{}] unresolved: {}\n",
            member.path,
            member.outcome.as_str(),
            super::error::brief(&member.note)
        ));
    }
    out
}

fn truncate(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        text.to_string()
    } else {
        let mut result: String = text.chars().take(width.saturating_sub(1)).collect();
        result.push('…');
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nn::budget::{Budget, BudgetCaps};
    use crate::nn::cancel::CancellationToken;
    use crate::nn::catalog::Catalog;
    use crate::nn::discover::{discover, DiscoverOptions};
    use std::path::Path;

    fn budget() -> Budget {
        Budget::new(BudgetCaps::default(), None, CancellationToken::new())
    }

    fn write_safetensors(dir: &Path, name: &str, tensors: &[(&str, &str, &[u64], &[u8])]) {
        let mut body: Vec<u8> = Vec::new();
        let mut spans = Vec::new();
        for (tname, dtype, shape, payload) in tensors {
            let begin = body.len();
            body.extend_from_slice(payload);
            spans.push((tname, dtype, shape, begin, begin + payload.len()));
        }
        let entries: Vec<String> = spans
            .iter()
            .map(|(n, dt, sh, b, e)| {
                let dims: Vec<String> = sh.iter().map(|d| d.to_string()).collect();
                format!(
                    "\"{}\":{{\"dtype\":\"{}\",\"shape\":[{}],\"data_offsets\":[{},{}]}}",
                    n,
                    dt,
                    dims.join(","),
                    b,
                    e
                )
            })
            .collect();
        let header = format!("{{{}}}", entries.join(","));
        let mut data = Vec::new();
        data.extend_from_slice(&(header.len() as u64).to_le_bytes());
        data.extend_from_slice(header.as_bytes());
        data.extend_from_slice(&body);
        std::fs::write(dir.join(name), data).unwrap();
    }

    fn sample_catalog(dir: &Path) -> Catalog {
        write_safetensors(
            dir,
            "m.safetensors",
            &[
                ("model.alpha", "F32", &[2, 3], &[0u8; 24]),
                ("model.beta", "U8", &[64], &[7u8; 64]),
                ("other.gamma", "F16", &[2], &[0, 0, 0, 0]),
            ],
        );
        let report = discover(
            &dir.join("m.safetensors"),
            &DiscoverOptions::default(),
            &budget(),
        )
        .unwrap();
        Catalog::from_discovery(&report).unwrap()
    }

    #[test]
    fn lists_all_tensors_by_name_order() {
        let dir = tempfile::tempdir().unwrap();
        let catalog = sample_catalog(dir.path());
        let page = list_tensors(
            &catalog,
            &ListFilters::default(),
            SortField::Name,
            DEFAULT_LIMIT,
            0,
        )
        .unwrap();
        assert_eq!(page.total_matches, 3);
        let names: Vec<&str> = page
            .items
            .iter()
            .map(|&i| catalog.tensors[i].original_name.as_str())
            .collect();
        assert_eq!(names, vec!["model.alpha", "model.beta", "other.gamma"]);
    }

    #[test]
    fn filters_by_encoding_and_name_regex() {
        let dir = tempfile::tempdir().unwrap();
        let catalog = sample_catalog(dir.path());
        let filters = ListFilters {
            encoding: Some("safetensors.F32".to_string()),
            ..ListFilters::default()
        };
        let page = list_tensors(&catalog, &filters, SortField::Name, DEFAULT_LIMIT, 0).unwrap();
        assert_eq!(page.total_matches, 1);
        assert_eq!(catalog.tensors[page.items[0]].original_name, "model.alpha");

        let filters = ListFilters {
            name_regex: Some("^model\\.".to_string()),
            ..ListFilters::default()
        };
        let page = list_tensors(&catalog, &filters, SortField::Name, DEFAULT_LIMIT, 0).unwrap();
        assert_eq!(page.total_matches, 2);
    }

    #[test]
    fn sorts_by_bytes_descending() {
        let dir = tempfile::tempdir().unwrap();
        let catalog = sample_catalog(dir.path());
        let page = list_tensors(
            &catalog,
            &ListFilters::default(),
            SortField::Bytes,
            DEFAULT_LIMIT,
            0,
        )
        .unwrap();
        let sizes: Vec<Option<u64>> = page
            .items
            .iter()
            .map(|&i| catalog.tensors[i].payload_length)
            .collect();
        assert_eq!(sizes, vec![Some(64), Some(24), Some(4)]);
    }

    #[test]
    fn paginates_with_totals() {
        let dir = tempfile::tempdir().unwrap();
        let catalog = sample_catalog(dir.path());
        let page = list_tensors(&catalog, &ListFilters::default(), SortField::Name, 2, 0).unwrap();
        assert_eq!(page.items.len(), 2);
        assert_eq!(page.total_matches, 3);
        let page = list_tensors(&catalog, &ListFilters::default(), SortField::Name, 2, 2).unwrap();
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.total_matches, 3);
        // Beyond the end: empty page but honest totals.
        let page = list_tensors(&catalog, &ListFilters::default(), SortField::Name, 2, 9).unwrap();
        assert!(page.items.is_empty());
        assert_eq!(page.total_matches, 3);
    }

    #[test]
    fn rejects_bad_regex_and_limits() {
        let dir = tempfile::tempdir().unwrap();
        let catalog = sample_catalog(dir.path());
        let filters = ListFilters {
            name_regex: Some("(unclosed".to_string()),
            ..ListFilters::default()
        };
        assert_eq!(
            list_tensors(&catalog, &filters, SortField::Name, 10, 0)
                .unwrap_err()
                .code()
                .as_str(),
            "INVALID_REQUEST"
        );
        assert!(clamp_pagination(Some(0), 0).is_err());
        assert!(clamp_pagination(Some(MAX_LIMIT + 1), 0).is_err());
        assert_eq!(clamp_pagination(None, 5).unwrap(), (DEFAULT_LIMIT, 5));
    }

    #[test]
    fn envelopes_and_text_render() {
        let dir = tempfile::tempdir().unwrap();
        let catalog = sample_catalog(dir.path());
        let page = list_tensors(
            &catalog,
            &ListFilters::default(),
            SortField::Name,
            DEFAULT_LIMIT,
            0,
        )
        .unwrap();
        let envelope =
            tensors_envelope(&catalog, &page, &ListFilters::default(), SortField::Name).unwrap();
        let text = envelope.to_json_string().unwrap();
        assert!(text.contains("\"total_matches\":\"3\""));
        assert!(text.contains("\"view\":\"tensors\""));
        let human = tensors_text(&catalog, &page);
        assert!(human.contains("model.alpha"));
        assert!(human.contains("3 tensors"));
        let sources = sources_envelope(&catalog).unwrap();
        assert!(sources
            .to_json_string()
            .unwrap()
            .contains("\"view\":\"sources\""));
        assert!(sources_text(&catalog).contains("m.safetensors"));
    }
}
