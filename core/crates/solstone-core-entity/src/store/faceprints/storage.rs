// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
use super::*;
use solstone_core_journal_io::{JournalRoot, LockOptions, contained_path, hold_lock, read_bytes};
use std::path::{Path, PathBuf};

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}
fn hold_directory(dir: &Path) -> Result<(JournalRoot, solstone_core_journal_io::FileLock)> {
    let binding = JournalRoot::open(dir).map_err(err)?;
    let lock = hold_lock(dir.join(ARCHIVE), LockOptions::default()).map_err(err)?;
    binding.revalidate_canonical_binding().map_err(err)?;
    Ok((binding, lock))
}
pub fn entity_directory(journal: &Path, entity: &str) -> Result<PathBuf> {
    super::super::entity_paths::entity_memory_path(journal, entity, false).map_err(err)
}
fn optional(path: &Path) -> Result<Option<Vec<u8>>> {
    if !solstone_core_journal_io::path_lexists(path).map_err(err)? {
        return Ok(None);
    }
    read_bytes(path, vec![]).map(Some).map_err(err)
}
pub(super) fn read_archive(dir: &Path) -> Result<Option<Archive>> {
    optional(&dir.join(ARCHIVE))?
        .map(|b| codec::read(&b))
        .transpose()
}
fn read_state(dir: &Path) -> Result<FacesState> {
    let state: FacesState = optional(&dir.join(STATE))?
        .map(|b| serde_json::from_slice(&b).map_err(err))
        .transpose()?
        .unwrap_or_default();
    if state.schema_version != 1 {
        return Err("unsupported face owner-state version".into());
    }
    for photo in state
        .decision
        .photo
        .iter()
        .chain(state.last.iter().filter_map(|u| u.decision.photo.as_ref()))
    {
        if !valid_embedding(&photo.face.embedding) || photo.encoder.width != WIDTH {
            return Err("invalid confirmed face anchor".into());
        }
    }
    Ok(state)
}
fn apply_rejections(archive: &mut Archive, state: &FacesState) {
    for f in &mut archive.faces {
        if let Some((_, rejected)) = state
            .decision
            .rejections
            .iter()
            .rev()
            .find(|(k, _)| k == &f.metadata.key)
        {
            f.metadata.rejected = *rejected;
        }
    }
}

/// Coherent read under the same trust and archive locks as every mutation.
/// Do not serialize the returned private state into an API response.
pub fn load(journal: &Path, entity: &str) -> Result<(Option<Archive>, FacesState)> {
    let _trust = crate::hold_entity_trust_lock(journal).map_err(err)?;
    let dir = entity_directory(journal, entity)?;
    load_directory(&dir)
}

/// Batch readers resolve the identity map once, then use its folder coordinates.
pub fn load_in_dir(journal: &Path, entity_dir: &str) -> Result<(Option<Archive>, FacesState)> {
    let _trust = crate::hold_entity_trust_lock(journal).map_err(err)?;
    let path = super::super::paths::identity_path(journal, entity_dir).map_err(err)?;
    load_directory(path.parent().expect("entity identity has a directory"))
}

fn load_directory(dir: &Path) -> Result<(Option<Archive>, FacesState)> {
    if !solstone_core_journal_io::path_lexists(&dir.join(ARCHIVE)).map_err(err)?
        && !solstone_core_journal_io::path_lexists(&dir.join(STATE)).map_err(err)?
    {
        return Ok((None, FacesState::default()));
    }
    let (binding, _lock) = hold_directory(dir)?;
    let mut archive = read_archive(dir)?;
    let state = read_state(dir)?;
    if let Some(a) = &mut archive {
        apply_rejections(a, &state);
    }
    binding.revalidate_canonical_binding().map_err(err)?;
    Ok((archive, state))
}

