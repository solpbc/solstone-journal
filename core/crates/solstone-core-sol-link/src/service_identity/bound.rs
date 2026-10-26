// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Identity preparation through one retained journal and its existing sidecar.

use std::error::Error;
use std::ffi::OsStr;
use std::fmt;
use std::io;
use std::path::Path;
use std::sync::Arc;

use serde_json::{Value, json};
use solstone_core_journal_io::{
    ExclusivePublication, JournalRoot, LockOptions, hold_exclusive_parent_lock_bound_checked,
    media::MediaDirectory, write_bytes_exclusive_bound_detailed_checked,
};

use super::{ServiceIdentity, now_ms};
use crate::ca::{generate_ca, jid_from_spki, load_ca};
use crate::committed::CommittedIdentity;

type Failure = Box<dyn Error + Send + Sync>;
#[cfg(unix)]
type Parent = solstone_core_journal_io::FlatDirectory;
#[cfg(windows)]
type Parent = solstone_core_journal_io::WindowsFlatDirectory;

/// Actual file effects, including publications preceding a later refusal.
pub struct IdentityPublication {
    pub leaf: &'static str,
    pub native: IdentityNativePublication,
    pub confirmation_refusal: Option<Failure>,
}

pub enum IdentityNativePublication {
    Created(ExclusivePublication),
    #[cfg(unix)]
    Replaced(solstone_core_journal_io::BoundAtomicOutcome),
    #[cfg(windows)]
    Replaced(solstone_core_journal_io::DetailedAtomicOutcome),
}

impl IdentityPublication {
    pub fn confirmed(&self) -> bool {
        let confirmed = match &self.native {
            IdentityNativePublication::Created(p) => {
                #[cfg(unix)]
                {
                    p.is_fully_confirmed()
                }
                #[cfg(windows)]
                {
                    // Windows has no supported parent metadata flush. Retain
                    // that unavailable fact; an actual error still refuses.
                    use solstone_core_journal_io::{
                        FinalNameConfirmation, MetadataDurability, StageCleanup,
                    };
                    matches!(p.final_name, FinalNameConfirmation::Confirmed { .. })
                        && matches!(p.durability, MetadataDurability::Unproven { source: None })
                        && matches!(p.cleanup, StageCleanup::Removed)
                }
            }
            #[cfg(unix)]
            IdentityNativePublication::Replaced(p) => matches!(
                p,
                solstone_core_journal_io::BoundAtomicOutcome::Published { .. }
            ),
            #[cfg(windows)]
            IdentityNativePublication::Replaced(p) => matches!(
                p,
                solstone_core_journal_io::DetailedAtomicOutcome::Published
            ),
        };
        confirmed && self.confirmation_refusal.is_none()
    }
}

impl fmt::Debug for IdentityPublication {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Native observations contain private bytes. Never forward Debug.
        f.debug_struct("IdentityPublication")
            .field("leaf", &self.leaf)
            .field("confirmed", &self.confirmed())
            .field("confirmation_refused", &self.confirmation_refusal.is_some())
            .finish_non_exhaustive()
    }
}

pub struct IdentityPreparation {
    pub identity: CommittedIdentity,
    pub service: ServiceIdentity,
    pub publications: Vec<IdentityPublication>,
    /// False on Windows, whose native directory durability is unavailable.
    pub directories_synced: bool,
}

pub struct IdentityPreparationRefusal {
    pub publications: Vec<IdentityPublication>,
    pub cause: Failure,
}

impl fmt::Debug for IdentityPreparationRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IdentityPreparationRefusal")
            .field("publications", &self.publications)
            .finish_non_exhaustive()
    }
}
impl fmt::Display for IdentityPreparationRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("service identity preparation was refused")
    }
}
impl Error for IdentityPreparationRefusal {}

fn refused(detail: &'static str) -> Failure {
    Box::new(io::Error::other(detail))
}

fn logical(checkpoint: &dyn Fn() -> Result<(), String>) -> Result<(), Failure> {
    checkpoint().map_err(|_| refused("service identity preparation interrupted"))
}

