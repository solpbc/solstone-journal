// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use sha2::{Digest, Sha256};
use solstone_core_top::{FrameSample, PlainTopStyle, render_frame, transform_trusted_render};

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
