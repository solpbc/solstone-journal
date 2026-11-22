// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Public-client registration: Client ID Metadata Documents (CIMD) when supported,
//! with RFC 7591 Dynamic Client Registration (`token_endpoint_auth_method: "none"`) fallback.

use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::discovery::AuthorizationServerMetadata;
use crate::extract_oauth_error_code;
use crate::ssrf::{HttpRequest, OAuthHttpClient, SsrfFetchError, validate_https_url};

/// How a public OAuth client obtained its `client_id`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RegistrationMode {
    Cimd,
    Dcr,
    Static,
    OwnerByo,
}

/// Request parameters for RFC 7591 public-client Dynamic Client Registration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DcrRequest {
    pub client_name: String,
    pub client_uri: Option<String>,
    pub redirect_uris: Vec<String>,
    pub grant_types: Vec<String>,
    pub response_types: Vec<String>,
    pub scope: Option<String>,
}

impl DcrRequest {
    pub fn public_auth_code(client_name: impl Into<String>, redirect_uris: Vec<String>) -> Self {
        Self {
            client_name: client_name.into(),
            client_uri: None,
            redirect_uris,
            grant_types: vec!["authorization_code".to_owned(), "refresh_token".to_owned()],
            response_types: vec!["code".to_owned()],
            scope: None,
        }
    }
}

/// Resolved public-client registration ready for authorization and token exchange.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedClient {
    pub client_id: String,
    pub mode: RegistrationMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registration_client_uri: Option<String>,
}

/// Errors from CIMD or RFC 7591 DCR client resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistrationError {
    InvalidCimdUrl,
    InvalidRegistrationEndpoint,
    EmptyRedirectUris,
    Unsupported,
    Fetch(SsrfFetchError),
    Rejected { status: u16, error: Option<String> },
    MalformedResponse,
    UnexpectedConfidentialClient,
}

impl fmt::Display for RegistrationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidCimdUrl => formatter.write_str("invalid HTTPS CIMD client_id URL"),
            Self::InvalidRegistrationEndpoint => {
                formatter.write_str("invalid HTTPS registration_endpoint URL")
            }
            Self::EmptyRedirectUris => {
                formatter.write_str("DCR registration requires at least one redirect_uri")
            }
            Self::Unsupported => formatter.write_str(
                "authorization server supports neither CIMD nor dynamic client registration",
            ),
            Self::Fetch(err) => err.fmt(formatter),
            Self::Rejected { status, error } => match error {
                Some(code) => write!(formatter, "client registration rejected ({status}: {code})"),
                None => write!(formatter, "client registration rejected ({status})"),
            },
            Self::MalformedResponse => {
                formatter.write_str("dynamic client registration returned malformed JSON")
            }
            Self::UnexpectedConfidentialClient => formatter.write_str(
                "dynamic client registration returned a non-public token_endpoint_auth_method",
            ),
        }
    }
}

impl std::error::Error for RegistrationError {}

/// Validate a static HTTPS Client ID Metadata Document URL (must be valid HTTPS with non-root path).
pub fn validate_cimd_client_id(url: &str) -> Result<(), RegistrationError> {
    let parsed = validate_https_url(url).map_err(|_| RegistrationError::InvalidCimdUrl)?;
    if parsed.path == "/" || parsed.query.is_some() {
        return Err(RegistrationError::InvalidCimdUrl);
    }
    Ok(())
}

/// Resolve a public OAuth client for `as_meta`:
/// 1. Prefer CIMD (`client_id` = `cimd_url`) when `as_meta.client_id_metadata_document_supported` is `true`
///    and `cimd_url` is supplied.
/// 2. Fall back to RFC 7591 DCR (`token_endpoint_auth_method: "none"`) when `registration_endpoint`
///    and `dcr_request` are available.
pub fn resolve_public_client(
    http: &dyn OAuthHttpClient,
    as_meta: &AuthorizationServerMetadata,
    cimd_url: Option<&str>,
    dcr_request: Option<&DcrRequest>,
) -> Result<ResolvedClient, RegistrationError> {
    if as_meta.client_id_metadata_document_supported
        && let Some(url) = cimd_url
    {
        validate_cimd_client_id(url)?;
        return Ok(ResolvedClient {
            client_id: url.to_owned(),
            mode: RegistrationMode::Cimd,
            registration_client_uri: None,
        });
    }

    if let (Some(reg_endpoint), Some(req)) = (as_meta.registration_endpoint.as_deref(), dcr_request)
    {
        return register_dynamic_public_client(http, reg_endpoint, req);
    }

    Err(RegistrationError::Unsupported)
}

