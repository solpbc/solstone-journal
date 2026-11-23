// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use jiff::Span;
use jiff::civil::Date;
use serde::Deserialize;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use solstone_core_format::content::week_left_out::{LeftOut, read_left_out};
use solstone_core_home::weekly::{
    Said, SaidEvidence, TrustedSaid, WeekJudgment, checked_neighbors, format_week_intro, judge,
    resolve_day_cell_state, week_day,
};
use solstone_core_indexer_query::AdmittedCategory;

use super::ToolError;
use crate::dispatch::{DispatchPrincipal, available_facets, codec};
use crate::permissions::ConnectionReadSnapshot;
use crate::references::{
    ActivityReference, LocationDayReference, ReferenceKind, ReferenceTarget, WeekRangeCursor,
};
use crate::registry::category_token;
use crate::segment_links::{LinkSubject, LiveTranscriptPages, issue_segment_links};

/// Maximum inclusive days one range request may span (one full leap year).
pub(crate) const MAX_RANGE_DAYS: usize = 366;
/// Maximum day rows returned in one range page.
pub(crate) const MAX_SPAN_PAGE_DAYS: usize = 100;
/// Soft serialized byte ceiling per range page, keeping results comfortably
/// inside hosted agent tool-result limits even when `include` is set.
pub(crate) const MAX_SPAN_PAGE_BYTES: usize = 96 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WeekQuery {
    Single(Option<String>),
    Range {
        day_from: String,
        day_to: String,
        cursor: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ValidatedGetWeek {
    pub(crate) query: WeekQuery,
    pub(crate) include: BTreeSet<AdmittedCategory>,
}

pub(crate) const SUMMARY_KINDS: &[AdmittedCategory] = &[
    AdmittedCategory::Body,
    AdmittedCategory::Places,
    AdmittedCategory::Photos,
    AdmittedCategory::Notes,
    AdmittedCategory::Entities,
];

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WeekParams {
    #[serde(default)]
    week: Option<Value>,
    #[serde(default)]
    day_from: Option<Value>,
    #[serde(default)]
    day_to: Option<Value>,
    #[serde(default)]
    cursor: Option<Value>,
    #[serde(default)]
    include: Vec<String>,
}

fn parse_optional_day(value: Option<Value>) -> Result<Option<String>, ToolError> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => {
            if s.len() > super::MAX_DAY_BYTES {
                return Err(ToolError::InvalidInput);
            }
            let Some(day) = super::normalize_day(Some(s))? else {
                return Err(ToolError::InvalidInput);
            };
            if week_day(&day).is_none() {
                return Err(ToolError::InvalidInput);
            }
            Ok(Some(day))
        }
        Some(_) => Err(ToolError::InvalidInput),
    }
}

fn parse_optional_cursor(value: Option<Value>) -> Result<Option<String>, ToolError> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => {
            if s.is_empty() || s.len() > super::MAX_OPAQUE_REFERENCE_BYTES {
                return Err(ToolError::InvalidInput);
            }
            Ok(Some(s))
        }
        Some(_) => Err(ToolError::InvalidInput),
    }
}

pub(crate) fn validate(params: Option<&Value>) -> Result<ValidatedGetWeek, ToolError> {
    let params = match params {
        None | Some(Value::Null) => {
            return Ok(ValidatedGetWeek {
                query: WeekQuery::Single(None),
                include: BTreeSet::new(),
            });
        }
        Some(val) => val,
    };
    let params = serde_json::from_value::<WeekParams>(params.clone())
        .map_err(|_| ToolError::InvalidInput)?;

    // Any day names its week, written either way a day is elsewhere.
    let week = parse_optional_day(params.week)?;
    let day_from = parse_optional_day(params.day_from)?;
    let day_to = parse_optional_day(params.day_to)?;
    let cursor = parse_optional_cursor(params.cursor)?;

    if params.include.len() > SUMMARY_KINDS.len() {
        return Err(ToolError::InvalidInput);
    }
    let include = params
        .include
        .iter()
        .map(|word| {
            AdmittedCategory::from_stored_word(word)
                .filter(|kind| SUMMARY_KINDS.contains(kind))
                .ok_or(ToolError::InvalidInput)
        })
        .collect::<Result<_, _>>()?;

    let has_range_arg = day_from.is_some() || day_to.is_some() || cursor.is_some();
    if week.is_some() && has_range_arg {
        return Err(ToolError::WeekRangeArguments);
    }

    let query = match (day_from, day_to) {
        (None, None) => {
            if cursor.is_some() {
                return Err(ToolError::WeekRangeArguments);
            }
            WeekQuery::Single(week)
        }
        (Some(from), Some(to)) => {
            let (Some(from_date), Some(to_date)) = (week_day(&from), week_day(&to)) else {
                return Err(ToolError::InvalidInput);
            };
            let span_days = from_date
                .until(to_date)
                .map_err(|_| ToolError::WeekRangeArguments)?
                .get_days();
            if span_days < 0 || span_days >= MAX_RANGE_DAYS as i32 {
                return Err(ToolError::WeekRangeArguments);
            }
            WeekQuery::Range {
                day_from: from,
                day_to: to,
                cursor,
            }
        }
        _ => return Err(ToolError::WeekRangeArguments),
    };

    Ok(ValidatedGetWeek { query, include })
}

pub(crate) struct WeekRead {
    pub(crate) value: Value,
    pub(crate) count: usize,
    pub(crate) targets: Vec<String>,
}

fn sunday_of(date: Date) -> Date {
    date - Span::new().days(date.weekday().to_sunday_zero_offset())
}

fn agent_day_state<'a>(stored: &'a str, memory_key: Option<&str>, left_out: &LeftOut) -> &'a str {
    match left_out {
        LeftOut::Keys(keys) => resolve_day_cell_state(stored, memory_key, keys),
        LeftOut::Unreadable if stored == "memory" => "not_on_page",
        _ => stored,
    }
}

fn week_entry_visible(key: &str, left_out: &LeftOut) -> bool {
    match left_out {
        LeftOut::None => true,
        LeftOut::Keys(keys) => !keys.contains(key),
        LeftOut::Unreadable => false,
    }
}

pub(crate) fn read_week(
    journal: &Path,
    request: &ValidatedGetWeek,
    principal: DispatchPrincipal<'_>,
    snapshot: &ConnectionReadSnapshot,
) -> Result<WeekRead, ToolError> {
    match &request.query {
        WeekQuery::Single(week) => {
            let sunday_date = if let Some(day) = week.as_deref() {
                let Some(date) = week_day(day) else {
                    return Err(ToolError::InvalidInput);
                };
                // The week a day falls in starts on the Sunday on or before it.
                sunday_of(date)
            } else {
                let owner_zone = solstone_core_journal_config::owner_zone(journal);
                let today = jiff::Timestamp::now().to_zoned_datetime(owner_zone).date();
                sunday_of(today)
            };
            project_sunday(journal, sunday_date, principal, snapshot, &request.include)
        }
        WeekQuery::Range {
            day_from,
            day_to,
            cursor,
        } => project_range_with_limits(
            journal,
            day_from,
            day_to,
            cursor.as_deref(),
            principal,
            snapshot,
            &request.include,
            MAX_SPAN_PAGE_DAYS,
            MAX_SPAN_PAGE_BYTES,
        ),
    }
}

