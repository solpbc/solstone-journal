// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use solstone_core_top::{
    BrainHealthState, FrameSample, PlainTopStyle, TopMalformed, TopMalformedKind, TopRenderOp,
    TopRoute, TopState, TrustedToken, render_frame, render_ops, transform_trusted_render,
};

#[path = "support/mod.rs"]
mod support;

const FIXTURE: &str = include_str!("../../../fixtures/top_reference.json");

const TOKEN_SPELLINGS: [&str; 12] = [
    "<HOME>",
    "<CLEAR>",
    "<BOLD>",
    "<DIM>",
    "<CYAN>",
    "<GREEN>",
    "<MAGENTA>",
    "<RED>",
    "<SELECT>",
    "</SELECT>",
    "<YELLOW>",
    "<NORMAL>",
];

const TOKEN_ANSI: [&str; 12] = [
    "\x1b[H", "\x1b[2J", "\x1b[1m", "\x1b[2m", "\x1b[36m", "\x1b[32m", "\x1b[35m", "\x1b[31m",
    "\x1b[7m", "\x1b[27m", "\x1b[33m", "\x1b[0m",
];

fn reconstruct(ops: &[TopRenderOp]) -> String {
    ops.iter()
        .map(|op| match op {
            TopRenderOp::Style(token) => token.spelling().to_owned(),
            TopRenderOp::Print(text) => text.clone(),
        })
        .collect()
}

fn style_sequence(ops: &[TopRenderOp]) -> Vec<TrustedToken> {
    ops.iter()
        .filter_map(|op| match op {
            TopRenderOp::Style(token) => Some(*token),
            TopRenderOp::Print(_) => None,
        })
        .collect()
}

fn print_text(ops: &[TopRenderOp]) -> String {
    ops.iter()
        .filter_map(|op| match op {
            TopRenderOp::Print(text) => Some(text.as_str()),
            TopRenderOp::Style(_) => None,
        })
        .collect()
}

fn assert_no_private_markers(ops: &[TopRenderOp]) {
    for op in ops {
        if let TopRenderOp::Print(text) = op {
            assert!(
                !text.contains(['\u{e000}', '\u{e001}', '\u{e002}', '\u{e003}']),
                "private marker in print {text:?}"
            );
        }
    }
}

fn expected_print_payload(raw: &str) -> String {
    raw.replace('\u{1b}', "\\x1b")
}

fn observe_and_service_state(payload: &str) -> TopState {
    let mut state = support::state_for_render_case("empty");
    state
        .observe_status
        .insert("mode".into(), serde_json::json!("idle"));
    state
        .observe_status
        .insert("stream".into(), serde_json::json!(payload));
    state.services.push(serde_json::json!({
        "name": payload, "pid": 1, "ref": "service", "uptime_seconds": 0
    }));
    state
        .service_status
        .insert(payload.to_owned(), ("started".into(), 100.0));
    state
}

#[test]
fn render_ops_reconstructs_the_approved_fixture_for_every_retained_case() {
    let fixture: serde_json::Value = serde_json::from_str(FIXTURE).unwrap();
    let cases = fixture["renders"].as_array().expect("renders array");
    assert_eq!(cases.len(), 10, "retained render case count");
    for case in cases {
        let name = case["name"].as_str().unwrap();
        let width = case["width"].as_u64().unwrap() as usize;
        let sample = FrameSample {
            wall_seconds: 100.0,
            monotonic_seconds: 100.0,
        };
        let state = support::state_for_render_case(name);
        let ops = render_ops(&state, sample, width);
        let expected = transform_trusted_render(case["render"].as_str().unwrap(), width);
        assert_eq!(reconstruct(&ops), expected, "{name}");
        assert_eq!(
            reconstruct(&ops),
            render_frame(&state, sample, width, &PlainTopStyle),
            "{name}"
        );
        assert!(ops.len() <= solstone_core_top::MAX_FRAME_OPS, "{name}");
    }
}

