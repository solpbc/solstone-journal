// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;

use axum::response::Response;
use serde_json::{Map, Value, json};

use crate::http::json_response;

mod settings_copy {
    include!(concat!(env!("OUT_DIR"), "/settings_copy.rs"));
}
mod install_copy {
    include!(concat!(env!("OUT_DIR"), "/install_copy.rs"));
}
mod sol_voice_copy {
    include!(concat!(env!("OUT_DIR"), "/sol_voice_copy.rs"));
}

pub async fn get(journal_root: std::path::PathBuf) -> Response {
    json_response(payload(&journal_root))
}

pub fn payload(_journal_root: &Path) -> Value {
    json!({
        "settings_copy": constants(settings_copy::COPY_JSON),
        "install_copy": constants(install_copy::COPY_JSON),
        "sol_voice_copy": constants(sol_voice_copy::COPY_JSON),
    })
}

pub fn sol_voice_constants() -> Map<String, Value> {
    constants(sol_voice_copy::COPY_JSON)
}

fn constants(source: &str) -> Map<String, Value> {
    serde_json::from_str(source).expect("generated Python copy constants")
}
