// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::Path;

use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use solstone_core_journal_config::{entity_tier8_typo_acceptance_enabled, read_journal_config};
use solstone_core_journal_io::{
    JsonWriteOptions, MalformedPolicy, contained_path, read_json, write_json,
};

use super::census::{IdentityCensus, scan_identity_census};
use super::write::{
    AmbiguityChoiceEntity, EntityWriteError, ambiguity_now_iso, mutate_ambiguities,
};
use crate::hold_entity_trust_lock;

pub const PREFIX_CUTOFF: f64 = 35.0;
pub const TYPO_FLOOR: f64 = 93.0;
pub const ENTITY_REVIEW_POLICY_VERSION: u64 = 1;
pub const REVIEW_SWEEP_RECEIPT_RELATIVE_PATH: &str = "health/entity-review-sweep.json";

/// Return whether a normalized query is a recognized placeholder.
pub fn is_placeholder_query(normalized_query: &str) -> bool {
    let query = normalized_query.trim();
    if let Some(rest) = query.strip_prefix("speaker ") {
        !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit())
    } else if let Some(rest) = query.strip_prefix("participant ") {
        !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit())
    } else if let Some(rest) = query.strip_prefix("colleague ") {
        rest.len() == 1 && rest.chars().all(|c| c.is_ascii_lowercase())
    } else {
        false
    }
}

/// Validate the `review` sub-object schema.
pub fn validate_review_object(
    review: &Map<String, Value>,
    is_merge: bool,
) -> Result<(), &'static str> {
    if let Some(suppression) = review.get("suppression")
        && !suppression.is_null()
    {
        let obj = suppression
            .as_object()
            .ok_or("review.suppression is not an object")?;
        let reason = obj
            .get("reason")
            .and_then(Value::as_str)
            .ok_or("missing or invalid review.suppression.reason")?;
        if is_merge {
            if reason != "stale" {
                return Err("merge candidate review suppression reason must be stale");
            }
        } else if !matches!(
            reason,
            "stale" | "speaker_type" | "placeholder" | "low_prefix"
        ) {
            return Err("unknown review.suppression.reason");
        }
        if obj.get("at").and_then(Value::as_str).is_none() {
            return Err("missing or invalid review.suppression.at");
        }
        if obj.get("evidence_key").and_then(Value::as_str).is_none() {
            return Err("missing or invalid review.suppression.evidence_key");
        }
    }

    if let Some(released) = review.get("released") {
        let arr = released
            .as_array()
            .ok_or("review.released is not an array")?;
        for item in arr {
            let obj = item
                .as_object()
                .ok_or("review.released entry is not an object")?;
            let reason = obj
                .get("reason")
                .and_then(Value::as_str)
                .ok_or("missing or invalid review.released.reason")?;
            if is_merge {
                if reason != "stale" {
                    return Err("merge candidate review.released reason must be stale");
                }
            } else if !matches!(
                reason,
                "stale" | "speaker_type" | "placeholder" | "low_prefix" | "typo"
            ) {
                return Err("unknown review.released.reason");
            }
            if obj.get("evidence_key").and_then(Value::as_str).is_none() {
                return Err("missing or invalid review.released.evidence_key");
            }
        }
    }

    if let Some(history) = review.get("history") {
        let arr = history.as_array().ok_or("review.history is not an array")?;
        for item in arr {
            let obj = item
                .as_object()
                .ok_or("review.history entry is not an object")?;
            let action = obj
                .get("action")
                .and_then(Value::as_str)
                .ok_or("missing or invalid review.history.action")?;
            if !matches!(
                action,
                "set_aside" | "reopen" | "undo" | "auto_resolve" | "undo_choice"
            ) {
                return Err("unknown review.history.action");
            }
            let reason = obj
                .get("reason")
                .and_then(Value::as_str)
                .ok_or("missing or invalid review.history.reason")?;
            if is_merge {
                if reason != "stale" {
                    return Err("merge candidate review.history reason must be stale");
                }
            } else if !matches!(
                reason,
                "stale" | "speaker_type" | "placeholder" | "low_prefix" | "typo"
            ) {
                return Err("unknown review.history.reason");
            }
            if obj.get("at").and_then(Value::as_str).is_none() {
                return Err("missing or invalid review.history.at");
            }
            if obj.get("evidence_key").and_then(Value::as_str).is_none() {
                return Err("missing or invalid review.history.evidence_key");
            }
        }
    }

    if let Some(choice) = review.get("choice")
        && !choice.is_null()
    {
        if is_merge {
            return Err("merge candidate review cannot contain choice");
        }
        let obj = choice.as_object().ok_or("review.choice is not an object")?;
        if obj.get("kind").and_then(Value::as_str) != Some("automatic") {
            return Err("review.choice.kind must be automatic");
        }
        if obj.get("reason").and_then(Value::as_str) != Some("typo") {
            return Err("review.choice.reason must be typo");
        }
        if obj.get("entity_id").and_then(Value::as_str).is_none() {
            return Err("missing or invalid review.choice.entity_id");
        }
        if obj.get("at").and_then(Value::as_str).is_none() {
            return Err("missing or invalid review.choice.at");
        }
    }

    Ok(())
}

