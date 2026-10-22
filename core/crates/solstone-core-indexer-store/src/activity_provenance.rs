// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Private proof that an activity chunk was indexed from one exact file.
//!
//! One row per activity chunk, keyed by `chunks.rowid`. The digest is the
//! SHA-256 of the file bytes `produce_chunks` consumed. A proof row must not
//! outlive that chunk row id, and it must not be attached to a row the writer
//! did not commit.

use std::fs;
use std::path::Path;

use rusqlite::{Connection, params};
use sha2::{Digest, Sha256};
use solstone_core_format::content::{Family, JsonObject, produce_chunks};

use crate::StoreError;
use crate::db::sqlite_table_exists;

pub(crate) const CREATE_ACTIVITY_PROVENANCE: &str = "\
CREATE TABLE IF NOT EXISTS activity_provenance(
rowid INTEGER PRIMARY KEY,
digest BLOB NOT NULL,
hidden INTEGER NOT NULL CHECK (hidden IN (0, 1))
)";

pub(crate) const CREATE_ACTIVITY_MEMBERSHIP: &str = "\
CREATE TABLE IF NOT EXISTS activity_membership(
rowid INTEGER PRIMARY KEY,
is_activity INTEGER NOT NULL CHECK (is_activity IN (0, 1))
)";

// `chunks` is FTS5 with `path` unindexed, so a path predicate on it reads the
// whole index. These run once or twice per file on every scan; they reach a
// file's rows through the indexed `chunk_sources` mapping instead.
const MEMBERSHIP_MISSING_FOR_PATH: &str = "SELECT EXISTS(SELECT 1 FROM chunk_sources AS s JOIN chunks ON chunks.rowid=s.rowid LEFT JOIN activity_membership AS m ON m.rowid=s.rowid WHERE s.path=? AND chunks.agent='activity' AND m.rowid IS NULL)";
const CHUNK_COUNT_FOR_PATH: &str = "SELECT count(*) FROM chunk_sources WHERE path = ?";
const DIGESTS_FOR_PATH: &str = "\
SELECT CASE WHEN typeof(p.digest)='blob' AND length(p.digest)=32 THEN p.digest ELSE NULL END \
FROM chunk_sources AS s JOIN activity_provenance AS p ON p.rowid = s.rowid WHERE s.path = ?";

pub(crate) fn record_activity_membership(
    conn: &Connection,
    rowid: i64,
    activity: bool,
) -> Result<(), StoreError> {
    conn.execute(
        "INSERT INTO activity_membership(rowid, is_activity) VALUES (?, ?)",
        params![rowid, i64::from(activity)],
    )?;
    Ok(())
}

/// Batch the ambiguous legacy population once per scan.
pub(crate) fn missing_activity_memberships(
    conn: &Connection,
) -> Result<std::collections::BTreeSet<String>, StoreError> {
    let mut statement = conn.prepare("SELECT DISTINCT chunks.path FROM chunks LEFT JOIN activity_membership AS m ON m.rowid=chunks.rowid WHERE chunks.agent='activity' AND m.rowid IS NULL")?;
    Ok(statement
        .query_map([], |row| row.get(0))?
        .collect::<Result<_, _>>()?)
}

pub(crate) fn activity_membership_current(
    conn: &Connection,
    rel: &str,
) -> Result<bool, StoreError> {
    Ok(!conn.query_row(MEMBERSHIP_MISSING_FOR_PATH, [rel], |row| {
        row.get::<_, bool>(0)
    })?)
}

pub(crate) fn digest_bytes(bytes: &[u8]) -> [u8; 32] {
    let hashed = Sha256::digest(bytes);
    let mut digest = [0_u8; 32];
    digest.copy_from_slice(&hashed);
    digest
}

