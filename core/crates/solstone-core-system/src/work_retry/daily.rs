// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use super::Unit;
use solstone_core_journal_io::{DailyUnitIdentity, DailyUnitStatus};
use std::path::Path;

pub const INPUT_CONTRACT: &str = "solstone.retry-input.v1";

pub fn identity(unit: &Unit) -> Result<DailyUnitIdentity, String> {
    unit.validate()?;
    match unit {
        Unit::Daily { day, talent, facet } => {
            Ok(DailyUnitIdentity::new(day, talent, facet.clone()))
        }
        _ => Err("not daily work".into()),
    }
}

pub fn evidence(
    journal: &Path,
    unit: &Unit,
) -> Result<solstone_core_indexer::daily_evidence::DailyEvidence, String> {
    let (talents, apps) = crate::daily_coverage::package_roots()?;
    evidence_with_roots(journal, unit, &talents, &apps)
}

pub fn evidence_with_roots(
    journal: &Path,
    unit: &Unit,
    talents: &Path,
    apps: &Path,
) -> Result<solstone_core_indexer::daily_evidence::DailyEvidence, String> {
    let id = identity(unit)?;
    let configs = crate::daily_coverage::daily_configs(journal, talents, apps)?;
    let config = configs
        .iter()
        .find(|c| c.key == id.name)
        .ok_or("daily talent is no longer enabled")?;
    let today = crate::daily_coverage::local_day(journal, chrono::Utc::now());
    solstone_core_indexer::daily_evidence::compute_daily_evidence(
        journal,
        if id.name == "daily_schedule" {
            &today
        } else {
            &id.day
        },
        &config.key,
        &config.metadata,
        &config.body,
        id.facet.as_deref(),
        None,
    )
}

/// Existing daily records already retain capped state beyond log retention.
/// Project the input under a fixed contract so upgrades grant no permission.
pub fn observe(journal: &Path, unit: &Unit) -> Result<Option<String>, String> {
    observe_checked(journal, unit, &|| Ok(()))
}

pub fn observe_checked(
    journal: &Path,
    unit: &Unit,
    checkpoint: &dyn Fn() -> Result<(), String>,
) -> Result<Option<String>, String> {
    observe_using_checked(journal, unit, || evidence(journal, unit), checkpoint)
}

/// Observe the same durable retry contract using the embedded host's packaged resources.
pub fn observe_with_roots_checked(
    journal: &Path,
    unit: &Unit,
    talents: &Path,
    apps: &Path,
    checkpoint: &dyn Fn() -> Result<(), String>,
) -> Result<Option<String>, String> {
    observe_using_checked(
        journal,
        unit,
        || evidence_with_roots(journal, unit, talents, apps),
        checkpoint,
    )
}

#[cfg(test)]
fn observe_using(
    journal: &Path,
    unit: &Unit,
    read_evidence: impl FnOnce() -> Result<solstone_core_indexer::daily_evidence::DailyEvidence, String>,
) -> Result<Option<String>, String> {
    observe_using_checked(journal, unit, read_evidence, &|| Ok(()))
}

fn observe_using_checked(
    journal: &Path,
    unit: &Unit,
    read_evidence: impl FnOnce() -> Result<solstone_core_indexer::daily_evidence::DailyEvidence, String>,
    checkpoint: &dyn Fn() -> Result<(), String>,
) -> Result<Option<String>, String> {
    checkpoint()?;
    let id = identity(unit)?;
    let Some(observed) = solstone_core_journal_io::load_daily_unit_record(journal, &id)
        .map_err(|e| e.to_string())?
    else {
        return Ok(None);
    };
    if observed.status != DailyUnitStatus::Capped
        && super::read(journal, &unit.id()?)?.is_none_or(|record| !record.held)
    {
        return Ok(None);
    }
    let _claim =
        solstone_core_journal_io::claim_daily_execution(journal, &id).map_err(|e| e.to_string())?;
    checkpoint()?;
    let Some(record) = solstone_core_journal_io::load_daily_unit_record(journal, &identity(unit)?)
        .map_err(|e| e.to_string())?
    else {
        return Ok(None);
    };
    let held = super::read(journal, &unit.id()?)?.filter(|r| r.held);
    let evidence = read_evidence()?;
    if record.status.is_terminal_success()
        || crate::daily_coverage::accepted_reuse(
            journal,
            &record,
            &evidence,
            crate::daily_coverage::local_now(journal, chrono::Utc::now()),
        )
        .is_some()
    {
        if held.is_some() {
            super::reconcile_accepted_checked(journal, unit, checkpoint)?;
        }
        return Ok(None);
    }
    if record.has_uncommitted_started_receipt()
        || (record.status != DailyUnitStatus::Capped && held.is_none())
    {
        return Ok(None);
    }
    let current = evidence.retry_revision_under(INPUT_CONTRACT);
    let baseline = if record.identity.name == "daily_schedule"
        || evidence.revision_under(&record.contract_digest) == record.evidence_revision
    {
        current.clone()
    } else {
        record.evidence_revision.clone()
    };
    let reason = held
        .as_ref()
        .map(|r| r.reason.as_str())
        .or(record.reason_code.as_deref())
        .unwrap_or("daily_failed");
    super::observe_held_checked(journal, unit, reason, &baseline, checkpoint)?;
    Ok(Some(current))
}

