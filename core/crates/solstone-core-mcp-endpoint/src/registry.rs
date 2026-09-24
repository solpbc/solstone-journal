// SPDX-License-Identifier: MIT

use std::collections::BTreeSet;
use std::path::Path;

use serde_json::{Value, json};
use solstone_core_indexer_query::AdmittedCategory;

use crate::permissions::{ConnectionReadSnapshot, PermissionDecision};
use crate::tools::search::MAX_QUERY_BYTES;
use crate::tools::{MAX_DAY_BYTES, MAX_OPAQUE_REFERENCE_BYTES};

/// What one tool needs before a connection may see or call it.
///
/// ✅ `read` is a **named half**, not the whole requirement: a `write` half is
/// an addition beside it, and no existing reader has to be taught that what it
/// treated as the requirement was only part of one. ⛔ There is no write
/// variant, stub or dead branch here — the shape is the whole deliverable, the
/// same discipline increment A applied to `ConnectionPermission::read`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ToolRequirement {
    pub(crate) read: &'static [AdmittedCategory],
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
    pub(crate) description: &'static str,
    pub(crate) input_schema: fn(&SchemaContext<'_>) -> Value,
    pub(crate) requires: ToolRequirement,
    pub(crate) audit_name: solstone_core_mcp_audit::ToolName,
}

const NOTHING: ToolRequirement = ToolRequirement { read: &[] };
const TRANSCRIPTS: ToolRequirement = ToolRequirement {
    read: &[AdmittedCategory::Transcripts],
};
const ENTITIES: ToolRequirement = ToolRequirement {
    read: &[AdmittedCategory::Entities],
};

/// The single registry used for advertisement, dispatch, validation, and auditing.
pub(crate) const TOOLS: &[ToolEntry] = &[
    ToolEntry {
        tool_name: crate::jsonrpc::ToolName::ListFacets,
        wire_name: "list_facets",
        description: "List facets available to this connection.",
        input_schema: list_facets_input_schema,
        requires: NOTHING,
        audit_name: solstone_core_mcp_audit::ToolName::ListFacets,
    },
    ToolEntry {
        tool_name: crate::jsonrpc::ToolName::Search,
        wire_name: "search",
        description: "Search indexed journal content available to this connection.",
        input_schema: search_input_schema,
        requires: TRANSCRIPTS,
        audit_name: solstone_core_mcp_audit::ToolName::Search,
    },
    ToolEntry {
        tool_name: crate::jsonrpc::ToolName::Fetch,
        wire_name: "fetch",
        description: "Fetch an indexed entry returned by search.",
        input_schema: fetch_input_schema,
        requires: TRANSCRIPTS,
        audit_name: solstone_core_mcp_audit::ToolName::Fetch,
    },
    ToolEntry {
        tool_name: crate::jsonrpc::ToolName::ListTranscripts,
        wire_name: "list_transcripts",
        description: "List transcript segments available to this connection.",
        input_schema: list_transcripts_input_schema,
        requires: TRANSCRIPTS,
        audit_name: solstone_core_mcp_audit::ToolName::ListTranscripts,
    },
    ToolEntry {
        tool_name: crate::jsonrpc::ToolName::GetTranscript,
        wire_name: "get_transcript",
        description: "Read approved transcript text from one segment.",
        input_schema: transcript_input_schema,
        requires: TRANSCRIPTS,
        audit_name: solstone_core_mcp_audit::ToolName::GetTranscript,
    },
    ToolEntry {
        tool_name: crate::jsonrpc::ToolName::ListEntities,
        wire_name: "list_entities",
        description: "List entities available to this connection.",
        input_schema: list_entities_input_schema,
        requires: ENTITIES,
        audit_name: solstone_core_mcp_audit::ToolName::ListEntities,
    },
    ToolEntry {
        tool_name: crate::jsonrpc::ToolName::GetEntity,
        wire_name: "get_entity",
        description: "Read an entity returned by list_entities.",
        input_schema: fetch_input_schema,
        requires: ENTITIES,
        audit_name: solstone_core_mcp_audit::ToolName::GetEntity,
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
    entry
        .requires
        .read
        .iter()
        .all(|category| snapshot.categories.contains(category))
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
    let PermissionDecision::Snapshot(snapshot) = permission else {
        return json!({ "tools": [] });
    };
    let facet_ids = crate::dispatch::available_facets(journal_root, snapshot)
        .map(|facets| {
            facets
                .into_iter()
                .map(|(id, _, _)| id)
                .collect::<Vec<String>>()
        })
        .unwrap_or_default();
    let context = SchemaContext {
        categories: &snapshot.categories,
        facet_ids: &facet_ids,
    };
    let tools = TOOLS
        .iter()
        .filter(|entry| snapshot_allows(snapshot, entry))
        .map(|entry| {
            json!({
                "name": entry.wire_name,
                "description": entry.description,
                "inputSchema": (entry.input_schema)(&context),
                // Every tool in the closed registry reads and none writes.
                "annotations": { "readOnlyHint": true },
            })
        })
        .collect::<Vec<_>>();
    json!({ "tools": tools })
}

pub(crate) fn category_token(category: &AdmittedCategory) -> &'static str {
    match category {
        AdmittedCategory::Transcripts => "transcripts",
        AdmittedCategory::Entities => "entities",
        AdmittedCategory::Facets => "facets",
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
        }
    })
}