/// Bound atomic publication, followed by an exact read-back check.
pub(super) fn publish(dir: &Path, root: &JournalRoot, name: &str, bytes: &[u8]) -> Result<()> {
    let admitted = || root.revalidate_canonical_binding().map_err(err);
    admitted()?;
    #[cfg(unix)]
    {
        let outcome = solstone_core_journal_io::atomic_replace_bound_checked(
            root,
            std::ffi::OsStr::new(name),
            bytes,
            0o600,
            &admitted,
        )
        .map_err(err)?;
        if !matches!(
            outcome,
            solstone_core_journal_io::BoundAtomicOutcome::Published { .. }
        ) {
            return Err("face publication durability uncertain".into());
        }
    }
    #[cfg(not(unix))]
    solstone_core_journal_io::atomic_replace_checked(
        dir.join(name),
        bytes,
        solstone_core_journal_io::AtomicWriteOptions { mode: Some(0o600) },
        &admitted,
    )
    .map_err(err)?;
    admitted()?;
    if read_bytes(dir.join(name), vec![]).map_err(err)? != bytes {
        return Err("face publication changed after write".into());
    }
    Ok(())
}
pub(super) fn save_archive(dir: &Path, binding: &JournalRoot, archive: &Archive) -> Result<()> {
    publish(dir, binding, ARCHIVE, &codec::write(archive)?)
}
fn save_state(dir: &Path, binding: &JournalRoot, state: &FacesState) -> Result<()> {
    publish(
        dir,
        binding,
        STATE,
        &serde_json::to_vec(state).map_err(err)?,
    )
}
fn publish_photo(dir: &Path, binding: &JournalRoot, state: &FacesState) -> Result<()> {
    binding.revalidate_canonical_binding().map_err(err)?;
    if let Some(photo) = &state.decision.photo {
        publish(dir, binding, PHOTO, &photo.jpeg)?;
    } else {
        #[expect(
            clippy::disallowed_methods,
            reason = "system internal: remove the photo projection; owner state retains the authoritative JPEG and undo"
        )]
        solstone_core_journal_io::remove_file(dir, PHOTO).map_err(err)?;
    }
    binding.revalidate_canonical_binding().map_err(err)
}

/// State is authoritative; photo.jpg is its repairable materialized copy.
/// Routes serving a confirmed photo use this function, including after a crash.
pub fn confirmed_photo(journal: &Path, entity: &str) -> Result<Option<Vec<u8>>> {
    let _trust = crate::hold_entity_trust_lock(journal).map_err(err)?;
    let dir = entity_directory(journal, entity)?;
    if !solstone_core_journal_io::path_lexists(&dir.join(STATE)).map_err(err)?
        && !solstone_core_journal_io::path_lexists(&dir.join(PHOTO)).map_err(err)?
    {
        return Ok(None);
    }
    let (binding, _lock) = hold_directory(&dir)?;
    let state = read_state(&dir)?;
    if let Some(photo) = state.decision.photo {
        if optional(&dir.join(PHOTO))?.as_deref() != Some(photo.jpeg.as_slice()) {
            publish(&dir, &binding, PHOTO, &photo.jpeg)?;
        }
        Ok(Some(photo.jpeg))
    } else {
        publish_photo(&dir, &binding, &state)?;
        Ok(None)
    }
}

pub fn crop_path(journal: &Path, key: &FaceKey, crop: &str) -> Result<PathBuf> {
    let segment = segment_relative(key)?;
    // Only the producer's private crop namespace, never a raw or arbitrary file.
    let prefix = accumulation::source_prefix(&key.source_name);
    let Some(file) = crop.strip_prefix(&format!("{prefix}-crops/")) else {
        return Err("invalid face crop reference".into());
    };
    if file.contains(['/', '\\']) || !file.ends_with(".jpg") || file.starts_with('.') {
        return Err("invalid face crop filename".into());
    }
    contained_path(journal, &format!("{segment}/{crop}")).map_err(err)
}
pub fn segment_relative(key: &FaceKey) -> Result<String> {
    for value in [&key.day, &key.stream, &key.segment_key, &key.source_name] {
        if value.is_empty() || value.contains(['/', '\\']) || value == "." || value == ".." {
            return Err("invalid face coordinate".into());
        }
    }
    match key.stream_layout.as_str() {
        "direct" => Ok(format!("chronicle/{}/{}", key.day, key.segment_key)),
        "named" => Ok(format!(
            "chronicle/{}/{}/{}",
            key.day, key.stream, key.segment_key
        )),
        _ => Err("invalid face stream layout".into()),
    }
}

