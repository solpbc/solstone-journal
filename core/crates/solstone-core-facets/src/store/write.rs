// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;

use serde_json::{Map, Value};
use solstone_core_journal_io::{JsonWriteOptions, remove_dir_all, write_json};

use crate::hold_facet_trust_lock;

use super::declaration::{observe_declared_facet_inventory, read_facet_declaration};
use super::error::FacetWriteError;
use super::facet_id::allocate_facet_id_locked;
use super::identity::read_facet_entity_link;
use super::paths::{declaration_path, facet_entity_link_path};
use super::retired::{
    RetiredFacet, first_free_facet_name, record_retired_facet, retired_facet_entry,
};

/// Create a facet declaration in its requested directory.
pub fn create_facet(
    journal_root: &Path,
    facet_dir: &str,
    title: &str,
    description: &str,
    color: &str,
    emoji: &str,
    icon: Option<&str>,
) -> Result<(), FacetWriteError> {
    let _trust = hold_facet_trust_lock(journal_root)?;
    if read_facet_declaration(journal_root, facet_dir)?.is_some() {
        return Err(FacetWriteError::AlreadyExists {
            path: declaration_path(journal_root, facet_dir)?,
        });
    }
    if retired_facet_entry(journal_root, facet_dir)?.is_some() {
        return Err(FacetWriteError::NameRetired {
            name: facet_dir.to_owned(),
        });
    }
    let id = allocate_facet_id_locked(journal_root)?;
    let mut declaration = Map::new();
    declaration.insert("id".to_owned(), Value::String(id));
    declaration.insert("title".to_owned(), Value::String(title.to_owned()));
    declaration.insert(
        "description".to_owned(),
        Value::String(description.to_owned()),
    );
    declaration.insert("color".to_owned(), Value::String(color.to_owned()));
    declaration.insert("emoji".to_owned(), Value::String(emoji.to_owned()));
    if let Some(icon) = icon.filter(|icon| !icon.is_empty()) {
        declaration.insert("icon".to_owned(), Value::String(icon.to_owned()));
    }
    save_facet_declaration(journal_root, facet_dir, &Value::Object(declaration))
}

/// The facet a journal with nothing to route to is given.
pub const DEFAULT_FACET: &str = "personal";
const DEFAULT_FACET_TITLE: &str = "Personal";

/// Give a journal with no enabled facet its default one; a no-op otherwise.
///
/// Every segment is routed to at least one enabled facet, and the owner's activity
/// lists are built from those routes, so a journal with none shows nothing. A
/// declared-but-muted `personal` is unmuted rather than duplicated. Returns whether
/// anything was written.
pub fn ensure_default_facet(journal_root: &Path) -> Result<bool, FacetWriteError> {
    let _trust = hold_facet_trust_lock(journal_root)?;
    let inventory = observe_declared_facet_inventory(journal_root)?;
    if !inventory.needs_default_facet() {
        return Ok(false);
    }
    if inventory.muted.iter().any(|facet| facet == DEFAULT_FACET) {
        set_facet_muted(journal_root, DEFAULT_FACET, false)?;
    } else {
        // A retired `personal` is never recreated; the default takes the next
        // free name and keeps the same title.
        let name = if retired_facet_entry(journal_root, DEFAULT_FACET)?.is_none() {
            DEFAULT_FACET.to_owned()
        } else {
            first_free_facet_name(journal_root, DEFAULT_FACET)?
        };
        create_facet(journal_root, &name, DEFAULT_FACET_TITLE, "", "", "", None)?;
    }
    Ok(true)
}

/// Refuse muting or deleting the journal's last enabled facet.
fn refuse_last_enabled(journal_root: &Path, facet_dir: &str) -> Result<(), FacetWriteError> {
    let inventory = observe_declared_facet_inventory(journal_root)?;
    if inventory.enabled.len() == 1 && inventory.enabled[0] == facet_dir {
        return Err(FacetWriteError::LastEnabledFacet {
            facet: facet_dir.to_owned(),
        });
    }
    Ok(())
}

