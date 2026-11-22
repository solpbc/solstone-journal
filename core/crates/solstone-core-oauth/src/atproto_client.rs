// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! ATProto OAuth client flow for a journal signing in to its owner's own
//! atproto host: identity resolution (handle or DID to the account's PDS),
//! authorization-server discovery from that PDS, the loopback client id,
//! Pushed Authorization Request with PKCE and DPoP, the code exchange with the
//! `iss` and `sub` checks the ATProto OAuth profile requires, and DPoP-bound
//! XRPC calls to the PDS.
//!
//! Every request goes from the journal to the owner's identity directory and
//! host; nothing here names or reaches a host the journal's maker runs.

use std::collections::BTreeMap;
use std::fmt;
use std::time::Duration;

use serde_json::Value;

use crate::atproto::{
    DpopKeyPair, DpopRequestError, ParError, build_par_authorize_url, push_authorization_request,
    send_with_dpop_nonce_retry,
};
use crate::client::{
    CodeExchangeRequest, OAuthClientError, TokenResponse, exchange_authorization_code,
};
use crate::discovery::{
    AuthorizationServerMetadata, DiscoveryError, discover_resource_and_auth_server,
};
use crate::ssrf::{
    DEFAULT_REQUEST_TIMEOUT, HttpMethod, HttpRequest, HttpResponse, OAuthHttpClient,
    SsrfFetchError, validate_https_url,
};
use crate::{code_challenge_s256, percent_encode, random_token};

/// The base scope every ATProto OAuth session must carry.
pub const ATPROTO_SCOPE: &str = "atproto";

/// The public PLC directory, where `did:plc` documents resolve.
pub const DEFAULT_PLC_DIRECTORY: &str = "https://plc.directory";

const MAX_DID_DOCUMENT_BYTES: usize = 64 * 1024;

/// Errors from the ATProto sign-in flow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AtprotoClientError {
    InvalidIdentifier,
    HandleNotResolved,
    HandleConflict,
    DidDocumentInvalid,
    NoPds,
    HandleMismatch,
    Discovery(DiscoveryError),
    ServerUnsupported(&'static str),
    Par(ParError),
    Exchange(OAuthClientError),
    IssuerMismatch,
    SubjectMissing,
    SubjectMismatch,
    SubjectServerMismatch,
    ScopeMissingAtproto,
    Fetch(SsrfFetchError),
    Random,
    Dpop,
}

impl fmt::Display for AtprotoClientError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidIdentifier => formatter.write_str("not a handle or DID"),
            Self::HandleNotResolved => formatter.write_str("handle did not resolve to a DID"),
            Self::HandleConflict => {
                formatter.write_str("handle's DNS record and well-known file name different DIDs")
            }
            Self::DidDocumentInvalid => formatter.write_str("DID document is missing or invalid"),
            Self::NoPds => formatter.write_str("DID document names no atproto host"),
            Self::HandleMismatch => {
                formatter.write_str("handle and DID document do not name each other")
            }
            Self::Discovery(err) => err.fmt(formatter),
            Self::ServerUnsupported(what) => {
                write!(formatter, "authorization server does not support {what}")
            }
            Self::Par(err) => err.fmt(formatter),
            Self::Exchange(err) => err.fmt(formatter),
            Self::IssuerMismatch => {
                formatter.write_str("authorization response came from a different issuer")
            }
            Self::SubjectMissing => formatter.write_str("token response carried no account DID"),
            Self::SubjectMismatch => {
                formatter.write_str("token response is for a different account")
            }
            Self::SubjectServerMismatch => {
                formatter.write_str("the account's host is not served by this authorization server")
            }
            Self::ScopeMissingAtproto => formatter.write_str("granted scope lacks `atproto`"),
            Self::Fetch(err) => err.fmt(formatter),
            Self::Random => formatter.write_str("failed to generate random values"),
            Self::Dpop => formatter.write_str("failed to sign a DPoP proof"),
        }
    }
}

impl std::error::Error for AtprotoClientError {}

/// True when `value` is syntactically a DID of a method atproto accepts.
pub fn is_atproto_did(value: &str) -> bool {
    let rest = if let Some(rest) = value.strip_prefix("did:plc:") {
        rest
    } else if let Some(rest) = value.strip_prefix("did:web:") {
        rest
    } else {
        return false;
    };
    !rest.is_empty()
        && rest.len() <= 2048
        && rest
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_' | b':' | b'%'))
}

