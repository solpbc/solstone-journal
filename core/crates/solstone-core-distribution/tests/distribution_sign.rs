// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use minisign::{PublicKey, SignatureBox};
use solstone_core_distribution::digest::sha256_hex;
mod support;

use support::{
    BINARY, PASSPHRASE, assert_no_signature_artifacts, build_fixture, manifest_path, minisig_path,
    partial_path, sign_dir, sign_ok,
};

fn sign_refuses(dir: &Path, key: Option<&Path>, pin: Option<&Path>, stdin: &[u8]) -> Output {
    let output = sign_dir(dir, key, pin, stdin, None);
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    output
}

fn assert_no_signature_paths(dir: &Path, basename: &str) {
    assert!(
        !dir.join(format!("{basename}.manifest.json.minisig"))
            .exists()
    );
    assert!(
        !dir.join(format!("{basename}.manifest.json.minisig.partial"))
            .exists()
    );
}

fn verify(pk: &PublicKey, manifest: &Path, minisig: &Path) -> bool {
    let signature = SignatureBox::from_file(minisig).expect("load minisig");
    let bytes = fs::read(manifest).expect("read manifest");
    minisign::verify(
        pk,
        &signature,
        std::io::Cursor::new(bytes),
        true,
        false,
        false,
    )
    .is_ok()
}

fn assert_minisig_format(path: &Path) {
    let text = fs::read_to_string(path).expect("read minisig");
    let mut lines = text.lines();
    let untrusted = lines.next().expect("untrusted");
    assert!(untrusted.starts_with("untrusted comment:"));
    lines.next().expect("signature");
    let trusted = lines.next().expect("trusted");
    assert!(trusted.starts_with("trusted comment:"));
    lines.next().expect("global");
}

#[test]
fn stdin_passphrase_writes_verifiable_minisig() {
    let fixture = build_fixture("sign-stdin", env!("CARGO_PKG_VERSION"));
    let before = fs::read(manifest_path(&fixture)).expect("manifest before");
    sign_ok(
        &fixture.dest,
        &fixture.key_path,
        &fixture.pin_path,
        PASSPHRASE.as_bytes(),
    );
    let minisig = minisig_path(&fixture);
    assert!(minisig.is_file());
    assert_minisig_format(&minisig);
    assert_eq!(
        fs::read(manifest_path(&fixture)).expect("manifest after"),
        before
    );
    assert!(verify(
        &fixture.signing_pk,
        &manifest_path(&fixture),
        &minisig
    ));
    assert!(!verify(
        &fixture.foreign_pk,
        &manifest_path(&fixture),
        &minisig
    ));
}

#[test]
fn adjacent_pass_file_signs_and_wrong_pass_refuses() {
    let fixture = build_fixture("sign-passfile", env!("CARGO_PKG_VERSION"));
    let pass_path = fixture.key_path.with_extension("pass");
    fs::write(&pass_path, PASSPHRASE.as_bytes()).expect("write .pass");
    sign_ok(&fixture.dest, &fixture.key_path, &fixture.pin_path, b"");
    let minisig = minisig_path(&fixture);
    assert!(minisig.is_file());
    assert!(verify(
        &fixture.signing_pk,
        &manifest_path(&fixture),
        &minisig
    ));

    fs::remove_file(&minisig).expect("remove signed dest");
    fs::write(&pass_path, b"not-the-passphrase").expect("wrong .pass");
    sign_refuses(
        &fixture.dest,
        Some(&fixture.key_path),
        Some(&fixture.pin_path),
        b"",
    );
    assert_no_signature_artifacts(&fixture);
}

#[test]
fn flipped_manifest_refuses_and_flipped_archive_keeps_signature() {
    let fixture = build_fixture("sign-flip", env!("CARGO_PKG_VERSION"));
    sign_ok(
        &fixture.dest,
        &fixture.key_path,
        &fixture.pin_path,
        PASSPHRASE.as_bytes(),
    );
    let minisig = minisig_path(&fixture);
    let manifest = manifest_path(&fixture);
    let original_manifest = fs::read(&manifest).expect("manifest");
    let mut flipped_manifest = original_manifest.clone();
    flipped_manifest[0] ^= 0x01;
    fs::write(&manifest, &flipped_manifest).expect("flip manifest");
    assert!(!verify(&fixture.signing_pk, &manifest, &minisig));

    fs::write(&manifest, &original_manifest).expect("restore manifest");
    let archive_name = format!("{}.tar.gz", fixture.basename);
    let archive = fixture.dest.join(&archive_name);
    let mut flipped_archive = fs::read(&archive).expect("archive");
    flipped_archive[0] ^= 0x01;
    fs::write(&archive, &flipped_archive).expect("flip archive");
    assert!(verify(&fixture.signing_pk, &manifest, &minisig));
    let listed = serde_json::from_slice::<serde_json::Value>(&original_manifest)
        .expect("manifest json")
        .get("files")
        .and_then(|files| files.get(&archive_name))
        .and_then(serde_json::Value::as_str)
        .expect("listed digest")
        .to_owned();
    assert_ne!(sha256_hex(&fs::read(&archive).expect("reread")), listed);
}

