// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Shared, transport-independent execution of authenticated MCP tool calls.

use crate::audit::Reason;
use solstone_core_convey_http::window_catalog::ENGLISH;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::{Arc, OnceLock};

use jiff::Timestamp;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use solstone_core_format::agent_memory::{Coordinate, Origin, SourceKey};
use solstone_core_format::content::topics::{
    TopicDocument, kind_words, segment_path as topic_segment,
};
use solstone_core_indexer_query::{
    AdmittedCategory, ConnectionBoundary, ConnectionScope, ConnectionSearchRequest,
    ConnectionStartAfter, EntryLocation, OwnMemoryOpenError, QueryBoundary,
    indexed_path_has_later_chunk, locate_indexed_entry, memory_row_matches_original,
    open_own_memory_index, read_indexed_entry, read_indexed_path_entries, read_own_memory_row,
    search_connection_with_possible_names,
};
use solstone_core_indexer_store::classification::{FacetDeclarationSet, classify_source};
use solstone_core_journal_io::paths::{
    PathOrDay, Segment, day_dirs, iter_segments_skipping_unreadable_streams,
};
use solstone_core_memory_original::{OriginalRead, read_original};
use solstone_core_possible_names::is_possible_names_path;
use solstone_core_transcripts::segment_clock;

use solstone_core_mcp_audit::{Admission, AuditCoordinates, Outcome, result_shape};

use crate::activity_admit::{
    ActivityAdmit, ActivityCursorProof, activity_cursor_proof_matches, admit_activity,
};
use crate::audit;
use crate::permissions::{ConnectionReadSnapshot, PermissionDecision, evaluate_connection_read};
use crate::possible_names::ConnectionPossibleNames;
use crate::references::{
    ActivityReference, CursorReference, EntityListCursor, EntityReference, EntryReference,
    FacetListCursor, ReferenceCodec, ReferenceKind, ReferenceTarget, SegmentReference,
    TranscriptListCursor,
};
use crate::registry::{ToolEntry, category_token, find_tool, requirement_refusal};
use crate::segment_links::{LinkSubject, LiveTranscriptPages, issue_segment_links};
use crate::tools::{
    ToolError, ValidatedFetch, ValidatedGetActivitySources, ValidatedGetEntity,
    ValidatedGetTranscript, ValidatedGetWeek, ValidatedListEntities, ValidatedListFacets,
    ValidatedListTranscripts, ValidatedSearch, ValidatedTool, execute_after_audit,
};

/// Upper bound on indexed rows examined to fill one search page after live drops.
pub(crate) const MAX_SEARCH_EXAMINED_ROWS: usize = 1_000;
const MAX_INDEXED_ENTRY_BYTES: u64 = 64 * 1024;
const MAX_SNIPPET_CHARS: usize = 800;
const MAX_IMPORT_PREVIEW_ENTRIES: usize = 8;
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
const SEARCH_OUTSIDE_FACETS: &str = "This connection sees only some facets. Calendar events \
and the rest of the journal that belongs to no facet are not in these results.";
const LIST_FACETS_EMPTY: &str = "No facets. This connection can see none.";
const LIST_ENTITIES_EMPTY: &str = "No entities. This connection can see none that match. This is a complete answer, not an error.";
const LIST_TRANSCRIPTS_EMPTY: &str = "No transcript segments. This connection can see none that \
match. This is a complete answer, not an error.";
const LIST_TRANSCRIPTS_BODY_ELSEWHERE: &str = "No transcript segments, but this journal holds body data. List a day to see which body sources it holds in other_sources.";
const LIST_TRANSCRIPTS_OTHER_SOURCES: &str = "No transcript segments, but this day holds body data. Pass a label from other_sources as source to list it.";
const LIST_TRANSCRIPTS_WORKOUTS: &str = "This list leaves out Strava workouts, counted in omitted_workouts. No source label lists them: search finds a workout by its words, and list_activities lists the activity recorded for it, when it has one.";
// Screen and terminal text is never transcript, so an empty read says where it is.
const GET_TRANSCRIPT_EMPTY: &str = "This segment has no transcript text. Text shown on a \
screen or in a terminal is not transcript: search with category screen_text finds it, when \
this connection may read screen text.";
pub(crate) const ACTIVITY_SOURCES_EMPTY: &str = "This activity has no transcript sources. Text \
shown on a screen or in a terminal is not a transcript source: search with category screen_text \
finds it, when this connection may read screen text.";
// Out of this connection's reach is permanent, so it never reads as a retry.
pub(crate) const ACTIVITY_SOURCES_OUT_OF_REACH: &str = "This connection may not read this \
activity's sources. This is a complete answer, not an error: asking again returns the same.";

/// A prepared response together with the owner coordinates it was built from.
///
/// ⚠ Those coordinates are deliberately absent from `value`: an agent-visible
/// response carries no path, chunk index, row id or stream (increment B), while
/// the owner's log carries exactly those. One prepared value, two audiences.
pub(crate) struct Prepared {
    value: Value,
    images: Vec<Vec<u8>>,
    count: usize,
    targets: Vec<String>,
    empty_note: Option<String>,
    served_as_is: &'static [&'static str],
}

impl Prepared {
    pub(crate) fn new(value: Value, count: usize, targets: Vec<String>) -> Self {
        Self {
            value,
            images: Vec::new(),
            count,
            targets,
            empty_note: None,
            served_as_is: &[],
        }
    }

    /// Name the fields release must not escape: text the tool already made
    /// literal itself, or a reference the agent passes back unchanged.
    pub(crate) fn served_as_is(mut self, keys: &'static [&'static str]) -> Self {
        self.served_as_is = keys;
        self
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
    pub(crate) images: Vec<Vec<u8>>,
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
    KindDenied(AdmittedCategory),
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
        Timestamp::now(),
    )
    .map(|output| crate::jsonrpc::with_note(output.value, output.empty_note.as_deref()))
    .map_err(|error| match error {
        DispatchError::InvalidInput
        | DispatchError::Tool(
            ToolError::UseGetTranscript
            | ToolError::ActivityDayArguments
            | ToolError::TranscriptTimeArguments
            | ToolError::WeekRangeArguments,
        ) => McpProbeError::InvalidInput,
        DispatchError::PermissionDenied(_) | DispatchError::KindDenied(_) => {
            McpProbeError::PermissionDenied
        }
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
    now: Timestamp,
) -> Result<ToolOutput, DispatchError> {
    if tool_name == crate::jsonrpc::ToolName::SuggestCorrection
        && !crate::suggestions::suggestions_ready()
    {
        return Err(DispatchError::InvalidInput);
    }
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
                owner_denial_reason(reason),
            );
            return Err(DispatchError::PermissionDenied(reason));
        }
    };

    if let Some((refusal, missing_kind)) =
        scope_refusal(journal_root, principal, &snapshot, entry, &validated)
    {
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
        record_outcome(journal_root, &coordinates, now, Outcome::Refused, refusal);
        return Err(missing_kind.map_or(
            DispatchError::PermissionDenied("no_permission"),
            DispatchError::KindDenied,
        ));
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

    let mut prepared = match executed {
        Ok(prepared) => prepared,
        Err(error) => {
            record_outcome(
                journal_root,
                &coordinates,
                now,
                Outcome::Error,
                owner_error_reason(error),
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
            Reason::PermissionChanged,
        );
        return Err(DispatchError::PermissionDenied("no_permission"));
    }

    literal_journal_values(&mut prepared.value, prepared.served_as_is);

    // 🔑 The digest is what makes replay a checkable claim rather than an
    // assumed one: re-fetch the named targets, compare, and a difference means
    // the journal actually moved. See `content_digest` for why it cannot be
    // taken over the released bytes.
    let shape = result_shape(
        prepared.count,
        prepared.targets,
        if prepared.images.len() == 1 && prepared.value.get("items").is_none() {
            let image = &prepared.images[0];
            content_digest(
                &json!({"value": prepared.value, "image_sha256": format!("{:x}", Sha256::digest(image))}),
            )
        } else if !prepared.images.is_empty() {
            let digests: Vec<_> = prepared
                .images
                .iter()
                .map(|image| format!("{:x}", Sha256::digest(image)))
                .collect();
            content_digest(&json!({"value": prepared.value, "image_sha256": digests}))
        } else {
            content_digest(&prepared.value)
        },
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
        images: prepared.images,
        value: prepared.value,
        empty_note: prepared.empty_note,
    })
}

/// Serve journal text literally, so markup in it cannot become a live link,
/// image or HTML when an agent's reply is displayed.
///
/// 🔑 Every read tool's response passes through here, so a field a tool adds
/// later is covered without the tool remembering to escape it. A value that
/// cannot carry markup keeps its bytes, which leaves references, ids, days and
/// counts as they were. Object keys are escaped too, because some responses
/// key a map by a journal name.
fn literal_journal_values(value: &mut Value, served_as_is: &[&str]) {
    match value {
        Value::String(text) => *text = solstone_core_format::markdown::literal_markup(text),
        Value::Array(items) => items
            .iter_mut()
            .for_each(|item| literal_journal_values(item, served_as_is)),
        Value::Object(object) => {
            *object = std::mem::take(object)
                .into_iter()
                .map(|(key, mut value)| {
                    if served_as_is.contains(&key.as_str()) {
                        return (key, value);
                    }
                    literal_journal_values(&mut value, served_as_is);
                    (solstone_core_format::markdown::literal_markup(&key), value)
                })
                .collect();
        }
        _ => {}
    }
}

/// An activity record served as JSON text: each value literal, and the JSON
/// itself inert if the text is displayed whole.
fn literal_record_text(record: Map<String, Value>) -> Result<String, DispatchError> {
    let mut record = Value::Object(record);
    literal_journal_values(&mut record, &[]);
    serde_json::to_string(&record)
        .map(|json| solstone_core_format::markdown::literal_json_syntax(&json))
        .map_err(|_| DispatchError::Tool(ToolError::ReferenceNotFound))
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
    now: Timestamp,
    outcome: Outcome,
    reason: Reason,
) {
    let _ = audit::write_described_outcome(journal_root, coordinates, now, outcome, reason, false);
}

/// A named kind can be refused in words without revealing what the journal
/// holds. Other scope details are visible only in the owner's log.
fn scope_refusal(
    journal_root: &Path,
    principal: DispatchPrincipal<'_>,
    snapshot: &ConnectionReadSnapshot,
    entry: &ToolEntry,
    request: &ValidatedTool,
) -> Option<(Reason, Option<AdmittedCategory>)> {
    let named = match request {
        ValidatedTool::GetTranscript(request)
            if request.offset_ms.is_some()
                && !snapshot.categories.contains(&AdmittedCategory::Screenshots) =>
        {
            Some(AdmittedCategory::Screenshots)
        }
        ValidatedTool::Search(request) | ValidatedTool::Find(_, request) => request
            .category
            .into_iter()
            .chain(request.content_kinds.iter().flatten().copied())
            .find(|category| !snapshot.categories.contains(category)),
        _ => fetch_target_missing_kind(journal_root, principal, snapshot, request),
    };
    let named = named.or_else(|| match request {
        ValidatedTool::Search(request) | ValidatedTool::Find(_, request)
            if request.place.is_some()
                && !snapshot.categories.contains(&AdmittedCategory::Places) =>
        {
            Some(AdmittedCategory::Places)
        }
        _ => None,
    });
    if let Some(kind) = named {
        return Some((kind_reason(kind), Some(kind)));
    }
    if let Some(reason) = requirement_refusal(snapshot, entry) {
        return Some((reason, None));
    }
    if let ValidatedTool::Search(request) | ValidatedTool::Find(_, request) = request {
        if request.place.is_some() && !matches!(snapshot.scope, ConnectionScope::WholeJournal) {
            return Some((Reason::WholePlace, None));
        }
        if (request.person.is_some() || request.activity.is_some())
            && !snapshot.categories.contains(&AdmittedCategory::Facets)
        {
            return Some((Reason::PersonOrActivity, Some(AdmittedCategory::Facets)));
        }
    }
    if !arguments_within_scope(snapshot, request) {
        return Some((Reason::OutsideScope, None));
    }
    None
}

pub(crate) fn kind_refusal(kind: AdmittedCategory) -> String {
    kind_reason(kind).render(ENGLISH)
}

fn kind_reason(kind: AdmittedCategory) -> Reason {
    match kind {
        AdmittedCategory::Transcripts => Reason::ReadTranscripts,
        AdmittedCategory::Messages => Reason::ReadMessages,
        AdmittedCategory::Entities => Reason::ReadEntities,
        AdmittedCategory::ProfilePics => Reason::ReadProfilePics,
        AdmittedCategory::Facets => Reason::ReadFacets,
        AdmittedCategory::ScreenText => Reason::ReadScreenText,
        AdmittedCategory::Screenshots => Reason::ReadScreenshots,
        AdmittedCategory::Body => Reason::ReadBody,
        AdmittedCategory::Notes => Reason::ReadNotes,
        AdmittedCategory::Documents => Reason::ReadDocuments,
        AdmittedCategory::Calendar => Reason::ReadCalendar,
        AdmittedCategory::Places => Reason::ReadPlaces,
        AdmittedCategory::Photos => Reason::ReadPhotos,
    }
}

/// A reference follows its target's registered material category. An unresolved
/// token is left for the tool to report as a missing reference.
fn fetch_target_missing_kind(
    journal_root: &Path,
    principal: DispatchPrincipal<'_>,
    snapshot: &ConnectionReadSnapshot,
    request: &ValidatedTool,
) -> Option<AdmittedCategory> {
    let ValidatedTool::Fetch(fetch) = request else {
        return None;
    };
    // An activity's name follows the facets category whatever it names, so
    // the refusal says nothing of whether the activity is there. A segment
    // name has the same three-part slash syntax; leave it for `fetch` to
    // direct to `get_transcript` instead of reporting a missing facets grant.
    if crate::names::parse_activity(&fetch.reference).is_some()
        && !crate::names::is_segment_name_shape(&fetch.reference)
    {
        return (!snapshot.categories.contains(&AdmittedCategory::Facets))
            .then_some(AdmittedCategory::Facets);
    }
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
            use solstone_core_format::content::kinds::{
                additional_indexed_permission, agent_indexed_permission,
            };
            // The topic boundary does not disclose which inputs a saved reference may contain.
            let category = agent_indexed_permission(Some(journal_root), path, category)?;
            [Some(category), additional_indexed_permission(path)]
                .into_iter()
                .flatten()
                .find(|kind| !snapshot.categories.contains(kind))?
        }
        ReferenceTarget::Activity(_)
            if !snapshot.categories.contains(&AdmittedCategory::Facets) =>
        {
            AdmittedCategory::Facets
        }
        _ => return None,
    };
    Some(missing)
}

