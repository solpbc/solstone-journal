// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Facet names are permanent: a name that belonged to a deleted or merged-away
//! facet is recorded in `facets/retired.json` and never given to another
//! facet. This module holds the journal-side operations on that record: merge
//! admission, keeping stored search classifications in step with it, and the
//! owner-run repairs in `journal facet doctor`.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::Path;
use std::time::Duration;

use chrono::Utc;
use serde_json::Value;
use solstone_core_facets::{
    RetiredFacet, RetiredFacetState, RetiredFacets, append_action_log, hold_facet_trust_lock,
    is_well_formed_facet_id, observe_declared_facet_inventory, read_facet_declaration,
    read_retired_facets, record_retired_facet, retired_facets_path,
};
use solstone_core_indexer_store::classification::FacetDeclarationSet;
use solstone_core_indexer_store::reconcile::{ReconcileReport, reconcile_stale_classifications};
use solstone_core_journal_io::{LockOptions, hold_lock};

const RECONCILE_LOCK: &str = "health/locks/facet-reconcile";

/// What a merge may proceed with, read under the facet trust lock.
pub(crate) struct MergeAdmission {
    pub(crate) source_id: Option<String>,
    pub(crate) destination_id: String,
    source_title: Option<String>,
}

impl MergeAdmission {
    pub(crate) fn retired_entry(&self) -> RetiredFacet {
        RetiredFacet::merged(
            self.source_id.clone(),
            self.destination_id.clone(),
            self.source_title.clone(),
        )
    }
}

/// A declared facet's id (when well formed) and title.
type Declared = (Option<String>, Option<String>);

fn declared_id(journal: &Path, name: &str) -> Result<Option<Declared>, String> {
    let Some(snapshot) = read_facet_declaration(journal, name).map_err(|e| e.to_string())? else {
        return Ok(None);
    };
    let value = snapshot.value();
    let id = value
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| is_well_formed_facet_id(id))
        .map(str::to_owned);
    let title = value
        .get("title")
        .and_then(Value::as_str)
        .map(str::to_owned);
    Ok(Some((id, title)))
}

/// Check a merge before anything moves. The caller holds the facet trust lock.
///
/// The owner's merge joins two live facets; DEST needs a stable id, because
/// SOURCE's name will resolve to it. The doctor's merge folds an undeclared
/// directory into the facet adopted for its name-variant group.
pub(crate) fn admit_facet_merge(
    journal: &Path,
    source: &str,
    destination: &str,
    doctor_orphan: bool,
) -> Result<MergeAdmission, String> {
    let retired = read_retired_facets(journal)
        .entries_for_write()
        .map_err(|error| error.to_string())?;
    let Some((destination_id, _)) = declared_id(journal, destination)? else {
        return Err(format!("'{destination}' isn't a facet"));
    };
    let Some(destination_id) = destination_id else {
        return Err(format!(
            "'{destination}' has no stable id yet; run 'journal backfill-facet-ids' first"
        ));
    };
    if retired
        .get(destination)
        .is_some_and(|entry| entry.id.as_deref() != Some(destination_id.as_str()))
    {
        return Err(format!(
            "'{destination}' is recorded as a retired facet name; run 'journal facet doctor' for details"
        ));
    }
    let source_declaration = declared_id(journal, source)?;
    if doctor_orphan {
        if source_declaration.is_some() {
            return Err(format!(
                "'{source}' is a declared facet, not an orphan folder"
            ));
        }
        if retired.contains_key(source) {
            return Err(format!("'{source}' is a retired facet name"));
        }
        return Ok(MergeAdmission {
            source_id: None,
            destination_id,
            source_title: None,
        });
    }
    let Some((source_id, source_title)) = source_declaration else {
        return Err(format!("'{source}' isn't a facet"));
    };
    let inventory = observe_declared_facet_inventory(journal).map_err(|e| e.to_string())?;
    if inventory.enabled.len() == 1 && inventory.enabled[0] == source {
        return Err(format!(
            "'{source}' is the only facet that isn't muted, and a journal keeps at least one; unmute '{destination}' first"
        ));
    }
    Ok(MergeAdmission {
        source_id,
        destination_id,
        source_title,
    })
}

