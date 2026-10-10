// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Owner references written before the journal knew its owner.

use std::fs;
use std::path::Path;

use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::{Map, Value};
use solstone_core_facets::DestinationObservation;

use crate::{JournalOwner, NamedActor};

/// The actor fields a Story item names, each with the id field the Story hook
/// resolves it into.
const ACTOR_FIELDS: [(&str, &str); 4] = [
    ("owner", "owner_entity_id"),
    ("counterparty", "counterparty_entity_id"),
    ("from", "from_entity_id"),
    ("to", "to_entity_id"),
];

/// Give the owner's id to every activity actor written as the owner ("you",
/// or one of the owner's own names) whose id was left empty because the
/// journal had no principal when the item was written. That is the id the
/// Story hook writes once a principal exists.
///
/// Meeting records are left as they are: there the hook also requires the
/// owner's recognized voice before "you" stands, which this pass does not
/// re-check. Muted or unreadable facets are skipped, as the hooks skip them.
///
/// Filling an id changes that day's daily evidence, which re-owes the day's
/// daily outputs. So, as for a contract change (operator approval,
/// 2026-09-30), only today and the last seven closed days are filled; older
/// day files are left exactly as written.
/// Returns how many actors it filled.
pub fn resolve_owner_actors(journal: &Path) -> Result<usize, String> {
    resolve_owner_actors_at(journal, Utc::now())
}

fn resolve_owner_actors_at(journal: &Path, now: DateTime<Utc>) -> Result<usize, String> {
    let owner = JournalOwner::load(journal)?;
    let Some(owner_id) = owner.id.clone() else {
        return Ok(0);
    };
    let today = chrono::NaiveDate::parse_from_str(
        &solstone_core_system::daily_coverage::local_day(journal, now),
        "%Y%m%d",
    )
    .map_err(|e| e.to_string())?;
    let first_day = (today
        - chrono::Duration::days(solstone_core_system::daily_coverage::CONTRACT_REOWE_CLOSED_DAYS))
    .format("%Y%m%d")
    .to_string();
    let mut filled = 0;
    for facet in
        solstone_core_facets::list_declared_facet_names(journal).map_err(|e| e.to_string())?
    {
        let Ok(DestinationObservation::Ready { id, muted: false }) =
            solstone_core_facets::observe_facet_destination(journal, &facet)
        else {
            continue;
        };
        for day in activity_days(journal, &facet)
            .into_iter()
            .filter(|day| *day >= first_day)
        {
            let records = solstone_core_facets::load_activity_records(journal, &facet, &day, true)
                .map_err(|e| e.to_string())?;
            for record in records {
                // The snapshot only finds candidates. The patch replaces whole
                // arrays, so it is derived from the record as it stands inside
                // the writer's lock, never from the snapshot.
                if owner_patch(&record, &owner, &owner_id).is_none() {
                    continue;
                }
                let Some(record_id) = record.get("id").and_then(Value::as_str) else {
                    continue;
                };
                let _guard = solstone_core_facets::hold_activity_enrichment(journal, &facet, &id)
                    .map_err(|e| e.to_string())?;
                let mut count = 0;
                let timestamp = Utc::now().to_rfc3339_opts(SecondsFormat::Micros, true);
                solstone_core_facets::update_activity_record_with(
                    journal,
                    &facet,
                    &day,
                    record_id,
                    |current| {
                        let (_, patch, filled) = owner_patch(current, &owner, &owner_id)?;
                        count = filled;
                        Some(patch)
                    },
                    "owner_identity",
                    "resolved references to you",
                    &timestamp,
                )
                .map_err(|e| e.to_string())?;
                filled += count;
            }
        }
    }
    Ok(filled)
}

/// The arrays of one record whose owner references gain the owner's id, the
/// record's id, and how many actors that fills; `None` when nothing changes.
fn owner_patch(
    record: &Map<String, Value>,
    owner: &JournalOwner,
    owner_id: &str,
) -> Option<(String, Map<String, Value>, usize)> {
    if record.get("activity").and_then(Value::as_str) == Some("meeting") {
        return None;
    }
    let record_id = record
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())?;
    let mut patch = Map::new();
    let mut count = 0;
    for key in ["participation", "relations"] {
        let Some(Value::Array(items)) = record.get(key) else {
            continue;
        };
        let mut changed = false;
        let items = items
            .iter()
            .map(|item| {
                let Value::Object(object) = item else {
                    return item.clone();
                };
                let mut object = object.clone();
                for (field, id_field) in ACTOR_FIELDS {
                    if object.get(id_field) == Some(&Value::Null)
                        && object
                            .get(field)
                            .and_then(Value::as_str)
                            .is_some_and(|name| owner.actor(name) == NamedActor::Owner)
                    {
                        object.insert(id_field.to_owned(), Value::String(owner_id.to_owned()));
                        changed = true;
                        count += 1;
                    }
                }
                Value::Object(object)
            })
            .collect();
        if changed {
            patch.insert(key.to_string(), Value::Array(items));
        }
    }
    (!patch.is_empty()).then(|| (record_id.to_owned(), patch, count))
}

