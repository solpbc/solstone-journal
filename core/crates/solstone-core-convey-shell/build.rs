// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;
use sha2::{Digest, Sha256};

include!("../../build-support/convey_asset_refs.rs");
include!("../../build-support/convey_shell_bundle.rs");
include!("../../build-support/convey_entry_table.rs");

// The app table is generated from the same source the shell serves from.
#[allow(dead_code)]
#[path = "src/registry.rs"]
mod registry;

fn collect_files(directory: &Path, files: &mut Vec<PathBuf>) {
    println!("cargo:rerun-if-changed={}", directory.display());
    let mut entries: Vec<_> = fs::read_dir(directory)
        .expect("embedded asset directory is readable")
        .map(|entry| entry.expect("embedded asset directory entry is readable"))
        .collect();
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            collect_files(&path, files);
        } else {
            println!("cargo:rerun-if-changed={}", path.display());
            files.push(path);
        }
    }
}

fn content_type(path: &Path) -> &'static str {
    match path.extension().and_then(|extension| extension.to_str()) {
        Some("css") => "text/css; charset=utf-8",
        Some("html") => "text/html; charset=utf-8",
        Some("ico") => "image/x-icon",
        Some("jpeg") | Some("jpg") => "image/jpeg",
        Some("js") => "text/javascript; charset=utf-8",
        Some("json") | Some("map") => "application/json",
        Some("png") => "image/png",
        Some("svg") => "image/svg+xml",
        Some("webp") => "image/webp",
        Some("woff2") => "font/woff2",
        _ => "application/octet-stream",
    }
}

fn read_json(path: &Path) -> Value {
    let source = fs::read_to_string(path).unwrap_or_else(|error| {
        panic!("convey data asset {} is readable: {error}", path.display())
    });
    serde_json::from_str(&source).unwrap_or_else(|error| {
        panic!(
            "convey data asset {} is valid JSON: {error}",
            path.display()
        )
    })
}

fn read_object(path: &Path) -> serde_json::Map<String, Value> {
    match read_json(path) {
        Value::Object(value) => value,
        _ => panic!("convey data asset {} is a JSON object", path.display()),
    }
}

fn prefixed_object(
    values: &serde_json::Map<String, Value>,
    prefix: &str,
) -> serde_json::Map<String, Value> {
    values
        .iter()
        .filter(|(name, _)| name.starts_with(prefix))
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect()
}

fn required_string<'a>(
    path: &Path,
    values: &'a serde_json::Map<String, Value>,
    name: &str,
) -> &'a str {
    values
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("convey data asset {} has string {name}", path.display()))
}