/// Bring stored search classifications in step with facet names.
///
/// Single-flight through its own lock, taken before the facet trust lock,
/// which is held only while the declarations are read. Callers must not hold
/// the facet trust lock.
pub(crate) fn reconcile_facet_classifications(journal: &Path) -> Result<ReconcileReport, String> {
    let _single = hold_lock(
        journal.join(RECONCILE_LOCK),
        LockOptions {
            timeout: Duration::from_secs(120),
            mode: Some(0o600),
            ..LockOptions::default()
        },
    )
    .map_err(|error| error.to_string())?;
    let mut snapshot = || {
        let _trust = hold_facet_trust_lock(journal).map_err(|error| {
            solstone_core_indexer_store::StoreError::Io(io::Error::other(error.to_string()))
        })?;
        FacetDeclarationSet::from_journal(journal)
    };
    reconcile_stale_classifications(journal, &mut snapshot).map_err(|error| error.to_string())
}

// ---- journal facet doctor ------------------------------------------------

/// A name the action logs show was renamed, merged or deleted, with what it
/// should be recorded as.
#[derive(Debug)]
pub(crate) struct HistoryProposal {
    pub(crate) name: String,
    pub(crate) entry: RetiredFacet,
    pub(crate) evidence: String,
}

#[derive(Debug, Default)]
pub(crate) struct HistoryScan {
    pub(crate) proposals: Vec<HistoryProposal>,
    /// Names the logs mention whose outcome can't be recovered.
    pub(crate) unrecoverable: Vec<String>,
    /// Names the logs show were retired but that are live again: history
    /// reused them before names were permanent. Reported, never recorded,
    /// because recording would hide the live facet's material from its own
    /// agents.
    pub(crate) reused: Vec<String>,
}

#[derive(Debug)]
enum LoggedEvent {
    Renamed {
        old: String,
        new: String,
    },
    Merged {
        source: String,
        dest: String,
        source_id: Option<String>,
    },
    Deleted {
        name: String,
        id: Option<String>,
    },
}

fn read_log_events(journal: &Path) -> Vec<(String, LoggedEvent)> {
    let mut files = Vec::new();
    let actions = journal.join("config/actions");
    if let Ok(entries) = fs::read_dir(&actions) {
        files.extend(entries.flatten().map(|entry| entry.path()));
    }
    if let Ok(facets) = fs::read_dir(journal.join("facets")) {
        for facet in facets.flatten() {
            if let Ok(entries) = fs::read_dir(facet.path().join("logs")) {
                files.extend(entries.flatten().map(|entry| entry.path()));
            }
        }
    }
    let text_field = |params: &Value, key: &str| {
        params
            .get(key)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    };
    let mut events = Vec::new();
    for path in files {
        if path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
            continue;
        }
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        for line in text.lines() {
            let Ok(record) = serde_json::from_str::<Value>(line) else {
                continue;
            };
            let timestamp = record
                .get("timestamp")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned();
            let params = record.get("params").cloned().unwrap_or(Value::Null);
            let event = match record.get("action").and_then(Value::as_str) {
                // A title change logs old_title/new_title and never moves a name.
                Some("facet_rename") => match (
                    text_field(&params, "old_name"),
                    text_field(&params, "new_name"),
                ) {
                    (Some(old), Some(new)) if old != new => Some(LoggedEvent::Renamed { old, new }),
                    _ => None,
                },
                Some("facet_merge") => {
                    match (text_field(&params, "source"), text_field(&params, "dest")) {
                        (Some(source), Some(dest)) if source != dest => Some(LoggedEvent::Merged {
                            source,
                            dest,
                            source_id: text_field(&params, "source_id")
                                .filter(|id| is_well_formed_facet_id(id)),
                        }),
                        _ => None,
                    }
                }
                Some("facet_delete") => {
                    text_field(&params, "name").map(|name| LoggedEvent::Deleted {
                        id: text_field(&params, "id").filter(|id| is_well_formed_facet_id(id)),
                        name,
                    })
                }
                _ => None,
            };
            if let Some(event) = event {
                events.push((timestamp, event));
            }
        }
    }
    events.sort_by(|left, right| left.0.cmp(&right.0));
    events
}

