// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use axum::{
    body::Body,
    http::{Response, header},
};

// solstone/convey/static/shell.html is this crate's only out-of-crate compile input.
const SHELL: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../solstone/convey/static/shell.html"
));
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

    use super::{REMOVALS_JS, WORKSPACE_SUFFIX};
    use solstone_core_retention_client as retention;

    fn removal_copy() -> Vec<(&'static str, &'static str)> {
        let source = std::str::from_utf8(REMOVALS_JS).expect("removal card source is UTF-8");
        let (_, copy) = source
            .split_once("const COPY = Object.freeze({\n")
            .expect("copy table starts");
        let (copy, _) = copy
            .split_once("  });\n\n  // PENDING")
            .expect("copy table ends before pending states");
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

    #[test]
    fn embedded_assets_match_python_reference_sources() {
        assert_eq!(
            include_bytes!("../assets/workspace.html").strip_suffix(WORKSPACE_SUFFIX),
            Some(
                &include_bytes!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/../../../solstone/apps/home/workspace.html"
                ))[..]
            ),
            "the native workspace divergence is exactly the removal card script line"
        );
        assert_eq!(
            include_bytes!("../assets/home.js"),
            include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../../solstone/apps/home/static/home.js"
            )),
        );
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
            "card.total",
            "row.identity",
            "row.origin_policy",
            "row.origin_offload",
            "row.what",
            "row.kept",
            "row.delete",
            "row.keep",
            "confirm.heading",
            "confirm.body_one",
            "confirm.body_many",
            "confirm.go",
            "confirm.cancel",
            "done.deleted",
            "done.partial",
            "done.halted",
            "done.refused_none",
            "done.refused_item",
            "done.unknown",
            "done.kept_policy",
            "done.kept_offload",
            "failed.badge",
            "failed.body",
        ]
        .into_iter()
        .collect::<BTreeSet<_>>();
        assert_eq!(
            keys, expected,
            "the card has one complete authored copy table"
        );
        assert_eq!(copy.len(), 27);
        for (_, value) in copy {
            assert_eq!(value, value.to_lowercase(), "authored copy is lowercase");
            assert!(!value.contains('\u{2014}'), "authored copy has no em dash");
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
        let selection_cap = format!("const MAX_SELECTION = {};", retention::MAX_REMOVE_MARK_IDS);
        assert!(source.contains(&selection_cap));
        assert!(source.contains("if (stream === '_default') return null;"));
        assert!(!source.contains("function formatDay"));

        let (_, failed) = source
            .split_once("function failedRow(row)")
            .expect("failed renderer");
        let (failed, _) = failed
            .split_once("function markedRows()")
            .expect("failed renderer end");
        assert!(failed.contains("identity(row)"));
        assert!(failed.contains("copy(\"failed.badge\")"));
        assert!(failed.contains("copy(\"failed.body\""));
        assert!(!failed.contains("row.what") && !failed.contains("row.origin_"));

        let (_, identity) = source
            .split_once("function identity(row)")
            .expect("identity renderer");
        let (identity, _) = identity
            .split_once("function markedRow(row)")
            .expect("identity renderer end");
        assert!(identity.contains("if (stream === null)"));
        assert!(identity.contains("data-removal-identity>' + escapeHtml(row.day) + '</p>"));
        assert!(identity.contains("copy(\"row.identity\", { date: row.day, stream: stream })"));

        let (_, confirmation) = source
            .split_once("function confirmationHtml()")
            .expect("confirmation renderer");
        let (confirmation, _) = confirmation
            .split_once("function render()")
            .expect("confirmation end");
        assert!(confirmation.contains("selected.length === 1"));
        assert!(confirmation.contains("copyWithoutDefaultStream : copy)(\"confirm.body_one\""));
        assert!(confirmation.contains("copyWithoutDefaultStream"));
        assert!(source.contains("COPY[key].replace(' · {stream}', '')"));
        assert!(confirmation.contains("copy(\"confirm.body_many\""));

        let (_, outcomes) = source
            .split_once("function showOutcome")
            .expect("outcome renderer");
        let (outcomes, _) = outcomes
            .split_once("function selectedRows()")
            .expect("outcome end");
        assert!(outcomes.contains("case 'approve.partial':"));
        assert!(outcomes.contains("case 'approve.refused_after_start':"));
        assert!(outcomes.contains("copy(\"done.refused_none\")"));
        assert!(outcomes.contains("case 'tool.unavailable':"));
        assert!(outcomes.contains("case 'request.too_large':"));
        assert!(outcomes.contains("case 'approve.policy_keeps':"));
        assert!(outcomes.contains("<ul>' + items + '</ul>"));
    }
}
