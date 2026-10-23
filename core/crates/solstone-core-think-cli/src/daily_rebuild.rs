// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Continuation of the daily portion of one sealed owner rebuild.
use sha2::{Digest, Sha256};
use solstone_core_journal_io::{DailyUnitIdentity, DailyUnitRecord};
use solstone_core_system::work_intent::{self, DailyTarget};

use crate::context::ThinkContext;

pub(crate) fn digest(record: &Option<DailyUnitRecord>) -> Result<Option<String>, String> {
    record
        .as_ref()
        .map(|record| {
            serde_json::to_vec(record)
                .map(|bytes| format!("{:x}", Sha256::digest(bytes)))
                .map_err(|e| e.to_string())
        })
        .transpose()
}

pub(crate) fn initialize(
    context: &ThinkContext,
    identities: Vec<(DailyUnitIdentity, String)>,
) -> Result<(), String> {
    let Some(lease) = &context.rebuild else {
        return Ok(());
    };
    let targets = identities
        .into_iter()
        .map(|(identity, contract)| {
            let record =
                solstone_core_journal_io::load_daily_unit_record(&context.journal, &identity)
                    .map_err(|e| e.to_string())?;
            Ok(DailyTarget {
                baseline: digest(&record)?,
                identity,
                contract,
                token: None,
                dispatch_started: false,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    work_intent::update_rebuild(
        &context.journal,
        &context.day,
        &lease.id,
        &|| context.rebuild_admission_checkpoint(),
        |p| {
            if p.segments.is_none() {
                return Err("segment work has no verified seal".into());
            }
            match &p.daily {
                Some(saved)
                    if saved
                        .iter()
                        .map(|t| (&t.identity, &t.contract))
                        .ne(targets.iter().map(|t| (&t.identity, &t.contract))) =>
                {
                    Err("requested daily applicability changed".into())
                }
                Some(_) => Ok(()),
                None => {
                    p.daily = Some(targets);
                    Ok(())
                }
            }
        },
    )
}

pub(crate) struct Plan {
    pub token: String,
    pub baseline: Option<String>,
    pub first_reset: bool,
    pub dispatch_started: bool,
}

pub(crate) fn plan(
    context: &ThinkContext,
    identity: &DailyUnitIdentity,
    record: &Option<DailyUnitRecord>,
    contract: &str,
) -> Result<Option<Plan>, String> {
    let Some(lease) = &context.rebuild else {
        return Ok(None);
    };
    let next = solstone_core_journal_io::cortex_use::allocate_cortex_use_id(
        &context.journal,
        context.event_now_ms(),
    )
    .map_err(|e| e.to_string())?
    .to_string();
    let target = work_intent::update_rebuild(
        &context.journal,
        &context.day,
        &lease.id,
        &|| context.rebuild_admission_checkpoint(),
        |p| {
            let target = p
                .daily
                .as_mut()
                .and_then(|targets| targets.iter_mut().find(|t| &t.identity == identity))
                .ok_or("unit is outside the requested daily scope")?;
            if target.contract != contract {
                return Err("requested daily contract changed".into());
            }
            if target.token.is_none() {
                if digest(record)? != target.baseline {
                    return Err("daily baseline changed before reservation".into());
                }
                target.token = Some(next);
            }
            Ok(target.clone())
        },
    )?;
    let token = target.token.expect("planned token");
    let first_reset = record.as_ref().and_then(|r| r.lock_token.as_deref()) != Some(token.as_str());
    if first_reset {
        if target.dispatch_started {
            return Err("daily reset authority is uncertain after possible dispatch".into());
        }
        if lease.resumed {
            let unit = solstone_core_system::work_retry::Unit::Daily {
                day: context.day.clone(),
                talent: identity.name.clone(),
                facet: identity.facet.clone(),
            };
            if solstone_core_system::work_retry::read(&context.journal, &unit.id()?)?
                .is_some_and(|record| record.held)
            {
                return Err("daily target retains an owner retry hold".into());
            }
        }
        if digest(record)? != target.baseline {
            return Err("daily target was replaced outside this request".into());
        }
        if solstone_core_cortex_client::use_file_status(&context.journal.join("talents"), &token)
            .map_err(|e| e.to_string())?
            != solstone_core_cortex_client::UseFileStatus::NotFound
        {
            return Err("daily reset has an unexpected prior execution".into());
        }
    }
    Ok(Some(Plan {
        token,
        baseline: target.baseline,
        first_reset,
        dispatch_started: target.dispatch_started,
    }))
}

pub(crate) fn accepted(context: &ThinkContext, record: &DailyUnitRecord) -> Result<bool, String> {
    Ok(record.status.is_terminal_success()
        && record.is_reusable_for(&record.evidence_revision, &record.contract_digest)
        && solstone_core_journal_io::accepted_daily_artifacts_valid(&context.journal, record)
            .map_err(|e| e.to_string())?)
}

pub(crate) fn continuation_use(
    context: &ThinkContext,
    record: &DailyUnitRecord,
    plan: &Plan,
    evidence: &str,
    contract: &str,
) -> Result<String, String> {
    if record.identity.name.is_empty()
        || record.lock_token.as_deref() != Some(&plan.token)
        || record.evidence_revision != evidence
        || record.contract_digest != contract
        || record.has_uncommitted_started_receipt()
        || record.status != solstone_core_journal_io::DailyUnitStatus::Unfinished
    {
        return Err("daily continuation has changed or held authority".into());
    }
    let packet = record
        .frozen_packet
        .as_ref()
        .ok_or("daily continuation has no packet")?;
    if record.packet_digest.as_deref()
        != Some(solstone_core_talent_runtime::daily_prepare::packet_digest(packet).as_str())
    {
        return Err("daily continuation packet changed".into());
    }
    let prior = record
        .use_id
        .as_deref()
        .ok_or("daily continuation has no execution identity")?;
    let status =
        solstone_core_cortex_client::use_file_status(&context.journal.join("talents"), prior)
            .map_err(|e| e.to_string())?;
    if status == solstone_core_cortex_client::UseFileStatus::NotFound && !plan.dispatch_started {
        return Ok(prior.into()); // The reset was durable but Cortex had not claimed it.
    }
    if status == solstone_core_cortex_client::UseFileStatus::Running {
        return Err("daily execution is still active or uncertain".into());
    }
    if record.generated_result.is_none() {
        return Err("daily model outcome is uncertain or held; owner review required".into());
    }
    solstone_core_journal_io::cortex_use::allocate_cortex_use_id(
        &context.journal,
        context.event_now_ms(),
    )
    .map(|id| id.to_string())
    .map_err(|e| e.to_string())
}

pub(crate) fn mark_dispatch(
    context: &ThinkContext,
    identity: &DailyUnitIdentity,
    token: &str,
) -> std::io::Result<()> {
    let Some(lease) = &context.rebuild else {
        return Ok(());
    };
    work_intent::update_rebuild(
        &context.journal,
        &context.day,
        &lease.id,
        &|| context.rebuild_admission_checkpoint(),
        |p| {
            let target = p
                .daily
                .as_mut()
                .and_then(|targets| {
                    targets
                        .iter_mut()
                        .find(|t| &t.identity == identity && t.token.as_deref() == Some(token))
                })
                .ok_or("daily dispatch lost request authority")?;
            target.dispatch_started = true;
            Ok(())
        },
    )
    .map_err(std::io::Error::other)
}

pub(crate) fn reserve_continuation(
    context: &ThinkContext,
    identity: &DailyUnitIdentity,
    token: &str,
    use_id: &str,
    expected: &DailyUnitRecord,
) -> std::io::Result<()> {
    let failure = |e: String| std::io::Error::other(e);
    let _execution = solstone_core_journal_io::claim_daily_execution(&context.journal, identity)
        .map_err(|e| failure(e.to_string()))?;
    solstone_core_journal_io::with_daily_unit_authority(&context.journal, identity, |authority| {
        authority.require_token(token)?;
        if authority.record() != Some(expected) {
            return Err(solstone_core_journal_io::DailyUnitError::Malformed(
                "daily continuation changed before admission".into(),
            ));
        }
        context
            .rebuild_admission_checkpoint()
            .map_err(solstone_core_journal_io::DailyUnitError::Malformed)?;
        // Preserve the original publication token, frozen packet, result, plan,
        // receipts, failure counts and consumed permissions.
        authority
            .record_mut()
            .as_mut()
            .expect("token requires record")
            .use_id = Some(use_id.into());
        authority.checkpoint()
    })
    .map_err(|e| failure(e.to_string()))
}

pub(crate) fn require_complete(context: &ThinkContext) -> Result<(), String> {
    let Some(lease) = &context.rebuild else {
        return Ok(());
    };
    let intents = work_intent::read(&context.journal)?;
    let targets = intents
        .get(&context.day)
        .filter(|i| i.id == lease.id)
        .and_then(|i| i.rebuild.as_ref())
        .and_then(|p| p.daily.as_ref())
        .ok_or("requested daily scope is missing")?;
    for target in targets {
        let record =
            solstone_core_journal_io::load_daily_unit_record(&context.journal, &target.identity)
                .map_err(|e| e.to_string())?
                .ok_or("requested daily target has no receipt")?;
        if record.lock_token != target.token
            || target.token.is_none()
            || !accepted(context, &record)?
        {
            return Err("requested daily target has no accepted completion".into());
        }
    }
    context.checkpoint()
}

#[cfg(test)]
mod tests {
    use super::*;
    use solstone_core_journal_io::{
        DailyUnitStatus, load_daily_unit_record, save_daily_unit_record,
    };
    use std::sync::Arc;

    fn init(context: &ThinkContext, ids: Vec<DailyUnitIdentity>) -> Result<(), String> {
        initialize(
            context,
            ids.into_iter().map(|id| (id, "C".into())).collect(),
        )
    }
    fn planned(
        context: &ThinkContext,
        identity: &DailyUnitIdentity,
        record: &Option<DailyUnitRecord>,
    ) -> Result<Option<Plan>, String> {
        plan(context, identity, record, "C")
    }

    fn context(root: &std::path::Path) -> ThinkContext {
        let day = "20260101";
        std::fs::create_dir_all(root.join("chronicle").join(day)).unwrap();
        let request = work_intent::register(root, day, true).unwrap();
        let lease = work_intent::begin_rebuild(
            root,
            day,
            &request.id,
            &solstone_core_journal_io::JournalRoot::open(root)
                .unwrap()
                .identity()
                .observation_id(),
        )
        .unwrap();
        let mut context = ThinkContext::new(
            root,
            day.into(),
            root.join("chronicle").join(day),
            1_700_000_000_000,
        )
        .unwrap();
        context.rebuild = Some(Arc::new(lease));
        let receipts_digest =
            crate::daily_lifecycle::rebuild_segment_receipts(&context, None).unwrap();
        work_intent::update_rebuild(root, day, &request.id, &|| Ok(()), |p| {
            p.segments = Some(work_intent::SegmentSeal {
                generation: 0,
                raw_fingerprint: solstone_core_system::catchup::read_raw_input_fingerprint(
                    root, day,
                )
                .unwrap(),
                receipts_digest,
            });
            Ok(())
        })
        .unwrap();
        context
    }

    #[test]
    fn reset_identity_distinguishes_started_a_from_untouched_old_current_b() {
        let root = tempfile::tempdir().unwrap();
        let context = context(root.path());
        let a = DailyUnitIdentity::new(&context.day, "morning_briefing", None);
        let b = DailyUnitIdentity::new(
            &context.day,
            "entities:entities_review",
            Some("work".into()),
        );
        for identity in [&a, &b] {
            let mut old = DailyUnitRecord::new(identity.clone(), "old-E", "C");
            old.status = DailyUnitStatus::Committed;
            old.lock_token = Some("old".into());
            save_daily_unit_record(root.path(), &old).unwrap();
        }
        init(&context, vec![a.clone(), b.clone()]).unwrap();
        let original_a = load_daily_unit_record(root.path(), &a).unwrap();
        let first_a = planned(&context, &a, &original_a).unwrap().unwrap();
        assert!(first_a.first_reset);
        let mut reset_a = DailyUnitRecord::new(a.clone(), "new-E", "C");
        reset_a.lock_token = Some(first_a.token.clone());
        save_daily_unit_record(root.path(), &reset_a).unwrap();
        let continued_a = planned(&context, &a, &Some(reset_a.clone()))
            .unwrap()
            .unwrap();
        assert!(!continued_a.first_reset);
        assert_eq!(continued_a.token, first_a.token);
        let old_b = load_daily_unit_record(root.path(), &b).unwrap();
        let untouched_b = planned(&context, &b, &old_b).unwrap().unwrap();
        assert!(
            untouched_b.first_reset,
            "old-current B still owes its requested new attempt"
        );
        assert_ne!(untouched_b.token, "old");
        assert!(plan(&context, &b, &old_b, "changed-contract").is_err());
        assert!(
            require_complete(&context).is_err(),
            "progress and current-looking status are not owner proof"
        );
        reset_a.lock_token = Some("unrelated".into());
        assert!(planned(&context, &a, &Some(reset_a)).is_err());
        assert!(
            init(&context, vec![a]).is_err(),
            "applicability cannot shrink after a loss"
        );
    }

    #[test]
    fn rebuild_continuation_does_not_replay_global_timing_maintenance() {
        let root = tempfile::tempdir().unwrap();
        let mut context = context(root.path());
        let lease = context.rebuild.take().unwrap();
        let id = lease.id.clone();
        drop(lease);
        context.rebuild = Some(Arc::new(
            work_intent::begin_rebuild(
                root.path(),
                &context.day,
                &id,
                &solstone_core_journal_io::JournalRoot::open(root.path())
                    .unwrap()
                    .identity()
                    .observation_id(),
            )
            .unwrap(),
        ));
        context.talent_root = root.path().join("fixture-talents");
        context.apps_root = root.path().join("fixture-apps");
        std::fs::create_dir_all(&context.talent_root).unwrap();
        std::fs::create_dir_all(&context.apps_root).unwrap();
        std::fs::write(context.talent_root.join("daily_schedule.md"),
            "{\n\"type\":\"generate\",\"max_output_tokens\":1024,\"schedule\":\"daily\",\"priority\":1,\"output\":\"json\",\"hook\":{\"pre\":\"daily_schedule\",\"post\":\"daily_schedule\"}\n}\nChoose a daily processing time.").unwrap();
        let loaded = solstone_core_talent_config::load_talent_configs(
            &context.talent_root,
            &context.apps_root,
            None,
            solstone_core_talent_config::TalentFilter {
                r#type: None,
                schedule: Some("daily"),
                include_disabled: false,
            },
        )
        .unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].key, "daily_schedule");
        let identity = DailyUnitIdentity::new(&context.day, "daily_schedule", None);
        let record = DailyUnitRecord::new(identity.clone(), "possible-send", "C");
        save_daily_unit_record(root.path(), &record).unwrap();
        let mut log = crate::run_log::RunLogWriter::open(root.path(), &context.day, "daily");
        crate::daily::run(&context, &mut log, None, true, 1).unwrap();
        assert_eq!(
            load_daily_unit_record(root.path(), &identity).unwrap(),
            Some(record)
        );
        require_complete(&context).unwrap();
    }

    #[test]
    fn request_authority_survives_cross_day_activity_but_not_stop_or_input_drift() {
        let root = tempfile::tempdir().unwrap();
        let context = context(root.path());
        let activity = crate::segment::activity_context(&context, "20251231").unwrap();
        let digest = crate::daily_lifecycle::rebuild_segment_receipts(&context, None).unwrap();
        let lease = context.rebuild.as_ref().unwrap();
        work_intent::update_rebuild(root.path(), &lease.day, &lease.id, &|| Ok(()), |p| {
            p.segments.as_mut().unwrap().receipts_digest = digest;
            Ok(())
        })
        .unwrap();
        assert!(activity.checkpoint().is_ok());
        assert!(activity.rebuild_admission_checkpoint().is_ok());
        let identity = DailyUnitIdentity::new(&context.day, "morning_briefing", None);
        init(&context, vec![identity.clone()]).unwrap();
        solstone_core_journal_io::bump_stream_marker(root.path(), &context.day).unwrap();
        assert!(planned(&context, &identity, &None).is_err());
        work_intent::stop_rebuilds(root.path()).unwrap();
        assert!(activity.checkpoint().is_err());
    }

    #[test]
    fn accepted_owner_publication_is_authority_when_execution_progress_was_not_written() {
        use solstone_core_generate::{
            ClientError, GenerateExecutor, GenerateRequest, GenerateResponse, GeneratedResponse,
        };
        use solstone_core_talent_runtime::{
            ExecutionContext, RuntimeOutcome, prepare::RuntimePaths,
        };
        struct Count(std::sync::atomic::AtomicUsize);
        impl GenerateExecutor for Count {
            fn execute(&self, _: &GenerateRequest) -> Result<GenerateResponse, ClientError> {
                self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(GenerateResponse::Generated(Box::new(GeneratedResponse {
                    id: None,
                    text: "A retained source result".into(),
                    model: "test".into(),
                    usage: serde_json::json!({"input_tokens":1,"output_tokens":1}),
                    finish_reason: "stop".into(),
                    thinking: None,
                    schema_validation: None,
                    input_budget: None,
                    request_budget: None,
                    inference: None,
                    hints_applied: Vec::new(),
                })))
            }
        }
        let root = tempfile::tempdir().unwrap();
        let mut context = context(root.path());
        let identity = DailyUnitIdentity::new(&context.day, "morning_briefing", None);
        let b = DailyUnitIdentity::new(&context.day, "morning_briefing", Some("work".into()));
        let relative = format!("chronicle/{}/talents/morning_briefing.md", context.day);
        let b_relative = format!("chronicle/{}/talents/work/briefing.md", context.day);
        let generator = Count(std::sync::atomic::AtomicUsize::new(0));
        let paths = RuntimePaths {
            talent_root: context.talent_root.clone(),
            apps_root: context.apps_root.clone(),
            templates_dir: context.talent_root.join("templates"),
        };
        let day = context.day.clone();
        let publish = |identity: &DailyUnitIdentity, token: &str, relative: &str| {
            let before = root
                .path()
                .join(relative)
                .exists()
                .then(|| std::fs::read(root.path().join(relative)).unwrap());
            let packet = serde_json::json!({
            "version":1,"prepared":{"name":"morning_briefing","config":{
                "day":day,"type":"generate","max_output_tokens":1024,"prompt":"frozen source evidence",
                "facet":identity.facet,
                "model":"test","provider":"test","hook":{"pre":"morning_briefing"},
                "output_path":root.path().join(relative),"_daily_artifact_before":{relative:before}}},
            "hook":"morning_briefing",
            "state":{"MorningBriefing":{"values":{},"activities":[],"initial_gaps":[],"counts":{},"generated":"","model":""}}
            });
            let mut record = DailyUnitRecord::new(identity.clone(), "E", "C");
            record.lock_token = Some(token.into());
            record.use_id = Some(token.into());
            record.packet_digest = Some(
                solstone_core_talent_runtime::daily_prepare::packet_digest(&packet),
            );
            record.frozen_packet = Some(packet);
            save_daily_unit_record(root.path(), &record).unwrap();
            let request =
                serde_json::json!({"name":"morning_briefing","day":day,"facet":identity.facet,
                "lock_token":token,"evidence_revision":"E","contract_digest":"C"})
                .as_object()
                .unwrap()
                .clone();
            let outcome = solstone_core_talent_runtime::execute_request(
                request,
                &paths,
                &ExecutionContext {
                    journal: root.path().into(),
                },
                &generator,
                &mut Vec::new(),
                &solstone_core_generate::Cancellation::never(),
            );
            assert!(
                matches!(outcome, RuntimeOutcome::Finished { .. }),
                "{outcome:?}"
            );
        };
        // B genuinely has an accepted old-current publication before this request.
        publish(&b, "old-b", &b_relative);
        let old_b = load_daily_unit_record(root.path(), &b).unwrap().unwrap();
        assert!(accepted(&context, &old_b).unwrap());
        init(&context, vec![identity.clone(), b.clone()]).unwrap();
        let first = planned(&context, &identity, &None).unwrap().unwrap();
        mark_dispatch(&context, &identity, &first.token).unwrap();
        publish(&identity, &first.token, &relative);
        let completed = load_daily_unit_record(root.path(), &identity)
            .unwrap()
            .unwrap();
        let bytes = std::fs::read(root.path().join(&relative)).unwrap();
        assert!(accepted(&context, &completed).unwrap());
        // No successful Cortex completion log or request-completion bit exists.
        // Executor loss after A's actual publication and before untouched B's reset.
        let lease = context.rebuild.take().unwrap();
        let request_id = lease.id.clone();
        drop(lease);
        context.rebuild = Some(Arc::new(
            work_intent::begin_rebuild(
                root.path(),
                &context.day,
                &request_id,
                &solstone_core_journal_io::JournalRoot::open(root.path())
                    .unwrap()
                    .identity()
                    .observation_id(),
            )
            .unwrap(),
        ));
        assert!(context.rebuild.as_ref().unwrap().resumed);
        init(&context, vec![identity.clone(), b.clone()]).unwrap();
        let resumed = planned(&context, &identity, &Some(completed.clone()))
            .unwrap()
            .unwrap();
        assert!(!resumed.first_reset);
        assert!(
            require_complete(&context).is_err(),
            "old-current B still owes this rebuild"
        );
        assert_eq!(generator.0.load(std::sync::atomic::Ordering::SeqCst), 2);
        assert_eq!(
            load_daily_unit_record(root.path(), &identity).unwrap(),
            Some(completed.clone())
        );
        assert_eq!(std::fs::read(root.path().join(&relative)).unwrap(), bytes);
        let next_b = planned(&context, &b, &Some(old_b)).unwrap().unwrap();
        assert!(next_b.first_reset);
        assert_ne!(next_b.token, "old-b");
        mark_dispatch(&context, &b, &next_b.token).unwrap();
        publish(&b, &next_b.token, &b_relative);
        require_complete(&context).unwrap();
        assert_eq!(generator.0.load(std::sync::atomic::Ordering::SeqCst), 3);
        assert_eq!(
            load_daily_unit_record(root.path(), &identity).unwrap(),
            Some(completed.clone())
        );
        std::fs::write(root.path().join(relative), "changed after publication").unwrap();
        assert!(!accepted(&context, &completed).unwrap());
        assert!(require_complete(&context).is_err());
    }

    #[test]
    fn only_proved_no_send_or_retained_result_can_continue_without_reset() {
        let root = tempfile::tempdir().unwrap();
        let context = context(root.path());
        let identity = DailyUnitIdentity::new(&context.day, "morning_briefing", None);
        init(&context, vec![identity.clone()]).unwrap();
        let first_plan = planned(&context, &identity, &None).unwrap().unwrap();
        let mut record = DailyUnitRecord::new(identity.clone(), "E", "C");
        record.lock_token = Some(first_plan.token.clone());
        record.use_id = Some(first_plan.token.clone());
        let packet = serde_json::json!({"frozen":"exact"});
        record.packet_digest = Some(solstone_core_talent_runtime::daily_prepare::packet_digest(
            &packet,
        ));
        record.frozen_packet = Some(packet);
        save_daily_unit_record(root.path(), &record).unwrap();
        let before_send = planned(&context, &identity, &Some(record.clone()))
            .unwrap()
            .unwrap();
        assert_eq!(
            continuation_use(&context, &record, &before_send, "E", "C").unwrap(),
            first_plan.token
        );
        mark_dispatch(&context, &identity, &first_plan.token).unwrap();
        assert!(
            planned(&context, &identity, &None).is_err(),
            "a restored baseline after possible send cannot regain first-reset authority"
        );
        let uncertain = planned(&context, &identity, &Some(record.clone()))
            .unwrap()
            .unwrap();
        assert!(
            continuation_use(&context, &record, &uncertain, "E", "C").is_err(),
            "missing log after possible send is not no-send proof"
        );
        record.generated_result =
            Some(serde_json::json!({"response":"retained", "output":"retained"}));
        save_daily_unit_record(root.path(), &record).unwrap();
        let fresh = continuation_use(&context, &record, &uncertain, "E", "C").unwrap();
        assert_ne!(fresh, first_plan.token);
        reserve_continuation(&context, &identity, &first_plan.token, &fresh, &record).unwrap();
        let after = load_daily_unit_record(root.path(), &identity)
            .unwrap()
            .unwrap();
        assert_eq!(after.lock_token, record.lock_token);
        assert_eq!(after.generated_result, record.generated_result);
        assert_eq!(after.frozen_packet, record.frozen_packet);
        assert_eq!(after.receipts, record.receipts);
        assert_eq!(after.failure_count, record.failure_count);
        assert_eq!(after.use_id.as_deref(), Some(fresh.as_str()));
        assert!(
            reserve_continuation(&context, &identity, &first_plan.token, "another", &record)
                .is_err()
        );
        assert!(continuation_use(&context, &after, &uncertain, "changed-E", "C").is_err());
    }
}
