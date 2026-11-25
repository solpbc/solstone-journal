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

use solstone_core_window_catalog::{ENGLISH, text};

use jiff::civil::{Date, DateTime};
use solstone_core_journal_config::SavedLane;

use crate::freshness::{BacklogStatusEvaluation, SummaryFreshness, UnfinishedActivitiesAggregate};

/// The hour the overnight window closes, local time. The morning briefing is
/// due by then too; before it, nothing overnight can be reported as missed.
pub const OVERNIGHT_WINDOW_END_HOUR: u32 = 10;

const NOT_YET_FIRST_NIGHT_ID: &str = "journal.window.system.health.not.yet.first.night";
const NOT_YET_ENGINE_ID: &str = "journal.window.system.health.not.yet.engine";
const NOT_YET_ENGINE_BYO_ID: &str = "journal.window.system.health.not.yet.engine.byo";
const NOT_YET_ENGINE_SPP_ID: &str = "journal.window.system.health.not.yet.engine.spp";
const NOT_YET_ENGINE_LOCAL_ID: &str = "journal.window.system.health.not.yet.engine.local";
pub const NOT_YET_SEARCH_ID: &str = "journal.window.system.health.not.yet.search";

/// Why the nightly catch-up hasn't had its first chance yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotYet {
    /// No way to think is chosen, so the nightly run is off on purpose
    /// (the supervisor skips it: `no_thinking_engine_chosen`).
    AwaitingEngine(Option<SavedLane>),
    /// A way to think is chosen and the journal's first overnight window is
    /// still ahead, with no catch-up finished yet.
    FirstNight,
}

impl NotYet {
    pub fn text(self) -> String {
        self.text_in(ENGLISH)
    }

    pub fn text_in(self, locale: &str) -> String {
        let id = match self {
            Self::AwaitingEngine(None) => NOT_YET_ENGINE_ID,
            Self::AwaitingEngine(Some(SavedLane::Byo)) => NOT_YET_ENGINE_BYO_ID,
            Self::AwaitingEngine(Some(SavedLane::Spp)) => NOT_YET_ENGINE_SPP_ID,
            Self::AwaitingEngine(Some(SavedLane::Local)) => NOT_YET_ENGINE_LOCAL_ID,
            Self::FirstNight => NOT_YET_FIRST_NIGHT_ID,
        };
        text(locale, id)
    }

    /// Where the note leads: the fix when there is one, the detail otherwise.
    pub const fn href(self) -> &'static str {
        match self {
            Self::AwaitingEngine(None) => "/app/thinking/",
            Self::AwaitingEngine(Some(SavedLane::Byo)) => "/app/thinking/#byo-setup",
            Self::AwaitingEngine(Some(SavedLane::Spp)) => "/app/thinking/#confidential-setup",
            Self::AwaitingEngine(Some(SavedLane::Local)) => "/app/thinking/#local-setup",
            Self::FirstNight => "/app/health/#backlogVerdict",
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AwaitingEngine(_) => "awaiting_engine",
            Self::FirstNight => "first_night",
        }
    }
}

/// The pure rule. `first_day` is the journal's earliest day, if it has one.
pub fn not_yet_rule(
    engine_chosen: bool,
    daily_catchup_finished: bool,
    first_day: Option<Date>,
    local_now: DateTime,
) -> Option<NotYet> {
    if !engine_chosen {
        return Some(NotYet::AwaitingEngine(None));
    }
    if daily_catchup_finished {
        return None;
    }
    let first_day = first_day.unwrap_or(local_now.date());
    let hour = i8::try_from(OVERNIGHT_WINDOW_END_HOUR).expect("valid window hour");
    // A first day at the end of the calendar has its window past every time.
    let ahead = first_day
        .tomorrow()
        .ok()
        .is_none_or(|next| local_now < next.at(hour, 0, 0, 0));
    ahead.then_some(NotYet::FirstNight)
}

/// Read the journal-level facts and apply the rule.
/// A corrupt config reads as no engine chosen, exactly as the supervisor reads it.
/// Cheap first: an established journal returns on its catch-up record without
/// listing its days.
pub fn journal_not_yet(journal: &Path, local_now: DateTime) -> Option<NotYet> {
    if solstone_core_journal_config::no_thinking_engine_chosen(journal) {
        return Some(NotYet::AwaitingEngine(
            solstone_core_journal_config::saved_thinking_lane(journal),
        ));
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
pub fn summary_not_yet(journal: &Path, local_now: DateTime) -> Option<NotYet> {
    match std::fs::symlink_metadata(journal.join("stats.json")) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            journal_not_yet(journal, local_now)
        }
        _ => None,
    }
}

/// The backlog evaluation for a summary that hasn't come up yet.
pub fn not_yet_evaluation(not_yet: NotYet) -> BacklogStatusEvaluation {
    not_yet_evaluation_in(not_yet, ENGLISH)
}

