// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Recording entity deletes the journal made before it kept a record of them.
//!
//! Since deletes started being recorded in `entities/retired.json`, the journal
//! never re-creates a deleted entity from its name. Deletes made before that
//! left only their action-log rows. This finds them there, and with `fix`
//! records each one whose entity is gone, so its name stays retired too.
//!
//! Every input it can't read is a refusal, never an absence: a merge log,
//! identity map, record or day file that can't be read would otherwise let a
//! tombstone land over a merge the journal really made.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde_json::Value;

use crate::action_log::read_journal_action_rows;

const DELETE_ACTION: &str = "journal_entity_delete";

/// What the doctor decided for one deleted id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeleteVerdict {
    /// Recorded (or, without `fix`, would be).
    Record,
    /// Left as it is, and why.
    Leave(String),
}

/// One id the action log shows was deleted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeleteFinding {
    pub entity_id: String,
    /// The day file of the row that counted.
    pub day: String,
    /// `immediate`, `committed` or `failed (removed)`.
    pub kind: String,
    /// The delete's time, as recorded when it was confirmed.
    pub at: Option<String>,
    pub verdict: DeleteVerdict,
}

/// A delete the log shows was asked for but never finished.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnfinishedDelete {
    pub entity_id: Option<String>,
    pub day: String,
    /// Whether the entity is still in the journal; `None` when an entity
    /// folder can't be read, so it can't be told.
    pub still_here: Option<bool>,
}

/// The whole picture, and what `fix` wrote.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EntityDoctorReport {
    /// The first and last action-log day it could see.
    pub window: Option<(String, String)>,
    pub unreadable_days: Vec<String>,
    pub malformed_action_lines: usize,
    pub malformed_merge_lines: usize,
    pub findings: Vec<DeleteFinding>,
    pub unfinished: Vec<UnfinishedDelete>,
    /// Ids merged away that are live again (re-created before merges were
    /// permanent). Reported only.
    pub live_again_merged: Vec<String>,
    /// Ids recorded by this run.
    pub recorded: Vec<String>,
    /// Why `entities/retired.json` can't be used, when it can't; owner text.
    pub record_problem: Option<String>,
    /// Entity folders whose `entity.json` can't be read; whether their
    /// entities are still here can't be told, so `fix` records nothing.
    pub unreadable_entities: Vec<String>,
    /// An entity merge was interrupted; `fix` settles it first.
    pub merge_recovery_pending: bool,
    /// A merge-log line can't be read and names no source; `fix` refuses.
    pub merge_log_incomplete: bool,
}

/// Why the doctor can't give a complete answer; nothing was written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntityDoctorError {
    /// An input that must be read couldn't be; the text names it.
    Unreadable(String),
    /// `fix` refused before writing anything; the text says why.
    Refused(String),
    /// A write failed partway; `recorded` were written before it.
    WriteFailed {
        recorded: Vec<String>,
        detail: String,
    },
}

impl std::fmt::Display for EntityDoctorError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unreadable(detail) | Self::Refused(detail) => formatter.write_str(detail),
            Self::WriteFailed { recorded, detail } => write!(
                formatter,
                "{detail} (recorded before it stopped: {})",
                if recorded.is_empty() {
                    "none".to_owned()
                } else {
                    recorded.join(", ")
                }
            ),
        }
    }
}

impl std::error::Error for EntityDoctorError {}

/// Report what the action log shows, writing nothing.
pub fn check_entity_records(journal: &Path) -> Result<EntityDoctorReport, EntityDoctorError> {
    let report = examine(journal)?;
    Ok(report)
}

