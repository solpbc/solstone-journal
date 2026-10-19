// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::Value;

use super::{JsonObject, ProducedChunks, recorded_chunk};

fn clean_url(raw: &str) -> Option<String> {
    let (scheme, rest) = raw.split_once("://")?;
    if scheme.is_empty()
        || !scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '-' || c == '.')
    {
        return None;
    }
    let rest = match rest.split_once('#') {
        Some((before, _)) => before,
        None => rest,
    };
    let rest = match rest.split_once('?') {
        Some((before, _)) => before,
        None => rest,
    };
    let (authority, path) = match rest.split_once('/') {
        Some((auth, path_part)) => (auth, format!("/{path_part}")),
        None => (rest, String::new()),
    };
    let host_port = match authority.rsplit_once('@') {
        Some((_, hp)) => hp,
        None => authority,
    };
    if host_port.is_empty() && scheme != "file" {
        return None;
    }
    if host_port
        .chars()
        .any(|c| c.is_whitespace() || c == '/' || c == '?' || c == '#')
    {
        return None;
    }
    Some(format!("{scheme}://{host_port}{path}"))
}

fn validate_exe(exe: &str) -> Option<&str> {
    let trimmed = exe.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.contains('/') || trimmed.contains('\\') || trimmed.chars().any(char::is_whitespace) {
        return None;
    }
    Some(trimmed)
}

fn wrap_in_fence(lines: &[String]) -> String {
    let body = lines.join("\n");
    let mut max_backticks = 0;
    let mut current_run = 0;
    for ch in body.chars() {
        if ch == '`' {
            current_run += 1;
            if current_run > max_backticks {
                max_backticks = current_run;
            }
        } else {
            current_run = 0;
        }
    }
    let mut fence_len = (max_backticks + 1).max(3);

    loop {
        let mut lengthened = false;
        for line in lines {
            let trimmed = line.trim();
            if !trimmed.is_empty()
                && trimmed.chars().all(|c| c == '`')
                && trimmed.len() >= fence_len
            {
                fence_len = trimmed.len() + 1;
                lengthened = true;
                break;
            }
        }
        if !lengthened {
            break;
        }
    }

    let fence = "`".repeat(fence_len);
    format!("{fence}\n{body}\n{fence}")
}

