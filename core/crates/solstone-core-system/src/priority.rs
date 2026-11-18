// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Default task classification; explicit import and owner intents override it.

use jiff::civil::Date;
use solstone_core_journal_config::work_priority::{WorkBand, WorkPriority};

pub fn classify(command: &[String], today: Date) -> WorkPriority {
    let argument = |flag: &str| {
        command
            .windows(2)
            .find(|pair| pair[0] == flag)
            .map(|pair| pair[1].as_str())
    };
    let date = argument("--day")
        .and_then(solstone_core_time::parse_day)
        .unwrap_or_else(|| {
            if command
                .iter()
                .any(|arg| matches!(arg.as_str(), "--cadence" | "--settle"))
            {
                today
            } else {
                today - jiff::Span::new().days(1)
            }
        });
    let age = today.duration_since(date).as_hours() / 24;
    let band = if command.iter().any(|arg| arg == "--from-scratch") {
        WorkBand::Waiting
    } else if command
        .iter()
        .any(|arg| matches!(arg.as_str(), "--weekly" | "--cadence"))
    {
        WorkBand::Recent
    } else if age <= 0 {
        WorkBand::Now
    } else if age == 1 {
        WorkBand::Waiting
    } else if age <= 7 {
        WorkBand::Recent
    } else if command
        .iter()
        .any(|arg| matches!(arg.as_str(), "--segment" | "--segments" | "--sense-batch"))
    {
        WorkBand::Completing
    } else {
        WorkBand::Remake
    };
    let time = argument("--segment")
        .and_then(|segment| segment.get(..6))
        .filter(|time| time.bytes().all(|byte| byte.is_ascii_digit()))
        .and_then(|time| time.parse::<u64>().ok())
        .unwrap_or(0);
    let source_time = solstone_core_time::day_key(date)
        .parse::<u64>()
        .unwrap_or(0)
        * 1_000_000
        + time;
    WorkPriority { band, source_time }
}

/// Coalesced work retains both inferred work and its best explicit request.
/// Derived work ages while waiting; a later owner ask must not be lost then.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PriorityIntent {
    explicit: Option<WorkPriority>,
    derived: bool,
}

impl Default for PriorityIntent {
    fn default() -> Self {
        Self::new(None)
    }
}

impl PriorityIntent {
    pub(crate) fn new(explicit: Option<WorkPriority>) -> Self {
        Self {
            explicit,
            derived: explicit.is_none(),
        }
    }
    pub(crate) fn merge(&mut self, other: Self) {
        self.derived |= other.derived;
        self.explicit = self
            .explicit
            .into_iter()
            .chain(other.explicit)
            .min_by_key(|priority| priority.sort_key());
    }
    pub(crate) fn resolve(self, command: &[String], today: Date) -> WorkPriority {
        self.explicit
            .into_iter()
            .chain(self.derived.then(|| classify(command, today)))
            .min_by_key(|priority| priority.sort_key())
            .expect("priority has a source")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dated_modes_and_owner_override_share_one_order() {
        let today = jiff::civil::date(2026, 10, 11);
        let command = |args: &[&str]| args.iter().map(|arg| arg.to_string()).collect::<Vec<_>>();
        for (args, band) in [
            (
                vec!["--day", "20261011", "--segment", "120000_300"],
                WorkBand::Now,
            ),
            (vec!["--day", "20261010"], WorkBand::Waiting),
            (vec!["--day", "20261004"], WorkBand::Recent),
            (
                vec!["--day", "20261003", "--segment", "120000_300"],
                WorkBand::Completing,
            ),
            (vec!["--day", "20261003"], WorkBand::Remake),
            (
                vec!["--day", "20261003", "--from-scratch"],
                WorkBand::Waiting,
            ),
            (vec!["--weekly"], WorkBand::Recent),
            (vec!["--cadence"], WorkBand::Recent),
            (vec!["--flush"], WorkBand::Waiting),
            (vec!["--settle"], WorkBand::Now),
        ] {
            assert_eq!(classify(&command(&args), today).band, band);
        }
        let older = classify(
            &command(&["--day", "20261011", "--segment", "090000_300"]),
            today,
        );
        let newer = classify(
            &command(&["--day", "20261011", "--segment", "120000_300"]),
            today,
        );
        assert!(newer.sort_key() < older.sort_key());
        assert_eq!(newer.source_time, 20261011120000);
    }
    #[test]
    fn waiting_intent_ages_without_losing_explicit_requests() {
        let command = [
            "journal",
            "think",
            "--day",
            "20261011",
            "--segment",
            "120000_300",
        ]
        .map(str::to_owned);
        let date = |day| jiff::civil::date(2026, 10, day);
        let mut intent = PriorityIntent::default();
        assert_eq!(
            intent.resolve(&command, date(11)).to_string(),
            "1:20261011120000"
        );
        assert_eq!(
            intent.resolve(&command, date(12)).to_string(),
            "2:20261011120000"
        );
        assert_eq!(intent.resolve(&command, date(18)).band, WorkBand::Recent);
        assert_eq!(
            intent.resolve(&command, date(19)).band,
            WorkBand::Completing
        );
        let daily = &command[..4];
        assert_eq!(intent.resolve(daily, date(19)).band, WorkBand::Remake);
        intent.merge(PriorityIntent::new(Some(
            "2:20261011120000".parse().unwrap(),
        )));
        assert_eq!(intent.resolve(&command, date(11)).band, WorkBand::Now);
        assert_eq!(intent.resolve(&command, date(19)).band, WorkBand::Waiting);
        // The tied owner request also remains after the derived candidate ages.
        let mut tied = PriorityIntent::default();
        tied.merge(PriorityIntent::new(Some(
            "1:20261011120000".parse().unwrap(),
        )));
        assert_eq!(tied.resolve(&command, date(19)).band, WorkBand::Now);
        for value in ["4:20261011120000", "2:20261011120000"] {
            let explicit = PriorityIntent::new(Some(value.parse().unwrap()));
            assert_eq!(explicit.resolve(&command, date(11)).to_string(), value);
            assert_eq!(explicit.resolve(&command, date(19)).to_string(), value);
        }
    }

    #[test]
    fn undated_priority_matches_the_executed_think_day() {
        let today = jiff::civil::date(2026, 10, 11);
        for (mode, date, band) in [
            ("", 20261010000000, WorkBand::Waiting),
            ("--flush", 20261010000000, WorkBand::Waiting),
            ("--settle", 20261011000000, WorkBand::Now),
            ("--cadence", 20261011000000, WorkBand::Recent),
        ] {
            let priority = classify(&["journal".into(), "think".into(), mode.into()], today);
            assert_eq!(priority.source_time, date);
            assert_eq!(priority.band, band);
        }
    }
}
