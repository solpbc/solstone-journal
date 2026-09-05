// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! No-follow recovery admission for Cortex active-use records.

use std::ffi::OsStr;
use std::path::{Component, Path};

use serde_json::Value;

mod admission;
mod allocation;
mod catalog;
pub(crate) mod census;
mod lock;
mod namespace;
#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

pub use admission::{
    CortexAdmissionError, CortexAdmittedUse, CortexUseFileIdentity, admit_active_use,
    complete_active_use, recover_active_use,
};
#[cfg(any(test, feature = "test-hooks"))]
#[doc(hidden)]
pub use admission::{
    CortexAdmissionPrimitive, admit_active_use_with_test_timing,
    complete_active_use_with_test_timing, recover_active_use_with_test_timing,
    run_with_cortex_admission_fault,
};
pub use allocation::allocate_cortex_use_id;
pub use catalog::{
    CortexCatalogError, CortexRecoveryCandidate, CortexRecoveryCatalog, CortexRecoveryDisposition,
    CortexRecoveryTalent, build_recovery_catalog,
};
#[cfg(any(test, feature = "test-hooks"))]
#[doc(hidden)]
pub use census::census_cortex_namespace_with_test_timing;
pub use census::{
    CortexCensus, CortexCensusError, CortexCensusLeaf, CortexLifecycleProjections,
    CortexTalentCensus, census_cortex_namespace, parse_cortex_lifecycle_name,
};
#[cfg(feature = "test-hooks")]
pub use census::{
    CortexCensusPrimitive, run_with_cortex_census_barrier, run_with_cortex_census_fault,
};
#[cfg(feature = "test-hooks")]
#[doc(hidden)]
pub use lock::acquire_cortex_namespace_lock_with_test_timing;
pub use lock::{CortexNamespaceLock, CortexNamespaceLockError, acquire_cortex_namespace_lock};
pub use namespace::{
    CortexNamespaceAuthority, CortexNamespaceError, create_or_admit_cortex_namespace,
};

#[cfg(all(unix, feature = "test-hooks"))]
pub use unix::{
    CortexUseReadPrimitive, run_with_cortex_use_read_barrier, run_with_cortex_use_read_fault,
};

const MAXIMUM_FIRST_ROW_BYTES: usize = 64 * 1024;

/// The bounded recovery operation that owns the diagnostic vocabulary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CortexUseOperation {
    /// Cortex startup recovery.
    Recovery,
}

impl CortexUseOperation {
    const fn token(self) -> &'static str {
        match self {
            Self::Recovery => "cortex_recovery",
        }
    }
}

/// A closed refusal class for one nonfatal Cortex recovery candidate.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CortexUseRefusal {
    /// The first record does not describe this active use.
    InvalidRequest,
    /// The candidate leaf was not a regular file.
    CandidateNonregular,
    /// Candidate observation, opening, or reading failed.
    CandidateIo,
    /// The candidate changed while it was being read.
    CandidateIdentityChanged,
    /// The completed destination already exists.
    DestinationOccupied,
    /// The completed destination could not be observed.
    DestinationIo,
    /// A real talent directory could not be enumerated.
    TalentDirectoryRefused,
}

impl CortexUseRefusal {
    const fn token(self) -> &'static str {
        match self {
            Self::InvalidRequest => "invalid_request",
            Self::CandidateNonregular => "candidate_nonregular",
            Self::CandidateIo => "candidate_io",
            Self::CandidateIdentityChanged => "candidate_identity_changed",
            Self::DestinationOccupied => "destination_occupied",
            Self::DestinationIo => "destination_io",
            Self::TalentDirectoryRefused => "talent_directory_refused",
        }
    }
}

/// The only fatal Cortex recovery inspection failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CortexUseFatal {
    /// The `talents/` root could not be admitted or revalidated.
    RootInspectionFailed,
}

/// Opaque identity of a no-follow-admitted `talents/` root.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CortexUseRootIdentity {
    #[cfg(unix)]
    unix: (libc::dev_t, libc::ino_t),
    #[cfg(windows)]
    windows: crate::windows_identity::WindowsFileIdentity,
}

impl CortexUseFatal {
    const fn token(self) -> &'static str {
        match self {
            Self::RootInspectionFailed => "root_inspection_failed",
        }
    }
}

/// A recovery request admitted from an active record.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CortexUseRequest {
    /// The admitted active-use identity.
    pub use_id: String,
}

/// Result of reading and admitting one candidate active record.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CortexUseCandidateRead {
    /// The first row is a stable request for this talent directory and leaf.
    Accepted(CortexUseRequest),
    /// The candidate was safely refused without mutation.
    Refused(CortexUseRefusal),
}

