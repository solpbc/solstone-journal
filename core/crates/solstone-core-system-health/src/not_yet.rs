// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Whether the nightly catch-up has had its first chance to run yet.
//!
//! The backlog summary (`<journal>/stats.json`) is written only by the nightly
//! catch-up. Until that run has had a chance, a missing summary is not a
//! reason to be unsure whether the journal is caught up: the question just
//! hasn't come up. Home, `/app/health` and the stats page all read this one
//! rule, so the three surfaces say the same thing.

use std::path::Path;

use chrono::{Duration, NaiveDate, NaiveDateTime, NaiveTime};

use crate::freshness::{BacklogStatusEvaluation, SummaryFreshness, UnfinishedActivitiesAggregate};

/// The hour the overnight window closes, local time. The morning briefing is
/// due by then too; before it, nothing overnight can be reported as missed.
pub const OVERNIGHT_WINDOW_END_HOUR: u32 = 10;

pub const NOT_YET_FIRST_NIGHT: &str =
    "you'll see whether your journal is caught up after its first night.";
pub const NOT_YET_ENGINE: &str = "your journal starts catching up once processing is set up.";
pub const NOT_YET_SEARCH: &str = "search catches up overnight.";

/// Why the nightly catch-up hasn't had its first chance yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotYet {
    /// No way to think is chosen, so the nightly run is off on purpose
    /// (the supervisor skips it: `no_thinking_engine_chosen`).
    AwaitingEngine,
    /// A way to think is chosen and the journal's first overnight window is
    /// still ahead, with no catch-up finished yet.
    FirstNight,
}

impl NotYet {
    pub const fn text(self) -> &'static str {
        match self {
            Self::AwaitingEngine => NOT_YET_ENGINE,
            Self::FirstNight => NOT_YET_FIRST_NIGHT,
        }
    }

    /// Where the note leads: the fix when there is one, the detail otherwise.
    pub const fn href(self) -> &'static str {
        match self {
            Self::AwaitingEngine => "/app/thinking/",
            Self::FirstNight => "/app/health/#backlogVerdict",
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AwaitingEngine => "awaiting_engine",
            Self::FirstNight => "first_night",
        }
    }
}

/// The pure rule. `first_day` is the journal's earliest day, if it has one.
pub fn not_yet_rule(
    engine_chosen: bool,
    daily_catchup_finished: bool,
    first_day: Option<NaiveDate>,
    local_now: NaiveDateTime,
) -> Option<NotYet> {
    if !engine_chosen {
        return Some(NotYet::AwaitingEngine);
    }
    if daily_catchup_finished {
        return None;
    }
    let first_day = first_day.unwrap_or(local_now.date());
    let window_end = (first_day + Duration::days(1)).and_time(
        NaiveTime::from_hms_opt(OVERNIGHT_WINDOW_END_HOUR, 0, 0).expect("valid window hour"),
    );
    (local_now < window_end).then_some(NotYet::FirstNight)
}

/// Read the journal-level facts and apply the rule.
/// A corrupt config reads as no engine chosen, exactly as the supervisor reads it.
/// Cheap first: an established journal returns on its catch-up record without
/// listing its days.
pub fn journal_not_yet(journal: &Path, local_now: NaiveDateTime) -> Option<NotYet> {
    if solstone_core_journal_config::no_thinking_engine_chosen(journal) {
        return Some(NotYet::AwaitingEngine);
    }
    if crate::catchup_state::read_daily_catchup_finished(journal) {
        return None;
    }
    // A chronicle that can't be listed can't prove the first night is ahead:
    // fall back to today's rules rather than stay calm indefinitely.
    let first_day = earliest_day(journal).ok()?;
    not_yet_rule(true, false, first_day, local_now)
}

/// The backlog summary's own reading: only a summary that does not exist can be
/// "not yet". Any file there, readable or not, keeps today's rules.
pub fn summary_not_yet(journal: &Path, local_now: NaiveDateTime) -> Option<NotYet> {
    match std::fs::symlink_metadata(journal.join("stats.json")) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            journal_not_yet(journal, local_now)
        }
        _ => None,
    }
}

/// The backlog evaluation for a summary that hasn't come up yet.
pub fn not_yet_evaluation(not_yet: NotYet) -> BacklogStatusEvaluation {
    BacklogStatusEvaluation {
        verdict: not_yet.text().to_owned(),
        freshness: SummaryFreshness::Unknown,
        pending_days: 0,
        oldest_pending_day: None,
        unfinished_activities: UnfinishedActivitiesAggregate {
            activities: 0,
            day_count: 0,
            oldest_day: None,
        },
    }
}