/// Delete a facet directory when its declaration exists.
pub fn delete_facet(journal_root: &Path, facet_dir: &str) -> Result<bool, FacetWriteError> {
    let _trust = hold_facet_trust_lock(journal_root)?;
    let path = declaration_path(journal_root, facet_dir)?;
    if read_facet_declaration(journal_root, facet_dir)?.is_none() {
        return Ok(false);
    }
    refuse_last_enabled(journal_root, facet_dir)?;
    // The name is retired before the directory goes: a crash in between leaves
    // the live facet with its own leftover entry, which resolves to the live
    // facet and lets a retried delete complete.
    let declaration = read_facet_declaration(journal_root, facet_dir)?;
    let id = declaration.as_ref().and_then(|snapshot| {
        snapshot
            .value()
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| super::facet_id::is_well_formed_facet_id(id))
            .map(str::to_owned)
    });
    let title = declaration
        .as_ref()
        .and_then(|snapshot| snapshot.value().get("title").and_then(Value::as_str))
        .map(str::to_owned);
    record_retired_facet(journal_root, facet_dir, RetiredFacet::deleted(id, title))?;
    remove_dir_all(journal_root, &format!("facets/{facet_dir}"))
        .map_err(FacetWriteError::EntityLinkRemoval)?;
    let _ = path;
    Ok(true)
}

/// Update facet metadata while preserving identity and unknown declaration fields.
pub fn update_facet(
    journal_root: &Path,
    facet_dir: &str,
    title: &str,
    description: &str,
    color: &str,
    emoji: &str,
    icon: Option<&str>,
) -> Result<(), FacetWriteError> {
    let _trust = hold_facet_trust_lock(journal_root)?;
    let path = declaration_path(journal_root, facet_dir)?;
    let snapshot = read_facet_declaration(journal_root, facet_dir)?
        .ok_or(FacetWriteError::DeclarationMissing { path })?;
    let mut declaration = snapshot.into_value();
    let object = declaration
        .as_object_mut()
        .expect("facet declaration reader returns an object");
    object.insert("title".to_owned(), Value::String(title.to_owned()));
    object.insert(
        "description".to_owned(),
        Value::String(description.to_owned()),
    );
    object.insert("color".to_owned(), Value::String(color.to_owned()));
    object.insert("emoji".to_owned(), Value::String(emoji.to_owned()));
    match icon.filter(|icon| !icon.is_empty()) {
        Some(icon) => {
            object.insert("icon".to_owned(), Value::String(icon.to_owned()));
        }
        None => {
            object.remove("icon");
        }
    }
    save_facet_declaration(journal_root, facet_dir, &declaration)
}

/// Persist muted state using the compact true-or-absent representation.
pub fn set_facet_muted(
    journal_root: &Path,
    facet_dir: &str,
    muted: bool,
) -> Result<(), FacetWriteError> {
    let _trust = hold_facet_trust_lock(journal_root)?;
    let path = declaration_path(journal_root, facet_dir)?;
    let snapshot = read_facet_declaration(journal_root, facet_dir)?
        .ok_or(FacetWriteError::DeclarationMissing { path })?;
    if muted {
        refuse_last_enabled(journal_root, facet_dir)?;
    }
    let mut declaration = snapshot.into_value();
    let object = declaration
        .as_object_mut()
        .expect("facet declaration reader returns an object");
    if muted {
        object.insert("muted".to_owned(), Value::Bool(true));
    } else {
        object.remove("muted");
    }
    save_facet_declaration(journal_root, facet_dir, &declaration)
}

