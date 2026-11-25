// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Resumable chunked import uploads under `/app/import/api/uploads`.

use std::{
    collections::{BTreeSet, HashMap},
    fs::{self, OpenOptions},
    io::{Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

use axum::{
    body::{Body, to_bytes},
    extract::{Path as AxumPath, Query, State},
    http::{HeaderMap, StatusCode, header},
    response::Response,
};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use solstone_core_import::{SourceHash, validate_timestamp};
use solstone_core_journal_io::{
    AtomicWriteOptions, atomic_replace, available_disk_bytes, contained_path,
    create_directory_with_mode, is_uuid_v4, mint_uuid_v4,
};

use crate::{
    AppState,
    callosum::emit_importer_event,
    http::{error, json as json_response},
    lifecycle::{
        APPLE_SOURCE, HEIC_REFUSAL_MESSAGE, STRAVA_SOURCE, VerifiedFileStageInput,
        VerifiedSourceFile, allowlisted_device_noun, evernote_upload_refusal, imported_manifest,
        is_heic_upload, now_ms, parse_and_validate_context, replay_summary, safe_filename,
        stage_verified_upload_file, staged_by_client_item, staged_by_source_hash, summary,
    },
    mobile_import_contract::{ImportErrorKind, ImportReasonCode, UploadReasonCode},
    multipart, save_stream, start_lock,
};

/// File size at or above which clients switch from single-POST `/save` to resumable uploads.
pub const RESUMABLE_THRESHOLD_BYTES: u64 = 16 * 1024 * 1024;
/// Fixed maximum chunk size for `PUT /app/import/api/uploads/{upload_id}`.
pub const RESUMABLE_CHUNK_BYTES: u64 = 4 * 1024 * 1024;
/// Maximum whole-file size accepted by resumable uploads (`4 GiB`).
pub const MAX_UPLOAD_FILE_BYTES: u64 = 4 * 1024 * 1024 * 1024;
/// Bridge drain ceiling (`8 MiB`), matching `spl-rust::journal_bridge` `MAX_UPLOAD_BODY`.
pub const BRIDGE_DRAIN_CEILING_BYTES: usize = 8 * 1024 * 1024;
/// Minimum free disk headroom required above an upload's remaining bytes (`256 MiB`).
pub const MIN_FREE_DISK_BYTES: u64 = 256 * 1024 * 1024;
/// Maximum aggregate bytes reserved across concurrent open `.uploads/` (`8 GiB`).
pub const MAX_ACTIVE_UPLOADS_BYTES: u64 = 8 * 1024 * 1024 * 1024;
/// Rolling inactivity retention window for uncommitted partial uploads (`7 days`).
pub const PARTIAL_UPLOAD_TTL_MS: i64 = 7 * 24 * 60 * 60 * 1_000;
/// Window within which an open upload reports `phase: "sending"` rather than `"paused"`.
pub const ACTIVE_PAUSED_THRESHOLD_MS: i64 = 30_000;
/// Directory under `<journal>/imports/` holding uncommitted partial uploads.
pub const UPLOADS_DIR: &str = ".uploads";

const PAYLOAD_PART_FILE: &str = "payload.part";
const META_JSON_FILE: &str = "meta.json";
const MAX_CLIENT_ITEM_ID_BYTES: usize = 256;
const MAX_CLIENT_BAG_BYTES: usize = 16 * 1024;
pub const MAX_JSON_CONTROL_BYTES: usize = 64 * 1024;
/// Minimum mtime age before `.save-tmp/` files not held by this process are swept as orphans.
pub const SAVE_TMP_ORPHAN_MIN_AGE: Duration = Duration::from_secs(60 * 60);
const PROGRESS_EVENT_THROTTLE: Duration = Duration::from_millis(500);
const RECENT_COMMIT_RETENTION: Duration = Duration::from_secs(15 * 60);

pub const UPLOAD_COLLECTION_PATH: &str = "/app/import/api/uploads";
pub const UPLOAD_ITEM_PATH: &str = "/app/import/api/uploads/{upload_id}";
pub const UPLOAD_COMMIT_PATH: &str = "/app/import/api/uploads/{upload_id}/commit";

/// FIPS 180-4 SHA-256 state with serializable midstate and `< 64`-byte tail.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResumableSha256 {
    state: [u32; 8],
    blocks_bytes: u64,
    tail: [u8; 64],
    tail_len: usize,
}

const SHA256_H0: [u32; 8] = [
    0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
];

const SHA256_K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

fn compress_block(state: &mut [u32; 8], block: &[u8; 64]) {
    let mut w = [0_u32; 64];
    for (index, word_bytes) in block.chunks_exact(4).enumerate() {
        w[index] = u32::from_be_bytes([word_bytes[0], word_bytes[1], word_bytes[2], word_bytes[3]]);
    }
    for index in 16..64 {
        let s0 =
            w[index - 15].rotate_right(7) ^ w[index - 15].rotate_right(18) ^ (w[index - 15] >> 3);
        let s1 =
            w[index - 2].rotate_right(17) ^ w[index - 2].rotate_right(19) ^ (w[index - 2] >> 10);
        w[index] = w[index - 16]
            .wrapping_add(s0)
            .wrapping_add(w[index - 7])
            .wrapping_add(s1);
    }

    let mut a = state[0];
    let mut b = state[1];
    let mut c = state[2];
    let mut d = state[3];
    let mut e = state[4];
    let mut f = state[5];
    let mut g = state[6];
    let mut h = state[7];

    for index in 0..64 {
        let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
        let ch = (e & f) ^ ((!e) & g);
        let temp1 = h
            .wrapping_add(s1)
            .wrapping_add(ch)
            .wrapping_add(SHA256_K[index])
            .wrapping_add(w[index]);
        let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
        let maj = (a & b) ^ (a & c) ^ (b & c);
        let temp2 = s0.wrapping_add(maj);

        h = g;
        g = f;
        f = e;
        e = d.wrapping_add(temp1);
        d = c;
        c = b;
        b = a;
        a = temp1.wrapping_add(temp2);
    }

    state[0] = state[0].wrapping_add(a);
    state[1] = state[1].wrapping_add(b);
    state[2] = state[2].wrapping_add(c);
    state[3] = state[3].wrapping_add(d);
    state[4] = state[4].wrapping_add(e);
    state[5] = state[5].wrapping_add(f);
    state[6] = state[6].wrapping_add(g);
    state[7] = state[7].wrapping_add(h);
}

impl Default for ResumableSha256 {
    fn default() -> Self {
        Self::new()
    }
}

impl ResumableSha256 {
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: SHA256_H0,
            blocks_bytes: 0,
            tail: [0_u8; 64],
            tail_len: 0,
        }
    }

    #[must_use]
    pub fn total_bytes(&self) -> u64 {
        self.blocks_bytes.saturating_add(self.tail_len as u64)
    }

    pub fn update(&mut self, mut bytes: &[u8]) {
        if self.tail_len > 0 {
            let need = 64 - self.tail_len;
            if bytes.len() < need {
                self.tail[self.tail_len..self.tail_len + bytes.len()].copy_from_slice(bytes);
                self.tail_len += bytes.len();
                return;
            }
            self.tail[self.tail_len..64].copy_from_slice(&bytes[..need]);
            compress_block(&mut self.state, &self.tail);
            self.blocks_bytes = self.blocks_bytes.saturating_add(64);
            self.tail_len = 0;
            bytes = &bytes[need..];
        }
        let mut iter = bytes.chunks_exact(64);
        for chunk in &mut iter {
            let block: &[u8; 64] = chunk.try_into().expect("64-byte chunk");
            compress_block(&mut self.state, block);
            self.blocks_bytes = self.blocks_bytes.saturating_add(64);
        }
        let rem = iter.remainder();
        if !rem.is_empty() {
            self.tail[..rem.len()].copy_from_slice(rem);
            self.tail_len = rem.len();
        }
    }

    #[must_use]
    pub fn finalize_hex(&self) -> String {
        encode_hex(&self.finalize_bytes())
    }

    #[must_use]
    pub fn finalize_bytes(&self) -> [u8; 32] {
        let mut state = self.state;
        let total_bits = self.total_bytes().wrapping_mul(8);
        let mut pad_block = [0_u8; 64];
        pad_block[..self.tail_len].copy_from_slice(&self.tail[..self.tail_len]);
        pad_block[self.tail_len] = 0x80;
        if self.tail_len >= 56 {
            compress_block(&mut state, &pad_block);
            pad_block = [0_u8; 64];
        }
        pad_block[56..64].copy_from_slice(&total_bits.to_be_bytes());
        compress_block(&mut state, &pad_block);

        let mut out = [0_u8; 32];
        for (index, word) in state.iter().enumerate() {
            out[index * 4..(index + 1) * 4].copy_from_slice(&word.to_be_bytes());
        }
        out
    }

    fn to_persisted(&self) -> (String, u64, String) {
        let mut state_bytes = [0_u8; 32];
        for (index, word) in self.state.iter().enumerate() {
            state_bytes[index * 4..(index + 1) * 4].copy_from_slice(&word.to_be_bytes());
        }
        (
            encode_hex(&state_bytes),
            self.blocks_bytes,
            encode_hex(&self.tail[..self.tail_len]),
        )
    }

    fn from_persisted(
        midstate_hex: &str,
        blocks_bytes: u64,
        tail_hex: &str,
        expected_offset: u64,
    ) -> Option<Self> {
        if !blocks_bytes.is_multiple_of(64) {
            return None;
        }
        let state_bytes = decode_hex(midstate_hex)?;
        if state_bytes.len() != 32 {
            return None;
        }
        let tail_bytes = decode_hex(tail_hex)?;
        if tail_bytes.len() >= 64 {
            return None;
        }
        if blocks_bytes.checked_add(tail_bytes.len() as u64)? != expected_offset {
            return None;
        }
        let mut state = [0_u32; 8];
        for (index, chunk) in state_bytes.chunks_exact(4).enumerate() {
            state[index] = u32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        }
        let mut tail = [0_u8; 64];
        tail[..tail_bytes.len()].copy_from_slice(&tail_bytes);
        Some(Self {
            state,
            blocks_bytes,
            tail,
            tail_len: tail_bytes.len(),
        })
    }
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

fn decode_hex(raw: &str) -> Option<Vec<u8>> {
    let trimmed = raw.trim();
    if !trimmed.len().is_multiple_of(2) {
        return None;
    }
    let mut out = Vec::with_capacity(trimmed.len() / 2);
    let bytes = trimmed.as_bytes();
    let mut idx = 0;
    while idx < bytes.len() {
        let hi = hex_nibble(bytes[idx])?;
        let lo = hex_nibble(bytes[idx + 1])?;
        out.push((hi << 4) | lo);
        idx += 2;
    }
    Some(out)
}

fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn parse_sha256_hex(raw: &str) -> Option<String> {
    let decoded = decode_hex(raw)?;
    (decoded.len() == 32).then(|| encode_hex(&decoded))
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct UploadMeta {
    schema_version: u32,
    upload_id: String,
    client_item_id: String,
    filename: String,
    original_filename: String,
    #[serde(default)]
    mime_type: Option<String>,
    total_bytes: u64,
    next_offset: u64,
    #[serde(default)]
    sha256: Option<String>,
    sha256_midstate_hex: String,
    sha256_blocks_bytes: u64,
    #[serde(default)]
    sha256_tail_hex: String,
    created_at_ms: i64,
    last_activity_at_ms: i64,
    expires_at_ms: i64,
    #[serde(default)]
    imported_via: String,
    #[serde(default)]
    device_noun: Option<String>,
    #[serde(default)]
    source_hint: Option<String>,
    #[serde(default)]
    data: Value,
    #[serde(default)]
    context: Option<Map<String, Value>>,
}

impl UploadMeta {
    fn is_expired(&self, now: i64) -> bool {
        now >= self.expires_at_ms
    }

    fn phase(&self, now: i64) -> &'static str {
        if is_committing(&self.upload_id) {
            "checking"
        } else if now.saturating_sub(self.last_activity_at_ms) <= ACTIVE_PAUSED_THRESHOLD_MS {
            "sending"
        } else {
            "paused"
        }
    }

    fn status_value(&self, now: i64) -> Value {
        json!({
            "upload_id": self.upload_id,
            "client_item_id": self.client_item_id,
            "filename": self.original_filename,
            "status": "open",
            "phase": self.phase(now),
            "next_offset": self.next_offset,
            "total_bytes": self.total_bytes,
            "chunk_bytes": RESUMABLE_CHUNK_BYTES,
            "imported_via": self.imported_via,
            "device_noun": self.device_noun,
            "source_hint": self.source_hint,
            "created_at": format_rfc3339_ms(self.created_at_ms),
            "last_activity_at": format_rfc3339_ms(self.last_activity_at_ms),
            "expires_at": format_rfc3339_ms(self.expires_at_ms),
        })
    }

    fn create_or_resume_value(&self) -> Value {
        json!({
            "upload_id": self.upload_id,
            "client_item_id": self.client_item_id,
            "status": "open",
            "next_offset": self.next_offset,
            "total_bytes": self.total_bytes,
            "chunk_bytes": RESUMABLE_CHUNK_BYTES,
            "expires_at": format_rfc3339_ms(self.expires_at_ms),
        })
    }
}

fn format_rfc3339_ms(ms: i64) -> String {
    Timestamp::from_millisecond(ms)
        .unwrap_or(Timestamp::UNIX_EPOCH)
        .strftime("%Y-%m-%dT%H:%M:%SZ")
        .to_string()
}

fn is_valid_upload_id(upload_id: &str) -> bool {
    is_uuid_v4(upload_id)
}

fn uploads_root(root: &Path) -> PathBuf {
    root.join("imports").join(UPLOADS_DIR)
}

fn contained_upload_dir(root: &Path, upload_id: &str) -> Option<PathBuf> {
    if !is_valid_upload_id(upload_id) {
        return None;
    }
    let relative = format!("imports/{UPLOADS_DIR}/{upload_id}");
    contained_path(root, &relative).ok()
}

fn upload_lock_key(upload_id: &str) -> String {
    format!("upload:{upload_id}")
}

const CREATE_LOCK_KEY: &str = "upload:__create__";

static COMMITTING_IDS: OnceLock<Mutex<BTreeSet<String>>> = OnceLock::new();

struct CommittingGuard {
    upload_id: String,
}

impl CommittingGuard {
    fn enter(upload_id: &str) -> Self {
        let set = COMMITTING_IDS.get_or_init(|| Mutex::new(BTreeSet::new()));
        let mut guard = match set.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard.insert(upload_id.to_owned());
        Self {
            upload_id: upload_id.to_owned(),
        }
    }
}

impl Drop for CommittingGuard {
    fn drop(&mut self) {
        if let Some(set) = COMMITTING_IDS.get() {
            let mut guard = match set.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            guard.remove(&self.upload_id);
        }
    }
}

fn is_committing(upload_id: &str) -> bool {
    COMMITTING_IDS.get().is_some_and(|set| {
        let guard = match set.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard.contains(upload_id)
    })
}

static PROGRESS_EMITTED_AT: OnceLock<Mutex<HashMap<String, Instant>>> = OnceLock::new();

fn should_emit_progress(upload_id: &str, force: bool) -> bool {
    let map = PROGRESS_EMITTED_AT.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = match map.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    let now = Instant::now();
    if guard.len() > 256 {
        guard.retain(|_, last| now.duration_since(*last) < Duration::from_secs(300));
    }
    if force {
        guard.insert(upload_id.to_owned(), now);
        return true;
    }
    match guard.get(upload_id) {
        Some(last) if now.duration_since(*last) < PROGRESS_EVENT_THROTTLE => false,
        _ => {
            guard.insert(upload_id.to_owned(), now);
            true
        }
    }
}

fn clear_progress_throttle(upload_id: &str) {
    if let Some(map) = PROGRESS_EMITTED_AT.get() {
        let mut guard = match map.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard.remove(upload_id);
    }
}

type RecentCommitKey = (PathBuf, String);
type RecentCommitEntry = (Instant, String);
type RecentCommitMap = HashMap<RecentCommitKey, RecentCommitEntry>;

static RECENT_COMMITS: OnceLock<Mutex<RecentCommitMap>> = OnceLock::new();

fn record_recent_commit(root: &Path, upload_id: &str, client_item_id: &str) {
    let map = RECENT_COMMITS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = match map.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    let now = Instant::now();
    guard.retain(|_, (recorded, _)| now.duration_since(*recorded) < RECENT_COMMIT_RETENTION);
    guard.insert(
        (root.to_path_buf(), upload_id.to_owned()),
        (now, client_item_id.to_owned()),
    );
}

fn lookup_recent_commit(root: &Path, upload_id: &str) -> Option<String> {
    let map = RECENT_COMMITS.get()?;
    let mut guard = match map.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    let now = Instant::now();
    guard.retain(|_, (recorded, _)| now.duration_since(*recorded) < RECENT_COMMIT_RETENTION);
    guard
        .get(&(root.to_path_buf(), upload_id.to_owned()))
        .map(|(_, client_item_id)| client_item_id.clone())
}

#[cfg(test)]
static TEST_FREE_SPACE_OVERRIDES: OnceLock<Mutex<HashMap<PathBuf, u64>>> = OnceLock::new();

fn query_available_disk_bytes(path: &Path) -> std::io::Result<u64> {
    #[cfg(test)]
    if let Some(map_mutex) = TEST_FREE_SPACE_OVERRIDES.get() {
        let guard = match map_mutex.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        for (prefix, simulated) in guard.iter() {
            if path.starts_with(prefix) {
                return Ok(*simulated);
            }
        }
    }
    available_disk_bytes(path)
}

fn ensure_uploads_root(root: &Path) -> Result<PathBuf, String> {
    let imports = root.join("imports");
    create_directory_with_mode(&imports, 0o700).map_err(|err| err.to_string())?;
    let uploads = imports.join(UPLOADS_DIR);
    create_directory_with_mode(&uploads, 0o700).map_err(|err| err.to_string())?;
    Ok(uploads)
}

fn read_upload_meta(dir: &Path) -> Option<UploadMeta> {
    let bytes = fs::read(dir.join(META_JSON_FILE)).ok()?;
    let meta: UploadMeta = serde_json::from_slice(&bytes).ok()?;
    if meta.schema_version != 1
        || !is_valid_upload_id(&meta.upload_id)
        || meta.total_bytes == 0
        || meta.total_bytes > MAX_UPLOAD_FILE_BYTES
        || meta.next_offset > meta.total_bytes
    {
        return None;
    }
    Some(meta)
}

fn remove_contained_tree(
    root: &Path,
    dir: &Path,
) -> Result<(), solstone_core_journal_io::PathError> {
    #[expect(
        clippy::disallowed_methods,
        reason = "system internal: uncommitted partial upload staging under imports/.uploads/ has never entered the journal"
    )]
    {
        solstone_core_journal_io::remove_contained_tree(root, dir)
    }
}

fn write_upload_meta(dir: &Path, meta: &UploadMeta) -> Result<(), String> {
    let mut bytes = serde_json::to_vec_pretty(meta).map_err(|err| err.to_string())?;
    bytes.push(b'\n');
    atomic_replace(
        dir.join(META_JSON_FILE),
        &bytes,
        AtomicWriteOptions { mode: Some(0o600) },
    )
    .map_err(|err| err.to_string())
}

fn rebuild_midstate_from_part(part_path: &Path, target_offset: u64) -> Option<ResumableSha256> {
    use std::io::Read;
    let mut file = fs::File::open(part_path).ok()?;
    let mut hasher = ResumableSha256::new();
    let mut remaining = target_offset;
    let mut buffer = vec![0_u8; 64 * 1024];
    while remaining > 0 {
        let to_read = (remaining as usize).min(buffer.len());
        let count = file.read(&mut buffer[..to_read]).ok()?;
        if count == 0 {
            return None;
        }
        hasher.update(&buffer[..count]);
        remaining -= count as u64;
    }
    Some(hasher)
}

/// Loads `meta.json`, ensures `payload.part` is at least `meta.next_offset` (truncating any
/// uncommitted crash tail above `meta.next_offset`), and reconstructs `ResumableSha256`.
fn load_verified_open_upload(
    root: &Path,
    dir: &Path,
    now: i64,
) -> Option<(UploadMeta, PathBuf, ResumableSha256)> {
    let meta = read_upload_meta(dir)?;
    if meta.is_expired(now) {
        let _ = remove_contained_tree(root, dir);
        clear_progress_throttle(&meta.upload_id);
        return None;
    }
    let part_path = dir.join(PAYLOAD_PART_FILE);
    let part_meta = fs::symlink_metadata(&part_path).ok()?;
    if !part_meta.file_type().is_file() || part_meta.len() < meta.next_offset {
        let _ = remove_contained_tree(root, dir);
        clear_progress_throttle(&meta.upload_id);
        return None;
    }
    if part_meta.len() > meta.next_offset {
        let file = OpenOptions::new().write(true).open(&part_path).ok()?;
        file.set_len(meta.next_offset).ok()?;
        let _ = file.sync_data();
    }
    let hasher = ResumableSha256::from_persisted(
        &meta.sha256_midstate_hex,
        meta.sha256_blocks_bytes,
        &meta.sha256_tail_hex,
        meta.next_offset,
    )
    .or_else(|| rebuild_midstate_from_part(&part_path, meta.next_offset))?;
    Some((meta, part_path, hasher))
}

/// Run once per journal root per process when the import router initializes.
pub(crate) fn sweep_startup_once(root: &Path) {
    static SWEPT_ROOTS: OnceLock<Mutex<BTreeSet<PathBuf>>> = OnceLock::new();
    let set = SWEPT_ROOTS.get_or_init(|| Mutex::new(BTreeSet::new()));
    let canonical = fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let first_time = {
        let mut guard = match set.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard.insert(canonical)
    };
    if first_time {
        sweep_uploads_and_save_temps(root);
    }
}

/// Sweeps expired or malformed partial `.uploads/` directories AND orphaned `.save-tmp/`
/// temporary files left behind by a killed process, without touching any live in-flight `/save`.
pub fn sweep_uploads_and_save_temps(root: &Path) {
    sweep_expired_uploads_with_grace(root, Duration::ZERO);
    sweep_orphaned_save_temps(root);
}

/// Sweeps expired or stale unreadable partial upload directories under `imports/.uploads/`.
pub fn sweep_expired_uploads(root: &Path) {
    sweep_expired_uploads_with_grace(root, Duration::from_secs(60));
}

fn dir_older_than_grace(path: &Path, grace: Duration) -> bool {
    if grace.is_zero() {
        return true;
    }
    fs::symlink_metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|modified| modified.elapsed().ok())
        .is_some_and(|age| age >= grace)
}

