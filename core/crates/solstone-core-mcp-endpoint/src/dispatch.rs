// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Shared, transport-independent execution of authenticated MCP tool calls.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::sync::OnceLock;

use chrono::{DateTime, Utc};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use solstone_core_indexer_query::{
    AdmittedCategory, ConnectionBoundary, ConnectionScope, ConnectionSearchRequest,
    ConnectionStartAfter, IndexedEntry, QueryBoundary, read_indexed_entry, search_connection,
};
use solstone_core_indexer_store::classification::{FacetDeclarationSet, classify_source};
use solstone_core_journal_io::paths::{PathOrDay, iter_segments};

use solstone_core_mcp_audit::{Admission, AuditCoordinates, Outcome, result_shape};

use crate::audit;
use crate::permissions::{ConnectionReadSnapshot, PermissionDecision, evaluate_connection_read};
use crate::references::{
    CursorReference, EntityReference, EntryReference, ReferenceCodec, ReferenceKind,
    ReferenceTarget, SegmentReference,
};
use crate::registry::{ToolEntry, category_token, find_tool, snapshot_allows};
use crate::tools::{
    ToolError, ValidatedFetch, ValidatedGetEntity, ValidatedGetTranscript, ValidatedListEntities,
    ValidatedListFacets, ValidatedListTranscripts, ValidatedSearch, ValidatedTool,
    execute_after_audit,
};

/// Upper bound on indexed rows examined to fill one search page after live drops.
pub(crate) const MAX_SEARCH_EXAMINED_ROWS: usize = 1_000;
const MAX_INDEXED_ENTRY_BYTES: u64 = 64 * 1024;
const MAX_SNIPPET_CHARS: usize = 800;

static REFERENCES: OnceLock<ReferenceCodec> = OnceLock::new();

/// A prepared response together with the owner coordinates it was built from.
///
/// ⚠ Those coordinates are deliberately absent from `value`: an agent-visible
/// response carries no path, chunk index, row id or stream (increment B), while
/// the owner's log carries exactly those. One prepared value, two audiences.
pub(crate) struct Prepared {
    value: Value,
    count: usize,
    targets: Vec<String>,
}

impl Prepared {
    fn new(value: Value, count: usize, targets: Vec<String>) -> Self {
        Self {
            value,
            count,
            targets,
        }
    }
}

/// The authenticated connection identity supplied by either wire or probe.
#[derive(Clone, Copy)]
pub(crate) struct DispatchPrincipal<'a> {
    pub(crate) connection: &'a str,
    pub(crate) agent_identity: &'a str,
}

/// Transport-neutral result classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DispatchError {
    InvalidInput,
    PermissionDenied(&'static str),
    Tool(ToolError),
}

/// Public probe failure shape. It never contains journal coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpProbeError {
    InvalidTool,
    InvalidInput,
    PermissionDenied,
    Unavailable,
}

/// Invoke the exact wire dispatch sequence without creating a listener or socket.
pub fn run_mcp_probe(
    journal_root: &Path,
    connection: &str,
    tool: &str,
    arguments: &Value,
) -> Result<Value, McpProbeError> {
    let entry = crate::registry::tool_by_wire_name(tool).ok_or(McpProbeError::InvalidTool)?;
    dispatch_authenticated_tool_call(
        journal_root,
        DispatchPrincipal {
            connection,
            agent_identity: connection,
        },
        entry.tool_name,
        Some(arguments),
        Utc::now(),
    )
    .map_err(|error| match error {
        DispatchError::InvalidInput => McpProbeError::InvalidInput,
        DispatchError::PermissionDenied(_) => McpProbeError::PermissionDenied,
        DispatchError::Tool(_) => McpProbeError::Unavailable,
    })
}

