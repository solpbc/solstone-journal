// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! RFC 9110 byte-range parsing (`u64`), `If-Range` strong-ETag matching, and
//! bounded-memory streaming over a prepared immutable ZIP file.

use std::io::SeekFrom;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use axum::body::Bytes;
use axum::http::HeaderMap;
use futures_core::Stream;
use tokio::io::{AsyncRead, AsyncSeekExt, ReadBuf};

/// Chunk size read per poll from the staged export file.
const STREAM_CHUNK_BYTES: usize = 64 * 1024;

/// Minimum interval (seconds) between rolling-activity `ready.json` writes
/// during an active continuous transfer.
pub(crate) const ACTIVITY_TOUCH_INTERVAL_SECS: i64 = 30;

/// Minimum interval (seconds) between on-disk revocation and Trash erasure
/// epoch re-reads during an active continuous transfer. Kept short (2 s) so
/// pairing revocation and permanent Trash erasure interrupt in-flight transfers
/// promptly while avoiding per-chunk (64 KiB) disk reads; in-process signals
/// (set by `reconcile_ready_state`) still stop streams immediately at zero I/O.
pub(crate) const POLICY_RECHECK_INTERVAL_SECS: i64 = 2;

/// Result of evaluating `Range` + `If-Range` against a fixed `total_len`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RangeDecision {
    /// Serve the entire file (`200 OK`, `0..total_len`).
    Full,
    /// Serve an inclusive byte range (`206 Partial Content`, `start..=end`).
    Partial { start: u64, end: u64 },
    /// Range cannot be satisfied (`416 Range Not Satisfiable`, `Content-Range: bytes */total_len`).
    Unsatisfiable,
}

/// Evaluate RFC 9110 `Range` and `If-Range` headers against `total_len` and `strong_etag`.
///
/// Only strong quoted entity-tags (`"..."`) are matched for `If-Range` (weak
/// tags `W/"..."` and HTTP-dates never satisfy a strong validator match, falling
/// back to `200 OK` full representation per RFC 9110 §13.1.5).
pub(crate) fn evaluate_range(
    headers: &HeaderMap,
    total_len: u64,
    strong_etag: &str,
) -> RangeDecision {
    let Some(range_header) = headers.get(axum::http::header::RANGE) else {
        return RangeDecision::Full;
    };
    let Ok(range_str) = range_header.to_str() else {
        return RangeDecision::Full;
    };
    if let Some(if_range) = headers.get(axum::http::header::IF_RANGE) {
        let matches = if_range
            .to_str()
            .ok()
            .map(str::trim)
            .is_some_and(|val| !val.starts_with("W/") && val == strong_etag);
        if !matches {
            return RangeDecision::Full;
        }
    }
    parse_single_byte_range(range_str.trim(), total_len)
}

/// Parse a single `bytes=...` range spec with 64-bit offsets.
///
/// Multiple comma-separated ranges fall back to `Full` (RFC 9110 §14.2 allows
/// ignoring multi-range requests and returning `200 OK`, avoiding multipart
/// framing complexity). Malformed syntax falls back to `Full`; syntactically
/// valid out-of-bounds ranges return `Unsatisfiable` (`416`).
pub(crate) fn parse_single_byte_range(spec: &str, total_len: u64) -> RangeDecision {
    let Some(rest) = spec.strip_prefix("bytes=") else {
        return RangeDecision::Full;
    };
    let rest = rest.trim();
    if rest.is_empty() || rest.contains(',') {
        return RangeDecision::Full;
    }
    let Some((start_raw, end_raw)) = rest.split_once('-') else {
        return RangeDecision::Full;
    };
    let start_raw = start_raw.trim();
    let end_raw = end_raw.trim();

    if start_raw.is_empty() {
        // Suffix byte range: `bytes=-N`
        let Ok(suffix) = end_raw.parse::<u64>() else {
            return RangeDecision::Full;
        };
        if suffix == 0 || total_len == 0 {
            return RangeDecision::Unsatisfiable;
        }
        let take = suffix.min(total_len);
        return RangeDecision::Partial {
            start: total_len - take,
            end: total_len - 1,
        };
    }

    let Ok(start) = start_raw.parse::<u64>() else {
        return RangeDecision::Full;
    };
    if total_len == 0 || start >= total_len {
        return RangeDecision::Unsatisfiable;
    }
    if end_raw.is_empty() {
        return RangeDecision::Partial {
            start,
            end: total_len - 1,
        };
    }
    let Ok(end) = end_raw.parse::<u64>() else {
        return RangeDecision::Full;
    };
    if end < start {
        return RangeDecision::Full;
    }
    RangeDecision::Partial {
        start,
        end: end.min(total_len - 1),
    }
}

