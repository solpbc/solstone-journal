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
    for (entity_id, entity_dir) in sorted_child_dirs(&journal.join("entities"))? {
        let entity_file = entity_dir.join("entity.json");
        if !entity_file.is_file() {
            continue;
        }
        let Some(mut entity) = read_json_object(&entity_file) else {
            continue;
        };
        entity.insert("id".to_string(), Value::String(entity_id));
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
    let journal_entities = load_journal_entities(journal)?;
    let mut candidates = Vec::new();
    let entity_root = journal.join("facets").join(facet).join("entities");
    for (entity_id, entity_dir) in sorted_child_dirs(&entity_root)? {
        let relationship_file = entity_dir.join("entity.json");
        if !relationship_file.is_file() {
            continue;
        }
        let Some(mut relationship) = read_json_object(&relationship_file) else {
            continue;
        };
        relationship.insert("entity_id".to_string(), Value::String(entity_id.clone()));
        if json_truthy(relationship.get("detached")) {
            continue;
        }
        let enriched =
            enrich_relationship_with_journal(relationship, journal_entities.get(&entity_id));
        if json_truthy(enriched.get("blocked")) {
            continue;
        }
        if let Some(candidate) = candidate_from_entity(&enriched) {
            candidates.push(candidate);
        }
    }
    Ok(candidates)
}

fn load_journal_entities(journal: &Path) -> io::Result<BTreeMap<String, JsonObject>> {
    let mut entities = BTreeMap::new();
    for (entity_id, entity_dir) in sorted_child_dirs(&journal.join("entities"))? {
        let entity_file = entity_dir.join("entity.json");
        if !entity_file.is_file() {
            continue;
        }
        let Some(mut entity) = read_json_object(&entity_file) else {
            continue;
        };
        entity.insert("id".to_string(), Value::String(entity_id.clone()));
        entities.insert(entity_id, entity);
    }
    Ok(entities)
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

fn read_json_object(path: &Path) -> Option<JsonObject> {
    let text = fs::read_to_string(path).ok()?;
    match serde_json::from_str::<Value>(&text).ok()? {
        Value::Object(record) => Some(record),
        _ => None,
    }
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
