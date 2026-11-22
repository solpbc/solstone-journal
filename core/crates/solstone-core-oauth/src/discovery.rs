// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! RFC 9728 Protected Resource Metadata and RFC 8414 Authorization Server Metadata
//! discovery over DNS-rebinding-proof SSRF-safe HTTPS fetch.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::ssrf::{HttpRequest, OAuthHttpClient, SsrfFetchError, validate_https_url};

/// RFC 9728 OAuth 2.0 Protected Resource Metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtectedResourceMetadata {
    pub resource: String,
    #[serde(default)]
    pub authorization_servers: Vec<String>,
    #[serde(default)]
    pub scopes_supported: Vec<String>,
    #[serde(default)]
    pub bearer_methods_supported: Vec<String>,
}

/// RFC 8414 Authorization Server Metadata (extended with CIMD, RFC 9207 `iss`,
/// RFC 8628 Device Code, RFC 9126 PAR, and RFC 9449 DPoP fields).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthorizationServerMetadata {
    pub issuer: String,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    #[serde(default)]
    pub registration_endpoint: Option<String>,
    #[serde(default)]
    pub revocation_endpoint: Option<String>,
    #[serde(default)]
    pub pushed_authorization_request_endpoint: Option<String>,
    #[serde(default)]
    pub device_authorization_endpoint: Option<String>,
    #[serde(default)]
    pub client_id_metadata_document_supported: bool,
    #[serde(default)]
    pub authorization_response_iss_parameter_supported: bool,
    #[serde(default)]
    pub require_pushed_authorization_requests: bool,
    #[serde(default)]
    pub code_challenge_methods_supported: Vec<String>,
    #[serde(default)]
    pub grant_types_supported: Vec<String>,
    #[serde(default)]
    pub response_types_supported: Vec<String>,
    #[serde(default)]
    pub token_endpoint_auth_methods_supported: Vec<String>,
    #[serde(default)]
    pub dpop_signing_alg_values_supported: Vec<String>,
    #[serde(default)]
    pub scopes_supported: Vec<String>,
}

/// Errors from RFC 9728 / RFC 8414 discovery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiscoveryError {
    InvalidUrl,
    Fetch(SsrfFetchError),
    NotFound,
    HttpStatus(u16),
    InvalidJson,
    ResourceMismatch,
    IssuerMismatch,
    InvalidEndpointUrl(&'static str),
    MissingAuthorizationServer,
    PkceS256Unsupported,
}

impl fmt::Display for DiscoveryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidUrl => formatter.write_str("invalid HTTPS discovery URL"),
            Self::Fetch(err) => err.fmt(formatter),
            Self::NotFound => formatter.write_str("OAuth discovery metadata not found"),
            Self::HttpStatus(code) => {
                write!(formatter, "OAuth discovery returned HTTP status {code}")
            }
            Self::InvalidJson => formatter.write_str("OAuth discovery returned invalid JSON"),
            Self::ResourceMismatch => formatter
                .write_str("protected resource metadata resource did not match requested URL"),
            Self::IssuerMismatch => formatter
                .write_str("authorization server metadata issuer did not match requested issuer"),
            Self::InvalidEndpointUrl(field) => {
                write!(
                    formatter,
                    "authorization server metadata field `{field}` is not a valid HTTPS URL"
                )
            }
            Self::MissingAuthorizationServer => formatter
                .write_str("protected resource metadata advertised no authorization_servers"),
            Self::PkceS256Unsupported => {
                formatter.write_str("authorization server does not advertise PKCE S256 support")
            }
        }
    }
}

impl std::error::Error for DiscoveryError {}

/// Compute RFC 9728 § 3.1 Protected Resource Metadata discovery URLs in lookup order.
pub fn protected_resource_metadata_urls(resource_url: &str) -> Result<Vec<String>, DiscoveryError> {
    let parsed = validate_https_url(resource_url).map_err(|_| DiscoveryError::InvalidUrl)?;
    let origin = parsed.origin();
    let trimmed_path = parsed.path.trim_end_matches('/');
    if trimmed_path.is_empty() {
        Ok(vec![format!(
            "{origin}/.well-known/oauth-protected-resource"
        )])
    } else {
        Ok(vec![
            format!("{origin}/.well-known/oauth-protected-resource{trimmed_path}"),
            format!("{origin}/.well-known/oauth-protected-resource"),
        ])
    }
}

/// Compute RFC 8414 § 3.1 (+ OIDC Discovery fallback) Authorization Server Metadata URLs.
pub fn authorization_server_metadata_urls(issuer: &str) -> Result<Vec<String>, DiscoveryError> {
    let parsed = validate_https_url(issuer).map_err(|_| DiscoveryError::InvalidUrl)?;
    let origin = parsed.origin();
    let trimmed_path = parsed.path.trim_end_matches('/');
    if trimmed_path.is_empty() {
        Ok(vec![
            format!("{origin}/.well-known/oauth-authorization-server"),
            format!("{origin}/.well-known/openid-configuration"),
        ])
    } else {
        Ok(vec![
            format!("{origin}/.well-known/oauth-authorization-server{trimmed_path}"),
            format!("{origin}/.well-known/openid-configuration{trimmed_path}"),
            format!("{origin}{trimmed_path}/.well-known/openid-configuration"),
        ])
    }
}

