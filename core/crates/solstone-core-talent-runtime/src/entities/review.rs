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

/// Whether `name` is, as written, the name of a known entity other than the
/// owner: the confident tiers only (name or alias, ignoring case, email, id).
fn names_known_other(
    name: &str,
    candidates: &[solstone_core_entity_matching::EntityNameCandidate],
    owner: &crate::JournalOwner,
) -> bool {
    solstone_core_entity_matching::find_matching_entity(name, candidates, 90.0).is_some_and(
        |matched| {
            matched.tier.is_high_confidence()
                && candidates[matched.candidate_index].id.as_deref() != owner.id.as_deref()
        },
    )
}

fn threshold(entity_type: &str) -> usize {
    match entity_type {
        "Person" => 2,
        "Company" | "Project" => 3,
        "Tool" => 5,
        _ => 5,
    }
}

/// The most candidates one review reads. A day with more keeps the rest for a
/// later day, when they recur.
const MAX_CANDIDATES: usize = 20;
/// The most notes one candidate carries: its latest, at most one per window day.
const MAX_CONTEXTS: usize = REVIEW_WINDOW_DAYS as usize;
/// The longest note shown, in characters.
const MAX_CONTEXT_CHARS: usize = 480;
/// The most name pairs one review reads.
const MAX_PAIRS: usize = 60;
/// What the request adds around the talent's own text.
const FRAMING_TOKENS: u32 = 256;

#[derive(Clone, Debug, PartialEq)]
struct ReviewInputs {
    eligible: Vec<Value>,
    hints: Vec<(String, String)>,
    prior: Vec<Value>,
    /// The type each pair name carries on the most days, shown beside it.
    types: BTreeMap<String, String>,
    /// How many window days each pair name was seen, for ranking.
    name_days: BTreeMap<String, usize>,
}

fn pair_key(left: &str, right: &str) -> BTreeSet<String> {
    BTreeSet::from([
        solstone_core_entity_matching::entity_slug(left),
        solstone_core_entity_matching::entity_slug(right),
    ])
}

fn row_pair_key(row: &Value) -> BTreeSet<String> {
    let side = |slug: &str, name: &str| {
        row[slug]
            .as_str()
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| {
                solstone_core_entity_matching::entity_slug(row[name].as_str().unwrap_or_default())
            })
    };
    BTreeSet::from([side("source_slug", "source"), side("target_slug", "target")])
}

/// A candidate seen on more days comes first.
fn candidate_rank(item: &Value) -> (std::cmp::Reverse<u64>, (String, String)) {
    (
        std::cmp::Reverse(item["day_count"].as_u64().unwrap_or_default()),
        format_name_key(item["name"].as_str().unwrap_or_default()),
    )
}

/// A pair's place in line: untouched, unproposed, fewer days, then by name.
type PairRank = (
    bool,
    bool,
    std::cmp::Reverse<usize>,
    (String, String),
    (String, String),
);

/// A pair touching a candidate comes first, then one already proposed, then the
/// pair whose names were seen on more days.
fn pair_rank(inputs: &ReviewInputs, (left, right): &(String, String)) -> PairRank {
    let key = pair_key(left, right);
    let touches = inputs
        .eligible
        .iter()
        .any(|item| item["slug"].as_str().is_some_and(|slug| key.contains(slug)));
    let proposed = inputs.prior.iter().any(|row| row_pair_key(row) == key);
    let days = inputs.name_days.get(left).copied().unwrap_or_default()
        + inputs.name_days.get(right).copied().unwrap_or_default();
    (
        !touches,
        !proposed,
        std::cmp::Reverse(days),
        format_name_key(left),
        format_name_key(right),
    )
}

fn sort_for_display(inputs: &mut ReviewInputs) {
    inputs.eligible.sort_by_key(|item| {
        (
            format_name_key(item["name"].as_str().unwrap_or_default()),
            item["type"].as_str().unwrap_or_default().to_owned(),
        )
    });
    inputs
        .hints
        .sort_by_key(|pair| (format_name_key(&pair.0), format_name_key(&pair.1)));
}

/// Keep the highest-ranked candidates and pairs within the count bounds.
fn bound(inputs: &mut ReviewInputs) {
    inputs.eligible.sort_by_key(candidate_rank);
    inputs.eligible.truncate(MAX_CANDIDATES);
    let ranked = {
        let mut hints = inputs.hints.clone();
        hints.sort_by_key(|pair| pair_rank(inputs, pair));
        hints
    };
    inputs.hints = ranked.into_iter().take(MAX_PAIRS).collect();
    keep_prior_for_pairs(inputs);
    sort_for_display(inputs);
}

fn keep_prior_for_pairs(inputs: &mut ReviewInputs) {
    let live = inputs
        .hints
        .iter()
        .map(|(left, right)| pair_key(left, right))
        .collect::<BTreeSet<_>>();
    inputs.prior.retain(|row| live.contains(&row_pair_key(row)));
}

/// The input budget at the floor window, less the talent's own text.
fn packet_budget(template: &str, max_output_tokens: u32) -> u32 {
    solstone_core_local::generate::compute_input_budget(
        max_output_tokens,
        solstone_core_local::plan::LOCAL_MIN_CONTEXT_TOKENS,
    )
    .saturating_sub(solstone_core_tokens::count(
        &template.replace("$review_packet", ""),
    ))
    .saturating_sub(FRAMING_TOKENS)
}

/// The counts above fit the floor for ordinary text. A script that takes more
/// tokens per character can still overflow it, so the lowest-ranked pairs, and
/// then candidates, wait for a later day until the packet fits.
fn fit_to_budget(inputs: &mut ReviewInputs, budget: u32) {
    while solstone_core_tokens::count(&format_review_packet(inputs)) > budget {
        if !inputs.hints.is_empty() {
            let worst = inputs
                .hints
                .iter()
                .enumerate()
                .max_by_key(|(_, pair)| pair_rank(inputs, pair))
                .map(|(index, _)| index)
                .expect("pairs are not empty");
            inputs.hints.remove(worst);
            keep_prior_for_pairs(inputs);
        } else if inputs.eligible.len() > 1 {
            let worst = inputs
                .eligible
                .iter()
                .enumerate()
                .max_by_key(|(_, item)| candidate_rank(item))
                .map(|(index, _)| index)
                .expect("candidates are not empty");
            inputs.eligible.remove(worst);
        } else {
            break;
        }
    }
}

/// Words that name a thing's form, not a different thing.
const FORM_WORDS: [&str; 10] = [
    "the",
    "inc",
    "llc",
    "ltd",
    "corp",
    "corporation",
    "co",
    "company",
    "gmbh",
    "plc",
];

/// One name is the other plus words that narrow it: a product of a company, a
/// part of a project, a version of a tool, a surname after a first name. The
/// review's own standard calls those different things, or can't tell them
/// apart, so the pair is not asked: a wrong merge is worse than a missed one.
/// A name that adds only a corporate suffix or a leading article is still asked.
fn narrows(left: &str, right: &str) -> bool {
    let words = |name: &str| {
        solstone_core_entity_matching::entity_slug(name)
            .split('_')
            .map(str::to_owned)
            .collect::<Vec<_>>()
    };
    let (left_words, right_words) = (words(left), words(right));
    let (longer, shorter) = if left_words.len() > right_words.len() {
        (&left_words, &right_words)
    } else {
        (&right_words, &left_words)
    };
    let n = shorter.len();
    if longer.len() == n || shorter.first().is_none_or(String::is_empty) {
        return false;
    }
    let extra = if longer[..n] == shorter[..] {
        &longer[n..]
    } else if longer[longer.len() - n..] == shorter[..] {
        &longer[..longer.len() - n]
    } else {
        return false;
    };
    !extra.iter().all(|word| FORM_WORDS.contains(&word.as_str()))
}

fn shorten(text: &str) -> String {
    if text.chars().count() <= MAX_CONTEXT_CHARS {
        return text.to_owned();
    }
    let cut = text.chars().take(MAX_CONTEXT_CHARS - 1).collect::<String>();
    format!("{}…", cut.trim_end())
}

type CandidateBucket = (
    BTreeSet<String>,
    BTreeMap<String, BTreeSet<String>>,
    Vec<Value>,
);

/// The type carried on the most days, with its day count. A tie for the most days has no
/// dominant type.
fn dominant_type(type_days: &BTreeMap<String, BTreeSet<String>>) -> Option<(&str, usize)> {
    let most = type_days.values().map(BTreeSet::len).max()?;
    let mut leaders = type_days.iter().filter(|(_, days)| days.len() == most);
    let (entity_type, _) = leaders.next()?;
    leaders
        .next()
        .is_none()
        .then_some((entity_type.as_str(), most))
}

/// The bucket every name the owner answers to falls into. No entity slug is
/// empty, so it never collides with another candidate's bucket.
const OWNER_BUCKET: &str = "";

