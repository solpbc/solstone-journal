// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Dedicated `/app/export/` workspace API: async sizing, background live-journal
//! preparation, rolling 24-hour inactivity retention, Trash permanent-erasure
//! invalidation, requester-scoped access, and resumable byte-range downloads.

#![allow(
    clippy::disallowed_methods,
    reason = "export service creates and removes its own private staged archives strictly outside the journal root"
)]

mod range;
mod staging;
#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::fmt::Write as _;
use std::fs::OpenOptions;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::body::Body;
use axum::extract::{Path as AxumPath, Query, State};
use axum::http::{HeaderMap, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Extension, Json, Router};
use jiff::Timestamp;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use solstone_core_convey_http::identity::AccessBasis;
use solstone_core_journal_archive::{
    ArchiveError, ArchiveSource, EncodeArchiveError, EncodeArchiveRequest, EncodeOptions,
    EncodeProgress, EstimateProgress, ExportBreakdown, ExportMode, encode_archive_with_options,
    evaluate_lighter_export,
};
use solstone_core_journal_config::owner_zone;
use solstone_core_journal_io::disk_space::JournalDiskSpace;
use solstone_core_trash::read_erasure_epoch;

use self::range::{
    ActiveDownloadGuard, ActiveTransferHooks, RangeDecision, TrackedRangeStream, evaluate_range,
};
use self::staging::{
    ChangeFingerprint, ReadyRecord, RequesterScope, StagingArea, required_staging_bytes,
};
use crate::asset_response;

/// Rolling inactivity lifetime for a prepared export copy (24 hours).
pub(crate) const ROLLING_INACTIVITY_SECS: i64 = 24 * 60 * 60;

/// Cached estimate freshness window (5 minutes).
const ESTIMATE_CACHE_TTL_SECS: i64 = 5 * 60;

/// Wall-clock abstraction so 24-hour rolling expiry and >24-hour continuous
/// downloads are tested deterministically without real waits.
pub(crate) trait ExportClock: Send + Sync + 'static {
    fn now_epoch_secs(&self) -> i64;
}

/// Real system wall clock.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct SystemExportClock;

impl ExportClock for SystemExportClock {
    fn now_epoch_secs(&self) -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0)
    }
}

/// Optional disk-space provider override for deterministic disk-pressure tests.
pub(crate) trait DiskSpaceProbe: Send + Sync + 'static {
    fn read(&self, staging: &StagingArea) -> std::io::Result<JournalDiskSpace>;
}

struct RealDiskSpaceProbe;

impl DiskSpaceProbe for RealDiskSpaceProbe {
    fn read(&self, staging: &StagingArea) -> std::io::Result<JournalDiskSpace> {
        staging.disk_space()
    }
}

/// Shared coordinator for one journal's `/app/export/` service.
#[derive(Clone)]
pub(crate) struct ExportService {
    inner: Arc<ExportServiceInner>,
}

struct ExportServiceInner {
    journal_root: PathBuf,
    staging: StagingArea,
    clock: Arc<dyn ExportClock>,
    disk_probe: Arc<dyn DiskSpaceProbe>,
    state: Mutex<CoordinatorState>,
}

#[derive(Default)]
struct CoordinatorState {
    estimate: EstimateSlot,
    job: Option<ActiveJob>,
    last_terminal: Option<TerminalOutcome>,
    ready: Option<ReadyEntry>,
}

#[derive(Clone, Debug, Default)]
struct EstimateSlot {
    status: EstimateStatus,
    measured_at_epoch_secs: Option<i64>,
    progress: EstimateProgress,
    breakdown: Option<ExportBreakdown>,
    error: Option<String>,
    running: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum EstimateStatus {
    #[default]
    Idle,
    Measuring,
    Ready,
    Error,
}

struct ActiveJob {
    job_id: String,
    mode: ExportMode,
    owner: RequesterScope,
    started_at_epoch_secs: i64,
    phase: &'static str,
    files_done: u64,
    files_total: u64,
    bytes_read: u64,
    bytes_total: u64,
    cancel: Arc<AtomicBool>,
}

#[derive(Clone, Debug)]
struct TerminalOutcome {
    owner: RequesterScope,
    status: &'static str,
    job_id: Option<String>,
    mode: Option<ExportMode>,
    error_code: Option<&'static str>,
    error_detail: Option<String>,
    required_bytes: Option<u64>,
    available_bytes: Option<u64>,
}

struct ReadyEntry {
    record: ReadyRecord,
    active_downloads: usize,
    retired_by_erasure: bool,
    retired_by_revocation: bool,
}

impl ExportService {
    /// Create a production export service for `journal_root`, sweeping orphaned
    /// `.part` files and recovering any still-valid prepared copy on disk.
    pub(crate) fn new(journal_root: PathBuf) -> Self {
        let staging = StagingArea::resolve(&journal_root).unwrap_or_else(|_| StagingArea {
            dir: std::env::temp_dir().join("solstone-export-staging-fallback"),
            same_volume_as_journal: false,
        });
        Self::with_deps(
            journal_root,
            staging,
            Arc::new(SystemExportClock),
            Arc::new(RealDiskSpaceProbe),
        )
    }

    pub(crate) fn with_deps(
        journal_root: PathBuf,
        staging: StagingArea,
        clock: Arc<dyn ExportClock>,
        disk_probe: Arc<dyn DiskSpaceProbe>,
    ) -> Self {
        let mut initial_state = CoordinatorState::default();
        let now = clock.now_epoch_secs();
        if let Some(record) = staging.read_ready_record() {
            let epoch_matches =
                read_erasure_epoch(&journal_root).is_ok_and(|epoch| epoch == record.erasure_epoch);
            let within_window =
                now.saturating_sub(record.last_activity_epoch_secs) < ROLLING_INACTIVITY_SECS;
            let authorized = is_requester_authorized(&journal_root, &record.owner);
            if epoch_matches && within_window && authorized {
                let keep = record.zip_filename.clone();
                staging.sweep_orphans(Some(&keep));
                initial_state.ready = Some(ReadyEntry {
                    record,
                    active_downloads: 0,
                    retired_by_erasure: false,
                    retired_by_revocation: false,
                });
            } else {
                staging.clear_ready_files(Some(&record));
            }
        } else {
            staging.sweep_orphans(None);
        }

        let inner = Arc::new(ExportServiceInner {
            journal_root,
            staging,
            clock,
            disk_probe,
            state: Mutex::new(initial_state),
        });
        let service = Self { inner };
        service.spawn_reaper();
        service
    }

