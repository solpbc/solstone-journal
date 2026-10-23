// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Optional development observations of existing events, never queue authority.
use serde_json::{Value, json};
use solstone_core_journal_io::JournalRoot;
use solstone_core_system::queue::{QueuedTaskSnapshot, TaskQueueEvent};
use std::{
    collections::hash_map::RandomState,
    hash::BuildHasher,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

const MAX_ROWS: usize = 32;
const MAX_AGE: Duration = Duration::from_secs(5);

#[derive(Default)]
struct Latest {
    sequence: u64,
    event: Option<(Instant, Value)>,
}

pub(crate) struct DailyQueueTap {
    root: Arc<JournalRoot>,
    host: String,
    sequence: AtomicU64,
    invalid: AtomicBool,
    closed: AtomicBool,
    latest: Mutex<Latest>,
}

impl DailyQueueTap {
    pub(crate) fn new(root: Arc<JournalRoot>) -> Self {
        // An opaque per-instance correlation token, not an access credential.
        let host = format!(
            "{:016x}{:016x}",
            RandomState::new().hash_one(0_u8),
            RandomState::new().hash_one(1_u8)
        );
        Self {
            root,
            host,
            sequence: AtomicU64::new(0),
            invalid: AtomicBool::new(false),
            closed: AtomicBool::new(false),
            latest: Mutex::new(Latest::default()),
        }
    }

    pub(crate) fn record(&self, event: &TaskQueueEvent) {
        let TaskQueueEvent::QueueChanged {
            partition,
            running_reference,
            queued_depth,
            queue,
        } = event
        else {
            return;
        };
        if partition.as_str() != "daily"
            || self.closed.load(Ordering::Acquire)
            || self.invalid.load(Ordering::Acquire)
        {
            return;
        }
        let Ok(previous) = self
            .sequence
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| n.checked_add(1))
        else {
            self.invalid.store(true, Ordering::Release);
            return;
        };
        let sequence = previous + 1;
        let emitted = Instant::now();
        // Never wait for an observer or execute caller callbacks on the queue.
        let Ok(mut latest) = self.latest.try_lock() else {
            self.invalid.store(true, Ordering::Release);
            return;
        };
        if sequence <= latest.sequence || self.closed.load(Ordering::Acquire) {
            self.invalid.store(true, Ordering::Release);
            return;
        }
        let mut represented = Vec::new();
        let mut ambiguous = *queued_depth != queue.len() || queue.len() > MAX_ROWS;
        for row in queue.iter().take(MAX_ROWS) {
            if let Some(value) = project(row) {
                represented.push(value);
            } else {
                ambiguous = true;
            }
        }
        latest.sequence = sequence;
        latest.event = Some((
            emitted,
            json!({
                "occupied_or_held":running_reference.is_some(),"queued":represented,
                "ambiguous":ambiguous,"queued_entries":queue.len()
            }),
        ));
    }

    pub(crate) fn binding(&self) -> Result<(String, String), &'static str> {
        if self.closed.load(Ordering::Acquire) {
            return Err("observer_stopped");
        }
        if self.invalid.load(Ordering::Acquire) {
            return Err("observer_lost_or_reordered_event");
        }
        if self.root.revalidate_canonical_binding().is_err() {
            self.invalid.store(true, Ordering::Release);
            return Err("observer_root_changed");
        }
        Ok((self.root.identity().observation_id(), self.host.clone()))
    }

    pub(crate) fn close(&self) {
        self.closed.store(true, Ordering::Release);
    }

    pub(crate) fn read(&self) -> Result<Value, &'static str> {
        self.read_using(Instant::now)
    }

    #[cfg(all(test, feature = "full-tests"))]
    fn read_at(&self, now: Instant) -> Result<Value, &'static str> {
        self.read_using(|| now)
    }

    fn read_using(&self, now: impl FnOnce() -> Instant) -> Result<Value, &'static str> {
        if self.closed.load(Ordering::Acquire) {
            return Err("observer_stopped");
        }
        if self.invalid.load(Ordering::Acquire) {
            return Err("observer_lost_or_reordered_event");
        }
        if self.root.revalidate_canonical_binding().is_err() {
            self.invalid.store(true, Ordering::Release);
            return Err("observer_root_changed");
        }
        let latest = self.latest.try_lock().map_err(|_| {
            self.invalid.store(true, Ordering::Release);
            "observer_busy"
        })?;
        let now = now();
        let sequence = latest.sequence;
        if sequence != self.sequence.load(Ordering::Acquire) {
            return Err("observer_event_in_flight");
        }
        let (age, event) = match &latest.event {
            None => (None, None),
            Some((at, event)) => {
                let age = now
                    .checked_duration_since(*at)
                    .ok_or("observer_clock_ambiguous")?;
                if age > MAX_AGE {
                    return Err("observer_emission_stale");
                }
                (Some(age.as_micros()), Some(event.clone()))
            }
        };
        if self.closed.load(Ordering::Acquire)
            || self.invalid.load(Ordering::Acquire)
            || sequence != self.sequence.load(Ordering::Acquire)
        {
            return Err("observer_changed_during_read");
        }
        Ok(
            json!({"native_root":self.root.identity().observation_id(),"host_instance":self.host,
                  "emission_sequence":sequence,"emission_age_us":age,"event":event,
                  "snapshot_order":"emission_only","queue_snapshot_age_known":false,
                  "authorizes_release":false}),
        )
    }
}

