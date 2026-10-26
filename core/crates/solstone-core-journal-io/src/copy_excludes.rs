// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Journal-relative operational paths omitted from backups and portable copies.
//!
//! Raw content may have any plain filename, including writer-shaped temps and
//! locks. Keep ambiguous inert files: a name alone cannot establish ownership.

/// Root-relative patterns. `*` spans one component; `**` spans directories.
/// A matching directory also omits its descendants. The numeric day selector
/// uses eight `[0-9]` classes, shared with restic's glob syntax.
pub const COPY_EXCLUDES: &[&str] = &[
    "indexer",
    "cache",
    ".cache",
    "mcp-endpoint",
    "config/chatgpt-sign-in.json*",
    // Deletion pivots must never be restored, including ambiguous raw names.
    "**/.removing_*",
    // These namespaces are application-owned, unlike raw segment filenames.
    "health/sync",
    "health/brain.json",
    // Local restore obligation stays with its target.
    crate::whole_journal_restore::WHOLE_JOURNAL_RESTORE_FILE,
    "health/!solstone-claim-00000000-*",
    "health/brain.log",
    "health/brain-fingerprint.key",
    "health/brain-refresh.lease",
    "health/supervisor.ready",
    "health/supervisor.start_time",
    // The Windows resident's own process identity and Callosum pipe file,
    // its counterparts of the pid, start time and socket above. Another
    // machine's copies would name a process this one is not running.
    "health/supervisor.process_instance",
    "health/callosum.pipe-*",
    "health/parakeet-cpp.placement",
    "health/scheduler.json",
    "health/**/*.sock",
    "health/**/*.pid",
    "health/**/*.port",
    "health/**/*.lock",
    "health/**/*.tmp",
    "health/**/.tmp*",
    // Per-day processing claims are process-local. Keep durable health records
    // and raw content, including a capture stream literally named health.
    "chronicle/[0-9][0-9][0-9][0-9][0-9][0-9][0-9][0-9]/health/daily-units/*.lock",
    "chronicle/[0-9][0-9][0-9][0-9][0-9][0-9][0-9][0-9]/health/segment-think/**/*.lock",
    "chronicle/[0-9][0-9][0-9][0-9][0-9][0-9][0-9][0-9]/health/talent-provenance/**/*.lock",
    "chronicle/[0-9][0-9][0-9][0-9][0-9][0-9][0-9][0-9]/health/stream.updated.lock",
    "chronicle/[0-9][0-9][0-9][0-9][0-9][0-9][0-9][0-9]/health/daily.updated.lock",
    "chronicle/[0-9][0-9][0-9][0-9][0-9][0-9][0-9][0-9]/health/sense-day.lease",
    // Atomic candidates can contain credentials before their final rename.
    "config/**/*.lock",
    "config/**/*.tmp",
    "config/**/.tmp*",
    "link/**/*.lock",
    "link/**/*.tmp",
    "link/**/.tmp*",
    "backup/**/*.lock",
    "backup/**/*.tmp",
    "backup/**/.tmp*",
];

/// Whether a slash-separated journal-relative path is operational copy state.
pub fn copy_excluded(member: &str) -> bool {
    let components: Vec<_> = member.split('/').collect();
    COPY_EXCLUDES.iter().any(|pattern| {
        let pattern: Vec<_> = pattern.split('/').collect();
        prefix_matches(&pattern, &components)
    })
}

/// Exact database filenames in mixed owner/application namespaces.
pub const LEGACY_DAY_DATABASES: &[&str] = &["indexer.sqlite", "transcripts.sqlite"];
pub const BODY_DATABASES: &[&str] = &["health-dedupe.sqlite", ".health-dedupe.sqlite.rebuild"];
pub const SQLITE_SUFFIXES: &[&str] = &["", "-wal", "-shm", "-journal"];

/// A directory with one of these names is owner content, never a database prune.
pub fn copy_file_excluded(member: &str) -> bool {
    let parts: Vec<_> = member.split('/').collect();
    let (name, databases) = match parts.as_slice() {
        ["imports", name] => (*name, BODY_DATABASES),
        ["chronicle", day, "indexer", name]
            if day.len() == 8 && day.bytes().all(|byte| byte.is_ascii_digit()) =>
        {
            (*name, LEGACY_DAY_DATABASES)
        }
        _ => return false,
    };
    databases.iter().any(|database| {
        SQLITE_SUFFIXES.iter().any(|suffix| {
            name.strip_prefix(database)
                .is_some_and(|tail| tail == *suffix)
        })
    })
}

fn prefix_matches(pattern: &[&str], path: &[&str]) -> bool {
    match pattern.split_first() {
        None => true,
        Some((&"**", rest)) => (0..=path.len()).any(|skip| prefix_matches(rest, &path[skip..])),
        Some((first, rest)) => path.split_first().is_some_and(|(name, tail)| {
            component_matches(first, name) && prefix_matches(rest, tail)
        }),
    }
}

