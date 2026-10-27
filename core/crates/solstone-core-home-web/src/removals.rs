// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::{
    Extension, Json,
    body::Bytes,
    extract::{Path as RoutePath, Query, State, rejection::PathRejection},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use chrono::SecondsFormat;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use solstone_core_journal_io::JournalRoot;
use solstone_core_retention::mark_mutation::{
    BoundMutationOutcome, BoundMutationRefusal, MutationStatus, decline_reviewed,
};
use solstone_core_retention::marks::{Approval, MarkId, MarkState};
use solstone_core_retention::review_context::{
    ReviewContext, ReviewRefusal, check_review_context, load_at_root, read_review_context,
};
use solstone_core_retention_client as retention;

const LIST_EMPTY: &str = "list.empty";
const LIST_READY: &str = "list.ready";
const LIST_REGISTER_UNAVAILABLE: &str = "list.register_unavailable";
const TOOL_UNAVAILABLE: &str = "tool.unavailable";
const OUTCOME_UNKNOWN: &str = "outcome.unknown";
const REQUEST_INVALID: &str = "request.invalid";
const REQUEST_TOO_LARGE: &str = "request.too_large";
const APPROVE_REFUSED_BEFORE_START: &str = "approve.refused_before_start";
#[cfg(test)]
const APPROVE_REFUSED_AFTER_START: &str = "approve.refused_after_start";
#[cfg(test)]
const APPROVE_PARTIAL: &str = "approve.partial";
#[cfg(test)]
const APPROVE_DELETED: &str = "approve.deleted";
#[cfg(test)]
const APPROVE_HALTED: &str = "approve.halted";
const DECLINED_DONE: &str = "declined.done";
const DECLINED_REFUSED: &str = "declined.refused";
const DECLINED_UNKNOWN: &str = "declined.unknown";
const REFUSAL_ITEM_NAMED: &str = "refusal.item_named";
const REFUSAL_ITEM_UNNAMED: &str = "refusal.item_unnamed";
const RECOVER_DONE: &str = "recover.done";
const RECOVER_NONE: &str = "recover.none";
const RECOVER_FAILED: &str = "recover.failed";

enum ClientCall {
    Completed(Result<Value, retention::ClientError>),
    Join,
}

struct RemovalOutcome {
    state: &'static str,
    removed_count: usize,
    not_removed_count: usize,
    halted: bool,
    refusals: Vec<Value>,
}

struct RecoverOutcome {
    state: &'static str,
    finished_count: usize,
    not_finished_count: usize,
    halted: bool,
    refusals: Vec<Value>,
}

const MAX_REVIEW_CONTEXT_BYTES: usize = 512 * 1024;
const LIST_REVIEW_TOO_LARGE: &str = "list.review_too_large";

const REVIEW_PAGE_SIZE: usize = 20;

#[derive(Default, Deserialize)]
pub struct ReviewPageQuery {
    #[serde(default)]
    offset: usize,
}

#[derive(Serialize)]
struct ReviewPage {
    state: &'static str,
    removals: Vec<Value>,
    review_context: Option<String>,
    offset: usize,
    next_offset: Option<usize>,
    total_rows: usize,
}

pub async fn list(
    State(journal_root): State<PathBuf>,
    Query(query): Query<ReviewPageQuery>,
) -> Response {
    match tokio::task::spawn_blocking(move || rendered_review(&journal_root, query.offset)).await {
        Ok(Ok(page)) => uncached(Json(page).into_response()),
        Ok(Err(state)) => list_response(state, Vec::new(), None),
        Err(_) => list_response(OUTCOME_UNKNOWN, Vec::new(), None),
    }
}

fn rendered_review(journal_root: &Path, offset: usize) -> Result<ReviewPage, &'static str> {
    let root = JournalRoot::open(journal_root).map_err(|_| LIST_REGISTER_UNAVAILABLE)?;
    let register = load_at_root(&root).map_err(|_| LIST_REGISTER_UNAVAILABLE)?;
    let mut marks = register
        .marks
        .values()
        .filter(|mark| project_mark((*mark).clone()).is_some())
        .collect::<Vec<_>>();
    marks.sort_by(|left, right| {
        (&right.target.day, &right.target.dir, right.id.as_str()).cmp(&(
            &left.target.day,
            &left.target.dir,
            left.id.as_str(),
        ))
    });
    let total_rows = marks.len();
    // A completed action may have removed the last page since it was rendered.
    let offset = if offset < total_rows { offset } else { 0 };
    let mut ids = marks
        .iter()
        .skip(offset)
        .take(REVIEW_PAGE_SIZE)
        .map(|mark| mark.id.clone())
        .collect::<Vec<_>>();
    let observed = read_review_context(&root, &ids).map_err(|_| LIST_REGISTER_UNAVAILABLE)?;
    let mut context = observed.clone();
    loop {
        let serialized = serde_json::to_string(&context).map_err(|_| OUTCOME_UNKNOWN)?;
        let next = offset + ids.len();
        let next_offset = (next < total_rows).then_some(next);
        if serialized.len() <= MAX_REVIEW_CONTEXT_BYTES {
            let removals = project_context(&context);
            return Ok(ReviewPage {
                state: if removals.is_empty() {
                    LIST_EMPTY
                } else {
                    LIST_READY
                },
                removals,
                review_context: Some(serialized),
                offset,
                next_offset,
                total_rows,
            });
        }
        if ids.len() == 1 {
            // Keep the identity visible and allow navigation past this item, but
            // never send truncated approval evidence or actionable controls.
            return Ok(ReviewPage {
                state: LIST_REVIEW_TOO_LARGE,
                removals: project_context(&context),
                review_context: None,
                offset,
                next_offset,
                total_rows,
            });
        }
        ids.pop();
        context = observed.select(&ids).map_err(|_| OUTCOME_UNKNOWN)?;
    }
}

pub async fn approve(
    deps: Option<Extension<solstone_core_backup_web::BackupWebDeps>>,
    Extension(clock): Extension<crate::Clock>,
    body: Result<Bytes, axum::extract::rejection::BytesRejection>,
) -> Response {
    let context = match action_context(body) {
        Ok(context) => context,
        Err((status, state)) => return uncached(request_response(status, state)),
    };
    let count = context.marks().len();
    #[cfg(not(any(
        target_os = "linux",
        target_os = "macos",
        target_os = "windows",
        target_os = "android",
        target_os = "ios"
    )))]
    {
        let _ = (deps, clock);
        uncached(write_response(count, refused_before_start()))
    }
    #[cfg(any(
        target_os = "linux",
        target_os = "macos",
        target_os = "windows",
        target_os = "android",
        target_os = "ios"
    ))]
    {
        let Some(Extension(deps)) = deps else {
            return uncached(write_response(count, refused_before_start()));
        };
        #[cfg(all(test, feature = "full-tests"))]
        #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
        let after_start = approval_tests::take_public_start_hook();
        let result = tokio::task::spawn_blocking(move || {
            let now = clock.now();
            let started = solstone_core_backup_web::archive_worker::removal::start(
                &deps,
                context,
                now.date_naive(),
                now,
            );
            #[cfg(all(test, feature = "full-tests"))]
            #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
            if let (Ok(started), Some(after_start)) = (&started, after_start) {
                after_start(started.generation());
            }
            started
        })
        .await;
        match result {
            Ok(Ok(started)) => uncached(
                Json(json!({"state":"approve.started", "generation":started.generation()}))
                    .into_response(),
            ),
            Ok(Err(_)) => uncached(write_response(count, refused_before_start())),
            Err(_) => approval_unavailable(OUTCOME_UNKNOWN, StatusCode::SERVICE_UNAVAILABLE),
        }
    }
}

