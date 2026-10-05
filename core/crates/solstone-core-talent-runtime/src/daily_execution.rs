// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Execute the packet reserved by daily admission and resume its publication.

use std::io::Write;

use serde_json::{Map, Value, json};
use solstone_core_generate::OneShotClient;
use solstone_core_journal_io::{
    AcceptedDailyResult, DailyUnitAuthority, DailyUnitIdentity, DailyUnitRecord, DailyUnitStatus,
    with_daily_unit_authority,
};

use crate::contract::{CommitDisposition, CommitPlan, PrePostState, StageSpec};
use crate::writers::PreparedDailyPublication;
use crate::{ExecutionContext, PreparedTalent, RuntimeOutcome, StageError};

fn failure(name: &str, error: impl std::fmt::Display) -> StageError {
    StageError::new("write", "daily_publication", name, error.to_string())
}

fn same_kind_family(
    prev_kind: Option<&str>,
    prev_detail: Option<&str>,
    next_kind: Option<&str>,
) -> bool {
    if prev_kind == next_kind {
        return true;
    }
    match (prev_kind, next_kind) {
        (
            Some("merge_proposal_preparation" | "merge_proposals_changed"),
            Some("merge_proposal_preparation" | "merge_proposals_changed"),
        ) => true,
        (None, Some("calendar_changed")) => {
            prev_detail.is_some_and(|d| d.contains("calendar changed after prompt preparation"))
        }
        _ => false,
    }
}

fn apply_stage_failure(record: &mut DailyUnitRecord, error: &StageError) {
    let prev_kind = record.owner_conflict_kind.as_deref();
    let prev_detail = record.error_detail.as_deref();
    let next_kind = error.owner_conflict_kind();
    let reason = error.reason_code();

    let kind_changed = !same_kind_family(prev_kind, prev_detail, next_kind);
    let reason_changed = record.reason_code.as_deref() != Some(reason);

    record.status = if error.phase == "conflict" {
        DailyUnitStatus::Conflicting
    } else {
        DailyUnitStatus::Failed
    };
    record.error_detail = Some(error.to_string());
    if kind_changed || reason_changed {
        record.failure_count = 0;
    }
    record.reason_code = Some(reason.to_owned());
    record.owner_conflict_kind = next_kind.map(str::to_owned);
    if error.phase == "conflict" {
        record.failure_count = record.failure_count.saturating_add(1);
    }
}

fn retained_response(value: &Value) -> Result<crate::GeneratedTalentResponse, String> {
    let response = value
        .get("response")
        .and_then(Value::as_str)
        .ok_or_else(|| "retained daily response is malformed".to_owned())?;
    let optional = |key| {
        value
            .get(key)
            .filter(|v| !v.is_null())
            .cloned()
            .map(Box::new)
    };
    Ok((response.to_owned(), optional("usage"), optional("degraded")))
}

fn finished(record: &DailyUnitRecord) -> Result<RuntimeOutcome, String> {
    let accepted = record
        .accepted
        .as_ref()
        .ok_or("missing accepted daily result")?;
    let result = accepted
        .generated_result
        .as_ref()
        .ok_or("missing accepted response")?;
    let (response, usage, degraded) = retained_response(result)?;
    let output = result
        .get("output")
        .and_then(Value::as_str)
        .unwrap_or(&response)
        .to_owned();
    Ok(RuntimeOutcome::Finished {
        output,
        disposition: if accepted.status == DailyUnitStatus::CommittedNoOutput {
            CommitDisposition::CommittedNoOutput
        } else {
            CommitDisposition::Written
        },
        usage,
        degraded,
        output_changed: None,
    })
}

pub(crate) fn execute(
    request: Map<String, Value>,
    context: &ExecutionContext,
    generate: &OneShotClient,
    writer: &mut impl Write,
) -> RuntimeOutcome {
    let name = request
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("daily");
    let Some(token) = request
        .get("lock_token")
        .and_then(Value::as_str)
        .filter(|v| !v.is_empty())
    else {
        return RuntimeOutcome::StageFailed(failure(name, "missing daily attempt token"));
    };
    let Some(day) = request.get("day").and_then(Value::as_str) else {
        return RuntimeOutcome::StageFailed(failure(name, "missing daily attempt day"));
    };
    if day.len() != 8 || chrono::NaiveDate::parse_from_str(day, "%Y%m%d").is_err() {
        return RuntimeOutcome::StageFailed(failure(name, "invalid daily attempt day"));
    }
    let facet = request
        .get("facet")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let identity = DailyUnitIdentity::new(day, name, facet);
    let record = match with_daily_unit_authority(&context.journal, &identity, |authority| {
        authority.require_token(token)?;
        Ok(authority.record().expect("token requires record").clone())
    }) {
        Ok(record) => record,
        Err(error) => return RuntimeOutcome::StageFailed(failure(name, error)),
    };
    for (key, expected) in [
        ("evidence_revision", record.evidence_revision.as_str()),
        ("contract_digest", record.contract_digest.as_str()),
    ] {
        if request
            .get(key)
            .is_some_and(|v| v.as_str() != Some(expected))
        {
            return RuntimeOutcome::StageFailed(failure(name, format!("daily {key} mismatch")));
        }
    }
    let Some(packet) = record.frozen_packet.as_ref() else {
        return RuntimeOutcome::StageFailed(failure(name, "daily attempt has no frozen packet"));
    };
    if record.packet_digest.as_deref() != Some(crate::daily_prepare::packet_digest(packet).as_str())
    {
        return RuntimeOutcome::StageFailed(failure(name, "daily packet digest mismatch"));
    }
    let (mut prepared, stage) = match crate::daily_prepare::thaw(packet) {
        Ok(value) => value,
        Err(error) => return RuntimeOutcome::StageFailed(failure(name, error)),
    };
    if prepared.name != name {
        return RuntimeOutcome::StageFailed(failure(name, "daily packet talent mismatch"));
    }
    if (name != "daily_schedule" && prepared.config.get("day").and_then(Value::as_str) != Some(day))
        || prepared.config.get("facet").and_then(Value::as_str)
            != request.get("facet").and_then(Value::as_str)
    {
        return RuntimeOutcome::StageFailed(failure(name, "daily packet scope mismatch"));
    }
    prepared
        .config
        .insert("lock_token".to_owned(), json!(token));
    if let Some(use_id) = record.use_id.as_ref() {
        prepared.config.insert("use_id".to_owned(), json!(use_id));
    }
    crate::emit_start(writer, &prepared);
    if record.status.is_terminal_success()
        && record.is_reusable_for(&record.evidence_revision, &record.contract_digest)
    {
        return match solstone_core_journal_io::accepted_daily_artifacts_valid(
            &context.journal,
            &record,
        ) {
            Ok(true) => finished(&record)
                .unwrap_or_else(|error| RuntimeOutcome::StageFailed(failure(name, error))),
            Ok(false) => RuntimeOutcome::StageFailed(failure(
                name,
                "accepted daily artifact is missing or changed",
            )),
            Err(error) => RuntimeOutcome::StageFailed(failure(name, error)),
        };
    }
    let skip = prepared
        .config
        .get("skip_reason")
        .and_then(Value::as_str)
        .is_some();
    let generated = if let Some(value) = record.generated_result.as_ref() {
        match retained_response(value) {
            Ok(value) => value,
            Err(error) => return RuntimeOutcome::StageFailed(failure(name, error)),
        }
    } else if skip {
        (String::new(), None, None)
    } else {
        let talent_type = prepared
            .config
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("generate");
        if talent_type != "generate" && !talent_type.is_empty() {
            return RuntimeOutcome::StageFailed(failure(
                name,
                format!("unsupported talent type: {talent_type}"),
            ));
        }
        match crate::generate_response(&mut prepared, context, generate, writer) {
            Ok(value) => value,
            Err(outcome) => return outcome,
        }
    };
    // Model execution holds no publication lock. The check below is repeated
    // while holding the authority through every actual owner write and receipt.
    let result = with_daily_unit_authority(&context.journal, &identity, |authority| {
        authority.require_token(token)?;
        let usage = generated.1.clone();
        let degraded = generated.2.clone();
        let outcome = publish(
            authority,
            token,
            &prepared,
            stage.as_ref(),
            context,
            generated,
            skip,
            writer,
        );
        Ok(match outcome {
            Ok(outcome) => Ok(outcome),
            Err(mut error) => {
                let record = authority.record_mut().as_mut().expect("authority");
                // Rejected model output has no actions to recover. Keep valid
                // results for owner preparation/I/O recovery, but allow the
                // normal bounded retry to replace an unusable model response.
                if matches!(error.phase, "parse" | "commit") && record.action_plan.is_none() {
                    record.generated_result = None;
                    error.stage = "daily_output_validation";
                }
                // Every day's schedule writes into the same later-dated
                // calendars, so its conflicts are mostly another day's schedule
                // rather than the owner. A facet's merge proposals are likewise
                // rewritten by other days' reviews of that facet, so a proposal
                // that changed after preparation is stale input, not an owner
                // decision (accepted or dismissed proposals never conflict).
                // Reusing the prompt and response can only conflict again, so the
                // next attempt prepares a fresh prompt. A saved plan is cleared
                // only when no owner-action receipts have landed.
                let stale_preparation = match error.owner_conflict_kind() {
                    Some(
                        "merge_proposal_preparation"
                        | "merge_proposals_changed"
                        | "calendar_changed",
                    ) => true,
                    None => {
                        name == "schedule"
                            && error
                                .detail
                                .contains("calendar changed after prompt preparation")
                    }
                    _ => false,
                };
                let receipt_free = !record
                    .receipts
                    .iter()
                    .any(|r| r.get("kind").and_then(Value::as_str) == Some("owner_action"));
                if error.phase == "conflict" && stale_preparation && receipt_free {
                    record.generated_result = None;
                    record.frozen_packet = None;
                    record.packet_digest = None;
                    record.action_plan = None;
                }

                apply_stage_failure(record, &error);
                authority.checkpoint()?;
                if error.usage.is_none() {
                    error.usage = usage;
                }
                if error.degraded.is_none() {
                    error.degraded = degraded;
                }
                Err(error)
            }
        })
    });
    match result {
        Ok(Ok(outcome)) => outcome,
        Ok(Err(error)) => RuntimeOutcome::StageFailed(error),
        Err(error) => RuntimeOutcome::StageFailed(failure(name, error)),
    }
}

