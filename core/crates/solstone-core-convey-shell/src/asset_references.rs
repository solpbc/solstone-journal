// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeSet;

use regex::Regex;

use crate::assets::INVENTORY;

fn resolve(base: &str, reference: &str) -> Option<String> {
    let reference = reference.split(['?', '#']).next().unwrap_or_default();
    if reference.is_empty() || reference.contains(':') || reference.starts_with("//") {
        return None;
    }
    let joined = if reference.starts_with('/') {
        reference.to_owned()
    } else {
        format!(
            "{}/{reference}",
            base.rsplit_once('/').expect("asset directory").0
        )
    };
    let mut parts = Vec::new();
    for part in joined.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            _ => parts.push(part),
        }
    }
    Some(format!("/{}", parts.join("/")))
}

fn references(source: &str) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    for pattern in [
        r#"(?i)\b(?:src|href)\s*=\s*["']([^"']+)["']"#,
        r#"(?i)url\(\s*["']?([^\s)"']+)["']?\s*\)"#,
        r#"["'`](/(?:static/|app/[^/"'`]+/static/)[^"'`]+)["'`]"#,
    ] {
        for captures in Regex::new(pattern)
            .expect("reference pattern")
            .captures_iter(source)
        {
            found.insert(captures[1].to_owned());
        }
    }
    found
}

// These URLs are constructed by browser libraries or the shell at runtime.
const RUNTIME_ASSETS: &[(&str, &str)] = &[
    (
        "/static/sol-status/mark-connecting.svg",
        "websocket status table",
    ),
    (
        "/static/sol-status/mark-connecting-animated.svg",
        "connecting animation when motion is allowed",
    ),
    (
        "/static/sol-status/mark-offline.svg",
        "websocket status table",
    ),
    (
        "/static/sol-status/mark-attention.svg",
        "websocket status table",
    ),
    (
        "/static/sol-status/mark-paused.svg",
        "websocket status table",
    ),
    ("/static/sol-status/mark.svg", "websocket status table"),
    (
        "/static/vendor/leaflet/images/marker-icon.png",
        "Leaflet default marker",
    ),
    (
        "/static/vendor/leaflet/images/marker-icon-2x.png",
        "Leaflet retina marker",
    ),
    (
        "/static/vendor/leaflet/images/marker-shadow.png",
        "Leaflet marker shadow",
    ),
];

#[test]
fn resolver_checks_resource_forms_and_relative_paths() {
    let refs = references(
        r#"<script src="/static/a.js"></script><link href='../b.css?q=1'> url(images/c.png) const p='/app/home/static/home.js';"#,
    );
    assert_eq!(refs.len(), 4);
    assert_eq!(
        resolve("/static/vendor/a.css", "../b.css?q=1").as_deref(),
        Some("/static/b.css")
    );
    assert_eq!(resolve("/static/a.css", "#mask"), None);
    assert_eq!(resolve("/static/a.css", "data:image/png;base64,abc"), None);
    assert!(refs.contains("/static/a.js"));
}

#[test]
fn embedded_references_resolve() {
    let paths: BTreeSet<_> = INVENTORY.iter().map(|asset| asset.path).collect();
    let mut referenced = BTreeSet::new();
    let mut missing = BTreeSet::new();
    for asset in INVENTORY {
        assert_eq!(asset.byte_len, asset.bytes.len());
        assert_eq!(asset.sha256.len(), 64);
        if !["text/html", "text/javascript", "text/css"]
            .iter()
            .any(|kind| asset.content_type.starts_with(kind))
        {
            continue;
        }
        let source = std::str::from_utf8(asset.bytes).expect("text asset UTF-8");
        for reference in references(source) {
            // websocket.js appends a STATUS_MAP stem and .svg; all expansions
            // are checked below, including the motion-specific variant.
            if asset.path == "/static/websocket.js" && reference == "/static/sol-status/" {
                continue;
            }
            // Navigation, media and API hrefs are routes, not static assets.
            let resource = reference.starts_with("/static/")
                || reference.contains("/static/")
                || [
                    ".html", ".js", ".css", ".svg", ".png", ".ico", ".woff2", ".woff", ".ttf",
                    ".jpg", ".jpeg", ".webp", ".gif",
                ]
                .iter()
                .any(|extension| {
                    reference
                        .split(['?', '#'])
                        .next()
                        .unwrap_or_default()
                        .ends_with(extension)
                });
            if !resource {
                continue;
            }
            if let Some(path) = resolve(asset.path, &reference) {
                if paths.contains(path.as_str()) {
                    referenced.insert(path);
                } else {
                    missing.insert(format!("{} -> {}", asset.path, reference));
                }
            }
        }
    }
    for (path, reason) in RUNTIME_ASSETS {
        assert!(!reason.is_empty());
        assert!(paths.contains(path), "runtime asset {path}");
        referenced.insert((*path).to_owned());
    }
    let websocket = INVENTORY
        .iter()
        .find(|a| a.path == "/static/websocket.js")
        .unwrap();
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
        assert!(
            paths.contains(app.workspace_url.as_str()),
            "workspace {}",
            app.workspace_url
        );
        referenced.insert(app.workspace_url);
    }
    assert!(
        missing.is_empty(),
        "unresolved references:\n{}",
        missing.into_iter().collect::<Vec<_>>().join("\n")
    );
    let unreferenced: Vec<_> = paths
        .into_iter()
        .filter(|path| !referenced.contains(*path))
        .collect();
    println!(
        "Unreferenced inventory assets (kept):\n{}",
        unreferenced.join("\n")
    );
}
