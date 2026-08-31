// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Dynamic client registration for CIMD and classic public clients.

use std::net::IpAddr;

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::Serialize;
use tokio::sync::watch;

use super::cimd::{CimdAttemptIo, CimdDocument, canonicalize_ip, fetch_cimd, fetch_cimd_with_io};
use super::redirect::parse_redirect_uri;
use super::store::{OAuthStoreError, RegisteredClient};
use super::urlparse::validate_cimd_url;
use super::{
    DCR_MAX_BODY_BYTES, MAX_CLIENT_ID_BYTES, MAX_CLIENT_NAME_BYTES, MAX_REDIRECT_URI_BYTES,
    MAX_REDIRECT_URIS_PER_CLIENT, OAuthRuntime, reject_non_identity_encoding,
};
use crate::http1::{HttpRequest, HttpResponse};
use crate::tokens::{RandomSource, SystemRandomSource};

const CLIENT_ID_BYTES: usize = 32;

/// Why a CIMD client could not be resolved or registered.
#[derive(Debug)]
pub(crate) enum CimdRegistrationError {
    Fetch,
    NoAllowlistedRedirect,
    RequestedRedirectNotInDocument,
    InvalidName,
    Randomness,
    Store(OAuthStoreError),
}

/// A CIMD client record plus whether this call inserted it.
pub(crate) struct ResolvedCimdClient {
    pub(crate) client: RegisteredClient,
    pub(crate) created: bool,
}

enum ParsedRegistration {
    Cimd {
        client_id: String,
        requested_redirect_uris: Option<Vec<String>>,
    },
    Classic {
        redirect_uris: Vec<String>,
        client_name: Option<String>,
    },
}

/// Register a CIMD or classic public client.
pub(crate) async fn register(
    request: &HttpRequest,
    source: IpAddr,
    oauth: &OAuthRuntime,
    shutdown: &mut watch::Receiver<bool>,
) -> HttpResponse {
    match parse_registration(request) {
        Ok(ParsedRegistration::Cimd {
            client_id,
            requested_redirect_uris,
        }) => match resolve_or_register_cimd_client(
            oauth,
            source,
            &client_id,
            requested_redirect_uris,
            shutdown,
        )
        .await
        {
            Ok(resolved) => registration_outcome(resolved),
            Err(error) => map_cimd_error(error),
        },
        Ok(ParsedRegistration::Classic {
            redirect_uris,
            client_name,
        }) => complete_classic(
            oauth,
            source,
            redirect_uris,
            client_name,
            &SystemRandomSource,
        ),
        Err(response) => response,
    }
}

pub(crate) async fn register_with_io<IO: CimdAttemptIo>(
    request: &HttpRequest,
    source: IpAddr,
    oauth: &OAuthRuntime,
    shutdown: &mut watch::Receiver<bool>,
    io: IO,
    random: &dyn RandomSource,
) -> HttpResponse {
    match parse_registration(request) {
        Ok(ParsedRegistration::Cimd {
            client_id,
            requested_redirect_uris,
        }) => {
            match resolve_or_register_cimd_client_with_io(
                oauth,
                source,
                &client_id,
                requested_redirect_uris,
                shutdown,
                io,
                random,
            )
            .await
            {
                Ok(resolved) => registration_outcome(resolved),
                Err(error) => map_cimd_error(error),
            }
        }
        Ok(ParsedRegistration::Classic {
            redirect_uris,
            client_name,
        }) => complete_classic(oauth, source, redirect_uris, client_name, random),
        Err(response) => response,
    }
}

