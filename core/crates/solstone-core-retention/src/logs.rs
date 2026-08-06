// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Operational log and cache retention: one scanner over a table of classes.
//!
//! # Eleven bespoke functions, or one table
//!
//! The reference has a separate scanner per class. Four of them already delegate to a
//! shared one, and reading the other six shows why: every class is the same operation
//! parameterised three ways — *where to look*, *how to date an entry*, and *what kind
//! of entry to remove*. So this is a table, and adding a class is a row.
//!
//! That is not tidiness. A per-class function is a per-class place to forget the
//! containment check, the symlink check, or the exemption — and the reference has
//! eleven of them, one of which forgot something (below).
//!
//! # 🔴 The subsystem that prunes logs did not prune its own
//!
//! `health/pruning-runs/<day>.jsonl` is written by every prune run and pruned by
//! none. The class that looks like it should cover it, `chronicle_health_logs`, walks
//! `chronicle/<day>/health/` — not the journal root's `health/`. So it grew without
//! bound on every owner's disk. It is a **row in the table below**.
//!
//! ⚠ `health/retention.log` has the same defect and is *not* fixed here: it is one
//! append-only file rather than a set of dated ones, so it needs line-date compaction
//! rather than deletion. Named in the outcome, not silently omitted.
//!
//! # ⛔ Two classes reach inside the chronicle
//!
//! `chronicle_health_logs` prunes `chronicle/<day>/health/`, which sits beside the
//! owner's streams. A glob that walked one directory too far would delete recordings,
//! and the only thing standing between those two outcomes is that `health` is a
//! reserved name a stream cannot take. So this module refuses to remove anything it
//! cannot place under a declared class root, and a test asserts that a plan over a
//! journal holding real segments names none of their files.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Datelike, NaiveDate, Utc};
use solstone_core_journal_io::paths::{DirEntryKind, list_dir_entries};

/// Where a class's entries live, relative to the journal root.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Location {
    /// Entries directly inside one fixed directory.
    Fixed(&'static str),
    /// Entries inside `<base>/<*>[/<tail>]`, one level of expansion.
    ///
    /// ⛔ Exactly one level. A recursive walk is how a log pruner reaches content it
    /// was never meant to see.
    Expand {
        base: &'static str,
        tail: Option<&'static str>,
    },
}

/// How an entry's day is determined.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DatedBy {
    /// The expanded directory component, as `YYYYMMDD`.
    ExpandedDir,
    /// The filename stem, as `YYYYMMDD`.
    Stem,
    /// The filename stem, as epoch milliseconds.
    StemEpochMillis,
    /// The entry's own modification time.
    ///
    /// ⚠ The weakest dating available, and used only where there is nothing else: a
    /// cache directory whose name carries no date. A restored copy reads as young,
    /// which keeps it.
    Mtime,
}

/// What kind of entry a class removes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EntryKind {
    File,
    Directory,
}

/// One class of prunable operational data.
#[derive(Clone, Copy, Debug)]
pub struct Class {
    pub name: &'static str,
    pub location: Location,
    /// Extensions an entry must carry, or empty for any.
    ///
    /// ⚠ Extensions rather than glob patterns: every pattern the reference uses is
    /// `*.<ext>`, and a glob engine here would be a second place for a path to be
    /// interpreted.
    pub extensions: &'static [&'static str],
    pub dated_by: DatedBy,
    pub entry: EntryKind,
    /// Stem suffixes never pruned, however old.
    pub exempt_stem_suffixes: &'static [&'static str],
}