/// Normalize a handle the way atproto compares them: drop a leading `@`,
/// lowercase, and require at least two dot-separated labels.
pub fn normalize_handle(raw: &str) -> Option<String> {
    let handle = raw.trim().trim_start_matches('@').to_ascii_lowercase();
    let labels: Vec<&str> = handle.split('.').collect();
    let valid = handle.len() <= 253
        && labels.len() >= 2
        && labels.iter().all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        });
    valid.then_some(handle)
}

/// A resolved atproto account: its DID, its verified handle if any, and the
/// PDS that hosts its repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedIdentity {
    pub did: String,
    pub handle: Option<String>,
    pub pds: String,
}

/// The parts of a DID document the client reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DidDocument {
    pub id: String,
    pub also_known_as: Vec<String>,
    pub pds: Option<String>,
}

impl DidDocument {
    pub fn parse(bytes: &[u8]) -> Result<Self, AtprotoClientError> {
        let json: Value =
            serde_json::from_slice(bytes).map_err(|_| AtprotoClientError::DidDocumentInvalid)?;
        let id = json
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| is_atproto_did(id))
            .ok_or(AtprotoClientError::DidDocumentInvalid)?
            .to_owned();
        let also_known_as = json
            .get("alsoKnownAs")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(ToOwned::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        let pds = json
            .get("service")
            .and_then(Value::as_array)
            .and_then(|services| {
                services.iter().find(|service| {
                    let id = service.get("id").and_then(Value::as_str).unwrap_or("");
                    let kind = service.get("type").and_then(Value::as_str).unwrap_or("");
                    id.ends_with("#atproto_pds") && kind == "AtprotoPersonalDataServer"
                })
            })
            .and_then(|service| service.get("serviceEndpoint").and_then(Value::as_str))
            .filter(|endpoint| validate_https_url(endpoint).is_ok())
            .map(|endpoint| endpoint.trim_end_matches('/').to_owned());
        Ok(Self {
            id,
            also_known_as,
            pds,
        })
    }

    /// The handle the document claims, from its first `at://` alias.
    pub fn claimed_handle(&self) -> Option<String> {
        self.also_known_as
            .iter()
            .find_map(|aka| aka.strip_prefix("at://"))
            .and_then(normalize_handle)
    }
}

fn get_bounded(
    http: &dyn OAuthHttpClient,
    url: &str,
    max_bytes: usize,
) -> Result<HttpResponse, AtprotoClientError> {
    http.send(&HttpRequest::get(url).with_max_body_bytes(max_bytes))
        .map_err(AtprotoClientError::Fetch)
}

/// Looks up DNS TXT records for handle resolution. A name with no TXT
/// records answers an empty list; `None` means the lookup itself failed.
pub trait HandleDns: Send + Sync {
    fn txt(&self, name: &str) -> Option<Vec<String>>;
}

/// Resolves handles by their HTTPS well-known file alone.
pub struct NoHandleDns;

impl HandleDns for NoHandleDns {
    fn txt(&self, _name: &str) -> Option<Vec<String>> {
        Some(Vec::new())
    }
}

/// The DID a handle's `_atproto.<handle>` TXT records name, if any. Records
/// not of the `did=` form are ignored; two different DIDs are a conflict. A
/// failed lookup counts as no answer, so the well-known file can still
/// resolve the handle.
fn handle_did_from_dns(
    dns: &dyn HandleDns,
    handle: &str,
) -> Result<Option<String>, AtprotoClientError> {
    let Some(records) = dns.txt(&format!("_atproto.{handle}")) else {
        return Ok(None);
    };
    let mut found: Option<String> = None;
    for did in records
        .iter()
        .filter_map(|record| record.trim().strip_prefix("did="))
        .map(str::trim)
        .filter(|did| is_atproto_did(did))
    {
        match &found {
            Some(existing) if existing != did => return Err(AtprotoClientError::HandleConflict),
            Some(_) => {}
            None => found = Some(did.to_owned()),
        }
    }
    Ok(found)
}

/// The DID `https://<handle>/.well-known/atproto-did` names, if it answers.
fn handle_did_from_well_known(http: &dyn OAuthHttpClient, handle: &str) -> Option<String> {
    let url = format!("https://{handle}/.well-known/atproto-did");
    let response = get_bounded(http, &url, 2048).ok()?;
    if response.status != 200 {
        return None;
    }
    let did = std::str::from_utf8(&response.body).ok()?.trim();
    is_atproto_did(did).then(|| did.to_owned())
}

