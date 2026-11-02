// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(feature = "test-stubs")]

use std::path::PathBuf;
#[cfg(target_os = "linux")]
use std::process::Command;
use std::sync::{Arc, mpsc};
use std::time::Duration;

use serde_json::{Map, json};
use solstone_core_callosum::{CallosumSocketConnection, CallosumSocketServer};
use solstone_core_journal_io::{
    DayMarkerPairStatus, PublishOutcome, bump_stream_marker, day_marker_pair_status,
    publish_daily_marker_if_current,
};
use solstone_core_sense::{
    SenseDispatcher,
    batch::{
        BatchError, BatchRequest, process_day_with_fixture_program,
        process_day_with_fixture_program_and_timeout,
    },
    dispatch::Outbound,
    service::run_until,
};
use tokio::sync::oneshot;

fn observing(files: &[&str]) -> Map<String, serde_json::Value> {
    Map::from_iter([
        ("day".into(), json!("20260812")),
        ("stream".into(), json!("default")),
        ("segment".into(), json!("120000_2")),
        ("files".into(), json!(files)),
    ])
}

async fn start_service(
    journal: PathBuf,
    fixture: PathBuf,
) -> (
    CallosumSocketConnection,
    Arc<SenseDispatcher>,
    oneshot::Sender<()>,
) {
    let socket = journal.join("health/callosum.sock");
    let service = CallosumSocketConnection::new(&socket, Map::new());
    let (outbound, receiver) = mpsc::channel::<Outbound>();
    let dispatcher = Arc::new(SenseDispatcher::new_with_fixture_program(
        journal, false, false, outbound, fixture,
    ));
    let loop_dispatcher = Arc::clone(&dispatcher);
    let (stop_tx, mut stop_rx) = oneshot::channel();
    tokio::spawn(async move {
        run_until(service, loop_dispatcher, receiver, async move {
            let _ = (&mut stop_rx).await;
        })
        .await;
    });
    let mut peer = CallosumSocketConnection::new(&socket, Map::new());
    peer.start();
    (peer, dispatcher, stop_tx)
}

async fn next_event(
    peer: &mut CallosumSocketConnection,
    tract: &str,
    event: &str,
) -> solstone_core_callosum::CallosumEnvelope {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let message = peer.next_message().await.expect("peer");
            if message.tract == tract && message.event == event {
                break message;
            }
        }
    })
    .await
    .expect("event")
}

/// Waits for an event, failing if a notification card arrives first: a failed
/// or timed-out file is recorded on its segment, not announced per file.
async fn next_event_rejecting_cards(
    peer: &mut CallosumSocketConnection,
    tract: &str,
    event: &str,
) -> solstone_core_callosum::CallosumEnvelope {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let message = peer.next_message().await.expect("peer");
            assert!(
                !(message.tract == "notification" && message.event == "show"),
                "a per-file failure card was sent"
            );
            if message.tract == tract && message.event == event {
                break message;
            }
        }
    })
    .await
    .expect("event")
}

async fn no_second_observed_for(peer: &mut CallosumSocketConnection, segment: &str) {
    assert!(
        tokio::time::timeout(Duration::from_millis(350), async {
            loop {
                let message = peer.next_message().await.expect("peer");
                if message.tract == "observe"
                    && message.event == "observed"
                    && message.extra["segment"] == segment
                {
                    break message;
                }
            }
        })
        .await
        .is_err(),
        "segment must emit observe.observed exactly once"
    );
}

