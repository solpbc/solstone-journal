// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Facet link folders: the one owner of `facets/<facet>/entities/<folder>/`.
//!
//! A **link folder** is a directory under a facet's `entities/` that holds
//! `entity.json`; it links one journal entity into the facet and keeps that
//! entity's facet notes in `observations.jsonl` beside it. A directory there
//! without `entity.json` is an **orphan folder**: notes written for a name the
//! facet does not link. Plain files there (`<day>.jsonl` and friends) are
//! detected-entity day artifacts and are not links.
//!
//! The rule this module keeps: in one facet an entity has at most one link
//! folder, and that folder is named by the entity's id. Readers find a link by
//! the id it names, preferring the folder named by that id, so state written
//! before the rule still reads deterministically. Writers never displace
//! another entity's link: a folder named by the id that already links a
//! different entity (or cannot be read) is refused as needing repair, and only
//! the on-demand doctor moves it.
//!
//! Every function here works on an `entities/` directory addressed as a
//! `(root, relative)` pair, so the same code serves a journal facet and a
//! private staged copy of one.

use std::cmp::Ordering;
use std::collections::BTreeSet;
use std::fmt;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};
use solstone_core_journal_io::{
    AtomicWriteError, AtomicWriteOptions, DirEntryKind, JsonWriteOptions, MalformedPolicy,
    PathError, ReadError, contained_path, list_dir_entries, path_lexists, read_bytes, read_json,
    remove_dir_all, write_bytes_exclusive, write_json,
};

use super::derived::timestamp_ms;
use super::observations::{
    ObservationParseSource, ObservationRow, ObservationStoreError, parse_observation_file,
    serialize_observation_rows,
};

const LINK_FILE: &str = "entity.json";
const OBSERVATIONS_FILE: &str = "observations.jsonl";

/// Why a link-folder operation refused or failed.
#[derive(Debug)]
pub enum LinkFolderError {
    Path(PathError),
    Read(ReadError),
    Observations(ObservationStoreError),
    Write(AtomicWriteError),
    /// A link's `entity.json` is not a JSON object.
    NotObject {
        path: PathBuf,
    },
    /// A file both folders carry with different bytes; nothing was written.
    Conflict {
        path: PathBuf,
    },
    /// Something in a folder that isn't a file (a folder or a link), which a
    /// fold can't carry; nothing was written.
    NotAFile {
        path: PathBuf,
    },
    /// The folder named by `entity_id` links another entity or cannot be
    /// read, so the entity's link cannot be put there without moving it.
    NeedsRepair {
        entities: String,
        entity_id: String,
    },
    /// A caller-supplied write hook refused.
    Hook(String),
    /// The entity or retired-name records could not be read.
    Resolve(String),
    /// An entity id that can't name a folder: empty, a dot name, or holding a
    /// path separator.
    InvalidName {
        name: String,
    },
    /// Note ids ran out of room.
    IdSpace,
}

impl fmt::Display for LinkFolderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Path(error) => error.fmt(formatter),
            Self::Read(error) => error.fmt(formatter),
            Self::Observations(error) => error.fmt(formatter),
            Self::Write(error) => error.fmt(formatter),
            Self::NotObject { path } => {
                write!(formatter, "{} is not a JSON object", path.display())
            }
            Self::Conflict { path } => write!(
                formatter,
                "{} has different contents in the folders being combined, so nothing was combined",
                path.display()
            ),
            Self::NotAFile { path } => write!(
                formatter,
                "{} is a folder or link, not a file, so nothing was combined",
                path.display()
            ),
            Self::NeedsRepair {
                entities,
                entity_id,
            } => {
                let facet = entities
                    .strip_prefix("facets/")
                    .and_then(|rest| rest.strip_suffix("/entities"))
                    .map_or_else(
                        || "the facet being merged into".to_owned(),
                        |facet| format!("the '{facet}' facet"),
                    );
                write!(
                    formatter,
                    "the folder for '{entity_id}' in {facet} holds another entity or can't be read, so nothing was changed; run 'solstone journal facet doctor' to see what needs repair"
                )
            }
            Self::Hook(message) | Self::Resolve(message) => formatter.write_str(message),
            Self::InvalidName { name } => {
                write!(formatter, "'{name}' can't be used as an entity id")
            }
            Self::IdSpace => formatter.write_str("notes have run out of id numbers"),
        }
    }
}

impl std::error::Error for LinkFolderError {}

impl From<PathError> for LinkFolderError {
    fn from(error: PathError) -> Self {
        Self::Path(error)
    }
}

impl From<ReadError> for LinkFolderError {
    fn from(error: ReadError) -> Self {
        Self::Read(error)
    }
}

impl From<AtomicWriteError> for LinkFolderError {
    fn from(error: AtomicWriteError) -> Self {
        Self::Write(error)
    }
}

impl From<ObservationStoreError> for LinkFolderError {
    fn from(error: ObservationStoreError) -> Self {
        Self::Observations(error)
    }
}

/// A hook run with a folder's root-relative path before this module first
/// changes anything under it. Entity merge captures its rollback here.
pub type BeforeWrite<'a> = &'a mut dyn FnMut(&str) -> Result<(), LinkFolderError>;

/// One readable link folder.
#[derive(Debug, Clone, PartialEq)]
pub struct LinkEntry {
    /// The folder's name under `entities/`.
    pub dir: String,
    /// The stored `entity_id`, or the folder name when none is stored.
    pub entity_id: String,
    /// Whether `entity_id` was stored rather than taken from the folder name.
    pub id_written: bool,
    /// The whole `entity.json` object, unknown fields included.
    pub value: Value,
}

impl LinkEntry {
    pub fn detached(&self) -> bool {
        self.value.get("detached") == Some(&Value::Bool(true))
    }
}