/// Every class, mirroring the reference's `CLASS_NAMES` plus the one it omitted.
///
/// ⚠ The reference declares **ten** classes and performs an eleventh operation
/// (compacting the root task log) that is not one. The count matters only because a
/// plan asserts it covered every row.
pub const CLASSES: &[Class] = &[
    Class {
        name: "chronicle_health_logs",
        location: Location::Expand {
            base: "chronicle",
            tail: Some("health"),
        },
        extensions: &["log", "jsonl"],
        dated_by: DatedBy::ExpandedDir,
        entry: EntryKind::File,
        exempt_stem_suffixes: &[],
    },
    Class {
        name: "talent_run_logs",
        location: Location::Expand {
            base: "talents",
            tail: None,
        },
        extensions: &["jsonl"],
        dated_by: DatedBy::StemEpochMillis,
        entry: EntryKind::File,
        // ⛔ A live run's log. Deleting it truncates a talent mid-run.
        exempt_stem_suffixes: &["_active"],
    },
    Class {
        name: "talent_day_index",
        location: Location::Fixed("talents"),
        extensions: &["jsonl"],
        dated_by: DatedBy::Stem,
        entry: EntryKind::File,
        exempt_stem_suffixes: &[],
    },
    Class {
        name: "cogitate_history_cache",
        location: Location::Fixed(".cache/cogitate-history"),
        extensions: &[],
        dated_by: DatedBy::Mtime,
        entry: EntryKind::Directory,
        exempt_stem_suffixes: &[],
    },
    Class {
        name: "tokens",
        location: Location::Fixed("tokens"),
        extensions: &["jsonl"],
        dated_by: DatedBy::Stem,
        entry: EntryKind::File,
        exempt_stem_suffixes: &[],
    },
    Class {
        name: "local_inference",
        location: Location::Fixed("health/local-inference"),
        extensions: &["jsonl"],
        dated_by: DatedBy::Stem,
        entry: EntryKind::File,
        exempt_stem_suffixes: &[],
    },
    Class {
        name: "awareness_logs",
        location: Location::Fixed("awareness"),
        extensions: &["jsonl"],
        dated_by: DatedBy::Stem,
        entry: EntryKind::File,
        exempt_stem_suffixes: &[],
    },
    Class {
        name: "config_actions",
        location: Location::Fixed("config/actions"),
        extensions: &["jsonl"],
        dated_by: DatedBy::Stem,
        entry: EntryKind::File,
        exempt_stem_suffixes: &[],
    },
    Class {
        name: "facet_logs",
        location: Location::Expand {
            base: "facets",
            tail: Some("logs"),
        },
        extensions: &["jsonl"],
        dated_by: DatedBy::Stem,
        entry: EntryKind::File,
        exempt_stem_suffixes: &[],
    },
    Class {
        name: "observer_history",
        location: Location::Expand {
            base: "apps/observer/observers",
            tail: Some("hist"),
        },
        extensions: &["jsonl"],
        dated_by: DatedBy::Stem,
        entry: EntryKind::File,
        exempt_stem_suffixes: &[],
    },
    // 🔴 The row the reference does not have.
    Class {
        name: "pruning_runs",
        location: Location::Fixed("health/pruning-runs"),
        extensions: &["jsonl"],
        dated_by: DatedBy::Stem,
        entry: EntryKind::File,
        exempt_stem_suffixes: &[],
    },
];

/// A log entry the planner has judged prunable.
///
/// ⛔ Only this module can build one, and only from a class's declared location, so
/// the removal verb cannot be handed a path that no class produced. The chronicle
/// sits under one of those locations, which is why the guarantee is structural rather
/// than a check at the call site.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LogTarget {
    class: &'static str,
    rel: String,
    kind: EntryKind,
    day: NaiveDate,
    bytes: u64,
}

impl LogTarget {
    pub fn class(&self) -> &'static str {
        self.class
    }
    pub fn rel(&self) -> &str {
        &self.rel
    }
    pub fn kind(&self) -> EntryKind {
        self.kind
    }
    pub fn day(&self) -> NaiveDate {
        self.day
    }
    pub fn bytes(&self) -> u64 {
        self.bytes
    }
}

