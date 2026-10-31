// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The owner's asks to think a corrected day again.
//!
//! An ask names a day, the activities on it to run again and whether that
//! day's morning briefing may regenerate even though its morning has passed.
//! It is its own record: the per-day owner work intent never carries it, and
//! nothing here writes a daily unit record. Work discovery reads the asks,
//! compares their creation time with timestamps already on disk, and changes
//! an ask only by id, under the store lock, against the values it read.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use solstone_core_journal_io::durability::{ArtifactId, artifact};
use solstone_core_journal_io::{JsonWriteOptions, LockOptions, hold_lock};

/// The store version this journal reads and writes.
pub const STORE_VERSION: u32 = 1;

/// One activity an ask runs again.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AskedActivity {
    pub facet: String,
    pub activity: String,
}

/// How far an ask's week page step has come. Only attempts are counted here;
/// whether the page is owed is read from the page itself.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WeekStep {
    #[serde(default)]
    pub attempts: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub counted_at_ms: Option<i64>,
    /// Why the step ended, once it has.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended: Option<String>,
}

/// The reason a week step ends when the page was made after the ask.
pub const WEEK_BUILT: &str = "built";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OwnerAsk {
    pub id: String,
    pub day: String,
    pub created_at_ms: i64,
    /// Activities not yet handed to their activity work.
    pub to_seed: BTreeSet<AskedActivity>,
    #[serde(default)]
    pub seeded: BTreeSet<AskedActivity>,
    /// Whether the day's briefing may regenerate though its morning has passed.
    pub thaw: bool,
    /// One way: once set, the briefing reads frozen again for this ask.
    #[serde(default)]
    pub thaw_done: bool,
    /// The thaw ended on a briefing that reached its failure cap.
    #[serde(default)]
    pub thaw_capped: bool,
    /// Activities that could not be run again, counted by reason.
    #[serde(default)]
    pub dropped: BTreeMap<String, u64>,
    #[serde(default)]
    pub week: WeekStep,
}

