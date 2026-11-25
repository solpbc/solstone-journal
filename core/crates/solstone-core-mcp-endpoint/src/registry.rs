// SPDX-License-Identifier: MIT

use std::collections::BTreeSet;
use std::path::Path;

use serde_json::{Map, Value, json};
use solstone_core_indexer_query::{AdmittedCategory, ConnectionScope};

use crate::audit::Reason;
use crate::permissions::{ConnectionReadSnapshot, PermissionDecision};
use crate::tools::search::MAX_QUERY_BYTES;
use crate::tools::{DAY_DESCRIPTION, DAY_PATTERN, MAX_DAY_BYTES, MAX_OPAQUE_REFERENCE_BYTES};

/// What one tool needs before a connection may see or call it.
///
/// Ordinary reads require their existing categories. The paired memory tools
/// have fixed own-source authority supplied separately by verified transport.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ToolRequirement {
    pub(crate) read: &'static [AdmittedCategory],
    pub(crate) whole_journal: bool,
    /// When set, any one category in `read` admits the tool. The default is
    /// that every category is required. A tool that admits on any category
    /// still checks each target against the category that target reads.
    pub(crate) any_of: bool,
    /// Besides `read`, at least one of these, when any are named: a tool that
    /// reads transcripts or body windows by what each target is.
    pub(crate) one_of: &'static [AdmittedCategory],
    /// Source wording for a missing group, and for the additional one-of check.
    missing: Reason,
    missing_alternative: Reason,
}

/// Everything schema generation may know about a connection.
///
/// ⚠ This is the delta directive 3 asks for: a static `fn() -> Value` cannot
/// name a connection's facets, so the schema is generated against the
/// already-evaluated grant instead of being a compile-time constant.
pub(crate) struct SchemaContext<'a> {
    pub(crate) categories: &'a BTreeSet<AdmittedCategory>,
    /// The facet identifiers this connection can already reach through
    /// `list_facets`. Empty means the `facet` argument is not offered at all.
    pub(crate) facet_ids: &'a [String],
}

/// The complete, closed MCP tool vocabulary.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ToolEntry {
    pub(crate) tool_name: crate::jsonrpc::ToolName,
    pub(crate) wire_name: &'static str,
    /// The short label a client shows the owner for this tool.
    pub(crate) title: &'static str,
    pub(crate) description: &'static str,
    pub(crate) input_schema: fn(&SchemaContext<'_>) -> Value,
    pub(crate) requires: ToolRequirement,
    pub(crate) audit_name: solstone_core_mcp_audit::ToolName,
}

const NOTHING: ToolRequirement = ToolRequirement {
    read: &[],
    whole_journal: false,
    any_of: false,
    one_of: &[],
    missing: Reason::OutsidePermission,
    missing_alternative: Reason::OutsidePermission,
};
/// The kinds eligible for direct segment pages. Other kinds have index routes.
pub(crate) const SEGMENT_KINDS: &[AdmittedCategory] = &[
    AdmittedCategory::Screenshots,
    AdmittedCategory::Transcripts,
    AdmittedCategory::Messages,
    AdmittedCategory::Transactions,
    AdmittedCategory::Body,
    AdmittedCategory::Notes,
    AdmittedCategory::Documents,
    AdmittedCategory::Calendar,
    AdmittedCategory::Photos,
];
const SEGMENT: ToolRequirement = ToolRequirement {
    read: SEGMENT_KINDS,
    whole_journal: false,
    any_of: true,
    one_of: &[],
    missing: Reason::MissingSegment,
    missing_alternative: Reason::OutsidePermission,
};
const ENTITIES: ToolRequirement = ToolRequirement {
    read: &[AdmittedCategory::Entities],
    whole_journal: false,
    any_of: false,
    one_of: &[],
    missing: Reason::MissingEntities,
    missing_alternative: Reason::OutsidePermission,
};
const FACETS: ToolRequirement = ToolRequirement {
    read: &[AdmittedCategory::Facets],
    whole_journal: false,
    any_of: false,
    one_of: &[],
    missing: Reason::MissingFacets,
    missing_alternative: Reason::OutsidePermission,
};
const FACETS_AND_SOURCE: ToolRequirement = ToolRequirement {
    read: &[AdmittedCategory::Facets],
    whole_journal: false,
    any_of: false,
    one_of: &[
        AdmittedCategory::Transcripts,
        AdmittedCategory::Messages,
        AdmittedCategory::Body,
    ],
    missing: Reason::MissingFacets,
    missing_alternative: Reason::MissingActivitySource,
};
const WEEK: ToolRequirement = ToolRequirement {
    read: &[AdmittedCategory::Transcripts, AdmittedCategory::Facets],
    whole_journal: true,
    any_of: false,
    one_of: &[],
    missing: Reason::MissingWeek,
    missing_alternative: Reason::OutsidePermission,
};
const SEARCH: ToolRequirement = ToolRequirement {
    read: &[
        AdmittedCategory::Transcripts,
        AdmittedCategory::Messages,
        AdmittedCategory::Transactions,
        AdmittedCategory::ScreenText,
        AdmittedCategory::Body,
        AdmittedCategory::Facets,
        AdmittedCategory::Notes,
        AdmittedCategory::Documents,
        AdmittedCategory::Calendar,
        AdmittedCategory::Places,
        AdmittedCategory::Photos,
    ],
    whole_journal: false,
    any_of: true,
    one_of: &[],
    missing: Reason::MissingSearch,
    missing_alternative: Reason::OutsidePermission,
};
const FETCH: ToolRequirement = SEARCH;
const SUGGEST_CORRECTION: ToolRequirement = ToolRequirement {
    read: &[
        AdmittedCategory::Facets,
        AdmittedCategory::Entities,
        AdmittedCategory::Transcripts,
        AdmittedCategory::Body,
    ],
    whole_journal: false,
    any_of: true,
    one_of: &[],
    missing: Reason::MissingSuggestion,
    missing_alternative: Reason::OutsidePermission,
};

