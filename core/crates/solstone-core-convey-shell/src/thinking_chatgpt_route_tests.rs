// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Thinking API routes for Sign in with ChatGPT, against a loopback stand-in for OpenAI.

use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread;

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use serde_json::{Value, json};
use solstone_core_thinking::chatgpt;
use tower::ServiceExt;

/// The pending-attempt registry and the endpoint overrides are process-wide, so every test
/// that drives the ChatGPT routes holds this.
pub(crate) static CHATGPT_ROUTES: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

const ATTEMPT_STATES: [&str; 4] = ["pending", "signed_in", "failed", "expired"];

const CLOSED_REASONS: [&str; 12] = [
    "signed_in",
    "denied",
    "callback_invalid",
    "plan_usage_not_granted",
    "account_mismatch",
    "registration_refused",
    "exchange_failed",
    "superseded",
    "busy",
    "storage",
    "cancelled",
    "expired",
];

#[derive(Debug, Clone)]
struct StubRequest {
    method: String,
    path: String,
    form: BTreeMap<String, String>,
    bearer: Option<String>,
}

#[derive(Default)]
struct StubState {
    nonce: String,
    requests: Vec<StubRequest>,
}

fn base64_url(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let value = chunk.iter().enumerate().fold(0u32, |acc, (index, byte)| {
            acc | (u32::from(*byte) << (16 - 8 * index))
        });
        for index in 0..=chunk.len() {
            out.push(ALPHABET[((value >> (18 - 6 * index)) & 0x3f) as usize] as char);
        }
    }
    out
}

fn id_token(client_id: &str, nonce: &str) -> String {
    let payload = json!({
        "iss": "https://auth.openai.com",
        "aud": [client_id],
        "exp": 4_000_000_000u64,
        "nonce": nonce,
        "sub": "user-sub-123",
        "email": "user@example.com",
    });
    format!(
        "{}.{}.",
        base64_url(br#"{"alg":"none"}"#),
        base64_url(payload.to_string().as_bytes())
    )
}

fn parse_form(body: &str) -> BTreeMap<String, String> {
    body.split('&')
        .filter_map(|pair| pair.split_once('='))
        .map(|(key, value)| {
            (
                percent_encoding::percent_decode_str(key)
                    .decode_utf8_lossy()
                    .into_owned(),
                percent_encoding::percent_decode_str(value)
                    .decode_utf8_lossy()
                    .into_owned(),
            )
        })
        .collect()
}

fn answer(stream: &mut TcpStream, status: u16, body: &str) {
    let response = format!(
        "HTTP/1.1 {status} Stub\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}

fn serve(mut stream: TcpStream, state: &Mutex<StubState>) {
    let mut reader = BufReader::new(stream.try_clone().expect("stream clones"));
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() {
        return;
    }
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let path = parts.next().unwrap_or_default().to_string();
    let mut content_length = 0usize;
    let mut bearer = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
            break;
        }
        if let Some((name, value)) = line.trim_end().split_once(':') {
            let value = value.trim();
            if name.eq_ignore_ascii_case("content-length") {
                content_length = value.parse().unwrap_or(0);
            } else if name.eq_ignore_ascii_case("authorization") {
                bearer = value.strip_prefix("Bearer ").map(ToString::to_string);
            }
        }
    }
    let mut body = vec![0u8; content_length];
    if reader.read_exact(&mut body).is_err() {
        return;
    }
    let form = parse_form(&String::from_utf8_lossy(&body));
    let nonce = {
        let mut state = state.lock().expect("stub state");
        state.requests.push(StubRequest {
            method: method.clone(),
            path: path.clone(),
            form: form.clone(),
            bearer,
        });
        state.nonce.clone()
    };
    match (method.as_str(), path.as_str()) {
        ("POST", "/api/accounts/oauth/token")
            if form.get("grant_type").map(String::as_str) == Some("authorization_code") =>
        {
            let client_id = form.get("client_id").cloned().unwrap_or_default();
            let body = json!({
                "token_type": "Bearer",
                "access_token": format!("tok-{client_id}"),
                "refresh_token": format!("rt-{client_id}"),
                "expires_in": 3600,
                "scope": "openid profile email offline_access chatgpt.tokens.use.direct",
                "id_token": id_token(&client_id, &nonce),
            });
            answer(&mut stream, 200, &body.to_string());
        }
        ("POST", "/api/accounts/oauth/revoke") => answer(&mut stream, 200, ""),
        ("GET", "/v1/models") => answer(
            &mut stream,
            200,
            r#"{"models":[
                {"slug":"gpt-5","display_name":"GPT-5","visibility":"list"},
                {"slug":"gpt-hidden","display_name":"Hidden","visibility":"hide"},
                {"slug":"gpt-5-mini","display_name":"GPT-5 mini","visibility":"list"}
            ]}"#,
        ),
        _ => answer(&mut stream, 404, r#"{"error":"not_found"}"#),
    }
}

