// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Shared, transport-independent execution of authenticated MCP tool calls.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::sync::OnceLock;

use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use solstone_core_indexer_query::{
    AdmittedCategory, ConnectionBoundary, ConnectionScope, ConnectionSearchRequest,
    ConnectionStartAfter, IndexedEntry, QueryBoundary, read_indexed_entry, search_connection,
};
use solstone_core_indexer_store::classification::{FacetDeclarationSet, classify_source};
use solstone_core_journal_io::paths::{PathOrDay, iter_segments};

use crate::audit;
use crate::permissions::{ConnectionReadSnapshot, PermissionDecision, evaluate_connection_read};
use crate::references::{
    CursorReference, EntityReference, EntryReference, ReferenceCodec, ReferenceKind,
    ReferenceTarget, SegmentReference,
};
use crate::registry::{ToolEntry, find_tool, snapshot_allows};
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
    let validated = validate(entry, arguments)?;

    let snapshot = match evaluate_connection_read(journal_root, principal.connection) {
        PermissionDecision::Snapshot(snapshot) => snapshot,
        PermissionDecision::Denied { reason } => {
            // Refused vs served records are identical apart from timestamps — deliberate deferral to the later activity-log increment, not an oversight.
            audit_call(journal_root, now, principal.agent_identity, entry)?;
            return Err(DispatchError::PermissionDenied(reason));
        }
    };

    if !snapshot_allows(&snapshot, entry) || !arguments_within_scope(&snapshot, &validated) {
        // Missing categories are permission denials and are still durable audit events.
        audit_call(journal_root, now, principal.agent_identity, entry)?;
        return Err(DispatchError::PermissionDenied("no_permission"));
    }

    let prepared = execute_after_audit(
        || audit_call(journal_root, now, principal.agent_identity, entry),
        || execute_validated(journal_root, principal, &snapshot, validated),
    )?;

    // A grant can be narrowed while a bounded read is in flight. Do not release
    // any prepared bytes after that change.
    if !permission_generation_is_current(journal_root, principal.connection, &snapshot) {
        return Err(DispatchError::PermissionDenied("no_permission"));
    }

    Ok(prepared)
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
    agent_identity: &str,
    entry: &ToolEntry,
) -> Result<(), DispatchError> {
    audit::write_admitted_interaction(journal_root, now, agent_identity, entry.audit_name)
        .map(|_| ())
        .map_err(|_| DispatchError::Tool(ToolError::AuditUnavailable))
}

fn execute_validated(
    journal_root: &Path,
    principal: DispatchPrincipal<'_>,
    snapshot: &ConnectionReadSnapshot,
    validated: ValidatedTool,
) -> Result<Value, DispatchError> {
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

fn category_token(category: &AdmittedCategory) -> &'static str {
    match category {
        AdmittedCategory::Transcripts => "transcripts",
        AdmittedCategory::Entities => "entities",
        AdmittedCategory::Facets => "facets",
    }
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
) -> Result<Value, DispatchError> {
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
    Ok(json!({
        "results": results,
        "next_cursor": next_cursor,
        "coverage": {
            "index_classification_complete": index_coverage_complete,
            "live_examination_complete": live_examination_complete,
            "transcript_index": "Raw transcript JSONL is not covered by index search.",
        },
        "degraded": degraded,
    }))
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
) -> Result<Value, DispatchError> {
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
    Ok(
        json!({"title": format!("Indexed journal entry — {}", entry.day), "date": entry.day, "text": text}),
    )
}

fn list_facets(
    journal_root: &Path,
    snapshot: &ConnectionReadSnapshot,
    request: ValidatedListFacets,
) -> Result<Value, DispatchError> {
    let include_details = snapshot.categories.contains(&AdmittedCategory::Facets);
    let facets = available_facets(journal_root, snapshot)?;
    let values = facets.into_iter().take(request.limit).map(|(id, name, declaration)| {
        let title = declaration.as_ref().map_or_else(|| name.clone(), |value| if value.title.is_empty() { name.clone() } else { value.title.clone() });
        if include_details {
            json!({"id": id, "name": title, "description": declaration.as_ref().map(|item| item.description.clone()).unwrap_or_default()})
        } else { json!({"id": id, "name": title}) }
    }).collect::<Vec<_>>();
    Ok(json!({"facets": values}))
}

fn list_entities(
    journal_root: &Path,
    principal: DispatchPrincipal<'_>,
    snapshot: &ConnectionReadSnapshot,
    request: ValidatedListEntities,
) -> Result<Value, DispatchError> {
    let facets = available_facets(journal_root, snapshot)?;
    let mut entities = Vec::new();
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
            entities.push(json!({"name": entity_name(&item.identity), "reference": reference}));
        }
    }
    Ok(json!({"entities": entities}))
}

fn get_entity(
    journal_root: &Path,
    principal: DispatchPrincipal<'_>,
    snapshot: &ConnectionReadSnapshot,
    request: ValidatedGetEntity,
) -> Result<Value, DispatchError> {
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
    Ok(json!({"name": entity_name(&item.identity)}))
}

fn list_transcripts(
    journal_root: &Path,
    principal: DispatchPrincipal<'_>,
    snapshot: &ConnectionReadSnapshot,
    request: ValidatedListTranscripts,
) -> Result<Value, DispatchError> {
    let boundary = boundary(
        snapshot,
        Some(AdmittedCategory::Transcripts),
        request.facet_id.as_deref(),
    );
    let chronicle = journal_root.join("chronicle");
    let mut segments = Vec::new();
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
            segments.push(json!({"title": format!("Transcript segment — {}", day), "date": day, "reference": reference}));
        }
    }
    Ok(json!({"transcripts": segments}))
}

fn get_transcript(
    journal_root: &Path,
    principal: DispatchPrincipal<'_>,
    snapshot: &ConnectionReadSnapshot,
    request: ValidatedGetTranscript,
) -> Result<Value, DispatchError> {
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
    Ok(
        json!({"entries": page.entries.into_iter().map(|item| item.text).collect::<Vec<_>>(), "next_cursor": next_cursor }),
    )
}

/// An admitted segment-derived path used only to classify the segment's live
/// assignment. The path need not exist: `classify_source` reads `facets.json`.
fn segment_assignment_probe_path(day: &str, stream: &str, segment: &str) -> String {
    format!("{day}/{stream}/{segment}/talents/transcript.md")
}

fn available_facets(
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
    let names = solstone_core_facets::list_declared_facet_names(journal_root)
        .map_err(|_| DispatchError::Tool(ToolError::FileUnreadable))?;
    let mut facets = Vec::new();
    for name in names {
        let declaration = solstone_core_facets::read_facet_declaration(journal_root, &name)
            .map_err(|_| DispatchError::Tool(ToolError::FileUnreadable))?;
        let Some(declaration) = declaration else {
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
