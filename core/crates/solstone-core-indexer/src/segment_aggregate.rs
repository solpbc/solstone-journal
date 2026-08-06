// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::{Path, PathBuf};

use glob::{Pattern, glob};

use crate::stream::extract_stream;
use solstone_core_format::chunker::format_markdown;
use solstone_core_format::paths::resolve_journal_path;
use solstone_core_format::segment::time_bucket;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentAggregateRow {
    pub path: String,
    pub day: String,
    pub facet: String,
    pub agent: String,
    pub stream: Option<String>,
    pub idx: i64,
    pub time_bucket: String,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentAggregate {
    pub rows: Vec<SegmentAggregateRow>,
    pub warnings: Vec<String>,
    pub complete: bool,
}

pub fn build_segment_aggregate(journal: &Path, rel_segment: &str) -> SegmentAggregate {
    let mut warnings = Vec::new();
    let stream_lookup = extract_stream(journal, rel_segment);
    let stream = stream_lookup.stream;
    warnings.extend(stream_lookup.warning);

    let segment_dir = match resolve_journal_path(journal, rel_segment) {
        Ok(path) => path,
        Err(error) => {
            warnings.push(format!(
                "segment aggregate path failed for {rel_segment}: {error}"
            ));
            return SegmentAggregate {
                rows: Vec::new(),
                warnings,
                complete: true,
            };
        }
    };

    let mut talent_files = Vec::new();
    collect_globbed_paths(
        &segment_dir,
        "talents/*.md",
        &mut talent_files,
        &mut warnings,
    );
    collect_globbed_paths(
        &segment_dir,
        "talents/*/*.md",
        &mut talent_files,
        &mut warnings,
    );
    talent_files.sort_by_key(|path| path.to_string_lossy().into_owned());

    let mut contents = Vec::new();
    let mut complete = true;
    for path in talent_files {
        match fs::read_to_string(&path) {
            Ok(text) => contents.push(text),
            Err(error) => {
                complete = false;
                warnings.push(format!(
                    "segment aggregate read failed for {}: {error}",
                    path.display()
                ));
            }
        }
    }

    let content = contents.join("\n\n---\n\n");
    let day = rel_segment
        .split('/')
        .next()
        .unwrap_or_default()
        .to_string();
    let bucket = time_bucket(rel_segment);
    let formatted = format_markdown(&content);
    warnings.extend(formatted.warnings);

    let mut rows = Vec::new();
    for chunk in formatted.chunks {
        let content = chunk.markdown.trim();
        if content.is_empty() {
            continue;
        }
        rows.push(SegmentAggregateRow {
            path: rel_segment.to_string(),
            day: day.clone(),
            facet: String::new(),
            agent: "segment".to_string(),
            stream: stream.clone(),
            idx: rows.len() as i64,
            time_bucket: bucket.clone(),
            content: content.to_string(),
        });
    }

    SegmentAggregate {
        rows,
        warnings,
        complete,
    }
}

fn collect_globbed_paths(
    root: &Path,
    suffix: &str,
    paths: &mut Vec<PathBuf>,
    warnings: &mut Vec<String>,
) {
    let Some(root_str) = root.to_str() else {
        warnings.push(format!(
            "segment aggregate glob root is not valid UTF-8: {}",
            root.display()
        ));
        return;
    };
    let separator = if root_str.ends_with('/') { "" } else { "/" };
    let pattern = format!("{}{separator}{suffix}", Pattern::escape(root_str));
    let entries = match glob(&pattern) {
        Ok(entries) => entries,
        Err(error) => {
            warnings.push(format!(
                "segment aggregate glob pattern failed for {pattern}: {error}"
            ));
            return;
        }
    };
    for entry in entries {
        match entry {
            Ok(path) => paths.push(path),
            Err(error) => warnings.push(format!("segment aggregate glob failed: {error}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_root(name: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be available")
            .as_nanos();
        std::env::temp_dir().join(format!("solstone-core-indexer-aggregate-{name}-{stamp}"))
    }

    fn write(root: &Path, rel: &str, text: &str) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().expect("test path should have parent"))
            .expect("create parent");
        fs::write(path, text).expect("write test file");
    }

    #[test]
    fn aggregate_uses_markdown_formatter_cardinality_and_tokens() {
        let root = temp_root("markdown-cardinality");
        write(
            &root,
            "chronicle/20240102/default/090000_300/talents/audio.md",
            "# Audio\n\nintro alpha\n\n- item one\n- item two\n",
        );

        let aggregate = build_segment_aggregate(&root, "20240102/default/090000_300");

        assert!(aggregate.complete);
        assert!(aggregate.warnings.is_empty());
        assert_eq!(aggregate.rows.len(), 2);
        assert_eq!(
            tokens(&aggregate.rows[0].content),
            ["audio", "intro", "alpha", "item", "one"]
        );
        assert_eq!(
            tokens(&aggregate.rows[1].content),
            ["audio", "intro", "alpha", "item", "two"]
        );

        fs::remove_dir_all(root).expect("cleanup aggregate root");
    }

    #[test]
    fn aggregate_retains_markdown_formatter_warnings() {
        let root = temp_root("markdown-warning");
        write(
            &root,
            "chronicle/20240102/default/090000_300/talents/audio.md",
            &format!("# Audio\n\n{}\n\nkept alpha\n", "z".repeat(2049)),
        );

        let aggregate = build_segment_aggregate(&root, "20240102/default/090000_300");

        assert_eq!(
            aggregate.warnings,
            vec!["Dropped 1 line(s) exceeding 2048 chars during markdown sanitization"]
        );
        assert_eq!(aggregate.rows.len(), 1);
        assert_eq!(
            tokens(&aggregate.rows[0].content),
            ["audio", "kept", "alpha"]
        );

        fs::remove_dir_all(root).expect("cleanup aggregate root");
    }

    fn tokens(text: &str) -> Vec<String> {
        text.split(|ch: char| !ch.is_ascii_alphanumeric())
            .filter(|token| !token.is_empty())
            .map(|token| token.to_ascii_lowercase())
            .collect()
    }
}