/// What occupies one folder name under `entities/`.
#[derive(Debug, Clone, PartialEq)]
pub enum FolderState {
    Absent,
    Link(LinkEntry),
    /// A directory without `entity.json`.
    Orphan,
    /// `entity.json` exists and cannot be read as a link.
    Unreadable,
    /// Something other than a directory holds the name.
    NotADirectory,
}

/// How a fold settles two links' `entity.json` fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkFieldPolicy {
    /// Earliest `attached_at`, latest `updated_at`/`last_seen`, blanks filled,
    /// and detached only when both links are.
    Merge,
    /// The receiving link's fields stand as they are.
    TargetWins,
}

/// Rows a fold of two notes files added, renumbered, and dropped as copies.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FoldRows {
    pub added: usize,
    pub renumbered: usize,
    pub copies_dropped: usize,
    /// Link fields where the receiving link kept its own, different value.
    pub fields_kept: usize,
}

impl FoldRows {
    fn absorb(&mut self, other: FoldRows) {
        self.added += other.added;
        self.renumbered += other.renumbered;
        self.copies_dropped += other.copies_dropped;
        self.fields_kept += other.fields_kept;
    }
}

/// Combine two notes files' rows. No note is lost.
///
/// An incoming row is a copy, and is dropped, only when a receiving row has
/// the same content, `observed_at`, `source_day` and `relation`; the receiving
/// row stays exactly as it is, retired or not. Every other incoming row is
/// appended with every field kept, and always above every id already in the
/// receiving file: it keeps its own id when that is higher, otherwise it
/// takes the next one. A cursor that walks ids upward therefore never skips
/// an appended row.
pub fn fold_observation_rows(
    into: Vec<ObservationRow>,
    from: Vec<ObservationRow>,
) -> Result<(Vec<ObservationRow>, FoldRows), LinkFolderError> {
    let copies: BTreeSet<String> = into.iter().map(copy_key).collect();
    let mut highest = into.iter().map(|row| row.id).max().unwrap_or(0);
    let mut report = FoldRows::default();
    // Older merges could leave one id on two rows of a file; a later one of
    // them takes a fresh id so every row is addressable again.
    let mut seen = BTreeSet::new();
    let mut rows = Vec::with_capacity(into.len() + from.len());
    for mut row in into {
        if !seen.insert(row.id) {
            highest = highest.checked_add(1).ok_or(LinkFolderError::IdSpace)?;
            row.id = highest;
            report.renumbered += 1;
        }
        rows.push(row);
    }
    for mut row in from {
        if copies.contains(&copy_key(&row)) {
            report.copies_dropped += 1;
            continue;
        }
        if row.id > highest {
            highest = row.id;
        } else {
            highest = highest.checked_add(1).ok_or(LinkFolderError::IdSpace)?;
            row.id = highest;
            report.renumbered += 1;
        }
        report.added += 1;
        rows.push(row);
    }
    Ok((rows, report))
}

/// Whether `name` can be one folder under `entities/`: not empty, no path
/// separator, and not a dot name (dot names are scratch space, never entities).
pub fn is_folder_name(name: &str) -> bool {
    !name.is_empty() && !name.starts_with('.') && !name.contains(['/', '\\', '\0'])
}

fn check_name(name: &str) -> Result<(), LinkFolderError> {
    if is_folder_name(name) {
        Ok(())
    } else {
        Err(LinkFolderError::InvalidName {
            name: name.to_owned(),
        })
    }
}

fn copy_key(row: &ObservationRow) -> String {
    serde_json::to_string(&(
        &row.content,
        row.observed_at,
        &row.source_day,
        &row.relation,
    ))
    .expect("observation key serializes")
}

/// The link folders of one facet's `entities/` directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkDirs {
    root: PathBuf,
    entities: String,
}

impl LinkDirs {
    /// The link folders of `facet` in `journal`.
    pub fn for_facet(journal: &Path, facet: &str) -> Self {
        Self {
            root: journal.to_path_buf(),
            entities: format!("facets/{facet}/entities"),
        }
    }