async fn wait_for_clients(server: &CallosumSocketServer) {
    for _ in 0..50 {
        if server.client_count() >= 2 {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("service and peer did not connect");
}

async fn wait_for_client_count(server: &CallosumSocketServer, count: usize) {
    for _ in 0..50 {
        if server.client_count() >= count {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("callosum clients did not connect");
}

#[tokio::test]
async fn batch_processing_publishes_observed_to_a_real_callosum_socket() {
    let root = tempfile::tempdir().expect("journal");
    let segment = root.path().join("chronicle/20260812/default/120000_2");
    std::fs::create_dir_all(&segment).expect("segment");
    std::fs::write(segment.join("audio.flac"), b"audio").expect("audio");
    let socket = root.path().join("health/callosum.sock");
    let server = CallosumSocketServer::bind(&socket).await.expect("server");
    let mut peer = CallosumSocketConnection::new(&socket, Map::new());
    peer.start();
    wait_for_client_count(&server, 1).await;

    let request = BatchRequest {
        day: "20260812".into(),
        jobs: 1,
        reprocess: None,
        segment: Some("120000_2".into()),
        stream: Some("default".into()),
        dry_run: false,
        verbose: false,
        debug: false,
    };
    let journal = root.path().to_path_buf();
    let handler = PathBuf::from(env!("CARGO_BIN_EXE_solstone-core-sense-test-handler"));
    tokio::task::spawn_blocking(move || {
        process_day_with_fixture_program(&journal, &request, None, handler)
    })
    .await
    .expect("batch task")
    .expect("batch processing");

    let observed = next_event(&mut peer, "observe", "observed").await;
    assert_eq!(observed.extra["segment"], "120000_2");
    assert!(segment.join("audio.flac.handler").is_file());
    server.stop().await;
}

#[tokio::test]
async fn standalone_batch_completion_dirties_a_previously_completed_day() {
    let root = tempfile::tempdir().expect("journal");
    let segment = root.path().join("chronicle/20260812/default/120000_2");
    std::fs::create_dir_all(&segment).expect("segment");
    std::fs::write(segment.join("audio.flac"), b"audio").expect("audio");
    let admitted = bump_stream_marker(root.path(), "20260812").expect("stream marker");
    assert_eq!(
        publish_daily_marker_if_current(
            root.path(),
            "20260812",
            admitted,
            "raw-before-batch",
            || Ok("raw-before-batch".to_owned()),
        )
        .expect("daily marker"),
        PublishOutcome::Published(admitted)
    );
    assert_eq!(
        day_marker_pair_status(root.path(), "20260812").expect("complete pair"),
        DayMarkerPairStatus::Complete
    );
    let socket = root.path().join("health/callosum.sock");
    let server = CallosumSocketServer::bind(&socket).await.expect("server");
    let request = BatchRequest {
        day: "20260812".into(),
        jobs: 1,
        reprocess: None,
        segment: Some("120000_2".into()),
        stream: Some("default".into()),
        dry_run: false,
        verbose: false,
        debug: false,
    };
    let journal = root.path().to_path_buf();
    let handler = PathBuf::from(env!("CARGO_BIN_EXE_solstone-core-sense-test-handler"));
    tokio::task::spawn_blocking(move || {
        process_day_with_fixture_program(&journal, &request, None, handler)
    })
    .await
    .expect("batch task")
    .expect("batch processing");

    assert_eq!(
        day_marker_pair_status(root.path(), "20260812").expect("dirty pair"),
        DayMarkerPairStatus::Dirty
    );
    server.stop().await;
}

async fn run_fixture_batch(files: &[&str]) -> Result<(), BatchError> {
    let root = tempfile::tempdir().expect("journal");
    let segment = root.path().join("chronicle/20260812/default/120000_2");
    std::fs::create_dir_all(&segment).expect("segment");
    for name in files {
        std::fs::write(segment.join(name), b"audio").expect("audio");
    }
    let socket = root.path().join("health/callosum.sock");
    let server = CallosumSocketServer::bind(&socket).await.expect("server");
    let request = BatchRequest {
        day: "20260812".into(),
        jobs: 1,
        reprocess: None,
        segment: Some("120000_2".into()),
        stream: Some("default".into()),
        dry_run: false,
        verbose: false,
        debug: false,
    };
    let journal = root.path().to_path_buf();
    let handler = PathBuf::from(env!("CARGO_BIN_EXE_solstone-core-sense-test-handler"));
    let result = tokio::task::spawn_blocking(move || {
        process_day_with_fixture_program(&journal, &request, None, handler)
    })
    .await
    .expect("batch task");
    server.stop().await;
    result
}

#[tokio::test]
async fn batch_nonzero_exit_returns_failed_tally() {
    let result = run_fixture_batch(&["fail.flac"]).await;
    assert!(
        matches!(result, Err(BatchError::Failed { failed: 1, ran: 1 })),
        "expected Failed {{ failed: 1, ran: 1 }}, got {result:?}"
    );
}

#[tokio::test]
async fn batch_zero_exit_without_output_is_a_failure_for_each_modality() {
    for name in [
        "no-output.flac",
        "no-output.m4a",
        "no-output.webm",
        "no-output.png",
    ] {
        let result = run_fixture_batch(&[name]).await;
        assert!(
            matches!(result, Err(BatchError::Failed { failed: 1, ran: 1 })),
            "{name}: {result:?}"
        );
    }
}

#[tokio::test]
async fn batch_success_requires_an_output_and_accepts_each_modality() {
    for name in ["ok.flac", "ok.m4a", "ok.webm", "ok.png"] {
        run_fixture_batch(&[name]).await.expect(name);
    }
}

#[tokio::test]
async fn batch_provider_blocked_exit_is_not_a_failure() {
    run_fixture_batch(&["blocked.flac"])
        .await
        .expect("exit 69 is a deferral, not a batch failure");
}

#[tokio::test]
async fn batch_mixed_ok_and_fail_returns_partial_tally() {
    let result = run_fixture_batch(&["ok.flac", "fail.flac"]).await;
    assert!(
        matches!(result, Err(BatchError::Failed { failed: 1, ran: 2 })),
        "expected Failed {{ failed: 1, ran: 2 }}, got {result:?}"
    );
}

#[tokio::test]
async fn standalone_batch_marker_failure_is_a_failed_run_after_output_mutation() {
    let root = tempfile::tempdir().expect("journal");
    let segment = root.path().join("chronicle/20260812/default/120000_2");
    std::fs::create_dir_all(&segment).expect("segment");
    std::fs::write(segment.join("ok.flac"), b"audio").expect("audio");
    let health = root.path().join("chronicle/20260812/health");
    std::fs::create_dir_all(&health).expect("health");
    std::fs::create_dir(health.join("stream.updated")).expect("block stream marker");
    let socket = root.path().join("health/callosum.sock");
    let server = CallosumSocketServer::bind(&socket).await.expect("server");
    let request = BatchRequest {
        day: "20260812".into(),
        jobs: 1,
        reprocess: None,
        segment: Some("120000_2".into()),
        stream: Some("default".into()),
        dry_run: false,
        verbose: false,
        debug: false,
    };
    let journal = root.path().to_path_buf();
    let handler = PathBuf::from(env!("CARGO_BIN_EXE_solstone-core-sense-test-handler"));
    let result = tokio::task::spawn_blocking(move || {
        process_day_with_fixture_program(&journal, &request, None, handler)
    })
    .await
    .expect("batch task");

    assert!(segment.join("ok.flac.handler").is_file());
    assert!(
        matches!(result, Err(BatchError::Failed { failed: 1, ran: 1 })),
        "marker failure must fail the run, got {result:?}"
    );
    server.stop().await;
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn batch_timeout_reaps_active_managed_process_tree_before_returning() {
    let root = tempfile::tempdir().expect("journal");
    let segment = root.path().join("chronicle/20260812/default/120000_2");
    std::fs::create_dir_all(&segment).expect("segment");
    std::fs::write(segment.join("grandchild.webm"), b"screen").expect("screen");
    let socket = root.path().join("health/callosum.sock");
    let server = CallosumSocketServer::bind(&socket).await.expect("server");
    let request = BatchRequest {
        day: "20260812".into(),
        jobs: 1,
        reprocess: None,
        segment: Some("120000_2".into()),
        stream: Some("default".into()),
        dry_run: false,
        verbose: false,
        debug: false,
    };
    let timeout = Duration::from_secs(2);
    let journal = root.path().to_path_buf();
    let handler = PathBuf::from(env!("CARGO_BIN_EXE_solstone-core-sense-test-handler"));
    let result = tokio::task::spawn_blocking(move || {
        process_day_with_fixture_program_and_timeout(
            &journal,
            &request,
            None,
            handler,
            Some(timeout),
        )
    })
    .await
    .expect("batch task");

    assert!(
        matches!(result, Err(BatchError::TimedOut { timeout: actual }) if actual == timeout),
        "expected typed aggregate timeout, got {result:?}"
    );
    let pid_file = segment.join("grandchild.webm.grandchild-pid");
    let pid = std::fs::read_to_string(&pid_file).expect("active fixture wrote grandchild pid");
    assert!(
        !PathBuf::from(format!("/proc/{}", pid.trim())).exists(),
        "batch timeout returned before the managed descendant was reaped"
    );
    server.stop().await;
}

#[tokio::test]
async fn real_socket_event_spawns_built_audio_and_screen_handlers_once() {
    let root = tempfile::tempdir().expect("journal");
    std::fs::create_dir_all(root.path().join("config")).expect("config");
    std::fs::write(
        root.path().join("config/journal.json"),
        br#"{"providers":{"active":{"provider":"openai"}}}"#,
    )
    .expect("settings");
    let segment = root.path().join("chronicle/20260812/default/120000_2");
    std::fs::create_dir_all(&segment).expect("segment");
    std::fs::write(segment.join("audio.flac"), b"audio").expect("audio");
    std::fs::write(segment.join("screen.webm"), b"screen").expect("screen");
    let socket = root.path().join("health/callosum.sock");
    let server = CallosumSocketServer::bind(&socket).await.expect("server");
    let (mut peer, _dispatcher, stop) = start_service(
        root.path().to_path_buf(),
        PathBuf::from(env!("CARGO_BIN_EXE_solstone-core-sense-test-handler")),
    )
    .await;
    wait_for_clients(&server).await;
    assert!(peer.emit(
        "observe",
        "observing",
        observing(&["audio.flac", "screen.webm"])
    ));
    let observed = next_event(&mut peer, "observe", "observed").await;
    assert_eq!(observed.extra["segment"], "120000_2");
    assert!(!observed.extra.contains_key("error"));
    assert!(segment.join("audio.flac.handler").is_file());
    assert!(segment.join("screen.webm.handler").is_file());
    no_second_observed_for(&mut peer, "120000_2").await;
    let _ = stop.send(());
    server.stop().await;
}

#[tokio::test]
async fn fresh_journal_without_an_engine_transcribes_and_holds_screen_work() {
    let root = tempfile::tempdir().expect("journal");
    let segment = root.path().join("chronicle/20260812/default/120000_2");
    std::fs::create_dir_all(&segment).expect("segment");
    std::fs::write(segment.join("audio.flac"), b"audio").expect("audio");
    std::fs::write(segment.join("screen.webm"), b"screen").expect("screen");
    let socket = root.path().join("health/callosum.sock");
    let server = CallosumSocketServer::bind(&socket).await.expect("server");
    let (mut peer, _dispatcher, stop) = start_service(
        root.path().to_path_buf(),
        PathBuf::from(env!("CARGO_BIN_EXE_solstone-core-sense-test-handler")),
    )
    .await;
    wait_for_clients(&server).await;
    assert!(peer.emit(
        "observe",
        "observing",
        observing(&["audio.flac", "screen.webm"])
    ));
    let observed = next_event(&mut peer, "observe", "observed").await;
    assert_eq!(observed.extra["segment"], "120000_2");
    assert_eq!(observed.extra["note"], "no_engine");
    assert!(!observed.extra.contains_key("error"));
    assert!(segment.join("audio.flac.handler").is_file());
    assert!(!segment.join("screen.webm.handler").exists());
    no_second_observed_for(&mut peer, "120000_2").await;
    let _ = stop.send(());
    server.stop().await;
}

#[tokio::test]
async fn provider_blocked_is_observed_without_error_and_is_redeliverable() {
    let root = tempfile::tempdir().expect("journal");
    std::fs::create_dir_all(root.path().join("config")).expect("config");
    std::fs::write(
        root.path().join("config/journal.json"),
        br#"{"providers":{"active":{"provider":"openai"}}}"#,
    )
    .expect("settings");
    let segment = root.path().join("chronicle/20260812/default/120000_2");
    std::fs::create_dir_all(&segment).expect("segment");
    std::fs::write(segment.join("blocked.flac"), b"audio").expect("audio");
    let socket = root.path().join("health/callosum.sock");
    let server = CallosumSocketServer::bind(&socket).await.expect("server");
    let (mut peer, _dispatcher, stop) = start_service(
        root.path().to_path_buf(),
        PathBuf::from(env!("CARGO_BIN_EXE_solstone-core-sense-test-handler")),
    )
    .await;
    wait_for_clients(&server).await;
    assert!(peer.emit("observe", "observing", observing(&["blocked.flac"])));
    let first = next_event(&mut peer, "observe", "observed").await;
    assert!(!first.extra.contains_key("error"));
    assert!(segment.join("blocked.flac").is_file());
    assert!(peer.emit("observe", "observing", observing(&["blocked.flac"])));
    let second = next_event(&mut peer, "observe", "observed").await;
    assert!(!second.extra.contains_key("error"));
    let _ = stop.send(());
    server.stop().await;
}

#[tokio::test]
async fn duplicate_observing_while_work_is_queued_spawns_once() {
    let root = tempfile::tempdir().expect("journal");
    std::fs::create_dir_all(root.path().join("config")).expect("config");
    std::fs::write(
        root.path().join("config/journal.json"),
        br#"{"providers":{"active":{"provider":"openai"}},"transcribe":{"max_runtime":1}}"#,
    )
    .expect("settings");
    let segment = root.path().join("chronicle/20260812/default/120000_2");
    std::fs::create_dir_all(&segment).expect("segment");
    std::fs::write(segment.join("sleep.flac"), b"audio").expect("audio");
    let socket = root.path().join("health/callosum.sock");
    let server = CallosumSocketServer::bind(&socket).await.expect("server");
    let (mut peer, _dispatcher, stop) = start_service(
        root.path().to_path_buf(),
        PathBuf::from(env!("CARGO_BIN_EXE_solstone-core-sense-test-handler")),
    )
    .await;
    wait_for_clients(&server).await;
    assert!(peer.emit("observe", "observing", observing(&["sleep.flac"])));
    assert!(peer.emit("observe", "observing", observing(&["sleep.flac"])));
    let _ = next_event(&mut peer, "observe", "observed").await;
    let marker = std::fs::read_to_string(segment.join("sleep.flac.handler")).expect("marker");
    assert_eq!(
        marker.lines().filter(|line| *line == "transcribe").count(),
        1
    );
    no_second_observed_for(&mut peer, "120000_2").await;
    let _ = stop.send(());
    server.stop().await;
}

#[tokio::test]
async fn no_matching_handler_completes_promptly_and_does_not_leak_segment_state() {
    let root = tempfile::tempdir().expect("journal");
    std::fs::create_dir_all(root.path().join("config")).expect("config");
    std::fs::write(
        root.path().join("config/journal.json"),
        br#"{"providers":{"active":{"provider":"openai"}}}"#,
    )
    .expect("settings");
    let first = root.path().join("chronicle/20260812/default/120000_2");
    let second = root.path().join("chronicle/20260812/default/120001_2");
    std::fs::create_dir_all(&first).expect("first segment");
    std::fs::create_dir_all(&second).expect("second segment");
    std::fs::write(first.join("unhandled.txt"), b"text").expect("unhandled input");
    std::fs::write(second.join("audio.flac"), b"audio").expect("handled input");
    let socket = root.path().join("health/callosum.sock");
    let server = CallosumSocketServer::bind(&socket).await.expect("server");
    let (mut peer, _dispatcher, stop) = start_service(
        root.path().to_path_buf(),
        PathBuf::from(env!("CARGO_BIN_EXE_solstone-core-sense-test-handler")),
    )
    .await;
    wait_for_clients(&server).await;
    assert!(peer.emit("observe", "observing", observing(&["unhandled.txt"])));
    let observed = next_event(&mut peer, "observe", "observed").await;
    assert_eq!(observed.extra["note"], "no handlers");
    no_second_observed_for(&mut peer, "120000_2").await;
    let mut next = observing(&["audio.flac"]);
    next.insert("segment".into(), json!("120001_2"));
    assert!(peer.emit("observe", "observing", next));
    let follow_up = next_event(&mut peer, "observe", "observed").await;
    assert_eq!(follow_up.extra["segment"], "120001_2");
    assert!(second.join("audio.flac.handler").is_file());
    let _ = stop.send(());
    server.stop().await;
}

#[tokio::test]
async fn child_receives_only_supported_segment_environment_from_observing_event() {
    let root = tempfile::tempdir().expect("journal");
    std::fs::create_dir_all(root.path().join("config")).expect("config");
    std::fs::write(
        root.path().join("config/journal.json"),
        br#"{"providers":{"active":{"provider":"openai"}}}"#,
    )
    .expect("settings");
    let segment = root.path().join("chronicle/20260812/default/120000_2");
    std::fs::create_dir_all(&segment).expect("segment");
    std::fs::write(segment.join("audio.flac"), b"audio").expect("audio");
    let socket = root.path().join("health/callosum.sock");
    let server = CallosumSocketServer::bind(&socket).await.expect("server");
    let (mut peer, _dispatcher, stop) = start_service(
        root.path().to_path_buf(),
        PathBuf::from(env!("CARGO_BIN_EXE_solstone-core-sense-test-handler")),
    )
    .await;
    wait_for_clients(&server).await;
    let mut event = observing(&["audio.flac"]);
    event.insert("cid".into(), json!("sha256:test"));
    event.insert("source".into(), json!("capture-agent"));
    assert!(peer.emit("observe", "observing", event));
    let _ = next_event(&mut peer, "observe", "observed").await;
    let marker = std::fs::read_to_string(segment.join("audio.flac.handler")).expect("marker");
    assert!(marker.contains("SOL_SEGMENT=120000_2\n"));
    assert!(!marker.contains("OBSERVER_NAME="));
    assert!(marker.contains("SEGMENT_META={\"stream\":\"default\"}\n"));
    marker
        .lines()
        .find_map(|line| line.strip_prefix("SOL_QUEUE_WAIT_MS="))
        .expect("queue wait")
        .parse::<u128>()
        .expect("non-negative queue wait");
    let _ = stop.send(());
    server.stop().await;
}

#[tokio::test]
async fn failure_records_observed_error_without_a_card() {
    let root = tempfile::tempdir().expect("journal");
    std::fs::create_dir_all(root.path().join("config")).expect("config");
    std::fs::write(
        root.path().join("config/journal.json"),
        br#"{"providers":{"active":{"provider":"openai"}}}"#,
    )
    .expect("settings");
    let segment = root.path().join("chronicle/20260812/default/120000_2");
    std::fs::create_dir_all(&segment).expect("segment");
    std::fs::write(segment.join("fail.flac"), b"audio").expect("audio");
    let socket = root.path().join("health/callosum.sock");
    let server = CallosumSocketServer::bind(&socket).await.expect("server");
    let (mut peer, _dispatcher, stop) = start_service(
        root.path().to_path_buf(),
        PathBuf::from(env!("CARGO_BIN_EXE_solstone-core-sense-test-handler")),
    )
    .await;
    wait_for_clients(&server).await;
    assert!(peer.emit("observe", "observing", observing(&["fail.flac"])));
    let observed = next_event_rejecting_cards(&mut peer, "observe", "observed").await;
    assert_eq!(observed.extra["error"], true);
    assert!(
        observed.extra["errors"][0]
            .as_str()
            .expect("error")
            .contains("exit 7")
    );
    let _ = stop.send(());
    server.stop().await;
}

#[tokio::test]
async fn shutdown_nonzero_records_segment_error_without_notification() {
    let root = tempfile::tempdir().expect("journal");
    std::fs::create_dir_all(root.path().join("config")).expect("config");
    std::fs::write(
        root.path().join("config/journal.json"),
        br#"{"providers":{"active":{"provider":"openai"}}}"#,
    )
    .expect("settings");
    let segment = root.path().join("chronicle/20260812/default/120000_2");
    std::fs::create_dir_all(&segment).expect("segment");
    std::fs::write(segment.join("sleep.flac"), b"audio").expect("audio");
    let socket = root.path().join("health/callosum.sock");
    let server = CallosumSocketServer::bind(&socket).await.expect("server");
    let (mut peer, _dispatcher, stop) = start_service(
        root.path().to_path_buf(),
        PathBuf::from(env!("CARGO_BIN_EXE_solstone-core-sense-test-handler")),
    )
    .await;
    wait_for_clients(&server).await;
    assert!(peer.emit("observe", "observing", observing(&["sleep.flac"])));
    let _ = next_event(&mut peer, "observe", "detected").await;
    let _ = stop.send(());
    let observed = next_event(&mut peer, "observe", "observed").await;
    assert_eq!(observed.extra["error"], true);
    assert!(
        tokio::time::timeout(Duration::from_millis(200), async {
            loop {
                let event = peer.next_message().await.expect("peer");
                if event.tract == "notification" && event.event == "show" {
                    break event;
                }
            }
        })
        .await
        .is_err(),
        "shutdown non-zero path does not notify"
    );
    server.stop().await;
}

#[tokio::test]
async fn shutdown_promptly_terminates_within_cap_without_notification() {
    let root = tempfile::tempdir().expect("journal");
    std::fs::create_dir_all(root.path().join("config")).expect("config");
    std::fs::write(
        root.path().join("config/journal.json"),
        br#"{"providers":{"active":{"provider":"openai"}}}"#,
    )
    .expect("settings");
    let segment = root.path().join("chronicle/20260812/default/120000_2");
    std::fs::create_dir_all(&segment).expect("segment");
    std::fs::write(segment.join("sleep.flac"), b"audio").expect("audio");
    let socket = root.path().join("health/callosum.sock");
    let server = CallosumSocketServer::bind(&socket).await.expect("server");
    let (mut peer, _dispatcher, stop) = start_service(
        root.path().to_path_buf(),
        PathBuf::from(env!("CARGO_BIN_EXE_solstone-core-sense-test-handler")),
    )
    .await;
    wait_for_clients(&server).await;
    assert!(peer.emit("observe", "observing", observing(&["sleep.flac"])));
    let _ = next_event(&mut peer, "observe", "detected").await;
    let started = tokio::time::Instant::now();
    let _ = stop.send(());
    let observed = tokio::time::timeout(
        Duration::from_secs(3),
        next_event(&mut peer, "observe", "observed"),
    )
    .await
    .expect("shutdown completes promptly");
    assert!(started.elapsed() < Duration::from_secs(3));
    assert_eq!(observed.extra["error"], true);
    assert!(
        tokio::time::timeout(Duration::from_millis(200), async {
            loop {
                let event = peer.next_message().await.expect("peer");
                if event.tract == "notification" && event.event == "show" {
                    break event;
                }
            }
        })
        .await
        .is_err(),
        "within-cap shutdown does not notify"
    );
    server.stop().await;
}

#[tokio::test]
async fn watchdog_timeout_records_error_without_a_card_when_shutdown_follows() {
    let root = tempfile::tempdir().expect("journal");
    std::fs::create_dir_all(root.path().join("config")).expect("config");
    std::fs::write(
        root.path().join("config/journal.json"),
        br#"{"providers":{"active":{"provider":"openai"}},"transcribe":{"max_runtime":1}}"#,
    )
    .expect("settings");
    let segment = root.path().join("chronicle/20260812/default/120000_2");
    std::fs::create_dir_all(&segment).expect("segment");
    std::fs::write(segment.join("sleep.flac"), b"audio").expect("audio");
    let socket = root.path().join("health/callosum.sock");
    let server = CallosumSocketServer::bind(&socket).await.expect("server");
    let (mut peer, _dispatcher, stop) = start_service(
        root.path().to_path_buf(),
        PathBuf::from(env!("CARGO_BIN_EXE_solstone-core-sense-test-handler")),
    )
    .await;
    wait_for_clients(&server).await;
    assert!(peer.emit("observe", "observing", observing(&["sleep.flac"])));
    let _ = next_event(&mut peer, "observe", "detected").await;
    // The one-second cap runs from the handler's launch, which a loaded host
    // can delay; stopping before it would end the handler as a shutdown.
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let _ = stop.send(());
    let observed = next_event_rejecting_cards(&mut peer, "observe", "observed").await;
    assert_eq!(observed.extra["error"], true);
    assert!(
        observed.extra["errors"][0]
            .as_str()
            .expect("error")
            .contains("watchdog_timeout"),
        "{:?}",
        observed.extra["errors"]
    );
    server.stop().await;
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn watchdog_uses_managed_termination_to_reap_grandchild_then_keeps_dispatching() {
    let root = tempfile::tempdir().expect("journal");
    std::fs::create_dir_all(root.path().join("config")).expect("config");
    std::fs::write(
        root.path().join("config/journal.json"),
        br#"{"providers":{"active":{"provider":"openai"}},"describe":{"max_runtime":1}}"#,
    )
    .expect("settings");
    let segment = root.path().join("chronicle/20260812/default/120000_2");
    std::fs::create_dir_all(&segment).expect("segment");
    std::fs::write(segment.join("grandchild.webm"), b"screen").expect("screen");
    std::fs::write(segment.join("audio.flac"), b"audio").expect("audio");
    let socket = root.path().join("health/callosum.sock");
    let server = CallosumSocketServer::bind(&socket).await.expect("server");
    let (mut peer, _dispatcher, stop) = start_service(
        root.path().to_path_buf(),
        PathBuf::from(env!("CARGO_BIN_EXE_solstone-core-sense-test-handler")),
    )
    .await;
    wait_for_clients(&server).await;
    assert!(peer.emit("observe", "observing", observing(&["grandchild.webm"])));
    let observed = next_event(&mut peer, "observe", "observed").await;
    assert_eq!(observed.extra["error"], true);
    let pid_file = segment.join("grandchild.webm.grandchild-pid");
    for _ in 0..50 {
        if pid_file.is_file() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let pid = std::fs::read_to_string(&pid_file).expect("grandchild pid");
    assert!(
        !PathBuf::from(format!("/proc/{}", pid.trim())).exists(),
        "ManagedProcess::terminate reaped descendant"
    );
    assert!(peer.emit("observe", "observing", observing(&["audio.flac"])));
    let recovered = next_event(&mut peer, "observe", "observed").await;
    assert!(
        !recovered.extra.contains_key("error"),
        "dispatcher accepts later work"
    );
    let _ = stop.send(());
    server.stop().await;
}

#[cfg(target_os = "linux")]
#[test]
fn falsification_raw_child_signal_leaves_grandchild_alive() {
    let root = tempfile::tempdir().expect("temp");
    let input = root.path().join("grandchild.webm");
    std::fs::write(&input, b"screen").expect("input");
    let fixture = env!("CARGO_BIN_EXE_solstone-core-sense-test-handler");
    let mut child = Command::new(fixture)
        .args(["describe", input.to_str().expect("utf8")])
        .spawn()
        .expect("parent");
    let pid_file = root.path().join("grandchild.webm.grandchild-pid");
    for _ in 0..50 {
        if pid_file.is_file() {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    child.kill().expect("raw child signal");
    let _ = child.wait();
    let pid = std::fs::read_to_string(&pid_file).expect("grandchild pid");
    let proc = PathBuf::from(format!("/proc/{}", pid.trim()));
    assert!(
        proc.exists(),
        "raw Popen-style child signal leaves the descendant alive; this is the required falsification"
    );
    let status = Command::new("kill")
        .args(["-KILL", pid.trim()])
        .status()
        .expect("cleanup grandchild");
    assert!(status.success());
}

#[cfg(feature = "full-tests")]
struct TestMemoryProbe {
    available: std::sync::atomic::AtomicU64,
}

#[cfg(feature = "full-tests")]
impl solstone_core_sense::memory::MemoryProbe for TestMemoryProbe {
    fn available_bytes(&self) -> Option<u64> {
        Some(self.available.load(std::sync::atomic::Ordering::SeqCst))
    }
    fn total_bytes(&self) -> Option<u64> {
        None
    }
    fn unified_memory(&self) -> bool {
        false
    }
}

#[cfg(feature = "full-tests")]
fn make_observing_envelope(
    day: &str,
    segment: &str,
    files: &[&str],
    batch: bool,
    import_id: Option<&str>,
) -> solstone_core_callosum::CallosumEnvelope {
    let mut extra = Map::from_iter([
        ("day".into(), json!(day)),
        ("stream".into(), json!("stream1")),
        ("segment".into(), json!(segment)),
        ("files".into(), json!(files)),
        ("batch".into(), json!(batch)),
    ]);
    if let Some(id) = import_id {
        extra.insert("meta".into(), json!({ "import_id": id }));
    }
    solstone_core_callosum::CallosumEnvelope {
        tract: "observe".into(),
        event: "observing".into(),
        ts: None,
        extra,
    }
}

#[cfg(feature = "full-tests")]
#[test]
fn live_arrival_takes_the_next_free_handler_slot() {
    let temp = tempfile::tempdir().expect("temp journal");
    std::fs::create_dir_all(temp.path().join("config")).expect("config");
    std::fs::write(
        temp.path().join("config/journal.json"),
        br#"{"providers":{"active":{"provider":"openai"}},"memory":{"floor_mib":1}}"#,
    )
    .expect("settings");

    let seg1 = temp.path().join("chronicle/20261010/stream1/100000_1");
    let seg2 = temp.path().join("chronicle/20261011/stream1/110000_1");
    let seg3 = temp.path().join("chronicle/20261012/stream1/120000_1");
    let seg_live = temp.path().join("chronicle/20261001/stream1/010000_1");

    std::fs::create_dir_all(&seg1).expect("seg1");
    std::fs::create_dir_all(&seg2).expect("seg2");
    std::fs::create_dir_all(&seg3).expect("seg3");
    std::fs::create_dir_all(&seg_live).expect("seg_live");

    std::fs::write(seg1.join("audio.flac"), b"audio").expect("audio1");
    std::fs::write(seg2.join("audio.flac"), b"audio").expect("audio2");
    std::fs::write(seg3.join("audio.flac"), b"audio").expect("audio3");
    std::fs::write(seg_live.join("live_sleep.flac"), b"audio").expect("live_sleep");

    let probe = Arc::new(TestMemoryProbe {
        available: std::sync::atomic::AtomicU64::new(0),
    });
    let (outbound, receiver) = mpsc::channel();
    let fixture = PathBuf::from(env!("CARGO_BIN_EXE_solstone-core-sense-test-handler"));
    let admission = solstone_core_sense::memory::Admission::new(probe.clone());
    let dispatcher = SenseDispatcher::new_with_fixture_program_and_admission(
        temp.path().to_path_buf(),
        false,
        false,
        outbound,
        fixture,
        admission,
    );

    dispatcher.handle(&make_observing_envelope(
        "20261010",
        "100000_1",
        &["audio.flac"],
        true,
        Some("imp1"),
    ));
    dispatcher.handle(&make_observing_envelope(
        "20261011",
        "110000_1",
        &["audio.flac"],
        true,
        Some("imp2"),
    ));
    dispatcher.handle(&make_observing_envelope(
        "20261012",
        "120000_1",
        &["audio.flac"],
        true,
        Some("imp3"),
    ));

    let start = std::time::Instant::now();
    loop {
        let event = receiver
            .recv_timeout(Duration::from_secs(5))
            .expect("throttle event");
        if event.event == "memory_throttle_started" {
            break;
        }
        if start.elapsed() > Duration::from_secs(10) {
            panic!("timed out waiting for memory_throttle_started");
        }
    }

    dispatcher.handle(&make_observing_envelope(
        "20261001",
        "010000_1",
        &["live_sleep.flac"],
        false,
        None,
    ));

    probe
        .available
        .store(10 * 1024 * 1024, std::sync::atomic::Ordering::SeqCst);

    let live_log = seg_live.join("live_sleep.flac.handler");
    let start = std::time::Instant::now();
    loop {
        if live_log.is_file() {
            let content = std::fs::read_to_string(&live_log).unwrap_or_default();
            if content.contains("SOLSTONE_WORK_PRIORITY=1:20261001010000") {
                break;
            }
        }
        if start.elapsed() > Duration::from_secs(10) {
            panic!("timed out waiting for live handler log");
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    assert!(!seg1.join("audio.flac.handler").exists());
    assert!(!seg2.join("audio.flac.handler").exists());
    assert!(!seg3.join("audio.flac.handler").exists());

    dispatcher.status();
    let status = loop {
        let event = receiver
            .recv_timeout(Duration::from_secs(2))
            .expect("status event");
        if event.event == "status" {
            break event;
        }
    };
    assert_eq!(status.fields["recent_error_count"], 0);
    assert_eq!(status.fields["pending_queue_depth"], 4);

    assert!(dispatcher.stop_and_wait());
}

#[cfg(feature = "full-tests")]
#[test]
fn memory_release_runs_live_work_and_leaves_the_import_eligible() {
    let temp = tempfile::tempdir().expect("temp journal");
    std::fs::create_dir_all(temp.path().join("config")).expect("config");
    std::fs::write(
        temp.path().join("config/journal.json"),
        br#"{"providers":{"active":{"provider":"openai"}},"transcribe":{"max_concurrent":2},"memory":{"floor_mib":1}}"#,
    )
    .expect("settings");

    let seg_a = temp.path().join("chronicle/20261010/stream1/100000_1");
    let seg_b = temp.path().join("chronicle/20261011/stream1/110000_1");
    let seg_live = temp.path().join("chronicle/20261001/stream1/010000_1");

    std::fs::create_dir_all(&seg_a).expect("seg_a");
    std::fs::create_dir_all(&seg_b).expect("seg_b");
    std::fs::create_dir_all(&seg_live).expect("seg_live");

    std::fs::write(seg_a.join("import_a_sleep.flac"), b"audio").expect("audio_a");
    std::fs::write(seg_b.join("import_b.flac"), b"audio").expect("audio_b");
    std::fs::write(seg_live.join("live_sleep.flac"), b"audio").expect("audio_live");

    let probe = Arc::new(TestMemoryProbe {
        available: std::sync::atomic::AtomicU64::new(10 * 1024 * 1024),
    });
    let (outbound, receiver) = mpsc::channel();
    let fixture = PathBuf::from(env!("CARGO_BIN_EXE_solstone-core-sense-test-handler"));
    let admission = solstone_core_sense::memory::Admission::new(probe.clone());
    let dispatcher = SenseDispatcher::new_with_fixture_program_and_admission(
        temp.path().to_path_buf(),
        false,
        false,
        outbound,
        fixture,
        admission,
    );

    dispatcher.handle(&make_observing_envelope(
        "20261010",
        "100000_1",
        &["import_a_sleep.flac"],
        true,
        Some("imp_a"),
    ));

    let log_a = seg_a.join("import_a_sleep.flac.handler");
    let start = std::time::Instant::now();
    loop {
        if log_a.is_file() {
            let content = std::fs::read_to_string(&log_a).unwrap_or_default();
            if content.contains("SOLSTONE_WORK_PRIORITY=4:") {
                break;
            }
        }
        if start.elapsed() > Duration::from_secs(10) {
            panic!("timed out waiting for import A handler log");
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    probe
        .available
        .store(0, std::sync::atomic::Ordering::SeqCst);

    dispatcher.handle(&make_observing_envelope(
        "20261011",
        "110000_1",
        &["import_b.flac"],
        true,
        Some("imp_b"),
    ));

    let log_b = seg_b.join("import_b.flac.handler");
    let start = std::time::Instant::now();
    loop {
        let event = receiver
            .recv_timeout(Duration::from_secs(5))
            .expect("event");
        if event.event == "memory_throttle_started" {
            break;
        }
        if start.elapsed() > Duration::from_secs(10) {
            panic!("timed out waiting for throttle on import B");
        }
    }
    assert!(!log_b.exists());

    dispatcher.handle(&make_observing_envelope(
        "20261001",
        "010000_1",
        &["live_sleep.flac"],
        false,
        None,
    ));

    probe
        .available
        .store(10 * 1024 * 1024, std::sync::atomic::Ordering::SeqCst);

    let live_log = seg_live.join("live_sleep.flac.handler");
    let start = std::time::Instant::now();
    loop {
        if live_log.is_file() {
            let content = std::fs::read_to_string(&live_log).unwrap_or_default();
            if content.contains("SOLSTONE_WORK_PRIORITY=1:") {
                break;
            }
        }
        if start.elapsed() > Duration::from_secs(10) {
            panic!("timed out waiting for live handler log");
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    assert!(!log_b.exists());
    let content_a = std::fs::read_to_string(&log_a).expect("read log A");
    assert_eq!(
        content_a
            .lines()
            .filter(|l| l.starts_with("SOLSTONE_WORK_PRIORITY=4:"))
            .count(),
        1
    );
    for event in receiver.try_iter() {
        if event.event == "observed" {
            assert_ne!(
                event.fields.get("segment").and_then(|v| v.as_str()),
                Some("110000_1")
            );
        }
    }

    dispatcher.status();
    let status = loop {
        let event = receiver
            .recv_timeout(Duration::from_secs(2))
            .expect("status event");
        if event.event == "status" {
            break event;
        }
    };
    assert_eq!(status.fields["recent_error_count"], 0);
    assert_eq!(status.fields["pending_queue_depth"], 3);

    assert!(dispatcher.stop_and_wait());
}

#[cfg(feature = "full-tests")]
#[test]
fn archive_file_bound_includes_lease_deferred_work_and_does_not_limit_live_admission() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(temp.path().join("config")).unwrap();
    std::fs::write(
        temp.path().join("config/journal.json"),
        br#"{"providers":{"active":{"provider":"openai"}},"memory":{"floor_mib":1}}"#,
    )
    .unwrap();
    let held = solstone_core_sense::lease::acquire_sense_day_lease(temp.path(), "20261010")
        .unwrap()
        .unwrap();
    let directory = temp.path().join("chronicle/20261010/stream1/100000_300");
    std::fs::create_dir_all(&directory).unwrap();
    let files: Vec<_> = (0..100).map(|index| format!("part-{index}.flac")).collect();
    for file in &files {
        std::fs::write(directory.join(file), b"audio").unwrap();
    }
    let probe = Arc::new(TestMemoryProbe {
        available: std::sync::atomic::AtomicU64::new(0),
    });
    let (outbound, receiver) = mpsc::channel();
    let dispatcher = SenseDispatcher::new_with_fixture_program_and_admission(
        temp.path().into(),
        false,
        false,
        outbound,
        PathBuf::from(env!("CARGO_BIN_EXE_solstone-core-sense-test-handler")),
        solstone_core_sense::memory::Admission::new(probe),
    );
    let refs: Vec<_> = files.iter().map(String::as_str).collect();
    let mut event =
        make_observing_envelope("20261010", "100000_300", &refs, true, Some("archive1"));
    event.extra.insert(
        "meta".into(),
        json!({"import_id":"archive1","audio_archive":true}),
    );
    dispatcher.handle(&event);
    assert_eq!(dispatcher.archive_pending_files(), 64);
    dispatcher.handle(&event);
    dispatcher.status();
    assert_eq!(
        dispatcher.archive_pending_files(),
        64,
        "a contended day is admitted once"
    );
    drop(held);
    dispatcher.status();
    assert_eq!(
        dispatcher.archive_pending_files(),
        64,
        "the bound survives lease acquisition"
    );
    let live = temp.path().join("chronicle/20261001/stream1/010000_1");
    std::fs::create_dir_all(&live).unwrap();
    std::fs::write(live.join("audio.flac"), b"audio").unwrap();
    dispatcher.handle(&make_observing_envelope(
        "20261001",
        "010000_1",
        &["audio.flac"],
        false,
        None,
    ));
    assert_eq!(dispatcher.archive_pending_files(), 64);
    for _ in receiver.try_iter() {}
    dispatcher.status();
    let status = loop {
        let event = receiver.recv_timeout(Duration::from_secs(2)).unwrap();
        if event.event == "status" {
            break event;
        }
    };
    assert_eq!(
        status.fields["pending_queue_depth"], 65,
        "live admission remains available"
    );
    assert_eq!(status.fields["archive_pending_queue_depth"], 64);
    assert!(dispatcher.stop_and_wait());
}
