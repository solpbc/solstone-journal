// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::thread;

use chrono::{Local, TimeZone};
use serde_json::{Map, Value, json};

#[derive(Clone, Debug)]
pub struct CortexStore {
    journal: PathBuf,
    talents: PathBuf,
}

impl CortexStore {
    pub fn new(journal: PathBuf) -> io::Result<Self> {
        let talents = journal.join("talents");
        fs::create_dir_all(&talents)?;
        Ok(Self { journal, talents })
    }

    pub(crate) fn active_path(&self, name: &str, use_id: &str) -> PathBuf {
        self.talents
            .join(safe_name(name))
            .join(format!("{use_id}_active.jsonl"))
    }

    pub fn claim(
        &self,
        name: &str,
        use_id: &str,
        request: &Map<String, Value>,
    ) -> io::Result<Option<PathBuf>> {
        let path = self.active_path(name, use_id);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let line = serde_json::to_vec(&Value::Object(request.clone()))?;
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut file) => {
                file.write_all(&line)?;
                file.write_all(b"\n")?;
                Ok(Some(path))
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// Append only to the active inode. This intentionally does not create a
    /// missing file: late monitor output must not resurrect a completed use.
    pub(crate) fn append_active(
        &self,
        path: &Path,
        event: &Map<String, Value>,
    ) -> io::Result<bool> {
        let line = serde_json::to_vec(&Value::Object(event.clone()))?;
        match OpenOptions::new().append(true).open(path) {
            Ok(mut file) => {
                file.write_all(&line)?;
                file.write_all(b"\n")?;
                Ok(true)
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error),
        }
    }

    pub(crate) fn recover(&self) {
        let Ok(entries) = fs::read_dir(&self.talents) else {
            return;
        };
        for directory in entries.flatten().filter_map(|entry| {
            entry
                .file_type()
                .ok()
                .filter(|kind| kind.is_dir())
                .map(|_| entry.path())
        }) {
            let Ok(files) = fs::read_dir(directory) else {
                continue;
            };
            for active in files.flatten().map(|entry| entry.path()).filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.ends_with("_active.jsonl"))
            }) {
                let Some(use_id) = active
                    .file_stem()
                    .and_then(|name| name.to_str())
                    .and_then(|name| name.strip_suffix("_active"))
                    .map(str::to_owned)
                else {
                    continue;
                };
                let mut error = synthesized_error(
                    &use_id,
                    "Recovered: Cortex restarted while talent was running",
                );
                // Recovery deliberately uses create-or-append. Its caller has just
                // globbed existing active paths, unlike late-event appends.
                if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&active) {
                    let _ = writeln!(file, "{}", Value::Object(std::mem::take(&mut error)));
                    let completed = active.with_file_name(format!("{use_id}.jsonl"));
                    let _ = fs::rename(&active, completed);
                }
            }
        }
    }

    pub(crate) fn complete(
        &self,
        use_id: &str,
        active: &Path,
        request: Option<&Map<String, Value>>,
    ) {
        let completed = active.with_file_name(format!("{use_id}.jsonl"));
        if fs::rename(active, &completed).is_err() {
            eprintln!("cortex: failed to complete talent file {use_id}");
            return;
        }
        let Some(request) = request else { return };
        let Some(name) = request
            .get("name")
            .and_then(Value::as_str)
            .filter(|name| !name.is_empty())
        else {
            return;
        };
        let link = self.talents.join(format!("{}.log", safe_name(name)));
        atomic_symlink(&link, &format!("{}/{use_id}.jsonl", safe_name(name)));
        self.append_day_index(use_id, request, &completed);
    }

    pub(crate) fn has_finish(&self, active: &Path) -> bool {
        let Ok(text) = fs::read_to_string(active) else {
            return false;
        };
        text.lines().any(|line| {
            let Ok(event) = serde_json::from_str::<Value>(line) else {
                return false;
            };
            event.get("event") == Some(&Value::String("finish".into()))
                || (event.get("event") == Some(&Value::String("error".into()))
                    && event
                        .get("terminal")
                        .and_then(Value::as_bool)
                        .unwrap_or(true))
        })
    }

    pub(crate) fn append_day_index(
        &self,
        use_id: &str,
        request: &Map<String, Value>,
        completed: &Path,
    ) {
        let day = request
            .get("day")
            .and_then(Value::as_str)
            .filter(|day| is_day_key(day))
            .map(str::to_owned)
            .unwrap_or_else(|| day_from_use_id(use_id));
        if !is_day_key(&day) {
            return;
        }
        // No sender populates a request `ts`: the dispatcher stamps `ts` onto each
        // relayed event, not onto the request. Defaulting to 0 made every day-index
        // row render at the epoch and, because `runtime` treats a zero start as
        // unknown, left `runtime_seconds` null. The use id is epoch milliseconds --
        // `day_from_use_id` above already relies on that -- so derive from it.
        let start_ts = request
            .get("ts")
            .and_then(Value::as_i64)
            .filter(|timestamp| *timestamp != 0)
            .unwrap_or_else(|| ts_from_use_id(use_id));
        let mut thinking_count = 0_u64;
        let mut tool_count = 0_u64;
        let mut degraded = Value::Null;
        let mut error_message = Value::Null;
        let mut reason_code = Value::Null;
        let mut model = Value::Null;
        let mut runtime_seconds = Value::Null;
        let mut status = "completed";
        if let Ok(text) = fs::read_to_string(completed) {
            for line in text.lines() {
                let Ok(event) = serde_json::from_str::<Value>(line) else {
                    continue;
                };
                match event.get("event").and_then(Value::as_str) {
                    Some("thinking") => thinking_count += 1,
                    Some("tool_start") => tool_count += 1,
                    Some("start") => model = event.get("model").cloned().unwrap_or(Value::Null),
                    Some("finish") => {
                        status = "completed";
                        degraded = event.get("degraded").cloned().unwrap_or(Value::Null);
                        runtime_seconds =
                            runtime(start_ts, event.get("ts").and_then(Value::as_i64));
                    }
                    Some("error") => {
                        status = "error";
                        error_message = event
                            .get("error")
                            .and_then(Value::as_str)
                            .map(|text| Value::String(text.chars().take(200).collect()))
                            .unwrap_or(Value::Null);
                        reason_code = event.get("reason_code").cloned().unwrap_or(Value::Null);
                        runtime_seconds =
                            runtime(start_ts, event.get("ts").and_then(Value::as_i64));
                    }
                    _ => {}
                }
            }
        }
        let summary = json!({
            "use_id": use_id,
            "name": request.get("name").cloned().unwrap_or(Value::Null),
            "day": day,
            "facet": request.get("facet").cloned().unwrap_or(Value::Null),
            "ts": start_ts,
            "status": status,
            "runtime_seconds": runtime_seconds,
            "provider": request.get("provider").cloned().unwrap_or(Value::Null),
            "model": model,
            "schedule": request.get("schedule").cloned().unwrap_or(Value::Null),
            "thinking_count": thinking_count,
            "tool_count": tool_count,
            // Keep cost in the fixed day-index shape, but leave it null: the reference
            // price table is generated Python source that a native artifact may not execute;
            // a bundled snapshot silently drifts, and a conservative fallback can trip a
            // budget ceiling early, making a measured-looking value worse than none.
            // Dropping the field would break the exact-18-field contract.
            "cost": Value::Null,
            "error_message": if status == "error" { error_message } else { Value::Null },
            "reason_code": if status == "error" { reason_code } else { Value::Null },
            "degraded": degraded,
            "output_file": summarize_output_file(&self.journal.join(&day), &self.journal, request)
                .map(Value::String)
                .unwrap_or(Value::Null),
            "prompt": request.get("prompt").cloned().unwrap_or_else(|| Value::String(String::new())),
        });
        let path = self.talents.join(format!("{day}.jsonl"));
        if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
            let _ = writeln!(file, "{summary}");
        }
    }

    #[cfg(test)]
    pub(crate) fn talents(&self) -> &Path {
        &self.talents
    }
    pub(crate) fn journal(&self) -> &Path {
        &self.journal
    }
}

