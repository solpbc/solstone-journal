// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Shared, transport-independent execution of authenticated MCP tool calls.

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::OnceLock;

use chrono::{DateTime, Utc};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use solstone_core_format::agent_memory::{Coordinate, Origin, SourceKey};
use solstone_core_indexer_query::{
    AdmittedCategory, ConnectionBoundary, ConnectionScope, ConnectionSearchRequest,
    ConnectionStartAfter, IndexedEntry, OwnMemoryOpenError, QueryBoundary,
    indexed_path_has_later_chunk, memory_row_matches_original, open_own_memory_index,
    read_indexed_entry, read_indexed_path_entries, read_own_memory_row, search_connection,
};
use solstone_core_indexer_store::classification::{FacetDeclarationSet, classify_source};
use solstone_core_journal_io::paths::{
    PathOrDay, Segment, day_dirs, iter_segments_skipping_unreadable_streams,
};
use solstone_core_memory_original::{OriginalRead, read_original};
use solstone_core_transcripts::segment_clock;

use solstone_core_mcp_audit::{Admission, AuditCoordinates, Outcome, result_shape};

use crate::activity_admit::{
    ActivityAdmit, ActivityCursorProof, activity_cursor_proof_matches, admit_activity,
};
use crate::audit;
use crate::permissions::{ConnectionReadSnapshot, PermissionDecision, evaluate_connection_read};
use crate::references::{
    CursorReference, EntityListCursor, EntityReference, EntryReference, FacetListCursor,
    ReferenceCodec, ReferenceKind, ReferenceTarget, SegmentReference, TranscriptListCursor,
};
use crate::registry::{ToolEntry, category_token, find_tool, requirement_refusal};
use crate::segment_links::{LinkSubject, LiveTranscriptPages, issue_segment_links};
use crate::tools::{
    ToolError, ValidatedFetch, ValidatedGetEntity, ValidatedGetTranscript, ValidatedGetWeek,
    ValidatedListEntities, ValidatedListFacets, ValidatedListTranscripts, ValidatedSearch,
    ValidatedTool, execute_after_audit,
};

/// Upper bound on indexed rows examined to fill one search page after live drops.
pub(crate) const MAX_SEARCH_EXAMINED_ROWS: usize = 1_000;
const MAX_INDEXED_ENTRY_BYTES: u64 = 64 * 1024;
const MAX_SNIPPET_CHARS: usize = 800;
/// What `get_entity` returns of an entity's notes in the requested facet.
const MAX_ENTITY_NOTES: usize = 50;
const MAX_ENTITY_NOTE_BYTES: u64 = 4 * 1024;
const MAX_ENTITY_DESCRIPTION_CHARS: usize = 2_000;
const MAX_CARD_DESCRIPTION_CHARS: usize = 400;
const MAX_CARD_OBSERVATIONS_CHARS: usize = 400;
const MAX_CARD_NOTES: usize = 8;
const MAX_CARD_NOTE_BYTES: u64 = 2048;

static REFERENCES: OnceLock<ReferenceCodec> = OnceLock::new();

// The plain-words reading of each empty result. ⚠ Search does not cover raw
// transcripts (`coverage.transcript_index`), so an empty search must never be
// worded as "the journal has nothing".
const SEARCH_EMPTY: &str = "No results. Nothing this connection can search in the journal \
matches this. This is a complete answer, not an error, but it does not show that something \
never happened: search does not read conversations word for word. If the owner named a day, \
call list_transcripts for that day, narrowed to any time they mention, then get_transcript. \
Otherwise tell the owner a search of the journal found nothing on this, rather than reading \
transcripts one by one.";
const SEARCH_EMPTY_INCOMPLETE: &str = "No results, but this search did not check the whole \
index, so it does not show the journal has nothing on this. Tell the owner the search came back \
empty and may be incomplete.";
const LIST_FACETS_EMPTY: &str = "No facets. This connection can see none.";
const LIST_ENTITIES_EMPTY: &str = "No entities. This connection can see none that match. This is a complete answer, not an error.";
const LIST_TRANSCRIPTS_EMPTY: &str = "No transcript segments. This connection can see none that \
match. This is a complete answer, not an error.";
const GET_TRANSCRIPT_EMPTY: &str = "This segment has no transcript text.";

/// A prepared response together with the owner coordinates it was built from.
///
/// ⚠ Those coordinates are deliberately absent from `value`: an agent-visible
/// response carries no path, chunk index, row id or stream (increment B), while
/// the owner's log carries exactly those. One prepared value, two audiences.
pub(crate) struct Prepared {
    value: Value,
    count: usize,
    targets: Vec<String>,
    empty_note: Option<String>,
}

impl Prepared {
    pub(crate) fn new(value: Value, count: usize, targets: Vec<String>) -> Self {
        Self {
            value,
            count,
            targets,
            empty_note: None,
        }
    }

    /// Attach the plain-words reading of an empty result. Ignored when the
    /// result is not empty, so a caller cannot tell an agent "nothing" about
    /// a response that carries something.
    pub(crate) fn when_empty(mut self, note: impl Into<String>) -> Self {
        if self.count == 0 {
            self.empty_note = Some(note.into());
        }
        self
    }
}

