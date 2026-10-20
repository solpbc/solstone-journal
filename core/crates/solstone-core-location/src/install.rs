// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Downloading and removing the location-accuracy packs.
//!
//! The files come from the journal's own update origin through the same pinned
//! fetch every downloaded model uses: each one is checked against the size and
//! SHA-256 this build was made with before it is used. Every owner fetches the
//! same bytes, and nothing about location is in the request.
//!
//! A new version downloads beside the one in use, and `installed.json` is
//! written last, so an interrupted download leaves the old one working.

use std::fmt;
use std::fs;
use std::io;
use std::path::Path;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use solstone_core_assets::{RuntimeFetchQuery, catalog, mint_runtime_fetch};
use solstone_core_journal_io::{
    AtomicWriteOptions, DEFAULT_LOCK_POLL_INTERVAL, LockError, LockOptions, atomic_replace,
    hold_daily_location_admission, hold_lock,
};

use crate::accuracy::{
    INSTALLED_RECORD, Installed, InstalledFile, accuracy_root, asset_root, hold_publication,
    read_installed, release_loaded,
};

/// The catalog unit the two files belong to.
pub const UNIT: &str = "location-accuracy";
/// Room left on the disk after a download, beyond the files themselves.
const HEADROOM_BYTES: u64 = 512 * 1024 * 1024;

/// One file this build offers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OfferedFile {
    pub file: String,
    pub sha256: String,
    pub size_bytes: u64,
    pub origin_key: &'static str,
}

/// What this build offers to download.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Offered {
    pub version: String,
    pub files: Vec<OfferedFile>,
}

impl Offered {
    pub fn size_bytes(&self) -> u64 {
        self.files.iter().map(|file| file.size_bytes).sum()
    }

    /// Whether `installed` already holds exactly these files.
    pub fn is_installed(&self, installed: &Installed) -> bool {
        installed.version == self.version
            && self.files.iter().all(|offered| {
                installed.files.iter().any(|file| {
                    file.file == offered.file
                        && file.sha256 == offered.sha256
                        && file.size_bytes == offered.size_bytes
                })
            })
    }
}

/// The packs this build can download on this computer, if any.
pub fn offered() -> Option<Offered> {
    let mut files: Vec<OfferedFile> = catalog()
        .iter()
        .filter(|row| row.unit == UNIT)
        .filter(|row| {
            mint_runtime_fetch(&RuntimeFetchQuery {
                origin_key: Some(row.origin_key),
                ..RuntimeFetchQuery::default()
            })
            .is_ok()
        })
        .map(|row| OfferedFile {
            file: row.filename.to_owned(),
            sha256: row.sha256.to_owned(),
            size_bytes: row.size_bytes,
            origin_key: row.origin_key,
        })
        .collect();
    let version = catalog()
        .iter()
        .find(|row| row.unit == UNIT)
        .map(|row| row.version.to_owned())?;
    files.sort_by(|a, b| a.file.cmp(&b.file));
    (!files.is_empty()).then_some(Offered { version, files })
}

/// Why an install stopped.
#[derive(Debug)]
pub enum InstallError {
    /// This build has nothing to offer on this computer.
    NothingOffered,
    /// Another install or refresh operation is in progress.
    Busy,
    /// Not enough free space; the numbers are bytes.
    NotEnoughSpace {
        needed: u64,
        available: u64,
    },
    Download {
        file: String,
        detail: String,
    },
    Write(io::Error),
}

impl InstallError {
    /// A stable code for the map app.
    pub fn reason_code(&self) -> &'static str {
        match self {
            Self::NothingOffered => "nothing_offered",
            Self::Busy => "accuracy_busy",
            Self::NotEnoughSpace { .. } => "not_enough_space",
            Self::Download { .. } => "download_failed",
            Self::Write(_) => "write_failed",
        }
    }
}

impl fmt::Display for InstallError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NothingOffered => formatter.write_str("no location accuracy download is offered"),
            Self::Busy => formatter.write_str("another location accuracy operation is in progress"),
            Self::NotEnoughSpace { needed, available } => write!(
                formatter,
                "needs {needed} bytes free, {available} available"
            ),
            Self::Download { file, detail } => write!(formatter, "{file}: {detail}"),
            Self::Write(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for InstallError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobSource {
    Automatic,
    Explicit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccuracyJob {
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
    pub source: JobSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub received: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason_code: Option<String>,
}

pub fn accuracy_job(journal: &Path) -> Option<AccuracyJob> {
    let job_path = accuracy_root(journal).join("job.json");
    let bytes = fs::read(&job_path).ok()?;
    let job: AccuracyJob = serde_json::from_slice(&bytes).ok()?;
    if job.state == "downloading" {
        let lock_path = accuracy_root(journal).join(".worker.lock");
        if !solstone_core_journal_io::lock_is_held(&lock_path).unwrap_or(false) {
            return None;
        }
    }
    Some(job)
}

fn write_job(journal: &Path, job: &AccuracyJob) -> io::Result<()> {
    let job_path = accuracy_root(journal).join("job.json");
    let mut bytes = serde_json::to_vec_pretty(job)?;
    bytes.push(b'\n');
    atomic_replace(&job_path, &bytes, AtomicWriteOptions::default())
        .map_err(|e| io::Error::other(e.to_string()))
}

fn remove_job(journal: &Path) {
    let job_path = accuracy_root(journal).join("job.json");
    let _ = fs::remove_file(job_path);
}

#[cfg(unix)]
fn available_bytes(path: &Path) -> io::Result<u64> {
    let stat = nix::sys::statvfs::statvfs(path).map_err(io::Error::from)?;
    #[allow(
        clippy::unnecessary_cast,
        reason = "fsblkcnt_t is 32-bit on macOS and 64-bit on Linux"
    )]
    let blocks = stat.blocks_available() as u64;
    Ok(blocks.saturating_mul(stat.fragment_size()))
}

#[cfg(windows)]
fn available_bytes(path: &Path) -> io::Result<u64> {
    solstone_core_journal_io::windows_available_disk_bytes(path)
}

