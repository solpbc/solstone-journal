// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Streaming pure-Rust OpenDocument reader (`.odt`, `.ods`, `.odp`).
//!
//! Security and data-integrity invariants:
//! - Checks all case/separator variants of `META-INF/manifest.xml` for
//!   `<manifest:encryption-data>` before reading `content.xml`, returning
//!   `BackendOutcome::Encrypted` on encrypted ODF archives.
//! - Opens ONLY exact root `"content.xml"` (ignoring embedded `Object N/content.xml`).
//! - Uses a streaming `quick-xml::Reader` with `MAX_XML_DEPTH = 256` (no full DOM).
//! - Skips `<text:tracked-changes>` (omitting deleted revisions), styles, font
//!   declarations, scripts, and embedded objects.
//! - Defeats ODF repeat bombs (e.g. `number-rows-repeated="1048576"` and
//!   `number-columns-repeated="16384"`, including non-trailing sparse repeats)
//!   by discarding empty trailing repeats, capping per-row columns at
//!   `MAX_ODF_COLS`, failing fast with `ExtractError::LimitExceeded` on
//!   explosive non-empty repeat attributes, and tracking `<text:s>` space
//!   expansion against `limits.max_expanded_text_chars`.
//! - Supports nested tables in ODT via an explicit table stack.

use std::io::Cursor;

use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};

use crate::sniff::{
    ZipBudget, ZipReadOutcome, find_all_zip_entries_case_insensitive, find_exact_zip_entry,
};
use crate::{BackendOutcome, DocumentFormat, ExtractError, ExtractLimits, RawExtraction};

const MAX_XML_DEPTH: usize = 256;
const MAX_ODF_COLS: usize = 256;
const MAX_TEXT_SPACE_RUN: usize = 256;

fn max_odf_cells(limits: &ExtractLimits) -> usize {
    if limits.max_memory_bytes <= 24 * 1024 * 1024 {
        40_000
    } else {
        100_000
    }
}

pub(crate) fn extract_odf(
    bytes: &[u8],
    format: DocumentFormat,
    limits: &ExtractLimits,
) -> Result<BackendOutcome, ExtractError> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|err| ExtractError::Invalid(format!("invalid OpenDocument archive: {err}")))?;
    let mut budget = ZipBudget::new();

    // 1. Check all case/separator variants of META-INF/manifest.xml for encryption-data.
    for idx in find_all_zip_entries_case_insensitive(&archive, "meta-inf/manifest.xml") {
        let manifest_bytes = match budget.read_entry_bounded(&mut archive, idx, limits)? {
            ZipReadOutcome::Encrypted => return Ok(BackendOutcome::Encrypted),
            ZipReadOutcome::Bytes(data) => data,
        };
        if manifest_has_encryption(&manifest_bytes)? {
            return Ok(BackendOutcome::Encrypted);
        }
    }

    // 2. Optional root meta.xml for <dc:title>.
    let title = if let Some(meta_idx) = find_exact_zip_entry(&archive, "meta.xml") {
        match budget.read_entry_bounded(&mut archive, meta_idx, limits)? {
            ZipReadOutcome::Encrypted => return Ok(BackendOutcome::Encrypted),
            ZipReadOutcome::Bytes(meta_bytes) => parse_meta_title(&meta_bytes),
        }
    } else {
        None
    };

    // 3. Open ONLY exact root "content.xml" (never embedded Object N/content.xml).
    let content_idx = find_exact_zip_entry(&archive, "content.xml").ok_or_else(|| {
        ExtractError::Invalid("OpenDocument archive is missing root content.xml".to_owned())
    })?;
    let content_bytes = match budget.read_entry_bounded(&mut archive, content_idx, limits)? {
        ZipReadOutcome::Encrypted => return Ok(BackendOutcome::Encrypted),
        ZipReadOutcome::Bytes(data) => data,
    };

    let (mut markdown, mut plain_text, truncated, warnings) =
        parse_odf_content(&content_bytes, format, limits)?;

    // 4. Optional root styles.xml: extract static authored headers/footers from
    //    <office:master-styles>/<style:master-page> (skipping placeholder frames
    //    and dynamic field-template paragraphs like "Page 1 / 99").
    if let Some(styles_idx) = find_exact_zip_entry(&archive, "styles.xml") {
        match budget.read_entry_bounded(&mut archive, styles_idx, limits)? {
            ZipReadOutcome::Encrypted => return Ok(BackendOutcome::Encrypted),
            ZipReadOutcome::Bytes(styles_bytes) => {
                let master_blocks = parse_odf_master_styles(&styles_bytes, limits)?;
                if !master_blocks.is_empty() {
                    let extra = master_blocks.join("\n\n");
                    if markdown.trim().is_empty() {
                        markdown = extra.clone();
                    } else {
                        markdown.push_str("\n\n");
                        markdown.push_str(&extra);
                    }
                    if plain_text.trim().is_empty() {
                        plain_text = extra;
                    } else {
                        plain_text.push_str("\n\n");
                        plain_text.push_str(&extra);
                    }
                }
            }
        }
    }

    Ok(BackendOutcome::Extracted(RawExtraction {
        format,
        title,
        markdown,
        plain_text,
        truncated,
        warnings,
    }))
}

