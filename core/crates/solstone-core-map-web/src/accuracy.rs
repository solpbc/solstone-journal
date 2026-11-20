// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The owner's "location accuracy" choice: what is offered, what is
//! installed, and one download or removal at a time.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde_json::{Value, json};
use solstone_core_convey_http::coalesce::ResponseLocale;
use solstone_core_convey_http::envelope::error_envelope;
use solstone_core_convey_http::owner_read::{OwnerReadRole, spawn_blocking_response};
use solstone_core_convey_http::window_catalog::{response_locale, text};
use solstone_core_location::accuracy::installed_snapshot;
use solstone_core_location::install;

use crate::AppState;

/// Everything the map app shows about the choice.
pub(crate) fn status(journal: &Path, locale: &str) -> Value {
    status_with_offered(journal, install::offered(), locale)
}

pub(crate) fn status_with_offered(
    journal: &Path,
    offered: Option<solstone_core_location::install::Offered>,
    locale: &str,
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
            "detail": packs.and_then(|p| p.load_failure.clone()).unwrap_or_else(|| text(locale, "journal.window.map.accuracy.status.verification.unavailable")),
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

pub(crate) async fn get(
    State(state): State<Arc<AppState>>,
    locale: Option<Extension<ResponseLocale>>,
) -> Response {
    let locale = response_locale(locale);
    let journal_root = state.journal_root.clone();
    spawn_blocking_response(OwnerReadRole::MapAccuracy, move || {
        Json(status(&journal_root, locale)).into_response()
    })
    .await
}

fn busy(locale: &str) -> Response {
    error_envelope(
        "accuracy_busy",
        text(locale, "journal.window.map.response.accuracy.busy"),
        text(locale, "journal.window.map.response.accuracy.busy.detail"),
        StatusCode::CONFLICT,
    )
    .into_response()
}

fn run_download(journal_root: PathBuf, reservation: install::InstallReservation) {
    if let Err(error) = install::install_reserved(&journal_root, reservation, |_, _| {}) {
        log::warn!("location accuracy download: {error}");
    }
}

pub(crate) async fn download(
    State(state): State<Arc<AppState>>,
    locale: Option<Extension<ResponseLocale>>,
) -> Response {
    let locale = response_locale(locale);
    start_download(&state.journal_root, locale, |journal, reservation| {
        std::thread::Builder::new()
            .name("location-accuracy".to_owned())
            .spawn(move || run_download(journal, reservation))
            .map(|_| ())
    })
}

pub(crate) fn start_download(
    journal_root: &Path,
    locale: &str,
    spawn: impl FnOnce(PathBuf, install::InstallReservation) -> std::io::Result<()>,
) -> Response {
    if let Some(job) = install::accuracy_job(journal_root)
        && job.state == "downloading"
    {
        return busy(locale);
    }
    if install::offered().is_none() {
        return error_envelope(
            "accuracy_not_offered",
            text(locale, "journal.window.map.response.accuracy.not.offered"),
            text(
                locale,
                "journal.window.map.response.accuracy.not.offered.detail",
            ),
            StatusCode::NOT_FOUND,
        )
        .into_response();
    }
    let reservation = match install::reserve_install(journal_root) {
        Ok(reservation) => reservation,
        Err(install::InstallError::Busy) => return busy(locale),
        Err(error) => {
            return error_envelope(
                error.reason_code(),
                text(
                    locale,
                    "journal.window.map.response.accuracy.download.failed",
                ),
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
            text(
                locale,
                "journal.window.map.response.accuracy.download.failed",
            ),
            error.to_string(),
            StatusCode::INTERNAL_SERVER_ERROR,
        )
        .into_response();
    }
    (StatusCode::ACCEPTED, Json(status(journal_root, locale))).into_response()
}

pub(crate) async fn remove(
    State(state): State<Arc<AppState>>,
    locale: Option<Extension<ResponseLocale>>,
) -> Response {
    let locale = response_locale(locale);
    // Removing is two unlinks and a directory; it runs inline.
    match install::remove(&state.journal_root) {
        Ok(()) => Json(status(&state.journal_root, locale)).into_response(),
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => busy(locale),
        Err(error) => error_envelope(
            "accuracy_remove_failed",
            text(locale, "journal.window.map.response.accuracy.remove.failed"),
            error.to_string(),
            StatusCode::INTERNAL_SERVER_ERROR,
        )
        .into_response(),
    }
}
