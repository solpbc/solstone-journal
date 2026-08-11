// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

pub(crate) fn replace_section(existing: &str, heading: &str, new_value: &str) -> Option<String> {
    let lines: Vec<&str> = existing.split('\n').collect();
    let target = format!("## {heading}");
    let mut start = None;
    let mut end = None;
    for (index, line) in lines.iter().enumerate() {
        if *line == target {
            start = Some(index);
        } else if start.is_some() && line.starts_with("## ") {
            end = Some(index);
            break;
        }
    }
    let start = start?;
    let end = end.unwrap_or(lines.len());
    let mut new_lines = Vec::new();
    new_lines.extend_from_slice(&lines[..start + 1]);
    if !new_value.is_empty() {
        new_lines.extend(new_value.split('\n'));
    }
    new_lines.push("");
    new_lines.extend_from_slice(&lines[end..]);
    Some(new_lines.join("\n"))
}

pub(crate) fn prune_partner_getting_started(content: &str) -> String {
    if !content.contains("## getting started") {
        return content.to_owned();
    }
    let lines: Vec<&str> = content.split('\n').collect();
    let mut start = None;
    let mut end = None;
    for (index, line) in lines.iter().enumerate() {
        if *line == "## getting started" {
            start = Some(index);
        } else if start.is_some() && line.starts_with("## ") {
            end = Some(index);
            break;
        }
    }
    let Some(start) = start else {
        return content.to_owned();
    };
    let end = end.unwrap_or(lines.len());
    lines[..start]
        .iter()
        .chain(lines[end..].iter())
        .copied()
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::{prune_partner_getting_started, replace_section};

    #[test]
    fn replace_section_returns_none_when_the_heading_is_absent() {
        assert_eq!(
            replace_section("# partner\n", "work patterns", "value"),
            None
        );
    }

    #[test]
    fn replace_section_preserves_the_next_top_level_heading() {
        let existing = "# partner\n\n## work patterns\nold\n\n## communication style\nkeep\n";
        assert_eq!(
            replace_section(existing, "work patterns", "new"),
            Some("# partner\n\n## work patterns\nnew\n\n## communication style\nkeep\n".to_owned())
        );
    }

    #[test]
    fn replace_section_replaces_a_final_section_and_keeps_its_trailing_empty_line() {
        assert_eq!(
            replace_section("## work patterns\nold\n", "work patterns", "new"),
            Some("## work patterns\nnew\n".to_owned())
        );
    }

    #[test]
    fn replace_section_inserts_no_value_lines_for_an_empty_value() {
        assert_eq!(
            replace_section("## work patterns\nold\n## next\n", "work patterns", ""),
            Some("## work patterns\n\n## next\n".to_owned())
        );
    }

    #[test]
    fn prune_removes_only_the_getting_started_top_level_section() {
        let content = "# partner\n\n## getting started\nintro\n### nested\nkeep with section\n\n## work patterns\nobserved\n";
        assert_eq!(
            prune_partner_getting_started(content),
            "# partner\n\n## work patterns\nobserved\n"
        );
    }

    #[test]
    fn prune_leaves_content_without_the_exact_heading_unchanged() {
        let content = "## Getting Started\ncase differs\n";
        assert_eq!(prune_partner_getting_started(content), content);
    }
}
