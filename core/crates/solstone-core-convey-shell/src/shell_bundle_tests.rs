// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The two-file shell: the bundles, the app table they carry, the build-time
//! gzip copies, and `/api/shell`'s per-request part. Answers go through the
//! same layers a connection serves them with, compression included.

use std::collections::BTreeSet;
use std::io::Read;

use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode, header};
use serde_json::Value;
use tower::ServiceExt;

use crate::assets::{self, AssetKind};
use crate::registry::{served_apps, shell_payload_with_agents, shell_registry};

mod bundle {
    include!("../../../build-support/convey_shell_bundle.rs");
    include!("../../../build-support/convey_entry_table.rs");
}

fn established_journal() -> tempfile::TempDir {
    let journal = tempfile::tempdir().unwrap();
    std::fs::create_dir(journal.path().join("config")).unwrap();
    std::fs::write(
        journal.path().join("config/journal.json"),
        br#"{"setup":{"completed_at":1}}"#,
    )
    .unwrap();
    journal
}

fn served(journal: &std::path::Path) -> Router {
    solstone_core_convey_http::serve::common_layers(crate::router(journal.to_owned()))
}

async fn get(
    app: &Router,
    path: &str,
    headers: &[(&str, &str)],
) -> (StatusCode, HeaderMap, Vec<u8>) {
    let mut request =
        Request::get(path).extension(solstone_core_convey_http::identity::AccessBasis::Localhost);
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let response = app
        .clone()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap()
        .to_vec();
    (status, headers, body)
}

fn gunzip(bytes: &[u8]) -> Vec<u8> {
    let mut plain = Vec::new();
    flate2::read::GzDecoder::new(bytes)
        .read_to_end(&mut plain)
        .expect("a gzip body");
    plain
}

fn one_line_per_header(path: &str, headers: &HeaderMap) {
    for name in headers.keys() {
        let lines: Vec<_> = headers.get_all(name).iter().collect();
        assert_eq!(lines.len(), 1, "{path} repeats {name}: {lines:?}");
    }
}

fn source(path: &str) -> &'static str {
    std::str::from_utf8(assets::record(path).unwrap().bytes).unwrap()
}

fn files_of<'a>(
    tags: &'a [bundle::BundledTag],
    which: &str,
) -> Vec<&'a bundle::BundledTag> {
    tags.iter()
        .filter(|tag| bundle::script_bundle_of(tag) == which)
        .collect()
}

fn assert_bundle_holds(text: &str, files: &[&bundle::BundledTag]) {
    let mut cursor = 0;
    for tag in files {
        let banner = format!("/* file: {} */\n", tag.path);
        assert_eq!(text.matches(&banner).count(), 1, "{}", tag.path);
        let at = text[cursor..]
            .find(&banner)
            .unwrap_or_else(|| panic!("{} is out of order", tag.path))
            + cursor;
        let body = source(&tag.path);
        let body_at = text[at..]
            .find(body)
            .unwrap_or_else(|| panic!("{} isn't in the bundle unchanged", tag.path))
            + at;
        cursor = body_at + body.len();
    }
    let lines: Vec<&str> = text.split('\n').collect();
    let table_start = text.find("var segments = [").unwrap() + "var segments = ".len();
    let table_end = table_start + text[table_start..].find("];").unwrap() + 1;
    let table: Vec<(usize, String)> = serde_json::from_str(&text[table_start..table_end]).unwrap();
    assert_eq!(table.len(), files.len());
    for ((line, name), tag) in table.iter().zip(files) {
        assert_eq!(format!("/{name}"), tag.path);
        let first = source(&tag.path).split('\n').next().unwrap();
        assert_eq!(lines[line - 1], first, "{name}");
    }
}

