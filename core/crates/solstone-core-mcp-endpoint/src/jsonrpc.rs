// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Minimal JSON-RPC 2.0 envelopes for the read-only MCP surface.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// A validated JSON-RPC 2.0 request envelope.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct JsonRpcRequest {
    pub(crate) jsonrpc: String,
    #[serde(default)]
    pub(crate) id: Option<Value>,
    pub(crate) method: String,
    #[serde(default)]
    pub(crate) params: Option<Value>,
}

/// A JSON-RPC response with exactly one success or error payload.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct JsonRpcResponse {
    jsonrpc: &'static str,
    id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonRpcErrorObject>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
struct JsonRpcErrorObject {
    code: i32,
    message: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<Value>,
}

type JsonRpcFailure = Box<JsonRpcResponse>;

/// An admitted MCP method after envelope and parameter validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum McpMethod {
    Initialize,
    ToolsList,
    ToolsCall(ToolName),
}

/// The closed read-only tool registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ToolName {
    ListFacets,
    Search,
    Fetch,
    ListTranscripts,
    GetTranscript,
    ListEntities,
    GetEntity,
}

impl JsonRpcResponse {
    pub(crate) fn success(id: Option<&Value>, result: Value) -> Self {
        Self {
            jsonrpc: "2.0",
            id: response_id(id),
            result: Some(result),
            error: None,
        }
    }

    pub(crate) fn parse_error() -> Self {
        Self::error(None, -32700, "parse error")
    }

    pub(crate) fn invalid_request(id: Option<&Value>) -> Self {
        Self::error(id, -32600, "invalid request")
    }

    pub(crate) fn method_not_found(id: Option<&Value>) -> Self {
        Self::error(id, -32601, "method not found")
    }

    pub(crate) fn invalid_params(id: Option<&Value>) -> Self {
        Self::error(id, -32602, "invalid params")
    }

    pub(crate) fn internal_error(id: Option<&Value>, message: &'static str) -> Self {
        Self::error(id, -32603, message)
    }

    pub(crate) fn tool_not_found(id: Option<&Value>) -> Self {
        Self::error(id, -32601, "tool not found")
    }

    pub(crate) fn tool_error(id: Option<&Value>, reason: &'static str) -> Self {
        Self::error_with_data(
            id,
            -32000,
            "tool execution failed",
            json!({ "reason": reason }),
        )
    }

    pub(crate) fn permission_denied(id: Option<&Value>, reason: &'static str) -> Self {
        Self::error_with_data(id, -32001, "Access denied", json!({ "reason": reason }))
    }

    fn error(id: Option<&Value>, code: i32, message: &'static str) -> Self {
        Self::error_with_optional_data(id, code, message, None)
    }

    fn error_with_data(id: Option<&Value>, code: i32, message: &'static str, data: Value) -> Self {
        Self::error_with_optional_data(id, code, message, Some(data))
    }

    fn error_with_optional_data(
        id: Option<&Value>,
        code: i32,
        message: &'static str,
        data: Option<Value>,
    ) -> Self {
        Self {
            jsonrpc: "2.0",
            id: response_id(id),
            result: None,
            error: Some(JsonRpcErrorObject {
                code,
                message,
                data,
            }),
        }
    }

    /// Mark a response whose client is holding a tool list that no longer
    /// describes its grant.
    ///
    /// 🔑 This is consequence 2 designed in rather than bolted on: `tools/list`
    /// became stateful the moment a schema named a connection's facets, so the
    /// signal rides the response the client is already reading. ✅ `_meta` on a
    /// success is MCP's own out-of-band channel and leaves `structuredContent`
    /// untouched; on an error it joins `data` beside the closed refusal reason.
    /// ⚠ When a server→client stream exists, `notifications/tools/list_changed`
    /// becomes the second consumer of the same session state — ⛔ not a second
    /// mechanism.
    pub(crate) fn with_tools_list_changed(mut self) -> Self {
        if let Some(result) = self.result.as_mut().and_then(Value::as_object_mut) {
            result.insert("_meta".to_owned(), json!({ "tools_list_changed": true }));
        }
        if let Some(error) = self.error.as_mut() {
            match error.data.as_mut().and_then(Value::as_object_mut) {
                Some(data) => {
                    data.insert("tools_list_changed".to_owned(), Value::Bool(true));
                }
                None => error.data = Some(json!({ "tools_list_changed": true })),
            }
        }
        self
    }

