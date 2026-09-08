// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::panic::{self, AssertUnwindSafe};
use std::time::{Duration, Instant};

use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::json;
use solstone_core_system::process::{
    HelperAdmissionStatus, observe_bounded_helper_admission, retry_bounded_helper_admission_until,
};

use crate::{
    BackupWebDeps,
    operation::{self, Terminal},
};

pub(crate) fn readiness() -> &'static str {
    match observe_bounded_helper_admission() {
        HelperAdmissionStatus::Ready => "ready",
        HelperAdmissionStatus::Pending(_) => "pending",
        HelperAdmissionStatus::Contended => "contended",
    }
}

const CONFLICTING_MUTATIONS: &[&str] = &[
    "/app/backup/keys/generate",
    "/app/backup/confirm",
    "/app/backup/retention",
    "/app/backup/offload/config",
    "/app/backup/offload/enable",
    "/app/backup/offload/disable",
    "/app/backup/backup-now",
    "/app/backup/enable",
    "/app/backup/enable-hosted",
    "/app/backup/destination",
    "/app/backup/recovery-key/rotate",
    "/app/backup/teardown",
    "/app/backup/restore",
    "/app/backup/restore-hosted",
    "/app/backup/restore-hosted/prepare",
    "/app/backup/offload/restore",
];

pub(crate) async fn admit_mutation(
    operations: operation::SharedOperationSlot,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    if request.method() == axum::http::Method::POST
        && CONFLICTING_MUTATIONS.contains(&request.uri().path())
    {
        // This observation is a pre-handler refusal, not a reservation. The
        // helper facade still decides admission at the actual launch boundary.
        let pending = operation::observed_current(&operations).map_or(true, |current| {
            current.is_some_and(|current| current.phase == "cleanup_pending")
        });
        if pending || readiness() != "ready" {
            return operation::busy_response();
        }
    }
    next.run(request).await
}

pub(crate) fn spawn_worker<F>(mut deps: BackupWebDeps, generation: u64, work: F)
where
    F: FnOnce(BackupWebDeps) -> Terminal + Send + 'static,
{
    if !operation::start_worker(&deps.operations, generation) {
        operation::finish(
            &deps.operations,
            generation,
            "error",
            Some("failed".into()),
            None,
        );
        return;
    }
    let operations = deps.operations.clone();
    let failed_spawn = operations.clone();
    let (runner, cleanup) =
        solstone_core_backup_runtime::windows_cleanup::track_cleanup(deps.runner.clone());
    deps.runner = runner;
    let started = spawn_thread(move || {
        let terminal = panic::catch_unwind(AssertUnwindSafe(|| work(deps)))
            .unwrap_or_else(|_| Terminal::error("failed"));
        let owners = cleanup.try_iter().collect();
        operation::finish_worker(&operations, generation, terminal, owners);
    });
    if started.is_err() {
        operation::finish_worker(
            &failed_spawn,
            generation,
            Terminal::error("failed"),
            Vec::new(),
        );
    }
}