/// Result of observing the exact completed-name destination for an admitted request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CortexUseDestinationCheck {
    /// No entry currently occupies the completed destination.
    Vacant,
    /// The destination was occupied or could not be observed.
    Refused(CortexUseRefusal),
}

/// Fixed-order counts of nonfatal recovery refusals.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CortexUseRefusalCounts {
    invalid_request: u64,
    candidate_nonregular: u64,
    candidate_io: u64,
    candidate_identity_changed: u64,
    destination_occupied: u64,
    destination_io: u64,
    talent_directory_refused: u64,
}

impl CortexUseRefusalCounts {
    /// Add one refusal to its stable diagnostic class.
    pub fn record(&mut self, refusal: CortexUseRefusal) {
        let count = match refusal {
            CortexUseRefusal::InvalidRequest => &mut self.invalid_request,
            CortexUseRefusal::CandidateNonregular => &mut self.candidate_nonregular,
            CortexUseRefusal::CandidateIo => &mut self.candidate_io,
            CortexUseRefusal::CandidateIdentityChanged => &mut self.candidate_identity_changed,
            CortexUseRefusal::DestinationOccupied => &mut self.destination_occupied,
            CortexUseRefusal::DestinationIo => &mut self.destination_io,
            CortexUseRefusal::TalentDirectoryRefused => &mut self.talent_directory_refused,
        };
        *count = count.saturating_add(1);
    }

    /// Whether every refusal class has a zero count.
    pub fn is_empty(&self) -> bool {
        self.as_ordered().iter().all(|(_, count)| *count == 0)
    }

    /// Return the count for one refusal class.
    pub fn get(&self, refusal: CortexUseRefusal) -> u64 {
        self.as_ordered()
            .into_iter()
            .find_map(|(candidate, count)| (candidate == refusal).then_some(count))
            .expect("every Cortex-use refusal has a count")
    }

    fn as_ordered(&self) -> [(CortexUseRefusal, u64); 7] {
        [
            (CortexUseRefusal::InvalidRequest, self.invalid_request),
            (
                CortexUseRefusal::CandidateNonregular,
                self.candidate_nonregular,
            ),
            (CortexUseRefusal::CandidateIo, self.candidate_io),
            (
                CortexUseRefusal::CandidateIdentityChanged,
                self.candidate_identity_changed,
            ),
            (
                CortexUseRefusal::DestinationOccupied,
                self.destination_occupied,
            ),
            (CortexUseRefusal::DestinationIo, self.destination_io),
            (
                CortexUseRefusal::TalentDirectoryRefused,
                self.talent_directory_refused,
            ),
        ]
    }
}

/// Format the one bounded aggregate diagnostic, omitting a clean scan.
pub fn format_cortex_use_summary(
    operation: CortexUseOperation,
    counts: &CortexUseRefusalCounts,
) -> Option<String> {
    (!counts.is_empty()).then(|| {
        let mut diagnostic = operation.token().to_owned();
        for (refusal, count) in counts.as_ordered() {
            if count != 0 {
                diagnostic.push(' ');
                diagnostic.push_str(refusal.token());
                diagnostic.push('=');
                diagnostic.push_str(&count.to_string());
            }
        }
        debug_assert!(diagnostic.chars().count() <= 1_000);
        diagnostic
    })
}

/// Format the one bounded fatal recovery diagnostic.
pub fn format_cortex_use_fatal(operation: CortexUseOperation, fatal: CortexUseFatal) -> String {
    let diagnostic = format!("{} {}", operation.token(), fatal.token());
    debug_assert!(diagnostic.chars().count() <= 1_000);
    diagnostic
}

/// Project a talent name into its one direct on-disk directory name.
pub fn talent_directory_name(name: &str) -> String {
    let candidate = name.replace(':', "--").replace(['/', '\\'], "-");
    if candidate.is_empty()
        || Path::new(&candidate)
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return "_invalid".to_owned();
    }
    candidate
}

/// Admit one active Cortex record by reading its bounded, identity-stable first row.
///
/// Both the expected talent-directory projection and active leaf projection are derived
/// from these two inputs so every request refusal uses this module's closed taxonomy.
pub fn read_cortex_use_request(
    talent_directory: &Path,
    active_leaf: &OsStr,
) -> CortexUseCandidateRead {
    #[cfg(unix)]
    {
        unix::read_cortex_use_request(talent_directory, active_leaf)
    }
    #[cfg(windows)]
    {
        windows::read_cortex_use_request(talent_directory, active_leaf)
    }
}

/// Admit one completed Cortex record by reading its bounded, identity-stable first row.
pub fn read_cortex_use_completed_request(
    talent_directory: &Path,
    completed_leaf: &OsStr,
) -> CortexUseCandidateRead {
    #[cfg(unix)]
    {
        unix::read_cortex_use_completed_request(talent_directory, completed_leaf)
    }
    #[cfg(windows)]
    {
        windows::read_cortex_use_completed_request(talent_directory, completed_leaf)
    }
}

