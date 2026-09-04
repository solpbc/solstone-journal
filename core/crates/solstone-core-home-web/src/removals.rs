// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::{Path, PathBuf};

use axum::{
    Extension, Json,
    body::Bytes,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use chrono::{Local, SecondsFormat};
use serde_json::{Map, Value, json};
use solstone_core_retention_client as retention;

const LIST_EMPTY: &str = "list.empty";
const LIST_READY: &str = "list.ready";
const LIST_REGISTER_UNAVAILABLE: &str = "list.register_unavailable";
const TOOL_UNAVAILABLE: &str = "tool.unavailable";
const OUTCOME_UNKNOWN: &str = "outcome.unknown";
const REQUEST_INVALID: &str = "request.invalid";
const REQUEST_TOO_LARGE: &str = "request.too_large";
const APPROVE_POLICY_KEEPS: &str = "approve.policy_keeps";
const APPROVE_REFUSED_BEFORE_START: &str = "approve.refused_before_start";
const APPROVE_REFUSED_AFTER_START: &str = "approve.refused_after_start";
const APPROVE_PARTIAL: &str = "approve.partial";
const APPROVE_DELETED: &str = "approve.deleted";
const APPROVE_HALTED: &str = "approve.halted";
const DECLINED_DONE: &str = "declined.done";
const DECLINED_PARTIAL: &str = "declined.partial";
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

pub async fn list(State(journal_root): State<PathBuf>) -> Response {
    match call_marks(journal_root).await {
        ClientCall::Completed(Ok(receipt)) => match project_register(&receipt) {
            Some(removals) if removals.is_empty() => list_response(LIST_EMPTY, removals),
            Some(removals) => list_response(LIST_READY, removals),
            None => list_response(OUTCOME_UNKNOWN, Vec::new()),
        },
        ClientCall::Completed(Err(retention::ClientError::Refused(refused)))
            if marks_store_refusal(refused.receipt_value()) =>
        {
            list_response(LIST_REGISTER_UNAVAILABLE, Vec::new())
        }
        ClientCall::Completed(Err(error)) => list_response(error_state(&error), Vec::new()),
        ClientCall::Join => list_response(OUTCOME_UNKNOWN, Vec::new()),
    }
}

pub async fn approve(
    State(journal_root): State<PathBuf>,
    Extension(clock): Extension<crate::Clock>,
    body: Bytes,
) -> Response {
    let mark_ids = match mark_ids(body) {
        Ok(mark_ids) => mark_ids,
        Err(state) => return request_response(StatusCode::BAD_REQUEST, state),
    };
    if mark_ids.len() > retention::MAX_REMOVE_MARK_IDS {
        return request_response(StatusCode::BAD_REQUEST, REQUEST_TOO_LARGE);
    }
    let policy = policy(&journal_root);
    if !retention::policy_would_release(&policy) {
        return write_response(
            APPROVE_POLICY_KEEPS,
            mark_ids.len(),
            RemovalOutcome {
                state: APPROVE_POLICY_KEEPS,
                removed_count: 0,
                not_removed_count: 0,
                halted: false,
                refusals: Vec::new(),
            },
        );
    }
    let policy = serde_json::to_string(&policy).expect("retention policy serializes");
    let now = clock.now();
    let call = call_remove_marked(
        journal_root.clone(),
        now.with_timezone(&Local).format("%Y-%m-%d").to_string(),
        now.to_rfc3339_opts(SecondsFormat::Secs, true),
        policy,
        mark_ids.clone(),
    )
    .await;
    let outcome = approve_outcome(call);
    let requested_count = mark_ids.len();
    append_action(
        &journal_root,
        "removal_approve",
        json!({
            "mark_ids": mark_ids.clone(),
            "requested_count": requested_count,
            "removed_count": outcome.removed_count,
            "not_removed_count": outcome.not_removed_count,
            "halted": outcome.halted,
            "outcome_state": outcome.state,
        }),
    );
    write_response(outcome.state, requested_count, outcome)
}

pub async fn decline(State(journal_root): State<PathBuf>, body: Bytes) -> Response {
    let mark_ids = match mark_ids(body) {
        Ok(mark_ids) => mark_ids,
        Err(state) => return request_response(StatusCode::BAD_REQUEST, state),
    };
    if mark_ids.len() > retention::MAX_REMOVE_MARK_IDS {
        return request_response(StatusCode::BAD_REQUEST, REQUEST_TOO_LARGE);
    }

    let mut declined_count = 0usize;
    let mut refused_count = 0usize;
    let mut unavailable_count = 0usize;
    let mut unknown_count = 0usize;
    for mark_id in &mark_ids {
        match call_decline(journal_root.clone(), mark_id.clone()).await {
            ClientCall::Completed(Ok(receipt)) if decline_success(&receipt) => {
                declined_count = declined_count.saturating_add(1);
            }
            ClientCall::Completed(Err(retention::ClientError::Refused(refused)))
                if decline_refusal(refused.receipt_value()) =>
            {
                refused_count = refused_count.saturating_add(1);
            }
            ClientCall::Completed(Err(retention::ClientError::BinaryUnavailable(_)))
            | ClientCall::Completed(Err(retention::ClientError::RequestTooLarge(_))) => {
                unavailable_count = unavailable_count.saturating_add(1);
            }
            ClientCall::Completed(Err(retention::ClientError::OutcomeUnknown(_)))
            | ClientCall::Completed(Err(retention::ClientError::Refused(_)))
            | ClientCall::Completed(Ok(_))
            | ClientCall::Join => {
                unknown_count = unknown_count.saturating_add(1);
            }
        }
    }

    let state = decline_state(
        declined_count,
        refused_count,
        unavailable_count,
        unknown_count,
    );
    let requested_count = mark_ids.len();
    append_action(
        &journal_root,
        "removal_decline",
        json!({
            "mark_ids": mark_ids.clone(),
            "requested_count": requested_count,
            "declined_count": declined_count,
            "refused_count": refused_count,
            "unavailable_count": unavailable_count,
            "unknown_count": unknown_count,
            "outcome_state": state,
        }),
    );
    Json(json!({
        "state": state,
        "requested_count": requested_count,
        "declined_count": declined_count,
        "refused_count": refused_count,
        "unavailable_count": unavailable_count,
        "unknown_count": unknown_count,
    }))
    .into_response()
}

pub async fn recover(
    State(journal_root): State<PathBuf>,
    Extension(clock): Extension<crate::Clock>,
    body: Bytes,
) -> Response {
    if let Err(state) = recover_empty(body) {
        return request_response(StatusCode::BAD_REQUEST, state);
    }
    let at = clock.now().to_rfc3339_opts(SecondsFormat::Secs, true);
    let outcome = recover_from_call(call_recover(journal_root.clone(), at).await);
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
    Json(json!({
        "state": outcome.state,
        "finished_count": outcome.finished_count,
        "not_finished_count": outcome.not_finished_count,
        "halted": outcome.halted,
        "refusals": outcome.refusals,
    }))
    .into_response()
}

fn mark_ids(body: Bytes) -> Result<Vec<String>, &'static str> {
    let Value::Object(request) =
        serde_json::from_slice::<Value>(&body).map_err(|_| REQUEST_INVALID)?
    else {
        return Err(REQUEST_INVALID);
    };
    if request.len() != 1 {
        return Err(REQUEST_INVALID);
    }
    let Some(Value::Array(mark_ids)) = request.get("mark_ids") else {
        return Err(REQUEST_INVALID);
    };
    if mark_ids.is_empty() {
        return Err(REQUEST_INVALID);
    }
    mark_ids
        .iter()
        .map(|mark_id| mark_id.as_str().map(str::to_owned).ok_or(REQUEST_INVALID))
        .collect()
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

fn policy(journal_root: &Path) -> retention::Policy {
    let config = solstone_core_journal_config::read_journal_config(journal_root)
        .expect("session gate handled configuration")
        .config
        .unwrap_or_default();
    retention::policy_from_retention(
        config
            .get("retention")
            .and_then(Value::as_object)
            .unwrap_or(&Map::new()),
    )
}

async fn call_marks(journal_root: PathBuf) -> ClientCall {
    match tokio::task::spawn_blocking(move || retention::marks(journal_root)).await {
        Ok(result) => ClientCall::Completed(result),
        Err(_) => ClientCall::Join,
    }
}

async fn call_remove_marked(
    journal_root: PathBuf,
    today: String,
    now: String,
    policy: String,
    mark_ids: Vec<String>,
) -> ClientCall {
    match tokio::task::spawn_blocking(move || {
        retention::remove_marked(journal_root, today, now, policy, &mark_ids)
    })
    .await
    {
        Ok(result) => ClientCall::Completed(result),
        Err(_) => ClientCall::Join,
    }
}

async fn call_decline(journal_root: PathBuf, mark_id: String) -> ClientCall {
    match tokio::task::spawn_blocking(move || retention::decline(journal_root, mark_id)).await {
        Ok(result) => ClientCall::Completed(result),
        Err(_) => ClientCall::Join,
    }
}

async fn call_recover(journal_root: PathBuf, at: String) -> ClientCall {
    match tokio::task::spawn_blocking(move || retention::recover(journal_root, at)).await {
        Ok(result) => ClientCall::Completed(result),
        Err(_) => ClientCall::Join,
    }
}

fn project_register(receipt: &Value) -> Option<Vec<Value>> {
    let register = receipt.get("marks")?.as_object()?;
    register.get("version")?.as_u64()?;
    let marks = register.get("marks")?.as_object()?;
    let mut removals = Vec::new();
    for value in marks.values() {
        let mark = serde_json::from_value::<retention::Mark>(value.clone()).ok()?;
        if let Some(row) = project_mark(mark) {
            removals.push(row);
        }
    }
    Some(removals)
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
    match mark.state {
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

fn marks_store_refusal(receipt: &Value) -> bool {
    has_exact_keys(receipt, ["ok", "verb", "error"])
        && receipt.get("ok") == Some(&Value::Bool(false))
        && receipt.get("verb") == Some(&Value::String("marks".to_owned()))
        && receipt.get("error").and_then(Value::as_str).is_some()
}

fn approve_outcome(call: ClientCall) -> RemovalOutcome {
    match call {
        ClientCall::Completed(Ok(receipt)) => {
            removal_outcome(&receipt).unwrap_or_else(unknown_outcome)
        }
        ClientCall::Completed(Err(retention::ClientError::Refused(refused))) => {
            let receipt = refused.receipt_value();
            if remove_preflight_refusal(receipt) {
                refused_before_start()
            } else {
                removal_outcome(receipt).unwrap_or_else(unknown_outcome)
            }
        }
        ClientCall::Completed(Err(error)) => error_outcome(&error),
        ClientCall::Join => unknown_outcome(),
    }
}

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

fn error_outcome(error: &retention::ClientError) -> RemovalOutcome {
    let state = error_state(error);
    RemovalOutcome {
        state,
        removed_count: 0,
        not_removed_count: 0,
        halted: false,
        refusals: Vec::new(),
    }
}

fn unknown_outcome() -> RemovalOutcome {
    RemovalOutcome {
        state: OUTCOME_UNKNOWN,
        removed_count: 0,
        not_removed_count: 0,
        halted: false,
        refusals: Vec::new(),
    }
}

fn error_state(error: &retention::ClientError) -> &'static str {
    match error {
        retention::ClientError::BinaryUnavailable(_)
        | retention::ClientError::RequestTooLarge(_) => TOOL_UNAVAILABLE,
        retention::ClientError::OutcomeUnknown(_) | retention::ClientError::Refused(_) => {
            OUTCOME_UNKNOWN
        }
    }
}

fn decline_success(receipt: &Value) -> bool {
    receipt.get("ok") == Some(&Value::Bool(true))
        && receipt.get("verb") == Some(&Value::String("decline".to_owned()))
        && receipt.get("marks").and_then(Value::as_object).is_some()
}

fn decline_refusal(receipt: &Value) -> bool {
    has_exact_keys(receipt, ["ok", "verb", "error"])
        && receipt.get("ok") == Some(&Value::Bool(false))
        && receipt.get("verb") == Some(&Value::String("decline".to_owned()))
        && receipt.get("error").and_then(Value::as_str).is_some()
}

fn decline_state(
    declined_count: usize,
    refused_count: usize,
    unavailable_count: usize,
    unknown_count: usize,
) -> &'static str {
    if unknown_count > 0 {
        DECLINED_UNKNOWN
    } else if declined_count == 0 && unavailable_count > 0 && refused_count == 0 {
        TOOL_UNAVAILABLE
    } else if declined_count > 0 && (refused_count > 0 || unavailable_count > 0) {
        DECLINED_PARTIAL
    } else if declined_count > 0 {
        DECLINED_DONE
    } else {
        DECLINED_REFUSED
    }
}

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

fn list_response(state: &'static str, removals: Vec<Value>) -> Response {
    Json(json!({"state": state, "removals": removals})).into_response()
}

fn request_response(status: StatusCode, state: &'static str) -> Response {
    (status, Json(json!({"state": state}))).into_response()
}

fn write_response(
    state: &'static str,
    requested_count: usize,
    outcome: RemovalOutcome,
) -> Response {
    Json(json!({
        "state": state,
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
        APPROVE_REFUSED_BEFORE_START, ClientCall, DECLINED_DONE, DECLINED_PARTIAL,
        DECLINED_REFUSED, DECLINED_UNKNOWN, LIST_REGISTER_UNAVAILABLE, OUTCOME_UNKNOWN,
        RECOVER_DONE, RECOVER_FAILED, RECOVER_NONE, REFUSAL_ITEM_NAMED, REFUSAL_ITEM_UNNAMED,
        REQUEST_INVALID, TOOL_UNAVAILABLE, decline_state, decline_success, error_state,
        marks_store_refusal, project_mark, recover_empty, recover_from_call, recover_outcome,
        recover_unavailable, recover_unknown, refusal_item, removal_outcome,
        remove_preflight_refusal,
    };

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
        assert!(decline_success(
            &json!({"ok": true, "verb": "decline", "marks": {}})
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
    fn sequential_decline_aggregation_keeps_unknown_dominant() {
        assert_eq!(decline_state(2, 0, 0, 0), DECLINED_DONE);
        assert_eq!(decline_state(1, 1, 0, 0), DECLINED_PARTIAL);
        assert_eq!(decline_state(0, 2, 0, 0), DECLINED_REFUSED);
        assert_eq!(decline_state(0, 0, 1, 0), TOOL_UNAVAILABLE);
        assert_eq!(decline_state(1, 0, 0, 1), DECLINED_UNKNOWN);
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
