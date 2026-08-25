// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#[cfg(target_os = "linux")]
mod linux {
    use std::env;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::sync::OnceLock;
    use std::time::Duration;

    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use chrono::{Duration as ChronoDuration, Utc};
    use serde_json::{Value, json};
    use solstone_core_brain::{begin_refresh, finish_refresh};
    use solstone_core_convey_shell::router;
    use solstone_core_local::install::{manifest, pins, status};
    use tokio::sync::Mutex;
    use tower::ServiceExt;

    const MODEL_ID: &str = "local/qwen3.5-4b";
    const DESIRED_FINGERPRINT: &str =
        "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";

    static PATH_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

    fn temporary_journal(name: &str, provider: &str) -> tempfile::TempDir {
        let root = tempfile::Builder::new()
            .prefix(&format!("solstone-thinking-{name}-"))
            .tempdir_in("/var/tmp")
            .expect("temporary journal");
        let config = json!({
            "setup":{"completed_at":1767225600_u64},
            "providers":{"active":{"provider":provider,"model":if provider == "local" { MODEL_ID } else { "gpt-5" }}},
        });
        fs::create_dir_all(root.path().join("config")).expect("config directory creates");
        fs::write(
            root.path().join("config/journal.json"),
            serde_json::to_vec(&config).expect("config serializes"),
        )
        .expect("config writes");
        root
    }

    fn write_runtime_health(journal: &Path, phase: &str) {
        let path = journal.join("health/providers/runtime/local.json");
        fs::create_dir_all(path.parent().expect("runtime directory")).expect("runtime directory");
        fs::write(
            path,
            serde_json::to_vec(&json!({
                "schema_version":1,
                "provider":"local",
                "revision":3,
                "phase":phase,
                "reason_code":null,
                "detail":{},
                "desired_fingerprint_sha256":DESIRED_FINGERPRINT,
                "incarnation":null,
                "generation":0,
                "attempt":0,
                "process":null,
                "updated_at":"2026-08-18T00:00:00Z",
                "display_deadline_at":null,
                "owner":null,
            }))
            .expect("runtime health serializes"),
        )
        .expect("runtime health writes");
    }

    fn seed_ready_brain(journal: &Path) {
        write_runtime_health(journal, "ready");
        let now = Utc::now();
        let evidence = json!({
            "status":"ok",
            "observed_at":now.to_rfc3339(),
            "expires_at":(now + ChronoDuration::days(1)).to_rfc3339(),
        });
        let permit = begin_refresh(
            journal,
            now,
            None,
            None,
            false,
            Some(DESIRED_FINGERPRINT.to_owned()),
        )
        .expect("brain refresh starts")
        .expect("brain refresh permit");
        finish_refresh(
            journal,
            permit,
            json!({
                "configuration":evidence,
                "lane_prerequisites":evidence,
                "generate":evidence,
                "cogitate":evidence,
            }),
            now,
            Some(DESIRED_FINGERPRINT.to_owned()),
        )
        .expect("brain refresh finishes");
    }

    fn runtime_root(journal: &Path, backend: &str) -> (PathBuf, Value) {
        let root = pins::cache_root(journal);
        let key = pins::platform_key();
        match backend {
            "cuda" => {
                let (_, digest, _) = pins::cuda_pin(&key).expect("CUDA pin");
                (
                    root.join("cuda").join(key).join(digest),
                    pins::cuda_identity(&pins::platform_key()).expect("CUDA identity"),
                )
            }
            "vulkan" => {
                let (release, _, _, _) = pins::vulkan_pin(&key).expect("Vulkan pin");
                (
                    root.join("bin").join(key).join(release),
                    pins::vulkan_identity(&pins::platform_key()).expect("Vulkan identity"),
                )
            }
            _ => panic!("unknown backend {backend}"),
        }
    }