/// Observe the completed-name destination immediately before a recovery mutation.
pub fn check_cortex_use_destination(
    talent_directory: &Path,
    request: &CortexUseRequest,
) -> CortexUseDestinationCheck {
    #[cfg(unix)]
    {
        unix::check_cortex_use_destination(talent_directory, request)
    }
    #[cfg(windows)]
    {
        windows::check_cortex_use_destination(talent_directory, request)
    }
}

/// Admit the `talents/` root without following links or reparse points.
pub fn inspect_cortex_use_root(root: &Path) -> Result<CortexUseRootIdentity, CortexUseFatal> {
    #[cfg(unix)]
    {
        unix::inspect_cortex_use_root(root)
    }
    #[cfg(windows)]
    {
        windows::inspect_cortex_use_root(root)
    }
}

/// Revalidate the `talents/` root against its previously admitted identity.
pub fn revalidate_cortex_use_root(
    root: &Path,
    expected: &CortexUseRootIdentity,
) -> Result<(), CortexUseFatal> {
    #[cfg(unix)]
    {
        unix::revalidate_cortex_use_root(root, expected)
    }
    #[cfg(windows)]
    {
        windows::revalidate_cortex_use_root(root, expected)
    }
}

fn expected_active_use_id(leaf: &OsStr) -> Option<&str> {
    leaf.to_str()
        .and_then(|text| text.strip_suffix(".jsonl"))
        .and_then(|stem| stem.strip_suffix("_active"))
        .filter(|stem| !stem.is_empty())
}

fn expected_completed_use_id(leaf: &OsStr) -> Option<&str> {
    leaf.to_str()
        .and_then(|text| text.strip_suffix(".jsonl"))
        .filter(|stem| !stem.is_empty())
}

/// Validate the current Cortex request row against its talent directory and use identity.
pub fn parse_cortex_use_request(
    talent_directory: &Path,
    expected_use_id: &str,
    first_row: &[u8],
) -> CortexUseCandidateRead {
    let request = match serde_json::from_slice::<Value>(first_row) {
        Ok(Value::Object(request)) => request,
        _ => return CortexUseCandidateRead::Refused(CortexUseRefusal::InvalidRequest),
    };
    if request.get("event").is_some_and(|event| event != "request") {
        return CortexUseCandidateRead::Refused(CortexUseRefusal::InvalidRequest);
    }
    let Some(use_id) = request
        .get("use_id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
    else {
        return CortexUseCandidateRead::Refused(CortexUseRefusal::InvalidRequest);
    };
    let Some(name) = request
        .get("name")
        .and_then(Value::as_str)
        .filter(|name| !name.is_empty())
    else {
        return CortexUseCandidateRead::Refused(CortexUseRefusal::InvalidRequest);
    };
    let Some(directory_name) = talent_directory.file_name().and_then(OsStr::to_str) else {
        return CortexUseCandidateRead::Refused(CortexUseRefusal::InvalidRequest);
    };
    if talent_directory_name(name) != directory_name {
        return CortexUseCandidateRead::Refused(CortexUseRefusal::InvalidRequest);
    }
    if use_id != expected_use_id {
        return CortexUseCandidateRead::Refused(CortexUseRefusal::InvalidRequest);
    }
    CortexUseCandidateRead::Accepted(CortexUseRequest {
        use_id: use_id.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn talent_directory_name_cannot_escape_a_direct_talent_directory() {
        assert_eq!(talent_directory_name("conversation"), "conversation");
        assert_eq!(talent_directory_name("app:name"), "app--name");
        assert_eq!(talent_directory_name("foo/../etc"), "foo-..-etc");
        assert_eq!(talent_directory_name(".."), "_invalid");
    }

    #[test]
    fn diagnostic_formatting_is_closed_and_clean_scans_are_silent() {
        let mut counts = CortexUseRefusalCounts::default();
        assert_eq!(
            format_cortex_use_summary(CortexUseOperation::Recovery, &counts),
            None
        );
        counts.record(CortexUseRefusal::DestinationIo);
        counts.record(CortexUseRefusal::InvalidRequest);
        assert_eq!(
            format_cortex_use_summary(CortexUseOperation::Recovery, &counts),
            Some("cortex_recovery invalid_request=1 destination_io=1".into())
        );
        assert_eq!(
            format_cortex_use_fatal(
                CortexUseOperation::Recovery,
                CortexUseFatal::RootInspectionFailed
            ),
            "cortex_recovery root_inspection_failed"
        );
    }
}
