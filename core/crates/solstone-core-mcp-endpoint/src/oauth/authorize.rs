// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Authorization endpoint for journal-hosted MCP OAuth.

use std::net::IpAddr;

use tokio::sync::watch;

#[cfg(test)]
use super::cimd::CimdAttemptIo;
use super::cimd::canonicalize_ip;
#[cfg(test)]
use super::dcr::resolve_or_register_cimd_client_with_io;
use super::dcr::{CimdRegistrationError, resolve_or_register_cimd_client};
use super::rate_limit::PairingFailureRecord;
use super::redirect::{RedirectHost, parse_redirect_uri, redirect_uri_is_allowed};
use super::store::{IssuedAuthorization, OAuthStoreError, RegisteredClient};
use super::urlparse::{query_value_encode, validate_cimd_url};
use super::{
    AUTHORIZE_MAX_BODY_BYTES, MAX_CLIENT_ID_BYTES, MAX_CODE_BYTES, MAX_STATE_BYTES, OAuthRuntime,
    parse_urlencoded_pairs, reject_non_identity_encoding,
};
use crate::http1::{HttpRequest, HttpResponse};
#[cfg(test)]
use crate::tokens::RandomSource;

const AUTHORIZE_CSP: &str = "default-src 'none'; form-action 'self'; frame-ancestors 'none'";

/// GET `/authorize`.
pub(crate) async fn get_authorize(
    request: &HttpRequest,
    source: IpAddr,
    oauth: &OAuthRuntime,
    shutdown: &mut watch::Receiver<bool>,
) -> HttpResponse {
    let Some(pairs) = parse_get_pairs(request) else {
        return local_error("authorization request could not be started");
    };
    let Some(client_id) = field(&pairs, "client_id").filter(|value| !value.is_empty()) else {
        return local_error("authorization request could not be started");
    };
    let client = match resolve_live(oauth, source, client_id, shutdown).await {
        Ok(client) => client,
        Err(_) => return local_error("authorization request could not be started"),
    };
    finish_authorize_get(&pairs, client, source, oauth)
}

/// GET `/authorize` with injected CIMD I/O.
#[cfg(test)]
pub(crate) async fn get_authorize_with_io<IO: CimdAttemptIo>(
    request: &HttpRequest,
    source: IpAddr,
    oauth: &OAuthRuntime,
    shutdown: &mut watch::Receiver<bool>,
    io: IO,
    random: &dyn RandomSource,
) -> HttpResponse {
    let Some(pairs) = parse_get_pairs(request) else {
        return local_error("authorization request could not be started");
    };
    let Some(client_id) = field(&pairs, "client_id").filter(|value| !value.is_empty()) else {
        return local_error("authorization request could not be started");
    };
    let client =
        match resolve_authorize_client(oauth, source, client_id, shutdown, io, random).await {
            Ok(client) => client,
            Err(_) => return local_error("authorization request could not be started"),
        };
    finish_authorize_get(&pairs, client, source, oauth)
}

fn parse_get_pairs(request: &HttpRequest) -> Option<Vec<(String, String)>> {
    let pairs = parse_urlencoded_pairs(query_from_target(&request.target))?;
    if oversize_authorize_get_fields(&pairs) {
        return None;
    }
    Some(pairs)
}

fn finish_authorize_get(
    pairs: &[(String, String)],
    client: RegisteredClient,
    source: IpAddr,
    oauth: &OAuthRuntime,
) -> HttpResponse {
    let Some(redirect_uri) = field(pairs, "redirect_uri").filter(|value| !value.is_empty()) else {
        return local_error("authorization request could not be started");
    };
    if !redirect_uri_is_allowed(redirect_uri, &client.redirect_uris) {
        return local_error("authorization request could not be started");
    }
    let state = field(pairs, "state");
    let expected_resource = canonical_resource(oauth);
    if field(pairs, "response_type") != Some("code")
        || field(pairs, "code_challenge_method") != Some("S256")
        || field(pairs, "code_challenge").is_none_or(|value| value.is_empty())
        || field(pairs, "resource") != Some(expected_resource.as_str())
    {
        return error_redirect(redirect_uri, "invalid_request", state);
    }
    let code_challenge = field(pairs, "code_challenge").expect("challenge present");
    match oauth.store.create_transaction(
        &client.id,
        redirect_uri,
        field(pairs, "resource").expect("resource present"),
        &oauth.resource_origin,
        code_challenge,
        "S256",
        state,
        &canonicalize_ip(source).to_string(),
    ) {
        Ok(transaction_id) => consent_page(&client, redirect_uri, &transaction_id),
        Err(OAuthStoreError::Quota) => {
            error_redirect(redirect_uri, "temporarily_unavailable", state)
        }
        Err(_) => error_redirect(redirect_uri, "server_error", state),
    }
}

