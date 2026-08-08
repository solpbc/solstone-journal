// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Shared admission guard for entities that may become speaker labels.

/// Return whether an entity's raw type is exactly the speaker-admissible type.
pub(crate) fn is_admissible_person(entity_type: Option<&str>) -> bool {
    entity_type == Some("Person")
}

#[cfg(test)]
mod tests {
    use super::is_admissible_person;

    #[test]
    fn ac4_person_guard_is_an_exact_allowlist() {
        assert!(is_admissible_person(Some("Person")));
        assert!(!is_admissible_person(Some("Human")));
        assert!(!is_admissible_person(Some("person")));
        assert!(!is_admissible_person(None));
    }
}
