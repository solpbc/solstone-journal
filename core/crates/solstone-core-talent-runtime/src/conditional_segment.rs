// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The two early segment skips whose applicability can be established again.
//! Recovery also requires exact use receipts and the current segment
//! publication/input guards. A prior output has no implicit retirement owner.

use crate::{
    ExecutionContext, RuntimeOutcome,
    contract::GateDecision,
    prepare::{self, RuntimePaths},
};
use serde_json::{Map, Value};
use solstone_core_journal_io::paths::Segment;

fn request_stream(request: &Map<String, Value>) -> Result<&str, String> {
    match request.get("stream") {
        None => Ok(solstone_core_journal_io::DEFAULT_STREAM),
        Some(Value::String(stream)) if !stream.is_empty() => Ok(stream),
        _ => Err("conditional skip stream is invalid".into()),
    }
}

pub fn skip_is_applicable(
    request: &Map<String, Value>,
    paths: &RuntimePaths,
    context: &ExecutionContext,
    outcome: &RuntimeOutcome,
) -> Result<bool, String> {
    let name = match outcome {
        RuntimeOutcome::Skipped {
            stage,
            talent,
            reason,
        } if stage == "documents"
            && talent == "documents"
            && reason == "not a document import segment" =>
        {
            "documents"
        }
        RuntimeOutcome::PrepareSkipped { talent, reason }
            if talent == "screen" && reason == "missing_required_percepts" =>
        {
            "screen"
        }
        _ => return Ok(false),
    };
    if request.get("name").and_then(Value::as_str) != Some(name) {
        return Ok(false);
    }
    let coordinate = |key| {
        request
            .get(key)
            .and_then(Value::as_str)
            .ok_or_else(|| format!("conditional skip lacks {key}"))
    };
    let day = coordinate("day")?;
    let stream = request_stream(request)?;
    let key = coordinate("segment")?;
    if let Some(env) = request.get("env") {
        for (field, expected) in [
            ("SOL_DAY", day),
            ("SOL_STREAM", stream),
            ("SOL_SEGMENT", key),
        ] {
            if !(field == "SOL_STREAM"
                && stream == solstone_core_journal_io::DEFAULT_STREAM
                && env.get(field).is_none())
                && env.get(field).and_then(Value::as_str) != Some(expected)
            {
                return Err("conditional skip environment changed".into());
            }
        }
    }
    if let Some(unit) = request.get("_work_unit") {
        let expected = solstone_core_system::work_retry::Unit::Segment {
            day: day.into(),
            stream: stream.into(),
            segment: key.into(),
            talent: name.into(),
            direct: stream == solstone_core_journal_io::DEFAULT_STREAM,
        };
        if serde_json::from_value::<solstone_core_system::work_retry::Unit>(unit.clone())
            .map_err(|e| e.to_string())?
            != expected
        {
            return Err("conditional skip work unit changed".into());
        }
    }
    let segment = Segment::resolve_exact(&context.journal, day, stream, key)
        .map_err(|e| e.to_string())?
        .ok_or("conditional skip segment disappeared")?;
    if let Some(value) = request.get("output_path") {
        let path =
            std::path::Path::new(value.as_str().ok_or("conditional output path is invalid")?);
        if path.parent() != Some(segment.path().join("talents").as_path())
            || path.file_stem().and_then(|v| v.to_str()) != Some(name)
            || !matches!(
                path.extension().and_then(|v| v.to_str()),
                Some("json" | "md")
            )
        {
            return Err("conditional output target changed".into());
        }
    }
    if !output_absent(&context.journal, &segment, name)? {
        return Ok(false);
    }
    let prepared = prepare::prepare(
        request.clone(),
        paths,
        context,
        prepare::PrepareMode::Preview,
    )
    .map_err(|e| e.to_string())?;
    if name == "documents" {
        return Ok(prepared.config.get("skip_reason").is_none()
            && crate::documents::gate(&prepared, context).map_err(|e| e.to_string())?
                == GateDecision::Skip("not a document import segment".into()));
    }
    Ok(prepared.config.get("skip_reason").and_then(Value::as_str)
        == Some("missing_required_percepts")
        && prepared
            .config
            .get("source_counts")
            .and_then(|c| c.get("percepts"))
            .and_then(Value::as_u64)
            == Some(0)
        && solstone_core_transcripts::screen_inputs_absent(&context.journal, &segment)?)
}

