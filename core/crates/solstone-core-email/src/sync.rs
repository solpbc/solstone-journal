// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! End-to-end email source orchestration: connect, resumable header scan,
//! choice-before-sync, newest-first batched fetch/extract/land, re-reach,
//! status projection, password rotation, and disconnect.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use jiff::Timestamp;
use jiff::tz::TimeZone;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use solstone_core_journal_io::atomic::{AtomicWriteOptions, atomic_replace};

use crate::cache::HeaderCache;
use crate::choice::{
    Accounting, Choice, ChoicePreview, compute_accounting, detect_folder_cap, earliest_sent_month,
    preview_choice, range_cutoff_ms,
};
use crate::error::{EmailError, Result};
use crate::extract::{
    self, ANSWERED_NO_TEXT_FALLBACK, CAP_CHARS, Evidence, Extracted, ForwardedBlock, cap_text,
    owner_fallback_text,
};
use crate::imap::{ImapSource, MailboxInfo, WireAudit};
use crate::model::{BodyChoice, Header, MessageType, Part, RangePreset, choose_body};
use crate::select::{
    Decision, RULES_VERSION, STALE_SKIP_BEFORE_VERSION, evaluate_decisions, learn_owner_aliases,
};
use crate::store::{
    self, AccountMeta, Credentials, EMAIL_STREAM, LandedRecord, PROVIDER_GMAIL, SourceState,
    SyncRunRecord,
};

const SCAN_PAGE_SIZE: u32 = 250;
const FETCH_BATCH_SIZE: usize = 50;

/// Summary returned by [`run_header_scan`].
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ScanReport {
    pub mailbox: String,
    pub checked: u64,
    pub total: u64,
    pub sent_found: usize,
    pub eligible_sent: usize,
    pub left_out_total: usize,
    pub complete: bool,
    pub earliest_month: Option<String>,
    pub folder_cap_warning: Option<u64>,
    pub wire: WireAudit,
}

/// One written email segment created during a sync batch, ready for publication
/// into the stream record, search indexer, and supervisor event bus.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreatedEmailSegment {
    pub day: String,
    pub segment: String,
    pub file: PathBuf,
}

/// Summary returned by [`run_sync`].
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SyncReport {
    pub run_id: Option<String>,
    pub scanned: u64,
    pub landed_messages: usize,
    pub landed_segments: usize,
    pub skipped_already_landed: usize,
    pub missing_in_mailbox: usize,
    pub wire: WireAudit,
    #[serde(default)]
    pub created_segments: Vec<CreatedEmailSegment>,
}

/// Options for [`run_sync`].
#[derive(Clone, Debug, Default)]
pub struct SyncOptions {
    /// Stop after landing at most this many exchanges (used by tests to
    /// exercise interrupted-sync resume cleanly).
    pub max_messages: Option<usize>,
    /// Override selection rules version (defaults to [`RULES_VERSION`]).
    pub rules_version: Option<u32>,
    /// Override stale-skip re-reach threshold (defaults to [`STALE_SKIP_BEFORE_VERSION`]).
    pub stale_before_version: Option<u32>,
    /// Skip acquiring the file lock when the caller already holds it.
    pub lock_held: bool,
}

