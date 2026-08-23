// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use solstone_core_top::{
    AnsiTopStyle, FrameSample, PlainTopStyle, TopRenderOp, frame_ops, render_frame,
    transform_trusted_render,
};

#[path = "support/mod.rs"]
mod support;

const FIXTURE: &str = include_str!("../../../fixtures/top_reference.json");

fn reconstruct(ops: &[TopRenderOp]) -> String {
    ops.iter()
        .map(|op| match op {
            TopRenderOp::Style(token) => token.spelling().to_owned(),
            TopRenderOp::Print(text) => text.clone(),
        })
        .collect()
}

#[test]
fn frame_ops_match_plain_and_ansi_for_retained_render_cases() {
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
        let ansi_ops = frame_ops(&render_frame(&state, sample, width, &AnsiTopStyle));
        let plain_ops = frame_ops(&render_frame(&state, sample, width, &PlainTopStyle));
        assert_eq!(ansi_ops, plain_ops, "{name}");
        let expected = transform_trusted_render(case["render"].as_str().unwrap(), width);
        assert_eq!(reconstruct(&plain_ops), expected, "{name}");
    }
}