/// Run the only authenticated known-tool sequence. Wire and CLI probe call this
/// function; no caller may read before it has admitted the current snapshot.
pub(crate) fn dispatch_authenticated_tool_call(
    journal_root: &Path,
    principal: DispatchPrincipal<'_>,
    tool_name: crate::jsonrpc::ToolName,
    arguments: Option<&Value>,
    now: DateTime<Utc>,
) -> Result<Value, DispatchError> {
    let entry = find_tool(tool_name);
    // ⚠ Validation precedes admission, unchanged: malformed protocol traffic
    // stays outside the per-agent tool trail, and only a syntactically valid
    // known tool call earns a record.
    let validated = validate(entry, arguments)?;
    let recorded = recorded_arguments(&validated);

    let snapshot = match evaluate_connection_read(journal_root, principal.connection) {
        PermissionDecision::Snapshot(snapshot) => snapshot,
        PermissionDecision::Denied { reason } => {
            let coordinates = audit_call(journal_root, now, principal, entry, recorded)?;
            record_outcome(
                journal_root,
                &coordinates,
                now,
                Outcome::Refused,
                Some(owner_denial_reason(reason)),
                None,
            );
            return Err(DispatchError::PermissionDenied(reason));
        }
    };

    if let Some(refusal) = scope_refusal(&snapshot, entry, &validated) {
        // Missing categories are permission denials and are still durable audit
        // events — now ones an owner can tell apart from a served call.
        let coordinates = audit_call(journal_root, now, principal, entry, recorded)?;
        record_outcome(
            journal_root,
            &coordinates,
            now,
            Outcome::Refused,
            Some(&refusal),
            None,
        );
        return Err(DispatchError::PermissionDenied("no_permission"));
    }

    let (coordinates, executed) = execute_after_audit(
        || audit_call(journal_root, now, principal, entry, recorded),
        || execute_validated(journal_root, principal, &snapshot, validated),
    )?;

    let prepared = match executed {
        Ok(prepared) => prepared,
        Err(error) => {
            record_outcome(
                journal_root,
                &coordinates,
                now,
                Outcome::Error,
                Some(owner_error_reason(error)),
                None,
            );
            return Err(error);
        }
    };

    // A grant can be narrowed while a bounded read is in flight. Do not release
    // any prepared bytes after that change.
    if !permission_generation_is_current(journal_root, principal.connection, &snapshot) {
        record_outcome(
            journal_root,
            &coordinates,
            now,
            Outcome::Refused,
            Some("this connection's permission changed while the response was being prepared"),
            None,
        );
        return Err(DispatchError::PermissionDenied("no_permission"));
    }

    // 🔑 The digest is what makes replay a checkable claim rather than an
    // assumed one: re-fetch the named targets, compare, and a difference means
    // the journal actually moved. See `content_digest` for why it cannot be
    // taken over the released bytes.
    let shape = result_shape(
        prepared.count,
        prepared.targets,
        content_digest(&prepared.value),
    );
    let outcome = if prepared.count == 0 {
        Outcome::Empty
    } else {
        Outcome::Served
    };
    // ⚠ The outcome record gates release. A prepared response whose outcome
    // cannot be recorded is refused rather than served: that widens fail-closed,
    // ⛔ it does not relax it. The admission stays, so the call reads uncertain.
    audit::write_outcome(journal_root, &coordinates, now, outcome, None, Some(shape))
        .map_err(|_| DispatchError::Tool(ToolError::AuditUnavailable))?;

    Ok(prepared.value)
}

/// Record a terminal outcome for a call that is already failing.
///
/// ⚠ Best effort, deliberately: the call's result is decided, and a second
/// failure here can only degrade the owner's reading from `refused` or `error`
/// to `uncertain`. ⛔ It can never turn a refusal into a release — that
/// direction is the one guarded by `?` at the serving gate.
fn record_outcome(
    journal_root: &Path,
    coordinates: &AuditCoordinates,
    now: DateTime<Utc>,
    outcome: Outcome,
    reason: Option<&str>,
    result: Option<solstone_core_mcp_audit::ResultShape>,
) {
    let _ = audit::write_outcome(journal_root, coordinates, now, outcome, reason, result);
}

/// The owner-visible reason a call was refused, or `None` when it is allowed.
///
/// 🔑 These strings reach the owner's log and nothing else. On the wire every
/// one of them renders as the same closed `no_permission`, which is the
/// asymmetry this design turns on: the same fact is deliberately hidden from
/// the agent and deliberately visible to the owner, and ⛔ fixing one must not
/// weaken the other.
fn scope_refusal(
    snapshot: &ConnectionReadSnapshot,
    entry: &ToolEntry,
    request: &ValidatedTool,
) -> Option<String> {
    if !snapshot_allows(snapshot, entry) {
        let missing = entry
            .requires
            .read
            .iter()
            .filter(|category| !snapshot.categories.contains(category))
            .map(category_token)
            .collect::<Vec<_>>()
            .join(" and ");
        return Some(format!(
            "this connection does not have the {missing} category"
        ));
    }
    if !arguments_within_scope(snapshot, request) {
        return Some(
            "the request named a category or facet outside this connection's permission".to_owned(),
        );
    }
    None
}

fn owner_denial_reason(reason: &'static str) -> &'static str {
    match reason {
        "unenforceable" => {
            "the stored permission for this connection could not be enforced by this build"
        }
        _ => "this connection has no read permission",
    }
}

/// ⛔ **No failure reason may contain an outcome word.** `served`, `empty`,
/// `refused`, `error` and `uncertain` are the five values an owner filters on,
/// so a reason reading *"the connection's permission refused the request"* on
/// the `error` path means `--outcome refused` does not return the row whose own
/// text says refused: the surface that exists to say how a request ended
/// disagrees with itself about the ending.
///
/// ⚠ The two non-tool arms are unreachable from the one call site: an executor
/// returns only [`ToolError`], because invalid input is rejected before
/// admission and permission is decided above. They are kept exhaustive and
/// worded accurately rather than left to imply a path that does not exist.
///
/// ⛔ **Never name the journal's derived index as "the journal index".**
/// `cmo/brand/system-anatomy.md` puts "the index" on the journal's never-list by
/// name, and this is the failure the ban exists for: an owner reading *"the
/// journal index is empty"* reads it as **their journal** being empty, when a
/// full journal with an unbuilt search index reports exactly that. ✅ Name the
/// mechanism — the search index — and leave the journal out of the compound.
fn owner_error_reason(error: DispatchError) -> &'static str {
    match error {
        DispatchError::Tool(ToolError::IndexAbsent) => "the search index does not exist",
        DispatchError::Tool(ToolError::IndexUnreadable) => "the search index could not be read",
        DispatchError::Tool(ToolError::IndexLocked) => "the search index was locked",
        DispatchError::Tool(ToolError::EmptyIndex) => "the search index is empty",
        DispatchError::Tool(ToolError::NotIndexed) => "the requested material is not indexed",
        DispatchError::Tool(ToolError::FileUnreadable) => "a journal file could not be read",
        DispatchError::Tool(ToolError::ReferenceNotFound) => {
            "the reference did not resolve to anything this connection may read"
        }
        DispatchError::Tool(ToolError::AuditUnavailable) => "the audit record could not be written",
        DispatchError::Tool(ToolError::InvalidInput) | DispatchError::InvalidInput => {
            "the request's arguments were not valid for this tool"
        }
        DispatchError::PermissionDenied(_) => {
            "the request was outside this connection's permission"
        }
    }
}