    pub(crate) fn to_bytes(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec(self)
    }
}

/// Parse an envelope, preserving the standard parse-versus-invalid distinction.
pub(crate) fn parse_request(body: &[u8]) -> Result<JsonRpcRequest, JsonRpcFailure> {
    let value = serde_json::from_slice::<Value>(body)
        .map_err(|_| Box::new(JsonRpcResponse::parse_error()))?;
    let request = serde_json::from_value::<JsonRpcRequest>(value)
        .map_err(|_| Box::new(JsonRpcResponse::invalid_request(None)))?;
    if request.jsonrpc != "2.0" || !valid_id(request.id.as_ref()) {
        return Err(Box::new(JsonRpcResponse::invalid_request(
            request.id.as_ref(),
        )));
    }
    Ok(request)
}

/// Classify a known method and its shape without executing any journal tool.
pub(crate) fn classify_method(request: &JsonRpcRequest) -> Result<McpMethod, JsonRpcFailure> {
    match request.method.as_str() {
        "initialize" => Ok(McpMethod::Initialize),
        "tools/list" => Ok(McpMethod::ToolsList),
        "tools/call" => classify_tool_call(request),
        _ => Err(Box::new(JsonRpcResponse::method_not_found(
            request.id.as_ref(),
        ))),
    }
}

pub(crate) fn initialize_result() -> Value {
    json!({
        "protocolVersion": "2025-03-26",
        "capabilities": { "tools": {} },
        "serverInfo": { "name": "solstone-journal", "version": "2.0" }
    })
}

/// Render a successful tool result in the MCP `CallToolResult` shape.
///
/// `content` is required by MCP clients, including when the machine-readable
/// response is also available in `structuredContent`. Keeping both forms lets
/// a client display the response while consuming its typed JSON without
/// inferring a response type from a journal-specific object.
pub(crate) fn tool_result(value: Value) -> Value {
    let text = value.to_string();
    json!({
        "content": [{ "type": "text", "text": text }],
        "structuredContent": value,
    })
}

fn classify_tool_call(request: &JsonRpcRequest) -> Result<McpMethod, JsonRpcFailure> {
    let Some(Value::Object(params)) = request.params.as_ref() else {
        return Err(Box::new(JsonRpcResponse::invalid_params(
            request.id.as_ref(),
        )));
    };
    let Some(Value::String(name)) = params.get("name") else {
        return Err(Box::new(JsonRpcResponse::invalid_params(
            request.id.as_ref(),
        )));
    };
    if let Some(entry) = crate::registry::tool_by_wire_name(name) {
        Ok(McpMethod::ToolsCall(entry.tool_name))
    } else {
        Err(Box::new(JsonRpcResponse::tool_not_found(
            request.id.as_ref(),
        )))
    }
}

/// Borrow the MCP tool argument object, leaving its schema to the named tool.
pub(crate) fn tool_arguments(request: &JsonRpcRequest) -> Option<&Value> {
    request
        .params
        .as_ref()
        .and_then(Value::as_object)
        .and_then(|params| params.get("arguments"))
}

fn valid_id(id: Option<&Value>) -> bool {
    id.is_none_or(|value| value.is_null() || value.is_string() || value.is_number())
}

fn response_id(id: Option<&Value>) -> Value {
    id.cloned().unwrap_or(Value::Null)
}

#[cfg(all(test, not(feature = "full-tests")))]
mod tests {
    use serde_json::{Value, json};

    use super::{
        JsonRpcResponse, McpMethod, ToolName, classify_method, initialize_result, parse_request,
        tool_result,
    };
    use std::collections::BTreeSet;

    use crate::permissions::{ConnectionReadSnapshot, PermissionDecision};
    use crate::registry::advertised_tools_list;
    use solstone_core_indexer_query::ConnectionScope;

