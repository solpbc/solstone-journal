// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;

use serde_json::{Value, json};
use solstone_core_entity::{
    EncoderIdentity, EntityMergeOpenClass, EntityMergeOptions, EntityOperationContext,
    EntityOperationKind, commit_entity_merge_attributed, count_observations,
    entity_merge_open_class, entity_merge_recovery_pending, find_active_recorded_merge,
    load_merge_candidates_strict, read_entity_identity, read_identity_map, read_merge_log,
};
use solstone_core_facets::facet_links::LinkDirs;
use solstone_core_journal_link::Place;
use solstone_core_speaker_resolve::keep_separate::find_assertion;

use super::store::{SuggestionReceipt, SuggestionRecord};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum PairState {
    Free,
    OwnerAnswered,
    JournalCandidate,
}

pub(super) fn pair_key(first: &str, second: &str) -> String {
    let mut ids = [first, second];
    ids.sort_unstable();
    // Use a structured pair so separators in a durable ID cannot collide.
    format!("same_person:{}", json!(ids))
}

fn same_pair(first: &str, second: &str, left: &str, right: &str) -> bool {
    (first == left && second == right) || (first == right && second == left)
}

pub(super) fn pair_state(root: &Path, first: &str, second: &str) -> Result<PairState, String> {
    let mut candidate = false;
    for row in load_merge_candidates_strict(root).map_err(|error| error.to_string())? {
        let (Some(source), Some(target)) =
            (row["source_slug"].as_str(), row["target_slug"].as_str())
        else {
            continue;
        };
        if !same_pair(first, second, source, target) {
            continue;
        }
        if row["status"] == "dismissed" {
            return Ok(PairState::OwnerAnswered);
        }
        candidate |= entity_merge_open_class(&row) != EntityMergeOpenClass::Other;
    }
    let speaker_candidates =
        solstone_core_speaker_resolve::speaker_review_candidates::load_candidates_strict(root)
            .map_err(|error| error.to_string())?;
    for row in speaker_candidates {
        let (Some(source), Some(target)) = (row["source_id"].as_str(), row["target_id"].as_str())
        else {
            continue;
        };
        if !same_pair(first, second, source, target) {
            continue;
        }
        if row["status"] == "dismissed" {
            return Ok(PairState::OwnerAnswered);
        }
        candidate |= row["status"] == "open";
    }
    if find_assertion(root, first, second)
        .map_err(|error| error.to_string())?
        .is_some()
    {
        return Ok(PairState::OwnerAnswered);
    }
    Ok(if candidate {
        PairState::JournalCandidate
    } else {
        PairState::Free
    })
}

fn pair(record: &SuggestionRecord) -> Option<[&str; 2]> {
    Some([
        record.target["entity_id"].as_str()?,
        record.target["other_entity_id"].as_str()?,
    ])
}

pub(super) fn record_is_pair(record: &SuggestionRecord, first: &str, second: &str) -> bool {
    pair(record).is_some_and(|[left, right]| same_pair(first, second, left, right))
}

pub(super) fn record_has_address(record: &SuggestionRecord, id: &str, facet: &str) -> bool {
    record.kind == "same_person"
        && pair(record).is_some_and(|[first, second]| {
            (id == first && record.target["facet"].as_str() == Some(facet))
                || (id == second && record.target["other_facet"].as_str() == Some(facet))
        })
}

/// Use only a standing merge's old facet membership, never a guess about
/// the original source based on the survivor's current memberships.
pub(super) fn retired_address_in_facet(
    root: &Path,
    source: &str,
    successor: &str,
    facet: &str,
) -> bool {
    let Ok(Some(merge_id)) = find_active_recorded_merge(root, source, successor) else {
        return false;
    };
    read_merge_log(root).is_ok_and(|(rows, _)| {
        rows.iter().any(|row| {
            row.merge_id.as_deref() == Some(&merge_id)
                && row.source_id == source
                && row.target_id == successor
                && row.source_facet_ids.iter().any(|id| id == facet)
        })
    })
}

pub(super) fn person_name(root: &Path, id: &str) -> Option<String> {
    let map = read_identity_map(root).ok()?;
    let identity = read_entity_identity(root, map.resolved.get(id)?).ok()??;
    let value = identity.value();
    if value["id"] != id || value["type"] != "Person" || value["blocked"] == true {
        return None;
    }
    value["name"].as_str().map(str::to_owned)
}

pub(super) fn people_baseline(root: &Path, ids: [&str; 2]) -> Option<[Value; 2]> {
    let person = |id: &str| {
        let name = person_name(root, id)?;
        let open_url = solstone_core_journal_link::seal(root, &Place::Person { id: id.to_owned() })
            .map(|sealed| format!("/app/open/{sealed}"));
        Some(json!({"id":id, "name":name, "facets":[], "attachments":[], "open_url":open_url}))
    };
    Some([person(ids[0])?, person(ids[1])?])
}

fn people(root: &Path, ids: [&str; 2]) -> Option<[Value; 2]> {
    let mut people = people_baseline(root, ids)?;
    let mut attachments = [Vec::<Value>::new(), Vec::new()];
    for facet in solstone_core_facets::list_facet_directories(root).ok()? {
        let (links, _) = LinkDirs::for_facet(root, &facet).scan().ok()?;
        for link in links {
            if let Some(index) = ids.iter().position(|id| *id == link.entity_id)
                && link.id_written
                && !link.detached()
            {
                let notes = count_observations(root, &facet, &link.dir).ok();
                attachments[index].push(json!({"facet":facet, "notes":notes}));
            }
        }
    }
    for (person, attachments) in people.iter_mut().zip(attachments) {
        person["facets"] = json!(
            attachments
                .iter()
                .map(|item| &item["facet"])
                .collect::<Vec<_>>()
        );
        person["attachments"] = json!(attachments);
    }
    Some(people)
}

