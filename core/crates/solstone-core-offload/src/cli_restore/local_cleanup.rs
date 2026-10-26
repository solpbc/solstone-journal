// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Local retirement of completed NoMark metadata. No media publication or CAS.

use std::ffi::OsStr;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use solstone_core_journal_io::JournalRoot;
use solstone_core_journal_io::activity::ActivityLease;
use solstone_core_journal_io::activity_obligations::{
    IntentRead, MAX_INTENT_BYTES, NativeObligation, intent_filename, observe_obligations,
};
use solstone_core_journal_io::journal_root::JournalEntryKind;
use solstone_core_journal_io::media::{
    Cleanup, Confirmation, Durability, MediaDirectory, MediaObservation, MediaRead, MediaRemoval,
    MediaStageCreation, OriginalName, PublicationEffect, RemovalEffect, RemovalPhase,
    RemovalReceipt, StageRetirement, StageRetirementReceipt, digest_reader,
    exclusive_local_activity,
};
use solstone_core_retention::archive_catalog::{RestoreDisposition, read_complete_catalog};
use solstone_core_retention::restore_intent::{
    CurrentContent, DestinationObservation, MarkRetirement, Obligation, RegisterDecision,
    RestoreIntent, inspect_recovery, selected_name,
};
use solstone_core_retention::review_context;

fn refuse(reason: impl std::fmt::Display) -> io::Error {
    io::Error::other(reason.to_string())
}

struct Candidate {
    relative: PathBuf,
    read: MediaRead,
    intent: RestoreIntent,
    parent: Arc<MediaDirectory>,
    live: Arc<MediaDirectory>,
    health: Arc<MediaDirectory>,
    directory: Arc<MediaDirectory>,
    creation: Option<MediaStageCreation>,
    removal: Option<MediaRemoval>,
    retired: bool,
}

impl Candidate {
    fn capture(root: &Arc<JournalRoot>, obligation: &NativeObligation) -> io::Result<Self> {
        let NativeObligation::Intent {
            relative,
            entry,
            contents:
                IntentRead::Complete {
                    operation_id,
                    bytes,
                },
        } = obligation
        else {
            return Err(refuse("unqualified restore intent"));
        };
        let intent =
            RestoreIntent::decode(Ok(bytes)).map_err(|e| refuse(format!("intent: {e:?}")))?;
        let name = intent_filename(operation_id).ok_or_else(|| refuse("invalid intent name"))?;
        if intent.event.operation_id != *operation_id
            || relative != &Path::new("health/offload/restore-intents").join(&name)
            || !matches!(intent.retirement, MarkRetirement::NoMark)
            || intent.recovery_binding.is_some()
            || intent.legacy_binding.is_some()
            || intent.register != Obligation::NotRequired
            || intent.marker != Obligation::Complete
            || intent.event_recorded != Obligation::Complete
            || intent.cleanup != Obligation::Complete
            || intent.event.files.iter().any(|f| {
                !matches!(
                    f.disposition,
                    RestoreDisposition::Published | RestoreDisposition::AlreadyPresent
                )
            })
            || intent.outcome.as_ref().is_none_or(|facts| {
                facts.confirmation != Confirmation::Confirmed
                    || facts.cleanup != Cleanup::Complete
                    || !matches!(
                        facts.effect,
                        PublicationEffect::Published | PublicationEffect::AlreadyPresent
                    )
                    || facts.effect == PublicationEffect::Published
                        && (facts.file_durability != Durability::Synced
                            || !cfg!(windows) && facts.parent_durability != Durability::Synced)
            })
        {
            return Err(refuse(
                "restore intent still owes work or physical authority",
            ));
        }
        let segment = intent.locator.relative_segment();
        let parent = MediaDirectory::open(
            Arc::clone(root),
            segment.parent().ok_or_else(|| refuse("invalid segment"))?,
        )?;
        let live = parent
            .open_child(
                segment
                    .file_name()
                    .ok_or_else(|| refuse("invalid segment"))?,
            )?
            .ok_or_else(|| refuse("missing segment"))?;
        let health = MediaDirectory::open(Arc::clone(root), Path::new("health/offload"))?;
        let directory = health
            .open_child(OsStr::new("restore-intents"))?
            .ok_or_else(|| refuse("missing intents directory"))?;
        let read = directory
            .read_file_bounded(&name, MAX_INTENT_BYTES)?
            .ok_or_else(|| refuse("missing intent"))?;
        if !read.complete || read.entry != *entry || read.bytes != *bytes {
            return Err(refuse("intent changed"));
        }
        let candidate = Self {
            relative: relative.clone(),
            read,
            intent,
            parent,
            live,
            health,
            directory,
            creation: None,
            removal: None,
            retired: false,
        };
        candidate.verify(root)?;
        Ok(candidate)
    }

