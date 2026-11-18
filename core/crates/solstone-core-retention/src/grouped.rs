// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Shared independent group decisions and advisory proposal inspection.

use jiff::Timestamp;
use jiff::civil::Date;
use solstone_core_journal_io::paths::SegmentLayout;

use crate::age::segment_age;
use crate::archive_proof::AcceptedProcessingObservation;
use crate::class::{MediaClass, classify, partition_empty_audio};
use crate::content::{ClosedHandlerSet, HandlerRegistry, JournalMedia, MediaClassifier};
use crate::eligibility::{Blocker, FoundContent, ProvenRaw, RawRelease, resolve};
use crate::policy::{Eligibility, Policy};
use crate::receipt::Target;
use crate::scan::CheckedSegment;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DecisionPurpose {
    PolicyRelease,
    OffloadProposal,
}

pub(crate) enum SideDecision {
    NoMedia,
    Kept(Eligibility),
    Unproven(Vec<Blocker>),
    Eligible {
        policy: Option<Eligibility>,
        proven: Vec<ProvenRaw>,
    },
}

#[allow(
    clippy::too_many_arguments,
    reason = "one shared decision over explicitly supplied facts"
)]
pub(crate) fn decide_side(
    side: &[FoundContent],
    registry: &dyn HandlerRegistry,
    classifier: &dyn MediaClassifier,
    policy: &Policy,
    target: &Target,
    today: Date,
    now: Timestamp,
    purpose: DecisionPurpose,
) -> SideDecision {
    if side.is_empty() {
        return SideDecision::NoMedia;
    }
    let class = classify(side, registry);
    let policy_verdict =
        if purpose == DecisionPurpose::PolicyRelease || class == MediaClass::NoDecodableAudio {
            let records = side
                .iter()
                .map(|item| item.sidecar.record.as_ref())
                .collect::<Vec<_>>();
            let verdict = policy.evaluate(
                &target.stream,
                segment_age(&target.day, &records, today, now),
                class,
            );
            if !verdict.is_eligible() {
                return SideDecision::Kept(verdict);
            }
            Some(verdict)
        } else {
            None
        };
    match resolve(
        registry,
        classifier,
        &target.day,
        &target.stream,
        &target.dir,
        side,
    ) {
        RawRelease::Held(blockers) => SideDecision::Unproven(blockers),
        RawRelease::Releasable(proven) if proven.is_empty() => SideDecision::NoMedia,
        RawRelease::Releasable(proven) => SideDecision::Eligible {
            policy: policy_verdict,
            proven,
        },
    }
}

/// Proposal evidence only. This value cannot enter a removal door or grant P.
#[derive(Clone, Debug)]
pub struct InspectionFile {
    name: String,
    bytes: u64,
    processing: AcceptedProcessingObservation,
}

impl InspectionFile {
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn bytes(&self) -> u64 {
        self.bytes
    }
    pub fn processing(&self) -> &AcceptedProcessingObservation {
        &self.processing
    }
}

#[derive(Clone, Debug)]
pub enum GroupVerdict {
    NoMedia,
    Kept(Eligibility),
    Unproven(Vec<Blocker>),
    Eligible,
}

#[derive(Clone, Debug)]
pub struct GroupInspection {
    class: MediaClass,
    members: Vec<String>,
    verdict: GroupVerdict,
    files: Vec<InspectionFile>,
}

impl GroupInspection {
    pub fn class(&self) -> MediaClass {
        self.class
    }
    pub fn members(&self) -> &[String] {
        &self.members
    }
    pub fn verdict(&self) -> &GroupVerdict {
        &self.verdict
    }
    pub fn eligible(&self) -> &[InspectionFile] {
        &self.files
    }
}

/// Advisory current structural inspection, without archive/revision/raw-identity checks.
#[derive(Clone, Debug)]
pub struct GroupedInspection {
    target: Target,
    layout: SegmentLayout,
    empty: GroupInspection,
    ordinary: GroupInspection,
}