fn search_input_schema(context: &SchemaContext<'_>) -> Value {
    let mut schema = json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["query"],
        "properties": {
            "query": { "type": "string", "minLength": 1, "maxLength": MAX_QUERY_BYTES },
            "limit": { "type": "integer", "minimum": 1, "maximum": 100 },
            "cursor": { "type": "string", "minLength": 1, "maxLength": MAX_OPAQUE_REFERENCE_BYTES },
            "day": { "type": "string", "minLength": 1, "maxLength": MAX_DAY_BYTES },
            "day_from": { "type": "string", "minLength": 1, "maxLength": MAX_DAY_BYTES },
            "day_to": { "type": "string", "minLength": 1, "maxLength": MAX_DAY_BYTES },
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
            "day": { "type": "string", "minLength": 1, "maxLength": MAX_DAY_BYTES },
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
            .tempdir_in("/var/tmp")
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
        ]
        .into_iter()
        .collect()
    }

    #[test]
    fn empty_snapshot_only_advertises_facet_listing() {
        let journal = journal_with_two_facets();
        let snapshot = ConnectionReadSnapshot {
            categories: BTreeSet::new(),
            scope: ConnectionScope::WholeJournal,
            generation: 1,
        };

        let tools = advertised_tools_list(journal.path(), &PermissionDecision::Snapshot(snapshot));
        assert_eq!(tools["tools"].as_array().unwrap().len(), 1);
        assert_eq!(tools["tools"][0]["name"], "list_facets");
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
            .tempdir_in("/var/tmp")
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
    fn every_advertised_tool_is_marked_read_only() {
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
                tool["annotations"]["readOnlyHint"], true,
                "{} is not marked read-only",
                tool["name"]
            );
        }
    }

    #[test]
    fn list_schemas_only_advertise_the_arguments_their_validators_accept() {
        let categories = all_categories();
        let facet_ids = vec![FACET_A.to_owned()];
        let context = context(&categories, &facet_ids);
        for schema in [
            list_facets_input_schema(&context),
            list_transcripts_input_schema(&context),
            list_entities_input_schema(&context),
        ] {
            assert_eq!(schema["additionalProperties"], false);
            assert!(schema["properties"].get("cursor").is_none());
        }
        assert!(
            list_transcripts_input_schema(&context)["properties"]
                .get("day")
                .is_some()
        );
        assert!(
            list_transcripts_input_schema(&context)["properties"]
                .get("facet")
                .is_some()
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
        assert!(crate::tools::facets::validate(Some(&json!({"cursor": "nope"}))).is_err());
        assert!(
            crate::tools::transcripts::validate_list(Some(&json!({"cursor": "nope"}))).is_err()
        );
        assert!(crate::tools::entities::validate_list(Some(&json!({"cursor": "nope"}))).is_err());
    }

    #[test]
    fn opaque_schema_bounds_match_the_validators() {
        let categories = all_categories();
        let facet_ids = vec![FACET_A.to_owned()];
        let context = context(&categories, &facet_ids);
        let opaque = "x".repeat(MAX_OPAQUE_REFERENCE_BYTES);
        assert_eq!(
            search_input_schema(&context)["properties"]["cursor"]["maxLength"],
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
            crate::tools::entities::validate_list(Some(&json!({
                "facet": "x".repeat(MAX_FACET_BYTES + 1)
            })))
            .is_err()
        );
    }
}
