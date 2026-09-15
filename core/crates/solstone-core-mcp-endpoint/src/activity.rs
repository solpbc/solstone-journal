// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The owner's view of what an agent asked for.
//!
//! 🔴 **There is no projection table, and that is the answer to the recursion.**
//! The canonical `mcp.agent` records *are* the projection; this module is a
//! bounded, deterministic walk over them. Increment B already excludes that
//! stream from the chunk index twice over — by stream marker and by path
//! component — so extending `ConnectionBoundary` to a second store would have
//! been guarding a table that does not exist. What keeps this reader away from
//! the boundary it audits is the **closed registry**: `tools/call` can only
//! name one of the seven [`crate::registry::TOOLS`] rows, and the dispatcher
//! matches a closed enum, so no wire input reaches this function.
//!
//! ⛔ Owner authentication only. This is reached from `journal mcp activity`,
//! never from a bearer credential, and never through
//! [`crate::dispatch::dispatch_authenticated_tool_call`].

use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use std::fs;
use std::path::Path;

use chrono::{DateTime, Utc};
use solstone_core_journal_io::PathError;
use solstone_core_journal_io::paths::{day_dirs, list_dir_entries};
use solstone_core_mcp_audit::{
    AUDIT_STREAM, INTERACTION_FILE, InteractionRecord, OUTCOME_FILE, Outcome, OutcomeRecord,
    RequestRecord, ResultShape, ToolName,
};

/// Upper bound on records opened to fill one activity page.
pub const MAX_EXAMINED_RECORDS: usize = 5_000;

/// The outcome an owner reads back, including the one nothing can write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordedOutcome {
    Served,
    Empty,
    Refused,
    Error,
    /// An admission with no outcome sibling. The call may or may not have run:
    /// the process that would have recorded which is the one that did not
    /// survive, so this value exists only here and never on disk.
    Uncertain,
}

impl RecordedOutcome {
    #[must_use]
    pub const fn token(self) -> &'static str {
        match self {
            Self::Served => "served",
            Self::Empty => "empty",
            Self::Refused => "refused",
            Self::Error => "error",
            Self::Uncertain => "uncertain",
        }
    }

    /// Parse an owner-supplied filter token. ⛔ An unknown token is rejected
    /// rather than widened to "any outcome".
    #[must_use]
    pub fn from_token(value: &str) -> Option<Self> {
        match value {
            "served" => Some(Self::Served),
            "empty" => Some(Self::Empty),
            "refused" => Some(Self::Refused),
            "error" => Some(Self::Error),
            "uncertain" => Some(Self::Uncertain),
            _ => None,
        }
    }

    const fn from_written(outcome: Outcome) -> Self {
        match outcome {
            Outcome::Served => Self::Served,
            Outcome::Empty => Self::Empty,
            Outcome::Refused => Self::Refused,
            Outcome::Error => Self::Error,
        }
    }
}

/// A position in the total activity order, for continuing a page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivityAnchor {
    pub day: String,
    pub segment: String,
}

/// An owner's filter over recorded agent activity.
#[derive(Debug, Clone, Default)]
pub struct ActivityQuery {
    pub connection: Option<String>,
    pub tool: Option<ToolName>,
    pub outcome: Option<RecordedOutcome>,
    /// Inclusive `YYYYMMDD` bounds. ⚠ Callers validate the shape; an
    /// eight-digit day compares correctly as bytes and nothing else does.
    pub day_from: Option<String>,
    pub day_to: Option<String>,
    pub limit: usize,
    pub start_after: Option<ActivityAnchor>,
}

/// One interaction as the owner reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivityEntry {
    pub day: String,
    pub segment: String,
    pub schema: u32,
    pub agent_identity: String,
    pub connection: Option<String>,
    pub timestamp: DateTime<Utc>,
    pub tool_name: ToolName,
    pub request: Option<RequestRecord>,
    pub outcome: RecordedOutcome,
    /// The owner-visible reason. ⚠ Deliberately richer than the wire refusal,
    /// which stays closed and indistinguishable.
    pub reason: Option<String>,
    pub result: Option<ResultShape>,
}