#[test]
fn the_bundles_hold_the_shell_page_files_in_order_one_banner_each_unchanged() {
    let page = source("/static/shell.html");
    let tags = bundle::shell_page_tags(page).unwrap();
    let boot = files_of(&tags.scripts, bundle::SHELL_SCRIPT_BUNDLE);
    let rest = files_of(&tags.scripts, bundle::SHELL_REST_BUNDLE);
    assert!(boot.len() >= 20 && rest.len() >= 5 && tags.styles.len() == 3);
    let script = source("/static/shell.js");
    let rest_script = source("/static/shell-rest.js");
    assert_bundle_holds(script, &boot);
    assert_bundle_holds(rest_script, &rest);
    assert!(script.contains("SOL_SHELL_REGISTRY"));
    assert!(!rest_script.contains("SOL_SHELL_REGISTRY"));
    assert!(!script.contains("/* file: /static/sunarc.js */"));
    assert!(rest_script.contains("/* file: /static/sunarc.js */"));
    let mut cursor = 0;
    let style = source("/static/shell.css");
    for tag in &tags.styles {
        let banner = format!("/* file: {} */\n", tag.path);
        assert_eq!(style.matches(&banner).count(), 1, "{}", tag.path);
        let at = style[cursor..]
            .find(&banner)
            .unwrap_or_else(|| panic!("{} is out of order", tag.path))
            + cursor;
        let body = source(&tag.path);
        let body_at = style[at..]
            .find(body)
            .unwrap_or_else(|| panic!("{} isn't in the bundle unchanged", tag.path))
            + at;
        cursor = body_at + body.len();
    }
}

#[test]
fn the_served_shell_page_loads_the_two_bundles_and_keeps_its_inline_scripts() {
    let page = String::from_utf8(assets::served_bytes("/static/shell.html")).unwrap();
    let script = assets::record("/static/shell.js").unwrap().hashed.unwrap();
    let rest = assets::record("/static/shell-rest.js").unwrap().hashed.unwrap();
    let style = assets::record("/static/shell.css").unwrap().hashed.unwrap();
    assert_eq!(page.matches("<script src=").count(), 1, "{page}");
    assert_eq!(page.matches("rel=\"stylesheet\"").count(), 1, "{page}");
    assert!(page.contains(&format!(
        "<script src=\"{script}\" defer data-shell-part=\"boot\"></script>"
    )));
    let rest_tag = format!("<script data-shell-part=\"rest\" data-src=\"{rest}\"></script>");
    assert!(page.contains(&rest_tag), "{page}");
    assert!(!rest_tag.contains(" src="));
    assert!(page.contains(&format!("<link rel=\"stylesheet\" href=\"{style}\">")));
    // The cross-document opt-in stays in the document, ahead of the bundled
    // sheet. A sheet that is parsed but not yet active is not seen when the
    // new page is revealed, and the transition then rejects with no object.
    let opt_in = page.find("@view-transition { navigation: auto; }").unwrap();
    let sheet = page.find("rel=\"stylesheet\"").unwrap();
    assert!(page[..opt_in].contains("<style>"));
    assert!(opt_in < sheet);
    let source = source("/static/shell.html");
    assert_eq!(
        page.matches("<script>").count(),
        source.matches("<script>").count(),
        "every inline script stays"
    );
    // The stylesheet bundle sits where the first stylesheet was, and the
    // script bundle where the first script was.
    assert!(page.find(style).unwrap() < page.find("rel=\"icon\"").unwrap());
    assert!(page.find(script).unwrap() < page.find("</head>").unwrap());
}

#[test]
fn a_change_to_any_file_changes_its_bundle_and_the_shell_tree_hash() {
    let page = source("/static/shell.html");
    let tags = bundle::shell_page_tags(page).unwrap();
    let files: Vec<_> = tags
        .scripts
        .iter()
        .map(|tag| (tag.clone(), source(&tag.path).to_owned()))
        .collect();
    let built = bundle::script_bundle(&files, "", "shell.js.map").unwrap();
    for index in 0..files.len() {
        let mut changed = files.clone();
        changed[index].1.push_str("\n// changed\n");
        let rebuilt = bundle::script_bundle(&changed, "", "shell.js.map").unwrap();
        assert_ne!(rebuilt.text, built.text, "{}", files[index].0.path);
    }
    // The bundles are files of the shell's tree, so their bytes are in its hash.
    for path in [
        "/static/shell.js",
        "/static/shell-rest.js",
        "/static/shell.css",
        "/static/shell.js.map",
        "/static/shell-rest.js.map",
    ] {
        assert_eq!(
            assets::record(path).unwrap().tree,
            Some("/static"),
            "{path}"
        );
    }
    // And the build reruns on any change under the shell's static directory.
    let build = include_str!("../build.rs");
    assert!(build.contains("collect_files(&static_root, &mut files)"));
}

