// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Read-only commitment ledger fold used by the journal-data Health API.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use chrono::{DateTime, Utc};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use solstone_core_entity_matching::{EntityNameCandidate, find_matching_entity};
use solstone_core_facets::{
    list_declared_facet_names, load_activity_records, read_facet_declaration,
};

use super::HealthError;

const DAY_MS: i64 = 86_400_000;
const ACTION_MATCH_THRESHOLD: f64 = 78.0;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LedgerItem {
    pub(crate) id: String,
    pub(crate) state: String,
    pub(crate) opened_at: i64,
    pub(crate) age_days: i64,
}

#[derive(Debug, Clone)]
struct Commitment {
    id: String,
    owner_entity_id: Option<String>,
    counterparty_entity_id: Option<String>,
    counterparty_normalized: String,
    action_normalized: String,
    opened_at: i64,
    opening_key: SortKey,
}

#[derive(Debug, Clone)]
struct StoryClosure {
    owner_entity_id: Option<String>,
    counterparty_entity_id: Option<String>,
    counterparty_normalized: String,
    action_normalized: String,
    sort_key: SortKey,
}

#[derive(Debug, Clone)]
struct ManualClose {
    state: String,
    order_key: (i64, String, String, String, usize),
}

type SortKey = (i64, String, String, String);

/// Return open ledger items, optionally retaining only stale entries.
pub(crate) fn list_open(
    journal_root: &Path,
    now: DateTime<Utc>,
    age_days_gte: Option<i64>,
) -> Result<Vec<LedgerItem>, HealthError> {
    let mut commitments = BTreeMap::<String, Commitment>::new();
    let mut closures = Vec::<StoryClosure>::new();
    let mut manual_closes = BTreeMap::<String, Vec<ManualClose>>::new();
    for facet in enabled_facets(journal_root)? {
        for day in activity_days(journal_root, &facet)? {
            let records = load_activity_records(journal_root, &facet, &day, false)
                .map_err(|error| HealthError::internal(error.to_string()))?;
            for record in records {
                scan_record(
                    &facet,
                    &day,
                    &record,
                    &mut commitments,
                    &mut closures,
                    &mut manual_closes,
                );
            }
        }
    }
    closures.sort_by_key(|value| value.sort_key.clone());
    let mut consumed_closures = BTreeSet::new();
    let now_ms = now.timestamp_millis();
    let mut items = commitments
        .into_values()
        .map(|commitment| {
            let matched = closures
                .iter()
                .enumerate()
                .filter_map(|(index, closure)| {
                    (!consumed_closures.contains(&index)
                        && story_closure_matches(&commitment, closure))
                    .then_some((index, closure))
                })
                .collect::<Vec<_>>();
            for (index, _) in &matched {
                consumed_closures.insert(*index);
            }
            let state = resolve_state(
                &matched
                    .into_iter()
                    .map(|(_, value)| value)
                    .collect::<Vec<_>>(),
                manual_closes
                    .get(&commitment.id)
                    .map(Vec::as_slice)
                    .unwrap_or_default(),
            );
            LedgerItem {
                id: commitment.id,
                state,
                opened_at: commitment.opened_at,
                age_days: (now_ms - commitment.opened_at) / DAY_MS,
            }
        })
        .filter(|item| item.state == "open")
        .filter(|item| age_days_gte.is_none_or(|minimum| item.age_days >= minimum))
        .collect::<Vec<_>>();
    items.sort_by(|left, right| {
        (right.age_days, right.opened_at, &right.id).cmp(&(left.age_days, left.opened_at, &left.id))
    });
    Ok(items)
}

fn enabled_facets(journal_root: &Path) -> Result<Vec<String>, HealthError> {
    let facets = list_declared_facet_names(journal_root)
        .map_err(|error| HealthError::internal(error.to_string()))?;
    Ok(facets
        .into_iter()
        .filter(|facet| {
            read_facet_declaration(journal_root, facet)
                .ok()
                .flatten()
                .is_some_and(|declaration| declaration.muted != Some(true))
        })
        .collect())
}

fn activity_days(journal_root: &Path, facet: &str) -> Result<Vec<String>, HealthError> {
    let directory = journal_root.join("facets").join(facet).join("activities");
    let entries = match fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(HealthError::internal(error.to_string())),
    };
    let mut days = entries
        .map(|entry| entry.map(|value| value.path()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| HealthError::internal(error.to_string()))?
        .into_iter()
        .filter(|path| path.is_file())
        .filter_map(|path| {
            let stem = path.file_stem()?.to_str()?;
            (path.extension().and_then(|value| value.to_str()) == Some("jsonl") && valid_day(stem))
                .then(|| stem.to_owned())
        })
        .collect::<Vec<_>>();
    days.sort();
    Ok(days)
}

