// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#[test]
fn ac15_settings_build_and_sources_do_not_spawn_processes() {
    let sources = [
        include_str!("../build.rs"),
        include_str!("lib.rs"),
        include_str!("assets.rs"),
        include_str!("chat.rs"),
        include_str!("convey.rs"),
        include_str!("config.rs"),
        include_str!("facets.rs"),
        include_str!("activities.rs"),
        include_str!("icons.rs"),
        include_str!("keys.rs"),
        include_str!("logs.rs"),
        include_str!("observe.rs"),
        include_str!("processing.rs"),
        include_str!("request_body.rs"),
        include_str!("storage.rs"),
        include_str!("sol_voice.rs"),
        include_str!("state.rs"),
        include_str!("sync.rs"),
        include_str!("transcribe.rs"),
        include_str!("vision.rs"),
        include_str!("mutations.rs"),
    ];
    for source in sources {
        assert!(!source.contains("Command::new"));
        assert!(!source.contains(".spawn("));
    }
}
