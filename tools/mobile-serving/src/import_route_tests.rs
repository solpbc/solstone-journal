// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use super::*;
use crate::MobileServing;
use crate::foreground::PreparedImport;
use solstone_core_import_sources::document::{
    PdfPayload, PdfWorker, PdfWorkerRequest, WorkerFailure,
};
use std::fs;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::Duration;

fn in_isolated_child(name: &str) -> bool {
    in_isolated_child_exact(&format!("import_route::tests::{name}"))
}

pub(super) fn in_isolated_child_exact(full_name: &str) -> bool {
    const CHILD: &str = "FC2_RUNTIME_IMPORT_CONTROL_CHILD";
    if std::env::var(CHILD).ok().as_deref() == Some(full_name) {
        return false;
    }
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", full_name, "--nocapture"])
        .env(CHILD, full_name)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        if std::time::Instant::now() >= deadline {
            child.kill().unwrap();
            let output = child.wait_with_output().unwrap();
            panic!(
                "retirement control did not settle: {}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let output = child.wait_with_output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success()
            && stdout.contains(full_name)
            && stdout.contains("1 passed; 0 failed"),
        "{}{}",
        stdout,
        String::from_utf8_lossy(&output.stderr)
    );
    true
}

pub(super) fn fixture(
    factory: Arc<crate::foreground::ImportSeamFactory>,
) -> (
    tempfile::TempDir,
    Arc<MobileServing>,
    Arc<ForegroundProcessingHost>,
) {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir_all(root.path().join("config")).unwrap();
    fs::write(root.path().join("config/journal.json"),r#"{"setup":{"completed_at":1},"identity":{"timezone":"UTC"},"processing":{"history_confirmed":true}}"#).unwrap();
    let payload = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../core/payload");
    let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = match probe.local_addr().unwrap() {
        std::net::SocketAddr::V4(address) => address,
        _ => unreachable!(),
    };
    drop(probe);
    let server = Arc::new(
        MobileServing::start_with_payload(root.path().to_path_buf(), address, payload.clone())
            .unwrap(),
    );
    let host = Arc::new(
        ForegroundProcessingHost::open_with_imports(
            Arc::clone(&server.root),
            &payload,
            Arc::new(|| false),
            Arc::new(AtomicBool::new(false)),
            server.backup_dependencies(),
            factory,
            false,
            false,
        )
        .unwrap(),
    );
    host.stop_discovery_for_test();
    server
        .import_route()
        .install(Arc::new(ForegroundImportRoute::new(Arc::clone(&host))))
        .unwrap();
    (root, server, host)
}

#[test]
fn retained_stop_and_router_do_not_retain_the_retired_foreground_host() {
    if in_isolated_child("retained_stop_and_router_do_not_retain_the_retired_foreground_host") {
        return;
    }
    let _serial = crate::foreground::TEST_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let (_root, server, host) = fixture(Arc::new(|_| panic!("no import is requested")));
    let weak = Arc::downgrade(&host);
    let retained_stop = server.stop_handle();
    host.shutdown().unwrap();
    server.shutdown_for_retirement().unwrap();
    drop(host);
    assert!(
        weak.upgrade().is_none(),
        "the retained router/Stop still owns the foreground host"
    );
    assert!(retained_stop.is_stopping());
    retained_stop.request_stop();
    drop(server);
    assert!(weak.upgrade().is_none());
    drop(retained_stop);
}

struct TestResource {
    owner: std::thread::ThreadId,
    dropped: Arc<AtomicBool>,
}
impl Drop for TestResource {
    fn drop(&mut self) {
        assert_eq!(std::thread::current().id(), self.owner);
        self.dropped.store(true, Ordering::Release);
    }
}
struct HeldPdf {
    entered: mpsc::Sender<WorkStop>,
    release: Arc<Mutex<mpsc::Receiver<()>>>,
}
impl PdfWorker for HeldPdf {
    fn execute(&self, _: &PdfWorkerRequest) -> Result<PdfPayload, WorkerFailure> {
        self.entered
            .send(solstone_core_import::installed_stop().expect("installed native Stop"))
            .unwrap();
        self.release
            .lock()
            .unwrap()
            .recv_timeout(Duration::from_secs(10))
            .unwrap();
        assert!(!solstone_core_import::work_continues());
        Err(WorkerFailure::Cancelled)
    }
}

fn blocked_resource(preparation: bool) {
    let (entered_send, entered) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let released = Arc::new(Mutex::new(released));
    let dropped = Arc::new(AtomicBool::new(false));
    let observed = Arc::clone(&dropped);
    let (root, server, host) = fixture(Arc::new(move |context| {
        let resource = Rc::new(TestResource {
            owner: std::thread::current().id(),
            dropped: Arc::clone(&observed),
        });
        let pdf: Box<dyn PdfWorker + Send> = if preparation {
            entered_send.send(context.stop().clone()).unwrap();
            released
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(10))
                .unwrap();
            Box::new(solstone_core_import_sources::NullPdfWorker)
        } else {
            Box::new(HeldPdf {
                entered: entered_send.clone(),
                release: Arc::clone(&released),
            })
        };
        let seams = context.seams_with_pdf(pdf);
        Ok(PreparedImport::new(context.arguments().to_vec(), seams).with_resources(resource))
    }));
    let pdf = root.path().join("held.pdf");
    fs::write(&pdf, b"%PDF-1.1\n").unwrap();
    let slot = server.import_route();
    let captured = slot
        .capture(
            vec![
                "solstone".into(),
                "journal".into(),
                "importer".into(),
                pdf.to_str().unwrap().into(),
                "--source".into(),
                "document".into(),
                "--save".into(),
                "--timestamp".into(),
                "20260809_090000".into(),
            ],
            0,
        )
        .unwrap();
    let (outcome_send, outcome) = mpsc::channel();
    drop(outcome); // Delivery loss never detaches the retained route worker.
    slot.run_retained(captured, move |result| {
        let _ = outcome_send.send(result);
    })
    .unwrap();
    let native_stop = entered.recv_timeout(Duration::from_secs(10)).unwrap();
    let retained_stop = server.stop_handle();
    retained_stop.request_stop();
    assert!(
        native_stop.is_requested(),
        "cloneable Stop must signal imports before any join"
    );
    assert!(!dropped.load(Ordering::Acquire));
    let retiring = Arc::clone(&server);
    let retiring_host = Arc::clone(&host);
    let (done_send, done) = mpsc::channel();
    assert!(!solstone_core_journal_io::whole_journal_restore::target_held_for_restore(root.path()));
    let retirement = std::thread::spawn(move || {
        retiring_host.shutdown().unwrap();
        done_send.send(retiring.shutdown_for_retirement()).unwrap();
    });
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while !solstone_core_journal_io::whole_journal_restore::target_held_for_restore(root.path()) {
        assert!(
            matches!(done.try_recv(), Err(mpsc::TryRecvError::Empty)),
            "retirement finished or disconnected before entering the target hold"
        );
        assert!(
            std::time::Instant::now() < deadline,
            "serving retirement never entered its maintained target hold"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(
        matches!(
            done.recv_timeout(Duration::from_millis(50)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ),
        "retirement acknowledged or disconnected with a still-held worker"
    );
    assert!(!dropped.load(Ordering::Acquire));
    release.send(()).unwrap();
    done.recv_timeout(Duration::from_secs(10)).unwrap().unwrap();
    retirement.join().unwrap();
    assert!(
        dropped.load(Ordering::Acquire),
        "the original worker must retire its resource before acknowledgment"
    );
    host.shutdown().unwrap();
    drop(host);
    drop(server);
    drop(retained_stop);
}

#[test]
fn stop_and_disconnected_delivery_wait_for_factory_resources() {
    if in_isolated_child("stop_and_disconnected_delivery_wait_for_factory_resources") {
        return;
    }
    let _serial = crate::foreground::TEST_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    blocked_resource(true);
}
#[test]
fn stop_and_disconnected_delivery_wait_for_joined_pdf_execution() {
    if in_isolated_child("stop_and_disconnected_delivery_wait_for_joined_pdf_execution") {
        return;
    }
    let _serial = crate::foreground::TEST_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    blocked_resource(false);
}

#[test]
fn a_panicked_import_worker_keeps_retirement_failure_sticky_and_the_runtime_owned() {
    if in_isolated_child(
        "a_panicked_import_worker_keeps_retirement_failure_sticky_and_the_runtime_owned",
    ) {
        return;
    }
    struct Panics;
    impl CapturedImport for Panics {
        fn preview(&self) -> Result<CliRun, String> {
            panic!("preview control")
        }
        fn run(&self) -> Result<CliRun, String> {
            panic!("retained worker control")
        }
    }
    let (_root, server, host) = fixture(Arc::new(|_| panic!("factory unused")));
    server
        .import_route()
        .run_retained(Arc::new(Panics), |_| {})
        .unwrap();
    host.shutdown().unwrap();
    assert!(server.shutdown_for_retirement().is_err());
    assert!(server.shutdown_for_retirement().is_err());
    assert!(server.shutdown_failed.load(Ordering::Acquire));
    assert!(!server.shutdown_complete.load(Ordering::Acquire));
    assert!(server.services.lock().unwrap().is_some());
    assert_eq!(server.runtime.block_on(async { 17 }), 17);
    // The production caller retains failed owners. This isolated child exits
    // with its real Runtime retained; dropping it would intentionally park.
    std::mem::forget(server);
    std::mem::forget(host);
}

#[cfg(target_os = "linux")]
#[path = "import_route_owner_tests.rs"]
mod owner_effects;
