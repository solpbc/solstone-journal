// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;

use serde_json::json;
use solstone_core_top::{FrameSample, PlainTopStyle, TopState, render_frame};

fn state_for_render_case(name: &str) -> TopState {
    let mut state = TopState::default();
    match name {
        "empty" => {}
        "one" | "full" | "wide" => {
            state.services.push(json!({
                "name":"supervisor", "pid":101, "ref":"svc-a", "uptime_seconds":3660
            }));
            state
                .service_status
                .insert("supervisor".into(), ("started".into(), 100.0));
            state.last_log_lines.insert(
                "svc-a".into(),
                json!([{"seconds":0}, "stdout", "service α line"]),
            );
            if matches!(name, "full" | "wide") {
                state.services.push(json!({
                    "name":"local-service-name", "pid":102, "ref":"svc-b", "uptime_seconds":86460
                }));
                state
                    .crashed
                    .push(json!({"name":"crash\u{001b}", "restart_attempts":3}));
                state.running_tasks = BTreeMap::from([
                    (
                        "task-a".into(),
                        json!({"ref":"task-a", "name":"backup", "pid":201}),
                    ),
                    (
                        "task-supervised".into(),
                        json!({"ref":"task-supervised", "name":"supervisor-copy", "pid":101}),
                    ),
                ]);
                state.last_log_lines.insert(
                    "task-a".into(),
                    json!([{"seconds":0}, "stderr", "task error zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz"]),
                );
                state.finished_tasks = BTreeMap::from([
                    ("ghost-ok".into(), json!({"name":"done", "exit_code":0})),
                    ("ghost-bad".into(), json!({"name":"bad", "exit_code":4})),
                    (
                        "ghost-unknown".into(),
                        json!({"name":"lost", "exit_code":null}),
                    ),
                ]);
                state.command_queues =
                    BTreeMap::from([("backup".into(), json!(2)), ("health".into(), json!(3))]);
                state.observe_status = BTreeMap::from([
                    ("mode".into(), json!("screencast")),
                    ("stream".into(), json!("display")),
                ]);
                state.displayed_mode = "screencast".into();
                state.last_active_ts = 99.0;
                state.think_running = true;
                state.think_status = BTreeMap::from([("mode".into(), json!("batch"))]);
                state.brain_health = Some(json!({"lines":["Brain Health — OK", "  memory good"]}));
            }
        }
        "think-failed" | "brain-supplied" => {
            state.think_last_completed = BTreeMap::from([
                ("success".into(), json!(2)),
                ("failed".into(), json!(1)),
                ("duration_ms".into(), json!(61234)),
                ("failed_names".into(), json!(["agent-x"])),
            ]);
            if name == "brain-supplied" {
                state.brain_health = Some(json!({"lines":["Brain Health — DEGRADED", "  item"]}));
            }
        }
        "observe-idle" | "observe-tmux-yellow" | "observe-tmux-yellow-upper" => {
            let mode = if name == "observe-idle" {
                "idle"
            } else {
                "tmux"
            };
            state.observe_status = BTreeMap::from([
                ("mode".into(), json!(mode)),
                ("tmux".into(), json!({"captures":2})),
                ("activity".into(), json!({"screen_locked":true})),
            ]);
            state.displayed_mode = mode.into();
            state.last_active_ts = 90.0;
        }
        "last-selected" => {
            state.services = vec![
                json!({"name":"first", "pid":101, "ref":"first", "uptime_seconds":1}),
                json!({"name":"last", "pid":201, "ref":"last", "uptime_seconds":2}),
            ];
            state.selected = 1;
        }
        other => panic!("unrecognized retained render case: {other}"),
    }
    state
}

#[test]
fn retained_render_recipes_use_each_captured_state_shape() {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("../../../fixtures/top_reference.json")).unwrap();
    for case in fixture["renders"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let width = case["width"].as_u64().unwrap() as usize;
        let state = state_for_render_case(name);
        let rendered = render_frame(
            &state,
            FrameSample {
                wall_seconds: 100.0,
                monotonic_seconds: 100.0,
            },
            width,
            &PlainTopStyle,
        );
        assert!(rendered.contains("solstone activity manager"), "{name}");
        assert!(
            rendered
                .lines()
                .filter(|line| !line.is_empty() && line.chars().all(|ch| ch == '─'))
                .all(|line| line.chars().count() == width),
            "{name}"
        );
        match name {
            "empty" => assert!(rendered.contains("(waiting for services)"), "{name}"),
            "one" => {
                assert!(rendered.contains("supervisor"), "{name}");
                assert!(!rendered.contains("(waiting for services)"), "{name}");
            }
            "full" | "wide" => {
                assert!(rendered.contains("local-servi"), "{name}");
                assert!(rendered.contains("backup"), "{name}");
                assert!(rendered.contains("Crashed"), "{name}");
                assert!(rendered.contains("crash"), "{name}");
            }
            "think-failed" => assert!(rendered.contains("agent-x"), "{name}"),
            "brain-supplied" => assert!(rendered.contains("DEGRADED"), "{name}"),
            "observe-idle" => {
                assert!(rendered.contains("screen_locked"), "{name}");
                assert!(rendered.contains("\"idle\""), "{name}");
            }
            "observe-tmux-yellow" | "observe-tmux-yellow-upper" => {
                assert!(rendered.contains("\"tmux\""), "{name}");
                assert!(rendered.contains("captures"), "{name}");
            }
            "last-selected" => {
                assert!(rendered.contains("first"), "{name}");
                assert!(rendered.contains("last"), "{name}");
                assert_eq!(state.selected, 1, "{name}");
            }
            other => panic!("unrecognized retained render case: {other}"),
        }
    }
}