fn main() {
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let root = manifest
        .join("../../..")
        .canonicalize()
        .expect("repository root resolves");
    let static_root = manifest.join("assets/static");
    let speakers_root = manifest.join("assets/speakers");
    let speakers_copy = speakers_root.join("copy.json");
    let network_copy = manifest.join("assets/network_copy.json");
    let outcomes = manifest.join("assets/outcomes.json");
    let pairing_config = manifest.join("assets/pairing_config.json");
    let entities_workspace = manifest.join("assets/entities/workspace.html");
    let body_workspace = manifest.join("assets/body/workspace.html");
    let favicon = root.join("favicon.ico");
    let workspace = speakers_root.join("workspace.html");
    let speakers_static = speakers_root.join("who_is_this.js");
    let thinking_root = manifest.join("assets/thinking");
    let thinking_workspace = thinking_root.join("workspace.html");
    let mut thinking_static_files = Vec::new();
    collect_files(&thinking_root, &mut thinking_static_files);
    thinking_static_files.retain(|path| {
        path.file_name()
            .is_some_and(|name| name != "workspace.html")
    });
    let network_root = manifest.join("assets/network");
    let network_workspace = network_root.join("workspace.html");
    let network_static = network_root.join("network.js");
    let agents_workspace = manifest.join("assets/agents/workspace.html");
    let more_workspace = manifest.join("assets/more/workspace.html");
    let more_static = manifest.join("assets/more/more.js");

    let mut files = Vec::new();
    collect_files(&static_root, &mut files);
    for path in [
        &favicon,
        &workspace,
        &speakers_static,
        &speakers_copy,
        &network_copy,
        &outcomes,
        &pairing_config,
        &body_workspace,
        &entities_workspace,
        &thinking_workspace,
        &network_workspace,
        &network_static,
        &agents_workspace,
        &more_workspace,
        &more_static,
    ] {
        println!("cargo:rerun-if-changed={}", path.display());
    }

    // The icon catalogues and the source-link script are built into what uses
    // them and never fetched by a page. The manual test pages are served to
    // development builds only.
    let release = env::var("PROFILE").as_deref() == Ok("release");
    let mut assets = Vec::new();
    for path in files {
        let relative = path
            .strip_prefix(&static_root)
            .expect("static asset is under static root")
            .to_string_lossy()
            .replace('\\', "/");
        if (relative.starts_with("icons/") && relative.ends_with(".json"))
            || relative == "source_link_back.js"
            || relative == "vendor/VENDOR.md"
        {
            continue;
        }
        if release && relative.starts_with("tests/") {
            continue;
        }
        assets.push((format!("/static/{relative}"), path));
    }
    assets.push(("/favicon.ico".to_owned(), favicon));
    assets.push(("/app/body/workspace".to_owned(), body_workspace));
    assets.push(("/app/entities/workspace".to_owned(), entities_workspace));
    assets.push(("/app/speakers/workspace".to_owned(), workspace));
    assets.push((
        "/app/speakers/static/who_is_this.js".to_owned(),
        speakers_static,
    ));
    assets.push(("/app/thinking/workspace".to_owned(), thinking_workspace));
    // The two SVGs are OpenAI's sign-in button assets, used as OpenAI's guidelines direct.
    for path in thinking_static_files {
        let name = path
            .file_name()
            .expect("thinking static file has a name")
            .to_string_lossy();
        assets.push((format!("/app/thinking/static/{name}"), path));
    }
    assets.push(("/app/network/workspace".to_owned(), network_workspace));
    assets.push(("/app/network/static/network.js".to_owned(), network_static));
    assets.push(("/app/agents/workspace".to_owned(), agents_workspace));
    assets.push(("/app/more/workspace".to_owned(), more_workspace));
    assets.push(("/app/more/static/more.js".to_owned(), more_static));

    let output = PathBuf::from(env::var_os("OUT_DIR").expect("output dir"));
    let shell_page_path = static_root.join("shell.html");
    let shell_tags = shell_bundles(&static_root, &shell_page_path, &output, &mut assets);
    assets.sort_by(|left, right| left.0.cmp(&right.0));

    let speakers = read_object(&speakers_copy);
    let speaker_copy = serde_json::to_string(&Value::Object(prefixed_object(&speakers, "SPK_")))
        .expect("speaker copy serializes");
    let network_copy_json = serde_json::to_string(&Value::Object(read_object(&network_copy)))
        .expect("network copy serializes");
    let spl_outcome_strings_json = serde_json::to_string(&Value::Object(read_object(&outcomes)))
        .expect("SPL outcome copy serializes");
    let home_address_strings_json =
        serde_json::to_string(&Value::Object(read_object(&pairing_config)))
            .expect("home address copy serializes");
    let not_in_new_voices = serde_json::to_string(required_string(
        &speakers_copy,
        &speakers,
        "TR_NOT_IN_NEW_VOICES",
    ))
    .expect("copy string serializes");

    let mut generated = String::from("pub(super) static GENERATED_ASSETS: &[EmbeddedAsset] = &[\n");
    for (route, path) in &assets {
        generated.push_str(&format!(
            "    EmbeddedAsset {{ path: {route:?}, content_type: {:?}, bytes: include_bytes!({:?}) }},\n",
            content_type(path),
            path,
        ));
    }
    generated.push_str("];\n");
    generated.push_str(&inventory(&assets, &shell_tags, &output));
    generated.push_str("pub(super) static SHELL_BUNDLE_SEGMENTS: &[(&str, &str)] = &[\n");
    for tag in &shell_tags.styles {
        generated.push_str(&format!("    ({SHELL_STYLE_BUNDLE:?}, {:?}),\n", tag.path));
    }
    for tag in &shell_tags.scripts {
        generated.push_str(&format!("    ({SHELL_SCRIPT_BUNDLE:?}, {:?}),\n", tag.path));
    }
    generated.push_str("];\n");

    generated.push_str(&format!(
        "pub(super) const SPEAKER_COPY_JSON: &str = {speaker_copy:?};\n"
    ));
    generated.push_str(&format!(
        "pub(super) const NETWORK_COPY_JSON: &str = {network_copy_json:?};\n"
    ));
    generated.push_str(&format!(
        "pub(super) const SPL_OUTCOME_STRINGS_JSON: &str = {spl_outcome_strings_json:?};\n"
    ));
    generated.push_str(&format!(
        "pub(super) const HOME_ADDRESS_STRINGS_JSON: &str = {home_address_strings_json:?};\n"
    ));
    generated.push_str(&format!(
        "pub(super) const NOT_IN_NEW_VOICES_COPY: &str = {not_in_new_voices};\n"
    ));

    fs::write(output.join("embedded_assets.rs"), generated)
        .expect("generated embedded asset source writes");
}

