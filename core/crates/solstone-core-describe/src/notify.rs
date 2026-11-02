// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;
use std::time::Duration;

use serde_json::json;
use solstone_core_callosum::CallosumOneShotSender;
use solstone_core_format::paths::relative_to_journal;
use solstone_core_format::segment::{is_date_key, segment_key};

/// Best-effort live callosum notification for blocked work the owner has to
/// fix. Any other reason, or none, sends nothing: the run is still deferred,
/// its reason logged and its work retried.
/// Notification failure never changes media handling.
pub fn blocked(journal: &Path, work_key: &str, reason_code: Option<&str>) {
    if let Some(row) = blocked_row(work_key, reason_code) {
        send(journal, &row);
    }
}

fn blocked_row(work_key: &str, reason_code: Option<&str>) -> Option<serde_json::Value> {
    let code = reason_code?;
    let message = owner_fix(code)?;
    Some(json!({
        "tract": "notification",
        "event": "show",
        "app": "thinking",
        "icon": "eye",
        "title": "screen descriptions are waiting",
        "message": message,
        "action": "/app/thinking",
        "key": format!("observe.describe:{code}"),
        "work_key": work_key,
        "reason_code": code,
    }))
}

/// What the owner has to do for a blocked reason that waiting will not clear.
/// Every other reason returns `None` and stays quiet: loading, installing, a
/// busy or unreachable engine, and usage limits, which providers also report
/// for short rate limits that clear on their own.
fn owner_fix(reason_code: &str) -> Option<&'static str> {
    Some(match reason_code {
        "provider_key_invalid" => {
            "your AI provider turned down your key. check it on the thinking page"
        }
        "provider_key_missing" => "your AI provider needs a key. add one on the thinking page",
        "chatgpt_sign_in_required" => "signed out of ChatGPT. sign in again on the thinking page",
        "chatgpt_not_eligible" => {
            "this ChatGPT account can't be used for thinking. pick another provider on the thinking page"
        }
        "confidential_access_ended" => {
            "confidential processing isn't active for this journal. open the thinking page to see why"
        }
        _ => return None,
    })
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

/// One framed line through the local Callosum transport: the Unix socket, or
/// on Windows the resident's authenticated named pipe. Best effort, as above.
fn send(journal: &Path, row: &serde_json::Value) {
    let _ = CallosumOneShotSender::new(journal.join("health/callosum.sock"), SEND_TIMEOUT)
        .send_line(&format!("{row}\n"));
}

const SEND_TIMEOUT: Duration = Duration::from_secs(2);

#[cfg(all(test, not(feature = "full-tests")))]
mod tests {
    use std::path::Path;

    use super::{blocked_row, day_for_path, segment_for_video_path};

    #[test]
    fn a_reason_the_owner_must_fix_gets_a_titled_notice_with_a_route() {
        let row = blocked_row("work-1", Some("provider_key_invalid")).expect("owner-fixable");
        assert_eq!(row["tract"], "notification");
        assert_eq!(row["event"], "show");
        assert!(!row["title"].as_str().unwrap_or_default().is_empty());
        assert!(!row["message"].as_str().unwrap_or_default().is_empty());
        assert_eq!(row["action"], "/app/thinking");
        assert_eq!(row["key"], "observe.describe:provider_key_invalid");
        assert_eq!(row["work_key"], "work-1");
    }

    #[test]
    fn every_other_reason_or_no_reason_stays_quiet() {
        for code in [
            "local_model_loading",
            "local_model_installing",
            "install_busy",
            "provider_unavailable",
            "provider_quota_exceeded",
            "chatgpt_usage_limit",
            "local_endpoint_unreachable",
            "thinking_engine_not_chosen",
            "unknown",
        ] {
            assert!(blocked_row("work-1", Some(code)).is_none(), "{code}");
        }
        assert!(blocked_row("work-1", None).is_none());
    }

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
