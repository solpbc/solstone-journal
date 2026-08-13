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

fn assignment_end(source: &str, start: usize) -> usize {
    let bytes = source.as_bytes();
    let mut depth = 0usize;
    let mut quote = None;
    let mut escaped = false;
    for (offset, byte) in bytes[start..].iter().copied().enumerate() {
        if let Some(delimiter) = quote {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == delimiter {
                quote = None;
            }
            continue;
        }
        match byte {
            b'\'' | b'"' => quote = Some(byte),
            b'(' | b'[' => depth += 1,
            b')' | b']' => depth = depth.saturating_sub(1),
            b'\n' if depth == 0 => return start + offset,
            _ => {}
        }
    }
    source.len()
}

fn python_strings(expression: &str, constant_name: &str) -> Vec<String> {
    let bytes = expression.as_bytes();
    let mut strings = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'\'' && bytes[index] != b'"' {
            index += 1;
            continue;
        }
        let delimiter = bytes[index];
        index += 1;
        let mut value = Vec::new();
        while index < bytes.len() && bytes[index] != delimiter {
            if bytes[index] == b'\\' {
                assert!(
                    index + 1 < bytes.len(),
                    "speaker copy constant {constant_name} ends with a backslash; \
                     this parser does not support general Python escape decoding"
                );
                let escaped = bytes[index + 1];
                assert!(
                    matches!(escaped, b'\\' | b'\'' | b'"'),
                    "speaker copy constant {constant_name} uses unsupported Python escape \\{}; \
                     this parser does not support general Python escape decoding; \
                     use different quoting or extend the parser",
                    escaped as char
                );
                index += 1;
            }
            value.push(bytes[index]);
            index += 1;
        }
        assert!(index < bytes.len(), "speaker copy string is terminated");
        index += 1;
        strings.push(String::from_utf8(value).expect("speaker copy is UTF-8"));
    }
    strings
}

fn python_constants(source: &str) -> serde_json::Map<String, Value> {
    let mut constants = serde_json::Map::new();
    for line in source.lines() {
        let Some((name, _)) = line.split_once(" =") else {
            continue;
        };
        if !name.starts_with("SPK_")
            || !name
                .chars()
                .all(|character| character == '_' || character.is_ascii_uppercase())
        {
            continue;
        }
        let start = source.find(line).expect("source line occurs") + name.len() + 3;
        let end = assignment_end(source, start);
        let expression = &source[start..end];
        let strings = python_strings(expression, name);
        assert!(
            !strings.is_empty(),
            "speaker copy constant is a string or list"
        );
        let value = if expression.trim_start().starts_with('[') {
            Value::Array(strings.into_iter().map(Value::String).collect())
        } else {
            Value::String(strings.concat())
        };
        constants.insert(name.to_owned(), value);
    }
    constants
}

fn python_constant(source: &str, name: &str) -> String {
    let assignment = format!("{name} =");
    let start = source.find(&assignment).expect("copy constant exists") + assignment.len();
    let end = assignment_end(source, start);
    python_strings(&source[start..end], name).concat()
}

fn main() {
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let root = manifest
        .join("../../..")
        .canonicalize()
        .expect("repository root resolves");
    let static_root = root.join("solstone/convey/static");
    let speakers_root = manifest.join("assets/speakers");
    let speakers_copy = speakers_root.join("copy.py");
    let favicon = root.join("favicon.ico");
    let workspace = speakers_root.join("workspace.html");
    let speakers_static = speakers_root.join("who_is_this.js");
    let devices_workspace = manifest.join("assets/devices/workspace.html");

    let mut files = Vec::new();
    collect_files(&static_root, &mut files);
    for path in [
        &favicon,
        &workspace,
        &speakers_static,
        &speakers_copy,
        &devices_workspace,
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
    assets.push(("/app/devices/workspace".to_owned(), devices_workspace));
    assets.push(("/app/speakers/workspace".to_owned(), workspace));
    assets.push((
        "/app/speakers/static/who_is_this.js".to_owned(),
        speakers_static,
    ));
    assets.sort_by(|left, right| left.0.cmp(&right.0));

    let copy_source = fs::read_to_string(&speakers_copy).expect("speaker copy source is readable");
    let speaker_copy = serde_json::to_string(&Value::Object(python_constants(&copy_source)))
        .expect("speaker copy serializes");
    let not_in_new_voices =
        serde_json::to_string(&python_constant(&copy_source, "TR_NOT_IN_NEW_VOICES"))
            .expect("speaker copy string serializes");

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
        "pub(super) const NOT_IN_NEW_VOICES_COPY: &str = {not_in_new_voices};\n"
    ));

    let output = PathBuf::from(env::var_os("OUT_DIR").expect("output dir"));
    fs::write(output.join("embedded_assets.rs"), generated)
        .expect("generated embedded asset source writes");
}