fn manifest_has_encryption(xml_bytes: &[u8]) -> Result<bool, ExtractError> {
    let mut reader = Reader::from_reader(Cursor::new(xml_bytes));
    let mut buf = Vec::new();
    let mut depth = 0usize;
    loop {
        buf.clear();
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                depth += 1;
                if depth > MAX_XML_DEPTH {
                    return Err(ExtractError::LimitExceeded(
                        "OpenDocument manifest.xml exceeds XML nesting depth limit".to_owned(),
                    ));
                }
                if local_name_eq(e.name().as_ref(), b"encryption-data") {
                    return Ok(true);
                }
            }
            Ok(Event::Empty(e)) => {
                if local_name_eq(e.name().as_ref(), b"encryption-data") {
                    return Ok(true);
                }
            }
            Ok(Event::End(_)) => depth = depth.saturating_sub(1),
            Ok(Event::Eof) => break,
            Err(err) => {
                return Err(ExtractError::Invalid(format!(
                    "malformed OpenDocument manifest.xml: {err}"
                )));
            }
            _ => {}
        }
    }
    Ok(false)
}

fn parse_meta_title(xml_bytes: &[u8]) -> Option<String> {
    let mut reader = Reader::from_reader(Cursor::new(xml_bytes));
    let mut buf = Vec::new();
    let mut depth = 0usize;
    let mut in_title = false;
    let mut title_buf = String::new();
    loop {
        buf.clear();
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                depth += 1;
                if depth > MAX_XML_DEPTH {
                    return None;
                }
                if local_name_eq(e.name().as_ref(), b"title") {
                    in_title = true;
                }
            }
            Ok(Event::Text(t)) if in_title && title_buf.len() < 1024 => {
                if let Ok(s) = t.decode() {
                    title_buf.push_str(&s);
                }
            }
            Ok(Event::GeneralRef(r)) if in_title && title_buf.len() < 1024 => {
                push_xml_entity(&mut title_buf, &r);
            }
            Ok(Event::End(e)) => {
                depth = depth.saturating_sub(1);
                if local_name_eq(e.name().as_ref(), b"title") && in_title {
                    let clean = title_buf.split_whitespace().collect::<Vec<_>>().join(" ");
                    return (!clean.is_empty()).then_some(clean);
                }
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    None
}

struct TableState {
    name: Option<String>,
    rows: Vec<Vec<String>>,
    current_row: Option<RowState>,
    truncated_cols: bool,
    truncated_rows: bool,
}

struct RowState {
    repeat: u64,
    cells: Vec<String>,
    current_cell: Option<CellState>,
    pending_empty_col_overflow: bool,
    truncated_cols: bool,
}

struct CellState {
    repeat: u64,
    paragraphs: Vec<String>,
}

struct OdfParser<'a> {
    format: DocumentFormat,
    limits: &'a ExtractLimits,
    max_cells: usize,
    total_cells: usize,
    expanded_chars: usize,
    md_blocks: Vec<String>,
    plain_blocks: Vec<String>,
    warnings: Vec<String>,
    // Current text block (<text:p> or <text:h>).
    in_block: bool,
    block_heading_level: Option<usize>,
    block_buf: String,
    // List nesting for ODT/ODP.
    list_depth: usize,
    list_item_first_para: Vec<bool>,
    // Table stack (supports nested ODT tables cleanly).
    tables: Vec<TableState>,
    // ODP slide / notes state.
    slide_index: usize,
    pending_slide_heading: Option<(String, String)>,
    in_notes: bool,
    notes_blocks: Vec<String>,
    // Subtree skip counter for tracked-changes, styles, embedded objects, etc.
    skip_depth: usize,
    // Stop cleanly when sequential content already exceeds cell/output cap.
    stopped_early: bool,
    any_truncated: bool,
}

