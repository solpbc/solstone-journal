// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Atomic installation and verification of the Parakeet CoreML model tree.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use solstone_core_assets::{Artifact, Platform, RuntimeFetchQuery, canonical_host_pair, catalog};
use solstone_core_journal_config::{
    JournalConfigRead,
    parakeet_coreml::{
        ParakeetCoremlFile, ParakeetCoremlSentinel, default_parakeet_coreml_cache_dir,
        parakeet_coreml_cache_dir, parakeet_coreml_model_root, parakeet_coreml_sentinel_path,
        read_valid_parakeet_coreml_sentinel,
    },
};
use solstone_core_journal_io::{
    AtomicWriteOptions, DEFAULT_LOCK_POLL_INTERVAL, DEFAULT_LOCK_TIMEOUT, LockError, LockOptions,
    atomic_replace, hold_lock,
};
use thiserror::Error;

use super::archive;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PublicationMarker {
    phase: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tree_canonical: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tree_backup: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sentinel_backup: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    staging: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    published_catalog_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    published_files: Option<Vec<ParakeetCoremlFile>>,
}

fn write_publication_marker(path: &Path, marker: &PublicationMarker) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut bytes = serde_json::to_vec_pretty(marker)?;
    bytes.push(b'\n');
    atomic_replace(path, &bytes, AtomicWriteOptions::default())
        .map_err(|e| std::io::Error::other(e.to_string()))
}

/// Recover any interrupted CoreML publication.
///
/// Callers hold the publication lock, except single-threaded tests.
fn hold_coreml_admission(
    home: &Path,
) -> Result<solstone_core_journal_io::FileLock, CoremlInstallError> {
    hold_lock(
        default_parakeet_coreml_cache_dir(home).join(".admission.lock"),
        LockOptions::default(),
    )
    .map_err(|error| CoremlInstallError::new("coreml_busy", error.to_string(), 74))
}

/// Hold installed model reads through the helper lifetime. Readers coexist;
/// only tree/receipt publication excludes them, never downloading.
pub fn hold_parakeet_coreml_read_admission(
    home: &Path,
) -> Result<solstone_core_journal_io::FileLock, CoremlInstallError> {
    let cache = default_parakeet_coreml_cache_dir(home);
    let marker = cache.join(".publication.json");
    for _ in 0..2 {
        let path = cache.join(".admission.lock");
        #[cfg(unix)]
        let result = solstone_core_journal_io::hold_shared_lock(path, LockOptions::default());
        #[cfg(windows)]
        let result = hold_lock(path, LockOptions::default());
        let guard = result
            .map_err(|error| CoremlInstallError::new("coreml_busy", error.to_string(), 74))?;
        if !marker.exists() {
            return Ok(guard);
        }
        let committed = fs::read(&marker)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<PublicationMarker>(&bytes).ok())
            .is_some_and(|marker| publication_was_committed(home, &marker));
        // Cleanup of an obsolete backup belongs to the installer, not a model
        // reader. A committed canonical installation remains admitted.
        if committed {
            return Ok(guard);
        }
        drop(guard);
        recover_parakeet_coreml_publication(home)?;
    }
    Err(CoremlInstallError::new(
        "coreml_busy",
        "parakeet CoreML publication lock busy",
        74,
    ))
}

/// Recover interrupted publication under the short exclusive admission lock.
/// The download worker lease is not needed: active publication owns this lock.
pub fn recover_parakeet_coreml_publication(home: &Path) -> Result<(), CoremlInstallError> {
    let _admission = hold_coreml_admission(home)?;
    recover_publication_locked(home)
}

fn publication_was_committed(home: &Path, marker: &PublicationMarker) -> bool {
    if marker.phase == "committed" {
        return true;
    }
    let Some(sentinel) = fs::read(parakeet_coreml_sentinel_path(home))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<ParakeetCoremlSentinel>(&bytes).ok())
    else {
        return false;
    };
    marker.phase == "pending"
        && marker.tree_canonical.as_ref().is_some_and(|tree| {
            tree.is_dir() && *tree == parakeet_coreml_model_root(sentinel.cache_dir())
        })
        && marker.published_catalog_version.as_deref() == Some(sentinel.catalog_version())
        && marker.published_files.as_deref() == Some(sentinel.files())
}

fn recover_publication_locked(home: &Path) -> Result<(), CoremlInstallError> {
    let default_cache = default_parakeet_coreml_cache_dir(home);
    let marker_path = default_cache.join(".publication.json");
    if !marker_path.exists() {
        return Ok(());
    }

    let bytes = match fs::read(&marker_path) {
        Ok(bytes) => bytes,
        Err(_) => return Ok(()),
    };
    let marker: PublicationMarker = match serde_json::from_slice(&bytes) {
        Ok(marker) => marker,
        Err(e) => {
            return Err(CoremlInstallError::new(
                "recovery_failed",
                e.to_string(),
                74,
            ));
        }
    };

    match marker.phase.as_str() {
        "committed" => {
            if let Some(tree_backup) = &marker.tree_backup
                && tree_backup.exists()
            {
                fs::remove_dir_all(tree_backup)
                    .map_err(|e| CoremlInstallError::new("recovery_failed", e.to_string(), 74))?;
            }
            if let Some(sentinel_backup) = &marker.sentinel_backup
                && sentinel_backup.exists()
            {
                fs::remove_file(sentinel_backup)
                    .map_err(|e| CoremlInstallError::new("recovery_failed", e.to_string(), 74))?;
            }
            let _ = fs::remove_file(&marker_path);
            Ok(())
        }
        "pending" => {
            let sentinel_path = parakeet_coreml_sentinel_path(home);
            let canonical_tree = marker.tree_canonical.as_ref();

            if publication_was_committed(home, &marker) {
                if let Some(tree_backup) = &marker.tree_backup
                    && tree_backup.exists()
                {
                    let _ = fs::remove_dir_all(tree_backup);
                }
                if let Some(sentinel_backup) = &marker.sentinel_backup
                    && sentinel_backup.exists()
                {
                    let _ = fs::remove_file(sentinel_backup);
                }
                if let Some(staging) = &marker.staging
                    && staging.exists()
                {
                    let _ = fs::remove_dir_all(staging);
                }
                let _ = fs::remove_file(&marker_path);
                return Ok(());
            }

            if !sentinel_path.exists()
                && let Some(sentinel_backup) = &marker.sentinel_backup
                && sentinel_backup.exists()
            {
                fs::rename(sentinel_backup, &sentinel_path)
                    .map_err(|e| CoremlInstallError::new("recovery_failed", e.to_string(), 74))?;
            }

            if let Some(tree_backup) = &marker.tree_backup
                && tree_backup.exists()
                && let Some(target) = canonical_tree
            {
                let had_target = target.exists();
                let discard = target.with_extension(format!(
                    "discard.{}.{}",
                    std::process::id(),
                    COUNTER.fetch_add(1, Ordering::Relaxed)
                ));
                if had_target {
                    fs::rename(target, &discard).map_err(|e| {
                        CoremlInstallError::new("recovery_failed", e.to_string(), 74)
                    })?;
                }
                if let Err(e) = fs::rename(tree_backup, target) {
                    if had_target {
                        let _ = fs::rename(&discard, target);
                    }
                    return Err(CoremlInstallError::new(
                        "recovery_failed",
                        e.to_string(),
                        74,
                    ));
                }
                if had_target {
                    let _ = fs::remove_dir_all(&discard);
                }
            }

            if let Some(staging) = &marker.staging
                && staging.exists()
            {
                let _ = fs::remove_dir_all(staging);
            }
            let _ = fs::remove_file(&marker_path);
            Ok(())
        }
        _ => {
            let _ = fs::remove_file(&marker_path);
            Ok(())
        }
    }
}

const UNIT: &str = "parakeet-coreml";
const FLUIDAUDIO_VERSION: &str = "0.14.0";
static COUNTER: AtomicU64 = AtomicU64::new(1);

/// Printed by `install-models` before it asks this installer to fetch assets.
pub const PARAKEET_COREML_DOWNLOAD_DISCLOSURE: &str = "parakeet assets: downloading the parakeet-tdt-0.6b-v3 Core ML model (CC-BY-4.0) from updates.solstone.app. see THIRD_PARTY_NOTICES.md.";

#[derive(Debug, Error)]
#[error("{message}")]
pub struct CoremlInstallError {
    pub reason_code: String,
    message: String,
    pub exit_code: u8,
}

impl CoremlInstallError {
    pub fn new(reason_code: impl Into<String>, message: impl Into<String>, exit_code: u8) -> Self {
        Self {
            reason_code: reason_code.into(),
            message: message.into(),
            exit_code,
        }
    }
}

fn rows() -> Result<Vec<&'static Artifact>, CoremlInstallError> {
    let rows = catalog()
        .iter()
        .filter(|artifact| artifact.unit == UNIT && artifact.platform == Some(Platform::MacosArm64))
        .collect::<Vec<_>>();
    Ok(rows)
}

fn shared_catalog_version(rows: &[&Artifact]) -> Result<&'static str, CoremlInstallError> {
    let first = rows.first().ok_or_else(|| {
        CoremlInstallError::new("catalog_empty", "no parakeet-coreml catalog rows found", 65)
    })?;
    let version = first.version;
    if rows.iter().any(|r| r.version != version) {
        return Err(CoremlInstallError::new(
            "catalog_version_mismatch",
            "parakeet-coreml catalog rows have conflicting versions",
            65,
        ));
    }
    Ok(version)
}

fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher)?;
    Ok(format!("{:x}", hasher.finalize()))
}

fn require_coreml_host(os_name: &str, arch: &str) -> Result<(), CoremlInstallError> {
    if os_name == "darwin" && arch == "arm64" {
        Ok(())
    } else {
        Err(CoremlInstallError::new(
            "platform_unsupported",
            format!("parakeet CoreML is only supported on darwin/arm64, not {os_name}/{arch}"),
            69,
        ))
    }
}

fn archive_error_reason_code(error: &archive::ArchiveError) -> &'static str {
    match error {
        archive::ArchiveError::HostRefused { .. } => "download_host_refused",
        archive::ArchiveError::InsecureScheme { .. } => "download_insecure_scheme",
        archive::ArchiveError::UrlUserinfoRefused { .. } => "download_url_userinfo_refused",
        archive::ArchiveError::SizeMismatch { .. } => "download_size_mismatch",
        archive::ArchiveError::DigestMismatch { .. } => "download_digest_mismatch",
        archive::ArchiveError::RedirectHopLimitExceeded { .. } => "download_redirect_hop_limit",
        archive::ArchiveError::OriginUnavailable { .. } => "download_origin_unreachable",
        archive::ArchiveError::Io(_) | archive::ArchiveError::Download(_) => "download_failed",
        archive::ArchiveError::PathEscape(_) => "download_failed",
    }
}

fn archive_error(error: archive::ArchiveError) -> CoremlInstallError {
    CoremlInstallError::new(archive_error_reason_code(&error), error.to_string(), 74)
}