/// Reason an active HTTP download stream must stop delivering further bytes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TransferAbortReason {
    Revoked,
    Erased,
}

/// Callback interface connecting an active HTTP download stream to `ExportService`.
pub(crate) trait ActiveTransferHooks: Send + Sync + 'static {
    /// Current clock timestamp in epoch seconds (matches `ExportClock`).
    fn now_epoch_secs(&self) -> i64;
    /// Refresh rolling inactivity clock during a long continuous transfer
    /// (throttled to `ACTIVITY_TOUCH_INTERVAL_SECS`).
    fn touch_activity(&self);
    /// Zero-I/O in-memory check for revocation or Trash permanent erasure.
    fn check_abort_fast(&self) -> Option<TransferAbortReason>;
    /// On-disk re-check for revocation and Trash permanent erasure (throttled
    /// to `POLICY_RECHECK_INTERVAL_SECS`).
    fn check_abort_disk(&self) -> Option<TransferAbortReason>;
    /// Decrement `active_downloads`; if the copy was retired by Trash erasure
    /// or revocation and just became idle (`active_downloads == 0`), delete its
    /// staged files immediately.
    fn release_active_download(&self);
}

/// RAII guard guaranteeing `release_active_download` runs on normal completion,
/// client disconnect, error, or panic.
pub(crate) struct ActiveDownloadGuard {
    hooks: Arc<dyn ActiveTransferHooks>,
}

impl ActiveDownloadGuard {
    pub(crate) fn new(hooks: Arc<dyn ActiveTransferHooks>) -> Self {
        Self { hooks }
    }
}

impl Drop for ActiveDownloadGuard {
    fn drop(&mut self) {
        self.hooks.release_active_download();
    }
}

/// Bounded-memory byte-range stream over an already-opened immutable file.
pub(crate) struct TrackedRangeStream {
    file: tokio::fs::File,
    remaining: u64,
    last_activity_touch_secs: i64,
    last_policy_check_secs: i64,
    buffer: Vec<u8>,
    guard: Option<ActiveDownloadGuard>,
}

impl TrackedRangeStream {
    pub(crate) async fn open(
        mut file: tokio::fs::File,
        start: u64,
        length: u64,
        guard: ActiveDownloadGuard,
    ) -> std::io::Result<Self> {
        if start > 0 {
            file.seek(SeekFrom::Start(start)).await?;
        }
        let now = guard.hooks.now_epoch_secs();
        Ok(Self {
            file,
            remaining: length,
            last_activity_touch_secs: now,
            last_policy_check_secs: now,
            buffer: vec![0u8; STREAM_CHUNK_BYTES],
            guard: Some(guard),
        })
    }
}

