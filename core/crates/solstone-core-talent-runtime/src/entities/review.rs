// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Entity-review hook.
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use crate::contract::{CommitPlan, GateDecision, ParsedOutput, PrePostState};
use crate::writers::WriteIntent;
use crate::{
    ExecutionContext, PreparedTalent, RuntimeOutcome, StageError, apply_template_vars, stage_error,
};
use caseless::default_case_fold_str;
use chrono::{Duration, NaiveDate};
use serde_json::{Map, Value};
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ReviewState {
    packet: String,
}

const REVIEW_WINDOW_DAYS: i64 = 7;

fn format_name_key(name: &str) -> (String, String) {
    (default_case_fold_str(name), name.to_owned())
}

fn review_days(day: &str) -> Vec<String> {
    let Ok(day) = NaiveDate::parse_from_str(day, "%Y%m%d") else {
        return Vec::new();
    };
    (1..=REVIEW_WINDOW_DAYS)
        .rev()
        .map(|offset| (day - Duration::days(offset)).format("%Y%m%d").to_string())
        .collect()
}

fn threshold(entity_type: &str) -> usize {
    match entity_type {
        "Person" => 2,
        "Company" | "Project" => 3,
        "Tool" => 5,
        _ => 5,
    }
}

#[derive(Clone, Debug, PartialEq)]
struct ReviewInputs {
    eligible: Vec<Value>,
    hints: Vec<(String, String)>,
    prior: Vec<Value>,
}

type CandidateBucket = (BTreeSet<String>, BTreeSet<String>, Vec<Value>);