/// Concatenate the shell page's stylesheets and scripts into one file each
/// (and the script bundle's source map), add them to the shell's assets, and
/// return the page's tags so the served page can name the bundles instead.
fn shell_bundles(
    static_root: &Path,
    page_path: &Path,
    output: &Path,
    assets: &mut Vec<(String, PathBuf)>,
) -> ShellPageTags {
    let page = fs::read_to_string(page_path).expect("shell page is readable");
    let tags = shell_page_tags(&page).unwrap_or_else(|error| panic!("{error}"));
    let read = |tag: &BundledTag| -> (BundledTag, String) {
        let relative = tag
            .path
            .strip_prefix("/static/")
            .expect("bundled files are shell files");
        let file = static_root.join(relative);
        let source = fs::read_to_string(&file)
            .unwrap_or_else(|error| panic!("{} is readable: {error}", file.display()));
        (tag.clone(), source)
    };
    let styles: Vec<_> = tags.styles.iter().map(read).collect();
    let scripts: Vec<_> = tags.scripts.iter().map(read).collect();

    let registry = serde_json::to_string(&registry::shell_registry()).expect("registry serializes");
    let registry = format!("globalThis.SOL_SHELL_REGISTRY = {registry};\n");

    let map_name = SHELL_SCRIPT_MAP.rsplit('/').next().expect("map file name");
    let script_name = SHELL_SCRIPT_BUNDLE
        .rsplit('/')
        .next()
        .expect("script file name");
    let style_name = SHELL_STYLE_BUNDLE
        .rsplit('/')
        .next()
        .expect("style file name");
    let script = script_bundle(&scripts, &registry, map_name)
        .unwrap_or_else(|error| panic!("the shell's scripts can't be bundled: {error}"));
    let style = style_bundle(&styles)
        .unwrap_or_else(|error| panic!("the shell's stylesheets can't be bundled: {error}"));
    let map = line_source_map(&script, &scripts, script_name);

    let directory = output.join("shell-bundle");
    fs::create_dir_all(&directory).expect("bundle directory");
    for (route, name, text) in [
        (SHELL_SCRIPT_BUNDLE, script_name, &script.text),
        (SHELL_STYLE_BUNDLE, style_name, &style.text),
        (SHELL_SCRIPT_MAP, map_name, &map),
    ] {
        let path = directory.join(name);
        // Rewrite only on change, so an unchanged bundle keeps its timestamp.
        if fs::read_to_string(&path).ok().as_deref() != Some(text.as_str()) {
            fs::write(&path, text).expect("bundle writes");
        }
        assert!(
            !assets.iter().any(|(existing, _)| existing == route),
            "{route} is a generated bundle; no file may be served at it"
        );
        assets.push((route.to_owned(), path));
    }
    tags
}

