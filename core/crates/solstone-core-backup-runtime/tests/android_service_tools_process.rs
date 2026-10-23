// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Process-global integration test for Android service tools OnceLock lifecycle
//! and environment variable isolation.
//!
//! Because `solstone-core-spp-attest::android_service_tools` uses a process-global
//! `OnceLock` for admitted tools, this test runs in its own binary target to ensure
//! isolation from other tests.

#![cfg(not(windows))]

use std::fs;
use std::path::Path;

use solstone_core_backup_runtime::engine::{Clock, prepare};
use solstone_core_backup_runtime::resolve::{
    LiveAndroidAuthority, ToolPlatform, resolve_operational_tools_for_platform,
    resolve_restic_member_for_platform, resolve_tools_for_platform,
};
use solstone_core_backup_runtime::runner::ToolRunner;
use solstone_core_spp_attest::android_service_tools::{
    ANDROID_APPRAISAL_STAGING_DIR, ANDROID_CA_BUNDLE_FILENAME, ANDROID_NATIVE_RECEIPT_INPUTS,
    ANDROID_NVAT_FILENAME, ANDROID_NVATTEST_FILENAME, ANDROID_RCLONE_FILENAME,
    ANDROID_RESTIC_FILENAME, AndroidServiceToolRole, AndroidServiceToolsRefusal,
    admitted_android_verifier, create_android_appraisal_staging,
    has_admitted_android_service_tools, install_android_service_tools, read_test_ca_bundle_pem,
    render_android_service_tools_manifest, resolve_admitted_android_service_tool,
    revalidate_installed_android_service_tools,
};
use solstone_core_spp_attest::error::GpuAppraisalReason;
use solstone_core_spp_attest::nvgpu::{
    GpuProfile, ManifestSet, StatusMode, write_production_appraisal_staging,
};
use solstone_core_spp_attest::tlv::{GpuEnvelope, GpuField};
use solstone_core_spp_attest::{
    NvattestCommand, NvattestInstallation, authorize_nvattest_spawn,
    build_nvattest_attest_command_with_installation,
    build_nvattest_offline_attest_command_with_installation,
};
use tempfile::tempdir;

struct NoOpRunner;

impl ToolRunner for NoOpRunner {
    fn run(
        &self,
        _: &solstone_core_backup_runtime::runner::ToolRequest<'_>,
    ) -> std::io::Result<solstone_core_backup_runtime::runner::ToolOutput> {
        panic!("resolve must not invoke runner")
    }
}

struct FixedClock;

impl Clock for FixedClock {
    fn now_unix(&self) -> i64 {
        1_700_000_000
    }
    fn iso_week(&self) -> u8 {
        1
    }
}

fn configured_journal() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let destination = solstone_core_backup::Destination {
        repository: "s3:repo".into(),
        backend: "s3".into(),
        credentials: serde_json::json!({"access_key_id":"access","secret_access_key":"secret"})
            .as_object()
            .unwrap()
            .clone(),
    };
    solstone_core_backup::set_destination(dir.path(), &destination).unwrap();
    solstone_core_backup::generate_and_store_keys(dir.path()).unwrap();
    solstone_core_backup::set_enabled(dir.path(), true).unwrap();
    dir
}

const MOBILE_NATIVE_DRIVER: &[u8] = include_bytes!("../../../../scripts/mobile_native_build.py");
const MOBILE_NATIVE_INPUTS: &[u8] = include_bytes!("../../../distribution/mobile/inputs.json");
const MOBILE_NVATTEST_PATCH: &[u8] =
    include_bytes!("../../../distribution/mobile/nvattest-mobile.patch");