#[test]
fn the_lexer_refuses_what_it_cannot_read_and_names_the_file_and_line() {
    for (source, says) in [
        ("var a = 'open;\n", "a.js:1: a string never ends"),
        (
            "function f() {\n  return 1;\n",
            "a.js:1: a bracket opened here never closes",
        ),
        (
            "var a = [1, 2);\n",
            "a.js:1: ')' closes the '[' opened on line 1",
        ),
        ("/* never closed\n", "a.js:1: a comment never ends"),
        ("var t = `a ${b\n", "a.js:1: a template"),
        ("var r = /abc\n", "a.js:1: a regular expression never ends"),
        ("if (a) {}\n/b/.test(c);\n", "a.js:2: can't tell"),
        ("a++ / 2;\n", "a.js:1: can't tell"),
        ("var \\u0061 = 1;\n", "a.js:1: a character"),
        ("const { a, b } = c;\n", "a.js:1: a top-level destructuring"),
    ] {
        let error = bundle::script_facts("a.js", source).unwrap_err();
        assert!(error.starts_with(says), "{source:?}: {error}");
    }
}

#[test]
fn the_lexer_finds_every_top_level_name_and_strictness() {
    let facts = bundle::script_facts(
        "a.js",
        "// note\n'use strict';\nfunction one() { var inner = 1; }\nasync function two() {}\n\
         class Three {}\nvar four = 4, five = { a: 1 }, six;\nlet seven = (x) => x / 2\n\
         const eight = /a\\/b/g;\nif (four) /x/.test('x');\n(function () { var hidden; })();\n\
         var nine = `t ${one()} ${`n ${2}`}`;\n",
    )
    .unwrap();
    assert!(facts.strict);
    assert_eq!(
        facts.top_level,
        [
            "one", "two", "Three", "four", "five", "six", "seven", "eight", "nine"
        ]
    );
    assert!(
        !bundle::script_facts("b.js", "(function(){ 'use strict'; })();\n")
            .unwrap()
            .strict
    );
}

#[test]
fn the_bundle_refuses_a_shared_or_reserved_top_level_name() {
    let tag = |path: &str| bundle::BundledTag {
        start: 0,
        end: 0,
        path: path.to_owned(),
        media: None,
        part: "boot".to_owned(),
    };
    let twice = [
        (tag("/static/a.js"), "var shared = 1;\n".to_owned()),
        (tag("/static/b.js"), "function shared() {}\n".to_owned()),
    ];
    let error = bundle::script_bundle(&twice, "", "m").err().unwrap();
    assert!(error.contains("shared"), "{error}");
    // Built apart, each file is fine. The page check is what still refuses
    // the name, which is how the two bundles are produced.
    assert!(bundle::script_bundle(&twice[..1], "", "m").is_ok());
    assert!(bundle::script_bundle(&twice[1..], "", "m").is_ok());
    let across = bundle::refuse_shared_top_level_names(&twice).err().unwrap();
    assert!(across.contains("shared"), "{across}");
    let reserved = [(tag("/static/a.js"), "var location = 1;\n".to_owned())];
    assert!(bundle::script_bundle(&reserved, "", "m").is_err());
}

