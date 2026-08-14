// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::net::Ipv4Addr;
use std::path::Path;

use serde_json::Value;
use solstone_core_journal_io::LockOptions;

use crate::{ConfigMutationError, JournalConfigMutation, mutate_journal_config};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairingAddressMigrationReport {
    pub changed: bool,
}
pub fn migrate_pairing_home_address(
    journal: &Path,
) -> Result<PairingAddressMigrationReport, ConfigMutationError> {
    let transaction = mutate_journal_config(journal, LockOptions::default(), |config| {
        let changed = match config.get_mut("pairing").and_then(Value::as_object_mut) {
            Some(pairing) if pairing.contains_key("host_url") => {
                let legacy = pairing
                    .get("host_url")
                    .and_then(Value::as_str)
                    .and_then(parse);
                if let Some(home) = legacy {
                    pairing.insert("home_address".to_owned(), Value::String(home));
                }
                pairing.remove("host_url");
                true
            }
            _ => false,
        };
        JournalConfigMutation {
            changed,
            value: changed,
        }
    })?;
    Ok(PairingAddressMigrationReport {
        changed: transaction.value,
    })
}
fn parse(raw: &str) -> Option<String> {
    let raw = raw.trim();
    let rest = raw.strip_prefix("http://")?;
    let rest = rest.strip_suffix('/').unwrap_or(rest);
    if rest.contains(['/', '?', '#', '@']) {
        return None;
    }
    let (host, port) = rest.rsplit_once(':')?;
    let host = host.parse::<Ipv4Addr>().ok()?;
    let port = port.parse::<u16>().ok()?;
    if port != 7657
        || host.is_loopback()
        || host.is_unspecified()
        || host.is_link_local()
        || host.is_multicast()
    {
        return None;
    }
    Some(format!("{host}:{port}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;
    #[test]
    fn valid_moves_and_invalid_removes() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("config/journal.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            b"{\"pairing\":{\"host_url\":\"http://192.168.1.4:7657\"}}\n",
        )
        .unwrap();
        assert!(migrate_pairing_home_address(temp.path()).unwrap().changed);
        let value: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        assert_eq!(value["pairing"]["home_address"], "192.168.1.4:7657");
    }
}
