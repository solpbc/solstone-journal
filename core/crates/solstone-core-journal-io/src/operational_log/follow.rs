// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Transactional, descriptor-bound follower for canonical operational logs.

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};

use chrono::{Duration, NaiveDate};

#[cfg(test)]
use std::cell::RefCell;

use super::catalog::{
    OplogCatalogEntry, OplogCatalogError, OplogCatalogSnapshot, probe_retained_oplog_lease,
};
use super::{LeaseProbe, OplogFileIdentity};

/// Maximum payload bytes read across one successful follower tick.
pub const OPLOG_FOLLOW_TICK_BYTE_BUDGET: usize = 256 * 1024;
/// Largest one-record read, including its newline delimiter.
pub const OPLOG_FOLLOW_MAX_RECORD_BYTES: usize = 16 * 1024;
const _: () = assert!(OPLOG_FOLLOW_MAX_RECORD_BYTES < OPLOG_FOLLOW_TICK_BYTE_BUDGET);

const READERS_SERVICED_PER_TICK: usize =
    OPLOG_FOLLOW_TICK_BYTE_BUDGET / OPLOG_FOLLOW_MAX_RECORD_BYTES;

#[cfg(test)]
thread_local! {
    static AFTER_FRONTIER_SAMPLE: RefCell<Option<Box<dyn FnOnce()>>> = const { RefCell::new(None) };
}

/// Supplies all-or-error catalog snapshots for a local-day set.
pub trait OplogSnapshotSource {
    fn snapshot(&self, days: &[NaiveDate]) -> Result<OplogCatalogSnapshot, OplogCatalogError>;
}

/// Local day authority used for the follower's two-day window and tombstones.
pub trait OplogClock {
    fn today(&self) -> NaiveDate;
}

struct TrackedOplog {
    entry: OplogCatalogEntry,
    file: File,
    /// Absolute file position of the last committed payload byte.
    committed_offset: u64,
    /// Largest frontier successfully observed before a commit.
    last_observed_frontier: u64,
}

/// Persistent identity-indexed follower state.
#[derive(Default)]
pub struct OplogFollowState {
    tracked: HashMap<OplogFileIdentity, TrackedOplog>,
    tombstones: HashMap<OplogFileIdentity, OplogCatalogEntry>,
    round_robin_start: usize,
}

/// Result of initial discovery.
pub struct OplogInitialDiscovery {
    pub state: OplogFollowState,
    pub has_tracked_sources: bool,
}

/// Post-commit result of one follower cycle.
#[derive(Debug)]
pub enum OplogFollowTick {
    Continued {
        rows: Vec<(OplogCatalogEntry, String)>,
    },
    Stopped,
}

impl OplogFollowTick {
    /// Rows durably committed by this tick, if it was not stopped.
    pub fn into_rows(self) -> Option<Vec<(OplogCatalogEntry, String)>> {
        match self {
            Self::Continued { rows } => Some(rows),
            Self::Stopped => None,
        }
    }
}

/// Identity-based canonical operational-log follower.
#[derive(Default)]
pub struct OplogFollower {
    state: OplogFollowState,
}

impl OplogFollower {
    /// Build follower state from current/prior-day admission-bound descriptors.
    pub fn discover_initial(
        source: &dyn OplogSnapshotSource,
        clock: &dyn OplogClock,
    ) -> Result<OplogInitialDiscovery, OplogCatalogError> {
        let snapshot = source.snapshot(&days(clock.today()))?;
        let mut state = OplogFollowState::default();
        for (entry, file) in snapshot.into_catalogued_entries() {
            let tracked = tracked_from_admission(entry, file)?;
            state.tracked.insert(tracked.entry.identity(), tracked);
        }
        Ok(OplogInitialDiscovery {
            has_tracked_sources: !state.tracked.is_empty(),
            state,
        })
    }

    /// Adopt state returned by [`Self::discover_initial`].
    pub fn from_state(state: OplogFollowState) -> Self {
        Self { state }
    }

    /// Borrow persistent state for test observation or handoff.
    pub fn state(&self) -> &OplogFollowState {
        &self.state
    }

