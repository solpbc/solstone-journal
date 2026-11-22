// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! What the journal keeps under `social/`: the activities it shared, so a
//! sync can bring each shared copy up to date or remove it, and the
//! activities others shared with it, each marked shared and attributed to its
//! sharer. Nothing under `social/` is offered to talents, agents, search or
//! the dashboard: a received memory is someone else's account.

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use solstone_core_journal_io::atomic::{JsonWriteOptions, write_json};

use crate::record::{ActivityRecord, ActivityRef, RecordError, build_record, hex};
use crate::space::{
    RemoteRecord, Session, SpaceError, delete_activity, list_shared_activities, put_activity,
    space_authority,
};

const SOCIAL_DIR: &str = "social";

#[derive(Debug)]
pub enum StoreError {
    Io(PathBuf, String),
    Space(SpaceError),
    Record(RecordError),
    UnknownShare,
}

impl fmt::Display for StoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(path, detail) => write!(formatter, "{}: {detail}", path.display()),
            Self::Space(err) => err.fmt(formatter),
            Self::Record(err) => err.fmt(formatter),
            Self::UnknownShare => formatter.write_str("no such share"),
        }
    }
}

impl std::error::Error for StoreError {}

impl From<SpaceError> for StoreError {
    fn from(err: SpaceError) -> Self {
        Self::Space(err)
    }
}

fn write(path: &Path, value: &impl Serialize) -> Result<(), StoreError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|err| StoreError::Io(parent.to_path_buf(), err.to_string()))?;
    }
    write_json(
        path,
        value,
        JsonWriteOptions {
            mode: Some(0o600),
            indent: Some(2),
            sort_keys: true,
        },
    )
    .map_err(|err| StoreError::Io(path.to_path_buf(), err.to_string()))
}

fn read<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<Option<T>, StoreError> {
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|err| StoreError::Io(path.to_path_buf(), err.to_string())),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(StoreError::Io(path.to_path_buf(), err.to_string())),
    }
}

// ---- shared by this journal ----------------------------------------------

/// One activity this journal shared.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Share {
    pub activity: ActivityRef,
    pub space: String,
    pub rkey: String,
    pub created_at: String,
    pub digest: String,
    pub uri: String,
    pub cid: String,
    pub written_at: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Shares {
    shares: Vec<Share>,
}

fn shares_path(journal: &Path) -> PathBuf {
    journal.join(SOCIAL_DIR).join("sent").join("shares.json")
}

/// The activities this journal has shared.
pub fn shares(journal: &Path) -> Result<Vec<Share>, StoreError> {
    Ok(read::<Shares>(&shares_path(journal))?
        .unwrap_or_default()
        .shares)
}

fn save_shares(journal: &Path, shares: Vec<Share>) -> Result<(), StoreError> {
    write(&shares_path(journal), &Shares { shares })
}

/// A record key for a new share: a TID, the usual key for records written
/// over time. It says nothing about the activity it names.
fn new_rkey(now: Timestamp) -> String {
    const ALPHABET: &[u8] = b"234567abcdefghijklmnopqrstuvwxyz";
    let micros = now.as_microsecond().max(0) as u64;
    let mut random = [0u8; 2];
    let _ = ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut random);
    let clock = u64::from(u16::from_le_bytes(random)) & 0x3ff;
    let value = ((micros & ((1 << 53) - 1)) << 10) | clock;
    (0..13)
        .rev()
        .map(|index| char::from(ALPHABET[((value >> (index * 5)) & 31) as usize]))
        .collect()
}

/// Share one activity into `space` (which the signed-in account created and
/// added its readers to).
pub fn share_activity(
    journal: &Path,
    session: &Session<'_>,
    activity: &ActivityRef,
    space: &str,
    now: Timestamp,
) -> Result<Share, StoreError> {
    let mut all = shares(journal)?;
    if let Some(existing) = all
        .iter()
        .find(|s| &s.activity == activity && s.space == space)
    {
        return Ok(existing.clone());
    }
    let record = build_record(journal, activity, now, now).map_err(StoreError::Record)?;
    let rkey = new_rkey(now);
    let written = put_activity(session, space, &rkey, &record_value(&record))?;
    let share = Share {
        activity: activity.clone(),
        space: space.to_owned(),
        rkey,
        created_at: record.created_at.clone(),
        digest: record.content_digest(),
        uri: written.uri,
        cid: written.cid,
        written_at: now.to_string(),
    };
    all.push(share.clone());
    save_shares(journal, all)?;
    Ok(share)
}

