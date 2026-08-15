// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Durable proposals for removals that require an owner decision.
//!
//! This register records what retention proposes. It does not perform a removal;
//! the door remains the only module allowed to do that.

use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use solstone_core_journal_io::{
    AtomicWriteError, JsonWriteOptions, LockError, LockOptions, hold_lock, write_json,
};

use crate::receipt::Target;

const REGISTER_VERSION: u32 = 1;
const REGISTER_FILE: &str = "retention-marks.json";
const UNRECORDED_PROPOSAL_REASON: &str = "removal failed before a proposal was recorded";

/// Who originated a removal proposal.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Initiator {
    Owner,
    Policy,
    Offload,
}

/// Whether a proposal needs an approval beyond its initiator.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Approval {
    Required,
    NotRequired,
}

/// The scope of material a proposal concerns.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Material {
    RawOnly,
    FullSegment,
}

/// The four removal-proposal classes this register can hold.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RemovalClass {
    /// An owner-directed whole-segment removal.
    OwnerSegmentRemoval,
    /// An owner-directed raw-content release.
    OwnerRawRelease,
    /// A policy-driven raw-content release awaiting owner approval.
    ///
    /// The narrower "raw release doesn't need approval" reading from decision
    /// record `260806-founder-retention-marks-for-removal-and-the-owner-approves`
    /// is a one-field edit from [`Approval::Required`] to [`Approval::NotRequired`].
    PolicyRawRelease,
    /// An offload-driven raw-content release awaiting owner approval.
    ///
    /// The narrower "raw release doesn't need approval" reading from decision
    /// record `260806-founder-retention-marks-for-removal-and-the-owner-approves`
    /// is a one-field edit from [`Approval::Required`] to [`Approval::NotRequired`].
    /// Produced by the retention CLI's `mark-offload` verb.
    OffloadRawRelease,
}

impl RemovalClass {
    /// The class's originating actor, approval requirement, and material scope.
    pub fn axes(self) -> (Initiator, Approval, Material) {
        match self {
            Self::OwnerSegmentRemoval => (
                Initiator::Owner,
                Approval::NotRequired,
                Material::FullSegment,
            ),
            Self::OwnerRawRelease => (Initiator::Owner, Approval::NotRequired, Material::RawOnly),
            Self::PolicyRawRelease => (Initiator::Policy, Approval::Required, Material::RawOnly),
            Self::OffloadRawRelease => (Initiator::Offload, Approval::Required, Material::RawOnly),
        }
    }

    /// The stable discriminant included in a [`MarkId`] preimage.
    pub fn tag(self) -> &'static str {
        match self {
            Self::OwnerSegmentRemoval => "owner_segment_removal",
            Self::OwnerRawRelease => "owner_raw_release",
            Self::PolicyRawRelease => "policy_raw_release",
            Self::OffloadRawRelease => "offload_raw_release",
        }
    }
}

/// The canonical digest identity for one class-and-target proposal.
///
/// `Ord` and `Hash` are deliberate here: this is a canonical digest identity,
/// unlike [`Target`], whose duplicate request rows must remain distinct.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MarkId(String);

impl MarkId {
    /// Derive a stable ID from the class and the target's named path components.
    pub fn derive(class: RemovalClass, target: &Target, names: &[String]) -> Self {
        let class_tag = class.tag();
        let preimage = format!(
            "solstone-retention-mark-v1|{}:{}|{}:{}|{}:{}|{}:{}|names:{}{}",
            class_tag.len(),
            class_tag,
            target.day.len(),
            target.day,
            target.stream.len(),
            target.stream,
            target.dir.len(),
            target.dir,
            names.len(),
            names
                .iter()
                .map(|name| format!("|{}:{name}", name.len()))
                .collect::<String>(),
        );
        Self(format!("{:x}", Sha256::digest(preimage.as_bytes())))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Parse the canonical hexadecimal digest form emitted by the register.
    pub fn parse(value: &str) -> Option<Self> {
        (value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
            .then(|| Self(value.to_ascii_lowercase()))
    }
}

/// The current proposal for one class-and-target identity.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Mark {
    pub id: MarkId,
    pub class: RemovalClass,
    pub target: Target,
    pub marked_at: String,
    pub proposal: Proposal,
    pub state: MarkState,
}

/// Human-readable proposed removal scope.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Proposal {
    pub bytes: u64,
    pub reason: String,
    /// Sorted, unique basenames of the raw files covered by this proposal.
    pub names: Vec<String>,
}

