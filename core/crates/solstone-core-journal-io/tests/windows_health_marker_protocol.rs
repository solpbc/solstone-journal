// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(windows)]

use std::fs;
use std::sync::{Arc, Barrier, mpsc};
use std::thread;
use std::time::Duration;

use solstone_core_journal_io::{
    DayMarkerPairStatus, HealthMarkerKind, HealthMarkerState, LockOptions, PublishOutcome,
    bump_stream_marker, day_marker_pair_status, health_marker_path, hold_lock,
    publish_daily_marker_if_current, read_health_marker,
};

const DAY: &str = "2026-09-02";
const COMPLETION_BOUND: Duration = Duration::from_secs(2);

fn temporary(label: &str) -> tempfile::TempDir {
    tempfile::Builder::new().prefix(label).tempdir().unwrap()
}

fn generation(root: &std::path::Path) -> u64 {
    match read_health_marker(root, DAY, HealthMarkerKind::Stream).unwrap() {
        HealthMarkerState::Versioned { marker, .. } => marker.generation,
        state => panic!("expected versioned stream marker, got {state:?}"),
    }
}

#[test]
fn windows_health_marker_protocol() {
    let journal = temporary("health-marker-public-");
    assert!(matches!(
        read_health_marker(journal.path(), DAY, HealthMarkerKind::Stream).unwrap(),
        HealthMarkerState::Absent
    ));
    assert_eq!(
        day_marker_pair_status(journal.path(), DAY).unwrap(),
        DayMarkerPairStatus::Complete
    );

    assert_eq!(bump_stream_marker(journal.path(), DAY).unwrap(), 1);
    assert_eq!(generation(journal.path()), 1);
    assert_eq!(
        day_marker_pair_status(journal.path(), DAY).unwrap(),
        DayMarkerPairStatus::Dirty
    );
    assert_eq!(
        publish_daily_marker_if_current(journal.path(), DAY, 1, "raw-v1", || {
            Ok("raw-v1".to_owned())
        })
        .unwrap(),
        PublishOutcome::Published(1)
    );
    assert!(matches!(
        read_health_marker(journal.path(), DAY, HealthMarkerKind::Daily).unwrap(),
        HealthMarkerState::Versioned { marker, .. }
            if marker.version == 1
                && marker.generation == 1
                && marker.fingerprint.as_deref() == Some("raw-v1")
    ));
    assert_eq!(
        day_marker_pair_status(journal.path(), DAY).unwrap(),
        DayMarkerPairStatus::Complete
    );

    assert_eq!(bump_stream_marker(journal.path(), DAY).unwrap(), 2);
    assert_eq!(
        day_marker_pair_status(journal.path(), DAY).unwrap(),
        DayMarkerPairStatus::Dirty
    );
    assert_eq!(
        publish_daily_marker_if_current(journal.path(), DAY, 2, "raw-v2", || {
            Ok("raw-v2".to_owned())
        })
        .unwrap(),
        PublishOutcome::Published(2)
    );
    assert_eq!(
        day_marker_pair_status(journal.path(), DAY).unwrap(),
        DayMarkerPairStatus::Complete
    );

    let legacy = temporary("health-marker-legacy-");
    for kind in [HealthMarkerKind::Stream, HealthMarkerKind::Daily] {
        let path = health_marker_path(legacy.path(), DAY, kind);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, []).unwrap();
        assert!(matches!(
            read_health_marker(legacy.path(), DAY, kind).unwrap(),
            HealthMarkerState::LegacyEmpty { .. }
        ));
    }

    let unlocked = temporary("health-marker-unlocked-");
    let unlocked_root = unlocked.path().to_owned();
    let unlocked_ready = Arc::new(Barrier::new(2));
    let unlocked_child_ready = Arc::clone(&unlocked_ready);
    let (unlocked_send, unlocked_receive) = mpsc::sync_channel(1);
    let unlocked_child = thread::spawn(move || {
        unlocked_child_ready.wait();
        unlocked_send
            .send(bump_stream_marker(&unlocked_root, DAY))
            .unwrap();
    });
    unlocked_ready.wait();
    assert_eq!(
        unlocked_receive
            .recv_timeout(COMPLETION_BOUND)
            .expect("unlocked positive-control bump must complete within the bound")
            .unwrap(),
        1
    );
    unlocked_child.join().unwrap();

    let locked = temporary("health-marker-locked-");
    assert_eq!(bump_stream_marker(locked.path(), DAY).unwrap(), 1);
    let stream = health_marker_path(locked.path(), DAY, HealthMarkerKind::Stream);
    let held = hold_lock(&stream, LockOptions::default()).unwrap();
    let locked_root = locked.path().to_owned();
    let locked_ready = Arc::new(Barrier::new(2));
    let locked_child_ready = Arc::clone(&locked_ready);
    let (locked_send, locked_receive) = mpsc::sync_channel(1);
    let locked_child = thread::spawn(move || {
        locked_child_ready.wait();
        locked_send
            .send(bump_stream_marker(&locked_root, DAY))
            .unwrap();
    });
    locked_ready.wait();
    assert!(
        matches!(
            locked_receive.recv_timeout(COMPLETION_BOUND),
            Err(mpsc::RecvTimeoutError::Timeout)
        ),
        "a competing bump must remain blocked while the real sidecar lock is held"
    );
    assert_eq!(generation(locked.path()), 1);
    drop(held);
    assert_eq!(
        locked_receive
            .recv_timeout(COMPLETION_BOUND)
            .expect("blocked bump must complete after the sidecar lock is released")
            .unwrap(),
        2
    );
    locked_child.join().unwrap();

    println!("JOURNAL_WIN_CI_HEALTH_MARKER=read/bump/lock/publish/legacy/pair/pass");
}