/// Report, then record every delete the report marks `Record`. Runs under
/// the entity trust lock after settling any interrupted merge, and decides
/// again there, so nothing it writes rests on a reading taken outside it.
pub fn repair_entity_records(journal: &Path) -> Result<EntityDoctorReport, EntityDoctorError> {
    let _trust = solstone_core_entity::hold_entity_trust_lock(journal)
        .map_err(|error| EntityDoctorError::Refused(error.to_string()))?;
    solstone_core_entity::recover_interrupted_entity_merge(journal).map_err(|error| {
        EntityDoctorError::Refused(format!(
            "an interrupted entity merge couldn't be settled, so nothing was recorded: {error}"
        ))
    })?;
    let mut report = examine(journal)?;
    if let Some(day) = report.unreadable_days.first() {
        return Err(EntityDoctorError::Refused(format!(
            "config/actions/{day}.jsonl can't be read, so deletes from that day can't be checked; nothing was recorded"
        )));
    }
    if let solstone_core_entity::RetiredRecordHold::Damaged(detail) =
        solstone_core_entity::retired_record_hold(journal, "")
    {
        return Err(EntityDoctorError::Refused(detail));
    }
    if report.merge_log_incomplete {
        return Err(EntityDoctorError::Refused(
            "a line in logs/entity-merges.jsonl can't be read and doesn't say which entity it merged, so no delete can be checked against it; nothing was recorded".to_owned(),
        ));
    }
    if !report.unreadable_entities.is_empty() {
        return Err(EntityDoctorError::Refused(format!(
            "these entity folders can't be read or don't say which entity they hold, so no delete can be checked against them; nothing was recorded: {}",
            report.unreadable_entities.join(", ")
        )));
    }
    let mut recorded = Vec::new();
    for finding in &report.findings {
        if finding.verdict != DeleteVerdict::Record {
            continue;
        }
        let at = finding.at.clone().unwrap_or_default();
        match solstone_core_entity::record_seeded_deletion(journal, &finding.entity_id, &at) {
            Ok(true) => recorded.push(finding.entity_id.clone()),
            Ok(false) => {}
            Err(detail) => return Err(EntityDoctorError::WriteFailed { recorded, detail }),
        }
    }
    report.recorded = recorded;
    Ok(report)
}

/// One delete row, reduced to what the doctor weighs.
struct DeleteRow {
    day: String,
    entity_id: Option<String>,
    pending_id: Option<String>,
    phase: Option<String>,
    timestamp: Option<String>,
    removed: bool,
    unsettled: bool,
}

