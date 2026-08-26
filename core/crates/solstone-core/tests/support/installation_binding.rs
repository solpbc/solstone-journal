// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Test-only identity admission matching the supervisor's production gate.

use std::path::{Path, PathBuf};

use solstone_core_installation_identity::{
    ArtifactBindingEvidence, LegacyManifestEvidence, OwnerBase, PlatformTag, SetupAdmissionRequest,
    admit_setup, journal_token_from_path, namespace_name, root_token_from_path,
};

fn identity_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .canonicalize()
        .expect("workspace root from manifest directory")
}

pub fn admit_for(journal: &Path) -> PathBuf {
    let home = journal.join(".supervisor-home");
    std::fs::create_dir_all(&home).expect("test identity home");
    let root = identity_root();
    let owner = OwnerBase::at_home(home.clone(), PlatformTag::current()).expect("test owner");
    let admission = admit_setup(SetupAdmissionRequest {
        owner,
        root_token: root_token_from_path(&root).expect("root token"),
        journal_token: journal_token_from_path(journal).expect("journal token"),
        journal_is_explicit: true,
        legacy_manifest: LegacyManifestEvidence::Absent,
        artifacts: ArtifactBindingEvidence::Fresh,
    })
    .expect("test installation admission");
    drop(admission);
    home
}

#[allow(dead_code)]
pub fn admitted_record_path(home: &Path) -> PathBuf {
    let root = identity_root();
    let root_token = root_token_from_path(&root).expect("root token");
    let owner = OwnerBase::at_home(home.to_path_buf(), PlatformTag::current()).expect("test owner");
    let namespace = namespace_name(owner.platform(), &root_token);
    owner
        .path()
        .join("namespaces")
        .join(namespace.as_hex())
        .join("record")
}

#[allow(dead_code)]
pub fn corrupt_admitted_record_checksum(home: &Path) {
    let record_path = admitted_record_path(home);
    let mut record = std::fs::read(&record_path).expect("read admitted identity record");
    let line_end = record
        .len()
        .checked_sub(1)
        .expect("record has final newline");
    assert_eq!(record[line_end], b'\n', "record has final newline");
    let line_start = record[..line_end]
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(0, |index| index + 1);
    let checksum = b"checksum=";
    assert_eq!(
        &record[line_start..line_start + checksum.len()],
        checksum,
        "record checksum remains its final line"
    );
    let digit = line_start + checksum.len();
    assert!(
        record[digit].is_ascii_hexdigit(),
        "checksum starts with hex"
    );
    record[digit] = if record[digit] == b'a' { b'b' } else { b'a' };
    // Writing an existing file preserves its mode; only the checksum byte changes.
    std::fs::write(record_path, record).expect("corrupt admitted record checksum");
}
