// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::Path;

fn repository_root() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("repository root")
        .to_path_buf()
}

fn native_workspace() -> String {
    fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/thinking/workspace.html"))
        .expect("native workspace reads")
}

fn native_script() -> String {
    fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/thinking/thinking.js"))
        .expect("native script reads")
}

fn heading_levels(source: &str) -> Vec<u8> {
    source
        .match_indices("<h")
        .filter_map(|(index, _)| source.as_bytes().get(index + 2).copied())
        .filter_map(|level| (b'1'..=b'3').contains(&level).then_some(level - b'0'))
        .collect()
}

fn authored_workspace_copy(source: &str) -> String {
    let css_start = source
        .find("    .thinking-workspace .thinking-runs-header")
        .expect("Runs CSS starts");
    let css_end = source[css_start..]
        .find("    @media (max-width: 760px)")
        .expect("Runs CSS ends")
        + css_start;
    let header_start = source
        .find("    <header class=\"thinking-runs-header\">")
        .expect("Runs markup starts");
    let header_end = source[header_start..]
        .find("    </header>")
        .expect("Runs header ends")
        + header_start
        + "    </header>".len();
    let runs_start = source
        .find("    <section class=\"thinking-runs-panel\" id=\"thinkingRunsPanel\"")
        .expect("Runs panel starts");
    let runs_end = source[runs_start..]
        .find("    </section>")
        .expect("Runs panel ends")
        + runs_start
        + "    </section>".len();
    let identity_start = source
        .find("    <section class=\"thinking-runs-panel\" id=\"thinkingIdentityPanel\"")
        .expect("Identity panel starts");
    let identity_end = source[identity_start..]
        .find("    </section>")
        .expect("Identity panel ends")
        + identity_start
        + "    </section>".len();
    let modal_start = source
        .find("<div class=\"thinking-runs-modal\"")
        .expect("Runs modal starts");
    let modal_end = source[modal_start..]
        .find("</div>\n</div>")
        .expect("Runs modal ends")
        + modal_start
        + "</div>\n</div>".len();
    format!(
        "{}{}{}{}{}",
        &source[css_start..css_end],
        &source[header_start..header_end],
        &source[runs_start..runs_end],
        &source[identity_start..identity_end],
        &source[modal_start..modal_end],
    )
}

fn authored_script_copy(source: &str) -> &str {
    let start = source
        .find("  function encodeThinkingSegment(value)")
        .expect("Runs script starts");
    let end = source[start..]
        .find("  function providerLabel(provider)")
        .expect("Runs script ends")
        + start;
    &source[start..end]
}

fn contains_copy_literal(value: &serde_json::Value, needle: &str) -> bool {
    match value {
        serde_json::Value::String(text) => text == needle,
        serde_json::Value::Array(values) => values
            .iter()
            .any(|value| contains_copy_literal(value, needle)),
        serde_json::Value::Object(values) => values
            .values()
            .any(|value| contains_copy_literal(value, needle)),
        _ => false,
    }
}

#[test]
fn thinking_runs_heading_structure_has_one_h1_and_no_skips() {
    let workspace = native_workspace();
    assert_eq!(workspace.matches("<h1").count(), 1, "one page heading");
    let headings = heading_levels(&workspace);
    assert_eq!(headings.first(), Some(&1), "heading sequence starts at h1");
    for pair in headings.windows(2) {
        assert!(
            pair[1] <= pair[0] + 1,
            "heading levels do not skip: {headings:?}"
        );
    }
}

#[test]
fn thinking_runs_tab_markup_has_roving_tabs_and_labelled_panels() {
    let workspace = native_workspace();
    let script = native_script();
    for needle in [
        "role=\"tablist\"",
        "role=\"tab\"",
        "aria-selected=\"true\"",
        "aria-labelledby=\"thinkingSetupTab\"",
        "aria-labelledby=\"thinkingRunsTab\"",
        "aria-labelledby=\"thinkingIdentityTab\"",
        "id=\"thinkingRunsNoOutput\"",
        "this run doesn't have a saved output.",
    ] {
        assert!(
            workspace.contains(needle),
            "missing workspace contract: {needle}"
        );
    }
    for needle in [
        "function activateThinkingSectionTab(tabId, origin)",
        "tabIndex = selected ? 0 : -1",
        "ArrowLeft",
        "ArrowRight",
        "function bindThinkingSectionTabs()",
    ] {
        assert!(script.contains(needle), "missing tab behavior: {needle}");
    }
}

