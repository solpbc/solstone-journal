// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use solstone_core_journal_io::{JsonWriteOptions, LockOptions, hold_lock, write_json};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::client_description::{
    ClientDescriptionResponse, JournalIdentityMeta, PutSelfDescriptionRequest,
    StoredClientDescription, current_display_label, sanitize_reported, sanitize_string,
};
use crate::ledger::{AuthorizedClientsRead, read_authorized_clients};

#[derive(Debug)]
pub enum DescriptionStoreError {
    Lock(solstone_core_journal_io::LockError),
    Unreadable(PathBuf),
    Write(solstone_core_journal_io::AtomicWriteError),
}

#[derive(Debug)]
pub enum DescriptionMutationError {
    NotAuthorized,
    NotFound,
    RevisionConflict,
    Invalid(&'static str),
    UnreadableLedger(PathBuf),
    UnreadableStore(PathBuf),
    Lock(solstone_core_journal_io::LockError),
    Write(solstone_core_journal_io::AtomicWriteError),
}

impl From<DescriptionStoreError> for DescriptionMutationError {
    fn from(error: DescriptionStoreError) -> Self {
        match error {
            DescriptionStoreError::Lock(e) => DescriptionMutationError::Lock(e),
            DescriptionStoreError::Unreadable(p) => DescriptionMutationError::UnreadableStore(p),
            DescriptionStoreError::Write(e) => DescriptionMutationError::Write(e),
        }
    }
}

pub fn client_descriptions_path(journal_root: &Path) -> PathBuf {
    journal_root.join("link").join("client-descriptions.json")
}

pub fn read_descriptions(
    journal_root: &Path,
) -> Result<BTreeMap<String, StoredClientDescription>, DescriptionStoreError> {
    let path = client_descriptions_path(journal_root);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(_) => return Err(DescriptionStoreError::Unreadable(path)),
    };
    serde_json::from_slice(&bytes).map_err(|_| DescriptionStoreError::Unreadable(path))
}

fn lock_descriptions(
    path: &Path,
) -> Result<solstone_core_journal_io::FileLock, DescriptionStoreError> {
    hold_lock(
        path,
        LockOptions {
            mode: Some(0o600),
            ..LockOptions::default()
        },
    )
    .map_err(DescriptionStoreError::Lock)
}

fn lock_authorization(
    path: &Path,
) -> Result<solstone_core_journal_io::FileLock, DescriptionMutationError> {
    hold_lock(
        path,
        LockOptions {
            mode: Some(0o600),
            ..LockOptions::default()
        },
    )
    .map_err(DescriptionMutationError::Lock)
}

/// Retrieve the client description response for a given CID without holding persistent locks.
pub fn get_description_response(
    journal_root: &Path,
    cid: &str,
    is_owner: bool,
    journal_meta: JournalIdentityMeta,
) -> Result<ClientDescriptionResponse, DescriptionMutationError> {
    let auth_path = journal_root.join("link").join("authorized_clients.json");
    let entry = match read_authorized_clients(&auth_path) {
        AuthorizedClientsRead::Present(clients) => {
            clients.into_iter().find(|e| e.fingerprint == cid)
        }
        AuthorizedClientsRead::Missing => None,
        AuthorizedClientsRead::Unreadable
        | AuthorizedClientsRead::Malformed
        | AuthorizedClientsRead::DuplicateCid => {
            return Err(DescriptionMutationError::UnreadableLedger(auth_path));
        }
    };
    let Some(entry) = entry else {
        return if is_owner {
            Err(DescriptionMutationError::NotFound)
        } else {
            Err(DescriptionMutationError::NotAuthorized)
        };
    };

    let descriptions = read_descriptions(journal_root)?;
    let stored = descriptions.get(cid);
    let display_label = current_display_label(&entry, stored);

    let (protocol_version, revision, reported, owner_label, updated_at) = match stored {
        Some(stored) => (
            stored.protocol_version,
            stored.revision,
            stored.reported.clone(),
            stored.owner_label.clone(),
            stored.updated_at.clone(),
        ),
        None => (1, 0, None, None, None),
    };

    Ok(ClientDescriptionResponse {
        protocol_version,
        revision,
        reported,
        owner_label,
        display_label,
        updated_at,
        journal: journal_meta,
    })
}

