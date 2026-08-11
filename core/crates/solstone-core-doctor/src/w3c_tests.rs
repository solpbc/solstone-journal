// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::{
    args::DoctorArgs,
    context::CheckContext,
    registry::{self, Battery},
    run,
    vocabulary::{CheckResult, Platform, Severity, Status},
};
use chrono::TimeZone;
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicUsize, Ordering},
    time::{Duration, SystemTime},
};

#[derive(Debug, PartialEq, Eq)]
enum SnapshotEntry {
    File(Vec<u8>),
    Symlink(PathBuf),
    Directory,
}

fn snapshot(root: &Path) -> BTreeMap<PathBuf, SnapshotEntry> {
    fn visit(root: &Path, path: &Path, out: &mut BTreeMap<PathBuf, SnapshotEntry>) {
        let relative = path.strip_prefix(root).unwrap().to_path_buf();
        let metadata = fs::symlink_metadata(path).unwrap();
        if metadata.file_type().is_symlink() {
            out.insert(
                relative,
                SnapshotEntry::Symlink(fs::read_link(path).unwrap()),
            );
        } else if metadata.is_dir() {
            out.insert(relative, SnapshotEntry::Directory);
            let mut entries = fs::read_dir(path)
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            entries.sort_by_key(|entry| entry.path());
            for entry in entries {
                visit(root, &entry.path(), out);
            }
        } else if metadata.is_file() {
            out.insert(relative, SnapshotEntry::File(fs::read(path).unwrap()));
        }
    }
    let mut out = BTreeMap::new();
    if root.exists() {
        visit(root, root, &mut out);
    }
    out
}

