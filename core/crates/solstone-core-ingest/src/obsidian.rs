// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Turning note changes from a vault into a memory.
//!
//! The journal keeps each note's latest text so it can work out what changed.
//! That text lives in `config/obsidian-vaults/<vault id>/` and is never a
//! memory, never searched and never shown. An upload's lines are applied in
//! time order against it, and what they amount to becomes one transcript for
//! the window. The state is committed only after that transcript is durable:
//! a crash between the two recomputes the same bytes, and a resend, or a
//! second device reporting what the journal already has, changes nothing.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use axum::Json;
use axum::extract::{Extension, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use solstone_core_convey_http::identity::AccessBasis;
use solstone_core_convey_http::owner_read::{OwnerReadRole, spawn_blocking_response};
use solstone_core_format::obsidian::{
    Change, KnownRename, NoteChange, NoteEntry, only_link_rewrites, render_edit, render_transcript,
};
use solstone_core_journal_io::{AtomicWriteOptions, atomic_replace};
use solstone_core_sol_link::ledger::AuthorizationLedger;

use crate::router::{IngestState, refusal};
use crate::validation::{validate_access, validate_protocol};

const STATE_DIR: &str = "config/obsidian-vaults";

/// How long a rename stays known, so the edits Obsidian makes to keep links
/// working can be folded into it when they arrive in a later window.
const RENAME_MEMORY_MS: i64 = 24 * 60 * 60 * 1_000;

const PRIVATE: AtomicWriteOptions = AtomicWriteOptions { mode: Some(0o600) };

#[derive(Clone, Debug, Deserialize, Serialize)]
struct NoteState {
    path: String,
    text: String,
    content_hash: String,
    changed_at_ms: i64,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct VaultState {
    vault: String,
    #[serde(default)]
    devices: BTreeMap<String, String>,
    #[serde(default)]
    renames: Vec<StoredRename>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct StoredRename {
    from: String,
    to: String,
    at_ms: i64,
}

fn digest(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}

fn vault_dir(journal: &Path, vault: &str) -> PathBuf {
    journal.join(STATE_DIR).join(&digest(vault)[..32])
}

fn note_file(journal: &Path, vault: &str, path: &str) -> PathBuf {
    let id = digest(path);
    vault_dir(journal, vault)
        .join("notes")
        .join(&id[..2])
        .join(format!("{id}.json"))
}

fn read_note(journal: &Path, vault: &str, path: &str) -> Result<Option<NoteState>, String> {
    read_json(&note_file(journal, vault, path))
}

fn read_vault(journal: &Path, vault: &str) -> Result<VaultState, String> {
    Ok(
        read_json(&vault_dir(journal, vault).join("vault.json"))?.unwrap_or_else(|| VaultState {
            vault: vault.to_owned(),
            ..VaultState::default()
        }),
    )
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<Option<T>, String> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|error| format!("{}: {error}", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("{}: {error}", path.display())),
    }
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let bytes = serde_json::to_vec(value).map_err(|error| error.to_string())?;
    atomic_replace(path, &bytes, PRIVATE).map_err(|error| format!("{}: {error}", path.display()))
}

type NoteKey = (String, String);

/// One note's net change across the lines of an upload.
struct Slot {
    vault: String,
    /// Where the note was when the upload began.
    start_path: String,
    /// Where it is now.
    path: String,
    /// What the journal held before the upload, or `None` at first sight.
    base: Option<String>,
    renamed_from: Option<String>,
    deleted: bool,
    /// Seen for the first time and gone again within one upload.
    vanished: bool,
    text: String,
    at_ms: i64,
}

/// What an upload amounts to, ready to commit once its memory is durable.
pub(crate) struct Prepared {
    /// The window's memory, or `None` when the upload changes nothing.
    pub(crate) transcript: Option<Vec<u8>>,
    notes: HashMap<NoteKey, Option<NoteState>>,
    vaults: BTreeMap<String, VaultUpdate>,
}

struct VaultUpdate {
    last_synced_at: String,
    last_synced_ms: i64,
    renames: Vec<StoredRename>,
}

/// Apply an upload's lines against the journal's state, writing nothing.
pub(crate) fn prepare(journal: &Path, mut changes: Vec<NoteChange>) -> Result<Prepared, String> {
    // The sort is stable, so lines at one moment keep the file's order.
    changes.sort_by_key(NoteChange::changed_at_ms);
    let mut overlay: HashMap<NoteKey, Option<NoteState>> = HashMap::new();
    let mut read: HashMap<NoteKey, Option<NoteState>> = HashMap::new();
    let mut slots: Vec<Slot> = Vec::new();
    let mut slot_at: HashMap<NoteKey, usize> = HashMap::new();
    let mut vaults: BTreeMap<String, VaultUpdate> = BTreeMap::new();

    for change in &changes {
        let at_ms = change.changed_at_ms();
        let update = vaults
            .entry(change.vault.clone())
            .or_insert_with(|| VaultUpdate {
                last_synced_at: change.observed_at.to_rfc3339(),
                last_synced_ms: change.observed_at.timestamp_millis(),
                renames: Vec::new(),
            });
        if change.observed_at.timestamp_millis() > update.last_synced_ms {
            update.last_synced_ms = change.observed_at.timestamp_millis();
            update.last_synced_at = change.observed_at.to_rfc3339();
        }
        let key = (change.vault.clone(), change.path.clone());
        match change.change {
            Change::Removed => {
                if let Some(index) = slot_at.remove(&key) {
                    overlay.insert(key, None);
                    let slot = &mut slots[index];
                    slot.deleted = true;
                    slot.vanished = slot.base.is_none();
                    continue;
                }
                let Some(current) = load(journal, &overlay, &mut read, &key)? else {
                    continue;
                };
                slot_at.insert(key.clone(), slots.len());
                slots.push(Slot {
                    vault: change.vault.clone(),
                    start_path: change.path.clone(),
                    path: change.path.clone(),
                    base: Some(current.text),
                    renamed_from: None,
                    deleted: true,
                    vanished: false,
                    text: String::new(),
                    at_ms,
                });
                overlay.insert(key, None);
            }
            Change::Changed => {
                let text = change.text.clone().unwrap_or_default();
                let hash = change.content_hash.clone().unwrap_or_default();
                if let Some(&index) = slot_at.get(&key) {
                    slots[index].text.clone_from(&text);
                    overlay.insert(key, Some(state(change, text, hash)));
                    continue;
                }
                let current = load(journal, &overlay, &mut read, &key)?;
                match &current {
                    Some(held) if held.content_hash == hash => continue,
                    Some(held) if at_ms < held.changed_at_ms => continue,
                    _ => {}
                }
                slot_at.insert(key.clone(), slots.len());
                slots.push(Slot {
                    vault: change.vault.clone(),
                    start_path: change.path.clone(),
                    path: change.path.clone(),
                    base: current.map(|held| held.text),
                    renamed_from: None,
                    deleted: false,
                    vanished: false,
                    text: text.clone(),
                    at_ms,
                });
                overlay.insert(key, Some(state(change, text, hash)));
            }
            Change::Renamed => {
                let text = change.text.clone().unwrap_or_default();
                let hash = change.content_hash.clone().unwrap_or_default();
                let previous = change.previous_path.clone().unwrap_or_default();
                let previous_key = (change.vault.clone(), previous.clone());
                if let Some(index) = slot_at.remove(&previous_key) {
                    let slot = &mut slots[index];
                    slot.path.clone_from(&change.path);
                    if slot.base.is_some() {
                        slot.renamed_from.get_or_insert(previous.clone());
                    }
                    slot.text.clone_from(&text);
                    slot_at.insert(key.clone(), index);
                    overlay.insert(previous_key, None);
                    overlay.insert(key, Some(state(change, text, hash)));
                    update.renames.push(StoredRename {
                        from: previous,
                        to: change.path.clone(),
                        at_ms,
                    });
                    continue;
                }
                if let Some(held) = load(journal, &overlay, &mut read, &previous_key)? {
                    if at_ms < held.changed_at_ms {
                        continue;
                    }
                    slot_at.insert(key.clone(), slots.len());
                    slots.push(Slot {
                        vault: change.vault.clone(),
                        start_path: previous.clone(),
                        path: change.path.clone(),
                        base: Some(held.text),
                        renamed_from: Some(previous.clone()),
                        deleted: false,
                        vanished: false,
                        text: text.clone(),
                        at_ms,
                    });
                    overlay.insert(previous_key, None);
                    overlay.insert(key, Some(state(change, text, hash)));
                    update.renames.push(StoredRename {
                        from: previous,
                        to: change.path.clone(),
                        at_ms,
                    });
                    continue;
                }
                // The journal doesn't have the old path. A second device
                // reporting a rename it already has changes nothing.
                let current = load(journal, &overlay, &mut read, &key)?;
                match &current {
                    Some(held) if held.content_hash == hash => continue,
                    Some(held) if at_ms < held.changed_at_ms => continue,
                    _ => {}
                }
                slot_at.insert(key.clone(), slots.len());
                slots.push(Slot {
                    vault: change.vault.clone(),
                    start_path: change.path.clone(),
                    path: change.path.clone(),
                    base: current.map(|held| held.text),
                    renamed_from: None,
                    deleted: false,
                    vanished: false,
                    text: text.clone(),
                    at_ms,
                });
                overlay.insert(key, Some(state(change, text, hash)));
            }
        }
    }

    let known = known_renames(journal, &vaults)?;
    let mut entries: Vec<(i64, NoteEntry)> = Vec::new();
    for slot in &slots {
        let renames = renames_near(
            known
                .get(&slot.vault)
                .map(Vec::as_slice)
                .unwrap_or_default(),
            slot.at_ms,
        );
        if let Some(entry) = entry_for(slot, &renames) {
            entries.push((slot.at_ms, entry));
        }
    }
    entries.sort_by(|left, right| {
        left.0
            .cmp(&right.0)
            .then_with(|| left.1.path.cmp(&right.1.path))
    });
    let entries = entries
        .into_iter()
        .map(|(_, entry)| entry)
        .collect::<Vec<_>>();
    let transcript = (!entries.is_empty()).then(|| render_transcript(&entries).into_bytes());
    Ok(Prepared {
        transcript,
        notes: overlay,
        vaults,
    })
}

fn state(change: &NoteChange, text: String, content_hash: String) -> NoteState {
    NoteState {
        path: change.path.clone(),
        text,
        content_hash,
        changed_at_ms: change.changed_at_ms(),
    }
}

/// A note as this upload has left it so far, or as the journal holds it.
fn load(
    journal: &Path,
    overlay: &HashMap<NoteKey, Option<NoteState>>,
    read: &mut HashMap<NoteKey, Option<NoteState>>,
    key: &NoteKey,
) -> Result<Option<NoteState>, String> {
    if let Some(held) = overlay.get(key) {
        return Ok(held.clone());
    }
    if let Some(held) = read.get(key) {
        return Ok(held.clone());
    }
    let held = read_note(journal, &key.0, &key.1)?;
    read.insert(key.clone(), held.clone());
    Ok(held)
}

fn known_renames(
    journal: &Path,
    vaults: &BTreeMap<String, VaultUpdate>,
) -> Result<BTreeMap<String, Vec<StoredRename>>, String> {
    let mut known = BTreeMap::new();
    for (vault, update) in vaults {
        let held = read_vault(journal, vault)?;
        let renames = held
            .renames
            .into_iter()
            .chain(update.renames.iter().cloned())
            .collect();
        known.insert(vault.clone(), renames);
    }
    Ok(known)
}

/// The renames close enough to `at_ms` for an edit then to be Obsidian keeping
/// links working.
fn renames_near(renames: &[StoredRename], at_ms: i64) -> Vec<KnownRename> {
    renames
        .iter()
        .filter(|rename| (at_ms - rename.at_ms).abs() <= RENAME_MEMORY_MS)
        .map(|rename| KnownRename {
            from: rename.from.clone(),
            to: rename.to.clone(),
        })
        .collect()
}

fn entry_for(slot: &Slot, renames: &[KnownRename]) -> Option<NoteEntry> {
    if slot.deleted {
        // Seen and gone within one upload leaves nothing to say.
        if slot.vanished {
            return None;
        }
        return Some(NoteEntry {
            path: slot.start_path.clone(),
            status: Some("Deleted".to_owned()),
            body: String::new(),
        });
    }
    let Some(base) = &slot.base else {
        return (!slot.text.trim().is_empty()).then(|| NoteEntry {
            path: slot.path.clone(),
            status: None,
            body: slot.text.clone(),
        });
    };
    if let Some(from) = &slot.renamed_from {
        return Some(NoteEntry {
            path: slot.path.clone(),
            status: Some(format!("Renamed from {from}")),
            body: render_edit(base, &slot.text).unwrap_or_default(),
        });
    }
    if only_link_rewrites(base, &slot.text, renames) {
        return None;
    }
    render_edit(base, &slot.text).map(|body| NoteEntry {
        path: slot.path.clone(),
        status: Some("Edited".to_owned()),
        body,
    })
}

impl Prepared {
    /// Save what this upload left the journal knowing.
    pub(crate) fn commit(self, journal: &Path, cid: &str) -> Result<(), String> {
        for ((vault, path), state) in &self.notes {
            let file = note_file(journal, vault, path);
            match state {
                Some(state) => write_json(&file, state)?,
                None => match std::fs::remove_file(&file) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(format!("{}: {error}", file.display())),
                },
            }
        }
        for (vault, update) in self.vaults {
            let mut held = read_vault(journal, &vault)?;
            held.devices.insert(cid.to_owned(), update.last_synced_at);
            held.renames.extend(update.renames);
            if let Some(latest) = held.renames.iter().map(|rename| rename.at_ms).max() {
                held.renames
                    .retain(|rename| latest - rename.at_ms <= RENAME_MEMORY_MS);
            }
            write_json(&vault_dir(journal, &vault).join("vault.json"), &held)?;
        }
        Ok(())
    }
}

/// The vaults this journal has from devices other than the caller.
pub(crate) async fn obsidian_vaults(
    Extension(basis): Extension<AccessBasis>,
    State(state): State<IngestState>,
    headers: HeaderMap,
) -> Response {
    let cid = match validate_access(&basis) {
        Ok(cid) => cid,
        Err((code, status, detail)) => return refusal(code, status, detail),
    };
    if let Err((code, status, detail)) = validate_protocol(&headers) {
        return refusal(code, status, detail);
    }
    spawn_blocking_response(
        OwnerReadRole::DeviceIngestSegments,
        move || match listed_vaults(&state.journal_root, &cid) {
            Ok(vaults) => Json(json!({ "vaults": vaults })).into_response(),
            Err(error) => {
                log::warn!("obsidian vault list unreadable: {error}");
                refusal(
                    crate::model::ReasonCode::JournalReadFailed,
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "cannot read the vault list",
                )
            }
        },
    )
    .await
}

fn listed_vaults(journal: &Path, cid: &str) -> Result<Vec<Value>, String> {
    let root = journal.join(STATE_DIR);
    let entries = match std::fs::read_dir(&root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.to_string()),
    };
    let mut ledger = AuthorizationLedger::new(journal);
    let mut vaults = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| error.to_string())?;
        let Some(held) = read_json::<VaultState>(&entry.path().join("vault.json"))? else {
            continue;
        };
        for (device, last_synced_at) in &held.devices {
            if device != cid {
                vaults.push(json!({
                    "vault": held.vault,
                    "device": device_label(&mut ledger, device),
                    "last_synced_at": last_synced_at,
                }));
            }
        }
    }
    vaults.sort_by_key(|vault| vault["vault"].as_str().map(str::to_owned));
    Ok(vaults)
}

fn device_label(ledger: &mut AuthorizationLedger, cid: &str) -> Option<String> {
    let entry = ledger.get(cid)?;
    let label = entry.display_label();
    (!label.is_empty()).then(|| label.to_owned())
}

#[cfg(test)]
#[path = "obsidian_tests.rs"]
mod tests;