/// Whether a mark is pending owner action or records an execution failure.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MarkState {
    Marked,
    Failed(Failure),
}

/// The most recent failure recorded for a mark.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Failure {
    pub at: String,
    pub reason: String,
    pub staged: Option<String>,
}

/// The versioned, canonical removal register.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Register {
    pub version: u32,
    pub marks: BTreeMap<MarkId, Mark>,
}

/// Marks proven ready for a removal run before any target is attempted.
#[derive(Clone, Debug)]
pub struct PreflightMarks(Vec<(MarkId, Mark)>);

impl PreflightMarks {
    pub(crate) fn as_slice(&self) -> &[(MarkId, Mark)] {
        &self.0
    }
}

/// Construct and upsert an offload proposal after the caller has scanned and
/// validated owner-media names. This is the importable half of the retention
/// CLI's `mark-offload` body.
pub fn upsert_offload(
    journal: &Path,
    target: &Target,
    names: Vec<String>,
    bytes: u64,
    reason: String,
    now: &str,
) -> Result<Register, StoreError> {
    upsert(
        journal,
        RemovalClass::OffloadRawRelease,
        target,
        Proposal {
            bytes,
            reason,
            names,
        },
        now,
    )
}

/// Resolve the current offload proposal for the target's exact file names.
pub fn resolve_offload(
    journal: &Path,
    target: &Target,
    names: &[String],
) -> Result<Register, StoreError> {
    resolve(
        journal,
        &MarkId::derive(RemovalClass::OffloadRawRelease, target, names),
    )
}

impl Register {
    pub fn empty() -> Self {
        Self {
            version: REGISTER_VERSION,
            marks: BTreeMap::new(),
        }
    }
}

/// Failure while reading or mutating the removal register.
#[derive(Debug)]
pub enum StoreError {
    Lock(LockError),
    Read(io::Error),
    Malformed(serde_json::Error),
    UnsupportedVersion { found: u32 },
    Integrity { reason: &'static str },
    DuplicateProposal { id: MarkId },
    Write(AtomicWriteError),
}

/// A reason a requested mark cannot be acted on before any target is attempted.
#[derive(Debug)]
pub enum PreflightRefusal {
    Empty,
    Duplicate,
    Missing { id: MarkId },
    NotRequired { id: MarkId },
    Failed { id: MarkId },
    Register(StoreError),
}

impl fmt::Display for PreflightRefusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(formatter, "at least one --mark is required"),
            Self::Duplicate => write!(formatter, "the same mark was named more than once"),
            Self::Missing { id } => write!(
                formatter,
                "no mark named `{}` exists; run `marks` to see current marks — an id changes whenever its proposal's file list changes",
                id.as_str()
            ),
            Self::NotRequired { id } => {
                write!(
                    formatter,
                    "mark `{}` does not require approval",
                    id.as_str()
                )
            }
            Self::Failed { id } => {
                write!(formatter, "mark `{}` has a recorded failure", id.as_str())
            }
            Self::Register(error) => error.fmt(formatter),
        }
    }
}

impl From<StoreError> for PreflightRefusal {
    fn from(error: StoreError) -> Self {
        Self::Register(error)
    }
}

impl fmt::Display for StoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Lock(_) => write!(
                formatter,
                "the removal register is busy; no marks were changed, so please try again"
            ),
            Self::Read(_) => write!(
                formatter,
                "the removal register could not be read; no marks were changed"
            ),
            Self::Malformed(_) => write!(
                formatter,
                "the removal register is not valid JSON; no marks were changed"
            ),
            Self::UnsupportedVersion { .. } => write!(
                formatter,
                "the removal register uses an unsupported version; no marks were changed"
            ),
            Self::Integrity { .. } => write!(
                formatter,
                "the removal register has inconsistent entries; no marks were changed"
            ),
            Self::DuplicateProposal { .. } => write!(
                formatter,
                "the same removal was proposed more than once; no marks were changed"
            ),
            Self::Write(_) => write!(
                formatter,
                "the removal register could not be saved; no marks were changed"
            ),
        }
    }
}

