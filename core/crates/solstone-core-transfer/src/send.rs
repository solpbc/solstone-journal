// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native paired-link journal segment sending.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{Duration as ChronoDuration, NaiveDate};
use serde_json::json;
use solstone_core_journal_io::{PathOrDay, iter_segments};

use crate::TransferError;
use crate::export::hash_file;
use crate::manifest::{ManifestFile, SegmentManifest};
use crate::peer::{MultipartFile, PeerLoopbackClient, multipart_body, with_peer_bridge};
pub use crate::peer::{ResolvedPeer, resolve_peer};

/// Segment control files that must never be included in a journal upload.
pub const RESERVED_SEGMENT_FILENAMES: [&str; 3] =
    ["stream.json", "ingest.json", "ingest.json.lock"];

const RETRY_BACKOFF: [Duration; 3] = [
    Duration::from_secs(1),
    Duration::from_secs(5),
    Duration::from_secs(15),
];

/// Input to [`send`].
#[derive(Debug, Clone)]
pub struct SendRequest {
    /// Paired peer's human-facing label.
    pub to: String,
    /// One day, an inclusive day range, or all existing chronicle days.
    pub day: Option<String>,
    /// Count changed segments without posting any uploads.
    pub dry_run: bool,
}

/// Terminal outcome for a completed send operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendTerminal {
    /// All applicable segments were considered.
    Complete,
    /// The paired-link identity was missing or invalid at the remote journal.
    AuthenticationInvalid,
    /// The paired-link identity was revoked or disabled at the remote journal.
    AuthenticationRevoked,
}

/// Counts accumulated by a send operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendReport {
    /// Segments sent, or segments that would be sent in dry-run mode.
    pub sent: usize,
    /// Segments already synchronized or containing only reserved files.
    pub skipped: usize,
    /// Segments rejected without a retryable failure.
    pub failed: usize,
    /// Total uploaded regular-file bytes.
    pub bytes_transferred: u64,
    /// Whether the operation stopped normally or on an authorization response.
    pub terminal: SendTerminal,
}

/// Send changed local segments to a paired peer.
pub fn send(journal: &Path, request: SendRequest) -> Result<SendReport, TransferError> {
    let days = parse_day_spec(request.day.as_deref(), journal)?;
    let peer = resolve_peer(journal, &request.to)?;
    with_peer_bridge(journal, &peer, |loopback| {
        send_over_bridge(journal, &peer, &days, request.dry_run, loopback)
    })
}

/// Parse the Python send command's day selector.
pub fn parse_day_spec(spec: Option<&str>, journal: &Path) -> Result<Vec<String>, TransferError> {
    let Some(spec) = spec else {
        let chronicle = journal.join("chronicle");
        let day_root = if chronicle.is_dir() {
            chronicle
        } else {
            journal.into()
        };
        let mut days = fs::read_dir(day_root)?
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .filter_map(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                (entry.path().is_dir() && is_eight_digit_day(&name)).then_some(name)
            })
            .collect::<Vec<_>>();
        days.sort();
        return Ok(days);
    };
    if is_eight_digit_day(spec) {
        return Ok(vec![spec.to_string()]);
    }
    let Some((start, end)) = spec.split_once('-') else {
        return Err(TransferError::InvalidDay);
    };
    if !is_eight_digit_day(start) || !is_eight_digit_day(end) || end.contains('-') {
        return Err(TransferError::InvalidDay);
    }
    let start = parse_calendar_day(start)?;
    let end = parse_calendar_day(end)?;
    if start > end {
        return Err(TransferError::InvalidDay);
    }
    let mut days = Vec::new();
    let mut current = start;
    while current <= end {
        days.push(current.format("%Y%m%d").to_string());
        current += ChronoDuration::days(1);
    }
    Ok(days)
}

