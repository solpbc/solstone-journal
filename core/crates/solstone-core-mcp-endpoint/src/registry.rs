// SPDX-License-Identifier: MIT

use serde_json::{Value, json};
use solstone_core_indexer_query::AdmittedCategory;

use crate::permissions::{ConnectionReadSnapshot, PermissionDecision};
use crate::tools::search::MAX_QUERY_BYTES;
use crate::tools::{MAX_DAY_BYTES, MAX_FACET_BYTES, MAX_OPAQUE_REFERENCE_BYTES};

/// The complete, closed MCP tool vocabulary.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ToolEntry {
    pub(crate) tool_name: crate::jsonrpc::ToolName,
    pub(crate) wire_name: &'static str,
    pub(crate) description: &'static str,
    pub(crate) input_schema: fn() -> Value,
    pub(crate) required_categories: &'static [AdmittedCategory],
    pub(crate) audit_name: solstone_core_mcp_audit::ToolName,
}

const TRANSCRIPTS: &[AdmittedCategory] = &[AdmittedCategory::Transcripts];
const ENTITIES: &[AdmittedCategory] = &[AdmittedCategory::Entities];

/// The single registry used for advertisement, dispatch, validation, and auditing.
pub(crate) const TOOLS: &[ToolEntry] = &[
    ToolEntry {
        tool_name: crate::jsonrpc::ToolName::ListFacets,
        wire_name: "list_facets",
        description: "List facets available to this connection.",
        input_schema: list_facets_input_schema,
        required_categories: &[],
        audit_name: solstone_core_mcp_audit::ToolName::ListFacets,
    },
    ToolEntry {
        tool_name: crate::jsonrpc::ToolName::Search,
        wire_name: "search",
        description: "Search indexed journal content available to this connection.",
        input_schema: search_input_schema,
        required_categories: TRANSCRIPTS,
        audit_name: solstone_core_mcp_audit::ToolName::Search,
    },
    ToolEntry {
        tool_name: crate::jsonrpc::ToolName::Fetch,
        wire_name: "fetch",
        description: "Fetch an indexed entry returned by search.",
        input_schema: fetch_input_schema,
        required_categories: TRANSCRIPTS,
        audit_name: solstone_core_mcp_audit::ToolName::Fetch,
    },
    ToolEntry {
        tool_name: crate::jsonrpc::ToolName::ListTranscripts,
        wire_name: "list_transcripts",
        description: "List transcript segments available to this connection.",
        input_schema: list_transcripts_input_schema,
        required_categories: TRANSCRIPTS,
        audit_name: solstone_core_mcp_audit::ToolName::ListTranscripts,
    },
    ToolEntry {
        tool_name: crate::jsonrpc::ToolName::GetTranscript,
        wire_name: "get_transcript",
        description: "Read approved transcript text from one segment.",
        input_schema: transcript_input_schema,
        required_categories: TRANSCRIPTS,
        audit_name: solstone_core_mcp_audit::ToolName::GetTranscript,
    },
    ToolEntry {
        tool_name: crate::jsonrpc::ToolName::ListEntities,
        wire_name: "list_entities",
        description: "List entities available to this connection.",
        input_schema: list_entities_input_schema,
        required_categories: ENTITIES,
        audit_name: solstone_core_mcp_audit::ToolName::ListEntities,
    },
    ToolEntry {
        tool_name: crate::jsonrpc::ToolName::GetEntity,
        wire_name: "get_entity",
        description: "Read an entity returned by list_entities.",
        input_schema: fetch_input_schema,
        required_categories: ENTITIES,
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
        .required_categories
        .iter()
        .all(|category| snapshot.categories.contains(category))
}

pub(crate) fn advertised_tools_list(permission: &PermissionDecision) -> Value {
    let PermissionDecision::Snapshot(snapshot) = permission else {
        return json!({ "tools": [] });
    };
    let tools = TOOLS
        .iter()
        .filter(|entry| snapshot_allows(snapshot, entry))
        .map(|entry| {
            json!({
                "name": entry.wire_name,
                "description": entry.description,
                "inputSchema": (entry.input_schema)(),
            })
        })
        .collect::<Vec<_>>();
    json!({ "tools": tools })
}

fn list_facets_input_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "limit": { "type": "integer", "minimum": 1, "maximum": 100 },
        }
    })
}

