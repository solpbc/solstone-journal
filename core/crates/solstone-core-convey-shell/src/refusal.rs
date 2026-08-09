// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct AppNotConverted<'a> {
    pub error: &'static str,
    pub reason_code: &'static str,
    pub detail: String,
    pub app: &'a str,
}

impl<'a> AppNotConverted<'a> {
    pub fn new(app: &'a str) -> Self {
        Self {
            error: "This app isn't available yet.",
            reason_code: "app_not_converted",
            detail: format!("The {app} app has not been ported to the native shell."),
            app,
        }
    }
}