    /// Read a finite round-robin batch and commit it only after all descriptor
    /// liveness probes and discovery complete successfully.
    pub fn tick(
        &mut self,
        source: &dyn OplogSnapshotSource,
        clock: &dyn OplogClock,
        stop: &dyn Fn() -> bool,
    ) -> Result<OplogFollowTick, OplogCatalogError> {
        if stop() {
            return Ok(OplogFollowTick::Stopped);
        }

        let identities = self.ordered_identities();
        let mut frontiers = HashMap::with_capacity(identities.len());
        for identity in &identities {
            let tracked = self.tracked(identity);
            let frontier = file_len(&tracked.file, &tracked.entry)?;
            if frontier < tracked.last_observed_frontier || frontier < tracked.committed_offset {
                return Err(OplogCatalogError::identity_for_day(tracked.entry.day()));
            }
            frontiers.insert(*identity, frontier);
        }

        #[cfg(test)]
        if let Some(action) = AFTER_FRONTIER_SAMPLE.with(|hook| hook.borrow_mut().take()) {
            action();
        }

        let selected = selected_identities(
            &identities,
            self.state.round_robin_start,
            READERS_SERVICED_PER_TICK,
        );
        let mut proposed_offsets = HashMap::with_capacity(selected.len());
        let mut incomplete = HashMap::with_capacity(selected.len());
        let mut staged_rows = HashMap::with_capacity(selected.len());
        for identity in &selected {
            let tracked = self.tracked_mut(identity);
            let frontier = frontiers[identity];
            match read_record(
                &mut tracked.file,
                tracked.committed_offset,
                frontier,
                &tracked.entry,
            )? {
                ReadRecord::Complete { line, offset } => {
                    proposed_offsets.insert(*identity, offset);
                    staged_rows.insert(*identity, (tracked.entry.clone(), line));
                }
                ReadRecord::Incomplete(bytes) => {
                    incomplete.insert(*identity, bytes);
                    proposed_offsets.insert(*identity, tracked.committed_offset);
                }
            }
        }

        let mut probes = HashMap::with_capacity(identities.len());
        for identity in &identities {
            let tracked = self.tracked(identity);
            let probe = probe_retained_oplog_lease(&tracked.file, *identity);
            if probe == LeaseProbe::Indeterminate {
                return Err(OplogCatalogError::kind_for_day(
                    "oplog_catalog_lease_indeterminate",
                    tracked.entry.day(),
                ));
            }
            probes.insert(*identity, probe);
        }

        let mut final_sizes = HashMap::with_capacity(identities.len());
        for identity in &identities {
            let tracked = self.tracked(identity);
            final_sizes.insert(*identity, file_len(&tracked.file, &tracked.entry)?);
        }

        // A final unterminated record is valid only after the descriptor
        // conclusively reaches a stable released EOF.
        for identity in &selected {
            let Some(bytes) = incomplete.remove(identity) else {
                continue;
            };
            if bytes.is_empty()
                || probes[identity] != LeaseProbe::Released
                || final_sizes[identity] != frontiers[identity]
            {
                continue;
            }
            let entry = &self.tracked(identity).entry;
            let line = String::from_utf8(bytes)
                .map_err(|_| OplogCatalogError::kind_for_day("oplog_catalog_utf8", entry.day()))?;
            proposed_offsets.insert(*identity, frontiers[identity]);
            staged_rows.insert(*identity, (entry.clone(), line));
        }

        // The snapshot is discovery-only for current readers: descriptor state
        // remains the sole liveness and progress authority after admission.
        let today = clock.today();
        let snapshot = source.snapshot(&days(today))?;
        let mut adopted = Vec::new();
        for (entry, file) in snapshot.into_catalogued_entries() {
            let identity = entry.identity();
            if self.state.tracked.contains_key(&identity)
                || self.state.tombstones.contains_key(&identity)
            {
                continue;
            }
            adopted.push((identity, tracked_from_admission(entry, file)?));
        }

        let retired = identities
            .iter()
            .copied()
            .filter(|identity| {
                probes[identity] == LeaseProbe::Released
                    && final_sizes[identity] == frontiers[identity]
                    && proposed_offsets
                        .get(identity)
                        .copied()
                        .unwrap_or_else(|| self.tracked(identity).committed_offset)
                        >= frontiers[identity]
            })
            .collect::<Vec<_>>();
        let expired_tombstones = self
            .state
            .tombstones
            .iter()
            .filter_map(|(identity, entry)| {
                NaiveDate::parse_from_str(entry.day(), "%Y%m%d")
                    .ok()
                    .filter(|day| *day != today && *day != today - Duration::days(1))
                    .map(|_| *identity)
            })
            .collect::<Vec<_>>();

        if stop() {
            return Ok(OplogFollowTick::Stopped);
        }

        // Commit begins here: every error-capable action is above this point.
        for identity in &identities {
            let tracked = self.tracked_mut(identity);
            tracked.last_observed_frontier = frontiers[identity];
            if let Some(offset) = proposed_offsets.get(identity) {
                tracked.committed_offset = *offset;
            }
        }
        for identity in retired {
            let tracked = self
                .state
                .tracked
                .remove(&identity)
                .expect("selected tracked identity exists");
            self.state.tombstones.insert(identity, tracked.entry);
        }
        for identity in expired_tombstones {
            self.state.tombstones.remove(&identity);
        }
        for (identity, tracked) in adopted {
            self.state.tracked.insert(identity, tracked);
        }
        if !identities.is_empty() {
            self.state.round_robin_start =
                (self.state.round_robin_start + selected.len()) % identities.len();
        }

        Ok(OplogFollowTick::Continued {
            rows: selected
                .into_iter()
                .filter_map(|identity| staged_rows.remove(&identity))
                .collect(),
        })
    }

