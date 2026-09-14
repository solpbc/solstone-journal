// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Closed MCP read-only tool registry table and dispatch.

use serde_json::{Value, json};
use solstone_core_mcp_audit::ToolName as AuditToolName;

use crate::jsonrpc::ToolName;
use crate::permissions::PermissionDecision;
use crate::tools::{ToolError, ValidatedTool, fetch, search};

/// One closed tool registry entry.
pub(crate) struct ToolEntry {
    pub(crate) tool_name: ToolName,
    pub(crate) wire_name: &'static str,
    pub(crate) audit_name: AuditToolName,
    #[allow(dead_code)]
    pub(crate) required_categories: &'static [&'static str],
    pub(crate) input_schema: fn() -> Value,
    pub(crate) validate: fn(Option<&Value>) -> Result<ValidatedTool, ToolError>,
}

pub(crate) const TOOLS: &[ToolEntry] = &[
    ToolEntry {
        tool_name: ToolName::Search,
        wire_name: "search",
        audit_name: AuditToolName::Search,
        required_categories: &["transcripts", "entities", "facets"],
        input_schema: search_input_schema,
        validate: |params| search::validate(params).map(ValidatedTool::Search),
    },
    ToolEntry {
        tool_name: ToolName::Fetch,
        wire_name: "fetch",
        audit_name: AuditToolName::Fetch,
        required_categories: &["transcripts", "entities", "facets"],
        input_schema: fetch_input_schema,
        validate: |params| fetch::validate(params).map(ValidatedTool::Fetch),
    },
];

fn search_input_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "query": { "type": "string", "minLength": 1 },
            "limit": { "type": "integer", "minimum": 1, "maximum": 100, "default": 10 },
            "offset": { "type": "integer", "minimum": 0, "maximum": 10000, "default": 0 },
            "day": { "type": "string" },
            "day_from": { "type": "string" },
            "day_to": { "type": "string" },
            "facet": { "type": "string" },
            "agent": { "type": "string" },
            "stream": { "type": "string" },
            "time_bucket": { "type": "string" },
            "relax": { "type": "boolean", "default": false },
            "counts": { "type": "boolean", "default": false },
            "order": { "type": "string", "enum": ["relevance", "recency"], "default": "relevance" }
        },
        "required": ["query"]
    })
}

fn fetch_input_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": { "id": { "type": "string", "minLength": 3 } },
        "required": ["id"]
    })
}

/// Find a tool entry by its wire name.
#[must_use]
pub(crate) fn find_tool_by_wire_name(wire_name: &str) -> Option<&'static ToolEntry> {
    TOOLS.iter().find(|t| t.wire_name == wire_name)
}

/// Find a tool entry by typed ToolName.
#[must_use]
pub(crate) fn find_tool(tool_name: ToolName) -> &'static ToolEntry {
    TOOLS
        .iter()
        .find(|t| t.tool_name == tool_name)
        .expect("closed tool registry entry exists")
}

/// Return the advertised tools list based on the connection's permission decision.
#[must_use]
pub(crate) fn advertised_tools_list(decision: &PermissionDecision) -> Value {
    match decision {
        PermissionDecision::Allowed => {
            let tools: Vec<Value> = TOOLS
                .iter()
                .map(|t| {
                    json!({
                        "name": t.wire_name,
                        "inputSchema": (t.input_schema)(),
                        "annotations": { "readOnlyHint": true }
                    })
                })
                .collect();
            json!({ "tools": tools })
        }
        PermissionDecision::Denied { .. } => json!({ "tools": [] }),
    }
}

#[cfg(all(test, not(feature = "full-tests")))]
mod tests {
    use super::*;

    #[test]
    fn registry_contains_only_search_and_fetch() {
        assert_eq!(TOOLS.len(), 2);
        assert_eq!(TOOLS[0].wire_name, "search");
        assert_eq!(TOOLS[1].wire_name, "fetch");
    }

    #[test]
    fn advertised_tools_list_filters_by_permission() {
        let allowed = advertised_tools_list(&PermissionDecision::Allowed);
        let allowed_tools = allowed.get("tools").unwrap().as_array().unwrap();
        assert_eq!(allowed_tools.len(), 2);

        let denied_no_perm = advertised_tools_list(&PermissionDecision::Denied {
            reason: "no_permission",
        });
        let denied_tools = denied_no_perm.get("tools").unwrap().as_array().unwrap();
        assert!(denied_tools.is_empty());

        let denied_unenforceable = advertised_tools_list(&PermissionDecision::Denied {
            reason: "unenforceable",
        });
        let unenforce_tools = denied_unenforceable
            .get("tools")
            .unwrap()
            .as_array()
            .unwrap();
        assert!(unenforce_tools.is_empty());
    }
}
