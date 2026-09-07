// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::Value;
use serde_json::json;
use solstone_core_spl::enroll_home;
use solstone_core_spl::relay_access::*;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

struct TempJournal(PathBuf);
impl TempJournal {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "solstone-spl-relay-access-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}
impl Drop for TempJournal {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[allow(clippy::too_many_arguments)]
fn create_test_jwt(
    iss: &str,
    sub: &str,
    aud: &str,
    scope: &str,
    ver: i64,
    instance_id: &str,
    iat: i64,
    exp: i64,
    jti: &str,
) -> String {
    let header = "eyJhbGciOiJub25lIiwidHlwIjoiSldUIn0";
    let payload = json!({
        "iss": iss,
        "sub": sub,
        "aud": aud,
        "scope": scope,
        "ver": ver,
        "instance_id": instance_id,
        "iat": iat,
        "exp": exp,
        "jti": jti,
    });
    let payload_str = serde_json::to_string(&payload).unwrap();
    let payload_b64 = base64_url_encode(payload_str.as_bytes());
    format!("{header}.{payload_b64}.signature")
}

fn base64_url_encode(bytes: &[u8]) -> String {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let mut b = (chunk[0] as u32) << 16;
        if chunk.len() > 1 {
            b |= (chunk[1] as u32) << 8;
        }
        if chunk.len() > 2 {
            b |= chunk[2] as u32;
        }
        out.push(CHARS[((b >> 18) & 0x3F) as usize] as char);
        out.push(CHARS[((b >> 12) & 0x3F) as usize] as char);
        if chunk.len() > 1 {
            out.push(CHARS[((b >> 6) & 0x3F) as usize] as char);
        }
        if chunk.len() > 2 {
            out.push(CHARS[(b & 0x3F) as usize] as char);
        }
    }
    out
}

#[test]
fn test_jwt_validation_strict() {
    let now = 1000;
    let exp = 2000;
    let exp_rfc3339 = "1970-01-01T00:33:20Z";
    let inst = "inst-abc";
    let iss = "my-issuer";

    let valid_token = create_test_jwt(
        iss,
        &format!("instance:{inst}"),
        "spl-relay",
        "session.dial",
        2,
        inst,
        500,
        exp,
        "jti-1",
    );

    let validated_exp = validate_device_token(&valid_token, inst, iss, exp_rfc3339, now).unwrap();
    assert_eq!(validated_exp, exp);

    // Mismatched issuer
    assert!(validate_device_token(&valid_token, inst, "other-iss", exp_rfc3339, now).is_err());

    // Mismatched instance_id
    assert!(validate_device_token(&valid_token, "other-inst", iss, exp_rfc3339, now).is_err());

    // Wrong sub format
    let bad_sub = create_test_jwt(
        iss,
        "user:123",
        "spl-relay",
        "session.dial",
        2,
        inst,
        500,
        exp,
        "jti-1",
    );
    assert!(validate_device_token(&bad_sub, inst, iss, exp_rfc3339, now).is_err());

    // Wrong aud
    let bad_aud = create_test_jwt(
        iss,
        &format!("instance:{inst}"),
        "wrong-aud",
        "session.dial",
        2,
        inst,
        500,
        exp,
        "jti-1",
    );
    assert!(validate_device_token(&bad_aud, inst, iss, exp_rfc3339, now).is_err());

    // Wrong scope
    let bad_scope = create_test_jwt(
        iss,
        &format!("instance:{inst}"),
        "spl-relay",
        "session.admin",
        2,
        inst,
        500,
        exp,
        "jti-1",
    );
    assert!(validate_device_token(&bad_scope, inst, iss, exp_rfc3339, now).is_err());

    // Empty jti
    let empty_jti = create_test_jwt(
        iss,
        &format!("instance:{inst}"),
        "spl-relay",
        "session.dial",
        2,
        inst,
        500,
        exp,
        "",
    );
    assert!(validate_device_token(&empty_jti, inst, iss, exp_rfc3339, now).is_err());

    // Expired (exp <= now)
    assert!(validate_device_token(&valid_token, inst, iss, exp_rfc3339, 3000).is_err());

    // exp <= iat
    let exp_le_iat = create_test_jwt(
        iss,
        &format!("instance:{inst}"),
        "spl-relay",
        "session.dial",
        2,
        inst,
        2500,
        2000,
        "jti-1",
    );
    assert!(validate_device_token(&exp_le_iat, inst, iss, exp_rfc3339, now).is_err());

    // Wrong ver
    let bad_ver_token = create_test_jwt(
        iss,
        &format!("instance:{inst}"),
        "spl-relay",
        "session.dial",
        1,
        inst,
        500,
        exp,
        "jti-1",
    );
    assert!(validate_device_token(&bad_ver_token, inst, iss, exp_rfc3339, now).is_err());

    // Extra key rejected
    let header = "eyJhbGciOiJub25lIiwidHlwIjoiSldUIn0";
    let payload_extra = json!({
        "iss": iss,
        "sub": format!("instance:{inst}"),
        "aud": "spl-relay",
        "scope": "session.dial",
        "ver": 2,
        "instance_id": inst,
        "iat": 500,
        "exp": exp,
        "jti": "jti-1",
        "extra": "bad",
    });
    let payload_b64 = base64_url_encode(serde_json::to_string(&payload_extra).unwrap().as_bytes());
    let bad_key_token = format!("{header}.{payload_b64}.sig");
    assert!(validate_device_token(&bad_key_token, inst, iss, exp_rfc3339, now).is_err());

    // Non-integer iat / exp
    let payload_str_iat = json!({
        "iss": iss,
        "sub": format!("instance:{inst}"),
        "aud": "spl-relay",
        "scope": "session.dial",
        "ver": 2,
        "instance_id": inst,
        "iat": "500",
        "exp": exp,
        "jti": "jti-1",
    });
    let bad_iat_token = format!(
        "{header}.{}.sig",
        base64_url_encode(serde_json::to_string(&payload_str_iat).unwrap().as_bytes())
    );
    assert!(validate_device_token(&bad_iat_token, inst, iss, exp_rfc3339, now).is_err());

    // RFC3339 mismatch
    assert!(validate_device_token(&valid_token, inst, iss, "1970-01-01T00:33:21Z", now).is_err());
}

#[test]
fn test_fetch_relay_access_refuses_oversized_request_body() {
    let oversized_token = "a".repeat(17000);
    let err = fetch_relay_access(
        "http://127.0.0.1:1",
        &oversized_token,
        "inst-1",
        DEFAULT_RELAY_ISSUER,
        0,
        Duration::from_millis(100),
    )
    .unwrap_err();
    assert!(matches!(err, RelayAccessError::Unavailable(_)));
}

#[test]
fn test_redirect_location_never_followed() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();