/// Update a linked device's self-reported metadata using CAS.
pub fn put_self_description(
    journal_root: &Path,
    cid: &str,
    request: PutSelfDescriptionRequest,
    now: OffsetDateTime,
    journal_meta: JournalIdentityMeta,
) -> Result<ClientDescriptionResponse, DescriptionMutationError> {
    let auth_path = journal_root.join("link").join("authorized_clients.json");
    let _auth_lock = lock_authorization(&auth_path)?;

    let entry = match read_authorized_clients(&auth_path) {
        AuthorizedClientsRead::Present(clients) => {
            clients.into_iter().find(|e| e.fingerprint == cid)
        }
        AuthorizedClientsRead::Missing => None,
        AuthorizedClientsRead::Unreadable
        | AuthorizedClientsRead::Malformed
        | AuthorizedClientsRead::DuplicateCid => {
            return Err(DescriptionMutationError::UnreadableLedger(auth_path));
        }
    };
    let Some(entry) = entry else {
        return Err(DescriptionMutationError::NotAuthorized);
    };

    if request.protocol_version != 1 {
        return Err(DescriptionMutationError::Invalid(
            "protocol_version must be 1",
        ));
    }
    let sanitized_reported = match request.reported {
        Some(r) => Some(sanitize_reported(r).map_err(DescriptionMutationError::Invalid)?),
        None => None,
    };

    let desc_path = client_descriptions_path(journal_root);
    let _desc_lock = lock_descriptions(&desc_path)?;
    let mut descriptions = read_descriptions(journal_root)?;

    let existing = descriptions.get(cid).cloned();
    let current_revision = existing.as_ref().map(|s| s.revision).unwrap_or(0);
    if current_revision != request.expected_revision {
        return Err(DescriptionMutationError::RevisionConflict);
    }

    let now_str = now
        .format(&Rfc3339)
        .map_err(|_| DescriptionMutationError::Invalid("failed to format timestamp"))?;

    let final_stored = match existing {
        None => {
            if sanitized_reported.is_none() {
                // Identical to absent: no-op, do not write file
                StoredClientDescription::initial()
            } else {
                let new_desc = StoredClientDescription {
                    protocol_version: 1,
                    revision: 1,
                    reported: sanitized_reported,
                    owner_label: None,
                    updated_at: Some(now_str),
                };
                descriptions.insert(cid.to_owned(), new_desc.clone());
                write_json(
                    &desc_path,
                    &descriptions,
                    JsonWriteOptions {
                        mode: Some(0o600),
                        ..JsonWriteOptions::default()
                    },
                )
                .map_err(DescriptionMutationError::Write)?;
                new_desc
            }
        }
        Some(cur) => {
            if cur.reported == sanitized_reported {
                // No change in reported metadata: no-op
                cur
            } else {
                let new_desc = StoredClientDescription {
                    protocol_version: 1,
                    revision: cur.revision + 1,
                    reported: sanitized_reported,
                    owner_label: cur.owner_label,
                    updated_at: Some(now_str),
                };
                descriptions.insert(cid.to_owned(), new_desc.clone());
                write_json(
                    &desc_path,
                    &descriptions,
                    JsonWriteOptions {
                        mode: Some(0o600),
                        ..JsonWriteOptions::default()
                    },
                )
                .map_err(DescriptionMutationError::Write)?;
                new_desc
            }
        }
    };

    let display_label = current_display_label(&entry, Some(&final_stored));

    Ok(ClientDescriptionResponse {
        protocol_version: final_stored.protocol_version,
        revision: final_stored.revision,
        reported: final_stored.reported,
        owner_label: final_stored.owner_label,
        display_label,
        updated_at: final_stored.updated_at,
        journal: journal_meta,
    })
}