impl<'a> OdfParser<'a> {
    fn new(format: DocumentFormat, limits: &'a ExtractLimits) -> Self {
        Self {
            format,
            limits,
            max_cells: max_odf_cells(limits),
            total_cells: 0,
            expanded_chars: 0,
            md_blocks: Vec::new(),
            plain_blocks: Vec::new(),
            warnings: Vec::new(),
            in_block: false,
            block_heading_level: None,
            block_buf: String::new(),
            list_depth: 0,
            list_item_first_para: Vec::new(),
            tables: Vec::new(),
            slide_index: 0,
            pending_slide_heading: None,
            in_notes: false,
            notes_blocks: Vec::new(),
            skip_depth: 0,
            stopped_early: false,
            any_truncated: false,
        }
    }

    fn charge_expanded_chars(&mut self, count: usize) -> Result<(), ExtractError> {
        self.expanded_chars = self.expanded_chars.saturating_add(count);
        if self.expanded_chars > self.limits.max_expanded_text_chars {
            return Err(ExtractError::LimitExceeded(format!(
                "OpenDocument expanded text exceeded limit of {} characters",
                self.limits.max_expanded_text_chars
            )));
        }
        Ok(())
    }

    fn push_text(&mut self, text: &str) -> Result<(), ExtractError> {
        if !self.in_block || text.is_empty() {
            return Ok(());
        }
        self.charge_expanded_chars(text.chars().count())?;
        self.block_buf.push_str(text);
        Ok(())
    }

    fn flush_pending_slide_heading(&mut self) {
        if let Some((h_md, h_plain)) = self.pending_slide_heading.take() {
            self.md_blocks.push(h_md);
            self.plain_blocks.push(h_plain);
        }
    }

    fn emit_block(&mut self, md: String, plain: String) {
        if md.trim().is_empty() && plain.trim().is_empty() {
            return;
        }
        if self.in_notes {
            self.notes_blocks.push(plain.trim().to_owned());
        } else {
            self.flush_pending_slide_heading();
            self.md_blocks.push(md);
            self.plain_blocks.push(plain);
        }
    }

    fn flush_block(&mut self) {
        if !self.in_block {
            return;
        }
        self.in_block = false;
        let heading = self.block_heading_level.take();
        let raw = std::mem::take(&mut self.block_buf);
        let text = normalize_odf_paragraph(&raw);
        if text.is_empty() {
            return;
        }

        // If inside a table cell, append to that cell's paragraphs.
        if let Some(table) = self.tables.last_mut()
            && let Some(row) = table.current_row.as_mut()
            && let Some(cell) = row.current_cell.as_mut()
        {
            cell.paragraphs.push(text);
            return;
        }

        if let Some(level) = heading {
            let clamped = level.clamp(1, 6);
            let hashes = "#".repeat(clamped);
            self.emit_block(format!("{hashes} {text}"), text);
            return;
        }

        if let Some(is_first) = self.list_item_first_para.last_mut() {
            let indent = "  ".repeat(self.list_depth.saturating_sub(1));
            if *is_first {
                *is_first = false;
                self.emit_block(format!("{indent}- {text}"), text);
            } else {
                self.emit_block(format!("{indent}  {text}"), text);
            }
            return;
        }

        self.emit_block(text.clone(), text);
    }

    fn finish_cell(&mut self) -> Result<(), ExtractError> {
        let cell = {
            let Some(table) = self.tables.last_mut() else {
                return Ok(());
            };
            let Some(row) = table.current_row.as_mut() else {
                return Ok(());
            };
            let Some(cell) = row.current_cell.take() else {
                return Ok(());
            };
            cell
        };

        let joined = cell.paragraphs.join(" ");
        let trimmed = joined.trim().to_owned();
        let repeat = cell.repeat.max(1);

        if trimmed.is_empty() {
            // Bound non-trailing sparse empty repeats to MAX_ODF_COLS so a row
            // like `<cell repeat="16384"/><cell>X</cell>` never allocates 16k
            // empty columns. Only mark `truncated_cols` if a non-empty cell
            // actually follows (LibreOffice Calc emits trailing empty repeats
            // at the end of every normal row).
            if let Some(table) = self.tables.last_mut()
                && let Some(row) = table.current_row.as_mut()
            {
                let remaining_cols = MAX_ODF_COLS.saturating_sub(row.cells.len());
                let repeat_usize = usize::try_from(repeat).unwrap_or(usize::MAX);
                if repeat_usize > remaining_cols {
                    row.pending_empty_col_overflow = true;
                }
                let to_push = repeat_usize.min(remaining_cols);
                for _ in 0..to_push {
                    row.cells.push(String::new());
                }
            }
            return Ok(());
        }

        // Non-empty cell with explosive repeat attribute fails fast as a bomb.
        let repeat_usize = usize::try_from(repeat).unwrap_or(usize::MAX);
        if repeat > 1
            && (repeat_usize > MAX_ODF_COLS
                || self.total_cells.saturating_add(repeat_usize) > self.max_cells)
        {
            return Err(ExtractError::LimitExceeded(format!(
                "OpenDocument cell repeat count {repeat} exceeds cell limit"
            )));
        }
        let char_expansion = trimmed
            .chars()
            .count()
            .saturating_mul(repeat_usize.saturating_sub(1));
        if char_expansion > 0 {
            self.charge_expanded_chars(char_expansion)?;
        }

        if let Some(table) = self.tables.last_mut()
            && let Some(row) = table.current_row.as_mut()
        {
            if row.pending_empty_col_overflow {
                row.truncated_cols = true;
            }
            for _ in 0..repeat_usize {
                if row.cells.len() >= MAX_ODF_COLS {
                    row.truncated_cols = true;
                    break;
                }
                row.cells.push(trimmed.clone());
            }
        }
        Ok(())
    }