    std::thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let response = "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:9999/leak\r\nContent-Length: 0\r\n\r\n";
            let _ = stream.write_all(response.as_bytes());
        }
    });

    let err = fetch_relay_access(
        &format!("http://127.0.0.1:{port}"),
        "tok-1",
        "inst-1",
        DEFAULT_RELAY_ISSUER,
        0,
        Duration::from_millis(500),
    )
    .unwrap_err();
    assert!(matches!(err, RelayAccessError::Unavailable(_)));
}

fn setup_test_journal(
    journal_root: &std::path::Path,
    instance_id: &str,
    service_token: &str,
    relay_origin: &str,
) {
    std::fs::create_dir_all(journal_root.join("config")).unwrap();
    std::fs::create_dir_all(journal_root.join("link/tokens")).unwrap();

    std::fs::write(
        journal_root.join("config/journal.json"),
        json!({
            "link": {
                "posture": "spl",
                "relay_url": relay_origin,
            }
        })
        .to_string(),
    )
    .unwrap();

    std::fs::write(
        journal_root.join("link/tokens/account.json"),
        json!({
            "service_token": service_token,
        })
        .to_string(),
    )
    .unwrap();

    std::fs::write(
        journal_root.join("link/state.json"),
        json!({
            "instance_id": instance_id,
            "home_label": "test-home",
        })
        .to_string(),
    )
    .unwrap();
}