/// The edits that make the served shell page load the bundles: the first
/// stylesheet and first script become the bundles, the rest go.
fn bundle_edits(
    page: &str,
    tags: &ShellPageTags,
    css: &str,
    js: &str,
) -> Vec<(usize, usize, String)> {
    let indent = |tag: &BundledTag| -> String {
        page[tag.start..tag.end]
            .chars()
            .take_while(|c| *c == ' ' || *c == '\t')
            .collect()
    };
    let newline = |tag: &BundledTag| {
        if page[..tag.end].ends_with('\n') {
            "\n"
        } else {
            ""
        }
    };
    let mut edits = Vec::new();
    for (index, tag) in tags.styles.iter().enumerate() {
        let text = if index == 0 {
            format!(
                "{}<link rel=\"stylesheet\" href=\"{css}\">{}",
                indent(tag),
                newline(tag)
            )
        } else {
            String::new()
        };
        edits.push((tag.start, tag.end, text));
    }
    for (index, tag) in tags.scripts.iter().enumerate() {
        let text = if index == 0 {
            format!(
                "{}<script src=\"{js}\" defer></script>{}",
                indent(tag),
                newline(tag)
            )
        } else {
            String::new()
        };
        edits.push((tag.start, tag.end, text));
    }
    edits.sort();
    edits
}

/// Whether a response of this content type is one the server gzips: the same
/// allowlist as the server's compression layer.
fn compressible(content_type: &str) -> bool {
    let essence = content_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    match essence.as_str() {
        "text/event-stream" => false,
        "application/json"
        | "application/javascript"
        | "application/manifest+json"
        | "image/svg+xml" => true,
        other => other.starts_with("text/") || other.ends_with("+json"),
    }
}

/// Below this the server sends a response as it is (its compression layer's
/// threshold).
const GZIP_MIN_BYTES: usize = 1024;

fn gzip(bytes: &[u8]) -> Vec<u8> {
    use std::io::Write;
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
    encoder.write_all(bytes).expect("gzip writes");
    encoder.finish().expect("gzip finishes")
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    File,
    Template,
    Dynamic,
}

struct Record {
    route: String,
    content_type: String,
    file: PathBuf,
    kind: Kind,
    bytes: Vec<u8>,
    tree: Option<String>,
    /// (start, end, target path): a reference to rewrite to the target's hashed path.
    edits: Vec<(usize, usize, String)>,
}

fn is_text(content_type: &str) -> bool {
    ["text/html", "text/javascript", "text/css"]
        .iter()
        .any(|kind| content_type.starts_with(kind))
}

/// Every crate that serves dashboard assets hands its list over through its
/// `links` metadata; this shell's own assets are the embedded table above.
fn sibling_records() -> Vec<(Kind, String, String, PathBuf)> {
    let mut lists: Vec<(String, String)> = env::vars()
        .filter(|(name, _)| {
            name.starts_with("DEP_SOLSTONE_CONVEY_ASSETS_") && name.ends_with("_RECORDS")
        })
        .collect();
    lists.sort();
    let mut records = Vec::new();
    for (_, path) in lists {
        println!("cargo:rerun-if-changed={path}");
        let list = fs::read_to_string(&path).expect("sibling asset records readable");
        for line in list.lines().filter(|line| !line.is_empty()) {
            let fields: Vec<_> = line.split('\t').collect();
            let [kind, route, content_type, file] = fields[..] else {
                panic!("asset record line {line:?} has four fields");
            };
            let kind = match kind {
                "asset" => Kind::File,
                "template" => Kind::Template,
                "dynamic" => Kind::Dynamic,
                _ => panic!("asset record {route} has unknown kind {kind}"),
            };
            records.push((
                kind,
                route.to_owned(),
                content_type.to_owned(),
                PathBuf::from(file),
            ));
        }
    }
    records
}