/// POST `/authorize`.
pub(crate) fn post_authorize(
    request: &HttpRequest,
    source: IpAddr,
    oauth: &OAuthRuntime,
) -> HttpResponse {
    if let Err(response) = reject_non_identity_encoding(request) {
        return response;
    }
    if request.body.len() > AUTHORIZE_MAX_BODY_BYTES {
        return HttpResponse::error(
            413,
            "Payload Too Large",
            "authorization request exceeds its size limit",
        );
    }
    let Ok(body) = std::str::from_utf8(&request.body) else {
        return local_error("authorization is temporarily unavailable");
    };
    let Some(pairs) = parse_urlencoded_pairs(body) else {
        return local_error("authorization is temporarily unavailable");
    };
    let transaction_id = field(&pairs, "transaction_id").unwrap_or("");
    let pairing_code = field(&pairs, "pairing_code").unwrap_or("");
    if transaction_id.is_empty()
        || pairing_code.is_empty()
        || transaction_id.len() > MAX_CODE_BYTES
        || pairing_code.len() > MAX_CODE_BYTES
    {
        return local_error(
            "this authorization request has expired or is no longer valid; restart it from the client",
        );
    }
    let generation = match oauth.store.pairing_generation() {
        Ok(generation) => generation,
        Err(_) => return local_error("authorization is temporarily unavailable"),
    };
    oauth.pairing_limiter.prune_generation(generation);
    if oauth.pairing_limiter.is_limited(source, generation) {
        return html_status(
            429,
            "Too Many Requests",
            "<p>wait and try again, or ask the owner for a new code</p>",
        );
    }
    match oauth.store.complete_pairing(transaction_id, pairing_code) {
        Ok(issued) => success_redirect(&issued),
        Err(OAuthStoreError::PairingMismatch) => {
            if oauth.pairing_limiter.record_failure(source, generation)
                == PairingFailureRecord::JustTripped
            {
                let _ = oauth.store.lock_pairing_code();
            }
            let still_pending = oauth
                .store
                .pending_transaction_exists(transaction_id)
                .unwrap_or(false);
            if still_pending {
                retry_page(transaction_id)
            } else {
                local_error(
                    "too many attempts for this request; restart authorization from the client",
                )
            }
        }
        Err(OAuthStoreError::PairingLocked) => local_error(
            "pairing is temporarily locked; ask the owner to run `journal mcp pairing generate`.",
        ),
        Err(
            OAuthStoreError::TransactionNotFound
            | OAuthStoreError::TransactionExpired
            | OAuthStoreError::TransactionExhausted,
        ) => local_error(
            "this authorization request has expired or is no longer valid; restart it from the client",
        ),
        Err(OAuthStoreError::NoActivePairing) => {
            local_error("no active pairing code; ask the owner to generate one.")
        }
        Err(_) => local_error("authorization is temporarily unavailable"),
    }
}

#[cfg(test)]
async fn resolve_authorize_client<IO: CimdAttemptIo>(
    oauth: &OAuthRuntime,
    source: IpAddr,
    client_id: &str,
    shutdown: &mut watch::Receiver<bool>,
    io: IO,
    random: &dyn RandomSource,
) -> Result<RegisteredClient, CimdRegistrationError> {
    if validate_cimd_url(client_id).is_ok() {
        resolve_or_register_cimd_client_with_io(
            oauth, source, client_id, None, shutdown, io, random,
        )
        .await
        .map(|resolved| resolved.client)
    } else {
        match oauth.store.lookup_client_by_cimd_url(client_id) {
            Ok(Some(client)) => Ok(client),
            Ok(None) => Err(CimdRegistrationError::Fetch),
            Err(error) => Err(CimdRegistrationError::Store(error)),
        }
    }
}

