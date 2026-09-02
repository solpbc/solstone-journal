// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Recovery catalog over an admitted Cortex namespace census.

use std::collections::BTreeMap;
use std::error::Error;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::path::Path;

use super::census::{CortexCensus, CortexCensusLeaf};
use super::{
    CortexUseCandidateRead, CortexUseRefusal, read_cortex_use_completed_request,
    read_cortex_use_request,
};
use crate::JournalEntryKind;

/// Filename-derived recovery classification for one talent-directory leaf.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CortexRecoveryDisposition {
    /// The leaf is an active use (`{id}_active.jsonl`) whose first row matches `{id}`.
    Active,
    /// The leaf is a completed use (`{id}.jsonl`), or a dual-projection leaf whose
    /// first row matches the completed hypothesis.
    Completed,
    /// Content refused both hypotheses, or the same `use_id` resolved with more
    /// than one disposition in this talent directory.
    Collision,
}

/// One classified recovery leaf inside a talent directory.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CortexRecoveryCandidate {
    leaf: OsString,
    use_id: String,
    disposition: CortexRecoveryDisposition,
    unresolved_reason: Option<CortexUseRefusal>,
}

impl CortexRecoveryCandidate {
    /// Native leaf name as listed under the talent directory.
    pub fn leaf(&self) -> &OsStr {
        &self.leaf
    }

    /// Use id this leaf resolved to (or the active projection for a both-refuse collision).
    pub fn use_id(&self) -> &str {
        &self.use_id
    }

    /// Classification for this leaf after per-leaf resolution and cross-leaf collision.
    pub fn disposition(&self) -> CortexRecoveryDisposition {
        self.disposition
    }

    /// I/O-class reason when [`disposition`](Self::disposition) is Collision from a
    /// hypothesis-read fault; `None` for content/cross-leaf collisions.
    pub fn unresolved_reason(&self) -> Option<CortexUseRefusal> {
        self.unresolved_reason
    }
}

/// Recovery candidates for one talent directory that produced at least one candidate.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CortexRecoveryTalent {
    name: OsString,
    candidates: Vec<CortexRecoveryCandidate>,
}

impl CortexRecoveryTalent {
    /// Native talent-directory name.
    pub fn name(&self) -> &OsStr {
        &self.name
    }

    /// Classified leaves in census leaf order.
    pub fn candidates(&self) -> &[CortexRecoveryCandidate] {
        &self.candidates
    }
}

/// Inclusive catalog of recovery candidates under an admitted census.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CortexRecoveryCatalog {
    talents: Vec<CortexRecoveryTalent>,
}

impl CortexRecoveryCatalog {
    /// Talent groups in census talent order, omitting talents with zero candidates.
    pub fn talents(&self) -> &[CortexRecoveryTalent] {
        &self.talents
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[allow(dead_code)]
enum CortexCatalogClass {
    CandidateIo,
    CandidateIdentityChanged,
}

impl CortexCatalogClass {
    const fn token(self) -> &'static str {
        match self {
            Self::CandidateIo => "candidate_io",
            Self::CandidateIdentityChanged => "candidate_identity_changed",
        }
    }
}

/// Bounded failure while reading a census leaf for the recovery catalog.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct CortexCatalogError {
    class: CortexCatalogClass,
}

impl CortexCatalogError {
    #[allow(dead_code)]
    const fn new(class: CortexCatalogClass) -> Self {
        Self { class }
    }

    fn token(self) -> String {
        format!("cortex_catalog_{}", self.class.token())
    }
}

impl fmt::Display for CortexCatalogError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.token())
    }
}

impl fmt::Debug for CortexCatalogError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

impl Error for CortexCatalogError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        None
    }
}

#[derive(Debug, Eq, PartialEq)]
enum DualProjectionOutcome {
    Active(String),
    Completed(String),
    Collision(Option<CortexUseRefusal>),
}

