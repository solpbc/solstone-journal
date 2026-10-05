// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The paired memory tools, reachable only through verified transport authority.

use std::path::Path;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use serde_json::{Map, Value, json};
use solstone_core_callosum::CallosumOneShotSender;
use solstone_core_format::agent_memory::{
    Coordinate, MAX_NOTE_BYTES, SourceKey, digest, validate_operation_id,
};
use solstone_core_indexer_query::{OwnMemoryOpenError, open_own_memory_index, read_own_memory_row};
use solstone_core_mcp_audit::{
    Admission, AuditCoordinates, MemoryResultFacts, Outcome, ResultShape, result_shape,
};
use solstone_core_memory_original::{
    OriginalRead, read_original, read_original_until, read_source_guard,
};

use crate::http1::HttpResponse;
use crate::jsonrpc::{JsonRpcRequest, JsonRpcResponse, ToolName, tool_arguments, tool_result};
use crate::memory::{
    AppendReceipt, AppendResult, AuthenticatedMemorySource, prepare_connection_memory,
};
use crate::memory_recall::{RECALL_DEADLINE, RecallArgs, RecallPage, recall_until};
use crate::references::ReferenceCodec;
use crate::server::MemoryAuthority;

const MAX_PREPARED_BYTES: usize = 512 * 1024;
const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
static REFERENCES: OnceLock<Option<ReferenceCodec>> = OnceLock::new();

fn bounded_json(value: &impl serde::Serialize, maximum: usize) -> Result<Vec<u8>, ()> {
    // The closed result's inputs already cap content, origins, cursor and id.
    // JSON escaping expands each UTF-8 byte by at most six. These fixed input
    // caps bound the intermediate allocations; also check both actual serialized
    // representations before any successful terminal.
    let output = serde_json::to_vec(value).map_err(|_| ())?;
    if output.len() > maximum {
        return Err(());
    }
    Ok(output)
}

fn response_bytes(
    request: &JsonRpcRequest,
    value: Value,
    list_changed: bool,
) -> Result<Vec<u8>, ()> {
    bounded_json(&value, MAX_PREPARED_BYTES)?;
    let response = JsonRpcResponse::success(request.id.as_ref(), tool_result(value, None));
    let response = if list_changed {
        response.with_tools_list_changed()
    } else {
        response
    };
    bounded_json(&response, MAX_RESPONSE_BYTES)
}

fn error_response(request: &JsonRpcRequest, reason: &'static str) -> HttpResponse {
    let response = JsonRpcResponse::tool_error(request.id.as_ref(), reason);
    HttpResponse::json(
        200,
        "OK",
        response
            .to_bytes()
            .expect("bounded error response serializes"),
    )
}

pub(crate) fn execute(
    journal: &Path,
    authority: &MemoryAuthority<'_>,
    request: &JsonRpcRequest,
    tool: ToolName,
    now: DateTime<Utc>,
    list_changed: bool,
) -> HttpResponse {
    #[cfg(all(test, feature = "full-tests"))]
    let now = faults::now(journal, now);
    match tool {
        ToolName::SaveMemory => save(journal, authority, request, now, list_changed),
        ToolName::RecallMemory => recall_response(journal, authority, request, now, list_changed),
        _ => error_response(request, "invalid_memory_tool"),
    }
}

fn invalid_params(request: &JsonRpcRequest) -> HttpResponse {
    HttpResponse::json(
        200,
        "OK",
        JsonRpcResponse::invalid_params(request.id.as_ref())
            .to_bytes()
            .expect("parameter error serializes"),
    )
}

fn save_args(arguments: Option<&Value>) -> Option<(&str, &str)> {
    let arguments = arguments?.as_object()?;
    if arguments.len() != 2 {
        return None;
    }
    let content = arguments.get("content")?.as_str()?;
    let operation = arguments.get("operation_id")?.as_str()?;
    if content.is_empty()
        || content.len() > MAX_NOTE_BYTES
        || validate_operation_id(operation).is_err()
    {
        return None;
    }
    Some((content, operation))
}

