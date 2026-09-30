// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use axum::Json;
use axum::extract::Path as AxumPath;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Datelike, Utc};
use serde::Deserialize;
use serde_json::{Value, json};
use solstone_core_convey_http::cant_open_response;
use solstone_core_convey_http::owner_read::{OwnerReadRole, spawn_blocking_response};
use solstone_core_home::weekly::{LeftOut, WeekJudgment, judge, page_model, week_day};

use crate::Clock;

fn owner_year(journal: &Path, now: DateTime<Utc>) -> i32 {
    solstone_core_home::HomeContext::new(journal, now)
        .local_date()
        .year()
}

#[derive(Deserialize)]
pub struct LeaveOutRequest {
    pub key: String,
    #[serde(default)]
    pub undo: bool,
}

pub async fn week_shell(AxumPath(week): AxumPath<String>, journal_root: PathBuf) -> Response {
    if week_day(&week).is_none() {
        return cant_open_response(
            StatusCode::BAD_REQUEST,
            "your journal won't follow this link.",
        );
    }

    spawn_blocking_response(OwnerReadRole::HomeWeek, move || {
        match judge(&journal_root, &week) {
            WeekJudgment::Page(_) => {
                let bytes =
                    include_bytes!("../../solstone-core-convey-shell/assets/static/shell.html");
                Response::builder()
                    .header(axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8")
                    .body(axum::body::Body::from(bytes.as_slice()))
                    .expect("shell response")
            }
            WeekJudgment::Absent => {
                cant_open_response(StatusCode::NOT_FOUND, "it isn't in your journal.")
            }
            WeekJudgment::CantShow => cant_open_response(
                StatusCode::NOT_FOUND,
                "your journal can't show this kind of source.",
            ),
            WeekJudgment::CouldntCheck => cant_open_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "your journal couldn't check this source.",
            ),
        }
    })
    .await
}

pub async fn week_api(
    AxumPath(week): AxumPath<String>,
    journal_root: PathBuf,
    clock: Clock,
    is_moment: fn(&str) -> bool,
) -> Response {
    if week_day(&week).is_none() {
        return cant_open_response(
            StatusCode::BAD_REQUEST,
            "your journal won't follow this link.",
        );
    }

    spawn_blocking_response(OwnerReadRole::HomeWeek, move || {
        let now = clock.now();
        let year = owner_year(&journal_root, now);
        match page_model(&journal_root, &week, year, &is_moment) {
            Ok(model) => Json(model).into_response(),
            Err(WeekJudgment::Absent) => {
                cant_open_response(StatusCode::NOT_FOUND, "it isn't in your journal.")
            }
            Err(WeekJudgment::CantShow) => cant_open_response(
                StatusCode::NOT_FOUND,
                "your journal can't show this kind of source.",
            ),
            Err(WeekJudgment::CouldntCheck) | Err(WeekJudgment::Page(_)) => cant_open_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "your journal couldn't check this source.",
            ),
        }
    })
    .await
}

pub async fn week_leave_out(
    AxumPath(week): AxumPath<String>,
    journal_root: PathBuf,
    clock: Clock,
    is_moment: fn(&str) -> bool,
    Json(req): Json<LeaveOutRequest>,
) -> Response {
    if week_day(&week).is_none() {
        return cant_open_response(
            StatusCode::BAD_REQUEST,
            "your journal won't follow this link.",
        );
    }

    let join_fail_msg = if req.undo {
        "couldn't bring this back. nothing changed."
    } else {
        "couldn't leave this out. nothing changed."
    };

    let res = tokio::task::spawn_blocking(move || {
        let now = clock.now();
        let year = owner_year(&journal_root, now);
        save_left_out(&journal_root, &week, req, year, is_moment)
    })
    .await;

    match res {
        Ok(Ok(model)) => Json(model).into_response(),
        Ok(Err((status, msg))) => (status, Json(json!({ "error": msg }))).into_response(),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": join_fail_msg })),
        )
            .into_response(),
    }
}

pub fn save_left_out(
    journal_root: &Path,
    week: &str,
    req: LeaveOutRequest,
    current_year: i32,
    is_moment: fn(&str) -> bool,
) -> Result<Value, (StatusCode, &'static str)> {
    let fail_msg = if req.undo {
        "couldn't bring this back. nothing changed."
    } else {
        "couldn't leave this out. nothing changed."
    };

    let trusted = match judge(journal_root, week) {
        WeekJudgment::Page(t) => t,
        _ => return Err((StatusCode::BAD_REQUEST, fail_msg)),
    };

    if !req.undo && !trusted.memories.iter().any(|m| m.key == req.key) {
        return Err((StatusCode::BAD_REQUEST, fail_msg));
    }

    let weekly_dir = journal_root.join("reflections/weekly");
    let _ = fs::create_dir_all(&weekly_dir);
    let target_path = weekly_dir.join("left-out.json");
    let lock_path = weekly_dir.join("left-out.json.lock");

    let guard = match solstone_core_journal_io::hold_lock(
        &target_path,
        solstone_core_journal_io::LockOptions::default(),
    ) {
        Ok(g) => g,
        Err(_) => return Err((StatusCode::INTERNAL_SERVER_ERROR, fail_msg)),
    };

    let left_out = solstone_core_home::weekly::read_left_out(journal_root);
    let mut keys = match left_out {
        LeftOut::Unreadable => {
            drop(guard);
            let _ = fs::remove_file(&lock_path);
            return Err((StatusCode::INTERNAL_SERVER_ERROR, fail_msg));
        }
        LeftOut::Keys(k) => k,
        LeftOut::None => BTreeSet::new(),
    };

    if req.undo {
        keys.remove(&req.key);
    } else {
        keys.insert(req.key);
    }

    let bytes = solstone_core_home::weekly::left_out_bytes(&keys);

    if solstone_core_journal_io::atomic_replace(
        &target_path,
        &bytes,
        solstone_core_journal_io::AtomicWriteOptions::default(),
    )
    .is_err()
    {
        drop(guard);
        let _ = fs::remove_file(&lock_path);
        return Err((StatusCode::INTERNAL_SERVER_ERROR, fail_msg));
    }

    drop(guard);
    let _ = fs::remove_file(&lock_path);

    page_model(journal_root, week, current_year, &is_moment)
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, fail_msg))
}