/// A released tool response: the typed value, and for an empty result a plain
/// sentence telling the agent what the zero means.
///
/// 🔑 Small local models keep digging after an empty result (measured: a 12B
/// model made five or six calls on a question the journal had nothing on, and
/// a skill-file rule did not stop it). The response itself is the lever, so a
/// valid zero says so in words beside the typed empty array.
pub(crate) struct ToolOutput {
    pub(crate) value: Value,
    pub(crate) empty_note: Option<String>,
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
    if entry.tool_name.is_memory() {
        return Err(McpProbeError::InvalidTool);
    }
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
    .map(|output| output.value)
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
) -> Result<ToolOutput, DispatchError> {
    let entry = find_tool(tool_name);
    // ⚠ Validation precedes admission, unchanged: malformed protocol traffic
    // stays outside the per-agent tool trail, and only a syntactically valid
    // known tool call earns a record.
    let validated = validate(journal_root, entry, arguments)?;
    let recorded = recorded_arguments(&validated);

    let snapshot = match evaluate_connection_read(journal_root, principal.connection) {
        PermissionDecision::Snapshot(snapshot) => snapshot,
        PermissionDecision::Denied { reason } => {
            let coordinates = audit_call(journal_root, now, principal, entry, recorded, None)?;
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

    if let Some(refusal) = scope_refusal(principal, &snapshot, entry, &validated) {
        // Missing categories are permission denials and are still durable audit
        // events — now ones an owner can tell apart from a served call.
        let coordinates = audit_call(
            journal_root,
            now,
            principal,
            entry,
            recorded,
            Some(&snapshot),
        )?;
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
        || {
            audit_call(
                journal_root,
                now,
                principal,
                entry,
                recorded,
                Some(&snapshot),
            )
        },
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

    Ok(ToolOutput {
        value: prepared.value,
        empty_note: prepared.empty_note,
    })
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
    principal: DispatchPrincipal<'_>,
    snapshot: &ConnectionReadSnapshot,
    entry: &ToolEntry,
    request: &ValidatedTool,
) -> Option<String> {
    if let Some(reason) = requirement_refusal(snapshot, entry) {
        return Some(reason);
    }
    if let Some(reason) = fetch_target_category_refusal(principal, snapshot, request) {
        return Some(reason);
    }
    if !arguments_within_scope(snapshot, request) {
        return Some(
            "the request named a category or facet outside this connection's permission".to_owned(),
        );
    }
    None
}

/// A reference follows its target's registered material category. An unresolved
/// token is left for the tool to report as a missing reference.
fn fetch_target_category_refusal(
    principal: DispatchPrincipal<'_>,
    snapshot: &ConnectionReadSnapshot,
    request: &ValidatedTool,
) -> Option<String> {
    let ValidatedTool::Fetch(fetch) = request else {
        return None;
    };
    let Ok(target) = codec().resolve_entry_or_activity(
        &fetch.reference,
        principal.connection,
        snapshot.generation,
    ) else {
        return None;
    };
    let missing = match target {
        ReferenceTarget::Entry(entry) => {
            let path = entry.path.strip_prefix("chronicle/").unwrap_or(&entry.path);
            let spec = solstone_core_format::content::resolve_spec(path)?;
            let solstone_core_format::content::IndexDisposition::Admitted { category, .. } =
                spec.disposition
            else {
                return None;
            };
            if snapshot.categories.contains(&category) {
                return None;
            }
            category
        }
        ReferenceTarget::Activity(_)
            if !snapshot.categories.contains(&AdmittedCategory::Facets) =>
        {
            AdmittedCategory::Facets
        }
        _ => return None,
    };
    Some(format!(
        "this connection does not have the {} category",
        category_token(&missing)
    ))
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
/// The title an agent shows for an indexed entry: the day when there is one.
fn entry_title(day: &str) -> String {
    if day.is_empty() {
        "journal entry".to_owned()
    } else {
        format!("journal entry from {day}")
    }
}

/// ⛔ **Never name the journal's derived index as "the journal index".**
/// Owner-facing copy reserves "the journal" for the owner's files. An owner
/// reading *"the journal index is empty"* can read it as **their journal** being
/// empty, when a full journal with an unbuilt search index reports exactly
/// that. ✅ Name the mechanism — the search index — and leave the journal out
/// of the compound.
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
            if let Some(cursor) = request.cursor.as_deref() {
                arguments.insert("cursor".to_owned(), reference_fingerprint(cursor).into());
            }
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
            insert_present(&mut arguments, "time_from", request.time_from.as_deref());
            insert_present(&mut arguments, "time_to", request.time_to.as_deref());
            if let Some(cursor) = request.cursor.as_deref() {
                arguments.insert("cursor".to_owned(), reference_fingerprint(cursor).into());
            }
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
            if let Some(cursor) = request.cursor.as_deref() {
                arguments.insert("cursor".to_owned(), reference_fingerprint(cursor).into());
            }
        }
        ValidatedTool::GetEntity(request) => {
            arguments.insert(
                "reference".to_owned(),
                reference_fingerprint(&request.reference).into(),
            );
        }
        ValidatedTool::GetWeek(request) => {
            insert_present(&mut arguments, "week", request.week.as_deref());
        }
        ValidatedTool::ListActivities(request) => {
            arguments.insert("limit".to_owned(), request.limit.into());
            insert_present(&mut arguments, "day", request.day.as_deref());
            arguments.insert("from".to_owned(), request.from.clone().into());
            arguments.insert("to".to_owned(), request.to.clone().into());
            insert_present(&mut arguments, "facet", request.facet.as_deref());
            if let Some(entity) = request.entity.as_deref() {
                if crate::references::is_framed_entity_token(entity) {
                    arguments.insert("entity".to_owned(), reference_fingerprint(entity).into());
                } else {
                    arguments.insert("entity".to_owned(), entity.into());
                }
            }
            if let Some(cursor) = request.cursor.as_deref() {
                arguments.insert("cursor".to_owned(), reference_fingerprint(cursor).into());
            }
        }
        ValidatedTool::GetActivitySources(request) => {
            arguments.insert(
                "reference".to_owned(),
                reference_fingerprint(&request.reference).into(),
            );
            if let Some(cursor) = request.cursor.as_deref() {
                arguments.insert("cursor".to_owned(), reference_fingerprint(cursor).into());
            }
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

fn validate(
    journal_root: &Path,
    entry: &ToolEntry,
    arguments: Option<&Value>,
) -> Result<ValidatedTool, DispatchError> {
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
        crate::jsonrpc::ToolName::GetWeek => {
            crate::tools::week::validate(arguments).map(ValidatedTool::GetWeek)
        }
        crate::jsonrpc::ToolName::ListActivities => {
            let owner_zone = solstone_core_journal_config::owner_zone(journal_root);
            crate::tools::activities::validate_list(arguments, &owner_zone)
                .map(ValidatedTool::ListActivities)
        }
        crate::jsonrpc::ToolName::GetActivitySources => {
            crate::tools::activities::validate_get_sources(arguments)
                .map(ValidatedTool::GetActivitySources)
        }
        crate::jsonrpc::ToolName::SaveMemory | crate::jsonrpc::ToolName::RecallMemory => {
            return Err(DispatchError::InvalidInput);
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
    snapshot: Option<&ConnectionReadSnapshot>,
) -> Result<AuditCoordinates, DispatchError> {
    audit::write_admitted_interaction(
        journal_root,
        now,
        &Admission {
            connection: principal.connection,
            agent_identity: principal.agent_identity,
            tool_name: entry.audit_name,
            arguments,
            permission: snapshot.map(audit_permission_snapshot),
        },
    )
    .map_err(|_| DispatchError::Tool(ToolError::AuditUnavailable))
}

fn audit_permission_snapshot(
    snapshot: &ConnectionReadSnapshot,
) -> solstone_core_mcp_audit::PermissionSnapshotRecord {
    let categories = snapshot
        .categories
        .iter()
        .map(|category| match category {
            solstone_core_indexer_query::AdmittedCategory::Transcripts => "transcripts".to_owned(),
            solstone_core_indexer_query::AdmittedCategory::Entities => "entities".to_owned(),
            solstone_core_indexer_query::AdmittedCategory::Facets => "facets".to_owned(),
            solstone_core_indexer_query::AdmittedCategory::ScreenText => "screen_text".to_owned(),
        })
        .collect();
    let (scope, facet_ids) = match &snapshot.scope {
        solstone_core_indexer_query::ConnectionScope::WholeJournal => {
            ("whole_journal".to_owned(), Vec::new())
        }
        solstone_core_indexer_query::ConnectionScope::ChosenFacets { ids } => {
            ("facets".to_owned(), ids.iter().cloned().collect())
        }
    };
    solstone_core_mcp_audit::PermissionSnapshotRecord {
        generation: snapshot.generation,
        categories,
        scope,
        facet_ids,
    }
}

fn get_week(
    journal_root: &Path,
    principal: DispatchPrincipal<'_>,
    snapshot: &ConnectionReadSnapshot,
    request: ValidatedGetWeek,
) -> Result<Prepared, DispatchError> {
    let read =
        crate::tools::week::read_week(journal_root, request.week.as_deref(), principal, snapshot)
            .map_err(DispatchError::Tool)?;
    Ok(Prepared::new(read.value, read.count, read.targets)
        .when_empty("no week to read. this is a complete answer, not an error."))
}

fn execute_validated(
    journal_root: &Path,
    principal: DispatchPrincipal<'_>,
    snapshot: &ConnectionReadSnapshot,
    validated: ValidatedTool,
) -> Result<Prepared, DispatchError> {
    match validated {
        ValidatedTool::ListFacets(request) => {
            list_facets(journal_root, principal, snapshot, request)
        }
        ValidatedTool::ListActivities(request) => {
            crate::tools::activities::execute_list(journal_root, principal, snapshot, request)
        }
        ValidatedTool::GetActivitySources(request) => {
            crate::tools::activities::execute_get_sources(
                journal_root,
                principal,
                snapshot,
                request,
            )
        }
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
        ValidatedTool::GetWeek(request) => get_week(journal_root, principal, snapshot, request),
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

pub(crate) fn boundary(
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

pub(crate) fn live_allows(
    journal_root: &Path,
    boundary: &ConnectionBoundary,
    path: &str,
    stream: Option<&str>,
) -> bool {
    FacetDeclarationSet::from_journal(journal_root)
        .map(|declarations| classify_source(journal_root, path, stream, &declarations))
        .is_ok_and(|classification| boundary.allows_classification(&classification))
}

pub(crate) fn codec() -> &'static ReferenceCodec {
    REFERENCES.get_or_init(|| {
        ReferenceCodec::new().expect("system randomness initializes MCP reference codec")
    })
}

fn is_plain_literal_query(query: &str) -> bool {
    if query
        .chars()
        .any(|c| matches!(c, '"' | ':' | '*' | '(' | ')'))
    {
        return false;
    }
    for token in query.split_whitespace() {
        if matches!(token, "AND" | "OR" | "NOT") {
            return false;
        }
    }
    true
}

pub(crate) fn fold_literal(text: &str) -> String {
    let trimmed = text.trim();
    let mut result = String::with_capacity(trimmed.len());
    let mut in_whitespace = false;
    for ch in trimmed.chars() {
        if ch.is_whitespace() {
            if !in_whitespace {
                result.push(' ');
                in_whitespace = true;
            }
        } else {
            in_whitespace = false;
            for lower in ch.to_lowercase() {
                result.push(lower);
            }
        }
    }
    result
}

fn clean_note_entry(raw: &str) -> Option<String> {
    let cleaned = raw.trim().trim_start_matches("- ").trim();
    if cleaned.is_empty() {
        None
    } else {
        Some(cleaned.to_owned())
    }
}

fn derived_name_query(name: &str, kind: Option<&str>) -> Option<String> {
    let kind = kind?;
    let folded_kind = fold_literal(kind);
    let folded_name = fold_literal(name);
    let kind_tokens: Vec<&str> = folded_kind.split(' ').filter(|t| !t.is_empty()).collect();
    let name_tokens: Vec<&str> = folded_name.split(' ').filter(|t| !t.is_empty()).collect();
    if kind_tokens.is_empty() || kind_tokens.len() >= name_tokens.len() {
        return None;
    }
    if !name_tokens.starts_with(&kind_tokens) {
        return None;
    }
    let collapsed_name_tokens: Vec<&str> = name.split_whitespace().collect();
    if collapsed_name_tokens.len() != name_tokens.len() {
        return None;
    }
    let remainder = collapsed_name_tokens[kind_tokens.len()..].join(" ");
    if remainder.is_empty() {
        None
    } else {
        Some(remainder)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EntityResolutionStatus {
    NotAttempted,
    Incomplete,
    Resolved,
}

struct ResolvedIdentitySummary {
    facet_id: String,
    entity_id: String,
    name: String,
    kind: Option<String>,
    derived: Option<String>,
}

struct EntityMatchCard {
    facet_id: String,
    entity_id: String,
    value: Value,
    observations_path: Option<String>,
}

struct EntityResolutionOutcome {
    status: EntityResolutionStatus,
    effective_query: String,
    identities: Vec<ResolvedIdentitySummary>,
    cards: Vec<EntityMatchCard>,
    matches_truncated: bool,
}

fn resolve_search_entities(
    journal_root: &Path,
    principal: DispatchPrincipal<'_>,
    snapshot: &ConnectionReadSnapshot,
    search_boundary: &ConnectionBoundary,
    request: &ValidatedSearch,
) -> EntityResolutionOutcome {
    if !search_boundary
        .categories()
        .contains(&AdmittedCategory::Entities)
        || request.query.trim().is_empty()
        || !is_plain_literal_query(&request.query)
    {
        return EntityResolutionOutcome {
            status: EntityResolutionStatus::NotAttempted,
            effective_query: request.query.clone(),
            identities: Vec::new(),
            cards: Vec::new(),
            matches_truncated: false,
        };
    }

    let facets = match available_facets(journal_root, snapshot) {
        Ok(facets) => facets,
        Err(_) => {
            return EntityResolutionOutcome {
                status: EntityResolutionStatus::Incomplete,
                effective_query: request.query.clone(),
                identities: Vec::new(),
                cards: Vec::new(),
                matches_truncated: false,
            };
        }
    };

    struct GrantedMatch {
        facet_id: String,
        facet_dir: String,
        entity_id: String,
        name: String,
        kind: Option<String>,
        derived: Option<String>,
        relationship_dir: String,
        relationship: Value,
    }

    let mut matches = Vec::new();
    let query_folded = fold_literal(&request.query);

    for (facet_id, facet_dir, _) in facets {
        if request.facet_id.as_deref().is_some_and(|id| id != facet_id) {
            continue;
        }
        if matches!(
            search_boundary.scope(),
            ConnectionScope::ChosenFacets { ids } if !ids.contains(&facet_id)
        ) {
            continue;
        }
        let scoped = match read_scoped_entities(journal_root, &facet_dir) {
            Ok(scoped) => scoped,
            Err(_) => {
                return EntityResolutionOutcome {
                    status: EntityResolutionStatus::Incomplete,
                    effective_query: request.query.clone(),
                    identities: Vec::new(),
                    cards: Vec::new(),
                    matches_truncated: false,
                };
            }
        };
        for item in scoped {
            let name = entity_name(&item.identity);
            let kind = item
                .identity
                .get("type")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .map(str::to_owned);
            drop(item.identity);

            if fold_literal(&name) == query_folded {
                let derived = derived_name_query(&name, kind.as_deref());
                matches.push(GrantedMatch {
                    facet_id: facet_id.clone(),
                    facet_dir: facet_dir.clone(),
                    entity_id: item.entity_id,
                    name,
                    kind,
                    derived,
                    relationship_dir: item.relationship_dir,
                    relationship: item.relationship,
                });
            }
        }
    }

    matches.sort_by(|a, b| {
        a.facet_id
            .cmp(&b.facet_id)
            .then_with(|| a.entity_id.cmp(&b.entity_id))
    });

    let effective_query = if matches.is_empty() {
        request.query.clone()
    } else {
        if let Some(derived) = &matches[0].derived {
            if matches.iter().all(|m| m.derived.as_ref() == Some(derived)) {
                derived.clone()
            } else {
                request.query.clone()
            }
        } else {
            request.query.clone()
        }
    };

    let identities = matches
        .iter()
        .map(|m| ResolvedIdentitySummary {
            facet_id: m.facet_id.clone(),
            entity_id: m.entity_id.clone(),
            name: m.name.clone(),
            kind: m.kind.clone(),
            derived: m.derived.clone(),
        })
        .collect();

    let is_dated = request.day.is_some() || request.day_from.is_some() || request.day_to.is_some();
    if is_dated {
        return EntityResolutionOutcome {
            status: EntityResolutionStatus::Resolved,
            effective_query,
            identities,
            cards: Vec::new(),
            matches_truncated: false,
        };
    }

    let total_matches = matches.len();
    let matches_truncated = total_matches > request.limit;
    let returned_matches = &matches[..total_matches.min(request.limit)];

    let mut cards = Vec::new();
    for m in returned_matches {
        let desc = m
            .relationship
            .get("description")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|t| !t.is_empty());
        let desc_chars_count = desc.map(|d| d.chars().count()).unwrap_or(0);
        let desc_was_cut = desc_chars_count > MAX_CARD_DESCRIPTION_CHARS;
        let desc_cut = desc.map(|d| {
            d.chars()
                .take(MAX_CARD_DESCRIPTION_CHARS)
                .collect::<String>()
        });

        let notes_path =
            solstone_core_facets::facet_links::LinkDirs::for_facet(journal_root, &m.facet_dir)
                .observations_rel(&m.relationship_dir);

        let notes_boundary = boundary(
            snapshot,
            Some(AdmittedCategory::Entities),
            Some(&m.facet_id),
        );
        let indexed_notes = match read_indexed_path_entries(
            journal_root,
            &notes_boundary,
            &notes_path,
            MAX_CARD_NOTES,
            MAX_CARD_NOTE_BYTES,
        ) {
            Ok(notes) => notes,
            Err(_) => {
                return EntityResolutionOutcome {
                    status: EntityResolutionStatus::Incomplete,
                    effective_query: request.query.clone(),
                    identities: Vec::new(),
                    cards: Vec::new(),
                    matches_truncated: false,
                };
            }
        };

        let cleaned_notes = indexed_notes
            .entries
            .iter()
            .filter_map(|e| clean_note_entry(e))
            .collect::<Vec<_>>();

        let joined = cleaned_notes.join("\n");
        let joined_chars_count = joined.chars().count();
        let joined_was_cut = joined_chars_count > MAX_CARD_OBSERVATIONS_CHARS;
        let observations_cut = if joined.is_empty() {
            None
        } else {
            Some(
                joined
                    .chars()
                    .take(MAX_CARD_OBSERVATIONS_CHARS)
                    .collect::<String>(),
            )
        };
        let observations_path = if !cleaned_notes.is_empty() {
            Some(notes_path)
        } else {
            None
        };

        let is_truncated = desc_was_cut || joined_was_cut || indexed_notes.truncated;

        let reference = match codec().mint(
            principal.connection,
            snapshot.generation,
            ReferenceTarget::Entity(EntityReference {
                facet_id: m.facet_id.clone(),
                entity_id: m.entity_id.clone(),
            }),
        ) {
            Ok(r) => r,
            Err(_) => {
                return EntityResolutionOutcome {
                    status: EntityResolutionStatus::Incomplete,
                    effective_query: request.query.clone(),
                    identities: Vec::new(),
                    cards: Vec::new(),
                    matches_truncated: false,
                };
            }
        };

        let mut card_val = json!({
            "name": m.name,
            "facet": m.facet_id,
            "reference": reference,
        });
        if let Some(kind) = &m.kind {
            card_val["type"] = json!(kind);
        }
        if let Some(description) = desc_cut {
            card_val["description"] = json!(description);
        }
        if let Some(observations) = observations_cut {
            card_val["observations"] = json!(observations);
        }
        if is_truncated {
            card_val["truncated"] = Value::Bool(true);
        }

        cards.push(EntityMatchCard {
            facet_id: m.facet_id.clone(),
            entity_id: m.entity_id.clone(),
            value: card_val,
            observations_path,
        });
    }

    EntityResolutionOutcome {
        status: EntityResolutionStatus::Resolved,
        effective_query,
        identities,
        cards,
        matches_truncated,
    }
}

struct SearchAnchor {
    day: String,
    stream: String,
    path: String,
    idx: i64,
    row_id: i64,
    content_fingerprint: String,
    activity_proof: Option<String>,
    omit_cursor: bool,
}

fn search(
    journal_root: &Path,
    principal: DispatchPrincipal<'_>,
    snapshot: &ConnectionReadSnapshot,
    request: ValidatedSearch,
) -> Result<Prepared, DispatchError> {
    let boundary = boundary(snapshot, request.category, request.facet_id.as_deref());
    let resolution =
        resolve_search_entities(journal_root, principal, snapshot, &boundary, &request);
    let resolution_status_str = match resolution.status {
        EntityResolutionStatus::NotAttempted => "not_attempted",
        EntityResolutionStatus::Incomplete => "incomplete",
        EntityResolutionStatus::Resolved => "resolved",
    };
    let resolution_identities_val = resolution
        .identities
        .iter()
        .map(|id| {
            json!({
                "facet_id": id.facet_id,
                "entity_id": id.entity_id,
                "name": id.name,
                "type": id.kind,
                "derived": id.derived,
            })
        })
        .collect::<Vec<_>>();
    let query_hash = digest_json(&json!({
        "query": request.query,
        "effective_query": resolution.effective_query,
        "day": request.day,
        "day_from": request.day_from,
        "day_to": request.day_to,
        "category": request.category.map(|category| category_token(&category)),
        "facet": request.facet_id,
        "entity_resolution": {
            "status": resolution_status_str,
            "identities": resolution_identities_val,
        },
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
    let mut screen_text_checks = std::collections::BTreeMap::new();
    let mut response_checks = 0;
    // The owner's coordinates for each hit released. ⚠ These never enter
    // `results`: the wire response carries no path or chunk index.
    let mut targets = Vec::new();
    let mut examined = 0_usize;
    let mut live_dropped = false;
    let mut index_coverage_complete = true;
    let mut degraded = false;
    let mut relaxed = false;
    let mut has_more = false;
    let mut anchor: Option<SearchAnchor> = None;
    while results.len() < request.limit && examined < MAX_SEARCH_EXAMINED_ROWS {
        let batch = (MAX_SEARCH_EXAMINED_ROWS - examined).min(100);
        let response = search_connection(
            journal_root,
            &boundary,
            &ConnectionSearchRequest {
                query: resolution.effective_query.clone(),
                limit: batch,
                start_after: start_after.clone(),
                day: request.day.clone(),
                day_from: request.day_from.clone(),
                day_to: request.day_to.clone(),
                // A hosted agent sends a sentence as often as keywords. When every
                // word must match and none does, the index retries on the content
                // words; `relaxed` tells the agent the hits match only some of them.
                relax: true,
                ..ConnectionSearchRequest::default()
            },
            // Journal days are the owner's days, so relative dates resolve against
            // the owner's today.
            Utc::now()
                .with_timezone(&solstone_core_journal_config::owner_zone(journal_root))
                .date_naive(),
        )
        .map_err(index_error)?;
        index_coverage_complete &= response.coverage_complete;
        degraded |= response.degraded.is_some();
        relaxed |= response.relaxed;
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
            anchor = Some(SearchAnchor {
                day: hit.metadata.day.clone(),
                stream: hit.metadata.stream.clone(),
                path: hit.metadata.path.clone(),
                idx: hit.metadata.idx,
                row_id: hit.row_id,
                content_fingerprint: digest(&hit.text),
                activity_proof: None,
                omit_cursor: false,
            });
            if !live_allows(
                journal_root,
                &boundary,
                &hit.metadata.path,
                Some(&hit.metadata.stream),
            ) {
                live_dropped = true;
                continue;
            }
            let (served_text, activity_proof, hidden, record, record_identity) =
                match admit_activity(
                    journal_root,
                    &boundary,
                    &hit.metadata.path,
                    hit.metadata.idx,
                    hit.row_id,
                    Some(&hit.text),
                    None,
                    false,
                    None,
                    MAX_INDEXED_ENTRY_BYTES,
                ) {
                    Ok(ActivityAdmit::NotActivity(text)) => (text, None, None, None, None),
                    Ok(ActivityAdmit::Admitted(admitted)) => (
                        admitted.text,
                        Some(admitted.proof),
                        Some(admitted.hidden),
                        Some(admitted.record),
                        Some(admitted.record_identity),
                    ),
                    Ok(ActivityAdmit::Withheld { cursor_proof }) => {
                        if let Some(anchor) = anchor.as_mut() {
                            match cursor_proof {
                                Some(proof) => anchor.activity_proof = Some(proof),
                                None => anchor.omit_cursor = true,
                            }
                        }
                        live_dropped = true;
                        continue;
                    }
                    Err(error) => return Err(index_error(error)),
                };
            if let Some(anchor) = anchor.as_mut() {
                anchor.activity_proof = activity_proof.clone();
            }
            let memory_origin = if hit.metadata.stream.starts_with("agent-memory-") {
                let Some(origin) =
                    memory_hit_origin(journal_root, &hit.metadata.path, &hit.metadata.stream)
                else {
                    live_dropped = true;
                    continue;
                };
                Some(origin)
            } else {
                None
            };

            let is_screen_text =
                solstone_core_format::content::screen_text::is_screen_text_path(&hit.metadata.path);
            let (source_digest, byte_offset) = if is_screen_text {
                let Some(sha256) = hit.metadata.source_digest.clone() else {
                    live_dropped = true;
                    continue;
                };
                (Some(sha256), Some(0))
            } else {
                (None, None)
            };

            #[cfg(feature = "test-hooks")]
            crate::source_freeze_barrier::checkpoint(
                crate::source_freeze_barrier::FreezePoint::Before,
            );
            let allowed = crate::source_release::check_indexed_source_release(
                journal_root,
                &hit.metadata.path,
            );
            let mut single_subject = [LinkSubject {
                path: &hit.metadata.path,
                idx: hit.metadata.idx,
                text: &served_text,
                segments: Vec::new(),
                more_segments: false,
                activity_record: record.as_ref(),
            }];
            let mut checker = LiveTranscriptPages::default();
            if allowed && !is_screen_text {
                issue_segment_links(
                    journal_root,
                    principal,
                    snapshot,
                    &mut single_subject,
                    3,
                    30,
                    &mut response_checks,
                    &mut checker,
                );
            }
            #[cfg(feature = "test-hooks")]
            crate::source_freeze_barrier::checkpoint(
                crate::source_freeze_barrier::FreezePoint::After,
            );

            if !allowed
                || source_digest.as_deref().is_some_and(|digest| {
                    !*screen_text_checks
                        .entry((hit.metadata.path.clone(), digest.to_owned()))
                        .or_insert_with(|| {
                            screen_text_identity_current(journal_root, &hit.metadata.path, digest)
                        })
                })
            {
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
                        path: hit.metadata.path.clone(),
                        idx: hit.metadata.idx,
                        row_id: hit.row_id,
                        activity_proof,
                        record_identity,
                        source_digest,
                        byte_offset,
                    }),
                )
                .map_err(|_| DispatchError::Tool(ToolError::ReferenceNotFound))?;
            let mut value = json!({
                "title": entry_title(&hit.metadata.day),
                "date": hit.metadata.day,
                "snippet": snippet(&served_text),
                "reference": reference,
            });
            if let Some(offset_ms) = hit.metadata.group_offset_ms {
                if let Some(segment) =
                    solstone_core_format::content::screen_text::screen_text_segment(
                        &hit.metadata.path,
                    )
                {
                    value["segment"] = Value::String(segment.to_owned());
                }
                value["minute"] = Value::String(
                    solstone_core_format::content::screen_text::screen_text_minute(offset_ms),
                );
            }
            let is_story_activity = record
                .as_ref()
                .and_then(|r| r.get("story"))
                .and_then(|s| s.get("body"))
                .and_then(Value::as_str)
                .is_some_and(|b| !b.trim().is_empty());
            let is_weekly_reflection = hit.metadata.path.starts_with("reflections/weekly/")
                && hit.metadata.path.ends_with(".md");
            if is_story_activity || is_weekly_reflection {
                value["derived"] = Value::Bool(true);
            }
            if let Some(hidden) = hidden {
                value["hidden"] = Value::Bool(hidden);
            }
            if let Some(origin) = memory_origin {
                value["origin"] = public_memory_origin(&origin);
            }
            // A calendar result says whether it is the latest version of the
            // event or a change recorded on its date, and when it was recorded.
            if let Some(calendar) = &hit.metadata.calendar {
                value["calendar"] = json!(calendar);
            }
            if !single_subject[0].segments.is_empty() {
                value["segments"] = Value::Array(std::mem::take(&mut single_subject[0].segments));
            }
            if single_subject[0].more_segments {
                value["more_segments"] = Value::Bool(true);
            }
            results.push(value);
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
        .filter(|anchor| !anchor.omit_cursor)
        .filter(|_| has_more || examined == MAX_SEARCH_EXAMINED_ROWS)
        .map(|anchor| {
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
                    day: anchor.day,
                    stream: anchor.stream,
                    path: anchor.path,
                    idx: anchor.idx,
                    row_id: anchor.row_id,
                    content_fingerprint: anchor.content_fingerprint,
                    activity_proof: anchor.activity_proof,
                }),
            )
        })
        .transpose()
        .map_err(|_| DispatchError::Tool(ToolError::ReferenceNotFound))?;
    let live_examination_complete =
        examined < MAX_SEARCH_EXAMINED_ROWS && !(live_dropped && results.len() < request.limit);
    let count = results.len() + resolution.cards.len();
    for card in &resolution.cards {
        targets.push(format!("facet:{}/entity:{}", card.facet_id, card.entity_id));
        if let Some(obs_path) = &card.observations_path {
            targets.push(obs_path.clone());
        }
    }
    let complete = index_coverage_complete && live_examination_complete && !degraded;
    let mut response_val = json!({
        "results": results,
        "next_cursor": next_cursor,
        "coverage": {
            "index_classification_complete": index_coverage_complete,
            "live_examination_complete": live_examination_complete,
            "transcript_index": "Search does not read conversations word for word. Read the segments a result lists with get_transcript.",
        },
        "degraded": degraded,
        "relaxed": relaxed,
    });
    if !resolution.cards.is_empty() {
        let card_vals = resolution
            .cards
            .iter()
            .map(|c| c.value.clone())
            .collect::<Vec<_>>();
        response_val["entity_matches"] = Value::Array(card_vals);
    }
    if resolution.matches_truncated {
        response_val["entity_matches_truncated"] = Value::Bool(true);
    }
    if resolution.status == EntityResolutionStatus::Incomplete {
        response_val["entity_context_incomplete"] = Value::Bool(true);
    }
    let mut prepared = Prepared::new(response_val, count, targets);
    if count == 0 && resolution.status != EntityResolutionStatus::Incomplete {
        prepared = prepared.when_empty(if complete {
            SEARCH_EMPTY
        } else {
            SEARCH_EMPTY_INCOMPLETE
        });
    }
    Ok(prepared)
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
    match activity_cursor_proof_matches(
        journal_root,
        boundary,
        &cursor.path,
        cursor.idx,
        cursor.row_id,
        &content,
        cursor.activity_proof.as_deref().unwrap_or(""),
        MAX_INDEXED_ENTRY_BYTES,
    )
    .map_err(index_error)?
    {
        ActivityCursorProof::NotActivity => {}
        ActivityCursorProof::Matches => {}
        ActivityCursorProof::Reject => {
            return Err(DispatchError::Tool(ToolError::ReferenceNotFound));
        }
    }
    if digest(&content) != cursor.content_fingerprint {
        return Err(DispatchError::Tool(ToolError::ReferenceNotFound));
    }
    Ok(ConnectionStartAfter {
        day: cursor.day,
        path: cursor.path,
        idx: cursor.idx,
    })
}

/// How a markdown fetch represents an indexed chunk.
enum MarkdownFetch {
    /// The indexed chunk is still that index in the current file, so the whole
    /// file is the record.
    Document(String),
    /// The index admitted no later chunk of this path, and the current file
    /// does not verify it. ac10 keeps that indexed snapshot rather than the
    /// bytes now on disk. A non-markdown path is the same snapshot: this fetch
    /// does not expand it.
    IndexedChunk,
    /// The index admitted a later chunk of this path, or this reference is not
    /// the first chunk, and the current file does not verify it. The indexed
    /// slice is not the whole record.
    Reject,
}

fn markdown_fetch(
    journal_root: &Path,
    boundary: &ConnectionBoundary,
    path: &str,
    idx: i64,
    indexed: &str,
) -> Result<MarkdownFetch, DispatchError> {
    if !path.ends_with(".md") {
        return Ok(MarkdownFetch::IndexedChunk);
    }
    if let Some(full_text) = verified_markdown_document(journal_root, path, idx, indexed) {
        return Ok(MarkdownFetch::Document(full_text));
    }
    // A first chunk is the whole indexed document only when the index admitted
    // no later chunk of this path. The connection boundary is the same one
    // that admitted the reference; classification is per path, so a sibling
    // chunk is visible whenever this one is.
    if idx == 0
        && !indexed_path_has_later_chunk(journal_root, boundary, path).map_err(index_error)?
    {
        return Ok(MarkdownFetch::IndexedChunk);
    }
    Ok(MarkdownFetch::Reject)
}

fn verified_markdown_document(
    journal_root: &Path,
    path: &str,
    idx: i64,
    indexed: &str,
) -> Option<String> {
    let bytes = crate::activity_admit::read_contained_source(journal_root, path)?;
    let full_text = String::from_utf8(bytes).ok()?;
    let produced = solstone_core_format::content::produce_chunks(
        solstone_core_format::content::Family::Markdown,
        path,
        &full_text,
    );
    let idx = usize::try_from(idx).ok()?;
    let chunk = produced.chunks.get(idx)?;
    (chunk.content == indexed).then_some(full_text)
}

fn screen_text_identity_current(journal_root: &Path, path: &str, pinned_digest: &str) -> bool {
    let Ok(connection) = solstone_core_indexer_store::open_index_reader(journal_root) else {
        return false;
    };
    solstone_core_indexer_store::screen_text_current_digest(&connection, journal_root, path)
        .ok()
        .flatten()
        .is_some_and(|digest| digest == pinned_digest)
}

fn fetch(
    journal_root: &Path,
    principal: DispatchPrincipal<'_>,
    snapshot: &ConnectionReadSnapshot,
    request: ValidatedFetch,
) -> Result<Prepared, DispatchError> {
    let target = codec()
        .resolve_entry_or_activity(
            &request.reference,
            principal.connection,
            snapshot.generation,
        )
        .map_err(|_| DispatchError::Tool(ToolError::ReferenceNotFound))?;
    match target {
        ReferenceTarget::Entry(entry) => {
            let is_screen_text =
                solstone_core_format::content::screen_text::is_screen_text_path(&entry.path);
            let boundary = boundary(snapshot, None, None);
            if !live_allows(journal_root, &boundary, &entry.path, Some(&entry.stream)) {
                return Err(DispatchError::Tool(ToolError::ReferenceNotFound));
            }
            if is_screen_text {
                #[cfg(feature = "test-hooks")]
                crate::source_freeze_barrier::checkpoint(
                    crate::source_freeze_barrier::FreezePoint::Before,
                );
                let (Some(source_digest), Some(byte_offset)) =
                    (entry.source_digest.as_deref(), entry.byte_offset)
                else {
                    return Err(DispatchError::Tool(ToolError::ReferenceNotFound));
                };
                let Ok(conn) = solstone_core_indexer_store::open_index_reader(journal_root) else {
                    return Err(DispatchError::Tool(ToolError::ReferenceNotFound));
                };
                let page = solstone_core_indexer_store::screen_text_pinned_page(
                    &conn,
                    journal_root,
                    &entry.path,
                    source_digest,
                    byte_offset,
                )
                .ok()
                .flatten()
                .ok_or(DispatchError::Tool(ToolError::ReferenceNotFound))?;
                let mut value = json!({
                    "title": entry_title(&entry.day),
                    "date": entry.day,
                    "text": page.text,
                });
                if let Some(next_off) = page.next_offset {
                    let next_ref = codec()
                        .mint(
                            principal.connection,
                            snapshot.generation,
                            ReferenceTarget::Entry(EntryReference {
                                day: entry.day.clone(),
                                stream: entry.stream.clone(),
                                path: entry.path.clone(),
                                idx: entry.idx,
                                row_id: entry.row_id,
                                activity_proof: None,
                                record_identity: None,
                                source_digest: Some(source_digest.to_owned()),
                                byte_offset: Some(next_off),
                            }),
                        )
                        .map_err(|_| DispatchError::Tool(ToolError::ReferenceNotFound))?;
                    value["next_reference"] = json!(next_ref);
                }
                #[cfg(feature = "test-hooks")]
                crate::source_freeze_barrier::checkpoint(
                    crate::source_freeze_barrier::FreezePoint::After,
                );
                if !crate::source_release::check_indexed_source_release(journal_root, &entry.path)
                    || !screen_text_identity_current(journal_root, &entry.path, source_digest)
                {
                    return Err(DispatchError::Tool(ToolError::ReferenceNotFound));
                }
                let target = format!("{}#{}", entry.path, entry.idx);
                return Ok(Prepared::new(value, 1, vec![target]));
            }
            let (text, hidden, record) = match admit_activity(
                journal_root,
                &boundary,
                &entry.path,
                entry.idx,
                entry.row_id,
                None,
                entry.activity_proof.as_deref(),
                true,
                entry.record_identity.as_deref(),
                MAX_INDEXED_ENTRY_BYTES,
            )
            .map_err(index_error)?
            {
                ActivityAdmit::Admitted(a) => {
                    let cleaned =
                        crate::tools::activities::clean_activity_record_for_wire(a.record.clone());
                    let cleaned_text = serde_json::to_string(&cleaned)
                        .map_err(|_| DispatchError::Tool(ToolError::ReferenceNotFound))?;
                    // Links resolve from the admitted record. Cleaning drops
                    // `stream`, and the resolver would then cross streams.
                    (cleaned_text, Some(a.hidden), Some(a.record))
                }
                ActivityAdmit::NotActivity(content) => {
                    let text = match markdown_fetch(
                        journal_root,
                        &boundary,
                        &entry.path,
                        entry.idx,
                        &content,
                    )? {
                        MarkdownFetch::Document(text) => text,
                        MarkdownFetch::IndexedChunk => content,
                        MarkdownFetch::Reject => {
                            return Err(DispatchError::Tool(ToolError::ReferenceNotFound));
                        }
                    };
                    (text, None, None)
                }
                ActivityAdmit::Withheld { .. } => {
                    return Err(DispatchError::Tool(ToolError::ReferenceNotFound));
                }
            };
            #[cfg(feature = "test-hooks")]
            crate::source_freeze_barrier::checkpoint(
                crate::source_freeze_barrier::FreezePoint::Before,
            );
            let allowed =
                crate::source_release::check_indexed_source_release(journal_root, &entry.path);
            let mut subjects = [LinkSubject {
                path: &entry.path,
                idx: entry.idx,
                text: &text,
                segments: Vec::new(),
                more_segments: false,
                activity_record: record.as_ref(),
            }];
            let mut checker = LiveTranscriptPages::default();
            let mut response_checks = 0;
            if allowed {
                issue_segment_links(
                    journal_root,
                    principal,
                    snapshot,
                    &mut subjects,
                    48,
                    48,
                    &mut response_checks,
                    &mut checker,
                );
            }
            #[cfg(feature = "test-hooks")]
            crate::source_freeze_barrier::checkpoint(
                crate::source_freeze_barrier::FreezePoint::After,
            );

            if !allowed {
                return Err(DispatchError::Tool(ToolError::ReferenceNotFound));
            }

            let mut value =
                json!({"title": entry_title(&entry.day), "date": entry.day, "text": text});
            if let Some(hidden) = hidden {
                value["hidden"] = Value::Bool(hidden);
            }
            if entry.stream.starts_with("agent-memory-") {
                let origin = memory_hit_origin(journal_root, &entry.path, &entry.stream)
                    .ok_or(DispatchError::Tool(ToolError::ReferenceNotFound))?;
                value["origin"] = public_memory_origin(&origin);
            }
            if !subjects[0].segments.is_empty() {
                value["segments"] = Value::Array(subjects[0].segments.clone());
            }
            if subjects[0].more_segments {
                value["more_segments"] = Value::Bool(true);
            }
            let target = format!("{}#{}", entry.path, entry.idx);
            Ok(Prepared::new(value, 1, vec![target]))
        }
        ReferenceTarget::Activity(act_ref) => {
            let rel_path = act_ref.relative_path().map_err(DispatchError::Tool)?;
            let boundary = boundary(snapshot, None, None);
            if !live_allows(journal_root, &boundary, &rel_path, None) {
                return Err(DispatchError::Tool(ToolError::ReferenceNotFound));
            }
            let Some(bytes) = crate::activity_admit::read_contained_source(journal_root, &rel_path)
            else {
                return Err(DispatchError::Tool(ToolError::ReferenceNotFound));
            };
            let file_digest = crate::activity_admit::hex_encode(&Sha256::digest(&bytes));
            if file_digest != act_ref.file_digest {
                return Err(DispatchError::Tool(ToolError::ReferenceNotFound));
            }
            let Ok(file_text) = String::from_utf8(bytes) else {
                return Err(DispatchError::Tool(ToolError::ReferenceNotFound));
            };
            let Some((record, _)) =
                crate::activity_admit::record_by_identity(&file_text, &act_ref.record_identity)
            else {
                return Err(DispatchError::Tool(ToolError::ReferenceNotFound));
            };

            let hidden = record
                .get("hidden")
                .is_some_and(solstone_core_facets::activity_value_truthy);
            let cleaned = crate::tools::activities::clean_activity_record_for_wire(record.clone());
            let text = serde_json::to_string(&cleaned)
                .map_err(|_| DispatchError::Tool(ToolError::ReferenceNotFound))?;

            #[cfg(feature = "test-hooks")]
            crate::source_freeze_barrier::checkpoint(
                crate::source_freeze_barrier::FreezePoint::Before,
            );
            let allowed =
                crate::source_release::check_indexed_source_release(journal_root, &rel_path);
            let mut subjects = [LinkSubject {
                path: &rel_path,
                idx: 0,
                text: &text,
                segments: Vec::new(),
                more_segments: false,
                activity_record: Some(&record),
            }];
            let mut checker = LiveTranscriptPages::default();
            let mut response_checks = 0;
            if allowed {
                issue_segment_links(
                    journal_root,
                    principal,
                    snapshot,
                    &mut subjects,
                    48,
                    48,
                    &mut response_checks,
                    &mut checker,
                );
            }
            #[cfg(feature = "test-hooks")]
            crate::source_freeze_barrier::checkpoint(
                crate::source_freeze_barrier::FreezePoint::After,
            );

            if !allowed {
                return Err(DispatchError::Tool(ToolError::ReferenceNotFound));
            }

            let mut value = json!({
                "title": entry_title(&act_ref.day),
                "date": act_ref.day,
                "text": text,
            });
            if hidden {
                value["hidden"] = Value::Bool(true);
            }
            if !subjects[0].segments.is_empty() {
                value["segments"] = Value::Array(subjects[0].segments.clone());
            }
            if subjects[0].more_segments {
                value["more_segments"] = Value::Bool(true);
            }
            let target = format!("{rel_path}#{}", act_ref.record_identity);
            Ok(Prepared::new(value, 1, vec![target]))
        }
        _ => Err(DispatchError::Tool(ToolError::ReferenceNotFound)),
    }
}

fn memory_hit_origin(journal_root: &Path, path: &str, stream: &str) -> Option<Origin> {
    let component = stream.strip_prefix("agent-memory-")?;
    let Ok(source_key) = SourceKey::parse(format!("sha256:{component}")) else {
        return None;
    };
    let parts = path
        .strip_prefix("chronicle/")
        .unwrap_or(path)
        .split('/')
        .collect::<Vec<_>>();
    let [day, path_stream, segment, note] = parts.as_slice() else {
        return None;
    };
    if *note != "note.txt" || *path_stream != stream {
        return None;
    }
    let coordinate = Coordinate {
        day: (*day).to_owned(),
        stream: stream.to_owned(),
        segment: (*segment).to_owned(),
    };
    let live = match read_original(journal_root, &source_key, &coordinate) {
        OriginalRead::Ready {
            bytes,
            origin,
            ready,
        } => (bytes, origin, ready),
        _ => return None,
    };
    let connection = match open_own_memory_index(journal_root, std::time::Duration::from_secs(5)) {
        Ok(connection) => connection,
        Err(OwnMemoryOpenError::Pending | OwnMemoryOpenError::Unavailable) => return None,
    };
    let row = read_own_memory_row(&connection, source_key.as_str(), path)
        .ok()
        .flatten()?;
    memory_row_matches_original(&row, &source_key, &coordinate, &live.0, &live.1, &live.2)
        .then_some(live.1)
}

fn public_memory_origin(origin: &Origin) -> Value {
    json!({"kind": origin.kind, "creation_label": origin.creation_label, "created_at": origin.created_at})
}

fn list_facets(
    journal_root: &Path,
    principal: DispatchPrincipal<'_>,
    snapshot: &ConnectionReadSnapshot,
    request: ValidatedListFacets,
) -> Result<Prepared, DispatchError> {
    let cursor = request
        .cursor
        .as_deref()
        .map(|token| {
            let ReferenceTarget::FacetList(cursor) = codec()
                .resolve(
                    token,
                    ReferenceKind::FacetList,
                    principal.connection,
                    snapshot.generation,
                )
                .map_err(|_| DispatchError::Tool(ToolError::ReferenceNotFound))?
            else {
                return Err(DispatchError::Tool(ToolError::ReferenceNotFound));
            };
            Ok(cursor)
        })
        .transpose()?;

    let include_details = snapshot.categories.contains(&AdmittedCategory::Facets);
    let mut facets = available_facets(journal_root, snapshot)?;
    facets.sort_by(|(id_a, _, _), (id_b, _, _)| id_a.cmp(id_b));

    if let Some(cursor) = cursor.as_ref() {
        facets.retain(|(id, _, _)| id > &cursor.last_facet_id);
    }

    let has_more = facets.len() > request.limit;
    let page = facets.into_iter().take(request.limit).collect::<Vec<_>>();
    let last_facet_id = page.last().map(|(id, _, _)| id.clone());

    let mut targets = Vec::new();
    let values = page
        .into_iter()
        .map(|(id, name, declaration)| {
            let title = declaration.as_ref().map_or_else(
                || name.clone(),
                |value| {
                    if value.title.is_empty() {
                        name.clone()
                    } else {
                        value.title.clone()
                    }
                },
            );
            targets.push(format!("facet:{id}"));
            if include_details {
                json!({
                    "id": id,
                    "name": title,
                    "description": declaration.as_ref().map(|item| item.description.clone()).unwrap_or_default(),
                })
            } else {
                json!({"id": id, "name": title})
            }
        })
        .collect::<Vec<_>>();

    let next_cursor = if has_more {
        if let Some(last_facet_id) = last_facet_id {
            Some(
                codec()
                    .mint(
                        principal.connection,
                        snapshot.generation,
                        ReferenceTarget::FacetList(FacetListCursor { last_facet_id }),
                    )
                    .map_err(|_| DispatchError::Tool(ToolError::ReferenceNotFound))?,
            )
        } else {
            None
        }
    } else {
        None
    };

    let count = values.len();
    let mut response_val = json!({"facets": values});
    if let Some(next_cursor) = next_cursor {
        response_val["next_cursor"] = json!(next_cursor);
    }
    Ok(Prepared::new(response_val, count, targets).when_empty(LIST_FACETS_EMPTY))
}

pub(crate) fn read_scoped_entities(
    journal_root: &Path,
    facet_dir: &str,
) -> Result<Vec<solstone_core_facets::ScopedFacetEntity>, DispatchError> {
    let mut entities =
        solstone_core_facets::list_scoped_facet_entities(journal_root, facet_dir, false, false)
            .map_err(|_| DispatchError::Tool(ToolError::FileUnreadable))?;
    // Legacy folders can name the same facet relationship. Match the link
    // owner's preference: the folder named by the entity id, then name order.
    entities.sort_by(|a, b| {
        a.entity_id
            .cmp(&b.entity_id)
            .then_with(|| {
                (a.relationship_dir != a.entity_id).cmp(&(b.relationship_dir != b.entity_id))
            })
            .then_with(|| a.relationship_dir.cmp(&b.relationship_dir))
    });
    entities.dedup_by(|a, b| a.entity_id == b.entity_id);
    Ok(entities)
}

fn list_entities(
    journal_root: &Path,
    principal: DispatchPrincipal<'_>,
    snapshot: &ConnectionReadSnapshot,
    request: ValidatedListEntities,
) -> Result<Prepared, DispatchError> {
    let cursor = request
        .cursor
        .as_deref()
        .map(|token| {
            let ReferenceTarget::EntityList(cursor) = codec()
                .resolve(
                    token,
                    ReferenceKind::EntityList,
                    principal.connection,
                    snapshot.generation,
                )
                .map_err(|_| DispatchError::Tool(ToolError::ReferenceNotFound))?
            else {
                return Err(DispatchError::Tool(ToolError::ReferenceNotFound));
            };
            if cursor.facet != request.facet_id {
                return Err(DispatchError::Tool(ToolError::ReferenceNotFound));
            }
            Ok(cursor)
        })
        .transpose()?;

    let facets = available_facets(journal_root, snapshot)?;
    // Every facet in the effective filter must be read before paging so a malformed relationship fails closed.
    let mut all_entities = Vec::new();
    for (facet_id, facet_dir, _) in facets {
        if request.facet_id.as_deref().is_some_and(|id| id != facet_id) {
            continue;
        }
        let scoped = read_scoped_entities(journal_root, &facet_dir)?;
        for item in scoped {
            all_entities.push((facet_id.clone(), item));
        }
    }

    all_entities.sort_by(|(f_a, item_a), (f_b, item_b)| {
        f_a.cmp(f_b)
            .then_with(|| item_a.entity_id.cmp(&item_b.entity_id))
    });

    if let Some(cursor) = cursor.as_ref() {
        all_entities.retain(|(facet_id, item)| {
            (facet_id.as_str(), item.entity_id.as_str())
                > (
                    cursor.last_facet_id.as_str(),
                    cursor.last_entity_id.as_str(),
                )
        });
    }

    let has_more = all_entities.len() > request.limit;
    let page = all_entities
        .into_iter()
        .take(request.limit)
        .collect::<Vec<_>>();

    let last_anchor = page
        .last()
        .map(|(facet_id, item)| (facet_id.clone(), item.entity_id.clone()));

    let mut targets = Vec::new();
    let mut entities = Vec::new();
    for (facet_id, item) in page {
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

    let next_cursor = if has_more {
        if let Some((last_facet_id, last_entity_id)) = last_anchor {
            Some(
                codec()
                    .mint(
                        principal.connection,
                        snapshot.generation,
                        ReferenceTarget::EntityList(EntityListCursor {
                            facet: request.facet_id.clone(),
                            last_facet_id,
                            last_entity_id,
                        }),
                    )
                    .map_err(|_| DispatchError::Tool(ToolError::ReferenceNotFound))?,
            )
        } else {
            None
        }
    } else {
        None
    };

    let count = entities.len();
    let mut response_val = json!({"entities": entities});
    if let Some(next_cursor) = next_cursor {
        response_val["next_cursor"] = json!(next_cursor);
    }
    Ok(Prepared::new(response_val, count, targets).when_empty(LIST_ENTITIES_EMPTY))
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
    let item = read_scoped_entities(journal_root, &facet_dir)?
        .into_iter()
        .find(|item| item.entity_id == reference.entity_id)
        .ok_or(DispatchError::Tool(ToolError::ReferenceNotFound))?;
    let mut targets = vec![format!(
        "facet:{}/entity:{}",
        reference.facet_id, reference.entity_id
    )];
    // 🔑 Only what this facet holds about the entity: its relationship here and
    // its notes here. ⛔ Never the journal-level aliases or description, nor
    // anything from another facet, even when the entity is shared.
    let mut value = json!({"name": entity_name(&item.identity)});
    let text = |object: &Value, key: &str| {
        object
            .get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_owned)
    };
    if let Some(kind) = text(&item.identity, "type") {
        value["type"] = json!(kind);
    }
    if let Some(description) = text(&item.relationship, "description") {
        if description.chars().count() > MAX_ENTITY_DESCRIPTION_CHARS {
            value["description_truncated"] = json!(true);
        }
        value["description"] = json!(
            description
                .chars()
                .take(MAX_ENTITY_DESCRIPTION_CHARS)
                .collect::<String>()
        );
    }
    if let Some(last_seen) = text(&item.relationship, "last_seen") {
        value["last_seen"] = json!(last_seen);
    }
    // Notes are read from the index, as `fetch` reads, never from the live file.
    let notes_path =
        solstone_core_facets::facet_links::LinkDirs::for_facet(journal_root, &facet_dir)
            .observations_rel(&item.relationship_dir);
    let notes = read_indexed_path_entries(
        journal_root,
        &boundary(
            snapshot,
            Some(AdmittedCategory::Entities),
            Some(&reference.facet_id),
        ),
        &notes_path,
        MAX_ENTITY_NOTES,
        MAX_ENTITY_NOTE_BYTES,
    )
    .map_err(index_error)?;
    let notes_list = notes
        .entries
        .iter()
        .filter_map(|note| clean_note_entry(note))
        .collect::<Vec<_>>();
    if !notes_list.is_empty() {
        targets.push(notes_path);
    }
    value["notes"] = json!(notes_list);
    if notes.truncated {
        value["notes_truncated"] = json!(true);
    }
    Ok(Prepared::new(value, 1, targets))
}

fn is_after_cursor(
    day: &str,
    start_seconds: u32,
    stream: &str,
    name: &str,
    cursor: Option<&TranscriptListCursor>,
) -> bool {
    let Some(cursor) = cursor else {
        return true;
    };
    let cursor_start = segment_clock(&cursor.last_name)
        .map(|c| c.start_seconds)
        .unwrap_or(0);
    match day.cmp(&cursor.last_day) {
        std::cmp::Ordering::Greater => false,
        std::cmp::Ordering::Less => true,
        std::cmp::Ordering::Equal => {
            (start_seconds, stream, name)
                > (
                    cursor_start,
                    cursor.last_stream.as_str(),
                    cursor.last_name.as_str(),
                )
        }
    }
}

fn list_transcripts(
    journal_root: &Path,
    principal: DispatchPrincipal<'_>,
    snapshot: &ConnectionReadSnapshot,
    request: ValidatedListTranscripts,
) -> Result<Prepared, DispatchError> {
    let cursor = request
        .cursor
        .as_deref()
        .map(|token| {
            let ReferenceTarget::TranscriptList(cursor) = codec()
                .resolve(
                    token,
                    ReferenceKind::TranscriptList,
                    principal.connection,
                    snapshot.generation,
                )
                .map_err(|_| DispatchError::Tool(ToolError::ReferenceNotFound))?
            else {
                return Err(DispatchError::Tool(ToolError::ReferenceNotFound));
            };
            if cursor.day != request.day
                || cursor.time_from != request.time_from
                || cursor.time_to != request.time_to
                || cursor.facet != request.facet_id
            {
                return Err(DispatchError::Tool(ToolError::ReferenceNotFound));
            }
            Ok(cursor)
        })
        .transpose()?;

    let boundary = boundary(
        snapshot,
        Some(AdmittedCategory::Transcripts),
        request.facet_id.as_deref(),
    );
    let owner_zone = solstone_core_journal_config::owner_zone(journal_root);

    let day_map =
        day_dirs(journal_root).map_err(|_| DispatchError::Tool(ToolError::FileUnreadable))?;
    let days: Vec<String> = if let Some(ref day) = request.day {
        if day_map.contains_key(day) {
            vec![day.clone()]
        } else {
            Vec::new()
        }
    } else {
        let mut keys: Vec<String> = day_map.into_keys().collect();
        keys.sort_unstable_by(|a, b| b.cmp(a));
        keys
    };

    let parse_time_to_seconds = |t: &str| -> Option<u32> {
        let (h, m) = t.split_once(':')?;
        let h: u32 = h.parse().ok()?;
        let m: u32 = m.parse().ok()?;
        Some(h * 3600 + m * 60)
    };

    let time_filter =
        if request.day.is_some() && (request.time_from.is_some() || request.time_to.is_some()) {
            let from_sec = request
                .time_from
                .as_deref()
                .and_then(parse_time_to_seconds)
                .unwrap_or(0);
            let to_sec = request
                .time_to
                .as_deref()
                .and_then(parse_time_to_seconds)
                .unwrap_or(86400);
            Some((from_sec, to_sec))
        } else {
            None
        };

    let mut segments = Vec::new();
    let mut targets = Vec::new();
    let mut unreadable = 0;
    let mut last_examined: Option<(String, String, String)> = None;
    let mut has_more = false;

    'outer: for day in &days {
        let day_segments =
            iter_segments_skipping_unreadable_streams(journal_root, PathOrDay::Day(day))
                .map_err(|_| DispatchError::Tool(ToolError::FileUnreadable))?;

        let mut matching_segments = Vec::new();
        for segment in day_segments {
            let Ok(identity) = segment.record_identity() else {
                continue;
            };
            let stream = identity.stream.to_string();
            let name = identity.name.to_string();
            // Calendar notes are not something the owner did; they are found
            // through search, not listed as a moment of the day.
            if solstone_core_segment::is_calendar_stream(journal_root, &stream) {
                continue;
            }
            let clock = segment_clock(&name);
            if let Some((from_sec, to_sec)) = time_filter {
                let Some(ref clock) = clock else {
                    continue;
                };
                let start = clock.start_seconds;
                let end = clock.exclusive_end_seconds;
                if std::cmp::max(start, from_sec) >= std::cmp::min(end, to_sec) {
                    continue;
                }
            }
            matching_segments.push((segment, stream, name, clock));
        }

        matching_segments.sort_by(
            |(_, stream_a, name_a, clock_a), (_, stream_b, name_b, clock_b)| {
                let start_a = clock_a.as_ref().map(|c| c.start_seconds).unwrap_or(0);
                let start_b = clock_b.as_ref().map(|c| c.start_seconds).unwrap_or(0);
                start_a
                    .cmp(&start_b)
                    .then_with(|| stream_a.cmp(stream_b))
                    .then_with(|| name_a.cmp(name_b))
            },
        );

        let speakers = solstone_core_talent_runtime::agent_speakers(journal_root);
        for (segment, stream, name, clock) in matching_segments {
            let start_seconds = clock.as_ref().map(|c| c.start_seconds).unwrap_or(0);
            if !is_after_cursor(day, start_seconds, &stream, &name, cursor.as_ref()) {
                continue;
            }

            // Scope comes first: a segment this connection may not read neither
            // fills the page nor earns a `next_cursor`.
            let probe_path = segment_assignment_probe_path(day, &stream, &name);
            if !live_allows(journal_root, &boundary, &probe_path, Some(&stream)) {
                continue;
            }

            #[cfg(feature = "test-hooks")]
            crate::source_freeze_barrier::checkpoint(
                crate::source_freeze_barrier::FreezePoint::Before,
            );
            let allowed = crate::source_release::check_segment_transcript_release(
                journal_root,
                day,
                &stream,
                &name,
            );
            let page = if allowed && segments.len() < request.limit {
                Some(solstone_core_transcripts::read_segment_transcript_page(
                    &segment,
                    None,
                    speakers.as_ref(),
                ))
            } else {
                None
            };
            // Freeze the page before the last deletion observation. A removal
            // can replace the directory while its initial live marker is read.
            let allowed = allowed
                && crate::source_release::check_segment_transcript_release(
                    journal_root,
                    day,
                    &stream,
                    &name,
                );
            #[cfg(feature = "test-hooks")]
            crate::source_freeze_barrier::checkpoint(
                crate::source_freeze_barrier::FreezePoint::After,
            );

            if !allowed {
                continue;
            }

            if segments.len() == request.limit {
                has_more = true;
                break 'outer;
            }

            last_examined = Some((day.clone(), stream.clone(), name.clone()));

            let page = match page.expect("allowed and within limit reads page") {
                Ok(page) => page,
                Err(_) => {
                    unreadable += 1;
                    continue;
                }
            };

            let reference = codec()
                .mint(
                    principal.connection,
                    snapshot.generation,
                    ReferenceTarget::Segment(SegmentReference {
                        day: day.clone(),
                        stream: stream.clone(),
                        segment: name.clone(),
                        version: page.version.fingerprint(),
                        source_index: None,
                        byte_offset: None,
                        statement_ordinal: None,
                    }),
                )
                .map_err(|_| DispatchError::Tool(ToolError::ReferenceNotFound))?;

            targets.push(format!("{day}/{stream}/{name}"));
            let time_str = clock.as_ref().map(|c| c.label.as_str()).unwrap_or("");
            let mut seg_val = json!({
                "title": format!("Transcript segment — {}", day),
                "date": day,
                "time": time_str,
                "reference": reference,
            });
            if let Some(zone) =
                differing_capture_zone(journal_root, &owner_zone, day, &name, &segment)
            {
                seg_val["capture_zone"] = zone;
            }
            segments.push(seg_val);
        }
    }

    let next_cursor = if has_more {
        if let Some((last_day, last_stream, last_name)) = last_examined {
            Some(
                codec()
                    .mint(
                        principal.connection,
                        snapshot.generation,
                        ReferenceTarget::TranscriptList(TranscriptListCursor {
                            day: request.day.clone(),
                            time_from: request.time_from.clone(),
                            time_to: request.time_to.clone(),
                            facet: request.facet_id.clone(),
                            last_day,
                            last_stream,
                            last_name,
                        }),
                    )
                    .map_err(|_| DispatchError::Tool(ToolError::ReferenceNotFound))?,
            )
        } else {
            None
        }
    } else {
        None
    };

    let count = segments.len();
    let mut response_val = json!({
        "transcripts": segments,
    });
    if let Some(cursor) = next_cursor {
        response_val["next_cursor"] = json!(cursor);
    }
    if unreadable > 0 {
        response_val["unreadable"] = json!(unreadable);
    }

    Ok(Prepared::new(response_val, count, targets).when_empty(LIST_TRANSCRIPTS_EMPTY))
}

fn get_transcript(
    journal_root: &Path,
    principal: DispatchPrincipal<'_>,
    snapshot: &ConnectionReadSnapshot,
    request: ValidatedGetTranscript,
) -> Result<Prepared, DispatchError> {
    let owner_zone = solstone_core_journal_config::owner_zone(journal_root);
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
    let segment =
        iter_segments_skipping_unreadable_streams(journal_root, PathOrDay::Day(&reference.day))
            .map_err(|_| DispatchError::Tool(ToolError::ReferenceNotFound))?
            .into_iter()
            .find(|segment| {
                segment
                    .record_identity()
                    .is_ok_and(|id| id.stream == reference.stream && id.name == reference.segment)
            })
            .filter(|segment| !solstone_core_segment::is_calendar_segment(journal_root, segment))
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
                statement_ordinal: cursor
                    .statement_ordinal
                    .ok_or(DispatchError::Tool(ToolError::ReferenceNotFound))?,
                version: solstone_core_transcripts::SegmentTranscriptVersion::from_fingerprint(
                    cursor.version,
                ),
            })
        })
        .transpose()?;
    #[cfg(feature = "test-hooks")]
    crate::source_freeze_barrier::checkpoint(crate::source_freeze_barrier::FreezePoint::Before);
    let allowed = crate::source_release::check_segment_transcript_release(
        journal_root,
        &reference.day,
        &reference.stream,
        &reference.segment,
    );
    let page_res = if allowed {
        let speakers = solstone_core_talent_runtime::agent_speakers(journal_root);
        Some(solstone_core_transcripts::read_segment_transcript_page(
            &segment,
            continuation.as_ref(),
            speakers.as_ref(),
        ))
    } else {
        None
    };
    let allowed = allowed
        && crate::source_release::check_segment_transcript_release(
            journal_root,
            &reference.day,
            &reference.stream,
            &reference.segment,
        );
    #[cfg(feature = "test-hooks")]
    crate::source_freeze_barrier::checkpoint(crate::source_freeze_barrier::FreezePoint::After);

    if !allowed {
        return Err(DispatchError::Tool(ToolError::ReferenceNotFound));
    }
    let page = page_res
        .expect("allowed reads page")
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
                        statement_ordinal: Some(next.statement_ordinal),
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
    let mut response_val = json!({"entries": entries, "next_cursor": next_cursor });
    if page.untimed {
        response_val["untimed"] = Value::Bool(true);
    }
    if let Some(zone) = differing_capture_zone(
        journal_root,
        &owner_zone,
        &reference.day,
        &reference.segment,
        &segment,
    ) {
        response_val["capture_zone"] = zone;
    }
    Ok(Prepared::new(response_val, count, vec![target]).when_empty(GET_TRANSCRIPT_EMPTY))
}

pub(crate) fn differing_capture_zone(
    journal_root: &Path,
    owner: &solstone_core_journal_config::Tz,
    day: &str,
    key: &str,
    segment: &Segment,
) -> Option<Value> {
    let reported = solstone_core_callosum::read_reported_zone(journal_root, segment.path())?;
    let view = solstone_core_journal_config::capture_zone_view(
        day,
        key,
        reported.tz.as_deref(),
        reported.utc_offset_seconds,
        owner,
    )?;
    if !view.differs {
        return None;
    }
    Some(json!({
        "tz": view.tz,
        "utc_offset_seconds": view.utc_offset_seconds,
        "label": view.label,
    }))
}

/// An admitted segment-derived path used only to classify the segment's live
/// assignment. The path need not exist: `classify_source` reads `facets.json`.
pub(crate) fn segment_assignment_probe_path(day: &str, stream: &str, segment: &str) -> String {
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

pub(crate) fn entity_name(identity: &Value) -> String {
    identity
        .get("name")
        .or_else(|| identity.get("display_name"))
        .and_then(Value::as_str)
        .unwrap_or("Entity")
        .to_owned()
}

pub(crate) fn scope_ids(scope: &ConnectionScope) -> Vec<String> {
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
/// cannot tell whether a replay still serves the same content, and it fails green.**
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
