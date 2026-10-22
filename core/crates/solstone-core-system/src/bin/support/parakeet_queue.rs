// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native child-visible queue capability proof; never records credential values.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use solstone_core_system::cap::DefaultCapResolver;
use solstone_core_system::process::{ReadFileGrant, ReadFileGrantKind};
use solstone_core_system::provider_runtime::*;
use solstone_core_system::queue::{SystemProcessStateProbe, TaskQueue, TaskQueueOptions};
use solstone_core_system::request::{BusTaskRequest, ExecutionRequest, WireTaskRequest};

pub fn probe(mut args: impl Iterator<Item = String>) {
    let _admission = solstone_core_system::process::receive_windows_launch()
        .expect("native queued child admission");
    assert_eq!(args.next().as_deref(), Some("--queue-auth-probe"));
    let root = PathBuf::from(args.next().expect("proof root"));
    let tag = args.next().expect("proof tag");
    let expected = args.next().expect("capability expectation");
    assert_eq!(std::env::var("KEEP_QUEUE_ENV").as_deref(), Ok("preserved"));
    let result = if expected == "none" {
        for key in ["TOKEN", "NONCE", "PORT"] {
            assert!(
                std::env::var(format!("SOLSTONE_PARAKEET_AUTH_{key}"))
                    .unwrap_or_default()
                    .is_empty()
            );
        }
        serde_json::json!({"capability":"unavailable"})
    } else {
        assert!(
            std::env::var_os(solstone_core_journal_config::work_priority::WORK_PRIORITY_ENV)
                .is_some()
        );
        let port = std::fs::read_to_string(root.join("health/parakeet-cpp.port"))
            .expect("published port")
            .trim()
            .parse::<u16>()
            .expect("port");
        let (authorization, nonce) = parakeet_auth_headers(port).expect("queued capability");
        let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).expect("provider");
        stream
            .set_read_timeout(Some(Duration::from_secs(120)))
            .expect("read deadline");
        let (method, path, body, content_type) = match args.next() {
            Some(wav) => {
                let mut body = b"--queue-proof\r\nContent-Disposition: form-data; name=\"response_format\"\r\n\r\nverbose_json\r\n--queue-proof\r\nContent-Disposition: form-data; name=\"timestamp_granularities[]\"\r\n\r\nword\r\n--queue-proof\r\nContent-Disposition: form-data; name=\"file\"; filename=\"audio.wav\"\r\nContent-Type: audio/wav\r\n\r\n".to_vec();
                body.extend(std::fs::read(wav).expect("public WAV"));
                body.extend(b"\r\n--queue-proof--\r\n");
                (
                    "POST",
                    "/v1/audio/transcriptions",
                    body,
                    "multipart/form-data; boundary=queue-proof",
                )
            }
            None => ("GET", "/health", Vec::new(), "application/json"),
        };
        write!(stream, "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: {authorization}\r\nX-Solstone-Nonce: {nonce}\r\nConnection: close\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n\r\n", body.len()).expect("request headers");
        stream.write_all(&body).expect("request body");
        let mut response = String::new();
        stream
            .read_to_string(&mut response)
            .expect("provider response");
        assert!(
            response.starts_with("HTTP/1.1 200") || response.starts_with("HTTP/1.0 200"),
            "queued request was refused"
        );
        let (_, body) = response.split_once("\r\n\r\n").expect("HTTP body");
        serde_json::from_str::<serde_json::Value>(body).expect("provider JSON")
    };
    let output = root.join(format!("{tag}.json"));
    let mut file = std::fs::File::create_new(output).expect("new proof artifact");
    file.write_all(
        serde_json::to_string(&result)
            .expect("proof JSON")
            .as_bytes(),
    )
    .expect("proof output");
}

fn submit(
    queue: &TaskQueue,
    root: &Path,
    program: &Path,
    tag: &str,
    expected: &str,
    wav: Option<&str>,
) {
    let mut command = vec![
        program.display().to_string(),
        "journal".into(),
        "think".into(),
        "--queue-auth-probe".into(),
        root.display().to_string(),
        tag.into(),
        expected.into(),
    ];
    if let Some(wav) = wav {
        command.push(wav.into());
    }
    let request = BusTaskRequest::decode(
        WireTaskRequest {
            cmd: Some(command),
            reference: Some(tag.into()),
            ..Default::default()
        },
        tag,
    )
    .expect("queue request");
    queue.submit(ExecutionRequest::Bus(request));
    let deadline = Instant::now() + Duration::from_secs(150);
    while !queue.history().iter().any(|entry| entry.reference == tag) {
        assert!(Instant::now() < deadline, "queue proof deadline");
        // Match supervisor ticks: an exited root can briefly precede the Job's
        // quiescence counter, and held completions need a later proof check.
        queue.enforce_deadlines(Instant::now());
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        root.join(format!("{tag}.json")).is_file(),
        "queued child did not produce its proof artifact: {:?}",
        queue.history().iter().find(|entry| entry.reference == tag)
    );
}

