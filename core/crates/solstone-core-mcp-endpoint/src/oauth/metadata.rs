// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! OAuth metadata documents for the journal-local MCP resource.

use crate::http1::HttpResponse;

/// RFC 9728 protected-resource metadata for `{resource_origin}/mcp`.
pub(crate) fn protected_resource(resource_origin: &str) -> HttpResponse {
    json_no_store(serde_json::json!({
        "resource": format!("{resource_origin}/mcp"),
        "authorization_servers": [resource_origin],
        "bearer_methods_supported": ["header"],
    }))
}

/// RFC 8414 authorization-server metadata for `{resource_origin}`.
pub(crate) fn authorization_server(resource_origin: &str) -> HttpResponse {
    json_no_store(serde_json::json!({
        "issuer": resource_origin,
        "authorization_endpoint": format!("{resource_origin}/authorize"),
        "token_endpoint": format!("{resource_origin}/token"),
        "registration_endpoint": format!("{resource_origin}/register"),
        "response_types_supported": ["code"],
        "grant_types_supported": ["authorization_code", "refresh_token"],
        "code_challenge_methods_supported": ["S256"],
        "token_endpoint_auth_methods_supported": ["none"],
        "authorization_response_iss_parameter_supported": true,
    }))
}

fn json_no_store(value: serde_json::Value) -> HttpResponse {
    HttpResponse::json(
        200,
        "OK",
        serde_json::to_vec(&value).expect("OAuth metadata JSON serializes"),
    )
    .with_header("Cache-Control", "no-store".to_owned())
}

#[cfg(all(test, not(feature = "full-tests")))]
mod tests {
    use super::{authorization_server, protected_resource};

    const ORIGIN: &str = "https://mcp.test";

    fn header<'a>(response: &'a crate::http1::HttpResponse, name: &str) -> Option<&'a str> {
        response
            .extra_headers
            .iter()
            .find(|(candidate, _)| candidate.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    fn json_body(response: &crate::http1::HttpResponse) -> serde_json::Value {
        serde_json::from_slice(&response.body).expect("metadata body is JSON")
    }

    #[test]
    fn protected_resource_body_and_headers_match_the_contract() {
        let response = protected_resource(ORIGIN);
        assert_eq!(response.status, 200);
        assert_eq!(response.content_type, Some("application/json"));
        assert_eq!(header(&response, "Cache-Control"), Some("no-store"));
        assert_eq!(
            json_body(&response),
            serde_json::json!({
                "resource": "https://mcp.test/mcp",
                "authorization_servers": ["https://mcp.test"],
                "bearer_methods_supported": ["header"],
            })
        );
    }

    #[test]
    fn authorization_server_body_and_headers_match_the_contract() {
        let response = authorization_server(ORIGIN);
        assert_eq!(response.status, 200);
        assert_eq!(response.content_type, Some("application/json"));
        assert_eq!(header(&response, "Cache-Control"), Some("no-store"));
        assert_eq!(
            json_body(&response),
            serde_json::json!({
                "issuer": "https://mcp.test",
                "authorization_endpoint": "https://mcp.test/authorize",
                "token_endpoint": "https://mcp.test/token",
                "registration_endpoint": "https://mcp.test/register",
                "response_types_supported": ["code"],
                "grant_types_supported": ["authorization_code", "refresh_token"],
                "code_challenge_methods_supported": ["S256"],
                "token_endpoint_auth_methods_supported": ["none"],
                "authorization_response_iss_parameter_supported": true,
            })
        );
    }
}