/// The days a facet holds activity records for, from its day files' names.
fn activity_days(journal: &Path, facet: &str) -> Vec<String> {
    let Ok(directory) =
        solstone_core_journal_io::contained_path(journal, &format!("facets/{facet}/activities"))
    else {
        return Vec::new();
    };
    let Ok(entries) = fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut days = entries
        .filter_map(Result::ok)
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter_map(|name| name.strip_suffix(".jsonl").map(str::to_owned))
        .filter(|stem| stem.len() == 8 && stem.bytes().all(|byte| byte.is_ascii_digit()))
        .collect::<Vec<_>>();
    days.sort();
    days
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// Noon on 20260101, the day of the journal's records, in its zone.
    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-01-01T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    fn journal(principal: bool) -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        solstone_core_facets::create_facet(root.path(), "work", "Work", "", "", "", None).unwrap();
        let write = |path: &str, text: String| {
            let path = root.path().join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        };
        write(
            "config/journal.json",
            json!({"identity":{"name":"Jordan Rivers","timezone":"UTC"}}).to_string(),
        );
        write(
            "entities/jordan/entity.json",
            json!({"id":"jordan","name":"Jordan Rivers","type":"Person","is_principal":principal})
                .to_string(),
        );
        let rows = [
            json!({"id":"a1","activity":"coding","relations":[
                {"from":"you","from_entity_id":null,"to":"Priya","to_entity_id":null,"kind":"works-with","note":""},
                {"from":"Jordan Rivers","from_entity_id":null,"to":"Priya","to_entity_id":"priya","kind":"works-with","note":""},
                {"from":"you","from_entity_id":"someone_else","to":"Priya","to_entity_id":"priya","kind":"works-with","note":""},
                {"from":"your agent","from_entity_id":null,"to":"Priya","to_entity_id":"priya","kind":"works-with","note":""}
            ]}),
            json!({"id":"m1","activity":"meeting","relations":[
                {"from":"you","from_entity_id":null,"to":"Priya","to_entity_id":"priya","kind":"works-with","note":""}
            ]}),
        ];
        write(
            "facets/work/activities/20260101.jsonl",
            rows.iter().map(|row| row.to_string() + "\n").collect(),
        );
        root
    }

    fn relations(root: &Path, id: &str) -> Vec<Value> {
        relations_on(root, "20260101", id)
    }

    fn relations_on(root: &Path, day: &str, id: &str) -> Vec<Value> {
        solstone_core_facets::get_activity_record(root, "work", day, id)
            .unwrap()
            .unwrap()["relations"]
            .as_array()
            .unwrap()
            .clone()
    }

    #[test]
    fn an_owner_reference_written_without_an_owner_gains_the_owners_id() {
        let root = journal(true);
        assert_eq!(resolve_owner_actors_at(root.path(), now()).unwrap(), 2);
        let items = relations(root.path(), "a1");
        assert_eq!(items[0]["from_entity_id"], "jordan");
        assert!(items[0]["to_entity_id"].is_null());
        assert_eq!(items[1]["from_entity_id"], "jordan");
        assert_eq!(items[2]["from_entity_id"], "someone_else");
        assert!(items[3]["from_entity_id"].is_null());
        // A meeting's "you" also rests on the owner's voice, which this pass
        // does not check, so it is left as it is.
        assert!(relations(root.path(), "m1")[0]["from_entity_id"].is_null());
        // A second pass finds nothing left to fill.
        assert_eq!(resolve_owner_actors_at(root.path(), now()).unwrap(), 0);
    }

    #[test]
    fn without_a_principal_nothing_is_filled() {
        let root = journal(false);
        let before = fs::read(root.path().join("facets/work/activities/20260101.jsonl")).unwrap();
        assert_eq!(resolve_owner_actors_at(root.path(), now()).unwrap(), 0);
        assert_eq!(
            fs::read(root.path().join("facets/work/activities/20260101.jsonl")).unwrap(),
            before
        );
    }

    #[test]
    fn a_reference_older_than_the_re_derivable_days_is_left_as_written() {
        // Filling an id changes that day's daily evidence and re-owes its daily
        // outputs. A release re-derives only the open day and the last seven
        // closed days (2026-09-30), so the start-up pass reaches no further back.
        let root = journal(true);
        let source = root.path().join("facets/work/activities/20260101.jsonl");
        let rows = fs::read(&source).unwrap();
        let oldest_kept = root.path().join("facets/work/activities/20251225.jsonl");
        let oldest_re_derived = root.path().join("facets/work/activities/20251226.jsonl");
        fs::write(&oldest_kept, &rows).unwrap();
        fs::rename(&source, &oldest_re_derived).unwrap();
        // Today is 20260102: the seven closed days before it start at 20251226.
        let today = DateTime::parse_from_rfc3339("2026-01-02T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);

        assert_eq!(resolve_owner_actors_at(root.path(), today).unwrap(), 2);

        assert_eq!(fs::read(&oldest_kept).unwrap(), rows);
        let items = relations_on(root.path(), "20251226", "a1");
        assert_eq!(items[0]["from_entity_id"], "jordan");
        assert_eq!(items[1]["from_entity_id"], "jordan");
    }
}