#[cfg(test)]
thread_local! {
    static FAIL_WORKER_START: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn spawn_thread(
    work: impl FnOnce() + Send + 'static,
) -> std::io::Result<std::thread::JoinHandle<()>> {
    #[cfg(test)]
    if FAIL_WORKER_START.with(|fail| fail.replace(false)) {
        return Err(std::io::Error::other(
            "injected backup worker creation failure",
        ));
    }
    std::thread::Builder::new().spawn(work)
}

pub(crate) async fn retry(deps: BackupWebDeps) -> Response {
    let deadline = Instant::now() + Duration::from_secs(2);
    // This recovery request selects only server-owned cleanup state. Its blocking
    // action is awaited; it does not schedule another backup or a retry loop.
    let mut task = tokio::task::spawn_blocking(move || retry_owned(&deps, deadline));
    match tokio::time::timeout_at(deadline.into(), &mut task).await {
        Ok(Ok(response)) => response,
        Ok(Err(_)) => operation::busy_response(),
        Err(_) => {
            // Abort prevents queued work from starting. An already-running
            // blocking call keeps the same internal deadline and original owner.
            task.abort();
            operation::busy_response()
        }
    }
}

fn retry_owned(deps: &BackupWebDeps, deadline: Instant) -> Response {
    let generation = {
        let Ok(guard) = deps.operations.try_lock() else {
            return operation::busy_response();
        };
        guard.as_ref().map(|slot| slot.generation)
    };
    let _ = retry_bounded_helper_admission_until(deadline);
    let current = match operation::observed_generation(&deps.operations, generation) {
        Ok(current) => current,
        Err(()) => return operation::busy_response(),
    };
    let state = readiness();
    let code = if state == "ready" {
        StatusCode::OK
    } else {
        StatusCode::ACCEPTED
    };
    (
        code,
        Json(json!({"operation": current, "cleanup_admission": state})),
    )
        .into_response()
}

#[cfg(test)]
#[path = "cleanup_native_tests.rs"]
mod native_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use solstone_core_convey_http::identity::AccessBasis;
    use solstone_core_convey_shell::{
        authorization_gate::authorized_router_with_router, session_gate,
    };
    use solstone_core_sol_link::{DeviceDoorAuthorization, ledger::AuthorizedClientsRead};
    use tower::ServiceExt;

    fn deps() -> (tempfile::TempDir, BackupWebDeps) {
        let root = crate::test_support::root("fresh");
        let deps = BackupWebDeps::production(
            root.path().to_path_buf(),
            crate::measurement::new(root.path()),
        );
        (root, deps)
    }

    #[test]
    fn failed_worker_creation_releases_only_its_active_slot() {
        let (_root, deps) = deps();
        let generation = operation::begin(&deps.operations, "restore", None, None, None)
            .unwrap()
            .generation;
        FAIL_WORKER_START.with(|fail| fail.set(true));
        spawn_worker(deps.clone(), generation, |_| {
            panic!("failed spawn must not run operation")
        });
        let current = operation::observed_current(&deps.operations)
            .unwrap()
            .unwrap();
        assert_eq!(current.phase, "error");
        assert_eq!(current.reason_code.as_deref(), Some("failed"));
        assert!(!operation::worker_or_cleanup_active(
            &deps.operations,
            generation
        ));
        assert!(operation::begin(&deps.operations, "restore", None, None, None).is_ok());
    }

    #[tokio::test]
    async fn cleanup_route_keeps_door_and_established_session_gates() {
        let (root, deps) = deps();
        let config_path = root.path().join("config/journal.json");
        let before = std::fs::read(&config_path).unwrap();
        let (_, authorization) = tokio::sync::watch::channel(DeviceDoorAuthorization::from(
            AuthorizedClientsRead::Missing,
        ));
        let router = authorized_router_with_router(
            session_gate::apply_layer(crate::routes_with_deps(deps), root.path().to_path_buf()),
            root.path().to_path_buf(),
            authorization,
        )
        .into_inner();
        let unauthenticated = Request::post("/app/backup/api/cleanup/retry")
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            router
                .clone()
                .oneshot(unauthenticated)
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
        let authorized = Request::post("/app/backup/api/cleanup/retry")
            .extension(AccessBasis::Localhost)
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            router.clone().oneshot(authorized).await.unwrap().status(),
            StatusCode::OK
        );
        assert_eq!(std::fs::read(&config_path).unwrap(), before);
        std::fs::remove_file(config_path).unwrap();
        let unestablished = Request::post("/app/backup/api/cleanup/retry")
            .extension(AccessBasis::Localhost)
            .body(Body::empty())
            .unwrap();
        assert_ne!(
            router.oneshot(unestablished).await.unwrap().status(),
            StatusCode::OK
        );
    }
}