fn record_value(record: &ActivityRecord) -> Value {
    serde_json::to_value(record).unwrap_or(Value::Null)
}

/// What one sync pass did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SyncReport {
    pub updated: usize,
    pub unchanged: usize,
    pub removed: usize,
}

/// Bring every shared copy up to date with its activity: rewrite it when the
/// owner edited the activity or it gained new data, and remove it when the
/// activity, or every segment of it, is gone.
pub fn sync_shares(
    journal: &Path,
    session: &Session<'_>,
    now: Timestamp,
) -> Result<SyncReport, StoreError> {
    let mut report = SyncReport::default();
    let mut kept = Vec::new();
    for mut share in shares(journal)? {
        let created_at: Timestamp = share.created_at.parse().unwrap_or(now);
        match build_record(journal, &share.activity, created_at, now) {
            Ok(record) if record.content_digest() == share.digest => {
                report.unchanged += 1;
                kept.push(share);
            }
            Ok(record) => {
                let written =
                    put_activity(session, &share.space, &share.rkey, &record_value(&record))?;
                share.digest = record.content_digest();
                share.uri = written.uri;
                share.cid = written.cid;
                share.written_at = now.to_string();
                report.updated += 1;
                kept.push(share);
            }
            Err(RecordError::ActivityNotFound | RecordError::NoSegments) => {
                delete_activity(session, &share.space, &share.rkey)?;
                report.removed += 1;
            }
            Err(err) => return Err(StoreError::Record(err)),
        }
    }
    save_shares(journal, kept)?;
    Ok(report)
}

/// Stop sharing one activity: delete its record from the space.
pub fn unshare(
    journal: &Path,
    session: &Session<'_>,
    space: &str,
    rkey: &str,
) -> Result<(), StoreError> {
    let mut all = shares(journal)?;
    let index = all
        .iter()
        .position(|s| s.space == space && s.rkey == rkey)
        .ok_or(StoreError::UnknownShare)?;
    delete_activity(session, space, rkey)?;
    all.remove(index);
    save_shares(journal, all)
}

// ---- shared with this journal --------------------------------------------

/// Who shared a received memory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedBy {
    pub did: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handle: Option<String>,
}

/// One received memory, as stored: always marked shared.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReceivedMemory {
    pub shared: bool,
    pub shared_by: SharedBy,
    pub space: String,
    pub collection: String,
    pub cid: String,
    pub received_at: String,
    pub updated_at: String,
    pub record: Value,
}

/// A space this journal follows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceivedSpace {
    pub space: String,
    pub shared_by: SharedBy,
    pub added_at: String,
}

fn space_dir(journal: &Path, space: &str) -> PathBuf {
    let key = hex(&Sha256::digest(space.as_bytes()));
    journal.join(SOCIAL_DIR).join("received").join(&key[..16])
}

fn safe_rkey(rkey: &str) -> bool {
    !rkey.is_empty()
        && rkey.len() <= 512
        && rkey != "."
        && rkey != ".."
        && rkey != "space"
        && rkey
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_' | b':' | b'~'))
}

/// The spaces this journal follows.
pub fn followed_spaces(journal: &Path) -> Result<Vec<ReceivedSpace>, StoreError> {
    let dir = journal.join(SOCIAL_DIR).join("received");
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(StoreError::Io(dir, err.to_string())),
    };
    let mut spaces = Vec::new();
    for entry in entries {
        let path = entry
            .map_err(|err| StoreError::Io(dir.clone(), err.to_string()))?
            .path()
            .join("space.json");
        if let Some(space) = read::<ReceivedSpace>(&path)? {
            spaces.push(space);
        }
    }
    spaces.sort_by(|a, b| a.added_at.cmp(&b.added_at));
    Ok(spaces)
}

