// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::time::Instant;

use serde_json::{Map, Value, json};

use crate::memory::ThrottleState;

pub fn sanitize_reason(reason: &str) -> String {
    reason
        .split_whitespace()
        .filter(|part| !part.contains('/') && !part.contains('\\'))
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(200)
        .collect()
}

#[derive(Debug)]
pub struct Health {
    started: Instant,
    pub recent_error_count: u8,
    pub last_successful_sync: Option<i64>,
    pub last_error_reason: Option<String>,
}

impl Default for Health {
    fn default() -> Self {
        Self {
            started: Instant::now(),
            recent_error_count: 0,
            last_successful_sync: None,
            last_error_reason: None,
        }
    }
}

impl Health {
    pub fn success(&mut self) {
        self.recent_error_count = 0;
        self.last_successful_sync = Some(chrono::Utc::now().timestamp_millis());
    }
    pub fn failure(&mut self, reason: &str) {
        self.recent_error_count = self.recent_error_count.saturating_add(1).min(99);
        self.last_error_reason = Some(sanitize_reason(reason));
    }
    pub fn beacon(&self, pending: usize, throttle: ThrottleState) -> Map<String, Value> {
        let mut object = Map::new();
        object.insert("name".into(), json!("native.observe"));
        object.insert("stream_type".into(), json!("screen_audio"));
        object.insert("version".into(), json!(env!("CARGO_PKG_VERSION")));
        object.insert("uptime".into(), json!(self.started.elapsed().as_secs()));
        object.insert(
            "last_successful_sync".into(),
            self.last_successful_sync.map_or(Value::Null, |v| json!(v)),
        );
        object.insert("pending_queue_depth".into(), json!(pending));
        object.insert("recent_error_count".into(), json!(self.recent_error_count));
        object.insert(
            "last_error_reason".into(),
            self.last_error_reason
                .as_ref()
                .map_or(Value::Null, |v| json!(v)),
        );
        object.insert("memory_throttled".into(), json!(throttle.throttled));
        object.insert("memory_throttle_count".into(), json!(throttle.count));
        object.insert(
            "memory_floor_mib".into(),
            throttle.floor_mib.map_or(Value::Null, |v| json!(v)),
        );
        object.insert(
            "memory_available_mib".into(),
            throttle.available_mib.map_or(Value::Null, |v| json!(v)),
        );
        object
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reason_is_single_line_and_capped() {
        let value = sanitize_reason(&format!("/private/path\n{}", "x".repeat(300)));
        assert!(!value.contains('\n'));
        assert!(!value.contains("/private/path"));
        assert_eq!(value.chars().count(), 200);
    }
    #[test]
    fn count_caps_and_success_resets() {
        let mut health = Health::default();
        for _ in 0..120 {
            health.failure("bad");
        }
        assert_eq!(health.recent_error_count, 99);
        health.success();
        assert_eq!(health.recent_error_count, 0);
    }
}
