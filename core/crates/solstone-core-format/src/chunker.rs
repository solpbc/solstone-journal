// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, LinkType, Options, Parser, Tag, TagEnd};

const MAX_LINE_CHARS: usize = 2048;
const MAX_CHUNK_CHARS: usize = 4096;
const OVERLONG_LINE_WARNING: &str =
    "Dropped {count} line(s) exceeding 2048 chars during markdown sanitization";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkdownChunk {
    pub markdown: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkdownFormat {
    pub chunks: Vec<MarkdownChunk>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Header {
    level: HeadingLevel,
    text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum MarkdownBlock {
    Heading(Header),
    Paragraph(String),
    List(ListBlock),
    Table(TableBlock),
    Code(CodeBlock),
    BlockQuote(String),
    ThematicBreak,
    HtmlBlock,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ListBlock {
    items: Vec<ListItem>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ListItem {
    text: String,
    is_definition_item: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TableBlock {
    headers: Vec<String>,
    rows: Vec<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CodeBlock {
    info: String,
    body: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RawChunk {
    headers: Vec<Header>,
    body: String,
}

pub fn format_markdown(input: &str) -> MarkdownFormat {
    let (sanitized, warnings) = sanitize_markdown(input);
    let raw_chunks = chunk_markdown(&sanitized);
    let chunks = raw_chunks
        .into_iter()
        .map(|chunk| {
            let rendered = render_chunk(&chunk);
            let rendered_chars = rendered.chars().count();
            let markdown = if rendered_chars > MAX_CHUNK_CHARS {
                render_header_stub(&chunk.headers, rendered_chars)
            } else {
                rendered
            };
            MarkdownChunk { markdown }
        })
        .collect();
    MarkdownFormat { chunks, warnings }
}

fn sanitize_markdown(input: &str) -> (String, Vec<String>) {
    let mut clean = Vec::new();
    let mut dropped = 0usize;
    for line in input.split('\n') {
        if line.chars().count() > MAX_LINE_CHARS {
            dropped += 1;
        } else {
            clean.push(line);
        }
    }
    let warnings = if dropped == 0 {
        Vec::new()
    } else {
        vec![OVERLONG_LINE_WARNING.replace("{count}", &dropped.to_string())]
    };
    (clean.join("\n"), warnings)
}

fn chunk_markdown(input: &str) -> Vec<RawChunk> {
    let blocks = parse_blocks(input);
    chunk_blocks(&blocks)
}

fn parse_blocks(input: &str) -> Vec<MarkdownBlock> {
    let mut blocks = Vec::new();
    let mut active = ActiveBlock::None;
    for event in Parser::new_ext(input, Options::ENABLE_TABLES) {
        active = match active {
            ActiveBlock::None => start_top_level(event, &mut blocks),
            ActiveBlock::Heading(mut heading) => {
                if heading.handle(event, &mut blocks) {
                    ActiveBlock::None
                } else {
                    ActiveBlock::Heading(heading)
                }
            }
            ActiveBlock::Paragraph(mut paragraph) => {
                if paragraph.handle(event, &mut blocks) {
                    ActiveBlock::None
                } else {
                    ActiveBlock::Paragraph(paragraph)
                }
            }
            ActiveBlock::List(mut list) => {
                if list.handle(event, &mut blocks) {
                    ActiveBlock::None
                } else {
                    ActiveBlock::List(list)
                }
            }
            ActiveBlock::Table(mut table) => {
                if table.handle(event, &mut blocks) {
                    ActiveBlock::None
                } else {
                    ActiveBlock::Table(table)
                }
            }
            ActiveBlock::Code(mut code) => {
                if code.handle(event, &mut blocks) {
                    ActiveBlock::None
                } else {
                    ActiveBlock::Code(code)
                }
            }
            ActiveBlock::BlockQuote(mut quote) => {
                if quote.handle(event, &mut blocks) {
                    ActiveBlock::None
                } else {
                    ActiveBlock::BlockQuote(quote)
                }
            }
            ActiveBlock::HtmlBlock => match event {
                Event::End(TagEnd::HtmlBlock) => {
                    blocks.push(MarkdownBlock::HtmlBlock);
                    ActiveBlock::None
                }
                _ => ActiveBlock::HtmlBlock,
            },
        };
    }
    blocks
}

fn start_top_level(event: Event<'_>, blocks: &mut Vec<MarkdownBlock>) -> ActiveBlock {
    match event {
        Event::Start(Tag::Heading { level, .. }) => {
            ActiveBlock::Heading(HeadingBuilder::new(level))
        }
        Event::Start(Tag::Paragraph) => ActiveBlock::Paragraph(TextBlockBuilder::default()),
        Event::Start(Tag::List(_)) => ActiveBlock::List(ListBuilder::new()),
        Event::Start(Tag::Table(_)) => ActiveBlock::Table(TableBuilder::default()),
        Event::Start(Tag::CodeBlock(kind)) => ActiveBlock::Code(CodeBlockBuilder::new(kind)),
        Event::Start(Tag::BlockQuote(_)) => ActiveBlock::BlockQuote(QuoteBuilder::default()),
        Event::Start(Tag::HtmlBlock) => ActiveBlock::HtmlBlock,
        Event::Rule => {
            blocks.push(MarkdownBlock::ThematicBreak);
            ActiveBlock::None
        }
        _ => ActiveBlock::None,
    }
}

fn chunk_blocks(blocks: &[MarkdownBlock]) -> Vec<RawChunk> {
    let mut chunks = Vec::new();
    let mut headers: Vec<Header> = Vec::new();
    let mut intro: Option<String> = None;

    for (idx, block) in blocks.iter().enumerate() {
        match block {
            MarkdownBlock::Heading(header) => {
                while headers.last().is_some_and(|existing| {
                    heading_rank(existing.level) >= heading_rank(header.level)
                }) {
                    headers.pop();
                }
                if !header.text.trim().is_empty() {
                    headers.push(header.clone());
                }
                intro = None;
            }
            MarkdownBlock::Paragraph(text) => {
                if matches!(
                    blocks.get(idx + 1),
                    Some(MarkdownBlock::List(_)) | Some(MarkdownBlock::Table(_))
                ) {
                    intro = Some(text.clone());
                } else {
                    push_chunk(&mut chunks, &headers, text.clone());
                    intro = None;
                }
            }
            MarkdownBlock::List(list) => {
                if is_definition_list(list) {
                    let mut body = String::new();
                    append_intro(&mut body, intro.as_deref());
                    for item in &list.items {
                        append_piece(&mut body, &item.text);
                    }
                    push_chunk(&mut chunks, &headers, body);
                } else {
                    for item in &list.items {
                        let mut body = String::new();
                        append_intro(&mut body, intro.as_deref());
                        append_piece(&mut body, &item.text);
                        push_chunk(&mut chunks, &headers, body);
                    }
                }
                intro = None;
            }
            MarkdownBlock::Table(table) => {
                for row in &table.rows {
                    let mut body = String::new();
                    append_intro(&mut body, intro.as_deref());
                    append_table_row(&mut body, &table.headers);
                    append_table_row(&mut body, row);
                    push_chunk(&mut chunks, &headers, body);
                }
                intro = None;
            }
            MarkdownBlock::Code(code) => {
                let mut body = String::new();
                append_piece(&mut body, &code.info);
                append_piece(&mut body, &code.body);
                push_chunk(&mut chunks, &headers, body);
                intro = None;
            }
            MarkdownBlock::BlockQuote(text) => {
                push_chunk(&mut chunks, &headers, text.clone());
                intro = None;
            }
            MarkdownBlock::ThematicBreak | MarkdownBlock::HtmlBlock => {
                intro = None;
            }
        }
    }

    chunks
}

fn push_chunk(chunks: &mut Vec<RawChunk>, headers: &[Header], body: String) {
    if body.trim().is_empty() {
        return;
    }
    chunks.push(RawChunk {
        headers: headers.to_vec(),
        body,
    });
}

fn append_intro(body: &mut String, intro: Option<&str>) {
    if let Some(intro) = intro {
        append_piece(body, intro);
    }
}

fn append_piece(body: &mut String, piece: &str) {
    let trimmed = piece.trim();
    if trimmed.is_empty() {
        return;
    }
    if !body.is_empty() {
        body.push_str("\n\n");
    }
    body.push_str(trimmed);
}

fn append_table_row(body: &mut String, cells: &[String]) {
    if cells.is_empty() {
        return;
    }
    append_piece(body, &cells.join(" "));
}

fn is_definition_list(list: &ListBlock) -> bool {
    if list.items.len() < 2 {
        return false;
    }
    let matches = list
        .items
        .iter()
        .filter(|item| item.is_definition_item)
        .count();
    matches >= 2 && matches * 2 >= list.items.len()
}

fn render_chunk(chunk: &RawChunk) -> String {
    let mut markdown = String::new();
    for header in &chunk.headers {
        markdown.push_str(&"#".repeat(heading_rank(header.level)));
        markdown.push(' ');
        markdown.push_str(header.text.trim());
        markdown.push_str("\n\n");
    }
    markdown.push_str(chunk.body.trim());
    markdown
}

fn render_header_stub(headers: &[Header], original_size: usize) -> String {
    let mut parts: Vec<String> = headers
        .iter()
        .map(|header| {
            format!(
                "{} {}",
                "#".repeat(heading_rank(header.level)),
                header.text.trim()
            )
        })
        .collect();
    parts.push(format!(
        "\n[Content too large to index: {} chars]",
        format_usize_with_commas(original_size)
    ));
    parts.join("\n\n")
}

fn format_usize_with_commas(value: usize) -> String {
    let digits = value.to_string();
    let mut out = String::new();
    for (idx, ch) in digits.chars().rev().enumerate() {
        if idx > 0 && idx % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out.chars().rev().collect()
}

fn heading_rank(level: HeadingLevel) -> usize {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

enum ActiveBlock {
    None,
    Heading(HeadingBuilder),
    Paragraph(TextBlockBuilder),
    List(ListBuilder),
    Table(TableBuilder),
    Code(CodeBlockBuilder),
    BlockQuote(QuoteBuilder),
    HtmlBlock,
}

struct HeadingBuilder {
    level: HeadingLevel,
    text: TextCollector,
}

impl HeadingBuilder {
    fn new(level: HeadingLevel) -> Self {
        Self {
            level,
            text: TextCollector::default(),
        }
    }

    fn handle(&mut self, event: Event<'_>, blocks: &mut Vec<MarkdownBlock>) -> bool {
        match event {
            Event::End(TagEnd::Heading(_)) => {
                blocks.push(MarkdownBlock::Heading(Header {
                    level: self.level,
                    text: std::mem::take(&mut self.text).finish(),
                }));
                true
            }
            event => {
                self.text.handle_event(event);
                false
            }
        }
    }
}

#[derive(Default)]
struct TextBlockBuilder {
    text: TextCollector,
}

impl TextBlockBuilder {
    fn handle(&mut self, event: Event<'_>, blocks: &mut Vec<MarkdownBlock>) -> bool {
        match event {
            Event::End(TagEnd::Paragraph) => {
                blocks.push(MarkdownBlock::Paragraph(
                    std::mem::take(&mut self.text).finish(),
                ));
                true
            }
            Event::Start(Tag::CodeBlock(kind)) => {
                self.text.push_text(&code_info(&kind));
                false
            }
            event => {
                self.text.handle_event(event);
                false
            }
        }
    }
}

#[derive(Default)]
struct QuoteBuilder {
    text: TextCollector,
}

impl QuoteBuilder {
    fn handle(&mut self, event: Event<'_>, blocks: &mut Vec<MarkdownBlock>) -> bool {
        match event {
            Event::End(TagEnd::BlockQuote(_)) => {
                blocks.push(MarkdownBlock::BlockQuote(
                    std::mem::take(&mut self.text).finish(),
                ));
                true
            }
            Event::End(TagEnd::Paragraph) | Event::End(TagEnd::Item) => {
                self.text.push_text("\n");
                false
            }
            Event::Start(Tag::CodeBlock(kind)) => {
                self.text.push_text(&code_info(&kind));
                false
            }
            event => {
                self.text.handle_event(event);
                false
            }
        }
    }
}

struct CodeBlockBuilder {
    info: String,
    body: TextCollector,
}

impl CodeBlockBuilder {
    fn new(kind: CodeBlockKind<'_>) -> Self {
        Self {
            info: code_info(&kind),
            body: TextCollector::default(),
        }
    }

    fn handle(&mut self, event: Event<'_>, blocks: &mut Vec<MarkdownBlock>) -> bool {
        match event {
            Event::End(TagEnd::CodeBlock) => {
                blocks.push(MarkdownBlock::Code(CodeBlock {
                    info: self.info.trim().to_string(),
                    body: std::mem::take(&mut self.body).finish(),
                }));
                true
            }
            Event::Text(text) => {
                self.body.push_text(&text);
                false
            }
            Event::SoftBreak | Event::HardBreak => {
                self.body.push_text("\n");
                false
            }
            _ => false,
        }
    }
}

struct ListBuilder {
    depth: usize,
    items: Vec<ListItem>,
    current_item: Option<ListItemBuilder>,
}

impl ListBuilder {
    fn new() -> Self {
        Self {
            depth: 1,
            items: Vec::new(),
            current_item: None,
        }
    }

    fn handle(&mut self, event: Event<'_>, blocks: &mut Vec<MarkdownBlock>) -> bool {
        match event {
            Event::Start(Tag::List(_)) => {
                self.depth += 1;
                if let Some(item) = &mut self.current_item {
                    item.mark_complex();
                }
                false
            }
            Event::End(TagEnd::List(_)) => {
                self.depth -= 1;
                if self.depth == 0 {
                    blocks.push(MarkdownBlock::List(ListBlock {
                        items: std::mem::take(&mut self.items),
                    }));
                    true
                } else {
                    false
                }
            }
            Event::Start(Tag::Item) if self.depth == 1 => {
                self.current_item = Some(ListItemBuilder::default());
                false
            }
            Event::End(TagEnd::Item) if self.depth == 1 => {
                if let Some(item) = self.current_item.take() {
                    self.items.push(item.finish());
                }
                false
            }
            Event::Start(Tag::Item) => {
                if let Some(item) = &mut self.current_item {
                    item.mark_complex();
                    item.push_separator();
                }
                false
            }
            event => {
                if let Some(item) = &mut self.current_item {
                    item.handle_event(event);
                }
                false
            }
        }
    }
}

#[derive(Default)]
struct ListItemBuilder {
    text: TextCollector,
    block_count: usize,
    in_first_paragraph: bool,
    leading_strong: bool,
    in_leading_strong: bool,
    saw_text_before_strong: bool,
    strong_text: String,
    following_text: String,
    complex: bool,
}

impl ListItemBuilder {
    fn handle_event(&mut self, event: Event<'_>) {
        match event {
            Event::Start(Tag::Paragraph) => {
                self.block_count += 1;
                self.in_first_paragraph = self.block_count == 1;
            }
            Event::End(TagEnd::Paragraph) => {
                self.in_first_paragraph = false;
                self.push_separator();
            }
            Event::Start(Tag::CodeBlock(kind)) => {
                self.block_count += 1;
                self.mark_complex();
                self.push_text(&code_info(&kind));
            }
            Event::End(TagEnd::CodeBlock) => {
                self.push_separator();
            }
            Event::Start(Tag::Strong) => {
                self.ensure_text_block();
                if self.in_first_paragraph && !self.saw_text_before_strong && !self.leading_strong {
                    self.leading_strong = true;
                    self.in_leading_strong = true;
                }
            }
            Event::End(TagEnd::Strong) => {
                self.in_leading_strong = false;
            }
            Event::Text(text) | Event::Code(text) => {
                self.ensure_text_block();
                self.push_text(&text);
            }
            Event::InlineHtml(html) => {
                self.ensure_text_block();
                self.push_text(&html);
            }
            Event::SoftBreak | Event::HardBreak => {
                self.ensure_text_block();
                self.push_text("\n");
            }
            event => {
                self.text.handle_event(event);
            }
        }
    }

    fn push_text(&mut self, text: &str) {
        if self.in_first_paragraph {
            if self.in_leading_strong {
                self.strong_text.push_str(text);
            } else if self.leading_strong {
                self.following_text.push_str(text);
            } else if !text.trim().is_empty() {
                self.saw_text_before_strong = true;
            }
        }
        self.text.push_text(text);
    }

    fn ensure_text_block(&mut self) {
        if self.block_count == 0 {
            self.block_count = 1;
            self.in_first_paragraph = true;
        }
    }

    fn push_separator(&mut self) {
        self.text.push_text("\n");
    }

    fn mark_complex(&mut self) {
        self.complex = true;
    }

    fn finish(self) -> ListItem {
        let text = self.text.finish();
        let strong_has_colon = self.strong_text.trim_end().ends_with(':')
            || self.following_text.trim_start().starts_with(':');
        let is_definition_item = !self.complex
            && self.block_count == 1
            && self.leading_strong
            && strong_has_colon
            && !text.trim().ends_with('.');
        ListItem {
            text,
            is_definition_item,
        }
    }
}

#[derive(Default)]
struct TableBuilder {
    headers: Vec<String>,
    rows: Vec<Vec<String>>,
    in_head: bool,
    current_row: Option<Vec<String>>,
    current_cell: Option<TextCollector>,
}

impl TableBuilder {
    fn handle(&mut self, event: Event<'_>, blocks: &mut Vec<MarkdownBlock>) -> bool {
        match event {
            Event::Start(Tag::TableHead) => {
                self.in_head = true;
                false
            }
            Event::End(TagEnd::TableHead) => {
                self.in_head = false;
                false
            }
            Event::Start(Tag::TableRow) => {
                self.current_row = Some(Vec::new());
                false
            }
            Event::End(TagEnd::TableRow) => {
                if let Some(row) = self.current_row.take() {
                    if self.in_head {
                        self.headers = row;
                    } else {
                        self.rows.push(row);
                    }
                }
                false
            }
            Event::Start(Tag::TableCell) => {
                self.current_cell = Some(TextCollector::default());
                false
            }
            Event::End(TagEnd::TableCell) => {
                if let Some(cell) = self.current_cell.take() {
                    let text = cell.finish();
                    if let Some(row) = &mut self.current_row {
                        row.push(text);
                    } else if self.in_head {
                        self.headers.push(text);
                    }
                }
                false
            }
            Event::End(TagEnd::Table) => {
                blocks.push(MarkdownBlock::Table(TableBlock {
                    headers: std::mem::take(&mut self.headers),
                    rows: std::mem::take(&mut self.rows),
                }));
                true
            }
            event => {
                if let Some(cell) = &mut self.current_cell {
                    cell.handle_event(event);
                }
                false
            }
        }
    }
}

#[derive(Default)]
struct TextCollector {
    text: String,
    links: Vec<LinkContext>,
}

impl TextCollector {
    fn handle_event(&mut self, event: Event<'_>) {
        match event {
            Event::Text(text) | Event::Code(text) => self.push_text(&text),
            Event::SoftBreak | Event::HardBreak => self.push_text("\n"),
            Event::InlineHtml(html) => self.push_text(&html),
            Event::Start(Tag::Link {
                link_type,
                dest_url,
                title,
                id,
            }) => self
                .links
                .push(LinkContext::new(link_type, dest_url, title, id)),
            Event::Start(Tag::Image {
                link_type,
                dest_url,
                title,
                id,
            }) => self
                .links
                .push(LinkContext::new(link_type, dest_url, title, id)),
            Event::End(TagEnd::Link | TagEnd::Image) => {
                if let Some(link) = self.links.pop() {
                    for extra in link.extra_text() {
                        self.push_text(&extra);
                    }
                }
            }
            _ => {}
        }
    }

    fn push_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        if !self.text.is_empty()
            && !self.text.ends_with(char::is_whitespace)
            && !text.starts_with(char::is_whitespace)
        {
            self.text.push(' ');
        }
        self.text.push_str(text);
    }

    fn finish(self) -> String {
        self.text.trim().to_string()
    }
}

struct LinkContext {
    link_type: LinkType,
    dest_url: String,
    title: String,
    id: String,
}

impl LinkContext {
    fn new(
        link_type: LinkType,
        dest_url: impl ToString,
        title: impl ToString,
        id: impl ToString,
    ) -> Self {
        Self {
            link_type,
            dest_url: dest_url.to_string(),
            title: title.to_string(),
            id: id.to_string(),
        }
    }

    fn extra_text(&self) -> Vec<String> {
        match self.link_type {
            LinkType::Inline => [self.dest_url.trim(), self.title.trim()]
                .into_iter()
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .collect(),
            LinkType::Reference => {
                if self.id.trim().is_empty() {
                    Vec::new()
                } else {
                    vec![self.id.trim().to_string()]
                }
            }
            LinkType::Autolink | LinkType::Email => Vec::new(),
            _ => Vec::new(),
        }
    }
}

fn code_info(kind: &CodeBlockKind<'_>) -> String {
    match kind {
        CodeBlockKind::Fenced(info) => info.trim().to_string(),
        CodeBlockKind::Indented => String::new(),
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use serde_json::Value;

    const MARKDOWN_FIXTURE: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/markdown_chunks.json"
    ));
    pub(crate) const OVERSIZED_SIZE_NORMALIZATION: &str = "oversized_size";
    pub(crate) const OVERSIZED_SIZE_TOKEN: &str = "normalizedsize";

    pub(crate) fn markdown_fixture() -> Value {
        serde_json::from_str(MARKDOWN_FIXTURE).expect("parse markdown chunks fixture")
    }

    pub(crate) fn token_comparison_enabled(case: &Value) -> bool {
        case.get("token_comparison")
            .and_then(Value::as_bool)
            .unwrap_or(true)
    }

    pub(crate) fn strings(value: &Value) -> Vec<String> {
        value
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .map(|item| item.as_str().expect("string item").to_string())
                    .collect()
            })
            .unwrap_or_default()
    }

    pub(crate) fn rust_tokenize(text: &str) -> Vec<String> {
        text.split(|ch: char| !ch.is_ascii_alphanumeric())
            .filter(|token| !token.is_empty())
            .map(|token| token.to_ascii_lowercase())
            .collect()
    }

    pub(crate) fn normalize_tokens(tokens: Vec<String>, normalizations: &[String]) -> Vec<String> {
        if normalizations
            .iter()
            .any(|normalization| normalization == OVERSIZED_SIZE_NORMALIZATION)
        {
            normalize_oversized_size_tokens(tokens)
        } else {
            tokens
        }
    }

    fn normalize_oversized_size_tokens(tokens: Vec<String>) -> Vec<String> {
        let mut normalized = Vec::new();
        let mut i = 0;
        while i < tokens.len() {
            if i + 5 < tokens.len()
                && tokens[i..i + 5] == ["content", "too", "large", "to", "index"]
            {
                normalized.extend_from_slice(&tokens[i..i + 5]);
                let mut j = i + 5;
                while j < tokens.len() && tokens[j] != "chars" {
                    j += 1;
                }
                if j < tokens.len() {
                    normalized.push(OVERSIZED_SIZE_TOKEN.to_string());
                    normalized.push("chars".to_string());
                    i = j + 1;
                    continue;
                }
            }
            normalized.push(tokens[i].clone());
            i += 1;
        }
        normalized
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::{
        markdown_fixture, normalize_tokens, rust_tokenize, strings, token_comparison_enabled,
    };
    use super::*;

    #[test]
    fn markdown_chunks_match_python_oracle_tokens() {
        let fixture = markdown_fixture();
        for case in fixture["cases"].as_array().expect("fixture cases") {
            let id = case["id"].as_str().expect("case id");
            let input = case["input"].as_str().expect("case input");
            let formatted = format_markdown(input);
            let expected_warnings = strings(&case["warnings"]);
            assert_eq!(formatted.warnings, expected_warnings, "{id} warnings");
            assert_eq!(
                formatted.chunks.len(),
                case["chunk_count"].as_u64().expect("chunk count") as usize,
                "{id} chunk count"
            );
            if !token_comparison_enabled(case) {
                continue;
            }

            for (idx, expected_chunk) in case["chunks"]
                .as_array()
                .expect("case chunks")
                .iter()
                .enumerate()
            {
                let normalizations = strings(&expected_chunk["normalizations"]);
                let recorded_tokens = strings(&expected_chunk["tokens"]);
                let python_markdown = expected_chunk["markdown"]
                    .as_str()
                    .expect("python rendered markdown");
                assert_eq!(
                    normalize_tokens(rust_tokenize(python_markdown), &normalizations),
                    recorded_tokens,
                    "{id}:{idx} fixture tokenizer"
                );
                assert_eq!(
                    normalize_tokens(
                        rust_tokenize(&formatted.chunks[idx].markdown),
                        &normalizations
                    ),
                    recorded_tokens,
                    "{id}:{idx} native tokens; native chunk {:?}",
                    formatted.chunks[idx].markdown
                );
            }
        }
    }

    #[test]
    fn empty_context_only_inputs_produce_no_chunks() {
        for input in ["", "  \n", "# Heading\n", "---\n", "| A |\n| --- |\n"] {
            assert!(format_markdown(input).chunks.is_empty(), "{input:?}");
        }
    }

    #[test]
    fn overlong_lines_are_dropped_with_warning() {
        let input = format!("# Long\n\n{}\n\nkept alpha", "z".repeat(MAX_LINE_CHARS + 1));
        let formatted = format_markdown(&input);
        assert_eq!(
            formatted.warnings,
            vec!["Dropped 1 line(s) exceeding 2048 chars during markdown sanitization"]
        );
        assert_eq!(formatted.chunks.len(), 1);
        assert!(formatted.chunks[0].markdown.contains("kept alpha"));
        assert!(!formatted.chunks[0].markdown.contains('z'));
    }

    #[test]
    fn non_ascii_bounds_count_characters_not_utf8_bytes() {
        let under_line_bound = format!("# Accent\n\n{}\n", "é".repeat(MAX_LINE_CHARS - 1));
        let formatted = format_markdown(&under_line_bound);
        assert!(formatted.warnings.is_empty());
        assert_eq!(formatted.chunks.len(), 1);
        assert!(formatted.chunks[0].markdown.contains('é'));

        let over_line_bound = format!(
            "# Accent\n\n{}\n\nkept alpha\n",
            "é".repeat(MAX_LINE_CHARS + 1)
        );
        let formatted = format_markdown(&over_line_bound);
        assert_eq!(
            formatted.warnings,
            vec!["Dropped 1 line(s) exceeding 2048 chars during markdown sanitization"]
        );
        assert_eq!(formatted.chunks.len(), 1);
        assert!(formatted.chunks[0].markdown.contains("kept alpha"));
        assert!(!formatted.chunks[0].markdown.contains('é'));

        let multi_line = ["é".repeat(1300), "é".repeat(1300), "é".repeat(1300)].join("\n");
        let formatted = format_markdown(&format!("# Accent\n\n{multi_line}\n"));
        assert!(formatted.warnings.is_empty());
        assert_eq!(formatted.chunks.len(), 1);
        assert!(formatted.chunks[0].markdown.len() > MAX_CHUNK_CHARS);
        assert!(formatted.chunks[0].markdown.chars().count() < MAX_CHUNK_CHARS);
        assert!(
            !formatted.chunks[0]
                .markdown
                .contains("[Content too large to index:")
        );
    }

    #[test]
    fn oversized_chunks_become_header_stub() {
        let oversized_line = "alpha ".repeat(300);
        let input = format!(
            "# Big\n\n{}\n{}\n{}",
            oversized_line, oversized_line, oversized_line
        );
        let formatted = format_markdown(&input);
        assert_eq!(formatted.chunks.len(), 1);
        assert!(formatted.chunks[0].markdown.starts_with("# Big\n\n"));
        assert!(
            formatted.chunks[0]
                .markdown
                .contains("[Content too large to index:")
        );
        assert!(!formatted.chunks[0].markdown.contains("alpha alpha"));
    }
}
