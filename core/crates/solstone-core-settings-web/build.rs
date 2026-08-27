// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

fn read_json(path: &Path) -> Value {
    let source = fs::read_to_string(path).unwrap_or_else(|error| {
        panic!(
            "settings data asset {} is readable: {error}",
            path.display()
        )
    });
    serde_json::from_str(&source).unwrap_or_else(|error| {
        panic!(
            "settings data asset {} is valid JSON: {error}",
            path.display()
        )
    })
}

fn read_object(path: &Path) -> serde_json::Map<String, Value> {
    match read_json(path) {
        Value::Object(value) => value,
        _ => panic!("settings data asset {} is a JSON object", path.display()),
    }
}

fn write_constants(path: &Path, constants: &serde_json::Map<String, Value>) {
    let json = serde_json::to_string(constants).expect("copy constants serialize");
    fs::write(
        path,
        format!("pub const COPY_JSON: &str = r##\"{json}\"##;\n"),
    )
    .expect("generated copy constants write");
}

fn write_value(path: &Path, value: &Value) {
    let json = serde_json::to_string(value).expect("data serializes");
    fs::write(path, format!("pub const JSON: &str = r##\"{json}\"##;\n"))
        .expect("generated data write");
}

fn main() {
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest directory"));
    let modules = [
        (manifest.join("assets/copy.json"), "settings_copy.rs"),
        (manifest.join("assets/install_copy.json"), "install_copy.rs"),
        (manifest.join("assets/backup_copy.json"), "backup_copy.rs"),
    ];
    let output = PathBuf::from(env::var_os("OUT_DIR").expect("output directory"));
    let mut source_names = fs::read_dir(manifest.join("src"))
        .expect("settings source directory")
        .filter_map(Result::ok)
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| name.ends_with(".rs"))
        .collect::<Vec<_>>();
    source_names.sort();
    fs::write(
        output.join("settings_sources.rs"),
        format!("pub const SOURCES: &[&str] = &{:?};\n", source_names),
    )
    .expect("settings source manifest");
    println!("cargo:rerun-if-changed={}", manifest.join("src").display());
    for (path, generated) in modules {
        println!("cargo:rerun-if-changed={}", path.display());
        write_constants(&output.join(generated), &read_object(&path));
    }
    let activities = manifest.join("assets/default_activities.json");
    println!("cargo:rerun-if-changed={}", activities.display());
    let activity_defaults = read_json(&activities);
    if !activity_defaults.is_array() {
        panic!(
            "settings data asset {} is a JSON array",
            activities.display()
        );
    }
    write_value(&output.join("default_activities.rs"), &activity_defaults);
    for path in [
        manifest.join("assets/workspace.html"),
        manifest.join("assets/settings.js"),
    ] {
        println!("cargo:rerun-if-changed={}", path.display());
    }
}