/// One durable owner action. The returned token is required by undo.
fn action(
    journal: &Path,
    entity: &str,
    change: impl FnOnce(&mut FacesState, &Archive) -> ActionResult<()>,
) -> ActionResult<String> {
    let _trust = crate::hold_entity_trust_lock(journal).map_err(err)?;
    let dir = entity_directory(journal, entity)?;
    let (binding, _lock) = hold_directory(&dir)?;
    let mut state = read_state(&dir)?;
    let mut archive = read_archive(&dir)?.ok_or(ActionError::NoEvidence)?;
    apply_rejections(&mut archive, &state);
    let token = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Nanos, true);
    let mut previous = state.decision.clone();
    change(&mut state, &archive)?;
    for (key, _) in state
        .decision
        .rejections
        .iter()
        .skip(previous.rejections.len())
    {
        let was = archive
            .faces
            .iter()
            .find(|f| &f.metadata.key == key)
            .is_some_and(|f| f.metadata.rejected);
        previous.rejections.push((key.clone(), was));
    }
    state.last = Some(Undo {
        token: token.clone(),
        decision: previous,
    });
    save_state(&dir, &binding, &state)?;
    apply_rejections(&mut archive, &state);
    save_archive(&dir, &binding, &archive)?;
    // A failed projection never loses the committed choice or undo snapshot.
    publish_photo(&dir, &binding, &state)?;
    Ok(token)
}

pub fn choose(journal: &Path, entity: &str, key: &FaceKey) -> ActionResult<String> {
    let _trust = crate::hold_entity_trust_lock(journal).map_err(err)?;
    // Busy producer claims are refused rather than waited for under entity trust.
    let (archive, _) = load(journal, entity)?;
    let face = archive
        .as_ref()
        .and_then(|a| a.faces.iter().find(|f| &f.metadata.key == key))
        .ok_or(ActionError::UnknownKey)?;
    let crop = face.metadata.crop.as_deref().ok_or("face has no crop")?;
    let path = crop_path(journal, key, crop)?;
    let segment = contained_path(journal, &segment_relative(key)?).map_err(err)?;
    let claims = solstone_core_segment::output_attempt::OutputClaims::discover(
        &segment,
        accumulation::claim_options(),
    )
    .map_err(ActionError::source)?;
    let jpeg = optional(&path)?
        .filter(|bytes| !bytes.is_empty())
        .ok_or("face crop is no longer available")?;
    action(journal, entity, |state, archive| {
        claims.segment_identity(&segment).map_err(err)?;
        let current = archive
            .faces
            .iter()
            .find(|f| &f.metadata.key == key)
            .ok_or(ActionError::UnknownKey)?;
        if current != face {
            return Err("face evidence changed; refresh before choosing".into());
        }
        state.decision.photo = Some(ChosenPhoto {
            face: current.clone(),
            encoder: archive.envelope.encoder.clone(),
            chosen_at: chrono::Utc::now().to_rfc3339(),
            jpeg,
        });
        state.decision.dismissed_at = None;
        Ok(())
    })
}
pub fn reject(journal: &Path, entity: &str, key: &FaceKey) -> ActionResult<String> {
    action(journal, entity, |state, archive| {
        if !archive.faces.iter().any(|f| &f.metadata.key == key) {
            return Err(ActionError::UnknownKey);
        }
        state.decision.rejections.push((key.clone(), true));
        Ok(())
    })
}
pub fn dismiss(journal: &Path, entity: &str) -> ActionResult<String> {
    action(journal, entity, |state, archive| {
        state.decision.photo = None;
        state.decision.dismissed_at = Some(chrono::Utc::now().to_rfc3339());
        state.decision.dismissed_looks =
            archive.faces.iter().map(|f| f.embedding.clone()).collect();
        Ok(())
    })
}
pub fn undo(journal: &Path, entity: &str, last: &str) -> ActionResult<()> {
    let _trust = crate::hold_entity_trust_lock(journal).map_err(err)?;
    let dir = entity_directory(journal, entity)?;
    let (binding, _lock) = hold_directory(&dir)?;
    let mut state = read_state(&dir)?;
    let undo = state
        .last
        .take()
        .filter(|u| u.token == last)
        .ok_or(ActionError::StaleAction)?;
    state.decision = undo.decision;
    save_state(&dir, &binding, &state)?;
    if let Some(mut a) = read_archive(&dir)? {
        apply_rejections(&mut a, &state);
        save_archive(&dir, &binding, &a)?;
    }
    publish_photo(&dir, &binding, &state)?;
    Ok(())
}