async fn resolve_live(
    oauth: &OAuthRuntime,
    source: IpAddr,
    client_id: &str,
    shutdown: &mut watch::Receiver<bool>,
) -> Result<RegisteredClient, CimdRegistrationError> {
    if validate_cimd_url(client_id).is_ok() {
        resolve_or_register_cimd_client(oauth, source, client_id, None, shutdown)
            .await
            .map(|resolved| resolved.client)
    } else {
        match oauth.store.lookup_client_by_cimd_url(client_id) {
            Ok(Some(client)) => Ok(client),
            Ok(None) => Err(CimdRegistrationError::Fetch),
            Err(error) => Err(CimdRegistrationError::Store(error)),
        }
    }
}

fn oversize_authorize_get_fields(pairs: &[(String, String)]) -> bool {
    for (name, value) in pairs {
        let cap = match name.as_str() {
            "client_id" | "redirect_uri" | "resource" => MAX_CLIENT_ID_BYTES,
            "state" => MAX_STATE_BYTES,
            "code_challenge" | "transaction_id" | "pairing_code" => MAX_CODE_BYTES,
            _ => continue,
        };
        if value.len() > cap {
            return true;
        }
    }
    false
}

fn field<'a>(pairs: &'a [(String, String)], name: &str) -> Option<&'a str> {
    pairs
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.as_str())
}

fn query_from_target(target: &str) -> &str {
    match target.split_once('?') {
        Some((_, query)) => query
            .split_once('#')
            .map(|(query, _)| query)
            .unwrap_or(query),
        None => "",
    }
}

fn canonical_resource(oauth: &OAuthRuntime) -> String {
    format!("{}/mcp", oauth.resource_origin)
}

fn html_escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for char in value.chars() {
        match char {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            other => escaped.push(other),
        }
    }
    escaped
}

fn html_status(status: u16, reason: &'static str, body: &str) -> HttpResponse {
    HttpResponse::html(
        status,
        reason,
        format!("<!DOCTYPE html><html><body>{body}</body></html>"),
    )
    .with_header("Cache-Control", "no-store".to_owned())
    .with_header("Content-Security-Policy", AUTHORIZE_CSP.to_owned())
    .with_header("X-Frame-Options", "DENY".to_owned())
}

fn local_error(message: &str) -> HttpResponse {
    html_status(
        400,
        "Bad Request",
        &format!("<p>{}</p>", html_escape(message)),
    )
}

fn retry_page(transaction_id: &str) -> HttpResponse {
    html_status(
        400,
        "Bad Request",
        &format!(
            "<p>incorrect pairing code, try again</p>\
<form method=\"post\" action=\"/authorize\">\
<input type=\"hidden\" name=\"transaction_id\" value=\"{}\">\
<input type=\"text\" name=\"pairing_code\">\
<button type=\"submit\">Authorize</button>\
</form>",
            html_escape(transaction_id)
        ),
    )
}

fn consent_page(
    client: &RegisteredClient,
    redirect_uri: &str,
    transaction_id: &str,
) -> HttpResponse {
    let host = parse_redirect_uri(redirect_uri)
        .map(|parsed| redirect_host_label(&parsed))
        .unwrap_or_else(|_| "unknown".to_owned());
    let client_label = client
        .client_name
        .as_deref()
        .unwrap_or(client.client_id.as_str());
    html_status(
        200,
        "OK",
        &format!(
            "<p>Redirect host: <strong>{}</strong></p>\
<p>Client: {}</p>\
<form method=\"post\" action=\"/authorize\">\
<input type=\"hidden\" name=\"transaction_id\" value=\"{}\">\
<label>Pairing code <input type=\"text\" name=\"pairing_code\" autocomplete=\"off\"></label>\
<button type=\"submit\">Authorize</button>\
</form>",
            html_escape(&host),
            html_escape(client_label),
            html_escape(transaction_id)
        ),
    )
}

