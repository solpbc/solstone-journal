// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! SQLite claim index for indexed audio archive members.
//!
//! Stored in `imports/audio-archive-claims.sqlite`.

use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{Connection, OpenFlags, OptionalExtension, params};

use super::AudioArchiveError;
use super::identity::MemberIdentity;
use super::plan::PlannedMember;
use super::record::ArchiveStore;

const CLAIM_DB_FILENAME: &str = "audio-archive-claims.sqlite";
pub const DEFAULT_WRITE_BUSY_TIMEOUT: Duration = Duration::from_millis(2000);

/// Read-only hints. The retained run intent and published segment, never this
/// accelerator, establish whether a part actually belongs to an archive.
#[cfg(unix)]
pub(crate) fn pending_runs_for_coordinate(
    journal: &Path,
    day: &str,
    key: &str,
) -> Result<Vec<String>, String> {
    let path = claim_db_path(journal);
    if !path.try_exists().map_err(|error| error.to_string())? {
        return Ok(Vec::new());
    }
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|error| error.to_string())?;
    connection
        .busy_timeout(DEFAULT_WRITE_BUSY_TIMEOUT)
        .map_err(|error| error.to_string())?;
    let present = connection
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='audio_archive_pending_tile'",
            [],
            |_| Ok(()),
        )
        .optional()
        .map_err(|error| error.to_string())?
        .is_some();
    if !present {
        return Ok(Vec::new());
    }
    let mut statement = connection.prepare("SELECT import_id FROM audio_archive_pending_tile WHERE day=?1 AND segment_key=?2 ORDER BY import_id LIMIT 65").map_err(|error| error.to_string())?;
    let ids = statement
        .query_map(params![day, key], |row| row.get::<_, String>(0))
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    if ids.len() > 64 {
        return Err("archive coordinate lookup exceeds its bound".into());
    }
    for id in &ids {
        crate::validate_timestamp(id).map_err(|error| error.to_string())?;
    }
    Ok(ids)
}

/// Look up one archive without enumerating previous runs. The run record remains authoritative.
/// Missing state is read-only; a damaged living run must not silently become a new run.
pub fn read_matching_archive_run(
    journal: &Path,
    archive_identity: &str,
) -> Result<Option<String>, AudioArchiveError> {
    let path = claim_db_path(journal);
    if !path.try_exists()? {
        return Ok(None);
    }
    let conn = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|error| map_read_sqlite(&path, error))?;
    conn.busy_timeout(DEFAULT_WRITE_BUSY_TIMEOUT)
        .map_err(|error| map_read_sqlite(&path, error))?;
    let exists = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='audio_archive_run'",
            [],
            |_| Ok(()),
        )
        .optional()
        .map_err(|error| map_read_sqlite(&path, error))?
        .is_some();
    if !exists {
        return Ok(None);
    }
    let import_id: Option<String> = conn
        .query_row(
            "SELECT import_id FROM audio_archive_run WHERE archive_identity=?",
            [archive_identity],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| map_read_sqlite(&path, error))?;
    let Some(import_id) = import_id else {
        return Ok(None);
    };
    crate::timestamp::validate_timestamp(&import_id).map_err(|error| {
        AudioArchiveError::ClaimDatabaseCorrupt(format!("invalid archive run ID: {error}"))
    })?;
    match fs::symlink_metadata(journal.join("imports").join(&import_id)) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
        Ok(metadata) if !metadata.file_type().is_dir() => {
            return Err(AudioArchiveError::ClaimDatabaseCorrupt(format!(
                "run {import_id} is not a directory"
            )));
        }
        Ok(_) => {}
    }
    let settings = ArchiveStore::open(journal, &import_id, false)?.settings()?;
    Ok((settings.archive_identity == archive_identity).then_some(import_id))
}

/// Record the latest admitted run for this archive in the indexed lookup accelerator.
pub(crate) fn record_archive_run(
    journal: &Path,
    import_id: &str,
    archive_identity: &str,
) -> Result<(), AudioArchiveError> {
    let path = claim_db_path(journal);
    let was_missing = !path.try_exists()?;
    // Publication has already created the private imports directory and run record.
    let conn = Connection::open(&path).map_err(map_sqlite)?;
    #[cfg(unix)]
    if was_missing {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(not(unix))]
    let _ = was_missing;
    conn.busy_timeout(DEFAULT_WRITE_BUSY_TIMEOUT)
        .map_err(map_sqlite)?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS audio_archive_run (
            archive_identity TEXT PRIMARY KEY NOT NULL,
            import_id TEXT NOT NULL
        )",
    )
    .map_err(map_sqlite)?;
    conn.execute(
        "INSERT INTO audio_archive_run(archive_identity,import_id) VALUES(?,?)
         ON CONFLICT(archive_identity) DO UPDATE SET import_id=excluded.import_id",
        params![archive_identity, import_id],
    )
    .map_err(map_sqlite)?;
    Ok(())
}

