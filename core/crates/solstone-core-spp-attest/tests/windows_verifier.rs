// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native process and signed-inventory boundary; test hooks never ship.
#![cfg(windows)]

use minisign::KeyPair;
use solstone_core_distribution::{
    manifest_verify::install_test_fixture_pin,
    windows_payload::{
        WINDOWS_PAYLOAD_MANIFEST, WINDOWS_PAYLOAD_SIGNATURE, render_windows_payload_manifest,
    },
};
use solstone_core_spp_attest::{
    error::GpuAppraisalReason,
    nvgpu::{appraise::run_nvattest_for_tests, build_nvattest_attest_command, locate_nvattest},
};
use std::{
    fs,
    io::Cursor,
    path::{Path, PathBuf},
    time::Duration,
};

const RUNTIME: [&str; 3] = ["msvcp140.dll", "vcruntime140.dll", "vcruntime140_1.dll"];

fn key() -> &'static KeyPair {
    static KEY: std::sync::OnceLock<KeyPair> = std::sync::OnceLock::new();
    KEY.get_or_init(|| {
        let key = KeyPair::generate_unencrypted_keypair().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let pin = dir.path().join("pin.pub");
        fs::write(&pin, key.pk.to_box().unwrap().to_bytes()).unwrap();
        install_test_fixture_pin(&pin).unwrap();
        key
    })
}

fn fixture() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("signed ü payload");
    fs::create_dir_all(root.join("bin")).unwrap();
    fs::create_dir_all(root.join("share/ca")).unwrap();
    fs::copy(
        std::env::current_exe().unwrap(),
        root.join("bin/nvattest.exe"),
    )
    .unwrap();
    let system =
        PathBuf::from(std::env::var_os("SystemRoot").expect("SystemRoot")).join("System32");
    for name in RUNTIME {
        fs::copy(system.join(name), root.join("bin").join(name)).expect("VC runtime in System32");
    }
    fs::write(root.join("share/ca/ca-bundle.pem"), b"fixture CA").unwrap();
    sign(&root);
    fs::write(
        root.with_extension("pin.pub"),
        key().pk.to_box().unwrap().to_bytes(),
    )
    .unwrap();
    dir
}

fn sign(root: &Path) {
    let bytes = render_windows_payload_manifest(root, &"a".repeat(40), &"b".repeat(64)).unwrap();
    let key = key();
    let signature = minisign::sign(
        Some(&key.pk),
        &key.sk,
        Cursor::new(&bytes),
        None,
        Some("fixture payload"),
    )
    .unwrap();
    fs::create_dir_all(root.join("share/provenance")).unwrap();
    fs::write(root.join(WINDOWS_PAYLOAD_MANIFEST), bytes).unwrap();
    fs::write(
        root.join(WINDOWS_PAYLOAD_SIGNATURE),
        signature.into_string(),
    )
    .unwrap();
}

#[test]
fn signed_layout_refuses_missing_or_changed_executable_ca_and_runtime() {
    let dir = fixture();
    let root = dir.path().join("signed ü payload");
    assert!(locate_nvattest(&root).is_ok());
    for path in [
        "bin/nvattest.exe",
        "share/ca/ca-bundle.pem",
        "bin/msvcp140.dll",
        "bin/vcruntime140.dll",
        "bin/vcruntime140_1.dll",
    ] {
        let path = root.join(path);
        let original = fs::read(&path).unwrap();
        fs::remove_file(&path).unwrap();
        assert!(
            locate_nvattest(&root).is_err(),
            "missing {}",
            path.display()
        );
        fs::write(&path, &original).unwrap();
        // Same length forces a digest check rather than a tree-size refusal.
        let mut changed = original.clone();
        changed[0] ^= 1;
        fs::write(&path, changed).unwrap();
        assert!(
            locate_nvattest(&root).is_err(),
            "changed {}",
            path.display()
        );
        fs::write(&path, original).unwrap();
    }
    fs::write(root.join("bin/unexpected.dll"), b"planted").unwrap();
    assert_eq!(
        locate_nvattest(&root),
        Err(GpuAppraisalReason::NvattestIntegrityFailed)
    );
}

#[test]
fn production_spawn_clears_hostile_environment_and_uses_the_verified_directory() {
    let dir = fixture();
    let root = dir.path().join("signed ü payload");
    let hostile = dir.path().join("hostile");
    fs::create_dir(&hostile).unwrap();
    fs::write(hostile.join("vcruntime140.dll"), b"planted").unwrap();
    // A separate driver process owns the hostile environment; no test mutates
    // the harness process's global environment or current directory.
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "hostile_launch_driver",
            "--ignored",
            "--nocapture",
        ])
        .env("SOLSTONE_TEST_PACKAGE", &root)
        .env("PATH", &hostile)
        .env("OPENSSL_CONF", hostile.join("evil.cnf"))
        .env("OPENSSL_MODULES", &hostile)
        .env("SSL_CERT_DIR", &hostile)
        .env("CURL_CA_BUNDLE", "evil")
        .env("NVAT_RIM_SERVICE_BASE_URL", "evil")
        .env("HTTPS_PROXY", "evil")
        .current_dir(&hostile)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value =
        serde_json::from_slice(&fs::read(root.with_extension("child-report.json")).unwrap())
            .unwrap();
    let cwd = PathBuf::from(report["cwd"].as_str().unwrap());
    assert_eq!(
        fs::canonicalize(cwd).unwrap(),
        fs::canonicalize(root.join("bin")).unwrap()
    );
    assert_eq!(report["forbidden"], serde_json::json!([]));
    assert!(!report["path"].as_str().unwrap().contains("hostile"));
}

#[test]
#[ignore = "child process entry"]
fn hostile_launch_driver() {
    let root = PathBuf::from(std::env::var_os("SOLSTONE_TEST_PACKAGE").unwrap());
    install_test_fixture_pin(&root.with_extension("pin.pub")).unwrap();
    let mut command =
        build_nvattest_attest_command(&root, &root.join("evidence.json"), &[7; 32], "remote", None)
            .unwrap();
    command.argv.truncate(1);
    command.argv.extend(
        [
            "--exact",
            "windows_child_launch_report",
            "--ignored",
            "--nocapture",
        ]
        .map(Into::into),
    );
    // The launch boundary enforces its policy even if an invocation was
    // prepared with extra ambient configuration.
    command.env.insert("OPENSSL_CONF".into(), "evil".into());
    let output = run_nvattest_for_tests(command, Duration::from_secs(10)).unwrap();
    assert!(output.status.success());
}

#[test]
#[ignore = "child process entry"]
fn windows_child_launch_report() {
    let root = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let forbidden: Vec<_> = std::env::vars()
        .map(|(name, _)| name.to_ascii_uppercase())
        .filter(|name| {
            name.starts_with("OPENSSL_")
                || name.starts_with("SSL_CERT_")
                || name.starts_with("NVAT_")
                || name.contains("PROXY")
                || name == "CURL_CA_BUNDLE"
        })
        .collect();
    let report = serde_json::json!({"cwd": std::env::current_dir().unwrap(), "path": std::env::var("PATH").unwrap(), "forbidden": forbidden});
    fs::write(
        root.with_extension("child-report.json"),
        serde_json::to_vec(&report).unwrap(),
    )
    .unwrap();
}