fn redirect_host_label(parsed: &super::redirect::ParsedRedirectUri) -> String {
    let host = match parsed.host {
        RedirectHost::Localhost => "localhost",
        RedirectHost::V4Loopback => "127.0.0.1",
        RedirectHost::Claude => "claude.ai",
    };
    match parsed.port {
        Some(port) => format!("{host}:{port}"),
        None => host.to_owned(),
    }
}

fn error_redirect(redirect_uri: &str, error: &str, state: Option<&str>) -> HttpResponse {
    let mut params = vec![("error", error)];
    if let Some(state) = state {
        params.push(("state", state));
    }
    redirect_to(redirect_uri, &params)
}

fn success_redirect(issued: &IssuedAuthorization) -> HttpResponse {
    let mut params = vec![
        ("code", issued.code.as_str()),
        ("iss", issued.issuer.as_str()),
    ];
    if let Some(state) = issued.state.as_deref() {
        params.push(("state", state));
    }
    redirect_to(&issued.redirect_uri, &params)
}

fn redirect_to(base: &str, params: &[(&str, &str)]) -> HttpResponse {
    let mut location = base.to_owned();
    let mut separator = if base.contains('?') { '&' } else { '?' };
    for (name, value) in params {
        location.push(separator);
        separator = '&';
        location.push_str(&query_value_encode(name));
        location.push('=');
        location.push_str(&query_value_encode(value));
    }
    HttpResponse::empty(302, "Found")
        .with_header("Location", location)
        .with_header("Cache-Control", "no-store".to_owned())
}