    fn ordered_identities(&self) -> Vec<OplogFileIdentity> {
        let mut identities = self.state.tracked.keys().copied().collect::<Vec<_>>();
        identities.sort_by(|left, right| {
            let left = &self.tracked(left).entry;
            let right = &self.tracked(right).entry;
            left.day().cmp(right.day()).then_with(|| {
                left.leaf()
                    .as_encoded_bytes()
                    .cmp(right.leaf().as_encoded_bytes())
            })
        });
        identities
    }

    fn tracked(&self, identity: &OplogFileIdentity) -> &TrackedOplog {
        self.state
            .tracked
            .get(identity)
            .expect("tracked identity exists")
    }

    fn tracked_mut(&mut self, identity: &OplogFileIdentity) -> &mut TrackedOplog {
        self.state
            .tracked
            .get_mut(identity)
            .expect("tracked identity exists")
    }
}

fn tracked_from_admission(
    entry: OplogCatalogEntry,
    file: File,
) -> Result<TrackedOplog, OplogCatalogError> {
    let frontier = file_len(&file, &entry)?;
    let committed_offset = entry.payload_offset() as u64;
    if committed_offset > frontier {
        return Err(OplogCatalogError::identity_for_day(entry.day()));
    }
    Ok(TrackedOplog {
        entry,
        file,
        committed_offset,
        last_observed_frontier: frontier,
    })
}

fn file_len(file: &File, entry: &OplogCatalogEntry) -> Result<u64, OplogCatalogError> {
    file.metadata()
        .map(|metadata| metadata.len())
        .map_err(|_| OplogCatalogError::io_for_day(entry.day()))
}

enum ReadRecord {
    Complete { line: String, offset: u64 },
    Incomplete(Vec<u8>),
}

