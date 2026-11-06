// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Browser authorization for the loopback browser port.
//!
//! The owner's own programs reach the journal over the owner socket, which
//! admits only this OS account. A browser can't use that socket, so the owner
//! asks the journal over the socket for a one-time authorization link, and the
//! browser opens it on the TCP port. Spending the link sets a cookie that is
//! held in memory only, so it ends when the journal restarts.
//!
//! The owner's "open local access" choice decides what TCP serves:
//!
//! - **open** (the default): TCP serves every request as the owner, as it
//!   always has. Links still work and are harmless.
//! - **closed**: TCP serves only a browser holding a cookie, or a valid link on
//!   the bootstrap route. Browsers otherwise get a neutral page that names
//!   nothing, and programs get a 409 JSON pointer to `solstone journal open`.
//!
//! Closing needs a live owner socket. Without one the owner's own programs
//! could not reach the journal at all, and no link could be minted to get back
//! in, so the port stays open and the setting reports itself unavailable.
//!
//! The gate sits on the loopback browser port alone. The owner socket, the
//! paired-device door and the agent doors have their own admission.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::header::{
    ACCEPT, CACHE_CONTROL, CONTENT_TYPE, COOKIE, LOCATION, REFERRER_POLICY, SET_COOKIE,
};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{Value, json};
use solstone_core_convey_http::envelope::error_envelope;
use solstone_core_convey_http::identity::AccessBasis;

/// The host every link uses. A cookie set on one loopback name is not sent to
/// another, so links, the cookie and printed addresses all use this one.
pub(crate) const LINK_HOST: &str = "localhost";
/// How long a minted link can be spent.
pub(crate) const LINK_LIFETIME: Duration = Duration::from_secs(60);
/// Socket-only: mint a one-time link for a path.
pub(crate) const MINT_ROUTE: &str = "/_solstone/browser-link";
/// The owner's choice, read and changed over the socket or an admitted browser.
pub(crate) const SETTING_ROUTE: &str = "/app/settings/api/local-access";
/// The `config/journal.json` key that holds the owner's choice.
const CONFIG_KEY: &str = "local_access";
const AUTH_PARAM: &str = "auth";
const MAX_OUTSTANDING_LINKS: usize = 32;
const MAX_SESSIONS: usize = 64;
const MAX_PATH_BYTES: usize = 2048;

#[cfg(windows)]
const CLOSED_PROGRAM_MESSAGE: &str = "this journal only opens for the account it belongs to. if it's yours and this program runs another way, as a service or inside WSL, you can open local access in your journal's settings.";
#[cfg(not(windows))]
const CLOSED_PROGRAM_MESSAGE: &str = "programs on this computer can no longer open this journal here. if it's yours, run solstone journal open to open it in a browser, or update this program.";
const CONSENT_NEEDED: &str = "opening local access lets any account on this computer, and any program running here, open your journal without asking. confirm to turn it on.";
const UNAVAILABLE: &str = "this choice needs your journal's private connection on this computer, which isn't running right now. local access stays open until it is.";
const SAVE_FAILED: &str = "your choice could not be saved. please try again.";

/// Live browser-authorization state for one running journal.
pub(crate) struct BrowserAccess {
    journal_root: PathBuf,
    port: u16,
    open: AtomicBool,
    /// Closing is honoured only while the owner socket is serving.
    socket_live: AtomicBool,
    links: Mutex<HashMap<String, PendingLink>>,
    sessions: Mutex<Sessions>,
}

struct PendingLink {
    path: String,
    expires: Instant,
}

#[derive(Default)]
struct Sessions {
    live: HashSet<String>,
    order: VecDeque<String>,
}

impl Sessions {
    fn insert(&mut self, session: String) {
        if self.live.insert(session.clone()) {
            self.order.push_back(session);
        }
        while self.order.len() > MAX_SESSIONS {
            if let Some(oldest) = self.order.pop_front() {
                self.live.remove(&oldest);
            }
        }
    }
}

impl BrowserAccess {
    pub(crate) fn load(journal_root: &Path, port: u16) -> Arc<Self> {
        Arc::new(Self {
            journal_root: journal_root.to_owned(),
            port,
            open: AtomicBool::new(saved_open(journal_root)),
            socket_live: AtomicBool::new(false),
            links: Mutex::new(HashMap::new()),
            sessions: Mutex::new(Sessions::default()),
        })
    }

