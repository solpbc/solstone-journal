// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use solstone_core_distribution::manifest_verify::{
    capture_signature, discover_manifest, install_test_fixture_pin, validate_release_set,
    verify_manifest_signature,
};
use solstone_core_distribution::promote;
use solstone_core_distribution::publish::{self, PublishRequest};

mod support;

use support::{
    Fixture, PASSPHRASE, build_publisher_fixture, minisig_path, publisher_pin_path, sign_fixture,
    sign_ok, write_identity,
};

fn fixture(label: &str, version: &str) -> Fixture {
    // Every publisher fixture uses one process-lifetime test identity, so this
    // process-wide override is stable even while the integration tests run in
    // parallel.
    install_test_fixture_pin(publisher_pin_path()).expect("install fixture pin");
    let fixture = build_publisher_fixture(label, version);
    sign_fixture(&fixture);
    fixture
}

fn request(src: &Path, dest: &Path, lane: &str) -> PublishRequest {
    PublishRequest {
        src: src.to_path_buf(),
        dest: dest.to_path_buf(),
        lane: lane.to_owned(),
        fail_after: None,
    }
}

fn snapshot(path: &Path) -> BTreeMap<String, Vec<u8>> {
    promote::snapshot_dir(path).expect("snapshot")
}

fn latest_body(dest: &Path, lane: &str) -> String {
    fs::read_to_string(dest.join("solstone-journal").join(lane).join("latest")).expect("latest")
}