/// Canonical `/app/sources/api/email` JSON snapshot.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StatusSnapshot {
    pub source: &'static str,
    pub state: &'static str,
    pub phase: String,
    pub provider: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attention_reason: Option<String>,
    pub connected: bool,
    pub syncing: bool,
    pub scan: ScanSnapshot,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub choice: Option<ChoiceSnapshot>,
    pub sync: SyncSnapshot,
    pub accounting: Accounting,
    pub runs: Vec<RunSnapshot>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScanSnapshot {
    pub complete: bool,
    pub paused: bool,
    pub checked: u64,
    pub total: u64,
    pub sent_found: usize,
    pub eligible_sent: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub earliest_month: Option<String>,
    pub bodies_read: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub folder_cap_warning: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChoiceSnapshot {
    pub range: RangePreset,
    pub types: Vec<MessageType>,
    pub covered_ranges: Vec<RangePreset>,
    pub updated_at: String,
}

/// Presets covered by an existing historical range choice (for widen-mode UI tags).
#[must_use]
pub fn covered_ranges_for(range: RangePreset) -> Vec<RangePreset> {
    match range {
        RangePreset::None => Vec::new(),
        RangePreset::Last30d => vec![RangePreset::Last30d],
        RangePreset::Last6m => vec![RangePreset::Last30d, RangePreset::Last6m],
        RangePreset::Last1y => vec![
            RangePreset::Last30d,
            RangePreset::Last6m,
            RangePreset::Last1y,
        ],
        RangePreset::All => vec![
            RangePreset::Last30d,
            RangePreset::Last6m,
            RangePreset::Last1y,
            RangePreset::All,
        ],
    }
}

/// Fast non-blocking check for Home/rail attention badges: true iff credentials
/// exist and the last check/sync recorded an unresolved attention reason.
#[must_use]
pub fn needs_attention(root: &Path) -> bool {
    let connected = matches!(store::read_credentials(root), Ok(Some(_)));
    if !connected {
        return false;
    }
    store::read_state(root)
        .ok()
        .is_some_and(|s| s.attention_reason.is_some())
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SyncSnapshot {
    pub active: bool,
    pub done: usize,
    pub total: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_added_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_checked_at: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RunSnapshot {
    pub id: String,
    pub synced_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub range_label: Option<RangePreset>,
    pub messages: usize,
    pub live_messages: usize,
    pub live_segments: usize,
}

struct BodiesCleanup<'a>(&'a Path);

impl Drop for BodiesCleanup<'_> {
    fn drop(&mut self) {
        store::clear_transient_bodies(self.0);
    }
}

/// Record a credential or IMAP access failure as `needs_attention` on disk.
pub fn record_attention(root: &Path, reason: &str) -> Result<()> {
    let mut state = store::read_state(root)?;
    state.phase = "needs_attention".to_owned();
    state.attention_reason = Some(reason.to_owned());
    state.fetching_active = false;
    store::write_state(root, &state)
}

/// Verify credentials against a read-only IMAP source, persist them, and
/// prepare the account for its header-only scan (0 bodies read).
pub fn connect_and_verify(
    root: &Path,
    email: &str,
    app_password: &str,
    source: &mut dyn ImapSource,
    now: Timestamp,
) -> Result<MailboxInfo> {
    let _lock = store::hold_sync_lock(root)?;
    let normalized_email = store::normalize_email_address(email)?;
    let normalized_password = store::normalize_app_password(app_password)?;
    let mb = source.mailbox_info()?;
    store::clear_cancel(root);

    let prev_account = store::read_account(root)?;
    let same_account = prev_account
        .as_ref()
        .is_some_and(|a| a.account.eq_ignore_ascii_case(&normalized_email));

    if !same_account {
        // Switching accounts resets the rebuildable header cache, pending
        // decisions, and choice, while leaving previously landed exchanges intact.
        let cache_path = store::header_cache_path(root);
        if cache_path.exists() {
            let _ = std::fs::remove_file(&cache_path);
        }
        let _ = std::fs::remove_file(store::config_dir(root).join("decisions.jsonl"));
        let _ = std::fs::remove_file(store::config_dir(root).join("choice.json"));
    }

    let creds = Credentials {
        provider: PROVIDER_GMAIL.to_owned(),
        account: normalized_email.clone(),
        app_password: normalized_password,
    };
    store::write_credentials(root, Some(&creds))?;

    let mut aliases = prev_account
        .filter(|_| same_account)
        .map(|a| a.owner_aliases)
        .unwrap_or_default();
    if !aliases
        .iter()
        .any(|a| a.eq_ignore_ascii_case(&normalized_email))
    {
        aliases.push(normalized_email.clone());
    }
    let meta = AccountMeta {
        provider: PROVIDER_GMAIL.to_owned(),
        account: normalized_email,
        mailbox: mb.name.clone(),
        owner_aliases: aliases,
        connected_at: now.to_string(),
        disconnected_at: None,
    };
    store::write_account(root, &meta)?;

    let mut state = if same_account {
        store::read_state(root)?
    } else {
        SourceState::default()
    };
    state.attention_reason = None;
    state.scan_paused = false;
    state.scan_total = u64::from(mb.exists);
    state.folder_cap_warning = detect_folder_cap(u64::from(mb.exists));
    state.phase = if same_account && state.scan_complete {
        if store::read_choice(root)?.is_some() {
            "steady".to_owned()
        } else {
            "awaiting_choice".to_owned()
        }
    } else {
        "scanning".to_owned()
    };
    store::write_state(root, &state)?;
    Ok(mb)
}

/// Replace the stored app password after verifying it against a read-only IMAP
/// session, clearing `needs_attention` without losing scan state or choices.
pub fn update_password(
    root: &Path,
    new_password: &str,
    source: &mut dyn ImapSource,
) -> Result<MailboxInfo> {
    let _lock = store::hold_sync_lock(root)?;
    let normalized = store::normalize_app_password(new_password)?;
    let account = store::read_account(root)?.ok_or(EmailError::NotConnected)?;
    let mb = source.mailbox_info()?;
    let creds = Credentials {
        provider: account.provider,
        account: account.account,
        app_password: normalized,
    };
    store::write_credentials(root, Some(&creds))?;

    let mut state = store::read_state(root)?;
    state.attention_reason = None;
    state.phase = if !state.scan_complete {
        if state.scan_paused {
            "scan_paused".to_owned()
        } else {
            "scanning".to_owned()
        }
    } else if store::read_choice(root)?.is_some() {
        "steady".to_owned()
    } else {
        "awaiting_choice".to_owned()
    };
    store::write_state(root, &state)?;
    Ok(mb)
}

/// Disconnect Gmail: remove the app password and mark state disconnected.
/// All exchanges already in the journal and the landed/tombstone ledger stay.
pub fn disconnect(root: &Path, now: Timestamp) -> Result<()> {
    store::request_cancel(root)?;
    store::write_credentials(root, None)?;
    let _lock = store::hold_sync_lock(root)?;
    store::write_credentials(root, None)?;
    store::clear_transient_bodies(root);
    store::clear_cancel(root);

    if let Some(mut account) = store::read_account(root)? {
        account.disconnected_at = Some(now.to_string());
        store::write_account(root, &account)?;
    }
    let mut state = store::read_state(root)?;
    state.phase = "disconnected".to_owned();
    state.attention_reason = None;
    state.fetching_active = false;
    state.scan_paused = false;
    store::write_state(root, &state)?;
    Ok(())
}

/// Run (or resume, or catch up) the read-only header-only scan into
/// `cache/email/headers.sqlite` and evaluate selection decisions.
///
/// Reads **zero** message bodies.
pub fn run_header_scan(
    root: &Path,
    source: &mut dyn ImapSource,
    now: Timestamp,
    zone: &TimeZone,
    lock_held: bool,
) -> Result<ScanReport> {
    run_header_scan_with_versions(
        root,
        source,
        now,
        zone,
        lock_held,
        RULES_VERSION,
        STALE_SKIP_BEFORE_VERSION,
    )
}

pub fn run_header_scan_with_versions(
    root: &Path,
    source: &mut dyn ImapSource,
    now: Timestamp,
    zone: &TimeZone,
    lock_held: bool,
    rules_version: u32,
    stale_before_version: u32,
) -> Result<ScanReport> {
    let _lock = if lock_held {
        None
    } else {
        Some(store::hold_sync_lock(root)?)
    };
    let mut account = store::read_account(root)?.ok_or(EmailError::NotConnected)?;
    store::clear_cancel(root);

    let mb = match source.mailbox_info() {
        Ok(mb) => mb,
        Err(err) => {
            if let Some(reason) = err.attention_reason() {
                let _ = record_attention(root, reason);
            }
            return Err(err);
        }
    };
    let folder_cap_warning = detect_folder_cap(u64::from(mb.exists));

    let mut cache = HeaderCache::open(&store::header_cache_path(root))?;
    let cursor = cache.cursor(&mb.name)?;
    let start_uid = match cursor {
        Some(c) if c.uid_validity == mb.uid_validity => c.last_uid.saturating_add(1),
        Some(_) => {
            cache.clear_all()?;
            1
        }
        None => 1,
    };

    let highest_uid = mb.uid_next.saturating_sub(1);
    let mut state = store::read_state(root)?;
    let initial_scan = !state.scan_complete;
    if initial_scan {
        state.phase = "scanning".to_owned();
    }
    state.scan_paused = false;
    state.attention_reason = None;
    state.scan_total = u64::from(mb.exists).max(cache.count()?);
    state.folder_cap_warning = folder_cap_warning;
    store::write_state(root, &state)?;

    if highest_uid == 0 || start_uid > highest_uid {
        let last_uid = highest_uid.max(start_uid.saturating_sub(1));
        cache.put_page(&mb.name, mb.uid_validity, last_uid, mb.exists, true, &[])?;
    } else {
        let mut lo = start_uid;
        while lo <= highest_uid {
            if store::is_cancel_requested(root) {
                state.scan_paused = true;
                if initial_scan {
                    state.phase = "scan_paused".to_owned();
                }
                state.scan_checked = cache.count()?;
                store::write_state(root, &state)?;
                return Err(EmailError::Cancelled);
            }
            let hi = lo.saturating_add(SCAN_PAGE_SIZE - 1).min(highest_uid);
            let rows = match source.scan_page(&mb, lo, hi) {
                Ok(rows) => rows,
                Err(err) => {
                    if let Some(reason) = err.attention_reason() {
                        let _ = record_attention(root, reason);
                    } else if initial_scan {
                        state.scan_paused = true;
                        state.phase = "scan_paused".to_owned();
                        state.scan_checked = cache.count()?;
                        let _ = store::write_state(root, &state);
                    }
                    return Err(err);
                }
            };
            let is_last = hi >= highest_uid;
            cache.put_page(&mb.name, mb.uid_validity, hi, mb.exists, is_last, &rows)?;
            let checked = cache.count()?;
            state.scan_checked = checked;
            state.scan_total = u64::from(mb.exists).max(checked);
            for h in &rows {
                if h.is_sent() && !h.is_draft() {
                    state.scan_sent_found += 1;
                }
            }
            store::write_state(root, &state)?;
            lo = hi.saturating_add(1);
        }
    }

    // Recompute owner aliases and selection decisions across the full header index.
    let all_headers = cache.all()?;
    let mut aliases = learn_owner_aliases(&account.account, &all_headers);
    for prev in &account.owner_aliases {
        if !aliases.iter().any(|a| a.eq_ignore_ascii_case(prev)) {
            aliases.push(prev.clone());
        }
    }
    aliases.sort();
    account.owner_aliases = aliases;
    account.mailbox = mb.name.clone();
    store::write_account(root, &account)?;

    let existing_decisions = store::read_decisions(root)?;
    let existing_map = store::decision_map(&existing_decisions);
    let landed = store::read_landed(root)?;
    let landed_keys = store::landed_key_set(&landed);

    let decisions = evaluate_decisions(
        &account.account,
        &account.owner_aliases,
        &all_headers,
        &existing_map,
        &landed_keys,
        rules_version,
        stale_before_version,
    );
    store::write_decisions(root, &decisions)?;

    let eligible_sent = decisions.iter().filter(|d| d.outcome == "selected").count();
    let left_out_total = decisions.iter().filter(|d| d.outcome == "left_out").count();
    let sent_found = eligible_sent + left_out_total;
    let earliest = earliest_sent_month(&decisions, zone);

    state.scan_complete = true;
    state.scan_paused = false;
    state.scan_checked = all_headers.len() as u64;
    state.scan_total = u64::from(mb.exists).max(all_headers.len() as u64);
    state.scan_sent_found = sent_found;
    state.scan_eligible_sent = eligible_sent;
    state.scan_earliest_month = earliest.clone();
    state.folder_cap_warning = folder_cap_warning;
    state.last_checked_at = Some(now.to_string());
    if !state.fetching_active {
        state.phase = if store::read_choice(root)?.is_some() {
            "steady".to_owned()
        } else {
            "awaiting_choice".to_owned()
        };
    }
    store::write_state(root, &state)?;

    Ok(ScanReport {
        mailbox: mb.name,
        checked: state.scan_checked,
        total: state.scan_total,
        sent_found,
        eligible_sent,
        left_out_total,
        complete: true,
        earliest_month: earliest,
        folder_cap_warning,
        wire: source.wire_audit(),
    })
}

/// Persist the owner's selective-sync choice (`range` + `types`) and return the
/// updated choice and its live preview delta.
pub fn save_choice(
    root: &Path,
    range: RangePreset,
    types: Vec<MessageType>,
    now: Timestamp,
    zone: &TimeZone,
) -> Result<(Choice, ChoicePreview)> {
    if types.is_empty() {
        return Err(EmailError::InvalidInput(
            "choose at least one type of message.".into(),
        ));
    }
    let mut deduped = Vec::new();
    for t in types {
        if !deduped.contains(&t) {
            deduped.push(t);
        }
    }
    let prev = store::read_choice(root)?;
    let since_ms = match range {
        RangePreset::All | RangePreset::None => None,
        preset => {
            let computed = range_cutoff_ms(preset, now, zone);
            // If keeping the exact same range preset while updating types,
            // keep the earlier cutoff so already-chosen boundary stays stable.
            match (&prev, computed) {
                (Some(p), Some(c)) if p.range == preset => {
                    Some(p.since_ms.map_or(c, |prev_ms| prev_ms.min(c)))
                }
                _ => computed,
            }
        }
    };
    let confirmed_at_ms = prev
        .as_ref()
        .map_or_else(|| now.as_millisecond(), |p| p.confirmed_at_ms);

    let choice = Choice {
        range,
        types: deduped.clone(),
        since_ms,
        confirmed_at_ms,
        updated_at: now.to_string(),
    };
    store::write_choice(root, &choice)?;

    let decisions = store::read_decisions(root)?;
    let landed = store::read_landed(root)?;
    let landed_keys = store::landed_key_set(&landed);
    let state = store::read_state(root)?;
    let preview = preview_choice(
        &decisions,
        &landed_keys,
        range,
        &deduped,
        now,
        zone,
        state.folder_cap_warning,
    );

    let mut next_state = state;
    if next_state.phase == "awaiting_choice" {
        next_state.phase = if preview.delta_messages > 0 {
            "fetching".to_owned()
        } else {
            "steady".to_owned()
        };
        next_state.fetch_total = preview.delta_messages;
        next_state.fetch_done = 0;
        store::write_state(root, &next_state)?;
    }

    Ok((choice, preview))
}

/// Preview a candidate `(range, types)` selection directly from persisted
/// header decisions (zero bodies read).
pub fn choice_preview(
    root: &Path,
    range: RangePreset,
    types: &[MessageType],
    now: Timestamp,
    zone: &TimeZone,
) -> Result<ChoicePreview> {
    let decisions = store::read_decisions(root)?;
    let landed = store::read_landed(root)?;
    let landed_keys = store::landed_key_set(&landed);
    let state = store::read_state(root)?;
    Ok(preview_choice(
        &decisions,
        &landed_keys,
        range,
        types,
        now,
        zone,
        state.folder_cap_warning,
    ))
}

/// Execute one sync pass: incremental header catch-up, versioned re-reach for
/// stale skips, newest-first batched fetch of chosen unlanded exchanges,
/// extraction, atomic landing into `chronicle/<day>/import.email/<key>/`, and
/// immediate transient body cleanup per batch.
pub fn run_sync(
    root: &Path,
    source: &mut dyn ImapSource,
    now: Timestamp,
    zone: &TimeZone,
    opts: SyncOptions,
) -> Result<SyncReport> {
    run_sync_with_publisher(root, source, now, zone, opts, None)
}

/// Callback invoked after each landed batch of email segments so callers can
/// publish newly written segments into the stream record, search indexer, and
/// supervisor event bus.
pub type BatchPublishHook<'a> = &'a dyn Fn(&str, &[CreatedEmailSegment]);

/// Same as [`run_sync`], invoking `on_batch` after each landed batch so callers
/// can publish newly created segments into the stream record, search indexer,
/// and supervisor event bus while a long sync is still running.
pub fn run_sync_with_publisher(
    root: &Path,
    source: &mut dyn ImapSource,
    now: Timestamp,
    zone: &TimeZone,
    opts: SyncOptions,
    on_batch: Option<BatchPublishHook<'_>>,
) -> Result<SyncReport> {
    let _lock = if opts.lock_held {
        None
    } else {
        Some(store::hold_sync_lock(root)?)
    };
    let _bodies_guard = BodiesCleanup(root);
    let rules_version = opts.rules_version.unwrap_or(RULES_VERSION);
    let stale_before = opts
        .stale_before_version
        .unwrap_or(STALE_SKIP_BEFORE_VERSION);

    // Step 1: catch up headers + run re-reach over stale skips.
    let scan =
        run_header_scan_with_versions(root, source, now, zone, true, rules_version, stale_before)?;
    let Some(choice) = store::read_choice(root)? else {
        return Ok(SyncReport {
            run_id: None,
            scanned: scan.checked,
            landed_messages: 0,
            landed_segments: 0,
            skipped_already_landed: 0,
            missing_in_mailbox: 0,
            wire: source.wire_audit(),
            created_segments: Vec::new(),
        });
    };
    let account = store::read_account(root)?.ok_or(EmailError::NotConnected)?;
    let decisions = store::read_decisions(root)?;
    let mut landed = store::read_landed(root)?;
    let mut landed_keys = store::landed_key_set(&landed);

    let mut skipped_already_landed = 0usize;
    let mut pending: Vec<Decision> = Vec::new();
    for d in decisions {
        if !choice.includes_decision(&d) {
            continue;
        }
        if landed_keys.contains(&d.key) {
            skipped_already_landed += 1;
            continue;
        }
        pending.push(d);
    }
    // Newest first so recent days land before older history.
    pending.sort_by(|a, b| b.sent_ms.cmp(&a.sent_ms).then_with(|| a.key.cmp(&b.key)));
    if let Some(max) = opts.max_messages {
        pending.truncate(max);
    }

    if pending.is_empty() {
        let mut state = store::read_state(root)?;
        state.fetching_active = false;
        state.fetch_done = 0;
        state.fetch_total = 0;
        state.phase = "steady".to_owned();
        state.last_checked_at = Some(now.to_string());
        store::write_state(root, &state)?;
        return Ok(SyncReport {
            run_id: None,
            scanned: scan.checked,
            landed_messages: 0,
            landed_segments: 0,
            skipped_already_landed,
            missing_in_mailbox: 0,
            wire: source.wire_audit(),
            created_segments: Vec::new(),
        });
    }

    let cache = HeaderCache::open(&store::header_cache_path(root))?;
    let all_headers = cache.all()?;
    let by_key: HashMap<&str, &Header> = all_headers.iter().map(|h| (h.key.as_str(), h)).collect();
    let mut by_msg_id: HashMap<&str, Vec<&Header>> = HashMap::new();
    for h in &all_headers {
        if let Some(id) = h.message_id.as_deref() {
            by_msg_id.entry(id).or_default().push(h);
        }
    }

    let total_to_fetch = pending.len();
    let run_id = make_run_id(&account.account, now, total_to_fetch);
    let mut runs = store::read_runs(root)?;
    let mut run_record = SyncRunRecord {
        id: run_id.clone(),
        synced_at: now.to_string(),
        range_label: Some(choice.range),
        messages: 0,
        segments: Vec::new(),
    };

    let mut state = store::read_state(root)?;
    state.fetching_active = true;
    state.fetch_done = 0;
    state.fetch_total = total_to_fetch;
    state.phase = "fetching".to_owned();
    store::write_state(root, &state)?;

    let mut total_landed = 0usize;
    let mut total_segments = 0usize;
    let mut missing_in_mailbox = 0usize;
    let mut owner_plain_samples: Vec<String> = Vec::new();
    let mut all_created_segments: Vec<CreatedEmailSegment> = Vec::new();

    for batch in pending.chunks(FETCH_BATCH_SIZE) {
        if store::is_cancel_requested(root) {
            state.fetching_active = false;
            state.phase = "steady".to_owned();
            store::write_state(root, &state)?;
            return Err(EmailError::Cancelled);
        }

        // Gather all unique keys (selected owner messages + their certain parents)
        // needed for this batch.
        let mut needed_keys = BTreeSet::new();
        for d in batch {
            needed_keys.insert(d.key.clone());
            if let Some(pk) = &d.parent.parent_key {
                needed_keys.insert(pk.clone());
            }
        }

        let mut plan: Vec<(&Header, BodyChoice)> = Vec::new();
        let mut fetch_reqs: Vec<(&Header, &Part)> = Vec::new();
        for key in &needed_keys {
            if let Some(&h) = by_key.get(key.as_str()) {
                plan.push((h, choose_body(&h.tree)));
            }
        }
        for (h, body_choice) in &plan {
            if let Some(p) = &body_choice.plain {
                fetch_reqs.push((h, p));
            }
            if let Some(p) = &body_choice.html {
                fetch_reqs.push((h, p));
            }
        }

        let fetched_parts = match source.fetch_texts(&fetch_reqs) {
            Ok(v) => v,
            Err(err) => {
                if let Some(reason) = err.attention_reason() {
                    let _ = record_attention(root, reason);
                } else {
                    state.fetching_active = false;
                    let _ = store::write_state(root, &state);
                }
                return Err(err);
            }
        };

        let mut bodies: HashMap<String, FetchedBody> = HashMap::new();
        let mut cursor_idx = 0usize;
        for (h, body_choice) in &plan {
            let mut fb = FetchedBody::default();
            if let Some(p) = &body_choice.plain {
                fb.plain = fetched_parts.get(cursor_idx).cloned().flatten();
                if p.format
                    .as_deref()
                    .is_some_and(|f| f.eq_ignore_ascii_case("flowed"))
                {
                    fb.flowed = Some(
                        p.delsp
                            .as_deref()
                            .is_some_and(|d| d.eq_ignore_ascii_case("yes")),
                    );
                }
                cursor_idx += 1;
            }
            if body_choice.html.is_some() {
                fb.html = fetched_parts.get(cursor_idx).cloned().flatten();
                cursor_idx += 1;
            }
            let had_requested_text = body_choice.plain.is_some() || body_choice.html.is_some();
            fb.missing_from_server = had_requested_text && fb.plain.is_none() && fb.html.is_none();
            bodies.insert(h.key.clone(), fb);
        }

        // Accumulate recurring signature samples across owner messages.
        for d in batch {
            if let Some(fb) = bodies.get(&d.key)
                && let Some(p) = &fb.plain
            {
                let unflowed = match fb.flowed {
                    Some(delsp) => extract::unflow(p, delsp),
                    None => p.clone(),
                };
                owner_plain_samples.push(unflowed);
            }
        }
        let recurring = extract::recurring_signatures(&owner_plain_samples);

        // Extract each exchange in this batch.
        let mut prepared: Vec<PreparedExchange> = Vec::new();
        for d in batch {
            let Some(&owner_hdr) = by_key.get(d.key.as_str()) else {
                missing_in_mailbox += 1;
                continue;
            };
            let Some(owner_body) = bodies.get(&d.key) else {
                missing_in_mailbox += 1;
                continue;
            };
            if owner_body.missing_from_server {
                missing_in_mailbox += 1;
                continue;
            }

            let parent_hdr = d
                .parent
                .parent_key
                .as_deref()
                .and_then(|pk| by_key.get(pk).copied());
            let parent_evidence = d
                .parent
                .parent_key
                .as_deref()
                .and_then(|pk| bodies.get(pk))
                .map(evidence_text);

            let ev = Evidence {
                parent: parent_evidence.as_deref(),
                recurring_signatures: &recurring,
            };
            let is_reply = !owner_hdr.in_reply_to.is_empty() || !owner_hdr.references.is_empty();
            let owner_ex = extract::extract(
                owner_body.plain.as_deref(),
                owner_body.flowed,
                owner_body.html.as_deref(),
                is_reply,
                &ev,
            );

            let reply_to = match (parent_hdr, d.parent.parent_key.as_deref()) {
                (Some(ph), Some(pk)) => {
                    bodies.get(pk).filter(|b| !b.missing_from_server).map(|pb| {
                        let gp_key = crate::select::resolve_parent(ph, &by_msg_id).parent_key;
                        let gp_evidence = gp_key
                            .as_deref()
                            .and_then(|g| bodies.get(g))
                            .map(evidence_text);
                        let pev = Evidence {
                            parent: gp_evidence.as_deref(),
                            recurring_signatures: &recurring,
                        };
                        let p_is_reply = !ph.in_reply_to.is_empty() || !ph.references.is_empty();
                        let pex = extract::extract(
                            pb.plain.as_deref(),
                            pb.flowed,
                            pb.html.as_deref(),
                            p_is_reply,
                            &pev,
                        );
                        build_reply_to(ph, &pex)
                    })
                }
                _ => None,
            };

            let sent_ms = owner_hdr.when_ms().unwrap_or_else(|| now.as_millisecond());
            prepared.push(PreparedExchange {
                decision: d.clone(),
                owner_hdr: owner_hdr.clone(),
                owner_ex,
                reply_to,
                sent_ms,
            });
        }

        // Group batch exchanges into 5-minute moment windows on the owner's local day.
        let windows = group_moment_windows(prepared, zone);
        let mut batch_created_segments: Vec<CreatedEmailSegment> = Vec::new();
        for (day, base_hms, start_ts, exchanges) in windows {
            let seg_key = allocate_fresh_segment_key(root, &day, &base_hms);
            let seg_rel = format!("{day}/{EMAIL_STREAM}/{seg_key}");
            let seg_dir = root.join("chronicle").join(&seg_rel);
            std::fs::create_dir_all(&seg_dir).map_err(|e| EmailError::Storage(e.to_string()))?;

            let mut jsonl = String::new();
            let header_line = json!({
                "source": "email",
                "provider": account.provider,
                "run_id": run_id,
                "email": {
                    "account": account.account,
                }
            });
            jsonl.push_str(&header_line.to_string());
            jsonl.push('\n');

            let mut batch_landed = Vec::new();
            for ex in &exchanges {
                let row = build_turn_row(ex, start_ts, &account.provider);
                let row_str = row.to_string();
                let digest = hex_sha256(row_str.as_bytes());
                jsonl.push_str(&row_str);
                jsonl.push('\n');
                batch_landed.push(LandedRecord {
                    key: ex.decision.key.clone(),
                    run_id: run_id.clone(),
                    day: day.clone(),
                    segment: seg_key.clone(),
                    segment_rel: seg_rel.clone(),
                    landed_at: now.to_string(),
                    sha256: digest,
                });
            }

            let transcript_path = seg_dir.join("conversation_transcript.jsonl");
            atomic_replace(
                &transcript_path,
                jsonl.as_bytes(),
                AtomicWriteOptions::default(),
            )
            .map_err(|e| EmailError::Storage(e.to_string()))?;

            for rec in batch_landed {
                landed_keys.insert(rec.key.clone());
                landed.push(rec);
                total_landed += 1;
                run_record.messages += 1;
            }
            run_record.segments.push(seg_rel);
            batch_created_segments.push(CreatedEmailSegment {
                day,
                segment: seg_key,
                file: transcript_path,
            });
            total_segments += 1;
        }

        // Persist durable landed ledger + run record + progress after every batch,
        // and immediately discard transient fetched bodies.
        store::write_landed(root, &landed)?;
        upsert_run_record(&mut runs, &run_record);
        store::write_runs(root, &runs)?;
        state.fetch_done = total_landed;
        if total_landed > 0 {
            state.last_added_at = Some(now.to_string());
        }
        store::write_state(root, &state)?;
        store::clear_transient_bodies(root);
        if !batch_created_segments.is_empty() {
            if let Some(publish_batch) = on_batch {
                publish_batch(&run_id, &batch_created_segments);
            }
            all_created_segments.extend(batch_created_segments);
        }
    }

    state.fetching_active = false;
    state.phase = "steady".to_owned();
    state.last_checked_at = Some(now.to_string());
    store::write_state(root, &state)?;

    Ok(SyncReport {
        run_id: (total_landed > 0).then_some(run_id),
        scanned: scan.checked,
        landed_messages: total_landed,
        landed_segments: total_segments,
        skipped_already_landed,
        missing_in_mailbox,
        wire: source.wire_audit(),
        created_segments: all_created_segments,
    })
}

/// Build the exact `/app/sources/api/email` JSON status snapshot.
pub fn status_snapshot(root: &Path, zone: &TimeZone) -> Result<StatusSnapshot> {
    let creds = store::read_credentials(root)?;
    let account = store::read_account(root)?;
    let choice = store::read_choice(root)?;
    let state = store::read_state(root)?;
    let decisions = store::read_decisions(root)?;
    let landed = store::read_landed(root)?;
    let live_map = store::live_landed_map(root, &landed);
    let runs = store::read_runs(root)?;

    let accounting = compute_accounting(&decisions, &live_map, choice.as_ref());
    let connected = creds.is_some();

    let phase = if !connected {
        if account.is_some() || !landed.is_empty() {
            "disconnected".to_owned()
        } else {
            "not_connected".to_owned()
        }
    } else if state.attention_reason.is_some() {
        "needs_attention".to_owned()
    } else if state.fetching_active {
        "fetching".to_owned()
    } else if !state.scan_complete {
        if state.scan_paused {
            "scan_paused".to_owned()
        } else {
            "scanning".to_owned()
        }
    } else if choice.is_none() {
        "awaiting_choice".to_owned()
    } else if !state.phase.is_empty() {
        state.phase.clone()
    } else {
        "steady".to_owned()
    };

    let ui_state = match phase.as_str() {
        "not_connected" => "ready_to_set_up",
        "scanning" | "scan_paused" | "awaiting_choice" => "setting_up",
        "fetching" | "steady" => "on",
        "needs_attention" => "needs_attention",
        "disconnected" => "off",
        _ => "ready_to_set_up",
    };

    let earliest = state
        .scan_earliest_month
        .clone()
        .or_else(|| earliest_sent_month(&decisions, zone));
    let eligible_sent = decisions.iter().filter(|d| d.outcome == "selected").count();
    let sent_found = if accounting.sent_total > 0 {
        accounting.sent_total
    } else {
        state.scan_sent_found
    };

    // Build per-run summaries with live segment / message counts so already-trashed
    // runs reflect their current Trash status immediately.
    let mut run_live_msgs: HashMap<&str, usize> = HashMap::new();
    for rec in &landed {
        if live_map.get(&rec.key).copied().unwrap_or(false) {
            *run_live_msgs.entry(rec.run_id.as_str()).or_default() += 1;
        }
    }
    let mut run_snapshots: Vec<RunSnapshot> = runs
        .iter()
        .rev()
        .filter_map(|r| {
            let live_segments = r
                .segments
                .iter()
                .filter(|seg_rel| {
                    let dir = root.join("chronicle").join(seg_rel);
                    dir.join("conversation_transcript.jsonl").is_file()
                        && !dir.join("tombstone.json").is_file()
                })
                .count();
            let live_messages = run_live_msgs.get(r.id.as_str()).copied().unwrap_or(0);
            if live_segments == 0 && live_messages == 0 {
                return None;
            }
            Some(RunSnapshot {
                id: r.id.clone(),
                synced_at: r.synced_at.clone(),
                range_label: r.range_label,
                messages: r.messages,
                live_messages,
                live_segments,
            })
        })
        .collect();
    // Cap returned runs list to most recent 50 runs for bounded responses.
    run_snapshots.truncate(50);

    Ok(StatusSnapshot {
        source: "email",
        state: ui_state,
        phase,
        provider: account
            .as_ref()
            .map_or_else(|| PROVIDER_GMAIL.to_owned(), |a| a.provider.clone()),
        account: account.map(|a| a.account),
        attention_reason: if connected {
            state.attention_reason
        } else {
            None
        },
        connected,
        syncing: state.fetching_active || (connected && !state.scan_complete && !state.scan_paused),
        scan: ScanSnapshot {
            complete: state.scan_complete,
            paused: state.scan_paused,
            checked: state.scan_checked,
            total: state.scan_total,
            sent_found,
            eligible_sent: if eligible_sent > 0 {
                eligible_sent
            } else {
                state.scan_eligible_sent
            },
            earliest_month: earliest,
            bodies_read: 0,
            folder_cap_warning: state.folder_cap_warning,
        },
        choice: choice.map(|c| ChoiceSnapshot {
            covered_ranges: covered_ranges_for(c.range),
            range: c.range,
            types: c.types,
            updated_at: c.updated_at,
        }),
        sync: SyncSnapshot {
            active: state.fetching_active,
            done: state.fetch_done,
            total: state.fetch_total,
            last_added_at: state.last_added_at,
            last_checked_at: state.last_checked_at,
        },
        accounting,
        runs: run_snapshots,
    })
}

// ---------------------------------------------------------------------------
// Internal helpers for extraction, windowing, and segment serialization
// ---------------------------------------------------------------------------

#[derive(Default)]
struct FetchedBody {
    plain: Option<String>,
    html: Option<String>,
    flowed: Option<bool>,
    missing_from_server: bool,
}

fn evidence_text(b: &FetchedBody) -> String {
    match (&b.plain, &b.html) {
        (Some(p), _) if !p.trim().is_empty() => p.clone(),
        (_, Some(h)) => crate::html::render(h),
        _ => String::new(),
    }
}

struct PreparedExchange {
    decision: Decision,
    owner_hdr: Header,
    owner_ex: Extracted,
    reply_to: Option<Value>,
    sent_ms: i64,
}

fn build_reply_to(ph: &Header, pex: &Extracted) -> Value {
    let author = ph
        .from
        .first()
        .map(crate::model::Addr::label)
        .unwrap_or_else(|| "Unknown".to_owned());
    let from_addr = ph.from.first().and_then(|a| a.address.clone());
    let sent_at = ph
        .when_ms()
        .and_then(|ms| Timestamp::from_millisecond(ms).ok())
        .map(|ts| ts.to_string());
    let raw_text = pex.text.trim();
    let capped = if raw_text.is_empty() {
        extract::CappedText {
            text: ANSWERED_NO_TEXT_FALLBACK.to_owned(),
            truncated: false,
            cut_chars: 0,
            cut_at: None,
        }
    } else {
        cap_text(raw_text, CAP_CHARS)
    };
    let mut obj = Map::new();
    obj.insert("author".into(), Value::String(author));
    if let Some(addr) = from_addr {
        obj.insert("from".into(), Value::String(addr));
    }
    if let Some(ts) = sent_at {
        obj.insert("sent_at".into(), Value::String(ts));
    }
    obj.insert("text".into(), Value::String(capped.text));
    if capped.truncated {
        obj.insert("truncated".into(), Value::Bool(true));
        obj.insert("cut_chars".into(), Value::from(capped.cut_chars as u64));
        if let Some(how) = capped.cut_at {
            obj.insert("cut_at".into(), Value::String(how.to_owned()));
        }
    }
    let atts = ph.named_attachments();
    if !atts.is_empty() {
        obj.insert(
            "attachments".into(),
            serde_json::to_value(&atts).unwrap_or(Value::Null),
        );
    }
    Value::Object(obj)
}

fn merge_forwarded(blocks: &[ForwardedBlock]) -> Option<Value> {
    let first = blocks.first()?;
    let mut obj = Map::new();
    if let Some(from) = &first.from {
        obj.insert("from".into(), Value::String(from.clone()));
    }
    if let Some(date) = &first.date {
        obj.insert("date".into(), Value::String(date.clone()));
    }
    if let Some(subject) = &first.subject {
        obj.insert("subject".into(), Value::String(subject.clone()));
    }
    let combined_text = if blocks.len() == 1 {
        first.text.clone()
    } else {
        let joined = blocks
            .iter()
            .map(|b| b.text.as_str())
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n---\n\n");
        cap_text(&joined, CAP_CHARS).text
    };
    obj.insert("text".into(), Value::String(combined_text));
    if first.truncated {
        obj.insert("truncated".into(), Value::Bool(true));
        obj.insert("cut_chars".into(), Value::from(first.cut_chars as u64));
        if let Some(how) = &first.cut_at {
            obj.insert("cut_at".into(), Value::String(how.clone()));
        }
    }
    Some(Value::Object(obj))
}

fn build_turn_row(ex: &PreparedExchange, seg_start: Timestamp, provider: &str) -> Value {
    let h = &ex.owner_hdr;
    let offset_secs = ex.sent_ms.saturating_sub(seg_start.as_millisecond()).max(0) / 1000;
    let start_str = format!(
        "{:02}:{:02}:{:02}",
        offset_secs / 3600,
        (offset_secs % 3600) / 60,
        offset_secs % 60
    );
    let sent_at = Timestamp::from_millisecond(ex.sent_ms)
        .ok()
        .map(|ts| ts.to_string());
    let to_labels: Vec<String> = h.to.iter().map(crate::model::Addr::label).collect();
    let to_addresses: Vec<Option<String>> = h.to.iter().map(|a| a.address.clone()).collect();
    let cc_labels: Vec<String> = h.cc.iter().map(crate::model::Addr::label).collect();
    let atts = h.named_attachments();
    let file_count = atts.iter().filter(|a| !a.inline).count();
    let inline_count = atts.iter().filter(|a| a.inline).count();
    let has_forwarded = !ex.owner_ex.forwarded.is_empty();

    let (turn_text, uncertain_spans) = if ex.owner_ex.text.trim().is_empty() {
        (
            owner_fallback_text(file_count, inline_count, has_forwarded).to_owned(),
            Vec::new(),
        )
    } else {
        (
            ex.owner_ex.text.clone(),
            ex.owner_ex.uncertain_spans.clone(),
        )
    };

    let mut email_meta = Map::new();
    email_meta.insert("provider".into(), Value::String(provider.to_owned()));
    email_meta.insert("key".into(), Value::String(ex.decision.key.clone()));
    if let Some(mid) = &h.message_id {
        email_meta.insert("message_id".into(), Value::String(mid.clone()));
    }
    if let Some(gm) = &h.gm_msgid {
        email_meta.insert("gm_msgid".into(), Value::String(gm.clone()));
    }
    if let Some(gt) = &h.gm_thrid {
        email_meta.insert("gm_thrid".into(), Value::String(gt.clone()));
    }
    if let Some(from_addr) = h.from.first().and_then(|a| a.address.clone()) {
        email_meta.insert("from".into(), Value::String(from_addr));
    }
    if !to_addresses.is_empty() {
        email_meta.insert(
            "to_addresses".into(),
            serde_json::to_value(&to_addresses).unwrap_or(Value::Null),
        );
    }
    if let Some(mt) = ex.decision.message_type {
        email_meta.insert("message_type".into(), Value::String(mt.as_str().to_owned()));
    }
    email_meta.insert(
        "parent_outcome".into(),
        Value::String(ex.decision.parent.outcome.clone()),
    );
    if let Some(rule) = ex.decision.parent.parent_match_rule {
        email_meta.insert(
            "parent_match_rule".into(),
            Value::String(rule.as_str().to_owned()),
        );
    }
    if let Some(reason) = &ex.decision.parent.reason {
        email_meta.insert("parent_reason".into(), Value::String(reason.clone()));
    }
    email_meta.insert(
        "extractor".into(),
        Value::String(ex.owner_ex.extractor.clone()),
    );
    email_meta.insert(
        "rules_version".into(),
        Value::from(ex.decision.rules_version),
    );

    let mut row = Map::new();
    row.insert("start".into(), Value::String(start_str));
    row.insert("source_ms".into(), Value::from(ex.sent_ms));
    if let Some(ts) = sent_at {
        row.insert("sent_at".into(), Value::String(ts));
    }
    row.insert("speaker".into(), Value::String("You".into()));
    row.insert("is_owner".into(), Value::Bool(true));
    if !to_labels.is_empty() {
        row.insert(
            "to".into(),
            serde_json::to_value(&to_labels).unwrap_or(Value::Null),
        );
    }
    if !cc_labels.is_empty() {
        row.insert(
            "cc".into(),
            serde_json::to_value(&cc_labels).unwrap_or(Value::Null),
        );
    }
    if let Some(subj) = h
        .subject
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        row.insert("subject".into(), Value::String(subj.to_owned()));
    }
    row.insert("text".into(), Value::String(turn_text));
    if !uncertain_spans.is_empty() {
        row.insert(
            "uncertain_spans".into(),
            serde_json::to_value(&uncertain_spans).unwrap_or(Value::Null),
        );
    }
    if let Some(fwd) = merge_forwarded(&ex.owner_ex.forwarded) {
        row.insert("forwarded".into(), fwd);
    }
    if let Some(reply) = &ex.reply_to {
        row.insert("reply_to".into(), reply.clone());
    }
    if !atts.is_empty() {
        row.insert(
            "attachments".into(),
            serde_json::to_value(&atts).unwrap_or(Value::Null),
        );
    }
    row.insert("email".into(), Value::Object(email_meta));
    Value::Object(row)
}