#[test]
fn a_stylesheet_with_a_media_query_keeps_it_and_its_place() {
    let tags = bundle::shell_page_tags(source("/static/shell.html")).unwrap();
    let dark = tags
        .styles
        .iter()
        .position(|tag| tag.path == "/static/tokens-dark.css")
        .unwrap();
    assert_eq!(
        tags.styles[dark].media.as_deref(),
        Some("(prefers-color-scheme: dark)")
    );
    let style = source("/static/shell.css");
    let tokens = style.find("/* file: /static/tokens.css */").unwrap();
    let wrapped = style
        .find("/* file: /static/tokens-dark.css */\n@media (prefers-color-scheme: dark) {\n")
        .unwrap();
    let app = style.find("/* file: /static/app.css */").unwrap();
    assert!(tokens < wrapped && wrapped < app);
    let error = bundle::check_stylesheet("a.css", "@import url(b.css);\n").unwrap_err();
    assert!(error.contains("@import"), "{error}");
    assert!(bundle::check_stylesheet("a.css", "a { color: red;\n").is_err());
}

fn assert_source_map(script_path: &str, map_path: &str, which: &str) {
    let map: Value = serde_json::from_str(source(map_path)).unwrap();
    let sources: Vec<&str> = map["sources"]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_str().unwrap())
        .collect();
    let tags = bundle::shell_page_tags(source("/static/shell.html")).unwrap();
    let paths: Vec<&str> = tags
        .scripts
        .iter()
        .filter(|tag| bundle::script_bundle_of(tag) == which)
        .map(|tag| tag.path.as_str())
        .collect();
    assert_eq!(sources, paths, "{map_path}");
    // Decode: every mapped line names a file and a line holding the same text.
    let lines: Vec<&str> = source(script_path).split('\n').collect();
    let decode = |segment: &str| -> Vec<i64> {
        let mut values = Vec::new();
        let (mut value, mut shift) = (0i64, 0);
        for c in segment.bytes() {
            let digit = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/"
                .iter()
                .position(|d| *d == c)
                .unwrap() as i64;
            value += (digit & 31) << shift;
            if digit & 32 == 0 {
                values.push(if value & 1 == 1 {
                    -(value >> 1)
                } else {
                    value >> 1
                });
                value = 0;
                shift = 0;
            } else {
                shift += 5;
            }
        }
        values
    };
    let (mut file, mut line) = (0i64, 0i64);
    let mut mapped = 0;
    for (index, segment) in map["mappings"].as_str().unwrap().split(';').enumerate() {
        if segment.is_empty() {
            continue;
        }
        let values = decode(segment);
        file += values[1];
        line += values[2];
        let original = source(sources[file as usize])
            .split('\n')
            .nth(line as usize)
            .unwrap();
        assert_eq!(lines[index], original, "bundle line {}", index + 1);
        mapped += 1;
    }
    let source_lines: usize = paths.iter().map(|path| source(path).matches('\n').count()).sum();
    assert!(
        mapped * 2 > source_lines,
        "{script_path} mapped {mapped} of {source_lines} source lines"
    );
}

#[test]
fn the_source_map_maps_each_bundle_line_to_its_file_line() {
    assert_source_map(
        "/static/shell.js",
        "/static/shell.js.map",
        bundle::SHELL_SCRIPT_BUNDLE,
    );
    assert_source_map(
        "/static/shell-rest.js",
        "/static/shell-rest.js.map",
        bundle::SHELL_REST_BUNDLE,
    );
}