fn uncached(mut response: Response) -> Response {
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-store"),
    );
    response
}

fn approval_unavailable(state: &'static str, status: StatusCode) -> Response {
    uncached((status, Json(json!({"state": state}))).into_response())
}

async fn approval_receiving(
    deps: Option<Extension<solstone_core_backup_web::BackupWebDeps>>,
    id: Result<RoutePath<String>, PathRejection>,
    cancel: bool,
) -> Response {
    let Ok(RoutePath(id)) = id else {
        return approval_unavailable(REQUEST_INVALID, StatusCode::BAD_REQUEST);
    };
    let Some(Extension(deps)) = deps else {
        return approval_unavailable("approve.unavailable", StatusCode::SERVICE_UNAVAILABLE);
    };
    let result = tokio::task::spawn_blocking(move || {
        use solstone_core_backup_web::archive_worker::removal;
        if cancel {
            removal::cancel_response(&deps.operations, &id)
        } else {
            removal::response(&deps.operations, &id)
        }
    })
    .await;
    use solstone_core_backup_web::archive_worker::removal::ApprovalStatusRefusal;
    match result {
        Ok(Ok(response)) => uncached(response),
        Ok(Err(ApprovalStatusRefusal::Stale)) => {
            approval_unavailable("approve.stale", StatusCode::CONFLICT)
        }
        Ok(Err(ApprovalStatusRefusal::Unavailable)) | Err(_) => {
            approval_unavailable("approve.unavailable", StatusCode::SERVICE_UNAVAILABLE)
        }
    }
}

pub async fn approval_status(
    deps: Option<Extension<solstone_core_backup_web::BackupWebDeps>>,
    id: Result<RoutePath<String>, PathRejection>,
) -> Response {
    approval_receiving(deps, id, false).await
}

pub async fn cancel_approval(
    deps: Option<Extension<solstone_core_backup_web::BackupWebDeps>>,
    id: Result<RoutePath<String>, PathRejection>,
    body: Result<Bytes, axum::extract::rejection::BytesRejection>,
) -> Response {
    let body = match body {
        Ok(body) if body.len() <= 1024 => body,
        Ok(_) => return approval_unavailable(REQUEST_TOO_LARGE, StatusCode::PAYLOAD_TOO_LARGE),
        Err(error) => {
            return if error.status() == StatusCode::PAYLOAD_TOO_LARGE {
                approval_unavailable(REQUEST_TOO_LARGE, StatusCode::PAYLOAD_TOO_LARGE)
            } else {
                approval_unavailable(REQUEST_INVALID, StatusCode::BAD_REQUEST)
            };
        }
    };
    if !serde_json::from_slice::<serde_json::Map<String, Value>>(&body)
        .is_ok_and(|object| object.is_empty())
    {
        return approval_unavailable(REQUEST_INVALID, StatusCode::BAD_REQUEST);
    }
    approval_receiving(deps, id, true).await
}

// Direct fixture entry for the same retained worker used by the public handler.
#[cfg(all(test, feature = "full-tests"))]
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
fn dispatch_approval(
    deps: Option<Extension<solstone_core_backup_web::BackupWebDeps>>,
    body: Result<Bytes, axum::extract::rejection::BytesRejection>,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<solstone_core_backup_web::archive_worker::removal::ApprovalStart, &'static str> {
    let context = action_context(body).map_err(|(_, state)| state)?;
    let Extension(deps) = deps.ok_or(APPROVE_REFUSED_BEFORE_START)?;
    solstone_core_backup_web::archive_worker::removal::start(&deps, context, now.date_naive(), now)
        .map_err(|_| APPROVE_REFUSED_BEFORE_START)
}

#[cfg(all(test, feature = "full-tests"))]
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
mod approval_tests;

#[cfg(all(test, feature = "full-tests"))]
static DECLINE_PAUSE_HOOK: std::sync::Mutex<
    Option<(std::sync::mpsc::Sender<()>, std::sync::mpsc::Receiver<()>)>,
> = std::sync::Mutex::new(None);

#[cfg(all(test, feature = "full-tests"))]
pub(crate) fn arm_decline_pause() -> (std::sync::mpsc::Receiver<()>, std::sync::mpsc::Sender<()>) {
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    *DECLINE_PAUSE_HOOK.lock().unwrap() = Some((entered_tx, release_rx));
    (entered_rx, release_tx)
}

#[cfg(all(test, feature = "full-tests"))]
fn pause_decline() {
    let pair = {
        let mut guard = DECLINE_PAUSE_HOOK.lock().unwrap();
        guard.take()
    };
    if let Some((entered_tx, release_rx)) = pair {
        let _ = entered_tx.send(());
        let _ = release_rx.recv();
    }
}

#[cfg(all(test, feature = "full-tests"))]
static RECOVER_PAUSE_HOOK: std::sync::Mutex<
    Option<(std::sync::mpsc::Sender<()>, std::sync::mpsc::Receiver<()>)>,
> = std::sync::Mutex::new(None);

#[cfg(all(test, feature = "full-tests"))]
pub(crate) fn arm_recover_pause() -> (std::sync::mpsc::Receiver<()>, std::sync::mpsc::Sender<()>) {
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    *RECOVER_PAUSE_HOOK.lock().unwrap() = Some((entered_tx, release_rx));
    (entered_rx, release_tx)
}

#[cfg(all(test, feature = "full-tests"))]
fn pause_recover() {
    let pair = {
        let mut guard = RECOVER_PAUSE_HOOK.lock().unwrap();
        guard.take()
    };
    if let Some((entered_tx, release_rx)) = pair {
        let _ = entered_tx.send(());
        let _ = release_rx.recv();
    }
}

#[cfg(all(test, feature = "full-tests"))]
static RECOVER_APPEND_PAUSE_HOOK: std::sync::Mutex<
    Option<(std::sync::mpsc::Sender<()>, std::sync::mpsc::Receiver<()>)>,
> = std::sync::Mutex::new(None);

#[cfg(all(test, feature = "full-tests"))]
pub(crate) fn arm_recover_append_pause()
-> (std::sync::mpsc::Receiver<()>, std::sync::mpsc::Sender<()>) {
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    *RECOVER_APPEND_PAUSE_HOOK.lock().unwrap() = Some((entered_tx, release_rx));
    (entered_rx, release_tx)
}

#[cfg(all(test, feature = "full-tests"))]
fn pause_recover_append() {
    let pair = {
        let mut guard = RECOVER_APPEND_PAUSE_HOOK.lock().unwrap();
        guard.take()
    };
    if let Some((entered_tx, release_rx)) = pair {
        let _ = entered_tx.send(());
        let _ = release_rx.recv();
    }
}

#[cfg(all(test, feature = "full-tests"))]
mod route_lifetime_tests;

