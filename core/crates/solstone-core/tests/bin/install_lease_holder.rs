// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::env;
use std::fs;
use std::path::Path;
use std::thread;
use std::time::Duration;

use solstone_core_local::install::{lease, status};

fn main() {
    let mut args = env::args().skip(1);
    let journal = args.next().expect("journal path");
    let provider = args.next().expect("provider");
    let holding = args.next().expect("holding marker");
    let go = args.next().expect("go marker");
    let journal = Path::new(&journal);

    let _held = lease::acquire(journal, &provider)
        .expect("acquire lease")
        .expect("lease must be free for the holder");
    fs::write(&holding, "holding").expect("write holding marker");
    while !Path::new(&go).is_file() {
        thread::sleep(Duration::from_millis(10));
    }
    let current = status::read_status(journal, &provider).expect("read in-flight status");
    let installed = status::transition(current, "installed", None, None).expect("transition");
    status::write_status(journal, installed).expect("write installed status");
}