fn sha256_file(path: &Path) -> io::Result<String> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = std::io::Read::read(&mut file, &mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn files_match_offered(directory: &Path, offered: &Offered) -> bool {
    for file in &offered.files {
        let path = directory.join(&file.file);
        let Ok(meta) = fs::metadata(&path) else {
            return false;
        };
        if !meta.is_file() || meta.len() != file.size_bytes {
            return false;
        }
        let Ok(actual_sha) = sha256_file(&path) else {
            return false;
        };
        if actual_sha != file.sha256 {
            return false;
        }
    }
    true
}

fn generation(
    offered: &Offered,
) -> io::Result<solstone_core_artifact_download::generation::Generation> {
    use solstone_core_artifact_download::generation::{Generation, Member};
    Generation::new(
        UNIT,
        offered
            .files
            .iter()
            .map(|f| Member {
                file: f.file.clone(),
                sha256: f.sha256.clone(),
                size_bytes: f.size_bytes,
            })
            .collect(),
    )
}

fn sweep(journal: &Path, _keep: Option<&str>) {
    crate::cache::sweep(journal, offered().as_ref());
}

fn maintain(journal: &Path, offer: Option<&Offered>) -> Result<(), InstallError> {
    let _admission = hold_daily_location_admission(journal)
        .map_err(|e| InstallError::Write(io::Error::other(e)))?;
    let _publication =
        hold_publication(journal).map_err(|e| InstallError::Write(io::Error::other(e)))?;
    if let Some(installed) =
        crate::accuracy::read_installed_strict(journal).map_err(InstallError::Write)?
    {
        crate::cache::remember_legacy(journal, &installed).map_err(InstallError::Write)?;
    }
    if let (Some(mut installed), Some(offer)) = (read_installed(journal), offer)
        && installed.cache_generation.is_none()
        && offer.is_installed(&installed)
        && files_match_offered(&installed.directory(journal), offer)
    {
        let inventory = generation(offer).map_err(InstallError::Write)?;
        let target = inventory.directory(&asset_root(journal).join("generations"));
        let _hold = solstone_core_artifact_download::generation::hold_write(&target)
            .map_err(InstallError::Write)?;
        if !inventory.is_sealed(&target) {
            inventory
                .back_from(&installed.directory(journal), &target)
                .map_err(InstallError::Write)?;
        }
        installed.cache_generation = Some(inventory.key());
        atomic_replace(
            accuracy_root(journal).join(INSTALLED_RECORD),
            &serde_json::to_vec(&installed)
                .map_err(|e| InstallError::Write(io::Error::other(e)))?,
            AtomicWriteOptions::default(),
        )
        .map_err(|e| InstallError::Write(io::Error::other(e)))?;
        release_loaded();
    }
    crate::cache::sweep(journal, offer);
    Ok(())
}

/// Refresh installed accuracy packs if previously installed and catalog pins moved.
pub fn refresh_installed(journal: &Path) -> Result<(), InstallError> {
    maintain(journal, offered().as_ref())?;
    if read_installed(journal).is_none() {
        return Ok(());
    }
    let Some(offered) = offered() else {
        return Ok(());
    };
    let mut prod_fetch =
        |file: &OfferedFile, destination: &Path, file_progress: &mut dyn FnMut(u64)| {
            let handle = mint_runtime_fetch(&RuntimeFetchQuery {
                origin_key: Some(file.origin_key),
                ..RuntimeFetchQuery::default()
            })
            .map_err(|_| InstallError::NothingOffered)?;
            solstone_core_artifact_download::download_runtime_fetch(
                &handle,
                destination,
                |received, _| file_progress(received),
            )
            .map_err(|error| InstallError::Download {
                file: file.file.clone(),
                detail: error.to_string(),
            })?;
            Ok(())
        };
    refresh_installed_with_fetch(journal, &offered, &mut available_bytes, &mut prod_fetch)
}

pub fn refresh_installed_with_fetch(
    journal: &Path,
    offered: &Offered,
    available: &mut impl FnMut(&Path) -> io::Result<u64>,
    fetch: &mut impl FnMut(&OfferedFile, &Path, &mut dyn FnMut(u64)) -> Result<(), InstallError>,
) -> Result<(), InstallError> {
    maintain(journal, Some(offered))?;
    let Some(installed) = read_installed(journal) else {
        return Ok(());
    };
    if offered.is_installed(&installed) {
        return Ok(());
    }
    install_offered(
        journal,
        offered,
        JobSource::Automatic,
        available,
        |_, _| {},
        fetch,
    )?;
    Ok(())
}

/// A download admitted before its worker starts. Holding it prevents duplicate
/// workers, and the recorded removal generation fences a later owner removal.
pub struct InstallReservation {
    offered: Offered,
    worker: Worker,
}

struct Worker {
    _guard: solstone_core_journal_io::FileLock,
    removal_generation: u64,
}

fn removal_generation(journal: &Path) -> io::Result<u64> {
    match fs::read(accuracy_root(journal).join("removal-generation.json")) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(io::Error::other),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(0),
        Err(error) => Err(error),
    }
}

fn reserve_worker(
    journal: &Path,
    offered: &Offered,
    source: JobSource,
) -> Result<Option<Worker>, InstallError> {
    let root = accuracy_root(journal);
    fs::create_dir_all(&root).map_err(InstallError::Write)?;
    let guard = match hold_lock(
        root.join(".worker.lock"),
        LockOptions {
            timeout: Duration::ZERO,
            poll_interval: DEFAULT_LOCK_POLL_INTERVAL,
            mode: None,
        },
    ) {
        Ok(guard) => guard,
        Err(LockError::Timeout(_)) if source == JobSource::Automatic => return Ok(None),
        Err(LockError::Timeout(_)) => return Err(InstallError::Busy),
        Err(LockError::Io { source, .. }) => return Err(InstallError::Write(source)),
    };
    let _admission = hold_daily_location_admission(journal)
        .map_err(|error| InstallError::Write(io::Error::other(error.to_string())))?;
    if source == JobSource::Automatic && read_installed(journal).is_none() {
        return Ok(None);
    }
    let generation = removal_generation(journal).map_err(InstallError::Write)?;
    write_job(
        journal,
        &AccuracyJob {
            state: "downloading".to_owned(),
            phase: None,
            source,
            received: Some(0),
            total: Some(offered.size_bytes()),
            reason_code: None,
        },
    )
    .map_err(InstallError::Write)?;
    Ok(Some(Worker {
        _guard: guard,
        removal_generation: generation,
    }))
}

/// Reserve the existing explicit download before returning an accepted action.
pub fn reserve_install(journal: &Path) -> Result<InstallReservation, InstallError> {
    let offered = offered().ok_or(InstallError::NothingOffered)?;
    let worker =
        reserve_worker(journal, &offered, JobSource::Explicit)?.ok_or(InstallError::Busy)?;
    Ok(InstallReservation { offered, worker })
}

/// Download the reserved offer through the production origin policy.
pub fn install_reserved(
    journal: &Path,
    reservation: InstallReservation,
    mut progress: impl FnMut(u64, u64),
) -> Result<Installed, InstallError> {
    let mut fetch = |file: &OfferedFile, destination: &Path, file_progress: &mut dyn FnMut(u64)| {
        let handle = mint_runtime_fetch(&RuntimeFetchQuery {
            origin_key: Some(file.origin_key),
            ..RuntimeFetchQuery::default()
        })
        .map_err(|_| InstallError::NothingOffered)?;
        solstone_core_artifact_download::download_runtime_fetch(
            &handle,
            destination,
            |received, _| file_progress(received),
        )
        .map_err(|error| InstallError::Download {
            file: file.file.clone(),
            detail: error.to_string(),
        })?;
        Ok(())
    };
    install_reserved_offer(
        journal,
        &reservation.offered,
        JobSource::Explicit,
        &reservation.worker,
        &mut available_bytes,
        &mut progress,
        &mut fetch,
    )?
    .ok_or(InstallError::NothingOffered)
}

/// Download what [`offered`] lists. `progress` reports received and total bytes.
pub fn install(journal: &Path, progress: impl FnMut(u64, u64)) -> Result<Installed, InstallError> {
    install_reserved(journal, reserve_install(journal)?, progress)
}

fn install_offered(
    journal: &Path,
    offered: &Offered,
    source: JobSource,
    available: &mut impl FnMut(&Path) -> io::Result<u64>,
    progress: impl FnMut(u64, u64),
    fetch: &mut impl FnMut(&OfferedFile, &Path, &mut dyn FnMut(u64)) -> Result<(), InstallError>,
) -> Result<Option<Installed>, InstallError> {
    let Some(worker) = reserve_worker(journal, offered, source)? else {
        return Ok(None);
    };
    install_reserved_offer(
        journal, offered, source, &worker, available, progress, fetch,
    )
}

