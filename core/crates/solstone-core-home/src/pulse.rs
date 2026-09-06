// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Router-free assembly for the home pulse and briefing payloads.

use std::collections::BTreeSet;

use chrono::{DateTime, FixedOffset, Utc};
use serde_json::{Map, Value, json};
use solstone_core_entity::read_journal_principal;
use solstone_core_facets::{ConnectionsHorizon, refresh_connections_horizon};

use crate::HomeContext;
use crate::briefing;
use crate::connections::build_connections_card;
use crate::formatting::{
    format_activity_label, format_duration, format_gap_links, format_heatmap_summary,
    format_newsletter_summary, format_processing_summary, relative_time,
};
use crate::health_glance::build_health_glance;
use crate::needs_you::{classify_needs_you, needs_dedup_key};
use crate::readers::{
    briefing_freshness, briefing_lateness_state, briefing_needs_items, collect_activities,
    collect_anticipated_activities, collect_top_activities_yesterday, compute_briefing_phase,
    count_journal_age_days, get_capture_health, last_observe_relative_seconds, load_awareness,
    load_backlog_source, load_briefing, load_connections_network, load_flow_md,
    load_latest_weekly_reflection, load_pulse_narrative, load_stats, load_yesterday_stats,
    newsletter_attempts_from_think_logs, overnight_window_passed, read_steward_health,
    read_steward_summary, render_briefing_sections, resolve_attention, summarize_pipeline_day,
};

const FIRST_WEEK_FRAMING: &str = "most of what your journal keeps becomes useful after about a week, once your journal has enough of your days in it to show patterns. for now, here's what's already happening:";

/// Complete pre-route pulse context. The instant stays typed until projection.
///
/// `now` is the wall clock the surface reads — the instant in the journal's
/// local day coordinate. Its only consumer compares it against local activity
/// end times, so a UTC wall clock would misjudge which events are past.
struct PulseContext {
    fields: Map<String, Value>,
    now: DateTime<FixedOffset>,
}

impl PulseContext {
    fn into_pulse_payload(mut self) -> Value {
        self.fields.remove("show_welcome");
        self.fields
            .insert("now".to_owned(), format_now(self.now).into());
        if let Some(attention) = self.fields.get("attention").and_then(Value::as_object) {
            self.fields.insert(
                "attention".to_owned(),
                json!({
                    "placeholder_text": attention.get("placeholder_text").cloned().unwrap_or(Value::Null),
                    "context_lines": attention.get("context_lines").cloned().unwrap_or_else(|| json!([])),
                }),
            );
        }
        Value::Object(self.fields)
    }
}