    fn finish_row(&mut self) -> Result<(), ExtractError> {
        let mut row = {
            let Some(table) = self.tables.last_mut() else {
                return Ok(());
            };
            let Some(row) = table.current_row.take() else {
                return Ok(());
            };
            if row.truncated_cols {
                table.truncated_cols = true;
            }
            row
        };
        // Discard trailing empty cells.
        while row.cells.last().is_some_and(String::is_empty) {
            row.cells.pop();
        }
        // Discard completely empty rows immediately (defeats trailing
        // `number-rows-repeated="1048576"`).
        if row.cells.is_empty() {
            return Ok(());
        }

        let repeat = row.repeat.max(1);
        let cols = row.cells.len();
        let row_chars: usize = row.cells.iter().map(|c| c.chars().count()).sum();
        let repeat_usize = usize::try_from(repeat).unwrap_or(usize::MAX);
        let cells_needed = repeat_usize.saturating_mul(cols);

        if repeat > 1 && self.total_cells.saturating_add(cells_needed) > self.max_cells {
            return Err(ExtractError::LimitExceeded(format!(
                "OpenDocument row repeat count {repeat} exceeds cell budget of {}",
                self.max_cells
            )));
        }
        if repeat > 1 {
            let extra_chars = row_chars.saturating_mul(repeat_usize.saturating_sub(1));
            self.charge_expanded_chars(extra_chars)?;
        }

        // Legitimate sequential rows that reach max_cells truncate cleanly.
        if self.total_cells.saturating_add(cells_needed) > self.max_cells {
            if let Some(table) = self.tables.last_mut() {
                table.truncated_rows = true;
            }
            self.stopped_early = true;
            return Ok(());
        }

        self.total_cells = self.total_cells.saturating_add(cells_needed);
        if let Some(table) = self.tables.last_mut() {
            for _ in 0..repeat_usize {
                table.rows.push(row.cells.clone());
            }
        }
        Ok(())
    }

    fn finish_table(&mut self) {
        let Some(table) = self.tables.pop() else {
            return;
        };
        if table.truncated_cols {
            self.any_truncated = true;
            let msg = format!("OpenDocument table columns truncated to {MAX_ODF_COLS}");
            if !self.warnings.contains(&msg) {
                self.warnings.push(msg);
            }
        }
        if table.truncated_rows {
            self.any_truncated = true;
            let msg = format!(
                "OpenDocument table rows truncated at {} cell limit",
                self.max_cells
            );
            if !self.warnings.contains(&msg) {
                self.warnings.push(msg);
            }
        }
        if table.rows.is_empty() {
            return;
        }

        let (md_table, plain_table) = render_gfm_table(&table.rows);

        // If this was a nested table inside an outer table cell, fold its text
        // into the enclosing cell rather than corrupting outer table structure.
        if let Some(outer) = self.tables.last_mut()
            && let Some(row) = outer.current_row.as_mut()
            && let Some(cell) = row.current_cell.as_mut()
        {
            cell.paragraphs.push(plain_table.replace('\n', "; "));
            return;
        }

        if self.format == DocumentFormat::Ods
            && let Some(name) = table.name.filter(|n| !n.trim().is_empty())
        {
            let heading = format!("## {}", name.trim());
            self.emit_block(
                format!("{heading}\n\n{md_table}"),
                format!("{}\n{plain_table}", name.trim()),
            );
        } else {
            self.emit_block(md_table, plain_table);
        }
    }

    fn finish_slide(&mut self) {
        if !self.notes_blocks.is_empty() {
            self.flush_pending_slide_heading();
            let notes = std::mem::take(&mut self.notes_blocks).join("\n");
            let quoted = notes
                .lines()
                .map(|l| format!("> {l}"))
                .collect::<Vec<_>>()
                .join("\n");
            self.md_blocks.push(format!("> **Notes:**\n{quoted}"));
            self.plain_blocks.push(format!("Notes: {notes}"));
        }
        self.pending_slide_heading = None;
    }
}