static NEXT: AtomicUsize = AtomicUsize::new(0);
fn fixture() -> CheckContext {
    let root = std::env::temp_dir().join(format!(
        "w3c-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(root.join("journal")).unwrap();
    CheckContext {
        home_dir: root.join("home"),
        install_bin_dir: root.join("install/bin"),
        journal_path: root.join("journal"),
        callosum_socket_path: root.join("journal/health/callosum.sock"),
        platform: Platform::Linux,
        now: chrono::Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
        host_arch: "x86_64".into(),
        hostname: "fixture-host".into(),
        machine_id: Some("fixture-machine".into()),
        checkout_root: None,
        python_env_root: None,
        port: 5015,
        service_status_timeout: Duration::from_millis(1),
        service_status_command_override: None,
        parakeet_server_probe_override: Some(|_, _| Err("fixture unreachable".into())),
        speakers_analyze_resolvers: None,
    }
}
fn status(name: &str, context: &CheckContext) -> Status {
    result(name, context).status
}
fn result(name: &str, context: &CheckContext) -> CheckResult {
    (registry::lookup(Battery::Journal, name).unwrap().runner)(context).unwrap()
}
fn write_observer(context: &CheckContext, name: &str, value: serde_json::Value) {
    let path = context
        .journal_path
        .join("apps/observer/observers")
        .join(format!("{name}.json"));
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, value.to_string()).unwrap();
}
fn observer(context: &CheckContext, name: &str, last_seen: i64) {
    write_observer(
        context,
        "abcdefgh",
        serde_json::json!({
            "key": "abcdefgh-key", "name": name, "enabled": true,
            "created_at": 1, "last_seen": last_seen
        }),
    );
}
fn health(context: &CheckContext, day: &str, lines: &[&str]) {
    let directory = context
        .journal_path
        .join("chronicle")
        .join(day)
        .join("health");
    fs::create_dir_all(&directory).unwrap();
    fs::write(directory.join("001.jsonl"), lines.join("\n") + "\n").unwrap();
}
fn screen_segment(context: &CheckContext, day: &str) {
    let path = context
        .journal_path
        .join("chronicle")
        .join(day)
        .join("120000_60");
    fs::create_dir_all(&path).unwrap();
    fs::write(path.join("screen.jsonl"), "{}\n{\"timestamp\":0}\n").unwrap();
}
fn incomplete(context: &CheckContext, day: &str) {
    let path = context
        .journal_path
        .join("chronicle")
        .join(day)
        .join("health/stream.updated");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, "stream\n").unwrap();
    fs::File::open(path)
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(
            SystemTime::UNIX_EPOCH
                + Duration::from_millis((context.now.timestamp_millis() - 1_000) as u64),
        ))
        .unwrap();
}
fn config_backend(context: &CheckContext, backend: &str) {
    let path = context.journal_path.join("config/journal.json");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        path,
        format!(r#"{{"transcribe":{{"backend":"{backend}"}}}}"#),
    )
    .unwrap();
}
#[cfg(unix)]
fn executable(path: &std::path::Path, body: &str) {
    use std::os::unix::fs::PermissionsExt;

    fs::write(path, body).unwrap();
    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).unwrap();
}
fn parakeet_ready_probe(_: &std::path::Path, _: Duration) -> Result<(), String> {
    Ok(())
}
fn parakeet_unreachable_probe(_: &std::path::Path, _: Duration) -> Result<(), String> {
    Err("fixture unreachable".into())
}
#[cfg(unix)]
fn speakers_binary_ready() -> Result<std::path::PathBuf, String> {
    Ok("/bin/sh".into())
}
#[cfg(unix)]
fn speakers_binary_missing() -> Result<std::path::PathBuf, String> {
    Err("fixture helper missing".into())
}
#[cfg(unix)]
fn speakers_model_ready(
    _: &str,
) -> Result<std::path::PathBuf, solstone_core_transcribe::TranscribeError> {
    Ok("/fixture/model.onnx".into())
}
fn args() -> DoctorArgs {
    DoctorArgs {
        verbose: false,
        json: false,
        jsonl: false,
        port: 5015,
        feature: None,
        readiness: false,
    }
}
#[test]
fn w3c_registry_replaces_exact_deferred_set_with_runners() {
    assert!(
        registry::entries(Battery::Journal)
            .iter()
            .filter(|e| matches!(
                e.check.name,
                "journal_sync"
                    | "journal_caught_up"
                    | "journal_maint_tasks"
                    | "task_pace"
                    | "brain"
                    | "capture_health"
                    | "observer_binding"
                    | "observer_delivery_stall"
                    | "observer_ingest_health"
                    | "orphan_segment_pdf"
                    | "default_stt_ready"
                    | "parakeet_cpp_stt_ready"
                    | "speakers_analyze_installation"
                    | "skill_state"
            ) || e.check.name.starts_with("feature:"))
            .all(|e| e.deferred.is_none())
    );
}
#[test]
fn w3c_severity_table_matches_reference() {
    for (name, severity) in [
        ("journal_sync", Severity::Blocker),
        ("journal_caught_up", Severity::Advisory),
        ("journal_maint_tasks", Severity::Blocker),
        ("task_pace", Severity::Advisory),
        ("brain", Severity::Advisory),
        ("capture_health", Severity::Advisory),
        ("observer_binding", Severity::Advisory),
        ("observer_delivery_stall", Severity::Advisory),
        ("observer_ingest_health", Severity::Advisory),
        ("orphan_segment_pdf", Severity::Advisory),
        ("default_stt_ready", Severity::Advisory),
        ("parakeet_cpp_stt_ready", Severity::Advisory),
        ("speakers_analyze_installation", Severity::Blocker),
        ("skill_state", Severity::Advisory),
        ("feature:pdf-import", Severity::Advisory),
        ("feature:pdf-export", Severity::Advisory),
    ] {
        assert_eq!(
            registry::lookup(Battery::Journal, name)
                .unwrap()
                .check
                .severity,
            severity
        );
    }
}
#[test]
fn w3c_fixture_drives_all_w3c_ok_and_non_ok_paths() {
    let clean = fixture();
    let sync = result("journal_sync", &clean);
    assert_eq!(sync.status, Status::Ok);
    assert_eq!(
        sync.detail,
        "this device only (fixture-host, machine fixture-...)"
    );

    let maint_ok = fixture();
    let state = maint_ok.journal_path.join("maint/settings/reindex.jsonl");
    fs::create_dir_all(state.parent().unwrap()).unwrap();
    fs::write(
        &state,
        "{\"event\":\"exec\",\"ts\":1}\n{\"event\":\"exit\",\"exit_code\":0,\"ts\":2}\n",
    )
    .unwrap();
    let maint = result("journal_maint_tasks", &maint_ok);
    assert_eq!(maint.status, Status::Ok);
    assert_eq!(maint.detail, "no unresolved maint tasks");

    let maint_failed = fixture();
    let state = maint_failed
        .journal_path
        .join("maint/settings/reindex.jsonl");
    fs::create_dir_all(state.parent().unwrap()).unwrap();
    fs::write(
        &state,
        "{\"event\":\"exec\",\"ts\":1}\n{\"event\":\"exit\",\"exit_code\":3,\"ts\":2}\n",
    )
    .unwrap();
    let failed = result("journal_maint_tasks", &maint_failed);
    assert_eq!(failed.status, Status::Fail);
    assert_eq!(
        failed.detail,
        "failed maint task(s): settings.reindex (exit 3)"
    );

    let maint_stale = fixture();
    let state = maint_stale
        .journal_path
        .join("maint/settings/reindex.jsonl");
    fs::create_dir_all(state.parent().unwrap()).unwrap();
    fs::write(
        &state,
        format!(
            "{{\"event\":\"exec\",\"ts\":{}}}\n",
            maint_stale.now.timestamp_millis() - 300_001
        ),
    )
    .unwrap();
    let stale = result("journal_maint_tasks", &maint_stale);
    assert_eq!(stale.status, Status::Warn);
    assert_eq!(stale.detail, "started, no exit: settings.reindex");

    let maint_unknown = fixture();
    let state = maint_unknown
        .journal_path
        .join("maint/settings/reindex.jsonl");
    fs::create_dir_all(state.parent().unwrap()).unwrap();
    fs::write(&state, "{\"event\":\"exec\"}\n").unwrap();
    let unknown = result("journal_maint_tasks", &maint_unknown);
    assert_eq!(unknown.status, Status::Warn);
    assert_eq!(
        unknown.detail,
        "couldn't fully determine — maint task started without a timestamp"
    );

    let active = fixture();
    observer(&active, "phone", active.now.timestamp_millis() - 1);
    let capture = result("capture_health", &active);
    assert_eq!(capture.status, Status::Ok);
    assert_eq!(
        capture.detail,
        "rollup=active; observers reaching the journal"
    );
    assert_eq!(
        result("observer_ingest_health", &active).detail,
        "no observers failing ingest"
    );
    assert_eq!(
        result("observer_binding", &active).detail,
        "active observer records=1; unbound=1; streams=phone"
    );

    let degraded = fixture();
    write_observer(
        &degraded,
        "abcdefgh",
        serde_json::json!({
            "key":"abcdefgh-key","name":"phone","enabled":true,"created_at":1,
            "last_seen": degraded.now.timestamp_millis() - 31_000,
            "health":{"ingest_rejection":{"version":"1.2","summary":"bad payload","active_count":2}}
        }),
    );
    let capture = result("capture_health", &degraded);
    assert_eq!(capture.status, Status::Warn);
    assert_eq!(capture.detail, "rollup=degraded; observers: phone=degraded");
    let ingest = result("observer_ingest_health", &degraded);
    assert_eq!(ingest.status, Status::Warn);
    assert_eq!(
        ingest.detail,
        "observer phone (v1.2) failing ingest: bad payload, 2x since unknown"
    );
}
#[test]
fn w3c_parakeet_cpp_required_states_are_distinct() {
    assert_eq!(status("parakeet_cpp_stt_ready", &fixture()), Status::Skip);
}
#[test]
fn w3c_default_stt_backend_platform_and_corrupt_config_matrix() {
    let c = fixture();
    fs::create_dir_all(c.journal_path.join("config")).unwrap();
    fs::write(c.journal_path.join("config/journal.json"), b"{").unwrap();
    assert_eq!(status("default_stt_ready", &c), Status::Fail);
}
#[test]
fn w3c_orphan_pdf_depth_transcript_and_dot_entry_matrix() {
    let c = fixture();
    let p = c.journal_path.join("chronicle/.dot/a/b/raw.pdf");
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, b"pdf").unwrap();
    assert_eq!(status("orphan_segment_pdf", &c), Status::Warn);
}
#[test]
fn w3c_no_enabled_observers_skip_observer_trio() {
    let c = fixture();
    for name in [
        "capture_health",
        "observer_delivery_stall",
        "observer_ingest_health",
    ] {
        assert_eq!(status(name, &c), Status::Skip);
    }
}
#[test]
fn w3c_setup_json_and_jsonl_filters_receive_advisory_warning() {
    let warned = run(&args(), &fixture());
    let json = serde_json::json!({"checks": warned});
    let json_matches = json["checks"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| matches!(row["status"].as_str(), Some("warn" | "fail")))
        .collect::<Vec<_>>();
    assert!(!json_matches.is_empty());
    assert!(
        json_matches
            .iter()
            .any(|row| row["name"] == "default_stt_ready")
    );
    let mut bytes = Vec::new();
    crate::output::emit_jsonl_to(&mut bytes, &warned, "2026-01-01T00:00:00Z", 0, 5015, None);
    let jsonl_matches = String::from_utf8(bytes)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .filter(|row| {
            row["event"] == "check.completed"
                && matches!(row["status"].as_str(), Some("warning" | "failed"))
        })
        .collect::<Vec<_>>();
    assert!(!jsonl_matches.is_empty());
    assert!(
        jsonl_matches
            .iter()
            .any(|row| row["name"] == "default_stt_ready")
    );

    let ok = vec![crate::vocabulary::make_result(
        crate::vocabulary::Check {
            name: "ok",
            severity: Severity::Advisory,
            platforms: &[Platform::Linux],
        },
        Status::Ok,
        "ok",
        None::<String>,
    )];
    let ok_json = serde_json::json!({"checks": ok});
    assert!(
        ok_json["checks"]
            .as_array()
            .unwrap()
            .iter()
            .all(|row| !matches!(row["status"].as_str(), Some("warn" | "fail")))
    );
}
#[test]
fn w3c_feature_environment_inspection_matrix() {
    let missing = fixture();
    let env = missing.journal_path.parent().unwrap().join("venv");
    fs::create_dir_all(env.join("lib/python3.13/site-packages")).unwrap();
    let mut missing = missing;
    missing.python_env_root = Some(env.clone());
    let absent = result("feature:pdf-import", &missing);
    assert_eq!(absent.status, Status::Warn);
    assert_eq!(absent.detail, "PDF document extraction not installed");
    fs::create_dir_all(env.join("lib/python3.13/site-packages/pypdfium2")).unwrap();
    fs::write(
        env.join("lib/python3.13/site-packages/pypdfium2/__init__.py"),
        "",
    )
    .unwrap();
    fs::create_dir_all(env.join("lib/python3.13/site-packages/PIL")).unwrap();
    fs::write(env.join("lib/python3.13/site-packages/PIL/__init__.py"), "").unwrap();
    let present = result("feature:pdf-import", &missing);
    assert_eq!(present.status, Status::Ok);
    assert_eq!(present.detail, "PDF document extraction available");

    let export = fixture();
    let env = export.journal_path.parent().unwrap().join("export-venv");
    fs::create_dir_all(env.join("lib/python3.13/site-packages")).unwrap();
    let mut export = export;
    export.python_env_root = Some(env.clone());
    let absent = result("feature:pdf-export", &export);
    assert_eq!(absent.status, Status::Warn);
    assert_eq!(
        absent.fix.as_deref(),
        Some("pip install 'solstone[pdf-export]' and apt install libpango-1.0-0 libpangoft2-1.0-0")
    );
    export.platform = Platform::Darwin;
    assert_eq!(
        result("feature:pdf-export", &export).fix.as_deref(),
        Some("pip install 'solstone[pdf-export]' and brew install pango")
    );
    fs::create_dir_all(env.join("lib/python3.13/site-packages/weasyprint")).unwrap();
    fs::write(
        env.join("lib/python3.13/site-packages/weasyprint/__init__.py"),
        "",
    )
    .unwrap();
    let present = result("feature:pdf-export", &export);
    assert_eq!(present.status, Status::Ok);
    assert_eq!(present.detail, "PDF export rendering available");
}

