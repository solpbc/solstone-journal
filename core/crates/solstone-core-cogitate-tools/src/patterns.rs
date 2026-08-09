// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use regex::{Regex, RegexBuilder};

/// Python `fnmatch.fnmatch`-style wildcard matching. `*` deliberately spans `/`.
pub(crate) fn fnmatch(value: &str, pattern: &str) -> bool {
    let value: Vec<char> = value.chars().collect();
    let pattern: Vec<char> = pattern.chars().collect();
    matches(&value, &pattern, 0, 0)
}

fn matches(value: &[char], pattern: &[char], value_at: usize, pattern_at: usize) -> bool {
    if pattern_at == pattern.len() {
        return value_at == value.len();
    }
    match pattern[pattern_at] {
        '*' => (value_at..=value.len()).any(|next| matches(value, pattern, next, pattern_at + 1)),
        '?' => value_at < value.len() && matches(value, pattern, value_at + 1, pattern_at + 1),
        '[' => match_class(value, pattern, value_at, pattern_at),
        character => {
            value_at < value.len()
                && value[value_at] == character
                && matches(value, pattern, value_at + 1, pattern_at + 1)
        }
    }
}

fn match_class(value: &[char], pattern: &[char], value_at: usize, pattern_at: usize) -> bool {
    let mut index = pattern_at + 1;
    let negated = pattern.get(index) == Some(&'!');
    if negated {
        index += 1;
    }
    if pattern.get(index) == Some(&']') {
        index += 1;
    }
    let Some(offset) = pattern[index..]
        .iter()
        .position(|character| *character == ']')
    else {
        return value_at < value.len()
            && value[value_at] == '['
            && matches(value, pattern, value_at + 1, pattern_at + 1);
    };
    if value_at == value.len() {
        return false;
    }
    let end = index + offset;
    let mut index = pattern_at + 1 + usize::from(negated);
    let mut accepted = false;
    while index < end {
        if index + 2 < end && pattern[index + 1] == '-' {
            accepted |= pattern[index] <= value[value_at] && value[value_at] <= pattern[index + 2];
            index += 3;
        } else {
            accepted |= pattern[index] == value[value_at];
            index += 1;
        }
    }
    (accepted != negated) && matches(value, pattern, value_at + 1, end + 1)
}

pub(crate) fn grep_matcher(
    pattern: &str,
    regex: bool,
    case_sensitive: bool,
) -> Result<Regex, regex::Error> {
    let expression = if regex {
        pattern.to_owned()
    } else {
        regex::escape(pattern)
    };
    RegexBuilder::new(&expression)
        .case_insensitive(!case_sensitive)
        .build()
}

#[cfg(test)]
mod tests {
    use super::fnmatch;

    #[test]
    fn bracket_classes_match_python_fnmatch() {
        assert!(fnmatch("a", "[^abc]"));
        assert!(!fnmatch("z", "[^abc]"));
        assert!(fnmatch("z", "[!abc]"));
        assert!(!fnmatch("a", "[!abc]"));
        assert!(fnmatch("]", "[]a]"));
        assert!(fnmatch("a", "[]a]"));
        assert!(fnmatch("[abc", "[abc"));
        assert!(!fnmatch("abc", "[abc"));
    }
}
