// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![allow(dead_code)]

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use minisign::{KeyPair, PublicKey};
use solstone_core_distribution::inventory;
use solstone_core_distribution::promote::{PromoteRequest, promote};
use solstone_core_distribution::provenance::Provenance;

pub const BINARY: &str = env!("CARGO_BIN_EXE_solstone-distribution-fixture");
pub const PASSPHRASE: &str = "fixture-pass";
const HEX_COMMIT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const HEX_LOCK: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

static PUBLISHER_IDENTITY: OnceLock<(PublicKey, PathBuf, PathBuf)> = OnceLock::new();

pub struct Fixture {
    pub root: PathBuf,
    pub dest: PathBuf,
    pub basename: String,
    pub key_path: PathBuf,
    pub pin_path: PathBuf,
    pub foreign_pin_path: PathBuf,
    pub signing_pk: PublicKey,
    pub foreign_pk: PublicKey,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

pub fn scratch(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let path = PathBuf::from(format!(
        "/var/tmp/solstone-distribution-{label}-{}-{nanos}",
        std::process::id()
    ));
    fs::create_dir_all(&path).expect("scratch");
    path
}

pub fn linux_produce_dir(root: &Path, version: &str) -> (PathBuf, String) {
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
            commit: HEX_COMMIT.into(),
            lock_sha256: HEX_LOCK.into(),
        },
        expected: Provenance {
            commit: HEX_COMMIT.into(),
            lock_sha256: HEX_LOCK.into(),
        },
        fail_after: None,
        apple: None,
    })
    .expect("promote linux fixture");
    for name in inventory::artifact_set(&basename) {
        assert!(dest.join(&name).is_file(), "missing {name}");
    }
    (dest, basename)
}

pub fn write_identity(path: &Path, pass: &str) -> (PublicKey, PathBuf, PathBuf) {
    let KeyPair { pk, sk } =
        KeyPair::generate_encrypted_keypair(Some(pass.to_owned())).expect("keypair");
    let key_path = path.join("fixture.key");
    let pin_path = path.join("fixture.pub");
    fs::write(&key_path, sk.to_box(None).expect("secret box").to_bytes()).expect("write key");
    fs::write(&pin_path, pk.to_box().expect("public box").to_bytes()).expect("write pin");
    (pk, key_path, pin_path)
}

pub fn build_fixture(label: &str, version: &str) -> Fixture {
    let root = scratch(label);
    let keys = root.join("keys");
    fs::create_dir_all(&keys).expect("keys");
    let (dest, basename) = linux_produce_dir(&root, version);
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

fn publisher_identity() -> &'static (PublicKey, PathBuf, PathBuf) {
    PUBLISHER_IDENTITY.get_or_init(|| {
        let root = scratch("publisher-identity");
        write_identity(&root, PASSPHRASE)
    })
}

pub fn publisher_pin_path() -> &'static Path {
    &publisher_identity().2
}

pub fn build_publisher_fixture(label: &str, version: &str) -> Fixture {
    let root = scratch(label);
    let (dest, basename) = linux_produce_dir(&root, version);
    let (signing_pk, key_path, pin_path) = publisher_identity();
    let foreign_dir = root.join("foreign");
    fs::create_dir_all(&foreign_dir).expect("foreign");
    let (foreign_pk, _, foreign_pin_path) = write_identity(&foreign_dir, PASSPHRASE);
    Fixture {
        root,
        dest,
        basename,
        key_path: key_path.clone(),
        pin_path: pin_path.clone(),
        foreign_pin_path,
        signing_pk: signing_pk.clone(),
        foreign_pk,
    }
}

pub fn sign_dir(
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
    if let Err(error) = child.stdin.take().expect("stdin").write_all(stdin)
        && error.kind() != std::io::ErrorKind::BrokenPipe
    {
        panic!("write stdin: {error}");
    }
    child.wait_with_output().expect("wait sign")
}

pub fn sign_ok(dir: &Path, key: &Path, pin: &Path, stdin: &[u8]) -> Output {
    let output = sign_dir(dir, Some(key), Some(pin), stdin, None);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    output
}

#[allow(dead_code)]
pub fn sign_fixture(fixture: &Fixture) {
    sign_ok(
        &fixture.dest,
        &fixture.key_path,
        &fixture.pin_path,
        PASSPHRASE.as_bytes(),
    );
}

pub fn minisig_path(fixture: &Fixture) -> PathBuf {
    fixture
        .dest
        .join(format!("{}.manifest.json.minisig", fixture.basename))
}

pub fn manifest_path(fixture: &Fixture) -> PathBuf {
    fixture
        .dest
        .join(format!("{}.manifest.json", fixture.basename))
}

pub fn partial_path(fixture: &Fixture) -> PathBuf {
    fixture.dest.join(format!(
        "{}.manifest.json.minisig.partial",
        fixture.basename
    ))
}

pub fn assert_no_signature_artifacts(fixture: &Fixture) {
    assert!(!minisig_path(fixture).exists());
    assert!(!partial_path(fixture).exists());
}
