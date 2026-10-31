// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::cell::RefCell;
use std::path::Path;
use std::sync::Arc;

use hmac::{Hmac, Mac};
use serde_json::{Map, Value};
use sha2::Sha256;
use solstone_core_journal_config::read_journal_config;
use solstone_core_journal_config_write::{
    BoundConfigOutcome, BoundConfigRefusal, CommitConfigError, ConfigExpectation,
    ConfigMutationError, JournalConfigMutation, LockOptions, commit_journal_config_checked,
    conditional_update_journal_config_at_root, mutate_journal_config_at_root_checked,
    mutate_journal_config_checked,
};
use solstone_core_journal_io::JournalRoot;

use crate::{
    BackupError, BackupKeys, Destination, HostedBinding, OFFLOAD_STATUSES, RESTORE_SCOPES,
    RESTORE_STATUSES, VERIFICATION_STATUSES, backup_defaults, format_recovery_key_display,
    generate_daily_key, generate_recovery_key, load_hosted_binding, merge_backup_config,
};

const RETENTION_KEYS: [&str; 4] = ["hourly", "daily", "weekly", "monthly"];
const OFFLOAD_KEYS: [&str; 3] = ["enabled", "budget_bytes", "floor_bytes"];

pub fn get_backup_config(journal: &Path) -> Result<Map<String, Value>, BackupError> {
    let read = read_journal_config(journal).map_err(BackupError::ConfigLoad)?;
    Ok(merge_backup_config(&read.config.unwrap_or_default()))
}

pub fn get_destination(journal: &Path) -> Result<Option<Destination>, BackupError> {
    Ok(destination_from_config(&get_backup_config(journal)?))
}

pub fn destination_from_config(config: &Map<String, Value>) -> Option<Destination> {
    let destination = config.get("destination")?.as_object()?;
    let (Some(repository), Some(backend), Some(credentials)) = (
        destination.get("repository").and_then(Value::as_str),
        destination.get("backend").and_then(Value::as_str),
        destination.get("credentials").and_then(Value::as_object),
    ) else {
        return None;
    };
    Some(Destination {
        repository: repository.to_owned(),
        backend: backend.to_owned(),
        credentials: credentials.clone(),
    })
}

pub fn get_keys(journal: &Path) -> Result<Option<BackupKeys>, BackupError> {
    let config = get_backup_config(journal)?;
    build_keys(config.get("daily_key"), config.get("recovery_key"))
}

pub fn generate_and_store_keys(journal: &Path) -> Result<BackupKeys, BackupError> {
    let generated_daily = generate_daily_key()?;
    let generated_recovery = generate_recovery_key()?;
    mutate_backup_section(journal, move |backup| {
        let daily = backup.get("daily_key").cloned().unwrap_or(Value::Null);
        let recovery = backup.get("recovery_key").cloned().unwrap_or(Value::Null);
        let mut changed = false;
        let daily = if daily.is_null() {
            changed = true;
            Value::String(generated_daily)
        } else {
            daily
        };
        let recovery = if recovery.is_null() {
            changed = true;
            Value::String(generated_recovery)
        } else {
            recovery
        };
        let keys = build_keys(Some(&daily), Some(&recovery))?.expect("non-null keys are present");
        backup.insert("daily_key".to_owned(), daily);
        backup.insert("recovery_key".to_owned(), recovery);
        Ok((changed, keys))
    })
}

pub fn set_destination(journal: &Path, destination: &Destination) -> Result<(), BackupError> {
    let value = Map::from_iter([
        (
            "repository".to_owned(),
            Value::String(destination.repository.clone()),
        ),
        (
            "backend".to_owned(),
            Value::String(destination.backend.clone()),
        ),
        (
            "credentials".to_owned(),
            Value::Object(destination.credentials.clone()),
        ),
    ]);
    mutate_backup_section(journal, |backup| {
        set_value(backup, "destination", Value::Object(value))
    })
}

pub fn set_enabled(journal: &Path, enabled: bool) -> Result<(), BackupError> {
    mutate_backup_section(journal, |backup| {
        set_value(backup, "enabled", Value::Bool(enabled))
    })
}

/// When the owner paused backup, as unix seconds. A pause keeps every setting
/// and only stops work that reaches the destination: backup runs, verification,
/// prune and media offload. Restore and turn off & delete are never paused.
pub fn backup_paused_at(config: &Map<String, Value>) -> Option<i64> {
    config.get("paused_at").and_then(Value::as_i64)
}

/// Pause backup at `paused_at`, or resume it with `None`. Pausing an already
/// paused backup keeps the original time, so "paused since" stays true.
pub fn set_backup_paused(journal: &Path, paused_at: Option<i64>) -> Result<(), BackupError> {
    mutate_backup_section(journal, |backup| match paused_at {
        Some(_) if backup_paused_at(backup).is_some() => Ok((false, ())),
        Some(time) => set_value(backup, "paused_at", Value::from(time)),
        None => Ok((backup.remove("paused_at").is_some(), ())),
    })
}

pub fn set_mode(journal: &Path, mode: &str) -> Result<(), BackupError> {
    if !matches!(mode, "byo" | "operated") {
        return Err(BackupError::InvalidMode);
    }
    mutate_backup_section(journal, |backup| {
        set_value(backup, "mode", Value::String(mode.to_owned()))
    })
}

/// Publish a mode through the original journal, preserving publication uncertainty.
pub fn set_mode_at_root_checked(
    root: &Arc<JournalRoot>,
    mode: &str,
    checkpoint: &dyn Fn() -> Result<(), String>,
) -> Result<BoundConfigOutcome, BoundConfigRefusal> {
    if !matches!(mode, "byo" | "operated") {
        return Err(BoundConfigRefusal::Conflict);
    }
    let admit = || {
        admit_backup_task()?;
        checkpoint()
    };
    let transaction =
        mutate_journal_config_at_root_checked(root, mutation_lock_options(), &admit, |config| {
            let backup = config
                .entry("backup")
                .or_insert_with(|| Value::Object(Map::new()));
            let Some(backup) = backup.as_object_mut() else {
                return JournalConfigMutation {
                    changed: false,
                    value: false,
                };
            };
            let mode = Value::String(mode.to_owned());
            let changed = backup.get("mode") != Some(&mode);
            backup.insert("mode".into(), mode);
            JournalConfigMutation {
                changed,
                value: true,
            }
        })?;
    if !transaction.value {
        return Err(BoundConfigRefusal::Conflict);
    }
    Ok(transaction.outcome)
}

pub fn set_retention(journal: &Path, retention: &Map<String, Value>) -> Result<(), BackupError> {
    if retention.len() != RETENTION_KEYS.len()
        || !RETENTION_KEYS
            .iter()
            .all(|key| retention.contains_key(*key))
    {
        return Err(BackupError::InvalidRetentionShape);
    }
    if RETENTION_KEYS
        .iter()
        .any(|key| !is_nonnegative_integer(retention.get(*key)))
    {
        return Err(BackupError::InvalidRetentionValue);
    }
    mutate_backup_section(journal, |backup| {
        set_value(backup, "retention", Value::Object(retention.clone()))
    })
}

pub fn set_offload(journal: &Path, offload: &Map<String, Value>) -> Result<(), BackupError> {
    if offload.len() != OFFLOAD_KEYS.len()
        || !OFFLOAD_KEYS.iter().all(|key| offload.contains_key(*key))
    {
        return Err(BackupError::InvalidOffloadShape);
    }
    if !matches!(offload.get("enabled"), Some(Value::Bool(_))) {
        return Err(BackupError::InvalidOffloadEnabled);
    }
    if ["budget_bytes", "floor_bytes"]
        .iter()
        .any(|key| !is_positive_or_null(offload.get(*key)))
    {
        return Err(BackupError::InvalidOffloadBytes);
    }
    mutate_backup_section(journal, |backup| {
        set_value(backup, "offload", Value::Object(offload.clone()))
    })
}

pub fn set_recovery_key(journal: &Path, recovery_key: &str) -> Result<(), BackupError> {
    mutate_backup_section(journal, |backup| {
        set_value(
            backup,
            "recovery_key",
            Value::String(recovery_key.to_owned()),
        )
    })
}

pub fn set_recovery_key_confirmed(journal: &Path, confirmed: bool) -> Result<(), BackupError> {
    mutate_backup_section(journal, |backup| {
        set_value(backup, "confirmed_recovery_key", Value::Bool(confirmed))
    })
}

pub fn clear_backup_config(journal: &Path) -> Result<(), BackupError> {
    let defaults = backup_defaults();
    mutate_backup_section(journal, |backup| {
        let changed = *backup != defaults;
        *backup = defaults;
        Ok((changed, ()))
    })
}

