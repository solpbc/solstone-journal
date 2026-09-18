// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;

use serde_json::Value;
use solstone_core_journal_io::durability::{
    ArtifactId, DurableObservation, DurableRead, observe_json_durable, read_json_durable, set_aside,
};

use super::error::FacetStoreError;
use super::paths::declaration_path;

/// Complete read-compatible facet declaration.
#[derive(Debug, Clone, PartialEq)]
pub struct FacetDeclarationSnapshot {
    pub title: String,
    pub description: String,
    pub color: String,
    pub emoji: String,
    pub icon: Option<String>,
    pub muted: Option<bool>,
    value: Value,
}

impl FacetDeclarationSnapshot {
    /// The original durable declaration, including unknown fields.
    pub fn value(&self) -> &Value {
        &self.value
    }

    pub(super) fn into_value(self) -> Value {
        self.value
    }
}

/// Read one facet declaration without persisting a fallback identity.
pub fn read_facet_declaration(
    journal_root: &Path,
    facet_dir: &str,
) -> Result<Option<FacetDeclarationSnapshot>, FacetStoreError> {
    let path = declaration_path(journal_root, facet_dir)?;
    let value: Value =
        match read_json_durable(ArtifactId::FacetDeclaration, &path).map_err(|e| {
            FacetStoreError::from(solstone_core_journal_io::ReadError::Io {
                path: path.clone(),
                source: e,
            })
        })? {
            DurableRead::Present(val) => val,
            DurableRead::Absent | DurableRead::SetAside(_) | DurableRead::Unreadable { .. } => {
                return Ok(None);
            }
        };
    if value.is_null() {
        return Ok(None);
    }
    let Some(object) = value.as_object() else {
        let _ = set_aside(&path);
        return Ok(None);
    };
    Ok(Some(FacetDeclarationSnapshot {
        title: string_field(object.get("title")),
        description: string_field(object.get("description")),
        color: string_field(object.get("color")),
        emoji: string_field(object.get("emoji")),
        icon: non_empty_string(object.get("icon")).map(str::to_owned),
        muted: object.get("muted").and_then(Value::as_bool),
        value,
    }))
}

/// Closed deterministic facet-identity outcomes vs untyped read failures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FacetIdentityError {
    Absent,
    Invalid,
    Replaced,
    Failed(String),
}

impl FacetIdentityError {
    pub fn into_review_owner_error(self) -> solstone_core_entity::ReviewOwnerError {
        use solstone_core_entity::{ReviewOwnerConflictKind, ReviewOwnerError};
        match self {
            Self::Absent => ReviewOwnerError::conflict(
                ReviewOwnerConflictKind::OwningFacetChanged,
                "conflict: owning facet no longer exists",
            ),
            Self::Invalid => ReviewOwnerError::conflict(
                ReviewOwnerConflictKind::OwningFacetChanged,
                "conflict: owning facet has no valid stable identity",
            ),
            Self::Replaced => ReviewOwnerError::conflict(
                ReviewOwnerConflictKind::OwningFacetChanged,
                "conflict: owning facet was replaced after preparation",
            ),
            Self::Failed(detail) => ReviewOwnerError::failed(detail),
        }
    }
}

impl std::fmt::Display for FacetIdentityError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Absent => formatter.write_str("conflict: owning facet no longer exists"),
            Self::Invalid => {
                formatter.write_str("conflict: owning facet has no valid stable identity")
            }
            Self::Replaced => {
                formatter.write_str("conflict: owning facet was replaced after preparation")
            }
            Self::Failed(detail) => formatter.write_str(detail),
        }
    }
}

impl From<FacetIdentityError> for String {
    fn from(error: FacetIdentityError) -> Self {
        error.to_string()
    }
}

impl From<FacetIdentityError> for solstone_core_entity::ReviewOwnerError {
    fn from(error: FacetIdentityError) -> Self {
        error.into_review_owner_error()
    }
}

/// Stable lifecycle identity for a prepared facet-owned mutation. This read
/// never allocates an identity for a missing or unadopted declaration.
pub fn facet_write_identity(root: &Path, facet: &str) -> Result<String, FacetIdentityError> {
    let declaration = read_facet_declaration(root, facet)
        .map_err(|e| FacetIdentityError::Failed(e.to_string()))?
        .ok_or(FacetIdentityError::Absent)?;
    let id = declaration
        .value()
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| super::facet_id::is_well_formed_facet_id(id))
        .ok_or(FacetIdentityError::Invalid)?;
    Ok(id.to_owned())
}