/// Resolve a handle to a DID, the two ways atproto publishes one: the
/// `_atproto.<handle>` DNS TXT record first, then the handle's HTTPS
/// well-known file. Either alone resolves the handle; when both answer and
/// name different DIDs the handle is refused.
pub fn resolve_handle(
    http: &dyn OAuthHttpClient,
    dns: &dyn HandleDns,
    handle: &str,
) -> Result<String, AtprotoClientError> {
    let handle = normalize_handle(handle).ok_or(AtprotoClientError::InvalidIdentifier)?;
    let from_dns = handle_did_from_dns(dns, &handle)?;
    let from_well_known = handle_did_from_well_known(http, &handle);
    match (from_dns, from_well_known) {
        (Some(dns_did), Some(web_did)) if dns_did != web_did => {
            Err(AtprotoClientError::HandleConflict)
        }
        (Some(did), _) | (None, Some(did)) => Ok(did),
        (None, None) => Err(AtprotoClientError::HandleNotResolved),
    }
}

/// Resolve a DID to its document: `did:plc` through the PLC directory,
/// `did:web` through the host's `/.well-known/did.json`.
pub fn resolve_did_document(
    http: &dyn OAuthHttpClient,
    did: &str,
    plc_directory: &str,
) -> Result<DidDocument, AtprotoClientError> {
    if !is_atproto_did(did) {
        return Err(AtprotoClientError::InvalidIdentifier);
    }
    let url = if did.starts_with("did:plc:") {
        format!("{}/{did}", plc_directory.trim_end_matches('/'))
    } else {
        let host = did.trim_start_matches("did:web:");
        if host.contains(':') {
            // Path-based did:web is not valid in atproto.
            return Err(AtprotoClientError::InvalidIdentifier);
        }
        format!("https://{host}/.well-known/did.json")
    };
    let response = get_bounded(http, &url, MAX_DID_DOCUMENT_BYTES)?;
    if response.status != 200 {
        return Err(AtprotoClientError::DidDocumentInvalid);
    }
    let document = DidDocument::parse(&response.body)?;
    if document.id != did {
        return Err(AtprotoClientError::DidDocumentInvalid);
    }
    Ok(document)
}

/// Resolve what the owner typed (a handle or a DID) to an account and its
/// PDS. A handle is verified both ways: it resolves to the DID, and the DID
/// document claims the handle back. A DID input keeps its claimed handle only
/// when that handle resolves back to it.
pub fn resolve_identity(
    http: &dyn OAuthHttpClient,
    dns: &dyn HandleDns,
    input: &str,
    plc_directory: &str,
) -> Result<ResolvedIdentity, AtprotoClientError> {
    let input = input.trim();
    if is_atproto_did(input) {
        let document = resolve_did_document(http, input, plc_directory)?;
        let pds = document.pds.clone().ok_or(AtprotoClientError::NoPds)?;
        let handle = document
            .claimed_handle()
            .filter(|handle| resolve_handle(http, dns, handle).ok().as_deref() == Some(input));
        return Ok(ResolvedIdentity {
            did: document.id,
            handle,
            pds,
        });
    }
    let handle = normalize_handle(input).ok_or(AtprotoClientError::InvalidIdentifier)?;
    let did = resolve_handle(http, dns, &handle)?;
    let document = resolve_did_document(http, &did, plc_directory)?;
    if document.claimed_handle().as_deref() != Some(handle.as_str()) {
        return Err(AtprotoClientError::HandleMismatch);
    }
    let pds = document.pds.ok_or(AtprotoClientError::NoPds)?;
    Ok(ResolvedIdentity {
        did,
        handle: Some(handle),
        pds,
    })
}

/// The atproto loopback client id: `http://localhost` carrying the exact
/// redirect URI and scope as query parameters, so no client metadata document
/// is fetched from anywhere.
pub fn loopback_client_id(redirect_uri: &str, scope: &str) -> String {
    format!(
        "http://localhost?redirect_uri={}&scope={}",
        percent_encode(redirect_uri),
        percent_encode(scope)
    )
}

/// The authorization server that serves an account's PDS.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AtprotoAuthServer {
    pub pds: String,
    pub metadata: AuthorizationServerMetadata,
    pub par_endpoint: String,
}

/// Discover the authorization server for `pds` and check it offers what the
/// ATProto profile requires: PAR, PKCE S256 and ES256 DPoP.
pub fn discover_auth_server(
    http: &dyn OAuthHttpClient,
    pds: &str,
) -> Result<AtprotoAuthServer, AtprotoClientError> {
    let (_, metadata) =
        discover_resource_and_auth_server(http, pds).map_err(AtprotoClientError::Discovery)?;
    let par_endpoint = metadata
        .pushed_authorization_request_endpoint
        .clone()
        .ok_or(AtprotoClientError::ServerUnsupported("PAR"))?;
    if !metadata
        .code_challenge_methods_supported
        .iter()
        .any(|m| m == "S256")
    {
        return Err(AtprotoClientError::ServerUnsupported("PKCE S256"));
    }
    if !metadata
        .dpop_signing_alg_values_supported
        .iter()
        .any(|alg| alg == "ES256")
    {
        return Err(AtprotoClientError::ServerUnsupported("ES256 DPoP"));
    }
    Ok(AtprotoAuthServer {
        pds: pds.trim_end_matches('/').to_owned(),
        metadata,
        par_endpoint,
    })
}

