// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::{Map, Value, json};
use solstone_core_callosum::{CallosumEnvelope, CallosumOneShotSender};
use std::time::Duration;

pub fn snapshot(root: &std::path::Path) -> Value {
    match solstone_core_thinking::read_config(root) {
        Ok(config) => {
            solstone_core_thinking::brain::presentation(root, &config, false)["brain"].clone()
        }
        Err(_) => fallback(),
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
    extra.insert("cmd".to_owned(), json!(["journal", "brain", "refresh"]));
    transport(&CallosumEnvelope {
        tract: "supervisor".to_owned(),
        event: "request".to_owned(),
        ts: None,
        extra,
    })
}
fn fallback() -> Value {
    json!({"state":"unknown","headline":"thinking status unavailable","reason_code":"brain_record_unavailable","reason_text":"brain record unavailable","failing_component":null,"action":{"label":"check again","refresh":true},"identity":{"lane":null,"provider":null,"model":null},"evidence":{"observed_at":null,"age_seconds":null,"age_text":null},"components":{"generate":{"status":null,"reason_code":null,"reason_text":"unknown","observed_at":null},"cogitate":{"status":null,"reason_code":null,"reason_text":"unknown","observed_at":null}},"progressing":false})
}