pub(super) fn change_parts(root: &Path, record: &SuggestionRecord, status: &str) -> Value {
    let current = if status == "pending" {
        pair(record)
            .and_then(|ids| people(root, ids))
            .map(|people| json!(people))
    } else {
        None
    };
    json!({
        "people": current.as_ref().unwrap_or(&record.baseline["people"]),
        "keep_id":record.value["keep_id"],
    })
}

pub(super) fn is_stale(root: &Path, record: &SuggestionRecord) -> bool {
    let Some([first, second]) = pair(record) else {
        return true;
    };
    if entity_merge_recovery_pending(root) {
        return true;
    }
    let names_match = [first, second].into_iter().enumerate().all(|(index, id)| {
        person_name(root, id).is_some_and(|name| record.baseline["people"][index]["name"] == name)
    });
    !names_match || !matches!(pair_state(root, first, second), Ok(PairState::Free))
}

/// Why a waiting card can no longer be accepted, so the owner's card can say
/// where the pair is answered. `journal_candidate` is only an open entity pair,
/// the one kind the possible duplicates list shows; a set-aside or speaker
/// candidate is `journal_question`. `busy` is a merge still being recovered or
/// an unreadable answer, which is not a change to either person.
pub(super) fn stale_reason(root: &Path, record: &SuggestionRecord) -> &'static str {
    let Some([first, second]) = pair(record) else {
        return "changed";
    };
    if entity_merge_recovery_pending(root) {
        return "busy";
    }
    let names_match = [first, second].into_iter().enumerate().all(|(index, id)| {
        person_name(root, id).is_some_and(|name| record.baseline["people"][index]["name"] == name)
    });
    if !names_match {
        return "changed";
    }
    match pair_state(root, first, second) {
        Ok(PairState::OwnerAnswered) => "owner_answered",
        Ok(PairState::JournalCandidate) if open_entity_pair(root, first, second) => {
            "journal_candidate"
        }
        Ok(PairState::JournalCandidate) => "journal_question",
        Ok(PairState::Free) => "changed",
        Err(_) => "busy",
    }
}

fn open_entity_pair(root: &Path, first: &str, second: &str) -> bool {
    load_merge_candidates_strict(root).is_ok_and(|rows| {
        rows.iter().any(|row| {
            matches!(
                (row["source_slug"].as_str(), row["target_slug"].as_str()),
                (Some(source), Some(target)) if same_pair(first, second, source, target)
            ) && entity_merge_open_class(row) == EntityMergeOpenClass::Open
        })
    })
}

pub(super) fn accept_value(record: &SuggestionRecord, value: &Value) -> Option<Value> {
    let keep = value.get("keep_id")?.as_str()?;
    (value.as_object()?.len() == 1 && pair(record)?.contains(&keep))
        .then(|| json!({"keep_id":keep}))
}

pub(super) fn apply(
    root: &Path,
    record: &SuggestionRecord,
    value: &Value,
) -> Result<SuggestionReceipt, String> {
    let [first, second] = pair(record).ok_or("missing people")?;
    let keep = value["keep_id"].as_str().ok_or("missing person to keep")?;
    let drop = if keep == first { second } else { first };
    let encoder = EncoderIdentity {
        id: "unresolved".to_owned(),
        sha256: "0".repeat(64),
        width: 256,
    };
    let context = EntityOperationContext {
        kind: EntityOperationKind::Merge,
        caller: Value::Null,
        actor: json!({"kind":"owner"}),
        metadata: json!({
            "suggestion_id":record.id,
            "suggested_by":{"source":record.source, "label":record.label},
        }),
    };
    let report = commit_entity_merge_attributed(
        root,
        drop,
        keep,
        EntityMergeOptions {
            keep_source_as_aka: true,
        },
        &encoder,
        &context,
    )
    .map_err(|error| error.to_string())?;
    Ok(SuggestionReceipt {
        entity_id: Some(report.target_id),
        ..Default::default()
    })
}

pub(super) fn existing_effect(
    root: &Path,
    record: &SuggestionRecord,
) -> Option<(Value, SuggestionReceipt)> {
    if entity_merge_recovery_pending(root) {
        return None;
    }
    let [first, second] = pair(record)?;
    for (source, target) in [(first, second), (second, first)] {
        let Ok(Some(merge_id)) = find_active_recorded_merge(root, source, target) else {
            continue;
        };
        // The surviving person can be merged again before a lost receipt is
        // retried. Its own removed history cannot be the only proof.
        let (audit, _) = read_merge_log(root).ok()?;
        let matches = audit.iter().any(|row| {
            row.merge_id.as_deref() == Some(&merge_id)
                && row.suggestion_id.as_deref() == Some(&record.id)
                && row.source_id == source
                && row.target_id == target
        });
        if matches {
            return Some((
                json!({"keep_id":target}),
                SuggestionReceipt {
                    entity_id: Some(target.to_owned()),
                    ..Default::default()
                },
            ));
        }
    }
    None
}