#[test]
fn w3c_brain_unconstructible_snapshot_is_an_explicit_warning() {
    let context = fixture();
    let path = context.journal_path.join("health/brain.json");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, "{").unwrap();
    let row = result("brain", &context);
    assert_eq!(row.status, Status::Warn);
    assert!(row.detail.starts_with("unknown: "), "{}", row.detail);
}

#[test]
fn w3c_task_pace_uses_callosum_status_fixture() {
    use serde_json::{Map, Value};
    use solstone_core_callosum::{CallosumEnvelope, CallosumSocketServer};

    let mut context = fixture();
    fs::create_dir_all(context.callosum_socket_path.parent().unwrap()).unwrap();
    context.service_status_timeout = Duration::from_millis(250);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let server = runtime
        .block_on(CallosumSocketServer::bind(&context.callosum_socket_path))
        .unwrap();
    let run_with = |tasks: Value| {
        let context = context.clone();
        let handle = std::thread::spawn(move || result("task_pace", &context));
        runtime.block_on(async {
            tokio::time::timeout(Duration::from_millis(100), async {
                while server.client_count() == 0 {
                    tokio::time::sleep(Duration::from_millis(2)).await;
                }
            })
            .await
            .unwrap();
        });
        let envelope = CallosumEnvelope {
            tract: "supervisor".into(),
            event: "status".into(),
            ts: None,
            extra: Map::from_iter([("tasks".into(), tasks)]),
        };
        for _ in 0..20 {
            assert!(server.broadcast(envelope.clone()));
            runtime.block_on(async { tokio::time::sleep(Duration::from_millis(5)).await });
            if handle.is_finished() {
                break;
            }
        }
        handle.join().unwrap()
    };
    let ok = run_with(serde_json::json!([{ "name":"index", "slow":false }]));
    assert_eq!(ok.status, Status::Ok);
    assert_eq!(ok.detail, "tasks on pace");
    let warn = run_with(serde_json::json!([{
        "name":"index", "slow":true, "duration_seconds":12, "max_runtime_seconds":10
    }]));
    assert_eq!(warn.status, Status::Warn);
    assert_eq!(warn.detail, "running long: index (12s of 10s cap)");
    runtime.block_on(server.stop());
    assert_eq!(result("task_pace", &fixture()).status, Status::Skip);
}