fn search_input_schema() -> Value {
    json!({
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
            "category": { "type": "string", "enum": ["transcripts", "entities", "facets"] },
            "facet": { "type": "string", "minLength": 1, "maxLength": MAX_FACET_BYTES },
        }
    })
}

fn fetch_input_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["reference"],
        "properties": {
            "reference": { "type": "string", "minLength": 1, "maxLength": MAX_OPAQUE_REFERENCE_BYTES },
        }
    })
}

fn list_transcripts_input_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "limit": { "type": "integer", "minimum": 1, "maximum": 100 },
            "day": { "type": "string", "minLength": 1, "maxLength": MAX_DAY_BYTES },
            "facet": { "type": "string", "minLength": 1, "maxLength": MAX_FACET_BYTES },
        }
    })
}

fn list_entities_input_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "limit": { "type": "integer", "minimum": 1, "maximum": 100 },
            "facet": { "type": "string", "minLength": 1, "maxLength": MAX_FACET_BYTES },
        }
    })
}

fn transcript_input_schema() -> Value {
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
    use crate::permissions::ConnectionReadSnapshot;
    use solstone_core_indexer_query::ConnectionScope;
    use std::collections::BTreeSet;

    #[test]
    fn empty_snapshot_only_advertises_facet_listing() {
        let snapshot = ConnectionReadSnapshot {
            categories: BTreeSet::new(),
            scope: ConnectionScope::WholeJournal,
            generation: 1,
        };

        let tools = advertised_tools_list(&PermissionDecision::Snapshot(snapshot));
        assert_eq!(tools["tools"].as_array().unwrap().len(), 1);
        assert_eq!(tools["tools"][0]["name"], "list_facets");
    }

    #[test]
    fn chosen_facet_advertisement_contains_no_journal_content() {
        let snapshot = ConnectionReadSnapshot {
            categories: [AdmittedCategory::Transcripts].into_iter().collect(),
            scope: ConnectionScope::ChosenFacets {
                ids: ["123e4567-e89b-42d3-a456-426614174000".to_owned()]
                    .into_iter()
                    .collect(),
            },
            generation: 1,
        };

        let serialized = advertised_tools_list(&PermissionDecision::Snapshot(snapshot)).to_string();
        assert!(!serialized.contains("Alpha"));
        assert!(!serialized.contains("20260914/default/090000_300"));
    }

    #[test]
    fn list_schemas_only_advertise_the_arguments_their_validators_accept() {
        for schema in [
            list_facets_input_schema(),
            list_transcripts_input_schema(),
            list_entities_input_schema(),
        ] {
            assert_eq!(schema["additionalProperties"], false);
            assert!(schema["properties"].get("cursor").is_none());
        }
        assert!(
            list_transcripts_input_schema()["properties"]
                .get("day")
                .is_some()
        );
        assert!(
            list_transcripts_input_schema()["properties"]
                .get("facet")
                .is_some()
        );
        assert!(
            list_entities_input_schema()["properties"]
                .get("facet")
                .is_some()
        );
        assert_eq!(
            search_input_schema()["properties"]["query"]["maxLength"],
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
        let opaque = "x".repeat(MAX_OPAQUE_REFERENCE_BYTES);
        assert_eq!(
            search_input_schema()["properties"]["cursor"]["maxLength"],
            MAX_OPAQUE_REFERENCE_BYTES
        );
        assert_eq!(
            fetch_input_schema()["properties"]["reference"]["maxLength"],
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
