// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::await_outcome::{CargoTestEvidence, cargo_test_abort_discriminator};

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum RaceVerdict {
    Green,
    Inconclusive(String),
    Failed(String),
}

impl RaceVerdict {
    pub(crate) fn describe(&self) -> String {
        match self {
            Self::Green => "GREEN".to_owned(),
            Self::Inconclusive(detail) => format!("INCONCLUSIVE: {detail}"),
            Self::Failed(detail) => format!("FAILED: {detail}"),
        }
    }
}

pub(crate) fn classify(status: i32, output: &str) -> RaceVerdict {
    if status == 0 {
        return RaceVerdict::Green;
    }

    let named = named_failures(output);
    if !named.is_empty() {
        let ordinary = named
            .iter()
            .filter(|failure| !failure.marker_tagged)
            .map(|failure| failure.name.clone())
            .collect::<Vec<_>>();
        if !ordinary.is_empty() {
            return RaceVerdict::Failed(format!(
                "named libtest failure(s): {}",
                ordinary.join(", ")
            ));
        }
        return RaceVerdict::Inconclusive(format!(
            "SUPERVISOR_RACE_INCONCLUSIVE named libtest failure(s): {}",
            named
                .iter()
                .map(|failure| failure.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }

    match cargo_test_abort_discriminator(output) {
        CargoTestEvidence::RanWithoutParseableOutcome { target } => RaceVerdict::Inconclusive(
            format!("cargo test aborted before a parseable outcome: {target}"),
        ),
        CargoTestEvidence::NoTestBinaryEvidence => RaceVerdict::Failed(
            "cargo build or runner failure before test-binary evidence".to_owned(),
        ),
    }
}

struct NamedFailure {
    name: String,
    marker_tagged: bool,
}

fn named_failures(output: &str) -> Vec<NamedFailure> {
    const FAILURES: &str = "failures:\n";
    const RESULT: &str = "\ntest result: FAILED.";

    let mut failures = Vec::new();
    let mut cursor = 0;
    while let Some(relative) = output[cursor..].find(FAILURES) {
        let start = cursor + relative;
        if start > 0 && output.as_bytes()[start - 1] != b'\n' {
            cursor = start + FAILURES.len();
            continue;
        }
        let names_start = start + FAILURES.len();
        let Some(result_relative) = output[names_start..].find(RESULT) else {
            break;
        };
        let result = names_start + result_relative;
        for name in output[names_start..result]
            .lines()
            .filter_map(|line| line.strip_prefix("    ").filter(|name| !name.is_empty()))
        {
            failures.push(NamedFailure {
                name: name.to_owned(),
                marker_tagged: failure_section_has_marker(output, start, name),
            });
        }
        cursor = result + RESULT.len();
    }
    failures
}

fn failure_section_has_marker(output: &str, failures_start: usize, name: &str) -> bool {
    // The per-test stdout block lives AFTER the `failures:` line that introduces
    // it, so searching `output[..failures_start]` could never find the header --
    // it looked for the evidence strictly before the point where the evidence
    // begins, and a marked SUPERVISOR_RACE_INCONCLUSIVE wait was therefore routed as a hard
    // FAILED. That is the exact false red AC3 exists to prevent. Measured on a
    // quiet host: `dilation 1.22x` reported as `named libtest failure(s)`.
    let header = format!("---- {name} stdout ----");
    let Some(header_at) = output.find(&header) else {
        return false;
    };
    let section_start = header_at + header.len();
    // Bound the section at whichever comes first: the next test's stdout block,
    // or the name-list `failures:` block. Without a bound, one marked test would
    // lend its marker to every later failure in the same capture.
    let mut end = output.len();
    if let Some(next) = output[section_start..].find("\n---- ") {
        end = end.min(section_start + next);
    }
    if let Some(names) = output[section_start..].find("\nfailures:\n") {
        end = end.min(section_start + names);
    }
    let _ = failures_start;
    output[section_start..end].contains("SUPERVISOR_RACE_INCONCLUSIVE")
}