/// Register owed discovery before a tile can become visible. Run-owned state stays authoritative.
pub fn register_pending_tile(
    journal: &Path,
    import_id: &str,
    day: &str,
    segment: &str,
) -> Result<(), AudioArchiveError> {
    let conn = Connection::open(claim_db_path(journal))?;
    conn.busy_timeout(DEFAULT_WRITE_BUSY_TIMEOUT)?;
    conn.execute_batch("CREATE TABLE IF NOT EXISTS audio_archive_pending_tile (
        import_id TEXT NOT NULL, day TEXT NOT NULL, segment_key TEXT NOT NULL,
        discovery_order INTEGER NOT NULL DEFAULT 0,
        PRIMARY KEY(import_id,day,segment_key));
        CREATE INDEX IF NOT EXISTS audio_archive_pending_time ON audio_archive_pending_tile(discovery_order,day DESC,segment_key DESC,import_id);
        CREATE INDEX IF NOT EXISTS audio_archive_pending_run ON audio_archive_pending_tile(import_id,discovery_order,day DESC,segment_key DESC);
        CREATE INDEX IF NOT EXISTS audio_archive_pending_day ON audio_archive_pending_tile(day);")?;
    conn.execute(
        "INSERT OR IGNORE INTO audio_archive_pending_tile(import_id,day,segment_key) VALUES(?,?,?)",
        params![import_id, day, segment],
    )?;
    Ok(())
}

