// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use solstone_core_top::{
    FrameSample, PlainTopStyle, TopState, render_frame, transform_trusted_render,
};

#[test]
fn all_rows_are_scalar_bounded_without_atom_splitting() {
    let mut state = TopState::default();
    state
        .services
        .push(serde_json::json!({"name":"\u{1b}<RED>界", "pid":1,"ref":"r","uptime_seconds":0}));
    for width in [0, 1, 2, 4, 40, 120] {
        let rendered = render_frame(&state, FrameSample::default(), width, &PlainTopStyle);
        for row in rendered.lines() {
            let payload = row
                .replace("<HOME>", "")
                .replace("<CLEAR>", "")
                .replace("<BOLD>", "")
                .replace("<DIM>", "")
                .replace("<CYAN>", "")
                .replace("<GREEN>", "")
                .replace("<MAGENTA>", "")
                .replace("<RED>", "")
                .replace("<SELECT>", "")
                .replace("</SELECT>", "")
                .replace("<YELLOW>", "")
                .replace("<NORMAL>", "");
            assert!(payload.chars().count() <= width, "width {width}: {row:?}");
        }
    }
}

#[test]
fn hostile_payload_work_is_prefix_bounded() {
    let render =
        |length| transform_trusted_render(&format!("<BOLD>{}", "\u{1b}".repeat(length)), 40);
    let one = render(1024 * 1024);
    let sixteen = render(16 * 1024 * 1024);
    assert_eq!(one, sixteen);
    assert!(one.ends_with("<NORMAL>"));
}
