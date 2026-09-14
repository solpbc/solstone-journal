// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Cross-process behaviour of the per-day Sense lease.
//!
//! This lives in `tests/` rather than beside the unit tests on purpose: it
//! spawns a second process, which the library/binary unit gate does not run.
//! The full-workspace gate does run it, and the properties it proves -- that
//! the lease is genuinely contended between processes and that it is released
//! by the kernel when a holder dies without the file being unlinked -- cannot
//! be observed from a single process at all.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use solstone_core_sense::lease::{acquire_sense_day_lease, sense_day_lease_path};

const DAY: &str = "20260914";

#[test]
fn sense_day_lease_is_contended_across_processes_and_survives_holder_death() {
    // Child role: hold the lease, announce it, then sit until killed.
    if let Ok(day) = std::env::var("SENSE_DAY_LEASE_CHILD_DAY") {
        let journal = PathBuf::from(
            std::env::var("SENSE_DAY_LEASE_CHILD_JOURNAL").expect("child journal path"),
        );
        let _lease = acquire_sense_day_lease(&journal, &day)
            .expect("child acquires lease")
            .expect("child lease is Some");
        let ready =
            PathBuf::from(std::env::var("SENSE_DAY_LEASE_CHILD_READY").expect("child ready path"));
        std::fs::write(&ready, b"ready").expect("child writes ready marker");
        std::thread::sleep(Duration::from_secs(20));
        return;
    }

    let temp = tempfile::tempdir().expect("tempdir");
    let journal = temp.path();
    let ready = journal.join("child_ready");

    let mut child = std::process::Command::new(std::env::current_exe().expect("current exe"))
        .arg("--exact")
        .arg("sense_day_lease_is_contended_across_processes_and_survives_holder_death")
        .arg("--nocapture")
        .env("SENSE_DAY_LEASE_CHILD_DAY", DAY)
        .env("SENSE_DAY_LEASE_CHILD_JOURNAL", journal)
        .env("SENSE_DAY_LEASE_CHILD_READY", &ready)
        .spawn()
        .expect("spawn lease-holding child");

    let deadline = Instant::now() + Duration::from_secs(10);
    while !ready.exists() {
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("child did not acquire the lease within its deadline");
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    let contended = acquire_sense_day_lease(journal, DAY).expect("parent acquire is checked");
    assert!(
        contended.is_none(),
        "the lease must be contended while another process holds it"
    );

    child.kill().expect("kill the lease holder");
    let _ = child.wait();

    let lease_path = sense_day_lease_path(journal, DAY);
    assert!(
        lease_path.is_file(),
        "the lease file must survive holder death; release must never unlink it"
    );

    let reacquired = acquire_sense_day_lease(journal, DAY)
        .expect("acquire after holder death is checked")
        .expect("the lease must be acquirable once the holder is gone");
    assert!(reacquired.path().is_file());
}
