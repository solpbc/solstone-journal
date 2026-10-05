// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Durable, journal-rooted MCP interaction audit records and the owner-only
//! reader over them.
//!
//! Two immutable siblings describe one tool call. `interaction.json` is the
//! **admission**: it is published before the tool executes, so a served call
//! without one is impossible. `outcome.json` is published after the prepared
//! response has been approved for release. A segment holding an admission and
//! no outcome reads as *uncertain* — which is exactly why [`Outcome`], the
//! *written* vocabulary, has no such variant: the process that would write it
//! is the one that did not survive. That value exists only in the owner's
//! reader, where it is derived from the absent file.
//!
//! ⛔ **There is no projection table.** The canonical records are the
//! projection; the owner's reader walks this stream directly, in
//! `solstone-core-mcp-endpoint`'s `activity` module. That stream is excluded
//! from the chunk index twice over — by stream marker and by path component —
//! so the log a connection's calls produce is unreachable by those same calls,
//! with no second store to keep excluded.
//!
//! ⚠ **This crate stays a write-only, dependency-pure leaf.** Its public write
//! surface returns coordinates and never interaction contents, so the endpoint
//! that serves agents cannot obtain a record from here; that is pinned by
//! `mcp_audit_boundary` in `solstone-core-repository-contracts`. The owner's
//! read lives outside it.

use std::error::Error;
use std::fmt;
use std::path::Path;

use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use solstone_core_journal_io::atomic::{
    DetailedAtomicError, ExclusivePublication, write_bytes_exclusive_detailed,
};
use solstone_core_journal_io::{
    AtomicWriteError, AtomicWriteOptions, FinalNameConfirmation, MetadataDurability, PathError,
    SegmentDeconflictError, StageCleanup, day_path, find_available_segment, segment_path,
};

/// The admission file inside one audit segment.
pub const INTERACTION_FILE: &str = "interaction.json";
/// The outcome file inside one audit segment.
pub const OUTCOME_FILE: &str = "outcome.json";
const MAX_SEGMENT_ATTEMPTS: usize = 128;

/// The chronicle stream every MCP audit record is published under.
pub const AUDIT_STREAM: &str = "mcp.agent";

/// The admission schema this build writes. Schema 1 records carry neither a
/// connection key nor a request, and are left exactly as they were found.
pub const INTERACTION_SCHEMA: u32 = 3;

/// The outcome schema this build writes.
pub const OUTCOME_SCHEMA: u32 = 1;

/// Upper bound on the serialized recorded arguments. The tool validators bound
/// every field already; this is the second bound, so a future argument cannot
/// silently make an audit record unbounded.
pub const MAX_ARGUMENT_BYTES: usize = 8 * 1024;

/// Upper bound on how many served targets one outcome record names.
pub const MAX_RECORDED_TARGETS: usize = 50;

/// MCP tools that may be represented in an interaction record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolName {
    ListFacets,
    Search,
    Fetch,
    ListTranscripts,
    GetTranscript,
    ListEntities,
    GetEntity,
    SaveMemory,
    RecallMemory,
}

impl ToolName {
    /// The wire token this tool is recorded under.
    #[must_use]
    pub const fn token(self) -> &'static str {
        match self {
            Self::ListFacets => "list_facets",
            Self::Search => "search",
            Self::Fetch => "fetch",
            Self::ListTranscripts => "list_transcripts",
            Self::GetTranscript => "get_transcript",
            Self::ListEntities => "list_entities",
            Self::GetEntity => "get_entity",
            Self::SaveMemory => "save_memory",
            Self::RecallMemory => "recall_memory",
        }
    }

    /// Parse an owner-supplied filter token. Unknown tokens are rejected rather
    /// than widened to "any tool".
    #[must_use]
    pub fn from_token(value: &str) -> Option<Self> {
        [
            Self::ListFacets,
            Self::Search,
            Self::Fetch,
            Self::ListTranscripts,
            Self::GetTranscript,
            Self::ListEntities,
            Self::GetEntity,
            Self::SaveMemory,
            Self::RecallMemory,
        ]
        .into_iter()
        .find(|tool| tool.token() == value)
    }
}

