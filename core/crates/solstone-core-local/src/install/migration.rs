// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Migration of historical provider install hints into provider-owned status.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

use super::{fingerprint, lease, manifest, pins, readiness, status};

const LOCAL_MODEL: &str = "local/qwen3.5-4b";
const LEGACY_OPERATIONAL_KEYS: &[&str] = &[
    "install_state",
    "last_transition_at",
    "last_progress_at",
    "progress_bytes_received",
    "progress_bytes_total",
    "install_error",
    "binary_artifact",
    "binary_sha256",
    "binary_path",
    "model_id",
    "model_path",
    "model_sha256",
    "mmproj_path",
    "mmproj_sha256",
    "mlx_model_id",
    "mlx_revision",
    "mlx_snapshot_dir",
    "mlx_variant_dir",
    "binary_artifact_cpu",
    "binary_sha256_cpu",
    "binary_path_cpu",
    "binary_artifact_vulkan",
    "binary_sha256_vulkan",
    "binary_path_vulkan",
    "model_repo",
    "model_filename",
    "model_revision",
];

#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct ProviderInstallMigration {
    pub actions: Vec<String>,
    pub removed: usize,
    pub moved: usize,
}

fn legacy_record<'a>(
    config: &'a Map<String, Value>,
    provider: &str,
) -> Option<&'a Map<String, Value>> {
    config
        .get("providers")?
        .as_object()?
        .get("bundled")?
        .as_object()?
        .get(provider)?
        .as_object()
}

fn legacy_has_state(record: &Map<String, Value>) -> bool {
    LEGACY_OPERATIONAL_KEYS
        .iter()
        .any(|key| record.contains_key(*key))
        || record.contains_key("vulkan_device_index")
}

fn target(provider: &str, journal: &Path) -> Result<(Value, String, String), String> {
    let target = match provider {
        "local" => super::local_target(
            journal,
            LOCAL_MODEL,
            if cfg!(target_os = "macos") {
                super::LocalBackend::Metal
            } else {
                super::LocalBackend::Existing
            },
        ),
        "parakeet" => super::parakeet_target(journal),
        _ => unreachable!("closed provider migration census"),
    }
    .map_err(|error| {
        error
            .envelope
            .error
            .map(|error| error.message)
            .unwrap_or_else(|| "provider target resolution failed".to_owned())
    })?;
    let canonical = fingerprint::canonical(target.clone())?;
    let digest = fingerprint::sha256(&canonical);
    Ok((target, canonical, digest))
}