/// Live facet directory names mapped to their ids.
fn live_facets(journal: &Path) -> Result<BTreeMap<String, Option<String>>, String> {
    let mut live = BTreeMap::new();
    for name in
        solstone_core_facets::list_declared_facet_names(journal).map_err(|e| e.to_string())?
    {
        let id = declared_id(journal, &name)?.and_then(|(id, _)| id);
        live.insert(name, id);
    }
    Ok(live)
}

/// Propose retired entries from the action logs. Only names that are neither
/// live nor already recorded are proposed.
pub(crate) fn scan_history(
    journal: &Path,
    retired: &BTreeMap<String, RetiredFacet>,
) -> Result<HistoryScan, String> {
    let live = live_facets(journal)?;
    let events = read_log_events(journal);
    // Where each name went next, in log order; the last event for a name wins.
    let mut next: BTreeMap<String, Option<String>> = BTreeMap::new();
    let mut logged_ids: BTreeMap<String, String> = BTreeMap::new();
    let mut mentioned = BTreeSet::new();
    for (_, event) in &events {
        match event {
            LoggedEvent::Renamed { old, new } => {
                next.insert(old.clone(), Some(new.clone()));
                mentioned.insert(old.clone());
            }
            LoggedEvent::Merged {
                source,
                dest,
                source_id,
            } => {
                next.insert(source.clone(), Some(dest.clone()));
                if let Some(id) = source_id {
                    logged_ids.insert(source.clone(), id.clone());
                }
                mentioned.insert(source.clone());
            }
            LoggedEvent::Deleted { name, id } => {
                next.insert(name.clone(), None);
                if let Some(id) = id {
                    logged_ids.insert(name.clone(), id.clone());
                }
                mentioned.insert(name.clone());
            }
        }
    }
    let mut scan = HistoryScan::default();
    for name in mentioned {
        if retired.contains_key(&name) {
            continue;
        }
        if live.contains_key(&name) {
            // A logged delete or merge of a facet that is still here either
            // failed or was undone; a name retired and then reused is only
            // reported.
            if next.get(&name).is_some_and(Option::is_some) {
                scan.reused.push(name);
            }
            continue;
        }
        // Follow the name forward to where it lives now.
        let mut current = name.clone();
        let mut outcome: Option<Option<String>> = None;
        for _ in 0..64 {
            match next.get(&current) {
                Some(Some(to)) => {
                    if let Some(Some(id)) = live.get(to) {
                        outcome = Some(Some(id.clone()));
                        break;
                    }
                    current = to.clone();
                }
                Some(None) => {
                    outcome = Some(None);
                    break;
                }
                None => break,
            }
        }
        let id = logged_ids.get(&name).cloned();
        match outcome {
            Some(Some(successor)) => {
                let state = match next.get(&name) {
                    Some(Some(to)) if events.iter().any(|(_, event)| matches!(event, LoggedEvent::Renamed { old, new } if *old == name && new == to)) => {
                        RetiredFacetState::Renamed
                    }
                    _ => RetiredFacetState::Merged,
                };
                let entry = match state {
                    RetiredFacetState::Renamed => RetiredFacet::renamed(id, successor),
                    _ => RetiredFacet::merged(id, successor, None),
                };
                scan.proposals.push(HistoryProposal {
                    evidence: format!(
                        "{name} was {} into {}",
                        if state == RetiredFacetState::Renamed {
                            "renamed"
                        } else {
                            "merged"
                        },
                        current_live_name(&live, entry.successor.as_deref())
                    ),
                    name,
                    entry,
                });
            }
            Some(None) => scan.proposals.push(HistoryProposal {
                evidence: format!("{name} was deleted"),
                name,
                entry: RetiredFacet::deleted(id, None),
            }),
            None => scan.unrecoverable.push(name),
        }
    }
    Ok(scan)
}

fn current_live_name(live: &BTreeMap<String, Option<String>>, id: Option<&str>) -> String {
    live.iter()
        .find(|(_, live_id)| live_id.as_deref() == id)
        .map(|(name, _)| name.clone())
        .unwrap_or_default()
}