#[test]
fn thinking_runs_tab_sizing_and_motion_contracts_are_present() {
    let workspace = native_workspace();
    for needle in [
        "min-height: 44px",
        "min-height: 46px",
        "prefers-reduced-motion: reduce",
    ] {
        assert!(
            workspace.contains(needle),
            "missing style contract: {needle}"
        );
    }
}

#[test]
fn thinking_runs_authored_copy_excludes_prohibited_terms() {
    let workspace = native_workspace();
    let script = native_script();
    let scoped_copy = format!(
        "{}{}",
        authored_workspace_copy(&workspace),
        authored_script_copy(&script)
    )
    .to_ascii_lowercase();
    for prohibited in [
        "owner:",
        "agent identity",
        "activity",
        "reprocessing",
        "thinking history",
        "—",
    ] {
        assert!(
            !scoped_copy.contains(prohibited),
            "prohibited Runs/Identity copy: {prohibited}"
        );
    }
}

#[test]
fn thinking_runs_assets_match_python_reference_byte_for_byte() {
    let root = repository_root();
    assert_eq!(
        native_workspace(),
        fs::read_to_string(root.join("solstone/apps/thinking/workspace.html"))
            .expect("Python workspace reads"),
        "Thinking workspace copies stay byte-identical"
    );
    assert_eq!(
        native_script(),
        fs::read_to_string(root.join("solstone/apps/thinking/static/thinking.js"))
            .expect("Python script reads"),
        "Thinking script copies stay byte-identical"
    );
}

#[test]
fn thinking_runs_copy_payload_remains_frozen() {
    let routes = fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/thinking.rs"))
        .expect("Thinking routes read");
    assert!(
        routes.contains("\"copy\":solstone_core_thinking_copy::thinking_copy_payload()"),
        "the state payload continues to source copy only from the frozen copy payload"
    );
    let payload = serde_json::to_value(solstone_core_thinking_copy::thinking_copy_payload())
        .expect("Thinking copy payload serializes");
    for literal in [
        "setup",
        "runs",
        "identity",
        "talent runs",
        "date",
        "previous day",
        "next day",
        "facet",
        "all",
        "loading talent runs…",
        "no talent runs on this day",
        "talent runs will appear here after they finish.",
        "couldn't load talent runs",
        "try again",
        "some run details aren't available right now.",
        "that talent run isn't available.",
        "loading run details…",
        "loading run log…",
        "this run is still in progress.",
        "check back soon.",
        "couldn't load that run",
        "couldn't load that prompt",
        "loading output…",
        "couldn't load that output",
        "this run doesn't have a saved output.",
        "loading identity…",
        "couldn't load identity",
        "identity details aren't available right now.",
        "name",
        "naming status",
        "you",
        "not set",
        "run log",
        "output",
        "prompt",
        "close",
    ] {
        assert!(
            !contains_copy_literal(&payload, literal),
            "Runs/Identity literal entered thinking_copy_payload: {literal}"
        );
    }
}

#[test]
fn thinking_runs_list_source_has_explicit_table_controls_and_mobile_cards() {
    let script = native_script();
    for needle in [
        "cell.scope = 'col'",
        "thinking-runs-run-control",
        "thinking-runs-cards",
        "function renderThinkingRunList(host, runs)",
    ] {
        assert!(
            script.contains(needle),
            "missing run-list source contract: {needle}"
        );
    }
    let row_start = script
        .find("runs.forEach((run) => {")
        .expect("run rows are rendered");
    let row_end = script[row_start..]
        .find("body.appendChild(row);")
        .expect("run row is appended")
        + row_start;
    let row = &script[row_start..row_end];
    assert!(
        row.contains("prompt.appendChild(thinkingRunControl(run));"),
        "every generated run row appends its explicit control"
    );
    assert!(
        !row.contains("row.addEventListener(") && !row.contains("row.onclick"),
        "run rows are not clickable"
    );
}

#[test]
fn thinking_runs_list_styles_do_not_force_horizontal_scrolling() {
    let workspace = native_workspace();
    let table_start = workspace
        .find(".thinking-workspace .thinking-runs-table {")
        .expect("run table style starts");
    let table_end = workspace[table_start..]
        .find("    }")
        .expect("run table style ends")
        + table_start;
    assert!(
        !workspace[table_start..table_end].contains("overflow-x"),
        "run list must not force horizontal scrolling"
    );
    assert!(workspace.contains(".thinking-workspace .thinking-runs-cards {"));
}
