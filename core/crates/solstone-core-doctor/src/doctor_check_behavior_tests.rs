// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::{
    args::DoctorArgs,
    context::CheckContext,
    output,
    registry::{self, Battery},
    run,
    vocabulary::{CheckResult, ClientRegistryState, Platform, Severity, Status},
};
use chrono::TimeZone;
use solstone_core_sol_link::client_status::{
    ClientActivityState, ClientInspection, ClientLedgerUnavailable,
};
use solstone_core_system_health::MODALITY_INPUT_AGED_MS;
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
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

// W3C names are standards-body vocabulary, not project-phase identifiers.
const W3C_CHECK_NAMES: &[&str] = &[
    "journal_sync",
    "journal_caught_up",
    "task_pace",
    "brain",
    "capture_health",
    "client_binding",
    "client_delivery_stall",
    "client_ingest_health",
    "client_transport_refusal",
    "orphan_segment_pdf",
    "default_stt_ready",
    "parakeet_cpp_stt_ready",
    "speakers_analyze_installation",
    "vad_runtime_ready",
    "skill_state",
    "unretryable_transcribe_input",
];

const BASELINE_CHECK_NAMES: &[&str] = &[
    "config_dir_readable",
    "journal_dir_writable",
    "supervisor_conflict",
    "service_running",
    "launchd_stale_plist",
];

// The earlier check set landed before this branch. These names carry `deferred:
// None` now and so are indistinguishable from the baseline rows in the
// registry — the classification cannot be derived after the fact and has to be
// written down to keep the partition assertion below self-policing.
const EARLIER_CHECK_NAMES: &[&str] = &[
    "disk_space",
    "service_identity",
    "local_bin_solstone_reachable",
];

fn fixture() -> CheckContext {
    let root = std::env::temp_dir().join(format!(
        "check-{}-{}",
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
        checkout_root: None,
        payload_root: None,
        port: 5015,
        service_status_timeout: Duration::from_millis(1),
        service_status_command_override: None,
        parakeet_server_probe_override: Some(|_, _| Err("fixture unreachable".into())),
        speakers_analyze_resolvers: None,
        vad_runtime_probe: None,
        free_space_bytes_override: None,
    }
}
fn status(name: &str, context: &CheckContext) -> Status {
    result(name, context).status
}
fn result(name: &str, context: &CheckContext) -> CheckResult {
    let entry = registry::lookup(Battery::Journal, name).unwrap();
    if name == "brain" {
        return crate::checks::brain::run_with_clock(context, entry.check, || context.now).unwrap();
    }
    (entry.runner)(context).unwrap()
}
fn write_client_fixture(context: &CheckContext, name: &str, value: serde_json::Value) {
    if value.get("enabled").and_then(serde_json::Value::as_bool) == Some(false)
        || value.get("revoked").and_then(serde_json::Value::as_bool) == Some(true)
    {
        return;
    }
    let link = context.journal_path.join("link");
    fs::create_dir_all(&link).unwrap();
    let authorization_path = link.join("authorized_clients.json");
    let activity_path = link.join("devices.json");
    let cid = name.to_owned();
    let label = value
        .get("name")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown");
    let mut clients = fs::read(&authorization_path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Vec<serde_json::Value>>(&bytes).ok())
        .unwrap_or_default();
    clients
        .retain(|entry| entry.get("fingerprint").and_then(serde_json::Value::as_str) != Some(&cid));
    clients.push(serde_json::json!({
        "fingerprint": cid,
        "device_label": label,
        "paired_at": "2026-01-01T00:00:00Z",
        "instance_id": "fixture",
        "kind": "cert",
    }));
    fs::write(&authorization_path, serde_json::to_vec(&clients).unwrap()).unwrap();

    let mut activity = fs::read(&activity_path)
        .ok()
        .and_then(|bytes| {
            serde_json::from_slice::<serde_json::Map<String, serde_json::Value>>(&bytes).ok()
        })
        .unwrap_or_default();
    let rfc3339 = |timestamp| {
        chrono::Utc
            .timestamp_millis_opt(timestamp)
            .single()
            .expect("fixture timestamp")
            .to_rfc3339()
    };
    let last_seen = value
        .get("last_seen")
        .and_then(serde_json::Value::as_i64)
        .unwrap_or_else(|| context.now.timestamp_millis());
    let mut entry = serde_json::json!({"last_seen_at": rfc3339(last_seen)});
    if let Some(last_accepted) = value
        .get("last_segment_received_at")
        .and_then(serde_json::Value::as_i64)
    {
        entry["last_accepted_ingest_at"] = rfc3339(last_accepted).into();
        entry["last_accepted_segment"] = serde_json::json!({"day": "20260101", "name": "fixture"});
    }
    if let Some(sources) = value.get("sources") {
        entry["sources"] = sources.clone();
    }
    if let Some(rejection) = value
        .get("health")
        .and_then(|health| health.get("ingest_rejection"))
    {
        let first = rejection
            .get("first_ts")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or_else(|| context.now.timestamp_millis());
        let latest = rejection
            .get("latest_ts")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or_else(|| context.now.timestamp_millis());
        entry["ingest_rejection"] = serde_json::json!({
            "reason_code": rejection.get("reason_code").and_then(serde_json::Value::as_str).unwrap_or("ingest_rejected"),
            "first": rfc3339(first),
            "latest": rfc3339(latest),
            "active_count": rejection.get("active_count").and_then(serde_json::Value::as_u64).unwrap_or(1),
        });
    }
    if let Some(refusal) = value
        .get("health")
        .and_then(|health| health.get("transport_refusal"))
    {
        let at = rfc3339(context.now.timestamp_millis());
        entry["transport_refusal"] = serde_json::json!({
            "reason_code": refusal.get("reason_code").and_then(serde_json::Value::as_str).unwrap_or("stream_limit"),
            "first": at,
            "latest": at,
            "active_count": refusal.get("active_count").and_then(serde_json::Value::as_u64).unwrap_or(1),
        });
    }
    activity.insert(name.to_owned(), entry);
    fs::write(&activity_path, serde_json::to_vec(&activity).unwrap()).unwrap();
}
fn write_unassessed_client(context: &CheckContext, name: &str, last_seen: i64) {
    write_client_fixture(
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
fn stage_brain_ready(context: &CheckContext) {
    let path = context.journal_path.join("health/brain.json");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let observed_at = "2025-12-31T23:59:00+00:00";
    let record = serde_json::json!({
        "schema_version": 1,
        "revision": 3,
        "aggregate_state": "ready",
        "reason_code": null,
        "active_lane": "none",
        "active_provider": "none",
        "active_model": null,
        "fingerprint_sha256": null,
        "checking": null,
        "evidence": {
            "configuration": {
                "status": "ok",
                "observed_at": observed_at,
                "expires_at": "2026-01-02T00:00:00+00:00"
            },
            "lane_prerequisites": null,
            "generate": null,
            "cogitate": null
        },
        "runtime_failure_marker": null,
        "diagnostic": {},
        "updated_at": observed_at,
    });
    fs::write(path, record.to_string()).unwrap();
}

fn stage_brain_checking(context: &CheckContext) -> solstone_core_brain::BrainRefreshPermit {
    let path = context.journal_path.join("config/journal.json");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, r#"{"providers":{"active":{"provider":"anthropic"}}}"#).unwrap();
    solstone_core_brain::begin_refresh(
        &context.journal_path,
        context.now,
        Some("0123456789abcdef".into()),
        None,
        false,
        None,
    )
    .unwrap()
    .expect("configured cloud provider starts a refresh and holds its lease")
}
#[cfg(unix)]
fn executable(path: &std::path::Path, body: &str) {
    use std::os::unix::fs::PermissionsExt;

    let name = path.file_name().expect("executable name");
    let staging = path.with_file_name(format!("{}.staging", name.to_string_lossy()));
    fs::write(&staging, body).unwrap();
    let mut permissions = fs::metadata(&staging).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&staging, permissions).unwrap();
    fs::rename(&staging, path).unwrap();
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
        readiness: false,
    }
}

fn configure_daily_work(root: &Path, enabled: Option<&str>) {
    let (talent, apps) = solstone_core_system::daily_coverage::package_roots().unwrap();
    let overrides = solstone_core_system::daily_coverage::daily_configs(root, &talent, &apps)
        .unwrap()
        .into_iter()
        .map(|config| {
            let key = match config.key.split_once(':') {
                Some((app, name)) => format!("talent.{app}.{name}"),
                None => format!("talent.system.{}", config.key),
            };
            (
                key,
                serde_json::json!({"disabled": enabled != Some(config.key.as_str())}),
            )
        })
        .collect::<serde_json::Map<String, serde_json::Value>>();
    fs::create_dir_all(root.join("config")).unwrap();
    fs::write(
        root.join("config/journal.json"),
        serde_json::to_vec(&serde_json::json!({
            "identity":{"timezone":"UTC"}, "talent_overrides": overrides
        }))
        .unwrap(),
    )
    .unwrap();
}

fn stage_backlog_pending(context: &CheckContext) {
    // This fixture isolates segment backlog rather than daily evidence parsing.
    configure_daily_work(&context.journal_path, None);
    screen_segment(context, "20251231");
    health(
        context,
        "20251231",
        &[
            r#"{"event":"sense.complete","ts":1,"mode":"segment","stream":"_default","segment":"120000_60","density":"active"}"#,
        ],
    );
    incomplete(context, "20251231");
}

fn stage_raw_audio_pending(context: &CheckContext, modified: SystemTime) {
    // Isolate the raw-audio grace period from historical daily coverage.
    configure_daily_work(&context.journal_path, None);
    let path = context
        .journal_path
        .join("chronicle")
        .join("20251231")
        .join("120000_60");
    fs::create_dir_all(&path).unwrap();
    let audio = path.join("audio.wav");
    fs::write(&audio, b"fixture").unwrap();
    fs::File::open(&audio)
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(modified))
        .unwrap();
    incomplete(context, "20251231");
}