/// One bounded page of activity, newest first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivityPage {
    pub entries: Vec<ActivityEntry>,
    /// Where the walk stopped. ⚠ A resume position, not a promise that more
    /// records exist: paging continues until a page comes back empty. ⛔ A
    /// **coordinate**, never a time — see [`segment_sort_key`].
    pub next: Option<ActivityAnchor>,
    /// Records opened while filling this page.
    pub examined: usize,
    /// False when [`MAX_EXAMINED_RECORDS`] stopped the walk before the corpus
    /// did. ⚠ An empty page with this false means "not found yet", ⛔ never
    /// "there are none".
    pub examination_complete: bool,
    /// Admissions present on disk that could not be parsed. ⛔ Never silently
    /// skipped: in an audit reader a dropped record is a false clean.
    pub unreadable: usize,
}

/// Failure while reading the owner's activity log.
#[derive(Debug)]
pub enum ActivityReadError {
    Path(PathError),
}

impl fmt::Display for ActivityReadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // ⚠ `write!` is refused across this crate's production source by
        // `production_source_has_no_logging_or_printing_surface`; rendering
        // through `format_args!` is the idiom the permission store already uses.
        match self {
            Self::Path(error) => fmt::Display::fmt(
                &format_args!("could not read the activity log: {error}"),
                formatter,
            ),
        }
    }
}

impl Error for ActivityReadError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Path(error) => Some(error),
        }
    }
}

/// Sort key giving a total, deterministic order over one day's segment names.
///
/// A segment name is `HHMMSS_<length-in-seconds>` — a start time and a
/// duration, ⛔ not a sequence number. ⚠ Lexicographic order interleaves `_10`
/// between `_1` and `_2`, so the suffix compares numerically and the raw name
/// breaks any remaining tie, which keeps the order total for a name that does
/// not parse at all.
///
/// 🔴 **And a coordinate is not a clock.** When two records collide, the
/// journal's deconflicting allocator takes a **random ±1 step** on either the
/// start second or the length until it finds a free name, so a record whose own
/// timestamp is `05:31:11.406` can land at `053112_1` while one stamped
/// `05:31:11.455` lands at `053112_2` — measured on a real journal, 3 of 8
/// consecutive records disagreed with their own coordinate. ⚠ The walk is
/// ordered by coordinate because that is the key it can page on without reading
/// a whole day first; the **page it returns is then ordered by the recorded
/// timestamp**, which is the authority. ⛔ So `next` is a coordinate cursor and
/// must never be read as a time.
fn segment_sort_key(name: &str) -> (String, u64, String) {
    match name.rsplit_once('_') {
        Some((head, tail)) => match tail.parse::<u64>() {
            Ok(index) => (head.to_owned(), index, name.to_owned()),
            Err(_) => (name.to_owned(), 0, name.to_owned()),
        },
        None => (name.to_owned(), 0, name.to_owned()),
    }
}

fn day_in_range(day: &str, query: &ActivityQuery) -> bool {
    query
        .day_from
        .as_deref()
        .is_none_or(|from| day.as_bytes() >= from.as_bytes())
        && query
            .day_to
            .as_deref()
            .is_none_or(|to| day.as_bytes() <= to.as_bytes())
}

fn matches(entry: &ActivityEntry, query: &ActivityQuery) -> bool {
    query
        .connection
        .as_deref()
        .is_none_or(|wanted| entry.connection.as_deref() == Some(wanted))
        && query.tool.is_none_or(|wanted| entry.tool_name == wanted)
        && query.outcome.is_none_or(|wanted| entry.outcome == wanted)
}