/// One sign-in in flight, between the PAR and the callback.
#[derive(Clone, PartialEq, Eq)]
pub struct PendingAuthorization {
    pub authorize_url: String,
    pub state: String,
    pub code_verifier: String,
    pub client_id: String,
    pub redirect_uri: String,
    pub scope: String,
    pub issuer: String,
    pub token_endpoint: String,
    pub expected_did: Option<String>,
    pub dpop_nonce: Option<String>,
}

impl fmt::Debug for PendingAuthorization {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PendingAuthorization")
            .field("client_id", &self.client_id)
            .field("scope", &self.scope)
            .field("issuer", &self.issuer)
            .field("expected_did", &self.expected_did)
            .finish_non_exhaustive()
    }
}

/// What the caller supplies to start a sign-in.
pub struct AuthorizationStart<'a> {
    pub server: &'a AtprotoAuthServer,
    pub client_id: &'a str,
    pub redirect_uri: &'a str,
    pub scope: &'a str,
    /// The handle or DID the owner typed; sent as `login_hint`.
    pub login_hint: Option<&'a str>,
    /// The DID the hint resolved to, checked against the token's `sub`.
    pub expected_did: Option<&'a str>,
    pub dpop_key: &'a DpopKeyPair,
    pub now_unix: u64,
}

/// Push the authorization request and return the URL to open in the owner's
/// browser, with the secrets the callback needs.
pub fn begin_authorization(
    http: &dyn OAuthHttpClient,
    start: &AuthorizationStart<'_>,
) -> Result<PendingAuthorization, AtprotoClientError> {
    if !start
        .scope
        .split_whitespace()
        .any(|scope| scope == ATPROTO_SCOPE)
    {
        return Err(AtprotoClientError::ScopeMissingAtproto);
    }
    let state = random_token(24).map_err(|_| AtprotoClientError::Random)?;
    let code_verifier = random_token(48).map_err(|_| AtprotoClientError::Random)?;
    let mut params = BTreeMap::new();
    params.insert("client_id".to_owned(), start.client_id.to_owned());
    params.insert("response_type".to_owned(), "code".to_owned());
    params.insert("redirect_uri".to_owned(), start.redirect_uri.to_owned());
    params.insert("scope".to_owned(), start.scope.to_owned());
    params.insert("state".to_owned(), state.clone());
    params.insert(
        "code_challenge".to_owned(),
        code_challenge_s256(&code_verifier),
    );
    params.insert("code_challenge_method".to_owned(), "S256".to_owned());
    if let Some(hint) = start.login_hint.filter(|hint| !hint.is_empty()) {
        params.insert("login_hint".to_owned(), hint.to_owned());
    }
    let par = push_authorization_request(
        http,
        &start.server.par_endpoint,
        &params,
        Some((start.dpop_key, None, start.now_unix)),
    )
    .map_err(AtprotoClientError::Par)?;
    let authorize_url = build_par_authorize_url(
        &start.server.metadata.authorization_endpoint,
        start.client_id,
        &par.request_uri,
    )
    .map_err(AtprotoClientError::Par)?;
    Ok(PendingAuthorization {
        authorize_url,
        state,
        code_verifier,
        client_id: start.client_id.to_owned(),
        redirect_uri: start.redirect_uri.to_owned(),
        scope: start.scope.to_owned(),
        issuer: start.server.metadata.issuer.clone(),
        token_endpoint: start.server.metadata.token_endpoint.clone(),
        expected_did: start.expected_did.map(ToOwned::to_owned),
        dpop_nonce: par.dpop_nonce,
    })
}

/// A signed-in session, ready to store.
#[derive(Clone, PartialEq, Eq)]
pub struct AtprotoSession {
    pub did: String,
    pub pds: String,
    pub issuer: String,
    pub token_endpoint: String,
    pub client_id: String,
    pub tokens: TokenResponse,
}

impl fmt::Debug for AtprotoSession {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AtprotoSession")
            .field("did", &self.did)
            .field("pds", &self.pds)
            .field("issuer", &self.issuer)
            .field("tokens", &self.tokens)
            .finish()
    }
}