fn names(dir: &Path) -> Vec<String> {
    let mut names = fs::read_dir(dir)
        .expect("read directory")
        .map(|entry| {
            entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect::<Vec<_>>();
    names.sort();
    names
}

fn assert_signature_refusal_leaves_dest_unchanged(fixture: &Fixture, expected: &[&str]) {
    let dest = fixture.root.join("dest");
    fs::create_dir_all(&dest).expect("dest");
    fs::write(dest.join("marker"), b"prior").expect("marker");
    let before = snapshot(&dest);
    let error =
        publish::run(&request(&fixture.dest, &dest, "release")).expect_err("signature refusal");
    assert!(
        expected
            .iter()
            .any(|refusal| error.to_string().contains(refusal)),
        "expected one of {expected:?}: {error}"
    );
    assert_eq!(snapshot(&dest), before);
}

#[test]
fn ac1_publish_writes_lane_version_layout_and_latest() {
    let fixture = fixture("publish-ac1", "1.2.3");
    let dest = fixture.root.join("dest");
    let args = [
        "--lane".to_owned(),
        "release".to_owned(),
        fixture.dest.display().to_string(),
        "--dest".to_owned(),
        dest.display().to_string(),
    ];
    let report = publish::run_cli(&args).expect("publish");
    let version_dir = dest.join("solstone-journal/release/1.2.3");
    assert_eq!(report.version, "1.2.3");
    assert_eq!(names(&version_dir), names(&fixture.dest));
    for name in names(&fixture.dest) {
        assert_eq!(
            fs::read(fixture.dest.join(&name)).expect("source"),
            fs::read(version_dir.join(&name)).expect("published"),
            "{name}"
        );
    }
    assert_eq!(latest_body(&dest, "release"), "version=1.2.3\n");
}

#[test]
fn ac2_latest_pointer_advances_to_v2_in_every_lane() {
    for lane in ["release", "staging", "dev"] {
        let first = fixture(&format!("publish-ac2-{lane}-first"), "1.0.0");
        let second = fixture(&format!("publish-ac2-{lane}-second"), "2.0.0");
        let dest = first.root.join("dest");
        publish::run(&request(&first.dest, &dest, lane)).expect("publish v1");
        let v1 = dest.join("solstone-journal").join(lane).join("1.0.0");
        let before = snapshot(&v1);
        publish::run(&request(&second.dest, &dest, lane)).expect("publish v2");
        assert_eq!(latest_body(&dest, lane), "version=2.0.0\n");
        assert_eq!(snapshot(&v1), before);
    }
}

#[test]
fn ac3_fail_after_objects_leaves_all_objects_and_prior_latest() {
    let first = fixture("publish-ac3-first", "1.0.0");
    let second = fixture("publish-ac3-second", "2.0.0");
    let dest = first.root.join("dest");
    publish::run(&request(&first.dest, &dest, "release")).expect("publish v1");
    let mut failed = request(&second.dest, &dest, "release");
    failed.fail_after = Some("objects".into());
    assert!(
        publish::run(&failed)
            .expect_err("objects checkpoint")
            .to_string()
            .contains("injected-failure objects")
    );
    let version_dir = dest.join("solstone-journal/release/2.0.0");
    assert_eq!(names(&version_dir), names(&second.dest));
    assert_eq!(latest_body(&dest, "release"), "version=1.0.0\n");
}

#[test]
fn ac4_fail_after_object_leaves_partial_not_final() {
    let fixture = fixture("publish-ac4", "1.2.3");
    let dest = fixture.root.join("dest");
    let source_names = names(&fixture.dest);
    let target = source_names[source_names.len() / 2].clone();
    let mut failed = request(&fixture.dest, &dest, "release");
    failed.fail_after = Some(format!("object:{target}"));
    assert!(
        publish::run(&failed)
            .expect_err("object checkpoint")
            .to_string()
            .contains(&format!("injected-failure object:{target}"))
    );
    let version_dir = dest.join("solstone-journal/release/1.2.3");
    assert!(
        version_dir
            .join(format!("{target}.publish-partial"))
            .is_file()
    );
    assert!(!version_dir.join(&target).exists());
}

#[test]
fn ac5_closed_set_is_immutable_and_republish_keeps_latest_behavior() {
    for lane in ["release", "staging"] {
        let fixture = fixture(&format!("publish-ac5-{lane}"), "1.2.3");
        let dest = fixture.root.join("dest");
        publish::run(&request(&fixture.dest, &dest, lane)).expect("first publish");
        let version_dir = dest.join("solstone-journal").join(lane).join("1.2.3");
        let before = snapshot(&version_dir);
        fs::write(fixture.dest.join("extra.txt"), b"unlisted").expect("extra source entry");
        assert!(
            publish::run(&request(&fixture.dest, &dest, lane))
                .expect_err("closed set")
                .to_string()
                .contains("unexpected-top-level-regular")
        );
        assert_eq!(snapshot(&version_dir), before);
        fs::remove_file(fixture.dest.join("extra.txt")).expect("remove extra");
        publish::run(&request(&fixture.dest, &dest, lane)).expect("idempotent republish");
        assert_eq!(snapshot(&version_dir), before);
        assert_eq!(latest_body(&dest, lane), "version=1.2.3\n");
    }

    let fixture = fixture("publish-ac5-dev", "1.2.3");
    let dest = fixture.root.join("dest");
    publish::run(&request(&fixture.dest, &dest, "dev")).expect("first dev publish");
    let version_dir = dest.join("solstone-journal/dev/1.2.3");
    let name = format!("{}.tar.gz", fixture.basename);
    fs::write(version_dir.join(&name), b"local corruption").expect("corrupt destination");
    publish::run(&request(&fixture.dest, &dest, "dev")).expect("dev overwrite from captured set");
    assert_eq!(
        fs::read(version_dir.join(&name)).expect("published tar"),
        fs::read(fixture.dest.join(&name)).expect("source tar")
    );
}

#[test]
fn ac7_invalid_src_refuses_before_any_dest_write() {
    let root = support::scratch("publish-ac7");
    let dest = root.join("dest");
    fs::create_dir_all(&dest).expect("dest");
    fs::write(dest.join("marker"), b"prior").expect("marker");
    let before = snapshot(&dest);
    for src in [root.join("missing"), root.join("empty")] {
        if src.file_name().is_some_and(|name| name == "empty") {
            fs::create_dir_all(&src).expect("empty");
        }
        assert!(publish::run(&request(&src, &dest, "release")).is_err());
        assert_eq!(snapshot(&dest), before);
    }
    fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn ac8_publish_writes_no_stray_root_artifacts() {
    let fixture = fixture("publish-ac8", "1.2.3");
    let dest = fixture.root.join("dest");
    publish::run(&request(&fixture.dest, &dest, "release")).expect("publish");
    assert_eq!(names(&dest), vec!["solstone-journal".to_owned()]);
    for name in names(&fixture.dest) {
        assert!(!dest.join(&name).exists(), "stray {name}");
    }
}

#[test]
fn ac16_resume_after_objects_failure_sets_latest_to_v2() {
    let first = fixture("publish-ac16-first", "1.0.0");
    let second = fixture("publish-ac16-second", "2.0.0");
    let dest = first.root.join("dest");
    publish::run(&request(&first.dest, &dest, "release")).expect("first");
    let mut failed = request(&second.dest, &dest, "release");
    failed.fail_after = Some("objects".into());
    assert!(publish::run(&failed).is_err());
    let v2 = dest.join("solstone-journal/release/2.0.0");
    let before = snapshot(&v2);
    publish::run(&request(&second.dest, &dest, "release")).expect("resume");
    assert_eq!(snapshot(&v2), before);
    assert_eq!(latest_body(&dest, "release"), "version=2.0.0\n");
}

#[cfg(unix)]
#[test]
fn listed_member_symlink_refuses_publishing() {
    use std::os::unix::fs::symlink;

    let fixture = fixture("publish-listed-symlink", "1.2.3");
    let name = format!("{}.deb", fixture.basename);
    let member = fixture.dest.join(&name);
    let replacement = fixture.root.join("replacement.deb");
    fs::rename(&member, &replacement).expect("move member");
    symlink(&replacement, &member).expect("symlink member");
    let error = publish::run(&request(
        &fixture.dest,
        &fixture.root.join("dest"),
        "release",
    ))
    .expect_err("symlink refusal");
    assert!(
        error.to_string().contains("listed-member-symlink"),
        "{error}"
    );
}

#[test]
fn missing_corrupted_or_foreign_signature_refuse_before_destination_write() {
    let missing = fixture("publish-missing-signature", "1.2.3");
    fs::remove_file(minisig_path(&missing)).expect("remove signature");
    assert_signature_refusal_leaves_dest_unchanged(&missing, &["missing-signature"]);

    let corrupted = fixture("publish-corrupted-signature", "1.2.3");
    let minisig = minisig_path(&corrupted);
    let mut bytes = fs::read(&minisig).expect("signature");
    let signature_start = bytes
        .iter()
        .position(|byte| *byte == b'\n')
        .expect("first line")
        + 1;
    let signature_end = signature_start
        + bytes[signature_start..]
            .iter()
            .position(|byte| *byte == b'\n')
            .expect("signature line");
    let index = (signature_start..signature_end)
        .rfind(|index| bytes[*index].is_ascii_alphanumeric())
        .expect("base64 character");
    bytes[index] = if bytes[index] == b'A' { b'B' } else { b'A' };
    fs::write(&minisig, bytes).expect("corrupt signature");
    // A damaged minisign may still parse and fail the pinned-key check, or no
    // longer parse at all. Both refuse safely before the destination changes.
    assert_signature_refusal_leaves_dest_unchanged(
        &corrupted,
        &["signature-pin-mismatch", "unparseable-signature"],
    );

    let foreign = fixture("publish-foreign-signature", "1.2.3");
    let foreign_dir = foreign.root.join("replacement-key");
    fs::create_dir_all(&foreign_dir).expect("foreign key dir");
    let (_, foreign_key, foreign_pin) = write_identity(&foreign_dir, PASSPHRASE);
    sign_ok(
        &foreign.dest,
        &foreign_key,
        &foreign_pin,
        PASSPHRASE.as_bytes(),
    );
    assert_signature_refusal_leaves_dest_unchanged(&foreign, &["signature-pin-mismatch"]);
}

#[test]
fn validated_release_set_captures_member_bytes_before_later_source_mutation() {
    let fixture = fixture("publish-captured-member", "1.2.3");
    let manifest = discover_manifest(&fixture.dest).expect("manifest");
    let signature = capture_signature(&fixture.dest, &manifest, true)
        .expect("signature")
        .expect("required signature");
    verify_manifest_signature(&manifest, &signature).expect("signature verifies");
    let validated =
        validate_release_set(&fixture.dest, &manifest, Some(&signature)).expect("release set");
    let name = format!("{}.rpm", fixture.basename);
    let captured = validated
        .members
        .get(&name)
        .expect("captured member")
        .clone();

    fs::write(fixture.dest.join(&name), b"mutated after validation").expect("mutate member");

    assert_eq!(validated.members.get(&name), Some(&captured));
    assert_ne!(
        fs::read(fixture.dest.join(&name)).expect("mutated member"),
        captured
    );
}

#[test]
fn digest_mismatch_and_duplicate_manifest_member_refuse_publishing() {
    let mismatch = fixture("publish-digest-mismatch", "1.2.3");
    let archive = mismatch.dest.join(format!("{}.rpm", mismatch.basename));
    fs::write(&archive, b"changed").expect("change member");
    assert!(
        publish::run(&request(
            &mismatch.dest,
            &mismatch.root.join("dest"),
            "release"
        ))
        .expect_err("digest mismatch")
        .to_string()
        .contains("member-digest-mismatch")
    );

    let duplicate = fixture("publish-duplicate-member", "1.2.3");
    let manifest = duplicate
        .dest
        .join(format!("{}.manifest.json", duplicate.basename));
    let text = fs::read_to_string(&manifest).expect("manifest");
    let marker = "  \"files\": {\n";
    let first = text
        .lines()
        .find(|line| line.starts_with("    \""))
        .expect("member line");
    fs::write(
        &manifest,
        text.replacen(marker, &format!("{marker}{first}\n"), 1),
    )
    .expect("duplicate member");
    assert!(
        publish::run(&request(
            &duplicate.dest,
            &duplicate.root.join("dest"),
            "release"
        ))
        .expect_err("duplicate manifest key")
        .to_string()
        .contains("duplicate-member")
    );
}

#[cfg(unix)]
#[test]
fn publish_output_is_a_valid_install_sh_origin() {
    use std::os::unix::fs::PermissionsExt;

    let fixture = fixture("publish-install-origin", "1.2.3");
    let origin = fixture.root.join("origin");
    publish::run(&request(&fixture.dest, &origin, "release")).expect("publish");
    let bin = fixture.root.join("bin");
    fs::create_dir_all(&bin).expect("bin");
    let curl = bin.join("curl");
    fs::write(
        &curl,
        r#"#!/bin/sh
set -eu
while [ "$#" -gt 0 ]; do
  case "$1" in
    -sS|--http1.1) shift ;;
    -D) headers=$2; shift 2 ;;
    -o) dest=$2; shift 2 ;;
    -w) shift 2 ;;
    *) url=$1; shift ;;
  esac
