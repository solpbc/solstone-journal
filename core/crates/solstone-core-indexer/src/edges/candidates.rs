// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use chrono_tz::Tz;
use serde_json::{Map, Value};
use solstone_core_entity_matching::{EntityNameCandidate, find_matching_entity};

use crate::edges::speaker::SpeakerEntityIndex;
use crate::edges::{EdgeContext, EdgeError};

type JsonObject = Map<String, Value>;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct EdgeDropCounter {
    drops: usize,
}

impl EdgeDropCounter {
    pub fn reset(&mut self) {
        self.drops = 0;
    }

    pub fn record_drop(&mut self) {
        self.drops += 1;
    }

    pub fn drops(&self) -> usize {
        self.drops
    }
}

pub struct EdgeResolver {
    journal: PathBuf,
    cache: BTreeMap<String, Vec<EntityNameCandidate>>,
    drops: EdgeDropCounter,
    owner_timezone: Option<Result<Tz, EdgeError>>,
    speaker_entities: Option<SpeakerEntityIndex>,
}

impl EdgeResolver {
    pub fn new(journal: &Path) -> Self {
        Self {
            journal: journal.to_path_buf(),
            cache: BTreeMap::new(),
            drops: EdgeDropCounter::default(),
            owner_timezone: None,
            speaker_entities: None,
        }
    }

    pub fn begin_file(&mut self) {
        self.drops.reset();
    }

    pub fn drops(&self) -> usize {
        self.drops.drops()
    }

    pub fn drops_mut(&mut self) -> &mut EdgeDropCounter {
        &mut self.drops
    }

    pub fn resolve(
        &mut self,
        context: &EdgeContext,
        name: &str,
    ) -> Result<Option<String>, EdgeError> {
        if name.trim().is_empty() {
            self.record_drop();
            return Ok(None);
        }
        if !self.cache.contains_key(&context.facet) {
            let candidates = load_candidates(&self.journal, &context.facet).map_err(|error| {
                EdgeError::Io(format!(
                    "candidate load failed for facet {:?}: {error}",
                    context.facet
                ))
            })?;
            self.cache.insert(context.facet.clone(), candidates);
        }
        let candidates = self.cache.get(&context.facet).ok_or_else(|| {
            EdgeError::Io(format!("candidate cache missing for {:?}", context.facet))
        })?;
        let matched = find_matching_entity(name, candidates, 90.0);
        let candidate = matched.and_then(|result| candidates.get(result.candidate_index));
        let entity_id = candidate.and_then(|candidate| candidate.id.as_deref());
        Ok(match entity_id {
            Some(entity_id) if !entity_id.is_empty() => Some(entity_id.to_string()),
            _ => {
                self.record_drop();
                None
            }
        })
    }

    pub fn record_drop(&mut self) {
        self.drops.record_drop();
    }

    pub fn preflight_owner_timezone(&mut self) -> Result<(), EdgeError> {
        self.owner_timezone().map(|_| ())
    }

    pub(super) fn owner_timezone(&mut self) -> Result<Tz, EdgeError> {
        if let Some(timezone) = &self.owner_timezone {
            return timezone.clone();
        }
        let timezone = super::owner_timezone_for_journal(&self.journal);
        self.owner_timezone = Some(timezone.clone());
        timezone
    }

    pub(super) fn speaker_entities(&mut self) -> Result<&SpeakerEntityIndex, EdgeError> {
        if self.speaker_entities.is_none() {
            let candidates = super::speaker::build_speaker_entity_index(&self.journal)?;
            self.speaker_entities = Some(candidates);
        }
        match self.speaker_entities.as_ref() {
            Some(candidates) => Ok(candidates),
            None => Err(EdgeError::Io("speaker entity cache missing".to_string())),
        }
    }
}

fn load_candidates(journal: &Path, facet: &str) -> io::Result<Vec<EntityNameCandidate>> {
    if facet.is_empty() {
        return load_journal_candidates(journal);
    }
    load_facet_candidates(journal, facet)
}

fn load_journal_candidates(journal: &Path) -> io::Result<Vec<EntityNameCandidate>> {
    let mut candidates = Vec::new();
    for (entity_dir, mut entity) in journal_entities_by_folder(journal)? {
        entity.insert("id".to_string(), Value::String(entity_dir));
        if json_truthy(entity.get("blocked")) {
            continue;
        }
        if let Some(candidate) = candidate_from_entity(&entity) {
            candidates.push(candidate);
        }
    }
    Ok(candidates)
}