fn build_review_inputs(journal: &Path, facet: &str, day: &str) -> Result<ReviewInputs, String> {
    let mut buckets: BTreeMap<String, CandidateBucket> = BTreeMap::new();
    let mut names = BTreeSet::new();
    for review_day in review_days(day) {
        for entity in solstone_core_facets::read_detected_entities(journal, facet, &review_day)
            .map_err(|error| error.to_string())?
        {
            let name = entity
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim();
            let entity_type = entity
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim();
            let description = entity
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim();
            let slug = solstone_core_entity_matching::entity_slug(name);
            if name.is_empty() || entity_type.is_empty() || slug.is_empty() {
                continue;
            }
            let bucket = buckets.entry(slug).or_default();
            bucket.0.insert(review_day.clone());
            bucket.1.insert(entity_type.to_owned());
            bucket
                .2
                .push(serde_json::json!({"day":review_day,"name":name,"description":description}));
            names.insert(name.to_owned());
        }
    }
    let attached = solstone_core_facets::list_scoped_facet_entities(journal, facet, false, false)
        .map_err(|error| error.to_string())?;
    let candidates = attached
        .iter()
        .map(
            |entity| solstone_core_entity_matching::EntityNameCandidate {
                id: Some(entity.entity_id.clone()),
                name: entity
                    .identity
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                aka: entity
                    .identity
                    .get("aka")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect(),
                emails: entity
                    .identity
                    .get("emails")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect(),
            },
        )
        .collect::<Vec<_>>();
    let mut eligible = Vec::new();
    for (slug, (days, types, mut contexts)) in buckets {
        if types.len() != 1 {
            continue;
        }
        let entity_type = types.into_iter().next().unwrap();
        if days.len() < threshold(&entity_type) {
            continue;
        }
        contexts.sort_by_key(|context| {
            (
                context["day"].as_str().unwrap_or_default().to_owned(),
                format_name_key(context["name"].as_str().unwrap_or_default()),
                context["description"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
            )
        });
        let latest = contexts
            .iter()
            .map(|context| context["day"].as_str().unwrap_or_default())
            .max()
            .unwrap_or_default();
        let name = contexts
            .iter()
            .filter(|context| context["day"].as_str() == Some(latest))
            .map(|context| context["name"].as_str().unwrap_or_default())
            .min_by_key(|name| format_name_key(name))
            .unwrap_or_default();
        if solstone_core_entity_matching::find_matching_entity(name, &candidates, 90.0)
            .is_some_and(|matched| matched.tier.is_high_confidence())
        {
            continue;
        }
        eligible.push(serde_json::json!({"name":name,"slug":slug,"type":entity_type,"day_count":days.len(),"contexts":contexts}));
    }
    eligible.sort_by_key(|item| {
        (
            format_name_key(item["name"].as_str().unwrap_or_default()),
            item["type"].as_str().unwrap_or_default().to_owned(),
        )
    });
    let names = names.into_iter().collect::<Vec<_>>();
    let mut hints = Vec::new();
    for (index, left) in names.iter().enumerate() {
        for right in names.iter().skip(index + 1) {
            if solstone_core_entity_matching::entity_slug(left)
                != solstone_core_entity_matching::entity_slug(right)
                && solstone_core_entity_matching::is_name_variant_match(left, right)
            {
                let pair = if format_name_key(left) <= format_name_key(right) {
                    (left.clone(), right.clone())
                } else {
                    (right.clone(), left.clone())
                };
                if !hints.contains(&pair) {
                    hints.push(pair);
                }
            }
        }
    }
    hints.sort_by_key(|pair| (format_name_key(&pair.0), format_name_key(&pair.1)));
    let mut prior = solstone_core_entity::load_merge_candidates(journal, Some(facet), None)
        .map_err(|error| error.to_string())?;
    prior.sort_by_key(|row| {
        (
            default_case_fold_str(row["source"].as_str().unwrap_or_default()),
            default_case_fold_str(row["target"].as_str().unwrap_or_default()),
            row["status"].as_str().unwrap_or("open").to_owned(),
        )
    });
    Ok(ReviewInputs {
        eligible,
        hints,
        prior,
    })
}

fn format_review_packet(inputs: &ReviewInputs) -> String {
    let mut lines = vec!["These are recurring people and things noticed across recent days in one area of the owner's life.".to_owned(), "Judge from the facts below. Save stable, useful context; leave ambiguity out.".to_owned(), String::new(), "## Recurring candidates".to_owned(), String::new()];
    if inputs.eligible.is_empty() {
        lines.extend(["None.".to_owned(), String::new()]);
    }
    for item in &inputs.eligible {
        lines.extend([
            format!("### {}", item["name"].as_str().unwrap_or_default()),
            format!("Type: {}", item["type"].as_str().unwrap_or_default()),
            format!("Distinct days seen: {}", item["day_count"]),
            "What happened:".to_owned(),
        ]);
        for context in item["contexts"].as_array().into_iter().flatten() {
            lines.push(format!(
                "- {}: {} — {}",
                context["day"].as_str().unwrap_or_default(),
                context["name"].as_str().unwrap_or_default(),
                context["description"]
                    .as_str()
                    .filter(|value| !value.is_empty())
                    .unwrap_or("No description saved.")
            ));
        }
        lines.push(String::new());
    }
    lines.extend(["## Possible name variants".to_owned(), String::new()]);
    if inputs.hints.is_empty() {
        lines.push("None.".to_owned())
    } else {
        lines.extend(
            inputs
                .hints
                .iter()
                .map(|(left, right)| format!("- {left} / {right}")),
        );
    }
    lines.push(String::new());
    lines.extend(["## Prior merge decisions".to_owned(), String::new()]);
    if inputs.prior.is_empty() {
        lines.push("None.".to_owned())
    } else {
        lines.extend(inputs.prior.iter().map(|row| {
            format!(
                "- {} -> {} ({})",
                row["source"].as_str().unwrap_or_default(),
                row["target"].as_str().unwrap_or_default(),
                row["status"].as_str().unwrap_or("open")
            )
        }));
    }
    lines.join("\n").trim().to_owned() + "\n"
}
pub fn gate(
    prepared: &PreparedTalent,
    context: &ExecutionContext,
) -> Result<GateDecision, StageError> {
    let Some(day) = prepared
        .config
        .get("day")
        .and_then(Value::as_str)
        .filter(|day| !day.is_empty())
    else {
        return Ok(GateDecision::Skip("no_day".to_owned()));
    };
    let Some(facet) = prepared
        .config
        .get("facet")
        .and_then(Value::as_str)
        .filter(|facet| !facet.is_empty())
    else {
        return Ok(GateDecision::Skip("no_facet".to_owned()));
    };
    let inputs = build_review_inputs(&context.journal, facet, day)
        .map_err(|detail| stage_error("gate", "entities:entities_review", prepared, detail))?;
    if inputs.eligible.is_empty() && inputs.hints.is_empty() {
        return Ok(GateDecision::Skip("no_candidates".to_owned()));
    }
    Ok(GateDecision::Proceed)
}
pub fn build(
    prepared: &mut PreparedTalent,
    context: &ExecutionContext,
) -> Result<PrePostState, RuntimeOutcome> {
    let facet = prepared
        .config
        .get("facet")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let day = prepared
        .config
        .get("day")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let inputs = build_review_inputs(&context.journal, facet, day).map_err(|e| {
        RuntimeOutcome::StageFailed(stage_error(
            "build",
            "entities:entities_review",
            prepared,
            e.to_string(),
        ))
    })?;
    let mut owner_before = Map::new();
    for row in &inputs.eligible {
        let name = row["name"].as_str().ok_or_else(|| {
            RuntimeOutcome::StageFailed(stage_error(
                "build",
                "entities:entities_review",
                prepared,
                "candidate missing name",
            ))
        })?;
        let slug = row["slug"].as_str().ok_or_else(|| {
            RuntimeOutcome::StageFailed(stage_error(
                "build",
                "entities:entities_review",
                prepared,
                "candidate missing slug",
            ))
        })?;
        let snapshot =
            solstone_core_facets::review_promotion_snapshot(&context.journal, facet, name)
                .map_err(|e| {
                    RuntimeOutcome::StageFailed(stage_error(
                        "build",
                        "entities:entities_review",
                        prepared,
                        e,
                    ))
                })?;
        owner_before.insert(slug.to_owned(), snapshot);
    }
    let after = build_review_inputs(&context.journal, facet, day).map_err(|e| {
        RuntimeOutcome::StageFailed(stage_error(
            "build",
            "entities:entities_review",
            prepared,
            e,
        ))
    })?;
    if inputs != after {
        return Err(RuntimeOutcome::StageFailed(stage_error(
            "build",
            "entities:entities_review",
            prepared,
            "review inputs changed during preparation",
        )));
    }
    prepared.config.insert(
        "_daily_review_owner_before".to_owned(),
        Value::Object(owner_before),
    );
    prepared.config.insert(
        "_daily_review_inputs".to_owned(),
        serde_json::json!({"eligible":inputs.eligible,"hints":inputs.hints,"prior":inputs.prior}),
    );
    Ok(PrePostState::EntitiesReview(ReviewState {
        packet: format_review_packet(&inputs),
    }))
}
pub fn apply_prompt_override(
    prepared: &mut PreparedTalent,
    state: &PrePostState,
) -> Result<(), StageError> {
    let PrePostState::EntitiesReview(state) = state else {
        return Err(stage_error(
            "prompt_override",
            "entities:entities_review",
            prepared,
            "missing review state",
        ));
    };
    apply_template_vars(
        &mut prepared.config,
        &Map::from_iter([(
            "review_packet".to_owned(),
            Value::String(state.packet.clone()),
        )]),
    );
    Ok(())
}
pub fn parse(
    output: &str,
    _: &PreparedTalent,
    _: &PrePostState,
) -> Result<ParsedOutput, StageError> {
    Ok(ParsedOutput::Text(output.to_owned()))
}
pub fn commit(
    parsed: ParsedOutput,
    prepared: &PreparedTalent,
    _: &PrePostState,
) -> Result<CommitPlan, StageError> {
    let ParsedOutput::Text(output) = parsed else {
        return Err(stage_error(
            "commit",
            "entities:entities_review",
            prepared,
            "expected text output",
        ));
    };
    let (Some(facet), Some(day)) = (
        prepared.config.get("facet").and_then(Value::as_str),
        prepared.config.get("day").and_then(Value::as_str),
    ) else {
        return Ok(CommitPlan::NoOutput);
    };
    Ok(CommitPlan::Write(WriteIntent::EntitiesReview {
        output,
        facet: facet.to_owned(),
        day: day.to_owned(),
    }))
}
pub fn prepare_publication(
    journal: &Path,
    output: &str,
    facet: &str,
    day: &str,
    prepared: &PreparedTalent,
) -> Result<Vec<crate::writers::PreparedDailyAction>, solstone_core_entity::ReviewOwnerError> {
    use crate::writers::PreparedDailyAction;
    let facet_id = solstone_core_facets::facet_write_identity(journal, facet).map_err(|_| {
        solstone_core_entity::ReviewOwnerError::conflict(
            solstone_core_entity::ReviewOwnerConflictKind::OwningFacetChanged,
            "conflict: owning facet changed after prompt preparation",
        )
    })?;
    let data: Value = serde_json::from_str(output).map_err(|e| e.to_string())?;
    let promotions = data
        .get("promotions")
        .and_then(Value::as_array)
        .ok_or("review output lacks promotions")?;
    let merges = data
        .get("merges")
        .and_then(Value::as_array)
        .ok_or("review output lacks merges")?;
    let inputs = prepared
        .config
        .get("_daily_review_inputs")
        .ok_or("missing frozen review admission inputs")?;
    let owner_before = prepared
        .config
        .get("_daily_review_owner_before")
        .and_then(Value::as_object)
        .ok_or("missing frozen review owner state")?;
    let eligible = inputs
        .get("eligible")
        .and_then(Value::as_array)
        .ok_or("missing frozen review eligibility")?;
    let hints: Vec<(String, String)> = serde_json::from_value(
        inputs
            .get("hints")
            .cloned()
            .ok_or("missing frozen review hints")?,
    )
    .map_err(|e| e.to_string())?;
    let mut actions = Vec::new();
    let mut promoted = 0usize;
    let mut aliased = 0usize;
    let mut skipped = 0usize;
    let mut seen = BTreeSet::new();
    for row in promotions {
        let name = row
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        let slug = solstone_core_entity_matching::entity_slug(name);
        let description = row
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if row.get("promote").and_then(Value::as_bool) != Some(true)
            || description.is_empty()
            || !seen.insert(slug.clone())
        {
            skipped += 1;
            continue;
        }
        let Some(candidate) = eligible
            .iter()
            .find(|candidate| candidate.get("slug").and_then(Value::as_str) == Some(slug.as_str()))
        else {
            skipped += 1;
            continue;
        };
        let aliases = row
            .get("aliases")
            .and_then(Value::as_array)
            .ok_or("review promotion lacks aliases")?
            .iter()
            .filter_map(Value::as_str)
            .map(str::trim)
            .filter(|alias| !alias.is_empty())
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let _facet_guard =
            solstone_core_facets::hold_facet_trust_lock(journal).map_err(|e| e.to_string())?;
        let _entity_guard =
            solstone_core_entity::hold_entity_trust_lock(journal).map_err(|e| e.to_string())?;
        let canonical_name = candidate
            .get("name")
            .and_then(Value::as_str)
            .ok_or("frozen candidate lacks name")?;
        let expected = owner_before
            .get(&slug)
            .ok_or("missing frozen promotion owner state")?;
        if &solstone_core_facets::review_promotion_snapshot(journal, facet, canonical_name)?
            != expected
        {
            return Err(solstone_core_entity::ReviewOwnerError::conflict(
                solstone_core_entity::ReviewOwnerConflictKind::PromotionOwnerState,
                "conflict: promotion owner state changed after prompt preparation",
            ));
        }
        let promotion = solstone_core_facets::prepare_review_promotion(
            journal,
            facet,
            candidate
                .get("type")
                .and_then(Value::as_str)
                .ok_or("frozen candidate lacks type")?,
            candidate
                .get("name")
                .and_then(Value::as_str)
                .ok_or("frozen candidate lacks name")?,
            description,
            &aliases,
        )?;
        if let Some(change) = promotion.identity {
            actions.push(PreparedDailyAction::Identity {
                facet: facet.into(),
                facet_id: facet_id.clone(),
                change,
            });
        }
        actions.push(PreparedDailyAction::Attachment {
            change: promotion.attachment,
        });
        if let Some(change) = promotion.aliases {
            aliased += aliases.len();
            actions.push(PreparedDailyAction::Aliases {
                facet: facet.into(),
                facet_id: facet_id.clone(),
                change,
            });
        }
        promoted += 1;
    }
    let hints = hints
        .into_iter()
        .map(|(a, b)| {
            BTreeSet::from([
                solstone_core_entity_matching::entity_slug(&a),
                solstone_core_entity_matching::entity_slug(&b),
            ])
        })
        .collect::<Vec<_>>();
    let mut proposals = Vec::new();
    for row in merges {
        let source = row
            .get("source")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        let target = row
            .get("canonical")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        let summary = row
            .get("evidence")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        let a = solstone_core_entity_matching::entity_slug(source);
        let b = solstone_core_entity_matching::entity_slug(target);
        if a.is_empty()
            || b.is_empty()
            || a == b
            || summary.is_empty()
            || !hints.contains(&BTreeSet::from([a.clone(), b.clone()]))
        {
            skipped += 1;
            continue;
        }
        proposals.push(serde_json::json!({"facet":facet,"day":day,"source":source,"source_slug":a,"target":target,"target_slug":b,"summary":summary}));
    }
    if !proposals.is_empty() {
        let _entity_guard =
            solstone_core_entity::hold_entity_trust_lock(journal).map_err(|e| e.to_string())?;
        let prior = inputs
            .get("prior")
            .and_then(Value::as_array)
            .ok_or("missing frozen proposal state")?;
        let current = solstone_core_entity::load_merge_candidates(journal, Some(facet), None)
            .map_err(|e| e.to_string())?;
        for proposal in &proposals {
            let same_key = |row: &&Value| {
                row.get("source_slug") == proposal.get("source_slug")
                    && row.get("target_slug") == proposal.get("target_slug")
            };
            let expected = prior.iter().find(same_key);
            let actual = current.iter().find(same_key);
            if actual.is_some_and(|row| {
                matches!(
                    row.get("status").and_then(Value::as_str),
                    Some("accepted" | "dismissed")
                )
            }) {
                continue;
            }
            if actual != expected {
                return Err(solstone_core_entity::ReviewOwnerError::conflict(
                    solstone_core_entity::ReviewOwnerConflictKind::MergeProposalPreparation,
                    "conflict: merge proposal changed after prompt preparation",
                ));
            }
        }
        actions.push(PreparedDailyAction::MergeProposals {
            facet: facet.into(),
            facet_id: facet_id.clone(),
            batch: solstone_core_entity::prepare_merge_proposals(journal, &proposals)?,
        });
    }
    let outcome = serde_json::json!({"promoted":promoted,"aliased":aliased,"merges":proposals.len(),"skipped":skipped,"errored":0,"error":null,"ts":chrono::Utc::now().timestamp_millis()});
    let path = journal
        .join("facets")
        .join(facet)
        .join("entities")
        .join(format!("{day}_review_outcome.json"));
    actions.push(crate::writers::prepare_frozen_output_action(
        journal,
        &path,
        format!("{outcome}\n").into_bytes(),
        prepared,
    )?);
    Ok(actions)
}

pub fn apply_result(
    journal: &std::path::Path,
    output: &str,
    facet: &str,
    day: &str,
) -> Result<(), String> {
    let mut counts = BTreeMap::from([
        ("promoted", 0usize),
        ("aliased", 0),
        ("merges", 0),
        ("skipped", 0),
        ("errored", 0),
    ]);
    let mut error = None;
    let result = (|| -> Result<(), String> {
        let Value::Object(data) =
            serde_json::from_str(output).map_err(|_| "invalid JSON".to_owned())?
        else {
            return Err("result is not a JSON object".to_owned());
        };
        let (Some(promotions), Some(merges)) = (
            data.get("promotions").and_then(Value::as_array),
            data.get("merges").and_then(Value::as_array),
        ) else {
            return Err("result missing expected arrays".to_owned());
        };
        let inputs = build_review_inputs(journal, facet, day)?;
        let eligible = inputs
            .eligible
            .iter()
            .filter_map(|row| row["slug"].as_str().map(|slug| (slug.to_owned(), row)))
            .collect::<BTreeMap<_, _>>();
        for row in promotions {
            let Some(row) = row.as_object() else {
                *counts.get_mut("skipped").unwrap() += 1;
                continue;
            };
            let (Some(name), Some(description), Some(promote), Some(aliases)) = (
                row.get("name").and_then(Value::as_str),
                row.get("description").and_then(Value::as_str),
                row.get("promote").and_then(Value::as_bool),
                row.get("aliases").and_then(Value::as_array),
            ) else {
                *counts.get_mut("skipped").unwrap() += 1;
                continue;
            };
            let slug = solstone_core_entity_matching::entity_slug(name.trim());
            let Some(candidate) = eligible.get(&slug) else {
                *counts.get_mut("skipped").unwrap() += 1;
                continue;
            };
            if !promote || description.trim().is_empty() {
                *counts.get_mut("skipped").unwrap() += 1;
                continue;
            }
            let canonical_name = candidate["name"].as_str().unwrap_or_default();
            let canonical_slug = candidate["slug"].as_str().unwrap_or_default();
            let entity_id = match solstone_core_facets::attach_or_reactivate_entity(
                journal,
                facet,
                candidate["type"].as_str().unwrap_or_default(),
                canonical_name,
                description.trim(),
            ) {
                Ok(outcome) => {
                    *counts.get_mut("promoted").unwrap() += 1;
                    outcome.relationship["entity_id"]
                        .as_str()
                        .map(str::to_owned)
                }
                Err(solstone_core_facets::FacetEntityWriteError::EntityExists { .. }) => {
                    *counts.get_mut("skipped").unwrap() += 1;
                    Some(canonical_slug.to_owned())
                }
                Err(error_value) => {
                    *counts.get_mut("errored").unwrap() += 1;
                    error = Some(format!("FacetEntityWriteError: {error_value}"));
                    None
                }
            };
            let Some(entity_id) = entity_id else { continue };
            for alias in aliases {
                let Some(alias) = alias
                    .as_str()
                    .map(str::trim)
                    .filter(|alias| !alias.is_empty())
                else {
                    *counts.get_mut("skipped").unwrap() += 1;
                    continue;
                };
                if solstone_core_entity_matching::entity_slug(alias) == canonical_slug {
                    *counts.get_mut("skipped").unwrap() += 1;
                    continue;
                }
                match solstone_core_facets::add_entity_aka(journal, facet, &entity_id, alias) {
                    Ok(_) => *counts.get_mut("aliased").unwrap() += 1,
                    Err(solstone_core_facets::FacetEntityWriteError::AkaConflict { .. }) => {
                        *counts.get_mut("skipped").unwrap() += 1
                    }
                    Err(error_value) => {
                        *counts.get_mut("errored").unwrap() += 1;
                        error = Some(format!("FacetEntityWriteError: {error_value}"));
                    }
                }
            }
        }
        let hints = inputs
            .hints
            .into_iter()
            .map(|(left, right)| {
                BTreeSet::from([
                    solstone_core_entity_matching::entity_slug(&left),
                    solstone_core_entity_matching::entity_slug(&right),
                ])
            })
            .collect::<Vec<_>>();
        for row in merges {
            let Some(row) = row.as_object() else {
                *counts.get_mut("skipped").unwrap() += 1;
                continue;
            };
            let (Some(source), Some(canonical), Some(evidence)) = (
                row.get("source")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty()),
                row.get("canonical")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty()),
                row.get("evidence")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty()),
            ) else {
                *counts.get_mut("skipped").unwrap() += 1;
                continue;
            };
            let source_slug = solstone_core_entity_matching::entity_slug(source);
            let target_slug = solstone_core_entity_matching::entity_slug(canonical);
            if source_slug == target_slug
                || !hints
                    .iter()
                    .any(|hint| hint == &BTreeSet::from([source_slug.clone(), target_slug.clone()]))
            {
                *counts.get_mut("skipped").unwrap() += 1;
                continue;
            }
            match solstone_core_entity::record_merge_candidate(
                journal,
                facet,
                day,
                source,
                &source_slug,
                canonical,
                &target_slug,
                evidence,
                Some("name-variant"),
                None,
                None,
            ) {
                Ok(_) => *counts.get_mut("merges").unwrap() += 1,
                Err(error_value) => {
                    *counts.get_mut("errored").unwrap() += 1;
                    error = Some(format!("EntityReviewCandidateError: {error_value}"));
                }
            }
        }
        Ok(())
    })();
    if let Err(detail) = result {
        error = Some(detail);
    }
    write_outcome(journal, facet, day, &counts, error.as_deref());
    Ok(())
}

