// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::{Path, PathBuf};

use serde_json::Value;
use solstone_core_body_source::{BodyRawRetention, BodySourceFamily};

use crate::bounded_file::read_bounded_regular;
use crate::bundle::BodyIngestError;

const APPLE_PATH: &str = "imports/_approvals/health_import_preflight.json";
const APPLE_SCHEMA: &str = "solstone.health_import_preflight.v1";
const APPLE_CHECKLIST: &str = "solstone.health_import_preflight.checklist.v3";
pub const OURA_PATH: &str = "imports/_approvals/oura_sync_preflight.json";
const OURA_SCHEMA: &str = "solstone.oura_sync_preflight.v1";
pub const OURA_CHECKLIST: &str = "solstone.oura_sync_preflight.checklist.v2";
const MAX_APPROVAL_BYTES: usize = 64 * 1024;

pub(crate) fn pin_journal_target(journal: &Path) -> Result<PathBuf, BodyIngestError> {
    require_absolute_target(journal)?;
    journal
        .canonicalize()
        .map_err(|_| BodyIngestError::gate("target_journal_unreadable"))
}

pub(crate) fn apple_approval(
    journal: &Path,
    confirmed: bool,
) -> Result<BodyRawRetention, BodyIngestError> {
    require_absolute_target(journal)?;
    if !confirmed {
        return Err(BodyIngestError::gate("per_run_confirmation_missing"));
    }
    let bytes = read_approval(journal, APPLE_PATH)?;
    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|_| BodyIngestError::gate("malformed_approval_artifact"))?;
    let object = value
        .as_object()
        .ok_or_else(|| BodyIngestError::gate("malformed_approval_artifact"))?;
    if object.get("schema").and_then(Value::as_str) != Some(APPLE_SCHEMA) {
        return Err(BodyIngestError::gate("unsupported_approval_schema"));
    }
    if object.get("checklist_version").and_then(Value::as_str) != Some(APPLE_CHECKLIST) {
        return Err(BodyIngestError::gate("checklist_version_mismatch"));
    }
    let approved = object
        .get("approved_importers")
        .and_then(Value::as_array)
        .is_some_and(|values| {
            values
                .iter()
                .any(|value| value.as_str() == Some("apple_health"))
        });
    if !approved {
        return Err(BodyIngestError::gate("importer_not_approved"));
    }
    if object.get("requires_per_run_confirmation") != Some(&Value::Bool(true))
        || object.get("no_real_health_data_in_artifact") != Some(&Value::Bool(true))
    {
        return Err(BodyIngestError::gate("checklist_incomplete"));
    }
    let configured_root = object
        .get("journal_root")
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .ok_or_else(|| BodyIngestError::gate("journal_root_binding_missing"))?;
    require_absolute_binding(&configured_root)?;
    let actual = journal
        .canonicalize()
        .map_err(|_| BodyIngestError::gate("target_journal_unreadable"))?;
    let configured = configured_root
        .canonicalize()
        .map_err(|_| BodyIngestError::gate("journal_root_binding_invalid"))?;
    if configured != actual {
        return Err(BodyIngestError::gate("journal_root_binding_mismatch"));
    }
    validate_destinations(object.get("replication_destinations"))?;
    let raw = object
        .get("raw_retention")
        .and_then(Value::as_object)
        .ok_or_else(|| BodyIngestError::gate("raw_retention_missing"))?;
    let decision = raw
        .get("decision")
        .and_then(Value::as_str)
        .ok_or_else(|| BodyIngestError::gate("raw_retention_missing"))?;
    let retention = BodyRawRetention::from_bytes(decision.as_bytes())
        .map_err(|_| BodyIngestError::gate("raw_retention_invalid"))?;
    if retention == BodyRawRetention::RetainComplete
        && raw.get("unparsed_sensitive_modalities_acknowledged") != Some(&Value::Bool(true))
    {
        return Err(BodyIngestError::gate(
            "unparsed_sensitive_modalities_not_acknowledged",
        ));
    }
    Ok(retention)
}

