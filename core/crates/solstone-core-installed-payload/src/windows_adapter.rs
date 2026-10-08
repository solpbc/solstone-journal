// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Windows member lookup through the signed payload verifier.
//!
//! Callers that already use `verify_windows_payload` keep that entry point,
//! including its process-wide `share_verification` scope. This adapter is the
//! member API on top of the same function. It does not admit a POSIX manifest.

use std::path::{Path, PathBuf};

use crate::windows_payload::{WindowsPayloadError, verify_windows_payload};

pub fn windows_declared_member(root: &Path, path: &str) -> Result<PathBuf, WindowsPayloadError> {
    verify_windows_payload(root)?.declared_path(path)
}

#[cfg(all(test, feature = "test-fixture-pin"))]
mod tests {
    use std::fs;
    use std::io::Cursor;

    use minisign::KeyPair;

    use super::windows_declared_member;
    use crate::pin::install_test_fixture_pin;
    use crate::windows_payload::{
        WINDOWS_PAYLOAD_MANIFEST, WINDOWS_PAYLOAD_SIGNATURE, WindowsPayloadRefusal,
        render_windows_payload_manifest, verify_windows_payload,
    };

    const COMMIT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const LOCK: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    fn signed_fixture() -> tempfile::TempDir {
        let root = tempfile::tempdir().expect("payload root");
        fs::create_dir_all(root.path().join("bin")).expect("bin");
        fs::write(root.path().join("bin/ced.dll"), b"ced dll").expect("dll");
        fs::write(root.path().join("bin/tool.exe"), b"tool").expect("tool");
        let manifest = render_windows_payload_manifest(root.path(), COMMIT, LOCK).expect("render");
        let KeyPair { pk, sk } = KeyPair::generate_unencrypted_keypair().expect("keypair");
        let pin_dir = tempfile::tempdir().expect("pin dir");
        let pin = pin_dir.path().join("pin.pub");
        fs::write(&pin, pk.to_box().expect("public box").to_bytes()).expect("pin");
        install_test_fixture_pin(&pin).expect("install pin");
        let signature = minisign::sign(
            Some(&pk),
            &sk,
            Cursor::new(manifest.as_slice()),
            None,
            Some("fixture payload manifest"),
        )
        .expect("sign");
        let manifest_path = root.path().join(WINDOWS_PAYLOAD_MANIFEST);
        fs::create_dir_all(manifest_path.parent().expect("parent")).expect("provenance");
        fs::write(&manifest_path, manifest).expect("manifest");
        fs::write(
            root.path().join(WINDOWS_PAYLOAD_SIGNATURE),
            signature.into_string(),
        )
        .expect("signature");
        root
    }

    #[test]
    fn adapter_matches_declared_path_and_refusal_kind() {
        let root = signed_fixture();
        let verified = verify_windows_payload(root.path()).expect("admit");
        for path in ["bin/ced.dll", "bin/tool.exe"] {
            let direct = verified.declared_path(path).expect(path);
            let adapted = windows_declared_member(root.path(), path).expect(path);
            assert_eq!(direct, adapted, "{path}");
        }
        let direct_missing = verified
            .declared_path("bin/missing.exe")
            .expect_err("missing");
        let adapted_missing =
            windows_declared_member(root.path(), "bin/missing.exe").expect_err("missing");
        assert_eq!(direct_missing.kind, WindowsPayloadRefusal::MissingMember);
        assert_eq!(adapted_missing.kind, direct_missing.kind);
        assert_eq!(adapted_missing.detail, direct_missing.detail);

        let dll = root.path().join("bin/ced.dll");
        let mut bytes = fs::read(&dll).expect("read dll");
        bytes[0] ^= 0xff;
        fs::write(&dll, bytes).expect("flip dll");
        let direct = verify_windows_payload(root.path()).expect_err("mutated");
        let adapted = windows_declared_member(root.path(), "bin/ced.dll").expect_err("mutated");
        assert_eq!(direct.kind, WindowsPayloadRefusal::Digest);
        assert_eq!(adapted.kind, direct.kind);
        assert_eq!(adapted.detail, direct.detail);
    }
}