done
file="$FAKE_CURL_ROOT/${url#https://updates.solstone.app/}"
if [ -f "$file" ]; then
  cp "$file" "$dest"
  [ -z "${headers:-}" ] || printf 'HTTP/1.1 200 OK\r\n\r\n' >"$headers"
  printf 200
else
  : >"$dest"
  [ -z "${headers:-}" ] || printf 'HTTP/1.1 404 Not Found\r\n\r\n' >"$headers"
  printf 404
fi
"#,
    )
    .expect("fake curl");
    let mut permissions = fs::metadata(&curl).expect("curl metadata").permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&curl, permissions).expect("chmod curl");
    // Signature semantics have their own integration matrix. This origin test
    // needs only a verifier-shaped executable so it can reach extraction.
    let minisign = bin.join("minisign");
    fs::write(&minisign, "#!/bin/sh\nexit 0\n").expect("fake minisign");
    let mut permissions = fs::metadata(&minisign)
        .expect("minisign metadata")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&minisign, permissions).expect("chmod minisign");
    let prefix = fixture.root.join("prefix");
    let install = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../distribution/install.sh");
    let output = std::process::Command::new("sh")
        .arg(install)
        .args([
            "--prefix",
            prefix.to_str().expect("prefix"),
            "--origin",
            "https://updates.solstone.app",
            "--no-path",
        ])
        .env("FAKE_CURL_ROOT", &origin)
        .env("SOLSTONE_UNAME_S", "Linux")
        .env("SOLSTONE_UNAME_M", "x86_64")
        .env("TMPDIR", &fixture.root)
        .env(
            "PATH",
            format!(
                "{}:{}",
                bin.display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )
        .output()
        .expect("install");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(prefix.join("current").is_symlink());
}