pub(crate) fn ensure_review_object(
    row: &mut Map<String, Value>,
    is_merge: bool,
) -> &mut Map<String, Value> {
    if !row.contains_key("review") || !row["review"].is_object() {
        let mut review = Map::new();
        review.insert("suppression".to_owned(), Value::Null);
        review.insert("released".to_owned(), Value::Array(Vec::new()));
        review.insert("history".to_owned(), Value::Array(Vec::new()));
        if !is_merge {
            review.insert("choice".to_owned(), Value::Null);
        }
        row.insert("review".to_owned(), Value::Object(review));
    }
    row.get_mut("review")
        .and_then(Value::as_object_mut)
        .expect("review object ensured")
}

/// Evaluate and apply automatic review policies to one ambiguity row.
pub fn apply_ambiguity_review_policy(
    row: &mut Map<String, Value>,
    census: &IdentityCensus,
    typo_acceptance_enabled: bool,
    now: &str,
) {
    let status = row
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("open")
        .to_owned();

    // Owner resolved and dismissed rows are never rewritten by policy.
    if status == "dismissed" {
        return;
    }
    if status == "resolved" {
        // If it is an automatic typo resolution, verify it remains untouched.
        return;
    }

    let origins = row
        .get("origins")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let has_participation = origins.iter().any(|origin| {
        origin
            .as_object()
            .and_then(|o| o.get("lane"))
            .and_then(Value::as_str)
            == Some("talent.participation")
    });

    let normalized_query = row
        .get("normalized_query")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let observed_tier = row
        .get("observed_tier")
        .and_then(Value::as_i64)
        .unwrap_or_default();

    let candidates = row
        .get("ranked_candidates")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let candidate_ids: Vec<String> = candidates
        .iter()
        .filter_map(|c| c.get("id").and_then(Value::as_str).map(str::to_owned))
        .collect();

    let proved_present: Vec<String> = candidate_ids
        .iter()
        .filter(|id| census.is_proved_present(id))
        .cloned()
        .collect();
    let proved_absent: Vec<String> = candidate_ids
        .iter()
        .filter(|id| census.is_proved_absent(id))
        .cloned()
        .collect();
    let all_proved =
        census.complete && (proved_present.len() + proved_absent.len() == candidate_ids.len());

    let is_speaker_only = !origins.is_empty()
        && origins.iter().all(|origin| {
            origin
                .as_object()
                .and_then(|o| o.get("lane"))
                .and_then(Value::as_str)
                .is_some_and(|lane| {
                    lane == "apps.speakers.attribution" || lane == "talent.speaker_attribution"
                })
        });

    // Check suppression predicates in priority order
    let mut qualifying_suppression: Option<(&'static str, String)> = None;

    if !has_participation {
        // 1. Stale: all candidates proved absent
        if census.complete && all_proved && proved_present.is_empty() && !candidate_ids.is_empty() {
            let mut sorted_ids = candidate_ids.clone();
            sorted_ids.sort();
            qualifying_suppression = Some(("stale", format!("stale:{}", sorted_ids.join(","))));
        }

        // 2. Placeholder: normalized query matches full placeholder pattern
        if qualifying_suppression.is_none() && is_placeholder_query(&normalized_query) {
            qualifying_suppression =
                Some(("placeholder", format!("placeholder:{normalized_query}")));
        }

        // 3. Speaker-only non-Person:
        if qualifying_suppression.is_none()
            && is_speaker_only
            && all_proved
            && !proved_present.is_empty()
            && proved_present.iter().all(|id| {
                census.get_entity(id).is_some_and(|entity| {
                    entity.entity_type.as_deref().is_some()
                        && entity.entity_type.as_deref() != Some("Person")
                })
            })
        {
            let mut sorted_present = proved_present.clone();
            sorted_present.sort();
            qualifying_suppression = Some((
                "speaker_type",
                format!("speaker_type:{}", sorted_present.join(",")),
            ));
        }

        // 4. Low prefix: tier 7 and every present score is finite and strictly < 35.0
        if qualifying_suppression.is_none()
            && observed_tier == 7
            && all_proved
            && !proved_present.is_empty()
            && proved_present.iter().all(|id| {
                candidates
                    .iter()
                    .find(|c| c.get("id").and_then(Value::as_str) == Some(id.as_str()))
                    .and_then(|c| c.get("score").and_then(Value::as_f64))
                    .is_some_and(|score| score.is_finite() && score < PREFIX_CUTOFF)
            })
        {
            let mut sorted_present = proved_present.clone();
            sorted_present.sort();
            qualifying_suppression = Some((
                "low_prefix",
                format!("low_prefix:{}", sorted_present.join(",")),
            ));
        }
    }

    let review_obj = row.get("review").and_then(Value::as_object);
    let current_suppression = review_obj
        .and_then(|r| r.get("suppression"))
        .and_then(Value::as_object)
        .and_then(|s| {
            let reason = s.get("reason").and_then(Value::as_str)?;
            let key = s.get("evidence_key").and_then(Value::as_str)?;
            Some((reason.to_owned(), key.to_owned()))
        });

    let released_keys: HashSet<(String, String)> = review_obj
        .and_then(|r| r.get("released"))
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|item| {
                    let obj = item.as_object()?;
                    let reason = obj.get("reason").and_then(Value::as_str)?;
                    let key = obj.get("evidence_key").and_then(Value::as_str)?;
                    Some((reason.to_owned(), key.to_owned()))
                })
                .collect()
        })
        .unwrap_or_default();

    let mut suppression_active = false;

    if let Some((curr_reason, curr_key)) = current_suppression {
        if let Some((qual_reason, qual_key)) = &qualifying_suppression
            && qual_reason == &curr_reason
            && qual_key == &curr_key
        {
            // Unchanged qualifying evidence keeps the existing suppression.
            suppression_active = true;
        } else {
            // Predicate is false or evidence changed: clear suppression and record reopen.
            let review_mut = ensure_review_object(row, false);
            review_mut.insert("suppression".to_owned(), Value::Null);
            let history = review_mut
                .entry("history".to_owned())
                .or_insert_with(|| Value::Array(Vec::new()))
                .as_array_mut()
                .expect("history array");
            history.push(json!({
                "action": "reopen",
                "reason": curr_reason,
                "at": now,
                "evidence_key": curr_key,
            }));

            // Check if a new qualifying suppression applies and is not released
            if let Some((new_reason, new_key)) = qualifying_suppression {
                if !released_keys.contains(&(new_reason.to_owned(), new_key.clone())) {
                    let review_mut = ensure_review_object(row, false);
                    review_mut.insert(
                        "suppression".to_owned(),
                        json!({
                            "reason": new_reason,
                            "at": now,
                            "evidence_key": new_key,
                        }),
                    );
                    let history = review_mut
                        .entry("history".to_owned())
                        .or_insert_with(|| Value::Array(Vec::new()))
                        .as_array_mut()
                        .expect("history array");
                    history.push(json!({
                        "action": "set_aside",
                        "reason": new_reason,
                        "at": now,
                        "evidence_key": new_key,
                    }));
                    suppression_active = true;
                }
            }
        }
    } else if let Some((new_reason, new_key)) = qualifying_suppression {
        if !released_keys.contains(&(new_reason.to_owned(), new_key.clone())) {
            let review_mut = ensure_review_object(row, false);
            review_mut.insert(
                "suppression".to_owned(),
                json!({
                    "reason": new_reason,
                    "at": now,
                    "evidence_key": new_key,
                }),
            );
            let history = review_mut
                .entry("history".to_owned())
                .or_insert_with(|| Value::Array(Vec::new()))
                .as_array_mut()
                .expect("history array");
            history.push(json!({
                "action": "set_aside",
                "reason": new_reason,
                "at": now,
                "evidence_key": new_key,
            }));
            suppression_active = true;
        }
    }

    // Typo auto-acceptance runs only when no suppression is active
    if !suppression_active
        && typo_acceptance_enabled
        && observed_tier == 8
        && census.complete
        && all_proved
        && proved_present.len() == 1
    {
        let candidate_id = &proved_present[0];
        let score = candidates
            .iter()
            .find(|c| c.get("id").and_then(Value::as_str) == Some(candidate_id.as_str()))
            .and_then(|c| c.get("score").and_then(Value::as_f64))
            .unwrap_or(0.0);

        let prior_choices = row
            .get("audit")
            .and_then(Value::as_object)
            .and_then(|a| a.get("prior_choices"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();

        let candidate_entity = census.get_entity(candidate_id);
        let candidate_blocked = candidate_entity.is_some_and(|e| e.blocked);
        let speaker_eligible = if is_speaker_only {
            candidate_entity.is_some_and(|e| e.entity_type.as_deref() == Some("Person") && !e.blocked)
        } else {
            !candidate_blocked
        };

        let typo_key = format!("typo:{candidate_id}:{normalized_query}");

        if score.is_finite()
            && score >= TYPO_FLOOR
            && prior_choices.is_empty()
            && !released_keys.contains(&("typo".to_owned(), typo_key.clone()))
            && !candidate_blocked
            && speaker_eligible
        {
            // Typo fold guard: check if any other proved entity matches normalized_query
            let fold_collision = census.entities.values().any(|entity| {
                if &entity.id == candidate_id {
                    return false;
                }
                solstone_core_entity_matching::normalize_resolution_query(&entity.name)
                    == normalized_query
                    || entity.aka.iter().any(|aka| {
                        solstone_core_entity_matching::normalize_resolution_query(aka)
                            == normalized_query
                    })
                    || entity.emails.iter().any(|email| {
                        solstone_core_entity_matching::normalize_resolution_query(email)
                            == normalized_query
                    })
            });

            if !fold_collision {
                row.insert("status".to_owned(), Value::String("resolved".to_owned()));
                row.insert(
                    "resolved_entity_id".to_owned(),
                    Value::String(candidate_id.clone()),
                );
                row.insert("resolved_at".to_owned(), Value::String(now.to_owned()));

                let review_mut = ensure_review_object(row, false);
                review_mut.insert(
                    "choice".to_owned(),
                    json!({
                        "kind": "automatic",
                        "reason": "typo",
                        "entity_id": candidate_id,
                        "at": now,
                    }),
                );
                let history = review_mut
                    .entry("history".to_owned())
                    .or_insert_with(|| Value::Array(Vec::new()))
                    .as_array_mut()
                    .expect("history array");
                history.push(json!({
                    "action": "auto_resolve",
                    "reason": "typo",
                    "at": now,
                    "evidence_key": typo_key,
                }));
            }
        }
    }
}

/// Apply automatic review policy to one merge candidate row.
pub fn apply_merge_candidate_review_policy(
    row: &mut Map<String, Value>,
    census: &IdentityCensus,
    now: &str,
) {
    let status = row.get("status").and_then(Value::as_str).unwrap_or("open");
    if status == "accepted" || status == "dismissed" {
        return;
    }

    let target_slug = row
        .get("target_slug")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    if target_slug.is_empty() {
        return;
    }

    let is_stale = census.is_proved_absent(&target_slug);
    let stale_key = format!("stale:{target_slug}");

    let review_obj = row.get("review").and_then(Value::as_object);
    let current_suppression = review_obj
        .and_then(|r| r.get("suppression"))
        .and_then(Value::as_object)
        .and_then(|s| {
            let reason = s.get("reason").and_then(Value::as_str)?;
            let key = s.get("evidence_key").and_then(Value::as_str)?;
            Some((reason.to_owned(), key.to_owned()))
        });

    let released_keys: HashSet<(String, String)> = review_obj
        .and_then(|r| r.get("released"))
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|item| {
                    let obj = item.as_object()?;
                    let reason = obj.get("reason").and_then(Value::as_str)?;
                    let key = obj.get("evidence_key").and_then(Value::as_str)?;
                    Some((reason.to_owned(), key.to_owned()))
                })
                .collect()
        })
        .unwrap_or_default();

    if is_stale {
        if let Some((curr_reason, curr_key)) = &current_suppression
            && curr_reason == "stale"
            && curr_key == &stale_key
        {
            // Unchanged
        } else if !released_keys.contains(&("stale".to_owned(), stale_key.clone())) {
            let review_mut = ensure_review_object(row, true);
            review_mut.insert(
                "suppression".to_owned(),
                json!({
                    "reason": "stale",
                    "at": now,
                    "evidence_key": stale_key,
                }),
            );
            let history = review_mut
                .entry("history".to_owned())
                .or_insert_with(|| Value::Array(Vec::new()))
                .as_array_mut()
                .expect("history array");
            history.push(json!({
                "action": "set_aside",
                "reason": "stale",
                "at": now,
                "evidence_key": stale_key,
            }));
        }
    } else if let Some((curr_reason, curr_key)) = current_suppression {
        let review_mut = ensure_review_object(row, true);
        review_mut.insert("suppression".to_owned(), Value::Null);
        let history = review_mut
            .entry("history".to_owned())
            .or_insert_with(|| Value::Array(Vec::new()))
            .as_array_mut()
            .expect("history array");
        history.push(json!({
            "action": "reopen",
            "reason": curr_reason,
            "at": now,
            "evidence_key": curr_key,
        }));
    }
}

