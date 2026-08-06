// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;
use std::sync::OnceLock;

use glob::{MatchOptions, Pattern};

use super::EdgeError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EdgePatternRoot {
    Structural,
    DayRooted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EdgeSourceKind {
    Activity,
    Observation,
    Copresence,
    EventLegacy,
    Screen,
    Document,
    Speaker,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct EdgeSourcePattern {
    pub pattern: &'static str,
    pub root: EdgePatternRoot,
    pub kind: EdgeSourceKind,
}

pub(crate) const EDGE_SOURCE_PATTERNS: &[EdgeSourcePattern] = &[
    EdgeSourcePattern {
        pattern: "facets/*/activities/*.jsonl",
        root: EdgePatternRoot::Structural,
        kind: EdgeSourceKind::Activity,
    },
    EdgeSourcePattern {
        pattern: "facets/*/entities/*/observations.jsonl",
        root: EdgePatternRoot::Structural,
        kind: EdgeSourceKind::Observation,
    },
    EdgeSourcePattern {
        pattern: "facets/*/entities/*.jsonl",
        root: EdgePatternRoot::Structural,
        kind: EdgeSourceKind::Copresence,
    },
    EdgeSourcePattern {
        pattern: "facets/*/events/*.jsonl",
        root: EdgePatternRoot::Structural,
        kind: EdgeSourceKind::EventLegacy,
    },
    EdgeSourcePattern {
        pattern: "*/*/*/screen.jsonl",
        root: EdgePatternRoot::DayRooted,
        kind: EdgeSourceKind::Screen,
    },
    EdgeSourcePattern {
        pattern: "*/*/*/*_screen.jsonl",
        root: EdgePatternRoot::DayRooted,
        kind: EdgeSourceKind::Screen,
    },
    EdgeSourcePattern {
        pattern: "*/*/*/talents/documents.json",
        root: EdgePatternRoot::DayRooted,
        kind: EdgeSourceKind::Document,
    },
    EdgeSourcePattern {
        pattern: "*/*/*/talents/speaker_labels.json",
        root: EdgePatternRoot::DayRooted,
        kind: EdgeSourceKind::Speaker,
    },
];

struct CompiledEdgeSourcePattern {
    pattern: Pattern,
    kind: EdgeSourceKind,
}

static STRUCTURAL_EDGE_SOURCE_PATTERNS: OnceLock<Vec<CompiledEdgeSourcePattern>> = OnceLock::new();
static DAY_ROOTED_EDGE_SOURCE_PATTERNS: OnceLock<Vec<CompiledEdgeSourcePattern>> = OnceLock::new();

fn compile_patterns(root: EdgePatternRoot) -> Vec<CompiledEdgeSourcePattern> {
    EDGE_SOURCE_PATTERNS
        .iter()
        .filter(|spec| spec.root == root)
        .map(|spec| CompiledEdgeSourcePattern {
            pattern: Pattern::new(spec.pattern).expect("edge source pattern should be valid"),
            kind: spec.kind,
        })
        .collect()
}

fn structural_patterns() -> &'static [CompiledEdgeSourcePattern] {
    STRUCTURAL_EDGE_SOURCE_PATTERNS.get_or_init(|| compile_patterns(EdgePatternRoot::Structural))
}

fn day_rooted_patterns() -> &'static [CompiledEdgeSourcePattern] {
    DAY_ROOTED_EDGE_SOURCE_PATTERNS.get_or_init(|| compile_patterns(EdgePatternRoot::DayRooted))
}

pub(crate) fn patterns_for_root(
    root: EdgePatternRoot,
) -> impl Iterator<Item = &'static EdgeSourcePattern> {
    EDGE_SOURCE_PATTERNS
        .iter()
        .filter(move |spec| spec.root == root)
}

pub fn edge_source_for_rel(rel: &str) -> Result<Option<EdgeSourceKind>, EdgeError> {
    let options = MatchOptions {
        case_sensitive: true,
        require_literal_separator: true,
        require_literal_leading_dot: false,
    };
    let rel_path = Path::new(rel);
    for spec in structural_patterns().iter().chain(day_rooted_patterns()) {
        if spec.pattern.matches_path_with(rel_path, options) {
            return Ok(Some(spec.kind));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edge_source_patterns_match_exact_structural_shapes() {
        assert_eq!(
            edge_source_for_rel("facets/work/activities/20260430.jsonl"),
            Ok(Some(EdgeSourceKind::Activity))
        );
        assert_eq!(
            edge_source_for_rel("facets/work/entities/alice/observations.jsonl"),
            Ok(Some(EdgeSourceKind::Observation))
        );
        assert_eq!(
            edge_source_for_rel("facets/work/entities/20260430.jsonl"),
            Ok(Some(EdgeSourceKind::Copresence))
        );
        assert_eq!(
            edge_source_for_rel("facets/work/events/20260430.jsonl"),
            Ok(Some(EdgeSourceKind::EventLegacy))
        );
        assert_eq!(
            edge_source_for_rel("facets/work/events/screen.jsonl"),
            Ok(Some(EdgeSourceKind::EventLegacy)),
            "structural edge source must win over the day-rooted screen pattern"
        );
        assert_eq!(
            edge_source_for_rel("20260430/default/090000_300/screen.jsonl"),
            Ok(Some(EdgeSourceKind::Screen))
        );
        assert_eq!(
            edge_source_for_rel("20260430/default/090000_300/left_screen.jsonl"),
            Ok(Some(EdgeSourceKind::Screen))
        );
        assert_eq!(
            edge_source_for_rel("20260430/default/090000_300/talents/documents.json"),
            Ok(Some(EdgeSourceKind::Document))
        );
        assert_eq!(
            edge_source_for_rel("20260430/default/090000_300/talents/speaker_labels.json"),
            Ok(Some(EdgeSourceKind::Speaker))
        );
        assert_eq!(
            edge_source_for_rel("facets/work/entities/alice/extra/observations.jsonl"),
            Ok(None)
        );
    }

    #[test]
    fn edge_source_patterns_compile() {
        for pattern in EDGE_SOURCE_PATTERNS {
            Pattern::new(pattern.pattern).expect("edge source pattern compiles");
        }
    }
}
