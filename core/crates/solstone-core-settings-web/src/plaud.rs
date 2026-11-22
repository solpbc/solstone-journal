// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Settings routes for official Plaud MCP OAuth sign-in, status, manual sync, and disconnect.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::{
    Extension,
    body::Bytes,
    extract::Path as AxumPath,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::{Map, Value, json};
use solstone_core_callosum::{
    CallosumConnectionPhase, CallosumReceiveEvent, CallosumSocketConnection,
};
use solstone_core_convey_http::window_catalog::text;
use solstone_core_import::plaud::{self, PlaudAttemptState, PlaudBeginError, PlaudSignInOutcome};

use crate::{
    SourceSyncDelegate,
    http::json_response,
    request_body::{JsonBody, json_body},
};

/// A Plaud sync this server requested via Callosum is still running.
static SYNCING: AtomicBool = AtomicBool::new(false);

const LOOPBACK_SIGNIN_TIMEOUT: Duration = Duration::from_secs(plaud::ATTEMPT_EXPIRATION_SECS);
const PASTE_SIGNIN_TIMEOUT: Duration = Duration::from_secs(30);
const BUS_CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
const SYNC_WATCH_MAX_DURATION: Duration = Duration::from_secs(35 * 60);

fn attention_text(locale: &str, code: &str) -> String {
    let id = match code {
        "auth" => "journal.window.settings.plaud.response.auth",
        "audio_not_ready" => "journal.window.settings.plaud.response.audio.not.ready",
        "download" => "journal.window.settings.plaud.response.download",
        "storage" | "sync" => "journal.window.settings.plaud.response.storage",
        _ => "journal.window.settings.plaud.response.unavailable",
    };
    text(locale, id)
}

fn outcome_text(locale: &str, outcome: PlaudSignInOutcome) -> String {
    let id = match outcome {
        PlaudSignInOutcome::Denied => "journal.window.settings.plaud.response.signin.denied",
        PlaudSignInOutcome::CallbackInvalid => {
            "journal.window.settings.plaud.response.callback.invalid"
        }
        PlaudSignInOutcome::Expired => "journal.window.settings.plaud.response.attempt.expired",
        PlaudSignInOutcome::Cancelled => "journal.window.settings.plaud.signin.cancelled",
        PlaudSignInOutcome::Busy => "journal.window.settings.plaud.response.busy",
        PlaudSignInOutcome::Storage => "journal.window.settings.plaud.response.storage",
        PlaudSignInOutcome::ExchangeFailed | PlaudSignInOutcome::SignedIn => {
            "journal.window.settings.plaud.signin.failed"
        }
    };
    text(locale, id)
}

fn error_response(status: StatusCode, code: &str, message: String) -> Response {
    (
        status,
        axum::Json(json!({ "error": message, "code": code })),
    )
        .into_response()
}

pub async fn get(locale: &str, root: PathBuf) -> Response {
    let syncing_in_process = SYNCING.load(Ordering::SeqCst);
    let Ok(status) =
        tokio::task::spawn_blocking(move || plaud::status(&root, syncing_in_process)).await
    else {
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "storage",
            text(locale, "journal.window.settings.plaud.load.failed"),
        );
    };
    let attention = status
        .attention_code
        .as_deref()
        .map(|code| attention_text(locale, code));
    json_response(json!({
        "connected": status.connected,
        "account_label": status.account_label,
        "enabled": status.enabled,
        "syncing": status.syncing,
        "recordings": status.recordings,
        "last_sync": status.last_sync,
        "last_added": status.last_added,
        "attention": attention,
        "attention_code": status.attention_code,
    }))
}

pub async fn begin_sign_in(locale: &str, root: PathBuf, body: Bytes) -> Response {
    let redirect_uri = if body.is_empty() {
        None
    } else {
        match json_body(body) {
            JsonBody::Value(Value::Object(map)) => map
                .get("redirect_uri")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|uri| !uri.is_empty())
                .map(str::to_owned),
            JsonBody::Missing => None,
            _ => {
                return error_response(
                    StatusCode::BAD_REQUEST,
                    "invalid_redirect_uri",
                    text(
                        locale,
                        "journal.window.settings.plaud.response.callback.invalid",
                    ),
                );
            }
        }
    };

    let attempt =
        match tokio::task::spawn_blocking(move || plaud::begin_sign_in(redirect_uri.as_deref()))
            .await
        {
            Ok(Ok(attempt)) => attempt,
            Ok(Err(PlaudBeginError::InvalidRedirectUri)) => {
                return error_response(
                    StatusCode::BAD_REQUEST,
                    "invalid_redirect_uri",
                    text(
                        locale,
                        "journal.window.settings.plaud.response.callback.invalid",
                    ),
                );
            }
            Ok(Err(err)) => {
                log::warn!("Plaud sign-in begin failed: {err}");
                return error_response(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "signin_unavailable",
                    text(
                        locale,
                        "journal.window.settings.plaud.response.signin.unavailable",
                    ),
                );
            }
            Err(_) => {
                return error_response(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "signin_unavailable",
                    text(
                        locale,
                        "journal.window.settings.plaud.response.signin.unavailable",
                    ),
                );
            }
        };

    if attempt.listener.is_some() {
        let bg_attempt = attempt.clone();
        let bg_root = root;
        std::thread::spawn(move || {
            let _ = plaud::finish_sign_in(&bg_root, &bg_attempt, None, LOOPBACK_SIGNIN_TIMEOUT);
        });
    }

    json_response(json!({
        "attempt_id": attempt.attempt_id,
        "authorize_url": attempt.authorize_url,
        "expected_state": attempt.expected_state,
        "redirect_uri": attempt.redirect_uri,
    }))
}