/// Replace only this activity's associations; other activities retain shared rows.
/// Caller holds the source claims and authoritative activity lock until return.
pub fn reconcile(
    journal: &Path,
    activity: &str,
    encoder: &Encoder,
    desired: &std::collections::BTreeMap<String, Vec<Face>>,
) -> Result<usize> {
    let _trust = crate::hold_entity_trust_lock(journal).map_err(err)?;
    let entities = crate::live_journal_entities(journal).map_err(err)?;
    reconcile_in_dirs(journal, activity, encoder, desired, &entities)
}

pub(super) fn reconcile_in_dirs(
    journal: &Path,
    activity: &str,
    encoder: &Encoder,
    desired: &std::collections::BTreeMap<String, Vec<Face>>,
    entities: &[(String, crate::JournalEntity)],
) -> Result<usize> {
    let mut active = vec![];
    for (folder, entity) in entities {
        let dir = journal.join("entities").join(folder);
        if desired.get(&entity.id).is_some_and(|v| !v.is_empty())
            || solstone_core_journal_io::path_lexists(&dir.join(ARCHIVE)).map_err(err)?
            || solstone_core_journal_io::path_lexists(&dir.join(STATE)).map_err(err)?
        {
            active.push((folder, entity));
        }
    }
    let entities = active;
    // Preflight all archives before moving any associations. Interrupted writes
    // are replayed from the current activity, never from an old desired snapshot.
    for (dir, _) in &entities {
        if let Some(a) = read_archive(&journal.join("entities").join(dir))?
            && a.envelope.encoder != *encoder
        {
            return Err("face encoder mismatch".into());
        }
        let state = read_state(&journal.join("entities").join(dir))?;
        if state
            .decision
            .photo
            .iter()
            .chain(state.last.iter().filter_map(|u| u.decision.photo.as_ref()))
            .any(|p| p.encoder != *encoder)
        {
            return Err("confirmed face encoder mismatch".into());
        }
    }
    let mut added = 0;
    for (folder, entity) in entities {
        let dir = journal.join("entities").join(folder);
        if desired.get(&entity.id).is_none_or(Vec::is_empty)
            && !solstone_core_journal_io::path_lexists(&dir.join(ARCHIVE)).map_err(err)?
        {
            continue;
        }
        let (binding, _lock) = hold_directory(&dir)?;
        let before = read_archive(&dir)?;
        let mut archive = before
            .clone()
            .unwrap_or_else(|| Archive::empty(encoder.clone()));
        let state = read_state(&dir)?;
        apply_rejections(&mut archive, &state);
        let wanted = desired
            .get(&entity.id)
            .map(Vec::as_slice)
            .unwrap_or_default();
        for f in &mut archive.faces {
            f.metadata.activities.remove(activity);
        }
        for f in wanted {
            if let Some(old) = archive
                .faces
                .iter_mut()
                .find(|o| o.metadata.key == f.metadata.key)
            {
                let rejected = old.metadata.rejected;
                let added_at = old.metadata.added_at.clone();
                let mut activities = old.metadata.activities.clone();
                activities.insert(activity.to_owned());
                *old = f.clone();
                old.metadata.rejected |= rejected;
                old.metadata.activities = activities;
                old.metadata.added_at = added_at;
            } else {
                let mut f = f.clone();
                f.metadata.activities = BTreeSet::from([activity.to_owned()]);
                archive.faces.push(f);
                added += 1;
            }
        }
        archive.faces.retain(|f| !f.metadata.activities.is_empty());
        if before.as_ref() != Some(&archive) && (before.is_some() || !archive.faces.is_empty()) {
            save_archive(&dir, &binding, &archive)?;
        }
    }
    Ok(added)
}

