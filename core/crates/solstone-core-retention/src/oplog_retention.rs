// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Liveness-gated retention for canonical operational logs.
//!
//! This is deliberately separate from the ordinary log-class table: a canonical
//! oplog is eligible only after a lease probe on the descriptor admitted by the
//! operational-log catalog. A malformed or unsafe `oplog--` candidate makes the
//! catalog unavailable for its whole day, so that day is kept fail-closed while
//! other days are still considered independently.
//!
//! A log's age is its partition day, unless the log was written later than
//! that. Re-running a past day writes that run's logs into the past day's
//! partition, and a pinned writer's opened instant lies on that day too, so
//! only the admitted descriptor's last write says the run was recent. A log
//! last written inside the window is kept with it.

use std::fs::File;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{DateTime, NaiveDate};
use solstone_core_journal_io::JournalRoot;
use solstone_core_journal_io::operational_log::{
    LeaseProbe, OplogFileIdentity, catalog_oplogs, probe_retained_oplog_lease,
};

use crate::layout::oplog_rel;
use crate::logs::{LogPolicy, chronicle_days, cutoff, day_key};

/// One canonical oplog whose containing day is old and whose lease was released.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OplogRetentionTarget {
    day: NaiveDate,
    leaf: String,
    identity: OplogFileIdentity,
    bytes: u64,
}

impl OplogRetentionTarget {
    pub fn day(&self) -> NaiveDate {
        self.day
    }

    pub fn leaf(&self) -> &str {
        &self.leaf
    }

    pub fn identity(&self) -> OplogFileIdentity {
        self.identity
    }

    pub fn bytes(&self) -> u64 {
        self.bytes
    }

    pub fn rel(&self) -> String {
        oplog_rel(&day_key(self.day), &self.leaf)
    }
}

/// Why an oplog or whole day was retained.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OplogRetentionKept {
    /// The containing day is inside the configured retention window.
    TooYoung,
    /// The catalogued file still holds its writer lease.
    Active,
    /// The lease probe could not safely decide whether the writer is live.
    Indeterminate,
    /// The complete catalog for this day could not be admitted safely.
    DayUnavailable { kind: String },
}

impl OplogRetentionKept {
    pub fn label(&self) -> &'static str {
        match self {
            Self::TooYoung => "too_young",
            Self::Active => "active",
            Self::Indeterminate => "indeterminate",
            Self::DayUnavailable { .. } => "day_unavailable",
        }
    }

    pub fn detail(&self) -> Option<&str> {
        match self {
            Self::DayUnavailable { kind } => Some(kind),
            Self::TooYoung | Self::Active | Self::Indeterminate => None,
        }
    }
}

/// One retained canonical oplog, or a day-level fail-closed diagnostic.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RetainedOplog {
    day: NaiveDate,
    leaf: Option<String>,
    reason: OplogRetentionKept,
}

impl RetainedOplog {
    pub fn day(&self) -> NaiveDate {
        self.day
    }

    pub fn leaf(&self) -> Option<&str> {
        self.leaf.as_deref()
    }

    pub fn reason(&self) -> &OplogRetentionKept {
        &self.reason
    }
}

/// The read-only canonical-oplog retention plan.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct OplogRetentionPlan {
    pub prunable: Vec<OplogRetentionTarget>,
    pub retained: Vec<RetainedOplog>,
}

impl OplogRetentionPlan {
    pub fn bytes(&self) -> u64 {
        self.prunable
            .iter()
            .map(OplogRetentionTarget::bytes)
            .fold(0, u64::saturating_add)
    }
}

/// Decide which canonical oplogs can be removed. **Reads only.**
///
/// Cataloguing happens one day at a time. `catalog_oplogs` intentionally treats a
/// malformed or wrong-kind reserved candidate as an all-or-nothing day failure; a
/// per-day call confines that fail-closed result to the affected partition.
pub fn plan_oplog_retention(
    journal: &Path,
    policy: &LogPolicy,
    today: NaiveDate,
) -> OplogRetentionPlan {
    plan_oplog_retention_with_probe(journal, policy, today, probe_retained_oplog_lease)
}

