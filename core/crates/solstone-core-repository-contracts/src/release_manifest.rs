// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Canonical offline validation for the fleet Rust release-manifest v1 contract.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const PRODUCT: &str = "solstone-journal";
const CARGO_DENY_VERSION: &str = "cargo-deny 0.20.2";
const SCHEMA_ID: &str = "https://solpbc.org/schemas/rust-release-manifest/v1.json";
const SCHEMA_DIALECT: &str = "https://json-schema.org/draft/2020-12/schema";
const SCHEMA_SHA256: &str = "d4eabf52bcc68b56945912d351f818e5444fe8c6461cb5c48b096f87b17a875c";
const SCHEMA_BYTES: &[u8] = include_bytes!("../../../../schemas/rust-release-manifest/v1.json");

#[derive(Clone, Debug)]
pub enum ManifestSelection {
    SelfTest,
    Manifest(PathBuf),
    ReleaseDir(PathBuf),
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    schema_version: u64,
    product: String,
    version: String,
    source_commit: String,
    source_dirty: bool,
    cargo_lock_sha256: String,
    rust: RustEvidence,
    target: TargetEvidence,
    native_tools: BTreeMap<String, String>,
    dependency_policy: DependencyPolicy,
    active_exceptions: Vec<String>,
    artifacts: Vec<ArtifactEvidence>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RustEvidence {
    rustc_verbose: String,
    cargo_version: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "kind", deny_unknown_fields)]