fn setup_fixture_roots(native_dir: &Path, resource_dir: &Path) {
    let restic_bytes = b"\x7fELFdummy-restic-pie";
    let rclone_bytes = b"\x7fELFdummy-rclone-pie";
    let nvattest_bytes = solstone_core_distribution::elf::fixture_shared_needed(
        solstone_core_distribution::elf::machine_aarch64(),
        &["libnvat.so", "libm.so", "libdl.so", "libc.so"],
        None,
        None,
    );
    let nvat_bytes = solstone_core_distribution::elf::fixture_shared_needed(
        solstone_core_distribution::elf::machine_aarch64(),
        &["libz.so", "liblog.so", "libdl.so", "libm.so", "libc.so"],
        Some(ANDROID_NVAT_FILENAME),
        None,
    );
    fs::write(native_dir.join(ANDROID_RESTIC_FILENAME), restic_bytes).unwrap();
    fs::write(native_dir.join(ANDROID_RCLONE_FILENAME), rclone_bytes).unwrap();
    fs::write(native_dir.join(ANDROID_NVATTEST_FILENAME), nvattest_bytes).unwrap();
    fs::write(native_dir.join(ANDROID_NVAT_FILENAME), nvat_bytes).unwrap();

    let ca_bytes = read_test_ca_bundle_pem().unwrap();
    fs::write(resource_dir.join(ANDROID_CA_BUNDLE_FILENAME), ca_bytes).unwrap();

    for rim in ManifestSet::QUALIFIED_595_71_05.entries() {
        fs::write(resource_dir.join(rim.filename()), rim.xml()).unwrap();
    }
}

fn current_native_inputs() -> serde_json::Value {
    let mut inputs = serde_json::Map::new();
    for name in ANDROID_NATIVE_RECEIPT_INPUTS {
        let pin = solstone_core_distribution::mobile_input_pin(name);
        let revision = if *name == "nvattest" {
            solstone_core_distribution::NVATTEST_SDK_REVISION
        } else {
            pin.revision
        };
        inputs.insert(
            (*name).to_owned(),
            serde_json::json!({
                "revision": revision,
                "sha256": pin.archive_sha256,
            }),
        );
    }
    serde_json::Value::Object(inputs)
}

fn current_native_patches() -> serde_json::Value {
    let sha = solstone_core_distribution::digest::sha256_hex;
    let regorus = solstone_core_distribution::regorus_build_revision_pin();
    serde_json::json!([
        {"component": "nvattest", "sha256": sha(MOBILE_NVATTEST_PATCH)},
        {
            "component": "regorus-build-revision",
            "revision": regorus.revision,
            "original_sha256": regorus.original_sha256,
            "patched_sha256": regorus.patched_sha256,
        }
    ])
}

fn receipt_for_tree(native_dir: &Path, resource_dir: &Path) -> Vec<u8> {
    let sha = solstone_core_distribution::digest::sha256_hex;
    let artifact = |component: &str, bytes: &[u8]| {
        serde_json::json!({
            "component": component,
            "path": component,
            "sha256": sha(bytes),
            "size_bytes": bytes.len(),
        })
    };
    serde_json::to_vec(&serde_json::json!({
        "platform": "android",
        "complete": true,
        "status": "passed",
        "exit": 0,
        "driver_sha256": sha(MOBILE_NATIVE_DRIVER),
        "inputs_sha256": sha(MOBILE_NATIVE_INPUTS),
        "inputs": current_native_inputs(),
        "patches": current_native_patches(),
        "artifacts": [
            artifact("restic-cli", &fs::read(native_dir.join(ANDROID_RESTIC_FILENAME)).unwrap()),
            artifact("rclone-cli", &fs::read(native_dir.join(ANDROID_RCLONE_FILENAME)).unwrap()),
            artifact("nvattest-cli", &fs::read(native_dir.join(ANDROID_NVATTEST_FILENAME)).unwrap()),
            artifact("nvattest-library", &fs::read(native_dir.join(ANDROID_NVAT_FILENAME)).unwrap()),
            artifact("nvattest-ca-bundle", &fs::read(resource_dir.join(ANDROID_CA_BUNDLE_FILENAME)).unwrap()),
        ],
    }))
    .unwrap()
}

fn appraisal_envelope() -> GpuEnvelope {
    GpuEnvelope {
        fields: vec![
            GpuField {
                field_id: 2,
                value: b"evidence-bytes".to_vec(),
            },
            GpuField {
                field_id: 3,
                value: b"certificate-bytes".to_vec(),
            },
            GpuField {
                field_id: 7,
                value: b"gh100".to_vec(),
            },
        ],
        nonce: [0x11; 32],
        spdm_nonce: [0x22; 32],
        raw: Vec::new(),
    }
}

