// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Socket, filesystem-permission, and multi-thread concurrency tests for `solstone-core-oauth`.
//! Run under `--features full-tests` (registry package suite `package::solstone-core-oauth`).

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::Duration;

use crate::callback::{
    AttemptGate, CallbackError, CallbackOutcome, CallbackPolicy, ClaimResult, ClaimSource,
    LoopbackBindSpec, LoopbackHost, LoopbackListener, SUCCESS_BODY, parse_callback_url,
    receive_loopback_callback, respond_to_callback_stream,
};
use crate::registration::RegistrationMode;
use crate::ssrf::{HttpRequest, HttpResponse, OAuthHttpClient, SsrfFetchError};
use crate::store::{
    ClientRegistration, OAuthCredentialDoc, OAuthCredentialManager, OAuthTokens, ServiceId,
    credential_path, save_credential_file,
};

fn raw_http(port: u16, req: &str) -> String {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect loopback");
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("read timeout");
    stream.write_all(req.as_bytes()).expect("write request");
    stream.flush().expect("flush");
    let mut resp = String::new();
    stream.read_to_string(&mut resp).expect("read response");
    resp
}

#[test]
fn loopback_listener_enforces_host_path_method_duplicates_state_csrf_dos_and_responds_200() {
    let bound = LoopbackListener::bind(&LoopbackBindSpec::ephemeral(
        LoopbackHost::Ipv4Loopback,
        "/auth/callback",
    ))
    .expect("bind ephemeral loopback");
    let port = bound.port();
    assert_eq!(
        bound.redirect_uri(),
        format!("http://127.0.0.1:{port}/auth/callback")
    );

    let policy = CallbackPolicy::new("secret-state-99", bound.redirect_uri())
        .with_expected_issuer("https://auth.example.com", true);
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_bg = Arc::clone(&cancel);
    let policy_bg = policy.clone();

    let server = thread::spawn(move || {
        let mut delivery =
            receive_loopback_callback(&bound, &policy_bg, Duration::from_secs(10), &cancel_bg)
                .expect("valid callback eventually arrives");
        respond_to_callback_stream(Some(&mut delivery.stream), true);
        delivery.outcome
    });

    // 1. Wrong Host (`localhost` when bound for `127.0.0.1`) -> 404, does NOT consume attempt.
    let resp_wrong_host = raw_http(
        port,
        &format!(
            "GET /auth/callback?code=c1&state=secret-state-99 HTTP/1.1\r\nHost: localhost:{port}\r\nConnection: close\r\n\r\n"
        ),
    );
    assert!(
        resp_wrong_host.starts_with("HTTP/1.1 404"),
        "{resp_wrong_host}"
    );

    // 2. Wrong path -> 404, does NOT consume attempt.
    let resp_wrong_path = raw_http(
        port,
        &format!(
            "GET /wrong?code=c1&state=secret-state-99 HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"
        ),
    );
    assert!(
        resp_wrong_path.starts_with("HTTP/1.1 404"),
        "{resp_wrong_path}"
    );

    // 3. Non-GET method -> 400, does NOT consume attempt.
    let resp_post = raw_http(
        port,
        &format!(
            "POST /auth/callback?code=c1&state=secret-state-99 HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        ),
    );
    assert!(resp_post.starts_with("HTTP/1.1 400"), "{resp_post}");

    // 4. Duplicate query param -> 400, does NOT consume attempt.
    let resp_dup = raw_http(
        port,
        &format!(
            "GET /auth/callback?code=c1&state=secret-state-99&state=other HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"
        ),
    );
    assert!(resp_dup.starts_with("HTTP/1.1 400"), "{resp_dup}");

    // 5. Wrong state (CSRF-DoS attempt) -> 400, does NOT consume attempt!
    let resp_wrong_state = raw_http(
        port,
        &format!(
            "GET /auth/callback?code=forged&state=wrong-state HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"
        ),
    );
    assert!(
        resp_wrong_state.starts_with("HTTP/1.1 400"),
        "{resp_wrong_state}"
    );
    assert!(resp_wrong_state.contains("Cache-Control: no-store"));

    // 6. Legitimate callback still succeeds after all five hostile probes!
    let resp_ok = raw_http(
        port,
        &format!(
            "GET /auth/callback?code=real-code&state=secret-state-99&iss=https%3A%2F%2Fauth.example.com HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"
        ),
    );
    assert!(resp_ok.starts_with("HTTP/1.1 200 OK"), "{resp_ok}");
    assert!(resp_ok.contains("Cache-Control: no-store"));
    assert!(resp_ok.ends_with(SUCCESS_BODY));

    match server.join().unwrap() {
        CallbackOutcome::Accepted(cb) => {
            assert_eq!(cb.code, "real-code");
            assert_eq!(cb.issuer.as_deref(), Some("https://auth.example.com"));
        }
        other => panic!("expected Accepted, got {other:?}"),
    }
}

#[test]
fn loopback_listener_supports_localhost_host_binding_and_paste_race_cancellation() {
    let bound = LoopbackListener::bind(&LoopbackBindSpec::ephemeral(
        LoopbackHost::Localhost,
        "/callback",
    ))
    .expect("bind localhost loopback");
    let port = bound.port();
    assert_eq!(
        bound.redirect_uri(),
        format!("http://localhost:{port}/callback")
    );

    let gate = AttemptGate::new(1000, 600);
    let policy = CallbackPolicy::new("race-state", bound.redirect_uri());
    let cancel = gate.cancel_flag();
    let policy_bg = policy.clone();

    let listener_thread = thread::spawn(move || {
        receive_loopback_callback(&bound, &policy_bg, Duration::from_secs(10), &cancel)
    });

    // Broker/paste callback wins the race and cancels the loopback listener immediately.
    let pasted = format!("http://localhost:{port}/callback?code=pasted-code&state=race-state");
    assert!(matches!(
        parse_callback_url(&pasted, &policy),
        CallbackOutcome::Accepted(cb) if cb.code == "pasted-code"
    ));
    assert_eq!(
        gate.try_claim(ClaimSource::BrokerOrPaste, 1005),
        ClaimResult::Claimed
    );
    assert_eq!(
        gate.try_claim(ClaimSource::Loopback, 1006),
        ClaimResult::AlreadyClaimed {
            winner: ClaimSource::BrokerOrPaste
        }
    );

    // Loopback listener wakes and terminates with Cancelled.
    assert!(matches!(
        listener_thread.join().unwrap(),
        Err(CallbackError::Cancelled)
    ));
}

#[test]
fn flock_serializes_concurrent_rotating_refreshes_to_one_http_call() {
    let dir = tempfile::tempdir().unwrap();
    let journal = dir.path().to_path_buf();
    let service = ServiceId::new("plaud").unwrap();

    let mut doc = OAuthCredentialDoc::new(&service, "https://mcp.plaud.ai/token");
    doc.registration = Some(ClientRegistration {
        client_id: "plaud-c1".to_owned(),
        client_secret: None,
        mode: RegistrationMode::Dcr,
        client_refused: false,
    });
    doc.tokens = Some(OAuthTokens {
        access_token: "stale-access".to_owned(),
        refresh_token: Some("single-use-rt-1".to_owned()),
        token_type: "Bearer".to_owned(),
        expires_at: Some(1),
        scopes: vec!["read".to_owned()],
    });
    save_credential_file(&journal, &service, &doc).unwrap();

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(credential_path(&journal, &service))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "credential file must be mode 0o600");
    }

    struct CountingHttp {
        calls: AtomicUsize,
    }
    impl OAuthHttpClient for CountingHttp {
        fn send(&self, _request: &HttpRequest) -> Result<HttpResponse, SsrfFetchError> {
            let n = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
            // Hold briefly inside the refresh network round-trip so sibling threads collide on flock.
            thread::sleep(Duration::from_millis(80));
            let body = format!(
                r#"{{"access_token":"fresh-at-{n}","refresh_token":"rotated-rt-{n}","token_type":"Bearer","expires_in":7200}}"#
            );
            Ok(HttpResponse {
                status: 200,
                headers: Vec::new(),
                body: body.into_bytes(),
            })
        }
    }

    let http = Arc::new(CountingHttp {
        calls: AtomicUsize::new(0),
    });
    let barrier = Arc::new(Barrier::new(4));
    let mut handles = Vec::new();
    for _ in 0..4 {
        let j = journal.clone();
        let s = service.clone();
        let h = Arc::clone(&http);
        let b = Arc::clone(&barrier);
        handles.push(thread::spawn(move || {
            let mgr = OAuthCredentialManager::new(j, s, h);
            b.wait();
            mgr.access_token().expect("thread gets refreshed token")
        }));
    }

    let tokens: Vec<String> = handles
        .into_iter()
        .map(|h| h.join().unwrap().access_token)
        .collect();
    assert_eq!(
        http.calls.load(Ordering::SeqCst),
        1,
        "flock must serialize concurrent refreshes into exactly one token endpoint call"
    );
    assert!(
        tokens.iter().all(|t| t == "fresh-at-1"),
        "all 4 callers must observe the one refreshed token: {tokens:?}"
    );
}
