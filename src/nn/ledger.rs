//! Per-GPU memory ledger (BF-005): static fit verdicts for a partition
//! plan against supplied, provenance-labeled device-capacity
//! observations. Byte accounting only — no throughput or concurrency
//! claims, ever.

use super::error::NnError;
use super::json::{Json, ParseLimits};
use super::report::ResultEnvelope;
use std::path::Path;

pub struct DeviceObs {
    pub kind: String,
    pub bytes: u64,
    pub source: String,
    pub observed_at: String,
    pub ecc: String,
    pub units: String,
}

pub struct Device {
    pub name: String,
    pub uuid: Option<String>,
    pub observations: Vec<DeviceObs>,
    pub selected: Option<String>,
}

pub struct DeviceFit {
    pub name: String,
    pub stages: Vec<usize>,
    pub encoded_bytes: u64,
    pub overhead_bytes: u64,
    pub selected_kind: String,
    pub selected_bytes: Option<u64>,
    pub fits: bool,
    pub reasons: Vec<String>,
}

fn get_str(j: &Json, k: &str) -> Option<String> {
    j.get(k).and_then(Json::as_str).map(str::to_string)
}

/// Parse the devices file: [{name, uuid?, selected?, observations:
/// [{kind, bytes, source?, observed_at?, ecc?, units?}]}].
pub fn load_devices(path: &Path) -> Result<Vec<Device>, NnError> {
    let text = std::fs::read_to_string(path).map_err(NnError::Io)?;
    let root = Json::parse_foreign(&text, ParseLimits::for_input_len(text.len()))?;
    let list = root.as_array().ok_or_else(|| NnError::MalformedInput {
        detail: "devices file must be an array".into(),
    })?;
    let mut out = Vec::new();
    for d in list {
        let name = get_str(d, "name").ok_or_else(|| NnError::MalformedInput {
            detail: "device missing name".into(),
        })?;
        let mut observations = Vec::new();
        if let Some(obs) = d.get("observations").and_then(Json::as_array) {
            for o in obs {
                let kind = get_str(o, "kind").ok_or_else(|| NnError::MalformedInput {
                    detail: format!("device {name}: observation missing kind"),
                })?;
                let bytes = o
                    .get("bytes")
                    .and_then(Json::as_number_u64)
                    .or_else(|| get_str(o, "bytes").and_then(|s| s.parse().ok()))
                    .ok_or_else(|| NnError::MalformedInput {
                        detail: format!("device {name}/{kind}: missing or non-numeric bytes"),
                    })?;
                observations.push(DeviceObs {
                    kind,
                    bytes,
                    source: get_str(o, "source").unwrap_or_else(|| "unknown".into()),
                    observed_at: get_str(o, "observed_at").unwrap_or_else(|| "unknown".into()),
                    ecc: get_str(o, "ecc").unwrap_or_else(|| "unknown".into()),
                    units: get_str(o, "units").unwrap_or_else(|| "bytes".into()),
                });
            }
        }
        out.push(Device {
            name,
            uuid: get_str(d, "uuid"),
            selected: get_str(d, "selected"),
            observations,
        });
    }
    if out.is_empty() {
        return Err(NnError::InvalidRequest {
            message: "devices file lists no devices".into(),
        });
    }
    Ok(out)
}

/// Parse the partition envelope for stage weight bytes and unlayered
/// accounting (accepts the nn partition --report-format json output).
pub fn load_plan(path: &Path) -> Result<(Vec<u64>, u64, usize), NnError> {
    let text = std::fs::read_to_string(path).map_err(NnError::Io)?;
    let root = Json::parse_strict(&text, ParseLimits::for_input_len(text.len()))?;
    let sem = root.get("semantic").cloned().unwrap_or(Json::Null);
    let stages: Vec<u64> = sem
        .get("stages")
        .and_then(Json::as_array)
        .map(|a| {
            a.iter()
                .map(|s| {
                    s.get("weight_bytes")
                        .and_then(Json::as_number_u64)
                        .or_else(|| get_str(s, "weight_bytes").and_then(|v| v.parse().ok()))
                        .unwrap_or(0)
                })
                .collect()
        })
        .unwrap_or_default();
    if stages.is_empty() {
        return Err(NnError::InvalidRequest {
            message: "plan carries no stages".into(),
        });
    }
    let unlayered = sem
        .get("unlayered_bytes")
        .and_then(Json::as_number_u64)
        .or_else(|| get_str(&sem, "unlayered_bytes").and_then(|v| v.parse().ok()))
        .unwrap_or(0);
    let unplaced = sem
        .get("unlayered_tensors")
        .and_then(Json::as_number_u64)
        .or_else(|| get_str(&sem, "unlayered_tensors").and_then(|v| v.parse().ok()))
        .unwrap_or(0) as usize;
    Ok((stages, unlayered, unplaced))
}

