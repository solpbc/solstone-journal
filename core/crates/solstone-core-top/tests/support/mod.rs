// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;

use serde_json::json;
use solstone_core_top::TopState;

pub fn state_for_render_case(name: &str) -> TopState {
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
            state.memory_cache.insert(101, 10 * 1_048_576);
            state.last_log_at.insert("svc-a".into(), 100.0);
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
                state.last_log_at.insert("task-a".into(), 100.0);
                state.task_started_at.insert("task-a".into(), 39.0);
                state.memory_cache.insert(101, 10 * 1_048_576);
                state.memory_cache.insert(201, 6 * 1_048_576);
                state.finished_tasks = BTreeMap::from([
                    ("a-lost".into(), json!({"name":"lost", "exit_code":null})),
                    ("b-bad".into(), json!({"name":"bad", "exit_code":4})),
                    ("c-ok".into(), json!({"name":"done", "exit_code":0})),
                ]);
                state.command_queues =
                    BTreeMap::from([("backup".into(), json!(2)), ("health".into(), json!(3))]);
                state.observe_status = BTreeMap::from([
                    ("mode".into(), json!("screencast")),
                    ("stream".into(), json!("display")),
                    ("screencast".into(), json!({"window_elapsed_seconds":60})),
                    ("audio".into(), json!({"threshold_hits":2,"will_save":true})),
                    ("describe".into(), json!({"running":["x"],"queued":["y"]})),
                    ("transcribe".into(), json!({"queued":["z","w"]})),
                ]);
                state.displayed_mode = "screencast".into();
                state.last_active_ts = 99.0;
                state.observe_last_ts = if name == "wide" { 1.0 } else { 99.0 };
                state.recent_segments =
                    vec![json!(["260810", "003", 60]), json!(["260810", "002", 120])];
                state.think_running = true;
                state.think_status = BTreeMap::from([
                    ("mode".into(), json!("batch")),
                    ("day".into(), json!("260810")),
                    ("segment".into(), json!("003")),
                    ("segments_completed".into(), json!(2)),
                    ("segments_total".into(), json!(4)),
                    ("agents_completed".into(), json!(1)),
                    ("agents_total".into(), json!(3)),
                    ("current_agents".into(), json!(["b", "a"])),
                ]);
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
            state.observe_last_ts = 70.0;
        }
        "last-selected" => {
            state.services = vec![
                json!({"name":"first", "pid":101, "ref":"first", "uptime_seconds":1}),
                json!({"name":"last", "pid":201, "ref":"last", "uptime_seconds":2}),
            ];
            state.selected = 1;
            state.memory_cache = BTreeMap::from([(101, 10 * 1_048_576), (201, 6 * 1_048_576)]);
        }
        other => panic!("unrecognized retained render case: {other}"),
    }
    state
}
