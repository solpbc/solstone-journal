// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

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
        Some("json") => "application/json",
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
    ] {
        println!("cargo:rerun-if-changed={}", path.display());
    }

    let mut assets = Vec::new();
    for path in files {
        let relative = path
            .strip_prefix(&static_root)
            .expect("static asset is under static root")
            .to_string_lossy()
            .replace('\\', "/");
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
    for path in thinking_static_files {
        let name = path
            .file_name()
            .expect("thinking static file has a name")
            .to_string_lossy();
        assets.push((format!("/app/thinking/static/{name}"), path));
    }
    assets.push(("/app/network/workspace".to_owned(), network_workspace));
    assets.push(("/app/network/static/network.js".to_owned(), network_static));
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
    for (route, path) in assets {
        generated.push_str(&format!(
            "    EmbeddedAsset {{ path: {route:?}, content_type: {:?}, bytes: include_bytes!({:?}) }},\n",
            content_type(&path),
            path,
        ));
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

    let output = PathBuf::from(env::var_os("OUT_DIR").expect("output dir"));
    fs::write(output.join("embedded_assets.rs"), generated)
        .expect("generated embedded asset source writes");
}