fn appraisal_profile() -> GpuProfile {
    GpuProfile::new(
        "84edaf3d0205a8280068ab485bf45edfc81f371ab7a7dcccaef8538728ccd8a3",
        ManifestSet::QUALIFIED_595_71_05,
        StatusMode::OfflineSignedAge,
    )
}

fn write_staging(
    proof: &[u8],
) -> Result<solstone_core_spp_attest::nvgpu::ProductionAppraisalStaging, GpuAppraisalReason> {
    write_production_appraisal_staging(
        &appraisal_envelope(),
        &[0x11; 32],
        &appraisal_profile(),
        proof,
    )
}

#[cfg(unix)]
fn permission_bits(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    fs::symlink_metadata(path).unwrap().permissions().mode() & 0o777
}

fn assert_desktop_staging_skips_resource(resource_root: &Path) {
    let marker = resource_root.join(ANDROID_APPRAISAL_STAGING_DIR);
    assert!(!marker.exists());
    let proof = b"desktop-proof";
    let staged = write_staging(proof).unwrap();
    let temp = std::env::temp_dir();
    assert_eq!(
        staged.rim_dir().parent().unwrap().as_os_str(),
        temp.as_os_str()
    );
    assert_eq!(
        staged.evidence().parent().unwrap().as_os_str(),
        temp.as_os_str()
    );
    assert_eq!(
        staged.proof().parent().unwrap().as_os_str(),
        temp.as_os_str()
    );
    assert_eq!(fs::read(staged.proof()).unwrap(), proof);
    assert!(!marker.exists());
}

fn exercise_private_appraisal_staging(resource_root: &Path) {
    let staging = create_android_appraisal_staging().unwrap();
    assert_eq!(
        staging.as_os_str(),
        resource_root
            .join(ANDROID_APPRAISAL_STAGING_DIR)
            .as_os_str()
    );
    assert_eq!(
        staging.parent().unwrap().as_os_str(),
        resource_root.as_os_str()
    );
    #[cfg(unix)]
    assert_eq!(permission_bits(&staging), 0o700);

    let proof = b"offline-proof";
    let staged = write_staging(proof).unwrap();
    assert_eq!(
        staged.rim_dir().parent().unwrap().as_os_str(),
        staging.as_os_str()
    );
    assert_eq!(
        staged.evidence().parent().unwrap().as_os_str(),
        staging.as_os_str()
    );
    assert_eq!(
        staged.proof().parent().unwrap().as_os_str(),
        staging.as_os_str()
    );
    assert!(
        staged
            .rim_dir()
            .join(ManifestSet::QUALIFIED_595_71_05.entries()[0].filename())
            .is_file()
    );
    assert_eq!(fs::read(staged.proof()).unwrap(), proof);
    let evidence = fs::read(staged.evidence()).unwrap();
    assert!(evidence.windows(64).any(|window| window == [b'1'; 64]));
    #[cfg(unix)]
    {
        assert_eq!(permission_bits(staged.rim_dir()), 0o700);
        assert_eq!(permission_bits(staged.evidence()), 0o600);
        assert_eq!(permission_bits(staged.proof()), 0o600);
    }
    let rim = staged.rim_dir().to_path_buf();
    let evidence_path = staged.evidence().to_path_buf();
    let proof_path = staged.proof().to_path_buf();
    drop(staged);
    assert!(!rim.exists());
    assert!(!evidence_path.exists());
    assert!(!proof_path.exists());
    assert!(staging.is_dir());
    assert!(fs::read_dir(&staging).unwrap().next().is_none());

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::symlink_metadata(&staging).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&staging, perms).unwrap();
        assert_eq!(
            create_android_appraisal_staging().unwrap_err().code(),
            "unsafe-path"
        );
        assert_staging_integrity_failed(proof);
        assert!(fs::read_dir(&staging).unwrap().next().is_none());

        fs::remove_dir(&staging).unwrap();
        let foreign = tempdir().unwrap();
        std::os::unix::fs::symlink(foreign.path(), &staging).unwrap();
        assert_eq!(
            create_android_appraisal_staging().unwrap_err().code(),
            "unsafe-path"
        );
        assert_staging_integrity_failed(proof);
        assert!(fs::read_dir(foreign.path()).unwrap().next().is_none());
        fs::remove_file(&staging).unwrap();
    }
    #[cfg(not(unix))]
    {
        fs::remove_dir_all(&staging).unwrap();
    }
    assert!(!resource_root.join(ANDROID_APPRAISAL_STAGING_DIR).exists());
}

