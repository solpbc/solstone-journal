// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Execution ownership for service-consent handoffs beyond their HTTP response.

use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use solstone_core_journal_io::JournalRoot;
use solstone_core_journal_io::whole_journal_restore::{
    TargetUse, begin_target_use_bound, whole_journal_restore_blocks,
    whole_journal_restore_generation,
};
use solstone_core_thinking::confidential::{
    HandoffResult, OperationHandle, OperationRegistry, Phase,
};
use tokio::sync::{Mutex as AsyncMutex, oneshot, watch};
use tokio::task::JoinHandle;

use crate::recovery_watch::RecoveryWatch;

pub(crate) struct ServiceHandoffs {
    root: Arc<JournalRoot>,
    stop: Option<watch::Receiver<bool>>,
    tasks: AsyncMutex<Vec<OwnedHandoff>>,
}

struct OwnedHandoff {
    task: Option<JoinHandle<()>>,
    run: Arc<HandoffRun>,
}

pub(crate) struct HandoffRun {
    root: Arc<JournalRoot>,
    stop: Option<watch::Receiver<bool>>,
    operations: Arc<OperationRegistry>,
    service: &'static str,
    operation: OperationHandle,
    generation: u64,
    admission: Mutex<Option<Arc<TargetUse>>>,
    child: AsyncMutex<Option<JoinHandle<()>>>,
    recovery: RecoveryWatch,
}

pub(crate) fn config_confirmed(
    outcome: &solstone_core_journal_config_write::BoundConfigOutcome,
) -> bool {
    use solstone_core_journal_config_write::BoundConfigOutcome;
    match outcome {
        BoundConfigOutcome::Unchanged => true,
        BoundConfigOutcome::Published(p) => {
            p.durability_uncertainty.is_none()
                && p.observation_uncertainty.is_none()
                && p.confirmation_refusal.is_none()
        }
    }
}

pub(crate) fn interrupted() -> HandoffResult {
    HandoffResult {
        phase: Phase::Error,
        guidance: None,
        retryable: true,
        subscribe_url: None,
    }
}

fn stopped(stop: &Option<watch::Receiver<bool>>) -> bool {
    stop.as_ref()
        .is_some_and(|stop| *stop.borrow() || stop.has_changed().is_err())
}

async fn wait_stop(stop: &mut Option<watch::Receiver<bool>>) {
    let Some(stop) = stop else {
        return std::future::pending().await;
    };
    while !*stop.borrow() {
        if stop.changed().await.is_err() {
            return;
        }
    }
}

impl ServiceHandoffs {
    pub(crate) fn for_request(
        existing: Option<Arc<Self>>,
        admitted_root: Arc<JournalRoot>,
    ) -> Arc<Self> {
        existing.unwrap_or_else(|| Arc::new(Self::new(admitted_root, None)))
    }

    pub(crate) fn new(root: Arc<JournalRoot>, stop: Option<watch::Receiver<bool>>) -> Self {
        Self {
            root,
            stop,
            tasks: AsyncMutex::new(Vec::new()),
        }
    }

    pub(crate) fn root(&self) -> Arc<JournalRoot> {
        Arc::clone(&self.root)
    }

    pub(crate) async fn spawn<F, W>(
        self: &Arc<Self>,
        service: &'static str,
        operations: Arc<OperationRegistry>,
        operation: OperationHandle,
        work: W,
    ) where
        W: FnOnce(Arc<HandoffRun>) -> F + Send + 'static,
        F: Future<Output = HandoffResult> + Send + 'static,
    {
        // Registration and joins share this lock. Stop is already published by
        // the app before it waits for the lock, so it cannot miss a late task.
        let mut tasks = self.tasks.lock().await;
        let mut index = 0;
        while index < tasks.len() {
            if tasks[index]
                .task
                .as_ref()
                .is_none_or(JoinHandle::is_finished)
            {
                tasks[index].join().await;
                tasks.remove(index);
            } else {
                index += 1;
            }
        }
        if stopped(&self.stop) {
            operations.finish(service, operation, interrupted());
            return;
        }
        let root = Arc::clone(&self.root);
        let admitted = tokio::task::spawn_blocking(move || {
            let lease = begin_target_use_bound(Arc::clone(&root))?;
            let generation = whole_journal_restore_generation(root.canonical_path());
            Ok::<_, solstone_core_journal_io::whole_journal_restore::TargetUseRefused>((
                lease, generation,
            ))
        })
        .await;
        let Ok(Ok((lease, generation))) = admitted else {
            operations.finish(service, operation, interrupted());
            return;
        };
        let run = Arc::new(HandoffRun {
            root: Arc::clone(&self.root),
            stop: self.stop.clone(),
            operations,
            service,
            operation,
            generation,
            admission: Mutex::new(Some(Arc::new(lease))),
            child: AsyncMutex::new(None),
            recovery: RecoveryWatch::new(
                &format!("service-consent-{service}"),
                Arc::clone(&self.root),
            ),
        });
        let owned = Arc::clone(&run);
        let lifetime = Arc::clone(self);
        let task = tokio::spawn(async move {
            let _lifetime = lifetime;
            if owned.cancelled() {
                owned.settle(interrupted()).await;
                return;
            }
            let evaluation = tokio::spawn(work(Arc::clone(&owned)));
            let result = evaluation.await.unwrap_or_else(|_| interrupted());
            owned.settle(result).await;
        });
        tasks.push(OwnedHandoff {
            task: Some(task),
            run,
        });
    }