/// The single registry used for advertisement, dispatch, validation, and auditing.
pub(crate) const TOOLS: &[ToolEntry] = &[
    ToolEntry {
        tool_name: crate::jsonrpc::ToolName::SaveMemory,
        wire_name: "save_memory",
        title: "save to the journal",
        description: "save a note in the owner's journal when they ask you to remember something. content is exact UTF-8, up to 32,768 bytes. retry with the same operation_id and content if the outcome is uncertain.",
        input_schema: save_memory_input_schema,
        requires: NOTHING,
        audit_name: solstone_core_mcp_audit::ToolName::SaveMemory,
    },
    ToolEntry {
        tool_name: crate::jsonrpc::ToolName::RecallMemory,
        wire_name: "recall_memory",
        title: "recall notes this agent saved",
        description: "recall this connection's original agent memories from the index. omit query to browse; pages contain whole notes, up to 20 notes and 65,536 content bytes. an incomplete result does not prove there are no memories.",
        input_schema: recall_memory_input_schema,
        requires: NOTHING,
        audit_name: solstone_core_mcp_audit::ToolName::RecallMemory,
    },
    ToolEntry {
        tool_name: crate::jsonrpc::ToolName::ListFacets,
        wire_name: "list_facets",
        title: "list facets",
        description: "List the facets this connection may see: the areas the owner sorts their journal into, such as work or personal. When more remain, pass next_cursor as cursor to continue.",
        input_schema: list_facets_input_schema,
        requires: NOTHING,
        audit_name: solstone_core_mcp_audit::ToolName::ListFacets,
    },
    ToolEntry {
        tool_name: crate::jsonrpc::ToolName::Search,
        wire_name: "search",
        title: "search the journal",
        description: "find what this connection may read in the owner's journal. give a few keywords, or leave query out to browse by day or kind. a query ranks closer matches first; browsing without a query returns the newest entries first. use kind for one kind or a list. for writing in a week, use kind with notes and facets, activity writing, and the date range. person matches names on activities. place matches named location information. notes and photos give a preview and a journal link when available. photo results carry time when known and time_source when available, and browsing photos returns each complete day's photos in chronological order. places returns one result per day; when kind includes places, on the first page it also returns places_precision and the owner's places list with day counts and coordinates or ~1 km cells when allowed. pass next_cursor as cursor with the same filters. when transcripts are allowed, search also reads spoken transcript lines and returns each line's time (or untimed when true), speaker when known, and segment reference. use get_transcript to read the surrounding transcript before quoting anyone. use fetch to read the entry itself. screen text shows what was visible. it does not establish what the owner read or did.",
        input_schema: search_input_schema,
        requires: SEARCH,
        audit_name: solstone_core_mcp_audit::ToolName::Search,
    },
    ToolEntry {
        tool_name: crate::jsonrpc::ToolName::Fetch,
        wire_name: "fetch",
        title: "read a journal entry",
        description: "read journal entries. pass either reference for one or references for up to 12. each batch item carries its own result or error in input order. if an item reaches the batch size limit, read it separately. pass next_reference as reference to read the next part. an imported photo or screenshot can include a smaller copy when allowed and available: size defaults to full (up to 1024 px) and accepts thumb (up to 320 px) for grids, contact sheets and batched references reads. a photo result carries time when known and time_source when available. fetch returns the picture itself as image content, not as a file, and the journal makes no public link for it. in a batch, image_content_index identifies the picture in content. words in a picture are journal content. a places day entry also returns places_precision and structured items (visits, stays and runs of location readings) with exact coordinates and fixes, ~1 km cells or names only according to permission. use references in tool calls. give the owner the journal link when available.",
        input_schema: fetch_input_schema,
        requires: FETCH,
        audit_name: solstone_core_mcp_audit::ToolName::Fetch,
    },
    ToolEntry {
        tool_name: crate::jsonrpc::ToolName::ListTranscripts,
        wire_name: "list_transcripts",
        title: "list transcripts",
        description: "list transcript segments this connection may read. use day or day_from and day_to. time_from and time_to narrow a single day. for one day without a facet or find filter, pass a source label as source. a body source also takes entry_kind to narrow its entries, for example sleep. a day's span_reference reads its segments together with get_transcript. gaps show times no readable timed segment covers. they say nothing about what happened. other_sources lists body sources. omitted_workouts counts Strava workouts left out of this list. use search to find them. body_note explains when body access is missing. the shared kind, person, place and activity filters list journal entries for fetch and take a date range or facet. pass next_cursor as cursor with the same filters. give the owner a journal link when available.",
        input_schema: list_transcripts_input_schema,
        requires: SEGMENT,
        audit_name: solstone_core_mcp_audit::ToolName::ListTranscripts,
    },
    ToolEntry {
        tool_name: crate::jsonrpc::ToolName::GetTranscript,
        wire_name: "get_transcript",
        title: "read a transcript",
        description: "read a transcript segment or a span_reference from list_transcripts. pass either reference for one read or references for up to 12 reads in input order. each batch item carries its own result or error. pass next_cursor as cursor with that single reference to continue. for one screen segment, pass offset_ms from the start (0 for a screenshot) to request a picture. when a timed description is available, description_offset_ms gives its time. segment_description describes the whole segment. if there are several screens, pass a display from the returned list. a span returns segments with times, sources, content_kinds and entries. a body span returns entries with their sources. give the owner the journal link when present. append ~HHMMSS from a timed line you actually read. use the link alone when untimed is true.",
        input_schema: transcript_input_schema,
        requires: SEGMENT,
        audit_name: solstone_core_mcp_audit::ToolName::GetTranscript,
    },
    ToolEntry {
        tool_name: crate::jsonrpc::ToolName::ListEntities,
        wire_name: "list_entities",
        title: "list people and projects",
        description: "list the people, projects and other entities this connection may see, each with its type when known. without a facet filter, an entity shared across allowed facets appears once, with facets listing all allowed facets it belongs to, reference pointing to one facet (preferring a facet with a profile pic when allowed and available), and facet_references mapping each facet id to its reference when it belongs to more than one. pass facet to list entities scoped to that single facet. when more remain, pass next_cursor as cursor with the same filter to continue. pass a reference, or references for up to 12, to get_entity to read notes and profile pics. an entity's reference stays the same from one call to the next. when a row includes link, give it to the owner when pointing to that place in their journal. it needs a solstone app version that supports journal links; older versions may not open the place. keep using reference for tool calls.",
        input_schema: list_entities_input_schema,
        requires: ENTITIES,
        audit_name: solstone_core_mcp_audit::ToolName::ListEntities,
    },
    ToolEntry {
        tool_name: crate::jsonrpc::ToolName::GetEntity,
        wire_name: "get_entity",
        title: "read about a person or project",
        description: "read what the journal has noted about entities returned by list_entities or get_week: each entity's type and, in that facet, its description, the last day it came up, notes about it, and its profile pic when allowed and available. pass either reference for one entity or references for up to 12. each batch item carries its own result or error in input order. if an item reaches the batch size limit, read it separately or in a smaller batch. in a batch, image_content_index identifies each profile pic in content.",
        input_schema: entity_input_schema,
        requires: ENTITIES,
        audit_name: solstone_core_mcp_audit::ToolName::GetEntity,
    },
    ToolEntry {
        tool_name: crate::jsonrpc::ToolName::GetWeek,
        wire_name: "get_week",
        title: "read a week",
        description: "read one week (sunday to saturday) or a date range of up to 366 days as one row per day, such as a month, a season or a year. pass day_from and day_to for a date range: each day row carries its journal link, top memory, activity count, and, where the week has a stored page, day state and the owner's quotes with the activity each quote came from. a range omits activity rows and pages up to 100 days at a time; pass next_cursor as cursor with the same arguments to continue. pass any day as week for one week's full view (memories, up to seven activity rows and body items), or omit week, day_from and day_to for the current week. use include in either mode for a day-by-day summary of body, places, photos, notes or entities. these summaries use what this connection may read and are available without a week page. photos lists up to 50 photos per day in chronological order with time when known, time_source when available, and description (more is true when a day has over 50, and count_is_lower_bound when a range summary is incomplete; search has the rest). entities gives a count and up to 50 non-owner people listed as present or heard in readable activities per day (more is true when a day has over 50), plus top-level people and co_occurrence lists on the first page summarizing non-owner people across the full requested range (day and activity counts for spoken or attended presence, separate in_photo_days and in_photo_activities counts for appearances in imported photos, references and journal links, and who-with-whom co-occurrence counts for spoken or attended presence). body gives daily movement totals (steps, distance_m and active_energy_kcal), nights by source (to_bed, waking, asleep_seconds and stages_seconds with deep, core or light, rem, awake and unspecified when held), and bounded daily readings by source and recorder (resting_heart_rate_bpm, hrv_sdnn_ms and heart_rate_bpm as held min, max and count, plus min_at and max_at on heart_rate_bpm). check the notes alongside movement totals and readings. places lists up to 50 visits, stays and runs of location readings per day with a reference for fetch, plus top-level places_precision and the owner's places list on the first page (with exact coordinates, ~1 km cells or names only according to permission). without a readable week page, days have no state or quotes; memories and summaries still come from readable activities and entries. said quotes the owner's words. on single-week activity rows, in_photo lists people seen in imported photos separately from on_screen entries. mentioned, on_screen and scheduled alone do not establish attendance. use list_activities to list individual activities on a day or short range.",
        input_schema: week_input_schema,
        requires: WEEK,
        audit_name: solstone_core_mcp_audit::ToolName::GetWeek,
    },
    ToolEntry {
        tool_name: crate::jsonrpc::ToolName::ListActivities,
        wire_name: "list_activities",
        title: "list activities",
        description: "list individual activities from the facets this connection may read. use day or an inclusive range with day_from and day_to. for a day-by-day overview across a month, a season or a year (with day links, memories, owner quotes and optional daily summaries), use get_week with day_from and day_to instead when available. on the activity list without the shared find filters, a moment in several facets appears once, with the other facets in also_in. when places is allowed, an activity carries place when available, with words and, according to permission, exact coordinates or a ~1 km cell. participants and heard both count as spoken or attended presence. in_photo lists people seen in imported photos separately from on_screen entries (screen recordings, imported screenshots and other non-photo entries). mentioned, on_screen and scheduled alone do not establish attendance. pass an activity reference to get_activity_sources. in a filtered result, use activity_reference when present. use kind, person, place or activity to find matching journal entries, then pass their references to fetch. pass next_cursor as cursor with the same filters. give the owner a journal link when available.",
        input_schema: list_activities_input_schema,
        requires: FACETS,
        audit_name: solstone_core_mcp_audit::ToolName::ListActivities,
    },
    ToolEntry {
        tool_name: crate::jsonrpc::ToolName::GetActivitySources,
        wire_name: "get_activity_sources",
        title: "read activity sources",
        description: "read the transcript or body sources for activities. pass either reference for one activity or references for up to 12. use activity_reference from a filtered list, or reference from the activity list without the shared find filters or from get_week. each batch item carries its own result or error in input order. pass next_cursor as cursor with that single reference to read the rest. only quote from the source text. give the owner a journal link when available.",
        input_schema: get_activity_sources_input_schema,
        requires: FACETS_AND_SOURCE,
        audit_name: solstone_core_mcp_audit::ToolName::GetActivitySources,
    },
    ToolEntry {
        tool_name: crate::jsonrpc::ToolName::SuggestCorrection,
        wire_name: "suggest_correction",
        title: "suggest a correction",
        description: "suggest a correction to the journal when the owner says something recorded is wrong, when you know firsthand from your own work, or when the owner asks you to look. the suggestion will be held for the owner's review.",
        input_schema: suggest_correction_input_schema,
        requires: SUGGEST_CORRECTION,
        audit_name: solstone_core_mcp_audit::ToolName::SuggestCorrection,
    },
];