fn earliest_day(journal: &Path) -> Result<Option<NaiveDate>, solstone_core_journal_io::PathError> {
    Ok(solstone_core_journal_io::day_dirs(journal)?
        .keys()
        .filter_map(|day| NaiveDate::parse_from_str(day, "%Y%m%d").ok())
        .min())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(day: &str, hour: u32, minute: u32) -> NaiveDateTime {
        NaiveDate::parse_from_str(day, "%Y%m%d")
            .unwrap()
            .and_hms_opt(hour, minute, 0)
            .unwrap()
    }

    fn day(value: &str) -> Option<NaiveDate> {
        NaiveDate::parse_from_str(value, "%Y%m%d").ok()
    }

    #[test]
    fn no_engine_is_awaiting_engine_on_any_day() {
        assert_eq!(
            not_yet_rule(false, false, day("20260901"), at("20260923", 15, 0)),
            Some(NotYet::AwaitingEngine)
        );
        assert_eq!(
            not_yet_rule(false, true, day("20260901"), at("20260923", 15, 0)),
            Some(NotYet::AwaitingEngine)
        );
    }

    #[test]
    fn first_night_holds_until_the_overnight_window_closes() {
        let first = day("20260923");
        assert_eq!(
            not_yet_rule(true, false, first, at("20260923", 15, 0)),
            Some(NotYet::FirstNight)
        );
        assert_eq!(
            not_yet_rule(true, false, first, at("20260924", 7, 0)),
            Some(NotYet::FirstNight)
        );
        assert_eq!(
            not_yet_rule(true, false, first, at("20260924", 9, 59)),
            Some(NotYet::FirstNight)
        );
        assert_eq!(
            not_yet_rule(true, false, first, at("20260924", 10, 0)),
            None
        );
        assert_eq!(
            not_yet_rule(true, false, first, at("20260930", 12, 0)),
            None
        );
    }

    #[test]
    fn no_chronicle_yet_counts_as_today() {
        assert_eq!(
            not_yet_rule(true, false, None, at("20260923", 23, 0)),
            Some(NotYet::FirstNight)
        );
    }

    #[test]
    fn a_finished_catchup_ends_the_first_night() {
        assert_eq!(
            not_yet_rule(true, true, day("20260923"), at("20260924", 2, 0)),
            None
        );
    }

    #[cfg(unix)]
    #[test]
    fn an_unlistable_chronicle_keeps_todays_rules() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        std::fs::create_dir_all(root.join("config")).unwrap();
        std::fs::write(
            root.join("config/journal.json"),
            br#"{"providers":{"active":{"provider":"local"}}}"#,
        )
        .unwrap();
        let chronicle = root.join("chronicle");
        std::fs::create_dir_all(chronicle.join("20260923")).unwrap();
        std::fs::set_permissions(&chronicle, std::fs::Permissions::from_mode(0o000)).unwrap();
        let listed = solstone_core_journal_io::day_dirs(root);
        std::fs::set_permissions(&chronicle, std::fs::Permissions::from_mode(0o755)).unwrap();
        if listed.is_ok() {
            // Running with privileges that ignore modes; nothing to measure.
            return;
        }
        std::fs::set_permissions(&chronicle, std::fs::Permissions::from_mode(0o000)).unwrap();
        let reading = summary_not_yet(root, at("20260923", 15, 0));
        std::fs::set_permissions(&chronicle, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(reading, None);
    }

    #[test]
    fn summary_present_is_never_not_yet() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("stats.json"), b"not json").unwrap();
        assert_eq!(summary_not_yet(temp.path(), at("20260923", 15, 0)), None);
    }

    #[test]
    fn a_blank_journal_awaits_an_engine() {
        let temp = tempfile::tempdir().unwrap();
        assert_eq!(
            summary_not_yet(temp.path(), at("20260923", 15, 0)),
            Some(NotYet::AwaitingEngine)
        );
    }

    #[test]
    fn engine_and_catchup_state_are_read_from_disk() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        std::fs::create_dir_all(root.join("config")).unwrap();
        std::fs::write(
            root.join("config/journal.json"),
            br#"{"providers":{"active":{"provider":"local"}}}"#,
        )
        .unwrap();
        std::fs::create_dir_all(root.join("chronicle/20260923")).unwrap();
        assert_eq!(
            summary_not_yet(root, at("20260924", 7, 0)),
            Some(NotYet::FirstNight)
        );

        std::fs::create_dir_all(root.join("health")).unwrap();
        // Admitted and still running: not finished.
        std::fs::write(
            root.join("health/catchup-state.json"),
            br#"{"version":1,"entries":{"20260923:daily-catchup":{"attempts":1,"last_outcome":"","active":{"ref":"r","started_at":1}}}}"#,
        )
        .unwrap();
        assert_eq!(
            summary_not_yet(root, at("20260924", 7, 0)),
            Some(NotYet::FirstNight)
        );

        // Outcomes the supervisor retries on its own have not settled yet, and a
        // segment-repair entry is not a whole-day catch-up.
        for entries in [
            r#"{"20260923:daily-catchup":{"attempts":1,"last_outcome":"superseded","active":null}}"#,
            r#"{"20260923:daily-catchup":{"attempts":1,"last_outcome":"progressing","active":null}}"#,
            r#"{"20260923:segment-repair":{"attempts":1,"last_outcome":"completed","active":null}}"#,
        ] {
            std::fs::write(
                root.join("health/catchup-state.json"),
                format!(r#"{{"version":1,"entries":{entries}}}"#),
            )
            .unwrap();
            assert_eq!(
                summary_not_yet(root, at("20260924", 7, 0)),
                Some(NotYet::FirstNight),
                "{entries}"
            );
        }

        // An unreadable state file finishes nothing; the clock still bounds it.
        std::fs::write(root.join("health/catchup-state.json"), b"{not json").unwrap();
        assert_eq!(
            summary_not_yet(root, at("20260924", 9, 59)),
            Some(NotYet::FirstNight)
        );
        assert_eq!(summary_not_yet(root, at("20260924", 10, 0)), None);

        std::fs::write(
            root.join("health/catchup-state.json"),
            br#"{"version":1,"entries":{"20260923:daily-catchup":{"attempts":1,"last_outcome":"error","active":null}}}"#,
        )
        .unwrap();
        assert_eq!(summary_not_yet(root, at("20260924", 7, 0)), None);
    }
}
