// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Run record persistence and resume settings verification for indexed audio archives.
//!
//! Stored in `imports/<id>/audio-archive.sqlite`.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use jiff::tz::TimeZone;
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use super::AudioArchiveError;
use super::plan::{
    AUDIO_ARCHIVE_LEN_SECONDS, AUDIO_ARCHIVE_WINDOW_MS, AudioArchivePlan, AudioArchiveSettings,
    AudioArchiveSize, MemberDisposition, PartOutcome,
};

pub const AUDIO_ARCHIVE_RECORD_VERSION: u32 = 1;
pub const AUDIO_ARCHIVE_RECORD_FILENAME: &str = "audio-archive.sqlite";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioArchiveRecordSettings {
    pub owner_zone: String,
    pub window_ms: i64,
    pub stream: String,
    pub qualifier: String,
    pub archive_identity: String,
    pub len: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioArchiveRecordMember {
    pub byte_len: u64,
    pub sha256: String,
    pub canonical_occurrence: String,
    pub original_relative_name: String,
    pub duration_ms: i64,
    pub source_start: super::identity::MemberStartEvidence,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_note: Option<String>,
    pub disposition: MemberDisposition,
    /// A staging rejection can follow an earlier saved part of this member.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub save_rejection: Option<super::SourceRejection>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioArchiveRecordPart {
    pub member_sha256: String,
    pub member_occurrence: String,
    pub day: String,
    pub segment_key: String,
    pub offset_ms: i64,
    pub origin_ms: i64,
    pub length_ms: i64,
    pub stored_name: String,
    pub outcome: PartOutcome,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioArchiveRecord {
    pub version: u32,
    pub settings: AudioArchiveRecordSettings,
    pub members: Vec<AudioArchiveRecordMember>,
    pub parts: Vec<AudioArchiveRecordPart>,
}

pub fn audio_archive_record_path(journal_root: &Path, import_id: &str) -> PathBuf {
    journal_root
        .join("imports")
        .join(import_id)
        .join(AUDIO_ARCHIVE_RECORD_FILENAME)
}

/// Convert a computed plan into the serializable initial run record.
pub fn plan_to_record(plan: &AudioArchivePlan) -> AudioArchiveRecord {
    let settings = AudioArchiveRecordSettings {
        owner_zone: plan.owner_zone.iana_name().unwrap_or_default().to_owned(),
        window_ms: AUDIO_ARCHIVE_WINDOW_MS,
        stream: plan.stream.clone(),
        qualifier: plan.settings.qualifier.clone(),
        archive_identity: plan.settings.archive_identity.clone(),
        len: AUDIO_ARCHIVE_LEN_SECONDS,
    };

    let members = plan
        .members
        .iter()
        .map(|m| AudioArchiveRecordMember {
            byte_len: m.identity.byte_len,
            sha256: m.identity.sha256.clone(),
            canonical_occurrence: m.identity.occurrence.clone(),
            original_relative_name: m.original_relative_name.clone(),
            duration_ms: m.duration_ms,
            source_start: m.source_start.clone(),
            run_note: m.run_note.clone(),
            disposition: m.disposition.clone(),
            save_rejection: None,
        })
        .collect();

    let mut parts = Vec::new();
    for tile in &plan.tiles {
        for part in &tile.parts {
            parts.push(AudioArchiveRecordPart {
                member_sha256: part.member_identity.sha256.clone(),
                member_occurrence: part.member_identity.occurrence.clone(),
                day: part.day.clone(),
                segment_key: part.segment_key.clone(),
                offset_ms: part.offset_ms,
                origin_ms: part.origin_ms,
                length_ms: part.length_ms,
                stored_name: part.stored_name.clone(),
                outcome: part.outcome,
            });
        }
    }

    AudioArchiveRecord {
        version: AUDIO_ARCHIVE_RECORD_VERSION,
        settings,
        members,
        parts,
    }
}

/// Create the run-owned plan once. Progress is updated in bounded tile transactions.
pub fn save_audio_archive_record(
    journal_root: &Path,
    import_id: &str,
    record: &AudioArchiveRecord,
) -> Result<PathBuf, AudioArchiveError> {
    ArchiveStore::create(journal_root, import_id, record)?;
    Ok(audio_archive_record_path(journal_root, import_id))
}

/// Explicit full detail export. Discovery and status use bounded store queries instead.
pub fn read_audio_archive(
    journal_root: &Path,
    import_id: &str,
) -> Result<Option<AudioArchiveRecord>, AudioArchiveError> {
    validate_id(import_id)?;
    let path = audio_archive_record_path(journal_root, import_id);
    match fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
        Ok(_) => ArchiveStore::open(journal_root, import_id, false)?
            .read_record()
            .map(Some),
    }
}

fn validate_id(import_id: &str) -> Result<(), AudioArchiveError> {
    crate::timestamp::validate_timestamp(import_id)
        .map(|_| ())
        .map_err(|error| AudioArchiveError::Other(error.to_string()))
}

fn validate_part(part: &AudioArchiveRecordPart) -> Result<(), AudioArchiveError> {
    validate_coordinates(&part.day, &part.segment_key)?;
    if part.offset_ms < 0
        || part.origin_ms < 0
        || part.length_ms <= 0
        || part
            .offset_ms
            .checked_add(part.length_ms)
            .is_none_or(|end| end > AUDIO_ARCHIVE_WINDOW_MS)
    {
        return Err(AudioArchiveError::ClaimDatabaseCorrupt(
            "invalid archive part placement".to_owned(),
        ));
    }
    solstone_core_format::audio_archive::validate_stored_name(&part.stored_name)
        .map_err(|error| AudioArchiveError::ClaimDatabaseCorrupt(error.to_string()))
}

pub(crate) fn validate_coordinates(day: &str, key: &str) -> Result<(), AudioArchiveError> {
    if day.len() != 8
        || !day.bytes().all(|byte| byte.is_ascii_digit())
        || solstone_core_time::parse_day(day).is_none()
        || solstone_core_format::segment::segment_parse(key).is_none()
        || !key.ends_with("_300")
    {
        return Err(AudioArchiveError::ClaimDatabaseCorrupt(
            "invalid archive tile coordinate".to_owned(),
        ));
    }
    Ok(())
}

fn outcome_key(outcome: PartOutcome) -> &'static str {
    match outcome {
        PartOutcome::Planned => "planned",
        PartOutcome::Staged => "staged",
        PartOutcome::Saved => "saved",
        PartOutcome::SavedButUnavailable => "saved_but_unavailable",
        PartOutcome::Rejected => "rejected",
    }
}