fn suggest_correction_input_schema(_context: &SchemaContext<'_>) -> Value {
    let reference = json!({
        "type": "string", "minLength": 1,
        "maxLength": crate::tools::MAX_OPAQUE_REFERENCE_BYTES,
        "description": "the reference returned by a journal read tool"
    });
    let mut kind_names = vec!["recording", "what_happened", "who_said_it", "name"];
    let mut target = json!({
        "type": "object",
        "properties": {
            "reference": reference,
            "lines": {
                "type": "array", "minItems": 1, "maxItems": 20,
                "description": "required for who_said_it. use the time and opening words of each line.",
                "items": {
                    "type": "object",
                    "properties": {
                        "time": {"type": "string", "pattern": "^([01][0-9]|2[0-3]):[0-5][0-9]:[0-5][0-9]$"},
                        "starts_with": {"type": "string", "minLength": 1}
                    },
                    "required": ["time", "starts_with"], "additionalProperties": false
                }
            }
        },
        "required": ["reference"], "additionalProperties": false
    });
    if crate::suggestions::same_person_ready() {
        kind_names.push("same_person");
        target["properties"]["other_reference"] = reference;
        target["properties"]["other_reference"]["description"] =
            json!("the second person reference, required for same_person.");
    }
    json!({
        "type": "object",
        "properties": {
            "kind": {
                "type": "string",
                "enum": kind_names,
                "description": "the kind of correction to suggest"
            },
            "target": target,
            "value": {
                "type": "object",
                "description": "use an empty object for recording or same_person, text for what_happened, speaker for who_said_it, and text with mode for name.",
                "properties": {
                    "text": {"type": "string", "minLength": 1, "maxLength": 600,
                        "description": "for what_happened, use you for the owner and name everyone else, yourself included. the journal reads I as the owner. for name, give the new name, up to 200 characters."},
                    "speaker": {"type": "string", "minLength": 1,
                        "description": "owner, someone_else, or the reference of a visible person"},
                    "mode": {"type": "string", "enum": ["replace", "also_known_as"]}
                },
                "additionalProperties": false
            },
            "basis": {
                "type": "string",
                "enum": ["owner_said", "my_own_work", "journal_shows", "outside_source"],
                "description": "how you know this correction is needed"
            },
            "reason": {
                "type": "string",
                "minLength": 1,
                "maxLength": 200,
                "description": "a concise reason explaining why this correction is suggested (up to 200 characters)"
            },
            "operation_id": {
                "type": "string",
                "description": "a unique client operation id for idempotency"
            }
        },
        "required": ["kind", "target", "value", "basis", "reason", "operation_id"],
        "additionalProperties": false
    })
}

pub(crate) fn tool_by_wire_name(name: &str) -> Option<&'static ToolEntry> {
    TOOLS.iter().find(|entry| entry.wire_name == name)
}

pub(crate) fn find_tool(name: crate::jsonrpc::ToolName) -> &'static ToolEntry {
    TOOLS
        .iter()
        .find(|entry| entry.tool_name == name)
        .expect("every JSON-RPC tool is present in the closed registry")
}

pub(crate) fn snapshot_allows(snapshot: &ConnectionReadSnapshot, entry: &ToolEntry) -> bool {
    requirement_refusal(snapshot, entry).is_none()
}

pub(crate) fn requirement_refusal(
    snapshot: &ConnectionReadSnapshot,
    entry: &ToolEntry,
) -> Option<Reason> {
    if entry.requires.any_of {
        let admitted = entry
            .requires
            .read
            .iter()
            .any(|category| snapshot.categories.contains(category));
        if !entry.requires.read.is_empty() && !admitted {
            return Some(entry.requires.missing);
        }
    } else {
        let missing = entry
            .requires
            .read
            .iter()
            .filter(|category| !snapshot.categories.contains(category))
            .copied()
            .collect::<Vec<_>>();
        if !missing.is_empty() {
            return Some(match missing.as_slice() {
                [AdmittedCategory::Transcripts] => Reason::MissingTranscripts,
                [AdmittedCategory::Facets] => Reason::MissingFacets,
                _ => entry.requires.missing,
            });
        }
    }
    if !entry.requires.one_of.is_empty()
        && !entry
            .requires
            .one_of
            .iter()
            .any(|category| snapshot.categories.contains(category))
    {
        return Some(entry.requires.missing_alternative);
    }
    if entry.requires.whole_journal && !matches!(snapshot.scope, ConnectionScope::WholeJournal) {
        return Some(Reason::WeekWhole);
    }
    None
}

/// The tool list one connection may see, with each schema naming only what that
/// connection can actually use.
///
/// 🔑 Facet enumeration comes from `available_facets`, the **same**
/// snapshot-scoped resolution `list_facets` performs, so the schema discloses
/// nothing the connection cannot already fetch. ⛔ Resolving facets separately
/// here would turn discovery into a content read that never passes the tool
/// authorization path — the objection this construction exists to answer.
///
/// ⚠ This now touches disk on the discovery path. A resolution failure omits
/// the `facet` argument rather than guessing at one: the agent is told less,
/// and call-time authorization is unchanged either way.
pub(crate) fn advertised_tools_list(journal_root: &Path, permission: &PermissionDecision) -> Value {
    let facet_ids = connection_facet_ids(journal_root, permission);
    advertised_tools_list_with_facet_ids(permission, &facet_ids)
}