#[tokio::test]
async fn test_stalling_listener_timeout_and_late_overtake() {
    let stall_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let stall_port = stall_listener.local_addr().unwrap().port();

    let (stall_tx, stall_rx) = tokio::sync::oneshot::channel();
    let (finish_tx, finish_rx) = std::sync::mpsc::channel();

    std::thread::spawn(move || {
        if let Ok((mut stream, _)) = stall_listener.accept() {
            let mut buf = [0u8; 1024];
            let _ = stream.read(&mut buf);
            let _ = stall_tx.send(());
            let _ = finish_rx.recv_timeout(Duration::from_secs(2));
            let exp = 2000;
            let exp_rfc3339 = "1970-01-01T00:33:20Z";
            let token_a = create_test_jwt(
                DEFAULT_RELAY_ISSUER,
                "instance:inst-test",
                "spl-relay",
                "session.dial",
                2,
                "inst-test",
                500,
                exp,
                "jti-a",
            );
            let body = json!({
                "protocol_version": 2,
                "device_token": token_a,
                "expires_at": exp_rfc3339,
            })
            .to_string();
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(response.as_bytes());
        }
    });

    let journal = TempJournal::new();
    let journal_root = &journal.0;
    setup_test_journal(
        journal_root,
        "inst-test",
        "token-xyz",
        &format!("http://127.0.0.1:{stall_port}"),
    );

    let cache = RelayAccessCache::with_options(DEFAULT_RELAY_ISSUER, Duration::from_millis(50));

    // Caller 1 starts acquire and times out after 50ms
    let res1 = cache.acquire(journal_root, 1000).await;
    assert!(res1.is_err(), "caller 1 should time out");
    tokio::time::timeout(Duration::from_secs(2), stall_rx)
        .await
        .expect("not timed out")
        .expect("stall listener got connection");

    // Setup fast server B
    let fast_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let fast_port = fast_listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        if let Ok((mut stream, _)) = fast_listener.accept() {
            let mut buf = [0u8; 1024];
            let _ = stream.read(&mut buf);
            let exp = 3000;
            let exp_rfc3339 = "1970-01-01T00:50:00Z";
            let token_b = create_test_jwt(
                DEFAULT_RELAY_ISSUER,
                "instance:inst-test",
                "spl-relay",
                "session.dial",
                2,
                "inst-test",
                500,
                exp,
                "jti-b",
            );
            let body = json!({
                "protocol_version": 2,
                "device_token": token_b,
                "expires_at": exp_rfc3339,
            })
            .to_string();
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(response.as_bytes());
        }
    });

    // Update config to point to fast server B
    std::fs::write(
        journal_root.join("config/journal.json"),
        json!({
            "link": {
                "posture": "spl",
                "relay_url": format!("http://127.0.0.1:{fast_port}"),
            }
        })
        .to_string(),
    )
    .unwrap();

    // Caller 2 starts acquire with successor attempt and succeeds
    let res2 = cache.acquire(journal_root, 1000).await.unwrap();
    assert_eq!(res2.expires_at, "1970-01-01T00:50:00Z");

    // Now let stalling worker A finish
    let _ = finish_tx.send(());
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Current cache entry must still be snapshot B
    let current = cache.try_current(journal_root, 1000).unwrap();
    assert_eq!(current.expires_at, "1970-01-01T00:50:00Z");
}

#[tokio::test]
async fn test_epoch_bump_rejects_late_post() {
    let stall_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let stall_port = stall_listener.local_addr().unwrap().port();

    let (stall_tx, stall_rx) = tokio::sync::oneshot::channel();
    let (finish_tx, finish_rx) = std::sync::mpsc::channel();

    std::thread::spawn(move || {
        if let Ok((mut stream, _)) = stall_listener.accept() {
            let mut buf = [0u8; 1024];
            let _ = stream.read(&mut buf);
            let _ = stall_tx.send(());
            let _ = finish_rx.recv_timeout(Duration::from_secs(2));
            let exp = 2000;
            let exp_rfc3339 = "1970-01-01T00:33:20Z";
            let token_a = create_test_jwt(
                DEFAULT_RELAY_ISSUER,
                "instance:inst-test",
                "spl-relay",
                "session.dial",
                2,
                "inst-test",
                500,
                exp,
                "jti-a",
            );
            let body = json!({
                "protocol_version": 2,
                "device_token": token_a,
                "expires_at": exp_rfc3339,
            })
            .to_string();
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(response.as_bytes());
        }
    });

    let journal = TempJournal::new();
    let journal_root = &journal.0;
    setup_test_journal(
        journal_root,
        "inst-test",
        "token-xyz",
        &format!("http://127.0.0.1:{stall_port}"),
    );

    let cache = RelayAccessCache::with_options(DEFAULT_RELAY_ISSUER, Duration::from_millis(50));

    let cache_clone = cache.clone();
    let root_clone = journal_root.to_path_buf();
    let handle = tokio::spawn(async move { cache_clone.acquire(&root_clone, 1000).await });

    tokio::time::timeout(Duration::from_secs(2), stall_rx)
        .await
        .expect("not timed out")
        .expect("stall listener got connection");
    cache.bump_epoch();

    let _ = finish_tx.send(());
    assert!(
        handle.await.unwrap().is_err(),
        "invalidated waiter must not receive old credential"
    );

    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(cache.try_current(journal_root, 1000).is_none());
}

