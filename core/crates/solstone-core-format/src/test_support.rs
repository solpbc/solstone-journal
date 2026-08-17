// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_TEMP_PATH: AtomicU64 = AtomicU64::new(0);

pub(crate) fn reserve_temp_path(prefix: &str) -> PathBuf {
    reserve_temp_path_with(prefix, &NEXT_TEMP_PATH)
}

fn reserve_temp_path_with(prefix: &str, sequence: &AtomicU64) -> PathBuf {
    loop {
        let sequence = sequence.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("{prefix}-{}-{sequence}", std::process::id()));
        match fs::create_dir(&path) {
            Ok(()) => {
                fs::remove_dir(&path).expect("release temporary path reservation");
                return path;
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => panic!("reserve temporary path {}: {error}", path.display()),
        }
    }
}

#[test]
fn temp_path_reservation_retries_a_collision_without_removing_it() {
    struct Collision(PathBuf);

    impl Drop for Collision {
        fn drop(&mut self) {
            let _ = fs::remove_dir(&self.0);
        }
    }

    static TEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let test_id = TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let prefix = format!("solstone-core-format-reservation-collision-{test_id}");
    let collision = std::env::temp_dir().join(format!("{prefix}-{}-0", std::process::id()));
    fs::create_dir(&collision).expect("create colliding path");
    let collision = Collision(collision);

    let reserved = reserve_temp_path_with(&prefix, &AtomicU64::new(0));

    assert!(collision.0.is_dir());
    assert_eq!(
        reserved.file_name().unwrap().to_string_lossy(),
        format!("{prefix}-{}-1", std::process::id())
    );
    assert!(!reserved.exists());
}