    /// The link folders of an `entities/` directory at `entities` below `root`.
    pub fn at(root: &Path, entities: &str) -> Self {
        Self {
            root: root.to_path_buf(),
            entities: entities.to_owned(),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// `entities/` relative to the root.
    pub fn entities_rel(&self) -> &str {
        &self.entities
    }

    /// A folder's path relative to the root.
    pub fn folder_rel(&self, dir: &str) -> String {
        format!("{}/{dir}", self.entities)
    }

    /// A folder's link file relative to the root.
    pub fn link_rel(&self, dir: &str) -> String {
        format!("{}/{LINK_FILE}", self.folder_rel(dir))
    }

    /// A folder's notes file relative to the root.
    pub fn observations_rel(&self, dir: &str) -> String {
        format!("{}/{OBSERVATIONS_FILE}", self.folder_rel(dir))
    }

    /// Remove a folder and everything in it; an absent folder is removed.
    pub fn remove_folder(&self, dir: &str) -> Result<(), PathError> {
        remove_dir_all(&self.root, &self.folder_rel(dir))
    }

    pub fn folder_path(&self, dir: &str) -> Result<PathBuf, PathError> {
        contained_path(&self.root, &self.folder_rel(dir))
    }

    pub fn link_path(&self, dir: &str) -> Result<PathBuf, PathError> {
        contained_path(&self.root, &format!("{}/{LINK_FILE}", self.folder_rel(dir)))
    }

    pub fn observations_path(&self, dir: &str) -> Result<PathBuf, PathError> {
        contained_path(
            &self.root,
            &format!("{}/{OBSERVATIONS_FILE}", self.folder_rel(dir)),
        )
    }

    /// Every directory under `entities/`, link or orphan, in name order.
    pub fn all_folders(&self) -> Result<Vec<String>, PathError> {
        let entities = contained_path(&self.root, &self.entities)?;
        if !path_lexists(&entities)? {
            return Ok(Vec::new());
        }
        Ok(list_dir_entries(&entities)?
            .into_iter()
            .filter(|entry| entry.kind == DirEntryKind::Directory)
            .filter_map(|entry| entry.name.to_str().map(str::to_owned))
            .collect())
    }

    /// Every folder holding `entity.json`, readable or not, in name order. The
    /// file itself is not followed, so a link that leads nowhere is listed too.
    pub fn link_folders(&self) -> Result<Vec<String>, PathError> {
        let mut folders = Vec::new();
        for dir in self.all_folders()? {
            if path_lexists(&self.folder_path(&dir)?.join(LINK_FILE))? {
                folders.push(dir);
            }
        }
        Ok(folders)
    }

    /// Read one folder's link strictly: `None` when it has no `entity.json`,
    /// an error when that file is malformed.
    pub fn read_link(&self, dir: &str) -> Result<Option<LinkEntry>, LinkFolderError> {
        // An `entity.json` that leads nowhere or out of the journal is never
        // followed; it reads as a link that can't be read.
        let path = match self.link_path(dir) {
            Ok(path) => path,
            Err(error) => {
                let file = self.folder_path(dir)?.join(LINK_FILE);
                if !path_lexists(&file)? {
                    return Err(error.into());
                }
                return Err(LinkFolderError::Read(ReadError::Io {
                    path: file,
                    source: std::io::Error::other(error.to_string()),
                }));
            }
        };
        let value: Value = read_json(&path, Value::Null, MalformedPolicy::Raise)?;
        if value.is_null() {
            return Ok(None);
        }
        let Some(object) = value.as_object() else {
            return Err(LinkFolderError::NotObject { path });
        };
        let stored = object
            .get("entity_id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .map(str::to_owned);
        Ok(Some(LinkEntry {
            dir: dir.to_owned(),
            entity_id: stored.clone().unwrap_or_else(|| dir.to_owned()),
            id_written: stored.is_some(),
            value,
        }))
    }

    /// What holds the folder name `dir`.
    pub fn state(&self, dir: &str) -> Result<FolderState, LinkFolderError> {
        let folder = self.folder_path(dir)?;
        if !path_lexists(&folder)? {
            return Ok(FolderState::Absent);
        }
        if !folder.is_dir() {
            return Ok(FolderState::NotADirectory);
        }
        if !path_lexists(&folder.join(LINK_FILE))? {
            return Ok(FolderState::Orphan);
        }
        match self.read_link(dir) {
            Ok(Some(link)) => Ok(FolderState::Link(link)),
            Ok(None) => Ok(FolderState::Orphan),
            Err(LinkFolderError::Path(error)) => Err(LinkFolderError::Path(error)),
            Err(_) => Ok(FolderState::Unreadable),
        }
    }

    /// Every readable link, in name order, and the folders whose link could
    /// not be read. An unreadable link is never matched to any entity.
    pub fn scan(&self) -> Result<(Vec<LinkEntry>, Vec<String>), LinkFolderError> {
        let mut links = Vec::new();
        let mut unreadable = Vec::new();
        for dir in self.link_folders()? {
            match self.read_link(&dir) {
                Ok(Some(link)) if is_folder_name(&link.entity_id) => links.push(link),
                Ok(Some(_)) => unreadable.push(dir),
                Ok(None) => {}
                Err(LinkFolderError::Path(error)) => return Err(LinkFolderError::Path(error)),
                Err(_) => unreadable.push(dir),
            }
        }
        Ok((links, unreadable))
    }

    /// Every readable link naming `entity_id`: the folder named by the id
    /// first, then name order.
    pub fn folders_for(&self, entity_id: &str) -> Result<Vec<LinkEntry>, LinkFolderError> {
        let (links, _) = self.scan()?;
        let mut matching: Vec<LinkEntry> = links
            .into_iter()
            .filter(|link| link.entity_id == entity_id)
            .collect();
        matching.sort_by_key(|link| link.dir != entity_id);
        Ok(matching)
    }

    /// Like `folders_for`, but an unreadable link anywhere in the facet is an
    /// error, never an absence: for operations that must not miss a link.
    pub fn folders_for_strict(&self, entity_id: &str) -> Result<Vec<LinkEntry>, LinkFolderError> {
        let mut matching = Vec::new();
        for dir in self.link_folders()? {
            if let Some(link) = self.read_link(&dir)?
                && link.entity_id == entity_id
            {
                matching.push(link);
            }
        }
        matching.sort_by_key(|link| link.dir != entity_id);
        Ok(matching)
    }

    /// The link for `entity_id`, preferring the folder named by the id.
    pub fn find(&self, entity_id: &str) -> Result<Option<LinkEntry>, LinkFolderError> {
        Ok(self.folders_for(entity_id)?.into_iter().next())
    }

    /// Refuse unless folder `entity_id` is free, an orphan, or already links
    /// `entity_id`.
    fn require_placeable(&self, entity_id: &str) -> Result<FolderState, LinkFolderError> {
        let state = self.state(entity_id)?;
        match &state {
            FolderState::Absent | FolderState::Orphan => Ok(state),
            FolderState::Link(link) if link.entity_id == entity_id => Ok(state),
            _ => Err(self.needs_repair(entity_id)),
        }
    }

    /// Check, writing nothing, that a new link for `entity_id` could be put in
    /// the folder named by its id.
    pub fn check_placeable(&self, entity_id: &str) -> Result<(), LinkFolderError> {
        check_name(entity_id)?;
        if self.require_placeable(entity_id)? == FolderState::Orphan {
            // The unlinked folder there must be one a link can be put into.
            self.plan(
                entity_id,
                Some(entity_id),
                &[Participant::merge(self, entity_id)],
            )?;
        }
        Ok(())
    }

    fn needs_repair(&self, entity_id: &str) -> LinkFolderError {
        LinkFolderError::NeedsRepair {
            entities: self.entities.clone(),
            entity_id: entity_id.to_owned(),
        }
    }

    /// The folders that, with `incoming`, become `entity_id`'s one folder here,
    /// the one it will be named by first. Refuses when folder `entity_id` holds
    /// something that isn't one of them.
    fn entity_participants<'a>(
        &'a self,
        entity_id: &str,
        incoming: &[(&'a LinkDirs, &str)],
        policy: LinkFieldPolicy,
    ) -> Result<Vec<Participant<'a>>, LinkFolderError> {
        check_name(entity_id)?;
        let own = self.folders_for(entity_id)?;
        let mut participants = Vec::new();
        for link in &own {
            participants.push(Participant::merge(self, &link.dir));
        }
        match self.state(entity_id)? {
            FolderState::Absent => {}
            FolderState::Link(link) if link.entity_id == entity_id => {}
            FolderState::Orphan => participants.push(Participant::merge(self, entity_id)),
            // Folder `entity_id` may already be one of the folders coming in:
            // a link a merge is about to relink, left there by older builds.
            FolderState::Link(_)
                if incoming
                    .iter()
                    .any(|(dirs, dir)| *dirs == self && *dir == entity_id) => {}
            _ => return Err(self.needs_repair(entity_id)),
        }
        for (dirs, dir) in incoming {
            if participants
                .iter()
                .any(|existing| existing.dirs == *dirs && existing.dir == *dir)
            {
                continue;
            }
            participants.push(Participant {
                dirs,
                dir: (*dir).to_owned(),
                policy,
                remove: false,
                incoming: true,
            });
        }
        // The entity's own links lead, so their fields and note ids stand;
        // the result is always written to folder `entity_id`, whichever of
        // these it already is.
        Ok(participants)
    }

    /// Leave `entity_id` with at most one link folder here, named by its id.
    ///
    /// Every folder that links it, and an unlinked folder of that name, become
    /// one. Nothing is written unless the whole result can be built: a folder
    /// named by the id that holds another entity or can't be read, notes that
    /// don't parse where two files combine, or one file name carried with
    /// different bytes, each refuse first. Returns the folder, or `None` when
    /// the entity has no link here.
    pub fn settle(
        &self,
        entity_id: &str,
        before_write: BeforeWrite<'_>,
    ) -> Result<(Option<String>, FoldRows), LinkFolderError> {
        let own = self.folders_for(entity_id)?;
        if own.is_empty() {
            check_name(entity_id)?;
            return Ok((None, FoldRows::default()));
        }
        if own.len() == 1 && own[0].dir == entity_id {
            return Ok((Some(entity_id.to_owned()), FoldRows::default()));
        }
        let participants = self.entity_participants(entity_id, &[], LinkFieldPolicy::Merge)?;
        let plan = self.plan(entity_id, Some(entity_id), &participants)?;
        let rows = plan.rows;
        self.apply(plan, before_write)?;
        Ok((Some(entity_id.to_owned()), rows))
    }

    /// Bring folder `incoming_dir` of `incoming` in as `entity_id`'s link, in
    /// the folder named by its id, together with any folders here that link
    /// it. All-or-nothing as `settle` is. The incoming folder is left in place
    /// unless it is the folder named by the id; removing it is the caller's.
    pub fn take_in(
        &self,
        entity_id: &str,
        incoming: &LinkDirs,
        incoming_dir: &str,
        policy: LinkFieldPolicy,
        before_write: BeforeWrite<'_>,
    ) -> Result<(String, FoldRows), LinkFolderError> {
        self.take_in_all(entity_id, &[(incoming, incoming_dir)], policy, before_write)
    }

    /// `take_in` for several incoming folders at once.
    pub fn take_in_all(
        &self,
        entity_id: &str,
        incoming: &[(&LinkDirs, &str)],
        policy: LinkFieldPolicy,
        before_write: BeforeWrite<'_>,
    ) -> Result<(String, FoldRows), LinkFolderError> {
        let participants = self.entity_participants(entity_id, incoming, policy)?;
        let plan = self.plan(entity_id, Some(entity_id), &participants)?;
        let rows = plan.rows;
        self.apply(plan, before_write)?;
        Ok((entity_id.to_owned(), rows))
    }

    /// Check, writing nothing, that `take_in_all` would accept these folders.
    pub fn check_take_in_all(
        &self,
        entity_id: &str,
        incoming: &[(&LinkDirs, &str)],
    ) -> Result<(), LinkFolderError> {
        let participants = self.entity_participants(entity_id, incoming, LinkFieldPolicy::Merge)?;
        self.plan(entity_id, Some(entity_id), &participants)
            .map(|_| ())
    }

    /// Check, writing nothing, that `take_in` would accept this folder.
    pub fn check_take_in(
        &self,
        entity_id: &str,
        incoming: &LinkDirs,
        incoming_dir: &str,
    ) -> Result<(), LinkFolderError> {
        self.check_take_in_all(entity_id, &[(incoming, incoming_dir)])
    }

    /// Create the link for an entity with none here, in the folder named by
    /// its id, and bring in the notes of an unlinked folder of that name and,
    /// when they can be combined, of `name_folder`. Returns the folder and the
    /// rows brought in.
    pub fn place(
        &self,
        entity_id: &str,
        name_folder: Option<&str>,
        link: &Map<String, Value>,
        before_write: BeforeWrite<'_>,
    ) -> Result<(String, FoldRows), LinkFolderError> {
        check_name(entity_id)?;
        match self.require_placeable(entity_id)? {
            FolderState::Link(existing) => return Ok((existing.dir, FoldRows::default())),
            FolderState::Absent | FolderState::Orphan => {}
            _ => return Err(self.needs_repair(entity_id)),
        }
        let mut participants = Vec::new();
        if self.state(entity_id)? == FolderState::Orphan {
            participants.push(Participant::merge(self, entity_id));
        }
        let mut plan = None;
        if let Some(name_folder) = name_folder.filter(|name| {
            is_folder_name(name)
                && *name != entity_id
                && matches!(self.state(name), Ok(FolderState::Orphan))
        }) {
            let mut with_name = participants.clone();
            with_name.push(Participant {
                dirs: self,
                dir: name_folder.to_owned(),
                policy: LinkFieldPolicy::TargetWins,
                remove: true,
                incoming: true,
            });
            // Notes under the name come along when they can; a folder that
            // can't be combined stays as it is and never blocks the link.
            plan = self.plan(entity_id, Some(entity_id), &with_name).ok();
        }
        let mut plan = match plan {
            Some(plan) => plan,
            None => self.plan(entity_id, Some(entity_id), &participants)?,
        };
        let mut object = link.clone();
        object.insert("entity_id".to_owned(), Value::String(entity_id.to_owned()));
        plan.link = LinkFile::Object(object);
        let rows = plan.rows;
        self.apply(plan, before_write)?;
        Ok((entity_id.to_owned(), rows))
    }

    /// Bring the notes of unlinked folder `orphan_dir` into `entity_id`'s
    /// link, which must be the folder named by its id, and remove the orphan.
    /// A no-op when `orphan_dir` is not an unlinked folder.
    pub fn adopt_orphan(
        &self,
        entity_id: &str,
        orphan_dir: &str,
        before_write: BeforeWrite<'_>,
    ) -> Result<FoldRows, LinkFolderError> {
        check_name(entity_id)?;
        if orphan_dir == entity_id
            || !is_folder_name(orphan_dir)
            || self.state(orphan_dir)? != FolderState::Orphan
        {
            return Ok(FoldRows::default());
        }
        match self.state(entity_id)? {
            FolderState::Link(link) if link.entity_id == entity_id => {}
            _ => return Err(self.needs_repair(entity_id)),
        }
        let participants = vec![
            Participant::merge(self, entity_id),
            Participant {
                dirs: self,
                dir: orphan_dir.to_owned(),
                policy: LinkFieldPolicy::TargetWins,
                remove: true,
                incoming: true,
            },
        ];
        let plan = self.plan(entity_id, Some(entity_id), &participants)?;
        let rows = plan.rows;
        self.apply(plan, before_write)?;
        Ok(rows)
    }

    /// Combine folder `from_dir` of `from` into this folder `into_dir` as it
    /// is, relinking nothing: the way notes under an unlinked name combine.
    /// The incoming folder is left in place.
    pub fn fold_into(
        &self,
        from: &LinkDirs,
        from_dir: &str,
        into_dir: &str,
        policy: LinkFieldPolicy,
        before_write: BeforeWrite<'_>,
    ) -> Result<FoldRows, LinkFolderError> {
        check_name(into_dir)?;
        let mut participants = Vec::new();
        if self.state(into_dir)? != FolderState::Absent {
            participants.push(Participant::merge(self, into_dir));
        }
        participants.push(Participant {
            dirs: from,
            dir: from_dir.to_owned(),
            policy,
            remove: false,
            incoming: true,
        });
        let plan = self.plan(into_dir, None, &participants)?;
        let rows = plan.rows;
        self.apply(plan, before_write)?;
        Ok(rows)
    }

    /// Build, reading only, what folder `folder` holds once `participants`
    /// (the first one receiving) become one. Every refusal happens here.
    fn plan(
        &self,
        folder: &str,
        relink: Option<&str>,
        participants: &[Participant<'_>],
    ) -> Result<FoldPlan, LinkFolderError> {
        let mut rows_report = FoldRows::default();
        let mut link = LinkFile::Missing;
        // Each notes file, and whether it was brought in.
        let mut notes: Vec<(PathBuf, Vec<u8>, bool)> = Vec::new();
        let mut extra: std::collections::BTreeMap<String, Vec<u8>> =
            std::collections::BTreeMap::new();
        for participant in participants {
            let dirs = participant.dirs;
            let dir = &participant.dir;
            match read_link_file(&dirs.link_path(dir)?)? {
                LinkFile::Object(object) => match &mut link {
                    LinkFile::Object(current) => {
                        if participant.policy == LinkFieldPolicy::Merge {
                            rows_report.fields_kept += merge_link_fields(current, &object);
                        }
                    }
                    _ => link = LinkFile::Object(object),
                },
                LinkFile::Null => {
                    if matches!(link, LinkFile::Missing) {
                        link = LinkFile::Null;
                    }
                }
                LinkFile::Missing => {}
            }
            let observations = dirs.observations_path(dir)?;
            if path_lexists(&observations)? {
                let bytes = read_bytes(&observations, Vec::new())?;
                // A byte-identical copy (a fold interrupted after writing its
                // result) adds nothing and is never parsed twice.
                if !notes.iter().any(|(_, seen, _)| *seen == bytes) {
                    notes.push((observations.clone(), bytes, participant.incoming));
                }
            }
            for name in extra_files(dirs, dir)? {
                let bytes = read_bytes(dirs.folder_path(dir)?.join(&name), Vec::new())?;
                match extra.get(&name) {
                    Some(seen) if *seen != bytes => {
                        return Err(LinkFolderError::Conflict {
                            path: PathBuf::from(dirs.folder_rel(dir)).join(name),
                        });
                    }
                    Some(_) => {}
                    None => {
                        extra.insert(name, bytes);
                    }
                }
            }
        }
        if let (Some(entity_id), LinkFile::Object(object)) = (relink, &mut link) {
            object.insert("entity_id".to_owned(), Value::String(entity_id.to_owned()));
        }
        // One notes file moves exactly as written; two or more combine.
        let notes = match notes.len() {
            0 => None,
            1 => {
                let (_, bytes, incoming) = notes.remove(0);
                if incoming {
                    rows_report.added += count_lines(&bytes);
                }
                Some(bytes)
            }
            _ => {
                let mut combined: Option<Vec<ObservationRow>> = None;
                for (path, bytes, incoming) in &notes {
                    let text = std::str::from_utf8(bytes).map_err(|_| {
                        LinkFolderError::Resolve(format!("{} is not UTF-8 text", path.display()))
                    })?;
                    let rows =
                        parse_observation_file(text, ObservationParseSource::Path(path))?.full_rows;
                    combined = Some(match combined {
                        None => {
                            if *incoming {
                                rows_report.added += rows.len();
                            }
                            rows
                        }
                        Some(into) => {
                            let (folded, report) = fold_observation_rows(into, rows)?;
                            rows_report.absorb(FoldRows {
                                fields_kept: 0,
                                added: if *incoming { report.added } else { 0 },
                                ..report
                            });
                            folded
                        }
                    });
                }
                Some(serialize_observation_rows(&combined.unwrap_or_default()).into_bytes())
            }
        };
        let remove = participants
            .iter()
            .filter(|participant| {
                participant.dirs == self && participant.dir != folder && participant.remove
            })
            .map(|participant| participant.dir.clone())
            .collect();
        Ok(FoldPlan {
            folder: folder.to_owned(),
            link,
            link_source: participants.iter().find_map(|participant| {
                participant
                    .dirs
                    .link_path(&participant.dir)
                    .ok()
                    .filter(|path| path.exists())
            }),
            notes,
            extra,
            remove,
            rows: rows_report,
        })
    }

    /// Write a plan: folder `plan.folder` gets its complete result first, and
    /// only then are the folders it replaced removed, so a crash in between
    /// leaves a duplicate for the next settle, never a loss.
    fn apply(&self, plan: FoldPlan, before_write: BeforeWrite<'_>) -> Result<(), LinkFolderError> {
        before_write(&self.folder_rel(&plan.folder))?;
        for dir in &plan.remove {
            before_write(&self.folder_rel(dir))?;
        }
        let link_path = self.link_path(&plan.folder)?;
        match &plan.link {
            LinkFile::Object(object) => {
                if !matches!(read_link_file(&link_path)?, LinkFile::Object(existing) if existing == *object)
                {
                    write_link(&link_path, object)?;
                }
            }
            LinkFile::Null => {
                if !path_lexists(&link_path)?
                    && let Some(source) = &plan.link_source
                {
                    let bytes = read_bytes(source, Vec::new())?;
                    write_bytes_exclusive(
                        &link_path,
                        &bytes,
                        AtomicWriteOptions { mode: Some(0o600) },
                    )?;
                }
            }
            LinkFile::Missing => {}
        }
        if let Some(bytes) = &plan.notes {
            let path = self.observations_path(&plan.folder)?;
            if !path_lexists(&path)? || read_bytes(&path, Vec::new())? != *bytes {
                solstone_core_journal_io::atomic_replace(
                    &path,
                    bytes,
                    AtomicWriteOptions::default(),
                )?;
            }
        }
        for (name, bytes) in &plan.extra {
            let path = self.folder_path(&plan.folder)?.join(name);
            if !path_lexists(&path)? {
                write_bytes_exclusive(&path, bytes, AtomicWriteOptions { mode: Some(0o600) })?;
            }
        }
        let result = self.folder_path(&plan.folder)?;
        for dir in &plan.remove {
            // A folder that is the result under another spelling (a letter
            // case the filesystem ignores) is never removed.
            if solstone_core_journal_io::same_directory(&self.folder_path(dir)?, &result)? {
                continue;
            }
            self.remove_folder(dir)?;
        }
        Ok(())
    }
}

/// One folder taking part in a fold.
#[derive(Clone)]
struct Participant<'a> {
    dirs: &'a LinkDirs,
    dir: String,
    policy: LinkFieldPolicy,
    /// Removed once the fold is written (only ever a folder of the receiving
    /// `LinkDirs`).
    remove: bool,
    /// Brought in from elsewhere: its surviving notes count as added.
    incoming: bool,
}

impl<'a> Participant<'a> {
    fn merge(dirs: &'a LinkDirs, dir: &str) -> Self {
        Self {
            dirs,
            dir: dir.to_owned(),
            policy: LinkFieldPolicy::Merge,
            remove: true,
            incoming: false,
        }
    }
}

/// What one folder holds once a fold is written.
struct FoldPlan {
    folder: String,
    link: LinkFile,
    /// Where a `null` link file comes from, when that is all there is.
    link_source: Option<PathBuf>,
    notes: Option<Vec<u8>>,
    extra: std::collections::BTreeMap<String, Vec<u8>>,
    remove: Vec<String>,
    rows: FoldRows,
}

/// One scalar rule for combining two links of the same entity. Returns how
/// many incoming values lost to a different value the receiving link keeps.
pub fn merge_link_fields(into: &mut Map<String, Value>, from: &Map<String, Value>) -> usize {
    let both_detached = into.get("detached") == Some(&Value::Bool(true))
        && from.get("detached") == Some(&Value::Bool(true));
    let mut kept = 0;
    for (key, value) in from {
        if key == "entity_id" || key == "detached" || is_blank(Some(value)) {
            continue;
        }
        let replace = match key.as_str() {
            "attached_at" => {
                is_blank(into.get(key)) || replaces(value, into.get(key), Ordering::Less)
            }
            "updated_at" => {
                is_blank(into.get(key)) || replaces(value, into.get(key), Ordering::Greater)
            }
            "last_seen" => {
                is_blank(into.get(key)) || value.as_str() > into.get(key).and_then(Value::as_str)
            }
            _ => is_blank(into.get(key)),
        };
        if replace {
            into.insert(key.clone(), value.clone());
        } else if !matches!(key.as_str(), "attached_at" | "updated_at" | "last_seen")
            && into.get(key) != Some(value)
        {
            kept += 1;
        }
    }
    if both_detached {
        into.insert("detached".to_owned(), Value::Bool(true));
    } else {
        into.remove("detached");
    }
    kept
}

/// Whether an incoming link time replaces the receiving one: it must compare
/// as `wanted` to it. Times compare as instants, whether stored as epoch
/// milliseconds or RFC 3339 text, and a readable time beats an unreadable
/// one. Two values that are not times compare as text.
fn replaces(incoming: &Value, current: Option<&Value>, wanted: Ordering) -> bool {
    match (timestamp_ms(Some(incoming)), timestamp_ms(current)) {
        (Some(incoming), Some(current)) => incoming.cmp(&current) == wanted,
        (Some(_), None) => true,
        (None, Some(_)) => false,
        (None, None) => match (incoming.as_str(), current.and_then(Value::as_str)) {
            (Some(incoming), Some(current)) => incoming.cmp(current) == wanted,
            _ => false,
        },
    }
}

fn is_blank(value: Option<&Value>) -> bool {
    value.is_none_or(|value| {
        value.is_null()
            || value == ""
            || value == &Value::Array(Vec::new())
            || value == &Value::Object(Map::new())
    })
}

/// Files in a folder other than the link, the notes, and lock sentinels.
fn count_lines(bytes: &[u8]) -> usize {
    bytes
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.iter().all(u8::is_ascii_whitespace))
        .count()
}

fn extra_files(dirs: &LinkDirs, dir: &str) -> Result<Vec<String>, LinkFolderError> {
    let folder = dirs.folder_path(dir)?;
    if !path_lexists(&folder)? {
        return Ok(Vec::new());
    }
    let mut names = Vec::new();
    for entry in list_dir_entries(&folder)? {
        let Some(name) = entry.name.to_str() else {
            continue;
        };
        // Locks are sentinels, and desktop litter (`.DS_Store`, `Thumbs.db`,
        // `desktop.ini`) is the file manager's, not the journal's.
        if name == LINK_FILE
            || name == OBSERVATIONS_FILE
            || name.ends_with(".lock")
            || name == ".DS_Store"
            || name.starts_with("._")
            || solstone_core_journal_io::is_publication_candidate_name(std::ffi::OsStr::new(name))
            || name.eq_ignore_ascii_case("thumbs.db")
            || name.eq_ignore_ascii_case("desktop.ini")
        {
            continue;
        }
        if entry.kind != DirEntryKind::File {
            return Err(LinkFolderError::NotAFile {
                path: PathBuf::from(dirs.folder_rel(dir)).join(name),
            });
        }
        names.push(name.to_owned());
    }
    Ok(names)
}

#[derive(Clone)]
enum LinkFile {
    Missing,
    /// The file holds JSON `null`: present, naming no entity.
    Null,
    Object(Map<String, Value>),
}

fn read_link_file(path: &Path) -> Result<LinkFile, LinkFolderError> {
    if !path_lexists(path)? {
        return Ok(LinkFile::Missing);
    }
    match read_json::<Value>(path, Value::Null, MalformedPolicy::Raise)? {
        Value::Object(object) => Ok(LinkFile::Object(object)),
        Value::Null => Ok(LinkFile::Null),
        _ => Err(LinkFolderError::NotObject {
            path: path.to_path_buf(),
        }),
    }
}

fn write_link(path: &Path, link: &Map<String, Value>) -> Result<(), LinkFolderError> {
    write_json(
        path,
        &Value::Object(link.clone()),
        JsonWriteOptions {
            indent: Some(2),
            sort_keys: false,
            mode: None,
        },
    )?;
    Ok(())
}

/// The folder of notes written under `name` in `facet` before the entity it
/// names was linked there, when those notes can only be about `entity_id`: it
/// is an unlinked folder, the name's slug is not the entity's id, and no other
/// entity -- live, merged away or deleted -- answers to that slug. Otherwise
/// `None`, and such a folder is left alone.
pub fn adoptable_name_folder(
    journal: &Path,
    facet: &str,
    entity_id: &str,
    name: &str,
) -> Result<Option<String>, LinkFolderError> {
    let slug = solstone_core_entity_matching::entity_slug(name);
    if slug == entity_id || !is_folder_name(&slug) {
        return Ok(None);
    }
    // Most names have no such folder; only one that does pays for the checks.
    if LinkDirs::for_facet(journal, facet).state(&slug)? != FolderState::Orphan {
        return Ok(None);
    }
    let live = super::map::read_identity_map(journal)
        .map_err(|error| LinkFolderError::Resolve(error.to_string()))?;
    if live.resolved.contains_key(&slug)
        || super::identity::read_entity_identity(journal, &slug)
            .map_err(|error| LinkFolderError::Resolve(error.to_string()))?
            .is_some()
    {
        return Ok(None);
    }
    if super::retired::retired_state(journal, &slug)
        .map_err(LinkFolderError::Resolve)?
        .is_some()
    {
        return Ok(None);
    }
    Ok(Some(slug))
}

/// What keeps an entity's links in one facet from being one folder named by
/// its id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkIssueKind {
    /// More than one folder links the entity.
    Duplicate,
    /// Its one folder is named differently from its id.
    Misnamed,
    /// The linked id was merged away; its links belong with `successor`.
    Merged { successor: String },
    /// The linked id was deleted.
    Deleted,
    /// The link's `entity.json` can't be read.
    Unreadable,
}

/// One facet's link problem for one entity (or, when unreadable, one folder).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkIssue {
    pub facet: String,
    pub entity_id: String,
    pub folders: Vec<String>,
    pub kind: LinkIssueKind,
    /// Why a repair left it as it is, when one was tried and failed.
    pub note: Option<String>,
    /// Whether that repair failed with an error, not merely left it.
    pub failed: bool,
}

