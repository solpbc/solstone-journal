// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use axum::body::Bytes;
use serde_json::Value;

#[derive(Debug)]
pub enum JsonBody {
    Missing,
    Invalid,
    Value(Value),
}

pub fn json_body(body: Bytes) -> JsonBody {
    if body.is_empty() {
        return JsonBody::Missing;
    }
    match serde_json::from_slice(&body) {
        Ok(value) => JsonBody::Value(value),
        Err(_) => JsonBody::Invalid,
    }
}