fn project_sunday(
    journal: &Path,
    sunday: Date,
    principal: DispatchPrincipal<'_>,
    snapshot: &ConnectionReadSnapshot,
    include: &BTreeSet<AdmittedCategory>,
) -> Result<WeekRead, ToolError> {
    let start_day = solstone_core_time::day_key(sunday);
    // A week ending past the last representable day is not one that can be read.
    let saturday = sunday
        .checked_add(Span::new().days(6))
        .map_err(|_| ToolError::InvalidInput)?;
    let end_day = solstone_core_time::day_key(saturday);

    let week_dates: Vec<Date> = (0..7).map(|d| sunday + Span::new().days(d)).collect();
    let prev_dates: Vec<Date> = (1..=7).map(|d| sunday - Span::new().days(8 - d)).collect();

    let available = available_facets(journal, snapshot).map_err(|_| ToolError::FileUnreadable)?;
    let facets: Vec<(String, String, String)> = available
        .into_iter()
        .map(|(id, dir, decl)| {
            let title = decl.as_ref().map_or_else(
                || dir.clone(),
                |v| {
                    if v.title.is_empty() {
                        dir.clone()
                    } else {
                        v.title.clone()
                    }
                },
            );
            (id, dir, title)
        })
        .collect();

    let mut prior_keys = BTreeSet::new();
    for date in &prev_dates {
        let day_str = solstone_core_time::day_key(*date);
        for (_, dir, _) in &facets {
            let rows = solstone_core_facets::read_activity_records_strict(journal, dir, &day_str)
                .map_err(|_| ToolError::FileUnreadable)?;
            let superseded =
                solstone_core_facets::superseded_record_ids(journal, dir, &day_str, &rows);
            for row in rows {
                if row.get("hidden").and_then(Value::as_bool).unwrap_or(false)
                    || solstone_core_facets::is_retired_plan(&row)
                    || solstone_core_facets::is_body_record(&row)
                    || row
                        .get("id")
                        .and_then(Value::as_str)
                        .is_some_and(|id| superseded.contains(id))
                {
                    continue;
                }
                let k = group_key_for_record(&row);
                if !k.is_empty() {
                    prior_keys.insert(k);
                }
            }
        }
    }

    struct Collected {
        record: Map<String, Value>,
        day: String,
        facet_id: String,
        facet_dir: String,
        facet_title: String,
        file_digest: String,
        record_identity: String,
        duration: u64,
        sort_key: (String, String, i64, String),
        /// The other facets holding a copy of this moment, by title, and
        /// those copies' records.
        also_in: Vec<String>,
        copies: Vec<Map<String, Value>>,
    }

    let mut collected = Vec::new();
    // Body faces, composed for this connection: workouts, nights and sessions.
    let gate = crate::body_source::window_gate(journal, snapshot);
    // A body record or Strava line this connection would read with the body
    // permission, so the week says it needs it.
    let mut body_withheld = false;
    let mut body = Vec::new();
    // The windows body faces were composed from, for the audit.
    let mut windows = Vec::new();
    // Each facet's faces over the week, grouped once, the first time a day
    // file asks.
    let mut week_faces: BTreeMap<String, BTreeMap<String, Vec<_>>> = BTreeMap::new();
    let first_day = week_dates
        .first()
        .map(|date| solstone_core_time::day_key(*date));
    let last_day = week_dates
        .last()
        .map(|date| solstone_core_time::day_key(*date));
    for date in &week_dates {
        let day_str = solstone_core_time::day_key(*date);
        for (f_id, f_dir, f_title) in &facets {
            let rel_path = format!("facets/{f_dir}/activities/{day_str}.jsonl");
            let day_file_bytes = crate::activity_admit::read_contained_source(journal, &rel_path)
                .unwrap_or_default();
            if day_file_bytes.is_empty() {
                let rows =
                    solstone_core_facets::read_activity_records_strict(journal, f_dir, &day_str)
                        .map_err(|_| ToolError::FileUnreadable)?;
                if !rows.is_empty() {
                    return Err(ToolError::FileUnreadable);
                }
                continue;
            }
            let rows = solstone_core_facets::read_activity_records_strict(journal, f_dir, &day_str)
                .map_err(|_| ToolError::FileUnreadable)?;
            let file_digest = crate::activity_admit::hex_encode(&Sha256::digest(&day_file_bytes));

            let file_text =
                String::from_utf8(day_file_bytes).map_err(|_| ToolError::FileUnreadable)?;
            let mut line_identities = Vec::new();
            for line in file_text.lines() {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                let Ok(Value::Object(rec)) = serde_json::from_str::<Value>(trimmed) else {
                    return Err(ToolError::FileUnreadable);
                };
                line_identities.push(crate::activity_admit::compute_record_identity(
                    &rec, trimmed,
                ));
            }
            if rows.len() != line_identities.len() {
                return Err(ToolError::FileUnreadable);
            }
            if !week_faces.contains_key(f_dir) {
                let faces = solstone_core_location::body_records::body_faces_range(
                    journal,
                    f_dir,
                    first_day.as_deref().unwrap_or(&day_str),
                    last_day.as_deref().unwrap_or(&day_str),
                    Some(&gate),
                    false,
                )
                .map_err(|_| ToolError::FileUnreadable)?;
                week_faces.insert(f_dir.clone(), faces);
            }
            let faces = week_faces
                .get(f_dir)
                .and_then(|faces| faces.get(&day_str))
                .map_or(&[][..], Vec::as_slice);

            // The same test every read of one of these records makes again.
            let judged =
                crate::may_see::ActivityDay::new(journal, snapshot, f_dir, &day_str, &rows);
            for (row, rec_id) in rows.into_iter().zip(line_identities) {
                match judged.shows(&row) {
                    crate::may_see::Shown::Yes => {}
                    crate::may_see::Shown::No => continue,
                    crate::may_see::Shown::NeedsBody => {
                        body_withheld = true;
                        continue;
                    }
                }
                if solstone_core_facets::is_body_record(&row) {
                    let id = row.get("id").and_then(Value::as_str);
                    if let Some(face) = faces
                        .iter()
                        .find(|face| Some(face.anchor.id.as_str()) == id)
                    {
                        crate::body_source::name_windows(
                            &mut windows,
                            crate::body_source::face_windows(face),
                        );
                        body.push((
                            day_str.clone(),
                            face.anchor.stored_start.clone(),
                            ActivityReference {
                                facet_id: f_id.clone(),
                                facet_dir: f_dir.clone(),
                                day: day_str.clone(),
                                record_identity: rec_id,
                                file_digest: file_digest.clone(),
                                unit_index: None,
                                source_binding: None,
                            },
                            f_title.clone(),
                            solstone_core_location::body_records::face_json(face),
                        ));
                    }
                    continue;
                }
                let (_, _, dur) = crate::tools::activities::activity_times_and_duration(&row);
                let sort_key = crate::tools::activities::sort_activity_key(&day_str, f_id, &row);
                collected.push(Collected {
                    record: row,
                    day: day_str.clone(),
                    facet_id: f_id.clone(),
                    facet_dir: f_dir.clone(),
                    facet_title: f_title.clone(),
                    file_digest: file_digest.clone(),
                    record_identity: rec_id,
                    duration: dur.unwrap_or(0),
                    sort_key,
                    also_in: Vec::new(),
                    copies: Vec::new(),
                });
            }
        }
    }

    // A moment recorded in several facets counts once, and its time once,
    // as list_activities lists it.
    collected.sort_by(|a, b| a.sort_key.cmp(&b.sort_key));
    let collected: Vec<Collected> = solstone_core_facets::merge_moment_copies(
        collected,
        |item| item.day.as_str(),
        |item| &item.record,
    )
    .into_iter()
    .map(|moment| {
        let mut primary = moment.primary;
        for copy in moment.copies {
            primary.also_in.push(copy.facet_title);
            primary.copies.push(copy.record);
        }
        primary
    })
    .collect();
    let mut people_by_day = BTreeMap::<String, BTreeSet<String>>::new();
    if include.contains(&AdmittedCategory::Entities)
        && snapshot.categories.contains(&AdmittedCategory::Entities)
    {
        for row in &collected {
            let people = crate::tools::activities::activity_people_across(
                std::iter::once(&row.record).chain(row.copies.iter()),
            );
            people_by_day
                .entry(row.day.clone())
                .or_default()
                .extend(people.participants.into_iter().chain(people.heard));
        }
    }
    let activities_total = collected.len();
    let mut groups: BTreeMap<String, Vec<Collected>> = BTreeMap::new();
    for item in collected {
        let k = group_key_for_record(&item.record);
        groups.entry(k).or_default().push(item);
    }

    struct GroupRank {
        representative: Collected,
        recurrence: usize,
        duration: u64,
        participant_count: usize,
        novelty: u8,
        earliest_sort_key: (String, String, i64, String),
    }

    let mut ranked = Vec::new();
    for (k, mut items) in groups {
        let distinct_days: BTreeSet<&str> = items.iter().map(|it| it.day.as_str()).collect();
        let recurrence = distinct_days.len();
        let duration: u64 = items.iter().map(|it| it.duration).sum();
        let participant_count = items
            .iter()
            .map(|it| crate::tools::activities::activity_entities(&it.record).len())
            .max()
            .unwrap_or(0);
        let novelty = if !prior_keys.contains(&k) { 1 } else { 0 };

        items.sort_by(|a, b| a.sort_key.cmp(&b.sort_key));
        let earliest_sort_key = items[0].sort_key.clone();
        let representative = items.remove(0);

        ranked.push(GroupRank {
            representative,
            recurrence,
            duration,
            participant_count,
            novelty,
            earliest_sort_key,
        });
    }

    ranked.sort_by(|a, b| {
        b.recurrence
            .cmp(&a.recurrence)
            .then_with(|| b.duration.cmp(&a.duration))
            .then_with(|| b.participant_count.cmp(&a.participant_count))
            .then_with(|| b.novelty.cmp(&a.novelty))
            .then_with(|| a.earliest_sort_key.cmp(&b.earliest_sort_key))
    });
    // One row from each day first, in that order, then fill any seats still
    // open from the same order. A recurring name stays one row, on its earliest day.
    let mut spread = Vec::new();
    let mut deferred = Vec::new();
    let mut seen_days = BTreeSet::new();
    for group in ranked {
        if seen_days.insert(group.representative.day.clone()) {
            spread.push(group);
            if spread.len() == 7 {
                break;
            }
        } else {
            deferred.push(group);
        }
    }
    if spread.len() < 7 {
        for group in deferred {
            if spread.len() == 7 {
                break;
            }
            spread.push(group);
        }
    }
    let ranked = spread;

    let owner_zone = solstone_core_journal_config::owner_zone(journal);
    let now = jiff::Timestamp::now().to_zoned_datetime(owner_zone.clone());
    let zone_str =
        solstone_core_journal_config::zone_label(Some(&owner_zone), now.offset().seconds());

    let sources = crate::dispatch::source_labels(journal);
    let mut activities = Vec::new();
    let mut memories = Vec::new();
    let possible_names = crate::possible_names::ConnectionPossibleNames::new(journal, snapshot);
    let mut checker = LiveTranscriptPages::new(std::sync::Arc::clone(&possible_names), snapshot);
    for group in ranked {
        let rep = group.representative;
        let reference = crate::dispatch::activity_reference(
            principal,
            snapshot,
            ActivityReference {
                facet_id: rep.facet_id.clone(),
                facet_dir: rep.facet_dir.clone(),
                day: rep.day.clone(),
                record_identity: rep.record_identity.clone(),
                file_digest: rep.file_digest.clone(),
                unit_index: None,
                source_binding: None,
            },
        )
        .map_err(|_| ToolError::ReferenceNotFound)?;

        let (start, end, duration) =
            crate::tools::activities::activity_times_and_duration(&rep.record);
        let people = crate::tools::activities::activity_people_across(
            std::iter::once(&rep.record).chain(rep.copies.iter()),
        );
        let entities = crate::tools::activities::activity_entities(&rep.record);
        let title = crate::tools::activities::activity_title(&rep.record);

        let activity_content_kind = if solstone_core_format::content::is_strava_reading(&rep.record)
        {
            "body"
        } else {
            "facets"
        };
        let mut activity = json!({
            "title": title,
            "facet": rep.facet_title,
            "day": rep.day,
            "content_kind": activity_content_kind,
            "start": start,
            "end": end,
            "duration": duration,
            "participants": people.participants,
            "heard": people.heard,
            "mentioned": people.mentioned,
            "on_screen": people.on_screen,
            "entities": entities,
            "reference": reference.clone(),
            "zone": zone_str,
        });
        crate::journal_links::activity(
            journal,
            &mut activity,
            &rep.facet_dir,
            &rep.day,
            &rep.record_identity,
        );
        if let Some(kind) = crate::tools::activities::activity_kind(&rep.record) {
            activity["kind"] = json!(kind);
        }
        if let Some(stream) = rep.record.get("stream").and_then(Value::as_str) {
            activity["source"] = json!(crate::dispatch::source_label(&sources, stream));
        }
        if people.unnamed_voices > 0 {
            activity["unnamed_voices"] = json!(people.unnamed_voices);
        }
        if !rep.also_in.is_empty() {
            activity["also_in"] = json!(rep.also_in);
        }
        activities.push(activity);

        let mem_text = format_memory_text(&title, start.as_deref(), end.as_deref());

        let rel_path = format!("facets/{}/activities/{}.jsonl", rep.facet_dir, rep.day);
        let mut single_subject = [LinkSubject {
            path: &rel_path,
            idx: 0,
            text: "",
            segments: Vec::new(),
            more_segments: false,
            activity_record: Some(&rep.record),
        }];
        let mut response_checks = 0;
        issue_segment_links(
            journal,
            principal,
            snapshot,
            &mut single_subject,
            48,
            48,
            &mut response_checks,
            &mut checker,
        );
        let refs = std::mem::take(&mut single_subject[0].segments);

        memories.push(json!({
            "day": rep.day,
            "text": mem_text,
            "source": reference,
            "refs": refs,
        }));
    }

    let stem = &start_day;
    let mut days = Vec::new();
    let mut said = Vec::new();
    let mut previous = None;
    let mut next = None;

    // Day states, quotes and navigation exist only on the week's page. A week
    // with no page it can read leaves all three empty rather than inventing
    // states, because on a page `nothing_shared` means the day held nothing.
    if let WeekJudgment::Page(trusted) = judge(journal, stem) {
        let left_out = read_left_out(journal);

        let navigation = checked_neighbors(journal, stem);
        if !navigation.failed {
            previous = navigation.previous;
            next = navigation.next;
        }

        let memory_by_id = trusted
            .memories
            .iter()
            .map(|m| (m.id.as_str(), m))
            .collect::<BTreeMap<_, _>>();

        for day in &trusted.days {
            let memory = day
                .memory_id
                .as_deref()
                .and_then(|id| memory_by_id.get(id).copied());
            let memory_key = memory.map(|m| m.key.as_str());
            let state = agent_day_state(day.state.as_str(), memory_key, &left_out);
            let mut cell = json!({"day": day.day, "state": state});
            // A calendar cell is already admitted. Mint from its day alone:
            // testing source existence here would reveal withheld body days.
            crate::journal_links::attach(
                journal,
                &mut cell,
                solstone_core_journal_link::Place::Day {
                    day: day.day.clone(),
                },
            );
            days.push(cell);
        }

        if let Said::Entries(said_entries) = &trusted.said {
            let mut speech = None;
            for entry in said_entries {
                if !week_entry_visible(&entry.key, &left_out)
                    || !speech
                        .get_or_insert_with(|| WeekSpeech::new(journal, snapshot))
                        .shows(entry)
                {
                    continue;
                }
                let mut quote = json!({
                    "day": entry.day,
                    "quote": entry.quote,
                    "uri": entry.uri,
                });
                // The weekly reader already validated this activity URI.
                if let Some((facet, day, id)) = parse_said_uri(&entry.uri) {
                    crate::journal_links::activity(journal, &mut quote, facet, day, id);
                }
                said.push(quote);
            }
        }
    }

    if !include.is_empty() {
        let today = solstone_core_time::day_key(now.date());
        let served_days: Vec<_> = week_dates
            .iter()
            .map(|date| solstone_core_time::day_key(*date))
            .filter(|day| day <= &today)
            .collect();
        let mut day_keys = served_days.clone();
        let mut body_days = BTreeMap::new();
        let mut body_summary_note = None;
        if !day_keys.is_empty()
            && include.contains(&AdmittedCategory::Body)
            && snapshot.categories.contains(&AdmittedCategory::Body)
        {
            if let Ok(previous) = sunday.yesterday() {
                day_keys.insert(0, solstone_core_time::day_key(previous));
            }
            match crate::body_source::week_totals(journal, snapshot, &day_keys) {
                Ok((totals, targets)) => {
                    body_days = totals;
                    windows.extend(targets);
                }
                Err(_) => body_summary_note = Some(ToolError::FileUnreadable.agent_message()),
            }
        }
        let mut places = BTreeMap::new();
        let mut places_note = None;
        if include.contains(&AdmittedCategory::Places)
            && let Some(precision) = snapshot.places_precision()
        {
            match week_places_summaries(journal, principal, snapshot, &served_days, precision) {
                Some(summaries) => places = summaries,
                None => places_note = Some(ToolError::FileUnreadable.agent_message()),
            }
        }
        let mut kind_summaries =
            prefetch_kind_summaries(journal, principal, snapshot, &served_days, include);
        let mut cells: BTreeMap<_, _> = days
            .into_iter()
            .filter_map(|cell| Some((cell["day"].as_str()?.to_owned(), cell)))
            .collect();
        for key in served_days {
            let cell = cells.entry(key.clone()).or_insert_with(|| {
                let mut cell = json!({"day": key});
                crate::journal_links::attach(
                    journal,
                    &mut cell,
                    solstone_core_journal_link::Place::Day { day: key.clone() },
                );
                cell
            });
            let people = people_by_day.remove(&key).unwrap_or_default();
            let (summary, day_targets) = build_day_summary(
                snapshot,
                &key,
                include,
                &mut body_days,
                body_summary_note,
                &mut places,
                places_note,
                people,
                &mut kind_summaries,
            );
            windows.extend(day_targets);
            cell["summary"] = Value::Object(summary);
        }
        days = cells.into_values().collect();
    }

    let memory_days = memories
        .iter()
        .filter_map(|memory| memory["day"].as_str())
        .collect::<BTreeSet<_>>()
        .len();
    let intro = format_week_intro(memory_days, said.len())
        .map(Value::String)
        .unwrap_or(Value::Null);

    body.sort_by(|left, right| (&left.0, &left.1).cmp(&(&right.0, &right.1)));
    let mut body_items = Vec::new();
    for (day, _, target, facet_title, face) in body {
        let reference = crate::dispatch::activity_reference(principal, snapshot, target.clone())
            .map_err(|_| ToolError::ReferenceNotFound)?;
        let mut item = crate::tools::activities::body_activity(
            &face,
            &facet_title,
            &day,
            reference,
            &zone_str,
        );
        crate::journal_links::activity(
            journal,
            &mut item,
            &target.facet_dir,
            &target.day,
            &target.record_identity,
        );
        body_items.push(item);
    }

    let value = json!({
        "week": {"start": start_day, "end": end_day},
        "intro": intro,
        "days": days,
        "memories": memories,
        "said": said,
        "previous": previous,
        "next": next,
        "activities": activities,
        "activities_total": activities_total,
    });
    // Only a week with body faces carries the key, so a journal without body
    // data answers exactly as before.
    let mut value = value;
    if !body_items.is_empty() {
        value["body"] = Value::Array(body_items);
    }
    if body_withheld {
        value["body_note"] = json!(crate::body_source::BODY_NOT_GRANTED);
    }
    if include.contains(&AdmittedCategory::Places)
        && let Some(precision) = snapshot.places_precision()
    {
        value["places_precision"] = json!(precision.wire_value());
        let today = solstone_core_time::day_key(now.date());
        if let Some(roster) = crate::places_view::build_places_roster(
            journal,
            Some(&start_day),
            Some(&end_day),
            &today,
            precision,
            None,
            None,
        ) {
            value["places"] = Value::Array(roster.items);
            if roster.truncated {
                value["places_truncated"] = Value::Bool(true);
            }
        }
    }

    Ok(WeekRead {
        value,
        count: 1,
        targets: std::iter::once(format!("week:{start_day}"))
            .chain(windows)
            .collect(),
    })
}

/// Recheck a weekly quote against the admitted activity and its live speech.
/// Stored coordinates only narrow the comparison and are never opened as paths.
struct WeekSpeech<'a> {
    journal: &'a Path,
    snapshot: &'a ConnectionReadSnapshot,
    voices: Option<solstone_core_transcripts::VoiceNames>,
    hides: crate::body_source::BodyHides,
}

impl<'a> WeekSpeech<'a> {
    fn new(journal: &'a Path, snapshot: &'a ConnectionReadSnapshot) -> Self {
        Self {
            journal,
            snapshot,
            voices: solstone_core_talent_runtime::voice_names(journal),
            hides: crate::body_source::BodyHides::read(journal),
        }
    }

    fn shows(&self, entry: &TrustedSaid) -> bool {
        let Some(voices) = self.voices.as_ref() else {
            return false;
        };
        if !self
            .snapshot
            .categories
            .contains(&AdmittedCategory::Transcripts)
            || entry.evidence == SaidEvidence::Unreadable
        {
            return false;
        }
        let Some((facet, day, id)) = parse_said_uri(&entry.uri) else {
            return false;
        };
        let lines = self.activity_speech(facet, day, id, voices);
        let candidates = lines.iter().filter(|line| match &entry.evidence {
            SaidEvidence::Legacy => true,
            SaidEvidence::Sentence { file, sentence_id } => {
                line.file == *file && line.sentence_id == *sentence_id
            }
            SaidEvidence::Unreadable => false,
        });
        if matches!(entry.evidence, SaidEvidence::Sentence { .. })
            && candidates.clone().take(2).count() != 1
        {
            return false;
        }
        solstone_core_transcripts::unique_owner_span(
            &entry.quote,
            candidates.map(|l| l.text.as_str()),
        )
        .is_some()
    }
    fn activity_speech(
        &self,
        facet: &str,
        day: &str,
        id: &str,
        voices: &solstone_core_transcripts::VoiceNames,
    ) -> Vec<solstone_core_transcripts::OwnerVoiceSentence> {
        let Some(activity) =
            crate::may_see::activity_at(self.journal, self.snapshot, facet, day, id, None)
        else {
            return vec![];
        };
        let crate::may_see::SeenRecord::Activity(record) = activity.record else {
            return vec![];
        };
        let mut lines = Vec::new();
        for seg in crate::segment_links::activity_segment_candidates(
            self.journal,
            &activity.place.rel_path,
            &record,
        ) {
            let Ok(identity) = seg.record_identity() else {
                continue;
            };
            let released = || {
                crate::source_release::check_segment_transcript_release(
                    self.journal,
                    day,
                    identity.stream,
                    identity.name,
                )
            };
            if !released()
                || crate::may_see::segment_shown(
                    self.journal,
                    self.snapshot,
                    &self.hides,
                    None,
                    day,
                    identity.stream,
                    identity.name,
                    &seg,
                )
                .is_none()
            {
                continue;
            }
            if let Some(sentences) = solstone_core_transcripts::bounded_owner_voice_sentences(
                self.journal,
                seg.path(),
                voices,
                crate::activity_admit::ACTIVITY_SOURCE_BUDGET as usize,
            ) && released()
            {
                lines.extend(sentences);
            }
        }
        if crate::source_release::check_indexed_source_release(
            self.journal,
            &activity.place.rel_path,
        ) {
            lines
        } else {
            Vec::new()
        }
    }
}

fn format_memory_text(title: &str, start: Option<&str>, end: Option<&str>) -> String {
    match (start, end) {
        (Some(s), Some(e)) => format!("{title} ({s} - {e})"),
        (Some(s), None) => format!("{title} ({s})"),
        _ => title.to_owned(),
    }
}

fn parse_said_uri(uri: &str) -> Option<(&str, &str, &str)> {
    let (facet, rest) = uri
        .strip_prefix("sol://facets/")
        .and_then(|uri| uri.split_once("/activities/"))?;
    let (day, id) = rest.split_once('#')?;
    Some((facet, day, id))
}

fn prefetch_kind_summaries(
    journal: &Path,
    principal: DispatchPrincipal<'_>,
    snapshot: &ConnectionReadSnapshot,
    day_keys: &[String],
    include: &BTreeSet<AdmittedCategory>,
) -> BTreeMap<AdmittedCategory, crate::dispatch::WeekKindSummaries> {
    let mut prefetched = BTreeMap::new();
    let (Some(first_day), Some(last_day)) = (day_keys.first(), day_keys.last()) else {
        return prefetched;
    };
    for &kind in include {
        if matches!(
            kind,
            AdmittedCategory::Body | AdmittedCategory::Places | AdmittedCategory::Entities
        ) || !snapshot.categories.contains(&kind)
        {
            continue;
        }
        prefetched.insert(
            kind,
            crate::dispatch::week_kind_summaries_range(
                journal, principal, snapshot, first_day, last_day, kind,
            ),
        );
    }
    prefetched
}