#[cfg(unix)]
fn stage_parakeet_ready(context: &mut CheckContext, backend: &str) {
    config_backend(context, backend);
    let artifacts = solstone_core_system::provider_runtime::parakeet_cpp_artifacts(
        &context.journal_path,
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
    context.parakeet_server_probe_override = Some(parakeet_ready_probe);
}

#[cfg(unix)]
fn stage_vad_runtime(context: &mut CheckContext, ready: bool) {
    let stub = context.install_bin_dir.join("vad-coverage-stub");
    fs::create_dir_all(stub.parent().unwrap()).unwrap();
    if ready {
        executable(
            &stub,
            "#!/bin/sh\nprintf '%s\\n' '{\"schema\":\"solstone-vad-error-v1\",\"reason\":\"malformed-request\",\"detail\":\"empty\"}'\nexit 64\n",
        );
    } else {
        executable(
            &stub,
            "#!/bin/sh\necho 'error while loading shared libraries: libonnxruntime.so.1: cannot open shared object file' >&2\nexit 127\n",
        );
    }
    inject_vad_probe(context, stub, Duration::from_secs(2));
}

#[cfg(unix)]
fn stage_speakers_analyze(context: &mut CheckContext, ready: bool) {
    context.speakers_analyze_resolvers = Some((
        if ready {
            speakers_binary_ready
        } else {
            speakers_binary_missing
        },
        speakers_model_ready,
    ));
}

#[cfg(unix)]
fn stage_router_skills(context: &mut CheckContext, broken: bool) {
    use std::os::unix::fs::symlink;

    let root = context.journal_path.parent().unwrap().join("checkout");
    let payload = root.join(solstone_core_journal::CHECKOUT_PAYLOAD_ROOT);
    for name in ["solstone", "journal"] {
        let source = payload.join("solstone/talent").join(name);
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("SKILL.md"), "x").unwrap();
    }
    for parent in [
        context.journal_path.join(".claude/skills"),
        context.journal_path.join(".agents/skills"),
    ] {
        fs::create_dir_all(&parent).unwrap();
        for name in ["solstone", "journal"] {
            let source = payload.join("solstone/talent").join(name);
            symlink(
                solstone_core_skill_state::expected_link_target(&source, &parent),
                parent.join(name),
            )
            .unwrap();
        }
    }
    context.checkout_root = Some(root);
    context.payload_root = Some(payload);
    if broken {
        let parent = context.journal_path.join(".claude/skills");
        fs::remove_file(parent.join("solstone")).unwrap();
    }
}

fn task_pace_with(tasks: serde_json::Value) -> CheckResult {
    task_pace_from_status(Some(serde_json::json!({
        "tract": "supervisor",
        "event": "status",
        "tasks": tasks,
    })))
}

fn task_pace_from_status(status: Option<serde_json::Value>) -> CheckResult {
    crate::checks::task_pace::from_status(
        crate::vocabulary::Check {
            name: "task_pace",
            severity: Severity::Advisory,
            platforms: &[Platform::Linux],
        },
        status.as_ref(),
    )
    .unwrap()
}

#[derive(Clone, Copy)]
enum SecondBranch {
    DifferentStatus,
    DifferentDetail,
}

fn staged_coverage_result(name: &str, ok: bool) -> CheckResult {
    let mut context = fixture();
    match name {
        "journal_sync" => {
            if !ok {
                fs::remove_dir_all(&context.journal_path).unwrap();
            }
        }
        "journal_caught_up" => {
            if !ok {
                stage_backlog_pending(&context);
            }
        }
        "task_pace" => {
            return if ok {
                task_pace_with(serde_json::json!([{ "name":"index", "slow":false }]))
            } else {
                task_pace_from_status(None)
            };
        }
        "brain" => {
            if ok {
                stage_brain_ready(&context);
            } else {
                let path = context.journal_path.join("health/brain.json");
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                fs::write(path, "{").unwrap();
            }
        }
        "capture_health" => {
            let now = context.now.timestamp_millis();
            if ok {
                write_client_fixture(
                    &context,
                    "abcdefgh",
                    serde_json::json!({"key":"abcdefgh-key","name":"phone","enabled":true,"created_at":1,"last_seen":now-1,"last_segment_received_at":now-1}),
                );
            } else {
                write_client_fixture(
                    &context,
                    "abcdefgh",
                    serde_json::json!({"key":"abcdefgh-key","name":"phone","enabled":true,"created_at":1,"last_seen":now-1,"last_segment_received_at":now-86_400_001,"health":{"ingest_rejection":{"active_count":1}}}),
                );
            }
        }
        "client_binding" => {
            if ok {
                write_client_fixture(
                    &context,
                    "abcdefgh",
                    serde_json::json!({"key":"abcdefgh-key","name":"phone","enabled":true,"created_at":1,"device_binding":{"device":format!("sha256:{}", "a".repeat(64)),"kind":"cert"}}),
                );
            }
        }
        "client_delivery_stall" => {
            let now = context.now.timestamp_millis();
            if ok {
                write_client_fixture(
                    &context,
                    "abcdefgh",
                    serde_json::json!({"key":"abcdefgh-key","name":"phone","enabled":true,"created_at":1,"last_seen":now-1_000,"last_segment_received_at":now-1_000}),
                );
            } else {
                write_client_fixture(
                    &context,
                    "abcdefgh",
                    serde_json::json!({"key":"abcdefgh-key","name":"phone","enabled":true,"created_at":1,"last_seen":now-1_000,"last_segment_received_at":now-1_000}),
                );
                write_client_fixture(
                    &context,
                    "ijklmnop",
                    serde_json::json!({"key":"ijklmnop-key","name":"tablet","enabled":true,"created_at":1,"last_seen":now-1_000,"last_segment_received_at":now-21_600_001,"health":{"ingest_rejection":{"active_count":1}}}),
                );
            }
        }
        "client_ingest_health" => {
            write_unassessed_client(&context, "phone", context.now.timestamp_millis() - 1);
            if !ok {
                write_client_fixture(
                    &context,
                    "abcdefgh",
                    serde_json::json!({"key":"abcdefgh-key","name":"phone","enabled":true,"created_at":1,"health":{"ingest_rejection":{"version":"1.2","summary":"bad payload","active_count":2}}}),
                );
            }
        }
        "client_transport_refusal" => {
            write_unassessed_client(&context, "phone", context.now.timestamp_millis() - 1);
            if !ok {
                write_client_fixture(
                    &context,
                    "abcdefgh",
                    serde_json::json!({"key":"abcdefgh-key","name":"phone","enabled":true,"created_at":1,"health":{"transport_refusal":{"reason_code":"stream_limit","active_count":9}}}),
                );
            }
        }
        "orphan_segment_pdf" => {
            let chronicle = context.journal_path.join("chronicle");
            fs::create_dir_all(&chronicle).unwrap();
            if !ok {
                let path = chronicle.join(".dot/a/b/raw.pdf");
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                fs::write(path, "pdf").unwrap();
            }
        }
        "default_stt_ready" => {
            if ok {
                #[cfg(unix)]
                stage_parakeet_ready(&mut context, "parakeet");
            } else {
                config_backend(&context, "whisper");
            }
        }
        "parakeet_cpp_stt_ready" => {
            if ok {
                #[cfg(unix)]
                stage_parakeet_ready(&mut context, "parakeet-cpp");
            }
        }
        "speakers_analyze_installation" => {
            #[cfg(unix)]
            stage_speakers_analyze(&mut context, ok);
        }
        "vad_runtime_ready" => {
            #[cfg(unix)]
            stage_vad_runtime(&mut context, ok);
        }
        "skill_state" => {
            #[cfg(unix)]
            stage_router_skills(&mut context, !ok);
        }
        "unretryable_transcribe_input" => {
            if !ok {
                let segment_dir = context.journal_path.join("chronicle/20260101/120000_60");
                fs::create_dir_all(&segment_dir).unwrap();
                let audio_jsonl = segment_dir.join("audio.jsonl");
                fs::write(
                    audio_jsonl,
                    format!(
                        "{{\"_solstone_processing\":{{\"handler\":\"{}\",\"state\":\"{}\",\"reason_code\":\"{}\"}}}}\n",
                        solstone_core_processing_record::vocab::HANDLER_TRANSCRIBE,
                        solstone_core_processing_record::vocab::STATE_FAILED,
                        solstone_core_processing_record::vocab::REASON_CORRUPT_INPUT
                    ),
                )
                .unwrap();
            }
        }
        _ => unreachable!("unknown W3C check {name}"),
    }
    result(name, &context)
}
#[test]
fn registry_replaces_deferred_check_sets_with_runners() {
    assert!(
        registry::entries(Battery::Journal)
            .iter()
            .filter(|e| matches!(
                e.check.name,
                "journal_sync"
                    | "journal_caught_up"
                    | "task_pace"
                    | "brain"
                    | "capture_health"
                    | "client_binding"
                    | "client_delivery_stall"
                    | "client_ingest_health"
                    | "client_transport_refusal"
                    | "orphan_segment_pdf"
                    | "default_stt_ready"
                    | "parakeet_cpp_stt_ready"
                    | "speakers_analyze_installation"
                    | "vad_runtime_ready"
                    | "skill_state"
                    | "unretryable_transcribe_input"
            ))
            .all(|e| e.deferred.is_none())
    );
}
#[test]
fn check_severity_table_matches_reference() {
    for (name, severity) in [
        ("journal_sync", Severity::Blocker),
        ("journal_caught_up", Severity::Advisory),
        ("task_pace", Severity::Advisory),
        ("brain", Severity::Advisory),
        ("capture_health", Severity::Advisory),
        ("client_binding", Severity::Advisory),
        ("client_delivery_stall", Severity::Advisory),
        ("client_ingest_health", Severity::Advisory),
        ("client_transport_refusal", Severity::Advisory),
        ("orphan_segment_pdf", Severity::Advisory),
        ("default_stt_ready", Severity::Advisory),
        ("parakeet_cpp_stt_ready", Severity::Advisory),
        ("speakers_analyze_installation", Severity::Blocker),
        ("vad_runtime_ready", Severity::Blocker),
        ("skill_state", Severity::Advisory),
        ("unretryable_transcribe_input", Severity::Advisory),
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
fn fixture_covers_ok_and_non_ok_paths() {
    let coverage = [
        ("journal_sync", SecondBranch::DifferentStatus),
        ("journal_caught_up", SecondBranch::DifferentStatus),
        ("task_pace", SecondBranch::DifferentStatus),
        ("brain", SecondBranch::DifferentStatus),
        ("capture_health", SecondBranch::DifferentStatus),
        // The Python reference reports both binding branches as OK; changing
        // the unbound stream branch into a warning would be a regression.
        ("client_binding", SecondBranch::DifferentDetail),
        ("client_delivery_stall", SecondBranch::DifferentStatus),
        ("client_ingest_health", SecondBranch::DifferentStatus),
        ("client_transport_refusal", SecondBranch::DifferentStatus),
        ("orphan_segment_pdf", SecondBranch::DifferentStatus),
        ("default_stt_ready", SecondBranch::DifferentStatus),
        ("parakeet_cpp_stt_ready", SecondBranch::DifferentStatus),
        (
            "speakers_analyze_installation",
            SecondBranch::DifferentStatus,
        ),
        ("vad_runtime_ready", SecondBranch::DifferentStatus),
        ("skill_state", SecondBranch::DifferentStatus),
        (
            "unretryable_transcribe_input",
            SecondBranch::DifferentStatus,
        ),
    ];
    let coverage_names = coverage
        .iter()
        .map(|(name, _)| *name)
        .collect::<std::collections::BTreeSet<_>>();
    let names = W3C_CHECK_NAMES
        .iter()
        .copied()
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        names, coverage_names,
        "every native W3C registry row needs AC1 coverage"
    );
    for name in W3C_CHECK_NAMES {
        let entry = registry::lookup(Battery::Journal, name)
            .unwrap_or_else(|| panic!("W3C check {name} is missing from the registry"));
        assert!(
            entry.deferred.is_none(),
            "W3C check {name} must resolve to a real runner"
        );
    }
    let baseline_names = BASELINE_CHECK_NAMES
        .iter()
        .copied()
        .collect::<std::collections::BTreeSet<_>>();
    let earlier_names = EARLIER_CHECK_NAMES
        .iter()
        .copied()
        .collect::<std::collections::BTreeSet<_>>();
    for name in EARLIER_CHECK_NAMES {
        let entry = registry::lookup(Battery::Journal, name)
            .or_else(|| registry::lookup(Battery::JournalReadiness, name))
            .unwrap_or_else(|| {
                panic!("earlier check set check {name} is missing from the registry")
            });
        assert!(
            entry.deferred.is_none(),
            "earlier check set check {name} must resolve to a real runner"
        );
    }
    assert!(
        registry::entries(Battery::Journal)
            .iter()
            .chain(registry::entries(Battery::JournalReadiness))
            .all(|entry| entry.deferred.is_none()),
        "every deferred wave has landed; no registry row may still be a stub"
    );
    assert!(baseline_names.is_disjoint(&earlier_names));
    assert!(baseline_names.is_disjoint(&names));
    assert!(earlier_names.is_disjoint(&names));
    let partition = baseline_names
        .union(&earlier_names)
        .copied()
        .chain(names.iter().copied())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        registry::union_names(),
        partition,
        "every registry check must be classified as baseline, earlier, or W3C"
    );
    for (name, kind) in coverage {
        let ok = staged_coverage_result(name, true);
        let second = staged_coverage_result(name, false);
        assert_eq!(ok.status, Status::Ok, "{name} OK branch: {}", ok.detail);
        match kind {
            SecondBranch::DifferentStatus => assert_ne!(
                second.status,
                Status::Ok,
                "{name} non-OK branch: {}",
                second.detail
            ),
            SecondBranch::DifferentDetail => {
                assert_eq!(second.status, Status::Ok, "{name}: {}", second.detail);
                assert_ne!(ok.detail, second.detail, "{name} branches must differ");
            }
        }
    }
}
#[test]
fn parakeet_cpp_required_states_are_distinct() {
    assert_eq!(status("parakeet_cpp_stt_ready", &fixture()), Status::Skip);
}
#[test]
fn default_stt_backend_platform_and_corrupt_config_matrix() {
    let c = fixture();
    fs::create_dir_all(c.journal_path.join("config")).unwrap();
    fs::write(c.journal_path.join("config/journal.json"), b"{").unwrap();
    assert_eq!(status("default_stt_ready", &c), Status::Fail);
}
#[test]
fn orphan_pdf_depth_transcript_and_dot_entry_matrix() {
    let c = fixture();
    let p = c.journal_path.join("chronicle/.dot/a/b/raw.pdf");
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, b"pdf").unwrap();
    assert_eq!(status("orphan_segment_pdf", &c), Status::Warn);
}
#[test]
fn no_enabled_clients_skip_client_delivery_and_ingest_checks() {
    let c = fixture();
    for name in [
        "capture_health",
        "client_delivery_stall",
        "client_ingest_health",
        "client_transport_refusal",
    ] {
        assert_eq!(status(name, &c), Status::Skip);
    }
    let capture = result("capture_health", &c);
    let stall = result("client_delivery_stall", &c);
    assert_eq!(capture.client_delivery, stall.client_delivery);
    let facts = capture.client_delivery.as_ref().expect("facts");
    assert_eq!(facts.registry, ClientRegistryState::RegistryEmpty);
    assert!(facts.assessed.is_empty());
    assert!(facts.unassessed.is_empty());
}

