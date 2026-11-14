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
    /// Why the journal asked rather than the owner correcting something:
    /// `trash:<operation>` when recordings on the day went to the trash or
    /// came back. Such an ask merges per cause and day, keeps what was made
    /// from the day out of search while it is pending, and never gives up.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cause: Option<String>,
    /// The trash items whose change this ask carries, so a re-run of one
    /// item's change adds nothing while another item's change asks again.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub items: BTreeSet<String>,
}

/// The cause prefix of an ask the trash files.
pub use solstone_core_journal_io::trash_asks::TRASH_CAUSE;

impl OwnerAsk {
    /// The trash filed this ask: something on its day left or came back.
    pub fn from_trash(&self) -> bool {
        self.cause
            .as_deref()
            .is_some_and(|cause| cause.starts_with(TRASH_CAUSE))
    }

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
        cause: None,
        items: BTreeSet::new(),
    };
    let saved = ask.clone();
    mutate(journal, |asks| {
        asks.insert(saved.id.clone(), saved);
        Ok(((), true))
    })?;
    Ok(ask)
}

/// Ask the journal to make `day` again because trash item `item`, part of
/// `operation`, changed recordings on it: run `activities` again, let the
/// day's briefing regenerate however old it is, and re-make its week's page.
///
/// One ask per operation and day. The same item asking again (a stopped
/// change finished by the sweep) only adds activities not yet asked; another
/// item of the operation reopens the ask from now, since the day changed
/// again after whatever the ask may already have made.
pub fn record_for_trash(
    journal: &Path,
    day: &str,
    operation: &str,
    item: &str,
    activities: &[AskedActivity],
    now_ms: i64,
) -> Result<OwnerAsk, AskError> {
    if !valid_day(day) {
        return Err(AskError::InvalidDay);
    }
    if activities.iter().any(|asked| !valid_activity(day, asked)) {
        return Err(AskError::InvalidActivity);
    }
    let cause = format!("{TRASH_CAUSE}{operation}");
    let mut random = [0u8; 16];
    getrandom::fill(&mut random).map_err(|error| AskError::Write(error.to_string()))?;
    let fresh_id: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
    mutate(journal, |asks| {
        let existing = asks
            .values()
            .find(|ask| ask.day == day && ask.cause.as_deref() == Some(cause.as_str()))
            .map(|ask| ask.id.clone());
        let Some(id) = existing else {
            let ask = OwnerAsk {
                id: fresh_id.clone(),
                day: day.to_owned(),
                created_at_ms: now_ms,
                to_seed: activities.iter().cloned().collect(),
                seeded: BTreeSet::new(),
                thaw: true,
                thaw_done: false,
                thaw_capped: false,
                dropped: BTreeMap::new(),
                week: WeekStep::default(),
                cause: Some(cause.clone()),
                items: BTreeSet::from([item.to_owned()]),
            };
            asks.insert(fresh_id.clone(), ask.clone());
            return Ok((ask, true));
        };
        let current = asks[&id].clone();
        let mut next = current.clone();
        for asked in activities {
            if !next.seeded.contains(asked) {
                next.to_seed.insert(asked.clone());
            }
        }
        if next.items.insert(item.to_owned()) {
            next.created_at_ms = next.created_at_ms.max(now_ms);
            next.thaw = true;
            next.thaw_done = false;
            next.thaw_capped = false;
            next.week = WeekStep::default();
            for asked in std::mem::take(&mut next.seeded) {
                next.to_seed.insert(asked);
            }
        }
        let changed = next != current;
        asks.insert(id, next.clone());
        Ok((next, changed))
    })
}

pub use solstone_core_journal_io::trash_asks::{TrashPending, trash_pending};

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

/// Wake work discovery for `day`. Best effort: the next pass finds the ask.
pub fn wake(journal: &Path, day: &str) {
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

impl AskError {
    /// The stable reason code a reply gives when an ask was not saved.
    #[must_use]
    pub fn reason_code(&self) -> &'static str {
        match self {
            Self::Unreadable(_) => "owner_asks_unreadable",
            _ => "owner_ask_unsaved",
        }
    }
}

/// Every activity on `day`, in any facet, whose stream and segments include
/// one of `segments`, each given as (stream, segment key). A facet whose
/// activities can't be read is skipped and logged.
pub fn activities_over(
    journal: &Path,
    day: &str,
    segments: &BTreeSet<(String, String)>,
) -> Result<Vec<AskedActivity>, String> {
    let mut activities = Vec::new();
    if segments.is_empty() {
        return Ok(activities);
    }
    for facet in
        solstone_core_facets::list_declared_facet_names(journal).map_err(|e| e.to_string())?
    {
        let records = match solstone_core_facets::load_activity_records(journal, &facet, day, true)
        {
            Ok(records) => records,
            Err(error) => {
                log::warn!("owner ask: activities of {facet} on {day} not read: {error}");
                continue;
            }
        };
        for record in records {
            let Some(stream) = record.get("stream").and_then(serde_json::Value::as_str) else {
                continue;
            };
            let holds = record
                .get("segments")
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(serde_json::Value::as_str)
                .any(|key| segments.contains(&(stream.to_owned(), key.to_owned())));
            let Some(id) = record.get("id").and_then(serde_json::Value::as_str) else {
                continue;
            };
            let asked = AskedActivity {
                facet: facet.clone(),
                activity: id.to_owned(),
            };
            if holds && valid_activity(day, &asked) {
                activities.push(asked);
            }
        }
    }
    activities.sort();
    activities.dedup();
    Ok(activities)
}