#[allow(clippy::too_many_arguments)]
fn build_day_summary(
    snapshot: &ConnectionReadSnapshot,
    day: &str,
    include: &BTreeSet<AdmittedCategory>,
    body_days: &mut BTreeMap<String, Value>,
    body_summary_note: Option<&'static str>,
    places: &mut BTreeMap<String, Value>,
    places_note: Option<&'static str>,
    people: BTreeSet<String>,
    kind_summaries: &mut BTreeMap<AdmittedCategory, crate::dispatch::WeekKindSummaries>,
) -> (Map<String, Value>, Vec<String>) {
    let mut summary = Map::new();
    let mut targets = Vec::new();
    for &kind in include {
        let value = if !snapshot.categories.contains(&kind) {
            json!({"note": if kind == AdmittedCategory::Body {
                crate::body_source::BODY_NOT_GRANTED.to_owned()
            } else {
                crate::dispatch::kind_refusal(kind)
            }})
        } else {
            match kind {
                AdmittedCategory::Body => {
                    if let Some(note) = body_summary_note {
                        json!({"note": note})
                    } else {
                        body_days.remove(day).unwrap_or_else(|| json!({}))
                    }
                }
                AdmittedCategory::Places => {
                    if let Some(note) = places_note {
                        json!({"note": note})
                    } else if let Some(day_places) = places.remove(day) {
                        targets.push(format!("location:{day}"));
                        day_places
                    } else {
                        json!({"items": []})
                    }
                }
                AdmittedCategory::Entities => {
                    json!({
                        "count": people.len(),
                        "people": people.iter().take(12).cloned().collect::<Vec<_>>(),
                    })
                }
                _ => {
                    let (value, kind_targets) = kind_summaries
                        .get_mut(&kind)
                        .expect("admitted index-backed summary kind prefetched")
                        .take_day(day);
                    targets.extend(kind_targets);
                    value
                }
            }
        };
        summary.insert(kind.as_str().to_owned(), value);
    }
    (summary, targets)
}

struct DayActivitiesSummary {
    memory: Option<String>,
    activities_total: usize,
    admitted_titles: BTreeMap<(String, String), String>,
    people: BTreeSet<String>,
}

fn summarize_day_activities(
    journal: &Path,
    snapshot: &ConnectionReadSnapshot,
    facets: &[(String, String)],
    day_str: &str,
    collect_entities: bool,
) -> Result<DayActivitiesSummary, ToolError> {
    struct DayCollected {
        record: Map<String, Value>,
        day: String,
        duration: u64,
        sort_key: (String, String, i64, String),
        copies: Vec<Map<String, Value>>,
    }

    let mut collected = Vec::new();
    let mut admitted_titles = BTreeMap::new();
    for (f_id, f_dir) in facets {
        let rel_path = format!("facets/{f_dir}/activities/{day_str}.jsonl");
        let day_file_bytes =
            crate::activity_admit::read_contained_source(journal, &rel_path).unwrap_or_default();
        if day_file_bytes.is_empty() {
            let rows = solstone_core_facets::read_activity_records_strict(journal, f_dir, day_str)
                .map_err(|_| ToolError::FileUnreadable)?;
            if !rows.is_empty() {
                return Err(ToolError::FileUnreadable);
            }
            continue;
        }
        let rows = solstone_core_facets::read_activity_records_strict(journal, f_dir, day_str)
            .map_err(|_| ToolError::FileUnreadable)?;
        let judged = crate::may_see::ActivityDay::new(journal, snapshot, f_dir, day_str, &rows);
        for row in rows {
            if judged.shows(&row) != crate::may_see::Shown::Yes
                || solstone_core_facets::is_body_record(&row)
            {
                continue;
            }
            if let Some(id) = row.get("id").and_then(Value::as_str) {
                admitted_titles.insert(
                    (f_dir.clone(), id.to_owned()),
                    crate::tools::activities::activity_title(&row),
                );
            }
            let (_, _, dur) = crate::tools::activities::activity_times_and_duration(&row);
            let sort_key = crate::tools::activities::sort_activity_key(day_str, f_id, &row);
            collected.push(DayCollected {
                record: row,
                day: day_str.to_owned(),
                duration: dur.unwrap_or(0),
                sort_key,
                copies: Vec::new(),
            });
        }
    }

    collected.sort_by(|a, b| a.sort_key.cmp(&b.sort_key));
    let merged: Vec<DayCollected> = solstone_core_facets::merge_moment_copies(
        collected,
        |item| item.day.as_str(),
        |item| &item.record,
    )
    .into_iter()
    .map(|moment| {
        let mut primary = moment.primary;
        for copy in moment.copies {
            primary.copies.push(copy.record);
        }
        primary
    })
    .collect();

    let mut people = BTreeSet::new();
    if collect_entities {
        for row in &merged {
            let found = crate::tools::activities::activity_people_across(
                std::iter::once(&row.record).chain(row.copies.iter()),
            );
            people.extend(found.participants.into_iter().chain(found.heard));
        }
    }

    let activities_total = merged.len();
    let mut groups: BTreeMap<String, Vec<DayCollected>> = BTreeMap::new();
    for item in merged {
        let k = group_key_for_record(&item.record);
        groups.entry(k).or_default().push(item);
    }

    struct BestActivity {
        duration: u64,
        people_count: usize,
        earliest_sort_key: (String, String, i64, String),
        record: Map<String, Value>,
    }

    let mut best: Option<BestActivity> = None;
    for (_, mut items) in groups {
        let duration: u64 = items.iter().map(|it| it.duration).sum();
        let people_count = items
            .iter()
            .map(|it| {
                let found = crate::tools::activities::activity_people_across(
                    std::iter::once(&it.record).chain(it.copies.iter()),
                );
                crate::tools::activities::activity_entities(&it.record)
                    .len()
                    .max(found.participants.len() + found.heard.len())
            })
            .max()
            .unwrap_or(0);
        items.sort_by(|a, b| a.sort_key.cmp(&b.sort_key));
        let earliest_sort_key = items[0].sort_key.clone();
        let representative = items.remove(0).record;
        let is_better = match &best {
            None => true,
            Some(current) => (duration, people_count)
                .cmp(&(current.duration, current.people_count))
                .then_with(|| current.earliest_sort_key.cmp(&earliest_sort_key))
                .is_gt(),
        };
        if is_better {
            best = Some(BestActivity {
                duration,
                people_count,
                earliest_sort_key,
                record: representative,
            });
        }
    }

    let memory = best.map(|best| {
        let title = crate::tools::activities::activity_title(&best.record);
        let (start, end, _) = crate::tools::activities::activity_times_and_duration(&best.record);
        format_memory_text(&title, start.as_deref(), end.as_deref())
    });

    Ok(DayActivitiesSummary {
        memory,
        activities_total,
        admitted_titles,
        people,
    })
}

#[allow(clippy::too_many_arguments)]
fn project_range_with_limits(
    journal: &Path,
    day_from: &str,
    day_to: &str,
    cursor: Option<&str>,
    principal: DispatchPrincipal<'_>,
    snapshot: &ConnectionReadSnapshot,
    include: &BTreeSet<AdmittedCategory>,
    max_days: usize,
    max_bytes: usize,
) -> Result<WeekRead, ToolError> {
    let (Some(start_date), Some(end_date)) = (week_day(day_from), week_day(day_to)) else {
        return Err(ToolError::InvalidInput);
    };
    let include_tokens: Vec<String> = include
        .iter()
        .map(|kind| category_token(kind).to_owned())
        .collect();

    // Validate the cursor before clamping against today so a tampered,
    // foreign or mismatched cursor is always refused.
    let cursor_next_date = if let Some(token) = cursor {
        let ReferenceTarget::WeekRange(c) = codec()
            .resolve(
                token,
                ReferenceKind::WeekRange,
                principal.connection,
                snapshot.generation,
            )
            .map_err(|_| ToolError::ReferenceNotFound)?
        else {
            return Err(ToolError::ReferenceNotFound);
        };
        if c.day_from != day_from || c.day_to != day_to || c.include != include_tokens {
            return Err(ToolError::ReferenceNotFound);
        }
        let Some(next_date) = week_day(&c.next_day) else {
            return Err(ToolError::ReferenceNotFound);
        };
        if next_date < start_date || next_date > end_date {
            return Err(ToolError::ReferenceNotFound);
        }
        Some(next_date)
    } else {
        None
    };

    let owner_zone = solstone_core_journal_config::owner_zone(journal);
    let today = jiff::Timestamp::now().to_zoned_datetime(owner_zone).date();
    if start_date > today {
        return Ok(WeekRead {
            value: json!({
                "range": {"start": day_from, "end": day_to},
                "days": [],
                "next_cursor": Value::Null,
            }),
            count: 0,
            targets: Vec::new(),
        });
    }

    let effective_end = end_date.min(today);
    let effective_end_day = solstone_core_time::day_key(effective_end);
    let page_start = cursor_next_date.unwrap_or(start_date);
    if page_start > effective_end {
        return Ok(WeekRead {
            value: json!({
                "range": {"start": day_from, "end": effective_end_day},
                "days": [],
                "next_cursor": Value::Null,
            }),
            count: 0,
            targets: Vec::new(),
        });
    }

    let mut candidate_dates = Vec::new();
    let mut current = page_start;
    while current <= effective_end && candidate_dates.len() < max_days {
        candidate_dates.push(current);
        let Ok(next) = current.tomorrow() else {
            break;
        };
        current = next;
    }

    // Read weekly reflection pages once per Sunday touched by this page.
    let left_out = read_left_out(journal);
    let mut day_states: BTreeMap<String, String> = BTreeMap::new();
    let candidate_day_keys: Vec<String> = candidate_dates
        .iter()
        .map(|date| solstone_core_time::day_key(*date))
        .collect();
    let mut said_by_day: BTreeMap<String, Vec<TrustedSaid>> = BTreeMap::new();
    let mut sundays = BTreeSet::new();
    for &date in &candidate_dates {
        sundays.insert(solstone_core_time::day_key(sunday_of(date)));
    }
    let mut speech = None;
    for stem in sundays {
        if let WeekJudgment::Page(trusted) = judge(journal, &stem) {
            let memory_by_id = trusted
                .memories
                .iter()
                .map(|m| (m.id.as_str(), m))
                .collect::<BTreeMap<_, _>>();
            for day in &trusted.days {
                let memory = day
                    .memory_id
                    .as_deref()
                    .and_then(|id| memory_by_id.get(id).copied());
                let memory_key = memory.map(|m| m.key.as_str());
                let state = agent_day_state(day.state.as_str(), memory_key, &left_out);
                day_states.insert(day.day.clone(), state.to_owned());
            }
            if let Said::Entries(said_entries) = trusted.said {
                for entry in said_entries {
                    if candidate_day_keys.binary_search(&entry.day).is_err()
                        || !week_entry_visible(&entry.key, &left_out)
                        || !speech
                            .get_or_insert_with(|| WeekSpeech::new(journal, snapshot))
                            .shows(&entry)
                    {
                        continue;
                    }
                    said_by_day
                        .entry(entry.day.clone())
                        .or_default()
                        .push(entry);
                }
            }
        }
    }

    let facets: Vec<(String, String)> = available_facets(journal, snapshot)
        .map_err(|_| ToolError::FileUnreadable)?
        .into_iter()
        .map(|(id, dir, _)| (id, dir))
        .collect();
    let collect_entities = include.contains(&AdmittedCategory::Entities)
        && snapshot.categories.contains(&AdmittedCategory::Entities);

    let mut body_days = BTreeMap::new();
    let mut body_targets = Vec::new();
    let mut body_summary_note = None;
    if !candidate_day_keys.is_empty()
        && include.contains(&AdmittedCategory::Body)
        && snapshot.categories.contains(&AdmittedCategory::Body)
    {
        let mut body_keys = candidate_day_keys.clone();
        if let Ok(previous) = page_start.yesterday() {
            body_keys.insert(0, solstone_core_time::day_key(previous));
        }
        match crate::body_source::week_totals(journal, snapshot, &body_keys) {
            Ok((totals, targets)) => {
                body_days = totals;
                body_targets = targets;
            }
            Err(_) => body_summary_note = Some(ToolError::FileUnreadable.agent_message()),
        }
    }
    let mut places = BTreeMap::new();
    let mut places_note = None;
    let places_precision = if include.contains(&AdmittedCategory::Places) {
        snapshot.places_precision()
    } else {
        None
    };
    if !candidate_day_keys.is_empty()
        && let Some(precision) = places_precision
    {
        match week_places_summaries(journal, principal, snapshot, &candidate_day_keys, precision) {
            Some(summaries) => places = summaries,
            None => places_note = Some(ToolError::FileUnreadable.agent_message()),
        }
    }
    let places_roster = if cursor.is_none()
        && let Some(precision) = places_precision
    {
        let today_key = solstone_core_time::day_key(today);
        crate::places_view::build_places_roster(
            journal,
            Some(day_from),
            Some(&effective_end_day),
            &today_key,
            precision,
            None,
            None,
        )
    } else {
        None
    };
    let mut kind_summaries =
        prefetch_kind_summaries(journal, principal, snapshot, &candidate_day_keys, include);

    let mut days = Vec::new();
    let mut served_weeks = BTreeSet::new();
    let mut extra_targets = Vec::new();
    let mut page_bytes: usize = places_roster
        .as_ref()
        .and_then(|roster| serde_json::to_string(&roster.items).ok())
        .map_or(0, |s| s.len());
    let mut last_served_day: Option<String> = None;
    let mut resume_date: Option<Date> = None;

    for &date in &candidate_dates {
        let day_str = solstone_core_time::day_key(date);
        let summary_activities =
            summarize_day_activities(journal, snapshot, &facets, &day_str, collect_entities)?;

        let mut cell = json!({"day": day_str});
        if let Some(state) = day_states.get(&day_str) {
            cell["state"] = json!(state);
        }
        // A calendar cell is already admitted. Mint from its day alone:
        // testing source existence here would reveal withheld body days.
        crate::journal_links::attach(
            journal,
            &mut cell,
            solstone_core_journal_link::Place::Day {
                day: day_str.clone(),
            },
        );
        if let Some(mem_text) = summary_activities.memory {
            cell["memory"] = json!(mem_text);
        }
        cell["activities_total"] = json!(summary_activities.activities_total);

        if let Some(entries) = said_by_day.get(&day_str)
            && !entries.is_empty()
        {
            let mut quotes = Vec::with_capacity(entries.len());
            for entry in entries {
                let mut quote = json!({"quote": entry.quote});
                if let Some((facet, qday, id)) = parse_said_uri(&entry.uri) {
                    if qday == day_str
                        && let Some(title) = summary_activities
                            .admitted_titles
                            .get(&(facet.to_owned(), id.to_owned()))
                    {
                        quote["activity"] = json!(title);
                    }
                    crate::journal_links::activity(journal, &mut quote, facet, qday, id);
                }
                quotes.push(quote);
            }
            cell["said"] = Value::Array(quotes);
        }

        let mut day_targets = Vec::new();
        if !include.is_empty() {
            let (summary, kind_targets) = build_day_summary(
                snapshot,
                &day_str,
                include,
                &mut body_days,
                body_summary_note,
                &mut places,
                places_note,
                summary_activities.people,
                &mut kind_summaries,
            );
            day_targets = kind_targets;
            cell["summary"] = Value::Object(summary);
        }

        let row_bytes = serde_json::to_string(&cell).map_or(0, |s| s.len()) + 1;
        if !days.is_empty() && page_bytes.saturating_add(row_bytes) > max_bytes {
            resume_date = Some(date);
            break;
        }
        page_bytes = page_bytes.saturating_add(row_bytes);
        last_served_day = Some(day_str);
        served_weeks.insert(solstone_core_time::day_key(sunday_of(date)));
        extra_targets.extend(day_targets);
        days.push(cell);
    }

    if resume_date.is_none()
        && let Some(&last_date) = candidate_dates.last()
        && last_date < effective_end
    {
        resume_date = last_date.tomorrow().ok().filter(|d| d <= &effective_end);
    }

    if let Some(ref last) = last_served_day {
        for target in body_targets {
            if target.split('/').next().is_some_and(|d| d <= last.as_str()) {
                extra_targets.push(target);
            }
        }
    }

    let next_cursor = match resume_date {
        Some(next_date) => Some(
            codec()
                .mint(
                    principal.connection,
                    snapshot.generation,
                    ReferenceTarget::WeekRange(WeekRangeCursor {
                        day_from: day_from.to_owned(),
                        day_to: day_to.to_owned(),
                        include: include_tokens,
                        next_day: solstone_core_time::day_key(next_date),
                    }),
                )
                .map_err(|_| ToolError::ReferenceNotFound)?,
        ),
        None => None,
    };

    let targets = served_weeks
        .into_iter()
        .map(|stem| format!("week:{stem}"))
        .chain(extra_targets)
        .collect();
    let count = days.len();
    let mut value = json!({
        "range": {"start": day_from, "end": effective_end_day},
        "days": days,
        "next_cursor": next_cursor,
    });
    if let Some(precision) = places_precision {
        value["places_precision"] = json!(precision.wire_value());
    }
    if let Some(roster) = places_roster {
        value["places"] = Value::Array(roster.items);
        if roster.truncated {
            value["places_truncated"] = Value::Bool(true);
        }
    }

    Ok(WeekRead {
        value,
        count,
        targets,
    })
}

