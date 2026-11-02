// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Low free space on the journal's disk, announced as an owner notice.
//!
//! Every few minutes the supervisor reads the free and total space of the
//! filesystem holding the journal. Below the journal's own free-space floor
//! for that disk (the same rule that suggests the offload floor) it raises
//! `disk_low:journal`, early enough that the owner can still free space or
//! move the journal before writes start failing. It clears only once free
//! space is well back above the floor, so a disk hovering at the line notifies
//! once. Between the two points it announces nothing and the current episode
//! stands. A read that fails is not a reading: nothing is announced.

use std::path::Path;
use std::time::{Duration, Instant};

use serde_json::{Map, Value, json};
use solstone_core_callosum::CallosumSocketServer;
use solstone_core_offload::measurement::{
    device_free_bytes, device_total_bytes, suggest_offload_defaults,
};

use super::bus::emit;

/// A statvfs-style read is cheap; free space moves over hours, not seconds.
pub(super) const CHECK_INTERVAL: Duration = Duration::from_secs(300);

const TRACT: &str = "owner_notice";
const KEY: &str = "disk_low:journal";
const ACTION: &str = "/app/settings#storage";
const LOW_TITLE: &str = "your journal's disk is running low on space";
const LOW_MESSAGE: &str = "free up some space on it, or move your journal to a larger disk.";
const BACK_TITLE: &str = "your journal's disk has space again";

/// Free and total bytes on the filesystem holding the journal.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct Space {
    pub free: u64,
    pub total: u64,
}

/// What one reading says about the episode.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Level {
    Low,
    Back,
}

pub(super) fn due(last: Option<Instant>, tick: Instant) -> bool {
    last.is_none_or(|last| tick.saturating_duration_since(last) >= CHECK_INTERVAL)
}

/// Read the journal's filesystem. `None` when either figure is unreadable.
pub(super) fn read(journal: &Path) -> Option<Space> {
    match (device_free_bytes(journal), device_total_bytes(journal)) {
        (Ok(free), Ok(total)) => Some(Space { free, total }),
        (free, total) => {
            log::debug!(
                "journal free space unreadable: free={:?} total={:?}",
                free.err(),
                total.err()
            );
            None
        }
    }
}

/// Raise below the floor; clear at or above the floor plus half again; say
/// nothing in between, or for a disk whose size is unknown.
pub(super) fn level(space: Space) -> Option<Level> {
    let floor = suggest_offload_defaults(space.total).ok()?.floor_bytes;
    let clear = floor.saturating_add(floor / 2);
    if space.free < floor {
        Some(Level::Low)
    } else if space.free >= clear {
        Some(Level::Back)
    } else {
        None
    }
}

/// The Callosum event and fields announcing `level`.
pub(super) fn announcement(level: Level) -> (&'static str, Map<String, Value>) {
    match level {
        Level::Low => (
            "raise",
            Map::from_iter([
                ("key".to_owned(), json!(KEY)),
                ("title".to_owned(), json!(LOW_TITLE)),
                ("message".to_owned(), json!(LOW_MESSAGE)),
                ("action".to_owned(), json!(ACTION)),
            ]),
        ),
        Level::Back => (
            "clear",
            Map::from_iter([
                ("key".to_owned(), json!(KEY)),
                ("title".to_owned(), json!(BACK_TITLE)),
            ]),
        ),
    }
}

/// One supervisor tick: when due, read the disk and announce what it says.
pub(super) fn check(
    journal: &Path,
    server: &CallosumSocketServer,
    last: &mut Option<Instant>,
    tick: Instant,
) {
    if !due(*last, tick) {
        return;
    }
    *last = Some(tick);
    announce(server, read(journal));
}