/// Clear an observed backup section while preserving unrelated configuration.
/// Missing or conflicting state returns false; read/write failures return errors.
/// This enforces local consistency, not teardown authorization.
pub fn clear_backup_config_if_unchanged(
    journal: &Path,
    expected: &Map<String, Value>,
) -> Result<bool, BackupError> {
    let read = read_journal_config(journal).map_err(BackupError::ConfigLoad)?;
    let (Some(mut config), Some(fingerprint)) = (read.config, read.sha256) else {
        return Ok(false);
    };
    if merge_backup_config(&config) != *expected {
        return Ok(false);
    }
    config.insert("backup".into(), Value::Object(backup_defaults()));
    run_mutation_hook();
    let admit = || admit_backup_task();
    match commit_journal_config_checked(
        journal,
        ConfigExpectation::Sha256(fingerprint),
        &config,
        mutation_lock_options(),
        &admit,
    ) {
        Ok(()) => Ok(true),
        Err(CommitConfigError::Conflict(_)) => Ok(false),
        Err(CommitConfigError::Load(error)) => Err(BackupError::ConfigLoad(error)),
        Err(CommitConfigError::Lock(error)) => Err(BackupError::ConfigMutation(
            ConfigMutationError::Lock(error),
        )),
        Err(CommitConfigError::Write(error)) => Err(BackupError::ConfigMutation(
            ConfigMutationError::Write(error),
        )),
    }
}

/// Clear only the originally admitted backup settings beneath the retained root.
/// This preserves publication uncertainty and supplies no teardown authorization.
pub fn clear_backup_config_at_root(
    root: &Arc<JournalRoot>,
    expected_sha256: &str,
    expected_backup: &Map<String, Value>,
) -> Result<BoundConfigOutcome, BoundConfigRefusal> {
    conditional_update_journal_config_at_root(
        root,
        expected_sha256,
        |config| cleared_backup_configuration(config, expected_backup),
        mutation_lock_options(),
    )
}

pub(crate) fn cleared_backup_configuration(
    config: &Map<String, Value>,
    expected_backup: &Map<String, Value>,
) -> Option<Map<String, Value>> {
    if merge_backup_config(config) != *expected_backup {
        return None;
    }
    let mut replacement = config.clone();
    replacement.insert("backup".into(), Value::Object(backup_defaults()));
    Some(replacement)
}

/// Durable pending key revocation intent staged before remote key removal.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PendingKeyRevocation {
    pub original_key_id: String,
    pub candidate_binding: String,
    pub execution: PendingExecution,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setup_fingerprint: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PendingExecution {
    pub mode: String,
    pub repository: String,
    pub backend: String,
    pub credential_binding: String,
}

impl PendingKeyRevocation {
    /// A present malformed intent remains unresolved and permits no remote work.
    pub fn parse(value: &Value) -> Option<Self> {
        let pending: Self = serde_json::from_value(value.clone()).ok()?;
        fn binding(value: &str) -> bool {
            value.len() == 64
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        }
        (!pending.original_key_id.trim().is_empty()
            && !pending.original_key_id.contains("[redacted]")
            && binding(&pending.candidate_binding)
            && matches!(pending.execution.mode.as_str(), "byo" | "operated")
            && !pending.execution.repository.trim().is_empty()
            && !pending.execution.backend.trim().is_empty()
            && binding(&pending.execution.credential_binding)
            && pending.setup_fingerprint.as_deref().is_some_and(binding))
        .then_some(pending)
    }
}

#[cfg(test)]
mod pending_revocation_tests {
    use super::*;

    fn intent(candidate: &str) -> PendingKeyRevocation {
        PendingKeyRevocation {
            original_key_id: "original".into(),
            candidate_binding: candidate_binding("daily", candidate).unwrap(),
            execution: PendingExecution {
                mode: "byo".into(),
                repository: "s3:example.invalid/bucket".into(),
                backend: "s3".into(),
                credential_binding: "b".repeat(64),
            },
            setup_fingerprint: Some("a".repeat(64)),
        }
    }

    fn config() -> Map<String, Value> {
        serde_json::json!({"backup":{"daily_key":"daily","recovery_key":"old", "confirmed_recovery_key":true}})
            .as_object().unwrap().clone()
    }

    #[test]
    fn malformed_present_intents_never_decode_as_retryable() {
        let valid = serde_json::to_value(intent("new")).unwrap();
        assert!(PendingKeyRevocation::parse(&valid).is_some());
        for malformed in [
            Value::Null,
            Value::Bool(false),
            serde_json::json!([]),
            serde_json::json!({}),
        ] {
            assert!(PendingKeyRevocation::parse(&malformed).is_none());
        }
        for field in ["candidate_binding", "setup_fingerprint", "original_key_id"] {
            let mut malformed = valid.clone();
            malformed[field] = Value::String(String::new());
            assert!(PendingKeyRevocation::parse(&malformed).is_none());
        }
        let mut missing = valid.clone();
        missing.as_object_mut().unwrap().remove("setup_fingerprint");
        assert!(PendingKeyRevocation::parse(&missing).is_none());
        let mut unknown = valid;
        unknown["unrecognized"] = Value::Bool(true);
        assert!(PendingKeyRevocation::parse(&unknown).is_none());
    }

    #[test]
    fn exact_restage_preserves_owner_confirmation_and_refuses_substitution() {
        let pending = intent("new");
        let mut staged = staged_revocation_configuration(&config(), "new", &pending).unwrap();
        assert_eq!(staged["backup"]["confirmed_recovery_key"], false);
        staged.get_mut("backup").unwrap()["confirmed_recovery_key"] = Value::Bool(true);
        let restaged = staged_revocation_configuration(&staged, "new", &pending).unwrap();
        assert_eq!(restaged["backup"]["confirmed_recovery_key"], true);
        assert!(staged_revocation_configuration(&staged, "other", &pending).is_none());
        assert!(staged_revocation_configuration(&staged, "other", &intent("other")).is_none());
        let mut malformed = config();
        malformed.get_mut("backup").unwrap()["pending_key_revocation"] = Value::Null;
        assert!(staged_revocation_configuration(&malformed, "new", &pending).is_none());
    }
}

/// Compute HMAC-SHA256 candidate binding of recovery key under daily key.
pub fn candidate_binding(daily_key: &str, recovery_key: &str) -> Option<String> {
    let mut inputs = serde_json::json!(["candidate-binding-v1", recovery_key]);
    inputs.sort_all_objects();
    let mut mac = Hmac::<Sha256>::new_from_slice(daily_key.as_bytes()).ok()?;
    mac.update(&serde_json::to_vec(&inputs).ok()?);
    Some(
        mac.finalize()
            .into_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
    )
}

/// Compute HMAC-SHA256 credential binding under daily key.
pub fn credential_binding(daily_key: &str, credentials: &Map<String, Value>) -> Option<String> {
    let mut inputs = serde_json::json!(["credential-binding-v1", credentials]);
    inputs.sort_all_objects();
    let mut mac = Hmac::<Sha256>::new_from_slice(daily_key.as_bytes()).ok()?;
    mac.update(&serde_json::to_vec(&inputs).ok()?);
    Some(
        mac.finalize()
            .into_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
    )
}

/// Stage pending key revocation and replacement recovery key beneath the retained root.
pub fn stage_pending_key_revocation_at_root(
    root: &Arc<JournalRoot>,
    expected_sha256: &str,
    candidate_key: &str,
    pending: &PendingKeyRevocation,
    expected_binding: Option<&HostedBinding>,
) -> Result<BoundConfigOutcome, BoundConfigRefusal> {
    update_pending_with_binding(root, expected_sha256, pending, expected_binding, |config| {
        staged_revocation_configuration(config, candidate_key, pending)
    })
}

fn update_pending_with_binding(
    root: &Arc<JournalRoot>,
    expected_sha256: &str,
    pending: &PendingKeyRevocation,
    expected_binding: Option<&HostedBinding>,
    update: impl FnOnce(&Map<String, Value>) -> Option<Map<String, Value>>,
) -> Result<BoundConfigOutcome, BoundConfigRefusal> {
    let mut exclusion = None;
    let mut result = solstone_core_journal_config_write::conditional_update_journal_config_at_root(
        root,
        expected_sha256,
        |config| {
            // This callback runs under config exclusion. Keep config→binding order,
            // and retain the binding guard until publication and confirmation return.
            if pending.execution.mode == "operated" {
                exclusion =
                    Some(crate::hold_rotation_binding_at_root(root, expected_binding?).ok()?);
            } else if expected_binding.is_some() {
                return None;
            }
            let backup = merge_backup_config(config);
            if backup_setup_fingerprint(&backup, expected_binding) != pending.setup_fingerprint {
                return None;
            }
            update(config)
        },
        mutation_lock_options(),
    );
    if let Some(held) = exclusion
        && let Err(error) = held.revalidate()
        && let Ok(BoundConfigOutcome::Published(publication)) = &mut result
    {
        publication.confirmation_refusal.get_or_insert(
            solstone_core_journal_config_write::BoundConfigConfirmationRefusal::Admission(
                Box::new(error),
            ),
        );
    }
    result
}

pub(crate) fn staged_revocation_configuration(
    config: &Map<String, Value>,
    candidate_key: &str,
    pending: &PendingKeyRevocation,
) -> Option<Map<String, Value>> {
    let mut replacement = config.clone();
    let mut backup = replacement
        .get("backup")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();

    if PendingKeyRevocation::parse(&serde_json::to_value(pending).ok()?).as_ref() != Some(pending)
        || candidate_binding(backup.get("daily_key")?.as_str()?, candidate_key).as_deref()
            != Some(pending.candidate_binding.as_str())
    {
        return None;
    }

    if let Some(existing_pending) = backup.get("pending_key_revocation") {
        let parsed = PendingKeyRevocation::parse(existing_pending)?;
        if parsed != *pending
            || backup.get("recovery_key").and_then(Value::as_str) != Some(candidate_key)
        {
            return None;
        }
    } else {
        backup.insert("confirmed_recovery_key".into(), Value::Bool(false));
    }

    backup.insert(
        "recovery_key".into(),
        Value::String(candidate_key.to_owned()),
    );
    backup.insert(
        "pending_key_revocation".into(),
        serde_json::to_value(pending).ok()?,
    );
    replacement.insert("backup".into(), Value::Object(backup));
    Some(replacement)
}