/// Read one bounded page of the owner's agent-activity log, newest first.
///
/// ⚠ "Newest" is by the **recorded timestamp**, which the returned page is
/// sorted on. The underlying walk runs in coordinate order because that is what
/// it can resume from, and a deconflicted coordinate random-walks a few seconds
/// around its record's real time ([`segment_sort_key`]), so a burst straddling
/// a page boundary can still be split a second out of clock order. Within a
/// page the order is the true one.
///
/// ⛔ **Left as a documented bound, not fixed: the drift is attempt-bounded,
/// not step-bounded, so a lookahead cannot be sized correctly.** The
/// deconflicting walk retries up to `MAX_SEGMENT_ATTEMPTS` (128, and
/// `solstone-core-mcp-audit`'s own caller retries the whole allocation up to
/// another 128 times) — each retry a fresh ±1 coin flip on the *current*
/// candidate, not the original one — so the worst case a page boundary would
/// have to look past is only bounded by that retry ceiling, not by "a few
/// seconds." A correct fix needs either a lookahead deep enough to cover that
/// ceiling (which would mean re-examining most of a busy day on every page) or
/// a cursor that is no longer a plain coordinate, and the second option is the
/// resumability this function is required to keep. The single-collision case
/// this file measures (3 of 8 consecutive records, ±1 second) is the common
/// shape; nothing here should be read as bounding the worst case to it.
pub fn read_activity(
    journal_root: &Path,
    query: &ActivityQuery,
) -> Result<ActivityPage, ActivityReadError> {
    let limit = query.limit.max(1);
    let days = day_dirs(journal_root).map_err(ActivityReadError::Path)?;
    let mut ordered = days
        .into_iter()
        .filter(|(day, _)| day_in_range(day, query))
        .filter(|(day, _)| {
            query
                .start_after
                .as_ref()
                .is_none_or(|anchor| day.as_str() <= anchor.day.as_str())
        })
        .collect::<Vec<_>>();
    ordered.sort_by(|left, right| right.0.cmp(&left.0));

    let mut entries = Vec::new();
    let mut examined = 0_usize;
    let mut unreadable = 0_usize;
    let mut next = None;
    let mut examination_complete = true;

    'days: for (day, directory) in ordered {
        let stream = directory.join(AUDIT_STREAM);
        let mut segments = list_dir_entries(&stream)
            .map_err(ActivityReadError::Path)?
            .into_iter()
            .filter_map(|entry| entry.name.to_str().map(str::to_owned))
            .collect::<Vec<_>>();
        segments.sort_by_cached_key(|name| std::cmp::Reverse(segment_sort_key(name)));
        for segment in segments {
            if let Some(anchor) = query.start_after.as_ref()
                && anchor.day == day
                && segment_sort_key(&segment) >= segment_sort_key(&anchor.segment)
            {
                continue;
            }
            let segment_directory = stream.join(&segment);
            let Ok(bytes) = fs::read(segment_directory.join(INTERACTION_FILE)) else {
                continue;
            };
            examined += 1;
            // The resume position is the last record actually opened, so
            // continuing never steps over one the walk has not looked at.
            let position = ActivityAnchor {
                day: day.clone(),
                segment: segment.clone(),
            };
            match serde_json::from_slice::<InteractionRecord>(&bytes) {
                Ok(record) => {
                    let sibling = fs::read(segment_directory.join(OUTCOME_FILE))
                        .ok()
                        .and_then(|bytes| serde_json::from_slice::<OutcomeRecord>(&bytes).ok());
                    let entry = ActivityEntry {
                        day: day.clone(),
                        segment: segment.clone(),
                        schema: record.schema,
                        agent_identity: record.agent_identity,
                        connection: record.connection,
                        timestamp: record.timestamp,
                        tool_name: record.tool_name,
                        request: record.request,
                        outcome: sibling
                            .as_ref()
                            .map_or(RecordedOutcome::Uncertain, |outcome| {
                                RecordedOutcome::from_written(outcome.outcome)
                            }),
                        reason: sibling.as_ref().and_then(|outcome| outcome.reason.clone()),
                        result: sibling.and_then(|outcome| outcome.result),
                    };
                    if matches(&entry, query) {
                        entries.push(entry);
                    }
                }
                Err(_) => unreadable += 1,
            }
            if examined >= MAX_EXAMINED_RECORDS {
                examination_complete = false;
                next = Some(position);
                break 'days;
            }
            if entries.len() >= limit {
                next = Some(position);
                break 'days;
            }
        }
    }

    // The walk collected in coordinate order so it could resume; the owner is
    // owed clock order, and the timestamp is the authority. Coordinate breaks
    // the tie, because three records can share one second.
    entries.sort_by(|left, right| {
        right
            .timestamp
            .cmp(&left.timestamp)
            .then_with(|| right.day.cmp(&left.day))
            .then_with(|| segment_sort_key(&right.segment).cmp(&segment_sort_key(&left.segment)))
    });

    Ok(ActivityPage {
        entries,
        next,
        examined,
        examination_complete,
        unreadable,
    })
}

/// Count recorded outcomes across one page.
///
/// ⚠ A count carries its construction: this counts the entries of the page it
/// is handed, which is one bounded walk, ⛔ never the whole log.
/// [`ActivityPage::examination_complete`] says whether the bound was reached.
#[must_use]
pub fn tally(entries: &[ActivityEntry]) -> BTreeMap<&'static str, usize> {
    let mut counts = BTreeMap::new();
    for entry in entries {
        *counts.entry(entry.outcome.token()).or_insert(0) += 1;
    }
    counts
}