fn read(directory: &MediaDirectory, name: &str) -> Result<Option<Vec<u8>>, Failure> {
    directory
        .read_file_bounded(OsStr::new(name), 1024 * 1024)
        .map(|value| value.map(|value| value.bytes))
        .map_err(Into::into)
}

fn value(bytes: &[u8]) -> Result<Value, Failure> {
    let value: Value =
        serde_json::from_slice(bytes).map_err(|_| refused("invalid identity state JSON"))?;
    if value.as_object().is_none_or(|v| {
        v.get("instance_id")
            .and_then(Value::as_str)
            .is_none_or(str::is_empty)
            || v.get("home_label")
                .and_then(Value::as_str)
                .is_none_or(str::is_empty)
    }) {
        return Err(refused("invalid identity state fields"));
    }
    Ok(value)
}

fn bytes(value: &Value) -> Result<Vec<u8>, Failure> {
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    Ok(bytes)
}

/// Uses the original root for all reads, locks and writes. A caller must retain
/// its existing target-use lease and join this whole synchronous effect.
/// Fresh cert/key persistence has one logical admission cut: later Stop/off
/// drains it, while physical binding and native errors remain effective.
pub fn prepare_service_identity_at_root_checked(
    root: &Arc<JournalRoot>,
    checkpoint: &dyn Fn() -> Result<(), String>,
) -> Result<IdentityPreparation, Box<IdentityPreparationRefusal>> {
    let mut publications = Vec::new();
    let result = (|| -> Result<_, Failure> {
        logical(checkpoint)?;
        root.revalidate_canonical_binding()?;
        #[cfg(unix)]
        let link = solstone_core_journal_io::create_or_open_flat_directory_bound(
            &**root,
            OsStr::new("link"),
            0o700,
            root.canonical_path(),
        )?;
        #[cfg(windows)]
        let link = solstone_core_journal_io::create_or_open_windows_flat_directory_bound(
            &**root,
            OsStr::new("link"),
            root.canonical_path(),
        )?;
        let link_dir = MediaDirectory::open(root.clone(), Path::new("link"))?;
        same(&link, &link_dir)?;
        let check_link = || -> Result<(), String> {
            logical(checkpoint).map_err(|e| e.to_string())?;
            link_dir.revalidate().map_err(|e| e.to_string())
        };
        let options = LockOptions::default();
        let lock = hold_exclusive_parent_lock_bound_checked(
            &link,
            OsStr::new("identity"),
            link_dir.diagnostic_path(),
            options,
            &check_link,
        )?;
        check_link().map_err(io::Error::other)?;
        lock.revalidate_bound(&link, OsStr::new("identity.lock"))?;
        let primary = read(&link_dir, "state.json")?;
        // Validate existing state before generating or writing any CA material.
        let primary_value = primary.as_deref().map(value).transpose()?;
        let old_ca_dir = link_dir.open_child(OsStr::new("ca"))?;
        let cert = old_ca_dir
            .as_ref()
            .map(|d| read(d, "cert.pem"))
            .transpose()?
            .flatten();
        let key = old_ca_dir
            .as_ref()
            .map(|d| read(d, "private.pem"))
            .transpose()?
            .flatten();
        let fresh = match (&cert, &key) {
            (Some(_), Some(_)) => false,
            (None, None) if primary.is_none() => true,
            _ => return Err(refused("committed CA is incomplete")),
        };
        #[cfg(unix)]
        let ca_parent = solstone_core_journal_io::create_or_open_flat_directory_bound(
            &link,
            OsStr::new("ca"),
            0o700,
            &root.canonical_path().join("link"),
        )?;
        #[cfg(windows)]
        let ca_parent = solstone_core_journal_io::create_or_open_windows_flat_directory_bound(
            &link,
            OsStr::new("ca"),
            &root.canonical_path().join("link"),
        )?;
        let ca_dir = MediaDirectory::open(root.clone(), Path::new("link/ca"))?;
        same(&ca_parent, &ca_dir)?;
        if old_ca_dir
            .as_ref()
            .is_some_and(|old| old.object_identity() != ca_dir.object_identity())
        {
            return Err(refused("identity CA parent changed"));
        }
        let physical = || -> Result<(), String> {
            root.revalidate_canonical_binding()
                .map_err(|e| e.to_string())?;
            link_dir.revalidate().map_err(|e| e.to_string())?;
            ca_dir.revalidate().map_err(|e| e.to_string())?;
            lock.revalidate_bound(&link, OsStr::new("identity.lock"))
                .map_err(|e| e.to_string())
        };
        physical().map_err(io::Error::other)?;
        logical(checkpoint)?;
        let (certificate, private) = if fresh {
            // Do not generate over inherited fallback state, even if its CA
            // leaves are missing. There is no rotation or cleanup authority.
            if read(&ca_dir, "state.json")?.is_some() {
                return Err(refused("state exists without committed CA"));
            }
            let ca = generate_ca()?;
            (
                ca.certificate_pem().as_bytes().to_vec(),
                ca.private_key_pem(),
            )
        } else {
            (
                cert.expect("complete pair"),
                String::from_utf8(key.expect("complete pair"))
                    .map_err(|_| refused("invalid private key encoding"))?,
            )
        };
        let certificate_text = std::str::from_utf8(&certificate)
            .map_err(|_| refused("invalid certificate encoding"))?;
        let ca = load_ca(certificate_text, &private)?;
        let instance_id = jid_from_spki(ca.spki_der())?;
        let fallback = if primary_value.is_none() {
            read(&ca_dir, "state.json")?
        } else {
            None
        };
        let (mut state, state_bytes, state_path, normalize) = match primary_value {
            Some(state) => {
                let normalize = state["instance_id"].as_str() != Some(&instance_id);
                (
                    state,
                    primary,
                    root.canonical_path().join("link/state.json"),
                    normalize,
                )
            }
            None => match fallback {
                Some(state_bytes) => {
                    let state = value(&state_bytes)?;
                    if state["instance_id"].as_str() != Some(&instance_id) {
                        return Err(refused("native state does not match committed CA"));
                    }
                    (
                        state,
                        Some(state_bytes),
                        root.canonical_path().join("link/ca/state.json"),
                        false,
                    )
                }
                None => {
                    let label = crate::mark::mark_words_from_jid(&instance_id)
                        .map_err(|_| refused("could not derive identity label"))?;
                    (
                        json!({"instance_id":instance_id,"home_label":label,"locked_at":now_ms()}),
                        None,
                        root.canonical_path().join("link/state.json"),
                        false,
                    )
                }
            },
        };
        if normalize {
            state["instance_id"] = Value::String(instance_id);
        }
        let needs_state_publication = state_bytes.is_none() || normalize;
        let desired = if needs_state_publication {
            bytes(&state)?
        } else {
            state_bytes.expect("existing state")
        };
        physical().map_err(io::Error::other)?;
        // Final logical cut for a new CA. Later cancellation does not strand
        // cert without key: physical checks continue, and the owner joins all
        // persistence before suppressing portal work.
        logical(checkpoint)?;
        if fresh {
            create(
                &ca_parent,
                &ca_dir,
                "cert.pem",
                &certificate,
                0o644,
                &physical,
                &mut publications,
            )?;
            create(
                &ca_parent,
                &ca_dir,
                "private.pem",
                private.as_bytes(),
                0o600,
                &physical,
                &mut publications,
            )?;
        }
        if needs_state_publication {
            let check_state = || {
                physical()?;
                if fresh { Ok(()) } else { checkpoint() }
            };
            if normalize {
                #[cfg(unix)]
                let native = solstone_core_journal_io::atomic_replace_bound_checked(
                    &link,
                    OsStr::new("state.json"),
                    &desired,
                    0o600,
                    &check_state,
                )?;
                #[cfg(windows)]
                let native = solstone_core_journal_io::atomic_replace_detailed_bound_checked(
                    &link,
                    &lock,
                    OsStr::new("state.json"),
                    &desired,
                    0o600,
                    &check_state,
                )?;
                let refusal = confirm(&link_dir, "state.json", &desired, &physical).err();
                publications.push(IdentityPublication {
                    leaf: "state.json",
                    native: IdentityNativePublication::Replaced(native),
                    confirmation_refusal: refusal,
                });
                if !publications.last().expect("just pushed").confirmed() {
                    return Err(refused("identity state publication unconfirmed"));
                }
            } else {
                create(
                    &link,
                    &link_dir,
                    "state.json",
                    &desired,
                    0o600,
                    &check_state,
                    &mut publications,
                )?;
            }
        }
        // New parent entries must reach stable metadata too. These are real
        // retained handle syncs, separate from the file publication receipts.
        #[cfg(unix)]
        if !publications.is_empty() {
            ca_dir.sync()?;
            link_dir.sync()?;
            MediaDirectory::open(root.clone(), Path::new(""))?.sync()?;
        }
        physical().map_err(io::Error::other)?;
        confirm(&ca_dir, "cert.pem", &certificate, &physical)?;
        confirm(&ca_dir, "private.pem", private.as_bytes(), &physical)?;
        let state_dir = if state_path.ends_with("link/state.json") {
            &link_dir
        } else {
            &ca_dir
        };
        confirm(state_dir, "state.json", &desired, &physical)?;
        logical(checkpoint)?;
        let text = std::str::from_utf8(&desired)
            .map_err(|_| refused("invalid identity state encoding"))?;
        let identity = crate::committed::from_explicit_material(
            root.canonical_path().join("link/ca/cert.pem"),
            certificate,
            private,
            state_path,
            text,
        )?;
        let service = ServiceIdentity {
            instance_id: identity.instance_id().to_owned(),
            home_label: identity.home_label().to_owned(),
            locked_at: state["locked_at"].as_i64(),
        };
        Ok((identity, service))
    })();
    match result {
        Ok((identity, service)) => {
            let directories_synced = cfg!(unix) && !publications.is_empty();
            Ok(IdentityPreparation {
                identity,
                service,
                publications,
                directories_synced,
            })
        }
        Err(cause) => Err(Box::new(IdentityPreparationRefusal {
            publications,
            cause,
        })),
    }
}