pub async fn decline(
    deps: Option<Extension<solstone_core_backup_web::BackupWebDeps>>,
    State(journal_root): State<PathBuf>,
    body: Result<Bytes, axum::extract::rejection::BytesRejection>,
) -> Response {
    let context = match action_context(body) {
        Ok(context) => context,
        Err((status, state)) => return request_response(status, state),
    };
    let deps = if let Some(Extension(deps)) = deps {
        if deps.journal_root != journal_root {
            return solstone_core_backup_web::invalid_operation_response();
        }
        Some(deps)
    } else {
        None
    };
    let count = context.marks().len();
    match tokio::task::spawn_blocking(move || {
        let _guard = if let Some(deps) = deps {
            match solstone_core_backup_web::admit_write(
                &deps.operations,
                solstone_core_backup_web::AdmitWrite::DuringOperation,
            ) {
                Ok(guard) => Some(guard),
                Err(response) => return response,
            }
        } else {
            None
        };
        #[cfg(all(test, feature = "full-tests"))]
        pause_decline();
        decline_current(&journal_root, &context)
    })
    .await
    {
        Ok(response) => response,
        Err(_) => Json(decline_payload(count, None)).into_response(),
    }
}

fn decline_current(journal_root: &Path, context: &ReviewContext) -> Response {
    let count = context.marks().len();
    let root = match JournalRoot::open(journal_root) {
        Ok(root) => Arc::new(root),
        Err(error) => {
            let result = Err(BoundMutationRefusal::Review(ReviewRefusal::Root(error)));
            return Json(decline_payload(count, Some(&result))).into_response();
        }
    };
    let result = check_review_context(Arc::clone(&root), context)
        .map_err(BoundMutationRefusal::Review)
        .and_then(decline_reviewed);
    // Keep the original root and whole mutation result through serialization.
    Json(decline_payload(count, Some(&result))).into_response()
}

fn decline_payload(
    count: usize,
    result: Option<&Result<BoundMutationOutcome, BoundMutationRefusal>>,
) -> Value {
    let (state, declined_count, refused_count, unknown_count) = match result {
        Some(Ok(BoundMutationOutcome::Published(result)))
            if result.status == MutationStatus::Applied =>
        {
            (DECLINED_DONE, count, 0, 0)
        }
        None | Some(Ok(BoundMutationOutcome::PublishedUncertain { .. })) => {
            (DECLINED_UNKNOWN, 0, 0, count)
        }
        Some(_) => (DECLINED_REFUSED, 0, count, 0),
    };
    json!({
        "state": state,
        "requested_count": count,
        "declined_count": declined_count,
        "refused_count": refused_count,
        "unavailable_count": 0,
        "unknown_count": unknown_count,
        "mutation": result.map(solstone_core_backup_web::removal_report::mutation_facts),
    })
}

fn action_context(
    body: Result<Bytes, axum::extract::rejection::BytesRejection>,
) -> Result<ReviewContext, (StatusCode, &'static str)> {
    let body = body.map_err(|error| {
        if error.status() == StatusCode::PAYLOAD_TOO_LARGE {
            (StatusCode::PAYLOAD_TOO_LARGE, REQUEST_TOO_LARGE)
        } else {
            (StatusCode::BAD_REQUEST, REQUEST_INVALID)
        }
    })?;
    decode_review_request(&body).map_err(|state| {
        let status = if state == REQUEST_TOO_LARGE {
            StatusCode::PAYLOAD_TOO_LARGE
        } else {
            StatusCode::BAD_REQUEST
        };
        (status, state)
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReviewRequest {
    review_context: String,
    mark_ids: Vec<String>,
}

fn decode_review_request(body: &[u8]) -> Result<ReviewContext, &'static str> {
    let request: ReviewRequest = serde_json::from_slice(body).map_err(|_| REQUEST_INVALID)?;
    if request.mark_ids.len() > retention::MAX_REMOVE_MARK_IDS
        || request.review_context.len() > MAX_REVIEW_CONTEXT_BYTES
    {
        return Err(REQUEST_TOO_LARGE);
    }
    let context: ReviewContext =
        serde_json::from_str(&request.review_context).map_err(|_| REQUEST_INVALID)?;
    let ids = request
        .mark_ids
        .iter()
        .map(|id| MarkId::parse(id).ok_or(REQUEST_INVALID))
        .collect::<Result<Vec<_>, _>>()?;
    let selected = context.select(&ids).map_err(|_| REQUEST_INVALID)?;
    if selected
        .marks()
        .iter()
        .any(|mark| mark.state != MarkState::Marked || mark.class.axes().1 != Approval::Required)
    {
        return Err(REQUEST_INVALID);
    }
    // Rendered transport grants no authority; each consuming action must
    // freshly validate the selected full rows against its retained root.
    Ok(selected)
}

pub async fn recover(
    deps: Option<Extension<solstone_core_backup_web::BackupWebDeps>>,
    State(journal_root): State<PathBuf>,
    Extension(clock): Extension<crate::Clock>,
    body: Bytes,
) -> Response {
    if let Err(state) = recover_empty(body) {
        return request_response(StatusCode::BAD_REQUEST, state);
    }
    let deps = if let Some(Extension(deps)) = deps {
        if deps.journal_root != journal_root {
            return solstone_core_backup_web::invalid_operation_response();
        }
        Some(deps)
    } else {
        None
    };
    let at = clock.now().to_rfc3339_opts(SecondsFormat::Secs, true);
    let outcome = match tokio::task::spawn_blocking(move || {
        let _guard = if let Some(deps) = deps {
            match solstone_core_backup_web::admit_write(
                &deps.operations,
                solstone_core_backup_web::AdmitWrite::DuringOperation,
            ) {
                Ok(guard) => Some(guard),
                Err(response) => return Err(Box::new(response)),
            }
        } else {
            None
        };
        #[cfg(all(test, feature = "full-tests"))]
        pause_recover();
        let call = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            retention::recover(journal_root.clone(), at.clone())
        })) {
            Ok(result) => ClientCall::Completed(result),
            Err(_) => ClientCall::Join,
        };
        #[cfg(all(test, feature = "full-tests"))]
        pause_recover_append();
        let outcome = recover_from_call(call);
        append_action(
            &journal_root,
            "removal_recover",
            json!({
                "finished_count": outcome.finished_count,
                "not_finished_count": outcome.not_finished_count,
                "halted": outcome.halted,
                "refusals": outcome.refusals,
                "outcome_state": outcome.state,
            }),
        );
        Ok(outcome)
    })
    .await
    {
        Ok(Ok(outcome)) => outcome,
        Ok(Err(response)) => return *response,
        Err(_) => recover_from_call(ClientCall::Join),
    };
    recover_response(&outcome)
}

fn recover_empty(body: Bytes) -> Result<(), &'static str> {
    if body.is_empty() {
        return Ok(());
    }
    match serde_json::from_slice::<Value>(&body) {
        Ok(Value::Object(request)) if request.is_empty() => Ok(()),
        _ => Err(REQUEST_INVALID),
    }
}

fn project_context(context: &ReviewContext) -> Vec<Value> {
    let mut removals = context
        .marks()
        .iter()
        .cloned()
        .filter_map(project_mark)
        .collect::<Vec<_>>();
    // The register is keyed by mark id, which is not an order the owner can
    // read. Newest first, so a page of rows is a run of days.
    removals.sort_by(|left, right| {
        let key = |row: &Value| {
            (
                row.get("day")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned(),
                row.get("dir")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned(),
                row.get("id")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned(),
            )
        };
        key(right).cmp(&key(left))
    });
    removals
}

