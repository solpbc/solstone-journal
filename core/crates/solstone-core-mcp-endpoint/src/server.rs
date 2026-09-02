// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! TLS-terminating TCP admission for the journal-local MCP endpoint.

use std::fmt;
use std::io;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use chrono::Utc;
use solstone_core_mcp_audit::ToolName as AuditToolName;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Semaphore, watch};
use tokio::time::timeout;
use tokio_rustls::TlsAcceptor;

use crate::http1::{Http1Connection, Http1Error, HttpMethod, HttpRequest, HttpResponse};
use crate::jsonrpc::{
    JsonRpcResponse, McpMethod, classify_method, initialize_result, parse_request, tool_arguments,
    tool_result, tools_list_result,
};
use crate::oauth::OAuthRuntime;
use crate::oauth::store::OAuthStore;
use crate::permits::{connection_permit_pool, try_acquire_connection_permit};
use crate::proxy_preface::{ParsedPreface, parse_preface};
use crate::session::{SessionError, SessionTable};
use crate::tokens::{TokenStore, TokenStoreError, VerifiedToken};
use crate::{audit, tools};

const PROXY_PREFACE_DEADLINE: Duration = Duration::from_secs(2);
const TLS_HANDSHAKE_DEADLINE: Duration = Duration::from_secs(5);

/// Failure while running the TCP accept loop.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum ServerError {
    Accept,
}

impl fmt::Display for ServerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("MCP listener accept loop failed")
    }
}

impl std::error::Error for ServerError {}

/// Accept connections using an injected TLS configuration.
pub(crate) async fn serve(
    listener: TcpListener,
    tls_config: Arc<rustls::ServerConfig>,
    journal_root: Arc<PathBuf>,
    oauth: Arc<OAuthRuntime>,
    shutdown: watch::Receiver<bool>,
) -> Result<(), ServerError> {
    serve_with_permit_pool(
        listener,
        tls_config,
        journal_root,
        oauth,
        shutdown,
        Arc::new(SessionTable::new()),
        connection_permit_pool(),
    )
    .await
}

async fn serve_with_permit_pool(
    listener: TcpListener,
    tls_config: Arc<rustls::ServerConfig>,
    journal_root: Arc<PathBuf>,
    oauth: Arc<OAuthRuntime>,
    mut shutdown: watch::Receiver<bool>,
    sessions: Arc<SessionTable>,
    permits: Arc<Semaphore>,
) -> Result<(), ServerError> {
    loop {
        if *shutdown.borrow() {
            return Ok(());
        }
        tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    return Ok(());
                }
            }
            accepted = listener.accept() => {
                let (socket, _) = accepted.map_err(|_| ServerError::Accept)?;
                let Some(permit) = try_acquire_connection_permit(&permits) else {
                    drop(socket);
                    continue;
                };
                let connection_config = Arc::clone(&tls_config);
                let connection_root = Arc::clone(&journal_root);
                let connection_oauth = Arc::clone(&oauth);
                let connection_shutdown = shutdown.clone();
                let connection_sessions = Arc::clone(&sessions);
                tokio::spawn(async move {
                    let _permit = permit;
                    let _ = handle_connection(
                        socket,
                        connection_config,
                        connection_root,
                        connection_oauth,
                        connection_sessions,
                        connection_shutdown,
                    )
                    .await;
                });
            }
        }
    }
}

async fn handle_connection(
    mut socket: TcpStream,
    tls_config: Arc<rustls::ServerConfig>,
    journal_root: Arc<PathBuf>,
    oauth: Arc<OAuthRuntime>,
    sessions: Arc<SessionTable>,
    mut shutdown: watch::Receiver<bool>,
) -> Result<(), ConnectionError> {
    let preface = timeout(PROXY_PREFACE_DEADLINE, parse_preface(&mut socket))
        .await
        .map_err(|_| ConnectionError::PrefaceTimeout)?
        .map_err(|_| ConnectionError::Preface)?;
    let ParsedPreface { source, trailing } = preface;
    let source = source.ip();
    let stream = PrefixedStream::new(socket, trailing);
    let tls_stream = timeout(
        TLS_HANDSHAKE_DEADLINE,
        TlsAcceptor::from(tls_config).accept(stream),
    )
    .await
    .map_err(|_| ConnectionError::TlsTimeout)?
    .map_err(|_| ConnectionError::Tls)?;

    let token_store = TokenStore::open(journal_root.as_path());
    let mut http = Http1Connection::new(tls_stream);
    let mut waiting_for_next_request = false;
    loop {
        let request = tokio::select! {
            changed = shutdown.changed() => {
                let _ = changed;
                return Ok(());
            }
            request = http.read_request(waiting_for_next_request) => request,
        };
        let request = match request {
            Ok(Some(request)) => request,
            Ok(None) | Err(Http1Error::IdleTimeout) => return Ok(()),
            Err(error) => {
                let _ = http.write_response(&http_error_response(error)).await;
                return Ok(());
            }
        };
        let response = process_request(
            &request,
            &token_store,
            &sessions,
            journal_root.as_path(),
            source,
            oauth.as_ref(),
            &mut shutdown,
        )
        .await;
        http.write_response(&response)
            .await
            .map_err(|_| ConnectionError::HttpWrite)?;
        if request.connection_close || response.close {
            return Ok(());
        }
        waiting_for_next_request = true;
    }
}

#[derive(Debug)]
enum ConnectionError {
    PrefaceTimeout,
    Preface,
    TlsTimeout,
    Tls,
    HttpWrite,
}

async fn process_request(
    request: &HttpRequest,
    token_store: &TokenStore,
    sessions: &SessionTable,
    journal_root: &Path,
    source: IpAddr,
    oauth: &OAuthRuntime,
    shutdown: &mut watch::Receiver<bool>,
) -> HttpResponse {
    let path = request_path(&request.target);
    match (request.method, path) {
        (HttpMethod::Get, "/.well-known/oauth-protected-resource") => {
            crate::oauth::metadata::protected_resource(&oauth.resource_origin)
        }
        (HttpMethod::Get, "/.well-known/oauth-authorization-server") => {
            crate::oauth::metadata::authorization_server(&oauth.resource_origin)
        }
        (HttpMethod::Post, "/register") => {
            crate::oauth::dcr::register(request, source, oauth, shutdown).await
        }
        (HttpMethod::Get, "/authorize") => {
            crate::oauth::authorize::get_authorize(request, source, oauth, shutdown).await
        }
        (HttpMethod::Post, "/authorize") => {
            crate::oauth::authorize::post_authorize(request, source, oauth)
        }
        (HttpMethod::Post, "/token") => crate::oauth::token::token(request, oauth),
        (_, "/mcp") => handle_mcp(request, token_store, sessions, journal_root, oauth),
        (
            HttpMethod::Post | HttpMethod::Delete,
            "/.well-known/oauth-protected-resource" | "/.well-known/oauth-authorization-server",
        ) => method_not_allowed_with_allow("GET"),
        (HttpMethod::Get | HttpMethod::Delete, "/register" | "/token") => {
            method_not_allowed_with_allow("POST")
        }
        _ => HttpResponse::error(404, "Not Found", "MCP endpoint was not found"),
    }
}

fn handle_mcp(
    request: &HttpRequest,
    token_store: &TokenStore,
    sessions: &SessionTable,
    journal_root: &Path,
    oauth: &OAuthRuntime,
) -> HttpResponse {
    let verified = match authenticate(request, token_store, &oauth.store, &oauth.resource_origin) {
        Ok(verified) => verified,
        Err(response) => return response,
    };
    if request.target != "/mcp" {
        return HttpResponse::error(404, "Not Found", "MCP endpoint was not found");
    }
    match request.method {
        HttpMethod::Delete => delete_session(request, sessions, &verified),
        HttpMethod::Post => post_json_rpc(request, sessions, &verified, journal_root),
        HttpMethod::Get => method_not_allowed_with_allow("POST, DELETE"),
    }
}

fn request_path(target: &str) -> &str {
    target.split(['?', '#']).next().unwrap_or(target)
}

fn method_not_allowed_with_allow(method: &'static str) -> HttpResponse {
    HttpResponse::error(405, "Method Not Allowed", "HTTP method is not allowed")
        .with_header("Allow", method.to_owned())
}

fn authenticate(
    request: &HttpRequest,
    token_store: &TokenStore,
    oauth_store: &OAuthStore,
    resource_origin: &str,
) -> Result<VerifiedToken, HttpResponse> {
    let authorization = request
        .header("authorization")
        .map_err(|_| HttpResponse::error(400, "Bad Request", "Authorization header is ambiguous"))?
        .ok_or_else(|| mcp_unauthorized(resource_origin, "Bearer authorization is required"))?;
    let Some(token) = authorization.strip_prefix("Bearer ") else {
        return Err(mcp_unauthorized(
            resource_origin,
            "Bearer authorization is required",
        ));
    };
    if token.is_empty() || token.bytes().any(|byte| byte.is_ascii_whitespace()) {
        return Err(mcp_unauthorized(
            resource_origin,
            "Bearer authorization is malformed",
        ));
    }
    match token_store.verify(token) {
        Ok(verified) => Ok(verified),
        Err(TokenStoreError::InvalidToken) => match oauth_store.verify_access_token(token) {
            Ok(verified) => Ok(verified),
            Err(crate::oauth::store::OAuthStoreError::InvalidToken) => Err(mcp_unauthorized(
                resource_origin,
                "Bearer token is invalid or revoked",
            )),
            Err(_) => Err(HttpResponse::error(
                503,
                "Service Unavailable",
                "Bearer token verification is unavailable",
            )),
        },
        Err(_) => Err(HttpResponse::error(
            503,
            "Service Unavailable",
            "Bearer token verification is unavailable",
        )),
    }
}

