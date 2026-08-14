// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use solstone_core_segment::{SupervisorRefusal, require_solstone_with};

use crate::CliError;

pub(crate) fn check<E, C>(lookup_env: E, connectivity: C) -> Result<(), CliError>
where
    E: Fn(&str) -> Option<String>,
    C: FnOnce() -> bool,
{
    require_solstone_with(lookup_env, connectivity).map_err(|refusal| match refusal {
        SupervisorRefusal::SpawnedUnavailable => CliError::SupervisorSpawnedUnavailable,
        SupervisorRefusal::Unavailable => CliError::SupervisorUnavailable,
    })
}