    fn write_runtime_artifact(journal: &Path, backend: &str) {
        let (root, identity) = runtime_root(journal, backend);
        fs::create_dir_all(&root).expect("runtime root creates");
        fs::write(root.join("llama-server"), b"fixture runtime").expect("runtime writes");
        let manifest = manifest::build_manifest(
            "local",
            if backend == "cuda" {
                "llama-server-cuda"
            } else {
                "llama-server-vulkan"
            },
            "target",
            json!({"pin_identity":identity}),
            manifest::runtime_inventory(&root, &[]).expect("runtime inventory"),
            None,
            None,
        )
        .expect("runtime manifest builds");
        manifest::write_manifest(&manifest::artifact_manifest_path(&root), &manifest)
            .expect("runtime manifest writes");
    }

    fn write_model_artifact(journal: &Path) {
        let identity = pins::model_identity(MODEL_ID).expect("model identity");
        let root = pins::cache_root(journal)
            .join("models")
            .join(MODEL_ID.replace('/', "__"));
        fs::create_dir_all(&root).expect("model root creates");
        for name in [
            identity["filename"].as_str().expect("model filename"),
            identity["mmproj_filename"]
                .as_str()
                .expect("projector filename"),
        ] {
            fs::write(root.join(name), b"fixture model").expect("model artifact writes");
        }
        let manifest = manifest::build_manifest(
            "local",
            "local-model",
            "target",
            json!({"pin_identity":identity}),
            manifest::inventory_for_tree(&root, "model").expect("model inventory"),
            None,
            None,
        )
        .expect("model manifest builds");
        manifest::write_manifest(&manifest::artifact_manifest_path(&root), &manifest)
            .expect("model manifest writes");
    }

    fn write_backend_status(journal: &Path, backend: Option<&str>) {
        let mut install = status::idle_status("local");
        install.target_fingerprint_json =
            backend.map(|backend| json!({"backend":backend}).to_string());
        install.target_fingerprint_sha256 = backend.map(|_| "target".to_owned());
        status::write_status(journal, install).expect("install status writes");
    }

    fn write_malformed_status(journal: &Path) {
        let path = status::status_path(journal, "local");
        fs::create_dir_all(path.parent().expect("status directory")).expect("status directory");
        fs::write(path, b"{").expect("malformed status writes");
    }

    async fn request_json(journal: &Path, path: &str) -> Value {
        let response = router(journal.to_path_buf())
            .oneshot(
                Request::get(path)
                    .body(Body::empty())
                    .expect("request builds"),
            )
            .await
            .expect("router responds");
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        serde_json::from_slice(
            &to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("response reads"),
        )
        .expect("response is JSON")
    }

    async fn request_json_with_watchdog(journal: &Path, path: &str) -> Value {
        let journal = journal.to_path_buf();
        let path = path.to_owned();
        let task_path = path.clone();
        let mut task = tokio::spawn(async move { request_json(&journal, &task_path).await });
        match tokio::time::timeout(Duration::from_secs(1), &mut task).await {
            Ok(Ok(value)) => value,
            Ok(Err(error)) => panic!("{path} request task failed: {error}"),
            Err(_) => {
                task.abort();
                panic!("{path} blocked for more than one second")
            }
        }
    }

    fn local_status(body: Value, path: &str) -> Value {
        match path {
            "/app/thinking/api/state" => body["providers"]["provider_status"]["local"].clone(),
            "/app/thinking/api/providers" => body["provider_status"]["local"].clone(),
            "/app/thinking/api/providers/local/status" => body,
            _ => panic!("unexpected provider route {path}"),
        }
    }

    async fn assert_provider_issues(journal: &Path, expected: &[&str]) {
        for path in [
            "/app/thinking/api/state",
            "/app/thinking/api/providers",
            "/app/thinking/api/providers/local/status",
        ] {
            let local = local_status(request_json(journal, path).await, path);
            assert_eq!(local["issues"], json!(expected), "{path}");
        }
    }