fn week_places_summaries(
    journal: &Path,
    principal: DispatchPrincipal<'_>,
    snapshot: &ConnectionReadSnapshot,
    days: &[String],
    precision: solstone_core_format::content::kinds::PlacesPrecision,
) -> Option<BTreeMap<String, Value>> {
    let (store, views) = admitted_locations(journal, days)?;
    let mut out = BTreeMap::new();
    for day in views {
        let day_key = day.day.clone();
        let reference = codec()
            .mint(
                principal.connection,
                snapshot.generation,
                ReferenceTarget::LocationDay(LocationDayReference {
                    day: day_key.clone(),
                }),
            )
            .ok();
        if let Some(summary) =
            crate::places_view::build_week_day_places(&store, &day, precision, reference)
        {
            out.insert(day_key, summary);
        }
    }
    Some(out)
}

/// Apply the map's live hide rules rather than serving a stale indexed place.
fn admitted_locations(
    journal: &Path,
    dates: &[String],
) -> Option<(
    solstone_core_location::places::PlaceStore,
    Vec<solstone_core_location::day::DayLocation>,
)> {
    use solstone_core_location::{day::Layout, places::PlaceStore};
    let store = PlaceStore::read_strict(journal).ok()?;
    let mut days = solstone_core_location::read_journal_days(journal, dates).ok()?;
    let mut admitted = BTreeMap::new();
    for day in &mut days {
        day.entries.retain(|entry| {
            let sources = entry.sources();
            !sources.is_empty()
                && sources.iter().all(|source| {
                    let key = (
                        source.day.clone(),
                        source.stream.clone(),
                        source.segment.clone(),
                    );
                    *admitted.entry(key).or_insert_with(|| {
                        let path = match source.layout {
                            Layout::Direct => format!(
                                "chronicle/{}/{}/location.jsonl",
                                source.day, source.segment
                            ),
                            Layout::Named => format!(
                                "chronicle/{}/{}/{}/location.jsonl",
                                source.day, source.stream, source.segment
                            ),
                        };
                        crate::source_release::check_segment_transcript_release(
                            journal,
                            &source.day,
                            &source.stream,
                            &source.segment,
                        ) && crate::activity_admit::read_contained_source(journal, &path).is_some()
                    })
                })
        });
    }
    days.retain(|day| !day.entries.is_empty());
    Some((store, days))
}

fn location_documents(journal: &Path, days: &[String]) -> Option<BTreeMap<String, String>> {
    let (store, days) = admitted_locations(journal, days)?;
    Some(
        days.into_iter()
            .map(|day| {
                let text = solstone_core_location::projection::render_day_document(&day, &store);
                (day.day, text)
            })
            .collect(),
    )
}

pub(crate) fn location_document(journal: &Path, day: &str) -> Option<String> {
    location_documents(journal, &[day.to_owned()])?.remove(day)
}

pub(crate) fn activity_locations(
    journal: &Path,
    day: &str,
) -> Option<(
    solstone_core_location::places::PlaceStore,
    solstone_core_location::day::DayLocation,
)> {
    let (store, mut views) = admitted_locations(journal, &[day.to_owned()])?;
    Some((store, views.pop()?))
}

/// A week's rows are keyed on the name each activity is shown under, so the
/// same name on several days is one recurring row and two different meetings
/// are two rows.
fn group_key_for_record(record: &Map<String, Value>) -> String {
    crate::tools::activities::activity_title(record)
        .trim()
        .to_lowercase()
}

#[cfg(all(test, not(feature = "full-tests")))]
mod tests {
    use std::collections::BTreeSet;
    use std::fs;
    use std::path::Path;

    use serde_json::{Value, json};
    use sha2::{Digest, Sha256};
    use solstone_core_format::content::week_left_out::left_out_bytes;
    use solstone_core_home::weekly::page_model;
    use solstone_core_mcp_audit::ToolName;

    use super::validate;
    use crate::activity::{ActivityQuery, RecordedOutcome, read_activity};
    use crate::dispatch::{
        DispatchError, DispatchPrincipal, McpProbeError, dispatch_authenticated_tool_call,
        run_mcp_probe,
    };
    use crate::jsonrpc::{ToolName as JsonTool, tool_result};
    use crate::permissions::{
        PermissionStore, ReadPermission, ReadScope, evaluate_connection_read,
    };
    use crate::registry::advertised_tools_list;
    use crate::tools::ToolError;

    const CONNECTION: &str = "bearer:week";
    const WHOLE_JOURNAL: &str =
        "this connection can read a week or date range only with access to the whole journal.";

    fn journal(prefix: &str) -> tempfile::TempDir {
        tempfile::Builder::new()
            .prefix(prefix)
            .tempdir_in(crate::test_scratch())
            .unwrap()
    }

    fn grant(root: &Path, categories: &[&str], scope: ReadScope) {
        PermissionStore::open(root)
            .set_permission(
                CONNECTION,
                ReadPermission {
                    categories: categories
                        .iter()
                        .map(|category| (*category).to_owned())
                        .collect(),
                    scope,
                },
            )
            .unwrap();
    }

    fn owner_speech(root: &Path, day: &str, segment: &str, text: &str) {
        write_raw(
            root,
            "entities/owner/entity.json",
            &json!({"id":"owner","type":"Person","name":"Ari","is_principal":true}).to_string(),
        );
        let dir = format!("chronicle/{day}/mic/{segment}");
        write_raw(
            root,
            &format!("{dir}/audio.jsonl"),
            &json!({"sentence_id":1,"start":"00:00:01","speaker":1,"text":text}).to_string(),
        );
        write_raw(
            root,
            &format!("{dir}/talents/speaker_labels.json"),
            &json!({"labels":[{"sentence_id":1,"speaker":"owner","method":"user_confirmed"}]})
                .to_string(),
        );
    }

    fn whole(root: &Path) {
        grant(root, &["transcripts", "facets"], ReadScope::WholeJournal);
    }

    fn probe(root: &Path, arguments: Value) -> Result<Value, McpProbeError> {
        run_mcp_probe(root, CONNECTION, "get_week", &arguments)
    }

    fn dispatch_week(
        root: &Path,
        arguments: Value,
    ) -> Result<crate::dispatch::ToolOutput, DispatchError> {
        dispatch_authenticated_tool_call(
            root,
            DispatchPrincipal {
                connection: CONNECTION,
                agent_identity: CONNECTION,
            },
            JsonTool::GetWeek,
            Some(&arguments),
            jiff::Timestamp::now(),
        )
    }

    fn page(root: &Path) -> crate::activity::ActivityPage {
        read_activity(
            root,
            &ActivityQuery {
                limit: 10,
                ..ActivityQuery::default()
            },
        )
        .unwrap()
    }

