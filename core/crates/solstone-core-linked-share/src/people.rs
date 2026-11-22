// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The journal owner and the linked person a share is for.

use std::path::Path;

use serde_json::Value;
use solstone_core_cli_copy::MessageDetail;
use solstone_core_entity::{JournalEntity, is_admissible_person, load_all_journal_entities_in};

use crate::names::{PersonRef, normalize};

/// Shown when the journal names its owner nowhere.
const UNNAMED_OWNER: &str = "the owner";

/// The journal owner: the principal entity and every name that means them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Owner {
    pub entity_id: Option<String>,
    /// Display name, as talents address the owner.
    pub name: String,
    names: PersonRef,
}

impl Owner {
    pub fn new(entity_id: Option<String>, name: impl Into<String>, other_names: &[String]) -> Self {
        let name = name.into();
        Self {
            names: PersonRef::new(entity_id.clone().unwrap_or_default(), &name, other_names),
            entity_id,
            name,
        }
    }

    /// The single admissible principal and the journal's identity names.
    /// The display name is identity `preferred`, then `name`, then the
    /// principal's own name.
    pub fn load(journal: &Path) -> Result<Self, String> {
        Self::load_detail(journal, solstone_core_cli_copy::ENGLISH)
            .map_err(|error| error.to_string())
    }

    pub(crate) fn load_detail(journal: &Path, locale: &str) -> Result<Self, MessageDetail> {
        let principals = load_all_journal_entities_in(journal, locale)
            .map_err(|error| error.identity_read_detail())?
            .into_iter()
            .filter(|entity| entity.is_principal() && is_admissible_person(entity))
            .collect::<Vec<_>>();
        // Two principals is a damaged identity: name no one rather than guess.
        let principal = (principals.len() == 1).then(|| PersonRef::from_entity(&principals[0]));
        let identity = solstone_core_journal_config::read_journal_config(journal)
            .ok()
            .and_then(|read| read.config)
            .and_then(|config| config.get("identity").cloned());
        let label = |key: &str| {
            identity
                .as_ref()
                .and_then(|identity| identity.get(key))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| {
                    !value.is_empty() && !solstone_core_journal_config::is_path_shaped_name(value)
                })
                .map(str::to_owned)
        };
        let display = label("preferred")
            .or_else(|| label("name"))
            .or_else(|| principal.as_ref().map(|person| person.name.clone()))
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| UNNAMED_OWNER.to_owned());
        let mut others = solstone_core_entity::journal_identity_names(journal);
        if let Some(principal) = &principal {
            others.push(principal.name.clone());
            others.extend(principal.aliases.iter().cloned());
        }
        Ok(Self::new(
            principal.map(|person| person.entity_id),
            display,
            &others,
        ))
    }

    /// Whether a message sender is the owner: "me", "you", or one of the
    /// owner's names.
    pub(crate) fn is_sender(&self, sender: &str) -> bool {
        matches!(normalize(sender).as_str(), "me" | "you") || self.names.names(sender)
    }
}

/// Resolve `--person`: an admissible person's entity id exactly, else the one
/// person whose name or alias is the query. The owner is never a candidate.
pub fn resolve_person(journal: &Path, query: &str) -> Result<PersonRef, String> {
    resolve_person_detail(journal, query, solstone_core_cli_copy::ENGLISH)
        .map_err(|error| error.to_string())
}

pub(crate) fn resolve_person_detail(
    journal: &Path,
    query: &str,
    locale: &str,
) -> Result<PersonRef, MessageDetail> {
    let people = load_all_journal_entities_in(journal, locale)
        .map_err(|error| {
            error
                .identity_read_detail()
                .context("journal.cli.share.provider.entities.read")
        })?
        .into_iter()
        .filter(|entity| is_admissible_person(entity) && !entity.is_principal())
        .collect::<Vec<_>>();
    resolve_among(&people, query)
}

