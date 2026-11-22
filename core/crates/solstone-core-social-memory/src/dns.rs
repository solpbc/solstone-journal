// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Handle lookups in DNS, through the computer's own resolvers.

use std::time::Duration;

use solstone_core_oauth::atproto_client::HandleDns;

const TXT_TIMEOUT: Duration = Duration::from_secs(5);

/// Asks the system resolvers for a handle's `_atproto` TXT record.
pub struct SystemHandleDns;

impl HandleDns for SystemHandleDns {
    fn txt(&self, name: &str) -> Option<Vec<String>> {
        solstone_core_dns::lookup_txt(name, TXT_TIMEOUT).ok()
    }
}