fn recall_args(arguments: Option<&Value>) -> Option<RecallArgs> {
    let empty = Map::new();
    let arguments = match arguments {
        None => &empty,
        Some(value) => value.as_object()?,
    };
    if arguments.keys().any(|key| {
        !matches!(
            key.as_str(),
            "query" | "limit" | "day" | "day_from" | "day_to" | "continuation"
        )
    }) {
        return None;
    }
    let string = |name: &str, maximum: usize| -> Option<Option<String>> {
        match arguments.get(name) {
            None => Some(None),
            Some(Value::String(value)) if value.len() <= maximum => Some(Some(value.clone())),
            _ => None,
        }
    };
    let limit = match arguments.get("limit") {
        None => None,
        Some(value) => {
            let value = usize::try_from(value.as_u64()?).ok()?;
            if !(1..=20).contains(&value) {
                return None;
            }
            Some(value)
        }
    };
    let continuation = string("continuation", crate::tools::MAX_OPAQUE_REFERENCE_BYTES)?;
    if continuation.as_ref().is_some_and(String::is_empty) {
        return None;
    }
    Some(RecallArgs {
        query: string("query", crate::tools::search::MAX_QUERY_BYTES)?,
        limit,
        day: string("day", crate::tools::MAX_DAY_BYTES)?,
        day_from: string("day_from", crate::tools::MAX_DAY_BYTES)?,
        day_to: string("day_to", crate::tools::MAX_DAY_BYTES)?,
        continuation,
    })
}

fn uncertain(request: &JsonRpcRequest, operation: &str, list_changed: bool) -> HttpResponse {
    let value = json!({"schema": 1, "status": "uncertain_retry", "operation_id": operation,
        "self_resolution": "retry save_memory with the same operation_id and exact content."});
    match response_bytes(request, value, list_changed) {
        Ok(bytes) => HttpResponse::json(200, "OK", bytes),
        Err(()) => error_response(request, "memory_response_unavailable"),
    }
}

fn save(
    journal: &Path,
    authority: &MemoryAuthority<'_>,
    request: &JsonRpcRequest,
    now: DateTime<Utc>,
    list_changed: bool,
) -> HttpResponse {
    let Some((content, operation)) = save_args(tool_arguments(request)) else {
        return invalid_params(request);
    };
    let prepared = match prepare_connection_memory(
        journal,
        AuthenticatedMemorySource {
            verified_id: authority.id(),
            creation_label: authority.label(),
        },
        content,
        operation,
        now,
    ) {
        Ok(prepared) => prepared,
        Err(error) => return error_response(request, error.wire_reason()),
    };
    let (status, receipt, deleted) = match prepared.result {
        AppendResult::Stored(receipt) => ("stored", receipt, false),
        AppendResult::Replayed(receipt) => ("replayed", receipt, false),
        AppendResult::Deleted(receipt) => ("deleted", receipt, true),
        AppendResult::UncertainRetry { .. } => return uncertain(request, operation, list_changed),
    };
    let (Some(audit), Some(outcome)) = (prepared.audit, prepared.outcome) else {
        return uncertain(request, operation, list_changed);
    };
    #[cfg(all(test, feature = "full-tests"))]
    if faults::before_preparation(journal) {
        return postcommit_failure(journal, &audit, request, operation, now, list_changed);
    }
    if !deleted {
        notify_note(journal, &receipt.coordinate);
    }
    let readiness = if deleted {
        "deleted"
    } else {
        indexed_readiness(journal, &receipt)
    };
    let readable = !deleted
        && readiness == "ready"
        && ordinary_readable(journal, authority.id(), &receipt.coordinate);
    let value = json!({"schema":1, "status":status, "operation_id":operation,
        "coordinate":receipt.coordinate, "origin":receipt.origin, "created_at":receipt.created_at,
        "digest":receipt.digest, "byte_count":receipt.byte_count,
        "own_recall":readiness, "ordinary_readable":readable});
    let shape_bytes = match bounded_json(&value, MAX_PREPARED_BYTES) {
        Ok(bytes) => bytes,
        Err(()) => {
            return postcommit_failure(journal, &audit, request, operation, now, list_changed);
        }
    };
    let mut shape = result_shape(
        1,
        vec![note_path(&receipt.coordinate)],
        digest(&shape_bytes),
    );
    shape.origin = Some(json!(receipt.origin));
    shape.created_at = Some(receipt.created_at);
    shape.byte_count = Some(receipt.byte_count);
    shape.memory = Some(MemoryResultFacts {
        complete: true,
        own_recall_ready: Some(readiness == "ready"),
        ordinary_readable: Some(readable),
    });
    let bytes = match response_bytes(request, value, list_changed) {
        Ok(bytes) => bytes,
        Err(()) => {
            return postcommit_failure(journal, &audit, request, operation, now, list_changed);
        }
    };
    #[cfg(all(test, feature = "full-tests"))]
    faults::before_terminal(journal, &audit, Some(&receipt.coordinate), authority.id());
    // Recheck after all preparation. A changed grant cannot make this receipt
    // release an ordinary read reference; none is minted by this tool.
    if !deleted
        && (!matches!(read_original(journal, &receipt.origin.source_key, &receipt.coordinate),
                OriginalRead::Ready { origin, ready, .. }
                if origin == receipt.origin && ready.digest == receipt.digest && ready.byte_count == receipt.byte_count)
            || (readiness == "ready" && indexed_readiness(journal, &receipt) != "ready")
            || (readable && !ordinary_readable(journal, authority.id(), &receipt.coordinate)))
    {
        return postcommit_failure(journal, &audit, request, operation, now, list_changed);
    }
    if crate::audit::write_outcome(journal, &audit, now, outcome, None, Some(shape)).is_err() {
        return uncertain(request, operation, list_changed);
    }
    HttpResponse::json(200, "OK", bytes)
}