fn examine(journal: &Path) -> Result<EntityDoctorReport, EntityDoctorError> {
    let scan = read_journal_action_rows(journal, DELETE_ACTION).map_err(|error| {
        EntityDoctorError::Unreadable(format!("config/actions can't be listed: {error}"))
    })?;
    let (merges, unreadable_merge_lines) =
        solstone_core_entity::read_merge_log(journal).map_err(|error| {
            EntityDoctorError::Unreadable(format!(
                "logs/entity-merges.jsonl can't be read, so merges can't be checked: {error}"
            ))
        })?;
    let live = solstone_core_entity::read_identity_map(journal).map_err(|error| {
        EntityDoctorError::Unreadable(format!(
            "the journal's entities can't be read, so which ones are still here can't be checked: {error}"
        ))
    })?;
    // Entity folders the map didn't resolve. Their identity files, damaged or
    // set aside, still say which id they held, and it is read from them
    // exactly. A file that names no id, or can't be read, makes every answer
    // unknown: missing an id is not proof an entity is gone.
    let resolved_dirs: BTreeSet<&String> = live.resolved.values().collect();
    let collision_dirs: BTreeSet<&String> = live
        .losers
        .iter()
        .filter(|loser| loser.reason == solstone_core_entity::IdentityMapLoserReason::CollisionLost)
        .map(|loser| &loser.entity_dir)
        .collect();
    let mut unreadable_entities = Vec::new();
    let mut unresolved_ids: BTreeSet<String> = BTreeSet::new();
    let entities_dir = solstone_core_journal_io::contained_path(journal, "entities")
        .map_err(|error| EntityDoctorError::Unreadable(error.to_string()))?;
    if solstone_core_journal_io::path_lexists(&entities_dir)
        .map_err(|error| EntityDoctorError::Unreadable(error.to_string()))?
    {
        for entry in solstone_core_journal_io::list_dir_entries(&entities_dir).map_err(|error| {
            EntityDoctorError::Unreadable(format!("entities can't be listed: {error}"))
        })? {
            let dir = entry.name.to_string_lossy().into_owned();
            if entry.kind != solstone_core_journal_io::DirEntryKind::Directory
                || resolved_dirs.contains(&dir)
                || collision_dirs.contains(&dir)
            {
                continue;
            }
            match ids_in_unresolved_folder(&entry.path) {
                Some(ids) => unresolved_ids.extend(ids),
                None => unreadable_entities.push(dir),
            }
        }
    }
    let is_live = |id: &str| -> Result<bool, EntityDoctorError> {
        if live.resolved.contains_key(id) {
            return Ok(true);
        }
        if unresolved_ids.contains(id) {
            return Ok(true);
        }
        // Anything at the folder, readable or not, is not a deleted entity.
        let path = solstone_core_journal_io::contained_path(journal, &format!("entities/{id}"))
            .map_err(|error| EntityDoctorError::Unreadable(error.to_string()))?;
        solstone_core_journal_io::path_lexists(&path)
            .map_err(|error| EntityDoctorError::Unreadable(error.to_string()))
    };

    let rows: Vec<DeleteRow> = scan
        .rows
        .iter()
        .map(|(day, record)| {
            let params = record.get("params").cloned().unwrap_or(Value::Null);
            let text = |key: &str| {
                params
                    .get(key)
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty())
                    .map(str::to_owned)
            };
            DeleteRow {
                day: day.clone(),
                entity_id: text("entity_id"),
                pending_id: text("pending_id"),
                phase: text("phase"),
                timestamp: record
                    .get("timestamp")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                removed: params.get("entity_removed") == Some(&Value::Bool(true)),
                unsettled: params.get("unsettled") == Some(&Value::Bool(true)),
            }
        })
        .collect();

    // A deferred delete is confirmed by its `pending` row; that is when the
    // owner asked for it, and the time a merge must be weighed against. Its
    // outcome row can come later, and before the outcome rows were honest a
    // `committed` row could follow a delete that failed.
    let mut pending_at: BTreeMap<String, (Option<String>, Option<String>, String)> =
        BTreeMap::new();
    let mut outcome_seen: BTreeMap<String, &DeleteRow> = BTreeMap::new();
    for row in &rows {
        let Some(pending_id) = &row.pending_id else {
            continue;
        };
        match row.phase.as_deref() {
            Some("pending") => {
                pending_at.insert(
                    pending_id.clone(),
                    (
                        row.timestamp.clone(),
                        row.entity_id.clone(),
                        row.day.clone(),
                    ),
                );
            }
            Some(_) => {
                outcome_seen.insert(pending_id.clone(), row);
            }
            None => {}
        }
    }

    // A merge line that can't be read still names its source when the
    // source can be read from it; that id counts as merged at an unknown
    // time. A line that names no source could be any merge.
    let mut unreadable_merge_sources: BTreeSet<String> = BTreeSet::new();
    let mut unreadable_merge_unknown = false;
    for line in &unreadable_merge_lines {
        match string_values_for_key(line.as_bytes(), "source_id") {
            Some(sources) if !sources.is_empty() => unreadable_merge_sources.extend(sources),
            _ => unreadable_merge_unknown = true,
        }
    }
    let merged_at = |id: &str| -> Vec<Option<i64>> {
        merges
            .iter()
            .filter(|row| row.source_id == id)
            .map(|row| row.ts_ms)
            .chain(unreadable_merge_sources.contains(id).then_some(None))
            .collect()
    };

    // The latest counted row per id decides.
    let mut counted: BTreeMap<String, (String, String, Option<String>, bool)> = BTreeMap::new();
    for row in &rows {
        let (kind, confirmed_at, pairing_missing) = match (row.phase.as_deref(), &row.pending_id) {
            (None, _) => ("immediate", row.timestamp.clone(), false),
            (Some("committed"), pending) => {
                let paired = pending.as_ref().and_then(|id| pending_at.get(id));
                (
                    "committed",
                    paired.and_then(|(at, _, _)| at.clone()),
                    paired.is_none(),
                )
            }
            (Some("failed"), pending) if row.removed => {
                let paired = pending.as_ref().and_then(|id| pending_at.get(id));
                (
                    "failed (removed)",
                    paired.and_then(|(at, _, _)| at.clone()),
                    paired.is_none(),
                )
            }
            _ => continue,
        };
        let Some(entity_id) = row.entity_id.clone().or_else(|| {
            row.pending_id
                .as_ref()
                .and_then(|id| pending_at.get(id))
                .and_then(|(_, id, _)| id.clone())
        }) else {
            continue;
        };
        // Without its confirmation row, a deferred delete's time is only
        // trustworthy when there is no merge to weigh it against.
        let at = if pairing_missing {
            if merged_at(&entity_id).is_empty() {
                row.timestamp.clone()
            } else {
                None
            }
        } else {
            confirmed_at
        };
        counted.insert(
            entity_id,
            (row.day.clone(), kind.to_owned(), at, pairing_missing),
        );
    }

    let mut report = EntityDoctorReport {
        merge_recovery_pending: solstone_core_entity::entity_merge_recovery_pending(journal),
        merge_log_incomplete: unreadable_merge_unknown,
        unreadable_entities: unreadable_entities.clone(),
        record_problem: match solstone_core_entity::retired_record_hold(journal, "") {
            solstone_core_entity::RetiredRecordHold::Damaged(detail) => Some(detail),
            _ => None,
        },
        window: scan.window.clone(),
        unreadable_days: scan.unreadable_days.clone(),
        malformed_action_lines: scan.malformed,
        malformed_merge_lines: unreadable_merge_lines.len(),
        ..EntityDoctorReport::default()
    };
    for (entity_id, (day, kind, at, _)) in counted {
        let verdict = if unreadable_merge_unknown {
            DeleteVerdict::Leave(
                "a line in logs/entity-merges.jsonl can't be read, so whether this was merged can't be told"
                    .to_owned(),
            )
        } else if unreadable_entities.is_empty() {
            decide(
                journal,
                &entity_id,
                at.as_deref(),
                &merged_at(&entity_id),
                &is_live,
            )?
        } else {
            DeleteVerdict::Leave(
                "an entity folder can't be read or doesn't say which entity it holds, so this one can't be checked"
                    .to_owned(),
            )
        };
        report.findings.push(DeleteFinding {
            entity_id,
            day,
            kind,
            at,
            verdict,
        });
    }

    // Deletes asked for and never finished: a `pending` with no outcome, or a
    // `failed` that left the removal unsettled.
    for (pending_id, (_, entity_id, day)) in &pending_at {
        let finished = outcome_seen
            .get(pending_id)
            .is_some_and(|row| !(row.phase.as_deref() == Some("failed") && row.unsettled));
        if finished {
            continue;
        }
        let still_here = match entity_id {
            Some(id) if solstone_core_entity::facet_links::is_folder_name(id) => {
                if is_live(id)? {
                    Some(true)
                } else if unreadable_entities.is_empty() {
                    Some(false)
                } else {
                    None
                }
            }
            _ => Some(false),
        };
        report.unfinished.push(UnfinishedDelete {
            entity_id: entity_id.clone(),
            day: day.clone(),
            still_here,
        });
    }

    let mut merged_ids: BTreeSet<String> = merges.iter().map(|row| row.source_id.clone()).collect();
    if let solstone_core_entity::RetiredEntities::Loaded(entries) =
        solstone_core_entity::read_retired_entities(journal)
    {
        merged_ids.extend(entries.into_keys());
    }
    for id in merged_ids {
        if live.resolved.contains_key(&id) {
            report.live_again_merged.push(id);
        }
    }
    Ok(report)
}

