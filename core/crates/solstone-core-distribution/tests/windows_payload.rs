// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::io::Cursor;

use minisign::KeyPair;
use solstone_core_distribution::manifest_verify::install_test_fixture_pin;
use solstone_core_distribution::windows_payload::{
    WINDOWS_CED_LIBRARY, WINDOWS_PAYLOAD_MANIFEST, WINDOWS_PAYLOAD_SIGNATURE,
    render_windows_payload_manifest, verify_windows_payload,
};

const COMMIT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const LOCK: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

fn fixture() -> tempfile::TempDir {
    let root = tempfile::tempdir().expect("temporary payload root");
    fs::create_dir_all(root.path().join("bin")).expect("bin");
    fs::create_dir_all(root.path().join("lib/solstone-core-pdf")).expect("lib");
    fs::write(root.path().join("bin/ced.dll"), b"ced dll").expect("ced");
    fs::write(
        root.path().join("lib/solstone-core-pdf/pdfium.dll"),
        b"pdfium dll",
    )
    .expect("pdfium");
    let manifest = render_windows_payload_manifest(root.path(), COMMIT, LOCK).expect("manifest");
    let KeyPair { pk, sk } = KeyPair::generate_unencrypted_keypair().expect("key pair");
    let pin = root.path().join("payload.pub");
    fs::write(&pin, pk.to_box().expect("public box").to_bytes()).expect("pin");
    install_test_fixture_pin(&pin).expect("fixture pin");
    fs::remove_file(&pin).expect("remove fixture pin from payload");
    let signature = minisign::sign(
        Some(&pk),
        &sk,
        Cursor::new(manifest.as_slice()),
        None,
        Some("fixture payload manifest"),
    )
    .expect("signature");
    let manifest_path = root.path().join(WINDOWS_PAYLOAD_MANIFEST);
    fs::create_dir_all(manifest_path.parent().expect("manifest parent")).expect("provenance");
    fs::write(&manifest_path, manifest).expect("write manifest");
    fs::write(
        root.path().join(WINDOWS_PAYLOAD_SIGNATURE),
        signature.into_string(),
    )
    .expect("write signature");
    root
}

#[test]
fn signed_windows_payload_is_complete_and_refuses_mutation() {
    let root = fixture();
    let verified = verify_windows_payload(root.path()).expect("valid payload");
    assert_eq!(verified.manifest().source_commit, COMMIT);
    assert_eq!(
        verified
            .declared_path("bin/ced.dll")
            .expect("declared CED path"),
        root.path().join(WINDOWS_CED_LIBRARY)
    );
    assert_eq!(
        verified.ced_library_path().expect("declared CED engine"),
        root.path().join(WINDOWS_CED_LIBRARY)
    );
    assert!(verified.declared_path("bin/not-admitted.dll").is_none());

    fs::write(root.path().join("bin/ced.dll"), b"changed").expect("change CED");
    assert!(
        verify_windows_payload(root.path())
            .expect_err("changed payload")
            .to_string()
            .contains("digest")
    );
    fs::write(root.path().join("bin/ced.dll"), b"ced dll").expect("restore CED");

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
#[ignore = "source-origin marker for the native Windows gate"]
fn journal_win_ci_windows_payload_marker() {
    println!("JOURNAL_WIN_CI_TARGET_WINDOWS_PAYLOAD=executed/pass");
}