    /// Record whether the owner socket came up for this run.
    pub(crate) fn set_socket_live(&self, live: bool) {
        self.socket_live.store(live, Ordering::SeqCst);
    }

    fn socket_live(&self) -> bool {
        self.socket_live.load(Ordering::SeqCst)
    }

    /// Whether the browser port admits only authorized browsers right now.
    fn closed(&self) -> bool {
        self.socket_live() && !self.open.load(Ordering::SeqCst)
    }

    fn cookie_name(&self) -> String {
        // Two journals on one computer use two ports but share a cookie host.
        format!("solstone_authz_{}", self.port)
    }

    fn mint(&self, path: &str, now: Instant) -> Result<String, &'static str> {
        let path = valid_path(path).ok_or("the path must start with a single /")?;
        let token = solstone_core_handoff_nonce::mint_nonce().map_err(|_| "no randomness")?;
        let mut links = self.links.lock().unwrap_or_else(|e| e.into_inner());
        links.retain(|_, link| link.expires > now);
        if links.len() >= MAX_OUTSTANDING_LINKS {
            return Err("too many unused links; try again in a minute");
        }
        links.insert(
            token.clone(),
            PendingLink {
                path: path.to_owned(),
                expires: now + LINK_LIFETIME,
            },
        );
        Ok(format!(
            "http://{LINK_HOST}:{}/?{AUTH_PARAM}={token}",
            self.port
        ))
    }

    /// Spend a link once. A spent or expired link is gone either way.
    fn spend(&self, token: &str, now: Instant) -> Option<String> {
        let link = self
            .links
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(token)?;
        (link.expires > now).then_some(link.path)
    }

    /// End every browser session, e.g. one taken while the port was open.
    fn end_sessions(&self) {
        *self.sessions.lock().unwrap_or_else(|e| e.into_inner()) = Sessions::default();
    }

    fn new_session(&self) -> Option<String> {
        let session = solstone_core_handoff_nonce::mint_nonce().ok()?;
        self.sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(session.clone());
        Some(session)
    }

    fn admitted(&self, headers: &HeaderMap) -> bool {
        let name = self.cookie_name();
        let sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        cookie_values(headers, &name).any(|value| sessions.live.contains(value))
    }

    fn set_cookie(&self, session: &str) -> HeaderValue {
        HeaderValue::from_str(&format!(
            "{}={session}; Path=/; HttpOnly; SameSite=Lax",
            self.cookie_name()
        ))
        .expect("cookie is ASCII")
    }

    fn snapshot(&self) -> Value {
        let open = self.open.load(Ordering::SeqCst);
        json!({
            "available": self.socket_live(),
            "open": open,
            "closed_now": self.closed(),
            "message": (!self.socket_live()).then_some(UNAVAILABLE),
            "account_check": cfg!(windows),
        })
    }
}

/// Whether the port can be closed. Without an owner socket, a Mac or Linux owner
/// would have no way back in. On Windows the account check admits the owner over
/// TCP, so a closed journal stays closed whether or not its pipe came up.
pub(crate) fn can_close(socket_up: bool) -> bool {
    socket_up || cfg!(windows)
}

/// A journal with no saved choice stays open: it predates new journals starting
/// closed, and an upgrade doesn't change what its owner relies on. Setup writes
/// the choice when it creates a journal.
fn saved_open(root: &Path) -> bool {
    solstone_core_journal_config::read_journal_config(root)
        .ok()
        .and_then(|read| read.config)
        .and_then(|config| config.get(CONFIG_KEY)?.get("open")?.as_bool())
        .unwrap_or(true)
}

/// A same-origin path: one leading slash, nothing that names another host.
fn valid_path(path: &str) -> Option<&str> {
    let path = if path.is_empty() { "/" } else { path };
    (path.starts_with('/')
        && !path.starts_with("//")
        && path.len() <= MAX_PATH_BYTES
        && !path.contains('\\')
        && path.bytes().all(|byte| byte.is_ascii_graphic()))
    .then_some(path)
}

