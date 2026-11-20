// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Source declarations and accepted deliveries, restricted to current pairings.
use std::collections::BTreeMap;
use std::path::Path;

use serde::Serialize;
use sha2::{Digest, Sha256};
use solstone_core_sol_link::ledger::{
    AuthorizedClientsRead, ClientActivity, ClientEntry, DeviceActivityRead, SourceRecord,
    read_authorized_clients, read_device_activity,
};
use solstone_core_sol_link::source_declarations::{self, Declaration, DeclarationError, State};
use solstone_core_sol_link::{current_display_label, read_descriptions};

type Key = (String, Option<String>);
type History = BTreeMap<Key, String>;

#[derive(Serialize)]
pub struct Snapshot {
    pub sources_rev: String,
    pub self_cid: Option<String>,
    pub devices: Vec<Device>,
}
impl Snapshot {
    pub fn include_recent_requests(&mut self, seen: &mut BTreeMap<String, jiff::Timestamp>) {
        seen.retain(|cid, _| {
            self.devices
                .binary_search_by(|d| d.cid.as_str().cmp(cid.as_str()))
                .is_ok()
        });
        for device in &mut self.devices {
            if let Some(stamp) = seen.get(&device.cid)
                && device
                    .device_last_seen_at
                    .as_ref()
                    .and_then(|s| s.parse::<jiff::Timestamp>().ok())
                    .is_none_or(|old| *stamp > old)
            {
                device.device_last_seen_at = Some(stamp.to_string());
            }
        }
    }
}
#[derive(Serialize)]
pub struct Device {
    cid: String,
    display_label: String,
    platform: Option<String>,
    device_type: Option<String>,
    device_last_seen_at: Option<String>,
    sources: Vec<KnownSource>,
}
#[derive(Serialize)]
struct KnownSource {
    source: String,
    detail: Option<String>,
    state: Option<State>,
    origin: &'static str,
    last_synced_at: Option<String>,
    declared_at: Option<String>,
}

pub(crate) fn authorized(root: &Path) -> Result<Vec<ClientEntry>, DeclarationError> {
    match read_authorized_clients(&root.join("link/authorized_clients.json")) {
        AuthorizedClientsRead::Present(v) => Ok(v),
        AuthorizedClientsRead::Missing => Ok(Vec::new()),
        _ => Err(DeclarationError::Unavailable),
    }
}

fn observed(history: &mut History, key: Key, stamp: &str) {
    let Ok(time) = stamp.parse::<jiff::Timestamp>() else {
        return;
    };
    if history
        .get(&key)
        .and_then(|v| v.parse::<jiff::Timestamp>().ok())
        .is_none_or(|old| time > old)
    {
        history.insert(key, stamp.to_owned());
    }
}
fn history(
    root: &Path,
    cid: &str,
    activity: Option<&ClientActivity>,
    vaults: Option<&BTreeMap<String, String>>,
) -> History {
    let mut history = History::new();
    for (vault, stamp) in vaults.into_iter().flatten() {
        observed(
            &mut history,
            ("obsidian".into(), Some(vault.clone())),
            stamp,
        );
    }
    for (name, row) in activity.into_iter().flat_map(|a| &a.sources) {
        let SourceRecord::Valid(row) = row else {
            continue;
        };
        let Some(stamp) = &row.last_accepted_ingest_at else {
            continue;
        };
        let kind = match name.as_str() {
            "share" => continue,
            "obsidian" if vaults.is_some_and(|v| !v.is_empty()) => continue,
            "" | "mobile" | "mobile-segment" => {
                if let Some(segment) = &row.last_accepted_segment {
                    for kind in crate::carried_kinds(root, cid, name, &segment.day, &segment.name) {
                        observed(&mut history, (kind.as_str().into(), None), stamp);
                    }
                }
                continue;
            }
            "tmux" => "terminal",
            "browser" => "browsers",
            "watch-audio" => "watch",
            "obsidian" | "calendar" | "screenshots" | "audio" | "screen" | "location"
            | "camera" | "safari" | "terminal" | "browsers" | "watch" => name,
            _ => continue,
        };
        observed(&mut history, (kind.into(), None), stamp);
    }
    history
}

