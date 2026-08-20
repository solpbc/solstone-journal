// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native speakers-analyze installation validation and generation borrowing.

use std::collections::BTreeMap;
use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};
use solstone_core_journal_io::{
    JsonWriteOptions, LeaseOptions, MalformedPolicy, acquire_file_lease, read_json, write_json,
};

use crate::args::{CliError, installation_error};
use crate::model_assets::resolve_model_asset_path;
use crate::resolve_model_asset;

const HELPER_BINARY_NAME: &str = "solstone-core-speakers-analyze";
const HELPER_BINARY_ENV: &str = "SOLSTONE_SPEAKERS_ANALYZE_BINARY";
const GENERATION_ENV_KEY: &str = "SOL_SPEAKERS_ANALYZE_INSTALL_GENERATION_ID";
const GENERATION_FD_ENV_KEY: &str = "SOL_SPEAKERS_ANALYZE_INSTALL_GENERATION_FD";
const GENERATION_TOKEN_ENV_KEY: &str = "SOL_SPEAKERS_ANALYZE_INSTALL_GENERATION_TOKEN";
const INSTALL_GENERATION_SCHEMA: &str = "solstone.speakers_analyze.install_generation.v1";
const PROOF_KEY_SCHEMA: &str = "solstone.speakers_analyze.install_proof_key.v1";
const WESPEAKER_ASSET: &str = "wespeaker-resnet34-256.onnx";
const PYANNOTE_ASSET: &str = "pyannote-segmentation-3.0.onnx";

/// A held or inherited speakers-analyze installation generation.
///
/// The owner keeps the advisory lease alive for its descendants. A borrower
/// retains no lease because its inherited descriptor is owned by the parent.
#[derive(Debug)]
pub struct SpeakersAnalyzeGeneration {
    _lease: Option<solstone_core_journal_io::FileLease>,
    inherited_fd: Option<i32>,
    environment: BTreeMap<OsString, OsString>,
}

impl SpeakersAnalyzeGeneration {
    /// Environment passed only to native child commands that can borrow this generation.
    pub fn inheritance_environment(&self) -> BTreeMap<OsString, OsString> {
        self.environment.clone()
    }
}

impl Drop for SpeakersAnalyzeGeneration {
    fn drop(&mut self) {
        if let Some(fd) = self.inherited_fd.take() {
            let _ = nix::unistd::close(fd);
        }
    }
}

/// Enter a validated installation generation for this journal process tree.
pub fn enter_speakers_analyze_generation(
    journal: &Path,
) -> Result<SpeakersAnalyzeGeneration, CliError> {
    let proof = installation_proof()?;
    let generation_path = generation_path(journal);
    let lease_path = generation_lock_path(journal);

    if borrowed_generation_matches(&generation_path, &lease_path, &proof) {
        return Ok(SpeakersAnalyzeGeneration {
            _lease: None,
            inherited_fd: None,
            environment: inherited_environment(),
        });
    }

    let lease = acquire_file_lease(
        &lease_path,
        LeaseOptions {
            attempts: 1,
            retry_max: Duration::ZERO,
            ..LeaseOptions::default()
        },
    )
    .map_err(|error| installation_error(format!("generation-lease: {error}")))?
    .ok_or_else(|| installation_error("generation-lease-contended"))?;

    // A lease holder always validates the pinned bytes before publishing a
    // record that descendants may borrow.
    validate_model_assets()?;
    let id = random_hex()?;
    let token = random_hex()?;
    fs::write(lease.path(), &token)
        .map_err(|error| installation_error(format!("generation-token: {error}")))?;
    let fd = duplicate_for_inheritance(&lease)?;
    let record = json!({
        "schema": INSTALL_GENERATION_SCHEMA,
        "id": id,
        "token": token,
        "proof": proof,
    });
    write_json(
        &generation_path,
        &record,
        JsonWriteOptions {
            mode: Some(0o600),
            indent: Some(2),
            sort_keys: true,
        },
    )
    .map_err(|error| installation_error(format!("generation-record: {error}")))?;
    Ok(SpeakersAnalyzeGeneration {
        _lease: Some(lease),
        inherited_fd: Some(fd),
        environment: BTreeMap::from([
            (OsString::from(GENERATION_ENV_KEY), OsString::from(id)),
            (
                OsString::from(GENERATION_FD_ENV_KEY),
                OsString::from(fd.to_string()),
            ),
            (
                OsString::from(GENERATION_TOKEN_ENV_KEY),
                OsString::from(token),
            ),
        ]),
    })
}