fn populate_shared_dynamic_families(
    state: &mut TopState,
    payload: &str,
    service_name: &str,
    crashed_name: &str,
) {
    state.services.push(serde_json::json!({
        "name": service_name, "pid": 1, "ref": "service", "uptime_seconds": 0
    }));
    state
        .service_status
        .insert(service_name.into(), ("started".into(), 100.0));
    state.last_log_lines.insert(
        "service".into(),
        serde_json::json!([{"seconds":0}, "stderr", format!("log-{payload}")]),
    );
    state.running_tasks.insert(
        "task".into(),
        serde_json::json!({"name":payload,"pid":2,"ref":"task"}),
    );
    state.last_log_lines.insert(
        "task".into(),
        serde_json::json!([{"seconds":0}, "stdout", format!("task-log-{payload}")]),
    );
    state.finished_tasks.insert(
        "ghost".into(),
        serde_json::json!({"name":payload,"exit_code":0}),
    );
    state
        .command_queues
        .insert(format!("queue-{payload}"), serde_json::json!(1));
    state
        .observe_status
        .insert("mode".into(), serde_json::json!("idle"));
    state.observe_status.insert(
        "stream".into(),
        serde_json::json!(format!("observe-{payload}")),
    );
    state.recent_segments.push(serde_json::json!([
        "20260101",
        format!("recent-{payload}"),
        60
    ]));
    state
        .crashed
        .push(serde_json::json!({"name":crashed_name,"restart_attempts":1}));
}

fn state_with_every_dynamic_family(
    payload: &str,
    service_name: &str,
    crashed_name: &str,
) -> TopState {
    let mut state = support::state_for_render_case("empty");
    populate_shared_dynamic_families(&mut state, payload, service_name, crashed_name);
    state.think_running = true;
    state.think_status = [
        ("mode".into(), serde_json::json!(payload)),
        ("day".into(), serde_json::json!(payload)),
        ("segment".into(), serde_json::json!(payload)),
        ("agents_total".into(), serde_json::json!(1)),
        ("agents_completed".into(), serde_json::json!(0)),
        (
            "current_agents".into(),
            serde_json::json!([format!("think-{payload}")]),
        ),
    ]
    .into();
    state.brain_health_state = BrainHealthState::Available {
        observed_at_monotonic: 100.0,
    };
    state.brain_health = Some(serde_json::json!({"lines":[format!("brain-{payload}")]}));
    state
}

fn state_with_every_dynamic_family_failed(
    payload: &str,
    service_name: &str,
    crashed_name: &str,
) -> TopState {
    let mut state = support::state_for_render_case("empty");
    populate_shared_dynamic_families(&mut state, payload, service_name, crashed_name);
    state.think_running = false;
    state.think_last_completed = [
        ("success".into(), serde_json::json!(0)),
        ("failed".into(), serde_json::json!(1)),
        ("duration_ms".into(), serde_json::json!(1000)),
        (
            "failed_names".into(),
            serde_json::json!([format!("failed-{payload}")]),
        ),
    ]
    .into();
    state.brain_health_state = BrainHealthState::Unavailable {
        message: format!("brain-{payload}"),
        observed_at_monotonic: 100.0,
    };
    state
}

fn family_print_markers(payload: &str, running: bool) -> Vec<String> {
    let escaped = expected_print_payload(payload);
    let mut markers = vec![
        format!("log-{escaped}"),
        format!("task-log-{escaped}"),
        format!("queue-{escaped}"),
        format!("observe-{escaped}"),
        format!("recent-{escaped}"),
        format!("brain-{escaped}"),
    ];
    if running {
        markers.push(format!("think-{escaped}"));
    } else {
        markers.push(format!("failed-{escaped}"));
    }
    markers
}

