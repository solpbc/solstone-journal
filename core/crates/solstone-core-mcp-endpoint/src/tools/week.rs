// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use chrono::{Datelike, NaiveDate, Offset, TimeDelta};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use solstone_core_home::weekly::{
    LeftOut, Said, WeekJudgment, checked_neighbors, format_week_intro, judge, read_left_out,
    resolve_day_cell_state, week_day,
};

use super::ToolError;
use crate::dispatch::{DispatchPrincipal, available_facets, codec};
use crate::permissions::ConnectionReadSnapshot;
use crate::references::{ActivityReference, ReferenceTarget};
use crate::segment_links::{LinkSubject, LiveTranscriptPages, issue_segment_links};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ValidatedGetWeek {
    pub(crate) week: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WeekParams {
    #[serde(default)]
    week: Option<Value>,
}

pub(crate) fn validate(params: Option<&Value>) -> Result<ValidatedGetWeek, ToolError> {
    let params = match params {
        None | Some(Value::Null) => return Ok(ValidatedGetWeek { week: None }),
        Some(val) => val,
    };
    let params = serde_json::from_value::<WeekParams>(params.clone())
        .map_err(|_| ToolError::InvalidInput)?;

    let week = match params.week {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => {
            if s.len() != 8 || !s.bytes().all(|b| b.is_ascii_digit()) || week_day(&s).is_none() {
                return Err(ToolError::InvalidInput);
            }
            Some(s)
        }
        Some(_) => return Err(ToolError::InvalidInput),
    };

    Ok(ValidatedGetWeek { week })
}

pub(crate) struct WeekRead {
    pub(crate) value: Value,
    pub(crate) count: usize,
    pub(crate) targets: Vec<String>,
}

pub(crate) fn read_week(
    journal: &Path,
    week: Option<&str>,
    principal: DispatchPrincipal<'_>,
    snapshot: &ConnectionReadSnapshot,
) -> Result<WeekRead, ToolError> {
    let sunday_date = if let Some(day) = week {
        let Some(date) = week_day(day) else {
            return Err(ToolError::InvalidInput);
        };
        if date.weekday() != chrono::Weekday::Sun {
            return Ok(empty_week());
        }
        date
    } else {
        let owner_zone = solstone_core_journal_config::owner_zone(journal);
        let today = chrono::Utc::now().with_timezone(&owner_zone).date_naive();
        let days_from_sun = today.weekday().num_days_from_sunday();
        today - TimeDelta::days(days_from_sun as i64)
    };

    project_sunday(journal, sunday_date, principal, snapshot)
}

fn empty_week() -> WeekRead {
    WeekRead {
        value: json!({"week": null}),
        count: 0,
        targets: Vec::new(),
    }
}

fn project_sunday(
    journal: &Path,
    sunday: NaiveDate,
    principal: DispatchPrincipal<'_>,
    snapshot: &ConnectionReadSnapshot,
) -> Result<WeekRead, ToolError> {
    let start_day = sunday.format("%Y%m%d").to_string();
    let end_day = (sunday + TimeDelta::days(6)).format("%Y%m%d").to_string();

    let week_dates: Vec<NaiveDate> = (0..7).map(|d| sunday + TimeDelta::days(d)).collect();
    let prev_dates: Vec<NaiveDate> = (1..=7).map(|d| sunday - TimeDelta::days(8 - d)).collect();

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
        let day_str = date.format("%Y%m%d").to_string();
        for (_, dir, _) in &facets {
            let rows = solstone_core_facets::read_activity_records_strict(journal, dir, &day_str)
                .map_err(|_| ToolError::FileUnreadable)?;
            for row in rows {
                if row.get("hidden").and_then(Value::as_bool).unwrap_or(false) {
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
    }

    let mut collected = Vec::new();
    for date in &week_dates {
        let day_str = date.format("%Y%m%d").to_string();
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

            for (row, rec_id) in rows.into_iter().zip(line_identities) {
                if row.get("hidden").and_then(Value::as_bool).unwrap_or(false)
                    || solstone_core_facets::is_retired_plan(&row)
                {
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
                });
            }
        }
    }

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
    ranked.truncate(7);

    let owner_zone = solstone_core_journal_config::owner_zone(journal);
    let now = chrono::Utc::now().with_timezone(&owner_zone);
    let zone_str = solstone_core_journal_config::zone_label(
        Some(owner_zone),
        now.offset().fix().local_minus_utc(),
    );

    let mut activities = Vec::new();
    let mut memories = Vec::new();
    for group in ranked {
        let rep = group.representative;
        let reference = codec()
            .mint(
                principal.connection,
                snapshot.generation,
                ReferenceTarget::Activity(ActivityReference {
                    facet_id: rep.facet_id.clone(),
                    facet_dir: rep.facet_dir.clone(),
                    day: rep.day.clone(),
                    record_identity: rep.record_identity.clone(),
                    file_digest: rep.file_digest.clone(),
                    unit_index: None,
                    source_binding: None,
                }),
            )
            .map_err(|_| ToolError::ReferenceNotFound)?;

        let (start, end, duration) =
            crate::tools::activities::activity_times_and_duration(&rep.record);
        let participants = crate::tools::activities::activity_participants(&rep.record);
        let entities = crate::tools::activities::activity_entities(&rep.record);
        let title = crate::tools::activities::activity_title(&rep.record);

        activities.push(json!({
            "title": title,
            "facet": rep.facet_title,
            "day": rep.day,
            "start": start,
            "end": end,
            "duration": duration,
            "participants": participants,
            "entities": entities,
            "reference": reference.clone(),
            "zone": zone_str,
        }));

        let time_str = match (&start, &end) {
            (Some(s), Some(e)) => Some(format!("{s} - {e}")),
            (Some(s), None) => Some(s.clone()),
            _ => None,
        };
        let mem_text = if let Some(ts) = time_str {
            format!("{title} ({ts})")
        } else {
            title
        };

        let rel_path = format!("facets/{}/activities/{}.jsonl", rep.facet_dir, rep.day);
        let mut single_subject = [LinkSubject {
            path: &rel_path,
            idx: 0,
            text: "",
            segments: Vec::new(),
            more_segments: false,
            activity_record: Some(&rep.record),
        }];
        let mut checker = LiveTranscriptPages::default();
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

    match judge(journal, stem) {
        WeekJudgment::Page(trusted) => {
            let left_out = read_left_out(journal);
            let left_out_unreadable = matches!(left_out, LeftOut::Unreadable);
            let left_out_keys = match left_out {
                LeftOut::Keys(keys) => keys,
                _ => BTreeSet::new(),
            };

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
                let state = resolve_day_cell_state(day.state.as_str(), memory_key, &left_out_keys);
                days.push(json!({"day": day.day, "state": state}));
            }

            if !left_out_unreadable && let Said::Entries(said_entries) = &trusted.said {
                for entry in said_entries {
                    if left_out_keys.contains(&entry.key) {
                        continue;
                    }
                    said.push(json!({
                        "day": entry.day,
                        "quote": entry.quote,
                        "uri": entry.uri,
                    }));
                }
            }
        }
        _ => {
            for date in &week_dates {
                let day_str = date.format("%Y%m%d").to_string();
                days.push(json!({"day": day_str, "state": "nothing_shared"}));
            }
        }
    }

    let intro = format_week_intro(memories.len(), said.len())
        .map(Value::String)
        .unwrap_or(Value::Null);

    let value = json!({
        "week": {"start": start_day, "end": end_day},
        "intro": intro,
        "days": days,
        "memories": memories,
        "said": said,
        "previous": previous,
        "next": next,
        "activities": activities,
    });

    Ok(WeekRead {
        value,
        count: 1,
        targets: vec![format!("week:{start_day}")],
    })
}

fn group_key_for_record(record: &Map<String, Value>) -> String {
    for key in ["activity", "title", "id"] {
        if let Some(s) = record.get(key).and_then(Value::as_str) {
            let trimmed = s.trim().to_lowercase();
            if !trimmed.is_empty() {
                return trimmed;
            }
        }
    }
    String::new()
}

#[cfg(all(test, not(feature = "full-tests")))]
mod tests {
    use std::collections::BTreeSet;
    use std::fs;
    use std::path::Path;

    use chrono::{Duration, NaiveDate, Utc};
    use serde_json::{Value, json};
    use sha2::{Digest, Sha256};
    use solstone_core_home::weekly::{left_out_bytes, page_model};
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
    const EMPTY_NOTE: &str = "no week to read. this is a complete answer, not an error.";
    const WHOLE_JOURNAL: &str =
        "this connection can read a week only with access to the whole journal.";

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
            Utc::now(),
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
        let date = NaiveDate::parse_from_str(sunday, "%Y%m%d").unwrap();
        (date + Duration::days(days)).format("%Y%m%d").to_string()
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
        assert_eq!(
            probe(root.path(), json!({"week": "20260309"})).unwrap(),
            json!({"week": null})
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
            "2026-03-08",
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
        let mon_root = journal("solstone-week-empty-mon-");
        whole(mon_root.path());
        assert_eq!(
            probe(mon_root.path(), json!({"week": "20260309"})).unwrap(),
            json!({"week": null})
        );
        let mon_out = dispatch_week(mon_root.path(), json!({"week": "20260309"})).unwrap();
        assert_eq!(mon_out.value, json!({"week": null}));
        assert_eq!(mon_out.empty_note.as_deref(), Some(EMPTY_NOTE));
        let rendered = tool_result(mon_out.value, mon_out.empty_note.as_deref());
        assert_eq!(rendered["content"][1]["text"], EMPTY_NOTE);

        assert_eq!(validate(None).unwrap().week, None);
        assert_eq!(validate(Some(&Value::Null)).unwrap().week, None);
        assert_eq!(validate(Some(&json!({"week": null}))).unwrap().week, None);
    }

    #[test]
    fn a_real_non_sunday_does_not_read_the_file() {
        let root = journal("solstone-week-monday-");
        whole(root.path());
        write_raw(
            root.path(),
            "reflections/weekly/20260309.json",
            r#"{"version":1,"sentinel":"SENTINEL_MONDAY"}"#,
        );
        let value = probe(root.path(), json!({"week": "20260309"})).unwrap();
        assert_eq!(value, json!({"week": null}));
        assert!(!value.to_string().contains("SENTINEL_MONDAY"));
        assert_eq!(page(root.path()).entries[0].outcome, RecordedOutcome::Empty);
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
        assert_eq!(value["days"][0]["state"], "nothing_shared");
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
        assert!(value["intro"].is_null());
        assert!(value["memories"].as_array().unwrap().is_empty());
        assert_eq!(value["said"][0]["quote"], "the owner's words");
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
        assert!(value["intro"].is_null());
        assert!(value["memories"].as_array().unwrap().is_empty());
        assert_eq!(value["said"].as_array().unwrap().len(), 1);
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
            json!({"id": "meeting_1", "title": "KEPT_MEMORY", "created_at": 100}).to_string()
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
        assert_eq!(hidden["memories"][0]["text"], "KEPT_MEMORY");
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
            assert_eq!(res["days"][0]["state"], "nothing_shared");
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
        assert_eq!(res["days"][0]["state"], "nothing_shared");
        assert!(!res.to_string().contains(sentinel));

        let res_default = probe(linked_ancestor.path(), json!({})).unwrap();
        assert_closed(&res_default);
        assert_eq!(res_default["said"], json!([]));
        assert_eq!(res_default["days"][0]["state"], "nothing_shared");
        assert!(!res_default.to_string().contains(sentinel));
        assert!(
            page(linked_ancestor.path())
                .entries
                .iter()
                .all(|entry| { !entry.reason.as_deref().unwrap_or("").contains(sentinel) })
        );
    }
}