/// Read-only indexed day barrier. Pending archive work holds day publication
/// without walking an archive stream or every run record.
pub fn archive_day_has_pending(journal: &Path, day: &str) -> Result<bool, AudioArchiveError> {
    let path = claim_db_path(journal);
    if !path.try_exists()? {
        return Ok(false);
    }
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    conn.busy_timeout(DEFAULT_WRITE_BUSY_TIMEOUT)?;
    if conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='audio_archive_pending_tile'",
            [],
            |_| Ok(()),
        )
        .optional()?
        .is_none()
    {
        return Ok(false);
    }
    Ok(conn
        .query_row(
            "SELECT 1 FROM audio_archive_pending_tile WHERE day=? LIMIT 1",
            [day],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

/// Bounded indexed discovery, independent of the number of prior imports and members.
pub fn update_pending_tiles(
    journal: &Path,
    limit: u32,
) -> Result<Vec<(String, String, String)>, AudioArchiveError> {
    update_pending_tiles_for_run(journal, limit, None)
}

pub(crate) fn update_pending_tiles_checked(
    journal: &Path,
    limit: u32,
    checkpoint: &dyn Fn() -> Result<(), String>,
) -> Result<Vec<(String, String, String)>, AudioArchiveError> {
    update_pending_tiles_for_run_checked(journal, limit, None, checkpoint)
}

pub(crate) fn update_pending_tiles_for_run(
    journal: &Path,
    limit: u32,
    import_id: Option<&str>,
) -> Result<Vec<(String, String, String)>, AudioArchiveError> {
    update_pending_tiles_for_run_checked(journal, limit, import_id, &|| Ok(()))
}

fn update_pending_tiles_for_run_checked(
    journal: &Path,
    limit: u32,
    import_id: Option<&str>,
    checkpoint: &dyn Fn() -> Result<(), String>,
) -> Result<Vec<(String, String, String)>, AudioArchiveError> {
    checkpoint().map_err(AudioArchiveError::Other)?;
    let path = claim_db_path(journal);
    if !path.try_exists()? {
        return Ok(Vec::new());
    }
    let mut conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    conn.busy_timeout(DEFAULT_WRITE_BUSY_TIMEOUT)?;
    let exists = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='audio_archive_pending_tile'",
            [],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    if !exists {
        return Ok(Vec::new());
    }
    // Rotate every selected batch, including damaged or not-yet-published tiles.
    // Discovery fairness is separate from the worker's newest-first band-4 order.
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    checkpoint().map_err(AudioArchiveError::Other)?;
    let selected: Vec<(String, String, String)> = {
        let sql = if import_id.is_some() {
            "SELECT import_id,day,segment_key FROM audio_archive_pending_tile WHERE import_id=?1 ORDER BY discovery_order,day DESC,segment_key DESC,import_id LIMIT ?2"
        } else {
            "SELECT import_id,day,segment_key FROM audio_archive_pending_tile WHERE ?1 IS NULL ORDER BY discovery_order,day DESC,segment_key DESC,import_id LIMIT ?2"
        };
        let mut query = tx.prepare(sql)?;
        query
            .query_map(params![import_id, limit.min(64)], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })?
            .collect::<Result<_, _>>()?
    };
    let next: i64 = tx.query_row(
        "SELECT COALESCE(MAX(discovery_order),0)+1 FROM audio_archive_pending_tile",
        [],
        |row| row.get(0),
    )?;
    for (id, day, key) in &selected {
        tx.execute("UPDATE audio_archive_pending_tile SET discovery_order=? WHERE import_id=? AND day=? AND segment_key=?",params![next,id,day,key])?;
    }
    checkpoint().map_err(AudioArchiveError::Other)?;
    tx.commit()?;
    Ok(selected)
}

pub fn retire_pending_tile(
    journal: &Path,
    import_id: &str,
    day: &str,
    segment: &str,
) -> Result<(), AudioArchiveError> {
    retire_pending_tile_checked(journal, import_id, day, segment, &|| Ok(()))
}

pub(crate) fn retire_pending_tile_checked(
    journal: &Path,
    import_id: &str,
    day: &str,
    segment: &str,
    checkpoint: &dyn Fn() -> Result<(), String>,
) -> Result<(), AudioArchiveError> {
    checkpoint().map_err(AudioArchiveError::Other)?;
    let mut conn =
        Connection::open_with_flags(claim_db_path(journal), OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    conn.busy_timeout(DEFAULT_WRITE_BUSY_TIMEOUT)?;
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    checkpoint().map_err(AudioArchiveError::Other)?;
    tx.execute(
        "DELETE FROM audio_archive_pending_tile WHERE import_id=? AND day=? AND segment_key=?",
        params![import_id, day, segment],
    )?;
    checkpoint().map_err(AudioArchiveError::Other)?;
    tx.commit()?;
    Ok(())
}

pub fn claim_db_path(journal_root: &Path) -> PathBuf {
    journal_root.join("imports").join(CLAIM_DB_FILENAME)
}

/// Read-only check for members currently claimed by living runs.
///
/// Missing database file returns empty and must not create the file.
/// If `current_import_id` is provided, claims owned by that run are ignored.
pub fn find_held_members(
    journal_root: &Path,
    current_import_id: Option<&str>,
    members: &[PlannedMember],
    busy_timeout: Duration,
) -> Result<HashSet<MemberIdentity>, AudioArchiveError> {
    let path = claim_db_path(journal_root);
    if !path.exists() {
        return Ok(HashSet::new());
    }

    let conn = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|error| map_read_sqlite(&path, error))?;

    conn.busy_timeout(busy_timeout)
        .map_err(|error| map_read_sqlite(&path, error))?;

    let table_exists: bool = match conn.query_row(
        "SELECT 1 FROM sqlite_master WHERE type='table' AND name='audio_archive_claim';",
        [],
        |_| Ok(true),
    ) {
        Ok(v) => v,
        Err(rusqlite::Error::QueryReturnedNoRows) => false,
        Err(error) => {
            return Err(map_read_sqlite(&path, error));
        }
    };
    if !table_exists {
        return Ok(HashSet::new());
    }

    let mut stmt = conn
        .prepare(
            "SELECT import_id, sha256 FROM audio_archive_claim WHERE byte_len = ? AND sha256 = ? AND occurrence = ?;",
        )
        .map_err(|error| map_read_sqlite(&path, error))?;

    let mut held = HashSet::new();
    let mut owners = HashMap::new();
    let mut owner_order = VecDeque::new();

    for member in members {
        let mut rows = stmt
            .query(params![
                member.identity.byte_len as i64,
                member.identity.sha256,
                member.identity.occurrence
            ])
            .map_err(|error| map_read_sqlite(&path, error))?;

        if let Some(row) = rows.next().map_err(|error| map_read_sqlite(&path, error))? {
            let row_import_id: String =
                row.get(0).map_err(|error| map_read_sqlite(&path, error))?;
            let sha256: String = row.get(1).map_err(|error| map_read_sqlite(&path, error))?;

            if !is_valid_sha256(&sha256)
                || crate::timestamp::validate_timestamp(&row_import_id).is_err()
            {
                return Err(AudioArchiveError::ClaimDatabaseCorrupt(format!(
                    "invalid claim record: sha256='{sha256}', import_id='{row_import_id}' in {}",
                    path.display()
                )));
            }

            if let Some(curr_id) = current_import_id
                && row_import_id == curr_id
            {
                continue;
            }

            let run_dir = journal_root.join("imports").join(&row_import_id);
            if run_dir.is_dir() {
                if !owners.contains_key(&row_import_id) {
                    let store = ArchiveStore::open(journal_root, &row_import_id, false).map_err(
                        |error| {
                            AudioArchiveError::ClaimDatabaseCorrupt(format!(
                                "run {row_import_id}: {error}"
                            ))
                        },
                    )?;
                    if owners.len() == 64
                        && let Some(oldest) = owner_order.pop_front()
                    {
                        owners.remove(&oldest);
                    }
                    owner_order.push_back(row_import_id.clone());
                    owners.insert(row_import_id.clone(), store);
                }
                if !owners[&row_import_id].holds(&member.identity)? {
                    // Timestamp directory IDs can be reused after whole-run removal.
                    // The valid current run record, not this accelerator, owns identity.
                    continue;
                }
                held.insert(member.identity.clone());
            }
        }
    }

    Ok(held)
}

/// Transactional insertion of claims for all members in a complete tile.
pub fn insert_tile_claims(
    journal_root: &Path,
    import_id: &str,
    member_identities: &[MemberIdentity],
    busy_timeout: Duration,
) -> Result<(), AudioArchiveError> {
    if member_identities.is_empty() {
        return Ok(());
    }

    let path = claim_db_path(journal_root);
    if let Some(parent) = path.parent()
        && !parent.exists()
    {
        solstone_core_journal_io::create_directory_with_mode(parent, 0o700)
            .map_err(|e| AudioArchiveError::Io(std::io::Error::other(e.to_string())))?;
    }

    let was_missing = !path.exists();

    let mut conn = Connection::open(&path).map_err(|error| {
        AudioArchiveError::ClaimDatabaseCorrupt(format!("{}: {error}", path.display()))
    })?;

    #[cfg(unix)]
    if was_missing {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
    }

    conn.busy_timeout(busy_timeout).map_err(map_sqlite)?;

    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS audio_archive_claim (
            byte_len INTEGER NOT NULL,
            sha256 TEXT NOT NULL,
            occurrence TEXT NOT NULL,
            import_id TEXT NOT NULL,
            PRIMARY KEY (byte_len, sha256, occurrence)
        );",
    )
    .map_err(map_sqlite)?;

    let tx = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(map_sqlite)?;

    {
        let mut insert_stmt = tx
            .prepare(
                "INSERT INTO audio_archive_claim (byte_len, sha256, occurrence, import_id)
                 VALUES (?, ?, ?, ?)
                 ON CONFLICT DO NOTHING;",
            )
            .map_err(map_sqlite)?;

        let mut select_stmt = tx
            .prepare(
                "SELECT import_id FROM audio_archive_claim WHERE byte_len = ? AND sha256 = ? AND occurrence = ?;",
            )
            .map_err(map_sqlite)?;

        let mut update_stmt = tx
            .prepare(
                "UPDATE audio_archive_claim SET import_id = ? WHERE byte_len = ? AND sha256 = ? AND occurrence = ?;",
            )
            .map_err(map_sqlite)?;

        for identity in member_identities {
            insert_stmt
                .execute(params![
                    identity.byte_len as i64,
                    identity.sha256,
                    identity.occurrence,
                    import_id
                ])
                .map_err(map_sqlite)?;

            let existing_id: String = select_stmt
                .query_row(
                    params![
                        identity.byte_len as i64,
                        identity.sha256,
                        identity.occurrence
                    ],
                    |row| row.get(0),
                )
                .map_err(map_sqlite)?;

            if existing_id != import_id {
                if crate::timestamp::validate_timestamp(&existing_id).is_err() {
                    return Err(AudioArchiveError::ClaimDatabaseCorrupt(format!(
                        "claim database contains invalid import_id '{existing_id}'"
                    )));
                }
                let other_dir = journal_root.join("imports").join(&existing_id);
                if other_dir.is_dir()
                    && ArchiveStore::open(journal_root, &existing_id, false)?.holds(identity)?
                {
                    return Err(AudioArchiveError::MemberHeldByLivingRun {
                        import_id: existing_id,
                    });
                }
                update_stmt
                    .execute(params![
                        import_id,
                        identity.byte_len as i64,
                        identity.sha256,
                        identity.occurrence
                    ])
                    .map_err(map_sqlite)?;
            }
        }
    }

    tx.commit().map_err(map_sqlite)?;
    Ok(())
}

fn map_read_sqlite(path: &Path, error: rusqlite::Error) -> AudioArchiveError {
    match map_sqlite(error) {
        AudioArchiveError::ClaimDatabaseBusy(msg) => AudioArchiveError::ClaimDatabaseBusy(msg),
        other => AudioArchiveError::ClaimDatabaseCorrupt(format!("{}: {other}", path.display())),
    }
}

fn map_sqlite(error: rusqlite::Error) -> AudioArchiveError {
    if let rusqlite::Error::SqliteFailure(err, _) = &error
        && (err.code == rusqlite::ffi::ErrorCode::DatabaseBusy
            || err.code == rusqlite::ffi::ErrorCode::DatabaseLocked)
    {
        return AudioArchiveError::ClaimDatabaseBusy(error.to_string());
    }
    AudioArchiveError::Sqlite(error)
}

fn is_valid_sha256(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}
