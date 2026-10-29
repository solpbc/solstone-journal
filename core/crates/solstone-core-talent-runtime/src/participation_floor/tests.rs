// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use solstone_core_generate::ContentPart;
use solstone_core_transcripts::{ENTRY_SOURCE_MAX_BYTES, EntrySource};

use super::*;
use crate::ExecutionContext;
use crate::prepare::{PrepareMode, RuntimePaths, prepare};

fn test_scratch_root() -> PathBuf {
    if cfg!(windows) {
        std::env::temp_dir()
    } else {
        PathBuf::from("/var/tmp")
    }
}

fn setup_env_in(
    root: tempfile::TempDir,
    day: &str,
) -> (tempfile::TempDir, RuntimePaths, ExecutionContext) {
    let journal = root.path().join("journal");
    let talent_root = root.path().join("talents");
    let templates_dir = root.path().join("templates");

    for p in [
        &talent_root,
        &templates_dir,
        &journal.join("config"),
        &journal.join("facets/work"),
        &journal.join("chronicle").join(day),
    ] {
        fs::create_dir_all(p).unwrap();
    }

    let payload = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../payload/solstone");
    for (from, to) in [
        (
            "talent/participation.md",
            talent_root.join("participation.md"),
        ),
        (
            "talent/participation.schema.json",
            talent_root.join("participation.schema.json"),
        ),
        ("talent/sense.md", talent_root.join("sense.md")),
        (
            "talent/sense.schema.json",
            talent_root.join("sense.schema.json"),
        ),
        ("talent/work.md", talent_root.join("work.md")),
        (
            "talent/story.schema.json",
            talent_root.join("story.schema.json"),
        ),
        (
            "talent/conversation.md",
            talent_root.join("conversation.md"),
        ),
        (
            "talent/conversation.schema.json",
            talent_root.join("conversation.schema.json"),
        ),
        (
            "think/templates/activity_preamble.md",
            templates_dir.join("activity_preamble.md"),
        ),
        (
            "think/templates/segment_preamble.md",
            templates_dir.join("segment_preamble.md"),
        ),
    ] {
        fs::copy(payload.join(from), to).unwrap();
    }

    fs::write(
        journal.join("facets/work/facet.json"),
        json!({"title":"Work","description":"Daily engineering work"}).to_string(),
    )
    .unwrap();

    fs::write(
        journal.join("config/journal.json"),
        json!({"providers":{"active":{"provider":"none"}}}).to_string(),
    )
    .unwrap();

    let paths = RuntimePaths {
        talent_root,
        apps_root: root.path().join("apps"),
        templates_dir,
    };

    (root, paths, ExecutionContext { journal })
}

fn setup_env(day: &str) -> (tempfile::TempDir, RuntimePaths, ExecutionContext) {
    let root = tempfile::tempdir().unwrap();
    setup_env_in(root, day)
}

fn setup_memory_env(day: &str) -> (tempfile::TempDir, RuntimePaths, ExecutionContext) {
    let root = tempfile::Builder::new()
        .prefix("solstone-participation-mem-")
        .tempdir_in(test_scratch_root())
        .unwrap();
    setup_env_in(root, day)
}

fn tmux_screen_jsonl(segment_key: &str, frame_count: usize) -> String {
    let mut out = String::new();
    for f in 0..frame_count {
        let time = f as f64 * 10.0;
        let p0 = json!({
            "id": "%0", "index": 0, "left": 0, "top": 0, "width": 80, "height": 24, "active": true,
            "content": format!("S{segment_key}-F{f}-P0 cmd\n")
        });
        let p1 = json!({
            "id": "%1", "index": 1, "left": 0, "top": 0, "width": 80, "height": 24, "active": false,
            "content": format!("S{segment_key}-F{f}-P1 log\n")
        });
        let raw = json!({
            "timestamp": time,
            "analysis": {"primary": "tmux", "visual_description": "Terminal"},
            "content": {
                "tmux": {
                    "session": "main",
                    "window": {"id": "@0", "index": 0, "name": "work"},
                    "windows": [{"id": "@0", "index": 0, "name": "work", "active": true}],
                    "panes": [p0, p1]
                }
            }
        });
        out.push_str(&raw.to_string());
        out.push('\n');
    }
    out
}