/// One segment whose speaker label bytes a committed change rewrote.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct RelabelledSegment {
    pub day: String,
    pub stream: String,
    pub segment_key: String,
}

/// How many closed days before today a broad label change still rewrites.
pub const RECENT_DAYS: i64 = 7;

/// After a committed change rewrote speaker labels across many segments, ask
/// the journal to think again the activities over them, for today and the
/// last [`RECENT_DAYS`] closed days in the owner's zone. Older days stay as
/// written, a day with no such activities gets no ask, and no briefing thaws.
/// A failed ask is logged and never fails the change.
pub fn ask_after_relabel(journal: &Path, changed: &[RelabelledSegment]) {
    let today = now()
        .with_timezone(&solstone_core_journal_config::owner_zone(journal))
        .date_naive();
    let earliest = today - chrono::Duration::days(RECENT_DAYS);
    let mut by_day = BTreeMap::<&str, BTreeSet<(String, String)>>::new();
    for segment in changed {
        let Ok(date) = chrono::NaiveDate::parse_from_str(&segment.day, "%Y%m%d") else {
            continue;
        };
        if date < earliest || date > today {
            continue;
        }
        by_day
            .entry(segment.day.as_str())
            .or_default()
            .insert((segment.stream.clone(), segment.segment_key.clone()));
    }
    for (day, segments) in by_day {
        let activities = match activities_over(journal, day, &segments) {
            Ok(activities) => activities,
            Err(error) => {
                log::warn!("relabel: activities on {day} not asked again: {error}");
                continue;
            }
        };
        if activities.is_empty() {
            continue;
        }
        if let Err(error) = ask_again(journal, day, &activities) {
            log::warn!("relabel: activities on {day} not asked again: {error}");
        }
    }
}

#[cfg(any(test, feature = "test-hooks"))]
thread_local! {
    static FIXED_NOW: std::cell::Cell<Option<chrono::DateTime<chrono::Utc>>> =
        const { std::cell::Cell::new(None) };
}

/// Run `body` with [`ask_after_relabel`]'s clock fixed at `now` on this
/// thread. Its asks are then recorded without waking work discovery.
#[cfg(any(test, feature = "test-hooks"))]
pub fn with_fixed_clock<T>(now: chrono::DateTime<chrono::Utc>, body: impl FnOnce() -> T) -> T {
    let previous = FIXED_NOW.replace(Some(now));
    let result = body();
    FIXED_NOW.set(previous);
    result
}

fn now() -> chrono::DateTime<chrono::Utc> {
    #[cfg(any(test, feature = "test-hooks"))]
    if let Some(now) = FIXED_NOW.get() {
        return now;
    }
    chrono::Utc::now()
}

