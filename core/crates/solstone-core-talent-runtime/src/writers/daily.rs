// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use solstone_core_journal_io::{
    AtomicWriteOptions, DailyUnitAuthority, LockOptions, atomic_replace, hold_lock,
};

use super::WriteIntent;
use crate::contract::{CommitDisposition, CommitPlan};
use crate::{ExecutionContext, PreparedTalent, StageError};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PreparedDailyPublication {
    pub actions: Vec<PreparedDailyAction>,
    pub no_output: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "owner", rename_all = "snake_case")]
pub enum PreparedDailyAction {
    Observation {
        batch: solstone_core_facets::PreparedObservationBatch,
    },
    Anticipation {
        batch: solstone_core_facets::PreparedAnticipationBatch,
    },
    Newsletter {
        batch: solstone_core_facets::PreparedNewsReplacement,
    },
    Identity {
        facet: String,
        facet_id: String,
        change: solstone_core_entity::PreparedIdentityChange,
    },
    Attachment {
        change: solstone_core_facets::PreparedReviewAttachment,
    },
    Aliases {
        facet: String,
        facet_id: String,
        change: solstone_core_entity::PreparedIdentityChange,
    },
    MergeProposals {
        facet: String,
        facet_id: String,
        batch: solstone_core_entity::PreparedMergeProposals,
    },
    DailyTime {
        batch: solstone_core_system::schedule::PreparedDailyTime,
    },
    Output {
        path: String,
        facet_identity: Option<(String, String)>,
        before: Option<Vec<u8>>,
        after: Vec<u8>,
    },
}

fn error(detail: impl Into<String>) -> StageError {
    let detail = detail.into();
    let phase = if detail.starts_with("conflict:") {
        "conflict"
    } else if detail.starts_with("validation:") {
        "parse"
    } else {
        "publication"
    };
    StageError::new(phase, "daily", "daily", detail)
}

fn read_optional(path: &Path) -> Result<Option<Vec<u8>>, String> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

fn relative_output(root: &Path, path: &Path) -> Result<String, String> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| "daily output is outside journal")?;
    if relative
        .components()
        .any(|part| !matches!(part, std::path::Component::Normal(_)))
        || relative.as_os_str().is_empty()
    {
        return Err("invalid daily output path".into());
    }
    Ok(relative.to_string_lossy().replace('\\', "/"))
}

fn action_facet(action: &PreparedDailyAction) -> Option<(&str, &str)> {
    match action {
        PreparedDailyAction::Observation { batch } => Some((&batch.facet, &batch.facet_id)),
        PreparedDailyAction::Anticipation { batch } => Some((&batch.facet, &batch.facet_id)),
        PreparedDailyAction::Newsletter { batch } => Some((&batch.facet, &batch.facet_id)),
        PreparedDailyAction::Attachment { change } => Some((&change.facet, &change.facet_id)),
        PreparedDailyAction::Identity {
            facet, facet_id, ..
        }
        | PreparedDailyAction::Aliases {
            facet, facet_id, ..
        }
        | PreparedDailyAction::MergeProposals {
            facet, facet_id, ..
        } => Some((facet, facet_id)),
        PreparedDailyAction::Output { facet_identity, .. } => facet_identity
            .as_ref()
            .map(|(facet, id)| (facet.as_str(), id.as_str())),
        PreparedDailyAction::DailyTime { .. } => None,
    }
}

fn verify_frozen_facet(
    action: &PreparedDailyAction,
    prepared: &PreparedTalent,
) -> Result<(), String> {
    if let Some((facet, id)) = action_facet(action) {
        let expected = prepared
            .config
            .get("_daily_facet_ids")
            .and_then(|map| map.get(facet))
            .and_then(Value::as_str)
            .ok_or("missing frozen facet identity")?;
        if id != expected {
            return Err("conflict: owning facet changed after prompt preparation".into());
        }
    }
    Ok(())
}

pub fn prepare_output_action(
    root: &Path,
    path: &Path,
    after: Vec<u8>,
) -> Result<PreparedDailyAction, String> {
    let path_string = relative_output(root, path)?;
    let facet_identity = if let Some(rest) = path_string.strip_prefix("facets/") {
        let facet = rest.split('/').next().ok_or("invalid facet output path")?;
        let _guard =
            solstone_core_facets::hold_facet_trust_lock(root).map_err(|e| e.to_string())?;
        Some((
            facet.to_owned(),
            solstone_core_facets::facet_write_identity(root, facet)?,
        ))
    } else {
        None
    };
    Ok(PreparedDailyAction::Output {
        path: path_string,
        facet_identity,
        before: read_optional(path)?,
        after,
    })
}

pub(crate) fn prepare_frozen_output_action(
    root: &Path,
    path: &Path,
    after: Vec<u8>,
    prepared: &PreparedTalent,
) -> Result<PreparedDailyAction, String> {
    let action = prepare_output_action(root, path, after)?;
    let PreparedDailyAction::Output { path, before, .. } = &action else {
        unreachable!()
    };
    verify_artifact_before(prepared, path, before.as_deref())?;
    verify_frozen_facet(&action, prepared)?;
    Ok(action)
}

fn verify_artifact_before(
    prepared: &PreparedTalent,
    path: &str,
    actual: Option<&[u8]>,
) -> Result<(), String> {
    let expected = prepared
        .config
        .get("_daily_artifact_before")
        .and_then(Value::as_object)
        .and_then(|map| map.get(path))
        .ok_or("missing frozen required-artifact snapshot")?;
    let expected: Option<Vec<u8>> = serde_json::from_value(expected.clone())
        .map_err(|e| format!("invalid frozen artifact snapshot: {e}"))?;
    if expected.as_deref() != actual {
        return Err("conflict: required artifact changed after prompt preparation".into());
    }
    Ok(())
}

pub fn prepare_daily_output(
    prepared: &PreparedTalent,
    output: &str,
    context: &ExecutionContext,
) -> Result<PreparedDailyPublication, StageError> {
    let Some(path) = prepared.config.get("output_path").and_then(Value::as_str) else {
        return Ok(PreparedDailyPublication {
            actions: Vec::new(),
            no_output: true,
        });
    };
    let action = prepare_frozen_output_action(
        &context.journal,
        Path::new(path),
        output.as_bytes().to_vec(),
        prepared,
    )
    .map_err(error)?;
    Ok(PreparedDailyPublication {
        actions: vec![action],
        no_output: false,
    })
}

fn validate_model_intent(intent: &WriteIntent) -> Result<(), StageError> {
    let (output, shape) = match intent {
        WriteIntent::EntitySuggest { output, .. } => (output, "suggest"),
        WriteIntent::EntityObserver { output, .. } => (output, "observer"),
        WriteIntent::EntitiesReview { output, .. } => (output, "review"),
        WriteIntent::Schedule { output, .. } => (output, "schedule"),
        WriteIntent::DailySchedule { output, .. } => (output, "maintenance"),
        _ => return Ok(()),
    };
    let invalid = |message: &str| error(format!("validation: {message}"));
    let value: Value = serde_json::from_str(output)
        .map_err(|e| error(format!("validation: invalid {shape} JSON: {e}")))?;
    match shape {
        "suggest" => {
            if !value.get("entities").is_some_and(Value::is_array) {
                return Err(invalid("suggest entities must be an array"));
            }
        }
        "observer" => {
            if !value.get("entities").is_some_and(Value::is_array) {
                return Err(invalid("observer entities must be an array"));
            }
        }
        "review" => {
            let promotions = value
                .get("promotions")
                .and_then(Value::as_array)
                .ok_or_else(|| invalid("review promotions must be an array"))?;
            if !value.get("merges").is_some_and(Value::is_array) {
                return Err(invalid("review merges must be an array"));
            }
            if promotions.iter().any(|row| {
                row.get("promote").and_then(Value::as_bool) == Some(true)
                    && !row.get("aliases").is_some_and(Value::is_array)
            }) {
                return Err(invalid(
                    "promoted review candidate must include aliases array",
                ));
            }
        }
        "maintenance" => {
            let primary = value
                .get("primary")
                .and_then(Value::as_str)
                .ok_or_else(|| invalid("maintenance primary is missing"))?;
            if chrono::NaiveTime::parse_from_str(primary, "%H:%M").is_err() {
                return Err(invalid("maintenance primary must be HH:MM"));
            }
        }
        "schedule" => {
            let _ = value
                .as_array()
                .or_else(|| value.get("events").and_then(Value::as_array))
                .ok_or_else(|| invalid("schedule events must be an array"))?;
            let WriteIntent::Schedule { day, .. } = intent else {
                unreachable!()
            };
            chrono::NaiveDate::parse_from_str(day, "%Y%m%d")
                .map_err(|_| invalid("invalid schedule day"))?;
        }
        _ => unreachable!(),
    }
    Ok(())
}

