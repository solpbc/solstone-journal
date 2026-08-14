// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::registry::{MaintBodyContext, MaintBodyResult};

pub fn unify_provider_config(c: &MaintBodyContext<'_>) -> MaintBodyResult {
    if c.dry_run {
        return ok("[DRY-RUN] Thinking provider configuration migration checked.");
    }
    match solstone_core_journal_config_write::unify_provider_config(c.journal) {
        Ok(false) => ok("Thinking provider config already unified."),
        Ok(true) => ok("Unified thinking provider config and removed retired provider settings."),
        Err(error) => failed(error),
    }
}

pub fn migrate_provider_install_state(c: &MaintBodyContext<'_>) -> MaintBodyResult {
    if c.dry_run {
        return ok("[DRY-RUN] Provider install state migration checked.");
    }
    match solstone_core_local::install::migration::migrate_legacy_provider_artifact_truth(c.journal)
    {
        Ok(result) if result.actions.is_empty() && result.removed == 0 && result.moved == 0 => {
            ok("Provider install state already uses provider-owned records.")
        }
        Ok(result) => {
            let mut lines = result.actions;
            if result.moved > 0 {
                lines.push("Moved local Vulkan device override to providers.local.".into());
            }
            if result.removed > 0 {
                lines.push("Removed legacy provider install state from providers.bundled.".into());
            }
            MaintBodyResult {
                stdout: lines,
                exit_code: 0,
            }
        }
        Err(error) => failed(error),
    }
}

pub fn pin_google_model_aliases(c: &MaintBodyContext<'_>) -> MaintBodyResult {
    if c.dry_run {
        return ok("[DRY-RUN] Google model alias migration checked.");
    }
    match solstone_core_journal_config_write::pin_google_model_aliases(c.journal) {
        Ok(result) if result.value.is_empty() => ok("Google model aliases already pinned."),
        Ok(result) => MaintBodyResult {
            stdout: result.value,
            exit_code: 0,
        },
        Err(error) => failed(error.to_string()),
    }
}

fn ok(line: &str) -> MaintBodyResult {
    MaintBodyResult {
        stdout: vec![line.to_owned()],
        exit_code: 0,
    }
}

fn failed(error: String) -> MaintBodyResult {
    MaintBodyResult {
        stdout: vec![error],
        exit_code: 1,
    }
}
