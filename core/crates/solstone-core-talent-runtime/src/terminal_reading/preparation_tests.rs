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
        ("talent/screen.md", talent_root.join("screen.md")),
        (
            "talent/screen.schema.json",
            talent_root.join("screen.schema.json"),
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
    fs::write(
        journal.join("chronicle/20261021/device/100000_300/tmux_main_screen.jsonl"),
        typing_frames(screen),
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
/// Two captures of a screen whose `❯` row is being typed: the row first holds
/// only its prompt, then the whole input, with the cursor at its end. A screen
/// with no such row is captured once, with no cursor fields.
fn typing_frames(screen: &str) -> String {
    let frame = |time: f64, content: &str, cursor: Option<(usize, usize)>| {
        let mut pane = json!({"id":"%0","index":0,"left":0,"top":0,"width":80,"height":24,"active":true,"content":content});
        if let Some((x, y)) = cursor {
            for (key, value) in [
                ("cursor_x", json!(x)),
                ("cursor_y", json!(y)),
                ("cursor_visible", json!(true)),
                ("alternate_on", json!(true)),
                ("pane_in_mode", json!(false)),
            ] {
                pane[key] = value;
            }
        }
        let raw = json!({"timestamp":time,"analysis":{"primary":"tmux","visual_description":"A terminal"},"content":{"tmux":{"session":"main","window":{"id":"@0","index":0,"name":"work"},"windows":[{"id":"@0","index":0,"name":"work","active":true}],"panes":[pane]}}});
        format!("{raw}\n")
    };
    let lines: Vec<&str> = screen.split('\n').collect();
    let Some(y) = lines.iter().position(|l| l.starts_with("❯ ")) else {
        return frame(0.001, screen, None);
    };
    let typed = lines[y].split("\x1b[2m").next().unwrap();
    let x = typed.chars().count();
    let mut prompt = lines.clone();
    prompt[y] = "❯ ";
    frame(0.001, &prompt.join("\n"), Some((2, y))) + &frame(5.001, screen, Some((x, y)))
}

fn display() -> String {
    format!(
        "A complete displayed briefing explains the earlier investigation.\n\n{}\n❯ Is the implementation ready?\n{}\n permissions: shift+tab to cycle\n",
        "─".repeat(40),
        "─".repeat(40)
    )
}

/// The request as preparation built it before terminal sampling.
fn unsampled(
    request: &serde_json::Map<String, Value>,
    paths: &crate::prepare::RuntimePaths,
    context: &ExecutionContext,
) -> PreparedTalent {
    SKIP.with(|skip| skip.set(true));
    let prepared = crate::prepare::prepare(
        request.clone(),
        paths,
        context,
        crate::prepare::PrepareMode::Preview,
    );
    SKIP.with(|skip| skip.set(false));
    prepared.unwrap()
}

fn source_text(prepared: &PreparedTalent) -> String {
    let request = crate::native_generate_request(prepared).unwrap();
    let solstone_core_generate::ContentPart::Text { text } = &request.contents[0] else {
        panic!("source part");
    };
    text.clone()
}

#[test]
fn the_request_cortex_sends_reads_the_sampled_terminal() {
    let (_root, paths, context, mut request) = setup(&display());
    // Cortex sends every segment talent an empty prompt.
    request.insert("prompt".into(), json!(""));
    let prepared = crate::prepare::prepare(
        request,
        &paths,
        &context,
        crate::prepare::PrepareMode::Preview,
    )
    .unwrap();
    let event = prepared
        .terminal_reading
        .as_ref()
        .expect("a terminal-only segment is sampled");
    assert_eq!(event["branch"], "sampled");
    assert_eq!(event["typed"], 1);
    let text = source_text(&prepared);
    assert!(text.contains(sample::GUIDE));
    assert!(
        text.contains("· typed\n❯ Is the implementation ready?"),
        "{text}"
    );
    assert!(text.contains("already on screen at its first capture (earlier work)"));
    assert!(text.contains("Typed at the keyboard in this segment, in order:\n- 10:00:05 · window \"work\": ❯ Is the implementation ready?\n"));
    assert!(
        !text.contains("\"panes\""),
        "the raw capture is replaced: {text}"
    );
}

#[test]
fn only_terminal_only_sources_bound_to_their_bytes_are_sampled() {
    let (_root, paths, context, request) = setup(&display());
    let raw = unsampled(&request, &paths, &context);
    assert!(prepare(&mut raw.clone(), &context).is_some());
    let mut wrong = raw.clone();
    wrong.entry_sources[0].as_mut().unwrap().records[0]["content"]["tmux"]["panes"][0]["content"] =
        json!("different bytes");
    let mut wrong_path = raw.clone();
    wrong_path.entry_sources[0].as_mut().unwrap().path = PathBuf::from("different identity");
    let mut missing = raw.clone();
    missing.entry_sources[0] = None;
    let mut spoken = raw.clone();
    spoken.config["source_counts"]["transcripts"] = json!(1);
    let mut remembered = raw.clone();
    remembered
        .config
        .insert("_memory_sources".into(), json!([{}]));
    for mut prepared in [wrong, wrong_path, missing, spoken, remembered] {
        let before = prepared.config["transcript"].clone();
        assert!(prepare(&mut prepared, &context).is_none());
        assert_eq!(
            prepared.config["transcript"], before,
            "an ineligible segment keeps the ordinary request"
        );
    }
}

#[test]
fn a_segment_larger_than_the_floor_window_is_sampled_to_fit_it() {
    // Sixty captures, each adding a hundred new lines: far over the floor window.
    let frames: String = (0..60)
        .map(|n| {
            let content: String = (0..100).map(|l| format!("build step {n} wrote output line {l} for module {}\n", n * 100 + l)).collect();
            let raw = json!({"timestamp":f64::from(n) * 5.0,"analysis":{"primary":"tmux"},"content":{"tmux":{"session":"main","window":{"id":"@0","name":"work"},"panes":[{"id":"%0","active":true,"content":content}]}}});
            format!("{raw}\n")
        })
        .collect();
    let (_root, paths, context, request) = setup("");
    fs::write(
        context
            .journal
            .join("chronicle/20261021/device/100000_300/tmux_main_screen.jsonl"),
        frames,
    )
    .unwrap();
    let prepared = crate::prepare::prepare(
        request,
        &paths,
        &context,
        crate::prepare::PrepareMode::Preview,
    )
    .unwrap();
    let event = prepared.terminal_reading.as_ref().expect("sampled");
    assert!(
        event["line_cap"].is_u64() || event["entries_left_out"].as_u64() > Some(0),
        "{event}"
    );
    let request = crate::native_generate_request(&prepared).unwrap();
    let used: u32 = request
        .contents
        .iter()
        .map(|part| match part {
            solstone_core_generate::ContentPart::Text { text } => solstone_core_tokens::count(text),
            _ => 0,
        })
        .sum();
    let floor = solstone_core_local::generate::compute_input_budget(
        u32::try_from(request.max_output_tokens).unwrap(),
        solstone_core_local::plan::LOCAL_MIN_CONTEXT_TOKENS,
    );
    assert!(used <= floor, "{used} tokens over the floor budget {floor}");
    let text = source_text(&prepared);
    assert!(
        text.contains("build step 59 wrote output line 0 "),
        "the segment's end is read"
    );
    assert!(
        text.contains("build step 0 wrote output line 99 "),
        "and its first screen's end"
    );
}

#[test]
fn screen_reads_the_same_sampled_terminal_in_one_request() {
    let (_root, paths, context, mut request) = setup(&display());
    request.insert("name".into(), json!("screen"));
    request.insert("prompt".into(), json!(""));
    let prepared = crate::prepare::prepare(
        request,
        &paths,
        &context,
        crate::prepare::PrepareMode::Preview,
    )
    .unwrap();
    let event = prepared
        .terminal_reading
        .as_ref()
        .expect("a terminal-only Screen segment is sampled");
    assert_eq!(event["talent"], "screen");
    assert!(!prepared.config.contains_key("_screen_batch_cuts"));
    let text = source_text(&prepared);
    assert!(text.contains(sample::GUIDE));
    assert!(
        text.contains("· typed\n❯ Is the implementation ready?"),
        "{text}"
    );
    assert!(
        !text.contains("**Tmux window:**"),
        "the Screen projection is replaced: {text}"
    );
}

#[test]
fn a_line_shown_in_the_segment_before_that_comes_back_is_not_read_as_new() {
    let capture = |frames: &[(f64, &str)]| -> String {
        frames
            .iter()
            .map(|(time, content)| {
                let raw = json!({"timestamp":time,"analysis":{"primary":"tmux"},"content":{"tmux":{"session":"main","window":{"id":"@0","name":"work"},"panes":[{"id":"%0","active":true,"content":content}]}}});
                format!("{raw}\n")
            })
            .collect()
    };
    let (_root, paths, context, request) = setup("");
    let day = context.journal.join("chronicle/20261021/device");
    fs::write(
        day.join("100000_300/tmux_main_screen.jsonl"),
        capture(&[
            (0.0, "$ make\n"),
            (5.0, "$ make\nbuilding\n"),
            (
                10.0,
                "⏺ Fixed the typo in setup.sh\n$ make\nbuilding\nbuild finished\n",
            ),
        ]),
    )
    .unwrap();
    let read = |request: &serde_json::Map<String, Value>| {
        let prepared = crate::prepare::prepare(
            request.clone(),
            &paths,
            &context,
            crate::prepare::PrepareMode::Preview,
        )
        .unwrap();
        assert!(prepared.terminal_reading.is_some());
        source_text(&prepared)
    };
    let alone = read(&request);
    assert!(
        alone.contains("new text\n```\n⏺ Fixed the typo in setup.sh\n"),
        "{alone}"
    );
    // The stream's segment before showed the fix; a later one, or another stream's, does not count.
    for (dir, line) in [
        ("device/095500_300", "⏺ Fixed the typo in setup.sh\n"),
        ("device/100500_300", "build finished\n"),
        ("other/095500_300", "building\n"),
    ] {
        let dir = context.journal.join("chronicle/20261021").join(dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("tmux_main_screen.jsonl"), capture(&[(0.0, line)])).unwrap();
    }
    let text = read(&request);
    assert!(!text.contains("⏺ Fixed the typo in setup.sh"), "{text}");
    assert!(text.contains("new text\n```\nbuilding\n"), "{text}");
    assert!(text.contains("new text\n```\nbuild finished\n"), "{text}");
}

#[test]
fn a_session_taken_in_on_output_alone_is_read_as_nobody_at_its_keyboard() {
    // Output alone leaves the cursor out and keeps the pane mode; a capture
    // from before the cursor fields has neither, and says nothing about who.
    let capture = |cursor: bool, mode: bool| -> String {
        [(0.0, "⏺ Running the replays\n"), (5.0, "⏺ Running the replays\n⏺ Ran five replays\n")]
            .iter()
            .map(|(time, content)| {
                let mut pane = json!({"id":"%0","active":true,"content":content});
                if cursor {
                    pane["cursor_x"] = json!(0);
                    pane["cursor_y"] = json!(0);
                    pane["cursor_visible"] = json!(true);
                }
                if mode {
                    pane["alternate_on"] = json!(true);
                    pane["pane_in_mode"] = json!(false);
                }
                let raw = json!({"timestamp":time,"analysis":{"primary":"tmux"},"content":{"tmux":{"session":"qa","window":{"id":"@0","name":"work"},"panes":[pane]}}});
                format!("{raw}\n")
            })
            .collect()
    };
    for (cursor, mode, labelled) in [
        (false, true, true),
        (true, true, false),
        (false, false, false),
    ] {
        let (_root, paths, context, request) = setup("");
        fs::write(
            context
                .journal
                .join("chronicle/20261021/device/100000_300/tmux_main_screen.jsonl"),
            capture(cursor, mode),
        )
        .unwrap();
        let prepared = crate::prepare::prepare(
            request.clone(),
            &paths,
            &context,
            crate::prepare::PrepareMode::Preview,
        )
        .unwrap();
        let text = source_text(&prepared);
        assert_eq!(
            text.contains(&super::sample::output_only_line("qa")),
            labelled,
            "cursor {cursor}, mode {mode}: {text}"
        );
    }
}

#[test]
fn a_listed_session_a_client_used_is_still_nobodys_when_nothing_was_typed() {
    // A capture on output alone shows the owner listed the session; a later
    // capture with the cursor only means a client used it. Typing decides.
    let frame = |time: f64, content: &str, cursor: Option<(usize, usize)>| -> String {
        let mut pane = json!({"id":"%0","active":true,"content":content,"alternate_on":true,"pane_in_mode":false});
        if let Some((x, y)) = cursor {
            pane["cursor_x"] = json!(x);
            pane["cursor_y"] = json!(y);
            pane["cursor_visible"] = json!(true);
        }
        let raw = json!({"timestamp":time,"analysis":{"primary":"tmux"},"content":{"tmux":{"session":"qa","window":{"id":"@0","name":"work"},"panes":[pane]}}});
        format!("{raw}\n")
    };
    let untyped = frame(0.0, "⏺ Running the replays\n", None)
        + &frame(
            5.0,
            "⏺ Running the replays\n⏺ Ran five replays\n",
            Some((0, 2)),
        );
    let typed = frame(0.0, "⏺ Ran five replays\n❯ ", Some((2, 1)))
        + &frame(5.0, "⏺ Ran five replays\n❯ now score them", Some((16, 1)))
        + &frame(
            10.0,
            "⏺ Ran five replays\n❯ now score them\n⏺ Scoring\n",
            None,
        );
    for (frames, labelled) in [(untyped, true), (typed, false)] {
        let (_root, paths, context, request) = setup("");
        fs::write(
            context
                .journal
                .join("chronicle/20261021/device/100000_300/tmux_main_screen.jsonl"),
            &frames,
        )
        .unwrap();
        let prepared = crate::prepare::prepare(
            request.clone(),
            &paths,
            &context,
            crate::prepare::PrepareMode::Preview,
        )
        .unwrap();
        let text = source_text(&prepared);
        assert_eq!(
            text.contains(&super::sample::output_only_line("qa")),
            labelled,
            "{frames}: {text}"
        );
    }
}
