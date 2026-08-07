// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::io::Write;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

use serde_json::json;

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
    let Ok(mut stream) = UnixStream::connect(journal.join("health/callosum.sock")) else {
        return;
    };
    let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
    let _ = stream.write_all(format!("{row}\n").as_bytes());
}