/// Compute the canonical SHA-256 group revision across member ambiguity rows.
pub fn ambiguity_group_revision(
    candidate_ids: &[String],
    member_rows: &[&Map<String, Value>],
) -> String {
    let mut sorted_candidates = candidate_ids.to_vec();
    sorted_candidates.sort();

    let mut sorted_members = member_rows.to_vec();
    sorted_members.sort_by(|left, right| {
        let left_id = left.get("ambiguity_id").and_then(Value::as_str).unwrap_or_default();
        let right_id = right.get("ambiguity_id").and_then(Value::as_str).unwrap_or_default();
        left_id.cmp(right_id)
    });

    let mut buffer = String::new();
    buffer.push_str(&sorted_candidates.join(","));
    buffer.push('\n');

    for member in sorted_members {
        let ambiguity_id = member
            .get("ambiguity_id")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let count = member
            .get("occurrence_count")
            .and_then(Value::as_i64)
            .unwrap_or(1);
        let mut origin_keys: Vec<String> = member
            .get("origin_keys")
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        origin_keys.sort();

        buffer.push_str(&format!(
            "{}|{}|{}\n",
            ambiguity_id,
            count,
            origin_keys.join(",")
        ));
    }

    let digest = Sha256::digest(buffer.as_bytes());
    format!("{digest:x}")
}

