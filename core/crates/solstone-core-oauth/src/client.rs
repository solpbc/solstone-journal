// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! OAuth 2.1 Authorization Code + PKCE (`S256`) + RFC 8707 `resource` + RFC 9207 `iss`
//! request construction and token exchange.

use std::collections::BTreeMap;
use std::fmt;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::atproto::{DpopError, DpopKeyPair, DpopRequestError, send_with_dpop_nonce_retry};
use crate::ssrf::{
    DEFAULT_REQUEST_TIMEOUT, HttpMethod, HttpRequest, OAuthHttpClient, SsrfFetchError,
    validate_https_url,
};
use crate::{code_challenge_s256, encode_form, extract_oauth_error_code, random_token};

/// Configuration for generating an Authorization Code + PKCE S256 request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorizeRequestConfig {
    pub authorization_endpoint: String,
    pub client_id: String,
    pub redirect_uri: String,
    pub scope: Option<String>,
    /// RFC 8707 target resource indicator (e.g. `https://mcp.plaud.ai/mcp`).
    pub resource: Option<String>,
    pub extra_params: BTreeMap<String, String>,
}

/// Prepared PKCE authorization request. `code_verifier` MUST remain strictly inside the journal.
#[derive(Clone, PartialEq, Eq)]
pub struct PreparedAuthorizeRequest {
    pub authorize_url: String,
    pub state: String,
    pub code_verifier: String,
    pub code_challenge: String,
    pub params: BTreeMap<String, String>,
}

impl fmt::Debug for PreparedAuthorizeRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PreparedAuthorizeRequest")
            .field("authorize_url", &self.authorize_url)
            .finish_non_exhaustive()
    }
}

/// Parsed OAuth 2.1 token endpoint response.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenResponse {
    pub access_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    pub token_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_in: Option<u64>,
    #[serde(default)]
    pub scopes: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dpop_nonce: Option<String>,
}

impl fmt::Debug for TokenResponse {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TokenResponse")
            .field("token_type", &self.token_type)
            .field("has_refresh_token", &self.refresh_token.is_some())
            .field("expires_in", &self.expires_in)
            .field("scopes", &self.scopes)
            .finish_non_exhaustive()
    }
}

/// Errors from authorization request creation or token exchange.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OAuthClientError {
    InvalidEndpoint,
    RandomFailed,
    Dpop(DpopError),
    Fetch(SsrfFetchError),
    Rejected { status: u16, error: Option<String> },
    MalformedTokenResponse(&'static str),
}

impl fmt::Display for OAuthClientError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidEndpoint => formatter.write_str("invalid HTTPS OAuth endpoint URL"),
            Self::RandomFailed => {
                formatter.write_str("failed to generate cryptographic randomness")
            }
            Self::Dpop(err) => err.fmt(formatter),
            Self::Fetch(err) => err.fmt(formatter),
            Self::Rejected { status, error } => match error {
                Some(code) => write!(
                    formatter,
                    "token endpoint rejected request ({status}: {code})"
                ),
                None => write!(formatter, "token endpoint rejected request ({status})"),
            },
            Self::MalformedTokenResponse(field) => {
                write!(formatter, "malformed token response field: {field}")
            }
        }
    }
}

impl std::error::Error for OAuthClientError {}

/// Build an OAuth 2.1 Authorization Code + PKCE (`S256`) request URL and form parameter map.
pub fn build_authorize_request(
    config: &AuthorizeRequestConfig,
) -> Result<PreparedAuthorizeRequest, OAuthClientError> {
    validate_https_url(&config.authorization_endpoint)
        .map_err(|_| OAuthClientError::InvalidEndpoint)?;
    let code_verifier = random_token(32).map_err(|_| OAuthClientError::RandomFailed)?;
    let state = random_token(24).map_err(|_| OAuthClientError::RandomFailed)?;
    build_authorize_request_with_secrets(config, code_verifier, state)
}

/// Deterministic variant of [`build_authorize_request`] accepting explicit PKCE verifier and state.
pub fn build_authorize_request_with_secrets(
    config: &AuthorizeRequestConfig,
    code_verifier: String,
    state: String,
) -> Result<PreparedAuthorizeRequest, OAuthClientError> {
    validate_https_url(&config.authorization_endpoint)
        .map_err(|_| OAuthClientError::InvalidEndpoint)?;
    let code_challenge = code_challenge_s256(&code_verifier);

    let mut params = config.extra_params.clone();
    params.insert("response_type".to_owned(), "code".to_owned());
    params.insert("client_id".to_owned(), config.client_id.clone());
    params.insert("redirect_uri".to_owned(), config.redirect_uri.clone());
    params.insert("code_challenge".to_owned(), code_challenge.clone());
    params.insert("code_challenge_method".to_owned(), "S256".to_owned());
    params.insert("state".to_owned(), state.clone());
    if let Some(scope) = config.scope.as_deref().filter(|s| !s.is_empty()) {
        params.insert("scope".to_owned(), scope.to_owned());
    }
    if let Some(resource) = config.resource.as_deref().filter(|r| !r.is_empty()) {
        params.insert("resource".to_owned(), resource.to_owned());
    }

    let separator = if config.authorization_endpoint.contains('?') {
        '&'
    } else {
        '?'
    };
    let authorize_url = format!(
        "{}{separator}{}",
        config.authorization_endpoint,
        encode_form(&params)
    );
    Ok(PreparedAuthorizeRequest {
        authorize_url,
        state,
        code_verifier,
        code_challenge,
        params,
    })
}

