// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Local redundant-copy settlement. Nothing here observes a remote operation,
//! confirms an archive, adopts a Mark or authorizes another creator.

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{DateTime, FixedOffset, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use solstone_core_journal_config::JournalConfigRead;
use solstone_core_journal_io::JournalRoot;
use solstone_core_journal_io::activity::ActivityLease;
use solstone_core_journal_io::activity_obligations::{NativeObligation, observe_obligations};
use solstone_core_journal_io::journal_root::JournalEntryKind;
use solstone_core_journal_io::media::{
    MediaDigest, MediaDirectory, MediaEntry, MediaIdentity, RetainedMediaFile, StageRetirement,
    StageRetirementReceipt, exclusive_local_activity,
};
use solstone_core_journal_io::paths::{DEFAULT_STREAM, SegmentLayout};
use solstone_core_retention::archive_catalog::{JournalCatalog, read_complete_catalog};
use solstone_core_retention::content::{
    ClosedHandlerSet, ContentName, JournalMedia, MediaClassifier,
};
use solstone_core_retention::grouped::{DecisionPurpose, inspect_grouped};
use solstone_core_retention::marks::Register;
use solstone_core_retention::receipt::Target;
use solstone_core_retention::scan::scan_segment_bound;
use solstone_core_segment::{OwnerDeletionState, RetainedOwnerDeletion};

const MAX_ENTRIES: usize = 4096;
const MAX_CONFIG: usize = 1024 * 1024;

/// Requested coordinates only. The owner independently proves current content.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CreatorStageSelection {
    pub stage: String,
    pub target: Target,
    pub layout: SegmentLayout,
    pub name: String,
}

/// Descriptive comparison data only. Every effect still needs a fresh retained
/// owner and complete proof; a saved confirmation cannot supply either.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CreatorConfirmation {
    version: u8,
    root_observation: String,
    context_revision: Option<String>,
    stages: Vec<StageConfirmation>,
}