impl Stream for TrackedRangeStream {
    type Item = Result<Bytes, std::io::Error>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        if this.remaining == 0 {
            this.guard.take();
            return Poll::Ready(None);
        }
        if let Some(guard) = this.guard.as_ref() {
            let mut abort = guard.hooks.check_abort_fast();
            let now = guard.hooks.now_epoch_secs();
            if abort.is_none()
                && now.saturating_sub(this.last_policy_check_secs) >= POLICY_RECHECK_INTERVAL_SECS
            {
                this.last_policy_check_secs = now;
                abort = guard.hooks.check_abort_disk();
            }
            if let Some(reason) = abort {
                this.guard.take();
                let msg = match reason {
                    TransferAbortReason::Revoked => "export requester authorization revoked",
                    TransferAbortReason::Erased => "export invalidated by permanent trash erasure",
                };
                return Poll::Ready(Some(Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    msg,
                ))));
            }
        }

        let to_read = (this.remaining.min(STREAM_CHUNK_BYTES as u64)) as usize;
        let mut read_buf = ReadBuf::new(&mut this.buffer[..to_read]);
        match Pin::new(&mut this.file).poll_read(cx, &mut read_buf) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Err(error)) => {
                this.guard.take();
                Poll::Ready(Some(Err(error)))
            }
            Poll::Ready(Ok(())) => {
                let filled = read_buf.filled();
                if filled.is_empty() {
                    this.guard.take();
                    return Poll::Ready(Some(Err(std::io::Error::new(
                        std::io::ErrorKind::UnexpectedEof,
                        "staged export file ended before declared length",
                    ))));
                }
                let n = filled.len() as u64;
                this.remaining = this.remaining.saturating_sub(n);
                if let Some(guard) = this.guard.as_ref() {
                    let now = guard.hooks.now_epoch_secs();
                    if now.saturating_sub(this.last_activity_touch_secs)
                        >= ACTIVITY_TOUCH_INTERVAL_SECS
                    {
                        this.last_activity_touch_secs = now;
                        guard.hooks.touch_activity();
                    }
                }
                Poll::Ready(Some(Ok(Bytes::copy_from_slice(filled))))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use axum::http::{HeaderMap, HeaderValue, header};

    use super::*;

    #[test]
    fn parses_64_bit_ranges_beyond_4_gib_and_enforces_strong_if_range() {
        let total = 300_000_000_000u64; // 300 GB (> 4 GiB)
        let etag = "\"0123456789abcdef\"";

        // Open-ended range beyond 4 GiB.
        let mut headers = HeaderMap::new();
        headers.insert(header::RANGE, HeaderValue::from_static("bytes=4294967296-"));
        headers.insert(header::IF_RANGE, HeaderValue::from_static(etag));
        assert_eq!(
            evaluate_range(&headers, total, etag),
            RangeDecision::Partial {
                start: 4_294_967_296,
                end: total - 1,
            }
        );

        // Closed range clamping end to total - 1.
        let mut headers = HeaderMap::new();
        headers.insert(
            header::RANGE,
            HeaderValue::from_static("bytes=299999999990-400000000000"),
        );
        assert_eq!(
            evaluate_range(&headers, total, etag),
            RangeDecision::Partial {
                start: 299_999_999_990,
                end: total - 1,
            }
        );

        // Suffix range.
        assert_eq!(
            parse_single_byte_range("bytes=-16", total),
            RangeDecision::Partial {
                start: total - 16,
                end: total - 1,
            }
        );

        // Unsatisfiable range at exact EOF -> 416.
        assert_eq!(
            parse_single_byte_range("bytes=300000000000-", total),
            RangeDecision::Unsatisfiable
        );

        // If-Range mismatch -> 200 Full.
        let mut headers = HeaderMap::new();
        headers.insert(
            header::RANGE,
            HeaderValue::from_static("bytes=4294967296-4294967303"),
        );
        headers.insert(
            header::IF_RANGE,
            HeaderValue::from_static("\"different-etag\""),
        );
        assert_eq!(evaluate_range(&headers, total, etag), RangeDecision::Full);

        // Weak If-Range -> never matches strong validator -> 200 Full.
        let mut headers = HeaderMap::new();
        headers.insert(header::RANGE, HeaderValue::from_static("bytes=0-10"));
        headers.insert(
            header::IF_RANGE,
            HeaderValue::from_static("W/\"0123456789abcdef\""),
        );
        assert_eq!(evaluate_range(&headers, total, etag), RangeDecision::Full);
    }
}