pub(crate) fn synthesized_error(use_id: &str, error: impl Into<String>) -> Map<String, Value> {
    Map::from_iter([
        ("event".into(), Value::String("error".into())),
        ("ts".into(), Value::from(now_ms())),
        ("use_id".into(), Value::String(use_id.to_owned())),
        ("error".into(), Value::String(error.into())),
    ])
}

pub(crate) fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn runtime(start: i64, end: Option<i64>) -> Value {
    match (start, end) {
        (start, Some(end)) if start != 0 && end != 0 => {
            Value::from(((end - start) as f64 / 1000.0 * 10.0).round() / 10.0)
        }
        _ => Value::Null,
    }
}

/// Epoch milliseconds encoded in a use id, or 0 when it is not a numeric id.
fn ts_from_use_id(use_id: &str) -> i64 {
    use_id.parse::<i64>().unwrap_or(0)
}

fn day_from_use_id(use_id: &str) -> String {
    use_id
        .parse::<i64>()
        .ok()
        .and_then(|milliseconds| Local.timestamp_millis_opt(milliseconds).single())
        .map(|time| time.format("%Y%m%d").to_string())
        .unwrap_or_default()
}

fn summarize_output_file(
    day_dir: &Path,
    journal_root: &Path,
    request: &Map<String, Value>,
) -> Option<String> {
    let output_path = request
        .get("output_path")
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .or_else(|| derived_output_path(day_dir, request))?;
    if !output_path.exists() {
        return None;
    }
    output_path
        .strip_prefix(day_dir)
        .or_else(|_| output_path.strip_prefix(journal_root))
        .ok()
        .map(|path| path.display().to_string())
}

