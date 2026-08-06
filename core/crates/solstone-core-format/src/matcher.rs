// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;
use std::sync::OnceLock;

use glob::{MatchOptions, Pattern};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PatternRoot {
    Structural,
    DayRooted,
}

pub trait PatternSpec<T: Copy> {
    fn pattern(&self) -> &'static str;
    fn root(&self) -> PatternRoot;
    fn value(&self) -> T;
}

struct CompiledPattern<T> {
    pattern: Pattern,
    value: T,
}

pub struct Resolver<T: Copy> {
    structural: OnceLock<Vec<CompiledPattern<T>>>,
    day_rooted: OnceLock<Vec<CompiledPattern<T>>>,
}

impl<T: Copy> Resolver<T> {
    pub const fn new() -> Self {
        Self {
            structural: OnceLock::new(),
            day_rooted: OnceLock::new(),
        }
    }

    pub fn resolve<P: PatternSpec<T>>(&self, patterns: &[P], rel: &str) -> Option<T> {
        let options = MatchOptions {
            case_sensitive: true,
            require_literal_separator: true,
            require_literal_leading_dot: false,
        };
        let rel_path = Path::new(rel);
        self.patterns_for_root(patterns, PatternRoot::Structural)
            .iter()
            .chain(self.patterns_for_root(patterns, PatternRoot::DayRooted))
            .find_map(|spec| {
                spec.pattern
                    .matches_path_with(rel_path, options)
                    .then_some(spec.value)
            })
    }

    fn patterns_for_root<P: PatternSpec<T>>(
        &self,
        patterns: &[P],
        root: PatternRoot,
    ) -> &[CompiledPattern<T>] {
        let cache = match root {
            PatternRoot::Structural => &self.structural,
            PatternRoot::DayRooted => &self.day_rooted,
        };
        cache.get_or_init(|| compile(patterns, root))
    }
}

impl<T: Copy> Default for Resolver<T> {
    fn default() -> Self {
        Self::new()
    }
}

fn compile<T: Copy, P: PatternSpec<T>>(
    patterns: &[P],
    root: PatternRoot,
) -> Vec<CompiledPattern<T>> {
    patterns
        .iter()
        .filter(|spec| spec.root() == root)
        .map(|spec| CompiledPattern {
            pattern: Pattern::new(spec.pattern()).expect("pattern should be valid"),
            value: spec.value(),
        })
        .collect()
}

pub fn patterns_for_root<T: Copy, P: PatternSpec<T>>(
    patterns: &[P],
    root: PatternRoot,
) -> impl Iterator<Item = &P> {
    patterns.iter().filter(move |spec| spec.root() == root)
}
