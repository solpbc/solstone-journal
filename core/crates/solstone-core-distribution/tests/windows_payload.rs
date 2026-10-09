// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::io::Cursor;

use minisign::KeyPair;
use solstone_core_distribution::manifest_verify::install_test_fixture_pin;
use solstone_core_distribution::windows_payload::{
    WINDOWS_CED_LIBRARY, WINDOWS_ONNXRUNTIME_LIBRARY, WINDOWS_PARAKEET_MODEL,
    WINDOWS_PARAKEET_SERVER, WINDOWS_PAYLOAD_MANIFEST, WINDOWS_PAYLOAD_SIGNATURE,
    WINDOWS_PDFIUM_LIBRARY, WINDOWS_PDFIUM_WORKER, WINDOWS_PYANNOTE_MODEL,
    WINDOWS_SILERO_VAD_MODEL, WINDOWS_SPEAKERS_ANALYZE_WORKER, WINDOWS_VAD_ANALYZE_WORKER,
    WINDOWS_WESPEAKER_MODEL, render_windows_payload_manifest, verify_windows_payload,
};

const COMMIT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const LOCK: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

fn fixture() -> tempfile::TempDir {
    let root = tempfile::tempdir().expect("temporary payload root");
    fs::create_dir_all(root.path().join("bin")).expect("bin");
    fs::create_dir_all(root.path().join("lib/solstone-core-pdf")).expect("lib");
    fs::create_dir_all(root.path().join("lib/solstone-core-speakers-analyze")).expect("onnx lib");
    fs::create_dir_all(root.path().join("lib/solstone_journal_models/assets")).expect("model lib");
    fs::create_dir_all(
        root.path()
            .join("lib/solstone_journal_models/assets/parakeet"),
    )
    .expect("Parakeet model lib");
    fs::create_dir_all(
        root.path()
            .join("lib/solstone_journal_models/assets/rfdetr"),
    )
    .expect("rfdetr model lib");
    fs::write(root.path().join("bin/ced.dll"), b"ced dll").expect("ced");
    fs::write(root.path().join("bin/rfdetr-cli.exe"), b"rfdetr cli").expect("rfdetr cli");
    fs::write(
        root.path()
            .join("lib/solstone_journal_models/assets/rfdetr/rfdetr-nano-f16.gguf"),
        b"rfdetr model",
    )
    .expect("rfdetr model");
    fs::write(root.path().join(WINDOWS_PDFIUM_WORKER), b"pdf worker").expect("pdf worker");
    fs::write(
        root.path().join("lib/solstone-core-pdf/pdfium.dll"),
        b"pdfium dll",
    )
    .expect("pdfium");
    fs::write(
        root.path().join(WINDOWS_SPEAKERS_ANALYZE_WORKER),
        b"speaker worker",
    )
    .expect("speaker worker");
    fs::write(root.path().join(WINDOWS_VAD_ANALYZE_WORKER), b"vad worker").expect("vad worker");
    fs::write(
        root.path().join(WINDOWS_ONNXRUNTIME_LIBRARY),
        b"onnx runtime",
    )
    .expect("onnx runtime");
    fs::write(
        root.path().join(WINDOWS_WESPEAKER_MODEL),
        b"wespeaker model",
    )
    .expect("wespeaker model");
    fs::write(root.path().join(WINDOWS_PYANNOTE_MODEL), b"pyannote model").expect("pyannote model");
    fs::write(root.path().join(WINDOWS_SILERO_VAD_MODEL), b"silero model").expect("silero model");
    fs::write(
        root.path().join(WINDOWS_PARAKEET_SERVER),
        b"Parakeet server",
    )
    .expect("Parakeet server");
    fs::write(root.path().join(WINDOWS_PARAKEET_MODEL), b"Parakeet model").expect("Parakeet model");
    sign_payload_manifest(root.path());
    root
}

fn keypair() -> &'static KeyPair {
    static KEYPAIR: std::sync::OnceLock<KeyPair> = std::sync::OnceLock::new();
    KEYPAIR.get_or_init(|| KeyPair::generate_unencrypted_keypair().expect("key pair"))
}

fn sign_payload_manifest(root: &std::path::Path) {
    let manifest_path = root.join(WINDOWS_PAYLOAD_MANIFEST);
    let sig_path = root.join(WINDOWS_PAYLOAD_SIGNATURE);
    let _ = fs::remove_file(&manifest_path);
    let _ = fs::remove_file(&sig_path);

    let manifest = render_windows_payload_manifest(root, COMMIT, LOCK).expect("manifest");
    let kp = keypair();
    let pin = root.join("payload.pub");
    fs::write(&pin, kp.pk.to_box().expect("public box").to_bytes()).expect("pin");
    install_test_fixture_pin(&pin).expect("fixture pin");
    fs::remove_file(&pin).expect("remove fixture pin from payload");
    let signature = minisign::sign(
        Some(&kp.pk),
        &kp.sk,
        Cursor::new(manifest.as_slice()),
        None,
        Some("fixture payload manifest"),
    )
    .expect("signature");
    fs::create_dir_all(manifest_path.parent().expect("manifest parent")).expect("provenance");
    fs::write(&manifest_path, manifest).expect("write manifest");
    fs::write(sig_path, signature.into_string()).expect("write signature");
}

