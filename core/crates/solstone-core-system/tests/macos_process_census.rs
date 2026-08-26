// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::os::unix::process::CommandExt;
use std::time::{Duration, Instant};

use solstone_core_system::process::{
    InstanceCensus, ProcessInstanceSource, SystemProcessInstanceSource,
};

#[test]
fn targeted_census_captures_a_live_child_and_grandchild() {
    let mut command = std::process::Command::new("/bin/sh");
    command.args(["-c", "sleep 30 & wait"]).process_group(0);
    let mut child = command.spawn().expect("spawn macOS census tree");
    let root_pid = child.id();
    let deadline = Instant::now() + Duration::from_secs(2);
    let census = loop {
        let census = SystemProcessInstanceSource.census_tree(root_pid, Some(deadline));
        if matches!(&census, InstanceCensus::Complete(rows) if rows.len() >= 2)
            || Instant::now() >= deadline
        {
            break census;
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let _ = nix::sys::signal::killpg(
        nix::unistd::Pid::from_raw(i32::try_from(root_pid).expect("root PID fits i32")),
        nix::sys::signal::Signal::SIGKILL,
    );
    let _ = child.wait();

    let InstanceCensus::Complete(rows) = census else {
        panic!("targeted macOS census was incomplete");
    };
    assert!(rows.iter().any(|row| row.instance.pid == root_pid));
    assert!(
        rows.iter().any(|row| row.ppid == root_pid),
        "targeted macOS census omitted the live grandchild edge"
    );
}
