// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use sha2::{Digest, Sha256};
use solstone_core_top::{
    AnsiTopStyle, BrainHealthState, FrameSample, PlainTopStyle, render_frame,
    transform_trusted_render,
};

mod support;

const FIXTURE: &str = include_str!("../../../fixtures/top_reference.json");

#[test]
fn all_retained_render_sources_transform_to_approved_digests() {
    let fixture: serde_json::Value = serde_json::from_str(FIXTURE).unwrap();
    let expected = [
        (
            "empty",
            "e9165028656015630b313878995ebd1b9e27a8819238e06cfd272ec8d3bc2970",
        ),
        (
            "one",
            "39cfb6308a1ab4954a398279c945207c54f6d029ba4c122c2587b0f0381daa53",
        ),
        (
            "full",
            "f78dbb5fb690b0678dbc9673e5634aa00ebc86069cc52e3c3a0b53c9a1645fbd",
        ),
        (
            "wide",
            "5a3a92938eb680c9765d6149c9eff393283e5c264749d2a66b7cfb1b9abeaadc",
        ),
        (
            "think-failed",
            "8493a9f454681050557f278c01faf35cffd9951ce2ece44a6d5ca439de42bf87",
        ),
        (
            "brain-supplied",
            "f92b0db46ca66c903c19269f0654ce12090056f05d6e8ba77b4e5e870ec1f53a",
        ),
        (
            "observe-idle",
            "1f274d78da18dc25dce5cb805325ce0711f4dc5452d23ded99dcc17d684622e2",
        ),
        (
            "observe-tmux-yellow",
            "f9350f4b41cea62cb47a594b40e42adcf8287ca0645742d2efdf2b9f03d1bd6a",
        ),
        (
            "observe-tmux-yellow-upper",
            "f9350f4b41cea62cb47a594b40e42adcf8287ca0645742d2efdf2b9f03d1bd6a",
        ),
        (
            "last-selected",
            "4bbeeac020a3d722fef905d09117c632a9d4d3bd5a78b143c19f546c0c72427a",
        ),
    ];
    for case in fixture["renders"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let width = case["width"].as_u64().unwrap() as usize;
        let expected_render = transform_trusted_render(case["render"].as_str().unwrap(), width);
        let production_render = render_frame(
            &support::state_for_render_case(name),
            FrameSample {
                wall_seconds: 100.0,
                monotonic_seconds: 100.0,
            },
            width,
            &PlainTopStyle,
        );
        assert_eq!(production_render, expected_render, "{name}");
        let actual = format!("{:x}", Sha256::digest(expected_render.as_bytes()));
        assert_eq!(
            Some(actual.as_str()),
            expected
                .iter()
                .find(|(candidate, _)| *candidate == name)
                .map(|(_, digest)| *digest),
            "{name}"
        );
    }
}

#[test]
fn payload_token_spellings_are_not_trusted_framing() {
    let rendered = transform_trusted_render("<BOLD>ab\u{e000}<RED>\u{e001}cd", 9);
    assert_eq!(rendered, "<BOLD>ab<RED>cd");
    assert_eq!(
        transform_trusted_render("<BOLD>ab<RED>cd", 3),
        "<BOLD>ab<RED>c<NORMAL>"
    );
}

#[test]
fn untrusted_state_payloads_remain_inert_tokens() {
    let mut state = support::state_for_render_case("empty");
    state
        .command_queues
        .insert("<RED>abcdef".into(), serde_json::json!(1));
    state.think_running = true;
    state.think_status = [
        ("mode".into(), serde_json::json!("<RED>")),
        ("day".into(), serde_json::json!("<GREEN>")),
        ("segment".into(), serde_json::json!("<YELLOW>")),
        ("agents_total".into(), serde_json::json!(1)),
        ("agents_completed".into(), serde_json::json!(0)),
    ]
    .into();
    let rendered = render_frame(
        &state,
        FrameSample {
            wall_seconds: 100.0,
            monotonic_seconds: 100.0,
        },
        40,
        &PlainTopStyle,
    );
    assert!(rendered.contains("<RED>abcdef ×1"));
    assert!(rendered.contains("[<RED>] <GREEN>/<YELLOW>"));
    assert_eq!(
        transform_trusted_render("<BOLD>\u{e000}<RED>\u{e001}abc", 6),
        "<BOLD><RED>a<NORMAL>"
    );
}

