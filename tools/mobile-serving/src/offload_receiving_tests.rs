// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use super::*;
use crate::execution::offload_receiving;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::sync::{Mutex, atomic::AtomicUsize, mpsc};
use std::time::Duration;

const PATH: &str = "/api/journal/maintenance/backup:offload";

struct ClearHooks;
impl Drop for ClearHooks {
    fn drop(&mut self) {
        *offload_receiving::AFTER_SUBMIT
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = None;
        *offload_receiving::AFTER_RESULT
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = None;
        *offload_receiving::PUBLICATION_FAULT
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = None;
        *crate::foreground::scheduled_commands::OFFLOAD_RECEIVING_HOOK
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = None;
    }
}

fn results() -> mpsc::Receiver<String> {
    let (send, receive) = mpsc::channel();
    *offload_receiving::AFTER_RESULT.lock().unwrap() = Some(Arc::new(move |_, id| {
        let _ = send.send(id.to_owned());
    }));
    receive
}

fn calls() -> Arc<AtomicUsize> {
    let count = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&count);
    *crate::foreground::scheduled_commands::OFFLOAD_RECEIVING_HOOK
        .lock()
        .unwrap() = Some(Arc::new(move |_, _, _| {
        observed.fetch_add(1, Ordering::SeqCst);
    }));
    count
}

fn status(service: &crate::MobileServing, id: &str) -> Value {
    let (code, value) = request(
        service,
        "GET",
        &format!("{PATH}/requests/{id}"),
        json!(null),
    );
    assert_eq!(code, 200, "{value}");
    value
}

fn receipt(root: &std::path::Path, id: &str, phase: &str, exit: Value) {
    std::fs::write(root.join(format!(".owner-offload-{id}.{phase}.json")),json!({"version":1,"request_id":id,"reference":format!("owner-offload-request-{id}"),"phase":phase,"exit_code":exit}).to_string()).unwrap();
}

fn serving() -> (
    tempfile::TempDir,
    crate::MobileServing,
    crate::foreground::ForegroundProcessingHost,
) {
    let journal = tempfile::tempdir().unwrap();
    std::fs::create_dir(journal.path().join("config")).unwrap();
    std::fs::write(
        journal.path().join("config/journal.json"),
        json!({
            "identity":{"timezone":"UTC"},
            "providers":{"active":{"provider":"openai","model":"fixture"}},
            "processing":{"history_confirmed":true},
            "setup":{"completed_at":chrono::Utc::now().timestamp_millis()},
            "backup":{"enabled":true,"mode":"byo","offload":{"enabled":false}}
        })
        .to_string(),
    )
    .unwrap();
    let mut schedules = json!({"daily_time":"00:15","weekly_time":"03:15"});
    for name in [
        "heartbeat",
        "weekly-agents",
        "cadence",
        "brain",
        "facet-candidates",
        "rebuild-edges",
        "search-catch-all",
    ] {
        schedules[name] = json!({"enabled":false});
    }
    for routine in solstone_core_maintenance::registry::routines() {
        let mut entry = solstone_core_maintenance::schedule_sync::expected_entry(routine);
        entry["enabled"] = json!(false);
        schedules[solstone_core_maintenance::schedule_sync::schedule_name(routine.id)] = entry;
    }
    std::fs::write(
        journal.path().join("config/schedules.json"),
        schedules.to_string(),
    )
    .unwrap();
    let (service, host) = reopen(journal.path());
    (journal, service, host)
}

fn reopen(
    root: &std::path::Path,
) -> (
    crate::MobileServing,
    crate::foreground::ForegroundProcessingHost,
) {
    let probe = std::net::TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)).unwrap();
    let std::net::SocketAddr::V4(address) = probe.local_addr().unwrap() else {
        unreachable!()
    };
    drop(probe);
    let config: Value =
        serde_json::from_slice(&std::fs::read(root.join("config/journal.json")).unwrap()).unwrap();
    let address = SocketAddrV4::new(
        Ipv4Addr::LOCALHOST,
        config["pairing"]["direct_port"]
            .as_u64()
            .map(|port| u16::try_from(port).unwrap())
            .unwrap_or(address.port()),
    );
    let service = crate::MobileServing::start_with_payload(
        root.to_path_buf(),
        address,
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../core/payload"),
    )
    .unwrap();
    let payload = root.join("empty-payload");
    for dir in [
        "solstone/talent",
        "solstone/apps",
        "solstone/think/templates",
    ] {
        std::fs::create_dir_all(payload.join(dir)).unwrap();
    }
    let host = crate::foreground::ForegroundProcessingHost::open(
        root,
        &payload,
        solstone_core_backup_web::BackupWebDeps::new(root.to_path_buf()),
    )
    .unwrap();
    host.stop_discovery_for_test();
    (service, host)
}