#[test]
fn every_unlisted_top_level_regular_file_refuses_signing() {
    let fixture = build_fixture("sign-extra", env!("CARGO_PKG_VERSION"));
    let extra_deb = fixture.dest.join("unlisted.deb");
    fs::write(&extra_deb, b"extra-deb").expect("extra deb");
    sign_refuses(
        &fixture.dest,
        Some(&fixture.key_path),
        Some(&fixture.pin_path),
        PASSPHRASE.as_bytes(),
    );
    assert_no_signature_artifacts(&fixture);

    fs::remove_file(&extra_deb).expect("remove extra deb");
    fs::write(fixture.dest.join("unlisted.pkg"), b"extra-pkg").expect("extra pkg");
    sign_refuses(
        &fixture.dest,
        Some(&fixture.key_path),
        Some(&fixture.pin_path),
        PASSPHRASE.as_bytes(),
    );
    assert_no_signature_artifacts(&fixture);

    fs::remove_file(fixture.dest.join("unlisted.pkg")).expect("remove extra pkg");
    fs::write(fixture.dest.join("unlisted.txt"), b"extra-text").expect("extra text");
    sign_refuses(
        &fixture.dest,
        Some(&fixture.key_path),
        Some(&fixture.pin_path),
        b"",
    );
    assert_no_signature_artifacts(&fixture);

    fs::remove_file(fixture.dest.join("unlisted.txt")).expect("remove extra text");
    fs::write(
        fixture
            .dest
            .join(format!("{}.signing.json", fixture.basename)),
        "{}\n",
    )
    .expect("unlisted sidecar");
    sign_refuses(
        &fixture.dest,
        Some(&fixture.key_path),
        Some(&fixture.pin_path),
        b"",
    );
    assert_no_signature_artifacts(&fixture);
}

#[test]
fn listed_archive_absent_or_digest_mismatch_refuses() {
    let fixture = build_fixture("sign-listed", env!("CARGO_PKG_VERSION"));
    let archive = fixture.dest.join(format!("{}.deb", fixture.basename));
    let original = fs::read(&archive).expect("deb");
    fs::remove_file(&archive).expect("remove listed");
    sign_refuses(
        &fixture.dest,
        Some(&fixture.key_path),
        Some(&fixture.pin_path),
        PASSPHRASE.as_bytes(),
    );
    assert_no_signature_artifacts(&fixture);

    let mut flipped = original.clone();
    flipped[0] ^= 0x01;
    fs::write(&archive, &flipped).expect("mismatch digest");
    sign_refuses(
        &fixture.dest,
        Some(&fixture.key_path),
        Some(&fixture.pin_path),
        PASSPHRASE.as_bytes(),
    );
    assert_no_signature_artifacts(&fixture);
}

#[cfg(unix)]
#[test]
fn listed_member_symlink_refuses_signing() {
    use std::os::unix::fs::symlink;

    let fixture = build_fixture("sign-listed-symlink", env!("CARGO_PKG_VERSION"));
    let archive = fixture.dest.join(format!("{}.deb", fixture.basename));
    let replacement = fixture.root.join("replacement.deb");
    fs::rename(&archive, &replacement).expect("move member");
    symlink(&replacement, &archive).expect("symlink listed member");
    sign_refuses(
        &fixture.dest,
        Some(&fixture.key_path),
        Some(&fixture.pin_path),
        b"",
    );
    assert_no_signature_artifacts(&fixture);
}

#[test]
fn duplicate_manifest_file_key_refuses_signing() {
    let fixture = build_fixture("sign-duplicate-member", env!("CARGO_PKG_VERSION"));
    let path = manifest_path(&fixture);
    let manifest = fs::read_to_string(&path).expect("manifest");
    let marker = "  \"files\": {\n";
    let first = manifest
        .lines()
        .find(|line| line.starts_with("    \""))
        .expect("first file line");
    let duplicate = format!("{first}\n");
    let rewritten = manifest.replacen(marker, &format!("{marker}{duplicate}"), 1);
    fs::write(path, rewritten).expect("duplicate key manifest");
    sign_refuses(
        &fixture.dest,
        Some(&fixture.key_path),
        Some(&fixture.pin_path),
        b"",
    );
    assert_no_signature_artifacts(&fixture);
}