fn postcommit_failure(
    journal: &Path,
    audit: &AuditCoordinates,
    request: &JsonRpcRequest,
    operation: &str,
    now: DateTime<Utc>,
    list_changed: bool,
) -> HttpResponse {
    let _ = crate::audit::write_outcome(
        journal,
        audit,
        now,
        Outcome::Error,
        Some("memory receipt preparation could not be confirmed; retry the same operation"),
        None,
    );
    uncertain(request, operation, list_changed)
}

fn note_path(coordinate: &Coordinate) -> String {
    format!(
        "{}/{}/{}/note.txt",
        coordinate.day, coordinate.stream, coordinate.segment
    )
}

fn indexed_readiness(journal: &Path, receipt: &AppendReceipt) -> &'static str {
    let source = &receipt.origin.source_key;
    let (bytes, origin, ready) = match read_original(journal, source, &receipt.coordinate) {
        OriginalRead::Ready {
            bytes,
            origin,
            ready,
        } => (bytes, origin, ready),
        OriginalRead::Deleted => return "deleted",
        _ => return "unavailable",
    };
    let connection = match open_own_memory_index(journal, Duration::from_secs(5)) {
        Ok(connection) => connection,
        Err(OwnMemoryOpenError::Pending) => return "pending",
        Err(OwnMemoryOpenError::Unavailable) => return "unavailable",
    };
    match read_own_memory_row(
        &connection,
        source.as_str(),
        &note_path(&receipt.coordinate),
    ) {
        Ok(Some(row))
            if crate::dispatch::memory_row_matches_live(
                &row,
                source,
                &receipt.coordinate,
                &bytes,
                &origin,
                &ready,
            ) =>
        {
            "ready"
        }
        Ok(_) => "pending",
        Err(_) => "unavailable",
    }
}

fn ordinary_readable(journal: &Path, connection: &str, coordinate: &Coordinate) -> bool {
    use solstone_core_indexer_query::{AdmittedCategory, ConnectionBoundary};
    let crate::permissions::PermissionDecision::Snapshot(snapshot) =
        crate::permissions::evaluate_connection_read(journal, connection)
    else {
        return false;
    };
    if !snapshot.categories.contains(&AdmittedCategory::Transcripts) {
        return false;
    }
    let path = note_path(coordinate);
    let Ok(declarations) =
        solstone_core_indexer_store::classification::FacetDeclarationSet::from_journal(journal)
    else {
        return false;
    };
    let classification = solstone_core_indexer_store::classification::classify_source(
        journal,
        &path,
        Some(&coordinate.stream),
        &declarations,
    );
    let Ok(boundary) = ConnectionBoundary::from_category_tokens(
        snapshot
            .categories
            .iter()
            .map(crate::registry::category_token),
        snapshot.scope,
    ) else {
        return false;
    };
    boundary.allows_classification(&classification)
}

fn notify_note(journal: &Path, coordinate: &Coordinate) {
    let sender =
        CallosumOneShotSender::new(journal.join("health/callosum.sock"), Duration::from_secs(2));
    let file = journal
        .join("chronicle")
        .join(&coordinate.day)
        .join(&coordinate.stream)
        .join(&coordinate.segment)
        .join("note.txt");
    for event in [
        json!({"tract":"supervisor", "event":"request", "cmd":["journal", "indexer", "--rescan-file", file], "ref":format!("memory-index-{}-{}", coordinate.day, coordinate.segment)}),
        json!({"tract":"observe", "event":"observed", "day":coordinate.day, "stream":coordinate.stream, "segment":coordinate.segment}),
    ] {
        if let Ok(line) = serde_json::to_string(&event) {
            let _ = sender.send_line(&format!("{line}\n"));
        }
    }
}