fn write_outcome(
    journal: &Path,
    facet: &str,
    day: &str,
    counts: &BTreeMap<&str, usize>,
    error: Option<&str>,
) {
    let path = journal
        .join("facets")
        .join(facet)
        .join("entities")
        .join(format!("{day}_review_outcome.json"));
    let mut payload = counts
        .iter()
        .map(|(name, value)| ((*name).to_owned(), Value::from(*value)))
        .collect::<Map<_, _>>();
    payload.insert(
        "error".to_owned(),
        error.map_or(Value::Null, |value| Value::String(value.to_owned())),
    );
    payload.insert(
        "ts".to_owned(),
        Value::from(chrono::Utc::now().timestamp_millis()),
    );
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(path, format!("{}\n", Value::Object(payload)));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn detect(root: &Path, day: &str, name: &str) {
        detect_in(root, "work", day, name);
    }

    fn detect_in(root: &Path, facet: &str, day: &str, name: &str) {
        solstone_core_facets::upsert_detection_segment(
            root,
            facet,
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

    #[test]
    fn review_window_and_name_key_match_reference() {
        // Derived from solstone/apps/entities/talent/entities_review.py:40-52,74-87.
        assert_eq!(
            review_days("20260108"),
            [
                "20260101", "20260102", "20260103", "20260104", "20260105", "20260106", "20260107"
            ]
        );
        assert!(format_name_key("Ada") < format_name_key("beta"));
        assert_eq!(format_name_key("Straße").0, "strasse");
        let root = tempfile::tempdir().unwrap();
        detect(root.path(), "20260101", "Ada");
        detect(root.path(), "20260102", "Ada Lovelace");
        assert_eq!(
            build_review_inputs(root.path(), "work", "20260108")
                .unwrap()
                .hints,
            [("Ada".to_owned(), "Ada Lovelace".to_owned())]
        );
    }

    #[test]
    fn candidates_ignore_entities_attached_to_other_facets() {
        // Derived from solstone/apps/entities/talent/entities_review.py:116-167.
        let root = tempfile::tempdir().unwrap();
        for day in ["20260101", "20260102"] {
            detect_in(root.path(), "work", day, "Ada");
        }
        solstone_core_facets::attach_or_reactivate_entity(
            root.path(),
            "personal",
            "Person",
            "Ada",
            "",
        )
        .unwrap();
        let inputs = build_review_inputs(root.path(), "work", "20260103").unwrap();
        assert!(
            inputs
                .eligible
                .iter()
                .any(|candidate| candidate["name"] == "Ada")
        );
    }

    #[test]
    fn low_confidence_name_matches_remain_review_candidates() {
        // Derived from solstone/apps/entities/talent/entities_review.py:158-162.
        let root = tempfile::tempdir().unwrap();
        for day in ["20260101", "20260102"] {
            detect(root.path(), day, "Ada");
        }
        solstone_core_facets::attach_or_reactivate_entity(
            root.path(),
            "work",
            "Person",
            "Ada Lovelace",
            "",
        )
        .unwrap();
        let matched = solstone_core_entity_matching::find_matching_entity(
            "Ada",
            &[solstone_core_entity_matching::EntityNameCandidate {
                id: Some("ada_lovelace".to_owned()),
                name: "Ada Lovelace".to_owned(),
                aka: Vec::new(),
                emails: Vec::new(),
            }],
            90.0,
        )
        .unwrap();
        assert!(!matched.tier.is_high_confidence());
        let inputs = build_review_inputs(root.path(), "work", "20260103").unwrap();
        assert!(
            inputs
                .eligible
                .iter()
                .any(|candidate| candidate["name"] == "Ada")
        );
    }

    #[test]
    fn promotions_aliases_merges_and_outcomes_persist() {
        // Derived from solstone/apps/entities/talent/entities_review.py:251-485.
        let root = tempfile::tempdir().unwrap();
        for day in ["20260101", "20260102"] {
            detect(root.path(), day, "Ada Lovelace");
        }
        detect(root.path(), "20260101", "Ada");
        detect(root.path(), "20260102", "Ada");
        apply_result(root.path(), r#"{"promotions":[{"name":"Ada Lovelace","description":"Writes analytical notes.","promote":true,"aliases":["Countess Ada"]}],"merges":[{"source":"Ada","canonical":"Ada Lovelace","evidence":"same recurring person"}]}"#, "work", "20260108").unwrap();
        let attached =
            solstone_core_facets::list_scoped_facet_entities(root.path(), "work", false, false)
                .unwrap();
        let entity = attached
            .iter()
            .find(|entity| entity.entity_id == "ada_lovelace")
            .unwrap();
        assert_eq!(entity.identity["type"], "Person");
        assert!(
            entity.identity["aka"]
                .as_array()
                .unwrap()
                .contains(&Value::String("Countess Ada".to_owned()))
        );
        assert_eq!(
            solstone_core_entity::load_merge_candidates(root.path(), Some("work"), None)
                .unwrap()
                .len(),
            1
        );
        let sidecar: Value = serde_json::from_str(
            &fs::read_to_string(
                root.path()
                    .join("facets/work/entities/20260108_review_outcome.json"),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(sidecar["promoted"], 1);
        assert_eq!(sidecar["aliased"], 1);
        assert_eq!(sidecar["merges"], 1);
    }

    #[test]
    fn malformed_result_writes_error_outcome() {
        // Derived from solstone/apps/entities/talent/entities_review.py:426-485.
        let root = tempfile::tempdir().unwrap();
        apply_result(root.path(), "not json", "work", "20260108").unwrap();
        let outcome: Value = serde_json::from_str(
            &fs::read_to_string(
                root.path()
                    .join("facets/work/entities/20260108_review_outcome.json"),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(outcome["error"], "invalid JSON");
    }
}