fn assert_fit_contents(prepared: &crate::PreparedTalent) {
    let contents = crate::generate_contents(prepared);
    let parts = Value::Array(
        contents
            .iter()
            .map(|p| match p {
                ContentPart::Text { text } => json!({"type": "text", "text": text}),
                _ => json!({}),
            })
            .collect(),
    );

    let max_output_tokens = prepared
        .config
        .get("max_output_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(6912) as u32;

    let mut count = solstone_core_tokens::count;

    let (_, budget32) = solstone_core_local::generate::fit_contents(
        &parts,
        None,
        max_output_tokens,
        32_768,
        &mut count,
    )
    .unwrap();
    assert!(
        budget32.is_none(),
        "unexpected clip at window 32,768: {budget32:?}"
    );

    let (_, budget256) = solstone_core_local::generate::fit_contents(
        &parts,
        None,
        max_output_tokens,
        262_144,
        &mut count,
    )
    .unwrap();
    assert!(
        budget256.is_none(),
        "unexpected clip at window 262,144: {budget256:?}"
    );
}

// Case 1: 288 terminal-only segments across 20261029
#[test]
fn test_case_1_terminal_only_288_segments() {
    let day = "20261029";
    let (_root, paths, context) = setup_env(day);

    let mut span = Vec::new();
    for i in 0..288 {
        let s = i * 300;
        let h = s / 3600;
        let m = (s % 3600) / 60;
        let sec = s % 60;
        let key = format!("{h:02}{m:02}{sec:02}_300");
        let seg_dir = context.journal.join("chronicle").join(day).join(&key);
        fs::create_dir_all(&seg_dir).unwrap();
        fs::write(
            seg_dir.join("main_screen.jsonl"),
            tmux_screen_jsonl(&key, 2),
        )
        .unwrap();
        span.push(key);
    }

    let request = json!({
        "name": "participation",
        "day": day,
        "facet": "work",
        "span": span,
        "activity": {
            "id": "act-288-term",
            "destination_id": "act-288-term",
            "segments": span,
            "activity_type": "coding",
            "started_at": "2026-10-29T00:00:00Z",
            "ended_at": "2026-10-29T23:59:59Z"
        }
    });

    let prepared = prepare(
        request.as_object().unwrap().clone(),
        &paths,
        &context,
        PrepareMode::Preview,
    )
    .unwrap();

    assert_fit_contents(&prepared);

    let event = prepared
        .participation_floor
        .as_ref()
        .expect("event present");
    let transcript = prepared.config["transcript"].as_str().unwrap();

    let entries_omitted = event["entries_omitted"].as_u64().unwrap() as usize;
    assert!(entries_omitted > 0);

    // Verify dropped runs match entries_omitted and time spans are real heading times
    let mut sum_dropped = 0;
    for line in transcript.lines() {
        if line.contains(" entries left out ") {
            let parts: Vec<&str> = line.split_whitespace().collect();
            let count: usize = parts[0].parse().unwrap();
            sum_dropped += count;
            assert_eq!(parts[1], "entries");
            assert_eq!(parts[2], "left");
            assert_eq!(parts[3], "out");
            let start = parts[4];
            assert_eq!(parts[5], "-");
            let end = parts[6];
            assert!(parts[7].starts_with('('));
            let unit = parts[8].trim_end_matches(')');
            assert!(unit == "frames" || unit == "lines");
            assert_eq!(start.len(), 8);
            assert_eq!(end.len(), 8);
        }
    }
    assert_eq!(sum_dropped, entries_omitted);

    // Kept frames include first segment's first frame id and last segment's last frame id
    let first_frame = format!("S{}-F0-P0", span.first().unwrap());
    let last_frame = format!("S{}-F1-P0", span.last().unwrap());
    assert!(transcript.contains(&first_frame));
    assert!(transcript.contains(&last_frame));
}

// Case 2: 288 mixed segments
#[test]
fn test_case_2_mixed_288_segments() {
    let day = "20261029";
    let (_root, paths, context) = setup_env(day);

    let mut span = Vec::new();
    for i in 0..288 {
        let s = i * 300;
        let h = s / 3600;
        let m = (s % 3600) / 60;
        let sec = s % 60;
        let key = format!("{h:02}{m:02}{sec:02}_300");
        let seg_dir = context.journal.join("chronicle").join(day).join(&key);
        fs::create_dir_all(&seg_dir).unwrap();
        fs::write(
            seg_dir.join("main_screen.jsonl"),
            tmux_screen_jsonl(&key, 1),
        )
        .unwrap();
        fs::write(
            seg_dir.join("app_desktop_activity.jsonl"),
            format!("{{\"app\": \"terminal\", \"window\": \"win{i}\"}}\n"),
        )
        .unwrap();
        fs::write(
            seg_dir.join("browser_chrome.jsonl"),
            format!("{{\"url\": \"https://example.com/{i}\", \"title\": \"Page {i}\"}}\n"),
        )
        .unwrap();
        span.push(key);
    }

    let request = json!({
        "name": "participation",
        "day": day,
        "facet": "work",
        "span": span,
        "activity": {
            "id": "act-288-mixed",
            "destination_id": "act-288-mixed",
            "segments": span,
            "activity_type": "coding"
        }
    });

    let prepared = prepare(
        request.as_object().unwrap().clone(),
        &paths,
        &context,
        PrepareMode::Preview,
    )
    .unwrap();

    assert_fit_contents(&prepared);

    let event = prepared
        .participation_floor
        .as_ref()
        .expect("event present");
    let transcript = prepared.config["transcript"].as_str().unwrap();

    let entries_omitted = event["entries_omitted"].as_u64().unwrap() as usize;
    assert!(entries_omitted > 0);

    let mut sum_dropped = 0;
    for line in transcript.lines() {
        if line.contains(" entries left out ") {
            let parts: Vec<&str> = line.split_whitespace().collect();
            let count: usize = parts[0].parse().unwrap();
            sum_dropped += count;
            assert_eq!(parts[1], "entries");
            assert_eq!(parts[2], "left");
            assert_eq!(parts[3], "out");
            let start = parts[4];
            assert_eq!(parts[5], "-");
            let end = parts[6];
            assert!(parts[7].starts_with('('));
            let unit = parts[8].trim_end_matches(')');
            assert!(unit == "frames" || unit == "lines");
            assert_eq!(start.len(), 8);
            assert_eq!(end.len(), 8);
        }
    }
    assert_eq!(sum_dropped, entries_omitted);
}

// Case 3: Screen-share call (speech + Sense > 3/4 share, >= 100 screen entries)
#[test]
fn test_case_3_screen_share_call() {
    let day = "20261029";
    let (_root, paths, context) = setup_env(day);

    let mut span = Vec::new();
    for i in 0..100 {
        let h = 10 + i / 60;
        let m = i % 60;
        let key = format!("{:02}{:02}00_60", h, m);
        let seg_dir = context.journal.join("chronicle").join(day).join(&key);
        fs::create_dir_all(&seg_dir).unwrap();
        fs::write(
            seg_dir.join("main_screen.jsonl"),
            tmux_screen_jsonl(&key, 1),
        )
        .unwrap();

        // Speech entries
        let mut audio = String::new();
        for l in 0..15 {
            let text = if i == 0 && l == 0 {
                "SPEECH_FIRST line in first segment".to_string()
            } else if i == 99 && l == 14 {
                "SPEECH_LAST line in last segment".to_string()
            } else {
                format!("speaker line {l} for segment {key} with sufficient words")
            };
            audio.push_str(&format!(
                "{{\"start\": \"00:00:{l:02}\", \"text\": \"{text}\"}}\n"
            ));
        }
        fs::write(seg_dir.join("audio.jsonl"), audio).unwrap();

        // Sense entry
        let sense_dir = seg_dir.join("talents");
        fs::create_dir_all(&sense_dir).unwrap();
        let summary =
            "Sense summary observation discussing architecture in detail with relevant topics "
                .repeat(25);
        fs::write(sense_dir.join("sense.md"), &summary).unwrap();

        span.push(key);
    }

    let request = json!({
        "name": "participation",
        "day": day,
        "facet": "work",
        "span": span,
        "activity": {
            "id": "act-screen-share",
            "destination_id": "act-screen-share",
            "segments": span,
            "activity_type": "meeting"
        }
    });

    let prepared = prepare(
        request.as_object().unwrap().clone(),
        &paths,
        &context,
        PrepareMode::Preview,
    )
    .unwrap();

    assert_fit_contents(&prepared);

    let event = prepared
        .participation_floor
        .as_ref()
        .expect("event present");
    let transcript = prepared.config["transcript"].as_str().unwrap();

    let speech_kept = event["speech_lines_kept"].as_u64().unwrap();
    let speech_omitted = event["speech_lines_omitted"].as_u64().unwrap();
    let sense_kept = event["sense_kept"].as_u64().unwrap();
    let sense_omitted = event["sense_omitted"].as_u64().unwrap();

    assert!(speech_kept > 0 && speech_omitted > 0);
    assert!(sense_kept > 0 && sense_omitted > 0);

    assert!(transcript.contains("SPEECH_FIRST"));
    assert!(transcript.contains("SPEECH_LAST"));

    let sense_left_out_lines: Vec<&str> = transcript
        .lines()
        .filter(|l| l.starts_with("Sense left out "))
        .collect();
    assert_eq!(sense_left_out_lines.len(), sense_omitted as usize);

    assert!(transcript.contains(&format!("Speech lines left out: {speech_omitted}")));

    let screen_omitted =
        event["entries_omitted"].as_u64().unwrap() + event["frames_omitted"].as_u64().unwrap();
    assert!(screen_omitted > 0);
    assert!(transcript.contains("frames left out") || transcript.contains("entries left out"));
}

// Case 4: Oversized speech file (> 64 KiB) through loader, then sample_to_budget
#[test]
fn test_case_4_oversized_speech_file() {
    let day = "20261029";
    let (_root, _paths, context) = setup_env(day);

    let seg_key = "090000_300";
    let seg_dir = context.journal.join("chronicle").join(day).join(seg_key);
    fs::create_dir_all(&seg_dir).unwrap();

    let mut audio = String::new();
    audio.push_str("{\"start\": \"00:00:00\", \"text\": \"OVERSIZED_FIRST line discussion\", \"speaker\": 1}\n");
    while audio.len() <= ENTRY_SOURCE_MAX_BYTES + 2048 {
        audio.push_str("{\"start\": \"00:00:01\", \"text\": \"Discussion about system performance and caching strategy over time.\", \"speaker\": 1}\n");
    }
    audio.push_str(
        "{\"start\": \"00:05:00\", \"text\": \"OVERSIZED_LAST line discussion\", \"speaker\": 1}\n",
    );
    fs::write(seg_dir.join("audio.jsonl"), &audio).unwrap();

    let config = json!({
        "name": "participation",
        "day": day,
        "segment": seg_key,
        "sources": {
            "transcripts": true,
            "percepts": false,
            "talents": false
        }
    });

    let loaded =
        crate::transcript::load_transcript(&context.journal, config.as_object().unwrap()).unwrap();

    assert_eq!(loaded.sources.len(), 1);
    assert!(
        loaded.sources[0].is_none(),
        "expected source to be None for file > 64 KiB"
    );

    let sample =
        sample_to_budget(&loaded.text, &loaded.sources, &context.journal, day, 500).unwrap();

    assert!(sample.transcript.contains("OVERSIZED_FIRST"));
    assert!(sample.transcript.contains("OVERSIZED_LAST"));

    let event = &sample.event;
    let omitted = event["speech_lines_omitted"].as_u64().unwrap();
    assert!(omitted > 0);
    assert!(
        sample
            .transcript
            .contains(&format!("Speech lines left out: {omitted}"))
    );
}

// Case 5: Fixed budget sample_to_budget
#[test]
fn test_case_5_fixed_budget_sample_to_budget() {
    let transcript = "## 2026-10-29 09:00:00 - 09:05:00\n\n### Screen Activity\n### 09:00:00\nF0\n### 09:01:00\nF1\n### 09:02:00\nF2\n### 09:03:00\nF3\n### 09:04:00\nF4\n### 09:05:00\nF5\n\n";
    let sources: Vec<Option<EntrySource>> = vec![Some(EntrySource {
        path: PathBuf::from("mock_screen.jsonl"),
        sha256: "sha".into(),
        rendered_range: 0..0,
        stream: None,
        file: "mock_screen.jsonl".into(),
        chunks: Vec::new(),
        chunk_offsets: Vec::new(),
        records: Vec::new(),
        unparsed_lines: 0,
        voice_named: std::collections::BTreeSet::new(),
    })];

    let journal = Path::new("/var/tmp");
    let total_tokens = solstone_core_tokens::count(transcript);

    // Total tokens ~ 60, budget will thin 6 frames to 4 frames (F0, F1, F3, F5)
    let s1 =
        sample_to_budget(transcript, &sources, journal, "20261029", total_tokens - 10).unwrap();
    assert!(s1.transcript.contains("2 frames left out"));
    assert_eq!(s1.event["frames_omitted"], 2);
    assert!(s1.transcript.contains("F0"));
    assert!(s1.transcript.contains("F1"));
    assert!(!s1.transcript.contains("F2"));
    assert!(s1.transcript.contains("F3"));
    assert!(!s1.transcript.contains("F4"));
    assert!(s1.transcript.contains("F5"));

    let s2 = sample_to_budget(transcript, &sources, journal, "20261029", 5000).unwrap();
    assert_eq!(s2.transcript, transcript);
    assert!(!s2.transcript.contains("frames left out"));
    assert_eq!(s2.event["frames_omitted"], 0);
    assert_eq!(s2.event["entries_omitted"], 0);

    // Call s1 again: deterministic
    let s1_again =
        sample_to_budget(transcript, &sources, journal, "20261029", total_tokens - 10).unwrap();
    assert_eq!(s1.transcript, s1_again.transcript);
    assert_eq!(s1.event, s1_again.event);
}

// Case 6: Facet that leaves room
#[test]
fn test_case_6_facet_leaves_room() {
    let day = "20261029";
    let (_root, paths, context) = setup_env(day);

    let large_desc = "Architecture and design planning ".repeat(300);
    fs::write(
        context.journal.join("facets/work/facet.json"),
        json!({"title":"Work","description": large_desc}).to_string(),
    )
    .unwrap();

    let mut span = Vec::new();
    for i in 0..50 {
        let key = format!("10{:02}00_60", i % 60);
        let seg_dir = context.journal.join("chronicle").join(day).join(&key);
        fs::create_dir_all(&seg_dir).unwrap();
        fs::write(
            seg_dir.join("main_screen.jsonl"),
            tmux_screen_jsonl(&key, 2),
        )
        .unwrap();
        span.push(key);
    }

    let request = json!({
        "name": "participation",
        "day": day,
        "facet": "work",
        "span": span,
        "activity": {
            "id": "act-facet-leaves-room",
            "destination_id": "act-facet-leaves-room",
            "segments": span,
            "activity_type": "coding"
        }
    });

    let prepared = prepare(
        request.as_object().unwrap().clone(),
        &paths,
        &context,
        PrepareMode::Preview,
    )
    .unwrap();

    assert_fit_contents(&prepared);
    assert!(prepared.config.get("input_budget").is_none());
}

// Case 7: Facet description alone over the floor
#[test]
fn test_case_7_facet_alone_over_floor() {
    let day = "20261029";
    let (_root, paths, context) = setup_env(day);

    let huge_desc = "Massive context expansion description word ".repeat(6000);
    fs::write(
        context.journal.join("facets/work/facet.json"),
        json!({"title":"Work","description": huge_desc}).to_string(),
    )
    .unwrap();

    let seg_key = "100000_60";
    let seg_dir = context.journal.join("chronicle").join(day).join(seg_key);
    fs::create_dir_all(&seg_dir).unwrap();
    fs::write(
        seg_dir.join("main_screen.jsonl"),
        tmux_screen_jsonl(seg_key, 2),
    )
    .unwrap();

    let request = json!({
        "name": "participation",
        "day": day,
        "facet": "work",
        "span": [seg_key],
        "activity": {
            "id": "act-over-floor",
            "destination_id": "act-over-floor",
            "segments": [seg_key],
            "activity_type": "coding"
        }
    });

    // 1. With served_context_window at 32768
    fs::write(
        context.journal.join("config/journal.json"),
        json!({"providers":{"active":{"provider":"none"},"local":{"served_context_window":32768}}})
            .to_string(),
    )
    .unwrap();

    let prep1 = prepare(
        request.as_object().unwrap().clone(),
        &paths,
        &context,
        PrepareMode::Preview,
    )
    .unwrap();

    let t1 = prep1.config["transcript"].as_str().unwrap().to_owned();
    let event1 = prep1.participation_floor.as_ref().unwrap().clone();
    let disc1 = event1["disclosure"].as_str().unwrap();
    assert!(t1.contains(disc1));

    // 2. With served_context_window at 262144
    fs::write(
        context.journal.join("config/journal.json"),
        json!({"providers":{"active":{"provider":"none"},"local":{"served_context_window":262144}}}).to_string(),
    )
    .unwrap();

    let prep2 = prepare(
        request.as_object().unwrap().clone(),
        &paths,
        &context,
        PrepareMode::Preview,
    )
    .unwrap();

    let t2 = prep2.config["transcript"].as_str().unwrap().to_owned();
    let event2 = prep2.participation_floor.as_ref().unwrap().clone();

    assert_eq!(t1, t2);
    assert_eq!(event1, event2);

    // No sampled frame bodies in either transcript
    assert!(!t1.contains("S100000_60-F0"));
    assert!(!t2.contains("S100000_60-F0"));

    // Disclosure contains the three omitted population counts
    assert!(disc1.contains("0 speech lines"));
    assert!(disc1.contains("0 Sense segments"));
    assert!(disc1.contains("1 other entry"));

    // Fit contents at 262144 returns no clip
    let contents = crate::generate_contents(&prep2);
    let parts = Value::Array(
        contents
            .iter()
            .map(|p| match p {
                ContentPart::Text { text } => json!({"type": "text", "text": text}),
                _ => json!({}),
            })
            .collect(),
    );
    let mut count = solstone_core_tokens::count;
    let (_, budget256) =
        solstone_core_local::generate::fit_contents(&parts, None, 6912, 262_144, &mut count)
            .unwrap();
    assert!(budget256.is_none());

    // Fit contents at 32768 allows context_preserved_overflow
    let res32 = solstone_core_local::generate::fit_contents(&parts, None, 6912, 32_768, &mut count);
    match res32 {
        Ok((_, None)) => {}
        Err(e) if e.reason_code == "context_preserved_overflow" => {}
        Ok((_, Some(b))) => panic!("unexpected clip at 32,768: {b:?}"),
        Err(e) => panic!("unexpected err at 32,768: {e:?}"),
    }
}

// Case 8: Served window does not change normal sampled activity
#[test]
fn test_case_8_served_window_invariant() {
    let day = "20261029";
    let (_root, paths, context) = setup_env(day);

    let mut span = Vec::new();
    for i in 0..60 {
        let key = format!("10{:02}00_60", i % 60);
        let seg_dir = context.journal.join("chronicle").join(day).join(&key);
        fs::create_dir_all(&seg_dir).unwrap();
        fs::write(
            seg_dir.join("main_screen.jsonl"),
            tmux_screen_jsonl(&key, 2),
        )
        .unwrap();
        span.push(key);
    }

    let request = json!({
        "name": "participation",
        "day": day,
        "facet": "work",
        "span": span,
        "activity": {
            "id": "act-served-window",
            "destination_id": "act-served-window",
            "segments": span,
            "activity_type": "coding"
        }
    });

    fs::write(
        context.journal.join("config/journal.json"),
        json!({"providers":{"active":{"provider":"none"},"local":{"served_context_window":32768}}})
            .to_string(),
    )
    .unwrap();
    let prep1 = prepare(
        request.as_object().unwrap().clone(),
        &paths,
        &context,
        PrepareMode::Preview,
    )
    .unwrap();

    fs::write(
        context.journal.join("config/journal.json"),
        json!({"providers":{"active":{"provider":"none"},"local":{"served_context_window":262144}}}).to_string(),
    )
    .unwrap();
    let prep2 = prepare(
        request.as_object().unwrap().clone(),
        &paths,
        &context,
        PrepareMode::Preview,
    )
    .unwrap();

    assert_eq!(
        prep1.config["transcript"].as_str().unwrap(),
        prep2.config["transcript"].as_str().unwrap()
    );
    assert_fit_contents(&prep1);
    assert_fit_contents(&prep2);
}

// Case 9: Switch off
#[test]
fn test_case_9_switch_off() {
    let day = "20261029";
    let (_root, paths, context) = setup_env(day);

    // Temp copy of participation with sample_to_floor false
    fs::write(
        paths.talent_root.join("participation_off.md"),
        r#"{
  "type": "generate",
  "title": "Participation",
  "schedule": "activity",
  "activities": ["*"],
  "max_output_tokens": 6912,
  "load": {
    "sample_to_floor": false,
    "transcripts": true,
    "percepts": true,
    "talents": {"sense": true}
  }
}
$activity_preamble"#,
    )
    .unwrap();
    fs::copy(
        paths.talent_root.join("participation.schema.json"),
        paths.talent_root.join("participation_off.schema.json"),
    )
    .unwrap();

    let mut span = Vec::new();
    for i in 0..288 {
        let s = i * 300;
        let h = s / 3600;
        let m = (s % 3600) / 60;
        let sec = s % 60;
        let key = format!("{h:02}{m:02}{sec:02}_300");
        let seg_dir = context.journal.join("chronicle").join(day).join(&key);
        fs::create_dir_all(&seg_dir).unwrap();
        fs::write(
            seg_dir.join("main_screen.jsonl"),
            tmux_screen_jsonl(&key, 2),
        )
        .unwrap();
        span.push(key);
    }

    let request = json!({
        "name": "participation_off",
        "day": day,
        "facet": "work",
        "span": span,
        "activity": {
            "id": "act-switch-off",
            "destination_id": "act-switch-off",
            "segments": span,
            "activity_type": "coding"
        }
    });

    let prep_part = prepare(
        request.as_object().unwrap().clone(),
        &paths,
        &context,
        PrepareMode::Preview,
    )
    .unwrap();

    let transcript = prep_part.config["transcript"].as_str().unwrap();
    assert!(!transcript.contains("left out"));
    assert!(prep_part.participation_floor.is_none());

    // Work and conversation also contain every frame id and no left out note
    let mut req_work = request.clone();
    req_work["name"] = json!("work");
    let prep_work = prepare(
        req_work.as_object().unwrap().clone(),
        &paths,
        &context,
        PrepareMode::Preview,
    )
    .unwrap();
    let work_t = prep_work.config["transcript"].as_str().unwrap();
    assert!(!work_t.contains("left out"));

    let mut req_conv = request.clone();
    req_conv["name"] = json!("conversation");
    let prep_conv = prepare(
        req_conv.as_object().unwrap().clone(),
        &paths,
        &context,
        PrepareMode::Preview,
    )
    .unwrap();
    let conv_t = prep_conv.config["transcript"].as_str().unwrap();
    assert!(!conv_t.contains("left out"));

    for key in &span {
        for f in 0..2 {
            for pane in 0..2 {
                let frame_id = format!("S{key}-F{f}-P{pane}");
                assert!(
                    transcript.contains(&frame_id),
                    "participation missing {frame_id}"
                );
                assert!(work_t.contains(&frame_id), "work missing {frame_id}");
                assert!(
                    conv_t.contains(&frame_id),
                    "conversation missing {frame_id}"
                );
            }
        }
    }
}