pub async fn sign_in_status(locale: &str, AxumPath(attempt_id): AxumPath<String>) -> Response {
    let status = plaud::attempt_status(&attempt_id);
    let error = match (status.state, status.reason) {
        (PlaudAttemptState::Failed | PlaudAttemptState::Expired, Some(outcome)) => {
            Some(outcome_text(locale, outcome))
        }
        _ => None,
    };
    json_response(json!({
        "state": status.state,
        "reason": status.reason,
        "account_label": status.account_label,
        "error": error,
    }))
}

pub async fn finish_sign_in(
    locale: &str,
    root: PathBuf,
    AxumPath(attempt_id): AxumPath<String>,
    body: Bytes,
) -> Response {
    let JsonBody::Value(Value::Object(map)) = json_body(body) else {
        return error_response(
            StatusCode::BAD_REQUEST,
            "callback_invalid",
            text(
                locale,
                "journal.window.settings.plaud.response.callback.invalid",
            ),
        );
    };
    let Some(callback_url) = map
        .get("callback_url")
        .or_else(|| map.get("redirect_url"))
        .or_else(|| map.get("url"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|uri| !uri.is_empty())
        .map(str::to_owned)
    else {
        return error_response(
            StatusCode::BAD_REQUEST,
            "callback_invalid",
            text(
                locale,
                "journal.window.settings.plaud.response.callback.invalid",
            ),
        );
    };

    let attempt = match plaud::active_attempt_named(&attempt_id) {
        Ok(attempt) => attempt,
        Err(outcome) => {
            let status = match outcome {
                PlaudSignInOutcome::Expired | PlaudSignInOutcome::Cancelled => StatusCode::GONE,
                _ => StatusCode::BAD_REQUEST,
            };
            return error_response(status, outcome.as_str(), outcome_text(locale, outcome));
        }
    };

    match tokio::task::spawn_blocking(move || {
        plaud::finish_sign_in(&root, &attempt, Some(&callback_url), PASTE_SIGNIN_TIMEOUT)
    })
    .await
    {
        Ok(Ok(finished)) => json_response(json!({
            "success": true,
            "reason": finished.outcome.as_str(),
            "account_label": finished.account_label,
        })),
        Ok(Err(outcome)) => {
            let status = match outcome {
                PlaudSignInOutcome::Expired | PlaudSignInOutcome::Cancelled => StatusCode::GONE,
                PlaudSignInOutcome::Busy => StatusCode::CONFLICT,
                PlaudSignInOutcome::Storage => StatusCode::INTERNAL_SERVER_ERROR,
                _ => StatusCode::BAD_REQUEST,
            };
            error_response(status, outcome.as_str(), outcome_text(locale, outcome))
        }
        Err(_) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "storage",
            text(locale, "journal.window.settings.plaud.response.storage"),
        ),
    }
}

pub async fn cancel_sign_in() -> Response {
    plaud::cancel_active_attempt();
    json_response(json!({ "success": true }))
}

pub async fn disconnect(locale: &str, root: PathBuf) -> Response {
    match tokio::task::spawn_blocking(move || plaud::disconnect(&root)).await {
        Ok(Ok(result)) => json_response(json!({
            "success": true,
            "revoked": result.revoked,
        })),
        Ok(Err(err)) => {
            log::warn!("Plaud disconnect failed: {err}");
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "storage",
                text(locale, "journal.window.settings.plaud.disconnect.failed"),
            )
        }
        Err(_) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "storage",
            text(locale, "journal.window.settings.plaud.disconnect.failed"),
        ),
    }
}