fn component_matches(pattern: &str, name: &str) -> bool {
    // Keep the journal day selector identical to restic's numeric glob.
    if pattern == "[0-9][0-9][0-9][0-9][0-9][0-9][0-9][0-9]" {
        return name.len() == 8 && name.bytes().all(|byte| byte.is_ascii_digit());
    }
    let mut parts = pattern.split('*');
    let first = parts.next().unwrap_or_default();
    let Some(mut rest) = name.strip_prefix(first) else {
        return false;
    };
    let pieces: Vec<_> = parts.collect();
    let Some((last, middle)) = pieces.split_last() else {
        return rest.is_empty();
    };
    for part in middle {
        let Some(index) = rest.find(part) else {
            return false;
        };
        rest = &rest[index + part.len()..];
    }
    rest.ends_with(last)
}

#[cfg(test)]
mod tests {
    use super::{copy_excluded, copy_file_excluded};

    #[test]
    fn database_files_match_only_exact_operational_slots() {
        for member in [
            "imports/health-dedupe.sqlite",
            "imports/.health-dedupe.sqlite.rebuild-journal",
            "chronicle/20260101/indexer/indexer.sqlite-wal",
            "chronicle/20260101/indexer/transcripts.sqlite-shm",
        ] {
            assert!(copy_file_excluded(member), "{member}");
        }
        for member in [
            "imports/health-dedupe.sqlite-notes",
            "imports/health-dedupe.sqlite/original.key",
            "chronicle/20260101/indexer/120000_60.sqlite",
            "chronicle/20260101/indexer/indexer.sqlite/original.key",
            "chronicle/20260101/indexer/indexer.sqlite.lock",
        ] {
            assert!(!copy_file_excluded(member), "{member}");
        }
    }

    #[test]
    fn operational_paths_are_rooted_and_owner_names_survive() {
        for path in [
            "indexer/journal.sqlite-wal",
            "cache/providers/model",
            "health/brain-fingerprint.key",
            "health/whole-journal-restore.json",
            "health/callosum.sock",
            "health/callosum.pipe-secret",
            "health/supervisor.process_instance",
            "health/locks/backup-activity.lock",
            "chronicle/20261003/health/daily-units/u-entities%3Aentity_suggest.f-personal.execution.lock",
            "chronicle/20261003/health/daily-units/u-entities%3Aentity_suggest.f-personal.json.lock",
            "chronicle/20260929/health/segment-think/import.document/120001_0.lock",
            "chronicle/20260929/health/talent-provenance/activity-inputs/personal/activity.json.lock",
            "chronicle/20260929/health/stream.updated.lock",
            "chronicle/20260929/health/daily.updated.lock",
            "chronicle/20260929/health/sense-day.lease",
            "health/sync/state",
            "mcp-endpoint/pop.ed25519.pk8",
            "config/chatgpt-sign-in.json.unreadable-1",
            "config/.tmp_123_4.tmp",
            "config/_stage_123_4.tmp",
            "link/ca/.tmp_123_4.tmp",
            "backup/hosted/.tmp_123_4.tmp",
            "chronicle/20260101/source/.removing_120000_60/tombstone.json",
            ".removing_old/file",
            "chronicle/20260101/120000_60/.removing_original",
            "imports/item/deep/.removing_old/file",
        ] {
            assert!(copy_excluded(path), "operational path included: {path}");
        }
        for path in [
            "entities/cache/entity.json",
            "facets/indexer/facet.json",
            "imports/item/source.key",
            "imports/item/cache/a.sqlite",
            "imports/health-dedupe.sqlite/original.key",
            "imports/health-dedupe.sqlite-notes/original.key",
            "chronicle/20260101/indexer/120000_60.sqlite/original.key",
            "chronicle/20260101/indexer/indexer.sqlite/original.key",
            "imports/item/.removing",
            "imports/item/notes.removing_original",
            "chronicle/20260101/indexer/120000_60/original.key",
            "chronicle/20260101/source/120000_60/brain.json",
            "chronicle/20260101/source/120000_60/audio.jsonl.lock",
            "chronicle/20260101/source/120000_60/.tmp_123_4.tmp",
            "entities/indexer/brain-fingerprint.key",
            "facets/cache/note.tmp",
            "health/retention.log",
            "health/pruning-runs/run.jsonl",
            "chronicle/20260101/health/talent-provenance/run.json",
            "chronicle/20261003/health/daily-units/u-entities%3Aentity_suggest.f-personal.json",
            "chronicle/20261003/import.audio/120001_7/owner.execution.lock",
            "chronicle/20261003/health/daily-units-original/owner.execution.lock",
            "chronicle/20260929/health/segment-think-original/owner.lock",
            "chronicle/20260929/health/120001_0/owner.lock",
            "chronicle/20260929/health/talent-provenance/activity-inputs/personal/activity.json",
            "chronicle/20260929/health/stream.updated",
            "chronicle/20260929/health/daily.updated",
            "chronicle/20260929/health/sense-day.lease-notes",
            "chronicle/notes/health/segment-think/import.document/120001_0.lock",
            "chronicle/notes/health/daily-units/owner.execution.lock",
            "chronicle/2026a003/health/daily-units/owner.execution.lock",
            "chronicle/202610031/health/daily-units/owner.execution.lock",
        ] {
            assert!(!copy_excluded(path), "owner path excluded: {path}");
        }
    }
}