fn install_reserved_offer(
    journal: &Path,
    offered: &Offered,
    source: JobSource,
    worker: &Worker,
    available: &mut impl FnMut(&Path) -> io::Result<u64>,
    mut progress: impl FnMut(u64, u64),
    fetch: &mut impl FnMut(&OfferedFile, &Path, &mut dyn FnMut(u64)) -> Result<(), InstallError>,
) -> Result<Option<Installed>, InstallError> {
    let root = accuracy_root(journal);
    let canceled =
        || removal_generation(journal).map(|generation| generation != worker.removal_generation);
    let mut run = || -> Result<Option<Installed>, InstallError> {
        if canceled().map_err(InstallError::Write)? {
            return Ok(None);
        }
        let incumbent = read_installed(journal);
        let inventory = generation(offered).map_err(InstallError::Write)?;
        let generations = asset_root(journal).join("generations");
        let target = inventory.directory(&generations);
        let shared = solstone_core_artifact_download::generation::hold_use(&target)
            .map_err(InstallError::Write)?;
        let needs_publication = !inventory.is_sealed(&target);
        let _target_hold = if needs_publication {
            drop(shared);
            solstone_core_artifact_download::generation::hold_write(&target)
                .map_err(InstallError::Write)?
        } else {
            shared
        };
        let legacy = incumbent
            .as_ref()
            .filter(|installed| files_match_offered(&installed.directory(journal), offered));
        if let Some(legacy) = legacy
            && !inventory.is_sealed(&target)
        {
            inventory
                .back_from(&legacy.directory(journal), &target)
                .map_err(InstallError::Write)?;
        }
        if !inventory.is_sealed(&target) {
            let staging = asset_root(journal).join(".fetch");
            fs::create_dir_all(&staging).map_err(InstallError::Write)?;
            let remaining: u64 = offered
                .files
                .iter()
                .map(|f| {
                    if solstone_core_artifact_download::retained_prefix_bytes(
                        &target.join(&f.file),
                        &f.sha256,
                        f.size_bytes,
                    ) == f.size_bytes
                    {
                        return 0;
                    }
                    f.size_bytes.saturating_sub(
                        solstone_core_artifact_download::retained_prefix_bytes(
                            &staging.join(&f.file),
                            &f.sha256,
                            f.size_bytes,
                        ),
                    )
                })
                .sum();
            let needed = remaining.saturating_add(HEADROOM_BYTES);
            let free = available(&staging).map_err(InstallError::Write)?;
            if free < needed {
                return Err(InstallError::NotEnoughSpace {
                    needed,
                    available: free,
                });
            }
            let total = offered.size_bytes();
            let mut done = 0u64;
            let mut last_progress_write = Instant::now();
            for file in &offered.files {
                let destination = staging.join(&file.file);
                if solstone_core_artifact_download::retained_prefix_bytes(
                    &target.join(&file.file),
                    &file.sha256,
                    file.size_bytes,
                ) == file.size_bytes
                {
                    done += file.size_bytes;
                    continue;
                }
                fetch(file, &destination, &mut |received| {
                    progress(done + received, total);
                    if last_progress_write.elapsed() < Duration::from_secs(1) {
                        return;
                    }
                    if let Ok(_admission) = hold_daily_location_admission(journal)
                        && canceled().is_ok_and(|canceled| !canceled)
                    {
                        let _ = write_job(
                            journal,
                            &AccuracyJob {
                                state: "downloading".into(),
                                phase: None,
                                source,
                                received: Some(done + received),
                                total: Some(total),
                                reason_code: None,
                            },
                        );
                    }
                    last_progress_write = Instant::now();
                })?;
                if canceled().map_err(InstallError::Write)? {
                    return Ok(None);
                }
                done += file.size_bytes;
                progress(done, total);
            }
            {
                let _admission = hold_daily_location_admission(journal)
                    .map_err(|e| InstallError::Write(io::Error::other(e)))?;
                if canceled().map_err(InstallError::Write)? {
                    return Ok(None);
                }
                write_job(
                    journal,
                    &AccuracyJob {
                        state: "downloading".into(),
                        phase: Some("verifying".into()),
                        source,
                        received: Some(total),
                        total: Some(total),
                        reason_code: None,
                    },
                )
                .map_err(InstallError::Write)?;
            }
            fs::create_dir_all(&target).map_err(InstallError::Write)?;
            for file in &offered.files {
                if solstone_core_artifact_download::retained_prefix_bytes(
                    &target.join(&file.file),
                    &file.sha256,
                    file.size_bytes,
                ) == file.size_bytes
                {
                    continue;
                }
                if solstone_core_artifact_download::retained_prefix_bytes(
                    &staging.join(&file.file),
                    &file.sha256,
                    file.size_bytes,
                ) != file.size_bytes
                {
                    return Err(InstallError::Download {
                        file: file.file.clone(),
                        detail: "downloaded pack failed size or digest verification".into(),
                    });
                }
                // Keep transfer sidecars out of the immutable inventory. Moving
                // the verified member does not allocate another multi-GB copy.
                solstone_core_journal_io::install_retained_file(
                    staging.join(&file.file),
                    target.join(&file.file),
                    AtomicWriteOptions::default(),
                )
                .map_err(|e| InstallError::Write(io::Error::other(e)))?;
            }
        }
        if needs_publication {
            inventory.seal(&target).map_err(InstallError::Write)?;
        }
        let _admission = hold_daily_location_admission(journal)
            .map_err(|e| InstallError::Write(io::Error::other(e)))?;
        let _publication =
            hold_publication(journal).map_err(|e| InstallError::Write(io::Error::other(e)))?;
        if canceled().map_err(InstallError::Write)?
            || (source == JobSource::Automatic && read_installed(journal).is_none())
        {
            remove_job(journal);
            return Ok(None);
        }
        // The legacy folder stays in the shared record for frozen releases.
        // They ignore cache_generation and can reuse their original pack set.
        let folder = incumbent
            .as_ref()
            .map(|i| {
                if i.folder.is_empty() {
                    i.version.clone()
                } else {
                    i.folder.clone()
                }
            })
            .unwrap_or_else(|| offered.version.clone());
        if let Some(old) = &incumbent {
            crate::cache::remember_legacy(journal, old).map_err(InstallError::Write)?;
        }
        let new_installed = Installed {
            version: offered.version.clone(),
            folder,
            cache_generation: Some(inventory.key()),
            files: offered
                .files
                .iter()
                .map(|f| InstalledFile {
                    file: f.file.clone(),
                    sha256: f.sha256.clone(),
                    size_bytes: f.size_bytes,
                })
                .collect(),
        };
        let mut bytes = serde_json::to_vec_pretty(&new_installed)
            .map_err(|e| InstallError::Write(io::Error::other(e)))?;
        bytes.push(b'\n');
        atomic_replace(
            root.join(INSTALLED_RECORD),
            &bytes,
            AtomicWriteOptions::default(),
        )
        .map_err(|e| InstallError::Write(io::Error::other(e)))?;
        release_loaded();
        sweep(journal, None);
        remove_job(journal);
        Ok(Some(new_installed))
    };

    match run() {
        Ok(val) => Ok(val),
        Err(error) => {
            let _admission = hold_daily_location_admission(journal)
                .map_err(|error| InstallError::Write(io::Error::other(error.to_string())))?;
            if canceled().map_err(InstallError::Write)? {
                return Ok(None);
            }
            let failed_job = AccuracyJob {
                state: "failed".to_string(),
                phase: None,
                source,
                received: None,
                total: None,
                reason_code: Some(error.reason_code().to_string()),
            };
            let _ = write_job(journal, &failed_job);
            Err(error)
        }
    }
}

/// Replace `installed.json` and sweep version directories the record does not name.
///
/// The daily location admission lock is held around both steps. Callers that
/// already downloaded files call this after the download returns.
pub fn publish_installed_record(journal: &Path, installed: &Installed) -> Result<(), InstallError> {
    let _admission = hold_daily_location_admission(journal)
        .map_err(|error| InstallError::Write(io::Error::other(error.to_string())))?;
    let _publication = hold_publication(journal)
        .map_err(|error| InstallError::Write(io::Error::other(error.to_string())))?;
    if !installed.valid_paths() {
        return Err(InstallError::Write(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid installed accuracy paths",
        )));
    }
    if let Some(old) = read_installed(journal) {
        crate::cache::remember_legacy(journal, &old).map_err(InstallError::Write)?;
    }
    let root = accuracy_root(journal);
    fs::create_dir_all(&root).map_err(InstallError::Write)?;
    let mut bytes = serde_json::to_vec_pretty(installed).unwrap_or_default();
    bytes.push(b'\n');
    atomic_replace(
        root.join(INSTALLED_RECORD),
        &bytes,
        AtomicWriteOptions::default(),
    )
    .map_err(|error| InstallError::Write(io::Error::other(error.to_string())))?;
    let keep = if !installed.folder.is_empty() {
        &installed.folder
    } else {
        &installed.version
    };
    sweep(journal, Some(keep));
    release_loaded();
    Ok(())
}