fn build_review_inputs(journal: &Path, facet: &str, day: &str) -> Result<ReviewInputs, String> {
    let owner = crate::JournalOwner::load(journal)?;
    let mut buckets: BTreeMap<String, CandidateBucket> = BTreeMap::new();
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
            // "Speaker 2" or "Unknown" stands for someone the moment never named.
            if solstone_core_entity::is_placeholder_query(
                &solstone_core_entity_matching::normalize_resolution_query(name),
            ) {
                continue;
            }
            // Every name the owner answers to is one person: the owner. Saving
            // them separately would list the owner once per name.
            // A misheard form of the owner's name joins them too, unless it is
            // another known person's name as written.
            let is_owner = entity_type == "Person"
                && (owner.actor(name) == crate::NamedActor::Owner
                    || (owner.sounds_like(name) && !names_known_other(name, &candidates, &owner)));
            let bucket = buckets
                .entry(if is_owner {
                    OWNER_BUCKET.to_owned()
                } else {
                    slug
                })
                .or_default();
            bucket.0.insert(review_day.clone());
            bucket
                .1
                .entry(entity_type.to_owned())
                .or_default()
                .insert(review_day.clone());
            bucket
                .2
                .push(serde_json::json!({"day":review_day,"name":name,"description":description}));
            if !is_owner {
                names.insert(name.to_owned());
            }
        }
    }
    // Each pair name's days and type, read from its own bucket before the
    // buckets are spent on candidates.
    let mut name_days = BTreeMap::new();
    let mut types = BTreeMap::new();
    for name in &names {
        if let Some((days, type_days, _)) =
            buckets.get(&solstone_core_entity_matching::entity_slug(name))
        {
            name_days.insert(name.clone(), days.len());
            if let Some((entity_type, _)) = dominant_type(type_days) {
                types.insert(name.clone(), entity_type.to_owned());
            }
        }
    }
    let mut eligible = Vec::new();
    for (slug, (days, type_days, mut contexts)) in buckets {
        let Some((entity_type, type_day_count)) = dominant_type(&type_days) else {
            continue;
        };
        if type_day_count < threshold(entity_type) {
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
        let latest_name = contexts
            .iter()
            .filter(|context| context["day"].as_str() == Some(latest))
            .map(|context| context["name"].as_str().unwrap_or_default())
            .min_by_key(|name| format_name_key(name))
            .unwrap_or_default();
        // The owner is saved under one name whichever of theirs the days used:
        // the principal's own, so promotion reaches the principal. Before there
        // is one, the fullest name the days used becomes it; "you" never does.
        let name = if slug == OWNER_BUCKET {
            let fullest = contexts
                .iter()
                .map(|context| context["name"].as_str().unwrap_or_default())
                .filter(|name| {
                    !matches!(
                        default_case_fold_str(name.trim()).as_str(),
                        "you" | "yourself"
                    )
                })
                .max_by_key(|name| {
                    (
                        name.split_whitespace().count(),
                        name.chars().count(),
                        std::cmp::Reverse(format_name_key(name)),
                    )
                });
            match (owner.name.as_deref(), fullest) {
                (Some(principal), _) => principal,
                (None, Some(fullest)) => fullest,
                (None, None) => continue,
            }
        } else {
            latest_name
        };
        let name = name.to_owned();
        let slug = solstone_core_entity_matching::entity_slug(&name);
        if solstone_core_entity_matching::find_matching_entity(&name, &candidates, 90.0)
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
                && (solstone_core_entity_matching::is_name_variant_match(left, right)
                    || crate::possible_names::retrieval::near_spelling(left, right))
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
    let prior = solstone_core_entity::load_merge_candidates(journal, Some(facet), None)
        .map_err(|error| error.to_string())?;
    let saved = solstone_core_entity::load_all_journal_entities(journal)
        .map_err(|error| error.to_string())?
        .into_iter()
        .map(|entity| entity.id)
        .collect::<BTreeSet<_>>();
    Ok(select_review_inputs(
        eligible, hints, prior, types, name_days, &saved,
    ))
}

/// Choose what one review reads from what the window holds: the candidates'
/// latest notes, and the pairs a merge could settle, within the bounds.
fn select_review_inputs(
    mut eligible: Vec<Value>,
    mut hints: Vec<(String, String)>,
    mut prior: Vec<Value>,
    types: BTreeMap<String, String>,
    name_days: BTreeMap<String, usize>,
    saved: &BTreeSet<String>,
) -> ReviewInputs {
    // The latest notes carry a candidate; older ones repeat it.
    for item in &mut eligible {
        if let Some(contexts) = item["contexts"].as_array_mut() {
            let latest = contexts.split_off(contexts.len().saturating_sub(MAX_CONTEXTS));
            *contexts = latest
                .into_iter()
                .map(|mut context| {
                    let short = shorten(context["description"].as_str().unwrap_or_default());
                    context["description"] = Value::String(short);
                    context
                })
                .collect();
        }
    }
    // Two spellings of one pair of names ask one question.
    let mut asked = BTreeSet::new();
    hints.retain(|(left, right)| asked.insert(pair_key(left, right)));
    // A pair the owner accepted or dismissed is settled and is not asked again.
    let settled = prior
        .iter()
        .filter(|row| {
            matches!(
                row["status"].as_str(),
                Some("accepted" | "dismissed" | "resolved")
            )
        })
        .map(row_pair_key)
        .collect::<BTreeSet<_>>();
    hints.retain(|(left, right)| !settled.contains(&pair_key(left, right)));
    // A proposal is accepted as a merge of two saved entities, so a pair is
    // asked only when both names are saved, or are today's candidates.
    let candidates = eligible
        .iter()
        .filter_map(|item| item["slug"].as_str())
        .collect::<BTreeSet<_>>();
    hints.retain(|(left, right)| {
        [left, right].iter().all(|name| {
            let slug = solstone_core_entity_matching::entity_slug(name);
            saved.contains(&slug) || candidates.contains(slug.as_str())
        })
    });
    hints.retain(|(left, right)| !narrows(left, right));
    prior.sort_by_key(|row| {
        (
            default_case_fold_str(row["source"].as_str().unwrap_or_default()),
            default_case_fold_str(row["target"].as_str().unwrap_or_default()),
            row["status"].as_str().unwrap_or("open").to_owned(),
        )
    });
    let mut inputs = ReviewInputs {
        eligible,
        hints,
        prior,
        types,
        name_days,
    };
    bound(&mut inputs);
    inputs
}

fn format_review_packet(inputs: &ReviewInputs) -> String {
    let mut lines = vec!["These are recurring people and things noticed across recent days in one area of the owner's life.".to_owned(), String::new(), "## Recurring candidates".to_owned(), String::new()];
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
    }
    // Numbered, so a merge names its pair by number instead of copying names.
    let typed = |name: &str| match inputs.types.get(name) {
        Some(entity_type) => format!("\"{name}\" ({entity_type})"),
        None => format!("\"{name}\""),
    };
    for (index, (left, right)) in inputs.hints.iter().enumerate() {
        lines.push(format!(
            "{}. {} and {}",
            index + 1,
            typed(left),
            typed(right)
        ));
    }
    lines.join("\n").trim().to_owned() + "\n"
}