fn write_sentinel(path: &Path, sentinel: &ParakeetCoremlSentinel) -> std::io::Result<()> {
    let parent = path.parent().expect("sentinel has a parent");
    fs::create_dir_all(parent)?;
    let temporary = parent.join(format!(
        ".install-complete.tmp.{}.{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let bytes = serde_json::to_vec(sentinel).expect("static CoreML sentinel serializes");
    let result = fs::write(&temporary, bytes).and_then(|_| fs::rename(&temporary, path));
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

/// Install the catalogued CoreML tree using the production origin policy.
pub fn install_parakeet_coreml_model(
    home_dir: &Path,
    config: &JournalConfigRead,
    force: bool,
) -> Result<PathBuf, CoremlInstallError> {
    let (os_name, arch) = canonical_host_pair(std::env::consts::OS, std::env::consts::ARCH);
    let rows = rows()?;
    install_with_rows(
        home_dir,
        config,
        force,
        (os_name, arch),
        &rows,
        &mut |_, _| {},
    )
}

/// Refresh an installed CoreML model tree if on darwin/arm64 and catalog pins moved.
pub fn refresh_installed_parakeet_coreml(
    journal: &Path,
    home_dir: &Path,
    config: &JournalConfigRead,
) -> Result<PathBuf, CoremlInstallError> {
    let (os_name, arch) = canonical_host_pair(std::env::consts::OS, std::env::consts::ARCH);
    if !(os_name == "darwin" && arch == "arm64") {
        return Ok(parakeet_coreml_model_root(&parakeet_coreml_cache_dir(
            config, home_dir,
        )));
    }
    let rows = rows()?;
    let total_bytes: u64 = rows.iter().map(|r| r.size_bytes).sum();
    let mut completed_bytes: u64 = 0;
    let mut progress_at = std::time::Instant::now();
    let mut progress_error: Option<String> = None;

    let mut fetch = |query: &RuntimeFetchQuery<'_>, destination: &Path| {
        if let Some(ref err) = progress_error {
            return Err(CoremlInstallError::new(
                "status_write_failed",
                err.clone(),
                74,
            ));
        }
        let fetch_res = fetch_runtime_member(query, destination, &mut |received, _| {
            if progress_error.is_some() {
                return;
            }
            let cumulative = completed_bytes.saturating_add(received);
            let state = match super::status::read_status(journal, "parakeet") {
                Ok(s) => s,
                Err(e) => {
                    progress_error = Some(e.to_string());
                    return;
                }
            };
            match super::status::bump_progress(
                state,
                Some(cumulative),
                Some(total_bytes),
                &mut progress_at,
            )
            .and_then(|next| {
                next.map(|next| super::status::write_status(journal, next))
                    .transpose()
            }) {
                Ok(Some(_)) => {}
                Ok(None) => {}
                Err(err) => progress_error = Some(err.to_string()),
            }
        });
        fetch_res?;
        if let Some(ref err) = progress_error {
            return Err(CoremlInstallError::new(
                "status_write_failed",
                err.clone(),
                74,
            ));
        }
        if let Some(row) = rows.iter().find(|r| Some(r.origin_key) == query.origin_key) {
            completed_bytes = completed_bytes.saturating_add(row.size_bytes);
        }
        Ok(())
    };
    refresh_with_rows(
        journal,
        home_dir,
        config,
        (os_name, arch),
        &rows,
        &mut fetch,
    )
}

/// Verify the model tree and install sentinel without fetching.
pub fn check_parakeet_coreml_install(
    home_dir: &Path,
    config: &JournalConfigRead,
) -> Result<(), CoremlInstallError> {
    let (os_name, arch) = canonical_host_pair(std::env::consts::OS, std::env::consts::ARCH);
    check_parakeet_coreml_install_with_platform(home_dir, config, os_name, arch)
}

/// Whether the CoreML model is installed at `cache_dir`: a valid sentinel for
/// that directory and every file it lists, at its recorded size. It does not
/// hash, so it is cheap enough to ask before each transcription. A model from
/// an earlier catalog still counts; following pins is the boot refresh's job.
/// Only darwin/arm64 runs CoreML, so that is the platform the sentinel must name.
pub fn parakeet_coreml_installed(home_dir: &Path, cache_dir: &Path) -> bool {
    parakeet_coreml_installed_on(home_dir, cache_dir, "darwin", "arm64")
}

fn parakeet_coreml_installed_on(
    home_dir: &Path,
    cache_dir: &Path,
    os_name: &str,
    arch: &str,
) -> bool {
    let Some(sentinel) = read_valid_parakeet_coreml_sentinel(home_dir, os_name, arch) else {
        return false;
    };
    if sentinel.cache_dir() != cache_dir {
        return false;
    }
    let model_root = parakeet_coreml_model_root(cache_dir);
    if sentinel.files().is_empty() {
        // An older sentinel lists no files; the boot refresh adopts it.
        return fs::read_dir(&model_root).is_ok_and(|mut entries| entries.next().is_some());
    }
    sentinel.files().iter().all(|file| {
        fs::metadata(model_root.join(&file.file))
            .is_ok_and(|meta| meta.is_file() && meta.len() == file.size_bytes)
    })
}

/// How long a failed self-heal waits before the next attempt may start, the
/// same persisted cooldown the linux provider follow honours.
pub const COREML_HEAL_COOLDOWN: Duration = Duration::from_secs(30 * 60);

/// What one self-heal pass found or did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoremlHeal {
    AlreadyInstalled,
    Installed,
    OwnerCancelled,
    CoolingDown,
    InstallRunning,
    Failed(String),
}

/// Install the CoreML model when it is missing, recording the attempt in the
/// journal's `parakeet` install status (`health/providers/parakeet.json`) under
/// the journal's `parakeet` install lease, as the linux installer does.
pub fn heal_missing_parakeet_coreml(
    journal: &Path,
    home_dir: &Path,
    config: &JournalConfigRead,
) -> Result<CoremlHeal, String> {
    let installed =
        parakeet_coreml_installed(home_dir, &parakeet_coreml_cache_dir(config, home_dir));
    let rows = rows().map_err(|error| error.to_string())?;
    let catalog_version = shared_catalog_version(&rows).map_err(|error| error.to_string())?;
    heal_with(
        journal,
        installed,
        catalog_version,
        chrono::Utc::now(),
        || download_publish_coreml(journal, home_dir, config, false),
    )
}

fn heal_with(
    journal: &Path,
    installed: bool,
    catalog_version: &str,
    now: chrono::DateTime<chrono::Utc>,
    install: impl FnOnce() -> Result<PathBuf, CoremlInstallError>,
) -> Result<CoremlHeal, String> {
    use super::{lease, status};

    if lease::is_held(journal, "parakeet").map_err(|error| error.to_string())? {
        return Ok(CoremlHeal::InstallRunning);
    }
    if installed {
        return Ok(CoremlHeal::AlreadyInstalled);
    }
    let record = status::read_status(journal, "parakeet").map_err(|error| error.to_string())?;
    if record.install_state == "failed" {
        if record.error_code.as_deref() == Some("install_cancelled") {
            return Ok(CoremlHeal::OwnerCancelled);
        }
        let cooling = record
            .last_transition_at
            .as_deref()
            .and_then(|stamp| chrono::DateTime::parse_from_rfc3339(stamp).ok())
            .and_then(|at| (now - at.with_timezone(&chrono::Utc)).to_std().ok())
            .is_some_and(|since| since < COREML_HEAL_COOLDOWN);
        if cooling {
            return Ok(CoremlHeal::CoolingDown);
        }
    }
    let Some(lease) = lease::acquire(journal, "parakeet").map_err(|error| error.to_string())?
    else {
        return Ok(CoremlHeal::InstallRunning);
    };
    let fingerprint_json = serde_json::json!({
        "catalog_version": catalog_version,
        "provider": "parakeet",
        "variant": "coreml",
    })
    .to_string();
    let fingerprint_sha256 = format!("{:x}", Sha256::digest(fingerprint_json.as_bytes()));
    let state = super::admit_parakeet_attempt(
        journal,
        &lease,
        fingerprint_json,
        fingerprint_sha256,
        "downloading",
        |_| {},
    )
    .map_err(|error| error.to_string())?;
    let result = install();
    let state = current_attempt_status(journal, &state).map_err(|error| error.to_string())?;
    let (next, outcome) = match result {
        Ok(_) => (
            status::transition(state, "installed", None, None),
            CoremlHeal::Installed,
        ),
        Err(error) => (
            status::transition(
                state,
                "failed",
                Some(error.to_string()),
                Some(error.reason_code.clone()),
            ),
            CoremlHeal::Failed(error.to_string()),
        ),
    };
    next.and_then(|next| status::write_status(journal, next))
        .map_err(|error| error.to_string())?;
    Ok(outcome)
}

fn write_attempt_phase(journal: &Path, phase: &str) -> Result<(), CoremlInstallError> {
    let mut current = super::status::read_status(journal, "parakeet")
        .map_err(|error| CoremlInstallError::new("status_unreadable", error.to_string(), 74))?;
    if phase == "verifying" {
        current.progress_bytes_received = current.progress_bytes_total;
    }
    super::status::transition(current, phase, None, None)
        .and_then(|next| super::status::write_status(journal, next))
        .map_err(|error| CoremlInstallError::new("status_write_failed", error.to_string(), 74))?;
    Ok(())
}

fn current_attempt_status(
    journal: &Path,
    admitted: &super::status::InstallStatus,
) -> Result<super::status::InstallStatus, CoremlInstallError> {
    let current = super::status::read_status(journal, "parakeet")
        .map_err(|error| CoremlInstallError::new("status_unreadable", error.to_string(), 74))?;
    if current.attempt_id != admitted.attempt_id {
        return Err(CoremlInstallError::new(
            "status_write_failed",
            "the speech install attempt changed.",
            74,
        ));
    }
    Ok(current)
}

pub fn run_coreml_parakeet_attempt(
    journal: &Path,
    lease: &super::lease::InstallLease,
    home_dir: &Path,
    config: &JournalConfigRead,
    force: bool,
    on_admitted: impl FnOnce(&super::status::InstallStatus),
) -> Result<PathBuf, CoremlInstallError> {
    use super::status;

    let rows = rows()?;
    let catalog_version = shared_catalog_version(&rows)?;
    let fingerprint_json = serde_json::json!({
        "catalog_version": catalog_version,
        "provider": "parakeet",
        "variant": "coreml",
    })
    .to_string();
    let fingerprint_sha256 = format!("{:x}", Sha256::digest(fingerprint_json.as_bytes()));
    let state = super::admit_parakeet_attempt(
        journal,
        lease,
        fingerprint_json,
        fingerprint_sha256,
        "downloading",
        on_admitted,
    )
    .map_err(|e| CoremlInstallError::new("status_write_failed", e.to_string(), 74))?;

    match download_publish_coreml(journal, home_dir, config, force) {
        Ok(target) => {
            let record = current_attempt_status(journal, &state)?;
            let next = status::transition(record, "installed", None, None)
                .map_err(|e| CoremlInstallError::new("status_write_failed", e.to_string(), 74))?;
            status::write_status(journal, next)
                .map_err(|e| CoremlInstallError::new("status_write_failed", e.to_string(), 74))?;
            Ok(target)
        }
        Err(error) => {
            let record = current_attempt_status(journal, &state)?;
            let write_res = status::transition(
                record,
                "failed",
                Some(error.to_string()),
                Some(error.reason_code.clone()),
            )
            .and_then(|failed| status::write_status(journal, failed));
            if let Err(term_err) = write_res {
                return Err(CoremlInstallError::new(
                    "status_write_failed",
                    term_err.to_string(),
                    74,
                ));
            }
            Err(error)
        }
    }
}

fn download_publish_coreml(
    journal: &Path,
    home_dir: &Path,
    config: &JournalConfigRead,
    force: bool,
) -> Result<PathBuf, CoremlInstallError> {
    let platform = canonical_host_pair(std::env::consts::OS, std::env::consts::ARCH);
    let rows = rows()?;
    let mut publish = |staging: &Path, target: &Path| fs::rename(staging, target);
    let mut write = write_sentinel;
    let mut progress_at = std::time::Instant::now();
    install_observed_with_rows(
        home_dir,
        config,
        force,
        platform,
        &rows,
        CoremlAdmission::Mint,
        &mut publish,
        &mut write,
        Some(journal),
        &mut |received, total| {
            let state = super::status::read_status(journal, "parakeet").map_err(|error| {
                CoremlInstallError::new("status_unreadable", error.to_string(), 74)
            })?;
            super::status::bump_progress(state, Some(received), total, &mut progress_at)
                .and_then(|next| {
                    next.map(|next| super::status::write_status(journal, next))
                        .transpose()
                })
                .map_err(|error| {
                    CoremlInstallError::new("status_write_failed", error.to_string(), 74)
                })?;
            Ok(())
        },
    )
}

fn fetch_runtime_member(
    query: &solstone_core_assets::RuntimeFetchQuery<'_>,
    destination: &Path,
    on_chunk: &mut impl FnMut(u64, Option<u64>),
) -> Result<(), CoremlInstallError> {
    let handle = solstone_core_assets::mint_runtime_fetch(query).map_err(|err| {
        CoremlInstallError::new(
            solstone_core_assets::FetchMintError::REASON_CODE,
            err.to_string(),
            65,
        )
    })?;
    solstone_core_artifact_download::download_runtime_fetch(&handle, destination, on_chunk)
        .map_err(archive_error)?;
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CoremlAdmission {
    Mint,
    #[cfg(feature = "test-hooks")]
    Fixture,
}

fn install_with_rows(
    home_dir: &Path,
    config: &JournalConfigRead,
    force: bool,
    platform: (&str, &str),
    rows: &[&Artifact],
    progress: &mut impl FnMut(u64, Option<u64>),
) -> Result<PathBuf, CoremlInstallError> {
    let mut publish = |staging: &Path, target: &Path| fs::rename(staging, target);
    let mut write = write_sentinel;
    install_with_rows_and_seams(
        home_dir,
        config,
        force,
        platform,
        rows,
        CoremlAdmission::Mint,
        &mut publish,
        &mut write,
        progress,
    )
}

/// Internal fixture seam: run the CoreML installer with injected publish and sentinel writers.
#[allow(clippy::too_many_arguments)]
pub(crate) fn install_with_rows_and_seams(
    home_dir: &Path,
    config: &JournalConfigRead,
    force: bool,
    platform: (&str, &str),
    rows: &[&Artifact],
    admission: CoremlAdmission,
    publish: &mut impl FnMut(&Path, &Path) -> std::io::Result<()>,
    write: &mut impl FnMut(&Path, &ParakeetCoremlSentinel) -> std::io::Result<()>,
    progress: &mut impl FnMut(u64, Option<u64>),
) -> Result<PathBuf, CoremlInstallError> {
    install_observed_with_rows(
        home_dir,
        config,
        force,
        platform,
        rows,
        admission,
        publish,
        write,
        None,
        &mut |received, total| {
            progress(received, total);
            Ok(())
        },
    )
}

#[allow(clippy::too_many_arguments)]
fn install_observed_with_rows(
    home_dir: &Path,
    config: &JournalConfigRead,
    force: bool,
    platform: (&str, &str),
    rows: &[&Artifact],
    admission: CoremlAdmission,
    publish: &mut impl FnMut(&Path, &Path) -> std::io::Result<()>,
    write: &mut impl FnMut(&Path, &ParakeetCoremlSentinel) -> std::io::Result<()>,
    journal: Option<&Path>,
    progress: &mut impl FnMut(u64, Option<u64>) -> Result<(), CoremlInstallError>,
) -> Result<PathBuf, CoremlInstallError> {
    let (os_name, arch) = platform;
    require_coreml_host(os_name, arch)?;
    let cache_dir = parakeet_coreml_cache_dir(config, home_dir);
    let target = parakeet_coreml_model_root(&cache_dir);

    let default_cache_dir = default_parakeet_coreml_cache_dir(home_dir);
    fs::create_dir_all(&default_cache_dir)
        .map_err(|e| CoremlInstallError::new("install_failed", e.to_string(), 74))?;
    let lock_path = default_cache_dir.join(".publish.lock");
    let _lock = hold_lock(
        &lock_path,
        LockOptions {
            timeout: DEFAULT_LOCK_TIMEOUT,
            poll_interval: DEFAULT_LOCK_POLL_INTERVAL,
            mode: None,
        },
    )
    .map_err(|e| match e {
        LockError::Timeout(_) => {
            CoremlInstallError::new("coreml_busy", "parakeet CoreML publication lock busy", 74)
        }
        other => CoremlInstallError::new(
            "install_failed",
            format!("failed to acquire publication lock: {other}"),
            74,
        ),
    })?;

    recover_parakeet_coreml_publication(home_dir)?;

    if !force && check_with_rows(home_dir, config, os_name, arch, rows).is_ok() {
        return Ok(target);
    }

    let catalog_version = shared_catalog_version(rows)?;
    let files = rows
        .iter()
        .map(|r| ParakeetCoremlFile {
            file: r.filename.to_string(),
            sha256: r.sha256.to_string(),
            size_bytes: r.size_bytes,
        })
        .collect::<Vec<_>>();

    if !force && files_match_rows(&target, rows) {
        if let Some(journal) = journal {
            write_attempt_phase(journal, "installing")?;
        }
        publish_coreml(
            home_dir,
            os_name,
            arch,
            &cache_dir,
            catalog_version,
            files,
            None,
            &target,
            publish,
            write,
        )?;
        return Ok(target);
    }

    let total = rows.iter().map(|row| row.size_bytes).sum();
    let mut completed = 0_u64;
    let mut fetch = |query: &RuntimeFetchQuery<'_>, destination: &Path| {
        let row = rows
            .iter()
            .find(|row| Some(row.origin_key) == query.origin_key)
            .expect("fetch query belongs to the selected rows");
        let mut progress_error = None;
        let mut on_chunk = |received, _| {
            if progress_error.is_none() {
                progress_error = progress(completed.saturating_add(received), Some(total)).err();
            }
        };
        let result = match admission {
            CoremlAdmission::Mint => fetch_runtime_member(query, destination, &mut on_chunk),
            #[cfg(feature = "test-hooks")]
            CoremlAdmission::Fixture => {
                let handle = solstone_core_assets::runtime_fetch_handle_fixture(
                    row.unit,
                    row.origin_key,
                    row.sha256,
                    row.size_bytes,
                );
                solstone_core_artifact_download::download_runtime_fetch(
                    &handle,
                    destination,
                    &mut on_chunk,
                )
                .map(|_| ())
                .map_err(archive_error)
            }
        };
        if let Some(error) = progress_error {
            return Err(error);
        }
        result?;
        completed = completed.saturating_add(row.size_bytes);
        Ok(())
    };

    download_and_commit_coreml_observed(
        home_dir, config, platform, rows, false, &mut fetch, publish, write, journal,
    )
}

fn check_parakeet_coreml_install_with_platform(
    home_dir: &Path,
    config: &JournalConfigRead,
    os_name: &str,
    arch: &str,
) -> Result<(), CoremlInstallError> {
    let rows = rows()?;
    check_with_rows(home_dir, config, os_name, arch, &rows)
}

// Setup polls must neither recover publication nor hash an unchanged model tree
// on every request. This memo stores one integrity proof, never job state.
#[cfg(unix)]
#[derive(Clone, PartialEq, Eq)]
struct CoremlTreeStamp {
    entries: Vec<CoremlFileStamp>,
    rows: Vec<(String, String, u64)>,
}

#[cfg(unix)]
#[derive(Clone, PartialEq, Eq)]
struct CoremlFileStamp {
    path: PathBuf,
    device: u64,
    inode: u64,
    len: u64,
    modified: (i64, i64),
    changed: (i64, i64),
}

#[cfg(unix)]
fn coreml_tree_stamp(
    home: &Path,
    config: &JournalConfigRead,
    rows: &[&Artifact],
) -> Option<CoremlTreeStamp> {
    use std::os::unix::fs::MetadataExt;
    let configured_cache = parakeet_coreml_cache_dir(config, home);
    let root = parakeet_coreml_model_root(&configured_cache);
    let mut paths = vec![
        parakeet_coreml_sentinel_path(home),
        configured_cache,
        root.clone(),
    ];
    paths.extend(rows.iter().map(|row| root.join(row.filename)));
    let entries = paths
        .into_iter()
        .map(|path| {
            let meta = fs::metadata(&path).ok()?;
            Some(CoremlFileStamp {
                path,
                device: meta.dev(),
                inode: meta.ino(),
                len: meta.len(),
                modified: (meta.mtime(), meta.mtime_nsec()),
                changed: (meta.ctime(), meta.ctime_nsec()),
            })
        })
        .collect::<Option<Vec<_>>>()?;
    Some(CoremlTreeStamp {
        entries,
        rows: rows
            .iter()
            .map(|row| (row.filename.into(), row.sha256.into(), row.size_bytes))
            .collect(),
    })
}

/// Read-only integrity verification for setup progress. Reuse a digest proof
/// only while the complete tree, receipt and offered catalog identities match.
#[cfg(unix)]
pub fn parakeet_coreml_current_verified(home: &Path, config: &JournalConfigRead) -> bool {
    static PROOF: std::sync::Mutex<Option<(CoremlTreeStamp, bool)>> = std::sync::Mutex::new(None);
    let Ok(rows) = rows() else {
        return false;
    };
    current_verified_with_rows(home, config, &rows, &PROOF, || {
        check_with_rows(home, config, "darwin", "arm64", &rows).is_ok()
    })
}

#[cfg(unix)]
fn current_verified_with_rows(
    home: &Path,
    config: &JournalConfigRead,
    rows: &[&Artifact],
    proof: &std::sync::Mutex<Option<(CoremlTreeStamp, bool)>>,
    verify: impl FnOnce() -> bool,
) -> bool {
    let cache = default_parakeet_coreml_cache_dir(home);
    let lock_path = cache.join(".admission.lock");
    let Ok(guard) = solstone_core_journal_io::hold_existing_lock(
        &lock_path,
        LockOptions {
            timeout: Duration::ZERO,
            ..LockOptions::default()
        },
    ) else {
        return false;
    };
    let marker = cache.join(".publication.json");
    if marker.exists()
        && !fs::read(&marker)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<PublicationMarker>(&bytes).ok())
            .is_some_and(|marker| publication_was_committed(home, &marker))
    {
        return false;
    }
    let Some(before) = coreml_tree_stamp(home, config, rows) else {
        return false;
    };
    let Ok(mut cached) = proof.lock() else {
        return false;
    };
    let verified = match cached.as_ref() {
        Some((stamp, verified)) if stamp == &before => *verified,
        _ => verify(),
    };
    // Legacy installations can predate the admission lock. Refuse a snapshot
    // if a writer established its authority while the unlocked read ran.
    if guard.is_none()
        && solstone_core_journal_io::hold_existing_lock(
            &lock_path,
            LockOptions {
                timeout: Duration::ZERO,
                ..LockOptions::default()
            },
        )
        .map_or(true, |guard| guard.is_some())
    {
        return false;
    }
    if coreml_tree_stamp(home, config, rows).as_ref() != Some(&before) {
        return false;
    }
    *cached = Some((before, verified));
    verified
}