fn scan_record(
    facet: &str,
    day: &str,
    record: &Map<String, Value>,
    commitments: &mut BTreeMap<String, Commitment>,
    closures: &mut Vec<StoryClosure>,
    manual_closes: &mut BTreeMap<String, Vec<ManualClose>>,
) {
    let record_id = string(record.get("id"));
    if record_id.is_empty() {
        return;
    }
    let created_at = integer(record.get("created_at"));
    let key = |timestamp| {
        (
            timestamp,
            facet.to_owned(),
            day.to_owned(),
            record_id.clone(),
        )
    };
    for raw in values(record.get("commitments")) {
        let Some(value) = raw.as_object() else {
            continue;
        };
        let owner = string(value.get("owner")).trim().to_owned();
        let action = string(value.get("action")).trim().to_owned();
        if owner.is_empty() || action.is_empty() {
            continue;
        }
        let owner_entity_id = optional_string(value.get("owner_entity_id"));
        let counterparty_entity_id = optional_string(value.get("counterparty_entity_id"));
        let id = dedup_key(
            owner_entity_id.as_deref(),
            &normalize(&action),
            counterparty_entity_id.as_deref(),
        );
        let candidate = Commitment {
            id: id.clone(),
            owner_entity_id,
            counterparty_entity_id,
            counterparty_normalized: normalize(&string(value.get("counterparty"))),
            action_normalized: normalize(&action),
            opened_at: created_at,
            opening_key: key(created_at),
        };
        match commitments.get(&id) {
            Some(current) if current.opening_key <= candidate.opening_key => {}
            _ => {
                commitments.insert(id, candidate);
            }
        }
    }
    for raw in values(record.get("closures")) {
        let Some(value) = raw.as_object() else {
            continue;
        };
        let action = string(value.get("action")).trim().to_owned();
        if action.is_empty() {
            continue;
        }
        closures.push(StoryClosure {
            owner_entity_id: optional_string(value.get("owner_entity_id")),
            counterparty_entity_id: optional_string(value.get("counterparty_entity_id")),
            counterparty_normalized: normalize(&string(value.get("counterparty"))),
            action_normalized: normalize(&action),
            sort_key: key(created_at),
        });
    }
    for (index, raw) in values(record.get("edits")).iter().enumerate() {
        let Some(edit) = raw.as_object() else {
            continue;
        };
        if edit.get("fields")
            != Some(&Value::Array(vec![Value::String(
                "ledger_close".to_owned(),
            )]))
        {
            continue;
        }
        let Some(close) = edit.get("ledger_close").and_then(Value::as_object) else {
            continue;
        };
        let item_id = string(close.get("item_id"));
        let state = string(close.get("as_state"));
        if item_id.is_empty() || !matches!(state.as_str(), "closed" | "dropped") {
            continue;
        }
        let closed_at = parse_edit_timestamp(edit.get("timestamp")).unwrap_or(created_at);
        manual_closes.entry(item_id).or_default().push(ManualClose {
            state,
            order_key: (
                closed_at,
                facet.to_owned(),
                day.to_owned(),
                record_id.clone(),
                index,
            ),
        });
    }
}

fn story_closure_matches(item: &Commitment, closure: &StoryClosure) -> bool {
    entity_pair_matches(
        item.owner_entity_id.as_deref(),
        closure.owner_entity_id.as_deref(),
        true,
    ) && counterparty_matches(item, closure)
        && actions_match(&item.action_normalized, &closure.action_normalized)
}

fn entity_pair_matches(left: Option<&str>, right: Option<&str>, allow_both_missing: bool) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => left == right,
        (None, None) => allow_both_missing,
        _ => false,
    }
}

fn counterparty_matches(item: &Commitment, closure: &StoryClosure) -> bool {
    match (
        item.counterparty_entity_id.as_deref(),
        closure.counterparty_entity_id.as_deref(),
    ) {
        (Some(left), Some(right)) => left == right,
        (None, None) => item.counterparty_normalized == closure.counterparty_normalized,
        _ => false,
    }
}

fn actions_match(left: &str, right: &str) -> bool {
    !left.is_empty()
        && !right.is_empty()
        && find_matching_entity(
            left,
            &[EntityNameCandidate {
                id: None,
                name: right.to_owned(),
                aka: Vec::new(),
                emails: Vec::new(),
            }],
            ACTION_MATCH_THRESHOLD,
        )
        .is_some()
}

fn resolve_state(story: &[&StoryClosure], manual: &[ManualClose]) -> String {
    if let Some(latest) = manual.iter().max_by_key(|value| value.order_key.clone()) {
        return latest.state.clone();
    }
    if story
        .iter()
        .min_by_key(|value| value.sort_key.clone())
        .is_some()
    {
        return "closed".to_owned();
    }
    "open".to_owned()
}

fn dedup_key(owner: Option<&str>, action: &str, counterparty: Option<&str>) -> String {
    let mut digest = Sha256::new();
    digest.update(owner.unwrap_or_default());
    digest.update("|");
    digest.update(action);
    digest.update("|");
    digest.update(counterparty.unwrap_or_default());
    format!("{:x}", digest.finalize())[..16].to_owned()
}