/// Clear pending key revocation intent beneath the retained root after confirmed remote revocation.
pub fn clear_pending_key_revocation_at_root(
    root: &Arc<JournalRoot>,
    expected_sha256: &str,
    expected_pending: &PendingKeyRevocation,
    expected_binding: Option<&HostedBinding>,
) -> Result<BoundConfigOutcome, BoundConfigRefusal> {
    update_pending_with_binding(
        root,
        expected_sha256,
        expected_pending,
        expected_binding,
        |config| cleared_revocation_configuration(config, expected_pending),
    )
}

pub(crate) fn cleared_revocation_configuration(
    config: &Map<String, Value>,
    expected_pending: &PendingKeyRevocation,
) -> Option<Map<String, Value>> {
    let mut replacement = config.clone();
    let mut backup = replacement
        .get("backup")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();

    let existing_pending = backup.get("pending_key_revocation")?;
    let parsed = PendingKeyRevocation::parse(existing_pending)?;
    if parsed != *expected_pending {
        return None;
    }
    if candidate_binding(
        backup.get("daily_key")?.as_str()?,
        backup.get("recovery_key")?.as_str()?,
    )
    .as_deref()
        != Some(expected_pending.candidate_binding.as_str())
    {
        return None;
    }
    backup.remove("pending_key_revocation");
    replacement.insert("backup".into(), Value::Object(backup));
    Some(replacement)
}

/// Opaque identity of the destination and backup key used by an admitted copy.
/// Schedule/history changes do not change it. Missing setup is unknown.
pub fn backup_setup_fingerprint(
    config: &Map<String, Value>,
    binding: Option<&HostedBinding>,
) -> Option<String> {
    let keys = build_keys(config.get("daily_key"), config.get("recovery_key")).ok()??;
    let target = match config.get("mode").and_then(Value::as_str)? {
        "byo" => {
            let destination = destination_from_config(config)?;
            serde_json::json!([
                "byo",
                destination.repository,
                destination.backend,
                destination.credentials
            ])
        }
        "operated" => {
            let b = binding?;
            serde_json::json!([
                "operated",
                b.broker_endpoint,
                b.account_id,
                b.instance_id,
                b.bucket,
                b.prefix,
                b.broker_token
            ])
        }
        _ => return None,
    };
    let mut inputs = serde_json::json!(["backup-setup-v1", target]);
    inputs.sort_all_objects();
    let mut mac = Hmac::<Sha256>::new_from_slice(keys.daily_key.as_bytes()).ok()?;
    mac.update(&serde_json::to_vec(&inputs).ok()?);
    Some(
        mac.finalize()
            .into_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
    )
}

pub fn record_backup_result(
    journal: &Path,
    status: &str,
    time: Value,
    snapshot_id: Value,
    error_reason: Value,
    detail: Option<&str>,
    setup_fingerprint: Option<&str>,
) -> Result<(), BackupError> {
    let mut map = Map::from_iter([
        ("time".into(), time),
        ("snapshot_id".into(), snapshot_id),
        ("status".into(), Value::String(status.into())),
        ("error_reason".into(), error_reason),
    ]);
    if let Some(detail) = detail {
        map.insert("detail".into(), Value::String(detail.into()));
    }
    if let Some(fingerprint) = setup_fingerprint {
        map.insert(
            "setup_fingerprint".into(),
            Value::String(fingerprint.into()),
        );
    }
    mutate_backup_section(journal, |backup| {
        // Keep the successful operation's whole result, including incomplete-copy
        // caveats. This is history, not proof of present coverage or verification.
        let last_success = if status == "ok" {
            Value::Object(map.clone())
        } else {
            backup
                .get("last_backup")
                .and_then(|prior| {
                    prior.get("last_success").or_else(|| {
                        (prior.get("status").and_then(Value::as_str) == Some("ok")).then_some(prior)
                    })
                })
                .cloned()
                .unwrap_or(Value::Null)
        };
        map.insert("last_success".into(), last_success);
        set_value(backup, "last_backup", Value::Object(map))
    })
}

pub fn record_prune_result(
    journal: &Path,
    status: &str,
    time: Value,
    error_reason: Value,
) -> Result<(), BackupError> {
    record(
        journal,
        "last_prune",
        Map::from_iter([
            ("time".into(), time),
            ("status".into(), Value::String(status.into())),
            ("error_reason".into(), error_reason),
        ]),
    )
}

pub fn record_offload_result(
    journal: &Path,
    status: &str,
    time: Value,
    reason: Value,
    files_marked: Value,
    bytes_marked: Value,
    ran_out_of_markable_media: Value,
) -> Result<(), BackupError> {
    if !OFFLOAD_STATUSES.contains(&status) {
        return Err(BackupError::InvalidOffloadStatus);
    }
    mutate_backup_section(journal, |backup| {
        let prior = backup.get("last_offload").and_then(Value::as_object);
        let last_ok = if status == "ok" {
            time.clone()
        } else {
            prior
                .and_then(|value| value.get("last_ok_time"))
                .cloned()
                .unwrap_or(Value::Null)
        };
        set_value(
            backup,
            "last_offload",
            Value::Object(Map::from_iter([
                ("time".into(), time),
                ("status".into(), Value::String(status.into())),
                ("reason".into(), reason),
                ("last_ok_time".into(), last_ok),
                ("files_marked".into(), files_marked),
                ("bytes_marked".into(), bytes_marked),
                (
                    "ran_out_of_markable_media".into(),
                    ran_out_of_markable_media,
                ),
            ])),
        )
    })
}

pub fn record_verification_result(
    journal: &Path,
    status: &str,
    time: Value,
    reason: Value,
    checked_subset: Value,
) -> Result<(), BackupError> {
    if !VERIFICATION_STATUSES.contains(&status) {
        return Err(BackupError::InvalidVerificationStatus);
    }
    mutate_backup_section(journal, |backup| {
        let prior = backup.get("last_verification").and_then(Value::as_object);
        let last_ok = if status == "ok" {
            time.clone()
        } else {
            prior
                .and_then(|value| value.get("last_ok_time"))
                .cloned()
                .unwrap_or(Value::Null)
        };
        let checked = if status == "ok" {
            checked_subset
        } else {
            Value::Null
        };
        set_value(
            backup,
            "last_verification",
            Value::Object(Map::from_iter([
                ("time".into(), time),
                ("status".into(), Value::String(status.into())),
                ("reason".into(), reason),
                ("last_ok_time".into(), last_ok),
                ("checked_subset".into(), checked),
            ])),
        )
    })
}

#[allow(clippy::too_many_arguments)]
pub fn record_restore_result(
    journal: &Path,
    status: &str,
    time: Value,
    reason: Value,
    scope: &str,
    day: Value,
    segments_selected: Value,
    segments_restored: Value,
    files_expected: Value,
    files_restored: Value,
    bytes_expected: Value,
    bytes_restored: Value,
) -> Result<(), BackupError> {
    if !RESTORE_STATUSES.contains(&status) {
        return Err(BackupError::InvalidRestoreStatus);
    }
    if !RESTORE_SCOPES.contains(&scope) {
        return Err(BackupError::InvalidRestoreScope);
    }
    if [&segments_selected, &segments_restored]
        .iter()
        .any(|value| !is_nonnegative_integer(Some(value)))
        || [
            &files_expected,
            &files_restored,
            &bytes_expected,
            &bytes_restored,
        ]
        .iter()
        .any(|value| !is_nonnegative_integer_or_null(Some(value)))
    {
        return Err(BackupError::InvalidRestoreCounters);
    }
    record(
        journal,
        "last_restore",
        Map::from_iter([
            ("time".into(), time),
            ("status".into(), Value::String(status.into())),
            ("reason".into(), reason),
            ("scope".into(), Value::String(scope.into())),
            ("day".into(), day),
            ("segments_selected".into(), segments_selected),
            ("segments_restored".into(), segments_restored),
            ("files_expected".into(), files_expected),
            ("files_restored".into(), files_restored),
            ("bytes_expected".into(), bytes_expected),
            ("bytes_restored".into(), bytes_restored),
        ]),
    )
}