pub fn oura_approval(
    journal: &Path,
    confirmed: bool,
    scheduled: bool,
) -> Result<BodyRawRetention, BodyIngestError> {
    require_absolute_target(journal)?;
    if !confirmed && !scheduled {
        return Err(BodyIngestError::gate("per_run_confirmation_missing"));
    }
    let bytes = read_approval(journal, OURA_PATH)?;
    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|_| BodyIngestError::gate("malformed_approval_artifact"))?;
    let object = value
        .as_object()
        .ok_or_else(|| BodyIngestError::gate("malformed_approval_artifact"))?;
    if object.get("schema").and_then(Value::as_str) != Some(OURA_SCHEMA) {
        return Err(BodyIngestError::gate("unsupported_approval_schema"));
    }
    if object.get("checklist_version").and_then(Value::as_str) != Some(OURA_CHECKLIST) {
        return Err(BodyIngestError::gate("checklist_version_mismatch"));
    }
    if object.get("requires_per_run_confirmation") != Some(&Value::Bool(true)) {
        return Err(BodyIngestError::gate("checklist_incomplete"));
    }
    let configured_root = object
        .get("journal_root")
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .ok_or_else(|| BodyIngestError::gate("journal_root_binding_missing"))?;
    require_absolute_binding(&configured_root)?;
    let actual = journal
        .canonicalize()
        .map_err(|_| BodyIngestError::gate("target_journal_unreadable"))?;
    let configured = configured_root
        .canonicalize()
        .map_err(|_| BodyIngestError::gate("journal_root_binding_invalid"))?;
    if configured != actual {
        return Err(BodyIngestError::gate("journal_root_binding_mismatch"));
    }
    validate_destinations(object.get("replication_destinations"))?;
    let decision = object
        .get("raw_retention")
        .and_then(Value::as_object)
        .and_then(|raw| raw.get("decision"))
        .and_then(Value::as_str)
        .ok_or_else(|| BodyIngestError::gate("raw_retention_missing"))?;
    let retention = BodyRawRetention::from_bytes(decision.as_bytes())
        .map_err(|_| BodyIngestError::gate("raw_retention_invalid"))?;
    retention
        .check_compatible(&BodySourceFamily::OuraApi)
        .map_err(|_| BodyIngestError::gate("raw_retention_incompatible"))?;
    if scheduled {
        let consent = object
            .get("scheduled_sync")
            .and_then(Value::as_object)
            .ok_or_else(|| BodyIngestError::gate("scheduled_sync_consent_missing"))?;
        if consent.get("approved") != Some(&Value::Bool(true)) {
            return Err(BodyIngestError::gate("scheduled_sync_not_approved"));
        }
        let cadence = consent.get("cadence").and_then(Value::as_str).unwrap_or("");
        if cadence.trim().is_empty() {
            return Err(BodyIngestError::gate("scheduled_sync_cadence_invalid"));
        }
        let valid_until = consent
            .get("valid_until")
            .and_then(Value::as_str)
            .ok_or_else(|| BodyIngestError::gate("scheduled_sync_valid_until_missing"))?;
        let expiry = chrono::DateTime::parse_from_rfc3339(valid_until)
            .map_err(|_| BodyIngestError::gate("scheduled_sync_valid_until_invalid"))?;
        if chrono::Utc::now() >= expiry.with_timezone(&chrono::Utc) {
            return Err(BodyIngestError::gate("scheduled_sync_consent_expired"));
        }
    }
    Ok(retention)
}

fn read_approval(journal: &Path, relative: &str) -> Result<Vec<u8>, BodyIngestError> {
    read_bounded_regular(journal, relative, MAX_APPROVAL_BYTES).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            BodyIngestError::gate("missing_approval_artifact")
        } else {
            BodyIngestError::gate("malformed_approval_artifact")
        }
    })
}

fn require_absolute_target(journal: &Path) -> Result<(), BodyIngestError> {
    if !journal.is_absolute() {
        return Err(BodyIngestError::gate("target_journal_not_absolute"));
    }
    Ok(())
}

fn require_absolute_binding(configured_root: &Path) -> Result<(), BodyIngestError> {
    if !configured_root.is_absolute() {
        return Err(BodyIngestError::gate("journal_root_binding_not_absolute"));
    }
    Ok(())
}

