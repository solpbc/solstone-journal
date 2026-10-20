// SPDX-License-Identifier: MIT

use std::collections::BTreeSet;
use std::path::Path;

use serde_json::{Map, Value, json};
use solstone_core_indexer_query::{AdmittedCategory, ConnectionScope};

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
};
const TRANSCRIPTS: ToolRequirement = ToolRequirement {
    read: &[AdmittedCategory::Transcripts],
    whole_journal: false,
    any_of: false,
};
const ENTITIES: ToolRequirement = ToolRequirement {
    read: &[AdmittedCategory::Entities],
    whole_journal: false,
    any_of: false,
};
const FACETS: ToolRequirement = ToolRequirement {
    read: &[AdmittedCategory::Facets],
    whole_journal: false,
    any_of: false,
};
const FACETS_AND_TRANSCRIPTS: ToolRequirement = ToolRequirement {
    read: &[AdmittedCategory::Facets, AdmittedCategory::Transcripts],
    whole_journal: false,
    any_of: false,
};
const WEEK: ToolRequirement = ToolRequirement {
    read: &[AdmittedCategory::Transcripts, AdmittedCategory::Facets],
    whole_journal: true,
    any_of: false,
};
const SEARCH: ToolRequirement = ToolRequirement {
    read: &[AdmittedCategory::Transcripts, AdmittedCategory::ScreenText],
    whole_journal: false,
    any_of: true,
};
/// `list_activities` mints activity references under a facets grant, and search
/// mints entry references under a transcripts or screen_text grant. Advertising `fetch` for
/// any grant lets those references round-trip. The call still refuses an
/// entry without transcripts or screen_text and an activity without facets.
const FETCH: ToolRequirement = ToolRequirement {
    read: &[
        AdmittedCategory::Transcripts,
        AdmittedCategory::Facets,
        AdmittedCategory::ScreenText,
    ],
    whole_journal: false,
    any_of: true,
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
        description: "search what this connection may see of the owner's journal: short summaries of conversations and work sessions, activities and their stories, text recognized from screens, imported documents and calendar events, and notes about people and projects. screen text says what was visible, not what the owner read, typed or did. use a few keywords such as names or topics, not a sentence. days are YYYYMMDD or YYYY-MM-DD. search does not read conversations word for word. when a result lists transcript segments, read them with get_transcript before saying who said, promised or decided something. when a result shows more_segments, fetch it for more. an activity result includes hidden, true or false. trust that field. the text can mention hidden without the activity being hidden. if an activity you just saw disappears, search again; a changed activity file stays out until the journal indexes it.",
        input_schema: search_input_schema,
        requires: SEARCH,
        audit_name: solstone_core_mcp_audit::ToolName::Search,
    },
    ToolEntry {
        tool_name: crate::jsonrpc::ToolName::Fetch,
        wire_name: "fetch",
        title: "read a journal entry",
        description: "read the text of an entry returned by search. pass its reference unchanged. if next_reference is present, pass it as reference to fetch the next part. join the text parts in order. when the entry lists transcript segments, read them with get_transcript. if it still shows more_segments, call list_transcripts for that day and the entry's time. an activity includes hidden, true or false, along with its text. trust that field. the text can mention hidden without the activity being hidden. if the reference does not resolve, search again.",
        input_schema: fetch_input_schema,
        requires: FETCH,
        audit_name: solstone_core_mcp_audit::ToolName::Fetch,
    },
    ToolEntry {
        tool_name: crate::jsonrpc::ToolName::ListTranscripts,
        wire_name: "list_transcripts",
        title: "list transcripts",
        description: "List transcript segments this connection may read, each with its time. Give one day (YYYYMMDD or YYYY-MM-DD), and a time range within it when you know one; without a day, the newest days come first. When more remain, pass next_cursor as cursor to continue. Read one with get_transcript.",
        input_schema: list_transcripts_input_schema,
        requires: TRANSCRIPTS,
        audit_name: solstone_core_mcp_audit::ToolName::ListTranscripts,
    },
    ToolEntry {
        tool_name: crate::jsonrpc::ToolName::GetTranscript,
        wire_name: "get_transcript",
        title: "read a transcript",
        description: "Read what was said in one transcript segment, from a search result or from list_transcripts. When it returns next_cursor, pass it as cursor to read the rest. When it says untimed, the times are not real. If a segment can't be read, search again or list that day.",
        input_schema: transcript_input_schema,
        requires: TRANSCRIPTS,
        audit_name: solstone_core_mcp_audit::ToolName::GetTranscript,
    },
    ToolEntry {
        tool_name: crate::jsonrpc::ToolName::ListEntities,
        wire_name: "list_entities",
        title: "list people and projects",
        description: "List the people, projects and other entities this connection may see. When more remain, pass next_cursor as cursor to continue. To learn who or what one is, call get_entity with its reference.",
        input_schema: list_entities_input_schema,
        requires: ENTITIES,
        audit_name: solstone_core_mcp_audit::ToolName::ListEntities,
    },
    ToolEntry {
        tool_name: crate::jsonrpc::ToolName::GetEntity,
        wire_name: "get_entity",
        title: "read about a person or project",
        description: "Read what the journal has noted about an entity returned by list_entities: its type and, in that facet, its description, the last day it came up, and notes about it.",
        input_schema: fetch_input_schema,
        requires: ENTITIES,
        audit_name: solstone_core_mcp_audit::ToolName::GetEntity,
    },
    ToolEntry {
        tool_name: crate::jsonrpc::ToolName::GetWeek,
        wire_name: "get_week",
        title: "read a week",
        description: "read one week. memories are derived from activities and are not the owner's words. quotes are the owner's words. leaving an entry off the week page leaves that source visible to the other tools. pass week as the sunday it starts, YYYYMMDD, or omit it for the week containing today.",
        input_schema: week_input_schema,
        requires: WEEK,
        audit_name: solstone_core_mcp_audit::ToolName::GetWeek,
    },
    ToolEntry {
        tool_name: crate::jsonrpc::ToolName::ListActivities,
        wire_name: "list_activities",
        title: "list activities",
        description: "List journal activities for one day or an inclusive range across granted facets. When more remain, pass next_cursor as cursor to continue. Call get_activity_sources to read the verbatim transcript segments an activity spans.",
        input_schema: list_activities_input_schema,
        requires: FACETS,
        audit_name: solstone_core_mcp_audit::ToolName::ListActivities,
    },
    ToolEntry {
        tool_name: crate::jsonrpc::ToolName::GetActivitySources,
        wire_name: "get_activity_sources",
        title: "read activity sources",
        description: "Read the verbatim transcript segments an activity spans. Pass the activity reference returned by list_activities or get_week. When more remain, pass next_cursor as cursor to continue. When a unit says untimed, the times inside it are not real.",
        input_schema: get_activity_sources_input_schema,
        requires: FACETS_AND_TRANSCRIPTS,
        audit_name: solstone_core_mcp_audit::ToolName::GetActivitySources,
    },
];

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
) -> Option<String> {
    if entry.requires.any_of {
        let admitted = entry
            .requires
            .read
            .iter()
            .any(|category| snapshot.categories.contains(category));
        if !entry.requires.read.is_empty() && !admitted {
            let names = entry
                .requires
                .read
                .iter()
                .map(category_token)
                .collect::<Vec<_>>()
                .join(" or ");
            return Some(format!(
                "this connection does not have the {names} category"
            ));
        }
    } else {
        let missing = entry
            .requires
            .read
            .iter()
            .filter(|category| !snapshot.categories.contains(category))
            .map(category_token)
            .collect::<Vec<_>>();
        if !missing.is_empty() {
            return Some(format!(
                "this connection does not have the {} category",
                missing.join(" and ")
            ));
        }
    }
    if entry.requires.whole_journal && !matches!(snapshot.scope, ConnectionScope::WholeJournal) {
        return Some(
            "this connection can read a week only with access to the whole journal.".to_owned(),
        );
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
    let snapshot = match permission {
        PermissionDecision::Snapshot(snapshot) => Some(snapshot),
        PermissionDecision::Denied { .. } => None,
    };
    let empty_categories = BTreeSet::new();
    let facet_ids = connection_facet_ids(journal_root, permission);
    let context = SchemaContext {
        categories: snapshot
            .map(|snapshot| &snapshot.categories)
            .unwrap_or(&empty_categories),
        facet_ids: &facet_ids,
    };
    let tools = TOOLS
        .iter()
        .filter(|entry| {
            entry.tool_name.is_memory()
                || snapshot.is_some_and(|snapshot| snapshot_allows(snapshot, entry))
        })
        .map(|entry| {
            json!({
                "name": entry.wire_name,
                "description": entry.description,
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
    if entry.tool_name == crate::jsonrpc::ToolName::SaveMemory {
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
        if !properties.contains_key(key) {
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
    if name == "search" && query_empty && !day_bounded {
        return "search needs `query`, or `day`, `day_from` or `day_to` to list what those days hold."
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

/// The shape a formatted argument must take, for arguments whose format the
/// type, length and range checks cannot see.
fn format_rule(key: &str) -> Option<&'static str> {
    match key {
        "day" | "day_from" | "day_to" => Some("is YYYYMMDD or YYYY-MM-DD"),
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
            "operation_id": {"type": "string", "minLength": 1, "maxLength": 128, "pattern": "^[A-Za-z0-9._:-]+$", "description": "an id you choose for this save, made of letters, digits, '.', '_', ':' or '-'; reuse it only to retry the same note"}
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
    match category {
        AdmittedCategory::Transcripts => "transcripts",
        AdmittedCategory::Entities => "entities",
        AdmittedCategory::Facets => "facets",
        AdmittedCategory::ScreenText => "screen_text",
    }
}

/// The `facet` argument, enumerated for this connection. `None` means the
/// connection has no facet it may name, so the argument is not offered.
fn facet_property(context: &SchemaContext<'_>) -> Option<Value> {
    if context.facet_ids.is_empty() {
        return None;
    }
    Some(json!({ "type": "string", "enum": context.facet_ids }))
}

fn category_property(context: &SchemaContext<'_>) -> Option<Value> {
    if context.categories.is_empty() {
        return None;
    }
    Some(json!({
        "type": "string",
        "enum": context.categories.iter().map(category_token).collect::<Vec<_>>(),
    }))
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
    let mut schema = json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "query": { "type": "string", "maxLength": MAX_QUERY_BYTES, "description": SEARCH_QUERY_DESCRIPTION },
            "limit": { "type": "integer", "minimum": 1, "maximum": 100 },
            "cursor": { "type": "string", "minLength": 1, "maxLength": MAX_OPAQUE_REFERENCE_BYTES },
            "day": { "type": "string", "minLength": 1, "maxLength": MAX_DAY_BYTES, "pattern": DAY_PATTERN, "description": DAY_DESCRIPTION },
            "day_from": { "type": "string", "minLength": 1, "maxLength": MAX_DAY_BYTES, "pattern": DAY_PATTERN, "description": DAY_DESCRIPTION },
            "day_to": { "type": "string", "minLength": 1, "maxLength": MAX_DAY_BYTES, "pattern": DAY_PATTERN, "description": DAY_DESCRIPTION },
        }
    });
    if let Some(properties) = schema.get_mut("properties").and_then(Value::as_object_mut)
        && let Some(category) = category_property(context)
    {
        properties.insert("category".to_owned(), category);
    }
    with_scoped_facet(schema, context)
}

fn fetch_input_schema(_context: &SchemaContext<'_>) -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["reference"],
        "properties": {
            "reference": { "type": "string", "minLength": 1, "maxLength": MAX_OPAQUE_REFERENCE_BYTES },
        }
    })
}

fn list_transcripts_input_schema(context: &SchemaContext<'_>) -> Value {
    let schema = json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "limit": { "type": "integer", "minimum": 1, "maximum": 100 },
            "cursor": { "type": "string", "minLength": 1, "maxLength": MAX_OPAQUE_REFERENCE_BYTES },
            "day": { "type": "string", "minLength": 1, "maxLength": MAX_DAY_BYTES, "pattern": DAY_PATTERN, "description": DAY_DESCRIPTION },
            "time_from": { "type": "string", "description": "Start of a time range within day, as HH:MM." },
            "time_to": { "type": "string", "description": "End of a time range within day, as HH:MM." },
        }
    });
    with_scoped_facet(schema, context)
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
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["reference"],
        "properties": {
            "reference": { "type": "string", "minLength": 1, "maxLength": MAX_OPAQUE_REFERENCE_BYTES },
            "cursor": { "type": "string", "minLength": 1, "maxLength": MAX_OPAQUE_REFERENCE_BYTES },
        }
    })
}

