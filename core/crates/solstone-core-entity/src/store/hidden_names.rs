// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The names an owner hid in a facet, and what a facet keeps hidden.
//!
//! A facet's hidden names live in `facets/<facet>/hidden-names.json`:
//!
//! ```json
//! {"names": [{"name": "Bob Smith", "hidden_at": "2026-11-11T07:00:00Z"}]}
//! ```
//!
//! `name` is the name as the owner saw it when they hid it. A detected name is
//! hidden by an entry when the two have the same `normalize_resolution_query`
//! key, the key detection itself matches names on; the key is worked out on
//! every read and never stored. The facets store is the only writer.
//!
//! [`facet_hidden_keys`] adds the names a hidden entity keeps hidden in the
//! facet: the keys of every link folder the hide marked (`detached_by:
//! "hide"`). It reads links and entities strictly, and when something it needs
//! can't be read it answers [`HideUndecidable`] rather than guessing.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};
use solstone_core_entity_matching::{
    EntityNameCandidate, ExclusionKeys, NameProbe, normalize_resolution_query,
};
use solstone_core_journal_io::durability::{ArtifactId, DurableObservation, observe_json_durable};

use super::facet_links::{LinkDirs, is_folder_name};
use super::identity::{IdentityObservation, observe_entity_identity};

/// The file name of a facet's hidden names, in the facet's folder.
pub const HIDDEN_NAMES_FILE: &str = "hidden-names.json";

/// The `detached_by` value a hide writes on an entity's links.
pub const HIDE_DETACHED_BY: &str = "hide";

/// A facet's hidden-names file, relative to the journal.
pub fn hidden_names_rel(facet: &str) -> String {
    format!("facets/{facet}/{HIDDEN_NAMES_FILE}")
}

/// The key a hidden name is matched on.
pub fn hidden_name_key(name: &str) -> String {
    normalize_resolution_query(name)
}

/// One name the owner hid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HiddenName {
    /// The name as the owner saw it.
    pub name: String,
    /// When the owner hid it (RFC 3339).
    pub hidden_at: String,
}

impl HiddenName {
    /// The key this name is matched on.
    pub fn key(&self) -> String {
        hidden_name_key(&self.name)
    }
}

/// What a facet's hidden-names file holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HiddenNames {
    /// No file: the owner has hidden no name here.
    Absent,
    /// The file's names, in the order they were hidden.
    Present(Vec<HiddenName>),
    /// The file exists but can't be read or isn't a valid record. It is left
    /// exactly as found.
    Damaged { path: PathBuf, detail: String },
}

impl HiddenNames {
    /// The names, empty when absent; the damage when damaged.
    pub fn names(&self) -> Result<&[HiddenName], (&Path, &str)> {
        match self {
            Self::Absent => Ok(&[]),
            Self::Present(names) => Ok(names),
            Self::Damaged { path, detail } => Err((path, detail)),
        }
    }
}

/// Read a facet's hidden names without changing the file.
pub fn read_hidden_names(journal: &Path, facet: &str) -> HiddenNames {
    let rel = hidden_names_rel(facet);
    let path = match solstone_core_journal_io::contained_path(journal, &rel) {
        Ok(path) => path,
        Err(error) => {
            return HiddenNames::Damaged {
                path: journal.join(&rel),
                detail: error.to_string(),
            };
        }
    };
    match observe_json_durable::<Value>(ArtifactId::FacetHiddenNames, &path) {
        DurableObservation::Absent => HiddenNames::Absent,
        DurableObservation::Present(value) => match names_from_value(&value) {
            Ok(names) => HiddenNames::Present(names),
            Err(detail) => HiddenNames::Damaged { path, detail },
        },
        DurableObservation::Malformed { path, source } => HiddenNames::Damaged {
            path,
            detail: source.to_string(),
        },
        DurableObservation::Unreadable { path, source } => HiddenNames::Damaged {
            path,
            detail: source.to_string(),
        },
    }
}

