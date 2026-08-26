// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Read-only evidence imported from the Python-era observer registry.

use std::collections::{BTreeSet, HashMap};
use std::fs::File;
use std::path::Path;

use serde_json::Value;
use solstone_core_observer::store::{
    HistoryStop, ObserverRecord, history_days, history_path, load_history,
    load_observers_with_inventory,
};

#[derive(Clone, Debug)]
pub(crate) struct ResolvedObserver {
    pub(crate) record: ObserverRecord,
    pub(crate) prefix: String,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct HistoryEvidence {
    pub(crate) records: Vec<Value>,
    pub(crate) observed_segments: BTreeSet<String>,
    pub(crate) pruned_segments: BTreeSet<String>,
}

#[derive(Debug)]
pub(crate) enum ObserverEvidenceError {
    RegistryUnreadable,
    RecordUnreadable,
    Ambiguous { prefixes: Vec<String> },
    HistoryUnreadable,
    HistoryTorn,
    Malformed,
    JournalRead,
}

/// Select the one unrevoked observer cryptographically bound to `cid`.
///
/// An observer without `device_binding` is deliberately omitted rather than
/// mis-attributed. Its material is unreachable natively either way and
/// self-heals when that device next `/register`s.
pub(crate) fn resolve_device_observer(
    journal_root: &Path,
    cid: &str,
) -> Result<Option<ResolvedObserver>, ObserverEvidenceError> {
    let loaded = load_observers_with_inventory(journal_root)
        .map_err(|_| ObserverEvidenceError::RegistryUnreadable)?;
    if loaded.regular_json_entries != loaded.records.len() {
        return Err(ObserverEvidenceError::RecordUnreadable);
    }
    let mut matching: Vec<_> = loaded
        .records
        .into_iter()
        .filter(|record| !record.revoked() && record.device_binding_device() == Some(cid))
        .collect();
    if matching.len() > 1 {
        let mut prefixes = matching
            .iter()
            .map(ObserverRecord::prefix)
            .collect::<Vec<_>>();
        prefixes.sort();
        return Err(ObserverEvidenceError::Ambiguous { prefixes });
    }
    let Some(record) = matching.pop() else {
        return Ok(None);
    };
    // A present non-string stream refuses. This deliberately diverges from
    // store::prune::attribution::observer_prefix_for_stream's
    // `.and_then(Value::as_str)` fall-through: evidence readers must not guess.
    if record.value().contains_key("stream")
        && !record.value().get("stream").is_some_and(Value::is_string)
    {
        return Err(ObserverEvidenceError::Malformed);
    }
    Ok(Some(ResolvedObserver {
        prefix: record.prefix(),
        record,
    }))
}

pub(crate) fn observer_history_days(
    journal_root: &Path,
    observer: Option<&ResolvedObserver>,
) -> Result<BTreeSet<String>, ObserverEvidenceError> {
    let Some(observer) = observer else {
        return Ok(BTreeSet::new());
    };
    history_days(journal_root, &observer.prefix)
        .map(|days| days.into_iter().collect())
        .map_err(|_| ObserverEvidenceError::JournalRead)
}

pub(crate) fn read_history_day(
    journal_root: &Path,
    observer: Option<&ResolvedObserver>,
    day: &str,
) -> Result<HistoryEvidence, ObserverEvidenceError> {
    let Some(observer) = observer else {
        return Ok(HistoryEvidence::default());
    };
    let path = history_path(journal_root, &observer.prefix, day);
    match File::open(&path) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(HistoryEvidence::default());
        }
        Err(_) => return Err(ObserverEvidenceError::HistoryUnreadable),
    }
    let loaded = load_history(&path);
    if loaded.stopped.is_some() {
        return Err(match loaded.stopped {
            Some(HistoryStop::Malformed { .. } | HistoryStop::Io) => {
                ObserverEvidenceError::HistoryTorn
            }
            None => unreachable!("checked stopped above"),
        });
    }
    let mut latest: HashMap<String, Option<String>> = HashMap::new();
    let mut observed_segments = BTreeSet::new();
    for record in &loaded.records {
        let Some(object) = record.as_object() else {
            return Err(ObserverEvidenceError::Malformed);
        };
        if let Some(segment) = object.get("segment").and_then(Value::as_str)
            && !segment.is_empty()
        {
            latest.insert(
                segment.to_owned(),
                object
                    .get("type")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            );
            if object.get("type").and_then(Value::as_str) == Some("observed") {
                observed_segments.insert(segment.to_owned());
            }
        }
    }
    Ok(HistoryEvidence {
        records: loaded.records,
        observed_segments,
        pruned_segments: latest
            .into_iter()
            .filter_map(|(segment, kind)| (kind.as_deref() == Some("pruned")).then_some(segment))
            .collect(),
    })
}

pub(crate) fn fallback_stream(
    observer: &ResolvedObserver,
) -> Result<String, ObserverEvidenceError> {
    if let Some(stream) = observer.record.value().get("stream") {
        let stream = stream.as_str().ok_or(ObserverEvidenceError::Malformed)?;
        if !stream.is_empty() {
            return Ok(stream.to_owned());
        }
    }
    let name = observer
        .record
        .name()
        .ok_or(ObserverEvidenceError::Malformed)?;
    python_observer_stream_name(name).ok_or(ObserverEvidenceError::Malformed)
}

/// Python's `stream_name(observer=...)`, deliberately not `project_stream_name`:
/// that is the native WRITE-path projection while these rows were written by
/// Python's historical reader/writer contract. The middle `?stream=` fallback
/// is skipped deliberately: a query string must never select which directory a
/// device's material is read from.
fn python_observer_stream_name(observer: &str) -> Option<String> {
    let base = strip_hostname(observer);
    let normalized = base
        .trim()
        .to_ascii_lowercase()
        .chars()
        .fold((String::new(), false), |(mut output, slash), character| {
            if character.is_whitespace() || matches!(character, '/' | '\\') {
                (output, true)
            } else {
                if slash && !output.is_empty() {
                    output.push('-');
                }
                output.push(character);
                (output, false)
            }
        })
        .0;
    let valid = normalized.chars().enumerate().all(|(index, character)| {
        character.is_ascii_lowercase()
            || character.is_ascii_digit()
            || matches!(character, '.' | '_' | '-')
                && (index != 0 || character.is_ascii_lowercase() || character.is_ascii_digit())
    });
    (!normalized.is_empty() && !normalized.contains("..") && valid).then_some(normalized)
}

fn strip_hostname(name: &str) -> String {
    let name = name.trim();
    let parts = name.split('.').collect::<Vec<_>>();
    if parts
        .iter()
        .filter(|part| !part.is_empty())
        .all(|part| part.chars().all(char::is_numeric))
    {
        return parts
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join("-");
    }
    parts.first().copied().unwrap_or_default().to_owned()
}