/// Parameters for resolving a group of ambiguities in a single atomic transaction.
#[derive(Debug, Clone)]
pub struct AmbiguityGroupResolveRequest {
    pub entity_id: String,
    pub member_ids: Vec<String>,
    pub revision: String,
    pub eligible_by_member: HashMap<String, Vec<AmbiguityChoiceEntity>>,
}

/// Execute group resolution across selected members of an ambiguity card.
pub fn resolve_ambiguity_group(
    journal_root: &Path,
    request: &AmbiguityGroupResolveRequest,
) -> Result<Vec<Value>, EntityWriteError> {
    let _trust = hold_entity_trust_lock(journal_root)?;
    if request.member_ids.is_empty() {
        return Err(EntityWriteError::AmbiguityRowInvalid {
            detail: "no member ids provided for group resolve".to_owned(),
        });
    }

    let result = mutate_ambiguities(journal_root, |rows| {
        let now = ambiguity_now_iso();

        // 1. Locate all open unsuppressed rows sharing the target candidate ID set
        let first_member_id = &request.member_ids[0];
        let first_row = rows
            .iter()
            .filter_map(Value::as_object)
            .find(|r| r.get("ambiguity_id").and_then(Value::as_str) == Some(first_member_id))
            .ok_or_else(|| EntityWriteError::AmbiguityRowInvalid {
                detail: format!("member {first_member_id} not found"),
            })?;

        let candidate_ids: BTreeSet<String> = first_row
            .get("ranked_candidates")
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(|c| c.get("id").and_then(Value::as_str).map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();

        if !candidate_ids.contains(&request.entity_id) {
            return Err(EntityWriteError::AmbiguityChoiceInvalid {
                entity_id: request.entity_id.clone(),
                detail: "chosen entity is not in the candidate set".to_owned(),
            });
        }

        let mut group_members: Vec<&Map<String, Value>> = Vec::new();
        for row in rows.iter().filter_map(Value::as_object) {
            let status = row.get("status").and_then(Value::as_str).unwrap_or("open");
            if status != "open" {
                continue;
            }
            if let Some(review) = row.get("review").and_then(Value::as_object)
                && let Some(suppression) = review.get("suppression")
                && !suppression.is_null()
            {
                continue;
            }
            let row_candidates: BTreeSet<String> = row
                .get("ranked_candidates")
                .and_then(Value::as_array)
                .map(|arr| {
                    arr.iter()
                        .filter_map(|c| c.get("id").and_then(Value::as_str).map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default();
            if row_candidates == candidate_ids {
                group_members.push(row);
            }
        }

        let candidate_id_list: Vec<String> = candidate_ids.into_iter().collect();
        let expected_revision = ambiguity_group_revision(&candidate_id_list, &group_members);
        if expected_revision != request.revision {
            return Err(EntityWriteError::AmbiguityRowInvalid {
                detail: "group revision mismatch".to_owned(),
            });
        }

        let census = super::census::scan_identity_census(journal_root).map_err(EntityWriteError::Read)?;
        if !census.complete {
            return Err(EntityWriteError::CensusIncomplete {
                detail: "identity census is incomplete for group resolve".to_owned(),
            });
        }
        if census.is_proved_absent(&request.entity_id) {
            return Err(EntityWriteError::AmbiguityChoiceInvalid {
                entity_id: request.entity_id.clone(),
                detail: "chosen entity is absent".to_owned(),
            });
        }
        let chosen_entity = census.get_entity(&request.entity_id);

        // Verify all selected members exist in the group and are eligible
        for member_id in &request.member_ids {
            let member_row = group_members
                .iter()
                .find(|r| r.get("ambiguity_id").and_then(Value::as_str) == Some(member_id))
                .ok_or_else(|| EntityWriteError::AmbiguityRowInvalid {
                    detail: format!("member {member_id} is not in the card group"),
                })?;

            let is_speaker_only = member_row
                .get("origins")
                .and_then(Value::as_array)
                .map(|origins| {
                    !origins.is_empty()
                        && origins.iter().all(|o| {
                            o.as_object()
                                .and_then(|obj| obj.get("lane"))
                                .and_then(Value::as_str)
                                .is_some_and(|lane| {
                                    lane == "apps.speakers.attribution"
                                        || lane == "talent.speaker_attribution"
                                })
                        })
                })
                .unwrap_or(false);
            if is_speaker_only {
                let is_eligible_person = chosen_entity.is_some_and(|e| {
                    e.entity_type.as_deref() == Some("Person") && !e.blocked
                });
                if !is_eligible_person {
                    return Err(EntityWriteError::AmbiguityChoiceInvalid {
                        entity_id: request.entity_id.clone(),
                        detail: format!(
                            "chosen entity {} is not an eligible Person for speaker-only member {member_id}",
                            request.entity_id
                        ),
                    });
                }
            }

            let eligible = request
                .eligible_by_member
                .get(member_id)
                .ok_or_else(|| EntityWriteError::AmbiguityRowInvalid {
                    detail: format!("eligibility missing for member {member_id}"),
                })?;
            let selected = eligible
                .iter()
                .find(|e| e.id == request.entity_id)
                .ok_or_else(|| EntityWriteError::AmbiguityChoiceInvalid {
                    entity_id: request.entity_id.clone(),
                    detail: format!("entity {} ineligible for member {member_id}", request.entity_id),
                })?;
            if selected.blocked {
                return Err(EntityWriteError::AmbiguityChoiceInvalid {
                    entity_id: request.entity_id.clone(),
                    detail: format!("entity {} is blocked for member {member_id}", request.entity_id),
                });
            }
        }

        // Apply resolution to selected members
        let mut resolved_rows = Vec::new();
        for member_id in &request.member_ids {
            let row = rows
                .iter_mut()
                .filter_map(Value::as_object_mut)
                .find(|r| r.get("ambiguity_id").and_then(Value::as_str) == Some(member_id))
                .expect("verified present");

            let previous_id = row
                .get("resolved_entity_id")
                .and_then(Value::as_str)
                .map(str::to_owned);
            if row.get("status").and_then(Value::as_str) == Some("resolved")
                && previous_id
                    .as_deref()
                    .is_some_and(|id| id != request.entity_id)
            {
                let previous_at = row
                    .get("resolved_at")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                let audit = row
                    .get_mut("audit")
                    .and_then(Value::as_object_mut)
                    .ok_or_else(|| EntityWriteError::AmbiguityRowInvalid {
                        detail: "ambiguity row has invalid audit".to_owned(),
                    })?;
                let priors = audit
                    .get_mut("prior_choices")
                    .and_then(Value::as_array_mut)
                    .ok_or_else(|| EntityWriteError::AmbiguityRowInvalid {
                        detail: "ambiguity row has invalid audit.prior_choices".to_owned(),
                    })?;
                priors.push(json!({
                    "resolved_entity_id": previous_id.unwrap(),
                    "resolved_at": previous_at,
                    "replaced_at": now.clone(),
                    "replaced_by_origin": {
                        "lane": "apps.entities.resolve_ambiguity_group",
                        "field": "entity_id",
                    }
                }));
            }

            row.insert("status".to_owned(), Value::String("resolved".to_owned()));
            row.insert(
                "resolved_entity_id".to_owned(),
                Value::String(request.entity_id.clone()),
            );
            row.insert("resolved_at".to_owned(), Value::String(now.clone()));

            if let Some(review) = row.get_mut("review").and_then(Value::as_object_mut) {
                review.insert("choice".to_owned(), Value::Null);
            }

            resolved_rows.push(Value::Object(row.clone()));
        }

        Ok(Value::Array(resolved_rows))
    })?;

    Ok(result.as_array().cloned().unwrap_or_default())
}

/// Restore an active suppression or undo an active automatic typo resolution.
pub fn restore_review(
    journal_root: &Path,
    target: &ReviewRestoreTarget,
) -> Result<Option<Value>, EntityWriteError> {
    let _trust = hold_entity_trust_lock(journal_root)?;
    let now = ambiguity_now_iso();

    match target {
        ReviewRestoreTarget::Ambiguity { ambiguity_id } => {
            let res = mutate_ambiguities(journal_root, |rows| {
                let Some(row) = rows
                    .iter_mut()
                    .filter_map(Value::as_object_mut)
                    .find(|r| r.get("ambiguity_id").and_then(Value::as_str) == Some(ambiguity_id))
                else {
                    return Ok(Value::Null);
                };

                let review = ensure_review_object(row, false);
                let choice = review
                    .get("choice")
                    .and_then(Value::as_object)
                    .cloned();

                if let Some(choice_obj) = choice {
                    let choice_entity_id = choice_obj.get("entity_id").and_then(Value::as_str);
                    let current_resolved = row.get("resolved_entity_id").and_then(Value::as_str);

                    if current_resolved == choice_entity_id {
                        row.insert("status".to_owned(), Value::String("open".to_owned()));
                        row.remove("resolved_entity_id");
                        row.remove("resolved_at");

                        let candidate_id = choice_entity_id.unwrap_or_default().to_owned();
                        let norm_query = row.get("normalized_query").and_then(Value::as_str).unwrap_or_default().to_owned();
                        let typo_key = format!("typo:{candidate_id}:{norm_query}");

                        let review_mut = ensure_review_object(row, false);
                        review_mut.insert("choice".to_owned(), Value::Null);

                        let history = review_mut
                            .entry("history".to_owned())
                            .or_insert_with(|| Value::Array(Vec::new()))
                            .as_array_mut()
                            .expect("history array");
                        history.push(json!({
                            "action": "undo_choice",
                            "reason": "typo",
                            "at": now.clone(),
                            "evidence_key": typo_key,
                        }));

                        let released = review_mut
                            .entry("released".to_owned())
                            .or_insert_with(|| Value::Array(Vec::new()))
                            .as_array_mut()
                            .expect("released array");
                        released.push(json!({
                            "reason": "typo",
                            "evidence_key": typo_key,
                        }));

                        return Ok(Value::Object(row.clone()));
                    }
                }

                // If suppression is active on an open row
                let status = row.get("status").and_then(Value::as_str).unwrap_or("open");
                if status == "open" {
                    let review_mut = ensure_review_object(row, false);
                    let suppression = review_mut
                        .get("suppression")
                        .and_then(Value::as_object)
                        .cloned();

                    if let Some(supp_obj) = suppression {
                        let reason = supp_obj.get("reason").and_then(Value::as_str).unwrap_or_default().to_owned();
                        let evidence_key = supp_obj.get("evidence_key").and_then(Value::as_str).unwrap_or_default().to_owned();

                        review_mut.insert("suppression".to_owned(), Value::Null);

                        let history = review_mut
                            .entry("history".to_owned())
                            .or_insert_with(|| Value::Array(Vec::new()))
                            .as_array_mut()
                            .expect("history array");
                        history.push(json!({
                            "action": "undo",
                            "reason": reason,
                            "at": now.clone(),
                            "evidence_key": evidence_key,
                        }));

                        let released = review_mut
                            .entry("released".to_owned())
                            .or_insert_with(|| Value::Array(Vec::new()))
                            .as_array_mut()
                            .expect("released array");
                        released.push(json!({
                            "reason": reason,
                            "evidence_key": evidence_key,
                        }));

                        return Ok(Value::Object(row.clone()));
                    }
                }

                Ok(Value::Object(row.clone()))
            })?;
            if res.is_null() {
                Ok(None)
            } else {
                Ok(Some(res))
            }
        }
        ReviewRestoreTarget::MergeCandidate {
            facet,
            source_slug,
            target_slug,
        } => {
            let res = super::review_candidates::mutate_candidates(journal_root, |rows| {
                let target_key = format!("{facet}|{source_slug}|{target_slug}");
                let Some(row) = rows
                    .iter_mut()
                    .filter_map(Value::as_object_mut)
                    .find(|r| {
                        let f = r.get("facet").and_then(Value::as_str).unwrap_or_default();
                        let s = r.get("source_slug").and_then(Value::as_str).unwrap_or_default();
                        let t = r.get("target_slug").and_then(Value::as_str).unwrap_or_default();
                        format!("{f}|{s}|{t}") == target_key
                    })
                else {
                    return Ok(Value::Null);
                };

                let status = row.get("status").and_then(Value::as_str).unwrap_or("open");
                if status == "open" {
                    let review_mut = ensure_review_object(row, true);
                    let suppression = review_mut
                        .get("suppression")
                        .and_then(Value::as_object)
                        .cloned();

                    if let Some(supp_obj) = suppression {
                        let reason = supp_obj.get("reason").and_then(Value::as_str).unwrap_or_default().to_owned();
                        let evidence_key = supp_obj.get("evidence_key").and_then(Value::as_str).unwrap_or_default().to_owned();

                        review_mut.insert("suppression".to_owned(), Value::Null);

                        let history = review_mut
                            .entry("history".to_owned())
                            .or_insert_with(|| Value::Array(Vec::new()))
                            .as_array_mut()
                            .expect("history array");
                        history.push(json!({
                            "action": "undo",
                            "reason": reason,
                            "at": now.clone(),
                            "evidence_key": evidence_key,
                        }));

                        let released = review_mut
                            .entry("released".to_owned())
                            .or_insert_with(|| Value::Array(Vec::new()))
                            .as_array_mut()
                            .expect("released array");
                        released.push(json!({
                            "reason": reason,
                            "evidence_key": evidence_key,
                        }));

                        return Ok(Value::Object(row.clone()));
                    }
                }

                Ok(Value::Object(row.clone()))
            })
            .map_err(|err| match err {
                super::review_candidates::EntityReviewCandidateError::TrustLock(e) => {
                    EntityWriteError::TrustLock(e)
                }
                super::review_candidates::EntityReviewCandidateError::Store(e) => {
                    EntityWriteError::Read(e)
                }
                super::review_candidates::EntityReviewCandidateError::Lock(e) => {
                    EntityWriteError::AmbiguityLock(e)
                }
                super::review_candidates::EntityReviewCandidateError::Write(e) => {
                    EntityWriteError::AmbiguityWrite(e)
                }
            })?;
            if res.is_null() {
                Ok(None)
            } else {
                Ok(Some(res))
            }
        }
    }
}

/// Target selector for restoring a suppressed or auto-resolved review item.
#[derive(Debug, Clone)]
pub enum ReviewRestoreTarget {
    Ambiguity {
        ambiguity_id: String,
    },
    MergeCandidate {
        facet: String,
        source_slug: String,
        target_slug: String,
    },
}

/// One-shot startup sweep executing review policy across ambiguities and merge candidates.
pub fn sweep_entity_review_policy(journal_root: &Path) -> Result<(), EntityWriteError> {
    let receipt_path = match contained_path(journal_root, REVIEW_SWEEP_RECEIPT_RELATIVE_PATH) {
        Ok(p) => p,
        Err(err) => return Err(EntityWriteError::Read(err.into())),
    };

    if receipt_path.exists() {
        if let Ok(value) = read_json::<Value>(&receipt_path, Value::Null, MalformedPolicy::Raise) {
            if value.get("policy_version").and_then(Value::as_u64)
                == Some(ENTITY_REVIEW_POLICY_VERSION)
            {
                return Ok(());
            }
        }
    }

    let census = scan_identity_census(journal_root).map_err(EntityWriteError::Read)?;
    if !census.complete {
        return Err(EntityWriteError::CensusIncomplete {
            detail: "identity census is incomplete".to_owned(),
        });
    }

    let config_read = read_journal_config(journal_root).ok();
    let typo_enabled = entity_tier8_typo_acceptance_enabled(
        config_read.as_ref().and_then(|r| r.config.as_ref()),
    );

    let now = ambiguity_now_iso();
    let _trust = hold_entity_trust_lock(journal_root)?;

    mutate_ambiguities(journal_root, |rows| {
        for row in rows.iter_mut().filter_map(Value::as_object_mut) {
            apply_ambiguity_review_policy(row, &census, typo_enabled, &now);
        }
        Ok(Value::Null)
    })?;

    super::review_candidates::mutate_candidates(journal_root, |rows| {
        for row in rows.iter_mut().filter_map(Value::as_object_mut) {
            apply_merge_candidate_review_policy(row, &census, &now);
        }
        Ok(Value::Null)
    })
    .map_err(|err| match err {
        super::review_candidates::EntityReviewCandidateError::TrustLock(e) => {
            EntityWriteError::TrustLock(e)
        }
        super::review_candidates::EntityReviewCandidateError::Store(e) => {
            EntityWriteError::Read(e)
        }
        super::review_candidates::EntityReviewCandidateError::Lock(e) => {
            EntityWriteError::AmbiguityLock(e)
        }
        super::review_candidates::EntityReviewCandidateError::Write(e) => {
            EntityWriteError::AmbiguityWrite(e)
        }
    })?;

    if let Some(parent) = receipt_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let receipt_json = json!({
        "schema_version": 1,
        "policy_version": ENTITY_REVIEW_POLICY_VERSION,
        "completed_at": now,
    });
    write_json(
        &receipt_path,
        &receipt_json,
        JsonWriteOptions {
            mode: Some(0o600),
            indent: Some(2),
            sort_keys: true,
        },
    )
    .map_err(EntityWriteError::AmbiguityWrite)?;

    Ok(())
}