fn plan_oplog_retention_with_probe(
    journal: &Path,
    policy: &LogPolicy,
    today: NaiveDate,
    probe: impl Fn(&File, OplogFileIdentity) -> LeaseProbe,
) -> OplogRetentionPlan {
    let Some(cutoff) = cutoff(policy, today) else {
        return OplogRetentionPlan::default();
    };
    let mut plan = OplogRetentionPlan::default();

    for day in chronicle_days(journal) {
        if day >= cutoff {
            plan.retained.push(RetainedOplog {
                day,
                leaf: None,
                reason: OplogRetentionKept::TooYoung,
            });
            continue;
        }
        let Ok(root) = JournalRoot::open(journal) else {
            plan.retained.push(RetainedOplog {
                day,
                leaf: None,
                reason: OplogRetentionKept::DayUnavailable {
                    kind: "oplog_catalog_root".to_owned(),
                },
            });
            continue;
        };
        let snapshot = match catalog_oplogs(root, &[day]) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                plan.retained.push(RetainedOplog {
                    day,
                    leaf: None,
                    reason: OplogRetentionKept::DayUnavailable {
                        kind: error.kind().to_owned(),
                    },
                });
                continue;
            }
        };
        for (entry, file) in snapshot.into_catalogued_entries() {
            let Some(leaf) = entry.leaf().to_str() else {
                plan.retained.push(RetainedOplog {
                    day,
                    leaf: None,
                    reason: OplogRetentionKept::DayUnavailable {
                        kind: "oplog_catalog_utf8".to_owned(),
                    },
                });
                continue;
            };
            let written_recently = entry
                .modified()
                .and_then(utc_day)
                .is_some_and(|written| written >= cutoff);
            match probe(&file, entry.identity()) {
                LeaseProbe::Released if written_recently => plan.retained.push(RetainedOplog {
                    day,
                    leaf: Some(leaf.to_owned()),
                    reason: OplogRetentionKept::TooYoung,
                }),
                LeaseProbe::Released => plan.prunable.push(OplogRetentionTarget {
                    day,
                    leaf: leaf.to_owned(),
                    identity: entry.identity(),
                    bytes: entry.size(),
                }),
                LeaseProbe::Active => plan.retained.push(RetainedOplog {
                    day,
                    leaf: Some(leaf.to_owned()),
                    reason: OplogRetentionKept::Active,
                }),
                LeaseProbe::Indeterminate => plan.retained.push(RetainedOplog {
                    day,
                    leaf: Some(leaf.to_owned()),
                    reason: OplogRetentionKept::Indeterminate,
                }),
            }
        }
    }
    plan
}

/// The UTC day of a last write, dated the way the mtime log classes date theirs.
fn utc_day(when: SystemTime) -> Option<NaiveDate> {
    let seconds = when.duration_since(UNIX_EPOCH).ok()?.as_secs();
    DateTime::from_timestamp(i64::try_from(seconds).ok()?, 0).map(|when| when.date_naive())
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::unwrap_used,
    reason = "fixture creation for the deterministic retained-lease probe"
)]
mod tests {
    use chrono::{FixedOffset, TimeZone};
    use solstone_core_journal_io::operational_log::{OplogFormat, create_oplog_at};

    use super::*;

    fn day(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).expect("day")
    }

    fn policy() -> LogPolicy {
        LogPolicy {
            days: 7,
            enabled: true,
        }
    }

    /// One released oplog in the 2026-01-01 partition, last written on `written`.
    fn january_oplog(journal: &Path, written: NaiveDate) -> String {
        let instant = FixedOffset::east_opt(0)
            .expect("UTC")
            .with_ymd_and_hms(2026, 1, 1, 12, 0, 0)
            .single()
            .expect("instant");
        let leaf = {
            let writer = create_oplog_at(
                JournalRoot::open(journal).expect("journal root"),
                "source",
                "run",
                OplogFormat::Log,
                instant,
            )
            .expect("oplog");
            writer.leaf_name().to_owned()
        };
        std::fs::OpenOptions::new()
            .write(true)
            .open(journal.join("chronicle/20260101/health").join(&leaf))
            .expect("oplog")
            .set_modified(SystemTime::from(
                written.and_hms_opt(12, 0, 0).expect("noon").and_utc(),
            ))
            .expect("last write");
        leaf
    }

    #[test]
    fn an_indeterminate_probe_keeps_an_old_catalogued_oplog() {
        let journal = tempfile::tempdir().expect("journal");
        let leaf = january_oplog(journal.path(), day(2026, 1, 1));

        let plan =
            plan_oplog_retention_with_probe(journal.path(), &policy(), day(2026, 8, 5), |_, _| {
                LeaseProbe::Indeterminate
            });
        assert!(plan.prunable.is_empty(), "{plan:?}");
        assert!(plan.retained.iter().any(|retained| {
            retained.day() == day(2026, 1, 1)
                && retained.leaf() == Some(leaf.as_str())
                && retained.reason() == &OplogRetentionKept::Indeterminate
        }));
    }

    /// Re-running a past day writes its logs into that day. Those logs are as
    /// young as the run, so the next prune keeps them; an old run's are removed.
    #[test]
    fn a_past_day_run_inside_the_window_keeps_its_logs() {
        let journal = tempfile::tempdir().expect("journal");
        let leaf = january_oplog(journal.path(), day(2026, 8, 4));

        let plan =
            plan_oplog_retention_with_probe(journal.path(), &policy(), day(2026, 8, 5), |_, _| {
                LeaseProbe::Released
            });
        assert!(plan.prunable.is_empty(), "{plan:?}");
        assert!(plan.retained.iter().any(|retained| {
            retained.day() == day(2026, 1, 1)
                && retained.leaf() == Some(leaf.as_str())
                && retained.reason() == &OplogRetentionKept::TooYoung
        }));

        let later =
            plan_oplog_retention_with_probe(journal.path(), &policy(), day(2026, 8, 20), |_, _| {
                LeaseProbe::Released
            });
        assert_eq!(
            later
                .prunable
                .iter()
                .map(OplogRetentionTarget::leaf)
                .collect::<Vec<_>>(),
            vec![leaf.as_str()],
            "{later:?}"
        );
    }
}