fn parse_registration(request: &HttpRequest) -> Result<ParsedRegistration, HttpResponse> {
    reject_non_identity_encoding(request)?;
    if request.body.len() > DCR_MAX_BODY_BYTES {
        return Err(HttpResponse::error(
            413,
            "Payload Too Large",
            "registration request exceeds its size limit",
        ));
    }
    let value: serde_json::Value =
        serde_json::from_slice(&request.body).map_err(|_| invalid_metadata())?;
    let object = value.as_object().ok_or_else(invalid_metadata)?;

    if let Some(client_id) = object.get("client_id") {
        let client_id = client_id.as_str().ok_or_else(invalid_metadata)?;
        if client_id.len() > MAX_CLIENT_ID_BYTES {
            return Err(invalid_metadata());
        }
    }
    if let Some(name) = object.get("client_name") {
        let name = name.as_str().ok_or_else(invalid_metadata)?;
        if name.len() > MAX_CLIENT_NAME_BYTES {
            return Err(invalid_metadata());
        }
    }
    if let Some(uris) = object.get("redirect_uris") {
        let uris = uris.as_array().ok_or_else(invalid_metadata)?;
        if uris.len() > MAX_REDIRECT_URIS_PER_CLIENT {
            return Err(invalid_metadata());
        }
        for uri in uris {
            let uri = uri.as_str().ok_or_else(invalid_metadata)?;
            if uri.len() > MAX_REDIRECT_URI_BYTES {
                return Err(invalid_metadata());
            }
        }
    }

    if object.contains_key("client_id") {
        let client_id = object
            .get("client_id")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(invalid_metadata)?;
        validate_cimd_url(client_id).map_err(|_| invalid_metadata())?;
        let requested_redirect_uris = match object.get("redirect_uris") {
            Some(uris) => Some(string_list(uris)?),
            None => None,
        };
        return Ok(ParsedRegistration::Cimd {
            client_id: client_id.to_owned(),
            requested_redirect_uris,
        });
    }

    if let Some(method) = object.get("token_endpoint_auth_method")
        && method.as_str() != Some("none")
    {
        return Err(invalid_metadata());
    }
    if let Some(grants) = object.get("grant_types")
        && !grant_types_allowed(grants)
    {
        return Err(invalid_metadata());
    }
    if let Some(responses) = object.get("response_types")
        && !is_exactly_code(responses)
    {
        return Err(invalid_metadata());
    }
    let redirect_uris = match object.get("redirect_uris") {
        Some(uris) => string_list(uris)?,
        None => return Err(invalid_metadata()),
    };
    if redirect_uris.is_empty() {
        return Err(invalid_metadata());
    }
    let client_name = match object.get("client_name") {
        Some(name) => Some(name.as_str().ok_or_else(invalid_metadata)?.to_owned()),
        None => None,
    };
    Ok(ParsedRegistration::Classic {
        redirect_uris,
        client_name,
    })
}

/// Fetch (if needed) and return the registered CIMD client for `client_id`.
pub(crate) async fn resolve_or_register_cimd_client(
    oauth: &OAuthRuntime,
    source: IpAddr,
    client_id: &str,
    requested_redirect_uris: Option<Vec<String>>,
    shutdown: &mut watch::Receiver<bool>,
) -> Result<ResolvedCimdClient, CimdRegistrationError> {
    if let Some(existing) = oauth
        .store
        .lookup_client_by_cimd_url(client_id)
        .map_err(CimdRegistrationError::Store)?
    {
        return Ok(ResolvedCimdClient {
            client: existing,
            created: false,
        });
    }
    #[cfg(feature = "full-tests")]
    if let Some((target, config)) = &oauth.cimd_fetch_override {
        let io = super::LoopbackCimdIo::new(*target, std::sync::Arc::clone(config));
        return match fetch_cimd_with_io(client_id, source, &oauth.cimd_bulkhead, shutdown, io).await
        {
            Ok(document) => complete_cimd_document(
                oauth,
                source,
                client_id,
                requested_redirect_uris,
                document,
                &SystemRandomSource,
            ),
            Err(_) => Err(CimdRegistrationError::Fetch),
        };
    }
    match fetch_cimd(client_id, source, &oauth.cimd_bulkhead, shutdown).await {
        Ok(document) => complete_cimd_document(
            oauth,
            source,
            client_id,
            requested_redirect_uris,
            document,
            &SystemRandomSource,
        ),
        Err(_) => Err(CimdRegistrationError::Fetch),
    }
}

pub(crate) async fn resolve_or_register_cimd_client_with_io<IO: CimdAttemptIo>(
    oauth: &OAuthRuntime,
    source: IpAddr,
    client_id: &str,
    requested_redirect_uris: Option<Vec<String>>,
    shutdown: &mut watch::Receiver<bool>,
    io: IO,
    random: &dyn RandomSource,
) -> Result<ResolvedCimdClient, CimdRegistrationError> {
    if let Some(existing) = oauth
        .store
        .lookup_client_by_cimd_url(client_id)
        .map_err(CimdRegistrationError::Store)?
    {
        return Ok(ResolvedCimdClient {
            client: existing,
            created: false,
        });
    }
    match fetch_cimd_with_io(client_id, source, &oauth.cimd_bulkhead, shutdown, io).await {
        Ok(document) => complete_cimd_document(
            oauth,
            source,
            client_id,
            requested_redirect_uris,
            document,
            random,
        ),
        Err(_) => Err(CimdRegistrationError::Fetch),
    }
}