/// Update or clear an owner label override on a linked device.
pub fn patch_owner_label(
    journal_root: &Path,
    cid: &str,
    new_label: Option<String>,
    now: OffsetDateTime,
    journal_meta: JournalIdentityMeta,
) -> Result<ClientDescriptionResponse, DescriptionMutationError> {
    let auth_path = journal_root.join("link").join("authorized_clients.json");
    let _auth_lock = lock_authorization(&auth_path)?;

    let entry = match read_authorized_clients(&auth_path) {
        AuthorizedClientsRead::Present(clients) => {
            clients.into_iter().find(|e| e.fingerprint == cid)
        }
        AuthorizedClientsRead::Missing => None,
        AuthorizedClientsRead::Unreadable
        | AuthorizedClientsRead::Malformed
        | AuthorizedClientsRead::DuplicateCid => {
            return Err(DescriptionMutationError::UnreadableLedger(auth_path));
        }
    };
    let Some(entry) = entry else {
        return Err(DescriptionMutationError::NotFound);
    };

    let sanitized_label =
        sanitize_string(new_label, 80).map_err(DescriptionMutationError::Invalid)?;

    let desc_path = client_descriptions_path(journal_root);
    let _desc_lock = lock_descriptions(&desc_path)?;
    let mut descriptions = read_descriptions(journal_root)?;

    let existing = descriptions.get(cid).cloned();
    let now_str = now
        .format(&Rfc3339)
        .map_err(|_| DescriptionMutationError::Invalid("failed to format timestamp"))?;

    let final_stored = match existing {
        None => {
            if sanitized_label.is_none() {
                // Identical to absent: no-op, do not write file
                StoredClientDescription::initial()
            } else {
                let new_desc = StoredClientDescription {
                    protocol_version: 1,
                    revision: 1,
                    reported: None,
                    owner_label: sanitized_label,
                    updated_at: Some(now_str),
                };
                descriptions.insert(cid.to_owned(), new_desc.clone());
                write_json(
                    &desc_path,
                    &descriptions,
                    JsonWriteOptions {
                        mode: Some(0o600),
                        ..JsonWriteOptions::default()
                    },
                )
                .map_err(DescriptionMutationError::Write)?;
                new_desc
            }
        }
        Some(cur) => {
            if cur.owner_label == sanitized_label {
                // No change in owner override: no-op
                cur
            } else {
                let new_desc = StoredClientDescription {
                    protocol_version: 1,
                    revision: cur.revision + 1,
                    reported: cur.reported,
                    owner_label: sanitized_label,
                    updated_at: Some(now_str),
                };
                descriptions.insert(cid.to_owned(), new_desc.clone());
                write_json(
                    &desc_path,
                    &descriptions,
                    JsonWriteOptions {
                        mode: Some(0o600),
                        ..JsonWriteOptions::default()
                    },
                )
                .map_err(DescriptionMutationError::Write)?;
                new_desc
            }
        }
    };

    let display_label = current_display_label(&entry, Some(&final_stored));

    Ok(ClientDescriptionResponse {
        protocol_version: final_stored.protocol_version,
        revision: final_stored.revision,
        reported: final_stored.reported,
        owner_label: final_stored.owner_label,
        display_label,
        updated_at: final_stored.updated_at,
        journal: journal_meta,
    })
}

