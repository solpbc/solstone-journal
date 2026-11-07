// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The drift test: every static reference in every embedded text file, as it is
//! served, resolves through this build's manifest to a hashed path, or is on the
//! explicit list below with the reason it stays on its stable path.

use std::collections::BTreeSet;

use regex::Regex;

use crate::asset_cache::{
    has_tree_hash_shape, reference_spans, resolve_reference, rewrite_references,
};
use crate::assets::{self, AssetKind, INVENTORY};

/// Paths built at runtime, or named absolutely from inside their own tree (a
/// tree can't name its own hash), with how each one resolves.
const RUNTIME_ASSETS: &[(&str, &str)] = &[
    (
        "/static/sol-status/mark-connecting.svg",
        "status table and boot mark; stays unhashed, revalidated",
    ),
    (
        "/static/sol-status/mark-connecting-animated.svg",
        "status table when motion is allowed; stays unhashed, revalidated",
    ),
    (
        "/static/sol-status/mark-offline.svg",
        "status table; stays unhashed, revalidated",
    ),
    (
        "/static/sol-status/mark-attention.svg",
        "status table; stays unhashed, revalidated",
    ),
    (
        "/static/sol-status/mark-paused.svg",
        "status table; stays unhashed, revalidated",
    ),
    (
        "/static/sol-status/mark.svg",
        "status table and sun-arc mark; stays unhashed, revalidated",
    ),
    (
        "/static/vendor/leaflet/images/marker-icon.png",
        "Leaflet joins it to its hashed stylesheet's directory, so it resolves hashed",
    ),
    (
        "/static/vendor/leaflet/images/marker-icon-2x.png",
        "Leaflet joins it to its hashed stylesheet's directory, so it resolves hashed",
    ),
    (
        "/static/vendor/leaflet/images/marker-shadow.png",
        "Leaflet joins it to its hashed stylesheet's directory, so it resolves hashed",
    ),
];

/// Absolute references to a file in the same tree as the file making them.
const SAME_TREE_ABSOLUTE: &[(&str, &str)] = &[
    (
        "/static/shell_boot.js",
        "/static/sol-status/mark-connecting.svg",
    ),
    ("/static/sunarc.js", "/static/sol-status/mark.svg"),
    // websocket.js appends a status stem and .svg; every expansion is listed above.
    ("/static/websocket.js", "/static/sol-status/"),
];

/// Documents served with references left on their stable paths.
const UNREWRITTEN_DOCUMENTS: &[(&str, &str)] = &[(
    "/init",
    "the setup page carries this computer's setup facts, so it is never stored; its stable paths keep being served",
)];

fn is_resource(reference: &str) -> bool {
    let path = reference.split(['?', '#']).next().unwrap_or_default();
    reference.contains("/static/")
        || [
            ".html", ".js", ".css", ".svg", ".png", ".ico", ".woff2", ".woff", ".ttf", ".jpg",
            ".jpeg", ".webp", ".gif",
        ]
        .iter()
        .any(|extension| path.ends_with(extension))
}

fn served_text(path: &str) -> String {
    let record = assets::record(path).unwrap();
    let bytes = match record.kind {
        AssetKind::File => assets::served_bytes(path),
        AssetKind::Template => {
            rewrite_references(path, std::str::from_utf8(record.bytes).unwrap()).into_bytes()
        }
        AssetKind::Dynamic => record.bytes.to_vec(),
    };
    String::from_utf8(bytes).expect("text asset UTF-8")
}

#[test]
fn the_scanner_finds_resource_forms_and_resolves_relative_paths() {
    let source = r#"<script src="/static/a.js"></script><link href='../b.css?q=1'> url(images/c.png) const p='/app/home/static/home.js';"#;
    let refs: BTreeSet<_> = reference_spans(source)
        .into_iter()
        .map(|(start, end)| &source[start..end])
        .collect();
    assert_eq!(
        refs,
        BTreeSet::from([
            "/static/a.js",
            "../b.css?q=1",
            "images/c.png",
            "/app/home/static/home.js"
        ])
    );
    assert_eq!(
        resolve_reference("/static/vendor/a.css", "../b.css?q=1").as_deref(),
        Some("/static/b.css")
    );
    assert_eq!(resolve_reference("/static/a.css", "#mask"), None);
    assert_eq!(
        resolve_reference("/static/a.css", "data:image/png;base64,abc"),
        None
    );
}

