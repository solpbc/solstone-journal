// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Supervisor availability checks shared by native journal-host commands.

use std::env;
use std::fs;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream};
use std::path::Path;
use std::time::Duration;

/// Interactive refusal text shared with the Python command contract.
pub const SUPERVISOR_MESSAGE: &str =
    "sol: solstone isn't running. Start it with 'journal up' and retry.";
const SUPERVISOR_TIMEOUT: Duration = Duration::from_millis(200);

/// The two unavailable-supervisor outcomes the CLI boundary distinguishes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SupervisorRefusal {
    SpawnedUnavailable,
    Unavailable,
}

impl SupervisorRefusal {
    /// The parent-facing message; spawned children deliberately stay silent.
    pub const fn message(self) -> Option<&'static str> {
        match self {
            Self::SpawnedUnavailable => None,
            Self::Unavailable => Some(SUPERVISOR_MESSAGE),
        }
    }
}

/// Check the current process environment and journal's recorded Convey port.
pub fn require_solstone(journal: &Path) -> Result<(), SupervisorRefusal> {
    require_solstone_with(|name| env::var(name).ok(), || is_solstone_up(journal))
}

/// Testable three-branch supervisor preflight.
pub fn require_solstone_with<E, C>(lookup_env: E, connectivity: C) -> Result<(), SupervisorRefusal>
where
    E: Fn(&str) -> Option<String>,
    C: FnOnce() -> bool,
{
    if lookup_env("SOL_SKIP_SUPERVISOR_CHECK").as_deref() == Some("1") || connectivity() {
        return Ok(());
    }
    if lookup_env("SOL_SUPERVISOR_SPAWNED").as_deref() == Some("1") {
        return Err(SupervisorRefusal::SpawnedUnavailable);
    }
    Err(SupervisorRefusal::Unavailable)
}

/// True only when the recorded local Convey port accepts a 200 ms TCP connection.
pub fn is_solstone_up(journal: &Path) -> bool {
    let Some(port) = read_convey_port(journal) else {
        return false;
    };
    TcpStream::connect_timeout(
        &SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port),
        SUPERVISOR_TIMEOUT,
    )
    .is_ok()
}

/// Read the locally recorded Convey port.
pub fn read_convey_port(journal: &Path) -> Option<u16> {
    fs::read_to_string(journal.join("health/convey.port"))
        .ok()?
        .trim()
        .parse()
        .ok()
}

#[cfg(test)]
#[allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod tests {
    use super::*;

    fn empty_env(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn skip_override_bypasses_connectivity() {
        assert!(
            require_solstone_with(
                |name| (name == "SOL_SKIP_SUPERVISOR_CHECK").then(|| "1".to_owned()),
                || false,
            )
            .is_ok()
        );
    }

    #[test]
    fn spawned_unavailable_is_silent() {
        let refusal = require_solstone_with(
            |name| (name == "SOL_SUPERVISOR_SPAWNED").then(|| "1".to_owned()),
            || false,
        )
        .unwrap_err();
        assert_eq!(refusal, SupervisorRefusal::SpawnedUnavailable);
        assert_eq!(refusal.message(), None);
    }

    #[test]
    fn unavailable_has_the_exact_interactive_message() {
        let refusal = require_solstone_with(empty_env, || false).unwrap_err();
        assert_eq!(refusal, SupervisorRefusal::Unavailable);
        assert_eq!(refusal.message(), Some(SUPERVISOR_MESSAGE));
    }
}