/// Broadcast what one reading says; an unreadable disk says nothing.
fn announce(server: &CallosumSocketServer, space: Option<Space>) {
    if let Some(level) = space.and_then(level) {
        let (event, extra) = announcement(level);
        emit(server, TRACT, event, extra);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GB: u64 = 1_000_000_000;

    fn on_disk(total: u64) -> impl Fn(u64) -> Option<Level> {
        move |free| level(Space { free, total })
    }

    #[test]
    fn checks_on_the_first_tick_and_then_every_few_minutes() {
        let origin = Instant::now();
        assert!(due(None, origin));
        assert!(!due(
            Some(origin),
            origin + CHECK_INTERVAL - Duration::from_secs(1)
        ));
        assert!(due(Some(origin), origin + CHECK_INTERVAL));
    }

    #[test]
    fn raises_below_the_floor_and_clears_only_well_above_it() {
        // A 100 GB disk keeps a 20 GB floor; it clears at 30 GB.
        let at = on_disk(100 * GB);
        assert_eq!(at(20 * GB - 1), Some(Level::Low));
        assert_eq!(at(0), Some(Level::Low));
        assert_eq!(at(20 * GB), None);
        assert_eq!(at(30 * GB - 1), None);
        assert_eq!(at(30 * GB), Some(Level::Back));
        assert_eq!(at(90 * GB), Some(Level::Back));
    }

    #[test]
    fn a_disk_hovering_at_the_line_announces_one_episode() {
        // Readings that cross the raise point back and forth without
        // reaching the clear point never produce a clear, so the episode
        // record sees one start and no end.
        let at = on_disk(100 * GB);
        let readings = [25, 19, 21, 19, 22, 20, 19, 29, 31, 29, 31];
        let levels = readings
            .iter()
            .filter_map(|free| at(free * GB))
            .collect::<Vec<_>>();
        let edges = levels
            .iter()
            .scan(None, |last, level| {
                let edge = (*last != Some(*level)).then_some(*level);
                *last = Some(*level);
                Some(edge)
            })
            .flatten()
            .collect::<Vec<_>>();
        assert_eq!(edges, [Level::Low, Level::Back]);
    }

    #[test]
    fn the_floor_follows_the_disk_size() {
        // Large disks keep a tenth, small ones a quarter.
        let large = on_disk(2_000 * GB);
        assert_eq!(large(200 * GB - 1), Some(Level::Low));
        assert_eq!(large(200 * GB), None);
        assert_eq!(large(300 * GB), Some(Level::Back));
        let small = on_disk(40 * GB);
        assert_eq!(small(10 * GB - 1), Some(Level::Low));
        assert_eq!(small(15 * GB), Some(Level::Back));
    }

    #[test]
    fn an_unknown_disk_size_says_nothing() {
        assert_eq!(level(Space { free: 0, total: 0 }), None);
    }

    #[test]
    fn announcements_carry_the_stable_key_and_a_journal_path() {
        let (event, raise) = announcement(Level::Low);
        assert_eq!(event, "raise");
        assert_eq!(raise["key"], KEY);
        let action = raise["action"].as_str().unwrap();
        assert!(action.starts_with("/app/") && !action.contains("//") && !action.contains(".."));
        assert!(!raise["title"].as_str().unwrap().is_empty());
        let (event, clear) = announcement(Level::Back);
        assert_eq!(event, "clear");
        assert_eq!(clear["key"], KEY);
        assert!(!clear["title"].as_str().unwrap().is_empty());
    }
}

#[cfg(all(test, unix, feature = "full-tests"))]
mod socket_tests {
    use std::sync::Arc;

    use solstone_core_callosum::{CallosumReceiveEvent, CallosumSocketConnection};
    use tempfile::TempDir;

    use super::*;

    #[test]
    fn reads_the_journal_filesystem() {
        let journal = TempDir::new().unwrap();
        let space = read(journal.path()).expect("a real directory is readable");
        assert!(space.total > 0 && space.free <= space.total);
        assert_eq!(read(&journal.path().join("missing")), None);
    }

    #[tokio::test]
    async fn readings_reach_the_bus_as_owner_notices_and_failed_reads_say_nothing() {
        const GB: u64 = 1_000_000_000;
        let journal = TempDir::new().unwrap();
        let socket = journal.path().join("callosum.sock");
        let server = Arc::new(CallosumSocketServer::bind(&socket).await.unwrap());
        let mut observer = CallosumSocketConnection::new(&socket, Map::new());
        observer.start();
        for _ in 0..200 {
            if server.client_count() > 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;

        let total = 100 * GB;
        let space = |free| Some(Space { free, total });
        // Low, low again, a failed read, hovering between the two points,
        // back, and a due check whose read fails on a missing path.
        announce(&server, space(5 * GB));
        announce(&server, space(19 * GB));
        announce(&server, None);
        announce(&server, space(25 * GB));
        announce(&server, space(40 * GB));
        let mut last = None;
        let tick = Instant::now();
        check(&journal.path().join("missing"), &server, &mut last, tick);
        assert_eq!(last, Some(tick));

        let mut seen = Vec::new();
        let _ = tokio::time::timeout(Duration::from_secs(2), async {
            while let Some(event) = observer.next_event().await {
                if let CallosumReceiveEvent::Envelope { envelope, .. } = event
                    && envelope.tract == TRACT
                {
                    seen.push((envelope.event, envelope.extra["key"].clone()));
                }
            }
        })
        .await;
        // Each reading is re-announced; the episode record turns the two
        // raises into one notice.
        assert_eq!(
            seen,
            [
                ("raise".to_owned(), json!(KEY)),
                ("raise".to_owned(), json!(KEY)),
                ("clear".to_owned(), json!(KEY)),
            ]
        );
    }
}
