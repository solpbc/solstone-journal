// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! App-package runtime authority with the ordinary pinned model generation.

use super::{DispatchError, failure, manifest, pins, resolved_fingerprint, status};
use serde_json::{Value, json};
use std::path::Path;

pub fn target_for_key(model_id: &str, key: &str) -> Result<Value, DispatchError> {
    let identity = crate::embedded::identity_for_key(key).ok_or_else(|| {
        failure(
            "package",
            "embedded_runtime_unavailable",
            "the journal app does not provide this local runtime",
            65,
        )
    })?;
    target_for_identity(model_id, &identity)
}

pub(super) fn target_for_identity(
    model_id: &str,
    identity: &crate::embedded::RuntimeIdentity,
) -> Result<Value, DispatchError> {
    let model = pins::model_identity(model_id)
        .ok_or_else(|| failure("model", "unsupported_model", model_id, 65))?;
    Ok(
        json!({"provider":"local", "runtime":"llama.cpp", "backend":identity.backend(),
        "backend_reason":"runtime included in the journal app", "runtime_pin":identity.pin(), "model_pin":model}),
    )
}

pub fn download_bytes(model_id: &str) -> Option<u64> {
    pins::model_generation(model_id)
        .map(|generation| generation.files.iter().map(|file| file.size_bytes).sum())
}

/// Execute one admitted app-owned installer with the ordinary durable status
/// and OS lease. The caller retains its thread/root until this call returns.
pub fn install_with_lease_admitted(
    journal: &Path,
    model_id: &str,
    lease: super::lease::InstallLease,
    owner: Value,
    cancelled: &dyn Fn() -> bool,
    admitted: impl FnOnce(&status::InstallStatus),
) -> Result<Value, DispatchError> {
    let identity = crate::embedded::identity_for_key(&pins::platform_key()).ok_or_else(|| {
        failure(
            "package",
            "embedded_runtime_unavailable",
            "the journal app does not provide this local runtime",
            65,
        )
    })?;
    let object = serde_json::Map::from_iter([
        ("journal".into(), json!(journal)),
        ("model_id".into(), json!(model_id)),
        ("owner".into(), owner),
    ]);
    super::run_with_lease(
        &object,
        super::RunKind::Mobile {
            identity: &identity,
            cancelled,
        },
        journal.to_owned(),
        lease,
        None,
        None,
        admitted,
    )
}

pub(super) fn install_model(
    journal: &Path,
    model_id: &str,
    state: &mut status::InstallStatus,
    cancelled: &dyn Fn() -> bool,
) -> Result<Value, DispatchError> {
    checkpoint(cancelled)?;
    let bytes = download_bytes(model_id)
        .ok_or_else(|| failure("model", "unsupported_model", model_id, 65))?;
    super::install_model_with_cancel(journal, model_id, state, 0, bytes, Some(cancelled))?;
    checkpoint(cancelled)?;
    Ok(json!({"runtime_source":"app-package", "model_id":model_id}))
}

pub(super) fn checkpoint(cancelled: &dyn Fn() -> bool) -> Result<(), DispatchError> {
    if cancelled() {
        Err(failure(
            "cancelled",
            "install_cancelled",
            "install_cancelled",
            74,
        ))
    } else {
        Ok(())
    }
}

pub(crate) fn inspect(
    journal: &Path,
    model_id: &str,
    key: &str,
    check: fn(&Path, &Value, &[&str]) -> Value,
) -> Value {
    inspect_with_identity(
        journal,
        model_id,
        crate::embedded::identity_for_key(key),
        check,
    )
}