#[test]
fn every_dynamic_row_family_keeps_hostile_payload_as_print() {
    let hostile = "z\u{e000}\u{e001}\u{e002}\u{e003}\u{1b}\u{202e}\x07";
    let wide = format!("{}{hostile}", "界".repeat(32));
    let sample = FrameSample {
        wall_seconds: 100.0,
        monotonic_seconds: 100.0,
    };
    for running in [true, false] {
        let label = if running { "running" } else { "failed" };
        let build = if running {
            state_with_every_dynamic_family
        } else {
            state_with_every_dynamic_family_failed
        };
        let ops = render_ops(&build(hostile, &wide, &wide), sample, 512);
        let control = render_ops(&build("payload", "payload", "payload"), sample, 512);
        assert_no_private_markers(&ops);
        assert_eq!(style_sequence(&ops), style_sequence(&control), "{label}");
        let text = print_text(&ops);
        for escaped in [
            "\\u{e000}",
            "\\u{e001}",
            "\\u{e002}",
            "\\u{e003}",
            "\\x1b",
            "\\u{202e}",
            "\\u{7}",
        ] {
            assert!(
                text.contains(escaped),
                "{label} missing {escaped}: {text:?}"
            );
        }
        let mut prefixes = vec![
            "log-z",
            "task-log-z",
            "queue-z",
            "observe-z",
            "recent-z",
            "brain-z",
        ];
        prefixes.push(if running { "think-z" } else { "failed-z" });
        for prefix in prefixes {
            assert!(text.contains(prefix), "{label} missing {prefix}: {text:?}");
        }
        assert!(
            text.contains("界"),
            "{label} missing large unicode: {text:?}"
        );
    }
}

#[test]
fn every_dynamic_row_family_keeps_token_spellings_and_ansi_as_print() {
    let sample = FrameSample {
        wall_seconds: 100.0,
        monotonic_seconds: 100.0,
    };
    for running in [true, false] {
        let label = if running { "running" } else { "failed" };
        let build = if running {
            state_with_every_dynamic_family
        } else {
            state_with_every_dynamic_family_failed
        };
        let control = render_ops(&build("payload", "payload", "payload"), sample, 512);
        let control_styles = style_sequence(&control);
        for token in TOKEN_SPELLINGS
            .iter()
            .copied()
            .chain(TOKEN_ANSI.iter().copied())
        {
            let payload = format!("x{token}y");
            let ops = render_ops(&build(&payload, &payload, &payload), sample, 512);
            assert_no_private_markers(&ops);
            assert_eq!(style_sequence(&ops), control_styles, "{label} {token:?}");
            let text = print_text(&ops);
            for marker in family_print_markers(&payload, running) {
                assert!(
                    text.contains(&marker),
                    "{label} {token:?} missing {marker}: {text:?}"
                );
            }
        }
    }
}

#[test]
fn malformed_event_diagnostic_stays_inert_print() {
    // TopMalformed Display formats only TopRoute (a fixed enum) and
    // TopMalformedKind (&'static str keys). It never carries request-derived
    // text, so token-spelling injection does not apply to this family.
    let sample = FrameSample {
        wall_seconds: 100.0,
        monotonic_seconds: 100.0,
    };
    let control = TopState {
        malformed_events: 1,
        ..TopState::default()
    };
    let mut state = control.clone();
    state.last_malformed = Some(TopMalformed {
        route: TopRoute::ThinkCompleted,
        kind: TopMalformedKind::WrongType("failed_names[]"),
    });
    let ops = render_ops(&state, sample, 512);
    let control_ops = render_ops(&control, sample, 512);
    assert_no_private_markers(&ops);
    assert_eq!(style_sequence(&ops), style_sequence(&control_ops));
    let text = print_text(&ops);
    assert!(text.contains("malformed events: 1"), "{text:?}");
    assert!(
        text.contains("think/completed: WrongType(\"failed_names[]\")"),
        "{text:?}"
    );
}

#[test]
fn bel_control_in_payload_stays_print() {
    let sample = FrameSample {
        wall_seconds: 100.0,
        monotonic_seconds: 100.0,
    };
    let control = render_ops(&observe_and_service_state("payload"), sample, 512);
    let ops = render_ops(&observe_and_service_state("x\x07y"), sample, 512);
    assert_eq!(style_sequence(&ops), style_sequence(&control));
    let text = print_text(&ops);
    assert!(text.contains("\\u{7}"), "{text:?}");
    assert!(!text.contains('\x07'), "{text:?}");
}