/// Remove the download. Names fall back to the area tier on the next pass.
pub fn remove(journal: &Path) -> io::Result<()> {
    let _admission = hold_daily_location_admission(journal)
        .map_err(|error| io::Error::other(error.to_string()))?;
    if accuracy_job(journal)
        .is_some_and(|job| job.state == "downloading" && job.source == JobSource::Explicit)
    {
        return Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            "location accuracy is downloading",
        ));
    }
    let _publication =
        hold_publication(journal).map_err(|error| io::Error::other(error.to_string()))?;
    let generation = removal_generation(journal)?
        .checked_add(1)
        .ok_or_else(|| io::Error::other("location removal generation exhausted"))?;
    atomic_replace(
        accuracy_root(journal).join("removal-generation.json"),
        &serde_json::to_vec(&generation)?,
        AtomicWriteOptions::default(),
    )
    .map_err(|error| io::Error::other(error.to_string()))?;
    let revoked = read_installed(journal);
    if let Some(old) = &revoked {
        crate::cache::remember_legacy(journal, old)?;
    }
    release_loaded();
    remove_job(journal);
    match fs::remove_file(accuracy_root(journal).join(INSTALLED_RECORD)) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    sweep(journal, None);
    if let Some(old) = &revoked {
        crate::cache::remove_legacy(journal, old)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accuracy::installed_packs;
    use std::sync::atomic::{AtomicUsize, Ordering};
    #[cfg(feature = "full-tests")]
    use std::sync::mpsc;
    #[cfg(feature = "full-tests")]
    use std::thread;

    #[cfg(feature = "full-tests")]
    #[test]
    fn installed_record_publish_and_remove_wait_for_the_admission_guard() {
        let journal = tempfile::tempdir().unwrap();
        let root = accuracy_root(journal.path());
        fs::create_dir_all(root.join("old")).unwrap();
        fs::write(root.join("old/venues.slpk"), b"x").unwrap();
        let installed = Installed {
            version: "v9".to_owned(),
            folder: String::new(),
            cache_generation: None,
            files: vec![InstalledFile {
                file: "venues.slpk".to_owned(),
                sha256: "2d711642b726b04401627ca9fbac32f5c8530fb1903cc4db02258717921a4881"
                    .to_owned(),
                size_bytes: 1,
            }],
        };
        let guard = hold_daily_location_admission(journal.path()).unwrap();
        let path = journal.path().to_path_buf();
        let published = installed.clone();
        let (sender, receiver) = mpsc::channel();
        let (ready_sender, ready) = mpsc::channel();
        let worker = thread::spawn(move || {
            ready_sender.send(()).unwrap();
            publish_installed_record(&path, &published).unwrap();
            sender.send(()).unwrap();
        });
        ready.recv().unwrap();
        assert!(matches!(
            receiver.recv_timeout(std::time::Duration::from_millis(100)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        assert!(!root.join(INSTALLED_RECORD).exists());
        assert!(root.join("old").exists());
        drop(guard);
        receiver.recv().unwrap();
        worker.join().unwrap();
        assert!(root.join(INSTALLED_RECORD).exists());
        assert!(root.join("old").exists());

        let guard = hold_daily_location_admission(journal.path()).unwrap();
        let path = journal.path().to_path_buf();
        let (sender, receiver) = mpsc::channel();
        let (ready_sender, ready) = mpsc::channel();
        let worker = thread::spawn(move || {
            ready_sender.send(()).unwrap();
            remove(&path).unwrap();
            sender.send(()).unwrap();
        });
        ready.recv().unwrap();
        assert!(matches!(
            receiver.recv_timeout(std::time::Duration::from_millis(100)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        assert!(root.join(INSTALLED_RECORD).exists());
        drop(guard);
        receiver.recv().unwrap();
        worker.join().unwrap();
        assert!(!root.join(INSTALLED_RECORD).exists());
    }

    #[test]
    fn removing_clears_the_record_and_preserves_unknown_files() {
        let journal = tempfile::tempdir().unwrap();
        let root = accuracy_root(journal.path());
        fs::create_dir_all(root.join("v1")).unwrap();
        fs::write(root.join("v1/venues.slpk"), b"x").unwrap();
        fs::write(root.join(INSTALLED_RECORD), b"{}").unwrap();
        remove(journal.path()).unwrap();
        assert!(!root.join(INSTALLED_RECORD).exists());
        assert!(root.join("v1").exists());
        remove(journal.path()).unwrap();
    }

    #[test]
    fn this_build_offers_the_venue_file() {
        let offered = offered().expect("this build offers location accuracy");
        assert!(offered.files.iter().any(|file| file.file == "venues.slpk"));
        assert!(offered.size_bytes() > 0);
    }

    #[test]
    fn an_offer_is_installed_only_when_every_file_matches() {
        let offered = Offered {
            version: "v2".to_owned(),
            files: vec![OfferedFile {
                file: "venues.slpk".to_owned(),
                sha256: "2d711642b726b04401627ca9fbac32f5c8530fb1903cc4db02258717921a4881"
                    .to_owned(),
                size_bytes: 3,
                origin_key: "assets/location-accuracy/v2/venues.slpk",
            }],
        };
        let mut installed = Installed {
            version: "v2".to_owned(),
            folder: String::new(),
            cache_generation: None,
            files: vec![InstalledFile {
                file: "venues.slpk".to_owned(),
                sha256: "2d711642b726b04401627ca9fbac32f5c8530fb1903cc4db02258717921a4881"
                    .to_owned(),
                size_bytes: 3,
            }],
        };
        assert!(offered.is_installed(&installed));
        installed.version = "v1".to_owned();
        assert!(!offered.is_installed(&installed));
        installed.version = "v2".to_owned();
        installed.files[0].sha256 = "bb".to_owned();
        assert!(!offered.is_installed(&installed));
    }

    #[test]
    fn never_installed_and_removed_record_no_downloads_no_record() {
        let journal = tempfile::tempdir().unwrap();
        refresh_installed(journal.path()).unwrap();
        assert!(
            !accuracy_root(journal.path())
                .join(INSTALLED_RECORD)
                .exists()
        );

        let root = accuracy_root(journal.path());
        fs::create_dir_all(root.join("v1")).unwrap();
        fs::write(root.join("v1/venues.slpk"), b"x").unwrap();
        fs::write(root.join(INSTALLED_RECORD), b"{}").unwrap();
        remove(journal.path()).unwrap();
        assert!(!root.join(INSTALLED_RECORD).exists());

        refresh_installed(journal.path()).unwrap();
        assert!(!root.join(INSTALLED_RECORD).exists());
    }

    #[test]
    fn matching_offer_makes_zero_downloads() {
        let journal = tempfile::tempdir().unwrap();
        let offered = Offered {
            version: "v2".to_owned(),
            files: vec![OfferedFile {
                file: "venues.slpk".to_owned(),
                sha256: "2d711642b726b04401627ca9fbac32f5c8530fb1903cc4db02258717921a4881"
                    .to_owned(),
                size_bytes: 3,
                origin_key: "assets/location-accuracy/v2/venues.slpk",
            }],
        };
        let installed = Installed {
            version: "v2".to_owned(),
            folder: String::new(),
            cache_generation: None,
            files: vec![InstalledFile {
                file: "venues.slpk".to_owned(),
                sha256: "2d711642b726b04401627ca9fbac32f5c8530fb1903cc4db02258717921a4881"
                    .to_owned(),
                size_bytes: 3,
            }],
        };
        publish_installed_record(journal.path(), &installed).unwrap();

        let mut downloads = 0;
        let mut fetch = |_file: &OfferedFile, _dest: &Path, _progress: &mut dyn FnMut(u64)| {
            downloads += 1;
            Ok(())
        };
        refresh_installed_with_fetch(
            journal.path(),
            &offered,
            &mut |_| Ok(10 * 1024 * 1024 * 1024),
            &mut fetch,
        )
        .unwrap();
        assert_eq!(downloads, 0);
    }

    #[test]
    fn live_files_matching_offer_with_behind_record_adopts_without_downloads() {
        let journal = tempfile::tempdir().unwrap();
        let root = accuracy_root(journal.path());
        let dir = root.join("v2");
        fs::create_dir_all(&dir).unwrap();
        let content = b"live venue data";
        let target_file = dir.join("venues.slpk");
        fs::write(&target_file, content).unwrap();
        let sha = sha256_file(&target_file).unwrap();

        let offered = Offered {
            version: "v2".to_owned(),
            files: vec![OfferedFile {
                file: "venues.slpk".to_owned(),
                sha256: sha.clone(),
                size_bytes: content.len() as u64,
                origin_key: "assets/location-accuracy/v2/venues.slpk",
            }],
        };
        let installed = Installed {
            version: "v2".to_owned(),
            folder: String::new(),
            cache_generation: None,
            files: vec![InstalledFile {
                file: "venues.slpk".to_owned(),
                sha256: "0".repeat(64),
                size_bytes: content.len() as u64,
            }],
        };
        publish_installed_record(journal.path(), &installed).unwrap();

        let mut downloads = 0;
        let mut fetch = |_file: &OfferedFile, _dest: &Path, _progress: &mut dyn FnMut(u64)| {
            downloads += 1;
            Ok(())
        };
        refresh_installed_with_fetch(
            journal.path(),
            &offered,
            &mut |_| Ok(10 * 1024 * 1024 * 1024),
            &mut fetch,
        )
        .unwrap();
        assert_eq!(downloads, 0);

        let updated = read_installed(journal.path()).unwrap();
        assert_eq!(updated.files[0].sha256, sha);
    }

    #[test]
    fn same_version_failure_on_second_file_keeps_pre_call_live_and_record() {
        let journal = tempfile::tempdir().unwrap();
        let root = accuracy_root(journal.path());
        let dir = root.join("v2");
        fs::create_dir_all(&dir).unwrap();
        let file1 = dir.join("f1.bin");
        let file2 = dir.join("f2.bin");
        fs::write(&file1, b"initial1").unwrap();
        fs::write(&file2, b"initial2").unwrap();

        let installed = Installed {
            version: "v2".to_owned(),
            folder: String::new(),
            cache_generation: None,
            files: vec![
                InstalledFile {
                    file: "f1.bin".to_owned(),
                    sha256: "d7c25c67b3f46c44d589ed345b787a15494bf827e57571063d330bf04877e0b9"
                        .to_owned(),
                    size_bytes: 8,
                },
                InstalledFile {
                    file: "f2.bin".to_owned(),
                    sha256: "498e0513748e47f13db570dd2017d46a27230fced471bd9c66ed20567568bde4"
                        .to_owned(),
                    size_bytes: 8,
                },
            ],
        };
        publish_installed_record(journal.path(), &installed).unwrap();
        let record_bytes_before = fs::read(root.join(INSTALLED_RECORD)).unwrap();

        let offered = Offered {
            version: "v2".to_owned(),
            files: vec![
                OfferedFile {
                    file: "f1.bin".to_owned(),
                    sha256: "6039092f70864b935e614eaf0ff41bcb91357e8e746997f9af55f5b9f7f8f684"
                        .to_owned(),
                    size_bytes: 4,
                    origin_key: "k1",
                },
                OfferedFile {
                    file: "f2.bin".to_owned(),
                    sha256: "d504f01608b7a8b04349a9ccf4630f4917e67cc4eb6803da31ed503e04a00c68"
                        .to_owned(),
                    size_bytes: 4,
                    origin_key: "k2",
                },
            ],
        };

        let mut fetch = |file: &OfferedFile, dest: &Path, _progress: &mut dyn FnMut(u64)| {
            if file.file == "f1.bin" {
                fs::write(dest, b"new1").unwrap();
                Ok(())
            } else {
                Err(InstallError::Download {
                    file: file.file.clone(),
                    detail: "network interrupted".to_owned(),
                })
            }
        };

        let result = refresh_installed_with_fetch(
            journal.path(),
            &offered,
            &mut |_| Ok(10 * 1024 * 1024 * 1024),
            &mut fetch,
        );
        assert!(result.is_err());

        assert_eq!(fs::read(&file1).unwrap(), b"initial1");
        assert_eq!(fs::read(&file2).unwrap(), b"initial2");
        assert_eq!(
            fs::read(root.join(INSTALLED_RECORD)).unwrap(),
            record_bytes_before
        );
    }

    #[test]
    fn next_call_writing_all_files_succeeds_and_cleans_staging_and_aside() {
        let journal = tempfile::tempdir().unwrap();
        let root = accuracy_root(journal.path());
        let dir = root.join("v2");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("f1.bin"), b"initial1").unwrap();
        fs::write(dir.join("f2.bin"), b"initial2").unwrap();

        let installed = Installed {
            version: "v2".to_owned(),
            folder: String::new(),
            cache_generation: None,
            files: vec![
                InstalledFile {
                    file: "f1.bin".to_owned(),
                    sha256: "d7c25c67b3f46c44d589ed345b787a15494bf827e57571063d330bf04877e0b9"
                        .to_owned(),
                    size_bytes: 8,
                },
                InstalledFile {
                    file: "f2.bin".to_owned(),
                    sha256: "498e0513748e47f13db570dd2017d46a27230fced471bd9c66ed20567568bde4"
                        .to_owned(),
                    size_bytes: 8,
                },
            ],
        };
        publish_installed_record(journal.path(), &installed).unwrap();

        let offered = Offered {
            version: "v2".to_owned(),
            files: vec![
                OfferedFile {
                    file: "f1.bin".to_owned(),
                    sha256: "6039092f70864b935e614eaf0ff41bcb91357e8e746997f9af55f5b9f7f8f684"
                        .to_owned(),
                    size_bytes: 4,
                    origin_key: "k1",
                },
                OfferedFile {
                    file: "f2.bin".to_owned(),
                    sha256: "d504f01608b7a8b04349a9ccf4630f4917e67cc4eb6803da31ed503e04a00c68"
                        .to_owned(),
                    size_bytes: 4,
                    origin_key: "k2",
                },
            ],
        };

        let mut fetch = |file: &OfferedFile, dest: &Path, _progress: &mut dyn FnMut(u64)| {
            if file.file == "f1.bin" {
                fs::write(dest, b"new1").unwrap();
            } else {
                fs::write(dest, b"new2").unwrap();
            }
            Ok(())
        };

        refresh_installed_with_fetch(
            journal.path(),
            &offered,
            &mut |_| Ok(10 * 1024 * 1024 * 1024),
            &mut fetch,
        )
        .unwrap();

        let updated = read_installed(journal.path()).unwrap();
        let target_dir = updated.directory(journal.path());
        assert_eq!(fs::read(target_dir.join("f1.bin")).unwrap(), b"new1");
        assert_eq!(fs::read(target_dir.join("f2.bin")).unwrap(), b"new2");
        assert_eq!(
            updated.files[0].sha256,
            "6039092f70864b935e614eaf0ff41bcb91357e8e746997f9af55f5b9f7f8f684"
        );
        assert_eq!(
            updated.files[1].sha256,
            "d504f01608b7a8b04349a9ccf4630f4917e67cc4eb6803da31ed503e04a00c68"
        );

        for entry in fs::read_dir(&root).unwrap().flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            assert!(
                name == updated.folder
                    || name == updated.version
                    || name == INSTALLED_RECORD
                    || name == ".worker.lock"
                    || name == ".worker.lock.lock"
                    || name == ".publication.lock"
                    || name == ".publication.lock.lock",
                "unexpected artifact left: {name}"
            );
        }
    }

    #[test]
    fn refresh_commit_after_record_deleted_does_not_recreate_record() {
        let journal = tempfile::tempdir().unwrap();
        let root = accuracy_root(journal.path());
        let dir = root.join("v2");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("venues.slpk"), b"v2 content").unwrap();

        let installed = Installed {
            version: "v2".to_owned(),
            folder: String::new(),
            cache_generation: None,
            files: vec![InstalledFile {
                file: "venues.slpk".to_owned(),
                sha256: "f5fe596ea3d308b881c55f3a49a8b4a3e24c625b70417b7ef36991d48e8d2948"
                    .to_owned(),
                size_bytes: 10,
            }],
        };
        publish_installed_record(journal.path(), &installed).unwrap();

        let offered = Offered {
            version: "v2".to_owned(),
            files: vec![OfferedFile {
                file: "venues.slpk".to_owned(),
                sha256: "3ac6c03ee14dfa66846d2cc860324888fceecaed43f4e73fe924b36ebda0f1a5"
                    .to_owned(),
                size_bytes: 4,
                origin_key: "k",
            }],
        };

        let mut fetch = |_file: &OfferedFile, dest: &Path, _progress: &mut dyn FnMut(u64)| {
            let _ = fs::remove_file(accuracy_root(journal.path()).join(INSTALLED_RECORD));
            fs::write(dest, b"new4").unwrap();
            Ok(())
        };

        let result = install_offered(
            journal.path(),
            &offered,
            JobSource::Automatic,
            &mut |_| Ok(10 * 1024 * 1024 * 1024),
            |_, _| {},
            &mut fetch,
        )
        .unwrap();
        assert!(result.is_none());
        assert!(!root.join(INSTALLED_RECORD).exists());
        assert_eq!(fs::read(dir.join("venues.slpk")).unwrap(), b"v2 content");
    }

    #[test]
    fn worker_lock_held_makes_zero_downloads() {
        let journal = tempfile::tempdir().unwrap();
        let root = accuracy_root(journal.path());
        fs::create_dir_all(&root).unwrap();

        let installed = Installed {
            version: "v2".to_owned(),
            folder: String::new(),
            cache_generation: None,
            files: vec![InstalledFile {
                file: "venues.slpk".to_owned(),
                sha256: "1".repeat(64),
                size_bytes: 3,
            }],
        };
        publish_installed_record(journal.path(), &installed).unwrap();

        let lock_path = root.join(".worker.lock");
        let _guard = hold_lock(&lock_path, LockOptions::default()).unwrap();

        let offered = Offered {
            version: "v2".to_owned(),
            files: vec![OfferedFile {
                file: "venues.slpk".to_owned(),
                sha256: "2d711642b726b04401627ca9fbac32f5c8530fb1903cc4db02258717921a4881"
                    .to_owned(),
                size_bytes: 3,
                origin_key: "k",
            }],
        };

        let downloads = AtomicUsize::new(0);
        let mut fetch = |_file: &OfferedFile, _dest: &Path, _progress: &mut dyn FnMut(u64)| {
            downloads.fetch_add(1, Ordering::SeqCst);
            Ok(())
        };

        refresh_installed_with_fetch(
            journal.path(),
            &offered,
            &mut |_| Ok(10 * 1024 * 1024 * 1024),
            &mut fetch,
        )
        .unwrap();
        assert_eq!(downloads.load(Ordering::SeqCst), 0);

        let result = install_offered(
            journal.path(),
            &offered,
            JobSource::Explicit,
            &mut |_| Ok(10 * 1024 * 1024 * 1024),
            |_, _| {},
            &mut fetch,
        );
        assert!(matches!(result, Err(InstallError::Busy)));
        assert_eq!(downloads.load(Ordering::SeqCst), 0);
    }

    fn setup_fixtures(journal: &Path, version: &str) -> (Offered, Installed) {
        let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/accuracy");
        let venues_bytes = fs::read(source.join("venues.slpk")).unwrap();
        let containment_bytes = fs::read(source.join("containment.slpk")).unwrap();
        let venues_sha = format!("{:x}", Sha256::digest(&venues_bytes));
        let containment_sha = format!("{:x}", Sha256::digest(&containment_bytes));

        let root = accuracy_root(journal);
        let dir = root.join(version);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("venues.slpk"), &venues_bytes).unwrap();
        fs::write(dir.join("containment.slpk"), &containment_bytes).unwrap();

        let installed = Installed {
            version: version.to_owned(),
            folder: String::new(),
            cache_generation: None,
            files: vec![
                InstalledFile {
                    file: "containment.slpk".to_owned(),
                    sha256: containment_sha.clone(),
                    size_bytes: containment_bytes.len() as u64,
                },
                InstalledFile {
                    file: "venues.slpk".to_owned(),
                    sha256: venues_sha.clone(),
                    size_bytes: venues_bytes.len() as u64,
                },
            ],
        };
        publish_installed_record(journal, &installed).unwrap();

        let offered = Offered {
            version: version.to_owned(),
            files: vec![
                OfferedFile {
                    file: "containment.slpk".to_owned(),
                    sha256: containment_sha,
                    size_bytes: containment_bytes.len() as u64,
                    origin_key: "assets/location-accuracy/containment.slpk",
                },
                OfferedFile {
                    file: "venues.slpk".to_owned(),
                    sha256: venues_sha,
                    size_bytes: venues_bytes.len() as u64,
                    origin_key: "assets/location-accuracy/venues.slpk",
                },
            ],
        };

        (offered, installed)
    }

    #[test]
    fn changed_version_and_same_version_changed_digest_refresh_fetch_err_preserves_packs_and_bytes()
    {
        // Changed version
        let journal = tempfile::tempdir().unwrap();
        let (_offered, _installed) = setup_fixtures(journal.path(), "v1");
        let root = accuracy_root(journal.path());
        let orig_venues = fs::read(root.join("v1/venues.slpk")).unwrap();
        let orig_containment = fs::read(root.join("v1/containment.slpk")).unwrap();

        let mut changed_version_offer = setup_fixtures(journal.path(), "v1").0;
        changed_version_offer.version = "v2".to_owned();
        changed_version_offer.files[0].sha256 = "2".repeat(64);

        let mut failing_fetch =
            |_file: &OfferedFile, _dest: &Path, _progress: &mut dyn FnMut(u64)| {
                Err(InstallError::Download {
                    file: "venues.slpk".to_string(),
                    detail: "failed".to_string(),
                })
            };
        let res = refresh_installed_with_fetch(
            journal.path(),
            &changed_version_offer,
            &mut |_| Ok(10 * 1024 * 1024 * 1024),
            &mut failing_fetch,
        );
        assert!(res.is_err());
        let packs = installed_packs(journal.path()).unwrap();
        assert_eq!(packs.identity(), "venues:fixture-1;containment:fixture-1");
        assert!(packs.has_venues() && packs.has_containment());
        assert_eq!(fs::read(root.join("v1/venues.slpk")).unwrap(), orig_venues);
        assert_eq!(
            fs::read(root.join("v1/containment.slpk")).unwrap(),
            orig_containment
        );
        assert_eq!(read_installed(journal.path()).unwrap().version, "v1");

        // Same version changed digest
        let mut same_version_offer = setup_fixtures(journal.path(), "v1").0;
        same_version_offer.files[0].sha256 = "1".repeat(64);
        let res2 = refresh_installed_with_fetch(
            journal.path(),
            &same_version_offer,
            &mut |_| Ok(10 * 1024 * 1024 * 1024),
            &mut failing_fetch,
        );
        assert!(res2.is_err());
        let packs2 = installed_packs(journal.path()).unwrap();
        assert_eq!(packs2.identity(), "venues:fixture-1;containment:fixture-1");
        assert!(packs2.has_venues() && packs2.has_containment());
        assert_eq!(fs::read(root.join("v1/venues.slpk")).unwrap(), orig_venues);
        assert_eq!(
            fs::read(root.join("v1/containment.slpk")).unwrap(),
            orig_containment
        );
        assert_eq!(read_installed(journal.path()).unwrap().version, "v1");
    }

    #[test]
    fn zero_download_adoption_opens_both_roles() {
        let journal = tempfile::tempdir().unwrap();
        let (offered, mut installed) = setup_fixtures(journal.path(), "v1");
        // Corrupt hashes in installed.json to simulate an older record
        installed.files[0].sha256 = "3".repeat(64);
        installed.files[1].sha256 = "4".repeat(64);
        publish_installed_record(journal.path(), &installed).unwrap();

        let mut fetch = |_file: &OfferedFile,
                         _dest: &Path,
                         _progress: &mut dyn FnMut(u64)|
         -> Result<(), InstallError> {
            panic!("fetch must not be called during adoption");
        };
        refresh_installed_with_fetch(
            journal.path(),
            &offered,
            &mut |_| Ok(10 * 1024 * 1024 * 1024),
            &mut fetch,
        )
        .unwrap();

        let packs = installed_packs(journal.path()).unwrap();
        assert!(packs.has_venues() && packs.has_containment());
        assert_eq!(packs.identity(), "venues:fixture-1;containment:fixture-1");
    }

    #[test]
    fn unreferenced_new_tree_present_keeps_incumbent_packs() {
        let journal = tempfile::tempdir().unwrap();
        let (offered, _installed) = setup_fixtures(journal.path(), "v1");
        let root = accuracy_root(journal.path());
        let unref_dir = root.join("v2");
        fs::create_dir_all(&unref_dir).unwrap();
        fs::write(unref_dir.join("venues.slpk"), b"garbage-venues").unwrap();
        fs::write(unref_dir.join("containment.slpk"), b"garbage-containment").unwrap();

        let mut fetch =
            |_: &OfferedFile, _: &Path, _: &mut dyn FnMut(u64)| -> Result<(), InstallError> {
                panic!("fetch must not be called when incumbent bytes match");
            };
        refresh_installed_with_fetch(
            journal.path(),
            &offered,
            &mut |_| Ok(10 * 1024 * 1024 * 1024),
            &mut fetch,
        )
        .unwrap();

        let packs = installed_packs(journal.path()).unwrap();
        assert_eq!(packs.identity(), "venues:fixture-1;containment:fixture-1");
        assert!(packs.has_venues() && packs.has_containment());
        assert_eq!(read_installed(journal.path()).unwrap().version, "v1");
    }

    fn replacement_fixture(file: &str) -> Vec<u8> {
        let mut bytes = fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("fixtures/accuracy")
                .join(file),
        )
        .unwrap();
        let header_length = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
        let header = &mut bytes[12..12 + header_length];
        let at = header
            .windows(b"fixture-1".len())
            .position(|part| part == b"fixture-1")
            .unwrap();
        header[at..at + b"fixture-2".len()].copy_from_slice(b"fixture-2");
        assert_eq!(
            crate::pack::Pack::from_bytes(bytes.clone())
                .unwrap()
                .version(),
            "fixture-2"
        );
        bytes
    }

    #[test]
    fn changed_version_and_same_version_retry_keep_both_incumbent_roles_until_commit() {
        for version in ["v1", "v2"] {
            let journal = tempfile::tempdir().unwrap();
            let (mut offered, incumbent) = setup_fixtures(journal.path(), "v1");
            let before = fs::read(accuracy_root(journal.path()).join(INSTALLED_RECORD)).unwrap();
            let incumbent_words = installed_packs(journal.path())
                .unwrap()
                .stop(39.7527, -105.0003, Some(95.0))
                .unwrap();
            offered.version = version.to_owned();
            for file in &mut offered.files {
                let bytes = replacement_fixture(&file.file);
                file.sha256 = format!("{:x}", Sha256::digest(&bytes));
                file.size_bytes = bytes.len() as u64;
            }
            // A process may have published a complete unreferenced tree before
            // dying. It must neither hide the incumbent nor strand the retry.
            let orphan = accuracy_root(journal.path()).join(format!("{version}.orphan"));
            fs::create_dir_all(&orphan).unwrap();
            for file in &offered.files {
                fs::write(orphan.join(&file.file), replacement_fixture(&file.file)).unwrap();
            }
            if version == "v2" {
                let canonical_orphan = accuracy_root(journal.path()).join(version);
                fs::create_dir_all(&canonical_orphan).unwrap();
                fs::write(
                    canonical_orphan.join("venues.slpk"),
                    b"interrupted generation",
                )
                .unwrap();
            }
            let mut requests = 0;
            let mut fail_later = |file: &OfferedFile, dest: &Path, _: &mut dyn FnMut(u64)| {
                requests += 1;
                if requests == 2 {
                    return Err(InstallError::Download {
                        file: file.file.clone(),
                        detail: "offline".to_owned(),
                    });
                }
                fs::write(dest, replacement_fixture(&file.file)).map_err(InstallError::Write)
            };
            assert!(
                refresh_installed_with_fetch(
                    journal.path(),
                    &offered,
                    &mut |_| Ok(u64::MAX),
                    &mut fail_later
                )
                .is_err()
            );
            assert_eq!(requests, 2);
            assert_eq!(
                fs::read(accuracy_root(journal.path()).join(INSTALLED_RECORD)).unwrap(),
                before
            );
            let packs = installed_packs(journal.path()).unwrap();
            assert!(packs.has_venues() && packs.has_containment());
            assert_eq!(packs.identity(), "venues:fixture-1;containment:fixture-1");
            assert_eq!(
                packs.stop(39.7527, -105.0003, Some(95.0)).unwrap(),
                incumbent_words
            );
            for file in &incumbent.files {
                assert_eq!(
                    sha256_file(&incumbent.directory(journal.path()).join(&file.file)).unwrap(),
                    file.sha256
                );
            }
            let mut fetch = |file: &OfferedFile, dest: &Path, _: &mut dyn FnMut(u64)| {
                fs::write(dest, replacement_fixture(&file.file)).map_err(InstallError::Write)
            };
            refresh_installed_with_fetch(
                journal.path(),
                &offered,
                &mut |_| Ok(u64::MAX),
                &mut fetch,
            )
            .unwrap();
            let installed = read_installed(journal.path()).unwrap();
            assert!(offered.is_installed(&installed));
            let packs = installed_packs(journal.path()).unwrap();
            assert!(packs.has_venues() && packs.has_containment());
            assert_eq!(packs.identity(), "venues:fixture-2;containment:fixture-2");
            assert_eq!(
                packs.stop(39.7527, -105.0003, Some(95.0)).unwrap(),
                incumbent_words
            );
            for file in &offered.files {
                assert_eq!(
                    sha256_file(&installed.directory(journal.path()).join(&file.file)).unwrap(),
                    file.sha256
                );
            }
            refresh_installed_with_fetch(
                journal.path(),
                &offered,
                &mut |_| panic!("no disk observation needed"),
                &mut |_, _, _| panic!("matching pins must not fetch"),
            )
            .unwrap();
        }
    }

    #[test]
    fn owner_removal_inside_paused_fetch_stays_removed_without_late_status() {
        for source in [JobSource::Automatic] {
            for fail_after_removal in [false, true] {
                let journal = tempfile::tempdir().unwrap();
                let (mut offered, _) = setup_fixtures(journal.path(), "v1");
                offered.version = "v2".to_owned();
                for file in &mut offered.files {
                    let bytes = replacement_fixture(&file.file);
                    file.sha256 = format!("{:x}", Sha256::digest(&bytes));
                    file.size_bytes = bytes.len() as u64;
                }
                let worker = reserve_worker(journal.path(), &offered, source)
                    .unwrap()
                    .unwrap();
                let (entered_tx, entered_rx) = std::sync::mpsc::channel();
                let (release_tx, release_rx) = std::sync::mpsc::channel();
                let path = journal.path().to_path_buf();
                let task = std::thread::spawn(move || {
                    let mut count = 0;
                    let mut fetch =
                        |file: &OfferedFile, dest: &Path, progress: &mut dyn FnMut(u64)| {
                            count += 1;
                            assert_eq!(count, 1, "removal must stop before a second fetch");
                            entered_tx.send(()).unwrap();
                            release_rx.recv().unwrap();
                            progress(1);
                            if fail_after_removal {
                                return Err(InstallError::Download {
                                    file: file.file.clone(),
                                    detail: "staging removed during fetch".to_owned(),
                                });
                            }
                            fs::create_dir_all(dest.parent().unwrap()).unwrap();
                            fs::write(dest, b"late bytes").map_err(InstallError::Write)
                        };
                    install_reserved_offer(
                        &path,
                        &offered,
                        source,
                        &worker,
                        &mut |_| Ok(u64::MAX),
                        |_, _| {},
                        &mut fetch,
                    )
                });
                entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                assert_eq!(accuracy_job(journal.path()).unwrap().state, "downloading");
                remove(journal.path()).unwrap();
                release_tx.send(()).unwrap();
                assert!(task.join().unwrap().unwrap().is_none());
                assert!(read_installed(journal.path()).is_none());
                assert!(installed_packs(journal.path()).is_none());
                assert!(accuracy_job(journal.path()).is_none());
            }
        }
    }

    #[test]
    fn explicit_reservation_is_busy_before_worker_start_and_releases_after_worker_loss() {
        let journal = tempfile::tempdir().unwrap();
        let (offered, _) = setup_fixtures(journal.path(), "v1");
        let worker = reserve_worker(journal.path(), &offered, JobSource::Explicit)
            .unwrap()
            .unwrap();
        assert_eq!(
            remove(journal.path()).unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
        assert!(read_installed(journal.path()).is_some());
        drop(worker);
        assert!(accuracy_job(journal.path()).is_none());
        remove(journal.path()).unwrap();
        assert!(read_installed(journal.path()).is_none());
    }

    #[test]
    fn automatic_fetch_failure_writes_job_and_subsequent_success_deletes_it() {
        let journal = tempfile::tempdir().unwrap();
        let (offered, _installed) = setup_fixtures(journal.path(), "v1");
        let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/accuracy");

        let mut new_offered = offered;
        new_offered.version = "v2".to_owned();
        let changed_name = new_offered.files[0].file.clone();
        let mut changed_bytes = fs::read(source.join(&changed_name)).unwrap();
        let last = changed_bytes.len() - 1;
        changed_bytes[last] ^= 1;
        new_offered.files[0].sha256 = format!("{:x}", Sha256::digest(&changed_bytes));

        let mut failing_fetch =
            |_file: &OfferedFile, _dest: &Path, _progress: &mut dyn FnMut(u64)| {
                Err(InstallError::Download {
                    file: "venues.slpk".to_string(),
                    detail: "error".to_string(),
                })
            };
        let err = refresh_installed_with_fetch(
            journal.path(),
            &new_offered,
            &mut |_| Ok(10 * 1024 * 1024 * 1024),
            &mut failing_fetch,
        );
        assert!(err.is_err());

        let job = accuracy_job(journal.path()).unwrap();
        assert_eq!(job.state, "failed");
        assert_eq!(job.source, JobSource::Automatic);
        assert_eq!(job.reason_code.as_deref(), Some("download_failed"));

        // Installed record remains
        assert_eq!(read_installed(journal.path()).unwrap().version, "v1");

        // Succeeding refresh deletes job.json
        let mut ok_fetch = |file: &OfferedFile, dest: &Path, _progress: &mut dyn FnMut(u64)| {
            if file.file == changed_name {
                fs::write(dest, &changed_bytes).unwrap();
            } else {
                fs::copy(source.join(&file.file), dest).unwrap();
            }
            Ok(())
        };
        refresh_installed_with_fetch(
            journal.path(),
            &new_offered,
            &mut |_| Ok(10 * 1024 * 1024 * 1024),
            &mut ok_fetch,
        )
        .unwrap();

        assert!(accuracy_job(journal.path()).is_none());
        assert!(!accuracy_root(journal.path()).join("job.json").exists());
    }

    #[test]
    fn held_lock_preserves_downloading_job_for_other_source() {
        let journal = tempfile::tempdir().unwrap();
        let (offered, _installed) = setup_fixtures(journal.path(), "v1");
        let root = accuracy_root(journal.path());
        let mut new_offered = offered;
        new_offered.version = "v2".to_owned();

        // 7a: Explicit downloading job, refresh lock timeout does not write or change it
        let explicit_job = AccuracyJob {
            state: "downloading".to_string(),
            phase: None,
            source: JobSource::Explicit,
            received: Some(10),
            total: Some(100),
            reason_code: None,
        };
        write_job(journal.path(), &explicit_job).unwrap();
        let lock_path = root.join(".worker.lock");
        let guard = hold_lock(&lock_path, LockOptions::default()).unwrap();

        let mut fetch = |_: &OfferedFile, _: &Path, _: &mut dyn FnMut(u64)| Ok(());
        let res = refresh_installed_with_fetch(
            journal.path(),
            &new_offered,
            &mut |_| Ok(10 * 1024 * 1024 * 1024),
            &mut fetch,
        );
        assert!(res.is_ok());
        let current_job = accuracy_job(journal.path()).unwrap();
        assert_eq!(current_job.state, "downloading");
        assert_eq!(current_job.source, JobSource::Explicit);
        assert_eq!(current_job.received, Some(10));
        drop(guard);

        // 7b: Automatic downloading job, explicit install returns Busy and does not write or change it
        let auto_job = AccuracyJob {
            state: "downloading".to_string(),
            phase: None,
            source: JobSource::Automatic,
            received: Some(20),
            total: Some(200),
            reason_code: None,
        };
        write_job(journal.path(), &auto_job).unwrap();
        let guard2 = hold_lock(&lock_path, LockOptions::default()).unwrap();

        let res2 = install_offered(
            journal.path(),
            &new_offered,
            JobSource::Explicit,
            &mut |_| Ok(10 * 1024 * 1024 * 1024),
            |_, _| {},
            &mut fetch,
        );
        assert!(matches!(res2, Err(InstallError::Busy)));
        let current_job2 = accuracy_job(journal.path()).unwrap();
        assert_eq!(current_job2.state, "downloading");
        assert_eq!(current_job2.source, JobSource::Automatic);
        assert_eq!(current_job2.received, Some(20));
        drop(guard2);
    }

    #[test]
    fn disk_observer_zero_returns_not_enough_space_and_no_fetch() {
        let journal = tempfile::tempdir().unwrap();
        let (offered, _installed) = setup_fixtures(journal.path(), "v1");
        let mut new_offered = offered;
        new_offered.version = "v2".to_owned();
        new_offered.files[0].sha256 = "0".repeat(64);

        let mut fetch =
            |_: &OfferedFile, _: &Path, _: &mut dyn FnMut(u64)| -> Result<(), InstallError> {
                panic!("fetch must not be called when disk space is 0");
            };
        let res = install_offered(
            journal.path(),
            &new_offered,
            JobSource::Explicit,
            &mut |_| Ok(0),
            |_, _| {},
            &mut fetch,
        );
        assert!(matches!(
            res,
            Err(InstallError::NotEnoughSpace {
                needed: _,
                available: 0
            })
        ));
    }
}

#[cfg(test)]
#[path = "install_generation_tests.rs"]
mod generation_tests;