    fn verify(&self, root: &Arc<JournalRoot>) -> io::Result<()> {
        root.revalidate_canonical_binding().map_err(refuse)?;
        let intent = &self.intent;
        let destination = DestinationObservation {
            root: root.identity().observation_id(),
            parent: self.parent.identity(),
            segment: self.live.identity(),
        };
        if destination != intent.destination || self.parent.identity() != intent.stage.parent {
            return Err(refuse("restore destination changed"));
        }
        self.parent.revalidate()?;
        self.live.revalidate()?;
        let current = self
            .live
            .observe_file(selected_name(intent))?
            .ok_or_else(|| refuse("published file missing"))?;
        if current.digest != intent.expected_digest()
            || intent.confirmed.as_ref() != Some(&current.clone().into())
        {
            return Err(refuse("published file changed"));
        }
        let catalog = read_complete_catalog(root).map_err(|e| refuse(format!("catalog: {e:?}")))?;
        if catalog.degraded() {
            return Err(refuse("degraded catalog"));
        }
        let mut segments = catalog
            .days
            .iter()
            .flat_map(|day| &day.segments)
            .filter(|segment| {
                segment.day == intent.locator.target.day
                    && segment.stream == intent.locator.target.stream
                    && segment.segment == intent.locator.segment
            });
        let segment = segments
            .next()
            .ok_or_else(|| refuse("missing restore catalog"))?;
        if segments.next().is_some() {
            return Err(refuse("ambiguous restore catalog"));
        }
        let register =
            review_context::load_at_root(root).map_err(|e| refuse(format!("register: {e:?}")))?;
        let decision = inspect_recovery(
            intent,
            &destination,
            segment,
            &register,
            CurrentContent::Present(current.into()),
        )
        .map_err(|e| refuse(format!("restore verification: {e:?}")))?;
        if !decision.event_recorded
            || !matches!(decision.register, RegisterDecision::NoMark)
            || decision.marker != Obligation::Complete
            || decision.cleanup != Obligation::Complete
        {
            return Err(refuse("restore remains incomplete"));
        }
        let read = self
            .directory
            .read_file_bounded(&self.read.entry.name, MAX_INTENT_BYTES)?;
        if read.as_ref() != Some(&self.read) {
            return Err(refuse("restore record changed"));
        }
        Ok(())
    }

    fn stage_relative(&self) -> io::Result<PathBuf> {
        Ok(self
            .intent
            .locator
            .relative_segment()
            .parent()
            .ok_or_else(|| refuse("invalid segment"))?
            .join(&self.intent.stage.stage_name))
    }

    fn removed(&self) -> bool {
        self.removal.as_ref().is_some_and(|removal| {
            let r = removal.receipt();
            r.effect == RemovalEffect::Deleted
                && r.original_name == OriginalName::Absent
                && r.claim_confirmation == Confirmation::Confirmed
                && r.cleanup == Cleanup::Complete
                && (cfg!(windows)
                    || r.source_durability == Durability::Synced
                        && r.claim_durability == Durability::Synced)
                && !r.failures.iter().any(|f| f.phase != RemovalPhase::Cleanup)
        })
    }
}