/// The caller holds facet trust through this check, mutation and receipt.
pub fn require_facet_write_identity(
    root: &Path,
    facet: &str,
    expected: &str,
) -> Result<(), FacetIdentityError> {
    if facet_write_identity(root, facet)? != expected {
        return Err(FacetIdentityError::Replaced);
    }
    Ok(())
}

/// Non-mutating facet identity observation for the review owner lifecycle.
/// Missing is Absent; a present object with a missing/invalid id is Invalid;
/// malformed, non-object, unreadable, or I/O is Failed. Never sets aside.
pub fn observe_facet_write_identity(
    root: &Path,
    facet: &str,
) -> Result<String, FacetIdentityError> {
    let path =
        declaration_path(root, facet).map_err(|e| FacetIdentityError::Failed(e.to_string()))?;
    match observe_json_durable::<Value>(ArtifactId::FacetDeclaration, &path) {
        DurableObservation::Absent => Err(FacetIdentityError::Absent),
        DurableObservation::Malformed { path, source } => Err(FacetIdentityError::Failed(format!(
            "{}: {source}",
            path.display()
        ))),
        DurableObservation::Unreadable { path, source } => Err(FacetIdentityError::Failed(
            format!("{}: {source}", path.display()),
        )),
        DurableObservation::Present(value) => {
            let Some(object) = value.as_object() else {
                return Err(FacetIdentityError::Failed(format!(
                    "{}: facet declaration is not an object",
                    path.display()
                )));
            };
            let id = object
                .get("id")
                .and_then(Value::as_str)
                .filter(|id| super::facet_id::is_well_formed_facet_id(id))
                .ok_or(FacetIdentityError::Invalid)?;
            Ok(id.to_owned())
        }
    }
}

/// Non-mutating expected-id check for the review owner lifecycle.
pub fn require_observed_facet_write_identity(
    root: &Path,
    facet: &str,
    expected: &str,
) -> Result<(), FacetIdentityError> {
    if observe_facet_write_identity(root, facet)? != expected {
        return Err(FacetIdentityError::Replaced);
    }
    Ok(())
}

fn string_field(value: Option<&Value>) -> String {
    value.and_then(Value::as_str).unwrap_or_default().to_owned()
}

fn non_empty_string(value: Option<&Value>) -> Option<&str> {
    value
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::FacetIdentityError;
    use solstone_core_entity::{ReviewOwnerConflictKind, ReviewOwnerError};

    #[test]
    fn absent_replaced_invalid_are_owning_facet_conflicts() {
        for error in [
            FacetIdentityError::Absent,
            FacetIdentityError::Invalid,
            FacetIdentityError::Replaced,
        ] {
            let mapped = error.into_review_owner_error();
            assert_eq!(
                mapped.kind(),
                Some(ReviewOwnerConflictKind::OwningFacetChanged)
            );
        }
    }

    #[test]
    fn read_io_malformed_facet_identity_stays_failed() {
        let mapped = FacetIdentityError::Failed("read failed".into()).into_review_owner_error();
        assert!(matches!(mapped, ReviewOwnerError::Failed { .. }));
        assert_eq!(mapped.kind(), None);
    }

    #[test]
    fn observe_facet_identity_leaves_malformed_and_unreadable_bytes_in_place() {
        use super::observe_facet_write_identity;
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("facets/work/facet.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"{").unwrap();
        let error = observe_facet_write_identity(root.path(), "work").unwrap_err();
        assert!(matches!(error, FacetIdentityError::Failed(_)), "{error}");
        assert_eq!(std::fs::read(&path).unwrap(), b"{");
        assert_eq!(
            std::fs::read_dir(path.parent().unwrap())
                .unwrap()
                .filter_map(Result::ok)
                .filter(|entry| entry.file_name().to_string_lossy().contains("wedged"))
                .count(),
            0
        );

        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        let error = observe_facet_write_identity(root.path(), "work").unwrap_err();
        assert!(matches!(error, FacetIdentityError::Failed(_)), "{error}");
        assert!(path.is_dir());
    }
}