/// What the owner's log records about the request itself.
///
/// 🔒 Directive 1: the query and arguments are recorded. What was asked of an
/// owner's memory is the owner's right to know, not a privacy trade-off.
fn recorded_arguments(validated: &ValidatedTool) -> Map<String, Value> {
    let mut arguments = Map::new();
    match validated {
        ValidatedTool::ListFacets(request) => {
            arguments.insert("limit".to_owned(), request.limit.into());
        }
        ValidatedTool::Search(request) => {
            arguments.insert("query".to_owned(), request.query.clone().into());
            arguments.insert("limit".to_owned(), request.limit.into());
            insert_present(&mut arguments, "day", request.day.as_deref());
            insert_present(&mut arguments, "day_from", request.day_from.as_deref());
            insert_present(&mut arguments, "day_to", request.day_to.as_deref());
            insert_present(&mut arguments, "facet", request.facet_id.as_deref());
            if let Some(category) = request.category {
                arguments.insert("category".to_owned(), category_token(&category).into());
            }
            if let Some(cursor) = request.cursor.as_deref() {
                arguments.insert("cursor".to_owned(), reference_fingerprint(cursor).into());
            }
        }
        ValidatedTool::Fetch(request) => {
            arguments.insert(
                "reference".to_owned(),
                reference_fingerprint(&request.reference).into(),
            );
        }
        ValidatedTool::ListTranscripts(request) => {
            arguments.insert("limit".to_owned(), request.limit.into());
            insert_present(&mut arguments, "day", request.day.as_deref());
            insert_present(&mut arguments, "facet", request.facet_id.as_deref());
        }
        ValidatedTool::GetTranscript(request) => {
            arguments.insert(
                "reference".to_owned(),
                reference_fingerprint(&request.reference).into(),
            );
            if let Some(cursor) = request.cursor.as_deref() {
                arguments.insert("cursor".to_owned(), reference_fingerprint(cursor).into());
            }
        }
        ValidatedTool::ListEntities(request) => {
            arguments.insert("limit".to_owned(), request.limit.into());
            insert_present(&mut arguments, "facet", request.facet_id.as_deref());
        }
        ValidatedTool::GetEntity(request) => {
            arguments.insert(
                "reference".to_owned(),
                reference_fingerprint(&request.reference).into(),
            );
        }
    }
    arguments
}

fn insert_present(arguments: &mut Map<String, Value>, key: &str, value: Option<&str>) {
    if let Some(value) = value {
        arguments.insert(key.to_owned(), value.into());
    }
}

/// A short, stable stand-in for an opaque reference argument.
///
/// ⛔ The token itself is never recorded. It is an encrypted, process-local
/// capability bound to one connection and grant generation, so its bytes tell
/// an owner nothing and stop meaning anything at the next restart. ✅ The
/// fingerprint distinguishes two calls, and the outcome record names the target
/// the reference resolved to, in the owner's own coordinates.
fn reference_fingerprint(token: &str) -> String {
    digest(token).chars().take(16).collect()
}

pub(crate) fn permission_generation_is_current(
    journal_root: &Path,
    connection: &str,
    snapshot: &ConnectionReadSnapshot,
) -> bool {
    matches!(
        evaluate_connection_read(journal_root, connection),
        PermissionDecision::Snapshot(current) if current.generation == snapshot.generation
    )
}

fn validate(entry: &ToolEntry, arguments: Option<&Value>) -> Result<ValidatedTool, DispatchError> {
    let validated = match entry.tool_name {
        crate::jsonrpc::ToolName::ListFacets => {
            crate::tools::facets::validate(arguments).map(ValidatedTool::ListFacets)
        }
        crate::jsonrpc::ToolName::Search => {
            crate::tools::search::validate(arguments).map(ValidatedTool::Search)
        }
        crate::jsonrpc::ToolName::Fetch => {
            crate::tools::fetch::validate(arguments).map(ValidatedTool::Fetch)
        }
        crate::jsonrpc::ToolName::ListTranscripts => {
            crate::tools::transcripts::validate_list(arguments).map(ValidatedTool::ListTranscripts)
        }
        crate::jsonrpc::ToolName::GetTranscript => {
            crate::tools::transcripts::validate_get(arguments).map(ValidatedTool::GetTranscript)
        }
        crate::jsonrpc::ToolName::ListEntities => {
            crate::tools::entities::validate_list(arguments).map(ValidatedTool::ListEntities)
        }
        crate::jsonrpc::ToolName::GetEntity => {
            crate::tools::entities::validate_get(arguments).map(ValidatedTool::GetEntity)
        }
    };
    validated.map_err(|error| match error {
        ToolError::InvalidInput => DispatchError::InvalidInput,
        error => DispatchError::Tool(error),
    })
}