    fn spawn_reaper(&self) {
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let weak = Arc::downgrade(&self.inner);
        handle.spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(60));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                interval.tick().await;
                let Some(inner) = weak.upgrade() else {
                    break;
                };
                let service = ExportService { inner };
                service.reconcile_ready_state();
            }
        });
    }

    /// Reconcile prepared-copy liveness against Trash erasure epoch, pairing
    /// revocation, and rolling 24-hour inactivity.
    pub(crate) fn reconcile_ready_state(&self) {
        let now = self.inner.clock.now_epoch_secs();
        let current_epoch = read_erasure_epoch(&self.inner.journal_root);
        let mut state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
        let Some(ready) = state.ready.as_mut() else {
            return;
        };

        // 1. Check permanent Trash erasure epoch (fail closed on unreadable epoch).
        let epoch_mismatch = match &current_epoch {
            Ok(epoch) => *epoch != ready.record.erasure_epoch,
            Err(_) => true,
        };
        if epoch_mismatch {
            ready.retired_by_erasure = true;
        }

        // 2. Check paired device revocation.
        if !is_requester_authorized(&self.inner.journal_root, &ready.record.owner) {
            ready.retired_by_revocation = true;
        }

        // 3. Check 24-hour rolling inactivity ONLY when no transfer is active.
        let idle_expired = ready.active_downloads == 0
            && now.saturating_sub(ready.record.last_activity_epoch_secs) >= ROLLING_INACTIVITY_SECS;

        let should_delete = ready.active_downloads == 0
            && (ready.retired_by_erasure || ready.retired_by_revocation || idle_expired);

        if should_delete {
            let removed = state.ready.take().expect("ready entry present");
            self.inner.staging.clear_ready_files(Some(&removed.record));
            if removed.retired_by_erasure {
                state.last_terminal = Some(TerminalOutcome {
                    owner: removed.record.owner,
                    status: "erased_invalidated",
                    job_id: None,
                    mode: Some(removed.record.mode),
                    error_code: Some("export_invalidated_by_erasure"),
                    error_detail: None,
                    required_bytes: None,
                    available_bytes: None,
                });
            } else if idle_expired {
                state.last_terminal = Some(TerminalOutcome {
                    owner: removed.record.owner,
                    status: "expired",
                    job_id: None,
                    mode: Some(removed.record.mode),
                    error_code: Some("export_expired"),
                    error_detail: None,
                    required_bytes: None,
                    available_bytes: None,
                });
            }
        }
    }

    fn trigger_estimate_if_needed(&self, force: bool) {
        let now = self.inner.clock.now_epoch_secs();
        let should_spawn = {
            let mut state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
            if state.estimate.running {
                false
            } else {
                let stale = force
                    || state.estimate.status == EstimateStatus::Idle
                    || state.estimate.status == EstimateStatus::Error
                    || state
                        .estimate
                        .measured_at_epoch_secs
                        .is_none_or(|t| now.saturating_sub(t) >= ESTIMATE_CACHE_TTL_SECS);
                if stale {
                    state.estimate.running = true;
                    state.estimate.status = EstimateStatus::Measuring;
                    state.estimate.error = None;
                    true
                } else {
                    false
                }
            }
        };
        if !should_spawn {
            return;
        }
        let inner = Arc::clone(&self.inner);
        tokio::task::spawn_blocking(move || {
            let outcome = (|| -> Result<ExportBreakdown, String> {
                let source = ArchiveSource::open_live(&inner.journal_root)
                    .map_err(|error| error.to_string())?;
                let plan = evaluate_lighter_export(
                    &inner.journal_root,
                    source.inventory(),
                    None,
                    None,
                    |progress| {
                        let mut state = inner.state.lock().unwrap_or_else(|e| e.into_inner());
                        state.estimate.progress = progress;
                    },
                )
                .map_err(|_| "estimate cancelled".to_owned())?;
                Ok(plan.breakdown)
            })();
            let mut state = inner.state.lock().unwrap_or_else(|e| e.into_inner());
            state.estimate.running = false;
            match outcome {
                Ok(breakdown) => {
                    state.estimate.status = EstimateStatus::Ready;
                    state.estimate.breakdown = Some(breakdown);
                    state.estimate.measured_at_epoch_secs = Some(inner.clock.now_epoch_secs());
                    state.estimate.error = None;
                }
                Err(error) => {
                    state.estimate.status = EstimateStatus::Error;
                    state.estimate.error = Some(error);
                }
            }
        });
    }

    fn estimate_payload(&self) -> Value {
        let disk = self.inner.disk_probe.read(&self.inner.staging).ok();
        let free_bytes = disk.map(|d| d.free_bytes);
        let total_bytes = disk.map(|d| d.total_bytes);
        let backup = read_backup_summary(&self.inner.journal_root);
        let host_os = if cfg!(target_os = "macos") {
            "macos"
        } else if cfg!(windows) {
            "windows"
        } else {
            "linux"
        };

        let slot = {
            let state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
            state.estimate.clone()
        };

        let status = match slot.status {
            EstimateStatus::Idle | EstimateStatus::Measuring => "measuring",
            EstimateStatus::Ready => "ready",
            EstimateStatus::Error => "error",
        };
        let (full_req, light_req, full_fits, light_fits) = match slot.breakdown {
            Some(ref b) => {
                let fr = required_staging_bytes(b.full_bytes);
                let lr = required_staging_bytes(b.light_bytes);
                (
                    Some(fr),
                    Some(lr),
                    free_bytes.map(|f| f >= fr),
                    free_bytes.map(|f| f >= lr),
                )
            }
            None => (None, None, None, None),
        };

        let days_counted = ChangeFingerprint::capture(&self.inner.journal_root).chronicle_days;
        json!({
            "status": status,
            "is_estimate": true,
            "measured_at": slot.measured_at_epoch_secs.and_then(format_epoch_utc),
            "days_counted": days_counted,
            "progress": slot.progress,
            "breakdown": slot.breakdown,
            "error": slot.error,
            "journal_disk_free_bytes": free_bytes,
            "journal_disk_total_bytes": total_bytes,
            "staging_volume_same_as_journal": self.inner.staging.same_volume_as_journal,
            "full_required_bytes": full_req,
            "light_required_bytes": light_req,
            "full_fits": full_fits,
            "light_fits": light_fits,
            "backup": backup,
            "journal_host_os": host_os,
            "cli_command_full": cli_command_for_host(host_os, false),
            "cli_command_light": cli_command_for_host(host_os, true),
        })
    }

    fn state_payload(&self, requester: &RequesterScope) -> Value {
        self.reconcile_ready_state();
        let now = self.inner.clock.now_epoch_secs();
        let current_fp = ChangeFingerprint::capture(&self.inner.journal_root);
        let state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());

        if let Some(job) = &state.job {
            if &job.owner == requester {
                return json!({
                    "status": "preparing",
                    "other_requester_busy": false,
                    "active_job": {
                        "job_id": job.job_id,
                        "mode": job.mode.as_str(),
                        "phase": job.phase,
                        "files_done": job.files_done,
                        "files_total": job.files_total,
                        "bytes_read": job.bytes_read,
                        "bytes_total": job.bytes_total,
                        "started_at": format_epoch_utc(job.started_at_epoch_secs),
                    },
                    "ready": Value::Null,
                });
            }
            return json!({
                "status": "idle",
                "other_requester_busy": true,
                "active_job": Value::Null,
                "ready": Value::Null,
            });
        }

        if let Some(ready) = &state.ready {
            if &ready.record.owner == requester
                && !ready.retired_by_erasure
                && !ready.retired_by_revocation
            {
                let expires_at = ready
                    .record
                    .last_activity_epoch_secs
                    .saturating_add(ROLLING_INACTIVITY_SECS);
                let remaining_secs = expires_at.saturating_sub(now).max(0);
                let contents_changed_since = current_fp != ready.record.fingerprint;
                let export_id = &ready.record.export_id;
                return json!({
                    "status": "ready",
                    "other_requester_busy": false,
                    "active_job": Value::Null,
                    "ready": {
                        "export_id": export_id,
                        "mode": ready.record.mode.as_str(),
                        "filename": ready.record.filename,
                        "size_bytes": ready.record.size_bytes,
                        "sha256": ready.record.sha256,
                        "etag": ready.record.strong_etag(),
                        "created_at": format_epoch_utc(ready.record.created_at_epoch_secs),
                        "last_activity_at": format_epoch_utc(ready.record.last_activity_epoch_secs),
                        "expires_at": format_epoch_utc(expires_at),
                        "expires_in_seconds": remaining_secs,
                        "active_downloads": ready.active_downloads,
                        "contents_changed_since": contents_changed_since,
                        "is_estimated_size": false,
                        "days_count": ready.record.days_count,
                        "files_count": ready.record.files_count,
                        "omitted_media_files": ready.record.omitted_media_files,
                        "omitted_media_bytes": ready.record.omitted_media_bytes,
                        "offloaded_days": ready.record.offloaded_days,
                        "offloaded_bytes": ready.record.offloaded_bytes,
                        "offload_degraded": ready.record.offload_degraded,
                        "download_path": format!("/app/export/api/ready/{export_id}/download"),
                        "legacy_download_path": format!("/app/import/api/journal-archive/export?id={export_id}"),
                    },
                });
            }
            if &ready.record.owner != requester {
                return json!({
                    "status": "idle",
                    "other_requester_busy": ready.active_downloads > 0,
                    "active_job": Value::Null,
                    "ready": Value::Null,
                });
            }
        }

        if let Some(term) = &state.last_terminal
            && &term.owner == requester
        {
            return json!({
                "status": term.status,
                "other_requester_busy": false,
                "active_job": {
                    "job_id": term.job_id,
                    "mode": term.mode.map(ExportMode::as_str),
                    "error_code": term.error_code,
                    "error_detail": term.error_detail,
                    "required_bytes": term.required_bytes,
                    "available_bytes": term.available_bytes,
                },
                "ready": Value::Null,
            });
        }

        json!({
            "status": "idle",
            "other_requester_busy": false,
            "active_job": Value::Null,
            "ready": Value::Null,
        })
    }

    fn start_job(
        &self,
        requester: RequesterScope,
        mode: ExportMode,
        replace: bool,
    ) -> Result<(StatusCode, Value), (StatusCode, Value)> {
        self.reconcile_ready_state();

        let disk = self.inner.disk_probe.read(&self.inner.staging).ok();
        let free_bytes = disk.map(|d| d.free_bytes);

        let (job_id, cancel) = {
            let mut state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());

            // Check running job.
            if let Some(job) = &state.job {
                if job.owner == requester && job.mode == mode {
                    return Ok((
                        StatusCode::ACCEPTED,
                        json!({
                            "status": "preparing",
                            "job_id": job.job_id,
                            "mode": job.mode.as_str(),
                        }),
                    ));
                }
                return Err((
                    StatusCode::CONFLICT,
                    json!({
                        "code": "export_busy",
                        "detail": "another export is already preparing",
                    }),
                ));
            }

            // Check existing ready copy.
            if let Some(ready) = &state.ready {
                if ready.active_downloads > 0 {
                    return Err((
                        StatusCode::CONFLICT,
                        json!({
                            "code": "active_download_in_progress",
                            "detail": "a prepared copy is currently downloading",
                        }),
                    ));
                }
                if ready.record.owner == requester
                    && ready.record.mode == mode
                    && !replace
                    && !ready.retired_by_erasure
                    && !ready.retired_by_revocation
                {
                    let export_id = ready.record.export_id.clone();
                    return Ok((
                        StatusCode::OK,
                        json!({
                            "status": "ready",
                            "export_id": export_id,
                            "mode": ready.record.mode.as_str(),
                        }),
                    ));
                }
            }

            // Preflight staging disk space against cached breakdown if available.
            if let (Some(free), Some(breakdown)) = (free_bytes, state.estimate.breakdown) {
                let est = match mode {
                    ExportMode::Full => breakdown.full_bytes,
                    ExportMode::WithoutProcessedMedia => breakdown.light_bytes,
                };
                let required = required_staging_bytes(est);
                if free < required {
                    let light_req = required_staging_bytes(breakdown.light_bytes);
                    return Err((
                        StatusCode::CONFLICT,
                        json!({
                            "code": "insufficient_journal_space",
                            "required_bytes": required,
                            "available_bytes": free,
                            "light_estimated_bytes": breakdown.light_bytes,
                            "light_required_bytes": light_req,
                            "light_fits": free >= light_req,
                        }),
                    ));
                }
            }

            // Clear any idle prior ready copy before claiming the single slot.
            if let Some(old) = state.ready.take() {
                self.inner.staging.clear_ready_files(Some(&old.record));
            }
            state.last_terminal = None;

            let job_id = mint_id("exp");
            let cancel = Arc::new(AtomicBool::new(false));
            state.job = Some(ActiveJob {
                job_id: job_id.clone(),
                mode,
                owner: requester.clone(),
                started_at_epoch_secs: self.inner.clock.now_epoch_secs(),
                phase: "scanning",
                files_done: 0,
                files_total: 0,
                bytes_read: 0,
                bytes_total: 0,
                cancel: Arc::clone(&cancel),
            });
            (job_id, cancel)
        };

        let inner = Arc::clone(&self.inner);
        let worker_job_id = job_id.clone();
        tokio::task::spawn_blocking(move || {
            run_prepare_worker(inner, worker_job_id, requester, mode, cancel);
        });

        Ok((
            StatusCode::ACCEPTED,
            json!({
                "status": "preparing",
                "job_id": job_id,
                "mode": mode.as_str(),
            }),
        ))
    }

    fn cancel_job(&self, requester: &RequesterScope, job_id: &str) -> StatusCode {
        let state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
        let Some(job) = &state.job else {
            return StatusCode::NOT_FOUND;
        };
        if &job.owner != requester || job.job_id != job_id {
            return StatusCode::NOT_FOUND;
        }
        job.cancel.store(true, Ordering::Release);
        StatusCode::ACCEPTED
    }

    fn discard_ready(
        &self,
        requester: &RequesterScope,
        export_id: &str,
        force: bool,
    ) -> Result<StatusCode, (StatusCode, Value)> {
        self.reconcile_ready_state();
        let mut state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
        let Some(ready) = &state.ready else {
            return Err((StatusCode::NOT_FOUND, json!({"code": "export_not_found"})));
        };
        if &ready.record.owner != requester || ready.record.export_id != export_id {
            return Err((StatusCode::NOT_FOUND, json!({"code": "export_not_found"})));
        }
        if ready.active_downloads > 0 && !force {
            return Err((
                StatusCode::CONFLICT,
                json!({
                    "code": "active_download_in_progress",
                    "active_downloads": ready.active_downloads,
                }),
            ));
        }
        let removed = state.ready.take().expect("ready entry present");
        self.inner.staging.clear_ready_files(Some(&removed.record));
        state.last_terminal = None;
        Ok(StatusCode::OK)
    }

    /// Serve `GET` or `HEAD` for a prepared export copy.
    pub(crate) async fn serve_download(
        &self,
        requester: &RequesterScope,
        export_id: &str,
        method: &Method,
        headers: &HeaderMap,
    ) -> Response {
        self.reconcile_ready_state();

        let (record, zip_path, guard) = {
            let mut state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
            let Some(ready) = state.ready.as_mut() else {
                if let Some(term) = &state.last_terminal
                    && &term.owner == requester
                    && matches!(
                        term.error_code,
                        Some("export_invalidated_by_erasure") | Some("export_expired")
                    )
                {
                    return (
                        StatusCode::GONE,
                        Json(json!({ "code": term.error_code.unwrap_or("export_expired") })),
                    )
                        .into_response();
                }
                return (
                    StatusCode::NOT_FOUND,
                    Json(json!({ "code": "export_not_found" })),
                )
                    .into_response();
            };

            if &ready.record.owner != requester || ready.record.export_id != export_id {
                return (
                    StatusCode::NOT_FOUND,
                    Json(json!({ "code": "export_not_found" })),
                )
                    .into_response();
            }
            if ready.retired_by_erasure {
                return (
                    StatusCode::GONE,
                    Json(json!({ "code": "export_invalidated_by_erasure" })),
                )
                    .into_response();
            }
            if ready.retired_by_revocation {
                return (
                    StatusCode::FORBIDDEN,
                    Json(json!({ "code": "export_requester_revoked" })),
                )
                    .into_response();
            }

            // Reset rolling 24-hour inactivity clock on every download request.
            let now = self.inner.clock.now_epoch_secs();
            ready.record.last_activity_epoch_secs = now;
            let _ = self.inner.staging.write_ready_record(&ready.record);

            let record = ready.record.clone();
            let zip_path = record.zip_path(&self.inner.staging.dir);
            let guard = if *method == Method::HEAD {
                None
            } else {
                ready.active_downloads = ready.active_downloads.saturating_add(1);
                let hooks: Arc<dyn ActiveTransferHooks> = Arc::new(ExportTransferHooks {
                    inner: Arc::clone(&self.inner),
                    export_id: record.export_id.clone(),
                    owner: record.owner.clone(),
                });
                Some(ActiveDownloadGuard::new(hooks))
            };
            (record, zip_path, guard)
        };

        let strong_etag = record.strong_etag();
        let total_len = record.size_bytes;
        let decision = evaluate_range(headers, total_len, &strong_etag);

        let disposition = format!("attachment; filename=\"{}\"", record.filename);
        let apply_common_headers = |builder: axum::http::response::Builder| {
            builder
                .header(header::CONTENT_TYPE, "application/zip")
                .header(header::CONTENT_DISPOSITION, disposition.as_str())
                .header(header::ACCEPT_RANGES, "bytes")
                .header(header::ETAG, strong_etag.as_str())
                .header(header::CACHE_CONTROL, "no-store")
                .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
                .header("x-solstone-export-sha256", record.sha256.as_str())
        };

        let (status, start, length, content_range) = match decision {
            RangeDecision::Unsatisfiable => {
                drop(guard);
                return apply_common_headers(
                    Response::builder().status(StatusCode::RANGE_NOT_SATISFIABLE),
                )
                .header(header::CONTENT_RANGE, format!("bytes */{total_len}"))
                .header(header::CONTENT_LENGTH, "0")
                .body(Body::empty())
                .expect("416 response builds");
            }
            RangeDecision::Full => (StatusCode::OK, 0, total_len, None),
            RangeDecision::Partial { start, end } => (
                StatusCode::PARTIAL_CONTENT,
                start,
                end.saturating_sub(start).saturating_add(1),
                Some(format!("bytes {start}-{end}/{total_len}")),
            ),
        };

        if *method == Method::HEAD {
            let mut builder = apply_common_headers(Response::builder().status(status))
                .header(header::CONTENT_LENGTH, length.to_string());
            if let Some(cr) = content_range {
                builder = builder.header(header::CONTENT_RANGE, cr);
            }
            return builder.body(Body::empty()).expect("HEAD response builds");
        }

        let file = match tokio::fs::File::open(&zip_path).await {
            Ok(file) => file,
            Err(_) => {
                drop(guard);
                return (StatusCode::GONE, Json(json!({ "code": "export_missing" })))
                    .into_response();
            }
        };

        let stream = match TrackedRangeStream::open(
            file,
            start,
            length,
            guard.expect("GET holds active download guard"),
        )
        .await
        {
            Ok(stream) => stream,
            Err(_) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({ "code": "export_read_failed" })),
                )
                    .into_response();
            }
        };

        let mut builder = apply_common_headers(Response::builder().status(status))
            .header(header::CONTENT_LENGTH, length.to_string());
        if let Some(cr) = content_range {
            builder = builder.header(header::CONTENT_RANGE, cr);
        }
        builder
            .body(Body::from_stream(stream))
            .expect("export download response builds")
    }
}