fn write_device(
    context: &CheckContext,
    prefix: &str,
    name: &str,
    last_seen: i64,
    last_sent: Option<i64>,
) {
    let mut value = serde_json::json!({
        "key": format!("{prefix}-key"),
        "name": name,
        "enabled": true,
        "created_at": 1,
        "last_seen": last_seen,
    });
    if let Some(stamp) = last_sent {
        value["last_segment_received_at"] = serde_json::json!(stamp);
    }
    write_client_fixture(context, prefix, value);
}

#[test]
fn lone_long_stop_is_informational() {
    let hour = 3_600_000;
    for (sent_age, seen_age) in [(89 * hour, 1_000), (25 * hour, 25 * hour)] {
        let c = fixture();
        let now = c.now.timestamp_millis();
        write_device(
            &c,
            "abcdefgh",
            "phone",
            now - seen_age,
            Some(now - sent_age),
        );
        let capture = result("capture_health", &c);
        let stall = result("client_delivery_stall", &c);
        assert_eq!(capture.status, Status::Ok);
        assert_eq!(stall.status, Status::Ok);
    }
}

#[test]
fn active_peer_does_not_make_a_quiet_device_failed() {
    let hour = 3_600_000;
    for sent_age in [7 * hour, 25 * hour] {
        let c = fixture();
        let now = c.now.timestamp_millis();
        write_device(&c, "abcdefgh", "alpha", now - 1_000, Some(now - 29_000));
        write_device(&c, "ijklmnop", "bravo", now - 1_000, Some(now - sent_age));
        let capture = result("capture_health", &c);
        let stall = result("client_delivery_stall", &c);
        assert_eq!(capture.status, Status::Ok);
        assert_eq!(stall.status, Status::Ok);
    }
}

#[test]
fn overnight_quiet_is_ok() {
    let c = fixture();
    let now = c.now.timestamp_millis();
    let eight = 8 * 3_600_000;
    write_device(&c, "abcdefgh", "alpha", now - eight, Some(now - eight));
    write_device(&c, "ijklmnop", "bravo", now - eight, Some(now - eight));
    assert_eq!(status("capture_health", &c), Status::Ok);
    assert_eq!(status("client_delivery_stall", &c), Status::Ok);
}

#[test]
fn fleet_long_stop_is_informational() {
    let c = fixture();
    let now = c.now.timestamp_millis();
    for (index, prefix) in ["abcdefgh", "ijklmnop", "qrstuvwx", "yzabcdef"]
        .into_iter()
        .enumerate()
    {
        write_device(
            &c,
            prefix,
            &format!("d{index}"),
            now - 200_000,
            Some(now - 41 * 3_600_000),
        );
    }
    assert_eq!(status("client_delivery_stall", &c), Status::Ok);
}

#[test]
fn no_assessed_skips_both_checks() {
    let c = fixture();
    write_unassessed_client(&c, "phone", c.now.timestamp_millis() - 1);
    let capture = result("capture_health", &c);
    let stall = result("client_delivery_stall", &c);
    assert_eq!(capture.status, Status::Skip);
    assert_eq!(stall.status, Status::Skip);
    assert!(capture.detail.contains("rollup=no_senders"));
    assert_eq!(
        stall.detail,
        "the solstone app hasn't added anything to your journal yet"
    );
    assert_eq!(capture.client_delivery, stall.client_delivery);
    let facts = capture.client_delivery.as_ref().expect("facts");
    assert_eq!(facts.registry, ClientRegistryState::RegistryComplete);
    assert!(facts.assessed.is_empty());
    assert_eq!(facts.unassessed.len(), 1);
    assert_eq!(facts.unassessed[0].name, "phone");
    assert_eq!(facts.unassessed[0].reason, "awaiting_first_delivery");
}

#[test]
fn unassessed_residue_does_not_drag() {
    let c = fixture();
    let now = c.now.timestamp_millis();
    write_unassessed_client(&c, "residue", now - 200_000);
    write_device(&c, "ijklmnop", "peer", now - 1, Some(now - 1_000));
    let capture = result("capture_health", &c);
    let stall = result("client_delivery_stall", &c);
    assert_eq!(capture.status, Status::Ok);
    assert_eq!(stall.status, Status::Ok);
    assert_eq!(capture.client_delivery, stall.client_delivery);
    let facts = capture.client_delivery.as_ref().expect("facts");
    assert_eq!(facts.registry, ClientRegistryState::RegistryComplete);
    assert_eq!(facts.assessed.len(), 1);
    assert_eq!(facts.assessed[0].name, "peer");
    assert_eq!(facts.unassessed.len(), 1);
    assert_eq!(facts.unassessed[0].name, "residue");
    assert_eq!(facts.unassessed[0].reason, "awaiting_first_delivery");
    assert_eq!(facts.unassessed[0].reach, "offline");
}

#[test]
fn delivery_facts_distinguish_remaining_tokens() {
    let now_offset = |context: &CheckContext, delta: i64| context.now.timestamp_millis() - delta;

    let invalid = fixture();
    write_client_fixture(
        &invalid,
        "abcdefgh",
        serde_json::json!({
            "key": "abcdefgh-key",
            "name": "bad",
            "enabled": true,
            "created_at": 1,
            "last_seen": now_offset(&invalid, 1),
            "last_segment_received_at": "not-a-stamp",
        }),
    );
    let capture = result("capture_health", &invalid);
    let stall = result("client_delivery_stall", &invalid);
    assert_eq!(capture.client_delivery, stall.client_delivery);
    let facts = capture.client_delivery.as_ref().expect("facts");
    assert_eq!(facts.registry, ClientRegistryState::RegistryComplete);
    assert!(facts.assessed.is_empty());
    assert_eq!(facts.unassessed.len(), 1);
    assert_eq!(facts.unassessed[0].name, "bad");
    assert_eq!(facts.unassessed[0].reason, "awaiting_first_delivery");

    let residue = fixture();
    write_client_fixture(
        &residue,
        "abcdefgh",
        serde_json::json!({
            "key": "abcdefgh-key",
            "name": "old",
            "enabled": true,
            "created_at": 1,
            "last_seen": now_offset(&residue, 200_000),
        }),
    );
    let capture = result("capture_health", &residue);
    let stall = result("client_delivery_stall", &residue);
    assert_eq!(capture.client_delivery, stall.client_delivery);
    let facts = capture.client_delivery.as_ref().expect("facts");
    assert_eq!(facts.unassessed.len(), 1);
    assert_eq!(facts.unassessed[0].name, "old");
    assert_eq!(facts.unassessed[0].reason, "awaiting_first_delivery");

    let partial = fixture();
    write_device(
        &partial,
        "abcdefgh",
        "peer",
        now_offset(&partial, 1),
        Some(now_offset(&partial, 1_000)),
    );
    let capture = result("capture_health", &partial);
    let stall = result("client_delivery_stall", &partial);
    assert_eq!(capture.client_delivery, stall.client_delivery);
    assert_eq!(
        capture.client_delivery.as_ref().expect("facts").registry,
        ClientRegistryState::RegistryComplete
    );

    let ineligible = fixture();
    write_client_fixture(
        &ineligible,
        "abcdefgh",
        serde_json::json!({
            "key": "abcdefgh-key",
            "name": "off",
            "enabled": false,
            "created_at": 1,
            "last_seen": now_offset(&ineligible, 1),
            "last_segment_received_at": now_offset(&ineligible, 1),
        }),
    );
    write_client_fixture(
        &ineligible,
        "ijklmnop",
        serde_json::json!({
            "key": "ijklmnop-key",
            "name": "gone",
            "enabled": true,
            "revoked": true,
            "created_at": 1,
            "last_seen": now_offset(&ineligible, 1),
            "last_segment_received_at": now_offset(&ineligible, 1),
        }),
    );
    let capture = result("capture_health", &ineligible);
    let stall = result("client_delivery_stall", &ineligible);
    assert_eq!(capture.client_delivery, stall.client_delivery);
    let facts = capture.client_delivery.as_ref().expect("facts");
    assert_eq!(facts.registry, ClientRegistryState::RegistryEmpty);
    assert!(facts.assessed.is_empty());
    assert!(facts.unassessed.is_empty());
}