impl Error for StoreError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Lock(error) => Some(error),
            Self::Read(error) => Some(error),
            Self::Malformed(error) => Some(error),
            Self::Write(error) => Some(error),
            Self::UnsupportedVersion { .. }
            | Self::Integrity { .. }
            | Self::DuplicateProposal { .. } => None,
        }
    }
}

/// Read the register without creating its directory or file.
pub fn load(journal: &Path) -> Result<Register, StoreError> {
    load_path(&register_path(journal))
}

/// Validate named marks before a removal run reaches any target.
pub fn preflight(journal: &Path, ids: &[MarkId]) -> Result<PreflightMarks, PreflightRefusal> {
    let register = load(journal)?;
    preflight_register(&register, ids)
}

/// Drop one pending approval after validating it while holding the register lock.
pub fn decline(journal: &Path, id: &MarkId) -> Result<Register, PreflightRefusal> {
    mutate(journal, |register| {
        preflight_register(register, std::slice::from_ref(id))?;
        register.marks.remove(id);
        Ok(true)
    })
}

/// Reconcile one class's marks with the current set of proposals.
///
/// | Existing mark | Proposal this round | Action |
/// | --- | --- | --- |
/// | `Marked` | absent | Remove it. A stale mark must never be executable. |
/// | `Failed` | absent | Keep it unchanged, including `staged`. |
/// | `Failed` | present | Keep state, `staged`, and `marked_at`; refresh proposal data. |
/// | `Marked` | present | Preserve `marked_at`; refresh proposal data. |
/// | absent | present | Insert a new `Marked` entry using `at`. |
///
/// Marks of other classes are not selected for mutation.
pub fn reconcile(
    journal: &Path,
    class: RemovalClass,
    proposals: &[(Target, Proposal)],
    at: &str,
) -> Result<Register, StoreError> {
    let proposals = index_proposals(class, proposals)?;
    mutate(journal, |register| {
        let stale: Vec<MarkId> = register
            .marks
            .iter()
            .filter(|(id, mark)| {
                mark.class == class
                    && matches!(&mark.state, MarkState::Marked)
                    && !proposals.contains_key(*id)
            })
            .map(|(id, _)| id.clone())
            .collect();
        let mut changed = false;
        for id in stale {
            register.marks.remove(&id);
            changed = true;
        }

        for (id, (target, proposal)) in proposals {
            match register.marks.get_mut(&id) {
                Some(mark) => {
                    if mark.proposal != proposal {
                        mark.proposal = proposal;
                        changed = true;
                    }
                }
                None => {
                    register.marks.insert(
                        id.clone(),
                        Mark {
                            id,
                            class,
                            target,
                            marked_at: at.to_owned(),
                            proposal,
                            state: MarkState::Marked,
                        },
                    );
                    changed = true;
                }
            }
        }
        Ok(changed)
    })
}

/// Insert or update a single mark's proposal without touching any other mark.
///
/// Unlike [`reconcile`], this does not treat its input as the authoritative full
/// state for the class. `mark-offload` mints one mark per segment, so its updates
/// must leave marks from earlier segments and prior runs in place.
pub fn upsert(
    journal: &Path,
    class: RemovalClass,
    target: &Target,
    proposal: Proposal,
    at: &str,
) -> Result<Register, StoreError> {
    validate_proposal(&proposal)?;
    let id = MarkId::derive(class, target, &proposal.names);
    mutate(journal, |register| {
        let changed = match register.marks.get_mut(&id) {
            Some(mark) => {
                if mark.proposal != proposal {
                    mark.proposal = proposal;
                    true
                } else {
                    false
                }
            }
            None => {
                register.marks.insert(
                    id.clone(),
                    Mark {
                        id,
                        class,
                        target: target.clone(),
                        marked_at: at.to_owned(),
                        proposal,
                        state: MarkState::Marked,
                    },
                );
                true
            }
        };
        Ok(changed)
    })
}