/// Decode the bytes of a hidden-names file. Empty bytes hold no names, as an
/// absent file does; anything else must be a valid record.
pub fn decode_hidden_names(bytes: &[u8]) -> Result<Vec<HiddenName>, String> {
    if bytes.trim_ascii().is_empty() {
        return Ok(Vec::new());
    }
    let value: Value = serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
    names_from_value(&value)
}

/// Encode names as the file's bytes, in the order given.
pub fn encode_hidden_names(names: &[HiddenName]) -> Vec<u8> {
    let names: Vec<Value> = names
        .iter()
        .map(|entry| json!({"name": entry.name, "hidden_at": entry.hidden_at}))
        .collect();
    let mut bytes =
        serde_json::to_vec_pretty(&json!({ "names": names })).expect("hidden names serialize");
    bytes.push(b'\n');
    bytes
}

fn names_from_value(value: &Value) -> Result<Vec<HiddenName>, String> {
    let root = value.as_object().ok_or("not a JSON object")?;
    if let Some(key) = root.keys().find(|key| key.as_str() != "names") {
        return Err(format!("unknown field {key:?}"));
    }
    let rows = root
        .get("names")
        .and_then(Value::as_array)
        .ok_or("names is not a list")?;
    rows.iter()
        .enumerate()
        .map(|(index, row)| {
            name_from_value(row).map_err(|detail| format!("name {index}: {detail}"))
        })
        .collect()
}

fn name_from_value(value: &Value) -> Result<HiddenName, String> {
    let row = value.as_object().ok_or("not an object")?;
    if let Some(key) = row
        .keys()
        .find(|key| !matches!(key.as_str(), "name" | "hidden_at"))
    {
        return Err(format!("unknown field {key:?}"));
    }
    let text = |key: &str| {
        row.get(key)
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| format!("{key} is not text"))
    };
    let name = text("name")?;
    if name.trim().is_empty() {
        return Err("name is empty".to_owned());
    }
    Ok(HiddenName {
        name,
        hidden_at: text("hidden_at")?,
    })
}

/// Why a facet's hides can't be decided. Nothing was changed.
#[derive(Debug)]
pub enum HideUndecidable {
    /// The facet's hidden-names file is damaged.
    HiddenNames { path: PathBuf, detail: String },
    /// The facet's link folders can't be listed.
    Links { path: PathBuf, detail: String },
    /// A link folder's `entity.json` can't be read.
    Link { path: PathBuf, detail: String },
    /// A hidden entity's `entity.json` exists but can't be read.
    Entity { path: PathBuf, detail: String },
}

impl HideUndecidable {
    /// The file or folder that couldn't be read.
    pub fn path(&self) -> &Path {
        match self {
            Self::HiddenNames { path, .. }
            | Self::Links { path, .. }
            | Self::Link { path, .. }
            | Self::Entity { path, .. } => path,
        }
    }
}

impl fmt::Display for HideUndecidable {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (what, path, detail) = match self {
            Self::HiddenNames { path, detail } => ("the facet's hidden names", path, detail),
            Self::Links { path, detail } => ("the facet's entity links", path, detail),
            Self::Link { path, detail } => ("an entity link", path, detail),
            Self::Entity { path, detail } => ("a hidden entity", path, detail),
        };
        write!(
            formatter,
            "can't tell what is hidden: {what} at {} can't be read: {detail}",
            path.display()
        )
    }
}

impl std::error::Error for HideUndecidable {}

/// A hidden entity linked in a facet, and the keys it keeps hidden there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HidingEntity {
    /// The link folder's name under `facets/<facet>/entities/`.
    pub folder: String,
    /// The entity's id.
    pub id: String,
    /// The entity's name, or its id when it has none on record.
    pub name: String,
    keys: ExclusionKeys,
}

impl HidingEntity {
    /// Whether this entity keeps the detected name hidden.
    pub fn covers(&self, name: &str) -> bool {
        self.keys.contains(name)
    }
}

/// What keeps a detected name hidden in a facet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HiddenBy<'a> {
    /// The owner hid the name itself.
    Owner,
    /// A hidden entity linked in the facet matches the name.
    Entity(&'a HidingEntity),
}