fn group_moment_windows(
    mut exchanges: Vec<PreparedExchange>,
    zone: &TimeZone,
) -> Vec<(String, String, Timestamp, Vec<PreparedExchange>)> {
    exchanges.sort_by_key(|e| e.sent_ms);
    let mut windows: Vec<(String, String, Timestamp, Vec<PreparedExchange>)> = Vec::new();
    for ex in exchanges {
        let ts = Timestamp::from_millisecond(ex.sent_ms).unwrap_or(Timestamp::UNIX_EPOCH);
        let z = ts.to_zoned_datetime(zone.clone());
        let day = z.strftime("%Y%m%d").to_string();
        if let Some(last) = windows.last_mut()
            && last.0 == day
            && ts.as_second().saturating_sub(last.2.as_second()) < 300
        {
            last.3.push(ex);
            continue;
        }
        let hms = z.strftime("%H%M%S").to_string();
        windows.push((day, hms, ts, vec![ex]));
    }
    windows
}

fn allocate_fresh_segment_key(root: &Path, day: &str, hms: &str) -> String {
    let stream_dir = root.join("chronicle").join(day).join(EMAIL_STREAM);
    let mut duration = 300u32;
    loop {
        let candidate = format!("{hms}_{duration}");
        if !stream_dir.join(&candidate).exists() {
            return candidate;
        }
        duration = duration.saturating_add(1);
    }
}