fn assert_staging_integrity_failed(proof: &[u8]) {
    match write_staging(proof) {
        Err(GpuAppraisalReason::NvattestIntegrityFailed) => {}
        Err(other) => panic!("staging refusal {other:?}"),
        Ok(staged) => {
            panic!("staging wrote {}", staged.rim_dir().display())
        }
    }
}

fn refuse_changed_appraisal_staging(code: &str) {
    assert_eq!(create_android_appraisal_staging().unwrap_err().code(), code);
    assert_staging_integrity_failed(b"must-not-land");
}

fn refuse_verifier(installation: &NvattestInstallation, evidence: &Path) {
    assert_eq!(
        build_nvattest_attest_command_with_installation(
            installation,
            evidence,
            &[0x11; 32],
            "remote",
            None,
        )
        .unwrap_err(),
        GpuAppraisalReason::NvattestIntegrityFailed
    );
}

fn exercise_verifier_construction(foreign_dir: &Path, resource_dir: &Path) -> NvattestCommand {
    let admitted = admitted_android_verifier().unwrap();
    let installation = NvattestInstallation {
        binary: admitted.binary.clone(),
        lib_dir: admitted.lib_dir.clone(),
        ca_bundle: admitted.ca_bundle.clone(),
    };
    let evidence = resource_dir.join("evidence.json");
    fs::write(&evidence, b"{}").unwrap();
    let command = build_nvattest_attest_command_with_installation(
        &installation,
        &evidence,
        &[0x11; 32],
        "remote",
        None,
    )
    .unwrap();
    authorize_nvattest_spawn(&command).unwrap();

    let offline = build_nvattest_offline_attest_command_with_installation(
        &installation,
        &evidence,
        &[0x11; 32],
        resource_dir,
        &evidence,
        1_700_000_000,
    )
    .unwrap();
    assert!(
        offline
            .argv
            .iter()
            .all(|argument| { argument.as_os_str() != std::ffi::OsStr::new("--ca-bundle") })
    );
    authorize_nvattest_spawn(&offline).unwrap();

    let mut forged_binary = installation.clone();
    forged_binary.binary = foreign_dir.join(ANDROID_NVATTEST_FILENAME);
    refuse_verifier(&forged_binary, &evidence);

    let mut dotted_binary = installation.clone();
    dotted_binary.binary = installation
        .binary
        .parent()
        .unwrap()
        .join(".")
        .join(ANDROID_NVATTEST_FILENAME);
    assert_ne!(
        dotted_binary.binary.as_os_str(),
        installation.binary.as_os_str()
    );
    refuse_verifier(&dotted_binary, &evidence);

    let mut forged_lib = installation.clone();
    forged_lib.lib_dir = foreign_dir.to_path_buf();
    refuse_verifier(&forged_lib, &evidence);

    let mut dotted_lib = installation.clone();
    dotted_lib.lib_dir = installation.lib_dir.join(".");
    assert_ne!(
        dotted_lib.lib_dir.as_os_str(),
        installation.lib_dir.as_os_str()
    );
    refuse_verifier(&dotted_lib, &evidence);
    let mut slashed_lib = installation.clone();
    slashed_lib.lib_dir = std::path::PathBuf::from(format!("{}/", installation.lib_dir.display()));
    assert_ne!(
        slashed_lib.lib_dir.as_os_str(),
        installation.lib_dir.as_os_str()
    );
    refuse_verifier(&slashed_lib, &evidence);

    let mut forged_ca = installation.clone();
    forged_ca.ca_bundle = foreign_dir.join(ANDROID_CA_BUNDLE_FILENAME);
    refuse_verifier(&forged_ca, &evidence);
    assert_eq!(
        build_nvattest_offline_attest_command_with_installation(
            &forged_ca,
            &evidence,
            &[0x11; 32],
            resource_dir,
            &evidence,
            1_700_000_000,
        )
        .unwrap_err(),
        GpuAppraisalReason::NvattestIntegrityFailed
    );

    let mut dotted_ca = installation.clone();
    dotted_ca.ca_bundle = installation
        .ca_bundle
        .parent()
        .unwrap()
        .join(".")
        .join(ANDROID_CA_BUNDLE_FILENAME);
    assert_ne!(
        dotted_ca.ca_bundle.as_os_str(),
        installation.ca_bundle.as_os_str()
    );
    refuse_verifier(&dotted_ca, &evidence);

    let mut bad_exec = command.clone();
    bad_exec.executable = foreign_dir.join(ANDROID_NVATTEST_FILENAME);
    assert_eq!(
        authorize_nvattest_spawn(&bad_exec).unwrap_err(),
        GpuAppraisalReason::NvattestIntegrityFailed
    );
    let mut bad_lib = command.clone();
    bad_lib.lib_dir = foreign_dir.to_path_buf();
    assert_eq!(
        authorize_nvattest_spawn(&bad_lib).unwrap_err(),
        GpuAppraisalReason::NvattestIntegrityFailed
    );
    let mut bad_ca = command.clone();
    bad_ca.ca_bundle = foreign_dir.join(ANDROID_CA_BUNDLE_FILENAME);
    assert_eq!(
        authorize_nvattest_spawn(&bad_ca).unwrap_err(),
        GpuAppraisalReason::NvattestIntegrityFailed
    );

    let original = fs::read(&installation.binary).unwrap();
    fs::write(&installation.binary, b"replaced-nvattest-member").unwrap();
    refuse_verifier(&installation, &evidence);
    assert_eq!(
        authorize_nvattest_spawn(&command).unwrap_err(),
        GpuAppraisalReason::NvattestIntegrityFailed
    );
    fs::write(&installation.binary, original).unwrap();
    build_nvattest_attest_command_with_installation(
        &installation,
        &evidence,
        &[0x11; 32],
        "remote",
        None,
    )
    .unwrap();
    command
}

