// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::io::ErrorKind;
use std::path::Path;

use serde_json::Value;

use solstone_core_format::paths::resolve_journal_path;
use solstone_core_format::segment::segment_key;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamLookup {
    pub stream: Option<String>,
    pub warning: Option<String>,
}

pub fn extract_stream(journal: &Path, rel: &str) -> StreamLookup {
    let normalized = rel.replace('\\', "/");
    let parts: Vec<&str> = normalized.split('/').collect();
    if parts.len() < 3 || segment_key(parts[2]).is_none() {
        return StreamLookup {
            stream: None,
            warning: None,
        };
    }
    let rel_segment = parts[..3].join("/");
    let seg_dir = match resolve_journal_path(journal, &rel_segment) {
        Ok(path) => path,
        Err(error) => {
            return StreamLookup {
                stream: None,
                warning: Some(format!(
                    "stream marker path failed for {rel_segment}: {error}"
                )),
            };
        }
    };
    let marker_path = seg_dir.join("stream.json");
    let text = match fs::read_to_string(&marker_path) {
        Ok(text) => text,
        Err(error) => {
            if error.kind() == ErrorKind::NotFound {
                return StreamLookup {
                    stream: None,
                    warning: None,
                };
            }
            return StreamLookup {
                stream: None,
                warning: Some(format!(
                    "stream marker read failed for {}: {error}",
                    marker_path.display()
                )),
            };
        }
    };
    let value: Value = match serde_json::from_str(&text) {
        Ok(value) => value,
        Err(error) => {
            return StreamLookup {
                stream: None,
                warning: Some(format!(
                    "stream marker JSON failed for {}: {error}",
                    marker_path.display()
                )),
            };
        }
    };
    StreamLookup {
        stream: value
            .get("stream")
            .and_then(Value::as_str)
            .map(str::to_string),
        warning: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::reserve_temp_path;
    use std::fs;
    use std::path::PathBuf;

    fn temp_root(name: &str) -> PathBuf {
        reserve_temp_path(&format!("solstone-core-indexer-stream-{name}"))
    }

    #[test]
    fn reads_stream_for_segment_paths() {
        let root = temp_root("read");
        let seg = root.join("chronicle/20240101/default/123456_300");
        fs::create_dir_all(&seg).expect("create segment");
        fs::write(seg.join("stream.json"), r#"{"stream":"default"}"#).expect("write marker");
        let lookup = extract_stream(&root, "20240101/default/123456_300/talents/audio.md");
        assert_eq!(lookup.stream, Some("default".to_string()));
        assert_eq!(lookup.warning, None);
        fs::remove_dir_all(root).expect("cleanup stream root");
    }

    #[test]
    fn missing_stream_marker_is_silent() {
        let root = temp_root("missing");
        let seg = root.join("chronicle/20240101/default/123456_300");
        fs::create_dir_all(&seg).expect("create segment");
        let lookup = extract_stream(&root, "20240101/default/123456_300/talents/audio.md");
        assert_eq!(lookup.stream, None);
        assert_eq!(lookup.warning, None);
        fs::remove_dir_all(root).expect("cleanup stream root");
    }

    #[test]
    fn malformed_stream_marker_warns() {
        let root = temp_root("malformed");
        let seg = root.join("chronicle/20240101/default/123456_300");
        fs::create_dir_all(&seg).expect("create segment");
        fs::write(seg.join("stream.json"), "{not json").expect("write marker");
        let lookup = extract_stream(&root, "20240101/default/123456_300/talents/audio.md");
        assert_eq!(lookup.stream, None);
        assert!(
            lookup
                .warning
                .is_some_and(|warning| warning.contains("stream marker JSON failed"))
        );
        fs::remove_dir_all(root).expect("cleanup stream root");
    }

    #[test]
    fn coding_activity_path_is_not_a_segment_stream() {
        let root = temp_root("activity");
        let lookup = extract_stream(
            &root,
            "facets/work/activities/20260214/coding_093000_300/session_review.md",
        );
        assert_eq!(lookup.stream, None);
        assert_eq!(lookup.warning, None);
    }
}
