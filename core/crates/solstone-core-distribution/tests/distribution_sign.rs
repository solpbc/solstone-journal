// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use minisign::{KeyPair, PublicKey, SignatureBox};
use solstone_core_distribution::digest::sha256_hex;
use solstone_core_distribution::inventory;
use solstone_core_distribution::promote::{PromoteRequest, promote};
use solstone_core_distribution::provenance::Provenance;

const BINARY: &str = env!("CARGO_BIN_EXE_solstone-distribution");
const PASSPHRASE: &str = "fixture-pass";

struct Fixture {
    root: PathBuf,
    dest: PathBuf,
    basename: String,
    key_path: PathBuf,
    pin_path: PathBuf,
    foreign_pin_path: PathBuf,
    signing_pk: PublicKey,
    foreign_pk: PublicKey,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn scratch(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let path = PathBuf::from(format!(
        "/var/tmp/solstone-distribution-sign-{label}-{}-{nanos}",
        std::process::id()
    ));
    fs::create_dir_all(&path).expect("scratch");
    path
}

fn linux_produce_dir(root: &Path) -> (PathBuf, String) {
    let version = env!("CARGO_PKG_VERSION");
    let basename = format!("solstone-journal-{version}-linux-x86_64");
    let dest = root.join("artifacts");
    let work = root.join("work");
    promote(&PromoteRequest {
        dest: dest.clone(),
        work,
        tree: vec![("bin/solstone-core".into(), b"core".to_vec(), 0o755)],
        version: version.to_owned(),
        basename: basename.clone(),
        os: "linux".into(),
        arch: "linux-x86_64".into(),
        deb_arch: "amd64".into(),
        rpm_arch: "x86_64".into(),
        dirty: false,
        observed: Provenance {
            commit: "aaa".into(),
            lock_sha256: "bbb".into(),
        },
        expected: Provenance {
            commit: "aaa".into(),
            lock_sha256: "bbb".into(),
        },
        fail_after: None,
        apple: None,
    })
    .expect("promote linux fixture");
    let expected = inventory::artifact_set(&basename);
    for name in &expected {
        assert!(dest.join(name).is_file(), "missing {name}");
    }
    (dest, basename)
}

fn write_identity(path: &Path, pass: &str) -> (PublicKey, PathBuf, PathBuf) {
    let KeyPair { pk, sk } =
        KeyPair::generate_encrypted_keypair(Some(pass.to_owned())).expect("keypair");
    let key_path = path.join("fixture.key");
    let pin_path = path.join("fixture.pub");
    fs::write(&key_path, sk.to_box(None).expect("secret box").to_bytes()).expect("write key");
    fs::write(&pin_path, pk.to_box().expect("public box").to_bytes()).expect("write pin");
    (pk, key_path, pin_path)
}

fn build_fixture(label: &str) -> Fixture {
    let root = scratch(label);
    let keys = root.join("keys");
    fs::create_dir_all(&keys).expect("keys");
    let (dest, basename) = linux_produce_dir(&root);
    let (signing_pk, key_path, pin_path) = write_identity(&keys, PASSPHRASE);
    let foreign_dir = keys.join("foreign");
    fs::create_dir_all(&foreign_dir).expect("foreign");
    let (foreign_pk, _, foreign_pin_path) = write_identity(&foreign_dir, PASSPHRASE);
    Fixture {
        root,
        dest,
        basename,
        key_path,
        pin_path,
        foreign_pin_path,
        signing_pk,
        foreign_pk,
    }
}

fn sign_dir(
    dir: &Path,
    key: Option<&Path>,
    pin: Option<&Path>,
    stdin: &[u8],
    cwd: Option<&Path>,
) -> Output {
    let mut command = Command::new(BINARY);
    command.arg("sign").arg(dir);
    command.env_remove("SOLSTONE_JOURNAL_MINISIGN_KEY");
    command.env_remove("SOLSTONE_JOURNAL_MINISIGN_PIN");
    if let Some(key) = key {
        command.env("SOLSTONE_JOURNAL_MINISIGN_KEY", key);
    }
    if let Some(pin) = pin {
        command.env("SOLSTONE_JOURNAL_MINISIGN_PIN", pin);
    }
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    command.stdin(Stdio::piped());
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
    let mut child = command.spawn().expect("spawn sign");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(stdin)
        .expect("write stdin");
    child.wait_with_output().expect("wait sign")
}

fn sign_ok(dir: &Path, key: &Path, pin: &Path, stdin: &[u8]) -> Output {
    let output = sign_dir(dir, Some(key), Some(pin), stdin, None);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    output
}

fn sign_refuses(dir: &Path, key: Option<&Path>, pin: Option<&Path>, stdin: &[u8]) -> Output {
    let output = sign_dir(dir, key, pin, stdin, None);
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    output
}

fn minisig_path(fixture: &Fixture) -> PathBuf {
    fixture
        .dest
        .join(format!("{}.manifest.json.minisig", fixture.basename))
}

fn partial_path(fixture: &Fixture) -> PathBuf {
    fixture.dest.join(format!(
        "{}.manifest.json.minisig.partial",
        fixture.basename
    ))
}

fn manifest_path(fixture: &Fixture) -> PathBuf {
    fixture
        .dest
        .join(format!("{}.manifest.json", fixture.basename))
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

fn assert_no_signature_artifacts(dir: &Path, basename: &str) {
    assert!(
        !dir.join(format!("{basename}.manifest.json.minisig"))
            .exists()
    );
    assert!(
        !dir.join(format!("{basename}.manifest.json.minisig.partial"))
            .exists()
    );
}

#[test]
fn stdin_passphrase_writes_verifiable_minisig() {
    let fixture = build_fixture("stdin");
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
    let fixture = build_fixture("passfile");
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
    assert_no_signature_artifacts(&fixture.dest, &fixture.basename);
}

#[test]
fn flipped_manifest_refuses_and_flipped_archive_keeps_signature() {
    let fixture = build_fixture("flip");
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
fn extra_unlisted_archive_refuses_allowed_sidecars_sign() {
    let fixture = build_fixture("extra");
    let extra_deb = fixture.dest.join("unlisted.deb");
    fs::write(&extra_deb, b"extra-deb").expect("extra deb");
    sign_refuses(
        &fixture.dest,
        Some(&fixture.key_path),
        Some(&fixture.pin_path),
        PASSPHRASE.as_bytes(),
    );
    assert_no_signature_artifacts(&fixture.dest, &fixture.basename);

    fs::remove_file(&extra_deb).expect("remove extra deb");
    fs::write(fixture.dest.join("unlisted.pkg"), b"extra-pkg").expect("extra pkg");
    sign_refuses(
        &fixture.dest,
        Some(&fixture.key_path),
        Some(&fixture.pin_path),
        PASSPHRASE.as_bytes(),
    );
    assert_no_signature_artifacts(&fixture.dest, &fixture.basename);

    fs::remove_file(fixture.dest.join("unlisted.pkg")).expect("remove extra pkg");
    fs::write(
        fixture
            .dest
            .join(format!("{}.signing.json", fixture.basename)),
        "{}\n",
    )
    .expect("optional sidecar");
    sign_ok(
        &fixture.dest,
        &fixture.key_path,
        &fixture.pin_path,
        PASSPHRASE.as_bytes(),
    );
    assert!(minisig_path(&fixture).is_file());
}

#[test]
fn listed_archive_absent_or_digest_mismatch_refuses() {
    let fixture = build_fixture("listed");
    let archive = fixture.dest.join(format!("{}.deb", fixture.basename));
    let original = fs::read(&archive).expect("deb");
    fs::remove_file(&archive).expect("remove listed");
    sign_refuses(
        &fixture.dest,
        Some(&fixture.key_path),
        Some(&fixture.pin_path),
        PASSPHRASE.as_bytes(),
    );
    assert_no_signature_artifacts(&fixture.dest, &fixture.basename);

    let mut flipped = original.clone();
    flipped[0] ^= 0x01;
    fs::write(&archive, &flipped).expect("mismatch digest");
    sign_refuses(
        &fixture.dest,
        Some(&fixture.key_path),
        Some(&fixture.pin_path),
        PASSPHRASE.as_bytes(),
    );
    assert_no_signature_artifacts(&fixture.dest, &fixture.basename);
}

#[test]
fn missing_inputs_each_refuse() {
    let fixture = build_fixture("missing");
    let dest = &fixture.dest;
    let key = &fixture.key_path;
    let pin = &fixture.pin_path;

    let unset = sign_refuses(dest, None, Some(pin), PASSPHRASE.as_bytes());
    assert!(
        String::from_utf8_lossy(&unset.stderr).contains("missing-key-env"),
        "{unset:?}"
    );
    assert_no_signature_artifacts(dest, &fixture.basename);

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
    assert_no_signature_artifacts(dest, &fixture.basename);

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
    assert_no_signature_artifacts(dest, &fixture.basename);

    let missing_pass = sign_refuses(dest, Some(key), Some(pin), b"");
    assert!(
        String::from_utf8_lossy(&missing_pass.stderr).contains("missing-passphrase"),
        "{missing_pass:?}"
    );
    assert_no_signature_artifacts(dest, &fixture.basename);

    let empty_dir = fixture.root.join("empty-dir");
    fs::create_dir_all(&empty_dir).expect("empty dir");
    let missing_manifest = sign_refuses(&empty_dir, Some(key), Some(pin), PASSPHRASE.as_bytes());
    assert_eq!(missing_manifest.status.code(), Some(2));
    assert_no_signature_artifacts(&empty_dir, &fixture.basename);
    assert_no_signature_artifacts(dest, &fixture.basename);
}

#[test]
fn pin_mismatch_is_fail_closed_and_cwd_does_not_load_the_pin() {
    let fixture = build_fixture("atomic");
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