#[cfg(all(test, not(feature = "full-tests")))]
mod tests {
    use std::io;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};
    use std::pin::Pin;
    use std::task::{Context, Poll};

    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    use sha2::{Digest, Sha256};
    use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
    use tokio::sync::watch;

    use super::{get_authorize_with_io, post_authorize};
    use crate::http1::{HttpMethod, HttpRequest, HttpResponse};
    use crate::oauth::cimd::CimdAttemptIo;
    use crate::oauth::urlparse::query_value_encode;
    use crate::oauth::{AUTHORIZE_MAX_BODY_BYTES, OAuthRuntime};
    use crate::tokens::SystemRandomSource;

    const CIMD_URL: &str = "https://client.example/cimd.json";
    const SOURCE: IpAddr = IpAddr::V4(Ipv4Addr::new(198, 51, 100, 10));
    const ORIGIN: &str = "https://mcp.test";
    const REDIRECT: &str = "http://127.0.0.1/callback";

    fn journal_root() -> tempfile::TempDir {
        tempfile::Builder::new()
            .prefix("solstone-mcp-authorize-")
            .tempdir_in("/var/tmp")
            .unwrap()
    }

    fn runtime(journal: &tempfile::TempDir) -> OAuthRuntime {
        OAuthRuntime::new(journal.path(), ORIGIN.to_owned())
    }

    fn pkce_challenge() -> String {
        URL_SAFE_NO_PAD.encode(Sha256::digest(b"pkce-verifier"))
    }

    fn header<'a>(response: &'a HttpResponse, name: &str) -> Option<&'a str> {
        response
            .extra_headers
            .iter()
            .find(|(candidate, _)| candidate.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    fn body_text(response: &HttpResponse) -> String {
        String::from_utf8(response.body.clone()).unwrap()
    }

    fn public_addr() -> SocketAddr {
        SocketAddr::from((Ipv4Addr::new(8, 8, 8, 8), 443))
    }

    fn cimd_http(name: &str) -> Vec<u8> {
        let body = format!(
            r#"{{"client_id":"{CIMD_URL}","redirect_uris":["{REDIRECT}"],"client_name":"{name}"}}"#
        );
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
        response: Vec<u8>,
        fail: bool,
    }

    impl FakeIo {
        fn ok(name: &str) -> Self {
            Self {
                response: cimd_http(name),
                fail: false,
            }
        }
        fn fail() -> Self {
            Self {
                response: Vec::new(),
                fail: true,
            }
        }
    }

    impl CimdAttemptIo for FakeIo {
        type Socket = ();
        type Connection = FakeConnection;

        async fn resolve(&mut self, _host: &str, _port: u16) -> io::Result<Vec<SocketAddr>> {
            Ok(vec![public_addr()])
        }
        async fn connect(&mut self, _address: SocketAddr) -> io::Result<Self::Socket> {
            Ok(())
        }
        async fn tls(
            &mut self,
            _socket: Self::Socket,
            _server_name: &str,
        ) -> io::Result<Self::Connection> {
            if self.fail {
                return Err(io::Error::other("CIMD TLS failed"));
            }
            Ok(FakeConnection {
                data: self.response.clone(),
                offset: 0,
            })
        }
    }

    fn get_request(query: &str) -> HttpRequest {
        let mut request = HttpRequest::from_test_parts(HttpMethod::Get, Vec::new(), Vec::new());
        request.target = format!("/authorize?{query}");
        request
    }

    fn post_request(body: &str) -> HttpRequest {
        HttpRequest::from_test_parts(HttpMethod::Post, Vec::new(), body.as_bytes().to_vec())
    }

    fn authorize_query(extra: &[(&str, &str)]) -> String {
        let challenge = pkce_challenge();
        let mut params = vec![
            ("client_id", CIMD_URL),
            ("redirect_uri", REDIRECT),
            ("response_type", "code"),
            ("code_challenge", challenge.as_str()),
            ("code_challenge_method", "S256"),
            ("resource", "https://mcp.test/mcp"),
        ];
        for (name, value) in extra {
            if let Some(existing) = params.iter_mut().find(|(key, _)| key == name) {
                existing.1 = value;
            } else {
                params.push((name, value));
            }
        }
        params
            .into_iter()
            .map(|(name, value)| format!("{name}={}", query_value_encode(value)))
            .collect::<Vec<_>>()
            .join("&")
    }

    async fn get_with(oauth: &OAuthRuntime, io: FakeIo, query: &str) -> HttpResponse {
        let (_tx, mut shutdown) = watch::channel(false);
        get_authorize_with_io(
            &get_request(query),
            SOURCE,
            oauth,
            &mut shutdown,
            io,
            &SystemRandomSource,
        )
        .await
    }

    fn hidden_transaction_id(body: &str) -> String {
        let marker = "name=\"transaction_id\" value=\"";
        let start = body.find(marker).expect("hidden transaction_id") + marker.len();
        let end = body[start..].find('"').expect("value terminator") + start;
        body[start..end].to_owned()
    }

    #[tokio::test(start_paused = true)]
    async fn get_happy_path_renders_consent_with_security_headers() {
        let journal = journal_root();
        let oauth = runtime(&journal);
        let response = get_with(&oauth, FakeIo::ok("fixture"), &authorize_query(&[])).await;
        assert_eq!(response.status, 200);
        assert_eq!(response.content_type, Some("text/html; charset=utf-8"));
        let body = body_text(&response);
        assert_eq!(body.matches("name=\"pairing_code\"").count(), 1);
        assert!(body.contains("name=\"transaction_id\""));
        assert!(!body.contains("name=\"client_id\""));
        assert!(!body.contains("name=\"redirect_uri\""));
        assert_eq!(header(&response, "Cache-Control"), Some("no-store"));
        assert_eq!(header(&response, "X-Frame-Options"), Some("DENY"));
        let csp = header(&response, "Content-Security-Policy").unwrap();
        assert!(csp.contains("frame-ancestors 'none'"));
    }

    struct UnusedIo;

    impl CimdAttemptIo for UnusedIo {
        type Socket = ();
        type Connection = FakeConnection;

        async fn resolve(&mut self, _host: &str, _port: u16) -> io::Result<Vec<SocketAddr>> {
            Err(io::Error::other("known CIMD client must not fetch"))
        }
        async fn connect(&mut self, _address: SocketAddr) -> io::Result<Self::Socket> {
            Err(io::Error::other("known CIMD client must not fetch"))
        }
        async fn tls(
            &mut self,
            _socket: Self::Socket,
            _server_name: &str,
        ) -> io::Result<Self::Connection> {
            Err(io::Error::other("known CIMD client must not fetch"))
        }
    }

    #[tokio::test(start_paused = true)]
    async fn get_known_cimd_client_does_not_fetch() {
        let journal = journal_root();
        let oauth = runtime(&journal);
        let first = get_with(&oauth, FakeIo::ok("fixture"), &authorize_query(&[])).await;
        assert_eq!(first.status, 200);
        let (_tx, mut shutdown) = watch::channel(false);
        let second = get_authorize_with_io(
            &get_request(&authorize_query(&[])),
            SOURCE,
            &oauth,
            &mut shutdown,
            UnusedIo,
            &SystemRandomSource,
        )
        .await;
        assert_eq!(second.status, 200);
        assert!(body_text(&second).contains("name=\"transaction_id\""));
        assert!(header(&second, "Location").is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn get_unfetchable_client_has_no_location() {
        let journal = journal_root();
        let oauth = runtime(&journal);
        let response = get_with(&oauth, FakeIo::fail(), &authorize_query(&[])).await;
        assert_eq!(response.status, 400);
        assert!(header(&response, "Location").is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn get_unknown_classic_id_has_no_location() {
        let journal = journal_root();
        let oauth = runtime(&journal);
        let (_tx, mut shutdown) = watch::channel(false);
        let mut request = get_request(
            "client_id=oauth%3Adcr%3Aunknown&redirect_uri=http%3A%2F%2F127.0.0.1%2Fcallback&response_type=code&code_challenge=abc&code_challenge_method=S256&resource=https%3A%2F%2Fmcp.test%2Fmcp",
        );
        request.target = "/authorize?client_id=oauth%3Adcr%3Aunknown&redirect_uri=http%3A%2F%2F127.0.0.1%2Fcallback&response_type=code&code_challenge=abc&code_challenge_method=S256&resource=https%3A%2F%2Fmcp.test%2Fmcp".into();
        let response = get_authorize_with_io(
            &request,
            SOURCE,
            &oauth,
            &mut shutdown,
            FakeIo::fail(),
            &SystemRandomSource,
        )
        .await;
        assert_eq!(response.status, 400);
        assert!(header(&response, "Location").is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn get_unregistered_redirect_has_no_location() {
        let journal = journal_root();
        let oauth = runtime(&journal);
        let query = authorize_query(&[("redirect_uri", "http://127.0.0.1/other")]);
        let response = get_with(&oauth, FakeIo::ok("fixture"), &query).await;
        assert_eq!(response.status, 400);
        assert!(header(&response, "Location").is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn get_plain_pkce_redirects_with_encoded_state() {
        let journal = journal_root();
        let oauth = runtime(&journal);
        let state = "a&b=#x";
        for method in [None, Some("plain")] {
            let mut extras = vec![("state", state)];
            let query = if let Some(method) = method {
                extras.push(("code_challenge_method", method));
                authorize_query(&extras)
            } else {
                authorize_query(&extras).replace("&code_challenge_method=S256", "")
            };
            let response = get_with(&oauth, FakeIo::ok("fixture"), &query).await;
            assert_eq!(response.status, 302);
            let location = header(&response, "Location").unwrap();
            assert!(location.starts_with(REDIRECT));
            assert!(location.contains("error=invalid_request"));
            assert!(location.contains(&format!("state={}", query_value_encode(state))));
            assert!(!location.contains("state=a&b"));
            assert!(!location.contains("#x"));
            assert_eq!(header(&response, "Cache-Control"), Some("no-store"));
        }
    }

    #[tokio::test(start_paused = true)]
    async fn get_wrong_resource_redirects() {
        let journal = journal_root();
        let oauth = runtime(&journal);
        let query = authorize_query(&[("resource", "https://mcp.test/other")]);
        let response = get_with(&oauth, FakeIo::ok("fixture"), &query).await;
        assert_eq!(response.status, 302);
        let location = header(&response, "Location").unwrap();
        assert!(location.contains("error=invalid_request"));
    }

    #[tokio::test(start_paused = true)]
    async fn get_consent_escapes_client_name() {
        let journal = journal_root();
        let oauth = runtime(&journal);
        let response = get_with(
            &oauth,
            FakeIo::ok("<script>alert(1)</script>"),
            &authorize_query(&[]),
        )
        .await;
        let body = body_text(&response);
        assert!(!body.contains("<script>"));
        assert!(body.contains("&lt;script&gt;"));
    }

    #[tokio::test(start_paused = true)]
    async fn post_happy_path_issues_a_redeemable_code() {
        let journal = journal_root();
        let oauth = runtime(&journal);
        let pairing = oauth.store.generate_pairing_code().unwrap();
        let get = get_with(
            &oauth,
            FakeIo::ok("fixture"),
            &authorize_query(&[("state", "st&ate")]),
        )
        .await;
        let transaction_id = hidden_transaction_id(&body_text(&get));
        let response = post_authorize(
            &post_request(&format!(
                "transaction_id={}&pairing_code={}",
                query_value_encode(&transaction_id),
                query_value_encode(&pairing.code)
            )),
            SOURCE,
            &oauth,
        );
        assert_eq!(response.status, 302);
        assert_eq!(header(&response, "Cache-Control"), Some("no-store"));
        let location = header(&response, "Location").unwrap();
        assert!(location.contains(&format!("state={}", query_value_encode("st&ate"))));
        assert!(location.contains(&format!("iss={}", query_value_encode(ORIGIN))));
        let code = location
            .split(['?', '&'])
            .find_map(|piece| piece.strip_prefix("code="))
            .unwrap();
        let tokens = oauth
            .store
            .redeem_authorization_code(
                code,
                CIMD_URL,
                REDIRECT,
                "https://mcp.test/mcp",
                "pkce-verifier",
            )
            .unwrap();
        assert!(!tokens.access_token.is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn post_wrong_code_retries_then_exhausts() {
        let journal = journal_root();
        let oauth = runtime(&journal);
        oauth.store.generate_pairing_code().unwrap();
        let get = get_with(&oauth, FakeIo::ok("fixture"), &authorize_query(&[])).await;
        let transaction_id = hidden_transaction_id(&body_text(&get));
        let body = format!(
            "transaction_id={}&pairing_code=00000000",
            query_value_encode(&transaction_id)
        );
        for _ in 0..4 {
            let response = post_authorize(&post_request(&body), SOURCE, &oauth);
            let page = body_text(&response);
            assert!(page.contains("incorrect pairing code"));
            assert!(page.contains(&transaction_id));
        }
        let fifth = post_authorize(&post_request(&body), SOURCE, &oauth);
        let page = body_text(&fifth);
        assert!(page.contains("too many attempts"));
        assert!(!page.contains("name=\"transaction_id\""));
        let sixth = post_authorize(&post_request(&body), SOURCE, &oauth);
        assert!(body_text(&sixth).contains("no longer valid"));
    }

    #[tokio::test(start_paused = true)]
    async fn post_twenty_failures_lock_pairing_and_then_429() {
        let journal = journal_root();
        let oauth = runtime(&journal);
        let pairing = oauth.store.generate_pairing_code().unwrap();
        for _ in 0..4 {
            let get = get_with(&oauth, FakeIo::ok("fixture"), &authorize_query(&[])).await;
            let transaction_id = hidden_transaction_id(&body_text(&get));
            let body = format!(
                "transaction_id={}&pairing_code=00000000",
                query_value_encode(&transaction_id)
            );
            for _ in 0..5 {
                post_authorize(&post_request(&body), SOURCE, &oauth);
            }
        }
        let limited = post_authorize(
            &post_request("transaction_id=x&pairing_code=00000000"),
            SOURCE,
            &oauth,
        );
        assert_eq!(limited.status, 429);
        let client = oauth
            .store
            .lookup_client_by_cimd_url(CIMD_URL)
            .unwrap()
            .unwrap();
        let transaction = oauth
            .store
            .create_transaction(
                &client.id,
                REDIRECT,
                "https://mcp.test/mcp",
                ORIGIN,
                &pkce_challenge(),
                "S256",
                None,
                &SOURCE.to_string(),
            )
            .unwrap();
        assert!(matches!(
            oauth.store.complete_pairing(&transaction, &pairing.code),
            Err(crate::oauth::store::OAuthStoreError::PairingLocked)
        ));
    }

    #[tokio::test(start_paused = true)]
    async fn post_ingress_bounds_do_not_touch_the_store() {
        let journal = journal_root();
        let oauth = runtime(&journal);
        let mut oversize = b"transaction_id=x&pairing_code=00000000&pad=".to_vec();
        oversize.resize(AUTHORIZE_MAX_BODY_BYTES + 1, b'x');
        let too_large = post_authorize(
            &HttpRequest::from_test_parts(HttpMethod::Post, Vec::new(), oversize),
            SOURCE,
            &oauth,
        );
        assert_eq!(too_large.status, 413);
        let encoded = post_authorize(
            &HttpRequest::from_test_parts(
                HttpMethod::Post,
                vec![("Content-Encoding".to_owned(), "gzip".to_owned())],
                b"transaction_id=x&pairing_code=00000000".to_vec(),
            ),
            SOURCE,
            &oauth,
        );
        assert_eq!(encoded.status, 400);
        assert!(!journal.path().join("mcp-endpoint/oauth.json").exists());
    }

    #[tokio::test(start_paused = true)]
    async fn post_ignores_spoofed_redirect_and_state_fields() {
        let journal = journal_root();
        let oauth = runtime(&journal);
        let pairing = oauth.store.generate_pairing_code().unwrap();
        let get = get_with(
            &oauth,
            FakeIo::ok("fixture"),
            &authorize_query(&[("state", "bound")]),
        )
        .await;
        let transaction_id = hidden_transaction_id(&body_text(&get));
        let response = post_authorize(
            &post_request(&format!(
                "transaction_id={}&pairing_code={}&redirect_uri={}&state=spoofed",
                query_value_encode(&transaction_id),
                query_value_encode(&pairing.code),
                query_value_encode("http://127.0.0.1/evil"),
            )),
            SOURCE,
            &oauth,
        );
        assert_eq!(response.status, 302);
        let location = header(&response, "Location").unwrap();
        assert!(location.starts_with(REDIRECT));
        assert!(!location.contains("127.0.0.1/evil"));
        assert!(location.contains(&format!("state={}", query_value_encode("bound"))));
        assert!(!location.contains("state=spoofed"));
    }

    #[tokio::test(start_paused = true)]
    async fn post_prunes_stale_generation_limiter_buckets() {
        let journal = journal_root();
        let oauth = runtime(&journal);
        oauth.store.generate_pairing_code().unwrap();
        let old_generation = oauth.store.pairing_generation().unwrap();
        let overflow = IpAddr::V4(Ipv4Addr::from(u32::MAX));
        for index in 0..u32::MAX {
            if oauth.pairing_limiter.is_limited(overflow, old_generation) {
                break;
            }
            oauth
                .pairing_limiter
                .record_failure(IpAddr::V4(Ipv4Addr::from(index)), old_generation);
        }
        assert!(oauth.pairing_limiter.is_limited(overflow, old_generation));

        oauth.store.generate_pairing_code().unwrap();
        let new_generation = oauth.store.pairing_generation().unwrap();
        assert_ne!(new_generation, old_generation);

        let response = post_authorize(
            &post_request("transaction_id=x&pairing_code=00000000"),
            SOURCE,
            &oauth,
        );
        assert_eq!(response.status, 400);
        assert!(!oauth.pairing_limiter.is_limited(overflow, old_generation));
        let fresh = IpAddr::V4(Ipv4Addr::new(203, 0, 113, 9));
        assert!(!oauth.pairing_limiter.is_limited(fresh, new_generation));
    }
}
