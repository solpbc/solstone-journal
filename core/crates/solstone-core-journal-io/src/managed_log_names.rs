// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Stable, reserved Windows component names for managed operational-log references.

#![allow(
    dead_code,
    reason = "the managed-log substrate is intentionally inactive"
)]

use std::ffi::OsString;

use crate::name_admission::check_portable_component;

const FNV_OFFSET_BASIS_128: u128 = 0x6c62_272e_07bb_0142_62b8_2175_6295_c58d;
const FNV_PRIME_128: u128 = 0x0000_0000_0100_0000_0000_0000_0000_013b;

const ROOT_ALIAS_DOMAIN: &[u8] = b"solstone-managed-log/root-alias/v1\0";
const PAYLOAD_DOMAIN: &[u8] = b"solstone-managed-log/canonical-payload/v1\0";

const ROOT_ALIAS_PREFIX: &str = "!solstone-ml-r-";
const DAY_ALIAS_PREFIX: &str = "!solstone-ml-d-";
const PAYLOAD_PREFIX: &str = "!solstone-ml-p-";
const LOCK_PREFIX: &str = "!solstone-ml-l-";

/// The retained alias role selects a persistent lock namespace.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ManagedLogAliasRole {
    Root,
    Day,
}

/// Derive the root alias solely from the caller's logical name.
pub(crate) fn root_alias_name(name: &str) -> OsString {
    reserved_component(ROOT_ALIAS_PREFIX, name_token(name), ".ref")
}

/// Derive the day alias from the same logical-name token as the root alias.
///
/// The day directory, rather than this component, supplies the day coordinate.
pub(crate) fn day_alias_name(name: &str) -> OsString {
    reserved_component(DAY_ALIAS_PREFIX, name_token(name), ".ref")
}

/// Derive a canonical payload component from the ordered logical coordinates.
pub(crate) fn canonical_payload_name(reference: &str, name: &str) -> OsString {
    reserved_component(PAYLOAD_PREFIX, pair_token(reference, name), ".log")
}

/// Derive a stable lock entry for one alias role without a per-day root lock.
pub(crate) fn alias_lock_name(role: ManagedLogAliasRole, name: &str) -> OsString {
    let role = match role {
        ManagedLogAliasRole::Root => "r",
        ManagedLogAliasRole::Day => "d",
    };
    let component = format!("{LOCK_PREFIX}{role}-{}.lock", name_token(name));
    check_portable_component(&component)
        .expect("managed-log lock derivation emits an admitted component");
    OsString::from(component)
}

pub(crate) fn name_token(name: &str) -> String {
    hex_token(fnv1a_128(ROOT_ALIAS_DOMAIN, &[name.as_bytes()]))
}

fn pair_token(reference: &str, name: &str) -> String {
    hex_token(fnv1a_128(
        PAYLOAD_DOMAIN,
        &[reference.as_bytes(), name.as_bytes()],
    ))
}

fn reserved_component(prefix: &str, token: String, suffix: &str) -> OsString {
    let component = format!("{prefix}{token}{suffix}");
    check_portable_component(&component)
        .expect("managed-log name derivation emits an admitted component");
    OsString::from(component)
}

/// FNV-1a 128 as published by the FNV project (public domain).
///
/// The explicit domain, field count, and little-endian field lengths make this
/// an algorithm-stable byte format rather than an implementation-defined Rust hash.
fn fnv1a_128(domain: &[u8], fields: &[&[u8]]) -> u128 {
    let mut hash = FNV_OFFSET_BASIS_128;
    absorb(&mut hash, domain);
    absorb(&mut hash, &(fields.len() as u64).to_le_bytes());
    for field in fields {
        absorb(&mut hash, &(field.len() as u64).to_le_bytes());
        absorb(&mut hash, field);
    }
    hash
}