/// Every link folder problem in the journal, facet by facet. Changes nothing.
pub fn check_journal_links(journal: &Path) -> Result<Vec<LinkIssue>, LinkFolderError> {
    let mut issues = Vec::new();
    for facet in journal_facets(journal)? {
        let dirs = LinkDirs::for_facet(journal, &facet);
        let (links, unreadable) = dirs.scan()?;
        for dir in unreadable {
            issues.push(LinkIssue {
                facet: facet.clone(),
                entity_id: String::new(),
                folders: vec![dir],
                kind: LinkIssueKind::Unreadable,
                note: None,
                failed: false,
            });
        }
        let mut by_entity: std::collections::BTreeMap<String, Vec<String>> =
            std::collections::BTreeMap::new();
        for link in links {
            by_entity.entry(link.entity_id).or_default().push(link.dir);
        }
        for (entity_id, folders) in by_entity {
            let kind = match super::retired::retired_state(journal, &entity_id) {
                Ok(Some(super::retired::RetiredState::Merged { successor })) => {
                    LinkIssueKind::Merged { successor }
                }
                Ok(Some(super::retired::RetiredState::Deleted)) => LinkIssueKind::Deleted,
                _ if folders.len() > 1 => LinkIssueKind::Duplicate,
                _ if folders[0] != entity_id => LinkIssueKind::Misnamed,
                _ => continue,
            };
            issues.push(LinkIssue {
                facet: facet.clone(),
                entity_id,
                folders,
                kind,
                note: None,
                failed: false,
            });
        }
    }
    Ok(issues)
}