/// Record the latest failure for one mark without reconciling other marks.
pub fn record_failure(
    journal: &Path,
    class: RemovalClass,
    target: &Target,
    names: &[String],
    failure: Failure,
    at: &str,
) -> Result<Register, StoreError> {
    mutate(journal, |register| {
        let id = MarkId::derive(class, target, names);
        match register.marks.get_mut(&id) {
            Some(mark) => {
                let state = MarkState::Failed(failure);
                if mark.state == state {
                    return Ok(false);
                }
                mark.state = state;
            }
            None => {
                register.marks.insert(
                    id.clone(),
                    Mark {
                        id,
                        class,
                        target: target.clone(),
                        marked_at: at.to_owned(),
                        proposal: Proposal {
                            bytes: 0,
                            reason: UNRECORDED_PROPOSAL_REASON.to_owned(),
                            names: names.to_vec(),
                        },
                        state: MarkState::Failed(failure),
                    },
                );
            }
        }
        Ok(true)
    })
}

/// Resolve a mark after its failure has been handled. Missing IDs are a no-op.
pub fn resolve(journal: &Path, id: &MarkId) -> Result<Register, StoreError> {
    mutate(journal, |register| Ok(register.marks.remove(id).is_some()))
}

/// Clear failed marks whose staged directory has been recovered or removed.
pub fn reconcile_recovered(journal: &Path) -> Result<Register, StoreError> {
    mutate(journal, |register| {
        let resolved = register
            .marks
            .iter()
            .filter_map(|(id, mark)| match &mark.state {
                MarkState::Failed(failure)
                    if failure
                        .staged
                        .as_ref()
                        .is_some_and(|staged| !journal.join(staged).exists()) =>
                {
                    Some(id.clone())
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        let changed = !resolved.is_empty();
        for id in resolved {
            register.marks.remove(&id);
        }
        Ok(changed)
    })
}

fn index_proposals(
    class: RemovalClass,
    proposals: &[(Target, Proposal)],
) -> Result<BTreeMap<MarkId, (Target, Proposal)>, StoreError> {
    let mut indexed = BTreeMap::new();
    for (target, proposal) in proposals {
        validate_proposal(proposal)?;
        let id = MarkId::derive(class, target, &proposal.names);
        if indexed
            .insert(id.clone(), (target.clone(), proposal.clone()))
            .is_some()
        {
            return Err(StoreError::DuplicateProposal { id });
        }
    }
    Ok(indexed)
}

fn preflight_register(
    register: &Register,
    ids: &[MarkId],
) -> Result<PreflightMarks, PreflightRefusal> {
    if ids.is_empty() {
        return Err(PreflightRefusal::Empty);
    }
    let mut unique = std::collections::BTreeSet::new();
    if ids.iter().any(|id| !unique.insert(id.as_str())) {
        return Err(PreflightRefusal::Duplicate);
    }
    let mut marks = Vec::new();
    for id in ids {
        let Some(mark) = register.marks.get(id) else {
            return Err(PreflightRefusal::Missing { id: id.clone() });
        };
        if mark.class.axes().1 != Approval::Required {
            return Err(PreflightRefusal::NotRequired { id: id.clone() });
        }
        if matches!(mark.state, MarkState::Failed(_)) {
            return Err(PreflightRefusal::Failed { id: id.clone() });
        }
        marks.push((id.clone(), mark.clone()));
    }
    Ok(PreflightMarks(marks))
}

fn mutate<F, E>(journal: &Path, mutation: F) -> Result<Register, E>
where
    F: FnOnce(&mut Register) -> Result<bool, E>,
    E: From<StoreError>,
{
    mutate_with_options(journal, register_lock_options(), mutation)
}

fn mutate_with_options<F, E>(
    journal: &Path,
    options: LockOptions,
    mutation: F,
) -> Result<Register, E>
where
    F: FnOnce(&mut Register) -> Result<bool, E>,
    E: From<StoreError>,
{
    let path = register_path(journal);
    let _lock = hold_lock(&path, options)
        .map_err(StoreError::Lock)
        .map_err(E::from)?;
    let mut register = load_path(&path).map_err(E::from)?;
    if mutation(&mut register)? {
        validate(&register).map_err(E::from)?;
        write_json(
            &path,
            &register,
            JsonWriteOptions {
                mode: Some(0o600),
                ..JsonWriteOptions::default()
            },
        )
        .map_err(StoreError::Write)
        .map_err(E::from)?;
    }
    Ok(register)
}

fn register_path(journal: &Path) -> PathBuf {
    journal.join("health").join(REGISTER_FILE)
}

fn register_lock_options() -> LockOptions {
    LockOptions {
        mode: Some(0o600),
        ..LockOptions::default()
    }
}

fn load_path(path: &Path) -> Result<Register, StoreError> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Register::empty()),
        Err(error) => return Err(StoreError::Read(error)),
    };
    let register = serde_json::from_slice(&bytes).map_err(StoreError::Malformed)?;
    validate(&register)?;
    Ok(register)
}