#[test]
fn signed_windows_payload_is_complete_and_refuses_mutation() {
    let root = fixture();
    let verified = verify_windows_payload(root.path()).expect("valid payload");
    assert_eq!(verified.manifest().source_commit, COMMIT);
    #[cfg(windows)]
    {
        let verbatim_root = root.path().canonicalize().expect("canonical root");
        let ordinary_root = std::path::Path::new(
            verbatim_root
                .to_str()
                .unwrap()
                .strip_prefix(r"\\?\")
                .unwrap(),
        );
        assert_ne!(ordinary_root.as_os_str(), verbatim_root.as_os_str());
        let ordinary = verify_windows_payload(ordinary_root).expect("ordinary payload");
        let verbatim = verify_windows_payload(&verbatim_root).expect("verbatim payload");
        for member in [
            WINDOWS_SPEAKERS_ANALYZE_WORKER,
            WINDOWS_ONNXRUNTIME_LIBRARY,
            WINDOWS_WESPEAKER_MODEL,
        ] {
            let ordinary_path = ordinary.declared_path(member).unwrap();
            let verbatim_path = verbatim.declared_path(member).unwrap();
            assert_ne!(ordinary_path.as_os_str(), verbatim_path.as_os_str());
            assert!(ordinary_path.is_file(), "ordinary inventory join {member}");
            assert!(verbatim_path.is_file(), "verbatim inventory join {member}");
            assert_eq!(
                ordinary_path.canonicalize().unwrap(),
                verbatim_path.canonicalize().unwrap()
            );
        }
    }

    assert_eq!(
        verified
            .declared_path("bin/ced.dll")
            .expect("declared CED path"),
        root.path().join(WINDOWS_CED_LIBRARY)
    );
    assert_eq!(
        verified
            .declared_path("bin/rfdetr-cli.exe")
            .expect("declared RF-DETR worker"),
        root.path().join("bin/rfdetr-cli.exe")
    );
    assert_eq!(
        verified
            .declared_path("lib/solstone_journal_models/assets/rfdetr/rfdetr-nano-f16.gguf")
            .expect("declared RF-DETR model"),
        root.path()
            .join("lib/solstone_journal_models/assets/rfdetr/rfdetr-nano-f16.gguf")
    );
    assert_eq!(
        verified.ced_library_path().expect("declared CED engine"),
        root.path().join(WINDOWS_CED_LIBRARY)
    );
    assert_eq!(
        verified
            .pdfium_library_path()
            .expect("declared PDFium engine"),
        root.path().join(WINDOWS_PDFIUM_LIBRARY)
    );
    assert_eq!(
        verified
            .pdfium_worker_path()
            .expect("declared PDFium worker"),
        root.path().join(WINDOWS_PDFIUM_WORKER)
    );
    assert_eq!(
        verified
            .speakers_analyze_worker_path()
            .expect("declared speaker worker"),
        root.path().join(WINDOWS_SPEAKERS_ANALYZE_WORKER)
    );
    assert_eq!(
        verified
            .vad_analyze_worker_path()
            .expect("declared VAD worker"),
        root.path().join(WINDOWS_VAD_ANALYZE_WORKER)
    );
    assert_eq!(
        verified
            .onnxruntime_library_path()
            .expect("declared ONNX Runtime"),
        root.path().join(WINDOWS_ONNXRUNTIME_LIBRARY)
    );
    assert_eq!(
        verified
            .wespeaker_model_path()
            .expect("declared wespeaker model"),
        root.path().join(WINDOWS_WESPEAKER_MODEL)
    );
    assert_eq!(
        verified
            .pyannote_model_path()
            .expect("declared pyannote model"),
        root.path().join(WINDOWS_PYANNOTE_MODEL)
    );
    assert_eq!(
        verified
            .silero_vad_model_path()
            .expect("declared VAD model"),
        root.path().join(WINDOWS_SILERO_VAD_MODEL)
    );
    assert_eq!(
        verified
            .parakeet_server_path()
            .expect("declared Parakeet server"),
        root.path().join(WINDOWS_PARAKEET_SERVER)
    );
    assert_eq!(
        verified
            .parakeet_model_path()
            .expect("declared Parakeet model"),
        root.path().join(WINDOWS_PARAKEET_MODEL)
    );
    assert!(verified.declared_path("bin/not-admitted.dll").is_err());

    fs::write(root.path().join("bin/ced.dll"), b"changed").expect("change CED");
    assert!(
        verify_windows_payload(root.path())
            .expect_err("changed payload")
            .to_string()
            .contains("digest")
    );
    fs::write(root.path().join("bin/ced.dll"), b"ced dll").expect("restore CED");

    // Admission hashes every DLL; any other member is hashed when asked for.
    fs::write(root.path().join(WINDOWS_PARAKEET_MODEL), b"Parakeet mode!").expect("change model");
    let admitted = verify_windows_payload(root.path()).expect("same-size model change admitted");
    assert!(
        admitted
            .parakeet_model_path()
            .expect_err("changed model")
            .to_string()
            .contains("digest")
    );
    admitted
        .silero_vad_model_path()
        .expect("an unchanged member is still served");
    fs::write(root.path().join(WINDOWS_PARAKEET_MODEL), b"Parakeet model!").expect("resize");
    assert!(
        verify_windows_payload(root.path())
            .expect_err("resized model")
            .to_string()
            .contains("bytes")
    );
    fs::write(root.path().join(WINDOWS_PARAKEET_MODEL), b"Parakeet model").expect("restore model");

    fs::write(root.path().join("unexpected.dll"), b"unexpected").expect("extra");
    assert!(
        verify_windows_payload(root.path())
            .expect_err("extra payload")
            .to_string()
            .contains("unexpected-member")
    );
    fs::remove_file(root.path().join("unexpected.dll")).expect("remove extra");

    fs::remove_file(root.path().join("lib/solstone-core-pdf/pdfium.dll")).expect("remove PDFium");
    assert!(
        verify_windows_payload(root.path())
            .expect_err("missing payload")
            .to_string()
            .contains("missing-member")
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;

        let source = root.path().join("bin/ced.dll");
        let replacement = root.path().join("ced.dll.replacement");
        fs::rename(&source, &replacement).expect("move CED");
        symlink(&replacement, &source).expect("symlink CED");
        assert!(
            verify_windows_payload(root.path())
                .expect_err("symlinked payload")
                .to_string()
                .contains("reparse-or-symlink")
        );
    }
}

#[test]
fn windows_component_evidence_error_conditions() {
    let root = fixture();
    let out = tempfile::tempdir().unwrap();

    // dirty true: error is windows-evidence-dirty-tree
    let err = solstone_core_distribution::component_evidence::render_windows_component_evidence(
        root.path(),
        COMMIT,
        true,
        None,
        out.path(),
    )
    .unwrap_err();
    assert_eq!(err.message, "windows-evidence-dirty-tree");

    // valid fixture, observed head "0".repeat(40): error starts with windows-evidence-commit-mismatch:
    let err = solstone_core_distribution::component_evidence::render_windows_component_evidence(
        root.path(),
        &"0".repeat(40),
        false,
        None,
        out.path(),
    )
    .unwrap_err();
    assert!(err.message.starts_with("windows-evidence-commit-mismatch:"));

    // valid fixture, observed head equal to COMMIT, requested_version: Some("0.0.0-not-the-workspace"): error starts with windows-evidence-version-mismatch:
    let err = solstone_core_distribution::component_evidence::render_windows_component_evidence(
        root.path(),
        COMMIT,
        false,
        Some("0.0.0-not-the-workspace"),
        out.path(),
    )
    .unwrap_err();
    assert!(
        err.message
            .starts_with("windows-evidence-version-mismatch:")
    );
}

#[test]
fn windows_component_evidence_happy_path() {
    let root = fixture();
    for comp in ["ced", "llama", "parakeet", "rfdetr", "onnx", "nvattest"] {
        let dir = root.path().join(format!("share/provenance/{comp}"));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("receipt.json"), b"{}").unwrap();
    }
    fs::write(root.path().join("bin/llama-server.exe"), b"llama server").unwrap();
    fs::write(root.path().join("bin/vulkan-1.dll"), b"vulkan loader").unwrap();
    // The CED model ships in the Windows payload as a pinned catalog member.
    let ced_model_dir = root.path().join("lib/solstone_journal_models/assets/ced");
    fs::create_dir_all(&ced_model_dir).unwrap();
    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    fs::copy(
        repo.join("core/models/assets/ced/ced-tiny-q8_0.gguf"),
        ced_model_dir.join("ced-tiny-q8_0.gguf"),
    )
    .unwrap();
    // The verifier ships its CA bundle beside it; the bundle has no receipt.
    fs::write(root.path().join("bin/nvattest.exe"), b"nvattest verifier").unwrap();
    fs::create_dir_all(root.path().join("share/ca")).unwrap();
    fs::write(root.path().join("share/ca/ca-bundle.pem"), b"ca bundle").unwrap();

    sign_payload_manifest(root.path());

    let out_dir = tempfile::tempdir().unwrap();
    solstone_core_distribution::component_evidence::render_windows_component_evidence(
        root.path(),
        COMMIT,
        false,
        None,
        out_dir.path(),
    )
    .expect("render_windows_component_evidence succeeds");

    let manifest_path = root.path().join(WINDOWS_PAYLOAD_MANIFEST);
    let manifest_bytes = fs::read(&manifest_path).unwrap();
    let expected_manifest_sha = solstone_core_distribution::digest::sha256_hex(&manifest_bytes);
    let manifest: solstone_core_installed_payload::windows_payload::WindowsPayloadManifest =
        serde_json::from_slice(&manifest_bytes).unwrap();

    let workspace_ver = env!("CARGO_PKG_VERSION");

    let prov_path = out_dir.path().join(
        solstone_core_distribution::component_evidence::provenance_file_name(
            workspace_ver,
            "windows-x86_64",
        ),
    );
    let prov_text = fs::read_to_string(&prov_path).unwrap();
    let prov_file: solstone_core_distribution::component_evidence::ProvenanceFile =
        serde_json::from_str(&prov_text).unwrap();

    assert_eq!(prov_file.basis, "producer-attested");
    assert_eq!(
        prov_file.manifests.payload_manifest_sha256,
        Some(expected_manifest_sha)
    );

    for record in &prov_file.records {
        let manifest_file = manifest
            .files
            .iter()
            .find(|f| f.path == record.path)
            .unwrap_or_else(|| panic!("record path {} not found in manifest", record.path));
        assert_eq!(record.final_sha256, manifest_file.sha256);
    }

    let comp_path = out_dir.path().join(
        solstone_core_distribution::component_evidence::components_file_name(
            workspace_ver,
            "windows-x86_64",
        ),
    );
    let comp_text = fs::read_to_string(&comp_path).unwrap();
    let comp_file: solstone_core_distribution::component_evidence::ComponentFile =
        serde_json::from_str(&comp_text).unwrap();

    assert!(!comp_file.components.iter().any(|c| c.id == "ffmpeg"));

    // The pinned CED model is named from its catalog row, not a Windows table.
    let ced_row = solstone_core_assets::catalog()
        .iter()
        .find(|a| a.unit == "ced-model" && a.filename == "ced-tiny-q8_0.gguf")
        .expect("ced-model catalog row");
    let ced_model = comp_file
        .components
        .iter()
        .find(|c| c.id == "ced-model")
        .expect("ced-model present");
    assert_eq!(ced_model.version, ced_row.version);
    assert!(
        prov_file
            .records
            .iter()
            .any(|r| r.id == "ced-model" && r.input.sha256 == ced_row.sha256)
    );

    // The CA bundle has no receipt; it is named from the verifier's
    // production pin (`nvattest_windows::production_pins().ca_bundle`).
    const CA_BUNDLE_PIN: &str = "3ff344e30b9b1ed2971044eabb438a08f2e2245ddb5f8ab1a3ad8b63ab4eaf91";
    let ca_record = prov_file
        .records
        .iter()
        .find(|r| r.id == "nvattest" && r.path == "share/ca/ca-bundle.pem")
        .expect("CA bundle provenance record");
    assert_eq!(ca_record.input.name, "ca-bundle.pem");
    assert_eq!(ca_record.input.sha256, CA_BUNDLE_PIN);
    let nvattest = comp_file
        .components
        .iter()
        .find(|c| c.id == "nvattest")
        .expect("nvattest present");
    assert!(
        nvattest
            .members
            .iter()
            .any(|m| m.path == "share/ca/ca-bundle.pem")
    );

    let llama = comp_file
        .components
        .iter()
        .find(|c| c.id == "llama-server")
        .expect("llama-server present");
    let vulkan = comp_file
        .components
        .iter()
        .find(|c| c.id == "vulkan-loader")
        .expect("vulkan-loader present");
    assert_ne!(llama.id, vulkan.id);

    let parakeet_server = comp_file
        .components
        .iter()
        .find(|c| c.id == "parakeet-server")
        .expect("parakeet-server present");
    let parakeet_model = comp_file
        .components
        .iter()
        .find(|c| c.id == "parakeet-model")
        .expect("parakeet-model present");
    assert_ne!(parakeet_server.id, parakeet_model.id);
}

#[test]
#[ignore = "source-origin marker for the native Windows gate"]
fn journal_win_ci_windows_payload_marker() {
    println!("JOURNAL_WIN_CI_TARGET_WINDOWS_PAYLOAD=executed/pass");
}