fn absorb(hash: &mut u128, bytes: &[u8]) {
    for byte in bytes {
        *hash ^= u128::from(*byte);
        *hash = hash.wrapping_mul(FNV_PRIME_128);
    }
}

fn hex_token(value: u128) -> String {
    format!("{value:032x}")
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;
    use crate::atomic::{ATOMIC_CANDIDATE_MARKER, publication_candidate_name};

    #[test]
    fn aliases_share_a_name_token_but_payloads_use_both_coordinates() {
        let root = root_alias_name("component");
        let day = day_alias_name("component");
        assert_eq!(
            &root.to_string_lossy()[15..47],
            &day.to_string_lossy()[15..47]
        );
        assert_ne!(
            canonical_payload_name("one", "component"),
            canonical_payload_name("two", "component")
        );
    }

    #[test]
    fn framing_and_domains_keep_known_coordinates_distinct() {
        assert_ne!(pair_token("a", "bc"), pair_token("ab", "c"));
        assert_ne!(name_token("a"), pair_token("a", "b"));
    }

    #[test]
    fn hostile_values_only_affect_safe_tokens() {
        for value in [
            "maintenance:<task>",
            "Name",
            "name",
            "a/b",
            "a\\b",
            "stream:zone",
            ".",
            "..",
            "CON",
            "NUL",
            "COM1",
            "trailing. ",
            "",
            "e\u{301}",
            "é",
            &"λ".repeat(128),
        ] {
            for derived in [
                root_alias_name(value),
                day_alias_name(value),
                canonical_payload_name("reference", value),
                alias_lock_name(ManagedLogAliasRole::Root, value),
                alias_lock_name(ManagedLogAliasRole::Day, value),
            ] {
                check_portable_component(&derived.to_string_lossy()).unwrap();
            }
        }
    }

    #[test]
    fn all_reserved_role_names_are_cartesian_disjoint_under_windows_case_folding() {
        let fixture =
            include_str!("../tests/fixtures/windows-compare-string-ordinal-ascii-corpus-260823.md");
        let mut values = fixture
            .split("```")
            .nth(1)
            .expect("fixture has a mapping code block")
            .split(',')
            .filter_map(|mapping| mapping.split_once(':').map(|(value, _)| value))
            .map(str::to_owned)
            .collect::<BTreeSet<_>>();
        values.extend(
            [
                "maintenance:<task>",
                "Name",
                "name",
                "a/b",
                "a\\b",
                "stream:zone",
                ".",
                "..",
                "CON",
                "NUL",
                "COM1",
                "trailing. ",
                "",
                "e\u{301}",
                "é",
                &"λ".repeat(128),
            ]
            .into_iter()
            .map(str::to_owned),
        );

        let mut names = Vec::new();
        for (index, value) in values.into_iter().enumerate() {
            let payload = canonical_payload_name("reference", &value);
            // Root and day are different physical parents. Within each parent,
            // every managed role, direct ordinary log, and real staging shape
            // must stay distinct under CompareStringOrdinal's ASCII fold.
            names.extend([
                ("root", root_alias_name(&value)),
                ("root", alias_lock_name(ManagedLogAliasRole::Root, &value)),
                ("root", OsString::from(format!("ordinary-{index}.log"))),
                ("day", day_alias_name(&value)),
                ("day", alias_lock_name(ManagedLogAliasRole::Day, &value)),
                ("day", payload.clone()),
                (
                    "day",
                    publication_candidate_name(
                        &payload,
                        ATOMIC_CANDIDATE_MARKER,
                        &[index as u128, 1],
                    ),
                ),
            ]);
        }
        for (index, (left_parent, left)) in names.iter().enumerate() {
            for (right_parent, right) in &names[index + 1..] {
                if left_parent == right_parent {
                    assert_ne!(
                        left.to_string_lossy().to_ascii_lowercase(),
                        right.to_string_lossy().to_ascii_lowercase()
                    );
                }
            }
        }
    }
}