enum TargetEvidence {
    #[serde(rename = "compiled")]
    Compiled {
        triple: String,
        profile: String,
        features: Vec<String>,
    },
    #[serde(rename = "source")]
    Source,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct DependencyPolicy {
    cargo_deny_version: String,
    deterministic_gate: String,
    advisory_checked_at: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ArtifactEvidence {
    path: String,
    sha256: String,
    bytes: u64,
}

#[derive(Serialize)]
struct ManifestWitness {
    schema: &'static str,
    product: &'static str,
    manifests: usize,
    artifacts: usize,
    controls: usize,
    verdict: &'static str,
}

pub fn run_manifest_check(repo: &Path, selection: ManifestSelection) -> Result<String, String> {
    verify_schema(repo)?;
    let (manifests, artifacts, controls) = match selection {
        ManifestSelection::SelfTest => run_self_test(repo)?,
        ManifestSelection::Manifest(path) => {
            let count = validate_manifest_path(repo, &path)?;
            (1, count, 0)
        }
        ManifestSelection::ReleaseDir(path) => validate_release_dir(repo, &path)?,
    };
    serde_json::to_string(&ManifestWitness {
        schema: "solstone.rust-release-manifest-check.v1",
        product: PRODUCT,
        manifests,
        artifacts,
        controls,
        verdict: "pass",
    })
    .map_err(|error| format!("release manifest witness serialization failed: {error}"))
}

fn verify_schema(repo: &Path) -> Result<(), String> {
    if sha256_hex(SCHEMA_BYTES) != SCHEMA_SHA256 {
        return Err("embedded rust release-manifest schema digest is not canonical".to_owned());
    }
    let vendored = fs::read(repo.join("schemas/rust-release-manifest/v1.json"))
        .map_err(|error| format!("read vendored rust release-manifest schema: {error}"))?;
    if vendored != SCHEMA_BYTES || sha256_hex(&vendored) != SCHEMA_SHA256 {
        return Err(
            "vendored rust release-manifest schema differs from canonical v1 bytes".to_owned(),
        );
    }
    let schema: serde_json::Value = serde_json::from_slice(SCHEMA_BYTES)
        .map_err(|error| format!("parse canonical rust release-manifest schema: {error}"))?;
    if schema.get("$id").and_then(serde_json::Value::as_str) != Some(SCHEMA_ID)
        || schema.get("$schema").and_then(serde_json::Value::as_str) != Some(SCHEMA_DIALECT)
    {
        return Err("canonical rust release-manifest schema identity is invalid".to_owned());
    }
    Ok(())
}

fn validate_release_dir(repo: &Path, path: &Path) -> Result<(usize, usize, usize), String> {
    let directory = safe_directory(path, "release directory")?;
    let mut manifests = Vec::new();
    for entry in fs::read_dir(&directory)
        .map_err(|error| format!("read release directory {}: {error}", directory.display()))?
    {
        let entry = entry.map_err(|error| format!("read release directory entry: {error}"))?;
        let name = entry.file_name();
        let name = name
            .to_str()
            .ok_or_else(|| "release directory contains a non-UTF-8 filename".to_owned())?;
        if name.ends_with(".rust-release-manifest.json") {
            manifests.push(entry.path());
        }
    }
    manifests.sort();
    if manifests.is_empty() {
        return Err("release directory contains no *.rust-release-manifest.json files".to_owned());
    }
    let mut artifacts = 0;
    for manifest in &manifests {
        artifacts += validate_manifest_path(repo, manifest)?;
    }
    Ok((manifests.len(), artifacts, 0))
}

fn validate_manifest_path(repo: &Path, path: &Path) -> Result<usize, String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("read manifest metadata {}: {error}", path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(format!(
            "manifest is not one regular non-link file: {}",
            path.display()
        ));
    }
    let canonical = fs::canonicalize(path)
        .map_err(|error| format!("resolve manifest {}: {error}", path.display()))?;
    let bytes = fs::read(&canonical)
        .map_err(|error| format!("read manifest {}: {error}", path.display()))?;
    let parent = canonical
        .parent()
        .ok_or_else(|| "manifest path has no parent".to_owned())?;
    validate_manifest_bytes(repo, parent, &bytes)
}

fn validate_manifest_bytes(repo: &Path, parent: &Path, bytes: &[u8]) -> Result<usize, String> {
    let manifest: Manifest = serde_json::from_slice(bytes)
        .map_err(|error| format!("manifest is not canonical v1 JSON: {error}"))?;
    validate_semantics(repo, parent, &manifest)?;
    Ok(manifest.artifacts.len())
}

fn validate_semantics(repo: &Path, parent: &Path, manifest: &Manifest) -> Result<(), String> {
    if manifest.schema_version != 1 {
        return Err("manifest schema_version must be 1".to_owned());
    }
    if manifest.product != PRODUCT {
        return Err(format!("manifest product must be {PRODUCT}"));
    }
    require_nonempty("version", &manifest.version)?;
    if !is_lower_hex(&manifest.source_commit, &[40, 64]) {
        return Err("manifest source_commit must be one full lowercase Git object id".to_owned());
    }
    if manifest.source_dirty {
        return Err("manifest source_dirty must be false".to_owned());
    }
    let lock = fs::read(repo.join("core/Cargo.lock"))
        .map_err(|error| format!("read core/Cargo.lock: {error}"))?;
    if manifest.cargo_lock_sha256 != sha256_hex(&lock) {
        return Err("manifest cargo_lock_sha256 does not match core/Cargo.lock".to_owned());
    }
    require_nonempty("rust.rustc_verbose", &manifest.rust.rustc_verbose)?;
    require_nonempty("rust.cargo_version", &manifest.rust.cargo_version)?;
    match &manifest.target {
        TargetEvidence::Compiled {
            triple,
            profile,
            features,
        } => {
            require_nonempty("target.triple", triple)?;
            require_nonempty("target.profile", profile)?;
            require_sorted_unique("target.features", features)?;
        }
        TargetEvidence::Source => {}
    }
    for (name, value) in &manifest.native_tools {
        require_nonempty("native_tools key", name)?;
        require_nonempty("native_tools value", value)?;
    }
    if manifest.dependency_policy.cargo_deny_version != CARGO_DENY_VERSION {
        return Err(format!(
            "manifest dependency_policy.cargo_deny_version must be {CARGO_DENY_VERSION}"
        ));
    }
    if manifest.dependency_policy.deterministic_gate != "pass" {
        return Err("manifest dependency_policy.deterministic_gate must be pass".to_owned());
    }
    if !is_canonical_utc(&manifest.dependency_policy.advisory_checked_at) {
        return Err(
            "manifest dependency_policy.advisory_checked_at must be canonical UTC seconds"
                .to_owned(),
        );
    }
    require_sorted_unique("active_exceptions", &manifest.active_exceptions)?;
    if manifest.artifacts.is_empty() {
        return Err("manifest artifacts must not be empty".to_owned());
    }
    let mut previous = None;
    let mut folded = BTreeSet::new();
    for artifact in &manifest.artifacts {
        validate_relative_artifact_path(&artifact.path)?;
        if previous
            .as_deref()
            .is_some_and(|value: &str| value >= artifact.path.as_str())
        {
            return Err("manifest artifacts must be sorted by unique path".to_owned());
        }
        previous = Some(artifact.path.clone());
        if !folded.insert(artifact.path.to_ascii_lowercase()) {
            return Err("manifest artifact paths collide under ASCII case folding".to_owned());
        }
        if !is_lower_hex(&artifact.sha256, &[64]) {
            return Err("manifest artifact sha256 is malformed".to_owned());
        }
        if artifact.bytes == 0 {
            return Err("manifest artifact bytes must be positive".to_owned());
        }
        let actual = read_contained_artifact(parent, &artifact.path)?;
        if actual.len() as u64 != artifact.bytes {
            return Err(format!(
                "manifest artifact byte count differs: {}",
                artifact.path
            ));
        }
        if sha256_hex(&actual) != artifact.sha256 {
            return Err(format!(
                "manifest artifact digest differs: {}",
                artifact.path
            ));
        }
    }
    Ok(())
}

fn read_contained_artifact(parent: &Path, relative: &str) -> Result<Vec<u8>, String> {
    let root = fs::canonicalize(parent)
        .map_err(|error| format!("resolve manifest artifact root: {error}"))?;
    let mut candidate = root.clone();
    for component in Path::new(relative).components() {
        let Component::Normal(segment) = component else {
            return Err("manifest artifact path is not a safe relative path".to_owned());
        };
        candidate.push(segment);
        let metadata = fs::symlink_metadata(&candidate)
            .map_err(|error| format!("read artifact metadata {}: {error}", candidate.display()))?;
        if metadata.file_type().is_symlink() {
            return Err(format!(
                "manifest artifact path contains a symbolic link: {relative}"
            ));
        }
    }
    let metadata = fs::symlink_metadata(&candidate)
        .map_err(|error| format!("read artifact metadata {}: {error}", candidate.display()))?;
    if !metadata.is_file() {
        return Err(format!(
            "manifest artifact is not one regular non-link file: {relative}"
        ));
    }
    let resolved = fs::canonicalize(&candidate)
        .map_err(|error| format!("resolve artifact {relative}: {error}"))?;
    if !resolved.starts_with(&root) {
        return Err(format!(
            "manifest artifact resolves outside its manifest directory: {relative}"
        ));
    }
    fs::read(&resolved).map_err(|error| format!("read artifact {relative}: {error}"))
}

fn run_self_test(repo: &Path) -> Result<(usize, usize, usize), String> {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("read system clock: {error}"))?
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "solstone-journal-release-manifest-selftest-{}-{nonce}",
        std::process::id()
    ));
    fs::create_dir(&root).map_err(|error| format!("create manifest self-test root: {error}"))?;
    let result = run_self_test_in(repo, &root);
    let cleanup = fs::remove_dir_all(&root);
    match (result, cleanup) {
        (Err(error), _) => Err(error),
        (Ok(_), Err(error)) => Err(format!("remove manifest self-test root: {error}")),
        (Ok(value), Ok(())) => Ok(value),
    }
}