#[test]
fn every_served_reference_resolves_to_a_hashed_path_or_is_listed() {
    let mut referenced = BTreeSet::new();
    let mut drift = BTreeSet::new();
    for asset in INVENTORY {
        assert_eq!(asset.byte_len, asset.bytes.len());
        assert_eq!(asset.sha256.len(), 64);
        if !["text/html", "text/javascript", "text/css"]
            .iter()
            .any(|kind| asset.content_type.starts_with(kind))
        {
            continue;
        }
        if UNREWRITTEN_DOCUMENTS
            .iter()
            .any(|(path, _)| *path == asset.path)
        {
            continue;
        }
        // A tree file's relative references resolve from its hashed address.
        let base = asset.hashed.unwrap_or(asset.path);
        let source = served_text(asset.path);
        for (start, end) in reference_spans(&source) {
            let reference = &source[start..end];
            if SAME_TREE_ABSOLUTE.contains(&(asset.path, reference)) {
                continue;
            }
            if !is_resource(reference) {
                continue;
            }
            let Some(path) = resolve_reference(base, reference) else {
                continue;
            };
            if let Some(target) = assets::hashed_record(&path) {
                referenced.insert(target.path.to_owned());
            } else if has_tree_hash_shape(&path) {
                drift.insert(format!(
                    "{} -> {reference}: not a hash of this build",
                    asset.path
                ));
            } else if let Some(target) = assets::record(&path) {
                referenced.insert(target.path.to_owned());
                if target.hashed.is_some() {
                    drift.insert(format!("{} -> {reference}: served unhashed", asset.path));
                }
            } else {
                drift.insert(format!("{} -> {reference}: no such asset", asset.path));
            }
        }
    }
    assert!(
        drift.is_empty(),
        "static references that don't resolve through the manifest:\n{}",
        drift.into_iter().collect::<Vec<_>>().join("\n")
    );

    // The instrument: the shell page's source, unrewritten, is drift.
    let raw = std::str::from_utf8(assets::record("/static/shell.html").unwrap().bytes).unwrap();
    let unhashed = reference_spans(raw)
        .into_iter()
        .filter_map(|(start, end)| resolve_reference("/static/shell.html", &raw[start..end]))
        .filter(|path| assets::record(path).is_some_and(|record| record.hashed.is_some()))
        .count();
    assert!(
        unhashed > 20,
        "the raw shell names {unhashed} unhashed tree files"
    );

    for (path, reason) in RUNTIME_ASSETS {
        assert!(!reason.is_empty());
        let record = assets::record(path).unwrap_or_else(|| panic!("runtime asset {path}"));
        // Each is still a tree file: served hashed to whatever names it that
        // way, and at its stable path to the code that builds it.
        assert!(record.hashed.is_some(), "{path}");
        referenced.insert((*path).to_owned());
    }
    for (from, reference) in SAME_TREE_ABSOLUTE {
        let source = std::str::from_utf8(assets::record(from).unwrap().bytes).unwrap();
        assert!(source.contains(reference), "{from} still names {reference}");
    }
    for (path, reason) in UNREWRITTEN_DOCUMENTS {
        assert!(!reason.is_empty());
        assert_eq!(
            assets::record(path).unwrap().kind,
            AssetKind::Dynamic,
            "{path}"
        );
    }
    let websocket = assets::record("/static/websocket.js").unwrap();
    let source = std::str::from_utf8(websocket.bytes).unwrap();
    for capture in Regex::new(r#""variant": "([^"]+)""#)
        .unwrap()
        .captures_iter(source)
    {
        let path = format!("/static/sol-status/{}.svg", &capture[1]);
        assert!(
            RUNTIME_ASSETS.iter().any(|(listed, _)| *listed == path),
            "unlisted runtime asset {path}"
        );
    }
    let payload = crate::registry::shell_payload_with_agents(true);
    for app in payload.apps {
        let record = assets::record(&app.workspace_url)
            .unwrap_or_else(|| panic!("workspace {}", app.workspace_url));
        assert!(
            record.tree.is_none(),
            "{} is a stable document",
            app.workspace_url
        );
        referenced.insert(app.workspace_url);
    }
    let unreferenced: Vec<_> = INVENTORY
        .iter()
        .map(|asset| asset.path)
        .filter(|path| !referenced.contains(*path))
        .collect();
    println!(
        "Unreferenced inventory assets (kept):\n{}",
        unreferenced.join("\n")
    );
}
