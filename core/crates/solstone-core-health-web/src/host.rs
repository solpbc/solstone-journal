// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

pub fn hostname() -> String {
    let value = std::env::var("HOSTNAME")
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| {
            std::fs::read_to_string("/etc/hostname").unwrap_or_else(|_| "unknown".to_owned())
        });
    let value = value.trim().to_ascii_lowercase();
    let value = value.split('.').next().unwrap_or(&value);
    value
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '-'
            }
        })
        .collect()
}
