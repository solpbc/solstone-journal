// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Canonical evidence for the pre-build archive transformation.

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::archive_seal::SealedArchiveSet;
use crate::digest::sha256_hex;

pub const COMPILED_EXPECTATION_ENV: &str = "SOLSTONE_RFDETR_COMPILED_EXPECTATION_RS";
pub const RFDETR_ARCHIVE_SLOT_ID: &str = "rfdetr-macos-metal-arm64";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PrebuildInputIdentity {
    pub target_id: String,
    pub commit: String,
    pub lock_sha256: String,
    pub inventory_sha256: String,
    pub slots: Vec<PrebuildSlotInput>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PrebuildSlotInput {
    pub slot_id: String,
    pub source_sha256: String,
    pub executables: Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DeliveryContract {
    pub target_id: String,
    pub prebuild_input_sha256: String,
    pub slots: Vec<DeliverySlotOutput>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DeliverySlotOutput {
    pub slot_id: String,
    pub staged_dest: String,
    pub archive_sha256: String,
    pub archive_size: u64,
    pub executables: Vec<(String, String)>,
}

#[derive(Debug)]
pub struct ArchiveContractError {
    message: String,
}

impl ArchiveContractError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for ArchiveContractError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ArchiveContractError {}

impl From<std::io::Error> for ArchiveContractError {
    fn from(error: std::io::Error) -> Self {
        Self::new(error.to_string())
    }
}

impl PrebuildInputIdentity {
    #[must_use]
    pub fn from_sealed_archives(
        target_id: &str,
        commit: &str,
        lock_sha256: &str,
        inventory_bytes: &[u8],
        sealed: &SealedArchiveSet,
    ) -> Self {
        let mut slots = sealed
            .archives
            .iter()
            .map(|archive| {
                let mut executables = archive
                    .signed_executables
                    .iter()
                    .map(|executable| {
                        (
                            executable.member_path.clone(),
                            executable.source_sha256.clone(),
                        )
                    })
                    .collect::<Vec<_>>();
                executables.sort();
                PrebuildSlotInput {
                    slot_id: archive.slot_id.clone(),
                    source_sha256: archive.source_sha256.clone(),
                    executables,
                }
            })
            .collect::<Vec<_>>();
        slots.sort_by(|left, right| left.slot_id.cmp(&right.slot_id));
        Self {
            target_id: target_id.to_owned(),
            commit: commit.to_owned(),
            lock_sha256: lock_sha256.to_owned(),
            inventory_sha256: sha256_hex(inventory_bytes),
            slots,
        }
    }

    #[must_use]
    pub fn digest(&self) -> String {
        sha256_hex(&canonical_json(self))
    }
}

impl DeliveryContract {
    #[must_use]
    pub fn from_sealed_archives(
        prebuild: &PrebuildInputIdentity,
        sealed: &SealedArchiveSet,
    ) -> Self {
        let mut slots = sealed
            .archives
            .iter()
            .map(|archive| {
                let mut executables = archive
                    .signed_executables
                    .iter()
                    .map(|executable| {
                        (
                            executable.member_path.clone(),
                            executable.signed_sha256.clone(),
                        )
                    })
                    .collect::<Vec<_>>();
                executables.sort();
                DeliverySlotOutput {
                    slot_id: archive.slot_id.clone(),
                    staged_dest: archive.staged_dest.clone(),
                    archive_sha256: archive.sha256.clone(),
                    archive_size: archive.size,
                    executables,
                }
            })
            .collect::<Vec<_>>();
        slots.sort_by(|left, right| left.slot_id.cmp(&right.slot_id));
        Self {
            target_id: prebuild.target_id.clone(),
            prebuild_input_sha256: prebuild.digest(),
            slots,
        }
    }

    #[must_use]
    pub fn digest(&self) -> String {
        sha256_hex(&canonical_json(self))
    }
}

/// Write the value included by the future local-installer build script.
pub fn write_rfdetr_compiled_expectation(
    work: &Path,
    contract: &DeliveryContract,
) -> Result<PathBuf, ArchiveContractError> {
    let slot = contract
        .slots
        .iter()
        .find(|slot| slot.slot_id == RFDETR_ARCHIVE_SLOT_ID)
        .ok_or_else(|| {
            ArchiveContractError::new(format!(
                "missing required:\n  delivery slot {RFDETR_ARCHIVE_SLOT_ID}"
            ))
        })?;
    let [(member_path, executable_sha256)] = slot.executables.as_slice() else {
        return Err(ArchiveContractError::new(format!(
            "unexpected:\n  delivery slot {} executable count {}",
            slot.slot_id,
            slot.executables.len()
        )));
    };
    let directory = work.join("archive-contract");
    fs::create_dir_all(&directory)?;
    let path = directory.join("rfdetr_compiled_expectation_value.rs");
    let value = format!(
        "pub const MACOS_DELIVERY_CONTRACT: Option<CompiledDeliveryContract> = Some(CompiledDeliveryContract {{\n    delivery_contract_sha256: {:?},\n    slot_id: {:?},\n    archive_sha256: {:?},\n    archive_size: {},\n    executable_member_path: {:?},\n    executable_sha256: {:?},\n}});\n",
        contract.digest(),
        slot.slot_id,
        slot.archive_sha256,
        slot.archive_size,
        member_path,
        executable_sha256,
    );
    fs::write(&path, value)?;
    Ok(path)
}

fn canonical_json<T: Serialize>(value: &T) -> Vec<u8> {
    serde_json::to_vec(value).expect("archive contract serialization")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive_seal::{SealedArchive, SealedExecutable};

    fn sealed_archives() -> SealedArchiveSet {
        SealedArchiveSet {
            archives: vec![SealedArchive {
                slot_id: RFDETR_ARCHIVE_SLOT_ID.to_owned(),
                staged_dest: "lib/rfdetr.tar.gz".to_owned(),
                bytes: b"sealed".to_vec(),
                sha256: "a".repeat(64),
                size: 6,
                source_sha256: "b".repeat(64),
                signed_executables: vec![SealedExecutable {
                    member_path: "rfdetr/rfdetr-cli".to_owned(),
                    source_sha256: "c".repeat(64),
                    signed_sha256: "d".repeat(64),
                    mode: 0o755,
                }],
            }],
        }
    }

    #[test]
    fn delivery_contract_binds_the_prebuild_identity() {
        let sealed = sealed_archives();
        let input = PrebuildInputIdentity::from_sealed_archives(
            "macos-arm64",
            "commit",
            &"e".repeat(64),
            b"[inventory]",
            &sealed,
        );
        let delivery = DeliveryContract::from_sealed_archives(&input, &sealed);
        assert_eq!(delivery.prebuild_input_sha256, input.digest());
        assert_ne!(delivery.digest(), input.digest());
    }

    #[test]
    fn generated_expectation_has_the_pinned_value_shape() {
        let sealed = sealed_archives();
        let input = PrebuildInputIdentity::from_sealed_archives(
            "macos-arm64",
            "commit",
            &"e".repeat(64),
            b"[inventory]",
            &sealed,
        );
        let delivery = DeliveryContract::from_sealed_archives(&input, &sealed);
        let temporary = tempfile::tempdir().expect("temporary work");
        let path = write_rfdetr_compiled_expectation(temporary.path(), &delivery)
            .expect("write expectation");
        let value = fs::read_to_string(path).expect("read expectation");
        assert!(value.starts_with(
            "pub const MACOS_DELIVERY_CONTRACT: Option<CompiledDeliveryContract> = Some(CompiledDeliveryContract {"
        ));
        assert!(value.contains("delivery_contract_sha256:"));
        assert!(value.contains("slot_id: \"rfdetr-macos-metal-arm64\""));
        assert!(value.contains("archive_size: 6"));
        assert!(value.contains("executable_member_path: \"rfdetr/rfdetr-cli\""));
        assert!(value.ends_with("});\n"));
    }
}
