// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Commit offload preferences through the originally admitted config parent.

use std::{path::Path, sync::Arc};

use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::{Map, Value, json};
use solstone_core_journal_config::read_journal_config_in_directory;
use solstone_core_journal_config_write::{
    BoundConfigOutcome, BoundConfigRefusal, LockOptions, MAX_BOUND_CONFIG_BYTES,
    conditional_update_journal_config_in_directory_checked,
};
use solstone_core_journal_io::{JournalRoot, media::MediaDirectory, whole_journal_restore};

use crate::{AdmitWrite, BackupWebDeps, admit_write, config, keys, measurement, response};

struct EnableSnapshot {
    root: Arc<JournalRoot>,
    directory: Arc<MediaDirectory>,
    sha256: String,
    backup: Map<String, Value>,
}

impl EnableSnapshot {
    fn read(root: Arc<JournalRoot>) -> Result<Self, ()> {
        let directory =
            MediaDirectory::open(Arc::clone(&root), Path::new("config")).map_err(|_| ())?;
        let read =
            read_journal_config_in_directory(&directory, MAX_BOUND_CONFIG_BYTES).map_err(|_| ())?;
        let sha256 = read.sha256.ok_or(())?;
        let backup = config::backup_from_map(&read.config.ok_or(())?)?;
        Ok(Self {
            root,
            directory,
            sha256,
            backup,
        })
    }

    fn commit(
        &self,
        defaults: &Value,
        checkpoint: &dyn Fn() -> Result<(), String>,
    ) -> Result<BoundConfigOutcome, BoundConfigRefusal> {
        conditional_update_journal_config_in_directory_checked(
            &self.root,
            &self.directory,
            &self.sha256,
            |config| {
                let mut config = config.clone();
                let backup = config.get_mut("backup")?.as_object_mut()?;
                let old = backup.get("offload").and_then(Value::as_object);
                let preference = |name: &str| {
                    old.and_then(|old| old.get(name))
                        .filter(|value| !value.is_null())
                        .cloned()
                        .unwrap_or_else(|| defaults["suggested_defaults"][name].clone())
                };
                let offload = json!({
                    "enabled": true,
                    "budget_bytes": preference("budget_bytes"),
                    "floor_bytes": preference("floor_bytes"),
                });
                if backup.get("offload") == Some(&offload) {
                    return None;
                }
                backup.insert("offload".into(), offload);
                Some(config)
            },
            LockOptions::default(),
            checkpoint,
        )
    }
}

fn publication(outcome: &BoundConfigOutcome) -> Value {
    match outcome {
        BoundConfigOutcome::Unchanged => json!({"state":"unchanged","confirmed":true}),
        BoundConfigOutcome::Published(p) => json!({
            "state":"published",
            "confirmed":p.durability_uncertainty.is_none()
                && p.observation_uncertainty.is_none() && p.confirmation_refusal.is_none(),
            "durability_uncertain":p.durability_uncertainty.is_some(),
            "observation_uncertain":p.observation_uncertainty.is_some(),
            "confirmation_refused":p.confirmation_refusal.is_some(),
        }),
    }
}

fn partial(reason: &str, publication: Value, verification: Value) -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({
            "error":"media offload setup couldn't finish; reload before trying again.",
            "reason_code":reason,
            "configuration_publication":publication,
            "verification_request":verification,
        })),
    )
        .into_response()
}

pub(crate) fn admit(
    deps: BackupWebDeps,
) -> Result<impl FnOnce() -> Response + Send + 'static, Response> {
    let guard = admit_write(&deps.operations, AdmitWrite::DuringOperation)?;
    let root = deps
        .retained_root()
        .map_err(|_| crate::invalid_operation_response())?;
    let target_use = whole_journal_restore::begin_target_use_bound(Arc::clone(&root))
        .map_err(|_| crate::invalid_operation_response())?;
    // Admission belongs to the queued frame before the blocking executor can
    // delay it or the HTTP caller can disappear.
    Ok(move || enable(deps, root, guard, target_use))
}

fn enable(
    deps: BackupWebDeps,
    root: Arc<JournalRoot>,
    guard: crate::WriteGuard,
    _target_use: whole_journal_restore::TargetUse,
) -> Response {
    let checkpoint = || {
        if deps.operations.is_stopping()
            || whole_journal_restore::whole_journal_restore_blocks(root.canonical_path())
        {
            return Err("backup owner unavailable".into());
        }
        root.revalidate_canonical_binding()
            .map_err(|_| "backup root changed".into())
    };
    if checkpoint().is_err() {
        return crate::invalid_operation_response();
    }
    let Ok(snapshot) = EnableSnapshot::read(Arc::clone(&root)) else {
        return response::invalid_config("");
    };
    if snapshot.backup.get("enabled") != Some(&Value::Bool(true)) {
        return response::error(
            StatusCode::BAD_REQUEST,
            "that action isn't available in the current state.",
            "invalid_operation_for_state",
            "backup is disabled",
        );
    }
    if snapshot.backup.get("confirmed_recovery_key") != Some(&Value::Bool(true)) {
        return response::error(
            StatusCode::BAD_REQUEST,
            "backup can't be turned on until you confirm your recovery key.",
            "backup_not_confirmed",
            "",
        );
    }
    if !matches!(keys::keys(&snapshot.backup), Ok(Some(_))) {
        return response::error(
            StatusCode::BAD_REQUEST,
            "that action isn't available in the current state.",
            "invalid_operation_for_state",
            "backup keys are missing",
        );
    }
    let outcome = match snapshot.commit(&measurement::snapshot(&deps.cache), &checkpoint) {
        Ok(outcome) => outcome,
        Err(_) => return response::invalid_config(""),
    };
    let published = publication(&outcome);
    // End the configuration write reservation before the queue can dispatch
    // verification and acquire the ordinary backup maintenance reservation.
    drop(guard);
    measurement::invalidate(&deps.cache);
    if published["confirmed"] != true || checkpoint().is_err() {
        return partial(
            "configuration_unconfirmed",
            published,
            json!({"state":"not_submitted"}),
        );
    }
    let verification = if snapshot.backup["last_verification"]["status"].is_null() {
        match &deps.verification_submitter {
            Some(submit) => match submit() {
                Ok(reference) => json!({"state":"accepted","ref":reference}),
                Err(_) => {
                    return partial("backup_unavailable", published, json!({"state":"refused"}));
                }
            },
            None if !cfg!(any(target_os = "ios", target_os = "android")) => {
                if crate::callosum::request(&deps.journal_root, "backup:verify") {
                    json!({"state":"requested"})
                } else {
                    // Preserve the legacy desktop best-effort request while
                    // reporting refusal independently of the saved setting.
                    json!({"state":"refused"})
                }
            }
            None => return partial("backup_unavailable", published, json!({"state":"refused"})),
        }
    } else {
        json!({"state":"not_needed"})
    };
    if checkpoint().is_err() {
        return partial("backup_unavailable", published, verification);
    }
    let Ok(mut status) = crate::status::offload(&deps.journal_root, &deps.cache, &deps.operations)
    else {
        return partial("backup_unavailable", published, verification);
    };
    if checkpoint().is_err() {
        return partial("backup_unavailable", published, verification);
    }
    status["configuration_publication"] = published;
    status["verification_request"] = verification;
    response::success(status)
}

#[cfg(all(test, feature = "full-tests"))]
#[path = "offload_enable_tests.rs"]
mod tests;
