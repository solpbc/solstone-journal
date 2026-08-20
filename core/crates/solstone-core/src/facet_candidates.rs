// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Orchestration for `journal facet-candidates`: supervisor gate, aggregation,
//! and durable upsert. CLI grammar lives in `solstone_core_cli`; this module
//! runs only after successful parsing.

use std::env;
use std::fs;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream};
use std::path::Path;
use std::process::ExitCode;
use std::time::Duration;

use chrono::Local;

const SUPERVISOR_MESSAGE: &str = "journal isn't running. start it with 'journal up' and retry.";
const SUPERVISOR_TIMEOUT: Duration = Duration::from_millis(200);

#[derive(Debug, PartialEq, Eq)]
enum SupervisorGate {
    Ready,
    SpawnedUnavailable,
    Unavailable,
}

fn require_solstone(journal_path: &Path) -> SupervisorGate {
    require_solstone_with(|name| env::var(name).ok(), || is_solstone_up(journal_path))
}

fn require_solstone_with<E, C>(lookup_env: E, connectivity: C) -> SupervisorGate
where
    E: Fn(&str) -> Option<String>,
    C: FnOnce() -> bool,
{
    if lookup_env("SOL_SKIP_SUPERVISOR_CHECK").as_deref() == Some("1") || connectivity() {
        return SupervisorGate::Ready;
    }
    if lookup_env("SOL_SUPERVISOR_SPAWNED").as_deref() == Some("1") {
        return SupervisorGate::SpawnedUnavailable;
    }
    SupervisorGate::Unavailable
}

fn is_solstone_up(journal_path: &Path) -> bool {
    let Some(port) = read_convey_port(journal_path) else {
        return false;
    };
    TcpStream::connect_timeout(
        &SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port),
        SUPERVISOR_TIMEOUT,
    )
    .is_ok()
}

fn read_convey_port(journal_path: &Path) -> Option<u16> {
    fs::read_to_string(journal_path.join("health/convey.port"))
        .ok()?
        .trim()
        .parse()
        .ok()
}

/// Run `journal facet-candidates` against the process-resolved journal root.
pub(crate) fn run(journal_path: &Path) -> ExitCode {
    if !journal_path.is_dir() {
        eprintln!(
            "journal facet-candidates: error: journal root {} is not a directory",
            journal_path.display()
        );
        return ExitCode::from(1);
    }

    match require_solstone(journal_path) {
        SupervisorGate::Ready => {}
        SupervisorGate::SpawnedUnavailable => return ExitCode::from(75),
        SupervisorGate::Unavailable => {
            eprintln!("{SUPERVISOR_MESSAGE}");
            return ExitCode::from(1);
        }
    }

    let today = Local::now().date_naive();
    let day = today.format("%Y%m%d").to_string();
    let candidates = match solstone_core_facets::aggregate_speculative_facets(
        journal_path,
        today,
        solstone_core_facets::FACET_CANDIDATE_MIN_SEGMENTS,
    ) {
        Ok(candidates) => candidates,
        Err(error) => {
            eprintln!("journal facet-candidates: error: {error}");
            return ExitCode::from(1);
        }
    };

    match solstone_core_facets::record_facet_candidates(journal_path, &day, &candidates) {
        Ok(count) => {
            println!("Recorded/updated {count} facet candidate(s).");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("journal facet-candidates: error: {error}");
            ExitCode::from(1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supervisor_gate_accepts_skip_or_connectivity_and_distinguishes_failures() {
        assert_eq!(
            require_solstone_with(
                |name| (name == "SOL_SKIP_SUPERVISOR_CHECK").then(|| "1".to_string()),
                || false,
            ),
            SupervisorGate::Ready
        );
        assert_eq!(
            require_solstone_with(|_| None, || true),
            SupervisorGate::Ready
        );
        assert_eq!(
            require_solstone_with(
                |name| (name == "SOL_SUPERVISOR_SPAWNED").then(|| "1".to_string()),
                || false,
            ),
            SupervisorGate::SpawnedUnavailable
        );
        assert_eq!(
            require_solstone_with(|_| None, || false),
            SupervisorGate::Unavailable
        );
    }
}