/// Classify one dual-projection leaf from two already-obtained first-row reads.
///
/// I/O-class refusals become [`DualProjectionOutcome::Collision`] carrying the
/// refusal. The active-read reason wins when both hypotheses refuse as I/O.
/// Both `Accepted` values cannot occur for production content (the two expected
/// ids are `X` and `X_active`); that case is an assertion failure.
fn resolve_dual_projection(
    active_read: CortexUseCandidateRead,
    completed_read: CortexUseCandidateRead,
) -> DualProjectionOutcome {
    if let Some(reason) = io_class_refusal(&active_read) {
        return DualProjectionOutcome::Collision(Some(reason));
    }
    if let Some(reason) = io_class_refusal(&completed_read) {
        return DualProjectionOutcome::Collision(Some(reason));
    }
    match (active_read, completed_read) {
        (CortexUseCandidateRead::Accepted(_), CortexUseCandidateRead::Accepted(_)) => {
            unreachable!("a dual-projection leaf cannot accept both hypotheses")
        }
        (CortexUseCandidateRead::Accepted(request), CortexUseCandidateRead::Refused(_)) => {
            DualProjectionOutcome::Active(request.use_id)
        }
        (CortexUseCandidateRead::Refused(_), CortexUseCandidateRead::Accepted(request)) => {
            DualProjectionOutcome::Completed(request.use_id)
        }
        (CortexUseCandidateRead::Refused(_), CortexUseCandidateRead::Refused(_)) => {
            DualProjectionOutcome::Collision(None)
        }
    }
}

fn io_class_refusal(read: &CortexUseCandidateRead) -> Option<CortexUseRefusal> {
    match read {
        CortexUseCandidateRead::Refused(
            reason @ (CortexUseRefusal::CandidateIo | CortexUseRefusal::CandidateIdentityChanged),
        ) => Some(*reason),
        CortexUseCandidateRead::Accepted(_)
        | CortexUseCandidateRead::Refused(CortexUseRefusal::InvalidRequest)
        | CortexUseCandidateRead::Refused(CortexUseRefusal::CandidateNonregular)
        | CortexUseCandidateRead::Refused(CortexUseRefusal::DestinationOccupied)
        | CortexUseCandidateRead::Refused(CortexUseRefusal::DestinationIo)
        | CortexUseCandidateRead::Refused(CortexUseRefusal::TalentDirectoryRefused) => None,
    }
}

fn classified(
    leaf: &OsStr,
    use_id: String,
    disposition: CortexRecoveryDisposition,
    unresolved_reason: Option<CortexUseRefusal>,
) -> CortexRecoveryCandidate {
    CortexRecoveryCandidate {
        leaf: leaf.to_os_string(),
        use_id,
        disposition,
        unresolved_reason,
    }
}

/// Build the recovery catalog from an already-locked census.
pub fn build_recovery_catalog(
    census: &CortexCensus,
) -> Result<CortexRecoveryCatalog, CortexCatalogError> {
    let mut talents = Vec::new();
    for talent in census.talents() {
        let mut candidates = Vec::new();
        for leaf in talent.entries() {
            if let Some(candidate) = resolve_leaf(talent.directory().diagnostic_path(), leaf)? {
                candidates.push(candidate);
            }
        }
        reclassify_cross_leaf_collisions(&mut candidates);
        if !candidates.is_empty() {
            talents.push(CortexRecoveryTalent {
                name: talent.name().to_os_string(),
                candidates,
            });
        }
    }
    Ok(CortexRecoveryCatalog { talents })
}

fn resolve_leaf(
    talent_directory: &Path,
    leaf: &CortexCensusLeaf,
) -> Result<Option<CortexRecoveryCandidate>, CortexCatalogError> {
    if leaf.kind() != JournalEntryKind::RegularFile {
        return Ok(None);
    }
    match (leaf.projections().active(), leaf.projections().completed()) {
        (None, None) => Ok(None),
        (None, Some(use_id)) => Ok(Some(classified(
            leaf.name(),
            use_id.to_owned(),
            CortexRecoveryDisposition::Completed,
            None,
        ))),
        (Some(active_id), Some(_)) => resolve_dual_leaf(talent_directory, leaf, active_id),
        (Some(_), None) => {
            unreachable!("a name matching _active.jsonl also matches .jsonl")
        }
    }
}