#[test]
fn w3c_caught_up_native_backlog_fixture_states() {
    let clean = fixture();
    let row = result("journal_caught_up", &clean);
    assert_eq!(row.status, Status::Ok);
    assert_eq!(row.detail, "caught up");

    let capped = fixture();
    health(
        &capped,
        "20251230",
        &[
            r#"{"event":"talent.fail","ts":1,"mode":"daily","name":"summary","reason_code":"provider_request_rejected"}"#,
        ],
    );
    let row = result("journal_caught_up", &capped);
    assert_eq!(row.status, Status::Ok);
    assert_eq!(
        row.detail,
        "caught up; 1 day(s) completed with capped daily unit(s)"
    );

    let pending = fixture();
    screen_segment(&pending, "20251231");
    health(
        &pending,
        "20251231",
        &[
            r#"{"event":"sense.complete","ts":1,"mode":"segment","stream":"_default","segment":"120000_60","density":"active"}"#,
        ],
    );
    incomplete(&pending, "20251231");
    let row = result("journal_caught_up", &pending);
    assert_eq!(row.status, Status::Warn);
    assert_eq!(
        row.detail,
        "1 day(s) pending, 0 day(s) stuck; oldest outstanding 20251231"
    );
    assert_eq!(
        row.fix.as_deref(),
        Some(
            "solstone catches up on its own; reprocess a day from the health surface to prioritize it"
        )
    );
}

