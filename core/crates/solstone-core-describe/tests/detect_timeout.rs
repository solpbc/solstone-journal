// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(unix)]

use std::io::Read;
use std::process::{Command, Stdio};
use std::time::Duration;

use solstone_core_describe::detect::wait_for_child;

#[test]
fn detector_timeout_cancels_a_child_that_reported_ready() {
    let mut child = Command::new("sh")
        .args(["-c", "printf ready; exec sleep 120"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("start ready child");
    let mut ready = [0; 5];
    child
        .stdout
        .as_mut()
        .expect("child stdout")
        .read_exact(&mut ready)
        .expect("ready event");
    assert_eq!(&ready, b"ready");

    let error = wait_for_child(&mut child, Duration::from_millis(100))
        .expect_err("ready child must be cancelled at the failure ceiling");
    assert_eq!(error, "rfdetr-cli detect timed out after 100ms");
    assert!(child.try_wait().expect("reaped child").is_some());
}