/// Parameters for exchanging an authorization code for tokens.
pub struct CodeExchangeRequest<'a> {
    pub token_endpoint: &'a str,
    pub client_id: &'a str,
    pub client_secret: Option<&'a str>,
    pub code: &'a str,
    pub redirect_uri: &'a str,
    pub code_verifier: &'a str,
    /// RFC 8707 target resource indicator, if bound at authorization time.
    pub resource: Option<&'a str>,
    /// Optional ES256 DPoP keypair + cached server `DPoP-Nonce` + current unix timestamp.
    pub dpop: Option<(&'a DpopKeyPair, Option<&'a str>, u64)>,
    pub timeout: Duration,
}

/// Exchange an authorization code + PKCE `code_verifier` at `token_endpoint`.
pub fn exchange_authorization_code(
    http: &dyn OAuthHttpClient,
    request: &CodeExchangeRequest<'_>,
) -> Result<TokenResponse, OAuthClientError> {
    validate_https_url(request.token_endpoint).map_err(|_| OAuthClientError::InvalidEndpoint)?;

    let mut form = BTreeMap::new();
    form.insert("grant_type".to_owned(), "authorization_code".to_owned());
    form.insert("code".to_owned(), request.code.to_owned());
    form.insert("redirect_uri".to_owned(), request.redirect_uri.to_owned());
    form.insert("client_id".to_owned(), request.client_id.to_owned());
    form.insert("code_verifier".to_owned(), request.code_verifier.to_owned());
    if let Some(secret) = request.client_secret.filter(|s| !s.is_empty()) {
        form.insert("client_secret".to_owned(), secret.to_owned());
    }
    if let Some(resource) = request.resource.filter(|r| !r.is_empty()) {
        form.insert("resource".to_owned(), resource.to_owned());
    }

    let body = encode_form(&form).into_bytes();
    let content_headers = vec![(
        "Content-Type".to_owned(),
        "application/x-www-form-urlencoded".to_owned(),
    )];
    let timeout = if request.timeout.is_zero() {
        DEFAULT_REQUEST_TIMEOUT
    } else {
        request.timeout
    };

    let (resp, dpop_nonce) = match request.dpop {
        Some((key, nonce, now_unix)) => send_with_dpop_nonce_retry(
            http,
            key,
            nonce,
            HttpMethod::Post,
            request.token_endpoint,
            &content_headers,
            Some(body),
            None,
            now_unix,
            timeout,
        )
        .map_err(|err| match err {
            DpopRequestError::Dpop(e) => OAuthClientError::Dpop(e),
            DpopRequestError::Fetch(e) => OAuthClientError::Fetch(e),
            DpopRequestError::Random => OAuthClientError::RandomFailed,
        })?,
        None => {
            let http_req = HttpRequest::post(
                request.token_endpoint,
                "application/x-www-form-urlencoded",
                body,
            )
            .with_timeout(timeout);
            let resp = http.send(&http_req).map_err(OAuthClientError::Fetch)?;
            (resp, None)
        }
    };

    if resp.status != 200 {
        return Err(OAuthClientError::Rejected {
            status: resp.status,
            error: extract_oauth_error_code(&resp.body),
        });
    }

    parse_token_response_body(&resp.body, dpop_nonce)
}