pub(crate) fn hidden_flag(source: Option<&JsonObject>) -> bool {
    source
        .and_then(|source| source.get("hidden"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

pub(crate) fn record_activity_provenance(
    conn: &Connection,
    rowid: i64,
    digest: &[u8; 32],
    hidden: bool,
) -> Result<(), StoreError> {
    conn.execute(
        "INSERT INTO activity_provenance(rowid, digest, hidden) VALUES (?, ?, ?)",
        params![rowid, digest.as_slice(), i64::from(hidden)],
    )?;
    Ok(())
}

pub(crate) fn delete_activity_provenance(
    conn: &Connection,
    rowids: &[i64],
) -> Result<(), StoreError> {
    for table in ["activity_provenance", "activity_membership"] {
        if rowids.is_empty() || !sqlite_table_exists(conn, table)? {
            continue;
        }
        let mut statement = conn.prepare_cached(&format!("DELETE FROM {table} WHERE rowid = ?"))?;
        for rowid in rowids {
            statement.execute([rowid])?;
        }
    }
    Ok(())
}

/// Whether the stored activity chunks still come from `path`'s current bytes.
///
/// A matching digest keeps the saved rendered text. Missing proof, a different
/// digest, or a file that now has chunks where none were stored is not current.
/// An empty file with no chunks is current, so a scan does not rewrite it on
/// every pass. A read error is not current: the caller indexes and records the
/// failure.
pub(crate) fn activity_source_current(
    conn: &Connection,
    rel: &str,
    path: &Path,
) -> Result<bool, StoreError> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(_) => return Ok(false),
    };
    if !sqlite_table_exists(conn, "activity_provenance")? {
        return Ok(false);
    }
    let digest = digest_bytes(text.as_bytes());
    let chunk_count: i64 = conn.query_row(CHUNK_COUNT_FOR_PATH, [rel], |row| row.get(0))?;
    if chunk_count == 0 {
        return Ok(!activity_file_has_chunk(rel, &text));
    }
    let mut statement = conn.prepare(DIGESTS_FOR_PATH)?;
    let digests = statement
        .query_map([rel], |row| row.get::<_, Option<Vec<u8>>>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    if digests.len() as i64 != chunk_count {
        return Ok(false);
    }
    // An index written before retired plans stopped being indexed still holds
    // their entries; the same bytes now render fewer chunks.
    if text.contains("\"anticipated\"")
        && produce_chunks(Family::Activity, rel, &text).chunks.len() as i64 != chunk_count
    {
        return Ok(false);
    }
    Ok(digests
        .iter()
        .all(|stored| stored.as_deref() == Some(digest.as_slice())))
}

fn activity_file_has_chunk(rel: &str, text: &str) -> bool {
    produce_chunks(Family::Activity, rel, text)
        .chunks
        .iter()
        .any(|chunk| !chunk.content.trim().is_empty())
}

#[cfg(test)]
mod plan_tests {
    use rusqlite::Connection;

    use super::{
        CHUNK_COUNT_FOR_PATH, CREATE_ACTIVITY_MEMBERSHIP, CREATE_ACTIVITY_PROVENANCE,
        DIGESTS_FOR_PATH, MEMBERSHIP_MISSING_FOR_PATH,
    };
    use crate::chunk_sources::{CREATE_CHUNK_SOURCES, CREATE_CHUNK_SOURCES_PATH_INDEX};

    #[test]
    fn per_file_activity_checks_never_scan_the_chunk_index() {
        let conn = Connection::open_in_memory().expect("open");
        conn.execute(
            "CREATE VIRTUAL TABLE chunks USING fts5(content, path UNINDEXED, day UNINDEXED, \
             facet UNINDEXED, agent UNINDEXED, stream UNINDEXED, idx UNINDEXED, time_bucket UNINDEXED)",
            [],
        )
        .expect("chunks");
        for create in [
            CREATE_CHUNK_SOURCES,
            CREATE_CHUNK_SOURCES_PATH_INDEX,
            CREATE_ACTIVITY_PROVENANCE,
            CREATE_ACTIVITY_MEMBERSHIP,
        ] {
            conn.execute(create, []).expect("schema");
        }
        for sql in [
            MEMBERSHIP_MISSING_FOR_PATH,
            CHUNK_COUNT_FOR_PATH,
            DIGESTS_FOR_PATH,
        ] {
            let mut statement = conn
                .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
                .expect("explain");
            let plan = statement
                .query_map(["facets/work/activities/20240101.jsonl"], |row| {
                    row.get::<_, String>(3)
                })
                .expect("plan")
                .collect::<Result<Vec<_>, _>>()
                .expect("plan rows");
            assert!(
                plan.iter().any(|step| step.contains("chunk_sources_path")),
                "{sql}: {plan:?}"
            );
            // A full FTS scan has an empty index constraint; a rowid probe is `0:=`.
            assert!(
                !plan
                    .iter()
                    .any(|step| step.ends_with("VIRTUAL TABLE INDEX 0:")),
                "{sql}: {plan:?}"
            );
        }
    }
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use std::fs::{self, File};
    use std::path::Path;
    use std::time::SystemTime;

    use rusqlite::{Connection, OptionalExtension, params};
    use sha2::{Digest, Sha256};

    use super::digest_bytes;
    use crate::db::{db_path, prune_by_paths, prune_chunks_by_stream, reset_index};
    use crate::scan::{rescan_file, scan_journal};
    use crate::test_support::reserve_temp_path;

    fn temp_root(name: &str) -> std::path::PathBuf {
        reserve_temp_path(&format!("solstone-activity-provenance-{name}"))
    }

    fn write(root: &Path, rel: &str, text: &str) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().expect("parent")).expect("create parent");
        fs::write(&path, text).expect("write activity");
    }

    fn file_digest(root: &Path, rel: &str) -> Vec<u8> {
        Sha256::digest(fs::read(root.join(rel)).expect("read activity")).to_vec()
    }

    fn chunk_content(conn: &Connection, rel: &str) -> Vec<String> {
        let mut statement = conn
            .prepare("SELECT content FROM chunks WHERE path = ? ORDER BY idx")
            .expect("prepare chunks");
        statement
            .query_map([rel], |row| row.get(0))
            .expect("query chunks")
            .collect::<Result<Vec<String>, _>>()
            .expect("chunk contents")
    }

    fn proofs(conn: &Connection, rel: &str) -> Vec<(Vec<u8>, i64)> {
        let mut statement = conn
            .prepare(
                "SELECT activity_provenance.digest, activity_provenance.hidden \
                 FROM chunks \
                 JOIN activity_provenance ON activity_provenance.rowid = chunks.rowid \
                 WHERE chunks.path = ? ORDER BY chunks.idx",
            )
            .expect("prepare proofs");
        statement
            .query_map([rel], |row| Ok((row.get(0)?, row.get(1)?)))
            .expect("query proofs")
            .collect::<Result<Vec<_>, _>>()
            .expect("proofs")
    }

    fn orphan_proofs(conn: &Connection) -> i64 {
        conn.query_row(
            "SELECT count(*) FROM activity_provenance AS p \
             LEFT JOIN chunks AS c ON c.rowid = p.rowid \
             WHERE c.rowid IS NULL",
            [],
            |row| row.get(0),
        )
        .expect("orphan proofs")
    }

    #[test]
    fn activity_scan_reindexes_when_bytes_change_and_keeps_saved_prose() {
        let root = temp_root("scan");
        let rel = "facets/work/activities/20240101.jsonl";
        let definitions = "facets/work/activities/activities.jsonl";
        let markdown = "facets/work/activities/20240101/notes/page.md";
        write(&root, rel, "{\"id\":\"one\",\"title\":\"First\"}\n");
        write(
            &root,
            definitions,
            "{\"id\":\"standup\",\"title\":\"Standup\"}\n",
        );
        write(&root, markdown, "# Beside\n\nnot an activity\n");
        let first = scan_journal(&root, true).expect("first scan");
        assert!(first.failed == 0, "{first:?}");

        let conn = Connection::open(db_path(&root)).expect("open");
        let digest = file_digest(&root, rel);
        let stored = proofs(&conn, rel);
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].0, digest);
        assert_eq!(stored[0].1, 0);
        let defined = proofs(&conn, definitions);
        assert_eq!(defined.len(), 1);
        assert_eq!(defined[0].1, 0);
        assert_eq!(defined[0].0, file_digest(&root, definitions));
        let markdown_proofs: i64 = conn
            .query_row(
                "SELECT count(*) FROM activity_provenance AS p \
                 JOIN chunks AS c ON c.rowid = p.rowid WHERE c.path = ?",
                [markdown],
                |row| row.get(0),
            )
            .expect("markdown proofs");
        assert_eq!(markdown_proofs, 0);
        let saved = chunk_content(&conn, rel);
        drop(conn);

        let unchanged = scan_journal(&root, false).expect("unchanged scan");
        assert_eq!(unchanged.indexed, 0);
        let conn = Connection::open(db_path(&root)).expect("open after unchanged");
        assert_eq!(chunk_content(&conn, rel), saved);
        assert_eq!(proofs(&conn, rel)[0].0, digest);
        drop(conn);

        let path = root.join(rel);
        let mtime = fs::metadata(&path)
            .expect("mtime")
            .modified()
            .expect("modified");
        write(
            &root,
            rel,
            "{\"id\":\"two\",\"title\":\"Second\",\"hidden\":true}\n",
        );
        File::options()
            .write(true)
            .open(&path)
            .expect("reopen")
            .set_modified(mtime)
            .expect("keep mtime");
        assert_eq!(
            fs::metadata(&path)
                .expect("mtime")
                .modified()
                .expect("modified"),
            mtime
        );
        let second = scan_journal(&root, false).expect("same-second scan");
        assert!(second.indexed >= 1, "{second:?}");
        let conn = Connection::open(db_path(&root)).expect("open after same second");
        let rewritten = chunk_content(&conn, rel);
        assert_ne!(rewritten, saved);
        assert!(rewritten[0].contains("Second"));
        assert!(!rewritten[0].contains("First"));
        let hidden = proofs(&conn, rel);
        assert_eq!(hidden[0].0, file_digest(&root, rel));
        assert_eq!(hidden[0].1, 1);
        drop(conn);

        write(
            &root,
            rel,
            "{\"id\":\"two\",\"title\":\"Second\",\"hidden\":true}\n",
        );
        File::options()
            .write(true)
            .open(&path)
            .expect("reopen legacy")
            .set_modified(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000))
            .expect("set mtime");
        scan_journal(&root, false).expect("align mtime");
        let conn = Connection::open(db_path(&root)).expect("open before legacy");
        conn.execute(
            "UPDATE chunks SET content = ? WHERE path = ?",
            params!["LEGACY PROSE", rel],
        )
        .expect("plant legacy prose");
        let legacy_digest = proofs(&conn, rel)[0].0.clone();
        drop(conn);
        let kept = scan_journal(&root, false).expect("matching digest scan");
        assert_eq!(kept.indexed, 0, "{kept:?}");
        let conn = Connection::open(db_path(&root)).expect("open after legacy");
        assert_eq!(chunk_content(&conn, rel), vec!["LEGACY PROSE".to_string()]);
        assert_eq!(proofs(&conn, rel)[0].0, legacy_digest);
        conn.execute("DELETE FROM activity_provenance", [])
            .expect("drop proof");
        drop(conn);
        let repaired = scan_journal(&root, false).expect("missing proof scan");
        assert!(repaired.indexed >= 1, "{repaired:?}");
        let conn = Connection::open(db_path(&root)).expect("open after repair");
        let repaired_text = chunk_content(&conn, rel);
        assert_ne!(repaired_text, vec!["LEGACY PROSE".to_string()]);
        assert_eq!(proofs(&conn, rel)[0].0, file_digest(&root, rel));
        assert_eq!(
            proofs(&conn, rel)[0].0,
            digest_bytes(&fs::read(&path).expect("bytes")).to_vec()
        );
        drop(conn);

        let empty = "facets/work/activities/20240103.jsonl";
        write(&root, empty, "\nnot json\n");
        let indexed_empty = scan_journal(&root, false).expect("empty activity scan");
        assert!(indexed_empty.indexed >= 1, "{indexed_empty:?}");
        let conn = Connection::open(db_path(&root)).expect("open empty");
        assert!(chunk_content(&conn, empty).is_empty());
        let files: i64 = conn
            .query_row(
                "SELECT count(*) FROM files WHERE path = ?",
                [empty],
                |row| row.get(0),
            )
            .expect("empty file row");
        assert_eq!(files, 1);
        drop(conn);
        let again = scan_journal(&root, false).expect("empty activity rescan");
        assert_eq!(again.indexed, 0, "{again:?}");

        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn activity_proof_follows_chunk_lifetime() {
        let root = temp_root("lifetime");
        let rel = "facets/work/activities/20240102.jsonl";
        write(&root, rel, "{\"id\":\"kept\",\"title\":\"Kept\"}\n");
        rescan_file(&root, Path::new(rel)).expect("rescan");
        let conn = Connection::open(db_path(&root)).expect("open");
        let original = chunk_content(&conn, rel);
        let original_proof = proofs(&conn, rel);
        let rowid: i64 = conn
            .query_row("SELECT rowid FROM chunks WHERE path = ?", [rel], |row| {
                row.get(0)
            })
            .expect("rowid");
        drop(conn);

        let mut conn = Connection::open(db_path(&root)).expect("open tx");
        let tx = conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .expect("begin");
        tx.execute("DELETE FROM activity_provenance WHERE rowid = ?", [rowid])
            .expect("delete proof");
        tx.execute("DELETE FROM chunks WHERE rowid = ?", [rowid])
            .expect("delete chunk");
        tx.execute(
            "INSERT INTO chunks(content, path, day, facet, agent, stream, idx, time_bucket) \
             VALUES ('INTRUDER', ?1, '20240102', 'work', 'activity', '', 0, '')",
            [rel],
        )
        .expect("insert intruder");
        let intruder = tx.last_insert_rowid();
        tx.execute(
            "INSERT INTO activity_provenance(rowid, digest, hidden) VALUES (?1, ?2, 1)",
            params![intruder, vec![0xab_u8; 32]],
        )
        .expect("insert intruder proof");
        tx.rollback().expect("rollback");
        drop(conn);

        let conn = Connection::open(db_path(&root)).expect("open after rollback");
        assert_eq!(chunk_content(&conn, rel), original);
        assert_eq!(proofs(&conn, rel), original_proof);
        assert_eq!(orphan_proofs(&conn), 0);
        let intruder_left: i64 = conn
            .query_row(
                "SELECT count(*) FROM chunks WHERE content = 'INTRUDER'",
                [],
                |row| row.get(0),
            )
            .expect("intruder");
        assert_eq!(intruder_left, 0);
        drop(conn);

        write(&root, rel, "{\"id\":\"kept\",\"title\":\"Rescanned\"}\n");
        rescan_file(&root, Path::new(rel)).expect("rescan changed");
        let conn = Connection::open(db_path(&root)).expect("open after rescan");
        assert!(chunk_content(&conn, rel)[0].contains("Rescanned"));
        assert_eq!(proofs(&conn, rel)[0].0, file_digest(&root, rel));
        assert_ne!(proofs(&conn, rel), original_proof);
        assert_eq!(orphan_proofs(&conn), 0);
        conn.execute(
            "UPDATE chunks SET stream = 'kept-stream' WHERE path = ?",
            [rel],
        )
        .expect("set stream");
        drop(conn);
        prune_chunks_by_stream(&root, "kept-stream").expect("stream prune");
        let conn = Connection::open(db_path(&root)).expect("open after stream prune");
        assert!(chunk_content(&conn, rel).is_empty());
        assert_eq!(orphan_proofs(&conn), 0);
        drop(conn);

        rescan_file(&root, Path::new(rel)).expect("rescan for path prune");
        prune_by_paths(&root, &[rel]).expect("path prune");
        let conn = Connection::open(db_path(&root)).expect("open after path prune");
        assert!(chunk_content(&conn, rel).is_empty());
        assert_eq!(orphan_proofs(&conn), 0);
        drop(conn);

        rescan_file(&root, Path::new(rel)).expect("rescan for removal");
        fs::remove_file(root.join(rel)).expect("remove source");
        let removed = scan_journal(&root, true).expect("removal scan");
        assert!(removed.removed >= 1, "{removed:?}");
        let conn = Connection::open(db_path(&root)).expect("open after removal");
        assert!(chunk_content(&conn, rel).is_empty());
        assert_eq!(orphan_proofs(&conn), 0);
        drop(conn);

        write(&root, rel, "{\"id\":\"kept\",\"title\":\"After reset\"}\n");
        rescan_file(&root, Path::new(rel)).expect("rescan before reset");
        reset_index(&root).expect("reset");
        let conn = Connection::open(db_path(&root)).expect("open after reset");
        let proofs_left: i64 = conn
            .query_row("SELECT count(*) FROM activity_provenance", [], |row| {
                row.get(0)
            })
            .expect("proofs after reset");
        assert_eq!(proofs_left, 0);
        assert_eq!(orphan_proofs(&conn), 0);
        conn.execute(
            "INSERT INTO chunks(content, path, day, facet, agent, stream, idx, time_bucket) \
             VALUES ('later', ?1, '20240102', 'work', 'activity', '', 0, '')",
            [rel],
        )
        .expect("reuse rowid");
        let later = conn.last_insert_rowid();
        let later_proof: Option<Vec<u8>> = conn
            .query_row(
                "SELECT digest FROM activity_provenance WHERE rowid = ?",
                [later],
                |row| row.get(0),
            )
            .optional()
            .expect("later proof");
        assert!(later_proof.is_none());
        drop(conn);
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn written_activity_shape_refreshes_on_same_second_change_and_alias_does_not() {
        let root = temp_root("shape");
        let shaped = "facets/alpha/events/20261010.jsonl";
        let alias = "facets/alpha/events/20261011.jsonl";
        let named = "facets/alpha/activities/20261010/notes/activity.md";
        write(
            &root,
            "facets/alpha/events/shape.json",
            r#"{"20261010.jsonl":"Activity","20261011.jsonl":"Event"}"#,
        );
        write(
            &root,
            shaped,
            "{\"id\":\"one\",\"title\":\"First\",\"hidden\":true}\n",
        );
        write(
            &root,
            alias,
            "{\"title\":\"Alias\",\"summary\":\"aliascontrolneedle\"}\n",
        );
        write(&root, named, "# Activity\n\nagentactivityneedle\n");
        let first = scan_journal(&root, true).expect("scan written shape");
        assert_eq!(first.failed, 0, "{first:?}");

        let conn = Connection::open(db_path(&root)).expect("open");
        assert_eq!(proofs(&conn, shaped).len(), 1);
        assert_eq!(proofs(&conn, shaped)[0].1, 1);
        assert!(chunk_content(&conn, shaped)[0].contains("First"));
        assert!(proofs(&conn, alias).is_empty());
        assert!(proofs(&conn, named).is_empty());
        let alias_saved = chunk_content(&conn, alias);
        let named_saved = chunk_content(&conn, named);
        let agent: String = conn
            .query_row("SELECT agent FROM chunks WHERE path = ?", [named], |row| {
                row.get(0)
            })
            .expect("markdown agent");
        assert_eq!(agent, "activity");
        drop(conn);

        let shaped_path = root.join(shaped);
        let alias_path = root.join(alias);
        let named_path = root.join(named);
        let shaped_mtime = fs::metadata(&shaped_path)
            .expect("shaped mtime")
            .modified()
            .expect("modified");
        let alias_mtime = fs::metadata(&alias_path)
            .expect("alias mtime")
            .modified()
            .expect("modified");
        let named_mtime = fs::metadata(&named_path)
            .expect("named mtime")
            .modified()
            .expect("modified");
        write(
            &root,
            shaped,
            "{\"id\":\"two\",\"title\":\"Second\",\"hidden\":false}\n",
        );
        write(
            &root,
            alias,
            "{\"title\":\"Alias\",\"summary\":\"aliaschangedneedle\"}\n",
        );
        write(&root, named, "# Activity\n\nagentchangedneedle\n");
        File::options()
            .write(true)
            .open(&shaped_path)
            .expect("shaped")
            .set_modified(shaped_mtime)
            .expect("keep shaped mtime");
        File::options()
            .write(true)
            .open(&alias_path)
            .expect("alias")
            .set_modified(alias_mtime)
            .expect("keep alias mtime");
        File::options()
            .write(true)
            .open(&named_path)
            .expect("named")
            .set_modified(named_mtime)
            .expect("keep named mtime");

        let second = scan_journal(&root, false).expect("same-second scan");
        assert!(second.indexed >= 1, "{second:?}");
        let conn = Connection::open(db_path(&root)).expect("open after same second");
        let rewritten = chunk_content(&conn, shaped);
        assert!(rewritten[0].contains("Second"), "{rewritten:?}");
        assert!(!rewritten[0].contains("First"));
        assert_eq!(proofs(&conn, shaped)[0].0, file_digest(&root, shaped));
        assert_eq!(proofs(&conn, shaped)[0].1, 0);
        assert_eq!(chunk_content(&conn, alias), alias_saved);
        assert!(proofs(&conn, alias).is_empty());
        assert_eq!(chunk_content(&conn, named), named_saved);
        assert!(proofs(&conn, named).is_empty());
        drop(conn);

        write(
            &root,
            shaped,
            "{\"id\":\"two\",\"title\":\"Second\",\"hidden\":false}\n",
        );
        File::options()
            .write(true)
            .open(&shaped_path)
            .expect("reopen legacy")
            .set_modified(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_100))
            .expect("align mtime");
        scan_journal(&root, false).expect("align digest");
        let conn = Connection::open(db_path(&root)).expect("open before legacy");
        conn.execute(
            "UPDATE chunks SET content = ? WHERE path = ?",
            params!["LEGACY PROSE", shaped],
        )
        .expect("plant legacy prose");
        drop(conn);
        let kept = scan_journal(&root, false).expect("matching digest scan");
        assert_eq!(kept.indexed, 0, "{kept:?}");
        let conn = Connection::open(db_path(&root)).expect("open after legacy");
        assert_eq!(
            chunk_content(&conn, shaped),
            vec!["LEGACY PROSE".to_string()]
        );
        drop(conn);

        write(
            &root,
            shaped,
            "{\"id\":\"three\",\"title\":\"Third\",\"hidden\":true}\n",
        );
        let current = fs::metadata(&shaped_path)
            .expect("third mtime")
            .modified()
            .expect("modified");
        File::options()
            .write(true)
            .open(&shaped_path)
            .expect("third")
            .set_modified(current)
            .expect("keep third mtime");
        rescan_file(&root, Path::new(shaped)).expect("rescan shaped activity");
        let conn = Connection::open(db_path(&root)).expect("open after rescan");
        assert!(chunk_content(&conn, shaped)[0].contains("Third"));
        assert_eq!(proofs(&conn, shaped)[0].1, 1);
        assert_eq!(proofs(&conn, shaped)[0].0, file_digest(&root, shaped));
        assert_eq!(chunk_content(&conn, alias), alias_saved);
        assert!(proofs(&conn, alias).is_empty());
        drop(conn);
        fs::remove_dir_all(root).expect("cleanup");
    }
}