#[test]
fn delivery_facts_match_across_checks_with_client_ledger() {
    let c = fixture();
    let now = c.now.timestamp_millis();
    write_unassessed_client(&c, "residue", now - 1);
    write_device(&c, "ijklmnop", "peer", now - 1, Some(now - 1_000));
    let capture = result("capture_health", &c);
    let stall = result("client_delivery_stall", &c);
    assert_eq!(capture.status, Status::Ok);
    assert_eq!(stall.status, Status::Ok);
    let capture_facts = serde_json::to_value(&capture.client_delivery).unwrap();
    let stall_facts = serde_json::to_value(&stall.client_delivery).unwrap();
    assert_eq!(capture_facts, stall_facts);
    assert_eq!(capture_facts["registry"], "registry_complete");
    assert_eq!(capture_facts["assessed"][0]["name"], "peer");
    assert_eq!(capture_facts["unassessed"][0]["name"], "residue");
}

#[test]
fn lone_six_hour_gap_is_ok() {
    let c = fixture();
    let now = c.now.timestamp_millis();
    write_device(&c, "abcdefgh", "phone", now - 1_000, Some(now - 21_600_001));
    assert_eq!(status("client_delivery_stall", &c), Status::Ok);
    assert_eq!(status("capture_health", &c), Status::Ok);
}

#[test]
fn rejection_without_last_sent_warns_capture() {
    let alone = fixture();
    write_client_fixture(
        &alone,
        "abcdefgh",
        serde_json::json!({
            "key":"abcdefgh-key","name":"rej","enabled":true,"created_at":1,
            "last_seen": alone.now.timestamp_millis() - 1,
            "health":{"ingest_rejection":{"active_count":1}}
        }),
    );
    let capture = result("capture_health", &alone);
    let stall = result("client_delivery_stall", &alone);
    assert_eq!(capture.status, Status::Warn);
    assert_ne!(stall.status, Status::Skip);
    assert_eq!(stall.status, Status::Warn);
    assert!(capture.detail.contains("having trouble adding"));
    assert!(!capture.detail.contains("still running"));
    assert!(!capture.detail.contains("asleep"));
    assert_eq!(capture.client_delivery, stall.client_delivery);
    let facts = capture.client_delivery.as_ref().expect("facts");
    assert_eq!(facts.assessed.len(), 1);
    assert_eq!(facts.assessed[0].name, "rej");
    assert!(facts.unassessed.is_empty());

    let with_peer = fixture();
    let now = with_peer.now.timestamp_millis();
    write_client_fixture(
        &with_peer,
        "abcdefgh",
        serde_json::json!({
            "key":"abcdefgh-key","name":"rej","enabled":true,"created_at":1,
            "last_seen": now - 1,
            "health":{"ingest_rejection":{"active_count":1}}
        }),
    );
    write_device(&with_peer, "ijklmnop", "peer", now - 1, Some(now - 1_000));
    assert_eq!(status("capture_health", &with_peer), Status::Warn);
}

#[test]
fn rejection_with_recent_delivery_warns_capture() {
    let c = fixture();
    let now = c.now.timestamp_millis();
    write_client_fixture(
        &c,
        "abcdefgh",
        serde_json::json!({
            "key":"abcdefgh-key","name":"rej","enabled":true,"created_at":1,
            "last_seen": now - 1,
            "last_segment_received_at": now - 120_000,
            "health":{"ingest_rejection":{"active_count":1}}
        }),
    );
    assert_eq!(status("capture_health", &c), Status::Warn);
}

#[test]
fn unreadable_skip_is_not_never_sent() {
    let check = |name| registry::lookup(Battery::Journal, name).unwrap().check;
    let inspection = ClientInspection::LedgerUnavailable {
        reason: ClientLedgerUnavailable::Unreadable,
        activity: ClientActivityState::Missing,
    };
    let capture = crate::checks::capture_health::result_from_assessment(
        inspection.clone(),
        check("capture_health"),
    );
    let stall = crate::checks::client_delivery_stall::result_from_assessment(
        inspection,
        check("client_delivery_stall"),
    );
    assert_eq!(capture.status, Status::Skip);
    assert_eq!(stall.status, Status::Skip);
    assert!(capture.detail.contains("rollup=unknown"));
    assert_ne!(
        stall.detail,
        "the solstone app hasn't added anything to your journal yet"
    );
    assert_eq!(capture.client_delivery, stall.client_delivery);
    let facts = capture.client_delivery.as_ref().expect("facts");
    assert_eq!(facts.registry, ClientRegistryState::RegistryUnknown);
    assert!(facts.assessed.is_empty());
    assert!(facts.unassessed.is_empty());
}

#[test]
fn client_binding_skips_when_authorization_ledger_is_unavailable() {
    let context = fixture();
    let ledger = context.journal_path.join("link/authorized_clients.json");
    fs::create_dir_all(&ledger).unwrap();

    let row = result("client_binding", &context);

    assert_eq!(row.status, Status::Skip);
    assert_eq!(
        row.detail,
        "device records unavailable: authorized client ledger unavailable: Unreadable"
    );
}

#[test]
fn a_turned_away_request_and_a_rejected_upload_are_separate_findings() {
    // The whole point. Before this, a device whose requests the door turned
    // away was reported by nothing at all, so the only visible signal was the
    // client's own "can't reach your journal" -- the same thing a dead network
    // produces. These two checks must not collapse into each other.
    let context = fixture();
    write_client_fixture(
        &context,
        "abcdefgh",
        serde_json::json!({
            "key":"abcdefgh-key", "name":"phone", "enabled":true, "created_at":1,
            "health":{"transport_refusal":{"reason_code":"stream_limit","active_count":9}}
        }),
    );

    let refusal = result("client_transport_refusal", &context);
    assert_eq!(refusal.status, Status::Warn);
    assert_eq!(
        refusal.detail,
        "device abcdefgh had requests turned away: stream_limit, 9x through 2026-01-01"
    );

    // The negative control: nothing was rejected at ingest, so the sibling
    // check stays quiet. A refusal that also tripped that one would be
    // reporting the wrong cause.
    assert_eq!(result("client_ingest_health", &context).status, Status::Ok);
}

#[test]
fn a_journal_that_turned_nothing_away_says_so_rather_than_staying_silent() {
    let context = fixture();
    write_unassessed_client(&context, "phone", context.now.timestamp_millis() - 1);

    let row = result("client_transport_refusal", &context);

    assert_eq!(row.status, Status::Ok);
    assert_eq!(row.detail, "no devices had requests turned away");
}

#[test]
fn client_ingest_health_formats_rejection_date_and_unknown_fallback() {
    let dated = fixture();
    write_client_fixture(
        &dated,
        "abcdefgh",
        serde_json::json!({
            "key":"abcdefgh-dated", "name":"dated", "enabled":true, "created_at":1,
            "health":{"ingest_rejection":{
                "version":"1.2", "summary":"bad payload", "active_count":2,
                "first_ts": dated.now.timestamp_millis()
            }}
        }),
    );
    let row = result("client_ingest_health", &dated);
    assert_eq!(row.status, Status::Warn);
    assert_eq!(
        row.detail,
        "device abcdefgh failing ingest: ingest_rejected, 2x since 2026-01-01"
    );

    let unknown = fixture();
    write_client_fixture(
        &unknown,
        "abcdefgh",
        serde_json::json!({
            "key":"abcdefgh-unknown", "name":"unknown", "enabled":true, "created_at":1,
            "health":{"ingest_rejection":{
                "version":"1.2", "summary":"bad payload", "active_count":2
            }}
        }),
    );
    let row = result("client_ingest_health", &unknown);
    assert_eq!(row.status, Status::Warn);
    assert_eq!(
        row.detail,
        "device abcdefgh failing ingest: ingest_rejected, 2x since 2026-01-01"
    );
}

#[test]
fn setup_json_and_jsonl_filters_receive_advisory_warning() {
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
    crate::output::emit_jsonl_to(&mut bytes, &warned, "2026-01-01T00:00:00Z", 0, 5015);
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
fn brain_unconstructible_snapshot_is_an_explicit_warning() {
    let context = fixture();
    let path = context.journal_path.join("health/brain.json");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, "{").unwrap();
    let row = result("brain", &context);
    assert_eq!(row.status, Status::Warn);
    assert!(row.detail.starts_with("unknown: "), "{}", row.detail);
}

#[test]
fn brain_ready_and_checking_records_are_healthy() {
    let ready = fixture();
    stage_brain_ready(&ready);
    let row = result("brain", &ready);
    assert_eq!(row.status, Status::Ok, "{}", row.detail);
    assert_eq!(
        row.detail,
        "processing is ready; state=ready; reason=ok; component=none; evidence_age=1m"
    );

    let checking = fixture();
    let _permit = stage_brain_checking(&checking);
    let row = result("brain", &checking);
    assert_eq!(row.status, Status::Ok, "{}", row.detail);
    assert_eq!(
        row.detail,
        "checking how processing runs; state=checking; reason=brain check in progress; component=none; evidence_age=unknown"
    );
}