struct ExportTransferHooks {
    inner: Arc<ExportServiceInner>,
    export_id: String,
    owner: RequesterScope,
}

impl ActiveTransferHooks for ExportTransferHooks {
    fn touch_activity(&self) {
        let now = self.inner.clock.now_epoch_secs();
        let mut state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(ready) = state.ready.as_mut()
            && ready.record.export_id == self.export_id
        {
            ready.record.last_activity_epoch_secs = now;
            let _ = self.inner.staging.write_ready_record(&ready.record);
        }
    }

    fn is_revoked(&self) -> bool {
        !is_requester_authorized(&self.inner.journal_root, &self.owner)
    }

    fn release_active_download(&self) {
        let current_epoch = read_erasure_epoch(&self.inner.journal_root);
        let now = self.inner.clock.now_epoch_secs();
        let mut state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
        let Some(ready) = state.ready.as_mut() else {
            return;
        };
        if ready.record.export_id != self.export_id {
            return;
        }
        ready.active_downloads = ready.active_downloads.saturating_sub(1);
        ready.record.last_activity_epoch_secs = now;
        let _ = self.inner.staging.write_ready_record(&ready.record);

        let epoch_mismatch = match &current_epoch {
            Ok(epoch) => *epoch != ready.record.erasure_epoch,
            Err(_) => true,
        };
        if epoch_mismatch {
            ready.retired_by_erasure = true;
        }
        if !is_requester_authorized(&self.inner.journal_root, &ready.record.owner) {
            ready.retired_by_revocation = true;
        }
        if ready.active_downloads == 0 && (ready.retired_by_erasure || ready.retired_by_revocation)
        {
            let removed = state.ready.take().expect("ready present");
            self.inner.staging.clear_ready_files(Some(&removed.record));
            if removed.retired_by_erasure {
                state.last_terminal = Some(TerminalOutcome {
                    owner: removed.record.owner,
                    status: "erased_invalidated",
                    job_id: None,
                    mode: Some(removed.record.mode),
                    error_code: Some("export_invalidated_by_erasure"),
                    error_detail: None,
                    required_bytes: None,
                    available_bytes: None,
                });
            }
        }
    }
}