#[cfg(test)]
mod tests {
    use std::fs;

    use chrono::{TimeZone, Utc};
    use serde_json::json;
    use solstone_core_mcp_audit::{
        Admission, AuditCoordinates, INTERACTION_FILE, Outcome, ToolName, result_shape,
        write_interaction_record, write_outcome_record,
    };

    use super::{
        ActivityAnchor, ActivityQuery, MAX_EXAMINED_RECORDS, RecordedOutcome, read_activity, tally,
    };

    fn fixture() -> tempfile::TempDir {
        tempfile::Builder::new()
            .prefix("solstone-mcp-activity-")
            .tempdir_in("/var/tmp")
            .expect("fixture journal")
    }

    fn admit(
        journal: &tempfile::TempDir,
        connection: &str,
        tool_name: ToolName,
        hour: u32,
        minute: u32,
    ) -> AuditCoordinates {
        write_interaction_record(
            journal.path(),
            Utc.with_ymd_and_hms(2026, 8, 31, hour, minute, 0).unwrap(),
            &Admission {
                connection,
                agent_identity: connection,
                tool_name,
                arguments: json!({"query": "budget"}).as_object().cloned().unwrap(),
            },
        )
        .expect("admission publishes")
    }

    fn query(limit: usize) -> ActivityQuery {
        ActivityQuery {
            limit,
            ..ActivityQuery::default()
        }
    }

    #[test]
    fn a_refusal_and_a_served_call_are_distinguishable_in_the_owners_log() {
        let journal = fixture();
        let refused = admit(&journal, "bearer:one", ToolName::Search, 10, 0);
        write_outcome_record(
            journal.path(),
            &refused,
            Utc::now(),
            Outcome::Refused,
            Some("the transcripts category is not granted to this connection"),
            None,
        )
        .unwrap();
        let served = admit(&journal, "bearer:one", ToolName::Search, 11, 0);
        write_outcome_record(
            journal.path(),
            &served,
            Utc::now(),
            Outcome::Served,
            None,
            Some(result_shape(
                1,
                vec!["20260831/default/090000_1/talents/brief.md#0".to_owned()],
                "abc123".to_owned(),
            )),
        )
        .unwrap();

        let page = read_activity(journal.path(), &query(10)).unwrap();
        assert_eq!(page.entries.len(), 2);
        // Newest first.
        assert_eq!(page.entries[0].outcome, RecordedOutcome::Served);
        assert_eq!(page.entries[1].outcome, RecordedOutcome::Refused);
        assert_eq!(
            page.entries[1].reason.as_deref(),
            Some("the transcripts category is not granted to this connection")
        );
        assert_eq!(page.entries[0].result.as_ref().unwrap().count, 1);
        assert_eq!(page.entries[0].result.as_ref().unwrap().digest, "abc123");
        // 🔑 § 5.3's gap: the owner can now be told "and it was refused".
        let only_refusals = read_activity(
            journal.path(),
            &ActivityQuery {
                outcome: Some(RecordedOutcome::Refused),
                ..query(10)
            },
        )
        .unwrap();
        assert_eq!(only_refusals.entries.len(), 1);
        assert_eq!(tally(&page.entries).get("refused"), Some(&1));
        assert_eq!(tally(&page.entries).get("served"), Some(&1));
    }

    #[test]
    fn the_recorded_query_is_what_the_agent_asked_for() {
        let journal = fixture();
        admit(&journal, "bearer:one", ToolName::Search, 9, 0);
        let page = read_activity(journal.path(), &query(10)).unwrap();
        let request = page.entries[0].request.as_ref().expect("schema 2 request");
        assert_eq!(request.arguments["query"], "budget");
        assert!(!request.arguments_omitted);
    }

    #[test]
    fn an_admission_without_an_outcome_reads_uncertain() {
        let journal = fixture();
        admit(&journal, "bearer:one", ToolName::GetEntity, 12, 0);
        let page = read_activity(journal.path(), &query(10)).unwrap();
        assert_eq!(page.entries[0].outcome, RecordedOutcome::Uncertain);
    }