/// What an agent asked for, as the dispatcher validated it.
///
/// ⚠ These are untrusted local strings. An owner surface renders them as data;
/// nothing in this crate or its callers interprets them as instructions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestRecord {
    /// Validated, normalized tool arguments. An opaque reference argument is
    /// recorded as a short fingerprint, never verbatim: a reference is a
    /// process-local capability whose bytes tell an owner nothing.
    pub arguments: Map<String, Value>,
    /// True when the arguments exceeded [`MAX_ARGUMENT_BYTES`] and were dropped
    /// rather than truncated into something that reads as complete.
    #[serde(default, skip_serializing_if = "is_false")]
    pub arguments_omitted: bool,
    /// The exact permission generation used to admit this request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission: Option<PermissionSnapshotRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PermissionSnapshotRecord {
    pub generation: u64,
    pub categories: Vec<String>,
    pub scope: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub facet_ids: Vec<String>,
}

/// One MCP tool interaction admitted into the journal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InteractionRecord {
    /// Absent on disk means schema 1 — the three-field record this crate wrote
    /// before the owner activity log existed.
    #[serde(default = "schema_one")]
    pub schema: u32,
    pub agent_identity: String,
    pub timestamp: DateTime<Utc>,
    pub tool_name: ToolName,
    /// The durable connection key. Schema 2 and later.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection: Option<String>,
    /// Schema 2 and later.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request: Option<RequestRecord>,
}

const fn schema_one() -> u32 {
    1
}

const fn is_false(value: &bool) -> bool {
    !*value
}

/// The outcome vocabulary a completed call can record about itself.
///
/// ⛔ There is deliberately no `Uncertain` variant: an uncertain call is one
/// whose outcome record was never published, which is a fact about a *missing*
/// file and cannot be written into a present one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// Authorized, executed, and at least one record prepared for release.
    Served,
    /// Authorized and executed; the response carried no records.
    Empty,
    /// Denied by the connection's permission.
    Refused,
    /// Authorized, but the tool could not complete.
    Error,
    Stored,
    Replayed,
    Deleted,
}

impl Outcome {
    #[must_use]
    pub const fn token(self) -> &'static str {
        match self {
            Self::Served => "served",
            Self::Empty => "empty",
            Self::Refused => "refused",
            Self::Error => "error",
            Self::Stored => "stored",
            Self::Replayed => "replayed",
            Self::Deleted => "deleted",
        }
    }
}

/// The shape of what came back — never the content.
///
/// Results are replayable and can be large, so the log
/// stores the reference plus a digest. The digest is what survives the journal
/// changing underneath: re-fetch the target, compare, and the owner knows
/// whether it still means what it meant then.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResultShape {
    /// How many records the prepared response carried.
    pub count: usize,
    /// Owner coordinates for what was served, bounded by
    /// [`MAX_RECORDED_TARGETS`]. ⛔ Never the records themselves.
    pub targets: Vec<String>,
    /// True when `targets` names fewer than `count` records.
    #[serde(default, skip_serializing_if = "is_false")]
    pub targets_truncated: bool,
    /// SHA-256, hex, over the served content with every `reference` and
    /// `next_cursor` removed and object keys sorted. It is a digest of the
    /// content, not of the exact bytes on the wire.
    pub digest: String,
    /// Source-keyed origin for a private memory record.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<Value>,
    /// First reservation time for a private memory record.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<DateTime<Utc>>,
    /// Exact UTF-8 byte count for a private memory record.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub byte_count: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory: Option<MemoryResultFacts>,
}

/// Body-free facts about the final prepared memory result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryResultFacts {
    pub complete: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub own_recall_ready: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ordinary_readable: Option<bool>,
}

/// One MCP tool outcome, published after the prepared response is approved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutcomeRecord {
    pub schema: u32,
    pub timestamp: DateTime<Utc>,
    pub outcome: Outcome,
    /// The owner-visible reason. ⚠ Deliberately richer than the wire refusal,
    /// which stays closed and indistinguishable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<ResultShape>,
}

/// Location of one durably published interaction record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditCoordinates {
    pub day: NaiveDate,
    pub stream: String,
    pub segment: String,
}

/// What the dispatcher admits before a tool executes.
#[derive(Debug, Clone)]
pub struct Admission<'a> {
    pub connection: &'a str,
    pub agent_identity: &'a str,
    pub tool_name: ToolName,
    pub arguments: Map<String, Value>,
    pub permission: Option<PermissionSnapshotRecord>,
}