fn request(service: &crate::MobileServing, method: &str, path: &str, body: Value) -> (u16, Value) {
    let reply = service
        .request_owner(method, path, &body.to_string())
        .unwrap();
    (
        reply.status,
        serde_json::from_str(&reply.body).unwrap_or(json!({"raw":reply.body})),
    )
}

#[test]
fn offload_receiving_lost_reply_recovers_same_identity() {
    let _serial = crate::foreground::TEST_LOCK
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let (_journal, service, host) = serving();
    let path = "/api/journal/maintenance/backup:offload";
    let body = json!({"request_id":"lost-reply-fixture"});
    let (first_status, first) = request(&service, "POST", path, body.clone());
    // The transport delivered the request. The caller loses its response and
    // must recover using only the identity it saved before dispatch.
    let (read_status, recovered) = request(
        &service,
        "GET",
        &format!("{path}/requests/lost-reply-fixture"),
        json!(null),
    );
    let (retry_status, retried) = request(&service, "POST", path, body);
    host.shutdown().unwrap();
    service.shutdown_for_retirement().unwrap();
    assert_eq!(first_status, 202, "{first}");
    assert_eq!(retry_status, 202, "{retried}");
    assert_eq!(
        read_status, 200,
        "keyed receiving lookup unavailable: {recovered}; first={first}; repeat={retried}"
    );
    assert_eq!(recovered["reference"], first["reference"]);
    assert_eq!(
        retried["reference"], first["reference"],
        "retry created a second identity"
    );
    assert_eq!(retried["reused"], true);
}

#[test]
fn offload_receiving_result_survives_restart_without_reexecution() {
    let _serial = crate::foreground::TEST_LOCK
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let _hooks = ClearHooks;
    let completed = results();
    let count = calls();
    let (journal, service, host) = serving();
    let config = std::fs::read(journal.path().join("config/journal.json")).unwrap();
    let (code, accepted) = request(&service, "POST", PATH, json!({"request_id":"restart"}));
    assert_eq!(code, 202, "{accepted}");
    assert_eq!(
        completed.recv_timeout(Duration::from_secs(10)).unwrap(),
        "restart"
    );
    assert_eq!(status(&service, "restart")["exit_code"], 0);
    host.shutdown().unwrap();
    service.shutdown_for_retirement().unwrap();
    drop(host);
    drop(service);
    let (service, host) = reopen(journal.path());
    let recovered = status(&service, "restart");
    let (code, retry) = request(&service, "POST", PATH, json!({"request_id":"restart"}));
    host.shutdown().unwrap();
    service.shutdown_for_retirement().unwrap();
    assert_eq!(recovered["state"], "finished");
    assert_eq!(recovered["reference"], accepted["reference"]);
    assert_eq!(recovered["exit_code"], 0);
    assert_eq!(code, 202, "{retry}");
    assert_eq!(retry["reused"], true);
    assert_eq!(
        count.load(Ordering::SeqCst),
        1,
        "restart/retry executed the leaf again"
    );
    assert_eq!(
        std::fs::read(journal.path().join("config/journal.json")).unwrap(),
        config
    );
}