fn derived_output_path(day_dir: &Path, request: &Map<String, Value>) -> Option<PathBuf> {
    let output = request.get("output")?;
    let name = request.get("name").and_then(Value::as_str)?;
    let name = match name.split_once(':') {
        Some((app, name)) => format!("_{app}_{name}"),
        None => name.to_owned(),
    };
    let extension = if output == "json" { "json" } else { "md" };
    let file = format!("{name}.{extension}");
    let segment = request
        .get("segment")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty());
    let facet = request
        .get("facet")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty());
    let stream = request
        .get("env")
        .and_then(Value::as_object)
        .and_then(|env| env.get("SOL_STREAM"))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty());
    let output_dir = match segment {
        Some(segment) => {
            let segment_dir = match stream {
                Some(stream) => day_dir.join(stream).join(segment),
                None => day_dir.join(segment),
            };
            segment_dir.join("talents")
        }
        None => day_dir.join("talents"),
    };
    Some(match facet {
        Some(facet) => output_dir.join(facet).join(file),
        None => output_dir.join(file),
    })
}

pub(crate) fn safe_name(name: &str) -> String {
    let candidate = name.replace(':', "--").replace(['/', '\\'], "-");
    if candidate.is_empty()
        || Path::new(&candidate)
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return "_invalid".to_owned();
    }
    candidate
}

fn is_day_key(day: &str) -> bool {
    day.len() == 8 && day.bytes().all(|byte| byte.is_ascii_digit())
}

pub(crate) fn atomic_symlink(path: &Path, target: &str) {
    let Some(parent) = path.parent() else { return };
    if fs::create_dir_all(parent).is_err() {
        return;
    }
    let temporary = temporary_link_path(path);
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        if symlink(target, &temporary).is_ok() {
            let _ = fs::rename(&temporary, path);
        }
    }
    if temporary.exists() || temporary.is_symlink() {
        let _ = fs::remove_file(temporary);
    }
}