fn send_over_bridge(
    journal: &Path,
    peer: &ResolvedPeer,
    days: &[String],
    dry_run: bool,
    loopback: &PeerLoopbackClient,
) -> Result<SendReport, TransferError> {
    let key_prefix: String = peer.instance_id.chars().take(8).collect();
    let manifest_path = format!("/app/import/journal/{key_prefix}/manifest/segments");
    let remote_manifest = query_remote_manifest(loopback, &manifest_path);
    let mut report = SendReport {
        sent: 0,
        skipped: 0,
        failed: 0,
        bytes_transferred: 0,
        terminal: SendTerminal::Complete,
    };
    for day in days {
        for segment in iter_segments(journal, PathOrDay::Day(day))? {
            let files = segment_files(&segment.path)?;
            if files.is_empty() {
                report.skipped += 1;
                continue;
            }
            let local_manifest = SegmentManifest {
                files: files
                    .iter()
                    .map(|file| {
                        Ok(ManifestFile {
                            name: file
                                .file_name()
                                .and_then(|name| name.to_str())
                                .unwrap_or_default()
                                .to_string(),
                            sha256: hash_file(file)?.0,
                            size: fs::metadata(file)?.len(),
                        })
                    })
                    .collect::<Result<Vec<_>, TransferError>>()?,
            };
            let route = format!("{}/{}", segment.stream, segment.key);
            let remote = remote_manifest
                .get(day)
                .and_then(|segments| segments.get(&route));
            if manifests_match(&local_manifest, remote) {
                report.skipped += 1;
                continue;
            }
            if dry_run {
                report.sent += 1;
                continue;
            }
            let path = crate::manifest::segment_ingest_path(&key_prefix);
            match upload_segment(loopback, &path, day, &segment.stream, &segment.key, &files)? {
                UploadOutcome::Sent(bytes) => {
                    report.sent += 1;
                    report.bytes_transferred += bytes;
                }
                UploadOutcome::Failed => report.failed += 1,
                UploadOutcome::AuthenticationInvalid => {
                    report.terminal = SendTerminal::AuthenticationInvalid;
                    return Ok(report);
                }
                UploadOutcome::AuthenticationRevoked => {
                    report.terminal = SendTerminal::AuthenticationRevoked;
                    return Ok(report);
                }
            }
        }
    }
    Ok(report)
}

fn query_remote_manifest(
    loopback: &PeerLoopbackClient,
    path: &str,
) -> BTreeMap<String, BTreeMap<String, SegmentManifest>> {
    loopback
        .get(path)
        .ok()
        .filter(|response| response.status == 200)
        .and_then(|response| serde_json::from_slice(&response.body).ok())
        .unwrap_or_default()
}

enum UploadOutcome {
    Sent(u64),
    Failed,
    AuthenticationInvalid,
    AuthenticationRevoked,
}

fn upload_segment(
    loopback: &PeerLoopbackClient,
    path: &str,
    day: &str,
    stream: &str,
    key: &str,
    files: &[PathBuf],
) -> Result<UploadOutcome, TransferError> {
    let bytes = files
        .iter()
        .map(|file| fs::metadata(file).map(|metadata| metadata.len()))
        .sum::<Result<u64, _>>()?;
    for (attempt, delay) in RETRY_BACKOFF.iter().enumerate() {
        let files = files
            .iter()
            .map(|file| {
                Ok(MultipartFile {
                    field_name: "files_0".to_string(),
                    file_name: file
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or_default()
                        .to_string(),
                    contents: fs::read(file)?,
                })
            })
            .collect::<Result<Vec<_>, std::io::Error>>()?;
        let metadata = json!({"segments": [{"day": day, "stream": stream, "segment_key": key, "files": files.iter().map(|file| file.file_name.clone()).collect::<Vec<_>>() }]}).to_string();
        let (body, boundary) = multipart_body(&metadata, &files);
        match loopback.post(
            path,
            &format!("multipart/form-data; boundary={boundary}"),
            body,
        ) {
            Ok(response) => match response.status {
                200 => return Ok(UploadOutcome::Sent(bytes)),
                401 => return Ok(UploadOutcome::AuthenticationInvalid),
                403 => return Ok(UploadOutcome::AuthenticationRevoked),
                500..=599 if attempt + 1 < RETRY_BACKOFF.len() => std::thread::sleep(*delay),
                _ => return Ok(UploadOutcome::Failed),
            },
            Err(_) if attempt + 1 < RETRY_BACKOFF.len() => std::thread::sleep(*delay),
            Err(_) => return Ok(UploadOutcome::Failed),
        }
    }
    Ok(UploadOutcome::Failed)
}

