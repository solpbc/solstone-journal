// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

pub const MAX_LINE_CHARS: usize = 2048;
pub const MAX_EXTRACTION_CHARS: usize = 32_768;
pub const EXTRACTION_BOUND_MARKER: &str = "\n\n[solstone: extraction output bounded before journaling - degenerate length sanitized/truncated]";

pub fn sanitize_markdown(text: &str) -> String {
    text.split('\n')
        .filter(|line| line.chars().count() <= MAX_LINE_CHARS)
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn bound_extraction_markdown(text: &str) -> String {
    let mut sanitized = sanitize_markdown(text);
    let mut changed = sanitized != text;
    let budget = MAX_EXTRACTION_CHARS - EXTRACTION_BOUND_MARKER.chars().count();
    if sanitized.chars().count() > budget {
        sanitized = sanitized.chars().take(budget).collect();
        changed = true;
    }
    if changed {
        sanitized.push_str(EXTRACTION_BOUND_MARKER);
    }
    sanitized
}

#[cfg(all(test, not(feature = "full-tests")))]
mod tests {
    use super::{EXTRACTION_BOUND_MARKER, MAX_EXTRACTION_CHARS, bound_extraction_markdown};

    #[test]
    fn drops_overlong_lines_and_marks_result() {
        let text = format!("kept\n{}\nalso kept", "x".repeat(2049));
        assert_eq!(
            bound_extraction_markdown(&text),
            format!("kept\nalso kept{EXTRACTION_BOUND_MARKER}")
        );
        assert_eq!(
            EXTRACTION_BOUND_MARKER,
            "\n\n[solstone: extraction output bounded before journaling - degenerate length sanitized/truncated]"
        );
    }

    #[test]
    fn caps_many_short_lines_independently_of_line_limit() {
        let text = "short line\n".repeat(4000);
        let bounded = bound_extraction_markdown(&text);
        assert!(bounded.ends_with(EXTRACTION_BOUND_MARKER));
        assert!(bounded.chars().count() <= MAX_EXTRACTION_CHARS);
    }

    #[test]
    fn leaves_healthy_markdown_byte_identical() {
        let text = "# Heading\n\nA short paragraph with café.";
        assert_eq!(bound_extraction_markdown(text), text);
    }
}