fn mcp_unauthorized(resource_origin: &str, message: &'static str) -> HttpResponse {
    HttpResponse::error(401, "Unauthorized", message).with_header(
        "WWW-Authenticate",
        format!(
            "Bearer realm=\"mcp\", resource_metadata=\"{resource_origin}/.well-known/oauth-protected-resource\""
        ),
    )
}

fn delete_session(
    request: &HttpRequest,
    sessions: &SessionTable,
    verified: &VerifiedToken,
) -> HttpResponse {
    if !request.body.is_empty() {
        return HttpResponse::error(400, "Bad Request", "DELETE requests must not have a body");
    }
    let session_id = match request.header("mcp-session-id") {
        Ok(Some(session_id)) => session_id,
        Ok(None) => {
            return HttpResponse::error(
                400,
                "Bad Request",
                "MCP session ID is required for DELETE",
            );
        }
        Err(_) => return HttpResponse::error(400, "Bad Request", "MCP session ID is ambiguous"),
    };
    match sessions.delete(session_id, &verified.id) {
        Ok(()) => HttpResponse::empty(204, "No Content"),
        Err(SessionError::Foreign) => HttpResponse::error(
            403,
            "Forbidden",
            "MCP session belongs to a different bearer token",
        ),
        Err(SessionError::NotFound) => {
            HttpResponse::error(404, "Not Found", "MCP session is invalid or expired")
        }
        Err(_) => HttpResponse::error(
            503,
            "Service Unavailable",
            "MCP session state is unavailable",
        ),
    }
}

fn post_json_rpc(
    request: &HttpRequest,
    sessions: &SessionTable,
    verified: &VerifiedToken,
    journal_root: &std::path::Path,
) -> HttpResponse {
    let content_type = match request.header("content-type") {
        Ok(Some(value)) => value,
        Ok(None) | Err(_) => {
            return HttpResponse::error(
                415,
                "Unsupported Media Type",
                "MCP requests require application/json",
            );
        }
    };
    if !content_type
        .split(';')
        .next()
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("application/json"))
    {
        return HttpResponse::error(
            415,
            "Unsupported Media Type",
            "MCP requests require application/json",
        );
    }
    let json_request = match parse_request(&request.body) {
        Ok(request) => request,
        Err(response) => return json_rpc_response(*response),
    };
    if matches!(json_request.method.as_str(), "tools/list" | "tools/call") {
        let session_id = match request.header("mcp-session-id") {
            Ok(session_id) => session_id,
            Err(_) => {
                return json_rpc_response(session_error_response(
                    json_request.id.as_ref(),
                    SessionError::NotFound,
                ));
            }
        };
        if let Some(session_id) = session_id
            && let Err(error) = sessions.validate(session_id, &verified.id)
        {
            return json_rpc_response(session_error_response(json_request.id.as_ref(), error));
        }
    }
    match classify_method(&json_request) {
        Ok(McpMethod::Initialize) => match sessions.create(&verified.id) {
            Ok(session_id) => json_rpc_response(JsonRpcResponse::success(
                json_request.id.as_ref(),
                initialize_result(),
            ))
            .with_session_id(session_id),
            Err(error) => {
                json_rpc_response(session_error_response(json_request.id.as_ref(), error))
            }
        },
        Ok(McpMethod::ToolsList) => json_rpc_response(JsonRpcResponse::success(
            json_request.id.as_ref(),
            tools_list_result(),
        )),
        Ok(McpMethod::ToolsCall(tool_name)) => {
            execute_tool_call(&json_request, tool_name, verified, journal_root)
        }
        Err(response) => json_rpc_response(*response),
    }
}

fn execute_tool_call(
    request: &crate::jsonrpc::JsonRpcRequest,
    tool_name: crate::jsonrpc::ToolName,
    verified: &VerifiedToken,
    journal_root: &std::path::Path,
) -> HttpResponse {
    let validated = match tools::validate(tool_name, tool_arguments(request)) {
        Ok(validated) => validated,
        Err(tools::ToolError::InvalidInput) => {
            return json_rpc_response(JsonRpcResponse::invalid_params(request.id.as_ref()));
        }
        Err(error) => {
            return json_rpc_response(JsonRpcResponse::tool_error(
                request.id.as_ref(),
                error.reason(),
            ));
        }
    };
    let now = Utc::now();
    let audit_tool = match tool_name {
        crate::jsonrpc::ToolName::Search => AuditToolName::Search,
        crate::jsonrpc::ToolName::Fetch => AuditToolName::Fetch,
    };
    match tools::execute_after_audit(
        || {
            audit::write_admitted_interaction(
                journal_root,
                now,
                &verified.agent_identity,
                audit_tool,
            )
            .map(|_| ())
            .map_err(|_| tools::ToolError::AuditUnavailable)
        },
        || tools::execute(journal_root, &validated, now),
    ) {
        Ok(result) => json_rpc_response(JsonRpcResponse::success(
            request.id.as_ref(),
            tool_result(result),
        )),
        Err(tools::ToolError::AuditUnavailable) => json_rpc_response(
            JsonRpcResponse::internal_error(request.id.as_ref(), "MCP audit publication failed"),
        ),
        Err(error) => json_rpc_response(JsonRpcResponse::tool_error(
            request.id.as_ref(),
            error.reason(),
        )),
    }
}

fn session_error_response(id: Option<&serde_json::Value>, error: SessionError) -> JsonRpcResponse {
    match error {
        SessionError::PrincipalLimit => {
            JsonRpcResponse::internal_error(id, "MCP bearer session limit reached")
        }
        SessionError::GlobalLimit => {
            JsonRpcResponse::internal_error(id, "MCP global session limit reached")
        }
        SessionError::Foreign => {
            JsonRpcResponse::internal_error(id, "MCP session belongs to a different bearer token")
        }
        SessionError::NotFound => {
            JsonRpcResponse::internal_error(id, "MCP session is invalid or expired")
        }
        SessionError::Randomness | SessionError::Unavailable => {
            JsonRpcResponse::internal_error(id, "MCP session state is unavailable")
        }
    }
}

fn json_rpc_response(response: JsonRpcResponse) -> HttpResponse {
    match response.to_bytes() {
        Ok(body) => HttpResponse::json(200, "OK", body),
        Err(_) => HttpResponse::error(
            500,
            "Internal Server Error",
            "MCP response serialization failed",
        ),
    }
}

fn http_error_response(error: Http1Error) -> HttpResponse {
    match error {
        Http1Error::HeaderTimeout | Http1Error::BodyTimeout => {
            HttpResponse::error(408, "Request Timeout", "HTTP request deadline expired")
        }
        Http1Error::BodyTooLarge => {
            HttpResponse::error(413, "Payload Too Large", "HTTP request body exceeds 64 KiB")
        }
        Http1Error::UnsupportedRequest => {
            HttpResponse::error(405, "Method Not Allowed", "HTTP/1.1 GET, POST, or DELETE")
        }
        _ => HttpResponse::error(400, "Bad Request", "HTTP/1.1 request framing is invalid"),
    }
}

/// An async stream that serves already-read bytes before its underlying stream.
struct PrefixedStream<S> {
    inner: S,
    prefix: Vec<u8>,
    prefix_offset: usize,
}

