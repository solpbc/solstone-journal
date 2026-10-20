// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The owner's "location accuracy" choice: what is offered, what is
//! installed, and one download or removal at a time.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};
use solstone_core_convey_http::envelope::error_envelope;
use solstone_core_convey_http::owner_read::{OwnerReadRole, spawn_blocking_response};
use solstone_core_location::accuracy::installed_snapshot;
use solstone_core_location::install;

use crate::AppState;

/// Everything the map app shows about the choice.
pub(crate) fn status(journal: &Path) -> Value {
    status_with_offered(journal, install::offered())
}

pub(crate) fn status_with_offered(
    journal: &Path,
    offered: Option<solstone_core_location::install::Offered>,
) -> Value {
    let snapshot = installed_snapshot(journal);
    let installed = snapshot.as_ref().map(|(installed, _)| installed);
    let packs = snapshot.as_ref().and_then(|(_, packs)| packs.as_ref());
    let raw_job = install::accuracy_job(journal);
    let mut job = match &raw_job {
        None => json!({"state": "idle"}),
        Some(job) if job.state == "downloading" => {
            json!({
                "state": if job.phase.as_deref() == Some("verifying") { "verifying" } else { "downloading" },
                "source": job.source,
                "received": job.received.unwrap_or(0),
                "total": job.total.unwrap_or(0),
            })
        }
        Some(job) => {
            json!({
                "state": "failed",
                "reason_code": job.reason_code.as_deref().unwrap_or("download_failed"),
            })
        }
    };
    let has_both_packs = packs
        .as_ref()
        .is_some_and(|p| p.has_venues() && p.has_containment());
    let current = match (&offered, &installed) {
        (Some(offered), Some(installed)) => offered.is_installed(installed) && has_both_packs,
        _ => false,
    };
    if current && raw_job.is_none() {
        job = json!({"state":"ready"});
    }
    if installed.is_some() && !has_both_packs && raw_job.is_none() {
        job = json!({
            "state": "failed",
            "reason_code": "integrity_failed",
            "detail": packs.and_then(|p| p.load_failure.as_deref()).unwrap_or("installed accuracy packs could not be verified"),
        });
    }
    json!({
        "offered": offered.as_ref().map(|offered| json!({
            "version": offered.version,
            "size_bytes": offered.size_bytes(),
            "files": offered.files.iter().map(|file| file.file.clone()).collect::<Vec<_>>(),
        })),
        "installed": installed.as_ref().map(|installed| json!({
            "version": installed.version,
            "size_bytes": installed.size_bytes(),
            "venues": packs.as_ref().is_some_and(|packs| packs.has_venues()),
            "containment": packs.as_ref().is_some_and(|packs| packs.has_containment()),
        })),
        "current": current,
        "attributions": packs.as_ref().map(|packs| packs.attributions()).unwrap_or_default(),
        "notices": packs.as_ref().map(|packs| packs.notices()).unwrap_or_default(),
        "job": job,
    })
}

pub(crate) async fn get(State(state): State<Arc<AppState>>) -> Response {
    let journal_root = state.journal_root.clone();
    spawn_blocking_response(OwnerReadRole::MapAccuracy, move || {
        Json(status(&journal_root)).into_response()
    })
    .await
}

fn busy() -> Response {
    error_envelope(
        "accuracy_busy",
        "location accuracy is already downloading.",
        "A download is running",
        StatusCode::CONFLICT,
    )
    .into_response()
}

fn run_download(journal_root: PathBuf, reservation: install::InstallReservation) {
    if let Err(error) = install::install_reserved(&journal_root, reservation, |_, _| {}) {
        log::warn!("location accuracy download: {error}");
    }
}

pub(crate) async fn download(State(state): State<Arc<AppState>>) -> Response {
    start_download(&state.journal_root, |journal, reservation| {
        std::thread::Builder::new()
            .name("location-accuracy".to_owned())
            .spawn(move || run_download(journal, reservation))
            .map(|_| ())
    })
}

pub(crate) fn start_download(
    journal_root: &Path,
    spawn: impl FnOnce(PathBuf, install::InstallReservation) -> std::io::Result<()>,
) -> Response {
    if let Some(job) = install::accuracy_job(journal_root)
        && job.state == "downloading"
    {
        return busy();
    }
    if install::offered().is_none() {
        return error_envelope(
            "accuracy_not_offered",
            "this version of your journal has no location accuracy download.",
            "Nothing offered",
            StatusCode::NOT_FOUND,
        )
        .into_response();
    }
    let reservation = match install::reserve_install(journal_root) {
        Ok(reservation) => reservation,
        Err(install::InstallError::Busy) => return busy(),
        Err(error) => {
            return error_envelope(
                error.reason_code(),
                "location accuracy couldn't be downloaded.",
                error.to_string(),
                StatusCode::INTERNAL_SERVER_ERROR,
            )
            .into_response();
        }
    };
    if let Err(error) = spawn(journal_root.to_path_buf(), reservation) {
        log::warn!("failed to spawn location accuracy download: {error}");
        return error_envelope(
            "accuracy_download_failed",
            "location accuracy couldn't be downloaded.",
            error.to_string(),
            StatusCode::INTERNAL_SERVER_ERROR,
        )
        .into_response();
    }
    (StatusCode::ACCEPTED, Json(status(journal_root))).into_response()
}

pub(crate) async fn remove(State(state): State<Arc<AppState>>) -> Response {
    // Removing is two unlinks and a directory; it runs inline.
    match install::remove(&state.journal_root) {
        Ok(()) => Json(status(&state.journal_root)).into_response(),
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => busy(),
        Err(error) => error_envelope(
            "accuracy_remove_failed",
            "location accuracy couldn't be removed.",
            error.to_string(),
            StatusCode::INTERNAL_SERVER_ERROR,
        )
        .into_response(),
    }
}