fn read_record(
    file: &mut File,
    committed_offset: u64,
    frontier: u64,
    entry: &OplogCatalogEntry,
) -> Result<ReadRecord, OplogCatalogError> {
    let available = frontier.saturating_sub(committed_offset);
    if available == 0 {
        return Ok(ReadRecord::Incomplete(Vec::new()));
    }
    let wanted = available.min(OPLOG_FOLLOW_MAX_RECORD_BYTES as u64) as usize;
    let mut bytes = vec![0; wanted];
    file.seek(SeekFrom::Start(committed_offset))
        .map_err(|_| OplogCatalogError::io_for_day(entry.day()))?;
    file.read_exact(&mut bytes)
        .map_err(|_| OplogCatalogError::io_for_day(entry.day()))?;
    if let Some(newline) = bytes.iter().position(|byte| *byte == b'\n') {
        let record = if newline > 0 && bytes[newline - 1] == b'\r' {
            &bytes[..newline - 1]
        } else {
            &bytes[..newline]
        };
        let line = String::from_utf8(record.to_vec())
            .map_err(|_| OplogCatalogError::kind_for_day("oplog_catalog_utf8", entry.day()))?;
        return Ok(ReadRecord::Complete {
            line,
            offset: committed_offset + newline as u64 + 1,
        });
    }
    if available > OPLOG_FOLLOW_MAX_RECORD_BYTES as u64 {
        return Err(OplogCatalogError::kind_for_day(
            "oplog_catalog_record_too_large",
            entry.day(),
        ));
    }
    Ok(ReadRecord::Incomplete(bytes))
}

fn selected_identities(
    identities: &[OplogFileIdentity],
    start: usize,
    maximum: usize,
) -> Vec<OplogFileIdentity> {
    if identities.is_empty() {
        return Vec::new();
    }
    let count = identities.len().min(maximum);
    (0..count)
        .map(|offset| identities[(start + offset) % identities.len()])
        .collect()
}