pub(crate) fn advertised_tools_list_with_facet_ids(
    permission: &PermissionDecision,
    facet_ids: &[String],
) -> Value {
    let snapshot = match permission {
        PermissionDecision::Snapshot(snapshot) => Some(snapshot),
        PermissionDecision::Denied { .. } => None,
    };
    let empty_categories = BTreeSet::new();
    let context = SchemaContext {
        categories: snapshot
            .map(|snapshot| &snapshot.categories)
            .unwrap_or(&empty_categories),
        facet_ids,
    };
    let tools = TOOLS
        .iter()
        .filter(|entry| {
            if entry.tool_name == crate::jsonrpc::ToolName::SuggestCorrection
                && !crate::suggestions::suggestions_ready()
            {
                return false;
            }
            entry.tool_name.is_memory()
                || snapshot.is_some_and(|snapshot| snapshot_allows(snapshot, entry))
        })
        .map(|entry| {
            let mut description = entry.description.to_owned();
            if entry.tool_name == crate::jsonrpc::ToolName::SuggestCorrection
                && crate::suggestions::same_person_ready()
            {
                description.push_str(" if two people in the journal are the same person, use same_person with both references.");
            }
            json!({
                "name": entry.wire_name,
                "description": description,
                "inputSchema": (entry.input_schema)(&context),
                "title": entry.title,
                "annotations": annotations(entry),
            })
        })
        .collect::<Vec<_>>();
    json!({ "tools": tools })
}

/// What a client may assume about a tool's effects. Every tool reads only
/// this journal. `save_memory` adds a note and changes nothing already there,
/// and a retry with the same `operation_id` and content stores it once.
fn annotations(entry: &ToolEntry) -> Value {
    if entry.tool_name == crate::jsonrpc::ToolName::SaveMemory
        || entry.tool_name == crate::jsonrpc::ToolName::SuggestCorrection
    {
        json!({
            "title": entry.title,
            "readOnlyHint": false,
            "destructiveHint": false,
            "idempotentHint": true,
            "openWorldHint": false,
        })
    } else {
        json!({ "title": entry.title, "readOnlyHint": true, "openWorldHint": false })
    }
}

fn connection_facet_ids(journal_root: &Path, permission: &PermissionDecision) -> Vec<String> {
    match permission {
        PermissionDecision::Snapshot(snapshot) => {
            crate::dispatch::available_facets(journal_root, snapshot)
                .map(|facets| facets.into_iter().map(|(id, _, _)| id).collect())
                .unwrap_or_default()
        }
        PermissionDecision::Denied { .. } => Vec::new(),
    }
}

/// Say what is wrong with a refused call's arguments, checked against the
/// schema this connection is advertised, so the agent can correct the call.
///
/// ⚠ Only a refusal reaches this, and the schema it reads names nothing the
/// connection's own `tools/list` did not already show it.
pub(crate) fn invalid_arguments_message(
    journal_root: &Path,
    permission: &PermissionDecision,
    tool: crate::jsonrpc::ToolName,
    arguments: Option<&Value>,
) -> String {
    let entry = find_tool(tool);
    let empty_categories = BTreeSet::new();
    let facet_ids = connection_facet_ids(journal_root, permission);
    let context = SchemaContext {
        categories: match permission {
            PermissionDecision::Snapshot(snapshot) => &snapshot.categories,
            PermissionDecision::Denied { .. } => &empty_categories,
        },
        facet_ids: &facet_ids,
    };
    explain_arguments(entry.wire_name, &(entry.input_schema)(&context), arguments)
}

/// The same, for the paired memory tools, whose schemas name no facet or
/// category and so never read the grant.
pub(crate) fn invalid_memory_arguments_message(
    tool: crate::jsonrpc::ToolName,
    arguments: Option<&Value>,
) -> String {
    let entry = find_tool(tool);
    let context = SchemaContext {
        categories: &BTreeSet::new(),
        facet_ids: &[],
    };
    explain_arguments(entry.wire_name, &(entry.input_schema)(&context), arguments)
}

fn explain_arguments(name: &str, schema: &Value, arguments: Option<&Value>) -> String {
    let empty = Map::new();
    let properties = schema["properties"].as_object().unwrap_or(&empty);
    let arguments = match arguments {
        None | Some(Value::Null) => &empty,
        Some(Value::Object(arguments)) => arguments,
        Some(_) => return format!("{name} takes its arguments as a JSON object."),
    };
    for key in arguments.keys() {
        if !properties.contains_key(key) && !is_earlier_name(name, key) {
            return if properties.is_empty() {
                format!("{name} takes no arguments, so `{key}` is not one.")
            } else {
                let accepted = properties.keys().map(String::as_str).collect::<Vec<_>>();
                format!(
                    "{name} has no `{key}` argument. It accepts: {}.",
                    accepted.join(", ")
                )
            };
        }
    }
    if properties.contains_key("references")
        && arguments.contains_key("reference") == arguments.contains_key("references")
    {
        return format!("{name} takes either `reference` or `references`.");
    }
    if properties.contains_key("references")
        && arguments.contains_key("references")
        && arguments.contains_key("cursor")
    {
        return format!("{name} takes `cursor` with a single `reference`.");
    }
    if name == "list_transcripts"
        && !arguments.contains_key("source")
        && (arguments.contains_key("entry_kind")
            || arguments
                .get("kind")
                .and_then(Value::as_str)
                .is_some_and(|word| AdmittedCategory::from_stored_word(word).is_none()))
    {
        return "list_transcripts needs `day` and a body `source` to filter by `entry_kind`."
            .to_owned();
    }
    if name == "search" && arguments.contains_key("category") && arguments.contains_key("kind") {
        return "search takes either `kind` or `category`.".to_owned();
    }
    for key in schema["required"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        if !arguments.contains_key(key) {
            return format!("{name} needs `{key}`.");
        }
    }
    for (key, value) in arguments {
        if is_earlier_name(name, key) {
            continue;
        }
        if let Some(problem) = explain_value(key, &properties[key], value) {
            return format!("{name}: {problem}");
        }
    }
    let day_bounded = ["day", "day_from", "day_to"]
        .iter()
        .any(|key| arguments.contains_key(*key));
    let query_empty = arguments
        .get("query")
        .and_then(Value::as_str)
        .is_none_or(|query| query.trim().is_empty());
    if name == "search"
        && query_empty
        && !day_bounded
        && !["kind", "category", "person", "place", "activity"]
            .iter()
            .any(|key| arguments.contains_key(*key))
    {
        return "search needs `query`, a day or date range, or a find filter such as `kind`."
            .to_owned();
    }
    let formats = arguments
        .keys()
        .filter_map(|key| format_rule(key).map(|rule| format!("`{key}` {rule}")))
        .collect::<Vec<_>>();
    if formats.is_empty() {
        format!("{name} could not use these arguments together.")
    } else {
        format!(
            "{name} could not use these arguments. Check that {}.",
            formats.join(", and ")
        )
    }
}

/// An argument name a tool took before its arguments were given one
/// vocabulary. It is still accepted, and no longer advertised.
fn is_earlier_name(tool: &str, key: &str) -> bool {
    tool == "list_activities" && matches!(key, "from" | "to")
}

/// The shape a formatted argument must take, for arguments whose format the
/// type, length and range checks cannot see.
fn format_rule(key: &str) -> Option<&'static str> {
    match key {
        "day" | "day_from" | "day_to" | "from" | "to" => Some("is YYYYMMDD or YYYY-MM-DD"),
        "week" => Some("is YYYYMMDD or YYYY-MM-DD"),
        "time_from" | "time_to" => Some("is HH:MM"),
        "operation_id" => Some("uses only letters, digits, '.', '_', ':' or '-'"),
        _ => None,
    }
}

