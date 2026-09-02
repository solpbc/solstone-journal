// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Injectable identity-based follower for canonical operational logs.

use std::collections::HashMap;
use std::io;

use chrono::{Duration, NaiveDate};

use super::catalog::{OplogCatalogEntry, OplogCatalogError, OplogCatalogSnapshot};
use super::{LeaseProbe, OplogFileIdentity};

/// The old path follower has no cycle bound; keep this lower-level drain finite.
pub const OPLOG_FOLLOW_MAX_LINES_PER_TICK: usize = 1024;

/// A normalized line reader for one descriptor-bound catalog entry.
pub trait OplogFollowReader {
    /// Read one line without its line terminator, or `None` at EOF.
    fn read_line(&mut self) -> io::Result<Option<String>>;
    /// Position a newly discovered initial source at EOF.
    fn seek_to_end(&mut self) -> io::Result<()>;
}

/// Opens a reader for an already validated catalog entry.
pub trait OplogEntryReaderFactory {
    fn open(&self, entry: &OplogCatalogEntry) -> io::Result<Box<dyn OplogFollowReader>>;
}

/// Supplies all-or-error catalog snapshots for a local-day set.
pub trait OplogSnapshotSource {
    fn snapshot(&self, days: &[NaiveDate]) -> Result<OplogCatalogSnapshot, OplogCatalogError>;
}

/// Identity liveness authority used only for absent tracked entries.
pub trait OplogIdentityProbe {
    fn probe(&self, entry: &OplogCatalogEntry) -> LeaseProbe;
}

/// Local day authority used for the follower's two-day window and tombstones.
pub trait OplogClock {
    fn today(&self) -> NaiveDate;
}

struct TrackedOplog {
    entry: OplogCatalogEntry,
    reader: Box<dyn OplogFollowReader>,
}

/// Persistent identity-indexed follower state.
#[derive(Default)]
pub struct OplogFollowState {
    tracked: HashMap<OplogFileIdentity, TrackedOplog>,
    tombstones: HashMap<OplogFileIdentity, OplogCatalogEntry>,
}

/// Result of initial discovery.
pub struct OplogInitialDiscovery {
    pub state: OplogFollowState,
    pub has_tracked_sources: bool,
}

/// Outcome of one ordered follower cycle.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OplogFollowTickOutcome {
    Continued,
    Stopped,
}

/// Identity-based canonical operational-log follower.
#[derive(Default)]
pub struct OplogFollower {
    state: OplogFollowState,
}