    fn names(root: &Path) -> Vec<String> {
        let decision = evaluate_connection_read(root, CONNECTION);
        advertised_tools_list(root, &decision)["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| {
                assert_eq!(
                    tool["annotations"]["readOnlyHint"],
                    tool["name"] != "save_memory"
                );
                tool["name"].as_str().unwrap().to_owned()
            })
            .collect()
    }

    fn shift(sunday: &str, days: i64) -> String {
        let date = solstone_core_time::parse_day(sunday).unwrap();
        solstone_core_time::day_key(date + jiff::Span::new().days(days))
    }

    fn write_raw(root: &Path, relative: &str, body: &str) {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, body).unwrap();
    }

    #[derive(Clone, Copy)]
    struct Memory<'a> {
        id: &'a str,
        key: &'a str,
        day_offset: i64,
        text: &'a str,
        uri: &'a str,
        refs: &'a [&'a str],
    }

    fn document(sunday: &str, memories: &[Memory<'_>], said: &str) -> String {
        let end = shift(sunday, 6);
        let days = (0..7)
            .map(|offset| {
                let day = shift(sunday, offset);
                memories
                    .iter()
                    .find(|memory| memory.day_offset == offset)
                    .map(|memory| {
                        format!(
                            r#"{{"day":"{day}","state":"memory","memory_id":"{}"}}"#,
                            memory.id
                        )
                    })
                    .unwrap_or_else(|| format!(r#"{{"day":"{day}","state":"nothing_shared"}}"#))
            })
            .collect::<Vec<_>>()
            .join(",");
        let memories_json = memories
            .iter()
            .map(|memory| {
                let day = shift(sunday, memory.day_offset);
                let refs = memory
                    .refs
                    .iter()
                    .map(|reference| format!("\"{reference}\""))
                    .collect::<Vec<_>>()
                    .join(",");
                format!(
                    r#"{{"id":"{}","key":"{}","day":"{day}","text":"{}","source":{{"kind":"briefing","uri":"{}","briefing_day":"{day}","refs":[{refs}]}}}}"#,
                    memory.id, memory.key, memory.text, memory.uri
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        format!(
            r#"{{"version":1,"week":{{"start":"{sunday}","end":"{end}"}},"days":[{days}],"memories":[{memories_json}],"said":{said}}}"#
        )
    }

    fn said_item(id: &str, key: &str, day: &str, quote: &str, uri: &str) -> String {
        format!(
            r#"{{"id":"{id}","key":"{key}","day":"{day}","quote":"{quote}","source":{{"kind":"activity","uri":"{uri}"}}}}"#
        )
    }

    fn said_key(day: &str, facet: &str, record_id: &str, quote: &str) -> String {
        let hex = format!(
            "{:x}",
            Sha256::digest(format!("said\n{day}\n{facet}\n{record_id}\n{quote}").as_bytes())
        );
        hex[..16].to_owned()
    }

    fn write_left_out(root: &Path, keys: &[&str]) {
        let keys = keys
            .iter()
            .map(|key| (*key).to_owned())
            .collect::<BTreeSet<_>>();
        write_raw(
            root,
            "health/week-left-out.json",
            &String::from_utf8(left_out_bytes(&keys)).unwrap(),
        );
    }

    fn assert_closed(value: &Value) {
        let keys = value
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>();
        assert_eq!(
            keys,
            BTreeSet::from([
                "activities".to_owned(),
                "activities_total".to_owned(),
                "days".to_owned(),
                "intro".to_owned(),
                "memories".to_owned(),
                "next".to_owned(),
                "previous".to_owned(),
                "said".to_owned(),
                "week".to_owned(),
            ])
        );
        let week_keys = value["week"]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>();
        assert_eq!(
            week_keys,
            BTreeSet::from(["end".to_owned(), "start".to_owned()])
        );
        for day in value["days"].as_array().unwrap() {
            let mut day = day.clone();
            day.as_object_mut().unwrap().remove("link");
            assert_eq!(
                day.as_object()
                    .unwrap()
                    .keys()
                    .cloned()
                    .collect::<BTreeSet<_>>(),
                BTreeSet::from(["day".to_owned(), "state".to_owned()])
            );
        }
        for memory in value["memories"].as_array().unwrap() {
            assert_eq!(
                memory
                    .as_object()
                    .unwrap()
                    .keys()
                    .cloned()
                    .collect::<BTreeSet<_>>(),
                BTreeSet::from([
                    "day".to_owned(),
                    "refs".to_owned(),
                    "source".to_owned(),
                    "text".to_owned(),
                ])
            );
            assert!(memory["source"].is_string());
            assert!(memory["refs"].is_array());
        }
        for quote in value["said"].as_array().unwrap() {
            let mut quote = quote.clone();
            quote.as_object_mut().unwrap().remove("link");
            assert_eq!(
                quote
                    .as_object()
                    .unwrap()
                    .keys()
                    .cloned()
                    .collect::<BTreeSet<_>>(),
                BTreeSet::from(["day".to_owned(), "quote".to_owned(), "uri".to_owned()])
            );
        }
    }

    fn assert_matches_page(root: &Path, sunday: &str, value: &Value) {
        let model = page_model(root, sunday, 2026, &|_| false).unwrap();
        let cells = model["cells"].as_array().unwrap();
        let days = value["days"].as_array().unwrap();
        assert_eq!(days.len(), 7);
        for (cell, day) in cells.iter().zip(days) {
            assert_eq!(day["day"], cell["day"]);
            assert_eq!(day["state"], cell["state"]);
        }
        assert_eq!(value["previous"].as_str(), href_stem(&model["prev_href"]));
        assert_eq!(value["next"].as_str(), href_stem(&model["next_href"]));
    }

    fn href_stem(href: &Value) -> Option<&str> {
        href.as_str().and_then(|href| href.rsplit('/').next())
    }

    fn one_memory<'a>(text: &'a str, uri: &'a str) -> Memory<'a> {
        Memory {
            id: "m0",
            key: "memory-key",
            day_offset: 0,
            text,
            uri,
            refs: &["sol://chronicle/20260308/room/120000_1"],
        }
    }

    #[test]
    fn a_week_of_one_activity_type_keeps_each_activity() {
        let root = journal("solstone-week-one-type-");
        whole(root.path());
        write_raw(
            root.path(),
            "facets/work/facet.json",
            &json!({"id": "123e4567-e89b-42d3-a456-426614174000", "title": "Work", "description": "Work"}).to_string(),
        );
        let meeting = |id: &str, title: &str, seconds: u32| {
            format!(
                "{}\n",
                json!({
                    "id": id,
                    "activity": "meeting",
                    "title": title,
                    "created_at": 100,
                    "segments": [format!("090000_{seconds}")],
                })
            )
        };
        write_raw(
            root.path(),
            "facets/work/activities/20260308.jsonl",
            &(meeting("kickoff", "Launch kickoff", 300) + &meeting("review", "Design review", 200)),
        );
        write_raw(
            root.path(),
            "facets/work/activities/20260309.jsonl",
            &meeting("standup_1", "Team standup", 100),
        );
        write_raw(
            root.path(),
            "facets/work/activities/20260310.jsonl",
            &meeting("standup_2", " team STANDUP ", 100),
        );

        let value = probe(root.path(), json!({"week": "20260308"})).unwrap();
        let titles = value["activities"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["title"].as_str().unwrap())
            .collect::<Vec<_>>();
        // The same name on two days is one row, and it ranks first for recurring.
        assert_eq!(titles, ["Team standup", "Launch kickoff", "Design review"]);
        assert_eq!(value["activities_total"], 4);
        assert_eq!(value["memories"].as_array().unwrap().len(), 3);
        // Two of the three rows share a day, so they come from two days, not three.
        assert_eq!(value["intro"], "your week, a memory from each of 2 days.");
        // This week has no page, so it has no day states and no quotes, even
        // though three of its days hold activities.
        assert_eq!(value["days"], json!([]));
        assert_eq!(value["said"], json!([]));
        assert!(value["previous"].is_null());
        assert!(value["next"].is_null());
    }

    #[test]
    fn day_links_do_not_disclose_a_body_only_day_without_body_permission() {
        let root = journal("solstone-week-day-links-");
        whole(root.path());
        let days: Vec<_> = (8..=14)
            .map(|day| json!({"day": format!("202603{day:02}"), "state": "nothing_shared"}))
            .collect();
        write_raw(
            root.path(),
            "reflections/weekly/20260308.json",
            &json!({
                "version": 1, "week": {"start":"20260308", "end":"20260314"},
                "days": days, "memories": [], "said": [],
            })
            .to_string(),
        );
        let before = probe(root.path(), json!({"week":"20260308"})).unwrap();
        write_raw(
            root.path(),
            "chronicle/20260310/import.apple_health/090000_300/body.jsonl",
            "{}\n",
        );
        let after = probe(root.path(), json!({"week":"20260308"})).unwrap();
        assert_eq!(
            before["days"], after["days"],
            "a withheld source changes no day link"
        );
        for row in before["days"].as_array().unwrap() {
            let sealed = row["link"].as_str().unwrap().split_once('#').unwrap().1;
            assert_eq!(
                solstone_core_journal_link::open(root.path(), sealed),
                Ok(solstone_core_journal_link::Place::Day {
                    day: row["day"].as_str().unwrap().into()
                })
            );
        }
    }

    #[test]
    fn a_page_keeps_its_day_states_and_quotes() {
        let root = journal("solstone-week-page-states-");
        whole(root.path());
        let sunday = "20260308";
        write_raw(
            root.path(),
            "facets/work/facet.json",
            &json!({"id": "123e4567-e89b-42d3-a456-426614174000", "title": "Work", "description": "Work"}).to_string(),
        );
        write_raw(
            root.path(),
            "facets/work/activities/20260309.jsonl",
            &format!(
                "{}\n",
                json!({"id": "standup", "activity": "meeting", "title": "Team standup",
                    "created_at": 100, "segments": ["090000_300"]})
            ),
        );
        owner_speech(root.path(), "20260309", "090000_300", "the owner's words");
        let states = [
            r#"{"day":"20260308","state":"memory","memory_id":"m0"}"#,
            r#"{"day":"20260309","state":"not_ready"}"#,
            r#"{"day":"20260310","state":"nothing_shared"}"#,
            r#"{"day":"20260311","state":"unreadable"}"#,
            r#"{"day":"20260312","state":"not_ready"}"#,
            r#"{"day":"20260313","state":"nothing_shared"}"#,
            r#"{"day":"20260314","state":"nothing_shared"}"#,
        ]
        .join(",");
        let quote = said_item(
            "s0",
            "quote-key",
            "20260309",
            "the owner's words",
            "sol://facets/work/activities/20260309#standup",
        );
        let memory_json = r#"{"id":"m0","key":"memory-key","day":"20260308","text":"a memory","source":{"kind":"briefing","uri":"sol://chronicle/20260308/talents/morning_briefing","briefing_day":"20260308","refs":["sol://chronicle/20260308/room/120000_1"]}}"#;
        write_raw(
            root.path(),
            "reflections/weekly/20260308.json",
            &format!(
                r#"{{"version":1,"week":{{"start":"{sunday}","end":"20260314"}},"days":[{states}],"memories":[{memory_json}],"said":[{quote}]}}"#
            ),
        );

        let value = probe(root.path(), json!({"week": sunday})).unwrap();
        assert_closed(&value);
        // Day links name calendar cells, independent of their state or sources.
        let opens = |row: &Value| {
            solstone_core_journal_link::open(
                root.path(),
                row["link"].as_str().unwrap().split_once('#').unwrap().1,
            )
            .unwrap()
        };
        assert_eq!(
            opens(&value["days"][1]),
            solstone_core_journal_link::Place::Day {
                day: "20260309".into()
            }
        );
        assert_eq!(
            opens(&value["said"][0]),
            solstone_core_journal_link::Place::Activity {
                facet: "work".into(),
                day: "20260309".into(),
                id: "standup".into(),
            }
        );
        let mut days = value["days"].clone();
        for day in days.as_array_mut().unwrap() {
            day.as_object_mut().unwrap().remove("link");
        }
        assert_eq!(
            days,
            json!([
                {"day": "20260308", "state": "memory"},
                {"day": "20260309", "state": "not_ready"},
                {"day": "20260310", "state": "nothing_shared"},
                {"day": "20260311", "state": "unreadable"},
                {"day": "20260312", "state": "not_ready"},
                {"day": "20260313", "state": "nothing_shared"},
                {"day": "20260314", "state": "nothing_shared"},
            ])
        );
        let mut said = value["said"].clone();
        for quote in said.as_array_mut().unwrap() {
            quote.as_object_mut().unwrap().remove("link");
        }
        assert_eq!(
            said,
            json!([{
                "day": "20260309",
                "quote": "the owner's words",
                "uri": "sol://facets/work/activities/20260309#standup",
            }])
        );
        assert_matches_page(root.path(), sunday, &value);
        assert_eq!(value["activities_total"], 1);
    }

    fn folded_week(root: &Path, held: bool) -> Value {
        grant(
            root,
            &["transcripts", "facets", "body"],
            ReadScope::WholeJournal,
        );
        write_raw(
            root,
            "facets/work/facet.json",
            &json!({"id": "123e4567-e89b-42d3-a456-426614174000", "title": "Work", "description": "Work"}).to_string(),
        );
        let key = "20260308|work|run_week";
        let lines = if held {
            json!({key: {"sha256": "x", "hidden": false, "fields": {}}})
        } else {
            json!({})
        };
        let line = json!({"id": "run_week", "activity": "exercise", "title": "Morning Run",
            "created_at": 100, "stream": "import.strava", "segments": ["090000_300"]});
        let code = json!({"id": "body_run", "source": "body", "activity": "workout",
            "account": {"key": "import.strava\u{1f}1"},
            "converted": {"primary": key, "decided_at": "t", "lines": lines}});
        write_raw(
            root,
            "facets/work/activities/20260308.jsonl",
            &format!("{line}\n{code}\n"),
        );
        probe(root, json!({"week": "20260308"})).unwrap()
    }

    #[test]
    fn a_strava_line_a_code_record_holds_isnt_a_row() {
        let open = journal("solstone-week-unfolded-");
        assert_eq!(folded_week(open.path(), false)["activities_total"], 1);
        // An unconverted line is body data: without body it isn't a row, and
        // the week says why.
        whole(open.path());
        let value = probe(open.path(), json!({"week": "20260308"})).unwrap();
        assert_eq!(value["activities_total"], 0);
        assert_eq!(value["body_note"], crate::body_source::BODY_NOT_GRANTED);
        let held = journal("solstone-week-folded-");
        let value = folded_week(held.path(), true);
        assert_eq!(value["activities"], json!([]));
        assert_eq!(value["activities_total"], 0);
    }

    #[test]
    fn a_strava_line_a_code_record_holds_doesnt_make_a_later_week_familiar() {
        let root = journal("solstone-week-prior-folded-");
        whole(root.path());
        write_raw(
            root.path(),
            "facets/work/facet.json",
            &json!({"id": "123e4567-e89b-42d3-a456-426614174000", "title": "Work", "description": "Work"}).to_string(),
        );
        let key = "20260301|work|run_prior";
        let line = json!({"id": "run_prior", "activity": "exercise", "title": "Morning Run",
            "created_at": 50, "stream": "import.strava", "segments": ["090000_300"]});
        let code = json!({"id": "body_prior", "source": "body", "activity": "workout",
            "account": {"key": "import.strava\u{1f}2"},
            "converted": {"primary": key, "decided_at": "t",
                "lines": {key: {"sha256": "x", "hidden": false, "fields": {}}}}});
        write_raw(
            root.path(),
            "facets/work/activities/20260301.jsonl",
            &format!("{line}\n{code}\n"),
        );
        let row = |id: &str, title: &str, segment: &str| {
            format!(
                "{}\n",
                json!({"id": id, "activity": "meeting", "title": title, "created_at": 100,
                    "segments": [segment]})
            )
        };
        write_raw(
            root.path(),
            "facets/work/activities/20260308.jsonl",
            &(row("first", "Morning Run", "090000_100") + &row("second", "Kickoff", "100000_100")),
        );
        let value = probe(root.path(), json!({"week": "20260308"})).unwrap();
        let titles = value["activities"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["title"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(titles, ["Morning Run", "Kickoff"]);
    }

    #[test]
    fn a_full_week_says_how_many_it_left_out_of_the_rows() {
        let root = journal("solstone-week-capped-");
        whole(root.path());
        write_raw(
            root.path(),
            "facets/work/facet.json",
            &json!({"id": "123e4567-e89b-42d3-a456-426614174000", "title": "Work", "description": "Work"}).to_string(),
        );
        let lines = (1..=9)
            .map(|n| {
                format!(
                    "{}\n",
                    json!({
                        "id": format!("meeting_{n}"),
                        "activity": "meeting",
                        "title": format!("Meeting {n}"),
                        "created_at": 100,
                        "segments": [format!("09000{n}_{}", n * 100)],
                    })
                )
            })
            .collect::<String>();
        write_raw(root.path(), "facets/work/activities/20260308.jsonl", &lines);

        let value = probe(root.path(), json!({"week": "20260308"})).unwrap();
        assert_eq!(value["activities"].as_array().unwrap().len(), 7);
        assert_eq!(value["activities_total"], 9);
    }

    fn meeting_line(id: &str, title: &str, seconds: u32) -> String {
        format!(
            "{}\n",
            json!({
                "id": id,
                "activity": "meeting",
                "title": title,
                "created_at": 100,
                "segments": [format!("090000_{seconds}")],
            })
        )
    }

    #[test]
    fn a_busy_day_does_not_take_every_row() {
        let root = journal("solstone-week-busy-");
        whole(root.path());
        write_raw(
            root.path(),
            "facets/work/facet.json",
            &json!({"id": "123e4567-e89b-42d3-a456-426614174000", "title": "Work", "description": "Work"}).to_string(),
        );
        let mut busy = String::new();
        for n in 0..8 {
            busy.push_str(&meeting_line(
                &format!("long_{n}"),
                &format!("Long meeting {n}"),
                1000 - n * 100,
            ));
        }
        write_raw(root.path(), "facets/work/activities/20260311.jsonl", &busy);
        for (day, title) in [
            ("20260308", "Sunday errand"),
            ("20260309", "Monday errand"),
            ("20260310", "Tuesday errand"),
            ("20260312", "Thursday errand"),
            ("20260313", "Friday errand"),
            ("20260314", "Saturday errand"),
        ] {
            write_raw(
                root.path(),
                &format!("facets/work/activities/{day}.jsonl"),
                &meeting_line(&format!("errand_{day}"), title, 60),
            );
        }

        let value = probe(root.path(), json!({"week": "20260308"})).unwrap();
        let rows = value["activities"].as_array().unwrap();
        assert_eq!(rows.len(), 7);
        assert_eq!(value["activities_total"], 14);
        let days = rows
            .iter()
            .map(|row| row["day"].as_str().unwrap())
            .collect::<BTreeSet<_>>();
        assert_eq!(days.len(), 7);
        assert_eq!(
            rows.iter().filter(|row| row["day"] == "20260311").count(),
            1
        );
        assert!(rows.iter().any(|row| row["title"] == "Long meeting 0"));
        assert_eq!(value["intro"], "your week, a memory from each of 7 days.");
    }

    #[test]
    fn a_recurring_activity_keeps_its_place_when_days_spread() {
        let root = journal("solstone-week-dominant-");
        whole(root.path());
        write_raw(
            root.path(),
            "facets/work/facet.json",
            &json!({"id": "123e4567-e89b-42d3-a456-426614174000", "title": "Work", "description": "Work"}).to_string(),
        );
        for offset in 0..7 {
            let day = shift("20260308", offset);
            let lines = meeting_line("rehearsal", "The rehearsal", 2000)
                + &meeting_line(&format!("errand_{day}"), &format!("Errand {day}"), 60);
            write_raw(
                root.path(),
                &format!("facets/work/activities/{day}.jsonl"),
                &lines,
            );
        }

        let value = probe(root.path(), json!({"week": "20260308"})).unwrap();
        let rows = value["activities"].as_array().unwrap();
        let titles = rows
            .iter()
            .map(|row| row["title"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(titles[0], "The rehearsal");
        assert_eq!(rows[0]["day"], "20260308");
        assert_eq!(titles.len(), 7);
        assert_eq!(value["activities_total"], 14);
        assert!(!titles.contains(&"Errand 20260308"));
        assert_eq!(
            titles
                .iter()
                .filter(|title| title.starts_with("Errand"))
                .count(),
            6
        );
        let days = rows
            .iter()
            .map(|row| row["day"].as_str().unwrap())
            .collect::<BTreeSet<_>>();
        assert_eq!(days.len(), 7);
    }

    #[test]
    fn whole_journal_grant_advertises_and_calls_get_week() {
        let root = journal("solstone-week-advertised-");
        whole(root.path());
        let decision = evaluate_connection_read(root.path(), CONNECTION);
        let listed = advertised_tools_list(root.path(), &decision);
        let tool = listed["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|tool| tool["name"] == "get_week")
            .unwrap();
        assert_eq!(tool["annotations"]["readOnlyHint"], true);
        write_raw(
            root.path(),
            "reflections/weekly/20260308.md",
            "# only markdown",
        );
        let value = probe(root.path(), json!({})).unwrap();
        assert!(value["week"]["start"].is_string());
        assert_eq!(value["activities"], json!([]));
        // Any day names the week it falls in.
        assert_eq!(
            probe(root.path(), json!({"week": "20260309"})).unwrap()["week"]["start"],
            "20260308"
        );
    }

    #[test]
    fn missing_categories_keep_the_existing_refusal() {
        let cases = [
            (
                &["facets"][..],
                "this connection does not have the transcripts category",
            ),
            (
                &["transcripts"][..],
                "this connection does not have the facets category",
            ),
            (
                &["entities"][..],
                "this connection does not have the transcripts and facets category",
            ),
        ];
        for (categories, reason) in cases {
            let root = journal("solstone-week-missing-");
            grant(root.path(), categories, ReadScope::WholeJournal);
            assert!(!names(root.path()).iter().any(|name| name == "get_week"));
            assert_eq!(
                probe(root.path(), json!({})),
                Err(McpProbeError::PermissionDenied)
            );
            let entry = &page(root.path()).entries[0];
            assert_eq!(entry.outcome, RecordedOutcome::Refused);
            assert_eq!(entry.tool_name, ToolName::GetWeek);
            assert_eq!(entry.reason.as_deref(), Some(reason));
        }
    }

    #[test]
    fn category_failure_wins_over_a_chosen_facet_grant() {
        let root = journal("solstone-week-category-wins-");
        grant(
            root.path(),
            &["entities"],
            ReadScope::Facets {
                ids: vec!["facet-a".to_owned(), "facet-b".to_owned()],
            },
        );
        assert_eq!(
            probe(root.path(), json!({})),
            Err(McpProbeError::PermissionDenied)
        );
        assert_eq!(
            page(root.path()).entries[0].reason.as_deref(),
            Some("this connection does not have the transcripts and facets category")
        );
    }

    #[test]
    fn chosen_facets_do_not_see_a_week_even_with_every_facet() {
        let root = journal("solstone-week-chosen-");
        for (name, id) in [("alpha", "facet-a"), ("beta", "facet-b")] {
            write_raw(
                root.path(),
                &format!("facets/{name}/facet.json"),
                &json!({"id": id, "title": name, "description": name}).to_string(),
            );
        }
        let mut ids = solstone_core_facets::list_declared_facet_names(root.path())
            .unwrap()
            .into_iter()
            .map(|name| {
                solstone_core_facets::read_facet_declaration(root.path(), &name)
                    .unwrap()
                    .unwrap()
                    .value()["id"]
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .collect::<Vec<_>>();
        ids.sort();
        assert_eq!(ids, vec!["facet-a".to_owned(), "facet-b".to_owned()]);
        grant(
            root.path(),
            &["transcripts", "facets"],
            ReadScope::Facets { ids },
        );
        assert!(!names(root.path()).iter().any(|name| name == "get_week"));
        assert_eq!(
            probe(root.path(), json!({})),
            Err(McpProbeError::PermissionDenied)
        );
        let reason = page(root.path()).entries[0].reason.clone().unwrap();
        assert_eq!(reason, WHOLE_JOURNAL);
        assert!(!reason.contains("does not have the"));
    }

    #[test]
    fn revoked_and_absent_grants_stay_on_the_denied_path() {
        let absent = journal("solstone-week-absent-grant-");
        assert!(!names(absent.path()).iter().any(|name| name == "get_week"));
        assert_eq!(
            probe(absent.path(), json!({})),
            Err(McpProbeError::PermissionDenied)
        );
        let absent_entry = &page(absent.path()).entries[0];
        assert_eq!(absent_entry.outcome, RecordedOutcome::Refused);
        assert_eq!(
            absent_entry.reason.as_deref(),
            Some("this connection has no read permission")
        );

        let revoked = journal("solstone-week-revoked-");
        whole(revoked.path());
        PermissionStore::open(revoked.path())
            .clear_permission(CONNECTION)
            .unwrap();
        assert!(!names(revoked.path()).iter().any(|name| name == "get_week"));
        assert_eq!(
            probe(revoked.path(), json!({})),
            Err(McpProbeError::PermissionDenied)
        );
        assert_eq!(
            page(revoked.path()).entries[0].reason.as_deref(),
            Some("this connection has no read permission")
        );
    }

    #[test]
    fn invalid_week_shapes_fail_before_any_journal_lookup() {
        let missing =
            crate::test_scratch().join(format!("solstone-week-no-journal-{}", std::process::id()));
        let _ = fs::remove_dir_all(&missing);
        assert!(!missing.exists());
        for week in [
            "2026/03/08",
            "..",
            r"C:\20260308",
            "20260308/../..",
            "20260231",
        ] {
            assert_eq!(
                validate(Some(&json!({"week": week}))),
                Err(ToolError::InvalidInput),
                "{week}"
            );
            assert_eq!(
                run_mcp_probe(&missing, CONNECTION, "get_week", &json!({"week": week})),
                Err(McpProbeError::InvalidInput),
                "{week}"
            );
        }
        assert!(!missing.exists());
    }

    #[test]
    fn absent_weeks_are_empty_answers() {
        let cases = [
            ("newest-missing", None, false),
            ("sunday-missing-dir", Some("20260308"), false),
            ("sunday-missing-file", Some("20260308"), true),
            ("markdown-only", None, true),
        ];
        for (name, week, create_weekly) in cases {
            let root = journal(&format!("solstone-week-empty-{name}-"));
            whole(root.path());
            if create_weekly {
                write_raw(
                    root.path(),
                    "reflections/weekly/20260308.md",
                    "# pre-format",
                );
            }
            let arguments = match week {
                Some(day) => json!({"week": day}),
                None => json!({}),
            };
            let value = probe(root.path(), arguments.clone()).unwrap();
            assert!(value["week"]["start"].is_string());
            assert_eq!(value["activities"], json!([]));
            assert_eq!(value["memories"], json!([]));
            assert_eq!(value["said"], json!([]));
            let output = dispatch_week(root.path(), arguments).unwrap();
            assert!(output.value["week"]["start"].is_string());
            assert!(output.empty_note.is_none());
        }
        // A day inside a week, in either form a day is written, reads that week.
        let mon_root = journal("solstone-week-empty-mon-");
        whole(mon_root.path());
        for day in ["20260309", "2026-03-09"] {
            let mon_out = dispatch_week(mon_root.path(), json!({"week": day})).unwrap();
            assert_eq!(mon_out.value["week"]["start"], "20260308", "{day}");
            assert!(mon_out.empty_note.is_none());
        }
        assert_eq!(
            validate(Some(&json!({"week": "2026-03-08"})))
                .unwrap()
                .query,
            super::WeekQuery::Single(Some("20260308".to_owned()))
        );

        assert_eq!(
            validate(None).unwrap().query,
            super::WeekQuery::Single(None)
        );
        assert_eq!(
            validate(Some(&Value::Null)).unwrap().query,
            super::WeekQuery::Single(None)
        );
        assert_eq!(
            validate(Some(&json!({"week": null}))).unwrap().query,
            super::WeekQuery::Single(None)
        );
    }

    #[test]
    fn a_day_inside_a_week_never_reads_a_file_named_for_that_day() {
        let root = journal("solstone-week-monday-");
        whole(root.path());
        write_raw(
            root.path(),
            "reflections/weekly/20260309.json",
            r#"{"version":1,"sentinel":"SENTINEL_MONDAY"}"#,
        );
        let value = probe(root.path(), json!({"week": "20260309"})).unwrap();
        assert_eq!(value["week"]["start"], "20260308");
        assert!(!value.to_string().contains("SENTINEL_MONDAY"));
        assert_eq!(
            page(root.path()).entries[0].outcome,
            RecordedOutcome::Served
        );
    }

    #[test]
    fn an_untrusted_newest_week_does_not_fall_through() {
        let root = journal("solstone-week-untrusted-newest-");
        whole(root.path());
        let older = document(
            "20260308",
            &[one_memory("OLDER_WEEK_MEMORY", "sol://older")],
            "[]",
        );
        write_raw(root.path(), "reflections/weekly/20260308.json", &older);
        write_raw(
            root.path(),
            "reflections/weekly/20260315.json",
            r#"{"version":2,"days":[]}"#,
        );
        let value = probe(root.path(), json!({"week": "20260315"})).unwrap();
        assert_closed(&value);
        assert_eq!(value["said"], json!([]));
        assert!(value["previous"].is_null());
        assert!(value["next"].is_null());
        assert_eq!(value["days"], json!([]));
    }

    #[test]
    fn a_served_week_matches_the_page_and_keeps_neighbors() {
        let root = journal("solstone-week-parity-");
        whole(root.path());
        let fixture =
            include_str!("../../../../../tests/fixtures/journal/reflections/weekly/20260308.json");
        write_raw(
            root.path(),
            "reflections/weekly/20260301.json",
            &document("20260301", &[one_memory("earlier", "sol://earlier")], "[]"),
        );
        write_raw(root.path(), "reflections/weekly/20260308.json", fixture);
        write_raw(
            root.path(),
            "reflections/weekly/20260315.json",
            &document("20260315", &[one_memory("later", "sol://later")], "[]"),
        );
        write_raw(root.path(), "reflections/weekly/notadate.json", "{}");
        write_raw(root.path(), "reflections/weekly/20260399.json", "{}");
        write_raw(
            root.path(),
            "reflections/weekly/20260322.json",
            r#"{"version":2,"days":[]}"#,
        );
        let value = probe(root.path(), json!({"week": "20260308"})).unwrap();
        assert_closed(&value);
        assert_eq!(value["week"]["start"], "20260308");
        assert_eq!(value["week"]["end"], "20260314");
        assert_matches_page(root.path(), "20260308", &value);
        assert_eq!(value["said"].as_array().unwrap().len(), 0);
        let shape = page(root.path()).entries[0].result.clone().unwrap();
        assert_eq!(
            page(root.path()).entries[0].outcome,
            RecordedOutcome::Served
        );
        assert_eq!(shape.count, 1);
        assert_eq!(shape.targets, vec!["week:20260308".to_owned()]);

        let middle = probe(root.path(), json!({"week": "20260315"})).unwrap();
        assert_eq!(middle["previous"], "20260308");
        assert!(middle["next"].is_null());
    }

    #[test]
    fn quote_only_and_fully_left_out_weeks_are_still_served() {
        let sunday = "20260308";
        let quote_uri = "sol://facets/work/activities/20260308#meeting_1";
        let quote = said_item("s0", "quote-key", sunday, "the owner's words", quote_uri);

        let quote_only = journal("solstone-week-quote-only-");
        whole(quote_only.path());
        write_raw(
            quote_only.path(),
            "reflections/weekly/20260308.json",
            &document(sunday, &[], &format!("[{quote}]")),
        );
        let value = probe(quote_only.path(), json!({"week": sunday})).unwrap();
        assert_closed(&value);
        assert_eq!(value["intro"], "nothing from this week is on this page.");
        assert!(value["memories"].as_array().unwrap().is_empty());
        assert!(value["said"].as_array().unwrap().is_empty());
        assert_matches_page(quote_only.path(), sunday, &value);
        assert_eq!(
            page(quote_only.path()).entries[0].outcome,
            RecordedOutcome::Served
        );
        assert_eq!(
            page(quote_only.path()).entries[0]
                .result
                .as_ref()
                .unwrap()
                .count,
            1
        );

        let hidden = journal("solstone-week-all-left-out-");
        whole(hidden.path());
        write_raw(
            hidden.path(),
            "reflections/weekly/20260308.json",
            &document(sunday, &[one_memory("HIDDEN_MEMORY", "sol://hidden")], "[]"),
        );
        write_left_out(hidden.path(), &["memory-key"]);
        let value = probe(hidden.path(), json!({"week": sunday})).unwrap();
        assert_closed(&value);
        assert_eq!(value["intro"], "nothing from this week is on this page.");
        assert!(value["memories"].as_array().unwrap().is_empty());
        assert!(!value.to_string().contains("HIDDEN_MEMORY"));
        assert_matches_page(hidden.path(), sunday, &value);
        let served = page(hidden.path()).entries[0].result.clone().unwrap();
        assert_eq!(
            page(hidden.path()).entries[0].outcome,
            RecordedOutcome::Served
        );
        assert_eq!(served.count, 1);
        assert_eq!(served.targets, vec!["week:20260308".to_owned()]);

        let kept_quote = journal("solstone-week-quote-kept-");
        whole(kept_quote.path());
        write_raw(
            kept_quote.path(),
            "reflections/weekly/20260308.json",
            &document(
                sunday,
                &[one_memory("HIDDEN_MEMORY", "sol://hidden")],
                &format!("[{quote}]"),
            ),
        );
        write_left_out(kept_quote.path(), &["memory-key"]);
        let value = probe(kept_quote.path(), json!({"week": sunday})).unwrap();
        assert_eq!(value["intro"], "nothing from this week is on this page.");
        assert!(value["memories"].as_array().unwrap().is_empty());
        assert!(value["said"].as_array().unwrap().is_empty());
        assert!(!value.to_string().contains("HIDDEN_MEMORY"));
        assert_matches_page(kept_quote.path(), sunday, &value);
        assert_eq!(
            page(kept_quote.path()).entries[0]
                .result
                .as_ref()
                .unwrap()
                .count,
            1
        );
    }

    fn active_markup(text: &str) -> bool {
        use pulldown_cmark::{Event, Options, Parser, Tag};
        Parser::new_ext(text, Options::all()).any(|event| {
            matches!(
                event,
                Event::Start(Tag::Image { .. } | Tag::Link { .. })
                    | Event::Html(_)
                    | Event::InlineHtml(_)
            )
        })
    }

    #[test]
    fn journal_text_in_a_week_read_stays_inert_in_both_channels() {
        let sunday = "20260308";
        let uri = "sol://facets/work/activities/20260308#meeting_1";
        // An unclosed bracket in one value and a link tail in another: escaped
        // one value at a time, they could still meet in the JSON text channel.
        let title = "open [here ![cover](https://example.test/cover.png) <img src=x>";
        let quote = "](https://example.test/joined)";
        let root = journal("solstone-week-literal-");
        whole(root.path());
        let facet_dir = root.path().join("facets/work");
        fs::create_dir_all(facet_dir.join("activities")).unwrap();
        fs::write(
            facet_dir.join("facet.json"),
            json!({"id": "123e4567-e89b-42d3-a456-426614174000", "title": "Work", "description": "Work"}).to_string(),
        )
        .unwrap();
        fs::write(
            facet_dir.join("activities/20260308.jsonl"),
            json!({"id": "meeting_1", "title": title, "created_at": 100, "segments":["090000_300"]})
                .to_string() + "\n",
        )
        .unwrap();
        owner_speech(root.path(), sunday, "090000_300", quote);
        let memory = Memory {
            id: "m0",
            key: "memory-key",
            day_offset: 0,
            text: "KEPT_MEMORY",
            uri,
            refs: &[],
        };
        let said = said_item(
            "quote-1",
            &said_key(sunday, "work", "meeting_1", quote),
            sunday,
            quote,
            uri,
        );
        write_raw(
            root.path(),
            "reflections/weekly/20260308.json",
            &document(sunday, &[memory], &format!("[{said}]")),
        );
        assert!(active_markup(title), "positive control");

        let week = probe(root.path(), json!({"week": sunday})).unwrap();
        let shown = week["activities"][0]["title"].as_str().unwrap();
        assert!(!active_markup(shown), "{shown}");
        assert_eq!(
            pulldown_cmark::Parser::new(shown)
                .filter_map(|event| match event {
                    pulldown_cmark::Event::Text(text) => Some(text.into_string()),
                    _ => None,
                })
                .collect::<String>(),
            title
        );
        let memory_text = week["memories"][0]["text"].as_str().unwrap();
        assert!(!active_markup(memory_text), "{memory_text}");
        assert_eq!(week["said"][0]["quote"], quote);
        assert_eq!(
            week["said"][0]["uri"], uri,
            "a source location keeps its bytes"
        );

        let plain = tool_result(week.clone(), None);
        assert!(
            active_markup(plain["content"][0]["text"].as_str().unwrap()),
            "the two values meet in plain JSON text"
        );
        let served = crate::jsonrpc::literal_tool_result(week.clone(), None);
        let text = served["content"][0]["text"].as_str().unwrap();
        assert!(!active_markup(text), "{text}");
        assert_eq!(serde_json::from_str::<Value>(text).unwrap(), week);
        assert_eq!(served["structuredContent"], week);
    }

    #[test]
    fn memory_and_quote_leave_outs_are_independent() {
        let sunday = "20260308";
        let facet = "work";
        let record_id = "meeting_1";
        let uri = "sol://facets/work/activities/20260308#meeting_1";
        let omitted = "OMITTED_QUOTE_SENTINEL";
        let restored = "RESTORED_QUOTE";
        let old_key = said_key(sunday, facet, record_id, omitted);
        let new_key = said_key(sunday, facet, record_id, restored);
        assert_ne!(old_key, new_key);
        let root = journal("solstone-week-keys-");
        whole(root.path());
        let facet_dir = root.path().join("facets/work");
        fs::create_dir_all(&facet_dir).unwrap();
        fs::write(
            facet_dir.join("facet.json"),
            json!({"id": "123e4567-e89b-42d3-a456-426614174000", "title": "Work", "description": "Work"}).to_string(),
        ).unwrap();
        let act_dir = facet_dir.join("activities");
        fs::create_dir_all(&act_dir).unwrap();
        fs::write(
            act_dir.join("20260308.jsonl"),
            json!({"id": "meeting_1", "title": "KEPT_MEMORY", "created_at": 100, "segments":["090000_300"]}).to_string()
                + "\n",
        )
        .unwrap();

        let memory = Memory {
            id: "m0",
            key: "memory-key",
            day_offset: 0,
            text: "KEPT_MEMORY",
            uri,
            refs: &[],
        };
        owner_speech(
            root.path(),
            sunday,
            "090000_300",
            &format!("{omitted}. {restored}."),
        );
        let write_quote = |id: &str, key: &str, quote: &str| {
            let said = said_item(id, key, sunday, quote, uri);
            write_raw(
                root.path(),
                "reflections/weekly/20260308.json",
                &document(sunday, &[memory], &format!("[{said}]")),
            );
        };
        write_left_out(root.path(), &[&old_key]);
        write_quote("quote-1", &old_key, omitted);
        let hidden = probe(root.path(), json!({"week": sunday})).unwrap();
        assert_closed(&hidden);
        assert!(hidden["said"].as_array().unwrap().is_empty());
        assert!(!hidden.to_string().contains(omitted));
        assert_eq!(
            hidden["memories"][0]["text"],
            "KEPT_MEMORY (09:00:00 - 09:05:00)"
        );
        assert!(hidden["memories"][0]["source"].is_string());
        assert_eq!(hidden["days"][0]["state"], "memory");

        write_quote("quote-2", &old_key, omitted);
        let rewritten = probe(root.path(), json!({"week": sunday})).unwrap();
        assert!(rewritten["said"].as_array().unwrap().is_empty());
        assert!(!rewritten.to_string().contains(omitted));
        assert_eq!(
            rewritten["memories"][0]["text"],
            hidden["memories"][0]["text"]
        );
        assert_eq!(
            rewritten["memories"][0]["day"],
            hidden["memories"][0]["day"]
        );
        assert_eq!(
            rewritten["memories"][0]["refs"],
            hidden["memories"][0]["refs"]
        );
        assert!(rewritten["memories"][0]["source"].is_string());

        write_quote("quote-2", &new_key, restored);
        let back = probe(root.path(), json!({"week": sunday})).unwrap();
        assert_eq!(back["said"][0]["quote"], restored);
        assert_eq!(back["said"][0]["uri"], uri);
        assert!(!back.to_string().contains(omitted));
        assert_eq!(back["memories"][0]["text"], hidden["memories"][0]["text"]);
        assert_eq!(back["memories"][0]["day"], hidden["memories"][0]["day"]);
        assert_eq!(back["memories"][0]["refs"], hidden["memories"][0]["refs"]);
        assert!(back["memories"][0]["source"].is_string());
        assert_matches_page(root.path(), sunday, &back);
    }

    #[test]
    fn a_week_and_a_range_require_the_same_live_spoken_sentence() {
        let root = journal("week-live-speech-");
        let root = root.path();
        whole(root);
        let day = "20260308";
        let segment = "090000_300";
        let words = "These are the exact spoken words.";
        let uri = "sol://facets/work/activities/20260308#meeting";
        write_raw(
            root,
            "facets/work/facet.json",
            &json!({"id": "123e4567-e89b-42d3-a456-426614174000", "title": "Work"}).to_string(),
        );
        let activity = |stream, hidden| {
            write_raw(root, "facets/work/activities/20260308.jsonl", &json!({"id":"meeting","title":"Morning meeting","stream":stream,"hidden":hidden,"segments":[segment]}).to_string())
        };
        let speech_file = format!("{day}/mic/{segment}/audio.jsonl");
        let write_quote = |file: Option<&str>, id: i64| {
            let mut item: Value =
                serde_json::from_str(&said_item("s0", "quote-key", day, words, uri)).unwrap();
            if let Some(file) = file {
                item["source"]["speech_file"] = json!(file);
                item["source"]["sentence_id"] = json!(id);
            }
            write_raw(
                root,
                "reflections/weekly/20260308.json",
                &document(day, &[], &json!([item]).to_string()),
            );
        };
        let check = |expected| {
            let week = probe(root, json!({"week":day})).unwrap();
            let range = probe(root, json!({"day_from":day,"day_to":day})).unwrap();
            assert_eq!(week["said"][0]["quote"].as_str(), expected);
            assert_eq!(range["days"][0]["said"][0]["quote"].as_str(), expected);
            if expected.is_none() {
                assert!(week["said"].as_array().unwrap().is_empty());
                assert!(range["days"][0].get("said").is_none());
            }
        };
        activity("mic", false);
        owner_speech(root, day, segment, words);
        write_quote(None, 0);
        check(Some(words));
        write_quote(Some(&speech_file), 1);
        check(Some(words));
        write_quote(Some(&speech_file), 2);
        check(None);
        write_quote(Some("../imports/unrelated.jsonl"), 1);
        check(None);
        write_quote(Some(&speech_file), 1);
        // A different person with the owner's display name did not say it as the owner.
        write_raw(
            root,
            "entities/guest/entity.json",
            &json!({"id":"guest","type":"Person","name":"Ari"}).to_string(),
        );
        let labels = format!("chronicle/{day}/mic/{segment}/talents/speaker_labels.json");
        write_raw(
            root,
            &labels,
            &json!({"labels":[{"sentence_id":1,"speaker":"guest","method":"user_confirmed"}]})
                .to_string(),
        );
        check(None);
        owner_speech(root, day, segment, words);
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            let outside = journal("week-live-speech-outside-");
            let linked = root.join(&labels);
            let target = outside.path().join("labels.json");
            fs::rename(&linked, &target).unwrap();
            symlink(&target, &linked).unwrap();
            check(None);
            fs::remove_file(&linked).unwrap();
            fs::rename(target, linked).unwrap();
            check(Some(words));
        }
        use solstone_core_speaker_resolve::owner_line_labels::{
            LineSpeaker, OwnerLine, set_owner_line_labels,
        };
        let line = OwnerLine {
            segment_dir: root.join("chronicle").join(day).join("mic").join(segment),
            source: "audio".into(),
            sentence_id: 1,
        };
        for (answer, expected) in [
            (LineSpeaker::Unknown, None),
            (LineSpeaker::Owner, Some(words)),
        ] {
            set_owner_line_labels(root, std::slice::from_ref(&line), &answer).unwrap();
            check(expected);
        }
        let tombstone = format!("chronicle/{day}/mic/{segment}/tombstone.json");
        write_raw(root, &tombstone, "{}");
        check(None);
        fs::remove_file(root.join(tombstone)).unwrap();
        check(Some(words));
        activity("mic", true);
        check(None);
        activity("mic", false);
        fs::remove_file(root.join("chronicle").join(&speech_file)).unwrap();
        check(None);
        // An older AI export can carry the same words, basename and a stale label.
        let chat = format!("chronicle/{day}/import.claude/{segment}");
        write_raw(
            root,
            &format!("{chat}/imported_audio.jsonl"),
            &json!({"start":"00:00:01","speaker":"You","text":words}).to_string(),
        );
        write_raw(
            root,
            &format!("{chat}/talents/speaker_labels.json"),
            &json!({"labels":[{"sentence_id":1,"speaker":"owner","method":"user_confirmed"}]})
                .to_string(),
        );
        activity("import.claude", false);
        write_quote(None, 0);
        check(None);
    }

    #[test]
    fn both_week_reads_respect_memory_omissions_when_the_file_is_unreadable() {
        let root = journal("solstone-week-omission-state-");
        whole(root.path());
        let sunday = "20260308";
        write_raw(
            root.path(),
            "reflections/weekly/20260308.json",
            &document(sunday, &[one_memory("stored memory", "sol://memory")], "[]"),
        );
        let states = || {
            [
                json!({"week":sunday}),
                json!({"day_from":sunday,"day_to":sunday}),
            ]
            .map(|args| probe(root.path(), args).unwrap()["days"][0]["state"].clone())
        };
        assert_eq!(states(), [json!("memory"), json!("memory")]);
        write_raw(root.path(), "health/week-left-out.json", "{");
        assert_eq!(states(), [json!("not_on_page"), json!("not_on_page")]);
    }

    #[test]
    fn an_unreadable_said_list_releases_nothing() {
        let root = journal("solstone-week-said-unreadable-");
        whole(root.path());
        // A said value that is not an array stays a page and is unreadable.
        write_raw(
            root.path(),
            "reflections/weekly/20260308.json",
            &document(
                "20260308",
                &[one_memory("SENTINEL_MEMORY", "sol://memory")],
                r#"{"quote":"SENTINEL_QUOTE"}"#,
            ),
        );
        let value = probe(root.path(), json!({"week": "20260308"})).unwrap();
        assert_closed(&value);
        assert_eq!(value["said"], json!([]));
        assert!(!value.to_string().contains("SENTINEL_QUOTE"));
    }

    #[test]
    fn navigation_read_failures_refuse_but_malformed_candidates_are_skipped() {
        let root = journal("week-navigation-read-");
        whole(root.path());
        for sunday in ["20260308", "20260322"] {
            write_raw(
                root.path(),
                &format!("reflections/weekly/{sunday}.json"),
                &document(
                    sunday,
                    &[one_memory("VISIBLE_NAVIGATION_MEMORY", "sol://memory")],
                    "[]",
                ),
            );
        }
        let candidate = root.path().join("reflections/weekly/20260315.json");
        fs::create_dir(&candidate).unwrap();
        let value = dispatch_week(root.path(), json!({"week":"20260308"}))
            .unwrap()
            .value;
        assert!(value["next"].is_null());
        fs::remove_dir(&candidate).unwrap();
        fs::write(candidate, "not valid JSON").unwrap();
        let value = dispatch_week(root.path(), json!({"week":"20260308"}))
            .unwrap()
            .value;
        assert_eq!(value["next"], "20260322");
    }

    #[cfg(unix)]
    #[test]
    fn symlink_escapes_and_a_non_regular_newest_week_fail_closed() {
        use std::os::unix::fs::symlink;

        let outside = journal("solstone-week-outside-");
        let sentinel = "OUTSIDE_WEEK_SENTINEL";
        write_raw(
            outside.path(),
            "secret.json",
            &format!(r#"{{"text":"{sentinel}"}}"#),
        );
        write_raw(
            outside.path(),
            "reflections/weekly/20260315.json",
            &document("20260315", &[one_memory(sentinel, "sol://outside")], "[]"),
        );

        let linked_file = journal("solstone-week-linked-file-");
        whole(linked_file.path());
        fs::create_dir_all(linked_file.path().join("reflections/weekly")).unwrap();
        symlink(
            outside.path().join("secret.json"),
            linked_file.path().join("reflections/weekly/20260308.json"),
        )
        .unwrap();
        write_raw(
            linked_file.path(),
            "reflections/weekly/20260301.json",
            &document(
                "20260301",
                &[one_memory("OLDER_REAL_WEEK", "sol://older")],
                "[]",
            ),
        );
        for arguments in [json!({"week": "20260308"}), json!({})] {
            let res = probe(linked_file.path(), arguments).unwrap();
            assert_closed(&res);
            assert_eq!(res["days"], json!([]));
            assert_eq!(res["said"], json!([]));
            assert!(!res.to_string().contains(sentinel));
        }

        let linked_leave_out = journal("solstone-week-linked-left-");
        whole(linked_leave_out.path());
        write_raw(
            linked_leave_out.path(),
            "reflections/weekly/20260308.json",
            &document(
                "20260308",
                &[one_memory("VISIBLE_MEMORY", "sol://memory")],
                "[]",
            ),
        );
        fs::create_dir_all(linked_leave_out.path().join("health")).unwrap();
        symlink(
            outside.path().join("secret.json"),
            linked_leave_out.path().join("health/week-left-out.json"),
        )
        .unwrap();
        let res = probe(linked_leave_out.path(), json!({"week": "20260308"})).unwrap();
        assert_closed(&res);
        assert_eq!(res["said"], json!([]));
        assert!(!res.to_string().contains(sentinel));
        assert!(!res.to_string().contains("VISIBLE_MEMORY"));

        let linked_ancestor = journal("solstone-week-linked-ancestor-");
        whole(linked_ancestor.path());
        symlink(
            outside.path().join("reflections"),
            linked_ancestor.path().join("reflections"),
        )
        .unwrap();
        let res = probe(linked_ancestor.path(), json!({"week": "20260315"})).unwrap();
        assert_closed(&res);
        assert_eq!(res["said"], json!([]));
        assert_eq!(res["days"], json!([]));
        assert!(!res.to_string().contains(sentinel));

        let res_default = probe(linked_ancestor.path(), json!({})).unwrap();
        assert_closed(&res_default);
        assert_eq!(res_default["said"], json!([]));
        assert_eq!(res_default["days"], json!([]));
        assert!(!res_default.to_string().contains(sentinel));
        assert!(
            page(linked_ancestor.path())
                .entries
                .iter()
                .all(|entry| { !entry.reason.as_deref().unwrap_or("").contains(sentinel) })
        );
    }

    #[test]
    fn range_validation_enforces_exclusivity_bounds_and_dates() {
        // Leap year (366 inclusive days) and single day both validate.
        assert_eq!(
            validate(Some(
                &json!({"day_from": "2024-01-01", "day_to": "2024-12-31"})
            ))
            .unwrap()
            .query,
            super::WeekQuery::Range {
                day_from: "20240101".to_owned(),
                day_to: "20241231".to_owned(),
                cursor: None,
            }
        );
        assert_eq!(
            validate(Some(
                &json!({"day_from": "20250308", "day_to": "2025-03-08"})
            ))
            .unwrap()
            .query,
            super::WeekQuery::Range {
                day_from: "20250308".to_owned(),
                day_to: "20250308".to_owned(),
                cursor: None,
            }
        );

        for refused in [
            // Mutual exclusivity with week
            json!({"week": "2025-03-08", "day_from": "2025-03-01", "day_to": "2025-03-08"}),
            json!({"week": "2025-03-08", "day_from": "2025-03-01"}),
            json!({"week": "2025-03-08", "cursor": "tok"}),
            // Half-specified ranges and bare cursor
            json!({"day_from": "2025-03-01"}),
            json!({"day_to": "2025-03-08"}),
            json!({"cursor": "tok"}),
            // Reversed range
            json!({"day_from": "2025-03-09", "day_to": "2025-03-08"}),
            // 367 inclusive days (exceeds 366-day leap-year cap)
            json!({"day_from": "2024-01-01", "day_to": "2025-01-01"}),
        ] {
            assert_eq!(
                validate(Some(&refused)),
                Err(ToolError::WeekRangeArguments),
                "{refused}"
            );
        }
        assert!(
            ToolError::WeekRangeArguments
                .agent_message()
                .contains("366")
        );

        for bad_format in [
            json!({"day_from": "tuesday", "day_to": "2025-03-08"}),
            json!({"day_from": "2025-03-08", "day_to": "2025-02-30"}),
            json!({"day_from": "2025-03-01", "day_to": "2025-03-08", "cursor": ""}),
        ] {
            assert_eq!(
                validate(Some(&bad_format)),
                Err(ToolError::InvalidInput),
                "{bad_format}"
            );
        }
    }

    #[test]
    fn range_mode_returns_compact_day_rows_links_quotes_with_activity_titles_and_handles_missing_pages()
     {
        let root = journal("solstone-week-range-basic-");
        whole(root.path());
        write_raw(
            root.path(),
            "facets/company/facet.json",
            &json!({"id": "123e4567-e89b-42d3-a456-426614174000", "title": "Company", "description": "Company"}).to_string(),
        );
        write_raw(
            root.path(),
            "facets/household/facet.json",
            &json!({"id": "123e4567-e89b-42d3-a456-426614174001", "title": "Household", "description": "Household"}).to_string(),
        );

        // 2025-10-22 (in stored week 2025-10-19): one long top activity, two shorter
        // quoted activities (Gap Y2), plus a hidden activity whose quote must not
        // disclose its hidden title.
        write_raw(
            root.path(),
            "facets/company/activities/20251022.jsonl",
            &([
                json!({"id": "play", "title": "Came in for the afternoon play", "created_at": 100, "segments": ["140000_6840"]}).to_string(),
                json!({"id": "earl", "title": "Heard that the Earl had died", "created_at": 200, "segments": ["162000_360"]}).to_string(),
                json!({"id": "secret_act", "title": "HIDDEN_ACTIVITY_TITLE", "hidden": true, "created_at": 250, "segments": ["170000_120"]}).to_string(),
            ]
            .join("\n")
                + "\n"),
        );
        write_raw(
            root.path(),
            "facets/household/activities/20251022.jsonl",
            &(json!({"id": "tower", "title": "Spoke of an earl dead in the Tower", "created_at": 300, "segments": ["214000_120"]}).to_string()
                + "\n"),
        );

        // 2025-10-26 (in week 2025-10-26, which intentionally has NO stored week page).
        write_raw(
            root.path(),
            "facets/company/activities/20251026.jsonl",
            &(json!({"id": "rehearsal", "title": "Morning rehearsal", "created_at": 400, "segments": ["090000_1800"]}).to_string()
                + "\n"),
        );

        owner_speech(root.path(), "20251022", "162000_360", "Not for my hand.");
        owner_speech(
            root.path(),
            "20251022",
            "214000_120",
            "Leave him his death.",
        );
        owner_speech(root.path(), "20251022", "170000_120", "Quiet aside.");
        let q1_key = said_key("20251022", "company", "earl", "Not for my hand.");
        let q2_key = said_key("20251022", "household", "tower", "Leave him his death.");
        let q3_key = said_key("20251022", "company", "secret_act", "Quiet aside.");
        let q_left_out = said_key("20251022", "company", "earl", "LEFT_OUT_QUOTE");
        let q_outside_range = said_key("20251019", "company", "sunday", "SUNDAY_OUTSIDE_RANGE");
        let said_json = format!(
            "[{}]",
            [
                said_item(
                    "s1",
                    &q1_key,
                    "20251022",
                    "Not for my hand.",
                    "sol://facets/company/activities/20251022#earl",
                ),
                said_item(
                    "s2",
                    &q2_key,
                    "20251022",
                    "Leave him his death.",
                    "sol://facets/household/activities/20251022#tower",
                ),
                said_item(
                    "s3",
                    &q3_key,
                    "20251022",
                    "Quiet aside.",
                    "sol://facets/company/activities/20251022#secret_act",
                ),
                said_item(
                    "s4",
                    &q_left_out,
                    "20251022",
                    "LEFT_OUT_QUOTE",
                    "sol://facets/company/activities/20251022#earl",
                ),
                said_item(
                    "s5",
                    &q_outside_range,
                    "20251019",
                    "SUNDAY_OUTSIDE_RANGE",
                    "sol://facets/company/activities/20251019#sunday",
                ),
            ]
            .join(",")
        );
        let mem = Memory {
            id: "m3",
            key: "mem-20251022",
            day_offset: 3,
            text: "Stored page memory",
            uri: "sol://chronicle/20251022/talents/morning_briefing",
            refs: &[],
        };
        write_raw(
            root.path(),
            "reflections/weekly/20251019.json",
            &document("20251019", &[mem], &said_json),
        );
        write_left_out(root.path(), &[&q_left_out]);

        // Read across the boundary from a stored week (Oct 22–25) into a week with
        // no stored page (Oct 26–27).
        let single_week = probe(root.path(), json!({"week": "20251019"})).unwrap();
        let range = probe(
            root.path(),
            json!({"day_from": "2025-10-22", "day_to": "2025-10-27"}),
        )
        .unwrap();

        assert_eq!(
            range["range"],
            json!({"start": "20251022", "end": "20251027"})
        );
        assert!(range["next_cursor"].is_null());
        assert!(
            range.get("activities").is_none(),
            "range omits activity rows"
        );
        assert!(range.get("memories").is_none());
        assert!(!range.to_string().contains("LEFT_OUT_QUOTE"));
        assert!(!range.to_string().contains("SUNDAY_OUTSIDE_RANGE"));
        assert!(!range.to_string().contains("HIDDEN_ACTIVITY_TITLE"));

        let days = range["days"].as_array().unwrap();
        assert_eq!(days.len(), 6);

        // Oct 22: stored week page + Gap Y2 quote attribution + identical day link.
        let oct22 = &days[0];
        assert_eq!(oct22["day"], "20251022");
        assert_eq!(oct22["state"], "memory");
        assert_eq!(oct22["link"], single_week["days"][3]["link"]);
        assert_eq!(
            oct22["memory"],
            "Came in for the afternoon play (14:00:00 - 15:54:00)"
        );
        assert_eq!(oct22["activities_total"], 3);
        let quotes = oct22["said"].as_array().unwrap();
        assert_eq!(quotes.len(), 2);
        assert_eq!(quotes[0]["quote"], "Not for my hand.");
        assert_eq!(quotes[0]["activity"], "Heard that the Earl had died");
        assert!(
            quotes[0]["link"]
                .as_str()
                .unwrap()
                .starts_with("https://go.solstone.app/j#")
        );
        assert_eq!(quotes[1]["quote"], "Leave him his death.");
        assert_eq!(quotes[1]["activity"], "Spoke of an earl dead in the Tower");
        assert!(!oct22.to_string().contains("Quiet aside."));

        // Oct 23 (in stored week, no activities): has state and link, no memory or said.
        assert_eq!(days[1]["day"], "20251023");
        assert_eq!(days[1]["state"], "nothing_shared");
        assert_eq!(days[1]["link"], single_week["days"][4]["link"]);
        assert!(days[1].get("memory").is_none());
        assert!(days[1].get("said").is_none());
        assert_eq!(days[1]["activities_total"], 0);

        // Oct 26 (no stored week page, has an activity): no invented state, no said,
        // still gets day link, memory and activity count.
        let oct26 = &days[4];
        assert_eq!(oct26["day"], "20251026");
        assert!(
            oct26.get("state").is_none(),
            "no invented state without a week page"
        );
        assert!(oct26.get("said").is_none());
        assert_eq!(oct26["memory"], "Morning rehearsal (09:00:00 - 09:30:00)");
        assert_eq!(oct26["activities_total"], 1);
        let oct26_sealed = oct26["link"].as_str().unwrap().split_once('#').unwrap().1;
        assert_eq!(
            solstone_core_journal_link::open(root.path(), oct26_sealed),
            Ok(solstone_core_journal_link::Place::Day {
                day: "20251026".into()
            })
        );

        // Oct 27 (no stored week page, no activities): no state, no memory, still has link.
        let oct27 = &days[5];
        assert_eq!(oct27["day"], "20251027");
        assert!(oct27.get("state").is_none());
        assert!(oct27.get("memory").is_none());
        assert_eq!(oct27["activities_total"], 0);

        // Audit log records day_from, day_to and week targets.
        let activity = page(root.path());
        let entry = &activity.entries[0];
        let req_args = &entry.request.as_ref().unwrap().arguments;
        assert_eq!(req_args["day_from"], "20251022");
        assert_eq!(req_args["day_to"], "20251027");
        let targets = &entry.result.as_ref().unwrap().targets;
        assert_eq!(
            targets,
            &vec!["week:20251019".to_owned(), "week:20251026".to_owned()]
        );
    }

    #[test]
    fn range_mode_does_not_disclose_a_withheld_body_only_day_without_a_week_page() {
        let root = journal("solstone-week-range-body-hide-");
        whole(root.path());
        let before = probe(
            root.path(),
            json!({"day_from": "20250310", "day_to": "20250312"}),
        )
        .unwrap();
        write_raw(
            root.path(),
            "chronicle/20250311/import.apple_health/090000_300/body.jsonl",
            "{}\n",
        );
        let after = probe(
            root.path(),
            json!({"day_from": "20250310", "day_to": "20250312"}),
        )
        .unwrap();
        assert_eq!(before, after, "a withheld body day changes nothing");
    }

    #[test]
    fn range_mode_pages_by_day_count_and_byte_budget_and_binds_cursors() {
        let root = journal("solstone-week-range-paging-");
        whole(root.path());
        write_raw(
            root.path(),
            "facets/work/facet.json",
            &json!({"id": "123e4567-e89b-42d3-a456-426614174000", "title": "Work", "description": "Work"}).to_string(),
        );
        write_raw(
            root.path(),
            "facets/work/activities/20250101.jsonl",
            &(json!({"id": "a1", "title": "New Year morning walk", "created_at": 100, "segments": ["080000_600"]}).to_string()
                + "\n"),
        );
        write_raw(
            root.path(),
            "facets/work/activities/20250415.jsonl",
            &(json!({"id": "a2", "title": "Spring review", "created_at": 200, "segments": ["100000_900"]}).to_string()
                + "\n"),
        );

        // 105 days via public probe: 100 on page 1, 5 on page 2.
        let p1 = probe(
            root.path(),
            json!({"day_from": "2025-01-01", "day_to": "2025-04-15"}),
        )
        .unwrap();
        assert_eq!(p1["days"].as_array().unwrap().len(), 100);
        assert_eq!(p1["days"][0]["day"], "20250101");
        assert_eq!(p1["days"][99]["day"], "20250410");
        let cursor = p1["next_cursor"].as_str().unwrap().to_owned();

        // Format normalization on page 2 (YYYYMMDD vs YYYY-MM-DD) keeps cursor valid.
        let p2 = probe(
            root.path(),
            json!({"day_from": "20250101", "day_to": "20250415", "cursor": cursor}),
        )
        .unwrap();
        assert_eq!(p2["days"].as_array().unwrap().len(), 5);
        assert_eq!(p2["days"][0]["day"], "20250411");
        assert_eq!(p2["days"][4]["day"], "20250415");
        assert_eq!(
            p2["days"][4]["memory"],
            "Spring review (10:00:00 - 10:15:00)"
        );
        assert!(p2["next_cursor"].is_null());

        // Audit logs a fingerprint for cursor, never the raw token.
        let latest = &page(root.path()).entries[0];
        let latest_args = &latest.request.as_ref().unwrap().arguments;
        assert!(latest_args["cursor"].is_string());
        assert_ne!(latest_args["cursor"].as_str().unwrap(), cursor);

        // Cursor rejects mismatched range, mismatched include, tampered bytes, or
        // a changed permission generation.
        for bad in [
            json!({"day_from": "20250101", "day_to": "20250416", "cursor": cursor}),
            json!({"day_from": "20250101", "day_to": "20250415", "include": ["entities"], "cursor": cursor}),
            json!({"day_from": "20250101", "day_to": "20250415", "cursor": "tampered"}),
        ] {
            assert_eq!(probe(root.path(), bad), Err(McpProbeError::Unavailable));
        }

        // Byte-budget pagination via internal helper: always emits at least 1 row
        // even when max_bytes is tiny, and never drops or duplicates days.
        let decision = evaluate_connection_read(root.path(), CONNECTION);
        let crate::permissions::PermissionDecision::Snapshot(snapshot) = decision else {
            panic!("expected snapshot");
        };
        let principal = DispatchPrincipal {
            connection: CONNECTION,
            agent_identity: CONNECTION,
        };
        let mut seen = Vec::new();
        let mut next: Option<String> = None;
        let mut pages = 0;
        loop {
            let read = super::project_range_with_limits(
                root.path(),
                "20250101",
                "20250105",
                next.as_deref(),
                principal,
                &snapshot,
                &BTreeSet::new(),
                100,
                1, // forces 1 day per page without wedging
            )
            .unwrap();
            pages += 1;
            for row in read.value["days"].as_array().unwrap() {
                seen.push(row["day"].as_str().unwrap().to_owned());
            }
            next = read.value["next_cursor"].as_str().map(str::to_owned);
            if next.is_none() {
                break;
            }
        }
        assert_eq!(pages, 5);
        assert_eq!(
            seen,
            vec!["20250101", "20250102", "20250103", "20250104", "20250105"]
        );
    }

    #[test]
    fn range_mode_omits_days_after_today_and_returns_empty_for_future_ranges() {
        let root = journal("solstone-week-range-future-");
        whole(root.path());
        let owner_zone = solstone_core_journal_config::owner_zone(root.path());
        let today = jiff::Timestamp::now().to_zoned_datetime(owner_zone).date();
        let yesterday = today.yesterday().unwrap();
        let tomorrow = today.tomorrow().unwrap();
        let next_week = today + jiff::Span::new().days(7);

        let partial = probe(
            root.path(),
            json!({
                "day_from": solstone_core_time::day_key(yesterday),
                "day_to": solstone_core_time::day_key(next_week),
            }),
        )
        .unwrap();
        assert_eq!(
            partial["range"],
            json!({
                "start": solstone_core_time::day_key(yesterday),
                "end": solstone_core_time::day_key(today),
            })
        );
        let partial_days: Vec<&str> = partial["days"]
            .as_array()
            .unwrap()
            .iter()
            .map(|d| d["day"].as_str().unwrap())
            .collect();
        assert_eq!(
            partial_days,
            vec![
                solstone_core_time::day_key(yesterday),
                solstone_core_time::day_key(today),
            ]
        );
        assert!(partial["next_cursor"].is_null());

        let future = probe(
            root.path(),
            json!({
                "day_from": solstone_core_time::day_key(tomorrow),
                "day_to": solstone_core_time::day_key(next_week),
            }),
        )
        .unwrap();
        assert_eq!(future["days"], json!([]));
        assert!(future["next_cursor"].is_null());
        assert_eq!(page(root.path()).entries[0].outcome, RecordedOutcome::Empty);
    }

    #[test]
    fn range_mode_include_works_across_pages_and_respects_kind_permissions() {
        let root = journal("solstone-week-range-include-");
        grant(
            root.path(),
            &["transcripts", "facets", "entities", "body"],
            ReadScope::WholeJournal,
        );
        write_raw(
            root.path(),
            "facets/work/facet.json",
            &json!({"id": "123e4567-e89b-42d3-a456-426614174000", "title": "Work", "description": "Work"}).to_string(),
        );
        write_raw(
            root.path(),
            "facets/work/activities/20250309.jsonl",
            &(json!({
                "id": "m1",
                "title": "Design review",
                "created_at": 100,
                "segments": ["090000_600"],
                "participation": [{"name": "Alice", "role": "attendee"}]
            })
            .to_string()
                + "\n"),
        );
        // Sleep recorded on 2025-03-08 waking on 2025-03-09: proves page 2 (starting
        // on 2025-03-09) prepends yesterday when reading body totals.
        write_raw(
            root.path(),
            "chronicle/20250308/import.apple_health/220000_300/body.jsonl",
            &(json!({
                "v": 1,
                "id": "sleep-1",
                "kind": "experience",
                "source": "apple_health",
                "type": "HKCategoryTypeIdentifierSleepAnalysis",
                "label": "night",
                "experience": "sleep",
                "start": "2025-03-08T22:00:00Z",
                "end": "2025-03-09T06:00:00Z",
                "fields": {"total_sleep_duration": 27000}
            })
            .to_string()
                + "\n"),
        );

        let decision = evaluate_connection_read(root.path(), CONNECTION);
        let crate::permissions::PermissionDecision::Snapshot(snapshot) = decision else {
            panic!("expected snapshot");
        };
        let principal = DispatchPrincipal {
            connection: CONNECTION,
            agent_identity: CONNECTION,
        };
        let include: BTreeSet<_> = [
            solstone_core_indexer_query::AdmittedCategory::Body,
            solstone_core_indexer_query::AdmittedCategory::Entities,
            solstone_core_indexer_query::AdmittedCategory::Places,
        ]
        .into_iter()
        .collect();

        // Force 1 day per page so 2025-03-09 lands on page 2.
        let p1 = super::project_range_with_limits(
            root.path(),
            "20250308",
            "20250309",
            None,
            principal,
            &snapshot,
            &include,
            1,
            super::MAX_SPAN_PAGE_BYTES,
        )
        .unwrap();
        let cursor = p1.value["next_cursor"].as_str().unwrap();
        let p2 = super::project_range_with_limits(
            root.path(),
            "20250308",
            "20250309",
            Some(cursor),
            principal,
            &snapshot,
            &include,
            1,
            super::MAX_SPAN_PAGE_BYTES,
        )
        .unwrap();
        let day = &p2.value["days"][0];
        assert_eq!(day["day"], "20250309");
        assert_eq!(day["summary"]["entities"]["count"], 1);
        assert_eq!(day["summary"]["entities"]["people"], json!(["Alice"]));
        assert_eq!(day["summary"]["body"]["nights"][0]["asleep_seconds"], 27000);
        // Connection lacks `places`, so summary carries a permission note.
        assert!(day["summary"]["places"]["note"].is_string());
        assert!(p2.value["next_cursor"].is_null());

        // Reordering `include` in the request still accepts the same cursor.
        let reordered = probe(
            root.path(),
            json!({
                "day_from": "20250308",
                "day_to": "20250309",
                "include": ["places", "entities", "body"],
                "cursor": cursor,
            }),
        )
        .unwrap();
        assert_eq!(reordered["days"][0]["day"], "20250309");
    }

    #[test]
    fn range_mode_keeps_markup_inert_and_fails_closed_on_malformed_activity() {
        let root = journal("solstone-week-range-safety-");
        whole(root.path());
        write_raw(
            root.path(),
            "facets/work/facet.json",
            &json!({"id": "123e4567-e89b-42d3-a456-426614174000", "title": "Work", "description": "Work"}).to_string(),
        );
        let title = "open [link](https://example.test) <img src=x>";
        write_raw(
            root.path(),
            "facets/work/activities/20250309.jsonl",
            &(json!({"id": "a1", "title": title, "created_at": 100,"segments":["090000_300"]})
                .to_string()
                + "\n"),
        );
        owner_speech(
            root.path(),
            "20250309",
            "090000_300",
            "hello [x](https://example.test)",
        );
        let q = said_item(
            "s1",
            &said_key("20250309", "work", "a1", "hello [x](https://example.test)"),
            "20250309",
            "hello [x](https://example.test)",
            "sol://facets/work/activities/20250309#a1",
        );
        write_raw(
            root.path(),
            "reflections/weekly/20250309.json",
            &document("20250309", &[], &format!("[{q}]")),
        );

        let res = probe(
            root.path(),
            json!({"day_from": "20250309", "day_to": "20250309"}),
        )
        .unwrap();
        assert!(!active_markup(res["days"][0]["memory"].as_str().unwrap()));
        assert!(!active_markup(
            res["days"][0]["said"][0]["quote"].as_str().unwrap()
        ));
        assert!(!active_markup(
            res["days"][0]["said"][0]["activity"].as_str().unwrap()
        ));

        // Malformed activity file fails closed.
        write_raw(
            root.path(),
            "facets/work/activities/20250310.jsonl",
            "not json\n",
        );
        assert_eq!(
            probe(
                root.path(),
                json!({"day_from": "20250309", "day_to": "20250310"})
            ),
            Err(McpProbeError::Unavailable)
        );
    }
}