fn decide(
    journal: &Path,
    entity_id: &str,
    at: Option<&str>,
    merges: &[Option<i64>],
    is_live: &dyn Fn(&str) -> Result<bool, EntityDoctorError>,
) -> Result<DeleteVerdict, EntityDoctorError> {
    let leave = |reason: &str| Ok(DeleteVerdict::Leave(reason.to_owned()));
    if !solstone_core_entity::facet_links::is_folder_name(entity_id) {
        return leave("its id can't name an entity");
    }
    if is_live(entity_id)? {
        return leave("it is in the journal now");
    }
    match solstone_core_entity::retired_record_hold(journal, entity_id) {
        solstone_core_entity::RetiredRecordHold::Held => return leave("it is already recorded"),
        solstone_core_entity::RetiredRecordHold::Damaged(_) => {
            return leave("the record can't be read");
        }
        solstone_core_entity::RetiredRecordHold::Free => {}
    }
    let Some(deleted_ms) = at
        .and_then(|at| chrono::DateTime::parse_from_rfc3339(at).ok())
        .map(|at| at.timestamp_millis())
    else {
        return leave("when it was deleted can't be told");
    };
    if merges.is_empty() {
        return Ok(DeleteVerdict::Record);
    }
    // A merge newer than the delete stands: the delete can only have been of
    // an earlier entity with that id, and the merge log already answers for
    // the name.
    if merges.iter().any(Option::is_none) {
        return leave(
            "a merge of it can't be dated, so whether it was merged after it was deleted can't be told",
        );
    }
    if merges
        .iter()
        .any(|merged| merged.is_some_and(|ms| ms >= deleted_ms))
    {
        return leave("it was merged after it was deleted");
    }
    Ok(DeleteVerdict::Record)
}