pub fn prepare_daily_publication(
    plan: CommitPlan,
    prepared: &PreparedTalent,
    context: &ExecutionContext,
) -> Result<PreparedDailyPublication, StageError> {
    let root = &context.journal;
    let mut actions = Vec::new();
    if let CommitPlan::Write(intent) = &plan {
        validate_model_intent(intent)?;
    }
    match plan {
        CommitPlan::NoOutput => {}
        CommitPlan::Write(WriteIntent::EntitySuggest { output, facet, day }) => {
            let path = root
                .join("facets")
                .join(&facet)
                .join("entities")
                .join(format!("{day}_observer_suggestions.json"));
            actions.push(
                prepare_frozen_output_action(
                    root,
                    &path,
                    format!("{output}\n").into_bytes(),
                    prepared,
                )
                .map_err(error)?,
            );
        }
        CommitPlan::Write(WriteIntent::EntityObserver {
            output,
            facet,
            day,
            served_ids,
            shown_observation_ids,
        }) => {
            let _trust = solstone_core_facets::hold_facet_trust_lock(root)
                .map_err(|e| error(e.to_string()))?;
            let path = root
                .join("facets")
                .join(&facet)
                .join("entities")
                .join(format!("{day}_observer_outcome.json"));
            // Owner drift must be classified before retryable model-reference errors.
            prepare_frozen_output_action(root, &path, Vec::new(), prepared).map_err(error)?;
            let (batches, outcome) = crate::entities::observer::prepare_publication(
                root,
                &output,
                &facet,
                &day,
                &served_ids,
                &shown_observation_ids,
                prepared,
            )
            .map_err(error)?;
            actions.extend(
                batches
                    .into_iter()
                    .map(|batch| PreparedDailyAction::Observation { batch }),
            );
            actions.push(
                prepare_frozen_output_action(
                    root,
                    &path,
                    format!("{outcome}\n").into_bytes(),
                    prepared,
                )
                .map_err(error)?,
            );
        }
        CommitPlan::Write(WriteIntent::Schedule { output, day }) => {
            let frozen = prepared
                .config
                .get("_daily_calendar_before")
                .and_then(Value::as_object)
                .ok_or_else(|| error("missing frozen calendar snapshots"))?;
            for batch in crate::schedule::prepare_publication(root, &output, &day).map_err(error)? {
                let expected = frozen
                    .get(&format!("{}/{}", batch.facet, batch.day))
                    .unwrap_or(&Value::Null);
                let expected = match expected {
                    Value::Null => None,
                    Value::String(text) => Some(text.as_str()),
                    _ => return Err(error("invalid frozen calendar snapshot")),
                };
                if expected != batch.before.as_deref() {
                    return Err(error("conflict: calendar changed after prompt preparation"));
                }
                actions.push(PreparedDailyAction::Anticipation { batch });
            }
        }
        CommitPlan::Write(WriteIntent::FacetNewsletter { output, facet, day }) => {
            let content = output.trim();
            if !content.is_empty() && content != "No activity" {
                let batch = solstone_core_facets::prepare_news_replacement(
                    root,
                    &facet,
                    &format!("{day}.md"),
                    content,
                )
                .map_err(error)?;
                verify_artifact_before(
                    prepared,
                    &format!("facets/{}/news/{}", batch.facet, batch.relative_path),
                    batch.before.as_ref().map(|s| s.as_bytes()),
                )
                .map_err(error)?;
                actions.push(PreparedDailyAction::Newsletter { batch });
            }
        }
        CommitPlan::Write(WriteIntent::DailySchedule {
            output,
            output_path,
        }) => {
            let value: Value = serde_json::from_str(&output)
                .map_err(|e| error(format!("invalid maintenance output: {e}")))?;
            let primary = value
                .get("primary")
                .and_then(Value::as_str)
                .ok_or_else(|| error("maintenance output lacks primary"))?;
            let batch = solstone_core_system::schedule::prepare_daily_time(
                &root.join("config/schedules.json"),
                primary,
            )
            .map_err(error)?;
            let expected = prepared
                .config
                .get("_daily_time_before")
                .ok_or_else(|| error("missing frozen daily-time snapshot"))?;
            if batch.before.as_ref().unwrap_or(&Value::Null) != expected {
                return Err(error(
                    "conflict: daily time changed after prompt preparation",
                ));
            }
            actions.push(PreparedDailyAction::DailyTime { batch });
            if let Some(path) = output_path {
                actions.push(
                    prepare_frozen_output_action(
                        root,
                        &PathBuf::from(path),
                        output.into_bytes(),
                        prepared,
                    )
                    .map_err(error)?,
                );
            }
        }
        CommitPlan::Write(WriteIntent::EntitiesReview { output, facet, day }) => {
            let identity = solstone_core_journal_io::DailyUnitIdentity::new(
                &day,
                "entities:entities_review",
                Some(facet.clone()),
            );
            actions.extend(
                crate::entities::review::prepare_publication(root, &output, &facet, &day, prepared)
                    .map_err(|e| review_error(&identity, e))?,
            );
        }
        CommitPlan::Write(_) => {
            return Err(error(format!(
                "unsupported daily write contract for {}",
                prepared.name
            )));
        }
    }
    let is_review = prepared.name == "entities:entities_review";
    let review_identity = is_review.then(|| {
        let facet = prepared
            .config
            .get("facet")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        let day = prepared
            .config
            .get("day")
            .and_then(Value::as_str)
            .unwrap_or_default();
        solstone_core_journal_io::DailyUnitIdentity::new(day, "entities:entities_review", facet)
    });
    for action in &actions {
        verify_frozen_facet(action, prepared).map_err(|e| {
            if let Some(id) = &review_identity {
                review_error(id, e)
            } else {
                error(e)
            }
        })?;
    }
    Ok(PreparedDailyPublication {
        no_output: actions.is_empty(),
        actions,
    })
}

/// Required replacement files are checked when deciding currentness. Domain
/// writes deliberately have only historical action receipts.
pub fn required_artifact_receipts(publication: &PreparedDailyPublication) -> Vec<Value> {
    publication
        .actions
        .iter()
        .filter_map(|action| {
            let (path, bytes) = match action {
                PreparedDailyAction::Output { path, after, .. } => (path.clone(), after.as_slice()),
                PreparedDailyAction::Newsletter { batch } => (
                    format!("facets/{}/news/{}", batch.facet, batch.relative_path),
                    batch.after.as_bytes(),
                ),
                _ => return None,
            };
            Some(json!({
                "kind": "required_artifact",
                "path": path,
                "sha256": format!("{:x}", Sha256::digest(bytes))
            }))
        })
        .collect()
}