fn recall_response(
    journal: &Path,
    authority: &MemoryAuthority<'_>,
    request: &JsonRpcRequest,
    now: DateTime<Utc>,
    list_changed: bool,
) -> HttpResponse {
    let Some(args) = recall_args(tool_arguments(request)) else {
        return invalid_params(request);
    };
    let source = SourceKey::from_verified_id(authority.id());
    let mut arguments = Map::new();
    arguments.insert("source_key".into(), json!(source.as_str()));
    arguments.insert("authority".into(), json!("own_memory"));
    for (name, value) in [
        ("query", &args.query),
        ("day", &args.day),
        ("day_from", &args.day_from),
        ("day_to", &args.day_to),
    ] {
        if let Some(value) = value {
            arguments.insert(name.into(), json!(value));
        }
    }
    arguments.insert("limit".into(), json!(args.limit.unwrap_or(5)));
    arguments.insert("continued".into(), json!(args.continuation.is_some()));
    let admission = Admission {
        connection: authority.id(),
        agent_identity: source.as_str(),
        tool_name: solstone_core_mcp_audit::ToolName::RecallMemory,
        arguments,
        permission: None,
    };
    let audit = match crate::audit::write_admitted_interaction(journal, now, &admission) {
        Ok(audit) => audit,
        Err(_) => return error_response(request, "memory_audit_unavailable"),
    };
    let Some(codec) = REFERENCES.get_or_init(|| ReferenceCodec::new().ok()) else {
        let _ = crate::audit::write_outcome(
            journal,
            &audit,
            now,
            Outcome::Error,
            Some("memory cursor initialization is unavailable"),
            None,
        );
        return error_response(request, "memory_response_unavailable");
    };
    let deadline = Instant::now() + RECALL_DEADLINE;
    let mut page = recall_until(
        journal,
        codec,
        authority.id(),
        authority.id(),
        authority.id(),
        args,
        now.with_timezone(&solstone_core_journal_config::owner_zone(journal))
            .date_naive(),
        deadline,
    );
    // Serialization and final live checks share the engine's absolute budget.
    if !page.notes.is_empty() && !page_is_live(journal, &source, &page, deadline) {
        page.notes.clear();
        page.continuation = None;
        page.complete = false;
        page.self_resolution = Some("retry recall or start a fresh query.");
        page.reason = Some(if Instant::now() >= deadline {
            "memory_recall_budget_exhausted"
        } else {
            "memory_source_unavailable"
        });
    }
    let targets = page
        .notes
        .iter()
        .map(|note| note.path.clone())
        .collect::<Vec<_>>();
    let notes = page.notes.iter().map(|note| {
        String::from_utf8(note.bytes.clone()).map(|content| json!({"content":content,
            "day":note.day, "origin":note.origin, "digest":note.ready.digest, "byte_count":note.bytes.len()}))
    }).collect::<Result<Vec<_>, _>>();
    let Ok(notes) = notes else {
        let _ = crate::audit::write_outcome(
            journal,
            &audit,
            now,
            Outcome::Error,
            Some("memory original is not valid UTF-8"),
            None,
        );
        return error_response(
            request,
            if Instant::now() >= deadline {
                "memory_recall_budget_exhausted"
            } else {
                "memory_source_unavailable"
            },
        );
    };
    let count = notes.len();
    let value = json!({"schema":1, "notes":notes, "continuation":page.continuation,
        "complete":page.complete, "reason":page.reason, "query_reason":page.query_reason, "self_resolution":page.self_resolution});
    let shape_bytes = match bounded_json(&value, MAX_PREPARED_BYTES) {
        Ok(bytes) => bytes,
        Err(()) => {
            let _ = crate::audit::write_outcome(
                journal,
                &audit,
                now,
                Outcome::Error,
                Some("memory response could not be prepared"),
                None,
            );
            return error_response(request, "memory_response_unavailable");
        }
    };
    let mut shape: ResultShape = result_shape(count, targets, digest(&shape_bytes));
    shape.memory = Some(MemoryResultFacts {
        complete: page.complete,
        own_recall_ready: None,
        ordinary_readable: None,
    });
    let bytes = match response_bytes(request, value, list_changed) {
        Ok(bytes) => bytes,
        Err(()) => {
            let _ = crate::audit::write_outcome(
                journal,
                &audit,
                now,
                Outcome::Error,
                Some("memory response could not be serialized"),
                None,
            );
            return error_response(request, "memory_response_unavailable");
        }
    };
    #[cfg(all(test, feature = "full-tests"))]
    faults::before_terminal(journal, &audit, None, authority.id());
    if !page.notes.is_empty() && !page_is_live(journal, &source, &page, deadline) {
        let _ = crate::audit::write_outcome(
            journal,
            &audit,
            now,
            Outcome::Error,
            Some("memory originals could not be reconfirmed before release"),
            None,
        );
        return error_response(
            request,
            if Instant::now() >= deadline {
                "memory_recall_budget_exhausted"
            } else {
                "memory_source_unavailable"
            },
        );
    }
    let outcome = if !page.complete {
        Outcome::Error
    } else if count == 0 {
        Outcome::Empty
    } else {
        Outcome::Served
    };
    if crate::audit::write_outcome(journal, &audit, now, outcome, page.reason, Some(shape)).is_err()
    {
        return error_response(request, "memory_audit_unavailable");
    }
    HttpResponse::json(200, "OK", bytes)
}

