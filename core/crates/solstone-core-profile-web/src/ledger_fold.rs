// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Read-only commitment, closure, and decision folds for profile responses.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use chrono::{DateTime, NaiveDate, Utc};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use solstone_core_entity_matching::{EntityNameCandidate, find_matching_entity};
use solstone_core_facets::{
    activity_value_or_empty, list_declared_facet_names, load_activity_records,
    read_facet_declaration,
};

use crate::error::{ProfileError, ProfileResult};
use crate::types::{ActivitySourceRef, Decision, LedgerItem};

const ACTION_MATCH_THRESHOLD: f64 = 78.0;
const DAY_MS: i64 = 86_400_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LedgerState {
    Open,
    Closed,
    // The profile response can contain dropped items even though no current profile route selects
    // them directly; retain the frozen ledger read vocabulary for internal callers.
    #[allow(dead_code)]
    Dropped,
    All,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LedgerSort {
    AgeDays,
    // Retained for the complete frozen ledger read contract; profile composition uses defaults.
    #[allow(dead_code)]
    OpenedAt,
    ClosedAt,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LedgerListQuery {
    pub(crate) state: LedgerState,
    pub(crate) owner: Option<String>,
    pub(crate) counterparty: Option<String>,
    pub(crate) age_days_gte: Option<i64>,
    pub(crate) closed_since: Option<String>,
    pub(crate) top: Option<usize>,
    pub(crate) sort: Option<LedgerSort>,
    pub(crate) facets: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DecisionQuery {
    pub(crate) owner: Option<String>,
    pub(crate) involving: Option<String>,
    pub(crate) since: Option<String>,
    pub(crate) top: Option<usize>,
    pub(crate) facets: Option<Vec<String>>,
}

#[derive(Debug, Clone)]
struct Commitment {
    item: LedgerItem,
    counterparty_normalized: String,
    action_normalized: String,
    opening_key: SortKey,
}

#[derive(Debug, Clone)]
struct StoryClosure {
    owner_entity_id: Option<String>,
    counterparty_entity_id: Option<String>,
    counterparty_normalized: String,
    action_normalized: String,
    closed_at: i64,
    sort_key: SortKey,
    source: ActivitySourceRef,
}

#[derive(Debug, Clone)]
struct ManualClose {
    state: String,
    closed_at: i64,
    manual_order_key: (i64, String, String, String, usize),
    source: ActivitySourceRef,
}

type SortKey = (i64, String, String, String);
type ScannedRecord = (String, String, Map<String, Value>);

pub(crate) fn list(
    journal_root: &Path,
    now: DateTime<Utc>,
    query: LedgerListQuery,
) -> ProfileResult<Vec<LedgerItem>> {
    let resolved_sort = resolve_sort(query.state, query.sort);
    let facets = match query.facets {
        Some(facets) => facets,
        None => enabled_facet_names(journal_root)?,
    };
    let records = scan_records(journal_root, &facets)?;
    let mut items = build_ledger_items(records, now);

    if query.state != LedgerState::All {
        let state = state_name(query.state);
        items.retain(|item| item.state == state);
    }
    if let Some(owner) = query.owner.as_deref() {
        items.retain(|item| {
            party_matches(owner, Some(&item.owner), item.owner_entity_id.as_deref())
        });
    }
    if let Some(counterparty) = query.counterparty.as_deref() {
        items.retain(|item| {
            party_matches(
                counterparty,
                item.counterparty.as_deref(),
                item.counterparty_entity_id.as_deref(),
            )
        });
    }
    if let Some(age_days) = query.age_days_gte {
        items.retain(|item| item.age_days >= age_days);
    }
    if let Some(closed_since) = query.closed_since.as_deref() {
        let threshold = parse_day_ms(closed_since, "closed_since")?;
        items.retain(|item| {
            item.closed_at
                .is_some_and(|closed_at| closed_at >= threshold)
        });
    }

    sort_items(&mut items, resolved_sort);
    if let Some(top) = query.top {
        items.truncate(top);
    }
    Ok(items)
}

pub(crate) fn decisions(journal_root: &Path, query: DecisionQuery) -> ProfileResult<Vec<Decision>> {
    if let Some(since) = query.since.as_deref() {
        parse_day_ms(since, "since")?;
    }
    let facets = match query.facets {
        Some(facets) => facets,
        None => enabled_facet_names(journal_root)?,
    };
    let mut deduped = BTreeMap::<String, Decision>::new();
    for (facet, day, record) in scan_records(journal_root, &facets)? {
        let record_id = record_string(&record, "id");
        if record_id.is_empty() {
            continue;
        }
        let created_at = record_created_at(&record);
        let source = source_ref(&facet, &day, &record_id, "decisions", created_at);
        for raw_decision in values(record.get("decisions")) {
            let Some(raw_decision) = raw_decision.as_object() else {
                continue;
            };
            let owner = record_string(raw_decision, "owner").trim().to_owned();
            let action = record_string(raw_decision, "action").trim().to_owned();
            if owner.is_empty() || action.is_empty() {
                continue;
            }
            let owner_entity_id = optional_string(raw_decision.get("owner_entity_id"));
            let id = decision_key(owner_entity_id.as_deref(), &normalize_action(&action), &day);
            let candidate = Decision {
                id: id.clone(),
                owner,
                owner_entity_id,
                action,
                context: record_string(raw_decision, "context"),
                day: day.clone(),
                created_at,
                source: source.clone(),
            };
            let candidate_key = chronological_key(
                candidate.created_at,
                &candidate.source.facet,
                &candidate.day,
                &candidate.source.activity_id,
            );
            let replace = deduped.get(&id).is_none_or(|current| {
                candidate_key
                    < chronological_key(
                        current.created_at,
                        &current.source.facet,
                        &current.day,
                        &current.source.activity_id,
                    )
            });
            if replace {
                deduped.insert(id, candidate);
            }
        }
    }

    let mut results = deduped.into_values().collect::<Vec<_>>();
    if let Some(owner) = query.owner.as_deref() {
        results.retain(|decision| {
            party_matches(
                owner,
                Some(&decision.owner),
                decision.owner_entity_id.as_deref(),
            )
        });
    }
    if let Some(involving) = query.involving.as_deref() {
        results.retain(|decision| {
            party_matches(
                involving,
                Some(&decision.owner),
                decision.owner_entity_id.as_deref(),
            )
        });
    }
    if let Some(since) = query.since.as_deref() {
        results.retain(|decision| decision.day.as_str() >= since);
    }
    results.sort_by(|left, right| {
        (
            right.created_at,
            &right.source.facet,
            &right.day,
            &right.source.activity_id,
        )
            .cmp(&(
                left.created_at,
                &left.source.facet,
                &left.day,
                &left.source.activity_id,
            ))
    });
    if let Some(top) = query.top {
        results.truncate(top);
    }
    Ok(results)
}

fn enabled_facet_names(journal_root: &Path) -> ProfileResult<Vec<String>> {
    let facets = list_declared_facet_names(journal_root).map_err(ProfileError::internal)?;
    let mut enabled = Vec::new();
    for facet in facets {
        let declaration =
            read_facet_declaration(journal_root, &facet).map_err(ProfileError::internal)?;
        if declaration.is_some_and(|declaration| declaration.muted != Some(true)) {
            enabled.push(facet);
        }
    }
    Ok(enabled)
}

fn scan_records(journal_root: &Path, facets: &[String]) -> ProfileResult<Vec<ScannedRecord>> {
    let mut records = Vec::new();
    for facet in facets {
        for day in activity_days(journal_root, facet)? {
            let day_records = load_activity_records(journal_root, facet, &day, false)
                .map_err(ProfileError::internal)?;
            records.extend(
                day_records
                    .into_iter()
                    .map(|record| (facet.clone(), day.clone(), record)),
            );
        }
    }
    Ok(records)
}

fn activity_days(journal_root: &Path, facet: &str) -> ProfileResult<Vec<String>> {
    let directory = journal_root.join("facets").join(facet).join("activities");
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(ProfileError::internal(error)),
    };
    let mut days = entries
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(ProfileError::internal)?
        .into_iter()
        .filter(|path| path.is_file())
        .filter_map(|path| {
            let stem = path.file_stem()?.to_str()?;
            (path.extension().and_then(|value| value.to_str()) == Some("jsonl")
                && is_eight_digit_day(stem))
            .then(|| stem.to_owned())
        })
        .collect::<Vec<_>>();
    days.sort();
    Ok(days)
}

fn is_eight_digit_day(value: &str) -> bool {
    value.len() == 8 && value.as_bytes().iter().all(u8::is_ascii_digit)
}

fn build_ledger_items(records: Vec<ScannedRecord>, now: DateTime<Utc>) -> Vec<LedgerItem> {
    let mut commitments = BTreeMap::<String, Commitment>::new();
    let mut story_closures = Vec::<StoryClosure>::new();
    let mut manual_closes = BTreeMap::<String, Vec<ManualClose>>::new();

    for (facet, day, record) in records {
        let record_id = record_string(&record, "id");
        if record_id.is_empty() {
            continue;
        }
        let created_at = record_created_at(&record);
        for raw_commitment in values(record.get("commitments")) {
            let Some(raw_commitment) = raw_commitment.as_object() else {
                continue;
            };
            let owner = record_string(raw_commitment, "owner").trim().to_owned();
            let action = record_string(raw_commitment, "action").trim().to_owned();
            if owner.is_empty() || action.is_empty() {
                continue;
            }
            let owner_entity_id = optional_string(raw_commitment.get("owner_entity_id"));
            let counterparty = non_empty(record_string(raw_commitment, "counterparty").trim());
            let counterparty_entity_id =
                optional_string(raw_commitment.get("counterparty_entity_id"));
            let action_normalized = normalize_action(&action);
            let id = dedup_key(
                owner_entity_id.as_deref(),
                &action_normalized,
                counterparty_entity_id.as_deref(),
            );
            let opening_key = chronological_key(created_at, &facet, &day, &record_id);
            let source = source_ref(&facet, &day, &record_id, "commitments", created_at);
            let item = LedgerItem {
                id: id.clone(),
                state: "open".to_owned(),
                owner,
                owner_entity_id,
                counterparty: counterparty.clone(),
                counterparty_entity_id,
                action: action.clone(),
                summary: action,
                when: non_empty(record_string(raw_commitment, "when").trim()),
                context: record_string(raw_commitment, "context"),
                opened_at: created_at,
                closed_at: None,
                age_days: 0,
                sources: vec![source.clone()],
            };
            let candidate = Commitment {
                counterparty_normalized: normalize_text(counterparty.as_deref()),
                action_normalized,
                opening_key: opening_key.clone(),
                item,
            };
            match commitments.get_mut(&id) {
                None => {
                    commitments.insert(id, candidate);
                }
                Some(existing) => {
                    existing.item.sources.push(source);
                    if opening_key < existing.opening_key {
                        let sources = std::mem::take(&mut existing.item.sources);
                        existing.item = candidate.item;
                        existing.item.sources = sources;
                        existing.counterparty_normalized = candidate.counterparty_normalized;
                        existing.action_normalized = candidate.action_normalized;
                        existing.opening_key = candidate.opening_key;
                    }
                }
            }
        }

        for raw_closure in values(record.get("closures")) {
            let Some(raw_closure) = raw_closure.as_object() else {
                continue;
            };
            let action = record_string(raw_closure, "action").trim().to_owned();
            if action.is_empty() {
                continue;
            }
            story_closures.push(StoryClosure {
                owner_entity_id: optional_string(raw_closure.get("owner_entity_id")),
                counterparty_entity_id: optional_string(raw_closure.get("counterparty_entity_id")),
                counterparty_normalized: normalize_text(
                    non_empty(record_string(raw_closure, "counterparty").trim()).as_deref(),
                ),
                action_normalized: normalize_action(&action),
                closed_at: created_at,
                sort_key: chronological_key(created_at, &facet, &day, &record_id),
                source: source_ref(&facet, &day, &record_id, "closures", created_at),
            });
        }

        for (edit_index, raw_edit) in values(record.get("edits")).iter().enumerate() {
            let Some(raw_edit) = raw_edit.as_object() else {
                continue;
            };
            if raw_edit.get("fields")
                != Some(&Value::Array(vec![Value::String(
                    "ledger_close".to_owned(),
                )]))
            {
                continue;
            }
            let Some(ledger_close) = raw_edit.get("ledger_close").and_then(Value::as_object) else {
                continue;
            };
            let Some(item_id) = ledger_close
                .get("item_id")
                .and_then(Value::as_str)
                .filter(|item_id| !item_id.is_empty())
            else {
                continue;
            };
            let Some(state) = ledger_close
                .get("as_state")
                .and_then(Value::as_str)
                .filter(|state| matches!(*state, "closed" | "dropped"))
            else {
                continue;
            };
            let closed_at = edit_timestamp_ms(raw_edit.get("timestamp")).unwrap_or(created_at);
            manual_closes
                .entry(item_id.to_owned())
                .or_default()
                .push(ManualClose {
                    state: state.to_owned(),
                    closed_at,
                    manual_order_key: (
                        closed_at,
                        facet.clone(),
                        day.clone(),
                        record_id.clone(),
                        edit_index,
                    ),
                    source: source_ref(&facet, &day, &record_id, "edits", created_at),
                });
        }
    }

    story_closures.sort_by_key(|closure| closure.sort_key.clone());
    let mut consumed_closures = BTreeSet::new();
    let now_ms = now.timestamp_millis();
    let mut items = commitments.into_values().collect::<Vec<_>>();
    items.sort_by_key(|commitment| commitment.opening_key.clone());
    items
        .into_iter()
        .map(|commitment| {
            let matched = story_closures
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
            let manual = manual_closes
                .get(&commitment.item.id)
                .map(Vec::as_slice)
                .unwrap_or_default();
            let (state, closed_at) = resolve_state(
                &matched
                    .iter()
                    .map(|(_, closure)| *closure)
                    .collect::<Vec<_>>(),
                manual,
            );
            let mut item = commitment.item;
            item.state = state;
            item.closed_at = closed_at;
            item.age_days = (now_ms - item.opened_at).div_euclid(DAY_MS);
            for (_, closure) in matched {
                item.sources.push(closure.source.clone());
            }
            for close in manual {
                item.sources.push(close.source.clone());
            }
            item.sources
                .sort_by(|left, right| source_sort_key(left).cmp(&source_sort_key(right)));
            item
        })
        .collect()
}

fn resolve_state(story: &[&StoryClosure], manual: &[ManualClose]) -> (String, Option<i64>) {
    if let Some(latest) = manual
        .iter()
        .max_by_key(|close| close.manual_order_key.clone())
    {
        return (latest.state.clone(), Some(latest.closed_at));
    }
    if let Some(earliest) = story.iter().min_by_key(|close| close.sort_key.clone()) {
        return ("closed".to_owned(), Some(earliest.closed_at));
    }
    ("open".to_owned(), None)
}

fn story_closure_matches(commitment: &Commitment, closure: &StoryClosure) -> bool {
    entity_pair_matches(
        commitment.item.owner_entity_id.as_deref(),
        closure.owner_entity_id.as_deref(),
        true,
    ) && counterparty_matches(commitment, closure)
        && actions_match(&commitment.action_normalized, &closure.action_normalized)
}

fn entity_pair_matches(left: Option<&str>, right: Option<&str>, allow_both_missing: bool) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => left == right,
        (None, None) => allow_both_missing,
        _ => false,
    }
}

