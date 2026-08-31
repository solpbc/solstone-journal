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
    Search,
    Fetch,
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

pub(crate) fn tools_list_result() -> Value {
    json!({
        "tools": [
            { "name": "search", "annotations": { "readOnlyHint": true } },
            { "name": "fetch", "annotations": { "readOnlyHint": true } }
        ]
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
    match name.as_str() {
        "search" => Ok(McpMethod::ToolsCall(ToolName::Search)),
        "fetch" => Ok(McpMethod::ToolsCall(ToolName::Fetch)),
        _ => Err(Box::new(JsonRpcResponse::tool_not_found(
            request.id.as_ref(),
        ))),
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
        tools_list_result,
    };

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
    fn tool_registry_is_exactly_the_two_read_only_tools() {
        let result = tools_list_result();
        assert_eq!(
            result,
            json!({
                "tools": [
                    { "name": "search", "annotations": { "readOnlyHint": true } },
                    { "name": "fetch", "annotations": { "readOnlyHint": true } }
                ]
            })
        );
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
}