fn cookie_values<'a>(headers: &'a HeaderMap, name: &'a str) -> impl Iterator<Item = &'a str> {
    headers
        .get_all(COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .filter_map(move |pair| {
            let (key, value) = pair.trim().split_once('=')?;
            (key == name).then_some(value)
        })
}

fn auth_token(request: &Request) -> Option<&str> {
    if request.uri().path() != "/" || request.method() != Method::GET {
        return None;
    }
    request.uri().query()?.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        (key == AUTH_PARAM).then_some(value)
    })
}

fn is_navigation(request: &Request) -> bool {
    let headers = request.headers();
    if headers
        .get("sec-fetch-mode")
        .is_some_and(|mode| mode == "navigate")
    {
        return true;
    }
    matches!(*request.method(), Method::GET | Method::HEAD)
        && headers
            .get(ACCEPT)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|accept| accept.contains("text/html"))
}

/// One accepted connection on the browser port, and the answer to whose it is.
/// Windows can say, so it asks once, the first time a closed port needs to know.
#[cfg(windows)]
#[derive(Clone)]
pub(crate) struct TcpConnection {
    server: std::net::SocketAddr,
    client: std::net::SocketAddr,
    owner: Arc<tokio::sync::OnceCell<bool>>,
}

#[cfg(windows)]
impl TcpConnection {
    pub(crate) fn new(server: std::net::SocketAddr, client: std::net::SocketAddr) -> Self {
        Self {
            server,
            client,
            owner: Arc::default(),
        }
    }
}

/// Whether this connection comes from the owner's own account.
#[cfg(windows)]
async fn owner_account(connection: Option<TcpConnection>) -> bool {
    let Some(connection) = connection else {
        return false;
    };
    *connection
        .owner
        .get_or_init(|| async {
            let (server, client) = (connection.server, connection.client);
            tokio::task::spawn_blocking(move || {
                solstone_core_local_endpoint::connection_is_owner(server, client)
            })
            .await
            .unwrap_or(false)
        })
        .await
}

#[cfg(windows)]
fn connection_of(request: &Request) -> Option<TcpConnection> {
    request.extensions().get::<TcpConnection>().cloned()
}

/// Only a browser's cookie can speak for the owner here.
#[cfg(not(windows))]
async fn owner_account(_connection: Option<()>) -> bool {
    false
}

#[cfg(not(windows))]
fn connection_of(_request: &Request) -> Option<()> {
    None
}

/// The browser port's admission gate. Supplies the owner basis only after
/// admission; nothing past it is reached without one.
pub(crate) async fn gate(
    State(access): State<Arc<BrowserAccess>>,
    mut request: Request,
    next: Next,
) -> Response {
    let connection = connection_of(&request);
    if let Some(token) = auth_token(&request).map(str::to_owned) {
        let admitted = access.admitted(request.headers()) || owner_account(connection).await;
        return bootstrap(&access, admitted, &token);
    }
    if !access.closed() || access.admitted(request.headers()) || owner_account(connection).await {
        request.extensions_mut().insert(AccessBasis::Localhost);
        return next.run(request).await;
    }
    if is_navigation(&request) {
        return landing(false);
    }
    let (status, body) = error_envelope(
        "local_access_closed",
        CLOSED_PROGRAM_MESSAGE,
        "",
        StatusCode::CONFLICT,
    );
    let mut response = (status, body).into_response();
    no_store(response.headers_mut());
    response
}

fn bootstrap(access: &BrowserAccess, admitted: bool, token: &str) -> Response {
    let spent = access.spend(token, Instant::now());
    let (path, session) = match spent {
        Some(path) => (path, access.new_session()),
        None if access.closed() && !admitted => return landing(true),
        // Open, or a browser that already has access (a link it queued past
        // its lifetime, or one reopened from history): a spent link goes home.
        None => ("/".to_owned(), None),
    };
    let mut response = Response::builder()
        .status(StatusCode::FOUND)
        .header(LOCATION, path)
        .body(Body::empty())
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response());
    if let Some(session) = session {
        response
            .headers_mut()
            .insert(SET_COOKIE, access.set_cookie(&session));
    }
    no_store(response.headers_mut());
    response
}

fn no_store(headers: &mut HeaderMap) {
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
}

fn landing(link_refused: bool) -> Response {
    let refused = if link_refused {
        "<p>this link has already been used or has expired.</p>"
    } else {
        ""
    };
    let html = format!("{LANDING_HEAD}{refused}{LANDING_TAIL}");
    let mut response = (
        StatusCode::FORBIDDEN,
        [(CONTENT_TYPE, "text/html; charset=utf-8")],
        html,
    )
        .into_response();
    no_store(response.headers_mut());
    response
}