fn explain_value(key: &str, property: &Value, value: &Value) -> Option<String> {
    match property["type"].as_str() {
        Some("string") => {
            let Some(text) = value.as_str() else {
                return Some(format!("`{key}` must be a string."));
            };
            if property["minLength"]
                .as_u64()
                .is_some_and(|minimum| minimum > 0)
                && text.is_empty()
            {
                return Some(format!("`{key}` must not be empty."));
            }
            if let Some(maximum) = property["maxLength"].as_u64()
                && text.len() as u64 > maximum
            {
                return Some(format!("`{key}` is too long: at most {maximum} bytes."));
            }
            if let Some(allowed) = property["enum"].as_array()
                && !allowed.contains(value)
            {
                let allowed = allowed.iter().filter_map(Value::as_str).collect::<Vec<_>>();
                return Some(format!("`{key}` must be one of: {}.", allowed.join(", ")));
            }
            None
        }
        Some("integer") => {
            let Some(number) = value.as_i64() else {
                return Some(format!("`{key}` must be a whole number."));
            };
            let minimum = property["minimum"].as_i64();
            let maximum = property["maximum"].as_i64();
            if minimum.is_some_and(|minimum| number < minimum)
                || maximum.is_some_and(|maximum| number > maximum)
            {
                return Some(match (minimum, maximum) {
                    (Some(minimum), Some(maximum)) => {
                        format!("`{key}` must be from {minimum} to {maximum}.")
                    }
                    (Some(minimum), None) => format!("`{key}` must be at least {minimum}."),
                    (None, _) => {
                        format!("`{key}` must be at most {}.", maximum.unwrap_or_default())
                    }
                });
            }
            None
        }
        _ => None,
    }
}

fn save_memory_input_schema(_context: &SchemaContext<'_>) -> Value {
    json!({
        "type": "object", "additionalProperties": false,
        "required": ["content", "operation_id"],
        "properties": {
            "content": {"type": "string", "minLength": 1, "maxLength": 32768, "description": "exact UTF-8 content, limited to 32,768 decoded bytes"},
            "operation_id": {"type": "string", "minLength": 1, "maxLength": 128, "pattern": "^[A-Za-z0-9._:-]+$", "description": "an id you choose for this save, made of letters, digits, '.', '_', ':' or '-'; reuse it only to retry the same note. the id is unique across this agent's whole history, not only the current pairing."}
        }
    })
}

fn recall_memory_input_schema(_context: &SchemaContext<'_>) -> Value {
    json!({
        "type": "object", "additionalProperties": false,
        "properties": {
            "query": {"type": "string", "maxLength": MAX_QUERY_BYTES},
            "limit": {"type": "integer", "minimum": 1, "maximum": 20, "default": 5},
            "day": {"type": "string", "maxLength": MAX_DAY_BYTES, "pattern": DAY_PATTERN, "description": DAY_DESCRIPTION},
            "day_from": {"type": "string", "maxLength": MAX_DAY_BYTES, "pattern": DAY_PATTERN, "description": DAY_DESCRIPTION},
            "day_to": {"type": "string", "maxLength": MAX_DAY_BYTES, "pattern": DAY_PATTERN, "description": DAY_DESCRIPTION},
            "continuation": {"type": "string", "minLength": 1, "maxLength": MAX_OPAQUE_REFERENCE_BYTES}
        }
    })
}

pub(crate) fn category_token(category: &AdmittedCategory) -> &'static str {
    category.as_str()
}

/// The `facet` argument, enumerated for this connection. `None` means the
/// connection has no facet it may name, so the argument is not offered.
fn facet_property(context: &SchemaContext<'_>) -> Option<Value> {
    if context.facet_ids.is_empty() {
        return None;
    }
    Some(json!({ "type": "string", "enum": context.facet_ids }))
}

/// Add the `facet` argument when, and only when, this connection has a facet
/// it may name. ⛔ An empty `enum` is not valid JSON Schema and would advertise
/// an argument no value satisfies, so the property is omitted instead.
fn with_scoped_facet(mut schema: Value, context: &SchemaContext<'_>) -> Value {
    if let Some(property) = facet_property(context)
        && let Some(properties) = schema.get_mut("properties").and_then(Value::as_object_mut)
    {
        properties.insert("facet".to_owned(), property);
    }
    schema
}

fn with_find_filters(mut schema: Value, context: &SchemaContext<'_>) -> Value {
    let kinds: Vec<_> = context
        .categories
        .iter()
        .filter(|category| category.kind_spec().option_of().is_none())
        .map(category_token)
        .collect();
    if !kinds.is_empty() {
        let kind = json!({"type": "string", "enum": kinds});
        schema["properties"]["kind"] = json!({
            "description": "one kind of material, or a list of kinds.",
            "anyOf": [kind, {"type": "array", "minItems": 1, "items": kind}],
        });
    }
    schema["properties"]["person"] = json!({"type": "string", "maxLength": 256, "description": "a person named on the activity."});
    schema["properties"]["place"] = json!({"type": "string", "maxLength": 256, "description": "a place named in the entry's location information. requires whole-journal access and the places permission."});
    schema["properties"]["activity"] = json!({"type": "string", "maxLength": 256, "description": "a type of activity such as writing. other selected kinds can still match."});
    with_scoped_facet(schema, context)
}

fn list_facets_input_schema(_context: &SchemaContext<'_>) -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "limit": { "type": "integer", "minimum": 1, "maximum": 100 },
            "cursor": { "type": "string", "minLength": 1, "maxLength": MAX_OPAQUE_REFERENCE_BYTES },
        }
    })
}

const SEARCH_QUERY_DESCRIPTION: &str = "a few plain keywords such as names or topics. when no entry matches every word, results match some of them and `relaxed` is true. leave it out to list what a day or range of days holds.";

fn search_input_schema(context: &SchemaContext<'_>) -> Value {
    let schema = json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "query": { "type": "string", "maxLength": MAX_QUERY_BYTES, "description": SEARCH_QUERY_DESCRIPTION },
            "limit": { "type": "integer", "minimum": 1, "maximum": 100, "default": 10 },
            "cursor": { "type": "string", "minLength": 1, "maxLength": MAX_OPAQUE_REFERENCE_BYTES },
            "day": { "type": "string", "minLength": 1, "maxLength": MAX_DAY_BYTES, "pattern": DAY_PATTERN, "description": DAY_DESCRIPTION },
            "day_from": { "type": "string", "minLength": 1, "maxLength": MAX_DAY_BYTES, "pattern": DAY_PATTERN, "description": DAY_DESCRIPTION },
            "day_to": { "type": "string", "minLength": 1, "maxLength": MAX_DAY_BYTES, "pattern": DAY_PATTERN, "description": DAY_DESCRIPTION },
        }
    });
    let mut schema = with_find_filters(schema, context);
    if !context.categories.is_empty() {
        schema["properties"]["category"] = json!({
            "type": "string",
            "enum": context.categories.iter().map(category_token).collect::<Vec<_>>(),
            "description": "older name for a single kind. use category or kind.",
        });
    }
    schema
}

fn with_batch_references(schema: &mut Value) {
    schema["properties"]["references"] = json!({
        "type": "array",
        "minItems": 1,
        "maxItems": crate::tools::MAX_BATCH_REFERENCES,
        "items": schema["properties"]["reference"],
        "description": "read one page per reference in order. each item has its own result or error. follow page continuations in a single read.",
    });
}

fn entity_input_schema(_context: &SchemaContext<'_>) -> Value {
    let reference = json!({
        "type": "string",
        "minLength": 1,
        "maxLength": MAX_OPAQUE_REFERENCE_BYTES,
    });
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "reference": reference,
            "references": {
                "type": "array",
                "minItems": 1,
                "maxItems": crate::tools::MAX_BATCH_REFERENCES,
                "items": reference,
                "description": "read up to 12 entities in order, each with its own result or error.",
            },
        }
    })
}

fn fetch_input_schema(_context: &SchemaContext<'_>) -> Value {
    let mut schema = json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "reference": { "type": "string", "minLength": 1, "maxLength": MAX_OPAQUE_REFERENCE_BYTES },
            "size": {
                "type": "string",
                "enum": ["full", "thumb"],
                "default": "full",
                "description": "image size for imported photos and screenshots: full (default, up to 1024 px) or thumb (up to 320 px, for grids, contact sheets and batches)."
            },
        }
    });
    with_batch_references(&mut schema);
    schema
}