fn run_prepare_worker(
    inner: Arc<ExportServiceInner>,
    job_id: String,
    owner: RequesterScope,
    mode: ExportMode,
    cancel: Arc<AtomicBool>,
) {
    let part_path = inner.staging.dir.join(format!("{job_id}.zip.part"));

    let finish_with_error = |status: &'static str,
                             error_code: &'static str,
                             detail: Option<String>,
                             required_bytes: Option<u64>,
                             available_bytes: Option<u64>| {
        let _ = std::fs::remove_file(&part_path);
        let mut state = inner.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.job.as_ref().is_some_and(|j| j.job_id == job_id) {
            state.job = None;
        }
        state.last_terminal = Some(TerminalOutcome {
            owner: owner.clone(),
            status,
            job_id: Some(job_id.clone()),
            mode: Some(mode),
            error_code: Some(error_code),
            error_detail: detail,
            required_bytes,
            available_bytes,
        });
    };

    // 1. Capture Trash permanent-erasure epoch before scanning (fail closed!).
    let start_epoch = match read_erasure_epoch(&inner.journal_root) {
        Ok(epoch) => epoch,
        Err(error) => {
            finish_with_error(
                "failed",
                "prepare_failed",
                Some(error.to_string()),
                None,
                None,
            );
            return;
        }
    };
    let fingerprint = ChangeFingerprint::capture(&inner.journal_root);

    // 2. Open live-journal source snapshot.
    let source = match ArchiveSource::open_live(&inner.journal_root) {
        Ok(source) => source,
        Err(error) => {
            if !read_erasure_epoch(&inner.journal_root).is_ok_and(|epoch| epoch == start_epoch) {
                finish_with_error(
                    "erased_invalidated",
                    "export_invalidated_by_erasure",
                    None,
                    None,
                    None,
                );
            } else if matches!(error, ArchiveError::SourceChanged { .. }) {
                finish_with_error("failed", "source_changed", None, None, None);
            } else {
                finish_with_error(
                    "failed",
                    "prepare_failed",
                    Some(error.to_string()),
                    None,
                    None,
                );
            }
            return;
        }
    };

    if cancel.load(Ordering::Acquire) {
        finish_with_error("cancelled", "cancelled", None, None, None);
        return;
    }

    // 3. Evaluate offload grouped eligibility if lighter mode is requested.
    let plan = match evaluate_lighter_export(
        &inner.journal_root,
        source.inventory(),
        None,
        Some(cancel.as_ref()),
        |_| {},
    ) {
        Ok(plan) => plan,
        Err(_) => {
            finish_with_error("cancelled", "cancelled", None, None, None);
            return;
        }
    };
    let planned_bytes = match mode {
        ExportMode::Full => plan.breakdown.full_bytes,
        ExportMode::WithoutProcessedMedia => plan.breakdown.light_bytes,
    };

    // Update cached estimate breakdown opportunistically with exact walk counts.
    {
        let mut state = inner.state.lock().unwrap_or_else(|e| e.into_inner());
        state.estimate.breakdown = Some(plan.breakdown);
        state.estimate.status = EstimateStatus::Ready;
        state.estimate.measured_at_epoch_secs = Some(inner.clock.now_epoch_secs());
    }

    // 4. Re-check actual staging volume disk space before writing bytes.
    if let Ok(space) = inner.disk_probe.read(&inner.staging) {
        let required = required_staging_bytes(planned_bytes);
        if space.free_bytes < required {
            finish_with_error(
                "failed",
                "insufficient_journal_space",
                None,
                Some(required),
                Some(space.free_bytes),
            );
            return;
        }
    }

    // 5. Create private 0o600 staging `.part` file and encode ZIP.
    let mut part_file = match create_private_stage_file(&part_path) {
        Ok(file) => file,
        Err(error) => {
            let code = if is_no_space(&error) {
                "insufficient_journal_space"
            } else {
                "prepare_failed"
            };
            finish_with_error("failed", code, Some(error.to_string()), None, None);
            return;
        }
    };

    {
        let mut state = inner.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(job) = state.job.as_mut()
            && job.job_id == job_id
        {
            job.phase = "packing";
        }
    }

    let now_secs = inner.clock.now_epoch_secs();
    let exported_at = format_epoch_utc(now_secs).unwrap_or_else(|| "2026-01-01T00:00:00Z".into());
    let request = EncodeArchiveRequest {
        source: &source,
        solstone_version: env!("CARGO_PKG_VERSION"),
        exported_at: &exported_at,
        day_window: None,
    };
    let omitted = match mode {
        ExportMode::Full => None,
        ExportMode::WithoutProcessedMedia => Some(&plan.omitted_members),
    };

    let progress_inner = Arc::clone(&inner);
    let progress_job_id = job_id.clone();
    let mut on_progress = |progress: EncodeProgress| {
        let mut state = progress_inner
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(job) = state.job.as_mut()
            && job.job_id == progress_job_id
        {
            job.files_done = progress.files_done;
            job.files_total = progress.files_total;
            job.bytes_read = progress.bytes_read;
            job.bytes_total = progress.bytes_total;
        }
    };

    let abort_journal = inner.journal_root.clone();
    let abort_epoch = start_epoch.clone();
    let abort_owner = owner.clone();
    let mut check_abort = || -> Result<(), String> {
        match read_erasure_epoch(&abort_journal) {
            Ok(current) if current == abort_epoch => {}
            _ => return Err("trash_erased".to_owned()),
        }
        if !is_requester_authorized(&abort_journal, &abort_owner) {
            return Err("requester_revoked".to_owned());
        }
        Ok(())
    };

    let encode_result = encode_archive_with_options(
        &request,
        &mut part_file,
        EncodeOptions {
            omitted_members: omitted,
            cancel: Some(cancel.as_ref()),
            on_progress: Some(&mut on_progress),
            check_abort: Some(&mut check_abort),
        },
    );

    if let Err(error) = encode_result {
        drop(part_file);
        if !read_erasure_epoch(&inner.journal_root).is_ok_and(|epoch| epoch == start_epoch) {
            finish_with_error(
                "erased_invalidated",
                "export_invalidated_by_erasure",
                None,
                None,
                None,
            );
            return;
        }
        match error {
            EncodeArchiveError::Cancelled => {
                finish_with_error("cancelled", "cancelled", None, None, None);
            }
            EncodeArchiveError::Aborted(ref reason) if reason == "trash_erased" => {
                finish_with_error(
                    "erased_invalidated",
                    "export_invalidated_by_erasure",
                    None,
                    None,
                    None,
                );
            }
            EncodeArchiveError::Aborted(ref reason) if reason == "requester_revoked" => {
                finish_with_error("cancelled", "export_requester_revoked", None, None, None);
            }
            EncodeArchiveError::Source(ArchiveError::SourceChanged { .. }) => {
                finish_with_error("failed", "source_changed", None, None, None);
            }
            EncodeArchiveError::ArchiveWrite { ref source, .. }
            | EncodeArchiveError::ArchiveFinish { ref source, .. }
                if is_no_space(source) =>
            {
                finish_with_error(
                    "failed",
                    "insufficient_journal_space",
                    Some(source.to_string()),
                    None,
                    None,
                );
            }
            other => {
                finish_with_error(
                    "failed",
                    "prepare_failed",
                    Some(other.to_string()),
                    None,
                    None,
                );
            }
        }
        return;
    }

    if let Err(error) = part_file.sync_all() {
        drop(part_file);
        let code = if is_no_space(&error) {
            "insufficient_journal_space"
        } else {
            "prepare_failed"
        };
        finish_with_error("failed", code, Some(error.to_string()), None, None);
        return;
    }
    drop(part_file);

    // 6. Compute exact SHA-256 and final byte size over the immutable `.part` file.
    {
        let mut state = inner.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(job) = state.job.as_mut()
            && job.job_id == job_id
        {
            job.phase = "verifying";
        }
    }

    let (size_bytes, sha256) = match hash_file(&part_path, &cancel) {
        Ok(pair) => pair,
        Err(HashError::Cancelled) => {
            finish_with_error("cancelled", "cancelled", None, None, None);
            return;
        }
        Err(HashError::Io(error)) => {
            finish_with_error(
                "failed",
                "prepare_failed",
                Some(error.to_string()),
                None,
                None,
            );
            return;
        }
    };

    // 7. Final atomic gate BEFORE publishing: verify cancel, Trash erasure epoch,
    //    and requester authorization one last time so an obsolete job can NEVER
    //    publish erased material.
    let export_id = job_id.clone();
    let zip_filename = format!("{export_id}.zip");
    let final_zip_path = inner.staging.dir.join(&zip_filename);
    let date_stamp = owner_date_ymd(&inner.journal_root, inner.clock.now_epoch_secs());
    let filename = match mode {
        ExportMode::Full => format!("solstone-journal-{date_stamp}.zip"),
        ExportMode::WithoutProcessedMedia => format!("solstone-journal-{date_stamp}-light.zip"),
    };

    let mut state = inner.state.lock().unwrap_or_else(|e| e.into_inner());
    let epoch_still_valid =
        read_erasure_epoch(&inner.journal_root).is_ok_and(|epoch| epoch == start_epoch);
    if cancel.load(Ordering::Acquire) {
        drop(state);
        finish_with_error("cancelled", "cancelled", None, None, None);
        return;
    }
    if !epoch_still_valid {
        drop(state);
        finish_with_error(
            "erased_invalidated",
            "export_invalidated_by_erasure",
            None,
            None,
            None,
        );
        return;
    }
    if !is_requester_authorized(&inner.journal_root, &owner) {
        drop(state);
        finish_with_error("cancelled", "export_requester_revoked", None, None, None);
        return;
    }

    if let Err(error) = std::fs::rename(&part_path, &final_zip_path) {
        drop(state);
        finish_with_error(
            "failed",
            "prepare_failed",
            Some(error.to_string()),
            None,
            None,
        );
        return;
    }

    let now = inner.clock.now_epoch_secs();
    let (files_count, omitted_media_files, omitted_media_bytes) = match mode {
        ExportMode::Full => (plan.breakdown.full_files, 0, 0),
        ExportMode::WithoutProcessedMedia => (
            plan.breakdown.light_files,
            plan.breakdown.processed_media_omitted.files,
            plan.breakdown.processed_media_omitted.bytes,
        ),
    };
    let days_count = fingerprint.chronicle_days as u64;
    let record = ReadyRecord {
        schema: ReadyRecord::SCHEMA,
        export_id,
        mode,
        filename,
        zip_filename,
        size_bytes,
        sha256,
        created_at_epoch_secs: now,
        last_activity_epoch_secs: now,
        owner: owner.clone(),
        erasure_epoch: start_epoch,
        fingerprint,
        days_count,
        files_count,
        omitted_media_files,
        omitted_media_bytes,
        offloaded_days: plan.breakdown.already_offloaded.days,
        offloaded_bytes: plan.breakdown.already_offloaded.bytes,
        offload_degraded: plan.breakdown.already_offloaded.degraded,
    };
    if let Err(error) = inner.staging.write_ready_record(&record) {
        let _ = std::fs::remove_file(&final_zip_path);
        drop(state);
        finish_with_error(
            "failed",
            "prepare_failed",
            Some(error.to_string()),
            None,
            None,
        );
        return;
    }

    state.job = None;
    state.last_terminal = None;
    state.ready = Some(ReadyEntry {
        record,
        active_downloads: 0,
        retired_by_erasure: false,
        retired_by_revocation: false,
    });
}

