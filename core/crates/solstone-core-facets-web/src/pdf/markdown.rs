// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};

use super::fonts::Face;

// Deliberate Markdown flavour: ENABLE_TABLES is the only option. Tables become
// text rows retaining every cell; images render alt text only and never fetch
// URLs (_safe_pdf_url_fetcher is deliberately unported); HTML is literal source
// text; links retain labels only; lists are indented bare lines; blockquotes are
// indented text; code uses Courier. No other extension is silently enabled.

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Run {
    pub(crate) text: String,
    pub(crate) face: Face,
    pub(crate) size: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Block {
    pub(crate) kind: BlockKind,
    pub(crate) runs: Vec<Run>,
    pub(crate) indent: f32,
    pub(crate) leading: f32,
    pub(crate) after: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BlockKind {
    Text,
    Table,
}

impl Block {
    pub(crate) fn text(text: String, face: Face, size: f32, indent: f32) -> Self {
        Self {
            kind: BlockKind::Text,
            runs: vec![Run { text, face, size }],
            indent,
            leading: size * 1.6,
            after: size * 0.55,
        }
    }
}

pub(crate) fn layout(markdown: &str) -> Vec<Block> {
    let mut output = Vec::new();
    let mut current: Option<Block> = None;
    let mut styles = vec![(Face::TimesRoman, 11.0_f32)];
    let mut list_depth = 0_u32;
    let mut table_cells: Vec<String> = Vec::new();
    let mut table_cell = String::new();
    let mut in_table = false;

    for event in Parser::new_ext(markdown, Options::ENABLE_TABLES) {
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                finish(&mut output, &mut current);
                let size = heading_size(level);
                current = Some(Block::text(String::new(), Face::TimesBold, size, 0.0));
                styles.push((Face::TimesBold, size));
            }
            Event::End(TagEnd::Heading(_)) => {
                styles.pop();
                finish(&mut output, &mut current);
            }
            Event::Start(Tag::Paragraph) => {
                finish(&mut output, &mut current);
                current = Some(Block::text(
                    String::new(),
                    Face::TimesRoman,
                    11.0,
                    list_indent(list_depth),
                ));
            }
            Event::End(TagEnd::Paragraph) => finish(&mut output, &mut current),
            Event::Start(Tag::BlockQuote(_)) => {
                finish(&mut output, &mut current);
                current = Some(Block::text(String::new(), Face::TimesRoman, 11.0, 18.0));
            }
            Event::End(TagEnd::BlockQuote(_)) => finish(&mut output, &mut current),
            Event::Start(Tag::List(_)) => list_depth += 1,
            Event::End(TagEnd::List(_)) => list_depth = list_depth.saturating_sub(1),
            Event::Start(Tag::Item) => {
                finish(&mut output, &mut current);
                current = Some(Block::text(
                    String::new(),
                    Face::TimesRoman,
                    11.0,
                    list_indent(list_depth),
                ));
            }
            Event::End(TagEnd::Item) => finish(&mut output, &mut current),
            Event::Start(Tag::CodeBlock(_)) => {
                finish(&mut output, &mut current);
                current = Some(Block::text(String::new(), Face::Courier, 9.9, 0.0));
            }
            Event::End(TagEnd::CodeBlock) => finish(&mut output, &mut current),
            Event::Start(Tag::Emphasis) => {
                styles.push((emphasis_face(current_face(&styles)), current_size(&styles)));
            }
            Event::End(TagEnd::Emphasis) => {
                styles.pop();
            }
            Event::Start(Tag::Strong) => {
                styles.push((strong_face(current_face(&styles)), current_size(&styles)));
            }
            Event::End(TagEnd::Strong) => {
                styles.pop();
            }
            Event::Start(Tag::Table(_)) => {
                finish(&mut output, &mut current);
                in_table = true;
            }
            Event::End(TagEnd::Table) => in_table = false,
            Event::Start(Tag::TableCell) => table_cell.clear(),
            Event::End(TagEnd::TableCell) => table_cells.push(std::mem::take(&mut table_cell)),
            Event::End(TagEnd::TableHead) | Event::End(TagEnd::TableRow) => {
                if !table_cells.is_empty() {
                    let mut row = Block::text(table_cells.join(" | "), Face::TimesRoman, 11.0, 0.0);
                    row.kind = BlockKind::Table;
                    output.push(row);
                    table_cells.clear();
                }
            }
            Event::Text(text) | Event::Html(text) | Event::InlineHtml(text) => {
                push_text(
                    &mut current,
                    &mut table_cell,
                    in_table,
                    text.as_ref(),
                    &styles,
                );
            }
            Event::Code(text) => push_run(&mut current, text.as_ref(), Face::Courier, 9.9),
            Event::SoftBreak | Event::HardBreak => push_run(
                &mut current,
                "\n",
                current_face(&styles),
                current_size(&styles),
            ),
            Event::Rule => {
                finish(&mut output, &mut current);
                output.push(Block::text("—".to_owned(), Face::TimesRoman, 11.0, 0.0));
            }
            _ => {}
        }
    }
    finish(&mut output, &mut current);
    output
}

