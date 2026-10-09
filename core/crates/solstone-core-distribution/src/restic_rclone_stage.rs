// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io::{Cursor, Write};
    use std::path::Path;

    use crate::digest::sha256_hex;
    use crate::inventory::StagedMember;
    use crate::pinned_stage::{ResolvedPin, plan_pinned_input, stage_pinned_plans};

    // Valid bzip2 bytes decompressing to b"hello bz2"
    const SYNTHETIC_RESTIC_BYTES: &[u8] = b"hello bz2";
    const SYNTHETIC_RESTIC_BZ2: &[u8] = &[
        66, 90, 104, 57, 49, 65, 89, 38, 83, 89, 252, 208, 76, 212, 0, 0, 2, 25, 128, 64, 0, 16, 0,
        18, 68, 128, 16, 32, 0, 49, 12, 8, 32, 15, 40, 54, 104, 195, 226, 238, 72, 167, 10, 18, 31,
        154, 9, 154, 128,
    ];

    fn make_zip(entries: &[(&str, &[u8], u32)]) -> Vec<u8> {
        let cursor = Cursor::new(Vec::new());
        let mut zip = zip::ZipWriter::new(cursor);
        for (path, data, mode) in entries {
            let opts = zip::write::SimpleFileOptions::default().unix_permissions(*mode);
            zip.start_file(*path, opts).unwrap();
            zip.write_all(data).unwrap();
        }
        let mut bytes = zip.finish().unwrap().into_inner();
        let cd_sig = [0x50, 0x4b, 0x01, 0x02];
        for (path, _, mode) in entries {
            let path_bytes = path.as_bytes();
            let mut i = 0;
            while i + 46 + path_bytes.len() <= bytes.len() {
                if bytes[i..i + 4] == cd_sig {
                    let name_len = u16::from_le_bytes([bytes[i + 28], bytes[i + 29]]) as usize;
                    if name_len == path_bytes.len()
                        && &bytes[i + 46..i + 46 + name_len] == path_bytes
                    {
                        bytes[i + 5] = 3; // Unix
                        let mode_bytes = (*mode << 16).to_le_bytes();
                        bytes[i + 38..i + 42].copy_from_slice(&mode_bytes);
                        break;
                    }
                }
                i += 1;
            }
        }
        bytes
    }

    #[test]
    fn test_synthetic_restic_bz2_staging_and_verification() {
        let stage_dir = tempfile::tempdir().expect("tempdir");
        let stage = stage_dir.path();

        let extracted_sha = sha256_hex(SYNTHETIC_RESTIC_BYTES);
        let bz2_sha = sha256_hex(SYNTHETIC_RESTIC_BZ2);
        let pin = ResolvedPin {
            sha256_hex: bz2_sha.clone(),
            size: SYNTHETIC_RESTIC_BZ2.len() as u64,
        };
        let staged_member = StagedMember {
            relpath: String::new(),
            dest: "lib/solstone-restic/restic".to_string(),
            mode: 0o755,
            extracted_sha256: extracted_sha.clone(),
            identity: None,
        };

        // 1. Successful plan and stage
        let plans = plan_pinned_input(
            "lib/solstone-restic/restic",
            SYNTHETIC_RESTIC_BZ2,
            &pin,
            "restic_0.19.0_linux_amd64.bz2",
            std::slice::from_ref(&staged_member),
            &[],
        )
        .expect("plan restic bz2");

        stage_pinned_plans(
            "lib/solstone-restic/restic",
            stage,
            SYNTHETIC_RESTIC_BZ2,
            "restic_0.19.0_linux_amd64.bz2",
            &plans,
        )
        .expect("stage restic bz2");

        let staged_path = stage.join("lib/solstone-restic/restic");
        assert!(staged_path.exists());
        assert_eq!(fs::read(&staged_path).unwrap(), SYNTHETIC_RESTIC_BYTES);
        assert!(!stage.join("bin").exists());

        // 2. A one-byte archive change fails the pin check
        let mut corrupted_bz2 = SYNTHETIC_RESTIC_BZ2.to_vec();
        corrupted_bz2[0] ^= 0xff;
        let err = plan_pinned_input(
            "lib/solstone-restic/restic",
            &corrupted_bz2,
            &pin,
            "restic_0.19.0_linux_amd64.bz2",
            std::slice::from_ref(&staged_member),
            &[],
        )
        .expect_err("tampered archive must fail pin check");
        assert!(err.to_string().contains("pin sha256 mismatch"));

        // 3. A wrong extracted_sha256 fails extract verification
        let bad_member = StagedMember {
            extracted_sha256: "0000000000000000000000000000000000000000000000000000000000000000"
                .into(),
            ..staged_member
        };
        let bad_plans = plan_pinned_input(
            "lib/solstone-restic/restic",
            SYNTHETIC_RESTIC_BZ2,
            &pin,
            "restic_0.19.0_linux_amd64.bz2",
            &[bad_member],
            &[],
        )
        .expect("plan accepts declared sha");
        let extract_err = stage_pinned_plans(
            "lib/solstone-restic/restic",
            stage,
            SYNTHETIC_RESTIC_BZ2,
            "restic_0.19.0_linux_amd64.bz2",
            &bad_plans,
        )
        .expect_err("wrong extracted sha must fail extract verification");
        assert!(
            extract_err
                .to_string()
                .contains("extracted sha256 mismatch")
        );
    }

    #[test]
    fn test_synthetic_rclone_zip_staging_and_verification() {
        let stage_dir = tempfile::tempdir().expect("tempdir");
        let stage = stage_dir.path();

        let rclone_binary = b"synthetic rclone binary payload";
        let zip_entries = [
            (
                "rclone-v1.74.4-linux-amd64/rclone",
                &rclone_binary[..],
                0o755,
            ),
            (
                "rclone-v1.74.4-linux-amd64/README.html",
                b"<html></html>",
                0o644,
            ),
            (
                "rclone-v1.74.4-linux-amd64/README.txt",
                b"readme text",
                0o644,
            ),
            ("rclone-v1.74.4-linux-amd64/rclone.1", b"manual page", 0o644),
        ];
        let zip_bytes = make_zip(&zip_entries);

        let extracted_sha = sha256_hex(rclone_binary);
        let zip_sha = sha256_hex(&zip_bytes);
        let pin = ResolvedPin {
            sha256_hex: zip_sha.clone(),
            size: zip_bytes.len() as u64,
        };
        let staged_member = StagedMember {
            relpath: "rclone-v1.74.4-linux-amd64/rclone".to_string(),
            dest: "lib/solstone-rclone/rclone".to_string(),
            mode: 0o755,
            extracted_sha256: extracted_sha.clone(),
            identity: None,
        };
        let ignored = [
            "rclone-v1.74.4-linux-amd64/README.html".to_string(),
            "rclone-v1.74.4-linux-amd64/README.txt".to_string(),
            "rclone-v1.74.4-linux-amd64/rclone.1".to_string(),
        ];

        // 1. Successful plan and stage
        let plans = plan_pinned_input(
            "lib/solstone-rclone/rclone",
            &zip_bytes,
            &pin,
            "rclone-v1.74.4-linux-amd64.zip",
            std::slice::from_ref(&staged_member),
            &ignored,
        )
        .expect("plan rclone zip");

        stage_pinned_plans(
            "lib/solstone-rclone/rclone",
            stage,
            &zip_bytes,
            "rclone-v1.74.4-linux-amd64.zip",
            &plans,
        )
        .expect("stage rclone zip");

        let staged_path = stage.join("lib/solstone-rclone/rclone");
        assert!(staged_path.exists());
        assert_eq!(fs::read(&staged_path).unwrap(), rclone_binary);
        assert!(!stage.join("bin").exists());

        // 2. A one-byte archive change fails the pin check
        let mut corrupted_zip = zip_bytes.clone();
        corrupted_zip[0] ^= 0xff;
        let err = plan_pinned_input(
            "lib/solstone-rclone/rclone",
            &corrupted_zip,
            &pin,
            "rclone-v1.74.4-linux-amd64.zip",
            std::slice::from_ref(&staged_member),
            &ignored,
        )
        .expect_err("tampered zip must fail pin check");
        assert!(err.to_string().contains("pin sha256 mismatch"));

        // 3. A wrong extracted_sha256 fails extract verification
        let bad_member = StagedMember {
            extracted_sha256: "0000000000000000000000000000000000000000000000000000000000000000"
                .into(),
            ..staged_member
        };
        let bad_plans = plan_pinned_input(
            "lib/solstone-rclone/rclone",
            &zip_bytes,
            &pin,
            "rclone-v1.74.4-linux-amd64.zip",
            &[bad_member],
            &ignored,
        )
        .expect("plan accepts declared sha");
        let extract_err = stage_pinned_plans(
            "lib/solstone-rclone/rclone",
            stage,
            &zip_bytes,
            "rclone-v1.74.4-linux-amd64.zip",
            &bad_plans,
        )
        .expect_err("wrong extracted sha must fail extract verification");
        assert!(
            extract_err
                .to_string()
                .contains("extracted sha256 mismatch")
        );
    }

    #[test]
    fn test_licence_bytes_staging_across_targets() {
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let inventory_toml = r#"
version = 1
product = "solstone"
payload = "payload.txt"
payload_dest_prefix = "share"
payload_src_root = "payload"
deny = []
[artifact]
basename = "solstone-{version}-{os}-{arch}"
[apple]
team_id = "7QCG8V4M6H"
app_identity = "Developer ID Application: sol pbc (7QCG8V4M6H)"
notary_profile = "sol-pbc-notary"
keychain = "~/Library/Keychains/sol-signing.keychain-db"
codesign_path = "/usr/bin/codesign"
xcode = "Xcode 26.6"
notarytool = "1.1.2 (41)"
[[target]]
id = "linux-x86_64"
os = "linux"
arch = "x86_64"
deb_arch = "amd64"
rpm_arch = "x86_64"
triple_musl = "x86_64-unknown-linux-musl"
triple_gnu = "x86_64-unknown-linux-gnu"
zig_gnu = "x86_64-linux-gnu.2.27"
[[target]]
id = "linux-aarch64"
os = "linux"
arch = "aarch64"
deb_arch = "arm64"
rpm_arch = "aarch64"
triple_musl = "aarch64-unknown-linux-musl"
triple_gnu = "aarch64-unknown-linux-gnu"
zig_gnu = "aarch64-linux-gnu.2.27"
[[target]]
id = "macos-arm64"
os = "macos"
arch = "arm64"
lane = "apple-native"
triple_apple = "aarch64-apple-darwin"
min_macos = "15.0"
[[target]]
id = "windows-x86_64"
os = "windows"
arch = "x86_64"
lane = "msvc-native"
triple_windows = "x86_64-pc-windows-msvc"
[[entry]]
kind = "licence-tree"
class = "notice"
source = "core/distribution/licenses/restic"
component = "restic"
targets = ["linux-x86_64", "linux-aarch64", "macos-arm64", "windows-x86_64"]
[[entry]]
kind = "licence-tree"
class = "notice"
source = "core/distribution/licenses/rclone"
component = "rclone"
targets = ["linux-x86_64", "linux-aarch64", "macos-arm64", "windows-x86_64"]
"#;

        let root = tempfile::tempdir().expect("tempdir");
        let dist_dir = root.path().join("core/distribution");
        fs::create_dir_all(&dist_dir).expect("dist dir");
        fs::write(dist_dir.join("payload.txt"), "").expect("write payload");
        let inventory_path = dist_dir.join("inventory.toml");
        fs::write(&inventory_path, inventory_toml).expect("write inventory");
        #[cfg(unix)]
        std::os::unix::fs::symlink(
            repo.join("core/distribution/licenses"),
            dist_dir.join("licenses"),
        )
        .expect("symlink licenses");
        let inventory = crate::inventory::load_inventory(&inventory_path).expect("load inventory");

        for target_id in [
            "linux-x86_64",
            "linux-aarch64",
            "macos-arm64",
            "windows-x86_64",
        ] {
            let stage_dir = root.path().join(format!("stage-{target_id}"));
            fs::create_dir_all(&stage_dir).expect("stage dir");

            crate::produce::stage_layout(
                &repo,
                &crate::pinned_stage::catalog_input_cache_dir(&repo),
                &inventory_path,
                &inventory,
                target_id,
                None,
                None,
                None,
                &stage_dir,
            )
            .expect("stage layout");

            let is_windows = target_id == "windows-x86_64";
            for comp in ["restic", "rclone"] {
                let source_dir = repo.join("core/distribution/licenses").join(comp);
                let rel_files = crate::inventory::collect_licence_relative_paths(
                    &source_dir,
                    &format!("core/distribution/licenses/{comp}"),
                )
                .expect("collect licence files");
                assert!(
                    !rel_files.is_empty(),
                    "licence files for {comp} must not be empty"
                );

                let prefix = if is_windows {
                    format!("share/licenses/{comp}/")
                } else {
                    format!("share/solstone-journal/licenses/{comp}/")
                };

                for rel in rel_files {
                    let source_bytes = fs::read(source_dir.join(&rel)).expect("read source file");
                    let staged_file = stage_dir.join(&prefix).join(&rel);
                    assert!(
                        staged_file.exists(),
                        "staged file must exist: {}",
                        staged_file.display()
                    );
                    let staged_bytes = fs::read(&staged_file).expect("read staged file");
                    assert_eq!(
                        source_bytes, staged_bytes,
                        "staged file must be byte-identical: {rel}"
                    );
                }
            }
        }
    }

    #[test]
    fn test_macos_fake_signer_post_sign_digest() {
        use crate::apple::ArchiveMemberSigner;
        use crate::apple::FakeArchiveMemberSigner;
        use crate::provenance::Provenance;

        let stage_dir = tempfile::tempdir().expect("stage tempdir");
        let stage = stage_dir.path();
        let member_rel = "lib/solstone-restic/restic";
        let staged_file = stage.join(member_rel);
        fs::create_dir_all(staged_file.parent().unwrap()).unwrap();

        let pre_sign_bytes = b"synthetic restic binary for macos";
        fs::write(&staged_file, pre_sign_bytes).unwrap();
        let declared_extracted_sha = sha256_hex(pre_sign_bytes);

        // Pre-sign bytes equal the declared extracted digest
        let pre_sign_digest = sha256_hex(&fs::read(&staged_file).unwrap());
        assert_eq!(pre_sign_digest, declared_extracted_sha);

        // Sign with FakeArchiveMemberSigner
        let signer = FakeArchiveMemberSigner::new("test");
        let signed = signer
            .sign_executable(&staged_file, member_rel)
            .expect("sign executable");

        let inventory = {
            let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
            crate::inventory::load_inventory(&repo.join("core/distribution/inventory.toml"))
                .expect("load inventory")
        };
        let request = crate::promote::PromoteRequest {
            dest: stage_dir.path().join("dest"),
            work: stage_dir.path().join("work"),
            tree: vec![],
            version: "2.0.37".to_string(),
            basename: "solstone-journal-2.0.37-macos-arm64".to_string(),
            os: "macos".to_string(),
            arch: "macos-arm64".to_string(),
            deb_arch: String::new(),
            rpm_arch: String::new(),
            dirty: false,
            observed: Provenance {
                commit: "aaa".to_string(),
                lock_sha256: "bbb".to_string(),
            },
            expected: Provenance {
                commit: "aaa".to_string(),
                lock_sha256: "bbb".to_string(),
            },
            fail_after: None,
            apple: None,
            inventory,
            fail_evidence_install: false,
            archives: Vec::new(),
            stage_mutator: None,
        };

        crate::promote::render_installed_manifest(&request, stage)
            .expect("render installed manifest");

        let manifest_bytes =
            fs::read(stage.join(solstone_core_installed_payload::INSTALLED_PAYLOAD_MANIFEST))
                .expect("read manifest");
        let manifest_json: serde_json::Value =
            serde_json::from_slice(&manifest_bytes).expect("parse manifest json");
        let files = manifest_json["files"].as_array().expect("files array");
        let manifest_entry = files
            .iter()
            .find(|f| f["path"] == member_rel)
            .expect("found restic entry in manifest");

        let manifest_sha = manifest_entry["sha256"].as_str().expect("sha string");
        assert_eq!(manifest_sha, signed.sha256);
        assert_ne!(pre_sign_digest, signed.sha256);
    }
}
