// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Fixtures for lookups that name a key and no stream: a capture segment with,
//! beside it under the same key, a body window or an older Apple card.
//!
//! A lookup that takes the first key match only shows the fault when the
//! neighbour is listed first, and a day's streams list in directory order. So
//! the capture stream is chosen among names that also sort after the body
//! stream, keeping the first that the day lists after its neighbour, laying the
//! neighbour again after the capture where the filesystem lists newest first.

#![allow(clippy::disallowed_methods, clippy::disallowed_types)]

use std::fs;
use std::path::{Path, PathBuf};

/// The stream body windows from Apple Health sit in.
pub const BODY_STREAM: &str = "import.apple_health";

/// Capture stream names, each sorting after [`BODY_STREAM`].
const CAPTURE_STREAMS: &[&str] = &[
    "phone", "laptop", "watch", "tablet", "desk", "mic", "pc", "studio", "zz",
];

/// What sits beside the capture segment under the same key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Beside {
    /// The capture segment alone.
    Nothing,
    /// A body window: `body.jsonl` and its stream marker.
    BodyWindow,
    /// A body window the owner deleted: its tombstone shows `body.jsonl`.
    DeletedBodyWindow,
    /// An older Apple day-summary card in the body stream, which isn't a body
    /// window and keeps its behaviour.
    RetiredCard,
}

/// A laid fixture.
#[derive(Debug, Clone)]
pub struct Twins {
    /// The capture segment's stream name.
    pub capture_stream: String,
    /// The capture segment's directory.
    pub capture: PathBuf,
    /// The neighbour's directory, when there is one.
    pub neighbour: Option<PathBuf>,
}

fn write(dir: &Path, name: &str, contents: &str) {
    let path = dir.join(name);
    fs::create_dir_all(path.parent().expect("file has a parent")).expect("create fixture dir");
    fs::write(path, contents).expect("write fixture file");
}

fn lay_neighbour(day_dir: &Path, day: &str, key: &str, beside: Beside) -> Option<PathBuf> {
    let dir = day_dir.join(BODY_STREAM).join(key);
    match beside {
        Beside::Nothing => return None,
        Beside::BodyWindow => {
            write(
                &dir,
                "body.jsonl",
                "{\"kind\":\"reading\",\"id\":\"r1\",\"metric\":\"heart_rate\"}\n",
            );
            write(&dir, "stream.json", "{}\n");
        }
        Beside::DeletedBodyWindow => {
            write(
                &dir,
                "tombstone.json",
                &format!(
                    "{{\"manifest\":[\"chronicle/{day}/{BODY_STREAM}/{key}/body.jsonl\",\
                     \"chronicle/{day}/{BODY_STREAM}/{key}/stream.json\"]}}"
                ),
            );
        }
        Beside::RetiredCard => {
            write(
                &dir,
                "day_summary_transcript.md",
                "# Day summary\n\nSteps.\n",
            );
        }
    }
    Some(dir)
}

/// Whether the day directory lists `first`'s stream ahead of `second`'s.
fn listed_ahead(day_dir: &Path, first: &str, second: &str) -> bool {
    let names = fs::read_dir(day_dir)
        .expect("read fixture day")
        .map(|entry| entry.expect("fixture entry").file_name())
        .collect::<Vec<_>>();
    let at = |name: &str| names.iter().position(|entry| entry == name);
    matches!((at(first), at(second)), (Some(a), Some(b)) if a < b)
}

/// Lay `<capture stream>/<key>` holding `capture_files` and, beside it, what
/// `beside` names under the same key. With a neighbour, the capture stream is
/// one the day lists after it and that sorts after it, so a first-match lookup
/// reaches the neighbour first.
///
/// # Panics
///
/// When no candidate stream lists after the neighbour: the fixture can't show
/// the fault on this filesystem, so the test is inconclusive rather than green.
pub fn lay(
    journal: &Path,
    day: &str,
    key: &str,
    capture_files: &[(&str, &str)],
    beside: Beside,
) -> Twins {
    let day_dir = journal.join("chronicle").join(day);
    fs::create_dir_all(&day_dir).expect("create fixture day");
    let neighbour = lay_neighbour(&day_dir, day, key, beside);
    for stream in CAPTURE_STREAMS {
        let capture = day_dir.join(stream).join(key);
        fs::create_dir_all(&capture).expect("create capture segment");
        for (name, contents) in capture_files {
            write(&capture, name, contents);
        }
        let twins = || Twins {
            capture_stream: (*stream).to_owned(),
            capture: capture.clone(),
            neighbour: neighbour.clone(),
        };
        if neighbour.is_none() || listed_ahead(&day_dir, BODY_STREAM, stream) {
            return twins();
        }
        // Some filesystems list newest first: lay the neighbour again, after
        // the capture.
        fs::remove_dir_all(day_dir.join(BODY_STREAM)).expect("remove neighbour stream");
        lay_neighbour(&day_dir, day, key, beside);
        if listed_ahead(&day_dir, BODY_STREAM, stream) {
            return twins();
        }
        fs::remove_dir_all(day_dir.join(stream)).expect("remove candidate stream");
    }
    panic!("inconclusive: no capture stream lists after {BODY_STREAM} on this filesystem");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::is_body_only_dir;
    use crate::test_support::TempDir;

    #[test]
    fn each_neighbour_is_what_it_says_and_is_listed_first() {
        for (beside, body_only) in [
            (Beside::BodyWindow, true),
            (Beside::DeletedBodyWindow, true),
            (Beside::RetiredCard, false),
        ] {
            let journal = TempDir::new();
            let twins = lay(
                journal.path(),
                "20261010",
                "090000_300",
                &[("audio.jsonl", "{}\n")],
                beside,
            );
            let neighbour = twins.neighbour.expect("a neighbour");
            assert_eq!(
                is_body_only_dir(BODY_STREAM, &neighbour),
                body_only,
                "{beside:?}"
            );
            assert!(twins.capture_stream.as_str() > BODY_STREAM);
            let day_dir = journal.path().join("chronicle/20261010");
            assert!(listed_ahead(&day_dir, BODY_STREAM, &twins.capture_stream));
            assert!(twins.capture.join("audio.jsonl").is_file());
        }
    }
}