/// Receipts retain completed facts only, never grant another effect.
#[derive(Default)]
pub(super) struct CompletedLocalCleanup {
    pub(super) stages: Vec<StageRetirementReceipt>,
    pub(super) intents: Vec<RemovalReceipt>,
}

pub(super) struct LocalCleanup {
    root: Arc<JournalRoot>,
    remaining: Vec<NativeObligation>,
    candidates: Vec<Candidate>,
    stages: Vec<(PathBuf, StageRetirement)>,
    complete: bool,
    // Keep the operation's exclusion until every native child has closed.
    activity: Arc<ActivityLease>,
}

impl LocalCleanup {
    #[cfg(all(test, feature = "full-tests"))]
    pub(super) fn removals(&self) -> impl Iterator<Item = &MediaRemoval> {
        self.candidates
            .iter()
            .filter_map(|candidate| candidate.removal.as_ref())
    }

    #[cfg(all(test, feature = "full-tests"))]
    pub(super) fn creations(&self) -> impl Iterator<Item = &MediaStageCreation> {
        self.candidates
            .iter()
            .filter_map(|candidate| candidate.creation.as_ref())
    }

    pub(super) fn new(activity: Arc<ActivityLease>) -> Self {
        Self {
            root: Arc::clone(activity.journal_root()),
            activity,
            remaining: vec![],
            candidates: vec![],
            stages: vec![],
            complete: false,
        }
    }

    pub(super) fn prepare(&mut self, checkpoint: &dyn Fn() -> io::Result<()>) -> io::Result<()> {
        checkpoint()?;
        if !exclusive_local_activity(&self.activity)? {
            return Err(refuse("private local activity is not qualified"));
        }
        self.remaining = observe_obligations(&self.root)
            .map_err(refuse)?
            .into_entries();
        for obligation in &self.remaining {
            if matches!(obligation, NativeObligation::Intent { .. }) {
                self.candidates
                    .push(Candidate::capture(&self.root, obligation)?);
            }
        }
        for obligation in &self.remaining {
            let NativeObligation::Stage { relative, entry } = obligation else {
                continue;
            };
            if entry.kind != JournalEntryKind::Directory {
                return Err(refuse("unknown stage entry"));
            }
            let parent_path = relative.parent().ok_or_else(|| refuse("invalid stage"))?;
            let parent = MediaDirectory::open(Arc::clone(&self.root), parent_path)?;
            let stage = parent
                .open_child(&entry.name)?
                .ok_or_else(|| refuse("stage disappeared"))?;
            if stage.identity() != entry.identity {
                return Err(refuse("stage replaced"));
            }
            let entries = stage
                .entries(1)?
                .ok_or_else(|| refuse("unrelated nonempty stage"))?;
            let mut matching = self.candidates.iter().filter(|candidate| {
                candidate
                    .stage_relative()
                    .as_ref()
                    .is_ok_and(|path| path == relative)
            });
            let candidate = matching.next();
            if matching.next().is_some() {
                return Err(refuse("multiply referenced stage"));
            }
            let owner = if let Some(candidate) = candidate {
                let expected = &candidate.intent.stage;
                if parent.identity() != expected.parent
                    || stage.identity() != expected.directory
                    || entries.iter().any(|leaf| {
                        leaf.kind != JournalEntryKind::RegularFile
                            || leaf.name != OsStr::new(&expected.leaf_name)
                            || leaf.identity != expected.leaf
                    })
                {
                    return Err(refuse("recorded stage changed"));
                }
                StageRetirement::recorded(
                    Arc::clone(&self.activity),
                    parent,
                    entry.name.clone(),
                    entry.identity,
                    expected.leaf_name.clone().into(),
                    expected.leaf,
                )
            } else {
                if !entries.is_empty()
                    || !(parent_path.as_os_str().is_empty()
                        || parent_path == Path::new("health/offload"))
                {
                    return Err(refuse("unqualified orphan stage"));
                }
                StageRetirement::empty(
                    Arc::clone(&self.activity),
                    parent,
                    entry.name.clone(),
                    entry.identity,
                )
            };
            self.stages.push((relative.clone(), owner));
        }
        self.verify_remaining(checkpoint)
    }