const LANDING_HEAD: &str = "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"><meta name=\"referrer\" content=\"no-referrer\"><title>journal</title><style>body{font-family:system-ui,-apple-system,sans-serif;max-width:32rem;margin:15vh auto;padding:0 1.5rem;line-height:1.5;color:#222;background:#fafaf8}code{background:#eee;padding:.1rem .3rem;border-radius:.2rem}@media(prefers-color-scheme:dark){body{color:#ddd;background:#1b1b1a}code{background:#333}}</style></head><body><h1>open your journal</h1>";
#[cfg(windows)]
const LANDING_TAIL: &str = "<p>if this journal is yours, open it from the journal app, or run <code>solstone journal open</code> in a terminal.</p></body></html>";
#[cfg(not(windows))]
const LANDING_TAIL: &str = "<p>open your journal from the journal app on a mac, or run <code>solstone journal open</code> in a terminal.</p></body></html>";

/// Routes only the owner socket serves.
pub(crate) fn socket_routes(access: Arc<BrowserAccess>) -> Router {
    Router::new().route(
        MINT_ROUTE,
        post(move |body: Option<Json<Value>>| {
            let access = access.clone();
            async move {
                let path = body
                    .as_ref()
                    .and_then(|Json(body)| body.get("path")?.as_str().map(str::to_owned))
                    .unwrap_or_else(|| "/".to_owned());
                match access.mint(&path, Instant::now()) {
                    Ok(url) => (
                        [(CACHE_CONTROL, "no-store")],
                        Json(json!({"url": url, "expires_in": LINK_LIFETIME.as_secs()})),
                    )
                        .into_response(),
                    Err(message) => {
                        (StatusCode::BAD_REQUEST, Json(json!({"error": message}))).into_response()
                    }
                }
            }
        }),
    )
}

/// The owner's choice, served on the socket and to an admitted browser.
pub(crate) fn setting_routes(access: Arc<BrowserAccess>) -> Router {
    let read = access.clone();
    Router::new().route(
        SETTING_ROUTE,
        get(move || {
            let access = read.clone();
            async move { ([(CACHE_CONTROL, "no-store")], Json(access.snapshot())) }
        })
        .put(move |headers: HeaderMap, Json(body): Json<Value>| {
            let access = access.clone();
            async move { change(access, &headers, body).await }
        }),
    )
}

async fn change(access: Arc<BrowserAccess>, headers: &HeaderMap, body: Value) -> Response {
    let Some(open) = body.get("open").and_then(Value::as_bool) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "say whether local access is open: {\"open\": true} or {\"open\": false}."})),
        )
            .into_response();
    };
    if !access.socket_live() {
        return (StatusCode::CONFLICT, Json(json!({"error": UNAVAILABLE}))).into_response();
    }
    let currently_open = access.open.load(Ordering::SeqCst);
    if open && !currently_open && body.get("consent").and_then(Value::as_bool) != Some(true) {
        return (
            StatusCode::CONFLICT,
            Json(json!({"error": CONSENT_NEEDED, "consent_required": true})),
        )
            .into_response();
    }
    let root = access.journal_root.clone();
    let saved = tokio::task::spawn_blocking(move || save(&root, open)).await;
    if !matches!(saved, Ok(Ok(()))) {
        if let Ok(Err(error)) = &saved {
            log::warn!("local access: {error}");
        }
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": SAVE_FAILED})),
        )
            .into_response();
    }
    // Closing ends every browser session. The browser that closed the port
    // keeps working: it was already the owner. A program gets no cookie.
    let session = if open {
        None
    } else {
        access.end_sessions();
        headers
            .contains_key("sec-fetch-site")
            .then(|| access.new_session())
            .flatten()
    };
    access.open.store(open, Ordering::SeqCst);
    let mut response = Json(access.snapshot()).into_response();
    if let Some(session) = session {
        response
            .headers_mut()
            .insert(SET_COOKIE, access.set_cookie(&session));
    }
    no_store(response.headers_mut());
    response
}