/// Remove a client description entry under an already-held authorization lock.
pub fn remove_description_entry(
    journal_root: &Path,
    cid: &str,
) -> Result<(), DescriptionStoreError> {
    let desc_path = client_descriptions_path(journal_root);
    if !desc_path.exists() {
        return Ok(());
    }
    let _desc_lock = lock_descriptions(&desc_path)?;
    let mut descriptions = match read_descriptions(journal_root) {
        Ok(descriptions) => descriptions,
        Err(DescriptionStoreError::Unreadable(_)) => return Ok(()),
        Err(err) => return Err(err),
    };
    if descriptions.remove(cid).is_some() {
        write_json(
            &desc_path,
            &descriptions,
            JsonWriteOptions {
                mode: Some(0o600),
                ..JsonWriteOptions::default()
            },
        )
        .map_err(DescriptionStoreError::Write)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client_description::ReportedDescription;
    use crate::ledger::{AuthorizationLedger, ClientEntry, ClientRole};
    use std::sync::atomic::{AtomicU64, Ordering};

    struct TempDir {
        path: PathBuf,
    }
    impl TempDir {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "sol-link-desc-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = fs::create_dir_all(&path);
            Self { path }
        }
        fn path(&self) -> &Path {
            &self.path
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn test_meta() -> JournalIdentityMeta {
        JournalIdentityMeta {
            name: Some("test-journal".into()),
            version: "1.0.0".into(),
        }
    }

    fn seed_client(journal_root: &Path, cid: &str, device_label: &str) {
        let mut ledger = AuthorizationLedger::new(journal_root);
        let entry = ClientEntry::new(
            cid,
            device_label,
            "2026-01-01T00:00:00Z",
            "inst-1",
            ClientRole::Roleless,
        );
        ledger.add(entry).unwrap();
    }

    #[test]
    fn missing_file_returns_absent_without_creating() {
        let dir = TempDir::new();
        let root = dir.path();
        let cid = "sha256:0000000000000000000000000000000000000000000000000000000000000001";
        seed_client(root, cid, "mac");

        let resp = get_description_response(root, cid, false, test_meta()).unwrap();
        assert_eq!(resp.revision, 0);
        assert_eq!(resp.reported, None);
        assert_eq!(resp.owner_label, None);
        assert_eq!(resp.display_label, "mac");
        assert!(!client_descriptions_path(root).exists());
    }

    #[test]
    fn put_self_cas_and_noop() {
        let dir = TempDir::new();
        let root = dir.path();
        let cid = "sha256:0000000000000000000000000000000000000000000000000000000000000001";
        seed_client(root, cid, "mac");

        let now = OffsetDateTime::now_utc();

        // 1. Conflict on wrong revision
        let err = put_self_description(
            root,
            cid,
            PutSelfDescriptionRequest {
                protocol_version: 1,
                expected_revision: 5,
                reported: Some(ReportedDescription {
                    name: Some("Living Room Mac".into()),
                    platform: Some("macos".into()),
                    device_type: None,
                    app_id: Some("solstone".into()),
                    app_version: Some("1.0.0".into()),
                }),
            },
            now,
            test_meta(),
        )
        .unwrap_err();
        assert!(matches!(err, DescriptionMutationError::RevisionConflict));

        // 2. Success at revision 0 -> becomes revision 1
        let resp = put_self_description(
            root,
            cid,
            PutSelfDescriptionRequest {
                protocol_version: 1,
                expected_revision: 0,
                reported: Some(ReportedDescription {
                    name: Some("Living Room Mac".into()),
                    platform: Some("macos".into()),
                    device_type: None,
                    app_id: Some("solstone".into()),
                    app_version: Some("1.0.0".into()),
                }),
            },
            now,
            test_meta(),
        )
        .unwrap();
        assert_eq!(resp.revision, 1);
        assert_eq!(resp.display_label, "Living Room Mac");
        assert!(client_descriptions_path(root).exists());

        // 3. Identical PUT is a no-op
        let resp_noop = put_self_description(
            root,
            cid,
            PutSelfDescriptionRequest {
                protocol_version: 1,
                expected_revision: 1,
                reported: Some(ReportedDescription {
                    name: Some("Living Room Mac".into()),
                    platform: Some("macos".into()),
                    device_type: None,
                    app_id: Some("solstone".into()),
                    app_version: Some("1.0.0".into()),
                }),
            },
            now,
            test_meta(),
        )
        .unwrap();
        assert_eq!(resp_noop.revision, 1);
    }

    #[test]
    fn patch_owner_label_and_precedence() {
        let dir = TempDir::new();
        let root = dir.path();
        let cid = "sha256:0000000000000000000000000000000000000000000000000000000000000001";
        seed_client(root, cid, "mac");
        let now = OffsetDateTime::now_utc();

        // Put reported name
        put_self_description(
            root,
            cid,
            PutSelfDescriptionRequest {
                protocol_version: 1,
                expected_revision: 0,
                reported: Some(ReportedDescription {
                    name: Some("Reported Name".into()),
                    platform: None,
                    device_type: None,
                    app_id: None,
                    app_version: None,
                }),
            },
            now,
            test_meta(),
        )
        .unwrap();

        // Patch owner label override
        let resp =
            patch_owner_label(root, cid, Some("Owner Override".into()), now, test_meta()).unwrap();
        assert_eq!(resp.revision, 2);
        assert_eq!(resp.owner_label.as_deref(), Some("Owner Override"));
        assert_eq!(resp.display_label, "Owner Override");

        // Clear owner label override -> reveals reported name
        let resp_clear = patch_owner_label(root, cid, None, now, test_meta()).unwrap();
        assert_eq!(resp_clear.revision, 3);
        assert_eq!(resp_clear.owner_label, None);
        assert_eq!(resp_clear.display_label, "Reported Name");
    }

    #[test]
    fn remove_deletes_description() {
        let dir = TempDir::new();
        let root = dir.path();
        let cid = "sha256:0000000000000000000000000000000000000000000000000000000000000001";
        seed_client(root, cid, "mac");
        let now = OffsetDateTime::now_utc();

        patch_owner_label(root, cid, Some("Custom".into()), now, test_meta()).unwrap();

        let mut ledger = AuthorizationLedger::new(root);
        ledger.remove(cid).unwrap();

        let descriptions = read_descriptions(root).unwrap();
        assert!(!descriptions.contains_key(cid));
    }
}