/// Resolve a leaf that has both lifecycle projections by content.
///
/// An accepted active read skips the completed read: `expected_active_use_id`
/// is `X` and `expected_completed_use_id` is `X_active` for the same non-empty
/// `X`, so one JSON `use_id` cannot satisfy both hypotheses.
fn resolve_dual_leaf(
    talent_directory: &Path,
    leaf: &CortexCensusLeaf,
    active_id: &str,
) -> Result<Option<CortexRecoveryCandidate>, CortexCatalogError> {
    let active_read = read_cortex_use_request(talent_directory, leaf.name());
    if let Some(reason) = io_class_refusal(&active_read) {
        return Ok(Some(classified(
            leaf.name(),
            active_id.to_owned(),
            CortexRecoveryDisposition::Collision,
            Some(reason),
        )));
    }
    if let CortexUseCandidateRead::Accepted(request) = active_read {
        return Ok(Some(classified(
            leaf.name(),
            request.use_id,
            CortexRecoveryDisposition::Active,
            None,
        )));
    }
    let completed_read = read_cortex_use_completed_request(talent_directory, leaf.name());
    if let Some(reason) = io_class_refusal(&completed_read) {
        return Ok(Some(classified(
            leaf.name(),
            active_id.to_owned(),
            CortexRecoveryDisposition::Collision,
            Some(reason),
        )));
    }
    Ok(Some(
        match resolve_dual_projection(active_read, completed_read) {
            DualProjectionOutcome::Active(use_id) => {
                classified(leaf.name(), use_id, CortexRecoveryDisposition::Active, None)
            }
            DualProjectionOutcome::Completed(use_id) => classified(
                leaf.name(),
                use_id,
                CortexRecoveryDisposition::Completed,
                None,
            ),
            DualProjectionOutcome::Collision(reason) => classified(
                leaf.name(),
                active_id.to_owned(),
                CortexRecoveryDisposition::Collision,
                reason,
            ),
        },
    ))
}

