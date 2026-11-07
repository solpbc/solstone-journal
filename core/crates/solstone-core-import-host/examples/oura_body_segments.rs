// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Publish a folder of staged Oura pages as body windows, on a journal you name.
//!
//! No shipped command reaches this path yet; distributions don't package
//! examples. It exists to prove the path on a running journal:
//!
//!     cargo run -p solstone-core-import-host --example oura_body_segments -- \
//!         <journal> <YYYYMMDD_HHMMSS> --pages DIR [--facet F]
//!
//! `DIR` holds `index.json` and the pages it names, each the body Oura returned.

use std::path::Path;

use solstone_core_import_host::body_segments::{OuraSegmentsRequest, run_oura_segments};

const USAGE: &str = "usage: oura_body_segments <journal> <YYYYMMDD_HHMMSS> --pages DIR [--facet F]";

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let [journal, timestamp, rest @ ..] = arguments.as_slice() else {
        eprintln!("{USAGE}");
        std::process::exit(2);
    };
    let mut pages = None;
    let mut facet = None;
    let mut options = rest.iter();
    while let Some(option) = options.next() {
        match (option.as_str(), options.next()) {
            ("--pages", Some(value)) => pages = Some(value.as_str()),
            ("--facet", Some(value)) => facet = Some(value.as_str()),
            _ => {
                eprintln!("{USAGE}");
                std::process::exit(2);
            }
        }
    }
    let Some(pages) = pages else {
        eprintln!("{USAGE}");
        std::process::exit(2);
    };
    let request = OuraSegmentsRequest::new(Path::new(pages), timestamp, facet);
    match run_oura_segments(Path::new(journal), &request) {
        Ok(outcome) => {
            println!(
                "import {} published {} windows, finished: {}\n",
                outcome.import_id, outcome.published, outcome.finished
            );
            if !outcome.holes.is_empty() {
                println!("Not written whole:");
                for hole in &outcome.holes {
                    println!("- {} {}: {}", hole.endpoint, hole.at, hole.reason.words());
                }
                println!();
            }
            println!("{}", outcome.summary);
        }
        Err(error) => {
            eprintln!("oura import failed: {error}");
            std::process::exit(1);
        }
    }
}