/// The caller checkpointed generated_result and this entire typed plan before
/// entry. Each action starts durably, commits under its owner's lock, then
/// checkpoints the receipt while both owner and unit authority remain held.
fn review_conflict_kind(detail: &str) -> Option<&'static str> {
    match detail {
        "conflict: promotion alias was claimed after preparation" => Some("alias_claimed"),
        "conflict: alias target is no longer attached" => Some("alias_target_detached"),
        "conflict: promotion owner state changed after prompt preparation" => {
            Some("promotion_owner_state")
        }
        "conflict: promoted identity moved after preparation" => Some("identity_moved"),
        "conflict: promoted identity changed after preparation" => Some("identity_changed"),
        "conflict: promotion identity disappeared" => Some("identity_disappeared"),
        "conflict: promotion identity blocked" => Some("identity_blocked"),
        "conflict: promotion relationship changed after preparation" => {
            Some("relationship_changed")
        }
        "conflict: merge proposal changed after prompt preparation" => {
            Some("merge_proposal_preparation")
        }
        "conflict: merge proposals changed after preparation" => Some("merge_proposals_changed"),
        "conflict: required output changed after preparation" => Some("output_artifact_changed"),
        "conflict: owning facet changed after prompt preparation" => Some("owning_facet_changed"),
        "conflict: required artifact changed after prompt preparation" => {
            Some("artifact_before_changed")
        }
        _ => None,
    }
}

fn review_error(
    identity: &solstone_core_journal_io::DailyUnitIdentity,
    detail: impl Into<String>,
) -> StageError {
    let detail = detail.into();
    if let Some(kind) = review_conflict_kind(&detail) {
        StageError::owner_conflict(identity, "daily_publication", kind, detail)
    } else if detail.starts_with("conflict:") {
        StageError::new(
            "unmapped_review_conflict",
            "daily_publication",
            &identity.name,
            format!("unmapped review owner conflict: {detail}"),
        )
    } else if detail.starts_with("validation:") {
        StageError::new("parse", "daily_publication", &identity.name, detail)
    } else {
        StageError::new("publication", "daily_publication", &identity.name, detail)
    }
}

pub fn publish_daily_publication(
    authority: &mut DailyUnitAuthority,
    token: &str,
    publication: &PreparedDailyPublication,
    context: &ExecutionContext,
) -> Result<CommitDisposition, StageError> {
    let record = authority
        .record()
        .cloned()
        .ok_or_else(|| error("missing publication record"))?;
    let is_review = record.identity.name == "entities:entities_review";
    let make_error = |detail: String| -> StageError {
        if is_review {
            review_error(&record.identity, detail)
        } else {
            error(detail)
        }
    };

    authority
        .require_token(token)
        .map_err(|e| make_error(e.to_string()))?;
    let serialized = serde_json::to_value(publication).map_err(|e| make_error(e.to_string()))?;
    if record.generated_result.is_none() || record.action_plan.as_ref() != Some(&serialized) {
        return Err(make_error(
            "publication requires the retained generated result and exact prepared plan".into(),
        ));
    }
    // Do not trust an in-memory assignment: checkpoint is required at entry,
    // before even the first start marker or owner action.
    authority
        .checkpoint()
        .map_err(|e| make_error(e.to_string()))?;
    for (index, action) in publication.actions.iter().enumerate() {
        authority
            .require_token(token)
            .map_err(|e| make_error(e.to_string()))?;
        let action_bytes = serde_json::to_vec(action).map_err(|e| make_error(e.to_string()))?;
        let action_id = format!("{index}:{:x}", Sha256::digest(&action_bytes));
        let prior =
            authority.record().unwrap().receipts.iter().find(|receipt| {
                receipt["kind"] == "owner_action" && receipt["action_id"] == action_id
            });
        if prior.is_some_and(|receipt| receipt["state"] == "committed") {
            continue;
        }
        let allow_before = prior.is_none();
        let is_review_action = matches!(
            action,
            PreparedDailyAction::Identity { .. }
                | PreparedDailyAction::Attachment { .. }
                | PreparedDailyAction::Aliases { .. }
                | PreparedDailyAction::MergeProposals { .. }
        ) || (matches!(action, PreparedDailyAction::Output { .. })
            && is_review);

        if !is_review_action && allow_before {
            authority.record_mut().as_mut().unwrap().receipts.push(json!({"kind":"owner_action","action_id":action_id,"token":token,"state":"started"}));
            authority
                .checkpoint()
                .map_err(|e| make_error(e.to_string()))?;
        }
        let auth_cell = std::cell::RefCell::new(&mut *authority);
        let start = || -> Result<(), String> {
            let mut auth = auth_cell.borrow_mut();
            auth.require_token(token).map_err(|e| e.to_string())?;
            let receipts = &mut auth
                .record_mut()
                .as_mut()
                .ok_or("missing publication record")?
                .receipts;
            if !receipts
                .iter()
                .any(|item| item["kind"] == "owner_action" && item["action_id"] == action_id)
            {
                receipts.push(json!({
                    "kind": "owner_action",
                    "action_id": action_id,
                    "token": token,
                    "state": "started"
                }));
                auth.checkpoint().map_err(|e| e.to_string())?;
            }
            Ok(())
        };
        let receipt = || -> Result<(), String> {
            let mut auth = auth_cell.borrow_mut();
            auth.require_token(token).map_err(|e| e.to_string())?;
            let receipts = &mut auth
                .record_mut()
                .as_mut()
                .ok_or("missing publication record")?
                .receipts;
            let prior = receipts
                .iter_mut()
                .find(|item| item["kind"] == "owner_action" && item["action_id"] == action_id)
                .ok_or("missing action start receipt")?;
            prior["state"] = Value::String("committed".into());
            auth.checkpoint().map_err(|e| e.to_string())
        };
        let _facet_guard = if let Some((facet, id)) = action_facet(action) {
            let guard = solstone_core_facets::hold_facet_trust_lock(&context.journal)
                .map_err(|e| make_error(e.to_string()))?;
            solstone_core_facets::require_facet_write_identity(&context.journal, facet, id)
                .map_err(|_| {
                    if is_review {
                        make_error("conflict: owning facet changed after prompt preparation".into())
                    } else {
                        make_error("conflict: owning facet no longer exists".into())
                    }
                })?;
            Some(guard)
        } else {
            None
        };
        let result = match action {
            PreparedDailyAction::Observation { batch } => {
                solstone_core_facets::publish_observation_batch(
                    &context.journal,
                    batch,
                    allow_before,
                    receipt,
                )
            }
            PreparedDailyAction::Anticipation { batch } => {
                solstone_core_facets::publish_anticipation_batch(
                    &context.journal,
                    batch,
                    allow_before,
                    receipt,
                )
            }
            PreparedDailyAction::Newsletter { batch } => {
                solstone_core_facets::publish_news_replacement(
                    &context.journal,
                    batch,
                    allow_before,
                    receipt,
                )
            }
            PreparedDailyAction::Identity { change, .. } => {
                solstone_core_entity::publish_identity_change(
                    &context.journal,
                    change,
                    allow_before,
                    start,
                    receipt,
                )
            }
            PreparedDailyAction::Attachment { change } => {
                solstone_core_facets::publish_review_attachment(
                    &context.journal,
                    change,
                    allow_before,
                    start,
                    receipt,
                )
            }
            PreparedDailyAction::Aliases { facet, change, .. } => {
                solstone_core_facets::publish_review_aliases(
                    &context.journal,
                    facet,
                    change,
                    allow_before,
                    start,
                    receipt,
                )
            }
            PreparedDailyAction::MergeProposals { batch, .. } => {
                solstone_core_entity::publish_merge_proposals(
                    &context.journal,
                    batch,
                    allow_before,
                    start,
                    receipt,
                )
            }
            PreparedDailyAction::DailyTime { batch } => {
                solstone_core_system::schedule::publish_daily_time(
                    &context.journal.join("config/schedules.json"),
                    batch,
                    allow_before,
                    receipt,
                )
            }
            PreparedDailyAction::Output {
                path,
                before,
                after,
                ..
            } => publish_output(
                &context.journal,
                path,
                before,
                after,
                allow_before,
                start,
                receipt,
            ),
        };
        result.map_err(make_error)?;
    }
    Ok(if publication.no_output {
        CommitDisposition::CommittedNoOutput
    } else {
        CommitDisposition::Written
    })
}