fn inspect_with_identity(
    journal: &Path,
    model_id: &str,
    identity: Option<crate::embedded::RuntimeIdentity>,
    check: fn(&Path, &Value, &[&str]) -> Value,
) -> Value {
    let Some(model_identity) = pins::model_identity(model_id) else {
        return json!({"provider":"local", "ready":false, "status":"host-ineligible",
            "reason_code":"unsupported_model", "target":{"model_id":model_id}});
    };
    let runtime_present = identity.is_some();
    let backend = identity.as_ref().map(|identity| identity.backend());
    let target = identity
        .as_ref()
        .and_then(|identity| target_for_identity(model_id, identity).ok())
        .and_then(|target| resolved_fingerprint(target).ok())
        .unwrap_or(Value::Null);
    let model_root = pins::model_directory(journal, model_id);
    let names = [
        model_identity["filename"].as_str().unwrap_or(""),
        model_identity["mmproj_filename"].as_str().unwrap_or(""),
    ];
    let model_proof = check(
        &manifest::artifact_manifest_path(&model_root),
        &model_identity,
        &names,
    );
    let (state, reason) = if !runtime_present {
        ("host-ineligible", "embedded_runtime_unavailable")
    } else {
        (
            model_proof["status"]
                .as_str()
                .unwrap_or("proof-unavailable"),
            model_proof["reason_code"]
                .as_str()
                .unwrap_or("proof_unavailable"),
        )
    };
    let install = status::read_observed_status(journal, "local")
        .map(|value| serde_json::to_value(value).unwrap())
        .unwrap_or(Value::Null);
    json!({"provider":"local", "ready":runtime_present && model_proof["status"]=="ready",
        "status":state,"reason_code":reason,
        "target":{"model_id":model_id,"target_fingerprint_json":target["target_fingerprint_json"],"target_fingerprint_sha256":target["target_fingerprint_sha256"]},
        "install":install,
        "host":{"platform_supported":runtime_present,"backend":backend,"backend_reason":"runtime included in the journal app"},
        "artifacts":{"model_id":model_id,"binary_installed":runtime_present,"runtime_source":"app-package","model_installed":model_proof["status"]=="ready","model_path":model_root.join(names[0]),"projector_path":model_root.join(names[1])},
        "proof":{"binary":{"status":if runtime_present {"ready"} else {"missing-or-mismatched"},"source":"app-package"},"model":model_proof}})
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(key: &str, code: char) -> crate::embedded::RuntimeIdentity {
        crate::embedded::RuntimeIdentity {
            artifact_key: key.into(),
            abi: 1,
            source_revision: "1".repeat(40),
            runtime_identity: format!("sha256:{}", code.to_string().repeat(64)),
        }
    }

    #[test]
    fn packaged_runtime_fingerprint_binds_code_and_both_model_members() {
        for (key, backend) in [("aarch64-android", "cpu"), ("aarch64-ios", "metal")] {
            let old = target_for_identity("local/qwen3.5-4b", &identity(key, '2')).unwrap();
            let new = target_for_identity("local/qwen3.5-4b", &identity(key, '3')).unwrap();
            assert_eq!(old["backend"], backend);
            assert_eq!(old["runtime_pin"]["source"], "app-package");
            assert!(old["model_pin"]["sha256"].is_string());
            assert!(old["model_pin"]["mmproj_sha256"].is_string());
            assert_ne!(
                resolved_fingerprint(old).unwrap(),
                resolved_fingerprint(new).unwrap()
            );
        }
        assert!(target_for_identity("local/unknown", &identity("aarch64-android", '2')).is_err());
        assert_eq!(download_bytes("local/unknown"), None);
    }

    #[test]
    #[cfg(feature = "full-tests")]
    fn packaged_code_does_not_substitute_for_verified_model_and_projector() {
        let root = tempfile::tempdir().unwrap();
        for key in ["aarch64-android", "aarch64-ios"] {
            let code = identity(key, '2');
            let missing = inspect_with_identity(
                root.path(),
                "local/qwen3.5-4b",
                Some(code.clone()),
                manifest::prove_manifest_required,
            );
            assert_eq!(missing["ready"], false);
            assert_eq!(missing["artifacts"]["binary_installed"], true);
            assert_eq!(missing["reason_code"], "manifest_missing");
            let model_root = pins::model_directory(root.path(), "local/qwen3.5-4b");
            std::fs::create_dir_all(&model_root).unwrap();
            let path = manifest::artifact_manifest_path(&model_root);
            std::fs::write(&path, b"not a manifest").unwrap();
            let corrupt = inspect_with_identity(
                root.path(),
                "local/qwen3.5-4b",
                Some(code),
                manifest::prove_manifest_required,
            );
            assert_eq!(corrupt["ready"], false);
            assert_eq!(corrupt["reason_code"], "manifest_malformed");
            std::fs::remove_file(path).unwrap();
        }
        let absent = inspect_with_identity(
            root.path(),
            "local/qwen3.5-4b",
            None,
            manifest::prove_manifest_required,
        );
        assert_eq!(absent["ready"], false);
        assert_eq!(absent["reason_code"], "embedded_runtime_unavailable");
        let unknown = inspect_with_identity(
            root.path(),
            "local/unknown",
            Some(identity("aarch64-android", '2')),
            manifest::prove_manifest_required,
        );
        assert_eq!(unknown["ready"], false);
        assert_eq!(unknown["reason_code"], "unsupported_model");
    }

    #[test]
    #[cfg(feature = "full-tests")]
    fn admitted_cancellation_keeps_its_attempt_and_never_marks_installed() {
        let root = tempfile::tempdir().unwrap();
        let object = serde_json::Map::from_iter([
            ("model_id".into(), json!("local/qwen3.5-4b")),
            ("owner".into(), json!({"kind":"app-owned-installer"})),
        ]);
        let lease = super::super::lease::acquire(root.path(), "local")
            .unwrap()
            .unwrap();
        let mut admitted = None;
        let error = super::super::run_with_lease(
            &object,
            super::super::RunKind::Mobile {
                identity: &identity("aarch64-android", '2'),
                cancelled: &|| true,
            },
            root.path().to_owned(),
            lease,
            None,
            None,
            |state| admitted = state.attempt_id.clone(),
        )
        .unwrap_err();
        assert_eq!(
            error.envelope.error.unwrap().reason_code,
            "install_cancelled"
        );
        let status = status::read_status(root.path(), "local").unwrap();
        assert_eq!(status.attempt_id, admitted);
        assert!(status.attempt_id.is_some());
        assert_eq!(status.install_state, "failed");
        assert_eq!(status.error_code.as_deref(), Some("install_cancelled"));
        assert!(!pins::model_directory(root.path(), "local/qwen3.5-4b").exists());
        assert!(!super::super::lease::is_held(root.path(), "local").unwrap());
    }
}