fn parse_outcome(key: &str) -> Result<PartOutcome, AudioArchiveError> {
    match key {
        "planned" => Ok(PartOutcome::Planned),
        "staged" => Ok(PartOutcome::Staged),
        "saved" => Ok(PartOutcome::Saved),
        "saved_but_unavailable" => Ok(PartOutcome::SavedButUnavailable),
        "rejected" => Ok(PartOutcome::Rejected),
        _ => Err(AudioArchiveError::ClaimDatabaseCorrupt(
            "unknown archive outcome".to_owned(),
        )),
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AudioArchiveStatus {
    pub size: AudioArchiveSize,
    pub total_parts: u64,
    pub saved_parts: u64,
    pub saved_members: u64,
    pub saved_tiles: u64,
    pub processed_parts: u64,
    pub pending_processing: u64,
    pub unavailable_parts: u64,
    pub rejected_parts: u64,
    pub save_complete: bool,
}

/// Run-owned indexed facts. A connection is kept for a writer, never reopened per member.
pub struct ArchiveStore {
    conn: Connection,
}

impl ArchiveStore {
    pub fn open(
        journal_root: &Path,
        import_id: &str,
        writable: bool,
    ) -> Result<Self, AudioArchiveError> {
        validate_id(import_id)?;
        let path = audio_archive_record_path(journal_root, import_id);
        if !fs::symlink_metadata(&path)?.file_type().is_file() {
            return Err(AudioArchiveError::ClaimDatabaseCorrupt(format!(
                "{} is not a plain archive record",
                path.display()
            )));
        }
        let flags = if writable {
            OpenFlags::SQLITE_OPEN_READ_WRITE
        } else {
            OpenFlags::SQLITE_OPEN_READ_ONLY
        };
        let conn = Connection::open_with_flags(&path, flags)?;
        conn.busy_timeout(super::claims::DEFAULT_WRITE_BUSY_TIMEOUT)?;
        if writable {
            conn.execute_batch("PRAGMA foreign_keys=ON; PRAGMA synchronous=FULL;")?;
        }
        let store = Self { conn };
        let version: u32 =
            store
                .conn
                .query_row("SELECT version FROM archive_run WHERE id=1", [], |row| {
                    row.get(0)
                })?;
        if version != AUDIO_ARCHIVE_RECORD_VERSION {
            return Err(AudioArchiveError::ClaimDatabaseCorrupt(
                "unsupported archive record version".to_owned(),
            ));
        }
        Ok(store)
    }

    fn create(
        journal_root: &Path,
        import_id: &str,
        record: &AudioArchiveRecord,
    ) -> Result<Self, AudioArchiveError> {
        validate_id(import_id)?;
        let directory = crate::staging::ensure_import_private_chain(journal_root, import_id)
            .map_err(|error| AudioArchiveError::Other(error.to_string()))?;
        let path = directory.join(AUDIO_ARCHIVE_RECORD_FILENAME);
        let temporary = directory.join(format!(
            ".audio-archive-plan-{}.sqlite",
            solstone_core_journal_io::mint_uuid_v4()
                .map_err(|error| AudioArchiveError::Other(error.to_string()))?
        ));
        let mut create = fs::OpenOptions::new();
        create.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            create.mode(0o600);
        }
        drop(create.open(&temporary)?);
        let mut conn = Connection::open_with_flags(&temporary, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        conn.busy_timeout(super::claims::DEFAULT_WRITE_BUSY_TIMEOUT)?;
        conn.execute_batch("PRAGMA foreign_keys=ON; PRAGMA synchronous=FULL;")?;
        let tx = conn.transaction()?;
        tx.execute_batch(
            "CREATE TABLE archive_run (
                id INTEGER PRIMARY KEY CHECK(id=1), version INTEGER NOT NULL, settings TEXT NOT NULL,
                size TEXT NOT NULL, save_complete INTEGER NOT NULL DEFAULT 0,
                total_parts INTEGER NOT NULL DEFAULT 0, saved_parts INTEGER NOT NULL DEFAULT 0,
                saved_members INTEGER NOT NULL DEFAULT 0, saved_tiles INTEGER NOT NULL DEFAULT 0,
                processed_parts INTEGER NOT NULL DEFAULT 0, unavailable_parts INTEGER NOT NULL DEFAULT 0,
                pending_processing INTEGER NOT NULL DEFAULT 0,
                rejected_parts INTEGER NOT NULL DEFAULT 0
             );
             CREATE TABLE members (
                ordinal INTEGER PRIMARY KEY, byte_len INTEGER NOT NULL, sha256 TEXT NOT NULL,
                occurrence TEXT NOT NULL, included INTEGER NOT NULL, pending_parts INTEGER NOT NULL DEFAULT 0,
                payload TEXT NOT NULL
             );
             CREATE INDEX members_identity ON members(byte_len,sha256,occurrence,included);
             CREATE TABLE tiles (
                day TEXT NOT NULL, segment_key TEXT NOT NULL, phase INTEGER NOT NULL DEFAULT 0,
                pending_parts INTEGER NOT NULL, PRIMARY KEY(day,segment_key)
             );
             CREATE INDEX tiles_pending ON tiles(day DESC,segment_key DESC) WHERE phase=2 AND pending_parts>0;
             CREATE TABLE parts (
                ordinal INTEGER PRIMARY KEY, member_ordinal INTEGER NOT NULL REFERENCES members(ordinal),
                day TEXT NOT NULL, segment_key TEXT NOT NULL, stored_name TEXT NOT NULL,
                outcome TEXT NOT NULL, processing INTEGER NOT NULL DEFAULT 0 CHECK(processing IN(0,1,2)),
                payload TEXT NOT NULL, UNIQUE(day,segment_key,stored_name)
             );
             CREATE INDEX parts_tile ON parts(day,segment_key);
             CREATE INDEX parts_member ON parts(member_ordinal);
             CREATE TRIGGER saved_once BEFORE UPDATE OF outcome ON parts
             WHEN OLD.outcome IN('saved','saved_but_unavailable') AND NEW.outcome NOT IN('saved','saved_but_unavailable')
             BEGIN SELECT RAISE(ABORT,'saved archive parts cannot become unsaved'); END;
             CREATE TRIGGER saved_part AFTER UPDATE OF outcome ON parts
             WHEN OLD.outcome NOT IN('saved','saved_but_unavailable') AND NEW.outcome IN('saved','saved_but_unavailable')
             BEGIN
                UPDATE archive_run SET saved_parts=saved_parts+1,pending_processing=pending_processing+(NEW.processing=0) WHERE id=1;
                UPDATE members SET pending_parts=pending_parts-1 WHERE ordinal=NEW.member_ordinal;
                UPDATE archive_run SET saved_members=saved_members+1 WHERE id=1 AND
                    (SELECT pending_parts FROM members WHERE ordinal=NEW.member_ordinal)=0;
             END;
             CREATE TRIGGER terminal_part AFTER UPDATE OF processing ON parts WHEN OLD.processing=0 AND NEW.processing!=0
             BEGIN
                UPDATE tiles SET pending_parts=pending_parts-1 WHERE day=NEW.day AND segment_key=NEW.segment_key;
                UPDATE archive_run SET processed_parts=processed_parts+(NEW.processing=1),
                    pending_processing=pending_processing-(OLD.outcome IN('saved','saved_but_unavailable')) WHERE id=1;
             END;
             CREATE TRIGGER part_availability AFTER UPDATE OF outcome ON parts
             WHEN OLD.outcome!=NEW.outcome
             BEGIN UPDATE archive_run SET unavailable_parts=unavailable_parts+
                (NEW.outcome='saved_but_unavailable')-(OLD.outcome='saved_but_unavailable'),
                rejected_parts=rejected_parts+(NEW.outcome='rejected')-(OLD.outcome='rejected') WHERE id=1; END;"
        )?;
        tx.execute(
            "INSERT INTO archive_run(id,version,settings,size) VALUES(1,?,?,?)",
            params![
                record.version,
                serde_json::to_string(&record.settings)?,
                serde_json::to_string(&compute_size_from_record(record))?
            ],
        )?;
        let mut member_indices = HashMap::new();
        {
            let mut insert = tx.prepare("INSERT INTO members(ordinal,byte_len,sha256,occurrence,included,payload) VALUES(?,?,?,?,?,?)")?;
            for (ordinal, member) in record.members.iter().enumerate() {
                let included = member.disposition == MemberDisposition::Included;
                insert.execute(params![
                    ordinal as i64,
                    member.byte_len as i64,
                    member.sha256,
                    member.canonical_occurrence,
                    included,
                    serde_json::to_string(member)?
                ])?;
                if included {
                    member_indices.insert(
                        (&member.sha256, &member.canonical_occurrence),
                        ordinal as i64,
                    );
                }
            }
        }
        {
            let mut insert = tx.prepare("INSERT INTO parts(ordinal,member_ordinal,day,segment_key,stored_name,outcome,processing,payload) VALUES(?,?,?,?,?,?,?,?)")?;
            for (ordinal, part) in record.parts.iter().enumerate() {
                validate_part(part)?;
                let member = member_indices
                    .get(&(&part.member_sha256, &part.member_occurrence))
                    .ok_or_else(|| {
                        AudioArchiveError::ClaimDatabaseCorrupt(
                            "archive part has no included member".to_owned(),
                        )
                    })?;
                insert.execute(params![
                    ordinal as i64,
                    member,
                    part.day,
                    part.segment_key,
                    part.stored_name,
                    outcome_key(part.outcome),
                    if part.outcome == PartOutcome::SavedButUnavailable {
                        2
                    } else {
                        0
                    },
                    serde_json::to_string(part)?
                ])?;
            }
        }
        tx.execute_batch(
            "INSERT INTO tiles(day,segment_key,phase,pending_parts)
                SELECT day,segment_key,CASE WHEN MIN(outcome IN('saved','saved_but_unavailable'))=1 THEN 2 ELSE 0 END,SUM(processing=0)
                FROM parts GROUP BY day,segment_key;
             UPDATE members SET pending_parts=(SELECT COUNT(*) FROM parts WHERE member_ordinal=members.ordinal AND outcome NOT IN('saved','saved_but_unavailable'));
             UPDATE archive_run SET total_parts=(SELECT COUNT(*) FROM parts),
                saved_parts=(SELECT COUNT(*) FROM parts WHERE outcome IN('saved','saved_but_unavailable')),
                saved_members=(SELECT COUNT(*) FROM members WHERE included=1 AND pending_parts=0),
                saved_tiles=(SELECT COUNT(*) FROM tiles WHERE phase=2),
                unavailable_parts=(SELECT COUNT(*) FROM parts WHERE outcome='saved_but_unavailable'),
                pending_processing=(SELECT COUNT(*) FROM parts WHERE outcome='saved' AND processing=0) WHERE id=1;"
        )?;
        tx.commit()?;
        // Publish only a complete committed plan, with create-new semantics.
        drop(conn);
        fs::File::options()
            .read(true)
            .write(true)
            .open(&temporary)?
            .sync_all()?;
        fs::hard_link(&temporary, &path)?;
        fs::remove_file(&temporary)?;
        solstone_core_journal_io::sync_dir(journal_root, &format!("imports/{import_id}"))
            .map_err(|error| AudioArchiveError::Other(error.to_string()))?;
        Self::open(journal_root, import_id, true)
    }

    pub fn settings(&self) -> Result<AudioArchiveRecordSettings, AudioArchiveError> {
        let value: String =
            self.conn
                .query_row("SELECT settings FROM archive_run WHERE id=1", [], |row| {
                    row.get(0)
                })?;
        let settings: AudioArchiveRecordSettings = serde_json::from_str(&value)?;
        let stream =
            crate::stream_name::canonicalize_stream_name("import", Some(&settings.qualifier))
                .map_err(|error| AudioArchiveError::ClaimDatabaseCorrupt(error.to_string()))?;
        if stream != settings.stream
            || stream == "import.audio"
            || solstone_core_time::zone(&settings.owner_zone).is_none()
            || settings.window_ms != AUDIO_ARCHIVE_WINDOW_MS
            || settings.len != AUDIO_ARCHIVE_LEN_SECONDS
        {
            return Err(AudioArchiveError::ClaimDatabaseCorrupt(
                "invalid archive placement settings".to_owned(),
            ));
        }
        Ok(settings)
    }

    pub fn status(&self) -> Result<AudioArchiveStatus, AudioArchiveError> {
        let (size,total_parts,saved_parts,saved_members,saved_tiles,processed_parts,unavailable_parts,save_complete,pending_processing) = self.conn.query_row(
            "SELECT size,total_parts,saved_parts,saved_members,saved_tiles,processed_parts,unavailable_parts,save_complete,pending_processing FROM archive_run WHERE id=1", [],
            |row| Ok((row.get::<_,String>(0)?,read_counter(row,1)?,read_counter(row,2)?,read_counter(row,3)?,read_counter(row,4)?,read_counter(row,5)?,read_counter(row,6)?,row.get(7)?,read_counter(row,8)?)))?;
        Ok(AudioArchiveStatus {
            size: serde_json::from_str(&size)?,
            total_parts,
            saved_parts,
            saved_members,
            saved_tiles,
            processed_parts,
            unavailable_parts,
            save_complete,
            pending_processing,
            rejected_parts: self.conn.query_row(
                "SELECT rejected_parts FROM archive_run WHERE id=1",
                [],
                |row| read_counter(row, 0),
            )?,
        })
    }

    pub fn holds(
        &self,
        identity: &super::identity::MemberIdentity,
    ) -> Result<bool, AudioArchiveError> {
        Ok(self.conn.query_row("SELECT 1 FROM members WHERE byte_len=? AND sha256=? AND occurrence=? AND included=1 AND EXISTS(SELECT 1 FROM parts WHERE parts.member_ordinal=members.ordinal AND parts.outcome!='rejected') LIMIT 1",
            params![identity.byte_len as i64, identity.sha256, identity.occurrence], |_| Ok(())).optional()?.is_some())
    }

    pub fn members(
        &self,
        offset: u64,
        limit: u32,
    ) -> Result<Vec<AudioArchiveRecordMember>, AudioArchiveError> {
        let ordinal =
            i64::try_from(offset).map_err(|error| AudioArchiveError::Other(error.to_string()))?;
        let mut query = self
            .conn
            .prepare("SELECT payload FROM members WHERE ordinal>=? ORDER BY ordinal LIMIT ?")?;
        let rows = query.query_map(params![ordinal, limit.min(1000)], |row| {
            row.get::<_, String>(0)
        })?;
        rows.map(|row| Ok(serde_json::from_str(&row?)?)).collect()
    }

    pub fn tile_parts(
        &self,
        day: &str,
        segment: &str,
    ) -> Result<Vec<AudioArchiveRecordPart>, AudioArchiveError> {
        validate_coordinates(day, segment)?;
        let mut query = self.conn.prepare(
            "SELECT payload,outcome FROM parts WHERE day=? AND segment_key=? ORDER BY ordinal",
        )?;
        let rows = query.query_map(params![day, segment], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        rows.map(|row| {
            let (payload, outcome) = row?;
            let mut part: AudioArchiveRecordPart = serde_json::from_str(&payload)?;
            part.outcome = parse_outcome(&outcome)?;
            validate_part(&part)?;
            Ok(part)
        })
        .collect()
    }

    pub fn tile_phase(&self, day: &str, segment: &str) -> Result<u32, AudioArchiveError> {
        Ok(self.conn.query_row(
            "SELECT phase FROM tiles WHERE day=? AND segment_key=?",
            params![day, segment],
            |row| row.get(0),
        )?)
    }

    pub fn has_tile(&self, day: &str, segment: &str) -> Result<bool, AudioArchiveError> {
        validate_coordinates(day, segment)?;
        Ok(self
            .conn
            .query_row(
                "SELECT 1 FROM tiles WHERE day=? AND segment_key=?",
                params![day, segment],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    pub fn tile_coordinates(&self) -> Result<Vec<(String, String)>, AudioArchiveError> {
        let mut query = self
            .conn
            .prepare("SELECT day,segment_key FROM tiles ORDER BY day,segment_key")?;
        Ok(query
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<Result<_, _>>()?)
    }

    pub fn member_for_part(
        &self,
        part: &AudioArchiveRecordPart,
    ) -> Result<AudioArchiveRecordMember, AudioArchiveError> {
        let payload: String=self.conn.query_row("SELECT members.payload FROM parts JOIN members ON members.ordinal=parts.member_ordinal WHERE parts.day=? AND parts.segment_key=? AND parts.stored_name=?",params![part.day,part.segment_key,part.stored_name],|row|row.get(0))?;
        Ok(serde_json::from_str(&payload)?)
    }

    pub fn has_other_unsaved_parts(
        &self,
        part: &AudioArchiveRecordPart,
    ) -> Result<bool, AudioArchiveError> {
        Ok(self.conn.query_row("SELECT 1 FROM parts WHERE member_ordinal=(SELECT member_ordinal FROM parts WHERE day=? AND segment_key=? AND stored_name=?) AND outcome NOT IN('saved','saved_but_unavailable') LIMIT 1",params![part.day,part.segment_key,part.stored_name],|_|Ok(())).optional()?.is_some())
    }

    /// Reject only never-published parts. Saved parts and their ownership are immutable.
    pub fn reject_member_remaining(
        &mut self,
        part: &AudioArchiveRecordPart,
        cause: super::SourceRejection,
    ) -> Result<(), AudioArchiveError> {
        let tx = self.conn.transaction()?;
        let ordinal: i64 = tx.query_row(
            "SELECT member_ordinal FROM parts WHERE day=? AND segment_key=? AND stored_name=?",
            params![part.day, part.segment_key, part.stored_name],
            |row| row.get(0),
        )?;
        let payload: String = tx.query_row(
            "SELECT payload FROM members WHERE ordinal=?",
            [ordinal],
            |row| row.get(0),
        )?;
        let mut member: AudioArchiveRecordMember = serde_json::from_str(&payload)?;
        member.save_rejection = Some(cause);
        tx.execute(
            "UPDATE members SET payload=? WHERE ordinal=?",
            params![serde_json::to_string(&member)?, ordinal],
        )?;
        tx.execute("UPDATE parts SET outcome='rejected',processing=2 WHERE member_ordinal=? AND outcome IN('planned','staged')",[ordinal])?;
        // Empty rejected tiles never become chronicle tiles. Their parts remain visible in detail.
        tx.execute("UPDATE tiles SET phase=3 WHERE phase<2 AND (day,segment_key) IN (SELECT day,segment_key FROM parts WHERE member_ordinal=?) AND NOT EXISTS(SELECT 1 FROM parts WHERE parts.day=tiles.day AND parts.segment_key=tiles.segment_key AND outcome!='rejected')",[ordinal])?;
        tx.commit()?;
        Ok(())
    }

    pub fn publishable_tile_parts(
        &self,
        day: &str,
        key: &str,
    ) -> Result<Vec<AudioArchiveRecordPart>, AudioArchiveError> {
        Ok(self
            .tile_parts(day, key)?
            .into_iter()
            .filter(|part| part.outcome != PartOutcome::Rejected)
            .collect())
    }

    pub fn part_is_rejected(
        &self,
        part: &AudioArchiveRecordPart,
    ) -> Result<bool, AudioArchiveError> {
        Ok(self.conn.query_row(
            "SELECT outcome='rejected' FROM parts WHERE day=? AND segment_key=? AND stored_name=?",
            params![part.day, part.segment_key, part.stored_name],
            |row| row.get(0),
        )?)
    }

    /// The writer has established that this tile was never published and retired any private intent.
    pub(crate) fn update_retryable_parts(
        &mut self,
        day: &str,
        key: &str,
        names: &[String],
    ) -> Result<(), AudioArchiveError> {
        if names.is_empty() {
            return Ok(());
        }
        let tx = self.conn.transaction()?;
        let phase: u32 = tx.query_row(
            "SELECT phase FROM tiles WHERE day=? AND segment_key=?",
            params![day, key],
            |row| row.get(0),
        )?;
        if phase == 2 {
            return Err(AudioArchiveError::Other(
                "published archive tiles cannot be retried".into(),
            ));
        }
        let mut changed = 0;
        let mut members = BTreeSet::new();
        for name in names {
            let ordinal: i64 = tx.query_row(
                "SELECT member_ordinal FROM parts WHERE day=? AND segment_key=? AND stored_name=?",
                params![day, key, name],
                |row| row.get(0),
            )?;
            changed+=tx.execute("UPDATE parts SET outcome='planned',processing=0 WHERE day=? AND segment_key=? AND stored_name=? AND outcome='rejected'",params![day,key,name])?;
            members.insert(ordinal);
        }
        tx.execute(
            "UPDATE tiles SET phase=0,pending_parts=pending_parts+? WHERE day=? AND segment_key=?",
            params![
                i64::try_from(changed)
                    .map_err(|error| AudioArchiveError::Other(error.to_string()))?,
                day,
                key
            ],
        )?;
        tx.execute("UPDATE archive_run SET save_complete=0 WHERE id=1", [])?;
        for ordinal in members {
            let rejected = tx
                .query_row(
                    "SELECT 1 FROM parts WHERE member_ordinal=? AND outcome='rejected' LIMIT 1",
                    [ordinal],
                    |_| Ok(()),
                )
                .optional()?
                .is_some();
            if !rejected {
                let payload: String = tx.query_row(
                    "SELECT payload FROM members WHERE ordinal=?",
                    [ordinal],
                    |row| row.get(0),
                )?;
                let mut member: AudioArchiveRecordMember = serde_json::from_str(&payload)?;
                member.save_rejection = None;
                tx.execute(
                    "UPDATE members SET payload=? WHERE ordinal=?",
                    params![serde_json::to_string(&member)?, ordinal],
                )?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn set_tile_outcomes(
        &mut self,
        day: &str,
        segment: &str,
        parts: &[(String, PartOutcome)],
        phase: u32,
    ) -> Result<(), AudioArchiveError> {
        self.set_tile_outcomes_checked(day, segment, parts, phase, &|| Ok(()))
    }

    pub(crate) fn set_tile_outcomes_checked(
        &mut self,
        day: &str,
        segment: &str,
        parts: &[(String, PartOutcome)],
        phase: u32,
        checkpoint: &dyn Fn() -> Result<(), String>,
    ) -> Result<(), AudioArchiveError> {
        checkpoint().map_err(AudioArchiveError::Other)?;
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        checkpoint().map_err(AudioArchiveError::Other)?;
        {
            let mut update=tx.prepare("UPDATE parts SET outcome=?,processing=CASE WHEN ?='saved_but_unavailable' AND processing=0 THEN 2 ELSE processing END WHERE day=? AND segment_key=? AND stored_name=? AND (outcome!=? OR (outcome='saved_but_unavailable' AND processing=0))")?;
            for (name, outcome) in parts {
                if update.execute(params![
                    outcome_key(*outcome),
                    outcome_key(*outcome),
                    day,
                    segment,
                    name,
                    outcome_key(*outcome)
                ])? == 0
                    && tx
                        .query_row(
                            "SELECT 1 FROM parts WHERE day=? AND segment_key=? AND stored_name=?",
                            params![day, segment, name],
                            |_| Ok(()),
                        )
                        .optional()?
                        .is_none()
                {
                    return Err(AudioArchiveError::ClaimDatabaseCorrupt(
                        "unknown tile part".to_owned(),
                    ));
                }
            }
        }
        tx.execute("UPDATE archive_run SET saved_tiles=saved_tiles+1 WHERE id=1 AND ?=2 AND (SELECT phase FROM tiles WHERE day=? AND segment_key=?)<2", params![phase,day,segment])?;
        tx.execute(
            "UPDATE tiles SET phase=? WHERE day=? AND segment_key=? AND phase!=?",
            params![phase, day, segment, phase],
        )?;
        checkpoint().map_err(AudioArchiveError::Other)?;
        tx.commit()?;
        Ok(())
    }

    pub fn pending_tiles(&self, limit: u32) -> Result<Vec<(String, String)>, AudioArchiveError> {
        let mut query=self.conn.prepare("SELECT day,segment_key FROM tiles WHERE phase=2 AND pending_parts>0 ORDER BY day DESC,segment_key DESC LIMIT ?")?;
        Ok(query
            .query_map([limit.min(64)], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<Result<_, _>>()?)
    }

    pub fn finish_processing(
        &mut self,
        day: &str,
        segment: &str,
        name: &str,
        unavailable: bool,
    ) -> Result<(), AudioArchiveError> {
        self.finish_processing_checked(day, segment, name, unavailable, &|| Ok(()))
    }

    pub(crate) fn finish_processing_checked(
        &mut self,
        day: &str,
        segment: &str,
        name: &str,
        unavailable: bool,
        checkpoint: &dyn Fn() -> Result<(), String>,
    ) -> Result<(), AudioArchiveError> {
        checkpoint().map_err(AudioArchiveError::Other)?;
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        checkpoint().map_err(AudioArchiveError::Other)?;
        tx.execute("UPDATE parts SET processing=?,outcome=CASE WHEN ? THEN 'saved_but_unavailable' ELSE outcome END WHERE day=? AND segment_key=? AND stored_name=? AND processing=0 AND (outcome='saved' OR (? AND outcome='saved_but_unavailable'))",
            params![if unavailable {2} else {1},unavailable,day,segment,name,unavailable])?;
        checkpoint().map_err(AudioArchiveError::Other)?;
        tx.commit()?;
        Ok(())
    }

    pub fn pending_part_names(
        &self,
        day: &str,
        segment: &str,
    ) -> Result<Vec<String>, AudioArchiveError> {
        let mut query=self.conn.prepare("SELECT stored_name FROM parts WHERE day=? AND segment_key=? AND processing=0 AND outcome='saved' ORDER BY ordinal")?;
        Ok(query
            .query_map(params![day, segment], |row| row.get(0))?
            .collect::<Result<_, _>>()?)
    }

    pub fn finish_saving(&self) -> Result<(), AudioArchiveError> {
        self.conn.execute(
            "UPDATE archive_run SET save_complete=1 WHERE id=1 AND total_parts=saved_parts+rejected_parts",
            [],
        )?;
        Ok(())
    }

    fn read_record(&self) -> Result<AudioArchiveRecord, AudioArchiveError> {
        let mut query = self
            .conn
            .prepare("SELECT payload FROM members ORDER BY ordinal")?;
        let members = query
            .query_map([], |row| row.get::<_, String>(0))?
            .map(|row| Ok(serde_json::from_str(&row?)?))
            .collect::<Result<_, AudioArchiveError>>()?;
        let mut query = self
            .conn
            .prepare("SELECT payload,outcome FROM parts ORDER BY ordinal")?;
        let parts = query
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .map(|row| {
                let (payload, outcome) = row?;
                let mut part: AudioArchiveRecordPart = serde_json::from_str(&payload)?;
                part.outcome = parse_outcome(&outcome)?;
                validate_part(&part)?;
                Ok(part)
            })
            .collect::<Result<_, AudioArchiveError>>()?;
        Ok(AudioArchiveRecord {
            version: AUDIO_ARCHIVE_RECORD_VERSION,
            settings: self.settings()?,
            members,
            parts,
        })
    }
}

fn read_counter(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<u64> {
    let value: i64 = row.get(index)?;
    u64::try_from(value).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            index,
            rusqlite::types::Type::Integer,
            Box::new(error),
        )
    })
}

/// Validate that a resumed run's settings match caller inputs exactly.
pub fn validate_resume_settings(
    record: &AudioArchiveRecord,
    owner_zone: &TimeZone,
    settings: &AudioArchiveSettings,
    stream: &str,
) -> Result<(), AudioArchiveError> {
    let requested = owner_zone.iana_name().unwrap_or_default();
    if record.settings.owner_zone != requested {
        return Err(AudioArchiveError::SettingsMismatch(format!(
            "owner zone mismatch: saved '{}', requested '{}'",
            record.settings.owner_zone, requested
        )));
    }
    if record.settings.stream != stream {
        return Err(AudioArchiveError::SettingsMismatch(format!(
            "stream mismatch: saved '{}', requested '{}'",
            record.settings.stream, stream
        )));
    }
    if record.settings.qualifier != settings.qualifier {
        return Err(AudioArchiveError::SettingsMismatch(format!(
            "qualifier mismatch: saved '{}', requested '{}'",
            record.settings.qualifier, settings.qualifier
        )));
    }
    if record.settings.archive_identity != settings.archive_identity {
        return Err(AudioArchiveError::SettingsMismatch(format!(
            "archive identity mismatch: saved '{}', requested '{}'",
            record.settings.archive_identity, settings.archive_identity
        )));
    }
    if record.settings.window_ms != AUDIO_ARCHIVE_WINDOW_MS {
        return Err(AudioArchiveError::SettingsMismatch(format!(
            "window_ms mismatch: saved '{}', expected '{AUDIO_ARCHIVE_WINDOW_MS}'",
            record.settings.window_ms
        )));
    }
    if record.settings.len != AUDIO_ARCHIVE_LEN_SECONDS {
        return Err(AudioArchiveError::SettingsMismatch(format!(
            "len mismatch: saved '{}', expected '{AUDIO_ARCHIVE_LEN_SECONDS}'",
            record.settings.len
        )));
    }
    Ok(())
}

/// Compute summary size metrics from an existing record without re-running the planner.
pub fn compute_size_from_record(record: &AudioArchiveRecord) -> AudioArchiveSize {
    let mut bytes = 0u64;
    let mut days_set = BTreeSet::new();
    let mut left_out_map = BTreeMap::new();

    for member in &record.members {
        match &member.disposition {
            MemberDisposition::Included => {
                bytes += member.byte_len;
            }
            MemberDisposition::LeftOut(reason) => {
                *left_out_map
                    .entry(reason.kind_key().to_owned())
                    .or_insert(0) += 1;
            }
        }
    }

    let mut tiles_set = BTreeSet::new();
    for part in &record.parts {
        days_set.insert(part.day.clone());
        tiles_set.insert((part.day.clone(), part.segment_key.clone()));
    }

    let span =
        if let (Some(first), Some(last)) = (days_set.iter().next(), days_set.iter().next_back()) {
            Some((first.clone(), last.clone()))
        } else {
            None
        };

    AudioArchiveSize {
        members: record.members.len(),
        source_bytes: record.members.iter().map(|member| member.byte_len).sum(),
        bytes,
        days: days_set.len(),
        span,
        tiles: tiles_set.len(),
        left_out: left_out_map,
    }
}