fn publish_output(
    root: &Path,
    relative: &str,
    before: &Option<Vec<u8>>,
    after: &[u8],
    allow_before: bool,
    start: impl FnOnce() -> Result<(), String>,
    receipt: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    let path = root.join(relative);
    relative_output(root, &path)?;
    let _lock = hold_lock(&path, LockOptions::default()).map_err(|e| e.to_string())?;
    let current = read_optional(&path)?;
    if current.as_deref() != Some(after) {
        if !allow_before || &current != before {
            return Err("conflict: required output changed after preparation".into());
        }
        start()?;
        atomic_replace(&path, after, AtomicWriteOptions { mode: Some(0o600) })
            .map_err(|e| e.to_string())?;
    } else {
        start()?;
    }
    receipt()
}

#[cfg(test)]
mod tests {
    use super::*;
    use solstone_core_journal_io::{DailyUnitIdentity, DailyUnitRecord, with_daily_unit_authority};

    fn fixture() -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        solstone_core_facets::create_facet(root.path(), "work", "Work", "", "", "", None).unwrap();
        solstone_core_facets::attach_or_reactivate_entity(
            root.path(),
            "work",
            "Person",
            "Ada",
            "Engineer",
        )
        .unwrap();
        root
    }

    #[test]
    fn observation_batch_recovers_drop_without_reinterpreting_shifted_indices() {
        let root = fixture();
        solstone_core_facets::write_facet_entity_observations(
            root.path(),
            "work",
            "ada",
            "{\"content\":\"Works at Acme\", \"observed_at\":1}\n{\"content\":\"Works at Acme part-time\", \"observed_at\":2}\n",
        )
        .unwrap();
        let batch = solstone_core_facets::prepare_observation_batch(
            root.path(),
            "work",
            "ada",
            &[json!({"op":"drop", "target_id":1, "target_quote":"Works at Acme"})],
            Some("20260910"),
        )
        .unwrap();
        assert!(
            solstone_core_facets::publish_observation_batch(root.path(), &batch, true, || Err(
                "receipt write failed".into()
            ))
            .is_err()
        );
        solstone_core_facets::publish_observation_batch(root.path(), &batch, false, || Ok(()))
            .unwrap();
        let rows = solstone_core_facets::read_live_observations(
            root.path(),
            "work",
            "ada",
            Default::default(),
        )
        .unwrap()
        .items;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].content, "Works at Acme part-time");
    }

    #[test]
    fn observation_add_and_quote_preserving_owner_edit_are_not_replayed() {
        let root = fixture();
        let add = solstone_core_facets::prepare_observation_batch(
            root.path(),
            "work",
            "ada",
            &[json!({"op":"add", "content":"Prefers concise updates"})],
            Some("20260910"),
        )
        .unwrap();
        assert!(
            solstone_core_facets::publish_observation_batch(root.path(), &add, true, || Err(
                "interrupt".into()
            ))
            .is_err()
        );
        std::fs::remove_file(
            root.path()
                .join("facets/work/entities/ada/observations.jsonl"),
        )
        .unwrap();
        assert!(
            solstone_core_facets::publish_observation_batch(root.path(), &add, false, || Ok(()))
                .is_err()
        );
        assert!(
            !root
                .path()
                .join("facets/work/entities/ada/observations.jsonl")
                .exists()
        );
        solstone_core_facets::add_observation(
            root.path(),
            "work",
            "ada",
            "Prefers concise updates",
            None,
            None,
        )
        .unwrap();
        let update = solstone_core_facets::prepare_observation_batch(root.path(), "work", "ada", &[json!({"op":"update", "target_id":1, "target_quote":"Prefers concise updates", "content":"Prefers concise updates; monthly"})], Some("20260910")).unwrap();
        assert!(
            solstone_core_facets::publish_observation_batch(root.path(), &update, true, || Err(
                "interrupt".into()
            ))
            .is_err()
        );
        solstone_core_facets::write_facet_entity_observations(
            root.path(),
            "work",
            "ada",
            "{\"content\":\"Prefers concise updates; weekly\", \"observed_at\":99}\n",
        )
        .unwrap();
        assert!(
            solstone_core_facets::publish_observation_batch(root.path(), &update, false, || Ok(()))
                .is_err()
        );
        let rows = solstone_core_facets::read_live_observations(
            root.path(),
            "work",
            "ada",
            Default::default(),
        )
        .unwrap()
        .items;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].content, "Prefers concise updates; weekly");
    }

    #[test]
    fn committed_action_receipt_preserves_later_owner_deletion() {
        let root = fixture();
        let batch = solstone_core_facets::prepare_observation_batch(
            root.path(),
            "work",
            "ada",
            &[json!({"op":"add", "content":"One retained fact"})],
            Some("20260910"),
        )
        .unwrap();
        let plan = PreparedDailyPublication {
            actions: vec![PreparedDailyAction::Observation { batch }],
            no_output: false,
        };
        let identity =
            DailyUnitIdentity::new("20260910", "entities:entity_observer", Some("work".into()));
        let context = ExecutionContext {
            journal: root.path().into(),
        };
        with_daily_unit_authority(root.path(), &identity, |authority| {
            let mut record = DailyUnitRecord::new(identity.clone(), "E", "C");
            record.lock_token = Some("attempt".into());
            record.generated_result = Some(json!({"output":"retained"}));
            record.action_plan = Some(serde_json::to_value(&plan).unwrap());
            *authority.record_mut() = Some(record);
            authority.checkpoint()?;
            publish_daily_publication(authority, "attempt", &plan, &context).unwrap();
            Ok(())
        })
        .unwrap();
        solstone_core_facets::delete_facet(root.path(), "work").unwrap();
        with_daily_unit_authority(root.path(), &identity, |authority| {
            // A retry has a fresh publication fence but retains the same plan
            // and the historical receipts of its completed owner actions.
            authority.record_mut().as_mut().unwrap().lock_token = Some("replacement".into());
            authority.checkpoint()?;
            assert!(publish_daily_publication(authority, "attempt", &plan, &context).is_err());
            publish_daily_publication(authority, "replacement", &plan, &context).unwrap();
            Ok(())
        })
        .unwrap();
        assert!(
            !root
                .path()
                .join("facets/work/entities/ada/observations.jsonl")
                .exists()
        );
    }

    #[test]
    fn durable_start_before_owner_write_is_visibly_ambiguous_on_restart() {
        let root = fixture();
        let batch = solstone_core_facets::prepare_observation_batch(
            root.path(),
            "work",
            "ada",
            &[json!({"op":"add", "content":"Must not resurrect"})],
            Some("20260910"),
        )
        .unwrap();
        let action = PreparedDailyAction::Observation { batch };
        let id = format!(
            "0:{:x}",
            Sha256::digest(serde_json::to_vec(&action).unwrap())
        );
        let plan = PreparedDailyPublication {
            actions: vec![action],
            no_output: false,
        };
        let identity =
            DailyUnitIdentity::new("20260910", "entities:entity_observer", Some("work".into()));
        let context = ExecutionContext {
            journal: root.path().into(),
        };
        with_daily_unit_authority(root.path(), &identity, |authority| {
            let mut record = DailyUnitRecord::new(identity.clone(), "E", "C");
            record.lock_token = Some("replacement".into());
            record.generated_result = Some(json!({"output":"retained"}));
            record.action_plan = Some(serde_json::to_value(&plan).unwrap());
            record.receipts.push(json!({"kind":"owner_action", "action_id":id, "token":"interrupted", "state":"started"}));
            *authority.record_mut() = Some(record);
            authority.checkpoint()
        }).unwrap();
        with_daily_unit_authority(root.path(), &identity, |authority| {
            let error =
                publish_daily_publication(authority, "replacement", &plan, &context).unwrap_err();
            assert_eq!(error.phase, "conflict");
            Ok(())
        })
        .unwrap();
        // The same observable state also results from an owner deleting an
        // unreceipted first write. It cannot safely authorize another append.
        assert!(
            !root
                .path()
                .join("facets/work/entities/ada/observations.jsonl")
                .exists()
        );
    }

    #[test]
    fn review_live_shaped_alias_refusal_keeps_prior_commits_and_no_uncommitted_started() {
        let root = fixture();
        let journal = root.path();
        let identity =
            DailyUnitIdentity::new("20260910", "entities:entities_review", Some("work".into()));
        let context = ExecutionContext {
            journal: journal.into(),
        };
        let facet_id = solstone_core_facets::facet_write_identity(journal, "work").unwrap();

        // 1. Setup claimant entity with alias "Late Claim" attached to "work"
        let claimant_identity = json!({
            "id": "late_claimant",
            "name": "Other",
            "type": "Person",
            "aka": ["Late Claim"]
        });
        solstone_core_entity::save_entity_identity(
            journal,
            "late_claimant",
            &claimant_identity,
            None,
        )
        .unwrap();
        solstone_core_facets::save_facet_entity_link(
            journal,
            "work",
            "late_claimant",
            "late_claimant",
            &Default::default(),
        )
        .unwrap();

        // 2. Action 0: Identity change for target (creates target entity)
        let target_identity = json!({"id":"target","name":"Target","type":"Person","aka":[]});
        let id_change = solstone_core_entity::PreparedIdentityChange {
            entity_id: "target".into(),
            entity_dir: "target".into(),
            before: None,
            after: target_identity.clone(),
        };
        let action_0 = PreparedDailyAction::Identity {
            facet: "work".into(),
            facet_id: facet_id.clone(),
            change: id_change,
        };

        // 3. Action 1: Attachment for target to "work"
        let att_change = solstone_core_facets::PreparedReviewAttachment {
            facet: "work".into(),
            facet_id: facet_id.clone(),
            relationship_dir: "target".into(),
            entity_id: "target".into(),
            before: None,
            after: json!({"entity_id":"target","type":"Person"}),
        };
        let action_1 = PreparedDailyAction::Attachment { change: att_change };

        // 4. Action 2: Aliases change for target trying to claim "Late Claim" (which claimant owns)
        let mut target_with_alias = target_identity.clone();
        target_with_alias["aka"] = json!(["Late Claim"]);
        let alias_id_change = solstone_core_entity::PreparedIdentityChange {
            entity_id: "target".into(),
            entity_dir: "target".into(),
            before: Some(target_identity.clone()),
            after: target_with_alias,
        };
        let action_2 = PreparedDailyAction::Aliases {
            facet: "work".into(),
            facet_id: facet_id.clone(),
            change: alias_id_change,
        };

        // 5. Action 3: Output action for outcome json
        let outcome_path = journal.join("facets/work/entities/20260910_review_outcome.json");
        let action_3 = PreparedDailyAction::Output {
            path: "facets/work/entities/20260910_review_outcome.json".into(),
            facet_identity: None,
            before: None,
            after: b"{\"outcome\":\"ok\"}\n".to_vec(),
        };

        let plan = PreparedDailyPublication {
            actions: vec![action_0, action_1, action_2, action_3],
            no_output: false,
        };

        with_daily_unit_authority(journal, &identity, |authority| {
            let mut record = DailyUnitRecord::new(identity.clone(), "E", "C");
            record.lock_token = Some("attempt-1".into());
            record.generated_result = Some(json!({"output":"retained"}));
            record.action_plan = Some(serde_json::to_value(&plan).unwrap());
            *authority.record_mut() = Some(record);
            authority.checkpoint()?;

            let err =
                publish_daily_publication(authority, "attempt-1", &plan, &context).unwrap_err();
            assert_eq!(err.talent, "entities:entities_review");
            assert_eq!(err.phase, "conflict");
            assert_eq!(err.owner_conflict_kind.as_deref(), Some("alias_claimed"));

            let binding = authority.record();
            let current_record = binding.as_ref().unwrap();
            assert!(!current_record.has_uncommitted_started_receipt());
            // Must have exactly 2 committed receipts (actions 0 and 1), and NO receipt for action 2 or 3
            assert_eq!(current_record.receipts.len(), 2);
            assert_eq!(current_record.receipts[0]["state"], "committed");
            assert_eq!(current_record.receipts[1]["state"], "committed");

            // Identity and attachment created by actions 0 and 1 remain committed
            assert!(
                solstone_core_entity::read_entity_identity(journal, "target")
                    .unwrap()
                    .is_some()
            );
            assert!(
                solstone_core_facets::read_facet_entity_link(journal, "work", "target")
                    .unwrap()
                    .is_some()
            );

            // Outcome file was never written
            assert!(!outcome_path.exists());
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn review_post_start_injected_fault_leaves_started_and_refuses_replay() {
        let root = fixture();
        let journal = root.path();
        let identity =
            DailyUnitIdentity::new("20260910", "entities:entities_review", Some("work".into()));
        let context = ExecutionContext {
            journal: journal.into(),
        };

        let facet_id = solstone_core_facets::facet_write_identity(journal, "work").unwrap();
        let target_identity = json!({"id":"target2","name":"Target2","type":"Person","aka":[]});
        let id_change = solstone_core_entity::PreparedIdentityChange {
            entity_id: "target2".into(),
            entity_dir: "target2".into(),
            before: None,
            after: target_identity.clone(),
        };
        let action = PreparedDailyAction::Identity {
            facet: "work".into(),
            facet_id,
            change: id_change,
        };
        let plan = PreparedDailyPublication {
            actions: vec![action],
            no_output: false,
        };

        // Simulate a crash right after start() was called for an action:
        let action_bytes = serde_json::to_vec(&plan.actions[0]).unwrap();
        let id = format!("0:{:x}", Sha256::digest(&action_bytes));
        with_daily_unit_authority(journal, &identity, |authority| {
            let mut record = DailyUnitRecord::new(identity.clone(), "E", "C");
            record.lock_token = Some("interrupted-token".into());
            record.generated_result = Some(json!({"output":"retained"}));
            record.action_plan = Some(serde_json::to_value(&plan).unwrap());
            record.receipts.push(json!({"kind":"owner_action", "action_id":id, "token":"interrupted-token", "state":"started"}));
            *authority.record_mut() = Some(record);
            authority.checkpoint()
        }).unwrap();

        // Restart with new worker attempt must refuse replay due to ambiguous uncommitted started receipt
        with_daily_unit_authority(journal, &identity, |authority| {
            let binding = authority.record();
            let record = binding.as_ref().unwrap();
            assert!(record.has_uncommitted_started_receipt());
            let err = publish_daily_publication(authority, "interrupted-token", &plan, &context)
                .unwrap_err();
            assert_eq!(err.phase, "conflict");
            assert_eq!(err.talent, "entities:entities_review");
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn malformed_model_output_is_distinct_from_owner_preparation_failure() {
        let root = fixture();
        let context = ExecutionContext {
            journal: root.path().into(),
        };
        let prepared = PreparedTalent {
            name: "schedule".into(),
            config: Default::default(),
        };
        let error = prepare_daily_publication(
            CommitPlan::Write(WriteIntent::Schedule {
                output: "not-json".into(),
                day: "20260910".into(),
            }),
            &prepared,
            &context,
        )
        .unwrap_err();
        assert_eq!(error.phase, "parse");
        let error = prepare_daily_publication(
            CommitPlan::Write(WriteIntent::Schedule {
                output: "[]".into(),
                day: "20260910".into(),
            }),
            &prepared,
            &context,
        )
        .unwrap_err();
        assert_eq!(error.phase, "publication");
    }

    #[test]
    fn schedule_intent_validation_accepts_mixed_future_and_non_future_events() {
        let valid_event = json!({
            "activity": "meeting",
            "target_date": "2026-09-20",
            "start": "09:00:00",
            "title": "Project sync",
            "description": "Discuss roadmap",
            "facet": "work",
            "participation": []
        });
        let non_future_event = json!({
            "activity": "meeting",
            "target_date": "2026-09-10",
            "start": "09:00:00",
            "title": "Past sync",
            "description": "Past roadmap",
            "facet": "work",
            "participation": []
        });
        let intent = WriteIntent::Schedule {
            output: json!({"events": [valid_event, non_future_event]}).to_string(),
            day: "20260910".into(),
        };
        assert!(validate_model_intent(&intent).is_ok());

        let array_intent = WriteIntent::Schedule {
            output: json!([valid_event, non_future_event]).to_string(),
            day: "20260910".into(),
        };
        assert!(validate_model_intent(&array_intent).is_ok());
    }

    #[test]
    fn schedule_intent_validation_rejects_malformed_json_events_shape_and_day() {
        let invalid_json = WriteIntent::Schedule {
            output: "not-json".into(),
            day: "20260910".into(),
        };
        let err = validate_model_intent(&invalid_json).unwrap_err();
        assert!(err.detail.starts_with("validation: invalid schedule JSON:"));

        let not_array = WriteIntent::Schedule {
            output: json!({"events": "not an array"}).to_string(),
            day: "20260910".into(),
        };
        let err = validate_model_intent(&not_array).unwrap_err();
        assert_eq!(err.detail, "validation: schedule events must be an array");

        let bad_day = WriteIntent::Schedule {
            output: "[]".into(),
            day: "bad-day".into(),
        };
        let err = validate_model_intent(&bad_day).unwrap_err();
        assert_eq!(err.detail, "validation: invalid schedule day");
    }

    #[test]
    fn calendar_replacement_and_supersession_are_one_recoverable_file() {
        let root = fixture();
        let old = json!({"id":"old", "source":"anticipated", "title":"Planning", "hidden":false})
            .as_object()
            .unwrap()
            .clone();
        assert!(matches!(
            solstone_core_facets::append_activity_record(root.path(), "work", "20260920", old)
                .unwrap(),
            solstone_core_facets::AppendOutcome::Written(_)
        ));
        let replacement = json!({"id":"new", "source":"anticipated", "title":"Planning", "cancelled":false, "hidden":false}).as_object().unwrap().clone();
        let batch = solstone_core_facets::prepare_anticipation_batch(
            root.path(),
            "work",
            "20260920",
            &[(replacement, vec!["old".into()])],
            "2026-09-14T10:00:00Z",
        )
        .unwrap();
        assert!(
            solstone_core_facets::publish_anticipation_batch(root.path(), &batch, true, || Err(
                "interrupt".into()
            ))
            .is_err()
        );
        solstone_core_facets::publish_anticipation_batch(root.path(), &batch, false, || Ok(()))
            .unwrap();
        let rows =
            solstone_core_facets::load_activity_records(root.path(), "work", "20260920", true)
                .unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(
            rows.iter().find(|row| row["id"] == "old").unwrap()["hidden"],
            true
        );
        let cancelled = json!({"id":"new", "source":"anticipated", "title":"Planning corrected", "cancelled":true, "hidden":true}).as_object().unwrap().clone();
        let correction = solstone_core_facets::prepare_anticipation_batch(
            root.path(),
            "work",
            "20260920",
            &[(cancelled, vec![])],
            "2026-09-14T11:00:00Z",
        )
        .unwrap();
        solstone_core_facets::publish_anticipation_batch(root.path(), &correction, true, || Ok(()))
            .unwrap();
        assert!(
            solstone_core_facets::load_activity_records(root.path(), "work", "20260920", false)
                .unwrap()
                .is_empty()
        );
        solstone_core_facets::set_activity_hidden(
            root.path(),
            "work",
            "20260920",
            "new",
            false,
            "owner",
            None,
            "2026-09-14T12:00:00Z",
        )
        .unwrap();
        assert!(
            solstone_core_facets::publish_anticipation_batch(
                root.path(),
                &correction,
                false,
                || Ok(())
            )
            .is_err()
        );
        assert_eq!(
            solstone_core_facets::load_activity_records(root.path(), "work", "20260920", false)
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn newsletter_and_config_reject_owner_changes_during_generation() {
        let root = fixture();
        let context = ExecutionContext {
            journal: root.path().into(),
        };
        solstone_core_facets::write_news_file(root.path(), "work", "20260910.md", "Old newsletter")
            .unwrap();
        let mut prepared = PreparedTalent {
            name: "facet_newsletter".into(),
            config: Default::default(),
        };
        prepared.config.insert(
            "_daily_artifact_before".into(),
            json!({"facets/work/news/20260910.md":b"Old newsletter".to_vec()}),
        );
        solstone_core_facets::write_news_file(
            root.path(),
            "work",
            "20260910.md",
            "Owner correction",
        )
        .unwrap();
        let failure = prepare_daily_publication(
            CommitPlan::Write(WriteIntent::FacetNewsletter {
                output: "Model replacement".into(),
                facet: "work".into(),
                day: "20260910".into(),
            }),
            &prepared,
            &context,
        )
        .unwrap_err();
        assert_eq!(failure.phase, "conflict");
        assert_eq!(
            solstone_core_facets::read_news_file(root.path(), "work", "20260910.md")
                .unwrap()
                .unwrap(),
            "Owner correction"
        );
        let path = root.path().join("config/schedules.json");
        solstone_core_system::schedule::set_schedule_metadata(
            &path,
            &serde_json::Map::from_iter([("daily_time".into(), json!("04:00"))]),
        )
        .unwrap();
        prepared.name = "daily_schedule".into();
        prepared
            .config
            .insert("_daily_time_before".into(), json!("03:00"));
        let failure = prepare_daily_publication(
            CommitPlan::Write(WriteIntent::DailySchedule {
                output: r#"{"primary":"05:00"}"#.into(),
                output_path: None,
            }),
            &prepared,
            &context,
        )
        .unwrap_err();
        assert_eq!(failure.phase, "conflict");
        let raw: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert_eq!(raw["daily_time"], "04:00");
    }

    #[test]
    fn observer_quote_preserving_edit_during_generation_is_not_adopted() {
        let root = fixture();
        solstone_core_facets::write_facet_entity_observations(
            root.path(),
            "work",
            "ada",
            "{\"content\":\"Prefers concise updates\", \"observed_at\":1}\n",
        )
        .unwrap();
        let prior =
            solstone_core_facets::read_facet_entity_observations(root.path(), "work", "ada")
                .unwrap();
        let mut prepared = PreparedTalent {
            name: "entities:entity_observer".into(),
            config: Default::default(),
        };
        prepared
            .config
            .insert("_daily_observation_before".into(), json!({"ada":prior}));
        prepared.config.insert(
            "_daily_artifact_before".into(),
            json!({"facets/work/entities/20260910_observer_outcome.json":null}),
        );
        prepared.config.insert(
            "_daily_facet_ids".into(),
            json!({"work":solstone_core_facets::facet_write_identity(root.path(), "work").unwrap()}),
        );
        prepared.config.insert("_daily_observer_resolution".into(), json!({"entities":[{"id":"ada", "name":"Ada", "aka":[], "emails":[], "blocked":false}], "choices":[]}));
        solstone_core_facets::write_facet_entity_observations(
            root.path(),
            "work",
            "ada",
            "{\"content\":\"Prefers concise updates; weekly\", \"observed_at\":2}\n",
        )
        .unwrap();
        let context = ExecutionContext {
            journal: root.path().into(),
        };
        let output = json!({"entities":[{"entity_id":"ada", "decisions":[{"op":"replace", "target_id":1, "target_quote":"Prefers concise updates", "content":"Prefers concise updates; monthly"}]}]}).to_string();
        let failure = prepare_daily_publication(
            CommitPlan::Write(WriteIntent::EntityObserver {
                output,
                facet: "work".into(),
                day: "20260910".into(),
                served_ids: std::collections::BTreeSet::from(["ada".into()]),
                shown_observation_ids: std::collections::BTreeMap::from([(
                    "ada".into(),
                    std::collections::BTreeSet::from([1]),
                )]),
            }),
            &prepared,
            &context,
        )
        .unwrap_err();
        assert_eq!(failure.phase, "conflict");
        let rows = solstone_core_facets::read_live_observations(
            root.path(),
            "work",
            "ada",
            Default::default(),
        )
        .unwrap()
        .items;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].content, "Prefers concise updates; weekly");
    }

    #[test]
    fn promotion_resumes_frozen_aliases_after_attachment() {
        let root = fixture();
        let plan = solstone_core_facets::prepare_review_promotion(
            root.path(),
            "work",
            "Person",
            "Grace Hopper",
            "Engineer",
            &["Amazing Grace".into()],
        )
        .unwrap();
        solstone_core_entity::publish_identity_change(
            root.path(),
            plan.identity.as_ref().unwrap(),
            true,
            || Ok(()),
            || Ok(()),
        )
        .unwrap();
        assert!(
            solstone_core_facets::publish_review_attachment(
                root.path(),
                &plan.attachment,
                true,
                || Ok(()),
                || Err("interrupt".into())
            )
            .is_err()
        );
        // No eligibility rebuild here: attachment has already changed that set.
        solstone_core_facets::publish_review_attachment(
            root.path(),
            &plan.attachment,
            false,
            || Ok(()),
            || Ok(()),
        )
        .unwrap();
        solstone_core_facets::publish_review_aliases(
            root.path(),
            "work",
            plan.aliases.as_ref().unwrap(),
            true,
            || Ok(()),
            || Ok(()),
        )
        .unwrap();
        let identity = solstone_core_entity::read_entity_identity(root.path(), "grace_hopper")
            .unwrap()
            .unwrap();
        assert_eq!(identity.value()["aka"], json!(["Amazing Grace"]));
    }

    #[test]
    fn prepared_merge_proposals_preserve_owner_decisions() {
        let root = fixture();
        let proposal = json!({"facet":"work", "day":"20260910", "source":"Ada", "source_slug":"ada", "target":"Ada Lovelace", "target_slug":"ada-lovelace", "summary":"Name variant"});
        let batch = solstone_core_entity::prepare_merge_proposals(
            root.path(),
            std::slice::from_ref(&proposal),
        )
        .unwrap();
        solstone_core_entity::publish_merge_proposals(
            root.path(),
            &batch,
            true,
            || Ok(()),
            || Ok(()),
        )
        .unwrap();
        solstone_core_entity::dismiss_merge_candidate(root.path(), "work", "ada", "ada-lovelace")
            .unwrap();
        assert!(
            solstone_core_entity::publish_merge_proposals(
                root.path(),
                &batch,
                false,
                || Ok(()),
                || Ok(())
            )
            .is_err()
        );
        let later =
            solstone_core_entity::prepare_merge_proposals(root.path(), &[proposal]).unwrap();
        solstone_core_entity::publish_merge_proposals(
            root.path(),
            &later,
            true,
            || Ok(()),
            || Ok(()),
        )
        .unwrap();
        assert_eq!(
            solstone_core_entity::load_merge_candidates(root.path(), Some("work"), None).unwrap()
                [0]["status"],
            "dismissed"
        );
    }
    #[test]
    fn legacy_facet_admission_assigns_identity_once_and_rejects_malformed_ids() {
        let root = fixture();
        let path = root.path().join("facets/work/facet.json");
        let mut value: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        value.as_object_mut().unwrap().remove("id");
        value["unknown_owner_field"] = json!({"keep":true});
        std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(solstone_core_facets::facet_write_identity(root.path(), "work").is_err());
        let id = solstone_core_facets::ensure_daily_facet_id(root.path(), "work").unwrap();
        assert!(solstone_core_journal_io::is_uuid_v4(&id));
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(
            solstone_core_facets::ensure_daily_facet_id(root.path(), "work").unwrap(),
            id
        );
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        let mut adopted: Value = serde_json::from_slice(&bytes).unwrap();
        adopted.as_object_mut().unwrap().remove("id");
        assert_eq!(adopted, value);
        value["id"] = json!("malformed");
        let malformed = serde_json::to_vec(&value).unwrap();
        std::fs::write(&path, &malformed).unwrap();
        assert!(solstone_core_facets::ensure_daily_facet_id(root.path(), "work").is_err());
        assert_eq!(std::fs::read(&path).unwrap(), malformed);
    }

    #[test]
    fn observation_absent_file_plan_cannot_write_after_detach_or_facet_replacement() {
        for mutation in ["detach", "delete", "replace"] {
            let root = fixture();
            let batch = solstone_core_facets::prepare_observation_batch(
                root.path(),
                "work",
                "ada",
                &[json!({"op":"add", "content":"Must not recreate memory"})],
                Some("20260910"),
            )
            .unwrap();
            assert!(batch.before.is_none());
            if mutation == "detach" {
                solstone_core_facets::detach_facet_entity(root.path(), "work", &batch.entity_id)
                    .unwrap();
            } else {
                solstone_core_facets::delete_facet(root.path(), "work").unwrap();
                if mutation == "replace" {
                    solstone_core_facets::create_facet(
                        root.path(),
                        "work",
                        "Work",
                        "",
                        "",
                        "",
                        None,
                    )
                    .unwrap();
                    solstone_core_facets::attach_or_reactivate_entity(
                        root.path(),
                        "work",
                        "Person",
                        "Ada",
                        "Engineer",
                    )
                    .unwrap();
                }
            }
            let failure =
                solstone_core_facets::publish_observation_batch(root.path(), &batch, true, || {
                    Ok(())
                })
                .unwrap_err();
            assert!(failure.starts_with("conflict:"), "{mutation}: {failure}");
            assert!(
                !root
                    .path()
                    .join("facets/work/entities/ada/observations.jsonl")
                    .exists()
            );
        }
    }

    #[test]
    fn retained_facet_actions_cannot_recreate_deleted_owner_paths() {
        for kind in [
            "calendar",
            "news",
            "output",
            "identity",
            "attachment",
            "aliases",
            "proposals",
        ] {
            let root = fixture();
            let facet_id = solstone_core_facets::facet_write_identity(root.path(), "work").unwrap();
            let action = match kind {
                "calendar" => PreparedDailyAction::Anticipation { batch: solstone_core_facets::prepare_anticipation_batch(root.path(), "work", "20260920", &[(json!({"id":"event", "source":"anticipated", "title":"Planning"}).as_object().unwrap().clone(), vec![])], "2026-09-14T10:00:00Z").unwrap() },
                "news" => PreparedDailyAction::Newsletter { batch: solstone_core_facets::prepare_news_replacement(root.path(), "work", "20260910.md", "News").unwrap() },
                "output" => prepare_output_action(root.path(), &root.path().join("facets/work/entities/20260910_review_outcome.json"), b"result".to_vec()).unwrap(),
                "proposals" => PreparedDailyAction::MergeProposals { facet:"work".into(), facet_id, batch:solstone_core_entity::prepare_merge_proposals(root.path(), &[json!({"facet":"work", "day":"20260910", "source":"Ada", "source_slug":"ada", "target":"Grace", "target_slug":"grace", "summary":"Variant"})]).unwrap() },
                _ => {
                    let promotion = solstone_core_facets::prepare_review_promotion(root.path(), "work", "Person", "Grace Hopper", "Engineer", &["Amazing Grace".into()]).unwrap();
                    match kind {
                        "identity" => PreparedDailyAction::Identity { facet:"work".into(), facet_id, change:promotion.identity.unwrap() },
                        "attachment" => PreparedDailyAction::Attachment { change:promotion.attachment },
                        _ => PreparedDailyAction::Aliases { facet:"work".into(), facet_id, change:promotion.aliases.unwrap() },
                    }
                },
            };
            let publication = PreparedDailyPublication {
                actions: vec![action],
                no_output: false,
            };
            let identity =
                DailyUnitIdentity::new("20260910", "entities:entities_review", Some("work".into()));
            with_daily_unit_authority(root.path(), &identity, |authority| {
                let mut record = DailyUnitRecord::new(identity.clone(), "E", "C");
                record.lock_token = Some("attempt".into());
                record.generated_result = Some(json!({"output":"retained"}));
                record.action_plan = Some(serde_json::to_value(&publication).unwrap());
                *authority.record_mut() = Some(record);
                authority.checkpoint()
            })
            .unwrap();
            solstone_core_facets::delete_facet(root.path(), "work").unwrap();
            with_daily_unit_authority(root.path(), &identity, |authority| {
                let error = publish_daily_publication(
                    authority,
                    "attempt",
                    &publication,
                    &ExecutionContext {
                        journal: root.path().into(),
                    },
                )
                .unwrap_err();
                assert_eq!(error.phase, "conflict", "{kind}: {error:?}");
                Ok(())
            })
            .unwrap();
            assert!(!root.path().join("facets/work").exists(), "{kind}");
            assert!(
                solstone_core_entity::read_entity_identity(root.path(), "grace_hopper")
                    .unwrap()
                    .is_none()
            );
        }
    }

    #[test]
    fn schedule_same_title_siblings_remain_visible_across_e_to_e2() {
        let root = fixture();
        let event = |start: &str| json!({"activity":"meeting", "target_date":"2026-09-20", "start":start, "title":"Project sync", "description":"Discuss roadmap", "facet":"work", "participation":[]});
        let output = json!({"events":[event("09:00:00"), event("15:00:00")]}).to_string();
        for _ in 0..2 {
            let batches =
                crate::schedule::prepare_publication(root.path(), &output, "20260910").unwrap();
            assert_eq!(batches.len(), 1);
            solstone_core_facets::publish_anticipation_batch(
                root.path(),
                &batches[0],
                true,
                || Ok(()),
            )
            .unwrap();
            assert_eq!(
                solstone_core_facets::load_activity_records(root.path(), "work", "20260920", false)
                    .unwrap()
                    .len(),
                2
            );
        }
        // One exact-ID update and a new replacement must not supersede the
        // exact-ID sibling that appears elsewhere in the incoming batch.
        let output = json!({"events":[event("10:00:00"), event("15:00:00")]}).to_string();
        let batches =
            crate::schedule::prepare_publication(root.path(), &output, "20260910").unwrap();
        solstone_core_facets::publish_anticipation_batch(root.path(), &batches[0], true, || Ok(()))
            .unwrap();
        let visible =
            solstone_core_facets::load_activity_records(root.path(), "work", "20260920", false)
                .unwrap();
        assert_eq!(visible.len(), 2);
        assert!(visible.iter().any(|row| row["start"] == "15:00:00"));
        assert!(visible.iter().any(|row| row["start"] == "10:00:00"));
    }

    #[test]
    fn facet_replacement_during_generation_cannot_be_adopted_by_empty_before_image() {
        let root = fixture();
        let facet_id = solstone_core_facets::facet_write_identity(root.path(), "work").unwrap();
        let mut prepared = PreparedTalent {
            name: "facet_newsletter".into(),
            config: Default::default(),
        };
        prepared
            .config
            .insert("_daily_facet_ids".into(), json!({"work":facet_id}));
        prepared.config.insert(
            "_daily_artifact_before".into(),
            json!({"facets/work/news/20260910.md":null}),
        );
        solstone_core_facets::delete_facet(root.path(), "work").unwrap();
        solstone_core_facets::create_facet(root.path(), "work", "Work", "", "", "", None).unwrap();
        let error = prepare_daily_publication(
            CommitPlan::Write(WriteIntent::FacetNewsletter {
                output: "Old facet result".into(),
                facet: "work".into(),
                day: "20260910".into(),
            }),
            &prepared,
            &ExecutionContext {
                journal: root.path().into(),
            },
        )
        .unwrap_err();
        assert_eq!(error.phase, "conflict");
        assert!(!root.path().join("facets/work/news/20260910.md").exists());
    }

    fn set_relation_test_alias(root: &Path, id: &str, aliases: &[&str]) {
        let mut identity = solstone_core_entity::read_entity_identity(root, id)
            .unwrap()
            .unwrap()
            .value()
            .clone();
        identity["aka"] = json!(aliases);
        let operation = solstone_core_entity::EntityOperationContext {
            kind: solstone_core_entity::EntityOperationKind::Update,
            caller: json!({"name":"owner"}),
            actor: json!({"name":"owner"}),
            metadata: json!({}),
        };
        solstone_core_entity::save_entity_identity(root, id, &identity, Some(&operation)).unwrap();
    }

    fn relation_test_prepared(
        root: &Path,
    ) -> (PreparedTalent, crate::entities::observer::ObserverState) {
        for name in ["Alpha", "Beta"] {
            solstone_core_facets::attach_or_reactivate_entity(
                root, "work", "Person", name, "Engineer",
            )
            .unwrap();
        }
        set_relation_test_alias(root, "alpha", &["Grace"]);
        let sugg_dir = root.join("facets/work/entities");
        std::fs::create_dir_all(&sugg_dir).unwrap();
        std::fs::write(
            sugg_dir.join("20260910_observer_suggestions.json"),
            json!({
                "facet": "work",
                "day": "20260910",
                "entities": [
                    {
                        "entity_id": "ada",
                        "suggestions": [
                            {"content": "Works on Project X"}
                        ]
                    }
                ]
            })
            .to_string(),
        )
        .unwrap();
        let mut prepared = PreparedTalent {
            name: "entities:entity_observer".into(),
            config: json!({"facet":"work", "day":"20260910"})
                .as_object()
                .unwrap()
                .clone(),
        };
        let state = crate::entities::observer::build(
            &mut prepared,
            &ExecutionContext {
                journal: root.into(),
            },
        )
        .unwrap();
        let crate::contract::PrePostState::EntityObserver(state) = state else {
            panic!("observer state")
        };
        assert!(state.served_ids.contains("ada"));
        (prepared, state)
    }

    fn relation_test_output() -> String {
        json!({"entities":[{"entity_id":"ada", "decisions":[{"op":"add", "content":"Collaborates with Grace", "relation":{"kind":"works-with", "target_name":"Grace", "note":"project"}}]}]}).to_string()
    }

    #[test]
    fn observer_retained_relation_keeps_id() {
        let root = fixture();
        let (prepared, state) = relation_test_prepared(root.path());
        let (batches, _) = crate::entities::observer::prepare_publication(
            root.path(),
            &relation_test_output(),
            "work",
            "20260910",
            &state.served_ids,
            &state.shown_observation_ids,
            &prepared,
        )
        .unwrap();
        assert_eq!(batches.len(), 1);
        solstone_core_facets::publish_observation_batch(root.path(), &batches[0], true, || Ok(()))
            .unwrap();
        let rows = solstone_core_facets::read_live_observations(
            root.path(),
            "work",
            "ada",
            Default::default(),
        )
        .unwrap()
        .items;
        assert_eq!(
            rows[0].relation.as_ref().unwrap()["target_entity_id"],
            "alpha"
        );
        assert!(
            !root
                .path()
                .join("facets/work/entities/beta/observations.jsonl")
                .exists()
        );
    }
}