fn audit_call(
    journal_root: &Path,
    now: DateTime<Utc>,
    principal: DispatchPrincipal<'_>,
    entry: &ToolEntry,
    arguments: Map<String, Value>,
) -> Result<AuditCoordinates, DispatchError> {
    audit::write_admitted_interaction(
        journal_root,
        now,
        &Admission {
            connection: principal.connection,
            agent_identity: principal.agent_identity,
            tool_name: entry.audit_name,
            arguments,
        },
    )
    .map_err(|_| DispatchError::Tool(ToolError::AuditUnavailable))
}

fn execute_validated(
    journal_root: &Path,
    principal: DispatchPrincipal<'_>,
    snapshot: &ConnectionReadSnapshot,
    validated: ValidatedTool,
) -> Result<Prepared, DispatchError> {
    match validated {
        ValidatedTool::ListFacets(request) => list_facets(journal_root, snapshot, request),
        ValidatedTool::Search(request) => search(journal_root, principal, snapshot, request),
        ValidatedTool::Fetch(request) => fetch(journal_root, principal, snapshot, request),
        ValidatedTool::ListTranscripts(request) => {
            list_transcripts(journal_root, principal, snapshot, request)
        }
        ValidatedTool::GetTranscript(request) => {
            get_transcript(journal_root, principal, snapshot, request)
        }
        ValidatedTool::ListEntities(request) => {
            list_entities(journal_root, principal, snapshot, request)
        }
        ValidatedTool::GetEntity(request) => get_entity(journal_root, principal, snapshot, request),
    }
}

fn arguments_within_scope(snapshot: &ConnectionReadSnapshot, request: &ValidatedTool) -> bool {
    let requested_facet = match request {
        ValidatedTool::Search(request) => request.facet_id.as_deref(),
        ValidatedTool::ListTranscripts(request) => request.facet_id.as_deref(),
        ValidatedTool::ListEntities(request) => request.facet_id.as_deref(),
        _ => None,
    };
    let facet_ok = requested_facet.is_none_or(|id| match &snapshot.scope {
        ConnectionScope::WholeJournal => true,
        ConnectionScope::ChosenFacets { ids } => ids.contains(id),
    });
    let category_ok = match request {
        ValidatedTool::Search(request) => request
            .category
            .is_none_or(|category| snapshot.categories.contains(&category)),
        _ => true,
    };
    facet_ok && category_ok
}

fn boundary(
    snapshot: &ConnectionReadSnapshot,
    category: Option<AdmittedCategory>,
    facet: Option<&str>,
) -> ConnectionBoundary {
    let categories = category
        .map(|category| BTreeSet::from([category]))
        .unwrap_or_else(|| snapshot.categories.clone());
    let scope = facet.map_or_else(
        || snapshot.scope.clone(),
        |id| ConnectionScope::ChosenFacets {
            ids: BTreeSet::from([id.to_owned()]),
        },
    );
    ConnectionBoundary::from_category_tokens(categories.iter().map(category_token), scope)
        .expect("closed admitted categories construct a boundary")
}

fn live_allows(
    journal_root: &Path,
    boundary: &ConnectionBoundary,
    path: &str,
    stream: Option<&str>,
) -> bool {
    FacetDeclarationSet::from_journal(journal_root)
        .map(|declarations| classify_source(journal_root, path, stream, &declarations))
        .is_ok_and(|classification| boundary.allows_classification(&classification))
}

fn codec() -> &'static ReferenceCodec {
    REFERENCES.get_or_init(|| {
        ReferenceCodec::new().expect("system randomness initializes MCP reference codec")
    })
}