#[test]
#[allow(unsafe_code)]
fn android_service_tools_process_lifecycle_and_env_isolation() {
    let dummy_exe = Path::new("/system/bin/app_process64");
    let journal = configured_journal();
    let capability = prepare(journal.path(), &FixedClock).unwrap();

    // 1. Pre-installation state:
    assert!(!has_admitted_android_service_tools());
    assert_eq!(
        create_android_appraisal_staging().unwrap_err().code(),
        "member-missing"
    );
    assert!(resolve_admitted_android_service_tool(AndroidServiceToolRole::Restic).is_err());
    assert!(resolve_admitted_android_service_tool(AndroidServiceToolRole::Rclone).is_err());
    assert!(revalidate_installed_android_service_tools().is_err());

    let err = resolve_tools_for_platform(
        &capability,
        &NoOpRunner,
        dummy_exe,
        ToolPlatform::AndroidAarch64,
        &LiveAndroidAuthority,
    )
    .unwrap_err();
    assert_eq!(err.detail(), "member-missing");

    let err_restic = resolve_restic_member_for_platform(
        dummy_exe,
        ToolPlatform::AndroidAarch64,
        &LiveAndroidAuthority,
    )
    .unwrap_err();
    assert_eq!(err_restic.detail(), "member-missing");

    let home = tempdir().unwrap();
    unsafe {
        std::env::set_var("HOME", home.path());
        std::env::set_var("PATH", "/injected/bin:/usr/bin");
        std::env::set_var("LD_LIBRARY_PATH", "/injected/lib");
        std::env::set_var("SOLSTONE_RESTIC_BUNDLE", "/injected/bundle");
        std::env::set_var("SOLSTONE_RCLONE_BUNDLE", "/injected/bundle");
    }

    // 2. Prepare valid fixture roots
    let native_temp = tempdir().unwrap();
    let resource_temp = tempdir().unwrap();
    setup_fixture_roots(native_temp.path(), resource_temp.path());
    assert_desktop_staging_skips_resource(resource_temp.path());

    let restic_full = native_temp.path().join(ANDROID_RESTIC_FILENAME);
    let rclone_full = native_temp.path().join(ANDROID_RCLONE_FILENAME);

    let receipt = receipt_for_tree(native_temp.path(), resource_temp.path());
    let manifest_bytes =
        render_android_service_tools_manifest(native_temp.path(), resource_temp.path(), &receipt)
            .unwrap();

    let mut stamped = serde_json::from_slice::<serde_json::Value>(&receipt).unwrap();
    for artifact in stamped["artifacts"].as_array_mut().unwrap() {
        if artifact["component"] == "restic-cli" {
            artifact["sha256"] = serde_json::Value::String("0".repeat(64));
        }
    }
    let stamp_err = install_android_service_tools(
        native_temp.path(),
        resource_temp.path(),
        &manifest_bytes,
        &serde_json::to_vec(&stamped).unwrap(),
    )
    .unwrap_err();
    assert_eq!(stamp_err.code(), "pin-mismatch");
    assert!(!has_admitted_android_service_tools());

    // 3. Install into OnceLock
    install_android_service_tools(
        native_temp.path(),
        resource_temp.path(),
        &manifest_bytes,
        &receipt,
    )
    .unwrap();

    // Idempotent reinstall: identical roots and manifest bytes must succeed
    install_android_service_tools(
        native_temp.path(),
        resource_temp.path(),
        &manifest_bytes,
        &receipt,
    )
    .unwrap();

    // A receipt that does not match the packaged bytes fails admission and does
    // not replace the authority.
    let stamp_after = install_android_service_tools(
        native_temp.path(),
        resource_temp.path(),
        &manifest_bytes,
        &serde_json::to_vec(&stamped).unwrap(),
    )
    .unwrap_err();
    assert_eq!(stamp_after.code(), "pin-mismatch");

    // Rejection on difference: different root or manifest bytes returns AlreadyInstalled
    let other_temp = tempdir().unwrap();
    setup_fixture_roots(other_temp.path(), resource_temp.path());
    let other_receipt = receipt_for_tree(other_temp.path(), resource_temp.path());
    let diff_err = install_android_service_tools(
        other_temp.path(),
        resource_temp.path(),
        &manifest_bytes,
        &other_receipt,
    )
    .unwrap_err();
    assert!(matches!(
        diff_err,
        AndroidServiceToolsRefusal::AlreadyInstalled
    ));
    assert_eq!(diff_err.code(), "already-installed");

    // 4. Post-installation queries:
    assert!(has_admitted_android_service_tools());
    assert_eq!(
        resolve_admitted_android_service_tool(AndroidServiceToolRole::Restic).unwrap(),
        restic_full
    );
    assert_eq!(
        resolve_admitted_android_service_tool(AndroidServiceToolRole::Rclone).unwrap(),
        rclone_full
    );
    assert!(revalidate_installed_android_service_tools().is_ok());

    // 5. Environmental isolation: planted environment must not alter resolution
    let resolved = resolve_tools_for_platform(
        &capability,
        &NoOpRunner,
        dummy_exe,
        ToolPlatform::AndroidAarch64,
        &LiveAndroidAuthority,
    )
    .unwrap();
    assert_eq!(
        resolved.restic.executable_path().unwrap().to_path_buf(),
        restic_full
    );
    assert_eq!(resolved.rclone_path, None);

    let resolved_restic = resolve_restic_member_for_platform(
        dummy_exe,
        ToolPlatform::AndroidAarch64,
        &LiveAndroidAuthority,
    )
    .unwrap();
    assert_eq!(resolved_restic, restic_full);

    let non_operated = resolve_operational_tools_for_platform(
        &NoOpRunner,
        journal.path(),
        true,
        dummy_exe,
        ToolPlatform::AndroidAarch64,
        &LiveAndroidAuthority,
    )
    .unwrap();
    assert_eq!(
        non_operated.restic.executable_path().unwrap().to_path_buf(),
        restic_full
    );
    assert_eq!(non_operated.rclone_path, None);

    // Operated mode needs rclone:
    solstone_core_backup::save_hosted_binding(
        journal.path(),
        &solstone_core_backup::HostedBinding {
            broker_endpoint: "https://backup.example".into(),
            account_id: "account".into(),
            instance_id: "instance".into(),
            bucket: "bucket".into(),
            prefix: "prefix".into(),
            broker_token: "token".into(),
        },
    )
    .unwrap();
    solstone_core_backup::set_mode(journal.path(), "operated").unwrap();
    let operated = prepare(journal.path(), &FixedClock).unwrap();
    let resolved_operated = resolve_tools_for_platform(
        &operated,
        &NoOpRunner,
        dummy_exe,
        ToolPlatform::AndroidAarch64,
        &LiveAndroidAuthority,
    )
    .unwrap();
    assert_eq!(
        resolved_operated
            .restic
            .executable_path()
            .unwrap()
            .to_path_buf(),
        restic_full
    );
    assert_eq!(resolved_operated.rclone_path, Some(rclone_full.clone()));

    let resolved_op = resolve_operational_tools_for_platform(
        &NoOpRunner,
        journal.path(),
        true,
        dummy_exe,
        ToolPlatform::AndroidAarch64,
        &LiveAndroidAuthority,
    )
    .unwrap();
    assert_eq!(
        resolved_op.restic.executable_path().unwrap().to_path_buf(),
        restic_full
    );
    assert_eq!(resolved_op.rclone_path, Some(rclone_full));

    let verifier_command = exercise_verifier_construction(other_temp.path(), resource_temp.path());
    exercise_private_appraisal_staging(resource_temp.path());

    // 6. Tamper check: modifying restic binary on disk triggers member-changed refusal
    fs::write(&restic_full, b"tampered-bytes-after-admission").unwrap();

    let reval_err = revalidate_installed_android_service_tools().unwrap_err();
    assert!(matches!(
        reval_err,
        AndroidServiceToolsRefusal::MemberChanged { .. }
    ));
    assert_eq!(reval_err.code(), "member-changed");

    let tampered_res = resolve_tools_for_platform(
        &capability,
        &NoOpRunner,
        dummy_exe,
        ToolPlatform::AndroidAarch64,
        &LiveAndroidAuthority,
    )
    .unwrap_err();
    assert_eq!(tampered_res.detail(), "member-changed");

    let lookup_err =
        resolve_admitted_android_service_tool(AndroidServiceToolRole::Restic).unwrap_err();
    assert_eq!(lookup_err.code(), "member-changed");
    refuse_changed_appraisal_staging("member-changed");
    assert!(
        !resource_temp
            .path()
            .join(ANDROID_APPRAISAL_STAGING_DIR)
            .exists()
    );

    // A new directory at the admitted path is a different root, even when the
    // path strings on an already built command still match.
    let native_path = native_temp.path().to_path_buf();
    let relocated = native_path.with_file_name(format!(
        "{}-relocated",
        native_path.file_name().unwrap().to_string_lossy()
    ));
    fs::rename(&native_path, &relocated).unwrap();
    fs::create_dir(&native_path).unwrap();
    let root_err = revalidate_installed_android_service_tools().unwrap_err();
    assert_eq!(root_err.code(), "root-substituted");
    refuse_changed_appraisal_staging("root-substituted");
    assert!(
        !resource_temp
            .path()
            .join(ANDROID_APPRAISAL_STAGING_DIR)
            .exists()
    );
    let substituted = NvattestInstallation {
        binary: native_path.join(ANDROID_NVATTEST_FILENAME),
        lib_dir: native_path.clone(),
        ca_bundle: resource_temp.path().join(ANDROID_CA_BUNDLE_FILENAME),
    };
    assert_eq!(
        build_nvattest_attest_command_with_installation(
            &substituted,
            &resource_temp.path().join("evidence.json"),
            &[0x11; 32],
            "remote",
            None,
        )
        .unwrap_err(),
        GpuAppraisalReason::NvattestIntegrityFailed
    );
    assert_eq!(
        authorize_nvattest_spawn(&verifier_command).unwrap_err(),
        GpuAppraisalReason::NvattestIntegrityFailed
    );
    let _ = fs::remove_dir_all(&relocated);
}