/// Fetch and validate RFC 9728 Protected Resource Metadata for `resource_url`.
pub fn discover_protected_resource(
    http: &dyn OAuthHttpClient,
    resource_url: &str,
) -> Result<ProtectedResourceMetadata, DiscoveryError> {
    let candidates = protected_resource_metadata_urls(resource_url)?;
    for url in candidates {
        let resp = http
            .send(&HttpRequest::get(url))
            .map_err(DiscoveryError::Fetch)?;
        if resp.status == 404 {
            continue;
        }
        if resp.status != 200 {
            return Err(DiscoveryError::HttpStatus(resp.status));
        }
        let meta: ProtectedResourceMetadata =
            serde_json::from_slice(&resp.body).map_err(|_| DiscoveryError::InvalidJson)?;
        if !urls_match_canonical_root(&meta.resource, resource_url) {
            return Err(DiscoveryError::ResourceMismatch);
        }
        if meta.authorization_servers.is_empty() {
            return Err(DiscoveryError::MissingAuthorizationServer);
        }
        for server in &meta.authorization_servers {
            validate_https_url(server)
                .map_err(|_| DiscoveryError::InvalidEndpointUrl("authorization_servers"))?;
        }
        return Ok(meta);
    }
    Err(DiscoveryError::NotFound)
}

/// Fetch and validate RFC 8414 Authorization Server Metadata for `issuer`.
pub fn discover_authorization_server(
    http: &dyn OAuthHttpClient,
    issuer: &str,
) -> Result<AuthorizationServerMetadata, DiscoveryError> {
    let candidates = authorization_server_metadata_urls(issuer)?;
    for url in candidates {
        let resp = http
            .send(&HttpRequest::get(url))
            .map_err(DiscoveryError::Fetch)?;
        if resp.status == 404 {
            continue;
        }
        if resp.status != 200 {
            return Err(DiscoveryError::HttpStatus(resp.status));
        }
        let meta: AuthorizationServerMetadata =
            serde_json::from_slice(&resp.body).map_err(|_| DiscoveryError::InvalidJson)?;
        validate_authorization_server_metadata(&meta, issuer)?;
        return Ok(meta);
    }
    Err(DiscoveryError::NotFound)
}

/// Discover both RFC 9728 Protected Resource Metadata and its primary RFC 8414
/// Authorization Server Metadata for a remote MCP or resource server URL.
pub fn discover_resource_and_auth_server(
    http: &dyn OAuthHttpClient,
    resource_url: &str,
) -> Result<(ProtectedResourceMetadata, AuthorizationServerMetadata), DiscoveryError> {
    let prm = discover_protected_resource(http, resource_url)?;
    let issuer = prm
        .authorization_servers
        .first()
        .ok_or(DiscoveryError::MissingAuthorizationServer)?;
    let as_meta = discover_authorization_server(http, issuer)?;
    Ok((prm, as_meta))
}

fn validate_authorization_server_metadata(
    meta: &AuthorizationServerMetadata,
    expected_issuer: &str,
) -> Result<(), DiscoveryError> {
    validate_https_url(&meta.issuer).map_err(|_| DiscoveryError::InvalidEndpointUrl("issuer"))?;
    if !urls_match_canonical_root(&meta.issuer, expected_issuer) {
        return Err(DiscoveryError::IssuerMismatch);
    }
    validate_https_url(&meta.authorization_endpoint)
        .map_err(|_| DiscoveryError::InvalidEndpointUrl("authorization_endpoint"))?;
    validate_https_url(&meta.token_endpoint)
        .map_err(|_| DiscoveryError::InvalidEndpointUrl("token_endpoint"))?;
    for (field, maybe_url) in [
        (
            "registration_endpoint",
            meta.registration_endpoint.as_deref(),
        ),
        ("revocation_endpoint", meta.revocation_endpoint.as_deref()),
        (
            "pushed_authorization_request_endpoint",
            meta.pushed_authorization_request_endpoint.as_deref(),
        ),
        (
            "device_authorization_endpoint",
            meta.device_authorization_endpoint.as_deref(),
        ),
    ] {
        if let Some(url) = maybe_url {
            validate_https_url(url).map_err(|_| DiscoveryError::InvalidEndpointUrl(field))?;
        }
    }
    if !meta.code_challenge_methods_supported.is_empty()
        && !meta
            .code_challenge_methods_supported
            .iter()
            .any(|method| method == "S256")
    {
        return Err(DiscoveryError::PkceS256Unsupported);
    }
    Ok(())
}

