// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::fault::{Fault, sort_faults};
use crate::hexutil::to_hex;
use crate::metadata::MessageMetadata;
use crate::model::{ArgumentSpec, ProtectedSlotSpec, SelectorSpec};
use crate::publish::atomic_write_file;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::Path;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoryRecord {
    pub id: String,
    pub resource: String,
    pub namespace: String,
    pub owner: String,
    pub tombstone: bool,
    pub replacement: Option<String>,
    pub format_intent: String,
    pub arguments: Vec<ArgumentSpec>,
    pub protected_slots: Vec<ProtectedSlotSpec>,
    pub selector: Option<SelectorSpec>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoryRegistry {
    pub history_version: u32,
    pub records: Vec<HistoryRecord>,
}

pub fn compute_resource_key(id: &str) -> String {
    format!("m{}", to_hex(id.as_bytes()))
}

pub fn validate_message_id(id: &str, namespace: &str) -> Result<(), &'static str> {
    let parts: Vec<&str> = id.split('.').collect();
    if parts.len() < 2 {
        return Err("invalid-id-syntax");
    }
    for part in &parts {
        if part.is_empty() {
            return Err("invalid-id-syntax");
        }
        let bytes = part.as_bytes();
        if !bytes[0].is_ascii_lowercase() {
            return Err("invalid-id-syntax");
        }
        if !bytes
            .iter()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        {
            return Err("invalid-id-syntax");
        }
    }
    let expected_prefix = format!("{}.", namespace);
    if !id.starts_with(&expected_prefix) {
        return Err("namespace-mismatch");
    }
    Ok(())
}

pub fn check_registry_integrity(reg: &HistoryRegistry, faults: &mut Vec<Fault>) {
    let mut seen_ids = HashSet::new();
    for rec in &reg.records {
        if !seen_ids.insert(&rec.id) {
            faults.push(Fault::new("id-reuse", "*", &rec.id));
        }
        if rec.resource != compute_resource_key(&rec.id) {
            faults.push(Fault::new("resource-mismatch", "*", &rec.id));
        }
    }
}

pub fn check_registry_evolution(
    existing: &HistoryRegistry,
    proposed: &HistoryRegistry,
    faults: &mut Vec<Fault>,
) {
    let proposed_ids: HashSet<&str> = proposed.records.iter().map(|r| r.id.as_str()).collect();
    for ex_rec in &existing.records {
        if !proposed_ids.contains(ex_rec.id.as_str()) {
            faults.push(Fault::new("history-loss", "*", &ex_rec.id));
        }
    }
}

pub fn verify_history_record_against_metadata(
    rec: &HistoryRecord,
    meta: &MessageMetadata,
    expected_owner: &str,
    faults: &mut Vec<Fault>,
) {
    let id = &rec.id;
    let expected_resource = compute_resource_key(id);
    if rec.resource != expected_resource {
        faults.push(Fault::new("resource-mismatch", "*", id));
    }
    if rec.owner != expected_owner {
        faults.push(Fault::new("owner-conflict", "*", id));
    }
    if rec.format_intent != meta.format_intent.as_str() {
        faults.push(Fault::new("format-semantic", "*", id));
    }

    // Arguments:
    // - argument name or type change: signature-mutation
    // - min/max change only: numeric-range
    // - format_intent or numeric style change: format-semantic
    let rec_args_len = rec.arguments.len();
    let meta_args_len = meta.arguments.len();
    if rec_args_len != meta_args_len {
        faults.push(Fault::new("signature-mutation", "*", id));
    } else {
        let mut sig_mut = false;
        let mut num_range = false;
        let mut fmt_sem = false;
        for (ra, ma) in rec.arguments.iter().zip(meta.arguments.iter()) {
            if ra.name != ma.name || ra.arg_type != ma.arg_type {
                sig_mut = true;
            } else {
                if ra.numeric_style != ma.numeric_style {
                    fmt_sem = true;
                }
                if ra.min != ma.min || ra.max != ma.max {
                    num_range = true;
                }
            }
        }
        if sig_mut {
            faults.push(Fault::new("signature-mutation", "*", id));
        } else {
            if fmt_sem {
                faults.push(Fault::new("format-semantic", "*", id));
            }
            if num_range {
                faults.push(Fault::new("numeric-range", "*", id));
            }
        }
    }

    // Protected slots:
    // - protected slot name, kind, or value change: protected-slot-mutation
    if rec.protected_slots != meta.protected_slots {
        faults.push(Fault::new("protected-slot-mutation", "*", id));
    }

    // Selector:
    // - selector name or type change: selector-mutation
    // - enum values or default change: enum-mutation
    match (&rec.selector, &meta.selector) {
        (None, None) => {}
        (Some(_), None) | (None, Some(_)) => {
            faults.push(Fault::new("selector-mutation", "*", id));
        }
        (Some(ra), Some(ma)) => {
            if ra.name != ma.name || ra.selector_type != ma.selector_type {
                faults.push(Fault::new("selector-mutation", "*", id));
            } else if ra.enum_values != ma.enum_values || ra.enum_default != ma.enum_default {
                faults.push(Fault::new("enum-mutation", "*", id));
            }
        }
    }
}

impl HistoryRegistry {
    pub fn load_from_file(path: &Path) -> Result<Self, Fault> {
        let content =
            std::fs::read_to_string(path).map_err(|_| Fault::catalog("history-missing"))?;
        let reg: Self =
            serde_json::from_str(&content).map_err(|_| Fault::catalog("history-syntax"))?;
        if reg.history_version != 1 {
            return Err(Fault::catalog("unsupported-version"));
        }
        Ok(reg)
    }

    pub fn save_to_file(&self, path: &Path) -> Result<(), Fault> {
        let mut content =
            serde_json::to_string_pretty(self).map_err(|_| Fault::catalog("history-syntax"))?;
        content.push('\n');
        atomic_write_file(path, content.as_bytes())
    }

    pub fn check_against_source(
        &self,
        source_ids: &[String],
        metadata_map: &HashMap<String, MessageMetadata>,
        namespace_owners: &HashMap<String, String>,
    ) -> Result<(), Vec<Fault>> {
        let mut faults = Vec::new();
        check_registry_integrity(self, &mut faults);

        let records_by_id: HashMap<&str, &HistoryRecord> =
            self.records.iter().map(|r| (r.id.as_str(), r)).collect();

        for id in source_ids {
            let meta = match metadata_map.get(id) {
                Some(m) => m,
                None => continue,
            };

            let ns = match id.rsplit_once('.') {
                Some((prefix, _)) => prefix,
                None => {
                    faults.push(Fault::new("invalid-id-syntax", "*", id));
                    continue;
                }
            };

            let expected_owner = match namespace_owners.get(ns) {
                Some(o) => o,
                None => {
                    faults.push(Fault::new("owner-conflict", "*", id));
                    continue;
                }
            };

            if let Some(rec) = records_by_id.get(id.as_str()) {
                if rec.tombstone {
                    faults.push(Fault::new("tombstone-reuse", "*", id));
                }
                verify_history_record_against_metadata(rec, meta, expected_owner, &mut faults);
            } else {
                faults.push(Fault::new("history-missing", "*", id));
            }
        }

        if !faults.is_empty() {
            sort_faults(&mut faults);
            Err(faults)
        } else {
            Ok(())
        }
    }
}