/// Repair what can be repaired without a judgment call, under the facet trust
/// lock: an entity's duplicate or misnamed folders become one folder named by
/// its id, and links to a merged-away id join the live entity it was merged
/// into. A folder held by another entity, a deleted id and an unreadable link
/// are left as they are. Returns what was repaired and what is left.
pub fn repair_journal_links(
    journal: &Path,
) -> Result<(Vec<LinkIssue>, Vec<LinkIssue>), LinkFolderError> {
    let _trust = crate::trust_lock::hold_facet_trust_lock(journal)
        .map_err(|error| LinkFolderError::Resolve(error.to_string()))?;
    let mut repaired = Vec::new();
    let mut failed: std::collections::BTreeMap<(String, String, Vec<String>), (String, bool)> =
        std::collections::BTreeMap::new();
    // Each pass can free a folder another entity needs, so repeat until a
    // pass changes nothing; every productive pass removes at least one
    // folder, so the bound is never what ends it.
    for _ in 0..=check_journal_links(journal)?.len() {
        let mut progress = false;
        for issue in check_journal_links(journal)? {
            let dirs = LinkDirs::for_facet(journal, &issue.facet);
            let attempt: Result<bool, LinkFolderError> = match &issue.kind {
                LinkIssueKind::Duplicate | LinkIssueKind::Misnamed => {
                    dirs.settle(&issue.entity_id, &mut |_| Ok(())).map(|_| true)
                }
                LinkIssueKind::Merged { successor } => {
                    match super::retired::live_merge_successor(journal, successor) {
                        Some(live) => {
                            let incoming: Vec<(&LinkDirs, &str)> = issue
                                .folders
                                .iter()
                                .map(|folder| (&dirs, folder.as_str()))
                                .collect();
                            dirs.take_in_all(&live, &incoming, LinkFieldPolicy::Merge, &mut |_| {
                                Ok(())
                            })
                            .and_then(|_| {
                                // A folder already named by the successor was
                                // relinked in place: it is the result.
                                for folder in issue.folders.iter().filter(|folder| **folder != live)
                                {
                                    dirs.remove_folder(folder)?;
                                }
                                Ok(true)
                            })
                        }
                        None => Ok(false),
                    }
                }
                LinkIssueKind::Deleted | LinkIssueKind::Unreadable => Ok(false),
            };
            let key = (
                issue.facet.clone(),
                issue.entity_id.clone(),
                issue.folders.clone(),
            );
            match attempt {
                Ok(true) => {
                    progress = true;
                    failed.remove(&key);
                    repaired.push(issue);
                }
                Ok(false) => {
                    if let LinkIssueKind::Merged { .. } = issue.kind {
                        failed.insert(
                            key,
                            ("the entity it joined can't be found".to_owned(), false),
                        );
                    }
                }
                Err(LinkFolderError::NeedsRepair { entity_id, .. }) => {
                    failed.insert(
                        key,
                        (
                            format!("folder '{entity_id}' holds another entity or can't be read"),
                            false,
                        ),
                    );
                }
                // One folder that can't be repaired never stops the rest.
                Err(error) => {
                    failed.insert(key, (error.to_string(), true));
                }
            }
        }
        if !progress {
            break;
        }
    }
    let left = check_journal_links(journal)?
        .into_iter()
        .map(|mut issue| {
            if let Some((note, errored)) = failed.get(&(
                issue.facet.clone(),
                issue.entity_id.clone(),
                issue.folders.clone(),
            )) {
                issue.note = Some(note.clone());
                issue.failed = *errored;
            }
            issue
        })
        .collect();
    Ok((repaired, left))
}

fn journal_facets(journal: &Path) -> Result<Vec<String>, LinkFolderError> {
    let facets = contained_path(journal, "facets")?;
    if !path_lexists(&facets)? {
        return Ok(Vec::new());
    }
    Ok(list_dir_entries(&facets)?
        .into_iter()
        .filter(|entry| entry.kind == DirEntryKind::Directory)
        .filter_map(|entry| entry.name.to_str().map(str::to_owned))
        .filter(|name| !name.starts_with('.'))
        .collect())
}