fn ask_again(
    journal: &Path,
    day: &str,
    activities: &[AskedActivity],
) -> Result<OwnerAsk, AskError> {
    #[cfg(any(test, feature = "test-hooks"))]
    if let Some(now) = FIXED_NOW.get() {
        return record(journal, day, activities, false, now.timestamp_millis());
    }
    think_again(journal, day, activities, false)
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
    fn a_trash_ask_is_one_per_operation_and_day_and_reopens_for_another_item() {
        let root = tempfile::tempdir().unwrap();
        let day = "20261003";
        let first =
            record_for_trash(root.path(), day, "op", "item-a", &[asked("work", "a")], 10).unwrap();
        assert!(first.from_trash() && first.thaw);
        // The same item again (a stopped change the sweep finishes): nothing new.
        let again =
            record_for_trash(root.path(), day, "op", "item-a", &[asked("work", "a")], 20).unwrap();
        assert_eq!(again, first);
        // What the pass made of it so far.
        update(root.path(), &first.id, |current| {
            let mut next = current.clone();
            next.to_seed.clear();
            next.seeded.insert(asked("work", "a"));
            next.thaw_done = true;
            next.week.ended = Some(WEEK_BUILT.into());
            Some(Some(next))
        })
        .unwrap();
        // Another item of the operation changed the day again: asked anew.
        let reopened =
            record_for_trash(root.path(), day, "op", "item-b", &[asked("work", "b")], 30).unwrap();
        assert_eq!(reopened.id, first.id);
        assert_eq!(reopened.created_at_ms, 30);
        assert!(!reopened.thaw_done && reopened.week.ended.is_none());
        assert_eq!(
            reopened.to_seed,
            BTreeSet::from([asked("work", "a"), asked("work", "b")])
        );
        assert_eq!(read(root.path()).unwrap().len(), 1);
        // Another operation, or another day, is its own ask.
        record_for_trash(root.path(), day, "op2", "item-c", &[], 40).unwrap();
        record_for_trash(root.path(), "20261004", "op", "item-b", &[], 40).unwrap();
        assert_eq!(read(root.path()).unwrap().len(), 3);
        let pending = trash_pending(root.path()).unwrap();
        assert_eq!(
            pending.briefings,
            BTreeSet::from([day.to_owned(), "20261004".to_owned()])
        );
        // A correction's ask is not the trash's.
        record(root.path(), "20261005", &[asked("work", "a")], true, 1).unwrap();
        assert!(!trash_pending(root.path()).unwrap().briefings.contains("20261005"));
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

    fn activity(root: &Path, facet: &str, day: &str, id: &str, stream: &str, segment: &str) {
        if !root.join("facets").join(facet).exists() {
            solstone_core_facets::create_facet(root, facet, facet, "", "", "", None).unwrap();
        }
        let row =
            serde_json::json!({"id":id,"activity":"meeting","stream":stream,"segments":[segment]});
        let _ = solstone_core_facets::append_activity_record(
            root,
            facet,
            day,
            row.as_object().unwrap().clone(),
        )
        .unwrap();
    }

    fn relabelled(day: &str, stream: &str, segment_key: &str) -> RelabelledSegment {
        RelabelledSegment {
            day: day.into(),
            stream: stream.into(),
            segment_key: segment_key.into(),
        }
    }

    /// 03:00 UTC on 1 November is still 31 October in Denver.
    fn denver_journal() -> (tempfile::TempDir, chrono::DateTime<chrono::Utc>) {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("config")).unwrap();
        std::fs::write(
            root.path().join("config/journal.json"),
            serde_json::json!({"identity":{"timezone":"America/Denver"}}).to_string(),
        )
        .unwrap();
        let now = chrono::DateTime::parse_from_rfc3339("2026-11-01T03:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        (root, now)
    }

    #[test]
    fn a_relabel_asks_for_recent_days_by_the_owners_day_and_skips_days_without_activities() {
        let (root, now) = denver_journal();
        // Today is 20261031 in the owner's zone: 20261024 is seven closed days
        // back and 20261023 eight.
        activity(root.path(), "work", "20261024", "kept", "mic", "120000_300");
        activity(
            root.path(),
            "home",
            "20261024",
            "other_facet",
            "mic",
            "120000_300",
        );
        activity(
            root.path(),
            "work",
            "20261024",
            "other_stream",
            "phone",
            "120000_300",
        );
        activity(
            root.path(),
            "work",
            "20261023",
            "too_old",
            "mic",
            "120000_300",
        );
        with_fixed_clock(now, || {
            ask_after_relabel(
                root.path(),
                &[
                    relabelled("20261024", "mic", "120000_300"),
                    relabelled("20261023", "mic", "120000_300"),
                    // A recent day with no activity over the segment.
                    relabelled("20261030", "mic", "090000_300"),
                ],
            );
        });
        let asks = read(root.path()).unwrap().into_values().collect::<Vec<_>>();
        assert_eq!(asks.len(), 1);
        assert_eq!(asks[0].day, "20261024");
        assert!(!asks[0].thaw);
        assert_eq!(
            asks[0].to_seed,
            BTreeSet::from([asked("work", "kept"), asked("home", "other_facet")])
        );
        assert_eq!(asks[0].created_at_ms, now.timestamp_millis());
    }

    #[test]
    fn a_relabel_with_no_recent_activity_asks_nothing() {
        let (root, now) = denver_journal();
        with_fixed_clock(now, || {
            ask_after_relabel(root.path(), &[relabelled("20261031", "mic", "120000_300")]);
        });
        assert!(!path(root.path()).exists());
    }

    #[test]
    fn a_relabel_ask_that_cannot_be_saved_is_left_to_the_log() {
        let (root, now) = denver_journal();
        activity(root.path(), "work", "20261031", "a", "mic", "120000_300");
        std::fs::create_dir_all(path(root.path()).parent().unwrap()).unwrap();
        std::fs::write(path(root.path()), "{").unwrap();
        with_fixed_clock(now, || {
            ask_after_relabel(root.path(), &[relabelled("20261031", "mic", "120000_300")]);
        });
        assert_eq!(std::fs::read_to_string(path(root.path())).unwrap(), "{");
    }

    #[cfg(feature = "full-tests")]
    #[test]
    fn a_wakeup_that_cannot_be_sent_is_not_an_error() {
        let root = tempfile::tempdir().unwrap();
        let ask = think_again(root.path(), "20260101", &[asked("work", "a")], true).unwrap();
        assert_eq!(read(root.path()).unwrap()[&ask.id], ask);
    }
}