/// Every string value stored under `key` in bytes that may not parse as JSON:
/// each `"key"` followed by `:` and a JSON string, decoded. Used to read ids
/// out of damaged files, where a missing value means "unknown", never "none".
///
/// `None` when any `"key":` isn't followed by a whole, non-empty JSON string
/// that ends where a value can end (at `,`, `}` or the end of the text): a
/// torn value can't be told from the start of a different one, and one good
/// value doesn't vouch for a damaged one beside it.
fn string_values_for_key(text: &[u8], key: &str) -> Option<Vec<String>> {
    let needle = format!("\"{key}\"");
    let needle = needle.as_bytes();
    let skip_space = |mut at: usize| {
        while at < text.len() && text[at].is_ascii_whitespace() {
            at += 1;
        }
        at
    };
    let mut values = Vec::new();
    let mut from = 0;
    while let Some(found) = text[from..]
        .windows(needle.len())
        .position(|window| window == needle)
    {
        from += found + needle.len();
        let colon = skip_space(from);
        if text.get(colon) != Some(&b':') {
            continue;
        }
        let open = skip_space(colon + 1);
        if text.get(open) != Some(&b'"') {
            return None;
        }
        // The shortest prefix that parses as a JSON string is the value.
        let mut close = open + 1;
        let mut escaped = false;
        while close < text.len() {
            match text[close] {
                b'\\' if !escaped => escaped = true,
                b'"' if !escaped => break,
                _ => escaped = false,
            }
            close += 1;
        }
        if close >= text.len() {
            return None;
        }
        let value = serde_json::from_slice::<String>(&text[open..=close]).ok()?;
        let follower = skip_space(close + 1);
        if value.is_empty() || !matches!(text.get(follower), None | Some(b',' | b'}')) {
            return None;
        }
        values.push(value);
        from = close + 1;
    }
    Some(values)
}

/// The ids an entity folder's identity files name, when the map couldn't
/// resolve the folder. `None` when any of them can't be read, isn't a plain
/// file, is implausibly large, or names no id: the folder's entity is then
/// unknown.
fn ids_in_unresolved_folder(folder: &Path) -> Option<Vec<String>> {
    const MAX_IDENTITY_BYTES: u64 = 1 << 20;
    let mut ids = Vec::new();
    for file in solstone_core_journal_io::list_dir_entries(folder).ok()? {
        if !file.name.to_string_lossy().starts_with("entity.") {
            continue;
        }
        // Opened once, never through a link or a pipe, and read only up to
        // the cap, so a file swapped after the listing can't stall the run.
        let bytes =
            solstone_core_journal_io::read_regular_file_capped(&file.path, MAX_IDENTITY_BYTES)
                .ok()?;
        // Stricter than the identity census, which reads an empty file as no
        // entity: a file truncated to nothing may have named one.
        let found = string_values_for_key(&bytes, "id")?;
        if found.is_empty() {
            return None;
        }
        ids.extend(found);
    }
    Some(ids)
}

#[cfg(test)]
mod tests {
    use super::string_values_for_key;

    fn values(text: &str) -> Option<Vec<String>> {
        string_values_for_key(text.as_bytes(), "id")
    }

    #[test]
    fn a_value_is_taken_only_when_it_is_whole() {
        assert_eq!(
            values(r#"{"id": "a\"b", "x": 1}"#),
            Some(vec!["a\"b".to_owned()])
        );
        assert_eq!(values(r#"{"id":"é"}"#), Some(vec!["é".to_owned()]));
        assert_eq!(values(r#"{"id": "alice""#), Some(vec!["alice".to_owned()]));
        assert_eq!(values(r#"{"name": "no key here"}"#), Some(vec![]));
        assert_eq!(values(r#"["id", "x"]"#), Some(vec![]));
        // Torn, glued, empty, not text, cut mid-escape.
        assert_eq!(values(r#"{"id":"ali{"id":"zed"}"#), None);
        assert_eq!(values(r#"{"id": ""}"#), None);
        assert_eq!(values(r#"{"id": null}"#), None);
        assert_eq!(values(r#"{"id": "ab\"#), None);
        assert_eq!(values(r#"{"id": "ali"#), None);
        // One good value doesn't vouch for a damaged one beside it.
        assert_eq!(values(r#"{"id": "", "id": "zed"}"#), None);
        assert_eq!(string_values_for_key(b"{\"id\": \"al\xffce\"}", "id"), None);
    }
}