enum HashError {
    Cancelled,
    Io(std::io::Error),
}

fn hash_file(path: &Path, cancel: &AtomicBool) -> Result<(u64, String), HashError> {
    let mut file = std::fs::File::open(path).map_err(HashError::Io)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 256 * 1024];
    let mut total = 0u64;
    loop {
        if cancel.load(Ordering::Acquire) {
            return Err(HashError::Cancelled);
        }
        let n = file.read(&mut buf).map_err(HashError::Io)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        total = total.saturating_add(n as u64);
    }
    Ok((total, format!("{:x}", hasher.finalize())))
}

fn create_private_stage_file(path: &Path) -> std::io::Result<std::fs::File> {
    let mut opts = OpenOptions::new();
    opts.create_new(true).read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    opts.open(path)
}

fn is_no_space(error: &std::io::Error) -> bool {
    if error.kind() == std::io::ErrorKind::StorageFull {
        return true;
    }
    #[cfg(unix)]
    if error.raw_os_error() == Some(nix::libc::ENOSPC) {
        return true;
    }
    #[cfg(windows)]
    if matches!(error.raw_os_error(), Some(112 | 39)) {
        return true;
    }
    false
}

fn mint_id(prefix: &str) -> String {
    let mut bytes = [0u8; 12];
    let _ = getrandom::fill(&mut bytes);
    let mut out = String::with_capacity(prefix.len() + 1 + 24);
    out.push_str(prefix);
    out.push('_');
    for b in bytes {
        let _ = write!(&mut out, "{b:02x}");
    }
    out
}