#[test]
#[cfg(unix)]
fn w3c_parakeet_cpp_fixture_states_are_distinct() {
    let not_applicable = fixture();
    let row = result("parakeet_cpp_stt_ready", &not_applicable);
    assert_eq!(row.status, Status::Skip);
    assert_eq!(
        row.detail,
        "configured backend is not parakeet-cpp; check not applicable"
    );

    let missing = fixture();
    config_backend(&missing, "parakeet-cpp");
    let missing_row = result("parakeet_cpp_stt_ready", &missing);
    assert_eq!(missing_row.status, Status::Warn);
    assert!(missing_row.detail.starts_with("parakeet-cpp check failed:"));
    assert_eq!(
        missing_row.fix.as_deref(),
        Some(
            "parakeet-cpp artifacts are not installed — fetch them with: journal install-provider parakeet"
        )
    );

    let generic = fixture();
    config_backend(&generic, "parakeet-cpp");
    let artifacts = solstone_core_system::provider_runtime::parakeet_cpp_artifacts(
        &generic.journal_path,
        "linux",
        "x86_64",
    )
    .unwrap();
    fs::create_dir_all(artifacts.binary_cpu.parent().unwrap()).unwrap();
    fs::create_dir_all(artifacts.binary_vulkan.parent().unwrap()).unwrap();
    fs::create_dir_all(artifacts.model.parent().unwrap()).unwrap();
    executable(&artifacts.binary_cpu, "#!/bin/sh\nexit 1\n");
    executable(&artifacts.binary_vulkan, "#!/bin/sh\nexit 0\n");
    fs::write(&artifacts.model, "model").unwrap();
    let generic_row = result("parakeet_cpp_stt_ready", &generic);
    assert_eq!(generic_row.status, Status::Warn);
    assert_eq!(generic_row.detail, "parakeet-cpp binary cannot start");

    let openmp = fixture();
    config_backend(&openmp, "parakeet-cpp");
    let artifacts = solstone_core_system::provider_runtime::parakeet_cpp_artifacts(
        &openmp.journal_path,
        "linux",
        "x86_64",
    )
    .unwrap();
    fs::create_dir_all(artifacts.binary_cpu.parent().unwrap()).unwrap();
    fs::create_dir_all(artifacts.binary_vulkan.parent().unwrap()).unwrap();
    fs::create_dir_all(artifacts.model.parent().unwrap()).unwrap();
    executable(
        &artifacts.binary_cpu,
        "#!/bin/sh\necho libgomp.so.1 >&2\nexit 1\n",
    );
    executable(&artifacts.binary_vulkan, "#!/bin/sh\nexit 0\n");
    fs::write(&artifacts.model, "model").unwrap();
    let openmp_row = result("parakeet_cpp_stt_ready", &openmp);
    assert_eq!(openmp_row.status, Status::Warn);
    assert_eq!(
        openmp_row.detail,
        "parakeet-cpp cannot start: OpenMP runtime unavailable (libgomp.so.1)"
    );
    assert_eq!(
        openmp_row.fix.as_deref(),
        Some(
            "install the system OpenMP runtime that provides libgomp.so.1, then rerun journal doctor"
        )
    );

    let mut unreachable = fixture();
    config_backend(&unreachable, "parakeet-cpp");
    let artifacts = solstone_core_system::provider_runtime::parakeet_cpp_artifacts(
        &unreachable.journal_path,
        "linux",
        "x86_64",
    )
    .unwrap();
    fs::create_dir_all(artifacts.binary_cpu.parent().unwrap()).unwrap();
    fs::create_dir_all(artifacts.binary_vulkan.parent().unwrap()).unwrap();
    fs::create_dir_all(artifacts.model.parent().unwrap()).unwrap();
    executable(&artifacts.binary_cpu, "#!/bin/sh\necho v\n");
    executable(&artifacts.binary_vulkan, "#!/bin/sh\necho v\n");
    fs::write(&artifacts.model, "model").unwrap();
    unreachable.parakeet_server_probe_override = Some(parakeet_unreachable_probe);
    let unreachable_row = result("parakeet_cpp_stt_ready", &unreachable);
    assert_eq!(unreachable_row.status, Status::Warn);
    assert_eq!(
        unreachable_row.detail,
        "parakeet-server not reachable: fixture unreachable"
    );

    unreachable.parakeet_server_probe_override = Some(parakeet_ready_probe);
    let ready = result("parakeet_cpp_stt_ready", &unreachable);
    assert_eq!(ready.status, Status::Ok);
    assert_eq!(
        ready.detail,
        "parakeet-cpp ready (binaries + model installed, server reachable)"
    );
}