/// Execute RFC 7591 Dynamic Client Registration as a public client (`token_endpoint_auth_method: "none"`).
pub fn register_dynamic_public_client(
    http: &dyn OAuthHttpClient,
    registration_endpoint: &str,
    request: &DcrRequest,
) -> Result<ResolvedClient, RegistrationError> {
    validate_https_url(registration_endpoint)
        .map_err(|_| RegistrationError::InvalidRegistrationEndpoint)?;
    if request.redirect_uris.is_empty() {
        return Err(RegistrationError::EmptyRedirectUris);
    }

    let mut payload = serde_json::Map::new();
    payload.insert("client_name".to_owned(), json!(request.client_name));
    payload.insert("redirect_uris".to_owned(), json!(request.redirect_uris));
    payload.insert("grant_types".to_owned(), json!(request.grant_types));
    payload.insert("response_types".to_owned(), json!(request.response_types));
    payload.insert("token_endpoint_auth_method".to_owned(), json!("none"));
    if let Some(uri) = &request.client_uri {
        payload.insert("client_uri".to_owned(), json!(uri));
    }
    if let Some(scope) = &request.scope {
        payload.insert("scope".to_owned(), json!(scope));
    }

    let body = serde_json::to_vec(&Value::Object(payload))
        .map_err(|_| RegistrationError::MalformedResponse)?;
    let http_req = HttpRequest::post(registration_endpoint, "application/json", body);
    let resp = http.send(&http_req).map_err(RegistrationError::Fetch)?;

    if resp.status != 200 && resp.status != 201 {
        return Err(RegistrationError::Rejected {
            status: resp.status,
            error: extract_oauth_error_code(&resp.body),
        });
    }

    let json: Value =
        serde_json::from_slice(&resp.body).map_err(|_| RegistrationError::MalformedResponse)?;
    let client_id = json
        .get("client_id")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or(RegistrationError::MalformedResponse)?
        .to_owned();

    if let Some(auth_method) = json
        .get("token_endpoint_auth_method")
        .and_then(Value::as_str)
        && auth_method != "none"
    {
        return Err(RegistrationError::UnexpectedConfidentialClient);
    }

    let registration_client_uri = json
        .get("registration_client_uri")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned);

    Ok(ResolvedClient {
        client_id,
        mode: RegistrationMode::Dcr,
        registration_client_uri,
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use crate::ssrf::HttpResponse;

    use super::*;

    struct RecordingHttp {
        response: HttpResponse,
        requests: Mutex<Vec<HttpRequest>>,
    }

    impl OAuthHttpClient for RecordingHttp {
        fn send(&self, request: &HttpRequest) -> Result<HttpResponse, SsrfFetchError> {
            self.requests.lock().unwrap().push(request.clone());
            Ok(self.response.clone())
        }
    }

    fn base_meta(cimd: bool, reg_endpoint: Option<&str>) -> AuthorizationServerMetadata {
        AuthorizationServerMetadata {
            issuer: "https://mcp.example.com".to_owned(),
            authorization_endpoint: "https://mcp.example.com/authorize".to_owned(),
            token_endpoint: "https://mcp.example.com/token".to_owned(),
            registration_endpoint: reg_endpoint.map(ToOwned::to_owned),
            revocation_endpoint: None,
            pushed_authorization_request_endpoint: None,
            device_authorization_endpoint: None,
            client_id_metadata_document_supported: cimd,
            authorization_response_iss_parameter_supported: false,
            require_pushed_authorization_requests: false,
            code_challenge_methods_supported: vec!["S256".to_owned()],
            grant_types_supported: vec![
                "authorization_code".to_owned(),
                "refresh_token".to_owned(),
            ],
            response_types_supported: vec!["code".to_owned()],
            token_endpoint_auth_methods_supported: vec!["none".to_owned()],
            dpop_signing_alg_values_supported: Vec::new(),
            scopes_supported: Vec::new(),
        }
    }

    #[test]
    fn prefers_cimd_without_network_call_when_server_advertises_cimd() {
        let http = RecordingHttp {
            response: HttpResponse {
                status: 500,
                headers: Vec::new(),
                body: Vec::new(),
            },
            requests: Mutex::new(Vec::new()),
        };
        let meta = base_meta(true, Some("https://mcp.example.com/register"));
        let dcr = DcrRequest::public_auth_code(
            "solstone",
            vec!["http://127.0.0.1:54321/callback".to_owned()],
        );
        let resolved = resolve_public_client(
            &http,
            &meta,
            Some("https://solstone.app/oauth/mcp-client-metadata.json"),
            Some(&dcr),
        )
        .expect("CIMD resolves");

        assert_eq!(resolved.mode, RegistrationMode::Cimd);
        assert_eq!(
            resolved.client_id,
            "https://solstone.app/oauth/mcp-client-metadata.json"
        );
        assert!(http.requests.lock().unwrap().is_empty());
    }

    #[test]
    fn falls_back_to_rfc7591_dcr_with_token_endpoint_auth_method_none() {
        let http = RecordingHttp {
            response: HttpResponse {
                status: 201,
                headers: Vec::new(),
                body: br#"{"client_id":"dcr-client-99","token_endpoint_auth_method":"none"}"#
                    .to_vec(),
            },
            requests: Mutex::new(Vec::new()),
        };
        let meta = base_meta(false, Some("https://mcp.plaud.ai/register"));
        let dcr = DcrRequest::public_auth_code(
            "solstone",
            vec!["http://127.0.0.1:54321/auth/callback".to_owned()],
        );
        let resolved = resolve_public_client(
            &http,
            &meta,
            Some("https://solstone.app/oauth/mcp-client-metadata.json"),
            Some(&dcr),
        )
        .expect("DCR fallback resolves");

        assert_eq!(resolved.mode, RegistrationMode::Dcr);
        assert_eq!(resolved.client_id, "dcr-client-99");

        let reqs = http.requests.lock().unwrap();
        assert_eq!(reqs.len(), 1);
        let sent_json: Value = serde_json::from_slice(reqs[0].body.as_ref().unwrap()).unwrap();
        assert_eq!(sent_json["token_endpoint_auth_method"], "none");
        assert_eq!(
            sent_json["redirect_uris"],
            json!(["http://127.0.0.1:54321/auth/callback"])
        );
    }
}