fn search(
    journal_root: &Path,
    principal: DispatchPrincipal<'_>,
    snapshot: &ConnectionReadSnapshot,
    request: ValidatedSearch,
) -> Result<Prepared, DispatchError> {
    let boundary = boundary(snapshot, request.category, request.facet_id.as_deref());
    let query_hash = digest_json(&json!({
        "query": request.query,
        "day": request.day,
        "day_from": request.day_from,
        "day_to": request.day_to,
        "category": request.category.map(|category| category_token(&category)),
        "facet": request.facet_id,
    }));
    let mut start_after = request
        .cursor
        .as_deref()
        .map(|cursor| {
            resolve_search_cursor(
                journal_root,
                principal,
                snapshot,
                &boundary,
                &query_hash,
                cursor,
            )
        })
        .transpose()?;
    let mut results = Vec::new();
    // The owner's coordinates for each hit released. ⚠ These never enter
    // `results`: the wire response carries no path or chunk index.
    let mut targets = Vec::new();
    let mut examined = 0_usize;
    let mut live_dropped = false;
    let mut index_coverage_complete = true;
    let mut degraded = false;
    let mut has_more = false;
    let mut anchor = None;
    while results.len() < request.limit && examined < MAX_SEARCH_EXAMINED_ROWS {
        let batch = (MAX_SEARCH_EXAMINED_ROWS - examined).min(100);
        let response = search_connection(
            journal_root,
            &boundary,
            &ConnectionSearchRequest {
                query: request.query.clone(),
                limit: batch,
                start_after: start_after.clone(),
                day: request.day.clone(),
                day_from: request.day_from.clone(),
                day_to: request.day_to.clone(),
                ..ConnectionSearchRequest::default()
            },
            Utc::now().date_naive(),
        )
        .map_err(index_error)?;
        index_coverage_complete &= response.coverage_complete;
        degraded |= response.degraded.is_some();
        let batch_len = response.results.len();
        if batch_len == 0 {
            break;
        }
        let mut processed = 0;
        for hit in response.results {
            processed += 1;
            examined += 1;
            start_after = Some(ConnectionStartAfter {
                day: hit.metadata.day.clone(),
                path: hit.metadata.path.clone(),
                idx: hit.metadata.idx,
            });
            anchor = Some((
                hit.metadata.day.clone(),
                hit.metadata.stream.clone(),
                hit.metadata.path.clone(),
                hit.metadata.idx,
                hit.row_id,
                digest(&hit.text),
            ));
            if !live_allows(
                journal_root,
                &boundary,
                &hit.metadata.path,
                Some(&hit.metadata.stream),
            ) {
                live_dropped = true;
                continue;
            }
            targets.push(format!("{}#{}", hit.metadata.path, hit.metadata.idx));
            let reference = codec()
                .mint(
                    principal.connection,
                    snapshot.generation,
                    ReferenceTarget::Entry(EntryReference {
                        day: hit.metadata.day.clone(),
                        stream: hit.metadata.stream.clone(),
                        path: hit.metadata.path,
                        idx: hit.metadata.idx,
                        row_id: hit.row_id,
                    }),
                )
                .map_err(|_| DispatchError::Tool(ToolError::ReferenceNotFound))?;
            results.push(json!({
                "title": format!("Indexed journal entry — {}", hit.metadata.day),
                "date": hit.metadata.day,
                "snippet": snippet(&hit.text),
                "reference": reference,
            }));
            if results.len() == request.limit {
                break;
            }
        }
        has_more = processed < batch_len || batch_len == batch;
        if !has_more || results.len() == request.limit {
            break;
        }
    }
    let next_cursor = anchor
        .filter(|_| has_more || examined == MAX_SEARCH_EXAMINED_ROWS)
        .map(|(day, stream, path, idx, row_id, content_fingerprint)| {
            codec().mint(
                principal.connection,
                snapshot.generation,
                ReferenceTarget::Cursor(CursorReference {
                    query_hash,
                    categories: boundary
                        .categories()
                        .iter()
                        .map(category_token)
                        .map(str::to_owned)
                        .collect(),
                    scope_ids: scope_ids(boundary.scope()),
                    whole_journal: matches!(boundary.scope(), ConnectionScope::WholeJournal),
                    day,
                    stream,
                    path,
                    idx,
                    row_id,
                    content_fingerprint,
                }),
            )
        })
        .transpose()
        .map_err(|_| DispatchError::Tool(ToolError::ReferenceNotFound))?;
    let live_examination_complete =
        examined < MAX_SEARCH_EXAMINED_ROWS && !(live_dropped && results.len() < request.limit);
    let count = results.len();
    Ok(Prepared::new(
        json!({
            "results": results,
            "next_cursor": next_cursor,
            "coverage": {
                "index_classification_complete": index_coverage_complete,
                "live_examination_complete": live_examination_complete,
                "transcript_index": "Raw transcript JSONL is not covered by index search.",
            },
            "degraded": degraded,
        }),
        count,
        targets,
    ))
}

fn resolve_search_cursor(
    journal_root: &Path,
    principal: DispatchPrincipal<'_>,
    snapshot: &ConnectionReadSnapshot,
    boundary: &ConnectionBoundary,
    query_hash: &str,
    token: &str,
) -> Result<ConnectionStartAfter, DispatchError> {
    let ReferenceTarget::Cursor(cursor) = codec()
        .resolve(
            token,
            ReferenceKind::Cursor,
            principal.connection,
            snapshot.generation,
        )
        .map_err(|_| DispatchError::Tool(ToolError::ReferenceNotFound))?
    else {
        return Err(DispatchError::Tool(ToolError::ReferenceNotFound));
    };
    if cursor.query_hash != query_hash
        || cursor.categories
            != boundary
                .categories()
                .iter()
                .map(category_token)
                .map(str::to_owned)
                .collect::<Vec<_>>()
        || cursor.scope_ids != scope_ids(boundary.scope())
        || cursor.whole_journal != matches!(boundary.scope(), ConnectionScope::WholeJournal)
        || !live_allows(journal_root, boundary, &cursor.path, Some(&cursor.stream))
    {
        return Err(DispatchError::Tool(ToolError::ReferenceNotFound));
    }
    let IndexedEntry::Found(content) = read_indexed_entry(
        journal_root,
        QueryBoundary::Connection(boundary.clone()),
        &cursor.path,
        cursor.idx,
        cursor.row_id,
        MAX_INDEXED_ENTRY_BYTES,
    )
    .map_err(index_error)?
    else {
        return Err(DispatchError::Tool(ToolError::ReferenceNotFound));
    };
    if digest(&content) != cursor.content_fingerprint {
        return Err(DispatchError::Tool(ToolError::ReferenceNotFound));
    }
    Ok(ConnectionStartAfter {
        day: cursor.day,
        path: cursor.path,
        idx: cursor.idx,
    })
}