/// Assemble all fields used by the Python home context in its build order.
fn build_pulse_context(context: &HomeContext) -> PulseContext {
    let today = context.today();
    let journal_age_days = count_journal_age_days(context);
    let capture_health = get_capture_health(context);
    let awareness = load_awareness(context);
    let attention = resolve_attention(context, &awareness).unwrap_or(Value::Null);
    let stats_data = load_stats(context, &today);
    let stats = stats_data
        .get("stats")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let segment_count = stats
        .get("transcript_segments")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let facet_data = stats_data
        .get("facet_data")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let flow = load_flow_md(context, &today);
    let flow_updated_at = flow.updated_at.and_then(|timestamp| {
        DateTime::from_timestamp(timestamp as i64, 0)
            .map(|time| time.with_timezone(&Utc).to_rfc3339())
    });
    let narrative = load_pulse_narrative(context, &today);
    let (narrative_content, narrative_updated_at, narrative_source, narrative_header, pulse_needs) =
        if let Some(content) = narrative.content {
            (
                Some(content),
                narrative.updated_at.or_else(|| flow_updated_at.clone()),
                "pulse",
                "pulse",
                narrative.needs.into_iter().map(Value::String).collect(),
            )
        } else {
            (
                flow.content.clone(),
                flow_updated_at.clone(),
                "flow",
                "today's flow",
                Vec::new(),
            )
        };
    let anticipated_activities = collect_anticipated_activities(context, &today);
    let activities = collect_activities(context, &today);
    let latest_weekly_reflection = load_latest_weekly_reflection(context);
    let last_observe_relative = last_observe_relative_seconds(context)
        .map(|seconds| format!("{} ago", relative_time(seconds as f64)));

    let briefing = load_briefing(context, &today);
    let briefing_document = briefing.clone().unwrap_or(Value::Null);
    let briefing_sections = Value::Object(
        render_briefing_sections(&briefing_document)
            .into_iter()
            .map(|(key, value)| (key, Value::String(value)))
            .collect(),
    );
    let briefing_meta = briefing_document
        .get("metadata")
        .cloned()
        .unwrap_or(Value::Null);
    let briefing_needs = briefing_needs_items(&briefing_document);
    let briefing_exists = !briefing_sections.as_object().is_none_or(Map::is_empty);
    let briefing_phase =
        compute_briefing_phase(segment_count, context.local_hour(), briefing_exists);
    let briefing_lateness = briefing_lateness_state(context.now_local(), briefing_phase);
    let show_welcome = narrative_content.is_none()
        && anticipated_activities.is_empty()
        && activities.is_empty()
        && !briefing_exists
        && attention.is_null()
        && pulse_needs.is_empty()
        && latest_weekly_reflection.is_none();

    let mut needs_keys = BTreeSet::new();
    if !attention.is_null() {
        needs_keys.insert(needs_dedup_key(&attention));
    }
    let mut deduped_pulse_needs = Vec::new();
    for need in &pulse_needs {
        let key = needs_dedup_key(need);
        if needs_keys.insert(key) {
            deduped_pulse_needs.push(need.clone());
        }
    }
    let needs_you_items = classify_needs_you(&attention, &deduped_pulse_needs);
    let needs_count = needs_you_items.len();
    let needs_summary = if needs_count == 0 {
        String::new()
    } else {
        format!(
            "{needs_count} item{} need{} attention",
            if needs_count == 1 { "" } else { "s" },
            if needs_count == 1 { "s" } else { "" },
        )
    };
    let mut briefing_needs_deduped = Vec::new();
    let mut briefing_needs_shared_count = 0;
    let mut seen_keys = needs_keys.clone();
    for item in briefing_needs {
        let key = needs_dedup_key(&item);
        if needs_keys.contains(&key) {
            briefing_needs_shared_count += 1;
        } else if seen_keys.insert(key) {
            briefing_needs_deduped.push(item);
        }
    }
    let briefing_needs_badge = (briefing_needs_shared_count > 0).then(|| {
        format!(
            "{briefing_needs_shared_count} item{} also in Pulse needs",
            if briefing_needs_shared_count == 1 {
                ""
            } else {
                "s"
            }
        )
    });
    let briefing_summary = (briefing_phase == "active").then(|| {
        briefing::summary(
            briefing.as_ref(),
            &briefing_sections,
            briefing_needs_deduped.len() as i64,
        )
    });

    let mut pipeline_status = read_steward_health(context).unwrap_or(Value::Null);
    if let (Some(pipeline), Some(summary)) = (
        pipeline_status.as_object_mut(),
        read_steward_summary(context, None),
    ) && let Some(summary) = summary.as_object()
    {
        pipeline.extend(summary.clone());
    }
    let brain = crate::readers::build_brain_snapshot(context);
    let backlog = load_backlog_source(context);
    let health_glance = build_health_glance(
        &capture_health,
        &pipeline_status,
        last_observe_relative.as_deref(),
        &backlog,
        &brain,
        context.now_utc,
    );
    let yesterday_processing = summarize_yesterday_processing(context, journal_age_days);
    let horizon = refresh_connections_horizon(context.journal_root());
    let connections = load_connections_card(context, horizon);

    let narrative_summary = narrative_content.as_ref().map_or_else(String::new, |_| {
        narrative_updated_at.as_ref().map_or_else(
            || narrative_header.to_owned(),
            |updated| format!("{narrative_header} — updated {updated}"),
        )
    });
    let mut today_parts = Vec::new();
    if !anticipated_activities.is_empty() {
        let count = anticipated_activities.len();
        today_parts.push(format!(
            "{count} anticipated activit{}",
            if count == 1 { "y" } else { "ies" }
        ));
    }
    if !activities.is_empty() {
        let count = activities.len();
        today_parts.push(format!(
            "{count} {}",
            if count == 1 { "activity" } else { "activities" }
        ));
    }

    let mut fields = Map::new();
    fields.insert("today".to_owned(), today.into());
    fields.insert("now".to_owned(), Value::Null);
    fields.insert("health_glance".to_owned(), health_glance);
    fields.insert("capture_health".to_owned(), capture_health);
    fields.insert("attention".to_owned(), attention);
    fields.insert("pipeline_status".to_owned(), pipeline_status);
    fields.insert("segment_count".to_owned(), segment_count.into());
    fields.insert("facet_data".to_owned(), facet_data);
    fields.insert("narrative_content".to_owned(), narrative_content.into());
    fields.insert(
        "narrative_updated_at".to_owned(),
        narrative_updated_at.into(),
    );
    fields.insert("narrative_source".to_owned(), narrative_source.into());
    fields.insert("narrative_header".to_owned(), narrative_header.into());
    fields.insert("pulse_needs".to_owned(), Value::Array(pulse_needs));
    fields.insert("flow_content".to_owned(), flow.content.into());
    fields.insert("flow_updated_at".to_owned(), flow_updated_at.into());
    fields.insert(
        "anticipated_activities".to_owned(),
        Value::Array(anticipated_activities),
    );
    fields.insert("activities".to_owned(), Value::Array(activities));
    fields.insert("needs_you_items".to_owned(), Value::Array(needs_you_items));
    fields.insert("briefing_sections".to_owned(), briefing_sections);
    fields.insert("briefing_meta".to_owned(), briefing_meta);
    fields.insert("briefing_phase".to_owned(), briefing_phase.into());
    fields.insert("briefing_lateness".to_owned(), briefing_lateness);
    fields.insert("briefing_exists".to_owned(), briefing_exists.into());
    fields.insert("briefing_summary".to_owned(), briefing_summary.into());
    fields.insert(
        "briefing_needs_deduped".to_owned(),
        Value::Array(
            briefing_needs_deduped
                .into_iter()
                .filter_map(|item| item.get("text").and_then(Value::as_str).map(str::to_owned))
                .map(Value::String)
                .collect(),
        ),
    );
    fields.insert(
        "briefing_needs_shared_count".to_owned(),
        briefing_needs_shared_count.into(),
    );
    fields.insert(
        "briefing_needs_badge".to_owned(),
        briefing_needs_badge.into(),
    );
    fields.insert(
        "latest_weekly_reflection".to_owned(),
        latest_weekly_reflection.into(),
    );
    fields.insert(
        "yesterday_processing".to_owned(),
        yesterday_processing.into(),
    );
    fields.insert("connections".to_owned(), connections);
    fields.insert("show_welcome".to_owned(), show_welcome.into());
    fields.insert("journal_age_days".to_owned(), journal_age_days.into());
    fields.insert(
        "home_state".to_owned(),
        if show_welcome { "welcome" } else { "active" }.into(),
    );
    fields.insert(
        "welcome_framing".to_owned(),
        if show_welcome && journal_age_days <= 7 {
            Value::String(FIRST_WEEK_FRAMING.to_owned())
        } else {
            Value::Null
        },
    );
    fields.insert("narrative_summary".to_owned(), narrative_summary.into());
    fields.insert("today_summary".to_owned(), today_parts.join(", ").into());
    fields.insert("needs_summary".to_owned(), needs_summary.into());
    debug_assert_eq!(fields.len(), 37);
    PulseContext {
        fields,
        now: context.now_local(),
    }
}