    fn runtime_phases() -> Vec<String> {
        serde_json::from_str::<Value>(include_str!("../../../fixtures/local_contract.json"))
            .expect("local contract parses")["brain_state"]["runtime_phases"]
            .as_array()
            .expect("runtime phases")
            .iter()
            .map(|phase| phase.as_str().expect("runtime phase string").to_owned())
            .collect()
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn provider_status_reads_never_probe_nvidia_and_replay_persisted_artifacts() {
        let _path_lock = PATH_LOCK.get_or_init(|| Mutex::new(())).lock().await;
        let probe = tempfile::Builder::new()
            .prefix("solstone-thinking-provider-probe-")
            .tempdir_in("/var/tmp")
            .expect("probe fixture");
        let bin = probe.path().join("bin");
        let receipt = probe.path().join("nvidia-smi-receipt");
        fs::create_dir(&bin).expect("fake bin creates");
        let shim = bin.join("nvidia-smi");
        fs::write(
            &shim,
            "#!/bin/sh\nprintf '%s\\n' \"$$\" >> \"$SOLSTONE_TEST_NVIDIA_RECEIPT\"\n/bin/sleep 5\n",
        )
        .expect("blocking fake nvidia-smi writes");
        fs::set_permissions(&shim, fs::Permissions::from_mode(0o755))
            .expect("blocking fake nvidia-smi is executable");
        let mut paths = vec![bin];
        if let Some(current) = env::var_os("PATH") {
            paths.extend(env::split_paths(&current));
        }
        let path = env::join_paths(paths).expect("PATH joins");
        // temp-env owns the RAII restoration guard for this outer scope. The
        // watchdog below wraps only individual requests, so PATH is restored
        // on normal completion or unwinding from a timeout assertion.
        temp_env::with_vars(
            [
                ("PATH", Some(path.as_os_str())),
                ("SOLSTONE_TEST_NVIDIA_RECEIPT", Some(receipt.as_os_str())),
            ],
            || {
                tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current().block_on(async {
                        let bundled = temporary_journal("probe-free", "local");
                        seed_ready_brain(bundled.path());
                        for path in [
                            "/app/thinking/api/state",
                            "/app/thinking/api/providers",
                            "/app/thinking/api/providers/local/status",
                        ] {
                            assert!(!receipt.exists(), "receipt must begin absent for {path}");
                            let _ = request_json_with_watchdog(bundled.path(), path).await;
                            assert!(
                                !receipt.exists(),
                                "{path} reached nvidia-smi through a provider-status read"
                            );
                        }

                        let cloud = temporary_journal("cloud-skips-local", "openai");
                        for path in [
                            "/app/thinking/api/state",
                            "/app/thinking/api/providers",
                            "/app/thinking/api/providers/local/status",
                        ] {
                            let local = local_status(
                                request_json_with_watchdog(cloud.path(), path).await,
                                path,
                            );
                            assert_eq!(
                                local,
                                json!({"selected":false,"configured":false,"generate_ready":false,"cogitate_ready":false,"issues":[]}),
                                "{path} non-local provider projection"
                            );
                            assert!(
                                !receipt.exists(),
                                "{path} reached nvidia-smi for a non-local provider"
                            );
                        }
                    });
                });
            },
        );

        for backend in ["cuda", "vulkan"] {
            for (binary, model, expected) in [
                (
                    false,
                    false,
                    vec![
                        "binary_missing",
                        "model_missing",
                        "run `journal install-provider local`",
                    ],
                ),
                (
                    true,
                    false,
                    vec!["model_missing", "run `journal install-provider local`"],
                ),
                (
                    false,
                    true,
                    vec!["binary_missing", "run `journal install-provider local`"],
                ),
                (true, true, Vec::new()),
            ] {
                let journal = temporary_journal("artifact-matrix", "local");
                seed_ready_brain(journal.path());
                write_backend_status(journal.path(), Some(backend));
                if binary {
                    write_runtime_artifact(journal.path(), backend);
                }
                if model {
                    write_model_artifact(journal.path());
                }
                assert_provider_issues(journal.path(), &expected).await;
                if binary && model {
                    let local = local_status(
                        request_json(journal.path(), "/app/thinking/api/providers").await,
                        "/app/thinking/api/providers",
                    );
                    assert_eq!(local["generate_ready"], true);
                    assert_eq!(local["cogitate_ready"], true);
                }
            }
        }