/// Parse a JSON token endpoint response body into [`TokenResponse`].
pub fn parse_token_response_body(
    body: &[u8],
    dpop_nonce: Option<String>,
) -> Result<TokenResponse, OAuthClientError> {
    let json: Value = serde_json::from_slice(body)
        .map_err(|_| OAuthClientError::MalformedTokenResponse("json"))?;
    let access_token = json
        .get("access_token")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or(OAuthClientError::MalformedTokenResponse("access_token"))?
        .to_owned();
    let refresh_token = json
        .get("refresh_token")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned);
    let token_type = json
        .get("token_type")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .unwrap_or("Bearer")
        .to_owned();
    let expires_in = json
        .get("expires_in")
        .and_then(Value::as_u64)
        .filter(|v| *v > 0);
    let scopes = json
        .get("scope")
        .and_then(Value::as_str)
        .map(|s| s.split_whitespace().map(ToOwned::to_owned).collect())
        .unwrap_or_default();
    let subject = json
        .get("sub")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned);

    Ok(TokenResponse {
        access_token,
        refresh_token,
        token_type,
        expires_in,
        scopes,
        subject,
        dpop_nonce,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Mutex;

    use crate::ssrf::HttpResponse;

    use super::*;

    struct ScriptedHttp {
        responses: Mutex<VecDeque<HttpResponse>>,
        requests: Mutex<Vec<HttpRequest>>,
    }

    impl ScriptedHttp {
        fn new(responses: Vec<HttpResponse>) -> Self {
            Self {
                responses: Mutex::new(VecDeque::from(responses)),
                requests: Mutex::new(Vec::new()),
            }
        }
    }

    impl OAuthHttpClient for ScriptedHttp {
        fn send(&self, request: &HttpRequest) -> Result<HttpResponse, SsrfFetchError> {
            self.requests.lock().unwrap().push(request.clone());
            self.responses
                .lock()
                .unwrap()
                .pop_front()
                .ok_or(SsrfFetchError::Connect)
        }
    }

    #[test]
    fn authorize_url_and_code_exchange_carry_pkce_s256_and_rfc8707_resource() {
        let cfg = AuthorizeRequestConfig {
            authorization_endpoint: "https://mcp.plaud.ai/authorize".to_owned(),
            client_id: "dcr-123".to_owned(),
            redirect_uri: "http://127.0.0.1:54321/auth/callback".to_owned(),
            scope: Some("read write".to_owned()),
            resource: Some("https://mcp.plaud.ai/mcp".to_owned()),
            extra_params: BTreeMap::new(),
        };
        let prepared = build_authorize_request_with_secrets(
            &cfg,
            "synthetic-verifier".to_owned(),
            "state-abc".to_owned(),
        )
        .expect("build authorize request");
        assert_eq!(
            prepared.code_challenge,
            code_challenge_s256("synthetic-verifier")
        );
        assert!(
            prepared
                .authorize_url
                .contains("code_challenge_method=S256")
        );
        assert!(
            prepared
                .authorize_url
                .contains("resource=https%3A%2F%2Fmcp.plaud.ai%2Fmcp")
        );
        assert!(prepared.authorize_url.contains("state=state-abc"));

        let http = ScriptedHttp::new(vec![HttpResponse {
            status: 200,
            headers: Vec::new(),
            body: br#"{"access_token":"at-1","refresh_token":"rt-1","token_type":"Bearer","expires_in":3600,"scope":"read write"}"#.to_vec(),
        }]);
        let tokens = exchange_authorization_code(
            &http,
            &CodeExchangeRequest {
                token_endpoint: "https://mcp.plaud.ai/token",
                client_id: "dcr-123",
                client_secret: None,
                code: "auth-code-9",
                redirect_uri: "http://127.0.0.1:54321/auth/callback",
                code_verifier: "synthetic-verifier",
                resource: Some("https://mcp.plaud.ai/mcp"),
                dpop: None,
                timeout: DEFAULT_REQUEST_TIMEOUT,
            },
        )
        .expect("token exchange succeeds");
        assert_eq!(tokens.access_token, "at-1");
        assert_eq!(tokens.refresh_token.as_deref(), Some("rt-1"));
        assert_eq!(tokens.expires_in, Some(3600));

        let sent = http.requests.lock().unwrap();
        let body = std::str::from_utf8(sent[0].body.as_ref().unwrap()).unwrap();
        assert!(body.contains("grant_type=authorization_code"));
        assert!(body.contains("code_verifier=synthetic-verifier"));
        assert!(body.contains("resource=https%3A%2F%2Fmcp.plaud.ai%2Fmcp"));
    }

    #[test]
    fn code_exchange_with_dpop_retries_once_on_use_dpop_nonce() {
        let key = DpopKeyPair::generate().unwrap();
        let http = ScriptedHttp::new(vec![
            HttpResponse {
                status: 400,
                headers: vec![("dpop-nonce".to_owned(), "tok-nonce-1".to_owned())],
                body: br#"{"error":"use_dpop_nonce"}"#.to_vec(),
            },
            HttpResponse {
                status: 200,
                headers: vec![("dpop-nonce".to_owned(), "tok-nonce-2".to_owned())],
                body: br#"{"access_token":"dpop-at","refresh_token":"dpop-rt","token_type":"DPoP","expires_in":300,"sub":"did:plc:example"}"#.to_vec(),
            },
        ]);

        let tokens = exchange_authorization_code(
            &http,
            &CodeExchangeRequest {
                token_endpoint: "https://bsky.social/oauth/token",
                client_id: "https://solstone.app/oauth-client-metadata.json",
                client_secret: None,
                code: "at-code",
                redirect_uri: "http://127.0.0.1:54321/callback",
                code_verifier: "ver-1",
                resource: None,
                dpop: Some((&key, None, 1_760_000_000)),
                timeout: DEFAULT_REQUEST_TIMEOUT,
            },
        )
        .expect("DPoP exchange succeeds");

        assert_eq!(tokens.token_type, "DPoP");
        assert_eq!(tokens.subject.as_deref(), Some("did:plc:example"));
        assert_eq!(tokens.dpop_nonce.as_deref(), Some("tok-nonce-2"));
        assert_eq!(http.requests.lock().unwrap().len(), 2);
    }
}
