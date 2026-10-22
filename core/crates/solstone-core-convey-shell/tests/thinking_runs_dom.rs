// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::io::ErrorKind;
use std::process::Command;

#[test]
fn thinking_runs_dom_contract() {
    match Command::new("node").arg("--version").output() {
        Err(error) if error.kind() == ErrorKind::NotFound => return,
        Err(error) => panic!("node availability probe failed: {error}"),
        Ok(output) if !output.status.success() => panic!(
            "node availability probe failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        ),
        Ok(_) => {}
    }
    let output = Command::new("node")
        .arg(format!(
            "{}/tests/thinking_runs_dom.js",
            env!("CARGO_MANIFEST_DIR")
        ))
        .arg(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("Thinking DOM harness starts");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "Thinking DOM harness failed:\nstdout:\n{}\nstderr:\n{}",
        stdout,
        String::from_utf8_lossy(&output.stderr),
    );
    assert!(
        stdout.starts_with("DOM CASES: ") && stdout.contains(" passed"),
        "Thinking DOM harness did not report its internal case count:\n{stdout}"
    );
    println!("{}", stdout.trim());
}

#[test]
#[cfg(feature = "full-tests")]
fn thinking_failed_installs_dom_from_persisted_status() {
    use serde_json::json;
    use solstone_core_local::install::archive::ArchiveError;
    use solstone_core_local::install::{
        AcquisitionTestGuard, LocalHardwareAdmission, run_local_with_gpu_admission, status,
        test_hooks::{FakeRuntimeFetch, with_fake_runtime_fetch},
        windows_engine::{WindowsLlamaPackage, set_test_windows_llama_package},
    };

    struct FetchFailure(u8);
    impl FakeRuntimeFetch for FetchFailure {
        fn fetch(&self, _: &str, _: &str, _: u64) -> Result<Vec<u8>, ArchiveError> {
            Err(match self.0 {
                0 => ArchiveError::Download("<img src=x onerror=alert(1)> private/path".into()),
                1 => ArchiveError::DigestMismatch {
                    expected: "expected".into(),
                    actual: "<img src=x onerror=alert(1)>".into(),
                },
                2 => ArchiveError::SizeMismatch {
                    expected: 100,
                    actual: 1,
                },
                _ => ArchiveError::Io(std::io::Error::new(
                    std::io::ErrorKind::StorageFull,
                    "<img src=x onerror=alert(1)>",
                )),
            })
        }
    }
    struct PackageReset;
    impl Drop for PackageReset {
        fn drop(&mut self) {
            set_test_windows_llama_package(None);
        }
    }
    let _package_reset = PackageReset;
    set_test_windows_llama_package(Some(WindowsLlamaPackage::mock()));
    let eligible_admission = if cfg!(target_os = "macos") {
        LocalHardwareAdmission::AppleSilicon {
            total_bytes: Some(16 * 1024 * 1024 * 1024),
        }
    } else if cfg!(target_os = "windows") {
        LocalHardwareAdmission::WindowsObserved {
            probe_ok: true,
            devices: vec![solstone_core_local::VulkanDevice {
                index: 0,
                name: "6 GiB fixture".into(),
                device_type: Some(2),
                vram_mib: 6144,
            }],
            override_index: None,
        }
    } else {
        LocalHardwareAdmission::GpuHost {
            backend: solstone_core_local::Backend::Vulkan,
            nvidia_detected: false,
            nvidia_memory_bytes: None,
            probe_ok: true,
            devices: vec![solstone_core_local::VulkanDevice {
                index: 0,
                name: "6 GiB fixture".into(),
                device_type: Some(2),
                vram_mib: 6144,
            }],
            override_index: None,
        }
    };
    let below_bar_admission = if cfg!(target_os = "macos") {
        LocalHardwareAdmission::AppleSilicon {
            total_bytes: Some(8 * 1024 * 1024 * 1024),
        }
    } else if cfg!(target_os = "windows") {
        LocalHardwareAdmission::WindowsObserved {
            probe_ok: true,
            devices: vec![solstone_core_local::VulkanDevice {
                index: 0,
                name: "4 GiB fixture".into(),
                device_type: Some(2),
                vram_mib: 4096,
            }],
            override_index: None,
        }
    } else {
        LocalHardwareAdmission::GpuHost {
            backend: solstone_core_local::Backend::Vulkan,
            nvidia_detected: false,
            nvidia_memory_bytes: None,
            probe_ok: true,
            devices: vec![solstone_core_local::VulkanDevice {
                index: 0,
                name: "4 GiB fixture".into(),
                device_type: Some(2),
                vram_mib: 4096,
            }],
            override_index: None,
        }
    };
    let root = tempfile::tempdir().unwrap();
    let mut cases = Vec::new();
    for (kind, class) in [
        (0, "download_failed"),
        (1, "integrity_failed"),
        (2, "integrity_failed"),
        (3, "disk_failed"),
    ] {
        let journal = root.path().join(format!("case-{kind}"));
        std::fs::create_dir(&journal).unwrap();
        let trace = AcquisitionTestGuard::new(false);
        let fake = std::rc::Rc::new(FetchFailure(kind));
        let payload = json!({"journal": journal});
        let error = with_fake_runtime_fetch(&fake, || {
            run_local_with_gpu_admission(payload.as_object().unwrap(), eligible_admission.clone())
                .map_err(Box::new)
        })
        .unwrap_err();
        assert!(
            trace.acquisitions() > 0,
            "the real acquisition boundary must run"
        );
        let expected = error.envelope.error.unwrap().reason_code;
        let stored = status::read_status(&journal, "local").unwrap();
        assert_eq!(stored.install_state, "failed");
        assert_eq!(stored.error_code.as_deref(), Some(expected.as_str()));
        let projected =
            solstone_core_thinking::local::bootstrap_status(&journal, "local/qwen3.5-4b");
        assert_eq!(projected["error_code"], expected);
        cases.push(json!({"status": projected, "class": class}));
    }
    // A first-class memory refusal needs no package or model acquisition.
    let journal = root.path().join("below-bar");
    let trace = AcquisitionTestGuard::new(false);
    let payload = json!({"journal": journal});
    let error = run_local_with_gpu_admission(payload.as_object().unwrap(), below_bar_admission)
        .unwrap_err();
    assert_eq!(trace.acquisitions(), 0);
    let (expected_reason, class) = if cfg!(target_os = "macos") {
        ("memory_insufficient", "memory_failed")
    } else {
        ("gpu_memory_insufficient", "gpu_memory_insufficient")
    };
    assert_eq!(error.envelope.error.unwrap().reason_code, expected_reason);
    cases.push(json!({"status": solstone_core_thinking::local::bootstrap_status(&journal, "local/qwen3.5-4b"), "class": class}));
    let mut warnings = Vec::new();
    if !cfg!(target_os = "windows") {
        let journal = root.path().join("unreadable-memory");
        let unknown = if cfg!(target_os = "macos") {
            LocalHardwareAdmission::AppleSilicon { total_bytes: None }
        } else {
            LocalHardwareAdmission::GpuHost {
                backend: solstone_core_local::Backend::Vulkan,
                nvidia_detected: false,
                nvidia_memory_bytes: None,
                probe_ok: true,
                devices: vec![solstone_core_local::VulkanDevice {
                    index: 0,
                    name: "unknown memory".into(),
                    device_type: Some(2),
                    vram_mib: 0,
                }],
                override_index: None,
            }
        };
        let trace = AcquisitionTestGuard::new(true);
        let request = json!({"journal": journal});
        let _ = run_local_with_gpu_admission(request.as_object().unwrap(), unknown);
        assert!(trace.acquisitions() > 0);
        let admitted = status::read_status(&journal, "local").unwrap();
        assert!(admitted.admission_warning.is_some());
        let lease = solstone_core_local::install::lease::acquire(&journal, "local")
            .unwrap()
            .unwrap();
        status::begin_or_replace(
            &journal,
            "local",
            admitted.target_fingerprint_json.unwrap(),
            admitted.target_fingerprint_sha256.unwrap(),
            None,
            "downloading",
            admitted.admission_warning,
        )
        .unwrap();
        let initial = solstone_core_thinking::local::bootstrap_status(&journal, "local/qwen3.5-4b");
        let mut progress = status::read_status(&journal, "local").unwrap();
        progress.progress_bytes_received = Some(10);
        progress.progress_bytes_total = Some(20);
        status::write_status(
            &journal,
            status::transition(progress, "downloading", None, None).unwrap(),
        )
        .unwrap();
        let progress =
            solstone_core_thinking::local::bootstrap_status(&journal, "local/qwen3.5-4b");
        let terminal = status::transition(
            status::read_status(&journal, "local").unwrap(),
            "failed",
            Some("download_failed".into()),
            Some("download_failed".into()),
        )
        .unwrap();
        status::write_status(&journal, terminal).unwrap();
        drop(lease);
        let terminal =
            solstone_core_thinking::local::bootstrap_status(&journal, "local/qwen3.5-4b");
        let availability =
            solstone_core_thinking::local::availability(&journal, "local/qwen3.5-4b");
        let _ =
            run_local_with_gpu_admission(request.as_object().unwrap(), eligible_admission.clone());
        let cleared = solstone_core_thinking::local::bootstrap_status(&journal, "local/qwen3.5-4b");
        assert!(cleared["admission_warning"].is_null());
        warnings.push(json!({"initial":initial,"progress":progress,"terminal":terminal,"cleared":cleared,"availability":availability}));
    }
    let receipts = root.path().join("receipts.json");
    std::fs::write(
        &receipts,
        serde_json::to_vec(&json!({
            "copy": solstone_core_thinking_copy::thinking_copy_payload(), "cases": cases, "warnings": warnings,
        }))
        .unwrap(),
    )
    .unwrap();
    let output = Command::new("node")
        .arg(format!(
            "{}/tests/thinking_runs_dom.js",
            env!("CARGO_MANIFEST_DIR")
        ))
        .arg(env!("CARGO_MANIFEST_DIR"))
        .arg(receipts)
        .output()
        .expect("Node is required for persisted install reason acceptance");
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("PERSISTED INSTALL CASES: 5 passed"));
}