const TOKENS: [&str; 12] = [
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

fn without_owned_ansi(mut rendered: String) -> String {
    for sequence in [
        "\u{1b}[H",
        "\u{1b}[2J",
        "\u{1b}[1m",
        "\u{1b}[2m",
        "\u{1b}[31m",
        "\u{1b}[32m",
        "\u{1b}[33m",
        "\u{1b}[35m",
        "\u{1b}[36m",
        "\u{1b}[7m",
        "\u{1b}[27m",
        "\u{1b}[0m",
    ] {
        rendered = rendered.replace(sequence, "");
    }
    assert!(!rendered.contains('\u{1b}'), "untrusted ESC reached output");
    rendered
}

fn assert_no_private_markers(rendered: &str) {
    assert!(!rendered.contains(['\u{e000}', '\u{e001}', '\u{e002}', '\u{e003}']));
}

#[test]
fn observe_and_service_payload_tokens_are_literal_and_service_identity_is_raw() {
    for token in TOKENS {
        let payload = format!("x{token}y");
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
            .insert(payload.clone(), ("started".into(), 100.0));
        let rendered = render_frame(
            &state,
            FrameSample {
                wall_seconds: 100.0,
                monotonic_seconds: 100.0,
            },
            512,
            &AnsiTopStyle,
        );
        assert_no_private_markers(&rendered);
        let text = without_owned_ansi(rendered);
        assert!(text.contains(&format!("✓ {payload}")), "{token}: {text:?}");
        assert!(
            text.matches(&payload).count() >= 2,
            "service and Observe should both retain {token}: {text:?}"
        );
    }
}

#[test]
fn every_dynamic_row_family_escapes_private_markers_and_controls() {
    let hostile = "z\u{e000}\u{e001}\u{e002}\u{e003}\u{1b}";
    let mut state = support::state_for_render_case("empty");
    state.services.push(serde_json::json!({
        "name": hostile, "pid": 1, "ref": "service", "uptime_seconds": 0
    }));
    state
        .service_status
        .insert(hostile.into(), ("started".into(), 100.0));
    state.last_log_lines.insert(
        "service".into(),
        serde_json::json!([{"seconds":0}, "stderr", format!("log-{hostile}")]),
    );
    state.running_tasks.insert(
        "task".into(),
        serde_json::json!({"name":hostile,"pid":2,"ref":"task"}),
    );
    state.last_log_lines.insert(
        "task".into(),
        serde_json::json!([{"seconds":0}, "stdout", format!("task-log-{hostile}")]),
    );
    state.finished_tasks.insert(
        "ghost".into(),
        serde_json::json!({"name":hostile,"exit_code":0}),
    );
    state
        .command_queues
        .insert(format!("queue-{hostile}"), serde_json::json!(1));
    state
        .observe_status
        .insert("mode".into(), serde_json::json!("idle"));
    state.observe_status.insert(
        "stream".into(),
        serde_json::json!(format!("observe-{hostile}")),
    );
    state.think_running = true;
    state.think_status = [
        ("mode".into(), serde_json::json!(hostile)),
        ("day".into(), serde_json::json!(hostile)),
        ("segment".into(), serde_json::json!(hostile)),
        ("agents_total".into(), serde_json::json!(1)),
        ("agents_completed".into(), serde_json::json!(0)),
        ("current_agents".into(), serde_json::json!([hostile])),
    ]
    .into();
    state.brain_health_state = BrainHealthState::Available {
        observed_at_monotonic: 100.0,
    };
    state.brain_health = Some(serde_json::json!({"lines":[format!("brain-{hostile}")]}));
    state
        .crashed
        .push(serde_json::json!({"name":hostile,"restart_attempts":1}));

    let rendered = render_frame(
        &state,
        FrameSample {
            wall_seconds: 100.0,
            monotonic_seconds: 100.0,
        },
        512,
        &AnsiTopStyle,
    );
    assert_no_private_markers(&rendered);
    let text = without_owned_ansi(rendered);
    for escaped in ["\\u{e000}", "\\u{e001}", "\\u{e002}", "\\u{e003}", "\\x1b"] {
        assert!(text.contains(escaped), "missing {escaped}: {text:?}");
    }
    for prefix in ["log-z", "task-log-z", "queue-z", "observe-z", "brain-z"] {
        assert!(text.contains(prefix), "missing {prefix}: {text:?}");
    }
}

#[test]
fn operational_log_clipping_never_splits_tokens_or_sanitizer_atoms() {
    for payload in TOKENS.into_iter().chain(["\u{e000}", "\u{1b}"]) {
        let sanitized = match payload {
            "\u{e000}" => "\\u{e000}",
            "\u{1b}" => "\\x1b",
            value => value,
        };
        for available in 0..=sanitized.chars().count() + 1 {
            let mut state = support::state_for_render_case("empty");
            state.services.push(serde_json::json!({
                "name":"svc", "pid":1, "ref":"service", "uptime_seconds":0
            }));
            state.last_log_lines.insert(
                "service".into(),
                serde_json::json!([{"seconds":0}, "stderr", payload]),
            );
            let rendered = render_frame(
                &state,
                FrameSample::default(),
                63 + available,
                &AnsiTopStyle,
            );
            assert_no_private_markers(&rendered);
            let text = without_owned_ansi(rendered);
            let row = text
                .lines()
                .find(|line| line.contains("svc"))
                .expect("service row");
            assert!(
                row.chars().count() <= 63 + available,
                "{payload:?}/{available}: {row:?}"
            );
            if matches!(payload, "\u{e000}" | "\u{1b}")
                && let Some(start) = row.rfind('\\')
            {
                assert_eq!(
                    &row[start..],
                    sanitized,
                    "atom split at {available}: {row:?}"
                );
            }
        }
    }
}