fn days(today: NaiveDate) -> [NaiveDate; 2] {
    [today - Duration::days(1), today]
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::collections::HashSet;
    use std::io::Write;
    use std::path::PathBuf;

    use chrono::{Datelike, FixedOffset, TimeZone};
    use tempfile::TempDir;

    use super::*;
    use crate::JournalRoot;
    use crate::operational_log::{OplogFormat, catalog_oplogs, create_oplog_at};

    struct Source(PathBuf);

    impl OplogSnapshotSource for Source {
        fn snapshot(&self, days: &[NaiveDate]) -> Result<OplogCatalogSnapshot, OplogCatalogError> {
            catalog_oplogs(
                JournalRoot::open(&self.0).map_err(|_| OplogCatalogError::root())?,
                days,
            )
        }
    }

    struct FailOnceSource {
        root: PathBuf,
        fail: Cell<bool>,
    }

    impl OplogSnapshotSource for FailOnceSource {
        fn snapshot(&self, days: &[NaiveDate]) -> Result<OplogCatalogSnapshot, OplogCatalogError> {
            if self.fail.replace(false) {
                return Err(OplogCatalogError::root());
            }
            catalog_oplogs(
                JournalRoot::open(&self.root).map_err(|_| OplogCatalogError::root())?,
                days,
            )
        }
    }

    struct Clock(Cell<NaiveDate>);

    impl OplogClock for Clock {
        fn today(&self) -> NaiveDate {
            self.0.get()
        }
    }

    fn date() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 8, 7).unwrap()
    }

    fn instant(day: NaiveDate) -> chrono::DateTime<FixedOffset> {
        FixedOffset::east_opt(0)
            .unwrap()
            .with_ymd_and_hms(day.year(), day.month(), day.day(), 12, 0, 0)
            .single()
            .unwrap()
    }

    fn writer(temporary: &TempDir, source: &str) -> crate::operational_log::OplogWriter {
        create_oplog_at(
            JournalRoot::open(temporary.path()).unwrap(),
            source,
            "run",
            OplogFormat::Log,
            instant(date()),
        )
        .unwrap()
    }

    fn follower(temporary: &TempDir) -> (OplogFollower, Source, Clock) {
        let source = Source(temporary.path().to_path_buf());
        let clock = Clock(Cell::new(date()));
        let initial = OplogFollower::discover_initial(&source, &clock).unwrap();
        (OplogFollower::from_state(initial.state), source, clock)
    }

    fn with_after_frontier_sample<T>(
        action: impl FnOnce() + 'static,
        operation: impl FnOnce() -> T,
    ) -> T {
        AFTER_FRONTIER_SAMPLE.with(|hook| hook.replace(Some(Box::new(action))));
        let result = operation();
        AFTER_FRONTIER_SAMPLE.with(|hook| hook.replace(None));
        result
    }

    #[test]
    fn newly_discovered_entries_start_at_payload_zero_and_are_byte_bounded() {
        let temporary = tempfile::tempdir().unwrap();
        let mut first = writer(&temporary, "first");
        writeln!(first, "first").unwrap();
        let (mut follower, source, clock) = follower(&temporary);
        let mut second = writer(&temporary, "second");
        writeln!(second, "second").unwrap();
        let first_tick = follower.tick(&source, &clock, &|| false).unwrap();
        assert_eq!(first_tick.into_rows().unwrap().len(), 1);
        let second_tick = follower.tick(&source, &clock, &|| false).unwrap();
        assert_eq!(second_tick.into_rows().unwrap().len(), 1);
    }

    #[test]
    fn released_catalogued_entry_retires_at_true_descriptor_eof() {
        let temporary = tempfile::tempdir().unwrap();
        let mut log = writer(&temporary, "retired");
        writeln!(log, "final").unwrap();
        let (mut follower, source, clock) = follower(&temporary);
        drop(log);
        let rows = follower
            .tick(&source, &clock, &|| false)
            .unwrap()
            .into_rows()
            .unwrap();
        assert_eq!(rows[0].1, "final");
        assert!(follower.state.tracked.is_empty());
        assert_eq!(follower.state.tombstones.len(), 1);
    }

    #[test]
    fn round_robin_selects_every_continuously_ready_reader_within_two_ticks() {
        let temporary = tempfile::tempdir().unwrap();
        let mut writers = Vec::new();
        for index in 0..20 {
            let source = format!("source-{index:02}");
            let mut log = writer(&temporary, &source);
            writeln!(log, "{source}").unwrap();
            writers.push(log);
        }
        let (mut follower, source, clock) = follower(&temporary);
        let mut seen = HashSet::new();
        for _ in 0..2 {
            let rows = follower
                .tick(&source, &clock, &|| false)
                .unwrap()
                .into_rows()
                .unwrap();
            seen.extend(
                rows.into_iter()
                    .map(|(entry, _)| entry.name().source().display_slug().to_owned()),
            );
        }
        assert_eq!(seen.len(), 20);
        drop(writers);
    }

    #[test]
    fn released_unselected_reader_is_probed_and_retires_without_a_row() {
        let temporary = tempfile::tempdir().unwrap();
        let mut active = Vec::new();
        for index in 0..READERS_SERVICED_PER_TICK {
            let source = format!("active-{index:02}");
            let mut log = writer(&temporary, &source);
            writeln!(log, "{source}").unwrap();
            active.push(log);
        }
        let released = writer(&temporary, "zulu");
        let (mut follower, source, clock) = follower(&temporary);
        let released_identity = follower
            .state
            .tracked
            .iter()
            .find_map(|(identity, tracked)| {
                (tracked.entry.name().source().display_slug() == "zulu").then_some(*identity)
            })
            .unwrap();
        assert!(
            !selected_identities(
                &follower.ordered_identities(),
                follower.state.round_robin_start,
                READERS_SERVICED_PER_TICK,
            )
            .contains(&released_identity)
        );
        drop(released);
        let rows = follower
            .tick(&source, &clock, &|| false)
            .unwrap()
            .into_rows()
            .unwrap();
        assert!(
            rows.iter()
                .all(|(entry, _)| entry.name().source().display_slug() != "zulu")
        );
        assert!(follower.state.tombstones.contains_key(&released_identity));
        drop(active);
    }

    #[test]
    fn append_after_frontier_then_release_defers_retirement_until_later_drain() {
        let temporary = tempfile::tempdir().unwrap();
        let mut log = writer(&temporary, "append");
        writeln!(log, "first").unwrap();
        let (mut follower, source, clock) = follower(&temporary);
        let first = with_after_frontier_sample(
            move || {
                writeln!(log, "second").unwrap();
                drop(log);
            },
            || {
                follower
                    .tick(&source, &clock, &|| false)
                    .unwrap()
                    .into_rows()
                    .unwrap()
            },
        );
        assert_eq!(
            first.into_iter().map(|(_, line)| line).collect::<Vec<_>>(),
            ["first"]
        );
        assert_eq!(follower.state.tracked.len(), 1);
        assert!(follower.state.tombstones.is_empty());
        let second = follower
            .tick(&source, &clock, &|| false)
            .unwrap()
            .into_rows()
            .unwrap();
        assert_eq!(
            second.into_iter().map(|(_, line)| line).collect::<Vec<_>>(),
            ["second"]
        );
        assert!(follower.state.tracked.is_empty());
        assert_eq!(follower.state.tombstones.len(), 1);
    }

    #[test]
    fn tombstones_survive_one_day_then_expire_on_the_second() {
        let temporary = tempfile::tempdir().unwrap();
        let mut log = writer(&temporary, "expired");
        writeln!(log, "final").unwrap();
        let (mut follower, source, clock) = follower(&temporary);
        drop(log);
        follower.tick(&source, &clock, &|| false).unwrap();
        assert_eq!(follower.state.tombstones.len(), 1);
        clock.0.set(date() + Duration::days(1));
        follower.tick(&source, &clock, &|| false).unwrap();
        assert_eq!(follower.state.tombstones.len(), 1);
        clock.0.set(date() + Duration::days(2));
        follower.tick(&source, &clock, &|| false).unwrap();
        assert!(follower.state.tombstones.is_empty());
    }

    #[test]
    fn retained_descriptor_growth_and_shrink_are_the_progress_authority() {
        let temporary = tempfile::tempdir().unwrap();
        let mut log = writer(&temporary, "sized");
        let path = temporary
            .path()
            .join("chronicle/20260807/health")
            .join(log.leaf_name());
        let (mut follower, source, clock) = follower(&temporary);
        writeln!(log, "late").unwrap();
        assert_eq!(
            follower
                .tick(&source, &clock, &|| false)
                .unwrap()
                .into_rows()
                .unwrap()[0]
                .1,
            "late"
        );
        drop(log);
        std::fs::OpenOptions::new()
            .write(true)
            .open(path)
            .unwrap()
            .set_len(0)
            .unwrap();
        assert_eq!(
            follower
                .tick(&source, &clock, &|| false)
                .unwrap_err()
                .kind(),
            "oplog_catalog_identity_changed"
        );
    }

    #[test]
    fn record_size_boundary_is_exact_and_oversize_fails_without_commit() {
        let temporary = tempfile::tempdir().unwrap();
        let mut exact = writer(&temporary, "exact");
        writeln!(exact, "{}", "x".repeat(OPLOG_FOLLOW_MAX_RECORD_BYTES - 1)).unwrap();
        let (mut follower, source, clock) = follower(&temporary);
        let rows = follower
            .tick(&source, &clock, &|| false)
            .unwrap()
            .into_rows()
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].1.len(), OPLOG_FOLLOW_MAX_RECORD_BYTES - 1);

        let mut oversized = writer(&temporary, "oversized");
        writeln!(oversized, "{}", "x".repeat(OPLOG_FOLLOW_MAX_RECORD_BYTES)).unwrap();
        follower.tick(&source, &clock, &|| false).unwrap();
        let identity = *follower
            .state
            .tracked
            .iter()
            .find_map(|(identity, tracked)| {
                (tracked.entry.name().source().display_slug() == "oversized").then_some(identity)
            })
            .unwrap();
        let offset = follower.state.tracked[&identity].committed_offset;
        assert_eq!(
            follower
                .tick(&source, &clock, &|| false)
                .unwrap_err()
                .kind(),
            "oplog_catalog_record_too_large"
        );
        assert_eq!(follower.state.tracked[&identity].committed_offset, offset);
    }

    #[test]
    fn snapshot_fault_does_not_commit_or_emit_the_pending_line_after_recovery() {
        let temporary = tempfile::tempdir().unwrap();
        let mut log = writer(&temporary, "fault");
        writeln!(log, "sentinel").unwrap();
        let source = FailOnceSource {
            root: temporary.path().to_path_buf(),
            fail: Cell::new(false),
        };
        let clock = Clock(Cell::new(date()));
        let initial = OplogFollower::discover_initial(&source, &clock).unwrap();
        let mut follower = OplogFollower::from_state(initial.state);
        let identity = *follower.state.tracked.keys().next().unwrap();
        let offset = follower.state.tracked[&identity].committed_offset;
        source.fail.set(true);
        assert!(follower.tick(&source, &clock, &|| false).is_err());
        assert_eq!(follower.state.tracked[&identity].committed_offset, offset);
        let rows = follower
            .tick(&source, &clock, &|| false)
            .unwrap()
            .into_rows()
            .unwrap();
        assert_eq!(
            rows.into_iter().map(|(_, line)| line).collect::<Vec<_>>(),
            ["sentinel"]
        );
    }

    #[test]
    fn invalid_utf8_at_released_true_eof_fails_closed() {
        let temporary = tempfile::tempdir().unwrap();
        let mut log = writer(&temporary, "utf8");
        log.write_all(&[0xff]).unwrap();
        let (mut follower, source, clock) = follower(&temporary);
        drop(log);
        assert_eq!(
            follower
                .tick(&source, &clock, &|| false)
                .unwrap_err()
                .kind(),
            "oplog_catalog_utf8"
        );
    }

    #[test]
    fn incomplete_multibyte_frontier_defers_until_a_complete_record_arrives() {
        let temporary = tempfile::tempdir().unwrap();
        let mut log = writer(&temporary, "multibyte");
        log.write_all(&[0xe2, 0x82]).unwrap();
        let (mut follower, source, clock) = follower(&temporary);
        assert!(
            follower
                .tick(&source, &clock, &|| false)
                .unwrap()
                .into_rows()
                .unwrap()
                .is_empty()
        );
        log.write_all(&[0xac, b'\n']).unwrap();
        assert_eq!(
            follower
                .tick(&source, &clock, &|| false)
                .unwrap()
                .into_rows()
                .unwrap()[0]
                .1,
            "€"
        );
    }

    #[cfg(unix)]
    #[test]
    fn unlinked_tracked_descriptor_keeps_draining_without_a_name_reopen() {
        use std::fs;

        let temporary = tempfile::tempdir().unwrap();
        let mut log = writer(&temporary, "unlinked");
        writeln!(log, "drain").unwrap();
        let path = temporary
            .path()
            .join("chronicle/20260807/health")
            .join(log.leaf_name());
        let (mut follower, source, clock) = follower(&temporary);
        fs::remove_file(path).unwrap();
        assert_eq!(
            follower
                .tick(&source, &clock, &|| false)
                .unwrap()
                .into_rows()
                .unwrap()[0]
                .1,
            "drain"
        );
        drop(log);
        follower.tick(&source, &clock, &|| false).unwrap();
        assert!(follower.state.tracked.is_empty());
    }

    #[test]
    fn stop_during_tick_has_no_rows_or_mutation() {
        let temporary = tempfile::tempdir().unwrap();
        let mut log = writer(&temporary, "stopped");
        writeln!(log, "sentinel").unwrap();
        let (mut follower, source, clock) = follower(&temporary);
        let identity = *follower.state.tracked.keys().next().unwrap();
        let offset = follower.state.tracked[&identity].committed_offset;
        let stopped = std::rc::Rc::new(Cell::new(false));
        let stop_from_hook = stopped.clone();
        let stop_for_tick = stopped.clone();
        let tick = with_after_frontier_sample(
            move || stop_from_hook.set(true),
            || follower.tick(&source, &clock, &|| stop_for_tick.get()),
        )
        .unwrap();
        assert!(matches!(tick, OplogFollowTick::Stopped));
        assert_eq!(follower.state.tracked[&identity].committed_offset, offset);
        assert!(follower.state.tombstones.is_empty());

        stopped.set(false);
        let rows = follower
            .tick(&source, &clock, &|| false)
            .unwrap()
            .into_rows()
            .unwrap();
        assert_eq!(
            rows.into_iter().map(|(_, line)| line).collect::<Vec<_>>(),
            ["sentinel"]
        );
    }
}