impl GroupedInspection {
    pub fn target(&self) -> &Target {
        &self.target
    }
    pub fn layout(&self) -> SegmentLayout {
        self.layout
    }
    pub fn empty(&self) -> &GroupInspection {
        &self.empty
    }
    pub fn ordinary(&self) -> &GroupInspection {
        &self.ordinary
    }
    pub fn eligible(&self) -> impl Iterator<Item = &InspectionFile> {
        self.empty.files.iter().chain(self.ordinary.files.iter())
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "explicit observation, location, policy and time"
)]
pub fn inspect_grouped(
    observed: &CheckedSegment,
    target: &Target,
    layout: SegmentLayout,
    policy: &Policy,
    today: Date,
    now: Timestamp,
    purpose: DecisionPurpose,
) -> GroupedInspection {
    let (empty, ordinary) = partition_empty_audio(observed.found.clone(), &ClosedHandlerSet);
    let inspect = |side: &[FoundContent]| {
        let decision = decide_side(
            side,
            &ClosedHandlerSet,
            &JournalMedia,
            policy,
            target,
            today,
            now,
            purpose,
        );
        let (verdict, files) = match decision {
            SideDecision::NoMedia => (GroupVerdict::NoMedia, Vec::new()),
            SideDecision::Kept(verdict) => (GroupVerdict::Kept(verdict), Vec::new()),
            SideDecision::Unproven(blockers) => (GroupVerdict::Unproven(blockers), Vec::new()),
            SideDecision::Eligible { .. } => (
                GroupVerdict::Eligible,
                side.iter()
                    .map(|item| InspectionFile {
                        name: item.name.as_str().to_owned(),
                        bytes: item.size,
                        processing: AcceptedProcessingObservation {
                            record: item.sidecar.record.clone(),
                            has_analysis_row: item.sidecar.has_analysis_row,
                        },
                    })
                    .collect(),
            ),
        };
        GroupInspection {
            class: classify(side, &ClosedHandlerSet),
            members: side
                .iter()
                .map(|item| item.name.as_str().to_owned())
                .collect(),
            verdict,
            files,
        }
    };
    GroupedInspection {
        target: target.clone(),
        layout,
        empty: inspect(&empty),
        ordinary: inspect(&ordinary),
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    reason = "synthetic group decision fixtures"
)]
mod tests {
    use super::*;
    use crate::content::ContentName;
    use crate::eligibility::SidecarFacts;
    use crate::policy::{Days, Rule, policy_from_journal_config, policy_from_retention};
    use serde_json::json;