fn sweep_expired_uploads_with_grace(root: &Path, incomplete_grace: Duration) {
    let uploads = uploads_root(root);
    let Ok(entries) = fs::read_dir(&uploads) else {
        return;
    };
    let now = now_ms();
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if !file_type.is_dir() {
            if dir_older_than_grace(&path, incomplete_grace) {
                let _ = fs::remove_file(&path);
            }
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if !is_valid_upload_id(&name) {
            if dir_older_than_grace(&path, incomplete_grace) {
                let _ = remove_contained_tree(root, &path);
            }
            continue;
        }
        match read_upload_meta(&path) {
            Some(meta) if meta.is_expired(now) => {
                let _ = remove_contained_tree(root, &path);
                clear_progress_throttle(&name);
            }
            Some(meta) => {
                let part = path.join(PAYLOAD_PART_FILE);
                let valid_part = fs::symlink_metadata(&part)
                    .is_ok_and(|m| m.file_type().is_file() && m.len() >= meta.next_offset);
                if !valid_part && dir_older_than_grace(&path, incomplete_grace) {
                    let _ = remove_contained_tree(root, &path);
                    clear_progress_throttle(&name);
                }
            }
            None => {
                if dir_older_than_grace(&path, incomplete_grace) {
                    let _ = remove_contained_tree(root, &path);
                    clear_progress_throttle(&name);
                }
            }
        }
    }
}

pub(crate) fn sweep_orphaned_save_temps(root: &Path) {
    let save_tmp = root.join("imports").join(crate::lifecycle::SAVE_TMP_DIR);
    let Ok(entries) = fs::read_dir(&save_tmp) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = fs::symlink_metadata(&path) else {
            continue;
        };
        if !meta.file_type().is_file() {
            continue;
        }
        if save_stream::is_active_save_path(&path) {
            continue;
        }
        let old_enough = meta
            .modified()
            .ok()
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|age| age >= SAVE_TMP_ORPHAN_MIN_AGE);
        if !old_enough {
            continue;
        }
        let _ = fs::remove_file(&path);
    }
}

/// Returns active (open, non-expired) uploads ordered by `created_at_ms` ascending.
/// Read-only: skips expired or incomplete directories without mutating disk.
pub(crate) fn list_active_upload_values(root: &Path) -> Vec<Value> {
    let uploads = uploads_root(root);
    let Ok(entries) = fs::read_dir(&uploads) else {
        return Vec::new();
    };
    let now = now_ms();
    let mut metas = Vec::new();
    for entry in entries.flatten() {
        if !entry.file_type().is_ok_and(|ft| ft.is_dir()) {
            continue;
        }
        let dir = entry.path();
        if let Some(meta) = read_upload_meta(&dir) {
            if meta.is_expired(now) {
                continue;
            }
            let part = dir.join(PAYLOAD_PART_FILE);
            if fs::symlink_metadata(&part)
                .is_ok_and(|m| m.file_type().is_file() && m.len() >= meta.next_offset)
            {
                metas.push(meta);
            }
        }
    }
    metas.sort_by(|left, right| {
        left.created_at_ms
            .cmp(&right.created_at_ms)
            .then_with(|| left.upload_id.cmp(&right.upload_id))
    });
    metas
        .into_iter()
        .map(|meta| meta.status_value(now))
        .collect()
}

fn scan_open_uploads_for_create(
    root: &Path,
    client_item_id: &str,
    now: i64,
) -> (Option<String>, u64, u64) {
    let uploads = uploads_root(root);
    let Ok(entries) = fs::read_dir(&uploads) else {
        return (None, 0, 0);
    };
    let mut matched_upload_id = None;
    let mut reserved_total_bytes = 0_u64;
    let mut remaining_unwritten_bytes = 0_u64;
    for entry in entries.flatten() {
        if !entry.file_type().is_ok_and(|ft| ft.is_dir()) {
            continue;
        }
        let dir = entry.path();
        let Some(meta) = read_upload_meta(&dir) else {
            continue;
        };
        if meta.is_expired(now) {
            continue;
        }
        let part = dir.join(PAYLOAD_PART_FILE);
        if !fs::symlink_metadata(&part)
            .is_ok_and(|m| m.file_type().is_file() && m.len() >= meta.next_offset)
        {
            continue;
        }
        reserved_total_bytes = reserved_total_bytes.saturating_add(meta.total_bytes);
        remaining_unwritten_bytes = remaining_unwritten_bytes
            .saturating_add(meta.total_bytes.saturating_sub(meta.next_offset));
        if meta.client_item_id == client_item_id && matched_upload_id.is_none() {
            matched_upload_id = Some(meta.upload_id);
        }
    }
    (
        matched_upload_id,
        reserved_total_bytes,
        remaining_unwritten_bytes,
    )
}

fn protocol_error(
    status: StatusCode,
    reason_code: &'static str,
    error_text: &'static str,
    detail: impl Into<String>,
    extra: Option<Map<String, Value>>,
) -> Response {
    let mut body = Map::from_iter([
        ("error".to_owned(), json!(error_text)),
        ("reason_code".to_owned(), json!(reason_code)),
        ("detail".to_owned(), json!(detail.into())),
    ]);
    if let Some(extra_fields) = extra {
        for (key, val) in extra_fields {
            body.insert(key, val);
        }
    }
    json_response(status, Value::Object(body))
}

fn upload_not_found() -> Response {
    let code = UploadReasonCode::UploadNotFound;
    protocol_error(
        code.status(),
        code.as_wire(),
        "that upload is no longer open.",
        "upload session does not exist or has expired",
        None,
    )
}

fn insufficient_space_response(
    required_bytes: u64,
    available_bytes: u64,
    detail: &str,
    next_offset: Option<u64>,
) -> Response {
    let mut extra = Map::from_iter([
        ("required_bytes".to_owned(), json!(required_bytes)),
        ("available_bytes".to_owned(), json!(available_bytes)),
    ]);
    if let Some(offset) = next_offset {
        extra.insert("next_offset".to_owned(), json!(offset));
    }
    let code = UploadReasonCode::InsufficientJournalSpace;
    protocol_error(
        code.status(),
        code.as_wire(),
        "your journal doesn't have enough free space for that file.",
        detail,
        Some(extra),
    )
}

fn missing_field(detail: impl Into<String>) -> Response {
    let code = ImportReasonCode::MissingRequiredField;
    error(
        code.status(),
        "journal.window.import.response.error.a.required.field.is.missing",
        code.as_wire(),
        detail.into(),
        None,
    )
}

fn invalid_state(detail: impl Into<String>, kind: Option<ImportErrorKind>) -> Response {
    let code = ImportReasonCode::InvalidOperationForState;
    error(
        code.status(),
        "journal.window.import.response.error.that.action.isn.t.available.in",
        code.as_wire(),
        detail.into(),
        kind,
    )
}

fn clean_trimmed(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
}

/// `POST /app/import/api/uploads`
pub(crate) async fn create(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    let is_json = headers
        .get(header::CONTENT_TYPE)
        .and_then(|val| val.to_str().ok())
        .is_some_and(|ct| {
            ct.split(';')
                .next()
                .unwrap_or(ct)
                .trim()
                .eq_ignore_ascii_case("application/json")
        });
    if !is_json {
        let code = UploadReasonCode::UnsupportedMediaType;
        return protocol_error(
            code.status(),
            code.as_wire(),
            "upload creation body must be application/json.",
            "Content-Type must be application/json",
            None,
        );
    }
    let raw_bytes = match to_bytes(body, MAX_JSON_CONTROL_BYTES).await {
        Ok(bytes) => bytes,
        Err(_) => {
            let code = ImportReasonCode::MultipartPartTooLarge;
            return error(
                code.status(),
                "journal.window.import.response.error.that.file.is.too.large.to",
                code.as_wire(),
                "create request body exceeds 64 KiB limit".to_owned(),
                None,
            );
        }
    };
    let Ok(payload) = serde_json::from_slice::<Value>(&raw_bytes) else {
        return invalid_state("request body must be valid JSON", None);
    };
    let Some(obj) = payload.as_object() else {
        return invalid_state("request body must be a JSON object", None);
    };
    let Some(client_item_id) = clean_trimmed(obj.get("client_item_id")) else {
        return missing_field("Missing client_item_id");
    };
    if client_item_id.len() > MAX_CLIENT_ITEM_ID_BYTES {
        return invalid_state("client_item_id exceeds maximum length", None);
    }
    let Some(original_filename) = clean_trimmed(obj.get("filename")) else {
        return missing_field("Missing filename");
    };
    if original_filename.len() > multipart::MAX_FILENAME_BYTES {
        return invalid_state("filename is too long", None);
    }
    let Some(filename) = safe_filename(&original_filename) else {
        return invalid_state("invalid filename", None);
    };
    let Some(total_bytes) = obj.get("total_bytes").and_then(Value::as_u64) else {
        return missing_field("Missing total_bytes");
    };
    if total_bytes == 0 {
        return invalid_state("total_bytes must be greater than zero", None);
    }
    if total_bytes > MAX_UPLOAD_FILE_BYTES {
        let code = ImportReasonCode::MultipartPartTooLarge;
        return error(
            code.status(),
            "journal.window.import.response.error.that.file.is.too.large.to",
            code.as_wire(),
            "upload exceeds 4 GiB limit".to_owned(),
            None,
        );
    }
    let sha256 = match obj.get("sha256") {
        None | Some(Value::Null) => None,
        Some(Value::String(raw)) if raw.trim().is_empty() => None,
        Some(Value::String(raw)) => match parse_sha256_hex(raw) {
            Some(normalized) => Some(normalized),
            None => return invalid_state("sha256 must be 64 hexadecimal characters", None),
        },
        Some(_) => return invalid_state("sha256 must be a string", None),
    };
    let mime_type = clean_trimmed(obj.get("mime_type"));
    let context = match parse_and_validate_context(obj.get("context")) {
        Ok(ctx) => ctx,
        Err(detail) => return invalid_state(detail, None),
    };

    let client_bag = match obj.get("client") {
        None | Some(Value::Null) => json!({}),
        Some(val) if val.is_object() => {
            let encoded_len = serde_json::to_vec(val)
                .map(|bytes| bytes.len())
                .unwrap_or(usize::MAX);
            if encoded_len > MAX_CLIENT_BAG_BYTES {
                return invalid_state("client metadata object exceeds 16 KiB limit", None);
            }
            val.clone()
        }
        Some(_) => json!({}),
    };
    let mut data = json!({
        "imported_via": clean_trimmed(obj.get("imported_via")).unwrap_or_else(|| "web_dashboard".to_owned()),
        "source_hint": clean_trimmed(obj.get("source_hint")),
        "setting": clean_trimmed(obj.get("setting")),
        "caption": clean_trimmed(obj.get("caption")),
        "captured_at": clean_trimmed(obj.get("captured_at")),
        "observer_handle": clean_trimmed(obj.get("observer_handle")),
        "area": clean_trimmed(obj.get("area")),
        "device_noun": allowlisted_device_noun(obj.get("device_noun").and_then(Value::as_str)),
        "client": client_bag,
    });
    if let Some(ref ctx) = context
        && let Some(setting) = ctx.get("setting")
    {
        data["setting"] = setting.clone();
    }

    if let Some(sentence) = evernote_upload_refusal(&data, Some(total_bytes)) {
        return error(
            StatusCode::BAD_REQUEST,
            "journal.window.import.response.error.this.export.is.over.gb.in",
            "evernote_export_too_big",
            sentence.to_owned(),
            None,
        );
    }
    if is_heic_upload(&original_filename, mime_type.as_deref()) {
        let code = ImportReasonCode::UnsupportedImageFormat;
        return error(
            code.status(),
            HEIC_REFUSAL_MESSAGE,
            code.as_wire(),
            solstone_core_convey_http::window_catalog::text(
                solstone_core_convey_http::window_catalog::ENGLISH,
                HEIC_REFUSAL_MESSAGE,
            ),
            None,
        );
    }

    let create_guard = start_lock::acquire(&state.root, CREATE_LOCK_KEY).await;
    let root = state.root.clone();
    let client_item_for_resume = client_item_id.clone();
    let orig_name_for_resume = original_filename.clone();
    let sha_for_resume = sha256.clone();
    let outcome = match tokio::task::spawn_blocking(move || {
        let _guard = create_guard;
        create_sync(
            &root,
            client_item_id,
            filename,
            original_filename,
            mime_type,
            total_bytes,
            sha256,
            data,
            context,
        )
    })
    .await
    {
        Ok(outcome) => outcome,
        Err(_) => {
            return protocol_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "import_metadata_failed",
                "upload worker stopped.",
                "create worker failed",
                None,
            );
        }
    };

    match outcome {
        CreateSyncOutcome::Done(resp) => resp,
        CreateSyncOutcome::ResumeOpen { upload_id } => {
            // Acquire the per-upload lock (after releasing `upload:__create__`) so refreshing
            // `expires_at` can never overwrite a concurrent `put_chunk` update.
            let upload_guard = start_lock::acquire(&state.root, &upload_lock_key(&upload_id)).await;
            let root = state.root.clone();
            match tokio::task::spawn_blocking(move || {
                let _guard = upload_guard;
                resume_open_sync(
                    &root,
                    &upload_id,
                    &client_item_for_resume,
                    &orig_name_for_resume,
                    total_bytes,
                    sha_for_resume,
                )
            })
            .await
            {
                Ok(resp) => resp,
                Err(_) => protocol_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "import_metadata_failed",
                    "upload worker stopped.",
                    "resume worker failed",
                    None,
                ),
            }
        }
    }
}