/// Everything a facet keeps hidden: the owner's hidden names and the keys of
/// every hidden entity linked there.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HiddenKeys {
    owner: HashSet<String>,
    entities: Vec<HidingEntity>,
}

impl HiddenKeys {
    /// What hides the detected `name`, the owner's own hide first.
    pub fn hidden_by(&self, name: &str) -> Option<HiddenBy<'_>> {
        let probe = NameProbe::new(name);
        if self.owner.contains(probe.normalized()) {
            return Some(HiddenBy::Owner);
        }
        self.entity_probe(&probe).map(HiddenBy::Entity)
    }

    /// Whether the detected `name` is hidden in the facet, by either kind.
    pub fn is_hidden(&self, name: &str) -> bool {
        self.hidden_by(name).is_some()
    }

    /// Whether the owner hid the detected `name` itself.
    pub fn hidden_by_owner(&self, name: &str) -> bool {
        self.owner.contains(&hidden_name_key(name))
    }

    /// The first hidden entity that matches the detected `name`.
    pub fn hidden_by_entity(&self, name: &str) -> Option<&HidingEntity> {
        self.entity_probe(&NameProbe::new(name))
    }

    fn entity_probe(&self, probe: &NameProbe<'_>) -> Option<&HidingEntity> {
        self.entities
            .iter()
            .find(|entity| entity.keys.contains_probe(probe))
    }

    /// The hidden entities linked in the facet, in link-folder order.
    pub fn entities(&self) -> &[HidingEntity] {
        &self.entities
    }

    /// Whether the link folder `folder` is marked by a hide.
    pub fn hides_folder(&self, folder: &str) -> bool {
        self.entities.iter().any(|entity| entity.folder == folder)
    }

    /// Whether nothing is hidden in the facet.
    pub fn is_empty(&self) -> bool {
        self.owner.is_empty() && self.entities.is_empty()
    }
}

/// Everything `facet` keeps hidden: the names in its `hidden-names.json` and
/// the keys of each link folder a hide marked (`detached_by: "hide"`).
///
/// A hide-marked link's keys are its entity's name, aliases, emails and id,
/// read straight from `entities/<id>/entity.json`; when that file is absent the
/// link's own id and name stand in. `blocked` is not read: every hide marks
/// the entity's links.
///
/// Undecidable when the hidden-names file is damaged, when any link file in
/// the facet can't be read (it might be a hide), or when a hide-marked link's
/// entity file exists but can't be read (its keys are unknown).
pub fn facet_hidden_keys(journal: &Path, facet: &str) -> Result<HiddenKeys, HideUndecidable> {
    let owner = match read_hidden_names(journal, facet) {
        HiddenNames::Absent => HashSet::new(),
        HiddenNames::Present(names) => names.iter().map(HiddenName::key).collect(),
        HiddenNames::Damaged { path, detail } => {
            return Err(HideUndecidable::HiddenNames { path, detail });
        }
    };
    let dirs = LinkDirs::for_facet(journal, facet);
    let folders = dirs
        .link_folders()
        .map_err(|error| HideUndecidable::Links {
            path: journal.join(dirs.entities_rel()),
            detail: error.to_string(),
        })?;
    let mut entities = Vec::new();
    let mut identities: HashMap<String, Option<Map<String, Value>>> = HashMap::new();
    for folder in folders {
        let link_path = || journal.join(dirs.link_rel(&folder));
        let link = match dirs.read_link(&folder) {
            Ok(Some(link)) => link,
            Ok(None) => continue,
            Err(error) => {
                return Err(HideUndecidable::Link {
                    path: link_path(),
                    detail: error.to_string(),
                });
            }
        };
        if !is_folder_name(&link.entity_id) {
            return Err(HideUndecidable::Link {
                path: link_path(),
                detail: format!("'{}' can't be an entity id", link.entity_id),
            });
        }
        if link.value.get("detached_by").and_then(Value::as_str) != Some(HIDE_DETACHED_BY) {
            continue;
        }
        let identity = match identities.get(&link.entity_id) {
            Some(identity) => identity.clone(),
            None => {
                let identity = read_identity_strict(journal, &link.entity_id)?;
                identities.insert(link.entity_id.clone(), identity.clone());
                identity
            }
        };
        entities.push(hiding_entity(
            folder,
            &link.entity_id,
            &link.value,
            identity,
        ));
    }
    Ok(HiddenKeys { owner, entities })
}

