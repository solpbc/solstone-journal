// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;
use std::time::SystemTime;

use serde_json::{Map, Value, json};

use crate::work::{SegmentContext, SegmentState};

pub fn detected(
    journal: &Path,
    file: &Path,
    handler: &str,
    reference: &str,
    context: &SegmentContext,
) -> Map<String, Value> {
    let file = file
        .strip_prefix(journal)
        .map(|v| v.display().to_string())
        .unwrap_or_else(|_| file.display().to_string());
    let mut fields = Map::from_iter([
        (String::from("file"), json!(file)),
        (String::from("handler"), json!(handler)),
        (String::from("ref"), json!(reference)),
    ]);
    fields.insert("day".into(), json!(context.key.day));
    fields.insert("segment".into(), json!(context.key.segment));
    if let Some(stream) = &context.key.stream {
        fields.insert("stream".into(), json!(stream));
    }
    fields
}

pub fn observed(state: &SegmentState, note: Option<&str>) -> Map<String, Value> {
    let mut fields = Map::from_iter([
        (String::from("segment"), json!(state.context.key.segment)),
        (String::from("day"), json!(state.context.key.day)),
        (
            String::from("duration"),
            json!(state.started_at.elapsed().as_secs()),
        ),
    ]);
    if state.context.batch {
        fields.insert("batch".into(), Value::Bool(true));
    }
    if let Some(stream) = &state.context.key.stream {
        fields.insert("stream".into(), json!(stream));
    }
    if !state.errors.is_empty() {
        fields.insert("error".into(), Value::Bool(true));
        fields.insert("errors".into(), json!(state.errors));
    }
    if let Some(note) = note {
        fields.insert("note".into(), json!(note));
    }
    fields
}

pub fn queue_wait_ms(queued_at: SystemTime) -> u128 {
    SystemTime::now()
        .duration_since(queued_at)
        .map_or(0, |v| v.as_millis())
}

pub fn throttle_started(stage: &str, available_mib: u64, floor_mib: u64) -> Map<String, Value> {
    Map::from_iter([
        (String::from("stage"), json!(stage)),
        (String::from("available_mib"), json!(available_mib)),
        (String::from("floor_mib"), json!(floor_mib)),
    ])
}
pub fn throttle_completed(stage: &str, waited_seconds: f64) -> Map<String, Value> {
    Map::from_iter([
        (String::from("stage"), json!(stage)),
        (String::from("waited_seconds"), json!(waited_seconds)),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::work::{SegmentContext, SegmentKey, SegmentState};
    #[test]
    fn detected_keeps_identity() {
        let c = SegmentContext {
            key: SegmentKey {
                day: "20260101".into(),
                stream: Some("s".into()),
                segment: "120000_1".into(),
            },
            cid: Some("cid".into()),
            source: Some("source".into()),
            batch: false,
            meta: None,
        };
        let v = detected(
            Path::new("/j"),
            Path::new("/j/chronicle/a"),
            "describe",
            "r",
            &c,
        );
        assert_eq!(v["handler"], "describe");
        assert_eq!(v["file"], "chronicle/a");
        assert_eq!(v["ref"], "r");
        assert_eq!(v["day"], "20260101");
        assert_eq!(v["segment"], "120000_1");
        assert!(v.get("observer").is_none());
        assert_eq!(v["stream"], "s");
    }

    #[test]
    fn detected_handler_matches_each_registry_spec() {
        let context = SegmentContext {
            key: SegmentKey {
                day: "20260101".into(),
                stream: None,
                segment: "120000_1".into(),
            },
            cid: None,
            source: None,
            batch: false,
            meta: None,
        };
        let specs = crate::registry::default_registry(10);
        assert_eq!(specs.len(), 3);
        for spec in specs {
            let fields = detected(
                Path::new("/j"),
                Path::new("/j/file"),
                spec.name,
                "r",
                &context,
            );
            assert_eq!(fields["handler"], spec.name);
        }
    }
    #[test]
    fn observed_includes_error_once_shape() {
        let c = SegmentContext {
            key: SegmentKey {
                day: "20260101".into(),
                stream: None,
                segment: "s".into(),
            },
            cid: None,
            source: None,
            batch: false,
            meta: None,
        };
        let mut s = SegmentState::new(c);
        s.errors.push("bad".into());
        assert_eq!(observed(&s, None)["error"], true);
    }
}