impl<S> PrefixedStream<S> {
    fn new(inner: S, prefix: Vec<u8>) -> Self {
        Self {
            inner,
            prefix,
            prefix_offset: 0,
        }
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for PrefixedStream<S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        read_buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.as_mut().get_mut();
        if this.prefix_offset < this.prefix.len() && read_buffer.remaining() > 0 {
            let count = read_buffer
                .remaining()
                .min(this.prefix.len() - this.prefix_offset);
            read_buffer.put_slice(&this.prefix[this.prefix_offset..this.prefix_offset + count]);
            this.prefix_offset += count;
            return Poll::Ready(Ok(()));
        }
        Pin::new(&mut this.inner).poll_read(context, read_buffer)
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for PrefixedStream<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.as_mut().get_mut().inner).poll_write(context, bytes)
    }

    fn poll_flush(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.as_mut().get_mut().inner).poll_flush(context)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.as_mut().get_mut().inner).poll_shutdown(context)
    }
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use std::fs;
    use std::io;
    use std::io::Read as _;
    use std::net::{Ipv4Addr, SocketAddr};
    use std::os::unix::net::UnixListener;
    use std::path::Path;
    use std::pin::Pin;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::task::{Context, Poll};
    use std::thread;

    use rcgen::{CertificateParams, KeyPair, PKCS_ECDSA_P256_SHA256};
    use rusqlite::params;
    use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName};
    use rustls::{ClientConfig, ClientConnection, RootCertStore, ServerConfig};
    use serde_json::{Value, json};
    use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _, ReadBuf};
    use tokio::net::{TcpListener, TcpStream};
    use tokio::sync::{Semaphore, watch};
    use tokio::task::JoinHandle;
    use tokio::time::{Duration, advance};
    use tokio_rustls::{TlsAcceptor, TlsConnector, client::TlsStream};

    use crate::jsonrpc::tools_list_result;
    use crate::oauth::OAuthRuntime;
    use crate::permits::{
        CONNECTION_PERMITS, connection_permit_pool, try_acquire_connection_permit,
    };
    use crate::session::SessionTable;
    use crate::tokens::TokenStore;

    use super::{ServerError, serve_with_permit_pool};

    const HOSTNAME: &str = "mcp.test";
    const VALID_PROXY_PREFACE: &[u8] = b"PROXY TCP4 198.51.100.12 127.0.0.1 4321 443\r\n";

    struct FragmentingStream<S> {
        inner: S,
        max_write: usize,
        write_count: Arc<AtomicUsize>,
    }

    impl<S: AsyncRead + Unpin> AsyncRead for FragmentingStream<S> {
        fn poll_read(
            mut self: Pin<&mut Self>,
            context: &mut Context<'_>,
            buffer: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            Pin::new(&mut self.inner).poll_read(context, buffer)
        }
    }

    impl<S: AsyncWrite + Unpin> AsyncWrite for FragmentingStream<S> {
        fn poll_write(
            mut self: Pin<&mut Self>,
            context: &mut Context<'_>,
            buffer: &[u8],
        ) -> Poll<io::Result<usize>> {
            let max_write = self.max_write;
            match Pin::new(&mut self.inner)
                .poll_write(context, &buffer[..buffer.len().min(max_write)])
            {
                Poll::Ready(Ok(written)) => {
                    if written > 0 {
                        self.write_count.fetch_add(1, Ordering::Relaxed);
                    }
                    Poll::Ready(Ok(written))
                }
                other => other,
            }
        }

        fn poll_flush(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
            Pin::new(&mut self.inner).poll_flush(context)
        }

        fn poll_shutdown(
            mut self: Pin<&mut Self>,
            context: &mut Context<'_>,
        ) -> Poll<io::Result<()>> {
            Pin::new(&mut self.inner).poll_shutdown(context)
        }
    }

    struct CoalescingPrefaceStream<S> {
        inner: S,
        preface: Option<Vec<u8>>,
        pending_first_write: Option<(Vec<u8>, usize)>,
        coalesced: Arc<AtomicBool>,
    }

    impl<S: AsyncRead + Unpin> AsyncRead for CoalescingPrefaceStream<S> {
        fn poll_read(
            mut self: Pin<&mut Self>,
            context: &mut Context<'_>,
            buffer: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            Pin::new(&mut self.inner).poll_read(context, buffer)
        }
    }

    impl<S: AsyncWrite + Unpin> AsyncWrite for CoalescingPrefaceStream<S> {
        fn poll_write(
            mut self: Pin<&mut Self>,
            context: &mut Context<'_>,
            buffer: &[u8],
        ) -> Poll<io::Result<usize>> {
            let Some((combined, handshake_len)) = self.pending_first_write.take().or_else(|| {
                self.preface.take().map(|mut preface| {
                    preface.extend_from_slice(buffer);
                    let handshake_len = buffer.len();
                    (preface, handshake_len)
                })
            }) else {
                return Pin::new(&mut self.inner).poll_write(context, buffer);
            };
            match Pin::new(&mut self.inner).poll_write(context, &combined) {
                Poll::Ready(Ok(written)) if written == combined.len() => {
                    self.coalesced.store(true, Ordering::Relaxed);
                    Poll::Ready(Ok(handshake_len))
                }
                Poll::Ready(Ok(_)) => Poll::Ready(Err(io::Error::other(
                    "fixture could not coalesce the PROXY preface and ClientHello",
                ))),
                Poll::Ready(Err(error)) => Poll::Ready(Err(error)),
                Poll::Pending => {
                    self.pending_first_write = Some((combined, handshake_len));
                    Poll::Pending
                }
            }
        }

        fn poll_flush(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
            Pin::new(&mut self.inner).poll_flush(context)
        }

        fn poll_shutdown(
            mut self: Pin<&mut Self>,
            context: &mut Context<'_>,
        ) -> Poll<io::Result<()>> {
            Pin::new(&mut self.inner).poll_shutdown(context)
        }
    }

    struct ServerHarness {
        address: SocketAddr,
        permits: std::sync::Arc<Semaphore>,
        shutdown: watch::Sender<bool>,
        task: JoinHandle<Result<(), ServerError>>,
        journal: tempfile::TempDir,
        oauth: Arc<OAuthRuntime>,
    }

    impl ServerHarness {
        async fn start(tls_config: std::sync::Arc<ServerConfig>) -> Self {
            let journal = tempfile::Builder::new()
                .prefix("solstone-mcp-server-")
                .tempdir_in("/var/tmp")
                .expect("fixture journal directory");
            Self::start_with_journal(tls_config, journal).await
        }

        async fn start_with_journal(
            tls_config: std::sync::Arc<ServerConfig>,
            journal: tempfile::TempDir,
        ) -> Self {
            Self::start_with_journal_override(tls_config, journal, None).await
        }

        async fn start_with_journal_override(
            tls_config: std::sync::Arc<ServerConfig>,
            journal: tempfile::TempDir,
            cimd: Option<(SocketAddr, std::sync::Arc<ClientConfig>)>,
        ) -> Self {
            let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
                .await
                .expect("fixture listener binds");
            let address = listener.local_addr().expect("fixture listener address");
            let permits = connection_permit_pool();
            let (shutdown, receiver) = watch::channel(false);
            let mut oauth = OAuthRuntime::new(journal.path(), format!("https://{HOSTNAME}"));
            if let Some((target, config)) = cimd {
                oauth = oauth.with_cimd_test_target(target, config);
            }
            let oauth = Arc::new(oauth);
            let task = tokio::spawn(serve_with_permit_pool(
                listener,
                tls_config,
                Arc::new(journal.path().to_path_buf()),
                Arc::clone(&oauth),
                receiver,
                Arc::new(SessionTable::new()),
                std::sync::Arc::clone(&permits),
            ));
            Self {
                address,
                permits,
                shutdown,
                task,
                journal,
                oauth,
            }
        }

        fn create_token(&self, label: &str) -> crate::tokens::CreatedToken {
            TokenStore::open(self.journal.path())
                .create(label)
                .expect("fixture creates bearer token")
        }

        async fn stop(self) -> tempfile::TempDir {
            self.shutdown.send(true).expect("server remains subscribed");
            self.task
                .await
                .expect("server task joins")
                .expect("server exits cleanly");
            self.journal
        }
    }

    fn tls_configs() -> (std::sync::Arc<ServerConfig>, std::sync::Arc<ClientConfig>) {
        tls_pair(HOSTNAME)
    }

    fn tls_pair(hostname: &str) -> (std::sync::Arc<ServerConfig>, std::sync::Arc<ClientConfig>) {
        let key_pair = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).expect("fixture key");
        let certificate = CertificateParams::new(vec![hostname.to_owned()])
            .expect("fixture params")
            .self_signed(&key_pair)
            .expect("fixture certificate");
        let certificate = CertificateDer::from(certificate.der().to_vec());
        let private_key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key_pair.serialize_der()));
        let provider = std::sync::Arc::new(rustls::crypto::ring::default_provider());
        let mut server = ServerConfig::builder_with_provider(std::sync::Arc::clone(&provider))
            .with_protocol_versions(&[&rustls::version::TLS13])
            .expect("ring provider supports TLS 1.3")
            .with_no_client_auth()
            .with_single_cert(vec![certificate.clone()], private_key)
            .expect("fixture server certificate");
        server.alpn_protocols = vec![b"http/1.1".to_vec()];

        let mut roots = RootCertStore::empty();
        roots.add(certificate).expect("fixture root");
        let mut client = ClientConfig::builder_with_provider(provider)
            .with_protocol_versions(&[&rustls::version::TLS13])
            .expect("ring provider supports TLS 1.3")
            .with_root_certificates(roots)
            .with_no_client_auth();
        client.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
        (std::sync::Arc::new(server), std::sync::Arc::new(client))
    }

    fn untrusted_client_config() -> std::sync::Arc<ClientConfig> {
        let provider = std::sync::Arc::new(rustls::crypto::ring::default_provider());
        let mut client = ClientConfig::builder_with_provider(provider)
            .with_protocol_versions(&[&rustls::version::TLS13])
            .expect("ring provider supports TLS 1.3")
            .with_root_certificates(RootCertStore::empty())
            .with_no_client_auth();
        client.alpn_protocols = vec![b"http/1.1".to_vec()];
        std::sync::Arc::new(client)
    }

    async fn connect_tls(
        address: SocketAddr,
        client_config: std::sync::Arc<ClientConfig>,
    ) -> TlsStream<TcpStream> {
        let mut socket = TcpStream::connect(address).await.expect("client connects");
        socket
            .write_all(VALID_PROXY_PREFACE)
            .await
            .expect("client sends PROXY preface");
        let name = ServerName::try_from(HOSTNAME.to_owned()).expect("fixture server name");
        TlsConnector::from(client_config)
            .connect(name, socket)
            .await
            .expect("TLS handshake completes")
    }

    async fn connect_tls_with_fragmented_client_hello(
        address: SocketAddr,
        client_config: std::sync::Arc<ClientConfig>,
    ) -> (TlsStream<FragmentingStream<TcpStream>>, Arc<AtomicUsize>) {
        let mut socket = TcpStream::connect(address).await.expect("client connects");
        socket
            .write_all(VALID_PROXY_PREFACE)
            .await
            .expect("client sends PROXY preface");
        socket.flush().await.expect("client flushes PROXY preface");
        let writes = Arc::new(AtomicUsize::new(0));
        let stream = FragmentingStream {
            inner: socket,
            max_write: 16,
            write_count: Arc::clone(&writes),
        };
        let name = ServerName::try_from(HOSTNAME.to_owned()).expect("fixture server name");
        let client = TlsConnector::from(client_config)
            .connect(name, stream)
            .await
            .expect("fragmented TLS handshake completes");
        (client, writes)
    }

    async fn connect_tls_with_coalesced_proxy_preface(
        address: SocketAddr,
        client_config: std::sync::Arc<ClientConfig>,
    ) -> (
        TlsStream<CoalescingPrefaceStream<TcpStream>>,
        Arc<AtomicBool>,
    ) {
        let socket = TcpStream::connect(address).await.expect("client connects");
        let coalesced = Arc::new(AtomicBool::new(false));
        let stream = CoalescingPrefaceStream {
            inner: socket,
            preface: Some(VALID_PROXY_PREFACE.to_vec()),
            pending_first_write: None,
            coalesced: Arc::clone(&coalesced),
        };
        let name = ServerName::try_from(HOSTNAME.to_owned()).expect("fixture server name");
        let client = TlsConnector::from(client_config)
            .connect(name, stream)
            .await
            .expect("coalesced TLS handshake completes");
        (client, coalesced)
    }

    async fn wait_for_permits(pool: &Semaphore, expected: usize) {
        for _ in 0..100 {
            if pool.available_permits() == expected {
                return;
            }
            tokio::task::yield_now().await;
        }
        panic!("connection permits did not return to expected capacity");
    }

    async fn send_and_confirm_close(address: SocketAddr, bytes: &[u8]) {
        let mut socket = TcpStream::connect(address).await.expect("client connects");
        socket.write_all(bytes).await.expect("client sends bytes");
        socket.shutdown().await.expect("client closes write half");
        let mut response = [0_u8; 1];
        assert_eq!(socket.read(&mut response).await.expect("client reads"), 0);
    }

    async fn post_json<S>(client: &mut TlsStream<S>, token: &str, request: Value) -> Value
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        post_json_with_headers(client, token, request, &[]).await.0
    }

    async fn post_json_with_headers<S>(
        client: &mut TlsStream<S>,
        token: &str,
        request: Value,
        headers: &[(&str, &str)],
    ) -> (Value, String)
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        let (status, body, head) =
            post_json_response_with_headers(client, token, request, headers).await;
        assert_eq!(status, 200, "MCP request succeeds: {head}");
        (
            serde_json::from_slice(&body).expect("successful response is JSON"),
            head,
        )
    }

    async fn post_json_response_with_headers<S>(
        client: &mut TlsStream<S>,
        token: &str,
        request: Value,
        headers: &[(&str, &str)],
    ) -> (u16, Vec<u8>, String)
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        let body = serde_json::to_vec(&request).expect("fixture JSON request");
        let headers = headers
            .iter()
            .map(|(name, value)| format!("{name}: {value}\r\n"))
            .collect::<String>();
        let request = format!(
            "POST /mcp HTTP/1.1\r\nHost: {HOSTNAME}\r\nAuthorization: Bearer {token}\r\nContent-Type: application/json\r\n{headers}Content-Length: {}\r\n\r\n",
            body.len(),
        );
        client
            .write_all(request.as_bytes())
            .await
            .expect("client writes request headers");
        client
            .write_all(&body)
            .await
            .expect("client writes request body");
        let mut head = Vec::new();
        loop {
            let mut byte = [0_u8; 1];
            client
                .read_exact(&mut byte)
                .await
                .expect("client reads response headers");
            head.push(byte[0]);
            if head.ends_with(b"\r\n\r\n") {
                break;
            }
        }
        let head = String::from_utf8(head).expect("response headers are text");
        let status = head
            .split_whitespace()
            .nth(1)
            .expect("response has status")
            .parse::<u16>()
            .expect("response status is numeric");
        let length = head
            .lines()
            .find_map(|line| line.strip_prefix("Content-Length: "))
            .expect("response has content length")
            .parse::<usize>()
            .expect("response length is numeric");
        let mut body = vec![0_u8; length];
        client
            .read_exact(&mut body)
            .await
            .expect("client reads response body");
        (status, body, head)
    }

    fn www_authenticate(head: &str) -> Option<&str> {
        head.lines()
            .find_map(|line| line.strip_prefix("WWW-Authenticate: "))
    }

    async fn exchange_http<S>(client: &mut TlsStream<S>, request: &str) -> (u16, Vec<u8>, String)
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        client
            .write_all(request.as_bytes())
            .await
            .expect("client writes request");
        let mut head = Vec::new();
        loop {
            let mut byte = [0_u8; 1];
            client
                .read_exact(&mut byte)
                .await
                .expect("client reads response headers");
            head.push(byte[0]);
            if head.ends_with(b"\r\n\r\n") {
                break;
            }
        }
        let head = String::from_utf8(head).expect("response headers are text");
        let status = head
            .split_whitespace()
            .nth(1)
            .expect("response has status")
            .parse::<u16>()
            .expect("response status is numeric");
        let length = head
            .lines()
            .find_map(|line| line.strip_prefix("Content-Length: "))
            .expect("response has content length")
            .parse::<usize>()
            .expect("response length is numeric");
        let mut body = vec![0_u8; length];
        client
            .read_exact(&mut body)
            .await
            .expect("client reads response body");
        (status, body, head)
    }

    fn seed_indexed_note(journal: &Path) {
        let path = "notes/mcp.txt";
        fs::create_dir_all(journal.join("notes")).expect("fixture notes directory");
        fs::write(journal.join(path), "MCP fixture search needle").expect("fixture source content");
        let connection =
            solstone_core_indexer_store::db::open_index(journal).expect("fixture index opens");
        connection
            .execute(
                "INSERT INTO chunks(content, path, day, facet, agent, stream, idx, time_bucket) VALUES (?1, ?2, '20260831', '', 'fixture', 'default', 0, '')",
                params!["MCP fixture search needle", path],
            )
            .expect("fixture chunk inserts");
        connection
            .execute(
                "REPLACE INTO index_build_state(id, schema_version, state, files_count, chunks_count) VALUES (1, 1, 'complete', 1, 1)",
                [],
            )
            .expect("fixture index completes");
    }

    fn session_id(headers: &str) -> String {
        headers
            .lines()
            .find_map(|line| line.strip_prefix("Mcp-Session-Id: "))
            .expect("initialize response has MCP session ID")
            .to_owned()
    }

    fn audit_record_count(journal: &Path) -> usize {
        fs::read_dir(journal.join("chronicle"))
            .expect("audit day exists")
            .flat_map(|day| {
                fs::read_dir(day.expect("day entry").path().join("mcp.agent"))
                    .expect("audit stream exists")
            })
            .filter(|entry| {
                entry
                    .as_ref()
                    .expect("audit segment")
                    .path()
                    .join("interaction.json")
                    .is_file()
            })
            .count()
    }

    fn raw_client_hello(client_config: std::sync::Arc<ClientConfig>) -> Vec<u8> {
        let name = ServerName::try_from(HOSTNAME.to_owned()).expect("fixture server name");
        let mut connection = ClientConnection::new(client_config, name).expect("fixture client");
        let mut bytes = Vec::new();
        connection
            .write_tls(&mut bytes)
            .expect("fixture ClientHello serializes");
        bytes
    }

    fn snapshot_tree(root: &Path) -> Vec<(String, Option<Vec<u8>>)> {
        fn visit(root: &Path, directory: &Path, snapshot: &mut Vec<(String, Option<Vec<u8>>)>) {
            let entries = fs::read_dir(directory).expect("snapshot directory reads");
            for entry in entries {
                let entry = entry.expect("snapshot directory entry");
                let path = entry.path();
                let relative = path
                    .strip_prefix(root)
                    .expect("snapshot path stays rooted")
                    .display()
                    .to_string();
                if entry.file_type().expect("snapshot file type").is_dir() {
                    snapshot.push((relative, None));
                    visit(root, &path, snapshot);
                } else {
                    snapshot.push((relative, Some(fs::read(path).expect("snapshot file bytes"))));
                }
            }
        }

        let mut snapshot = Vec::new();
        visit(root, root, &mut snapshot);
        snapshot.sort_by(|left, right| left.0.cmp(&right.0));
        snapshot
    }

    #[tokio::test]
    async fn valid_proxy_preface_completes_tls_with_http11_only() {
        let (server_config, client_config) = tls_configs();
        assert_eq!(server_config.alpn_protocols, vec![b"http/1.1".to_vec()]);
        let server = ServerHarness::start(server_config).await;

        let mut client = connect_tls(server.address, client_config).await;
        assert_eq!(
            client.get_ref().1.alpn_protocol(),
            Some(b"http/1.1".as_slice())
        );
        client.shutdown().await.expect("client closes TLS");
        drop(client);
        wait_for_permits(&server.permits, CONNECTION_PERMITS).await;
        server.stop().await;
    }

    #[tokio::test]
    async fn missing_malformed_and_overlong_prefaces_close_before_tls() {
        let (server_config, client_config) = tls_configs();
        let server = ServerHarness::start(server_config).await;

        send_and_confirm_close(server.address, &raw_client_hello(client_config)).await;
        send_and_confirm_close(
            server.address,
            b"BROXY TCP4 198.51.100.12 127.0.0.1 4321 443\r\n",
        )
        .await;
        send_and_confirm_close(server.address, &[b'x'; 108]).await;
        wait_for_permits(&server.permits, CONNECTION_PERMITS).await;
        server.stop().await;
    }

    #[tokio::test(start_paused = true)]
    async fn proxy_preface_deadline_is_absolute_despite_slow_progress() {
        let (server_config, _) = tls_configs();
        let server = ServerHarness::start(server_config).await;
        let mut client = TcpStream::connect(server.address)
            .await
            .expect("client connects");

        client.write_all(b"P").await.expect("client sends byte");
        tokio::task::yield_now().await;
        advance(Duration::from_secs(1)).await;
        client.write_all(b"R").await.expect("client makes progress");
        tokio::task::yield_now().await;
        advance(Duration::from_secs(1)).await;
        tokio::task::yield_now().await;

        let mut response = [0_u8; 1];
        assert_eq!(client.read(&mut response).await.expect("client reads"), 0);
        wait_for_permits(&server.permits, CONNECTION_PERMITS).await;
        server.stop().await;
    }

    #[tokio::test(start_paused = true)]
    async fn tls_handshake_deadline_releases_the_connection_permit() {
        let (server_config, _) = tls_configs();
        let server = ServerHarness::start(server_config).await;
        let mut client = TcpStream::connect(server.address)
            .await
            .expect("client connects");
        client
            .write_all(VALID_PROXY_PREFACE)
            .await
            .expect("client sends PROXY preface");
        tokio::task::yield_now().await;
        advance(Duration::from_secs(5)).await;
        tokio::task::yield_now().await;

        let mut response = [0_u8; 1];
        assert_eq!(client.read(&mut response).await.expect("client reads"), 0);
        wait_for_permits(&server.permits, CONNECTION_PERMITS).await;
        server.stop().await;
    }

    #[tokio::test]
    async fn each_connection_exit_path_releases_its_permit() {
        let (server_config, client_config) = tls_configs();
        let server = ServerHarness::start(server_config).await;

        send_and_confirm_close(
            server.address,
            b"BROXY TCP4 198.51.100.12 127.0.0.1 4321 443\r\n",
        )
        .await;
        wait_for_permits(&server.permits, CONNECTION_PERMITS).await;

        let mut socket = TcpStream::connect(server.address)
            .await
            .expect("client connects");
        socket
            .write_all(VALID_PROXY_PREFACE)
            .await
            .expect("client sends PROXY preface");
        let name = ServerName::try_from(HOSTNAME.to_owned()).expect("fixture server name");
        assert!(
            TlsConnector::from(untrusted_client_config())
                .connect(name, socket)
                .await
                .is_err()
        );
        wait_for_permits(&server.permits, CONNECTION_PERMITS).await;

        let mut client = connect_tls(server.address, client_config).await;
        client.shutdown().await.expect("client closes TLS");
        drop(client);
        wait_for_permits(&server.permits, CONNECTION_PERMITS).await;
        server.stop().await;
    }

    #[tokio::test]
    async fn task_cancellation_releases_an_owned_connection_permit() {
        let (server_config, _) = tls_configs();
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("fixture listener binds");
        let address = listener.local_addr().expect("fixture listener address");
        let client = TcpStream::connect(address).await.expect("client connects");
        let (socket, _) = listener.accept().await.expect("listener accepts client");
        let permits = connection_permit_pool();
        let permit = try_acquire_connection_permit(&permits).expect("connection is admitted");
        let (_, shutdown) = watch::channel(false);
        let journal = tempfile::Builder::new()
            .prefix("solstone-mcp-server-")
            .tempdir_in("/var/tmp")
            .expect("fixture journal directory");
        let oauth = Arc::new(OAuthRuntime::new(
            journal.path(),
            format!("https://{HOSTNAME}"),
        ));
        let task = tokio::spawn(async move {
            let _permit = permit;
            let _ = super::handle_connection(
                socket,
                server_config,
                Arc::new(journal.path().to_path_buf()),
                oauth,
                Arc::new(SessionTable::new()),
                shutdown,
            )
            .await;
        });
        tokio::task::yield_now().await;
        assert_eq!(permits.available_permits(), CONNECTION_PERMITS - 1);
        task.abort();
        assert!(
            task.await
                .expect_err("task cancellation is reported")
                .is_cancelled()
        );
        wait_for_permits(&permits, CONNECTION_PERMITS).await;
        drop(client);
    }

    #[tokio::test]
    async fn admitted_search_and_fetch_calls_each_publish_one_closed_audit_record() {
        let (server_config, client_config) = tls_configs();
        let server = ServerHarness::start(server_config).await;
        let health = server.journal.path().join("health");
        fs::create_dir_all(&health).expect("fixture health directory");
        let listener =
            UnixListener::bind(health.join("callosum.sock")).expect("fixture Callosum listener");
        let events = thread::spawn(move || {
            (0..2)
                .map(|_| {
                    let (mut connection, _) = listener.accept().expect("event sender connects");
                    let mut line = String::new();
                    connection.read_to_string(&mut line).expect("event reads");
                    serde_json::from_str::<Value>(&line).expect("event is JSON")
                })
                .collect::<Vec<_>>()
        });
        let token = server.create_token("audit-agent");
        let mut client = connect_tls(server.address, client_config).await;

        let search = post_json(
            &mut client,
            &token.token,
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/call",
                "params": {"name": "search", "arguments": {"query": "📅", "limit": 1, "offset": 0}}
            }),
        )
        .await;
        assert_eq!(
            search["result"]["structuredContent"]["reason"],
            "not_tokenizable"
        );
        let fetch = post_json(
            &mut client,
            &token.token,
            json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/call",
                "params": {"name": "fetch", "arguments": {"id": "notes/unindexed.txt:0"}}
            }),
        )
        .await;
        assert_eq!(fetch["error"]["data"]["reason"], "index_absent");
        client.shutdown().await.expect("client closes TLS");
        drop(client);

        let records = fs::read_dir(server.journal.path().join("chronicle"))
            .expect("audit day exists")
            .flat_map(|day| {
                fs::read_dir(day.expect("day entry").path().join("mcp.agent"))
                    .expect("audit stream exists")
            })
            .map(|entry| {
                entry
                    .expect("audit segment")
                    .path()
                    .join("interaction.json")
            })
            .collect::<Vec<_>>();
        assert_eq!(records.len(), 2);
        let mut names = records
            .iter()
            .map(|record| {
                let record: Value =
                    serde_json::from_slice(&fs::read(record).expect("audit record"))
                        .expect("audit record is JSON");
                assert_eq!(record.as_object().expect("record object").len(), 3);
                record["tool_name"]
                    .as_str()
                    .expect("closed tool name")
                    .to_owned()
            })
            .collect::<Vec<_>>();
        names.sort();
        assert_eq!(names, ["fetch", "search"]);
        let events = events.join().expect("Callosum listener joins");
        assert_eq!(events.len(), 2);
        assert!(events.iter().all(|event| {
            event
                == &json!({
                    "tract": "observe",
                    "event": "observed",
                    "day": event["day"],
                    "stream": "mcp.agent",
                    "segment": event["segment"],
                })
        }));
        wait_for_permits(&server.permits, CONNECTION_PERMITS).await;
        server.stop().await;
    }

    #[tokio::test]
    async fn fragmented_and_coalesced_tls_client_hellos_support_the_read_only_mcp_flow() {
        let (server_config, client_config) = tls_configs();
        let server = ServerHarness::start(server_config).await;
        seed_indexed_note(server.journal.path());
        let token = server.create_token("full-flow-agent");

        let (mut client, fragmented_writes) =
            connect_tls_with_fragmented_client_hello(server.address, Arc::clone(&client_config))
                .await;
        assert!(
            fragmented_writes.load(Ordering::Relaxed) >= 3,
            "the ClientHello must be split across multiple socket writes"
        );
        assert_eq!(
            client.get_ref().1.alpn_protocol(),
            Some(b"http/1.1".as_slice())
        );

        let (_, headers) = post_json_with_headers(
            &mut client,
            &token.token,
            json!({"jsonrpc": "2.0", "id": 1, "method": "initialize"}),
            &[],
        )
        .await;
        let session = session_id(&headers);
        let list = post_json_with_headers(
            &mut client,
            &token.token,
            json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
            &[("Mcp-Session-Id", &session)],
        )
        .await
        .0;
        assert_eq!(list["result"], tools_list_result());
        let search = post_json_with_headers(
            &mut client,
            &token.token,
            json!({
                "jsonrpc": "2.0",
                "id": 3,
                "method": "tools/call",
                "params": {"name": "search", "arguments": {"query": "needle", "limit": 10, "offset": 0}},
            }),
            &[("Mcp-Session-Id", &session)],
        )
        .await
        .0;
        assert_eq!(
            search["result"]["structuredContent"]["results"][0]["id"],
            "notes/mcp.txt:0"
        );
        let fetch = post_json_with_headers(
            &mut client,
            &token.token,
            json!({
                "jsonrpc": "2.0",
                "id": 4,
                "method": "tools/call",
                "params": {"name": "fetch", "arguments": {"id": "notes/mcp.txt:0"}},
            }),
            &[("Mcp-Session-Id", &session)],
        )
        .await
        .0;
        assert_eq!(
            fetch["result"]["structuredContent"],
            json!({"content": "MCP fixture search needle"})
        );
        client.shutdown().await.expect("client closes TLS");
        drop(client);

        let (mut coalesced_client, coalesced) =
            connect_tls_with_coalesced_proxy_preface(server.address, client_config).await;
        assert!(
            coalesced.load(Ordering::Relaxed),
            "the PROXY preface and ClientHello must share the first socket write"
        );
        let (_, coalesced_headers) = post_json_with_headers(
            &mut coalesced_client,
            &token.token,
            json!({"jsonrpc": "2.0", "id": 5, "method": "initialize"}),
            &[],
        )
        .await;
        assert!(!session_id(&coalesced_headers).is_empty());
        coalesced_client
            .shutdown()
            .await
            .expect("control client closes TLS");
        drop(coalesced_client);

        assert_eq!(audit_record_count(server.journal.path()), 2);
        wait_for_permits(&server.permits, CONNECTION_PERMITS).await;
        server.stop().await;
    }

    #[tokio::test]
    async fn tool_calls_are_contained_to_the_server_journal_root() {
        let (server_config, client_config) = tls_configs();
        let server = ServerHarness::start(server_config).await;
        seed_indexed_note(server.journal.path());

        let journal_b = tempfile::tempdir().expect("second fixture journal");
        seed_indexed_note(journal_b.path());
        let before_b = snapshot_tree(journal_b.path());

        let token = server.create_token("containment-agent");
        let mut client = connect_tls(server.address, client_config).await;
        let search = post_json(
            &mut client,
            &token.token,
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/call",
                "params": {"name": "search", "arguments": {"query": "needle", "limit": 10, "offset": 0}}
            }),
        )
        .await;
        assert_eq!(
            search["result"]["structuredContent"]["results"][0]["id"],
            "notes/mcp.txt:0"
        );
        let fetch = post_json(
            &mut client,
            &token.token,
            json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/call",
                "params": {"name": "fetch", "arguments": {"id": "notes/mcp.txt:0"}}
            }),
        )
        .await;
        assert_eq!(
            fetch["result"]["structuredContent"],
            json!({"content": "MCP fixture search needle"})
        );
        client.shutdown().await.expect("client closes TLS");
        drop(client);

        assert_eq!(snapshot_tree(journal_b.path()), before_b);
        let records = fs::read_dir(server.journal.path().join("chronicle"))
            .expect("audit day exists")
            .flat_map(|day| {
                fs::read_dir(day.expect("day entry").path().join("mcp.agent"))
                    .expect("audit stream exists")
            })
            .filter(|entry| {
                entry
                    .as_ref()
                    .expect("audit segment")
                    .path()
                    .join("interaction.json")
                    .is_file()
            })
            .count();
        assert_eq!(records, 2);
        wait_for_permits(&server.permits, CONNECTION_PERMITS).await;
        server.stop().await;
    }

    #[tokio::test]
    async fn revoked_bearer_rejects_an_existing_session_before_tool_dispatch() {
        let (server_config, client_config) = tls_configs();
        let server = ServerHarness::start(server_config).await;
        let token = server.create_token("revoked-session-agent");
        let mut client = connect_tls(server.address, client_config).await;
        let (_, headers) = post_json_with_headers(
            &mut client,
            &token.token,
            json!({"jsonrpc": "2.0", "id": 1, "method": "initialize"}),
            &[],
        )
        .await;
        let session = session_id(&headers);

        TokenStore::open(server.journal.path())
            .revoke("revoked-session-agent")
            .expect("fixture revokes bearer");
        let (status, body, head) = post_json_response_with_headers(
            &mut client,
            &token.token,
            json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
            &[("Mcp-Session-Id", &session)],
        )
        .await;
        assert_eq!(status, 401);
        assert_eq!(body, b"Bearer token is invalid or revoked");
        assert_eq!(
            www_authenticate(&head),
            Some(
                "Bearer realm=\"mcp\", resource_metadata=\"https://mcp.test/.well-known/oauth-protected-resource\""
            )
        );
        assert!(!server.journal.path().join("chronicle").exists());

        drop(client);
        wait_for_permits(&server.permits, CONNECTION_PERMITS).await;
        server.stop().await;
    }

    #[tokio::test]
    async fn oauth_discovery_and_unauthenticated_mcp_use_the_connection_pool() {
        let (server_config, client_config) = tls_configs();
        let server = ServerHarness::start(server_config).await;
        let origin = server.oauth.resource_origin.clone();
        let mut client = connect_tls(server.address, Arc::clone(&client_config)).await;

        let (status, body, _) = exchange_http(
            &mut client,
            &format!(
                "GET /.well-known/oauth-protected-resource HTTP/1.1\r\nHost: {HOSTNAME}\r\n\r\n"
            ),
        )
        .await;
        assert_eq!(status, 200);
        assert_eq!(
            serde_json::from_slice::<Value>(&body).expect("protected resource JSON"),
            json!({
                "resource": format!("{origin}/mcp"),
                "authorization_servers": [origin],
                "bearer_methods_supported": ["header"],
            })
        );

        let (status, body, _) = exchange_http(
            &mut client,
            &format!(
                "GET /.well-known/oauth-authorization-server HTTP/1.1\r\nHost: {HOSTNAME}\r\n\r\n"
            ),
        )
        .await;
        assert_eq!(status, 200);
        assert_eq!(
            serde_json::from_slice::<Value>(&body).expect("authorization server JSON"),
            json!({
                "issuer": origin,
                "authorization_endpoint": format!("{origin}/authorize"),
                "token_endpoint": format!("{origin}/token"),
                "registration_endpoint": format!("{origin}/register"),
                "response_types_supported": ["code"],
                "grant_types_supported": ["authorization_code", "refresh_token"],
                "code_challenge_methods_supported": ["S256"],
                "token_endpoint_auth_methods_supported": ["none"],
                "authorization_response_iss_parameter_supported": true,
                "client_id_metadata_document_supported": true,
            })
        );
        drop(client);

        let expected_challenge = format!(
            "Bearer realm=\"mcp\", resource_metadata=\"{origin}/.well-known/oauth-protected-resource\""
        );
        let mut client = connect_tls(server.address, Arc::clone(&client_config)).await;
        let (status, _, head) = exchange_http(
            &mut client,
            &format!("POST /mcp HTTP/1.1\r\nHost: {HOSTNAME}\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{{}}"),
        )
        .await;
        assert_eq!(status, 401);
        assert_eq!(www_authenticate(&head), Some(expected_challenge.as_str()));
        drop(client);

        let mut client = connect_tls(server.address, client_config).await;
        let (status, _, head) = exchange_http(
            &mut client,
            &format!("GET /mcp HTTP/1.1\r\nHost: {HOSTNAME}\r\n\r\n"),
        )
        .await;
        assert_eq!(status, 401);
        assert_eq!(www_authenticate(&head), Some(expected_challenge.as_str()));
        drop(client);

        wait_for_permits(&server.permits, CONNECTION_PERMITS).await;
        server.stop().await;
    }

    const CIMD_HOST: &str = "cimd.test";
    const CIMD_URL: &str = "https://cimd.test/cimd.json";
    const OAUTH_REDIRECT: &str = "http://127.0.0.1/callback";
    const OAUTH_VERIFIER: &str =
        "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-._~";

    fn pkce_challenge() -> String {
        use base64::Engine as _;
        use sha2::{Digest, Sha256};
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(Sha256::digest(OAUTH_VERIFIER.as_bytes()))
    }

    fn location_param(location: &str, name: &str) -> String {
        let query = location.split_once('?').expect("redirect has query").1;
        let prefix = format!("{name}=");
        query
            .split('&')
            .find_map(|piece| piece.strip_prefix(&prefix))
            .expect("parameter present")
            .to_owned()
    }

    fn hidden_transaction_id(body: &str) -> String {
        let marker = "name=\"transaction_id\" value=\"";
        let start = body.find(marker).expect("hidden transaction_id") + marker.len();
        let end = start + body[start..].find('"').expect("value terminator");
        body[start..end].to_owned()
    }

    async fn spawn_cimd_fixture() -> (
        SocketAddr,
        std::sync::Arc<ClientConfig>,
        tokio::task::JoinHandle<()>,
    ) {
        let (server_config, client_config) = tls_pair(CIMD_HOST);
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("CIMD fixture binds");
        let address = listener.local_addr().expect("CIMD fixture address");
        let json_body = format!(
            r#"{{"client_id":"{CIMD_URL}","redirect_uris":["{OAUTH_REDIRECT}"],"client_name":"fixture"}}"#
        );
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{json_body}",
            json_body.len()
        );
        let handle = tokio::spawn(async move {
            let acceptor = TlsAcceptor::from(server_config);
            loop {
                let Ok((socket, _)) = listener.accept().await else {
                    break;
                };
                let acceptor = acceptor.clone();
                let response = response.clone();
                tokio::spawn(async move {
                    let Ok(mut stream) = acceptor.accept(socket).await else {
                        return;
                    };
                    let mut head = Vec::new();
                    loop {
                        let mut byte = [0_u8; 1];
                        if stream.read_exact(&mut byte).await.is_err() {
                            return;
                        }
                        head.push(byte[0]);
                        if head.ends_with(b"\r\n\r\n") {
                            break;
                        }
                        if head.len() > 8192 {
                            return;
                        }
                    }
                    let _ = stream.write_all(response.as_bytes()).await;
                    let _ = stream.shutdown().await;
                });
            }
        });
        (address, client_config, handle)
    }

    async fn oauth_form(
        client: &mut TlsStream<TcpStream>,
        target: &str,
        body: &str,
    ) -> (u16, Vec<u8>, String) {
        exchange_http(
            client,
            &format!(
                "POST {target} HTTP/1.1\r\nHost: {HOSTNAME}\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            ),
        )
        .await
    }

    struct OauthNowGuard;

    impl Drop for OauthNowGuard {
        fn drop(&mut self) {
            crate::oauth::store::set_test_now(None);
        }
    }

    #[tokio::test]
    async fn oauth_end_to_end_through_the_tls_listener() {
        let (server_config, client_config) = tls_configs();
        let (cimd_addr, cimd_client, cimd_server) = spawn_cimd_fixture().await;
        let journal = tempfile::Builder::new()
            .prefix("solstone-mcp-server-")
            .tempdir_in("/var/tmp")
            .expect("fixture journal directory");
        seed_indexed_note(journal.path());
        let server = ServerHarness::start_with_journal_override(
            server_config,
            journal,
            Some((cimd_addr, cimd_client)),
        )
        .await;
        let origin = server.oauth.resource_origin.clone();
        let mut client = connect_tls(server.address, Arc::clone(&client_config)).await;

        let (status, body, _) = exchange_http(
            &mut client,
            &format!(
                "GET /.well-known/oauth-protected-resource HTTP/1.1\r\nHost: {HOSTNAME}\r\n\r\n"
            ),
        )
        .await;
        assert_eq!(status, 200);
        assert_eq!(
            serde_json::from_slice::<Value>(&body).unwrap()["resource"],
            format!("{origin}/mcp")
        );
        let (status, _, _) = exchange_http(
            &mut client,
            &format!(
                "GET /.well-known/oauth-authorization-server HTTP/1.1\r\nHost: {HOSTNAME}\r\n\r\n"
            ),
        )
        .await;
        assert_eq!(status, 200);

        let classic_body = serde_json::to_vec(&json!({
            "redirect_uris": [OAUTH_REDIRECT],
            "token_endpoint_auth_method": "none",
        }))
        .unwrap();
        let (status, body, _) = exchange_http(
            &mut client,
            &format!(
                "POST /register HTTP/1.1\r\nHost: {HOSTNAME}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                classic_body.len(),
                std::str::from_utf8(&classic_body).unwrap()
            ),
        )
        .await;
        assert_eq!(status, 201);
        let classic_id = serde_json::from_slice::<Value>(&body).unwrap()["client_id"]
            .as_str()
            .unwrap()
            .to_owned();
        assert!(classic_id.starts_with("oauth:dcr:"));

        let cimd_body = serde_json::to_vec(&json!({"client_id": CIMD_URL})).unwrap();
        let (status, body, _) = exchange_http(
            &mut client,
            &format!(
                "POST /register HTTP/1.1\r\nHost: {HOSTNAME}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                cimd_body.len(),
                std::str::from_utf8(&cimd_body).unwrap()
            ),
        )
        .await;
        assert_eq!(status, 201, "{}", String::from_utf8_lossy(&body));
        assert_eq!(
            serde_json::from_slice::<Value>(&body).unwrap()["client_id"],
            CIMD_URL
        );

        let challenge = pkce_challenge();
        let query = format!(
            "client_id={}&redirect_uri={}&response_type=code&code_challenge={}&code_challenge_method=S256&resource={}/mcp&state=st",
            crate::oauth::urlparse::query_value_encode(CIMD_URL),
            crate::oauth::urlparse::query_value_encode(OAUTH_REDIRECT),
            crate::oauth::urlparse::query_value_encode(&challenge),
            crate::oauth::urlparse::query_value_encode(&origin),
        );
        let (status, body, _) = exchange_http(
            &mut client,
            &format!("GET /authorize?{query} HTTP/1.1\r\nHost: {HOSTNAME}\r\n\r\n"),
        )
        .await;
        assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
        let html = String::from_utf8(body).unwrap();
        let transaction_id = hidden_transaction_id(&html);
        let pairing = server.oauth.store.generate_pairing_code().unwrap();
        let (status, _, head) = oauth_form(
            &mut client,
            "/authorize",
            &format!(
                "transaction_id={}&pairing_code={}",
                crate::oauth::urlparse::query_value_encode(&transaction_id),
                crate::oauth::urlparse::query_value_encode(&pairing.code)
            ),
        )
        .await;
        assert_eq!(status, 302);
        assert!(
            head.to_ascii_lowercase()
                .contains("cache-control: no-store")
        );
        let location = head
            .lines()
            .find_map(|line| line.strip_prefix("Location: "))
            .unwrap();
        let code = location_param(location, "code");
        assert!(!location_param(location, "state").is_empty());
        assert!(!location_param(location, "iss").is_empty());

        let (status, body, head) = oauth_form(
            &mut client,
            "/token",
            &format!(
                "grant_type=authorization_code&code={code}&redirect_uri={}&code_verifier={OAUTH_VERIFIER}&client_id={}&resource={origin}/mcp",
                crate::oauth::urlparse::query_value_encode(OAUTH_REDIRECT),
                crate::oauth::urlparse::query_value_encode(CIMD_URL),
            ),
        )
        .await;
        assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
        assert!(
            head.to_ascii_lowercase()
                .contains("cache-control: no-store")
        );
        let tokens = serde_json::from_slice::<Value>(&body).unwrap();
        let access = tokens["access_token"].as_str().unwrap().to_owned();
        let refresh = tokens["refresh_token"].as_str().unwrap().to_owned();
        let issued_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("wall clock after epoch")
            .as_secs() as i64;

        let (init, headers) = post_json_with_headers(
            &mut client,
            &access,
            json!({"jsonrpc": "2.0", "id": 1, "method": "initialize"}),
            &[],
        )
        .await;
        assert_eq!(init["jsonrpc"], "2.0");
        let session = session_id(&headers);
        let search = post_json_with_headers(
            &mut client,
            &access,
            json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/call",
                "params": {"name": "search", "arguments": {"query": "needle", "limit": 10, "offset": 0}},
            }),
            &[("Mcp-Session-Id", &session)],
        )
        .await
        .0;
        assert_eq!(
            search["result"]["structuredContent"]["results"][0]["id"],
            "notes/mcp.txt:0"
        );
        let fetch = post_json_with_headers(
            &mut client,
            &access,
            json!({
                "jsonrpc": "2.0",
                "id": 3,
                "method": "tools/call",
                "params": {"name": "fetch", "arguments": {"id": "notes/mcp.txt:0"}},
            }),
            &[("Mcp-Session-Id", &session)],
        )
        .await
        .0;
        assert_eq!(
            fetch["result"]["structuredContent"],
            json!({"content": "MCP fixture search needle"})
        );
        assert_eq!(audit_record_count(server.journal.path()), 2);

        {
            let _guard = OauthNowGuard;
            // 3601 exceeds ACCESS_TTL_SECS (3600) in oauth/store.rs.
            crate::oauth::store::set_test_now(Some(issued_at + 3601));
            let (status, _, _) = post_json_response_with_headers(
                &mut client,
                &access,
                json!({"jsonrpc": "2.0", "id": 8, "method": "initialize"}),
                &[("Mcp-Session-Id", &session)],
            )
            .await;
            assert_eq!(status, 401, "expired access token is rejected at /mcp");
        }
        drop(client);
        let mut client = connect_tls(server.address, Arc::clone(&client_config)).await;

        let (status, body, _) = oauth_form(
            &mut client,
            "/token",
            &format!(
                "grant_type=refresh_token&refresh_token={refresh}&client_id={}",
                crate::oauth::urlparse::query_value_encode(CIMD_URL)
            ),
        )
        .await;
        assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
        let rotated = serde_json::from_slice::<Value>(&body).unwrap();
        let access2 = rotated["access_token"].as_str().unwrap().to_owned();
        let refresh2 = rotated["refresh_token"].as_str().unwrap().to_owned();
        let (status, _, _) = oauth_form(
            &mut client,
            "/token",
            &format!(
                "grant_type=refresh_token&refresh_token={refresh}&client_id={}",
                crate::oauth::urlparse::query_value_encode(CIMD_URL)
            ),
        )
        .await;
        assert_eq!(status, 400);
        drop(client);
        let mut client = connect_tls(server.address, Arc::clone(&client_config)).await;
        post_json_with_headers(
            &mut client,
            &access2,
            json!({"jsonrpc": "2.0", "id": 4, "method": "initialize"}),
            &[],
        )
        .await;
        drop(client);

        let pairing = server.oauth.store.generate_pairing_code().unwrap();
        let challenge = pkce_challenge();
        let query = format!(
            "client_id={}&redirect_uri={}&response_type=code&code_challenge={}&code_challenge_method=S256&resource={origin}/mcp",
            crate::oauth::urlparse::query_value_encode(&classic_id),
            crate::oauth::urlparse::query_value_encode(OAUTH_REDIRECT),
            crate::oauth::urlparse::query_value_encode(&challenge),
        );
        let mut client = connect_tls(server.address, Arc::clone(&client_config)).await;
        let (status, body, _) = exchange_http(
            &mut client,
            &format!("GET /authorize?{query} HTTP/1.1\r\nHost: {HOSTNAME}\r\n\r\n"),
        )
        .await;
        assert_eq!(status, 200);
        let transaction_id = hidden_transaction_id(&String::from_utf8(body).unwrap());
        let (status, _, head) = oauth_form(
            &mut client,
            "/authorize",
            &format!(
                "transaction_id={}&pairing_code={}",
                crate::oauth::urlparse::query_value_encode(&transaction_id),
                crate::oauth::urlparse::query_value_encode(&pairing.code)
            ),
        )
        .await;
        assert_eq!(status, 302);
        let location = head
            .lines()
            .find_map(|line| line.strip_prefix("Location: "))
            .unwrap();
        let other_code = location_param(location, "code");
        let (status, body, _) = oauth_form(
            &mut client,
            "/token",
            &format!(
                "grant_type=authorization_code&code={other_code}&redirect_uri={}&code_verifier={OAUTH_VERIFIER}&client_id={}&resource={origin}/mcp",
                crate::oauth::urlparse::query_value_encode(OAUTH_REDIRECT),
                crate::oauth::urlparse::query_value_encode(&classic_id),
            ),
        )
        .await;
        assert_eq!(status, 200);
        let other_tokens = serde_json::from_slice::<Value>(&body).unwrap();
        let other_access = other_tokens["access_token"].as_str().unwrap().to_owned();
        drop(client);

        let journal = server.stop().await;
        let (server_config, client_config) = tls_configs();
        let server = ServerHarness::start_with_journal(server_config, journal).await;
        let mut client = connect_tls(server.address, Arc::clone(&client_config)).await;
        let (status, body, _) = oauth_form(
            &mut client,
            "/token",
            &format!(
                "grant_type=refresh_token&refresh_token={refresh2}&client_id={}",
                crate::oauth::urlparse::query_value_encode(CIMD_URL)
            ),
        )
        .await;
        assert_eq!(status, 200, "rotated refresh survives restart");
        let after_restart = serde_json::from_slice::<Value>(&body).unwrap();
        let access3 = after_restart["access_token"].as_str().unwrap().to_owned();
        let refresh3 = after_restart["refresh_token"].as_str().unwrap().to_owned();
        let (status, _, _) = oauth_form(
            &mut client,
            "/token",
            &format!(
                "grant_type=refresh_token&refresh_token={refresh}&client_id={}",
                crate::oauth::urlparse::query_value_encode(CIMD_URL)
            ),
        )
        .await;
        assert_eq!(status, 400, "pre-rotation refresh stays dead after restart");
        drop(client);
        let mut client = connect_tls(server.address, Arc::clone(&client_config)).await;
        post_json_with_headers(
            &mut client,
            &access3,
            json!({"jsonrpc": "2.0", "id": 5, "method": "initialize"}),
            &[],
        )
        .await;
        drop(client);

        server
            .oauth
            .store
            .revoke_client_by_client_id(CIMD_URL)
            .unwrap();
        let mut client = connect_tls(server.address, Arc::clone(&client_config)).await;
        let (status, _, _) = post_json_response_with_headers(
            &mut client,
            &access3,
            json!({"jsonrpc": "2.0", "id": 6, "method": "initialize"}),
            &[],
        )
        .await;
        assert_eq!(status, 401);
        drop(client);
        let mut client = connect_tls(server.address, Arc::clone(&client_config)).await;
        let (status, _, _) = oauth_form(
            &mut client,
            "/token",
            &format!(
                "grant_type=refresh_token&refresh_token={refresh3}&client_id={}",
                crate::oauth::urlparse::query_value_encode(CIMD_URL)
            ),
        )
        .await;
        assert_eq!(status, 400);
        drop(client);
        let mut client = connect_tls(server.address, client_config).await;
        post_json_with_headers(
            &mut client,
            &other_access,
            json!({"jsonrpc": "2.0", "id": 7, "method": "initialize"}),
            &[],
        )
        .await;
        drop(client);
        cimd_server.abort();
        wait_for_permits(&server.permits, CONNECTION_PERMITS).await;
        server.stop().await;
    }

    #[tokio::test]
    async fn oauth_bulkhead_saturation_does_not_block_other_traffic() {
        let (server_config, client_config) = tls_configs();
        let server = ServerHarness::start(server_config).await;
        seed_indexed_note(server.journal.path());
        let mut held = Vec::new();
        for octet in 1..=16 {
            held.push(
                server
                    .oauth
                    .cimd_bulkhead
                    .try_acquire(std::net::IpAddr::V4(std::net::Ipv4Addr::new(
                        203, 0, 113, octet,
                    )))
                    .expect("bulkhead admits stalled connector"),
            );
        }
        let mut client = connect_tls(server.address, Arc::clone(&client_config)).await;
        let classic_body = serde_json::to_vec(&json!({
            "redirect_uris": [OAUTH_REDIRECT],
            "token_endpoint_auth_method": "none",
        }))
        .unwrap();
        let (status, body, _) = exchange_http(
            &mut client,
            &format!(
                "POST /register HTTP/1.1\r\nHost: {HOSTNAME}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                classic_body.len(),
                std::str::from_utf8(&classic_body).unwrap()
            ),
        )
        .await;
        assert_eq!(status, 201);
        let classic_id = serde_json::from_slice::<Value>(&body).unwrap()["client_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let pairing = server.oauth.store.generate_pairing_code().unwrap();
        let challenge = pkce_challenge();
        let origin = server.oauth.resource_origin.clone();
        let query = format!(
            "client_id={}&redirect_uri={}&response_type=code&code_challenge={}&code_challenge_method=S256&resource={origin}/mcp",
            crate::oauth::urlparse::query_value_encode(&classic_id),
            crate::oauth::urlparse::query_value_encode(OAUTH_REDIRECT),
            crate::oauth::urlparse::query_value_encode(&challenge),
        );
        let (status, body, _) = exchange_http(
            &mut client,
            &format!("GET /authorize?{query} HTTP/1.1\r\nHost: {HOSTNAME}\r\n\r\n"),
        )
        .await;
        assert_eq!(status, 200);
        let transaction_id = hidden_transaction_id(&String::from_utf8(body).unwrap());
        let (status, _, head) = oauth_form(
            &mut client,
            "/authorize",
            &format!(
                "transaction_id={}&pairing_code={}",
                crate::oauth::urlparse::query_value_encode(&transaction_id),
                crate::oauth::urlparse::query_value_encode(&pairing.code)
            ),
        )
        .await;
        assert_eq!(status, 302);
        let location = head
            .lines()
            .find_map(|line| line.strip_prefix("Location: "))
            .unwrap();
        let code = location_param(location, "code");
        let (status, body, _) = oauth_form(
            &mut client,
            "/token",
            &format!(
                "grant_type=authorization_code&code={code}&redirect_uri={}&code_verifier={OAUTH_VERIFIER}&client_id={}&resource={origin}/mcp",
                crate::oauth::urlparse::query_value_encode(OAUTH_REDIRECT),
                crate::oauth::urlparse::query_value_encode(&classic_id),
            ),
        )
        .await;
        assert_eq!(status, 200);
        let tokens = serde_json::from_slice::<Value>(&body).unwrap();
        let access = tokens["access_token"].as_str().unwrap().to_owned();
        let refresh = tokens["refresh_token"].as_str().unwrap().to_owned();
        post_json_with_headers(
            &mut client,
            &access,
            json!({"jsonrpc": "2.0", "id": 1, "method": "initialize"}),
            &[],
        )
        .await;
        let static_token = server.create_token("static-agent");
        post_json_with_headers(
            &mut client,
            &static_token.token,
            json!({"jsonrpc": "2.0", "id": 2, "method": "initialize"}),
            &[],
        )
        .await;
        let (status, _, _) = oauth_form(
            &mut client,
            "/token",
            &format!(
                "grant_type=refresh_token&refresh_token={refresh}&client_id={}",
                crate::oauth::urlparse::query_value_encode(&classic_id)
            ),
        )
        .await;
        assert_eq!(status, 200);
        server
            .oauth
            .store
            .revoke_client_by_client_id(&classic_id)
            .unwrap();
        drop(held);
        drop(client);
        wait_for_permits(&server.permits, CONNECTION_PERMITS).await;
        server.stop().await;
    }

    #[tokio::test]
    async fn oauth_endpoints_respond_immediately_on_cold_start() {
        let (server_config, client_config) = tls_configs();
        let server = ServerHarness::start(server_config).await;
        let mut client = connect_tls(server.address, Arc::clone(&client_config)).await;
        let first = tokio::time::timeout(
            Duration::from_secs(2),
            exchange_http(
                &mut client,
                &format!(
                    "GET /.well-known/oauth-protected-resource HTTP/1.1\r\nHost: {HOSTNAME}\r\n\r\n"
                ),
            ),
        )
        .await
        .expect("discovery does not wait on think/indexer");
        assert_eq!(first.0, 200);
        let authorize = tokio::time::timeout(
            Duration::from_secs(2),
            exchange_http(
                &mut client,
                &format!("GET /authorize HTTP/1.1\r\nHost: {HOSTNAME}\r\n\r\n"),
            ),
        )
        .await
        .expect("authorize does not wait on think/indexer");
        assert_eq!(authorize.0, 400);
        let token = tokio::time::timeout(
            Duration::from_secs(2),
            oauth_form(&mut client, "/token", "grant_type=password"),
        )
        .await
        .expect("token does not wait on think/indexer");
        assert_eq!(token.0, 400);
        drop(client);
        let mut client = connect_tls(server.address, client_config).await;
        let register = tokio::time::timeout(
            Duration::from_secs(2),
            exchange_http(
                &mut client,
                "POST /register HTTP/1.1\r\nHost: mcp.test\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{}",
            ),
        )
        .await
        .expect("DCR does not wait on think/indexer");
        assert_eq!(register.0, 400);
        drop(client);
        wait_for_permits(&server.permits, CONNECTION_PERMITS).await;
        server.stop().await;
    }
}

#[cfg(all(test, not(feature = "full-tests")))]
mod unit_tests {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    use super::PrefixedStream;

    #[tokio::test]
    async fn prefixed_stream_replays_previously_read_bytes_before_the_socket() {
        let (mut writer, reader) = tokio::io::duplex(32);
        writer
            .write_all(b"socket-bytes")
            .await
            .expect("fixture writes socket bytes");
        writer.shutdown().await.expect("fixture closes writer");

        let mut stream = PrefixedStream::new(reader, b"preface-tail".to_vec());
        let mut bytes = Vec::new();
        stream
            .read_to_end(&mut bytes)
            .await
            .expect("prefixed stream reads");
        assert_eq!(bytes, b"preface-tailsocket-bytes");
    }
}