// Case 10: Pool kinds
#[test]
fn test_case_10_pool_kinds() {
    let day = "20261029";
    let (_root, paths, context) = setup_memory_env(day);

    let seg_key = "100000_60";

    // Valid agent-memory original
    let source = solstone_core_format::agent_memory::SourceKey::from_verified_id("test-source");
    let coordinate = solstone_core_format::agent_memory::Coordinate {
        day: day.into(),
        stream: format!("agent-memory-{}", source.component()),
        segment: seg_key.into(),
    };
    let mem_seg = context
        .journal
        .join("chronicle")
        .join(&coordinate.day)
        .join(&coordinate.stream)
        .join(&coordinate.segment);
    fs::create_dir_all(&mem_seg).unwrap();

    fs::write(
        mem_seg.join("document_transcript.md"),
        "DOC_TOKEN line 1\nDOC_TOKEN line 2\nDOC_TOKEN line 3\n",
    )
    .unwrap();
    fs::write(
        mem_seg.join("image_transcript.md"),
        "IMAGE_TOKEN line 1\nIMAGE_TOKEN line 2\n",
    )
    .unwrap();
    fs::write(
        mem_seg.join("workout.json"),
        json!({"workout": {"name": "WORKOUT_TOKEN run", "type": "Run", "distance_m": 5000.0}})
            .to_string(),
    )
    .unwrap();
    fs::write(
        mem_seg.join(format!("{seg_key}_protected_video.jsonl")),
        json!({"t": "protected_video", "ts": 1792027800000i64, "end_ts": 1792030500000i64, "service": "Netflix", "title": "VIDEO_TOKEN"}).to_string() + "\n",
    )
    .unwrap();

    let mem_bytes = b"MEM_TOKEN trusted note content for agent memory";
    let record: solstone_core_format::agent_memory::OperationRecord =
        serde_json::from_value(json!({
            "operation_id": "operation-1",
            "digest": solstone_core_format::agent_memory::digest(mem_bytes),
            "byte_count": mem_bytes.len(),
            "created_at": "2026-10-29T10:00:00Z",
            "origin_kind": "agent_memory",
            "creation_label": "connection label",
            "coordinate": coordinate,
            "phase": "chained",
            "chain": {"prev_day": null, "prev_segment": null, "seq": 1}
        }))
        .unwrap();
    let ready = solstone_core_format::agent_memory::ready_document(&record, &source).unwrap();
    fs::write(mem_seg.join("note.txt"), mem_bytes).unwrap();
    fs::write(
        mem_seg.join("origin.json"),
        serde_json::to_vec(&ready.origin).unwrap(),
    )
    .unwrap();
    fs::write(
        mem_seg.join("ready.json"),
        serde_json::to_vec(&ready).unwrap(),
    )
    .unwrap();
    fs::write(
        mem_seg.join("stream.json"),
        json!({"stream": &coordinate.stream, "prev_day": null, "prev_segment": null, "seq": 1})
            .to_string(),
    )
    .unwrap();

    let request = json!({
        "name": "participation",
        "day": day,
        "facet": "work",
        "span": [seg_key],
        "sources": {
            "transcripts": true,
            "percepts": true
        },
        "activity": {
            "id": "act-pool-kinds",
            "destination_id": "act-pool-kinds",
            "segments": [seg_key],
            "activity_type": "coding"
        }
    });

    let prep = prepare(
        request.as_object().unwrap().clone(),
        &paths,
        &context,
        PrepareMode::Preview,
    )
    .unwrap();

    let transcript = prep.config["transcript"].as_str().unwrap();
    assert!(transcript.contains("DOC_TOKEN"));
    assert!(transcript.contains("IMAGE_TOKEN"));
    assert!(transcript.contains("WORKOUT_TOKEN"));
    assert!(transcript.contains("VIDEO_TOKEN"));
    assert!(transcript.contains("MEM_TOKEN"));

    assert_fit_contents(&prep);

    // Separately call sample_to_budget with a small budget: each kind is pool, not speech
    let loaded =
        crate::transcript::load_transcript(&context.journal, request.as_object().unwrap()).unwrap();
    let small_sample =
        sample_to_budget(&loaded.text, &loaded.sources, &context.journal, day, 40).unwrap();
    assert_eq!(small_sample.event["speech_lines_omitted"], 0);
    assert!(!small_sample.transcript.contains("Speech lines left out"));
    let pool_omitted = small_sample.event["entries_omitted"].as_u64().unwrap()
        + small_sample.event["lines_omitted"].as_u64().unwrap();
    assert!(pool_omitted > 0);
}