fn fetch(
    journal_root: &Path,
    principal: DispatchPrincipal<'_>,
    snapshot: &ConnectionReadSnapshot,
    request: ValidatedFetch,
) -> Result<Prepared, DispatchError> {
    let ReferenceTarget::Entry(entry) = codec()
        .resolve(
            &request.reference,
            ReferenceKind::Entry,
            principal.connection,
            snapshot.generation,
        )
        .map_err(|_| DispatchError::Tool(ToolError::ReferenceNotFound))?
    else {
        return Err(DispatchError::Tool(ToolError::ReferenceNotFound));
    };
    let boundary = boundary(snapshot, None, None);
    if !live_allows(journal_root, &boundary, &entry.path, Some(&entry.stream)) {
        return Err(DispatchError::Tool(ToolError::ReferenceNotFound));
    }
    let IndexedEntry::Found(text) = read_indexed_entry(
        journal_root,
        QueryBoundary::Connection(boundary),
        &entry.path,
        entry.idx,
        entry.row_id,
        MAX_INDEXED_ENTRY_BYTES,
    )
    .map_err(index_error)?
    else {
        return Err(DispatchError::Tool(ToolError::ReferenceNotFound));
    };
    let target = format!("{}#{}", entry.path, entry.idx);
    Ok(Prepared::new(
        json!({"title": format!("Indexed journal entry — {}", entry.day), "date": entry.day, "text": text}),
        1,
        vec![target],
    ))
}

fn list_facets(
    journal_root: &Path,
    snapshot: &ConnectionReadSnapshot,
    request: ValidatedListFacets,
) -> Result<Prepared, DispatchError> {
    let include_details = snapshot.categories.contains(&AdmittedCategory::Facets);
    let facets = available_facets(journal_root, snapshot)?;
    let mut targets = Vec::new();
    let values = facets.into_iter().take(request.limit).map(|(id, name, declaration)| {
        let title = declaration.as_ref().map_or_else(|| name.clone(), |value| if value.title.is_empty() { name.clone() } else { value.title.clone() });
        targets.push(format!("facet:{id}"));
        if include_details {
            json!({"id": id, "name": title, "description": declaration.as_ref().map(|item| item.description.clone()).unwrap_or_default()})
        } else { json!({"id": id, "name": title}) }
    }).collect::<Vec<_>>();
    let count = values.len();
    Ok(Prepared::new(json!({"facets": values}), count, targets))
}

fn list_entities(
    journal_root: &Path,
    principal: DispatchPrincipal<'_>,
    snapshot: &ConnectionReadSnapshot,
    request: ValidatedListEntities,
) -> Result<Prepared, DispatchError> {
    let facets = available_facets(journal_root, snapshot)?;
    let mut entities = Vec::new();
    let mut targets = Vec::new();
    for (facet_id, facet_dir, _) in facets {
        if request.facet_id.as_deref().is_some_and(|id| id != facet_id) {
            continue;
        }
        let scoped = solstone_core_facets::list_scoped_facet_entities(
            journal_root,
            &facet_dir,
            false,
            false,
        )
        .map_err(|_| DispatchError::Tool(ToolError::FileUnreadable))?;
        for item in scoped {
            if entities.len() == request.limit {
                break;
            }
            let reference = codec()
                .mint(
                    principal.connection,
                    snapshot.generation,
                    ReferenceTarget::Entity(EntityReference {
                        facet_id: facet_id.clone(),
                        entity_id: item.entity_id.clone(),
                    }),
                )
                .map_err(|_| DispatchError::Tool(ToolError::ReferenceNotFound))?;
            targets.push(format!("facet:{facet_id}/entity:{}", item.entity_id));
            entities.push(json!({"name": entity_name(&item.identity), "reference": reference}));
        }
    }
    let count = entities.len();
    Ok(Prepared::new(json!({"entities": entities}), count, targets))
}

fn get_entity(
    journal_root: &Path,
    principal: DispatchPrincipal<'_>,
    snapshot: &ConnectionReadSnapshot,
    request: ValidatedGetEntity,
) -> Result<Prepared, DispatchError> {
    let ReferenceTarget::Entity(reference) = codec()
        .resolve(
            &request.reference,
            ReferenceKind::Entity,
            principal.connection,
            snapshot.generation,
        )
        .map_err(|_| DispatchError::Tool(ToolError::ReferenceNotFound))?
    else {
        return Err(DispatchError::Tool(ToolError::ReferenceNotFound));
    };
    let Some((_, facet_dir, _)) = available_facets(journal_root, snapshot)?
        .into_iter()
        .find(|(id, _, _)| id == &reference.facet_id)
    else {
        return Err(DispatchError::Tool(ToolError::ReferenceNotFound));
    };
    let item =
        solstone_core_facets::list_scoped_facet_entities(journal_root, &facet_dir, false, false)
            .map_err(|_| DispatchError::Tool(ToolError::FileUnreadable))?
            .into_iter()
            .find(|item| item.entity_id == reference.entity_id)
            .ok_or(DispatchError::Tool(ToolError::ReferenceNotFound))?;
    let target = format!(
        "facet:{}/entity:{}",
        reference.facet_id, reference.entity_id
    );
    Ok(Prepared::new(
        json!({"name": entity_name(&item.identity)}),
        1,
        vec![target],
    ))
}

