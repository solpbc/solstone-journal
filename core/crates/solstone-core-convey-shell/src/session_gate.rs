// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::PathBuf;

use axum::body::Body;
use axum::extract::State;
use axum::http::{Request, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use solstone_core_convey_http::envelope::error_envelope;

use crate::registry::served_app;
use crate::session::{SessionState, classify_session};
use solstone_core_journal_io::whole_journal_restore::TargetUseOwner;

#[derive(Debug, Clone)]
pub struct SessionGateState {
    pub journal_root: PathBuf,
    pub include_agents: bool,
    pub target_use: TargetUseOwner,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionExemption {
    Favicon,
    TopLevelStatic,
    InitSetup,
    UnknownAppPrefix,
    UnmatchedFallback,
}

pub const SESSION_GATE_EXEMPTIONS: &[SessionExemption] = &[
    SessionExemption::Favicon,
    SessionExemption::TopLevelStatic,
    SessionExemption::InitSetup,
    SessionExemption::UnknownAppPrefix,
    SessionExemption::UnmatchedFallback,
];

pub fn apply_layer(router: axum::Router, journal_root: PathBuf) -> axum::Router {
    apply_layer_with_agents(router, journal_root, false)
}

pub fn apply_layer_with_agents(
    router: axum::Router,
    journal_root: PathBuf,
    include_agents: bool,
) -> axum::Router {
    let target_use = TargetUseOwner::new(&journal_root);
    apply_layer_with_owner(router, journal_root, include_agents, target_use)
}

pub(crate) fn apply_layer_with_owner(
    router: axum::Router,
    journal_root: PathBuf,
    include_agents: bool,
    target_use: TargetUseOwner,
) -> axum::Router {
    router.route_layer(middleware::from_fn_with_state(
        SessionGateState {
            target_use,
            journal_root,
            include_agents,
        },
        require_session,
    ))
}

#[cfg(test)]
fn is_exempt(path: &str) -> bool {
    is_exempt_with_agents(path, false)
}

fn is_exempt_with_agents(path: &str, include_agents: bool) -> bool {
    SESSION_GATE_EXEMPTIONS
        .iter()
        .any(|exemption| match exemption {
            SessionExemption::Favicon => path == "/favicon.ico",
            SessionExemption::TopLevelStatic => path.starts_with("/static/"),
            SessionExemption::InitSetup => path == "/init" || path.starts_with("/init/"),
            SessionExemption::UnknownAppPrefix => {
                let mut segments = path.trim_start_matches('/').split('/');
                matches!(segments.next(), Some("app"))
                    && segments.next().is_some_and(|name| {
                        // `/app/link` aliases `/app/network` and is not an app
                        // registry entry; it must stay session-gated.
                        // `/app/devices` left the registry; leftover ingest
                        // routes and the permanent redirects must stay gated.
                        // Leftover news paths stay gated.
                        name != "link"
                            && name != "devices"
                            && name != "news"
                            && name != "agents"
                            && served_app(name, include_agents).is_none()
                    })
            }
            // The router leaves unmatched paths outside its route layer. This
            // declarative entry records that structural exemption with the others.
            SessionExemption::UnmatchedFallback => false,
        })
}

async fn require_session(
    State(state): State<SessionGateState>,
    mut request: Request<Body>,
    next: Next,
) -> Response {
    let started = std::time::Instant::now();
    let path = request.uri().path();
    if is_exempt_with_agents(path, state.include_agents) {
        return next.run(request).await;
    }
    if solstone_core_journal_io::whole_journal_restore::whole_journal_restore_blocks(
        &state.journal_root,
    ) {
        return redirect_to_init();
    }
    match classify_session(&state.journal_root) {
        SessionState::Established => {
            let Ok(target_use) = state.target_use.begin() else {
                return redirect_to_init();
            };
            request.extensions_mut().insert(target_use.root());
            request
                .extensions_mut()
                .insert(solstone_core_convey_http::coalesce::CoalesceScope(
                    state.journal_root.clone(),
                ));
            let gate = started.elapsed();
            let mut response =
                solstone_core_convey_http::request::run_admitted(target_use, request, next).await;
            if crate::timing::is_dashboard_response(&response) {
                crate::timing::append(&mut response, "gate", gate);
            }
            response
        }
        SessionState::Unestablished => redirect_to_init(),
        SessionState::Corrupt { detail } => corrupt_response(path, detail),
    }
}

fn redirect_to_init() -> Response {
    let location = "/init";
    Response::builder()
        .status(StatusCode::FOUND)
        .header(header::LOCATION, location)
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .body(Body::from(redirect_body(location)))
        .expect("redirect response builds")
}

pub(crate) fn redirect_body(location: &str) -> String {
    format!(
        "<!doctype html>\n<html lang=en>\n<title>Redirecting...</title>\n<h1>Redirecting...</h1>\n<p>You should be redirected automatically to the target URL: <a href=\"{location}\">{location}</a>. If not, click the link.\n"
    )
}

fn corrupt_response(path: &str, detail: String) -> Response {
    if path
        .trim_matches('/')
        .split('/')
        .any(|segment| segment == "api")
    {
        return error_envelope(
            "corrupt_config",
            "your settings couldn't be read.",
            detail,
            StatusCode::INTERNAL_SERVER_ERROR,
        )
        .into_response();
    }
    Response::builder()
        .status(StatusCode::INTERNAL_SERVER_ERROR)
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(Body::from(detail))
        .expect("corrupt response builds")
}

#[cfg(test)]
mod tests {
    use super::{SessionExemption, apply_layer, is_exempt};
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::routing::get;
    use tower::ServiceExt;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_admitted_original_stream_holds_copy_until_its_body_finishes() {
        use solstone_core_journal_io::whole_journal_restore::{
            hold_target_for_restore, release_target_for_restore, wait_for_target_use_to_end,
        };
        use std::time::Duration;
        use tokio::io::AsyncReadExt;
        let journal = tempfile::tempdir().unwrap();
        std::fs::create_dir(journal.path().join("config")).unwrap();
        std::fs::write(
            journal.path().join("config/journal.json"),
            br#"{"setup":{"completed_at":1}}"#,
        )
        .unwrap();
        let original = journal.path().join("original.bin");
        let expected = vec![0x81_u8; 16_385];
        std::fs::write(&original, &expected).unwrap();
        let router = apply_layer(
            axum::Router::new().route(
                "/original",
                get(move || {
                    let original = original.clone();
                    async move {
                        let file = tokio::fs::File::open(original).await.unwrap();
                        Body::from_stream(futures_util::stream::unfold(file, |mut file| async {
                            let mut bytes = vec![0; 4096];
                            match file.read(&mut bytes).await {
                                Ok(0) => None,
                                Ok(length) => {
                                    bytes.truncate(length);
                                    Some((Ok::<_, std::io::Error>(bytes), file))
                                }
                                Err(error) => Some((Err(error), file)),
                            }
                        }))
                    }
                }),
            ),
            journal.path().to_owned(),
        );
        let response = router
            .oneshot(Request::get("/original").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        hold_target_for_restore(journal.path());
        let held = journal.path().to_owned();
        let mut drain = tokio::task::spawn_blocking(move || wait_for_target_use_to_end(&held));
        assert!(
            tokio::time::timeout(Duration::from_millis(100), &mut drain)
                .await
                .is_err(),
            "headers do not finish an admitted original read"
        );
        let received = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(received.as_ref(), expected);
        tokio::time::timeout(Duration::from_secs(3), drain)
            .await
            .unwrap()
            .unwrap();
        release_target_for_restore(journal.path());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn lost_http_client_retains_its_actual_blocking_writer_until_drain() {
        use solstone_core_journal_io::whole_journal_restore::{
            hold_target_for_restore, release_target_for_restore, wait_for_target_use_to_end,
        };
        use std::sync::{Arc, Mutex, mpsc};
        use std::time::Duration;
        let journal = tempfile::tempdir().unwrap();
        std::fs::create_dir(journal.path().join("config")).unwrap();
        std::fs::write(
            journal.path().join("config/journal.json"),
            br#"{"setup":{"completed_at":1}}"#,
        )
        .unwrap();
        let path = journal.path().to_owned();
        let written = path.join("client-lost-http-write");
        let (entered, observing) = tokio::sync::oneshot::channel();
        let (release, released) = mpsc::channel();
        let frame = Arc::new(Mutex::new(Some((entered, released))));
        let target = written.clone();
        let router = apply_layer(
            axum::Router::new().route(
                "/controlled",
                get(move || {
                    let (entered, released) = frame.lock().unwrap().take().unwrap();
                    let target = target.clone();
                    async move {
                        tokio::task::spawn_blocking(move || {
                            entered.send(()).unwrap();
                            released.recv_timeout(Duration::from_secs(5)).unwrap();
                            std::fs::write(target, b"writer returned after client loss").unwrap();
                        })
                        .await
                        .unwrap();
                        "done"
                    }
                }),
            ),
            path.clone(),
        );
        let request = tokio::spawn(async move {
            router
                .oneshot(Request::get("/controlled").body(Body::empty()).unwrap())
                .await
                .unwrap()
        });
        tokio::time::timeout(Duration::from_secs(3), observing)
            .await
            .unwrap()
            .unwrap();
        request.abort();
        assert!(request.await.unwrap_err().is_cancelled());
        hold_target_for_restore(&path);
        let held = path.clone();
        let mut drain = tokio::task::spawn_blocking(move || wait_for_target_use_to_end(&held));
        assert!(
            tokio::time::timeout(Duration::from_millis(100), &mut drain)
                .await
                .is_err(),
            "the lost client cannot release its still-running writer"
        );
        assert!(!written.exists());
        release.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(3), drain)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            std::fs::read(written).unwrap(),
            b"writer returned after client loss"
        );
        release_target_for_restore(&path);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_admitted_http_writer_drains_before_copy_and_the_router_resumes() {
        use solstone_core_journal_io::whole_journal_restore::{
            hold_target_for_restore, release_target_for_restore, wait_for_target_use_to_end,
        };
        use std::sync::{Arc, Mutex};
        let journal = tempfile::tempdir().unwrap();
        std::fs::create_dir(journal.path().join("config")).unwrap();
        std::fs::write(
            journal.path().join("config/journal.json"),
            br#"{"setup":{"completed_at":1}}"#,
        )
        .unwrap();
        let path = journal.path().to_owned();
        let written = path.join("admitted-http-write");
        let release = Arc::new(tokio::sync::Notify::new());
        let unblock = release.clone();
        let (entered, observing) = tokio::sync::oneshot::channel();
        let entered = Arc::new(Mutex::new(Some(entered)));
        let target = written.clone();
        let router = apply_layer(
            axum::Router::new()
                .route(
                    "/controlled",
                    get(move || {
                        let entered = entered.clone();
                        let unblock = unblock.clone();
                        let target = target.clone();
                        async move {
                            entered.lock().unwrap().take().unwrap().send(()).unwrap();
                            unblock.notified().await;
                            std::fs::write(target, b"actual admitted handler returned").unwrap();
                            "done"
                        }
                    }),
                )
                .route("/ready", get(|| async { "ready" })),
            path.clone(),
        );
        let serving = router.clone();
        let request = tokio::spawn(async move {
            serving
                .oneshot(Request::get("/controlled").body(Body::empty()).unwrap())
                .await
                .unwrap()
        });
        tokio::time::timeout(std::time::Duration::from_secs(3), observing)
            .await
            .unwrap()
            .unwrap();
        hold_target_for_restore(&path);
        let held = path.clone();
        let (waiting, observed_wait) = tokio::sync::oneshot::channel();
        let drain = tokio::task::spawn_blocking(move || {
            waiting.send(()).unwrap();
            wait_for_target_use_to_end(&held);
        });
        observed_wait.await.unwrap();
        let refused = router
            .clone()
            .oneshot(Request::get("/ready").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(refused.status(), StatusCode::FOUND);
        assert!(
            !drain.is_finished(),
            "copy waits for the admitted HTTP handler"
        );
        assert!(!written.exists());
        release.notify_one();
        assert_eq!(request.await.unwrap().status(), StatusCode::OK);
        tokio::time::timeout(std::time::Duration::from_secs(3), drain)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            std::fs::read(written).unwrap(),
            b"actual admitted handler returned"
        );
        release_target_for_restore(&path);
        let resumed = router
            .oneshot(Request::get("/ready").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resumed.status(), StatusCode::OK);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn coalesced_route_wave_maintains_target_use_and_answers_aborted_clients() {
        use solstone_core_convey_http::coalesce::{self, Disposition, JoinEvent, JoinNotice};
        use solstone_core_convey_http::identity::AccessBasis;
        use solstone_core_journal_io::whole_journal_restore::{
            hold_target_for_restore, release_target_for_restore, wait_for_target_use_to_end,
        };
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::{Arc, Mutex};
        use tokio::sync::{Notify, mpsc, oneshot};

        let journal = tempfile::tempdir().unwrap();
        let path = journal.path().to_owned();
        std::fs::create_dir(path.join("config")).unwrap();
        std::fs::write(
            path.join("config").join("journal.json"),
            br#"{"setup":{"completed_at":1}}"#,
        )
        .unwrap();

        struct RestoreGuard(std::path::PathBuf);
        impl Drop for RestoreGuard {
            fn drop(&mut self) {
                release_target_for_restore(&self.0);
            }
        }
        let _guard = RestoreGuard(path.clone());

        let executions = Arc::new(AtomicUsize::new(0));
        let (entered_tx, mut entered_rx) = mpsc::unbounded_channel::<usize>();
        let release_slots: Arc<Mutex<std::collections::HashMap<usize, oneshot::Sender<()>>>> =
            Arc::new(Mutex::new(std::collections::HashMap::new()));

        let exec_c = executions.clone();
        let rel_slots_c = release_slots.clone();

        let handler = move || {
            let exec = exec_c.clone();
            let entered_tx = entered_tx.clone();
            let rel_slots = rel_slots_c.clone();
            async move {
                let id = exec.fetch_add(1, Ordering::SeqCst) + 1;
                let (rel_tx, rel_rx) = oneshot::channel();
                rel_slots.lock().unwrap().insert(id, rel_tx);
                entered_tx.send(id).unwrap();
                rel_rx.await.unwrap();
                format!("{id}")
            }
        };

        let router = apply_layer(
            axum::Router::new().route("/controlled", get(handler).layer(coalesce::layer())),
            path.clone(),
        );

        let registrations: Arc<Mutex<Vec<Disposition>>> = Arc::new(Mutex::new(Vec::new()));
        let answered_count = Arc::new(AtomicUsize::new(0));
        let all_answered_notify = Arc::new(Notify::new());

        let spawn_client = |router: axum::Router,
                            regs: Arc<Mutex<Vec<Disposition>>>,
                            ans_cnt: Arc<AtomicUsize>,
                            ans_not: Arc<Notify>| {
            let (reg_tx, reg_rx) = oneshot::channel();
            let reg_tx = Arc::new(Mutex::new(Some(reg_tx)));
            let handle = tokio::spawn(async move {
                let notice = JoinNotice::new(move |event| match event {
                    JoinEvent::Registered(disp) => {
                        regs.lock().unwrap().push(disp);
                        if let Some(tx) = reg_tx.lock().unwrap().take() {
                            let _ = tx.send(disp);
                        }
                    }
                    JoinEvent::Answered => {
                        if ans_cnt.fetch_add(1, Ordering::SeqCst) + 1 == 100 {
                            ans_not.notify_waiters();
                        }
                    }
                });
                let req = Request::get("/controlled")
                    .extension(AccessBasis::Localhost)
                    .extension(notice)
                    .body(Body::empty())
                    .unwrap();
                router.oneshot(req).await.unwrap()
            });
            (handle, reg_rx)
        };

        // --- Wave 1 ---
        let mut w1_handles = Vec::new();
        let mut w1_reg_rxs = Vec::new();
        for _ in 0..10 {
            let (h, rx) = spawn_client(
                router.clone(),
                registrations.clone(),
                answered_count.clone(),
                all_answered_notify.clone(),
            );
            w1_handles.push(h);
            w1_reg_rxs.push(rx);
        }
        for rx in w1_reg_rxs {
            rx.await.unwrap();
        }
        let e1 = entered_rx.recv().await.unwrap();
        assert_eq!(e1, 1);

        // Abort the 10 client tasks
        for h in w1_handles {
            h.abort();
        }

        // Spawn 10 retries for wave 1
        let mut retry_tasks: Vec<(usize, tokio::task::JoinHandle<axum::response::Response>)> =
            Vec::new();
        let mut w1_retry_reg_rxs = Vec::new();
        for _ in 0..10 {
            let (h, rx) = spawn_client(
                router.clone(),
                registrations.clone(),
                answered_count.clone(),
                all_answered_notify.clone(),
            );
            retry_tasks.push((1, h));
            w1_retry_reg_rxs.push(rx);
        }
        for rx in w1_retry_reg_rxs {
            rx.await.unwrap();
        }

        // Release execution 1
        release_slots
            .lock()
            .unwrap()
            .remove(&1)
            .unwrap()
            .send(())
            .unwrap();

        // Wait until execution 2 enters
        let e2 = entered_rx.recv().await.unwrap();
        assert_eq!(e2, 2);

        // --- Waves 2..=5 ---
        for w in 2..=5 {
            let mut handles = Vec::new();
            let mut reg_rxs = Vec::new();
            for _ in 0..10 {
                let (h, rx) = spawn_client(
                    router.clone(),
                    registrations.clone(),
                    answered_count.clone(),
                    all_answered_notify.clone(),
                );
                handles.push(h);
                reg_rxs.push(rx);
            }
            for rx in reg_rxs {
                rx.await.unwrap();
            }

            // Abort those 10
            for h in handles {
                h.abort();
            }

            // Spawn 10 retries
            let mut retry_reg_rxs = Vec::new();
            for _ in 0..10 {
                let (h, rx) = spawn_client(
                    router.clone(),
                    registrations.clone(),
                    answered_count.clone(),
                    all_answered_notify.clone(),
                );
                retry_tasks.push((w, h));
                retry_reg_rxs.push(rx);
            }
            for rx in retry_reg_rxs {
                rx.await.unwrap();
            }

            // Release execution w
            release_slots
                .lock()
                .unwrap()
                .remove(&w)
                .unwrap()
                .send(())
                .unwrap();

            // Wait until execution w + 1 enters
            let next_e = entered_rx.recv().await.unwrap();
            assert_eq!(next_e, w + 1);
        }

        // While execution 6 is held and admissions are outstanding, initiate drain handshake
        hold_target_for_restore(&path);
        let held_drain_path = path.clone();
        let (drain_started_tx, drain_started_rx) = oneshot::channel();
        let drain = tokio::task::spawn_blocking(move || {
            let _ = drain_started_tx.send(());
            wait_for_target_use_to_end(&held_drain_path);
        });
        drain_started_rx.await.unwrap();
        assert!(!drain.is_finished());

        // Release execution 6
        release_slots
            .lock()
            .unwrap()
            .remove(&6)
            .unwrap()
            .send(())
            .unwrap();

        // Await all 50 retry tasks and drop bodies
        for (in_flight_w, task) in retry_tasks {
            let resp = task.await.unwrap();
            assert_eq!(resp.status(), StatusCode::OK);
            let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
                .await
                .unwrap();
            assert_eq!(body, format!("{}", in_flight_w + 1).as_bytes());
        }

        // Wait until every request (100 total) has delivered JoinEvent::Answered
        while answered_count.load(Ordering::SeqCst) < 100 {
            all_answered_notify.notified().await;
        }

        // Drain finishes now
        drain.await.unwrap();

        // Assert executions == 6
        assert_eq!(executions.load(Ordering::SeqCst), 6);

        // Assert exactly one Registered(Immediate)
        let regs = registrations.lock().unwrap();
        assert_eq!(regs.len(), 100);
        let immediate_count = regs
            .iter()
            .filter(|d| **d == Disposition::Immediate)
            .count();
        assert_eq!(immediate_count, 1);
        assert_eq!(regs[0], Disposition::Immediate);
        for d in &regs[1..] {
            assert_eq!(*d, Disposition::Queued);
        }
    }

    #[tokio::test]
    async fn unlisted_routes_are_gated_by_default() {
        let temporary = std::env::temp_dir().join(format!(
            "solstone-convey-shell-default-gate-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&temporary);
        std::fs::create_dir(&temporary).expect("temporary directory creates");
        let router = apply_layer(
            axum::Router::new().route("/throwaway", get(|| async { "reachable" })),
            temporary.clone(),
        );
        let response = router
            .oneshot(Request::get("/throwaway").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FOUND);
        assert_eq!(response.headers()["location"], "/init");
        let _ = std::fs::remove_dir_all(temporary);
    }

    #[test]
    fn exemption_inventory_is_named_and_closed() {
        assert_eq!(
            super::SESSION_GATE_EXEMPTIONS,
            [
                SessionExemption::Favicon,
                SessionExemption::TopLevelStatic,
                SessionExemption::InitSetup,
                SessionExemption::UnknownAppPrefix,
                SessionExemption::UnmatchedFallback,
            ]
        );
    }

    #[test]
    fn init_setup_exemption_is_the_init_prefix_only() {
        assert!(is_exempt("/init"));
        assert!(is_exempt("/init/"));
        assert!(is_exempt("/init/mark"));
        assert!(is_exempt("/init/api/state"));
        assert!(!is_exempt("/initfoo"));
        assert!(!is_exempt("/app/link/workspace"));
        assert!(!is_exempt("/app/devices"));
        assert!(!is_exempt("/app/devices/"));
        assert!(!is_exempt("/app/devices/workspace"));
        assert!(!is_exempt("/app/devices/ingest"));
        assert!(!is_exempt("/app/devices/ingest/segments/20260804"));
        assert!(!is_exempt("/app/news"));
        assert!(!is_exempt("/app/news/"));
        assert!(!is_exempt("/app/news/workspace"));
    }
}