fn load_facet_candidates(journal: &Path, facet: &str) -> io::Result<Vec<EntityNameCandidate>> {
    let (journal_entities, dirs_by_id) = load_journal_entities(journal)?;
    let mut candidates = Vec::new();
    for (linked_dir, relationship) in facet_link_per_entity(journal, facet, &dirs_by_id)? {
        let mut relationship = relationship;
        relationship.insert("entity_id".to_string(), Value::String(linked_dir.clone()));
        let enriched =
            enrich_relationship_with_journal(relationship, journal_entities.get(&linked_dir));
        if json_truthy(enriched.get("blocked")) {
            continue;
        }
        if let Some(candidate) = candidate_from_entity(&enriched) {
            candidates.push(candidate);
        }
    }
    Ok(candidates)
}

type JournalEntities = (BTreeMap<String, JsonObject>, BTreeMap<String, String>);

/// Journal entities by directory, and each effective id's directory, as the
/// entity store's identity map resolves it.
fn load_journal_entities(journal: &Path) -> io::Result<JournalEntities> {
    let mut entities = BTreeMap::new();
    for (entity_dir, mut entity) in journal_entities_by_folder(journal)? {
        entity.insert("id".to_string(), Value::String(entity_dir.clone()));
        entities.insert(entity_dir, entity);
    }
    let dirs_by_id = identity_dirs_by_id(journal)?;
    Ok((entities, dirs_by_id))
}

/// Each effective entity id with the directory that holds it, as the entity
/// store's identity map resolves it; empty when there is no `entities/`.
pub(crate) fn identity_dirs_by_id(journal: &Path) -> io::Result<BTreeMap<String, String>> {
    if !journal.join("entities").is_dir() {
        return Ok(BTreeMap::new());
    }
    Ok(solstone_core_entity::read_identity_map(journal)
        .map_err(io::Error::other)?
        .resolved
        .into_iter()
        .collect())
}

/// Every readable journal entity, keyed by the directory edges key it on, in
/// directory order. An identity file that is missing, damaged or unreadable is
/// skipped, as the entity store's readers do.
pub(super) fn journal_entities_by_folder(journal: &Path) -> io::Result<Vec<(String, JsonObject)>> {
    let mut entities = Vec::new();
    for (entity_dir, _) in sorted_child_dirs(&journal.join("entities"))? {
        // A folder the store can't place inside the journal holds no entity.
        let placed = solstone_core_entity::entity_identity_path(journal, &entity_dir)
            .is_ok_and(|path| path.is_file());
        if !placed {
            continue;
        }
        if let Ok(Some(identity)) = solstone_core_entity::read_entity_identity(journal, &entity_dir)
            && let Value::Object(entity) = identity.value().clone()
        {
            entities.push((entity_dir, entity));
        }
    }
    Ok(entities)
}

/// The one link of `facet` that speaks for each entity, keyed by the journal
/// directory edges key the entity on, in folder order; an entity whose link
/// doesn't count is left out. A link names its entity by id and its folder
/// can differ (a merge moves links without renaming them), so a stored id is
/// resolved through `dirs_by_id`, falling back to the folder name. The
/// entity's own folder speaks for it, detached or not. Links left in other
/// folders count only when none of them is detached, and then only the
/// first: two would tie in a matcher and match nothing.
pub(crate) fn facet_link_per_entity(
    journal: &Path,
    facet: &str,
    dirs_by_id: &BTreeMap<String, String>,
) -> io::Result<Vec<(String, JsonObject)>> {
    let mut order = Vec::new();
    let mut groups = BTreeMap::<String, Vec<(String, JsonObject)>>::new();
    for (folder, relationship, written_id) in facet_links(journal, facet)? {
        let linked_dir = written_id
            .and_then(|id| dirs_by_id.get(&id).cloned())
            .unwrap_or_else(|| folder.clone());
        if !groups.contains_key(&linked_dir) {
            order.push(linked_dir.clone());
        }
        groups
            .entry(linked_dir)
            .or_default()
            .push((folder, relationship));
    }
    let mut chosen = Vec::new();
    for linked_dir in order {
        let mut group = groups.remove(&linked_dir).expect("grouped above");
        let own = group.iter().position(|(folder, _)| *folder == linked_dir);
        let relationship = match own {
            Some(index) => group.swap_remove(index).1,
            None if group
                .iter()
                .any(|(_, relationship)| json_truthy(relationship.get("detached"))) =>
            {
                continue;
            }
            None => group.swap_remove(0).1,
        };
        if json_truthy(relationship.get("detached")) {
            continue;
        }
        chosen.push((linked_dir, relationship));
    }
    Ok(chosen)
}