fn resolve(declaration: Option<&Declaration>, mut observed: History) -> Vec<KnownSource> {
    if let Some(decl) = declaration {
        return decl
            .sources
            .iter()
            .map(|s| KnownSource {
                source: s.source.clone(),
                detail: s.detail.clone(),
                state: Some(s.state.clone()),
                origin: "declared",
                last_synced_at: observed.remove(&(s.source.clone(), s.detail.clone())),
                declared_at: Some(decl.declared_at.clone()),
            })
            .collect();
    }
    observed
        .into_iter()
        .map(|((source, detail), stamp)| KnownSource {
            source,
            detail,
            state: None,
            origin: "observed",
            last_synced_at: Some(stamp),
            declared_at: None,
        })
        .collect()
}

// A status heartbeat only needs a revision. Compare inexpensive filesystem
// metadata before opening the registries or decoding legacy segment receipts.
// The cache belongs to the journal router, including when multiple journals
// are served in the same process.
#[derive(Default)]
pub struct RevisionCache {
    last: Option<(Stamps, String)>,
}

type Stamps = Vec<(std::path::PathBuf, Option<(std::time::SystemTime, u64)>)>;

fn registry_stamps(root: &Path) -> std::io::Result<Stamps> {
    let mut files: Vec<_> = [
        "link/authorized_clients.json",
        "link/client-descriptions.json",
        "link/devices.json",
        "link/source-declarations.json",
    ]
    .iter()
    .map(|name| root.join(name))
    .collect();
    match std::fs::read_dir(root.join("config/obsidian-vaults")) {
        Ok(entries) => {
            for entry in entries {
                files.push(entry?.path().join("vault.json"));
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    files.sort();
    files
        .into_iter()
        .map(|path| {
            let stamp = match path.metadata() {
                Ok(m) => Some((m.modified()?, m.len())),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                Err(e) => return Err(e),
            };
            Ok((path, stamp))
        })
        .collect()
}

impl RevisionCache {
    pub fn revision(&mut self, root: &Path) -> Option<String> {
        let stamps = registry_stamps(root).ok()?;
        if let Some((old, revision)) = &self.last
            && old == &stamps
        {
            return Some(revision.clone());
        }
        let revision = snapshot(root, None).ok()?.sources_rev;
        self.last = Some((stamps, revision.clone()));
        Some(revision)
    }
}

pub fn snapshot(root: &Path, cid: Option<String>) -> Result<Snapshot, DeclarationError> {
    let mut pairs = authorized(root)?;
    let descriptions = read_descriptions(root).map_err(|_| DeclarationError::Unavailable)?;
    if cid
        .as_ref()
        .is_some_and(|cid| !pairs.iter().any(|p| &p.fingerprint == cid))
    {
        return Err(DeclarationError::Revoked);
    }
    let declarations = source_declarations::read(root)?;
    let activities = match read_device_activity(&root.join("link/devices.json")) {
        DeviceActivityRead::Present(v) => v,
        DeviceActivityRead::Missing => BTreeMap::new(),
        _ => return Err(DeclarationError::Unavailable),
    };
    let vaults = crate::obsidian::synced_vaults(root).map_err(|_| DeclarationError::Unavailable)?;
    pairs.sort_by(|a, b| a.fingerprint.cmp(&b.fingerprint));
    let mut hash = Sha256::new();
    let mut devices = Vec::with_capacity(pairs.len());
    for pair in pairs {
        let stored = descriptions.get(&pair.fingerprint);
        let reported = stored.and_then(|d| d.reported.as_ref());
        let activity = activities.get(&pair.fingerprint);
        let declaration = declarations.get(&pair.fingerprint);
        let history = history(
            root,
            &pair.fingerprint,
            activity,
            vaults.get(&pair.fingerprint),
        );
        let device = Device {
            display_label: current_display_label(&pair, stored),
            platform: reported
                .and_then(|r| r.platform.clone())
                .or_else(|| pair.platform.map(|p| p.as_wire().to_string())),
            device_type: reported.and_then(|r| r.device_type.clone()),
            cid: pair.fingerprint,
            device_last_seen_at: activity.map(|a| a.last_seen_at.clone()),
            sources: resolve(declaration, history),
        };
        // Encode structural fields only, in canonical device and source order.
        let keys: Vec<_> = device
            .sources
            .iter()
            .map(|s| (&s.source, &s.detail, &s.state))
            .collect();
        serde_json::to_writer(
            &mut hash,
            &(
                &device.cid,
                &pair.paired_at,
                &device.display_label,
                &device.platform,
                &device.device_type,
                declaration.is_some(),
                keys,
            ),
        )
        .map_err(|_| DeclarationError::Unavailable)?;
        devices.push(device);
    }
    Ok(Snapshot {
        sources_rev: format!("{:x}", hash.finalize()),
        self_cid: cid,
        devices,
    })
}