#[test]
fn brain_refresh_after_battery_start_is_not_a_future_record() {
    let mut context = fixture();
    context.now = chrono::Utc::now();
    let _permit = stage_brain_checking(&context);
    context.now -= chrono::Duration::seconds(30);
    let entry = registry::lookup(Battery::Journal, "brain").unwrap();
    let row = (entry.runner)(&context).unwrap();
    assert_eq!(row.status, Status::Ok, "{}", row.detail);
    assert!(row.detail.contains("state=checking"), "{}", row.detail);
}

#[test]
fn brain_clock_is_sampled_after_reading_the_record() {
    let mut context = fixture();
    context.now = chrono::Utc::now();
    let _permit = stage_brain_checking(&context);
    let path = context.journal_path.join("health/brain.json");
    let before = fs::read(&path).unwrap();
    let entry = registry::lookup(Battery::Journal, "brain").unwrap();
    let row = crate::checks::brain::run_with_clock(&context, entry.check, || {
        // Replacing the file here must not change the already-read snapshot.
        fs::write(&path, "{").unwrap();
        context.now + chrono::Duration::seconds(1)
    })
    .unwrap();
    assert_eq!(row.status, Status::Ok, "{}", row.detail);
    fs::write(path, before).unwrap();
}

#[test]
fn brain_fresh_clock_preserves_future_and_expiry_warnings() {
    let mut future = fixture();
    future.now = chrono::Utc::now() + chrono::Duration::hours(1);
    let _permit = stage_brain_checking(&future);
    let entry = registry::lookup(Battery::Journal, "brain").unwrap();
    let row = (entry.runner)(&future).unwrap();
    assert_eq!(row.status, Status::Warn, "{}", row.detail);
    assert!(
        row.detail.contains("brain record invalid"),
        "{}",
        row.detail
    );

    let expired = fixture();
    stage_brain_ready(&expired);
    let row = crate::checks::brain::run_with_clock(&expired, entry.check, || {
        expired.now + chrono::Duration::days(2)
    })
    .unwrap();
    assert_eq!(row.status, Status::Warn, "{}", row.detail);
}

#[test]
fn task_pace_classifies_good_and_slow_injected_status() {
    let ok = task_pace_with(serde_json::json!([{ "name":"index", "slow":false }]));
    assert_eq!(ok.status, Status::Ok);
    assert_eq!(ok.detail, "tasks on pace");
    let warn = task_pace_with(serde_json::json!([{
        "name":"index", "slow":true, "duration_seconds":12, "max_runtime_seconds":10
    }]));
    assert_eq!(warn.status, Status::Warn);
    assert_eq!(warn.detail, "running long: index (12s of 10s cap)");
    assert_eq!(task_pace_from_status(None).status, Status::Skip);
}

#[test]
fn task_pace_malformed_slow_fields_warn() {
    let row = task_pace_with(serde_json::json!([{ "name": "index", "slow": true }]));
    assert_eq!(row.status, Status::Warn);
    assert_eq!(row.detail, "running long: index (0s of ?s cap)");
}

#[test]
fn task_pace_missing_status_skips() {
    let row = task_pace_from_status(None);
    assert_eq!(row.status, Status::Skip);
    assert_eq!(row.detail, "supervisor status unavailable");
}

#[test]
fn caught_up_native_backlog_fixture_states() {
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
    assert_eq!(row.status, Status::Warn);
    assert_eq!(
        row.detail, "1 day(s) pending, 0 day(s) stuck; oldest outstanding 20251230",
        "legacy terminal logs cannot prove current daily coverage"
    );

    let root = &capped.journal_path;
    configure_daily_work(root, Some("schedule"));
    let coverage =
        solstone_core_system::daily_coverage::read_daily_coverage(root, "20251230").unwrap();
    let unit = coverage
        .units
        .iter()
        .find(|unit| unit.identity.name == "schedule")
        .unwrap();
    let mut record = solstone_core_journal_io::DailyUnitRecord::new(
        unit.identity.clone(),
        &unit.evidence_revision,
        &unit.contract_digest,
    );
    record.status = solstone_core_journal_io::DailyUnitStatus::Capped;
    record.reason_code = Some("provider_request_rejected".into());
    record.failure_count = 1;
    solstone_core_journal_io::save_daily_unit_record(root, &record).unwrap();
    let row = result("journal_caught_up", &capped);
    assert_eq!(row.status, Status::Warn);
    assert_eq!(
        row.detail,
        "caught up; 1 day(s) completed with capped daily unit(s)"
    );

    let pending = fixture();
    stage_backlog_pending(&pending);
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

    let fresh_audio = fixture();
    stage_raw_audio_pending(
        &fresh_audio,
        SystemTime::UNIX_EPOCH
            + Duration::from_millis((fresh_audio.now.timestamp_millis() - 1_000) as u64),
    );
    let row = result("journal_caught_up", &fresh_audio);
    assert_eq!(row.status, Status::Ok);
    assert_eq!(row.detail, "caught up");

    let aged_audio = fixture();
    stage_raw_audio_pending(
        &aged_audio,
        SystemTime::UNIX_EPOCH
            + Duration::from_millis(
                (aged_audio.now.timestamp_millis() - MODALITY_INPUT_AGED_MS) as u64,
            ),
    );
    let row = result("journal_caught_up", &aged_audio);
    assert_eq!(row.status, Status::Warn);
    assert_eq!(
        row.detail,
        "1 day(s) pending, 0 day(s) stuck; oldest outstanding 20251231"
    );
}