fn list_transcripts(
    journal_root: &Path,
    principal: DispatchPrincipal<'_>,
    snapshot: &ConnectionReadSnapshot,
    request: ValidatedListTranscripts,
) -> Result<Prepared, DispatchError> {
    let boundary = boundary(
        snapshot,
        Some(AdmittedCategory::Transcripts),
        request.facet_id.as_deref(),
    );
    let chronicle = journal_root.join("chronicle");
    let mut segments = Vec::new();
    let mut targets = Vec::new();
    for day in fs::read_dir(&chronicle)
        .map_err(|_| DispatchError::Tool(ToolError::FileUnreadable))?
        .filter_map(Result::ok)
    {
        let Some(day) = day.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if request.day.as_deref().is_some_and(|wanted| wanted != day) {
            continue;
        }
        for segment in iter_segments(journal_root, PathOrDay::Day(&day))
            .map_err(|_| DispatchError::Tool(ToolError::FileUnreadable))?
        {
            if segments.len() == request.limit {
                break;
            }
            let Ok(identity) = segment.record_identity() else {
                continue;
            };
            let probe_path = segment_assignment_probe_path(&day, identity.stream, identity.name);
            if !live_allows(journal_root, &boundary, &probe_path, Some(identity.stream)) {
                continue;
            }
            let page = solstone_core_transcripts::read_segment_transcript_page(&segment, None)
                .map_err(|_| DispatchError::Tool(ToolError::FileUnreadable))?;
            let reference = codec()
                .mint(
                    principal.connection,
                    snapshot.generation,
                    ReferenceTarget::Segment(SegmentReference {
                        day: day.clone(),
                        stream: identity.stream.to_owned(),
                        segment: identity.name.to_owned(),
                        version: page.version.fingerprint(),
                        source_index: None,
                        byte_offset: None,
                    }),
                )
                .map_err(|_| DispatchError::Tool(ToolError::ReferenceNotFound))?;
            targets.push(format!("{day}/{}/{}", identity.stream, identity.name));
            segments.push(json!({"title": format!("Transcript segment — {}", day), "date": day, "reference": reference}));
        }
    }
    let count = segments.len();
    Ok(Prepared::new(
        json!({"transcripts": segments}),
        count,
        targets,
    ))
}

fn get_transcript(
    journal_root: &Path,
    principal: DispatchPrincipal<'_>,
    snapshot: &ConnectionReadSnapshot,
    request: ValidatedGetTranscript,
) -> Result<Prepared, DispatchError> {
    let ReferenceTarget::Segment(reference) = codec()
        .resolve(
            &request.reference,
            ReferenceKind::Segment,
            principal.connection,
            snapshot.generation,
        )
        .map_err(|_| DispatchError::Tool(ToolError::ReferenceNotFound))?
    else {
        return Err(DispatchError::Tool(ToolError::ReferenceNotFound));
    };
    let segment = iter_segments(journal_root, PathOrDay::Day(&reference.day))
        .map_err(|_| DispatchError::Tool(ToolError::ReferenceNotFound))?
        .into_iter()
        .find(|segment| {
            segment
                .record_identity()
                .is_ok_and(|id| id.stream == reference.stream && id.name == reference.segment)
        })
        .ok_or(DispatchError::Tool(ToolError::ReferenceNotFound))?;
    let probe_path =
        segment_assignment_probe_path(&reference.day, &reference.stream, &reference.segment);
    if !live_allows(
        journal_root,
        &boundary(snapshot, Some(AdmittedCategory::Transcripts), None),
        &probe_path,
        Some(&reference.stream),
    ) {
        return Err(DispatchError::Tool(ToolError::ReferenceNotFound));
    }
    let continuation = request
        .cursor
        .as_deref()
        .map(|token| {
            let ReferenceTarget::Segment(cursor) = codec()
                .resolve(
                    token,
                    ReferenceKind::Segment,
                    principal.connection,
                    snapshot.generation,
                )
                .map_err(|_| DispatchError::Tool(ToolError::ReferenceNotFound))?
            else {
                return Err(DispatchError::Tool(ToolError::ReferenceNotFound));
            };
            if cursor.day != reference.day
                || cursor.stream != reference.stream
                || cursor.segment != reference.segment
                || cursor.version != reference.version
            {
                return Err(DispatchError::Tool(ToolError::ReferenceNotFound));
            }
            Ok(solstone_core_transcripts::SegmentTranscriptCursor {
                source_index: cursor
                    .source_index
                    .ok_or(DispatchError::Tool(ToolError::ReferenceNotFound))?,
                byte_offset: cursor
                    .byte_offset
                    .ok_or(DispatchError::Tool(ToolError::ReferenceNotFound))?,
                version: solstone_core_transcripts::SegmentTranscriptVersion::from_fingerprint(
                    cursor.version,
                ),
            })
        })
        .transpose()?;
    let page =
        solstone_core_transcripts::read_segment_transcript_page(&segment, continuation.as_ref())
            .map_err(|_| DispatchError::Tool(ToolError::ReferenceNotFound))?;
    if page.version.fingerprint() != reference.version {
        return Err(DispatchError::Tool(ToolError::ReferenceNotFound));
    }
    let next_cursor = page
        .next
        .map(|next| {
            codec()
                .mint(
                    principal.connection,
                    snapshot.generation,
                    ReferenceTarget::Segment(SegmentReference {
                        day: reference.day.clone(),
                        stream: reference.stream.clone(),
                        segment: reference.segment.clone(),
                        version: reference.version,
                        source_index: Some(next.source_index),
                        byte_offset: Some(next.byte_offset),
                    }),
                )
                .map_err(|_| DispatchError::Tool(ToolError::ReferenceNotFound))
        })
        .transpose()?;
    let target = format!(
        "{}/{}/{}",
        reference.day, reference.stream, reference.segment
    );
    let entries = page
        .entries
        .into_iter()
        .map(|item| item.text)
        .collect::<Vec<_>>();
    let count = entries.len();
    Ok(Prepared::new(
        json!({"entries": entries, "next_cursor": next_cursor }),
        count,
        vec![target],
    ))
}

