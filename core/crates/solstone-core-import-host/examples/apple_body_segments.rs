// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Publish an Apple Health export as body windows, on a journal you name.
//!
//! No shipped command reaches this path yet; distributions don't package
//! examples. It exists to prove the path on a running journal:
//!
//!     cargo run -p solstone-core-import-host --example apple_body_segments -- \
//!         <journal> <export> <YYYYMMDD_HHMMSS> [facet]

use std::path::Path;

use solstone_core_import_host::body_segments::{AppleSegmentsRequest, run_apple_health_segments};

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let [journal, source, timestamp, rest @ ..] = arguments.as_slice() else {
        eprintln!("usage: apple_body_segments <journal> <export> <YYYYMMDD_HHMMSS> [facet]");
        std::process::exit(2);
    };
    let request = AppleSegmentsRequest {
        source: Path::new(source),
        timestamp,
        facet: rest.first().map(String::as_str),
    };
    match run_apple_health_segments(Path::new(journal), &request) {
        Ok(outcome) => {
            println!(
                "import {} published {} windows, finished: {}\n\n{}",
                outcome.import_id, outcome.published, outcome.finished, outcome.summary
            );
        }
        Err(error) => {
            eprintln!("apple health import failed: {error}");
            std::process::exit(1);
        }
    }
}