fn format_epoch_utc(secs: i64) -> Option<String> {
    let ts = Timestamp::from_second(secs).ok()?;
    Some(solstone_core_time::utc_z(
        ts,
        solstone_core_time::Precision::Secs,
    ))
}

fn owner_date_ymd(journal: &Path, secs: i64) -> String {
    let ts = Timestamp::from_second(secs).unwrap_or_else(|_| Timestamp::now());
    let date = owner_zone(journal).to_datetime(ts).date();
    format!("{:04}-{:02}-{:02}", date.year(), date.month(), date.day())
}

fn cli_command_for_host(host_os: &str, light: bool) -> String {
    let flag = if light {
        " --without-processed-media"
    } else {
        ""
    };
    let sample_out = match host_os {
        "macos" => "/Volumes/MyDrive/solstone-journal.zip",
        "windows" => "E:\\solstone-journal.zip",
        _ => "/media/MyDrive/solstone-journal.zip",
    };
    format!("solstone journal archive export{flag} --out {sample_out}")
}

fn read_backup_summary(journal: &Path) -> Value {
    match solstone_core_backup::status_view(journal) {
        Ok(view) => {
            let enabled = view
                .get("enabled")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let paused_at = view.get("paused_at").and_then(Value::as_i64);
            let last_backup = view.get("last_backup").and_then(Value::as_object);
            let failed_attempts = last_backup
                .and_then(|lb| lb.get("failed_attempts"))
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let failing_since = last_backup
                .and_then(|lb| lb.get("failing_since"))
                .and_then(Value::as_i64);
            let last_status = last_backup
                .and_then(|lb| lb.get("status"))
                .and_then(Value::as_str);
            let state = if !enabled {
                "not_configured"
            } else if paused_at.is_some() {
                "paused"
            } else if failed_attempts > 0 || failing_since.is_some() || last_status == Some("error")
            {
                "unreachable"
            } else {
                "healthy"
            };
            json!({
                "state": state,
                "enabled": enabled,
                "paused_at": paused_at,
            })
        }
        Err(_) => json!({
            "state": "unreachable",
            "enabled": false,
            "paused_at": Value::Null,
        }),
    }
}