impl OwnerAsk {
    /// Every step this ask asked for has finished.
    pub fn finished(&self) -> bool {
        self.to_seed.is_empty() && (!self.thaw || self.thaw_done) && self.week.ended.is_some()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AskError {
    #[error("invalid work day")]
    InvalidDay,
    #[error("invalid activity identity")]
    InvalidActivity,
    #[error("an ask must name an activity or thaw the briefing")]
    Empty,
    #[error("owner asks are unreadable: {0}")]
    Unreadable(String),
    #[error("owner ask could not be saved: {0}")]
    Write(String),
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Store {
    version: u32,
    asks: BTreeMap<String, OwnerAsk>,
}

#[derive(Deserialize)]
struct VersionProbe {
    version: u32,
}

fn path(journal: &Path) -> PathBuf {
    journal.join(artifact(ArtifactId::OwnerAsks).path)
}

fn valid_day(day: &str) -> bool {
    day.len() == 8
        && day.bytes().all(|c| c.is_ascii_digit())
        && chrono::NaiveDate::parse_from_str(day, "%Y%m%d").is_ok()
}

/// The identity rule activity work applies to its own records.
fn valid_activity(day: &str, asked: &AskedActivity) -> bool {
    crate::work_retry::Unit::Activity {
        day: day.to_owned(),
        facet: asked.facet.clone(),
        activity: asked.activity.clone(),
        talent: "activity".to_owned(),
    }
    .validate()
    .is_ok()
}

/// Every pending ask by id. No file is no asks; a damaged or newer store is
/// an error and is left exactly as it is.
pub fn read(journal: &Path) -> Result<BTreeMap<String, OwnerAsk>, String> {
    let bytes = match std::fs::read(path(journal)) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(error) => return Err(error.to_string()),
    };
    let probe: VersionProbe = serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
    if probe.version != STORE_VERSION {
        return Err(format!("unsupported owner ask version {}", probe.version));
    }
    let store: Store = serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
    for (id, ask) in &store.asks {
        if *id != ask.id
            || id.len() != 32
            || !id.bytes().all(|c| c.is_ascii_hexdigit())
            || !valid_day(&ask.day)
            || ask
                .to_seed
                .iter()
                .chain(&ask.seeded)
                .any(|asked| !valid_activity(&ask.day, asked))
        {
            return Err("unrecognized owner ask".into());
        }
    }
    Ok(store.asks)
}

/// Whether some ask on `day` still lets its past briefing regenerate. An
/// unreadable store thaws nothing.
pub fn thawed(journal: &Path, day: &str) -> bool {
    read(journal).is_ok_and(|asks| {
        asks.values()
            .any(|ask| ask.day == day && ask.thaw && !ask.thaw_done)
    })
}

fn mutate<T>(
    journal: &Path,
    update: impl FnOnce(&mut BTreeMap<String, OwnerAsk>) -> Result<(T, bool), String>,
) -> Result<T, AskError> {
    let path = path(journal);
    std::fs::create_dir_all(path.parent().expect("health parent"))
        .map_err(|error| AskError::Write(error.to_string()))?;
    let _lock = hold_lock(path.with_extension("lock"), LockOptions::default())
        .map_err(|error| AskError::Write(error.to_string()))?;
    let mut asks = read(journal).map_err(AskError::Unreadable)?;
    let (result, changed) = update(&mut asks).map_err(AskError::Write)?;
    if !changed {
        return Ok(result);
    }
    publish(&path, asks).map_err(AskError::Write)?;
    Ok(result)
}

fn publish(path: &Path, asks: BTreeMap<String, OwnerAsk>) -> Result<(), String> {
    let store = Store {
        version: STORE_VERSION,
        asks,
    };
    #[cfg(unix)]
    {
        let parent = std::fs::File::open(path.parent().expect("health parent"))
            .map_err(|e| e.to_string())?;
        let bytes = solstone_core_journal_io::atomic::serialize_json_bytes(
            &store,
            JsonWriteOptions::default(),
        )
        .map_err(|e| e.to_string())?;
        match solstone_core_journal_io::atomic::atomic_replace_bound_checked(
            &parent,
            path.file_name().expect("owner asks name"),
            &bytes,
            0o600,
            &|| Ok(()),
        )
        .map_err(|e| e.to_string())?
        {
            solstone_core_journal_io::atomic::BoundAtomicOutcome::Published { .. } => {}
            other => return Err(format!("owner ask publication is uncertain: {other:?}")),
        }
    }
    #[cfg(windows)]
    solstone_core_journal_io::write_json_checked(
        path,
        &store,
        JsonWriteOptions::default(),
        &|| Ok(()),
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Save a new ask. It never merges with another ask, and it is refused
/// before anything is written when it is invalid or the store is unreadable.
pub fn record(
    journal: &Path,
    day: &str,
    activities: &[AskedActivity],
    thaw: bool,
    now_ms: i64,
) -> Result<OwnerAsk, AskError> {
    if !valid_day(day) {
        return Err(AskError::InvalidDay);
    }
    if activities.iter().any(|asked| !valid_activity(day, asked)) {
        return Err(AskError::InvalidActivity);
    }
    if activities.is_empty() && !thaw {
        return Err(AskError::Empty);
    }
    let mut random = [0u8; 16];
    getrandom::fill(&mut random).map_err(|error| AskError::Write(error.to_string()))?;
    let ask = OwnerAsk {
        id: random.iter().map(|byte| format!("{byte:02x}")).collect(),
        day: day.to_owned(),
        created_at_ms: now_ms,
        to_seed: activities.iter().cloned().collect(),
        seeded: BTreeSet::new(),
        thaw,
        thaw_done: false,
        thaw_capped: false,
        dropped: BTreeMap::new(),
        week: WeekStep::default(),
    };
    let saved = ask.clone();
    mutate(journal, |asks| {
        asks.insert(saved.id.clone(), saved);
        Ok(((), true))
    })?;
    Ok(ask)
}

/// The one call an owner correction makes once it has committed: record the
/// ask, then wake work discovery. A wakeup that cannot be sent is not an
/// error; the next pass finds the ask.
pub fn think_again(
    journal: &Path,
    day: &str,
    activities: &[AskedActivity],
    thaw: bool,
) -> Result<OwnerAsk, AskError> {
    let ask = record(
        journal,
        day,
        activities,
        thaw,
        chrono::Utc::now().timestamp_millis(),
    )?;
    wake(journal, day);
    Ok(ask)
}

fn wake(journal: &Path, day: &str) {
    let envelope = solstone_core_callosum::CallosumEnvelope::supervisor_drain(day);
    let Ok(mut line) = serde_json::to_string(&envelope) else {
        return;
    };
    line.push('\n');
    if let Err(error) = solstone_core_callosum::CallosumOneShotSender::new(
        journal.join("health").join("callosum.sock"),
        Duration::from_secs(1),
    )
    .send_line(&line)
    {
        log::debug!("owner ask saved; work discovery will find it on its next pass: {error}");
    }
}

/// Change one ask under the store lock. `change` sees the ask as it is now
/// and compares it with what its caller read: `None` leaves it, `Some(None)`
/// removes it and `Some(Some(ask))` replaces it. Returns whether it changed.
pub fn update(
    journal: &Path,
    id: &str,
    change: impl FnOnce(&OwnerAsk) -> Option<Option<OwnerAsk>>,
) -> Result<bool, String> {
    mutate(journal, |asks| {
        let Some(current) = asks.get(id) else {
            return Ok((false, false));
        };
        match change(current) {
            None => Ok((false, false)),
            Some(None) => {
                asks.remove(id);
                Ok((true, true))
            }
            Some(Some(next)) => {
                if next.id != id || next.day != current.day {
                    return Err("an ask keeps its id and day".into());
                }
                let changed = next != *current;
                asks.insert(id.to_owned(), next);
                Ok((changed, changed))
            }
        }
    })
    .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asked(facet: &str, activity: &str) -> AskedActivity {
        AskedActivity {
            facet: facet.into(),
            activity: activity.into(),
        }
    }

    #[test]
    fn no_store_is_no_asks_and_nothing_thawed() {
        let root = tempfile::tempdir().unwrap();
        assert!(read(root.path()).unwrap().is_empty());
        assert!(!thawed(root.path(), "20260101"));
    }

    #[test]
    fn asks_never_merge_and_only_a_pending_thaw_thaws() {
        let root = tempfile::tempdir().unwrap();
        let first = record(root.path(), "20260101", &[asked("work", "a")], false, 1).unwrap();
        let second = record(root.path(), "20260101", &[asked("work", "a")], true, 2).unwrap();
        assert_ne!(first.id, second.id);
        assert_eq!(read(root.path()).unwrap().len(), 2);
        assert!(thawed(root.path(), "20260101"));
        assert!(!thawed(root.path(), "20260102"));
        update(root.path(), &second.id, |current| {
            let mut next = current.clone();
            next.thaw_done = true;
            Some(Some(next))
        })
        .unwrap();
        assert!(!thawed(root.path(), "20260101"));
    }

    #[test]
    fn refusals_write_nothing() {
        let root = tempfile::tempdir().unwrap();
        for day in ["2026010", "20261301", "2026-01-01", ""] {
            assert_eq!(
                record(root.path(), day, &[asked("work", "a")], true, 1),
                Err(AskError::InvalidDay)
            );
        }
        for (facet, activity) in [
            ("wo/rk", "a"),
            ("work", "a\\b"),
            (".", "a"),
            ("work", ".."),
            ("work", "a\0"),
            ("", "a"),
            ("work", ""),
        ] {
            assert_eq!(
                record(root.path(), "20260101", &[asked(facet, activity)], true, 1),
                Err(AskError::InvalidActivity)
            );
        }
        assert_eq!(
            record(root.path(), "20260101", &[], false, 1),
            Err(AskError::Empty)
        );
        assert!(!root.path().join("health").exists());
        assert!(record(root.path(), "20260101", &[], true, 1).is_ok());
    }

    #[test]
    fn a_damaged_or_newer_store_is_kept_thaws_nothing_and_refuses_new_asks() {
        for bytes in [
            "{".to_owned(),
            serde_json::json!({"version": STORE_VERSION + 1, "asks": {}}).to_string(),
        ] {
            let root = tempfile::tempdir().unwrap();
            let ask = record(root.path(), "20260101", &[], true, 1).unwrap();
            assert!(thawed(root.path(), "20260101"));
            std::fs::write(path(root.path()), &bytes).unwrap();
            assert!(read(root.path()).is_err());
            assert!(!thawed(root.path(), "20260101"));
            assert!(matches!(
                record(root.path(), "20260101", &[], true, 2),
                Err(AskError::Unreadable(_))
            ));
            assert!(update(root.path(), &ask.id, |_| Some(None)).is_err());
            assert_eq!(std::fs::read_to_string(path(root.path())).unwrap(), bytes);
        }
    }

    #[test]
    fn a_change_between_read_and_clear_is_never_lost() {
        let root = tempfile::tempdir().unwrap();
        let a = record(root.path(), "20260101", &[asked("work", "a")], false, 1).unwrap();
        let read_a = read(root.path()).unwrap()[&a.id].clone();
        // Another ask, and a new name on A, land after the read.
        let b = record(root.path(), "20260101", &[asked("work", "b")], true, 2).unwrap();
        let mut grown = read_a.clone();
        grown.to_seed.insert(asked("work", "c"));
        let injected = grown.clone();
        update(root.path(), &a.id, move |_| Some(Some(injected))).unwrap();
        // Clearing A as it was read refuses; moving the name it seeded keeps the new one.
        let stale = read_a.clone();
        assert!(
            !update(root.path(), &a.id, |current| (*current == stale)
                .then_some(None))
            .unwrap()
        );
        let seeded = asked("work", "a");
        assert!(
            update(root.path(), &a.id, |current| {
                let mut next = current.clone();
                next.to_seed.remove(&seeded).then(|| {
                    next.seeded.insert(seeded.clone());
                    Some(next)
                })
            })
            .unwrap()
        );
        let asks = read(root.path()).unwrap();
        assert_eq!(asks[&b.id], b);
        assert_eq!(asks[&a.id].to_seed, BTreeSet::from([asked("work", "c")]));
        assert_eq!(asks[&a.id].seeded, BTreeSet::from([asked("work", "a")]));
    }

    #[cfg(feature = "full-tests")]
    #[test]
    fn a_wakeup_that_cannot_be_sent_is_not_an_error() {
        let root = tempfile::tempdir().unwrap();
        let ask = think_again(root.path(), "20260101", &[asked("work", "a")], true).unwrap();
        assert_eq!(read(root.path()).unwrap()[&ask.id], ask);
    }
}