#[test]
fn a_tags_part_chooses_its_bundle() {
    let page = source("/static/shell.html");
    let moved = page.replacen(
        "<script src=\"/static/sunarc.js\" defer data-shell-part=\"rest\"></script>",
        "<script src=\"/static/sunarc.js\" defer data-shell-part=\"boot\"></script>",
        1,
    );
    assert_ne!(moved, page);
    let build = |page: &str| {
        let tags = bundle::shell_page_tags(page).unwrap();
        let files: Vec<_> = tags
            .scripts
            .iter()
            .map(|tag| (tag.clone(), source(&tag.path).to_owned()))
            .collect();
        let boot: Vec<_> = files
            .iter()
            .filter(|(tag, _)| bundle::script_bundle_of(tag) == bundle::SHELL_SCRIPT_BUNDLE)
            .cloned()
            .collect();
        let rest: Vec<_> = files
            .iter()
            .filter(|(tag, _)| bundle::script_bundle_of(tag) == bundle::SHELL_REST_BUNDLE)
            .cloned()
            .collect();
        (
            bundle::script_bundle(&boot, "globalThis.SOL_SHELL_REGISTRY = {};\n", "shell.js.map")
                .unwrap()
                .text,
            bundle::script_bundle(&rest, "", "shell-rest.js.map")
                .unwrap()
                .text,
        )
    };
    let (boot, rest) = build(page);
    assert!(rest.contains("/* file: /static/sunarc.js */"));
    assert!(!boot.contains("/* file: /static/sunarc.js */"));
    let (boot, rest) = build(&moved);
    assert!(boot.contains("/* file: /static/sunarc.js */"));
    assert!(!rest.contains("/* file: /static/sunarc.js */"));
    let bare = "<link rel=\"stylesheet\" href=\"/static/app.css\">\n\
                <script src=\"/static/error-handler.js\" defer></script>\n";
    let error = bundle::shell_page_tags(bare).unwrap_err();
    assert!(error.contains("data-shell-part"), "{error}");
}

// The app table in the bundle, with the names `/api/shell` gives, is exactly
// what each router serves: agents mounted or not.
#[tokio::test]
async fn the_shipped_app_table_and_the_served_names_match_each_router() {
    let script = source("/static/shell.js");
    let start = script.find("globalThis.SOL_SHELL_REGISTRY = ").unwrap()
        + "globalThis.SOL_SHELL_REGISTRY = ".len();
    let end = start + script[start..].find(";\n").unwrap();
    let table: Value = serde_json::from_str(&script[start..end]).unwrap();
    assert_eq!(table, serde_json::to_value(shell_registry()).unwrap());
    for include_agents in [false, true] {
        let expected =
            serde_json::to_value(shell_payload_with_agents(include_agents).apps).unwrap();
        let names: Vec<&str> = served_apps(include_agents).map(|app| app.name).collect();
        let joined: Vec<Value> = names
            .iter()
            .map(|name| {
                table["apps"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|app| app["name"] == *name)
                    .cloned()
                    .unwrap_or_else(|| panic!("{name} isn't in the table"))
            })
            .collect();
        assert_eq!(Value::Array(joined), expected, "agents: {include_agents}");
    }

    // A router without the agents routes names exactly its served apps, and
    // `/api/shell` carries only the per-request part.
    let journal = established_journal();
    let app = served(journal.path());
    let (status, headers, body) = get(&app, "/api/shell", &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers[header::CACHE_CONTROL], "no-store");
    let state: Value = serde_json::from_slice(&body).unwrap();
    let keys: BTreeSet<&str> = state
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        BTreeSet::from(["app_names", "clock", "pinned", "version"])
    );
    let names: Vec<&str> = served_apps(false).map(|app| app.name).collect();
    assert_eq!(state["app_names"], serde_json::json!(names));
    assert_eq!(state["version"], env!("CARGO_PKG_VERSION"));
    assert!(
        state["clock"]["today"]
            .as_str()
            .is_some_and(|day| day.len() == 8)
    );
}