/// Lifecycle cleanup runs even when recognition is disabled. Confirmed state
/// and its undo copy deliberately survive the removal of inferred evidence.
/// Hold before a source can move; keep through inferred-row cleanup. Recovery
/// may take ordinary source-file locks, so call before the global trash lock.
pub fn hold_source_removal(journal: &Path) -> Result<crate::EntityTrustLock> {
    let trust = crate::trust_lock::hold_entity_trust_lock_with_options(
        journal,
        accumulation::claim_options(),
    )
    .map_err(err)?;
    crate::recover_interrupted_entity_merge(journal)?;
    Ok(trust)
}

pub fn remove_segment_rows(journal: &Path, relative: &str) -> Result<usize> {
    // Lifecycle callers already fenced merges before moving the source. Never
    // recover a merge or wait for trust while holding the global trash lock.
    let _trust = crate::trust_lock::hold_entity_trust_lock_with_options(
        journal,
        accumulation::claim_options(),
    )
    .map_err(err)?;
    let mut removed = 0;
    for (folder, _) in crate::live_journal_entities(journal).map_err(err)? {
        let dir = journal.join("entities").join(folder);
        if !solstone_core_journal_io::path_lexists(&dir.join(ARCHIVE)).map_err(err)? {
            continue;
        }
        let (binding, _lock) = hold_directory(&dir)?;
        if let Some(mut archive) = read_archive(&dir)? {
            let before = archive.faces.len();
            archive
                .faces
                .retain(|f| segment_relative(&f.metadata.key).is_ok_and(|p| p != relative));
            removed += before - archive.faces.len();
            if archive.faces.len() != before {
                save_archive(&dir, &binding, &archive)?;
            }
        }
    }
    Ok(removed)
}

/// A recovered person cannot revive inferred rows from absent recordings.
/// Owner choices and their undo snapshots remain independent of source rows.
pub fn prune_unavailable_segments(journal: &Path, entity: &str) -> Result<usize> {
    let _trust = crate::hold_entity_trust_lock(journal).map_err(err)?;
    let dir = entity_directory(journal, entity)?;
    if !solstone_core_journal_io::path_lexists(&dir.join(ARCHIVE)).map_err(err)? {
        return Ok(0);
    }
    let (binding, _lock) = hold_directory(&dir)?;
    let Some(mut archive) = read_archive(&dir)? else {
        return Ok(0);
    };
    let mut live = std::collections::BTreeMap::new();
    for face in &archive.faces {
        let rel = segment_relative(&face.metadata.key)?;
        if let std::collections::btree_map::Entry::Vacant(entry) = live.entry(rel) {
            let segment = contained_path(journal, entry.key()).map_err(err)?;
            let present = segment.is_dir()
                && solstone_core_segment::owner_deletion_state(&segment).map_err(err)?
                    == solstone_core_segment::OwnerDeletionState::Live;
            entry.insert(present);
        }
    }
    let before = archive.faces.len();
    archive.faces.retain(|f| {
        segment_relative(&f.metadata.key).is_ok_and(|rel| live.get(&rel) == Some(&true))
    });
    let removed = before - archive.faces.len();
    if removed > 0 {
        save_archive(&dir, &binding, &archive)?;
    }
    Ok(removed)
}