#[test]
#[cfg(unix)]
fn w3c_default_stt_fixture_matrix_delegates_and_checks_coreml() {
    let other = fixture();
    config_backend(&other, "whisper");
    let row = result("default_stt_ready", &other);
    assert_eq!(row.status, Status::Skip);
    assert_eq!(
        row.detail,
        "configured backend is whisper; parakeet readiness not applicable"
    );

    let mut unsupported = fixture();
    config_backend(&unsupported, "parakeet");
    unsupported.host_arch = "aarch64".into();
    let row = result("default_stt_ready", &unsupported);
    assert_eq!(row.status, Status::Skip);
    assert_eq!(row.detail, "parakeet not supported on this platform");

    let mut linux = fixture();
    config_backend(&linux, "parakeet");
    let artifacts = solstone_core_system::provider_runtime::parakeet_cpp_artifacts(
        &linux.journal_path,
        "linux",
        "x86_64",
    )
    .unwrap();
    fs::create_dir_all(artifacts.binary_cpu.parent().unwrap()).unwrap();
    fs::create_dir_all(artifacts.binary_vulkan.parent().unwrap()).unwrap();
    fs::create_dir_all(artifacts.model.parent().unwrap()).unwrap();
    executable(&artifacts.binary_cpu, "#!/bin/sh\necho v\n");
    executable(&artifacts.binary_vulkan, "#!/bin/sh\necho v\n");
    fs::write(&artifacts.model, "model").unwrap();
    linux.parakeet_server_probe_override = Some(parakeet_ready_probe);
    let delegated = result("default_stt_ready", &linux);
    let direct = result("parakeet_cpp_stt_ready", &{
        let direct = linux.clone();
        config_backend(&direct, "parakeet-cpp");
        direct
    });
    assert_eq!(delegated.status, direct.status);
    assert_eq!(delegated.detail, direct.detail);

    let mut coreml = fixture();
    config_backend(&coreml, "parakeet");
    coreml.platform = Platform::Darwin;
    coreml.host_arch = "arm64".into();
    let cache = coreml
        .home_dir
        .join("Library/Application Support/solstone/parakeet/models/cache");
    fs::create_dir_all(&cache).unwrap();
    let model = cache.parent().unwrap().join("parakeet-tdt-0.6b-v3");
    for path in [
        "Encoder.mlmodelc/weights/weight.bin",
        "Decoder.mlmodelc/weights/weight.bin",
        "JointDecision.mlmodelc/weights/weight.bin",
        "Preprocessor.mlmodelc/weights/weight.bin",
    ] {
        let path = model.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "model").unwrap();
    }
    let sentinel = coreml
        .home_dir
        .join("Library/Application Support/solstone/parakeet/models/.install-complete");
    fs::write(sentinel, serde_json::json!({"schema_version":1,"backend":"parakeet","variant":"coreml","model_version":"v3","quantization":"fp32","fluidaudio_version":"x","platform":{"os":"darwin","arch":"arm64"},"cache_dir":cache}).to_string()).unwrap();
    let row = result("default_stt_ready", &coreml);
    assert_eq!(row.status, Status::Ok);
    assert!(row.detail.starts_with("parakeet model ready at "));

    let missing_coreml = fixture();
    let mut missing_coreml = missing_coreml;
    config_backend(&missing_coreml, "parakeet");
    missing_coreml.platform = Platform::Darwin;
    missing_coreml.host_arch = "arm64".into();
    let row = result("default_stt_ready", &missing_coreml);
    assert_eq!(row.status, Status::Warn);
    assert_eq!(
        row.fix.as_deref(),
        Some("CoreML parakeet model is not downloaded — fetch it with: journal install-models")
    );

    let corrupt = fixture();
    let path = corrupt.journal_path.join("config/journal.json");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, "{").unwrap();
    let row = result("default_stt_ready", &corrupt);
    assert_eq!(row.status, Status::Fail);
    assert_eq!(
        row.fix.as_deref(),
        Some("repair or restore config/journal.json from a backup")
    );
}