/// Every readable link of `facet`, in folder order: its folder, its fields,
/// and the entity id it stores, when it stores one. A link that is missing,
/// damaged or not a plain file is skipped.
pub(crate) fn facet_links(
    journal: &Path,
    facet: &str,
) -> io::Result<Vec<(String, JsonObject, Option<String>)>> {
    let dirs = solstone_core_entity::facet_links::LinkDirs::for_facet(journal, facet);
    let mut links = Vec::new();
    // A link folder the store can't place inside the journal holds nothing.
    let Ok(folders) = dirs.all_folders() else {
        return Ok(Vec::new());
    };
    for folder in folders {
        // A link that is dangling, or not a plain file, is not read.
        if !dirs.link_path(&folder).is_ok_and(|path| path.is_file()) {
            continue;
        }
        let Ok(Some(link)) = dirs.read_link(&folder) else {
            continue;
        };
        let Value::Object(fields) = link.value else {
            continue;
        };
        links.push((folder, fields, link.id_written.then_some(link.entity_id)));
    }
    Ok(links)
}

fn enrich_relationship_with_journal(
    mut relationship: JsonObject,
    journal_entity: Option<&JsonObject>,
) -> JsonObject {
    let relationship_entity_id = string_field(relationship.get("entity_id")).unwrap_or_default();
    if let Some(journal_entity) = journal_entity {
        let id = journal_entity
            .get("id")
            .cloned()
            .unwrap_or(Value::String(relationship_entity_id));
        relationship.insert("id".to_string(), id);
        let name = journal_entity
            .get("name")
            .cloned()
            .unwrap_or_else(|| Value::String(String::new()));
        relationship.insert("name".to_string(), name);
        let entity_type = journal_entity
            .get("type")
            .cloned()
            .unwrap_or_else(|| Value::String(String::new()));
        relationship.insert("type".to_string(), entity_type);
        if json_truthy(journal_entity.get("aka"))
            && let Some(value) = journal_entity.get("aka")
        {
            relationship.insert("aka".to_string(), value.clone());
        }
        if json_truthy(journal_entity.get("is_principal")) {
            relationship.insert("is_principal".to_string(), Value::Bool(true));
        }
        if json_truthy(journal_entity.get("blocked")) {
            relationship.insert("blocked".to_string(), Value::Bool(true));
        }
    } else {
        relationship.insert("id".to_string(), Value::String(relationship_entity_id));
    }
    relationship.remove("entity_id");
    relationship
}

fn candidate_from_entity(entity: &JsonObject) -> Option<EntityNameCandidate> {
    let name = string_field(entity.get("name"))?;
    if name.is_empty() {
        return None;
    }
    let id = string_field(entity.get("id")).filter(|value| !value.is_empty());
    Some(EntityNameCandidate {
        id,
        name,
        aka: string_array(entity.get("aka")),
        emails: string_array(entity.get("emails")),
    })
}

fn sorted_child_dirs(root: &Path) -> io::Result<Vec<(String, PathBuf)>> {
    if !root.exists() {
        return Ok(Vec::new());
    }
    if !root.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotADirectory,
            format!("{} is not a directory", root.display()),
        ));
    }
    let mut dirs = Vec::new();
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        dirs.push((
            entry.file_name().to_string_lossy().into_owned(),
            entry.path(),
        ));
    }
    dirs.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(dirs)
}

fn string_field(value: Option<&Value>) -> Option<String> {
    match value {
        Some(Value::String(text)) => Some(text.clone()),
        _ => None,
    }
}

fn string_array(value: Option<&Value>) -> Vec<String> {
    let Some(Value::Array(items)) = value else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| match item {
            Value::String(text) => Some(text.clone()),
            _ => None,
        })
        .collect()
}