/// Failure while publishing an MCP audit record.
#[derive(Debug)]
pub enum AuditWriteError {
    DayPath(PathError),
    SegmentAllocation(SegmentDeconflictError),
    SegmentPath(PathError),
    NoAvailableSegment,
    Serialization(serde_json::Error),
    AtomicWrite(AtomicWriteError),
    DetailedAtomic(DetailedAtomicError),
    /// Publication occurred, but the final name, cleanup, or metadata durability
    /// could not be confirmed. The destination may exist.
    PublishedUnconfirmed {
        path: std::path::PathBuf,
    },
    ParentSync(PathError),
}

impl fmt::Display for AuditWriteError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DayPath(error) => write!(formatter, "could not resolve MCP audit day: {error}"),
            Self::SegmentAllocation(error) => {
                write!(formatter, "could not allocate MCP audit segment: {error}")
            }
            Self::SegmentPath(error) => {
                write!(formatter, "could not create MCP audit segment: {error}")
            }
            Self::NoAvailableSegment => write!(formatter, "no MCP audit segment was available"),
            Self::Serialization(error) => {
                write!(
                    formatter,
                    "could not serialize MCP audit interaction: {error}"
                )
            }
            Self::AtomicWrite(error) => {
                write!(
                    formatter,
                    "could not publish MCP audit interaction: {error}"
                )
            }
            Self::DetailedAtomic(error) => {
                write!(
                    formatter,
                    "could not publish MCP audit interaction: {error}"
                )
            }
            Self::PublishedUnconfirmed { path } => write!(
                formatter,
                "MCP audit publication {} could not be confirmed",
                path.display()
            ),
            Self::ParentSync(error) => {
                write!(formatter, "could not sync MCP audit parent: {error}")
            }
        }
    }
}

impl Error for AuditWriteError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::DayPath(error) | Self::SegmentPath(error) => Some(error),
            Self::SegmentAllocation(error) => Some(error),
            Self::Serialization(error) => Some(error),
            Self::AtomicWrite(error) => Some(error),
            Self::DetailedAtomic(error) => Some(error),
            Self::ParentSync(error) => Some(error),
            Self::PublishedUnconfirmed { .. } => None,
            Self::NoAvailableSegment => None,
        }
    }
}

/// Publish one create-exclusive MCP admission record.
///
/// `now` is captured by the caller exactly once and drives both the record
/// timestamp and its chronicle day/segment coordinates. The record keeps the
/// UTC instant; the day and segment are the wall time in `now`'s zone, which
/// production passes as the journal's local zone so the record files under the
/// same day as everything else that happened then.
pub fn write_interaction_record<Z: TimeZone>(
    journal_root: &Path,
    now: DateTime<Z>,
    admission: &Admission<'_>,
) -> Result<AuditCoordinates, AuditWriteError> {
    write_interaction_record_with_before_publish(journal_root, now, admission, || {})
}

fn bounded_request(
    arguments: &Map<String, Value>,
    permission: Option<PermissionSnapshotRecord>,
) -> RequestRecord {
    let within_bound = serde_json::to_vec(arguments)
        .map(|bytes| bytes.len() <= MAX_ARGUMENT_BYTES)
        .unwrap_or(false);
    if within_bound {
        RequestRecord {
            arguments: arguments.clone(),
            arguments_omitted: false,
            permission,
        }
    } else {
        // ⛔ Dropped whole rather than truncated: a truncated argument reads as
        // the argument, and an audit record that lies about what was asked is
        // worse than one that says it could not say.
        RequestRecord {
            arguments: Map::new(),
            arguments_omitted: true,
            permission,
        }
    }
}