fn run_self_test_in(repo: &Path, root: &Path) -> Result<(usize, usize, usize), String> {
    let artifact = root.join("journal.tar.zst");
    fs::write(&artifact, b"canonical synthetic journal artifact\n")
        .map_err(|error| format!("write manifest self-test artifact: {error}"))?;
    let lock = fs::read(repo.join("core/Cargo.lock"))
        .map_err(|error| format!("read core/Cargo.lock: {error}"))?;
    let mut manifest = Manifest {
        schema_version: 1,
        product: PRODUCT.to_owned(),
        version: "0.0.0-selftest".to_owned(),
        source_commit: "0123456789abcdef0123456789abcdef01234567".to_owned(),
        source_dirty: false,
        cargo_lock_sha256: sha256_hex(&lock),
        rust: RustEvidence {
            rustc_verbose: "rustc 1.97.1 (self-test)".to_owned(),
            cargo_version: "cargo 1.97.1 (self-test)".to_owned(),
        },
        target: TargetEvidence::Source,
        native_tools: BTreeMap::new(),
        dependency_policy: DependencyPolicy {
            cargo_deny_version: CARGO_DENY_VERSION.to_owned(),
            deterministic_gate: "pass".to_owned(),
            advisory_checked_at: "2026-08-30T00:00:00Z".to_owned(),
        },
        active_exceptions: vec!["RUSTSEC-2023-0071".to_owned()],
        artifacts: vec![ArtifactEvidence {
            path: "journal.tar.zst".to_owned(),
            sha256: sha256_hex(b"canonical synthetic journal artifact\n"),
            bytes: 37,
        }],
    };
    let valid = serde_json::to_vec(&manifest)
        .map_err(|error| format!("serialize manifest self-test fixture: {error}"))?;
    let deterministic = serde_json::to_vec(&manifest)
        .map_err(|error| format!("repeat manifest self-test serialization: {error}"))?;
    if valid != deterministic {
        return Err("manifest serialization is not deterministic".to_owned());
    }
    validate_manifest_bytes(repo, root, &valid)?;

    let mut controls = 0;
    let mut reject = |label: &str, candidate: &Manifest| -> Result<(), String> {
        controls += 1;
        let bytes = serde_json::to_vec(candidate)
            .map_err(|error| format!("serialize {label} control: {error}"))?;
        if validate_manifest_bytes(repo, root, &bytes).is_ok() {
            return Err(format!(
                "manifest negative control unexpectedly passed: {label}"
            ));
        }
        Ok(())
    };

    manifest.schema_version = 2;
    reject("schema-version", &manifest)?;
    manifest.schema_version = 1;
    manifest.product = "solstone-windows".to_owned();
    reject("product", &manifest)?;
    manifest.product = PRODUCT.to_owned();
    manifest.source_dirty = true;
    reject("dirty-source", &manifest)?;
    manifest.source_dirty = false;
    manifest.cargo_lock_sha256 = "0".repeat(64);
    reject("lock-digest", &manifest)?;
    manifest.cargo_lock_sha256 = sha256_hex(&lock);
    manifest.dependency_policy.cargo_deny_version = "cargo-deny 0.20.1".to_owned();
    reject("cargo-deny-version", &manifest)?;
    manifest.dependency_policy.cargo_deny_version = CARGO_DENY_VERSION.to_owned();
    manifest.dependency_policy.advisory_checked_at = "2026-08-30 00:00:00Z".to_owned();
    reject("advisory-timestamp", &manifest)?;
    manifest.dependency_policy.advisory_checked_at = "2026-08-30T00:00:00Z".to_owned();
    manifest.artifacts[0].path = "../journal.tar.zst".to_owned();
    reject("artifact-traversal", &manifest)?;
    manifest.artifacts[0].path = "journal.tar.zst".to_owned();
    manifest.artifacts[0].sha256 = "1".repeat(64);
    reject("artifact-digest", &manifest)?;
    manifest.artifacts[0].sha256 = sha256_hex(b"canonical synthetic journal artifact\n");
    manifest.artifacts[0].bytes = 1;
    reject("artifact-bytes", &manifest)?;
    manifest.artifacts[0].bytes = 37;
    manifest.active_exceptions = vec!["z".to_owned(), "a".to_owned()];
    reject("exception-order", &manifest)?;
    manifest.active_exceptions = vec!["RUSTSEC-2023-0071".to_owned()];
    validate_semantics(repo, root, &manifest)?;

    controls += 1;
    let mut unknown: serde_json::Value = serde_json::from_slice(&valid)
        .map_err(|error| format!("parse unknown-field control: {error}"))?;
    unknown
        .as_object_mut()
        .ok_or_else(|| "valid manifest fixture is not an object".to_owned())?
        .insert("unknown".to_owned(), serde_json::Value::Bool(true));
    let unknown = serde_json::to_vec(&unknown)
        .map_err(|error| format!("serialize unknown-field control: {error}"))?;
    if validate_manifest_bytes(repo, root, &unknown).is_ok() {
        return Err("manifest negative control unexpectedly passed: unknown-field".to_owned());
    }

    Ok((1, 1, controls))
}