    fn verify_remaining(&self, checkpoint: &dyn Fn() -> io::Result<()>) -> io::Result<()> {
        checkpoint()?;
        self.activity.revalidate().map_err(refuse)?;
        let observed = observe_obligations(&self.root).map_err(refuse)?;
        if observed.entries().len() != self.remaining.len()
            || observed
                .entries()
                .iter()
                .any(|entry| !self.remaining.contains(entry))
        {
            return Err(refuse("recovery obligations changed"));
        }
        for candidate in self
            .candidates
            .iter()
            .filter(|candidate| !candidate.retired)
        {
            candidate.verify(&self.root)?;
        }
        checkpoint()
    }

    pub(super) fn retire(&mut self, checkpoint: &dyn Fn() -> io::Result<()>) -> io::Result<()> {
        for index in 0..self.stages.len() {
            self.verify_remaining(checkpoint)?;
            let root = &self.root;
            let candidates = &self.candidates;
            let stage_checkpoint = || {
                checkpoint()?;
                for candidate in candidates.iter().filter(|candidate| !candidate.retired) {
                    candidate.verify(root)?;
                }
                Ok(())
            };
            let (relative, stage) = &mut self.stages[index];
            stage.retire(&stage_checkpoint);
            if !stage.completed() {
                return Err(refuse(format!("stage retirement: {:?}", stage.receipt())));
            }
            self.remaining.retain(|entry| !matches!(entry, NativeObligation::Stage { relative: path, .. } if path == relative));
        }
        for index in 0..self.candidates.len() {
            self.verify_remaining(checkpoint)?;
            let candidate = &mut self.candidates[index];
            let expected = MediaObservation {
                identity: candidate.read.entry.identity,
                digest: digest_reader(&mut candidate.read.bytes.as_slice())?,
            };
            candidate.creation = Some(MediaStageCreation::new(
                Arc::clone(&candidate.health),
                Arc::clone(&self.activity),
            ));
            let creation = candidate
                .creation
                .as_mut()
                .ok_or_else(|| refuse("missing creation owner"))?;
            checkpoint()?;
            creation.create();
            checkpoint()?;
            let stage = creation
                .take_created()
                .ok_or_else(|| refuse(format!("intent claim creation: {:?}", creation.error())))?;
            candidate.removal = Some(candidate.directory.prepare_removal(
                &candidate.read.entry.name,
                &expected,
                stage,
            ));
            // The actual removal is installed outside its mutation/unwind frame.
            checkpoint()?;
            candidate
                .removal
                .as_mut()
                .ok_or_else(|| refuse("missing removal owner"))?
                .remove_checked(checkpoint);
            checkpoint()?;
            if !candidate.removed() {
                return Err(refuse(format!(
                    "intent removal: {:?}",
                    candidate.removal.as_ref().map(MediaRemoval::receipt)
                )));
            }
            candidate.retired = true;
            self.remaining.retain(|entry| !matches!(entry, NativeObligation::Intent { relative, .. } if *relative == candidate.relative));
        }
        self.verify_remaining(checkpoint)?;
        if !self.remaining.is_empty() {
            return Err(refuse("unresolved recovery obligations"));
        }
        self.complete = true;
        Ok(())
    }

    pub(super) fn into_completed(self) -> Result<CompletedLocalCleanup, Box<Self>> {
        if !self.complete {
            return Err(Box::new(self));
        }
        let mut report = CompletedLocalCleanup::default();
        for (_, stage) in self.stages {
            // Completion was checked before any owner was consumed.
            match stage.into_completed_receipt() {
                Ok(receipt) => report.stages.push(receipt),
                Err(_) => unreachable!("completed cleanup contains an incomplete stage"),
            }
        }
        for candidate in self.candidates {
            if let Some(removal) = candidate.removal {
                report.intents.push(removal.into_receipt());
            }
        }
        Ok(report)
    }
}