/// Persist a facet relationship's resolved journal entity link.
pub fn save_facet_entity_link(
    journal_root: &Path,
    facet_dir: &str,
    entity_dir: &str,
    journal_entity_id: &str,
    other_relationship_fields: &Map<String, Value>,
) -> Result<(), FacetWriteError> {
    let _trust = hold_facet_trust_lock(journal_root)?;
    let mut relationship = other_relationship_fields.clone();
    relationship.insert(
        "entity_id".to_owned(),
        Value::String(journal_entity_id.to_owned()),
    );
    let path = facet_entity_link_path(journal_root, facet_dir, entity_dir)?;
    write_json(&path, &Value::Object(relationship), json_options())
        .map_err(FacetWriteError::EntityLinkWrite)
}

/// Set or clear a relationship's detached marker while retaining every other field.
pub fn set_facet_entity_link_detached(
    journal_root: &Path,
    facet_dir: &str,
    entity_dir: &str,
    detached: bool,
) -> Result<bool, FacetWriteError> {
    let _trust = hold_facet_trust_lock(journal_root)?;
    let Some(snapshot) = read_facet_entity_link(journal_root, facet_dir, entity_dir)? else {
        return Ok(false);
    };
    let mut relationship = snapshot.value().clone();
    let object = relationship
        .as_object_mut()
        .expect("facet entity link reader returns an object");
    let changed = if detached {
        if object.get("detached") == Some(&Value::Bool(true)) {
            false
        } else {
            object.insert("detached".to_owned(), Value::Bool(true));
            true
        }
    } else {
        object.remove("detached").is_some()
    };
    if !changed {
        return Ok(false);
    }
    let path = facet_entity_link_path(journal_root, facet_dir, entity_dir)?;
    write_json(&path, &relationship, json_options()).map_err(FacetWriteError::EntityLinkWrite)?;
    Ok(true)
}

/// Remove one complete facet relationship directory when its link exists.
pub fn delete_facet_entity_link(
    journal_root: &Path,
    facet_dir: &str,
    entity_dir: &str,
) -> Result<bool, FacetWriteError> {
    let _trust = hold_facet_trust_lock(journal_root)?;
    if read_facet_entity_link(journal_root, facet_dir, entity_dir)?.is_none() {
        return Ok(false);
    }
    remove_dir_all(
        journal_root,
        &format!("facets/{facet_dir}/entities/{entity_dir}"),
    )
    .map_err(FacetWriteError::EntityLinkRemoval)?;
    Ok(true)
}

/// Change a facet's title. Its name, directory and id never change, so every
/// reference, grant and index row stays as it is.
pub fn retitle_facet(
    journal_root: &Path,
    facet_dir: &str,
    title: &str,
) -> Result<(), FacetWriteError> {
    let _trust = hold_facet_trust_lock(journal_root)?;
    let path = declaration_path(journal_root, facet_dir)?;
    let snapshot = read_facet_declaration(journal_root, facet_dir)?
        .ok_or(FacetWriteError::DeclarationMissing { path })?;
    let mut declaration = snapshot.into_value();
    declaration
        .as_object_mut()
        .expect("facet declaration reader returns an object")
        .insert("title".to_owned(), Value::String(title.to_owned()));
    save_facet_declaration(journal_root, facet_dir, &declaration)
}

pub(super) fn save_facet_declaration(
    journal_root: &Path,
    facet_dir: &str,
    declaration: &Value,
) -> Result<(), FacetWriteError> {
    let path = declaration_path(journal_root, facet_dir)?;
    write_json(&path, declaration, json_options()).map_err(FacetWriteError::DeclarationWrite)
}

fn json_options() -> JsonWriteOptions {
    JsonWriteOptions {
        mode: Some(0o600),
        indent: Some(2),
        sort_keys: false,
    }
}

#[cfg(test)]
mod default_facet_tests {
    use super::{DEFAULT_FACET, create_facet, delete_facet, ensure_default_facet, set_facet_muted};
    use crate::store::declaration::{observe_declared_facet_inventory, read_facet_declaration};
    use crate::store::error::FacetWriteError;

