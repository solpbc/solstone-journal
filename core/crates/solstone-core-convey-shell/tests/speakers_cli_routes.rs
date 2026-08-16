// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Authority coverage for the native speakers CLI route set.

use std::collections::BTreeSet;

const SPEAKERS_CLI_OPERATIONS: [&str; 21] = [
    "speakers.attribute-segment",
    "speakers.backfill",
    "speakers.backfill-last-seen",
    "speakers.bootstrap",
    "speakers.confirm-owner",
    "speakers.day-segments",
    "speakers.dismissals",
    "speakers.identify",
    "speakers.identify-operation",
    "speakers.identify-operations",
    "speakers.keep-separate-list",
    "speakers.link-import",
    "speakers.merge-names",
    "speakers.reject-owner",
    "speakers.resolve-names",
    "speakers.seed-from-imports",
    "speakers.sentences",
    "speakers.status",
    "speakers.suggest",
    "speakers.tag-owner",
    "speakers.wipe",
];

fn quoted(entry: &str, key: &str) -> Option<String> {
    entry.lines().map(str::trim).find_map(|line| {
        line.strip_prefix(key)?
            .trim_start()
            .strip_prefix('=')?
            .trim()
            .strip_prefix('"')?
            .strip_suffix('"')
            .map(str::to_owned)
    })
}

fn authority_routes(source: &str) -> Vec<(String, String)> {
    source
        .split("[[entries]]")
        .skip(1)
        .filter_map(|entry| {
            let id = quoted(entry, "operation_id")?;
            if !SPEAKERS_CLI_OPERATIONS.contains(&id.as_str()) {
                return None;
            }
            Some((quoted(entry, "route")?, quoted(entry, "method")?))
        })
        .collect()
}

fn registered_routes(source: &str) -> BTreeSet<(String, String)> {
    let mut routes = BTreeSet::new();
    let mut remainder = source;
    while let Some(start) = remainder.find(".route(") {
        let call = &remainder[start..];
        let Some(path_start) = call.find('"') else {
            break;
        };
        let after_path = &call[path_start + 1..];
        let Some(path_end) = after_path.find('"') else {
            break;
        };
        let path = &after_path[..path_end];
        let end = call.find("\n        .route").unwrap_or(call.len());
        let registration = &call[..end];
        if registration.contains("get(speakers_cli_") {
            routes.insert((path.to_owned(), "GET".to_owned()));
        }
        if registration.contains("post(speakers_cli_") {
            routes.insert((path.to_owned(), "POST".to_owned()));
        }
        remainder = &call[end..];
    }
    routes
}

#[test]
fn router_covers_every_speakers_cli_operation() {
    let authority = std::fs::read_to_string(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../../solstone/apps/speakers/native/authority.toml"),
    )
    .expect("speakers authority.toml is readable");
    let authority = authority.as_str();
    let inventory = authority_routes(authority);
    assert_eq!(
        inventory.len(),
        21,
        "speakers CLI authority inventory changed; update this explicit scope review"
    );
    let expected = inventory.into_iter().collect::<BTreeSet<_>>();
    let registered = registered_routes(include_str!("../src/lib.rs"));
    let missing = expected
        .difference(&registered)
        .cloned()
        .collect::<Vec<_>>();
    assert!(
        missing.is_empty(),
        "router is missing speakers CLI route-method pairs: {missing:?}"
    );
}

#[test]
fn authority_diff_reports_an_unregistered_bogus_entry() {
    let bogus = r#"
[[entries]]
operation_id = "speakers.status"
entry_type = "http"
method = "GET"
route = "/app/speakers/api/not-registered"
"#;
    let expected = authority_routes(bogus).into_iter().collect::<BTreeSet<_>>();
    let registered = registered_routes(include_str!("../src/lib.rs"));
    let missing = expected
        .difference(&registered)
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(
        missing,
        vec![(
            "/app/speakers/api/not-registered".to_owned(),
            "GET".to_owned()
        )]
    );
}