// Every compressible file is answered from its build-time gzip copy, and the
// answer is the plain answer gzipped, with the same validator rules as before.
#[tokio::test]
async fn every_precompressed_answer_is_the_plain_answer_with_its_validators() {
    let journal = established_journal();
    let app = served(journal.path());
    let mut checked = 0;
    let mut paths: Vec<(String, Option<&'static str>)> = Vec::new();
    for record in assets::INVENTORY {
        if record.gzip.is_empty() {
            continue;
        }
        assert_eq!(record.kind, AssetKind::File, "{}", record.path);
        if record.path == "/static/shell.html" {
            paths.push(("/app/home/".to_owned(), Some(record.etag)));
            paths.push(("/".to_owned(), Some(record.etag)));
        } else {
            paths.push((record.path.to_owned(), Some(record.etag)));
        }
        if let Some(hashed) = record.hashed {
            paths.push((hashed.to_owned(), None));
        }
    }
    for (path, etag) in &paths {
        let (status, plain_headers, plain) = get(&app, path, &[]).await;
        // The agents app's files are served only where its routes are mounted.
        if status == StatusCode::NOT_FOUND && path.starts_with("/app/agents/") {
            continue;
        }
        assert_eq!(status, StatusCode::OK, "{path}");
        assert!(
            plain_headers.get(header::CONTENT_ENCODING).is_none(),
            "{path}"
        );
        let (status, headers, packed) =
            get(&app, path, &[("accept-encoding", "gzip, deflate, br")]).await;
        assert_eq!(status, StatusCode::OK, "{path}");
        assert_eq!(headers[header::CONTENT_ENCODING], "gzip", "{path}");
        assert_eq!(gunzip(&packed), plain, "{path}");
        assert_eq!(
            headers[header::CONTENT_LENGTH],
            packed.len().to_string().as_str(),
            "{path}"
        );
        assert_eq!(headers[header::VARY], "accept-encoding", "{path}");
        assert_eq!(
            headers[header::CACHE_CONTROL],
            plain_headers[header::CACHE_CONTROL],
            "{path}"
        );
        one_line_per_header(path, &headers);
        if let Some(etag) = etag {
            let opaque = etag.trim_matches('"');
            assert_eq!(
                headers[header::ETAG],
                format!("\"{opaque}-gzip\"").as_str(),
                "{path}"
            );
            assert_eq!(plain_headers[header::ETAG], *etag, "{path}");
            for held in [etag.to_string(), format!("\"{opaque}-gzip\"")] {
                let (status, headers, body) = get(
                    &app,
                    path,
                    &[("accept-encoding", "gzip"), ("if-none-match", &held)],
                )
                .await;
                assert_eq!(status, StatusCode::NOT_MODIFIED, "{path} with {held}");
                assert!(body.is_empty());
                assert!(headers.get(header::CONTENT_ENCODING).is_none(), "{path}");
                one_line_per_header(path, &headers);
            }
        } else {
            assert!(headers.get(header::ETAG).is_none(), "{path}");
        }
        checked += 1;
    }
    assert!(checked > 100, "{checked}");
}

// The answer to "would the server gzip this?" is the compression layer's own,
// for the forms of Accept-Encoding clients send and a few they shouldn't.
#[tokio::test]
async fn gzip_is_chosen_exactly_when_the_compression_layer_would_choose_it() {
    let text = "a".repeat(4096);
    let app = solstone_core_convey_http::serve::common_layers(Router::new().route(
        "/",
        axum::routing::get(move || {
            let text = text.clone();
            async move { ([(header::CONTENT_TYPE, "text/plain")], text) }
        }),
    ));
    for value in [
        "",
        "gzip",
        "x-gzip",
        "GZIP",
        "gzip, deflate, br, zstd",
        "br",
        "*",
        "identity",
        "gzip;q=0",
        "gzip;q=0.001",
        "gzip;q=0.5, identity",
        "gzip;q=0.5, identity;q=0.5",
        "gzip;q=0.5, identity;q=0.4",
        "identity;q=0, gzip;q=0.1",
        "gzip;q=1.0",
        "gzip;q=1.5",
        "gzip;q=0.5000",
        "gzip;q=abc",
        "gzip;q= 0.5",
        "gzip; q=0.5",
        "deflate;q=1, gzip;q=0.2",
        "gzip;q=0, gzip;q=1",
    ] {
        let mut headers = HeaderMap::new();
        if !value.is_empty() {
            headers.insert(header::ACCEPT_ENCODING, value.parse().unwrap());
        }
        let request_headers: Vec<(&str, &str)> = if value.is_empty() {
            vec![]
        } else {
            vec![("accept-encoding", value)]
        };
        let (_, response, _) = get(&app, "/", &request_headers).await;
        let layer = response.get(header::CONTENT_ENCODING).is_some();
        assert_eq!(
            crate::asset_cache::accepts_gzip(&headers),
            layer,
            "Accept-Encoding: {value:?}"
        );
    }
}

fn served_entry_table(page: &[u8]) -> Value {
    let page = std::str::from_utf8(page).unwrap();
    let start = page
        .find("window.__SOL_ENTRY_TABLE__ = ")
        .expect("the page carries the table")
        + "window.__SOL_ENTRY_TABLE__ = ".len();
    let end = start
        + page[start..]
            .find(";\n")
            .expect("the table ends its statement");
    serde_json::from_str(&page[start..end]).expect("the table is JSON")
}

fn stem(path: &str) -> String {
    // A hashed path is its stable path with one 16-hex directory added.
    path.split('/')
        .filter(|part| !(part.len() == 16 && part.chars().all(|c| c.is_ascii_hexdigit())))
        .collect::<Vec<_>>()
        .join("/")
}

#[tokio::test]
async fn the_entry_table_is_every_views_fragment_tags_at_names_the_journal_serves() {
    let journal = established_journal();
    let app = served(journal.path());
    let (status, _, page) = get(&app, "/app/home/", &[]).await;
    assert_eq!(status, StatusCode::OK);
    let table = served_entry_table(&page);
    let views = table["views"].as_object().unwrap();
    let registry = shell_registry();
    assert_eq!(
        views.len(),
        registry.apps.len(),
        "one row per registry view"
    );
    for view in &registry.apps {
        let row = &views[view.name];
        assert_eq!(row["fragment"], view.workspace_url.as_str());
        let (status, _, fragment) = get(&app, &view.workspace_url, &[]).await;
        // The agents app is mounted only on routers that carry its routes.
        if view.name == "agents" && status == StatusCode::NOT_FOUND {
            continue;
        }
        assert_eq!(status, StatusCode::OK, "{}", view.name);
        // The served fragment names hashed files; the table must name the same.
        let facts =
            bundle::fragment_facts(view.name, std::str::from_utf8(&fragment).unwrap()).unwrap();
        let scripts: Vec<&str> = row["scripts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|script| script["src"].as_str().unwrap())
            .collect();
        assert_eq!(
            scripts,
            facts
                .scripts
                .iter()
                .map(|s| s.src.as_str())
                .collect::<Vec<_>>(),
            "{} scripts",
            view.name
        );
        let styles: Vec<&str> = row["styles"]
            .as_array()
            .unwrap()
            .iter()
            .map(|style| style.as_str().unwrap())
            .collect();
        assert_eq!(
            styles,
            facts.styles.iter().map(String::as_str).collect::<Vec<_>>(),
            "{} styles",
            view.name
        );
        for path in scripts.iter().chain(styles.iter()) {
            assert_ne!(stem(path), *path, "{path} is a hashed name");
            let (status, _, _) = get(&app, path, &[]).await;
            assert_eq!(status, StatusCode::OK, "{path}");
        }
    }
    let date_code: Vec<String> = table["dateViewCode"]
        .as_array()
        .unwrap()
        .iter()
        .map(|path| stem(path.as_str().unwrap()))
        .collect();
    assert_eq!(
        date_code.iter().collect::<BTreeSet<_>>(),
        [
            "/static/date-nav.css",
            "/static/date-nav.js",
            "/static/day-grid.css",
            "/static/day-grid.js"
        ]
        .iter()
        .map(|path| path.to_string())
        .collect::<BTreeSet<_>>()
        .iter()
        .collect::<BTreeSet<_>>(),
    );
}

#[test]
fn a_fragment_change_changes_its_row() {
    let hashed = |path: &str| Some(format!("/hashed{path}"));
    let before = bundle::fragment_facts("v", "<script src=\"/a.js\"></script>").unwrap();
    let after = bundle::fragment_facts(
        "v",
        "<script src=\"/b.js\" data-route-prefix=\"/app/v/x/\"></script><link rel=\"stylesheet\" href=\"/c.css\">",
    )
    .unwrap();
    let before_row = bundle::entry_row("/app/v/workspace", &before, &hashed);
    let after_row = bundle::entry_row("/app/v/workspace", &after, &hashed);
    assert_ne!(before_row, after_row);
    assert_eq!(after_row["scripts"][0]["src"], "/hashed/b.js");
    assert_eq!(after_row["scripts"][0]["routePrefix"], "/app/v/x/");
    assert_eq!(after_row["styles"][0], "/hashed/c.css");
}

#[test]
fn a_fragment_reads_only_real_tags_and_its_declared_reads() {
    let facts = bundle::fragment_facts(
        "v",
        "<!-- first-read: /app/v/{day} -> /app/v/api/day/{day} /app/v/api/other -->\n\
         <script>const x = '<script src=\"/inline.js\"></' + 'script>';</script>\n\
         <template><script src=\"/template.js\"></script></template>\n\
         <style>/* <link rel=\"stylesheet\" href=\"/style.css\"> */</style>\n\
         <!-- a comment with <script src=\"/comment.js\"></script> inside -->\n\
         <script src=\"/real.js\"></script>\n\
         <!-- open-day: /app/v/ -> /app/v/api/open -->",
    )
    .unwrap();
    assert_eq!(
        facts.scripts,
        vec![bundle::FragmentScript {
            src: "/real.js".into(),
            route_prefix: None
        }]
    );
    assert!(facts.styles.is_empty());
    assert_eq!(facts.reads[0].route, "/app/v/{day}");
    assert_eq!(
        facts.reads[0].urls,
        ["/app/v/api/day/{day}", "/app/v/api/other"]
    );
    assert_eq!(facts.open_day.unwrap().urls, ["/app/v/api/open"]);
}

#[test]
fn a_declaration_a_page_could_not_honour_stops_the_build() {
    for text in [
        // A date the route doesn't carry would have to come from the page.
        "<!-- first-read: /app/v/ -> /app/v/api/day/{day} -->",
        "<!-- first-read: /app/v/{facet} -> /app/v/api/x -->",
        "<!-- first-read: app/v/ -> /app/v/api/x -->",
        "<!-- first-read: /app/v/ -> api/x -->",
        "<!-- first-read: /app/v/ -->",
        "<!-- open-day: /app/v/{day} -> /app/v/api/open -->",
        "<!-- open-day: /app/v -> /app/v/api/open -->",
    ] {
        assert!(bundle::fragment_facts("v", text).is_err(), "{text}");
    }
}

/// A declared first read that the view never sends would be a wasted server
/// run on every open, so each declared address has to be in the view's code.
#[tokio::test]
async fn every_declared_first_read_is_one_the_views_code_asks_for() {
    let journal = established_journal();
    let app = served(journal.path());
    let mut checked = 0;
    for view in &shell_registry().apps {
        let (_, _, fragment) = get(&app, &view.workspace_url, &[]).await;
        let fragment = String::from_utf8(fragment).unwrap();
        let facts = bundle::fragment_facts(view.name, &fragment).unwrap();
        if facts.reads.is_empty() {
            continue;
        }
        let mut code = fragment.clone();
        for script in &facts.scripts {
            if script.src.starts_with("/static/vendor/") {
                continue;
            }
            let (status, _, body) = get(&app, &script.src, &[]).await;
            assert_eq!(status, StatusCode::OK, "{}", script.src);
            code.push_str(&String::from_utf8_lossy(&body));
        }
        for row in &facts.reads {
            for url in &row.urls {
                // A query the view builds at run time is matched on its path
                // here; the lab's early-read counts prove the whole address.
                let asked = url
                    .split('?')
                    .next()
                    .unwrap_or_default()
                    .replace("{day}", "${day}");
                assert!(
                    code.contains(&asked),
                    "{}: nothing asks for {asked}",
                    view.name
                );
                checked += 1;
            }
        }
    }
    assert!(checked >= 6, "the declared reads were found");
}