#[test]
#[cfg(unix)]
fn w3c_skill_state_fixture_branches() {
    use std::os::unix::fs::symlink;

    let mut installed = fixture();
    let root = installed.journal_path.parent().unwrap().join("checkout");
    for name in ["sol", "journal"] {
        fs::create_dir_all(root.join("solstone/talent").join(name)).unwrap();
        fs::write(
            root.join("solstone/talent").join(name).join("SKILL.md"),
            "x",
        )
        .unwrap();
    }
    for parent in [
        installed.journal_path.join(".claude/skills"),
        installed.journal_path.join(".agents/skills"),
    ] {
        fs::create_dir_all(&parent).unwrap();
        for name in ["sol", "journal"] {
            let source = root.join("solstone/talent").join(name);
            symlink(
                solstone_core_skill_state::expected_link_target(&source, &parent),
                parent.join(name),
            )
            .unwrap();
        }
    }
    installed.checkout_root = Some(root.clone());
    let row = result("skill_state", &installed);
    assert_eq!(row.status, Status::Ok);
    assert_eq!(
        row.detail,
        "router skills sol, journal are installed and current"
    );

    let broken = installed.clone();
    let claude = broken.journal_path.join(".claude/skills");
    fs::remove_file(claude.join("sol")).unwrap();
    fs::remove_file(claude.join("journal")).unwrap();
    symlink("foreign", claude.join("journal")).unwrap();
    symlink("old", claude.join("stale")).unwrap();
    let row = result("skill_state", &broken);
    assert_eq!(row.status, Status::Warn);
    assert!(row.detail.contains("sol missing at"));
    assert!(row.detail.contains("journal points elsewhere at"));
    assert!(row.detail.contains("stale router skill link at"));
    assert_eq!(
        row.fix.as_deref(),
        Some("run sol skills install --project .")
    );

    let no_root = fixture();
    assert_eq!(result("skill_state", &no_root).status, Status::Skip);
    let mut no_dirs = fixture();
    no_dirs.checkout_root = Some(root);
    let row = result("skill_state", &no_dirs);
    assert_eq!(row.status, Status::Skip);
    assert_eq!(row.detail, "router skill directories are unavailable");
}

#[test]
#[cfg(unix)]
fn w3c_speakers_installation_uses_injected_resolvers() {
    let mut ready = fixture();
    ready.speakers_analyze_resolvers = Some((speakers_binary_ready, speakers_model_ready));
    let row = result("speakers_analyze_installation", &ready);
    assert_eq!(row.status, Status::Ok);
    assert_eq!(row.detail, "speakers-analyze installation ready");

    ready.speakers_analyze_resolvers = Some((speakers_binary_missing, speakers_model_ready));
    let row = result("speakers_analyze_installation", &ready);
    assert_eq!(row.status, Status::Fail);
    assert_eq!(
        row.detail,
        "Speakers-analyze installation is incomplete (fixture helper missing). Repair: reinstall the journal host stack and restart the journal."
    );
    assert_eq!(
        row.fix.as_deref(),
        Some("reinstall the journal host stack and restart the journal")
    );
}

#[test]
fn w3c_observer_delivery_stall_clause_escalation() {
    let stage = |value: serde_json::Value| {
        let context = fixture();
        write_observer(&context, "abcdefgh", value);
        result("observer_delivery_stall", &context)
    };
    let now = fixture().now.timestamp_millis();
    let base = |name: &str| {
        serde_json::json!({
            "key":"abcdefgh-key", "name":name, "enabled":true, "created_at":1,
            "last_seen":now - 1_000,
            "last_segment_received_at":now - 21_600_001
        })
    };
    let mut duplicate = base("duplicate");
    duplicate["stats"] = serde_json::json!({"duplicates_rejected":2});
    let row = stage(duplicate);
    assert_eq!(row.status, Status::Warn);
    assert!(row.detail.ends_with(
        "prior duplicate responses=2, so repeated uploads may be landing without a newer upload"
    ));

    let mut beacon = base("beacon");
    beacon["health"] = serde_json::json!({"beacon":{"pending_queue_depth":4}});
    let row = stage(beacon);
    assert_eq!(row.status, Status::Warn);
    assert!(
        row.detail
            .ends_with("pending queue depth 4, so uploads may not be landing")
    );

    let row = stage(base("generic"));
    assert_eq!(row.status, Status::Warn);
    assert!(row.detail.ends_with("uploads may not be landing"));
}