/// An explicit unit request also covers a failed unit before its ordinary cap,
/// and an entity review whose conflict a fresh preparation can clear. Other
/// conflicts, accepted output and uncertain owner actions retain their guards.
pub fn request(journal: &Path, unit: &Unit) -> Result<String, String> {
    request_using(journal, unit, || evidence(journal, unit))
}

fn request_using(
    journal: &Path,
    unit: &Unit,
    read_evidence: impl FnOnce() -> Result<solstone_core_indexer::daily_evidence::DailyEvidence, String>,
) -> Result<String, String> {
    let record = solstone_core_journal_io::load_daily_unit_record(journal, &identity(unit)?)
        .map_err(|e| e.to_string())?
        .ok_or("daily unit is missing")?;
    let retryable = matches!(
        record.status,
        DailyUnitStatus::Failed | DailyUnitStatus::Capped
    ) || crate::daily_coverage::review_conflict_reprepares(&record);
    if !retryable || record.has_uncommitted_started_receipt() {
        return Err("this daily unit cannot be retried without resolving its current state".into());
    }
    let evidence = read_evidence()?;
    if crate::daily_coverage::accepted_reuse(
        journal,
        &record,
        &evidence,
        crate::daily_coverage::local_now(journal, chrono::Utc::now()),
    )
    .is_some()
    {
        return Err("this daily output is already accepted".into());
    }
    super::observe_held(
        journal,
        unit,
        record.reason_code.as_deref().unwrap_or("daily_failed"),
        &evidence.retry_revision_under(INPUT_CONTRACT),
    )?;
    super::request(journal, unit)
}

#[cfg(test)]
mod tests {
    use super::*;
    use solstone_core_journal_io::{AcceptedDailyResult, DailyUnitRecord, save_daily_unit_record};

    #[test]
    fn maintenance_has_one_identity_and_uses_todays_evidence() {
        let root = tempfile::tempdir().unwrap();
        let talents = root.path().join("talents");
        let apps = root.path().join("apps");
        std::fs::create_dir_all(&talents).unwrap();
        std::fs::create_dir_all(&apps).unwrap();
        std::fs::write(talents.join("daily_schedule.md"), "{\n\"type\":\"generate\",\"max_output_tokens\":1024,\"schedule\":\"daily\",\"priority\":1,\"output\":\"json\",\"hook\":{\"pre\":\"daily_schedule\",\"post\":\"daily_schedule\"}\n}\nMaintain schedule.").unwrap();
        let unit = Unit::Daily {
            day: "19990101".into(),
            talent: "daily_schedule".into(),
            facet: None,
        };
        let later = Unit::Daily {
            day: "20260101".into(),
            talent: "daily_schedule".into(),
            facet: None,
        };
        assert_eq!(unit.id().unwrap(), later.id().unwrap());
        let evidence = evidence_with_roots(root.path(), &unit, &talents, &apps).unwrap();
        let mut record = DailyUnitRecord::new(
            identity(&unit).unwrap(),
            &evidence.revision,
            &evidence.contract,
        );
        record.status = DailyUnitStatus::Capped;
        record.reason_code = Some("schema_invalid".into());
        save_daily_unit_record(root.path(), &record).unwrap();
        let input = observe_using(root.path(), &unit, || {
            evidence_with_roots(root.path(), &unit, &talents, &apps)
        })
        .unwrap()
        .unwrap();
        super::super::request(root.path(), &unit).unwrap();
        let grant = super::super::read(root.path(), &later.id().unwrap())
            .unwrap()
            .unwrap()
            .permission(&input, &[])
            .unwrap();
        super::super::consume(root.path(), &later, &grant, &grant.reason, &input, &[]).unwrap();
        assert!(
            super::super::read(root.path(), &unit.id().unwrap())
                .unwrap()
                .unwrap()
                .permission(&input, &[])
                .is_none()
        );
    }