        for status_case in ["missing", "malformed", "missing_backend", "unknown_backend"] {
            let journal = temporary_journal("invalid-status", "local");
            seed_ready_brain(journal.path());
            write_model_artifact(journal.path());
            match status_case {
                "missing" => {}
                "malformed" => write_malformed_status(journal.path()),
                "missing_backend" => write_backend_status(journal.path(), None),
                "unknown_backend" => write_backend_status(journal.path(), Some("metal")),
                _ => unreachable!(),
            }
            assert_provider_issues(
                journal.path(),
                &["binary_missing", "run `journal install-provider local`"],
            )
            .await;
        }

        let alternate = temporary_journal("alternate-backend", "local");
        seed_ready_brain(alternate.path());
        write_backend_status(alternate.path(), Some("cuda"));
        write_runtime_artifact(alternate.path(), "vulkan");
        write_model_artifact(alternate.path());
        assert_provider_issues(
            alternate.path(),
            &["binary_missing", "run `journal install-provider local`"],
        )
        .await;

        for phase in runtime_phases() {
            let journal = temporary_journal("runtime-phase", "local");
            seed_ready_brain(journal.path());
            write_backend_status(journal.path(), Some("vulkan"));
            write_runtime_artifact(journal.path(), "vulkan");
            write_model_artifact(journal.path());
            write_runtime_health(journal.path(), &phase);
            assert_provider_issues(journal.path(), &[]).await;
            let providers = request_json(journal.path(), "/app/thinking/api/providers").await;
            assert_eq!(providers["local_runtime"]["phase"], phase, "runtime phase");
            let local = local_status(providers, "/app/thinking/api/providers");
            assert_eq!(
                local["generate_ready"],
                phase == "ready",
                "generate readiness"
            );
            assert_eq!(
                local["cogitate_ready"],
                phase == "ready",
                "cogitate readiness"
            );
        }

        let corrupt_runtime = temporary_journal("corrupt-runtime", "local");
        seed_ready_brain(corrupt_runtime.path());
        write_backend_status(corrupt_runtime.path(), Some("vulkan"));
        write_runtime_artifact(corrupt_runtime.path(), "vulkan");
        write_model_artifact(corrupt_runtime.path());
        let runtime_path = corrupt_runtime
            .path()
            .join("health/providers/runtime/local.json");
        fs::write(&runtime_path, b"{").expect("corrupt runtime writes");
        assert_provider_issues(corrupt_runtime.path(), &[]).await;
        let corrupt_local = local_status(
            request_json(corrupt_runtime.path(), "/app/thinking/api/providers").await,
            "/app/thinking/api/providers",
        );
        assert_eq!(corrupt_local["generate_ready"], false);
        assert_eq!(corrupt_local["cogitate_ready"], false);

        let stale_brain = temporary_journal("stale-brain", "local");
        seed_ready_brain(stale_brain.path());
        write_backend_status(stale_brain.path(), Some("vulkan"));
        write_runtime_artifact(stale_brain.path(), "vulkan");
        write_model_artifact(stale_brain.path());
        let brain_path = stale_brain.path().join("health/brain.json");
        let mut brain: Value =
            serde_json::from_slice(&fs::read(&brain_path).expect("brain record reads"))
                .expect("brain record parses");
        brain["fingerprint_sha256"] = Value::String("x".repeat(64));
        fs::write(
            brain_path,
            serde_json::to_vec(&brain).expect("brain record serializes"),
        )
        .expect("brain record writes");
        assert_provider_issues(stale_brain.path(), &[]).await;
        let stale_local = local_status(
            request_json(stale_brain.path(), "/app/thinking/api/providers").await,
            "/app/thinking/api/providers",
        );
        assert_eq!(stale_local["generate_ready"], false);
        assert_eq!(stale_local["cogitate_ready"], false);
    }
}