enum CreateSyncOutcome {
    Done(Response),
    ResumeOpen { upload_id: String },
}

fn resume_open_sync(
    root: &Path,
    upload_id: &str,
    client_item_id: &str,
    original_filename: &str,
    total_bytes: u64,
    sha256: Option<String>,
) -> Response {
    let _target_use = match solstone_core_journal_io::whole_journal_restore::begin_target_use(root)
    {
        Ok(guard) => guard,
        Err(_) => {
            let code = UploadReasonCode::JournalBusy;
            return protocol_error(
                code.status(),
                code.as_wire(),
                "your journal is busy right now.",
                "journal restore is in progress",
                None,
            );
        }
    };
    let Some(dir) = contained_upload_dir(root, upload_id) else {
        return upload_not_found();
    };
    let now = now_ms();
    let Some((mut meta, _, _)) = load_verified_open_upload(root, &dir, now) else {
        if let Some(existing) = staged_by_client_item(root, client_item_id) {
            return json_response(
                StatusCode::OK,
                json!({
                    "status": "already_staged",
                    "replay": true,
                    "save_response": replay_summary(&existing),
                }),
            );
        }
        return upload_not_found();
    };
    let sha_matches = match (&meta.sha256, &sha256) {
        (Some(left), Some(right)) => left == right,
        _ => true,
    };
    if meta.total_bytes != total_bytes
        || meta.original_filename != original_filename
        || !sha_matches
    {
        return invalid_state(
            "client_item_id already open for different upload parameters",
            Some(ImportErrorKind::ClientItemConflict),
        );
    }
    if meta.sha256.is_none() && sha256.is_some() {
        meta.sha256 = sha256;
    }
    meta.last_activity_at_ms = now;
    meta.expires_at_ms = now.saturating_add(PARTIAL_UPLOAD_TTL_MS);
    let _ = write_upload_meta(&dir, &meta);
    json_response(StatusCode::OK, meta.create_or_resume_value())
}

#[allow(clippy::too_many_arguments)]
fn create_sync(
    root: &Path,
    client_item_id: String,
    filename: String,
    original_filename: String,
    mime_type: Option<String>,
    total_bytes: u64,
    sha256: Option<String>,
    data: Value,
    context: Option<Map<String, Value>>,
) -> CreateSyncOutcome {
    let _target_use = match solstone_core_journal_io::whole_journal_restore::begin_target_use(root)
    {
        Ok(guard) => guard,
        Err(_) => {
            return CreateSyncOutcome::Done(protocol_error(
                StatusCode::CONFLICT,
                "journal_busy",
                "your journal is busy right now.",
                "journal restore is in progress",
                None,
            ));
        }
    };

    // 1. Pre-upload idempotency against already-staged imports.
    if let Some(existing) = staged_by_client_item(root, &client_item_id) {
        let matches_existing = match &sha256 {
            Some(expected_hash) => {
                existing.get("source_hash").and_then(Value::as_str) == Some(expected_hash.as_str())
            }
            None => {
                existing.get("original_filename").and_then(Value::as_str)
                    == Some(original_filename.as_str())
                    && existing.get("file_size").and_then(Value::as_u64) == Some(total_bytes)
            }
        };
        if matches_existing {
            return CreateSyncOutcome::Done(json_response(
                StatusCode::OK,
                json!({
                    "status": "already_staged",
                    "replay": true,
                    "save_response": replay_summary(&existing),
                }),
            ));
        }
        return CreateSyncOutcome::Done(invalid_state(
            "client_item_id already staged for different content; use a new client_item_id or re-fetch the existing item",
            Some(ImportErrorKind::ClientItemConflict),
        ));
    }

    // 2. Pre-upload deduplication by whole-file SHA-256 when provided on create.
    let source_hint = clean_trimmed(data.get("source_hint"));
    if let Some(ref hash_hex) = sha256 {
        let source_hash = SourceHash::new(hash_hex.clone());
        let repeat_allowed_hint =
            matches!(source_hint.as_deref(), Some(STRAVA_SOURCE | APPLE_SOURCE));
        if !repeat_allowed_hint && let Some(found) = imported_manifest(root, &source_hash) {
            let manifest_source = found
                .manifest
                .get("source_type")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if !matches!(manifest_source, STRAVA_SOURCE | APPLE_SOURCE | "body") {
                let import_id = found
                    .path
                    .parent()
                    .and_then(Path::file_name)
                    .and_then(|name| name.to_str())
                    .filter(|id| validate_timestamp(id).is_ok())
                    .map(ToOwned::to_owned);
                return CreateSyncOutcome::Done(json_response(
                    StatusCode::OK,
                    json!({
                        "status": "already_imported",
                        "import_id": import_id,
                        "reason_code": ImportReasonCode::InvalidOperationForState.as_wire(),
                        "kind": ImportErrorKind::AlreadyImported.as_wire(),
                    }),
                ));
            }
        }
        if let Some(existing) = staged_by_source_hash(root, &source_hash) {
            return CreateSyncOutcome::Done(json_response(
                StatusCode::OK,
                json!({
                    "status": "already_staged",
                    "replay": false,
                    "save_response": summary(&existing),
                }),
            ));
        }
    }

    let uploads_dir = match ensure_uploads_root(root) {
        Ok(dir) => dir,
        Err(detail) => {
            return CreateSyncOutcome::Done(protocol_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "import_metadata_failed",
                "could not prepare uploads directory.",
                detail,
                None,
            ));
        }
    };

    // 3. Resume existing open upload for the same `client_item_id` under its own upload lock.
    let now = now_ms();
    let (existing_open_id, reserved_bytes, remaining_unwritten_bytes) =
        scan_open_uploads_for_create(root, &client_item_id, now);
    if let Some(upload_id) = existing_open_id {
        return CreateSyncOutcome::ResumeOpen { upload_id };
    }

    // 4. Upfront aggregate cap + disk free-space check (including other open uploads' remaining bytes).
    if reserved_bytes.saturating_add(total_bytes) > MAX_ACTIVE_UPLOADS_BYTES {
        let available_cap = MAX_ACTIVE_UPLOADS_BYTES.saturating_sub(reserved_bytes);
        return CreateSyncOutcome::Done(insufficient_space_response(
            total_bytes,
            available_cap,
            "concurrent partial uploads exceed 8 GiB staging cap",
            None,
        ));
    }
    let required_free = remaining_unwritten_bytes
        .saturating_add(total_bytes)
        .saturating_add(MIN_FREE_DISK_BYTES);
    match query_available_disk_bytes(&uploads_dir) {
        Ok(available) if available < required_free => {
            return CreateSyncOutcome::Done(insufficient_space_response(
                required_free,
                available,
                "journal volume does not have enough free space for this upload plus 256 MiB headroom",
                None,
            ));
        }
        Ok(_) => {}
        Err(err) => {
            return CreateSyncOutcome::Done(protocol_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "import_metadata_failed",
                "could not measure journal free space.",
                err.to_string(),
                None,
            ));
        }
    }

    // 5. Mint `upload_id` and initialize `payload.part` + `meta.json`.
    let upload_id = match mint_uuid_v4() {
        Ok(id) => id,
        Err(err) => {
            return CreateSyncOutcome::Done(protocol_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "import_metadata_failed",
                "could not mint upload identifier.",
                err.to_string(),
                None,
            ));
        }
    };
    let Some(dir) = contained_upload_dir(root, &upload_id) else {
        return CreateSyncOutcome::Done(protocol_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "import_metadata_failed",
            "invalid upload path.",
            "could not resolve upload directory",
            None,
        ));
    };
    if let Err(err) = create_directory_with_mode(&dir, 0o700) {
        return CreateSyncOutcome::Done(protocol_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "import_metadata_failed",
            "could not create upload directory.",
            err.to_string(),
            None,
        ));
    }
    let part_path = dir.join(PAYLOAD_PART_FILE);
    let create_part = {
        let mut opts = OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        opts.open(&part_path).and_then(|file| file.sync_all())
    };
    if let Err(err) = create_part {
        let _ = remove_contained_tree(root, &dir);
        return CreateSyncOutcome::Done(protocol_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "import_metadata_failed",
            "could not create partial upload file.",
            err.to_string(),
            None,
        ));
    }

    let (midstate_hex, blocks_bytes, tail_hex) = ResumableSha256::new().to_persisted();
    let imported_via = data
        .get("imported_via")
        .and_then(Value::as_str)
        .unwrap_or("web_dashboard")
        .to_owned();
    let device_noun = allowlisted_device_noun(data.get("device_noun").and_then(Value::as_str));
    let meta = UploadMeta {
        schema_version: 1,
        upload_id: upload_id.clone(),
        client_item_id: client_item_id.clone(),
        filename,
        original_filename: original_filename.clone(),
        mime_type,
        total_bytes,
        next_offset: 0,
        sha256,
        sha256_midstate_hex: midstate_hex,
        sha256_blocks_bytes: blocks_bytes,
        sha256_tail_hex: tail_hex,
        created_at_ms: now,
        last_activity_at_ms: now,
        expires_at_ms: now.saturating_add(PARTIAL_UPLOAD_TTL_MS),
        imported_via: imported_via.clone(),
        device_noun: device_noun.clone(),
        source_hint: source_hint.clone(),
        data,
        context,
    };
    if let Err(detail) = write_upload_meta(&dir, &meta) {
        let _ = remove_contained_tree(root, &dir);
        return CreateSyncOutcome::Done(protocol_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "import_metadata_failed",
            "could not persist upload metadata.",
            detail,
            None,
        ));
    }
    emit_importer_event(
        root,
        "upload_progress",
        Map::from_iter([
            ("upload_id".to_owned(), json!(upload_id)),
            ("client_item_id".to_owned(), json!(client_item_id)),
            ("filename".to_owned(), json!(original_filename)),
            ("bytes_received".to_owned(), json!(0_u64)),
            ("total_bytes".to_owned(), json!(total_bytes)),
            ("phase".to_owned(), json!("sending")),
            ("imported_via".to_owned(), json!(imported_via)),
            (
                "device_noun".to_owned(),
                device_noun.map_or(Value::Null, Value::String),
            ),
            (
                "source_hint".to_owned(),
                source_hint.map_or(Value::Null, Value::String),
            ),
        ]),
    );
    let _ = should_emit_progress(&meta.upload_id, true);
    CreateSyncOutcome::Done(json_response(
        StatusCode::CREATED,
        meta.create_or_resume_value(),
    ))
}

