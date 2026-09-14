// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::marker::PhantomData;
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

struct CompiledPattern {
    pattern: Pattern,
    index: usize,
}

pub struct Resolver<T: Copy, P: PatternSpec<T> + 'static> {
    patterns: &'static [P],
    structural: OnceLock<Vec<CompiledPattern>>,
    day_rooted: OnceLock<Vec<CompiledPattern>>,
    marker: PhantomData<T>,
}

impl<T: Copy, P: PatternSpec<T> + 'static> Resolver<T, P> {
    pub const fn new(patterns: &'static [P]) -> Self {
        Self {
            patterns,
            structural: OnceLock::new(),
            day_rooted: OnceLock::new(),
            marker: PhantomData,
        }
    }

    pub fn resolve(&self, rel: &str) -> Option<T> {
        self.resolve_index(rel)
            .map(|index| self.patterns[index].value())
    }

    /// Return the exact specification selected by first-match resolution.
    pub fn resolve_spec(&self, rel: &str) -> Option<&'static P> {
        self.resolve_index(rel).map(|index| &self.patterns[index])
    }

    fn resolve_index(&self, rel: &str) -> Option<usize> {
        let options = MatchOptions {
            case_sensitive: true,
            require_literal_separator: true,
            require_literal_leading_dot: false,
        };
        let rel_path = Path::new(rel);
        self.patterns_for_root(PatternRoot::Structural)
            .iter()
            .chain(self.patterns_for_root(PatternRoot::DayRooted))
            .find_map(|spec| {
                spec.pattern
                    .matches_path_with(rel_path, options)
                    .then_some(spec.index)
            })
    }

    fn patterns_for_root(&self, root: PatternRoot) -> &[CompiledPattern] {
        let cache = match root {
            PatternRoot::Structural => &self.structural,
            PatternRoot::DayRooted => &self.day_rooted,
        };
        cache.get_or_init(|| compile(self.patterns, root))
    }
}

fn compile<T: Copy, P: PatternSpec<T>>(patterns: &[P], root: PatternRoot) -> Vec<CompiledPattern> {
    patterns
        .iter()
        .enumerate()
        .filter(|(_, spec)| spec.root() == root)
        .map(|(index, spec)| CompiledPattern {
            pattern: Pattern::new(spec.pattern()).expect("pattern should be valid"),
            index,
        })
        .collect()
}

pub fn patterns_for_root<T: Copy, P: PatternSpec<T>>(
    patterns: &[P],
    root: PatternRoot,
) -> impl Iterator<Item = &P> {
    patterns.iter().filter(move |spec| spec.root() == root)
}