#[test]
fn offload_receiving_uncertain_damaged_and_unconfirmed_records_never_replay() {
    use solstone_core_journal_io::atomic::BoundPublicationPrimitive;
    let _serial = crate::foreground::TEST_LOCK
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let _hooks = ClearHooks;
    let count = calls();
    let (journal, service, host) = serving();
    assert_eq!(status(&service, "absent")["state"], "unknown");
    assert!(
        !journal
            .path()
            .join(".owner-offload-absent.received.json")
            .exists()
    );
    for (id, accepted) in [("reserved", false), ("accepted-unknown", true)] {
        receipt(journal.path(), id, "received", json!(null));
        if accepted {
            receipt(journal.path(), id, "accepted", json!(null));
        }
        let (code, retry) = request(&service, "POST", PATH, json!({"request_id":id}));
        assert_eq!(code, 202, "{retry}");
        assert_eq!(retry["reused"], true);
        assert_eq!(
            retry["state"],
            if accepted {
                "outcome_unknown"
            } else {
                "admission_unknown"
            }
        );
        assert_eq!(retry["terminal"], false);
    }
    for phase in ["received", "accepted", "result"] {
        let id = format!("damaged-{phase}");
        receipt(journal.path(), &id, "received", json!(null));
        let file = journal
            .path()
            .join(format!(".owner-offload-{id}.{phase}.json"));
        std::fs::write(&file, b"{partial").unwrap();
        for method in ["POST", "GET"] {
            let path = if method == "GET" {
                format!("{PATH}/requests/{id}")
            } else {
                PATH.to_owned()
            };
            assert_eq!(
                request(&service, method, &path, json!({"request_id":id})).0,
                503
            );
        }
        assert_eq!(std::fs::read(file).unwrap(), b"{partial");
    }
    for (id, primitive) in [
        ("unconfirmed", BoundPublicationPrimitive::CompletionObserve),
        ("unsynced", BoundPublicationPrimitive::ParentSync),
    ] {
        let fired = Arc::new(AtomicBool::new(false));
        *offload_receiving::PUBLICATION_FAULT.lock().unwrap() =
            Some(offload_receiving::PublicationFault {
                root: journal.path().to_path_buf(),
                primitive,
                fired: Arc::clone(&fired),
            });
        let (code, value) = request(&service, "POST", PATH, json!({"request_id":id}));
        assert_eq!(code, 503, "{value}");
        assert!(
            fired.load(Ordering::Acquire),
            "native publisher fault was not exercised"
        );
        *offload_receiving::PUBLICATION_FAULT.lock().unwrap() = None;
        let (code, retry) = request(&service, "POST", PATH, json!({"request_id":id}));
        assert_eq!(code, 202, "{retry}");
        assert_eq!(retry["state"], "admission_unknown");
    }
    for body in [
        json!({}),
        json!({"request_id":"../escape"}),
        json!({"request_id":"safe","command":"other"}),
    ] {
        assert_eq!(request(&service, "POST", PATH, body).0, 400);
    }
    host.shutdown().unwrap();
    service.shutdown_for_retirement().unwrap();
    assert_eq!(count.load(Ordering::SeqCst), 0);
}

#[cfg(unix)]
#[test]
fn offload_receiving_actual_request_drop_after_admission_keeps_fast_result() {
    use std::os::fd::OwnedFd;
    let _serial = crate::foreground::TEST_LOCK
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let _hooks = ClearHooks;
    let completed = results();
    let count = calls();
    let (journal, service, host) = serving();
    let (entered, observed) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let released = Mutex::new(released);
    *offload_receiving::AFTER_SUBMIT.lock().unwrap() = Some(Arc::new(move |_, id| {
        entered.send(id.to_owned()).unwrap();
        released
            .lock()
            .unwrap()
            .recv_timeout(Duration::from_secs(10))
            .unwrap();
        Ok(())
    }));
    let (read, write) = std::os::unix::net::UnixStream::pair().unwrap();
    write.set_nonblocking(true).unwrap();
    let response = service
        .open_owner_response(
            41,
            "POST",
            PATH,
            &[],
            json!({"request_id":"disconnected"})
                .to_string()
                .into_bytes(),
            std::fs::File::from(OwnedFd::from(write)),
        )
        .unwrap();
    assert_eq!(
        observed.recv_timeout(Duration::from_secs(10)).unwrap(),
        "disconnected"
    );
    service.cancel_owner_response(41);
    assert!(
        matches!(
            response.recv_timeout(Duration::from_secs(10)),
            Err(mpsc::RecvTimeoutError::Disconnected)
        ),
        "actual HTTP request future was not dropped"
    );
    assert_eq!(
        completed.recv_timeout(Duration::from_secs(10)).unwrap(),
        "disconnected"
    );
    assert!(
        !journal
            .path()
            .join(".owner-offload-disconnected.accepted.json")
            .exists(),
        "completion must precede acceptance in this control"
    );
    release.send(()).unwrap();
    let recovered = status(&service, "disconnected");
    let (code, retry) = request(&service, "POST", PATH, json!({"request_id":"disconnected"}));
    drop(read);
    host.shutdown().unwrap();
    service.shutdown_for_retirement().unwrap();
    assert_eq!(recovered["state"], "finished");
    assert_eq!(recovered["exit_code"], 0);
    assert_eq!(code, 202, "{retry}");
    assert_eq!(retry["reused"], true);
    assert_eq!(retry["reference"], recovered["reference"]);
    assert_eq!(count.load(Ordering::SeqCst), 1);
}

