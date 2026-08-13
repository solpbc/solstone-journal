// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Minimal decoded query parsing shared by native Body read routes.

use std::collections::BTreeMap;

/// Decodes form-style query keys and values from an Axum raw query string.
pub(crate) fn decoded_query_params(raw: &str) -> BTreeMap<String, String> {
    raw.split('&')
        .filter_map(|item| item.split_once('='))
        .map(|(key, value)| (percent_decode(key), percent_decode(value)))
        .collect()
}

fn percent_decode(value: &str) -> String {
    let mut output = Vec::new();
    let bytes = value.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && let Some(hex) = std::str::from_utf8(&bytes[index + 1..index + 3])
                .ok()
                .and_then(|hex| u8::from_str_radix(hex, 16).ok())
        {
            output.push(hex);
            index += 3;
            continue;
        }
        output.push(if bytes[index] == b'+' {
            b' '
        } else {
            bytes[index]
        });
        index += 1;
    }
    String::from_utf8_lossy(&output).into_owned()
}