#[allow(clippy::too_many_arguments)]
fn publish(
    authority: &mut DailyUnitAuthority,
    token: &str,
    prepared: &PreparedTalent,
    stage: Option<&(&'static StageSpec, PrePostState)>,
    context: &ExecutionContext,
    generated: crate::GeneratedTalentResponse,
    skip: bool,
    writer: &mut (impl Write + ?Sized),
) -> Result<RuntimeOutcome, StageError> {
    let name = &prepared.name;
    let record = authority.record().expect("checked publication authority");
    if record.status.is_terminal_success()
        && record.is_reusable_for(&record.evidence_revision, &record.contract_digest)
    {
        if !solstone_core_journal_io::accepted_daily_artifacts_valid(&context.journal, record)
            .map_err(|e| failure(name, e))?
        {
            return Err(failure(
                name,
                "accepted daily artifact is missing or changed",
            ));
        }
        return finished(record).map_err(|e| failure(name, e));
    }
    let (response, usage, degraded) = if let Some(value) = record.generated_result.as_ref() {
        retained_response(value).map_err(|e| failure(name, e))?
    } else {
        let value = json!({"response":generated.0,"usage":generated.1,"degraded":generated.2});
        authority
            .record_mut()
            .as_mut()
            .expect("authority")
            .generated_result = Some(value);
        authority.checkpoint().map_err(|e| failure(name, e))?;
        generated
    };
    let publication: PreparedDailyPublication = if let Some(plan) =
        authority.record().and_then(|r| r.action_plan.as_ref())
    {
        serde_json::from_value(plan.clone()).map_err(|e| failure(name, e))?
    } else {
        let publication = if skip {
            crate::writers::prepare_daily_publication(CommitPlan::NoOutput, prepared, context)?
        } else if let Some((spec, state)) = stage {
            if let Some(commit) = spec.commit {
                let parsed = (commit.parse)(&response, prepared, state)?;
                let plan = (commit.commit)(parsed, prepared, state)?;
                crate::writers::prepare_daily_publication(plan, prepared, context)?
            } else {
                crate::writers::prepare_daily_output(prepared, &response, context)?
            }
        } else {
            crate::writers::prepare_daily_output(prepared, &response, context)?
        };
        authority
            .record_mut()
            .as_mut()
            .expect("authority")
            .action_plan = Some(serde_json::to_value(&publication).map_err(|e| failure(name, e))?);
        authority.checkpoint().map_err(|e| failure(name, e))?;
        publication
    };
    let mut changed_paths = Vec::new();
    let publication_result = crate::writers::publish_daily_publication(
        authority,
        token,
        &publication,
        context,
        &mut changed_paths,
    );
    // This branch is the configured writer, as in preparation above. Derive
    // ownership from the stage contract rather than adding it to retained plans.
    let configured_output = !skip && stage.is_none_or(|(spec, _)| spec.commit.is_none());
    let output_changed = if configured_output {
        for rel in &changed_paths {
            let full = context.journal.join(rel);
            let attempt = solstone_core_indexer_store::attempt_saved_publication(
                &context.journal,
                &full,
                |j, p| {
                    solstone_core_indexer_store::scan::rescan_file(j, p).map_err(|e| e.to_string())
                },
            );
            crate::emit_index_attempt(writer, &attempt);
        }
        Some(!changed_paths.is_empty())
    } else {
        None
    };
    let disposition = publication_result?;
    let output = if let Some((spec, state)) = stage {
        if let Some(override_output) = spec.output_override {
            override_output(&response, prepared, state)?
        } else {
            response.clone()
        }
    } else {
        response.clone()
    };
    let record = authority.record_mut().as_mut().expect("authority");
    let status = if publication.no_output {
        DailyUnitStatus::CommittedNoOutput
    } else {
        DailyUnitStatus::Committed
    };
    let mut receipts = record.receipts.clone();
    receipts.extend(crate::writers::required_artifact_receipts(&publication));
    let retained = json!({"response":response,"usage":usage,"degraded":degraded,"output":output});
    record.generated_result = Some(retained.clone());
    record.status = status;
    record.reason_code = None;
    record.error_detail = None;
    record.updated_at_ms = chrono::Utc::now().timestamp_millis();
    record.accepted = Some(AcceptedDailyResult {
        evidence_revision: record.evidence_revision.clone(),
        contract_digest: record.contract_digest.clone(),
        status,
        packet_digest: record.packet_digest.clone(),
        generated_result: Some(retained),
        receipts,
        committed_at_ms: record.updated_at_ms,
    });
    authority.checkpoint().map_err(|e| failure(name, e))?;
    Ok(RuntimeOutcome::Finished {
        output,
        disposition,
        usage,
        degraded,
        output_changed,
    })
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use solstone_core_journal_io::{load_daily_unit_record, save_daily_unit_record};
    use std::fs;
    use std::path::Path;
    #[cfg(all(test, feature = "full-tests"))]
    use std::time::{Duration, Instant};

    #[cfg(all(test, feature = "full-tests"))]
    #[test]
    fn saved_daily_output_is_indexed_before_later_publication_or_override_error() {
        fn fail_override(
            _: &str,
            prepared: &PreparedTalent,
            _: &PrePostState,
        ) -> Result<String, StageError> {
            Err(StageError::new(
                "output_override",
                "test",
                &prepared.name,
                "later override failed",
            ))
        }
        static FAILING_OVERRIDE: StageSpec = StageSpec {
            stage: crate::contract::StageId::Documents,
            gate: None,
            build: None,
            prompt_override: None,
            commit: None,
            unavailable_commit: None,
            unavailable_before_generate: None,
            writes_as_intent: None,
            output_override: Some(fail_override),
        };
        for mode in ["later_action", "override", "override_index_error"] {
            let root = tempfile::tempdir().unwrap();
            let context = ExecutionContext {
                journal: root.path().to_path_buf(),
            };
            let relative = "chronicle/20260101/talents/plain.md";
            let path = context.journal.join(relative);
            let identity = DailyUnitIdentity::new("20260101", "plain", None);
            let prepared = PreparedTalent {
                name: "plain".into(),
                config: json!({"day":"20260101", "output_path":path})
                    .as_object()
                    .unwrap()
                    .clone(),
            };
            let mut actions = vec![json!({"owner":"output", "path":relative,
                "facet_identity":null, "before":null, "after":b"saved daily searchable text".to_vec()})];
            if mode == "later_action" {
                let blocked = "chronicle/20260101/talents/blocked.md";
                fs::create_dir_all(context.journal.join(blocked)).unwrap();
                actions.push(json!({"owner":"output", "path":blocked,
                    "facet_identity":null, "before":null, "after":b"blocked output".to_vec()}));
            }
            if mode == "override_index_error" {
                fs::write(context.journal.join("indexer"), b"not a directory").unwrap();
            }
            let stage = (mode != "later_action").then_some((&FAILING_OVERRIDE, PrePostState::None));
            let mut events = Vec::new();
            with_daily_unit_authority(&context.journal, &identity, |authority| {
                let mut record = DailyUnitRecord::new(identity.clone(), "E", "C");
                record.lock_token = Some("attempt".into());
                record.generated_result =
                    Some(json!({"response":"retained response", "usage":null, "degraded":null}));
                // The pre-existing retained plan shape carries no derived-index ownership flag.
                record.action_plan = Some(json!({"actions":actions, "no_output":false}));
                *authority.record_mut() = Some(record);
                authority.checkpoint()?;
                let result = publish(
                    authority,
                    "attempt",
                    &prepared,
                    stage.as_ref(),
                    &context,
                    ("unused generated response".into(), None, None),
                    false,
                    &mut events,
                );
                let error = result.unwrap_err();
                if mode != "later_action" {
                    assert_eq!(error.phase, "output_override");
                    assert_eq!(error.detail, "later override failed");
                } else {
                    assert_eq!(error.phase, "publication");
                }
                Ok(())
            })
            .unwrap();
            assert_eq!(
                fs::read_to_string(&path).unwrap(),
                "saved daily searchable text"
            );
            let attempts: Vec<Value> = String::from_utf8(events)
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect();
            assert_eq!(attempts.len(), 1, "{mode}: {attempts:?}");
            assert_eq!(attempts[0]["event"], "index.attempt");
            assert_eq!(attempts[0]["path"], "20260101/talents/plain.md");
            if mode == "override_index_error" {
                assert_eq!(attempts[0]["outcome"], "failed");
                assert!(
                    attempts[0]["cause"]
                        .as_str()
                        .is_some_and(|cause| !cause.is_empty())
                );
            } else {
                assert_eq!(attempts[0]["outcome"], "indexed");
                let results = solstone_core_indexer_query::search(
                    &context.journal,
                    solstone_core_indexer_query::OwnerBoundary,
                    &solstone_core_indexer_query::SearchRequest::new(
                        "saved daily searchable",
                        Default::default(),
                    ),
                    chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
                )
                .unwrap();
                assert_eq!(results.results.len(), 1);
                assert!(
                    results.results[0]
                        .text
                        .contains("saved daily searchable text")
                );
            }
        }
    }

    fn fixture(
        root: &Path,
        retained: bool,
    ) -> (ExecutionContext, DailyUnitIdentity, Map<String, Value>) {
        let context = ExecutionContext {
            journal: root.join("journal"),
        };
        fs::create_dir_all(&context.journal).unwrap();
        let identity = DailyUnitIdentity::new("20260101", "morning_briefing", None);
        // The serialized admission boundary is the subject: execution must not
        // require any talent files, index, or source inputs to rebuild this.
        let packet = json!({
            "version":1,
            "prepared":{"name":"morning_briefing","config":{
                "day":"20260101", "type":"generate", "max_output_tokens":1024, "prompt":"frozen source evidence",
                "model":"test-model", "provider":"test",
                "hook":{"pre":"morning_briefing"},
                "output_path":context.journal.join("chronicle/20260101/talents/morning_briefing.md"),
                "_daily_artifact_before":{"chronicle/20260101/talents/morning_briefing.md":null},
            }},
            "hook":"morning_briefing",
            "state":{"MorningBriefing":{"values":{}}},
        });
        let mut record = DailyUnitRecord::new(identity.clone(), "E", "contract");
        record.lock_token = Some("attempt-1".to_owned());
        record.use_id = Some("attempt-1".to_owned());
        record.packet_digest = Some(crate::daily_prepare::packet_digest(&packet));
        record.frozen_packet = Some(packet);
        if retained {
            record.generated_result = Some(
                json!({"response":"retained evidence result","usage":{"input_tokens":12},"degraded":null}),
            );
        }
        save_daily_unit_record(&context.journal, &record).unwrap();
        (context, identity, json!({"name":"morning_briefing","day":"20260101","lock_token":"attempt-1","evidence_revision":"E","contract_digest":"contract"}).as_object().unwrap().clone())
    }

    fn observer_fixture(root: &Path) -> (ExecutionContext, PreparedTalent, DailyUnitIdentity) {
        let context = ExecutionContext {
            journal: root.join("journal"),
        };
        solstone_core_facets::create_facet(&context.journal, "work", "Work", "", "", "", None)
            .unwrap();
        solstone_core_facets::attach_or_reactivate_entity(
            &context.journal,
            "work",
            "Person",
            "Ada",
            "Engineer",
        )
        .unwrap();
        solstone_core_facets::save_detected_entity(
            &context.journal,
            "work",
            "20260910",
            "Person",
            "Ada",
            "Discussed preferences",
        )
        .unwrap();
        solstone_core_facets::add_observation(
            &context.journal,
            "work",
            "ada",
            "Prefers concise updates",
            None,
            None,
        )
        .unwrap();
        let sugg_dir = context.journal.join("facets/work/entities");
        std::fs::create_dir_all(&sugg_dir).unwrap();
        std::fs::write(
            sugg_dir.join("20260910_observer_suggestions.json"),
            json!({
                "facet": "work",
                "day": "20260910",
                "entities": [
                    {
                        "entity_id": "ada",
                        "suggestions": [
                            {"content": "Prefers concise updates"}
                        ]
                    }
                ]
            })
            .to_string(),
        )
        .unwrap();
        let prepared = PreparedTalent {
            name: "entities:entity_observer".into(),
            config: json!({"day":"20260910", "facet":"work", "type":"generate", "max_output_tokens":1024, "prompt":"$observer_context", "model":"test-model", "provider":"test", "hook":{"pre":"entities:entity_observer", "post":"entities:entity_observer"}}).as_object().unwrap().clone(),
        };
        let identity = DailyUnitIdentity::new("20260910", &prepared.name, Some("work".into()));
        (context, prepared, identity)
    }

    fn observer_output(id: u64, quote: &str, content: &str) -> String {
        json!({"entities":[{"entity_id":"ada", "decisions":[{"op":"replace", "target_id":id, "target_quote":quote, "content":content}]}]}).to_string()
    }

    fn observer_request(token: &str) -> Map<String, Value> {
        json!({"name":"entities:entity_observer", "day":"20260910", "facet":"work", "lock_token":token}).as_object().unwrap().clone()
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[test]
    fn invalid_frozen_observer_reference_regenerates_and_preserves_prior_acceptance() {
        let root = tempfile::tempdir().unwrap();
        let (context, prepared, identity) = observer_fixture(root.path());
        let keep = observer_output(1, "Prefers concise updates", "Prefers concise updates");
        let good = observer_output(
            1,
            "Prefers concise updates",
            "Prefers concise weekly updates",
        );
        let stub = crate::test_support::sequenced_one_shot_stub(
            root.path(),
            &[
                crate::test_support::generated_response_value(&keep, Value::Null),
                crate::test_support::generated_response_value(&good, Value::Null),
            ],
        );
        let generate = OneShotClient::at_path(stub.clone());
        let mut record = DailyUnitRecord::new(identity.clone(), "E1", "C");
        let packet = crate::daily_prepare::freeze(prepared.clone(), &context).unwrap();
        record.packet_digest = Some(crate::daily_prepare::packet_digest(&packet));
        record.frozen_packet = Some(packet);
        record.lock_token = Some("first".into());
        save_daily_unit_record(&context.journal, &record).unwrap();
        let first = execute(
            observer_request("first"),
            &context,
            &generate,
            &mut Vec::new(),
        );
        assert!(
            matches!(first, RuntimeOutcome::Finished { .. }),
            "{first:?}"
        );
        let prior = load_daily_unit_record(&context.journal, &identity)
            .unwrap()
            .unwrap()
            .accepted
            .unwrap();
        let before =
            solstone_core_facets::read_facet_entity_observations(&context.journal, "work", "ada")
                .unwrap();
        let packet = crate::daily_prepare::freeze(prepared, &context).unwrap();
        let mut record = DailyUnitRecord::new(identity.clone(), "E2", "C");
        record.packet_digest = Some(crate::daily_prepare::packet_digest(&packet));
        record.frozen_packet = Some(packet.clone());
        record.lock_token = Some("bad".into());
        record.use_id = Some("bad".into());
        record.accepted = Some(prior.clone());
        record.generated_result = Some(json!({"response":"{invalid_json"}));
        save_daily_unit_record(&context.journal, &record).unwrap();
        let bad = execute(
            observer_request("bad"),
            &context,
            &OneShotClient::at_path(root.path().join("no-model")),
            &mut Vec::new(),
        );
        let RuntimeOutcome::StageFailed(error) = bad else {
            panic!("{bad:?}")
        };
        assert_eq!(error.phase, "parse", "{error:?}");
        let failed = load_daily_unit_record(&context.journal, &identity)
            .unwrap()
            .unwrap();
        assert_eq!(failed.reason_code.as_deref(), Some("schema_invalid"));
        assert!(failed.generated_result.is_none());
        assert!(failed.action_plan.is_none());
        assert!(failed.receipts.is_empty());
        assert_eq!(
            serde_json::to_value(&failed.accepted).unwrap(),
            serde_json::to_value(Some(&prior)).unwrap()
        );
        assert_eq!(
            solstone_core_facets::read_facet_entity_observations(&context.journal, "work", "ada")
                .unwrap(),
            before
        );
        assert_eq!(
            fs::read_to_string(stub.with_extension("sh.count"))
                .unwrap()
                .trim(),
            "1"
        );
        with_daily_unit_authority(&context.journal, &identity, |authority| {
            let current = authority.record_mut().as_mut().unwrap();
            current.lock_token = Some("retry".into());
            current.use_id = Some("retry".into());
            authority.checkpoint()
        })
        .unwrap();
        let retry = execute(
            observer_request("retry"),
            &context,
            &generate,
            &mut Vec::new(),
        );
        assert!(
            matches!(retry, RuntimeOutcome::Finished { .. }),
            "{retry:?}"
        );
        let accepted = load_daily_unit_record(&context.journal, &identity)
            .unwrap()
            .unwrap();
        assert!(accepted.is_reusable_for("E2", "C"));
        assert_eq!(accepted.frozen_packet.as_ref(), Some(&packet));
        assert_eq!(
            fs::read_to_string(stub.with_extension("sh.count"))
                .unwrap()
                .trim(),
            "2"
        );
        assert_eq!(
            solstone_core_facets::read_live_observations(
                &context.journal,
                "work",
                "ada",
                Default::default()
            )
            .unwrap()
            .items[0]
                .content,
            "Prefers concise weekly updates"
        );
    }

    #[test]
    fn owner_observation_drift_retains_even_an_invalid_model_response() {
        for (quote, changed_entity) in [
            ("Prefers concise updates", "ada"),
            ("\"Prefers concise updates\"", "ada"),
            ("\"Prefers concise updates\"", "grace"),
        ] {
            let root = tempfile::tempdir().unwrap();
            let (context, prepared, identity) = observer_fixture(root.path());
            if changed_entity == "grace" {
                solstone_core_facets::attach_or_reactivate_entity(
                    &context.journal,
                    "work",
                    "Person",
                    "Grace",
                    "Engineer",
                )
                .unwrap();
                solstone_core_facets::save_detected_entity(
                    &context.journal,
                    "work",
                    "20260910",
                    "Person",
                    "Grace",
                    "Discussed preferences",
                )
                .unwrap();
                solstone_core_facets::add_observation(
                    &context.journal,
                    "work",
                    "grace",
                    "Prefers concise updates",
                    None,
                    None,
                )
                .unwrap();
                let sugg_path = context
                    .journal
                    .join("facets/work/entities/20260910_observer_suggestions.json");
                std::fs::write(
                    &sugg_path,
                    json!({
                        "facet": "work",
                        "day": "20260910",
                        "entities": [
                            {
                                "entity_id": "ada",
                                "suggestions": [{"content": "Prefers concise updates"}]
                            },
                            {
                                "entity_id": "grace",
                                "suggestions": [{"content": "Prefers concise updates"}]
                            }
                        ]
                    })
                    .to_string(),
                )
                .unwrap();
            }
            let packet = crate::daily_prepare::freeze(prepared, &context).unwrap();
            let mut record = DailyUnitRecord::new(identity.clone(), "E", "C");
            record.packet_digest = Some(crate::daily_prepare::packet_digest(&packet));
            record.frozen_packet = Some(packet);
            record.lock_token = Some("attempt".into());
            let mut output: Value = serde_json::from_str(&observer_output(
                1,
                quote,
                "Prefers concise monthly updates",
            ))
            .unwrap();
            if changed_entity == "grace" {
                let mut later = output["entities"][0].clone();
                later["entity_id"] = json!("grace");
                later["decisions"][0]["target_id"] = json!(1);
                later["decisions"][0]["target_quote"] = json!("Prefers concise updates");
                output["entities"].as_array_mut().unwrap().push(later);
            }
            let response = json!({"response":output.to_string()});
            record.generated_result = Some(response.clone());
            save_daily_unit_record(&context.journal, &record).unwrap();
            solstone_core_facets::add_observation(
                &context.journal,
                "work",
                changed_entity,
                "Owner correction",
                None,
                None,
            )
            .unwrap();
            let before = solstone_core_facets::read_facet_entity_observations(
                &context.journal,
                "work",
                changed_entity,
            )
            .unwrap();
            let model = OneShotClient::at_path(root.path().join("no-model"));
            for _ in 0..2 {
                let outcome = execute(
                    observer_request("attempt"),
                    &context,
                    &model,
                    &mut Vec::new(),
                );
                let RuntimeOutcome::StageFailed(error) = outcome else {
                    panic!("{outcome:?}")
                };
                assert_eq!(error.phase, "conflict", "{error:?}");
                let failed = load_daily_unit_record(&context.journal, &identity)
                    .unwrap()
                    .unwrap();
                assert_eq!(failed.status, DailyUnitStatus::Conflicting);
                assert_eq!(failed.reason_code.as_deref(), Some("daily_owner_conflict"));
                assert_eq!(failed.generated_result.as_ref(), Some(&response));
                assert!(failed.action_plan.is_none());
                assert!(failed.receipts.is_empty());
                assert_eq!(
                    solstone_core_facets::read_facet_entity_observations(
                        &context.journal,
                        "work",
                        changed_entity
                    )
                    .unwrap(),
                    before
                );
            }
        }
    }

    #[test]
    fn frozen_observer_owner_guards_precede_invalid_model_references() {
        for changed_owner in ["outcome", "facet"] {
            let root = tempfile::tempdir().unwrap();
            let (context, prepared, identity) = observer_fixture(root.path());
            let before = solstone_core_facets::read_facet_entity_observations(
                &context.journal,
                "work",
                "ada",
            )
            .unwrap();
            let old_facet_id =
                solstone_core_facets::facet_write_identity(&context.journal, "work").unwrap();
            let packet = crate::daily_prepare::freeze(prepared, &context).unwrap();
            let mut record = DailyUnitRecord::new(identity.clone(), "E", "C");
            record.packet_digest = Some(crate::daily_prepare::packet_digest(&packet));
            record.frozen_packet = Some(packet);
            record.lock_token = Some("attempt".into());
            let response = json!({"response":observer_output(
                1, "\"Prefers concise updates\"", "Must not replace owner memory",
            )});
            record.generated_result = Some(response.clone());
            save_daily_unit_record(&context.journal, &record).unwrap();
            let outcome_path = context
                .journal
                .join("facets/work/entities/20260910_observer_outcome.json");
            if changed_owner == "outcome" {
                std::fs::write(&outcome_path, "Owner outcome correction\n").unwrap();
            } else {
                // A journal keeps one enabled facet; the sibling lets "work" go.
                let _ = solstone_core_facets::create_facet(
                    &context.journal,
                    "personal",
                    "Personal",
                    "",
                    "",
                    "",
                    None,
                );
                solstone_core_facets::delete_facet(&context.journal, "work").unwrap();
                // These tests recreate "work" to prove the fences against a replaced facet;
                // the store never reuses a name, so the retired record is dropped as a hand edit would.
                let _ = std::fs::remove_file(
                    std::path::Path::new(&context.journal).join("facets/retired.json"),
                );
                observer_fixture(root.path());
                assert_ne!(
                    solstone_core_facets::facet_write_identity(&context.journal, "work").unwrap(),
                    old_facet_id,
                );
                std::fs::write(
                    context
                        .journal
                        .join("facets/work/entities/ada/observations.jsonl"),
                    before.as_deref().unwrap_or_default(),
                )
                .unwrap();
            }
            assert_eq!(
                solstone_core_facets::read_facet_entity_observations(
                    &context.journal,
                    "work",
                    "ada"
                )
                .unwrap(),
                before,
                "The observation snapshot must remain unchanged",
            );
            let owner_outcome = std::fs::read(&outcome_path).ok();
            let model = OneShotClient::at_path(root.path().join("no-model"));
            for _ in 0..2 {
                let outcome = execute(
                    observer_request("attempt"),
                    &context,
                    &model,
                    &mut Vec::new(),
                );
                let RuntimeOutcome::StageFailed(error) = outcome else {
                    panic!("{outcome:?}")
                };
                assert_eq!(error.phase, "conflict", "{changed_owner}: {error:?}");
                assert!(
                    error.detail.contains(if changed_owner == "outcome" {
                        "required artifact changed"
                    } else {
                        "owning facet changed"
                    }),
                    "{error:?}"
                );
                let failed = load_daily_unit_record(&context.journal, &identity)
                    .unwrap()
                    .unwrap();
                assert_eq!(failed.status, DailyUnitStatus::Conflicting);
                assert_eq!(failed.reason_code.as_deref(), Some("daily_owner_conflict"));
                assert_eq!(failed.generated_result.as_ref(), Some(&response));
                assert!(failed.action_plan.is_none());
                assert!(failed.receipts.is_empty());
                assert_eq!(std::fs::read(&outcome_path).ok(), owner_outcome);
                assert_eq!(
                    solstone_core_facets::read_facet_entity_observations(
                        &context.journal,
                        "work",
                        "ada"
                    )
                    .unwrap(),
                    before,
                );
            }
        }
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[test]
    fn retained_response_publishes_without_model_or_live_preparation() {
        let root = tempfile::tempdir().unwrap();
        let (context, identity, request) = fixture(root.path(), true);
        let absent_generate = OneShotClient::at_path(root.path().join("not-installed"));
        let outcome = execute(request.clone(), &context, &absent_generate, &mut Vec::new());
        assert!(
            matches!(outcome, RuntimeOutcome::Finished { .. }),
            "{outcome:?}"
        );
        let output = context
            .journal
            .join("chronicle/20260101/talents/morning_briefing.md");
        assert_eq!(
            fs::read_to_string(&output).unwrap(),
            "retained evidence result"
        );
        let accepted = load_daily_unit_record(&context.journal, &identity)
            .unwrap()
            .unwrap();
        assert!(accepted.action_plan.is_some());
        assert!(accepted.is_reusable_for("E", "contract"));
        assert!(
            accepted
                .accepted
                .as_ref()
                .unwrap()
                .receipts
                .iter()
                .any(|r| r["kind"] == "required_artifact")
        );
        let unchanged = fs::read(&output).unwrap();
        let outcome = execute(request, &context, &absent_generate, &mut Vec::new());
        assert!(matches!(outcome, RuntimeOutcome::Finished { .. }));
        assert_eq!(fs::read(output).unwrap(), unchanged);

        // Positive control: without a retained response the absent model really
        // fails, so the passing case above did not silently skip publication.
        let fresh = tempfile::tempdir().unwrap();
        let (fresh_context, _, fresh_request) = fixture(fresh.path(), false);
        let outcome = execute(
            fresh_request,
            &fresh_context,
            &absent_generate,
            &mut Vec::new(),
        );
        assert!(!matches!(outcome, RuntimeOutcome::Finished { .. }));
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[test]
    fn rejected_model_response_can_be_regenerated_before_any_owner_action() {
        let root = tempfile::tempdir().unwrap();
        let context = ExecutionContext {
            journal: root.path().join("journal"),
        };
        fs::create_dir_all(&context.journal).unwrap();
        let identity = DailyUnitIdentity::new("20260101", "schedule", None);
        let prepared = PreparedTalent {
            name: "schedule".to_owned(),
            config: json!({
                "day":"20260101", "type":"generate", "max_output_tokens":1024, "prompt":"frozen calendar evidence",
                "model":"test-model", "provider":"test", "hook":{"post":"schedule"}
            })
            .as_object()
            .unwrap()
            .clone(),
        };
        let packet = crate::daily_prepare::freeze(prepared, &context).unwrap();
        let mut record = DailyUnitRecord::new(identity.clone(), "E", "C");
        record.lock_token = Some("attempt".to_owned());
        record.packet_digest = Some(crate::daily_prepare::packet_digest(&packet));
        record.frozen_packet = Some(packet);
        record.generated_result =
            Some(json!({"response":"malformed calendar", "usage":{"input_tokens":12}}));
        save_daily_unit_record(&context.journal, &record).unwrap();
        let request = json!({"name":"schedule","day":"20260101","lock_token":"attempt"})
            .as_object()
            .unwrap()
            .clone();
        let outcome = execute(
            request.clone(),
            &context,
            &OneShotClient::at_path(root.path().join("no-model")),
            &mut Vec::new(),
        );
        let RuntimeOutcome::StageFailed(error) = outcome else {
            panic!("expected rejected model output")
        };
        assert_eq!(error.phase, "parse");
        assert_eq!(error.usage.unwrap()["input_tokens"], 12);
        let failed = load_daily_unit_record(&context.journal, &identity)
            .unwrap()
            .unwrap();
        assert!(failed.generated_result.is_none());
        assert!(failed.action_plan.is_none());
        assert!(failed.accepted.is_none());
        assert!(failed.receipts.is_empty());
        let stub = crate::test_support::one_shot_stub(root.path(), "[]");
        let outcome = execute(
            request,
            &context,
            &OneShotClient::at_path(stub),
            &mut Vec::new(),
        );
        assert!(
            matches!(
                outcome,
                RuntimeOutcome::Finished {
                    disposition: CommitDisposition::CommittedNoOutput,
                    ..
                }
            ),
            "{outcome:?}"
        );
        assert!(
            load_daily_unit_record(&context.journal, &identity)
                .unwrap()
                .unwrap()
                .is_reusable_for("E", "C")
        );
    }

    #[test]
    fn retained_prepared_plan_resumes_without_replanning_owner_state() {
        let root = tempfile::tempdir().unwrap();
        let (context, identity, request) = fixture(root.path(), true);
        with_daily_unit_authority(&context.journal, &identity, |authority| {
            let packet = authority.record().unwrap().frozen_packet.as_ref().unwrap();
            let (prepared, _) = crate::daily_prepare::thaw(packet).unwrap();
            let publication = crate::writers::prepare_daily_output(
                &prepared,
                "retained evidence result",
                &context,
            )
            .unwrap();
            authority.record_mut().as_mut().unwrap().action_plan =
                Some(serde_json::to_value(publication).unwrap());
            authority.checkpoint()
        })
        .unwrap();
        let output = context
            .journal
            .join("chronicle/20260101/talents/morning_briefing.md");
        fs::create_dir_all(output.parent().unwrap()).unwrap();
        fs::write(&output, "intervening owner correction").unwrap();
        let outcome = execute(
            request,
            &context,
            &OneShotClient::at_path(root.path().join("no-model")),
            &mut Vec::new(),
        );
        assert!(
            matches!(outcome, RuntimeOutcome::StageFailed(_)),
            "{outcome:?}"
        );
        assert_eq!(
            fs::read_to_string(output).unwrap(),
            "intervening owner correction"
        );
        let record = load_daily_unit_record(&context.journal, &identity)
            .unwrap()
            .unwrap();
        assert!(record.accepted.is_none());
        assert_eq!(record.status, DailyUnitStatus::Conflicting);
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[test]
    fn replaced_worker_finishes_generation_but_cannot_reach_owner_write() {
        let root = tempfile::tempdir().unwrap();
        let (context, identity, request) = fixture(root.path(), false);
        use solstone_core_journal_io::cortex_use::{
            admit_active_use, complete_active_use, create_or_admit_cortex_namespace,
        };
        let namespace = create_or_admit_cortex_namespace(
            solstone_core_journal_io::JournalRoot::open(&context.journal).unwrap(),
        )
        .unwrap();
        let use_id = "1789400000001";
        let admitted = admit_active_use(
            &namespace,
            "morning_briefing",
            use_id,
            b"{\"event\":\"request\",\"name\":\"morning_briefing\",\"day\":\"20260101\"}\n",
        )
        .unwrap();
        with_daily_unit_authority(&context.journal, &identity, |authority| {
            authority.record_mut().as_mut().unwrap().use_id = Some(use_id.to_owned());
            authority.checkpoint()
        })
        .unwrap();
        let started = root.path().join("model-started");
        let release = root.path().join("model-release");
        let stub = crate::test_support::one_shot_stub(root.path(), "old response");
        let body = fs::read_to_string(&stub).unwrap();
        let barrier = format!(
            "cat >/dev/null\ntouch '{}'\nattempt=0\nwhile [ ! -f '{}' ]; do\n attempt=$((attempt+1))\n [ \"$attempt\" -lt 500 ] || exit 93\n sleep 0.01\ndone\n",
            started.display(),
            release.display()
        );
        assert!(body.contains("cat >/dev/null\n"));
        fs::write(&stub, body.replacen("cat >/dev/null\n", &barrier, 1)).unwrap();
        let worker_context = context.clone();
        let worker = std::thread::spawn(move || {
            execute(
                request,
                &worker_context,
                &OneShotClient::at_path(stub),
                &mut Vec::new(),
            )
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        while !started.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(started.exists(), "old worker did not reach model barrier");
        // Cortex can terminalize a timed-out use while its model child survives.
        // Closing that use must not confer authority on the surviving worker.
        let active = context
            .journal
            .join(format!("talents/morning_briefing/{use_id}_active.jsonl"));
        std::fs::OpenOptions::new()
            .append(true)
            .open(&active)
            .unwrap()
            .write_all(b"{\"event\":\"error\",\"reason_code\":\"talent_timeout\"}\n")
            .unwrap();
        complete_active_use(&namespace, "morning_briefing", use_id, admitted.identity()).unwrap();
        assert!(!active.exists());
        assert!(
            context
                .journal
                .join(format!("talents/morning_briefing/{use_id}.jsonl"))
                .is_file()
        );
        with_daily_unit_authority(&context.journal, &identity, |authority| {
            authority.require_token("attempt-1")?;
            let record = authority.record_mut().as_mut().unwrap();
            record.lock_token = Some("replacement-2".to_owned());
            record.evidence_revision = "E2".to_owned();
            authority.checkpoint()
        })
        .unwrap();
        fs::write(release, b"continue").unwrap();
        let outcome = worker.join().unwrap();
        assert!(
            matches!(outcome, RuntimeOutcome::StageFailed(_)),
            "{outcome:?}"
        );
        assert!(
            !context
                .journal
                .join("chronicle/20260101/talents/morning_briefing.md")
                .exists()
        );
        let record = load_daily_unit_record(&context.journal, &identity)
            .unwrap()
            .unwrap();
        assert_eq!(record.lock_token.as_deref(), Some("replacement-2"));
        assert!(record.generated_result.is_none());
        assert!(record.accepted.is_none());
    }
    #[test]
    fn daily_parse_failure_terminal_and_persisted_counter_use_the_same_cause() {
        for (previous, expected_count) in [("schema_invalid", 2), ("no_output", 0)] {
            let root = tempfile::tempdir().unwrap();
            let (context, old_identity, _) = fixture(root.path(), true);
            let mut record = load_daily_unit_record(&context.journal, &old_identity)
                .unwrap()
                .unwrap();
            record.identity = DailyUnitIdentity::new("20260101", "schedule", None);
            record.reason_code = Some(previous.to_owned());
            record.failure_count = 2;
            record.generated_result =
                Some(json!({"response":"invalid json","usage":null,"degraded":null}));
            let packet = record.frozen_packet.as_mut().unwrap();
            packet["prepared"]["name"] = json!("schedule");
            packet["prepared"]["config"]["hook"] = json!({"post":"schedule"});
            packet["hook"] = json!("schedule");
            packet["state"] = json!("None");
            record.packet_digest = Some(crate::daily_prepare::packet_digest(packet));
            save_daily_unit_record(&context.journal, &record).unwrap();
            let request =
                json!({"name":"schedule","day":"20260101","lock_token":record.lock_token});
            let generate = OneShotClient::at_path(root.path().join("absent-generate"));
            let paths = crate::prepare::RuntimePaths {
                talent_root: root.path().join("absent-talents"),
                apps_root: root.path().join("absent-apps"),
                templates_dir: root.path().join("absent-templates"),
            };
            let mut wire = Vec::new();
            crate::run_lines(
                std::io::Cursor::new(format!("{request}\n")),
                &mut wire,
                &paths,
                &context,
                Ok(&generate),
            );
            let terminal: Value = String::from_utf8(wire)
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str::<Value>(line).unwrap())
                .find(|row| row["terminal"] == true)
                .unwrap();
            assert_eq!(terminal["reason_code"], "schema_invalid");
            let after = load_daily_unit_record(&context.journal, &record.identity)
                .unwrap()
                .unwrap();
            assert_eq!(after.reason_code.as_deref(), Some("schema_invalid"));
            assert_eq!(after.failure_count, expected_count);
            assert!(after.generated_result.is_none());
        }
    }

    #[test]
    fn a_schedule_calendar_conflict_retries_from_a_fresh_prompt() {
        let root = tempfile::tempdir().unwrap();
        let context = ExecutionContext {
            journal: root.path().join("journal"),
        };
        fs::create_dir_all(&context.journal).unwrap();
        solstone_core_facets::create_facet(&context.journal, "work", "Work", "", "", "", None)
            .unwrap();
        let events = |details: &str| {
            json!({"events":[{"activity":"meeting", "target_date":"2026-01-20", "start":"09:00:00",
                "title":"Planning review", "description":"Discuss", "details":details,
                "facet":"work", "participation":[]}]})
            .to_string()
        };
        let schedule = |day: &str, details: &str| {
            for batch in
                crate::schedule::prepare_publication(&context.journal, &events(details), day)
                    .unwrap()
            {
                solstone_core_facets::publish_anticipation_batch(
                    &context.journal,
                    &batch,
                    true,
                    || Ok(()),
                )
                .unwrap();
            }
        };
        let calendar = || {
            solstone_core_facets::read_activity_file(&context.journal, "work", "20260120.jsonl")
                .unwrap()
        };
        schedule("20260105", "first");
        let facet_id =
            solstone_core_facets::facet_write_identity(&context.journal, "work").unwrap();
        let prepared = PreparedTalent {
            name: "schedule".to_owned(),
            config: json!({
                "day":"20260110", "type":"generate", "max_output_tokens":1024, "prompt":"frozen calendar evidence",
                "model":"test-model", "provider":"test", "hook":{"post":"schedule"},
                "_daily_facet_ids":{"work":facet_id},
                "_daily_calendar_before":{"work/20260120":calendar()},
            })
            .as_object()
            .unwrap()
            .clone(),
        };
        let packet = crate::daily_prepare::freeze(prepared, &context).unwrap();
        let identity = DailyUnitIdentity::new("20260110", "schedule", None);
        let mut record = DailyUnitRecord::new(identity.clone(), "E", "C");
        record.lock_token = Some("attempt".to_owned());
        record.packet_digest = Some(crate::daily_prepare::packet_digest(&packet));
        record.frozen_packet = Some(packet);
        record.generated_result = Some(json!({"response":events("third")}));
        save_daily_unit_record(&context.journal, &record).unwrap();
        // Another day's schedule updates the same event while this one ran.
        schedule("20260106", "second");
        let moved = calendar();
        let outcome = execute(
            json!({"name":"schedule","day":"20260110","lock_token":"attempt"})
                .as_object()
                .unwrap()
                .clone(),
            &context,
            &OneShotClient::at_path(root.path().join("no-model")),
            &mut Vec::new(),
        );
        let RuntimeOutcome::StageFailed(error) = outcome else {
            panic!("expected a calendar conflict, got {outcome:?}")
        };
        assert_eq!(error.phase, "conflict", "{error}");
        let record = load_daily_unit_record(&context.journal, &identity)
            .unwrap()
            .unwrap();
        assert_eq!(record.status, DailyUnitStatus::Conflicting);
        assert_eq!(record.failure_count, 1);
        assert!(record.frozen_packet.is_none());
        assert!(record.generated_result.is_none());
        assert_eq!(calendar(), moved);
    }

    #[test]
    fn a_review_merge_proposal_conflict_retries_from_a_fresh_prompt() {
        let root = tempfile::tempdir().unwrap();
        let context = ExecutionContext {
            journal: root.path().join("journal"),
        };
        fs::create_dir_all(&context.journal).unwrap();
        solstone_core_facets::create_facet(&context.journal, "work", "Work", "", "", "", None)
            .unwrap();
        for (day, name) in [("20260101", "Ada"), ("20260102", "Ada Lovelace")] {
            solstone_core_facets::upsert_detection_segment(
                &context.journal,
                "work",
                day,
                "090000_300",
                &[solstone_core_facets::DetectedEntityInput {
                    entity_type: "Person".to_owned(),
                    name: name.to_owned(),
                    description: "Recurring collaborator.".to_owned(),
                }],
            )
            .unwrap();
        }
        let source = solstone_core_entity_matching::entity_slug("Ada");
        let target = solstone_core_entity_matching::entity_slug("Ada Lovelace");
        // Publishes a merge proposal as another day's review of the facet does.
        let propose = |day: &str, summary: &str| {
            let proposal = json!({"facet":"work", "day":day, "source":"Ada", "source_slug":source,
                "target":"Ada Lovelace", "target_slug":target, "summary":summary});
            let batch =
                solstone_core_entity::prepare_merge_proposals(&context.journal, &[proposal])
                    .unwrap();
            solstone_core_entity::publish_merge_proposals(
                &context.journal,
                &batch,
                true,
                || Ok(()),
                || Ok(()),
            )
            .unwrap();
        };
        propose("20260105", "first");
        let prepared = PreparedTalent {
            name: "entities:entities_review".to_owned(),
            config: json!({
                "day":"20260108", "facet":"work", "type":"generate", "max_output_tokens":1024,
                "prompt":"review", "model":"test-model", "provider":"test",
                "hook":{"pre":"entities:entities_review", "post":"entities:entities_review"},
            })
            .as_object()
            .unwrap()
            .clone(),
        };
        let packet = crate::daily_prepare::freeze(prepared, &context).unwrap();
        assert_eq!(
            packet["prepared"]["config"]["_daily_review_inputs"]["prior"]
                .as_array()
                .map(Vec::len),
            Some(1)
        );
        let identity =
            DailyUnitIdentity::new("20260108", "entities:entities_review", Some("work".into()));
        let mut record = DailyUnitRecord::new(identity.clone(), "E", "C");
        record.lock_token = Some("attempt".to_owned());
        record.packet_digest = Some(crate::daily_prepare::packet_digest(&packet));
        record.frozen_packet = Some(packet);
        record.generated_result = Some(json!({"response":json!({"promotions":[],
            "merges":[{"source":"Ada", "canonical":"Ada Lovelace", "evidence":"same person"}]})
        .to_string()}));
        save_daily_unit_record(&context.journal, &record).unwrap();
        // Another day's review resurfaces the same proposal while this one ran.
        propose("20260106", "second");
        let outcome = execute(
            json!({"name":"entities:entities_review","day":"20260108","facet":"work","lock_token":"attempt"})
                .as_object()
                .unwrap()
                .clone(),
            &context,
            &OneShotClient::at_path(root.path().join("no-model")),
            &mut Vec::new(),
        );
        let RuntimeOutcome::StageFailed(error) = outcome else {
            panic!("expected a merge proposal conflict, got {outcome:?}")
        };
        assert_eq!(error.phase, "conflict", "{error}");
        assert_eq!(
            error.owner_conflict_kind(),
            Some("merge_proposal_preparation")
        );
        let record = load_daily_unit_record(&context.journal, &identity)
            .unwrap()
            .unwrap();
        assert_eq!(record.status, DailyUnitStatus::Conflicting);
        assert_eq!(record.failure_count, 1);
        assert!(record.frozen_packet.is_none());
        assert!(record.generated_result.is_none());
        assert!(record.packet_digest.is_none());
    }

    #[test]
    fn every_owner_conflict_counts_toward_its_retry_budget() {
        let identity = DailyUnitIdentity::new("20260910", "schedule", None);
        let mut record = DailyUnitRecord::new(identity.clone(), "E", "C");
        let conflict = StageError::new(
            "conflict",
            "daily",
            "daily",
            "conflict: calendar changed after prompt preparation",
        )
        .with_identity(&identity);
        apply_stage_failure(&mut record, &conflict);
        apply_stage_failure(&mut record, &conflict);
        assert_eq!(record.status, DailyUnitStatus::Conflicting);
        assert_eq!(record.failure_count, 2);
    }

    #[test]
    fn review_conflict_retry_budget_resets_on_kind_or_evidence_change() {
        let identity =
            DailyUnitIdentity::new("20260910", "entities:entities_review", Some("work".into()));
        let mut record = DailyUnitRecord::new(identity.clone(), "E", "C");
        let alias = StageError::owner_conflict(
            &identity,
            "daily_publication",
            solstone_core_entity::ReviewOwnerConflictKind::AliasClaimed,
            "alias",
        );
        apply_stage_failure(&mut record, &alias);
        assert_eq!(record.failure_count, 1);
        assert_eq!(record.owner_conflict_kind.as_deref(), Some("alias_claimed"));
        apply_stage_failure(&mut record, &alias);
        assert_eq!(record.failure_count, 2);

        let moved = StageError::owner_conflict(
            &identity,
            "daily_publication",
            solstone_core_entity::ReviewOwnerConflictKind::IdentityChanged,
            "identity",
        );
        apply_stage_failure(&mut record, &moved);
        assert_eq!(
            record.failure_count, 1,
            "a distinct later conflict kind must not inherit exhaustion"
        );
        assert_eq!(
            record.owner_conflict_kind.as_deref(),
            Some("identity_changed")
        );

        record = DailyUnitRecord::new(identity.clone(), "E2", "C2");
        apply_stage_failure(&mut record, &alias);
        assert_eq!(record.failure_count, 1);
        assert_eq!(record.evidence_revision, "E2");

        let failed = StageError::new(
            "publication",
            "daily_publication",
            "entities:entities_review",
            "read failed",
        )
        .with_identity(&identity);
        apply_stage_failure(&mut record, &failed);
        assert_eq!(record.failure_count, 0);
        assert_eq!(record.reason_code.as_deref(), Some("talent_stage_failed"));
        assert_eq!(record.owner_conflict_kind.as_deref(), None);
    }

    #[test]
    fn merge_proposal_conflict_family_shares_retry_budget() {
        let identity =
            DailyUnitIdentity::new("20260910", "entities:entities_review", Some("work".into()));
        let mut record = DailyUnitRecord::new(identity.clone(), "E", "C");
        let prep = StageError::owner_conflict(
            &identity,
            "daily_publication",
            solstone_core_entity::ReviewOwnerConflictKind::MergeProposalPreparation,
            "preparation changed",
        );
        apply_stage_failure(&mut record, &prep);
        assert_eq!(record.failure_count, 1);
        assert_eq!(
            record.owner_conflict_kind.as_deref(),
            Some("merge_proposal_preparation")
        );
        let changed = StageError::owner_conflict(
            &identity,
            "daily_publication",
            solstone_core_entity::ReviewOwnerConflictKind::MergeProposalsChanged,
            "candidates changed",
        );
        apply_stage_failure(&mut record, &changed);
        assert_eq!(
            record.failure_count, 2,
            "merge proposal conflict family must share one retry budget"
        );
        assert_eq!(
            record.owner_conflict_kind.as_deref(),
            Some("merge_proposals_changed")
        );
    }

    #[test]
    fn calendar_changed_conflict_family_shares_retry_budget() {
        let identity = DailyUnitIdentity::new("20260910", "schedule", None);

        // 1. Two calendar_changed failures share one counter (count 2)
        let mut record1 = DailyUnitRecord::new(identity.clone(), "E", "C");
        let cal = StageError::new("conflict", "daily", "daily", "calendar changed")
            .with_identity(&identity)
            .with_owner_conflict_kind("calendar_changed");
        apply_stage_failure(&mut record1, &cal);
        assert_eq!(record1.failure_count, 1);
        assert_eq!(
            record1.owner_conflict_kind.as_deref(),
            Some("calendar_changed")
        );
        apply_stage_failure(&mut record1, &cal);
        assert_eq!(record1.failure_count, 2);

        // 2. Kind-absent schedule detail containing "calendar changed after prompt preparation", then calendar_changed -> stays count 2
        let mut record2 = DailyUnitRecord::new(identity.clone(), "E", "C");
        let legacy_cal = StageError::new(
            "conflict",
            "daily",
            "daily",
            "conflict: calendar changed after prompt preparation",
        )
        .with_identity(&identity);
        apply_stage_failure(&mut record2, &legacy_cal);
        assert_eq!(record2.failure_count, 1);
        assert_eq!(record2.owner_conflict_kind.as_deref(), None);
        apply_stage_failure(&mut record2, &cal);
        assert_eq!(record2.failure_count, 2);
        assert_eq!(
            record2.owner_conflict_kind.as_deref(),
            Some("calendar_changed")
        );

        // 3. Kind-absent schedule detail containing "conflict: owning facet changed after prompt preparation", then calendar_changed -> is count 1
        let mut record3 = DailyUnitRecord::new(identity.clone(), "E", "C");
        let legacy_facet = StageError::new(
            "conflict",
            "daily",
            "daily",
            "conflict: owning facet changed after prompt preparation",
        )
        .with_identity(&identity);
        apply_stage_failure(&mut record3, &legacy_facet);
        assert_eq!(record3.failure_count, 1);
        assert_eq!(record3.owner_conflict_kind.as_deref(), None);
        apply_stage_failure(&mut record3, &cal);
        assert_eq!(
            record3.failure_count, 1,
            "distinct owning facet conflict followed by calendar changed must reset and count 1"
        );
        assert_eq!(
            record3.owner_conflict_kind.as_deref(),
            Some("calendar_changed")
        );
    }

    #[test]
    fn stale_preparation_clearing_and_retained_receipt_rules() {
        let root = tempfile::tempdir().unwrap();
        let journal = root.path().join("journal");
        let context = ExecutionContext {
            journal: journal.clone(),
        };
        fs::create_dir_all(&journal).unwrap();
        solstone_core_facets::create_facet(&journal, "work", "Work", "", "", "", None).unwrap();

        // Helpers for schedule calendar
        let events = |details: &str| {
            json!({"events":[{"activity":"meeting", "target_date":"2026-01-20", "start":"09:00:00",
                "title":"Planning review", "description":"Discuss", "details":details,
                "facet":"work", "participation":[]}]})
            .to_string()
        };
        let schedule_cal = |day: &str, details: &str| {
            for batch in
                crate::schedule::prepare_publication(&journal, &events(details), day).unwrap()
            {
                solstone_core_facets::publish_anticipation_batch(&journal, &batch, true, || Ok(()))
                    .unwrap();
            }
        };
        let calendar = || {
            solstone_core_facets::read_activity_file(&journal, "work", "20260120.jsonl").unwrap()
        };

        // Helpers for review merge proposals
        for (day, name) in [("20260101", "Ada"), ("20260102", "Ada Lovelace")] {
            solstone_core_facets::upsert_detection_segment(
                &journal,
                "work",
                day,
                "090000_300",
                &[solstone_core_facets::DetectedEntityInput {
                    entity_type: "Person".to_owned(),
                    name: name.to_owned(),
                    description: "Collaborator.".to_owned(),
                }],
            )
            .unwrap();
        }
        let source_slug = solstone_core_entity_matching::entity_slug("Ada");
        let target_slug = solstone_core_entity_matching::entity_slug("Ada Lovelace");
        let propose = |day: &str, summary: &str| {
            let proposal = json!({"facet":"work", "day":day, "source":"Ada", "source_slug":source_slug,
                "target":"Ada Lovelace", "target_slug":target_slug, "summary":summary});
            let batch =
                solstone_core_entity::prepare_merge_proposals(&journal, &[proposal]).unwrap();
            solstone_core_entity::publish_merge_proposals(
                &journal,
                &batch,
                true,
                || Ok(()),
                || Ok(()),
            )
            .unwrap();
        };

        // 1a. Receipt-free action_plan is cleared for merge_proposal_preparation
        propose("20260105", "first");
        let review_prep = PreparedTalent {
            name: "entities:entities_review".to_owned(),
            config: json!({
                "day":"20260108", "facet":"work", "type":"generate", "max_output_tokens":1024,
                "prompt":"review", "model":"test-model", "provider":"test",
                "hook":{"pre":"entities:entities_review", "post":"entities:entities_review"},
            })
            .as_object()
            .unwrap()
            .clone(),
        };
        let review_packet = crate::daily_prepare::freeze(review_prep, &context).unwrap();
        let review_identity =
            DailyUnitIdentity::new("20260108", "entities:entities_review", Some("work".into()));
        let mut review_record = DailyUnitRecord::new(review_identity.clone(), "E", "C");
        review_record.lock_token = Some("attempt-rev".to_owned());
        review_record.packet_digest = Some(crate::daily_prepare::packet_digest(&review_packet));
        review_record.frozen_packet = Some(review_packet);
        review_record.generated_result = Some(json!({"response":json!({"promotions":[],
            "merges":[{"source":"Ada", "canonical":"Ada Lovelace", "evidence":"same person"}]})
        .to_string()}));
        save_daily_unit_record(&journal, &review_record).unwrap();
        propose("20260106", "second"); // conflict

        let outcome = execute(
            json!({"name":"entities:entities_review","day":"20260108","facet":"work","lock_token":"attempt-rev"})
                .as_object()
                .unwrap()
                .clone(),
            &context,
            &OneShotClient::at_path(root.path().join("no-model")),
            &mut Vec::new(),
        );
        let RuntimeOutcome::StageFailed(err) = outcome else {
            panic!("expected StageFailed, got {outcome:?}");
        };
        assert_eq!(
            err.owner_conflict_kind(),
            Some("merge_proposal_preparation")
        );
        let loaded_rev = load_daily_unit_record(&journal, &review_identity)
            .unwrap()
            .unwrap();
        assert!(loaded_rev.action_plan.is_none());
        assert!(loaded_rev.frozen_packet.is_none());
        assert!(loaded_rev.generated_result.is_none());

        // 1b. Receipt-free action_plan is cleared for merge_proposals_changed (with pre-existing action_plan)
        for (day, name) in [("20260103", "Ada"), ("20260104", "Ada Lovelace")] {
            solstone_core_facets::upsert_detection_segment(
                &journal,
                "work",
                day,
                "090000_300",
                &[solstone_core_facets::DetectedEntityInput {
                    entity_type: "Person".to_owned(),
                    name: name.to_owned(),
                    description: "Collaborator.".to_owned(),
                }],
            )
            .unwrap();
        }
        let review_prep2 = PreparedTalent {
            name: "entities:entities_review".to_owned(),
            config: json!({
                "day":"20260109", "facet":"work", "type":"generate", "max_output_tokens":1024,
                "prompt":"review", "model":"test-model", "provider":"test",
                "hook":{"pre":"entities:entities_review", "post":"entities:entities_review"},
            })
            .as_object()
            .unwrap()
            .clone(),
        };
        let review_packet2 = crate::daily_prepare::freeze(review_prep2, &context).unwrap();
        let (thawed_prep2, thawed_stage2) = crate::daily_prepare::thaw(&review_packet2).unwrap();
        let (spec2, state2) = thawed_stage2.unwrap();
        let commit2 = spec2.commit.unwrap();
        let parsed2 = (commit2.parse)(
            &json!({"promotions":[], "merges":[{"source":"Ada", "canonical":"Ada Lovelace", "evidence":"same"}]}).to_string(),
            &thawed_prep2,
            &state2,
        )
        .unwrap();
        let plan2 = (commit2.commit)(parsed2, &thawed_prep2, &state2).unwrap();
        let pub_plan2 =
            crate::writers::prepare_daily_publication(plan2, &thawed_prep2, &context).unwrap();

        let review_identity2 =
            DailyUnitIdentity::new("20260109", "entities:entities_review", Some("work".into()));
        let mut review_record2 = DailyUnitRecord::new(review_identity2.clone(), "E", "C");
        review_record2.lock_token = Some("attempt-rev2".to_owned());
        review_record2.packet_digest = Some(crate::daily_prepare::packet_digest(&review_packet2));
        review_record2.frozen_packet = Some(review_packet2);
        review_record2.generated_result = Some(json!({"response":"{}"}));
        review_record2.action_plan = Some(serde_json::to_value(&pub_plan2).unwrap());
        save_daily_unit_record(&journal, &review_record2).unwrap();
        propose("20260107", "third"); // candidate list changes, causing publish_merge_proposals to fail

        let outcome2 = execute(
            json!({"name":"entities:entities_review","day":"20260109","facet":"work","lock_token":"attempt-rev2"})
                .as_object()
                .unwrap()
                .clone(),
            &context,
            &OneShotClient::at_path(root.path().join("no-model")),
            &mut Vec::new(),
        );
        let RuntimeOutcome::StageFailed(err2) = outcome2 else {
            panic!("expected StageFailed, got {outcome2:?}");
        };
        assert_eq!(err2.owner_conflict_kind(), Some("merge_proposals_changed"));
        let loaded_rev2 = load_daily_unit_record(&journal, &review_identity2)
            .unwrap()
            .unwrap();
        assert!(loaded_rev2.action_plan.is_none());
        assert!(loaded_rev2.frozen_packet.is_none());

        // 1c. Receipt-free action_plan is cleared for calendar_changed
        schedule_cal("20260105", "first");
        let facet_id = solstone_core_facets::facet_write_identity(&journal, "work").unwrap();
        let sched_prep = PreparedTalent {
            name: "schedule".to_owned(),
            config: json!({
                "day":"20260110", "type":"generate", "max_output_tokens":1024, "prompt":"frozen cal",
                "model":"test-model", "provider":"test", "hook":{"post":"schedule"},
                "_daily_facet_ids":{"work":facet_id.clone()},
                "_daily_calendar_before":{"work/20260120":calendar()},
            })
            .as_object()
            .unwrap()
            .clone(),
        };
        let sched_packet = crate::daily_prepare::freeze(sched_prep, &context).unwrap();
        let sched_identity = DailyUnitIdentity::new("20260110", "schedule", None);
        let mut sched_record = DailyUnitRecord::new(sched_identity.clone(), "E", "C");
        sched_record.lock_token = Some("attempt-sched".to_owned());
        sched_record.packet_digest = Some(crate::daily_prepare::packet_digest(&sched_packet));
        sched_record.frozen_packet = Some(sched_packet);
        sched_record.generated_result = Some(json!({"response":events("third")}));
        save_daily_unit_record(&journal, &sched_record).unwrap();
        schedule_cal("20260106", "second"); // calendar conflict

        let outcome = execute(
            json!({"name":"schedule","day":"20260110","lock_token":"attempt-sched"})
                .as_object()
                .unwrap()
                .clone(),
            &context,
            &OneShotClient::at_path(root.path().join("no-model")),
            &mut Vec::new(),
        );
        let RuntimeOutcome::StageFailed(err) = outcome else {
            panic!("expected StageFailed, got {outcome:?}");
        };
        assert_eq!(err.owner_conflict_kind(), Some("calendar_changed"));
        let loaded_sched = load_daily_unit_record(&journal, &sched_identity)
            .unwrap()
            .unwrap();
        assert!(loaded_sched.action_plan.is_none());
        assert!(loaded_sched.frozen_packet.is_none());
        assert!(loaded_sched.generated_result.is_none());

        // 2. Conflict whose detail is "conflict: owning facet changed after prompt preparation"
        // and whose kind is absent keeps the plan
        let obs_root = tempfile::tempdir().unwrap();
        let (obs_context, obs_prepared, obs_identity) = observer_fixture(obs_root.path());
        let mut obs_packet = crate::daily_prepare::freeze(obs_prepared, &obs_context).unwrap();
        obs_packet["prepared"]["config"]["_daily_facet_ids"]["work"] =
            json!("11111111-2222-4333-8444-555555555555");
        let mut obs_record = DailyUnitRecord::new(obs_identity.clone(), "E", "C");
        obs_record.lock_token = Some("attempt-obs".to_owned());
        obs_record.packet_digest = Some(crate::daily_prepare::packet_digest(&obs_packet));
        obs_record.frozen_packet = Some(obs_packet);
        let obs_out = observer_output(1, "Prefers concise updates", "Prefers concise updates");
        obs_record.generated_result = Some(json!({"response": obs_out}));
        save_daily_unit_record(&obs_context.journal, &obs_record).unwrap();

        let outcome = execute(
            json!({"name":"entities:entity_observer","day":"20260910","facet":"work","lock_token":"attempt-obs"})
                .as_object()
                .unwrap()
                .clone(),
            &obs_context,
            &OneShotClient::at_path(obs_root.path().join("no-model")),
            &mut Vec::new(),
        );
        let RuntimeOutcome::StageFailed(err) = outcome else {
            panic!("expected StageFailed, got {outcome:?}");
        };
        assert!(
            err.detail
                .contains("conflict: owning facet changed after prompt preparation")
        );
        assert_eq!(err.owner_conflict_kind(), None);
        let loaded_obs = load_daily_unit_record(&obs_context.journal, &obs_identity)
            .unwrap()
            .unwrap();
        assert!(
            loaded_obs.frozen_packet.is_some(),
            "facet conflict with absent kind must keep frozen packet"
        );

        // 3. Current-attempt owner_action receipt keeps plan and packet, while receipts only on accepted do not.
        // Also: started receipt without a commit does not take the fresh-prep path.
        let facet_id = solstone_core_facets::facet_write_identity(&journal, "work").unwrap();
        let cal_before = calendar();

        // 3a. Receipts sit only on accepted -> does clear plan and packet
        let prep_acc = PreparedTalent {
            name: "schedule".to_owned(),
            config: json!({
                "day":"20260112", "type":"generate", "max_output_tokens":1024, "prompt":"frozen cal",
                "model":"test-model", "provider":"test", "hook":{"post":"schedule"},
                "_daily_facet_ids":{"work":facet_id.clone()},
                "_daily_calendar_before":{"work/20260120":cal_before.clone()},
            })
            .as_object()
            .unwrap()
            .clone(),
        };
        let packet_acc = crate::daily_prepare::freeze(prep_acc, &context).unwrap();
        let id_acc = DailyUnitIdentity::new("20260112", "schedule", None);
        let mut rec_acc = DailyUnitRecord::new(id_acc.clone(), "E", "C");
        rec_acc.lock_token = Some("attempt-acc".to_owned());
        rec_acc.packet_digest = Some(crate::daily_prepare::packet_digest(&packet_acc));
        rec_acc.frozen_packet = Some(packet_acc);
        rec_acc.generated_result = Some(json!({"response":events("fifth")}));
        rec_acc.accepted = Some(AcceptedDailyResult {
            evidence_revision: "E0".into(),
            contract_digest: "C0".into(),
            status: DailyUnitStatus::Committed,
            packet_digest: Some("a".repeat(64)),
            generated_result: Some(json!({"response": "old", "output": "old"})),
            receipts: vec![
                json!({"kind": "owner_action", "action_id": "0:old", "token": "old", "state": "committed"}),
            ],
            committed_at_ms: 1,
        });
        save_daily_unit_record(&journal, &rec_acc).unwrap();
        schedule_cal("20260107", "conflict-acc"); // trigger calendar conflict

        let outcome = execute(
            json!({"name":"schedule","day":"20260112","lock_token":"attempt-acc"})
                .as_object()
                .unwrap()
                .clone(),
            &context,
            &OneShotClient::at_path(root.path().join("no-model")),
            &mut Vec::new(),
        );
        assert!(matches!(outcome, RuntimeOutcome::StageFailed(_)));
        let loaded_acc = load_daily_unit_record(&journal, &id_acc).unwrap().unwrap();
        assert!(loaded_acc.action_plan.is_none());
        assert!(loaded_acc.frozen_packet.is_none());

        // 3b. Current-attempt owner_action receipt (started) -> keeps plan and packet (no fresh-prep path)
        let cal_before = calendar();
        let prep_cur = PreparedTalent {
            name: "schedule".to_owned(),
            config: json!({
                "day":"20260113", "type":"generate", "max_output_tokens":1024, "prompt":"frozen cal",
                "model":"test-model", "provider":"test", "hook":{"post":"schedule"},
                "_daily_facet_ids":{"work":facet_id},
                "_daily_calendar_before":{"work/20260120":cal_before},
            })
            .as_object()
            .unwrap()
            .clone(),
        };
        let packet_cur = crate::daily_prepare::freeze(prep_cur, &context).unwrap();
        let id_cur = DailyUnitIdentity::new("20260113", "schedule", None);
        let mut rec_cur = DailyUnitRecord::new(id_cur.clone(), "E", "C");
        rec_cur.lock_token = Some("attempt-cur".to_owned());
        rec_cur.packet_digest = Some(crate::daily_prepare::packet_digest(&packet_cur));
        rec_cur.frozen_packet = Some(packet_cur);
        rec_cur.generated_result = Some(json!({"response":events("sixth")}));
        rec_cur.action_plan = Some(json!({"actions": []}));
        rec_cur.receipts.push(json!({"kind": "owner_action", "action_id": "0:cur", "token": "attempt-cur", "state": "started"}));
        save_daily_unit_record(&journal, &rec_cur).unwrap();
        schedule_cal("20260108", "conflict-cur"); // trigger calendar conflict

        let outcome = execute(
            json!({"name":"schedule","day":"20260113","lock_token":"attempt-cur"})
                .as_object()
                .unwrap()
                .clone(),
            &context,
            &OneShotClient::at_path(root.path().join("no-model")),
            &mut Vec::new(),
        );
        assert!(matches!(outcome, RuntimeOutcome::StageFailed(_)));
        let loaded_cur = load_daily_unit_record(&journal, &id_cur).unwrap().unwrap();
        assert!(loaded_cur.action_plan.is_some());
        assert!(
            loaded_cur.frozen_packet.is_some(),
            "started receipt keeps frozen packet"
        );
    }
}