/// Project a full context to the public pulse API shape.
pub fn pulse_payload(context: &HomeContext) -> Value {
    build_pulse_context(context).into_pulse_payload()
}

/// Project the briefing fields from a freshly assembled context for this request.
pub fn briefing_payload(context: &HomeContext) -> Value {
    let payload = pulse_payload(context);
    let field = |name: &str| payload.get(name).cloned().unwrap_or(Value::Null);
    json!({
        "exists": field("briefing_exists"),
        "phase": field("briefing_phase"),
        "summary": field("briefing_summary"),
        "meta": field("briefing_meta"),
        "sections": field("briefing_sections"),
        "needs_deduped": field("briefing_needs_deduped"),
        "needs_shared_count": field("briefing_needs_shared_count"),
        "needs_badge": field("briefing_needs_badge"),
    })
}

fn format_now(now: DateTime<FixedOffset>) -> String {
    now.format("%Y-%m-%dT%H:%M:%S%.6f").to_string()
}

fn load_connections_card(context: &HomeContext, horizon: Option<ConnectionsHorizon>) -> Value {
    let principal = read_journal_principal(context.journal_root()).map_err(|_| ());
    let network = match principal.as_ref() {
        Err(_) | Ok(None) => Ok(json!({})),
        Ok(Some(principal))
            if principal
                .get("id")
                .and_then(Value::as_str)
                .is_none_or(str::is_empty) =>
        {
            Ok(json!({}))
        }
        Ok(Some(principal)) => load_connections_network(context, principal)
            .map_err(|_| ())
            .and_then(|network| {
                network
                    .map(|network| serde_json::to_value(network).map_err(|_| ()))
                    .transpose()
            })
            .map(|network| network.unwrap_or_else(|| json!({}))),
    };
    build_connections_card(principal, network, horizon)
}

