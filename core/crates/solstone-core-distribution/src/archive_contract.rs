// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Canonical evidence for the pre-build archive transformation.

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::archive_seal::SealedArchiveSet;
use crate::digest::sha256_hex;
use crate::record::FileRecord;
use crate::stage::{staged_records, write_staged_file};

pub const COMPILED_EXPECTATION_ENV: &str = "SOLSTONE_RFDETR_COMPILED_EXPECTATION_RS";
pub const RFDETR_ARCHIVE_SLOT_ID: &str = "rfdetr-macos-metal-arm64";
pub const PREBUILD_INPUT_DEST: &str = "archive-contracts/prebuild-input.json";
pub const DELIVERY_CONTRACT_DEST: &str = "archive-contracts/delivery-contract.json";
pub const FINAL_INVOCATION_DEST: &str = "archive-contracts/final-invocation.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrebuildInputIdentity {
    pub target_id: String,
    pub commit: String,
    pub lock_sha256: String,
    pub inventory_sha256: String,
    pub slots: Vec<PrebuildSlotInput>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrebuildSlotInput {
    pub slot_id: String,
    pub source_sha256: String,
    pub executables: Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliveryContract {
    pub target_id: String,
    pub prebuild_input_sha256: String,
    pub slots: Vec<DeliverySlotOutput>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliverySlotOutput {
    pub slot_id: String,
    pub staged_dest: String,
    pub archive_sha256: String,
    pub archive_size: u64,
    pub executables: Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FinalInvocationRecord {
    pub target_id: String,
    pub commit: String,
    pub lock_sha256: String,
    pub delivery_contract_sha256: String,
    pub root_predecessor_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ValidatedChain {
    pub prebuild_input_sha256: String,
    pub delivery_contract_sha256: String,
    pub final_invocation_sha256: String,
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

impl FinalInvocationRecord {
    #[must_use]
    pub fn digest(&self) -> String {
        sha256_hex(&canonical_json(self))
    }
}

/// Seal the archive-transformation chain into the stage tree.
///
/// The root predecessor census intentionally excludes `FINAL_INVOCATION_DEST`:
/// the final record is written only after the census, and excluding it keeps the
/// record from hashing itself if this function is ever reordered or rerun.
pub(crate) fn stage_chain(
    stage: &Path,
    prebuild: &PrebuildInputIdentity,
    delivery: &DeliveryContract,
    commit: &str,
    lock_sha256: &str,
) -> Result<FinalInvocationRecord, ArchiveContractError> {
    write_json(stage, PREBUILD_INPUT_DEST, prebuild)?;
    write_json(stage, DELIVERY_CONTRACT_DEST, delivery)?;
    let final_record = FinalInvocationRecord {
        target_id: prebuild.target_id.clone(),
        commit: commit.to_owned(),
        lock_sha256: lock_sha256.to_owned(),
        delivery_contract_sha256: delivery.digest(),
        root_predecessor_sha256: root_predecessor_digest(stage)?,
    };
    write_json(stage, FINAL_INVOCATION_DEST, &final_record)?;
    Ok(final_record)
}

pub(crate) fn validate_staged_chain(
    stage: &Path,
    target_id: &str,
    commit: &str,
    lock_sha256: &str,
) -> Result<ValidatedChain, ArchiveContractError> {
    let prebuild: PrebuildInputIdentity = read_json(stage, PREBUILD_INPUT_DEST)?;
    let delivery: DeliveryContract = read_json(stage, DELIVERY_CONTRACT_DEST)?;
    let final_record: FinalInvocationRecord = read_json(stage, FINAL_INVOCATION_DEST)?;

    for (node, observed) in [
        ("prebuild input", prebuild.target_id.as_str()),
        ("delivery contract", delivery.target_id.as_str()),
        ("final invocation", final_record.target_id.as_str()),
    ] {
        if observed != target_id {
            return Err(ArchiveContractError::new(format!(
                "{node} target does not match expected target {target_id}: {observed}"
            )));
        }
    }
    let prebuild_input_sha256 = prebuild.digest();
    if delivery.prebuild_input_sha256 != prebuild_input_sha256 {
        return Err(ArchiveContractError::new(
            "delivery contract does not match its prebuild input",
        ));
    }
    let delivery_contract_sha256 = delivery.digest();
    if final_record.delivery_contract_sha256 != delivery_contract_sha256 {
        return Err(ArchiveContractError::new(
            "final invocation does not match its delivery contract",
        ));
    }
    if final_record.commit != commit || final_record.lock_sha256 != lock_sha256 {
        return Err(ArchiveContractError::new(
            "final invocation does not match the expected commit and lock",
        ));
    }
    if final_record.root_predecessor_sha256 != root_predecessor_digest(stage)? {
        return Err(ArchiveContractError::new(
            "final invocation does not match the staged tree predecessor",
        ));
    }
    for slot in &delivery.slots {
        crate::archive::refuse_escape(&slot.staged_dest)
            .map_err(|error| ArchiveContractError::new(error.as_str()))?;
        let path = stage.join(&slot.staged_dest);
        let bytes = fs::read(&path).map_err(|error| {
            ArchiveContractError::new(format!(
                "could not read staged delivery archive {}: {error}",
                path.display()
            ))
        })?;
        if bytes.len() as u64 != slot.archive_size || sha256_hex(&bytes) != slot.archive_sha256 {
            return Err(ArchiveContractError::new(format!(
                "staged delivery archive does not match contract: {}",
                slot.staged_dest
            )));
        }
    }
    Ok(ValidatedChain {
        prebuild_input_sha256,
        delivery_contract_sha256,
        final_invocation_sha256: final_record.digest(),
    })
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

fn write_json<T: Serialize>(
    stage: &Path,
    dest: &str,
    value: &T,
) -> Result<(), ArchiveContractError> {
    let bytes = serde_json::to_vec(value).expect("archive contract serialization");
    write_staged_file(stage, dest, &bytes)?;
    Ok(())
}

fn read_json<T: for<'de> Deserialize<'de>>(
    stage: &Path,
    dest: &str,
) -> Result<T, ArchiveContractError> {
    let path = stage.join(dest);
    let bytes = fs::read(&path).map_err(|error| {
        ArchiveContractError::new(format!(
            "could not read archive chain {}: {error}",
            path.display()
        ))
    })?;
    serde_json::from_slice(&bytes).map_err(|error| {
        ArchiveContractError::new(format!(
            "could not parse archive chain {}: {error}",
            path.display()
        ))
    })
}

fn root_predecessor_digest(stage: &Path) -> Result<String, ArchiveContractError> {
    let records = staged_records(stage)?;
    Ok(root_predecessor_digest_records(&records))
}

fn root_predecessor_digest_records(records: &[FileRecord]) -> String {
    let canonical = records
        .iter()
        .filter(|record| record.dest != FINAL_INVOCATION_DEST)
        .map(|record| (&record.dest, &record.kind, record.mode, &record.digest))
        .collect::<Vec<_>>();
    sha256_hex(&canonical_json(&canonical))
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
                sha256: sha256_hex(b"sealed"),
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

    fn staged_chain_fixture() -> (tempfile::TempDir, PrebuildInputIdentity, DeliveryContract) {
        let temporary = tempfile::tempdir().expect("temporary stage");
        let stage = temporary.path().join("stage");
        write_staged_file(&stage, "bin/solstone-core", b"binary").expect("stage binary");
        let sealed = sealed_archives();
        for archive in &sealed.archives {
            write_staged_file(&stage, &archive.staged_dest, &archive.bytes)
                .expect("stage sealed archive");
        }
        let prebuild = PrebuildInputIdentity::from_sealed_archives(
            "macos-arm64",
            "commit",
            &"e".repeat(64),
            b"[inventory]",
            &sealed,
        );
        let delivery = DeliveryContract::from_sealed_archives(&prebuild, &sealed);
        stage_chain(&stage, &prebuild, &delivery, "commit", &"e".repeat(64))
            .expect("stage archive chain");
        (temporary, prebuild, delivery)
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

    #[test]
    fn staged_chain_round_trips_and_returns_its_digests() {
        let (temporary, prebuild, delivery) = staged_chain_fixture();
        let chain = validate_staged_chain(
            &temporary.path().join("stage"),
            "macos-arm64",
            "commit",
            &"e".repeat(64),
        )
        .expect("validate staged chain");
        assert_eq!(chain.prebuild_input_sha256, prebuild.digest());
        assert_eq!(chain.delivery_contract_sha256, delivery.digest());
        assert_eq!(chain.final_invocation_sha256.len(), 64);
    }

    #[test]
    fn missing_chain_file_refuses_then_restaging_passes() {
        let (temporary, prebuild, delivery) = staged_chain_fixture();
        let stage = temporary.path().join("stage");
        fs::remove_file(stage.join(PREBUILD_INPUT_DEST)).expect("remove prebuild record");
        assert!(
            validate_staged_chain(&stage, "macos-arm64", "commit", &"e".repeat(64))
                .expect_err("missing chain file refuses")
                .to_string()
                .contains("could not read archive chain")
        );

        stage_chain(&stage, &prebuild, &delivery, "commit", &"e".repeat(64))
            .expect("restage chain");
        validate_staged_chain(&stage, "macos-arm64", "commit", &"e".repeat(64))
            .expect("restaged chain passes");
    }

    #[test]
    fn unparseable_chain_file_refuses() {
        let (temporary, _, _) = staged_chain_fixture();
        let stage = temporary.path().join("stage");
        write_staged_file(&stage, DELIVERY_CONTRACT_DEST, b"not json")
            .expect("tamper delivery record");
        assert!(
            validate_staged_chain(&stage, "macos-arm64", "commit", &"e".repeat(64))
                .expect_err("unparseable chain file refuses")
                .to_string()
                .contains("could not parse archive chain")
        );
    }

    #[test]
    fn delivery_must_match_its_prebuild_input() {
        let (temporary, _, mut delivery) = staged_chain_fixture();
        let stage = temporary.path().join("stage");
        delivery.prebuild_input_sha256 = "tampered".to_owned();
        write_json(&stage, DELIVERY_CONTRACT_DEST, &delivery).expect("tamper delivery record");
        assert!(
            validate_staged_chain(&stage, "macos-arm64", "commit", &"e".repeat(64))
                .expect_err("mismatched delivery refuses")
                .to_string()
                .contains("delivery contract does not match its prebuild input")
        );
    }

    #[test]
    fn final_invocation_must_match_its_delivery_contract() {
        let (temporary, _, _) = staged_chain_fixture();
        let stage = temporary.path().join("stage");
        let mut final_record: FinalInvocationRecord =
            read_json(&stage, FINAL_INVOCATION_DEST).expect("read final record");
        final_record.delivery_contract_sha256 = "tampered".to_owned();
        write_json(&stage, FINAL_INVOCATION_DEST, &final_record).expect("tamper final record");
        assert!(
            validate_staged_chain(&stage, "macos-arm64", "commit", &"e".repeat(64))
                .expect_err("mismatched final invocation refuses")
                .to_string()
                .contains("final invocation does not match its delivery contract")
        );
    }

    #[test]
    fn final_invocation_must_match_the_expected_commit_and_lock() {
        let (temporary, _, _) = staged_chain_fixture();
        let stage = temporary.path().join("stage");
        assert!(
            validate_staged_chain(&stage, "macos-arm64", "other-commit", &"e".repeat(64))
                .expect_err("commit mismatch refuses")
                .to_string()
                .contains("final invocation does not match the expected commit and lock")
        );
        assert!(
            validate_staged_chain(&stage, "macos-arm64", "commit", &"f".repeat(64))
                .expect_err("lock mismatch refuses")
                .to_string()
                .contains("final invocation does not match the expected commit and lock")
        );
    }

    #[test]
    fn staged_tree_addition_refuses() {
        let (temporary, _, _) = staged_chain_fixture();
        let stage = temporary.path().join("stage");
        write_staged_file(&stage, "extra", b"unexpected").expect("add staged file");
        assert!(
            validate_staged_chain(&stage, "macos-arm64", "commit", &"e".repeat(64))
                .expect_err("staged addition refuses")
                .to_string()
                .contains("final invocation does not match the staged tree predecessor")
        );
    }

    #[test]
    fn staged_derivative_must_match_its_delivery_slot() {
        let (temporary, _, delivery) = staged_chain_fixture();
        let stage = temporary.path().join("stage");
        write_staged_file(&stage, &delivery.slots[0].staged_dest, b"tampered")
            .expect("tamper staged archive");
        let mut final_record: FinalInvocationRecord =
            read_json(&stage, FINAL_INVOCATION_DEST).expect("read final record");
        final_record.root_predecessor_sha256 =
            root_predecessor_digest(&stage).expect("recount stage");
        write_json(&stage, FINAL_INVOCATION_DEST, &final_record).expect("rewrite final record");
        assert!(
            validate_staged_chain(&stage, "macos-arm64", "commit", &"e".repeat(64))
                .expect_err("staged derivative mismatch refuses")
                .to_string()
                .contains("staged delivery archive does not match contract")
        );
    }

    #[test]
    fn each_chain_node_must_match_the_expected_target() {
        let (temporary, _, mut delivery) = staged_chain_fixture();
        let stage = temporary.path().join("stage");
        delivery.target_id = "macos-x86_64".to_owned();
        write_json(&stage, DELIVERY_CONTRACT_DEST, &delivery).expect("rewrite delivery record");
        let mut final_record: FinalInvocationRecord =
            read_json(&stage, FINAL_INVOCATION_DEST).expect("read final record");
        final_record.delivery_contract_sha256 = delivery.digest();
        final_record.root_predecessor_sha256 =
            root_predecessor_digest(&stage).expect("recount stage");
        write_json(&stage, FINAL_INVOCATION_DEST, &final_record).expect("rewrite final record");

        assert!(
            validate_staged_chain(&stage, "macos-arm64", "commit", &"e".repeat(64))
                .expect_err("delivery target mismatch refuses")
                .to_string()
                .contains("delivery contract target does not match expected target")
        );
    }
}