fn upsert_run_record(runs: &mut Vec<SyncRunRecord>, updated: &SyncRunRecord) {
    if updated.messages == 0 {
        return;
    }
    if let Some(existing) = runs.iter_mut().find(|r| r.id == updated.id) {
        *existing = updated.clone();
    } else {
        runs.push(updated.clone());
    }
}

fn make_run_id(account: &str, now: Timestamp, count: usize) -> String {
    let stamp = now.strftime("%Y%m%d_%H%M%S");
    let tag = hex_sha256(format!("{account}|{}|{count}", now.as_millisecond()).as_bytes());
    format!("email-{stamp}-{}", &tag[..8])
}

fn hex_sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Build a content-free map of `run_id -> SyncRunRecord` for per-run Trash lookups.
pub fn find_run(root: &Path, run_id: &str) -> Result<Option<SyncRunRecord>> {
    let runs = store::read_runs(root)?;
    Ok(runs.into_iter().find(|r| r.id == run_id))
}

/// Validate a sync run ID before using it in HTTP path lookups.
#[must_use]
pub fn is_valid_run_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id.starts_with("email-")
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Compute segment/message/day breakdown for a single sync run's Trash preview.
#[must_use]
pub fn run_live_segments(root: &Path, run: &SyncRunRecord) -> Vec<String> {
    run.segments
        .iter()
        .filter(|seg_rel| {
            let dir = root.join("chronicle").join(seg_rel);
            dir.join("conversation_transcript.jsonl").is_file()
                && !dir.join("tombstone.json").is_file()
        })
        .cloned()
        .collect()
}