fn start_stub(state: Arc<Mutex<StubState>>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("stub binds");
    let base = format!("http://{}", listener.local_addr().expect("stub address"));
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { break };
            serve(stream, &state);
        }
    });
    base
}

fn query_param(url: &str, name: &str) -> String {
    url.split_once('?')
        .map(|(_, query)| query)
        .unwrap_or_default()
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .find(|(key, _)| *key == name)
        .map(|(_, value)| value.to_string())
        .unwrap_or_else(|| panic!("{name} missing from the authorize URL"))
}

struct Responses {
    app: Router,
    seen: Vec<(String, Value)>,
}

impl Responses {
    async fn call(&mut self, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
        let mut request = Request::builder().method(method).uri(uri);
        let body = match body {
            Some(value) => {
                request = request.header("content-type", "application/json");
                Body::from(value.to_string())
            }
            None => Body::empty(),
        };
        let response = self
            .app
            .clone()
            .oneshot(request.body(body).expect("request builds"))
            .await
            .expect("router responds");
        let status = response.status();
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body reads");
        let value: Value = serde_json::from_slice(&bytes).expect("response is JSON");
        if let Some(reason) = value.get("reason") {
            assert!(
                CLOSED_REASONS.contains(&reason.as_str().expect("reason is a string")),
                "{uri} returned a reason outside the closed set"
            );
        }
        self.seen.push((uri.to_string(), value.clone()));
        (status, value)
    }

    async fn begin(&mut self) -> (String, String) {
        let (status, body) = self
            .call("POST", "/app/thinking/api/chatgpt/sign-in", Some(json!({})))
            .await;
        assert_eq!(status, StatusCode::OK);
        (
            body["attempt_id"].as_str().expect("attempt id").to_string(),
            body["authorize_url"]
                .as_str()
                .expect("authorize url")
                .to_string(),
        )
    }

    async fn attempt(&mut self, attempt_id: &str) -> Value {
        let (status, body) = self
            .call(
                "GET",
                &format!("/app/thinking/api/chatgpt/sign-in/{attempt_id}"),
                None,
            )
            .await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            ATTEMPT_STATES.contains(&body["state"].as_str().expect("state is a string")),
            "an attempt state outside the closed set"
        );
        assert_eq!(
            body.get("reason").is_none(),
            body["state"] == "pending",
            "only a pending attempt reads without a reason"
        );
        body
    }

    async fn finish_at(&mut self, uri: &str, redirect_url: &str) -> String {
        let (status, body) = self
            .call("POST", uri, Some(json!({ "redirect_url": redirect_url })))
            .await;
        assert_eq!(status, StatusCode::OK);
        body["reason"].as_str().expect("reason").to_string()
    }

    async fn finish(&mut self, redirect_url: &str) -> String {
        self.finish_at("/app/thinking/api/chatgpt/sign-in/finish", redirect_url)
            .await
    }

    async fn finish_named(&mut self, attempt_id: &str, redirect_url: &str) -> String {
        self.finish_at(
            &format!("/app/thinking/api/chatgpt/sign-in/{attempt_id}/finish"),
            redirect_url,
        )
        .await
    }

    async fn status(&mut self) -> Value {
        let (status, body) = self
            .call("GET", "/app/thinking/api/chatgpt/status", None)
            .await;
        assert_eq!(status, StatusCode::OK);
        body
    }

    async fn signed_in(&mut self) -> bool {
        let (status, body) = self
            .call("GET", "/app/thinking/api/chatgpt/status", None)
            .await;
        assert_eq!(status, StatusCode::OK);
        body["signed_in"].as_bool().expect("signed_in flag")
    }
}