pub struct LedgerReport {
    pub devices: Vec<DeviceFit>,
    pub replication: u64,
    pub unlayered_bytes: u64,
    pub unlayered_tensors: usize,
    pub aggregate_nominal_fits: Option<bool>,
    pub all_fit: bool,
}

pub fn build(
    stages: &[u64],
    unlayered_bytes: u64,
    unlayered_tensors: usize,
    devices: &[Device],
    replication: u64,
    overheads: &[(String, u64, String)], // (device, bytes, source)
    reject_on_aggregate: bool,
) -> Result<LedgerReport, NnError> {
    let mut fits = Vec::new();
    let mut all_fit = true;
    for (i, dev) in devices.iter().enumerate() {
        let stage_idx: Vec<usize> = (0..stages.len())
            .filter(|s| s % devices.len() == i)
            .collect();
        let encoded: u64 = stage_idx.iter().map(|&s| stages[s]).sum::<u64>() * replication;
        let overhead: u64 = overheads
            .iter()
            .filter(|(d, _, _)| *d == dev.name)
            .map(|(_, b, _)| b)
            .sum();
        let selected_kind = dev.selected.clone().unwrap_or_else(|| "none".into());
        let selected_bytes = dev
            .observations
            .iter()
            .find(|o| o.kind == selected_kind)
            .map(|o| o.bytes);
        let mut reasons = Vec::new();
        let fits_d = match selected_bytes {
            Some(cap) => {
                let total = encoded + overhead;
                if total > cap {
                    reasons.push(format!(
                        "total {total} exceeds selected {selected_kind} capacity {cap}"
                    ));
                    false
                } else {
                    true
                }
            }
            None => {
                reasons.push(format!(
                    "no usable {selected_kind} observation named/selected"
                ));
                false
            }
        };
        if !fits_d {
            all_fit = false;
        }
        fits.push(DeviceFit {
            name: dev.name.clone(),
            stages: stage_idx,
            encoded_bytes: encoded,
            overhead_bytes: overhead,
            selected_kind,
            selected_bytes,
            fits: fits_d,
            reasons,
        });
    }
    let aggregate_nominal_fits = if reject_on_aggregate {
        let nominal: u64 = devices
            .iter()
            .filter_map(|d| {
                d.observations
                    .iter()
                    .find(|o| o.kind == "nominal")
                    .map(|o| o.bytes)
            })
            .sum();
        let need: u64 = stages.iter().sum::<u64>() * replication;
        Some(need <= nominal)
    } else {
        None
    };
    if let Some(agg) = aggregate_nominal_fits {
        if agg && !all_fit {
            all_fit = false; // per-rank failure stands even if aggregate fits
        }
    }
    Ok(LedgerReport {
        devices: fits,
        replication,
        unlayered_bytes,
        unlayered_tensors,
        aggregate_nominal_fits,
        all_fit,
    })
}

pub fn load_overheads(path: &Path) -> Result<Vec<(String, u64, String)>, NnError> {
    let text = std::fs::read_to_string(path).map_err(NnError::Io)?;
    let root = Json::parse_foreign(&text, ParseLimits::for_input_len(text.len()))?;
    let list: Vec<Json> = root.as_array().map(|a| a.to_vec()).unwrap_or_default();
    Ok(list
        .iter()
        .map(|e| {
            (
                get_str(e, "device").unwrap_or_default(),
                e.get("bytes").and_then(Json::as_number_u64).unwrap_or(0),
                get_str(e, "source").unwrap_or_else(|| "unknown".into()),
            )
        })
        .collect())
}