fn resolve_among(people: &[JournalEntity], query: &str) -> Result<PersonRef, MessageDetail> {
    if let Some(entity) = people.iter().find(|entity| entity.id == query) {
        return Ok(PersonRef::from_entity(entity));
    }
    let wanted = normalize(query);
    let matches = people
        .iter()
        .filter(|entity| {
            let names = entity.resolution_entity();
            !wanted.is_empty()
                && std::iter::once(&names.name)
                    .chain(&names.aka)
                    .any(|name| normalize(name) == wanted)
        })
        .map(PersonRef::from_entity)
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [person] => Ok(person.clone()),
        [] => Err(MessageDetail::message(
            "journal.cli.share.provider.person.absent",
            [("query", &query)],
        )),
        many => {
            let ids = many
                .iter()
                .map(|person| person.entity_id.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            Err(MessageDetail::message(
                "journal.cli.share.provider.person.ambiguous",
                [("query", &query), ("people", &ids)],
            ))
        }
    }
}

/// Listing declarations only traverses the directory. Reading activity rows only
/// resolves their path and reads the file. Other variants describe write operations.
pub(crate) fn facet_list_detail(error: &solstone_core_facets::FacetStoreError) -> MessageDetail {
    use solstone_core_facets::FacetStoreError as E;
    use solstone_core_journal_io::cli_diagnostics::{path_message, read_message};
    match error {
        E::Read(e) => read_message(e),
        E::Path(e) => path_message(e),
        other => other.to_string().into(),
    }
}

pub(crate) fn activity_read_detail(
    error: &solstone_core_facets::ActivityRecordStoreError,
) -> MessageDetail {
    use solstone_core_facets::ActivityRecordStoreError as E;
    use solstone_core_journal_io::cli_diagnostics::{path_message, read_message};
    match error {
        E::Read(e) => read_message(e),
        E::Path(e) => path_message(e),
        other => other.to_string().into(),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::json;

    use super::*;

    fn write(root: &Path, relative: &str, value: Value) {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().expect("parent")).expect("dirs");
        fs::write(path, value.to_string()).expect("write");
    }

    fn journal() -> tempfile::TempDir {
        let root = tempfile::tempdir().expect("root");
        write(
            root.path(),
            "config/journal.json",
            json!({"identity":{"name":"Sam Rivera","preferred":"Sam"}}),
        );
        write(
            root.path(),
            "entities/sam_rivera/entity.json",
            json!({"id":"sam_rivera","name":"Sam Rivera","type":"Person","is_principal":true}),
        );
        write(
            root.path(),
            "entities/jane_doe/entity.json",
            json!({"id":"jane_doe","name":"Jane Doe","aka":["JD","Janie"],"type":"Person"}),
        );
        write(
            root.path(),
            "entities/jane_park/entity.json",
            json!({"id":"jane_park","name":"Jane Park","aka":["Janie"],"type":"Person"}),
        );
        write(
            root.path(),
            "entities/acme/entity.json",
            json!({"id":"acme","name":"Acme","type":"Organization"}),
        );
        root
    }

    #[test]
    fn person_resolves_by_id_then_unique_name_or_alias() {
        let root = journal();
        assert_eq!(
            resolve_person(root.path(), "jane_park")
                .expect("id")
                .entity_id,
            "jane_park"
        );
        assert_eq!(
            resolve_person(root.path(), "jane doe")
                .expect("name")
                .entity_id,
            "jane_doe"
        );
        assert_eq!(
            resolve_person(root.path(), "JD").expect("alias").entity_id,
            "jane_doe"
        );
    }

    #[test]
    fn ambiguous_unknown_owner_and_non_person_queries_are_refused() {
        let root = journal();
        let error = resolve_person(root.path(), "janie").expect_err("ambiguous");
        assert!(
            error.contains("jane_doe") && error.contains("jane_park"),
            "{error}"
        );
        assert!(resolve_person(root.path(), "Nobody").is_err());
        // A derived first name is not an alias the owner gave.
        assert!(resolve_person(root.path(), "Jane").is_err());
        assert!(resolve_person(root.path(), "sam_rivera").is_err());
        assert!(resolve_person(root.path(), "acme").is_err());
    }

    #[test]
    fn owner_uses_the_preferred_name_and_knows_every_owner_name() {
        let root = journal();
        let owner = Owner::load(root.path()).expect("owner");
        assert_eq!(owner.name, "Sam");
        assert_eq!(owner.entity_id.as_deref(), Some("sam_rivera"));
        assert!(owner.is_sender("Sam Rivera"));
        assert!(owner.is_sender("Me"));
        assert!(owner.is_sender("you"));
        assert!(!owner.is_sender("Jane Doe"));
    }
}