fn write_interaction_record_with_before_publish<Z, F>(
    journal_root: &Path,
    now: DateTime<Z>,
    admission: &Admission<'_>,
    mut before_publish: F,
) -> Result<AuditCoordinates, AuditWriteError>
where
    Z: TimeZone,
    F: FnMut(),
{
    let wall = now.naive_local();
    let now = now.with_timezone(&Utc);
    let day = wall.date();
    let day_key = day.format("%Y%m%d").to_string();
    let day_directory = day_path(journal_root, &day_key, true).map_err(AuditWriteError::DayPath)?;
    let stream_directory = day_directory.join(AUDIT_STREAM);
    let record = InteractionRecord {
        schema: INTERACTION_SCHEMA,
        agent_identity: admission.agent_identity.to_owned(),
        timestamp: now,
        tool_name: admission.tool_name,
        connection: Some(admission.connection.to_owned()),
        request: Some(bounded_request(
            &admission.arguments,
            admission.permission.clone(),
        )),
    };
    let contents = serde_json::to_vec(&record).map_err(AuditWriteError::Serialization)?;
    let mut candidate = format!("{}_1", wall.format("%H%M%S"));

    for _ in 0..MAX_SEGMENT_ATTEMPTS {
        let segment = find_available_segment(&stream_directory, &candidate, MAX_SEGMENT_ATTEMPTS)
            .map_err(AuditWriteError::SegmentAllocation)?
            .ok_or(AuditWriteError::NoAvailableSegment)?;
        let segment_directory = segment_path(journal_root, &day_key, &segment, AUDIT_STREAM, true)
            .map_err(AuditWriteError::SegmentPath)?;
        sync_audit_parent_chain(journal_root, &day_key, &segment)?;
        before_publish();
        let path = segment_directory.join(INTERACTION_FILE);
        match write_bytes_exclusive_detailed(&path, &contents, AtomicWriteOptions::default()) {
            Ok(publication) if exclusive_confirmed(&publication) => {
                return Ok(AuditCoordinates {
                    day,
                    stream: AUDIT_STREAM.to_owned(),
                    segment,
                });
            }
            Ok(_) => return Err(AuditWriteError::PublishedUnconfirmed { path }),
            Err(error) if error.source.kind() == std::io::ErrorKind::AlreadyExists => {
                candidate = segment;
            }
            Err(error) => return Err(AuditWriteError::DetailedAtomic(error)),
        }
    }

    Err(AuditWriteError::NoAvailableSegment)
}

/// Publish the outcome sibling of one already-admitted interaction.
///
/// ⚠ Create-exclusive like its admission: an outcome is written once and never
/// amended. A failure here leaves the admission alone and therefore leaves the
/// call reading as *uncertain* in the owner's reader, which is the honest state.
pub fn write_outcome_record(
    journal_root: &Path,
    coordinates: &AuditCoordinates,
    now: DateTime<Utc>,
    outcome: Outcome,
    reason: Option<&str>,
    result: Option<ResultShape>,
) -> Result<(), AuditWriteError> {
    let day_key = coordinates.day.format("%Y%m%d").to_string();
    let segment_directory = segment_path(
        journal_root,
        &day_key,
        &coordinates.segment,
        &coordinates.stream,
        false,
    )
    .map_err(AuditWriteError::SegmentPath)?;
    let record = OutcomeRecord {
        schema: OUTCOME_SCHEMA,
        timestamp: now,
        outcome,
        reason: reason.map(str::to_owned),
        result,
    };
    let contents = serde_json::to_vec(&record).map_err(AuditWriteError::Serialization)?;
    sync_audit_parent_chain(journal_root, &day_key, &coordinates.segment)?;
    let path = segment_directory.join(OUTCOME_FILE);
    match write_bytes_exclusive_detailed(&path, &contents, AtomicWriteOptions::default()) {
        Ok(publication) if exclusive_confirmed(&publication) => Ok(()),
        Ok(_) => Err(AuditWriteError::PublishedUnconfirmed { path }),
        Err(error) => Err(AuditWriteError::DetailedAtomic(error)),
    }
}

fn exclusive_confirmed(publication: &ExclusivePublication) -> bool {
    let name = matches!(
        publication.final_name,
        FinalNameConfirmation::Confirmed { .. }
    );
    let cleanup = matches!(publication.cleanup, StageCleanup::Removed);
    #[cfg(unix)]
    let durability = matches!(publication.durability, MetadataDurability::Confirmed);
    #[cfg(windows)]
    let durability = matches!(
        publication.durability,
        MetadataDurability::Unproven { source: None }
    );
    name && cleanup && durability
}