    fn error_code(response: &JsonRpcResponse) -> i32 {
        serde_json::from_slice::<Value>(&response.to_bytes().unwrap()).unwrap()["error"]["code"]
            .as_i64()
            .unwrap() as i32
    }

    #[test]
    fn valid_mcp_envelopes_parse_and_classify() {
        let initialize =
            parse_request(br#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#).unwrap();
        assert_eq!(classify_method(&initialize), Ok(McpMethod::Initialize));
        let list = parse_request(br#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#).unwrap();
        assert_eq!(classify_method(&list), Ok(McpMethod::ToolsList));
        let call = parse_request(
            br#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"fetch"}}"#,
        )
        .unwrap();
        assert_eq!(
            classify_method(&call),
            Ok(McpMethod::ToolsCall(ToolName::Fetch))
        );
    }

    #[test]
    fn malformed_invalid_and_unknown_requests_use_standard_errors() {
        assert_eq!(error_code(&parse_request(b"{").unwrap_err()), -32700);
        assert_eq!(
            error_code(
                &parse_request(br#"{"jsonrpc":"1.0","id":1,"method":"initialize"}"#).unwrap_err()
            ),
            -32600
        );
        assert_eq!(
            error_code(&parse_request(br#"{"jsonrpc":"2.0","id":1}"#).unwrap_err()),
            -32600
        );
        let unknown = parse_request(br#"{"jsonrpc":"2.0","id":1,"method":"other"}"#).unwrap();
        assert_eq!(error_code(&classify_method(&unknown).unwrap_err()), -32601);
    }

    #[test]
    fn tool_registry_is_a_closed_scoped_vocabulary() {
        let journal = tempfile::Builder::new()
            .prefix("solstone-mcp-jsonrpc-")
            .tempdir_in("/var/tmp")
            .unwrap();
        let result = advertised_tools_list(
            journal.path(),
            &PermissionDecision::Snapshot(ConnectionReadSnapshot {
                categories: [
                    solstone_core_indexer_query::AdmittedCategory::Transcripts,
                    solstone_core_indexer_query::AdmittedCategory::Entities,
                    solstone_core_indexer_query::AdmittedCategory::Facets,
                ]
                .into_iter()
                .collect::<BTreeSet<_>>(),
                scope: ConnectionScope::WholeJournal,
                generation: 1,
            }),
        );
        let names = result["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["name"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            [
                "list_facets",
                "search",
                "fetch",
                "list_transcripts",
                "get_transcript",
                "list_entities",
                "get_entity"
            ]
        );
        assert!(result.to_string().contains("additionalProperties"));
        let known = parse_request(
            br#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"search"}}"#,
        )
        .unwrap();
        assert!(matches!(
            classify_method(&known),
            Ok(McpMethod::ToolsCall(ToolName::Search))
        ));
        let unknown = parse_request(
            br#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"write"}}"#,
        )
        .unwrap();
        let response = classify_method(&unknown).unwrap_err();
        assert_eq!(error_code(&response), -32601);
        assert!(
            String::from_utf8(response.to_bytes().unwrap())
                .unwrap()
                .contains("tool not found")
        );
        assert_eq!(
            JsonRpcResponse::internal_error(known.id.as_ref(), "tool execution is not implemented")
                .to_bytes()
                .unwrap(),
            serde_json::to_vec(&json!({
                "jsonrpc": "2.0",
                "id": 1,
                "error": { "code": -32603, "message": "tool execution is not implemented" }
            }))
            .unwrap()
        );
        assert_eq!(initialize_result()["capabilities"], json!({ "tools": {} }));
    }

    #[test]
    fn prepared_value_is_the_single_source_for_both_tool_result_representations() {
        let mut prepared = json!({"kept": "value", "drop_before_release": true});
        prepared
            .as_object_mut()
            .unwrap()
            .remove("drop_before_release");

        let rendered = tool_result(prepared);
        let content =
            serde_json::from_str::<Value>(rendered["content"][0]["text"].as_str().unwrap())
                .unwrap();
        assert_eq!(content, rendered["structuredContent"]);
        assert_eq!(content["kept"], "value");
        assert!(content.get("drop_before_release").is_none());
        assert!(
            rendered["structuredContent"]
                .get("drop_before_release")
                .is_none()
        );
    }
}