/// Names stored in segment assignments or per-segment `talents/<name>/`
/// folders that match no live facet and no retired entry, with a count and
/// the first and last day they appear.
pub(crate) fn scan_unresolved_references(
    journal: &Path,
    retired: &BTreeMap<String, RetiredFacet>,
) -> Result<BTreeMap<String, (usize, String, String)>, String> {
    let live = live_facets(journal)?;
    let known = |name: &str| live.contains_key(name) || retired.contains_key(name);
    let mut found: BTreeMap<String, (usize, String, String)> = BTreeMap::new();
    let mut note = |name: &str, day: &str| {
        let entry = found
            .entry(name.to_owned())
            .or_insert_with(|| (0, day.to_owned(), day.to_owned()));
        entry.0 += 1;
        if day < entry.1.as_str() {
            entry.1 = day.to_owned();
        }
        if day > entry.2.as_str() {
            entry.2 = day.to_owned();
        }
    };
    let Ok(days) = fs::read_dir(journal.join("chronicle")) else {
        return Ok(found);
    };
    for day in days.flatten() {
        let day_name = day.file_name().to_string_lossy().into_owned();
        if day_name.len() != 8 || !day_name.bytes().all(|byte| byte.is_ascii_digit()) {
            continue;
        }
        let Ok(segments) = solstone_core_journal_io::iter_segments(
            journal,
            solstone_core_journal_io::PathOrDay::Day(&day_name),
        ) else {
            continue;
        };
        for segment in segments {
            let talents = segment.path().join("talents");
            if let Ok(text) = fs::read_to_string(talents.join("facets.json"))
                && let Ok(Value::Array(rows)) = serde_json::from_str::<Value>(&text)
            {
                for row in rows {
                    if let Some(name) = row.get("facet").and_then(Value::as_str)
                        && !known(name)
                    {
                        note(name, &day_name);
                    }
                }
            }
            if let Ok(children) = fs::read_dir(&talents) {
                for child in children.flatten() {
                    if child.file_type().is_ok_and(|kind| kind.is_dir()) {
                        let name = child.file_name().to_string_lossy().into_owned();
                        if !known(&name) {
                            note(&name, &day_name);
                        }
                    }
                }
            }
        }
    }
    Ok(found)
}

/// Dot-named folders under `facets/`: merge leftovers, and anything else that
/// declares a facet it can never be.
pub(crate) fn scan_dot_directories(journal: &Path) -> (Vec<String>, Vec<String>) {
    let mut leftovers = Vec::new();
    let mut hidden = Vec::new();
    if let Ok(entries) = fs::read_dir(journal.join("facets")) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.starts_with('.') || !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                continue;
            }
            if name.starts_with(".facet-merge-") {
                leftovers.push(name);
            } else if entry.path().join("facet.json").exists() {
                hidden.push(name);
            }
        }
    }
    leftovers.sort();
    hidden.sort();
    (leftovers, hidden)
}

/// Repair a `facets/retired.json` that exists but does not parse. It is moved
/// aside, and the record is rebuilt from its last good copy, then the action
/// logs, then every name the damaged file still shows, so no name that was
/// reserved becomes free. A file that can't be read at all is left alone.
pub(crate) fn repair_malformed_retired_file(journal: &Path) -> Result<Option<String>, String> {
    let RetiredFacets::Malformed(detail) = read_retired_facets(journal) else {
        return Ok(None);
    };
    let _trust = hold_facet_trust_lock(journal).map_err(|e| e.to_string())?;
    let path = retired_facets_path(journal).map_err(|e| e.to_string())?;
    let damaged = fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let aside = path.with_file_name(format!(
        "retired.json.malformed-{}",
        Utc::now().format("%Y%m%dT%H%M%SZ")
    ));
    fs::rename(&path, &aside).map_err(|e| e.to_string())?;
    let mut prev = path.clone().into_os_string();
    prev.push(".prev");
    let mut recovered = match fs::read_to_string(&prev) {
        Ok(text) => match solstone_core_facets::parse_retired_facets(&text) {
            RetiredFacets::Loaded(entries) => entries,
            _ => BTreeMap::new(),
        },
        Err(_) => BTreeMap::new(),
    };
    let history = scan_history(journal, &recovered)?;
    for proposal in history.proposals {
        recovered.entry(proposal.name).or_insert(proposal.entry);
    }
    let live = live_facets(journal)?;
    let mut reserved = Vec::new();
    for name in names_in_damaged_file(&damaged) {
        if recovered.contains_key(&name) || live.contains_key(&name) {
            continue;
        }
        recovered.insert(name.clone(), RetiredFacet::deleted(None, None));
        reserved.push(name);
    }
    for (name, entry) in recovered {
        record_retired_facet(journal, &name, entry).map_err(|e| e.to_string())?;
    }
    let mut message = format!(
        "facets/retired.json did not parse ({detail}); it was moved to {} and rebuilt.",
        aside.display()
    );
    if !reserved.is_empty() {
        message.push_str(&format!(
            " These names could not be traced and stay reserved as deleted: {}.",
            reserved.join(", ")
        ));
    }
    Ok(Some(message))
}