fn complete_cimd_document(
    oauth: &OAuthRuntime,
    source: IpAddr,
    client_id: &str,
    requested_redirect_uris: Option<Vec<String>>,
    document: CimdDocument,
    random: &dyn RandomSource,
) -> Result<ResolvedCimdClient, CimdRegistrationError> {
    let selected = match requested_redirect_uris {
        Some(requested) => {
            if requested
                .iter()
                .any(|uri| !document.redirect_uris.iter().any(|listed| listed == uri))
            {
                return Err(CimdRegistrationError::RequestedRedirectNotInDocument);
            }
            requested
        }
        None => document.redirect_uris,
    };
    let redirect_uris = filter_redirect_uris(selected)?;
    let client_name = match document.client_name {
        Some(name) if name.len() > MAX_CLIENT_NAME_BYTES => {
            return Err(CimdRegistrationError::InvalidName);
        }
        other => other,
    };
    let existing = oauth
        .store
        .lookup_client_by_cimd_url(client_id)
        .map_err(CimdRegistrationError::Store)?;
    if let Some(existing) = existing {
        return Ok(ResolvedCimdClient {
            client: existing,
            created: false,
        });
    }
    match oauth.store.register_client_with_random(
        client_id,
        redirect_uris,
        client_name,
        &source_text(source),
        random,
    ) {
        Ok(created) => Ok(ResolvedCimdClient {
            client: created,
            created: true,
        }),
        Err(OAuthStoreError::Randomness) => Err(CimdRegistrationError::Randomness),
        Err(error) => Err(CimdRegistrationError::Store(error)),
    }
}

fn registration_outcome(resolved: ResolvedCimdClient) -> HttpResponse {
    if resolved.created {
        registration_response(201, "Created", &resolved.client)
    } else {
        registration_response(200, "OK", &resolved.client)
    }
}

fn map_cimd_error(error: CimdRegistrationError) -> HttpResponse {
    match error {
        CimdRegistrationError::Fetch => cimd_unavailable(),
        CimdRegistrationError::NoAllowlistedRedirect
        | CimdRegistrationError::RequestedRedirectNotInDocument
        | CimdRegistrationError::InvalidName => invalid_metadata(),
        CimdRegistrationError::Randomness => registration_unavailable(),
        CimdRegistrationError::Store(error) => store_error(error),
    }
}

fn complete_classic(
    oauth: &OAuthRuntime,
    source: IpAddr,
    redirect_uris: Vec<String>,
    client_name: Option<String>,
    random: &dyn RandomSource,
) -> HttpResponse {
    let redirect_uris = match filter_redirect_uris(redirect_uris) {
        Ok(uris) => uris,
        Err(_) => return invalid_metadata(),
    };
    let client_id = match mint_client_id(random) {
        Ok(client_id) => client_id,
        Err(response) => return response,
    };
    match oauth.store.register_client_with_random(
        &client_id,
        redirect_uris,
        client_name,
        &source_text(source),
        random,
    ) {
        Ok(created) => registration_response(201, "Created", &created),
        Err(error) => store_error(error),
    }
}

fn filter_redirect_uris(uris: Vec<String>) -> Result<Vec<String>, CimdRegistrationError> {
    let filtered: Vec<String> = uris
        .into_iter()
        .filter(|uri| parse_redirect_uri(uri).is_ok())
        .collect();
    if filtered.is_empty() || filtered.len() > MAX_REDIRECT_URIS_PER_CLIENT {
        return Err(CimdRegistrationError::NoAllowlistedRedirect);
    }
    Ok(filtered)
}

fn mint_client_id(random: &dyn RandomSource) -> Result<String, HttpResponse> {
    let mut bytes = [0_u8; CLIENT_ID_BYTES];
    let written = random
        .fill(&mut bytes)
        .map_err(|_| registration_unavailable())?;
    if written != bytes.len() {
        return Err(registration_unavailable());
    }
    Ok(format!("oauth:dcr:{}", URL_SAFE_NO_PAD.encode(bytes)))
}

fn string_list(value: &serde_json::Value) -> Result<Vec<String>, HttpResponse> {
    let items = value.as_array().ok_or_else(invalid_metadata)?;
    let mut uris = Vec::with_capacity(items.len());
    for item in items {
        uris.push(item.as_str().ok_or_else(invalid_metadata)?.to_owned());
    }
    Ok(uris)
}