#[test]
fn offload_receiving_coalesced_ids_each_keep_their_result() {
    let _serial = crate::foreground::TEST_LOCK
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let _hooks = ClearHooks;
    let completed = results();
    let (journal, service, host) = serving();
    let (entered, observed) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let released = Mutex::new(released);
    let count = Arc::new(AtomicUsize::new(0));
    let observed_count = Arc::clone(&count);
    *crate::foreground::scheduled_commands::OFFLOAD_RECEIVING_HOOK
        .lock()
        .unwrap() = Some(Arc::new(move |_, _, _| {
        if observed_count.fetch_add(1, Ordering::SeqCst) == 0 {
            entered.send(()).unwrap();
            released
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(10))
                .unwrap();
        }
    }));
    assert_eq!(
        request(&service, "POST", PATH, json!({"request_id":"blocker"})).0,
        202
    );
    observed.recv_timeout(Duration::from_secs(10)).unwrap();
    let (_, first) = request(&service, "POST", PATH, json!({"request_id":"follower-one"}));
    let (_, second) = request(&service, "POST", PATH, json!({"request_id":"follower-two"}));
    assert_ne!(first["reference"], second["reference"]);
    release.send(()).unwrap();
    let mut done = Vec::new();
    for _ in 0..3 {
        done.push(completed.recv_timeout(Duration::from_secs(10)).unwrap());
    }
    done.sort();
    assert_eq!(done, ["blocker", "follower-one", "follower-two"]);
    for id in ["follower-one", "follower-two"] {
        let recovered = status(&service, id);
        assert_eq!(recovered["state"], "finished");
        assert_eq!(recovered["exit_code"], 0);
        let (code, retry) = request(&service, "POST", PATH, json!({"request_id":id}));
        assert_eq!(code, 202, "{retry}");
        assert_eq!(retry["reused"], true);
        assert!(
            journal
                .path()
                .join(format!(".owner-offload-{id}.result.json"))
                .is_file()
        );
    }
    host.shutdown().unwrap();
    service.shutdown_for_retirement().unwrap();
    assert_eq!(
        count.load(Ordering::SeqCst),
        2,
        "queued followers must share one actual leaf after the blocker"
    );
}

#[test]
fn offload_receiving_stop_keeps_unstarted_outcome_unknown_without_replay() {
    let _serial = crate::foreground::TEST_LOCK
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let _hooks = ClearHooks;
    let completed = results();
    let (journal, service, host) = serving();
    let (entered, observed) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let released = Mutex::new(released);
    *crate::foreground::scheduled_commands::OFFLOAD_RECEIVING_HOOK
        .lock()
        .unwrap() = Some(Arc::new(move |_, _, _| {
        entered.send(()).unwrap();
        released
            .lock()
            .unwrap()
            .recv_timeout(Duration::from_secs(10))
            .unwrap();
    }));
    assert_eq!(
        request(&service, "POST", PATH, json!({"request_id":"stop-active"})).0,
        202
    );
    observed.recv_timeout(Duration::from_secs(10)).unwrap();
    assert_eq!(
        request(&service, "POST", PATH, json!({"request_id":"stop-queued"})).0,
        202
    );
    host.request_stop();
    service.stop_handle().request_stop();
    assert_eq!(
        request(&service, "POST", PATH, json!({"request_id":"after-stop"})).0,
        503
    );
    release.send(()).unwrap();
    host.shutdown().unwrap();
    service.shutdown_for_retirement().unwrap();
    assert_eq!(
        completed.recv_timeout(Duration::from_secs(10)).unwrap(),
        "stop-active"
    );
    assert!(
        !journal
            .path()
            .join(".owner-offload-stop-queued.result.json")
            .exists(),
        "unstarted shutdown work has no fabricated result"
    );
    assert!(
        !journal
            .path()
            .join(".owner-offload-after-stop.received.json")
            .exists()
    );
    drop(host);
    drop(service);
    let (service, host) = reopen(journal.path());
    let recovered = status(&service, "stop-queued");
    let (code, retry) = request(&service, "POST", PATH, json!({"request_id":"stop-queued"}));
    host.shutdown().unwrap();
    service.shutdown_for_retirement().unwrap();
    assert_eq!(recovered["state"], "outcome_unknown");
    assert_eq!(recovered["accepted"], true);
    assert_eq!(recovered["terminal"], false);
    assert_eq!(code, 202, "{retry}");
    assert_eq!(retry["reused"], true);
    assert!(
        completed.try_recv().is_err(),
        "old queued work ran after restart"
    );
}