fn requester_from_basis(basis: Option<Extension<AccessBasis>>) -> Option<RequesterScope> {
    match basis {
        Some(Extension(AccessBasis::Localhost)) => Some(RequesterScope::Local),
        Some(Extension(AccessBasis::LinkedDevice { cid, .. })) => {
            Some(RequesterScope::LinkedDevice {
                cid: cid.as_str().to_owned(),
            })
        }
        Some(Extension(AccessBasis::PairingPeer { .. })) | None => None,
    }
}

fn unauthorized_export_response() -> Response {
    (
        StatusCode::FORBIDDEN,
        Json(json!({ "code": "export_unauthorized" })),
    )
        .into_response()
}

#[cfg(feature = "host")]
fn is_requester_authorized(journal: &Path, owner: &RequesterScope) -> bool {
    match owner {
        RequesterScope::Local => true,
        RequesterScope::LinkedDevice { cid } => {
            let ledger = solstone_core_sol_link::ledger::AuthorizationLedger::new(journal);
            match solstone_core_sol_link::ledger::read_authorized_clients(
                ledger.authorized_clients_path(),
            ) {
                solstone_core_sol_link::ledger::AuthorizedClientsRead::Present(entries) => {
                    entries.iter().any(|entry| entry.fingerprint == *cid)
                }
                // Match door.rs D8: only a definite Present removal revokes an active carrier,
                // while a missing ledger also means no paired client is authorized.
                solstone_core_sol_link::ledger::AuthorizedClientsRead::Missing => false,
                solstone_core_sol_link::ledger::AuthorizedClientsRead::Unreadable
                | solstone_core_sol_link::ledger::AuthorizedClientsRead::Malformed
                | solstone_core_sol_link::ledger::AuthorizedClientsRead::DuplicateCid => true,
            }
        }
    }
}

#[cfg(not(feature = "host"))]
fn is_requester_authorized(_journal: &Path, owner: &RequesterScope) -> bool {
    matches!(owner, RequesterScope::Local)
}

#[derive(Deserialize)]
struct StartJobBody {
    #[serde(default)]
    mode: ExportMode,
    #[serde(default)]
    replace: bool,
}

