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
use solstone_core_sol_link::{
    current_display_label, live_seen_for, newer_last_seen_at, read_descriptions,
};

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
    folders: Option<&BTreeMap<String, String>>,
    use_safari: bool,
) -> History {
    let mut history = History::new();
    for (vault, stamp) in vaults.into_iter().flatten() {
        observed(
            &mut history,
            ("obsidian".into(), Some(vault.clone())),
            stamp,
        );
    }
    for (folder, stamp) in folders.into_iter().flatten() {
        observed(&mut history, ("folder".into(), Some(folder.clone())), stamp);
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
            "folder" if folders.is_some_and(|f| !f.is_empty()) => continue,
            "" | "mobile" | "mobile-segment" => {
                if let Some(segment) = &row.last_accepted_segment {
                    for kind in crate::carried_kinds(root, cid, name, &segment.day, &segment.name) {
                        observed(&mut history, (kind.as_str().into(), None), stamp);
                    }
                }
                continue;
            }
            "tmux" => "terminal",
            "browser" => {
                if use_safari {
                    "safari"
                } else {
                    "browsers"
                }
            }
            "watch-audio" => "watch",
            "folder" | "obsidian" | "calendar" | "screenshots" | "audio" | "screen"
            | "location" | "camera" | "safari" | "terminal" | "browsers" | "watch" => name,
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
    match std::fs::read_dir(root.join("config/folders")) {
        Ok(entries) => {
            for entry in entries {
                let entry = entry?;
                if entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| name.starts_with('.'))
                {
                    continue;
                }
                files.push(entry.path().join("folder.json"));
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
    let live_seen = live_seen_for(root);
    let vaults = crate::obsidian::synced_vaults(root).map_err(|_| DeclarationError::Unavailable)?;
    let folders = crate::folder::synced_folders(root).map_err(|_| DeclarationError::Unavailable)?;
    pairs.sort_by(|a, b| a.fingerprint.cmp(&b.fingerprint));
    let mut hash = Sha256::new();
    let mut devices = Vec::with_capacity(pairs.len());
    for pair in pairs {
        let stored = descriptions.get(&pair.fingerprint);
        let reported = stored.and_then(|d| d.reported.as_ref());
        let activity = activities.get(&pair.fingerprint);
        let declaration = declarations.get(&pair.fingerprint);
        let platform = reported
            .and_then(|r| r.platform.clone())
            .or_else(|| pair.platform.map(|p| p.as_wire().to_string()));
        let use_safari = declaration
            .is_some_and(|decl| decl.sources.iter().any(|s| s.source == "safari"))
            || platform
                .as_deref()
                .is_some_and(|p| p.eq_ignore_ascii_case("ios") || p.eq_ignore_ascii_case("ipados"));
        let history = history(
            root,
            &pair.fingerprint,
            activity,
            vaults.get(&pair.fingerprint),
            folders.get(&pair.fingerprint),
            use_safari,
        );
        let device_last_seen_at = newer_last_seen_at(
            activity.map(|a| a.last_seen_at.as_str()),
            live_seen.get(&pair.fingerprint).map(String::as_str),
        );
        let device = Device {
            display_label: current_display_label(&pair, stored),
            platform,
            device_type: reported.and_then(|r| r.device_type.clone()),
            cid: pair.fingerprint,
            device_last_seen_at,
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

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde_json::{Value, json};
    use solstone_core_sol_link::ledger::{
        AcceptedSegment, AuthorizationLedger, ClientEntry, ClientRole,
    };
    use solstone_core_sol_link::source_declarations::{self, Source, State};
    use solstone_core_sol_link::{
        Platform, ReportedDescription, StoredClientDescription, client_descriptions_path,
    };
    use tempfile::TempDir;

    use super::snapshot;

    fn pair_device(root: &std::path::Path, cid: &str, label: &str, platform: Option<Platform>) {
        let mut entry = ClientEntry::new(
            cid,
            label,
            "2026-08-13T00:00:00Z",
            "instance",
            ClientRole::Roleless,
        );
        entry.platform = platform;
        AuthorizationLedger::new(root).add(entry).unwrap();
    }

    fn write_reported(
        root: &std::path::Path,
        rows: impl IntoIterator<Item = (&'static str, ReportedDescription)>,
    ) {
        let map: BTreeMap<String, StoredClientDescription> = rows
            .into_iter()
            .map(|(cid, reported)| {
                let mut stored = StoredClientDescription::initial();
                stored.revision = 1;
                stored.reported = Some(reported);
                stored.updated_at = Some("2026-08-13T00:01:30Z".to_owned());
                (cid.to_owned(), stored)
            })
            .collect();
        std::fs::write(
            client_descriptions_path(root),
            serde_json::to_vec(&map).unwrap(),
        )
        .unwrap();
    }

    fn record_browser_upload(root: &std::path::Path, cid: &str, stamp: &str) {
        AuthorizationLedger::new(root)
            .record_accepted_ingest(
                cid,
                stamp,
                AcceptedSegment {
                    day: "20260813".to_owned(),
                    name: "120000_1".to_owned(),
                },
                Some("browser"),
            )
            .unwrap();
    }

    #[test]
    fn ios_browser_uploads_map_to_safari_across_declared_reported_and_legacy_pairing_while_desktop_maps_to_browsers()
     {
        let dir = TempDir::new().unwrap();
        let root = dir.path();

        pair_device(root, "sha256:declared_safari", "iPhone declared", None);
        source_declarations::replace(
            root,
            "sha256:declared_safari",
            vec![Source {
                source: "safari".to_owned(),
                detail: None,
                state: State::On,
            }],
            "2026-08-13T00:01:00Z".to_owned(),
        )
        .unwrap();
        record_browser_upload(root, "sha256:declared_safari", "2026-08-13T00:02:00Z");

        for (cid, label, _) in [
            ("sha256:reported_ios", "iPhone reported", "ios"),
            ("sha256:reported_ipados", "iPad reported", "ipados"),
        ] {
            pair_device(root, cid, label, None);
            record_browser_upload(root, cid, "2026-08-13T00:03:00Z");
        }
        write_reported(
            root,
            [
                (
                    "sha256:reported_ios",
                    ReportedDescription {
                        app_id: Some("app.solstone.ios".to_owned()),
                        platform: Some("ios".to_owned()),
                        device_type: Some("phone".to_owned()),
                        ..ReportedDescription::default()
                    },
                ),
                (
                    "sha256:reported_ipados",
                    ReportedDescription {
                        app_id: Some("app.solstone.ios".to_owned()),
                        platform: Some("ipados".to_owned()),
                        device_type: Some("tablet".to_owned()),
                        ..ReportedDescription::default()
                    },
                ),
            ],
        );

        pair_device(
            root,
            "sha256:legacy_ios",
            "iPhone legacy",
            Some(Platform::Ios),
        );
        record_browser_upload(root, "sha256:legacy_ios", "2026-08-13T00:04:00Z");

        pair_device(root, "sha256:desktop_mac", "MacBook", Some(Platform::Macos));
        record_browser_upload(root, "sha256:desktop_mac", "2026-08-13T00:05:00Z");

        let snap = serde_json::to_value(snapshot(root, None).unwrap()).unwrap();
        let devices = snap["devices"].as_array().unwrap();
        let by_cid = |cid: &str| -> &Value {
            devices
                .iter()
                .find(|d| d["cid"] == cid)
                .unwrap_or_else(|| panic!("missing device {cid}"))
        };

        assert_eq!(
            by_cid("sha256:declared_safari")["sources"],
            json!([{
                "source": "safari",
                "detail": null,
                "state": "on",
                "origin": "declared",
                "last_synced_at": "2026-08-13T00:02:00Z",
                "declared_at": "2026-08-13T00:01:00Z",
            }])
        );
        for cid in ["sha256:reported_ios", "sha256:reported_ipados"] {
            assert_eq!(
                by_cid(cid)["sources"],
                json!([{
                    "source": "safari",
                    "detail": null,
                    "state": null,
                    "origin": "observed",
                    "last_synced_at": "2026-08-13T00:03:00Z",
                    "declared_at": null,
                }])
            );
        }
        assert_eq!(
            by_cid("sha256:legacy_ios")["sources"],
            json!([{
                "source": "safari",
                "detail": null,
                "state": null,
                "origin": "observed",
                "last_synced_at": "2026-08-13T00:04:00Z",
                "declared_at": null,
            }])
        );
        assert_eq!(
            by_cid("sha256:desktop_mac")["sources"],
            json!([{
                "source": "browsers",
                "detail": null,
                "state": null,
                "origin": "observed",
                "last_synced_at": "2026-08-13T00:05:00Z",
                "declared_at": null,
            }])
        );
    }
}