#[test]
fn test_enroll_home_payload_has_no_home_label() {
    use std::io::{Read, Write};
    use std::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();

    let (body_tx, body_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut request = Vec::new();
            let mut buf = [0u8; 1024];
            while let Ok(n) = stream.read(&mut buf) {
                if n == 0 {
                    break;
                }
                request.extend_from_slice(&buf[..n]);
                if let Some(pos) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                    let header_part = String::from_utf8_lossy(&request[..pos]);
                    let cl: usize = header_part
                        .lines()
                        .find_map(|l| {
                            let (k, v) = l.split_once(':')?;
                            if k.trim().eq_ignore_ascii_case("content-length") {
                                v.trim().parse().ok()
                            } else {
                                None
                            }
                        })
                        .unwrap_or(0);
                    if request.len() >= pos + 4 + cl {
                        let body =
                            String::from_utf8_lossy(&request[pos + 4..pos + 4 + cl]).to_string();
                        let _ = body_tx.send(body);
                        break;
                    }
                }
            }
            let resp = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 26\r\n\r\n{\"service_token\":\"tok123\"}";
            let _ = stream.write_all(resp.as_bytes());
        }
    });

    let res = enroll_home(
        &format!("http://127.0.0.1:{port}"),
        "inst-test",
        "ca-pubkey-abc",
        "my home label",
    );
    assert!(res.is_ok());

    let body_str = body_rx
        .recv_timeout(std::time::Duration::from_secs(2))
        .unwrap();
    let body_val: Value = serde_json::from_str(&body_str).unwrap();
    let obj = body_val.as_object().unwrap();
    assert_eq!(obj.get("instance_id").unwrap(), "inst-test");
    assert_eq!(obj.get("ca_pubkey").unwrap(), "ca-pubkey-abc");
    assert!(
        !obj.contains_key("home_label"),
        "enroll_home payload must not contain home_label"
    );
}

fn delayed_access_server(
    instance: &str,
    jti: &str,
) -> (
    String,
    tokio::sync::oneshot::Receiver<()>,
    std::sync::mpsc::Sender<()>,
    std::thread::JoinHandle<()>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let token = create_test_jwt(
        DEFAULT_RELAY_ISSUER,
        &format!("instance:{instance}"),
        "spl-relay",
        "session.dial",
        2,
        instance,
        500,
        2000,
        jti,
    );
    let worker = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut bytes = [0; 4096];
        stream.read(&mut bytes).unwrap();
        let _ = started_tx.send(());
        release_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let body =
            json!({"protocol_version":2,"device_token":token,"expires_at":"1970-01-01T00:33:20Z"})
                .to_string();
        let _ = write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
    });
    (origin, started_rx, release_tx, worker)
}

#[tokio::test]
async fn in_flight_service_changes_reject_returned_capability() {
    for change in ["disable", "disable-enable", "token", "instance"] {
        let journal = TempJournal::new();
        let (origin, started, release, worker) = delayed_access_server("inst-test", "old");
        setup_test_journal(&journal.0, "inst-test", "service-old", &origin);
        let cache = RelayAccessCache::with_options(DEFAULT_RELAY_ISSUER, Duration::from_secs(1));
        let task_cache = cache.clone();
        let root = journal.0.clone();
        let acquisition = tokio::spawn(async move { task_cache.acquire(&root, 1000).await });
        tokio::time::timeout(Duration::from_secs(2), started)
            .await
            .unwrap()
            .unwrap();
        match change {
            "disable" => {
                solstone_core_spl::disable_spl(&journal.0).unwrap();
            }
            "disable-enable" => {
                solstone_core_spl::disable_spl(&journal.0).unwrap();
                setup_test_journal(&journal.0, "inst-test", "service-old", &origin);
            }
            "token" => solstone_core_spl::save_service_token(&journal.0, "service-new").unwrap(),
            "instance" => setup_test_journal(&journal.0, "inst-new", "service-old", &origin),
            _ => unreachable!(),
        }
        release.send(()).unwrap();
        assert!(
            acquisition.await.unwrap().is_err(),
            "{change} must invalidate the returned value"
        );
        assert!(cache.try_current(&journal.0, 1000).is_none());
        worker.join().unwrap();
    }
}