#[test]
#[cfg(unix)]
fn parakeet_cpp_fixture_states_are_distinct() {
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
    stage_parakeet_ready(&mut unreachable, "parakeet-cpp");
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
fn default_stt_fixture_matrix_delegates_and_checks_coreml() {
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
    stage_parakeet_ready(&mut linux, "parakeet");
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
    for artifact in solstone_core_assets::catalog()
        .iter()
        .filter(|artifact| artifact.unit == "parakeet-coreml")
    {
        let path = model.join(artifact.filename);
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
fn skill_state_fixture_branches() {
    use std::os::unix::fs::symlink;

    let mut installed = fixture();
    stage_router_skills(&mut installed, false);
    let root = installed.payload_root.clone().unwrap();
    let row = result("skill_state", &installed);
    assert_eq!(row.status, Status::Ok);
    assert_eq!(
        row.detail,
        "router skills solstone, journal are installed and current"
    );

    let broken = installed.clone();
    let claude = broken.journal_path.join(".claude/skills");
    fs::remove_file(claude.join("solstone")).unwrap();
    fs::remove_file(claude.join("journal")).unwrap();
    symlink("foreign", claude.join("journal")).unwrap();
    symlink("old", claude.join("stale")).unwrap();
    let row = result("skill_state", &broken);
    assert_eq!(row.status, Status::Warn);
    assert!(row.detail.contains("solstone missing at"));
    assert!(row.detail.contains("journal points elsewhere at"));
    assert!(row.detail.contains("stale router skill link at"));
    assert_eq!(
        row.fix.as_deref(),
        Some("run solstone skills install --project .")
    );

    let no_root = fixture();
    assert_eq!(result("skill_state", &no_root).status, Status::Skip);
    let mut no_dirs = fixture();
    no_dirs.payload_root = Some(root);
    let row = result("skill_state", &no_dirs);
    assert_eq!(row.status, Status::Skip);
    assert_eq!(row.detail, "router skill directories are unavailable");
}

#[test]
#[cfg(unix)]
fn speakers_installation_uses_injected_resolvers() {
    let mut ready = fixture();
    stage_speakers_analyze(&mut ready, true);
    let row = result("speakers_analyze_installation", &ready);
    assert_eq!(row.status, Status::Ok);
    assert_eq!(row.detail, "speakers-analyze installation ready");

    stage_speakers_analyze(&mut ready, false);
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

thread_local! {
    static VAD_SEAM: std::cell::RefCell<Option<crate::context::VadRuntimeProbeSeam>> =
        const { std::cell::RefCell::new(None) };
}

fn vad_injected_resolver() -> crate::context::VadRuntimeProbeSeam {
    VAD_SEAM.with(|slot| slot.borrow().clone().expect("injected VAD runtime seam"))
}

fn inject_vad_probe(context: &mut CheckContext, binary: PathBuf, timeout: Duration) {
    VAD_SEAM.with(|slot| {
        *slot.borrow_mut() = Some(crate::context::VadRuntimeProbeSeam { binary, timeout });
    });
    context.vad_runtime_probe = Some(vad_injected_resolver);
}

#[test]
#[cfg(unix)]
fn vad_runtime_ready_reports_loader_failure_as_blocker() {
    let mut context = fixture();
    let stub = context.install_bin_dir.join("vad-loader-stub");
    fs::create_dir_all(stub.parent().unwrap()).unwrap();
    executable(
        &stub,
        "#!/bin/sh\necho 'error while loading shared libraries: libonnxruntime.so.1: cannot open shared object file' >&2\nexit 127\n",
    );
    inject_vad_probe(&mut context, stub, Duration::from_secs(2));
    let row = result("vad_runtime_ready", &context);
    assert_eq!(row.status, Status::Fail);
    assert_eq!(row.severity, Severity::Blocker);
    assert!(row.detail.contains("libonnxruntime.so.1"), "{}", row.detail);
}

#[test]
#[cfg(unix)]
fn vad_runtime_ready_accepts_closed_stdin_usage_contract() {
    let mut context = fixture();
    let stub = context.install_bin_dir.join("vad-healthy-stub");
    fs::create_dir_all(stub.parent().unwrap()).unwrap();
    executable(
        &stub,
        "#!/bin/sh\nprintf '%s\\n' '{\"schema\":\"solstone-vad-error-v1\",\"reason\":\"malformed-request\",\"detail\":\"empty\"}'\nexit 64\n",
    );
    inject_vad_probe(&mut context, stub, Duration::from_secs(2));
    let row = result("vad_runtime_ready", &context);
    assert_eq!(row.status, Status::Ok);
    assert_eq!(row.severity, Severity::Blocker);
}

#[test]
fn stall_warn_omits_duplicate_and_queue_clauses() {
    let context = fixture();
    let now = context.now.timestamp_millis();
    write_client_fixture(
        &context,
        "abcdefgh",
        serde_json::json!({
            "key":"abcdefgh-key", "name":"phone", "enabled":true, "created_at":1,
            "last_seen": now - 1_000,
            "last_segment_received_at": now - 86_400_001,
            "stats": {"duplicates_rejected": 793866},
            "health": {"beacon": {"pending_queue_depth": 4}, "ingest_rejection":{"active_count":1}}
        }),
    );
    let row = result("client_delivery_stall", &context);
    assert_eq!(row.status, Status::Warn);
    assert!(!row.detail.contains("duplicate"));
    assert!(!row.detail.contains("pending queue"));
}

fn write_rejected_fleet(context: &CheckContext, name: impl Fn(usize) -> String, seen_age: i64) {
    let now = context.now.timestamp_millis();
    for (index, prefix) in ["abcdefgh", "ijklmnop", "qrstuvwx", "yzabcdef"]
        .into_iter()
        .enumerate()
    {
        write_client_fixture(
            context,
            prefix,
            serde_json::json!({
                "key": format!("{prefix}-key"), "name": name(index), "enabled": true,
                "created_at": 1, "last_seen": now-seen_age,
                "last_segment_received_at": now-41*3_600_000,
                "health": {"ingest_rejection": {"active_count": 1}}
            }),
        );
    }
    write_client_fixture(
        context,
        "residu01",
        serde_json::json!({
            "key": "residu01-key",
            "name": "unseen-residue",
            "enabled": true,
            "created_at": 1,
            "last_seen": now - 1,
        }),
    );
}

fn assert_facts_survive_in_json_and_not_text(capture: &CheckResult, stall: &CheckResult) {
    assert_eq!(capture.client_delivery, stall.client_delivery);
    let facts = capture.client_delivery.as_ref().expect("facts");
    assert_eq!(facts.assessed.len(), 4);
    assert_eq!(facts.unassessed.len(), 1);
    assert_eq!(facts.unassessed[0].name, "unseen-residue");
    assert!(!stall.detail.contains("unseen-residue"));
    let serialized = serde_json::to_value(facts).unwrap();
    for row in &facts.assessed {
        assert!(serialized.to_string().contains(&row.name));
    }
    let mut bytes = Vec::new();
    output::emit_jsonl_to(
        &mut bytes,
        std::slice::from_ref(stall),
        "2026-01-01T00:00:00Z",
        1,
        5015,
    );
    let jsonl = String::from_utf8(bytes).unwrap();
    assert!(jsonl.contains("unseen-residue"));
    for row in &facts.assessed {
        assert!(jsonl.contains(&row.name));
    }
    assert!(jsonl.contains("client_delivery"));
    assert!(
        serde_json::to_value(stall)
            .unwrap()
            .get("client_delivery")
            .is_some()
    );
    let mut text = Vec::new();
    output::emit_text_to(&mut text, std::slice::from_ref(stall), false).unwrap();
    let text = String::from_utf8(text).unwrap();
    assert!(!text.contains("unseen-residue"), "{text}");
    for token in [
        "awaiting_first_delivery",
        "invalid_delivery_evidence",
        "registration_residue",
        "registry_unknown",
        "partial_registry",
        "registry_empty",
        "no_eligible_records",
        "registry_complete",
    ] {
        assert!(!text.contains(token), "{token} leaked into text: {text}");
    }
}

#[test]
fn delivery_facts_survive_truncated_detail_and_are_absent_from_text() {
    let context = fixture();
    write_rejected_fleet(&context, |index| format!("d{index}"), 200_000);
    let capture = result("capture_health", &context);
    let stall = result("client_delivery_stall", &context);
    assert_eq!(capture.status, Status::Warn);
    assert_eq!(stall.status, Status::Warn);
    assert!(
        capture.detail.contains("+1 more"),
        "join_capped must fire: {}",
        capture.detail
    );
    assert!(
        stall.detail.contains("+1 more"),
        "join_capped must fire: {}",
        stall.detail
    );
    assert_facts_survive_in_json_and_not_text(&capture, &stall);
}

#[test]
fn delivery_facts_survive_400_truncation_with_rejections() {
    let context = fixture();
    let long = "x".repeat(160);
    write_rejected_fleet(&context, |index| format!("{long}{index}"), 200_000);
    let capture = result("capture_health", &context);
    let stall = result("client_delivery_stall", &context);
    assert_eq!(capture.status, Status::Warn);
    assert_eq!(stall.status, Status::Warn);
    assert!(
        stall.detail.ends_with("...") && stall.detail.len() == 400,
        "truncate must fire: len={} detail={}",
        stall.detail.len(),
        stall.detail
    );
    assert!(
        capture.detail.ends_with("...") && capture.detail.len() == 400,
        "truncate must fire: len={} detail={}",
        capture.detail.len(),
        capture.detail
    );
    assert_facts_survive_in_json_and_not_text(&capture, &stall);
}

#[test]
fn quiet_devices_preserve_reach_facts_without_inventing_failure() {
    let hour = 3_600_000;
    let running = fixture();
    let now = running.now.timestamp_millis();
    write_device(
        &running,
        "abcdefgh",
        "phone",
        now - 1_000,
        Some(now - 89 * hour),
    );
    let stale = fixture();
    let now = stale.now.timestamp_millis();
    write_device(
        &stale,
        "abcdefgh",
        "phone",
        now - 60_000,
        Some(now - 89 * hour),
    );
    let asleep = fixture();
    let now = asleep.now.timestamp_millis();
    write_device(
        &asleep,
        "abcdefgh",
        "phone",
        now - 200_000,
        Some(now - 89 * hour),
    );

    let running_capture = result("capture_health", &running);
    let running_stall = result("client_delivery_stall", &running);
    let stale_capture = result("capture_health", &stale);
    let stale_stall = result("client_delivery_stall", &stale);
    let asleep_capture = result("capture_health", &asleep);
    let asleep_stall = result("client_delivery_stall", &asleep);

    for row in [
        &running_capture,
        &running_stall,
        &stale_capture,
        &stale_stall,
        &asleep_capture,
        &asleep_stall,
    ] {
        assert_eq!(row.status, Status::Ok);
        assert!(row.fix.is_none());
        assert!(!row.detail.contains("isn't adding"));
        assert!(!row.detail.contains("still running"));
    }
    assert_eq!(
        running_capture.client_delivery,
        running_stall.client_delivery
    );
    assert_eq!(stale_capture.client_delivery, stale_stall.client_delivery);
    assert_eq!(asleep_capture.client_delivery, asleep_stall.client_delivery);
    let running_facts = serde_json::to_value(&running_capture.client_delivery).unwrap();
    let stale_facts = serde_json::to_value(&stale_capture.client_delivery).unwrap();
    let asleep_facts = serde_json::to_value(&asleep_capture.client_delivery).unwrap();
    assert_eq!(running_facts["assessed"][0]["reach"], "active");
    assert_eq!(stale_facts["assessed"][0]["reach"], "stale");
    assert_eq!(asleep_facts["assessed"][0]["reach"], "offline");
    let strip_reach = |value: &serde_json::Value| {
        let mut stripped = value.clone();
        for key in ["assessed", "unassessed"] {
            if let Some(rows) = stripped[key].as_array_mut() {
                for row in rows {
                    row.as_object_mut().unwrap().remove("reach");
                }
            }
        }
        stripped
    };
    assert_eq!(strip_reach(&running_facts), strip_reach(&stale_facts));
    assert_eq!(strip_reach(&running_facts), strip_reach(&asleep_facts));
}

fn rfc3339_millis(timestamp: i64) -> String {
    chrono::Utc
        .timestamp_millis_opt(timestamp)
        .single()
        .expect("fixture timestamp")
        .to_rfc3339()
}

#[test]
fn quiet_single_source_capture_and_stall_are_informational() {
    let hour = 3_600_000;
    let expected_capture =
        "rollup=quiet; no rejected uploads recorded; see devices for last delivery times";
    let expected_stall = "no rejected uploads recorded; see devices for last delivery times";
    for sources in [
        None,
        Some(serde_json::json!({
            "audio": {"last_accepted_ingest_at": ""}
        })),
        Some(serde_json::json!({
            "": {"last_accepted_ingest_at": ""}
        })),
    ] {
        let c = fixture();
        let now = c.now.timestamp_millis();
        let last_sent = now - 89 * hour;
        let stamp = rfc3339_millis(last_sent);
        let mut value = serde_json::json!({
            "key": "abcdefgh-key",
            "name": "phone",
            "enabled": true,
            "created_at": 1,
            "last_seen": now - 1_000,
            "last_segment_received_at": last_sent,
        });
        if let Some(mut sources) = sources {
            if let Some(audio) = sources.get_mut("audio") {
                audio["last_accepted_ingest_at"] = stamp.clone().into();
            }
            if let Some(default) = sources.get_mut("") {
                default["last_accepted_ingest_at"] = stamp.clone().into();
            }
            value["sources"] = sources;
        }
        write_client_fixture(&c, "abcdefgh", value);
        let capture = result("capture_health", &c);
        let stall = result("client_delivery_stall", &c);
        assert_eq!(capture.detail, expected_capture);
        assert_eq!(stall.detail, expected_stall);
        assert!(!capture.detail.contains("audio"));
        assert!(!stall.detail.contains("audio"));
        assert!(!capture.detail.contains("default"));
        assert!(!stall.detail.contains("default"));
    }
}

#[test]
fn multi_source_needs_attention_names_the_rejected_source() {
    let c = fixture();
    let now = c.now.timestamp_millis();
    write_client_fixture(
        &c,
        "abcdefgh",
        serde_json::json!({
            "key": "abcdefgh-key",
            "name": "phone",
            "enabled": true,
            "created_at": 1,
            "last_seen": now - 1_000,
            "last_segment_received_at": now - 1_000,
            "sources": {
                "audio": {"last_accepted_ingest_at": rfc3339_millis(now - 1_000)},
                "location": {"last_accepted_ingest_at": rfc3339_millis(now - 700_000), "ingest_rejection": {"active_count": 1, "reason_code":"ingest_rejected", "first":rfc3339_millis(now), "latest":rfc3339_millis(now)}},
            }
        }),
    );
    let capture = result("capture_health", &c);
    let stall = result("client_delivery_stall", &c);
    assert_eq!(capture.status, Status::Warn);
    assert_eq!(stall.status, Status::Warn);
    assert_eq!(
        capture.detail,
        "rollup=attention; the solstone app on phone is having trouble adding location"
    );
    assert_eq!(
        stall.detail,
        "the journal rejected an upload from phone (location needs attention)"
    );
}

#[test]
fn multi_source_plural_need_attention_joins_rejected_names() {
    let c = fixture();
    let now = c.now.timestamp_millis();
    let quiet = rfc3339_millis(now - 700_000);
    write_client_fixture(
        &c,
        "abcdefgh",
        serde_json::json!({
            "key": "abcdefgh-key",
            "name": "phone",
            "enabled": true,
            "created_at": 1,
            "last_seen": now - 1_000,
            "last_segment_received_at": now - 700_000,
            "sources": {
                "audio": {"last_accepted_ingest_at": quiet, "ingest_rejection": {"active_count": 1, "reason_code":"ingest_rejected", "first":rfc3339_millis(now), "latest":rfc3339_millis(now)}},
                "location": {"last_accepted_ingest_at": quiet, "ingest_rejection": {"active_count": 1, "reason_code":"ingest_rejected", "first":rfc3339_millis(now), "latest":rfc3339_millis(now)}},
                "screen": {"last_accepted_ingest_at": quiet, "ingest_rejection": {"active_count": 1, "reason_code":"ingest_rejected", "first":rfc3339_millis(now), "latest":rfc3339_millis(now)}},
            }
        }),
    );
    let capture = result("capture_health", &c);
    let stall = result("client_delivery_stall", &c);
    assert_eq!(capture.status, Status::Warn);
    assert_eq!(stall.status, Status::Warn);
    assert!(
        capture
            .detail
            .contains("having trouble adding audio, location, screen"),
        "capture should name the rejected sources: {}",
        capture.detail
    );
    assert!(
        stall
            .detail
            .contains("audio, location, screen need attention"),
        "stall should join rejected sources with the plural verb: {}",
        stall.detail
    );
    assert!(!capture.detail.contains("needs attention"));
    assert!(!stall.detail.contains("needs attention"));
}

#[test]
fn empty_source_is_named_default_on_a_multi_source_device() {
    let c = fixture();
    let now = c.now.timestamp_millis();
    write_client_fixture(
        &c,
        "abcdefgh",
        serde_json::json!({
            "key": "abcdefgh-key",
            "name": "phone",
            "enabled": true,
            "created_at": 1,
            "last_seen": now - 1_000,
            "last_segment_received_at": now - 1_000,
            "sources": {
                "audio": {"last_accepted_ingest_at": rfc3339_millis(now - 1_000)},
                "": {"last_accepted_ingest_at": rfc3339_millis(now - 700_000), "ingest_rejection": {"active_count": 1, "reason_code":"ingest_rejected", "first":rfc3339_millis(now), "latest":rfc3339_millis(now)}},
            }
        }),
    );
    let capture = result("capture_health", &c);
    let stall = result("client_delivery_stall", &c);
    assert_eq!(capture.status, Status::Warn);
    assert_eq!(stall.status, Status::Warn);
    assert_eq!(
        capture.detail,
        "rollup=attention; the solstone app on phone is having trouble adding default"
    );
    assert_eq!(
        stall.detail,
        "the journal rejected an upload from phone (default needs attention)"
    );
}

#[test]
fn batteries_preserve_staged_home_and_journal() {
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

#[test]
fn unretryable_transcribe_input_reports_healthy_zero_on_clean_journal() {
    let c = fixture();
    let scan_result = crate::checks::unretryable_transcribe_input::scan(&c.journal_path);
    assert_eq!(
        scan_result,
        crate::checks::unretryable_transcribe_input::UnretryableScan::Counted(
            std::collections::BTreeSet::new()
        )
    );

    let row = result("unretryable_transcribe_input", &c);
    assert_eq!(row.status, Status::Ok);
    assert_eq!(
        row.detail,
        "0 recordings the journal will not retry on its own (handler: transcribe, reason_code: corrupt_input)"
    );
    assert_eq!(row.fix, None);
}

#[test]
fn unretryable_transcribe_input_reports_warn_with_ids_and_fix_on_corrupt_input() {
    let c = fixture();
    let segment_dir = c.journal_path.join("chronicle/20260101/120000_60");
    fs::create_dir_all(&segment_dir).unwrap();
    let audio_jsonl = segment_dir.join("audio.jsonl");
    fs::write(
        audio_jsonl,
        "{\"_solstone_processing\":{\"handler\":\"transcribe\",\"state\":\"failed\",\"reason_code\":\"corrupt_input\"}}\n",
    )
    .unwrap();

    let scan_result = crate::checks::unretryable_transcribe_input::scan(&c.journal_path);
    let mut expected_set = std::collections::BTreeSet::new();
    expected_set.insert((
        "20260101".to_owned(),
        "_default".to_owned(),
        "120000_60".to_owned(),
    ));
    assert_eq!(
        scan_result,
        crate::checks::unretryable_transcribe_input::UnretryableScan::Counted(expected_set)
    );

    let row = result("unretryable_transcribe_input", &c);
    assert_eq!(row.status, Status::Warn);
    assert_eq!(
        row.detail,
        "1 recordings the journal will not retry on its own (handler: transcribe, reason_code: corrupt_input): 20260101/_default/120000_60"
    );
    assert_eq!(row.fix, Some("journal transcribe --redo".to_owned()));
}

#[test]
fn unretryable_transcribe_input_formats_direct_and_named_streams_and_truncates() {
    let c = fixture();
    for i in 0..15 {
        let day = format!("202601{:02}", i + 1);
        let segment_dir = c
            .journal_path
            .join(format!("chronicle/{day}/stream_long_name_{i}/120000_60"));
        fs::create_dir_all(&segment_dir).unwrap();
        fs::write(
            segment_dir.join("audio.jsonl"),
            "{\"_solstone_processing\":{\"handler\":\"transcribe\",\"state\":\"failed\",\"reason_code\":\"corrupt_input\"}}\n",
        )
        .unwrap();
    }

    let scan_result = crate::checks::unretryable_transcribe_input::scan(&c.journal_path);
    match scan_result {
        crate::checks::unretryable_transcribe_input::UnretryableScan::Counted(set) => {
            assert_eq!(set.len(), 15);
        }
        other => panic!("expected Counted, got {other:?}"),
    }

    let row = result("unretryable_transcribe_input", &c);
    assert_eq!(row.status, Status::Warn);
    assert!(row.detail.len() <= 400);
    assert!(row.detail.ends_with("..."));
    assert!(row.detail.starts_with(
        "15 recordings the journal will not retry on its own (handler: transcribe, reason_code: corrupt_input): "
    ));
}

#[test]
fn unretryable_transcribe_input_corrupt_input_on_completed_fold_day_is_healthy_to_caught_up_and_warn_to_unretryable()
 {
    let c = fixture();
    configure_daily_work(&c.journal_path, None);
    incomplete(&c, "20260101");

    let segment_dir = c.journal_path.join("chronicle/20260101/120000_60");
    fs::create_dir_all(&segment_dir).unwrap();
    fs::write(
        segment_dir.join("audio.jsonl"),
        "{\"_solstone_processing\":{\"handler\":\"transcribe\",\"state\":\"failed\",\"reason_code\":\"corrupt_input\"}}\n",
    )
    .unwrap();

    health(
        &c,
        "20260101",
        &[
            "{\"event\":\"sense.complete\",\"ts\":1,\"mode\":\"segment\",\"stream\":\"_default\",\"segment\":\"120000_60\",\"density\":\"active\"}",
            "{\"event\":\"talent.complete\",\"ts\":2,\"mode\":\"segment\",\"stream\":\"_default\",\"segment\":\"120000_60\",\"name\":\"documents\"}",
        ],
    );

    let caught_up_row = result("journal_caught_up", &c);
    assert_eq!(caught_up_row.status, Status::Ok);

    let unretryable_row = result("unretryable_transcribe_input", &c);
    assert_eq!(unretryable_row.status, Status::Warn);
}

#[test]
fn unretryable_transcribe_input_ac4_without_sense_complete_trips_caught_up() {
    let c = fixture();
    configure_daily_work(&c.journal_path, None);
    incomplete(&c, "20260101");

    let segment_dir = c.journal_path.join("chronicle/20260101/120000_60");
    fs::create_dir_all(&segment_dir).unwrap();
    fs::write(
        segment_dir.join("audio.jsonl"),
        "{\"_solstone_processing\":{\"handler\":\"transcribe\",\"state\":\"failed\",\"reason_code\":\"corrupt_input\"}}\n",
    )
    .unwrap();

    health(
        &c,
        "20260101",
        &[
            "{\"event\":\"talent.complete\",\"ts\":2,\"mode\":\"segment\",\"stream\":\"_default\",\"segment\":\"120000_60\",\"name\":\"documents\"}",
        ],
    );

    let caught_up_row = result("journal_caught_up", &c);
    assert_eq!(caught_up_row.status, Status::Warn);
}

#[test]
fn unretryable_transcribe_input_ac4_with_unrelated_pending_trips_caught_up() {
    let c = fixture();
    configure_daily_work(&c.journal_path, None);
    incomplete(&c, "20260101");

    let segment_dir = c.journal_path.join("chronicle/20260101/120000_60");
    fs::create_dir_all(&segment_dir).unwrap();
    fs::write(
        segment_dir.join("audio.jsonl"),
        "{\"_solstone_processing\":{\"handler\":\"transcribe\",\"state\":\"failed\",\"reason_code\":\"corrupt_input\"}}\n",
    )
    .unwrap();

    health(
        &c,
        "20260101",
        &[
            "{\"event\":\"sense.complete\",\"ts\":1,\"mode\":\"segment\",\"stream\":\"_default\",\"segment\":\"120000_60\",\"density\":\"active\"}",
            "{\"event\":\"talent.complete\",\"ts\":2,\"mode\":\"segment\",\"stream\":\"_default\",\"segment\":\"120000_60\",\"name\":\"documents\"}",
        ],
    );

    let aged = SystemTime::UNIX_EPOCH
        + Duration::from_millis(
            (c.now.timestamp_millis() - (MODALITY_INPUT_AGED_MS + 1_000)) as u64,
        );
    stage_raw_audio_pending(&c, aged);

    let caught_up_row = result("journal_caught_up", &c);
    assert_eq!(caught_up_row.status, Status::Warn);

    let unretryable_row = result("unretryable_transcribe_input", &c);
    assert_eq!(unretryable_row.status, Status::Warn);
}

#[test]
fn unretryable_transcribe_input_negative_twin_exhausted_decode_transient_is_zero() {
    let c = fixture();
    let segment_dir = c.journal_path.join("chronicle/20260101/120000_60");
    fs::create_dir_all(&segment_dir).unwrap();
    let audio_jsonl = segment_dir.join("audio.jsonl");
    let record = serde_json::json!({
        "handler": solstone_core_processing_record::vocab::HANDLER_TRANSCRIBE,
        "state": solstone_core_processing_record::vocab::STATE_FAILED,
        "reason_code": solstone_core_processing_record::vocab::REASON_DECODE_TRANSIENT,
        "attempts": 3,
    });
    fs::write(
        &audio_jsonl,
        format!("{{\"_solstone_processing\":{}}}\n", record),
    )
    .unwrap();

    let state = solstone_core_system_health::derive_modality_state(
        &segment_dir,
        "audio",
        false,
        true,
        false,
        Some(&record),
        c.now,
    );
    assert_eq!(state, solstone_core_system_health::DataState::FailedFinal);

    let scan_result = crate::checks::unretryable_transcribe_input::scan(&c.journal_path);
    assert_eq!(
        scan_result,
        crate::checks::unretryable_transcribe_input::UnretryableScan::Counted(
            std::collections::BTreeSet::new()
        )
    );

    let row = result("unretryable_transcribe_input", &c);
    assert_eq!(row.status, Status::Ok);
}

#[test]
fn unretryable_transcribe_input_negative_twin_describe_corrupt_input_is_zero() {
    let c = fixture();
    let segment_dir = c.journal_path.join("chronicle/20260101/120000_60");
    fs::create_dir_all(&segment_dir).unwrap();
    fs::write(
        segment_dir.join("screen.jsonl"),
        "{\"_solstone_processing\":{\"handler\":\"describe\",\"state\":\"failed\",\"reason_code\":\"corrupt_input\"}}\n",
    )
    .unwrap();

    let scan_result = crate::checks::unretryable_transcribe_input::scan(&c.journal_path);
    assert_eq!(
        scan_result,
        crate::checks::unretryable_transcribe_input::UnretryableScan::Counted(
            std::collections::BTreeSet::new()
        )
    );

    let row = result("unretryable_transcribe_input", &c);
    assert_eq!(row.status, Status::Ok);
}

#[test]
fn unretryable_transcribe_input_negative_twin_no_audio_stream_is_zero() {
    let c = fixture();
    let segment_dir = c.journal_path.join("chronicle/20260101/120000_60");
    fs::create_dir_all(&segment_dir).unwrap();
    let record = serde_json::json!({
        "handler": solstone_core_processing_record::vocab::HANDLER_TRANSCRIBE,
        "state": solstone_core_processing_record::vocab::STATE_FAILED,
        "reason_code": solstone_core_processing_record::vocab::REASON_NO_AUDIO_STREAM,
        "attempts": 1,
    });
    fs::write(
        segment_dir.join("audio.jsonl"),
        format!("{{\"_solstone_processing\":{}}}\n", record),
    )
    .unwrap();

    let state = solstone_core_system_health::derive_modality_state(
        &segment_dir,
        "audio",
        false,
        true,
        false,
        Some(&record),
        c.now,
    );
    assert_eq!(state, solstone_core_system_health::DataState::FailedFinal);

    let scan_result = crate::checks::unretryable_transcribe_input::scan(&c.journal_path);
    assert_eq!(
        scan_result,
        crate::checks::unretryable_transcribe_input::UnretryableScan::Counted(
            std::collections::BTreeSet::new()
        )
    );

    let row = result("unretryable_transcribe_input", &c);
    assert_eq!(row.status, Status::Ok);
}

#[test]
fn unretryable_transcribe_input_cannot_determine_on_missing_handler_field() {
    let c = fixture();
    let segment_dir = c.journal_path.join("chronicle/20260101/120000_60");
    fs::create_dir_all(&segment_dir).unwrap();
    fs::write(
        segment_dir.join("audio.jsonl"),
        "{\"_solstone_processing\":{\"state\":\"failed\",\"reason_code\":\"corrupt_input\"}}\n",
    )
    .unwrap();

    let scan_result = crate::checks::unretryable_transcribe_input::scan(&c.journal_path);
    assert!(matches!(
        scan_result,
        crate::checks::unretryable_transcribe_input::UnretryableScan::CannotDetermine(_)
    ));

    let row = result("unretryable_transcribe_input", &c);
    assert_eq!(row.status, Status::Warn);
    assert!(row.detail.starts_with(
        "could not determine how many recordings the journal will not retry on its own ("
    ));
}

#[test]
fn unretryable_transcribe_input_cannot_determine_on_oversized_first_row() {
    let c = fixture();
    let segment_dir = c.journal_path.join("chronicle/20260101/120000_60");
    fs::create_dir_all(&segment_dir).unwrap();
    let oversized = "a".repeat(70_000) + "\n";
    fs::write(segment_dir.join("audio.jsonl"), oversized).unwrap();

    let scan_result = crate::checks::unretryable_transcribe_input::scan(&c.journal_path);
    assert!(matches!(
        scan_result,
        crate::checks::unretryable_transcribe_input::UnretryableScan::CannotDetermine(_)
    ));

    let row = result("unretryable_transcribe_input", &c);
    assert_eq!(row.status, Status::Warn);
}

#[test]
fn unretryable_transcribe_input_scans_across_days_outside_backlog_window_and_multiple_streams() {
    let c = fixture();
    // Create 32 days: 20250101 through 20250201
    for i in 1..=31 {
        let day_dir = c.journal_path.join(format!("chronicle/202501{:02}", i));
        fs::create_dir_all(&day_dir).unwrap();
    }
    let day_dir = c.journal_path.join("chronicle/20250201");
    fs::create_dir_all(&day_dir).unwrap();

    // In the oldest day 20250101 (outside the 30-day window):
    // Two named streams with corrupt_input
    let phone_dir = c.journal_path.join("chronicle/20250101/phone/120000_60");
    fs::create_dir_all(&phone_dir).unwrap();
    fs::write(
        phone_dir.join("audio.jsonl"),
        "{\"_solstone_processing\":{\"handler\":\"transcribe\",\"state\":\"failed\",\"reason_code\":\"corrupt_input\"}}\n",
    )
    .unwrap();

    let watch_dir = c.journal_path.join("chronicle/20250101/watch/120000_60");
    fs::create_dir_all(&watch_dir).unwrap();
    fs::write(
        watch_dir.join("audio.jsonl"),
        "{\"_solstone_processing\":{\"handler\":\"transcribe\",\"state\":\"failed\",\"reason_code\":\"corrupt_input\"}}\n",
    )
    .unwrap();

    // Control: tablet stream with decode_transient attempts:1
    let tablet_dir = c.journal_path.join("chronicle/20250101/tablet/120000_60");
    fs::create_dir_all(&tablet_dir).unwrap();
    fs::write(
        tablet_dir.join("audio.jsonl"),
        "{\"_solstone_processing\":{\"handler\":\"transcribe\",\"state\":\"failed\",\"reason_code\":\"decode_transient\",\"attempts\":1}}\n",
    )
    .unwrap();

    let scan_result = crate::checks::unretryable_transcribe_input::scan(&c.journal_path);
    let mut expected_set = std::collections::BTreeSet::new();
    expected_set.insert((
        "20250101".to_owned(),
        "phone".to_owned(),
        "120000_60".to_owned(),
    ));
    expected_set.insert((
        "20250101".to_owned(),
        "watch".to_owned(),
        "120000_60".to_owned(),
    ));

    assert_eq!(
        scan_result,
        crate::checks::unretryable_transcribe_input::UnretryableScan::Counted(expected_set)
    );

    let row = result("unretryable_transcribe_input", &c);
    assert_eq!(row.status, Status::Warn);
    assert!(row.detail.starts_with(
        "2 recordings the journal will not retry on its own (handler: transcribe, reason_code: corrupt_input): "
    ));
    assert!(row.detail.contains("20250101/phone/120000_60"));
    assert!(row.detail.contains("20250101/watch/120000_60"));
    assert!(!row.detail.contains("tablet"));
}

#[test]
#[cfg(unix)]
fn unretryable_transcribe_input_cannot_determine_on_unreadable_directory() {
    use std::os::unix::fs::PermissionsExt;

    struct RestorePerms(PathBuf);
    impl Drop for RestorePerms {
        fn drop(&mut self) {
            let _ = fs::set_permissions(&self.0, fs::Permissions::from_mode(0o755));
        }
    }

    let c = fixture();
    let day_dir = c.journal_path.join("chronicle/20260101");
    let segment_dir = day_dir.join("120000_60");
    fs::create_dir_all(&segment_dir).unwrap();
    fs::write(
        segment_dir.join("audio.jsonl"),
        "{\"_solstone_processing\":{\"handler\":\"transcribe\",\"state\":\"failed\",\"reason_code\":\"corrupt_input\"}}\n",
    )
    .unwrap();

    let _guard = RestorePerms(day_dir.clone());
    fs::set_permissions(&day_dir, fs::Permissions::from_mode(0o000)).unwrap();

    let scan_result = crate::checks::unretryable_transcribe_input::scan(&c.journal_path);
    assert!(
        matches!(
            scan_result,
            crate::checks::unretryable_transcribe_input::UnretryableScan::CannotDetermine(_)
        ),
        "expected CannotDetermine on unreadable day dir, got {:?}",
        scan_result
    );

    let row = result("unretryable_transcribe_input", &c);
    assert_eq!(row.status, Status::Warn);
}

#[test]
fn unretryable_transcribe_input_registered_for_both_platforms() {
    let entry = registry::lookup(Battery::Journal, "unretryable_transcribe_input")
        .expect("unretryable_transcribe_input must be registered in Journal battery");
    assert!(entry.check.platforms.contains(&Platform::Linux));
    assert!(entry.check.platforms.contains(&Platform::Darwin));
}

#[test]
fn unretryable_transcribe_input_ac4_fixture_reports_backlog_view_complete() {
    let c = fixture();
    configure_daily_work(&c.journal_path, None);
    incomplete(&c, "20260101");

    let segment_dir = c.journal_path.join("chronicle/20260101/120000_60");
    fs::create_dir_all(&segment_dir).unwrap();
    fs::write(
        segment_dir.join("audio.jsonl"),
        "{\"_solstone_processing\":{\"handler\":\"transcribe\",\"state\":\"failed\",\"reason_code\":\"corrupt_input\"}}\n",
    )
    .unwrap();

    health(
        &c,
        "20260101",
        &[
            "{\"event\":\"sense.complete\",\"ts\":1,\"mode\":\"segment\",\"stream\":\"_default\",\"segment\":\"120000_60\",\"density\":\"active\"}",
            "{\"event\":\"talent.complete\",\"ts\":2,\"mode\":\"segment\",\"stream\":\"_default\",\"segment\":\"120000_60\",\"name\":\"documents\"}",
        ],
    );

    let source = solstone_core_system_health::FilesystemHealthLogSource::new(&c.journal_path);
    let segment_source = solstone_core_system_health::FilesystemSegmentSource;
    let view = solstone_core_system_health::read_backlog_view(
        &source,
        &segment_source,
        &c.journal_path,
        solstone_core_system_health::BACKLOG_DEFAULT_WINDOW,
        c.now,
    )
    .unwrap();
    let day = view
        .days
        .iter()
        .find(|d| d.day == "20260101")
        .expect("20260101 backlog day exists");
    assert_eq!(
        day.state,
        solstone_core_system_health::BACKLOG_STATE_COMPLETE
    );
    // `day.segments` in `BacklogDay` is `segment_depth` (not_sensed + completion.not_thought), which is 0 for a complete day.
    assert_eq!(day.segments, 0);
    assert_eq!(view.pending_days, 0);
    assert_eq!(view.stuck_days, 0);
}