fn validate(register: &Register) -> Result<(), StoreError> {
    if register.version != REGISTER_VERSION {
        return Err(StoreError::UnsupportedVersion {
            found: register.version,
        });
    }
    for (id, mark) in &register.marks {
        if id != &mark.id {
            return Err(StoreError::Integrity {
                reason: "a stored key does not match its mark",
            });
        }
        validate_proposal(&mark.proposal)?;
        if *id != MarkId::derive(mark.class, &mark.target, &mark.proposal.names) {
            return Err(StoreError::Integrity {
                reason: "a stored mark does not match its class and target",
            });
        }
    }
    Ok(())
}

fn validate_proposal(proposal: &Proposal) -> Result<(), StoreError> {
    if proposal.names.windows(2).any(|names| {
        names
            .first()
            .zip(names.get(1))
            .is_some_and(|(left, right)| left >= right)
    }) {
        return Err(StoreError::Integrity {
            reason: "proposal file names are not sorted and unique",
        });
    }
    if proposal
        .names
        .iter()
        .any(|name| name.is_empty() || name.contains('/'))
    {
        return Err(StoreError::Integrity {
            reason: "proposal contains an invalid file name",
        });
    }
    Ok(())
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::disallowed_methods,
    clippy::disallowed_types,
    reason = "test code; the crate-level denials exist to constrain the store"
)]
mod tests {
    use serde_json::json;

    use super::*;

    fn target(day: &str, stream: &str, dir: &str) -> Target {
        Target {
            day: day.to_owned(),
            stream: stream.to_owned(),
            dir: dir.to_owned(),
        }
    }

    fn proposal(reason: &str) -> Proposal {
        Proposal {
            bytes: 42,
            reason: reason.to_owned(),
            names: vec!["a.flac".to_owned(), "b.wav".to_owned()],
        }
    }

    fn mark(class: RemovalClass, state: MarkState) -> Mark {
        let target = target("20260805", "field.audio", "070000_17");
        let proposal = proposal("current");
        Mark {
            id: MarkId::derive(class, &target, &proposal.names),
            class,
            target,
            marked_at: "first".to_owned(),
            proposal,
            state,
        }
    }

    #[test]
    fn every_class_has_its_declared_axes() {
        for (class, expected_initiator, expected_approval, expected_material) in [
            (
                RemovalClass::OwnerSegmentRemoval,
                Initiator::Owner,
                Approval::NotRequired,
                Material::FullSegment,
            ),
            (
                RemovalClass::OwnerRawRelease,
                Initiator::Owner,
                Approval::NotRequired,
                Material::RawOnly,
            ),
            (
                RemovalClass::PolicyRawRelease,
                Initiator::Policy,
                Approval::Required,
                Material::RawOnly,
            ),
            (
                RemovalClass::OffloadRawRelease,
                Initiator::Offload,
                Approval::Required,
                Material::RawOnly,
            ),
        ] {
            let (initiator, approval, material) = class.axes();
            assert_eq!(
                initiator, expected_initiator,
                "{class:?} initiator mismatch"
            );
            assert_eq!(approval, expected_approval, "{class:?} approval mismatch");
            assert_eq!(material, expected_material, "{class:?} material mismatch");
        }
    }

