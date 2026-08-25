// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use axum::{
    body::Body,
    http::{Response, header},
};

// Shared Convey shell lives in solstone-core-convey-shell/assets/static/shell.html.
const SHELL: &[u8] = include_bytes!("../../solstone-core-convey-shell/assets/static/shell.html");
const WORKSPACE: &[u8] = include_bytes!("../assets/workspace.html");
const HOME_JS: &[u8] = include_bytes!("../assets/home.js");
const REMOVALS_JS: &[u8] = include_bytes!("../assets/removals.js");
#[cfg(test)]
const WORKSPACE_SUFFIX: &[u8] = b"<script src=\"/app/home/static/removals.js\"></script>\n";

pub async fn shell() -> Response<Body> {
    asset(SHELL, "text/html; charset=utf-8")
}

pub async fn workspace() -> Response<Body> {
    asset(WORKSPACE, "text/html; charset=utf-8")
}

pub async fn home_js() -> Response<Body> {
    asset(HOME_JS, "text/javascript; charset=utf-8")
}

pub async fn removals_js() -> Response<Body> {
    asset(REMOVALS_JS, "text/javascript; charset=utf-8")
}

fn asset(bytes: &'static [u8], content_type: &'static str) -> Response<Body> {
    Response::builder()
        .header(header::CONTENT_TYPE, content_type)
        .body(Body::from(bytes))
        .expect("embedded home asset response")
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::{HOME_JS, REMOVALS_JS, WORKSPACE_SUFFIX};

    fn removal_copy() -> Vec<(&'static str, &'static str)> {
        let source = std::str::from_utf8(REMOVALS_JS).expect("removal card source is UTF-8");
        let (_, copy) = source
            .split_once("const COPY = Object.freeze({\n")
            .expect("copy table starts");
        let (copy, _) = copy
            .split_once("  });\n\n  const LIST_URL")
            .expect("copy table ends before card routes");
        copy.lines()
            .map(str::trim)
            .map(|line| {
                let line = line.strip_suffix(',').unwrap_or(line);
                let (key, value) = line.split_once("\": \"").expect("copy table entry");
                (
                    key.strip_prefix('"').expect("quoted copy key"),
                    value.strip_suffix('"').expect("quoted copy value"),
                )
            })
            .collect()
    }

    /// The retired Python workspace differed from this crate's copy by exactly
    /// one line -- the removal card's script tag -- and the parity test that
    /// encoded that divergence read the Python file to prove it. With the source
    /// gone the comparison is unavailable, but the half that mattered is not:
    /// the shipped asset must still END with the removal card's script tag, or
    /// the card silently stops loading and `cargo build` stays green.
    #[test]
    fn embedded_workspace_still_ends_with_the_removal_card_script() {
        assert!(
            include_bytes!("../assets/workspace.html").ends_with(WORKSPACE_SUFFIX),
            "the shipped home workspace must end with the removal card script line"
        );
    }

    #[test]
    fn reading_briefing_entries_do_not_construct_search_anchors() {
        let source = std::str::from_utf8(HOME_JS).expect("home source is UTF-8");
        assert!(!source.contains("/app/search"));
        assert!(!source.contains("pulse-reading-link"));
        assert!(!source.contains("document.createElement('a')"));
    }

    #[test]
    fn removal_card_copy_and_rendering_constraints_are_explicit() {
        let source = std::str::from_utf8(REMOVALS_JS).expect("removal card source is UTF-8");
        let copy = removal_copy();
        let keys = copy.iter().map(|(key, _)| *key).collect::<BTreeSet<_>>();
        let expected = [
            "card.heading",
            "card.subhead",
            "card.empty",
            "card.unavailable",
            "card.total_one",
            "card.total_many",
            "row.identity",
            "row.origin_policy_one",
            "row.origin_policy_many",
            "row.origin_offload_one",
            "row.origin_offload_many",
            "row.what_one",
            "row.what_many",
            "row.kept_one",
            "row.kept_many",
            "row.delete",
            "row.keep",
            "bulk.select_all",
            "bulk.clear",
            "bulk.selected_one",
            "bulk.selected_many",
            "bulk.delete",
            "bulk.keep",
            "confirm.heading_one",
            "confirm.heading_many",
            "confirm.body_policy_one",
            "confirm.body_policy_many",
            "confirm.body_offload_one",
            "confirm.body_offload_many",
            "confirm.body_policy_selected",
            "confirm.body_offload_selected",
            "confirm.go_one",
            "confirm.go_many",
            "confirm.cancel",
            "confirm.recover.heading",
            "confirm.recover.body",
            "confirm.recover.go",
            "done.clause_deleted_one",
            "done.clause_deleted_many",
            "done.clause_not_removed_one",
            "done.clause_not_removed_many",
            "done.clause_halted",
            "done.refused_none_one",
            "done.refused_none_many",
            "done.refused_item",
            "done.refused_item_unnamed",
            "done.unknown",
            "done.kept_policy",
            "done.kept_offload",
            "done.too_many",
            "done.declined_failed",
            "done.declined_unknown",
            "done.recovered",
            "done.recovered_none",
            "done.recovered_leftover",
            "done.recover_failed",
            "done.recover_unknown",
            "failed.badge",
            "failed.body",
            "failed.finish",
        ]
        .into_iter()
        .collect::<BTreeSet<_>>();
        assert_eq!(
            keys, expected,
            "the card has one complete authored copy table"
        );
        assert_eq!(copy.len(), 60);
        for (_, value) in &copy {
            assert_eq!(*value, value.to_lowercase(), "authored copy is lowercase");
            assert!(!value.contains('\u{2014}'), "authored copy has no em dash");
        }
        let failed_body = copy
            .iter()
            .find(|(key, _)| *key == "failed.body")
            .expect("failed.body")
            .1;
        let recover_body = copy
            .iter()
            .find(|(key, _)| *key == "confirm.recover.body")
            .expect("confirm.recover.body")
            .1;
        for value in [failed_body, recover_body] {
            assert!(
                value.contains("it does not put anything back"),
                "denial phrase missing: {value}"
            );
            assert!(!value.contains("undo"), "denial key contains undo: {value}");
            assert!(
                !value.contains("restore"),
                "denial key contains restore: {value}"
            );
        }
        for forbidden in [
            "row.why",
            "card.unavailable_reason",
            "failed.why",
            "done.refused_reason",
        ] {
            assert!(
                !source.contains(forbidden),
                "forbidden owner field appears in the card: {forbidden}"
            );
        }
        assert!(
            !source.contains("/app/home/api/pulse") && !source.contains("/app/home/api/briefing"),
            "the removal card is independent of pulse and briefing data"
        );
        assert!(source.contains("const MAX_SELECTED_MARKS = 32;"));
        assert!(source.contains("if (stream === '_default') return null;"));
        assert!(!source.contains("function formatDay"));

        let (_, marked) = source
            .split_once("function markedRow(row)")
            .expect("marked renderer");
        let (marked, _) = marked
            .split_once("function failedRow(row)")
            .expect("marked renderer end");
        assert!(marked.contains("data-removal-select"));

        let (_, failed) = source
            .split_once("function failedRow(row)")
            .expect("failed renderer");
        let (failed, _) = failed
            .split_once("function markedRows()")
            .expect("failed renderer end");
        assert!(failed.contains("identity(identityText(row))"));
        assert!(failed.contains("copy(\"failed.badge\")"));
        assert!(failed.contains("copy(\"failed.body\""));
        assert!(!failed.contains("row.what") && !failed.contains("row.origin_"));
        assert!(!failed.contains("data-removal-select"));
        assert!(!failed.contains("data-removal-action"));

        let (_, outcomes) = source
            .split_once("function showOutcome")
            .expect("outcome renderer");
        let (outcomes, _) = outcomes
            .split_once("function showRecoverOutcome")
            .expect("outcome end");
        assert!(outcomes.contains("case 'approve.partial':"));
        assert!(outcomes.contains("case 'approve.refused_after_start':"));
        assert!(outcomes.contains("approveOutcome(response, rows, items)"));
        assert!(outcomes.contains("done.refused_none"));
        assert!(outcomes.contains("case 'declined.unknown':"));
        assert!(outcomes.contains("case 'tool.unavailable':"));
        assert!(outcomes.contains("case 'request.too_large':"));
        assert!(outcomes.contains("case 'approve.policy_keeps':"));
        assert!(outcomes.contains("refusalList(items)"));
        assert!(source.contains("refusal.item_unnamed"));
        assert!(source.contains("confirmation = { kind: 'delete', rows: rows };"));
        assert!(source.contains("confirmation = { kind: 'recover' };"));

        let (_, recover_map) = source
            .split_once("function showRecoverOutcome")
            .expect("recover renderer");
        let (recover_map, _) = recover_map
            .split_once("async function refresh()")
            .expect("recover renderer end");
        assert!(recover_map.contains("copy(\"done.recover_unknown\")"));
        assert!(recover_map.contains("copy(\"done.recover_failed\")"));
        assert!(recover_map.contains(
            "copy(finished > 0 ? \"done.recovered_leftover\" : \"done.recover_failed\")"
        ));
        assert!(recover_map.contains("copy(\"done.recovered\")"));
        assert!(recover_map.contains("copy(\"done.recovered_none\")"));
        assert!(recover_map.contains("recover.failed"));
    }
}