fn project_mark(mark: retention::Mark) -> Option<Value> {
    let (_, approval, _) = mark.class.axes();
    let approval_required = serde_json::to_value(approval)
        .ok()
        .and_then(|value| value.as_str().map(|value| value == "required"))
        .unwrap_or(false);
    let class = serde_json::to_value(mark.class).ok()?;
    let origin = serde_json::to_value(mark.class.axes().0).ok()?;
    let mut row = serde_json::Map::from_iter([
        ("id".to_owned(), Value::String(mark.id.as_str().to_owned())),
        ("class".to_owned(), class),
        ("origin".to_owned(), origin),
        ("day".to_owned(), Value::String(mark.target.day)),
        ("stream".to_owned(), Value::String(mark.target.stream)),
        ("dir".to_owned(), Value::String(mark.target.dir)),
        ("marked_at".to_owned(), Value::String(mark.marked_at)),
        ("count".to_owned(), json!(mark.proposal.names.len())),
        ("bytes".to_owned(), json!(mark.proposal.bytes)),
        (
            "size".to_owned(),
            Value::String(retention::human_bytes(mark.proposal.bytes)),
        ),
    ]);
    let unbound_offload =
        mark.class == retention::RemovalClass::OffloadRawRelease && mark.offload.is_none();
    match mark.state {
        // An earlier version's offload proposal carries no proof of its backup
        // copy, so it can never be approved; offer planning withdraws it.
        retention::MarkState::Marked if unbound_offload => return None,
        retention::MarkState::Marked if approval_required => {
            row.insert("state".to_owned(), Value::String("marked".to_owned()));
        }
        retention::MarkState::Marked => return None,
        retention::MarkState::Failed(failure) => {
            row.insert("state".to_owned(), Value::String("failed".to_owned()));
            row.insert("at".to_owned(), Value::String(failure.at));
            row.insert("reason".to_owned(), Value::String(failure.reason));
            row.insert(
                "staged".to_owned(),
                failure.staged.map_or(Value::Null, Value::String),
            );
        }
    }
    Some(Value::Object(row))
}

#[cfg(test)]
fn marks_store_refusal(receipt: &Value) -> bool {
    has_exact_keys(receipt, ["ok", "verb", "error"])
        && receipt.get("ok") == Some(&Value::Bool(false))
        && receipt.get("verb") == Some(&Value::String("marks".to_owned()))
        && receipt.get("error").and_then(Value::as_str).is_some()
}

#[cfg(test)]
fn removal_outcome(receipt: &Value) -> Option<RemovalOutcome> {
    let outcome = receipt.get("outcome")?.as_object()?;
    let targets = outcome.get("targets")?.as_array()?;
    let halted = match outcome.get("halted")? {
        Value::Null => false,
        Value::Object(value) if value.get("reason").and_then(Value::as_str).is_some() => true,
        _ => return None,
    };
    let mut removed_count = 0usize;
    let mut not_removed_count = 0usize;
    let mut refusals = Vec::new();
    for target in targets {
        let target = target.as_object()?;
        let removed = target.get("removed")?.as_array()?;
        if !removed.iter().all(Value::is_string) {
            return None;
        }
        removed_count = removed_count.saturating_add(removed.len());
        let not_removed = target.get("not_removed")?.as_array()?;
        not_removed_count = not_removed_count.saturating_add(not_removed.len());
        for item in not_removed {
            let item = item.as_object()?;
            let entry = item.get("entry").and_then(Value::as_str);
            let reason = item.get("reason").and_then(Value::as_str)?;
            refusals.push(refusal_item(entry, reason));
        }
    }
    let state = if halted {
        APPROVE_HALTED
    } else if removed_count == 0 && not_removed_count > 0 {
        APPROVE_REFUSED_AFTER_START
    } else if removed_count > 0 && not_removed_count > 0 {
        APPROVE_PARTIAL
    } else if removed_count > 0 {
        APPROVE_DELETED
    } else {
        return None;
    };
    Some(RemovalOutcome {
        state,
        removed_count,
        not_removed_count,
        halted,
        refusals,
    })
}

fn recover_outcome(receipt: &Value) -> Option<RecoverOutcome> {
    let outcome = receipt.get("outcome")?.as_object()?;
    let targets = outcome.get("targets")?.as_array()?;
    let halted = match outcome.get("halted")? {
        Value::Null => false,
        Value::Object(value) if value.get("reason").and_then(Value::as_str).is_some() => true,
        _ => return None,
    };
    let mut finished_count = 0usize;
    let mut not_finished_count = 0usize;
    let mut refusals = Vec::new();
    for target in targets {
        let target = target.as_object()?;
        let not_removed = target.get("not_removed")?.as_array()?;
        if not_removed.is_empty() {
            finished_count = finished_count.saturating_add(1);
            continue;
        }
        not_finished_count = not_finished_count.saturating_add(1);
        for item in not_removed {
            let item = item.as_object()?;
            let entry = item.get("entry").and_then(Value::as_str);
            let reason = item.get("reason").and_then(Value::as_str)?;
            refusals.push(refusal_item(entry, reason));
        }
    }
    let state = if halted || not_finished_count > 0 {
        RECOVER_FAILED
    } else if finished_count > 0 {
        RECOVER_DONE
    } else {
        RECOVER_NONE
    };
    Some(RecoverOutcome {
        state,
        finished_count,
        not_finished_count,
        halted,
        refusals,
    })
}

fn recover_response(outcome: &RecoverOutcome) -> Response {
    Json(json!({
        "state": outcome.state,
        "finished_count": outcome.finished_count,
        "not_finished_count": outcome.not_finished_count,
        "halted": outcome.halted,
        "refusals": outcome.refusals,
    }))
    .into_response()
}

fn recover_from_call(call: ClientCall) -> RecoverOutcome {
    match call {
        ClientCall::Completed(Ok(receipt)) => {
            recover_outcome(&receipt).unwrap_or_else(recover_unknown)
        }
        ClientCall::Completed(Err(retention::ClientError::Refused(refused))) => {
            recover_outcome(refused.receipt_value()).unwrap_or_else(recover_unknown)
        }
        ClientCall::Completed(Err(retention::ClientError::BinaryUnavailable(_)))
        | ClientCall::Completed(Err(retention::ClientError::RequestTooLarge(_))) => {
            recover_unavailable()
        }
        ClientCall::Completed(Err(retention::ClientError::OutcomeUnknown(_)))
        | ClientCall::Join => recover_unknown(),
    }
}

fn recover_unknown() -> RecoverOutcome {
    RecoverOutcome {
        state: OUTCOME_UNKNOWN,
        finished_count: 0,
        not_finished_count: 0,
        halted: false,
        refusals: Vec::new(),
    }
}

fn recover_unavailable() -> RecoverOutcome {
    RecoverOutcome {
        state: TOOL_UNAVAILABLE,
        finished_count: 0,
        not_finished_count: 0,
        halted: false,
        refusals: Vec::new(),
    }
}

#[cfg(test)]
fn remove_preflight_refusal(receipt: &Value) -> bool {
    has_exact_keys(receipt, ["ok", "verb", "error"])
        && receipt.get("ok") == Some(&Value::Bool(false))
        && receipt.get("verb") == Some(&Value::String("remove-marked".to_owned()))
        && receipt.get("error").and_then(Value::as_str).is_some()
}