fn validate_destinations(value: Option<&Value>) -> Result<(), BodyIngestError> {
    const NAMES: [&str; 5] = [
        "time_machine",
        "icloud",
        "solbase",
        "hosted_backup",
        "other",
    ];
    let destinations = value
        .and_then(Value::as_object)
        .ok_or_else(|| BodyIngestError::gate("replication_destinations_missing"))?;
    if destinations.len() != NAMES.len()
        || !NAMES.iter().all(|name| destinations.contains_key(*name))
    {
        return Err(BodyIngestError::gate("replication_destinations_invalid"));
    }
    for name in NAMES {
        let decision = destinations[name]
            .as_object()
            .and_then(|entry| entry.get("decision"))
            .and_then(Value::as_str);
        if !matches!(decision, Some("approved" | "excluded")) {
            return Err(BodyIngestError::gate("replication_destinations_invalid"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::symlink;
    use std::sync::atomic::{AtomicU64, Ordering};

    use nix::sys::stat::Mode;
    use nix::unistd::mkfifo;
    use serde_json::json;

    use super::*;

    static SEQUENCE: AtomicU64 = AtomicU64::new(0);

    fn temporary_journal() -> PathBuf {
        let journal = std::env::temp_dir().join(format!(
            "solstone-body-approval-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(journal.join("imports/_approvals")).unwrap();
        journal
    }

    #[test]
    fn both_gates_reject_a_relative_target_before_resolution() {
        for error in [
            apple_approval(Path::new("."), true).unwrap_err(),
            oura_approval(Path::new("."), true, false).unwrap_err(),
        ] {
            assert_eq!(error.stage(), "target_journal_not_absolute");
        }
    }

    #[test]
    fn both_gates_reject_a_relative_artifact_binding_before_resolution() {
        let journal = temporary_journal();
        fs::write(
            journal.join(APPLE_PATH),
            serde_json::to_vec(&json!({
                "schema": APPLE_SCHEMA,
                "checklist_version": APPLE_CHECKLIST,
                "approved_importers": ["apple_health"],
                "requires_per_run_confirmation": true,
                "no_real_health_data_in_artifact": true,
                "journal_root": "."
            }))
            .unwrap(),
        )
        .unwrap();
        assert_eq!(
            apple_approval(&journal, true).unwrap_err().stage(),
            "journal_root_binding_not_absolute"
        );

        fs::write(
            journal.join(OURA_PATH),
            serde_json::to_vec(&json!({
                "schema": OURA_SCHEMA,
                "checklist_version": OURA_CHECKLIST,
                "requires_per_run_confirmation": true,
                "journal_root": "."
            }))
            .unwrap(),
        )
        .unwrap();
        assert_eq!(
            oura_approval(&journal, true, false).unwrap_err().stage(),
            "journal_root_binding_not_absolute"
        );
        fs::remove_dir_all(journal).unwrap();
    }

    #[test]
    fn approval_read_refuses_symlinks_fifos_and_oversized_documents() {
        let journal = temporary_journal();
        let approval = journal.join(APPLE_PATH);
        let outside = journal.with_extension("outside-approval.json");
        fs::write(&outside, b"{}").unwrap();
        symlink(&outside, &approval).unwrap();
        assert_eq!(
            apple_approval(&journal, true).unwrap_err().stage(),
            "malformed_approval_artifact"
        );

        fs::remove_file(&approval).unwrap();
        mkfifo(&approval, Mode::S_IRUSR | Mode::S_IWUSR).unwrap();
        assert_eq!(
            apple_approval(&journal, true).unwrap_err().stage(),
            "malformed_approval_artifact"
        );

        fs::remove_file(&approval).unwrap();
        fs::write(&approval, vec![b'x'; MAX_APPROVAL_BYTES + 1]).unwrap();
        assert_eq!(
            apple_approval(&journal, true).unwrap_err().stage(),
            "malformed_approval_artifact"
        );

        fs::remove_file(outside).unwrap();
        fs::remove_dir_all(journal).unwrap();
    }
}