pub(crate) fn validate_skip(
    outcome: RuntimeOutcome,
    request: &Map<String, Value>,
    paths: &RuntimePaths,
    context: &ExecutionContext,
) -> RuntimeOutcome {
    let name = match &outcome {
        RuntimeOutcome::Skipped {
            stage,
            talent,
            reason,
        } if stage == "documents"
            && talent == "documents"
            && reason == "not a document import segment" =>
        {
            "documents"
        }
        RuntimeOutcome::PrepareSkipped { talent, reason }
            if talent == "screen" && reason == "missing_required_percepts" =>
        {
            "screen"
        }
        _ => return outcome,
    };
    match skip_is_applicable(request, paths, context, &outcome) {
        Ok(true) => outcome,
        result => RuntimeOutcome::StageFailed(crate::StageError::new(
            "prepare",
            "conditional_input",
            name,
            result
                .err()
                .unwrap_or_else(|| "conditional input or prior output needs reconciliation".into()),
        )),
    }
}

fn output_absent(journal: &std::path::Path, segment: &Segment, name: &str) -> Result<bool, String> {
    #[cfg(unix)]
    use solstone_core_journal_io::{
        FlatDirectory as Directory, list_flat_directory as list, open_flat_directory_bound as open,
    };
    use solstone_core_journal_io::{JournalEntryKind, JournalRoot};
    #[cfg(windows)]
    use solstone_core_journal_io::{
        WindowsFlatDirectory as Directory, list_windows_flat_directory as list,
        open_windows_flat_directory_bound as open,
    };
    fn acquire(root: &JournalRoot, relative: &std::path::Path) -> Result<Directory, String> {
        root.revalidate().map_err(|e| e.to_string())?;
        let mut parent: Option<Directory> = None;
        let mut diagnostic = root.canonical_path().to_path_buf();
        for part in relative.components() {
            let std::path::Component::Normal(name) = part else {
                return Err("invalid conditional output directory".into());
            };
            parent = Some(
                match &parent {
                    Some(parent) => open(parent, name, &diagnostic),
                    None => open(root, name, &diagnostic),
                }
                .map_err(|e| e.to_string())?
                .ok_or("conditional output directory disappeared")?,
            );
            diagnostic.push(name);
        }
        parent.ok_or("empty conditional output directory".into())
    }
    fn identity(dir: &Directory) -> solstone_core_journal_io::ObjectIdentity {
        #[cfg(unix)]
        {
            dir.identity()
        }
        #[cfg(windows)]
        {
            dir.object_identity()
        }
    }
    let root = JournalRoot::open(journal).map_err(|e| e.to_string())?;
    let relative = segment
        .path()
        .strip_prefix(journal)
        .map_err(|e| e.to_string())?;
    let directory = acquire(&root, relative)?;
    let entries = list(&directory, 1024)
        .map_err(|e| e.to_string())?
        .ok_or("conditional output inventory exceeds budget")?;
    // Native case aliases must never become proof that the canonical owner
    // directory is absent. Refuse noncanonical spellings on every platform.
    for entry in &entries {
        let spelling = entry
            .name
            .to_str()
            .ok_or("conditional directory name is not UTF-8")?;
        if spelling.to_uppercase() == "TALENTS" && spelling != "talents" {
            return Err("conditional output directory spelling needs reconciliation".into());
        }
    }
    let mut absent = true;
    let talent_directory = if let Some(entry) = entries.iter().find(|entry| entry.name == "talents")
    {
        if entry.kind != JournalEntryKind::Directory {
            return Err("conditional output parent is not a directory".into());
        }
        let child = open(&directory, &entry.name, segment.path())
            .map_err(|e| e.to_string())?
            .ok_or("conditional output parent disappeared")?;
        if !child.matches_entry(entry) {
            return Err("conditional output parent changed".into());
        }
        let children = list(&child, 1024)
            .map_err(|e| e.to_string())?
            .ok_or("conditional output inventory exceeds budget")?;
        for entry in &children {
            let file = entry
                .name
                .to_str()
                .ok_or("conditional output name is not UTF-8")?;
            // A differently cased leaf can still be this output on Windows.
            let folded = file.to_uppercase();
            let owner = name.to_ascii_uppercase();
            if folded == owner || folded.starts_with(&format!("{owner}.")) {
                absent = false;
            }
        }
        Some((child, children))
    } else {
        None
    };
    let rebound = acquire(&root, relative)?;
    if identity(&directory) != identity(&rebound)
        || list(&directory, 1024).map_err(|e| e.to_string())?.as_ref() != Some(&entries)
        || list(&rebound, 1024).map_err(|e| e.to_string())?.as_ref() != Some(&entries)
    {
        return Err("conditional output parent changed".into());
    }
    if let Some((child, children)) = talent_directory {
        let current = acquire(&root, &relative.join("talents"))?;
        if identity(&child) != identity(&current)
            || list(&child, 1024).map_err(|e| e.to_string())?.as_ref() != Some(&children)
            || list(&current, 1024).map_err(|e| e.to_string())?.as_ref() != Some(&children)
        {
            return Err("conditional output inventory changed".into());
        }
    }
    root.revalidate().map_err(|e| e.to_string())?;
    Ok(absent)
}