/// `PUT /app/import/api/uploads/{upload_id}?offset=<u64>`
///
/// Enforces exact `Content-Length` framing without `Transfer-Encoding: chunked` or
/// `Expect: 100-continue`, and drains bounded bodies (`<= 8 MiB`) to EOF before
/// returning any `4xx`/`5xx` error so `spl-rust::journal_bridge` never triggers `EarlyFinal`.
pub(crate) async fn put_chunk(
    State(state): State<AppState>,
    AxumPath(upload_id): AxumPath<String>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    if headers.contains_key(header::EXPECT) {
        return protocol_error(
            StatusCode::EXPECTATION_FAILED,
            "invalid_chunk_framing",
            "chunk upload does not support Expect: 100-continue.",
            "send fixed-size chunks without Expect: 100-continue",
            None,
        );
    }
    if headers.contains_key(header::TRANSFER_ENCODING) {
        return protocol_error(
            StatusCode::BAD_REQUEST,
            "content_length_required",
            "chunk upload requires an exact Content-Length.",
            "Transfer-Encoding is not permitted on chunk uploads",
            None,
        );
    }
    let Some(content_length) = headers
        .get(header::CONTENT_LENGTH)
        .and_then(|val| val.to_str().ok())
        .and_then(|raw| raw.trim().parse::<u64>().ok())
    else {
        return protocol_error(
            StatusCode::BAD_REQUEST,
            "content_length_required",
            "chunk upload requires an exact Content-Length.",
            "missing or invalid Content-Length header",
            None,
        );
    };
    if content_length > BRIDGE_DRAIN_CEILING_BYTES as u64 {
        return protocol_error(
            StatusCode::PAYLOAD_TOO_LARGE,
            "chunk_too_large",
            "that upload chunk exceeds the maximum chunk size.",
            format!("chunk Content-Length {content_length} exceeds bridge limit"),
            Some(Map::from_iter([(
                "max_chunk_bytes".to_owned(),
                json!(RESUMABLE_CHUNK_BYTES),
            )])),
        );
    }

    // Drain the bounded request body completely BEFORE returning any 4xx/5xx error!
    let chunk_bytes = match to_bytes(body, BRIDGE_DRAIN_CEILING_BYTES).await {
        Ok(bytes) => bytes,
        Err(_) => {
            return protocol_error(
                StatusCode::BAD_REQUEST,
                "incomplete_chunk_body",
                "chunk body could not be read completely.",
                "connection closed or exceeded drain limit while reading chunk",
                None,
            );
        }
    };
    if chunk_bytes.len() as u64 != content_length {
        return protocol_error(
            StatusCode::BAD_REQUEST,
            "incomplete_chunk_body",
            "chunk body length did not match Content-Length.",
            format!(
                "expected {content_length} bytes, received {}",
                chunk_bytes.len()
            ),
            None,
        );
    }
    if chunk_bytes.is_empty() {
        return protocol_error(
            StatusCode::BAD_REQUEST,
            "invalid_chunk_length",
            "chunk body must not be empty.",
            "chunk length must be at least 1 byte",
            None,
        );
    }
    if chunk_bytes.len() as u64 > RESUMABLE_CHUNK_BYTES {
        return protocol_error(
            StatusCode::PAYLOAD_TOO_LARGE,
            "chunk_too_large",
            "that upload chunk exceeds 4 MiB.",
            format!(
                "chunk length {} exceeds chunk_bytes {}",
                chunk_bytes.len(),
                RESUMABLE_CHUNK_BYTES
            ),
            Some(Map::from_iter([(
                "max_chunk_bytes".to_owned(),
                json!(RESUMABLE_CHUNK_BYTES),
            )])),
        );
    }

    let Some(offset) = query
        .get("offset")
        .and_then(|raw| raw.trim().parse::<u64>().ok())
    else {
        return protocol_error(
            StatusCode::BAD_REQUEST,
            "invalid_offset",
            "chunk upload requires a numeric offset query parameter.",
            "missing or invalid ?offset=<u64>",
            None,
        );
    };

    let expected_chunk_sha = match headers.get("x-chunk-sha256") {
        None => None,
        Some(raw) => match raw.to_str().ok().and_then(parse_sha256_hex) {
            Some(hex) => Some(hex),
            None => {
                return protocol_error(
                    StatusCode::BAD_REQUEST,
                    "invalid_chunk_checksum",
                    "X-Chunk-Sha256 must be 64 hexadecimal characters.",
                    "malformed X-Chunk-Sha256 header",
                    None,
                );
            }
        },
    };

    if !is_valid_upload_id(&upload_id) {
        return upload_not_found();
    }

    let lock_guard = start_lock::acquire(&state.root, &upload_lock_key(&upload_id)).await;
    let root = state.root.clone();
    match tokio::task::spawn_blocking(move || {
        let _guard = lock_guard;
        put_chunk_sync(
            &root,
            &upload_id,
            offset,
            expected_chunk_sha.as_deref(),
            &chunk_bytes,
        )
    })
    .await
    {
        Ok(resp) => resp,
        Err(_) => protocol_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "import_metadata_failed",
            "chunk worker stopped.",
            "put_chunk worker failed",
            None,
        ),
    }
}

fn put_chunk_sync(
    root: &Path,
    upload_id: &str,
    offset: u64,
    expected_chunk_sha: Option<&str>,
    chunk_bytes: &[u8],
) -> Response {
    let _target_use = match solstone_core_journal_io::whole_journal_restore::begin_target_use(root)
    {
        Ok(guard) => guard,
        Err(_) => {
            return protocol_error(
                StatusCode::CONFLICT,
                "journal_busy",
                "your journal is busy right now.",
                "journal restore is in progress",
                None,
            );
        }
    };
    let Some(dir) = contained_upload_dir(root, upload_id) else {
        return upload_not_found();
    };
    let now = now_ms();
    let Some((mut meta, part_path, mut hasher)) = load_verified_open_upload(root, &dir, now) else {
        return upload_not_found();
    };

    if let Some(expected_sha) = expected_chunk_sha {
        let actual_sha = format!("{:x}", Sha256::digest(chunk_bytes));
        if actual_sha != expected_sha {
            return protocol_error(
                StatusCode::UNPROCESSABLE_ENTITY,
                "chunk_checksum_mismatch",
                "chunk integrity check failed.",
                format!("expected {expected_sha}, got {actual_sha}"),
                Some(Map::from_iter([
                    ("next_offset".to_owned(), json!(meta.next_offset)),
                    ("total_bytes".to_owned(), json!(meta.total_bytes)),
                ])),
            );
        }
    }

    if offset != meta.next_offset {
        return protocol_error(
            StatusCode::CONFLICT,
            "offset_mismatch",
            "upload offset does not match journal position.",
            format!(
                "client sent offset {offset}, journal expects {}",
                meta.next_offset
            ),
            Some(Map::from_iter([
                ("next_offset".to_owned(), json!(meta.next_offset)),
                ("total_bytes".to_owned(), json!(meta.total_bytes)),
            ])),
        );
    }

    let chunk_len = chunk_bytes.len() as u64;
    let Some(new_offset) = offset.checked_add(chunk_len) else {
        return protocol_error(
            StatusCode::BAD_REQUEST,
            "chunk_exceeds_total_bytes",
            "chunk offset overflows file size.",
            "offset + chunk_len overflowed u64",
            None,
        );
    };
    if new_offset > meta.total_bytes {
        return protocol_error(
            StatusCode::BAD_REQUEST,
            "chunk_exceeds_total_bytes",
            "chunk extends past declared total_bytes.",
            format!(
                "offset {offset} + {chunk_len} exceeds total_bytes {}",
                meta.total_bytes
            ),
            Some(Map::from_iter([
                ("next_offset".to_owned(), json!(meta.next_offset)),
                ("total_bytes".to_owned(), json!(meta.total_bytes)),
            ])),
        );
    }

    let remaining_bytes = meta.total_bytes.saturating_sub(meta.next_offset);
    let required_free = remaining_bytes.saturating_add(MIN_FREE_DISK_BYTES);
    match query_available_disk_bytes(&dir) {
        Ok(available) if available < required_free => {
            return insufficient_space_response(
                required_free,
                available,
                "free space fell below remaining upload bytes plus 256 MiB headroom",
                Some(meta.next_offset),
            );
        }
        Ok(_) => {}
        Err(err) => {
            return protocol_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "import_metadata_failed",
                "could not measure journal free space.",
                err.to_string(),
                None,
            );
        }
    }

    let write_result = OpenOptions::new()
        .write(true)
        .open(&part_path)
        .and_then(|mut file| {
            file.seek(SeekFrom::Start(offset))?;
            file.write_all(chunk_bytes)?;
            file.set_len(new_offset)?;
            file.sync_data()
        });
    if let Err(err) = write_result {
        return protocol_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "import_metadata_failed",
            "could not write upload chunk.",
            err.to_string(),
            None,
        );
    }

    hasher.update(chunk_bytes);
    let (midstate_hex, blocks_bytes, tail_hex) = hasher.to_persisted();
    let updated_now = now_ms();
    meta.next_offset = new_offset;
    meta.sha256_midstate_hex = midstate_hex;
    meta.sha256_blocks_bytes = blocks_bytes;
    meta.sha256_tail_hex = tail_hex;
    meta.last_activity_at_ms = updated_now;
    meta.expires_at_ms = updated_now.saturating_add(PARTIAL_UPLOAD_TTL_MS);

    if let Err(detail) = write_upload_meta(&dir, &meta) {
        return protocol_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "import_metadata_failed",
            "could not update upload metadata.",
            detail,
            None,
        );
    }

    let complete = new_offset == meta.total_bytes;
    if should_emit_progress(upload_id, offset == 0 || complete) {
        emit_importer_event(
            root,
            "upload_progress",
            Map::from_iter([
                ("upload_id".to_owned(), json!(meta.upload_id)),
                ("client_item_id".to_owned(), json!(meta.client_item_id)),
                ("filename".to_owned(), json!(meta.original_filename)),
                ("bytes_received".to_owned(), json!(meta.next_offset)),
                ("total_bytes".to_owned(), json!(meta.total_bytes)),
                ("phase".to_owned(), json!("sending")),
                ("imported_via".to_owned(), json!(meta.imported_via)),
                (
                    "device_noun".to_owned(),
                    meta.device_noun.clone().map_or(Value::Null, Value::String),
                ),
                (
                    "source_hint".to_owned(),
                    meta.source_hint.clone().map_or(Value::Null, Value::String),
                ),
            ]),
        );
    }

    json_response(
        StatusCode::OK,
        json!({
            "upload_id": meta.upload_id,
            "next_offset": meta.next_offset,
            "total_bytes": meta.total_bytes,
            "complete": complete,
            "expires_at": format_rfc3339_ms(meta.expires_at_ms),
        }),
    )
}

/// `GET /app/import/api/uploads/{upload_id}`
pub(crate) async fn status(
    State(state): State<AppState>,
    AxumPath(upload_id): AxumPath<String>,
) -> Response {
    if !is_valid_upload_id(&upload_id) {
        return upload_not_found();
    }
    let lock_guard = start_lock::acquire(&state.root, &upload_lock_key(&upload_id)).await;
    let root = state.root.clone();
    match tokio::task::spawn_blocking(move || {
        let _guard = lock_guard;
        let Some(dir) = contained_upload_dir(&root, &upload_id) else {
            return upload_not_found();
        };
        let now = now_ms();
        let Some((meta, _, _)) = load_verified_open_upload(&root, &dir, now) else {
            return upload_not_found();
        };
        json_response(StatusCode::OK, meta.status_value(now))
    })
    .await
    {
        Ok(resp) => resp,
        Err(_) => upload_not_found(),
    }
}