#[tokio::test]
async fn changed_origin_starts_successor_without_joining_old_in_flight() {
    let journal = TempJournal::new();
    let (origin_a, started_a, release_a, worker_a) = delayed_access_server("inst-test", "a");
    setup_test_journal(&journal.0, "inst-test", "service", &origin_a);
    let cache = RelayAccessCache::with_options(DEFAULT_RELAY_ISSUER, Duration::from_secs(1));
    let a_cache = cache.clone();
    let a_root = journal.0.clone();
    let a = tokio::spawn(async move { a_cache.acquire(&a_root, 1000).await });
    tokio::time::timeout(Duration::from_secs(2), started_a)
        .await
        .unwrap()
        .unwrap();
    let (origin_b, started_b, release_b, worker_b) = delayed_access_server("inst-test", "b");
    setup_test_journal(&journal.0, "inst-test", "service", &origin_b);
    let b_cache = cache.clone();
    let b_root = journal.0.clone();
    let b = tokio::spawn(async move { b_cache.acquire(&b_root, 1000).await });
    tokio::time::timeout(Duration::from_secs(2), started_b)
        .await
        .unwrap()
        .unwrap();
    release_b.send(()).unwrap();
    let current = b.await.unwrap().unwrap();
    assert_eq!(current.relay_origin, origin_b);
    release_a.send(()).unwrap();
    assert!(a.await.unwrap().is_err());
    assert_eq!(cache.try_current(&journal.0, 1000).unwrap(), current);
    worker_a.join().unwrap();
    worker_b.join().unwrap();
}

#[tokio::test]
async fn concurrent_current_generation_coalesces_and_reuses_capability() {
    let journal = TempJournal::new();
    let (origin, started, release, worker) = delayed_access_server("inst-test", "shared");
    setup_test_journal(&journal.0, "inst-test", "service", &origin);
    let cache = RelayAccessCache::with_options(DEFAULT_RELAY_ISSUER, Duration::from_secs(1));
    let a_cache = cache.clone();
    let a_root = journal.0.clone();
    let a = tokio::spawn(async move { a_cache.acquire(&a_root, 1000).await });
    tokio::time::timeout(Duration::from_secs(2), started)
        .await
        .unwrap()
        .unwrap();
    let b_cache = cache.clone();
    let b_root = journal.0.clone();
    let b = tokio::spawn(async move { b_cache.acquire(&b_root, 1000).await });
    tokio::task::yield_now().await;
    release.send(()).unwrap();
    let first = a.await.unwrap().unwrap();
    assert_eq!(b.await.unwrap().unwrap(), first);
    worker.join().unwrap();
    // Listener is gone: a usable current cache remains sufficient.
    assert_eq!(cache.acquire(&journal.0, 1000).await.unwrap(), first);
}

#[test]
fn exact_claim_time_validation_rejects_future_issue_and_fractional_expiry() {
    let future = create_test_jwt(
        DEFAULT_RELAY_ISSUER,
        "instance:inst-test",
        "spl-relay",
        "session.dial",
        2,
        "inst-test",
        1061,
        2000,
        "jti",
    );
    assert!(
        validate_device_token(
            &future,
            "inst-test",
            DEFAULT_RELAY_ISSUER,
            "1970-01-01T00:33:20Z",
            1000
        )
        .is_err()
    );
    let valid = create_test_jwt(
        DEFAULT_RELAY_ISSUER,
        "instance:inst-test",
        "spl-relay",
        "session.dial",
        2,
        "inst-test",
        500,
        2000,
        "jti",
    );
    assert!(
        validate_device_token(
            &valid,
            "inst-test",
            DEFAULT_RELAY_ISSUER,
            "1970-01-01T00:33:20.5Z",
            1000
        )
        .is_err()
    );
}

#[tokio::test]
async fn malformed_configuration_is_unavailable_not_disabled() {
    let journal = TempJournal::new();
    std::fs::create_dir_all(journal.0.join("config")).unwrap();
    std::fs::write(journal.0.join("config/journal.json"), "{").unwrap();
    assert!(matches!(
        RelayAccessCache::new().acquire(&journal.0, 1000).await,
        Err(RelayAccessError::Unavailable(_))
    ));
}

#[tokio::test]
async fn capability_expiring_during_body_wait_is_not_ready() {
    let journal = TempJournal::new();
    let (origin, started, release, worker) = delayed_access_server("inst-test", "expiring");
    setup_test_journal(&journal.0, "inst-test", "service", &origin);
    let cache = RelayAccessCache::with_options(DEFAULT_RELAY_ISSUER, Duration::from_secs(3));
    let task_cache = cache.clone();
    let root = journal.0.clone();
    let task = tokio::spawn(async move { task_cache.acquire(&root, 1999).await });
    tokio::time::timeout(Duration::from_secs(2), started)
        .await
        .unwrap()
        .unwrap();
    tokio::time::sleep(Duration::from_millis(1100)).await;
    release.send(()).unwrap();
    assert!(task.await.unwrap().is_err());
    assert!(cache.try_current(&journal.0, 2000).is_none());
    worker.join().unwrap();
}