/// Why an entry was examined and kept.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Kept {
    /// Inside the retention window.
    TooYoung(NaiveDate),
    /// Exempt whatever its age.
    Exempt,
    /// ⚠ Could not be dated. Kept, and named: an undateable log is not an old one.
    Undateable,
    /// Not the kind of entry this class removes, or not a matching extension.
    NotAMatch,
}

/// One examined-and-kept entry.
#[derive(Clone, Debug)]
pub struct Retained {
    pub class: &'static str,
    pub rel: String,
    pub reason: Kept,
}

/// What a log-retention pass would do, before it does anything.
#[derive(Clone, Debug, Default)]
pub struct LogPlan {
    pub prunable: Vec<LogTarget>,
    pub retained: Vec<Retained>,
    /// Classes whose root does not exist. ⚠ Ordinary, and reported so a plan that
    /// found nothing can be told apart from a journal laid out differently.
    pub absent_classes: Vec<&'static str>,
}

impl LogPlan {
    pub fn examined(&self) -> usize {
        self.prunable.len().saturating_add(self.retained.len())
    }

    pub fn bytes(&self) -> u64 {
        self.prunable
            .iter()
            .map(LogTarget::bytes)
            .fold(0u64, u64::saturating_add)
    }

    /// Prunable entries of one class.
    pub fn by_class(&self, class: &str) -> Vec<&LogTarget> {
        self.prunable
            .iter()
            .filter(|target| target.class == class)
            .collect()
    }
}

/// The configured log-retention window.
#[derive(Clone, Copy, Debug, Default)]
pub struct LogPolicy {
    pub days: u32,
    /// 🔴 Off unless explicitly armed, like the media policy.
    pub enabled: bool,
}

// ⛔ `Default` is derived, and both defaults matter: `enabled: false` keeps the
// destructive path off, and `days: 0` is an unset window that keeps rather than a
// zero-day window that prunes everything. `plan` enforces the second.

/// Parse a `YYYYMMDD` component.
fn parse_day(text: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(text, "%Y%m%d").ok()
}

/// Parse an epoch-millisecond stem into a UTC date.
///
/// ⚠ The reference reads this in local time. This reads UTC, which can differ by one
/// day near midnight — always in the direction of the file appearing to belong to a
/// different day, never of an in-window file falling out of the window by more than
/// that day. Recorded rather than hidden.
fn parse_epoch_millis(text: &str) -> Option<NaiveDate> {
    let millis: i64 = text.parse().ok()?;
    Some(
        DateTime::<Utc>::UNIX_EPOCH
            .checked_add_signed(chrono::TimeDelta::try_milliseconds(millis)?)?
            .date_naive(),
    )
}

/// The stem and extension of a filename.
fn split_name(name: &str) -> (&str, Option<&str>) {
    match name.rsplit_once('.') {
        // A leading-dot name is a hidden file, not an extension.
        Some((stem, extension)) if !stem.is_empty() => (stem, Some(extension)),
        _ => (name, None),
    }
}

/// Every directory a class's entries are found in, with the expanded component.
fn class_roots(journal: &Path, class: &Class) -> Option<Vec<(PathBuf, Option<String>)>> {
    match class.location {
        Location::Fixed(rel) => {
            let path = journal.join(rel);
            path.is_dir().then(|| vec![(path, None)])
        }
        Location::Expand { base, tail } => {
            let base_path = journal.join(base);
            if !base_path.is_dir() {
                return None;
            }
            let entries = list_dir_entries(&base_path).ok()?;
            let mut roots = Vec::new();
            for entry in entries {
                // ⛔ Directories only, and never a symlink: `DirEntryKind` reports
                // kinds without following links, so a link out of the journal
                // cannot become a pruning root.
                if entry.kind != DirEntryKind::Directory {
                    continue;
                }
                let Some(name) = entry.name.to_str() else {
                    continue;
                };
                let path = match tail {
                    Some(tail) => entry.path.join(tail),
                    None => entry.path.clone(),
                };
                if path.is_dir() {
                    roots.push((path, Some(name.to_owned())));
                }
            }
            (!roots.is_empty()).then_some(roots)
        }
    }
}