    pub(crate) async fn join(&self) {
        let mut tasks = self.tasks.lock().await;
        while let Some(task) = tasks.first_mut() {
            task.join().await;
            tasks.remove(0);
        }
    }
}

impl OwnedHandoff {
    async fn join(&mut self) {
        if let Some(task) = self.task.as_mut() {
            let _ = task.await;
            self.task.take();
        }
        // A failed supervisor still retains its child and admission in this
        // record. A cancelled join waiter leaves this record in the owner.
        let pending = self
            .run
            .admission
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .is_some();
        if pending {
            self.run.settle(interrupted()).await;
        }
    }
}

impl HandoffRun {
    pub(crate) fn root(&self) -> Arc<JournalRoot> {
        Arc::clone(&self.root)
    }

    pub(crate) fn journal(&self) -> &std::path::Path {
        self.root.canonical_path()
    }

    pub(crate) fn cancelled(&self) -> bool {
        stopped(&self.stop)
            || !self.operations.is_open(self.service, self.operation)
            || self.root.revalidate_canonical_binding().is_err()
            || whole_journal_restore_blocks(self.root.canonical_path())
            || whole_journal_restore_generation(self.root.canonical_path()) != self.generation
    }

    pub(crate) async fn sleep(&self, duration: Duration) -> bool {
        let mut changes = self.recovery.subscribe();
        let mut stop = self.stop.clone();
        if self.cancelled() {
            return false;
        }
        tokio::select! {
            biased;
            _ = self.operations.wait_until_closed(self.service, self.operation) => false,
            _ = wait_stop(&mut stop) => false,
            _ = changes.changed() => !self.cancelled(),
            _ = tokio::time::sleep(duration) => !self.cancelled(),
        }
    }

    pub(crate) async fn blocking<T, F>(&self, work: F) -> Result<T, ()>
    where
        T: Send + 'static,
        F: FnOnce() -> T + Send + 'static,
    {
        let mut child = self.child.lock().await;
        if self.cancelled() {
            return Err(());
        }
        let admission = self
            .admission
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
            .cloned()
            .ok_or(())?;
        let (sender, receiver) = oneshot::channel();
        *child = Some(tokio::task::spawn_blocking(move || {
            let _admission = admission;
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(work));
            let _ = sender.send(result);
        }));
        let result = receiver.await;
        let joined = child.as_mut().expect("registered blocking child").await;
        child.take();
        if joined.is_err() {
            return Err(());
        }
        result.map_err(|_| ())?.map_err(|_| ())
    }

    async fn settle(&self, result: HandoffResult) {
        let mut child = self.child.lock().await;
        if let Some(task) = child.as_mut() {
            let _ = task.await;
            child.take();
        }
        drop(child);
        self.operations.finish(self.service, self.operation, result);
        self.admission
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take();
    }
}

