// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use axum::response::Response;
use serde_json::json;
use solstone_core_convey_http::window_catalog::resolve_table;

use crate::http::json_response;

pub async fn get(locale: &str) -> Response {
    let mut payload = json!({
        "settings_copy": {
            "LOCAL_ENDPOINT_DISCLOSURE": "journal.window.settings.state.local.endpoint.disclosure",
        },
        "install_copy": {
            "STT_LOCAL_REQUIREMENTS_TEMPLATE": "journal.window.settings.state.stt.local.requirements.template",
            "STT_LOCAL_UNSUPPORTED": "journal.window.settings.state.stt.local.unsupported",
            "STT_DETECTED_MEMORY_TEMPLATE": "journal.window.settings.state.stt.detected.memory.template",
            "STT_DETECTED_MEMORY_UNKNOWN": "journal.window.settings.state.stt.detected.memory.unknown",
            "STT_NO_LOCAL_STT_RECOVERY": "journal.window.settings.state.stt.no.local.stt.recovery",
            "STT_EXPLICIT_LOCAL_LOW_TEMPLATE": "journal.window.settings.state.stt.explicit.local.low.template",
        },
    });
    resolve_table(locale, &mut payload);
    json_response(payload)
}
