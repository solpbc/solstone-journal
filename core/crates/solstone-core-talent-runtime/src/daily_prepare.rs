// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Frozen, typed input to supported daily hook stages. Workers never rebuild it.

use crate::{
    ExecutionContext, PreparedTalent, RuntimeOutcome, StageError,
    contract::{self, GateDecision, PrePostState, StageSpec},
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

#[derive(Serialize, Deserialize)]
enum FrozenState {
    None,
    DailySchedule(crate::daily_schedule::DailySchedulePreState),
    FacetNewsletter(crate::facet_newsletter::FacetNewsletterState),
    MorningBriefing(crate::morning_briefing::MorningBriefingPreState),
    EntitiesReview(crate::entities::review::ReviewState),
    EntitySuggest(crate::entities::suggest::SuggestState),
    EntityObserver(crate::entities::observer::ObserverState),
}

fn no_output_reason(reason: &str) -> bool {
    matches!(
        reason,
        "disabled" | "no_input" | "no_candidates" | "no substantive facet/day sources"
    ) || reason.starts_with("missing_required_")
}

pub fn freeze(
    mut prepared: PreparedTalent,
    context: &ExecutionContext,
) -> Result<Value, RuntimeOutcome> {
    let hook = solstone_core_indexer::daily_evidence::daily_hook(&prepared.name, &prepared.config)
        .map_err(|error| failure(&prepared, error))?;
    let stage = contract::resolve_hook(&hook)
        .ok_or_else(|| failure(&prepared, format!("unsupported daily hook {hook}")))?;
    capture_owner_expectations(&mut prepared, context, &hook).map_err(|e| failure(&prepared, e))?;
    let mut state = PrePostState::None;
    if !prepared.config.contains_key("skip_reason") {
        if let Some(gate) = stage.gate {
            match gate(&prepared, context).map_err(RuntimeOutcome::StageFailed)? {
                GateDecision::Proceed => {}
                GateDecision::Skip(reason) => {
                    if hook == "facet_newsletter" && reason == "no substantive facet/day sources" {
                        // A no-output decision still owes the frozen coverage
                        // diagnostics, including sources wholly clipped by the
                        // canonical formatter. The admission E check brackets
                        // this read just as it brackets the normal build.
                        state = crate::facet_newsletter::build(&mut prepared, context)?;
                    }
                    prepared
                        .config
                        .insert("skip_reason".to_owned(), json!(reason));
                }
            }
        }
        if !prepared.config.contains_key("skip_reason") {
            if let Some(build) = stage.build {
                match build(&mut prepared, context) {
                    Ok(value) => state = value,
                    Err(RuntimeOutcome::Skipped { reason, .. }) if no_output_reason(&reason) => {
                        prepared
                            .config
                            .insert("skip_reason".to_owned(), json!(reason));
                    }
                    Err(error) => return Err(error),
                }
            }
            if let Some(apply) = stage.prompt_override {
                apply(&mut prepared, &state).map_err(RuntimeOutcome::StageFailed)?;
            }
        }
    }
    if let Some(reason) = prepared.config.get("skip_reason").and_then(Value::as_str)
        && !no_output_reason(reason)
    {
        return Err(failure(
            &prepared,
            format!("daily preparation refused: {reason}"),
        ));
    }
    let state = match state {
        PrePostState::None => FrozenState::None,
        PrePostState::DailySchedule(v) => FrozenState::DailySchedule(v),
        PrePostState::FacetNewsletter(v) => FrozenState::FacetNewsletter(v),
        PrePostState::MorningBriefing(v) => FrozenState::MorningBriefing(v),
        PrePostState::EntitiesReview(v) => FrozenState::EntitiesReview(v),
        PrePostState::EntitySuggest(v) => FrozenState::EntitySuggest(v),
        PrePostState::EntityObserver(v) => {
            prepared.config.insert(
                "_daily_observation_before".to_owned(),
                Value::Object(v.observation_before.clone()),
            );
            FrozenState::EntityObserver(v)
        }
        _ => {
            return Err(failure(
                &prepared,
                "unsupported frozen daily stage state".to_owned(),
            ));
        }
    };
    Ok(
        json!({"version":1,"prepared":{"name":prepared.name,"config":prepared.config},"hook":hook,"state":state}),
    )
}

fn failure(prepared: &PreparedTalent, detail: String) -> RuntimeOutcome {
    RuntimeOutcome::StageFailed(StageError::new(
        "prepare",
        "daily",
        prepared.name.clone(),
        detail,
    ))
}

pub type ThawedStage = (PreparedTalent, Option<(&'static StageSpec, PrePostState)>);

pub fn thaw(packet: &Value) -> Result<ThawedStage, String> {
    if packet.get("version").and_then(Value::as_u64) != Some(1) {
        return Err("unsupported frozen daily packet version".to_owned());
    }
    let value = packet
        .get("prepared")
        .ok_or("frozen daily packet has no prepared input")?;
    let name = value
        .get("name")
        .and_then(Value::as_str)
        .ok_or("frozen daily packet has no talent")?
        .to_owned();
    let config: Map<String, Value> = value
        .get("config")
        .and_then(Value::as_object)
        .cloned()
        .ok_or("frozen daily packet has no config")?;
    let hook = solstone_core_indexer::daily_evidence::daily_hook(&name, &config)?;
    if packet.get("hook").and_then(Value::as_str) != Some(&hook) {
        return Err("frozen daily packet hook mismatch".to_owned());
    }
    let stage = contract::resolve_hook(&hook).ok_or("unsupported frozen daily hook")?;
    let frozen: FrozenState = serde_json::from_value(
        packet
            .get("state")
            .cloned()
            .ok_or("frozen daily packet has no state")?,
    )
    .map_err(|e| e.to_string())?;
    let (state, expected) = match frozen {
        FrozenState::None => (PrePostState::None, "schedule"),
        FrozenState::DailySchedule(v) => (PrePostState::DailySchedule(v), "daily_schedule"),
        FrozenState::FacetNewsletter(v) => (PrePostState::FacetNewsletter(v), "facet_newsletter"),
        FrozenState::MorningBriefing(v) => (PrePostState::MorningBriefing(v), "morning_briefing"),
        FrozenState::EntitiesReview(v) => {
            (PrePostState::EntitiesReview(v), "entities:entities_review")
        }
        FrozenState::EntitySuggest(v) => {
            (PrePostState::EntitySuggest(v), "entities:entity_suggest")
        }
        FrozenState::EntityObserver(v) => {
            (PrePostState::EntityObserver(v), "entities:entity_observer")
        }
    };
    if let Some(reason) = config.get("skip_reason").and_then(Value::as_str) {
        if !no_output_reason(reason) {
            return Err("invalid frozen daily skip reason".to_owned());
        }
    } else if hook != expected {
        return Err("frozen daily packet state mismatch".to_owned());
    }
    Ok((PreparedTalent { name, config }, Some((stage, state))))
}

pub fn packet_digest(packet: &Value) -> String {
    solstone_core_indexer::daily_evidence::digest(packet)
}

pub(crate) struct SourceSearch {
    pub results: Vec<solstone_core_indexer_query::SearchHit>,
    pub total: Option<u64>,
    pub warnings: Vec<String>,
}

pub(crate) fn search_day_sources(
    journal: &std::path::Path,
    day: &str,
    agent: &str,
    facet: Option<&str>,
    limit: usize,
) -> Result<SourceSearch, String> {
    use solstone_core_indexer_query::{SearchHit, SearchMetadata};
    let agent = agent.to_lowercase();
    let facet = facet.map(str::to_lowercase);
    let projection = solstone_core_indexer::daily_evidence::capture_day_projection(journal, day)?;
    let rows = projection
        .chunks
        .into_iter()
        .filter(|row| row.agent == agent && facet.as_ref().is_none_or(|f| row.facet == *f))
        .collect::<Vec<_>>();
    let total = rows.len() as u64;
    let warnings = projection
        .sources
        .iter()
        .filter(|source| source.agent == agent && facet.as_ref().is_none_or(|f| source.facet == *f))
        .flat_map(|row| {
            row.warnings
                .iter()
                .map(|warning| format!("{}: {warning}", row.path))
        })
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    let results = rows
        .into_iter()
        .take(limit)
        .map(|row| SearchHit {
            row_id: 0,
            id: format!("{}:{}", row.path, row.idx),
            text: row.text,
            metadata: SearchMetadata {
                day: row.day,
                facet: row.facet,
                agent: row.agent,
                stream: solstone_core_indexer::stream::extract_stream(journal, &row.path)
                    .stream
                    .unwrap_or_default(),
                path: row.path,
                idx: row.idx as i64,
            },
            score: 0.0,
        })
        .collect();
    Ok(SourceSearch {
        results,
        total: Some(total),
        warnings,
    })
}

fn capture_owner_expectations(
    prepared: &mut PreparedTalent,
    context: &ExecutionContext,
    hook: &str,
) -> Result<(), String> {
    if prepared.config.contains_key("skip_reason") {
        return Ok(());
    }
    let facet_ids = {
        let _guard = solstone_core_facets::hold_facet_trust_lock(&context.journal)
            .map_err(|e| e.to_string())?;
        let names = if hook == "schedule" {
            solstone_core_facets::list_declared_facet_names(&context.journal)
                .map_err(|e| e.to_string())?
        } else if matches!(
            hook,
            "facet_newsletter"
                | "entities:entities_review"
                | "entities:entity_suggest"
                | "entities:entity_observer"
        ) {
            prepared
                .config
                .get("facet")
                .and_then(Value::as_str)
                .map(|facet| vec![facet.to_owned()])
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        let mut identities = Map::new();
        for facet in names {
            identities.insert(
                facet.clone(),
                json!(solstone_core_facets::facet_write_identity(
                    &context.journal,
                    &facet
                )?),
            );
        }
        identities
    };
    prepared
        .config
        .insert("_daily_facet_ids".to_owned(), Value::Object(facet_ids));
    let mut paths = std::collections::BTreeSet::new();
    if let Some(path) = prepared.config.get("output_path").and_then(Value::as_str) {
        paths.insert(std::path::PathBuf::from(path));
    }
    if let (Some(facet), Some(day)) = (
        prepared.config.get("facet").and_then(Value::as_str),
        prepared.config.get("day").and_then(Value::as_str),
    ) {
        let relative = match hook {
            "facet_newsletter" => Some(format!("facets/{facet}/news/{day}.md")),
            "entities:entity_suggest" => Some(format!(
                "facets/{facet}/entities/{day}_observer_suggestions.json"
            )),
            "entities:entity_observer" => Some(format!(
                "facets/{facet}/entities/{day}_observer_outcome.json"
            )),
            "entities:entities_review" => {
                Some(format!("facets/{facet}/entities/{day}_review_outcome.json"))
            }
            _ => None,
        };
        if let Some(relative) = relative {
            paths.insert(context.journal.join(relative));
        }
    }
    let mut artifacts = Map::new();
    for path in paths {
        let action = crate::writers::prepare_output_action(&context.journal, &path, Vec::new())?;
        let crate::writers::PreparedDailyAction::Output { path, before, .. } = action else {
            return Err("invalid artifact preparation".to_owned());
        };
        artifacts.insert(path, json!(before));
    }
    prepared.config.insert(
        "_daily_artifact_before".to_owned(),
        Value::Object(artifacts),
    );
    if hook == "daily_schedule" {
        let before = solstone_core_system::schedule::prepare_daily_time(
            &context.journal.join("config/schedules.json"),
            "00:00",
        )?
        .before;
        prepared.config.insert(
            "_daily_time_before".to_owned(),
            before.unwrap_or(Value::Null),
        );
    }
    if hook == "schedule" {
        let day = prepared
            .config
            .get("day")
            .and_then(Value::as_str)
            .ok_or("daily calendar preparation requires day")?;
        let _lock = solstone_core_facets::hold_facet_trust_lock(&context.journal)
            .map_err(|e| e.to_string())?;
        let mut before = Map::new();
        for facet in strict_entries(&context.journal.join("facets"))? {
            if !facet.file_type().map_err(|e| e.to_string())?.is_dir() {
                continue;
            }
            let facet_name = facet
                .file_name()
                .to_str()
                .ok_or("invalid facet filename")?
                .to_owned();
            for entry in strict_entries(&facet.path().join("activities"))? {
                let path = entry.path();
                if path.extension().and_then(|s| s.to_str()) != Some("jsonl") {
                    continue;
                }
                let target = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .ok_or("invalid activity filename")?;
                if chrono::NaiveDate::parse_from_str(target, "%Y%m%d").is_err() || target <= day {
                    continue;
                }
                let raw = solstone_core_facets::read_activity_file(
                    &context.journal,
                    &facet_name,
                    &format!("{target}.jsonl"),
                )
                .map_err(|e| e.to_string())?;
                if let Some(raw) = &raw {
                    for line in raw.lines().filter(|s| !s.trim().is_empty()) {
                        let row: Value = serde_json::from_str(line)
                            .map_err(|e| format!("calendar {facet_name}/{target}: {e}"))?;
                        if !row.is_object() {
                            return Err(format!(
                                "calendar {facet_name}/{target} row is not an object"
                            ));
                        }
                    }
                }
                before.insert(
                    format!("{facet_name}/{target}"),
                    raw.map(Value::String).unwrap_or(Value::Null),
                );
            }
        }
        prepared
            .config
            .insert("_daily_calendar_before".to_owned(), Value::Object(before));
    }
    Ok(())
}

fn strict_entries(path: &std::path::Path) -> Result<Vec<std::fs::DirEntry>, String> {
    match std::fs::read_dir(path) {
        Ok(entries) => entries
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn write(root: &std::path::Path, rel: &str, text: &str) {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    #[test]
    fn newsletter_no_output_retains_all_clipped_source_diagnostics() {
        let root = tempfile::tempdir().unwrap();
        solstone_core_facets::create_facet(root.path(), "work", "Work", "", "", "", None).unwrap();
        write(
            root.path(),
            "chronicle/20260910/mic/090000_60/talents/work/flow.md",
            &"long source line ".repeat(200),
        );
        let prepared = PreparedTalent {
            name: "facet_newsletter".to_owned(),
            config: json!({
                "type":"generate", "day":"20260910", "facet":"work", "prompt":"$source_packet",
                "hook":{"pre":"facet_newsletter","post":"facet_newsletter"}
            })
            .as_object()
            .unwrap()
            .clone(),
        };
        let packet = freeze(
            prepared,
            &ExecutionContext {
                journal: root.path().to_owned(),
            },
        )
        .unwrap();
        assert_eq!(
            packet
                .pointer("/prepared/config/skip_reason")
                .and_then(Value::as_str),
            Some("no substantive facet/day sources")
        );
        let gaps = packet
            .pointer("/state/FacetNewsletter/packet/gaps")
            .and_then(Value::as_array)
            .expect("no-output coverage packet retained");
        assert!(
            gaps.iter()
                .filter_map(Value::as_str)
                .any(|gap| gap.contains("clipped:") && gap.contains("flow.md")),
            "{gaps:?}"
        );
        assert!(thaw(&packet).is_ok());
    }
    #[test]
    fn frozen_newsletter_consumes_source_projection_without_index_and_thaws_without_reads() {
        let root = tempfile::tempdir().unwrap();
        write(
            root.path(),
            "facets/work/facet.json",
            r#"{"id":"12345678-1234-4234-8234-123456789abc","title":"Work","description":"test scope"}"#,
        );
        let rel = "chronicle/20260910/mic/090000_60/talents/work/flow.md";
        write(root.path(), rel, "# Work\n\nA freshly arrived source fact.");
        let context = ExecutionContext {
            journal: root.path().to_owned(),
        };
        let prepared = PreparedTalent {
            name: "facet_newsletter".to_owned(),
            config: Map::from_iter([
                ("day".to_owned(), json!("20260910")),
                ("facet".to_owned(), json!("work")),
                (
                    "hook".to_owned(),
                    json!({"pre":"facet_newsletter","post":"facet_newsletter"}),
                ),
                ("user_instruction".to_owned(), json!("${source_packet}")),
            ]),
        };
        let packet = freeze(prepared, &context).unwrap();
        assert!(
            !packet["prepared"]["config"]
                .as_object()
                .unwrap()
                .contains_key("skip_reason")
        );
        assert!(packet.to_string().contains("freshly arrived source fact"));
        assert!(
            packet
                .to_string()
                .contains("20260910/mic/090000_60/talents/work/flow.md:0")
        );
        let original_digest = packet_digest(&packet);
        std::fs::remove_file(root.path().join(rel)).unwrap();
        let (restored, state) = thaw(&packet).unwrap();
        assert!(
            restored.config["user_instruction"]
                .as_str()
                .unwrap()
                .contains("freshly arrived source fact")
        );
        assert!(matches!(state.unwrap().1, PrePostState::FacetNewsletter(_)));
        assert_eq!(packet_digest(&packet), original_digest);
        assert!(!root.path().join("indexer").exists());
    }
    #[test]
    fn frozen_packet_rejects_hook_state_mismatch_and_retains_legitimate_no_output() {
        let root = tempfile::tempdir().unwrap();
        let context = ExecutionContext {
            journal: root.path().to_owned(),
        };
        let config = Map::from_iter([
            ("hook".to_owned(), json!({"post":"schedule"})),
            ("skip_reason".to_owned(), json!("no_input")),
        ]);
        let packet = freeze(
            PreparedTalent {
                name: "schedule".to_owned(),
                config,
            },
            &context,
        )
        .unwrap();
        assert_eq!(thaw(&packet).unwrap().0.config["skip_reason"], "no_input");
        let mut corrupt = packet;
        corrupt["prepared"]["config"]
            .as_object_mut()
            .unwrap()
            .remove("skip_reason");
        corrupt["prepared"]["config"]["hook"] = json!({"pre":"morning_briefing"});
        corrupt["hook"] = json!("morning_briefing");
        corrupt["prepared"]["name"] = json!("morning_briefing");
        assert!(thaw(&corrupt).err().unwrap().contains("state mismatch"));
    }
    #[test]
    fn source_projection_returns_full_counts_but_bounds_recent_original_chunk_ids() {
        let root = tempfile::tempdir().unwrap();
        let text = (0..12)
            .map(|i| json!({"ts":i+1,"summary":format!("fact-{i}")}).to_string())
            .collect::<Vec<_>>()
            .join("\n");
        write(
            root.path(),
            "chronicle/20260910/talents/Followups.jsonl",
            &text,
        );
        let result = search_day_sources(root.path(), "20260910", "FOLLOWUPS", None, 10).unwrap();
        assert_eq!(result.total, Some(12));
        assert_eq!(result.results.len(), 10);
        assert_eq!(result.results[0].id, "20260910/talents/Followups.jsonl:11");
        assert_eq!(result.results[9].id, "20260910/talents/Followups.jsonl:2");
        let empty =
            search_day_sources(root.path(), "20260910", "followups", Some("other"), 10).unwrap();
        assert_eq!(empty.total, Some(0));
        assert!(empty.results.is_empty());
    }
}