fn owner_denial_reason(reason: &'static str) -> Reason {
    match reason {
        "unenforceable" => Reason::Unenforceable,
        _ => Reason::NoRead,
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
fn owner_error_reason(error: DispatchError) -> Reason {
    match error {
        DispatchError::Tool(ToolError::IndexAbsent) => Reason::IndexAbsent,
        DispatchError::Tool(ToolError::IndexUnreadable) => Reason::IndexUnreadable,
        DispatchError::Tool(ToolError::IndexLocked) => Reason::IndexLocked,
        DispatchError::Tool(ToolError::EmptyIndex) => Reason::EmptyIndex,
        DispatchError::Tool(ToolError::NotIndexed) => Reason::NotIndexed,
        DispatchError::Tool(ToolError::FileUnreadable) => Reason::FileUnreadable,
        DispatchError::Tool(ToolError::ReferenceNotFound) => Reason::ReferenceMissing,
        DispatchError::Tool(ToolError::AuditUnavailable) => Reason::AuditUnavailable,
        DispatchError::Tool(
            ToolError::InvalidInput
            | ToolError::UseGetTranscript
            | ToolError::SourceArguments
            | ToolError::ActivityDayArguments
            | ToolError::TranscriptTimeArguments
            | ToolError::WeekRangeArguments,
        )
        | DispatchError::InvalidInput => Reason::InvalidArguments,
        DispatchError::Tool(ToolError::SuggestionOperationConflict) => Reason::OperationConflict,
        DispatchError::PermissionDenied(_) | DispatchError::KindDenied(_) => {
            Reason::OutsidePermission
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
        ValidatedTool::Search(request) | ValidatedTool::Find(_, request) => {
            arguments.insert("query".to_owned(), request.query.clone().into());
            arguments.insert("limit".to_owned(), request.limit.into());
            insert_present(&mut arguments, "day", request.day.as_deref());
            insert_present(&mut arguments, "day_from", request.day_from.as_deref());
            insert_present(&mut arguments, "day_to", request.day_to.as_deref());
            insert_present(&mut arguments, "facet", request.facet_id.as_deref());
            insert_present(&mut arguments, "person", request.person.as_deref());
            insert_present(&mut arguments, "place", request.place.as_deref());
            insert_present(&mut arguments, "activity", request.activity.as_deref());
            if let Some(kinds) = &request.content_kinds {
                arguments.insert(
                    "kind".into(),
                    json!(kinds.iter().map(category_token).collect::<Vec<_>>()),
                );
            }
            if let Some(category) = request.category {
                arguments.insert("category".to_owned(), category_token(&category).into());
            }
            if let Some(cursor) = request.cursor.as_deref() {
                arguments.insert("cursor".to_owned(), reference_fingerprint(cursor).into());
            }
        }
        ValidatedTool::ReadBatch {
            references, size, ..
        } => {
            arguments.insert(
                "references".to_owned(),
                json!(
                    references
                        .iter()
                        .map(|reference| reference_fingerprint(reference))
                        .collect::<Vec<_>>()
                ),
            );
            if let Some(size) = size {
                arguments.insert("size".to_owned(), size.as_str().into());
            }
        }
        ValidatedTool::Fetch(request) => {
            arguments.insert(
                "reference".to_owned(),
                reference_fingerprint(&request.reference).into(),
            );
            if let Some(size) = request.size {
                arguments.insert("size".to_owned(), size.as_str().into());
            }
        }
        ValidatedTool::ListTranscripts(request) => {
            arguments.insert("limit".to_owned(), request.limit.into());
            insert_present(&mut arguments, "day", request.day.as_deref());
            insert_present(&mut arguments, "day_from", request.day_from.as_deref());
            insert_present(&mut arguments, "day_to", request.day_to.as_deref());
            insert_present(&mut arguments, "facet", request.facet_id.as_deref());
            insert_present(&mut arguments, "time_from", request.time_from.as_deref());
            insert_present(&mut arguments, "time_to", request.time_to.as_deref());
            insert_present(&mut arguments, "source", request.source.as_deref());
            insert_present(&mut arguments, "entry_kind", request.kind.as_deref());
            if let Some(cursor) = request.cursor.as_deref() {
                arguments.insert("cursor".to_owned(), reference_fingerprint(cursor).into());
            }
        }
        ValidatedTool::GetTranscript(request) => {
            if let Some(offset) = request.offset_ms {
                arguments.insert("offset_ms".into(), offset.into());
            }
            insert_present(&mut arguments, "display", request.display.as_deref());
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
            match &request.query {
                crate::tools::week::WeekQuery::Single(week) => {
                    insert_present(&mut arguments, "week", week.as_deref());
                }
                crate::tools::week::WeekQuery::Range {
                    day_from,
                    day_to,
                    cursor,
                } => {
                    arguments.insert("day_from".to_owned(), day_from.clone().into());
                    arguments.insert("day_to".to_owned(), day_to.clone().into());
                    if let Some(cursor) = cursor.as_deref() {
                        arguments.insert("cursor".to_owned(), reference_fingerprint(cursor).into());
                    }
                }
            }
            if !request.include.is_empty() {
                arguments.insert(
                    "include".into(),
                    json!(
                        request
                            .include
                            .iter()
                            .map(category_token)
                            .collect::<Vec<_>>()
                    ),
                );
            }
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
        ValidatedTool::SuggestCorrection(request) => {
            return crate::suggestions::submit::recorded_arguments(request);
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
pub(crate) fn reference_fingerprint(token: &str) -> String {
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
    if matches!(
        entry.tool_name,
        crate::jsonrpc::ToolName::Fetch
            | crate::jsonrpc::ToolName::GetTranscript
            | crate::jsonrpc::ToolName::GetActivitySources
    ) && let Some(object) = arguments.and_then(Value::as_object)
        && let Some(references) = object.get("references")
    {
        let Some(references) = references.as_array() else {
            return Err(DispatchError::InvalidInput);
        };
        let allowed_keys = if entry.tool_name == crate::jsonrpc::ToolName::Fetch {
            object
                .keys()
                .all(|key| matches!(key.as_str(), "references" | "size"))
        } else {
            object.len() == 1
        };
        if !allowed_keys || !(1..=crate::tools::MAX_BATCH_REFERENCES).contains(&references.len()) {
            return Err(DispatchError::InvalidInput);
        }
        let size = if entry.tool_name == crate::jsonrpc::ToolName::Fetch {
            crate::tools::fetch::parse_size(object.get("size"))
                .map_err(|_| DispatchError::InvalidInput)?
        } else {
            None
        };
        let references = references
            .iter()
            .map(|reference| {
                reference
                    .as_str()
                    .filter(|value| {
                        !value.is_empty() && value.len() <= crate::tools::MAX_OPAQUE_REFERENCE_BYTES
                    })
                    .map(str::to_owned)
                    .ok_or(DispatchError::InvalidInput)
            })
            .collect::<Result<Vec<_>, _>>()?;
        return Ok(ValidatedTool::ReadBatch {
            tool: entry.tool_name,
            references,
            size,
        });
    }
    use crate::jsonrpc::ToolName;
    if matches!(
        entry.tool_name,
        ToolName::ListActivities | ToolName::ListTranscripts
    ) && let Some(object) = arguments.and_then(Value::as_object)
        && (["person", "place", "activity"]
            .iter()
            .any(|key| object.contains_key(*key))
            || (object.contains_key("kind")
                && !(entry.tool_name == ToolName::ListTranscripts
                    && object.contains_key("source")
                    && object.get("kind").is_some_and(Value::is_string))))
    {
        if object.keys().any(|key| {
            !matches!(
                key.as_str(),
                "day"
                    | "day_from"
                    | "day_to"
                    | "from"
                    | "to"
                    | "facet"
                    | "limit"
                    | "cursor"
                    | "kind"
                    | "person"
                    | "place"
                    | "activity"
            )
        }) {
            return Err(DispatchError::InvalidInput);
        }
        let request = crate::tools::search::validate(arguments).map_err(|error| match error {
            ToolError::InvalidInput => DispatchError::InvalidInput,
            error => DispatchError::Tool(error),
        })?;
        return Ok(ValidatedTool::Find(entry.tool_name, request));
    }
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
        crate::jsonrpc::ToolName::SuggestCorrection => {
            crate::suggestions::submit::validate(arguments).map(ValidatedTool::SuggestCorrection)
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
    now: Timestamp,
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
        .map(|category| category.as_str().to_owned())
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
    let read = crate::tools::week::read_week(journal_root, &request, principal, snapshot)
        .map_err(DispatchError::Tool)?;
    // A said entry's uri and the already-literal preview snippets keep their bytes.
    Ok(Prepared::new(read.value, read.count, read.targets).served_as_is(&["uri", "snippet"]))
}

/// Maximum photo items included per day in `get_week(include=["photos"])`.
/// Keeps single-week and 96 KiB range pages well within client limits even on
/// unusually busy days while returning full days without truncation in ordinary
/// use; days with more set `"more": true`.
pub(crate) const MAX_WEEK_PHOTOS_PER_DAY: usize = 50;
const MAX_WEEK_NOTES_PER_DAY: usize = 8;
/// Safety bound on cursor pages pulled for one `get_week` page's kind summary
/// (up to 2,000 rows per page).
const MAX_WEEK_KIND_PAGES: usize = 20;

/// Chronological sort key for photos within a single day: timed photos first in
/// ascending `HH:MM:SS` order, followed by untimed (`upload_fallback`) photos
/// in deterministic path order.
fn photo_chronological_key<'a>(row: &'a Value, target: &'a str) -> (bool, &'a str, &'a str) {
    let time = row.get("time").and_then(Value::as_str);
    (time.is_none(), time.unwrap_or(""), target)
}

/// Pre-fetched index-backed daily summaries (`photos` or `notes`) for one
/// `get_week` page span, keyed by day so audit targets are attributed strictly
/// to days the page actually serves.
pub(crate) struct WeekKindSummaries {
    by_day: BTreeMap<String, (Value, Vec<String>)>,
    cutoff_day: Option<String>,
    range_incomplete: bool,
    error_note: Option<String>,
}

impl WeekKindSummaries {
    pub(crate) fn take_day(&mut self, day: &str) -> (Value, Vec<String>) {
        if let Some(note) = &self.error_note {
            return (json!({"note": note}), Vec::new());
        }
        if let Some(entry) = self.by_day.remove(day) {
            return entry;
        }
        let mut summary = json!({"count": 0, "items": []});
        if self.range_incomplete || self.cutoff_day.as_deref().is_some_and(|cut| day <= cut) {
            summary["count_is_lower_bound"] = json!(true);
        }
        (summary, Vec::new())
    }
}

/// Read admitted rows for one index-backed `get_week.include` kind across a
/// whole page span (`day_from..=day_to`) in batched range queries instead of
/// scanning the index once per day, preserving the exact search boundary,
/// live release checks and per-day audit target attribution.
pub(crate) fn week_kind_summaries_range(
    journal: &Path,
    principal: DispatchPrincipal<'_>,
    snapshot: &ConnectionReadSnapshot,
    day_from: &str,
    day_to: &str,
    kind: AdmittedCategory,
) -> WeekKindSummaries {
    let mut grouped: BTreeMap<String, Vec<(Value, String)>> = BTreeMap::new();
    let mut cursor: Option<String> = None;
    let mut range_incomplete = false;
    let mut cutoff_day: Option<String> = None;
    let mut oldest_seen_day: Option<String> = None;

    for page_idx in 0..MAX_WEEK_KIND_PAGES {
        let request = ValidatedSearch {
            query: String::new(),
            limit: 100,
            cursor: cursor.clone(),
            day: None,
            day_from: Some(day_from.to_owned()),
            day_to: Some(day_to.to_owned()),
            category: None,
            content_kinds: Some(BTreeSet::from([kind])),
            facet_id: None,
            person: None,
            place: None,
            activity: None,
        };
        let mut result = match search(journal, principal, snapshot, request) {
            Ok(result) => result,
            Err(DispatchError::Tool(error)) => {
                if page_idx == 0 {
                    return WeekKindSummaries {
                        by_day: BTreeMap::new(),
                        cutoff_day: None,
                        range_incomplete: false,
                        error_note: Some(error.agent_message().to_owned()),
                    };
                }
                range_incomplete = true;
                break;
            }
            Err(_) => {
                if page_idx == 0 {
                    return WeekKindSummaries {
                        by_day: BTreeMap::new(),
                        cutoff_day: None,
                        range_incomplete: false,
                        error_note: Some("this day's entries could not be read".to_owned()),
                    };
                }
                range_incomplete = true;
                break;
            }
        };
        if result.value["coverage"]["live_examination_complete"] != true
            || result.value["coverage"]["index_classification_complete"] != true
            || result.value["degraded"] == true
        {
            range_incomplete = true;
        }
        let next = result.value["next_cursor"].as_str().map(str::to_owned);
        let rows = result
            .value
            .get_mut("results")
            .and_then(Value::as_array_mut)
            .map(std::mem::take)
            .unwrap_or_default();
        for (row, target) in rows.into_iter().zip(result.targets) {
            let day = row["date"].as_str().unwrap_or_default().to_owned();
            if !day.is_empty()
                && oldest_seen_day
                    .as_deref()
                    .is_none_or(|oldest| day.as_str() < oldest)
            {
                oldest_seen_day = Some(day.clone());
            }
            grouped.entry(day).or_default().push((row, target));
        }
        if next.is_none() {
            break;
        }
        if page_idx + 1 == MAX_WEEK_KIND_PAGES {
            cutoff_day = oldest_seen_day.clone().or_else(|| Some(day_to.to_owned()));
            break;
        }
        cursor = next;
    }

    let limit = match kind {
        AdmittedCategory::Photos => MAX_WEEK_PHOTOS_PER_DAY,
        AdmittedCategory::Notes => MAX_WEEK_NOTES_PER_DAY,
        _ => 3,
    };
    let mut by_day = BTreeMap::new();
    for (day, mut entries) in grouped {
        if kind == AdmittedCategory::Photos {
            entries.sort_by(|(a_row, a_target), (b_row, b_target)| {
                photo_chronological_key(a_row, a_target)
                    .cmp(&photo_chronological_key(b_row, b_target))
            });
        }
        let total = entries.len();
        let mut items = Vec::with_capacity(total.min(limit));
        for (row, _) in entries.iter().take(limit) {
            let mut item = json!({"title": row["title"]});
            for field in ["source", "reference", "link"] {
                if let Some(value) = row.get(field) {
                    item[field] = value.clone();
                }
            }
            if kind == AdmittedCategory::Photos {
                for field in ["time", "time_source", "description"] {
                    if let Some(value) = row.get(field) {
                        item[field] = value.clone();
                    }
                }
            }
            items.push(item);
        }
        let mut summary = json!({"count": total, "items": items});
        if total > limit {
            summary["more"] = json!(true);
        }
        if range_incomplete || cutoff_day.as_deref().is_some_and(|cut| day.as_str() <= cut) {
            summary["count_is_lower_bound"] = json!(true);
        }
        let day_targets = entries.into_iter().map(|(_, target)| target).collect();
        by_day.insert(day, (summary, day_targets));
    }

    WeekKindSummaries {
        by_day,
        cutoff_day,
        range_incomplete,
        error_note: None,
    }
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
        ValidatedTool::Find(tool, request) => {
            let mut found = search(journal_root, principal, snapshot, request)?;
            let rows_key = match tool {
                crate::jsonrpc::ToolName::ListActivities => "activities",
                crate::jsonrpc::ToolName::ListTranscripts => "transcripts",
                _ => unreachable!("only list tools use the find projection"),
            };
            found.value[rows_key] = found
                .value
                .as_object_mut()
                .expect("search result object")
                .remove("results")
                .expect("search rows");
            Ok(found)
        }
        ValidatedTool::Fetch(request) => fetch(journal_root, principal, snapshot, request),
        ValidatedTool::ReadBatch {
            tool,
            references,
            size,
        } => Ok(read_batch(
            journal_root,
            principal,
            snapshot,
            tool,
            references,
            size,
        )),
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
        ValidatedTool::SuggestCorrection(request) => crate::suggestions::submit::execute(
            journal_root,
            principal,
            snapshot,
            request,
            Timestamp::now(),
        ),
    }
}

/// Batch one page per reference through the existing readers. The outer
/// dispatch still admits and releases the entire response under one grant.
fn read_batch(
    journal_root: &Path,
    principal: DispatchPrincipal<'_>,
    snapshot: &ConnectionReadSnapshot,
    tool: crate::jsonrpc::ToolName,
    references: Vec<String>,
    size: Option<crate::photo_picture::PictureSize>,
) -> Prepared {
    use crate::jsonrpc::ToolName;

    const MAX_ITEM_TEXT_BYTES: usize = 192 * 1024;
    const MAX_IMAGE_BYTES: usize = 3 * 1024 * 1024;
    let mut items = Vec::with_capacity(references.len());
    let mut batch = Prepared::new(Value::Null, 0, Vec::new());
    let mut text_bytes = 0;
    let mut image_bytes = 0;
    for (index, reference) in references.into_iter().enumerate() {
        let request = match tool {
            ToolName::Fetch => ValidatedTool::Fetch(ValidatedFetch { reference, size }),
            ToolName::GetTranscript => ValidatedTool::GetTranscript(ValidatedGetTranscript {
                offset_ms: None,
                display: None,
                reference,
                cursor: None,
            }),
            ToolName::GetActivitySources => {
                ValidatedTool::GetActivitySources(ValidatedGetActivitySources {
                    reference,
                    cursor: None,
                })
            }
            _ => unreachable!("only reference readers are batchable"),
        };
        if let Some((message, _)) = scope_refusal(
            journal_root,
            principal,
            snapshot,
            crate::registry::find_tool(tool),
            &request,
        ) {
            items.push(json!({
                "index": index,
                "error": {"code": "permission_denied", "message": message.render(ENGLISH)},
            }));
            continue;
        }
        let read = execute_validated(journal_root, principal, snapshot, request);
        let mut item = match read {
            Ok(item) => item,
            Err(error) => {
                let error = match error {
                    DispatchError::Tool(error) => error,
                    _ => ToolError::ReferenceNotFound,
                };
                items.push(json!({
                    "index": index,
                    "error": {"code": error.reason(), "message": error.agent_message()},
                }));
                continue;
            }
        };
        literal_journal_values(&mut item.value, item.served_as_is);
        let result = crate::jsonrpc::with_note(item.value, item.empty_note.as_deref());
        let mut row = json!({"index": index, "result": result});
        if !item.images.is_empty() {
            // The text result is content[0], followed by the images.
            row["image_content_index"] = json!(batch.images.len() + 1);
        }
        let bytes = row.to_string().len();
        let image_size: usize = item.images.iter().map(Vec::len).sum();
        if text_bytes + bytes > MAX_ITEM_TEXT_BYTES || image_bytes + image_size > MAX_IMAGE_BYTES {
            items.push(json!({
                "index": index,
                "error": {
                    "code": "batch_size_limit",
                    "message": "read this reference separately or in a smaller batch",
                },
            }));
            continue;
        }
        items.push(row);
        text_bytes += bytes;
        image_bytes += image_size;
        batch.count += item.count;
        batch.targets.extend(item.targets);
        batch.images.extend(item.images);
    }
    batch.value = json!({"items": items});
    // Each successful item was made literal with its own reader's rules.
    batch.served_as_is(&["result"])
}

fn arguments_within_scope(snapshot: &ConnectionReadSnapshot, request: &ValidatedTool) -> bool {
    let requested_facet = match request {
        ValidatedTool::Search(request) | ValidatedTool::Find(_, request) => {
            request.facet_id.as_deref()
        }
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

/// The segment route applies this scope, then checks each eligible file.
pub(crate) fn segment_boundary(
    snapshot: &ConnectionReadSnapshot,
    facet: Option<&str>,
) -> Option<ConnectionBoundary> {
    crate::registry::SEGMENT_KINDS
        .iter()
        .any(|category| snapshot.categories.contains(category))
        .then(|| boundary(snapshot, None, facet))
}

pub(crate) fn live_allows(
    journal_root: &Path,
    boundary: &ConnectionBoundary,
    path: &str,
    stream: Option<&str>,
) -> bool {
    FacetDeclarationSet::from_journal(journal_root)
        .map(|declarations| classify_source(journal_root, path, stream, &declarations))
        .is_ok_and(|classification| boundary.allows_classification(journal_root, &classification))
}

/// The reference an answer gives for an activity: its name, which stays the
/// same from one answer to the next, or a sealed reference when a part of the
/// name can't be written in one.
pub(crate) fn activity_reference(
    principal: DispatchPrincipal<'_>,
    snapshot: &ConnectionReadSnapshot,
    target: ActivityReference,
) -> Result<String, DispatchError> {
    if let Some(name) =
        crate::names::activity_name(&target.day, &target.facet_dir, &target.record_identity)
    {
        return Ok(name);
    }
    codec()
        .mint(
            principal.connection,
            snapshot.generation,
            ReferenceTarget::Activity(target),
        )
        .map_err(|_| DispatchError::Tool(ToolError::ReferenceNotFound))
}

/// The reference an answer gives for a segment: its name, or a sealed
/// reference when the name can't be written.
pub(crate) fn segment_reference(
    journal_root: &Path,
    principal: DispatchPrincipal<'_>,
    snapshot: &ConnectionReadSnapshot,
    target: SegmentReference,
) -> Result<String, DispatchError> {
    if let Some(name) =
        crate::names::segment_name(journal_root, &target.day, &target.stream, &target.segment)
    {
        return Ok(name);
    }
    codec()
        .mint(
            principal.connection,
            snapshot.generation,
            ReferenceTarget::Segment(target),
        )
        .map_err(|_| DispatchError::Tool(ToolError::ReferenceNotFound))
}

/// The reference an answer gives for an entity in one facet: its name, or a
/// sealed reference when the name can't be written.
pub(crate) fn entity_reference(
    principal: DispatchPrincipal<'_>,
    snapshot: &ConnectionReadSnapshot,
    facet_dir: Option<&str>,
    target: EntityReference,
) -> Result<String, DispatchError> {
    if let Some(name) = facet_dir.and_then(|dir| crate::names::entity_name(dir, &target.entity_id))
    {
        return Ok(name);
    }
    codec()
        .mint(
            principal.connection,
            snapshot.generation,
            ReferenceTarget::Entity(target),
        )
        .map_err(|_| DispatchError::Tool(ToolError::ReferenceNotFound))
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
        || request
            .category
            .is_some_and(|kind| kind != AdmittedCategory::Entities)
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
    // Read once a facet needs it: it reads every entity in the journal.
    let mut identities = None;

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
        if identities.is_none() {
            identities = solstone_core_facets::read_identity_map(journal_root).ok();
        }
        let scoped = match identities.as_ref().map_or(
            Err(DispatchError::Tool(ToolError::FileUnreadable)),
            |identities| read_scoped_entities_resolved(journal_root, &facet_dir, identities),
        ) {
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

        let reference = match entity_reference(
            principal,
            snapshot,
            Some(&m.facet_dir),
            EntityReference {
                facet_id: m.facet_id.clone(),
                entity_id: m.entity_id.clone(),
            },
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
        crate::journal_links::attach(
            journal_root,
            &mut card_val,
            solstone_core_journal_link::Place::Person {
                id: m.entity_id.clone(),
            },
        );
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
    mut request: ValidatedSearch,
) -> Result<Prepared, DispatchError> {
    let filtered = request.category.is_some()
        || request.content_kinds.is_some()
        || request.activity.is_some()
        || request.person.is_some()
        || request.place.is_some();
    let implicit_end = filtered
        && request.query.trim().is_empty()
        && request.day.is_none()
        && request.day_from.is_none()
        && request.day_to.is_none();
    let boundary =
        boundary(snapshot, None, request.facet_id.as_deref()).filtered_to(request.category);
    let boundary = if let Some(kinds) = &request.content_kinds {
        boundary.with_kind_filter(Some(kinds.clone()))
    } else {
        boundary
    };
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
    let mut query_identity = json!({
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
    });
    if let Some(kinds) = &request.content_kinds {
        query_identity["content_kind"] =
            json!(kinds.iter().map(category_token).collect::<Vec<_>>());
    }
    for (key, value) in [
        ("person", &request.person),
        ("place", &request.place),
        ("activity", &request.activity),
    ] {
        if let Some(value) = value {
            query_identity[key] = json!(value);
        }
    }
    let query_hash = digest_json(&query_identity);
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
    if implicit_end {
        // Blank-query browsing is newest-first. On a later page the cursor
        // already bounds it, so the query identity does not depend on midnight.
        request.day_to = Some(start_after.as_ref().map_or_else(
            || {
                solstone_core_time::day_key(
                    Timestamp::now()
                        .to_zoned_datetime(solstone_core_journal_config::owner_zone(journal_root))
                        .date(),
                )
            },
            |anchor| anchor.day.clone(),
        ));
    }
    let incoming_cursor_day = start_after.as_ref().map(|a| a.day.clone());
    let mut locations = BTreeMap::new();
    let mut location_documents = BTreeMap::new();
    let may_read_locations = snapshot.categories.contains(&AdmittedCategory::Places)
        && matches!(snapshot.scope, ConnectionScope::WholeJournal);
    let mut results = Vec::new();
    let mut owner_links = crate::journal_links::Recordings::default();
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
    // Possible-name rows are admitted inside the query, so one this connection
    // may not see never matches, counts, relaxes the query or anchors a cursor.
    let possible_names = ConnectionPossibleNames::new(journal_root, snapshot);
    let mut checker = LiveTranscriptPages::new(Arc::clone(&possible_names), snapshot);
    let mut import_previews = BTreeMap::new();
    let mut sources: Option<BTreeMap<String, String>> = None;
    while results.len() < request.limit && examined < MAX_SEARCH_EXAMINED_ROWS {
        // One query reads each activity day file once.
        let day_files = crate::body_source::ActivityDayFiles::default();
        let batch = (MAX_SEARCH_EXAMINED_ROWS - examined).min(100);
        let response = search_connection_with_possible_names(
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
            Timestamp::now()
                .to_zoned_datetime(solstone_core_journal_config::owner_zone(journal_root))
                .date(),
            possible_names.admission(),
            crate::body_source::body_entry_admission(journal_root, snapshot, day_files.clone()),
        )
        .map_err(index_error)?;
        index_coverage_complete &= response.coverage_complete;
        degraded |= response.degraded.is_some();
        relaxed |= response.relaxed;
        let batch_len = response.results.len();
        if batch_len == 0 {
            break;
        }
        let facet_declarations = FacetDeclarationSet::from_journal(journal_root).ok();
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
            let topic = if topic_segment(&hit.metadata.path).is_some() {
                let Some(document) =
                    TopicDocument::read(journal_root, &hit.metadata.path).filter(|doc| {
                        boundary.allows_topic(doc)
                            && doc.matches_chunk(&hit.metadata.path, hit.metadata.idx, &hit.text)
                    })
                else {
                    live_dropped = true;
                    continue;
                };
                Some(document)
            } else {
                None
            };
            let Some(classification) = facet_declarations
                .as_ref()
                .map(|declarations| {
                    classify_source(
                        journal_root,
                        &hit.metadata.path,
                        Some(&hit.metadata.stream),
                        declarations,
                    )
                })
                .filter(|classification| {
                    if topic.is_some() {
                        boundary.allows_scope(classification)
                    } else {
                        boundary.allows_classification(journal_root, classification)
                    }
                })
            else {
                live_dropped = true;
                continue;
            };
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
            // A body record's entry reads as its composed record does for this
            // connection, a group's hide included.
            let hidden = if record
                .as_ref()
                .is_some_and(solstone_core_facets::is_body_record)
            {
                let Some(face) = crate::body_source::body_entry_face(
                    journal_root,
                    snapshot,
                    &day_files,
                    &hit.metadata.path,
                    hit.metadata.idx,
                ) else {
                    live_dropped = true;
                    continue;
                };
                Some(face.hidden)
            } else {
                hidden
            };
            // A hidden activity is never returned: indexing leaves its entry
            // out, and this drops one a body group hides through another
            // member, or one indexed before that.
            if hidden == Some(true) {
                continue;
            }
            // A possible name serves its label and the words it labels, which
            // the segment's transcript grant (this row's own classification)
            // lets this connection read.
            let is_possible_name = is_possible_names_path(&hit.metadata.path);
            let served_text = if is_possible_name {
                let Some(entry) =
                    possible_names.indexed(&hit.metadata.path, hit.metadata.idx, &hit.text)
                else {
                    live_dropped = true;
                    continue;
                };
                crate::possible_names::matched_text(&entry)
            } else {
                served_text
            };
            let photo = crate::photo_picture::is_photo_description(&hit.metadata.path);
            let category = topic
                .as_ref()
                .and_then(|doc| {
                    doc.selected_kinds()
                        .find(|kind| boundary.selects_kind(*kind))
                })
                .unwrap_or_else(|| {
                    solstone_core_format::content::kinds::indexed_permission(
                        Some(journal_root),
                        &hit.metadata.path,
                        classification
                            .category
                            .and_then(AdmittedCategory::from_stored_word)
                            .unwrap_or(AdmittedCategory::Transcripts),
                    )
                });
            if request.query.trim().is_empty()
                && request.content_kinds.is_some()
                && match category {
                    AdmittedCategory::Photos => !photo || hit.metadata.idx != 0,
                    AdmittedCategory::Notes => topic.is_some() || hit.metadata.idx != 0,
                    _ => false,
                }
            {
                continue;
            }
            if !matches_find_filters(&request, record.as_ref(), category) {
                continue;
            }
            let mut activity_source = None;
            if filtered
                && let (Some(record), Some(id), Some((facet, day))) = (
                    &record,
                    record_identity.as_deref(),
                    crate::body_source::activity_file_place(&hit.metadata.path),
                )
            {
                let Some(seen) =
                    crate::may_see::activity_at(journal_root, snapshot, &facet, &day, id, None)
                else {
                    continue;
                };
                let shown = match &seen.record {
                    crate::may_see::SeenRecord::Activity(row)
                    | crate::may_see::SeenRecord::Body(row) => row,
                };
                if shown != record {
                    live_dropped = true;
                    continue;
                }
                activity_source = Some(activity_reference(
                    principal,
                    snapshot,
                    ActivityReference {
                        facet_id: seen.place.facet_id,
                        facet_dir: seen.place.facet_dir,
                        day,
                        record_identity: id.to_owned(),
                        file_digest: seen.file_digest,
                        unit_index: None,
                        source_binding: None,
                    },
                )?);
            }
            if category == AdmittedCategory::Places
                && let Some(day) = location_file_day(&hit.metadata.path)
            {
                let live = location_documents
                    .entry(day.to_owned())
                    .or_insert_with(|| current_location_document(journal_root, day));
                if !verified_markdown_document(
                    journal_root,
                    &hit.metadata.path,
                    hit.metadata.idx,
                    &served_text,
                )
                .is_some_and(|text| {
                    live.as_ref()
                        .is_none_or(|current| current.as_deref() == Some(&text))
                }) {
                    live_dropped = true;
                    continue;
                }
            }
            if let Some(place) = &request.place {
                if !may_read_locations {
                    continue;
                }
                let place = fold_literal(place);
                let matched = if category == AdmittedCategory::Places {
                    fold_literal(&served_text).contains(&place)
                } else {
                    record
                        .as_ref()
                        .and_then(|record| {
                            let span = solstone_core_facets::activity_span(record)?;
                            let stream = record.get("stream").and_then(Value::as_str);
                            locations
                                .entry(hit.metadata.day.clone())
                                .or_insert_with(|| {
                                    crate::tools::week::activity_locations(
                                        journal_root,
                                        &hit.metadata.day,
                                    )
                                })
                                .as_ref()
                                .and_then(|(store, day)| {
                                    solstone_core_location::location_beside(
                                        day, store, span, stream,
                                    )
                                })
                        })
                        .is_some_and(|location| fold_literal(&location.words).contains(&place))
                };
                if !matched {
                    continue;
                }
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

            let imported = photo
                || (category == AdmittedCategory::Documents
                    && hit.metadata.path.ends_with("/document_transcript.md"));
            let preview = if imported {
                let content = import_preview_content(&served_text);
                if !photo && !content.is_empty() {
                    Some(content.chars().take(MAX_SNIPPET_CHARS).collect::<String>())
                } else {
                    import_previews
                        .entry(hit.metadata.path.clone())
                        .or_insert_with(|| {
                            if photo
                                && let Some(full_text) = verified_markdown_document(
                                    journal_root,
                                    &hit.metadata.path,
                                    hit.metadata.idx,
                                    &served_text,
                                )
                            {
                                match crate::photo_picture::photo_preview_from_document(
                                    &full_text,
                                    MAX_SNIPPET_CHARS,
                                ) {
                                    crate::photo_picture::PhotoDocumentPreview::Description(
                                        text,
                                    ) => {
                                        return Some(text);
                                    }
                                    crate::photo_picture::PhotoDocumentPreview::Empty => {
                                        return None;
                                    }
                                    crate::photo_picture::PhotoDocumentPreview::Unstructured => {}
                                }
                            }
                            match read_indexed_path_entries(
                                journal_root,
                                &boundary,
                                &hit.metadata.path,
                                MAX_IMPORT_PREVIEW_ENTRIES,
                                MAX_INDEXED_ENTRY_BYTES,
                            ) {
                                Ok(indexed)
                                    if indexed
                                        .entries
                                        .iter()
                                        .any(|entry| entry == &served_text) =>
                                {
                                    import_preview(&indexed.entries, photo)
                                }
                                Ok(_) => None,
                                Err(_) => {
                                    degraded = true;
                                    None
                                }
                            }
                        })
                        .clone()
                }
            } else {
                None
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
                || topic.as_ref().is_some_and(|doc| {
                    TopicDocument::read(journal_root, &hit.metadata.path).as_ref() != Some(doc)
                })
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
            let hit_activity_id = record_identity.clone();
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
                        markdown_fingerprint: (!is_screen_text
                            && hit.metadata.path.ends_with(".md"))
                        .then(|| digest(&hit.text)),
                        source_digest,
                        byte_offset,
                    }),
                )
                .map_err(|_| DispatchError::Tool(ToolError::ReferenceNotFound))?;
            let mut value = json!({
                "title": entry_title(&hit.metadata.day),
                "date": hit.metadata.day,
                "snippet": if is_screen_text {
                    screen_text_snippet(&served_text)
                } else if imported {
                    snippet(preview.as_deref().unwrap_or_default())
                } else {
                    snippet(&served_text)
                },
                "reference": reference,
            });
            owner_links.entry(
                journal_root,
                &mut value,
                &hit.metadata.path,
                hit_activity_id.as_deref(),
            );
            let hit_kind = if record.as_ref().is_some_and(|record| {
                solstone_core_facets::is_body_record(record)
                    || solstone_core_format::content::is_strava_reading(record)
            }) {
                AdmittedCategory::Body
            } else {
                category
            };
            if let Some(doc) = &topic {
                insert_topic_kinds(&mut value, doc);
            } else {
                insert_indexed_kind(&mut value, &hit.metadata.path, hit_kind);
            }
            if let Some(stream) =
                indexed_hit_stream(&hit.metadata.stream, &hit.metadata.path, record.as_ref())
            {
                let labels = sources.get_or_insert_with(|| source_labels(journal_root));
                value["source"] = json!(source_label(labels, stream));
            }
            if let Some(reference) = activity_source {
                value["activity_reference"] = json!(reference);
            }
            if let Some(record) = &record {
                if request.activity.is_some() || request.content_kinds.is_some() {
                    value["title"] = json!(crate::tools::activities::activity_title(record));
                }
            } else if imported || category == AdmittedCategory::Notes {
                let title = if category == AdmittedCategory::Notes {
                    note_title(&served_text)
                } else {
                    material_title(&served_text)
                };
                if let Some(title) = title {
                    value["title"] = json!(title);
                }
                if let Some(preview) = preview {
                    value["description"] = json!(preview);
                }
                if value.get("link").is_none()
                    && solstone_core_journal_io::is_day_key(&hit.metadata.day)
                {
                    crate::journal_links::attach(
                        journal_root,
                        &mut value,
                        solstone_core_journal_link::Place::Day {
                            day: hit.metadata.day.clone(),
                        },
                    );
                }
            }
            if photo {
                let clock =
                    crate::photo_picture::photo_clock_info(journal_root, &hit.metadata.path);
                if let Some(time) = clock.time {
                    value["time"] = Value::String(time);
                }
                if let Some(time_source) = clock.time_source {
                    value["time_source"] = Value::String(time_source.to_owned());
                }
            }
            if is_possible_name {
                // Tentative: the journal's guess at who or what was meant.
                value["possible_name"] = Value::Bool(true);
            }
            if let Some(offset_ms) = hit.metadata.group_offset_ms {
                let segment = solstone_core_format::content::screen_text::screen_text_segment(
                    &hit.metadata.path,
                );
                if let Some(segment) = segment {
                    value["segment"] = Value::String(segment.to_owned());
                }
                value["minute"] = Value::String(
                    solstone_core_format::content::screen_text::screen_text_minute(offset_ms),
                );
                // When it was on screen, on the recording device's clock: the
                // offset alone reads as midnight for a screenshot.
                if let Some(time) = segment.and_then(|segment| {
                    solstone_core_format::content::screen_text::screen_text_clock(
                        segment, offset_ms,
                    )
                }) {
                    value["time"] = Value::String(time);
                }
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
    let anchor_day = anchor.as_ref().map(|a| a.day.clone());
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
    let pure_photo_browse =
        request.query.trim().is_empty()
            && (request.content_kinds.as_ref().is_some_and(|kinds| {
                kinds.len() == 1 && kinds.contains(&AdmittedCategory::Photos)
            }) || request.category == Some(AdmittedCategory::Photos));
    if pure_photo_browse && results.len() > 1 && results.len() == targets.len() {
        let trailing_split_day = if next_cursor.is_some() {
            anchor_day.as_deref()
        } else {
            None
        };
        let leading_split_day = incoming_cursor_day.as_deref();
        let mut paired: Vec<(Value, String)> = results.into_iter().zip(targets).collect();
        let mut start = 0;
        while start < paired.len() {
            let run_day = paired[start]
                .0
                .get("date")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned();
            let mut end = start + 1;
            while end < paired.len()
                && paired[end].0.get("date").and_then(Value::as_str) == Some(run_day.as_str())
            {
                end += 1;
            }
            if end - start > 1
                && trailing_split_day != Some(run_day.as_str())
                && leading_split_day != Some(run_day.as_str())
            {
                paired[start..end].sort_by(|(a_row, a_target), (b_row, b_target)| {
                    photo_chronological_key(a_row, a_target)
                        .cmp(&photo_chronological_key(b_row, b_target))
                });
            }
            start = end;
        }
        (results, targets) = paired.into_iter().unzip();
    }
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
    // A facet-limited connection never sees what belongs to no facet, such as
    // calendar events. Saying so keeps that boundary from reading as an empty
    // day.
    if !matches!(boundary.scope(), ConnectionScope::WholeJournal) {
        response_val["coverage"]["outside_facets"] = json!(SEARCH_OUTSIDE_FACETS);
    }
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
    // A connection without the body permission is told when its search would
    // have found body data with it. Once, on the first page.
    let body_withheld = request.cursor.is_none()
        && request.content_kinds.is_none()
        && search_withheld_body(journal_root, snapshot, &boundary, &resolution, &request)?;
    if body_withheld {
        response_val["body_note"] = json!(crate::body_source::BODY_NOT_GRANTED);
    }
    // Each snippet is made literal where it is cut.
    let mut prepared =
        Prepared::new(response_val, count, targets).served_as_is(&["snippet", "query"]);
    if count == 0 && body_withheld {
        prepared = prepared.when_empty(crate::body_source::BODY_NOT_GRANTED);
    } else if count == 0 && resolution.status != EntityResolutionStatus::Incomplete {
        prepared = prepared.when_empty(if complete {
            SEARCH_EMPTY
        } else {
            SEARCH_EMPTY_INCOMPLETE
        });
    }
    Ok(prepared)
}

/// Prefer a heading from the material itself, whenever one exists.
fn material_title(text: &str) -> Option<String> {
    let line = text.lines().find(|line| !line.trim().is_empty())?.trim();
    let heading = line.trim_start_matches('#').strip_prefix(' ')?.trim();
    (!heading.is_empty()).then(|| heading.chars().take(160).collect())
}

/// Prefer a heading for a note; fall back to its first non-empty line when the
/// note has no heading (Obsidian notes and agent-memory notes).
fn note_title(text: &str) -> Option<String> {
    if let Some(heading) = material_title(text) {
        return Some(heading);
    }
    let line = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.chars().all(|ch| ch == '#'))?;
    Some(line.chars().take(160).collect())
}

/// The index keeps an import header and its content in separate chunks.
fn import_preview_content(chunk: &str) -> &str {
    let mut content = chunk.trim();
    while let Some((line, rest)) = content.split_once('\n') {
        if !line.starts_with("# ") && !line.starts_with("## ") {
            break;
        }
        content = rest.trim_start();
    }
    let first = content.lines().next().unwrap_or_default().trim();
    if matches!(first, "Type: Image" | "Type: Document")
        || first
            .trim_start_matches(['[', ' '])
            .starts_with("page text unavailable")
        || first
            .trim_start_matches(['[', ' '])
            .starts_with("image description unavailable")
    {
        return "";
    }
    content
}

fn import_preview(entries: &[String], photo: bool) -> Option<String> {
    let chunks = || entries.iter().map(|entry| import_preview_content(entry));
    let text: std::borrow::Cow<'_, str> = if photo {
        chunks()
            .find_map(|chunk| {
                let (heading, body) = chunk.split_once('\n')?;
                if heading.trim_matches(['[', ']', ' ']) != "image description — model-derived" {
                    return None;
                }
                let body = body.trim();
                (!body.is_empty()
                    && body != "unavailable"
                    && !body.starts_with("unavailable —")
                    && !body.starts_with("unavailable -"))
                .then(|| {
                    std::borrow::Cow::Owned(crate::photo_picture::repair_chunked_punctuation(body))
                })
            })
            .or_else(|| {
                chunks().find_map(|chunk| {
                    let (heading, body) = chunk.split_once('\n')?;
                    (heading.trim() == "Caption:" && !body.trim().is_empty())
                        .then(|| std::borrow::Cow::Borrowed(body.trim()))
                })
            })
            .or_else(|| {
                chunks()
                    .find(|chunk| {
                        let first = chunk.lines().next().unwrap_or_default();
                        !first.is_empty()
                            && first.trim_matches(['[', ']', ' '])
                                != "image description — model-derived"
                            && first.trim() != "Caption:"
                    })
                    .map(std::borrow::Cow::Borrowed)
            })?
    } else {
        std::borrow::Cow::Borrowed(chunks().find(|chunk| !chunk.is_empty())?)
    };
    Some(text.chars().take(MAX_SNIPPET_CHARS).collect())
}

fn matches_find_filters(
    request: &ValidatedSearch,
    record: Option<&Map<String, Value>>,
    category: AdmittedCategory,
) -> bool {
    use crate::tools::activities::{activity_kind, activity_people_across};
    if request.activity.as_deref().is_some_and(|wanted| {
        // A mixed-kind find leaves selected original material in the answer.
        // With no other kind selected the activity predicate is exclusive.
        if category != AdmittedCategory::Facets {
            return request
                .content_kinds
                .as_ref()
                .is_none_or(|kinds| !kinds.contains(&category));
        }
        !record
            .and_then(activity_kind)
            .is_some_and(|kind| kind.eq_ignore_ascii_case(wanted))
    }) {
        return false;
    }
    if request.person.as_deref().is_some_and(|name| {
        let name = fold_literal(name);
        !record
            .map(|record| activity_people_across(std::iter::once(record)))
            .is_some_and(|people| {
                people
                    .participants
                    .iter()
                    .chain(&people.heard)
                    .chain(&people.mentioned)
                    .chain(&people.on_screen)
                    .any(|person| fold_literal(person).contains(&name))
            })
    }) {
        return false;
    }
    true
}

/// Whether this search would have found body data had the connection held the
/// body permission, within the same query, days, facets and kinds.
fn search_withheld_body(
    journal_root: &Path,
    snapshot: &ConnectionReadSnapshot,
    boundary: &ConnectionBoundary,
    resolution: &EntityResolutionOutcome,
    request: &ValidatedSearch,
) -> Result<bool, DispatchError> {
    if crate::body_source::holds_body(snapshot)
        || request.category.is_some_and(|kind| {
            !matches!(
                kind,
                AdmittedCategory::Transcripts | AdmittedCategory::Facets | AdmittedCategory::Body
            )
        })
    {
        return Ok(false);
    }
    let mut with_body = snapshot.clone();
    with_body.categories.insert(AdmittedCategory::Body);
    let mut categories = boundary.categories().clone();
    categories.insert(AdmittedCategory::Body);
    let probe = ConnectionBoundary::from_category_tokens(
        categories.iter().map(category_token),
        boundary.scope().clone(),
    )
    .expect("closed admitted categories construct a boundary");
    solstone_core_indexer_query::connection_search_withheld_body(
        journal_root,
        &probe,
        &ConnectionSearchRequest {
            query: resolution.effective_query.clone(),
            limit: 1,
            day: request.day.clone(),
            day_from: request.day_from.clone(),
            day_to: request.day_to.clone(),
            relax: true,
            ..ConnectionSearchRequest::default()
        },
        Timestamp::now()
            .to_zoned_datetime(solstone_core_journal_config::owner_zone(journal_root))
            .date(),
        crate::body_source::body_entry_admission(
            journal_root,
            &with_body,
            crate::body_source::ActivityDayFiles::default(),
        ),
    )
    .map_err(index_error)
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
    // A cursor names its anchor by row id; a re-index since moves it. The
    // fingerprint below still decides whether it is the same text.
    let row_id = current_row_id(
        journal_root,
        boundary,
        &cursor.path,
        cursor.idx,
        cursor.row_id,
    )?
    .ok_or(DispatchError::Tool(ToolError::ReferenceNotFound))?
    .0;
    let entry = read_indexed_entry(
        journal_root,
        QueryBoundary::Connection(boundary.clone()),
        &cursor.path,
        cursor.idx,
        row_id,
        MAX_INDEXED_ENTRY_BYTES,
    )
    .map_err(index_error)?;
    let Some(content) = entry.content().map(str::to_owned) else {
        return Err(DispatchError::Tool(ToolError::ReferenceNotFound));
    };
    match activity_cursor_proof_matches(
        journal_root,
        boundary,
        &cursor.path,
        cursor.idx,
        row_id,
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

/// The row that holds `path` at `idx` for this boundary now, and whether it
/// differs from the one the search named because the file was indexed again.
/// `None` when neither holds it.
fn current_row_id(
    journal_root: &Path,
    boundary: &ConnectionBoundary,
    path: &str,
    idx: i64,
    row_id: i64,
) -> Result<Option<(i64, bool)>, DispatchError> {
    Ok(
        match locate_indexed_entry(
            journal_root,
            QueryBoundary::Connection(boundary.clone()),
            path,
            idx,
            row_id,
        )
        .map_err(index_error)?
        {
            EntryLocation::AsSearched => Some((row_id, false)),
            EntryLocation::Reindexed(row_id) => Some((row_id, true)),
            EntryLocation::Gone => None,
        },
    )
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

fn location_file_day(path: &str) -> Option<&str> {
    path.strip_prefix("chronicle/")
        .unwrap_or(path)
        .strip_suffix("/location.md")
        .filter(|day| solstone_core_journal_io::is_day_key(day))
}

/// Current projection of a day the location writer has built. Older
/// standalone documents still have to match their contained indexed bytes.
fn current_location_document(journal: &Path, day: &str) -> Option<Option<String>> {
    solstone_core_location::projection::read_build_record(journal, day)?;
    Some(crate::tools::week::location_document(journal, day))
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
    // An activity's name: read as this connection's own list shows it now.
    if crate::names::is_name(&request.reference) {
        let Some(address) = crate::names::parse_activity(&request.reference) else {
            return Err(DispatchError::Tool(ToolError::ReferenceNotFound));
        };
        return match fetch_activity(journal_root, principal, snapshot, &address) {
            Err(DispatchError::Tool(ToolError::ReferenceNotFound))
                if crate::names::is_segment_name_shape(&request.reference) =>
            {
                Err(DispatchError::Tool(ToolError::UseGetTranscript))
            }
            result => result,
        };
    }
    let target = match codec().resolve_entry_or_activity(
        &request.reference,
        principal.connection,
        snapshot.generation,
    ) {
        Ok(target) => target,
        Err(_)
            if codec()
                .resolve_segment_or_span(
                    &request.reference,
                    principal.connection,
                    snapshot.generation,
                )
                .is_ok() =>
        {
            return Err(DispatchError::Tool(ToolError::UseGetTranscript));
        }
        Err(_) => return Err(DispatchError::Tool(ToolError::ReferenceNotFound)),
    };
    match target {
        ReferenceTarget::Entry(entry) => {
            let is_screen_text =
                solstone_core_format::content::screen_text::is_screen_text_path(&entry.path);
            let boundary = boundary(snapshot, None, None);
            if !live_allows(journal_root, &boundary, &entry.path, Some(&entry.stream)) {
                return Err(DispatchError::Tool(ToolError::ReferenceNotFound));
            }
            let topic = if topic_segment(&entry.path).is_some() {
                Some(
                    TopicDocument::read(journal_root, &entry.path)
                        .filter(|doc| boundary.allows_topic(doc))
                        .ok_or(DispatchError::Tool(ToolError::ReferenceNotFound))?,
                )
            } else {
                None
            };
            let possible_names = ConnectionPossibleNames::new(journal_root, snapshot);
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
                    "content_kind": AdmittedCategory::ScreenText.as_str(),
                });
                if let Some(stream) = indexed_hit_stream(&entry.stream, &entry.path, None) {
                    value["source"] = json!(source_label(&source_labels(journal_root), stream));
                }
                // The text keeps its bytes, and its headings count from here: when
                // its recording began, on the recording device's clock.
                if let Some(start) =
                    solstone_core_format::content::screen_text::screen_text_segment(&entry.path)
                        .and_then(|segment| {
                            solstone_core_format::content::screen_text::screen_text_clock(
                                segment, 0,
                            )
                        })
                {
                    value["segment_start"] = Value::String(start);
                }
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
                                markdown_fingerprint: None,
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
                // Screen text is written literal when it is recorded.
                return Ok(Prepared::new(value, 1, vec![target]).served_as_is(&["text"]));
            }
            // The file may have been indexed again since the search; read the
            // row that holds this path and chunk index now, and say so.
            let (row_id, reindexed) = current_row_id(
                journal_root,
                &boundary,
                &entry.path,
                entry.idx,
                entry.row_id,
            )?
            .unwrap_or((entry.row_id, false));
            let (text, hidden, record) = match admit_activity(
                journal_root,
                &boundary,
                &entry.path,
                entry.idx,
                row_id,
                None,
                entry.activity_proof.as_deref(),
                true,
                entry.record_identity.as_deref(),
                MAX_INDEXED_ENTRY_BYTES,
            )
            .map_err(index_error)?
            {
                // An activity file's record passes the same test the lists
                // make: hidden, superseded, a Strava line this connection no
                // longer reads and a body record without the body permission
                // are not found, as a missing one isn't.
                ActivityAdmit::Admitted(a)
                    if a.hidden
                        || crate::body_source::activity_file_place(&entry.path).is_some_and(
                            |(facet, day)| {
                                crate::may_see::ActivityDay::read(
                                    journal_root,
                                    snapshot,
                                    &facet,
                                    &day,
                                )
                                .is_none_or(|judged| {
                                    judged.shows(&a.record) != crate::may_see::Shown::Yes
                                })
                            },
                        ) =>
                {
                    return Err(DispatchError::Tool(ToolError::ReferenceNotFound));
                }
                // Never the file's row: a body record is served composed, for
                // this connection.
                ActivityAdmit::Admitted(a) if solstone_core_facets::is_body_record(&a.record) => {
                    let (facet, day) = crate::body_source::activity_file_place(&entry.path)
                        .ok_or(DispatchError::Tool(ToolError::ReferenceNotFound))?;
                    return fetch_body_face(
                        journal_root,
                        snapshot,
                        &facet,
                        &day,
                        &a.record_identity,
                        &entry.path,
                        &a.record,
                    );
                }
                ActivityAdmit::Admitted(a) => {
                    let cleaned =
                        crate::tools::activities::clean_activity_record_for_wire(a.record.clone());
                    let cleaned_text = literal_record_text(cleaned)?;
                    // Links resolve from the admitted record. Cleaning drops
                    // `stream`, and the resolver would then cross streams.
                    (cleaned_text, Some(a.hidden), Some(a.record))
                }
                // A single chunk still names the whole document after a
                // reindex. In a split document the numbered position can
                // move, so the reference must still match that chunk.
                ActivityAdmit::NotActivity(content)
                    if entry
                        .markdown_fingerprint
                        .as_ref()
                        .is_some_and(|fingerprint| *fingerprint != digest(&content))
                        && (entry.idx != 0
                            || indexed_path_has_later_chunk(
                                journal_root,
                                &boundary,
                                &entry.path,
                            )
                            .map_err(index_error)?) =>
                {
                    return Err(DispatchError::Tool(ToolError::ReferenceNotFound));
                }
                // A possible name is re-checked for this connection now, as search did.
                ActivityAdmit::NotActivity(content) if is_possible_names_path(&entry.path) => {
                    let live = possible_names
                        .indexed(&entry.path, entry.idx, &content)
                        .ok_or(DispatchError::Tool(ToolError::ReferenceNotFound))?;
                    (crate::possible_names::matched_text(&live), None, None)
                }
                ActivityAdmit::NotActivity(content) if location_file_day(&entry.path).is_some() => {
                    let day = location_file_day(&entry.path).expect("location document day");
                    let text =
                        verified_markdown_document(journal_root, &entry.path, entry.idx, &content)
                            .filter(|text| {
                                current_location_document(journal_root, day)
                                    .is_none_or(|live| live.as_ref() == Some(text))
                            })
                            .ok_or(DispatchError::Tool(ToolError::ReferenceNotFound))?;
                    (text, None, None)
                }
                ActivityAdmit::NotActivity(content) if topic.is_some() => {
                    let doc = topic
                        .as_ref()
                        .filter(|doc| doc.matches_chunk(&entry.path, entry.idx, &content))
                        .ok_or(DispatchError::Tool(ToolError::ReferenceNotFound))?;
                    (doc.text.clone(), None, None)
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
            let mut checker = LiveTranscriptPages::new(possible_names, snapshot);
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

            if !allowed
                || topic.as_ref().is_some_and(|doc| {
                    TopicDocument::read(journal_root, &entry.path).as_ref() != Some(doc)
                })
            {
                return Err(DispatchError::Tool(ToolError::ReferenceNotFound));
            }

            // An activity record arrives as literal JSON text; any other text
            // is made literal at release.
            let served_as_is: &'static [&'static str] =
                if record.is_some() { &["text"] } else { &[] };
            let photo = crate::photo_picture::is_photo_description(&entry.path);
            let decoded = photo
                .then(|| crate::photo_picture::decode_model_text(&text))
                .flatten();
            let mut value = json!({
                "title": entry_title(&entry.day),
                "date": entry.day,
                "text": decoded.as_deref().unwrap_or(&text),
            });
            if photo {
                let clock = crate::photo_picture::photo_clock_info(journal_root, &entry.path);
                if let Some(time) = clock.time {
                    value["time"] = Value::String(time);
                }
                if let Some(time_source) = clock.time_source {
                    value["time_source"] = Value::String(time_source.to_owned());
                }
            }
            if let Some(doc) = &topic {
                insert_topic_kinds(&mut value, doc);
            } else {
                insert_indexed_kind(
                    &mut value,
                    &entry.path,
                    indexed_entry_kind(journal_root, &entry.path, record.as_ref()),
                );
            }
            if let Some(stream) = indexed_hit_stream(&entry.stream, &entry.path, record.as_ref()) {
                value["source"] = json!(source_label(&source_labels(journal_root), stream));
            }
            if let Some(hidden) = hidden {
                value["hidden"] = Value::Bool(hidden);
            }
            if reindexed {
                value["reindexed_since_search"] = Value::Bool(true);
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
            // A photo's description arrives with the photo itself: the photos
            // kind that admitted the description admits its picture.
            let picture_size = request.size.unwrap_or_default();
            let mut picture = None;
            if photo {
                let (state, kept) =
                    crate::photo_picture::photo_picture(journal_root, &entry.path, picture_size);
                value["picture"] = state;
                picture = kept.map(|kept| kept.jpeg);
            }
            if let Some(folder) = crate::screen_picture::import_folder(&entry.path) {
                let (info, image) = if snapshot.categories.contains(&AdmittedCategory::Screenshots)
                {
                    crate::screen_picture::imported(journal_root, folder, picture_size)
                } else {
                    (
                        json!({"state":"not_allowed","message":kind_refusal(AdmittedCategory::Screenshots)}),
                        None,
                    )
                };
                if !live_allows(journal_root, &boundary, &entry.path, Some(&entry.stream))
                    || !crate::source_release::check_indexed_source_release(
                        journal_root,
                        &entry.path,
                    )
                {
                    return Err(DispatchError::Tool(ToolError::ReferenceNotFound));
                }
                value["picture"] = info;
                picture = image;
            }
            let target = format!("{}#{}", entry.path, entry.idx);
            let mut prepared = Prepared::new(value, 1, vec![target]).served_as_is(served_as_is);
            prepared.images.extend(picture);
            Ok(prepared)
        }
        ReferenceTarget::Activity(act_ref) => {
            fetch_activity(journal_root, principal, snapshot, &act_ref.into())
        }
        _ => Err(DispatchError::Tool(ToolError::ReferenceNotFound)),
    }
}

/// An activity fetched by reference: its record as this connection's own
/// list shows it now.
pub(crate) fn fetch_activity(
    journal_root: &Path,
    principal: DispatchPrincipal<'_>,
    snapshot: &ConnectionReadSnapshot,
    address: &crate::references::ActivityAddress,
) -> Result<Prepared, DispatchError> {
    // Hidden, superseded, out of scope and gone all read as not found.
    let seen = crate::may_see::activity_at(
        journal_root,
        snapshot,
        &address.facet,
        &address.day,
        &address.record_identity,
        address.file_digest.as_deref(),
    )
    .ok_or(DispatchError::Tool(ToolError::ReferenceNotFound))?;
    let place = seen.place;
    let record = match seen.record {
        crate::may_see::SeenRecord::Body(body) => {
            return fetch_body_face(
                journal_root,
                snapshot,
                &place.facet_dir,
                &place.day,
                &address.record_identity,
                &place.rel_path,
                &body,
            );
        }
        crate::may_see::SeenRecord::Activity(record) => record,
    };
    let cleaned = crate::tools::activities::clean_activity_record_for_wire(record.clone());
    let text = literal_record_text(cleaned)?;

    #[cfg(feature = "test-hooks")]
    crate::source_freeze_barrier::checkpoint(crate::source_freeze_barrier::FreezePoint::Before);
    let allowed =
        crate::source_release::check_indexed_source_release(journal_root, &place.rel_path);
    let mut subjects = [LinkSubject {
        path: &place.rel_path,
        idx: 0,
        text: &text,
        segments: Vec::new(),
        more_segments: false,
        activity_record: Some(&record),
    }];
    let mut checker = LiveTranscriptPages::new(
        ConnectionPossibleNames::new(journal_root, snapshot),
        snapshot,
    );
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
    crate::source_freeze_barrier::checkpoint(crate::source_freeze_barrier::FreezePoint::After);

    if !allowed {
        return Err(DispatchError::Tool(ToolError::ReferenceNotFound));
    }

    let activity_content_kind = if solstone_core_format::content::is_strava_reading(&record) {
        AdmittedCategory::Body
    } else {
        AdmittedCategory::Facets
    };
    let mut value = json!({
        "title": entry_title(&place.day),
        "date": place.day,
        "text": text,
        "content_kind": activity_content_kind.as_str(),
    });
    if let Some(stream) = record
        .get("stream")
        .and_then(Value::as_str)
        .filter(|stream| !stream.is_empty())
    {
        value["source"] = json!(source_label(&source_labels(journal_root), stream));
    }
    if !subjects[0].segments.is_empty() {
        value["segments"] = Value::Array(subjects[0].segments.clone());
    }
    if subjects[0].more_segments {
        value["more_segments"] = Value::Bool(true);
    }
    let target = format!("{}#{}", place.rel_path, address.record_identity);
    Ok(Prepared::new(value, 1, vec![target]).served_as_is(&["text"]))
}

/// A body record fetched by reference: the group it is in, as this connection
/// may read it. Any member's reference, a former anchor's included, reaches the
/// group. A record whose window this connection can't read is not found.
fn fetch_body_face(
    journal_root: &Path,
    snapshot: &ConnectionReadSnapshot,
    facet_dir: &str,
    day: &str,
    record_identity: &str,
    rel_path: &str,
    record: &Map<String, Value>,
) -> Result<Prepared, DispatchError> {
    let face = body_face_for(journal_root, snapshot, facet_dir, day, record)?;
    // A hidden group is not found, as a missing one isn't.
    if face.hidden {
        return Err(DispatchError::Tool(ToolError::ReferenceNotFound));
    }
    // A source app names itself, so every value is a third party's text: served
    // literal, as a capture record is.
    let Value::Object(item) = solstone_core_location::body_records::face_json(&face) else {
        return Err(DispatchError::Tool(ToolError::ReferenceNotFound));
    };
    let text = literal_record_text(item)?;
    let value = json!({
        "title": entry_title(&face.anchor.day),
        "date": face.anchor.day,
        "text": text,
        "content_kind": AdmittedCategory::Body.as_str(),
    });
    let mut targets = vec![format!("{rel_path}#{record_identity}")];
    crate::body_source::name_windows(&mut targets, crate::body_source::face_windows(&face));
    Ok(Prepared::new(value, 1, targets).served_as_is(&["text"]))
}

/// The face a body record is shown under for this connection.
pub(crate) fn body_face_for(
    journal_root: &Path,
    snapshot: &ConnectionReadSnapshot,
    facet_dir: &str,
    day: &str,
    record: &Map<String, Value>,
) -> Result<solstone_core_location::body_records::Face, DispatchError> {
    // A body record and everything on it is body data.
    if !crate::body_source::holds_body(snapshot) {
        return Err(DispatchError::Tool(ToolError::ReferenceNotFound));
    }
    let id = record.get("id").and_then(Value::as_str).unwrap_or_default();
    let gate = crate::body_source::window_gate(journal_root, snapshot);
    // Every day the record's group can show under, grouped once.
    let anchor_day = day.to_owned();
    let days = solstone_core_location::body_records::days_after(day);
    let from = days
        .iter()
        .min()
        .map_or(&anchor_day, |day| day.min(&anchor_day));
    let to = days
        .iter()
        .max()
        .map_or(&anchor_day, |day| day.max(&anchor_day));
    solstone_core_location::body_records::body_faces_range(
        journal_root,
        facet_dir,
        from,
        to,
        Some(&gate),
        true,
    )
    .map_err(|_| DispatchError::Tool(ToolError::ReferenceNotFound))?
    .into_values()
    .flatten()
    .find(|face| face.members.iter().any(|member| member.id == id))
    .ok_or(DispatchError::Tool(ToolError::ReferenceNotFound))
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
    json!({"kind": origin.kind, "creation_label": origin.creation_label, "created_at": solstone_core_time::utc_iso(origin.created_at)})
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
    let identities = solstone_core_facets::read_identity_map(journal_root)
        .map_err(|_| DispatchError::Tool(ToolError::FileUnreadable))?;
    read_scoped_entities_resolved(journal_root, facet_dir, &identities)
}

/// [`read_scoped_entities`] against one request's read of the identity map.
pub(crate) fn read_scoped_entities_resolved(
    journal_root: &Path,
    facet_dir: &str,
    identities: &solstone_core_facets::EntityIdentityMap,
) -> Result<Vec<solstone_core_facets::ScopedFacetEntity>, DispatchError> {
    let mut entities = solstone_core_facets::list_scoped_facet_entities_with_resolved_ids(
        journal_root,
        facet_dir,
        false,
        false,
        &identities.resolved,
    )
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
    // Read once a facet needs it: it reads every entity in the journal.
    let mut identities = None;
    // Every facet in the effective filter must be read before paging so a malformed relationship fails closed.
    let mut all_entities = Vec::new();
    // Each facet's folder, which an entity's name is written with.
    let folders: BTreeMap<String, String> = facets
        .iter()
        .map(|(id, dir, _)| (id.clone(), dir.clone()))
        .collect();
    for (facet_id, facet_dir, _) in facets {
        if request.facet_id.as_deref().is_some_and(|id| id != facet_id) {
            continue;
        }
        if identities.is_none() {
            identities = Some(
                solstone_core_facets::read_identity_map(journal_root)
                    .map_err(|_| DispatchError::Tool(ToolError::FileUnreadable))?,
            );
        }
        let identities = identities.as_ref().expect("read above");
        let scoped = read_scoped_entities_resolved(journal_root, &facet_dir, identities)?;
        for item in scoped {
            all_entities.push((facet_id.clone(), facet_dir.clone(), item));
        }
    }

    all_entities.sort_by(|(f_a, _, item_a), (f_b, _, item_b)| {
        f_a.cmp(f_b)
            .then_with(|| item_a.entity_id.cmp(&item_b.entity_id))
    });

    if let Some(cursor) = cursor.as_ref() {
        all_entities.retain(|(facet_id, _, item)| {
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
        .map(|(facet_id, _, item)| (facet_id.clone(), item.entity_id.clone()));

    let mut targets = Vec::new();
    let mut entities = Vec::new();
    for (facet_id, facet_dir, item) in page {
        let reference = entity_reference(
            principal,
            snapshot,
            folders.get(&facet_id).map(String::as_str),
            EntityReference {
                facet_id: facet_id.clone(),
                entity_id: item.entity_id.clone(),
            },
        )?;
        targets.push(format!("facet:{facet_id}/entity:{}", item.entity_id));
        // An entity in two facets is listed once for each; the facet tells the rows apart.
        let mut row = json!({
            "name": entity_name(&item.identity),
            "facet": facet_id,
            "reference": reference,
            "has_photo": snapshot.categories.contains(&AdmittedCategory::ProfilePics)
                && solstone_core_entities::photos::has_photo_in_facet(journal_root, &item.entity_dir, &facet_dir)
                    .map_err(|_| DispatchError::Tool(ToolError::FileUnreadable))?,
        });
        crate::journal_links::attach(
            journal_root,
            &mut row,
            solstone_core_journal_link::Place::Person {
                id: item.entity_id.clone(),
            },
        );
        entities.push(row);
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
    if crate::names::is_name(&request.reference) {
        let (entity_id, facet) = crate::names::parse_entity(&request.reference)
            .ok_or(DispatchError::Tool(ToolError::ReferenceNotFound))?;
        return read_entity(journal_root, snapshot, &facet, &entity_id);
    }
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
    read_entity(
        journal_root,
        snapshot,
        &reference.facet_id,
        &reference.entity_id,
    )
}

/// One entity as one facet holds it, when this connection's own list of that
/// facet's entities shows it now. `facet` is the facet's id or its folder,
/// matched against the facets this connection reaches.
pub(crate) fn read_entity(
    journal_root: &Path,
    snapshot: &ConnectionReadSnapshot,
    facet: &str,
    entity_id: &str,
) -> Result<Prepared, DispatchError> {
    if !snapshot.categories.contains(&AdmittedCategory::Entities) {
        return Err(DispatchError::Tool(ToolError::ReferenceNotFound));
    }
    let Some((facet_id, facet_dir)) = crate::may_see::reached_facet(journal_root, snapshot, facet)
    else {
        return Err(DispatchError::Tool(ToolError::ReferenceNotFound));
    };
    let item = read_scoped_entities(journal_root, &facet_dir)?
        .into_iter()
        .find(|item| item.entity_id == entity_id)
        .ok_or(DispatchError::Tool(ToolError::ReferenceNotFound))?;
    let mut targets = vec![format!("facet:{facet_id}/entity:{entity_id}")];
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
        &boundary(snapshot, Some(AdmittedCategory::Entities), Some(&facet_id)),
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
    let mut image = None;
    // The connection's profile pics option, then the owner's switch, read the
    // one way every face reader reads it.
    if snapshot.categories.contains(&AdmittedCategory::ProfilePics)
        && solstone_core_entities::photos::enabled(journal_root)
            .map_err(|_| DispatchError::Tool(ToolError::FileUnreadable))?
    {
        let photo =
            solstone_core_entities::photos::photo(journal_root, entity_id, Some(&facet_dir))
                .map_err(|_| DispatchError::Tool(ToolError::FileUnreadable))?;
        value["photo"] = json!({"state": photo.state});
        image = photo.jpeg.map(|bytes| bounded_photo(&bytes)).transpose()?;
    }
    let mut prepared = Prepared::new(value, 1, targets);
    prepared.images.extend(image);
    Ok(prepared)
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
                || cursor.day_from != request.day_from
                || cursor.day_to != request.day_to
                || cursor.time_from != request.time_from
                || cursor.time_to != request.time_to
                || cursor.facet != request.facet_id
                || cursor.source != request.source
                || cursor.kind != request.kind
            {
                return Err(DispatchError::Tool(ToolError::ReferenceNotFound));
            }
            Ok(cursor)
        })
        .transpose()?;
    // A body source's label lists that source's windows, its own way. Any
    // other label a list shows narrows this list to the segments it names.
    let mut source_filter: Option<crate::segment_span::SourceFilter> = None;
    if let Some(source) = request.source.as_deref() {
        if crate::body_source::is_body_source(journal_root, source) {
            return crate::body_source::list_source(
                journal_root,
                principal,
                snapshot,
                &request,
                source,
                cursor.as_ref(),
            );
        }
        let filter = crate::segment_span::SourceFilter::new(journal_root, source);
        // A label that names nothing here is said so, with nothing claimed
        // about the day.
        if let Some(day) = request.day.as_deref()
            && !crate::segment_span::day_holds_readable(journal_root, snapshot, day, &filter)?
        {
            return Ok(Prepared::new(json!({ "transcripts": [] }), 0, Vec::new())
                .when_empty(crate::body_source::NOTHING_READABLE));
        }
        // A kind belongs to a body source's entries. Said only of a source
        // this connection already reads.
        if request.kind.is_some() {
            return Err(DispatchError::Tool(ToolError::SourceArguments));
        }
        source_filter = Some(filter);
    }

    // A segment is read as what it is: a body window under body, any other
    // under transcripts. One of them may be all this connection holds.
    let segments_boundary = segment_boundary(snapshot, request.facet_id.as_deref());
    // Body windows and workouts are counted under body alone. Without it, what
    // would have been counted is said to need it instead.
    let body_boundary = boundary(
        snapshot,
        Some(AdmittedCategory::Body),
        request.facet_id.as_deref(),
    );
    let holds_body = crate::body_source::holds_body(snapshot);
    let mut body_withheld = false;
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
        let mut keys: Vec<String> = day_map
            .into_keys()
            .filter(|day| {
                request.day_from.as_ref().is_none_or(|from| day >= from)
                    && request.day_to.as_ref().is_none_or(|to| day <= to)
            })
            .collect();
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

    let sources = source_labels(journal_root);
    let mut owner_links = crate::journal_links::Recordings::default();
    let mut segments = Vec::new();
    let mut targets = Vec::new();
    let mut unreadable = 0;
    let mut last_examined: Option<(String, String, String)> = None;
    let mut has_more = false;
    let mut omitted_body = false;
    // Each workout counts once, by its id: a workout has a tile every five
    // minutes of its span.
    let mut omitted_workouts = std::collections::BTreeSet::new();
    // What the owner hid of their body data, read once for the list.
    let hides = crate::body_source::BodyHides::read(journal_root);
    let declarations = FacetDeclarationSet::from_journal(journal_root).ok();

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
            if source_filter
                .as_ref()
                .is_some_and(|source| !source.admits(&stream))
            {
                continue;
            }
            // Calendar notes are not something the owner did; they are found
            // through search, not listed as a moment of the day.
            if solstone_core_segment::is_calendar_stream(journal_root, &stream) {
                continue;
            }
            // Body windows are reached by naming their source: hundreds a day would
            // push what the owner said off the page.
            if solstone_core_segment::is_body_only_segment(&segment) {
                // What the owner hid isn't counted: a hidden day's windows and
                // workouts, and what a night or workout that started on one
                // left on a later day.
                if hides.shown(journal_root, day, &stream, &segment).is_none() {
                    continue;
                }
                // A Strava workout is body data too, but no source lists it:
                // the list counts it and says where it is found.
                if crate::body_source::is_left_out_workout(&segment)
                    && live_allows(
                        journal_root,
                        &body_boundary,
                        &segment_assignment_probe_path(day, &stream, &name),
                        Some(&stream),
                    )
                    && crate::source_release::check_segment_transcript_release(
                        journal_root,
                        day,
                        &stream,
                        &name,
                    )
                {
                    if holds_body {
                        omitted_workouts.insert(crate::body_source::workout_id(&segment));
                    } else {
                        body_withheld = true;
                    }
                }
                let window = segment
                    .path()
                    .join(solstone_core_format::content::body_records::BODY_RECORDS_FILE)
                    .is_file()
                    && !crate::body_source::holds_only_moved(segment.path())
                    && live_allows(
                        journal_root,
                        &body_boundary,
                        &segment_assignment_probe_path(day, &stream, &name),
                        Some(&stream),
                    );
                omitted_body = omitted_body || window && holds_body;
                body_withheld = body_withheld || window && !holds_body;
                continue;
            }
            let clock = segment_clock(&name);
            if let Some((from_sec, to_sec)) = time_filter {
                let Some(ref clock) = clock else {
                    continue;
                };
                if !clock.overlaps(from_sec, to_sec) {
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
        let possible_names = ConnectionPossibleNames::new(journal_root, snapshot);
        for (segment, stream, name, clock) in matching_segments {
            let start_seconds = clock.as_ref().map(|c| c.start_seconds).unwrap_or(0);
            if !is_after_cursor(day, start_seconds, &stream, &name, cursor.as_ref()) {
                continue;
            }

            // Scope comes first: a segment this connection may not read neither
            // fills the page nor earns a `next_cursor`. The test is the one a
            // read of this segment makes again.
            let Some(left_out) = segments_boundary.as_ref().and_then(|boundary| {
                crate::may_see::segment_shown_within(
                    journal_root,
                    boundary,
                    declarations.as_ref()?,
                    &hides,
                    day,
                    &stream,
                    &name,
                    &segment,
                )
            }) else {
                continue;
            };

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
            let page = if allowed {
                // Read as get_transcript serves it, so `lines` counts nothing
                // the owner hid.
                Some(
                    solstone_core_transcripts::read_segment_transcript_page_with_permissions(
                        &segment,
                        None,
                        speakers.as_ref(),
                        possible_names.labels(segment.path()).as_ref(),
                        &left_out,
                        Some(&snapshot.categories),
                    ),
                )
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

            let page = match page.expect("allowed reads page") {
                Ok(page) => page,
                Err(solstone_core_transcripts::SegmentTranscriptReadError::NoPermittedFiles) => {
                    continue;
                }
                Err(_) => {
                    unreadable += 1;
                    continue;
                }
            };

            if page.content_kinds.is_empty() && !page.unresolved_conversation {
                continue;
            }

            if segments.len() == request.limit {
                has_more = true;
                break 'outer;
            }
            last_examined = Some((day.clone(), stream.clone(), name.clone()));

            let reference = segment_reference(
                journal_root,
                principal,
                snapshot,
                SegmentReference {
                    day: day.clone(),
                    stream: stream.clone(),
                    segment: name.clone(),
                    version: page.version.fingerprint(),
                    source_index: None,
                    byte_offset: None,
                    statement_ordinal: None,
                    message: None,
                },
            )?;

            targets.push(format!("{day}/{stream}/{name}"));
            let time_str = clock.as_ref().map(|c| c.label.as_str()).unwrap_or("");
            let mut seg_val = json!({
                "title": segment_title(&page.content_kinds, page.unresolved_conversation, day),
                "date": day,
                "time": time_str,
                "source": source_label(&sources, &stream),
                "content_kinds": page.content_kinds,
                "reference": reference,
                // How much was said, so a segment can be chosen without reading it.
                "lines": page.entries.len(),
            });
            if page.unresolved_conversation {
                seg_val["conversation_state"] = json!("unresolved");
            }
            owner_links.attach(journal_root, &mut seg_val, day, &stream, &name);
            if let Some(clock) = &clock {
                seg_val["seconds"] = json!(
                    clock
                        .exclusive_end_seconds
                        .saturating_sub(clock.start_seconds)
                );
            }
            if page.next.is_some() {
                seg_val["more_lines"] = Value::Bool(true);
            }
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
                            day_from: request.day_from.clone(),
                            day_to: request.day_to.clone(),
                            time_from: request.time_from.clone(),
                            time_to: request.time_to.clone(),
                            facet: request.facet_id.clone(),
                            source: request.source.clone(),
                            kind: None,
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
    // A day's list can be read in one go, and says what it doesn't cover.
    if let Some(day) = request.day.as_deref() {
        response_val["gaps"] = json!(crate::segment_span::day_gaps(
            journal_root,
            snapshot,
            request.facet_id.as_deref(),
            day,
            request.time_from.as_deref(),
            request.time_to.as_deref(),
            source_filter.as_ref(),
        )?);
        if count > 0 {
            response_val["span_reference"] = json!(crate::segment_span::span_reference(
                principal,
                snapshot,
                day,
                request.time_from.as_deref(),
                request.time_to.as_deref(),
                request.facet_id.as_deref(),
                request.source.as_deref(),
            )?);
        }
    }
    // A list narrowed to one source says nothing of the day's other sources.
    let other_sources = request
        .day
        .as_deref()
        .filter(|_| request.source.is_none())
        .map_or_else(Vec::new, |day| {
            crate::body_source::other_sources(journal_root, snapshot, &hides, day)
        });
    if let Some(note) = request.day.as_deref().and_then(|day| {
        crate::body_source::listing_earlier_import_note(journal_root, snapshot, day)
    }) {
        response_val["earlier_imports_note"] = json!(note);
    }
    if !omitted_workouts.is_empty() {
        response_val["omitted_workouts"] = json!(omitted_workouts.len());
        response_val["workouts_note"] = json!(LIST_TRANSCRIPTS_WORKOUTS);
    }
    let body_withheld = body_withheld
        || request.day.as_deref().is_some_and(|day| {
            crate::body_source::earlier_import_withheld(journal_root, snapshot, day)
        });
    if body_withheld {
        response_val["body_note"] = json!(crate::body_source::BODY_NOT_GRANTED);
    }
    let empty_note = if body_withheld && other_sources.is_empty() {
        crate::body_source::BODY_NOT_GRANTED
    } else if other_sources.is_empty() {
        if omitted_body && request.day.is_none() {
            LIST_TRANSCRIPTS_BODY_ELSEWHERE
        } else if !omitted_workouts.is_empty() {
            LIST_TRANSCRIPTS_WORKOUTS
        } else {
            LIST_TRANSCRIPTS_EMPTY
        }
    } else {
        response_val["other_sources"] = json!(other_sources);
        LIST_TRANSCRIPTS_OTHER_SOURCES
    };

    Ok(Prepared::new(response_val, count, targets).when_empty(empty_note))
}

fn get_transcript(
    journal_root: &Path,
    principal: DispatchPrincipal<'_>,
    snapshot: &ConnectionReadSnapshot,
    request: ValidatedGetTranscript,
) -> Result<Prepared, DispatchError> {
    let moment = request.offset_ms.map(|ms| (ms, request.display.as_deref()));
    // A segment's name: found among the day's segments as the journal lists
    // them, then read as any segment is.
    if crate::names::is_name(&request.reference) {
        let address = crate::names::resolve_segment(journal_root, &request.reference)
            .ok_or(DispatchError::Tool(ToolError::ReferenceNotFound))?;
        return read_segment(
            journal_root,
            principal,
            snapshot,
            &address,
            request.cursor.as_deref(),
            moment,
        );
    }
    let reference = match codec()
        .resolve_segment_or_span(
            &request.reference,
            principal.connection,
            snapshot.generation,
        )
        .map_err(|_| DispatchError::Tool(ToolError::ReferenceNotFound))?
    {
        ReferenceTarget::Segment(reference) => reference,
        ReferenceTarget::SourceSpan(_) | ReferenceTarget::SegmentSpan(_)
            if request.offset_ms.is_some() =>
        {
            return Err(DispatchError::InvalidInput);
        }
        ReferenceTarget::SourceSpan(span) => {
            return crate::body_source::read_source_span(
                journal_root,
                principal,
                snapshot,
                &request,
                span,
            );
        }
        ReferenceTarget::SegmentSpan(span) => {
            return crate::segment_span::read_segment_span(
                journal_root,
                principal,
                snapshot,
                &request,
                span,
            );
        }
        _ => return Err(DispatchError::Tool(ToolError::ReferenceNotFound)),
    };
    read_segment(
        journal_root,
        principal,
        snapshot,
        &reference.into(),
        request.cursor.as_deref(),
        moment,
    )
}

/// One segment's transcript, when this connection may read that segment now.
pub(crate) fn read_segment(
    journal_root: &Path,
    principal: DispatchPrincipal<'_>,
    snapshot: &ConnectionReadSnapshot,
    reference: &crate::references::SegmentAddress,
    cursor: Option<&str>,
    moment: Option<(u32, Option<&str>)>,
) -> Result<Prepared, DispatchError> {
    let owner_zone = solstone_core_journal_config::owner_zone(journal_root);
    let segment =
        iter_segments_skipping_unreadable_streams(journal_root, PathOrDay::Day(&reference.day))
            .map_err(|_| DispatchError::Tool(ToolError::ReferenceNotFound))?
            .into_iter()
            .find(|segment| {
                segment
                    .record_identity()
                    .is_ok_and(|id| id.stream == reference.stream && id.name == reference.segment)
            })
            .ok_or(DispatchError::Tool(ToolError::ReferenceNotFound))?;
    // The same test every list and link makes before it shows this segment:
    // out of scope, a calendar note, a workout tile and a body window the
    // owner hid all read as one that isn't there. A later day's window leaves
    // out what a hidden night left in it.
    let left_out = crate::may_see::segment_shown(
        journal_root,
        snapshot,
        &crate::body_source::BodyHides::read(journal_root),
        None,
        &reference.day,
        &reference.stream,
        &reference.segment,
        &segment,
    )
    .ok_or(DispatchError::Tool(ToolError::ReferenceNotFound))?;
    let continuation = cursor
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
                || reference
                    .version
                    .is_some_and(|version| cursor.version != version)
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
                message: cursor.message,
                statement_ordinal: cursor
                    .statement_ordinal
                    .ok_or(DispatchError::Tool(ToolError::ReferenceNotFound))?,
                version: solstone_core_transcripts::SegmentTranscriptVersion::from_fingerprint(
                    cursor.version,
                ),
            })
        })
        .transpose()?;
    // A cursor keeps its pin: the transcript as it read on the page before.
    let pinned = reference.version.or(continuation
        .as_ref()
        .map(|continuation| continuation.version.fingerprint()));
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
        // A possible name is shown beside the words it may refer to, only
        // while it holds and only for an entity this connection may see.
        let possible_names =
            ConnectionPossibleNames::new(journal_root, snapshot).labels(segment.path());
        Some(
            solstone_core_transcripts::read_segment_transcript_page_with_permissions(
                &segment,
                continuation.as_ref(),
                speakers.as_ref(),
                possible_names.as_ref(),
                &left_out,
                Some(&snapshot.categories),
            ),
        )
    } else {
        None
    };
    let screen = if allowed && page_res.as_ref().is_some_and(Result::is_ok) {
        moment
            .map(|(offset, display)| {
                crate::screen_picture::moment(
                    journal_root,
                    segment.path(),
                    reference,
                    offset,
                    display,
                )
            })
            .transpose()?
    } else {
        None
    };
    let allowed = allowed
        && (screen.is_none()
            || crate::may_see::segment_scope(
                journal_root,
                snapshot,
                &reference.day,
                &reference.stream,
                &reference.segment,
            ) == Some(true))
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
    let version = page.version.fingerprint();
    if pinned.is_some_and(|pinned| pinned != version) {
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
                        version,
                        source_index: Some(next.source_index),
                        byte_offset: Some(next.byte_offset),
                        statement_ordinal: Some(next.statement_ordinal),
                        message: next.message,
                    }),
                )
                .map_err(|_| DispatchError::Tool(ToolError::ReferenceNotFound))
        })
        .transpose()?;
    let target = format!(
        "{}/{}/{}",
        reference.day, reference.stream, reference.segment
    );
    let parts = page
        .entries
        .iter()
        .enumerate()
        .filter_map(|(index, entry)| {
            entry
                .turn_part
                .as_ref()
                .map(|part| json!({"entry":index,"part":part}))
        })
        .collect::<Vec<_>>();
    let entries = page
        .entries
        .into_iter()
        .map(|item| solstone_core_format::markdown::literal_text(&item.text))
        .collect::<Vec<_>>();
    let count = entries.len();
    let mut content_kinds = page.content_kinds;
    if screen.is_some() && !content_kinds.contains(&AdmittedCategory::ScreenText.as_str()) {
        content_kinds.push(AdmittedCategory::ScreenText.as_str());
    }
    let mut response_val = json!({
        "entries": entries,
        "source": source_label(&source_labels(journal_root), &reference.stream),
        "content_kinds": content_kinds,
        "next_cursor": next_cursor,
    });
    if !parts.is_empty() {
        response_val["turn_parts"] = json!(parts);
    }
    if page.unresolved_conversation {
        response_val["conversation_state"] = json!("unresolved");
    }
    crate::journal_links::Recordings::default().attach(
        journal_root,
        &mut response_val,
        &reference.day,
        &reference.stream,
        &reference.segment,
    );
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
    // Each transcript entry is made literal after paging.
    let mut prepared = Prepared::new(response_val, count, vec![target]).served_as_is(&["entries"]);
    if let Some((info, image)) = screen {
        prepared.value["picture"] = info;
        prepared.images.extend(image);
        prepared.count = prepared.count.max(1);
        Ok(prepared)
    } else {
        Ok(prepared.when_empty(GET_TRANSCRIPT_EMPTY))
    }
}

pub(crate) fn differing_capture_zone(
    journal_root: &Path,
    owner: &solstone_core_journal_config::TimeZone,
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

/// Stream behind an indexed search hit or fetched entry, if any.
///
/// Entries with no stream behind them (day documents, weekly reflections,
/// import summaries, body faces, streamless activities) return `None` so they
/// carry no `source`.
fn indexed_hit_stream<'a>(
    stored_stream: &'a str,
    path: &'a str,
    activity_record: Option<&'a Map<String, Value>>,
) -> Option<&'a str> {
    if !stored_stream.is_empty() {
        return Some(stored_stream);
    }
    if let Some(record) = activity_record {
        if solstone_core_facets::is_body_record(record) {
            return None;
        }
        return record
            .get("stream")
            .and_then(Value::as_str)
            .filter(|stream| !stream.is_empty());
    }
    let rel = path.strip_prefix("chronicle/").unwrap_or(path);
    let mut parts = rel.split('/');
    let day = parts.next()?;
    let second = parts.next()?;
    if !solstone_core_journal_io::is_day_key(day) {
        return None;
    }
    let third = parts.next()?;
    if !second.is_empty()
        && solstone_core_format::segment::segment_key(third).is_some()
        && parts.next().is_some()
    {
        return Some(second);
    }
    if solstone_core_format::segment::segment_key(second).is_some() {
        return Some(solstone_core_journal_io::paths::DEFAULT_STREAM);
    }
    None
}

/// Older imported Markdown may be admitted without establishing a medium.
fn insert_indexed_kind(value: &mut Value, path: &str, kind: AdmittedCategory) {
    if solstone_core_format::content::conversation::unresolved_markdown(path) {
        value["conversation_state"] = json!("unresolved");
    } else {
        value["content_kind"] = json!(kind.as_str());
    }
}

fn insert_topic_kinds(value: &mut Value, doc: &TopicDocument) {
    let Some(kinds) = &doc.kinds else {
        return;
    };
    if kinds.len() == 1 {
        value["content_kind"] = json!(kinds.first().expect("one kind").as_str());
    } else {
        value["content_kinds"] = json!(kind_words(kinds));
    }
}

/// Content kind of an indexed entry as the registry and permission check settle it.
fn indexed_entry_kind(
    journal_root: &Path,
    path: &str,
    activity_record: Option<&Map<String, Value>>,
) -> AdmittedCategory {
    if activity_record.is_some_and(|record| {
        solstone_core_facets::is_body_record(record)
            || solstone_core_format::content::is_strava_reading(record)
    }) {
        return AdmittedCategory::Body;
    }
    let rel = path.strip_prefix("chronicle/").unwrap_or(path);
    let stored = solstone_core_format::content::resolve_spec(rel)
        .and_then(|spec| match spec.disposition {
            solstone_core_format::content::IndexDisposition::Admitted { category, .. } => {
                Some(category)
            }
            solstone_core_format::content::IndexDisposition::Excluded => None,
        })
        .unwrap_or(AdmittedCategory::Transcripts);
    solstone_core_format::content::kinds::indexed_permission(Some(journal_root), rel, stored)
}

/// Title for a listed segment row: speech keeps `"Transcript segment — {day}"`;
/// segments holding no speech are titled by what they actually hold.
fn segment_title(content_kinds: &[&str], unresolved: bool, day: &str) -> String {
    if content_kinds.contains(&AdmittedCategory::Transcripts.as_str()) {
        return format!("Transcript segment — {day}");
    }
    if content_kinds.is_empty() {
        return if unresolved {
            format!("Conversation — {day}")
        } else {
            format!("Segment — {day}")
        };
    }
    let noun = |kind: &str| match kind {
        "messages" => "messages",
        "notes" => "note",
        "documents" => "document",
        "photos" => "photo",
        "screen_text" => "screen",
        "body" => "body records",
        "calendar" => "calendar event",
        "places" => "places",
        "facets" => "activity",
        "entities" => "entity",
        _ => "segment",
    };
    let mut nouns: Vec<&str> = content_kinds.iter().map(|kind| noun(kind)).collect();
    if unresolved {
        nouns.push("conversation");
    }
    let phrase = match nouns.as_slice() {
        [only] => (*only).to_owned(),
        [first, second] => format!("{first} and {second}"),
        many => many.join(", "),
    };
    let mut chars = phrase.chars();
    let titled = match chars.next() {
        Some(first) => format!("{}{}", first.to_uppercase(), chars.as_str()),
        None => "Segment".to_owned(),
    };
    format!("{titled} — {day}")
}

/// Where each stream's segments came from, in the owner's words. ⛔ Never a
/// stream name. Unified import sources (`imported file`, `Obsidian`, `Evernote`,
/// `PDF`) share one label across their backing streams so narrowing by `source`
/// matches all of them; `content_kinds` and `reference` distinguish same-time
/// segments that share a source.
pub(crate) fn source_labels(journal_root: &Path) -> BTreeMap<String, String> {
    use solstone_core_home::provenance::{agent_stream_labels, load_stream_labels};
    agent_stream_labels(&load_stream_labels(journal_root))
}

pub(crate) fn source_label(labels: &BTreeMap<String, String>, stream: &str) -> String {
    use solstone_core_home::provenance::{ProvenancePart, provenance_label};
    labels
        .get(stream)
        .cloned()
        .unwrap_or_else(|| provenance_label(&ProvenancePart::Unknown, None))
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

/// Screen text is stored literal, so its snippet is cut, never escaped again.
/// A cut never leaves half an escape.
fn screen_text_snippet(value: &str) -> String {
    let mut cut = value.chars().take(MAX_SNIPPET_CHARS).collect::<String>();
    if cut.len() < value.len() {
        if let Some(amp) = cut.rfind('&')
            && !cut[amp..].contains(';')
        {
            cut.truncate(amp);
        }
        let trailing = cut.len() - cut.trim_end_matches('\\').len();
        if trailing % 2 == 1 {
            cut.pop();
        }
    }
    cut
}

fn snippet(value: &str) -> String {
    solstone_core_format::markdown::literal_text(
        &value.chars().take(MAX_SNIPPET_CHARS).collect::<String>(),
    )
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
/// names, coverage — with every sealed token (`reference`, `next_cursor`,
/// `span_reference`, `next_reference`, `continuation`, and journal `link`) and
/// classification label (`content_kind`, `content_kinds`) excluded, and object
/// keys sorted so a later field reordering does not silently invalidate every
/// record already written. ⚠ It is therefore a digest of the **content**, not
/// a fingerprint of the exact bytes on the wire; the targets beside it say
/// which records that content came from.
pub(crate) fn content_digest(value: &Value) -> String {
    digest(&canonical_content(value).to_string())
}

pub(crate) fn canonical_content(value: &Value) -> Value {
    match value {
        Value::Object(fields) => {
            let mut keys = fields
                .iter()
                .map(|(key, _)| key.as_str())
                .filter(|key| {
                    !matches!(
                        *key,
                        "reference"
                            | "activity_reference"
                            | "link"
                            | "next_cursor"
                            | "span_reference"
                            | "next_reference"
                            | "continuation"
                            | "content_kind"
                            | "content_kinds"
                            | "conversation_state"
                    )
                })
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

fn bounded_photo(bytes: &[u8]) -> Result<Vec<u8>, DispatchError> {
    let image = image::load_from_memory_with_format(bytes, image::ImageFormat::Jpeg)
        .map_err(|_| DispatchError::Tool(ToolError::FileUnreadable))?;
    let image = image.thumbnail(256, 256).to_rgb8();
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 85)
        .encode_image(&image)
        .map_err(|_| DispatchError::Tool(ToolError::FileUnreadable))?;
    Ok(jpeg)
}

#[cfg(test)]
mod literal_tests {
    use pulldown_cmark::{Event, Options, Parser, Tag};

    use super::{MAX_SNIPPET_CHARS, content_digest, snippet};

    #[test]
    fn audit_digest_tracks_presentation_and_ignores_randomized_references() {
        let source = "![cover](https://example.test/cover.png)";
        let before = serde_json::json!({"snippet":source, "reference":"first", "link":"one"});
        let after =
            serde_json::json!({"snippet":snippet(source), "reference":"second", "link":"two"});
        let replay =
            serde_json::json!({"snippet":snippet(source), "reference":"third", "link":"three"});
        assert_ne!(content_digest(&before), content_digest(&after));
        assert_eq!(content_digest(&after), content_digest(&replay));
    }

    #[test]
    fn audit_digest_ignores_added_kind_labels_but_tracks_content_and_source() {
        let plain_hit = serde_json::json!({
            "title": "journal entry from 20260407",
            "date": "20260407",
            "snippet": "body summary line",
            "reference": "ref-1"
        });
        let labelled_hit = serde_json::json!({
            "title": "journal entry from 20260407",
            "date": "20260407",
            "snippet": "body summary line",
            "content_kind": "body",
            "reference": "ref-2"
        });
        assert_eq!(content_digest(&plain_hit), content_digest(&labelled_hit));

        let plain_segment = serde_json::json!({
            "entries": ["[10:00:05] hello"],
            "source": "your mac · audio",
            "next_cursor": null
        });
        let labelled_segment = serde_json::json!({
            "entries": ["[10:00:05] hello"],
            "source": "your mac · audio",
            "content_kinds": ["transcripts"],
            "next_cursor": null
        });
        assert_eq!(
            content_digest(&plain_segment),
            content_digest(&labelled_segment)
        );

        let moved_content = serde_json::json!({
            "entries": ["[10:00:05] different words"],
            "source": "your mac · audio",
            "content_kinds": ["transcripts"],
            "next_cursor": null
        });
        assert_ne!(
            content_digest(&labelled_segment),
            content_digest(&moved_content)
        );

        let moved_source = serde_json::json!({
            "entries": ["[10:00:05] hello"],
            "source": "imported file",
            "content_kinds": ["transcripts"],
            "next_cursor": null
        });
        assert_ne!(
            content_digest(&labelled_segment),
            content_digest(&moved_source)
        );
    }

    #[test]
    fn snippets_escape_legacy_markup_after_truncating_source_characters() {
        let payload = "![cover](https://example.test/cover.png) <img src=x> &#33; 日本語\\";
        for text in [
            payload.to_owned(),
            format!(
                "{}![cover](https://example.test/cover.png)",
                "é".repeat(MAX_SNIPPET_CHARS - 12)
            ),
        ] {
            let escaped = snippet(&text);
            let mut visible = String::new();
            for event in Parser::new_ext(&escaped, Options::all()) {
                match event {
                    Event::Start(Tag::Image { .. } | Tag::Link { .. })
                    | Event::Html(_)
                    | Event::InlineHtml(_) => panic!("active snippet markup in {escaped:?}"),
                    Event::Text(text) => visible.push_str(&text),
                    _ => {}
                }
            }
            assert_eq!(
                visible,
                text.chars().take(MAX_SNIPPET_CHARS).collect::<String>()
            );
        }
    }
}