/// What each view needs at the start of a page, read from its fragment. Views
/// come from the registry; each fragment from the asset records, by the address
/// the registry gives it, whichever crate carries the file.
fn entry_table_json(
    records: &[Record],
    index: &BTreeMap<String, usize>,
    hashed: &dyn Fn(&str) -> Option<String>,
) -> String {
    let payload = registry::shell_registry();
    let mut views = serde_json::Map::new();
    let mut date_code: BTreeSet<String> = BTreeSet::new();
    for app in &payload.apps {
        let record = &records[*index.get(&app.workspace_url).unwrap_or_else(|| {
            panic!("view {} has no fragment at {}", app.name, app.workspace_url)
        })];
        let text = std::str::from_utf8(&record.bytes).expect("a view fragment is UTF-8");
        let facts =
            fragment_facts(&app.workspace_url, text).unwrap_or_else(|error| panic!("{error}"));
        if app.date_nav.is_some() {
            let shared = |path: &String| {
                path.starts_with("/static/") && !path.starts_with("/static/vendor/")
            };
            date_code.extend(
                facts
                    .styles
                    .iter()
                    .filter(|p| shared(p))
                    .filter_map(|p| hashed(p)),
            );
            date_code.extend(
                facts
                    .scripts
                    .iter()
                    .map(|s| &s.src)
                    .filter(|p| shared(p))
                    .filter_map(|p| hashed(p)),
            );
        }
        views.insert(
            app.name.to_owned(),
            entry_row(&app.workspace_url, &facts, hashed),
        );
    }
    entry_table_text(&serde_json::json!({ "views": views, "dateViewCode": date_code }))
}