/// Fully validate the helper and both pinned model assets for an invocation.
pub(crate) fn validate_speakers_analyze_runtime() -> Result<ValidatedInstallation, CliError> {
    let proof = installation_proof()?;
    let wespeaker_model = resolve_model_asset(WESPEAKER_ASSET)
        .map_err(|error| installation_error(format!("asset-missing: {error}")))?;
    let pyannote_model = resolve_model_asset(PYANNOTE_ASSET)
        .map_err(|error| installation_error(format!("asset-missing: {error}")))?;
    let helper = PathBuf::from(
        proof
            .get("helper")
            .and_then(|helper| helper.get("path"))
            .and_then(Value::as_str)
            .expect("installation proof always contains helper path"),
    );
    Ok(ValidatedInstallation {
        helper,
        wespeaker_model,
        pyannote_model,
    })
}

/// Paths used for one validated helper invocation.
#[derive(Debug)]
pub(crate) struct ValidatedInstallation {
    pub(crate) helper: PathBuf,
    pub(crate) wespeaker_model: PathBuf,
    pub(crate) pyannote_model: PathBuf,
}

fn installation_proof() -> Result<Value, CliError> {
    check_platform_coverage()?;
    let helper = helper_path()?;
    let wespeaker = resolve_model_asset_path(WESPEAKER_ASSET)
        .map_err(|error| installation_error(format!("asset-missing: {error}")))?;
    let pyannote = resolve_model_asset_path(PYANNOTE_ASSET)
        .map_err(|error| installation_error(format!("asset-missing: {error}")))?;
    Ok(json!({
        "schema": PROOF_KEY_SCHEMA,
        "platform": runtime_platform(),
        "helper": file_stamp(&helper)?,
        "assets": [
            { "name": WESPEAKER_ASSET, "file": file_stamp(&wespeaker)? },
            { "name": PYANNOTE_ASSET, "file": file_stamp(&pyannote)? },
        ],
    }))
}

fn check_platform_coverage() -> Result<(), CliError> {
    let (platform, architecture) = runtime_platform();
    let covered = matches!(
        (platform, architecture),
        ("linux", "x86_64" | "aarch64") | ("darwin", "arm64")
    );
    covered.then_some(()).ok_or_else(|| {
        installation_error(format!("platform-unsupported: {platform}/{architecture}"))
    })
}

fn runtime_platform() -> (&'static str, &'static str) {
    match (env::consts::OS, env::consts::ARCH) {
        ("macos", "aarch64") => ("darwin", "arm64"),
        (platform, architecture) => (platform, architecture),
    }
}

fn helper_path() -> Result<PathBuf, CliError> {
    let executable =
        env::current_exe().map_err(|error| installation_error(format!("helper-path: {error}")))?;
    let directory = executable.parent().ok_or_else(|| {
        installation_error("helper-path: current executable has no parent directory")
    })?;
    let candidate = match env::var(HELPER_BINARY_ENV) {
        Ok(path) if !path.is_empty() => PathBuf::from(path),
        _ => directory.join(HELPER_BINARY_NAME),
    };
    if !is_executable(&candidate) {
        return Err(installation_error(format!(
            "helper-not-executable: {}",
            candidate.display()
        )));
    }
    Ok(candidate)
}

fn validate_model_assets() -> Result<(), CliError> {
    resolve_model_asset(WESPEAKER_ASSET)
        .map_err(|error| installation_error(format!("asset-missing: {error}")))?;
    resolve_model_asset(PYANNOTE_ASSET)
        .map_err(|error| installation_error(format!("asset-missing: {error}")))?;
    Ok(())
}