/// `GET /app/import/api/uploads`
pub(crate) async fn list_active(State(state): State<AppState>) -> Response {
    let root = state.root.clone();
    match tokio::task::spawn_blocking(move || {
        let items = list_active_upload_values(&root);
        let total = items.len();
        json_response(StatusCode::OK, json!({ "items": items, "total": total }))
    })
    .await
    {
        Ok(resp) => resp,
        Err(_) => json_response(StatusCode::OK, json!({ "items": [], "total": 0 })),
    }
}

/// `POST /app/import/api/uploads/{upload_id}/commit`
pub(crate) async fn commit(
    State(state): State<AppState>,
    AxumPath(upload_id): AxumPath<String>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    if !is_valid_upload_id(&upload_id) {
        return upload_not_found();
    }
    let raw_bytes = match to_bytes(body, MAX_JSON_CONTROL_BYTES).await {
        Ok(bytes) => bytes,
        Err(_) => return invalid_state("commit request body is too large", None),
    };
    let commit_sha256 = if raw_bytes.is_empty() {
        None
    } else {
        let is_json = headers
            .get(header::CONTENT_TYPE)
            .and_then(|val| val.to_str().ok())
            .is_some_and(|ct| {
                ct.split(';')
                    .next()
                    .unwrap_or(ct)
                    .trim()
                    .eq_ignore_ascii_case("application/json")
            });
        if !is_json {
            return protocol_error(
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "unsupported_media_type",
                "commit body must be application/json.",
                "Content-Type must be application/json when a body is provided",
                None,
            );
        }
        let Ok(parsed) = serde_json::from_slice::<Value>(&raw_bytes) else {
            return invalid_state("commit body must be valid JSON", None);
        };
        let Some(obj) = parsed.as_object() else {
            return invalid_state("commit body must be a JSON object", None);
        };
        match obj.get("sha256") {
            None | Some(Value::Null) => None,
            Some(Value::String(raw)) if raw.trim().is_empty() => None,
            Some(Value::String(raw)) => match parse_sha256_hex(raw) {
                Some(hex) => Some(hex),
                None => return invalid_state("sha256 must be 64 hexadecimal characters", None),
            },
            Some(_) => return invalid_state("sha256 must be a string", None),
        }
    };

    let lock_guard = start_lock::acquire(&state.root, &upload_lock_key(&upload_id)).await;
    let state_clone = state.clone();
    match tokio::task::spawn_blocking(move || {
        let _guard = lock_guard;
        commit_sync(&state_clone, &upload_id, commit_sha256)
    })
    .await
    {
        Ok(resp) => resp,
        Err(_) => protocol_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "import_metadata_failed",
            "commit worker stopped.",
            "commit worker failed",
            None,
        ),
    }
}

fn commit_sync(state: &AppState, upload_id: &str, commit_sha256: Option<String>) -> Response {
    let _target_use =
        match solstone_core_journal_io::whole_journal_restore::begin_target_use(&state.root) {
            Ok(guard) => guard,
            Err(_) => {
                return protocol_error(
                    StatusCode::CONFLICT,
                    "journal_busy",
                    "your journal is busy right now.",
                    "journal restore is in progress",
                    None,
                );
            }
        };

    let Some(dir) = contained_upload_dir(&state.root, upload_id) else {
        return upload_not_found();
    };
    let now = now_ms();
    let Some((meta, part_path, hasher)) = load_verified_open_upload(&state.root, &dir, now) else {
        // Idempotent retry recovery: if this upload_id committed recently and its response
        // was lost in transit, return the staged replay summary.
        if let Some(client_item_id) = lookup_recent_commit(&state.root, upload_id)
            && let Some(existing) = staged_by_client_item(&state.root, &client_item_id)
        {
            return json_response(StatusCode::OK, replay_summary(&existing));
        }
        return upload_not_found();
    };

    if meta.next_offset < meta.total_bytes {
        return protocol_error(
            StatusCode::CONFLICT,
            "incomplete_upload",
            "that upload is not complete yet.",
            format!(
                "received {} of {} bytes",
                meta.next_offset, meta.total_bytes
            ),
            Some(Map::from_iter([
                ("next_offset".to_owned(), json!(meta.next_offset)),
                ("total_bytes".to_owned(), json!(meta.total_bytes)),
            ])),
        );
    }

    let expected_sha256 = match (meta.sha256.as_ref(), commit_sha256.as_ref()) {
        (Some(create_sha), Some(commit_sha)) if create_sha != commit_sha => {
            return invalid_state(
                "commit sha256 does not match sha256 declared at upload creation",
                None,
            );
        }
        (Some(create_sha), _) => create_sha.clone(),
        (None, Some(commit_sha)) => commit_sha.clone(),
        (None, None) => return missing_field("Missing sha256 on upload commit"),
    };

    let _committing = CommittingGuard::enter(upload_id);
    let actual_sha256 = hasher.finalize_hex();
    if actual_sha256 != expected_sha256 {
        let _ = remove_contained_tree(&state.root, &dir);
        clear_progress_throttle(upload_id);
        emit_importer_event(
            &state.root,
            "upload_aborted",
            Map::from_iter([
                ("upload_id".to_owned(), json!(meta.upload_id)),
                ("client_item_id".to_owned(), json!(meta.client_item_id)),
                ("reason".to_owned(), json!("checksum_mismatch")),
            ]),
        );
        return protocol_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "whole_file_checksum_mismatch",
            "the uploaded file did not match its checksum, so your journal discarded the partial copy.",
            format!("expected sha256 {expected_sha256}, computed {actual_sha256}"),
            Some(Map::from_iter([
                ("expected_sha256".to_owned(), json!(expected_sha256)),
                ("actual_sha256".to_owned(), json!(actual_sha256)),
            ])),
        );
    }

    let client_item_id = meta.client_item_id.clone();
    let total_bytes = meta.total_bytes;
    let outcome = stage_verified_upload_file(
        state,
        VerifiedFileStageInput {
            filename: meta.filename,
            original_filename: meta.original_filename,
            mime_type: meta.mime_type,
            file_size: meta.total_bytes,
            precomputed_hash: Some(SourceHash::new(actual_sha256)),
            source_file: VerifiedSourceFile::UploadPart(part_path),
            client_item_id: meta.client_item_id,
            data: meta.data,
            context: meta.context,
        },
    );

    let status = outcome.response.status();
    if status.is_server_error() {
        // Retain the verified upload and metadata on transient 5xx failures so a client retry
        // of POST /uploads/{id}/commit finishes without re-uploading 4 GiB.
        return outcome.response;
    }

    let _ = remove_contained_tree(&state.root, &dir);
    clear_progress_throttle(upload_id);

    if status.is_success() {
        record_recent_commit(&state.root, upload_id, &client_item_id);
        emit_importer_event(
            &state.root,
            "upload_committed",
            Map::from_iter([
                ("upload_id".to_owned(), json!(upload_id)),
                ("client_item_id".to_owned(), json!(client_item_id)),
                (
                    "import_id".to_owned(),
                    outcome.import_id.clone().map_or(Value::Null, Value::String),
                ),
                (
                    "timestamp".to_owned(),
                    outcome.import_id.map_or(Value::Null, Value::String),
                ),
                ("total_bytes".to_owned(), json!(total_bytes)),
            ]),
        );
    } else {
        emit_importer_event(
            &state.root,
            "upload_aborted",
            Map::from_iter([
                ("upload_id".to_owned(), json!(upload_id)),
                ("client_item_id".to_owned(), json!(client_item_id)),
                ("reason".to_owned(), json!("refused")),
            ]),
        );
    }

    outcome.response
}

