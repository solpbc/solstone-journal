// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Admission of the configured owner identity.

use std::path::Path;

use serde_json::Value;
use solstone_core_entity::{
    JournalEntity, is_admissible_person, read_entity_identity, read_identity_map,
};

/// Machine-readable reason for an inadmissible configured owner identity.
pub const OWNER_IDENTITY_INVALID_REASON: &str = "speaker_owner_identity_invalid";

/// The one configured owner identity that is safe to use for speaker artifacts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OwnerAdmission {
    Admitted(String),
    Invalid,
}

/// Why a supplied owner-artifact target is not safe to use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OwnerAdmissionFailure {
    IdentityInvalid,
    TargetMismatch {
        requested_id: String,
        admitted_id: String,
    },
}

/// Resolve the configured owner only when exactly one identity-map winner is an
/// unblocked `Person` explicitly marked `is_principal`.
pub fn admitted_owner_id(journal_root: &Path) -> OwnerAdmission {
    let Ok(map) = read_identity_map(journal_root) else {
        return OwnerAdmission::Invalid;
    };
    let mut owner_id = None;
    for (effective_id, entity_dir) in &map.resolved {
        let Ok(Some(identity)) = read_entity_identity(journal_root, entity_dir) else {
            return OwnerAdmission::Invalid;
        };
        let entity = JournalEntity {
            id: effective_id.clone(),
            value: identity.value().clone(),
        };
        if entity.value.get("is_principal") == Some(&Value::Bool(true))
            && is_admissible_person(&entity)
            && owner_id.replace(effective_id.clone()).is_some()
        {
            return OwnerAdmission::Invalid;
        }
    }
    owner_id.map_or(OwnerAdmission::Invalid, OwnerAdmission::Admitted)
}

/// Require that a caller's owner-artifact target is the admitted owner.
pub fn require_admitted_owner_target(
    journal_root: &Path,
    requested_id: &str,
) -> Result<String, OwnerAdmissionFailure> {
    match admitted_owner_id(journal_root) {
        OwnerAdmission::Invalid => Err(OwnerAdmissionFailure::IdentityInvalid),
        OwnerAdmission::Admitted(admitted_id) if admitted_id == requested_id => Ok(admitted_id),
        OwnerAdmission::Admitted(admitted_id) => Err(OwnerAdmissionFailure::TargetMismatch {
            requested_id: requested_id.to_owned(),
            admitted_id,
        }),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};

    use serde_json::{Value, json};

    use super::{
        OwnerAdmission, OwnerAdmissionFailure, admitted_owner_id, require_admitted_owner_target,
    };

    static NEXT: AtomicUsize = AtomicUsize::new(0);

    struct Temp(PathBuf);

    impl Temp {
        fn new() -> Self {
            let path = PathBuf::from("/var/tmp").join(format!(
                "solstone-owner-admission-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(path.join("entities")).unwrap();
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn entity(root: &Path, directory: &str, value: Value) {
        let path = root.join("entities").join(directory);
        fs::create_dir_all(&path).unwrap();
        fs::write(path.join("entity.json"), value.to_string()).unwrap();
    }

    fn principal(kind: Option<&str>) -> Value {
        let mut value = json!({"id":"owner","name":"Owner","is_principal":true});
        if let Some(kind) = kind {
            value["type"] = json!(kind);
        }
        value
    }

    #[test]
    fn admits_only_an_unblocked_principal_person() {
        let journal = Temp::new();
        entity(journal.path(), "owner", principal(Some("Person")));

        assert_eq!(
            admitted_owner_id(journal.path()),
            OwnerAdmission::Admitted("owner".into())
        );
        assert_eq!(
            require_admitted_owner_target(journal.path(), "other"),
            Err(OwnerAdmissionFailure::TargetMismatch {
                requested_id: "other".into(),
                admitted_id: "owner".into(),
            })
        );
    }

    #[test]
    fn rejects_every_non_person_or_malformed_principal() {
        for (name, value) in [
            ("tool", principal(Some("Tool"))),
            ("project", principal(Some("Project"))),
            ("company", principal(Some("Company"))),
            ("missing-type", principal(None)),
            (
                "blocked",
                json!({"id":"owner","type":"Person","is_principal":true,"blocked":true}),
            ),
        ] {
            let journal = Temp::new();
            entity(journal.path(), name, value);
            assert_eq!(
                admitted_owner_id(journal.path()),
                OwnerAdmission::Invalid,
                "{name}"
            );
        }

        let malformed = Temp::new();
        let path = malformed.path().join("entities/malformed");
        fs::create_dir_all(&path).unwrap();
        fs::write(path.join("entity.json"), "{").unwrap();
        assert_eq!(admitted_owner_id(malformed.path()), OwnerAdmission::Invalid);
    }

    #[test]
    fn rejects_absent_or_ambiguous_principals() {
        let absent = Temp::new();
        entity(
            absent.path(),
            "person",
            json!({"id":"person","type":"Person","is_principal":false}),
        );
        assert_eq!(admitted_owner_id(absent.path()), OwnerAdmission::Invalid);

        let ambiguous = Temp::new();
        entity(ambiguous.path(), "one", principal(Some("Person")));
        entity(
            ambiguous.path(),
            "two",
            json!({"id":"two","type":"Person","is_principal":true}),
        );
        assert_eq!(admitted_owner_id(ambiguous.path()), OwnerAdmission::Invalid);
    }

    #[test]
    fn ignores_a_principal_collision_loser() {
        let journal = Temp::new();
        entity(
            journal.path(),
            "owner",
            json!({"type":"Person","is_principal":true}),
        );
        entity(
            journal.path(),
            "z_winner",
            json!({"id":"owner","type":"Person","is_principal":false}),
        );

        assert_eq!(admitted_owner_id(journal.path()), OwnerAdmission::Invalid);
    }
}