#[test]
fn missing_inputs_each_refuse() {
    let fixture = build_fixture("sign-missing", env!("CARGO_PKG_VERSION"));
    let dest = &fixture.dest;
    let key = &fixture.key_path;
    let pin = &fixture.pin_path;

    let unset = sign_refuses(dest, None, Some(pin), PASSPHRASE.as_bytes());
    assert!(
        String::from_utf8_lossy(&unset.stderr).contains("missing-key-env"),
        "{unset:?}"
    );
    assert_no_signature_paths(dest, &fixture.basename);

    let mut empty_cmd = Command::new(BINARY);
    empty_cmd.arg("sign").arg(dest);
    empty_cmd.env("SOLSTONE_JOURNAL_MINISIGN_KEY", "");
    empty_cmd.env("SOLSTONE_JOURNAL_MINISIGN_PIN", pin);
    empty_cmd.stdin(Stdio::piped());
    empty_cmd.stdout(Stdio::piped());
    empty_cmd.stderr(Stdio::piped());
    let mut child = empty_cmd.spawn().expect("spawn empty");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(PASSPHRASE.as_bytes())
        .unwrap();
    let empty = child.wait_with_output().unwrap();
    assert_eq!(empty.status.code(), Some(2), "{empty:?}");
    assert!(
        String::from_utf8_lossy(&empty.stderr).contains("empty-key-path"),
        "{empty:?}"
    );
    assert_no_signature_paths(dest, &fixture.basename);

    let mut relative_cmd = Command::new(BINARY);
    relative_cmd.arg("sign").arg(dest);
    relative_cmd.env("SOLSTONE_JOURNAL_MINISIGN_KEY", "relative.key");
    relative_cmd.env("SOLSTONE_JOURNAL_MINISIGN_PIN", pin);
    relative_cmd.stdin(Stdio::piped());
    relative_cmd.stdout(Stdio::piped());
    relative_cmd.stderr(Stdio::piped());
    let mut child = relative_cmd.spawn().expect("spawn relative");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(PASSPHRASE.as_bytes())
        .unwrap();
    let relative = child.wait_with_output().unwrap();
    assert_eq!(relative.status.code(), Some(2), "{relative:?}");
    assert!(
        String::from_utf8_lossy(&relative.stderr).contains("relative-key-path"),
        "{relative:?}"
    );
    assert_no_signature_paths(dest, &fixture.basename);

    let missing_pass = sign_refuses(dest, Some(key), Some(pin), b"");
    assert!(
        String::from_utf8_lossy(&missing_pass.stderr).contains("missing-passphrase"),
        "{missing_pass:?}"
    );
    assert_no_signature_paths(dest, &fixture.basename);

    let empty_dir = fixture.root.join("empty-dir");
    fs::create_dir_all(&empty_dir).expect("empty dir");
    let missing_manifest = sign_refuses(&empty_dir, Some(key), Some(pin), PASSPHRASE.as_bytes());
    assert_eq!(missing_manifest.status.code(), Some(2));
    assert_no_signature_paths(&empty_dir, &fixture.basename);
    assert_no_signature_paths(dest, &fixture.basename);
}

#[test]
fn pin_mismatch_is_fail_closed_and_cwd_does_not_load_the_pin() {
    let fixture = build_fixture("sign-atomic", env!("CARGO_PKG_VERSION"));
    sign_ok(
        &fixture.dest,
        &fixture.key_path,
        &fixture.pin_path,
        PASSPHRASE.as_bytes(),
    );
    let minisig = minisig_path(&fixture);
    let first = fs::read(&minisig).expect("first minisig");
    assert!(verify(
        &fixture.signing_pk,
        &manifest_path(&fixture),
        &minisig
    ));

    let refused = sign_refuses(
        &fixture.dest,
        Some(&fixture.key_path),
        Some(&fixture.foreign_pin_path),
        PASSPHRASE.as_bytes(),
    );
    assert_eq!(refused.status.code(), Some(2));
    assert!(!partial_path(&fixture).exists());
    assert!(minisig.is_file());
    assert_eq!(fs::read(&minisig).expect("kept dest"), first);
    assert!(verify(
        &fixture.signing_pk,
        &manifest_path(&fixture),
        &minisig
    ));

    let away = fixture.root.join("not-repo");
    fs::create_dir_all(&away).expect("away");
    fs::remove_file(&minisig).expect("clear dest for cwd check");
    let output = sign_dir(
        &fixture.dest,
        Some(&fixture.key_path),
        Some(&fixture.pin_path),
        PASSPHRASE.as_bytes(),
        Some(&away),
    );
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(minisig.is_file());
    assert!(verify(
        &fixture.signing_pk,
        &manifest_path(&fixture),
        &minisig
    ));
}