fn grant_types_allowed(value: &serde_json::Value) -> bool {
    let Some(items) = value.as_array() else {
        return false;
    };
    let mut has_code = false;
    for item in items {
        match item.as_str() {
            Some("authorization_code") => has_code = true,
            Some("refresh_token") => {}
            _ => return false,
        }
    }
    has_code
}

fn is_exactly_code(value: &serde_json::Value) -> bool {
    matches!(
        value.as_array(),
        Some(items) if items.len() == 1 && items[0].as_str() == Some("code")
    )
}

fn source_text(source: IpAddr) -> String {
    canonicalize_ip(source).to_string()
}

fn registration_response(
    status: u16,
    reason: &'static str,
    client: &RegisteredClient,
) -> HttpResponse {
    let body = RegistrationBody {
        client_id: &client.client_id,
        redirect_uris: &client.redirect_uris,
        client_name: client.client_name.as_deref(),
        token_endpoint_auth_method: "none",
        grant_types: ["authorization_code", "refresh_token"],
        response_types: ["code"],
        client_id_issued_at: client.created_at.timestamp(),
    };
    HttpResponse::json(
        status,
        reason,
        serde_json::to_vec(&body).expect("registration JSON serializes"),
    )
}

#[derive(Serialize)]
struct RegistrationBody<'a> {
    client_id: &'a str,
    redirect_uris: &'a [String],
    #[serde(skip_serializing_if = "Option::is_none")]
    client_name: Option<&'a str>,
    token_endpoint_auth_method: &'static str,
    grant_types: [&'static str; 2],
    response_types: [&'static str; 1],
    client_id_issued_at: i64,
}

fn invalid_metadata() -> HttpResponse {
    HttpResponse::error(400, "Bad Request", "registration metadata is invalid")
}

fn cimd_unavailable() -> HttpResponse {
    HttpResponse::error(400, "Bad Request", "client metadata could not be retrieved")
}

fn registration_unavailable() -> HttpResponse {
    HttpResponse::error(
        503,
        "Service Unavailable",
        "OAuth registration is unavailable",
    )
}

fn store_error(error: OAuthStoreError) -> HttpResponse {
    match error {
        OAuthStoreError::Quota => HttpResponse::error(
            429,
            "Too Many Requests",
            "OAuth registration is unavailable",
        ),
        _ => registration_unavailable(),
    }
}

#[cfg(all(test, not(feature = "full-tests")))]
mod tests {
    use std::io;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};
    use std::path::Path;
    use std::pin::Pin;
    use std::task::{Context, Poll};

    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    use sha2::{Digest, Sha256};
    use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
    use tokio::sync::watch;

    use super::register_with_io;
    use crate::http1::{HttpMethod, HttpRequest, HttpResponse};
    use crate::oauth::cimd::CimdAttemptIo;
    use crate::oauth::store::OAuthStore;
    use crate::oauth::{
        DCR_MAX_BODY_BYTES, MAX_CLIENT_ID_BYTES, MAX_CLIENT_NAME_BYTES, MAX_REDIRECT_URI_BYTES,
        MAX_REDIRECT_URIS_PER_CLIENT, OAuthRuntime,
    };
    use crate::tokens::{RandomSource, RandomSourceError, SystemRandomSource, TokenStore};

    const CIMD_URL: &str = "https://client.example/cimd.json";
    const SOURCE: IpAddr = IpAddr::V4(Ipv4Addr::new(198, 51, 100, 10));
    const ORIGIN: &str = "https://mcp.test";

    fn journal_root() -> tempfile::TempDir {
        tempfile::Builder::new()
            .prefix("solstone-mcp-dcr-")
            .tempdir_in("/var/tmp")
            .unwrap()
    }

    fn runtime(journal: &tempfile::TempDir) -> OAuthRuntime {
        OAuthRuntime::new(journal.path(), ORIGIN.to_owned())
    }

    fn json_request(body: &str) -> HttpRequest {
        HttpRequest::from_test_parts(HttpMethod::Post, Vec::new(), body.as_bytes().to_vec())
    }

    fn request_with_headers(headers: Vec<(String, String)>, body: Vec<u8>) -> HttpRequest {
        HttpRequest::from_test_parts(HttpMethod::Post, headers, body)
    }

    fn public_addr() -> SocketAddr {
        SocketAddr::from((Ipv4Addr::new(8, 8, 8, 8), 443))
    }

    fn cimd_body(url: &str, redirects: &[&str], name: Option<&str>) -> String {
        let redirects = redirects
            .iter()
            .map(|uri| format!("\"{uri}\""))
            .collect::<Vec<_>>()
            .join(",");
        match name {
            Some(name) => format!(
                r#"{{"client_id":"{url}","redirect_uris":[{redirects}],"client_name":"{name}"}}"#
            ),
            None => format!(r#"{{"client_id":"{url}","redirect_uris":[{redirects}]}}"#),
        }
    }

    fn http_document(body: &str) -> Vec<u8> {
        format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        )
        .into_bytes()
    }

    struct FakeConnection {
        data: Vec<u8>,
        offset: usize,
    }

    impl AsyncRead for FakeConnection {
        fn poll_read(
            mut self: Pin<&mut Self>,
            _context: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            if self.offset >= self.data.len() {
                return Poll::Ready(Ok(()));
            }
            let remaining = self.data.len() - self.offset;
            let take = remaining.min(buf.remaining());
            buf.put_slice(&self.data[self.offset..self.offset + take]);
            self.offset += take;
            Poll::Ready(Ok(()))
        }
    }

    impl AsyncWrite for FakeConnection {
        fn poll_write(
            self: Pin<&mut Self>,
            _context: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<io::Result<usize>> {
            Poll::Ready(Ok(buf.len()))
        }

        fn poll_flush(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }

        fn poll_shutdown(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    struct FakeIo {
        addresses: Vec<SocketAddr>,
        response: Vec<u8>,
        fail_tls: bool,
    }

    impl FakeIo {
        fn document(redirects: &[&str], name: Option<&str>) -> Self {
            Self {
                addresses: vec![public_addr()],
                response: http_document(&cimd_body(CIMD_URL, redirects, name)),
                fail_tls: false,
            }
        }

        fn fail() -> Self {
            Self {
                addresses: vec![public_addr()],
                response: Vec::new(),
                fail_tls: true,
            }
        }
    }

    impl CimdAttemptIo for FakeIo {
        type Socket = ();
        type Connection = FakeConnection;

        async fn resolve(&mut self, _host: &str, _port: u16) -> io::Result<Vec<SocketAddr>> {
            Ok(self.addresses.clone())
        }

        async fn connect(&mut self, _address: SocketAddr) -> io::Result<Self::Socket> {
            Ok(())
        }

        async fn tls(
            &mut self,
            _socket: Self::Socket,
            _server_name: &str,
        ) -> io::Result<Self::Connection> {
            if self.fail_tls {
                return Err(io::Error::other("CIMD TLS failed"));
            }
            Ok(FakeConnection {
                data: self.response.clone(),
                offset: 0,
            })
        }
    }

    struct UnusedIo;

    impl CimdAttemptIo for UnusedIo {
        type Socket = ();
        type Connection = FakeConnection;

        async fn resolve(&mut self, _host: &str, _port: u16) -> io::Result<Vec<SocketAddr>> {
            Err(io::Error::other("classic DCR must not fetch CIMD"))
        }

        async fn connect(&mut self, _address: SocketAddr) -> io::Result<Self::Socket> {
            Err(io::Error::other("classic DCR must not fetch CIMD"))
        }

        async fn tls(
            &mut self,
            _socket: Self::Socket,
            _server_name: &str,
        ) -> io::Result<Self::Connection> {
            Err(io::Error::other("classic DCR must not fetch CIMD"))
        }
    }

    struct ShortRandom;

    impl RandomSource for ShortRandom {
        fn fill(&self, bytes: &mut [u8]) -> Result<usize, RandomSourceError> {
            let count = bytes.len().saturating_sub(1);
            bytes[..count].fill(0x5a);
            Ok(count)
        }
    }

    async fn register_json(oauth: &OAuthRuntime, io: FakeIo, body: &str) -> HttpResponse {
        let (_tx, mut shutdown) = watch::channel(false);
        register_with_io(
            &json_request(body),
            SOURCE,
            oauth,
            &mut shutdown,
            io,
            &SystemRandomSource,
        )
        .await
    }

    async fn register_classic(oauth: &OAuthRuntime, body: &str) -> HttpResponse {
        let (_tx, mut shutdown) = watch::channel(false);
        register_with_io(
            &json_request(body),
            SOURCE,
            oauth,
            &mut shutdown,
            UnusedIo,
            &SystemRandomSource,
        )
        .await
    }

    fn json_value(response: &HttpResponse) -> serde_json::Value {
        serde_json::from_slice(&response.body).expect("registration body is JSON")
    }

    fn oauth_path(journal: &Path) -> std::path::PathBuf {
        journal.join("mcp-endpoint/oauth.json")
    }

    fn pkce_challenge() -> String {
        URL_SAFE_NO_PAD.encode(Sha256::digest(b"pkce-verifier"))
    }

    fn redeem(store: &OAuthStore, client_record_id: &str, client_id: &str) -> String {
        let pairing = store.generate_pairing_code().unwrap();
        let transaction = store
            .create_transaction(
                client_record_id,
                "http://127.0.0.1/callback",
                "https://mcp.test/mcp",
                ORIGIN,
                &pkce_challenge(),
                "S256",
                Some("state-1"),
                "192.0.2.1",
            )
            .unwrap();
        let issued = store.complete_pairing(&transaction, &pairing.code).unwrap();
        store
            .redeem_authorization_code(
                &issued.code,
                client_id,
                "http://127.0.0.1/callback",
                "https://mcp.test/mcp",
                "pkce-verifier",
            )
            .unwrap()
            .access_token
    }

    #[tokio::test(start_paused = true)]
    async fn cimd_registration_uses_the_document_url() {
        let journal = journal_root();
        let oauth = runtime(&journal);
        let body = format!(r#"{{"client_id":"{CIMD_URL}"}}"#);
        let response = register_json(
            &oauth,
            FakeIo::document(&["http://127.0.0.1/callback"], Some("fixture")),
            &body,
        )
        .await;
        assert_eq!(response.status, 201);
        let json = json_value(&response);
        assert_eq!(json["client_id"], CIMD_URL);
        assert_eq!(
            json["redirect_uris"],
            serde_json::json!(["http://127.0.0.1/callback"])
        );
        assert_eq!(json["client_name"], "fixture");
        assert_eq!(json["token_endpoint_auth_method"], "none");
        let stored = oauth
            .store
            .lookup_client_by_cimd_url(CIMD_URL)
            .unwrap()
            .expect("CIMD client persisted");
        assert_eq!(stored.client_id, CIMD_URL);
    }

    #[tokio::test(start_paused = true)]
    async fn repeating_the_same_cimd_url_returns_the_existing_record() {
        let journal = journal_root();
        let oauth = runtime(&journal);
        let body = format!(r#"{{"client_id":"{CIMD_URL}"}}"#);
        let first = register_json(
            &oauth,
            FakeIo::document(&["http://127.0.0.1/callback"], Some("fixture")),
            &body,
        )
        .await;
        let (_tx, mut shutdown) = watch::channel(false);
        let second = register_with_io(
            &json_request(&body),
            SOURCE,
            &oauth,
            &mut shutdown,
            UnusedIo,
            &SystemRandomSource,
        )
        .await;
        assert_eq!(first.status, 201);
        assert_eq!(second.status, 200);
        assert_eq!(
            json_value(&first)["client_id"],
            json_value(&second)["client_id"]
        );
        assert_eq!(
            json_value(&first)["client_id_issued_at"],
            json_value(&second)["client_id_issued_at"]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn cimd_fetch_failure_does_not_persist_a_client() {
        let journal = journal_root();
        let oauth = runtime(&journal);
        let body = format!(r#"{{"client_id":"{CIMD_URL}"}}"#);
        let response = register_json(&oauth, FakeIo::fail(), &body).await;
        assert_eq!(response.status, 400);
        assert!(
            oauth
                .store
                .lookup_client_by_cimd_url(CIMD_URL)
                .unwrap()
                .is_none()
        );
        assert!(!oauth_path(journal.path()).exists());
    }

    #[tokio::test(start_paused = true)]
    async fn requested_redirect_outside_the_document_is_refused() {
        let journal = journal_root();
        let oauth = runtime(&journal);
        let body =
            format!(r#"{{"client_id":"{CIMD_URL}","redirect_uris":["http://127.0.0.1/other"]}}"#);
        let response = register_json(
            &oauth,
            FakeIo::document(&["http://127.0.0.1/callback"], None),
            &body,
        )
        .await;
        assert_eq!(response.status, 400);
        assert!(
            oauth
                .store
                .lookup_client_by_cimd_url(CIMD_URL)
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test(start_paused = true)]
    async fn non_allowlisted_redirects_are_dropped_when_one_survives() {
        let journal = journal_root();
        let oauth = runtime(&journal);
        let body = format!(r#"{{"client_id":"{CIMD_URL}"}}"#);
        let response = register_json(
            &oauth,
            FakeIo::document(
                &["http://127.0.0.1/callback", "https://evil.example/cb"],
                None,
            ),
            &body,
        )
        .await;
        assert_eq!(response.status, 201);
        assert_eq!(
            json_value(&response)["redirect_uris"],
            serde_json::json!(["http://127.0.0.1/callback"])
        );
    }

    #[tokio::test(start_paused = true)]
    async fn all_non_allowlisted_redirects_are_refused() {
        let journal = journal_root();
        let oauth = runtime(&journal);
        let body = format!(r#"{{"client_id":"{CIMD_URL}"}}"#);
        let response = register_json(
            &oauth,
            FakeIo::document(&["https://evil.example/cb"], None),
            &body,
        )
        .await;
        assert_eq!(response.status, 400);
        assert!(
            oauth
                .store
                .lookup_client_by_cimd_url(CIMD_URL)
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test(start_paused = true)]
    async fn classic_registration_mints_a_fresh_prefixed_id_each_call() {
        let journal = journal_root();
        let oauth = runtime(&journal);
        let body = r#"{"redirect_uris":["http://127.0.0.1/callback"]}"#;
        let first = register_classic(&oauth, body).await;
        let second = register_classic(&oauth, body).await;
        assert_eq!(first.status, 201);
        assert_eq!(second.status, 201);
        let first_id = json_value(&first)["client_id"].as_str().unwrap().to_owned();
        let second_id = json_value(&second)["client_id"]
            .as_str()
            .unwrap()
            .to_owned();
        assert!(first_id.starts_with("oauth:dcr:"));
        assert!(second_id.starts_with("oauth:dcr:"));
        assert_ne!(first_id, second_id);
    }

    #[tokio::test(start_paused = true)]
    async fn classic_registration_requires_redirect_uris() {
        let journal = journal_root();
        let oauth = runtime(&journal);
        let response = register_classic(&oauth, "{}").await;
        assert_eq!(response.status, 400);
        assert!(!oauth_path(journal.path()).exists());
    }

    #[tokio::test(start_paused = true)]
    async fn classic_registration_rejects_a_confidential_auth_method() {
        let journal = journal_root();
        let oauth = runtime(&journal);
        let response = register_classic(
            &oauth,
            r#"{"redirect_uris":["http://127.0.0.1/callback"],"token_endpoint_auth_method":"client_secret_basic"}"#,
        )
        .await;
        assert_eq!(response.status, 400);
        assert!(!oauth_path(journal.path()).exists());
    }

    #[tokio::test(start_paused = true)]
    async fn classic_registration_rejects_a_disallowed_grant_type() {
        let journal = journal_root();
        let oauth = runtime(&journal);
        let response = register_classic(
            &oauth,
            r#"{"redirect_uris":["http://127.0.0.1/callback"],"grant_types":["authorization_code","password"]}"#,
        )
        .await;
        assert_eq!(response.status, 400);
        assert!(!oauth_path(journal.path()).exists());
    }

    #[tokio::test(start_paused = true)]
    async fn classic_registration_rejects_more_than_sixteen_redirects() {
        let journal = journal_root();
        let oauth = runtime(&journal);
        let uris = (0..=MAX_REDIRECT_URIS_PER_CLIENT)
            .map(|index| format!("\"http://127.0.0.1/cb{index}\""))
            .collect::<Vec<_>>()
            .join(",");
        let body = format!(r#"{{"redirect_uris":[{uris}]}}"#);
        let response = register_classic(&oauth, &body).await;
        assert_eq!(response.status, 400);
        assert!(!oauth_path(journal.path()).exists());
    }

    #[tokio::test(start_paused = true)]
    async fn short_read_randomness_persists_nothing() {
        let journal = journal_root();
        let oauth = runtime(&journal);
        let (_tx, mut shutdown) = watch::channel(false);
        let response = register_with_io(
            &json_request(r#"{"redirect_uris":["http://127.0.0.1/callback"]}"#),
            SOURCE,
            &oauth,
            &mut shutdown,
            UnusedIo,
            &ShortRandom,
        )
        .await;
        assert_eq!(response.status, 503);
        assert!(!oauth_path(journal.path()).exists());
    }

    #[tokio::test(start_paused = true)]
    async fn oversize_body_is_refused_before_parsing() {
        let journal = journal_root();
        let oauth = runtime(&journal);
        let mut body = br#"{"redirect_uris":["http://127.0.0.1/callback"],"pad":""#.to_vec();
        body.resize(DCR_MAX_BODY_BYTES + 1, b'x');
        let (_tx, mut shutdown) = watch::channel(false);
        let response = register_with_io(
            &request_with_headers(Vec::new(), body),
            SOURCE,
            &oauth,
            &mut shutdown,
            UnusedIo,
            &SystemRandomSource,
        )
        .await;
        assert_eq!(response.status, 413);
        assert!(!oauth_path(journal.path()).exists());
    }

    #[tokio::test(start_paused = true)]
    async fn non_identity_encoding_is_refused_before_the_body() {
        let journal = journal_root();
        let oauth = runtime(&journal);
        let (_tx, mut shutdown) = watch::channel(false);
        let response = register_with_io(
            &request_with_headers(
                vec![("Content-Encoding".to_owned(), "gzip".to_owned())],
                br#"{"redirect_uris":["http://127.0.0.1/callback"]}"#.to_vec(),
            ),
            SOURCE,
            &oauth,
            &mut shutdown,
            UnusedIo,
            &SystemRandomSource,
        )
        .await;
        assert_eq!(response.status, 400);
        assert!(!oauth_path(journal.path()).exists());
    }

    #[tokio::test(start_paused = true)]
    async fn oversize_fields_are_refused() {
        let journal = journal_root();
        let oauth = runtime(&journal);
        let oversize_id = format!(
            "https://client.example/{}/cimd.json",
            "a".repeat(MAX_CLIENT_ID_BYTES)
        );
        let oversize_uri = format!("http://127.0.0.1/{}", "a".repeat(MAX_REDIRECT_URI_BYTES));
        let oversize_name = "n".repeat(MAX_CLIENT_NAME_BYTES + 1);
        for body in [
            format!(r#"{{"client_id":"{oversize_id}"}}"#),
            format!(r#"{{"redirect_uris":["{oversize_uri}"]}}"#),
            format!(
                r#"{{"redirect_uris":["http://127.0.0.1/callback"],"client_name":"{oversize_name}"}}"#
            ),
        ] {
            let response = register_classic(&oauth, &body).await;
            assert_eq!(response.status, 400, "{body}");
            assert!(!oauth_path(journal.path()).exists());
        }
    }

    #[tokio::test(start_paused = true)]
    async fn registered_identities_are_disjoint_from_static_labels() {
        let journal = journal_root();
        let oauth = runtime(&journal);
        let cimd = register_json(
            &oauth,
            FakeIo::document(&["http://127.0.0.1/callback"], Some("fixture")),
            &format!(r#"{{"client_id":"{CIMD_URL}"}}"#),
        )
        .await;
        let classic =
            register_classic(&oauth, r#"{"redirect_uris":["http://127.0.0.1/callback"]}"#).await;
        let cimd_id = json_value(&cimd)["client_id"].as_str().unwrap().to_owned();
        let classic_id = json_value(&classic)["client_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let cimd_record = oauth
            .store
            .lookup_client_by_cimd_url(&cimd_id)
            .unwrap()
            .unwrap();
        let classic_record = oauth
            .store
            .lookup_client_by_cimd_url(&classic_id)
            .unwrap()
            .unwrap();
        let cimd_token = redeem(&oauth.store, &cimd_record.id, &cimd_id);
        let classic_token = redeem(&oauth.store, &classic_record.id, &classic_id);
        let cimd_verified = oauth.store.verify_access_token(&cimd_token).unwrap();
        let classic_verified = oauth.store.verify_access_token(&classic_token).unwrap();
        assert_eq!(cimd_verified.agent_identity, CIMD_URL);
        assert_eq!(classic_verified.agent_identity, classic_id);
        assert!(classic_verified.agent_identity.starts_with("oauth:dcr:"));
        assert!(
            !classic_verified
                .agent_identity
                .starts_with("oauth:dcr:oauth:dcr:")
        );
        assert!(cimd_verified.agent_identity.contains(':'));
        assert!(classic_verified.agent_identity.contains(':'));
        let tokens = TokenStore::open(journal.path());
        assert!(tokens.create(&cimd_verified.agent_identity).is_err());
        assert!(tokens.create(&classic_verified.agent_identity).is_err());
    }

    #[test]
    fn parse_rejects_a_malformed_present_client_id() {
        let request = json_request(r#"{"client_id":"http://client.example/cimd.json"}"#);
        assert!(matches!(
            super::parse_registration(&request),
            Err(response) if response.status == 400
        ));
    }
}