    #[test]
    fn preflight_refuses_every_non_executable_mark_shape() {
        let required = mark(RemovalClass::PolicyRawRelease, MarkState::Marked);
        let missing = MarkId::derive(
            RemovalClass::PolicyRawRelease,
            &target("20260806", "field.audio", "070000_17"),
            &proposal("current").names,
        );
        let mut register = Register::empty();
        register.marks.insert(required.id.clone(), required.clone());

        assert!(matches!(
            preflight_register(&register, &[]),
            Err(PreflightRefusal::Empty)
        ));
        assert!(matches!(
            preflight_register(&register, &[required.id.clone(), required.id.clone()]),
            Err(PreflightRefusal::Duplicate)
        ));
        assert!(matches!(
            preflight_register(&register, std::slice::from_ref(&missing)),
            Err(PreflightRefusal::Missing { .. })
        ));
        assert_eq!(
            preflight_register(&register, std::slice::from_ref(&required.id))
                .unwrap()
                .as_slice()
                .len(),
            1
        );

        let not_required = mark(RemovalClass::OwnerRawRelease, MarkState::Marked);
        let mut not_required_register = Register::empty();
        not_required_register
            .marks
            .insert(not_required.id.clone(), not_required.clone());
        assert!(matches!(
            preflight_register(
                &not_required_register,
                std::slice::from_ref(&not_required.id)
            ),
            Err(PreflightRefusal::NotRequired { .. })
        ));

        let failed = mark(
            RemovalClass::PolicyRawRelease,
            MarkState::Failed(Failure {
                at: "first".to_owned(),
                reason: "needs recovery".to_owned(),
                staged: Some("set-aside".to_owned()),
            }),
        );
        let mut failed_register = Register::empty();
        failed_register
            .marks
            .insert(failed.id.clone(), failed.clone());
        assert!(matches!(
            preflight_register(&failed_register, std::slice::from_ref(&failed.id)),
            Err(PreflightRefusal::Failed { .. })
        ));
    }

    #[test]
    fn mark_id_matches_the_committed_golden_vector() {
        let actual = MarkId::derive(
            RemovalClass::PolicyRawRelease,
            &target("20260805", "field.audio", "070000_17"),
            &["audio.flac".to_owned()],
        );
        let expected =
            MarkId("c065c991b6204fa12bdbdde52d8bf0fdd3be3216bb1e3d57a7e4a9f778ef784c".to_owned());
        assert_eq!(actual, expected);
    }

    #[test]
    fn mark_ids_distinguish_every_field_and_boundaries() {
        let original = target("20260805", "field.audio", "070000_17");
        let names = vec!["a.flac".to_owned()];
        let id = MarkId::derive(RemovalClass::PolicyRawRelease, &original, &names);
        assert_ne!(
            id,
            MarkId::derive(RemovalClass::OwnerRawRelease, &original, &names)
        );
        assert_ne!(
            id,
            MarkId::derive(
                RemovalClass::PolicyRawRelease,
                &target("20260806", "field.audio", "070000_17"),
                &names,
            )
        );
        assert_ne!(
            id,
            MarkId::derive(
                RemovalClass::PolicyRawRelease,
                &target("20260805", "other.audio", "070000_17"),
                &names,
            )
        );
        assert_ne!(
            id,
            MarkId::derive(
                RemovalClass::PolicyRawRelease,
                &target("20260805", "field.audio", "070000_18"),
                &names,
            )
        );
        assert_ne!(
            MarkId::derive(
                RemovalClass::PolicyRawRelease,
                &target("20260805", "a", "b"),
                &names,
            ),
            MarkId::derive(
                RemovalClass::PolicyRawRelease,
                &target("20260805", "ab", ""),
                &names,
            ),
        );
    }

    #[test]
    fn a_mark_round_trips_through_serde() {
        let item = Mark {
            id: MarkId::derive(
                RemovalClass::OwnerRawRelease,
                &target("20260805", "field.audio", "070000_17"),
                &proposal("owner requested it").names,
            ),
            class: RemovalClass::OwnerRawRelease,
            target: target("20260805", "field.audio", "070000_17"),
            marked_at: "2026-08-06T12:00:00Z".to_owned(),
            proposal: proposal("owner requested it"),
            state: MarkState::Marked,
        };
        let encoded = serde_json::to_vec(&item).unwrap();
        let decoded: Mark = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(decoded, item);
    }

    #[test]
    fn register_json_is_strict_for_nested_entries() {
        let item = json!({
            "id": MarkId::derive(
                RemovalClass::PolicyRawRelease,
                &target("20260805", "field.audio", "070000_17"),
                &proposal("owner requested it").names,
            ),
            "class": "policy_raw_release",
            "target": {"day": "20260805", "stream": "field.audio", "dir": "070000_17"},
            "marked_at": "first",
            "proposal": {"bytes": 1, "reason": "current", "names": ["a.flac"], "extra": true},
            "state": "marked"
        });
        assert!(serde_json::from_value::<Mark>(item).is_err());
    }
}
