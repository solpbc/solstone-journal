// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Test-only identity admission matching the supervisor's production gate.

use std::path::{Path, PathBuf};

use solstone_core_installation_identity::{
    ArtifactBindingEvidence, LegacyManifestEvidence, OwnerBase, PlatformTag, SetupAdmissionRequest,
    admit_setup, journal_token_from_path, root_token_from_path,
};

pub fn admit_for(journal: &Path) -> PathBuf {
    let home = journal.join(".supervisor-home");
    std::fs::create_dir_all(&home).expect("test identity home");
    let executable = PathBuf::from(env!("CARGO_BIN_EXE_solstone-core"));
    let root = executable
        .ancestors()
        .find(|path| path.join("pyproject.toml").is_file())
        .expect("workspace root for test binary");
    let owner = OwnerBase::at_home(home.clone(), PlatformTag::current()).expect("test owner");
    let admission = admit_setup(SetupAdmissionRequest {
        owner,
        root_token: root_token_from_path(root).expect("root token"),
        journal_token: journal_token_from_path(journal).expect("journal token"),
        journal_is_explicit: true,
        legacy_manifest: LegacyManifestEvidence::Absent,
        artifacts: ArtifactBindingEvidence::Fresh,
    })
    .expect("test installation admission");
    drop(admission);
    home
}