// Case 11: Incomplete memory refusal
#[cfg(feature = "full-tests")]
#[test]
fn test_case_11_incomplete_memory() {
    let day = "20261029";
    let (_root, paths, context) = setup_memory_env(day);

    let source =
        solstone_core_format::agent_memory::SourceKey::from_verified_id("corrupt-memory-source");
    let stream = format!("agent-memory-{}", source.component());
    let seg_dir = context
        .journal
        .join("chronicle")
        .join(day)
        .join(&stream)
        .join("090000_60");
    fs::create_dir_all(&seg_dir).unwrap();
    fs::write(seg_dir.join("note.txt"), b"corrupt memory").unwrap();
    fs::write(seg_dir.join("ready.json"), b"invalid").unwrap();
    fs::write(
        seg_dir.join("stream.json"),
        json!({"stream": stream, "prev_day": null, "prev_segment": null, "seq": 1}).to_string(),
    )
    .unwrap();
    fs::write(
        seg_dir.join("audio.jsonl"),
        r#"{"start":"00:00:00","text":"audio transcript"}"#,
    )
    .unwrap();

    let request = json!({
        "name": "participation",
        "day": day,
        "facet": "work",
        "span": ["090000_60"],
        "activity": {
            "id": "act-corrupt-mem",
            "destination_id": "act-corrupt-mem",
            "segments": ["090000_60"],
            "activity_type": "coding"
        }
    });

    let result = prepare(
        request.as_object().unwrap().clone(),
        &paths,
        &context,
        PrepareMode::Preview,
    );

    match result {
        Err(crate::prepare::PrepareFailure::Refusal(msg)) => {
            assert_eq!(msg, "private memory context is incomplete");
        }
        other => panic!("expected PrepareFailure::Refusal, got {other:?}"),
    }
}