fn save(root: &Path, open: bool) -> Result<(), String> {
    let mut entry = serde_json::Map::new();
    entry.insert("open".into(), Value::Bool(open));
    if open {
        entry.insert(
            "opened_with_consent_at".into(),
            Value::String(chrono::Utc::now().to_rfc3339()),
        );
    }
    solstone_core_journal_config_write::mutate_journal_config(root, Default::default(), |config| {
        config.insert(CONFIG_KEY.into(), Value::Object(entry));
        solstone_core_journal_config_write::JournalConfigMutation {
            changed: true,
            value: (),
        }
    })
    .map(|_| ())
    .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::Request as HttpRequest;
    use axum::middleware::from_fn_with_state;
    use axum::routing::get;
    use tower::ServiceExt;

    fn access(open: bool, socket_live: bool) -> Arc<BrowserAccess> {
        Arc::new(BrowserAccess {
            journal_root: PathBuf::from("/nonexistent"),
            port: 5015,
            open: AtomicBool::new(open),
            socket_live: AtomicBool::new(socket_live),
            links: Mutex::new(HashMap::new()),
            sessions: Mutex::new(Sessions::default()),
        })
    }

    fn app(access: Arc<BrowserAccess>) -> Router {
        Router::new()
            .route(
                "/data",
                get(|basis: Option<axum::Extension<AccessBasis>>| async move {
                    format!("{:?}", basis.map(|b| b.0))
                }),
            )
            .layer(from_fn_with_state(access, gate))
    }

    async fn call(app: &Router, request: HttpRequest<Body>) -> Response {
        app.clone().oneshot(request).await.unwrap()
    }

    fn token_of(url: &str) -> &str {
        url.split("auth=").nth(1).unwrap()
    }

    #[tokio::test]
    async fn open_port_serves_the_owner_as_today() {
        let app = app(access(true, true));
        let response = call(&app, HttpRequest::get("/data").body(Body::empty()).unwrap()).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap();
        assert_eq!(&body[..], b"Some(Localhost)");
    }

    #[tokio::test]
    async fn closed_port_refuses_programs_with_json_and_browsers_with_a_page() {
        let app = app(access(false, true));
        let program = call(&app, HttpRequest::get("/data").body(Body::empty()).unwrap()).await;
        assert_eq!(program.status(), StatusCode::CONFLICT);
        assert!(
            program.headers()[CONTENT_TYPE]
                .to_str()
                .unwrap()
                .starts_with("application/json")
        );
        let browser = call(
            &app,
            HttpRequest::get("/data")
                .header("sec-fetch-mode", "navigate")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(browser.status(), StatusCode::FORBIDDEN);
        let body = axum::body::to_bytes(browser.into_body(), 8192)
            .await
            .unwrap();
        assert!(String::from_utf8_lossy(&body).contains("solstone journal open"));
    }

    #[tokio::test]
    async fn a_link_is_spent_once_and_its_cookie_admits_the_browser() {
        let access = access(false, true);
        let app = app(access.clone());
        let url = access.mint("/data", Instant::now()).unwrap();
        assert!(url.starts_with("http://localhost:5015/?auth="));
        let first = call(
            &app,
            HttpRequest::get(format!("/?auth={}", token_of(&url)))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(first.status(), StatusCode::FOUND);
        assert_eq!(first.headers()[LOCATION], "/data");
        assert_eq!(first.headers()[REFERRER_POLICY], "no-referrer");
        let cookie = first.headers()[SET_COOKIE].to_str().unwrap().to_owned();
        assert!(cookie.contains("HttpOnly") && cookie.contains("SameSite=Lax"));
        assert!(!cookie.contains("Domain") && !cookie.contains("Max-Age"));
        let pair = cookie.split(';').next().unwrap().to_owned();

        let again = call(
            &app,
            HttpRequest::get(format!("/?auth={}", token_of(&url)))
                .header("accept", "text/html")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(again.status(), StatusCode::FORBIDDEN);
        assert!(again.headers().get(SET_COOKIE).is_none());

        // The browser that spent it already has access, so the spent link goes home.
        let again_admitted = call(
            &app,
            HttpRequest::get(format!("/?auth={}", token_of(&url)))
                .header("accept", "text/html")
                .header("cookie", pair.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(again_admitted.status(), StatusCode::FOUND);
        assert_eq!(again_admitted.headers()[LOCATION], "/");
        assert!(again_admitted.headers().get(SET_COOKIE).is_none());

        let admitted = call(
            &app,
            HttpRequest::get("/data")
                .header("cookie", format!("other=1; {pair}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(admitted.status(), StatusCode::OK);
        let wrong = call(
            &app,
            HttpRequest::get("/data")
                .header("cookie", "solstone_authz_5015=guess")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(wrong.status(), StatusCode::CONFLICT);
    }

    #[test]
    fn an_expired_link_is_refused() {
        let access = access(false, true);
        let start = Instant::now();
        let url = access.mint("/", start).unwrap();
        assert!(
            access
                .spend(
                    token_of(&url),
                    start + LINK_LIFETIME + Duration::from_secs(1)
                )
                .is_none()
        );
        assert!(access.spend(token_of(&url), start).is_none());
    }

    #[test]
    fn links_only_go_to_this_origin() {
        let access = access(true, true);
        for bad in [
            "//elsewhere.example/",
            "https://elsewhere.example/",
            "/a\\b",
            "/a b",
        ] {
            assert!(access.mint(bad, Instant::now()).is_err(), "{bad}");
        }
        assert!(access.mint("/init", Instant::now()).is_ok());
        assert!(access.mint("", Instant::now()).unwrap().contains("auth="));
    }

    #[tokio::test]
    async fn without_a_live_socket_closing_does_not_close_the_port() {
        let app = app(access(false, false));
        let response = call(&app, HttpRequest::get("/data").body(Body::empty()).unwrap()).await;
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[test]
    fn ending_sessions_refuses_an_earlier_browser() {
        let access = access(false, true);
        let session = access.new_session().unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(
            COOKIE,
            HeaderValue::from_str(&format!("solstone_authz_5015={session}")).unwrap(),
        );
        assert!(access.admitted(&headers));
        access.end_sessions();
        assert!(!access.admitted(&headers));
    }

    #[tokio::test]
    async fn turning_local_access_on_needs_consent() {
        let access = access(false, true);
        let response = change(access.clone(), &HeaderMap::new(), json!({"open": true})).await;
        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert!(access.closed());
    }
}

#[cfg(all(test, feature = "full-tests"))]
#[cfg(windows)]
mod network_tests {
    use super::*;
    use axum::http::Request as HttpRequest;
    use axum::middleware::from_fn_with_state;
    use tower::ServiceExt;

    #[tokio::test]
    async fn a_closed_journal_with_no_pipe_still_refuses_a_stranger() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let server = listener.local_addr().unwrap();
        let _client = std::net::TcpStream::connect(server).unwrap();
        let (_accepted, peer) = listener.accept().unwrap();
        let stranger = std::net::SocketAddr::new(peer.ip(), peer.port().wrapping_add(1));
        let access = BrowserAccess::load(std::path::Path::new("nonexistent"), 5015);
        access.open.store(false, Ordering::SeqCst);
        access.set_socket_live(can_close(false));
        let app = Router::new()
            .route("/anything", get(|| async { "ok" }))
            .layer(from_fn_with_state(access, gate));
        let mut request = HttpRequest::builder()
            .uri("/anything")
            .body(Body::empty())
            .unwrap();
        request
            .extensions_mut()
            .insert(TcpConnection::new(server, stranger));
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn a_closed_port_admits_this_account_and_no_stranger() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let server = listener.local_addr().unwrap();
        let _client = std::net::TcpStream::connect(server).unwrap();
        let (_accepted, peer) = listener.accept().unwrap();
        let stranger = std::net::SocketAddr::new(peer.ip(), peer.port().wrapping_add(1));
        let access = BrowserAccess::load(std::path::Path::new("nonexistent"), 5015);
        access.open.store(false, Ordering::SeqCst);
        access.set_socket_live(true);
        let app = Router::new()
            .route("/anything", get(|| async { "ok" }))
            .layer(from_fn_with_state(access, gate));
        for (client, expected) in [(peer, StatusCode::OK), (stranger, StatusCode::CONFLICT)] {
            let mut request = HttpRequest::builder()
                .uri("/anything")
                .body(Body::empty())
                .unwrap();
            request
                .extensions_mut()
                .insert(TcpConnection::new(server, client));
            let response = app.clone().oneshot(request).await.unwrap();
            assert_eq!(response.status(), expected);
        }
    }
}