    #[test]
    fn a_schema_one_record_stays_minimal_when_read_back() {
        let journal = fixture();
        let segment = journal.path().join("chronicle/20260831/mcp.agent/010203_1");
        fs::create_dir_all(&segment).unwrap();
        fs::write(
            segment.join(INTERACTION_FILE),
            json!({
                "agent_identity": "operator",
                "timestamp": "2026-08-31T01:02:03Z",
                "tool_name": "fetch",
            })
            .to_string(),
        )
        .unwrap();

        let page = read_activity(journal.path(), &query(10)).unwrap();
        assert_eq!(page.entries.len(), 1);
        let entry = &page.entries[0];
        assert_eq!(entry.schema, 1);
        // ⛔ No connection and no request are invented for a record with none.
        assert!(entry.connection.is_none());
        assert!(entry.request.is_none());
        assert_eq!(entry.outcome, RecordedOutcome::Uncertain);
    }

    #[test]
    fn the_order_is_total_and_does_not_interleave_length_ten_between_one_and_two() {
        let journal = fixture();
        // Written directly: a segment name is `HHMMSS_<length>`, and the
        // deconflicting allocator steps the *time*, so it cannot produce the
        // same-second collision this ordering rule exists for.
        for (day, segment) in [
            ("20260830", "235959_1"),
            ("20260831", "120000_2"),
            ("20260831", "120000_10"),
            ("20260831", "120000_9"),
            ("20260831", "090000_1"),
            ("20260831", "not-a-segment-key"),
        ] {
            let directory = journal
                .path()
                .join("chronicle")
                .join(day)
                .join("mcp.agent")
                .join(segment);
            fs::create_dir_all(&directory).unwrap();
            fs::write(
                directory.join(INTERACTION_FILE),
                json!({
                    "schema": 2,
                    "agent_identity": "bearer:one",
                    "connection": "bearer:one",
                    "timestamp": "2026-08-31T12:00:00Z",
                    "tool_name": "search",
                    "request": {"arguments": {}},
                })
                .to_string(),
            )
            .unwrap();
        }

        let page = read_activity(journal.path(), &query(20)).unwrap();
        let segments = page
            .entries
            .iter()
            .map(|entry| format!("{}/{}", entry.day, entry.segment))
            .collect::<Vec<_>>();
        assert_eq!(
            segments,
            [
                "20260831/not-a-segment-key",
                "20260831/120000_10",
                "20260831/120000_9",
                "20260831/120000_2",
                "20260831/090000_1",
                "20260830/235959_1",
            ],
            "⚠ lexicographic order would put 120000_10 after 120000_2"
        );
        // Running it again gives the same answer: the order is not the
        // filesystem's, which is what "deterministic" has to mean here.
        let again = read_activity(journal.path(), &query(20)).unwrap();
        assert_eq!(again.entries, page.entries);
    }

    #[test]
    fn a_coordinate_that_disagrees_with_its_own_timestamp_is_ordered_by_the_timestamp() {
        // 🔴 Measured on a real journal: the deconflicting allocator takes a
        // random ±1 step, so 3 of 8 consecutive records landed on a coordinate
        // a second away from their own timestamp. Coordinate order put
        // `053113_1` (.431) ahead of `053112_2` (.455). The timestamp is the
        // authority and the page is sorted on it.
        let journal = fixture();
        for (segment, millis) in [
            ("053113_1", 431),
            ("053112_2", 455),
            ("053112_1", 406),
            ("053111_3", 382),
        ] {
            let directory = journal
                .path()
                .join("chronicle/20260831/mcp.agent")
                .join(segment);
            fs::create_dir_all(&directory).unwrap();
            fs::write(
                directory.join(INTERACTION_FILE),
                json!({
                    "schema": 2,
                    "agent_identity": "bearer:one",
                    "connection": "bearer:one",
                    "timestamp": format!("2026-08-31T05:31:11.{millis}Z"),
                    "tool_name": "list_facets",
                    "request": {"arguments": {}},
                })
                .to_string(),
            )
            .unwrap();
        }

        let page = read_activity(journal.path(), &query(10)).unwrap();
        let segments = page
            .entries
            .iter()
            .map(|entry| entry.segment.clone())
            .collect::<Vec<_>>();
        assert_eq!(
            segments,
            ["053112_2", "053113_1", "053112_1", "053111_3"],
            "⚠ coordinate order would put 053113_1 first"
        );
        // Still a total order: repeating the read gives the same answer.
        assert_eq!(
            read_activity(journal.path(), &query(10)).unwrap().entries,
            page.entries
        );
    }