fn check_with_rows(
    home_dir: &Path,
    config: &JournalConfigRead,
    os_name: &str,
    arch: &str,
    rows: &[&Artifact],
) -> Result<(), CoremlInstallError> {
    require_coreml_host(os_name, arch)?;
    let sentinel =
        read_valid_parakeet_coreml_sentinel(home_dir, os_name, arch).ok_or_else(|| {
            CoremlInstallError::new(
                "sentinel_not_ready",
                format!(
                    "parakeet CoreML sentinel not ready: {}; install it with: solstone journal install-models",
                    parakeet_coreml_sentinel_path(home_dir).display(),
                ),
                65,
            )
        })?;
    let expected_cache_dir = parakeet_coreml_cache_dir(config, home_dir);
    if sentinel.cache_dir() != expected_cache_dir {
        return Err(CoremlInstallError::new(
            "cache_dir_mismatch",
            format!(
                "parakeet CoreML sentinel cache dir ({}) does not match configured ({})",
                sentinel.cache_dir().display(),
                expected_cache_dir.display(),
            ),
            65,
        ));
    }
    check_model_tree_with_rows(sentinel.cache_dir(), rows)?;
    let expected_catalog_version = shared_catalog_version(rows)?;
    if sentinel.catalog_version() != expected_catalog_version {
        return Err(CoremlInstallError::new(
            "catalog_version_mismatch",
            format!(
                "parakeet CoreML catalog version mismatch: found {}, expected {}",
                sentinel.catalog_version(),
                expected_catalog_version,
            ),
            65,
        ));
    }
    Ok(())
}

fn check_model_tree_with_rows(
    cache_dir: &Path,
    rows: &[&Artifact],
) -> Result<(), CoremlInstallError> {
    let model_root = parakeet_coreml_model_root(cache_dir);
    for row in rows {
        let path = model_root.join(row.filename);
        let meta = match fs::metadata(&path) {
            Ok(meta) if meta.is_file() => meta,
            _ => {
                return Err(CoremlInstallError::new(
                    "model_incomplete",
                    format!(
                        "parakeet CoreML asset missing: {}; install it with: solstone journal install-models",
                        row.filename
                    ),
                    65,
                ));
            }
        };
        if meta.len() != row.size_bytes {
            return Err(CoremlInstallError::new(
                "model_incomplete",
                format!(
                    "parakeet CoreML asset size mismatch for {}: expected {}, found {}",
                    row.filename,
                    row.size_bytes,
                    meta.len()
                ),
                65,
            ));
        }
        let actual_sha = sha256_file(&path).map_err(|error| {
            CoremlInstallError::new(
                "model_incomplete",
                format!("failed to read {}: {error}", row.filename),
                65,
            )
        })?;
        if actual_sha != row.sha256 {
            return Err(CoremlInstallError::new(
                "model_incomplete",
                format!(
                    "parakeet CoreML asset digest mismatch for {}: expected {}, found {}",
                    row.filename, row.sha256, actual_sha
                ),
                65,
            ));
        }
    }
    Ok(())
}

/// Admit the installed tree independently of the currently offered catalog pin.
pub fn check_parakeet_coreml_cache_doctor(
    home_dir: &Path,
    config: &JournalConfigRead,
    os_name: &str,
    arch: &str,
) -> Result<PathBuf, String> {
    require_coreml_host(os_name, arch)
        .map_err(|error| format!("parakeet check failed: {error}"))?;
    let _admission =
        hold_parakeet_coreml_read_admission(home_dir).map_err(|error| error.to_string())?;
    let sentinel =
        read_valid_parakeet_coreml_sentinel(home_dir, os_name, arch).ok_or_else(|| {
            format!(
                "parakeet check failed: sentinel not ready at {}",
                default_parakeet_coreml_cache_dir(home_dir)
                    .join(".install-complete")
                    .display()
            )
        })?;

    let configured = parakeet_coreml_cache_dir(config, home_dir);
    if sentinel.cache_dir() != configured {
        return Err(format!(
            "parakeet check failed: installed cache {} does not match configured {}",
            sentinel.cache_dir().display(),
            configured.display()
        ));
    }

    let model_root = parakeet_coreml_model_root(sentinel.cache_dir());
    if !sentinel.files().is_empty() {
        for file in sentinel.files() {
            let path = model_root.join(&file.file);
            let meta = fs::metadata(&path).map_err(|error| {
                format!(
                    "parakeet check failed: asset missing: {}: {error}",
                    file.file
                )
            })?;
            if !meta.is_file() || meta.len() != file.size_bytes {
                return Err(format!(
                    "parakeet check failed: asset size mismatch for {}: expected {}, found {}",
                    file.file,
                    file.size_bytes,
                    meta.len()
                ));
            }
            let actual_sha = sha256_file(&path).map_err(|error| {
                format!(
                    "parakeet check failed: failed to read {}: {error}",
                    file.file
                )
            })?;
            if actual_sha != file.sha256 {
                return Err(format!(
                    "parakeet check failed: asset digest mismatch for {}: expected {}, found {}",
                    file.file, file.sha256, actual_sha
                ));
            }
        }
    } else {
        // Published legacy receipts have no installed digests. Preserve their
        // structural admission until the installer can adopt or refresh them;
        // today's offered digests describe freshness, not those installed bytes.
        let rows = rows().map_err(|error| error.to_string())?;
        for row in rows {
            if !model_root.join(row.filename).is_file() {
                return Err(format!(
                    "parakeet check failed: asset missing: {}",
                    row.filename
                ));
            }
        }
    }

    Ok(sentinel.cache_dir().to_path_buf())
}

fn publication_failure(
    home: &Path,
    reason: &str,
    error: impl std::fmt::Display,
) -> CoremlInstallError {
    let message = match recover_publication_locked(home) {
        Ok(()) => error.to_string(),
        Err(restore) => format!("{error}; restore failed: {restore}"),
    };
    CoremlInstallError::new(reason, message, 74)
}

fn files_match_rows(model_root: &Path, rows: &[&Artifact]) -> bool {
    for row in rows {
        let path = model_root.join(row.filename);
        let Ok(meta) = fs::metadata(&path) else {
            return false;
        };
        if !meta.is_file() || meta.len() != row.size_bytes {
            return false;
        }
        let Ok(actual_sha) = sha256_file(&path) else {
            return false;
        };
        if actual_sha != row.sha256 {
            return false;
        }
    }
    true
}