/// Keys that sit directly under `"names"` in a damaged file, found by a
/// forgiving scan, kept only when they are valid facet names.
fn names_in_damaged_file(text: &str) -> Vec<String> {
    let Some(start) = text.find("\"names\"") else {
        return Vec::new();
    };
    let mut names = Vec::new();
    let mut depth = 0usize;
    let mut chars = text[start + 7..].char_indices().peekable();
    while let Some((_, character)) = chars.next() {
        match character {
            '{' => depth += 1,
            '}' => {
                if depth <= 1 {
                    break;
                }
                depth -= 1;
            }
            '"' if depth == 1 => {
                let mut key = String::new();
                for (_, next) in chars.by_ref() {
                    if next == '"' {
                        break;
                    }
                    key.push(next);
                }
                let followed_by_colon = chars
                    .clone()
                    .find(|(_, next)| !next.is_whitespace())
                    .is_some_and(|(_, next)| next == ':');
                if followed_by_colon && valid_facet_name(&key) {
                    names.push(key);
                }
            }
            _ => {}
        }
    }
    names
}

fn valid_facet_name(name: &str) -> bool {
    let mut characters = name.chars();
    matches!(characters.next(), Some(character) if character.is_ascii_alphanumeric())
        && characters.all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
        })
}

/// `journal facet doctor --retire NAME (--into FACET | --deleted)`: record a
/// historical name the logs can't explain. References to it then resolve to
/// FACET, or to no facet.
pub(crate) fn retire_name(
    journal: &Path,
    name: &str,
    into: Option<&str>,
) -> Result<String, String> {
    if !valid_facet_name(name) {
        return Err(format!("'{name}' isn't a facet name"));
    }
    let entry = {
        let _trust = hold_facet_trust_lock(journal).map_err(|e| e.to_string())?;
        if journal.join("facets").join(name).exists() {
            return Err(format!(
                "'{name}' is a folder in this journal; only a name that no longer exists can be retired"
            ));
        }
        let entry = match into {
            Some(target) => {
                let Some((Some(id), _)) = declared_id(journal, target)? else {
                    return Err(format!(
                        "'{target}' isn't a facet with a stable id; run 'journal backfill-facet-ids' first"
                    ));
                };
                RetiredFacet::merged(None, id, None)
            }
            None => RetiredFacet::deleted(None, None),
        };
        record_retired_facet(journal, name, entry.clone()).map_err(|e| e.to_string())?;
        let _ = append_action_log(
            journal,
            None,
            "cli",
            "user",
            "facet_retire",
            serde_json::json!({
                "name": name,
                "state": if into.is_some() { "merged" } else { "deleted" },
                "successor": entry.successor,
                "into": into,
            }),
        );
        entry
    };
    let reconcile = reconcile_facet_classifications(journal)?;
    Ok(match (into, entry.successor.is_some()) {
        (Some(target), true) => format!(
            "Recorded '{name}' as merged into '{target}'. Search updated ({} of {} stored entries changed).\n",
            reconcile.changed, reconcile.candidates
        ),
        _ => format!(
            "Recorded '{name}' as deleted. Search updated ({} of {} stored entries changed).\n",
            reconcile.changed, reconcile.candidates
        ),
    })
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    use solstone_core_facets::{RetiredFacetState, read_retired_facets, retired_facet_entry};

    use super::{admit_facet_merge, repair_malformed_retired_file, retire_name, scan_history};

    const PERSONAL: &str = "44444444-4444-4444-8444-444444444444";
    const WORK: &str = "55555555-5555-4555-8555-555555555555";
    const X: &str = "77777777-7777-4777-8777-777777777777";

    static NEXT: AtomicU64 = AtomicU64::new(0);

    struct Journal(PathBuf);
    impl Journal {
        fn new() -> Self {
            let path = PathBuf::from("/var/tmp").join(format!(
                "solstone-core-journal-cli-facet-names-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(path.join("config/actions")).expect("journal");
            Self(path)
        }
        fn path(&self) -> &Path {
            &self.0
        }
        fn declare(&self, name: &str, id: &str, muted: bool) {
            let dir = self.0.join("facets").join(name);
            fs::create_dir_all(&dir).expect("facet");
            let muted = if muted { r#","muted":true"# } else { "" };
            fs::write(
                dir.join("facet.json"),
                format!(r#"{{"id":"{id}","title":"{name}"{muted}}}"#),
            )
            .expect("declaration");
        }
        fn log(&self, lines: &[&str]) {
            fs::write(
                self.0.join("config/actions/20260101.jsonl"),
                lines.join("\n") + "\n",
            )
            .expect("log");
        }
    }
    impl Drop for Journal {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn history_recovers_renames_merges_and_deletes_of_names_that_no_longer_exist() {
        let journal = Journal::new();
        journal.declare("personal", PERSONAL, false);
        journal.declare("work-life", WORK, false);
        fs::create_dir_all(journal.path().join("facets/work-life/logs")).expect("logs");
        fs::write(
            journal.path().join("facets/work-life/logs/20260101.jsonl"),
            r#"{"timestamp":"2026-01-01T00:00:01Z","action":"facet_rename","params":{"old_name":"work","new_name":"work-life"}}
"#,
        )
        .expect("rename log");
        journal.log(&[
            r#"{"timestamp":"2026-01-01T00:00:02Z","action":"facet_merge","params":{"source":"side","dest":"work"}}"#,
            r#"{"timestamp":"2026-01-01T00:00:03Z","action":"facet_delete","params":{"name":"old"}}"#,
            // A title change, a delete that never happened and a merge that
            // rolled back all leave the live facet alone.
            r#"{"timestamp":"2026-01-01T00:00:04Z","action":"facet_rename","params":{"old_title":"Personal","new_title":"Home"}}"#,
            r#"{"timestamp":"2026-01-01T00:00:05Z","action":"facet_delete","params":{"name":"personal"}}"#,
        ]);
        let scan = scan_history(journal.path(), &Default::default()).expect("scan");
        let found = scan
            .proposals
            .iter()
            .map(|proposal| {
                (
                    proposal.name.as_str(),
                    proposal.entry.state,
                    proposal.entry.successor.clone(),
                )
            })
            .collect::<Vec<_>>();
        assert!(found.contains(&("work", RetiredFacetState::Renamed, Some(WORK.to_owned()))));
        assert!(found.contains(&("side", RetiredFacetState::Merged, Some(WORK.to_owned()))));
        assert!(found.contains(&("old", RetiredFacetState::Deleted, None)));
        assert_eq!(found.len(), 3, "{found:?}");
        assert!(scan.reused.is_empty());
    }

    #[test]
    fn a_name_retired_and_later_reused_is_reported_never_recorded() {
        let journal = Journal::new();
        journal.declare("personal", PERSONAL, false);
        journal.declare("x", X, false);
        journal.declare("work", WORK, false);
        fs::create_dir_all(journal.path().join("facets/x/logs")).expect("logs");
        fs::write(
            journal.path().join("facets/x/logs/20260101.jsonl"),
            r#"{"timestamp":"2026-01-01T00:00:01Z","action":"facet_rename","params":{"old_name":"work","new_name":"x"}}
"#,
        )
        .expect("rename log");
        let scan = scan_history(journal.path(), &Default::default()).expect("scan");
        assert!(scan.proposals.is_empty());
        assert_eq!(scan.reused, vec!["work".to_owned()]);
    }

    #[test]
    fn retire_records_a_historical_name_and_refuses_one_that_exists() {
        let journal = Journal::new();
        journal.declare("personal", PERSONAL, false);
        journal.declare("work", WORK, false);
        retire_name(journal.path(), "job", Some("work")).expect("retire");
        let entry = retired_facet_entry(journal.path(), "job").unwrap().unwrap();
        assert_eq!(entry.successor.as_deref(), Some(WORK));
        assert!(retire_name(journal.path(), "work", None).is_err());
        retire_name(journal.path(), "gone", None).expect("retire deleted");
        let entry = retired_facet_entry(journal.path(), "gone")
            .unwrap()
            .unwrap();
        assert_eq!(entry.state, RetiredFacetState::Deleted);
    }

    #[test]
    fn a_malformed_record_is_rebuilt_without_freeing_any_name() {
        let journal = Journal::new();
        journal.declare("personal", PERSONAL, false);
        journal.declare("work", WORK, false);
        retire_name(journal.path(), "job", Some("work")).expect("retire");
        retire_name(journal.path(), "later", None).expect("second write keeps a last good copy");
        let record = journal.path().join("facets/retired.json");
        // Damage the file: it still shows the names it reserved, including
        // one the last good copy and the logs don't know, and a live name.
        fs::write(
            &record,
            br#"{"names":{"job":{"state":"merged"},"later":{},"mystery":{"state":"merged"},"work":{},"#,
        )
        .expect("damage");
        let message = repair_malformed_retired_file(journal.path())
            .expect("repair")
            .expect("repaired");
        assert!(message.contains("mystery"), "{message}");
        let entries = read_retired_facets(journal.path()).entries();
        assert_eq!(entries["job"].successor.as_deref(), Some(WORK));
        assert_eq!(entries["mystery"].state, RetiredFacetState::Deleted);
        assert!(entries.contains_key("later"));
        assert!(
            !entries.contains_key("work"),
            "a live name is never reserved"
        );
        assert!(
            fs::read_dir(journal.path().join("facets"))
                .unwrap()
                .flatten()
                .any(|entry| entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("retired.json.malformed-"))
        );
    }

    // Unix mode bits make the record unreadable; Windows has no equivalent here.
    #[cfg(unix)]
    #[test]
    fn an_unreadable_record_is_left_where_it_is() {
        use std::os::unix::fs::PermissionsExt;
        let journal = Journal::new();
        journal.declare("personal", PERSONAL, false);
        let record = journal.path().join("facets/retired.json");
        fs::write(&record, b"{}").expect("record");
        fs::set_permissions(&record, fs::Permissions::from_mode(0o000)).expect("chmod");
        if fs::read(&record).is_ok() {
            // Running with privileges that ignore file modes; nothing to prove.
            return;
        }
        assert!(
            repair_malformed_retired_file(journal.path())
                .unwrap()
                .is_none()
        );
        fs::set_permissions(&record, fs::Permissions::from_mode(0o600)).expect("chmod back");
        assert_eq!(fs::read(&record).unwrap(), b"{}");
    }

    #[test]
    fn a_merge_that_would_leave_no_enabled_facet_is_refused() {
        let journal = Journal::new();
        journal.declare("personal", PERSONAL, false);
        journal.declare("work", WORK, true);
        let refusal = admit_facet_merge(journal.path(), "personal", "work", false)
            .err()
            .expect("refused");
        assert!(refusal.contains("only facet that isn't muted"), "{refusal}");
        // DEST needs a stable id for SOURCE's name to resolve to.
        fs::write(
            journal.path().join("facets/work/facet.json"),
            br#"{"title":"work"}"#,
        )
        .expect("id-less");
        journal.declare("x", X, false);
        assert!(
            admit_facet_merge(journal.path(), "x", "work", false)
                .err()
                .expect("refused")
                .contains("no stable id")
        );
    }
}