fn temporary_link_path(path: &Path) -> PathBuf {
    path.with_extension(format!(
        "tmp{}_{:?}",
        std::process::id(),
        thread::current().id()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn request() -> Map<String, Value> {
        serde_json::from_value(
            json!({"name":"conversation","day":"19700101","ts":1000,"prompt":"p"}),
        )
        .unwrap()
    }

    fn complete_with_request(store: &CortexStore, use_id: &str, request: &Map<String, Value>) {
        let name = request["name"].as_str().expect("name");
        let active = store.claim(name, use_id, request).unwrap().unwrap();
        store.complete(use_id, &active, Some(request));
    }

    fn day_rows(store: &CortexStore, day: &str) -> Vec<Value> {
        fs::read_to_string(store.talents().join(format!("{day}.jsonl")))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    /// No sender puts a `ts` on the request, so the day index used to stamp every
    /// row at the epoch and report a null runtime. The use id is epoch milliseconds.
    #[test]
    fn day_index_row_dates_from_the_use_id_when_the_request_has_no_ts() {
        let directory = tempdir().unwrap();
        let store = CortexStore::new(directory.path().to_path_buf()).unwrap();
        let mut request = request();
        request.remove("ts");
        let use_id = "1788248640729";
        let active = store
            .claim("conversation", use_id, &request)
            .unwrap()
            .unwrap();
        store
            .append_active(
                &active,
                &serde_json::from_value(json!({"event":"start","ts":1788248640729_i64})).unwrap(),
            )
            .unwrap();
        store
            .append_active(
                &active,
                &serde_json::from_value(json!({"event":"finish","ts":1788248643229_i64})).unwrap(),
            )
            .unwrap();
        store.complete(use_id, &active, Some(&request));

        let rows = day_rows(&store, "19700101");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["ts"], json!(1788248640729_i64));
        assert_eq!(rows[0]["runtime_seconds"], json!(2.5));
    }

    #[test]
    fn append_without_create_drops_late_event_and_create_append_causes_bogus_recovery() {
        let directory = tempdir().unwrap();
        let store = CortexStore::new(directory.path().to_path_buf()).unwrap();
        let active = store
            .claim("conversation", "one", &request())
            .unwrap()
            .unwrap();
        store.complete("one", &active, Some(&request()));
        assert!(
            !store
                .append_active(&active, &synthesized_error("one", "late"))
                .unwrap()
        );
        assert!(!active.exists());
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(&active)
            .unwrap();
        store.recover();
        let completed = active.with_file_name("one.jsonl");
        assert!(
            fs::read_to_string(completed)
                .unwrap()
                .contains("Recovered: Cortex restarted while talent was running")
        );
    }

    #[test]
    fn day_index_terminal_status_is_last_wins() {
        let directory = tempdir().unwrap();
        let store = CortexStore::new(directory.path().to_path_buf()).unwrap();
        let active = store
            .claim("conversation", "one", &request())
            .unwrap()
            .unwrap();
        store
            .append_active(
                &active,
                &serde_json::from_value(
                    json!({"event":"error","terminal":false,"error":"not terminal","ts":1100}),
                )
                .unwrap(),
            )
            .unwrap();
        store
            .append_active(
                &active,
                &serde_json::from_value(json!({"event":"finish","ts":1200})).unwrap(),
            )
            .unwrap();
        store.complete("one", &active, Some(&request()));
        let index = fs::read_to_string(store.talents().join("19700101.jsonl")).unwrap();
        let row: Value = serde_json::from_str(index.trim()).unwrap();
        assert_eq!(row["status"], "completed");
        assert!(row["error_message"].is_null());
        assert!(row["reason_code"].is_null());
    }

    #[test]
    fn duplicate_claim_leaves_request_file_byte_identical() {
        let directory = tempdir().unwrap();
        let store = CortexStore::new(directory.path().to_path_buf()).unwrap();
        let request = request();
        let active = store
            .claim("conversation", "one", &request)
            .unwrap()
            .unwrap();
        let before = fs::read(&active).unwrap();
        assert!(
            store
                .claim("conversation", "one", &request)
                .unwrap()
                .is_none()
        );
        assert_eq!(fs::read(active).unwrap(), before);
    }

    #[test]
    fn recovery_does_not_create_day_index_or_repoint_symlink() {
        let directory = tempdir().unwrap();
        let store = CortexStore::new(directory.path().to_path_buf()).unwrap();
        let request = request();
        let active = store
            .claim("conversation", "one", &request)
            .unwrap()
            .unwrap();
        let link = store.talents().join("chat.log");
        atomic_symlink(&link, "chat/old.jsonl");
        let before = fs::read_link(&link).unwrap();
        store.recover();
        assert!(active.with_file_name("one.jsonl").exists());
        assert!(!store.talents().join("19700101.jsonl").exists());
        assert_eq!(fs::read_link(link).unwrap(), before);
    }

    #[test]
    fn completion_without_request_only_renames() {
        let directory = tempdir().unwrap();
        let store = CortexStore::new(directory.path().to_path_buf()).unwrap();
        let active = store
            .claim("conversation", "one", &request())
            .unwrap()
            .unwrap();
        store.complete("one", &active, None);
        assert!(active.with_file_name("one.jsonl").exists());
        assert!(!store.talents().join("chat.log").exists());
        assert!(!store.talents().join("19700101.jsonl").exists());
    }

    #[test]
    fn unreadable_completed_log_fabricates_completed_summary() {
        let directory = tempdir().unwrap();
        let store = CortexStore::new(directory.path().to_path_buf()).unwrap();
        let unreadable = directory.path().join("directory-not-log");
        fs::create_dir(&unreadable).unwrap();
        store.append_day_index("one", &request(), &unreadable);
        let row: Value = serde_json::from_str(
            &fs::read_to_string(store.talents().join("19700101.jsonl")).unwrap(),
        )
        .unwrap();
        assert_eq!(row["status"], "completed");
        assert!(row["runtime_seconds"].is_null());
        assert_eq!(row["thinking_count"], 0);
        assert!(row["model"].is_null());
    }

    #[test]
    fn day_index_has_exact_shape_and_declared_null_temporaries() {
        let directory = tempdir().unwrap();
        let store = CortexStore::new(directory.path().to_path_buf()).unwrap();
        let mut request = request();
        request.insert("model".into(), Value::String("request-model".into()));
        let active = store
            .claim("conversation", "one", &request)
            .unwrap()
            .unwrap();
        for event in [
            json!({"event":"start","model":"priced-model","provider":"provider","ts":1100}),
            json!({"event":"finish","usage":{"input_tokens":2},"ts":2300}),
        ] {
            store
                .append_active(&active, &serde_json::from_value(event).unwrap())
                .unwrap();
        }
        store.complete("one", &active, Some(&request));
        let row: Value = serde_json::from_str(
            &fs::read_to_string(store.talents().join("19700101.jsonl")).unwrap(),
        )
        .unwrap();
        let expected = [
            "use_id",
            "name",
            "day",
            "facet",
            "ts",
            "status",
            "runtime_seconds",
            "provider",
            "model",
            "schedule",
            "thinking_count",
            "tool_count",
            "cost",
            "error_message",
            "reason_code",
            "degraded",
            "output_file",
            "prompt",
        ];
        assert_eq!(
            row.as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            expected
        );
        assert!(row["cost"].is_null());
        assert!(row["output_file"].is_null());
        assert_eq!(row["model"], "priced-model");
        assert_eq!(row["runtime_seconds"], 1.3);
    }

    #[test]
    fn day_index_summarizes_daily_plain_and_facet_output_files() {
        let directory = tempdir().unwrap();
        let store = CortexStore::new(directory.path().to_path_buf()).unwrap();
        let day_dir = directory.path().join("19700101");
        fs::create_dir_all(day_dir.join("talents/work")).unwrap();
        fs::write(day_dir.join("talents/plain.md"), "plain").unwrap();
        fs::write(day_dir.join("talents/work/_app_facet.json"), "{}").unwrap();
        let plain = serde_json::from_value(json!({
            "name":"plain", "day":"19700101", "output":"md"
        }))
        .unwrap();
        let facet = serde_json::from_value(json!({
            "name":"app:facet", "day":"19700101", "output":"json", "facet":"work"
        }))
        .unwrap();
        complete_with_request(&store, "one", &plain);
        complete_with_request(&store, "two", &facet);
        let rows = day_rows(&store, "19700101");
        assert_eq!(rows[0]["output_file"], "talents/plain.md");
        assert_eq!(rows[1]["output_file"], "talents/work/_app_facet.json");
    }

    #[test]
    fn day_index_summarizes_segment_and_segment_facet_output_files() {
        let directory = tempdir().unwrap();
        let store = CortexStore::new(directory.path().to_path_buf()).unwrap();
        let day_dir = directory.path().join("19700101");
        fs::create_dir_all(day_dir.join("segment/talents")).unwrap();
        fs::create_dir_all(day_dir.join("focus/other/talents/work")).unwrap();
        fs::write(day_dir.join("segment/talents/plain.md"), "plain").unwrap();
        fs::write(
            day_dir.join("focus/other/talents/work/_app_facet.json"),
            "{}",
        )
        .unwrap();
        let segment = serde_json::from_value(json!({
            "name":"plain", "day":"19700101", "output":"md", "segment":"segment"
        }))
        .unwrap();
        let facet = serde_json::from_value(json!({
            "name":"app:facet", "day":"19700101", "output":"json", "segment":"other",
            "facet":"work", "env":{"SOL_STREAM":"focus"}
        }))
        .unwrap();
        complete_with_request(&store, "one", &segment);
        complete_with_request(&store, "two", &facet);
        let rows = day_rows(&store, "19700101");
        assert_eq!(rows[0]["output_file"], "segment/talents/plain.md");
        assert_eq!(
            rows[1]["output_file"],
            "focus/other/talents/work/_app_facet.json"
        );
    }

    #[test]
    fn day_index_summarizes_output_path_override_in_day_and_journal_root() {
        let directory = tempdir().unwrap();
        let store = CortexStore::new(directory.path().to_path_buf()).unwrap();
        let day_dir = directory.path().join("19700101");
        let day_override = day_dir.join("custom.md");
        let root_override = directory.path().join("shared/output.json");
        fs::create_dir_all(root_override.parent().unwrap()).unwrap();
        fs::create_dir_all(&day_dir).unwrap();
        fs::write(&day_override, "day").unwrap();
        fs::write(&root_override, "root").unwrap();
        let in_day = serde_json::from_value(json!({
            "name":"one", "day":"19700101", "output_path":day_override
        }))
        .unwrap();
        let in_root = serde_json::from_value(json!({
            "name":"two", "day":"19700101", "output_path":root_override
        }))
        .unwrap();
        complete_with_request(&store, "one", &in_day);
        complete_with_request(&store, "two", &in_root);
        let rows = day_rows(&store, "19700101");
        assert_eq!(rows[0]["output_file"], "custom.md");
        assert_eq!(rows[1]["output_file"], "shared/output.json");
    }

    #[test]
    fn day_index_output_file_is_null_when_output_is_missing() {
        let directory = tempdir().unwrap();
        let store = CortexStore::new(directory.path().to_path_buf()).unwrap();
        let request = serde_json::from_value(json!({
            "name":"plain", "day":"19700101", "output":"md"
        }))
        .unwrap();
        complete_with_request(&store, "one", &request);
        assert!(day_rows(&store, "19700101")[0]["output_file"].is_null());
    }

    #[test]
    fn recovery_create_append_and_late_no_create_append_are_deliberately_different() {
        let directory = tempdir().unwrap();
        let store = CortexStore::new(directory.path().to_path_buf()).unwrap();
        let active = store
            .claim("conversation", "one", &request())
            .unwrap()
            .unwrap();
        fs::remove_file(&active).unwrap();
        assert!(
            !store
                .append_active(&active, &synthesized_error("one", "late"))
                .unwrap()
        );
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(&active)
            .unwrap();
        store.recover();
        assert!(active.with_file_name("one.jsonl").exists());
    }

    #[test]
    fn completion_rename_failure_is_silent_and_success_is_observable() {
        let directory = tempdir().unwrap();
        let store = CortexStore::new(directory.path().to_path_buf()).unwrap();
        let missing = store.active_path("conversation", "missing");
        store.complete("missing", &missing, Some(&request()));
        assert!(!store.talents().join("19700101.jsonl").exists());
        let active = store
            .claim("conversation", "one", &request())
            .unwrap()
            .unwrap();
        store.complete("one", &active, Some(&request()));
        assert!(active.with_file_name("one.jsonl").exists());
    }

    #[test]
    fn missing_finish_scan_and_no_create_append_do_not_resurrect_active_file() {
        let directory = tempdir().unwrap();
        let store = CortexStore::new(directory.path().to_path_buf()).unwrap();
        let active = store.active_path("conversation", "one");
        assert!(!store.has_finish(&active));
        assert!(
            !store
                .append_active(
                    &active,
                    &synthesized_error("one", "Talent exited with code 1 without finish event")
                )
                .unwrap()
        );
        assert!(!active.exists());
    }

    #[test]
    fn atomic_symlink_cleans_dangling_temporary() {
        let directory = tempdir().unwrap();
        let link = directory.path().join("chat.log");
        let temporary = temporary_link_path(&link);
        #[cfg(unix)]
        std::os::unix::fs::symlink("missing", &temporary).unwrap();
        assert!(!temporary.exists());
        assert!(temporary.is_symlink());
        atomic_symlink(&link, "chat/one.jsonl");
        assert!(!temporary.is_symlink());
        assert!(!link.exists());
    }

    #[test]
    fn temporary_link_names_are_thread_unique() {
        let directory = tempdir().unwrap();
        let link = directory.path().join("chat.log");
        let (first_tx, first_rx) = std::sync::mpsc::channel();
        let (second_tx, second_rx) = std::sync::mpsc::channel();
        let first_link = link.clone();
        let second_link = link.clone();
        let first = thread::spawn(move || first_tx.send(temporary_link_path(&first_link)).unwrap());
        let second =
            thread::spawn(move || second_tx.send(temporary_link_path(&second_link)).unwrap());
        first.join().unwrap();
        second.join().unwrap();
        assert_ne!(first_rx.recv().unwrap(), second_rx.recv().unwrap());
    }

    #[test]
    fn day_index_uses_start_provenance_and_request_timestamp() {
        let directory = tempdir().unwrap();
        let store = CortexStore::new(directory.path().to_path_buf()).unwrap();
        let mut request = request();
        request.insert("model".into(), Value::String("request-model".into()));
        let active = store
            .claim("conversation", "one", &request)
            .unwrap()
            .unwrap();
        store
            .append_active(
                &active,
                &serde_json::from_value(json!({"event":"start","model":"start-model","provider":"start-provider","ts":9000})).unwrap(),
            )
            .unwrap();
        store
            .append_active(
                &active,
                &serde_json::from_value(json!({"event":"finish","ts":2356})).unwrap(),
            )
            .unwrap();
        let mut updated = request;
        updated.insert("provider".into(), Value::String("start-provider".into()));
        store.complete("one", &active, Some(&updated));
        let row: Value = serde_json::from_str(
            &fs::read_to_string(store.talents().join("19700101.jsonl")).unwrap(),
        )
        .unwrap();
        assert_eq!(row["provider"], "start-provider");
        assert_eq!(row["model"], "start-model");
        assert_eq!(row["runtime_seconds"], 1.4);
    }

    #[test]
    fn recovery_rerun_overwrites_recovered_log_and_duplicates_day_index() {
        let directory = tempdir().unwrap();
        let store = CortexStore::new(directory.path().to_path_buf()).unwrap();
        let request = request();
        let first = store
            .claim("conversation", "one", &request)
            .unwrap()
            .unwrap();
        let _second = store
            .claim("conversation", "two", &request)
            .unwrap()
            .unwrap();
        store.recover();
        let recovered = first.with_file_name("one.jsonl");
        let recovered_text = fs::read_to_string(&recovered).unwrap();
        store.append_day_index("one", &request, &recovered);
        let rerun = store
            .claim("conversation", "one", &request)
            .unwrap()
            .unwrap();
        store
            .append_active(
                &rerun,
                &serde_json::from_value(json!({"event":"finish","ts":1200})).unwrap(),
            )
            .unwrap();
        store.complete("one", &rerun, Some(&request));
        let rerun_text = fs::read_to_string(&recovered).unwrap();
        assert_ne!(rerun_text, recovered_text);
        let rows = fs::read_to_string(store.talents().join("19700101.jsonl")).unwrap();
        assert_eq!(
            rows.lines()
                .filter(|line| line.contains("\"use_id\":\"one\""))
                .count(),
            2
        );
    }

    #[test]
    fn safe_name_cannot_escape_the_talents_directory() {
        assert_eq!(safe_name("conversation"), "conversation");
        assert_eq!(safe_name("app:name"), "app--name");
        assert_eq!(safe_name("foo/../etc"), "foo-..-etc");
        assert_eq!(safe_name(".."), "_invalid");
        let directory = tempdir().unwrap();
        let store = CortexStore::new(directory.path().to_path_buf()).unwrap();
        let escaped = store.active_path("foo/../../outside", "one");
        assert!(escaped.starts_with(store.talents()));
    }

    #[test]
    fn day_index_ignores_a_path_shaped_day() {
        let directory = tempdir().unwrap();
        let store = CortexStore::new(directory.path().to_path_buf()).unwrap();
        let mut request = request();
        request.insert("day".into(), Value::String("../outside".into()));
        store.append_day_index("1000", &request, Path::new("/missing"));
        assert!(!directory.path().join("outside.jsonl").exists());
        assert!(!store.talents().join("../outside.jsonl").exists());
    }
}