fn sync_audit_parent_chain(
    journal_root: &Path,
    day: &str,
    segment: &str,
) -> Result<(), AuditWriteError> {
    // Sync each containing directory in the chain so a newly created parent
    // entry is durable before the child admission/outcome is published.
    #[cfg(unix)]
    solstone_core_journal_io::sync_root(journal_root).map_err(AuditWriteError::ParentSync)?;
    for relative in [
        "chronicle".to_owned(),
        format!("chronicle/{day}"),
        format!("chronicle/{day}/{AUDIT_STREAM}"),
        format!("chronicle/{day}/{AUDIT_STREAM}/{segment}"),
    ] {
        solstone_core_journal_io::sync_dir(journal_root, &relative)
            .map_err(AuditWriteError::ParentSync)?;
    }
    Ok(())
}

/// Build a bounded result shape from the owner coordinates a call served.
#[must_use]
pub fn result_shape(count: usize, targets: Vec<String>, digest: String) -> ResultShape {
    let truncated = targets.len() > MAX_RECORDED_TARGETS || targets.len() < count;
    let mut targets = targets;
    targets.truncate(MAX_RECORDED_TARGETS);
    ResultShape {
        count,
        targets,
        targets_truncated: truncated,
        digest,
        origin: None,
        created_at: None,
        byte_count: None,
        memory: None,
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
    use std::sync::{Arc, Barrier};
    use std::thread;

    use chrono::{FixedOffset, TimeZone, Utc};
    use serde_json::json;

    use super::{
        Admission, INTERACTION_FILE, INTERACTION_SCHEMA, InteractionRecord, MAX_ARGUMENT_BYTES,
        MAX_RECORDED_TARGETS, OUTCOME_FILE, Outcome, RequestRecord, ToolName, result_shape,
        write_interaction_record, write_interaction_record_with_before_publish,
        write_outcome_record,
    };

    static SEQUENCE: AtomicU64 = AtomicU64::new(0);

    fn journal_root() -> PathBuf {
        #[cfg(unix)]
        let scratch = PathBuf::from("/var/tmp");
        #[cfg(windows)]
        let scratch = std::env::temp_dir();
        let root = scratch.join(format!(
            "solstone-mcp-audit-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn admission(connection: &str, tool_name: ToolName) -> Admission<'_> {
        Admission {
            connection,
            agent_identity: connection,
            tool_name,
            arguments: serde_json::Map::new(),
            permission: None,
        }
    }

    #[test]
    fn memory_vocabulary_and_legacy_result_shapes_round_trip() {
        assert_eq!(
            ToolName::from_token("save_memory"),
            Some(ToolName::SaveMemory)
        );
        assert_eq!(ToolName::SaveMemory.token(), "save_memory");
        assert_eq!(
            ToolName::from_token("recall_memory"),
            Some(ToolName::RecallMemory)
        );
        assert_eq!(ToolName::RecallMemory.token(), "recall_memory");
        for (outcome, token) in [
            (Outcome::Stored, "stored"),
            (Outcome::Replayed, "replayed"),
            (Outcome::Deleted, "deleted"),
        ] {
            assert_eq!(outcome.token(), token);
            assert_eq!(
                serde_json::from_str::<Outcome>(&format!("\"{token}\"")).unwrap(),
                outcome
            );
        }
        let legacy: super::ResultShape = serde_json::from_value(json!({
            "count": 1,
            "targets": [],
            "digest": "legacy"
        }))
        .unwrap();
        assert!(legacy.origin.is_none());
        assert!(legacy.created_at.is_none());
        assert!(legacy.byte_count.is_none());
        assert!(legacy.memory.is_none());
        let facts = super::MemoryResultFacts {
            complete: false,
            own_recall_ready: None,
            ordinary_readable: None,
        };
        let encoded = serde_json::to_value(&facts).unwrap();
        assert_eq!(
            serde_json::from_value::<super::MemoryResultFacts>(encoded).unwrap(),
            facts
        );
    }

    #[test]
    fn an_admission_serializes_only_the_closed_schema_three_fields() {
        let record = InteractionRecord {
            schema: INTERACTION_SCHEMA,
            agent_identity: "operator".to_owned(),
            timestamp: Utc.with_ymd_and_hms(2026, 8, 31, 12, 34, 56).unwrap(),
            tool_name: ToolName::Search,
            connection: Some("bearer:abc".to_owned()),
            request: Some(RequestRecord {
                arguments: json!({"query": "budget"}).as_object().cloned().unwrap(),
                arguments_omitted: false,
                permission: None,
            }),
        };

        assert_eq!(
            serde_json::to_value(record).unwrap(),
            json!({
                "schema": 3,
                "agent_identity": "operator",
                "timestamp": "2026-08-31T12:34:56Z",
                "tool_name": "search",
                "connection": "bearer:abc",
                "request": {"arguments": {"query": "budget"}},
            })
        );
    }

    #[test]
    fn a_schema_one_record_deserializes_without_inventing_its_missing_halves() {
        let record = serde_json::from_value::<InteractionRecord>(json!({
            "agent_identity": "operator",
            "timestamp": "2026-08-31T01:02:03Z",
            "tool_name": "fetch",
        }))
        .unwrap();

        assert_eq!(record.schema, 1);
        assert_eq!(record.tool_name, ToolName::Fetch);
        // ⛔ Nothing is synthesized for a record that carries neither half.
        assert!(record.connection.is_none());
        assert!(record.request.is_none());
    }

    #[test]
    fn writes_one_closed_record_at_the_returned_coordinates() {
        let root = journal_root();
        let now = Utc.with_ymd_and_hms(2026, 8, 31, 12, 34, 56).unwrap();

        let coordinates =
            write_interaction_record(&root, now, &admission("bearer:one", ToolName::Fetch))
                .unwrap();

        assert_eq!(coordinates.day.to_string(), "2026-08-31");
        assert_eq!(coordinates.stream, "mcp.agent");
        assert_eq!(coordinates.segment, "123456_1");
        let record =
            fs::read_to_string(root.join("chronicle/20260831/mcp.agent/123456_1/interaction.json"))
                .unwrap();
        let record: serde_json::Value = serde_json::from_str(&record).unwrap();
        assert_eq!(record["tool_name"], "fetch");
        assert_eq!(record["connection"], "bearer:one");
        assert_eq!(record["schema"], 3);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn an_evening_call_files_under_the_local_day_and_keeps_the_utc_instant() {
        let root = journal_root();
        // 19:30 on Sep 28 at UTC-6 is 01:30 UTC on Sep 29.
        let now = FixedOffset::west_opt(6 * 3600)
            .unwrap()
            .with_ymd_and_hms(2026, 9, 28, 19, 30, 0)
            .unwrap();

        let coordinates =
            write_interaction_record(&root, now, &admission("bearer:one", ToolName::Fetch))
                .unwrap();

        assert_eq!(coordinates.day.to_string(), "2026-09-28");
        assert_eq!(coordinates.segment, "193000_1");
        let record =
            fs::read_to_string(root.join("chronicle/20260928/mcp.agent/193000_1/interaction.json"))
                .unwrap();
        let record: serde_json::Value = serde_json::from_str(&record).unwrap();
        assert_eq!(record["timestamp"], "2026-09-29T01:30:00Z");

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn an_outcome_is_written_once_and_never_amended() {
        let root = journal_root();
        let coordinates = write_interaction_record(
            &root,
            Utc.with_ymd_and_hms(2026, 8, 31, 12, 0, 0).unwrap(),
            &admission("bearer:one", ToolName::Search),
        )
        .unwrap();
        let now = Utc.with_ymd_and_hms(2026, 8, 31, 12, 0, 1).unwrap();
        write_outcome_record(&root, &coordinates, now, Outcome::Empty, None, None).unwrap();
        assert!(
            write_outcome_record(&root, &coordinates, now, Outcome::Served, None, None).is_err()
        );
        let stored = fs::read_to_string(
            root.join("chronicle/20260831/mcp.agent")
                .join(&coordinates.segment)
                .join(OUTCOME_FILE),
        )
        .unwrap();
        assert!(stored.contains("\"empty\""));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn the_written_outcome_vocabulary_cannot_express_uncertain() {
        // An uncertain call is one whose outcome file was never published. It is
        // a fact about a missing file and has no representation in a present one.
        assert!(serde_json::from_str::<Outcome>("\"uncertain\"").is_err());
        for token in ["served", "empty", "refused", "error"] {
            assert!(serde_json::from_str::<Outcome>(&format!("\"{token}\"")).is_ok());
        }
    }

    #[test]
    fn oversized_arguments_are_dropped_whole_rather_than_truncated() {
        let root = journal_root();
        let mut arguments = serde_json::Map::new();
        arguments.insert(
            "query".to_owned(),
            json!("x".repeat(MAX_ARGUMENT_BYTES + 1)),
        );
        let coordinates = write_interaction_record(
            &root,
            Utc.with_ymd_and_hms(2026, 8, 31, 12, 0, 0).unwrap(),
            &Admission {
                connection: "bearer:one",
                agent_identity: "bearer:one",
                tool_name: ToolName::Search,
                arguments,
                permission: None,
            },
        )
        .unwrap();

        let stored = fs::read_to_string(
            root.join("chronicle/20260831/mcp.agent")
                .join(&coordinates.segment)
                .join(INTERACTION_FILE),
        )
        .unwrap();
        let stored: serde_json::Value = serde_json::from_str(&stored).unwrap();
        assert_eq!(stored["request"]["arguments_omitted"], true);
        assert_eq!(stored["request"]["arguments"], json!({}));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_result_shape_names_bounded_targets_and_says_when_it_truncated() {
        let targets = (0..MAX_RECORDED_TARGETS + 5)
            .map(|index| format!("20260831/default/{index:06}_1"))
            .collect::<Vec<_>>();
        let shape = result_shape(MAX_RECORDED_TARGETS + 5, targets, "digest".to_owned());
        assert_eq!(shape.count, MAX_RECORDED_TARGETS + 5);
        assert_eq!(shape.targets.len(), MAX_RECORDED_TARGETS);
        assert!(shape.targets_truncated);

        let exact = result_shape(2, vec!["a".to_owned(), "b".to_owned()], "d".to_owned());
        assert!(!exact.targets_truncated);
    }

    #[test]
    fn a_tool_filter_token_round_trips_and_an_unknown_token_is_refused() {
        for tool in [
            ToolName::ListFacets,
            ToolName::Search,
            ToolName::Fetch,
            ToolName::ListTranscripts,
            ToolName::GetTranscript,
            ToolName::ListEntities,
            ToolName::GetEntity,
        ] {
            assert_eq!(ToolName::from_token(tool.token()), Some(tool));
        }
        assert!(ToolName::from_token("everything").is_none());
    }

    #[test]
    fn concurrent_interactions_retry_an_exclusive_write_collision() {
        let root = Arc::new(journal_root());
        let now = Utc.with_ymd_and_hms(2026, 8, 31, 12, 34, 56).unwrap();
        let selected = Arc::new(Barrier::new(2));
        let selections = Arc::new(AtomicUsize::new(0));

        let first = {
            let root = Arc::clone(&root);
            let selected = Arc::clone(&selected);
            let selections = Arc::clone(&selections);
            thread::spawn(move || {
                write_interaction_record_with_before_publish(
                    &root,
                    now,
                    &admission("first-agent", ToolName::Search),
                    move || {
                        if selections.fetch_add(1, Ordering::SeqCst) < 2 {
                            selected.wait();
                        }
                    },
                )
                .expect("first concurrent record publishes")
            })
        };
        let second = {
            let root = Arc::clone(&root);
            let selected = Arc::clone(&selected);
            let selections = Arc::clone(&selections);
            thread::spawn(move || {
                write_interaction_record_with_before_publish(
                    &root,
                    now,
                    &admission("second-agent", ToolName::Fetch),
                    move || {
                        if selections.fetch_add(1, Ordering::SeqCst) < 2 {
                            selected.wait();
                        }
                    },
                )
                .expect("second concurrent record publishes")
            })
        };

        let first = first.join().expect("first concurrent writer joins");
        let second = second.join().expect("second concurrent writer joins");
        assert_ne!(first.segment, second.segment);
        for (coordinates, agent_identity, tool_name) in [
            (first, "first-agent", "search"),
            (second, "second-agent", "fetch"),
        ] {
            let record = fs::read_to_string(
                root.join("chronicle")
                    .join(coordinates.day.format("%Y%m%d").to_string())
                    .join(coordinates.stream)
                    .join(coordinates.segment)
                    .join(INTERACTION_FILE),
            )
            .expect("concurrent record exists");
            let record: serde_json::Value = serde_json::from_str(&record).expect("record is JSON");
            assert_eq!(record["agent_identity"], agent_identity);
            assert_eq!(record["tool_name"], tool_name);
        }

        fs::remove_dir_all(root.as_ref()).unwrap();
    }
}
