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

/// Paths built at runtime, with how each one resolves.
const RUNTIME_ASSETS: &[(&str, &str)] = &[
    (
        "/static/sol-status/mark-connecting.svg",
        "status table and boot mark; resolved beside the script's own hashed address",
    ),
    (
        "/static/sol-status/mark-connecting-animated.svg",
        "status table when motion is allowed; resolved beside the script's own hashed address",
    ),
    (
        "/static/sol-status/mark-offline.svg",
        "status table; resolved beside the script's own hashed address",
    ),
    (
        "/static/sol-status/mark-attention.svg",
        "status table; resolved beside the script's own hashed address",
    ),
    (
        "/static/sol-status/mark-paused.svg",
        "status table; resolved beside the script's own hashed address",
    ),
    (
        "/static/sol-status/mark.svg",
        "status table and sun-arc mark; resolved beside the script's own hashed address",
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

/// The stable fallback each status-mark script keeps for when it isn't served
/// under a hashed address (a page from before hashing, the setup window).
const SAME_TREE_ABSOLUTE: &[(&str, &str)] = &[
    ("/static/shell_boot.js", "/static/sol-status/"),
    ("/static/sunarc.js", "/static/sol-status/"),
    ("/static/websocket.js", "/static/sol-status/"),
    // The same paths, inside the shell's script bundles. Sun arc's fallback
    // lives in the rest file; boot still names the status marks itself.
    ("/static/shell.js", "/static/sol-status/"),
    ("/static/shell-rest.js", "/static/sol-status/"),
];

/// Documents served with references left on their stable paths.
const UNREWRITTEN_DOCUMENTS: &[(&str, &str)] = &[(
    "/init",
    "the setup page carries this computer's setup facts, so it is never stored; its stable paths keep being served",
)];

/// Served files no page references, each with why it stays.
const KEPT_UNREFERENCED: &[(&str, &str)] = &[
    (
        "/init",
        "the setup page, reached by navigation and the session gate",
    ),
    ("/static/shell.html", "served by every view route"),
    (
        "/static/icons/LICENSE",
        "the icon set's license, which ships beside it",
    ),
    (
        "/static/vendor/leaflet/LICENSE",
        "the library's license, which ships beside it",
    ),
    (
        "/static/vendor/h3/LICENSE",
        "the library's license, which ships beside it",
    ),
    (
        "/static/vendor/h3/NOTICE",
        "the library's notice, which ships beside it",
    ),
    (
        "/static/sol-status/mark-error.svg",
        "kept with the status mark family; the status table deliberately doesn't name it",
    ),
];

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
    // The shell page loads its stylesheets and scripts through the two
    // bundles, so each file in one is referenced through it; the script
    // bundle names its source map in its last line.
    for (bundle, file) in assets::shell_bundle_segments() {
        assert!(
            referenced.contains(*bundle),
            "the shell page loads {bundle}"
        );
        assert!(assets::record(file).is_some(), "{file} is in {bundle}");
        referenced.insert((*file).to_owned());
    }
    let script_bundle = source_of("/static/shell.js");
    assert!(script_bundle.ends_with("//# sourceMappingURL=shell.js.map\n"));
    referenced.insert("/static/shell.js.map".to_owned());
    let rest_bundle = source_of("/static/shell-rest.js");
    assert!(rest_bundle.ends_with("//# sourceMappingURL=shell-rest.js.map\n"));
    referenced.insert("/static/shell-rest.js.map".to_owned());
    let unreferenced: Vec<_> = INVENTORY
        .iter()
        .map(|asset| asset.path)
        .filter(|path| !referenced.contains(*path))
        .filter(|path| !path.starts_with("/static/tests/"))
        // Fetched by name, from the source the owner picks to import.
        .filter(|path| !path.starts_with("/app/import/api/guide/"))
        .filter(|path| !KEPT_UNREFERENCED.iter().any(|(kept, _)| kept == path))
        .collect();
    assert!(
        unreferenced.is_empty(),
        "served files nothing references; remove them or list them with a reason:\n{}",
        unreferenced.join("\n")
    );
    for (path, reason) in KEPT_UNREFERENCED {
        assert!(!reason.is_empty());
        assert!(assets::record(path).is_some(), "{path} is listed but gone");
    }
}

fn source_of(path: &str) -> &'static str {
    std::str::from_utf8(assets::record(path).unwrap().bytes).unwrap_or("")
}

/// What a view's fragment and its own scripts use from the libraries the shell
/// doesn't carry for it. A view that uses one names it in its fragment.
#[test]
fn every_view_declares_the_shared_code_it_uses() {
    const NEEDS: &[(&[&str], &[&str])] = &[
        (
            &["renderMarkdown("],
            &[
                "/static/vendor/marked/marked.min.js",
                "/static/vendor/dompurify/dompurify.min.js",
            ],
        ),
        (
            &["data-date-nav", "DateNav."],
            &["/static/date-nav.js", "/static/date-nav.css"],
        ),
        (
            &["DayGrid."],
            &[
                "/static/date-nav.js",
                "/static/day-grid.js",
                "/static/day-grid.css",
            ],
        ),
    ];
    let mut missing = Vec::new();
    let mut declaring = 0;
    for fragment in INVENTORY
        .iter()
        .filter(|asset| asset.path.starts_with("/app/") && asset.path.ends_with("/workspace"))
    {
        let view = fragment.path.trim_end_matches("/workspace");
        let own = format!("{view}/static/");
        let html = source_of(fragment.path);
        let code: Vec<&str> = INVENTORY
            .iter()
            .filter(|asset| {
                asset.path.starts_with(&own) && asset.content_type.starts_with("text/javascript")
            })
            .map(|asset| source_of(asset.path))
            .chain([html])
            .collect();
        for (uses, declares) in NEEDS {
            if !uses
                .iter()
                .any(|name| code.iter().any(|text| text.contains(name)))
            {
                continue;
            }
            declaring += 1;
            for declared in *declares {
                if !html.contains(declared) {
                    missing.push(format!("{view} doesn't declare {declared}"));
                }
            }
        }
    }
    assert!(missing.is_empty(), "{}", missing.join("\n"));
    assert!(declaring >= 10, "the scan found only {declaring} uses");
}

/// The shell doesn't load the markdown libraries, so nothing in it may call
/// them. A shell script that needs markdown puts the libraries back in the
/// shell for it.
#[test]
fn no_shell_script_calls_render_markdown() {
    let shell = source_of("/static/shell.html");
    assert!(!shell.contains("vendor/marked"), "the shell names marked");
    let callers: Vec<_> = INVENTORY
        .iter()
        .filter(|asset| {
            asset.path.starts_with("/static/")
                && !asset.path.starts_with("/static/tests/")
                && !asset.path.starts_with("/static/vendor/")
                // The bundle holds the shell's scripts, each checked on its own.
                && asset.path != "/static/shell.js"
                && asset.path != "/static/shell-rest.js"
                && asset.content_type.starts_with("text/javascript")
        })
        .filter(|asset| {
            let calls = source_of(asset.path).matches("renderMarkdown(").count();
            calls > usize::from(asset.path == "/static/app.js")
        })
        .map(|asset| asset.path)
        .collect();
    assert!(callers.is_empty(), "{callers:?}");
}
