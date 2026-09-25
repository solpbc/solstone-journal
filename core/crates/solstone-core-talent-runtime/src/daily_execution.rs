// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Execute the packet reserved by daily admission and resume its publication.

use std::io::Write;

use serde_json::{Map, Value, json};
use solstone_core_cogitate_wire::CogitateOneShotClient;
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

fn apply_stage_failure(record: &mut DailyUnitRecord, error: &StageError) {
    record.status = if error.phase == "conflict" {
        DailyUnitStatus::Conflicting
    } else {
        DailyUnitStatus::Failed
    };
    record.error_detail = Some(error.to_string());
    let reason = error.reason_code();
    let kind_changed = record.owner_conflict_kind.as_deref() != error.owner_conflict_kind();
    let reason_changed = record.reason_code.as_deref() != Some(reason);
    if kind_changed || reason_changed {
        record.failure_count = 0;
    }
    record.reason_code = Some(reason.to_owned());
    record.owner_conflict_kind = error.owner_conflict_kind().map(str::to_owned);
    if error.phase == "conflict" && record.identity.name == "entities:entities_review" {
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
    })
}

pub(crate) fn execute(
    request: Map<String, Value>,
    context: &ExecutionContext,
    generate: &OneShotClient,
    cogitate: &CogitateOneShotClient,
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
        let engine = match crate::cogitate::from_prepared_config(&prepared.config) {
            Ok(engine) => engine,
            Err(outcome) => return outcome,
        };
        match crate::generate_response(&mut prepared, context, generate, cogitate, writer, engine) {
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

fn publish(
    authority: &mut DailyUnitAuthority,
    token: &str,
    prepared: &PreparedTalent,
    stage: Option<&(&'static StageSpec, PrePostState)>,
    context: &ExecutionContext,
    generated: crate::GeneratedTalentResponse,
    skip: bool,
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
    let disposition =
        crate::writers::publish_daily_publication(authority, token, &publication, context)?;
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
                "day":"20260101", "type":"generate", "prompt":"frozen source evidence",
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
            config: json!({"day":"20260910", "facet":"work", "type":"generate", "prompt":"$observer_context", "model":"test-model", "provider":"test", "hook":{"pre":"entities:entity_observer", "post":"entities:entity_observer"}}).as_object().unwrap().clone(),
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
        let cogitate = CogitateOneShotClient::at_path(root.path().join("no-cogitate"));
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
            &cogitate,
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
            &cogitate,
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
            &cogitate,
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
            let cogitate = CogitateOneShotClient::at_path(root.path().join("no-cogitate"));
            for _ in 0..2 {
                let outcome = execute(
                    observer_request("attempt"),
                    &context,
                    &model,
                    &cogitate,
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
            let cogitate = CogitateOneShotClient::at_path(root.path().join("no-cogitate"));
            for _ in 0..2 {
                let outcome = execute(
                    observer_request("attempt"),
                    &context,
                    &model,
                    &cogitate,
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
        let absent_cogitate =
            CogitateOneShotClient::at_path(root.path().join("not-installed-cogitate"));
        let outcome = execute(
            request.clone(),
            &context,
            &absent_generate,
            &absent_cogitate,
            &mut Vec::new(),
        );
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
        let outcome = execute(
            request,
            &context,
            &absent_generate,
            &absent_cogitate,
            &mut Vec::new(),
        );
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
            &absent_cogitate,
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
                "day":"20260101", "type":"generate", "prompt":"frozen calendar evidence",
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
        let absent_cogitate = CogitateOneShotClient::at_path(root.path().join("no-cogitate"));
        let outcome = execute(
            request.clone(),
            &context,
            &OneShotClient::at_path(root.path().join("no-model")),
            &absent_cogitate,
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
            &absent_cogitate,
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
            &CogitateOneShotClient::at_path(root.path().join("no-cogitate")),
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
                &CogitateOneShotClient::at_path(std::path::PathBuf::from("not-installed-cogitate")),
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
            let cogitate = CogitateOneShotClient::at_path(root.path().join("absent-cogitate"));
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
                Ok(&cogitate),
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
}