fn list_transcripts_input_schema(context: &SchemaContext<'_>) -> Value {
    let schema = json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "limit": { "type": "integer", "minimum": 1, "maximum": 100, "default": 10 },
            "cursor": { "type": "string", "minLength": 1, "maxLength": MAX_OPAQUE_REFERENCE_BYTES },
            "day": { "type": "string", "minLength": 1, "maxLength": MAX_DAY_BYTES, "pattern": DAY_PATTERN, "description": DAY_DESCRIPTION },
            "day_from": { "type": "string", "pattern": DAY_PATTERN, "description": DAY_DESCRIPTION },
            "day_to": { "type": "string", "pattern": DAY_PATTERN, "description": DAY_DESCRIPTION },
            "time_from": { "type": "string", "description": "Start of a time range within day, as HH:MM." },
            "time_to": { "type": "string", "description": "End of a time range within day, as HH:MM." },
            "source": { "type": "string", "minLength": 1, "maxLength": 256, "description": "A source label, exactly as a segment, search result, other_sources or that day's body document gives it. Matches every readable stream with that label. Needs day, and can't be combined with facet." },
            "entry_kind": { "type": "string", "minLength": 1, "maxLength": 256, "description": "one kind of entry in a body source, such as resting heart rate, workout or sleep. the list shows windows with matching entries. the span returns only those entries." },
        }
    });
    let mut schema = with_find_filters(schema, context);
    let legacy_kind = json!({"type": "string", "minLength": 1, "maxLength": 256});
    let kind = &mut schema["properties"]["kind"];
    if let Some(choices) = kind["anyOf"].as_array_mut() {
        // The string form also accepts older callers' body entry names.
        choices[0] = legacy_kind;
    } else {
        *kind = legacy_kind;
    }
    kind["description"] = json!(
        "one kind of journal material, or a list of kinds. older calls can give a body entry type here with source and day. use entry_kind for that."
    );
    schema
}

fn list_entities_input_schema(context: &SchemaContext<'_>) -> Value {
    let schema = json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "limit": { "type": "integer", "minimum": 1, "maximum": 100 },
            "cursor": { "type": "string", "minLength": 1, "maxLength": MAX_OPAQUE_REFERENCE_BYTES },
        }
    });
    with_scoped_facet(schema, context)
}

fn transcript_input_schema(_context: &SchemaContext<'_>) -> Value {
    let mut schema = json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "reference": { "type": "string", "minLength": 1, "maxLength": MAX_OPAQUE_REFERENCE_BYTES },
            "offset_ms": { "type": "integer", "minimum": 0, "maximum": u32::MAX, "description": "milliseconds from the start of a single screen segment" },
            "display": { "type": "string", "minLength": 1, "maxLength": 255 },
            "cursor": { "type": "string", "minLength": 1, "maxLength": MAX_OPAQUE_REFERENCE_BYTES },
        }
    });
    with_batch_references(&mut schema);
    schema
}

fn week_input_schema(_context: &SchemaContext<'_>) -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "include": {
                "type": "array",
                "maxItems": crate::tools::week::SUMMARY_KINDS.len(),
                "items": {"type": "string", "enum": crate::tools::week::SUMMARY_KINDS.iter().map(category_token).collect::<Vec<_>>()},
                "description": "kinds to summarize inside each day, for one week or across a date range. body gives daily movement totals (steps, distance_m, active_energy_kcal), nights with stages_seconds, and bounded daily readings (resting_heart_rate_bpm, hrv_sdnn_ms, heart_rate_bpm min/max/count). photos lists up to 50 photos per day in chronological order with time when known, time_source when available, and description. entities gives a count and up to 50 non-owner people per day, plus top-level people and co_occurrence summaries across the full range on the first page.",
            },
            "week": {
                "type": "string",
                "maxLength": MAX_DAY_BYTES,
                "pattern": DAY_PATTERN,
                "description": "any day in a single week (sunday to saturday), as YYYYMMDD or YYYY-MM-DD. omit week, day_from and day_to to read the week containing today. mutually exclusive with day_from, day_to and cursor.",
            },
            "day_from": {
                "type": "string",
                "minLength": 1,
                "maxLength": MAX_DAY_BYTES,
                "pattern": DAY_PATTERN,
                "description": "first day of an inclusive date range (up to 366 days, such as a month, a season or a year), as YYYYMMDD or YYYY-MM-DD. use with day_to to read one compact row per day instead of a single week. mutually exclusive with week.",
            },
            "day_to": {
                "type": "string",
                "minLength": 1,
                "maxLength": MAX_DAY_BYTES,
                "pattern": DAY_PATTERN,
                "description": "last day of an inclusive date range (up to 366 days), as YYYYMMDD or YYYY-MM-DD. use with day_from to read one compact row per day instead of a single week. days after today are omitted. mutually exclusive with week.",
            },
            "cursor": {
                "type": "string",
                "minLength": 1,
                "maxLength": MAX_OPAQUE_REFERENCE_BYTES,
                "description": "opaque cursor from next_cursor in a previous get_week date-range call. pass with the same day_from, day_to and include.",
            }
        }
    })
}

fn list_activities_input_schema(context: &SchemaContext<'_>) -> Value {
    let schema = json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "day": { "type": "string", "minLength": 1, "maxLength": MAX_DAY_BYTES, "pattern": DAY_PATTERN, "description": "a single day, YYYYMMDD or YYYY-MM-DD. mutually exclusive with day_from and day_to. without any of them, today." },
            "day_from": { "type": "string", "minLength": 1, "maxLength": MAX_DAY_BYTES, "pattern": DAY_PATTERN, "description": "first day of a range of individual activities, YYYYMMDD or YYYY-MM-DD (inclusive). for one summary row per day across a month, season or year, use get_week with day_from and day_to when available." },
            "day_to": { "type": "string", "minLength": 1, "maxLength": MAX_DAY_BYTES, "pattern": DAY_PATTERN, "description": "last day of a range of individual activities, YYYYMMDD or YYYY-MM-DD (inclusive)." },
            "entity": { "type": "string", "description": "filter by entity display name, or by an entity's reference from list_entities or search." },
            "limit": { "type": "integer", "minimum": 1, "maximum": 100, "default": 10, "description": "maximum number of activities to return (default 10, max 100)." },
            "cursor": { "type": "string", "minLength": 1, "maxLength": MAX_OPAQUE_REFERENCE_BYTES, "description": "opaque cursor from a previous list_activities call." },
        }
    });
    with_find_filters(schema, context)
}