// Case 12: Sparse note
#[test]
fn test_case_12_sparse_note() {
    let day = "20261029";
    let (_root, paths, context) = setup_env(day);

    let seg_key = "100000_60";
    let seg_dir = context.journal.join("chronicle").join(day).join(seg_key);
    fs::create_dir_all(&seg_dir).unwrap();
    fs::write(
        seg_dir.join("main_screen.jsonl"),
        tmux_screen_jsonl(seg_key, 1),
    )
    .unwrap();

    let request = json!({
        "name": "participation",
        "day": day,
        "facet": "work",
        "span": [seg_key],
        "activity": {
            "id": "act-sparse-note",
            "destination_id": "act-sparse-note",
            "segments": [seg_key],
            "activity_type": "coding"
        }
    });

    let prep = prepare(
        request.as_object().unwrap().clone(),
        &paths,
        &context,
        PrepareMode::Preview,
    )
    .unwrap();

    let transcript = prep.config["transcript"].as_str().unwrap();
    assert!(transcript.starts_with(SPARSE_INPUT_NOTE));
    assert!(prep.participation_floor.is_some());
}

// Case 13: Event on an unchanged fit
#[test]
fn test_case_13_event_on_unchanged_fit() {
    let day = "20261029";
    let (_root, paths, context) = setup_env(day);

    let seg_key = "100000_60";
    let seg_dir = context.journal.join("chronicle").join(day).join(seg_key);
    fs::create_dir_all(&seg_dir).unwrap();
    fs::write(
        seg_dir.join("main_screen.jsonl"),
        tmux_screen_jsonl(seg_key, 1),
    )
    .unwrap();

    let request = json!({
        "name": "participation",
        "day": day,
        "facet": "work",
        "span": [seg_key],
        "activity": {
            "id": "act-unchanged-fit",
            "destination_id": "act-unchanged-fit",
            "segments": [seg_key],
            "activity_type": "coding"
        }
    });

    let prep = prepare(
        request.as_object().unwrap().clone(),
        &paths,
        &context,
        PrepareMode::Preview,
    )
    .unwrap();

    let event = prep.participation_floor.as_ref().expect("event present");
    assert_eq!(event["speech_lines_omitted"], 0);
    assert_eq!(event["sense_omitted"], 0);
    assert_eq!(event["entries_omitted"], 0);
    assert_eq!(event["frames_omitted"], 0);
    assert_eq!(event["lines_omitted"], 0);
    assert!(event["disclosure"].is_null());

    let transcript = prep.config["transcript"].as_str().unwrap();
    assert!(!transcript.contains("left out"));
}