fn project(row: &QueuedTaskSnapshot) -> Option<Value> {
    let work = solstone_core_system::queue::DailyQueueWork::project(
        row.day.as_deref(),
        &row.command,
        &row.references,
        row.scheduler_name.as_deref(),
    )?;
    Some(json!(work))
}

pub(crate) fn live_read_age(
    started: Instant,
    finished: Instant,
    now: Instant,
) -> Result<u128, &'static str> {
    if finished.checked_duration_since(started).is_none()
        || now.checked_duration_since(finished).is_none()
    {
        return Err("observer_live_clock_ambiguous");
    }
    let age = now
        .checked_duration_since(started)
        .ok_or("observer_live_clock_ambiguous")?;
    if age > MAX_AGE {
        return Err("observer_live_read_stale");
    }
    Ok(age.as_micros())
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use super::*;
    use solstone_core_system::partition::{Partition, canonical_journal_command};

    const ID: &str = "0123456789abcdef0123456789abcdef";
    #[test]
    fn live_age_counts_validation_delay_and_rejects_clock_ambiguity() {
        let start = Instant::now();
        let finish = start + Duration::from_secs(4);
        assert_eq!(
            live_read_age(start, finish, start + Duration::from_secs(6)),
            Err("observer_live_read_stale")
        );
        assert_eq!(
            live_read_age(start, finish, start + Duration::from_secs(3)),
            Err("observer_live_clock_ambiguous")
        );
        assert_eq!(
            live_read_age(finish, start, finish),
            Err("observer_live_clock_ambiguous")
        );
        assert_eq!(live_read_age(start, finish, finish), Ok(4_000_000));
    }
    fn event() -> TaskQueueEvent {
        TaskQueueEvent::QueueChanged {
            partition: Partition::new("daily"),
            running_reference: None,
            queued_depth: 1,
            queue: vec![QueuedTaskSnapshot {
                references: vec![format!("supervisor-work-20261016-{ID}")],
                command: canonical_journal_command([
                    "think",
                    "--day",
                    "20261016",
                    "--from-scratch",
                    "--work-intent",
                    ID,
                ]),
                day: Some("20261016".into()),
                scheduler_name: None,
            }],
        }
    }
    fn tap() -> (tempfile::TempDir, DailyQueueTap) {
        let directory = tempfile::tempdir().unwrap();
        let root = Arc::new(JournalRoot::open(directory.path()).unwrap());
        (directory, DailyQueueTap::new(root))
    }
    #[test]
    fn exact_payload_is_root_host_bound_and_never_admission() {
        let (_directory, tap) = tap();
        assert!(tap.read().unwrap()["event"].is_null());
        tap.record(&event());
        let value = tap.read().unwrap();
        assert_eq!(value["native_root"], tap.root.identity().observation_id());
        assert_eq!(value["host_instance"], tap.host);
        assert_eq!(value["emission_sequence"], 1);
        assert_eq!(value["event"]["queued"][0]["request_id"], ID);
        assert_eq!(value["authorizes_release"], false);
        assert_eq!(value["queue_snapshot_age_known"], false);
        let (_other, other) = self::tap();
        assert_ne!(value["host_instance"], other.host);
        assert_ne!(value["native_root"], other.root.identity().observation_id());
    }
    #[test]
    fn unknown_arguments_mixed_references_and_occupancy_are_not_hidden() {
        let (_directory, tap) = tap();
        let mut value = event();
        if let TaskQueueEvent::QueueChanged {
            running_reference,
            queue,
            ..
        } = &mut value
        {
            *running_reference = Some("unrelated-private-reference".into());
            queue[0].command.push("SECRET".into());
        }
        tap.record(&value);
        let observed = tap.read().unwrap();
        assert_eq!(observed["event"]["occupied_or_held"], true);
        assert_eq!(observed["event"]["ambiguous"], true);
        assert!(observed["event"]["queued"].as_array().unwrap().is_empty());
        assert!(!observed.to_string().contains("SECRET"));
        assert!(!observed.to_string().contains("unrelated-private-reference"));
        let mut mixed = event();
        if let TaskQueueEvent::QueueChanged { queue, .. } = &mut mixed {
            queue[0].references.push("unrelated".into());
        }
        tap.record(&mixed);
        assert_eq!(tap.read().unwrap()["event"]["ambiguous"], true);
    }
    #[test]
    fn stale_future_stop_contention_and_overflow_refuse() {
        let (_directory, tap) = tap();
        tap.record(&event());
        let at = tap.latest.lock().unwrap().event.as_ref().unwrap().0;
        assert_eq!(
            tap.read_at(at + MAX_AGE + Duration::from_nanos(1)),
            Err("observer_emission_stale")
        );
        assert_eq!(
            tap.read_at(at - Duration::from_nanos(1)),
            Err("observer_clock_ambiguous")
        );
        {
            let _held = tap.latest.lock().unwrap();
            tap.record(&event());
        }
        assert_eq!(tap.read(), Err("observer_lost_or_reordered_event"));
        tap.record(&event());
        assert_eq!(tap.read(), Err("observer_lost_or_reordered_event"));
        let (_directory, stopped) = self::tap();
        stopped.record(&event());
        stopped.close();
        stopped.record(&event());
        assert_eq!(stopped.read(), Err("observer_stopped"));
        let (_directory, overflow) = self::tap();
        overflow.sequence.store(u64::MAX, Ordering::Release);
        overflow.record(&event());
        assert_eq!(overflow.read(), Err("observer_lost_or_reordered_event"));
    }
    #[test]
    fn late_old_queue_snapshot_remains_explicitly_historical() {
        let (_directory, tap) = tap();
        let older = event();
        let mut newer = event();
        if let TaskQueueEvent::QueueChanged {
            running_reference,
            queue,
            queued_depth,
            ..
        } = &mut newer
        {
            *running_reference = Some("reserved".into());
            queue.clear();
            *queued_depth = 0;
        }
        tap.record(&newer);
        tap.record(&older);
        let value = tap.read().unwrap();
        assert_eq!(value["emission_sequence"], 2);
        assert_eq!(value["event"]["occupied_or_held"], false);
        assert_eq!(value["authorizes_release"], false);
        assert_eq!(value["snapshot_order"], "emission_only");
    }
    #[test]
    fn replaced_root_refuses_even_after_the_original_is_restored() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("root");
        std::fs::create_dir(&path).unwrap();
        let root = Arc::new(JournalRoot::open(&path).unwrap());
        let tap = DailyQueueTap::new(root);
        tap.record(&event());
        let retained = directory.path().join("retained");
        std::fs::rename(&path, &retained).unwrap();
        std::fs::create_dir(&path).unwrap();
        assert_eq!(tap.read(), Err("observer_root_changed"));
        std::fs::remove_dir(&path).unwrap();
        std::fs::rename(retained, path).unwrap();
        assert_eq!(tap.read(), Err("observer_lost_or_reordered_event"));
    }
    #[test]
    fn bounded_grammar_refuses_invalid_days_ids_depth_and_large_queues() {
        let (_directory, tap) = tap();
        for mutate in 0..5 {
            let mut value = event();
            if let TaskQueueEvent::QueueChanged {
                queue,
                queued_depth,
                ..
            } = &mut value
            {
                match mutate {
                    0 => queue[0].day = Some("20260230".into()),
                    1 => queue[0].command[7] = "A".repeat(32),
                    2 => queue[0].scheduler_name = Some("other".into()),
                    3 => *queued_depth = 2,
                    _ => {
                        *queue = vec![queue[0].clone(); MAX_ROWS + 1];
                        *queued_depth = queue.len();
                    }
                }
            }
            tap.record(&value);
            let observed = tap.read().unwrap();
            assert_eq!(observed["event"]["ambiguous"], true);
            assert!(observed["event"]["queued"].as_array().unwrap().len() <= MAX_ROWS);
        }
    }
}