pub(super) fn render(rel: &str, records: &[JsonObject]) -> ProducedChunks {
    let mut chunks = Vec::new();
    let mut unsupported_version = false;

    for record in records {
        if record.get("v").and_then(Value::as_i64) != Some(1) {
            unsupported_version = true;
            continue;
        }

        if record.get("t").and_then(Value::as_str) != Some("desktop_activity") {
            continue;
        }

        let Some(kind) = record.get("kind").and_then(Value::as_str) else {
            continue;
        };
        if !matches!(
            kind,
            "foreground" | "activity" | "terminal" | "availability"
        ) {
            continue;
        }

        let Some(method) = record
            .get("method")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
        else {
            continue;
        };

        let Some(ts) = record
            .get("ts")
            .and_then(super::browser::integer_milliseconds)
        else {
            continue;
        };

        let Some(end_ts) = record
            .get("end_ts")
            .and_then(super::browser::integer_milliseconds)
        else {
            continue;
        };

        if end_ts < ts {
            continue;
        }

        let app;
        let mut app_id = None;
        let mut title = None;
        let mut url = None;
        let mut state = None;
        let mut pane_title = None;
        let mut cwd = None;
        let mut exe = None;
        let mut session = None;
        let mut window = None;
        let mut capability = None;
        let mut status = None;

        match kind {
            "foreground" => {
                let Some(val) = record
                    .get("app")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                else {
                    continue;
                };
                app = Some(val);
                app_id = record
                    .get("app_id")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty());
                title = record
                    .get("title")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty());
                url = record
                    .get("url")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .and_then(clean_url);
            }
            "activity" => {
                app = None;
                let Some(val) = record
                    .get("state")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|s| matches!(*s, "active" | "idle" | "locked"))
                else {
                    continue;
                };
                state = Some(val);
            }
            "terminal" => {
                app = None;
                pane_title = record
                    .get("pane_title")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty());
                cwd = record
                    .get("cwd")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty());
                exe = record
                    .get("exe")
                    .and_then(Value::as_str)
                    .and_then(validate_exe);
                session = record
                    .get("session")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty());
                window = record
                    .get("window")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty());
            }
            "availability" => {
                app = None;
                let Some(cap) = record
                    .get("capability")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|s| matches!(*s, "foreground" | "title" | "url" | "idle" | "terminal"))
                else {
                    continue;
                };
                let Some(stat) = record
                    .get("status")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|s| matches!(*s, "unavailable" | "denied" | "unsupported"))
                else {
                    continue;
                };
                capability = Some(cap);
                status = Some(stat);
            }
            _ => continue,
        }

        let mut lines = Vec::new();
        lines.push("desktop activity".to_string());
        lines.push(format!(
            "kind: {}",
            serde_json::to_string(kind).expect("json string")
        ));
        lines.push(format!(
            "method: {}",
            serde_json::to_string(method).expect("json string")
        ));
        lines.push(format!("start_ms: {ts}"));
        lines.push(format!("end_ms: {end_ts}"));
        let parts: Vec<&str> = rel.split('/').collect();
        if parts.len() == 4 && parts.iter().all(|p| !p.is_empty()) {
            lines.push(format!(
                "stream: {}",
                serde_json::to_string(parts[1]).expect("json string")
            ));
        }
        lines.push(format!(
            "path: {}",
            serde_json::to_string(rel).expect("json string")
        ));

        match kind {
            "foreground" => {
                if let Some(app) = app {
                    lines.push(format!(
                        "app: {}",
                        serde_json::to_string(app).expect("json string")
                    ));
                }
                if let Some(app_id) = app_id {
                    lines.push(format!(
                        "app_id: {}",
                        serde_json::to_string(app_id).expect("json string")
                    ));
                }
                if let Some(title) = title {
                    lines.push(format!(
                        "title: {}",
                        serde_json::to_string(title).expect("json string")
                    ));
                }
                if let Some(url) = &url {
                    lines.push(format!(
                        "url: {}",
                        serde_json::to_string(url).expect("json string")
                    ));
                }
            }
            "activity" => {
                if let Some(state) = state {
                    lines.push(format!(
                        "state: {}",
                        serde_json::to_string(state).expect("json string")
                    ));
                }
            }
            "terminal" => {
                if let Some(pane_title) = pane_title {
                    lines.push(format!(
                        "pane_title: {}",
                        serde_json::to_string(pane_title).expect("json string")
                    ));
                }
                if let Some(cwd) = cwd {
                    lines.push(format!(
                        "cwd: {}",
                        serde_json::to_string(cwd).expect("json string")
                    ));
                }
                if let Some(exe) = exe {
                    lines.push(format!(
                        "exe: {}",
                        serde_json::to_string(exe).expect("json string")
                    ));
                }
                if let Some(session) = session {
                    lines.push(format!(
                        "session: {}",
                        serde_json::to_string(session).expect("json string")
                    ));
                }
                if let Some(window) = window {
                    lines.push(format!(
                        "window: {}",
                        serde_json::to_string(window).expect("json string")
                    ));
                }
            }
            "availability" => {
                if let Some(capability) = capability {
                    lines.push(format!(
                        "capability: {}",
                        serde_json::to_string(capability).expect("json string")
                    ));
                }
                if let Some(status) = status {
                    lines.push(format!(
                        "status: {}",
                        serde_json::to_string(status).expect("json string")
                    ));
                }
            }
            _ => {}
        }
        let content = wrap_in_fence(&lines);

        let mut source = serde_json::Map::new();
        source.insert("kind".to_string(), Value::String(kind.to_string()));
        source.insert("method".to_string(), Value::String(method.to_string()));
        source.insert("ts".to_string(), Value::Number(ts.into()));
        source.insert("end_ts".to_string(), Value::Number(end_ts.into()));

        match kind {
            "foreground" => {
                if let Some(app) = app {
                    source.insert("app".to_string(), Value::String(app.to_string()));
                }
                if let Some(app_id) = app_id {
                    source.insert("app_id".to_string(), Value::String(app_id.to_string()));
                }
                if let Some(title) = title {
                    source.insert("title".to_string(), Value::String(title.to_string()));
                }
                if let Some(url) = url {
                    source.insert("url".to_string(), Value::String(url));
                }
            }
            "activity" => {
                if let Some(state) = state {
                    source.insert("state".to_string(), Value::String(state.to_string()));
                }
            }
            "terminal" => {
                if let Some(pane_title) = pane_title {
                    source.insert(
                        "pane_title".to_string(),
                        Value::String(pane_title.to_string()),
                    );
                }
                if let Some(cwd) = cwd {
                    source.insert("cwd".to_string(), Value::String(cwd.to_string()));
                }
                if let Some(exe) = exe {
                    source.insert("exe".to_string(), Value::String(exe.to_string()));
                }
                if let Some(session) = session {
                    source.insert("session".to_string(), Value::String(session.to_string()));
                }
                if let Some(window) = window {
                    source.insert("window".to_string(), Value::String(window.to_string()));
                }
            }
            "availability" => {
                if let Some(capability) = capability {
                    source.insert(
                        "capability".to_string(),
                        Value::String(capability.to_string()),
                    );
                }
                if let Some(status) = status {
                    source.insert("status".to_string(), Value::String(status.to_string()));
                }
            }
            _ => {}
        }

        chunks.push(recorded_chunk(content, ts, &source));
    }

    let mut warnings = Vec::new();
    if unsupported_version {
        warnings.push("unsupported desktop activity version".to_string());
    }

    ProducedChunks {
        chunks,
        agent_override: Some("desktop_activity".to_string()),
        header: None,
        error: None,
        warnings,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::{
        ContentResolution, Family, IndexDisposition, classify, produce_chunks,
        resolve_content_shape, resolve_spec,
    };
    use crate::test_support::reserve_temp_path;
    use std::fs;
    use std::path::PathBuf;

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(name: &str) -> Self {
            let path = reserve_temp_path(&format!("solstone-core-format-desktop-activity-{name}"));
            fs::create_dir_all(&path).expect("create temporary directory");
            Self { path }
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn desktop_activity_classification_disposition_and_unrecognized() {
        let valid_rel = "20260910/desk/120000_60/120000_60_desktop_activity.jsonl";
        assert_eq!(
            classify(valid_rel),
            ContentResolution::Indexed(Family::DesktopActivity)
        );
        let spec = resolve_spec(valid_rel).expect("spec resolves");
        assert_eq!(spec.family, Family::DesktopActivity);
        assert_eq!(spec.disposition, IndexDisposition::Excluded);

        assert_eq!(
            classify("desktop_activity.jsonl"),
            ContentResolution::Unrecognized
        );
        assert_eq!(
            classify("20260910/desk/120000_60/desktop_activity.jsonl"),
            ContentResolution::Unrecognized
        );
    }

    #[test]
    fn desktop_activity_shape_sidecar_cannot_reclassify() {
        let temp = TempDir::new("sidecar");
        let note_rel = "20260910/desk/120000_60/120000_60_desktop_activity.jsonl";
        let note_path = temp.path.join(note_rel);
        fs::create_dir_all(note_path.parent().unwrap()).unwrap();
        fs::write(&note_path, "{}\n").unwrap();
        fs::write(
            note_path.parent().unwrap().join("shape.json"),
            r#"{"120000_60_desktop_activity.jsonl":"Browser"}"#,
        )
        .unwrap();

        assert_eq!(
            resolve_content_shape(&note_path, note_rel),
            ContentResolution::Indexed(Family::DesktopActivity)
        );
    }

    #[test]
    fn desktop_activity_producers_kinds_and_interval_rules() {
        let rel = "20260910/desk/120000_60/120000_60_desktop_activity.jsonl";
        let methods = ["nsworkspace", "winevent", "x11-ewmh", "gnome-shell", "tmux"];
        for method in methods {
            let row = serde_json::json!({
                "v": 1,
                "t": "desktop_activity",
                "kind": "foreground",
                "method": method,
                "ts": 1000,
                "end_ts": 1000, // equal timestamps valid
                "app": "Code"
            });
            let produced = render(rel, &[row.as_object().unwrap().clone()]);
            assert_eq!(produced.chunks.len(), 1);
            assert!(
                produced.chunks[0]
                    .content
                    .contains(&format!("method: \"{method}\""))
            );
            assert!(produced.chunks[0].content.contains("start_ms: 1000"));
            assert!(produced.chunks[0].content.contains("end_ms: 1000"));
        }

        // Inverted timestamps skipped
        let inverted = serde_json::json!({
            "v": 1,
            "t": "desktop_activity",
            "kind": "foreground",
            "method": "tmux",
            "ts": 2000,
            "end_ts": 1000,
            "app": "Code"
        });
        let produced = render(rel, &[inverted.as_object().unwrap().clone()]);
        assert_eq!(produced.chunks.len(), 0);
    }

    #[test]
    fn desktop_activity_kinds_and_field_validations() {
        let rel = "20260910/desk/120000_60/120000_60_desktop_activity.jsonl";

        // Missing app skipped
        let missing_app = serde_json::json!({
            "v": 1,
            "t": "desktop_activity",
            "kind": "foreground",
            "method": "nsworkspace",
            "ts": 1000,
            "end_ts": 2000,
        });
        assert_eq!(
            render(rel, &[missing_app.as_object().unwrap().clone()])
                .chunks
                .len(),
            0
        );

        // Optional wrong type omitted
        let wrong_opt_type = serde_json::json!({
            "v": 1,
            "t": "desktop_activity",
            "kind": "foreground",
            "method": "nsworkspace",
            "ts": 1000,
            "end_ts": 2000,
            "app": "Code",
            "app_id": 12345, // wrong type: not a string
            "unknown_extra": "drop_me"
        });
        let produced = render(rel, &[wrong_opt_type.as_object().unwrap().clone()]);
        assert_eq!(produced.chunks.len(), 1);
        let chunk = &produced.chunks[0];
        assert!(!chunk.content.contains("app_id"));
        assert!(!chunk.content.contains("unknown_extra"));
        assert!(!chunk.content.contains("drop_me"));
        assert!(!chunk.source.as_ref().unwrap().contains_key("app_id"));
        assert!(!chunk.source.as_ref().unwrap().contains_key("unknown_extra"));

        // URL query, fragment, userinfo absent and stripped URL present
        let url_row = serde_json::json!({
            "v": 1,
            "t": "desktop_activity",
            "kind": "foreground",
            "method": "nsworkspace",
            "ts": 1000,
            "end_ts": 2000,
            "app": "Browser",
            "url": "https://user:secret@example.com:8443/a/b?token=1#frag"
        });
        let produced = render(rel, &[url_row.as_object().unwrap().clone()]);
        assert_eq!(produced.chunks.len(), 1);
        let chunk = &produced.chunks[0];
        assert!(
            chunk
                .content
                .contains(r#"url: "https://example.com:8443/a/b""#)
        );
        assert!(!chunk.content.contains("token=1"));
        assert!(!chunk.content.contains("frag"));
        assert!(!chunk.content.contains("user:secret"));
        assert_eq!(
            chunk
                .source
                .as_ref()
                .unwrap()
                .get("url")
                .and_then(Value::as_str),
            Some("https://example.com:8443/a/b")
        );

        // Terminal exe path absent and not rewritten
        let terminal_path_exe = serde_json::json!({
            "v": 1,
            "t": "desktop_activity",
            "kind": "terminal",
            "method": "tmux",
            "ts": 1000,
            "end_ts": 2000,
            "exe": "/usr/bin/bash",
            "session": "main"
        });
        let produced = render(rel, &[terminal_path_exe.as_object().unwrap().clone()]);
        assert_eq!(produced.chunks.len(), 1);
        let chunk = &produced.chunks[0];
        assert!(!chunk.content.contains("exe"));
        assert!(!chunk.content.contains("bash"));
        assert!(!chunk.source.as_ref().unwrap().contains_key("exe"));
        assert_eq!(
            chunk
                .source
                .as_ref()
                .unwrap()
                .get("session")
                .and_then(Value::as_str),
            Some("main")
        );

        // Terminal valid single-segment exe kept
        let terminal_valid_exe = serde_json::json!({
            "v": 1,
            "t": "desktop_activity",
            "kind": "terminal",
            "method": "tmux",
            "ts": 1000,
            "end_ts": 2000,
            "exe": "bash"
        });
        let produced = render(rel, &[terminal_valid_exe.as_object().unwrap().clone()]);
        assert_eq!(produced.chunks.len(), 1);
        assert!(produced.chunks[0].content.contains(r#"exe: "bash""#));
        assert_eq!(
            produced.chunks[0]
                .source
                .as_ref()
                .unwrap()
                .get("exe")
                .and_then(Value::as_str),
            Some("bash")
        );

        // Activity kind
        let activity_row = serde_json::json!({
            "v": 1,
            "t": "desktop_activity",
            "kind": "activity",
            "method": "winevent",
            "ts": 1000,
            "end_ts": 2000,
            "state": "idle"
        });
        let produced = render(rel, &[activity_row.as_object().unwrap().clone()]);
        assert_eq!(produced.chunks.len(), 1);
        assert!(produced.chunks[0].content.contains(r#"state: "idle""#));
        assert_eq!(
            produced.chunks[0]
                .source
                .as_ref()
                .unwrap()
                .get("state")
                .and_then(Value::as_str),
            Some("idle")
        );

        // Availability content and source lack app, title, url
        let avail_row = serde_json::json!({
            "v": 1,
            "t": "desktop_activity",
            "kind": "availability",
            "method": "x11-ewmh",
            "ts": 1000,
            "end_ts": 2000,
            "capability": "terminal",
            "status": "unsupported",
            "app": "Code",
            "title": "ignored",
            "url": "https://example.com/ignored"
        });
        let produced = render(rel, &[avail_row.as_object().unwrap().clone()]);
        assert_eq!(produced.chunks.len(), 1);
        let chunk = &produced.chunks[0];
        assert!(chunk.content.contains(r#"capability: "terminal""#));
        assert!(chunk.content.contains(r#"status: "unsupported""#));
        assert!(!chunk.content.contains("Code"));
        assert!(!chunk.content.contains("ignored"));
        assert!(!chunk.source.as_ref().unwrap().contains_key("app"));
        assert!(!chunk.source.as_ref().unwrap().contains_key("title"));
        assert!(!chunk.source.as_ref().unwrap().contains_key("url"));
    }

    #[test]
    fn desktop_activity_mixed_file_keeps_valid_rows_and_emits_one_warning_without_rewriting_bytes()
    {
        let rel = "20260910/desk/120000_60/120000_60_desktop_activity.jsonl";
        let original_text = [
            // 1. Valid row
            r#"{"v":1,"t":"desktop_activity","kind":"foreground","method":"nsworkspace","ts":1000,"end_ts":2000,"app":"Finder"}"#,
            // 2. Malformed line
            r#"{"v":1,"t":"desktop_activity""#,
            // 3. Invalid v1 row (end_ts < ts)
            r#"{"v":1,"t":"desktop_activity","kind":"foreground","method":"nsworkspace","ts":3000,"end_ts":1000,"app":"Finder"}"#,
            // 4. v of 999
            r#"{"v":999,"t":"desktop_activity","kind":"foreground","method":"nsworkspace","ts":4000,"end_ts":5000,"app":"Terminal"}"#,
        ]
        .join("\n");

        let text_copy = original_text.clone();
        let produced = produce_chunks(Family::DesktopActivity, rel, &text_copy);

        // Bytes unchanged
        assert_eq!(text_copy, original_text);
        assert_eq!(produced.chunks.len(), 1);
        assert!(produced.chunks[0].content.contains(r#"app: "Finder""#));
        assert_eq!(produced.error, None);
        assert_eq!(produced.warnings.len(), 1);
        assert!(!produced.warnings[0].contains("Terminal"));
        assert_eq!(produced.agent_override.as_deref(), Some("desktop_activity"));
    }

    #[test]
    fn desktop_activity_literal_evidence_block_and_fencing() {
        let rel = "20260910/desk/120000_60/120000_60_desktop_activity.jsonl";
        let raw_title = "<script>alert(1)</script> ![x](https://example.test/a.png) [open](https://example.test/p) ````\nsecond line";
        let raw_pane = "<script>alert(1)</script> ![x](https://example.test/a.png) [open](https://example.test/p) `````\nterminal line";
        let rows = vec![
            serde_json::json!({
                "v": 1,
                "t": "desktop_activity",
                "kind": "foreground",
                "method": "nsworkspace",
                "ts": 1000,
                "end_ts": 2000,
                "app": "TestApp",
                "title": raw_title,
                "url": "https://example.test/p"
            })
            .as_object()
            .unwrap()
            .clone(),
            serde_json::json!({
                "v": 1,
                "t": "desktop_activity",
                "kind": "terminal",
                "method": "tmux",
                "ts": 3000,
                "end_ts": 4000,
                "pane_title": raw_pane,
                "exe": "bash"
            })
            .as_object()
            .unwrap()
            .clone(),
        ];

        let produced = render(rel, &rows);
        assert_eq!(produced.error, None);
        assert_eq!(produced.chunks.len(), 2);

        for chunk in &produced.chunks {
            assert!(chunk.content.contains("<script>alert(1)</script>"));
            assert!(chunk.content.contains("![x](https://example.test/a.png)"));
            assert!(chunk.content.contains("[open](https://example.test/p)"));
            assert!(chunk.content.contains("````"));
            assert!(chunk.content.contains(r#"\n"#));
            assert!(!chunk.content.contains("\nsecond line"));
            assert!(!chunk.content.contains("\nterminal line"));

            let lines: Vec<&str> = chunk.content.lines().collect();
            let first_line = lines.first().copied().unwrap();
            let last_line = lines.last().copied().unwrap();
            assert_eq!(first_line, last_line);
            assert!(first_line.starts_with("```"));
            assert!(first_line.chars().all(|c| c == '`'));

            let body_lines = &lines[1..lines.len() - 1];
            let body = body_lines.join("\n");
            let mut max_backticks = 0;
            let mut current_run = 0;
            for ch in body.chars() {
                if ch == '`' {
                    current_run += 1;
                    if current_run > max_backticks {
                        max_backticks = current_run;
                    }
                } else {
                    current_run = 0;
                }
            }
            assert!(first_line.len() > max_backticks);

            for line in body_lines {
                let trimmed = line.trim();
                assert!(
                    !(trimmed.chars().all(|c| c == '`') && !trimmed.is_empty()),
                    "body line should not be only backticks: {line}"
                );
            }
        }

        let foreground_chunk = &produced.chunks[0];
        assert_eq!(
            foreground_chunk
                .source
                .as_ref()
                .unwrap()
                .get("title")
                .and_then(Value::as_str),
            Some(raw_title)
        );
        let first_fence = produced.chunks[0].content.lines().next().unwrap();
        assert!(
            !foreground_chunk
                .source
                .as_ref()
                .unwrap()
                .get("title")
                .unwrap()
                .as_str()
                .unwrap()
                .contains(first_fence)
        );
    }
}