pub fn text(r: &LedgerReport) -> String {
    let mut out = String::from("memory ledger (static byte estimates)\n");
    for d in &r.devices {
        out.push_str(&format!(
            "  {}: stages {:?} encoded {} x{} replication, overheads {}, selected {} ({}) — {}\n",
            d.name,
            d.stages,
            d.encoded_bytes,
            r.replication,
            d.overhead_bytes,
            d.selected_kind,
            d.selected_bytes
                .map(|b| b.to_string())
                .unwrap_or_else(|| "unresolved".into()),
            if d.fits { "fits" } else { "DOES NOT FIT" }
        ));
        for reason in &d.reasons {
            out.push_str(&format!("    reason: {reason}\n"));
        }
    }
    out.push_str(&format!(
        "  unplaced: {} tensors, {} bytes (never silently distributed)\n",
        r.unlayered_tensors, r.unlayered_bytes
    ));
    if let Some(agg) = r.aggregate_nominal_fits {
        out.push_str(&format!(
            "  aggregate nominal would fit: {agg} (per-rank verdicts stand regardless)\n"
        ));
    }
    out.push_str(
        "  claims: static encoded-byte accounting against SUPPLIED capacity observations with provenance; no throughput, concurrency, or runtime-behavior claims\n",
    );
    out
}

pub fn envelope(r: &LedgerReport) -> Result<ResultEnvelope, NnError> {
    let devices = r
        .devices
        .iter()
        .map(|d| {
            Json::object(vec![
                ("name", Json::Str(d.name.clone())),
                ("encoded_bytes", Json::Str(d.encoded_bytes.to_string())),
                ("overhead_bytes", Json::Str(d.overhead_bytes.to_string())),
                ("selected_kind", Json::Str(d.selected_kind.clone())),
                (
                    "selected_bytes",
                    match d.selected_bytes {
                        Some(b) => Json::Str(b.to_string()),
                        None => Json::Null,
                    },
                ),
                ("fits", Json::Bool(d.fits)),
            ])
        })
        .collect::<Result<Vec<_>, _>>()?;
    let semantic = Json::object(vec![
        ("devices", Json::Array(devices)),
        ("replication", Json::Str(r.replication.to_string())),
        ("unlayered_bytes", Json::Str(r.unlayered_bytes.to_string())),
        ("all_fit", Json::Bool(r.all_fit)),
    ])?;
    Ok(ResultEnvelope::new("ledger").with_semantic(semantic))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(name: &str, obs: Vec<(&str, u64)>, selected: &str) -> Device {
        Device {
            name: name.into(),
            uuid: None,
            selected: Some(selected.into()),
            observations: obs
                .into_iter()
                .map(|(kind, bytes)| DeviceObs {
                    kind: kind.into(),
                    bytes,
                    source: "test".into(),
                    observed_at: "t".into(),
                    ecc: "unknown".into(),
                    units: "bytes".into(),
                })
                .collect(),
        }
    }

    #[test]
    fn per_rank_verdict_beats_aggregate() {
        // 2 devices, nominal 40 GB each (aggregate 80 fits 60 of weights),
        // but one device's selected nvml_total is 4 GB -> per-rank fail.
        let devices = vec![
            dev(
                "d0",
                vec![("nominal", 40_000_000_000), ("nvml_total", 40_000_000_000)],
                "nvml_total",
            ),
            dev(
                "d1",
                vec![("nominal", 40_000_000_000), ("nvml_total", 4_000_000_000)],
                "nvml_total",
            ),
        ];
        let report = build(
            &[30_000_000_000, 30_000_000_000],
            0,
            0,
            &devices,
            1,
            &[],
            true,
        )
        .unwrap();
        assert!(!report.all_fit);
        assert_eq!(report.aggregate_nominal_fits, Some(true)); // aggregate fits anyway
        assert!(report.devices[0].fits); // hosts stage 0 (30G <= 40G selected)
        assert!(!report.devices[1].fits); // hosts stage 1 (30G > 4G selected)
    }

    #[test]
    fn replication_multiplies_per_hosting_device() {
        let devices = vec![dev("d0", vec![("nvml_total", 100)], "nvml_total")];
        let report = build(&[10], 0, 0, &devices, 4, &[], false).unwrap();
        assert_eq!(report.devices[0].encoded_bytes, 40);
        assert!(report.all_fit);
    }

    #[test]
    fn unresolved_selected_observation_fails() {
        let devices = vec![dev("d0", vec![("nominal", 100)], "nvml_total")];
        let report = build(&[10], 0, 0, &devices, 1, &[], false).unwrap();
        assert!(!report.all_fit);
    }
}