fn page_is_live(journal: &Path, source: &SourceKey, page: &RecallPage, deadline: Instant) -> bool {
    let Ok(_guard) = read_source_guard(journal, source, deadline) else {
        return false;
    };
    page.notes.iter().all(|note| {
        let coordinate = Coordinate {
            day: note.day.clone(),
            stream: note.origin.stream.clone(),
            segment: note.origin.segment.clone(),
        };
        matches!(read_original_until(journal, source, &coordinate, deadline),
            OriginalRead::Ready { bytes, origin, ready }
            if bytes == note.bytes && origin == note.origin && ready == note.ready)
    }) && Instant::now() < deadline
}

#[cfg(all(test, feature = "full-tests"))]
pub(crate) mod faults {
    use super::*;
    use std::path::PathBuf;
    use std::sync::Mutex;

    #[derive(Clone, Copy)]
    pub enum Fault {
        Preparation,
        Terminal,
        Delete,
        RevokeRead,
    }
    static FAULTS: Mutex<Vec<(PathBuf, Fault)>> = Mutex::new(Vec::new());
    static CLOCKS: Mutex<Vec<(PathBuf, DateTime<Utc>)>> = Mutex::new(Vec::new());

    pub fn set_now(journal: &Path, now: DateTime<Utc>) {
        let mut clocks = CLOCKS.lock().unwrap();
        clocks.retain(|(path, _)| path != journal);
        clocks.push((journal.to_path_buf(), now));
    }
    pub fn now(journal: &Path, default: DateTime<Utc>) -> DateTime<Utc> {
        CLOCKS
            .lock()
            .unwrap()
            .iter()
            .find(|(path, _)| path == journal)
            .map_or(default, |(_, now)| *now)
    }

    pub fn set(journal: &Path, fault: Fault) {
        FAULTS.lock().unwrap().push((journal.to_path_buf(), fault));
    }
    pub fn before_preparation(journal: &Path) -> bool {
        let mut faults = FAULTS.lock().unwrap();
        if let Some(index) = faults
            .iter()
            .position(|(path, fault)| path == journal && matches!(fault, Fault::Preparation))
        {
            faults.remove(index);
            true
        } else {
            false
        }
    }
    pub fn before_terminal(
        journal: &Path,
        audit: &AuditCoordinates,
        coordinate: Option<&Coordinate>,
        connection: &str,
    ) {
        let fault = {
            let mut faults = FAULTS.lock().unwrap();
            faults
                .iter()
                .position(|(path, _)| path == journal)
                .map(|index| faults.remove(index).1)
        };
        match fault {
            Some(Fault::Terminal) => {
                let path = journal
                    .join("chronicle")
                    .join(audit.day.format("%Y%m%d").to_string())
                    .join(&audit.stream)
                    .join(&audit.segment)
                    .join("outcome.json");
                std::fs::create_dir(path).unwrap();
            }
            Some(Fault::Delete) => {
                let coordinate = coordinate.unwrap();
                let target = solstone_core_retention::receipt::Target {
                    day: coordinate.day.clone(),
                    stream: coordinate.stream.clone(),
                    dir: coordinate.segment.clone(),
                };
                let removed = solstone_core_retention::door::remove_segments(
                    journal,
                    &[target],
                    &Utc::now().to_rfc3339(),
                    solstone_core_retention::tombstone::RemovalReason::OwnerSegmentDelete,
                    "journal-owner",
                );
                assert!(removed.removed_paths().next().is_some());
            }
            Some(Fault::RevokeRead) => {
                crate::permissions::PermissionStore::open(journal)
                    .remove_connection(connection)
                    .unwrap();
            }
            Some(Fault::Preparation) => unreachable!(),
            None => {}
        }
    }
}