impl CreatorConfirmation {
    pub fn revision(&self) -> io::Result<String> {
        Ok(format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(self).map_err(io::Error::other)?)
        ))
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StageConfirmation {
    stage: String,
    directory: MediaIdentity,
    revision: String,
    leaf: Option<FileConfirmation>,
    original: Option<OriginalConfirmation>,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct FileConfirmation {
    name: String,
    identity: MediaIdentity,
    revision: String,
    digest: MediaDigest,
}

impl FileConfirmation {
    fn capture(file: &RetainedMediaFile) -> io::Result<Self> {
        Ok(Self {
            name: file.entry().name.to_str().ok_or_else(refuse)?.to_owned(),
            identity: file.entry().identity,
            revision: file.entry().observation_revision()?,
            digest: file.digest().clone(),
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct OriginalConfirmation {
    selection: CreatorStageSelection,
    directory: MediaIdentity,
    group_revision: Vec<String>,
    raw: FileConfirmation,
}

fn refuse() -> io::Error {
    io::Error::other("creator recovery evidence is incomplete or changed")
}

struct Context {
    config_directory: Arc<MediaDirectory>,
    config: (JournalConfigRead, Option<MediaEntry>),
    register: Register,
    catalog: JournalCatalog,
    at: DateTime<FixedOffset>,
    inventory: Vec<(Arc<MediaDirectory>, Vec<MediaEntry>)>,
}

impl Context {
    fn revision(&self) -> io::Result<String> {
        let inventory = self
            .inventory
            .iter()
            .map(|(directory, entries)| {
                Ok(json!({"directory":directory.identity(),"entries":entry_revisions(entries)?}))
            })
            .collect::<io::Result<Vec<_>>>()?;
        let bytes=serde_json::to_vec(&json!({
            "config_directory":self.config_directory.identity(),
            "config_bytes":self.config.0.sha256,
            "config_entry":self.config.1.as_ref().map(MediaEntry::observation_revision).transpose()?,
            "register":self.register,"catalog":format!("{:?}",self.catalog),"inventory":inventory
        })).map_err(io::Error::other)?;
        Ok(format!("{:x}", Sha256::digest(bytes)))
    }
    fn capture(root: &Arc<JournalRoot>, at: DateTime<FixedOffset>) -> io::Result<Self> {
        let config_directory = MediaDirectory::open(Arc::clone(root), Path::new("config"))?;
        let config = solstone_core_journal_config::read_journal_config_observed_in_directory(
            &config_directory,
            MAX_CONFIG,
        )
        .map_err(|_| refuse())?;
        if config.0.config.is_none() {
            return Err(refuse());
        }
        Ok(Self {
            config_directory,
            config,
            register: solstone_core_retention::review_context::load_at_root(root)
                .map_err(|_| refuse())?,
            catalog: read_complete_catalog(root).map_err(|_| refuse())?,
            at,
            inventory: Vec::new(),
        })
    }

    fn revalidate(&self, root: &Arc<JournalRoot>) -> io::Result<()> {
        for (directory, entries) in &self.inventory {
            if directory.entries(MAX_ENTRIES)?.as_ref() != Some(entries) {
                return Err(refuse());
            }
        }
        self.config_directory.revalidate()?;
        let current = solstone_core_journal_config::read_journal_config_observed_in_directory(
            &self.config_directory,
            MAX_CONFIG,
        )
        .map_err(|_| refuse())?;
        if current != self.config
            || solstone_core_retention::review_context::load_at_root(root).map_err(|_| refuse())?
                != self.register
            || read_complete_catalog(root).map_err(|_| refuse())? != self.catalog
        {
            return Err(refuse());
        }
        Ok(())
    }
}

struct Original {
    selection: CreatorStageSelection,
    directory: Arc<MediaDirectory>,
    deletion: RetainedOwnerDeletion,
    entries: Vec<MediaEntry>,
    raw: RetainedMediaFile,
}

impl Original {
    fn confirmation(&self) -> io::Result<OriginalConfirmation> {
        Ok(OriginalConfirmation {
            selection: self.selection.clone(),
            directory: self.directory.identity(),
            group_revision: entry_revisions(&self.entries)?,
            raw: FileConfirmation::capture(&self.raw)?,
        })
    }
    fn capture(
        root: &Arc<JournalRoot>,
        context: &Context,
        selection: &CreatorStageSelection,
        checkpoint: &dyn Fn() -> io::Result<()>,
    ) -> io::Result<Self> {
        let target = &selection.target;
        let day = NaiveDate::parse_from_str(&target.day, "%Y%m%d").map_err(|_| refuse())?;
        if day.format("%Y%m%d").to_string() != target.day
            || !solstone_core_segment::is_safe_stream_component(&target.stream)
            || solstone_core_format::segment::segment_key(&target.dir).is_none()
            || target.dir.contains(['/', '\\'])
            || selection.layout == SegmentLayout::Direct && target.stream != DEFAULT_STREAM
        {
            return Err(refuse());
        }
        let name = ContentName::new(&selection.name).ok_or_else(refuse)?;
        if !JournalMedia.is_owner_media(&name) {
            return Err(refuse());
        }
        let mut relative = PathBuf::from("chronicle").join(&target.day);
        if selection.layout == SegmentLayout::Named {
            relative.push(&target.stream);
        }
        let parent = MediaDirectory::open(Arc::clone(root), &relative)?;
        let directory = parent
            .open_child(OsStr::new(&target.dir))?
            .ok_or_else(refuse)?;
        let deletion = RetainedOwnerDeletion::open(
            parent,
            OsStr::new(&target.dir),
            Some(&directory),
            MAX_ENTRIES,
        )
        .map_err(|_| refuse())?;
        let entries = directory.entries(MAX_ENTRIES)?.ok_or_else(refuse)?;
        let raw = directory
            .retain_file(OsStr::new(&selection.name), checkpoint)?
            .ok_or_else(refuse)?;
        let original = Self {
            selection: selection.clone(),
            directory,
            deletion,
            entries,
            raw,
        };
        original.revalidate(context, checkpoint)?;
        Ok(original)
    }

    fn revalidate(
        &self,
        context: &Context,
        checkpoint: &dyn Fn() -> io::Result<()>,
    ) -> io::Result<()> {
        checkpoint()?;
        if self.deletion.state().map_err(|_| refuse())? != OwnerDeletionState::Live
            || self.directory.entries(MAX_ENTRIES)?.as_ref() != Some(&self.entries)
        {
            return Err(refuse());
        }
        let target = &self.selection.target;
        if context.register.marks.values().any(|mark| {
            mark.target == *target
                && mark
                    .proposal
                    .names
                    .iter()
                    .any(|name| name == &self.selection.name)
        }) {
            return Err(refuse());
        }
        let key = solstone_core_format::segment::segment_key(&target.dir).ok_or_else(refuse)?;
        if context
            .catalog
            .days
            .iter()
            .filter(|day| day.day == target.day)
            .flat_map(|day| &day.segments)
            .filter(|segment| segment.stream == target.stream && segment.segment == key)
            .any(|segment| {
                segment
                    .active
                    .iter()
                    .any(|witness| witness.file.name == self.selection.name)
            })
        {
            return Err(refuse());
        }
        let scan = scan_segment_bound(
            &self.directory,
            &ClosedHandlerSet,
            &JournalMedia,
            MAX_ENTRIES,
        )?;
        let config = context.config.0.config.as_ref().ok_or_else(refuse)?;
        let policy = solstone_core_retention::policy::policy_from_journal_config(config);
        let inspection = inspect_grouped(
            &scan,
            target,
            self.selection.layout,
            &policy,
            context.at.date_naive(),
            context.at.with_timezone(&Utc),
            DecisionPurpose::OffloadProposal,
        );
        let accepted = inspection
            .eligible()
            .find(|file| file.name() == self.selection.name)
            .ok_or_else(refuse)?;
        let name = ContentName::new(&self.selection.name).ok_or_else(refuse)?;
        if scan.bound_entry(&name) != Some(self.raw.entry())
            || accepted.bytes() != self.raw.digest().size
        {
            return Err(refuse());
        }
        self.raw.revalidate_contents(checkpoint)?;
        if self.directory.entries(MAX_ENTRIES)?.as_ref() != Some(&self.entries) {
            return Err(refuse());
        }
        Ok(())
    }
}

struct Stage {
    entry: MediaEntry,
    directory: Arc<MediaDirectory>,
    leaf: Option<RetainedMediaFile>,
    original: Option<Original>,
    retirement: StageRetirement,
}

impl Stage {
    fn revalidate(
        &self,
        context: Option<&Context>,
        checkpoint: &dyn Fn() -> io::Result<()>,
    ) -> io::Result<()> {
        checkpoint()?;
        self.directory.revalidate()?;
        let entries = self.directory.entries(2)?.ok_or_else(refuse)?;
        match (&self.leaf, &self.original) {
            (None, None) if entries.is_empty() => Ok(()),
            (Some(leaf), Some(original)) if entries.as_slice() == [leaf.entry().clone()] => {
                original.revalidate(context.ok_or_else(refuse)?, checkpoint)?;
                leaf.revalidate_contents(checkpoint)?;
                if leaf.digest() != original.raw.digest() {
                    return Err(refuse());
                }
                Ok(())
            }
            _ => Err(refuse()),
        }
    }
}

/// The entire local outcome retains every proof, retirement and original lease.
/// Drop closes handles only; it never retries effects or starts ordinary work.
#[must_use]
pub struct CreatorStageRecovery {
    root: Arc<JournalRoot>,
    parent: Option<Arc<MediaDirectory>>,
    context: Option<Context>,
    stages: Vec<Stage>,
    reconciliation_originals: Vec<Original>,
    refusal: Option<String>,
    attempted: bool,
    reconciled: bool,
    // Native handles and their possible deferred effects retire before exclusion.
    activity: Option<Arc<ActivityLease>>,
}

impl CreatorStageRecovery {
    fn empty(root: Arc<JournalRoot>) -> Self {
        Self {
            root,
            parent: None,
            context: None,
            stages: Vec::new(),
            reconciliation_originals: Vec::new(),
            refusal: None,
            attempted: false,
            reconciled: false,
            activity: None,
        }
    }

    /// A complete clean observation creates no activity sidecar. Any actual
    /// creator stage needs the producer's existing exclusion before matching.
    pub fn observe(
        root: Arc<JournalRoot>,
        at: DateTime<FixedOffset>,
        checkpoint: &dyn Fn() -> io::Result<()>,
    ) -> Self {
        let mut owner = Self::empty(root);
        if owner.discover(at, checkpoint).is_err() {
            owner.refusal = Some("creator_recovery_refused".into());
        }
        owner
    }

    pub fn enter(
        root: Arc<JournalRoot>,
        selections: &[CreatorStageSelection],
        at: DateTime<FixedOffset>,
        checkpoint: &dyn Fn() -> io::Result<()>,
    ) -> Self {
        let mut owner = Self::empty(root);
        if owner.capture(selections, at, checkpoint).is_err() {
            owner.refusal = Some("creator_recovery_refused".into());
        }
        owner
    }

    /// Compare the complete newly proved population before any removal. Even an
    /// otherwise valid replacement or added/removed stage invalidates review.
    pub fn apply_confirmed(
        &mut self,
        expected: &CreatorConfirmation,
        checkpoint: &dyn Fn() -> io::Result<()>,
    ) {
        if !matches!(self.confirmation(checkpoint),Ok(current) if &current==expected) {
            self.refusal = Some("creator_recovery_confirmation_changed".into());
            return;
        }
        self.apply(checkpoint);
    }

    pub fn confirmation(
        &self,
        checkpoint: &dyn Fn() -> io::Result<()>,
    ) -> io::Result<CreatorConfirmation> {
        if self.refusal.is_some() || self.attempted || self.stages.len() > 64 {
            return Err(refuse());
        }
        if self.activity.is_some() {
            self.revalidate_pending(checkpoint)?;
        } else {
            checkpoint()?;
            self.root
                .revalidate_canonical_binding()
                .map_err(|_| refuse())?;
            if !observe_obligations(&self.root)
                .map_err(|_| refuse())?
                .entries()
                .is_empty()
            {
                return Err(refuse());
            }
        }
        let stages = self
            .stages
            .iter()
            .map(|stage| {
                Ok(StageConfirmation {
                    stage: stage.entry.name.to_str().ok_or_else(refuse)?.to_owned(),
                    directory: stage.directory.identity(),
                    revision: stage.entry.observation_revision()?,
                    leaf: stage
                        .leaf
                        .as_ref()
                        .map(FileConfirmation::capture)
                        .transpose()?,
                    original: stage
                        .original
                        .as_ref()
                        .map(Original::confirmation)
                        .transpose()?,
                })
            })
            .collect::<io::Result<Vec<_>>>()?;
        Ok(CreatorConfirmation {
            version: 1,
            root_observation: self.root.identity().observation_id(),
            context_revision: self.context.as_ref().map(Context::revision).transpose()?,
            stages,
        })
    }

    /// A separate read-only current-state result. It never relabels an old
    /// unknown/partial attempt, deletes a copy, or makes a remote claim.
    pub fn reconcile(
        root: Arc<JournalRoot>,
        prior: &CreatorConfirmation,
        at: DateTime<FixedOffset>,
        checkpoint: &dyn Fn() -> io::Result<()>,
    ) -> Self {
        let mut owner = Self::empty(root);
        let check = (|| {
            checkpoint()?;
            if prior.version != 1
                || prior.stages.len() > 64
                || prior.root_observation != owner.root.identity().observation_id()
            {
                return Err(refuse());
            }
            owner.activity = Some(
                ActivityLease::open_existing(Arc::clone(&owner.root))
                    .map_err(|_| refuse())?
                    .ok_or_else(refuse)?,
            );
            if !exclusive_local_activity(owner.activity.as_ref().ok_or_else(refuse)?)?
                || !observe_obligations(&owner.root)
                    .map_err(|_| refuse())?
                    .entries()
                    .is_empty()
            {
                return Err(refuse());
            }
            if prior.stages.iter().any(|stage| stage.original.is_some()) {
                owner.context = Some(Context::capture(&owner.root, at)?);
            }
            for stage in &prior.stages {
                if !valid_stage_name(&stage.stage) {
                    return Err(refuse());
                }
                match (&stage.leaf, &stage.original) {
                    (None, None) => {}
                    (Some(leaf), Some(original))
                        if leaf.digest == original.raw.digest
                            && original.selection.stage == stage.stage =>
                    {
                        let current = Original::capture(
                            &owner.root,
                            owner.context.as_ref().ok_or_else(refuse)?,
                            &original.selection,
                            checkpoint,
                        )?;
                        if current.confirmation()? != *original {
                            return Err(refuse());
                        }
                        owner.reconciliation_originals.push(current);
                    }
                    _ => return Err(refuse()),
                }
            }
            owner.revalidate_pending(checkpoint)?;
            Ok(())
        })();
        if check.is_err() {
            owner.refusal = Some("creator_recovery_reconciliation_refused".into());
        } else {
            owner.reconciled = true;
        }
        owner
    }

    fn capture(
        &mut self,
        selections: &[CreatorStageSelection],
        at: DateTime<FixedOffset>,
        checkpoint: &dyn Fn() -> io::Result<()>,
    ) -> io::Result<()> {
        checkpoint()?;
        self.root
            .revalidate_canonical_binding()
            .map_err(|_| refuse())?;
        self.activity = Some(
            ActivityLease::open_existing(Arc::clone(&self.root))
                .map_err(|_| refuse())?
                .ok_or_else(refuse)?,
        );
        self.capture_admitted(selections, at, checkpoint)
    }

    fn capture_admitted(
        &mut self,
        selections: &[CreatorStageSelection],
        at: DateTime<FixedOffset>,
        checkpoint: &dyn Fn() -> io::Result<()>,
    ) -> io::Result<()> {
        let activity = self.activity.as_ref().ok_or_else(refuse)?;
        if !exclusive_local_activity(activity)? {
            return Err(refuse());
        }
        let census = observe_obligations(&self.root).map_err(|_| refuse())?;
        if census.entries().len() > 64 {
            return Err(refuse());
        }
        let parent = MediaDirectory::open(Arc::clone(&self.root), Path::new(""))?;
        self.parent = Some(Arc::clone(&parent));
        let mut requested = BTreeMap::new();
        for selection in selections {
            if !valid_stage_name(&selection.stage)
                || requested
                    .insert(selection.stage.as_str(), selection)
                    .is_some()
            {
                return Err(refuse());
            }
        }
        if !selections.is_empty() && self.context.is_none() {
            self.context = Some(Context::capture(&self.root, at)?);
        }
        for obligation in census.entries() {
            checkpoint()?;
            let NativeObligation::Stage { relative, entry } = obligation else {
                return Err(refuse());
            };
            let name = entry
                .name
                .to_str()
                .filter(|name| valid_stage_name(name))
                .ok_or_else(refuse)?;
            if relative != Path::new(&entry.name) || entry.kind != JournalEntryKind::Directory {
                return Err(refuse());
            }
            let directory = parent.open_child(&entry.name)?.ok_or_else(refuse)?;
            if directory.identity() != entry.identity {
                return Err(refuse());
            }
            let children = directory.entries(2)?.ok_or_else(refuse)?;
            let (leaf, original, retirement) = match children.as_slice() {
                [] => (
                    None,
                    None,
                    StageRetirement::empty(
                        Arc::clone(activity),
                        Arc::clone(&parent),
                        entry.name.clone(),
                        entry.identity,
                    ),
                ),
                [child] if child.kind == JournalEntryKind::RegularFile => {
                    let selection = requested.remove(name).ok_or_else(refuse)?;
                    if child.name != OsStr::new(&selection.name) {
                        return Err(refuse());
                    }
                    let leaf = directory
                        .retain_file(&child.name, checkpoint)?
                        .ok_or_else(refuse)?;
                    if leaf.entry() != child {
                        return Err(refuse());
                    }
                    let original = Original::capture(
                        &self.root,
                        self.context.as_ref().ok_or_else(refuse)?,
                        selection,
                        checkpoint,
                    )?;
                    if leaf.digest() != original.raw.digest()
                        || leaf.entry().identity == original.raw.entry().identity
                    {
                        return Err(refuse());
                    }
                    let retirement = StageRetirement::recorded(
                        Arc::clone(activity),
                        Arc::clone(&parent),
                        entry.name.clone(),
                        entry.identity,
                        child.name.clone(),
                        child.identity,
                    );
                    (Some(leaf), Some(original), retirement)
                }
                _ => return Err(refuse()),
            };
            self.stages.push(Stage {
                entry: entry.clone(),
                directory,
                leaf,
                original,
                retirement,
            });
        }
        if !requested.is_empty() {
            return Err(refuse());
        }
        self.revalidate_pending(checkpoint)
    }

    fn discover(
        &mut self,
        at: DateTime<FixedOffset>,
        checkpoint: &dyn Fn() -> io::Result<()>,
    ) -> io::Result<()> {
        checkpoint()?;
        self.root
            .revalidate_canonical_binding()
            .map_err(|_| refuse())?;
        let initial = observe_obligations(&self.root).map_err(|_| refuse())?;
        if initial.entries().is_empty() {
            return Ok(());
        }
        self.activity = Some(
            ActivityLease::open_existing(Arc::clone(&self.root))
                .map_err(|_| refuse())?
                .ok_or_else(refuse)?,
        );
        if !exclusive_local_activity(self.activity.as_ref().ok_or_else(refuse)?)? {
            return Err(refuse());
        }
        let census = observe_obligations(&self.root).map_err(|_| refuse())?;
        if census.entries().len() > 64 {
            return Err(refuse());
        }
        let parent = MediaDirectory::open(Arc::clone(&self.root), Path::new(""))?;
        let mut copied = Vec::new();
        for obligation in census.entries() {
            checkpoint()?;
            let NativeObligation::Stage { relative, entry } = obligation else {
                return Err(refuse());
            };
            if relative != Path::new(&entry.name)
                || entry.kind != JournalEntryKind::Directory
                || !entry.name.to_str().is_some_and(valid_stage_name)
            {
                return Err(refuse());
            }
            let directory = parent.open_child(&entry.name)?.ok_or_else(refuse)?;
            if directory.identity() != entry.identity {
                return Err(refuse());
            }
            let children = directory.entries(2)?.ok_or_else(refuse)?;
            match children.as_slice() {
                [] => {}
                [child] if child.kind == JournalEntryKind::RegularFile => {
                    let leaf = directory
                        .retain_file(&child.name, checkpoint)?
                        .ok_or_else(refuse)?;
                    if leaf.entry() != child {
                        return Err(refuse());
                    }
                    copied.push((
                        entry.name.to_str().ok_or_else(refuse)?.to_owned(),
                        child.name.to_str().ok_or_else(refuse)?.to_owned(),
                        leaf,
                    ));
                }
                _ => return Err(refuse()),
            }
        }
        let mut selections = Vec::new();
        if !copied.is_empty() {
            let mut context = Context::capture(&self.root, at)?;
            let candidates = candidate_segments(&self.root, &mut context.inventory, checkpoint)?;
            for (stage, name, leaf) in &copied {
                let mut matches = Vec::new();
                for candidate in &candidates {
                    checkpoint()?;
                    let entries = candidate
                        .directory
                        .entries(MAX_ENTRIES)?
                        .ok_or_else(refuse)?;
                    let Some(entry) = entries.iter().find(|entry| {
                        entry.name == OsStr::new(name)
                            && entry.kind == JournalEntryKind::RegularFile
                            && entry.size == leaf.digest().size
                    }) else {
                        continue;
                    };
                    let current = candidate
                        .directory
                        .retain_file(&entry.name, checkpoint)?
                        .ok_or_else(refuse)?;
                    if current.digest() != leaf.digest() {
                        continue;
                    }
                    let selection = CreatorStageSelection {
                        stage: stage.clone(),
                        target: candidate.target.clone(),
                        layout: candidate.layout,
                        name: name.clone(),
                    };
                    Original::capture(&self.root, &context, &selection, checkpoint)?;
                    matches.push(selection);
                }
                if matches.len() != 1 {
                    return Err(refuse());
                }
                selections.extend(matches);
            }
            self.context = Some(context);
        }
        self.capture_admitted(&selections, at, checkpoint)
    }

    fn revalidate_pending(&self, checkpoint: &dyn Fn() -> io::Result<()>) -> io::Result<()> {
        checkpoint()?;
        let activity = self.activity.as_ref().ok_or_else(refuse)?;
        activity.revalidate().map_err(|_| refuse())?;
        if !exclusive_local_activity(activity)? {
            return Err(refuse());
        }
        if let Some(context) = &self.context {
            context.revalidate(&self.root)?;
            for original in &self.reconciliation_originals {
                original.revalidate(context, checkpoint)?;
            }
        }
        let current = observe_obligations(&self.root).map_err(|_| refuse())?;
        let expected = self
            .stages
            .iter()
            .filter(|stage| !stage.retirement.completed())
            .map(|stage| NativeObligation::Stage {
                relative: PathBuf::from(&stage.entry.name),
                entry: stage.entry.clone(),
            })
            .collect::<Vec<_>>();
        if current.entries() != expected {
            return Err(refuse());
        }
        for stage in self
            .stages
            .iter()
            .filter(|stage| !stage.retirement.completed())
        {
            stage.revalidate(self.context.as_ref(), checkpoint)?;
        }
        checkpoint()?;
        activity.revalidate().map_err(|_| refuse())
    }

    /// One explicit local attempt. Prior observation never replaces this fresh
    /// complete proof. A refusal or partial effect never triggers retry or Start.
    pub fn apply(&mut self, checkpoint: &dyn Fn() -> io::Result<()>) {
        if self.attempted || self.refusal.is_some() {
            return;
        }
        self.attempted = true;
        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.apply_once(checkpoint)))
            .is_err()
        {
            self.refusal = Some("creator_recovery_interrupted".into());
        }
    }

    fn apply_once(&mut self, checkpoint: &dyn Fn() -> io::Result<()>) {
        for index in 0..self.stages.len() {
            if self.revalidate_pending(checkpoint).is_err() {
                self.refusal = Some("creator_recovery_changed".into());
                return;
            }
            // Each native cut rechecks current original/group evidence. Stage
            // identity/shape is additionally guarded by the retirement owner.
            let stage = &mut self.stages[index];
            let context = self.context.as_ref();
            let original = stage.original.as_ref();
            let leaf = stage.leaf.as_ref();
            let root = Arc::clone(&self.root);
            let check_original = || {
                checkpoint()?;
                if let Some(context) = context {
                    context.revalidate(&root)?;
                    if let Some(original) = original {
                        original.revalidate(context, checkpoint)?;
                    }
                }
                Ok(())
            };
            stage
                .retirement
                .retire_with_leaf_check(&check_original, &|| {
                    check_original()?;
                    if let Some(leaf) = leaf {
                        leaf.revalidate_contents(checkpoint)?;
                    }
                    Ok(())
                });
            if !stage.retirement.completed() {
                self.refusal = Some("creator_recovery_partial".into());
                return;
            }
        }
        if self.revalidate_pending(checkpoint).is_err() {
            self.refusal = Some("creator_recovery_changed".into());
        }
    }

    pub fn report(&self) -> Value {
        json!({
            "scope":"local_redundant_creator_copies", "remote_outcome":"unknown",
            "root_observation":self.root.identity().observation_id(),
            "ordinary_work_started":false, "replay_authorized":false,
            "attempted":self.attempted, "refusal":self.refusal,
            "current_local_state_reconciled":self.reconciled,
            "previous_outcome":"unknown_or_partial",
            "local_complete":self.attempted && self.refusal.is_none()
                && self.stages.iter().all(|stage| stage.retirement.completed()),
            "stages":self.stages.iter().map(|stage| {
                let receipt = stage.retirement.receipt();
                json!({"stage":stage.entry.name.to_string_lossy(), "complete":stage.retirement.completed(),
                    "facts":receipt_facts(receipt)})
            }).collect::<Vec<_>>()
        })
    }
}

fn entry_revisions(entries: &[MediaEntry]) -> io::Result<Vec<String>> {
    entries
        .iter()
        .map(MediaEntry::observation_revision)
        .collect()
}

struct CandidateSegment {
    target: Target,
    layout: SegmentLayout,
    directory: Arc<MediaDirectory>,
}

fn inventory_entries(
    directory: &Arc<MediaDirectory>,
    inventory: &mut Vec<(Arc<MediaDirectory>, Vec<MediaEntry>)>,
    checkpoint: &dyn Fn() -> io::Result<()>,
) -> io::Result<Vec<MediaEntry>> {
    checkpoint()?;
    if inventory.len() >= MAX_ENTRIES {
        return Err(refuse());
    }
    let entries = directory.entries(MAX_ENTRIES)?.ok_or_else(refuse)?;
    inventory.push((Arc::clone(directory), entries.clone()));
    Ok(entries)
}

fn candidate_segments(
    root: &Arc<JournalRoot>,
    inventory: &mut Vec<(Arc<MediaDirectory>, Vec<MediaEntry>)>,
    checkpoint: &dyn Fn() -> io::Result<()>,
) -> io::Result<Vec<CandidateSegment>> {
    let parent = MediaDirectory::open(Arc::clone(root), Path::new(""))?;
    let Some(chronicle) = parent.open_child(OsStr::new("chronicle"))? else {
        return Ok(Vec::new());
    };
    let mut candidates = Vec::new();
    for day in inventory_entries(&chronicle, inventory, checkpoint)? {
        let day_name = day.name.to_str().ok_or_else(refuse)?;
        if !solstone_core_journal_io::paths::is_day_key(day_name) {
            continue;
        }
        if day.kind != JournalEntryKind::Directory {
            return Err(refuse());
        }
        let day_dir = chronicle.open_child(&day.name)?.ok_or_else(refuse)?;
        if day_dir.identity() != day.identity {
            return Err(refuse());
        }
        for child in inventory_entries(&day_dir, inventory, checkpoint)? {
            let name = child.name.to_str().ok_or_else(refuse)?;
            if child.kind != JournalEntryKind::Directory {
                continue;
            }
            let directory = day_dir.open_child(&child.name)?.ok_or_else(refuse)?;
            if directory.identity() != child.identity {
                return Err(refuse());
            }
            if solstone_core_format::segment::segment_key(name).is_some() {
                inventory_entries(&directory, inventory, checkpoint)?;
                candidates.push(CandidateSegment {
                    target: Target {
                        day: day_name.to_owned(),
                        stream: DEFAULT_STREAM.into(),
                        dir: name.to_owned(),
                    },
                    layout: SegmentLayout::Direct,
                    directory,
                });
            } else if solstone_core_segment::is_safe_stream_component(name) {
                for segment in inventory_entries(&directory, inventory, checkpoint)? {
                    let segment_name = segment.name.to_str().ok_or_else(refuse)?;
                    // Segment writers leave regular sibling lock files. Keep
                    // them in the inventory, outside the candidate population.
                    if segment.kind == JournalEntryKind::RegularFile
                        && segment_name.strip_suffix(".lock").is_some_and(|base| {
                            solstone_core_format::segment::segment_key(base).as_deref()
                                == Some(base)
                        })
                    {
                        continue;
                    }
                    if solstone_core_format::segment::segment_key(segment_name).is_none() {
                        continue;
                    }
                    if segment.kind != JournalEntryKind::Directory {
                        return Err(refuse());
                    }
                    let segment_dir = directory.open_child(&segment.name)?.ok_or_else(refuse)?;
                    if segment_dir.identity() != segment.identity {
                        return Err(refuse());
                    }
                    inventory_entries(&segment_dir, inventory, checkpoint)?;
                    candidates.push(CandidateSegment {
                        target: Target {
                            day: day_name.to_owned(),
                            stream: name.to_owned(),
                            dir: segment_name.to_owned(),
                        },
                        layout: SegmentLayout::Named,
                        directory: segment_dir,
                    });
                }
            }
            if candidates.len() > MAX_ENTRIES {
                return Err(refuse());
            }
        }
    }
    Ok(candidates)
}

fn valid_stage_name(name: &str) -> bool {
    name.strip_prefix(".tmp-offload-").is_some_and(|suffix| {
        suffix.len() == 32
            && suffix
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

fn receipt_facts(receipt: &StageRetirementReceipt) -> Value {
    json!({"existed":receipt.existed,
        "leaf_effect":format!("{:?}",receipt.leaf_effect),
        "leaf_absence":format!("{:?}",receipt.leaf_absence),
        "directory_effect":format!("{:?}",receipt.directory_effect),
        "directory_absence":format!("{:?}",receipt.directory_absence),
        "parent_durability":format!("{:?}",receipt.parent_durability),
        "failure":receipt.failure.as_ref().map(|error| format!("{:?}",error.kind()))})
}

#[cfg(all(test, feature = "full-tests"))]
#[cfg(target_os = "linux")]
mod tests;