    fn enabled(root: &std::path::Path) -> Vec<String> {
        observe_declared_facet_inventory(root).unwrap().enabled
    }

    #[test]
    fn a_journal_with_no_facets_gets_personal_and_a_second_boot_is_a_no_op() {
        let root = tempfile::tempdir().unwrap();
        assert!(ensure_default_facet(root.path()).unwrap());
        assert_eq!(enabled(root.path()), vec![DEFAULT_FACET]);
        let declaration = read_facet_declaration(root.path(), DEFAULT_FACET)
            .unwrap()
            .unwrap();
        assert_eq!(declaration.title, "Personal");
        assert!(!ensure_default_facet(root.path()).unwrap());
        assert_eq!(enabled(root.path()), vec![DEFAULT_FACET]);
    }

    #[test]
    fn any_enabled_facet_makes_it_a_no_op() {
        let root = tempfile::tempdir().unwrap();
        create_facet(root.path(), "work", "Work", "", "", "", None).unwrap();
        assert!(!ensure_default_facet(root.path()).unwrap());
        assert_eq!(enabled(root.path()), vec!["work"]);
    }

    #[test]
    fn an_all_muted_journal_gets_personal_and_a_muted_personal_is_unmuted_not_duplicated() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("facets/work")).unwrap();
        std::fs::write(
            root.path().join("facets/work/facet.json"),
            br#"{"title":"Work","muted":true}"#,
        )
        .unwrap();
        assert!(ensure_default_facet(root.path()).unwrap());
        assert_eq!(enabled(root.path()), vec![DEFAULT_FACET]);

        let muted = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(muted.path().join("facets/personal")).unwrap();
        std::fs::write(
            muted.path().join("facets/personal/facet.json"),
            br#"{"title":"Home","muted":true}"#,
        )
        .unwrap();
        assert!(ensure_default_facet(muted.path()).unwrap());
        let declaration = read_facet_declaration(muted.path(), DEFAULT_FACET)
            .unwrap()
            .unwrap();
        assert_eq!(declaration.title, "Home");
        assert_eq!(enabled(muted.path()), vec![DEFAULT_FACET]);
    }

    /// A damaged declaration is the owner's to repair; adding a facet beside it would
    /// hide the damage behind a working-looking journal.
    #[test]
    fn a_malformed_declaration_is_left_alone() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("facets/work")).unwrap();
        std::fs::write(root.path().join("facets/work/facet.json"), b"{broken").unwrap();
        assert!(!ensure_default_facet(root.path()).unwrap());
        assert!(!root.path().join("facets/personal").exists());
        assert_eq!(
            std::fs::read(root.path().join("facets/work/facet.json")).unwrap(),
            b"{broken"
        );
    }

    #[test]
    fn the_last_enabled_facet_cannot_be_muted_or_deleted() {
        let root = tempfile::tempdir().unwrap();
        create_facet(root.path(), "work", "Work", "", "", "", None).unwrap();
        assert!(matches!(
            set_facet_muted(root.path(), "work", true),
            Err(FacetWriteError::LastEnabledFacet { facet }) if facet == "work"
        ));
        assert!(matches!(
            delete_facet(root.path(), "work"),
            Err(FacetWriteError::LastEnabledFacet { facet }) if facet == "work"
        ));
        assert_eq!(enabled(root.path()), vec!["work"]);

        // With a sibling enabled, both are ordinary again.
        create_facet(root.path(), "personal", "Personal", "", "", "", None).unwrap();
        set_facet_muted(root.path(), "work", true).unwrap();
        assert_eq!(enabled(root.path()), vec!["personal"]);
        assert!(delete_facet(root.path(), "work").unwrap());
        // A muted facet never counts as the last enabled one.
        create_facet(root.path(), "archive", "Archive", "", "", "", None).unwrap();
        set_facet_muted(root.path(), "archive", true).unwrap();
        assert!(delete_facet(root.path(), "archive").unwrap());
    }
}