fn normalize(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn valid_day(value: &str) -> bool {
    value.len() == 8
        && value.as_bytes().iter().all(u8::is_ascii_digit)
        && chrono::NaiveDate::parse_from_str(value, "%Y%m%d").is_ok()
}

fn values(value: Option<&Value>) -> &[Value] {
    value
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
}

fn string(value: Option<&Value>) -> String {
    value.and_then(Value::as_str).unwrap_or_default().to_owned()
}

fn optional_string(value: Option<&Value>) -> Option<String> {
    value.and_then(Value::as_str).map(str::to_owned)
}

fn integer(value: Option<&Value>) -> i64 {
    value.and_then(Value::as_i64).unwrap_or(0)
}

fn parse_edit_timestamp(value: Option<&Value>) -> Option<i64> {
    value
        .and_then(Value::as_str)
        .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
        .map(|value| value.timestamp_millis())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use chrono::{Duration, TimeZone, Utc};
    use serde_json::{Value, json};
    use tempfile::TempDir;

    use super::{
        Commitment, ManualClose, StoryClosure, list_open, resolve_state, story_closure_matches,
    };

    fn temporary() -> TempDir {
        TempDir::new_in("/var/tmp").unwrap()
    }

    fn now() -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 4, 10, 12, 0, 0).unwrap()
    }

    fn write_rows(root: &Path, rows: &[Value]) {
        let path = root.join("facets/work/activities/20260401.jsonl");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            path,
            format!(
                "{}\n",
                rows.iter()
                    .map(|row| serde_json::to_string(row).unwrap())
                    .collect::<Vec<_>>()
                    .join("\n")
            ),
        )
        .unwrap();
        let declaration = root.join("facets/work/facet.json");
        fs::write(declaration, r#"{"name":"work"}"#).unwrap();
    }

    fn commitment_row(id: &str, opened_at: i64) -> Value {
        json!({"id":id,"created_at":opened_at,"commitments":[{"owner":"Owner","action":"send report"}]})
    }

    fn commitment() -> Commitment {
        Commitment {
            id: "item".into(),
            owner_entity_id: None,
            counterparty_entity_id: None,
            counterparty_normalized: "pat".into(),
            action_normalized: "send report".into(),
            opened_at: 0,
            opening_key: (0, String::new(), String::new(), String::new()),
        }
    }

    fn closure() -> StoryClosure {
        StoryClosure {
            owner_entity_id: None,
            counterparty_entity_id: None,
            counterparty_normalized: "pat".into(),
            action_normalized: "send the report".into(),
            sort_key: (1, String::new(), String::new(), String::new()),
        }
    }

    #[test]
    fn story_closures_close_matching_commitments_and_manual_edits_win() {
        let item = commitment();
        let story = closure();
        assert!(story_closure_matches(&item, &story));
        assert_eq!(resolve_state(&[&story], &[]), "closed");
        assert_eq!(
            resolve_state(
                &[&story],
                &[ManualClose {
                    state: "dropped".into(),
                    order_key: (2, String::new(), String::new(), String::new(), 0)
                }]
            ),
            "dropped"
        );
    }

    #[test]
    fn list_open_returns_a_plain_open_commitment() {
        let temporary = temporary();
        write_rows(
            temporary.path(),
            &[commitment_row(
                "open",
                (now() - Duration::days(3)).timestamp_millis(),
            )],
        );
        let items = list_open(temporary.path(), now(), None).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].state, "open");
        assert_eq!(items[0].age_days, 3);
    }

    #[test]
    fn list_open_stale_filter_uses_injected_report_clock() {
        let temporary = temporary();
        write_rows(
            temporary.path(),
            &[
                commitment_row("old", (now() - Duration::days(20)).timestamp_millis()),
                commitment_row("recent", (now() - Duration::days(3)).timestamp_millis()),
            ],
        );
        let items = list_open(temporary.path(), now(), Some(14)).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].age_days, 20);
    }

    #[test]
    fn list_open_excludes_a_manually_dropped_item() {
        let temporary = temporary();
        let opened_at = (now() - Duration::days(3)).timestamp_millis();
        write_rows(temporary.path(), &[commitment_row("open", opened_at)]);
        let item_id = list_open(temporary.path(), now(), None).unwrap()[0]
            .id
            .clone();
        write_rows(
            temporary.path(),
            &[
                commitment_row("open", opened_at),
                json!({"id":"close","created_at":now().timestamp_millis(),"edits":[{"fields":["ledger_close"],"timestamp":"2026-04-10T12:00:00Z","ledger_close":{"item_id":item_id,"as_state":"dropped"}}]}),
            ],
        );
        assert!(list_open(temporary.path(), now(), None).unwrap().is_empty());
    }
}