fn week_input_schema(_context: &SchemaContext<'_>) -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "week": {
                "type": "string",
                "pattern": "^[0-9]{8}$",
                "description": "the sunday a week starts, as YYYYMMDD. omit it to read the week containing today.",
            }
        }
    })
}

fn list_activities_input_schema(context: &SchemaContext<'_>) -> Value {
    let schema = json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "day": { "type": "string", "minLength": 1, "maxLength": MAX_DAY_BYTES, "pattern": DAY_PATTERN, "description": "A single day, YYYYMMDD or YYYY-MM-DD. Mutually exclusive with from and to." },
            "from": { "type": "string", "minLength": 1, "maxLength": MAX_DAY_BYTES, "pattern": DAY_PATTERN, "description": "Start of date range, YYYYMMDD or YYYY-MM-DD (inclusive)." },
            "to": { "type": "string", "minLength": 1, "maxLength": MAX_DAY_BYTES, "pattern": DAY_PATTERN, "description": "End of date range, YYYYMMDD or YYYY-MM-DD (inclusive)." },
            "entity": { "type": "string", "description": "Filter by entity display name or opaque entity reference." },
            "limit": { "type": "integer", "minimum": 1, "maximum": 100, "description": "Maximum number of activities to return (default 10, max 100)." },
            "cursor": { "type": "string", "minLength": 1, "maxLength": MAX_OPAQUE_REFERENCE_BYTES, "description": "Opaque cursor from a previous list_activities call." },
        }
    });
    with_scoped_facet(schema, context)
}

fn get_activity_sources_input_schema(_context: &SchemaContext<'_>) -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["reference"],
        "properties": {
            "reference": { "type": "string", "minLength": 1, "maxLength": MAX_OPAQUE_REFERENCE_BYTES, "description": "Opaque activity reference." },
            "cursor": { "type": "string", "minLength": 1, "maxLength": MAX_OPAQUE_REFERENCE_BYTES, "description": "Opaque cursor from a previous get_activity_sources call." },
        }
    })
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
            search["inputSchema"]["properties"]["category"]["enum"],
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
        let journal = journal_with_two_facets();
        let snapshot = ConnectionReadSnapshot {
            categories: all_categories(),
            scope: ConnectionScope::WholeJournal,
            generation: 1,
        };
        let tools = advertised_tools_list(journal.path(), &PermissionDecision::Snapshot(snapshot));
        let tools = tools["tools"].as_array().unwrap();
        assert_eq!(tools.len(), TOOLS.len());
        for tool in tools {
            assert_eq!(
                tool["annotations"]["readOnlyHint"],
                tool["name"] != "save_memory",
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
                ToolName::ListTranscripts,
                json!({"day": "tuesday"}),
                "`day`",
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
        let categories = all_categories();
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
    }
}