pub fn status_view(journal: &Path) -> Result<Map<String, Value>, BackupError> {
    let config = get_backup_config(journal)?;
    let destination = config
        .get("destination")
        .and_then(Value::as_object)
        .ok_or(BackupError::InvalidDestinationShape)?;
    let hosted = match load_hosted_binding(journal) {
        Some(HostedBinding { bucket, prefix, .. }) => Map::from_iter([
            ("bound".into(), Value::Bool(true)),
            ("bucket".into(), Value::String(bucket)),
            ("prefix".into(), Value::String(prefix)),
        ]),
        None => Map::from_iter([("bound".into(), Value::Bool(false))]),
    };
    Ok(Map::from_iter([
        ("enabled".into(), config["enabled"].clone()),
        (
            "paused_at".into(),
            backup_paused_at(&config).map_or(Value::Null, Value::from),
        ),
        ("mode".into(), config["mode"].clone()),
        (
            "destination".into(),
            Value::Object(Map::from_iter([
                (
                    "repository".into(),
                    destination
                        .get("repository")
                        .cloned()
                        .unwrap_or(Value::Null),
                ),
                (
                    "backend".into(),
                    destination.get("backend").cloned().unwrap_or(Value::Null),
                ),
                (
                    "credentials_set".into(),
                    Value::Bool(destination.get("credentials").is_some_and(value_truthy)),
                ),
            ])),
        ),
        (
            "daily_key_set".into(),
            Value::Bool(!config["daily_key"].is_null()),
        ),
        (
            "recovery_key_set".into(),
            Value::Bool(!config["recovery_key"].is_null()),
        ),
        (
            "recovery_key_confirmed".into(),
            Value::Bool(config["confirmed_recovery_key"].as_bool().unwrap_or(false)),
        ),
        (
            "recovery_key_revocation_pending".into(),
            Value::Bool(
                config
                    .get("pending_key_revocation")
                    .and_then(PendingKeyRevocation::parse)
                    .is_some(),
            ),
        ),
        (
            "recovery_key_revocation_unresolved".into(),
            Value::Bool(config.contains_key("pending_key_revocation")),
        ),
        ("retention".into(), config["retention"].clone()),
        ("offload".into(), config["offload"].clone()),
        ("schedule".into(), config["schedule"].clone()),
        ("last_backup".into(), config["last_backup"].clone()),
        ("last_prune".into(), config["last_prune"].clone()),
        ("last_offload".into(), config["last_offload"].clone()),
        (
            "last_verification".into(),
            config["last_verification"].clone(),
        ),
        ("last_restore".into(), config["last_restore"].clone()),
        ("hosted".into(), Value::Object(hosted)),
    ]))
}

pub(crate) fn build_keys(
    daily: Option<&Value>,
    recovery: Option<&Value>,
) -> Result<Option<BackupKeys>, BackupError> {
    match (daily, recovery) {
        (Some(Value::Null) | None, _) | (_, Some(Value::Null) | None) => Ok(None),
        (Some(Value::String(daily_key)), Some(Value::String(recovery_key))) => {
            format_recovery_key_display(recovery_key)?;
            Ok(Some(BackupKeys {
                daily_key: daily_key.clone(),
                recovery_key: recovery_key.clone(),
            }))
        }
        _ => Err(BackupError::StoredKeys),
    }
}

fn record(journal: &Path, key: &str, value: Map<String, Value>) -> Result<(), BackupError> {
    mutate_backup_section(journal, |backup| {
        set_value(backup, key, Value::Object(value))
    })
}
fn set_value(
    backup: &mut Map<String, Value>,
    key: &str,
    value: Value,
) -> Result<(bool, ()), BackupError> {
    let changed = backup.get(key) != Some(&value);
    backup.insert(key.to_owned(), value);
    Ok((changed, ()))
}
fn is_nonnegative_integer(value: Option<&Value>) -> bool {
    value
        .and_then(Value::as_i64)
        .is_some_and(|value| value >= 0)
}
fn is_nonnegative_integer_or_null(value: Option<&Value>) -> bool {
    matches!(value, Some(Value::Null)) || is_nonnegative_integer(value)
}
fn is_positive_or_null(value: Option<&Value>) -> bool {
    matches!(value, Some(Value::Null))
        || value.and_then(Value::as_i64).is_some_and(|value| value > 0)
}

fn value_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(value) => value.as_i64() != Some(0) && value.as_u64() != Some(0),
        Value::String(value) => !value.is_empty(),
        Value::Array(value) => !value.is_empty(),
        Value::Object(value) => !value.is_empty(),
    }
}

type BackupTaskCheckpoint = Arc<dyn Fn() -> Result<(), String> + Send + Sync>;

/// Install `checkpoint` for backup-owner writes on this thread until `body` returns.
///
/// The scheduled queue deadline is not the shared backup-route lifetime. Writers
/// called from the embedded run admit this checkpoint after their own lock and
/// immediately before the bytes change. An unset slot admits the write, so a
/// desktop caller that never installs a checkpoint keeps the current behavior.
pub fn with_backup_task_checkpoint<R>(
    checkpoint: Arc<dyn Fn() -> Result<(), String> + Send + Sync>,
    body: impl FnOnce() -> R,
) -> R {
    struct Guard(Option<BackupTaskCheckpoint>);
    impl Drop for Guard {
        fn drop(&mut self) {
            BACKUP_TASK_CHECKPOINT.with(|slot| *slot.borrow_mut() = self.0.take());
        }
    }
    let previous = BACKUP_TASK_CHECKPOINT.with(|slot| slot.borrow_mut().replace(checkpoint));
    let _guard = Guard(previous);
    body()
}

thread_local! {
    static BACKUP_TASK_CHECKPOINT: RefCell<Option<BackupTaskCheckpoint>> = const { RefCell::new(None) };
}

pub(crate) fn admit_backup_task() -> Result<(), String> {
    BACKUP_TASK_CHECKPOINT.with(|slot| match slot.borrow().as_ref() {
        Some(checkpoint) => checkpoint(),
        None => Ok(()),
    })
}

fn mutate_backup_section<T>(
    journal: &Path,
    operation: impl FnOnce(&mut Map<String, Value>) -> Result<(bool, T), BackupError>,
) -> Result<T, BackupError> {
    let admit = || admit_backup_task();
    let transaction =
        mutate_journal_config_checked(journal, mutation_lock_options(), &admit, |config| {
            run_mutation_hook();
            let backup = config
                .entry("backup".to_owned())
                .or_insert_with(|| Value::Object(Map::new()));
            let replaced_non_object = !backup.is_object();
            if replaced_non_object {
                *backup = Value::Object(Map::new());
            }
            let result = operation(backup.as_object_mut().expect("object assigned"));
            match result {
                Ok((changed, value)) => JournalConfigMutation {
                    changed: changed || replaced_non_object,
                    value: Ok(value),
                },
                Err(error) => JournalConfigMutation {
                    changed: false,
                    value: Err(error),
                },
            }
        })
        .map_err(BackupError::ConfigMutation)?;
    transaction.value
}