fn temporary_journal() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    fs::create_dir_all(dir.path().join("config")).expect("config dir");
    fs::write(
        dir.path().join("config/journal.json"),
        br#"{"setup":{"completed_at":1767225600}}"#,
    )
    .expect("config writes");
    dir
}

fn action_log_lines(root: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(root.join("config/actions")) else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .flat_map(|entry| {
            fs::read_to_string(entry.path())
                .unwrap_or_default()
                .lines()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
        })
        .collect()
}

fn registered_client(root: &Path) -> Option<String> {
    chatgpt::registration(root)
        .expect("registration reads")
        .map(|registration| registration.client_id)
}

#[tokio::test]
async fn chatgpt_routes_sign_in_by_pasted_url_list_models_and_sign_out() {
    let _routes = CHATGPT_ROUTES.lock().await;
    let dir = temporary_journal();
    let root = dir.path().to_path_buf();
    let stub = Arc::new(Mutex::new(StubState::default()));
    let base = start_stub(stub.clone());
    chatgpt::set_test_auth_base_url_override(Some(base.clone()));
    chatgpt::set_test_api_base_url_override(Some(base));
    let mut api = Responses {
        app: crate::router(root.clone()),
        seen: Vec::new(),
    };

    assert_eq!(
        api.status().await,
        json!({
            "state": "signed_out",
            "signed_in": false,
            "plan_usage_declined": false,
            "client_refused": false,
        })
    );

    // A second begin cancels the first; an unknown attempt reads expired.
    let (first, _) = api.begin().await;
    assert_eq!(api.attempt(&first).await, json!({"state": "pending"}));
    let (second, second_url) = api.begin().await;
    assert_ne!(first, second);
    assert_eq!(
        api.attempt(&first).await,
        json!({"state": "failed", "reason": "cancelled"})
    );
    assert_eq!(
        api.attempt("unknown-attempt-id").await,
        json!({"state": "expired", "reason": "expired"})
    );
    assert_eq!(api.attempt(&second).await, json!({"state": "pending"}));

    // The right state without a code is refused.
    let second_state = query_param(&second_url, "state");
    let second_attempt = chatgpt::get_active_attempt().expect("pending attempt");
    assert_eq!(second_attempt.attempt_id, second);
    assert_eq!(
        api.finish(&format!(
            "{}?state={second_state}&client_id=oaiapp_test",
            second_attempt.redirect_uri
        ))
        .await,
        "callback_invalid"
    );
    assert_eq!(
        api.attempt(&second).await,
        json!({"state": "failed", "reason": "callback_invalid"})
    );

    // A fresh attempt finishes from the pasted redirect URL, by its id.
    let (third, third_url) = api.begin().await;
    let third_attempt = chatgpt::get_active_attempt().expect("pending attempt");
    assert_eq!(third_attempt.attempt_id, third);
    let third_state = query_param(&third_url, "state");
    stub.lock().expect("stub state").nonce = third_attempt.nonce.clone();
    let code = "ac_test_code";
    let third_redirect = format!(
        "{}?code={code}&scope=openid&state={third_state}&client_id=oaiapp_test",
        third_attempt.redirect_uri
    );
    assert_eq!(api.finish_named(&first, &third_redirect).await, "cancelled");
    assert_eq!(
        api.finish_named("unknown-attempt-id", &third_redirect)
            .await,
        "expired"
    );
    assert_eq!(api.attempt(&third).await, json!({"state": "pending"}));
    assert_eq!(api.finish_named(&third, &third_redirect).await, "signed_in");
    assert_eq!(
        api.attempt(&third).await,
        json!({"state": "signed_in", "reason": "signed_in"})
    );
    let status = api.status().await;
    assert_eq!(status["state"], "signed_in");
    assert_eq!(status["signed_in"], true);
    assert_eq!(status["email"], "user@example.com");
    assert!(status["expires_at"].as_u64().is_some());
    assert_eq!(status["plan_usage_declined"], false);
    assert_eq!(status["client_refused"], false);
    assert_eq!(
        status.as_object().map(|fields| fields.len()),
        Some(6),
        "{status}"
    );
    assert_eq!(registered_client(&root).as_deref(), Some("oaiapp_test"));
    let exchange = stub
        .lock()
        .expect("stub state")
        .requests
        .iter()
        .find(|request| {
            request.form.get("grant_type").map(String::as_str) == Some("authorization_code")
        })
        .cloned()
        .expect("code exchanged");
    assert_eq!(exchange.form["code"], code);
    assert_eq!(exchange.form["code_verifier"], third_attempt.verifier);
    assert_eq!(exchange.form["client_id"], "oaiapp_test");
    assert_eq!(exchange.form["resource"], "https://api.openai.com/v1");
    assert_eq!(exchange.path, "/api/accounts/oauth/token");

    // With tokens stored and another attempt pending, every surface stays token-free.
    let (fourth, fourth_url) = api.begin().await;
    let fourth_attempt = chatgpt::get_active_attempt().expect("pending attempt");
    let fourth_state = query_param(&fourth_url, "state");
    assert_eq!(api.attempt(&fourth).await, json!({"state": "pending"}));
    assert_eq!(
        api.attempt(&third).await,
        json!({"state": "signed_in", "reason": "signed_in"})
    );
    assert!(api.signed_in().await);

    let (status, models) = api
        .call("GET", "/app/thinking/api/chatgpt/models", None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        models,
        json!({"models": [
            {"slug": "gpt-5", "display_name": "GPT-5"},
            {"slug": "gpt-5-mini", "display_name": "GPT-5 mini"},
        ]})
    );
    let models_request = stub
        .lock()
        .expect("stub state")
        .requests
        .iter()
        .find(|request| request.path == "/v1/models")
        .cloned()
        .expect("models requested");
    assert_eq!(models_request.method, "GET");
    assert_eq!(models_request.bearer.as_deref(), Some("tok-oaiapp_test"));

    // Sign-out revokes and keeps the registration; forget then clears it.
    let (status, body) = api
        .call(
            "POST",
            "/app/thinking/api/chatgpt/sign-out",
            Some(json!({})),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!({"revoked": true}));
    let revokes: Vec<_> = stub
        .lock()
        .expect("stub state")
        .requests
        .iter()
        .filter(|request| request.path == "/api/accounts/oauth/revoke")
        .cloned()
        .collect();
    assert_eq!(revokes.len(), 1);
    assert_eq!(revokes[0].form["token"], "rt-oaiapp_test");
    assert_eq!(revokes[0].form["client_id"], "oaiapp_test");
    assert!(!api.signed_in().await);
    assert_eq!(registered_client(&root).as_deref(), Some("oaiapp_test"));

    let (status, body) = api
        .call(
            "POST",
            "/app/thinking/api/chatgpt/sign-out",
            Some(json!({"forget": true})),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!({"revoked": true}));
    assert_eq!(registered_client(&root), None);
    assert!(!api.signed_in().await);
    assert_eq!(
        api.status().await,
        json!({
            "state": "signed_out",
            "signed_in": false,
            "plan_usage_declined": false,
            "client_refused": false,
        })
    );

    let (status, body) = api
        .call("GET", "/app/thinking/api/chatgpt/models", None)
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["reason_code"], "sign_in_required");

    chatgpt::set_test_auth_base_url_override(None);
    chatgpt::set_test_api_base_url_override(None);

    let secrets = [
        "tok-oaiapp_test".to_string(),
        "rt-oaiapp_test".to_string(),
        code.to_string(),
        third_attempt.verifier.clone(),
        fourth_attempt.verifier.clone(),
    ];
    let states = [second_state, third_state, fourth_state];
    for (uri, body) in &api.seen {
        let mut body = body.clone();
        if let Some(fields) = body.as_object_mut() {
            // The authorize URL carries the state by construction.
            fields.remove("authorize_url");
        }
        let text = body.to_string();
        for value in secrets.iter().chain(&states) {
            assert!(
                !text.contains(value.as_str()),
                "{uri} response carries a secret value"
            );
        }
    }

    let lines = action_log_lines(&root);
    for action in [
        "chatgpt_sign_in_begin",
        "chatgpt_sign_in_finish",
        "chatgpt_sign_out",
    ] {
        assert!(
            lines.iter().any(|line| line.contains(action)),
            "{action} is not in the action log"
        );
    }
    for line in &lines {
        for value in secrets.iter().chain(&states) {
            assert!(
                !line.contains(value.as_str()),
                "an action-log line carries a secret value"
            );
        }
        assert!(!line.contains("user@example.com"));
        assert!(!line.contains("redirect"));
        assert!(!line.contains("127.0.0.1"));
    }
}