fn parse_odf_content(
    xml_bytes: &[u8],
    format: DocumentFormat,
    limits: &ExtractLimits,
) -> Result<(String, String, bool, Vec<String>), ExtractError> {
    let mut reader = Reader::from_reader(Cursor::new(xml_bytes));
    let mut buf = Vec::new();
    let mut depth = 0usize;
    let mut parser = OdfParser::new(format, limits);

    loop {
        buf.clear();
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                depth += 1;
                if depth > MAX_XML_DEPTH {
                    return Err(ExtractError::LimitExceeded(
                        "OpenDocument content.xml exceeds XML nesting depth limit".to_owned(),
                    ));
                }
                if parser.skip_depth > 0 {
                    parser.skip_depth += 1;
                    continue;
                }
                let local = local_name_bytes(e.name().as_ref()).to_vec();
                if should_skip_subtree(&local) {
                    parser.skip_depth = 1;
                    continue;
                }
                if parser.stopped_early {
                    continue;
                }
                handle_start_element(&mut parser, &e, &local, false)?;
            }
            Ok(Event::Empty(e)) => {
                if depth + 1 > MAX_XML_DEPTH {
                    return Err(ExtractError::LimitExceeded(
                        "OpenDocument content.xml exceeds XML nesting depth limit".to_owned(),
                    ));
                }
                if parser.skip_depth > 0 || parser.stopped_early {
                    continue;
                }
                let local = local_name_bytes(e.name().as_ref()).to_vec();
                if should_skip_subtree(&local) {
                    continue;
                }
                handle_start_element(&mut parser, &e, &local, true)?;
            }
            Ok(Event::Text(t)) => {
                if parser.skip_depth == 0
                    && !parser.stopped_early
                    && let Ok(text) = t.decode()
                {
                    parser.push_text(&text)?;
                }
            }
            Ok(Event::CData(t)) => {
                if parser.skip_depth == 0 && !parser.stopped_early {
                    let text = String::from_utf8_lossy(t.as_ref());
                    parser.push_text(&text)?;
                }
            }
            Ok(Event::GeneralRef(r)) => {
                if parser.skip_depth == 0 && !parser.stopped_early && parser.in_block {
                    let mut entity_text = String::new();
                    push_xml_entity(&mut entity_text, &r);
                    parser.push_text(&entity_text)?;
                }
            }
            Ok(Event::End(e)) => {
                depth = depth.saturating_sub(1);
                if parser.skip_depth > 0 {
                    parser.skip_depth -= 1;
                    continue;
                }
                let local = local_name_bytes(e.name().as_ref()).to_vec();
                handle_end_element(&mut parser, &local)?;
            }
            Ok(Event::Eof) => break,
            Err(err) => {
                return Err(ExtractError::Invalid(format!(
                    "malformed OpenDocument content.xml: {err}"
                )));
            }
            _ => {}
        }
    }

    // Close any unclosed tables/blocks cleanly.
    parser.flush_block();
    while !parser.tables.is_empty() {
        parser.finish_table();
    }
    parser.finish_slide();

    Ok((
        parser.md_blocks.join("\n\n"),
        parser.plain_blocks.join("\n\n"),
        parser.any_truncated,
        parser.warnings,
    ))
}