fn heading_size(level: HeadingLevel) -> f32 {
    match level {
        HeadingLevel::H1 => 19.0,
        HeadingLevel::H2 => 15.0,
        HeadingLevel::H3 => 13.0,
        HeadingLevel::H4 | HeadingLevel::H5 | HeadingLevel::H6 => 12.0,
    }
}

fn list_indent(depth: u32) -> f32 {
    12.0 * depth as f32
}

fn current_face(styles: &[(Face, f32)]) -> Face {
    styles.last().expect("base style").0
}

fn current_size(styles: &[(Face, f32)]) -> f32 {
    styles.last().expect("base style").1
}

fn emphasis_face(face: Face) -> Face {
    match face {
        Face::TimesBold | Face::TimesBoldItalic => Face::TimesBoldItalic,
        Face::TimesRoman | Face::TimesItalic | Face::Courier => Face::TimesItalic,
    }
}

fn strong_face(face: Face) -> Face {
    match face {
        Face::TimesItalic | Face::TimesBoldItalic => Face::TimesBoldItalic,
        Face::TimesRoman | Face::TimesBold | Face::Courier => Face::TimesBold,
    }
}

fn push_text(
    current: &mut Option<Block>,
    table_cell: &mut String,
    in_table: bool,
    text: &str,
    styles: &[(Face, f32)],
) {
    if in_table {
        table_cell.push_str(text);
    } else {
        push_run(current, text, current_face(styles), current_size(styles));
    }
}

fn push_run(current: &mut Option<Block>, text: &str, face: Face, size: f32) {
    let block =
        current.get_or_insert_with(|| Block::text(String::new(), Face::TimesRoman, 11.0, 0.0));
    if let Some(last) = block
        .runs
        .last_mut()
        .filter(|last| last.face == face && last.size == size)
    {
        last.text.push_str(text);
    } else {
        block.runs.push(Run {
            text: text.to_owned(),
            face,
            size,
        });
    }
}

fn finish(output: &mut Vec<Block>, current: &mut Option<Block>) {
    if let Some(block) = current
        .take()
        .filter(|block| block.runs.iter().any(|run| !run.text.is_empty()))
    {
        output.push(block);
    }
}

#[cfg(test)]
mod tests {
    use super::{BlockKind, layout};
    use crate::pdf::{render, writer::extract_text_checked};

    fn rendered_text(markdown: &str) -> String {
        extract_text_checked(&render(markdown, "work", "Sun May 10, 2026"))
    }

    #[test]
    fn markdown_constructs_preserve_text() {
        let markdown = "| left unique cell | right unique cell |\n| --- | --- |\n| body unique cell | final unique cell |\n\n![only alt text](https://example.invalid/image.png)\n\n<section>literal html block</section>\n";
        let text = rendered_text(markdown);
        for expected in [
            "left unique cell",
            "right unique cell",
            "body unique cell",
            "final unique cell",
            "only alt text",
            "<section>literal html block</section>",
        ] {
            assert!(
                text.contains(expected),
                "missing {expected:?} from {text:?}"
            );
        }
        assert!(!text.contains("example.invalid"));

        let without_table = layout_without_table(markdown)
            .into_iter()
            .flat_map(|block| block.runs)
            .map(|run| run.text)
            .collect::<String>();
        assert!(
            !without_table.contains("body unique cell"),
            "stub must drop the table node"
        );
    }

    fn layout_without_table(markdown: &str) -> Vec<super::Block> {
        layout(markdown)
            .into_iter()
            .filter(|block| block.kind != BlockKind::Table)
            .collect()
    }
}
