// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use solstone_core_distribution::controlled_build::{
    BuildConfiguration, BuilderIdentity, CONTROLLED_BUILD_RECEIPT_SCHEMA_V1,
    ControlledBuildReceipt, ControlledBuildReceiptDraft, ControlledBuildReceiptPersistenceError,
    ControlledBuildReceiptPublication, DependencySource, SourceIdentity, ValidationReference,
    encode_controlled_build_receipt, read_controlled_build_receipt,
    write_controlled_build_receipt_exclusive,
};
use solstone_core_distribution::digest::sha256_hex;
use solstone_core_distribution::provenance::Provenance;

const RECEIPT_RACE_PATH: &str = "SOLSTONE_CONTROLLED_BUILD_RECEIPT_RACE_PATH";
const RECEIPT_RACE_WRITER: &str = "SOLSTONE_CONTROLLED_BUILD_RECEIPT_RACE_WRITER";

fn receipt(writer: &str) -> ControlledBuildReceipt {
    ControlledBuildReceiptDraft {
        schema: Some(CONTROLLED_BUILD_RECEIPT_SCHEMA_V1.to_owned()),
        source: Some(SourceIdentity {
            product: Provenance {
                commit: "fixture-product-commit".into(),
                lock_sha256: sha256_hex(b"fixture-product-lock"),
            },
            windows_dependency: DependencySource {
                repository: "fixture-dependency".into(),
                revision: "fixture-revision".into(),
                content_sha256: sha256_hex(b"fixture-dependency-content"),
            },
        }),
        inputs: Some(Vec::new()),
        builder: Some(BuilderIdentity {
            host: format!("receipt-race-{writer}"),
            toolchain: "fixture-toolchain".into(),
        }),
        configuration: Some(BuildConfiguration {
            target_triple: "fixture-target".into(),
            profile: "fixture-profile".into(),
            flags: Vec::new(),
            network_access_denied: true,
        }),
        outputs: Some(Vec::new()),
        supporting: Some(Vec::new()),
        validation: Some(ValidationReference {
            description: "fixture validation".into(),
            sha256: sha256_hex(b"fixture-validation"),
        }),
    }
    .validate()
    .expect("complete fixture receipt")
}

#[test]
#[ignore = "spawned twice by the parent race"]
fn controlled_build_receipt_two_process_same_path_child() {
    let path = PathBuf::from(
        std::env::var_os(RECEIPT_RACE_PATH).expect("race child requires destination path"),
    );
    let writer = std::env::var(RECEIPT_RACE_WRITER).expect("race writer identity");
    match write_controlled_build_receipt_exclusive(&path, &receipt(&writer)) {
        Ok(ControlledBuildReceiptPublication::Durable { .. }) => {
            #[cfg(windows)]
            panic!("Windows must not report durable receipt publication");
            #[cfg(not(windows))]
            println!("controlled-build-receipt-race={writer}:winner-durable");
        }
        Ok(ControlledBuildReceiptPublication::PublishedButNotDurable { .. }) => {
            #[cfg(not(windows))]
            panic!("non-Windows test filesystem must fully confirm publication");
            #[cfg(windows)]
            println!("controlled-build-receipt-race={writer}:winner-not-durable");
        }
        Ok(ControlledBuildReceiptPublication::PublicationUnconfirmed { publication, .. }) => {
            panic!("race winner must have a confirmed final name and cleanup: {publication:?}");
        }
        Err(ControlledBuildReceiptPersistenceError::Publish { source, .. })
            if source.source.kind() == io::ErrorKind::AlreadyExists =>
        {
            println!("controlled-build-receipt-race={writer}:loser-existing");
        }
        Err(other) => panic!("race writer failed unexpectedly: {other:?}"),
    }
}

fn spawn_writer(
    executable: &Path,
    child_name: &str,
    path: &Path,
    writer: &str,
) -> std::process::Child {
    Command::new(executable)
        .arg("--ignored")
        .arg("--exact")
        .arg(child_name)
        .arg("--nocapture")
        .env(RECEIPT_RACE_PATH, path)
        .env(RECEIPT_RACE_WRITER, writer)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn race writer")
}

#[test]
fn controlled_build_receipt_two_process_same_path_has_one_unmodified_winner() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let path = temporary.path().join("receipt.json");
    let executable = std::env::current_exe().expect("current test executable");
    let child_name = "controlled_build_receipt_two_process_same_path_child";
    let first = spawn_writer(&executable, child_name, &path, "first");
    let second = spawn_writer(&executable, child_name, &path, "second");
    let first_output = first.wait_with_output().expect("first result");
    let second_output = second.wait_with_output().expect("second result");
    assert!(
        first_output.status.success(),
        "first race writer failed: {}",
        String::from_utf8_lossy(&first_output.stderr)
    );
    assert!(
        second_output.status.success(),
        "second race writer failed: {}",
        String::from_utf8_lossy(&second_output.stderr)
    );
    let output = format!(
        "{}{}",
        String::from_utf8_lossy(&first_output.stdout),
        String::from_utf8_lossy(&second_output.stdout)
    );
    assert_eq!(
        output.matches(":loser-existing").count(),
        1,
        "exactly one writer must lose without modifying the winner: {output}"
    );
    assert_eq!(
        output.matches(":winner-").count(),
        1,
        "exactly one writer must publish: {output}"
    );

    let winner = read_controlled_build_receipt(&path).expect("strict winner reread");
    let expected_winner = match winner.builder.host.as_str() {
        "receipt-race-first" => receipt("first"),
        "receipt-race-second" => receipt("second"),
        other => panic!("final receipt has an unknown writer identity: {other}"),
    };
    assert_eq!(
        winner, expected_winner,
        "the published receipt must remain byte-for-byte attributable to one source writer"
    );
    let expected = encode_controlled_build_receipt(&expected_winner).expect("winner encoding");
    assert_eq!(fs::read(&path).expect("final receipt bytes"), expected);
    let names = fs::read_dir(temporary.path())
        .expect("race directory")
        .map(|entry| entry.expect("race entry").file_name())
        .collect::<Vec<_>>();
    assert_eq!(names, vec!["receipt.json"]);
    println!("CONTROLLED_BUILD_RECEIPT_RACE_PARENT=executed/pass");
}