fn parse_odf_master_styles(
    xml_bytes: &[u8],
    limits: &ExtractLimits,
) -> Result<Vec<String>, ExtractError> {
    let mut reader = Reader::from_reader(Cursor::new(xml_bytes));
    let mut buf = Vec::new();
    let mut depth = 0usize;
    let mut master_styles_depth: Option<usize> = None;
    let mut master_page_depth: Option<usize> = None;
    let mut skip_depth = 0usize;
    let mut in_para = false;
    let mut para_has_dynamic_field = false;
    let mut para_buf = String::new();
    let mut total_chars = 0usize;
    let mut blocks: Vec<String> = Vec::new();

    loop {
        buf.clear();
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                depth += 1;
                if depth > MAX_XML_DEPTH {
                    return Err(ExtractError::LimitExceeded(
                        "OpenDocument styles.xml exceeds XML nesting depth limit".to_owned(),
                    ));
                }
                if skip_depth > 0 {
                    skip_depth += 1;
                    continue;
                }
                let local = local_name_bytes(e.name().as_ref()).to_vec();
                if master_styles_depth.is_none() {
                    if local.eq_ignore_ascii_case(b"master-styles") {
                        master_styles_depth = Some(depth);
                    }
                    continue;
                }
                if master_page_depth.is_none() {
                    if local.eq_ignore_ascii_case(b"master-page") {
                        master_page_depth = Some(depth);
                    }
                    continue;
                }
                if should_skip_master_subtree(&e, &local) {
                    skip_depth = 1;
                    continue;
                }
                if in_para && is_dynamic_master_field(&local) {
                    para_has_dynamic_field = true;
                    continue;
                }
                if local.eq_ignore_ascii_case(b"p") || local.eq_ignore_ascii_case(b"h") {
                    in_para = true;
                    para_has_dynamic_field = false;
                    para_buf.clear();
                } else if local.eq_ignore_ascii_case(b"s") && in_para {
                    let count = attr_u64_local(&e, b"c")
                        .unwrap_or(1)
                        .min(MAX_TEXT_SPACE_RUN as u64) as usize;
                    total_chars = total_chars.saturating_add(count);
                    if total_chars > limits.max_expanded_text_chars {
                        return Err(ExtractError::LimitExceeded(format!(
                            "OpenDocument styles expanded text exceeded limit of {} characters",
                            limits.max_expanded_text_chars
                        )));
                    }
                    for _ in 0..count {
                        para_buf.push(' ');
                    }
                } else if local.eq_ignore_ascii_case(b"tab") && in_para {
                    para_buf.push('\t');
                } else if local.eq_ignore_ascii_case(b"line-break") && in_para {
                    para_buf.push('\n');
                }
            }
            Ok(Event::Empty(e)) => {
                if depth + 1 > MAX_XML_DEPTH {
                    return Err(ExtractError::LimitExceeded(
                        "OpenDocument styles.xml exceeds XML nesting depth limit".to_owned(),
                    ));
                }
                if skip_depth > 0 || master_page_depth.is_none() {
                    continue;
                }
                let local = local_name_bytes(e.name().as_ref()).to_vec();
                if is_dynamic_master_field(&local) && in_para {
                    para_has_dynamic_field = true;
                } else if local.eq_ignore_ascii_case(b"s") && in_para {
                    let count = attr_u64_local(&e, b"c")
                        .unwrap_or(1)
                        .min(MAX_TEXT_SPACE_RUN as u64) as usize;
                    total_chars = total_chars.saturating_add(count);
                    if total_chars > limits.max_expanded_text_chars {
                        return Err(ExtractError::LimitExceeded(format!(
                            "OpenDocument styles expanded text exceeded limit of {} characters",
                            limits.max_expanded_text_chars
                        )));
                    }
                    for _ in 0..count {
                        para_buf.push(' ');
                    }
                } else if local.eq_ignore_ascii_case(b"tab") && in_para {
                    para_buf.push('\t');
                } else if local.eq_ignore_ascii_case(b"line-break") && in_para {
                    para_buf.push('\n');
                }
            }
            Ok(Event::Text(t)) => {
                if skip_depth == 0
                    && in_para
                    && !para_has_dynamic_field
                    && let Ok(text) = t.decode()
                {
                    total_chars = total_chars.saturating_add(text.chars().count());
                    if total_chars > limits.max_expanded_text_chars {
                        return Err(ExtractError::LimitExceeded(format!(
                            "OpenDocument styles expanded text exceeded limit of {} characters",
                            limits.max_expanded_text_chars
                        )));
                    }
                    para_buf.push_str(&text);
                }
            }
            Ok(Event::GeneralRef(r)) => {
                if skip_depth == 0 && in_para && !para_has_dynamic_field {
                    push_xml_entity(&mut para_buf, &r);
                }
            }
            Ok(Event::End(e)) => {
                let closing_depth = depth;
                depth = depth.saturating_sub(1);
                if skip_depth > 0 {
                    skip_depth -= 1;
                    continue;
                }
                let local = local_name_bytes(e.name().as_ref()).to_vec();
                if in_para && (local.eq_ignore_ascii_case(b"p") || local.eq_ignore_ascii_case(b"h"))
                {
                    in_para = false;
                    if !para_has_dynamic_field {
                        let clean = normalize_odf_paragraph(&para_buf);
                        if !clean.is_empty() && !blocks.contains(&clean) {
                            blocks.push(clean);
                        }
                    }
                    para_buf.clear();
                    para_has_dynamic_field = false;
                }
                if master_page_depth == Some(closing_depth) {
                    master_page_depth = None;
                    in_para = false;
                    para_buf.clear();
                }
                if master_styles_depth == Some(closing_depth) {
                    master_styles_depth = None;
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
    }

    Ok(blocks)
}

fn should_skip_master_subtree(e: &BytesStart<'_>, local: &[u8]) -> bool {
    if should_skip_subtree(local) {
        return true;
    }
    if attr_value_local(e, b"display").is_some_and(|v| v.trim().eq_ignore_ascii_case("false")) {
        return true;
    }
    if attr_value_local(e, b"placeholder").is_some_and(|v| v.trim().eq_ignore_ascii_case("true")) {
        return true;
    }
    false
}

fn is_dynamic_master_field(local: &[u8]) -> bool {
    local.eq_ignore_ascii_case(b"page-number")
        || local.eq_ignore_ascii_case(b"page-count")
        || local.eq_ignore_ascii_case(b"sheet-name")
        || local.eq_ignore_ascii_case(b"title")
        || local.eq_ignore_ascii_case(b"date")
        || local.eq_ignore_ascii_case(b"time")
        || local.eq_ignore_ascii_case(b"header")
        || local.eq_ignore_ascii_case(b"footer")
        || local.eq_ignore_ascii_case(b"date-time")
}

fn should_skip_subtree(local: &[u8]) -> bool {
    local.eq_ignore_ascii_case(b"tracked-changes")
        || local.eq_ignore_ascii_case(b"automatic-styles")
        || local.eq_ignore_ascii_case(b"font-face-decls")
        || local.eq_ignore_ascii_case(b"scripts")
        || local.eq_ignore_ascii_case(b"object")
        || local.eq_ignore_ascii_case(b"binary-data")
}

fn handle_start_element(
    parser: &mut OdfParser<'_>,
    e: &BytesStart<'_>,
    local: &[u8],
    is_empty: bool,
) -> Result<(), ExtractError> {
    if local.eq_ignore_ascii_case(b"s") {
        // `<text:s/>` defaults to 1 space when `text:c` is omitted; clamp to 256.
        let count = attr_u64_local(e, b"c")
            .unwrap_or(1)
            .min(MAX_TEXT_SPACE_RUN as u64) as usize;
        if parser.in_block && count > 0 {
            parser.charge_expanded_chars(count)?;
            for _ in 0..count {
                parser.block_buf.push(' ');
            }
        }
        return Ok(());
    }
    if local.eq_ignore_ascii_case(b"tab") {
        parser.push_text("\t")?;
        return Ok(());
    }
    if local.eq_ignore_ascii_case(b"line-break") {
        parser.push_text("\n")?;
        return Ok(());
    }

    if local.eq_ignore_ascii_case(b"page") && parser.format == DocumentFormat::Odp {
        if !is_empty {
            parser.slide_index += 1;
            let raw_name = attr_value_local(e, b"name").unwrap_or_default();
            let clean_name = raw_name.trim();
            let heading = if clean_name.is_empty()
                || clean_name.to_ascii_lowercase().starts_with("page")
                    && clean_name[4..].chars().all(|c| c.is_ascii_digit())
            {
                format!("## Slide {}", parser.slide_index)
            } else {
                format!("## Slide {}: {clean_name}", parser.slide_index)
            };
            let plain_heading = heading.trim_start_matches("# ").to_owned();
            parser.pending_slide_heading = Some((heading, plain_heading));
        }
        return Ok(());
    }

    if local.eq_ignore_ascii_case(b"notes") && parser.format == DocumentFormat::Odp {
        if !is_empty {
            parser.in_notes = true;
        }
        return Ok(());
    }

    if local.eq_ignore_ascii_case(b"list") {
        if !is_empty {
            parser.list_depth += 1;
        }
        return Ok(());
    }
    if local.eq_ignore_ascii_case(b"list-item") {
        if !is_empty {
            parser.list_item_first_para.push(true);
        }
        return Ok(());
    }

    if local.eq_ignore_ascii_case(b"table") {
        if !is_empty {
            let name = attr_value_local(e, b"name");
            parser.tables.push(TableState {
                name,
                rows: Vec::new(),
                current_row: None,
                truncated_cols: false,
                truncated_rows: false,
            });
        }
        return Ok(());
    }

    if local.eq_ignore_ascii_case(b"table-row") {
        if let Some(table) = parser.tables.last_mut()
            && !is_empty
        {
            let repeat = attr_u64_local(e, b"number-rows-repeated").unwrap_or(1);
            table.current_row = Some(RowState {
                repeat,
                cells: Vec::new(),
                current_cell: None,
                pending_empty_col_overflow: false,
                truncated_cols: false,
            });
        }
        return Ok(());
    }

    if local.eq_ignore_ascii_case(b"table-cell")
        || local.eq_ignore_ascii_case(b"covered-table-cell")
    {
        if let Some(table) = parser.tables.last_mut()
            && let Some(row) = table.current_row.as_mut()
        {
            let repeat = attr_u64_local(e, b"number-columns-repeated").unwrap_or(1);
            row.current_cell = Some(CellState {
                repeat,
                paragraphs: Vec::new(),
            });
            if is_empty {
                parser.finish_cell()?;
            }
        }
        return Ok(());
    }

    if local.eq_ignore_ascii_case(b"h") {
        if !is_empty {
            parser.flush_block();
            let level = attr_u64_local(e, b"outline-level")
                .and_then(|v| usize::try_from(v).ok())
                .unwrap_or(1);
            parser.in_block = true;
            parser.block_heading_level = Some(level);
            parser.block_buf.clear();
        }
        return Ok(());
    }

    if local.eq_ignore_ascii_case(b"p") {
        if !is_empty {
            parser.flush_block();
            parser.in_block = true;
            parser.block_heading_level = None;
            parser.block_buf.clear();
        }
        return Ok(());
    }

    Ok(())
}

fn handle_end_element(parser: &mut OdfParser<'_>, local: &[u8]) -> Result<(), ExtractError> {
    if local.eq_ignore_ascii_case(b"p") || local.eq_ignore_ascii_case(b"h") {
        parser.flush_block();
    } else if local.eq_ignore_ascii_case(b"table-cell")
        || local.eq_ignore_ascii_case(b"covered-table-cell")
    {
        parser.flush_block();
        parser.finish_cell()?;
    } else if local.eq_ignore_ascii_case(b"table-row") {
        parser.finish_row()?;
    } else if local.eq_ignore_ascii_case(b"table") {
        parser.finish_table();
    } else if local.eq_ignore_ascii_case(b"list-item") {
        parser.list_item_first_para.pop();
    } else if local.eq_ignore_ascii_case(b"list") {
        parser.list_depth = parser.list_depth.saturating_sub(1);
    } else if local.eq_ignore_ascii_case(b"notes") && parser.format == DocumentFormat::Odp {
        parser.flush_block();
        parser.in_notes = false;
    } else if local.eq_ignore_ascii_case(b"page") && parser.format == DocumentFormat::Odp {
        parser.flush_block();
        parser.finish_slide();
    }
    Ok(())
}

fn render_gfm_table(rows: &[Vec<String>]) -> (String, String) {
    let col_count = rows
        .iter()
        .map(Vec::len)
        .max()
        .unwrap_or(0)
        .min(MAX_ODF_COLS);
    if col_count == 0 {
        return (String::new(), String::new());
    }

    let mut md = String::new();
    let mut plain = String::new();

    for (idx, row) in rows.iter().enumerate() {
        md.push('|');
        for col in 0..col_count {
            let cell = row.get(col).map(String::as_str).unwrap_or("");
            let escaped = escape_gfm_cell(cell);
            md.push(' ');
            md.push_str(&escaped);
            md.push_str(" |");

            if col > 0 {
                plain.push('\t');
            }
            plain.push_str(cell.trim());
        }
        md.push('\n');
        plain.push('\n');

        if idx == 0 {
            md.push('|');
            for _ in 0..col_count {
                md.push_str(" --- |");
            }
            md.push('\n');
        }
    }

    (md.trim_end().to_owned(), plain.trim_end().to_owned())
}

fn escape_gfm_cell(cell: &str) -> String {
    cell.replace('|', "\\|")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn normalize_odf_paragraph(raw: &str) -> String {
    raw.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn local_name_bytes(raw: &[u8]) -> &[u8] {
    match raw.iter().rposition(|&b| b == b':') {
        Some(idx) => &raw[idx + 1..],
        None => raw,
    }
}

fn local_name_eq(raw: &[u8], expected: &[u8]) -> bool {
    local_name_bytes(raw).eq_ignore_ascii_case(expected)
}

fn attr_value_local(e: &BytesStart<'_>, expected_local: &[u8]) -> Option<String> {
    for attr in e.attributes().flatten() {
        if local_name_eq(attr.key.as_ref(), expected_local)
            && let Ok(val) = attr.normalized_value(quick_xml::XmlVersion::Implicit1_0)
        {
            return Some(val.into_owned());
        }
    }
    None
}

fn attr_u64_local(e: &BytesStart<'_>, expected_local: &[u8]) -> Option<u64> {
    attr_value_local(e, expected_local)?
        .trim()
        .parse::<u64>()
        .ok()
}

fn push_xml_entity(out: &mut String, r: &quick_xml::events::BytesRef<'_>) {
    if let Ok(Some(ch)) = r.resolve_char_ref() {
        out.push(ch);
    } else if let Ok(name) = r.decode() {
        match name.as_ref() {
            "amp" => out.push('&'),
            "lt" => out.push('<'),
            "gt" => out.push('>'),
            "quot" => out.push('"'),
            "apos" => out.push('\''),
            _ => {}
        }
    }
}