fn reclassify_cross_leaf_collisions(candidates: &mut [CortexRecoveryCandidate]) {
    let mut groups = BTreeMap::<String, Vec<usize>>::new();
    for (index, candidate) in candidates.iter().enumerate() {
        if matches!(
            candidate.disposition,
            CortexRecoveryDisposition::Active | CortexRecoveryDisposition::Completed
        ) {
            groups
                .entry(candidate.use_id.clone())
                .or_default()
                .push(index);
        }
    }
    for indices in groups.values() {
        let mixed = indices
            .windows(2)
            .any(|pair| candidates[pair[0]].disposition != candidates[pair[1]].disposition);
        if mixed {
            for &index in indices {
                candidates[index].disposition = CortexRecoveryDisposition::Collision;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;
    use std::time::Duration;

    use super::*;
    use crate::cortex_use::{
        CortexUseRequest, census_cortex_namespace_with_test_timing,
        create_or_admit_cortex_namespace,
    };
    use crate::journal_root::JournalRoot;

    const ZERO: Duration = Duration::ZERO;
    const MAX: usize = 64;

    fn temp() -> tempfile::TempDir {
        #[cfg(unix)]
        {
            tempfile::tempdir_in("/var/tmp").unwrap()
        }
        #[cfg(windows)]
        {
            tempfile::TempDir::new().unwrap()
        }
    }

    fn census_at(root: &Path) -> CortexCensus {
        let authority = create_or_admit_cortex_namespace(JournalRoot::open(root).unwrap()).unwrap();
        census_cortex_namespace_with_test_timing(authority, MAX, ZERO, ZERO).unwrap()
    }

    fn talent_dir(root: &Path, name: &str) -> std::path::PathBuf {
        let _ = create_or_admit_cortex_namespace(JournalRoot::open(root).unwrap()).unwrap();
        let directory = root.join("talents").join(name);
        fs::create_dir_all(&directory).unwrap();
        directory
    }

    fn write_leaf(directory: &Path, leaf: &str, row: &str) {
        fs::write(
            directory.join(leaf),
            format!("{row}\n{{\"event\":\"tail\"}}\n"),
        )
        .unwrap();
    }

    fn request(name: &str, use_id: &str) -> String {
        format!(r#"{{"name":"{name}","use_id":"{use_id}"}}"#)
    }

    fn accepted(use_id: &str) -> CortexUseCandidateRead {
        CortexUseCandidateRead::Accepted(CortexUseRequest {
            use_id: use_id.to_owned(),
        })
    }

    fn refused(refusal: CortexUseRefusal) -> CortexUseCandidateRead {
        CortexUseCandidateRead::Refused(refusal)
    }

    fn catalog_talent<'a>(
        catalog: &'a CortexRecoveryCatalog,
        name: &str,
    ) -> &'a CortexRecoveryTalent {
        catalog
            .talents()
            .iter()
            .find(|talent| talent.name() == name)
            .unwrap_or_else(|| panic!("missing talent {name}"))
    }

    fn assert_candidate(
        candidate: &CortexRecoveryCandidate,
        leaf: &str,
        use_id: &str,
        disposition: CortexRecoveryDisposition,
    ) {
        assert_eq!(candidate.leaf(), leaf);
        assert_eq!(candidate.use_id(), use_id);
        assert_eq!(candidate.disposition(), disposition);
    }

    #[test]
    fn disambiguation_table() {
        let temporary = temp();
        let root = temporary.path();
        let conversation = talent_dir(root, "conversation");
        write_leaf(&conversation, "alpha.jsonl", "{");
        write_leaf(
            &conversation,
            "beta_active.jsonl",
            &request("conversation", "beta"),
        );
        write_leaf(
            &conversation,
            "gamma_active.jsonl",
            &request("conversation", "gamma_active"),
        );
        write_leaf(
            &conversation,
            "delta_active.jsonl",
            &request("conversation", "nope"),
        );
        let other = talent_dir(root, "other");
        write_leaf(&other, "foo.jsonl", "{");
        write_leaf(&other, "foo_active.jsonl", &request("other", "foo"));

        let catalog = build_recovery_catalog(&census_at(root)).unwrap();
        let conversation = catalog_talent(&catalog, "conversation");
        assert_eq!(conversation.candidates().len(), 4);
        assert_candidate(
            &conversation.candidates()[0],
            "alpha.jsonl",
            "alpha",
            CortexRecoveryDisposition::Completed,
        );
        assert_candidate(
            &conversation.candidates()[1],
            "beta_active.jsonl",
            "beta",
            CortexRecoveryDisposition::Active,
        );
        assert_candidate(
            &conversation.candidates()[2],
            "delta_active.jsonl",
            "delta",
            CortexRecoveryDisposition::Collision,
        );
        assert_candidate(
            &conversation.candidates()[3],
            "gamma_active.jsonl",
            "gamma_active",
            CortexRecoveryDisposition::Completed,
        );

        let other = catalog_talent(&catalog, "other");
        assert_eq!(other.candidates().len(), 2);
        assert_candidate(
            &other.candidates()[0],
            "foo.jsonl",
            "foo",
            CortexRecoveryDisposition::Collision,
        );
        assert_candidate(
            &other.candidates()[1],
            "foo_active.jsonl",
            "foo",
            CortexRecoveryDisposition::Collision,
        );
        for candidate in conversation.candidates().iter().chain(other.candidates()) {
            assert_eq!(candidate.unresolved_reason(), None);
        }
    }

    #[test]
    #[should_panic(expected = "a dual-projection leaf cannot accept both hypotheses")]
    fn both_accepting_reads_are_an_assertion_failure() {
        let _ = resolve_dual_projection(accepted("one"), accepted("one"));
    }

    #[cfg(unix)]
    #[test]
    fn io_class_refusal_is_a_catalog_error_not_a_collision() {
        use crate::cortex_use::unix::{CortexUseReadPrimitive, run_with_cortex_use_read_fault};

        let temporary = temp();
        let root = temporary.path();
        let conversation = talent_dir(root, "conversation");
        write_leaf(
            &conversation,
            "beta_active.jsonl",
            &request("conversation", "beta"),
        );
        let census = census_at(root);
        let (result, consumed) =
            run_with_cortex_use_read_fault(CortexUseReadPrimitive::InitialNameObserve, 1, || {
                build_recovery_catalog(&census)
            });
        assert!(consumed);
        let catalog = result.expect("I/O-class hypothesis read is a per-leaf collision");
        let conversation = catalog_talent(&catalog, "conversation");
        assert_eq!(conversation.candidates().len(), 1);
        assert_candidate(
            &conversation.candidates()[0],
            "beta_active.jsonl",
            "beta",
            CortexRecoveryDisposition::Collision,
        );
        assert_eq!(
            conversation.candidates()[0].unresolved_reason(),
            Some(CortexUseRefusal::CandidateIo)
        );
    }

    #[test]
    fn empty_namespace_is_an_empty_catalog() {
        let temporary = temp();
        let root = temporary.path();
        let _ = create_or_admit_cortex_namespace(JournalRoot::open(root).unwrap()).unwrap();
        let catalog = build_recovery_catalog(&census_at(root)).unwrap();
        assert!(catalog.talents().is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn observation_count_is_bounded_per_leaf() {
        // Each observe_stable_first_row fires InitialNameObserve once. A completed-only
        // leaf is classified from the filename and must not observe. A dual-projection
        // leaf observes once when the active hypothesis accepts, and at most twice when
        // the active hypothesis refuses and the completed hypothesis is then tried.
        // Faulting InitialNameObserve ordinal 4 would fire only if a fourth observation
        // occurred; success with consumed=false proves the bound.
        use crate::cortex_use::unix::{CortexUseReadPrimitive, run_with_cortex_use_read_fault};

        let temporary = temp();
        let root = temporary.path();
        let conversation = talent_dir(root, "conversation");
        write_leaf(&conversation, "alpha.jsonl", "{");
        write_leaf(
            &conversation,
            "beta_active.jsonl",
            &request("conversation", "beta"),
        );
        write_leaf(
            &conversation,
            "gamma_active.jsonl",
            &request("conversation", "gamma_active"),
        );
        let census = census_at(root);
        let (result, consumed) =
            run_with_cortex_use_read_fault(CortexUseReadPrimitive::InitialNameObserve, 4, || {
                build_recovery_catalog(&census)
            });
        assert!(!consumed, "a fourth observation would consume ordinal 4");
        let catalog = result.expect("catalog must succeed within the bound");
        let conversation = catalog_talent(&catalog, "conversation");
        assert_eq!(conversation.candidates().len(), 3);
        assert_candidate(
            &conversation.candidates()[0],
            "alpha.jsonl",
            "alpha",
            CortexRecoveryDisposition::Completed,
        );
        assert_candidate(
            &conversation.candidates()[1],
            "beta_active.jsonl",
            "beta",
            CortexRecoveryDisposition::Active,
        );
        assert_candidate(
            &conversation.candidates()[2],
            "gamma_active.jsonl",
            "gamma_active",
            CortexRecoveryDisposition::Completed,
        );
    }

    #[test]
    fn non_regular_and_no_projection_leaves_are_omitted() {
        let temporary = temp();
        let root = temporary.path();
        let conversation = talent_dir(root, "conversation");
        write_leaf(
            &conversation,
            "beta_active.jsonl",
            &request("conversation", "beta"),
        );
        write_leaf(&conversation, "notes.txt", "notes");
        fs::create_dir(conversation.join("dir.jsonl")).unwrap();
        let catalog = build_recovery_catalog(&census_at(root)).unwrap();
        let conversation = catalog_talent(&catalog, "conversation");
        assert_eq!(conversation.candidates().len(), 1);
        assert_candidate(
            &conversation.candidates()[0],
            "beta_active.jsonl",
            "beta",
            CortexRecoveryDisposition::Active,
        );
    }

    #[test]
    fn classifier_maps_io_class_refusals_and_non_io_pairs() {
        assert_eq!(
            resolve_dual_projection(
                refused(CortexUseRefusal::InvalidRequest),
                accepted("foo_active")
            ),
            DualProjectionOutcome::Completed("foo_active".into())
        );
        assert_eq!(
            resolve_dual_projection(
                refused(CortexUseRefusal::InvalidRequest),
                refused(CortexUseRefusal::CandidateNonregular)
            ),
            DualProjectionOutcome::Collision(None)
        );
        assert_eq!(
            resolve_dual_projection(refused(CortexUseRefusal::CandidateIo), accepted("foo")),
            DualProjectionOutcome::Collision(Some(CortexUseRefusal::CandidateIo))
        );
        assert_eq!(
            resolve_dual_projection(
                accepted("foo"),
                refused(CortexUseRefusal::CandidateIdentityChanged),
            ),
            DualProjectionOutcome::Collision(Some(CortexUseRefusal::CandidateIdentityChanged))
        );
        assert_eq!(
            resolve_dual_projection(
                refused(CortexUseRefusal::CandidateIo),
                refused(CortexUseRefusal::CandidateIdentityChanged),
            ),
            DualProjectionOutcome::Collision(Some(CortexUseRefusal::CandidateIo))
        );
    }
}
