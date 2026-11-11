// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeSet;
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
use solstone_core_format::content::week_left_out::{
    LeftOut, left_out_bytes, left_out_path, read_left_out,
};
use solstone_core_home::weekly::{Said, WeekJudgment, judge, page_model, week_day};

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
            WeekJudgment::CantShow | WeekJudgment::Untrusted => cant_open_response(
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
            Err(WeekJudgment::CantShow) | Err(WeekJudgment::Untrusted) => cant_open_response(
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
        let saved = save_left_out(&journal_root, &week, req, year, is_moment);
        // Every call queues the page, changed or not, so a retry after a lost
        // queue puts search right.
        let queued = queue_week_page(&journal_root, &week);
        (saved, queued)
    })
    .await;

    match res {
        Ok((Ok(mut model), queued)) => {
            if let Value::Object(fields) = &mut model {
                fields.insert("search_queued".to_owned(), Value::Bool(queued));
            }
            Json(model).into_response()
        }
        Ok((Err((status, msg)), _)) => (status, Json(json!({ "error": msg }))).into_response(),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": join_fail_msg })),
        )
            .into_response(),
    }
}

/// Queue the week's page for search, which leaves out a page with anything
/// left out. Whether it was queued.
fn queue_week_page(journal_root: &Path, week: &str) -> bool {
    // A journal that isn't there has no page to read, and queueing would make
    // its folder.
    if !journal_root.is_dir() {
        return false;
    }
    let page = Path::new("reflections")
        .join("weekly")
        .join(format!("{week}.md"));
    match solstone_core_indexer_todo::enqueue_path(journal_root, &page) {
        Ok(()) => true,
        Err(error) => {
            log::warn!(
                target: "solstone::indexer",
                "week page not queued after a leave-out week={week} cause={error}"
            );
            false
        }
    }
}

fn leave_out_key_trusted(trusted: &solstone_core_home::weekly::TrustedWeek, key: &str) -> bool {
    if trusted.memories.iter().any(|memory| memory.key == key) {
        return true;
    }
    match &trusted.said {
        Said::Entries(entries) => entries.iter().any(|entry| entry.key == key),
        Said::Unreadable => false,
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

    if !req.undo && !leave_out_key_trusted(&trusted, &req.key) {
        return Err((StatusCode::BAD_REQUEST, fail_msg));
    }

    let target_path = left_out_path(journal_root);

    let guard = match solstone_core_journal_io::hold_lock(
        &target_path,
        solstone_core_journal_io::LockOptions::default(),
    ) {
        Ok(g) => g,
        Err(_) => return Err((StatusCode::INTERNAL_SERVER_ERROR, fail_msg)),
    };

    let left_out = read_left_out(journal_root);
    let mut keys = match left_out {
        LeftOut::Unreadable => {
            drop(guard);
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

    let bytes = left_out_bytes(&keys);

    if solstone_core_journal_io::atomic_replace(
        &target_path,
        &bytes,
        solstone_core_journal_io::AtomicWriteOptions::default(),
    )
    .is_err()
    {
        drop(guard);
        return Err((StatusCode::INTERNAL_SERVER_ERROR, fail_msg));
    }

    drop(guard);

    page_model(journal_root, week, current_year, &is_moment).map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "your journal couldn't check this source.",
        )
    })
}