fn summarize_yesterday_processing(context: &HomeContext, journal_age_days: i64) -> Option<Value> {
    let stats_data = load_yesterday_stats(context)?;
    if journal_age_days == 0 {
        return None;
    }
    let stats = stats_data
        .get("stats")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let transcript_seconds = stats
        .get("transcript_duration")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    let transcript_segments = stats
        .get("transcript_segments")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let has_facet_activity = stats_data
        .get("facet_data")
        .and_then(Value::as_object)
        .is_some_and(|facets| {
            facets.values().any(|facet| {
                facet.get("minutes").and_then(Value::as_f64).unwrap_or(0.0) > 0.0
                    || facet.get("count").and_then(Value::as_i64).unwrap_or(0) > 0
            })
        });
    let activities = collect_top_activities_yesterday(context);
    if transcript_seconds <= 0.0
        && transcript_segments <= 0
        && !has_facet_activity
        && activities.is_empty()
    {
        return None;
    }
    let yesterday = context.yesterday();
    let today = context.today();
    let pipeline = summarize_pipeline_day(context, &yesterday);
    let briefing = briefing_freshness(context, &today);
    let briefing_valid = briefing
        .get("valid")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let overnight_passed = overnight_window_passed(context.local_hour());
    let (successful, attempted) = newsletter_attempts_from_think_logs(context, &yesterday);
    let successful = successful as i64;
    let attempted = attempted as i64;
    let sparse = (transcript_seconds > 0.0 || transcript_segments > 0)
        && !has_facet_activity
        && activities.is_empty();
    let mut reasons = Vec::new();
    if attempted > successful {
        reasons.push(Value::String("newsletter_partial".to_owned()));
    }
    if pipeline.get("status").and_then(Value::as_str) != Some("healthy") {
        reasons.push(Value::String("pipeline_warning".to_owned()));
    }
    // Before the overnight window has passed for the local day, a briefing that
    // is not there yet is pending, not missing.
    if !briefing_valid && overnight_passed {
        reasons.push(Value::String("briefing_missing".to_owned()));
    }
    let mode = if sparse {
        "sparse"
    } else if reasons.is_empty() {
        "healthy"
    } else {
        "degraded"
    };
    if mode == "sparse" {
        return Some(
            json!({"title":"Yesterday's processing","mode":"sparse","default_collapsed":false,"first_week_framing":null,"summary_line":format!("{} of audio went into your journal yesterday.", format_duration(transcript_seconds / 60.0)),"details":null,"gap_links":[],"sparse_lines":["no facet newsletters written.","there wasn't much else to process."],"status_reasons":reasons}),
        );
    }
    let mut details = vec![Value::String(format_newsletter_summary(
        successful, attempted,
    ))];
    if briefing_valid {
        details.push(Value::String(
            match briefing.get("generated_label").and_then(Value::as_str) {
                Some(label) => format!("your morning briefing was prepared at {label}."),
                None => "your morning briefing was prepared.".to_owned(),
            },
        ));
    }
    if let Some(summary) = format_heatmap_summary(&stats_data) {
        details.push(summary.into());
    }
    details.extend(
        activities
            .iter()
            .take(2)
            .map(|activity| Value::String(format_activity_label(activity))),
    );
    let failed_run_count = if mode == "degraded" {
        crate::formatting::count_failed_runs(&pipeline)
    } else {
        0
    };
    Some(json!({
        "title": if mode == "degraded" { "⚠ Yesterday's processing" } else { "Yesterday's processing" },
        "mode": mode,
        "default_collapsed": mode == "healthy" && journal_age_days >= 8,
        "first_week_framing": if journal_age_days <= 7 { Value::String(FIRST_WEEK_FRAMING.to_owned()) } else { Value::Null },
        "summary_line": format_processing_summary(mode, successful, attempted, briefing_valid),
        "details": details,
        "gap_links": if mode == "degraded" { Value::Array(format_gap_links(&pipeline, briefing_valid, &yesterday, &today, overnight_passed)) } else { Value::Array(Vec::new()) },
        "failed_run_count": failed_run_count,
        "sparse_lines": Value::Null,
        "status_reasons": reasons,
    }))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io::Write;
    use std::path::Path;

    use chrono::{FixedOffset, NaiveDate, TimeZone, Timelike};
    use solstone_core_journal_io::{
        JournalRoot,
        operational_log::{OplogFormat, create_oplog_at},
    };
    use tempfile::TempDir;

    use super::*;

    fn write_think_oplog(root: &Path, day: &str, run: &str, text: &str) {
        let day = NaiveDate::parse_from_str(day, "%Y%m%d").unwrap();
        let opened = FixedOffset::east_opt(0)
            .unwrap()
            .from_local_datetime(&day.and_hms_opt(12, 0, 0).unwrap())
            .single()
            .unwrap();
        let mut writer = create_oplog_at(
            JournalRoot::open(root).unwrap(),
            "think",
            run,
            OplogFormat::Jsonl,
            opened,
        )
        .unwrap();
        writer.write_all(text.as_bytes()).unwrap();
    }

    #[test]
    fn empty_payload_has_exact_public_key_set_and_naive_microsecond_now() {
        let root = TempDir::new().unwrap();
        let context = HomeContext::with_day_offset(
            root.path(),
            Utc.with_ymd_and_hms(2026, 8, 14, 22, 28, 35)
                .unwrap()
                .with_nanosecond(430_840_000)
                .unwrap(),
            utc_day(),
        );
        let payload = pulse_payload(&context);
        assert_eq!(payload.as_object().unwrap().len(), 36);
        assert_eq!(
            payload
                .as_object()
                .unwrap()
                .keys()
                .cloned()
                .collect::<BTreeSet<_>>(),
            [
                "today",
                "now",
                "health_glance",
                "capture_health",
                "attention",
                "pipeline_status",
                "segment_count",
                "facet_data",
                "narrative_content",
                "narrative_updated_at",
                "narrative_source",
                "narrative_header",
                "pulse_needs",
                "flow_content",
                "flow_updated_at",
                "anticipated_activities",
                "activities",
                "needs_you_items",
                "briefing_sections",
                "briefing_meta",
                "briefing_phase",
                "briefing_lateness",
                "briefing_exists",
                "briefing_summary",
                "briefing_needs_deduped",
                "briefing_needs_shared_count",
                "briefing_needs_badge",
                "latest_weekly_reflection",
                "yesterday_processing",
                "connections",
                "journal_age_days",
                "home_state",
                "welcome_framing",
                "narrative_summary",
                "today_summary",
                "needs_summary",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
        );
        assert_eq!(payload["now"], "2026-08-14T22:28:35.430840");
        assert!(payload.get("show_welcome").is_none());
        assert_eq!(
            payload["briefing_lateness"],
            json!({"late":false,"late_hours":0})
        );
    }

    #[test]
    fn empty_fixture_matches_the_captured_pulse_and_briefing() {
        let (_fixture, context) = fixture_context(
            "convey_home_empty_journal",
            Utc.with_ymd_and_hms(2026, 8, 14, 22, 28, 35)
                .unwrap()
                .with_nanosecond(430_840_000)
                .unwrap(),
        );
        let reference = reference_payload("reference-pulse-empty-journal.json");
        assert_payload_fields(&pulse_payload(&context), &reference["pulse"], &["now"]);
        assert_eq!(briefing_payload(&context), reference["briefing"]);
    }

    #[test]
    fn seeded_fixture_matches_the_captured_pulse_and_briefing() {
        let (_fixture, context) = fixture_context(
            "convey_home_seeded_journal",
            Utc.with_ymd_and_hms(2026, 8, 14, 23, 25, 13)
                .unwrap()
                .with_nanosecond(793_525_000)
                .unwrap(),
        );
        let payload = pulse_payload(&context);
        let reference = reference_payload("reference-pulse-seeded-journal.json");
        let mut expected_pulse = reference["pulse"].clone();
        assert_eq!(
            expected_pulse.pointer("/health_glance/cta/href"),
            Some(&json!("/app/network/"))
        );
        assert_eq!(
            payload.pointer("/health_glance/cta/href"),
            Some(&json!("/app/network/"))
        );
        assert_eq!(
            expected_pulse.pointer("/health_glance/verdict"),
            Some(&json!("ok"))
        );
        assert_eq!(
            payload.pointer("/health_glance/verdict"),
            Some(&json!("calm"))
        );
        *expected_pulse
            .pointer_mut("/health_glance/verdict")
            .unwrap() = json!("calm");
        assert_eq!(
            expected_pulse.pointer("/health_glance/severity"),
            Some(&json!("green"))
        );
        assert_eq!(
            payload.pointer("/health_glance/severity"),
            Some(&json!("neutral"))
        );
        *expected_pulse
            .pointer_mut("/health_glance/severity")
            .unwrap() = json!("neutral");
        assert_payload_fields(
            &payload,
            &expected_pulse,
            &[
                "now",
                "segment_count",
                "facet_data",
                "latest_weekly_reflection",
            ],
        );
        assert_eq!(briefing_payload(&context), reference["briefing"]);
        let mut expected_reflection = reference["pulse"]["latest_weekly_reflection"].clone();
        expected_reflection.as_object_mut().unwrap().remove("url");
        assert_eq!(payload["latest_weekly_reflection"], expected_reflection);
        let stats: Value = serde_json::from_str(include_str!(
            "../../../fixtures/convey_home_seeded_journal/chronicle/20260814/stats.json"
        ))
        .unwrap();
        assert_eq!(
            payload["segment_count"],
            stats["stats"]["transcript_segments"]
        );
        assert_eq!(payload["facet_data"], stats["facet_data"]);
        for pointer in [
            "/activities",
            "/anticipated_activities",
            "/needs_you_items",
            "/briefing_sections",
            "/pulse_needs",
            "/briefing_needs_deduped",
            "/today_summary",
            "/narrative_content",
            "/latest_weekly_reflection",
            "/yesterday_processing/details",
        ] {
            let value = payload.pointer(pointer).unwrap();
            assert!(
                !value.is_null()
                    && !value.as_array().is_some_and(Vec::is_empty)
                    && !value.as_object().is_some_and(Map::is_empty)
                    && value.as_str().is_none_or(|value| !value.is_empty()),
                "seeded payload field {pointer} must be non-empty"
            );
        }
        let reflection = payload["latest_weekly_reflection"].as_object().unwrap();
        assert_eq!(reflection.len(), 2);
        assert!(
            reflection
                .get("label")
                .and_then(Value::as_str)
                .is_some_and(|label| !label.is_empty())
        );
        assert!(!reflection.contains_key("url"));
    }

    #[test]
    fn empty_journal_phase_tracks_the_injected_hour() {
        let root = TempDir::new().unwrap();
        for hour in 0..24 {
            let context = utc_context(
                root.path(),
                Utc.with_ymd_and_hms(2026, 8, 14, hour, 0, 0).unwrap(),
            );
            assert_eq!(
                pulse_payload(&context)["briefing_lateness"],
                json!({"late":false,"late_hours":0}),
                "hour {hour}"
            );
        }
        for (hour, expected) in [(9, "pending"), (22, "eod")] {
            let context = utc_context(
                root.path(),
                Utc.with_ymd_and_hms(2026, 8, 14, hour, 0, 0).unwrap(),
            );
            assert_eq!(pulse_payload(&context)["briefing_phase"], expected);
        }
        let late = briefing::lateness_state(
            Utc.with_ymd_and_hms(2026, 8, 14, 13, 0, 0).unwrap(),
            "pending",
        );
        assert_eq!(late, json!({"late":true,"late_hours":3}));
    }

    fn mountain_day() -> FixedOffset {
        FixedOffset::west_opt(6 * 3600).expect("mountain daylight offset")
    }

    /// Two local days of stats: september 4th and the september 5th that is
    /// still running when the UTC clock has already rolled over to the 6th.
    fn september_journal() -> TempDir {
        let root = TempDir::new().unwrap();
        for (day, stats) in [
            (
                "20260904",
                r#"{"stats":{"transcript_duration":1800,"transcript_segments":6},"facet_data":{"solstone":{"minutes":30,"count":2}}}"#,
            ),
            (
                "20260905",
                r#"{"stats":{"transcript_duration":3600,"transcript_segments":12},"facet_data":{"solstone":{"minutes":45,"count":3}}}"#,
            ),
        ] {
            fs::create_dir_all(root.path().join("chronicle").join(day)).unwrap();
            fs::write(
                root.path().join("chronicle").join(day).join("stats.json"),
                stats,
            )
            .unwrap();
        }
        root
    }

    #[test]
    fn a_late_evening_pulse_reports_the_local_day_not_the_utc_one() {
        // 2026-09-05 21:30 in Denver is already 2026-09-06 03:30 UTC.
        let root = september_journal();
        let context = HomeContext::with_day_offset(
            root.path(),
            Utc.with_ymd_and_hms(2026, 9, 6, 3, 30, 0).unwrap(),
            mountain_day(),
        );
        let payload = pulse_payload(&context);
        assert_eq!(payload["today"], "20260905");
        assert_eq!(payload["now"], "2026-09-05T21:30:00.000000");
        assert_eq!(
            payload["segment_count"], 12,
            "the day still running supplies the flow narrative's segments"
        );
        assert_eq!(
            payload["briefing_phase"], "eod",
            "no briefing is pending at half past nine at night"
        );
        assert_eq!(
            payload["briefing_lateness"],
            json!({"late":false,"late_hours":0})
        );
        assert_eq!(
            payload.pointer("/yesterday_processing/gap_links"),
            Some(&json!([{
                "text": "your morning briefing wasn't prepared overnight.",
                "href": "/app/thinking/#runs/20260905/morning_briefing",
            }])),
            "the briefing gap points at today's local run, not tomorrow's",
        );
    }

    #[test]
    fn the_overnight_gap_lines_wait_for_the_overnight_window_to_pass() {
        let root = september_journal();
        let early = HomeContext::with_day_offset(
            root.path(),
            // 2026-09-05 08:00 in Denver: the morning briefing is still due.
            Utc.with_ymd_and_hms(2026, 9, 5, 14, 0, 0).unwrap(),
            mountain_day(),
        );
        let payload = pulse_payload(&early);
        assert_eq!(payload["today"], "20260905");
        assert_eq!(
            payload.pointer("/yesterday_processing/gap_links"),
            Some(&json!([])),
            "nothing overnight has missed its window at eight in the morning",
        );
        assert_eq!(
            payload.pointer("/yesterday_processing/status_reasons"),
            Some(&json!(["pipeline_warning"])),
        );

        let late = HomeContext::with_day_offset(
            root.path(),
            // 2026-09-05 11:00 in Denver: the window has closed.
            Utc.with_ymd_and_hms(2026, 9, 5, 17, 0, 0).unwrap(),
            mountain_day(),
        );
        let payload = pulse_payload(&late);
        assert_eq!(
            payload.pointer("/yesterday_processing/gap_links"),
            Some(&json!([{
                "text": "your morning briefing wasn't prepared overnight.",
                "href": "/app/thinking/#runs/20260905/morning_briefing",
            }])),
        );
        assert_eq!(
            payload.pointer("/yesterday_processing/status_reasons"),
            Some(&json!(["pipeline_warning", "briefing_missing"])),
        );
    }

    #[test]
    fn absent_yesterday_stats_and_empty_yesterday_return_none() {
        let root = TempDir::new().unwrap();
        let context = utc_context(
            root.path(),
            Utc.with_ymd_and_hms(2026, 8, 14, 13, 0, 0).unwrap(),
        );
        assert!(summarize_yesterday_processing(&context, 1).is_none());

        fs::create_dir_all(root.path().join("chronicle/20260813")).unwrap();
        fs::write(
            root.path().join("chronicle/20260813/stats.json"),
            r#"{"stats":{"transcript_duration":0,"transcript_segments":0},"facet_data":{}}"#,
        )
        .unwrap();
        assert!(summarize_yesterday_processing(&context, 1).is_none());
    }

    #[test]
    fn yesterday_processing_is_absent_on_the_first_journal_day() {
        let root = TempDir::new().unwrap();
        fs::create_dir_all(root.path().join("chronicle/20260813")).unwrap();
        fs::write(
            root.path().join("chronicle/20260813/stats.json"),
            r#"{"stats":{"transcript_duration":60,"transcript_segments":1},"facet_data":{"focus":{"minutes":1,"count":1}}}"#,
        )
        .unwrap();
        let context = utc_context(
            root.path(),
            Utc.with_ymd_and_hms(2026, 8, 14, 13, 0, 0).unwrap(),
        );
        assert!(summarize_yesterday_processing(&context, 0).is_none());
    }

    #[test]
    fn sparse_yesterday_precedes_processing_warnings() {
        let root = TempDir::new().unwrap();
        fs::create_dir_all(root.path().join("chronicle/20260813")).unwrap();
        fs::write(
            root.path().join("chronicle/20260813/stats.json"),
            r#"{"stats":{"transcript_duration":60,"transcript_segments":1},"facet_data":{}}"#,
        )
        .unwrap();
        let context = utc_context(
            root.path(),
            Utc.with_ymd_and_hms(2026, 8, 14, 13, 0, 0).unwrap(),
        );
        let processing = summarize_yesterday_processing(&context, 1).unwrap();
        assert_eq!(processing["mode"], "sparse");
        assert_eq!(processing["title"], "Yesterday's processing");
        assert_eq!(
            processing["status_reasons"],
            json!(["pipeline_warning", "briefing_missing"])
        );
        assert_eq!(
            processing["sparse_lines"],
            json!([
                "no facet newsletters written.",
                "there wasn't much else to process."
            ])
        );
    }

    #[test]
    fn pipeline_summary_caps_real_failures_before_gap_links() {
        let root = TempDir::new().unwrap();
        let failures = (0..21)
            .map(|index| {
                json!({
                    "event": "talent.fail",
                    "ts": index + 1,
                    "mode": "daily",
                    "name": format!("failure_{index:02}"),
                    "use_id": format!("run-{index:02}"),
                })
                .to_string()
            })
            .collect::<Vec<_>>()
            .join("\n");
        write_think_oplog(root.path(), "20260813", "daily", &format!("{failures}\n"));
        let context = utc_context(
            root.path(),
            Utc.with_ymd_and_hms(2026, 8, 14, 13, 0, 0).unwrap(),
        );
        let pipeline = summarize_pipeline_day(&context, "20260813");
        assert_eq!(
            pipeline["anomalies"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|anomaly| anomaly["kind"] == "talent_failure")
                .count(),
            20
        );
        assert_eq!(pipeline["talents"]["outstanding_failed"], 21);
        assert_eq!(pipeline["talents"]["failed_list_truncated"], true);
        assert!(
            format_gap_links(&pipeline, true, "20260813", "20260814", true)
                .iter()
                .any(|link| link["text"] == "…and 1 more didn't finish.")
        );
        // The honest total (21) survives even though the anomalies array
        // format_gap_links groups from is itself capped at 20.
        assert_eq!(crate::formatting::count_failed_runs(&pipeline), 21);
    }

    #[test]
    fn non_null_attention_is_reshaped_for_the_public_payload() {
        let mut fields = Map::new();
        fields.insert(
            "attention".to_owned(),
            json!({
                "placeholder_text": "Review your calendar",
                "context_lines": ["One meeting", "One follow-up"],
                "private_detail": "not public",
            }),
        );
        let payload = PulseContext {
            fields,
            now: Utc
                .with_ymd_and_hms(2026, 8, 14, 13, 0, 0)
                .unwrap()
                .fixed_offset(),
        }
        .into_pulse_payload();
        assert_eq!(
            payload["attention"],
            json!({
                "placeholder_text": "Review your calendar",
                "context_lines": ["One meeting", "One follow-up"],
            })
        );
        assert_eq!(payload["attention"].as_object().unwrap().len(), 2);
    }

    #[test]
    fn missing_yesterday_health_is_stale_and_degrades_processing() {
        let root = TempDir::new().unwrap();
        fs::create_dir_all(root.path().join("chronicle/20260813")).unwrap();
        fs::write(
            root.path().join("chronicle/20260813/stats.json"),
            r#"{"stats":{"transcript_duration":60,"transcript_segments":0},"facet_data":{"focus":{"minutes":1,"count":1}}}"#,
        )
        .unwrap();
        let context = utc_context(
            root.path(),
            Utc.with_ymd_and_hms(2026, 8, 14, 13, 0, 0).unwrap(),
        );
        assert_eq!(
            summarize_pipeline_day(&context, "20260813")["status"],
            "stale"
        );
        let processing = summarize_yesterday_processing(&context, 1).unwrap();
        assert_eq!(processing["mode"], "degraded");
        assert_eq!(processing["title"], "⚠ Yesterday's processing");
    }

    #[test]
    fn assembly_does_not_create_awareness_in_the_empty_fixture() {
        let root =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/convey_home_empty_journal");
        let before = tree_entries(&root);
        let context = utc_context(
            &root,
            Utc.with_ymd_and_hms(2026, 8, 14, 22, 28, 35)
                .unwrap()
                .with_nanosecond(430_840_000)
                .unwrap(),
        );
        let _ = pulse_payload(&context);
        assert_eq!(tree_entries(&root), before);
        assert!(!root.join("awareness").exists());
    }

    #[test]
    fn client_capture_is_active_at_twenty_nine_seconds() {
        let root = TempDir::new().unwrap();
        fs::create_dir_all(root.path().join("link")).unwrap();
        fs::write(
            root.path().join("link/authorized_clients.json"),
            r#"[{"fingerprint":"cid","device_label":"phone","paired_at":"2026-08-13T00:00:00Z","instance_id":"fixture","kind":"cert"}]"#,
        )
        .unwrap();
        fs::write(
            root.path().join("link/devices.json"),
            r#"{"cid":{"last_seen_at":"2026-08-14T23:24:44.793Z","last_accepted_ingest_at":"2026-08-14T23:24:44.793Z"}}"#,
        )
        .unwrap();
        let context = utc_context(
            root.path(),
            Utc.timestamp_millis_opt(1_786_749_913_793)
                .single()
                .unwrap(),
        );
        assert_eq!(
            crate::readers::get_capture_health(&context)["status"],
            "active"
        );
        assert_eq!(last_observe_relative_seconds(&context), Some(29));
    }

    #[test]
    fn reflection_fixture_accepts_an_unparseable_eight_digit_stem() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/convey_home_reflection_journal");
        let context = utc_context(root, Utc.with_ymd_and_hms(2026, 8, 14, 12, 0, 0).unwrap());
        assert_eq!(
            load_latest_weekly_reflection(&context),
            Some(json!({"day":"99999999","label":"99999999"}))
        );
    }

    #[test]
    fn seeded_fixture_keeps_the_no_client_cta_branch() {
        let root =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/convey_home_seeded_journal");
        let context = utc_context(
            root,
            Utc.with_ymd_and_hms(2026, 8, 14, 23, 25, 13)
                .unwrap()
                .with_nanosecond(793_525_000)
                .unwrap(),
        );
        assert_eq!(
            pulse_payload(&context)["health_glance"]["cta"]["href"],
            "/app/network/"
        );
    }

    #[test]
    fn assembly_keeps_muted_activities_but_excludes_them_from_yesterday_processing() {
        let (_fixture, context) = fixture_context(
            "convey_home_seeded_journal",
            Utc.with_ymd_and_hms(2026, 8, 14, 23, 25, 13)
                .unwrap()
                .with_nanosecond(793_525_000)
                .unwrap(),
        );
        let payload = pulse_payload(&context);
        assert!(
            payload["activities"]
                .as_array()
                .unwrap()
                .iter()
                .any(|activity| activity["facet"] == "muted")
        );
        assert!(
            payload["anticipated_activities"]
                .as_array()
                .unwrap()
                .iter()
                .any(|activity| activity["facet"] == "muted")
        );
        assert!(
            !payload["yesterday_processing"]["details"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(Value::as_str)
                .any(|detail| detail.contains("Muted"))
        );
        assert!(!payload.to_string().contains("Unlisted"));
    }

    fn write_principal(root: &Path) {
        solstone_core_entity::save_entity_identity(
            root,
            "owner",
            &json!({"id":"owner","name":"Owner","type":"Person","is_principal":true}),
            None,
        )
        .unwrap();
    }

    fn horizon_context(root: &Path) -> HomeContext {
        utc_context(root, Utc.with_ymd_and_hms(2026, 6, 2, 13, 0, 0).unwrap())
    }

    fn assert_horizon_failure_does_not_change_card(root: &Path) {
        let ctx = horizon_context(root);
        assert!(refresh_connections_horizon(root).is_none());
        let with_refresh = load_connections_card(&ctx, refresh_connections_horizon(root));
        let with_none = load_connections_card(&ctx, None);
        assert_eq!(with_refresh, with_none);
        assert!(with_refresh.get("horizon_day").is_none());
        assert!(with_refresh.get("horizon_note").is_none());
    }

    #[test]
    fn load_connections_card_does_not_degrade_when_horizon_scan_fails() {
        let missing = TempDir::new().unwrap();
        write_principal(missing.path());
        assert_horizon_failure_does_not_change_card(missing.path());

        let poisoned = TempDir::new().unwrap();
        write_principal(poisoned.path());
        fs::create_dir_all(poisoned.path().join("facets")).unwrap();
        fs::write(
            poisoned
                .path()
                .join("facets/.connections-horizon-cache.json"),
            "not-json{{{",
        )
        .unwrap();
        assert_horizon_failure_does_not_change_card(poisoned.path());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let unreadable = TempDir::new().unwrap();
            write_principal(unreadable.path());
            solstone_core_facets::create_facet(
                unreadable.path(),
                "work",
                "work",
                "Description",
                "blue",
                "💼",
                None,
            )
            .unwrap();
            let day = unreadable
                .path()
                .join("facets/work/entities/20260301.jsonl");
            fs::create_dir_all(day.parent().unwrap()).unwrap();
            fs::write(&day, "{\"name\":\"Ada\",\"segments\":[\"seg-1\"]}\n").unwrap();
            fs::create_dir_all(unreadable.path().join("chronicle/20260101")).unwrap();
            fs::set_permissions(&day, fs::Permissions::from_mode(0o000)).unwrap();
            if fs::File::open(&day).is_ok() {
                fs::set_permissions(&day, fs::Permissions::from_mode(0o600)).unwrap();
                return;
            }
            assert_horizon_failure_does_not_change_card(unreadable.path());
            fs::set_permissions(&day, fs::Permissions::from_mode(0o600)).unwrap();
        }
    }

    #[test]
    fn load_connections_card_omits_horizon_when_store_has_no_gap() {
        let root = TempDir::new().unwrap();
        write_principal(root.path());
        solstone_core_facets::create_facet(
            root.path(),
            "work",
            "work",
            "Description",
            "blue",
            "💼",
            None,
        )
        .unwrap();
        let day = root.path().join("facets/work/entities/20260101.jsonl");
        fs::create_dir_all(day.parent().unwrap()).unwrap();
        fs::write(&day, "{\"name\":\"Ada\",\"segments\":[\"seg-1\"]}\n").unwrap();
        fs::create_dir_all(root.path().join("chronicle/20260101")).unwrap();
        let ctx = horizon_context(root.path());
        let horizon = refresh_connections_horizon(root.path());
        assert!(horizon.is_none());
        let card = load_connections_card(&ctx, horizon);
        assert!(card.get("horizon_day").is_none());
        assert!(card.get("horizon_note").is_none());
    }

    fn utc_day() -> FixedOffset {
        FixedOffset::east_opt(0).expect("utc day offset")
    }

    /// Pin the day coordinate so a test's expectations do not depend on the
    /// host's zone; the local-day behaviour has its own tests below.
    fn utc_context(root: impl Into<std::path::PathBuf>, now: DateTime<Utc>) -> HomeContext {
        HomeContext::with_day_offset(root, now, utc_day())
    }

    fn fixture_context(fixture: &str, now: DateTime<Utc>) -> (TempDir, HomeContext) {
        let source = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures")
            .join(fixture);
        let root = TempDir::new().unwrap();
        copy_fixture(&source, root.path());
        if fixture == "convey_home_seeded_journal" {
            write_think_oplog(
                root.path(),
                "20260813",
                "daily",
                "{\"event\":\"run.summary\"}\n",
            );
        }
        // The captured references were recorded with a UTC day coordinate, so
        // pin it here rather than let the host's zone decide which day it is.
        let context = HomeContext::with_day_offset(root.path(), now, utc_day());
        (root, context)
    }

    fn copy_fixture(source: &Path, destination: &Path) {
        fs::create_dir_all(destination).unwrap();
        for entry in fs::read_dir(source).unwrap().filter_map(Result::ok) {
            let target = destination.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                copy_fixture(&entry.path(), &target);
            } else {
                fs::copy(entry.path(), target).unwrap();
            }
        }
    }

    fn reference_payload(name: &str) -> Value {
        let text = match name {
            "reference-pulse-empty-journal.json" => {
                include_str!("../../../fixtures/reference-pulse-empty-journal.json")
            }
            "reference-pulse-seeded-journal.json" => {
                include_str!("../../../fixtures/reference-pulse-seeded-journal.json")
            }
            _ => panic!("unknown reference payload: {name}"),
        };
        serde_json::from_str(text).unwrap()
    }

    fn assert_payload_fields(actual: &Value, expected: &Value, exceptions: &[&str]) {
        let actual = actual.as_object().unwrap();
        let expected = expected.as_object().unwrap();
        assert_eq!(
            actual.keys().collect::<BTreeSet<_>>(),
            expected.keys().collect::<BTreeSet<_>>(),
            "payload key set"
        );
        for key in expected.keys() {
            if exceptions.contains(&key.as_str()) {
                continue;
            }
            assert_eq!(actual.get(key), expected.get(key), "payload field {key}");
        }
    }

    fn tree_entries(root: &Path) -> Vec<String> {
        let mut entries =
            fs::read_dir(root)
                .unwrap()
                .filter_map(Result::ok)
                .flat_map(|entry| {
                    let path = entry.path();
                    let mut entries = vec![path.strip_prefix(root).unwrap().display().to_string()];
                    if path.is_dir() {
                        entries.extend(tree_entries(&path).into_iter().map(|child| {
                            format!("{}/{}", entry.file_name().to_string_lossy(), child)
                        }));
                    }
                    entries
                })
                .collect::<Vec<_>>();
        entries.sort();
        entries
    }
}