    #[test]
    fn accepted_frozen_briefing_reconciles_hold_without_permission() {
        let root = tempfile::tempdir().unwrap();
        let talents = root.path().join("talents");
        let apps = root.path().join("apps");
        std::fs::create_dir_all(&talents).unwrap();
        std::fs::create_dir_all(&apps).unwrap();
        std::fs::write(talents.join("morning_briefing.md"), "{\n\"type\":\"generate\",\"max_output_tokens\":1024,\"schedule\":\"daily\",\"priority\":1,\"output\":\"md\",\"hook\":{\"pre\":\"morning_briefing\"}\n}\nA morning briefing.").unwrap();
        let unit = Unit::Daily {
            day: "19990101".into(),
            talent: "morning_briefing".into(),
            facet: None,
        };
        let evidence = evidence_with_roots(root.path(), &unit, &talents, &apps).unwrap();
        let mut record = DailyUnitRecord::new(
            identity(&unit).unwrap(),
            &evidence.revision,
            &evidence.contract,
        );
        record.status = DailyUnitStatus::Capped;
        record.accepted = Some(AcceptedDailyResult {
            evidence_revision: "earlier input".into(),
            contract_digest: "earlier contract".into(),
            status: DailyUnitStatus::CommittedNoOutput,
            packet_digest: Some("a".repeat(64)),
            generated_result: Some(serde_json::json!({"response":"done","output":"done"})),
            receipts: vec![],
            committed_at_ms: 1,
        });
        save_daily_unit_record(root.path(), &record).unwrap();
        super::super::observe_held(root.path(), &unit, "schema_invalid", "old").unwrap();
        super::super::request(root.path(), &unit).unwrap();
        assert!(
            observe_using(root.path(), &unit, || Ok(evidence))
                .unwrap()
                .is_none()
        );
        let ledger = super::super::read(root.path(), &unit.id().unwrap())
            .unwrap()
            .unwrap();
        assert!(!ledger.held);
        assert!(
            ledger.owner.is_some(),
            "reconciliation cannot erase a newer unconsumed request"
        );
        assert!(
            ledger.permission("changed", &[]).is_none(),
            "accepted output is not eligible work"
        );
    }

    /// An entity review that stopped on a conflict after committing some of its
    /// changes, as its record reads once the automatic retry is spent.
    fn stopped_review(
        root: &Path,
        kind: Option<&str>,
        receipts: &[(&str, &str)],
    ) -> (Unit, solstone_core_indexer::daily_evidence::DailyEvidence) {
        let talents = root.join("talents");
        let apps = root.join("apps");
        std::fs::create_dir_all(&talents).unwrap();
        std::fs::create_dir_all(apps.join("entities/talent")).unwrap();
        std::fs::write(apps.join("entities/talent/entities_review.md"), "{\n\"type\":\"generate\",\"max_output_tokens\":1024,\"schedule\":\"daily\",\"priority\":56,\"output\":\"json\",\"multi_facet\":true,\"hook\":{\"pre\":\"entities:entities_review\",\"post\":\"entities:entities_review\"}\n}\nReview entities.").unwrap();
        let unit = Unit::Daily {
            day: "19990101".into(),
            talent: "entities:entities_review".into(),
            facet: Some("work".into()),
        };
        let evidence = evidence_with_roots(root, &unit, &talents, &apps).unwrap();
        let mut record = DailyUnitRecord::new(
            identity(&unit).unwrap(),
            &evidence.revision,
            &evidence.contract,
        );
        record.status = DailyUnitStatus::Conflicting;
        record.reason_code = Some("daily_owner_conflict".into());
        record.owner_conflict_kind = kind.map(str::to_owned);
        record.failure_count = 2;
        record.receipts = receipts
            .iter()
            .map(|(action, state)| {
                serde_json::json!({"kind":"owner_action","action_id":action,"token":"t","state":state})
            })
            .collect();
        save_daily_unit_record(root, &record).unwrap();
        (unit, evidence)
    }

    #[test]
    fn an_owner_request_retries_a_review_stopped_before_its_conflicting_write() {
        let root = tempfile::tempdir().unwrap();
        let (unit, evidence) = stopped_review(
            root.path(),
            Some("alias_claimed"),
            &[("0:a", "committed"), ("1:b", "committed")],
        );
        let input = evidence.retry_revision_under(INPUT_CONTRACT);
        request_using(root.path(), &unit, || Ok(evidence.clone())).unwrap();
        let ledger = super::super::read(root.path(), &unit.id().unwrap())
            .unwrap()
            .unwrap();
        assert!(ledger.held);
        assert_eq!(ledger.reason, "daily_owner_conflict");
        let grant = ledger.permission(&input, &[]).unwrap();
        assert!(grant.cause.starts_with("owner:"), "{grant:?}");
    }

    #[test]
    fn a_review_conflict_that_may_have_started_a_write_still_refuses() {
        for (kind, receipts) in [
            // A change that started and never confirmed may be half applied.
            (
                Some("alias_claimed"),
                &[("0:a", "committed"), ("1:b", "started")][..],
            ),
            // A conflict outside the known pre-write kinds keeps its guard.
            (None, &[("0:a", "committed")][..]),
        ] {
            let root = tempfile::tempdir().unwrap();
            let (unit, evidence) = stopped_review(root.path(), kind, receipts);
            let error = request_using(root.path(), &unit, || Ok(evidence)).unwrap_err();
            assert_eq!(
                error,
                "this daily unit cannot be retried without resolving its current state"
            );
            assert!(
                super::super::read(root.path(), &unit.id().unwrap())
                    .unwrap()
                    .is_none()
            );
        }
    }
}