fn counterparty_matches(commitment: &Commitment, closure: &StoryClosure) -> bool {
    match (
        commitment.item.counterparty_entity_id.as_deref(),
        closure.counterparty_entity_id.as_deref(),
    ) {
        (Some(left), Some(right)) => left == right,
        (None, None) => commitment.counterparty_normalized == closure.counterparty_normalized,
        _ => false,
    }
}

fn actions_match(commitment_action: &str, closure_action: &str) -> bool {
    !commitment_action.is_empty()
        && !closure_action.is_empty()
        && find_matching_entity(
            commitment_action,
            &[EntityNameCandidate {
                id: None,
                name: closure_action.to_owned(),
                aka: Vec::new(),
                emails: Vec::new(),
            }],
            ACTION_MATCH_THRESHOLD,
        )
        .is_some()
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

fn decision_key(owner: Option<&str>, action: &str, day: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(owner.unwrap_or_default());
    digest.update("|");
    digest.update(action);
    digest.update("|");
    digest.update(day);
    format!("{:x}", digest.finalize())[..16].to_owned()
}

fn normalize_action(value: &str) -> String {
    value
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn normalize_text(value: Option<&str>) -> String {
    value
        .unwrap_or_default()
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn chronological_key(created_at: i64, facet: &str, day: &str, activity_id: &str) -> SortKey {
    (
        created_at,
        facet.to_owned(),
        day.to_owned(),
        activity_id.to_owned(),
    )
}

fn source_ref(
    facet: &str,
    day: &str,
    activity_id: &str,
    field: &str,
    created_at: i64,
) -> ActivitySourceRef {
    ActivitySourceRef {
        facet: facet.to_owned(),
        day: day.to_owned(),
        activity_id: activity_id.to_owned(),
        field: field.to_owned(),
        created_at,
    }
}

fn source_sort_key(source: &ActivitySourceRef) -> (i64, &str, &str, &str, usize) {
    (
        source.created_at,
        &source.facet,
        &source.day,
        &source.activity_id,
        field_order(&source.field),
    )
}

fn field_order(field: &str) -> usize {
    match field {
        "commitments" => 0,
        "closures" => 1,
        "decisions" => 2,
        "edits" => 3,
        _ => 99,
    }
}

fn state_name(state: LedgerState) -> &'static str {
    match state {
        LedgerState::Open => "open",
        LedgerState::Closed => "closed",
        LedgerState::Dropped => "dropped",
        LedgerState::All => "all",
    }
}

fn resolve_sort(state: LedgerState, sort: Option<LedgerSort>) -> LedgerSort {
    sort.unwrap_or(match state {
        LedgerState::Closed | LedgerState::Dropped => LedgerSort::ClosedAt,
        LedgerState::Open | LedgerState::All => LedgerSort::AgeDays,
    })
}

fn sort_items(items: &mut [LedgerItem], sort: LedgerSort) {
    match sort {
        LedgerSort::AgeDays => items.sort_by(|left, right| {
            (right.age_days, right.opened_at, &right.id).cmp(&(
                left.age_days,
                left.opened_at,
                &left.id,
            ))
        }),
        LedgerSort::OpenedAt => items
            .sort_by(|left, right| (right.opened_at, &right.id).cmp(&(left.opened_at, &left.id))),
        LedgerSort::ClosedAt => items.sort_by(|left, right| {
            (
                right.closed_at.is_some(),
                right.closed_at.unwrap_or(-1),
                &right.id,
            )
                .cmp(&(
                    left.closed_at.is_some(),
                    left.closed_at.unwrap_or(-1),
                    &left.id,
                ))
        }),
    }
}

fn party_matches(query: &str, name: Option<&str>, entity_id: Option<&str>) -> bool {
    let normalized_query = normalize_text(Some(query));
    if normalized_query.is_empty() {
        return true;
    }
    [
        name.unwrap_or_default(),
        entity_id.unwrap_or_default(),
        &entity_id.unwrap_or_default().replace('_', " "),
    ]
    .into_iter()
    .any(|candidate| normalize_text(Some(candidate)).contains(&normalized_query))
}

fn parse_day_ms(day: &str, field_name: &str) -> ProfileResult<i64> {
    NaiveDate::parse_from_str(day, "%Y%m%d")
        .map_err(|_| ProfileError::internal(format!("{field_name} must match YYYYMMDD")))
        .and_then(|day| {
            day.and_hms_opt(0, 0, 0)
                .map(|day| day.and_utc().timestamp_millis())
                .ok_or_else(|| ProfileError::internal(format!("{field_name} must match YYYYMMDD")))
        })
}

fn record_string(record: &Map<String, Value>, key: &str) -> String {
    activity_value_or_empty(record.get(key))
}

fn record_created_at(record: &Map<String, Value>) -> i64 {
    let Some(value) = record.get("created_at") else {
        return 0;
    };
    if value.is_null() || value == &Value::Bool(false) {
        return 0;
    }
    value
        .as_i64()
        .or_else(|| value.as_u64().and_then(|value| i64::try_from(value).ok()))
        .or_else(|| value.as_f64().map(|value| value as i64))
        .or_else(|| {
            value
                .as_str()
                .and_then(|value| value.trim().parse::<i64>().ok())
        })
        .unwrap_or(0)
}

fn optional_string(value: Option<&Value>) -> Option<String> {
    value.and_then(Value::as_str).map(str::to_owned)
}

fn non_empty(value: &str) -> Option<String> {
    (!value.is_empty()).then(|| value.to_owned())
}

fn values(value: Option<&Value>) -> &[Value] {
    value
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
}

fn edit_timestamp_ms(value: Option<&Value>) -> Option<i64> {
    value
        .and_then(Value::as_str)
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
        .map(|value| value.timestamp_millis())
}

#[cfg(test)]
mod tests {
    use chrono::{Duration, TimeZone, Utc};
    use serde_json::json;

    use super::{DecisionQuery, LedgerListQuery, LedgerState, decisions, list};
    use crate::test_support::{journal, write_json, write_jsonl};

    fn now() -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 4, 10, 12, 0, 0).unwrap()
    }

    fn facet(root: &std::path::Path, name: &str, muted: bool) {
        write_json(
            root,
            &format!("facets/{name}/facet.json"),
            json!({"name":name,"muted":muted}),
        );
    }

    fn rows(root: &std::path::Path, facet: &str, day: &str, rows: &[serde_json::Value]) {
        write_jsonl(
            root,
            &format!("facets/{facet}/activities/{day}.jsonl"),
            rows,
        );
    }

    fn list_query(state: LedgerState) -> LedgerListQuery {
        LedgerListQuery {
            state,
            owner: None,
            counterparty: None,
            age_days_gte: None,
            closed_since: None,
            top: None,
            sort: None,
            facets: None,
        }
    }

    fn decision_query() -> DecisionQuery {
        DecisionQuery {
            owner: None,
            involving: None,
            since: None,
            top: None,
            facets: None,
        }
    }

    #[test]
    fn commitment_dedup_key_uses_normalized_action_and_earliest_payload() {
        let temporary = journal();
        facet(temporary.path(), "work", false);
        rows(
            temporary.path(),
            "work",
            "20260401",
            &[
                json!({"id":"later","created_at":200,"commitments":[{"owner":"Owner Later","owner_entity_id":"owner","counterparty":"Pat","counterparty_entity_id":"pat","action":"send report","context":"late"}]}),
                json!({"id":"earlier","created_at":100,"commitments":[{"owner":"Owner Early","owner_entity_id":"owner","counterparty":"Pat","counterparty_entity_id":"pat","action":" Send   Report ","context":"early"}]}),
            ],
        );

        let items = list(temporary.path(), now(), list_query(LedgerState::Open)).expect("ledger");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, "03b382d6f35ed848");
        assert_eq!(items[0].owner, "Owner Early");
        assert_eq!(items[0].context, "early");
        assert_eq!(items[0].sources.len(), 2);
        assert_eq!(
            items[0]
                .sources
                .iter()
                .map(|source| source.activity_id.as_str())
                .collect::<Vec<_>>(),
            vec!["earlier", "later"]
        );
    }

    #[test]
    fn fuzzy_story_closure_and_latest_manual_override_determine_state() {
        let temporary = journal();
        facet(temporary.path(), "work", false);
        rows(
            temporary.path(),
            "work",
            "20260401",
            &[
                json!({"id":"commit","created_at":100,"commitments":[{"owner":"Owner","owner_entity_id":"owner","counterparty":"Pat","counterparty_entity_id":"pat","action":"send report"}]}),
                json!({"id":"story","created_at":200,"closures":[{"owner_entity_id":"owner","counterparty_entity_id":"pat","action":"send the report"}]}),
                json!({"id":"manual","created_at":300,"edits":[
                    {"fields":["ledger_close"],"timestamp":"2026-04-02T00:00:00Z","ledger_close":{"item_id":"03b382d6f35ed848","as_state":"closed"}},
                    {"fields":["ledger_close"],"timestamp":"2026-04-03T00:00:00Z","ledger_close":{"item_id":"03b382d6f35ed848","as_state":"dropped"}}
                ]}),
            ],
        );

        let all = list(temporary.path(), now(), list_query(LedgerState::All)).expect("ledger");
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].state, "dropped");
        assert_eq!(
            all[0].closed_at,
            Some(
                Utc.with_ymd_and_hms(2026, 4, 3, 0, 0, 0)
                    .unwrap()
                    .timestamp_millis()
            )
        );
        assert_eq!(
            all[0]
                .sources
                .iter()
                .map(|source| source.field.as_str())
                .collect::<Vec<_>>(),
            vec!["commitments", "closures", "edits", "edits"]
        );
    }

    #[test]
    fn decisions_keep_earliest_duplicate_sort_descending_and_skip_muted_facets() {
        let temporary = journal();
        facet(temporary.path(), "work", false);
        facet(temporary.path(), "muted", true);
        rows(
            temporary.path(),
            "work",
            "20260401",
            &[
                json!({"id":"early","created_at":100,"decisions":[{"owner":"Owner","owner_entity_id":"owner","action":"choose rust","context":"earliest"}]}),
                json!({"id":"late-duplicate","created_at":200,"decisions":[{"owner":"Owner","owner_entity_id":"owner","action":"Choose   Rust","context":"later"}]}),
            ],
        );
        rows(
            temporary.path(),
            "work",
            "20260402",
            &[
                json!({"id":"new","created_at":300,"decisions":[{"owner":"Owner","owner_entity_id":"owner","action":"choose go","context":"newest"}]}),
            ],
        );
        rows(
            temporary.path(),
            "muted",
            "20260403",
            &[
                json!({"id":"muted","created_at":400,"decisions":[{"owner":"Owner","owner_entity_id":"owner","action":"choose hidden"}]}),
            ],
        );

        let mut query = decision_query();
        query.involving = Some("owner".to_owned());
        let found_decisions = decisions(temporary.path(), query).expect("decisions");
        assert_eq!(found_decisions.len(), 2);
        assert_eq!(found_decisions[0].action, "choose go");
        assert_eq!(found_decisions[1].id, "5e6e91e387f7195d");
        assert_eq!(found_decisions[1].context, "earliest");

        let mut since_query = decision_query();
        since_query.since = Some("20260402".to_owned());
        assert_eq!(
            decisions(temporary.path(), since_query)
                .expect("since")
                .len(),
            1
        );
    }

    #[test]
    fn closed_since_and_age_filters_apply_after_the_fold() {
        let temporary = journal();
        facet(temporary.path(), "work", false);
        let old = (now() - Duration::days(20)).timestamp_millis();
        rows(
            temporary.path(),
            "work",
            "20260321",
            &[
                json!({"id":"old","created_at":old,"commitments":[{"owner":"Owner","action":"old item"}]}),
            ],
        );
        let mut query = list_query(LedgerState::Open);
        query.age_days_gte = Some(14);
        assert_eq!(list(temporary.path(), now(), query).expect("age").len(), 1);
    }
}