    fn file(name: &str, state: &str, empty: bool, backfill: bool, rows: bool) -> FoundContent {
        let mut record = json!({
            "schema": "solstone.processing.v1", "handler": "transcribe",
            "state": state, "input_size": 7, "attempted_at": "2026-10-10T00:00:00Z"
        });
        if empty {
            record["reason_code"] = json!("no_speech");
        }
        if backfill {
            record["source"] = json!("backfill");
        }
        FoundContent {
            name: ContentName::new(name).unwrap(),
            size: 7,
            sidecar: SidecarFacts {
                record: Some(record),
                has_analysis_row: rows,
            },
        }
    }
    fn inspect(
        files: Vec<FoundContent>,
        policy: &Policy,
        purpose: DecisionPurpose,
    ) -> GroupedInspection {
        let target = Target {
            day: "20261010".into(),
            stream: "_default".into(),
            dir: "prefix_120000_60_suffix".into(),
        };
        let now = "2026-10-10T01:00:00Z".parse::<Timestamp>().unwrap();
        inspect_grouped(
            &CheckedSegment {
                found: files,
                bound_entries: std::collections::BTreeMap::new(),
            },
            &target,
            SegmentLayout::Direct,
            policy,
            jiff::tz::Offset::UTC.to_datetime(now).date(),
            now,
            purpose,
        )
    }
    fn policy() -> Policy {
        policy_from_retention(json!({}).as_object().unwrap())
    }
    #[test]
    fn unselected_ordinary_sibling_blocks_whole_ordinary_group() {
        let result = inspect(
            vec![
                file("selected.flac", "analyzed", false, false, true),
                file("unselected.flac", "pending", false, false, false),
            ],
            &policy(),
            DecisionPurpose::OffloadProposal,
        );
        assert_eq!(result.eligible().count(), 0);
        assert!(matches!(
            result.ordinary().verdict(),
            GroupVerdict::Unproven(_)
        ));
        assert_eq!(result.ordinary().members().len(), 2);
    }
    #[test]
    fn eligible_empty_is_independent_of_blocked_ordinary_and_floor() {
        let mut policy = policy();
        policy.minimum_age = Days(90);
        let result = inspect(
            vec![
                file("empty.flac", "empty", true, false, false),
                file("blocked.flac", "pending", false, false, false),
            ],
            &policy,
            DecisionPurpose::OffloadProposal,
        );
        assert_eq!(
            result
                .eligible()
                .map(InspectionFile::name)
                .collect::<Vec<_>>(),
            vec!["empty.flac"]
        );
        assert!(matches!(
            result.ordinary().verdict(),
            GroupVerdict::Unproven(_)
        ));
        assert_eq!(result.target().dir, "prefix_120000_60_suffix");
        assert_eq!(result.layout(), SegmentLayout::Direct);
    }
    #[test]
    fn kept_empty_does_not_hold_proven_ordinary_and_policy_still_keeps_ordinary() {
        let mut policy = policy();
        policy.empty_audio_rule = Rule::keep();
        let files = vec![
            file("empty.flac", "empty", true, false, false),
            file("ordinary.flac", "analyzed", false, false, true),
        ];
        let result = inspect(files.clone(), &policy, DecisionPurpose::OffloadProposal);
        assert!(matches!(result.empty().verdict(), GroupVerdict::Kept(_)));
        assert_eq!(
            result
                .eligible()
                .map(InspectionFile::name)
                .collect::<Vec<_>>(),
            vec!["ordinary.flac"]
        );
        assert_eq!(
            inspect(files, &policy, DecisionPurpose::PolicyRelease)
                .eligible()
                .count(),
            0
        );
    }
    #[test]
    fn backfill_empty_stays_ordinary_and_missing_analysis_rows_refuse() {
        let result = inspect(
            vec![file("backfill.flac", "empty", true, true, false)],
            &policy(),
            DecisionPurpose::OffloadProposal,
        );
        assert_eq!(result.empty().members().len(), 0);
        assert_eq!(result.ordinary().eligible().len(), 1);
        let result = inspect(
            vec![file("analyzed.flac", "analyzed", false, false, false)],
            &policy(),
            DecisionPurpose::OffloadProposal,
        );
        assert_eq!(result.eligible().count(), 0);
        assert!(matches!(
            result.ordinary().verdict(),
            GroupVerdict::Unproven(_)
        ));
    }
    #[test]
    fn preserve_all_is_strict_for_empty_and_legacy_rows_remain_ordinary() {
        for value in [
            json!(false),
            json!("true"),
            json!(1),
            json!(null),
            json!(true),
        ] {
            let policy = policy_from_journal_config(
                json!({"transcribe":{"preserve_all":value}})
                    .as_object()
                    .unwrap(),
            );
            let result = inspect(
                vec![file("empty.flac", "empty", true, false, false)],
                &policy,
                DecisionPurpose::OffloadProposal,
            );
            assert_eq!(result.eligible().count(), usize::from(value != json!(true)));
        }
        let mut legacy = file("legacy.flac", "analyzed", false, false, true);
        legacy.sidecar.record = None;
        let result = inspect(vec![legacy], &policy(), DecisionPurpose::OffloadProposal);
        assert_eq!(result.ordinary().eligible().len(), 1);
        assert!(
            result.ordinary().eligible()[0]
                .processing()
                .record
                .is_none()
        );
    }
}