/// A merge names its pair by number and the name to keep; it becomes a
/// proposal from the other name to that one. A pair already proposed in the
/// other direction keeps that direction: the review refreshes a proposal and
/// never files its reverse. Older outputs that name `source` and `canonical`
/// pass through the same direction rule.
fn resolve_merges(output: &str, inputs: Option<&Value>) -> String {
    let Ok(mut data) = serde_json::from_str::<Value>(output) else {
        return output.to_owned();
    };
    let Some(merges) = data.get("merges").and_then(Value::as_array).cloned() else {
        return output.to_owned();
    };
    let hints: Vec<(String, String)> = inputs
        .and_then(|inputs| inputs.get("hints"))
        .and_then(|hints| serde_json::from_value(hints.clone()).ok())
        .unwrap_or_default();
    let prior = inputs
        .and_then(|inputs| inputs.get("prior"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let slug = solstone_core_entity_matching::entity_slug;
    let open = |source: &str, target: &str| {
        prior.iter().find(|row| {
            row["status"].as_str().unwrap_or("open") == "open"
                && row["source_slug"].as_str() == Some(slug(source).as_str())
                && row["target_slug"].as_str() == Some(slug(target).as_str())
        })
    };
    let mut resolved = Vec::new();
    for row in merges {
        let evidence = row["evidence"].clone();
        let (source, canonical) = if let Some(number) = row.get("pair") {
            let Some((left, right)) = number
                .as_u64()
                .and_then(|number| usize::try_from(number).ok())
                .and_then(|number| number.checked_sub(1))
                .and_then(|index| hints.get(index))
            else {
                continue;
            };
            let keep = slug(row["keep"].as_str().unwrap_or_default().trim());
            if keep == slug(left) {
                (right.clone(), left.clone())
            } else if keep == slug(right) {
                (left.clone(), right.clone())
            } else {
                continue;
            }
        } else {
            (
                row["source"].as_str().unwrap_or_default().to_owned(),
                row["canonical"].as_str().unwrap_or_default().to_owned(),
            )
        };
        let (source, canonical) = match (open(&source, &canonical), open(&canonical, &source)) {
            (None, Some(reverse)) => (
                reverse["source"].as_str().unwrap_or(&canonical).to_owned(),
                reverse["target"].as_str().unwrap_or(&source).to_owned(),
            ),
            _ => (source, canonical),
        };
        resolved
            .push(serde_json::json!({"source":source,"canonical":canonical,"evidence":evidence}));
    }
    data["merges"] = Value::Array(resolved);
    serde_json::to_string(&data).unwrap_or_else(|_| output.to_owned())
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
    let mut inputs = inputs;
    let max_output_tokens = prepared
        .config
        .get("max_output_tokens")
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .unwrap_or_default();
    let template = prepared
        .config
        .get("user_instruction")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    fit_to_budget(&mut inputs, packet_budget(&template, max_output_tokens));
    owner_before.retain(|slug, _| {
        inputs
            .eligible
            .iter()
            .any(|item| item["slug"].as_str() == Some(slug.as_str()))
    });
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
    prepared: &PreparedTalent,
    _: &PrePostState,
) -> Result<ParsedOutput, StageError> {
    Ok(ParsedOutput::Text(resolve_merges(
        output,
        prepared.config.get("_daily_review_inputs"),
    )))
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
    let facet_id = solstone_core_facets::observe_facet_write_identity(journal, facet)?;
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
    let mut batch_principal_taken = false;
    // Every promotion is prepared before any publishes, so the journal cannot
    // show what earlier promotions in this batch will claim.
    let mut batch_claims = BTreeMap::new();
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
            batch_principal_taken,
            &batch_claims,
        )?;
        // A merged or deleted entity is never created again. A name a live
        // entity carries (one the owner added again under a new id) resolves
        // to that entity and is promoted as usual.
        if let Some(change) = &promotion.identity
            && solstone_core_entity::retired_state(journal, &change.entity_id)?.is_some()
        {
            skipped += 1;
            continue;
        }
        if let Some(change) = &promotion.identity
            && change.before.is_none()
            && change.after.get("is_principal").and_then(Value::as_bool) == Some(true)
        {
            batch_principal_taken = true;
        }
        for claim in &promotion.claims {
            batch_claims
                .entry(claim.clone())
                .or_insert_with(|| promotion.attachment.entity_id.clone());
        }
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
    let mut outcome = serde_json::json!({"promoted":promoted,"aliased":aliased,"merges":proposals.len(),"skipped":skipped,"errored":0,"error":null,"ts":chrono::Utc::now().timestamp_millis()});
    crate::partial_input::copy_marker(output, outcome.as_object_mut().expect("outcome object"));
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
    apply_result_cancellable(
        journal,
        output,
        facet,
        day,
        &solstone_core_generate::Cancellation::never(),
    )
}

pub(crate) fn apply_result_cancellable(
    journal: &std::path::Path,
    output: &str,
    facet: &str,
    day: &str,
    cancellation: &solstone_core_generate::Cancellation,
) -> Result<(), String> {
    let _trust = solstone_core_facets::hold_facet_trust_lock(journal).map_err(|e| e.to_string())?;
    crate::writers::check_cancellation(cancellation)?;

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
            crate::writers::check_cancellation(cancellation)?;
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
                // A merged or deleted entity is never created again; the
                // detection is not an error, and nothing is attached for it.
                Err(solstone_core_facets::FacetEntityWriteError::EntityWrite(
                    solstone_core_entity::EntityWriteError::IdentityMerged { .. }
                    | solstone_core_entity::EntityWriteError::IdentityDeleted { .. },
                )) => {
                    *counts.get_mut("skipped").unwrap() += 1;
                    None
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
                crate::writers::check_cancellation(cancellation)?;
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
            match solstone_core_entity::record_merge_candidate_checked(
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
                &|| crate::writers::check_cancellation(cancellation),
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
    crate::writers::check_cancellation(cancellation)?;
    write_outcome(journal, facet, day, &counts, error.as_deref(), output)
}

fn write_outcome(
    journal: &Path,
    facet: &str,
    day: &str,
    counts: &BTreeMap<&str, usize>,
    error: Option<&str>,
    output: &str,
) -> Result<(), String> {
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
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    crate::partial_input::copy_marker(output, &mut payload);
    fs::write(path, format!("{}\n", Value::Object(payload))).map_err(|e| e.to_string())
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

    /// Save a journal entity, so a pair naming it can be asked.
    fn save(root: &Path, name: &str, entity_type: &str) {
        let id = solstone_core_entity_matching::entity_slug(name);
        solstone_core_entity::save_entity_identity(
            root,
            &id,
            &serde_json::json!({"id":id,"name":name,"type":entity_type}),
            None,
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
        detect(root.path(), "20260101", "Marcus Feldhaus");
        detect(root.path(), "20260102", "Marcus Feldhans");
        save(root.path(), "Marcus Feldhaus", "Person");
        save(root.path(), "Marcus Feldhans", "Person");
        assert_eq!(
            build_review_inputs(root.path(), "work", "20260108")
                .unwrap()
                .hints,
            [("Marcus Feldhans".to_owned(), "Marcus Feldhaus".to_owned())]
        );
    }

    fn detect_typed(root: &Path, day: &str, detections: &[(&str, &str)]) {
        solstone_core_facets::upsert_detection_segment(
            root,
            "work",
            day,
            "090000_300",
            &detections
                .iter()
                .map(
                    |(name, entity_type)| solstone_core_facets::DetectedEntityInput {
                        entity_type: (*entity_type).to_owned(),
                        name: (*name).to_owned(),
                        description: "Came up again.".to_owned(),
                    },
                )
                .collect::<Vec<_>>(),
        )
        .unwrap();
    }

    #[test]
    fn a_name_is_judged_on_the_type_it_carries_on_the_most_days() {
        let root = tempfile::tempdir().unwrap();
        let days = [
            "20260101", "20260102", "20260103", "20260104", "20260105", "20260106",
        ];
        // Per name, the type it carries on each day in the window.
        let typed: [(&str, &[&str]); 6] = [
            ("Atlas", &["Project", "Project", "Project", "Tool"]),
            ("Beacon", &["Project", "Project", "Tool", "Tool"]),
            ("Cleo", &["Person", "Person", "Company"]),
            ("Delta", &["Project", "Project", "Tool"]),
            (
                "Ember",
                &["Project", "Project", "Tool", "Tool", "Company", "Company"],
            ),
            ("Fern", &["Person", "Person"]),
        ];
        for (index, day) in days.iter().enumerate() {
            let detections = typed
                .iter()
                .filter_map(|(name, types)| {
                    types.get(index).map(|entity_type| (*name, *entity_type))
                })
                .collect::<Vec<_>>();
            detect_typed(root.path(), day, &detections);
        }
        let inputs = build_review_inputs(root.path(), "work", "20260108").unwrap();
        let eligible = inputs
            .eligible
            .iter()
            .map(|candidate| {
                (
                    candidate["name"].as_str().unwrap(),
                    candidate["type"].as_str().unwrap(),
                )
            })
            .collect::<Vec<_>>();
        // Beacon and Ember tie for the most days; Delta is short of the Project bar.
        assert_eq!(
            eligible,
            [("Atlas", "Project"), ("Cleo", "Person"), ("Fern", "Person")]
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
        detect(root.path(), "20260101", "Marcus Feldhaus");
        detect(root.path(), "20260102", "Marcus Feldhans");
        save(root.path(), "Marcus Feldhaus", "Person");
        save(root.path(), "Marcus Feldhans", "Person");
        apply_result(root.path(), r#"{"promotions":[{"name":"Ada Lovelace","description":"Writes analytical notes.","promote":true,"aliases":["Countess Ada"]}],"merges":[{"source":"Marcus Feldhans","canonical":"Marcus Feldhaus","evidence":"same recurring person"}]}"#, "work", "20260108").unwrap();
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
    fn a_promotion_of_a_merged_name_is_skipped_not_errored() {
        let root = tempfile::tempdir().unwrap();
        for day in ["20260101", "20260102"] {
            detect(root.path(), day, "Sunstone");
        }
        fs::create_dir_all(root.path().join("entities/solstone")).unwrap();
        fs::write(
            root.path().join("entities/solstone/entity.json"),
            r#"{"id":"solstone","name":"Solstone","type":"Project"}"#,
        )
        .unwrap();
        fs::write(
            root.path().join("entities/retired.json"),
            r#"{"ids":{"sunstone":{"state":"merged","dir":"sunstone","successor":"solstone"}}}"#,
        )
        .unwrap();
        apply_result(root.path(), r#"{"promotions":[{"name":"Sunstone","description":"An earlier project name.","promote":true,"aliases":[]}],"merges":[]}"#, "work", "20260108").unwrap();
        assert!(!root.path().join("entities/sunstone").exists());
        let sidecar: Value = serde_json::from_str(
            &fs::read_to_string(
                root.path()
                    .join("facets/work/entities/20260108_review_outcome.json"),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(sidecar["promoted"], 0);
        assert_eq!(sidecar["errored"], 0);
        assert!(sidecar["error"].is_null());
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

    #[test]
    fn placeholder_labels_are_never_review_candidates() {
        let root = tempfile::tempdir().unwrap();
        for day in ["20260106", "20260107"] {
            detect_typed(
                root.path(),
                day,
                &[
                    ("Speaker 2", "Person"),
                    ("Unknown", "Person"),
                    ("Colleague", "Person"),
                    ("Participant 1", "Project"),
                    ("Ada", "Person"),
                ],
            );
        }
        let inputs = build_review_inputs(root.path(), "work", "20260108").unwrap();
        let names = inputs
            .eligible
            .iter()
            .map(|candidate| candidate["name"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(names, ["Ada"]);
        assert!(inputs.hints.is_empty());
    }

    fn owner_journal() -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        solstone_core_facets::create_facet(root.path(), "work", "Work", "", "", "", None).unwrap();
        fs::create_dir_all(root.path().join("config")).unwrap();
        fs::write(
            root.path().join("config/journal.json"),
            r#"{"identity":{"name":"Jordan Rivera","preferred":"Jo","aliases":["Jordy"]}}"#,
        )
        .unwrap();
        root
    }

    fn review_and_publish(root: &Path, promotions: Value) {
        use crate::{ExecutionContext, PreparedTalent};
        let prepared = PreparedTalent::new(
            "entities:entities_review".to_owned(),
            serde_json::json!({
                "day": "20260108",
                "facet": "work",
                "type": "generate",
                "hook": {"pre": "entities:entities_review", "post": "entities:entities_review"},
            })
            .as_object()
            .unwrap()
            .clone(),
        );
        let context = ExecutionContext {
            journal: root.to_path_buf(),
        };
        let packet = crate::daily_prepare::freeze(prepared, &context).unwrap();
        let (prepared, _) = crate::daily_prepare::thaw(&packet).unwrap();
        let output = serde_json::json!({"promotions": promotions, "merges": []}).to_string();
        for action in prepare_publication(root, &output, "work", "20260108", &prepared).unwrap() {
            match action {
                crate::writers::PreparedDailyAction::Identity { change, .. } => {
                    solstone_core_entity::publish_identity_change(
                        root,
                        &change,
                        true,
                        || Ok(()),
                        || Ok(()),
                    )
                    .unwrap();
                }
                crate::writers::PreparedDailyAction::Attachment { change } => {
                    solstone_core_facets::publish_review_attachment(
                        root,
                        &change,
                        true,
                        || Ok(()),
                        || Ok(()),
                    )
                    .unwrap();
                }
                _ => {}
            }
        }
    }

    fn work_entity_ids(root: &Path) -> Vec<String> {
        solstone_core_facets::list_scoped_facet_entities(root, "work", false, false)
            .unwrap()
            .into_iter()
            .map(|entity| entity.entity_id)
            .collect()
    }

    #[test]
    fn a_misheard_owner_name_joins_the_owner_unless_a_known_person_has_it() {
        let root = owner_journal();
        solstone_core_entity::save_entity_identity(
            root.path(),
            "jordan_rivera",
            &serde_json::json!({"id":"jordan_rivera","name":"Jordan Rivera","type":"Person","is_principal":true}),
            None,
        )
        .unwrap();
        // Someone else whose name is one letter off the owner's.
        solstone_core_facets::attach_or_reactivate_entity(
            root.path(),
            "work",
            "Person",
            "Jordan Rivero",
            "A neighbour.",
        )
        .unwrap();
        for day in ["20260106", "20260107"] {
            detect_typed(
                root.path(),
                day,
                &[("Jordan Riviera", "Person"), ("Jordan Rivero", "Person")],
            );
        }
        let inputs = build_review_inputs(root.path(), "work", "20260108").unwrap();
        let owner = inputs
            .eligible
            .iter()
            .find(|candidate| candidate["slug"] == "jordan_rivera")
            .unwrap();
        assert_eq!(owner["day_count"], 2);
        assert!(
            inputs
                .eligible
                .iter()
                .all(|candidate| candidate["slug"] != "jordan_riviera")
        );
        // The neighbour stays themself: already attached, so not eligible, and
        // never folded into the owner.
        assert!(
            inputs
                .eligible
                .iter()
                .all(|candidate| candidate["slug"] != "jordan_rivero")
        );
        assert!(inputs.hints.is_empty());
    }

    #[test]
    fn near_spellings_of_one_name_are_offered_as_possible_variants() {
        let root = tempfile::tempdir().unwrap();
        detect(root.path(), "20260101", "Marcus Feldhaus");
        detect(root.path(), "20260102", "Marcus Feldhans");
        // Too short to call, and a different first letter: never offered.
        detect(root.path(), "20260103", "Fryer");
        detect(root.path(), "20260104", "Friar");
        detect(root.path(), "20260105", "Russell Banks");
        detect(root.path(), "20260106", "Dussell Banks");
        for name in [
            "Marcus Feldhaus",
            "Marcus Feldhans",
            "Fryer",
            "Friar",
            "Russell Banks",
            "Dussell Banks",
        ] {
            save(root.path(), name, "Person");
        }
        assert_eq!(
            build_review_inputs(root.path(), "work", "20260108")
                .unwrap()
                .hints,
            [("Marcus Feldhans".to_owned(), "Marcus Feldhaus".to_owned())]
        );
    }

    #[test]
    fn a_settled_pair_is_not_asked_again_and_only_todays_pairs_bring_their_proposals() {
        let root = tempfile::tempdir().unwrap();
        detect_typed(
            root.path(),
            "20260101",
            &[
                ("Acme", "Company"),
                ("Acme Inc", "Company"),
                ("Globex", "Company"),
                ("Globex Corp", "Company"),
            ],
        );
        for name in ["Acme", "Acme Inc", "Globex", "Globex Corp"] {
            save(root.path(), name, "Company");
        }
        let propose = |source: &str, target: &str| {
            solstone_core_entity::record_merge_candidate(
                root.path(),
                "work",
                "20260101",
                source,
                &solstone_core_entity_matching::entity_slug(source),
                target,
                &solstone_core_entity_matching::entity_slug(target),
                "same person",
                None,
                None,
                None,
            )
            .unwrap();
        };
        propose("Acme Inc", "Acme");
        propose("Globex Corp", "Globex");
        propose("Initech LLC", "Initech");
        solstone_core_entity::dismiss_merge_candidate(root.path(), "work", "globex_corp", "globex")
            .unwrap();
        let inputs = build_review_inputs(root.path(), "work", "20260108").unwrap();
        assert_eq!(inputs.hints, [("Acme".to_owned(), "Acme Inc".to_owned())]);
        let prior = inputs
            .prior
            .iter()
            .map(|row| row["source"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(prior, ["Acme Inc"]);
        let packet = format_review_packet(&inputs);
        assert!(packet.contains("1. \"Acme\" (Company) and \"Acme Inc\" (Company)"));
        assert!(!packet.contains("Prior merge"));
    }

    #[test]
    fn a_pair_is_asked_only_when_a_merge_could_stand_and_the_names_can_be_one_thing() {
        let root = tempfile::tempdir().unwrap();
        detect_typed(
            root.path(),
            "20260101",
            &[
                ("Acme", "Company"),
                ("Acme Cloud", "Tool"),
                ("Acme Inc", "Company"),
                ("Orbit", "Project"),
                ("Orbit Mobile", "Project"),
                ("Lin", "Person"),
                ("Lin Okafor", "Person"),
                ("Quill", "Tool"),
                ("Quill App", "Tool"),
            ],
        );
        for (name, entity_type) in [
            ("Acme", "Company"),
            ("Acme Cloud", "Tool"),
            ("Acme Inc", "Company"),
            ("Orbit", "Project"),
            ("Orbit Mobile", "Project"),
            ("Lin", "Person"),
            ("Lin Okafor", "Person"),
            ("Quill", "Tool"),
        ] {
            save(root.path(), name, entity_type);
        }
        let inputs = build_review_inputs(root.path(), "work", "20260108").unwrap();
        // A product, a part and a surname narrow their name; an unsaved name
        // can't be merged; a corporate suffix is still asked.
        assert_eq!(inputs.hints, [("Acme".to_owned(), "Acme Inc".to_owned())]);
    }

    #[test]
    fn a_day_with_more_candidates_than_one_review_reads_keeps_the_most_recurring() {
        let root = tempfile::tempdir().unwrap();
        for index in 0..(MAX_CANDIDATES + 3) {
            let name = format!("Person {index:02}");
            // The last three are seen on two days, the rest on three.
            let days = if index >= MAX_CANDIDATES { 2 } else { 3 };
            for day in ["20260101", "20260102", "20260103"].iter().take(days) {
                detect(root.path(), day, &name);
            }
        }
        let inputs = build_review_inputs(root.path(), "work", "20260108").unwrap();
        assert_eq!(inputs.eligible.len(), MAX_CANDIDATES);
        assert!(inputs.eligible.iter().all(|item| item["day_count"] == 3));
    }

    #[test]
    fn a_packet_over_its_budget_leaves_pairs_then_candidates_for_a_later_day() {
        let root = tempfile::tempdir().unwrap();
        for day in ["20260101", "20260102"] {
            detect(root.path(), day, "Ada Lovelace");
            detect(root.path(), day, "Grace Hopper");
        }
        detect_typed(
            root.path(),
            "20260103",
            &[("Acme", "Company"), ("Acme Inc", "Company")],
        );
        save(root.path(), "Acme", "Company");
        save(root.path(), "Acme Inc", "Company");
        let mut inputs = build_review_inputs(root.path(), "work", "20260108").unwrap();
        assert_eq!((inputs.eligible.len(), inputs.hints.len()), (2, 1));
        let one_candidate = {
            let mut fewer = inputs.clone();
            fewer.hints.clear();
            fewer.eligible.truncate(1);
            solstone_core_tokens::count(&format_review_packet(&fewer))
        };
        fit_to_budget(&mut inputs, one_candidate);
        assert!(inputs.hints.is_empty());
        assert_eq!(inputs.eligible.len(), 1);
        assert!(solstone_core_tokens::count(&format_review_packet(&inputs)) <= one_candidate);
    }

    #[test]
    fn a_numbered_merge_becomes_its_pair_and_never_reverses_an_open_proposal() {
        let inputs = serde_json::json!({
            "hints": [["Ada", "Ada Lovelace"], ["Bob", "Bobby"]],
            "prior": [{"source":"Bobby","source_slug":"bobby","target":"Bob","target_slug":"bob","status":"open"}],
        });
        let output = r#"{"promotions":[],"merges":[{"pair":1,"keep":"Ada Lovelace","evidence":"same person"},{"pair":2,"keep":"Bobby","evidence":"same person"},{"pair":3,"keep":"Ada","evidence":"no such pair"},{"pair":1,"keep":"Grace","evidence":"neither name"}]}"#;
        let resolved: Value = serde_json::from_str(&resolve_merges(output, Some(&inputs))).unwrap();
        assert_eq!(
            resolved["merges"],
            serde_json::json!([
                {"source":"Ada","canonical":"Ada Lovelace","evidence":"same person"},
                {"source":"Bobby","canonical":"Bob","evidence":"same person"},
            ])
        );
        let named = r#"{"promotions":[],"merges":[{"source":"Ada","canonical":"Ada Lovelace","evidence":"same person"}]}"#;
        let resolved: Value = serde_json::from_str(&resolve_merges(named, Some(&inputs))).unwrap();
        assert_eq!(resolved["merges"][0]["canonical"], "Ada Lovelace");
    }

    /// Replays recorded review inputs against packets built by the evaluation
    /// harness. Run by hand: REVIEW_PARITY_DIR=<dir of case files> cargo test
    /// -p solstone-core-talent-runtime --lib review_packets_match_recorded -- --ignored
    #[test]
    #[ignore = "needs recorded inputs"]
    fn review_packets_match_recorded() {
        let dir = std::env::var("REVIEW_PARITY_DIR").expect("REVIEW_PARITY_DIR");
        let mut checked = 0;
        for entry in fs::read_dir(dir).unwrap() {
            let case: Value =
                serde_json::from_str(&fs::read_to_string(entry.unwrap().path()).unwrap()).unwrap();
            let field = |key: &str| case[key].clone();
            let inputs = select_review_inputs(
                serde_json::from_value(field("eligible")).unwrap(),
                serde_json::from_value(field("hints")).unwrap(),
                serde_json::from_value(field("prior")).unwrap(),
                serde_json::from_value(field("types")).unwrap(),
                serde_json::from_value(field("name_days")).unwrap(),
                &serde_json::from_value(field("saved")).unwrap(),
            );
            assert_eq!(
                format_review_packet(&inputs),
                case["packet"].as_str().unwrap(),
                "{}",
                case["id"]
            );
            assert_eq!(
                serde_json::to_value(&inputs.hints).unwrap(),
                case["hints_used"],
                "{}",
                case["id"]
            );
            checked += 1;
        }
        assert!(checked > 0);
        eprintln!("{checked} packets match");
    }

    #[test]
    fn every_owner_name_reaches_the_principal_instead_of_a_new_person() {
        let root = owner_journal();
        solstone_core_entity::save_entity_identity(
            root.path(),
            "jordan_rivera",
            &serde_json::json!({"id":"jordan_rivera","name":"Jordan Rivera","type":"Person","is_principal":true}),
            None,
        )
        .unwrap();
        // The owner under two of their names, and someone else under two of theirs.
        detect_typed(
            root.path(),
            "20260106",
            &[("Jo", "Person"), ("Marcus Feldhaus", "Person")],
        );
        detect_typed(
            root.path(),
            "20260107",
            &[("Jordy", "Person"), ("Marcus Feldhans", "Person")],
        );
        for name in ["Marcus Feldhaus", "Marcus Feldhans"] {
            save(root.path(), name, "Person");
        }
        let inputs = build_review_inputs(root.path(), "work", "20260108").unwrap();
        let owner = inputs
            .eligible
            .iter()
            .find(|candidate| candidate["slug"] == "jordan_rivera")
            .unwrap();
        assert_eq!(owner["name"], "Jordan Rivera");
        assert_eq!(owner["day_count"], 2);
        assert_eq!(
            inputs.hints,
            [("Marcus Feldhans".to_owned(), "Marcus Feldhaus".to_owned())]
        );

        review_and_publish(
            root.path(),
            serde_json::json!([
                {"name": "Jordan Rivera", "description": "The owner.", "promote": true, "aliases": []},
                {"name": "Jo", "description": "The owner again.", "promote": true, "aliases": []},
            ]),
        );
        assert_eq!(work_entity_ids(root.path()), ["jordan_rivera"]);
        for name in ["jo", "jordy"] {
            assert!(
                solstone_core_entity::read_entity_identity(root.path(), name)
                    .unwrap()
                    .is_none()
            );
        }
        // Attached, the owner is no longer a candidate in this facet.
        let again = build_review_inputs(root.path(), "work", "20260108").unwrap();
        assert!(
            again
                .eligible
                .iter()
                .all(|candidate| candidate["slug"] != "jordan_rivera")
        );
    }

    #[test]
    fn before_there_is_a_principal_the_owner_is_one_candidate_under_their_fullest_name() {
        let root = owner_journal();
        detect_typed(
            root.path(),
            "20260106",
            &[("Jo", "Person"), ("you", "Person")],
        );
        detect_typed(
            root.path(),
            "20260107",
            &[("Jordan Rivera", "Person"), ("Jo", "Person")],
        );
        let inputs = build_review_inputs(root.path(), "work", "20260108").unwrap();
        let names = inputs
            .eligible
            .iter()
            .map(|candidate| candidate["name"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(names, ["Jordan Rivera"]);

        review_and_publish(
            root.path(),
            serde_json::json!([
                {"name": "Jordan Rivera", "description": "The owner.", "promote": true, "aliases": []},
            ]),
        );
        let owner = crate::JournalOwner::load(root.path()).unwrap();
        assert_eq!(owner.id.as_deref(), Some("jordan_rivera"));
        assert_eq!(work_entity_ids(root.path()), ["jordan_rivera"]);
    }

    #[test]
    fn retained_review_marks_the_configured_owner_including_the_preferred_name() {
        use crate::{ExecutionContext, JournalOwner, NamedActor, PreparedTalent};
        use serde_json::json;

        for (candidate_name, promo_name) in [("Jordan Rivera", "Jordan Rivera"), ("Jo", "Jo")] {
            let root = tempfile::tempdir().unwrap();
            solstone_core_facets::create_facet(root.path(), "work", "Work", "", "", "", None)
                .unwrap();
            fs::create_dir_all(root.path().join("config")).unwrap();
            fs::write(
                root.path().join("config/journal.json"),
                r#"{"identity":{"name":"Jordan Rivera","preferred":"Jo","aliases":["Jordy"]}}"#,
            )
            .unwrap();
            detect(root.path(), "20260106", candidate_name);
            detect(root.path(), "20260107", candidate_name);

            let prepared = PreparedTalent::new(
                "entities:entities_review".to_owned(),
                json!({
                    "day": "20260108",
                    "facet": "work",
                    "type": "generate",
                    "hook": {"pre": "entities:entities_review", "post": "entities:entities_review"},
                })
                .as_object()
                .unwrap()
                .clone(),
            );
            let context = ExecutionContext {
                journal: root.path().to_path_buf(),
            };
            let packet = crate::daily_prepare::freeze(prepared, &context).unwrap();
            let (prepared, _) = crate::daily_prepare::thaw(&packet).unwrap();

            let output = json!({
                "promotions": [{
                    "name": promo_name,
                    "description": "Journal owner.",
                    "promote": true,
                    "aliases": []
                }],
                "merges": []
            })
            .to_string();

            let actions =
                prepare_publication(root.path(), &output, "work", "20260108", &prepared).unwrap();
            for action in actions {
                match action {
                    crate::writers::PreparedDailyAction::Identity { change, .. } => {
                        solstone_core_entity::publish_identity_change(
                            root.path(),
                            &change,
                            true,
                            || Ok(()),
                            || Ok(()),
                        )
                        .unwrap();
                    }
                    crate::writers::PreparedDailyAction::Attachment { change } => {
                        solstone_core_facets::publish_review_attachment(
                            root.path(),
                            &change,
                            true,
                            || Ok(()),
                            || Ok(()),
                        )
                        .unwrap();
                    }
                    _ => {}
                }
            }

            let principals = solstone_core_entity::load_all_journal_entities(root.path())
                .unwrap()
                .into_iter()
                .filter(|e| e.is_principal() && solstone_core_entity::is_admissible_person(e))
                .collect::<Vec<_>>();
            assert_eq!(principals.len(), 1);
            let principal_id = &principals[0].id;
            let owner = JournalOwner::load(root.path()).unwrap();
            assert_eq!(owner.id.as_deref(), Some(principal_id.as_str()));
            assert_eq!(owner.actor("Jordan Rivera"), NamedActor::Owner);
            assert_eq!(owner.actor("Jo"), NamedActor::Owner);
            assert_eq!(owner.actor("Jordy"), NamedActor::Owner);
        }
    }

    fn history_count(root: &Path, entity_id: &str) -> usize {
        let events_dir = root.join("entities").join(entity_id).join("history/events");
        if !events_dir.exists() {
            return 0;
        }
        fs::read_dir(events_dir)
            .unwrap()
            .filter_map(Result::ok)
            .count()
    }

    #[test]
    fn two_owner_names_publish_one_principal_and_replay_is_exact() {
        use crate::{ExecutionContext, JournalOwner, NamedActor, PreparedTalent};
        use serde_json::json;

        let root = tempfile::tempdir().unwrap();
        solstone_core_facets::create_facet(root.path(), "work", "Work", "", "", "", None).unwrap();
        fs::create_dir_all(root.path().join("config")).unwrap();
        fs::write(
            root.path().join("config/journal.json"),
            r#"{"identity":{"name":"Jordan Rivera","preferred":"Jo","aliases":["Jordy"]}}"#,
        )
        .unwrap();

        for name in ["Jordan Rivera", "Jo"] {
            detect(root.path(), "20260106", name);
            detect(root.path(), "20260107", name);
        }

        let prepared = PreparedTalent::new(
            "entities:entities_review".to_owned(),
            json!({
                "day": "20260108",
                "facet": "work",
                "type": "generate",
                "hook": {"pre": "entities:entities_review", "post": "entities:entities_review"},
            })
            .as_object()
            .unwrap()
            .clone(),
        );
        let context = ExecutionContext {
            journal: root.path().to_path_buf(),
        };
        let packet = crate::daily_prepare::freeze(prepared, &context).unwrap();
        let (prepared, _) = crate::daily_prepare::thaw(&packet).unwrap();

        let output = json!({
            "promotions": [
                {
                    "name": "Jordan Rivera",
                    "description": "Owner full name.",
                    "promote": true,
                    "aliases": ["Captain"]
                },
                {
                    "name": "Jo",
                    "description": "Owner preferred name.",
                    "promote": true,
                    "aliases": ["Skipper"]
                }
            ],
            "merges": []
        })
        .to_string();

        let actions =
            prepare_publication(root.path(), &output, "work", "20260108", &prepared).unwrap();

        for action in &actions {
            match action {
                crate::writers::PreparedDailyAction::Identity { change, .. } => {
                    solstone_core_entity::publish_identity_change(
                        root.path(),
                        change,
                        true,
                        || Ok(()),
                        || Ok(()),
                    )
                    .unwrap();
                }
                crate::writers::PreparedDailyAction::Attachment { change } => {
                    solstone_core_facets::publish_review_attachment(
                        root.path(),
                        change,
                        true,
                        || Ok(()),
                        || Ok(()),
                    )
                    .unwrap();
                }
                crate::writers::PreparedDailyAction::Aliases { change, .. } => {
                    solstone_core_facets::publish_review_aliases(
                        root.path(),
                        "work",
                        change,
                        true,
                        || Ok(()),
                        || Ok(()),
                    )
                    .unwrap();
                }
                _ => {}
            }
        }

        let principals = solstone_core_entity::load_all_journal_entities(root.path())
            .unwrap()
            .into_iter()
            .filter(|e| e.is_principal() && solstone_core_entity::is_admissible_person(e))
            .collect::<Vec<_>>();
        assert_eq!(principals.len(), 1);
        assert_eq!(principals[0].id, "jordan_rivera");

        // "Jo" is the owner too, so it was offered as the same candidate and
        // nothing is saved under it.
        assert!(
            solstone_core_entity::read_entity_identity(root.path(), "jo")
                .unwrap()
                .is_none()
        );
        assert_eq!(principals[0].value["aka"], json!(["Captain"]));

        let owner = JournalOwner::load(root.path()).unwrap();
        assert_eq!(owner.id.as_deref(), Some("jordan_rivera"));
        assert_eq!(owner.actor("Jordan Rivera"), NamedActor::Owner);
        assert_eq!(owner.actor("Jo"), NamedActor::Owner);
        assert_eq!(owner.actor("Jordy"), NamedActor::Owner);

        let count1 = history_count(root.path(), "jordan_rivera");
        assert!(count1 > 0);

        for action in &actions {
            if let crate::writers::PreparedDailyAction::Aliases { change, .. } = action {
                solstone_core_facets::publish_review_aliases(
                    root.path(),
                    "work",
                    change,
                    false,
                    || Ok(()),
                    || Ok(()),
                )
                .unwrap();
            }
        }

        let count2 = history_count(root.path(), "jordan_rivera");
        assert_eq!(count1, count2);

        let owner2 = JournalOwner::load(root.path()).unwrap();
        assert_eq!(owner2.id.as_deref(), Some("jordan_rivera"));
    }

    #[test]
    fn frozen_principal_grants_follow_the_prepared_plan_when_the_owner_changes() {
        use crate::{ExecutionContext, PreparedTalent};
        use serde_json::json;

        // 1. Unapplied grant, then another entity is saved with is_principal before publish
        let root1 = tempfile::tempdir().unwrap();
        solstone_core_facets::create_facet(root1.path(), "work", "Work", "", "", "", None).unwrap();
        fs::create_dir_all(root1.path().join("config")).unwrap();
        fs::write(
            root1.path().join("config/journal.json"),
            r#"{"identity":{"name":"Jordan Rivera"}}"#,
        )
        .unwrap();
        detect(root1.path(), "20260106", "Jordan Rivera");
        detect(root1.path(), "20260107", "Jordan Rivera");

        let prepared1 = PreparedTalent::new(
            "entities:entities_review".to_owned(),
            json!({
                "day": "20260108",
                "facet": "work",
                "type": "generate",
                "hook": {"pre": "entities:entities_review", "post": "entities:entities_review"},
            })
            .as_object()
            .unwrap()
            .clone(),
        );
        let context1 = ExecutionContext {
            journal: root1.path().to_path_buf(),
        };
        let packet1 = crate::daily_prepare::freeze(prepared1, &context1).unwrap();
        let (prepared1, _) = crate::daily_prepare::thaw(&packet1).unwrap();

        let output1 = json!({
            "promotions": [{
                "name": "Jordan Rivera",
                "description": "Owner.",
                "promote": true,
                "aliases": []
            }],
            "merges": []
        })
        .to_string();

        let actions1 =
            prepare_publication(root1.path(), &output1, "work", "20260108", &prepared1).unwrap();
        let identity_change1 = actions1
            .iter()
            .find_map(|a| match a {
                crate::writers::PreparedDailyAction::Identity { change, .. } => {
                    Some(change.clone())
                }
                _ => None,
            })
            .unwrap();

        solstone_core_entity::save_entity_identity(
            root1.path(),
            "other_owner",
            &json!({"id":"other_owner","name":"Other Owner","type":"Person","is_principal":true}),
            None,
        )
        .unwrap();

        let mut started1 = false;
        let err1 = solstone_core_entity::publish_identity_change(
            root1.path(),
            &identity_change1,
            true,
            || {
                started1 = true;
                Ok(())
            },
            || Ok(()),
        )
        .unwrap_err();
        assert_eq!(
            err1.kind(),
            Some(solstone_core_entity::ReviewOwnerConflictKind::PrincipalGrantRefused)
        );
        assert!(!started1);
        assert!(
            !root1
                .path()
                .join("entities/jordan_rivera/entity.json")
                .exists()
        );
        assert_eq!(history_count(root1.path(), "jordan_rivera"), 0);

        // 2. A plan prepared while some other principal exists mints no one: the
        //    configured name is the owner's, and so is the principal.
        let root2 = tempfile::tempdir().unwrap();
        solstone_core_facets::create_facet(root2.path(), "work", "Work", "", "", "", None).unwrap();
        fs::create_dir_all(root2.path().join("config")).unwrap();
        fs::write(
            root2.path().join("config/journal.json"),
            r#"{"identity":{"name":"Jordan Rivera"}}"#,
        )
        .unwrap();
        solstone_core_entity::save_entity_identity(
            root2.path(),
            "alex_smith",
            &json!({"id":"alex_smith","name":"Alex Smith","type":"Person","is_principal":true}),
            None,
        )
        .unwrap();
        detect(root2.path(), "20260106", "Jordan Rivera");
        detect(root2.path(), "20260107", "Jordan Rivera");

        let prepared2 = PreparedTalent::new(
            "entities:entities_review".to_owned(),
            json!({
                "day": "20260108",
                "facet": "work",
                "type": "generate",
                "hook": {"pre": "entities:entities_review", "post": "entities:entities_review"},
            })
            .as_object()
            .unwrap()
            .clone(),
        );
        let context2 = ExecutionContext {
            journal: root2.path().to_path_buf(),
        };
        let packet2 = crate::daily_prepare::freeze(prepared2, &context2).unwrap();
        let (prepared2, _) = crate::daily_prepare::thaw(&packet2).unwrap();

        let actions2 =
            prepare_publication(root2.path(), &output1, "work", "20260108", &prepared2).unwrap();
        assert!(
            !actions2
                .iter()
                .any(|a| matches!(a, crate::writers::PreparedDailyAction::Identity { .. }))
        );
        assert!(
            solstone_core_entity::read_entity_identity(root2.path(), "jordan_rivera")
                .unwrap()
                .is_none()
        );

        // 3. An already-applied grant, published again, adds no history event and stays principal
        let root3 = tempfile::tempdir().unwrap();
        solstone_core_facets::create_facet(root3.path(), "work", "Work", "", "", "", None).unwrap();
        fs::create_dir_all(root3.path().join("config")).unwrap();
        fs::write(
            root3.path().join("config/journal.json"),
            r#"{"identity":{"name":"Jordan Rivera"}}"#,
        )
        .unwrap();
        detect(root3.path(), "20260106", "Jordan Rivera");
        detect(root3.path(), "20260107", "Jordan Rivera");

        let prepared3 = PreparedTalent::new(
            "entities:entities_review".to_owned(),
            json!({
                "day": "20260108",
                "facet": "work",
                "type": "generate",
                "hook": {"pre": "entities:entities_review", "post": "entities:entities_review"},
            })
            .as_object()
            .unwrap()
            .clone(),
        );
        let context3 = ExecutionContext {
            journal: root3.path().to_path_buf(),
        };
        let packet3 = crate::daily_prepare::freeze(prepared3, &context3).unwrap();
        let (prepared3, _) = crate::daily_prepare::thaw(&packet3).unwrap();

        let actions3 =
            prepare_publication(root3.path(), &output1, "work", "20260108", &prepared3).unwrap();
        let identity_change3 = actions3
            .iter()
            .find_map(|a| match a {
                crate::writers::PreparedDailyAction::Identity { change, .. } => {
                    Some(change.clone())
                }
                _ => None,
            })
            .unwrap();
        solstone_core_entity::publish_identity_change(
            root3.path(),
            &identity_change3,
            true,
            || Ok(()),
            || Ok(()),
        )
        .unwrap();
        let c1 = history_count(root3.path(), "jordan_rivera");
        solstone_core_entity::publish_identity_change(
            root3.path(),
            &identity_change3,
            true,
            || Ok(()),
            || Ok(()),
        )
        .unwrap();
        let c2 = history_count(root3.path(), "jordan_rivera");
        assert_eq!(c1, c2);
        let j3 = solstone_core_entity::read_entity_identity(root3.path(), "jordan_rivera")
            .unwrap()
            .unwrap();
        assert_eq!(j3.value().get("is_principal"), Some(&Value::Bool(true)));
    }

    #[test]
    fn an_unapplied_grant_refuses_unreadable_settings_and_applied_actions_replay() {
        use crate::{ExecutionContext, PreparedTalent};
        use serde_json::json;

        // 1. Prepare grant while settings valid, replace with not-json, publish fails
        let root1 = tempfile::tempdir().unwrap();
        solstone_core_facets::create_facet(root1.path(), "work", "Work", "", "", "", None).unwrap();
        fs::create_dir_all(root1.path().join("config")).unwrap();
        fs::write(
            root1.path().join("config/journal.json"),
            r#"{"identity":{"name":"Jordan Rivera"}}"#,
        )
        .unwrap();
        detect(root1.path(), "20260106", "Jordan Rivera");
        detect(root1.path(), "20260107", "Jordan Rivera");

        let prepared1 = PreparedTalent::new(
            "entities:entities_review".to_owned(),
            json!({
                "day": "20260108",
                "facet": "work",
                "type": "generate",
                "hook": {"pre": "entities:entities_review", "post": "entities:entities_review"},
            })
            .as_object()
            .unwrap()
            .clone(),
        );
        let context1 = ExecutionContext {
            journal: root1.path().to_path_buf(),
        };
        let packet1 = crate::daily_prepare::freeze(prepared1, &context1).unwrap();
        let (prepared1, _) = crate::daily_prepare::thaw(&packet1).unwrap();

        let output1 = json!({
            "promotions": [{
                "name": "Jordan Rivera",
                "description": "Owner.",
                "promote": true,
                "aliases": []
            }],
            "merges": []
        })
        .to_string();

        let actions1 =
            prepare_publication(root1.path(), &output1, "work", "20260108", &prepared1).unwrap();
        let identity_change1 = actions1
            .iter()
            .find_map(|a| match a {
                crate::writers::PreparedDailyAction::Identity { change, .. } => {
                    Some(change.clone())
                }
                _ => None,
            })
            .unwrap();

        fs::write(root1.path().join("config/journal.json"), b"not-json").unwrap();

        let mut started1 = false;
        let err1 = solstone_core_entity::publish_identity_change(
            root1.path(),
            &identity_change1,
            true,
            || {
                started1 = true;
                Ok(())
            },
            || Ok(()),
        )
        .unwrap_err();
        assert!(matches!(
            err1,
            solstone_core_entity::ReviewOwnerError::Failed { .. }
        ));
        assert!(!started1);
        assert!(
            !root1
                .path()
                .join("entities/jordan_rivera/entity.json")
                .exists()
        );
        assert_eq!(history_count(root1.path(), "jordan_rivera"), 0);

        // 2. Readable config with no owner names, and missing settings file
        let root_no_names = tempfile::tempdir().unwrap();
        solstone_core_facets::create_facet(root_no_names.path(), "work", "Work", "", "", "", None)
            .unwrap();
        fs::create_dir_all(root_no_names.path().join("config")).unwrap();
        fs::write(
            root_no_names.path().join("config/journal.json"),
            r#"{"identity":{}}"#,
        )
        .unwrap();
        let promo_no_names = solstone_core_facets::prepare_review_promotion(
            root_no_names.path(),
            "work",
            "Person",
            "Jordan Rivera",
            "Desc",
            &[],
            false,
            &Default::default(),
        )
        .unwrap();
        let change_no_names = promo_no_names.identity.unwrap();
        assert_ne!(
            change_no_names.after.get("is_principal"),
            Some(&Value::Bool(true))
        );
        solstone_core_entity::publish_identity_change(
            root_no_names.path(),
            &change_no_names,
            true,
            || Ok(()),
            || Ok(()),
        )
        .unwrap();

        let root_missing = tempfile::tempdir().unwrap();
        solstone_core_facets::create_facet(root_missing.path(), "work", "Work", "", "", "", None)
            .unwrap();
        let promo_missing = solstone_core_facets::prepare_review_promotion(
            root_missing.path(),
            "work",
            "Person",
            "Jordan Rivera",
            "Desc",
            &[],
            false,
            &Default::default(),
        )
        .unwrap();
        let change_missing = promo_missing.identity.unwrap();
        assert_ne!(
            change_missing.after.get("is_principal"),
            Some(&Value::Bool(true))
        );
        solstone_core_entity::publish_identity_change(
            root_missing.path(),
            &change_missing,
            true,
            || Ok(()),
            || Ok(()),
        )
        .unwrap();

        // 3. After successful grant, replay with changed name, and separately corrupt file
        let root2 = tempfile::tempdir().unwrap();
        solstone_core_facets::create_facet(root2.path(), "work", "Work", "", "", "", None).unwrap();
        fs::create_dir_all(root2.path().join("config")).unwrap();
        fs::write(
            root2.path().join("config/journal.json"),
            r#"{"identity":{"name":"Jordan Rivera"}}"#,
        )
        .unwrap();
        detect(root2.path(), "20260106", "Jordan Rivera");
        detect(root2.path(), "20260107", "Jordan Rivera");

        let prepared2 = PreparedTalent::new(
            "entities:entities_review".to_owned(),
            json!({
                "day": "20260108",
                "facet": "work",
                "type": "generate",
                "hook": {"pre": "entities:entities_review", "post": "entities:entities_review"},
            })
            .as_object()
            .unwrap()
            .clone(),
        );
        let context2 = ExecutionContext {
            journal: root2.path().to_path_buf(),
        };
        let packet2 = crate::daily_prepare::freeze(prepared2, &context2).unwrap();
        let (prepared2, _) = crate::daily_prepare::thaw(&packet2).unwrap();

        let actions2 =
            prepare_publication(root2.path(), &output1, "work", "20260108", &prepared2).unwrap();
        let identity_change2 = actions2
            .iter()
            .find_map(|a| match a {
                crate::writers::PreparedDailyAction::Identity { change, .. } => {
                    Some(change.clone())
                }
                _ => None,
            })
            .unwrap();
        solstone_core_entity::publish_identity_change(
            root2.path(),
            &identity_change2,
            true,
            || Ok(()),
            || Ok(()),
        )
        .unwrap();
        let count_before = history_count(root2.path(), "jordan_rivera");

        fs::write(
            root2.path().join("config/journal.json"),
            r#"{"identity":{"name":"Someone Else"}}"#,
        )
        .unwrap();
        solstone_core_entity::publish_identity_change(
            root2.path(),
            &identity_change2,
            false,
            || Ok(()),
            || Ok(()),
        )
        .unwrap();
        assert_eq!(history_count(root2.path(), "jordan_rivera"), count_before);

        fs::write(root2.path().join("config/journal.json"), b"not-json").unwrap();
        solstone_core_entity::publish_identity_change(
            root2.path(),
            &identity_change2,
            false,
            || Ok(()),
            || Ok(()),
        )
        .unwrap();
        assert_eq!(history_count(root2.path(), "jordan_rivera"), count_before);

        let j2 = solstone_core_entity::read_entity_identity(root2.path(), "jordan_rivera")
            .unwrap()
            .unwrap();
        assert_eq!(j2.value().get("is_principal"), Some(&Value::Bool(true)));

        // 4. Principal-preserving alias: name matches owner, alias action has is_principal on both sides
        let root3 = tempfile::tempdir().unwrap();
        solstone_core_facets::create_facet(root3.path(), "work", "Work", "", "", "", None).unwrap();
        fs::create_dir_all(root3.path().join("config")).unwrap();
        fs::write(
            root3.path().join("config/journal.json"),
            r#"{"identity":{"name":"Jordan Rivera"}}"#,
        )
        .unwrap();
        detect(root3.path(), "20260106", "Jordan Rivera");
        detect(root3.path(), "20260107", "Jordan Rivera");

        let prepared3 = PreparedTalent::new(
            "entities:entities_review".to_owned(),
            json!({
                "day": "20260108",
                "facet": "work",
                "type": "generate",
                "hook": {"pre": "entities:entities_review", "post": "entities:entities_review"},
            })
            .as_object()
            .unwrap()
            .clone(),
        );
        let context3 = ExecutionContext {
            journal: root3.path().to_path_buf(),
        };
        let packet3 = crate::daily_prepare::freeze(prepared3, &context3).unwrap();
        let (prepared3, _) = crate::daily_prepare::thaw(&packet3).unwrap();

        let output_alias = json!({
            "promotions": [{
                "name": "Jordan Rivera",
                "description": "Owner with alias.",
                "promote": true,
                "aliases": ["Skipper"]
            }],
            "merges": []
        })
        .to_string();

        let actions3 =
            prepare_publication(root3.path(), &output_alias, "work", "20260108", &prepared3)
                .unwrap();
        let mut id_act = None;
        let mut att_act = None;
        let mut ali_act = None;
        for a in actions3 {
            match a {
                crate::writers::PreparedDailyAction::Identity { change, .. } => {
                    id_act = Some(change)
                }
                crate::writers::PreparedDailyAction::Attachment { change } => {
                    att_act = Some(change)
                }
                crate::writers::PreparedDailyAction::Aliases { change, .. } => {
                    ali_act = Some(change)
                }
                _ => {}
            }
        }
        let id_act = id_act.unwrap();
        let att_act = att_act.unwrap();
        let ali_act = ali_act.unwrap();

        assert_eq!(
            ali_act.before.as_ref().unwrap().get("is_principal"),
            Some(&Value::Bool(true))
        );
        assert_eq!(ali_act.after.get("is_principal"), Some(&Value::Bool(true)));

        solstone_core_entity::publish_identity_change(
            root3.path(),
            &id_act,
            true,
            || Ok(()),
            || Ok(()),
        )
        .unwrap();
        solstone_core_facets::publish_review_attachment(
            root3.path(),
            &att_act,
            true,
            || Ok(()),
            || Ok(()),
        )
        .unwrap();

        fs::write(root3.path().join("config/journal.json"), b"not-json").unwrap();

        solstone_core_facets::publish_review_aliases(
            root3.path(),
            "work",
            &ali_act,
            true,
            || Ok(()),
            || Ok(()),
        )
        .unwrap();

        let j3 = solstone_core_entity::read_entity_identity(root3.path(), "jordan_rivera")
            .unwrap()
            .unwrap();
        assert_eq!(j3.value().get("is_principal"), Some(&Value::Bool(true)));
        assert_eq!(j3.value()["aka"], json!(["Skipper"]));
    }

    #[test]
    fn review_does_not_grant_principal_outside_a_new_matching_person() {
        use crate::JournalOwner;
        use serde_json::json;

        let root = tempfile::tempdir().unwrap();
        solstone_core_facets::create_facet(root.path(), "work", "Work", "", "", "", None).unwrap();
        fs::create_dir_all(root.path().join("config")).unwrap();
        fs::write(
            root.path().join("config/journal.json"),
            r#"{"identity":{"name":"Jordan Rivera"}}"#,
        )
        .unwrap();

        // 1. Company whose name is configured owner
        let promo_co = solstone_core_facets::prepare_review_promotion(
            root.path(),
            "work",
            "Company",
            "Jordan Rivera",
            "A company.",
            &[],
            false,
            &Default::default(),
        )
        .unwrap();
        assert_ne!(
            promo_co
                .identity
                .as_ref()
                .unwrap()
                .after
                .get("is_principal"),
            Some(&Value::Bool(true))
        );

        // 2. Person whose name does not match
        let promo_diff = solstone_core_facets::prepare_review_promotion(
            root.path(),
            "work",
            "Person",
            "Sam Lee",
            "A coworker.",
            &[],
            false,
            &Default::default(),
        )
        .unwrap();
        assert_ne!(
            promo_diff
                .identity
                .as_ref()
                .unwrap()
                .after
                .get("is_principal"),
            Some(&Value::Bool(true))
        );

        // 3. Person named Sam Lee with model aliases ["Jordan Rivera"]
        let promo_alias = solstone_core_facets::prepare_review_promotion(
            root.path(),
            "work",
            "Person",
            "Sam Lee",
            "A coworker.",
            &["Jordan Rivera".into()],
            false,
            &Default::default(),
        )
        .unwrap();
        assert_ne!(
            promo_alias
                .identity
                .as_ref()
                .unwrap()
                .after
                .get("is_principal"),
            Some(&Value::Bool(true))
        );

        // 4. An existing principal stays the only principal when owner-name person is created
        solstone_core_entity::save_entity_identity(
            root.path(),
            "taylor_green",
            &json!({"id":"taylor_green","name":"Taylor Green","type":"Person","is_principal":true}),
            None,
        )
        .unwrap();
        let promo_existing_p = solstone_core_facets::prepare_review_promotion(
            root.path(),
            "work",
            "Person",
            "Jordan Rivera",
            "Owner candidate.",
            &[],
            false,
            &Default::default(),
        )
        .unwrap();
        assert_ne!(
            promo_existing_p
                .identity
                .as_ref()
                .unwrap()
                .after
                .get("is_principal"),
            Some(&Value::Bool(true))
        );

        // 5. An existing unmarked person with owner name is not selected/merged; promotion does not add flag
        let root2 = tempfile::tempdir().unwrap();
        solstone_core_facets::create_facet(root2.path(), "work", "Work", "", "", "", None).unwrap();
        fs::create_dir_all(root2.path().join("config")).unwrap();
        fs::write(
            root2.path().join("config/journal.json"),
            r#"{"identity":{"name":"Jordan Rivera"}}"#,
        )
        .unwrap();
        solstone_core_entity::save_entity_identity(
            root2.path(),
            "jordan_rivera",
            &json!({"id":"jordan_rivera","name":"Jordan Rivera","type":"Person"}),
            None,
        )
        .unwrap();
        let promo_unmarked = solstone_core_facets::prepare_review_promotion(
            root2.path(),
            "work",
            "Person",
            "Jordan Rivera",
            "Adopting existing.",
            &[],
            false,
            &Default::default(),
        )
        .unwrap();
        assert!(promo_unmarked.identity.is_none());
        let j2 = solstone_core_entity::read_entity_identity(root2.path(), "jordan_rivera")
            .unwrap()
            .unwrap();
        assert_ne!(j2.value().get("is_principal"), Some(&Value::Bool(true)));

        // 6. Two pre-existing principals are not rewritten. The new owner-name person is unmarked. JournalOwner.id is None
        let root3 = tempfile::tempdir().unwrap();
        solstone_core_facets::create_facet(root3.path(), "work", "Work", "", "", "", None).unwrap();
        fs::create_dir_all(root3.path().join("config")).unwrap();
        fs::write(
            root3.path().join("config/journal.json"),
            r#"{"identity":{"name":"Jordan Rivera"}}"#,
        )
        .unwrap();
        solstone_core_entity::save_entity_identity(
            root3.path(),
            "p1",
            &json!({"id":"p1","name":"P1","type":"Person","is_principal":true}),
            None,
        )
        .unwrap();
        solstone_core_entity::save_entity_identity(
            root3.path(),
            "p2",
            &json!({"id":"p2","name":"P2","type":"Person","is_principal":true}),
            None,
        )
        .unwrap();
        let promo_two_p = solstone_core_facets::prepare_review_promotion(
            root3.path(),
            "work",
            "Person",
            "Jordan Rivera",
            "New person.",
            &[],
            false,
            &Default::default(),
        )
        .unwrap();
        assert_ne!(
            promo_two_p
                .identity
                .as_ref()
                .unwrap()
                .after
                .get("is_principal"),
            Some(&Value::Bool(true))
        );
        let owner3 = JournalOwner::load(root3.path()).unwrap();
        assert!(owner3.id.is_none());
    }
}