/// Exchange the callback's code, then verify the result the way the ATProto
/// OAuth profile requires: the response came from the issuer the request went
/// to, the token names an account (`sub`), that account is the one the owner
/// asked for, and its own PDS is served by this same authorization server.
pub fn complete_authorization(
    http: &dyn OAuthHttpClient,
    pending: &PendingAuthorization,
    code: &str,
    response_issuer: Option<&str>,
    dpop_key: &DpopKeyPair,
    now_unix: u64,
    plc_directory: &str,
) -> Result<AtprotoSession, AtprotoClientError> {
    if response_issuer != Some(pending.issuer.as_str()) {
        return Err(AtprotoClientError::IssuerMismatch);
    }
    let tokens = exchange_authorization_code(
        http,
        &CodeExchangeRequest {
            token_endpoint: &pending.token_endpoint,
            client_id: &pending.client_id,
            client_secret: None,
            code,
            redirect_uri: &pending.redirect_uri,
            code_verifier: &pending.code_verifier,
            resource: None,
            dpop: Some((dpop_key, pending.dpop_nonce.as_deref(), now_unix)),
            timeout: DEFAULT_REQUEST_TIMEOUT,
        },
    )
    .map_err(AtprotoClientError::Exchange)?;
    if !tokens.scopes.iter().any(|scope| scope == ATPROTO_SCOPE) {
        return Err(AtprotoClientError::ScopeMissingAtproto);
    }
    let did = tokens
        .subject
        .clone()
        .filter(|sub| is_atproto_did(sub))
        .ok_or(AtprotoClientError::SubjectMissing)?;
    if let Some(expected) = &pending.expected_did
        && expected != &did
    {
        return Err(AtprotoClientError::SubjectMismatch);
    }
    let document = resolve_did_document(http, &did, plc_directory)?;
    let pds = document.pds.ok_or(AtprotoClientError::NoPds)?;
    let server = discover_auth_server(http, &pds)?;
    if server.metadata.issuer != pending.issuer {
        return Err(AtprotoClientError::SubjectServerMismatch);
    }
    Ok(AtprotoSession {
        did,
        pds,
        issuer: pending.issuer.clone(),
        token_endpoint: pending.token_endpoint.clone(),
        client_id: pending.client_id.clone(),
        tokens,
    })
}

/// A DPoP-bound access token and the key it is bound to.
pub struct XrpcAuth<'a> {
    pub access_token: &'a str,
    pub dpop_key: &'a DpopKeyPair,
    /// The PDS's last `DPoP-Nonce`, if known; the call retries once on a
    /// nonce challenge either way.
    pub nonce: Option<&'a str>,
}

/// The body of an XRPC call.
pub enum XrpcBody<'a> {
    /// A query: `GET` with these parameters.
    Query(&'a [(&'a str, &'a str)]),
    /// A procedure: `POST` with this JSON input.
    Procedure(&'a Value),
}

/// Build `<pds>/xrpc/<nsid>` with query parameters in the given order.
pub fn xrpc_url(service: &str, nsid: &str, params: &[(&str, &str)]) -> String {
    let mut url = format!("{}/xrpc/{nsid}", service.trim_end_matches('/'));
    for (index, (key, value)) in params.iter().enumerate() {
        url.push(if index == 0 { '?' } else { '&' });
        url.push_str(&percent_encode(key));
        url.push('=');
        url.push_str(&percent_encode(value));
    }
    url
}

/// Call an XRPC method on the owner's PDS with a DPoP-bound access token.
/// Returns the response and the PDS's latest nonce.
#[allow(clippy::too_many_arguments)]
pub fn xrpc_call(
    http: &dyn OAuthHttpClient,
    service: &str,
    nsid: &str,
    body: XrpcBody<'_>,
    auth: &XrpcAuth<'_>,
    now_unix: u64,
    timeout: Duration,
    max_body_bytes: usize,
) -> Result<(HttpResponse, Option<String>), AtprotoClientError> {
    let (method, url, payload, mut headers) = match body {
        XrpcBody::Query(params) => (
            HttpMethod::Get,
            xrpc_url(service, nsid, params),
            None,
            Vec::new(),
        ),
        XrpcBody::Procedure(input) => (
            HttpMethod::Post,
            xrpc_url(service, nsid, &[]),
            Some(serde_json::to_vec(input).map_err(|_| AtprotoClientError::Dpop)?),
            vec![("Content-Type".to_owned(), "application/json".to_owned())],
        ),
    };
    headers.push((
        "Authorization".to_owned(),
        format!("DPoP {}", auth.access_token),
    ));
    send_with_dpop_nonce_retry(
        &BodyCapped {
            inner: http,
            max_body_bytes,
        },
        auth.dpop_key,
        auth.nonce,
        method,
        &url,
        &headers,
        payload,
        Some(auth.access_token),
        now_unix,
        timeout,
    )
    .map_err(|err| match err {
        DpopRequestError::Fetch(err) => AtprotoClientError::Fetch(err),
        DpopRequestError::Random => AtprotoClientError::Random,
        DpopRequestError::Dpop(_) => AtprotoClientError::Dpop,
    })
}

/// Raises the response cap for XRPC reads, which can exceed the OAuth
/// default (a page of records).
struct BodyCapped<'a> {
    inner: &'a dyn OAuthHttpClient,
    max_body_bytes: usize,
}