/// An admitted segment-derived path used only to classify the segment's live
/// assignment. The path need not exist: `classify_source` reads `facets.json`.
fn segment_assignment_probe_path(day: &str, stream: &str, segment: &str) -> String {
    format!("{day}/{stream}/{segment}/talents/transcript.md")
}

/// The facets this connection may reach, resolved from the live journal.
///
/// 🔑 The registry generates a connection's tool schemas through **this**
/// function, the same resolution `list_facets` performs, so discovery names
/// only what the connection can already fetch. ⛔ Resolving facets separately
/// for the schema would make `tools/list` a content read that never passes the
/// tool authorization path.
pub(crate) fn available_facets(
    journal_root: &Path,
    snapshot: &ConnectionReadSnapshot,
) -> Result<
    Vec<(
        String,
        String,
        Option<solstone_core_facets::FacetDeclarationSnapshot>,
    )>,
    DispatchError,
> {
    // `list_facet_directories` is a bare readdir with no per-entry open, unlike
    // `list_declared_facet_names` (which opens every `facet.json` once itself
    // to pre-filter). Reading each candidate's declaration here, once, and
    // skipping whatever that single read cannot resolve, halves the opens on
    // this call's discovery-path callers without changing anything this
    // function returns: a missing, malformed, or non-object declaration was
    // already silently excluded by the two-read form, for the same reason
    // `read_text`/`read_json` treat any read failure as "not declared" there.
    let names = solstone_core_facets::list_facet_directories(journal_root)
        .map_err(|_| DispatchError::Tool(ToolError::FileUnreadable))?;
    let mut facets = Vec::new();
    for name in names {
        let Ok(Some(declaration)) =
            solstone_core_facets::read_facet_declaration(journal_root, &name)
        else {
            continue;
        };
        let Some(id) = declaration
            .value()
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| solstone_core_facets::is_well_formed_facet_id(id))
            .map(str::to_owned)
        else {
            continue;
        };
        if matches!(&snapshot.scope, ConnectionScope::ChosenFacets { ids } if !ids.contains(&id)) {
            continue;
        }
        facets.push((id, name, Some(declaration)));
    }
    Ok(facets)
}

fn entity_name(identity: &Value) -> String {
    identity
        .get("name")
        .or_else(|| identity.get("display_name"))
        .and_then(Value::as_str)
        .unwrap_or("Entity")
        .to_owned()
}

fn scope_ids(scope: &ConnectionScope) -> Vec<String> {
    match scope {
        ConnectionScope::WholeJournal => Vec::new(),
        ConnectionScope::ChosenFacets { ids } => ids.iter().cloned().collect(),
    }
}

fn snippet(value: &str) -> String {
    value.chars().take(MAX_SNIPPET_CHARS).collect()
}
fn digest(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}

/// SHA-256 over the content a call served, with the ephemeral envelope removed.
///
/// 🔴 **The obvious digest — over the prepared bytes exactly as released —
/// cannot do the job the founder's directive asks of it, and it fails green.**
/// Those bytes embed opaque references, which are AES-GCM tokens minted with a
/// fresh nonce on every call, so an identical replay of identical content
/// produces a different digest *every* time. Measured on a real journal: the
/// same `search` replayed against an unchanged corpus recorded `5cdb11ce…`
/// and then `dc800021…`. Nothing errors; the owner is simply told their
/// journal changed, always.
///
/// ✅ So the digest covers what was served — titles, dates, snippets, text,
/// names, coverage — with `reference` and `next_cursor` excluded, and object
/// keys sorted so a later field reordering does not silently invalidate every
/// record already written. ⚠ It is therefore a digest of the **content**, not
/// a fingerprint of the exact bytes on the wire; the targets beside it say
/// which records that content came from.
fn content_digest(value: &Value) -> String {
    digest(&canonical_content(value).to_string())
}

fn canonical_content(value: &Value) -> Value {
    match value {
        Value::Object(fields) => {
            let mut keys = fields
                .iter()
                .map(|(key, _)| key.as_str())
                .filter(|key| !matches!(*key, "reference" | "next_cursor"))
                .collect::<Vec<_>>();
            keys.sort_unstable();
            let mut canonical = Map::new();
            for key in keys {
                canonical.insert(key.to_owned(), canonical_content(&fields[key]));
            }
            Value::Object(canonical)
        }
        Value::Array(items) => Value::Array(items.iter().map(canonical_content).collect()),
        other => other.clone(),
    }
}
fn digest_json(value: &Value) -> String {
    digest(&serde_json::to_string(value).expect("JSON value serializes"))
}

fn index_error(error: solstone_core_indexer_query::IndexAccessError) -> DispatchError {
    let tool_error = match error.reason() {
        "index_absent" => ToolError::IndexAbsent,
        "index_unreadable" => ToolError::IndexUnreadable,
        "index_locked" => ToolError::IndexLocked,
        "empty_index" => ToolError::EmptyIndex,
        _ => ToolError::NotIndexed,
    };
    DispatchError::Tool(tool_error)
}