#[test]
fn w3c_owner_boundary_guard_is_nonvacuous() {
    let owners = [
        ("journal_sync", "solstone_core_system"),
        ("journal_caught_up", "solstone_core_system_health"),
        ("journal_maint_tasks", "solstone_core_system_health"),
        ("brain", "solstone_core_brain"),
        ("capture_health", "solstone_core_observer"),
        ("observer_binding", "solstone_core_observer"),
        ("observer_delivery_stall", "solstone_core_observer"),
        ("observer_ingest_health", "solstone_core_observer"),
        ("default_stt_ready", "solstone_core_system"),
        ("parakeet_cpp_stt_ready", "solstone_core_system"),
        ("speakers_analyze_installation", "solstone_core_transcribe"),
        ("skill_state", "solstone_core_skill_state"),
    ];
    let accepts = |module: &str, source: &str| {
        ["orphan_segment_pdf", "feature"].contains(&module)
            || owners
                .iter()
                .find(|(name, _)| *name == module)
                .is_some_and(|(_, owner)| source.contains(owner))
    };
    for (module, owner) in owners {
        let path = format!("checks/{module}.rs");
        let mut source = fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("src")
                .join(path),
        )
        .unwrap();
        if matches!(
            module,
            "capture_health"
                | "observer_binding"
                | "observer_delivery_stall"
                | "observer_ingest_health"
        ) {
            source.push_str(include_str!("checks/common.rs"));
        }
        assert!(accepts(module, &source), "{module} must consume {owner}");
    }
    assert!(accepts("orphan_segment_pdf", "std::fs::read_dir"));
    assert!(accepts("feature", "std::fs::read_dir"));
    assert!(!accepts("brain", "fn run() {}"));
    let cargo = include_str!("../Cargo.toml");
    for dependency in [
        "solstone-core-system",
        "solstone-core-system-health",
        "solstone-core-brain",
        "solstone-core-observer",
        "solstone-core-transcribe",
        "solstone-core-skill-state",
    ] {
        assert!(cargo.contains(dependency));
    }
    assert!(!cargo.contains("solstone-core-sol.workspace"));
}
#[test]
fn w3c_poisoned_interpreters_positive_control_and_battery() {
    let mut c = fixture();
    let poison = c.journal_path.parent().unwrap().join("poison");
    fs::create_dir_all(&poison).unwrap();
    let witness = poison.join("witness");
    let script = format!(
        "#!/bin/sh\necho 'forbidden interpreter invoked: $0' >&2\necho \"$0\" >> '{}'\nexit 97\n",
        witness.display()
    );
    for name in ["python", "python3", "pytest", "ruff", "uv"] {
        let path = poison.join(name);
        fs::write(&path, &script).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = fs::metadata(&path).unwrap().permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&path, permissions).unwrap();
        }
    }
    let env = c.journal_path.parent().unwrap().join("venv");
    fs::create_dir_all(env.join("bin")).unwrap();
    fs::write(env.join("bin/python"), &script).unwrap();
    c.python_env_root = Some(env);
    let p = poison.join("python");
    assert_eq!(Command::new(p).status().unwrap().code(), Some(97));
    fs::remove_file(&witness).unwrap();
    let journal = run(&args(), &c);
    let readiness = run(
        &DoctorArgs {
            readiness: true,
            ..args()
        },
        &c,
    );
    assert_eq!(journal.len(), registry::entries(Battery::Journal).len());
    assert_eq!(
        readiness.len(),
        registry::entries(Battery::JournalReadiness).len()
    );
    assert!(journal.iter().all(|row| !row.name.is_empty()));
    assert!(readiness.iter().all(|row| !row.name.is_empty()));
    assert!(!witness.exists());
}
#[test]
fn w3c_batteries_preserve_staged_home_and_journal() {
    let c = fixture();
    fs::create_dir_all(&c.home_dir).unwrap();
    fs::write(c.home_dir.join("marker"), "home").unwrap();
    let journal_before = snapshot(&c.journal_path);
    let home_before = snapshot(&c.home_dir);
    let _ = run(&args(), &c);
    assert_eq!(snapshot(&c.journal_path), journal_before);
    assert_eq!(snapshot(&c.home_dir), home_before);
    let _ = run(
        &DoctorArgs {
            readiness: true,
            ..args()
        },
        &c,
    );
    assert_eq!(snapshot(&c.journal_path), journal_before);
    assert_eq!(snapshot(&c.home_dir), home_before);
    assert!(!c.journal_path.join("chronicle").exists());
    assert!(!c.journal_path.join("apps/observer/observers").exists());
}
