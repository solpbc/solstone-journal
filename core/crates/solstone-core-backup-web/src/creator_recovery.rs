// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Fixed local native boundary. This does not create an HTTP route or enqueue work.

use serde::Deserialize;
use serde_json::Value;
use solstone_core_journal_io::JournalRoot;
use solstone_core_offload::creator_recovery::{CreatorConfirmation, CreatorStageRecovery};
use std::sync::Arc;

const MAX_MESSAGE: usize = 256 * 1024;

#[derive(Clone, Copy, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
enum Action {
    Observe,
    Apply,
    Reconcile,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    op: String,
    admission_generation: u64,
    request_id: String,
    action: Action,
    root_observation: Option<String>,
    observation_request_id: Option<String>,
    observation_epoch: Option<u64>,
    confirmation: Option<CreatorConfirmation>,
    confirmation_revision: Option<String>,
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_".contains(&byte))
}

/// Native establishes stopped retirement and serializes against Start. The same
/// actual owner survives effects and construction of the receiving result.
pub fn run_stopped(
    root: Arc<JournalRoot>,
    command: &Value,
    checkpoint: &dyn Fn() -> Result<(), String>,
) -> Result<Value, String> {
    let refused = || "creator_recovery_request_refused".to_owned();
    if serde_json::to_vec(command).map_err(|_| refused())?.len() > MAX_MESSAGE {
        return Err(refused());
    }
    let request: Request = serde_json::from_value(command.clone()).map_err(|_| refused())?;
    if request.op != "creator-recovery"
        || request.admission_generation & 1 == 0
        || !valid_id(&request.request_id)
        || request
            .root_observation
            .as_ref()
            .is_some_and(|expected| expected != &root.identity().observation_id())
    {
        return Err(refused());
    }
    match request.action {
        Action::Observe
            if request.confirmation.is_none()
                && request.confirmation_revision.is_none()
                && request.observation_epoch.is_none()
                && request.observation_request_id.is_none() => {}
        Action::Apply | Action::Reconcile
            if request.root_observation.is_some()
                && request
                    .observation_request_id
                    .as_deref()
                    .is_some_and(valid_id)
                && request.confirmation.as_ref().is_some_and(|confirmation| {
                    confirmation.revision().ok().as_ref() == request.confirmation_revision.as_ref()
                })
                && request.observation_epoch.is_some_and(|epoch| {
                    epoch & 1 == 1
                        && (request.action == Action::Reconcile
                            || epoch == request.admission_generation)
                }) => {}
        _ => return Err(refused()),
    }
    checkpoint()?;
    let check = || checkpoint().map_err(std::io::Error::other);
    let now = chrono::Local::now().fixed_offset();
    let mut owner = if request.action == Action::Reconcile {
        CreatorStageRecovery::reconcile(
            root,
            request.confirmation.as_ref().ok_or_else(refused)?,
            now,
            &check,
        )
    } else {
        CreatorStageRecovery::observe(root, now, &check)
    };
    let mut report = owner.report();
    if request.action != Action::Reconcile && report["refusal"].is_null() {
        let confirmation = owner.confirmation(&check).map_err(|_| refused())?;
        report["confirmation_revision"] =
            Value::String(confirmation.revision().map_err(|_| refused())?);
        report["confirmation"] = serde_json::to_value(&confirmation).map_err(|_| refused())?;
    }
    report["request_id"] = Value::String(request.request_id.clone());
    report["native_epoch"] = Value::from(request.admission_generation);
    // Reserve room for bounded partial-effect facts before the first cut.
    // Oversized review never becomes actionable.
    if serde_json::to_vec(&report).map_err(|_| refused())?.len() > MAX_MESSAGE / 2 {
        return Err(refused());
    }
    if request.action == Action::Apply {
        owner.apply_confirmed(request.confirmation.as_ref().ok_or_else(refused)?, &check);
        let facts = owner.report();
        for (key, value) in facts.as_object().ok_or_else(refused)? {
            report[key] = value.clone();
        }
    }
    // The actual frame releases handles/lease before receiving. Drop cannot
    // retry cleanup, schedule ordinary work, or infer a remote outcome.
    drop(owner);
    Ok(report)
}