/// Resolve the immutable local staged ZIP path for a prepared export owned by
/// `Local`, verifying Trash permanent-erasure epoch and rolling inactivity.
/// Used by on-device phone hosts so Files export hands the prepared file
/// directly without staging a second full-size copy in temporary storage.
pub fn prepared_export_path(journal_root: &Path, export_id: &str) -> Option<(PathBuf, String)> {
    if export_id.is_empty()
        || export_id.len() > 64
        || !export_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return None;
    }
    let staging = StagingArea::resolve(journal_root).ok()?;
    let record = staging.read_ready_record()?;
    if record.export_id != export_id || record.owner != RequesterScope::Local {
        return None;
    }
    let epoch = read_erasure_epoch(journal_root).ok()?;
    if epoch != record.erasure_epoch {
        return None;
    }
    let now = SystemExportClock.now_epoch_secs();
    if now.saturating_sub(record.last_activity_epoch_secs) >= ROLLING_INACTIVITY_SECS {
        return None;
    }
    Some((record.zip_path(&staging.dir), record.filename))
}

/// Mount all `/app/export/` routes plus the `?id=`-aware `/app/import/api/journal-archive/export` bridge.
pub(crate) fn router(journal_root: PathBuf, service: ExportService) -> Router {
    let legacy_root_get = journal_root.clone();
    let legacy_service_get = service.clone();
    let legacy_root_head = journal_root;
    let legacy_service_head = service.clone();
    Router::new()
        .route("/app/export/", get(shell))
        .route("/app/export/workspace", get(workspace))
        .route("/app/export/static/{*rest}", get(static_file))
        .route("/app/export/api/estimate", get(get_estimate))
        .route("/app/export/api/estimate/refresh", post(refresh_estimate))
        .route("/app/export/api/state", get(get_state))
        .route("/app/export/api/jobs", post(post_job))
        .route("/app/export/api/jobs/{job_id}/cancel", post(post_cancel))
        .route("/app/export/api/ready/{export_id}", delete(delete_ready))
        .route(
            "/app/export/api/ready/{export_id}/download",
            get(get_download).head(head_download),
        )
        .with_state(service)
        .route(
            "/app/import/api/journal-archive/export",
            get(move |basis, query, headers| {
                legacy_or_prepared_download(
                    legacy_root_get.clone(),
                    legacy_service_get.clone(),
                    Method::GET,
                    basis,
                    query,
                    headers,
                )
            })
            .head(move |basis, query, headers| {
                legacy_or_prepared_download(
                    legacy_root_head.clone(),
                    legacy_service_head.clone(),
                    Method::HEAD,
                    basis,
                    query,
                    headers,
                )
            }),
        )
}

async fn shell() -> Response {
    asset_response("/static/shell.html")
}

async fn workspace() -> Response {
    asset_response("/app/export/workspace")
}

async fn static_file(AxumPath(rest): AxumPath<String>) -> Response {
    asset_response(&format!("/app/export/static/{rest}"))
}

async fn get_estimate(
    State(service): State<ExportService>,
    basis: Option<Extension<AccessBasis>>,
) -> Response {
    if requester_from_basis(basis).is_none() {
        return unauthorized_export_response();
    }
    service.trigger_estimate_if_needed(false);
    (StatusCode::OK, Json(service.estimate_payload())).into_response()
}

async fn refresh_estimate(
    State(service): State<ExportService>,
    basis: Option<Extension<AccessBasis>>,
) -> Response {
    if requester_from_basis(basis).is_none() {
        return unauthorized_export_response();
    }
    service.trigger_estimate_if_needed(true);
    (StatusCode::ACCEPTED, Json(service.estimate_payload())).into_response()
}

async fn get_state(
    State(service): State<ExportService>,
    basis: Option<Extension<AccessBasis>>,
) -> Response {
    let Some(requester) = requester_from_basis(basis) else {
        return unauthorized_export_response();
    };
    (StatusCode::OK, Json(service.state_payload(&requester))).into_response()
}

async fn post_job(
    State(service): State<ExportService>,
    basis: Option<Extension<AccessBasis>>,
    Json(body): Json<StartJobBody>,
) -> Response {
    let Some(requester) = requester_from_basis(basis) else {
        return unauthorized_export_response();
    };
    match service.start_job(requester, body.mode, body.replace) {
        Ok((status, payload)) | Err((status, payload)) => (status, Json(payload)).into_response(),
    }
}

async fn post_cancel(
    State(service): State<ExportService>,
    basis: Option<Extension<AccessBasis>>,
    AxumPath(job_id): AxumPath<String>,
) -> Response {
    let Some(requester) = requester_from_basis(basis) else {
        return unauthorized_export_response();
    };
    service.cancel_job(&requester, &job_id).into_response()
}

async fn delete_ready(
    State(service): State<ExportService>,
    basis: Option<Extension<AccessBasis>>,
    AxumPath(export_id): AxumPath<String>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    let Some(requester) = requester_from_basis(basis) else {
        return unauthorized_export_response();
    };
    let force = query.get("force").is_some_and(|v| v == "1" || v == "true");
    match service.discard_ready(&requester, &export_id, force) {
        Ok(status) => status.into_response(),
        Err((status, payload)) => (status, Json(payload)).into_response(),
    }
}

async fn get_download(
    State(service): State<ExportService>,
    basis: Option<Extension<AccessBasis>>,
    AxumPath(export_id): AxumPath<String>,
    headers: HeaderMap,
) -> Response {
    let Some(requester) = requester_from_basis(basis) else {
        return unauthorized_export_response();
    };
    service
        .serve_download(&requester, &export_id, &Method::GET, &headers)
        .await
}

async fn head_download(
    State(service): State<ExportService>,
    basis: Option<Extension<AccessBasis>>,
    AxumPath(export_id): AxumPath<String>,
    headers: HeaderMap,
) -> Response {
    let Some(requester) = requester_from_basis(basis) else {
        return unauthorized_export_response();
    };
    service
        .serve_download(&requester, &export_id, &Method::HEAD, &headers)
        .await
}

async fn legacy_or_prepared_download(
    journal_root: PathBuf,
    service: ExportService,
    method: Method,
    basis: Option<Extension<AccessBasis>>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    if let Some(export_id) = query.get("id").filter(|id| !id.is_empty()) {
        let Some(requester) = requester_from_basis(basis) else {
            return unauthorized_export_response();
        };
        return service
            .serve_download(&requester, export_id, &method, &headers)
            .await;
    }
    if method == Method::HEAD {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }
    solstone_core_import_web::journal_archive::legacy_export(&journal_root).await
}