#[allow(clippy::too_many_arguments)]
fn publish_coreml(
    home_dir: &Path,
    os_name: &str,
    arch: &str,
    cache_dir: &Path,
    catalog_version: &str,
    files: Vec<ParakeetCoremlFile>,
    staging: Option<&Path>,
    target: &Path,
    publish: &mut impl FnMut(&Path, &Path) -> std::io::Result<()>,
    write: &mut impl FnMut(&Path, &ParakeetCoremlSentinel) -> std::io::Result<()>,
) -> Result<(), CoremlInstallError> {
    // Downloads use the worker lease. Only this short tree/receipt switch
    // excludes readers, so readiness never waits behind a model download.
    let _admission = hold_coreml_admission(home_dir)?;
    fs::create_dir_all(cache_dir)
        .map_err(|e| CoremlInstallError::new("install_failed", e.to_string(), 74))?;

    let sentinel_path = parakeet_coreml_sentinel_path(home_dir);
    let sentinel_parent = sentinel_path.parent().expect("sentinel has parent");
    fs::create_dir_all(sentinel_parent)
        .map_err(|error| CoremlInstallError::new("install_failed", error.to_string(), 74))?;
    let temp_sentinel = sentinel_parent.join(format!(
        ".install-complete.tmp.{}.{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));

    let new_record = ParakeetCoremlSentinel::new(
        cache_dir.to_path_buf(),
        os_name,
        arch,
        FLUIDAUDIO_VERSION,
        catalog_version,
        files,
    );

    if let Err(error) = write(&temp_sentinel, &new_record) {
        if let Some(staging_dir) = staging {
            let _ = fs::remove_dir_all(staging_dir);
        }
        let _ = fs::remove_file(&temp_sentinel);
        return Err(CoremlInstallError::new(
            "sentinel_write_failed",
            error.to_string(),
            74,
        ));
    }

    fs::create_dir_all(cache_dir)
        .map_err(|e| CoremlInstallError::new("install_failed", e.to_string(), 74))?;

    let default_cache = default_parakeet_coreml_cache_dir(home_dir);
    let marker_path = default_cache.join(".publication.json");

    let mut had_target = false;
    let mut tree_aside = PathBuf::new();

    if let Some(staging_dir) = staging {
        let target_parent = target.parent().expect("target has parent");
        fs::create_dir_all(target_parent)
            .map_err(|e| CoremlInstallError::new("install_failed", e.to_string(), 74))?;
        tree_aside = target_parent.join(format!(
            ".parakeet-tdt-0.6b-v3.previous.{}.{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        had_target = target.exists();

        let pending_marker = PublicationMarker {
            phase: "pending".to_string(),
            tree_canonical: Some(target.to_path_buf()),
            tree_backup: if had_target {
                Some(tree_aside.clone())
            } else {
                None
            },
            sentinel_backup: None,
            staging: Some(staging_dir.to_path_buf()),
            published_catalog_version: Some(catalog_version.to_string()),
            published_files: Some(new_record.files().to_vec()),
        };
        if let Err(error) = write_publication_marker(&marker_path, &pending_marker) {
            let _ = fs::remove_dir_all(staging_dir);
            let _ = fs::remove_file(&temp_sentinel);
            return Err(CoremlInstallError::new(
                "publish_failed",
                error.to_string(),
                74,
            ));
        }

        if had_target && let Err(error) = fs::rename(target, &tree_aside) {
            let _ = fs::remove_file(&marker_path);
            let _ = fs::remove_dir_all(staging_dir);
            let _ = fs::remove_file(&temp_sentinel);
            return Err(CoremlInstallError::new(
                "publish_failed",
                error.to_string(),
                74,
            ));
        }

        if let Err(error) = publish(staging_dir, target) {
            let _ = fs::remove_file(&temp_sentinel);
            return Err(publication_failure(home_dir, "publish_failed", error));
        }
    }

    let sentinel_bytes = match fs::read(&temp_sentinel) {
        Ok(bytes) => bytes,
        Err(error) => {
            let _ = fs::remove_file(&temp_sentinel);
            return Err(publication_failure(
                home_dir,
                "sentinel_write_failed",
                error,
            ));
        }
    };

    if let Err(error) = atomic_replace(
        &sentinel_path,
        &sentinel_bytes,
        AtomicWriteOptions::default(),
    ) {
        let _ = fs::remove_file(&temp_sentinel);
        return Err(publication_failure(
            home_dir,
            "sentinel_write_failed",
            error,
        ));
    }
    let _ = fs::remove_file(&temp_sentinel);

    if staging.is_some() {
        let committed_marker = PublicationMarker {
            phase: "committed".to_string(),
            tree_canonical: None,
            tree_backup: if had_target {
                Some(tree_aside.clone())
            } else {
                None
            },
            sentinel_backup: None,
            staging: None,
            published_catalog_version: None,
            published_files: None,
        };
        let _ = write_publication_marker(&marker_path, &committed_marker);
        if had_target
            && tree_aside.exists()
            && let Err(e) = fs::remove_dir_all(&tree_aside)
        {
            return Err(CoremlInstallError::new("publish_failed", e.to_string(), 74));
        }
        let _ = fs::remove_file(&marker_path);
    }

    Ok(())
}

fn refresh_with_rows(
    journal: &Path,
    home_dir: &Path,
    config: &JournalConfigRead,
    platform: (&str, &str),
    rows: &[&Artifact],
    fetch: &mut impl FnMut(&RuntimeFetchQuery<'_>, &Path) -> Result<(), CoremlInstallError>,
) -> Result<PathBuf, CoremlInstallError> {
    let (os_name, arch) = platform;
    require_coreml_host(os_name, arch)?;
    let cache_dir = parakeet_coreml_cache_dir(config, home_dir);
    let target = parakeet_coreml_model_root(&cache_dir);

    let default_cache = default_parakeet_coreml_cache_dir(home_dir);
    let marker_path = default_cache.join(".publication.json");
    let sentinel_path = parakeet_coreml_sentinel_path(home_dir);
    if !marker_path.exists() && !sentinel_path.exists() {
        return Ok(target);
    }

    let lock_path = default_cache.join(".publish.lock");
    let _publish_lock = match hold_lock(
        &lock_path,
        LockOptions {
            timeout: Duration::ZERO,
            poll_interval: DEFAULT_LOCK_POLL_INTERVAL,
            mode: None,
        },
    ) {
        Ok(guard) => guard,
        Err(LockError::Timeout(_)) => {
            log::warn!("parakeet CoreML refresh skipped: publication lock held");
            return Ok(target);
        }
        Err(other) => {
            log::warn!("parakeet CoreML refresh skipped: lock failed: {other}");
            return Ok(target);
        }
    };

    recover_parakeet_coreml_publication(home_dir)?;

    let Some(sentinel) = read_valid_parakeet_coreml_sentinel(home_dir, os_name, arch) else {
        return Ok(target);
    };
    if sentinel.cache_dir() != cache_dir {
        return Ok(target);
    }

    if check_with_rows(home_dir, config, os_name, arch, rows).is_ok() {
        return Ok(target);
    }

    let catalog_version = shared_catalog_version(rows)?;
    let files = rows
        .iter()
        .map(|r| ParakeetCoremlFile {
            file: r.filename.to_string(),
            sha256: r.sha256.to_string(),
            size_bytes: r.size_bytes,
        })
        .collect::<Vec<_>>();

    let mut publish = |staging: &Path, target: &Path| fs::rename(staging, target);
    let mut write = write_sentinel;

    if files_match_rows(&target, rows) {
        publish_coreml(
            home_dir,
            os_name,
            arch,
            &cache_dir,
            catalog_version,
            files,
            None,
            &target,
            &mut publish,
            &mut write,
        )?;
        return Ok(target);
    }

    let lease = match super::lease::acquire(journal, "parakeet") {
        Ok(Some(lease)) => lease,
        Ok(None) => {
            log::warn!("parakeet CoreML refresh skipped: install lease held");
            return Ok(target);
        }
        Err(err) => {
            log::warn!("parakeet CoreML refresh skipped: lease acquire error: {err}");
            return Ok(target);
        }
    };

    let fingerprint_json = serde_json::json!({
        "catalog_version": catalog_version,
        "provider": "parakeet",
        "variant": "coreml",
    })
    .to_string();
    let fingerprint_sha256 = format!("{:x}", Sha256::digest(fingerprint_json.as_bytes()));
    let state = super::admit_parakeet_attempt(
        journal,
        &lease,
        fingerprint_json,
        fingerprint_sha256,
        "downloading",
        |_| {},
    )
    .map_err(|e| CoremlInstallError::new("status_write_failed", e.to_string(), 74))?;

    let outcome = download_and_commit_coreml_observed(
        home_dir,
        config,
        platform,
        rows,
        true,
        fetch,
        &mut publish,
        &mut write,
        Some(journal),
    );
    match outcome {
        Ok(target) => {
            let record = current_attempt_status(journal, &state)?;
            let next = super::status::transition(record, "installed", None, None)
                .map_err(|e| CoremlInstallError::new("status_write_failed", e.to_string(), 74))?;
            super::status::write_status(journal, next)
                .map_err(|e| CoremlInstallError::new("status_write_failed", e.to_string(), 74))?;
            Ok(target)
        }
        Err(error) => {
            let record = current_attempt_status(journal, &state)?;
            let write_res = super::status::transition(
                record,
                "failed",
                Some(error.to_string()),
                Some(error.reason_code.clone()),
            )
            .and_then(|failed| super::status::write_status(journal, failed));
            if let Err(term_err) = write_res {
                return Err(CoremlInstallError::new(
                    "status_write_failed",
                    term_err.to_string(),
                    74,
                ));
            }
            Err(error)
        }
    }
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
fn download_and_commit_coreml(
    home_dir: &Path,
    config: &JournalConfigRead,
    platform: (&str, &str),
    rows: &[&Artifact],
    is_refresh: bool,
    fetch: &mut impl FnMut(&RuntimeFetchQuery<'_>, &Path) -> Result<(), CoremlInstallError>,
    publish: &mut impl FnMut(&Path, &Path) -> std::io::Result<()>,
    write: &mut impl FnMut(&Path, &ParakeetCoremlSentinel) -> std::io::Result<()>,
) -> Result<PathBuf, CoremlInstallError> {
    download_and_commit_coreml_observed(
        home_dir, config, platform, rows, is_refresh, fetch, publish, write, None,
    )
}

#[allow(clippy::too_many_arguments)]
fn download_and_commit_coreml_observed(
    home_dir: &Path,
    config: &JournalConfigRead,
    platform: (&str, &str),
    rows: &[&Artifact],
    is_refresh: bool,
    fetch: &mut impl FnMut(&RuntimeFetchQuery<'_>, &Path) -> Result<(), CoremlInstallError>,
    publish: &mut impl FnMut(&Path, &Path) -> std::io::Result<()>,
    write: &mut impl FnMut(&Path, &ParakeetCoremlSentinel) -> std::io::Result<()>,
    journal: Option<&Path>,
) -> Result<PathBuf, CoremlInstallError> {
    let (os_name, arch) = platform;
    let cache_dir = parakeet_coreml_cache_dir(config, home_dir);
    let target = parakeet_coreml_model_root(&cache_dir);
    let catalog_version = shared_catalog_version(rows)?;

    let parent = target.parent().expect("CoreML model root has a parent");
    fs::create_dir_all(parent)
        .map_err(|error| CoremlInstallError::new("install_failed", error.to_string(), 74))?;
    let staging = parent.join(format!(
        ".parakeet-tdt-0.6b-v3.stage.{}",
        uuid::Uuid::new_v4().simple()
    ));

    fs::create_dir(&staging)
        .map_err(|error| CoremlInstallError::new("install_failed", error.to_string(), 74))?;

    let download_result = (|| {
        for row in rows {
            let query = RuntimeFetchQuery {
                unit: Some(row.unit),
                origin_key: Some(row.origin_key),
                ..Default::default()
            };
            let destination = cache_dir.join(".fetch").join(row.sha256).join(row.filename);
            if let Some(dest_parent) = destination.parent() {
                fs::create_dir_all(dest_parent).map_err(|error| {
                    CoremlInstallError::new("install_failed", error.to_string(), 74)
                })?;
            }
            fetch(&query, &destination)?;
            let staged = staging.join(row.filename);
            if let Some(parent) = staged.parent() {
                fs::create_dir_all(parent)
                    .map_err(|e| CoremlInstallError::new("install_failed", e.to_string(), 74))?;
            }
            // A retained transfer survives later-member/publication failures.
            // Hard links avoid a second allocation on supported owner volumes.
            if fs::hard_link(&destination, &staged).is_err() {
                fs::copy(&destination, &staged)
                    .map_err(|e| CoremlInstallError::new("install_failed", e.to_string(), 74))?;
            }
        }
        Ok(())
    })();

    if let Err(error) = download_result {
        let _ = fs::remove_dir_all(&staging);
        return Err(error);
    }

    if is_refresh && read_valid_parakeet_coreml_sentinel(home_dir, os_name, arch).is_none() {
        let _ = fs::remove_dir_all(&staging);
        return Err(CoremlInstallError::new(
            "sentinel_removed",
            "incumbent sentinel was removed before publication",
            74,
        ));
    }

    let preparation = (|| {
        if let Some(journal) = journal {
            write_attempt_phase(journal, "verifying")?;
        }
        // Fetch handles prove digests; validate the complete staged tree too.
        for row in rows {
            let path = staging.join(row.filename);
            let metadata = fs::metadata(&path).map_err(|error| {
                CoremlInstallError::new("model_incomplete", error.to_string(), 65)
            })?;
            if !metadata.is_file() || metadata.len() != row.size_bytes {
                return Err(CoremlInstallError::new(
                    "model_incomplete",
                    "speech model asset is incomplete",
                    65,
                ));
            }
        }
        if let Some(journal) = journal {
            write_attempt_phase(journal, "installing")?;
        }
        Ok(())
    })();
    if let Err(error) = preparation {
        let _ = fs::remove_dir_all(&staging);
        return Err(error);
    }

    let files = rows
        .iter()
        .map(|r| ParakeetCoremlFile {
            file: r.filename.to_string(),
            sha256: r.sha256.to_string(),
            size_bytes: r.size_bytes,
        })
        .collect::<Vec<_>>();

    publish_coreml(
        home_dir,
        os_name,
        arch,
        &cache_dir,
        catalog_version,
        files,
        Some(&staging),
        &target,
        publish,
        write,
    )?;

    for row in rows {
        let _ = fs::remove_file(cache_dir.join(".fetch").join(row.sha256).join(row.filename));
    }
    Ok(target)
}

#[cfg(feature = "test-hooks")]
pub(crate) fn install_with_rows_for_test(
    home_dir: &Path,
    config: &JournalConfigRead,
    force: bool,
    rows: &[&Artifact],
) -> Result<PathBuf, CoremlInstallError> {
    let mut publish = |staging: &Path, target: &Path| fs::rename(staging, target);
    let mut write = write_sentinel;
    install_with_rows_and_seams(
        home_dir,
        config,
        force,
        ("darwin", "arm64"),
        rows,
        CoremlAdmission::Fixture,
        &mut publish,
        &mut write,
        &mut |_, _| {},
    )
}

#[cfg(any(test, feature = "test-hooks"))]
pub(crate) fn install_minted_rows_for_test(
    home_dir: &Path,
    config: &JournalConfigRead,
    force: bool,
    rows: &[&Artifact],
) -> Result<PathBuf, CoremlInstallError> {
    let mut publish = |staging: &Path, target: &Path| fs::rename(staging, target);
    let mut write = write_sentinel;
    install_with_rows_and_seams(
        home_dir,
        config,
        force,
        ("darwin", "arm64"),
        rows,
        CoremlAdmission::Mint,
        &mut publish,
        &mut write,
        &mut |_, _| {},
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coreml_catalog_rows_match_the_filtered_catalog_set() {
        let rows = rows().unwrap();
        let expected = catalog()
            .iter()
            .filter(|artifact| {
                artifact.unit == UNIT && artifact.platform == Some(Platform::MacosArm64)
            })
            .map(|artifact| artifact.filename)
            .collect::<std::collections::BTreeSet<_>>();
        let actual = rows
            .iter()
            .map(|artifact| artifact.filename)
            .collect::<std::collections::BTreeSet<_>>();
        assert!(!expected.is_empty());
        assert_eq!(actual, expected);
    }

    #[test]
    fn coreml_archive_error_reason_codes_cover_every_archive_error() {
        use archive::ArchiveError;
        assert_eq!(
            archive_error_reason_code(&ArchiveError::HostRefused {
                host: "blocked.test".to_owned(),
            }),
            "download_host_refused"
        );
        assert_eq!(
            archive_error_reason_code(&ArchiveError::InsecureScheme {
                scheme: "http".to_owned(),
                host: "example.test".to_owned(),
            }),
            "download_insecure_scheme"
        );
        assert_eq!(
            archive_error_reason_code(&ArchiveError::UrlUserinfoRefused {
                authority: "user@host".to_owned(),
            }),
            "download_url_userinfo_refused"
        );
        assert_eq!(
            archive_error_reason_code(&ArchiveError::SizeMismatch {
                expected: 1,
                actual: 2,
            }),
            "download_size_mismatch"
        );
        assert_eq!(
            archive_error_reason_code(&ArchiveError::DigestMismatch {
                expected: "a".repeat(64),
                actual: "b".repeat(64),
            }),
            "download_digest_mismatch"
        );
        assert_eq!(
            archive_error_reason_code(&ArchiveError::RedirectHopLimitExceeded { limit: 11 }),
            "download_redirect_hop_limit"
        );
        assert_eq!(
            archive_error_reason_code(&ArchiveError::OriginUnavailable {
                host: "updates.invalid".to_owned(),
                message: "timeout".to_owned(),
            }),
            "download_origin_unreachable"
        );
        assert_eq!(
            archive_error_reason_code(&ArchiveError::Io(std::io::Error::other("failure"))),
            "download_failed"
        );
        assert_eq!(
            archive_error_reason_code(&ArchiveError::Download("network".to_owned())),
            "download_failed"
        );
        assert_eq!(
            archive_error_reason_code(&ArchiveError::PathEscape("..".to_owned())),
            "download_failed"
        );
    }

    fn config(cache_dir: &Path) -> JournalConfigRead {
        let config = serde_json::json!({"transcribe": {"parakeet": {"cache_dir": cache_dir}}})
            .as_object()
            .unwrap()
            .clone();
        JournalConfigRead {
            present: true,
            sha256: None,
            config: Some(config),
        }
    }

    fn row(filename: &'static str, bytes: &[u8]) -> Artifact {
        Artifact {
            unit: UNIT,
            version: "test",
            filename,
            sha256: Box::leak(format!("{:x}", Sha256::digest(bytes)).into_boxed_str()),
            size_bytes: bytes.len() as u64,
            upstream_url: "https://upstream.invalid/test",
            origin_key: Box::leak(format!("test/{filename}").into_boxed_str()),
            artifact_key: None,
            platform: Some(Platform::MacosArm64),
            backend: None,
            extracted_binary_sha256: None,
        }
    }

    fn stage_installed(home: &Path, cache: &Path, bytes: &[u8]) {
        fs::create_dir_all(cache).unwrap();
        let target = parakeet_coreml_model_root(cache);
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("model.bin"), bytes).unwrap();
        let sentinel = ParakeetCoremlSentinel::new(
            cache.to_path_buf(),
            "darwin",
            "arm64",
            FLUIDAUDIO_VERSION,
            "test",
            vec![ParakeetCoremlFile {
                file: "model.bin".to_owned(),
                sha256: format!("{:x}", Sha256::digest(bytes)),
                size_bytes: bytes.len() as u64,
            }],
        );
        write_sentinel(&parakeet_coreml_sentinel_path(home), &sentinel).unwrap();
    }

    #[test]
    fn installed_means_a_sentinel_for_this_cache_and_its_files_at_their_sizes() {
        let temporary = tempfile::tempdir().unwrap();
        let home = temporary.path().join("home");
        let cache = temporary.path().join("cache");
        let other = temporary.path().join("other");
        fs::create_dir_all(&other).unwrap();
        assert!(!parakeet_coreml_installed_on(
            &home, &cache, "darwin", "arm64"
        ));

        stage_installed(&home, &cache, b"model-bytes");
        assert!(parakeet_coreml_installed_on(
            &home, &cache, "darwin", "arm64"
        ));
        assert!(!parakeet_coreml_installed_on(
            &home, &other, "darwin", "arm64"
        ));

        fs::write(
            parakeet_coreml_model_root(&cache).join("model.bin"),
            b"short",
        )
        .unwrap();
        assert!(!parakeet_coreml_installed_on(
            &home, &cache, "darwin", "arm64"
        ));
        fs::remove_file(parakeet_coreml_model_root(&cache).join("model.bin")).unwrap();
        assert!(!parakeet_coreml_installed_on(
            &home, &cache, "darwin", "arm64"
        ));
    }

    fn parakeet_status(journal: &Path) -> super::super::status::InstallStatus {
        super::super::status::read_status(journal, "parakeet").unwrap()
    }

    #[test]
    fn heal_installs_a_missing_model_and_records_it_in_the_journal() {
        let journal = tempfile::tempdir().unwrap();
        let mut calls = 0;
        let outcome = heal_with(journal.path(), false, "v1", chrono::Utc::now(), || {
            calls += 1;
            Ok(PathBuf::from("/model"))
        })
        .unwrap();
        assert_eq!(outcome, CoremlHeal::Installed);
        assert_eq!(calls, 1);
        let record = parakeet_status(journal.path());
        assert_eq!(record.install_state, "installed");
        assert!(
            record
                .target_fingerprint_json
                .as_deref()
                .is_some_and(|json| json.contains("\"coreml\""))
        );
    }

    #[test]
    fn a_failed_heal_is_recorded_and_waits_out_the_cooldown() {
        let journal = tempfile::tempdir().unwrap();
        let outcome = heal_with(journal.path(), false, "v1", chrono::Utc::now(), || {
            Err(CoremlInstallError::new(
                "download_failed",
                "network down",
                65,
            ))
        })
        .unwrap();
        assert!(matches!(outcome, CoremlHeal::Failed(_)));
        let record = parakeet_status(journal.path());
        assert_eq!(record.install_state, "failed");
        assert_eq!(record.error_code.as_deref(), Some("download_failed"));

        let soon = chrono::Utc::now() + chrono::Duration::minutes(1);
        let outcome = heal_with(journal.path(), false, "v1", soon, || {
            panic!("no attempt inside the cooldown")
        })
        .unwrap();
        assert_eq!(outcome, CoremlHeal::CoolingDown);

        let later = chrono::Utc::now() + chrono::Duration::minutes(31);
        let outcome = heal_with(journal.path(), false, "v1", later, || {
            Ok(PathBuf::from("/model"))
        })
        .unwrap();
        assert_eq!(outcome, CoremlHeal::Installed);
    }

    #[test]
    fn heal_holds_for_an_owner_cancel_and_a_running_install() {
        use super::super::{lease, status};
        let journal = tempfile::tempdir().unwrap();
        let begun = status::begin_or_replace(
            journal.path(),
            "parakeet",
            "{}".to_owned(),
            "0".repeat(64),
            None,
            "downloading",
            None,
        )
        .unwrap();
        let cancelled = status::transition(
            begun,
            "failed",
            Some("cancelled".to_owned()),
            Some("install_cancelled".to_owned()),
        )
        .unwrap();
        status::write_status(journal.path(), cancelled).unwrap();
        let later = chrono::Utc::now() + chrono::Duration::hours(2);
        assert_eq!(
            heal_with(journal.path(), false, "v1", later, || panic!(
                "owner cancelled"
            ))
            .unwrap(),
            CoremlHeal::OwnerCancelled
        );

        let other = tempfile::tempdir().unwrap();
        let _held = lease::acquire(other.path(), "parakeet").unwrap().unwrap();
        assert_eq!(
            heal_with(other.path(), false, "v1", chrono::Utc::now(), || {
                panic!("an install is already running")
            })
            .unwrap(),
            CoremlHeal::InstallRunning
        );
    }

    #[test]
    fn a_model_installed_another_way_clears_a_recorded_failure() {
        let journal = tempfile::tempdir().unwrap();
        heal_with(journal.path(), false, "v1", chrono::Utc::now(), || {
            Err(CoremlInstallError::new(
                "download_failed",
                "network down",
                65,
            ))
        })
        .unwrap();
        let outcome = heal_with(journal.path(), true, "v1", chrono::Utc::now(), || {
            panic!("already installed")
        })
        .unwrap();
        assert_eq!(outcome, CoremlHeal::AlreadyInstalled);
        let record = parakeet_status(journal.path());
        assert_eq!(record.install_state, "failed");
        assert_eq!(record.error_code.as_deref(), Some("download_failed"));
    }

    #[test]
    fn heal_with_lease_held_and_installed_true_returns_install_running() {
        let journal = tempfile::tempdir().unwrap();
        heal_with(journal.path(), false, "v1", chrono::Utc::now(), || {
            Err(CoremlInstallError::new(
                "download_failed",
                "network down",
                65,
            ))
        })
        .unwrap();
        let _lease = super::super::lease::acquire(journal.path(), "parakeet")
            .unwrap()
            .unwrap();
        let outcome = heal_with(journal.path(), true, "v1", chrono::Utc::now(), || {
            panic!("lease is held")
        })
        .unwrap();
        assert_eq!(outcome, CoremlHeal::InstallRunning);
        let record = parakeet_status(journal.path());
        assert_eq!(record.install_state, "failed");
        assert_eq!(record.error_code.as_deref(), Some("download_failed"));
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[cfg(all(feature = "test-hooks", feature = "runtime-fetch-test"))]
    #[test]
    fn observed_coreml_fetch_persists_real_cumulative_bytes_and_terminal_revision() {
        use super::super::{
            status,
            test_hooks::{RuntimeFetchLoopback, with_runtime_fetch_loopback},
        };
        use std::io::{Read, Write};
        let root = tempfile::tempdir().unwrap();
        let journal = root.path().join("journal");
        let home = root.path().join("home");
        let cfg = config(&root.path().join("cache"));
        let one = row("one", b"first");
        let two = row("two", b"second");
        let rows = [&one, &two];
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            for bytes in [b"first".as_slice(), b"second".as_slice()] {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(10)))
                    .unwrap();
                let mut request = [0; 1024];
                assert!(stream.read(&mut request).unwrap() > 0);
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    bytes.len()
                )
                .unwrap();
                stream.write_all(bytes).unwrap();
            }
        });
        let mut observed = Vec::new();
        let outcome = heal_with(&journal, false, "test", chrono::Utc::now(), || {
            with_runtime_fetch_loopback(
                RuntimeFetchLoopback {
                    base_url,
                    allowed_hosts: vec!["127.0.0.1".into()],
                    allow_http: true,
                },
                || {
                    install_observed_with_rows(
                        &home,
                        &cfg,
                        false,
                        ("darwin", "arm64"),
                        &rows,
                        CoremlAdmission::Fixture,
                        &mut |staging, target| {
                            assert_eq!(parakeet_status(&journal).install_state, "installing");
                            fs::rename(staging, target)
                        },
                        &mut write_sentinel,
                        Some(&journal),
                        &mut |received, total| {
                            observed.push((received, total));
                            let mut current = status::read_status(&journal, "parakeet").unwrap();
                            current.progress_bytes_received = Some(received);
                            current.progress_bytes_total = total;
                            status::write_status(&journal, current).map_err(|error| {
                                CoremlInstallError::new(
                                    "status_write_failed",
                                    error.to_string(),
                                    74,
                                )
                            })?;
                            Ok(())
                        },
                    )
                },
            )
        })
        .unwrap();
        server.join().unwrap();
        assert_eq!(outcome, CoremlHeal::Installed);
        assert!(observed.iter().any(|(n, _)| *n == 5));
        assert_eq!(observed.last(), Some(&(11, Some(11))));
        assert!(observed.windows(2).all(|pair| pair[0].0 <= pair[1].0));
        assert_eq!(parakeet_status(&journal).install_state, "installed");
        check_with_rows(&home, &cfg, "darwin", "arm64", &rows).unwrap();
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[cfg(all(feature = "test-hooks", feature = "runtime-fetch-test"))]
    #[test]
    fn progress_callback_failure_prevents_coreml_publication_and_is_durable() {
        use super::super::test_hooks::{RuntimeFetchLoopback, with_runtime_fetch_loopback};
        use std::io::{Read, Write};
        let root = tempfile::tempdir().unwrap();
        let journal = root.path().join("journal");
        let home = root.path().join("home");
        let cache = root.path().join("cache");
        let cfg = config(&cache);
        let artifact = row("model.bin", b"valid");
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0; 1024];
            assert!(stream.read(&mut request).unwrap() > 0);
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nvalid",
                )
                .unwrap();
        });
        let mut called = false;
        let result = heal_with(&journal, false, "test", chrono::Utc::now(), || {
            with_runtime_fetch_loopback(
                RuntimeFetchLoopback {
                    base_url,
                    allowed_hosts: vec!["127.0.0.1".into()],
                    allow_http: true,
                },
                || {
                    install_observed_with_rows(
                        &home,
                        &cfg,
                        false,
                        ("darwin", "arm64"),
                        &[&artifact],
                        CoremlAdmission::Fixture,
                        &mut |_, _| panic!("failed progress must prevent publication"),
                        &mut write_sentinel,
                        Some(&journal),
                        &mut |received, _| {
                            assert!(received > 0);
                            called = true;
                            Err(CoremlInstallError::new(
                                "status_write_failed",
                                "injected progress write failure",
                                74,
                            ))
                        },
                    )
                },
            )
        })
        .unwrap();
        server.join().unwrap();
        assert!(called);
        assert!(matches!(result, CoremlHeal::Failed(_)));
        assert_eq!(
            parakeet_status(&journal).error_code.as_deref(),
            Some("status_write_failed")
        );
        assert!(!parakeet_coreml_model_root(&cache).exists());
        assert!(!parakeet_coreml_sentinel_path(&home).exists());
    }

    #[cfg(unix)]
    #[test]
    fn setup_integrity_proof_reuses_only_unchanged_tree_and_never_creates_locks() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let cache = root.path().join("cache");
        let cfg = config(&cache);
        let artifact = row("model.bin", b"valid");
        let rows = [&artifact];
        let proof = std::sync::Mutex::new(None);
        let check = || check_with_rows(&home, &cfg, "darwin", "arm64", &rows).is_ok();
        assert!(!current_verified_with_rows(
            &home, &cfg, &rows, &proof, check
        ));
        assert!(!home.exists());
        stage_installed(&home, &cache, b"valid");
        let mut count = 0;
        assert!(current_verified_with_rows(
            &home,
            &cfg,
            &rows,
            &proof,
            || {
                count += 1;
                check()
            }
        ));
        assert!(current_verified_with_rows(
            &home,
            &cfg,
            &rows,
            &proof,
            || {
                count += 1;
                check()
            }
        ));
        assert_eq!(count, 1, "unchanged tree reuses its digest proof");
        assert!(
            !default_parakeet_coreml_cache_dir(&home)
                .join(".admission.lock")
                .exists()
        );
        fs::write(
            parakeet_coreml_model_root(&cache).join("model.bin"),
            b"other",
        )
        .unwrap();
        assert!(
            !current_verified_with_rows(&home, &cfg, &rows, &proof, check),
            "same-size corruption invalidates proof"
        );
        fs::write(
            parakeet_coreml_model_root(&cache).join("model.bin"),
            b"valid",
        )
        .unwrap();
        assert!(
            current_verified_with_rows(&home, &cfg, &rows, &proof, check),
            "repair invalidates the negative proof"
        );
        let sibling_cache = root.path().join("other-cache");
        fs::create_dir(&sibling_cache).unwrap();
        let other_cfg = config(&sibling_cache);
        assert!(
            !current_verified_with_rows(&home, &other_cfg, &rows, &proof, || {
                check_with_rows(&home, &other_cfg, "darwin", "arm64", &rows).is_ok()
            }),
            "a config change with the same model root invalidates the proof"
        );
        assert!(current_verified_with_rows(
            &home, &cfg, &rows, &proof, check
        ));
        let lock = hold_coreml_admission(&home).unwrap();
        assert!(
            !current_verified_with_rows(&home, &cfg, &rows, &proof, check),
            "publication excludes observation"
        );
        drop(lock);
        fs::remove_file(parakeet_coreml_model_root(&cache).join("model.bin")).unwrap();
        assert!(!current_verified_with_rows(
            &home, &cfg, &rows, &proof, check
        ));
    }

    #[test]
    fn check_rejects_a_sentinel_for_a_removed_cache_directory() {
        let temporary = tempfile::tempdir().unwrap();
        let home = temporary.path().join("home");
        let missing = temporary.path().join("removed/cache");
        let cfg = config(&missing);
        let record = ParakeetCoremlSentinel::new(
            missing,
            "darwin",
            "arm64",
            FLUIDAUDIO_VERSION,
            "test",
            Vec::new(),
        );
        let path = parakeet_coreml_sentinel_path(&home);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        write_sentinel(&path, &record).unwrap();

        let error = check_parakeet_coreml_install_with_platform(&home, &cfg, "darwin", "arm64")
            .unwrap_err();
        assert_eq!(error.reason_code, "sentinel_not_ready");
    }

    #[test]
    fn install_refuses_unminted_artifact_without_writing() {
        let temporary = tempfile::tempdir().unwrap();
        let home = temporary.path().join("home");
        let config = config(&temporary.path().join("cache"));
        let artifact = row("model.mil", b"model");
        let rows = [&artifact];
        let error = install_with_rows(
            &home,
            &config,
            false,
            ("darwin", "arm64"),
            &rows,
            &mut |_, _| {},
        )
        .unwrap_err();
        assert_eq!(error.reason_code, "component_packaged");
        assert!(!parakeet_coreml_model_root(&parakeet_coreml_cache_dir(&config, &home)).exists());
        assert!(!parakeet_coreml_sentinel_path(&home).exists());
    }

    #[test]
    fn check_empty_host_names_the_install_command_without_requests() {
        let temporary = tempfile::tempdir().unwrap();
        let home = temporary.path().join("home");
        let cfg = config(&temporary.path().join("cache"));
        let artifact = row("model.mil", b"model");
        let rows = [&artifact];
        let error = check_with_rows(&home, &cfg, "darwin", "arm64", &rows).unwrap_err();
        assert_eq!(error.reason_code, "sentinel_not_ready");
        assert!(
            error
                .to_string()
                .contains("solstone journal install-models")
        );
    }

    #[test]
    fn the_canonical_coreml_host_is_accepted_by_the_guard() {
        require_coreml_host("darwin", "arm64").unwrap();
    }

    #[test]
    fn owner_output_for_macos_aarch64_is_accepted_by_the_guard() {
        let (os_name, arch) = canonical_host_pair("macos", "aarch64");
        require_coreml_host(os_name, arch).unwrap();
    }

    #[test]
    fn a_genuinely_unsupported_host_is_still_refused() {
        let error =
            require_coreml_host("linux", "x86_64").expect_err("linux/x86_64 is not a CoreML host");
        assert_eq!(error.reason_code, "platform_unsupported");
    }

    #[test]
    fn same_name_and_size_different_digest_fails_check() {
        let temporary = tempfile::tempdir().unwrap();
        let home = temporary.path().join("home");
        let cache = temporary.path().join("cache");
        fs::create_dir_all(&cache).unwrap();
        let cfg = config(&cache);
        let artifact = row("model.bin", b"correct-bytes");
        let rows = [&artifact];

        let target = parakeet_coreml_model_root(&cache);
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("model.bin"), b"wrong---bytes").unwrap();

        let sentinel = ParakeetCoremlSentinel::new(
            cache.clone(),
            "darwin",
            "arm64",
            FLUIDAUDIO_VERSION,
            artifact.version,
            Vec::new(),
        );
        let sentinel_path = parakeet_coreml_sentinel_path(&home);
        write_sentinel(&sentinel_path, &sentinel).unwrap();

        let error = check_with_rows(&home, &cfg, "darwin", "arm64", &rows).unwrap_err();
        assert_eq!(error.reason_code, "model_incomplete");
    }

    #[test]
    fn legacy_sentinel_adopts_matching_files_with_zero_fetches() {
        let temporary = tempfile::tempdir().unwrap();
        let home = temporary.path().join("home");
        let cache = temporary.path().join("cache");
        fs::create_dir_all(&cache).unwrap();
        let cfg = config(&cache);
        let artifact = row("model.bin", b"matching-bytes");
        let rows = [&artifact];

        let target = parakeet_coreml_model_root(&cache);
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("model.bin"), b"matching-bytes").unwrap();

        let sentinel_path = parakeet_coreml_sentinel_path(&home);
        fs::create_dir_all(sentinel_path.parent().unwrap()).unwrap();
        fs::write(
            &sentinel_path,
            serde_json::json!({
                "schema_version": 1,
                "backend": "parakeet",
                "variant": "coreml",
                "model_version": "v3",
                "quantization": "fp32",
                "fluidaudio_version": "0.14.0",
                "platform": {"os": "darwin", "arch": "arm64"},
                "cache_dir": cache,
            })
            .to_string(),
        )
        .unwrap();

        let mut fetch_count = 0;
        let mut fetch = |_query: &RuntimeFetchQuery<'_>, _dest: &Path| {
            fetch_count += 1;
            Ok(())
        };

        refresh_with_rows(
            temporary.path(),
            &home,
            &cfg,
            ("darwin", "arm64"),
            &rows,
            &mut fetch,
        )
        .unwrap();
        assert_eq!(fetch_count, 0);

        let reloaded = read_valid_parakeet_coreml_sentinel(&home, "darwin", "arm64").unwrap();
        assert_eq!(reloaded.catalog_version(), artifact.version);
        assert_eq!(reloaded.files().len(), 1);
        assert_eq!(reloaded.files()[0].file, "model.bin");
    }

    #[test]
    fn sentinel_cache_dir_differs_from_configured_skips_fetch() {
        let temporary = tempfile::tempdir().unwrap();
        let home = temporary.path().join("home");
        let cache = temporary.path().join("cache");
        let other_cache = temporary.path().join("other_cache");
        fs::create_dir_all(&cache).unwrap();
        fs::create_dir_all(&other_cache).unwrap();
        let cfg = config(&cache);
        let artifact = row("model.bin", b"bytes");
        let rows = [&artifact];

        let sentinel = ParakeetCoremlSentinel::new(
            other_cache.clone(),
            "darwin",
            "arm64",
            FLUIDAUDIO_VERSION,
            artifact.version,
            Vec::new(),
        );
        let sentinel_path = parakeet_coreml_sentinel_path(&home);
        write_sentinel(&sentinel_path, &sentinel).unwrap();

        let mut fetch_count = 0;
        let mut fetch = |_query: &RuntimeFetchQuery<'_>, _dest: &Path| {
            fetch_count += 1;
            Ok(())
        };

        refresh_with_rows(
            temporary.path(),
            &home,
            &cfg,
            ("darwin", "arm64"),
            &rows,
            &mut fetch,
        )
        .unwrap();
        assert_eq!(fetch_count, 0);

        let reloaded = read_valid_parakeet_coreml_sentinel(&home, "darwin", "arm64").unwrap();
        assert_eq!(reloaded.cache_dir(), other_cache);
    }

    #[test]
    fn matching_digest_and_catalog_version_makes_zero_fetches() {
        let temporary = tempfile::tempdir().unwrap();
        let home = temporary.path().join("home");
        let cache = temporary.path().join("cache");
        fs::create_dir_all(&cache).unwrap();
        let cfg = config(&cache);
        let artifact = row("model.bin", b"perfect-bytes");
        let rows = [&artifact];

        let target = parakeet_coreml_model_root(&cache);
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("model.bin"), b"perfect-bytes").unwrap();

        let sentinel = ParakeetCoremlSentinel::new(
            cache.clone(),
            "darwin",
            "arm64",
            FLUIDAUDIO_VERSION,
            artifact.version,
            Vec::new(),
        );
        let sentinel_path = parakeet_coreml_sentinel_path(&home);
        write_sentinel(&sentinel_path, &sentinel).unwrap();

        let mut fetch_count = 0;
        let mut fetch = |_query: &RuntimeFetchQuery<'_>, _dest: &Path| {
            fetch_count += 1;
            Ok(())
        };

        refresh_with_rows(
            temporary.path(),
            &home,
            &cfg,
            ("darwin", "arm64"),
            &rows,
            &mut fetch,
        )
        .unwrap();
        assert_eq!(fetch_count, 0);
    }

    #[test]
    fn incumbent_exists_sentinel_writer_fails_preserves_tree_and_sentinel() {
        let temporary = tempfile::tempdir().unwrap();
        let home = temporary.path().join("home");
        let cache = temporary.path().join("cache");
        fs::create_dir_all(&cache).unwrap();
        let cfg = config(&cache);
        let artifact = row("model.bin", b"new-bytes");
        let rows = [&artifact];

        let target = parakeet_coreml_model_root(&cache);
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("old.bin"), b"old-bytes").unwrap();

        let old_sentinel = ParakeetCoremlSentinel::new(
            cache.clone(),
            "darwin",
            "arm64",
            FLUIDAUDIO_VERSION,
            "old-version",
            Vec::new(),
        );
        let sentinel_path = parakeet_coreml_sentinel_path(&home);
        write_sentinel(&sentinel_path, &old_sentinel).unwrap();

        let mut fetch = |_query: &RuntimeFetchQuery<'_>, dest: &Path| {
            fs::write(dest, b"new-bytes")
                .map_err(|e| CoremlInstallError::new("io", e.to_string(), 74))
        };
        let mut publish = |staging: &Path, target: &Path| fs::rename(staging, target);
        let mut failing_write = |_path: &Path, _sentinel: &ParakeetCoremlSentinel| {
            Err(std::io::Error::other("injected sentinel failure"))
        };

        let result = download_and_commit_coreml(
            &home,
            &cfg,
            ("darwin", "arm64"),
            &rows,
            true,
            &mut fetch,
            &mut publish,
            &mut failing_write,
        );

        assert_eq!(result.unwrap_err().reason_code, "sentinel_write_failed");
        assert_eq!(fs::read(target.join("old.bin")).unwrap(), b"old-bytes");
        assert!(!target.join("model.bin").exists());
        let reloaded = read_valid_parakeet_coreml_sentinel(&home, "darwin", "arm64").unwrap();
        assert_eq!(reloaded.catalog_version(), "old-version");
    }

    #[test]
    fn fetch_fails_on_later_file_preserves_tree_and_sentinel() {
        let temporary = tempfile::tempdir().unwrap();
        let home = temporary.path().join("home");
        let cache = temporary.path().join("cache");
        fs::create_dir_all(&cache).unwrap();
        let cfg = config(&cache);
        let artifact1 = row("file1.bin", b"bytes1");
        let artifact2 = row("file2.bin", b"bytes2");
        let rows = [&artifact1, &artifact2];

        let target = parakeet_coreml_model_root(&cache);
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("old.bin"), b"old-content").unwrap();

        let old_sentinel = ParakeetCoremlSentinel::new(
            cache.clone(),
            "darwin",
            "arm64",
            FLUIDAUDIO_VERSION,
            "old-v",
            Vec::new(),
        );
        let sentinel_path = parakeet_coreml_sentinel_path(&home);
        write_sentinel(&sentinel_path, &old_sentinel).unwrap();

        let mut fetch_step = 0;
        let mut fetch = |_query: &RuntimeFetchQuery<'_>, dest: &Path| {
            fetch_step += 1;
            if fetch_step == 1 {
                fs::write(dest, b"bytes1")
                    .map_err(|e| CoremlInstallError::new("io", e.to_string(), 74))
            } else {
                Err(CoremlInstallError::new(
                    "download_failed",
                    "network dropped",
                    74,
                ))
            }
        };

        let error = refresh_with_rows(
            temporary.path(),
            &home,
            &cfg,
            ("darwin", "arm64"),
            &rows,
            &mut fetch,
        )
        .unwrap_err();
        assert_eq!(error.reason_code, "download_failed");
        assert_eq!(fs::read(target.join("old.bin")).unwrap(), b"old-content");
        assert!(!target.join("file1.bin").exists());
        let reloaded = read_valid_parakeet_coreml_sentinel(&home, "darwin", "arm64").unwrap();
        assert_eq!(reloaded.catalog_version(), "old-v");
    }

    #[test]
    fn fetch_writes_matching_bytes_succeeds_and_second_refresh_is_noop() {
        let temporary = tempfile::tempdir().unwrap();
        let home = temporary.path().join("home");
        let cache = temporary.path().join("cache");
        fs::create_dir_all(&cache).unwrap();
        let cfg = config(&cache);
        let artifact = row("model.bin", b"new-payload");
        let rows = [&artifact];

        let target = parakeet_coreml_model_root(&cache);
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("model.bin"), b"stale-payload").unwrap();

        let old_sentinel = ParakeetCoremlSentinel::new(
            cache.clone(),
            "darwin",
            "arm64",
            FLUIDAUDIO_VERSION,
            "old-version",
            Vec::new(),
        );
        let sentinel_path = parakeet_coreml_sentinel_path(&home);
        write_sentinel(&sentinel_path, &old_sentinel).unwrap();

        let mut fetch_count = 0;
        let mut fetch = |_query: &RuntimeFetchQuery<'_>, dest: &Path| {
            fetch_count += 1;
            fs::write(dest, b"new-payload")
                .map_err(|e| CoremlInstallError::new("io", e.to_string(), 74))
        };

        refresh_with_rows(
            temporary.path(),
            &home,
            &cfg,
            ("darwin", "arm64"),
            &rows,
            &mut fetch,
        )
        .unwrap();
        assert_eq!(fetch_count, 1);
        check_with_rows(&home, &cfg, "darwin", "arm64", &rows).unwrap();

        let mut second_fetch_count = 0;
        let mut second_fetch = |_query: &RuntimeFetchQuery<'_>, _dest: &Path| {
            second_fetch_count += 1;
            Ok(())
        };
        refresh_with_rows(
            temporary.path(),
            &home,
            &cfg,
            ("darwin", "arm64"),
            &rows,
            &mut second_fetch,
        )
        .unwrap();
        assert_eq!(second_fetch_count, 0);
    }

    #[test]
    fn changed_pin_refresh_fetch_error_preserves_doctor_ok_and_canonical_bytes() {
        let temporary = tempfile::tempdir().unwrap();
        let home = temporary.path().join("home");
        let cache = default_parakeet_coreml_cache_dir(&home);
        fs::create_dir_all(&cache).unwrap();
        let cfg = config(&cache);
        let incumbent_sha = format!("{:x}", Sha256::digest(b"current-bytes"));
        let target = parakeet_coreml_model_root(&cache);
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("model.bin"), b"current-bytes").unwrap();

        let files = vec![ParakeetCoremlFile {
            file: "model.bin".to_string(),
            sha256: incumbent_sha,
            size_bytes: b"current-bytes".len() as u64,
        }];
        let sentinel = ParakeetCoremlSentinel::new(
            cache.clone(),
            "darwin",
            "arm64",
            FLUIDAUDIO_VERSION,
            "old-catalog-version",
            files,
        );
        let sentinel_path = parakeet_coreml_sentinel_path(&home);
        write_sentinel(&sentinel_path, &sentinel).unwrap();

        let new_artifact = row("model.bin", b"new-pin-bytes");
        let rows = [&new_artifact];

        let mut fetch = |_query: &RuntimeFetchQuery<'_>, _dest: &Path| {
            Err(CoremlInstallError::new(
                "download_failed",
                "injected fetch failure",
                74,
            ))
        };

        let result = refresh_with_rows(
            temporary.path(),
            &home,
            &cfg,
            ("darwin", "arm64"),
            &rows,
            &mut fetch,
        );
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().reason_code, "download_failed");

        let doc_result = check_parakeet_coreml_cache_doctor(&home, &cfg, "darwin", "arm64");
        assert!(doc_result.is_ok());
        assert_eq!(
            fs::read(target.join("model.bin")).unwrap(),
            b"current-bytes"
        );
    }

    #[test]
    fn absent_configured_cache_dir_created_and_sentinel_valid_without_sockets() {
        let temporary = tempfile::tempdir().unwrap();
        let home = temporary.path().join("home");
        let override_cache = temporary.path().join("override/cache");
        let cfg = config(&override_cache);
        let artifact = row("model.bin", b"model-bytes");
        let rows = [&artifact];

        assert!(!override_cache.exists());

        let mut fetch = |_query: &RuntimeFetchQuery<'_>, dest: &Path| {
            fs::write(dest, b"model-bytes")
                .map_err(|e| CoremlInstallError::new("io", e.to_string(), 74))
        };
        let mut publish = |staging: &Path, target: &Path| fs::rename(staging, target);
        let mut write = write_sentinel;

        let res = download_and_commit_coreml(
            &home,
            &cfg,
            ("darwin", "arm64"),
            &rows,
            false,
            &mut fetch,
            &mut publish,
            &mut write,
        );
        assert!(res.is_ok());
        assert!(override_cache.exists());
        assert!(read_valid_parakeet_coreml_sentinel(&home, "darwin", "arm64").is_some());
    }

    #[test]
    fn recover_pending_tree_restores_canonical_admission() {
        let temporary = tempfile::tempdir().unwrap();
        let home = temporary.path().join("home");
        let cache = default_parakeet_coreml_cache_dir(&home);
        let cfg = config(&cache);
        let target = parakeet_coreml_model_root(&cache);
        let target_parent = target.parent().unwrap();
        fs::create_dir_all(target_parent).unwrap();

        let tree_backup = target_parent.join(".parakeet-tdt-0.6b-v3.previous.backup");
        fs::create_dir_all(&tree_backup).unwrap();
        fs::write(tree_backup.join("model.bin"), b"canonical-bytes").unwrap();

        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("model.bin"), b"uncommitted-bytes").unwrap();

        let marker_path = cache.join(".publication.json");
        let marker = PublicationMarker {
            phase: "pending".to_string(),
            tree_canonical: Some(target.clone()),
            tree_backup: Some(tree_backup.clone()),
            sentinel_backup: None,
            staging: None,
            published_catalog_version: Some("published-v2".to_string()),
            published_files: Some(vec![ParakeetCoremlFile {
                file: "model.bin".to_string(),
                sha256: "uncommitted-sha".to_string(),
                size_bytes: 17,
            }]),
        };
        write_publication_marker(&marker_path, &marker).unwrap();

        recover_parakeet_coreml_publication(&home).unwrap();

        assert_eq!(
            fs::read(target.join("model.bin")).unwrap(),
            b"canonical-bytes"
        );
        assert!(!marker_path.exists());
        assert!(!tree_backup.exists());

        let artifact = row("model.bin", b"canonical-bytes");
        let rows = [&artifact];
        let sentinel = ParakeetCoremlSentinel::new(
            cache.clone(),
            "darwin",
            "arm64",
            FLUIDAUDIO_VERSION,
            artifact.version,
            vec![ParakeetCoremlFile {
                file: "model.bin".to_string(),
                sha256: artifact.sha256.to_string(),
                size_bytes: artifact.size_bytes,
            }],
        );
        let sentinel_path = parakeet_coreml_sentinel_path(&home);
        write_sentinel(&sentinel_path, &sentinel).unwrap();

        let mut fetch =
            |_query: &RuntimeFetchQuery<'_>, _dest: &Path| -> Result<(), CoremlInstallError> {
                panic!("fetch closure must not be called when bytes match");
            };
        refresh_with_rows(
            temporary.path(),
            &home,
            &cfg,
            ("darwin", "arm64"),
            &rows,
            &mut fetch,
        )
        .unwrap();

        recover_parakeet_coreml_publication(&home).unwrap();
    }

    #[test]
    fn committed_marker_deletes_backup_and_preserves_new_tree() {
        let temporary = tempfile::tempdir().unwrap();
        let home = temporary.path().join("home");
        let cache = default_parakeet_coreml_cache_dir(&home);
        let target = parakeet_coreml_model_root(&cache);
        let target_parent = target.parent().unwrap();
        fs::create_dir_all(target_parent).unwrap();

        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("model.bin"), b"new-tree-bytes").unwrap();

        let tree_backup = target_parent.join(".parakeet-tdt-0.6b-v3.previous.backup");
        fs::create_dir_all(&tree_backup).unwrap();
        fs::write(tree_backup.join("model.bin"), b"old-tree-bytes").unwrap();

        let marker_path = cache.join(".publication.json");
        let marker = PublicationMarker {
            phase: "committed".to_string(),
            tree_canonical: None,
            tree_backup: Some(tree_backup.clone()),
            sentinel_backup: None,
            staging: None,
            published_catalog_version: None,
            published_files: None,
        };
        write_publication_marker(&marker_path, &marker).unwrap();

        recover_parakeet_coreml_publication(&home).unwrap();

        assert!(!marker_path.exists());
        assert!(!tree_backup.exists());
        assert_eq!(
            fs::read(target.join("model.bin")).unwrap(),
            b"new-tree-bytes"
        );
    }

    #[test]
    fn no_marker_plus_leftover_backup_stays_removed() {
        let temporary = tempfile::tempdir().unwrap();
        let home = temporary.path().join("home");
        let cache = default_parakeet_coreml_cache_dir(&home);
        let target = parakeet_coreml_model_root(&cache);
        let target_parent = target.parent().unwrap();
        fs::create_dir_all(target_parent).unwrap();

        let tree_backup = target_parent.join(".parakeet-tdt-0.6b-v3.previous.backup");
        fs::create_dir_all(&tree_backup).unwrap();
        fs::write(tree_backup.join("model.bin"), b"backup-bytes").unwrap();

        recover_parakeet_coreml_publication(&home).unwrap();

        assert!(tree_backup.exists());
        assert!(!target.exists());
    }

    #[test]
    fn failed_restore_leaves_backup_and_pending_marker() {
        let temporary = tempfile::tempdir().unwrap();
        let home = temporary.path().join("home");
        let cache = default_parakeet_coreml_cache_dir(&home);
        fs::create_dir_all(&cache).unwrap();
        let target = temporary.path().join("missing-parent").join("tree");

        let backup_dir = temporary.path().join("external_backup");
        fs::create_dir_all(&backup_dir).unwrap();
        fs::write(backup_dir.join("model.bin"), b"bytes").unwrap();

        let marker_path = cache.join(".publication.json");
        let marker = PublicationMarker {
            phase: "pending".to_string(),
            tree_canonical: Some(target.clone()),
            tree_backup: Some(backup_dir.clone()),
            sentinel_backup: None,
            staging: None,
            published_catalog_version: Some("v2".to_string()),
            published_files: Some(vec![]),
        };
        write_publication_marker(&marker_path, &marker).unwrap();

        let result = recover_parakeet_coreml_publication(&home);
        assert!(result.is_err());
        assert!(backup_dir.exists());
        assert_eq!(fs::read(backup_dir.join("model.bin")).unwrap(), b"bytes");
        assert!(marker_path.exists());
        assert!(!target.exists());
    }

    #[test]
    fn sentinel_only_pending_restores_sentinel() {
        let temporary = tempfile::tempdir().unwrap();
        let home = temporary.path().join("home");
        let cache = default_parakeet_coreml_cache_dir(&home);
        let sentinel_path = parakeet_coreml_sentinel_path(&home);
        let sentinel_parent = sentinel_path.parent().unwrap();
        fs::create_dir_all(sentinel_parent).unwrap();

        let sentinel_backup = sentinel_parent.join(".install-complete.backup");
        let original_sentinel = ParakeetCoremlSentinel::new(
            cache.clone(),
            "darwin",
            "arm64",
            FLUIDAUDIO_VERSION,
            "v1",
            vec![],
        );
        write_sentinel(&sentinel_backup, &original_sentinel).unwrap();

        let marker_path = cache.join(".publication.json");
        let marker = PublicationMarker {
            phase: "pending".to_string(),
            tree_canonical: None,
            tree_backup: None,
            sentinel_backup: Some(sentinel_backup.clone()),
            staging: None,
            published_catalog_version: Some("v2".to_string()),
            published_files: Some(vec![]),
        };
        write_publication_marker(&marker_path, &marker).unwrap();

        recover_parakeet_coreml_publication(&home).unwrap();

        assert!(!marker_path.exists());
        assert!(sentinel_path.exists());
        let reloaded = read_valid_parakeet_coreml_sentinel(&home, "darwin", "arm64").unwrap();
        assert_eq!(reloaded.catalog_version(), "v1");
    }

    #[test]
    fn contention_across_journals_holds_publication_lock_through_fetch() {
        use std::sync::mpsc;
        use std::thread;

        let temporary = tempfile::tempdir().unwrap();
        let home = temporary.path().join("home");
        let cache = default_parakeet_coreml_cache_dir(&home);
        fs::create_dir_all(&cache).unwrap();
        let cfg = config(&cache);
        let artifact: &'static Artifact = Box::leak(Box::new(row("model.bin", b"new-bytes")));
        let rows: &'static [&'static Artifact] = Box::leak(Box::new([artifact]));

        let target = parakeet_coreml_model_root(&cache);
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("model.bin"), b"initial-bytes").unwrap();

        let initial_sha = format!("{:x}", Sha256::digest(b"initial-bytes"));
        let sentinel = ParakeetCoremlSentinel::new(
            cache.clone(),
            "darwin",
            "arm64",
            FLUIDAUDIO_VERSION,
            "old-version",
            vec![ParakeetCoremlFile {
                file: "model.bin".to_string(),
                sha256: initial_sha,
                size_bytes: b"initial-bytes".len() as u64,
            }],
        );
        let sentinel_path = parakeet_coreml_sentinel_path(&home);
        write_sentinel(&sentinel_path, &sentinel).unwrap();

        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();

        let home_clone = home.clone();
        let cfg_clone = cfg.clone();
        let journal_clone = temporary.path().to_path_buf();
        let worker = thread::spawn(move || {
            let mut fetch = |_query: &RuntimeFetchQuery<'_>, dest: &Path| {
                entered_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                fs::write(dest, b"new-bytes")
                    .map_err(|e| CoremlInstallError::new("io", e.to_string(), 74))
            };
            refresh_with_rows(
                &journal_clone,
                &home_clone,
                &cfg_clone,
                ("darwin", "arm64"),
                rows,
                &mut fetch,
            )
            .unwrap();
        });

        entered_rx.recv().unwrap();

        let mut second_fetch_count = 0;
        let mut second_fetch = |_query: &RuntimeFetchQuery<'_>, _dest: &Path| {
            second_fetch_count += 1;
            Ok(())
        };
        let other_journal = temporary.path().join("other-journal");
        let second_result = refresh_with_rows(
            &other_journal,
            &home,
            &cfg,
            ("darwin", "arm64"),
            rows,
            &mut second_fetch,
        );
        assert!(second_result.is_ok());
        assert_eq!(second_fetch_count, 0);

        assert!(check_parakeet_coreml_cache_doctor(&home, &cfg, "darwin", "arm64").is_ok());

        release_tx.send(()).unwrap();
        worker.join().unwrap();

        let mut following_fetch_count = 0;
        let mut following_fetch = |_query: &RuntimeFetchQuery<'_>, _dest: &Path| {
            following_fetch_count += 1;
            Ok(())
        };
        let following_result = refresh_with_rows(
            temporary.path(),
            &home,
            &cfg,
            ("darwin", "arm64"),
            rows,
            &mut following_fetch,
        );
        assert!(following_result.is_ok());
        assert_eq!(following_fetch_count, 0);
    }

    #[test]
    fn sentinel_removed_inside_paused_fetch_aborts_without_republish() {
        use std::sync::mpsc;
        use std::thread;

        let temporary = tempfile::tempdir().unwrap();
        let home = temporary.path().join("home");
        let cache = default_parakeet_coreml_cache_dir(&home);
        fs::create_dir_all(&cache).unwrap();
        let cfg = config(&cache);
        let artifact: &'static Artifact = Box::leak(Box::new(row("model.bin", b"new-bytes")));
        let rows: &'static [&'static Artifact] = Box::leak(Box::new([artifact]));

        let target = parakeet_coreml_model_root(&cache);
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("model.bin"), b"initial-bytes").unwrap();

        let initial_sha = format!("{:x}", Sha256::digest(b"initial-bytes"));
        let sentinel = ParakeetCoremlSentinel::new(
            cache.clone(),
            "darwin",
            "arm64",
            FLUIDAUDIO_VERSION,
            "old-version",
            vec![ParakeetCoremlFile {
                file: "model.bin".to_string(),
                sha256: initial_sha,
                size_bytes: b"initial-bytes".len() as u64,
            }],
        );
        let sentinel_path = parakeet_coreml_sentinel_path(&home);
        write_sentinel(&sentinel_path, &sentinel).unwrap();

        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();

        let home_clone = home.clone();
        let cfg_clone = cfg.clone();
        let journal_clone = temporary.path().to_path_buf();
        let worker = thread::spawn(move || {
            let mut fetch = |_query: &RuntimeFetchQuery<'_>, dest: &Path| {
                entered_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                fs::write(dest, b"new-bytes")
                    .map_err(|e| CoremlInstallError::new("io", e.to_string(), 74))
            };
            refresh_with_rows(
                &journal_clone,
                &home_clone,
                &cfg_clone,
                ("darwin", "arm64"),
                rows,
                &mut fetch,
            )
        });

        entered_rx.recv().unwrap();
        fs::remove_file(&sentinel_path).unwrap();
        release_tx.send(()).unwrap();

        let result = worker.join().unwrap();
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().reason_code, "sentinel_removed");
        assert!(!sentinel_path.exists());
        assert_eq!(
            fs::read(target.join("model.bin")).unwrap(),
            b"initial-bytes"
        );

        let mut publish = |staging: &Path, target: &Path| fs::rename(staging, target);
        let mut write = write_sentinel;
        let mut fresh_fetch = |_query: &RuntimeFetchQuery<'_>, dest: &Path| {
            fs::write(dest, b"new-bytes")
                .map_err(|e| CoremlInstallError::new("io", e.to_string(), 74))
        };
        download_and_commit_coreml(
            &home,
            &cfg,
            ("darwin", "arm64"),
            rows,
            false,
            &mut fresh_fetch,
            &mut publish,
            &mut write,
        )
        .unwrap();

        assert!(read_valid_parakeet_coreml_sentinel(&home, "darwin", "arm64").is_some());
        assert_eq!(fs::read(target.join("model.bin")).unwrap(), b"new-bytes");
    }
    #[cfg(all(unix, feature = "full-tests"))]
    #[test]
    fn failed_sentinel_publication_retains_recovery_intent_until_io_fault_is_lifted() {
        use std::os::unix::fs::PermissionsExt;
        struct Permissions(Vec<(PathBuf, fs::Permissions)>);
        impl Drop for Permissions {
            fn drop(&mut self) {
                for (path, mode) in &self.0 {
                    let _ = fs::set_permissions(path, mode.clone());
                }
            }
        }
        for remove_temporary in [false, true] {
            let temporary = tempfile::tempdir().unwrap();
            let home = temporary.path().join("home");
            let cache = temporary.path().join("override/cache");
            fs::create_dir_all(&cache).unwrap();
            let cfg = config(&cache);
            let target = parakeet_coreml_model_root(&cache);
            let old = row("Encoder.mlmodelc/weights/weight.bin", b"incumbent");
            let path = target.join(old.filename);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, b"incumbent").unwrap();
            let sentinel = parakeet_coreml_sentinel_path(&home);
            write_sentinel(
                &sentinel,
                &ParakeetCoremlSentinel::new(
                    cache.clone(),
                    "darwin",
                    "arm64",
                    FLUIDAUDIO_VERSION,
                    "old-pin",
                    vec![ParakeetCoremlFile {
                        file: old.filename.to_owned(),
                        sha256: old.sha256.to_owned(),
                        size_bytes: old.size_bytes,
                    }],
                ),
            )
            .unwrap();
            let marker = sentinel.parent().unwrap().join(".publication.json");
            let parents = [
                target.parent().unwrap().to_path_buf(),
                sentinel.parent().unwrap().to_path_buf(),
            ];
            let permissions = Permissions(
                parents
                    .iter()
                    .map(|path| (path.clone(), fs::metadata(path).unwrap().permissions()))
                    .collect(),
            );
            let staging = temporary.path().join("staged");
            fs::create_dir_all(staging.join("Encoder.mlmodelc/weights")).unwrap();
            fs::write(staging.join(old.filename), b"replacement").unwrap();
            let mut publish = |stage: &Path, canonical: &Path| {
                fs::rename(stage, canonical)?;
                if remove_temporary {
                    for entry in fs::read_dir(sentinel.parent().unwrap())? {
                        let entry = entry?;
                        if entry
                            .file_name()
                            .to_string_lossy()
                            .starts_with(".install-complete.tmp.")
                        {
                            fs::remove_file(entry.path())?;
                        }
                    }
                }
                fs::set_permissions(&parents[0], fs::Permissions::from_mode(0o555))?;
                if !remove_temporary {
                    fs::set_permissions(&parents[1], fs::Permissions::from_mode(0o555))?;
                }
                Ok(())
            };
            let error = publish_coreml(
                &home,
                "darwin",
                "arm64",
                &cache,
                "replacement-pin",
                vec![ParakeetCoremlFile {
                    file: old.filename.to_owned(),
                    sha256: format!("{:x}", Sha256::digest(b"replacement")),
                    size_bytes: b"replacement".len() as u64,
                }],
                Some(&staging),
                &target,
                &mut publish,
                &mut write_sentinel,
            )
            .unwrap_err();
            assert!(error.to_string().contains("restore failed"));
            assert!(
                marker.exists(),
                "failed rollback must retain discoverable recovery intent"
            );
            let pending: PublicationMarker =
                serde_json::from_slice(&fs::read(&marker).unwrap()).unwrap();
            assert_eq!(
                fs::read(pending.tree_backup.unwrap().join(old.filename)).unwrap(),
                b"incumbent"
            );
            drop(permissions);
            // The next real installer entry recovers before eligibility/fetch.
            refresh_with_rows(
                temporary.path(),
                &home,
                &cfg,
                ("darwin", "arm64"),
                &[&old],
                &mut |_, _| panic!("recovered matching bytes must not fetch"),
            )
            .unwrap();
            assert!(!marker.exists());
            assert_eq!(fs::read(target.join(old.filename)).unwrap(), b"incumbent");
            assert!(check_parakeet_coreml_cache_doctor(&home, &cfg, "darwin", "arm64").is_ok());
        }
    }
    #[cfg(feature = "full-tests")]
    #[test]
    fn doctor_waits_only_for_the_short_tree_and_receipt_publication() {
        use std::{sync::mpsc, thread};
        let temporary = tempfile::tempdir().unwrap();
        let home = temporary.path().join("home");
        let cache = default_parakeet_coreml_cache_dir(&home);
        fs::create_dir_all(&cache).unwrap();
        let cfg = config(&cache);
        let target = parakeet_coreml_model_root(&cache);
        let staging = temporary.path().join("staging");
        let mut files = Vec::new();
        let mut old_files = Vec::new();
        for row in rows().unwrap() {
            for (root, bytes) in [
                (&target, b"incumbent".as_slice()),
                (&staging, b"replacement".as_slice()),
            ] {
                let path = root.join(row.filename);
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                fs::write(path, bytes).unwrap();
            }
            old_files.push(ParakeetCoremlFile {
                file: row.filename.to_owned(),
                sha256: format!("{:x}", Sha256::digest(b"incumbent")),
                size_bytes: b"incumbent".len() as u64,
            });
            files.push(ParakeetCoremlFile {
                file: row.filename.to_owned(),
                sha256: format!("{:x}", Sha256::digest(b"replacement")),
                size_bytes: b"replacement".len() as u64,
            });
        }
        write_sentinel(
            &parakeet_coreml_sentinel_path(&home),
            &ParakeetCoremlSentinel::new(
                cache.clone(),
                "darwin",
                "arm64",
                FLUIDAUDIO_VERSION,
                "old-pin",
                old_files,
            ),
        )
        .unwrap();
        assert!(check_parakeet_coreml_cache_doctor(&home, &cfg, "darwin", "arm64").is_ok());
        let (moved_tx, moved_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let worker_home = home.clone();
        let worker_cache = cache.clone();
        let worker = thread::spawn(move || {
            let mut publish = |stage: &Path, canonical: &Path| {
                fs::rename(stage, canonical)?;
                moved_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                Ok(())
            };
            publish_coreml(
                &worker_home,
                "darwin",
                "arm64",
                &worker_cache,
                "new-pin",
                files,
                Some(&staging),
                &target,
                &mut publish,
                &mut write_sentinel,
            )
            .unwrap();
        });
        moved_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let (done_tx, done_rx) = mpsc::channel();
        let reader = thread::spawn(move || {
            done_tx
                .send(check_parakeet_coreml_cache_doctor(
                    &home, &cfg, "darwin", "arm64",
                ))
                .unwrap()
        });
        let premature = done_rx.recv_timeout(Duration::from_millis(100));
        release_tx.send(()).unwrap();
        worker.join().unwrap();
        reader.join().unwrap();
        assert!(matches!(premature, Err(mpsc::RecvTimeoutError::Timeout)));
        assert_eq!(done_rx.recv().unwrap().unwrap(), cache);
    }
    #[test]
    fn unreferenced_download_stage_does_not_strand_refresh_retry() {
        let temporary = tempfile::tempdir().unwrap();
        let home = temporary.path().join("home");
        let cache = default_parakeet_coreml_cache_dir(&home);
        fs::create_dir_all(&cache).unwrap();
        let cfg = config(&cache);
        let target = parakeet_coreml_model_root(&cache);
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("model.bin"), b"old-bytes").unwrap();
        write_sentinel(
            &parakeet_coreml_sentinel_path(&home),
            &ParakeetCoremlSentinel::new(
                cache.clone(),
                "darwin",
                "arm64",
                FLUIDAUDIO_VERSION,
                "old",
                vec![],
            ),
        )
        .unwrap();
        let orphan = target.parent().unwrap().join(format!(
            ".parakeet-tdt-0.6b-v3.stage.{}.{}",
            std::process::id(),
            COUNTER.load(Ordering::Relaxed)
        ));
        fs::create_dir_all(&orphan).unwrap();
        fs::write(orphan.join("model.bin"), b"interrupted-download").unwrap();
        let offered = row("model.bin", b"new-bytes");
        let mut calls = 0;
        refresh_with_rows(
            temporary.path(),
            &home,
            &cfg,
            ("darwin", "arm64"),
            &[&offered],
            &mut |_, path| {
                calls += 1;
                fs::write(path, b"new-bytes")
                    .map_err(|error| CoremlInstallError::new("write_failed", error.to_string(), 74))
            },
        )
        .unwrap();
        assert_eq!(calls, 1);
        assert_eq!(fs::read(target.join("model.bin")).unwrap(), b"new-bytes");
        assert!(check_with_rows(&home, &cfg, "darwin", "arm64", &[&offered]).is_ok());
    }

    #[test]
    #[cfg(feature = "full-tests")]
    fn coreml_heal_refresh_and_install_models_share_the_parakeet_lease() {
        use super::super::status;

        let root = tempfile::tempdir().unwrap();
        let journal = root.path();
        let initial = status::begin_or_replace(
            journal,
            "parakeet",
            r#"{"catalog_version":"v1"}"#.to_string(),
            "test-sha".to_string(),
            None,
            "downloading",
            None,
        )
        .unwrap();
        let failed = status::transition(
            initial,
            "failed",
            Some("install_cancelled".to_string()),
            Some("install_cancelled".to_string()),
        )
        .unwrap();
        status::write_status(journal, failed).unwrap();

        let r = heal_with(journal, false, "v1", chrono::Utc::now(), || {
            panic!("cancelled")
        });
        assert_eq!(r, Ok(CoremlHeal::OwnerCancelled));
        assert!(!super::super::lease::lease_path(journal, "parakeet").exists());

        let root2 = tempfile::tempdir().unwrap();
        let journal2 = root2.path();
        let r1 = heal_with(journal2, false, "v1", chrono::Utc::now(), || {
            Err(CoremlInstallError::new(
                "download_failed",
                "network down",
                65,
            ))
        });
        assert!(matches!(r1, Ok(CoremlHeal::Failed(_))));
        let r2 = heal_with(journal2, false, "v1", chrono::Utc::now(), || {
            panic!("cooling down")
        });
        assert_eq!(r2, Ok(CoremlHeal::CoolingDown));
        assert!(!super::super::lease::is_held(journal2, "parakeet").unwrap());

        let root3 = tempfile::tempdir().unwrap();
        let journal3 = root3.path();
        let home3 = root3.path().join("home");
        fs::create_dir_all(&home3).unwrap();
        let cfg3 = config(journal3);
        let offered = row("m.bin", b"bytes");
        let r_refresh = refresh_with_rows(
            journal3,
            &home3,
            &cfg3,
            ("darwin", "arm64"),
            &[&offered],
            &mut |_, _| panic!("fetch"),
        );
        assert!(r_refresh.is_ok());
        assert!(!status::status_path(journal3, "parakeet").exists());

        let root4 = tempfile::tempdir().unwrap();
        let journal4 = root4.path();
        let home4 = root4.path().join("home");
        fs::create_dir_all(&home4).unwrap();
        let default_cache = default_parakeet_coreml_cache_dir(&home4);
        fs::create_dir_all(&default_cache).unwrap();
        fs::write(default_cache.join(".publication.json"), b"").unwrap();
        let lock_guard = hold_lock(
            default_cache.join(".publish.lock"),
            LockOptions {
                timeout: std::time::Duration::ZERO,
                poll_interval: DEFAULT_LOCK_POLL_INTERVAL,
                mode: None,
            },
        )
        .unwrap();
        let cfg4 = config(journal4);
        let r_refresh2 = refresh_with_rows(
            journal4,
            &home4,
            &cfg4,
            ("darwin", "arm64"),
            &[&offered],
            &mut |_, _| panic!("fetch"),
        );
        assert!(r_refresh2.is_ok());
        assert!(!status::status_path(journal4, "parakeet").exists());
        drop(lock_guard);

        let root5 = tempfile::tempdir().unwrap();
        let journal5 = root5.path();
        let home5 = root5.path().join("home");
        fs::create_dir_all(&home5).unwrap();
        let cfg5 = config(journal5);
        let r_heal = heal_missing_parakeet_coreml(journal5, &home5, &cfg5);
        assert!(r_heal.is_err() || matches!(r_heal, Ok(CoremlHeal::Failed(_))));
        assert!(
            super::super::lease::acquire(journal5, "parakeet")
                .unwrap()
                .is_some()
        );
        assert!(status::status_path(journal5, "parakeet").exists());
    }
}