pub async fn sync_now(
    locale: &str,
    root: PathBuf,
    delegate: Option<Extension<SourceSyncDelegate>>,
) -> Response {
    let check_root = root.clone();
    let Ok((connected, locked)) = tokio::task::spawn_blocking(move || {
        (
            plaud::is_connected(&check_root),
            plaud::is_sync_locked(&check_root),
        )
    })
    .await
    else {
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "storage",
            text(locale, "journal.window.settings.plaud.response.storage"),
        );
    };

    if !connected {
        return error_response(
            StatusCode::BAD_REQUEST,
            "not_connected",
            text(
                locale,
                "journal.window.settings.plaud.response.not.connected",
            ),
        );
    }
    if SYNCING.load(Ordering::SeqCst) || locked {
        return error_response(
            StatusCode::BAD_REQUEST,
            "busy",
            text(locale, "journal.window.settings.plaud.response.busy"),
        );
    }

    if let Some(Extension(delegate)) = delegate {
        return delegate.trigger_plaud_sync().await;
    }

    start_desktop_sync(locale, &root).await
}

struct SyncingGuard;

impl Drop for SyncingGuard {
    fn drop(&mut self) {
        SYNCING.store(false, Ordering::SeqCst);
    }
}

async fn start_desktop_sync(locale: &str, root: &Path) -> Response {
    let socket = root.join("health").join("callosum.sock");
    let mut connection = CallosumSocketConnection::new(socket, Map::new());
    connection.start();
    let connected = tokio::time::timeout(BUS_CONNECT_TIMEOUT, async {
        loop {
            match connection.next_event().await {
                Some(CallosumReceiveEvent::Continuity {
                    phase: CallosumConnectionPhase::Connected,
                    ..
                }) => return true,
                Some(CallosumReceiveEvent::Continuity {
                    phase:
                        CallosumConnectionPhase::Unavailable { .. }
                        | CallosumConnectionPhase::Stopped { .. },
                    ..
                })
                | None => return false,
                Some(_) => {}
            }
        }
    })
    .await;

    if !matches!(connected, Ok(true)) {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            text(locale, "journal.window.settings.plaud.check.failed"),
        );
    }

    if SYNCING.swap(true, Ordering::SeqCst) {
        return error_response(
            StatusCode::BAD_REQUEST,
            "busy",
            text(locale, "journal.window.settings.plaud.response.busy"),
        );
    }
    let guard = SyncingGuard;

    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    let task_ref = format!("plaud-sync-{stamp}");
    let mut fields = Map::new();
    fields.insert("ref".to_owned(), Value::String(task_ref.clone()));
    fields.insert(
        "cmd".to_owned(),
        json!([
            "solstone", "journal", "importer", "--sync", "plaud", "--save"
        ]),
    );
    fields.insert("queue_if_active_cmd_differs".to_owned(), Value::Bool(true));

    if !connection.emit("supervisor", "request", fields) {
        drop(guard);
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            text(locale, "journal.window.settings.plaud.check.failed"),
        );
    }

    tokio::spawn(async move {
        let _guard = guard;
        let _ = tokio::time::timeout(SYNC_WATCH_MAX_DURATION, async move {
            loop {
                match connection.next_event().await {
                    Some(CallosumReceiveEvent::Envelope { envelope, .. })
                        if envelope.tract == "supervisor"
                            && envelope.event == "stopped"
                            && envelope.extra.get("ref").and_then(Value::as_str)
                                == Some(task_ref.as_str()) =>
                    {
                        break;
                    }
                    Some(CallosumReceiveEvent::Continuity {
                        phase: CallosumConnectionPhase::Stopped { .. },
                        ..
                    })
                    | None => break,
                    _ => {}
                }
            }
        })
        .await;
    });

    json_response(json!({ "success": true }))
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use super::*;
    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use solstone_core_callosum::CallosumSocketServer;
    use solstone_core_oauth::store::{
        OAuthCredentialDoc, OAuthTokens, now_unix_secs, save_credential_file,
    };
    use tower::ServiceExt;

    fn seed_connected_plaud(root: &Path) {
        std::fs::create_dir_all(root.join("config")).unwrap();
        std::fs::write(root.join("config/journal.json"), "{}").unwrap();
        let mut doc = OAuthCredentialDoc::new(&plaud::service_id(), "https://mcp.plaud.ai/token");
        doc.account_label = Some("Alex Rivera".to_owned());
        doc.registration = Some(solstone_core_oauth::store::ClientRegistration {
            client_id: "plaud-test-client".to_owned(),
            client_secret: None,
            mode: solstone_core_oauth::registration::RegistrationMode::Dcr,
            client_refused: false,
        });
        doc.tokens = Some(OAuthTokens {
            access_token: "secret-access-token-must-not-leak".to_owned(),
            refresh_token: Some("secret-refresh-token-must-not-leak".to_owned()),
            token_type: "Bearer".to_owned(),
            expires_at: Some(now_unix_secs() + 3600),
            scopes: Vec::new(),
        });
        save_credential_file(root, &plaud::service_id(), &doc).unwrap();
    }

    #[tokio::test]
    async fn status_masks_tokens_disconnect_keeps_recordings_and_invalid_redirect_is_refused() {
        let root = tempfile::tempdir().unwrap();
        seed_connected_plaud(root.path());
        std::fs::create_dir_all(root.path().join("imports")).unwrap();
        std::fs::write(
            root.path().join("imports/plaud.json"),
            r#"{"backend":"plaud","last_sync":"2026-11-22T00:00:00Z","last_added":"2026-11-21T23:00:00Z","files":{"of_1":{"status":"imported"}}}"#,
        )
        .unwrap();
        std::fs::write(
            root.path().join("config/schedules.json"),
            r#"{"sync:plaud":{"cmd":["solstone","journal","importer","--sync","plaud","--save"],"every":"hourly","enabled":true}}"#,
        )
        .unwrap();

        let router = crate::routes(root.path().to_path_buf());
        let response = router
            .clone()
            .oneshot(
                Request::get("/app/settings/api/plaud")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let raw = String::from_utf8_lossy(&body);
        assert!(!raw.contains("secret-access-token-must-not-leak"));
        assert!(!raw.contains("secret-refresh-token-must-not-leak"));
        let status: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(status["connected"], true);
        assert_eq!(status["account_label"], "Alex Rivera");
        assert_eq!(status["enabled"], true);
        assert_eq!(status["recordings"], 1);
        assert_eq!(status["last_sync"], "2026-11-22T00:00:00Z");
        assert_eq!(status["last_added"], "2026-11-21T23:00:00Z");

        // Fail-closed broker redirect validation.
        let bad_redirect = router
            .clone()
            .oneshot(
                Request::post("/app/settings/api/plaud/sign-in")
                    .body(Body::from(r#"{"redirect_uri":"https://evil.example/cb"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(bad_redirect.status(), StatusCode::BAD_REQUEST);

        // Unknown sign-in attempt returns expired.
        let unknown = router
            .clone()
            .oneshot(
                Request::get("/app/settings/api/plaud/sign-in/no-such-attempt")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(unknown.status(), StatusCode::OK);
        let unknown_body: Value =
            serde_json::from_slice(&to_bytes(unknown.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(unknown_body["state"], "expired");

        // Disconnect clears tokens & schedule, keeps imported recordings.
        let disc = router
            .clone()
            .oneshot(
                Request::delete("/app/settings/api/plaud")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(disc.status(), StatusCode::OK);

        let after = router
            .clone()
            .oneshot(
                Request::get("/app/settings/api/plaud")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let after_body: Value =
            serde_json::from_slice(&to_bytes(after.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(after_body["connected"], false);
        assert_eq!(after_body["enabled"], false);
        assert_eq!(after_body["recordings"], 1);

        // Manual sync refused when disconnected.
        let sync_refused = router
            .clone()
            .oneshot(
                Request::post("/app/settings/api/plaud/sync")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(sync_refused.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn manual_sync_dispatches_via_callosum_and_tracks_syncing_until_stopped() {
        let root = tempfile::tempdir().unwrap();
        seed_connected_plaud(root.path());
        std::fs::create_dir_all(root.path().join("health")).unwrap();
        let sock = root.path().join("health").join("callosum.sock");
        #[cfg(windows)]
        let _namespace =
            solstone_core_callosum::test_fixture::WindowsPipeNamespace::register(sock.clone());
        let server = CallosumSocketServer::bind(sock.clone()).await.unwrap();
        let mut supervisor = CallosumSocketConnection::new(sock, Map::new());
        supervisor.start();
        while !matches!(
            supervisor.next_event().await,
            Some(CallosumReceiveEvent::Continuity {
                phase: CallosumConnectionPhase::Connected,
                ..
            })
        ) {}

        let router = crate::routes(root.path().to_path_buf());
        let response = router
            .clone()
            .oneshot(
                Request::post("/app/settings/api/plaud/sync")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(SYNCING.load(Ordering::SeqCst));

        let request_env = loop {
            let msg = supervisor.next_message().await.unwrap();
            if msg.tract == "supervisor" && msg.event == "request" {
                break msg;
            }
        };
        assert_eq!(
            request_env.extra["cmd"],
            json!([
                "solstone", "journal", "importer", "--sync", "plaud", "--save"
            ])
        );
        let task_ref = request_env.extra["ref"].as_str().unwrap().to_owned();

        // Emit stopped so the background watcher clears SYNCING.
        let mut stopped = Map::new();
        stopped.insert("ref".to_owned(), Value::String(task_ref));
        stopped.insert("exit_code".to_owned(), Value::from(0));
        assert!(supervisor.emit("supervisor", "stopped", stopped));

        let cleared = tokio::time::timeout(Duration::from_secs(5), async {
            while SYNCING.load(Ordering::SeqCst) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;
        assert!(cleared.is_ok(), "SYNCING cleared after supervisor/stopped");
        server.stop().await;
    }
}