/// Convenience helper for CLI `--sync email --save [--scheduled]` execution over live Gmail.
pub fn sync_live_gmail(root: &Path, scheduled: bool, scan_only: bool) -> Result<SyncReport> {
    sync_live_gmail_with_publisher(root, scheduled, scan_only, None)
}

/// Same as [`sync_live_gmail`], invoking `on_batch` after each landed batch.
pub fn sync_live_gmail_with_publisher(
    root: &Path,
    scheduled: bool,
    scan_only: bool,
    on_batch: Option<BatchPublishHook<'_>>,
) -> Result<SyncReport> {
    let Some(creds) = store::read_credentials(root)? else {
        return Err(EmailError::NotConnected);
    };
    let state = store::read_state(root)?;
    if scheduled && state.attention_reason.is_some() {
        return Ok(SyncReport {
            run_id: None,
            scanned: state.scan_checked,
            landed_messages: 0,
            landed_segments: 0,
            skipped_already_landed: 0,
            missing_in_mailbox: 0,
            wire: WireAudit::default(),
            created_segments: Vec::new(),
        });
    }
    let now = Timestamp::now();
    let zone = store::journal_timezone(root);
    let mut client =
        match crate::imap::GmailImap::connect_gmail(&creds.account, &creds.app_password) {
            Ok(client) => client,
            Err(err) => {
                if let Some(reason) = err.attention_reason() {
                    let _ = record_attention(root, reason);
                }
                return Err(err);
            }
        };
    let outcome = if scan_only {
        let scan = run_header_scan(root, &mut client, now, &zone, false)?;
        Ok(SyncReport {
            run_id: None,
            scanned: scan.checked,
            landed_messages: 0,
            landed_segments: 0,
            skipped_already_landed: 0,
            missing_in_mailbox: 0,
            wire: scan.wire,
            created_segments: Vec::new(),
        })
    } else {
        run_sync_with_publisher(
            root,
            &mut client,
            now,
            &zone,
            SyncOptions::default(),
            on_batch,
        )
    };
    let _ = client.logout();
    outcome
}

/// Summary of a single email sync's landed segments by day (used for Trash preview).
#[must_use]
pub fn run_day_bounds(segments: &[String]) -> Option<(String, String)> {
    let mut days: BTreeSet<&str> = BTreeSet::new();
    for seg in segments {
        if let Some(day) = seg.split('/').next()
            && day.len() == 8
        {
            days.insert(day);
        }
    }
    let first = (*days.first()?).to_owned();
    let last = (*days.last()?).to_owned();
    Some((first, last))
}

/// Count live messages belonging to `run_id` using `config/email/landed.jsonl`.
pub fn run_live_message_count(root: &Path, run_id: &str) -> Result<usize> {
    let landed = store::read_landed(root)?;
    let live: BTreeMap<String, bool> = store::live_landed_map(root, &landed);
    Ok(landed
        .iter()
        .filter(|r| r.run_id == run_id && live.get(&r.key).copied().unwrap_or(false))
        .count())
}