pub fn not_yet_evaluation_in(not_yet: NotYet, locale: &str) -> BacklogStatusEvaluation {
    BacklogStatusEvaluation {
        verdict: not_yet.text_in(locale),
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

fn earliest_day(journal: &Path) -> Result<Option<Date>, solstone_core_journal_io::PathError> {
    let setup_day = solstone_core_segment::setup_completed_at_ms(journal).and_then(|ms| {
        let ts = jiff::Timestamp::from_millisecond(ms).ok()?;
        let tz = solstone_core_journal_config::owner_zone(journal);
        Some(tz.to_datetime(ts).date())
    });
    let days = solstone_core_journal_io::day_dirs(journal)?;
    let earliest_live = days
        .keys()
        .filter(|day| !solstone_core_segment::day_is_backfill_only(journal, day))
        .filter_map(|day| solstone_core_time::parse_day(day))
        .min();
    Ok(match (earliest_live, setup_day) {
        (Some(live), Some(setup)) => Some(live.max(setup)),
        (Some(live), None) => Some(live),
        (None, _) => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(day: &str, hour: i8, minute: i8) -> DateTime {
        solstone_core_time::parse_day(day)
            .unwrap()
            .at(hour, minute, 0, 0)
    }

    fn day(value: &str) -> Option<Date> {
        solstone_core_time::parse_day(value)
    }

    #[test]
    fn no_engine_is_awaiting_engine_on_any_day() {
        assert_eq!(
            not_yet_rule(false, false, day("20260901"), at("20260923", 15, 0)),
            Some(NotYet::AwaitingEngine(None))
        );
        assert_eq!(
            not_yet_rule(false, true, day("20260901"), at("20260923", 15, 0)),
            Some(NotYet::AwaitingEngine(None))
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
            Some(NotYet::AwaitingEngine(None))
        );
    }

    #[test]
    fn awaiting_engine_variants_and_hrefs() {
        assert_eq!(
            NotYet::AwaitingEngine(None).text(),
            "your journal starts catching up once you choose where the thinking happens."
        );
        assert_eq!(NotYet::AwaitingEngine(None).href(), "/app/thinking/");
        assert_eq!(NotYet::AwaitingEngine(None).as_str(), "awaiting_engine");

        for (lane, expected_href) in [
            (SavedLane::Byo, "/app/thinking/#byo-setup"),
            (SavedLane::Spp, "/app/thinking/#confidential-setup"),
            (SavedLane::Local, "/app/thinking/#local-setup"),
        ] {
            let not_yet = NotYet::AwaitingEngine(Some(lane));
            assert_ne!(
                not_yet.text(),
                "your journal starts catching up once you choose where the thinking happens."
            );
            assert_eq!(not_yet.href(), expected_href);
            assert_eq!(not_yet.as_str(), "awaiting_engine");
        }
    }

    #[test]
    fn saved_thinking_lane_is_reflected_in_journal_and_summary_not_yet() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        std::fs::create_dir_all(root.join("config")).unwrap();

        for (choice, expected_lane) in [
            ("byo", SavedLane::Byo),
            ("spp", SavedLane::Spp),
            ("local", SavedLane::Local),
        ] {
            // Saved choice + no provider + no stats.json
            std::fs::write(
                root.join("config/journal.json"),
                format!(r#"{{"setup":{{"thinking_choice":"{choice}"}}}}"#),
            )
            .unwrap();
            assert_eq!(
                summary_not_yet(root, at("20260923", 15, 0)),
                Some(NotYet::AwaitingEngine(Some(expected_lane)))
            );
            assert_eq!(
                journal_not_yet(root, at("20260923", 15, 0)),
                Some(NotYet::AwaitingEngine(Some(expected_lane)))
            );

            // Saved choice + whitespace provider
            std::fs::write(
                root.join("config/journal.json"),
                format!(
                    r#"{{"setup":{{"thinking_choice":"{choice}"}},"providers":{{"active":{{"provider":"   "}}}}}}"#
                ),
            )
            .unwrap();
            assert_eq!(
                summary_not_yet(root, at("20260923", 15, 0)),
                Some(NotYet::AwaitingEngine(Some(expected_lane)))
            );

            // Saved choice + non-empty active provider: not AwaitingEngine
            std::fs::write(
                root.join("config/journal.json"),
                format!(
                    r#"{{"setup":{{"thinking_choice":"{choice}"}},"providers":{{"active":{{"provider":"anthropic"}}}}}}"#
                ),
            )
            .unwrap();
            let reading = summary_not_yet(root, at("20260923", 15, 0));
            assert_ne!(reading, Some(NotYet::AwaitingEngine(Some(expected_lane))));
            assert_ne!(reading, Some(NotYet::AwaitingEngine(None)));
            assert_eq!(reading, Some(NotYet::FirstNight));

            // stats.json present: summary_not_yet is None even with saved lane and no provider
            std::fs::write(
                root.join("config/journal.json"),
                format!(r#"{{"setup":{{"thinking_choice":"{choice}"}}}}"#),
            )
            .unwrap();
            std::fs::write(root.join("stats.json"), b"{}").unwrap();
            assert_eq!(summary_not_yet(root, at("20260923", 15, 0)), None);
            std::fs::remove_file(root.join("stats.json")).unwrap();
        }

        // "later", missing setup, unreadable config -> AwaitingEngine(None)
        std::fs::write(
            root.join("config/journal.json"),
            br#"{"setup":{"thinking_choice":"later"}}"#,
        )
        .unwrap();
        assert_eq!(
            summary_not_yet(root, at("20260923", 15, 0)),
            Some(NotYet::AwaitingEngine(None))
        );

        std::fs::write(root.join("config/journal.json"), br#"{}"#).unwrap();
        assert_eq!(
            summary_not_yet(root, at("20260923", 15, 0)),
            Some(NotYet::AwaitingEngine(None))
        );

        std::fs::write(root.join("config/journal.json"), b"bad json").unwrap();
        assert_eq!(
            summary_not_yet(root, at("20260923", 15, 0)),
            Some(NotYet::AwaitingEngine(None))
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