fn file_stamp(path: &Path) -> Result<Value, CliError> {
    let metadata = fs::metadata(path).map_err(|error| {
        installation_error(format!("proof-metadata: {}: {error}", path.display()))
    })?;
    let modified_ns = metadata
        .modified()
        .ok()
        .and_then(|value| value.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|value| value.as_nanos().to_string());
    #[cfg(unix)]
    let mode = {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o777
    };
    #[cfg(not(unix))]
    let mode = 0;
    Ok(json!({
        "path": path.to_string_lossy(),
        "size": metadata.len(),
        "modified_ns": modified_ns,
        "mode": mode,
    }))
}

fn borrowed_generation_matches(generation_path: &Path, lease_path: &Path, proof: &Value) -> bool {
    let (Ok(id), Ok(fd), Ok(token)) = (
        env::var(GENERATION_ENV_KEY),
        env::var(GENERATION_FD_ENV_KEY),
        env::var(GENERATION_TOKEN_ENV_KEY),
    ) else {
        return false;
    };
    if !inherited_fd_is_open(&fd) || fs::read_to_string(lease_path).ok().as_deref() != Some(&token)
    {
        return false;
    }
    let Ok(record) = read_json(generation_path, Value::Null, MalformedPolicy::Skip) else {
        return false;
    };
    record.get("schema").and_then(Value::as_str) == Some(INSTALL_GENERATION_SCHEMA)
        && record.get("id").and_then(Value::as_str) == Some(&id)
        && record.get("token").and_then(Value::as_str) == Some(&token)
        && record.get("proof") == Some(proof)
}

#[cfg(unix)]
fn duplicate_for_inheritance(lease: &solstone_core_journal_io::FileLease) -> Result<i32, CliError> {
    lease
        .duplicate_for_inheritance()
        .map_err(|error| installation_error(format!("generation-fd: {error}")))
}

#[cfg(not(unix))]
fn duplicate_for_inheritance(_: &solstone_core_journal_io::FileLease) -> Result<i32, CliError> {
    Err(installation_error("generation-fd: unsupported platform"))
}

#[cfg(unix)]
fn inherited_fd_is_open(fd: &str) -> bool {
    let Ok(fd) = fd.parse::<i32>() else {
        return false;
    };
    Path::new("/dev/fd").join(fd.to_string()).exists()
}

#[cfg(not(unix))]
fn inherited_fd_is_open(_: &str) -> bool {
    false
}

fn random_hex() -> Result<String, CliError> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes)
        .map_err(|error| installation_error(format!("generation-random: {error}")))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn inherited_environment() -> BTreeMap<OsString, OsString> {
    [
        GENERATION_ENV_KEY,
        GENERATION_FD_ENV_KEY,
        GENERATION_TOKEN_ENV_KEY,
    ]
    .into_iter()
    .filter_map(|key| env::var_os(key).map(|value| (OsString::from(key), value)))
    .collect()
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;

    fs::metadata(path)
        .ok()
        .is_some_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

fn generation_path(journal: &Path) -> PathBuf {
    journal.join("health/speakers-analyze/install-generation.json")
}

fn generation_lock_path(journal: &Path) -> PathBuf {
    journal.join("health/speakers-analyze/install-generation.lock")
}

#[cfg(test)]
mod tests {
    use super::{check_platform_coverage, runtime_platform};

    #[test]
    fn runtime_platform_normalizes_macos_arm64_for_wheel_coverage() {
        let (platform, architecture) = runtime_platform();
        if std::env::consts::OS == "macos" && std::env::consts::ARCH == "aarch64" {
            assert_eq!((platform, architecture), ("darwin", "arm64"));
        } else {
            assert_eq!(
                (platform, architecture),
                (std::env::consts::OS, std::env::consts::ARCH)
            );
        }
    }

    #[test]
    fn current_target_has_covered_speakers_analyze_runtime() {
        check_platform_coverage().unwrap();
    }

    #[test]
    fn runtime_platform_is_a_known_native_target_shape() {
        let (platform, architecture) = runtime_platform();
        assert!(matches!(
            (platform, architecture),
            ("linux", "x86_64" | "aarch64") | ("darwin", "arm64")
        ));
    }
}