fn refusal_item(entry: Option<&str>, reason: &str) -> Value {
    let name = entry
        .and_then(|entry| Path::new(entry).file_name())
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty());
    match name {
        Some(name) => json!({"state": REFUSAL_ITEM_NAMED, "name": name, "reason": reason}),
        None => json!({"state": REFUSAL_ITEM_UNNAMED, "reason": reason}),
    }
}

#[cfg(test)]
fn error_state(error: &retention::ClientError) -> &'static str {
    match error {
        retention::ClientError::BinaryUnavailable(_)
        | retention::ClientError::RequestTooLarge(_) => TOOL_UNAVAILABLE,
        retention::ClientError::OutcomeUnknown(_) | retention::ClientError::Refused(_) => {
            OUTCOME_UNKNOWN
        }
    }
}

#[cfg(test)]
fn has_exact_keys<const N: usize>(value: &Value, expected: [&str; N]) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    object.len() == expected.len() && expected.iter().all(|key| object.contains_key(*key))
}

fn append_action(journal_root: &Path, action: &str, params: Value) {
    if let Err(error) =
        solstone_core_facets::append_action_log(journal_root, None, "app", "home", action, params)
    {
        log::warn!("home {action} could not append audit action: {error}");
    }
}

fn list_response(state: &'static str, removals: Vec<Value>, context: Option<String>) -> Response {
    uncached(
        Json(json!({"state": state, "removals": removals, "review_context": context}))
            .into_response(),
    )
}

fn request_response(status: StatusCode, state: &'static str) -> Response {
    (status, Json(json!({"state": state}))).into_response()
}

fn write_response(requested_count: usize, outcome: RemovalOutcome) -> Response {
    Json(json!({
        "state": outcome.state,
        "requested_count": requested_count,
        "removed_count": outcome.removed_count,
        "not_removed_count": outcome.not_removed_count,
        "halted": outcome.halted,
        "refusals": outcome.refusals,
    }))
    .into_response()
}

