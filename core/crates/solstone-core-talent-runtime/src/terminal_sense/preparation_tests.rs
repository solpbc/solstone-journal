// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use super::*;
use std::{fs, path::PathBuf};

fn setup(
    screen: &str,
) -> (
    tempfile::TempDir,
    crate::prepare::RuntimePaths,
    ExecutionContext,
    serde_json::Map<String, Value>,
) {
    let root = tempfile::tempdir().unwrap();
    let journal = root.path().join("journal");
    let talent_root = root.path().join("talents");
    let templates_dir = root.path().join("templates");
    for path in [
        &talent_root,
        &templates_dir,
        &journal.join("config"),
        &journal.join("chronicle/20261021/device/100000_300"),
    ] {
        fs::create_dir_all(path).unwrap();
    }
    let payload = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../payload/solstone");
    for (from, to) in [
        ("talent/sense.md", talent_root.join("sense.md")),
        (
            "talent/sense.schema.json",
            talent_root.join("sense.schema.json"),
        ),
        (
            "think/templates/segment_preamble.md",
            templates_dir.join("segment_preamble.md"),
        ),
    ] {
        fs::copy(payload.join(from), to).unwrap();
    }
    fs::create_dir_all(journal.join("facets/work")).unwrap();
    fs::write(
        journal.join("facets/work/facet.json"),
        r#"{"title":"Work"}"#,
    )
    .unwrap();
    fs::write(
        journal.join("config/journal.json"),
        json!({"providers":{"active":{"provider":"test","model":"test-model"}}}).to_string(),
    )
    .unwrap();
    let raw = json!({"timestamp":0.001,"analysis":{"primary":"tmux","visual_description":"A terminal"},"content":{"tmux":{"session":"main","window":{"id":"@0","index":0,"name":"work"},"windows":[{"id":"@0","index":0,"name":"work","active":true}],"panes":[{"id":"%0","index":0,"active":true,"content":screen}]}}});
    fs::write(
        journal.join("chronicle/20261021/device/100000_300/tmux_main_screen.jsonl"),
        format!("{raw}\n"),
    )
    .unwrap();
    let paths = crate::prepare::RuntimePaths {
        talent_root,
        apps_root: root.path().join("apps"),
        templates_dir,
    };
    (
        root,
        paths,
        ExecutionContext { journal },
        json!({"name":"sense","day":"20261021","stream":"device","segment":"100000_300"})
            .as_object()
            .unwrap()
            .clone(),
    )
}
fn display() -> String {
    format!(
        "A complete displayed briefing explains the earlier investigation.\n\n{}\n❯ Is the implementation ready?\n{}\n permissions: shift+tab to cycle\n",
        "─".repeat(40),
        "─".repeat(40)
    )
}
#[test]
fn native_loaded_source_bytes_and_identities_qualify_only_when_they_match() {
    let (_root, paths, context, request) = setup(&display());
    let mut prepared = crate::prepare::prepare(
        request,
        &paths,
        &context,
        crate::prepare::PrepareMode::Preview,
    )
    .unwrap();
    assert!(
        prepared.terminal_sense.is_some(),
        "matching native input must take the candidate: {}",
        inspect(&prepared, None).unwrap()["diagnostic"]
    );
    prepared.terminal_sense = None;
    assert!(prepare(&prepared, &context).is_some());
    let mut wrong = prepared.clone();
    wrong.entry_sources[0].as_mut().unwrap().records[0]["content"]["tmux"]["panes"][0]["content"] =
        json!("different bytes");
    assert!(prepare(&wrong, &context).is_none());
    let mut wrong = prepared.clone();
    wrong.entry_sources[0].as_mut().unwrap().path = PathBuf::from("different identity");
    assert!(prepare(&wrong, &context).is_none());
    let mut wrong = prepared.clone();
    wrong.entry_sources[0] = None;
    assert!(prepare(&wrong, &context).is_none());
    let mut wrong = prepared.clone();
    wrong.config["source_counts"]["transcripts"] = json!(1);
    assert!(prepare(&wrong, &context).is_none());
    let mut wrong = prepared;
    wrong.config.insert("_memory_sources".into(), json!([{}]));
    assert!(prepare(&wrong, &context).is_none());
}
#[test]
fn native_preparation_falls_back_for_unsupported_ui_styles() {
    for text in [
        display().replace("❯ Is", "❯ \x1b[2mIs"),
        display().replace("❯ Is", "❯ \x1b[9mIs"),
        display().replace("❯ Is", "❯ \x1b[2JIs"),
        "plain terminal without a known field".to_owned(),
    ] {
        let (_root, paths, context, mut request) = setup(&text);
        request.insert("_terminal_sense_binding".into(), json!({"current":"spoof"}));
        let p = crate::prepare::prepare(
            request,
            &paths,
            &context,
            crate::prepare::PrepareMode::Preview,
        )
        .unwrap();
        assert!(p.terminal_sense.is_none());
    }
}
#[test]
fn every_thinking_backend_prepares_the_same_candidate() {
    let (_root, paths, context, request) = setup(&display());
    let mut candidates = Vec::new();
    for config in [
        json!({"providers":{"active":{"provider":"test","model":"test-model"}}}),
        json!({"providers":{"active":{"provider":"local","model":"local/qwen3.5-4b"}}}),
        json!({"providers":{"active":{"provider":"local","model":"local/qwen3.5-4b"},"local":{"endpoint_url":"https://example.test","served_model_id":"Qwen/Qwen3.5-4B"}},"services":{"confidential":{}}}),
        json!({"providers":{"active":{"provider":"local","model":"local/other"},"local":{"endpoint_url":"https://example.test","served_model_id":"other","served_context_window":16384}}}),
        json!({"providers":{"active":{"provider":"openai","model":"openai-model"}}}),
        json!({"providers":{"active":{"provider":"anthropic","model":"anthropic-model"}}}),
        json!({"providers":{"active":{"provider":"google","model":"google-model"}}}),
    ] {
        fs::write(
            context.journal.join("config/journal.json"),
            config.to_string(),
        )
        .unwrap();
        let p = crate::prepare::prepare(
            request.clone(),
            &paths,
            &context,
            crate::prepare::PrepareMode::Preview,
        )
        .unwrap();
        let candidate = p
            .terminal_sense
            .expect("terminal input takes the candidate");
        candidates
            .push(solstone_core_generate::encode_one_shot_request(&candidate.request).unwrap());
    }
    assert!(candidates.windows(2).all(|pair| pair[0] == pair[1]));
}
#[test]
fn ordinary_fitting_trims_the_timeline_from_complete_screens() {
    use solstone_core_local::generate::{estimate_tokens, fit_contents};
    let (_root, paths, context, request) = setup(&display());
    let frames: String = (0..60)
        .map(|i| {
            let log: String = (0..=i).map(|j| format!("build step {j} finished\n")).collect();
            let raw = json!({"timestamp":0.001 + 5.0 * f64::from(i),"analysis":{"primary":"tmux","visual_description":"A terminal"},"content":{"tmux":{"session":"main","window":{"id":"@0","index":0,"name":"work"},"windows":[{"id":"@0","index":0,"name":"work","active":true}],"panes":[{"id":"%0","index":0,"active":true,"content":format!("{log}{}", display())}]}}});
            format!("{raw}\n")
        })
        .collect();
    fs::write(
        context
            .journal
            .join("chronicle/20261021/device/100000_300/tmux_main_screen.jsonl"),
        frames,
    )
    .unwrap();
    let p = crate::prepare::prepare(
        request,
        &paths,
        &context,
        crate::prepare::PrepareMode::Preview,
    )
    .unwrap();
    let request = p
        .terminal_sense
        .expect("terminal input takes the candidate")
        .request;
    let contents = Value::Array(
        request
            .contents
            .iter()
            .map(|part| match part {
                ContentPart::Text { text } => json!(text),
                ContentPart::Image { .. } => panic!("terminal Sense sends text"),
            })
            .collect(),
    );
    let system = request.system_instruction.as_deref();
    let max_output_tokens = u32::try_from(request.max_output_tokens).unwrap();
    let (mut refused, mut whole, mut from_complete_screens) = (false, false, 0);
    for window in (1024..=262_144).step_by(512) {
        let mut count = estimate_tokens;
        match fit_contents(&contents, system, max_output_tokens, window, &mut count) {
            Err(error) => {
                assert_eq!(error.reason_code, "context_preserved_overflow");
                assert!(!whole && from_complete_screens == 0);
                refused = true;
            }
            Ok((fitted, None)) => {
                assert_eq!(fitted, contents);
                whole = true;
                break;
            }
            Ok((fitted, Some(budget))) => {
                assert!(budget.clipped);
                // Only the timeline is trimmed; instructions, cards and passages stay whole.
                assert_eq!(
                    fitted.as_array().unwrap()[1..],
                    contents.as_array().unwrap()[1..]
                );
                let kept = fitted[0].as_str().unwrap();
                let kept = &kept[kept.find("\n\n").unwrap() + 2..];
                if !kept.starts_with("## ") {
                    continue;
                }
                from_complete_screens += 1;
                let first = &kept[..kept.find("End observation.\n").unwrap()];
                let observation = first
                    .lines()
                    .find_map(|line| line.strip_prefix("Observation: "))
                    .unwrap();
                let observation: Value = serde_json::from_str(observation).unwrap();
                assert_eq!(observation["roster_update"]["mode"], "full");
                assert!(observation.get("restart").is_none());
                let panes = first
                    .lines()
                    .filter(|line| line.starts_with("Pane change: "))
                    .collect::<Vec<_>>();
                assert!(!panes.is_empty());
                assert!(panes.iter().all(|line| line.contains(r#""mode":"full""#)));
            }
        }
    }
    assert!(refused && whole && from_complete_screens > 1);
}