/// Decide what a log-retention pass would remove. **Reads only.**
pub fn plan(journal: &Path, policy: &LogPolicy, today: NaiveDate) -> LogPlan {
    let mut built = LogPlan::default();
    if !policy.enabled || policy.days == 0 {
        // ⛔ `days: 0` is not "prune everything". An unset window keeps.
        return built;
    }
    let Some(cutoff) = today.checked_sub_days(chrono::Days::new(u64::from(policy.days))) else {
        return built;
    };

    for class in CLASSES {
        let Some(roots) = class_roots(journal, class) else {
            built.absent_classes.push(class.name);
            continue;
        };
        for (root, expanded) in roots {
            let Ok(entries) = list_dir_entries(&root) else {
                continue;
            };
            for entry in entries {
                let Some(name) = entry.name.to_str() else {
                    continue;
                };
                let rel = match entry.path.strip_prefix(journal) {
                    Ok(rel) => rel.to_string_lossy().into_owned(),
                    Err(_) => continue,
                };
                let retained = |reason| Retained {
                    class: class.name,
                    rel: rel.clone(),
                    reason,
                };

                let wanted = match class.entry {
                    EntryKind::File => DirEntryKind::File,
                    EntryKind::Directory => DirEntryKind::Directory,
                };
                if entry.kind != wanted {
                    built.retained.push(retained(Kept::NotAMatch));
                    continue;
                }
                let (stem, extension) = split_name(name);
                if !class.extensions.is_empty()
                    && !extension.is_some_and(|extension| {
                        class
                            .extensions
                            .iter()
                            .any(|wanted| wanted.eq_ignore_ascii_case(extension))
                    })
                {
                    built.retained.push(retained(Kept::NotAMatch));
                    continue;
                }
                if class
                    .exempt_stem_suffixes
                    .iter()
                    .any(|suffix| stem.ends_with(suffix))
                {
                    built.retained.push(retained(Kept::Exempt));
                    continue;
                }

                let day = match class.dated_by {
                    DatedBy::ExpandedDir => expanded.as_deref().and_then(parse_day),
                    DatedBy::Stem => parse_day(stem),
                    DatedBy::StemEpochMillis => parse_epoch_millis(stem),
                    DatedBy::Mtime => entry
                        .path
                        .symlink_metadata()
                        .ok()
                        .and_then(|meta| meta.modified().ok())
                        .and_then(|when| {
                            let seconds =
                                when.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs();
                            DateTime::from_timestamp(i64::try_from(seconds).ok()?, 0)
                                .map(|when| when.date_naive())
                        }),
                };
                // ⛔ Fail closed. An entry that cannot be dated is kept and named.
                let Some(day) = day else {
                    built.retained.push(retained(Kept::Undateable));
                    continue;
                };
                if day >= cutoff {
                    built.retained.push(retained(Kept::TooYoung(day)));
                    continue;
                }
                let bytes = entry
                    .path
                    .symlink_metadata()
                    .map_or(0, |meta| if meta.is_file() { meta.len() } else { 0 });
                built.prunable.push(LogTarget {
                    class: class.name,
                    rel,
                    kind: class.entry,
                    day,
                    bytes,
                });
            }
        }
    }
    built
}

/// The cutoff a window implies, for a caller that wants to explain a plan.
pub fn cutoff(policy: &LogPolicy, today: NaiveDate) -> Option<NaiveDate> {
    (policy.enabled && policy.days > 0)
        .then(|| today.checked_sub_days(chrono::Days::new(u64::from(policy.days))))
        .flatten()
}

/// The day a target's name encodes, formatted the way the journal writes days.
pub fn day_key(day: NaiveDate) -> String {
    format!("{:04}{:02}{:02}", day.year(), day.month(), day.day())
}