fn safe_directory(path: &Path, label: &str) -> Result<PathBuf, String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("read {label} metadata {}: {error}", path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(format!(
            "{label} is not one non-link directory: {}",
            path.display()
        ));
    }
    fs::canonicalize(path).map_err(|error| format!("resolve {label} {}: {error}", path.display()))
}

fn validate_relative_artifact_path(value: &str) -> Result<(), String> {
    if value.is_empty()
        || value.contains('\\')
        || value.bytes().any(|byte| byte.is_ascii_control())
        || value.as_bytes().get(1) == Some(&b':')
    {
        return Err("manifest artifact path is not portable".to_owned());
    }
    let path = Path::new(value);
    if path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err("manifest artifact path is not a safe relative path".to_owned());
    }
    Ok(())
}

fn require_nonempty(field: &str, value: &str) -> Result<(), String> {
    if value.is_empty() {
        Err(format!("manifest {field} must not be empty"))
    } else {
        Ok(())
    }
}

fn require_sorted_unique(field: &str, values: &[String]) -> Result<(), String> {
    if values.iter().any(String::is_empty) || values.windows(2).any(|pair| pair[0] >= pair[1]) {
        Err(format!("manifest {field} must be sorted and unique"))
    } else {
        Ok(())
    }
}

fn is_lower_hex(value: &str, lengths: &[usize]) -> bool {
    lengths.contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn is_canonical_utc(value: &str) -> bool {
    if value.len() != 20 {
        return false;
    }
    let bytes = value.as_bytes();
    if bytes[4] != b'-'
        || bytes[7] != b'-'
        || bytes[10] != b'T'
        || bytes[13] != b':'
        || bytes[16] != b':'
        || bytes[19] != b'Z'
    {
        return false;
    }
    for index in [0, 1, 2, 3, 5, 6, 8, 9, 11, 12, 14, 15, 17, 18] {
        if !bytes[index].is_ascii_digit() {
            return false;
        }
    }
    let number = |start: usize, end: usize| value[start..end].parse::<u32>().unwrap_or(u32::MAX);
    let year = number(0, 4);
    let month = number(5, 7);
    let day = number(8, 10);
    let hour = number(11, 13);
    let minute = number(14, 16);
    let second = number(17, 19);
    let leap = year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400));
    let days = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => 0,
    };
    year > 0 && day > 0 && day <= days && hour < 24 && minute < 60 && second < 60
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::{is_canonical_utc, read_contained_artifact, validate_relative_artifact_path};
    #[cfg(unix)]
    use std::fs;

    #[test]
    fn utc_parser_has_positive_and_coordinate_varying_controls() {
        assert!(is_canonical_utc("2026-08-30T23:59:59Z"));
        assert!(is_canonical_utc("2024-02-29T00:00:00Z"));
        assert!(!is_canonical_utc("2025-02-29T00:00:00Z"));
        assert!(!is_canonical_utc("2026-08-30T24:00:00Z"));
        assert!(!is_canonical_utc("2026-08-30T23:60:00Z"));
        assert!(!is_canonical_utc("2026-08-30T23:59:60Z"));
    }

    #[test]
    fn portable_path_gate_rejects_each_unsafe_coordinate() {
        assert!(validate_relative_artifact_path("journal.tar.zst").is_ok());
        for path in ["", "/tmp/a", "../a", "a/../b", "a\\b", "C:/a", "./a"] {
            assert!(validate_relative_artifact_path(path).is_err(), "{path}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn artifact_reader_rejects_a_symbolic_link_in_any_path_component() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().expect("artifact root");
        let outside = tempfile::tempdir().expect("outside root");
        fs::write(outside.path().join("journal.tar.zst"), b"outside").expect("outside artifact");
        symlink(outside.path(), root.path().join("nested")).expect("nested symlink");

        let error = read_contained_artifact(root.path(), "nested/journal.tar.zst")
            .expect_err("nested symlink must be rejected");
        assert!(error.contains("symbolic link"), "{error}");
    }
}