/// The received memories of one space, by record key.
pub fn received(
    journal: &Path,
    space: &str,
) -> Result<BTreeMap<String, ReceivedMemory>, StoreError> {
    let dir = space_dir(journal, space);
    let mut out = BTreeMap::new();
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(out),
        Err(err) => return Err(StoreError::Io(dir, err.to_string())),
    };
    for entry in entries {
        let path = entry
            .map_err(|err| StoreError::Io(dir.clone(), err.to_string()))?
            .path();
        let Some(rkey) = path
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.strip_suffix(".json"))
        else {
            continue;
        };
        if rkey == "space" {
            continue;
        }
        if let Some(memory) = read::<ReceivedMemory>(&path)? {
            out.insert(rkey.to_owned(), memory);
        }
    }
    Ok(out)
}

/// What a receive pass changed.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ReceiveReport {
    pub added: usize,
    pub updated: usize,
    pub removed: usize,
}

/// Decide, from what is held and what the sharer's host lists now, which
/// records to store and which held ones to drop. Only activity records are
/// taken; a held record the sharer no longer lists, or no longer lists as an
/// activity, is dropped.
pub fn plan_receive<'a>(
    held: &BTreeMap<String, ReceivedMemory>,
    listed: &'a [RemoteRecord],
) -> (Vec<&'a RemoteRecord>, Vec<String>) {
    let current: BTreeMap<&str, &RemoteRecord> = listed
        .iter()
        .filter(|r| {
            r.collection == crate::ACTIVITY_COLLECTION
                && safe_rkey(&r.rkey)
                && ActivityRecord::from_received(&r.value).is_some()
        })
        .map(|r| (r.rkey.as_str(), r))
        .collect();
    let store = current
        .values()
        .filter(|r| held.get(&r.rkey).is_none_or(|h| h.cid != r.cid))
        .copied()
        .collect();
    let drop = held
        .keys()
        .filter(|rkey| !current.contains_key(rkey.as_str()))
        .cloned()
        .collect();
    (store, drop)
}

/// Follow a space someone shared with this journal's owner and bring its
/// received memories up to date: new and edited activities are stored,
/// unshared ones are dropped.
pub fn receive_space(
    journal: &Path,
    session: &Session<'_>,
    space: &str,
    sharer_handle: Option<&str>,
    plc_directory: &str,
    now: Timestamp,
) -> Result<ReceiveReport, StoreError> {
    let shared_by = SharedBy {
        did: space_authority(space)?.to_owned(),
        handle: sharer_handle.map(ToOwned::to_owned),
    };
    // The space is followed once its sharer's host has let this account read
    // it, never on a reference that has not worked yet.
    let listed = list_shared_activities(session, space, plc_directory)?;
    let space_file = space_dir(journal, space).join("space.json");
    if read::<ReceivedSpace>(&space_file)?.is_none() {
        write(
            &space_file,
            &ReceivedSpace {
                space: space.to_owned(),
                shared_by: shared_by.clone(),
                added_at: now.to_string(),
            },
        )?;
    }
    apply_receive(journal, space, &shared_by, &listed, now)
}