/// Hash every asset tree in dependency order, rewrite each reference to a tree
/// file to its hashed path, and emit the inventory the shell serves from.
fn inventory(own: &[(String, PathBuf)], shell_tags: &ShellPageTags, output: &Path) -> String {
    let mut sources: Vec<(Kind, String, String, PathBuf)> = own
        .iter()
        .map(|(route, path)| {
            (
                Kind::File,
                route.clone(),
                content_type(path).to_owned(),
                path.clone(),
            )
        })
        .collect();
    sources.extend(sibling_records());
    sources.sort_by(|a, b| a.1.cmp(&b.1));
    for pair in sources.windows(2) {
        assert!(pair[0].1 != pair[1].1, "two crates serve {}", pair[0].1);
    }
    let mut records: Vec<Record> = sources
        .into_iter()
        .map(|(kind, route, content_type, file)| {
            println!("cargo:rerun-if-changed={}", file.display());
            let bytes = fs::read(&file)
                .unwrap_or_else(|error| panic!("asset {} readable: {error}", file.display()));
            let tree = tree_root(&route);
            assert!(
                tree.is_none() || kind == Kind::File,
                "{route} is filled in at response time, so it can't be part of a hashed tree"
            );
            Record {
                route,
                content_type,
                file,
                kind,
                bytes,
                tree,
                edits: Vec::new(),
            }
        })
        .collect();
    let index: BTreeMap<String, usize> = records
        .iter()
        .enumerate()
        .map(|(position, record)| (record.route.clone(), position))
        .collect();

    // Find each reference to a tree file, and the tree-to-tree edges they make.
    let mut depends: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for record in &records {
        if let Some(tree) = &record.tree {
            depends.entry(tree.clone()).or_default();
        }
    }
    for position in 0..records.len() {
        let record = &records[position];
        if record.kind == Kind::Dynamic || !is_text(&record.content_type) {
            continue;
        }
        let source = std::str::from_utf8(&record.bytes)
            .unwrap_or_else(|_| panic!("text asset {} is UTF-8", record.route));
        let mut edits = Vec::new();
        for (start, end) in reference_spans(source) {
            let reference = &source[start..end];
            let Some(target) = resolve_reference(&record.route, reference) else {
                continue;
            };
            let Some(target_tree) = index
                .get(&target)
                .and_then(|found| records[*found].tree.clone())
            else {
                continue;
            };
            let path_end = start + reference.find(['?', '#']).unwrap_or(reference.len());
            let relative = !reference.starts_with('/');
            match &record.tree {
                // Relative inside one tree already resolves under the hashed path.
                // Absolute inside one tree can't name its own hash, so it stays
                // on the stable path, which keeps being served.
                Some(tree) if *tree == target_tree => {}
                Some(tree) => {
                    assert!(
                        !relative,
                        "{} refers to {reference} relatively, outside its tree {tree}; \
                         under a hashed path that would not resolve. Use an absolute path.",
                        record.route
                    );
                    depends.entry(tree.clone()).or_default().insert(target_tree);
                    edits.push((start, path_end, target));
                }
                None => edits.push((start, path_end, target)),
            }
        }
        records[position].edits = edits;
    }

    // Hash trees so that every tree a tree refers to is hashed before it.
    let mut order = Vec::new();
    let mut state: BTreeMap<String, u8> = BTreeMap::new();
    fn visit(
        tree: &str,
        depends: &BTreeMap<String, BTreeSet<String>>,
        state: &mut BTreeMap<String, u8>,
        stack: &mut Vec<String>,
        order: &mut Vec<String>,
    ) {
        match state.get(tree) {
            Some(2) => return,
            Some(1) => {
                let from = stack
                    .iter()
                    .position(|item| item == tree)
                    .expect("on stack");
                let mut cycle = stack[from..].to_vec();
                cycle.push(tree.to_owned());
                panic!(
                    "dashboard asset trees refer to each other in a cycle, so none can be \
                     hashed first: {}",
                    cycle.join(" -> ")
                );
            }
            _ => {}
        }
        state.insert(tree.to_owned(), 1);
        stack.push(tree.to_owned());
        for next in &depends[tree] {
            visit(next, depends, state, stack, order);
        }
        stack.pop();
        state.insert(tree.to_owned(), 2);
        order.push(tree.to_owned());
    }
    for tree in depends.keys() {
        visit(tree, &depends, &mut state, &mut Vec::new(), &mut order);
    }

    let mut hashes: BTreeMap<String, String> = BTreeMap::new();
    // Set once every tree is hashed; only the shell page reads it, and the
    // shell page is in no tree.
    let entry_table: std::cell::OnceCell<String> = std::cell::OnceCell::new();
    let hashed_of = |target: &str, hashes: &BTreeMap<String, String>| -> String {
        let tree = records[index[target]]
            .tree
            .as_deref()
            .expect("target is a tree file");
        hashed_path(tree, &hashes[tree], target)
    };
    // Every edit to a record's source, with each reference's hashed path. The
    // shell page also loads the bundles in place of the files they hold.
    let edits_of =
        |record: &Record, hashes: &BTreeMap<String, String>| -> Vec<(usize, usize, String)> {
            let mut edits: Vec<(usize, usize, String)> = record
                .edits
                .iter()
                .map(|(start, end, target)| (*start, *end, hashed_of(target, hashes)))
                .collect();
            if record.route == "/static/shell.html" {
                let page = std::str::from_utf8(&record.bytes).expect("shell page is UTF-8");
                let css = hashed_of(SHELL_STYLE_BUNDLE, hashes);
                let js = hashed_of(SHELL_SCRIPT_BUNDLE, hashes);
                let replaced = bundle_edits(page, shell_tags, &css, &js);
                edits.retain(|(start, end, _)| {
                    !replaced
                        .iter()
                        .any(|(from, to, _)| start < to && from < end)
                });
                edits.extend(replaced);
                if let Some(table) = entry_table.get() {
                    let at = page
                        .find(ENTRY_TABLE_MARKER)
                        .expect("the shell page has the entry table's marker");
                    edits.push((at, at + ENTRY_TABLE_MARKER.len(), table.clone()));
                }
                edits.sort();
            }
            edits
        };
    let served_of = |record: &Record, hashes: &BTreeMap<String, String>| -> Vec<u8> {
        let edits = edits_of(record, hashes);
        let borrowed: Vec<(usize, usize, &str)> =
            edits.iter().map(|(s, e, t)| (*s, *e, t.as_str())).collect();
        apply_edits(&record.bytes, &borrowed)
    };
    for tree in &order {
        let served: Vec<(String, Vec<u8>)> = records
            .iter()
            .filter(|record| record.tree.as_deref() == Some(tree.as_str()))
            .map(|record| (record.route.clone(), served_of(record, &hashes)))
            .collect();
        let hash = tree_hash(
            tree,
            served
                .iter()
                .map(|(route, bytes)| (route.as_str(), bytes.as_slice())),
        );
        hashes.insert(tree.clone(), hash);
    }
    entry_table
        .set(entry_table_json(&records, &index, &|target: &str| {
            index
                .get(target)
                .filter(|found| records[**found].tree.is_some())
                .map(|_| hashed_of(target, &hashes))
        }))
        .expect("the entry table is set once");

    let gzip_directory = output.join("gzip");
    fs::create_dir_all(&gzip_directory).expect("gzip directory");
    let mut generated = String::from("pub static INVENTORY: &[AssetRecord] = &[\n");
    let mut hashed_index = Vec::new();
    for (position, record) in records.iter().enumerate() {
        let raw = format!("{:x}", Sha256::digest(&record.bytes));
        let hashed = record
            .tree
            .as_ref()
            .map(|tree| hashed_path(tree, &hashes[tree], &record.route));
        if let Some(hashed) = &hashed {
            hashed_index.push((hashed.clone(), position));
        }
        let (etag, gzip_bytes) = match record.kind {
            Kind::File => {
                let served = served_of(record, &hashes);
                let tag = format!("\"{}\"", &format!("{:x}", Sha256::digest(&served))[..32]);
                // Compressed once here, so no request spends a worker on it.
                let packed = (compressible(&record.content_type) && served.len() >= GZIP_MIN_BYTES)
                    .then(|| {
                        let file = gzip_directory.join(format!("{position}.gz"));
                        let packed = gzip(&served);
                        if fs::read(&file).ok().as_deref() != Some(packed.as_slice()) {
                            fs::write(&file, &packed).expect("gzip copy writes");
                        }
                        file
                    });
                (tag, packed)
            }
            Kind::Template | Kind::Dynamic => (String::new(), None),
        };
        let gzip_field = match &gzip_bytes {
            Some(file) => format!("include_bytes!({file:?})"),
            None => "&[]".to_owned(),
        };
        let edits: Vec<String> = edits_of(record, &hashes)
            .iter()
            .map(|(start, end, text)| format!("({start}, {end}, {text:?})"))
            .collect();
        let kind = match record.kind {
            Kind::File => "AssetKind::File",
            Kind::Template => "AssetKind::Template",
            Kind::Dynamic => "AssetKind::Dynamic",
        };
        generated.push_str(&format!(
            "    AssetRecord {{ path: {:?}, content_type: {:?}, byte_len: {}, sha256: {raw:?}, kind: {kind}, tree: {:?}, hashed: {:?}, etag: {etag:?}, edits: &[{}], gzip: {gzip_field}, #[cfg(test)] bytes: include_bytes!({:?}) }},\n",
            record.route,
            record.content_type,
            record.bytes.len(),
            record.tree,
            hashed,
            edits.join(", "),
            record.file,
        ));
    }
    generated.push_str("];\n");
    hashed_index.sort();
    generated.push_str("pub static HASHED: &[(&str, usize)] = &[\n");
    for (hashed, position) in hashed_index {
        generated.push_str(&format!("    ({hashed:?}, {position}),\n"));
    }
    generated.push_str("];\n");
    generated.push_str("pub static TREES: &[(&str, &str)] = &[\n");
    for (tree, hash) in &hashes {
        generated.push_str(&format!("    ({tree:?}, {hash:?}),\n"));
    }
    generated.push_str("];\n");
    generated
}