fn segment_files(segment: &Path) -> Result<Vec<PathBuf>, TransferError> {
    let mut files = fs::read_dir(segment)?
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .filter_map(|entry| {
            let path = entry.path();
            (path.is_file()
                && !RESERVED_SEGMENT_FILENAMES
                    .contains(&entry.file_name().to_string_lossy().as_ref()))
            .then_some(path)
        })
        .collect::<Vec<_>>();
    files.sort();
    Ok(files)
}

fn manifests_match(local: &SegmentManifest, remote: Option<&SegmentManifest>) -> bool {
    let local = local
        .files
        .iter()
        .map(|file| (&file.name, &file.sha256))
        .collect::<BTreeMap<_, _>>();
    let remote = remote
        .map(|manifest| {
            manifest
                .files
                .iter()
                .map(|file| (&file.name, &file.sha256))
                .collect::<BTreeMap<_, _>>()
        })
        .unwrap_or_default();
    local == remote
}

fn is_eight_digit_day(value: &str) -> bool {
    value.len() == 8 && value.bytes().all(|byte| byte.is_ascii_digit())
}
fn parse_calendar_day(value: &str) -> Result<NaiveDate, TransferError> {
    NaiveDate::parse_from_str(value, "%Y%m%d").map_err(|_| TransferError::InvalidDay)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;

    #[test]
    fn day_parser_matches_python_shapes() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("chronicle/20260203")).unwrap();
        fs::create_dir_all(root.path().join("chronicle/invalid")).unwrap();
        assert_eq!(parse_day_spec(None, root.path()).unwrap(), ["20260203"]);
        assert_eq!(
            parse_day_spec(Some("20260230"), root.path()).unwrap(),
            ["20260230"]
        );
        assert_eq!(
            parse_day_spec(Some("20260227-20260301"), root.path()).unwrap(),
            ["20260227", "20260228", "20260301"]
        );
        assert!(matches!(
            parse_day_spec(Some("20260230-20260301"), root.path()),
            Err(TransferError::InvalidDay)
        ));
        assert!(matches!(
            parse_day_spec(Some("20260301-20260228"), root.path()),
            Err(TransferError::InvalidDay)
        ));
    }

    #[test]
    fn peer_resolver_reports_python_compatible_messages() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(
            resolve_peer(root.path(), "office").unwrap_err().to_string(),
            "no peers paired (run \"sol link join --as peer\" first)"
        );
        let peers = root.path().join("peers");
        for (directory, label, instance_id) in [
            ("z", "zebra", "second"),
            ("a", "alpha", "first"),
            ("b", "alpha", "third"),
        ] {
            let dir = peers.join(directory);
            fs::create_dir_all(&dir).unwrap();
            fs::write(
                dir.join("peer.json"),
                json!({"label": label, "instance_id": instance_id}).to_string(),
            )
            .unwrap();
        }
        assert_eq!(
            resolve_peer(root.path(), "none").unwrap_err().to_string(),
            "no peer with label \"none\"; available: alpha, zebra"
        );
        assert_eq!(
            resolve_peer(root.path(), "alpha").unwrap_err().to_string(),
            "multiple peers with label \"alpha\": first, third; use <journal_root>/peers/<instance_id> directly"
        );
    }

    #[test]
    fn reserved_files_are_excluded() {
        let root = tempfile::tempdir().unwrap();
        for name in RESERVED_SEGMENT_FILENAMES {
            fs::write(root.path().join(name), b"control").unwrap();
        }
        fs::write(root.path().join("payload.json"), b"payload").unwrap();
        assert_eq!(
            segment_files(root.path()).unwrap(),
            [root.path().join("payload.json")]
        );
    }
}