pub fn run(mut args: impl Iterator<Item = String>) {
    let root = PathBuf::from(args.next().expect("proof root"));
    let real_package = args.next().map(PathBuf::from);
    let wav = args.next();
    let bin = std::env::current_exe()
        .expect("fixture executable")
        .parent()
        .expect("fixture bin")
        .to_path_buf();
    std::fs::create_dir_all(root.join("config")).expect("config directory");
    std::fs::create_dir_all(root.join("health")).expect("health directory");
    std::fs::write(root.join("config/journal.json"), br#"{"providers":{"active":{"provider":"local"}},"processing":{"gate":{"time_window":{"enabled":false}}}}"#).expect("local work permission");
    let grant_file = std::fs::File::open(root.join("config/journal.json")).expect("retained file");
    let shared = Arc::new(ParakeetRuntimeShared::default());
    let options = || TaskQueueOptions {
        read_file_grants: vec![
            ReadFileGrant::try_clone_read_only(
                ReadFileGrantKind::SpeakersAnalyzeGeneration,
                &grant_file,
            )
            .expect("read-only grant"),
        ],
        journal_root: root.clone(),
        cap_resolver: Arc::new(DefaultCapResolver::new(Duration::from_secs(180))),
        process_state_probe: Arc::new(SystemProcessStateProbe),
        queue_sink: None,
        process_sink: None,
        ready: false,
        before_deadline_commit: None,
        task_binary: Some(bin.join("solstone.exe")),
        child_environment: BTreeMap::from([
            (
                OsString::from("Solstone_Parakeet_Auth_Token"),
                "stale-token".into(),
            ),
            ("solstone_parakeet_auth_nonce".into(), "stale-nonce".into()),
            ("SOLSTONE_PARAKEET_AUTH_PORT".into(), "1".into()),
            ("KEEP_QUEUE_ENV".into(), "preserved".into()),
        ]),
    };
    // Construct before any provider is ready: no credential snapshot can pass.
    let queue = TaskQueue::with_parakeet_runtime(options(), Arc::clone(&shared));
    let unbound = TaskQueue::new(options());
    let package = real_package.unwrap_or_else(|| root.to_path_buf());
    let (binary, model) = if wav.is_some() {
        (
            package.join("lib/solstone-native/parakeet-server.exe"),
            package.join("lib/solstone_journal_models/assets/parakeet/tdt-0.6b-v3-q8_0.gguf"),
        )
    } else {
        let model = root.join("model.bin");
        std::fs::write(&model, "test-ready-auth").expect("fixture model");
        (bin.join("parakeet-server.exe"), model)
    };
    let mut lifecycle = ParakeetLifecycleSeam::with_timeouts(
        Arc::clone(&shared),
        Arc::new(SystemRuntimeClock::default()),
        Duration::from_secs(30),
        Duration::from_millis(20),
        Duration::from_secs(5),
    )
    .with_journal(&root);
    let mut state = ProviderRuntimeState::new(ProviderName::Parakeet);
    let mut launches = Vec::new();
    for generation in 1..=2 {
        let fingerprint = format!("queue-proof-{generation}");
        state.desired_fingerprint = Some(fingerprint.clone());
        shared.record_launch_request(
            Some(fingerprint.clone()),
            ParakeetLaunchConfig {
                binary_backend: "cpu".into(),
                env_updates: BTreeMap::from([
                    ("PARAKEET_ATT_CONTEXT".into(), "128".into()),
                    ("PARAKEET_SUBSAMPLING_TILE".into(), "256".into()),
                ]),
                gpu_index: None,
                binary_path: binary.clone(),
                model_path: model.clone(),
                package_root: Some(package.clone()),
                journal_path: root.clone(),
                threads: 4,
                desired_fingerprint_json: "{}".into(),
                desired_fingerprint_sha256: fingerprint.clone(),
                placement: ParakeetPlacement::Cpu,
            },
        );
        let fence = ProviderFence {
            incarnation: "queue-proof".into(),
            generation,
            fingerprint: Some(fingerprint),
            attempt: 1,
        };
        lifecycle.dispatch_start(&state, &fence);
        let managed = shared
            .wait_for_launch_result(&fence)
            .managed
            .expect("ready native provider");
        let (_, environment) = shared
            .sense_child_environment()
            .expect("provider capability revision");
        let environment = environment.expect("live provider capability");
        std::fs::write(
            root.join("health/parakeet-cpp.port"),
            environment[std::ffi::OsStr::new("SOLSTONE_PARAKEET_AUTH_PORT")]
                .to_string_lossy()
                .as_bytes(),
        )
        .expect("publish port");
        queue.set_ready();
        submit(
            &queue,
            &root,
            &bin.join("solstone.exe"),
            &format!("authenticated-{generation}"),
            "ready",
            wav.as_deref(),
        );
        launches.push((fence, managed));
    }
    let outside = root.join("outside/solstone.exe");
    std::fs::create_dir_all(outside.parent().expect("outside directory"))
        .expect("outside directory");
    std::fs::copy(bin.join("solstone.exe"), &outside).expect("unrelated same-basename executable");
    submit(&queue, &root, &outside, "unrelated", "none", None);
    unbound.set_ready();
    submit(
        &unbound,
        &root,
        &bin.join("solstone.exe"),
        "unbound",
        "none",
        None,
    );
    for (fence, managed) in launches {
        state.pending_stop_request = Some(ProviderStopCleanupRequest {
            managed,
            reason_code: ReasonCode::known("intent-removed"),
            target_phase: RuntimePhase::Stopped,
            target_reason_code: None,
            admission_exclusive: false,
            orphaned_start_outcome: false,
        });
        lifecycle.dispatch_stop(&state, &fence);
        assert_eq!(
            shared.wait_for_stop_cleanup_result(&fence).status,
            StopCleanupStatus::Stopped
        );
    }
    submit(
        &queue,
        &root,
        &bin.join("solstone.exe"),
        "revoked",
        "none",
        None,
    );
    queue.shutdown();
    unbound.shutdown();
    println!("PARAKEET_QUEUE_CAPABILITY_PROOF=pass");
}