fn refused_before_start() -> RemovalOutcome {
    RemovalOutcome {
        state: APPROVE_REFUSED_BEFORE_START,
        removed_count: 0,
        not_removed_count: 0,
        halted: false,
        refusals: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use axum::body::Bytes;
    use serde_json::{Value, json};
    use solstone_core_retention_client::RemovalClass;

    use super::{
        APPROVE_DELETED, APPROVE_HALTED, APPROVE_PARTIAL, APPROVE_REFUSED_AFTER_START,
        APPROVE_REFUSED_BEFORE_START, ClientCall, DECLINED_DONE, DECLINED_REFUSED,
        DECLINED_UNKNOWN, LIST_REGISTER_UNAVAILABLE, OUTCOME_UNKNOWN, RECOVER_DONE, RECOVER_FAILED,
        RECOVER_NONE, REFUSAL_ITEM_NAMED, REFUSAL_ITEM_UNNAMED, REQUEST_INVALID, TOOL_UNAVAILABLE,
        decline_payload, error_state, marks_store_refusal, project_mark, recover_empty,
        recover_from_call, recover_outcome, recover_unavailable, recover_unknown, refusal_item,
        removal_outcome, remove_preflight_refusal,
    };

    pub(super) fn review_fixture(
        count: usize,
        reason_bytes: usize,
    ) -> (tempfile::TempDir, Vec<super::retention::Mark>) {
        use solstone_core_retention::{
            Target,
            marks::{Mark, MarkId, MarkState, Proposal, Register},
        };
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("health")).unwrap();
        let marks = (0..count)
            .map(|index| {
                let target = Target {
                    day: "20260101".into(),
                    stream: "_default".into(),
                    dir: format!("{:06}_60", index),
                };
                let names = vec!["audio.flac".into()];
                Mark {
                    id: MarkId::derive(RemovalClass::PolicyRawRelease, &target, &names),
                    class: RemovalClass::PolicyRawRelease,
                    target,
                    marked_at: "2026-01-01T00:00:00Z".into(),
                    proposal: Proposal {
                        bytes: 1,
                        reason: "r".repeat(reason_bytes),
                        names,
                    },
                    state: MarkState::Marked,
                    offload: None,
                }
            })
            .collect::<Vec<_>>();
        let register = Register {
            version: 2,
            marks: marks
                .iter()
                .map(|mark| (mark.id.clone(), mark.clone()))
                .collect(),
        };
        fs::write(
            dir.path().join("health/retention-marks.json"),
            serde_json::to_vec(&register).unwrap(),
        )
        .unwrap();
        (dir, marks)
    }

    #[test]
    fn review_pages_bound_complete_contexts_without_skipping_marks() {
        let (dir, marks) = review_fixture(25, 40_000);
        let mut offset = 0;
        let mut seen = std::collections::BTreeSet::new();
        loop {
            let page = super::rendered_review(dir.path(), offset).unwrap();
            assert_eq!(page.total_rows, marks.len());
            assert_eq!(page.offset, offset);
            let serialized = page.review_context.unwrap();
            assert!(serialized.len() <= super::MAX_REVIEW_CONTEXT_BYTES);
            let context: super::ReviewContext = serde_json::from_str(&serialized).unwrap();
            for mark in context.marks() {
                assert_eq!(
                    Some(mark),
                    marks.iter().find(|original| original.id == mark.id)
                );
                assert!(seen.insert(mark.id.clone()));
            }
            match page.next_offset {
                Some(next) => {
                    assert!(next > offset);
                    offset = next;
                }
                None => break,
            }
        }
        assert_eq!(seen.len(), marks.len());
    }

    #[test]
    fn an_oversized_mark_has_no_consent_context_and_can_be_passed() {
        let (dir, mut marks) = review_fixture(2, super::MAX_REVIEW_CONTEXT_BYTES);
        marks[0].proposal.reason = "small review".into();
        let register = solstone_core_retention::marks::Register {
            version: 2,
            marks: marks
                .into_iter()
                .map(|mark| (mark.id.clone(), mark))
                .collect(),
        };
        fs::write(
            dir.path().join("health/retention-marks.json"),
            serde_json::to_vec(&register).unwrap(),
        )
        .unwrap();
        let first = super::rendered_review(dir.path(), 0).unwrap();
        assert_eq!(first.state, super::LIST_REVIEW_TOO_LARGE);
        assert!(first.review_context.is_none());
        assert_eq!(first.removals.len(), 1);
        assert_eq!(first.next_offset, Some(1));
        let next = super::rendered_review(dir.path(), first.next_offset.unwrap()).unwrap();
        assert_ne!(first.removals[0]["id"], next.removals[0]["id"]);
        assert_eq!(next.state, super::LIST_READY);
        assert!(next.review_context.is_some());
        assert!(next.next_offset.is_none());
    }

    #[test]
    fn paged_review_retains_stale_proposal_refusal() {
        use solstone_core_retention::marks::Register;
        let (dir, mut marks) = review_fixture(21, 10);
        let page = super::rendered_review(dir.path(), 20).unwrap();
        let context: super::ReviewContext =
            serde_json::from_str(&page.review_context.unwrap()).unwrap();
        let id = &context.marks()[0].id;
        marks
            .iter_mut()
            .find(|mark| &mark.id == id)
            .unwrap()
            .proposal
            .reason
            .push_str(" changed");
        let register = Register {
            version: 2,
            marks: marks
                .into_iter()
                .map(|mark| (mark.id.clone(), mark))
                .collect(),
        };
        fs::write(
            dir.path().join("health/retention-marks.json"),
            serde_json::to_vec(&register).unwrap(),
        )
        .unwrap();
        let root = std::sync::Arc::new(super::JournalRoot::open(dir.path()).unwrap());
        assert!(matches!(
            super::check_review_context(root, &context),
            Err(super::ReviewRefusal::Stale { .. })
        ));
    }

    fn receipt(removed: &[&str], entries: &[&str], halted: bool) -> Value {
        let not_removed = entries
            .iter()
            .map(|entry| json!({"entry": entry, "reason": "r", "staged": null}))
            .collect::<Vec<_>>();
        let halted = halted.then(|| json!({"reason": "h"}));
        json!({
            "ok": true,
            "verb": "remove-marked",
            "outcome": {
                "targets": [{"target": {}, "removed": removed, "not_removed": not_removed}],
                "halted": halted,
            },
        })
    }

    fn read_sources(directory: &Path) -> Vec<PathBuf> {
        let mut paths = Vec::new();
        for entry in fs::read_dir(directory).expect("home source directory") {
            let path = entry.expect("home source entry").path();
            if path.is_dir() {
                paths.extend(read_sources(&path));
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                paths.push(path);
            }
        }
        paths
    }

    #[test]
    fn home_source_uses_no_serialized_removal_class_name() {
        let needles = [
            RemovalClass::OwnerSegmentRemoval,
            RemovalClass::OwnerRawRelease,
            RemovalClass::PolicyRawRelease,
            RemovalClass::OffloadRawRelease,
        ]
        .into_iter()
        .map(|class| serde_json::to_value(class).expect("class serializes"))
        .map(|value| value.as_str().expect("class is a string").to_owned())
        .collect::<Vec<_>>();
        for path in read_sources(&PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src")) {
            let source = fs::read_to_string(&path).expect("home source reads");
            for needle in &needles {
                assert!(
                    !source.contains(needle),
                    "home source names a serialized removal class: {}",
                    path.display()
                );
            }
        }
    }

    #[test]
    fn refusal_receipt_discriminators_require_the_pinned_shapes() {
        let marks = json!({"ok": false, "verb": "marks", "error": "e"});
        assert!(marks_store_refusal(&marks));
        assert_eq!(LIST_REGISTER_UNAVAILABLE, "list.register_unavailable");
        assert!(!marks_store_refusal(
            &json!({"ok": false, "verb": "marks", "error": "e", "outcome": {}})
        ));

        let preflight = json!({"ok": false, "verb": "remove-marked", "error": "e"});
        assert!(remove_preflight_refusal(&preflight));
        assert!(!remove_preflight_refusal(
            &json!({"ok": false, "verb": "remove-marked", "error": "e", "outcome": {}})
        ));
    }

    #[test]
    fn removal_receipts_map_every_execution_state_without_owner_copy() {
        let refused = removal_outcome(&receipt(&[], &["chronicle/20260101/070000_17"], false))
            .expect("refusal receipt");
        assert_eq!(refused.state, APPROVE_REFUSED_AFTER_START);
        assert_eq!(refused.not_removed_count, 1);
        assert_eq!(refused.refusals[0]["state"], REFUSAL_ITEM_NAMED);
        assert_eq!(refused.refusals[0]["name"], "070000_17");
        assert_eq!(refused.refusals[0]["reason"], "r");

        let partial = removal_outcome(&receipt(&["a"], &["b"], false)).expect("partial receipt");
        assert_eq!(partial.state, APPROVE_PARTIAL);
        assert_eq!(partial.removed_count, 1);
        assert_eq!(partial.not_removed_count, 1);

        let deleted = removal_outcome(&receipt(&["a"], &[], false)).expect("success receipt");
        assert_eq!(deleted.state, APPROVE_DELETED);

        let halted = removal_outcome(&receipt(&["a"], &[], true)).expect("halt receipt");
        assert_eq!(halted.state, APPROVE_HALTED);
        assert!(halted.halted);

        assert!(removal_outcome(&receipt(&[], &[], false)).is_none());
        assert_eq!(REFUSAL_ITEM_UNNAMED, refusal_item(Some(""), "r")["state"]);
        assert_eq!(refusal_item(Some(""), "r")["reason"], "r");
        assert_eq!(REFUSAL_ITEM_UNNAMED, refusal_item(None, "r")["state"]);
        assert_eq!(APPROVE_REFUSED_BEFORE_START, "approve.refused_before_start");
        assert_eq!(OUTCOME_UNKNOWN, "outcome.unknown");
        assert_eq!(TOOL_UNAVAILABLE, "tool.unavailable");
    }

    #[test]
    fn retained_decline_projects_confirmed_uncertain_and_refused_mutations() {
        use solstone_core_journal_io::ExistingParentLockError;
        use solstone_core_retention::mark_mutation::{
            BoundMutationOutcome, BoundMutationRefusal, ConditionalResult, MutationStatus,
            RegisterUncertainty,
        };
        use solstone_core_retention::marks::Register;
        use solstone_core_retention::review_context::ReviewRefusal;
        use std::io;

        let result = || ConditionalResult {
            status: MutationStatus::Applied,
            register: Register::empty(),
        };
        let confirmed = Ok(BoundMutationOutcome::Published(result()));
        let payload = decline_payload(2, Some(&confirmed));
        assert_eq!(payload["state"], DECLINED_DONE);
        assert_eq!(payload["declined_count"], 2);
        assert_eq!(payload["unknown_count"], 0);
        assert_eq!(payload["mutation"]["publication"], "published");

        let uncertain = Ok(BoundMutationOutcome::PublishedUncertain {
            candidate: result(),
            uncertainty: RegisterUncertainty {
                durability: Some(io::Error::other("private durability path")),
                observation: Some(io::Error::other("private observation path")),
                bindings: vec![ReviewRefusal::RootChanged],
                lock: Some(ExistingParentLockError::NamespaceChanged {
                    path: PathBuf::from("private lock path"),
                }),
            },
        });
        let payload = decline_payload(2, Some(&uncertain));
        assert_eq!(payload["state"], DECLINED_UNKNOWN);
        assert_eq!(payload["declined_count"], 0);
        assert_eq!(payload["refused_count"], 0);
        assert_eq!(payload["unknown_count"], 2);
        let mutation = &payload["mutation"];
        assert_eq!(mutation["publication"], "published_uncertain");
        assert_eq!(mutation["status"], "applied");
        assert_eq!(mutation["durability_error"], "other error");
        assert_eq!(mutation["observation_error"], "other error");
        assert_eq!(mutation["binding_errors"], json!(["root_changed"]));
        assert_eq!(mutation["lock_error"]["kind"], "namespace_changed");
        assert!(!payload.to_string().contains("private"));

        let unchanged = Ok(BoundMutationOutcome::Unchanged(ConditionalResult {
            status: MutationStatus::AlreadyApplied,
            register: Register::empty(),
        }));
        let payload = decline_payload(2, Some(&unchanged));
        assert_eq!(payload["state"], DECLINED_REFUSED);
        assert_eq!(payload["declined_count"], 0);
        assert_eq!(payload["refused_count"], 2);
        assert_eq!(payload["mutation"]["publication"], "unchanged");
        assert_eq!(payload["mutation"]["status"], "already_applied");

        let refused = Err(BoundMutationRefusal::Unsupported);
        let payload = decline_payload(2, Some(&refused));
        assert_eq!(payload["state"], DECLINED_REFUSED);
        assert_eq!(payload["mutation"]["error"]["kind"], "unsupported");

        #[cfg(unix)]
        {
            let refused = Err(BoundMutationRefusal::Write(
                solstone_core_journal_io::atomic::DetailedAtomicError {
                    path: PathBuf::from("private destination"),
                    operation: "rename",
                    source: io::Error::other("private error"),
                    orphan_stage: Some("private stage".into()),
                    cleanup_error: Some(io::Error::other("private cleanup")),
                },
            ));
            let payload = decline_payload(2, Some(&refused));
            assert_eq!(payload["state"], DECLINED_REFUSED);
            assert_eq!(payload["mutation"]["error"]["kind"], "write");
            assert_eq!(payload["mutation"]["error"]["detail"]["orphan_stage"], true);
            assert_eq!(
                payload["mutation"]["error"]["detail"]["cleanup_error"],
                "other error"
            );
            assert!(!payload.to_string().contains("private"));
        }
        let payload = decline_payload(2, None);
        assert_eq!(payload["state"], DECLINED_UNKNOWN);
        assert_eq!(payload["unknown_count"], 2);
        assert_eq!(payload["declined_count"], 0);
        assert!(payload["mutation"].is_null());
    }

    #[test]
    fn client_errors_keep_nothing_ran_distinct_from_unknown_outcomes() {
        assert_eq!(
            error_state(&super::retention::ClientError::BinaryUnavailable(
                "x".to_owned()
            )),
            TOOL_UNAVAILABLE
        );
        assert_eq!(
            error_state(&super::retention::ClientError::RequestTooLarge(
                "x".to_owned()
            )),
            TOOL_UNAVAILABLE
        );
        assert_eq!(
            error_state(&super::retention::ClientError::OutcomeUnknown(
                "x".to_owned()
            )),
            OUTCOME_UNKNOWN
        );
    }

    #[test]
    fn failed_marks_keep_failure_fields_and_marked_marks_do_not_expose_reason() {
        let class = serde_json::to_value(RemovalClass::PolicyRawRelease).expect("class");
        let marked: super::retention::Mark = serde_json::from_value(json!({
            "id": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "class": class,
            "target": {"day": "20260101", "stream": "_default", "dir": "070000_17"},
            "marked_at": "2026-01-01T00:00:00Z",
            "proposal": {"bytes": 12, "reason": "r", "names": ["a", "b"]},
            "state": "marked",
        }))
        .expect("marked row");
        let marked = project_mark(marked).expect("approved marked row");
        assert_eq!(marked["count"], 2);
        assert_eq!(marked["size"], "12 B");
        assert!(marked.get("reason").is_none());

        let class = serde_json::to_value(RemovalClass::OwnerRawRelease).expect("class");
        let failed: super::retention::Mark = serde_json::from_value(json!({
            "id": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "class": class,
            "target": {"day": "20260101", "stream": "_default", "dir": "070000_17"},
            "marked_at": "2026-01-01T00:00:00Z",
            "proposal": {"bytes": 0, "reason": "r", "names": []},
            "state": {"failed": {"at": "2026-01-01T00:00:01Z", "reason": "r", "staged": null}},
        }))
        .expect("failed row");
        let failed = project_mark(failed).expect("failed row");
        assert_eq!(failed["state"], "failed");
        assert!(failed.get("reason").is_some());
        assert_eq!(failed["staged"], Value::Null);
    }

    #[test]
    fn an_offload_proposal_without_backup_proof_is_never_offered() {
        let class = serde_json::to_value(RemovalClass::OffloadRawRelease).expect("class");
        let unbound: super::retention::Mark = serde_json::from_value(json!({
            "id": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
            "class": class,
            "target": {"day": "20260101", "stream": "_default", "dir": "070000_17"},
            "marked_at": "2026-01-01T00:00:00Z",
            "proposal": {"bytes": 12, "reason": "r", "names": ["a"]},
            "state": "marked",
        }))
        .expect("earlier version's offload row");
        assert!(unbound.offload.is_none());
        assert!(project_mark(unbound).is_none());
    }

    fn leftover(day: &str, dir: &str) -> Value {
        json!({
            "target": {"day": day, "stream": "_default", "dir": dir},
            "removed": [],
            "not_removed": [{
                "entry": format!("chronicle/{day}/.removing_{dir}"),
                "reason": "r",
                "staged": format!("chronicle/{day}/.removing_{dir}"),
            }],
        })
    }

    fn finished(day: &str, dir: &str, removed: &[&str]) -> Value {
        json!({
            "target": {"day": day, "stream": "_default", "dir": dir},
            "removed": removed,
            "not_removed": [],
        })
    }

    fn recover_receipt(targets: Value, halted: Value) -> Value {
        json!({
            "ok": true,
            "verb": "recover",
            "outcome": {"targets": targets, "halted": halted},
            "index": {"ok": true, "chunks": 0, "files": 0},
            "detail": {"verb": "recover"},
        })
    }

    #[test]
    fn recover_receipts_contrast_with_the_approve_table() {
        let empty = recover_receipt(json!([]), Value::Null);
        assert!(removal_outcome(&empty).is_none());
        let none = recover_outcome(&empty).expect("empty recover");
        assert_eq!(none.state, RECOVER_NONE);
        assert_eq!(none.finished_count, 0);
        assert_eq!(none.not_finished_count, 0);
        assert!(!none.halted);

        let leftover_only =
            recover_receipt(json!([leftover("20260101", "070000_17")]), Value::Null);
        assert_eq!(
            removal_outcome(&leftover_only)
                .expect("approve leftover")
                .state,
            APPROVE_REFUSED_AFTER_START
        );
        let failed = recover_outcome(&leftover_only).expect("recover leftover");
        assert_eq!(failed.state, RECOVER_FAILED);
        assert_eq!(failed.finished_count, 0);
        assert_eq!(failed.not_finished_count, 1);
        assert!(!failed.halted);

        let mixed = recover_receipt(
            json!([
                finished(
                    "20260101",
                    "070000_17",
                    &["chronicle/20260101/_default/070000_17/a.flac"]
                ),
                leftover("20260102", "080000_17"),
            ]),
            Value::Null,
        );
        assert_eq!(
            removal_outcome(&mixed).expect("approve mixed").state,
            APPROVE_PARTIAL
        );
        let mixed_out = recover_outcome(&mixed).expect("recover mixed");
        assert_eq!(mixed_out.state, RECOVER_FAILED);
        assert_eq!(mixed_out.finished_count, 1);
        assert_eq!(mixed_out.not_finished_count, 1);

        let both_empty =
            recover_receipt(json!([finished("20260101", "070000_17", &[])]), Value::Null);
        assert!(removal_outcome(&both_empty).is_none());
        let done = recover_outcome(&both_empty).expect("finished empty removed");
        assert_eq!(done.state, RECOVER_DONE);
        assert_eq!(done.finished_count, 1);
        assert_eq!(done.not_finished_count, 0);

        let omitted_removed = json!({
            "ok": true,
            "verb": "recover",
            "outcome": {
                "targets": [{"not_removed": []}],
                "halted": null,
            },
        });
        assert!(removal_outcome(&omitted_removed).is_none());
        let omitted = recover_outcome(&omitted_removed).expect("finished without removed");
        assert_eq!(omitted.state, RECOVER_DONE);
        assert_eq!(omitted.finished_count, 1);

        let halted = recover_receipt(json!([]), json!({"reason": "h"}));
        assert_eq!(
            removal_outcome(&halted).expect("approve halt").state,
            APPROVE_HALTED
        );
        let halted_out = recover_outcome(&halted).expect("recover halt");
        assert_eq!(halted_out.state, RECOVER_FAILED);
        assert!(halted_out.halted);
        assert_eq!(halted_out.finished_count, 0);
        assert_eq!(halted_out.not_finished_count, 0);

        let no_outcome = json!({"ok": false, "verb": "recover", "error": "e"});
        assert!(removal_outcome(&no_outcome).is_none());
        assert!(recover_outcome(&no_outcome).is_none());
    }

    #[test]
    fn recover_empty_accepts_only_an_empty_body() {
        assert!(recover_empty(Bytes::new()).is_ok());
        assert!(recover_empty(Bytes::from_static(b"{}")).is_ok());
        assert_eq!(
            recover_empty(Bytes::from_static(br#"{"mark_ids":[]}"#)),
            Err(REQUEST_INVALID)
        );
        assert_eq!(
            recover_empty(Bytes::from_static(b"[]")),
            Err(REQUEST_INVALID)
        );
        assert_eq!(
            recover_empty(Bytes::from_static(b"null")),
            Err(REQUEST_INVALID)
        );
        assert_eq!(
            recover_empty(Bytes::from_static(b"\"x\"")),
            Err(REQUEST_INVALID)
        );
        assert_eq!(
            recover_empty(Bytes::from_static(b"{")),
            Err(REQUEST_INVALID)
        );
    }

    #[test]
    fn recover_call_dispatch_keeps_unavailable_distinct_from_unknown() {
        let unavailable = recover_from_call(ClientCall::Completed(Err(
            super::retention::ClientError::BinaryUnavailable("x".to_owned()),
        )));
        assert_eq!(unavailable.state, TOOL_UNAVAILABLE);
        assert_eq!(unavailable.finished_count, 0);
        assert_eq!(recover_unavailable().state, TOOL_UNAVAILABLE);

        let unknown = recover_from_call(ClientCall::Completed(Err(
            super::retention::ClientError::OutcomeUnknown("x".to_owned()),
        )));
        assert_eq!(unknown.state, OUTCOME_UNKNOWN);
        assert_eq!(unknown.finished_count, 0);
        assert_eq!(recover_unknown().state, OUTCOME_UNKNOWN);

        let join = recover_from_call(ClientCall::Join);
        assert_eq!(join.state, OUTCOME_UNKNOWN);
        assert_eq!(join.finished_count, 0);
    }
}

#[cfg(test)]
mod review_transport_tests {
    use super::*;

    fn row(class: retention::RemovalClass, name: &str) -> retention::Mark {
        let target = solstone_core_retention::receipt::Target {
            day: "20261009".into(),
            stream: "camera".into(),
            dir: "120000_60_suffix".into(),
        };
        let names = vec![name.to_owned()];
        retention::Mark {
            id: MarkId::derive(class, &target, &names),
            class,
            target,
            marked_at: "2026-10-10T00:00:00Z".into(),
            proposal: solstone_core_retention::marks::Proposal {
                names,
                bytes: 9_007_199_254_740_993,
                reason: "rendered".into(),
            },
            state: MarkState::Marked,
            offload: None,
        }
    }

    fn request(marks: &[retention::Mark], ids: &[MarkId]) -> Value {
        let context = json!({"root_observation": "native-root-v1:unix:0000000000000001:0000000000000002", "marks": marks}).to_string();
        json!({"review_context": context, "mark_ids": ids})
    }

    #[test]
    fn selected_transport_retains_complete_rendered_marks_and_ignores_unselected_failures() {
        let selected = row(retention::RemovalClass::PolicyRawRelease, "a.flac");
        let mut failed = row(retention::RemovalClass::PolicyRawRelease, "b.flac");
        failed.state = MarkState::Failed(solstone_core_retention::marks::Failure {
            at: "now".into(),
            reason: "unfinished".into(),
            staged: None,
        });
        let body = request(
            &[selected.clone(), failed.clone()],
            std::slice::from_ref(&selected.id),
        );
        let decoded =
            decode_review_request(body.to_string().as_bytes()).expect("valid rendered subset");
        assert_eq!(decoded.marks(), std::slice::from_ref(&selected));
        assert_eq!(decoded.marks()[0].proposal.bytes, 9_007_199_254_740_993);
        let cases = [
            Vec::new(),
            vec![selected.id.clone(), selected.id.clone()],
            vec![failed.id],
            vec![row(retention::RemovalClass::PolicyRawRelease, "missing.flac").id],
        ];
        for ids in cases {
            let mut invalid = body.clone();
            invalid["mark_ids"] = json!(ids);
            assert_eq!(
                decode_review_request(invalid.to_string().as_bytes()).unwrap_err(),
                REQUEST_INVALID
            );
        }
        let ordinary = row(retention::RemovalClass::OwnerRawRelease, "ordinary.flac");
        let body = request(
            std::slice::from_ref(&ordinary),
            std::slice::from_ref(&ordinary.id),
        );
        assert_eq!(
            decode_review_request(body.to_string().as_bytes()).unwrap_err(),
            REQUEST_INVALID
        );
    }

    #[test]
    fn review_request_strictly_refuses_missing_legacy_duplicate_and_oversized_transport() {
        let selected = row(retention::RemovalClass::PolicyRawRelease, "a.flac");
        let valid = request(
            std::slice::from_ref(&selected),
            std::slice::from_ref(&selected.id),
        );
        let mut unknown = valid.clone();
        unknown["extra"] = json!(true);
        let mut missing = valid.clone();
        missing
            .as_object_mut()
            .expect("request object")
            .remove("review_context");
        let mut malformed = valid.clone();
        malformed["review_context"] = json!("{}");
        let mut object = valid.clone();
        object["review_context"] = json!({});
        for invalid in [unknown, missing, malformed, object] {
            assert_eq!(
                decode_review_request(invalid.to_string().as_bytes()).unwrap_err(),
                REQUEST_INVALID
            );
        }
        let duplicate = format!("{{\"mark_ids\":[],{}", &valid.to_string()[1..]);
        assert_eq!(
            decode_review_request(duplicate.as_bytes()).unwrap_err(),
            REQUEST_INVALID
        );
        let mut oversized = valid.clone();
        oversized["mark_ids"] = json!(vec![selected.id; 33]);
        assert_eq!(
            decode_review_request(oversized.to_string().as_bytes()).unwrap_err(),
            REQUEST_TOO_LARGE
        );
        oversized = valid;
        oversized["review_context"] = json!("x".repeat(MAX_REVIEW_CONTEXT_BYTES + 1));
        assert_eq!(
            decode_review_request(oversized.to_string().as_bytes()).unwrap_err(),
            REQUEST_TOO_LARGE
        );
    }
}