/// Store and drop per [`plan_receive`].
pub fn apply_receive(
    journal: &Path,
    space: &str,
    shared_by: &SharedBy,
    listed: &[RemoteRecord],
    now: Timestamp,
) -> Result<ReceiveReport, StoreError> {
    let held = received(journal, space)?;
    let (store, drop) = plan_receive(&held, listed);
    let dir = space_dir(journal, space);
    let mut report = ReceiveReport::default();
    for remote in store {
        let received_at = held
            .get(&remote.rkey)
            .map(|h| h.received_at.clone())
            .unwrap_or_else(|| now.to_string());
        if held.contains_key(&remote.rkey) {
            report.updated += 1;
        } else {
            report.added += 1;
        }
        write(
            &dir.join(format!("{}.json", remote.rkey)),
            &ReceivedMemory {
                shared: true,
                shared_by: shared_by.clone(),
                space: space.to_owned(),
                collection: remote.collection.clone(),
                cid: remote.cid.clone(),
                received_at,
                updated_at: now.to_string(),
                record: remote.value.clone(),
            },
        )?;
    }
    for rkey in drop {
        // The received copy follows its sharer's unshare: the journal
        // dropping a mirror it keeps, not the owner deleting their own memory.
        let path = dir.join(format!("{rkey}.json"));
        fs::remove_file(&path).map_err(|err| StoreError::Io(path, err.to_string()))?;
        report.removed += 1;
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use solstone_core_oauth::atproto::DpopKeyPair;

    use super::*;
    use crate::ACTIVITY_COLLECTION;
    use crate::record::tests::activity_journal;
    use crate::space::tests::Routes;

    const ALICE: &str = "did:plc:aliceaaaaaaaaaaaaaaaaaaa";

    fn remote(rkey: &str, cid: &str, title: &str) -> RemoteRecord {
        RemoteRecord {
            collection: ACTIVITY_COLLECTION.to_owned(),
            rkey: rkey.to_owned(),
            cid: cid.to_owned(),
            value: json!({
                "$type": ACTIVITY_COLLECTION, "title": title, "startedAt": "2026-11-21T16:00:00Z",
                "endedAt": "2026-11-21T16:10:00Z", "segments": [], "createdAt": "x", "updatedAt": "y"
            }),
        }
    }

    #[test]
    fn received_memories_arrive_marked_shared_follow_edits_and_leave_on_unshare() {
        let dir = tempfile::tempdir().unwrap();
        let space = format!("at://{ALICE}/space/app.solstone.memory.space/s1");
        let by = SharedBy {
            did: ALICE.to_owned(),
            handle: Some("alice.test".to_owned()),
        };
        let t1: Timestamp = "2026-11-22T10:00:00Z".parse().unwrap();
        let t2: Timestamp = "2026-11-22T11:00:00Z".parse().unwrap();

        let first = apply_receive(
            dir.path(),
            &space,
            &by,
            &[remote("r1", "c1", "one"), remote("r2", "c2", "two")],
            t1,
        )
        .unwrap();
        assert_eq!(
            first,
            ReceiveReport {
                added: 2,
                updated: 0,
                removed: 0
            }
        );
        let held = received(dir.path(), &space).unwrap();
        assert!(held.values().all(|m| m.shared && m.shared_by == by));
        for path in fs::read_dir(space_dir(dir.path(), &space)).unwrap() {
            let rel = path
                .unwrap()
                .path()
                .strip_prefix(dir.path())
                .unwrap()
                .to_string_lossy()
                .into_owned();
            assert!(rel.starts_with("social/received/"), "{rel}");
        }

        // r1 edited, r2 unshared.
        let second = apply_receive(
            dir.path(),
            &space,
            &by,
            &[remote("r1", "c1b", "one, edited")],
            t2,
        )
        .unwrap();
        assert_eq!(
            second,
            ReceiveReport {
                added: 0,
                updated: 1,
                removed: 1
            }
        );
        let held = received(dir.path(), &space).unwrap();
        assert_eq!(held.len(), 1);
        assert_eq!(held["r1"].record["title"], "one, edited");
        assert_eq!(held["r1"].received_at, t1.to_string());

        // An unchanged listing changes nothing.
        let third = apply_receive(
            dir.path(),
            &space,
            &by,
            &[remote("r1", "c1b", "one, edited")],
            t2,
        )
        .unwrap();
        assert_eq!(third, ReceiveReport::default());
    }

    #[test]
    fn a_shared_activity_follows_edits_and_new_data_and_leaves_on_unshare() {
        let (dir, activity) = activity_journal();
        let journal = dir.path();
        let space = format!("at://{ALICE}/space/app.solstone.memory.space/s1");
        let put = "https://pds.alice.test/xrpc/com.atproto.space.putRecord";
        let delete = "https://pds.alice.test/xrpc/com.atproto.space.deleteRecord";
        let written = |cid: &str| json!({"uri": format!("at://{ALICE}/r/{cid}"), "cid": cid});
        let http = Routes::new(vec![
            (put, 200, written("c1")),
            (put, 200, written("c2")),
            (put, 200, written("c3")),
            (delete, 200, json!({})),
            (put, 200, written("c4")),
            (delete, 200, json!({})),
        ]);
        let key = DpopKeyPair::generate().unwrap();
        let session = Session {
            http: &http,
            did: ALICE,
            pds: "https://pds.alice.test",
            access_token: "t",
            dpop_key: &key,
            now_unix: 1,
            nonce: Default::default(),
        };
        let at = |time: &str| -> Timestamp { format!("2026-11-22T{time}:00Z").parse().unwrap() };
        let sent = |index: usize| -> Value {
            let requests = http.requests.lock().unwrap();
            serde_json::from_slice(requests[index].body.as_ref().unwrap()).unwrap()
        };
        let request_count = || http.requests.lock().unwrap().len();

        let share = share_activity(journal, &session, &activity, &space, at("10:00")).unwrap();
        assert_eq!(share.cid, "c1");
        assert_eq!(sent(0)["record"]["title"], "Sprint planning");
        assert_eq!(
            share_activity(journal, &session, &activity, &space, at("10:30")).unwrap(),
            share,
            "sharing the same activity again writes nothing"
        );
        let quiet = sync_shares(journal, &session, at("11:00")).unwrap();
        assert_eq!(quiet.unchanged, 1);
        assert_eq!(request_count(), 1, "an unchanged activity is not rewritten");

        // The owner edits the activity.
        let rows = journal.join("facets/work/activities/20261121.jsonl");
        let original = fs::read_to_string(&rows).unwrap();
        fs::write(
            &rows,
            original.replace("Sprint planning", "Sprint planning, round two"),
        )
        .unwrap();
        let edited = sync_shares(journal, &session, at("12:00")).unwrap();
        assert_eq!(edited.updated, 1);
        assert_eq!(sent(1)["rkey"], share.rkey.as_str());
        assert_eq!(sent(1)["record"]["title"], "Sprint planning, round two");
        assert_eq!(
            sent(1)["record"]["createdAt"],
            sent(0)["record"]["createdAt"]
        );

        // The activity gains a segment.
        let grown = fs::read_to_string(&rows)
            .unwrap()
            .replace(r#""091000_300"]"#, r#""091000_300","091500_300"]"#);
        fs::write(&rows, grown).unwrap();
        let talents = journal.join("chronicle/20261121/desk/091500_300/talents");
        fs::create_dir_all(&talents).unwrap();
        fs::write(
            talents.join("sense.json"),
            r#"{"activity_summary":"Wrote the tickets"}"#,
        )
        .unwrap();
        let more = sync_shares(journal, &session, at("13:00")).unwrap();
        assert_eq!(more.updated, 1);
        assert_eq!(sent(2)["rkey"], share.rkey.as_str());
        assert_eq!(sent(2)["record"]["segments"].as_array().unwrap().len(), 3);
        assert_eq!(sent(2)["record"]["endedAt"], "2026-11-21T16:20:00Z");
        assert_eq!(shares(journal).unwrap()[0].cid, "c3");

        // Unsharing deletes the record and forgets the share.
        unshare(journal, &session, &space, &share.rkey).unwrap();
        assert_eq!(sent(3)["rkey"], share.rkey.as_str());
        assert!(shares(journal).unwrap().is_empty());
        assert!(matches!(
            unshare(journal, &session, &space, &share.rkey),
            Err(StoreError::UnknownShare)
        ));

        // Shared again, then the owner deletes every segment: the copy goes.
        let again = share_activity(journal, &session, &activity, &space, at("14:00")).unwrap();
        for key in ["090000_300", "090500_300", "091500_300"] {
            fs::write(
                journal
                    .join("chronicle/20261121/desk")
                    .join(key)
                    .join("tombstone.json"),
                "{}",
            )
            .unwrap();
        }
        let gone = sync_shares(journal, &session, at("15:00")).unwrap();
        assert_eq!(gone.removed, 1);
        assert_eq!(sent(5)["rkey"], again.rkey.as_str());
        assert!(shares(journal).unwrap().is_empty());
        assert_eq!(request_count(), 6);
    }

    #[test]
    fn only_activity_records_with_safe_keys_are_taken() {
        let held = BTreeMap::new();
        let mut post = remote("p1", "c", "x");
        post.value["$type"] = json!("app.bsky.feed.post");
        let escape = remote("../../config/journal", "c", "x");
        let listed = [post, escape, remote("ok", "c", "fine")];
        let (store, drop) = plan_receive(&held, &listed);
        assert_eq!(
            store.iter().map(|r| r.rkey.as_str()).collect::<Vec<_>>(),
            ["ok"]
        );
        assert!(drop.is_empty());
    }

    #[test]
    fn record_keys_are_tids() {
        let now: Timestamp = "2026-11-22T10:00:00Z".parse().unwrap();
        let rkey = new_rkey(now);
        assert_eq!(rkey.len(), 13);
        assert!(
            rkey.bytes()
                .all(|b| b"234567abcdefghijklmnopqrstuvwxyz".contains(&b))
        );
        assert!(new_rkey("2026-11-23T10:00:00Z".parse().unwrap()) > rkey);
    }
}