fn publish_ready(
    journal: &Path,
    provider: &str,
    canonical: String,
    digest: String,
) -> Result<(), String> {
    let mut current = status::read_status(journal, provider).map_err(|error| error.to_string())?;
    if status::is_in_flight(&current.install_state) {
        return Err("cannot migrate while install is in flight".to_owned());
    }
    current.target_fingerprint_json = Some(canonical);
    current.target_fingerprint_sha256 = Some(digest);
    current.owner = Some(json!({"entry":"legacy_provider_install_state_migration"}));
    let installed =
        status::transition(current, "installed", None, None).map_err(|error| error.to_string())?;
    status::write_status(journal, installed)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

fn field_matches(record: &Map<String, Value>, key: &str, expected: impl AsRef<str>) -> bool {
    record.get(key).and_then(Value::as_str) == Some(expected.as_ref())
}

fn executable(path: &Path) -> bool {
    let Ok(metadata) = path.metadata() else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn promote_legacy_local(
    journal: &Path,
    legacy: &Map<String, Value>,
    target: &Value,
    target_sha: &str,
) -> Result<(), String> {
    if legacy.get("install_state").and_then(Value::as_str) != Some("installed") {
        return Err("legacy_status_not_installed".to_owned());
    }
    if legacy.contains_key("mlx_model_id") || legacy.contains_key("mlx_snapshot_dir") {
        return Err("manifest_missing".to_owned());
    }
    if target.get("backend").and_then(Value::as_str) != Some("vulkan") {
        return Err("manifest_missing".to_owned());
    }
    let key = pins::platform_key();
    let (release, archive_name, archive_sha, binary_name) =
        pins::vulkan_pin(&key).ok_or("unsupported_platform")?;
    let root = pins::cache_root(journal)
        .join("bin")
        .join(&key)
        .join(release);
    let binary = root.join(binary_name);
    if !field_matches(legacy, "binary_artifact", &key)
        || !field_matches(legacy, "binary_sha256", archive_sha)
        || !field_matches(legacy, "binary_path", binary.display().to_string())
        || !executable(&binary)
    {
        return Err("legacy_binary_mismatch".to_owned());
    }

    let model_identity = pins::model_identity(LOCAL_MODEL).ok_or("unsupported_model")?;
    let model_root = pins::cache_root(journal)
        .join("models")
        .join(LOCAL_MODEL.replace('/', "__"));
    let model = model_root.join(
        model_identity["filename"]
            .as_str()
            .ok_or("model_pin_invalid")?,
    );
    if !field_matches(legacy, "model_id", LOCAL_MODEL)
        || !field_matches(legacy, "model_path", model.display().to_string())
    {
        return Err("legacy_model_mismatch".to_owned());
    }
    manifest::verify_file_sha256(
        &model,
        model_identity["sha256"]
            .as_str()
            .ok_or("model_pin_invalid")?,
    )?;
    if let (Some(filename), Some(expected_sha)) = (
        model_identity["mmproj_filename"].as_str(),
        model_identity["mmproj_sha256"].as_str(),
    ) {
        let projector = model_root.join(filename);
        if !field_matches(legacy, "mmproj_path", projector.display().to_string()) {
            return Err("legacy_projector_path_mismatch".to_owned());
        }
        manifest::verify_file_sha256(&projector, expected_sha)?;
    }

    let runtime_manifest = manifest::build_manifest(
        "local",
        "llama-server-vulkan",
        target_sha,
        json!({"pin_identity":pins::vulkan_identity(&key).ok_or("unsupported_platform")?}),
        manifest::runtime_inventory(&root, &[archive_name.to_owned()])?,
        None,
        None,
    )?;
    manifest::write_manifest(&manifest::artifact_manifest_path(&root), &runtime_manifest)?;
    let model_manifest = manifest::build_manifest(
        "local",
        "local-model",
        target_sha,
        json!({"pin_identity":model_identity}),
        manifest::inventory_for_tree(&model_root, "model")?,
        None,
        None,
    )?;
    manifest::write_manifest(
        &manifest::artifact_manifest_path(&model_root),
        &model_manifest,
    )?;
    Ok(())
}

fn parakeet_paths(journal: &Path, key: &str, backend: &str) -> Result<(PathBuf, PathBuf), String> {
    let (release, _, _, binary_name) =
        pins::parakeet_backend_pin(key, backend).ok_or("unsupported_platform")?;
    let root = pins::parakeet_cache_root(journal)
        .join("bin")
        .join(key)
        .join(backend)
        .join(release);
    let binary = root.join(binary_name);
    Ok((root, binary))
}

fn promote_legacy_parakeet(
    journal: &Path,
    legacy: &Map<String, Value>,
    target: &Value,
    target_sha: &str,
) -> Result<(), String> {
    if legacy.get("install_state").and_then(Value::as_str) != Some("installed") {
        return Err("legacy_status_not_installed".to_owned());
    }
    let key = target["artifact_key"]
        .as_str()
        .ok_or("unsupported_platform")?;
    let mut binary_roots = Vec::new();
    for backend in ["cpu", "vulkan"] {
        let (root, binary) = parakeet_paths(journal, key, backend)?;
        let (_, archive_name, archive_sha, _) =
            pins::parakeet_backend_pin(key, backend).ok_or("unsupported_platform")?;
        if !field_matches(legacy, &format!("binary_artifact_{backend}"), key)
            || !field_matches(legacy, &format!("binary_sha256_{backend}"), archive_sha)
            || !field_matches(
                legacy,
                &format!("binary_path_{backend}"),
                binary.display().to_string(),
            )
            || !executable(&binary)
        {
            return Err(format!("legacy_binary_{backend}_mismatch"));
        }
        binary_roots.push((backend, archive_name, root));
    }

    let model_identity = pins::parakeet_model_identity();
    let model_root = pins::parakeet_cache_root(journal)
        .join("models")
        .join(
            model_identity["repo"]
                .as_str()
                .ok_or("model_pin_invalid")?
                .replace('/', "__"),
        )
        .join(
            model_identity["revision"]
                .as_str()
                .ok_or("model_pin_invalid")?,
        );
    let model = model_root.join(
        model_identity["filename"]
            .as_str()
            .ok_or("model_pin_invalid")?,
    );
    for (field, pin_field) in [
        ("model_repo", "repo"),
        ("model_filename", "filename"),
        ("model_revision", "revision"),
    ] {
        if !field_matches(
            legacy,
            field,
            model_identity[pin_field]
                .as_str()
                .ok_or("model_pin_invalid")?,
        ) {
            return Err(format!("legacy_{field}_mismatch"));
        }
    }
    if !field_matches(legacy, "model_path", model.display().to_string()) {
        return Err("legacy_model_path_mismatch".to_owned());
    }
    manifest::verify_file_sha256(
        &model,
        model_identity["sha256"]
            .as_str()
            .ok_or("model_pin_invalid")?,
    )?;

    for (backend, archive_name, root) in binary_roots {
        let built = manifest::build_manifest(
            "parakeet",
            "parakeet-server",
            target_sha,
            json!({"pin_identity":pins::parakeet_backend_identity(key, backend).ok_or("unsupported_platform")?}),
            manifest::runtime_inventory(&root, &[archive_name.to_owned()])?,
            None,
            None,
        )?;
        manifest::write_manifest(&manifest::artifact_manifest_path(&root), &built)?;
    }
    let model_manifest = manifest::build_manifest(
        "parakeet",
        "parakeet-model",
        target_sha,
        json!({"pin_identity":model_identity}),
        manifest::inventory_for_tree(&model_root, "model")?,
        None,
        None,
    )?;
    manifest::write_manifest(
        &manifest::artifact_manifest_path(&model_root),
        &model_manifest,
    )?;
    Ok(())
}

fn inspect(journal: &Path, provider: &str) -> Value {
    let mut input = Map::new();
    input.insert("journal".into(), json!(journal));
    if provider == "local" {
        input.insert("model_id".into(), json!(LOCAL_MODEL));
        readiness::inspect_local(input)
    } else {
        readiness::inspect_parakeet(input)
    }
}

pub fn migrate_legacy_provider_artifact_truth(
    journal: &Path,
) -> Result<ProviderInstallMigration, String> {
    let config = solstone_core_journal_config::read_journal_config(journal)
        .map_err(|error| error.to_string())?
        .config
        .unwrap_or_default();
    let mut clean_local = false;
    let mut clean_parakeet = false;
    let mut actions = Vec::new();

    for provider in ["local", "parakeet"] {
        let Some(legacy) =
            legacy_record(&config, provider).filter(|record| legacy_has_state(record))
        else {
            continue;
        };
        let Some(_lease) = lease::acquire(journal, provider).map_err(|error| error.to_string())?
        else {
            actions.push(format!(
                "{provider} install is in progress; legacy provider state will be retried on the next start."
            ));
            continue;
        };
        let inspected = inspect(journal, provider);
        let initial_status = inspected
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("missing-or-mismatched");
        let (target, canonical, digest) = target(provider, journal)?;
        let promoted = if initial_status == "ready" {
            Ok(())
        } else if matches!(initial_status, "proof-unavailable" | "host-ineligible") {
            Err(inspected
                .get("reason_code")
                .and_then(Value::as_str)
                .unwrap_or(initial_status)
                .to_owned())
        } else if provider == "local" {
            promote_legacy_local(journal, legacy, &target, &digest)
        } else {
            promote_legacy_parakeet(journal, legacy, &target, &digest)
        };
        let final_inspected = promoted.is_ok().then(|| inspect(journal, provider));
        let ready = final_inspected
            .as_ref()
            .is_some_and(|value| value.get("status").and_then(Value::as_str) == Some("ready"));
        if promoted.is_ok() && ready {
            publish_ready(journal, provider, canonical, digest)?;
            if provider == "local" {
                clean_local = true;
            } else {
                clean_parakeet = true;
            }
            actions.push(format!(
                "{provider} provider install proof was promoted to provider-owned status."
            ));
        } else {
            let reason = promoted
                .err()
                .or_else(|| {
                    final_inspected.and_then(|value| {
                        value
                            .get("reason_code")
                            .and_then(Value::as_str)
                            .map(str::to_owned)
                    })
                })
                .unwrap_or_else(|| "manifest_missing".to_owned());
            actions.push(format!(
                "Existing {provider} artifacts were not promoted ({reason}); a reinstall will rebuild provider-owned proof."
            ));
        }
    }

    let cleanup = solstone_core_journal_config_write::cleanup_legacy_provider_install_config(
        journal,
        clean_local,
        clean_parakeet,
    )
    .map_err(|error| error.to_string())?
    .value;
    Ok(ProviderInstallMigration {
        actions,
        removed: cleanup.removed,
        moved: cleanup.moved,
    })
}