fn same(parent: &Parent, directory: &MediaDirectory) -> Result<(), Failure> {
    #[cfg(unix)]
    let identity = parent.identity();
    #[cfg(windows)]
    let identity = parent.object_identity();
    if identity != directory.object_identity() {
        return Err(refused("identity parent changed"));
    }
    directory.revalidate()?;
    Ok(())
}

fn confirm(
    directory: &MediaDirectory,
    leaf: &str,
    bytes: &[u8],
    physical: &dyn Fn() -> Result<(), String>,
) -> Result<(), Failure> {
    physical().map_err(io::Error::other)?;
    if read(directory, leaf)?.as_deref() != Some(bytes) {
        return Err(refused("identity publication observation changed"));
    }
    physical().map_err(io::Error::other)?;
    Ok(())
}

fn create(
    parent: &Parent,
    directory: &MediaDirectory,
    leaf: &'static str,
    bytes: &[u8],
    mode: u32,
    physical: &dyn Fn() -> Result<(), String>,
    publications: &mut Vec<IdentityPublication>,
) -> Result<(), Failure> {
    let native = write_bytes_exclusive_bound_detailed_checked(
        parent,
        OsStr::new(leaf),
        bytes,
        mode,
        physical,
    )?;
    let confirmation_refusal = confirm(directory, leaf, bytes, physical).err();
    publications.push(IdentityPublication {
        leaf,
        native: IdentityNativePublication::Created(native),
        confirmation_refusal,
    });
    if !publications.last().expect("just pushed").confirmed() {
        return Err(refused("identity publication unconfirmed"));
    }
    Ok(())
}

