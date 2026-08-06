// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use solstone_core_format::matcher::{
    PatternRoot, PatternSpec, Resolver, patterns_for_root as filter_patterns_for_root,
};

use super::EdgeError;

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
    pub root: PatternRoot,
    pub kind: EdgeSourceKind,
}

pub(crate) const EDGE_SOURCE_PATTERNS: &[EdgeSourcePattern] = &[
    EdgeSourcePattern {
        pattern: "facets/*/activities/*.jsonl",
        root: PatternRoot::Structural,
        kind: EdgeSourceKind::Activity,
    },
    EdgeSourcePattern {
        pattern: "facets/*/entities/*/observations.jsonl",
        root: PatternRoot::Structural,
        kind: EdgeSourceKind::Observation,
    },
    EdgeSourcePattern {
        pattern: "facets/*/entities/*.jsonl",
        root: PatternRoot::Structural,
        kind: EdgeSourceKind::Copresence,
    },
    EdgeSourcePattern {
        pattern: "facets/*/events/*.jsonl",
        root: PatternRoot::Structural,
        kind: EdgeSourceKind::EventLegacy,
    },
    EdgeSourcePattern {
        pattern: "*/*/*/screen.jsonl",
        root: PatternRoot::DayRooted,
        kind: EdgeSourceKind::Screen,
    },
    EdgeSourcePattern {
        pattern: "*/*/*/*_screen.jsonl",
        root: PatternRoot::DayRooted,
        kind: EdgeSourceKind::Screen,
    },
    EdgeSourcePattern {
        pattern: "*/*/*/talents/documents.json",
        root: PatternRoot::DayRooted,
        kind: EdgeSourceKind::Document,
    },
    EdgeSourcePattern {
        pattern: "*/*/*/talents/speaker_labels.json",
        root: PatternRoot::DayRooted,
        kind: EdgeSourceKind::Speaker,
    },
];

impl PatternSpec<EdgeSourceKind> for EdgeSourcePattern {
    fn pattern(&self) -> &'static str {
        self.pattern
    }

    fn root(&self) -> PatternRoot {
        self.root
    }

    fn value(&self) -> EdgeSourceKind {
        self.kind
    }
}

static EDGE_SOURCE_RESOLVER: Resolver<EdgeSourceKind> = Resolver::new();

pub(crate) fn patterns_for_root(
    root: PatternRoot,
) -> impl Iterator<Item = &'static EdgeSourcePattern> {
    filter_patterns_for_root(EDGE_SOURCE_PATTERNS, root)
}

pub fn edge_source_for_rel(rel: &str) -> Result<Option<EdgeSourceKind>, EdgeError> {
    Ok(EDGE_SOURCE_RESOLVER.resolve(EDGE_SOURCE_PATTERNS, rel))
}

#[cfg(test)]
mod tests {
    use super::*;
    use glob::Pattern;

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
