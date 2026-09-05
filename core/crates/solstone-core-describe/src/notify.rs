// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::io::Write;
#[cfg(unix)]
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

use serde_json::json;
use solstone_core_format::paths::relative_to_journal;
use solstone_core_format::segment::{is_date_key, segment_key};

/// Best-effort live callosum notification. Notification failure never changes media handling.
pub fn blocked(
    journal: &Path,
    work_key: &str,
    reason_code: Option<&str>,
    provider: Option<&str>,
    context: Option<&str>,
) {
    let key = reason_code
        .map(|code| format!("observe.describe:{code}"))
        .unwrap_or_else(|| "observe.describe.session".to_owned());
    let mut row = json!({"tract":"notification", "event":"show", "key":key, "work_key":work_key});
    if let Some(code) = reason_code {
        row["reason_code"] = json!(code);
    }
    if let Some(provider) = provider {
        row["provider"] = json!(provider);
    }
    if let Some(context) = context {
        row["context"] = json!(context);
    }
    send(journal, &row);
}

/// Return the first date-key ancestor, matching Python's `day_from_path` scan.
pub fn day_for_path(path: &Path) -> Option<String> {
    path.ancestors().find_map(|ancestor| {
        let name = ancestor.file_name()?.to_str()?;
        is_date_key(name).then(|| name.to_owned())
    })
}

/// Return the immediate parent segment key, matching Python's `get_segment_key`.
pub fn segment_for_video_path(video_path: &Path) -> Option<String> {
    video_path
        .parent()?
        .file_name()?
        .to_str()
        .and_then(segment_key)
}

/// Best-effort successful-description event. Sending failure never changes media handling.
pub fn described(
    journal: &Path,
    input: &Path,
    output: &Path,
    duration_ms: u64,
    day: Option<&str>,
    segment: Option<&str>,
    observer: Option<&str>,
) {
    let input = relative_to_journal(journal, input).unwrap_or_else(|| input.display().to_string());
    let output =
        relative_to_journal(journal, output).unwrap_or_else(|| output.display().to_string());
    let mut row = json!({
        "tract":"observe",
        "event":"described",
        "input":input,
        "output":output,
        "duration_ms":duration_ms,
    });
    if let Some(day) = day {
        row["day"] = json!(day);
    }
    if let Some(segment) = segment {
        row["segment"] = json!(segment);
    }
    if let Some(observer) = observer.filter(|observer| !observer.is_empty()) {
        row["observer"] = json!(observer);
    }
    send(journal, &row);
}

#[cfg(unix)]
fn send(journal: &Path, row: &serde_json::Value) {
    let Ok(mut stream) = UnixStream::connect(journal.join("health/callosum.sock")) else {
        return;
    };
    let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
    let _ = stream.write_all(format!("{row}\n").as_bytes());
}

#[cfg(not(unix))]
fn send(_journal: &Path, _row: &serde_json::Value) {}

#[cfg(all(test, not(feature = "full-tests")))]
mod tests {
    use std::path::Path;

    use super::{day_for_path, segment_for_video_path};

    #[test]
    fn day_scan_matches_date_key_ancestors_only() {
        assert_eq!(
            day_for_path(Path::new(
                "/journal/chronicle/20260102/143022_300/screen.webm"
            )),
            Some("20260102".to_owned())
        );
        assert_eq!(
            day_for_path(Path::new("/journal/no-date/143022_300/screen.webm")),
            None
        );
        assert_eq!(
            day_for_path(Path::new("/journal/no-date/20260102.webm")),
            None
        );
    }

    #[test]
    fn segment_scan_is_limited_to_the_immediate_parent() {
        assert_eq!(
            segment_for_video_path(Path::new("/journal/20260102/143022_300/screen.webm")),
            Some("143022_300".to_owned())
        );
        assert_eq!(
            segment_for_video_path(Path::new("/journal/20260102/not-a-segment/screen.webm")),
            None
        );
        assert_eq!(segment_for_video_path(Path::new("screen.webm")), None);
    }
}