/// An entity's identity object, `None` when there is no file, an error when
/// the file is there and can't be read.
fn read_identity_strict(
    journal: &Path,
    entity_id: &str,
) -> Result<Option<Map<String, Value>>, HideUndecidable> {
    let path = || journal.join("entities").join(entity_id).join("entity.json");
    let observation =
        observe_entity_identity(journal, entity_id).map_err(|error| HideUndecidable::Entity {
            path: path(),
            detail: error.to_string(),
        })?;
    match observation {
        IdentityObservation::Present(snapshot) => Ok(snapshot.value().as_object().cloned()),
        IdentityObservation::Absent => Ok(None),
        IdentityObservation::Malformed(detail) | IdentityObservation::Unreadable(detail) => {
            Err(HideUndecidable::Entity {
                path: path(),
                detail,
            })
        }
    }
}

fn hiding_entity(
    folder: String,
    entity_id: &str,
    link: &Value,
    identity: Option<Map<String, Value>>,
) -> HidingEntity {
    let text = |source: &Value, key: &str| {
        source
            .get(key)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    };
    let mut emails = strings(link.get("emails"));
    let (name, aka) = match &identity {
        Some(identity) => {
            let identity = Value::Object(identity.clone());
            emails.extend(strings(identity.get("emails")));
            (
                text(&identity, "name").or_else(|| text(link, "name")),
                strings(identity.get("aka")),
            )
        }
        None => (text(link, "name"), strings(link.get("aka"))),
    };
    let name = name.unwrap_or_else(|| entity_id.to_owned());
    let mut keys = ExclusionKeys::default();
    keys.add_candidate(&EntityNameCandidate {
        id: Some(entity_id.to_owned()),
        name: name.clone(),
        aka,
        emails,
    });
    HidingEntity {
        folder,
        id: entity_id.to_owned(),
        name,
        keys,
    }
}

fn strings(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::Array(values)) => values
            .iter()
            .filter_map(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .collect(),
        Some(Value::String(value)) if !value.is_empty() => vec![value.clone()],
        _ => Vec::new(),
    }
}

/// Whether the link folder `folder` in `facet` belongs to a hidden entity, for
/// that folder alone, by the same per-link rule as [`facet_hidden_keys`]: an
/// attached link is visible, a detached link a hide marked is hidden, and a
/// detached link without the mark is hidden when its entity is `blocked`.
///
/// Unlike [`facet_hidden_keys`], nothing else in the facet is read, so a
/// damaged hidden-names file or another folder's damaged link leaves this
/// answer standing. Undecidable when this folder's link can't be read, or when
/// it is detached without the mark and its entity file exists but can't be
/// read.
pub fn link_folder_hidden(
    journal: &Path,
    facet: &str,
    folder: &str,
) -> Result<bool, HideUndecidable> {
    let dirs = LinkDirs::for_facet(journal, facet);
    let link_path = || journal.join(dirs.link_rel(folder));
    let link = match dirs.read_link(folder) {
        Ok(Some(link)) => link,
        Ok(None) => return Ok(false),
        Err(error) => {
            return Err(HideUndecidable::Link {
                path: link_path(),
                detail: error.to_string(),
            });
        }
    };
    if !link.detached() {
        return Ok(false);
    }
    if link.value.get("detached_by").and_then(Value::as_str) == Some(HIDE_DETACHED_BY) {
        return Ok(true);
    }
    if !is_folder_name(&link.entity_id) {
        return Err(HideUndecidable::Link {
            path: link_path(),
            detail: format!("'{}' can't be an entity id", link.entity_id),
        });
    }
    Ok(
        read_identity_strict(journal, &link.entity_id)?.is_some_and(|identity| {
            identity
                .get("blocked")
                .is_some_and(super::lifecycle::value_is_truthy)
        }),
    )
}