impl OplogFollower {
    /// Build follower state from current/prior-day entries, seeking initial sources to EOF.
    pub fn discover_initial(
        source: &dyn OplogSnapshotSource,
        factory: &dyn OplogEntryReaderFactory,
        clock: &dyn OplogClock,
    ) -> Result<OplogInitialDiscovery, OplogCatalogError> {
        let snapshot = source.snapshot(&days(clock.today()))?;
        let mut state = OplogFollowState::default();
        for entry in snapshot.entries() {
            let mut reader = factory
                .open(entry)
                .map_err(|_| OplogCatalogError::io_for_day(entry.day()))?;
            reader
                .seek_to_end()
                .map_err(|_| OplogCatalogError::io_for_day(entry.day()))?;
            state.tracked.insert(
                entry.identity(),
                TrackedOplog {
                    entry: entry.clone(),
                    reader,
                },
            );
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

    /// Drain a finite prefix, then reconcile one complete current/prior snapshot.
    pub fn tick(
        &mut self,
        source: &dyn OplogSnapshotSource,
        factory: &dyn OplogEntryReaderFactory,
        probe: &dyn OplogIdentityProbe,
        clock: &dyn OplogClock,
        stop: &dyn Fn() -> bool,
        emit: &mut dyn FnMut(&OplogCatalogEntry, String),
    ) -> Result<OplogFollowTickOutcome, OplogCatalogError> {
        if stop() {
            return Ok(OplogFollowTickOutcome::Stopped);
        }
        let mut remaining = OPLOG_FOLLOW_MAX_LINES_PER_TICK;
        let mut identities = self.state.tracked.keys().copied().collect::<Vec<_>>();
        identities.sort_by(|left, right| {
            let left = &self
                .state
                .tracked
                .get(left)
                .expect("tracked identity exists")
                .entry;
            let right = &self
                .state
                .tracked
                .get(right)
                .expect("tracked identity exists")
                .entry;
            left.day().cmp(right.day()).then_with(|| {
                left.leaf()
                    .as_encoded_bytes()
                    .cmp(right.leaf().as_encoded_bytes())
            })
        });
        for identity in identities {
            let tracked = self
                .state
                .tracked
                .get_mut(&identity)
                .expect("tracked identity exists");
            while remaining > 0 {
                if stop() {
                    return Ok(OplogFollowTickOutcome::Stopped);
                }
                let Some(line) = tracked
                    .reader
                    .read_line()
                    .map_err(|_| OplogCatalogError::io_for_day(tracked.entry.day()))?
                else {
                    break;
                };
                remaining -= 1;
                emit(&tracked.entry, line);
            }
            if remaining == 0 {
                return Ok(OplogFollowTickOutcome::Continued);
            }
        }
        if stop() {
            return Ok(OplogFollowTickOutcome::Stopped);
        }
        let today = clock.today();
        let snapshot = source.snapshot(&days(today))?;
        self.reconcile(snapshot, factory, probe, today)?;
        Ok(OplogFollowTickOutcome::Continued)
    }

    fn reconcile(
        &mut self,
        snapshot: OplogCatalogSnapshot,
        factory: &dyn OplogEntryReaderFactory,
        probe: &dyn OplogIdentityProbe,
        today: NaiveDate,
    ) -> Result<(), OplogCatalogError> {
        self.state.tombstones.retain(|_, entry| {
            let Ok(day) = NaiveDate::parse_from_str(entry.day(), "%Y%m%d") else {
                return false;
            };
            day == today || day == today - Duration::days(1)
        });
        let mut observed = HashMap::new();
        for entry in snapshot.entries() {
            if self.state.tracked.iter().any(|(identity, existing)| {
                *identity != entry.identity()
                    && existing.entry.day() == entry.day()
                    && existing.entry.leaf() == entry.leaf()
            }) {
                return Err(OplogCatalogError::identity_for_day(entry.day()));
            }
            if let Some(existing) = self.state.tracked.get_mut(&entry.identity()) {
                if existing.entry.leaf() != entry.leaf() || entry.size() < existing.entry.size() {
                    return Err(OplogCatalogError::identity_for_day(entry.day()));
                }
                existing.entry = entry.clone();
            }
            if let Some(tombstone) = self.state.tombstones.get(&entry.identity())
                && (tombstone.leaf() != entry.leaf() || tombstone.size() != entry.size())
            {
                return Err(OplogCatalogError::identity_for_day(entry.day()));
            }
            observed.insert(entry.identity(), entry.clone());
        }
        let missing = self
            .state
            .tracked
            .keys()
            .copied()
            .filter(|identity| !observed.contains_key(identity))
            .collect::<Vec<_>>();
        for identity in missing {
            let Some(tracked) = self.state.tracked.get(&identity) else {
                continue;
            };
            if probe.probe(&tracked.entry) == LeaseProbe::Released {
                let tracked = self
                    .state
                    .tracked
                    .remove(&identity)
                    .expect("tracked identity exists");
                self.state.tombstones.insert(identity, tracked.entry);
            }
        }
        for entry in observed.into_values() {
            if self.state.tracked.contains_key(&entry.identity()) {
                continue;
            }
            if self.state.tombstones.contains_key(&entry.identity()) {
                // A released identity may still be enumerated while an old writer
                // drains. Preserve the tombstone and do not replay its bytes.
                continue;
            }
            let reader = factory
                .open(&entry)
                .map_err(|_| OplogCatalogError::io_for_day(entry.day()))?;
            self.state
                .tracked
                .insert(entry.identity(), TrackedOplog { entry, reader });
        }
        Ok(())
    }
}

fn days(today: NaiveDate) -> [NaiveDate; 2] {
    [today - Duration::days(1), today]
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::collections::VecDeque;
    use std::io;
    use std::rc::Rc;

    use chrono::{Datelike, FixedOffset, TimeZone};
    use tempfile::TempDir;

    use super::*;
    use crate::JournalRoot;
    use crate::operational_log::{OplogFormat, catalog_oplogs, create_oplog_at};

    #[derive(Clone)]
    enum Step {
        Line(&'static str),
        Fault,
    }

    struct Source {
        snapshot: RefCell<OplogCatalogSnapshot>,
        requested_days: RefCell<Vec<Vec<NaiveDate>>>,
    }

    impl Source {
        fn new(snapshot: OplogCatalogSnapshot) -> Self {
            Self {
                snapshot: RefCell::new(snapshot),
                requested_days: RefCell::new(Vec::new()),
            }
        }
    }

    impl OplogSnapshotSource for Source {
        fn snapshot(&self, days: &[NaiveDate]) -> Result<OplogCatalogSnapshot, OplogCatalogError> {
            self.requested_days.borrow_mut().push(days.to_vec());
            Ok(self.snapshot.borrow().clone())
        }
    }

    struct ReaderFactory {
        queues: Rc<RefCell<HashMap<OplogFileIdentity, VecDeque<Step>>>>,
    }

    impl OplogEntryReaderFactory for ReaderFactory {
        fn open(&self, entry: &OplogCatalogEntry) -> io::Result<Box<dyn OplogFollowReader>> {
            Ok(Box::new(Reader {
                identity: entry.identity(),
                queues: self.queues.clone(),
            }))
        }
    }

    struct Reader {
        identity: OplogFileIdentity,
        queues: Rc<RefCell<HashMap<OplogFileIdentity, VecDeque<Step>>>>,
    }

    impl OplogFollowReader for Reader {
        fn read_line(&mut self) -> io::Result<Option<String>> {
            match self
                .queues
                .borrow_mut()
                .entry(self.identity)
                .or_default()
                .pop_front()
            {
                Some(Step::Line(line)) => Ok(Some(line.to_owned())),
                Some(Step::Fault) => Err(io::Error::other("injected read fault")),
                None => Ok(None),
            }
        }

        fn seek_to_end(&mut self) -> io::Result<()> {
            self.queues
                .borrow_mut()
                .entry(self.identity)
                .or_default()
                .clear();
            Ok(())
        }
    }

    struct Clock(Cell<NaiveDate>);

    impl OplogClock for Clock {
        fn today(&self) -> NaiveDate {
            self.0.get()
        }
    }

    struct Probe(Cell<LeaseProbe>);

    impl OplogIdentityProbe for Probe {
        fn probe(&self, _entry: &OplogCatalogEntry) -> LeaseProbe {
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

    fn add(temporary: &TempDir, source: &str, day: NaiveDate) -> OplogCatalogSnapshot {
        let writer = create_oplog_at(
            JournalRoot::open(temporary.path()).unwrap(),
            source,
            "run",
            OplogFormat::Log,
            instant(day),
        )
        .unwrap();
        drop(writer);
        catalog_oplogs(JournalRoot::open(temporary.path()).unwrap(), &[day]).unwrap()
    }

    fn push(
        queues: &Rc<RefCell<HashMap<OplogFileIdentity, VecDeque<Step>>>>,
        entry: &OplogCatalogEntry,
        steps: impl IntoIterator<Item = Step>,
    ) {
        queues
            .borrow_mut()
            .entry(entry.identity())
            .or_default()
            .extend(steps);
    }

    #[test]
    fn initial_entries_start_at_eof_and_late_bytes_emit_once() {
        let temporary = tempfile::tempdir().unwrap();
        let snapshot = add(&temporary, "existing", date());
        let entry = snapshot.entries()[0].clone();
        let queues = Rc::new(RefCell::new(HashMap::new()));
        push(&queues, &entry, [Step::Line("preexisting")]);
        let source = Source::new(snapshot);
        let factory = ReaderFactory {
            queues: queues.clone(),
        };
        let clock = Clock(Cell::new(date()));
        let initial = OplogFollower::discover_initial(&source, &factory, &clock).unwrap();
        let mut follower = OplogFollower::from_state(initial.state);
        let probe = Probe(Cell::new(LeaseProbe::Active));
        let mut emitted = Vec::new();
        follower
            .tick(
                &source,
                &factory,
                &probe,
                &clock,
                &|| false,
                &mut |_, line| emitted.push(line),
            )
            .unwrap();
        assert!(emitted.is_empty());
        push(&queues, &entry, [Step::Line("late")]);
        follower
            .tick(
                &source,
                &factory,
                &probe,
                &clock,
                &|| false,
                &mut |_, line| emitted.push(line),
            )
            .unwrap();
        assert_eq!(emitted, ["late"]);
    }

    #[test]
    fn newly_discovered_entries_start_at_zero_and_are_bounded() {
        let temporary = tempfile::tempdir().unwrap();
        let initial_snapshot = add(&temporary, "first", date());
        let expanded_snapshot = add(&temporary, "second", date());
        let second = expanded_snapshot
            .entries()
            .iter()
            .find(|entry| entry.name().source().display_slug() == "second")
            .unwrap()
            .clone();
        let queues = Rc::new(RefCell::new(HashMap::new()));
        let source = Source::new(initial_snapshot);
        let factory = ReaderFactory {
            queues: queues.clone(),
        };
        let clock = Clock(Cell::new(date()));
        let initial = OplogFollower::discover_initial(&source, &factory, &clock).unwrap();
        let mut follower = OplogFollower::from_state(initial.state);
        source.snapshot.replace(expanded_snapshot);
        push(&queues, &second, [Step::Line("new")]);
        let probe = Probe(Cell::new(LeaseProbe::Active));
        let mut emitted = Vec::new();
        follower
            .tick(
                &source,
                &factory,
                &probe,
                &clock,
                &|| false,
                &mut |_, line| emitted.push(line),
            )
            .unwrap();
        assert!(emitted.is_empty());
        follower
            .tick(
                &source,
                &factory,
                &probe,
                &clock,
                &|| false,
                &mut |_, line| emitted.push(line),
            )
            .unwrap();
        assert_eq!(emitted, ["new"]);

        let mut many = Vec::new();
        for _ in 0..=OPLOG_FOLLOW_MAX_LINES_PER_TICK {
            many.push(Step::Line("bounded"));
        }
        push(&queues, &second, many);
        emitted.clear();
        follower
            .tick(
                &source,
                &factory,
                &probe,
                &clock,
                &|| false,
                &mut |_, line| emitted.push(line),
            )
            .unwrap();
        assert_eq!(emitted.len(), OPLOG_FOLLOW_MAX_LINES_PER_TICK);
        follower
            .tick(
                &source,
                &factory,
                &probe,
                &clock,
                &|| false,
                &mut |_, line| emitted.push(line),
            )
            .unwrap();
        assert_eq!(emitted.len(), OPLOG_FOLLOW_MAX_LINES_PER_TICK + 1);
    }

    #[test]
    fn read_fault_does_not_duplicate_the_pending_line_after_recovery() {
        let temporary = tempfile::tempdir().unwrap();
        let snapshot = add(&temporary, "fault", date());
        let entry = snapshot.entries()[0].clone();
        let queues = Rc::new(RefCell::new(HashMap::new()));
        let source = Source::new(snapshot);
        let factory = ReaderFactory {
            queues: queues.clone(),
        };
        let clock = Clock(Cell::new(date()));
        let initial = OplogFollower::discover_initial(&source, &factory, &clock).unwrap();
        let mut follower = OplogFollower::from_state(initial.state);
        push(
            &queues,
            &entry,
            [Step::Line("first"), Step::Fault, Step::Line("sentinel")],
        );
        let probe = Probe(Cell::new(LeaseProbe::Active));
        let mut emitted = Vec::new();
        assert!(
            follower
                .tick(
                    &source,
                    &factory,
                    &probe,
                    &clock,
                    &|| false,
                    &mut |_, line| emitted.push(line)
                )
                .is_err()
        );
        assert_eq!(emitted, ["first"]);
        follower
            .tick(
                &source,
                &factory,
                &probe,
                &clock,
                &|| false,
                &mut |_, line| emitted.push(line),
            )
            .unwrap();
        follower
            .tick(
                &source,
                &factory,
                &probe,
                &clock,
                &|| false,
                &mut |_, line| emitted.push(line),
            )
            .unwrap();
        assert_eq!(emitted, ["first", "sentinel"]);
    }

    #[test]
    fn live_identity_accepts_monotonic_catalogued_growth_across_ticks() {
        use std::io::Write;

        let temporary = tempfile::tempdir().unwrap();
        let snapshot = add(&temporary, "growing", date());
        let entry = snapshot.entries()[0].clone();
        let queues = Rc::new(RefCell::new(HashMap::new()));
        let source = Source::new(snapshot);
        let factory = ReaderFactory {
            queues: queues.clone(),
        };
        let clock = Clock(Cell::new(date()));
        let initial = OplogFollower::discover_initial(&source, &factory, &clock).unwrap();
        let mut follower = OplogFollower::from_state(initial.state);
        let path = temporary
            .path()
            .join("chronicle")
            .join(entry.day())
            .join("health")
            .join(entry.leaf());
        let probe = Probe(Cell::new(LeaseProbe::Active));
        let mut emitted = Vec::new();
        let mut sizes = Vec::new();

        for line in ["first", "second", "third"] {
            writeln!(
                std::fs::OpenOptions::new()
                    .append(true)
                    .open(&path)
                    .unwrap(),
                "{line}"
            )
            .unwrap();
            let fresh =
                catalog_oplogs(JournalRoot::open(temporary.path()).unwrap(), &[date()]).unwrap();
            sizes.push(fresh.entries()[0].size());
            source.snapshot.replace(fresh);
            push(&queues, &entry, [Step::Line(line)]);
            assert_eq!(
                follower
                    .tick(
                        &source,
                        &factory,
                        &probe,
                        &clock,
                        &|| false,
                        &mut |_, line| emitted.push(line),
                    )
                    .unwrap(),
                OplogFollowTickOutcome::Continued
            );
        }

        assert_eq!(emitted, ["first", "second", "third"]);
        assert!(sizes.windows(2).all(|pair| pair[0] < pair[1]));
        assert_eq!(
            follower
                .state
                .tracked
                .get(&entry.identity())
                .unwrap()
                .entry
                .size(),
            *sizes.last().unwrap()
        );
    }

    #[test]
    fn shrinking_catalogued_size_for_a_live_identity_fails_closed() {
        use std::io::Write;

        let temporary = tempfile::tempdir().unwrap();
        let initial_snapshot = add(&temporary, "sized", date());
        let initial_entry = initial_snapshot.entries()[0].clone();
        let path = temporary
            .path()
            .join("chronicle")
            .join(initial_entry.day())
            .join("health")
            .join(initial_entry.leaf());
        writeln!(
            std::fs::OpenOptions::new()
                .append(true)
                .open(&path)
                .unwrap(),
            "late"
        )
        .unwrap();
        let snapshot =
            catalog_oplogs(JournalRoot::open(temporary.path()).unwrap(), &[date()]).unwrap();
        let entry = snapshot.entries()[0].clone();
        let queues = Rc::new(RefCell::new(HashMap::new()));
        let source = Source::new(snapshot);
        let factory = ReaderFactory { queues };
        let clock = Clock(Cell::new(date()));
        let initial = OplogFollower::discover_initial(&source, &factory, &clock).unwrap();
        let mut follower = OplogFollower::from_state(initial.state);
        std::fs::OpenOptions::new()
            .write(true)
            .open(path)
            .unwrap()
            .set_len(entry.payload_offset() as u64)
            .unwrap();
        source.snapshot.replace(
            catalog_oplogs(JournalRoot::open(temporary.path()).unwrap(), &[date()]).unwrap(),
        );
        let probe = Probe(Cell::new(LeaseProbe::Active));
        assert!(
            follower
                .tick(&source, &factory, &probe, &clock, &|| false, &mut |_, _| {})
                .is_err()
        );
    }

    #[test]
    fn tracked_entries_drain_in_catalog_order() {
        let temporary = tempfile::tempdir().unwrap();
        let _ = add(&temporary, "alpha", date());
        let snapshot = add(&temporary, "beta", date());
        let entries = snapshot.entries().to_vec();
        let queues = Rc::new(RefCell::new(HashMap::new()));
        let source = Source::new(snapshot);
        let factory = ReaderFactory {
            queues: queues.clone(),
        };
        let clock = Clock(Cell::new(date()));
        let initial = OplogFollower::discover_initial(&source, &factory, &clock).unwrap();
        let mut follower = OplogFollower::from_state(initial.state);
        for (entry, line) in entries.iter().zip(["first", "second"]) {
            push(&queues, entry, [Step::Line(line)]);
        }
        let probe = Probe(Cell::new(LeaseProbe::Active));
        let mut emitted = Vec::new();
        follower
            .tick(
                &source,
                &factory,
                &probe,
                &clock,
                &|| false,
                &mut |entry, line| {
                    emitted.push((entry.name().source().display_slug().to_owned(), line));
                },
            )
            .unwrap();
        assert_eq!(
            emitted,
            entries
                .iter()
                .zip(["first", "second"])
                .map(|(entry, line)| (
                    entry.name().source().display_slug().to_owned(),
                    line.to_owned()
                ))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn released_and_indeterminate_absence_follow_the_tombstone_rules() {
        let temporary = tempfile::tempdir().unwrap();
        let snapshot = add(&temporary, "retired", date());
        let entry = snapshot.entries()[0].clone();
        let queues = Rc::new(RefCell::new(HashMap::new()));
        let source = Source::new(snapshot.clone());
        let factory = ReaderFactory {
            queues: queues.clone(),
        };
        let clock = Clock(Cell::new(date()));
        let initial = OplogFollower::discover_initial(&source, &factory, &clock).unwrap();
        let mut follower = OplogFollower::from_state(initial.state);
        let probe = Probe(Cell::new(LeaseProbe::Indeterminate));
        source.snapshot.replace(OplogCatalogSnapshot::default());
        follower
            .tick(&source, &factory, &probe, &clock, &|| false, &mut |_, _| {})
            .unwrap();
        assert_eq!(follower.state.tracked.len(), 1);

        probe.0.set(LeaseProbe::Released);
        follower
            .tick(&source, &factory, &probe, &clock, &|| false, &mut |_, _| {})
            .unwrap();
        assert!(follower.state.tracked.is_empty());
        assert_eq!(follower.state.tombstones.len(), 1);
        source.snapshot.replace(snapshot);
        push(&queues, &entry, [Step::Line("must-not-replay")]);
        let mut emitted = Vec::new();
        follower
            .tick(
                &source,
                &factory,
                &probe,
                &clock,
                &|| false,
                &mut |_, line| emitted.push(line),
            )
            .unwrap();
        assert!(emitted.is_empty());
        clock.0.set(date() + Duration::days(2));
        follower
            .tick(&source, &factory, &probe, &clock, &|| false, &mut |_, _| {})
            .unwrap();
        assert!(follower.state.tombstones.is_empty());
    }

    #[test]
    fn day_boundary_requests_current_and_prior_then_evicts_old_tombstones() {
        let temporary = tempfile::tempdir().unwrap();
        let snapshot = add(&temporary, "boundary", date());
        let queues = Rc::new(RefCell::new(HashMap::new()));
        let source = Source::new(snapshot);
        let factory = ReaderFactory { queues };
        let clock = Clock(Cell::new(date()));
        let initial = OplogFollower::discover_initial(&source, &factory, &clock).unwrap();
        let mut follower = OplogFollower::from_state(initial.state);
        let probe = Probe(Cell::new(LeaseProbe::Released));
        source.snapshot.replace(OplogCatalogSnapshot::default());
        follower
            .tick(&source, &factory, &probe, &clock, &|| false, &mut |_, _| {})
            .unwrap();
        assert_eq!(follower.state.tombstones.len(), 1);

        clock.0.set(date() + Duration::days(1));
        follower
            .tick(&source, &factory, &probe, &clock, &|| false, &mut |_, _| {})
            .unwrap();
        assert_eq!(
            source.requested_days.borrow().last().unwrap(),
            &[date(), date() + Duration::days(1)]
        );
        assert_eq!(follower.state.tombstones.len(), 1);

        clock.0.set(date() + Duration::days(2));
        follower
            .tick(&source, &factory, &probe, &clock, &|| false, &mut |_, _| {})
            .unwrap();
        assert_eq!(
            source.requested_days.borrow().last().unwrap(),
            &[date() + Duration::days(1), date() + Duration::days(2)]
        );
        assert!(follower.state.tombstones.is_empty());
    }
}
