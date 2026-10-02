// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Full-tests integration tests for ChatGPT authentication (concurrent refresh & loopback sockets).

use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;

use crate::attempt::{begin_sign_in, finish_sign_in, get_status, sign_out};
use crate::credential::{ChatGptCredential, ClosedOutcome};
use crate::overrides::AUTH_BASE_URL_OVERRIDE_ENV;
use crate::refresh::ChatGptAuthManager;
use crate::store::{
    ChatGptSignInDoc, DIRECT_USE_SCOPE, LoadResult, Registration, Tokens, load_credential_file,
    save_credential_file,
};
use crate::test_support::REFRESH_MARKERS_ENV;
use crate::transport::{ChatGptTransport, HttpResponse, TransportError};

pub const WORKER_ENV: &str = "SOLSTONE_CHATGPT_AUTH_WORKER";

#[test]
fn eight_concurrent_callers_worker() {
    let Ok(args) = std::env::var(WORKER_ENV) else {
        return; // Return immediately when unset
    };

    let mut parts = args.split_whitespace();
    let journal_str = parts.next().expect("journal arg");
    let num_threads: usize = parts
        .next()
        .expect("threads arg")
        .parse()
        .expect("valid threads number");

    let journal = PathBuf::from(journal_str);

    let mut handles = Vec::new();
    for _ in 0..num_threads {
        let j = journal.clone();
        handles.push(thread::spawn(move || {
            let manager = ChatGptAuthManager::with_default_transport(j);
            manager.access_token()
        }));
    }

    for handle in handles {
        let res = handle.join().expect("thread join");
        assert_eq!(res.expect("token success"), "concurrent_refreshed_access");
    }

    std::process::exit(0);
}

fn write_initial_expired_doc(journal: &Path) {
    let mut doc = ChatGptSignInDoc::new_initial("concurrent-test-host".to_string());
    doc.registration = Some(Registration {
        client_id: "client-concurrent".to_string(),
        client_refused: false,
    });
    doc.tokens = Some(Tokens {
        access_token: "old_access_expired".to_string(),
        refresh_token: "old_refresh".to_string(),
        expires_at: 100, // expired
        scopes: vec![DIRECT_USE_SCOPE.to_string()],
    });
    save_credential_file(journal, &doc).expect("save doc");
}

#[test]
#[allow(unsafe_code)]
fn eight_concurrent_callers_single_grant() {
    let dir = tempfile::tempdir().unwrap();
    let journal = dir.path().to_path_buf();
    fs::create_dir_all(journal.join("config")).unwrap();
    write_initial_expired_doc(&journal);

    let marker_temp = tempfile::tempdir().unwrap();
    let marker_dir = marker_temp.path().to_path_buf();
    unsafe {
        std::env::set_var(REFRESH_MARKERS_ENV, &marker_dir);
    }

    // Start a mock OAuth token HTTP server
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let mock_auth_url = format!("http://127.0.0.1:{port}");
    crate::overrides::set_test_auth_base_url_override(Some(mock_auth_url.clone()));

    let grant_count = Arc::new(AtomicUsize::new(0));
    let grant_count_clone = grant_count.clone();
    let marker_dir_clone = marker_dir.clone();

    let server_handle = thread::spawn(move || {
        let _ = listener.set_nonblocking(false);
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let mut buf = [0u8; 4096];
            let n = stream.read(&mut buf).unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]);

            if req.contains("POST /oauth/token") {
                grant_count_clone.fetch_add(1, Ordering::SeqCst);

                // Wait until all 8 callers have recorded their refresh marker
                let start = std::time::Instant::now();
                loop {
                    let count = fs::read_dir(&marker_dir_clone)
                        .map(|entries| entries.filter_map(|e| e.ok()).count())
                        .unwrap_or(0);
                    if count >= 8 {
                        break;
                    }
                    if start.elapsed() > Duration::from_secs(30) {
                        panic!("timed out waiting for 8 refresh markers; got {count}");
                    }
                    thread::sleep(Duration::from_millis(20));
                }

                let response_body = r#"{
                    "access_token": "concurrent_refreshed_access",
                    "refresh_token": "concurrent_new_refresh",
                    "expires_in": 3600,
                    "scope": "openid profile email chatgpt.tokens.use.direct"
                }"#;
                let response = format!(
                    "HTTP/1.1 200 OK\r\n\
                     Content-Type: application/json\r\n\
                     Content-Length: {}\r\n\
                     Connection: close\r\n\
                     \r\n\
                     {response_body}",
                    response_body.len()
                );
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.flush();
            } else {
                let _ = stream.write_all(b"HTTP/1.1 404 Not Found\r\n\r\n");
            }
        }
    });

    let current_exe = std::env::current_exe().expect("current exe");

    // Launch two child worker processes (3 threads each = 6 callers)
    let mut child1 = Command::new(&current_exe)
        .arg("full_tests::eight_concurrent_callers_worker")
        .arg("--exact")
        .env(WORKER_ENV, format!("{} 3", journal.display()))
        .env(REFRESH_MARKERS_ENV, &marker_dir)
        .env(AUTH_BASE_URL_OVERRIDE_ENV, &mock_auth_url)
        .spawn()
        .expect("spawn child 1");

    let mut child2 = Command::new(&current_exe)
        .arg("full_tests::eight_concurrent_callers_worker")
        .arg("--exact")
        .env(WORKER_ENV, format!("{} 3", journal.display()))
        .env(REFRESH_MARKERS_ENV, &marker_dir)
        .env(AUTH_BASE_URL_OVERRIDE_ENV, &mock_auth_url)
        .spawn()
        .expect("spawn child 2");

    // Plus 2 callers in the parent process
    let mut parent_handles = Vec::new();
    for _ in 0..2 {
        let j = journal.clone();
        let auth_url = mock_auth_url.clone();
        let m_dir = marker_dir.clone();
        parent_handles.push(thread::spawn(move || {
            let _ = auth_url;
            let _ = m_dir;
            let manager = ChatGptAuthManager::with_default_transport(j);
            manager.access_token()
        }));
    }

    let status1 = child1.wait().expect("child 1 wait");
    assert!(status1.success(), "child 1 failed");

    let status2 = child2.wait().expect("child 2 wait");
    assert!(status2.success(), "child 2 failed");

    for h in parent_handles {
        let res = h.join().expect("parent thread join");
        assert_eq!(res.expect("parent token"), "concurrent_refreshed_access");
    }

    drop(server_handle);

    // Exactly 1 grant was issued across all 8 callers!
    assert_eq!(grant_count.load(Ordering::SeqCst), 1);

    let doc = match load_credential_file(&journal) {
        LoadResult::Present(d) => d,
        _ => panic!("doc present"),
    };
    assert_eq!(
        doc.tokens.as_ref().unwrap().access_token,
        "concurrent_refreshed_access"
    );
    assert_eq!(
        doc.tokens.as_ref().unwrap().refresh_token,
        "concurrent_new_refresh"
    );

    crate::overrides::set_test_auth_base_url_override(None);
    unsafe {
        std::env::remove_var(REFRESH_MARKERS_ENV);
    }
}