#[cfg(all(test, unix, feature = "test-hooks"))]
mod tests {
    use super::*;
    use solstone_core_journal_io::{
        BoundPublicationPrimitive, run_with_bound_publication_barrier,
        run_with_bound_publication_fault,
    };
    use std::fs;
    use std::sync::atomic::{AtomicBool, Ordering};

    fn root(path: &Path) -> Arc<JournalRoot> {
        Arc::new(JournalRoot::open(path).unwrap())
    }
    fn snapshot(path: &Path) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
        (
            fs::read(path.join("link/ca/cert.pem")).unwrap(),
            fs::read(path.join("link/ca/private.pem")).unwrap(),
            fs::read(path.join("link/state.json")).unwrap(),
        )
    }

    #[test]
    fn bound_identity_primary_priority_does_not_open_unselected_native_state() {
        let temp = tempfile::tempdir().unwrap();
        super::super::load_or_create_service_identity(temp.path()).unwrap();
        let before = snapshot(temp.path());
        let shadowed = temp.path().join("link/ca/state.json");
        fs::create_dir(&shadowed).unwrap();
        let prepared =
            prepare_service_identity_at_root_checked(&root(temp.path()), &|| Ok(())).unwrap();
        assert!(prepared.publications.is_empty());
        assert!(snapshot(temp.path()) == before);
        assert!(shadowed.is_dir());
    }

    #[test]
    fn bound_identity_reuses_the_actual_ordinary_owner_and_sidecar_without_migration() {
        use std::os::unix::fs::MetadataExt;
        let temp = tempfile::tempdir().unwrap();
        let legacy = super::super::load_or_create_service_identity(temp.path()).unwrap();
        let lock_path = temp.path().join("link/identity.lock");
        let lock = fs::metadata(&lock_path).unwrap();
        let before = snapshot(temp.path());
        let original = crate::committed::load_committed_identity(temp.path()).unwrap();
        let prepared =
            prepare_service_identity_at_root_checked(&root(temp.path()), &|| Ok(())).unwrap();
        assert!(prepared.publications.is_empty());
        assert_eq!(prepared.identity.instance_id(), legacy.instance_id);
        assert_eq!(
            prepared.identity.certificate_der(),
            original.certificate_der()
        );
        assert!(snapshot(temp.path()) == before);
        let after = fs::metadata(&lock_path).unwrap();
        assert_eq!((after.ino(), after.mode()), (lock.ino(), lock.mode()));

        // Normalization also uses this exact ordinary sidecar through the
        // Windows-compatible bound replacement capability, without migrating it.
        let mut state: Value = serde_json::from_slice(&before.2).unwrap();
        state["instance_id"] = json!("stale-id");
        state["keep"] = json!(true);
        fs::write(temp.path().join("link/state.json"), bytes(&state).unwrap()).unwrap();
        let repaired =
            prepare_service_identity_at_root_checked(&root(temp.path()), &|| Ok(())).unwrap();
        assert_eq!(repaired.publications.len(), 1);
        assert_eq!(
            repaired.identity.certificate_der(),
            original.certificate_der()
        );
        let after = fs::metadata(&lock_path).unwrap();
        assert_eq!((after.ino(), after.mode()), (lock.ino(), lock.mode()));
        let material = snapshot(temp.path());
        assert!(material.0 == before.0 && material.1 == before.1);
        state["instance_id"] = json!(legacy.instance_id);
        assert_eq!(serde_json::from_slice::<Value>(&material.2).unwrap(), state);
    }

    #[test]
    fn bound_identity_fresh_reuse_preserves_original_material_and_native_layout() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let root = root(temp.path());
        let first = prepare_service_identity_at_root_checked(&root, &|| Ok(())).unwrap();
        assert_eq!(first.publications.len(), 3);
        assert!(first.directories_synced);
        assert!(
            first
                .publications
                .iter()
                .all(IdentityPublication::confirmed)
        );
        let before = snapshot(temp.path());
        let der = first.identity.certificate_der().to_vec();
        assert_eq!(
            fs::metadata(temp.path().join("link/ca/private.pem"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        let second = prepare_service_identity_at_root_checked(&root, &|| Ok(())).unwrap();
        assert!(second.publications.is_empty());
        assert_eq!(second.identity.certificate_der(), der);
        assert_eq!(snapshot(temp.path()), before);
        // Native provisioned journals keep their state beside the CA. Reuse
        // that layout without manufacturing another primary state.
        fs::rename(
            temp.path().join("link/state.json"),
            temp.path().join("link/ca/state.json"),
        )
        .unwrap();
        let native = prepare_service_identity_at_root_checked(&root, &|| Ok(())).unwrap();
        assert!(native.publications.is_empty());
        assert_eq!(native.identity.certificate_der(), der);
        assert!(!temp.path().join("link/state.json").exists());
    }

    #[test]
    fn bound_identity_normalizes_only_primary_id_and_preserves_unknown_fields() {
        let temp = tempfile::tempdir().unwrap();
        let root = root(temp.path());
        let first = prepare_service_identity_at_root_checked(&root, &|| Ok(())).unwrap();
        let before = snapshot(temp.path());
        let mut state: Value = serde_json::from_slice(&before.2).unwrap();
        state["instance_id"] = json!("stale-synthetic-id");
        state["home_label"] = json!("Kept Label");
        state["locked_at"] = json!(17);
        state["future_field"] = json!({"preserve":true});
        fs::write(temp.path().join("link/state.json"), bytes(&state).unwrap()).unwrap();
        let repaired = prepare_service_identity_at_root_checked(&root, &|| Ok(())).unwrap();
        assert_eq!(repaired.publications.len(), 1);
        assert_eq!(
            repaired.identity.certificate_der(),
            first.identity.certificate_der()
        );
        let after = snapshot(temp.path());
        assert_eq!((&after.0, &after.1), (&before.0, &before.1));
        state["instance_id"] = json!(first.identity.instance_id());
        assert_eq!(serde_json::from_slice::<Value>(&after.2).unwrap(), state);
    }

    #[test]
    fn bound_identity_cancel_before_admission_has_no_ca_and_after_cert_drains_complete_material() {
        let temp = tempfile::tempdir().unwrap();
        let root = root(temp.path());
        assert!(
            prepare_service_identity_at_root_checked(&root, &|| Err("cancelled".to_owned()))
                .is_err()
        );
        assert!(!temp.path().join("link").exists());
        let cancelled = Arc::new(AtomicBool::new(false));
        let cut = Arc::clone(&cancelled);
        let (result, fired) = run_with_bound_publication_barrier(
            BoundPublicationPrimitive::ParentSync,
            1,
            move || {
                cut.store(true, Ordering::SeqCst);
            },
            || {
                prepare_service_identity_at_root_checked(&root, &|| {
                    if cancelled.load(Ordering::SeqCst) {
                        Err("cancelled".to_owned())
                    } else {
                        Ok(())
                    }
                })
            },
        );
        assert!(fired);
        let refusal = result.err().unwrap();
        assert_eq!(refusal.publications.len(), 3);
        assert!(
            refusal
                .publications
                .iter()
                .all(IdentityPublication::confirmed)
        );
        let before = snapshot(temp.path());
        let later = prepare_service_identity_at_root_checked(&root, &|| Ok(())).unwrap();
        assert!(later.publications.is_empty());
        assert_eq!(snapshot(temp.path()), before);
        assert_eq!(later.identity.certificate_pem(), before.0);
    }

    #[test]
    fn bound_identity_partial_native_failure_is_retained_and_never_formats_private_bytes() {
        let temp = tempfile::tempdir().unwrap();
        let root = root(temp.path());
        let (result, consumed) =
            run_with_bound_publication_fault(BoundPublicationPrimitive::ParentSync, 1, 5, || {
                prepare_service_identity_at_root_checked(&root, &|| Ok(()))
            });
        assert!(consumed);
        let error = result.err().unwrap();
        assert_eq!(error.publications.len(), 1);
        assert!(!error.publications[0].confirmed());
        assert!(temp.path().join("link/ca/cert.pem").exists());
        assert!(!temp.path().join("link/ca/private.pem").exists());
        assert!(prepare_service_identity_at_root_checked(&root, &|| Ok(())).is_err());
        let output = format!("{error:?} {error}");
        assert!(!output.contains("BEGIN"));
        assert!(!output.contains("observation"));
    }

    #[test]
    fn bound_identity_late_ca_or_state_leaf_is_preserved_with_prior_facts() {
        for (ordinal, relative) in [
            (1, "link/ca/cert.pem"),
            (2, "link/ca/private.pem"),
            (3, "link/state.json"),
        ] {
            let temp = tempfile::tempdir().unwrap();
            let root = root(temp.path());
            let leaf = temp.path().join(relative);
            let late = leaf.clone();
            let (result, fired) = run_with_bound_publication_barrier(
                BoundPublicationPrimitive::ExclusivePublish,
                ordinal,
                move || fs::write(late, b"inherited-late-sentinel").unwrap(),
                || prepare_service_identity_at_root_checked(&root, &|| Ok(())),
            );
            assert!(fired);
            let refusal = result.err().unwrap();
            assert_eq!(refusal.publications.len(), ordinal - 1);
            assert!(
                refusal
                    .publications
                    .iter()
                    .all(IdentityPublication::confirmed)
            );
            assert_eq!(fs::read(leaf).unwrap(), b"inherited-late-sentinel");
        }
    }

    #[test]
    fn bound_identity_incomplete_or_malformed_inherited_material_is_never_rotated() {
        for kind in ["incomplete", "malformed", "native-mismatch"] {
            let temp = tempfile::tempdir().unwrap();
            let root = root(temp.path());
            prepare_service_identity_at_root_checked(&root, &|| Ok(())).unwrap();
            if kind == "incomplete" {
                fs::remove_file(temp.path().join("link/ca/private.pem")).unwrap();
            } else if kind == "malformed" {
                fs::write(temp.path().join("link/state.json"), b"malformed-state").unwrap();
            } else {
                let mut state: Value =
                    serde_json::from_slice(&fs::read(temp.path().join("link/state.json")).unwrap())
                        .unwrap();
                state["instance_id"] = json!("wrong-native-id");
                fs::write(
                    temp.path().join("link/ca/state.json"),
                    bytes(&state).unwrap(),
                )
                .unwrap();
                fs::remove_file(temp.path().join("link/state.json")).unwrap();
            }
            let leaves = [
                "link/ca/cert.pem",
                "link/ca/private.pem",
                "link/state.json",
                "link/ca/state.json",
            ];
            let before = leaves.map(|p| fs::read(temp.path().join(p)).ok());
            let error = prepare_service_identity_at_root_checked(&root, &|| Ok(()))
                .err()
                .unwrap();
            assert!(error.publications.is_empty());
            assert_eq!(leaves.map(|p| fs::read(temp.path().join(p)).ok()), before);
        }
    }

    #[test]
    fn bound_identity_parent_swap_before_or_after_native_cut_preserves_replacement() {
        for relative in ["", "link/ca"] {
            for primitive in [
                BoundPublicationPrimitive::TempCreate,
                BoundPublicationPrimitive::ExclusivePublish,
            ] {
                let temp = tempfile::tempdir().unwrap();
                let original = temp.path().join("journal");
                fs::create_dir(&original).unwrap();
                let root = root(&original);
                let detached = temp.path().join("detached");
                let replacement = original.join(relative);
                let replaced = replacement.clone();
                let moved = detached.clone();
                let (result, fired) = run_with_bound_publication_barrier(
                    primitive,
                    1,
                    move || {
                        fs::rename(&replaced, moved).unwrap();
                        fs::create_dir(&replaced).unwrap();
                        fs::write(replaced.join("sentinel"), b"keep").unwrap();
                    },
                    || prepare_service_identity_at_root_checked(&root, &|| Ok(())),
                );
                assert!(fired);
                let refusal = result.err().unwrap();
                let published = primitive == BoundPublicationPrimitive::ExclusivePublish;
                assert_eq!(refusal.publications.len(), usize::from(published));
                if published {
                    assert!(!refusal.publications[0].confirmed());
                }
                let cert = if relative.is_empty() {
                    detached.join("link/ca/cert.pem")
                } else {
                    detached.join("cert.pem")
                };
                assert_eq!(cert.exists(), published);
                assert_eq!(fs::read(replacement.join("sentinel")).unwrap(), b"keep");
                assert_eq!(
                    fs::read_dir(replacement).unwrap().count(),
                    1,
                    "only inherited replacement sentinel remains"
                );
            }
        }
    }
}