impl OAuthHttpClient for BodyCapped<'_> {
    fn send(&self, request: &HttpRequest) -> Result<HttpResponse, SsrfFetchError> {
        let mut request = request.clone();
        request.max_body_bytes = request.max_body_bytes.max(self.max_body_bytes);
        self.inner.send(&request)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Mutex;

    use serde_json::json;

    use super::*;
    use crate::atproto::base64_url_decode;

    /// Answers by URL prefix, in order; records every request.
    struct RoutedHttp {
        routes: Mutex<VecDeque<(String, HttpResponse)>>,
        requests: Mutex<Vec<HttpRequest>>,
    }

    impl RoutedHttp {
        fn new(routes: Vec<(&str, u16, Value)>) -> Self {
            Self {
                routes: Mutex::new(
                    routes
                        .into_iter()
                        .map(|(prefix, status, body)| {
                            let body = match body {
                                Value::String(text) => text.into_bytes(),
                                other => serde_json::to_vec(&other).unwrap(),
                            };
                            (
                                prefix.to_owned(),
                                HttpResponse {
                                    status,
                                    headers: vec![("dpop-nonce".to_owned(), "n1".to_owned())],
                                    body,
                                },
                            )
                        })
                        .collect(),
                ),
                requests: Mutex::new(Vec::new()),
            }
        }
    }

    impl OAuthHttpClient for RoutedHttp {
        fn send(&self, request: &HttpRequest) -> Result<HttpResponse, SsrfFetchError> {
            self.requests.lock().unwrap().push(request.clone());
            let mut routes = self.routes.lock().unwrap();
            let index = routes
                .iter()
                .position(|(prefix, _)| request.url.starts_with(prefix.as_str()))
                .ok_or(SsrfFetchError::Connect)?;
            Ok(routes.remove(index).unwrap().1)
        }
    }

    const DID: &str = "did:plc:abcdefghijklmnopqrstuvwx";
    const PDS: &str = "https://pds.example.com";

    fn did_doc(handle: &str) -> Value {
        json!({
            "id": DID,
            "alsoKnownAs": [format!("at://{handle}")],
            "service": [{"id": "#atproto_pds", "type": "AtprotoPersonalDataServer", "serviceEndpoint": PDS}]
        })
    }

    fn server_routes() -> Vec<(&'static str, u16, Value)> {
        vec![
            (
                "https://pds.example.com/.well-known/oauth-protected-resource",
                200,
                json!({"resource": PDS, "authorization_servers": ["https://auth.example.com"]}),
            ),
            (
                "https://auth.example.com/.well-known/oauth-authorization-server",
                200,
                json!({
                    "issuer": "https://auth.example.com",
                    "authorization_endpoint": "https://auth.example.com/oauth/authorize",
                    "token_endpoint": "https://auth.example.com/oauth/token",
                    "pushed_authorization_request_endpoint": "https://auth.example.com/oauth/par",
                    "code_challenge_methods_supported": ["S256"],
                    "dpop_signing_alg_values_supported": ["ES256"]
                }),
            ),
        ]
    }

    #[test]
    fn a_handle_is_verified_both_ways_and_names_its_host() {
        let http = RoutedHttp::new(vec![
            (
                "https://alice.example.org/.well-known/atproto-did",
                200,
                Value::String(format!("{DID}\n")),
            ),
            (
                "https://plc.directory/did:plc:",
                200,
                did_doc("alice.example.org"),
            ),
        ]);
        let identity = resolve_identity(
            &http,
            &NoHandleDns,
            "@Alice.Example.org",
            DEFAULT_PLC_DIRECTORY,
        )
        .unwrap();
        assert_eq!(identity.did, DID);
        assert_eq!(identity.handle.as_deref(), Some("alice.example.org"));
        assert_eq!(identity.pds, PDS);
    }

    #[test]
    fn a_handle_the_did_document_does_not_claim_is_refused() {
        let http = RoutedHttp::new(vec![
            (
                "https://alice.example.org/.well-known/atproto-did",
                200,
                Value::String(DID.to_owned()),
            ),
            (
                "https://plc.directory/did:plc:",
                200,
                did_doc("mallory.example.org"),
            ),
        ]);
        assert_eq!(
            resolve_identity(
                &http,
                &NoHandleDns,
                "alice.example.org",
                DEFAULT_PLC_DIRECTORY
            ),
            Err(AtprotoClientError::HandleMismatch)
        );
    }

    struct MapDns(Vec<(&'static str, Option<Vec<String>>)>);

    impl HandleDns for MapDns {
        fn txt(&self, name: &str) -> Option<Vec<String>> {
            self.0
                .iter()
                .find(|(key, _)| *key == name)
                .map_or(Some(Vec::new()), |(_, answer)| answer.clone())
        }
    }

    const OTHER: &str = "did:plc:zzzzzzzzzzzzzzzzzzzzzzzz";

    #[test]
    fn a_handle_published_only_by_dns_resolves() {
        let http = RoutedHttp::new(vec![
            (
                "https://alice.example.org/.well-known/atproto-did",
                404,
                Value::String("not found".to_owned()),
            ),
            (
                "https://plc.directory/did:plc:",
                200,
                did_doc("alice.example.org"),
            ),
        ]);
        let dns = MapDns(vec![(
            "_atproto.alice.example.org",
            Some(vec!["v=other".to_owned(), format!("did={DID}")]),
        )]);
        let identity =
            resolve_identity(&http, &dns, "alice.example.org", DEFAULT_PLC_DIRECTORY).unwrap();
        assert_eq!(identity.did, DID);
        assert_eq!(identity.handle.as_deref(), Some("alice.example.org"));
    }

    #[test]
    fn dns_and_well_known_naming_different_accounts_is_refused() {
        let http = RoutedHttp::new(vec![(
            "https://alice.example.org/.well-known/atproto-did",
            200,
            Value::String(OTHER.to_owned()),
        )]);
        let dns = MapDns(vec![(
            "_atproto.alice.example.org",
            Some(vec![format!("did={DID}")]),
        )]);
        assert_eq!(
            resolve_handle(&http, &dns, "alice.example.org"),
            Err(AtprotoClientError::HandleConflict)
        );
    }

    #[test]
    fn two_dns_records_naming_different_accounts_are_refused() {
        let http = RoutedHttp::new(vec![]);
        let dns = MapDns(vec![(
            "_atproto.alice.example.org",
            Some(vec![format!("did={DID}"), format!("did={OTHER}")]),
        )]);
        assert_eq!(
            resolve_handle(&http, &dns, "alice.example.org"),
            Err(AtprotoClientError::HandleConflict)
        );
    }

    #[test]
    fn a_failed_dns_lookup_falls_back_to_the_well_known_file() {
        let http = RoutedHttp::new(vec![(
            "https://alice.example.org/.well-known/atproto-did",
            200,
            Value::String(DID.to_owned()),
        )]);
        let dns = MapDns(vec![("_atproto.alice.example.org", None)]);
        assert_eq!(
            resolve_handle(&http, &dns, "alice.example.org").as_deref(),
            Ok(DID)
        );
        assert_eq!(
            resolve_handle(&RoutedHttp::new(vec![]), &dns, "alice.example.org"),
            Err(AtprotoClientError::HandleNotResolved)
        );
    }

    #[test]
    fn the_loopback_client_id_carries_redirect_and_scope() {
        let id = loopback_client_id(
            "http://127.0.0.1:61234/callback",
            "atproto space:*?action=read",
        );
        assert_eq!(
            id,
            "http://localhost?redirect_uri=http%3A%2F%2F127.0.0.1%3A61234%2Fcallback&scope=atproto%20space%3A%2A%3Faction%3Dread"
        );
    }

    fn pending(http: &RoutedHttp, key: &DpopKeyPair) -> PendingAuthorization {
        let server = discover_auth_server(http, PDS).unwrap();
        begin_authorization(
            http,
            &AuthorizationStart {
                server: &server,
                client_id: "http://localhost?x",
                redirect_uri: "http://127.0.0.1:61234/callback",
                scope: "atproto space:*?action=read",
                login_hint: Some("alice.example.org"),
                expected_did: Some(DID),
                dpop_key: key,
                now_unix: 1_790_000_000,
            },
        )
        .unwrap()
    }

    #[test]
    fn sign_in_pushes_the_request_and_checks_the_account_it_returns() {
        let mut routes = server_routes();
        routes.push((
            "https://auth.example.com/oauth/par",
            201,
            json!({"request_uri": "urn:ietf:params:oauth:request_uri:r1", "expires_in": 60}),
        ));
        routes.push((
            "https://auth.example.com/oauth/token",
            200,
            json!({"access_token": "at1", "refresh_token": "rt1", "token_type": "DPoP", "expires_in": 3600, "scope": "atproto space:*?action=read", "sub": DID}),
        ));
        routes.push((
            "https://plc.directory/did:plc:",
            200,
            did_doc("alice.example.org"),
        ));
        routes.extend(server_routes());
        let http = RoutedHttp::new(routes);
        let key = DpopKeyPair::generate().unwrap();
        let pending = pending(&http, &key);
        assert!(
            pending
                .authorize_url
                .starts_with("https://auth.example.com/oauth/authorize?client_id=")
        );

        let session = complete_authorization(
            &http,
            &pending,
            "code1",
            Some("https://auth.example.com"),
            &key,
            1_790_000_001,
            DEFAULT_PLC_DIRECTORY,
        )
        .unwrap();
        assert_eq!(session.did, DID);
        assert_eq!(session.pds, PDS);

        let requests = http.requests.lock().unwrap();
        let par = requests
            .iter()
            .find(|r| r.url.ends_with("/oauth/par"))
            .unwrap();
        let form = String::from_utf8(par.body.clone().unwrap()).unwrap();
        assert!(form.contains("login_hint=alice.example.org"));
        assert!(form.contains("code_challenge_method=S256"));
        assert!(par.headers.iter().any(|(k, _)| k == "DPoP"));
    }

    #[test]
    fn a_token_for_another_account_is_refused() {
        let mut routes = server_routes();
        routes.push((
            "https://auth.example.com/oauth/par",
            201,
            json!({"request_uri": "urn:r", "expires_in": 60}),
        ));
        routes.push((
            "https://auth.example.com/oauth/token",
            200,
            json!({"access_token": "at1", "token_type": "DPoP", "scope": "atproto", "sub": "did:plc:someoneelse0000000000000"}),
        ));
        let http = RoutedHttp::new(routes);
        let key = DpopKeyPair::generate().unwrap();
        let pending = pending(&http, &key);
        assert_eq!(
            complete_authorization(
                &http,
                &pending,
                "c",
                Some("https://auth.example.com"),
                &key,
                1,
                DEFAULT_PLC_DIRECTORY
            ),
            Err(AtprotoClientError::SubjectMismatch)
        );
        assert_eq!(
            complete_authorization(
                &http,
                &pending,
                "c",
                Some("https://evil.example.com"),
                &key,
                1,
                DEFAULT_PLC_DIRECTORY
            ),
            Err(AtprotoClientError::IssuerMismatch)
        );
    }

    #[test]
    fn an_xrpc_call_binds_the_token_to_its_proof() {
        let http = RoutedHttp::new(vec![(
            "https://pds.example.com/xrpc/com.atproto.space.listRecords",
            200,
            json!({"records": []}),
        )]);
        let key = DpopKeyPair::generate().unwrap();
        let (response, nonce) = xrpc_call(
            &http,
            PDS,
            "com.atproto.space.listRecords",
            XrpcBody::Query(&[("space", "at://x"), ("repo", DID)]),
            &XrpcAuth {
                access_token: "at1",
                dpop_key: &key,
                nonce: None,
            },
            1,
            DEFAULT_REQUEST_TIMEOUT,
            1 << 20,
        )
        .unwrap();
        assert_eq!(response.status, 200);
        assert_eq!(nonce.as_deref(), Some("n1"));
        let requests = http.requests.lock().unwrap();
        assert_eq!(requests[0].max_body_bytes, 1 << 20);
        assert!(
            requests[0]
                .url
                .contains("?space=at%3A%2F%2Fx&repo=did%3Aplc%3A")
        );
        assert!(
            requests[0]
                .headers
                .iter()
                .any(|(k, v)| k == "Authorization" && v == "DPoP at1")
        );
        let proof = &requests[0]
            .headers
            .iter()
            .find(|(k, _)| k == "DPoP")
            .unwrap()
            .1;
        let claims: Value =
            serde_json::from_slice(&base64_url_decode(proof.split('.').nth(1).unwrap()).unwrap())
                .unwrap();
        assert!(claims.get("ath").is_some());
        assert_eq!(
            claims["htu"],
            "https://pds.example.com/xrpc/com.atproto.space.listRecords"
        );
    }
}
