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
    capture_owner_expectations(&mut prepared, context, &hook)?;
    let mut state = PrePostState::None;
    if !prepared.config.contains_key("skip_reason") {
        if let Some(gate) = stage.gate {
            match gate(&prepared, context).map_err(RuntimeOutcome::StageFailed)? {
                GateDecision::Proceed => {}
                GateDecision::Skip(reason) => {
                    prepared
                        .config
                        .insert("skip_reason".to_owned(), json!(reason));
                }
            }
        }
        if !prepared.config.contains_key("skip_reason") {
            if let Some(build) = stage.build {
                match build(
                    &mut prepared,
                    context,
                    &solstone_core_generate::Cancellation::never(),
                ) {
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
    let mut state = match state {
        PrePostState::None => FrozenState::None,
        PrePostState::DailySchedule(v) => FrozenState::DailySchedule(v),
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
    attach_daily_location(&mut prepared, &mut state, context, &hook)?;
    prepared.config.remove("_briefing_inclusion");
    Ok(
        json!({"version":1,"prepared":{"name":prepared.name,"config":prepared.config},"hook":hook,"state":state}),
    )
}

#[cfg(test)]
thread_local! {
    static CAPTURE_BARRIER: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
}

const LOCATION_PART_BYTE_CAP: usize = 16384;
const LOCATION_CAVEAT: &str = "These words do not establish someone else's presence and do not establish a future commitment.";

fn attach_daily_location(
    prepared: &mut PreparedTalent,
    state: &mut FrozenState,
    context: &ExecutionContext,
    hook: &str,
) -> Result<(), RuntimeOutcome> {
    use solstone_core_location::daily_location_fingerprint;
    let Some(day) = prepared
        .config
        .get("day")
        .and_then(Value::as_str)
        .map(str::to_owned)
    else {
        return Ok(());
    };
    if !crate::daily_location::location_applies(&context.journal, hook, &day)
        .map_err(|error| failure(prepared, error))?
    {
        return Ok(());
    }
    // Source mutation cannot pass through capture, including an ABA returning
    // to the same bytes after rendering transient words.
    let _admission = solstone_core_journal_io::hold_daily_location_admission(&context.journal)
        .map_err(|error| failure(prepared, error.to_string()))?;
    let first = daily_location_fingerprint(&context.journal, &day)
        .map_err(|error| failure(prepared, error.to_string()))?;
    #[cfg(test)]
    CAPTURE_BARRIER.with(|slot| {
        if let Some(barrier) = slot.borrow_mut().take() {
            barrier();
        }
    });
    let rendered =
        render_location_part(&context.journal, &day).map_err(|error| failure(prepared, error))?;
    let mut ordinary = crate::ordinary_text_parts(&prepared.config);
    let (part, omitted) = if hook == "morning_briefing"
        && matches!(state, FrozenState::MorningBriefing(_))
    {
        let activities = match state {
            FrozenState::MorningBriefing(bs) => bs.activities.clone(),
            _ => unreachable!(),
        };

        match fit_location_part(hook, &day, &rendered, &ordinary, prepared, state) {
            Ok(res) => res,
            Err(ref e) if e == "location coverage does not fit the input budget" => {
                let mut curr_meta_str = prepared
                    .config
                    .get("briefing_metadata")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .ok_or_else(|| {
                        failure(prepared, "missing briefing_metadata in config".to_owned())
                    })?;

                let mut metadata: Value = serde_json::from_str(&curr_meta_str).map_err(|e| {
                    failure(prepared, format!("invalid briefing_metadata JSON: {e}"))
                })?;
                let obj = metadata.as_object_mut().ok_or_else(|| {
                    failure(prepared, "briefing_metadata must be an object".to_owned())
                })?;

                let mut gaps: Vec<String> = obj
                    .get("gaps")
                    .and_then(Value::as_array)
                    .map(|arr| {
                        arr.iter()
                            .filter_map(Value::as_str)
                            .map(str::to_owned)
                            .collect()
                    })
                    .unwrap_or_default();

                let inclusion_arr = prepared
                    .config
                    .get("_briefing_inclusion")
                    .and_then(Value::as_array)
                    .ok_or_else(|| {
                        failure(
                            prepared,
                            "briefing inclusion does not match activities".to_owned(),
                        )
                    })?;

                if inclusion_arr.len() != activities.len() {
                    return Err(failure(
                        prepared,
                        "briefing inclusion does not match activities".to_owned(),
                    ));
                }

                let n = activities.len();
                let mut include_activities = Vec::with_capacity(n);
                let mut include_stories = Vec::with_capacity(n);

                for (i, item) in inclusion_arr.iter().enumerate() {
                    let Some(obj) = item.as_object() else {
                        return Err(failure(
                            prepared,
                            "briefing inclusion does not match activities".to_owned(),
                        ));
                    };
                    let facet = obj.get("facet").and_then(Value::as_str);
                    let day = obj.get("day").and_then(Value::as_str);
                    let id = obj.get("id").and_then(Value::as_str);
                    let act_inc = obj.get("activity").and_then(Value::as_bool);
                    let story_inc = obj.get("story").and_then(Value::as_bool);

                    if facet != Some(&activities[i].facet)
                        || day != Some(&activities[i].day)
                        || id != Some(&activities[i].id)
                    {
                        return Err(failure(
                            prepared,
                            "briefing inclusion does not match activities".to_owned(),
                        ));
                    }
                    let (Some(act_inc), Some(story_inc)) = (act_inc, story_inc) else {
                        return Err(failure(
                            prepared,
                            "briefing inclusion does not match activities".to_owned(),
                        ));
                    };
                    include_activities.push(act_inc);
                    include_stories.push(story_inc);
                }

                loop {
                    let story_to_drop = (0..n).rev().find(|&i| {
                        include_activities[i]
                            && include_stories[i]
                            && activities[i].story_body.is_some()
                    });

                    let (drop_idx, is_story_drop) = if let Some(idx) = story_to_drop {
                        (idx, true)
                    } else if let Some(idx) = (0..n).rev().find(|&i| include_activities[i]) {
                        (idx, false)
                    } else {
                        return Err(failure(
                            prepared,
                            "prepared request does not fit the floor input budget".to_owned(),
                        ));
                    };

                    let prev_digest = crate::morning_briefing::render_digest_activities(
                        &activities,
                        &include_activities,
                        &include_stories,
                    );

                    let ordinary_has_prev = crate::ordinary_text_parts(&prepared.config)
                        .iter()
                        .any(|p| p.contains(&prev_digest));
                    if !ordinary_has_prev {
                        return Err(failure(
                            prepared,
                            "ordinary text does not contain activity_digest".to_owned(),
                        ));
                    }

                    if is_story_drop {
                        include_stories[drop_idx] = false;
                    } else {
                        include_activities[drop_idx] = false;
                        include_stories[drop_idx] = false;
                    }
                    gaps.retain(|gap| !crate::morning_briefing::is_omission_gap(gap, &activities));
                    gaps.extend(crate::morning_briefing::omission_gaps(
                        &activities,
                        &include_activities,
                        &include_stories,
                    ));

                    let new_digest = crate::morning_briefing::render_digest_activities(
                        &activities,
                        &include_activities,
                        &include_stories,
                    );

                    for field in ["prompt", "user_instruction", "transcript"] {
                        if let Some(Value::String(text)) = prepared.config.get_mut(field) {
                            *text = text.replace(&prev_digest, &new_digest);
                        }
                    }

                    let new_preamble = crate::morning_briefing::coverage_preamble(&gaps);
                    let mut meta_val: Value =
                        serde_json::from_str(&curr_meta_str).map_err(|e| {
                            failure(prepared, format!("invalid briefing_metadata JSON: {e}"))
                        })?;
                    let meta_obj = meta_val.as_object_mut().ok_or_else(|| {
                        failure(prepared, "briefing_metadata must be an object".to_owned())
                    })?;
                    meta_obj.insert(
                        "gaps".into(),
                        Value::Array(gaps.iter().cloned().map(Value::String).collect()),
                    );
                    meta_obj.insert("coverage_preamble".into(), Value::String(new_preamble));
                    let new_meta_str = serde_json::to_string_pretty(&meta_obj).map_err(|e| {
                        failure(
                            prepared,
                            format!("failed to serialize briefing_metadata: {e}"),
                        )
                    })?;

                    for field in ["prompt", "user_instruction", "transcript"] {
                        if let Some(Value::String(text)) = prepared.config.get_mut(field) {
                            *text = text.replace(&curr_meta_str, &new_meta_str);
                        }
                    }
                    prepared.config.insert(
                        "briefing_metadata".into(),
                        Value::String(new_meta_str.clone()),
                    );
                    if let FrozenState::MorningBriefing(bs) = state {
                        bs.values.insert(
                            "briefing_metadata".into(),
                            Value::String(new_meta_str.clone()),
                        );
                    }
                    curr_meta_str = new_meta_str;

                    ordinary = crate::ordinary_text_parts(&prepared.config);
                    match fit_location_part(hook, &day, &rendered, &ordinary, prepared, state) {
                        Ok(res) => break res,
                        Err(ref e) if e == "location coverage does not fit the input budget" => {
                            continue;
                        }
                        Err(e) => return Err(failure(prepared, e)),
                    }
                }
            }
            Err(e) => return Err(failure(prepared, e)),
        }
    } else {
        fit_location_part(hook, &day, &rendered, &ordinary, prepared, state)
            .map_err(|error| failure(prepared, error))?
    };
    let second = daily_location_fingerprint(&context.journal, &day)
        .map_err(|error| failure(prepared, error.to_string()))?;
    if first != second {
        return Err(failure(
            prepared,
            "daily location changed while it was read".to_owned(),
        ));
    }
    prepared.config.insert(
        "_daily_location_fingerprint".to_owned(),
        Value::String(second),
    );
    if !ordinary.is_empty()
        && let Some(text) = part
    {
        prepared
            .config
            .insert("_daily_location_text".to_owned(), Value::String(text));
    }
    if omitted > 0 {
        prepared.config.insert(
            "_daily_location_omitted".to_owned(),
            Value::Number(omitted.into()),
        );
    }
    if hook == "morning_briefing"
        && let FrozenState::MorningBriefing(bs) = state
    {
        let old_meta_str = prepared
            .config
            .get("briefing_metadata")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| failure(prepared, "missing briefing_metadata in config".to_owned()))?;

        let ordinary_contains_old = crate::ordinary_text_parts(&prepared.config)
            .iter()
            .any(|p| p.contains(&old_meta_str));
        if !ordinary_contains_old {
            return Err(failure(
                prepared,
                "ordinary text does not contain briefing_metadata".to_owned(),
            ));
        }

        let mut metadata: Value = serde_json::from_str(&old_meta_str)
            .map_err(|e| failure(prepared, format!("invalid briefing_metadata JSON: {e}")))?;
        let obj = metadata
            .as_object_mut()
            .ok_or_else(|| failure(prepared, "briefing_metadata must be an object".to_owned()))?;

        let mut gaps_vec: Vec<String> = obj
            .get("gaps")
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();

        if omitted > 0 {
            let actual_notice = format!("location entries omitted: {omitted}.");
            if !gaps_vec.contains(&actual_notice) {
                gaps_vec.push(actual_notice);
            }
        }

        let new_preamble = crate::morning_briefing::coverage_preamble(&gaps_vec);
        obj.insert(
            "gaps".to_owned(),
            Value::Array(gaps_vec.into_iter().map(Value::String).collect()),
        );
        obj.insert("coverage_preamble".to_owned(), Value::String(new_preamble));
        let new_meta_str = serde_json::to_string_pretty(&obj).map_err(|e| {
            failure(
                prepared,
                format!("failed to serialize briefing_metadata: {e}"),
            )
        })?;

        for field in ["prompt", "user_instruction", "transcript"] {
            if let Some(Value::String(text)) = prepared.config.get_mut(field) {
                *text = text.replace(&old_meta_str, &new_meta_str);
            }
        }

        prepared.config.insert(
            "briefing_metadata".to_owned(),
            Value::String(new_meta_str.clone()),
        );
        bs.values
            .insert("briefing_metadata".into(), Value::String(new_meta_str));

        use solstone_core_local::generate::compute_input_budget;
        let max_output_tokens = prepared
            .config
            .get("max_output_tokens")
            .and_then(Value::as_u64)
            .and_then(|tokens| u32::try_from(tokens).ok())
            .unwrap_or(0);
        let floor_budget = compute_input_budget(
            max_output_tokens,
            solstone_core_local::plan::LOCAL_MIN_CONTEXT_TOKENS,
        );
        let schema_tokens = prepared
            .config
            .get("json_schema")
            .map(|s| solstone_core_tokens::count(&s.to_string()))
            .unwrap_or(0);
        let total_text_tokens: u32 = crate::model_text_parts(&prepared.config)
            .iter()
            .map(|part| solstone_core_tokens::count(part))
            .sum();
        if total_text_tokens + schema_tokens > floor_budget {
            return Err(failure(
                prepared,
                "prepared request does not fit the floor input budget".to_owned(),
            ));
        }
    }
    Ok(())
}

fn render_location_part(journal: &std::path::Path, day: &str) -> Result<String, String> {
    let view = solstone_core_location::read_journal_day(journal, day)
        .map_err(|error| error.to_string())?;
    let store = solstone_core_location::places::PlaceStore::read_strict(journal)
        .map_err(|error| error.to_string())?;
    Ok(solstone_core_location::projection::render_day_document(
        &view, &store,
    ))
}

fn location_label(hook: &str, day: &str) -> String {
    if hook == "morning_briefing" {
        format!("Location words for {day}, the analysis day.")
    } else {
        format!("Location words for {day}.")
    }
}

fn fit_location_part(
    hook: &str,
    day: &str,
    rendered: &str,
    ordinary: &[String],
    prepared: &PreparedTalent,
    state: &FrozenState,
) -> Result<(Option<String>, u32), String> {
    use solstone_core_local::generate::compute_input_budget;
    let max_output_tokens = prepared
        .config
        .get("max_output_tokens")
        .and_then(Value::as_u64)
        .and_then(|tokens| u32::try_from(tokens).ok())
        .unwrap_or(0);
    // Location fills the room left beside the talent's own input at the
    // journal's smallest window, so every thinking backend gets the same request.
    let budget = compute_input_budget(
        max_output_tokens,
        solstone_core_local::plan::LOCAL_MIN_CONTEXT_TOKENS,
    );

    if hook != "morning_briefing" || !matches!(state, FrozenState::MorningBriefing(_)) {
        let ordinary_tokens = ordinary.iter().fold(0u32, |total, text| {
            total.saturating_add(solstone_core_tokens::count(text))
        });
        let room = budget.saturating_sub(ordinary_tokens);
        let label = location_label(hook, day);
        if room == 0 {
            return Ok((None, clip_location_part(&label, rendered, 0).1));
        }
        let (part, omitted) = clip_location_to_room(&label, rendered, room);
        debug_assert!(
            part.as_ref()
                .is_none_or(|text| solstone_core_tokens::count(text) <= room)
        );
        return Ok((part, omitted));
    }

    let schema_tokens = prepared
        .config
        .get("json_schema")
        .map(|s| solstone_core_tokens::count(&s.to_string()))
        .unwrap_or(0);
    let ordinary_tokens = ordinary.iter().fold(0u32, |total, text| {
        total.saturating_add(solstone_core_tokens::count(text))
    });
    let room = budget
        .saturating_sub(ordinary_tokens)
        .saturating_sub(schema_tokens);

    let total = u32::try_from(
        rendered
            .split_inclusive('\n')
            .filter(|line| line.starts_with("- "))
            .count(),
    )
    .unwrap_or(u32::MAX);

    if total == 0 {
        let label = location_label(hook, day);
        let (part, omitted) = clip_location_part(&label, rendered, 0);
        return Ok((part, omitted));
    }

    let worst_notice = format!("location entries omitted: {total}.");
    let old_meta_str = prepared
        .config
        .get("briefing_metadata")
        .and_then(Value::as_str)
        .ok_or_else(|| "missing briefing_metadata in config".to_owned())?;
    let mut meta_val: Value = serde_json::from_str(old_meta_str)
        .map_err(|e| format!("invalid briefing_metadata JSON: {e}"))?;
    let meta_obj = meta_val
        .as_object_mut()
        .ok_or_else(|| "briefing_metadata must be an object".to_owned())?;

    let mut worst_gaps: Vec<String> = meta_obj
        .get("gaps")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    if !worst_gaps.contains(&worst_notice) {
        worst_gaps.push(worst_notice.clone());
    }

    let worst_preamble = crate::morning_briefing::coverage_preamble(&worst_gaps);
    meta_obj.insert(
        "gaps".into(),
        Value::Array(worst_gaps.into_iter().map(Value::String).collect()),
    );
    meta_obj.insert("coverage_preamble".into(), Value::String(worst_preamble));
    let worst_meta_str = serde_json::to_string_pretty(&meta_obj)
        .map_err(|e| format!("failed to serialize briefing_metadata: {e}"))?;

    let worst_ordinary_tokens = ordinary.iter().fold(0u32, |sum, part| {
        if part.contains(old_meta_str) {
            let replaced = part.replace(old_meta_str, &worst_meta_str);
            sum.saturating_add(solstone_core_tokens::count(&replaced))
        } else {
            sum.saturating_add(solstone_core_tokens::count(part))
        }
    });
    let meta_growth = if ordinary.iter().any(|p| p.contains(old_meta_str)) {
        worst_ordinary_tokens.saturating_sub(ordinary_tokens)
    } else {
        solstone_core_tokens::count(&worst_meta_str)
            .saturating_sub(solstone_core_tokens::count(old_meta_str))
    };
    let notice_tokens = solstone_core_tokens::count(&worst_notice);
    let needed = notice_tokens + meta_growth;

    if room < needed {
        return Err("location coverage does not fit the input budget".to_owned());
    }

    let observation_room = room.saturating_sub(needed);

    let label = location_label(hook, day);
    let (part, omitted) = if observation_room == 0 {
        (Some(worst_notice), total)
    } else {
        let (clipped_part, omitted) = clip_location_to_room(&label, rendered, observation_room);
        if omitted == total {
            (Some(worst_notice), total)
        } else if omitted > 0 {
            let actual_notice = format!("location entries omitted: {omitted}.");
            let text = if let Some(mut existing) = clipped_part {
                if !existing.ends_with('\n') {
                    existing.push('\n');
                }
                existing.push_str(&actual_notice);
                existing
            } else {
                actual_notice
            };
            (Some(text), omitted)
        } else {
            (clipped_part, 0)
        }
    };

    Ok((part, omitted))
}

/// Keep the longest prefix of whole observation lines that fits `room` tokens
/// and the part's byte cap.
fn clip_location_to_room(label: &str, rendered: &str, room: u32) -> (Option<String>, u32) {
    let (part, omitted) = clip_location_part(label, rendered, LOCATION_PART_BYTE_CAP);
    let Some(text) = part else {
        return (None, omitted);
    };
    if solstone_core_tokens::count(&text) <= room {
        return (Some(text), omitted);
    }
    // The byte end of each kept observation line, in order.
    let mut ends = Vec::new();
    let mut end = 0;
    for line in text.split_inclusive('\n') {
        end += line.len();
        if line.starts_with("- ") {
            ends.push(end);
        }
    }
    let total = omitted + u32::try_from(ends.len()).unwrap_or(u32::MAX);
    // The most lines that fit: fits(0) holds and fits(ends.len()) does not.
    let fits =
        |lines: usize| lines == 0 || solstone_core_tokens::count(&text[..ends[lines - 1]]) <= room;
    let (mut low, mut high) = (0, ends.len());
    while high - low > 1 {
        let middle = (low + high) / 2;
        if fits(middle) {
            low = middle;
        } else {
            high = middle;
        }
    }
    if low == 0 {
        return (None, total);
    }
    let kept = u32::try_from(low).unwrap_or(u32::MAX);
    (Some(text[..ends[low - 1]].to_owned()), total - kept)
}

/// Keep a prefix of whole observation lines that fits in `max_bytes`.
pub(crate) fn clip_location_part(
    label: &str,
    rendered: &str,
    max_bytes: usize,
) -> (Option<String>, u32) {
    let mut heading = String::new();
    let mut observations = Vec::new();
    for line in rendered.split_inclusive('\n') {
        if line.starts_with("- ") {
            observations.push(line);
        } else if observations.is_empty() {
            heading.push_str(line);
        }
    }
    let total = u32::try_from(observations.len()).unwrap_or(u32::MAX);
    let prefix = format!("{label}\n{LOCATION_CAVEAT}\n{heading}");
    if max_bytes == 0 || prefix.len() > max_bytes || observations.is_empty() {
        return (None, total);
    }
    let mut kept = prefix;
    let mut kept_count = 0u32;
    for line in observations {
        if kept.len() + line.len() > max_bytes {
            break;
        }
        kept.push_str(line);
        kept_count += 1;
    }
    if kept_count == 0 {
        return (None, total);
    }
    (Some(kept), total - kept_count)
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
        // Every current daily hook freezes its own pre-state.
        FrozenState::None => (PrePostState::None, ""),
        FrozenState::DailySchedule(v) => (PrePostState::DailySchedule(v), "daily_schedule"),
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
    Ok((
        PreparedTalent::new(name.to_owned(), config),
        Some((stage, state)),
    ))
}

pub fn packet_digest(packet: &Value) -> String {
    solstone_core_indexer::daily_evidence::digest(packet)
}

fn review_identity_stage_failed(
    prepared: &PreparedTalent,
    error: solstone_core_facets::FacetIdentityError,
) -> RuntimeOutcome {
    review_owner_stage_failed(prepared, error.into())
}

fn review_owner_stage_failed(
    prepared: &PreparedTalent,
    error: solstone_core_entity::ReviewOwnerError,
) -> RuntimeOutcome {
    let identity = solstone_core_journal_io::DailyUnitIdentity::new(
        prepared
            .config
            .get("day")
            .and_then(Value::as_str)
            .unwrap_or_default(),
        prepared.name.clone(),
        prepared
            .config
            .get("facet")
            .and_then(Value::as_str)
            .map(str::to_owned),
    );
    RuntimeOutcome::StageFailed(match error {
        solstone_core_entity::ReviewOwnerError::Conflict { kind, detail } => {
            StageError::owner_conflict(&identity, "daily_publication", kind, detail)
        }
        solstone_core_entity::ReviewOwnerError::Failed { detail } => StageError::new(
            "publication",
            "daily_publication",
            identity.name.clone(),
            detail,
        )
        .with_identity(&identity),
    })
}

fn capture_owner_expectations(
    prepared: &mut PreparedTalent,
    context: &ExecutionContext,
    hook: &str,
) -> Result<(), RuntimeOutcome> {
    if prepared.config.contains_key("skip_reason") {
        return Ok(());
    }
    let observe_review = hook == "entities:entities_review";
    let facet_ids = {
        let _guard = solstone_core_facets::hold_facet_trust_lock(&context.journal)
            .map_err(|e| failure(prepared, e.to_string()))?;
        let names = if matches!(
            hook,
            "entities:entities_review" | "entities:entity_suggest" | "entities:entity_observer"
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
            let id = if observe_review {
                solstone_core_facets::observe_facet_write_identity(&context.journal, &facet)
                    .map_err(|error| review_identity_stage_failed(prepared, error))?
            } else {
                solstone_core_facets::facet_write_identity(&context.journal, &facet)
                    .map_err(|error| failure(prepared, error.to_string()))?
            };
            identities.insert(facet, json!(id));
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
        let action =
            crate::writers::bind_output_action(&context.journal, &path, Vec::new(), observe_review)
                .map_err(|error| {
                    if observe_review {
                        review_owner_stage_failed(prepared, error)
                    } else {
                        failure(prepared, error.to_string())
                    }
                })?;
        let crate::writers::PreparedDailyAction::Output { path, before, .. } = action else {
            return Err(failure(prepared, "invalid artifact preparation".to_owned()));
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
        )
        .map_err(|e| failure(prepared, e))?
        .before;
        prepared.config.insert(
            "_daily_time_before".to_owned(),
            before.unwrap_or(Value::Null),
        );
    }
    Ok(())
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
    fn review_admission_snapshot_observes_malformed_facet_without_repair() {
        let root = tempfile::tempdir().unwrap();
        solstone_core_facets::create_facet(root.path(), "work", "Work", "", "", "", None).unwrap();
        let path = root.path().join("facets/work/facet.json");
        std::fs::write(&path, b"{").unwrap();
        let prepared = PreparedTalent::new(
            "entities:entities_review".to_owned(),
            json!({
                "type":"generate","max_output_tokens":1024,
                "day":"20260910",
                "facet":"work",
                "prompt":"review",
                "hook":{"pre":"entities:entities_review","post":"entities:entities_review"}
            })
            .as_object()
            .unwrap()
            .clone(),
        );
        let outcome = freeze(
            prepared,
            &ExecutionContext {
                journal: root.path().to_owned(),
            },
        )
        .unwrap_err();
        let RuntimeOutcome::StageFailed(error) = outcome else {
            panic!("{outcome:?}");
        };
        assert_eq!(error.phase, "publication");
        assert_eq!(error.reason_code(), "talent_stage_failed");
        assert_eq!(error.owner_conflict_kind(), None);
        assert_eq!(std::fs::read(&path).unwrap(), b"{");
        assert_eq!(
            std::fs::read_dir(path.parent().unwrap())
                .unwrap()
                .filter_map(Result::ok)
                .filter(|entry| entry.file_name().to_string_lossy().contains("wedged"))
                .count(),
            0
        );
    }

    #[test]
    fn review_admission_snapshot_types_missing_facet_as_owning_facet_changed() {
        let root = tempfile::tempdir().unwrap();
        let prepared = PreparedTalent::new(
            "entities:entities_review".to_owned(),
            json!({
                "type":"generate","max_output_tokens":1024,
                "day":"20260910",
                "facet":"missing",
                "prompt":"review",
                "hook":{"pre":"entities:entities_review","post":"entities:entities_review"}
            })
            .as_object()
            .unwrap()
            .clone(),
        );
        let outcome = freeze(
            prepared,
            &ExecutionContext {
                journal: root.path().to_owned(),
            },
        )
        .unwrap_err();
        let RuntimeOutcome::StageFailed(error) = outcome else {
            panic!("{outcome:?}");
        };
        assert_eq!(error.phase, "conflict");
        assert_eq!(error.reason_code(), "daily_owner_conflict");
        assert_eq!(error.owner_conflict_kind(), Some("owning_facet_changed"));
        assert_eq!(error.day(), "20260910");
        assert_eq!(error.facet(), Some("missing"));
    }

    #[test]
    fn frozen_packet_rejects_hook_state_mismatch_and_retains_legitimate_no_output() {
        let root = tempfile::tempdir().unwrap();
        let context = ExecutionContext {
            journal: root.path().to_owned(),
        };
        let config = Map::from_iter([
            (
                "hook".to_owned(),
                json!({"pre":"daily_schedule","post":"daily_schedule"}),
            ),
            ("skip_reason".to_owned(), json!("no_input")),
        ]);
        let packet = freeze(
            PreparedTalent::new("daily_schedule".to_owned(), config),
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

    const LOCATION_DAY: &str = "20261001";

    fn write_bytes(root: &std::path::Path, rel: &str, bytes: &[u8]) {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    fn location_journal(root: &std::path::Path, cutoff: &str) {
        write(
            root,
            "config/journal.json",
            &format!(
                r#"{{"identity":{{"timezone":"America/Denver"}},"daily_inputs":{{"location":{{"location-words-1":"{cutoff}"}}}}}}"#
            ),
        );
        let header = r#"{"accuracy":"full","fix_count":0,"gap":false,"kind":"location","platform":"ios","schema":"solstone.location.segment/1","source":"location","tier":"full"}"#;
        let body = format!(
            "{header}\n{}\n{}\n{}\n",
            r#"{"alt":null,"course":null,"h_acc":10.0,"lat":39.7555,"lon":-105.2211,"schema":"solstone.location.fix/1","speed":null,"stationary":false,"t":"2026-10-01T20:00:10Z","v_acc":null}"#,
            r#"{"alt":null,"course":null,"h_acc":10.0,"lat":39.7556,"lon":-105.2212,"schema":"solstone.location.fix/1","speed":null,"stationary":false,"t":"2026-10-01T20:04:10Z","v_acc":null}"#,
            r#"{"arrival":"2026-10-01T21:02:00Z","departure":"2026-10-01T22:40:00Z","h_acc":40,"lat":39.7527,"lon":-105.0001,"schema":"solstone.location.visit/1"}"#
        );
        write_bytes(
            root,
            &format!("chronicle/{LOCATION_DAY}/phone/140000_300/location.jsonl"),
            body.as_bytes(),
        );
        let store = solstone_core_location::places::PlaceStore {
            version: 1,
            places: vec![solstone_core_location::places::Place {
                id: "named".to_owned(),
                latitude: 39.7527,
                longitude: -105.0001,
                name: Some("$source_packet".to_owned()),
                alias: None,
                hidden: false,
            }],
        };
        store.write(root).unwrap();
    }

    fn prepared(name: &str, day: &str, extra: Value) -> PreparedTalent {
        let mut config = Map::from_iter([
            ("type".to_owned(), json!("generate")),
            ("max_output_tokens".to_owned(), json!(1024)),
            ("day".to_owned(), json!(day)),
            ("prompt".to_owned(), json!(format!("{name} ordinary"))),
        ]);
        if let Some(object) = extra.as_object() {
            for (key, value) in object {
                config.insert(key.clone(), value.clone());
            }
        }
        PreparedTalent::new(name.to_owned(), config)
    }

    fn text_parts(prepared: &PreparedTalent) -> Vec<String> {
        crate::model_text_parts(&prepared.config)
    }

    fn rendered_observations(root: &std::path::Path) -> Vec<String> {
        let view = solstone_core_location::read_journal_day(root, LOCATION_DAY).unwrap();
        let store = solstone_core_location::places::PlaceStore::read_strict(root).unwrap();
        solstone_core_location::projection::render_day_document(&view, &store)
            .lines()
            .filter(|line| line.starts_with("- "))
            .map(|line| format!("{line}\n"))
            .collect()
    }

    #[test]
    fn freeze_appends_location_for_its_recipients_only() {
        let root = tempfile::tempdir().unwrap();
        location_journal(root.path(), LOCATION_DAY);
        solstone_core_facets::create_facet(root.path(), "work", "Work", "", "", "", None).unwrap();
        let context = ExecutionContext {
            journal: root.path().to_owned(),
        };
        let observations = rendered_observations(root.path());
        assert!(observations.len() >= 2, "{observations:?}");
        assert!(
            observations
                .iter()
                .any(|line| line.contains("$source_packet")),
            "{observations:?}"
        );

        let recipients = [
            (
                "morning_briefing",
                prepared(
                    "morning_briefing",
                    LOCATION_DAY,
                    json!({
                        "hook":{"pre":"morning_briefing"},
                        "skip_reason":"no_input",
                        "messages":[{"role":"user","content":"briefing ordinary"}]
                    }),
                ),
            ),
            (
                "entities:entity_suggest",
                prepared(
                    "entities:entity_suggest",
                    LOCATION_DAY,
                    json!({
                        "facet":"work",
                        "hook":{"pre":"entities:entity_suggest","post":"entities:entity_suggest"}
                    }),
                ),
            ),
            (
                "entities:entity_observer",
                prepared(
                    "entities:entity_observer",
                    LOCATION_DAY,
                    json!({
                        "facet":"work",
                        "hook":{"pre":"entities:entity_observer","post":"entities:entity_observer"},
                        "skip_reason":"no_input"
                    }),
                ),
            ),
        ];
        for (hook, talent) in recipients {
            let ordinary = crate::ordinary_text_parts(&talent.config);
            assert!(!ordinary.is_empty(), "{hook}");
            let packet = freeze(talent, &context).unwrap();
            let config = packet["prepared"]["config"].as_object().unwrap();
            let fingerprint = config
                .get("_daily_location_fingerprint")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("{hook} missing fingerprint"));
            assert_eq!(fingerprint.len(), 64);
            assert!(config.get("_daily_location_omitted").is_none(), "{hook}");
            let text = config
                .get("_daily_location_text")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("{hook} missing location text"));
            assert!(text.contains(LOCATION_DAY), "{hook}");
            for line in &observations {
                assert!(text.contains(line.trim_end()), "{hook} lost {line}");
            }
            for coordinate in ["39.75", "105.2", "105.0"] {
                assert!(!text.contains(coordinate), "{hook} {coordinate}");
            }
            let thawed = thaw(&packet).unwrap().0;
            let parts = text_parts(&thawed);
            assert!(parts.len() > ordinary.len(), "{hook}");
            assert_eq!(&parts[..ordinary.len()], ordinary.as_slice(), "{hook}");
            assert_eq!(parts.last().map(String::as_str), Some(text), "{hook}");
            for part in &parts {
                for coordinate in ["39.75", "105.2", "105.0"] {
                    assert!(!part.contains(coordinate), "{hook}");
                }
            }
        }

        let excluded = [
            prepared(
                "daily_schedule",
                LOCATION_DAY,
                json!({"hook":{"pre":"daily_schedule","post":"daily_schedule"},"skip_reason":"no_input"}),
            ),
            prepared(
                "entities:entities_review",
                LOCATION_DAY,
                json!({
                    "facet":"work",
                    "hook":{"pre":"entities:entities_review","post":"entities:entities_review"},
                    "skip_reason":"no_input"
                }),
            ),
            prepared(
                "entities:entity_observer",
                "20260901",
                json!({"facet":"work","hook":{"pre":"entities:entity_observer","post":"entities:entity_observer"},"skip_reason":"no_input"}),
            ),
        ];
        for talent in excluded {
            let name = talent.name.clone();
            let packet = freeze(talent, &context).unwrap();
            let config = packet["prepared"]["config"].as_object().unwrap();
            assert!(
                config.get("_daily_location_fingerprint").is_none(),
                "{name}"
            );
            assert!(config.get("_daily_location_text").is_none(), "{name}");
            assert!(config.get("_daily_location_omitted").is_none(), "{name}");
        }

        let empty = prepared(
            "entities:entity_observer",
            LOCATION_DAY,
            json!({"facet":"work","hook":{"pre":"entities:entity_observer","post":"entities:entity_observer"},"skip_reason":"no_input","prompt":""}),
        );
        let packet = freeze(empty, &context).unwrap();
        let config = packet["prepared"]["config"].as_object().unwrap();
        assert_eq!(
            config
                .get("_daily_location_fingerprint")
                .and_then(Value::as_str)
                .map(str::len),
            Some(64)
        );
        assert!(config.get("_daily_location_text").is_none());
        let parts = text_parts(&thaw(&packet).unwrap().0);
        assert!(parts.is_empty());

        write(
            root.path(),
            "config/journal.json",
            r#"{"identity":{"timezone":"America/Denver"},"daily_inputs":{"location":{"location-words-1":"not-a-day"}}}"#,
        );
        let malformed = freeze(
            prepared(
                "entities:entity_observer",
                LOCATION_DAY,
                json!({"facet":"work","hook":{"pre":"entities:entity_observer","post":"entities:entity_observer"},"skip_reason":"no_input"}),
            ),
            &context,
        );
        assert!(malformed.is_err());
        let excluded_malformed = freeze(
            prepared(
                "daily_schedule",
                LOCATION_DAY,
                json!({"hook":{"pre":"daily_schedule","post":"daily_schedule"},"skip_reason":"no_input"}),
            ),
            &context,
        )
        .unwrap();
        assert!(
            excluded_malformed["prepared"]["config"]
                .get("_daily_location_fingerprint")
                .is_none()
        );
    }

    #[test]
    fn frozen_morning_briefing_with_location_fits_the_floor_budget() {
        let root = tempfile::tempdir().unwrap();
        location_journal(root.path(), LOCATION_DAY);

        let loc_path = root.path().join(format!(
            "chronicle/{LOCATION_DAY}/phone/140000_300/location.jsonl"
        ));
        let mut loc_content = std::fs::read_to_string(&loc_path).unwrap();
        for i in 0..70 {
            let hour = i / 60;
            let min = i % 60;
            loc_content.push_str(&format!(
                "{{\"arrival\":\"2026-10-01T{hour:02}:{min:02}:00Z\",\"departure\":\"2026-10-01T{hour:02}:{min:02}:30Z\",\"h_acc\":40,\"lat\":39.7527,\"lon\":-105.0001,\"schema\":\"solstone.location.visit/1\"}}\n"
            ));
        }
        std::fs::write(&loc_path, loc_content).unwrap();

        let observations = rendered_observations(root.path());
        let observation_bytes: usize = observations.iter().map(|s| s.len()).sum();
        assert!(
            observation_bytes > 8000,
            "rendered observations must total more than 8000 bytes, got {observation_bytes}"
        );

        write(
            root.path(),
            "health/local.ctx",
            &format!(
                "{}\n",
                solstone_core_local::plan::LOCAL_MIN_CONTEXT_TOKENS * 4
            ),
        );

        solstone_core_facets::create_facet(root.path(), "work", "Work", "", "", "", None).unwrap();
        let story_body = "S".repeat(1500);
        let story_tokens = solstone_core_tokens::count(&story_body);
        let record = json!({
            "id": "act-1",
            "title": "Short Activity",
            "start": "09:00",
            "story": {"body": story_body},
        });
        write(
            root.path(),
            &format!("facets/work/activities/{LOCATION_DAY}.jsonl"),
            &format!("{record}\n"),
        );

        let context = ExecutionContext {
            journal: root.path().to_owned(),
        };
        let max_output_tokens = 1024u32;
        let floor_budget = solstone_core_local::generate::compute_input_budget(
            max_output_tokens,
            solstone_core_local::plan::LOCAL_MIN_CONTEXT_TOKENS,
        );

        let unpadded_talent = prepared(
            "morning_briefing",
            LOCATION_DAY,
            json!({
                "hook": {"pre": "morning_briefing"},
                "prompt": "Prompt with $activity_digest and $briefing_metadata",
                "json_schema": {"type": "object"},
                "max_output_tokens": max_output_tokens,
            }),
        );
        let unpadded_packet = freeze(unpadded_talent, &context).unwrap();
        let (thawed_unpadded, _) = thaw(&unpadded_packet).unwrap();
        let unpadded_schema_tokens = match thawed_unpadded.config.get("json_schema") {
            Some(Value::String(s)) => solstone_core_tokens::count(s),
            Some(val) => solstone_core_tokens::count(&val.to_string()),
            None => 0,
        };
        let unpadded_text_tokens: u32 = crate::model_text_parts(&thawed_unpadded.config)
            .into_iter()
            .filter(|part| {
                thawed_unpadded
                    .config
                    .get("_daily_location_text")
                    .and_then(Value::as_str)
                    .is_none_or(|loc| part != loc)
            })
            .map(|part| solstone_core_tokens::count(&part))
            .sum();
        let unpadded_tokens = unpadded_text_tokens + unpadded_schema_tokens;

        let target_leftover = 400u32;
        let pad_tokens = floor_budget
            .saturating_sub(unpadded_tokens)
            .saturating_add(story_tokens)
            .saturating_sub(target_leftover);
        let user_instruction = " a".repeat(pad_tokens as usize);

        let talent = prepared(
            "morning_briefing",
            LOCATION_DAY,
            json!({
                "hook": {"pre": "morning_briefing"},
                "prompt": "Prompt with $activity_digest and $briefing_metadata",
                "json_schema": {"type": "object"},
                "user_instruction": user_instruction,
                "max_output_tokens": max_output_tokens,
            }),
        );
        let packet = freeze(talent, &context).unwrap();
        let (thawed, _) = thaw(&packet).unwrap();

        let schema_tokens = match thawed.config.get("json_schema") {
            Some(Value::String(s)) => solstone_core_tokens::count(s),
            Some(val) => solstone_core_tokens::count(&val.to_string()),
            None => 0,
        };
        let ordinary_text_tokens: u32 = crate::model_text_parts(&thawed.config)
            .into_iter()
            .filter(|part| {
                thawed
                    .config
                    .get("_daily_location_text")
                    .and_then(Value::as_str)
                    .is_none_or(|loc| part != loc)
            })
            .map(|part| solstone_core_tokens::count(&part))
            .sum();
        let ordinary_total = ordinary_text_tokens + schema_tokens;
        assert!(
            ordinary_total <= floor_budget,
            "ordinary request tokens {ordinary_total} must be <= floor budget {floor_budget}"
        );
        let leftover_floor_room = floor_budget.saturating_sub(ordinary_total);
        assert!(
            leftover_floor_room <= 800,
            "ordinary request tokens {ordinary_total} must be within 800 tokens of floor budget {floor_budget}"
        );

        let rendered_doc = render_location_part(root.path(), LOCATION_DAY).unwrap();
        let label = location_label("morning_briefing", LOCATION_DAY);
        let (fitted_16k, _) = clip_location_part(&label, &rendered_doc, 16384);
        let fitted_text_16k = fitted_16k.expect("16384-byte prefix must fit location observations");
        assert!(
            solstone_core_tokens::count(&fitted_text_16k) > leftover_floor_room,
            "token count of 16384-byte prefix ({}) must exceed leftover floor room ({leftover_floor_room})",
            solstone_core_tokens::count(&fitted_text_16k)
        );

        let total_text_tokens: u32 = crate::model_text_parts(&thawed.config)
            .iter()
            .map(|part| solstone_core_tokens::count(part))
            .sum();
        let total_tokens = total_text_tokens + schema_tokens;
        assert!(
            total_tokens <= floor_budget,
            "total tokens {total_tokens} must be <= floor budget {floor_budget}"
        );

        assert!(
            thawed.config.get("_daily_location_fingerprint").is_some(),
            "_daily_location_fingerprint must be present"
        );
        let omitted = thawed
            .config
            .get("_daily_location_omitted")
            .and_then(Value::as_u64)
            .expect("_daily_location_omitted must be present");

        if let Some(loc_text) = thawed
            .config
            .get("_daily_location_text")
            .and_then(Value::as_str)
        {
            let loc_obs: Vec<&str> = loc_text
                .lines()
                .filter(|line| line.starts_with("- "))
                .collect();
            let rendered_obs_trimmed: Vec<&str> =
                observations.iter().map(|s| s.trim_end()).collect();
            for line in &loc_obs {
                assert!(
                    rendered_obs_trimmed.contains(line),
                    "every observation line in location text must be a whole rendered observation: {line}"
                );
            }
            let absent_count = observations.len().saturating_sub(loc_obs.len()) as u64;
            assert_eq!(
                omitted, absent_count,
                "_daily_location_omitted must equal count of rendered observations absent from location text"
            );
            assert!(omitted >= 1, "at least one observation must be omitted");
        } else {
            assert_eq!(
                omitted,
                observations.len() as u64,
                "when _daily_location_text is absent, omitted must equal the full rendered observation count"
            );
        }

        let notice = format!("location entries omitted: {omitted}.");
        let model_parts = crate::model_text_parts(&thawed.config);
        assert!(
            model_parts.iter().any(|part| part.contains(&notice)),
            "location part in model_text_parts must contain omitted count"
        );
        for part in &model_parts {
            assert!(
                !part.to_lowercase().contains("complete"),
                "model text parts must not contain 'complete'"
            );
        }

        let raw_metadata = thawed
            .config
            .get("briefing_metadata")
            .and_then(Value::as_str)
            .expect("briefing_metadata must be present");
        let prompt_part = thawed
            .config
            .get("prompt")
            .and_then(Value::as_str)
            .expect("prompt must be present");
        assert!(
            prompt_part.contains(raw_metadata),
            "emitted prompt must contain exact briefing_metadata string"
        );
        assert!(
            model_parts.iter().any(|part| part.contains(raw_metadata)),
            "model_text_parts must contain exact briefing_metadata string"
        );

        let metadata: Value =
            serde_json::from_str(raw_metadata).expect("valid briefing_metadata JSON");
        let gaps = metadata["gaps"].as_array().expect("gaps array");
        assert!(
            gaps.iter().any(|g| g.as_str() == Some(&notice)),
            "saved briefing_metadata must contain omission notice in gaps"
        );
        assert!(
            gaps.iter().any(|g| {
                let s = g.as_str().unwrap_or("");
                s.starts_with("activities omitted: ") || s.starts_with("activity stories omitted: ")
            }),
            "gaps must contain activity/story omission gap: {gaps:?}"
        );
        let preamble = metadata["coverage_preamble"].as_str().unwrap();
        assert!(
            !preamble.to_lowercase().contains("complete"),
            "coverage_preamble must not contain 'complete'"
        );
        assert!(preamble.contains(" location entr"), "{preamble}");
        assert!(!preamble.contains("act-1"), "{preamble}");
        assert!(!preamble.contains(".."), "{preamble}");
    }

    #[test]
    fn morning_briefing_total_location_omission_fits_floor_budget() {
        let root = tempfile::tempdir().unwrap();
        location_journal(root.path(), LOCATION_DAY);
        let observations = rendered_observations(root.path());
        assert!(!observations.is_empty());
        let total = observations.len();

        solstone_core_facets::create_facet(root.path(), "work", "Work", "", "", "", None).unwrap();
        let story_body = "S".repeat(1500);
        let record = json!({
            "id": "act-1",
            "title": "Short Activity",
            "start": "09:00",
            "story": {"body": story_body},
        });
        write(
            root.path(),
            &format!("facets/work/activities/{LOCATION_DAY}.jsonl"),
            &format!("{record}\n"),
        );

        let context = ExecutionContext {
            journal: root.path().to_owned(),
        };
        let max_output_tokens = 1024u32;
        let floor_budget = solstone_core_local::generate::compute_input_budget(
            max_output_tokens,
            solstone_core_local::plan::LOCAL_MIN_CONTEXT_TOKENS,
        );

        let unpadded_talent = prepared(
            "morning_briefing",
            LOCATION_DAY,
            json!({
                "hook": {"pre": "morning_briefing"},
                "prompt": "Prompt with $activity_digest and $briefing_metadata",
                "json_schema": {"type": "object"},
                "max_output_tokens": max_output_tokens,
            }),
        );
        let unpadded_packet = freeze(unpadded_talent, &context).unwrap();
        let (thawed_unpadded, _) = thaw(&unpadded_packet).unwrap();
        let unpadded_schema_tokens = match thawed_unpadded.config.get("json_schema") {
            Some(Value::String(s)) => solstone_core_tokens::count(s),
            Some(val) => solstone_core_tokens::count(&val.to_string()),
            None => 0,
        };

        let expected_notice = format!("location entries omitted: {total}.");
        let notice_tokens = solstone_core_tokens::count(&expected_notice);
        let orig_meta_str = thawed_unpadded
            .config
            .get("briefing_metadata")
            .and_then(Value::as_str)
            .unwrap();
        let orig_meta_val: Value = serde_json::from_str(orig_meta_str).unwrap();
        let base_gaps: Vec<String> = orig_meta_val["gaps"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_owned())
            .collect();

        let mut story_dropped_gaps = base_gaps.clone();
        story_dropped_gaps.push("activity stories omitted: 1.".into());
        let story_dropped_preamble =
            crate::morning_briefing::coverage_preamble(&story_dropped_gaps);
        let mut story_dropped_meta_val = orig_meta_val.clone();
        {
            let obj = story_dropped_meta_val.as_object_mut().unwrap();
            obj.insert(
                "gaps".into(),
                Value::Array(
                    story_dropped_gaps
                        .iter()
                        .cloned()
                        .map(Value::String)
                        .collect(),
                ),
            );
            obj.insert(
                "coverage_preamble".into(),
                Value::String(story_dropped_preamble),
            );
        }
        let story_dropped_meta_str = serde_json::to_string_pretty(&story_dropped_meta_val).unwrap();

        let mut final_gaps = story_dropped_gaps;
        final_gaps.push(expected_notice.clone());
        let final_preamble = crate::morning_briefing::coverage_preamble(&final_gaps);
        let mut final_meta_val = story_dropped_meta_val.clone();
        {
            let obj = final_meta_val.as_object_mut().unwrap();
            obj.insert(
                "gaps".into(),
                Value::Array(final_gaps.iter().cloned().map(Value::String).collect()),
            );
            obj.insert("coverage_preamble".into(), Value::String(final_preamble));
        }
        let final_meta_str = serde_json::to_string_pretty(&final_meta_val).unwrap();

        let orig_prompt = thawed_unpadded
            .config
            .get("prompt")
            .and_then(Value::as_str)
            .unwrap();
        let story_snippet = format!("\nStory:\n{story_body}");
        let prompt_without_story = orig_prompt
            .replace(&story_snippet, "")
            .replace(orig_meta_str, &story_dropped_meta_str);
        let prompt_without_story_tokens = solstone_core_tokens::count(&prompt_without_story);

        let replaced_prompt =
            prompt_without_story.replace(&story_dropped_meta_str, &final_meta_str);
        let meta_growth = solstone_core_tokens::count(&replaced_prompt)
            .saturating_sub(prompt_without_story_tokens);
        let needed = notice_tokens + meta_growth;

        let other_tokens: u32 = crate::model_text_parts(&thawed_unpadded.config)
            .into_iter()
            .filter(|part| {
                part != orig_prompt
                    && thawed_unpadded
                        .config
                        .get("_daily_location_text")
                        .and_then(Value::as_str)
                        .is_none_or(|loc| part != loc)
            })
            .map(|part| solstone_core_tokens::count(&part))
            .sum();
        let base_tokens = prompt_without_story_tokens + other_tokens + unpadded_schema_tokens;

        let pad_tokens = floor_budget
            .saturating_sub(base_tokens)
            .saturating_sub(needed);
        let user_instruction = " a".repeat(pad_tokens as usize);

        let talent = prepared(
            "morning_briefing",
            LOCATION_DAY,
            json!({
                "hook": {"pre": "morning_briefing"},
                "prompt": "Prompt with $activity_digest and $briefing_metadata",
                "json_schema": {"type": "object"},
                "user_instruction": user_instruction,
                "max_output_tokens": max_output_tokens,
            }),
        );
        let packet = freeze(talent, &context).unwrap();
        let (thawed, _) = thaw(&packet).unwrap();

        let schema_tokens = match thawed.config.get("json_schema") {
            Some(Value::String(s)) => solstone_core_tokens::count(s),
            Some(val) => solstone_core_tokens::count(&val.to_string()),
            None => 0,
        };
        let total_text_tokens: u32 = crate::model_text_parts(&thawed.config)
            .iter()
            .map(|part| solstone_core_tokens::count(part))
            .sum();
        let total_tokens = total_text_tokens + schema_tokens;
        assert!(
            total_tokens <= floor_budget,
            "measured request tokens {total_tokens} must be <= floor budget {floor_budget}"
        );

        let loc_text = thawed
            .config
            .get("_daily_location_text")
            .and_then(Value::as_str)
            .expect("_daily_location_text must be present");
        assert_eq!(loc_text, &expected_notice);

        let omitted = thawed
            .config
            .get("_daily_location_omitted")
            .and_then(Value::as_u64)
            .expect("_daily_location_omitted must be present");
        assert_eq!(omitted, total as u64);

        let raw_metadata = thawed
            .config
            .get("briefing_metadata")
            .and_then(Value::as_str)
            .expect("briefing_metadata must be present");
        let prompt_part = thawed
            .config
            .get("prompt")
            .and_then(Value::as_str)
            .expect("prompt must be present");
        assert!(
            prompt_part.contains(raw_metadata),
            "emitted prompt must contain exact briefing_metadata string"
        );
        let parts = crate::model_text_parts(&thawed.config);
        assert!(
            parts.iter().any(|p| p.contains(raw_metadata)),
            "model_text_parts must contain exact briefing_metadata string"
        );
        assert!(parts.iter().any(|p| p.contains(&expected_notice)));

        for obs in &observations {
            assert!(
                !parts.iter().any(|p| p.contains(obs.trim())),
                "must have no rendered observation line"
            );
        }

        for coordinate in ["39.75", "105.2", "105.0"] {
            for part in &parts {
                assert!(!part.contains(coordinate), "must hide coordinates");
            }
        }

        for part in &parts {
            assert!(
                !part.to_lowercase().contains("complete"),
                "must not contain 'complete'"
            );
        }

        let metadata: Value =
            serde_json::from_str(raw_metadata).expect("briefing_metadata valid JSON");
        let gaps = metadata["gaps"].as_array().expect("gaps array");
        assert!(
            gaps.iter().any(|g| g.as_str() == Some(&expected_notice)),
            "saved metadata must contain the omission notice in gaps"
        );
        assert!(
            gaps.iter().any(|g| {
                let s = g.as_str().unwrap_or("");
                s.starts_with("activities omitted: ") || s.starts_with("activity stories omitted: ")
            }),
            "gaps must contain activity/story omission gap from padding: {gaps:?}"
        );
        let preamble = metadata["coverage_preamble"].as_str().unwrap();
        assert!(!preamble.to_lowercase().contains("complete"));
        assert!(preamble.contains(" location entr"), "{preamble}");
        assert!(!preamble.contains("act-1"), "{preamble}");
        assert!(!preamble.contains(".."), "{preamble}");
    }

    #[test]
    fn morning_briefing_drops_removable_activity_to_fit_location_coverage() {
        let root = tempfile::tempdir().unwrap();
        location_journal(root.path(), LOCATION_DAY);
        solstone_core_facets::create_facet(root.path(), "work", "Work", "", "", "", None).unwrap();
        let description = " a".repeat(14960);
        let record = json!({
            "id": "act-1",
            "title": "Activity",
            "start": "09:00",
            "description": description,
        });
        write(
            root.path(),
            &format!("facets/work/activities/{LOCATION_DAY}.jsonl"),
            &format!("{record}\n"),
        );

        let context = ExecutionContext {
            journal: root.path().to_owned(),
        };
        let talent = prepared(
            "morning_briefing",
            LOCATION_DAY,
            json!({
                "hook": {"pre": "morning_briefing"},
                "prompt": "Prompt with $activity_digest and $briefing_metadata",
                "json_schema": {"type": "object"},
                "max_output_tokens": 1024,
            }),
        );

        let stage = contract::resolve_hook("morning_briefing").unwrap();
        let mut probe_talent = talent.clone();
        let state = (stage.build.unwrap())(
            &mut probe_talent,
            &context,
            &solstone_core_generate::Cancellation::never(),
        )
        .unwrap();
        (stage.prompt_override.unwrap())(&mut probe_talent, &state).unwrap();
        let ordinary_tokens: u32 = crate::model_text_parts(&probe_talent.config)
            .iter()
            .map(|part| solstone_core_tokens::count(part))
            .sum();
        let schema_tokens = match probe_talent.config.get("json_schema") {
            Some(Value::String(s)) => solstone_core_tokens::count(s),
            Some(val) => solstone_core_tokens::count(&val.to_string()),
            None => 0,
        };
        let ordinary_request_tokens = ordinary_tokens + schema_tokens;
        let floor_budget = solstone_core_local::generate::compute_input_budget(
            1024,
            solstone_core_local::plan::LOCAL_MIN_CONTEXT_TOKENS,
        );
        assert_eq!(
            ordinary_request_tokens, floor_budget,
            "ordinary request tokens ({ordinary_request_tokens}) must equal floor budget ({floor_budget})"
        );

        let packet = freeze(talent, &context).expect("freeze must succeed");
        let (thawed, _) = thaw(&packet).unwrap();

        let raw_metadata = thawed
            .config
            .get("briefing_metadata")
            .and_then(Value::as_str)
            .expect("briefing_metadata must be present");
        let prompt_part = thawed
            .config
            .get("prompt")
            .and_then(Value::as_str)
            .expect("prompt must be present");
        assert!(
            prompt_part.contains(raw_metadata),
            "emitted prompt must contain exact briefing_metadata string"
        );

        let metadata: Value =
            serde_json::from_str(raw_metadata).expect("valid briefing_metadata JSON");
        let gaps = metadata["gaps"].as_array().expect("gaps array");
        assert!(
            gaps.iter()
                .any(|g| g.as_str() == Some("activities omitted: 1.")),
            "gaps must count the omitted activity: {gaps:?}"
        );
        assert!(
            gaps.iter().any(|g| g.as_str() == Some("the latest pulse")),
            "gaps must still include initial gap 'the latest pulse': {gaps:?}"
        );

        assert!(
            !prompt_part.contains(&description),
            "activity description must not be in the prompt"
        );

        let loc_text = thawed
            .config
            .get("_daily_location_text")
            .and_then(Value::as_str)
            .expect("_daily_location_text must be present");
        assert!(
            loc_text.lines().any(|l| l.starts_with("- ")),
            "location observation lines must be present in _daily_location_text: {loc_text}"
        );

        let model_parts = crate::model_text_parts(&thawed.config);
        assert!(
            model_parts
                .iter()
                .any(|p| p.lines().any(|l| l.starts_with("- "))),
            "location observation lines must be present in model_text_parts"
        );

        assert!(
            thawed.config.get("_daily_location_omitted").is_none(),
            "_daily_location_omitted must be absent"
        );

        let total_text_tokens: u32 = model_parts
            .iter()
            .map(|part| solstone_core_tokens::count(part))
            .sum();
        let final_schema_tokens = match thawed.config.get("json_schema") {
            Some(Value::String(s)) => solstone_core_tokens::count(s),
            Some(val) => solstone_core_tokens::count(&val.to_string()),
            None => 0,
        };
        assert!(
            total_text_tokens + final_schema_tokens <= floor_budget,
            "measured request tokens {} must be <= floor budget {}",
            total_text_tokens + final_schema_tokens,
            floor_budget
        );

        let preamble = metadata["coverage_preamble"].as_str().unwrap();
        assert!(!preamble.contains(".."), "{preamble}");
        assert!(!preamble.to_lowercase().contains("complete"), "{preamble}");
        for part in &model_parts {
            assert!(!part.to_lowercase().contains("complete"));
        }
    }

    #[test]
    fn morning_briefing_keeps_each_facet_briefing_inclusion_when_schedule_ids_collide() {
        let root = tempfile::tempdir().unwrap();
        location_journal(root.path(), LOCATION_DAY);

        solstone_core_facets::create_facet(root.path(), "a", "A", "", "", "", None).unwrap();
        solstone_core_facets::create_facet(root.path(), "b", "B", "", "", "", None).unwrap();

        let desc_a = " a".repeat(14349);
        let story_s = " s".repeat(493);
        let story_t = " t".repeat(493);

        let record_a = json!({
            "id": "anticipated_meeting_090000_1001",
            "title": "A",
            "start": "09:00:00",
            "description": desc_a,
            "story": {
                "body": story_s,
            },
        });
        write(
            root.path(),
            &format!("facets/a/activities/{LOCATION_DAY}.jsonl"),
            &format!("{record_a}\n"),
        );

        let record_b = json!({
            "id": "anticipated_meeting_090000_1001",
            "title": "B",
            "start": "09:00:00",
            "story": {
                "body": story_t,
            },
        });
        write(
            root.path(),
            &format!("facets/b/activities/{LOCATION_DAY}.jsonl"),
            &format!("{record_b}\n"),
        );

        let context = ExecutionContext {
            journal: root.path().to_owned(),
        };
        let talent = prepared(
            "morning_briefing",
            LOCATION_DAY,
            json!({
                "hook": {"pre": "morning_briefing"},
                "prompt": "Prompt with $activity_digest and $briefing_metadata",
                "json_schema": {"type": "object"},
                "max_output_tokens": 1024,
            }),
        );

        let stage = contract::resolve_hook("morning_briefing").unwrap();
        let mut probe_talent = talent.clone();
        let state = (stage.build.unwrap())(
            &mut probe_talent,
            &context,
            &solstone_core_generate::Cancellation::never(),
        )
        .unwrap();

        let PrePostState::MorningBriefing(ref briefing_state) = state else {
            panic!("expected MorningBriefing pre-state");
        };

        let mut unclipped_config = probe_talent.config.clone();
        crate::apply_template_vars(&mut unclipped_config, &briefing_state.values);

        let unclipped_text_tokens: u32 = crate::model_text_parts(&unclipped_config)
            .iter()
            .map(|part| solstone_core_tokens::count(part))
            .sum();
        let unclipped_schema_tokens = match unclipped_config.get("json_schema") {
            Some(Value::String(s)) => solstone_core_tokens::count(s),
            Some(val) => solstone_core_tokens::count(&val.to_string()),
            None => 0,
        };
        let unclipped_total = unclipped_text_tokens + unclipped_schema_tokens;
        assert_eq!(
            unclipped_total, 15585,
            "unclipped model_text_parts + schema tokens ({unclipped_total}) must be 15585"
        );

        (stage.prompt_override.unwrap())(&mut probe_talent, &state).unwrap();

        let ordinary_tokens: u32 = crate::ordinary_text_parts(&probe_talent.config)
            .iter()
            .map(|part| solstone_core_tokens::count(part))
            .sum();
        let probe_schema_tokens = match probe_talent.config.get("json_schema") {
            Some(Value::String(s)) => solstone_core_tokens::count(s),
            Some(val) => solstone_core_tokens::count(&val.to_string()),
            None => 0,
        };
        let ordinary_total = ordinary_tokens + probe_schema_tokens;
        let floor_budget = solstone_core_local::generate::compute_input_budget(
            1024,
            solstone_core_local::plan::LOCAL_MIN_CONTEXT_TOKENS,
        );
        assert_eq!(
            floor_budget, 15104,
            "floor budget for 1024 ({floor_budget}) must be 15104"
        );
        assert_eq!(
            ordinary_total, 15104,
            "ordinary tokens plus schema ({ordinary_total}) must equal 15104"
        );
        assert_eq!(
            ordinary_total, floor_budget,
            "ordinary tokens plus schema ({ordinary_total}) must equal computed floor ({floor_budget})"
        );

        let ordinary_parts = crate::ordinary_text_parts(&probe_talent.config);
        let ordinary_text = ordinary_parts.join("\n");
        assert!(
            ordinary_text.contains(&desc_a),
            "emitted ordinary text must contain description A*43027"
        );
        assert!(
            ordinary_text.contains(&story_s),
            "emitted ordinary text must contain story S*1500"
        );
        assert!(
            ordinary_text.contains("### B"),
            "emitted ordinary text must contain title B"
        );
        assert!(
            !ordinary_text.contains(&story_t),
            "emitted ordinary text must not contain story T*1500"
        );

        let pos_a = ordinary_text.find("### A").expect("### A must be present");
        let pos_b = ordinary_text.find("### B").expect("### B must be present");
        assert!(
            pos_a < pos_b,
            "### A must appear before ### B in emitted ordinary text"
        );

        let inclusion = probe_talent
            .config
            .get("_briefing_inclusion")
            .and_then(Value::as_array)
            .expect("_briefing_inclusion must be present on probe talent");
        assert_eq!(inclusion.len(), 2, "_briefing_inclusion must have 2 slots");
        assert_eq!(
            inclusion[0],
            json!({
                "facet": "a",
                "day": LOCATION_DAY,
                "id": "anticipated_meeting_090000_1001",
                "activity": true,
                "story": true,
            })
        );
        assert_eq!(
            inclusion[1],
            json!({
                "facet": "b",
                "day": LOCATION_DAY,
                "id": "anticipated_meeting_090000_1001",
                "activity": true,
                "story": false,
            })
        );

        let packet = freeze(talent, &context).expect("freeze must succeed");
        let (thawed, _) = thaw(&packet).unwrap();

        let thawed_ordinary = crate::ordinary_text_parts(&thawed.config).join("\n");
        assert!(
            thawed_ordinary.contains(&desc_a),
            "description A*43027 must remain after thaw"
        );
        assert!(
            thawed_ordinary.contains("### A"),
            "title A must remain after thaw"
        );
        assert!(
            thawed_ordinary.contains("### B"),
            "title B must remain after thaw"
        );

        let thaw_pos_a = thawed_ordinary.find("### A").unwrap();
        let thaw_pos_b = thawed_ordinary.find("### B").unwrap();
        assert!(
            thaw_pos_a < thaw_pos_b,
            "### A must appear before ### B after thaw"
        );

        assert!(
            !thawed_ordinary.contains(&story_s),
            "story S*1500 must be absent after thaw"
        );
        assert!(
            !thawed_ordinary.contains(&story_t),
            "story T*1500 must be absent after thaw"
        );

        let raw_metadata = thawed
            .config
            .get("briefing_metadata")
            .and_then(Value::as_str)
            .expect("briefing_metadata must be present");
        let metadata: Value =
            serde_json::from_str(raw_metadata).expect("valid briefing_metadata JSON");
        let gaps = metadata["gaps"].as_array().expect("gaps array");

        assert!(
            gaps.iter().any(|g| g
                .as_str()
                .is_some_and(|g| g.starts_with("activity stories omitted: "))),
            "gaps must count the omitted story: {gaps:?}"
        );
        assert!(
            gaps.iter().any(|g| g.as_str() == Some("the latest pulse")),
            "gaps must include 'the latest pulse': {gaps:?}"
        );
        assert!(
            !gaps
                .iter()
                .any(|g| g.as_str() == Some("activity anticipated_meeting_090000_1001")),
            "gaps must not include 'activity anticipated_meeting_090000_1001': {gaps:?}"
        );

        let loc_text = thawed
            .config
            .get("_daily_location_text")
            .and_then(Value::as_str)
            .expect("_daily_location_text must be present");
        assert!(
            loc_text.lines().any(|l| l.starts_with("- ")),
            "location observation lines must be present in _daily_location_text: {loc_text}"
        );

        let model_parts = crate::model_text_parts(&thawed.config);
        assert!(
            model_parts
                .iter()
                .any(|p| p.lines().any(|l| l.starts_with("- "))),
            "location observation lines must be present in model_text_parts"
        );

        assert!(
            thawed.config.get("_daily_location_omitted").is_none(),
            "_daily_location_omitted must be absent"
        );
        assert!(
            thawed.config.get("_briefing_inclusion").is_none(),
            "_briefing_inclusion must be absent"
        );

        let final_text_tokens: u32 = model_parts
            .iter()
            .map(|part| solstone_core_tokens::count(part))
            .sum();
        let final_schema_tokens = match thawed.config.get("json_schema") {
            Some(Value::String(s)) => solstone_core_tokens::count(s),
            Some(val) => solstone_core_tokens::count(&val.to_string()),
            None => 0,
        };
        assert!(
            final_text_tokens + final_schema_tokens <= floor_budget,
            "measured request tokens ({}) must be <= floor budget ({floor_budget})",
            final_text_tokens + final_schema_tokens
        );
    }

    #[test]
    fn morning_briefing_freeze_succeeds_without_prompt() {
        let root = tempfile::tempdir().unwrap();
        location_journal(root.path(), LOCATION_DAY);

        let cfg_path = root.path().join("config/journal.json");
        let mut cfg: Value =
            serde_json::from_str(&std::fs::read_to_string(&cfg_path).unwrap()).unwrap();
        cfg["providers"] = json!({
            "active": {
                "provider": "test",
                "model": "test-model"
            }
        });
        std::fs::write(&cfg_path, serde_json::to_string(&cfg).unwrap()).unwrap();

        solstone_core_facets::create_facet(root.path(), "fictional", "Fictional", "", "", "", None)
            .unwrap();

        let talent_root = root.path().join("talent");
        let apps_root = root.path().join("apps");
        let templates_dir = root.path().join("templates");
        for p in [&talent_root, &apps_root, &templates_dir] {
            std::fs::create_dir_all(p).unwrap();
        }
        let src_talent =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../payload/solstone/talent");
        std::fs::copy(
            src_talent.join("morning_briefing.md"),
            talent_root.join("morning_briefing.md"),
        )
        .unwrap();
        std::fs::copy(
            src_talent.join("morning_briefing.schema.json"),
            talent_root.join("morning_briefing.schema.json"),
        )
        .unwrap();

        let paths = crate::prepare::RuntimePaths {
            talent_root,
            apps_root,
            templates_dir,
        };
        let context = ExecutionContext {
            journal: root.path().to_owned(),
        };

        let request = json!({
            "name": "morning_briefing",
            "day": LOCATION_DAY,
        })
        .as_object()
        .unwrap()
        .clone();
        let prepared_talent = crate::prepare::prepare(
            request,
            &paths,
            &context,
            crate::prepare::PrepareMode::Execute,
        )
        .expect("prepare must succeed");

        assert!(
            prepared_talent.config.get("prompt").is_none(),
            "prompt must be absent before freeze"
        );
        let pre_ui = prepared_talent
            .config
            .get("user_instruction")
            .and_then(Value::as_str)
            .expect("user_instruction must be present");
        assert!(
            pre_ui.contains("$briefing_metadata"),
            "user_instruction must contain literal $briefing_metadata before freeze"
        );

        let packet = freeze(prepared_talent, &context).expect("freeze must succeed");
        let (thawed, _) = thaw(&packet).unwrap();

        let raw_metadata = thawed
            .config
            .get("briefing_metadata")
            .and_then(Value::as_str)
            .expect("briefing_metadata must be present on config");
        let post_ui = thawed
            .config
            .get("user_instruction")
            .and_then(Value::as_str)
            .expect("user_instruction must be present");
        assert!(
            post_ui.contains(raw_metadata),
            "user_instruction must contain exact briefing_metadata string"
        );

        let model_parts = crate::model_text_parts(&thawed.config);
        assert!(
            model_parts.iter().any(|part| part.contains(raw_metadata)),
            "model_text_parts must contain exact briefing_metadata string"
        );

        assert!(
            thawed.config.get("prompt").is_none(),
            "prompt must still be absent after freeze"
        );

        let metadata: Value =
            serde_json::from_str(raw_metadata).expect("valid briefing_metadata JSON");
        let gaps = metadata["gaps"].as_array().expect("gaps array");
        assert!(
            gaps.iter().any(|g| g.as_str() == Some("the latest pulse")),
            "gaps must include initial gap 'the latest pulse': {gaps:?}"
        );

        let max_output_tokens = thawed
            .config
            .get("max_output_tokens")
            .and_then(Value::as_u64)
            .and_then(|t| u32::try_from(t).ok())
            .expect("max_output_tokens must be set on prepared talent");
        let floor_budget = solstone_core_local::generate::compute_input_budget(
            max_output_tokens,
            solstone_core_local::plan::LOCAL_MIN_CONTEXT_TOKENS,
        );
        let total_text_tokens: u32 = model_parts
            .iter()
            .map(|part| solstone_core_tokens::count(part))
            .sum();
        let schema_tokens = match thawed.config.get("json_schema") {
            Some(Value::String(s)) => solstone_core_tokens::count(s),
            Some(val) => solstone_core_tokens::count(&val.to_string()),
            None => 0,
        };
        assert!(
            total_text_tokens + schema_tokens <= floor_budget,
            "measured request tokens {} must be <= floor budget {}",
            total_text_tokens + schema_tokens,
            floor_budget
        );

        let loc_text = thawed
            .config
            .get("_daily_location_text")
            .and_then(Value::as_str)
            .expect("_daily_location_text must be present");
        assert!(
            loc_text.lines().any(|l| l.starts_with("- ")),
            "location observation lines must be present in _daily_location_text: {loc_text}"
        );
        assert!(
            model_parts
                .iter()
                .any(|p| p.lines().any(|l| l.starts_with("- "))),
            "location observation lines must be present in model_text_parts"
        );
    }

    #[test]
    fn clip_location_part_drops_whole_observations_under_the_byte_cap() {
        let rendered = "# location\n\n- one\n- two\n- three\n";
        let (full, omitted) = clip_location_part("Label.", rendered, 10_000);
        assert_eq!(omitted, 0);
        let full = full.expect("all three lines fit");
        assert!(full.contains("- one\n") && full.contains("- three\n"));
        let (shrunk, omitted) = clip_location_part("Label.", rendered, full.len() - 1);
        assert_eq!(omitted, 1);
        let shrunk = shrunk.expect("two lines fit");
        assert!(shrunk.contains("- two\n"));
        assert!(!shrunk.contains("- three\n"));
        let (none, count) = clip_location_part("Label.", rendered, 1);
        assert!(none.is_none());
        assert_eq!(count, 3);
        let (none, count) = clip_location_part("Label.", "# heading only\n", 10_000);
        assert!(none.is_none());
        assert_eq!(count, 0);
    }

    #[test]
    fn location_is_sized_the_same_whatever_the_local_engine_launched() {
        let root = tempfile::tempdir().unwrap();
        location_journal(root.path(), LOCATION_DAY);
        let observations = rendered_observations(root.path());
        assert!(!observations.is_empty());
        let context = ExecutionContext {
            journal: root.path().to_owned(),
        };
        let freeze_with = |prompt: &str| {
            freeze(
                prepared(
                    "entities:entity_observer",
                    LOCATION_DAY,
                    json!({"facet":"work","hook":{"pre":"entities:entity_observer","post":"entities:entity_observer"},"skip_reason":"no_input","prompt":prompt}),
                ),
                &context,
            )
            .unwrap()
        };

        // Whatever window the local engine was launched with, the request is the same.
        let unlaunched = freeze_with("keep me");
        for launched in [b"300".as_slice(), b"16384", b"262144"] {
            write_bytes(root.path(), "health/local.ctx", launched);
            assert_eq!(freeze_with("keep me"), unlaunched);
        }
        let config = unlaunched["prepared"]["config"].as_object().unwrap();
        assert!(config.get("_daily_location_omitted").is_none());
        let parts = text_parts(&thaw(&unlaunched).unwrap().0);
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0], "keep me");
        for observation in &observations {
            assert!(parts[1].contains(observation.trim_end()));
        }

        // Input that already fills the floor budget leaves no room for location.
        let large = " a".repeat(solstone_core_local::plan::LOCAL_MIN_CONTEXT_TOKENS as usize);
        let packet = freeze_with(&large);
        let config = packet["prepared"]["config"].as_object().unwrap();
        assert_eq!(
            config.get("prompt").and_then(Value::as_str),
            Some(large.as_str())
        );
        assert!(config.get("_daily_location_fingerprint").is_some());
        assert!(config.get("_daily_location_text").is_none());
        assert_eq!(
            config
                .get("_daily_location_omitted")
                .and_then(Value::as_u64),
            Some(u64::try_from(observations.len()).unwrap())
        );
        let parts = text_parts(&thaw(&packet).unwrap().0);
        assert_eq!(parts, vec![large]);
        for coordinate in ["39.75", "105.2", "105.0"] {
            assert!(!parts[0].contains(coordinate));
        }
    }
    #[cfg(feature = "full-tests")]
    #[test]
    fn capture_excludes_a_supported_place_name_aba() {
        let root = tempfile::tempdir().unwrap();
        location_journal(root.path(), LOCATION_DAY);
        let context = ExecutionContext {
            journal: root.path().to_owned(),
        };
        let before =
            solstone_core_location::daily_location_fingerprint(root.path(), LOCATION_DAY).unwrap();
        let journal = root.path().to_path_buf();
        let worker = std::rc::Rc::new(std::cell::RefCell::new(None));
        let saved_worker = worker.clone();
        CAPTURE_BARRIER.with(|slot| {
            *slot.borrow_mut() = Some(Box::new(move || {
                let (sent, received) = std::sync::mpsc::channel();
                let handle = std::thread::spawn(move || {
                    let original =
                        solstone_core_location::places::PlaceStore::update(&journal, |store| {
                            store.places[0].name.replace("Transient name".into())
                        })
                        .unwrap();
                    solstone_core_location::places::PlaceStore::update(&journal, |store| {
                        store.places[0].name = original;
                    })
                    .unwrap();
                    // The probe receiver ends before capture releases admission.
                    let _ = sent.send(());
                });
                *saved_worker.borrow_mut() = Some(handle);
                assert!(
                    matches!(
                        received.recv_timeout(std::time::Duration::from_millis(100)),
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout)
                    ),
                    "supported ABA passed through capture"
                );
            }))
        });
        let mut talent = prepared("entities:entity_suggest", LOCATION_DAY, json!({}));
        attach_daily_location(
            &mut talent,
            &mut FrozenState::None,
            &context,
            "entities:entity_suggest",
        )
        .unwrap();
        worker.borrow_mut().take().unwrap().join().unwrap();
        assert_eq!(
            solstone_core_location::daily_location_fingerprint(root.path(), LOCATION_DAY).unwrap(),
            before
        );
        assert_eq!(talent.config["_daily_location_fingerprint"], before);
        assert!(
            !talent.config["_daily_location_text"]
                .as_str()
                .unwrap()
                .contains("Transient name")
        );
    }
    #[test]
    fn dense_location_lines_are_clipped_to_their_counted_room() {
        // Every digit is its own token, so these lines run under 3 bytes a token.
        let rendered: String = std::iter::once("# location\n\n".to_owned())
            .chain((0..200).map(|i| {
                format!("- a visit your phone reported at Home: arrived 07:{i:02}, left 18:03.\n")
            }))
            .collect();
        let label = location_label("entities:entity_suggest", LOCATION_DAY);
        for room in [200, 600, 1_500] {
            let (part, omitted) = clip_location_to_room(&label, &rendered, room);
            let part = part.expect("some lines fit");
            assert!(solstone_core_tokens::count(&part) <= room, "room {room}");
            let kept = 200 - omitted as usize;
            let (one_more, _) = clip_location_part(&label, &rendered, part.len() + 80);
            assert!(
                solstone_core_tokens::count(&one_more.unwrap()) > room,
                "room {room} keeps {kept} lines and could keep one more"
            );
        }
    }

    #[test]
    fn a_large_whole_first_observation_that_fits_is_not_discarded() {
        let talent = prepared("entities:entity_suggest", LOCATION_DAY, json!({}));
        let rendered = format!("# location\n\n- {}\n", "長".repeat(3500));
        let (part, omitted) = fit_location_part(
            "entities:entity_suggest",
            LOCATION_DAY,
            &rendered,
            &["ordinary".into()],
            &talent,
            &FrozenState::None,
        )
        .expect("fit");
        let part = part.expect("a complete observation fits the 16 KiB and request budgets");
        assert!(part.len() <= LOCATION_PART_BYTE_CAP);
        assert!(part.ends_with(&format!("- {}\n", "長".repeat(3500))));
        assert_eq!(omitted, 0);
    }
}