/// Remove references to activities that no longer exist after a recovery scan.
/// Caller holds entity trust for the entire authoritative activity census.
pub fn remove_orphan_activities(
    journal: &Path,
    live: &std::collections::BTreeSet<String>,
) -> Result<usize> {
    let _trust = crate::hold_entity_trust_lock(journal).map_err(err)?;
    let mut removed = 0;
    for (folder, _) in crate::live_journal_entities(journal).map_err(err)? {
        let dir = journal.join("entities").join(folder);
        if !solstone_core_journal_io::path_lexists(&dir.join(ARCHIVE)).map_err(err)? {
            continue;
        }
        let (binding, _lock) = hold_directory(&dir)?;
        if let Some(mut a) = read_archive(&dir)? {
            let before = a.clone();
            for f in &mut a.faces {
                f.metadata.activities.retain(|id| live.contains(id));
            }
            a.faces.retain(|f| !f.metadata.activities.is_empty());
            removed += before.faces.len() - a.faces.len();
            if a != before {
                save_archive(&dir, &binding, &a)?;
            }
        }
    }
    Ok(removed)
}

pub(crate) fn preflight_merge(journal: &Path, source: &str, target: &str) -> Result<()> {
    let (a, sa) = load(journal, source)?;
    let (b, sb) = load(journal, target)?;
    let encoders: Vec<_> = a
        .iter()
        .chain(b.iter())
        .map(|a| &a.envelope.encoder)
        .chain(
            sa.decision
                .photo
                .iter()
                .chain(sb.decision.photo.iter())
                .map(|p| &p.encoder),
        )
        .collect();
    if encoders.windows(2).any(|e| e[0] != e[1]) {
        return Err("face encoder mismatch during merge".into());
    }
    Ok(())
}
pub(crate) fn merge(journal: &Path, source: &str, target: &str) -> Result<()> {
    preflight_merge(journal, source, target)?;
    let (a, sa) = load(journal, source)?;
    let (b, mut sb) = load(journal, target)?;
    if a.is_none() && b.is_none() && sa == FacesState::default() && sb == FacesState::default() {
        return Ok(());
    }
    let dir = entity_directory(journal, target)?;
    let (binding, _lock) = hold_directory(&dir)?;
    if let Some(a) = a {
        let mut b = b.unwrap_or_else(|| Archive::empty(a.envelope.encoder.clone()));
        for f in a.faces {
            if let Some(old) = b
                .faces
                .iter_mut()
                .find(|g| g.metadata.key == f.metadata.key)
            {
                old.metadata.rejected |= f.metadata.rejected;
                old.metadata.activities.extend(f.metadata.activities);
            } else {
                b.faces.push(f);
            }
        }
        for face in b.faces.iter().filter(|f| f.metadata.rejected) {
            sb.decision
                .rejections
                .push((face.metadata.key.clone(), true));
        }
        save_archive(&dir, &binding, &b)?;
    }
    if sb.decision.photo.is_none() {
        sb.decision.photo = sa.decision.photo;
    }
    sb.decision
        .rejections
        .extend(sa.decision.rejections.into_iter().filter(|(_, v)| *v));
    sb.last = None; // a merge is permanent; old undo cannot restore pre-merge state.
    if sb != FacesState::default() {
        save_state(&dir, &binding, &sb)?;
    }
    if let Some(photo) = &sb.decision.photo {
        publish(&dir, &binding, PHOTO, &photo.jpeg)?;
    }
    Ok(())
}
