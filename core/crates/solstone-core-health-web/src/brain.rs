// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::{Map, Value, json};
use solstone_core_callosum::{CallosumEnvelope, CallosumOneShotSender};
use solstone_core_convey_http::window_catalog::text;
use std::time::Duration;

pub fn snapshot(root: &std::path::Path, locale: &str) -> Value {
    match solstone_core_thinking::read_config(root) {
        Ok(config) => solstone_core_thinking::brain::presentation_in(root, &config, false, locale)
            ["brain"]
            .clone(),
        Err(_) => fallback(locale),
    }
}
pub fn refresh(root: &std::path::Path) -> bool {
    refresh_with(root, |envelope| {
        let Ok(mut line) = serde_json::to_string(envelope) else {
            return false;
        };
        line.push('\n');
        CallosumOneShotSender::new(root.join("health/callosum.sock"), Duration::from_secs(1))
            .send_line(&line)
            .is_ok()
    })
}

pub fn refresh_with<F>(_root: &std::path::Path, mut transport: F) -> bool
where
    F: FnMut(&CallosumEnvelope) -> bool,
{
    let mut extra = Map::new();
    extra.insert(
        "cmd".to_owned(),
        serde_json::Value::from(solstone_core_system::partition::canonical_journal_command(
            ["brain", "refresh"],
        )),
    );
    transport(&CallosumEnvelope {
        tract: "supervisor".to_owned(),
        event: "request".to_owned(),
        ts: None,
        extra,
    })
}
fn fallback(locale: &str) -> Value {
    json!({"state":"unknown","headline":text(locale, "journal.window.health.brain.thinking.status.unavailable"),"reason_code":"brain_record_unavailable","reason_text":text(locale, "journal.window.health.brain.brain.record.unavailable"),"failing_component":null,"action":{"label":text(locale, "journal.window.health.brain.check.again"),"refresh":true},"identity":{"lane":null,"provider":null,"model":null},"evidence":{"observed_at":null,"age_seconds":null,"age_text":null},"components":{"generate":{"status":null,"reason_code":null,"reason_text":text(locale, "journal.window.health.brain.unknown"),"observed_at":null}},"progressing":false})
}