struct ExchangeTransport {
    body_map: std::sync::Mutex<BTreeMap<String, (u16, String)>>,
}

impl ExchangeTransport {
    fn new() -> Self {
        Self {
            body_map: std::sync::Mutex::new(BTreeMap::new()),
        }
    }

    fn set_response(&self, url_sub: &str, status: u16, body: &str) {
        self.body_map
            .lock()
            .unwrap()
            .insert(url_sub.to_string(), (status, body.to_string()));
    }
}

impl ChatGptTransport for ExchangeTransport {
    fn post_form(
        &self,
        url: &str,
        _form: &BTreeMap<String, String>,
        _timeout: Duration,
    ) -> Result<HttpResponse, TransportError> {
        let map = self.body_map.lock().unwrap();
        for (pattern, (status, body)) in map.iter() {
            if url.contains(pattern) {
                return Ok(HttpResponse {
                    status: *status,
                    body: body.clone(),
                    headers: BTreeMap::new(),
                });
            }
        }
        Ok(HttpResponse {
            status: 200,
            body: "{}".to_string(),
            headers: BTreeMap::new(),
        })
    }

    fn get_json(
        &self,
        url: &str,
        _bearer_token: &str,
        _timeout: Duration,
    ) -> Result<HttpResponse, TransportError> {
        let map = self.body_map.lock().unwrap();
        for (pattern, (status, body)) in map.iter() {
            if url.contains(pattern) {
                return Ok(HttpResponse {
                    status: *status,
                    body: body.clone(),
                    headers: BTreeMap::new(),
                });
            }
        }
        Ok(HttpResponse {
            status: 200,
            body: "{}".to_string(),
            headers: BTreeMap::new(),
        })
    }
}

