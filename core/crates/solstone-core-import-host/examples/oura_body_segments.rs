// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Ask Oura for a range of days and publish what it answers as body windows, on a
//! journal you name.
//!
//! Distributions don't package examples. It runs the same path as the owner
//! command, on a journal you name:
//!
//!     cargo run -p solstone-core-import-host --example oura_body_segments -- \
//!         <journal> <YYYYMMDD_HHMMSS> [--from YYYY-MM-DD --to YYYY-MM-DD] \
//!         [--facet F] [--pages DIR]
//!
//! Without `--pages` it asks Oura itself, with the journal's own tokens. With
//! `--pages DIR` a folder of recorded responses answers instead: `index.json` and
//! the pages it names, each a body Oura returned. With no range, each endpoint is
//! asked for what coverage doesn't hold, and its trailing seven days.

use std::path::Path;

use chrono::NaiveDate;
use solstone_core_import_host::body_segments::{
    LiveOuraTransport, OuraAuth, OuraImportRequest, RecordedOura, import_oura,
};
use solstone_core_import_host::oura_coverage::coverage_text;

const USAGE: &str = "usage: oura_body_segments <journal> <YYYYMMDD_HHMMSS> [--from YYYY-MM-DD --to YYYY-MM-DD] [--facet F] [--pages DIR]";

fn usage() -> ! {
    eprintln!("{USAGE}");
    std::process::exit(2);
}

fn day(text: &str) -> NaiveDate {
    NaiveDate::parse_from_str(text, "%Y-%m-%d").unwrap_or_else(|_| usage())
}

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let [journal, timestamp, rest @ ..] = arguments.as_slice() else {
        usage();
    };
    let (mut from, mut to, mut facet, mut pages) = (None, None, None, None);
    let mut options = rest.iter();
    while let Some(option) = options.next() {
        match (option.as_str(), options.next()) {
            ("--from", Some(value)) => from = Some(day(value)),
            ("--to", Some(value)) => to = Some(day(value)),
            ("--facet", Some(value)) => facet = Some(value.as_str()),
            ("--pages", Some(value)) => pages = Some(value.as_str()),
            _ => usage(),
        }
    }
    let range = match (from, to) {
        (Some(from), Some(to)) => Some((from, to)),
        (None, None) => None,
        _ => usage(),
    };
    let mut recorded;
    let mut live;
    let (transport, auth): (&mut dyn solstone_core_body_ingest::oura_sync::OuraHttp, _) =
        match pages {
            Some(folder) => {
                recorded = RecordedOura::open(Path::new(folder)).unwrap_or_else(|error| {
                    eprintln!("oura import failed: {error}");
                    std::process::exit(1);
                });
                (&mut recorded, OuraAuth::None)
            }
            None => {
                live = LiveOuraTransport::default();
                (&mut live, OuraAuth::Journal)
            }
        };
    let mut request = OuraImportRequest::new(timestamp, facet, transport);
    request.range = range;
    request.auth = auth;
    match import_oura(Path::new(journal), request) {
        Ok(outcome) => {
            println!(
                "import {} published {} windows, finished: {}\n",
                outcome.import_id, outcome.published, outcome.finished
            );
            for result in &outcome.results {
                println!("{}", result.line());
            }
            println!("\n{}", coverage_text(&outcome.coverage));
            println!("{}", outcome.summary);
        }
        Err(error) => {
            eprintln!("oura import stopped: {error}");
            std::process::exit(1);
        }
    }
}
