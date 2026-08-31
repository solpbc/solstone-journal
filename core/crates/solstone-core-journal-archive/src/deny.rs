// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

/// Patterns omitted from a portable journal archive.
///
/// Copied from `BACKUP_EXCLUDES` in
/// `core/crates/solstone-core-backup-runtime/src/engine.rs:36-56`.
/// Intentionally independent of that list: portable adds trees and `*.key`;
/// backup keeps `health/` as a tree. Do not pin the two together in a test.
pub(crate) const PORTABLE_DENY: &[&str] = &[
    "*.sqlite*",
    "indexer",
    "cache",
    ".cache",
    ".removing_*",
    "*.sock",
    "*.pid",
    "*.port",
    "*.lock",
    "*.tmp",
    ".tmp*",
    "brain.json",
    "brain.log",
    "brain-fingerprint.key",
    "brain-refresh.lease",
    "supervisor.ready",
    "supervisor.start_time",
    "parakeet-cpp.placement",
    "scheduler.json",
    // authored portable extras (not in BACKUP_EXCLUDES)
    "*.key",
    // authored top-level tree prunes — trailing slash is our shape
    "config/",
    "link/",
    "mcp-endpoint/",
    "tokens/",
    "awareness/",
    "apps/",
    "backup/",
    "solstone/",
    // merge scratch under imports/ (prefix, not restic basename)
    "imports/archive-merge-work/",
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DenyAction {
    Include,
    Skip,
    TreePrune,
}

pub(crate) fn deny_top_level(name: &str) -> DenyAction {
    for pattern in PORTABLE_DENY {
        if let Some(tree) = pattern.strip_suffix('/')
            && !tree.contains('/')
            && tree == name
        {
            return DenyAction::TreePrune;
        }
    }
    if basename_denied(name) {
        return DenyAction::Skip;
    }
    DenyAction::Include
}

pub(crate) fn deny_member(member: &str) -> bool {
    prefix_denied(member) || basename_denied(final_component(member))
}

fn prefix_denied(member: &str) -> bool {
    for pattern in PORTABLE_DENY {
        if !pattern.contains('/') {
            continue;
        }
        let prefix = pattern.strip_suffix('/').unwrap_or(pattern);
        if member == prefix || member.starts_with(&format!("{prefix}/")) {
            return true;
        }
    }
    false
}

fn basename_denied(name: &str) -> bool {
    PORTABLE_DENY
        .iter()
        .filter(|pattern| !pattern.contains('/'))
        .any(|pattern| deny_basename(pattern, name))
}

fn final_component(member: &str) -> &str {
    member.rsplit('/').next().unwrap_or(member)
}

/// Glob `pattern` against a single path component. `*` does not cross `/`.
pub(crate) fn deny_basename(pattern: &str, name: &str) -> bool {
    glob_bytes(pattern.as_bytes(), name.as_bytes())
}

fn glob_bytes(pattern: &[u8], name: &[u8]) -> bool {
    let mut parts = pattern.split(|byte| *byte == b'*');
    let Some(first) = parts.next() else {
        return name.is_empty();
    };
    let Some(mut rest) = name.strip_prefix(first) else {
        return false;
    };
    let Some(last) = parts.next_back() else {
        return rest.is_empty();
    };
    for piece in parts {
        if piece.is_empty() {
            continue;
        }
        match rest.windows(piece.len()).position(|window| window == piece) {
            Some(index) => rest = &rest[index + piece.len()..],
            None => return false,
        }
    }
    if last.is_empty() {
        true
    } else {
        rest.ends_with(last)
    }
}

#[cfg(test)]
mod tests {
    use super::{DenyAction, deny_basename, deny_member, deny_top_level};

    #[test]
    fn deny_basename_covers_restic_shapes() {
        assert!(deny_basename("*.sqlite*", "journal.sqlite"));
        assert!(deny_basename("*.sqlite*", "journal.sqlite-wal"));
        assert!(deny_basename("*.lock", "foo.lock"));
        assert!(deny_basename(".tmp*", ".tmp123"));
        assert!(deny_basename(".removing_*", ".removing_x"));
        assert!(deny_basename("brain.json", "brain.json"));
        assert!(!deny_basename("brain.json", "brain.json.bak"));
        assert!(deny_basename("indexer", "indexer"));
        assert!(!deny_basename("indexer", "indexer-backup"));
        assert!(deny_basename("*.key", "brain-fingerprint.key"));
        assert!(!deny_basename("*.sqlite*", "journal/sqlite"));
    }

    #[test]
    fn star_does_not_cross_slash_because_basename_is_the_subject() {
        assert!(!deny_member("chronicle/foo.sqlite/keep.txt"));
        assert!(deny_member("chronicle/20260101/foo.sqlite"));
        assert!(deny_basename("*.sqlite*", "foo.sqlite"));
    }

    #[test]
    fn top_level_tree_prune_is_not_any_depth() {
        assert_eq!(deny_top_level("config"), DenyAction::TreePrune);
        assert_eq!(deny_top_level("apps"), DenyAction::TreePrune);
        assert_eq!(deny_top_level("backup"), DenyAction::TreePrune);
        assert_eq!(deny_top_level("mcp-endpoint"), DenyAction::TreePrune);
        assert_eq!(deny_top_level("solstone"), DenyAction::TreePrune);
        assert_eq!(deny_top_level("chronicle"), DenyAction::Include);
        assert_eq!(deny_top_level("identity"), DenyAction::Include);
        assert_eq!(deny_top_level("health"), DenyAction::Include);
        assert_eq!(deny_top_level("indexer"), DenyAction::Skip);
        assert!(!deny_member("chronicle/foo/apps/x"));
        assert!(deny_member("apps/observer/x.json"));
        assert!(deny_member("config/journal.json"));
        assert!(deny_member("mcp-endpoint/pop.ed25519.pk8"));
        assert!(deny_member("mcp-endpoint/.create.lock"));
        assert!(!deny_member("chronicle/mcp-endpoint/keep.bin"));
        assert!(!deny_member(
            "chronicle/20260101/mcp.agent/120000_1/interaction.json"
        ));
    }

    #[test]
    fn prefix_prune_is_root_relative() {
        assert!(deny_member("imports/archive-merge-work"));
        assert!(deny_member("imports/archive-merge-work/extract-1/x"));
        assert!(!deny_member("imports/other/x"));
        assert!(!deny_member("other/imports/archive-merge-work/x"));
    }

    #[test]
    fn health_durable_audit_stays_and_brain_key_does_not() {
        assert!(!deny_member("health/retention.log"));
        assert!(!deny_member("health/pruning-runs/x.jsonl"));
        assert!(deny_member("health/brain.json"));
        assert!(deny_member("health/brain-fingerprint.key"));
        assert!(deny_member("backup/hosted/binding.json"));
        assert!(deny_member("solstone/apps/x"));
    }
}