#[test]
fn loopback_browser_sign_in_flow() {
    let dir = tempfile::tempdir().unwrap();
    let journal = dir.path().to_path_buf();
    fs::create_dir_all(journal.join("config")).unwrap();

    let attempt = begin_sign_in(&journal).expect("begin sign in");
    assert!(attempt.listener.is_some());

    let redirect_url = attempt.redirect_uri.clone();
    let state = attempt.state.clone();
    let nonce = attempt.nonce.clone();

    let transport = Arc::new(ExchangeTransport::new());

    // Payload: {"iss":"https://auth.openai.com","aud":"dynamic_issued_client","exp":2000000000,"nonce":"<nonce>","sub":"sub-browser","email":"user@example.com"}
    let payload_json = serde_json::json!({
        "iss": "https://auth.openai.com",
        "aud": "dynamic_issued_client",
        "exp": 2000000000u64,
        "nonce": nonce,
        "sub": "sub-browser",
        "email": "user@example.com",
    });
    let payload_bytes = serde_json::to_vec(&payload_json).unwrap();
    let mut b64 = String::new();
    for chunk in payload_bytes.chunks(3) {
        let mut buf = 0u32;
        for &b in chunk {
            buf = (buf << 8) | (b as u32);
        }
        let pad = 3 - chunk.len();
        buf <<= pad * 8;
        let alphabet = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
        b64.push(alphabet[((buf >> 18) & 0x3F) as usize] as char);
        b64.push(alphabet[((buf >> 12) & 0x3F) as usize] as char);
        if pad < 2 {
            b64.push(alphabet[((buf >> 6) & 0x3F) as usize] as char);
        }
        if pad < 1 {
            b64.push(alphabet[(buf & 0x3F) as usize] as char);
        }
    }
    let jwt = format!("eyJhbGciOiJub25lIn0.{b64}.");

    let token_resp = format!(
        r#"{{
        "token_type": "Bearer",
        "access_token": "browser_access_tok",
        "refresh_token": "browser_refresh_tok",
        "expires_in": 3600,
        "scope": "openid profile email chatgpt.tokens.use.direct",
        "id_token": "{jwt}"
    }}"#
    );
    transport.set_response("oauth/token", 200, &token_resp);

    // Simulate browser callback hitting loopback listener:
    // 1. Wrong host -> 404, does not claim attempt
    // 2. Wrong state -> 400, does not claim attempt
    // 3. Valid callback -> 200, claims attempt
    let callback_handle = thread::spawn(move || {
        thread::sleep(Duration::from_millis(50));
        let ureq_agent = ureq::Agent::new_with_config(
            ureq::Agent::config_builder()
                .http_status_as_error(false)
                .build(),
        );

        let port_str = redirect_url
            .split(':')
            .next_back()
            .unwrap()
            .split('/')
            .next()
            .unwrap();
        let wrong_host_url = format!(
            "http://127.0.0.1:{port_str}/auth/callback?code=abc&state={state}&client_id=dynamic_issued_client"
        );
        let resp_wrong_host = ureq_agent
            .get(&wrong_host_url)
            .header("Host", &format!("localhost:{port_str}"))
            .call()
            .unwrap();
        assert_eq!(resp_wrong_host.status(), 404);

        let wrong_state_url =
            format!("{redirect_url}?code=abc&state=wrong_state&client_id=dynamic_issued_client");
        let resp_wrong_state = ureq_agent.get(&wrong_state_url).call().unwrap();
        assert_eq!(resp_wrong_state.status(), 400);

        let valid_url = format!(
            "{redirect_url}?code=browser_code_123&state={state}&client_id=dynamic_issued_client"
        );
        let resp_valid = ureq_agent.get(&valid_url).call().unwrap();
        assert_eq!(resp_valid.status(), 200);
    });

    let res = finish_sign_in(
        &journal,
        transport.as_ref(),
        &attempt,
        None,
        Duration::from_secs(5),
    )
    .expect("finish sign in");

    callback_handle.join().unwrap();
    assert_eq!(res.outcome, ClosedOutcome::SignedIn);

    let status = get_status(&journal).expect("status");
    assert!(status.signed_in);

    // Test sign-out
    let sign_out_res = sign_out(&journal, transport.as_ref(), false).expect("sign out");
    assert!(sign_out_res.revoked);

    let status_after = get_status(&journal).expect("status after");
    assert!(!status_after.signed_in);
}

#[test]
fn access_token_busy_on_lock_contention() {
    use crate::credential::CredentialError;
    use crate::store::{CREDENTIAL_FILE, with_test_access_token_lock_timeout};
    use solstone_core_journal_io::hold_lock;

    let dir = tempfile::tempdir().unwrap();
    let journal = dir.path().to_path_buf();
    fs::create_dir_all(journal.join("config")).unwrap();
    write_initial_expired_doc(&journal);

    // Hold credential lock externally
    let lock_path = journal.join(CREDENTIAL_FILE);
    let _lock = hold_lock(
        &lock_path,
        solstone_core_journal_io::LockOptions {
            timeout: Duration::from_secs(5),
            ..Default::default()
        },
    )
    .expect("hold lock");

    // Shorten the access_token lock wait to 50ms for the test
    let err = with_test_access_token_lock_timeout(Duration::from_millis(50), || {
        let manager = ChatGptAuthManager::with_default_transport(journal);
        manager.access_token().expect_err("should fail busy")
    });

    match err {
        CredentialError::Busy => {}
        other => panic!("expected CredentialError::Busy, got: {other:?}"),
    }
}