    #[test]
    fn paging_resumes_after_the_anchor_without_repeating_or_skipping() {
        let journal = fixture();
        for _ in 0..7 {
            admit(&journal, "bearer:one", ToolName::Search, 12, 0);
        }
        let mut seen: Vec<String> = Vec::new();
        let mut anchor: Option<ActivityAnchor> = None;
        for _ in 0..10 {
            let page = read_activity(
                journal.path(),
                &ActivityQuery {
                    start_after: anchor.clone(),
                    ..query(3)
                },
            )
            .unwrap();
            if page.entries.is_empty() {
                break;
            }
            seen.extend(page.entries.iter().map(|entry| entry.segment.clone()));
            anchor = page.next;
            if anchor.is_none() {
                break;
            }
        }
        let mut unique = seen.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(seen.len(), 7, "no record repeated: {seen:?}");
        assert_eq!(unique.len(), 7, "no record skipped: {seen:?}");
    }

    #[test]
    fn a_malformed_admission_is_counted_rather_than_silently_skipped() {
        let journal = fixture();
        let segment = journal.path().join("chronicle/20260831/mcp.agent/010203_1");
        fs::create_dir_all(&segment).unwrap();
        fs::write(segment.join(INTERACTION_FILE), "{not json").unwrap();

        let page = read_activity(journal.path(), &query(10)).unwrap();
        assert!(page.entries.is_empty());
        assert_eq!(page.unreadable, 1);
        assert_eq!(page.examined, 1);
    }

    #[test]
    fn connection_tool_and_day_filters_narrow_the_same_walk() {
        let journal = fixture();
        write_interaction_record(
            journal.path(),
            Utc.with_ymd_and_hms(2026, 8, 30, 9, 0, 0).unwrap(),
            &Admission {
                connection: "bearer:one",
                agent_identity: "bearer:one",
                tool_name: ToolName::Search,
                arguments: serde_json::Map::new(),
            },
        )
        .unwrap();
        admit(&journal, "bearer:two", ToolName::Fetch, 9, 0);
        admit(&journal, "bearer:one", ToolName::Fetch, 10, 0);

        assert_eq!(
            read_activity(
                journal.path(),
                &ActivityQuery {
                    connection: Some("bearer:one".to_owned()),
                    ..query(10)
                }
            )
            .unwrap()
            .entries
            .len(),
            2
        );
        assert_eq!(
            read_activity(
                journal.path(),
                &ActivityQuery {
                    tool: Some(ToolName::Fetch),
                    ..query(10)
                }
            )
            .unwrap()
            .entries
            .len(),
            2
        );
        let by_day = read_activity(
            journal.path(),
            &ActivityQuery {
                day_from: Some("20260831".to_owned()),
                day_to: Some("20260831".to_owned()),
                ..query(10)
            },
        )
        .unwrap();
        assert_eq!(by_day.entries.len(), 2);
        assert!(by_day.entries.iter().all(|entry| entry.day == "20260831"));
    }

    #[test]
    fn an_empty_log_is_a_complete_examination_and_an_unknown_filter_token_is_refused() {
        let journal = fixture();
        let page = read_activity(journal.path(), &query(10)).unwrap();
        assert!(page.entries.is_empty());
        assert_eq!(page.examined, 0);
        // ⚠ The distinction the walk has to keep: nothing found, bound never hit.
        assert!(page.examination_complete);
        assert!(page.next.is_none());

        assert!(RecordedOutcome::from_token("anything").is_none());
        for token in ["served", "empty", "refused", "error", "uncertain"] {
            assert_eq!(
                RecordedOutcome::from_token(token).map(RecordedOutcome::token),
                Some(token)
            );
        }
        const { assert!(MAX_EXAMINED_RECORDS > 0) };
    }

    #[test]
    fn the_owner_reader_is_not_reachable_from_the_closed_tool_registry() {
        // 🔴 The recursion guard as a property rather than a comment: no
        // registry row names this surface, and the dispatcher only ever
        // executes a row's own validated tool.
        for entry in crate::registry::TOOLS {
            assert!(
                !entry.wire_name.contains("activity"),
                "{} names the owner activity surface",
                entry.wire_name
            );
        }
        assert_eq!(crate::registry::TOOLS.len(), 7);
        assert!(crate::registry::tool_by_wire_name("list_activity").is_none());
        assert!(crate::registry::tool_by_wire_name("get_agent_interaction").is_none());
    }
}