#[cfg(all(test, unix, feature = "full-tests"))]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    struct HeldPortal {
        entered: Mutex<Option<oneshot::Sender<()>>>,
        release: Mutex<std::sync::mpsc::Receiver<()>>,
        calls: std::sync::atomic::AtomicUsize,
        enrollments: std::sync::atomic::AtomicUsize,
    }

    impl HeldPortal {
        fn poll(&self) {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let entered = self.entered.lock().unwrap().take();
            if let Some(entered) = entered {
                entered.send(()).unwrap();
                self.release
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(5))
                    .unwrap();
            }
        }
    }

    impl crate::SmePoll for HeldPortal {
        fn poll(&self, base: &str, _: &str) -> crate::SmePollOutcome {
            assert_eq!(base, "https://synthetic.invalid");
            self.poll();
            crate::SmePollOutcome::Success(
                serde_json::json!({
                    "service":"sme", "state":"approved", "approved_at":"2026-10-26T00:00:00Z"
                })
                .as_object()
                .unwrap()
                .clone(),
            )
        }
    }

    impl crate::SplPoll for HeldPortal {
        fn poll(&self, base: &str, _: &str) -> crate::SplPollOutcome {
            assert_eq!(base, "https://synthetic.invalid");
            self.poll();
            crate::SplPollOutcome::Success(
                serde_json::json!({
                    "service":"spl", "state":"approved", "approved_at":"2026-10-26T00:00:00Z"
                })
                .as_object()
                .unwrap()
                .clone(),
            )
        }
    }

    impl crate::SplEnrollment for HeldPortal {
        fn enroll(
            &self,
            _: &std::path::Path,
            _: &solstone_core_sol_link::service_identity::ServiceIdentity,
            _: &str,
        ) -> Result<String, solstone_core_spl::EnrollError> {
            self.enrollments.fetch_add(1, Ordering::SeqCst);
            Ok("synthetic-embedded-service-token".to_owned())
        }
    }

    impl crate::ConfidentialPoll for HeldPortal {
        fn poll(&self, base: &str, _: &str) -> crate::PollOutcome {
            assert_eq!(base, "https://synthetic.invalid");
            self.poll();
            crate::PollOutcome::Success(
                serde_json::json!({
                    "endpoint_url":"https://synthetic.invalid/v1", "served_model_id":"synthetic",
                    "credential":"synthetic-embedded-credential", "account_id":"synthetic",
                    "created_at":"2026-10-26T00:00:00Z"
                })
                .as_object()
                .unwrap()
                .clone(),
            )
        }
    }

    async fn embedded_fixture(
        root: &std::path::Path,
        portal: Arc<HeldPortal>,
    ) -> crate::EmbeddedConveyHandle {
        use axum::Extension;
        let root = root.to_owned();
        std::fs::create_dir(root.join("config")).unwrap();
        std::fs::write(
            root.join("config/journal.json"),
            b"{\"setup\":{\"completed_at\":1}}\n",
        )
        .unwrap();
        let backup = solstone_core_backup_web::BackupWebDeps::new(root.clone());
        let app = crate::router_with_roots(
            root.clone(),
            None,
            Some(solstone_core_mcp_endpoint::owner_routes(root.clone())),
            None,
            Some(backup.clone()),
            None,
        )
        .layer(Extension(crate::SmeRuntimeOverride {
            portal_base_url: "https://synthetic.invalid".to_owned(),
            poll: portal.clone(),
        }))
        .layer(Extension(crate::SplRuntimeOverride {
            portal_base_url: "https://synthetic.invalid".to_owned(),
            poll: portal.clone(),
            enrollment: portal.clone(),
            subscribe_poll: Duration::from_secs(3600),
        }))
        .layer(Extension(crate::ConfidentialRuntimeOverride {
            portal_base_url: "https://synthetic.invalid".to_owned(),
            poll: portal,
            before_attempt: None,
            now: Arc::new(std::time::Instant::now),
            sleep: Arc::new(std::thread::sleep),
            access: None,
        }));
        crate::bind_embedded_router(
            root,
            std::net::SocketAddrV4::new(std::net::Ipv4Addr::LOCALHOST, 0),
            app,
            backup,
            None,
        )
        .await
        .unwrap()
    }

    async fn owner_request(
        owner: &crate::EmbeddedConveyHandle,
        method: &str,
        path: &str,
        body: &str,
    ) -> (axum::http::StatusCode, serde_json::Value) {
        let request = axum::http::Request::builder()
            .method(method)
            .uri(path)
            .header("content-type", "application/json")
            .body(axum::body::Body::from(body.to_owned()))
            .unwrap();
        let response = owner.request_owner(request).await;
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        (
            status,
            serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null),
        )
    }

    fn enabled(root: &std::path::Path, service: &str) -> bool {
        let config: serde_json::Value =
            serde_json::from_slice(&std::fs::read(root.join("config/journal.json")).unwrap())
                .unwrap();
        match service {
            "sme" => config["mcp_endpoint"]["enabled"] == true,
            "spl" => config["link"]["posture"] == "spl",
            "spp" => config["services"]["confidential"].is_object(),
            _ => unreachable!(),
        }
    }

    #[tokio::test]
    async fn actual_embedded_services_join_beyond_accepted_body_and_resume_after_restore() {
        use solstone_core_journal_io::whole_journal_restore::{
            WholeJournalRestoreSource, clear_whole_journal_restore, publish_whole_journal_restore,
        };
        for (service, path) in [
            ("sme", "/app/agents/api/enable"),
            ("spl", "/app/network/private-link/enable"),
            ("spp", "/app/thinking/api/confidential/enable"),
        ] {
            for restore in [false, true] {
                let temporary = tempfile::tempdir().unwrap();
                let (entered, waiting) = oneshot::channel();
                let (release, held) = std::sync::mpsc::channel();
                let portal = Arc::new(HeldPortal {
                    entered: Mutex::new(Some(entered)),
                    release: Mutex::new(held),
                    calls: 0.into(),
                    enrollments: 0.into(),
                });
                let owner = embedded_fixture(temporary.path(), Arc::clone(&portal)).await;
                let (status, body) = owner_request(&owner, "POST", path, "").await;
                assert_eq!(
                    status,
                    axum::http::StatusCode::ACCEPTED,
                    "{service}: {body}"
                );
                tokio::time::timeout(Duration::from_secs(5), waiting)
                    .await
                    .unwrap()
                    .unwrap();
                if restore {
                    let pending = publish_whole_journal_restore(
                        temporary.path(),
                        &WholeJournalRestoreSource::Byo {
                            repository: "synthetic".to_owned(),
                            backend: "local".to_owned(),
                        },
                        &"b".repeat(64),
                        "/",
                    )
                    .unwrap();
                    let journal = temporary.path().to_owned();
                    let mut drain = tokio::task::spawn_blocking(move || {
                        crate::recovery_drain::relinquish_admitted_work(&journal)
                    });
                    assert!(
                        tokio::time::timeout(Duration::from_millis(30), &mut drain)
                            .await
                            .is_err()
                    );
                    release.send(()).unwrap();
                    tokio::time::timeout(Duration::from_secs(5), drain)
                        .await
                        .unwrap()
                        .unwrap()
                        .unwrap();
                    owner.service_handoffs.join().await;
                    assert!(!enabled(temporary.path(), service));
                    clear_whole_journal_restore(temporary.path(), &pending).unwrap();
                    let journal = temporary.path().to_owned();
                    tokio::task::spawn_blocking(move || {
                        crate::recovery_drain::resume_after_whole_journal_restore(&journal)
                    })
                    .await
                    .unwrap();
                    let (status, body) = owner_request(&owner, "POST", path, "").await;
                    assert_eq!(
                        status,
                        axum::http::StatusCode::ACCEPTED,
                        "{service}: {body}"
                    );
                    tokio::time::timeout(Duration::from_secs(5), owner.service_handoffs.join())
                        .await
                        .unwrap();
                    assert!(
                        enabled(temporary.path(), service),
                        "fresh {service} consent can publish after recovery"
                    );
                    assert_eq!(portal.calls.load(Ordering::SeqCst), 2);
                    owner.shutdown_and_wait().await;
                } else {
                    assert!(
                        tokio::time::timeout(Duration::from_millis(30), owner.shutdown_and_wait())
                            .await
                            .is_err()
                    );
                    assert!(!enabled(temporary.path(), service));
                    release.send(()).unwrap();
                    tokio::time::timeout(Duration::from_secs(5), owner.shutdown_and_wait())
                        .await
                        .unwrap();
                    assert!(!enabled(temporary.path(), service));
                    assert_eq!(portal.calls.load(Ordering::SeqCst), 1);
                }
                assert_eq!(
                    portal.enrollments.load(Ordering::SeqCst),
                    usize::from(restore && service == "spl")
                );
            }
        }
    }

    #[tokio::test]
    async fn ordinary_spl_token_refusal_keeps_partial_posture_and_reports_repair_state() {
        let temporary = tempfile::tempdir().unwrap();
        let (entered, waiting) = oneshot::channel();
        let (release, held) = std::sync::mpsc::channel();
        let portal = Arc::new(HeldPortal {
            entered: Mutex::new(Some(entered)),
            release: Mutex::new(held),
            calls: 0.into(),
            enrollments: 0.into(),
        });
        let owner = embedded_fixture(temporary.path(), Arc::clone(&portal)).await;
        assert_eq!(
            owner_request(&owner, "POST", "/app/network/private-link/enable", "")
                .await
                .0,
            axum::http::StatusCode::ACCEPTED
        );
        waiting.await.unwrap();
        std::fs::write(
            temporary.path().join("link/tokens"),
            b"blocked-token-parent",
        )
        .unwrap();
        release.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(5), owner.service_handoffs.join())
            .await
            .unwrap();
        let (status, body) =
            owner_request(&owner, "GET", "/app/network/api/private-link", "").await;
        assert_eq!(status, axum::http::StatusCode::OK);
        assert_eq!(body["state"], "inconsistent");
        assert_eq!(body["operation"]["phase"], "error");
        assert_eq!(body["enrolled"], false);
        assert!(
            enabled(temporary.path(), "spl"),
            "already-published posture is retained"
        );
        assert_eq!(
            std::fs::read(temporary.path().join("link/tokens")).unwrap(),
            b"blocked-token-parent"
        );
        assert_eq!(portal.calls.load(Ordering::SeqCst), 1);
        assert_eq!(portal.enrollments.load(Ordering::SeqCst), 1);
        owner.shutdown_and_wait().await;
    }

    #[tokio::test]
    async fn ordinary_embedded_disable_cancels_each_actual_registry_and_rejects_old_approval() {
        for (service, enable, method, disable, body) in [
            (
                "sme",
                "/app/agents/api/enable",
                "PUT",
                "/app/agents/api/capability",
                "{\"enabled\":false}",
            ),
            (
                "spl",
                "/app/network/private-link/enable",
                "POST",
                "/app/network/private-link/disable",
                "",
            ),
            (
                "spp",
                "/app/thinking/api/confidential/enable",
                "POST",
                "/app/thinking/api/confidential/disable",
                "",
            ),
        ] {
            let temporary = tempfile::tempdir().unwrap();
            let (entered, waiting) = oneshot::channel();
            let (release, held) = std::sync::mpsc::channel();
            let portal = Arc::new(HeldPortal {
                entered: Mutex::new(Some(entered)),
                release: Mutex::new(held),
                calls: 0.into(),
                enrollments: 0.into(),
            });
            let owner = embedded_fixture(temporary.path(), Arc::clone(&portal)).await;
            assert_eq!(
                owner_request(&owner, "POST", enable, "").await.0,
                axum::http::StatusCode::ACCEPTED
            );
            tokio::time::timeout(Duration::from_secs(5), waiting)
                .await
                .unwrap()
                .unwrap();
            let (status, body) = owner_request(&owner, method, disable, body).await;
            assert_eq!(status, axum::http::StatusCode::OK, "{service}: {body}");
            assert!(!enabled(temporary.path(), service));
            assert!(
                tokio::time::timeout(Duration::from_millis(30), owner.service_handoffs.join())
                    .await
                    .is_err()
            );
            release.send(()).unwrap();
            tokio::time::timeout(Duration::from_secs(5), owner.service_handoffs.join())
                .await
                .unwrap();
            assert!(
                !enabled(temporary.path(), service),
                "stale approved {service} result cannot undo disable"
            );
            assert_eq!(portal.calls.load(Ordering::SeqCst), 1);
            assert_eq!(portal.enrollments.load(Ordering::SeqCst), 0);
            assert_eq!(
                owner_request(&owner, "POST", enable, "").await.0,
                axum::http::StatusCode::ACCEPTED
            );
            tokio::time::timeout(Duration::from_secs(5), owner.service_handoffs.join())
                .await
                .unwrap();
            assert!(
                enabled(temporary.path(), service),
                "fresh {service} consent can publish after disable"
            );
            assert_eq!(portal.calls.load(Ordering::SeqCst), 2);
            owner.shutdown_and_wait().await;
        }
    }

    #[tokio::test]
    async fn evaluation_panic_keeps_blocking_destructor_and_cancelled_join_owned() {
        struct Finished(Arc<AtomicBool>);
        impl Drop for Finished {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }
        let temporary = tempfile::tempdir().unwrap();
        let root = Arc::new(JournalRoot::open(temporary.path()).unwrap());
        let (stop, receiver) = watch::channel(false);
        let owner = Arc::new(ServiceHandoffs::new(root, Some(receiver)));
        let operations = Arc::new(OperationRegistry::default());
        let (handle, _) = operations.start_operation("sme", "enable", None).unwrap();
        let (entered, waiting) = oneshot::channel();
        let (release, held) = std::sync::mpsc::channel();
        let finished = Arc::new(AtomicBool::new(false));
        let destroyed = Arc::clone(&finished);
        let (panicked, observed_panic) = oneshot::channel();
        owner
            .spawn(
                "sme",
                Arc::clone(&operations),
                handle,
                move |run| async move {
                    let blocking = run.blocking(move || {
                        let _finished = Finished(destroyed);
                        entered.send(()).unwrap();
                        held.recv().unwrap();
                    });
                    let panic = async {
                        waiting.await.unwrap();
                        panicked.send(()).unwrap();
                        panic!("synthetic evaluation failure while native work is held");
                    };
                    tokio::join!(blocking, panic);
                    interrupted()
                },
            )
            .await;
        observed_panic.await.unwrap();
        stop.send_replace(true);
        assert!(
            tokio::time::timeout(Duration::from_millis(30), owner.join())
                .await
                .is_err()
        );
        assert!(!finished.load(Ordering::SeqCst));
        assert!(operations.is_open("sme", handle));
        release.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(5), owner.join())
            .await
            .unwrap();
        assert!(finished.load(Ordering::SeqCst));
        assert!(!operations.is_open("sme", handle));
        assert_eq!(operations.operation_raw("sme")["phase"], "error");
    }

    #[tokio::test]
    async fn operation_cancel_wakes_registered_sleep_without_rearming_it() {
        let temporary = tempfile::tempdir().unwrap();
        let root = Arc::new(JournalRoot::open(temporary.path()).unwrap());
        let owner = Arc::new(ServiceHandoffs::new(root, None));
        let operations = Arc::new(OperationRegistry::default());
        let (handle, _) = operations.start_operation("spl", "enable", None).unwrap();
        let (entered, waiting) = oneshot::channel();
        let (slept, result) = oneshot::channel();
        owner
            .spawn(
                "spl",
                Arc::clone(&operations),
                handle,
                move |run| async move {
                    let mut sleep = Box::pin(run.sleep(Duration::from_secs(3600)));
                    std::future::poll_fn(|context| {
                        assert!(sleep.as_mut().poll(context).is_pending());
                        std::task::Poll::Ready(())
                    })
                    .await;
                    entered.send(()).unwrap();
                    slept.send(sleep.await).unwrap();
                    interrupted()
                },
            )
            .await;
        waiting.await.unwrap();
        operations.cancel("spl", interrupted());
        assert!(
            !tokio::time::timeout(Duration::from_secs(1), result)
                .await
                .unwrap()
                .unwrap()
        );
        owner.join().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn target_use_survives_actual_terminal_publication_and_its_notifications() {
        struct HeldWake {
            entered: Mutex<Option<oneshot::Sender<()>>>,
            release: Mutex<std::sync::mpsc::Receiver<()>>,
        }
        impl std::task::Wake for HeldWake {
            fn wake(self: Arc<Self>) {
                if let Some(entered) = self.entered.lock().unwrap().take() {
                    entered.send(()).unwrap();
                    self.release
                        .lock()
                        .unwrap()
                        .recv_timeout(Duration::from_secs(5))
                        .unwrap();
                }
            }
        }
        let temporary = tempfile::tempdir().unwrap();
        let root = Arc::new(JournalRoot::open(temporary.path()).unwrap());
        let owner = Arc::new(ServiceHandoffs::new(root, None));
        let operations = Arc::new(OperationRegistry::default());
        let (handle, _) = operations.start_operation("sme", "enable", None).unwrap();
        let (entered, waiting) = oneshot::channel();
        let (release, held) = std::sync::mpsc::channel();
        let waker = std::task::Waker::from(Arc::new(HeldWake {
            entered: Mutex::new(Some(entered)),
            release: Mutex::new(held),
        }));
        let mut terminal = Box::pin(operations.wait_until_closed("sme", handle));
        assert!(
            terminal
                .as_mut()
                .poll(&mut std::task::Context::from_waker(&waker))
                .is_pending()
        );
        owner
            .spawn("sme", Arc::clone(&operations), handle, |_| async {
                interrupted()
            })
            .await;
        waiting.await.unwrap();
        let journal = temporary.path().to_owned();
        let mut drain = tokio::task::spawn_blocking(move || {
            solstone_core_journal_io::whole_journal_restore::wait_for_target_use_to_end(&journal)
        });
        assert!(
            tokio::time::timeout(Duration::from_millis(30), &mut drain)
                .await
                .is_err()
        );
        release.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(5), drain)
            .await
            .unwrap()
            .unwrap();
        owner.join().await;
        assert!(!operations.is_open("sme", handle));
    }
}