#[cfg(test)]
use std::rc::Rc;
#[cfg(test)]
thread_local! { static MUTATION_HOOK: RefCell<Option<Rc<dyn Fn()>>> = const { RefCell::new(None) }; }
#[cfg(test)]
pub(crate) struct MutationHookGuard(Option<Rc<dyn Fn()>>);
#[cfg(test)]
impl Drop for MutationHookGuard {
    fn drop(&mut self) {
        MUTATION_HOOK.with(|hook| *hook.borrow_mut() = self.0.take());
    }
}
#[cfg(test)]
pub(crate) fn install_mutation_hook(hook: Rc<dyn Fn()>) -> MutationHookGuard {
    MutationHookGuard(MUTATION_HOOK.with(|current| current.replace(Some(hook))))
}
#[cfg(test)]
fn run_mutation_hook() {
    MUTATION_HOOK.with(|hook| {
        if let Some(hook) = hook.borrow().as_ref() {
            hook();
        }
    });
}
#[cfg(not(test))]
fn run_mutation_hook() {}
pub(crate) fn mutation_lock_options() -> LockOptions {
    LockOptions::default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;
    use std::sync::{
        Arc, Barrier,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    };
    use std::thread;

    fn journal() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }
    fn write_config(root: &Path, value: Value) {
        let path = root.join("config/journal.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
    }

    fn config_path(root: &Path) -> std::path::PathBuf {
        root.join("config/journal.json")
    }

    fn assert_rejection_preserves_config(
        journal: &tempfile::TempDir,
        call: impl FnOnce() -> Result<(), BackupError>,
    ) {
        let before = fs::read(config_path(journal.path())).unwrap();
        assert!(call().is_err());
        assert_eq!(fs::read(config_path(journal.path())).unwrap(), before);
    }

    fn restore_with_counters(journal: &Path, counters: [Value; 6]) -> Result<(), BackupError> {
        record_restore_result(
            journal,
            "ok",
            Value::Null,
            Value::Null,
            "day",
            Value::Null,
            counters[0].clone(),
            counters[1].clone(),
            counters[2].clone(),
            counters[3].clone(),
            counters[4].clone(),
            counters[5].clone(),
        )
    }

    #[test]
    fn reads_merged_defaults_without_creating_config() {
        let journal = journal();
        let config = get_backup_config(journal.path()).unwrap();
        assert_eq!(config, backup_defaults());
        assert!(!journal.path().join("config").exists());
        assert_eq!(get_destination(journal.path()).unwrap(), None);
        assert_eq!(get_keys(journal.path()).unwrap(), None);
    }
    #[test]
    fn destination_requires_strings_and_object() {
        let journal = journal();
        write_config(
            journal.path(),
            json!({"backup":{"destination":{"repository":1,"backend":"s3","credentials":{}}}}),
        );
        assert_eq!(get_destination(journal.path()).unwrap(), None);
    }

    #[test]
    fn typed_setters_round_trip_without_extra_validation() {
        let journal = journal();
        write_config(
            journal.path(),
            json!({"backup": {"unknown_backup_key": "kept"}, "outside": "kept"}),
        );
        let destination = Destination {
            repository: "any repository".into(),
            backend: "any backend".into(),
            credentials: serde_json::from_value(json!({"unvalidated": [1]})).unwrap(),
        };
        set_destination(journal.path(), &destination).unwrap();
        set_enabled(journal.path(), true).unwrap();
        set_recovery_key(journal.path(), "unvalidated recovery key").unwrap();
        set_recovery_key_confirmed(journal.path(), true).unwrap();
        let raw: Value =
            serde_json::from_slice(&fs::read(config_path(journal.path())).unwrap()).unwrap();
        assert_eq!(raw["outside"], "kept");
        assert_eq!(raw["backup"]["unknown_backup_key"], "kept");
        assert_eq!(raw["backup"]["destination"]["repository"], "any repository");
        assert_eq!(raw["backup"]["enabled"], true);
        assert_eq!(raw["backup"]["recovery_key"], "unvalidated recovery key");
        assert_eq!(raw["backup"]["confirmed_recovery_key"], true);
    }

    #[test]
    fn set_mode_accepts_closed_vocabulary_and_rejects_without_mutating() {
        let journal = journal();
        write_config(journal.path(), json!({"backup": {}}));
        set_mode(journal.path(), "byo").unwrap();
        set_mode(journal.path(), "operated").unwrap();
        assert_rejection_preserves_config(&journal, || set_mode(journal.path(), "invalid"));
    }

    #[test]
    fn set_retention_accepts_exact_valid_shape_and_rejects_invalid_shapes_and_values() {
        let journal = journal();
        write_config(journal.path(), json!({"backup": {}}));
        let valid =
            serde_json::from_value(json!({"hourly": 0, "daily": 1, "weekly": 2, "monthly": 3}))
                .unwrap();
        set_retention(journal.path(), &valid).unwrap();
        for invalid in [
            json!({"hourly": 0, "daily": 1, "weekly": 2}),
            json!({"hourly": 0, "daily": 1, "weekly": 2, "monthly": 3, "extra": 4}),
            json!({"hourly": true, "daily": 1, "weekly": 2, "monthly": 3}),
            json!({"hourly": -1, "daily": 1, "weekly": 2, "monthly": 3}),
        ] {
            let invalid = serde_json::from_value(invalid).unwrap();
            assert_rejection_preserves_config(&journal, || set_retention(journal.path(), &invalid));
        }
    }

    #[test]
    fn set_offload_accepts_null_or_positive_bytes_and_rejects_invalid_values() {
        let journal = journal();
        write_config(journal.path(), json!({"backup": {}}));
        let valid = serde_json::from_value(
            json!({"enabled": true, "budget_bytes": null, "floor_bytes": 1}),
        )
        .unwrap();
        set_offload(journal.path(), &valid).unwrap();
        for invalid in [
            json!({"enabled": true, "budget_bytes": 1}),
            json!({"enabled": true, "budget_bytes": 1, "floor_bytes": 2, "extra": 3}),
            json!({"enabled": 1, "budget_bytes": 1, "floor_bytes": 2}),
            json!({"enabled": true, "budget_bytes": true, "floor_bytes": 2}),
            json!({"enabled": true, "budget_bytes": 0, "floor_bytes": 2}),
            json!({"enabled": true, "budget_bytes": -1, "floor_bytes": 2}),
        ] {
            let invalid = serde_json::from_value(invalid).unwrap();
            assert_rejection_preserves_config(&journal, || set_offload(journal.path(), &invalid));
        }
    }

    #[test]
    fn clear_backup_config_preserves_top_level_keys_and_is_idempotent() {
        let journal = journal();
        write_config(
            journal.path(),
            json!({"outside": {"kept": true}, "backup": {"enabled": true, "future": "discarded"}}),
        );
        clear_backup_config(journal.path()).unwrap();
        let raw: Value =
            serde_json::from_slice(&fs::read(config_path(journal.path())).unwrap()).unwrap();
        assert_eq!(raw["outside"], json!({"kept": true}));
        assert_eq!(raw["backup"], Value::Object(backup_defaults()));
        let before = fs::read(config_path(journal.path())).unwrap();
        clear_backup_config(journal.path()).unwrap();
        assert_eq!(fs::read(config_path(journal.path())).unwrap(), before);
    }
    #[test]
    fn record_backup_and_prune_accept_arbitrary_status_strings() {
        let journal = journal();
        write_config(journal.path(), json!({"backup":{}}));
        record_backup_result(
            journal.path(),
            "anything",
            json!(5),
            json!("snap"),
            Value::Null,
            None,
            None,
        )
        .unwrap();
        record_prune_result(journal.path(), "anything", json!(6), Value::Null).unwrap();
        let config = get_backup_config(journal.path()).unwrap();
        assert_eq!(config["last_backup"]["status"], "anything");
        assert!(
            !config["last_backup"]
                .as_object()
                .unwrap()
                .contains_key("detail")
        );
        assert_eq!(config["last_prune"]["status"], "anything");

        record_backup_result(
            journal.path(),
            "error",
            json!(6),
            Value::Null,
            json!("reason"),
            Some("detail-code"),
            None,
        )
        .unwrap();
        let config2 = get_backup_config(journal.path()).unwrap();
        assert_eq!(config2["last_backup"]["detail"], "detail-code");
    }

    #[test]
    fn record_offload_rejects_unknown_status_without_mutating() {
        let journal = journal();
        write_config(journal.path(), json!({"backup": {}}));
        assert_rejection_preserves_config(&journal, || {
            record_offload_result(
                journal.path(),
                "unknown",
                json!(1),
                Value::Null,
                json!(1),
                json!(2),
                json!(false),
            )
        });
    }

    #[test]
    fn backup_failures_preserve_success_and_its_incomplete_copy_caveat() {
        let journal = journal();
        record_backup_result(
            journal.path(),
            "ok",
            json!(100),
            json!("first"),
            json!("incomplete"),
            Some("unreadable-source"),
            None,
        )
        .unwrap();
        let success =
            get_backup_config(journal.path()).unwrap()["last_backup"]["last_success"].clone();
        assert_eq!(success["snapshot_id"], "first");
        assert_eq!(success["error_reason"], "incomplete");
        assert_eq!(success["detail"], "unreadable-source");
        assert!(
            success.get("last_success").is_none(),
            "history never nests itself"
        );
        for time in [200, 300] {
            record_backup_result(
                journal.path(),
                "error",
                json!(time),
                Value::Null,
                json!("timeout"),
                None,
                None,
            )
            .unwrap();
            let view = status_view(journal.path()).unwrap();
            assert_eq!(view["last_backup"]["time"], time);
            assert_eq!(view["last_backup"]["status"], "error");
            assert_eq!(view["last_backup"]["last_success"], success);
        }
        record_backup_result(
            journal.path(),
            "ok",
            json!(400),
            json!("next"),
            Value::Null,
            None,
            None,
        )
        .unwrap();
        let view = status_view(journal.path()).unwrap();
        assert_eq!(view["last_backup"]["last_success"]["snapshot_id"], "next");
        assert_eq!(view["last_backup"]["last_success"]["time"], 400);
        assert!(view["last_backup"]["last_success"]["error_reason"].is_null());
        assert!(view["last_backup"]["last_success"].get("detail").is_none());
    }

    #[test]
    fn setup_identity_covers_copy_inputs_and_ignores_object_order_and_schedule() {
        let mut config = serde_json::json!({"mode":"byo", "daily_key":"daily",
            "recovery_key":"0123456789ABCDEFGHJKMNPQRSTVWXYZ0123456789ABCDEFGHJKMNPQRSTVWXYZ",
            "destination":{"repository":"s3:repo","backend":"s3","credentials":{"a":"one","b":{"c":"two","d":"three"}}}
        }).as_object().unwrap().clone();
        let fingerprint = backup_setup_fingerprint(&config, None).unwrap();
        config["destination"]["credentials"] =
            serde_json::from_str(r#"{"b":{"d":"three","c":"two"},"a":"one"}"#).unwrap();
        config.insert("schedule".into(), serde_json::json!({"every":300}));
        config.insert("last_backup".into(), serde_json::json!({"status":"ok"}));
        assert_eq!(
            backup_setup_fingerprint(&config, None).unwrap(),
            fingerprint
        );
        let mut changed = config.clone();
        changed.insert("daily_key".into(), serde_json::json!("different"));
        assert_ne!(
            backup_setup_fingerprint(&changed, None).unwrap(),
            fingerprint
        );
        changed = config.clone();
        changed.insert(
            "recovery_key".into(),
            serde_json::json!("1123456789ABCDEFGHJKMNPQRSTVWXYZ0123456789ABCDEFGHJKMNPQRSTVWXYZ"),
        );
        assert_eq!(
            backup_setup_fingerprint(&changed, None).unwrap(),
            fingerprint,
            "recovery key is not used by a copy"
        );
        config.insert("mode".into(), serde_json::json!("operated"));
        let binding = HostedBinding {
            broker_endpoint: "https://broker".into(),
            account_id: "account".into(),
            instance_id: "instance".into(),
            bucket: "bucket".into(),
            prefix: "prefix".into(),
            broker_token: "token".into(),
        };
        let operated = backup_setup_fingerprint(&config, Some(&binding)).unwrap();
        assert_ne!(operated, fingerprint);
        assert!(backup_setup_fingerprint(&config, None).is_none());
        for field in 0..6 {
            let mut changed = binding.clone();
            match field {
                0 => changed.broker_endpoint.push('x'),
                1 => changed.account_id.push('x'),
                2 => changed.instance_id.push('x'),
                3 => changed.bucket.push('x'),
                4 => changed.prefix.push('x'),
                _ => changed.broker_token.push('x'),
            }
            assert_ne!(
                backup_setup_fingerprint(&config, Some(&changed)).unwrap(),
                operated
            );
        }
    }

    #[test]
    fn backup_history_keeps_intact_legacy_success_but_does_not_invent_lost_history() {
        let journal = journal();
        for prior in [
            json!({"status":"ok", "time":100, "snapshot_id":"legacy", "error_reason":"incomplete"}),
            json!({"status":"error", "time":100}),
            json!({"status":"ok", "time":100, "last_success":null}),
            Value::Null,
        ] {
            write_config(journal.path(), json!({"backup":{"last_backup":prior}}));
            record_backup_result(
                journal.path(),
                "error",
                json!(200),
                Value::Null,
                json!("locked"),
                None,
                None,
            )
            .unwrap();
            let saved = get_backup_config(journal.path()).unwrap()["last_backup"].clone();
            if prior.get("snapshot_id").is_some() {
                assert_eq!(saved["last_success"], prior);
            } else {
                assert!(saved["last_success"].is_null());
            }
        }
        clear_backup_config(journal.path()).unwrap();
        assert!(
            get_backup_config(journal.path()).unwrap()["last_backup"]
                .get("last_success")
                .is_none()
        );
    }

    #[test]
    fn record_offload_keeps_caller_owned_counter_values_and_last_ok_rules() {
        let journal = journal();
        write_config(
            journal.path(),
            json!({"backup": {"last_offload": {"last_ok_time": 99}}}),
        );
        record_offload_result(
            journal.path(),
            "ok",
            json!(1),
            Value::Null,
            json!(false),
            json!(-1),
            json!("not-bool"),
        )
        .unwrap();
        let config = get_backup_config(journal.path()).unwrap();
        assert_eq!(config["last_offload"]["last_ok_time"], 1);
        assert_eq!(config["last_offload"]["files_marked"], false);
        assert_eq!(config["last_offload"]["bytes_marked"], -1);
        assert_eq!(
            config["last_offload"]["ran_out_of_markable_media"],
            "not-bool"
        );
        record_offload_result(
            journal.path(),
            "ok",
            Value::Null,
            Value::Null,
            json!(0),
            json!(0),
            json!(false),
        )
        .unwrap();
        assert_eq!(
            get_backup_config(journal.path()).unwrap()["last_offload"]["last_ok_time"],
            Value::Null
        );
        record_offload_result(
            journal.path(),
            "ok",
            json!(7),
            Value::Null,
            json!(0),
            json!(0),
            json!(false),
        )
        .unwrap();
        record_offload_result(
            journal.path(),
            "stalled",
            json!(8),
            Value::Null,
            json!(0),
            json!(0),
            json!(false),
        )
        .unwrap();
        assert_eq!(
            get_backup_config(journal.path()).unwrap()["last_offload"]["last_ok_time"],
            7
        );
    }

    #[test]
    fn record_verification_rejects_unknown_status_without_mutating() {
        let journal = journal();
        write_config(journal.path(), json!({"backup": {}}));
        assert_rejection_preserves_config(&journal, || {
            record_verification_result(
                journal.path(),
                "unknown",
                json!(1),
                Value::Null,
                json!("1/1"),
            )
        });
    }

    #[test]
    fn record_verification_replaces_or_preserves_last_ok_and_checked_subset_as_required() {
        let journal = journal();
        write_config(
            journal.path(),
            json!({"backup": {"last_verification": {"last_ok_time": 99, "checked_subset": "old"}}}),
        );
        record_verification_result(journal.path(), "ok", json!(1), Value::Null, json!("new"))
            .unwrap();
        let config = get_backup_config(journal.path()).unwrap();
        assert_eq!(config["last_verification"]["last_ok_time"], 1);
        assert_eq!(config["last_verification"]["checked_subset"], "new");
        record_verification_result(journal.path(), "ok", Value::Null, Value::Null, json!("all"))
            .unwrap();
        assert_eq!(
            get_backup_config(journal.path()).unwrap()["last_verification"]["last_ok_time"],
            Value::Null
        );
        record_verification_result(journal.path(), "ok", json!(7), Value::Null, json!("kept"))
            .unwrap();
        record_verification_result(
            journal.path(),
            "error",
            json!(8),
            json!("reason"),
            json!("cleared"),
        )
        .unwrap();
        let config = get_backup_config(journal.path()).unwrap();
        assert_eq!(config["last_verification"]["last_ok_time"], 7);
        assert_eq!(config["last_verification"]["checked_subset"], Value::Null);
    }

    #[test]
    fn record_restore_rejects_status_and_scope_without_mutating() {
        let journal = journal();
        write_config(journal.path(), json!({"backup": {}}));
        assert_rejection_preserves_config(&journal, || {
            record_restore_result(
                journal.path(),
                "unknown",
                Value::Null,
                Value::Null,
                "day",
                Value::Null,
                json!(0),
                json!(0),
                json!(0),
                json!(0),
                json!(0),
                json!(0),
            )
        });
        assert_rejection_preserves_config(&journal, || {
            record_restore_result(
                journal.path(),
                "ok",
                Value::Null,
                Value::Null,
                "month",
                Value::Null,
                json!(0),
                json!(0),
                json!(0),
                json!(0),
                json!(0),
                json!(0),
            )
        });
    }

    #[test]
    fn record_restore_rejects_each_counter_kind_without_mutating() {
        let journal = journal();
        write_config(journal.path(), json!({"backup": {}}));
        for index in 0..6 {
            for invalid in [json!(true), json!(1.5), json!("3"), json!(-1)] {
                let mut counters = [json!(0), json!(0), json!(0), json!(0), json!(0), json!(0)];
                counters[index] = invalid;
                assert_rejection_preserves_config(&journal, || {
                    restore_with_counters(journal.path(), counters)
                });
            }
        }
    }

    #[test]
    fn record_restore_allows_null_journal_file_counters_but_not_segment_counters() {
        let journal = journal();
        write_config(journal.path(), json!({"backup": {}}));
        record_restore_result(
            journal.path(),
            "degraded",
            json!(7),
            json!("restore_summary_missing"),
            "journal",
            Value::Null,
            json!(0),
            json!(0),
            Value::Null,
            Value::Null,
            Value::Null,
            Value::Null,
        )
        .unwrap();
        let restored = &get_backup_config(journal.path()).unwrap()["last_restore"];
        assert_eq!(restored["scope"], "journal");
        assert_eq!(restored["files_expected"], Value::Null);
        assert_eq!(restored["bytes_restored"], Value::Null);

        assert_rejection_preserves_config(&journal, || {
            record_restore_result(
                journal.path(),
                "error",
                json!(8),
                json!("restore_failed"),
                "journal",
                Value::Null,
                Value::Null,
                json!(0),
                Value::Null,
                Value::Null,
                Value::Null,
                Value::Null,
            )
        });
    }

    #[cfg(unix)]
    #[test]
    fn matching_mutation_is_a_metadata_preserving_no_op() {
        use std::os::unix::fs::MetadataExt;

        let journal = journal();
        write_config(
            journal.path(),
            json!({"outside": "kept", "backup": {"enabled": true, "future": "kept"}}),
        );
        let path = config_path(journal.path());
        let before = fs::read(&path).unwrap();
        let metadata = fs::metadata(&path).unwrap();
        set_enabled(journal.path(), true).unwrap();
        let after = fs::metadata(&path).unwrap();
        assert_eq!(fs::read(&path).unwrap(), before);
        assert_eq!(after.ino(), metadata.ino());
        assert_eq!(after.mode(), metadata.mode());
        assert_eq!(after.modified().unwrap(), metadata.modified().unwrap());
    }

    #[test]
    fn mutation_preserves_unrelated_top_level_and_backup_keys() {
        let journal = journal();
        write_config(
            journal.path(),
            json!({"outside": {"kept": true}, "backup": {"future": "kept", "daily_key": "daily"}}),
        );
        set_enabled(journal.path(), true).unwrap();
        let raw: Value =
            serde_json::from_slice(&fs::read(config_path(journal.path())).unwrap()).unwrap();
        assert_eq!(raw["outside"], json!({"kept": true}));
        assert_eq!(raw["backup"]["future"], "kept");
        assert_eq!(raw["backup"]["daily_key"], "daily");
    }

    #[test]
    fn disjoint_mutations_survive_deterministic_contention() {
        let journal = tempfile::tempdir().unwrap();
        write_config(journal.path(), json!({"backup": {}}));
        let root = journal.path().to_owned();
        let start = Arc::new(Barrier::new(3));
        let release = Arc::new(Barrier::new(2));
        let first_hook = Arc::new(AtomicBool::new(true));
        let (entered_tx, entered_rx) = mpsc::channel();
        let (started_tx, started_rx) = mpsc::channel();
        let enabled_root = root.clone();
        let enabled_start = Arc::clone(&start);
        let enabled_release = Arc::clone(&release);
        let enabled_first = Arc::clone(&first_hook);
        let enabled_entered = entered_tx.clone();
        let enabled_started = started_tx.clone();
        let enabled = thread::spawn(move || {
            let _guard = install_mutation_hook(Rc::new(move || {
                enabled_entered.send(()).unwrap();
                if enabled_first.swap(false, Ordering::SeqCst) {
                    enabled_release.wait();
                }
            }));
            enabled_started.send(()).unwrap();
            enabled_start.wait();
            set_enabled(&enabled_root, true).unwrap();
        });
        let mode_root = root.clone();
        let mode_start = Arc::clone(&start);
        let mode_release = Arc::clone(&release);
        let mode_first = Arc::clone(&first_hook);
        let mode_entered = entered_tx;
        let mode_started = started_tx;
        let mode = thread::spawn(move || {
            let _guard = install_mutation_hook(Rc::new(move || {
                mode_entered.send(()).unwrap();
                if mode_first.swap(false, Ordering::SeqCst) {
                    mode_release.wait();
                }
            }));
            mode_started.send(()).unwrap();
            mode_start.wait();
            set_mode(&mode_root, "operated").unwrap();
        });
        started_rx.recv().unwrap();
        started_rx.recv().unwrap();
        start.wait();
        entered_rx.recv().unwrap();
        release.wait();
        enabled.join().unwrap();
        mode.join().unwrap();
        entered_rx.recv().unwrap();
        let config = get_backup_config(&root).unwrap();
        assert_eq!(config["enabled"], true);
        assert_eq!(config["mode"], "operated");
    }

    #[test]
    #[cfg(unix)]
    fn key_generation_returns_existing_pair_without_publishing() {
        use std::os::unix::fs::MetadataExt;

        let journal = journal();
        let first = generate_and_store_keys(journal.path()).unwrap();
        let config = config_path(journal.path());
        let bytes = fs::read(&config).unwrap();
        let metadata = fs::metadata(&config).unwrap();
        let inode = metadata.ino();
        let mode = metadata.mode();
        let modified = metadata.modified().unwrap();
        let second = generate_and_store_keys(journal.path()).unwrap();
        assert_eq!(second, first);
        assert_eq!(fs::read(&config).unwrap(), bytes);
        let after = fs::metadata(&config).unwrap();
        assert_eq!(after.ino(), inode);
        assert_eq!(after.mode(), mode);
        assert_eq!(after.modified().unwrap(), modified);
    }

    #[cfg(unix)]
    #[test]
    fn key_generation_fills_only_missing_key_with_one_publication() {
        use std::os::unix::fs::MetadataExt;

        let one_missing = tempfile::tempdir().unwrap();
        write_config(
            one_missing.path(),
            json!({"backup":{"daily_key":"existing"}}),
        );
        let config = config_path(one_missing.path());
        let before = fs::metadata(&config).unwrap();
        let transactions = Arc::new(AtomicUsize::new(0));
        let transaction_counter = Arc::clone(&transactions);
        let _guard = install_mutation_hook(Rc::new(move || {
            transaction_counter.fetch_add(1, Ordering::SeqCst);
        }));
        let keys = generate_and_store_keys(one_missing.path()).unwrap();
        assert_eq!(keys.daily_key, "existing");
        assert!(!keys.recovery_key.is_empty());
        assert_eq!(get_keys(one_missing.path()).unwrap(), Some(keys));
        assert_eq!(transactions.load(Ordering::SeqCst), 1);
        let after = fs::metadata(&config).unwrap();
        assert_ne!(after.ino(), before.ino());
    }

    #[test]
    fn concurrent_key_generation_converges_on_one_persisted_pair() {
        let concurrent = tempfile::tempdir().unwrap();
        let root = concurrent.path().to_owned();
        let start = Arc::new(Barrier::new(3));
        let release = Arc::new(Barrier::new(2));
        let first_hook = Arc::new(AtomicBool::new(true));
        let (entered_tx, entered_rx) = mpsc::channel();
        let (started_tx, started_rx) = mpsc::channel();
        let mut workers = Vec::new();
        for _ in 0..2 {
            let root = root.clone();
            let start = Arc::clone(&start);
            let release = Arc::clone(&release);
            let first_hook = Arc::clone(&first_hook);
            let entered_tx = entered_tx.clone();
            let started_tx = started_tx.clone();
            workers.push(thread::spawn(move || {
                let _guard = install_mutation_hook(Rc::new(move || {
                    entered_tx.send(()).unwrap();
                    if first_hook.swap(false, Ordering::SeqCst) {
                        release.wait();
                    }
                }));
                started_tx.send(()).unwrap();
                start.wait();
                generate_and_store_keys(&root).unwrap()
            }));
        }
        started_rx.recv().unwrap();
        started_rx.recv().unwrap();
        start.wait();
        entered_rx.recv().unwrap();
        release.wait();
        let left = workers.remove(0).join().unwrap();
        let right = workers.remove(0).join().unwrap();
        entered_rx.recv().unwrap();
        assert_eq!(left, right);
        assert_eq!(get_keys(&root).unwrap(), Some(left));
    }

    #[cfg(unix)]
    #[test]
    fn lock_acquisition_failure_does_not_publish() {
        use std::os::unix::fs::symlink;

        let journal = journal();
        write_config(journal.path(), json!({"backup": {}}));
        let config = journal.path().join("config/journal.json");
        let before = fs::read(&config).unwrap();
        let sentinel = journal.path().join("sentinel");
        fs::write(&sentinel, b"sentinel").unwrap();
        symlink(&sentinel, journal.path().join("config/journal.json.lock")).unwrap();
        assert!(set_enabled(journal.path(), true).is_err());
        assert_eq!(fs::read(&config).unwrap(), before);
    }
}

#[cfg(all(test, feature = "full-tests"))]
mod conditional_clear_tests {
    use super::*;
    use serde_json::json;
    use std::{fs, rc::Rc};

    fn initial() -> Value {
        json!({
            "backup": {
                "enabled":true,
                "daily_key":"original-key",
                "destination":{"repository":"s3:bucket/repo","credentials":{"secret":"original-secret"}},
                "future":{"nested":11}
            },
            "retention":{"raw_media":"keep"},
            "other":{"before":1}
        })
    }

    fn write(root: &Path, value: &Value) -> Vec<u8> {
        fs::create_dir_all(root.join("config")).unwrap();
        let bytes = serde_json::to_vec(value).unwrap();
        fs::write(root.join("config/journal.json"), &bytes).unwrap();
        bytes
    }

    #[test]
    fn conditional_clear_matches_merged_legacy_and_preserves_unrelated_config() {
        let root = tempfile::tempdir().unwrap();
        let mut value = initial();
        write(root.path(), &value);
        let expected = get_backup_config(root.path()).unwrap();
        assert_eq!(expected["daily_key"], "original-key");
        assert_eq!(expected["future"]["nested"], 11);
        assert_eq!(expected["retention"]["hourly"], 24);
        assert!(expected["destination"]["backend"].is_null());
        value["other"]["after"] = json!(2);
        write(root.path(), &value);

        assert!(clear_backup_config_if_unchanged(root.path(), &expected).unwrap());
        value["backup"] = Value::Object(backup_defaults());
        let actual: Value =
            serde_json::from_slice(&fs::read(root.path().join("config/journal.json")).unwrap())
                .unwrap();
        assert_eq!(actual, value);
    }

    #[test]
    fn conditional_clear_refuses_each_changed_key_credential_and_unknown_field() {
        for (pointer, changed) in [
            ("/backup/daily_key", json!("new-key")),
            (
                "/backup/destination/credentials/secret",
                json!("new-secret"),
            ),
            ("/backup/future/nested", json!(12)),
        ] {
            let root = tempfile::tempdir().unwrap();
            let mut value = initial();
            write(root.path(), &value);
            let expected = get_backup_config(root.path()).unwrap();
            *value.pointer_mut(pointer).unwrap() = changed;
            let before = write(root.path(), &value);

            assert!(!clear_backup_config_if_unchanged(root.path(), &expected).unwrap());
            assert_eq!(
                fs::read(root.path().join("config/journal.json")).unwrap(),
                before
            );
        }
    }

    #[test]
    fn conditional_clear_leaves_missing_directory_and_corrupt_config_untouched() {
        let root = tempfile::tempdir().unwrap();
        assert!(!clear_backup_config_if_unchanged(root.path(), &backup_defaults()).unwrap());
        assert!(!root.path().join("config").exists());
        assert!(!root.path().join("config/journal.json.lock").exists());

        fs::create_dir(root.path().join("config")).unwrap();
        let path = root.path().join("config/journal.json");
        fs::write(&path, b"{broken").unwrap();
        assert!(matches!(
            clear_backup_config_if_unchanged(root.path(), &backup_defaults()),
            Err(BackupError::ConfigLoad(_))
        ));
        assert_eq!(fs::read(path).unwrap(), b"{broken");
        assert!(!root.path().join("config/journal.json.lock").exists());
    }

    #[test]
    fn conditional_clear_cas_preserves_newer_disappeared_and_malformed_state() {
        for change in ["backup", "unrelated", "disappeared", "malformed"] {
            let root = tempfile::tempdir().unwrap();
            let mut value = initial();
            write(root.path(), &value);
            let expected = get_backup_config(root.path()).unwrap();
            let replacement = match change {
                "backup" => {
                    value["backup"]["daily_key"] = json!("replacement-key");
                    Some(serde_json::to_vec(&value).unwrap())
                }
                "unrelated" => {
                    value["other"]["after"] = json!(2);
                    Some(serde_json::to_vec(&value).unwrap())
                }
                "malformed" => Some(b"{replacement-broken".to_vec()),
                _ => None,
            };
            let path = root.path().join("config/journal.json");
            let hook_path = path.clone();
            let hook_replacement = replacement.clone();
            let _hook = install_mutation_hook(Rc::new(move || match &hook_replacement {
                Some(bytes) => fs::write(&hook_path, bytes).unwrap(),
                None => fs::remove_file(&hook_path).unwrap(),
            }));

            let result = clear_backup_config_if_unchanged(root.path(), &expected);
            if change == "malformed" {
                assert!(matches!(result, Err(BackupError::ConfigLoad(_))));
            } else {
                assert!(!result.unwrap());
            }
            match replacement {
                Some(bytes) => assert_eq!(fs::read(path).unwrap(), bytes),
                None => assert!(!path.exists(), "refusal must not materialize config"),
            }
        }
    }
}

#[cfg(all(test, feature = "full-tests"))]
mod retained_clear_tests {
    use super::*;

    #[cfg(unix)]
    mod unix {
        use super::*;
        use serde_json::json;
        use solstone_core_journal_config_write::BoundConfigOutcome;
        use solstone_core_journal_io::{
            BoundPublicationPrimitive, is_publication_candidate_name,
            run_with_bound_publication_faults_and_barrier,
        };
        use std::fs;

        fn fixture() -> (
            tempfile::TempDir,
            Arc<JournalRoot>,
            String,
            Map<String, Value>,
        ) {
            let root = tempfile::tempdir().unwrap();
            fs::create_dir(root.path().join("config")).unwrap();
            fs::write(
                root.path().join("config/journal.json"),
                serde_json::to_vec(&json!({
                    "backup": {"enabled":true,"daily_key":"backup-secret","unknown":{"keep":7}},
                    "providers":{"credential":"provider-secret"}, "other":42
                }))
                .unwrap(),
            )
            .unwrap();
            let read = read_journal_config(root.path()).unwrap();
            let expected = merge_backup_config(read.config.as_ref().unwrap());
            let admitted = Arc::new(JournalRoot::open(root.path()).unwrap());
            (root, admitted, read.sha256.unwrap(), expected)
        }

        #[test]
        fn retained_clear_matches_partial_legacy_defaults_and_preserves_other_settings() {
            let (root, admitted, sha, expected) = fixture();
            let BoundConfigOutcome::Published(receipt) =
                clear_backup_config_at_root(&admitted, &sha, &expected).unwrap()
            else {
                panic!("must publish")
            };
            assert!(receipt.confirmation_refusal.is_none());
            assert!(receipt.durability_uncertainty.is_none());
            assert!(receipt.observation_uncertainty.is_none());
            let config = read_journal_config(root.path()).unwrap().config.unwrap();
            assert_eq!(config["backup"], Value::Object(backup_defaults()));
            assert_eq!(config["other"], 42);
            assert_eq!(config["providers"]["credential"], "provider-secret");
            let diagnostic = format!("{receipt:?}");
            assert!(!diagnostic.contains("provider-secret"));
            assert!(!diagnostic.contains("backup-secret"));
        }

        #[test]
        fn retained_clear_refuses_changed_expected_backup_and_original_full_config_bytes() {
            for field in ["daily_key", "unknown"] {
                let (root, admitted, sha, mut expected) = fixture();
                let before = fs::read(root.path().join("config/journal.json")).unwrap();
                expected.insert(field.into(), json!("changed"));
                assert!(matches!(
                    clear_backup_config_at_root(&admitted, &sha, &expected).unwrap(),
                    BoundConfigOutcome::Unchanged
                ));
                assert_eq!(
                    fs::read(root.path().join("config/journal.json")).unwrap(),
                    before
                );
            }
            let (root, admitted, sha, expected) = fixture();
            let path = root.path().join("config/journal.json");
            let mut config = read_journal_config(root.path()).unwrap().config.unwrap();
            config.insert("other".into(), json!(43));
            let newer = serde_json::to_vec(&config).unwrap();
            fs::write(&path, &newer).unwrap();
            assert!(matches!(
                clear_backup_config_at_root(&admitted, &sha, &expected),
                Err(BoundConfigRefusal::Conflict)
            ));
            assert_eq!(fs::read(path).unwrap(), newer);
        }

        #[test]
        fn retained_clear_preserves_combined_native_and_confirmation_uncertainty_without_secrets() {
            let (root, admitted, sha, expected) = fixture();
            let path = root.path().join("config/journal.json");
            let newer = br#"{"providers":{"credential":"newer-provider-secret"},"newer":true}"#;
            let directory = root.path().join("config");
            let (result, faults, fired) = run_with_bound_publication_faults_and_barrier(
                [(BoundPublicationPrimitive::ParentSync, 1, 5)],
                BoundPublicationPrimitive::Rename,
                1,
                move || {
                    let stage = fs::read_dir(directory)
                        .unwrap()
                        .map(|entry| entry.unwrap())
                        .find(|entry| is_publication_candidate_name(&entry.file_name()))
                        .unwrap()
                        .path();
                    fs::write(stage, newer).unwrap();
                },
                || clear_backup_config_at_root(&admitted, &sha, &expected),
            );
            assert_eq!(faults, 1);
            assert!(fired);
            let BoundConfigOutcome::Published(receipt) = result.unwrap() else {
                panic!("rename landed")
            };
            assert!(receipt.published_file.as_ref().unwrap().size > 0);
            assert!(receipt.durability_uncertainty.is_some());
            assert!(receipt.observation_uncertainty.is_some());
            assert!(receipt.confirmation_refusal.is_some());
            assert_eq!(fs::read(path).unwrap(), newer);
            let diagnostic = format!("{receipt:?}");
            for secret in ["backup-secret", "provider-secret", "newer-provider-secret"] {
                assert!(!diagnostic.contains(secret));
            }
        }

        #[test]
        fn retained_clear_load_refusal_diagnostics_omit_configuration_credentials() {
            let (root, admitted, sha, expected) = fixture();
            let path = root.path().join("config/journal.json");
            let malformed = br#"{"providers":{"credential":"provider-secret"},"backup":{"daily_key":"backup-secret"},"torn":"#;
            fs::write(&path, malformed).unwrap();
            let error = clear_backup_config_at_root(&admitted, &sha, &expected).unwrap_err();
            assert!(matches!(error, BoundConfigRefusal::Load(_)));
            let diagnostic = format!("{error} {error:?}");
            for secret in ["backup-secret", "provider-secret"] {
                assert!(!diagnostic.contains(secret));
            }
            assert_eq!(fs::read(path).unwrap(), malformed);
            assert!(!root.path().join("config/journal.json.lock").exists());
        }
    }

    #[test]
    fn retained_clear_preserves_other_settings_and_refuses_stale_source() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("config")).unwrap();
        let path = root.path().join("config").join("journal.json");
        let original = br#"{"backup":{"enabled":true},"other":{"keep":7}}"#;
        std::fs::write(&path, original).unwrap();
        let admitted = Arc::new(JournalRoot::open(root.path()).unwrap());
        let read = read_journal_config(root.path()).unwrap();
        let sha = read.sha256.unwrap();
        let expected = get_backup_config(root.path()).unwrap();
        assert!(matches!(
            clear_backup_config_at_root(&admitted, &sha, &expected).unwrap(),
            BoundConfigOutcome::Published(_)
        ));
        let saved: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(saved["other"]["keep"], 7);
        assert_eq!(saved["backup"]["enabled"], false);
        let current = std::fs::read(&path).unwrap();
        assert!(matches!(
            clear_backup_config_at_root(&admitted, &sha, &expected),
            Err(BoundConfigRefusal::Conflict)
        ));
        assert_eq!(std::fs::read(path).unwrap(), current);
    }

    #[cfg(windows)]
    #[test]
    fn retained_clear_windows_refuses_changed_fingerprint_before_lock_or_mutation() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("config")).unwrap();
        let path = root.path().join("config/journal.json");
        let original = br#"{"backup":{"enabled":true}}"#;
        std::fs::write(&path, original).unwrap();
        let admitted = Arc::new(JournalRoot::open(root.path()).unwrap());
        let expected = get_backup_config(root.path()).unwrap();
        assert!(matches!(
            clear_backup_config_at_root(&admitted, "unneeded", &expected),
            Err(BoundConfigRefusal::Conflict)
        ));
        assert_eq!(std::fs::read(path).unwrap(), original);
        assert!(!root.path().join("config/journal.json.lock").exists());
    }

    #[test]
    fn a_pause_keeps_its_first_time_and_resume_leaves_every_other_setting() {
        let journal = tempfile::tempdir().unwrap();
        set_enabled(journal.path(), true).unwrap();
        let before = get_backup_config(journal.path()).unwrap();
        set_backup_paused(journal.path(), Some(100)).unwrap();
        set_backup_paused(journal.path(), Some(200)).unwrap();
        let paused = get_backup_config(journal.path()).unwrap();
        assert_eq!(backup_paused_at(&paused), Some(100));
        assert_eq!(status_view(journal.path()).unwrap()["paused_at"], 100);
        set_backup_paused(journal.path(), None).unwrap();
        assert_eq!(get_backup_config(journal.path()).unwrap(), before);
    }
}