/// `DELETE /app/import/api/uploads/{upload_id}`
pub(crate) async fn abort(
    State(state): State<AppState>,
    AxumPath(upload_id): AxumPath<String>,
) -> Response {
    if !is_valid_upload_id(&upload_id) {
        return upload_not_found();
    }
    let lock_guard = start_lock::acquire(&state.root, &upload_lock_key(&upload_id)).await;
    let root = state.root.clone();
    match tokio::task::spawn_blocking(move || {
        let _guard = lock_guard;
        let _target_use =
            match solstone_core_journal_io::whole_journal_restore::begin_target_use(&root) {
                Ok(guard) => guard,
                Err(_) => {
                    return protocol_error(
                        StatusCode::CONFLICT,
                        "journal_busy",
                        "your journal is busy right now.",
                        "journal restore is in progress",
                        None,
                    );
                }
            };
        let Some(dir) = contained_upload_dir(&root, &upload_id) else {
            return upload_not_found();
        };
        if !dir.exists() {
            return upload_not_found();
        }
        let client_item_id = read_upload_meta(&dir).map(|meta| meta.client_item_id);
        if let Err(err) = remove_contained_tree(&root, &dir) {
            return protocol_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "import_metadata_failed",
                "could not remove partial upload.",
                err.to_string(),
                None,
            );
        }
        clear_progress_throttle(&upload_id);
        emit_importer_event(
            &root,
            "upload_aborted",
            Map::from_iter([
                ("upload_id".to_owned(), json!(upload_id)),
                (
                    "client_item_id".to_owned(),
                    client_item_id.map_or(Value::Null, Value::String),
                ),
                ("reason".to_owned(), json!("cancelled")),
            ]),
        );
        json_response(
            StatusCode::OK,
            json!({ "deleted": true, "upload_id": upload_id }),
        )
    })
    .await
    {
        Ok(resp) => resp,
        Err(_) => protocol_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "import_metadata_failed",
            "abort worker stopped.",
            "abort worker failed",
            None,
        ),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use serde_json::{Value, json};
    use sha2::{Digest, Sha256};
    use tempfile::TempDir;
    use tokio::io::AsyncWriteExt;
    use tokio_util::io::ReaderStream;
    use tower::ServiceExt;

    use super::*;
    use crate::routes;

    struct FreeSpaceOverrideGuard {
        root: PathBuf,
    }

    impl FreeSpaceOverrideGuard {
        fn set(root: &Path, available: u64) -> Self {
            let mutex = TEST_FREE_SPACE_OVERRIDES.get_or_init(|| Mutex::new(HashMap::new()));
            let mut guard = match mutex.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            guard.insert(root.to_path_buf(), available);
            Self {
                root: root.to_path_buf(),
            }
        }
    }

    impl Drop for FreeSpaceOverrideGuard {
        fn drop(&mut self) {
            if let Some(mutex) = TEST_FREE_SPACE_OVERRIDES.get() {
                let mut guard = match mutex.lock() {
                    Ok(guard) => guard,
                    Err(poisoned) => poisoned.into_inner(),
                };
                guard.remove(&self.root);
            }
        }
    }

    /// Creates a tiny-buffered duplex `Body` stream (64-byte duplex buffer) so the writer task
    /// can only finish writing all `chunks` if the HTTP handler drains the entire body!
    fn metered_stream_body(chunks: Vec<Vec<u8>>) -> (Body, Arc<AtomicUsize>) {
        let written_chunks = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&written_chunks);
        let (mut writer, reader) = tokio::io::duplex(64);
        tokio::spawn(async move {
            for chunk in chunks {
                if writer.write_all(&chunk).await.is_err() {
                    return;
                }
                counter.fetch_add(1, Ordering::SeqCst);
            }
            let _ = writer.shutdown().await;
        });
        (Body::from_stream(ReaderStream::new(reader)), written_chunks)
    }

    fn setup_journal() -> TempDir {
        let parent = fs::canonicalize(std::env::temp_dir()).expect("temporary directory");
        let dir = TempDir::new_in(parent).expect("tempdir");
        fs::create_dir_all(dir.path().join("config")).expect("config dir");
        fs::write(
            dir.path().join("config/journal.json"),
            r#"{"timezone":"America/Denver"}"#,
        )
        .expect("write config");
        dir
    }

    async fn send_req(root: &Path, request: Request<Body>) -> (StatusCode, Value) {
        let response = routes(root.to_path_buf())
            .oneshot(request)
            .await
            .expect("router oneshot");
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 1024 * 1024)
            .await
            .expect("response body");
        let parsed = serde_json::from_slice::<Value>(&bytes)
            .unwrap_or_else(|_| json!({ "_raw": String::from_utf8_lossy(&bytes) }));
        (status, parsed)
    }

    #[test]
    fn resumable_sha256_matches_sha2_across_all_block_boundaries_and_persistence() {
        for len in [0_usize, 1, 55, 56, 63, 64, 65, 127, 128, 129, 8193, 131_075] {
            let data: Vec<u8> = (0..len).map(|i| ((i * 31 + 7) & 0xff) as u8).collect();
            let expected = format!("{:x}", Sha256::digest(&data));

            let mut oneshot = ResumableSha256::new();
            oneshot.update(&data);
            assert_eq!(
                oneshot.finalize_hex(),
                expected,
                "oneshot mismatch at {len}"
            );

            let mut split = ResumableSha256::new();
            let mut cursor = 0;
            for step in [1_usize, 7, 56, 63, 64, 65, 511, 4096] {
                if cursor >= data.len() {
                    break;
                }
                let end = (cursor + step).min(data.len());
                split.update(&data[cursor..end]);
                cursor = end;

                let (mid, blocks, tail) = split.to_persisted();
                split = ResumableSha256::from_persisted(&mid, blocks, &tail, cursor as u64)
                    .expect("persisted midstate restores");
            }
            if cursor < data.len() {
                split.update(&data[cursor..]);
            }
            assert_eq!(
                split.finalize_hex(),
                expected,
                "split+persisted mismatch at {len}"
            );
        }
    }

    #[tokio::test]
    async fn create_put_commit_stages_import_and_cleans_up_partial_dir() {
        let journal = setup_journal();
        let part_a = b"First chunk of a synthetic multi-part markdown note.\n".repeat(400);
        let part_b = b"Second chunk completing the synthetic markdown note.\n".repeat(250);
        let mut full = Vec::new();
        full.extend_from_slice(&part_a);
        full.extend_from_slice(&part_b);
        let full_sha = format!("{:x}", Sha256::digest(&full));

        // Web mode: omit sha256 at create, provide at commit.
        let (status, created) = send_req(
            journal.path(),
            Request::builder()
                .method("POST")
                .uri(UPLOAD_COLLECTION_PATH)
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "client_item_id": "web-item-1",
                        "filename": "notes.md",
                        "mime_type": "text/markdown",
                        "total_bytes": full.len(),
                        "imported_via": "web_dashboard",
                        "device_noun": "browser",
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{created}");
        let upload_id = created["upload_id"].as_str().unwrap().to_owned();
        assert_eq!(created["next_offset"], 0);
        assert_eq!(created["chunk_bytes"], RESUMABLE_CHUNK_BYTES);

        // Upload part A with X-Chunk-Sha256.
        let sha_a = format!("{:x}", Sha256::digest(&part_a));
        let (status, put_a) = send_req(
            journal.path(),
            Request::builder()
                .method("PUT")
                .uri(format!("{UPLOAD_COLLECTION_PATH}/{upload_id}?offset=0"))
                .header("content-type", "application/octet-stream")
                .header("content-length", part_a.len().to_string())
                .header("x-chunk-sha256", &sha_a)
                .body(Body::from(part_a.clone()))
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{put_a}");
        assert_eq!(put_a["next_offset"], part_a.len() as u64);
        assert_eq!(put_a["complete"], false);

        // Premature commit before part B -> 409 incomplete_upload.
        let (status, early) = send_req(
            journal.path(),
            Request::builder()
                .method("POST")
                .uri(format!("{UPLOAD_COLLECTION_PATH}/{upload_id}/commit"))
                .header("content-type", "application/json")
                .body(Body::from(json!({ "sha256": &full_sha }).to_string()))
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT, "{early}");
        assert_eq!(early["reason_code"], "incomplete_upload");

        // Upload part B.
        let sha_b = format!("{:x}", Sha256::digest(&part_b));
        let (status, put_b) = send_req(
            journal.path(),
            Request::builder()
                .method("PUT")
                .uri(format!(
                    "{UPLOAD_COLLECTION_PATH}/{upload_id}?offset={}",
                    part_a.len()
                ))
                .header("content-type", "application/octet-stream")
                .header("content-length", part_b.len().to_string())
                .header("x-chunk-sha256", &sha_b)
                .body(Body::from(part_b.clone()))
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{put_b}");
        assert_eq!(put_b["complete"], true);

        // Commit with finalized whole-file sha256.
        let (status, committed) = send_req(
            journal.path(),
            Request::builder()
                .method("POST")
                .uri(format!("{UPLOAD_COLLECTION_PATH}/{upload_id}/commit"))
                .header("content-type", "application/json")
                .body(Body::from(json!({ "sha256": &full_sha }).to_string()))
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{committed}");
        assert_eq!(committed["status"], "staged");
        assert_eq!(committed["client_item_id"], "web-item-1");
        assert_eq!(committed["recommended_action"], "start");

        let staged_path = PathBuf::from(committed["path"].as_str().unwrap());
        assert_eq!(fs::read(&staged_path).unwrap(), full);
        assert!(!uploads_root(journal.path()).join(&upload_id).exists());

        // Retrying commit after lost response returns idempotent replay summary.
        let (status, retry_commit) = send_req(
            journal.path(),
            Request::builder()
                .method("POST")
                .uri(format!("{UPLOAD_COLLECTION_PATH}/{upload_id}/commit"))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{retry_commit}");
        assert_eq!(retry_commit["replay"], true);

        // Retrying create for same client_item_id + sha256 short-circuits with 200 already_staged.
        let (status, dedup_create) = send_req(
            journal.path(),
            Request::builder()
                .method("POST")
                .uri(UPLOAD_COLLECTION_PATH)
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "client_item_id": "web-item-1",
                        "filename": "notes.md",
                        "total_bytes": full.len(),
                        "sha256": &full_sha,
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{dedup_create}");
        assert_eq!(dedup_create["status"], "already_staged");
        assert_eq!(dedup_create["replay"], true);
    }

    #[tokio::test]
    async fn put_drains_request_body_before_404_409_422_and_enforces_wire_framing() {
        let journal = setup_journal();
        let payload = vec![0x42_u8; 4096];
        let payload_sha = format!("{:x}", Sha256::digest(&payload));

        let (_, created) = send_req(
            journal.path(),
            Request::builder()
                .method("POST")
                .uri(UPLOAD_COLLECTION_PATH)
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "client_item_id": "drain-item",
                        "filename": "clip.m4a",
                        "total_bytes": 8192,
                        "sha256": payload_sha,
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await;
        let upload_id = created["upload_id"].as_str().unwrap().to_owned();

        // 1. Expect: 100-continue -> 417
        let (status, exp_err) = send_req(
            journal.path(),
            Request::builder()
                .method("PUT")
                .uri(format!("{UPLOAD_COLLECTION_PATH}/{upload_id}?offset=0"))
                .header("content-length", "4")
                .header("expect", "100-continue")
                .body(Body::from("abcd"))
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::EXPECTATION_FAILED);
        assert_eq!(exp_err["reason_code"], "invalid_chunk_framing");

        // 2. Missing Content-Length / Transfer-Encoding: chunked -> 400
        let (status, te_err) = send_req(
            journal.path(),
            Request::builder()
                .method("PUT")
                .uri(format!("{UPLOAD_COLLECTION_PATH}/{upload_id}?offset=0"))
                .header("transfer-encoding", "chunked")
                .body(Body::from("abcd"))
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(te_err["reason_code"], "content_length_required");

        // 3. 409 offset_mismatch drains all body frames first!
        let (body_409, frames_409) = metered_stream_body(vec![
            vec![1_u8; 1024],
            vec![2_u8; 1024],
            vec![3_u8; 1024],
            vec![4_u8; 1024],
        ]);
        let (status, conflict) = send_req(
            journal.path(),
            Request::builder()
                .method("PUT")
                .uri(format!("{UPLOAD_COLLECTION_PATH}/{upload_id}?offset=4096"))
                .header("content-length", "4096")
                .body(body_409)
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT, "{conflict}");
        assert_eq!(conflict["reason_code"], "offset_mismatch");
        assert_eq!(conflict["next_offset"], 0);
        assert_eq!(
            frames_409.load(Ordering::SeqCst),
            4,
            "all 4 1KiB chunks over a 64B pipe must be drained before 409 response"
        );

        // 4. 422 chunk_checksum_mismatch drains all body frames first!
        let (body_422, frames_422) = metered_stream_body(vec![vec![9_u8; 2048], vec![9_u8; 2048]]);
        let (status, bad_sha) = send_req(
            journal.path(),
            Request::builder()
                .method("PUT")
                .uri(format!("{UPLOAD_COLLECTION_PATH}/{upload_id}?offset=0"))
                .header("content-length", "4096")
                .header("x-chunk-sha256", &"0".repeat(64))
                .body(body_422)
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{bad_sha}");
        assert_eq!(bad_sha["reason_code"], "chunk_checksum_mismatch");
        assert_eq!(frames_422.load(Ordering::SeqCst), 2);

        // 5. 404 upload_not_found drains all body frames first!
        let (body_404, frames_404) = metered_stream_body(vec![vec![7_u8; 1024], vec![7_u8; 1024]]);
        let missing_id = "550e8400-e29b-41d4-a716-446655440000";
        let (status, not_found) = send_req(
            journal.path(),
            Request::builder()
                .method("PUT")
                .uri(format!("{UPLOAD_COLLECTION_PATH}/{missing_id}?offset=0"))
                .header("content-length", "2048")
                .body(body_404)
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{not_found}");
        assert_eq!(not_found["reason_code"], "upload_not_found");
        assert_eq!(frames_404.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn restart_recovers_midstate_truncates_uncommitted_tail_and_rejects_bad_whole_sha() {
        let journal = setup_journal();
        let chunk_0 = vec![0xaa_u8; 4096];
        let chunk_1 = vec![0xbb_u8; 2048];
        let mut combined = chunk_0.clone();
        combined.extend_from_slice(&chunk_1);
        let good_sha = format!("{:x}", Sha256::digest(&combined));

        let (_, created) = send_req(
            journal.path(),
            Request::builder()
                .method("POST")
                .uri(UPLOAD_COLLECTION_PATH)
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "client_item_id": "crash-resume-1",
                        "filename": "archive.txt",
                        "total_bytes": combined.len(),
                        "sha256": &good_sha,
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await;
        let upload_id = created["upload_id"].as_str().unwrap().to_owned();

        let (status, _) = send_req(
            journal.path(),
            Request::builder()
                .method("PUT")
                .uri(format!("{UPLOAD_COLLECTION_PATH}/{upload_id}?offset=0"))
                .header("content-length", chunk_0.len().to_string())
                .body(Body::from(chunk_0.clone()))
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        // Simulate a SIGKILL after writing 500 bytes of chunk 1 to payload.part
        // before meta.json was updated.
        let part_path = uploads_root(journal.path())
            .join(&upload_id)
            .join(PAYLOAD_PART_FILE);
        {
            let mut file = OpenOptions::new().append(true).open(&part_path).unwrap();
            file.write_all(&[0xff_u8; 500]).unwrap();
        }
        assert_eq!(fs::metadata(&part_path).unwrap().len(), 4596);

        // Resync via POST /uploads returns open with next_offset = 4096 (no chunk 0 re-send!).
        let (status, resumed) = send_req(
            journal.path(),
            Request::builder()
                .method("POST")
                .uri(UPLOAD_COLLECTION_PATH)
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "client_item_id": "crash-resume-1",
                        "filename": "archive.txt",
                        "total_bytes": combined.len(),
                        "sha256": &good_sha,
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(resumed["upload_id"], upload_id);
        assert_eq!(resumed["next_offset"], 4096);

        // Send chunk 1 -> truncates uncommitted 500B tail, appends chunk 1, commits cleanly!
        let (status, _) = send_req(
            journal.path(),
            Request::builder()
                .method("PUT")
                .uri(format!("{UPLOAD_COLLECTION_PATH}/{upload_id}?offset=4096"))
                .header("content-length", chunk_1.len().to_string())
                .body(Body::from(chunk_1))
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        // Native-style bodyless POST /commit uses sha256 from create.
        let (status, committed) = send_req(
            journal.path(),
            Request::builder()
                .method("POST")
                .uri(format!("{UPLOAD_COLLECTION_PATH}/{upload_id}/commit"))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{committed}");
        let staged = PathBuf::from(committed["path"].as_str().unwrap());
        assert_eq!(fs::read(&staged).unwrap(), combined);

        // Now test whole-file checksum mismatch -> 422 and deletes partial directory.
        let (_, bad_created) = send_req(
            journal.path(),
            Request::builder()
                .method("POST")
                .uri(UPLOAD_COLLECTION_PATH)
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "client_item_id": "bad-whole-sha",
                        "filename": "corrupt.txt",
                        "total_bytes": 16,
                        "sha256": "f".repeat(64),
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await;
        let bad_id = bad_created["upload_id"].as_str().unwrap().to_owned();
        let (status, _) = send_req(
            journal.path(),
            Request::builder()
                .method("PUT")
                .uri(format!("{UPLOAD_COLLECTION_PATH}/{bad_id}?offset=0"))
                .header("content-length", "16")
                .body(Body::from("0123456789abcdef"))
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (status, mismatch) = send_req(
            journal.path(),
            Request::builder()
                .method("POST")
                .uri(format!("{UPLOAD_COLLECTION_PATH}/{bad_id}/commit"))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{mismatch}");
        assert_eq!(mismatch["reason_code"], "whole_file_checksum_mismatch");
        assert!(!uploads_root(journal.path()).join(&bad_id).exists());
    }

    #[tokio::test]
    async fn free_space_headroom_and_sweeps_protect_live_save_and_clean_orphans() {
        let journal = setup_journal();
        {
            // Simulate only 100 MiB free on this journal only -> below 256 MiB headroom -> 507.
            let _low_space = FreeSpaceOverrideGuard::set(journal.path(), 100 * 1024 * 1024);
            let (status, space_err) = send_req(
                journal.path(),
                Request::builder()
                    .method("POST")
                    .uri(UPLOAD_COLLECTION_PATH)
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({
                            "client_item_id": "space-item",
                            "filename": "big.zip",
                            "total_bytes": 32 * 1024 * 1024,
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await;
            assert_eq!(status, StatusCode::INSUFFICIENT_STORAGE, "{space_err}");
            assert_eq!(space_err["reason_code"], "insufficient_journal_space");
        }

        // Simulate plenty of disk space on a dedicated journal to test the 8 GiB aggregate cap.
        {
            let cap_journal = setup_journal();
            let _ample_space =
                FreeSpaceOverrideGuard::set(cap_journal.path(), 64 * 1024 * 1024 * 1024);
            for idx in [1, 2] {
                let (status, _) = send_req(
                    cap_journal.path(),
                    Request::builder()
                        .method("POST")
                        .uri(UPLOAD_COLLECTION_PATH)
                        .header("content-type", "application/json")
                        .body(Body::from(
                            json!({
                                "client_item_id": format!("four-gib-{idx}"),
                                "filename": format!("part-{idx}.zip"),
                                "total_bytes": MAX_UPLOAD_FILE_BYTES,
                            })
                            .to_string(),
                        ))
                        .unwrap(),
                )
                .await;
                assert_eq!(status, StatusCode::CREATED);
            }
            let (status, cap_err) = send_req(
                cap_journal.path(),
                Request::builder()
                    .method("POST")
                    .uri(UPLOAD_COLLECTION_PATH)
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({
                            "client_item_id": "over-eight-gib",
                            "filename": "extra.zip",
                            "total_bytes": 1024,
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await;
            assert_eq!(status, StatusCode::INSUFFICIENT_STORAGE, "{cap_err}");
            assert_eq!(cap_err["reason_code"], "insufficient_journal_space");
        }

        // Create an open upload, test GET /uploads & DELETE /uploads/{id}.
        let (status, created) = send_req(
            journal.path(),
            Request::builder()
                .method("POST")
                .uri(UPLOAD_COLLECTION_PATH)
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "client_item_id": "cancel-me",
                        "filename": "voice.m4a",
                        "total_bytes": 1024,
                        "device_noun": "iphone",
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        let cancel_id = created["upload_id"].as_str().unwrap().to_owned();

        let (status, list_resp) = send_req(
            journal.path(),
            Request::builder()
                .method("GET")
                .uri(UPLOAD_COLLECTION_PATH)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(list_resp["total"], 1);
        assert_eq!(list_resp["items"][0]["device_noun"], "iphone");

        let (status, del_resp) = send_req(
            journal.path(),
            Request::builder()
                .method("DELETE")
                .uri(format!("{UPLOAD_COLLECTION_PATH}/{cancel_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{del_resp}");
        assert_eq!(del_resp["deleted"], true);
        assert!(!uploads_root(journal.path()).join(&cancel_id).exists());

        // Test .save-tmp orphan sweep spares both live active saves AND young (< 1h) unheld
        // files from an overlapping restart process, while deleting > 1h dead SIGKILL orphans.
        let save_tmp = journal.path().join("imports/.save-tmp");
        fs::create_dir_all(&save_tmp).unwrap();
        let dead_orphan = save_tmp.join(".tmp_dead_sigkill_orphan");
        let young_other_process = save_tmp.join(".tmp_young_other_process");
        let live_in_flight = save_tmp.join(".tmp_live_in_flight");
        fs::write(&dead_orphan, b"orphan bytes").unwrap();
        fs::write(&young_other_process, b"young bytes").unwrap();
        fs::write(&live_in_flight, b"live bytes").unwrap();
        let two_hours_ago =
            filetime::FileTime::from_unix_time((now_ms() / 1_000) - (2 * 60 * 60), 0);
        filetime::set_file_mtime(&dead_orphan, two_hours_ago).unwrap();
        filetime::set_file_mtime(&live_in_flight, two_hours_ago).unwrap();
        let _live_guard = save_stream::register_active_save_path(&live_in_flight);

        let (status, exp_created) = send_req(
            journal.path(),
            Request::builder()
                .method("POST")
                .uri(UPLOAD_COLLECTION_PATH)
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "client_item_id": "expire-me",
                        "filename": "old.zip",
                        "total_bytes": 1024,
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        let exp_id = exp_created["upload_id"].as_str().unwrap().to_owned();
        let exp_dir = uploads_root(journal.path()).join(&exp_id);
        let mut exp_meta = read_upload_meta(&exp_dir).unwrap();
        exp_meta.expires_at_ms = now_ms() - 1_000;
        write_upload_meta(&exp_dir, &exp_meta).unwrap();

        let in_creation_dir =
            uploads_root(journal.path()).join("550e8400-e29b-41d4-a716-446655440099");
        fs::create_dir_all(&in_creation_dir).unwrap();

        // Live 5-min opportunistic sweep gives a 60s grace window to directories mid-creation,
        // while immediately removing expired uploads.
        sweep_expired_uploads(journal.path());
        assert!(
            in_creation_dir.exists(),
            "live opportunistic sweep must spare young in-creation upload dir"
        );
        assert!(!exp_dir.exists(), "expired .uploads/<id> must be swept");

        // Startup sweep (zero grace) cleans both half-written crash dirs and dead .save-tmp orphans
        // while sparing live in-flight .save-tmp files.
        sweep_uploads_and_save_temps(journal.path());
        assert!(
            !in_creation_dir.exists(),
            "startup sweep must clean half-written crash upload dir"
        );
        assert!(!dead_orphan.exists(), "dead .save-tmp orphan must be swept");
        assert!(
            young_other_process.exists(),
            "young (< 1h) .save-tmp file must be spared across overlapping process restarts"
        );
        assert!(
            live_in_flight.exists(),
            "live in-flight .save-tmp file must never be deleted"
        );
    }

    #[tokio::test]
    async fn commit_5xx_retains_verified_upload_and_open_uploads_reserve_free_space() {
        // 1. Transient 5xx during commit retains payload.part + meta.json so retry succeeds.
        let journal = setup_journal();
        let payload = b"Resumable upload surviving a transient 5xx on commit.\n".repeat(128);
        let sha = format!("{:x}", Sha256::digest(&payload));

        let (status, created) = send_req(
            journal.path(),
            Request::builder()
                .method("POST")
                .uri(UPLOAD_COLLECTION_PATH)
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "client_item_id": "transient-500-item",
                        "filename": "note.md",
                        "total_bytes": payload.len(),
                        "sha256": &sha,
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        let upload_id = created["upload_id"].as_str().unwrap().to_owned();

        let (status, _) = send_req(
            journal.path(),
            Request::builder()
                .method("PUT")
                .uri(format!("{UPLOAD_COLLECTION_PATH}/{upload_id}?offset=0"))
                .header("content-length", payload.len().to_string())
                .body(Body::from(payload.clone()))
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        crate::lifecycle::fail_next_commit_metadata_write_for_root(journal.path());
        let (status, failed_commit) = send_req(
            journal.path(),
            Request::builder()
                .method("POST")
                .uri(format!("{UPLOAD_COLLECTION_PATH}/{upload_id}/commit"))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{failed_commit}");
        assert!(
            uploads_root(journal.path())
                .join(&upload_id)
                .join(PAYLOAD_PART_FILE)
                .is_file(),
            "payload.part must remain intact after transient 5xx commit failure"
        );

        let (status, retry_ok) = send_req(
            journal.path(),
            Request::builder()
                .method("POST")
                .uri(format!("{UPLOAD_COLLECTION_PATH}/{upload_id}/commit"))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{retry_ok}");
        assert_eq!(
            fs::read(retry_ok["path"].as_str().unwrap()).unwrap(),
            payload
        );

        // 2. Two 4 GiB creates on 5 GiB free: first succeeds, second fails 507 upfront because
        //    the first open upload's 4 GiB remaining bytes are counted!
        let five_gib_journal = setup_journal();
        let _five_gib =
            FreeSpaceOverrideGuard::set(five_gib_journal.path(), 5 * 1024 * 1024 * 1024);
        let (status, _) = send_req(
            five_gib_journal.path(),
            Request::builder()
                .method("POST")
                .uri(UPLOAD_COLLECTION_PATH)
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "client_item_id": "first-4g",
                        "filename": "one.zip",
                        "total_bytes": MAX_UPLOAD_FILE_BYTES,
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);

        let (status, second_err) = send_req(
            five_gib_journal.path(),
            Request::builder()
                .method("POST")
                .uri(UPLOAD_COLLECTION_PATH)
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "client_item_id": "second-4g",
                        "filename": "two.zip",
                        "total_bytes": MAX_UPLOAD_FILE_BYTES,
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::INSUFFICIENT_STORAGE, "{second_err}");
        assert_eq!(second_err["reason_code"], "insufficient_journal_space");

        // 3. Create body > 64 KiB -> 413, oversized client bag -> 400 JSON.
        let (status, bag_err) = send_req(
            journal.path(),
            Request::builder()
                .method("POST")
                .uri(UPLOAD_COLLECTION_PATH)
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "client_item_id": "big-bag",
                        "filename": "bag.txt",
                        "total_bytes": 100,
                        "client": { "blob": "x".repeat(20 * 1024) },
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bag_err}");
        assert_eq!(bag_err["reason_code"], "invalid_operation_for_state");
    }
}