/// A stored early skip must contain only its request, start and exact finish.
/// Any Generate attempt, error, cancellation or extra terminal refuses it.
#[allow(clippy::too_many_arguments)]
pub fn receipt_is_applicable(
    events: &[Value],
    use_id: &str,
    name: &str,
    day: &str,
    stream: &str,
    segment: &str,
    paths: &RuntimePaths,
    context: &ExecutionContext,
) -> Result<bool, String> {
    let [request, start, finish] = events else {
        return Ok(false);
    };
    let Some(request) = request.as_object() else {
        return Ok(false);
    };
    if request.contains_key("event")
        || request.get("use_id").and_then(Value::as_str) != Some(use_id)
        || request.get("name").and_then(Value::as_str) != Some(name)
        || request.get("day").and_then(Value::as_str) != Some(day)
        || request_stream(request)? != stream
        || request.get("segment").and_then(Value::as_str) != Some(segment)
        || start["event"] != "start"
        || start["name"] != name
        || finish["event"] != "finish"
        || finish["name"] != name
        || finish.get("output").is_some()
    {
        return Ok(false);
    }
    let Some(reason) = finish.get("skip_reason").and_then(Value::as_str) else {
        return Ok(false);
    };
    let outcome = match finish.get("stage") {
        Some(Value::String(stage)) => RuntimeOutcome::Skipped {
            stage: stage.into(),
            talent: name.into(),
            reason: reason.into(),
        },
        None => RuntimeOutcome::PrepareSkipped {
            talent: name.into(),
            reason: reason.into(),
        },
        _ => return Ok(false),
    };
    skip_is_applicable(request, paths, context, &outcome)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use solstone_core_generate::{
        Cancellation, GenerateRequest, GenerateResponse, GeneratedResponse,
    };
    use std::{
        fs,
        sync::atomic::{AtomicUsize, Ordering},
    };

    fn fixture(
        name: &str,
        stream: &str,
    ) -> (
        tempfile::TempDir,
        RuntimePaths,
        ExecutionContext,
        Map<String, Value>,
    ) {
        let root = tempfile::tempdir().unwrap();
        let paths = RuntimePaths {
            talent_root: root.path().join("talent"),
            apps_root: root.path().join("apps"),
            templates_dir: root.path().join("templates"),
        };
        for path in [&paths.talent_root, &paths.apps_root, &paths.templates_dir] {
            fs::create_dir_all(path).unwrap();
        }
        let extra = if name == "documents" {
            r#", "hook":{"pre":"documents"}, "load":{"transcripts":true,"percepts":false,"talents":false}"#
        } else {
            r#", "load":{"transcripts":true,"percepts":"required","talents":false}"#
        };
        fs::write(paths.talent_root.join(format!("{name}.md")), format!("{{\n\"type\":\"generate\",\"schedule\":\"segment\",\"output\":\"json\",\"max_output_tokens\":1024{extra}\n}}\nSegment record")).unwrap();
        let context = ExecutionContext {
            journal: root.path().join("journal"),
        };
        fs::create_dir_all(context.journal.join("config")).unwrap();
        fs::write(
            context.journal.join("config/journal.json"),
            r#"{"providers":{"active":{"provider":"test","model":"test-model"}}}"#,
        )
        .unwrap();
        let day = context.journal.join("chronicle/20260101");
        let directory = if stream == solstone_core_journal_io::DEFAULT_STREAM {
            day
        } else {
            day.join(stream)
        }
        .join("090000_60");
        fs::create_dir_all(directory.join("talents")).unwrap();
        fs::write(
            directory.join("talents/sense.json"),
            r#"{"density":"active","content_type":"work"}"#,
        )
        .unwrap();
        fs::write(directory.join("audio.jsonl"), "{\"start\":\"00:00:01\",\"text\":\"A sufficiently detailed synthetic meeting about the garden project.\"}\n").unwrap();
        let unit = solstone_core_system::work_retry::Unit::Segment {
            day: "20260101".into(),
            stream: stream.into(),
            segment: "090000_60".into(),
            talent: name.into(),
            direct: stream == solstone_core_journal_io::DEFAULT_STREAM,
        };
        let mut request = json!({"name":name,"use_id":"conditional-use","day":"20260101","stream":stream,"segment":"090000_60","output_path":directory.join("talents").join(format!("{name}.json")),"_work_unit":unit}).as_object().unwrap().clone();
        if stream == solstone_core_journal_io::DEFAULT_STREAM {
            request.remove("stream");
        }
        (root, paths, context, request)
    }

    fn run(
        request: &Map<String, Value>,
        paths: &RuntimePaths,
        context: &ExecutionContext,
    ) -> (RuntimeOutcome, Vec<Value>, usize) {
        let calls = AtomicUsize::new(0);
        let generate = |_: &GenerateRequest| {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok(GenerateResponse::Generated(Box::new(GeneratedResponse {
                id: None,
                text: "{\"summary\":\"A synthetic record.\"}".into(),
                model: "test-model".into(),
                finish_reason: "stop".into(),
                usage: json!({}),
                thinking: None,
                schema_validation: None,
                input_budget: None,
                request_budget: None,
                inference: None,
                hints_applied: vec![],
            })))
        };
        let mut bytes = Vec::new();
        let outcome = crate::execute_request(
            request.clone(),
            paths,
            context,
            &generate,
            &mut bytes,
            &Cancellation::never(),
        );
        crate::emit_outcome(&mut bytes, outcome.clone());
        let mut events = vec![Value::Object(request.clone())];
        events.extend(
            String::from_utf8(bytes)
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str::<Value>(line).unwrap()),
        );
        (outcome, events, calls.load(Ordering::SeqCst))
    }

    #[test]
    fn actual_conditional_skips_have_receipts_and_do_not_accumulate_retry_failures() {
        for (name, stream) in [
            ("documents", "notes"),
            ("screen", "notes"),
            ("documents", solstone_core_journal_io::DEFAULT_STREAM),
            ("screen", solstone_core_journal_io::DEFAULT_STREAM),
        ] {
            let (_root, paths, context, request) = fixture(name, stream);
            for _ in 0..6 {
                let (outcome, events, calls) = run(&request, &paths, &context);
                assert_eq!(calls, 0);
                assert_eq!(
                    skip_is_applicable(&request, &paths, &context, &outcome),
                    Ok(true),
                    "{outcome:?}"
                );
                assert_eq!(
                    receipt_is_applicable(
                        &events,
                        "conditional-use",
                        name,
                        "20260101",
                        stream,
                        "090000_60",
                        &paths,
                        &context
                    ),
                    Ok(true),
                    "{events:?}"
                );
            }
            let unit: solstone_core_system::work_retry::Unit =
                serde_json::from_value(request["_work_unit"].clone()).unwrap();
            let record =
                solstone_core_system::work_retry::read(&context.journal, &unit.id().unwrap())
                    .unwrap()
                    .unwrap();
            assert!(!record.held);
            assert_eq!(serde_json::to_value(&record).unwrap()["failure_count"], 0);
            let input =
                solstone_core_system::work_retry::input::current(&context.journal, &unit).unwrap();
            solstone_core_system::work_retry::observe_held(
                &context.journal,
                &unit,
                "missing_credential",
                &input,
            )
            .unwrap();
            let (outcome, _, calls) = run(&request, &paths, &context);
            assert!(matches!(outcome, RuntimeOutcome::StageFailed(_)));
            assert_eq!(calls, 0);
            assert!(
                solstone_core_system::work_retry::read(&context.journal, &unit.id().unwrap())
                    .unwrap()
                    .unwrap()
                    .held
            );
        }
    }

    #[test]
    fn conditional_receipt_refuses_changed_identity_or_non_early_outcomes() {
        let (_root, paths, context, request) = fixture("screen", "notes");
        let (_, events, _) = run(&request, &paths, &context);
        let accept = |events: &[Value]| {
            receipt_is_applicable(
                events,
                "conditional-use",
                "screen",
                "20260101",
                "notes",
                "090000_60",
                &paths,
                &context,
            )
            .unwrap_or(false)
        };
        assert!(accept(&events));
        for field in ["use_id", "name", "day", "stream", "segment"] {
            let mut changed = events.clone();
            changed[0][field] = json!("changed");
            assert!(!accept(&changed), "{field}");
        }
        for field in ["skip_reason", "stage", "name"] {
            let mut changed = events.clone();
            changed[2][field] = json!("unknown");
            assert!(!accept(&changed), "{field}");
        }
        for event in ["generate_input", "generate", "error", "cancelled", "finish"] {
            let mut changed = events.clone();
            changed.insert(2, json!({"event":event}));
            assert!(!accept(&changed), "{event}");
        }
        let directory = std::path::Path::new(request["output_path"].as_str().unwrap())
            .parent()
            .unwrap()
            .parent()
            .unwrap();
        for name in [
            "xscreen.jsonl",
            "browser_a.jsonl",
            "x_protected_video.jsonl",
            "x_desktop_activity.jsonl",
            "raw.MP4",
        ] {
            fs::write(directory.join(name), "malformed").unwrap();
            assert!(!accept(&events), "{name}");
            fs::remove_file(directory.join(name)).unwrap();
        }
    }

    #[test]
    fn conditional_skip_refuses_prior_artifacts_including_case_aliases() {
        for name in ["documents", "screen"] {
            for leaf in [
                format!("{name}.json"),
                format!("{name}.JSON").to_ascii_uppercase(),
                format!("{name}.md.sources.json"),
            ] {
                let (_root, paths, context, request) = fixture(name, "notes");
                let path = std::path::Path::new(request["output_path"].as_str().unwrap())
                    .with_file_name(leaf);
                let old = br#"{"summary":"prior saved output"}"#;
                fs::write(&path, old).unwrap();
                let (outcome, _, calls) = run(&request, &paths, &context);
                assert!(
                    matches!(outcome, RuntimeOutcome::StageFailed(_)),
                    "{name}: {outcome:?}"
                );
                assert_eq!(calls, 0);
                assert_eq!(fs::read(path).unwrap(), old);
            }
        }
    }

    #[test]
    fn conditional_skip_refuses_parent_case_alias_and_ambiguous_population() {
        for name in ["documents", "screen"] {
            for spelling in ["TALENTS", "Talents", "talentſ"] {
                let (_root, paths, context, mut request) = fixture(name, "notes");
                request.remove("_work_unit");
                let original = std::path::Path::new(request["output_path"].as_str().unwrap())
                    .parent()
                    .unwrap();
                let renamed = original.with_file_name(spelling);
                fs::rename(original, &renamed).unwrap();
                let output = renamed.join(format!("{name}.json").to_ascii_uppercase());
                let old = b"prior saved output";
                fs::write(&output, old).unwrap();
                let (outcome, _, calls) = run(&request, &paths, &context);
                assert!(
                    matches!(outcome, RuntimeOutcome::StageFailed(_)),
                    "{spelling}: {outcome:?}"
                );
                assert_eq!(calls, 0);
                assert_eq!(fs::read(&output).unwrap(), old);
                // A second canonical directory cannot erase the uncertainty.
                #[cfg(unix)]
                {
                    fs::create_dir(original).unwrap();
                    let (outcome, _, _) = run(&request, &paths, &context);
                    assert!(matches!(outcome, RuntimeOutcome::StageFailed(_)));
                }
            }
        }
    }

    #[test]
    fn applicable_document_and_screen_generate_and_publish_in_callable_host() {
        for (name, stream) in [("documents", "import.document"), ("screen", "notes")] {
            let (_root, paths, context, request) = fixture(name, stream);
            if name == "screen" {
                let path = std::path::Path::new(request["output_path"].as_str().unwrap())
                    .parent()
                    .unwrap()
                    .parent()
                    .unwrap()
                    .join("x_desktop_activity.jsonl");
                fs::write(path, "{\"v\":1,\"t\":\"desktop_activity\",\"kind\":\"foreground\",\"method\":\"ax\",\"ts\":1000,\"end_ts\":2000,\"app\":\"Synthetic garden planning\"}\n").unwrap();
            }
            let (outcome, events, calls) = run(&request, &paths, &context);
            assert!(
                matches!(
                    outcome,
                    RuntimeOutcome::Finished {
                        disposition: crate::contract::CommitDisposition::Written,
                        ..
                    }
                ),
                "{outcome:?}"
            );
            assert_eq!(calls, 1);
            assert!(std::path::Path::new(request["output_path"].as_str().unwrap()).is_file());
            assert!(
                !receipt_is_applicable(
                    &events,
                    "conditional-use",
                    name,
                    "20260101",
                    stream,
                    "090000_60",
                    &paths,
                    &context
                )
                .unwrap_or(false)
            );
        }
    }
}
