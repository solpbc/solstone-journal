// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use solstone_core_sol_client::aggregate::InventoryEntry;
pub(super) use solstone_core_sol_client::aggregate::entries;

fn message_id(operation: &str) -> String {
    let key = operation.replace(['-', '_'], ".");
    let restricted = match operation {
        "journal.facet.create" | "journal.facet.delete" | "journal.facet.rename" => "!",
        _ => "",
    };
    format!("{restricted}journal.cli.sol.help.command.{key}")
}

pub(super) fn description(locale: &str, entry: &InventoryEntry) -> String {
    if entry.help.is_empty() || !entries().iter().any(|known| std::ptr::eq(entry, known)) {
        return entry.help.to_owned();
    }
    solstone_core_cli_copy::render_for(locale, &message_id(entry.operation_id), &[])
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};
    use std::collections::HashSet;

    #[test]
    fn terminal_catalog_review_tracks_authority_wording() {
        let mut rows: Vec<_> = entries()
            .iter()
            .map(|entry| (entry.surface, entry.path, entry.operation_id, entry.help))
            .collect();
        rows.sort_unstable();
        let bytes = serde_json::to_vec(&rows).expect("authority strings serialize");
        assert_eq!(
            format!("{:x}", Sha256::digest(bytes)),
            include_str!("authority-help.sha256").trim(),
            "authority descriptions changed; review the terminal catalog against the native inventory, then run python3 scripts/build_native_sol_inventory.py --refresh-help-source",
        );
    }

    #[test]
    fn every_registered_nonempty_description_has_a_distinct_catalog_message() {
        let mut ids = HashSet::new();
        for entry in entries().iter().filter(|entry| !entry.help.is_empty()) {
            let id = message_id(entry.operation_id);
            assert!(
                ids.insert(id.clone()),
                "duplicate terminal inventory message {id}"
            );
            assert!(
                solstone_core_cli_copy::try_render_for(solstone_core_cli_copy::ENGLISH, &id, &[])
                    .is_some(),
                "missing terminal inventory description {id} for {path:?}; update the CLI catalog source and run make build-native-sol-inventory and make journal-cli-catalog",
                path = entry.path,
            );
        }
    }
}
