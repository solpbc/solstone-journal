// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;

use serde_json::json;
use solstone_core_system::provider_runtime::{
    LocalLaunchCommon, LocalLaunchConfig, LocalProbeSeam, LocalRuntimeShared, ProbeSeam,
    ProbeStatus, ProviderFence, ProviderName, ProviderRuntimeState, ReasonCode,
};

static NEXT_JOURNAL: AtomicU64 = AtomicU64::new(0);

fn probe_config() -> LocalLaunchConfig {
    LocalLaunchConfig::Mlx {
        common: LocalLaunchCommon {
            desired_fingerprint_json: json!({"provider":"local"}),
            desired_fingerprint_sha256: "fingerprint".into(),
            model_id: "local/test".into(),
            model_path: "test-model".into(),
            mmproj_path: None,
        },
        runtime_dir: None,
        interpreter_path: None,
    }
}

fn state() -> ProviderRuntimeState {
    let mut state = ProviderRuntimeState::new(ProviderName::Local);
    state.desired_fingerprint = Some("fingerprint".into());
    state
}

fn fence(attempt: u32) -> ProviderFence {
    ProviderFence {
        incarnation: "test".into(),
        generation: 1,
        fingerprint: Some("fingerprint".into()),
        attempt,
    }
}

fn journal() -> PathBuf {
    let sequence = NEXT_JOURNAL.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("solstone-local-probe-{sequence}"));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(path.join("health")).expect("health directory");
    path
}

fn write_port(journal: &std::path::Path, port: u16) {
    std::fs::write(journal.join("health/local.port"), port.to_string()).expect("port file");
}

fn serve(responses: Vec<&'static str>) -> (u16, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("address").port();
    let handle = thread::spawn(move || {
        for response in responses {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut request = [0; 1024];
            let _ = stream.read(&mut request);
            stream.write_all(response.as_bytes()).expect("response");
        }
    });
    (port, handle)
}

fn probe(
    shared: Arc<LocalRuntimeShared>,
    journal: &std::path::Path,
    probe_fence: &ProviderFence,
) -> solstone_core_system::provider_runtime::ProviderProbeOutcome {
    let state = state();
    shared.record_launch_request(state.desired_fingerprint.clone(), probe_config());
    let mut seam = LocalProbeSeam::new(shared.clone(), journal);
    seam.dispatch_probe(&state, probe_fence);
    shared.wait_for_probe_result(probe_fence)
}

#[test]
fn steady_state_probe_collapses_connect_outcomes_without_losing_unavailable() {
    let ready_journal = journal();
    let (ready_port, ready_server) = serve(vec![
        "HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}",
        "HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}",
    ]);
    write_port(&ready_journal, ready_port);
    let ready = probe(
        Arc::new(LocalRuntimeShared::default()),
        &ready_journal,
        &fence(1),
    );
    assert_eq!(ready.status, ProbeStatus::Ready);
    assert_eq!(ready.reason_code, ReasonCode::known("probe-ready"));
    ready_server.join().expect("ready server");
    std::fs::remove_dir_all(ready_journal).expect("remove ready journal");

    let loading_journal = journal();
    let (loading_port, loading_server) = serve(vec![
        "HTTP/1.1 503 Service Unavailable\r\nContent-Length: 13\r\n\r\nloading model",
    ]);
    write_port(&loading_journal, loading_port);
    let loading = probe(
        Arc::new(LocalRuntimeShared::default()),
        &loading_journal,
        &fence(2),
    );
    assert_eq!(loading.status, ProbeStatus::NotReady);
    assert_eq!(loading.reason_code, ReasonCode::known("probe-not-ready"));
    loading_server.join().expect("loading server");
    std::fs::remove_dir_all(loading_journal).expect("remove loading journal");

    let missing_port_journal = journal();
    let missing_port = probe(
        Arc::new(LocalRuntimeShared::default()),
        &missing_port_journal,
        &fence(3),
    );
    assert_eq!(missing_port.status, ProbeStatus::Unavailable);
    assert_eq!(
        missing_port.reason_code,
        ReasonCode::known("proof-observation-unavailable")
    );
    std::fs::remove_dir_all(missing_port_journal).expect("remove missing-port journal");

    let failed_journal = journal();
    let (failed_port, failed_server) = serve(vec![
        "HTTP/1.1 500 Internal Server Error\r\nContent-Length: 4\r\n\r\noops",
    ]);
    write_port(&failed_journal, failed_port);
    let failed = probe(
        Arc::new(LocalRuntimeShared::default()),
        &failed_journal,
        &fence(4),
    );
    assert_eq!(failed.status, ProbeStatus::Unavailable);
    assert_eq!(
        failed.reason_code,
        ReasonCode::known("proof-observation-unavailable")
    );
    failed_server.join().expect("failed server");
    std::fs::remove_dir_all(failed_journal).expect("remove failed journal");
}