/// Compare two HTTPS URLs allowing an optional trailing `/` on origin-only URLs
/// (e.g. `https://mcp.plaud.ai/` and `https://mcp.plaud.ai`), while requiring exact match
/// on any non-root path.
fn urls_match_canonical_root(left: &str, right: &str) -> bool {
    if left == right {
        return true;
    }
    let (Ok(l), Ok(r)) = (validate_https_url(left), validate_https_url(right)) else {
        return false;
    };
    l.host == r.host && l.path == r.path && l.query == r.query
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use crate::ssrf::HttpResponse;

    use super::*;

    struct MapHttp {
        routes: BTreeMap<String, Result<HttpResponse, SsrfFetchError>>,
    }

    impl OAuthHttpClient for MapHttp {
        fn send(&self, request: &HttpRequest) -> Result<HttpResponse, SsrfFetchError> {
            match self.routes.get(&request.url) {
                Some(Ok(resp)) => Ok(resp.clone()),
                Some(Err(err)) => Err(*err),
                None => Ok(HttpResponse {
                    status: 404,
                    headers: Vec::new(),
                    body: Vec::new(),
                }),
            }
        }
    }

    fn json_ok(body: &str) -> Result<HttpResponse, SsrfFetchError> {
        Ok(HttpResponse {
            status: 200,
            headers: vec![("content-type".to_owned(), "application/json".to_owned())],
            body: body.as_bytes().to_vec(),
        })
    }

    #[test]
    fn prm_and_as_discovery_follow_rfc9728_and_rfc8414_rules() {
        let mut routes = BTreeMap::new();
        routes.insert(
            "https://mcp.plaud.ai/.well-known/oauth-protected-resource/mcp".to_owned(),
            json_ok(
                r#"{"resource":"https://mcp.plaud.ai/mcp","authorization_servers":["https://mcp.plaud.ai/"],"bearer_methods_supported":["header"]}"#,
            ),
        );
        routes.insert(
            "https://mcp.plaud.ai/.well-known/oauth-authorization-server".to_owned(),
            json_ok(
                r#"{
                    "issuer":"https://mcp.plaud.ai",
                    "authorization_endpoint":"https://mcp.plaud.ai/authorize",
                    "token_endpoint":"https://mcp.plaud.ai/token",
                    "registration_endpoint":"https://mcp.plaud.ai/register",
                    "code_challenge_methods_supported":["S256"],
                    "grant_types_supported":["authorization_code","refresh_token"],
                    "client_id_metadata_document_supported":false
                }"#,
            ),
        );
        let http = MapHttp { routes };

        let (prm, as_meta) = discover_resource_and_auth_server(&http, "https://mcp.plaud.ai/mcp")
            .expect("discovery succeeds");
        assert_eq!(prm.resource, "https://mcp.plaud.ai/mcp");
        assert_eq!(as_meta.issuer, "https://mcp.plaud.ai");
        assert_eq!(
            as_meta.registration_endpoint.as_deref(),
            Some("https://mcp.plaud.ai/register")
        );
        assert!(!as_meta.client_id_metadata_document_supported);
    }

    #[test]
    fn discovery_refuses_ssrf_poisoned_endpoints_and_issuer_mismatch() {
        let mut routes = BTreeMap::new();
        // Poisoned token_endpoint targeting loopback HTTP -> rejected fail-closed!
        routes.insert(
            "https://evil.example/.well-known/oauth-authorization-server".to_owned(),
            json_ok(
                r#"{
                    "issuer":"https://evil.example",
                    "authorization_endpoint":"https://evil.example/authorize",
                    "token_endpoint":"http://127.0.0.1:5015/steal"
                }"#,
            ),
        );
        // Mismatched issuer -> rejected fail-closed!
        routes.insert(
            "https://mismatch.example/.well-known/oauth-authorization-server".to_owned(),
            json_ok(
                r#"{
                    "issuer":"https://other.example",
                    "authorization_endpoint":"https://mismatch.example/authorize",
                    "token_endpoint":"https://mismatch.example/token"
                }"#,
            ),
        );
        // SSRF transport refusal propagates cleanly.
        routes.insert(
            "https://rebind.example/.well-known/oauth-authorization-server".to_owned(),
            Err(SsrfFetchError::UnusableAddress),
        );
        let http = MapHttp { routes };

        assert_eq!(
            discover_authorization_server(&http, "https://evil.example"),
            Err(DiscoveryError::InvalidEndpointUrl("token_endpoint"))
        );
        assert_eq!(
            discover_authorization_server(&http, "https://mismatch.example"),
            Err(DiscoveryError::IssuerMismatch)
        );
        assert_eq!(
            discover_authorization_server(&http, "https://rebind.example"),
            Err(DiscoveryError::Fetch(SsrfFetchError::UnusableAddress))
        );
    }
}