fn json_truthy(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => false,
        Some(Value::Bool(value)) => *value,
        Some(Value::Number(value)) => value.as_f64() != Some(0.0),
        Some(Value::String(value)) => !value.is_empty(),
        Some(Value::Array(value)) => !value.is_empty(),
        Some(Value::Object(value)) => !value.is_empty(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::reserve_temp_path;
    use serde_json::json;

    fn temp_root(name: &str) -> PathBuf {
        reserve_temp_path(&format!("solstone-core-indexer-edge-candidates-{name}"))
    }

    fn write_json(root: &Path, rel: &str, value: Value) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().expect("test path should have parent"))
            .expect("create parent");
        fs::write(path, serde_json::to_string(&value).expect("encode json")).expect("write json");
    }

    #[test]
    fn facet_enrichment_preserves_relationship_aka_when_journal_aka_falsey() {
        let root = temp_root("relationship-aka");
        write_json(
            &root,
            "entities/alice/entity.json",
            json!({"name":"Alice Example","type":"Person","aka":[]}),
        );
        write_json(
            &root,
            "facets/work/entities/alice/entity.json",
            json!({"aka":["Work Alice"],"emails":["rel@example.com"]}),
        );

        let candidates = load_facet_candidates(&root, "work").expect("load facet candidates");
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].id.as_deref(), Some("alice"));
        assert_eq!(candidates[0].name, "Alice Example");
        assert_eq!(candidates[0].aka, vec!["Work Alice"]);
        assert_eq!(candidates[0].emails, vec!["rel@example.com"]);
        fs::remove_dir_all(root).expect("cleanup relationship aka root");
    }

    #[test]
    fn facet_enrichment_truthy_journal_fields_override_or_skip() {
        let root = temp_root("truthy");
        write_json(
            &root,
            "entities/alice/entity.json",
            json!({"name":"Alice Example","type":"Person","aka":["Journal Alice"]}),
        );
        write_json(
            &root,
            "entities/blocked/entity.json",
            json!({"name":"Blocked Person","type":"Person","blocked":true}),
        );
        write_json(
            &root,
            "facets/work/entities/alice/entity.json",
            json!({"aka":["Work Alice"],"emails":["rel@example.com"]}),
        );
        write_json(
            &root,
            "facets/work/entities/blocked/entity.json",
            json!({"aka":["Blocked Work"]}),
        );
        write_json(
            &root,
            "facets/work/entities/detached/entity.json",
            json!({"name":"Detached","detached":true}),
        );

        let candidates = load_facet_candidates(&root, "work").expect("load facet candidates");
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].aka, vec!["Journal Alice"]);
        fs::remove_dir_all(root).expect("cleanup truthy root");
    }

    #[test]
    fn enrichment_truthy_booleans_canonicalize_to_true() {
        let relationship = json!({"entity_id":"alice","is_principal":"relationship"});
        let journal_entity = json!({"id":"alice","name":"Alice Example","type":"Person","is_principal":"yes","blocked":"yes"});
        let enriched = enrich_relationship_with_journal(
            relationship
                .as_object()
                .expect("relationship object")
                .clone(),
            Some(journal_entity.as_object().expect("journal object")),
        );

        assert_eq!(enriched.get("is_principal"), Some(&Value::Bool(true)));
        assert_eq!(enriched.get("blocked"), Some(&Value::Bool(true)));
    }

    #[test]
    fn a_link_in_a_differently_named_folder_resolves_to_its_entity() {
        let root = temp_root("relinked");
        write_json(
            &root,
            "entities/solstone/entity.json",
            json!({"id":"solstone","name":"Solstone","type":"Project"}),
        );
        // After `sunstone` merged into `solstone`, the link kept its folder.
        write_json(
            &root,
            "facets/work/entities/sunstone/entity.json",
            json!({"entity_id":"solstone"}),
        );
        let candidates = load_facet_candidates(&root, "work").expect("load facet candidates");
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].id.as_deref(), Some("solstone"));
        assert_eq!(candidates[0].name, "Solstone");
        fs::remove_dir_all(root).expect("cleanup relinked root");
    }

    #[test]
    fn two_links_to_one_entity_give_one_candidate() {
        let root = temp_root("duplicate-links");
        write_json(
            &root,
            "entities/jane_doe/entity.json",
            json!({"id":"jane_doe","name":"Jane Doe","type":"Person"}),
        );
        write_json(
            &root,
            "facets/work/entities/jane/entity.json",
            json!({"entity_id":"jane_doe"}),
        );
        write_json(
            &root,
            "facets/work/entities/jane_doe/entity.json",
            json!({"entity_id":"jane_doe"}),
        );
        let candidates = load_facet_candidates(&root, "work").expect("load facet candidates");
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].id.as_deref(), Some("jane_doe"));
        assert!(find_matching_entity("Jane Doe", &candidates, 90.0).is_some());
        fs::remove_dir_all(root).expect("cleanup duplicate links root");
    }

    #[test]
    fn the_entitys_own_link_decides_over_a_link_left_in_another_folder() {
        let root = temp_root("own-link-decides");
        write_json(
            &root,
            "entities/beta/entity.json",
            json!({"id":"beta","name":"Beta Example","type":"Person"}),
        );
        write_json(
            &root,
            "facets/work/entities/alpha_old/entity.json",
            json!({"entity_id":"beta","aka":["Old Alpha"]}),
        );
        write_json(
            &root,
            "facets/work/entities/beta/entity.json",
            json!({"entity_id":"beta","detached":true}),
        );
        assert!(
            load_facet_candidates(&root, "work")
                .expect("load facet candidates")
                .is_empty()
        );
        write_json(
            &root,
            "facets/work/entities/beta/entity.json",
            json!({"entity_id":"beta","emails":["beta@example.com"]}),
        );
        let candidates = load_facet_candidates(&root, "work").expect("load facet candidates");
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].emails, vec!["beta@example.com"]);
        assert!(candidates[0].aka.is_empty());
        fs::remove_dir_all(root).expect("cleanup own link root");
    }

    #[test]
    fn a_detached_leftover_link_keeps_an_entity_without_its_own_link_out() {
        let root = temp_root("detached-leftover");
        write_json(
            &root,
            "entities/beta/entity.json",
            json!({"id":"beta","name":"Beta Example","type":"Person"}),
        );
        write_json(
            &root,
            "facets/work/entities/a_old/entity.json",
            json!({"entity_id":"beta"}),
        );
        write_json(
            &root,
            "facets/work/entities/b_old/entity.json",
            json!({"entity_id":"beta","detached":true}),
        );
        assert!(
            load_facet_candidates(&root, "work")
                .expect("load facet candidates")
                .is_empty()
        );
        fs::remove_dir_all(root).expect("cleanup detached leftover root");
    }

    #[test]
    fn links_whose_folder_id_and_directory_agree_are_unchanged() {
        let root = temp_root("agreeing-links");
        write_json(
            &root,
            "entities/alice/entity.json",
            json!({"id":"alice","name":"Alice Example","type":"Person"}),
        );
        write_json(
            &root,
            "entities/bob/entity.json",
            json!({"name":"Bob Example","type":"Person"}),
        );
        write_json(
            &root,
            "facets/work/entities/alice/entity.json",
            json!({"entity_id":"alice","aka":["Al"]}),
        );
        write_json(&root, "facets/work/entities/bob/entity.json", json!({}));
        let candidates = load_facet_candidates(&root, "work").expect("load facet candidates");
        let ids: Vec<_> = candidates.iter().map(|c| c.id.as_deref()).collect();
        assert_eq!(ids, vec![Some("alice"), Some("bob")]);
        assert_eq!(candidates[0].aka, vec!["Al"]);
        fs::remove_dir_all(root).expect("cleanup agreeing links root");
    }

    #[cfg(unix)]
    #[test]
    fn an_identity_the_store_cannot_place_is_skipped() {
        let root = temp_root("dangling-identity");
        write_json(
            &root,
            "entities/ada/entity.json",
            json!({"name":"Ada Example"}),
        );
        fs::create_dir_all(root.join("entities/gone")).expect("dangling folder");
        std::os::unix::fs::symlink(
            root.join("nowhere.json"),
            root.join("entities/gone/entity.json"),
        )
        .expect("dangling identity");
        write_json(&root, "facets/work/entities/ada/entity.json", json!({}));

        let journal = load_journal_candidates(&root).expect("load journal candidates");
        assert_eq!(journal.len(), 1);
        let facet = load_facet_candidates(&root, "work").expect("load facet candidates");
        assert_eq!(facet.len(), 1);
        fs::remove_dir_all(root).expect("cleanup dangling root");
    }

    #[test]
    fn empty_facet_uses_journal_emails_but_facet_does_not_copy_them() {
        let root = temp_root("emails");
        write_json(
            &root,
            "entities/alice/entity.json",
            json!({"name":"Alice Example","type":"Person","emails":["journal@example.com"]}),
        );
        write_json(&root, "facets/work/entities/alice/entity.json", json!({}));

        let journal_candidates = load_journal_candidates(&root).expect("load journal candidates");
        assert_eq!(journal_candidates[0].emails, vec!["journal@example.com"]);
        let facet_candidates = load_facet_candidates(&root, "work").expect("load facet candidates");
        assert!(facet_candidates[0].emails.is_empty());
        fs::remove_dir_all(root).expect("cleanup emails root");
    }
}