fn get_activity_sources_input_schema(_context: &SchemaContext<'_>) -> Value {
    let mut schema = json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "reference": { "type": "string", "minLength": 1, "maxLength": MAX_OPAQUE_REFERENCE_BYTES, "description": "An activity's reference, from list_activities or get_week." },
            "cursor": { "type": "string", "minLength": 1, "maxLength": MAX_OPAQUE_REFERENCE_BYTES, "description": "Opaque cursor from a previous get_activity_sources call." },
        }
    });
    with_batch_references(&mut schema);
    schema
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::permissions::{ConnectionReadSnapshot, PermissionStore, ReadPermission, ReadScope};
    use crate::tools::MAX_FACET_BYTES;
    use solstone_core_indexer_query::ConnectionScope;
    use std::collections::BTreeSet;
    use std::fs;

    const FACET_A: &str = "123e4567-e89b-42d3-a456-426614174000";
    const FACET_B: &str = "123e4567-e89b-42d3-a456-426614174001";

    fn journal_with_two_facets() -> tempfile::TempDir {
        let journal = tempfile::Builder::new()
            .prefix("solstone-mcp-registry-")
            .tempdir_in(crate::test_scratch())
            .unwrap();
        for (name, id, title) in [("alpha", FACET_A, "Alpha"), ("beta", FACET_B, "Beta")] {
            let directory = journal.path().join("facets").join(name);
            fs::create_dir_all(&directory).unwrap();
            fs::write(
                directory.join("facet.json"),
                json!({"id": id, "title": title, "description": format!("{title} details")})
                    .to_string(),
            )
            .unwrap();
        }
        journal
    }

    fn context<'a>(
        categories: &'a BTreeSet<AdmittedCategory>,
        facet_ids: &'a [String],
    ) -> SchemaContext<'a> {
        SchemaContext {
            categories,
            facet_ids,
        }
    }

    fn all_categories() -> BTreeSet<AdmittedCategory> {
        [
            AdmittedCategory::Transcripts,
            AdmittedCategory::Entities,
            AdmittedCategory::Facets,
            AdmittedCategory::ScreenText,
        ]
        .into_iter()
        .collect()
    }

    #[test]
    fn empty_snapshot_advertises_memory_pair_and_facet_listing() {
        let journal = journal_with_two_facets();
        let snapshot = ConnectionReadSnapshot {
            categories: BTreeSet::new(),
            scope: ConnectionScope::WholeJournal,
            generation: 1,
            places_precision: None,
        };

        let tools = advertised_tools_list(journal.path(), &PermissionDecision::Snapshot(snapshot));
        assert_eq!(tools["tools"].as_array().unwrap().len(), 3);
        assert_eq!(tools["tools"][0]["name"], "save_memory");
        assert_eq!(tools["tools"][1]["name"], "recall_memory");
        assert_eq!(tools["tools"][2]["name"], "list_facets");
    }

    #[test]
    fn a_chosen_facet_schema_enumerates_that_facet_and_no_other() {
        let journal = journal_with_two_facets();
        let snapshot = ConnectionReadSnapshot {
            categories: [AdmittedCategory::Transcripts].into_iter().collect(),
            scope: ConnectionScope::ChosenFacets {
                ids: [FACET_A.to_owned()].into_iter().collect(),
            },
            generation: 1,
            places_precision: None,
        };

        let tools = advertised_tools_list(journal.path(), &PermissionDecision::Snapshot(snapshot));
        let search = tools["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|tool| tool["name"] == "search")
            .unwrap();
        assert_eq!(
            search["inputSchema"]["properties"]["facet"]["enum"],
            json!([FACET_A])
        );
        // The one category this connection has, and not the two it does not.
        assert_eq!(
            search["inputSchema"]["properties"]["kind"]["anyOf"][0]["enum"],
            json!(["transcripts"])
        );
        let serialized = tools.to_string();
        assert!(!serialized.contains(FACET_B), "advertised a foreign facet");
        // ⛔ Discovery still carries no journal content: ids, never titles.
        assert!(!serialized.contains("Alpha"));
        assert!(!serialized.contains("Beta details"));
    }

    #[test]
    fn the_advertised_facet_enum_never_exceeds_what_list_facets_already_returns() {
        let journal = journal_with_two_facets();
        for scope in [
            ConnectionScope::WholeJournal,
            ConnectionScope::ChosenFacets {
                ids: [FACET_B.to_owned()].into_iter().collect(),
            },
        ] {
            let snapshot = ConnectionReadSnapshot {
                categories: all_categories(),
                scope,
                generation: 1,
                places_precision: None,
            };
            PermissionStore::open(journal.path())
                .set_permission(
                    "bearer:registry",
                    ReadPermission {
                        categories: vec![
                            "transcripts".to_owned(),
                            "entities".to_owned(),
                            "facets".to_owned(),
                        ],
                        scope: match &snapshot.scope {
                            ConnectionScope::WholeJournal => ReadScope::WholeJournal,
                            ConnectionScope::ChosenFacets { ids } => ReadScope::Facets {
                                ids: ids.iter().cloned().collect(),
                            },
                        },
                    },
                )
                .unwrap();
            let listed = crate::dispatch::run_mcp_probe(
                journal.path(),
                "bearer:registry",
                "list_facets",
                &json!({}),
            )
            .unwrap();
            let reachable = listed["facets"]
                .as_array()
                .unwrap()
                .iter()
                .map(|facet| facet["id"].as_str().unwrap().to_owned())
                .collect::<BTreeSet<_>>();

            let tools = advertised_tools_list(
                journal.path(),
                &PermissionDecision::Snapshot(snapshot.clone()),
            );
            let advertised = tools["tools"]
                .as_array()
                .unwrap()
                .iter()
                .find(|tool| tool["name"] == "search")
                .unwrap()["inputSchema"]["properties"]["facet"]["enum"]
                .as_array()
                .unwrap()
                .iter()
                .map(|id| id.as_str().unwrap().to_owned())
                .collect::<BTreeSet<_>>();
            assert_eq!(
                advertised, reachable,
                "the schema names exactly what list_facets already hands over"
            );
        }
    }

    #[test]
    fn a_journal_with_no_facets_offers_no_facet_argument_at_all() {
        let journal = tempfile::Builder::new()
            .prefix("solstone-mcp-registry-bare-")
            .tempdir_in(crate::test_scratch())
            .unwrap();
        let snapshot = ConnectionReadSnapshot {
            categories: all_categories(),
            scope: ConnectionScope::WholeJournal,
            generation: 1,
            places_precision: None,
        };
        let tools = advertised_tools_list(journal.path(), &PermissionDecision::Snapshot(snapshot));
        for tool in tools["tools"].as_array().unwrap() {
            assert!(
                tool["inputSchema"]["properties"].get("facet").is_none(),
                "{} offered an unusable facet argument",
                tool["name"]
            );
        }
    }

    #[test]
    fn advertised_effects_distinguish_append_from_reads() {
        // Dropped once in a rewrite without anyone noticing; clients use the
        // hint to run a tool without asking the owner each time.
        crate::suggestions::set_test_suggestions_ready(true);
        let journal = journal_with_two_facets();
        let snapshot = ConnectionReadSnapshot {
            categories: all_categories(),
            scope: ConnectionScope::WholeJournal,
            generation: 1,
            places_precision: None,
        };
        let tools = advertised_tools_list(journal.path(), &PermissionDecision::Snapshot(snapshot));
        let tools = tools["tools"].as_array().unwrap();
        assert_eq!(tools.len(), TOOLS.len());
        for tool in tools {
            assert_eq!(
                tool["annotations"]["readOnlyHint"],
                !matches!(
                    tool["name"].as_str(),
                    Some("save_memory" | "suggest_correction")
                ),
                "{} is not marked read-only",
                tool["name"]
            );
            // Connector directories require a title and an effect hint on
            // every tool.
            assert!(
                tool["title"]
                    .as_str()
                    .is_some_and(|title| !title.is_empty()),
                "{} has no title",
                tool["name"]
            );
            assert_eq!(tool["annotations"]["title"], tool["title"]);
        }
        let save = tools
            .iter()
            .find(|tool| tool["name"] == "save_memory")
            .unwrap();
        assert_eq!(save["annotations"]["destructiveHint"], false);
    }

    #[test]
    fn a_refused_call_names_the_argument_to_correct() {
        let journal = journal_with_two_facets();
        let snapshot = ConnectionReadSnapshot {
            categories: all_categories(),
            scope: ConnectionScope::ChosenFacets {
                ids: [FACET_A.to_owned()].into_iter().collect(),
            },
            generation: 1,
            places_precision: None,
        };
        let permission = PermissionDecision::Snapshot(snapshot);
        let explain = |tool, arguments: Value| {
            invalid_arguments_message(journal.path(), &permission, tool, Some(&arguments))
        };
        use crate::jsonrpc::ToolName;
        for (tool, arguments, named) in [
            (
                ToolName::Search,
                json!({"query": "x", "limit": 0}),
                "`limit`",
            ),
            (
                ToolName::Search,
                json!({"query": "x", "colour": "red"}),
                "`colour`",
            ),
            (
                ToolName::Search,
                json!({"query": "x", "category": "audio"}),
                "`category`",
            ),
            (ToolName::Search, json!({}), "`query`"),
            (ToolName::Fetch, json!({}), "`reference`"),
            (
                ToolName::Fetch,
                json!({"reference": "ref-1", "size": "huge"}),
                "`size`",
            ),
            (
                ToolName::Fetch,
                json!({"references": ["ref-1"], "size": "huge"}),
                "`size`",
            ),
            (
                ToolName::ListTranscripts,
                json!({"day": "tuesday"}),
                "`day`",
            ),
            (
                ToolName::ListActivities,
                json!({"from": "tuesday"}),
                "`from`",
            ),
            (ToolName::GetWeek, json!({"week": "tuesday"}), "`week`"),
            (
                ToolName::GetWeek,
                json!({"day_from": "tuesday", "day_to": "2025-01-02"}),
                "`day_from`",
            ),
            (
                ToolName::GetWeek,
                json!({"day_from": "2025-01-01", "day_to": "tuesday"}),
                "`day_to`",
            ),
        ] {
            let message = explain(tool, arguments);
            assert!(message.contains(named), "{message} does not name {named}");
        }
        // ⛔ A refusal discloses no facet beyond what tools/list already showed.
        let message = explain(ToolName::Search, json!({"query": "x", "facet": "other"}));
        assert!(
            message.contains(FACET_A) && !message.contains(FACET_B),
            "{message}"
        );
        let message =
            invalid_memory_arguments_message(ToolName::SaveMemory, Some(&json!({"content": "x"})));
        assert!(message.contains("`operation_id`"), "{message}");
    }

    #[test]
    fn list_schemas_only_advertise_the_arguments_their_validators_accept() {
        let mut categories = all_categories();
        categories.insert(AdmittedCategory::ProfilePics);
        let facet_ids = vec![FACET_A.to_owned()];
        let context = context(&categories, &facet_ids);
        for schema in [
            list_facets_input_schema(&context),
            list_entities_input_schema(&context),
        ] {
            assert_eq!(schema["additionalProperties"], false);
            assert!(schema["properties"].get("cursor").is_some());
        }
        let transcript_schema = list_transcripts_input_schema(&context);
        assert_eq!(transcript_schema["additionalProperties"], false);
        assert!(transcript_schema["properties"].get("cursor").is_some());
        assert!(transcript_schema["properties"].get("day").is_some());
        assert!(transcript_schema["properties"].get("time_from").is_some());
        assert!(transcript_schema["properties"].get("time_to").is_some());
        assert!(transcript_schema["properties"].get("facet").is_some());
        assert_eq!(
            transcript_schema["properties"]["kind"]["anyOf"][0]["type"],
            "string"
        );
        assert!(
            transcript_schema["properties"]["kind"]["anyOf"][0]
                .get("enum")
                .is_none()
        );
        assert_eq!(
            search_input_schema(&context)["properties"]["category"]["enum"],
            json!(categories.iter().map(category_token).collect::<Vec<_>>())
        );
        assert!(
            list_entities_input_schema(&context)["properties"]
                .get("facet")
                .is_some()
        );
        assert_eq!(
            search_input_schema(&context)["properties"]["query"]["maxLength"],
            MAX_QUERY_BYTES
        );
        assert!(crate::tools::facets::validate(Some(&json!({"cursor": "nope"}))).is_ok());
        assert!(crate::tools::facets::validate(Some(&json!({"cursor": ""}))).is_err());
        assert!(crate::tools::transcripts::validate_list(Some(&json!({"cursor": "nope"}))).is_ok());
        assert!(crate::tools::transcripts::validate_list(Some(&json!({"cursor": ""}))).is_err());
        assert!(crate::tools::entities::validate_list(Some(&json!({"cursor": "nope"}))).is_ok());
        assert!(crate::tools::entities::validate_list(Some(&json!({"cursor": ""}))).is_err());
    }

    #[test]
    fn tool_input_schemas_are_plain_objects_at_the_root() {
        let categories = all_categories();
        let facets = vec![FACET_A.to_owned()];
        let context = context(&categories, &facets);
        for entry in TOOLS {
            let schema = (entry.input_schema)(&context);
            assert_eq!(schema["type"], "object", "{}", entry.wire_name);
            for keyword in ["oneOf", "anyOf", "allOf"] {
                assert!(
                    schema.get(keyword).is_none(),
                    "{} has a top-level {keyword}",
                    entry.wire_name
                );
            }
        }
    }

    #[test]
    fn opaque_schema_bounds_match_the_validators() {
        let categories = all_categories();
        let facet_ids = vec![FACET_A.to_owned()];
        let context = context(&categories, &facet_ids);
        let opaque = "x".repeat(MAX_OPAQUE_REFERENCE_BYTES);
        let too_long = "x".repeat(MAX_OPAQUE_REFERENCE_BYTES + 1);
        assert_eq!(
            search_input_schema(&context)["properties"]["cursor"]["maxLength"],
            MAX_OPAQUE_REFERENCE_BYTES
        );
        assert_eq!(
            list_transcripts_input_schema(&context)["properties"]["cursor"]["maxLength"],
            MAX_OPAQUE_REFERENCE_BYTES
        );
        assert_eq!(
            list_facets_input_schema(&context)["properties"]["cursor"]["maxLength"],
            MAX_OPAQUE_REFERENCE_BYTES
        );
        assert_eq!(
            list_entities_input_schema(&context)["properties"]["cursor"]["maxLength"],
            MAX_OPAQUE_REFERENCE_BYTES
        );
        assert_eq!(
            week_input_schema(&context)["properties"]["cursor"]["maxLength"],
            MAX_OPAQUE_REFERENCE_BYTES
        );
        assert_eq!(
            fetch_input_schema(&context)["properties"]["reference"]["maxLength"],
            MAX_OPAQUE_REFERENCE_BYTES
        );
        assert!(
            crate::tools::search::validate(Some(&json!({
                "query": "query",
                "cursor": opaque,
                "day": "20260914",
                "facet": "facet-id"
            })))
            .is_ok()
        );
        assert!(
            crate::tools::search::validate(Some(&json!({
                "query": "query",
                "day": "x".repeat(MAX_DAY_BYTES + 1)
            })))
            .is_err()
        );
        assert!(
            crate::tools::facets::validate(Some(&json!({
                "cursor": opaque,
            })))
            .is_ok()
        );
        assert!(
            crate::tools::facets::validate(Some(&json!({
                "cursor": too_long,
            })))
            .is_err()
        );
        assert!(
            crate::tools::entities::validate_list(Some(&json!({
                "cursor": opaque,
            })))
            .is_ok()
        );
        assert!(
            crate::tools::entities::validate_list(Some(&json!({
                "cursor": too_long,
            })))
            .is_err()
        );
        assert!(
            crate::tools::entities::validate_list(Some(&json!({
                "facet": "x".repeat(MAX_FACET_BYTES + 1)
            })))
            .is_err()
        );
        let entity_schema = entity_input_schema(&context);
        assert_eq!(entity_schema["additionalProperties"], false);
        assert!(entity_schema.get("required").is_none());
        assert_eq!(
            entity_schema["properties"]["reference"]["maxLength"],
            MAX_OPAQUE_REFERENCE_BYTES
        );
        assert_eq!(
            entity_schema["properties"]["references"]["maxItems"],
            crate::tools::MAX_BATCH_REFERENCES
        );
        assert_eq!(
            explain_arguments(
                "get_entity",
                &entity_schema,
                Some(&json!({"reference": "a", "references": ["b"]}))
            ),
            "get_entity takes either `reference` or `references`."
        );
        assert_eq!(
            explain_arguments("get_entity", &entity_schema, Some(&json!({}))),
            "get_entity takes either `reference` or `references`."
        );
    }
}