#[test]
fn offload_receiving_admitted_before_acceptance_interruption_never_resubmits() {
    let _serial = crate::foreground::TEST_LOCK
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let _hooks = ClearHooks;
    let completed = results();
    let count = Arc::new(AtomicUsize::new(0));
    let observed_count = Arc::clone(&count);
    let (journal, service, host) = serving();
    let (entered, observed) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let released = Mutex::new(released);
    *crate::foreground::scheduled_commands::OFFLOAD_RECEIVING_HOOK
        .lock()
        .unwrap() = Some(Arc::new(move |_, _, _| {
        observed_count.fetch_add(1, Ordering::SeqCst);
        entered.send(()).unwrap();
        released
            .lock()
            .unwrap()
            .recv_timeout(Duration::from_secs(10))
            .unwrap();
    }));
    *offload_receiving::AFTER_SUBMIT.lock().unwrap() = Some(Arc::new(|_, _| Err(())));
    let (code, error) = request(
        &service,
        "POST",
        PATH,
        json!({"request_id":"interrupted-acceptance"}),
    );
    assert_eq!(code, 503, "{error}");
    observed.recv_timeout(Duration::from_secs(10)).unwrap();
    assert!(
        host.queue()
            .contains_reference("owner-offload-request-interrupted-acceptance")
    );
    let before = status(&service, "interrupted-acceptance");
    assert_eq!(before["state"], "admission_unknown");
    assert!(before["accepted"].is_null());
    assert_eq!(before["terminal"], false);
    assert!(
        !journal
            .path()
            .join(".owner-offload-interrupted-acceptance.accepted.json")
            .exists()
    );
    assert!(
        !journal
            .path()
            .join(".owner-offload-interrupted-acceptance.result.json")
            .exists()
    );
    let (code, retry) = request(
        &service,
        "POST",
        PATH,
        json!({"request_id":"interrupted-acceptance"}),
    );
    assert_eq!(code, 202, "{retry}");
    assert_eq!(retry["reused"], true);
    assert_eq!(retry["reference"], before["reference"]);
    host.request_stop();
    service.stop_handle().request_stop();
    release.send(()).unwrap();
    host.shutdown().unwrap();
    service.shutdown_for_retirement().unwrap();
    assert_eq!(
        completed.recv_timeout(Duration::from_secs(10)).unwrap(),
        "interrupted-acceptance"
    );
    drop(host);
    drop(service);
    let (service, host) = reopen(journal.path());
    let recovered = status(&service, "interrupted-acceptance");
    let (code, retry) = request(
        &service,
        "POST",
        PATH,
        json!({"request_id":"interrupted-acceptance"}),
    );
    host.shutdown().unwrap();
    service.shutdown_for_retirement().unwrap();
    assert_eq!(recovered["state"], "finished");
    assert_eq!(recovered["reference"], before["reference"]);
    assert_eq!(code, 202, "{retry}");
    assert_eq!(retry["reused"], true);
    assert_eq!(
        count.load(Ordering::SeqCst),
        1,
        "interrupted acceptance replayed the actual leaf"
    );
}
