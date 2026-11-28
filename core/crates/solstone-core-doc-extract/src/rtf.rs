// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Bounded, iterative pure-Rust RTF (`.rtf`) text and table extractor.
//!
//! Security and data-integrity invariants:
//! - `#![forbid(unsafe_code)]` (inherited from crate root), zero recursion.
//! - Group nesting depth capped at `MAX_RTF_DEPTH = 256` (`LimitExceeded`).
//! - Closed per-frame mode enum (`Emit`, `FontTable`, `ScanForShptxt`,
//!   `ScanForInfoTitle`, `CaptureTitle`, `Skip`) where `Skip` is strictly
//!   absorbing so nested control words (e.g. `{\shprslt {\shptxt ...}}`) can
//!   never re-enable emission.
//! - `\\`, `\{`, `\}`, and bounds-checked `\binN` are lexed in ALL modes
//!   (including `Skip`) so raw binary bytes containing `}` never corrupt group
//!   balance and oversized `\binN` fails fast with `ExtractError::Invalid`.
//! - Bare `\r`, `\n`, and `\0` are ignored without consuming `\ucN` fallback
//!   skip slots or splitting UTF-16 surrogate pairs.
//! - Font-table `(\fN, \fcharsetM)` entries (capped at `MAX_RTF_FONTS = 4096`)
//!   and document `\ansicpgN` drive contiguous `\'hh` hex-run decoding via
//!   `encoding_rs`.

use std::collections::HashMap;

use encoding_rs::{
    BIG5, EUC_KR, Encoding, GBK, MACINTOSH, SHIFT_JIS, UTF_8, WINDOWS_874, WINDOWS_1250,
    WINDOWS_1251, WINDOWS_1252, WINDOWS_1253, WINDOWS_1254, WINDOWS_1255, WINDOWS_1256,
    WINDOWS_1257, WINDOWS_1258,
};

use crate::{BackendOutcome, DocumentFormat, ExtractError, ExtractLimits, RawExtraction};

const MAX_RTF_DEPTH: usize = 256;
const MAX_RTF_FONTS: usize = 4096;
const MAX_RTF_COLS: usize = 256;
const MAX_TITLE_CHARS: usize = 1024;
const MAX_UC_SKIP: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GroupMode {
    Emit,
    FontTable,
    ScanForShptxt,
    ScanForInfoTitle,
    CaptureTitle,
    Skip,
}

#[derive(Clone, Copy, Debug)]
struct Frame {
    mode: GroupMode,
    uc: usize,
    encoding: &'static Encoding,
    /// True right after `{` before the first control word or literal token in
    /// this group (used to recognize group-leading destinations and `\*`).
    at_group_start: bool,
    /// True right after `{\*` awaiting the destination word.
    saw_ignorable_star: bool,
    /// Current `\fN` id while parsing a `\fonttbl` group/entry.
    font_id: Option<i32>,
    /// True when this group opened a separate logical destination (header,
    /// footer, footnote, shptxt) that should flush on exit.
    flush_on_exit: bool,
}

struct RtfParser<'a> {
    limits: &'a ExtractLimits,
    default_encoding: &'static Encoding,
    font_table: HashMap<i32, &'static Encoding>,
    stack: Vec<Frame>,
    hex_buf: Vec<u8>,
    pending_high_surrogate: Option<u16>,
    skip_fallback_tokens: usize,
    // Document-scoped paragraph and table accumulation.
    in_tbl: bool,
    pending_nest_sep: Option<&'static str>,
    current_para: String,
    current_row_cells: Vec<String>,
    pending_table_rows: Vec<Vec<String>>,
    md_blocks: Vec<String>,
    plain_blocks: Vec<String>,
    title_buf: String,
    captured_title: Option<String>,
    // Bounds & diagnostics.
    expanded_chars: usize,
    emitted_output_chars: usize,
    truncated_cols: bool,
    stopped_early: bool,
    warnings: Vec<String>,
}

impl<'a> RtfParser<'a> {
    fn new(limits: &'a ExtractLimits) -> Self {
        Self {
            limits,
            default_encoding: WINDOWS_1252,
            font_table: HashMap::new(),
            stack: Vec::with_capacity(32),
            hex_buf: Vec::new(),
            pending_high_surrogate: None,
            skip_fallback_tokens: 0,
            in_tbl: false,
            pending_nest_sep: None,
            current_para: String::new(),
            current_row_cells: Vec::new(),
            pending_table_rows: Vec::new(),
            md_blocks: Vec::new(),
            plain_blocks: Vec::new(),
            title_buf: String::new(),
            captured_title: None,
            expanded_chars: 0,
            emitted_output_chars: 0,
            truncated_cols: false,
            stopped_early: false,
            warnings: Vec::new(),
        }
    }

    fn current_mode(&self) -> GroupMode {
        self.stack.last().map_or(GroupMode::Skip, |f| f.mode)
    }

    fn current_encoding(&self) -> &'static Encoding {
        self.stack
            .last()
            .map_or(self.default_encoding, |f| f.encoding)
    }

    fn is_emitting(&self) -> bool {
        matches!(
            self.current_mode(),
            GroupMode::Emit | GroupMode::CaptureTitle
        )
    }

    fn charge_expanded_chars(&mut self, count: usize) -> Result<(), ExtractError> {
        self.expanded_chars = self.expanded_chars.saturating_add(count);
        if self.expanded_chars > self.limits.max_expanded_text_chars {
            return Err(ExtractError::LimitExceeded(format!(
                "RTF expanded text exceeded limit of {} characters",
                self.limits.max_expanded_text_chars
            )));
        }
        Ok(())
    }

    fn flush_hex_run(&mut self) -> Result<(), ExtractError> {
        if self.hex_buf.is_empty() {
            return Ok(());
        }
        let bytes = std::mem::take(&mut self.hex_buf);
        if !self.is_emitting() {
            return Ok(());
        }
        let encoding = self.current_encoding();
        let (decoded, _, _) = encoding.decode(&bytes);
        self.flush_unpaired_surrogate()?;
        self.push_decoded_str(&decoded)
    }

    fn flush_unpaired_surrogate(&mut self) -> Result<(), ExtractError> {
        if self.pending_high_surrogate.take().is_some() && self.is_emitting() {
            self.push_decoded_char('\u{FFFD}')?;
        }
        Ok(())
    }

    fn push_decoded_str(&mut self, text: &str) -> Result<(), ExtractError> {
        if text.is_empty() || !self.is_emitting() {
            return Ok(());
        }
        match self.current_mode() {
            GroupMode::CaptureTitle => {
                if self.captured_title.is_none() {
                    for ch in text.chars() {
                        if self.title_buf.chars().count() >= MAX_TITLE_CHARS {
                            break;
                        }
                        if !ch.is_control() || ch == ' ' {
                            self.title_buf.push(ch);
                        }
                    }
                }
                Ok(())
            }
            GroupMode::Emit => {
                let count = text.chars().count();
                self.charge_expanded_chars(count)?;
                if !self.in_tbl
                    && self.current_row_cells.is_empty()
                    && !self.pending_table_rows.is_empty()
                    && text.chars().any(|c| !c.is_whitespace())
                {
                    self.flush_pending_table();
                }
                if text.chars().any(|c| !c.is_whitespace())
                    && let Some(sep) = self.pending_nest_sep.take()
                {
                    self.current_para.push_str(sep);
                }
                for ch in text.chars() {
                    if ch == '\0' {
                        continue;
                    }
                    self.current_para.push(ch);
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn push_decoded_char(&mut self, ch: char) -> Result<(), ExtractError> {
        if ch == '\0' || !self.is_emitting() {
            return Ok(());
        }
        let mut buf = [0u8; 4];
        let s = ch.encode_utf8(&mut buf);
        self.push_decoded_str(s)
    }

    fn emit_unicode_unit(&mut self, code_unit: u16) -> Result<(), ExtractError> {
        self.flush_hex_run()?;
        if !self.is_emitting() {
            self.pending_high_surrogate = None;
            return Ok(());
        }
        if (0xD800..=0xDBFF).contains(&code_unit) {
            self.flush_unpaired_surrogate()?;
            self.pending_high_surrogate = Some(code_unit);
            return Ok(());
        }
        if (0xDC00..=0xDFFF).contains(&code_unit) {
            if let Some(hi) = self.pending_high_surrogate.take() {
                let scalar =
                    0x10000 + (((u32::from(hi) - 0xD800) << 10) | (u32::from(code_unit) - 0xDC00));
                let ch = char::from_u32(scalar).unwrap_or('\u{FFFD}');
                return self.push_decoded_char(ch);
            }
            return self.push_decoded_char('\u{FFFD}');
        }
        self.flush_unpaired_surrogate()?;
        let ch = char::from_u32(u32::from(code_unit)).unwrap_or('\u{FFFD}');
        self.push_decoded_char(ch)
    }

    fn flush_paragraph(&mut self) {
        let raw = std::mem::take(&mut self.current_para);
        let normalized = normalize_rtf_paragraph(&raw);
        if normalized.is_empty() {
            return;
        }
        if !self.in_tbl && self.current_row_cells.is_empty() && !self.pending_table_rows.is_empty()
        {
            self.flush_pending_table();
        }
        self.emitted_output_chars = self
            .emitted_output_chars
            .saturating_add(normalized.chars().count())
            .saturating_add(2);
        self.md_blocks.push(normalized.clone());
        self.plain_blocks.push(normalized);
        self.check_early_stop();
    }

    fn finish_cell(&mut self) {
        self.pending_nest_sep = None;
        let raw = std::mem::take(&mut self.current_para);
        let cell = sanitize_table_cell(&raw);
        if self.current_row_cells.len() >= MAX_RTF_COLS {
            self.truncated_cols = true;
            return;
        }
        self.current_row_cells.push(cell);
    }

    fn finish_row(&mut self) {
        self.pending_nest_sep = None;
        if !self.current_para.trim().is_empty() {
            self.finish_cell();
        } else {
            self.current_para.clear();
        }
        if self.current_row_cells.is_empty() {
            return;
        }
        let mut cells = std::mem::take(&mut self.current_row_cells);
        while cells.last().is_some_and(String::is_empty) {
            cells.pop();
        }
        if !cells.is_empty() {
            self.pending_table_rows.push(cells);
        }
    }

    fn flush_pending_table(&mut self) {
        if !self.current_row_cells.is_empty() {
            let mut cells = std::mem::take(&mut self.current_row_cells);
            while cells.last().is_some_and(String::is_empty) {
                cells.pop();
            }
            if !cells.is_empty() {
                self.pending_table_rows.push(cells);
            }
        }
        if self.pending_table_rows.is_empty() {
            return;
        }
        let rows = std::mem::take(&mut self.pending_table_rows);
        let (md, plain) = render_gfm_table(&rows);
        if !md.is_empty() {
            self.emitted_output_chars = self
                .emitted_output_chars
                .saturating_add(md.chars().count().min(plain.chars().count()))
                .saturating_add(2);
            self.md_blocks.push(md);
            self.plain_blocks.push(plain);
            self.check_early_stop();
        }
    }

    fn check_early_stop(&mut self) {
        if self.emitted_output_chars > self.limits.max_output_chars.saturating_add(1024) {
            self.stopped_early = true;
        }
    }

    fn open_group(&mut self) -> Result<(), ExtractError> {
        self.flush_hex_run()?;
        self.flush_unpaired_surrogate()?;
        if self.stack.len() >= MAX_RTF_DEPTH {
            return Err(ExtractError::LimitExceeded(format!(
                "RTF group nesting depth exceeded limit of {MAX_RTF_DEPTH}"
            )));
        }
        let parent = self.stack.last().copied();
        let frame = match parent {
            None => Frame {
                mode: GroupMode::Emit,
                uc: 1,
                encoding: self.default_encoding,
                at_group_start: true,
                saw_ignorable_star: false,
                font_id: None,
                flush_on_exit: false,
            },
            Some(p) => Frame {
                mode: p.mode,
                uc: p.uc,
                encoding: p.encoding,
                at_group_start: true,
                saw_ignorable_star: false,
                font_id: None,
                flush_on_exit: false,
            },
        };
        self.stack.push(frame);
        Ok(())
    }

    fn close_group(&mut self) -> Result<(), ExtractError> {
        self.flush_hex_run()?;
        self.flush_unpaired_surrogate()?;
        self.skip_fallback_tokens = 0;
        let Some(closed) = self.stack.pop() else {
            return Ok(());
        };
        if closed.mode == GroupMode::CaptureTitle && self.captured_title.is_none() {
            let clean = self
                .title_buf
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            if !clean.is_empty() {
                self.captured_title = Some(clean);
            }
        }
        if closed.flush_on_exit && !self.in_tbl {
            self.flush_paragraph();
        }
        Ok(())
    }

    fn handle_control_word(
        &mut self,
        word: &str,
        param: Option<i64>,
    ) -> Result<Option<usize>, ExtractError> {
        // `\binN` MUST be honored and bounds-checked in ALL modes (including Skip).
        if word == "bin" {
            self.flush_hex_run()?;
            self.flush_unpaired_surrogate()?;
            let n = param.unwrap_or(0);
            if n < 0 {
                return Err(ExtractError::Invalid(format!(
                    "RTF \\bin has negative length {n}"
                )));
            }
            let skip_len = usize::try_from(n).map_err(|_| {
                ExtractError::Invalid(format!("RTF \\bin length {n} exceeds addressable memory"))
            })?;
            return Ok(Some(skip_len));
        }

        let Some(top) = self.stack.last_mut() else {
            return Ok(None);
        };
        let was_start = top.at_group_start;
        let saw_star = top.saw_ignorable_star;
        top.at_group_start = false;
        top.saw_ignorable_star = false;

        // `Skip` is strictly absorbing — no nested destination can ever exit it.
        if top.mode == GroupMode::Skip {
            return Ok(None);
        }

        // Handle `{\*\<word>` ignorable destinations.
        if saw_star {
            if word == "shpinst" && matches!(top.mode, GroupMode::Emit | GroupMode::ScanForShptxt) {
                top.mode = GroupMode::ScanForShptxt;
            } else {
                top.mode = GroupMode::Skip;
            }
            return Ok(None);
        }

        // Mode-specific destination transitions.
        match top.mode {
            GroupMode::FontTable => {
                match word {
                    "f" => {
                        if let Some(id) = param.and_then(|v| i32::try_from(v).ok()) {
                            top.font_id = Some(id);
                        }
                    }
                    "fcharset" => {
                        if let (Some(id), Some(cs)) =
                            (top.font_id, param.and_then(|v| i32::try_from(v).ok()))
                            && let Some(enc) = encoding_for_fcharset(cs)
                            && (self.font_table.len() < MAX_RTF_FONTS
                                || self.font_table.contains_key(&id))
                        {
                            self.font_table.insert(id, enc);
                        }
                    }
                    "cpg" => {
                        if let (Some(id), Some(cpg)) =
                            (top.font_id, param.and_then(|v| i32::try_from(v).ok()))
                            && let Some(enc) = encoding_for_codepage(cpg)
                            && (self.font_table.len() < MAX_RTF_FONTS
                                || self.font_table.contains_key(&id))
                        {
                            self.font_table.insert(id, enc);
                        }
                    }
                    _ if was_start && is_skip_destination(word) => {
                        top.mode = GroupMode::Skip;
                    }
                    _ => {}
                }
                return Ok(None);
            }
            GroupMode::ScanForShptxt => {
                if was_start {
                    if word == "shptxt" {
                        top.mode = GroupMode::Emit;
                        top.flush_on_exit = true;
                        self.flush_paragraph();
                    } else if word == "shpinst" {
                        top.mode = GroupMode::ScanForShptxt;
                    } else if word == "sp"
                        || word == "sn"
                        || word == "sv"
                        || word == "shprslt"
                        || is_skip_destination(word)
                    {
                        top.mode = GroupMode::Skip;
                    }
                }
                return Ok(None);
            }
            GroupMode::ScanForInfoTitle => {
                if was_start {
                    if word == "title" {
                        top.mode = GroupMode::CaptureTitle;
                    } else {
                        top.mode = GroupMode::Skip;
                    }
                }
                return Ok(None);
            }
            GroupMode::CaptureTitle | GroupMode::Emit => {
                if was_start && top.mode == GroupMode::Emit {
                    if word == "fonttbl" {
                        top.mode = GroupMode::FontTable;
                        return Ok(None);
                    }
                    if word == "info" {
                        top.mode = GroupMode::ScanForInfoTitle;
                        return Ok(None);
                    }
                    if word == "shpinst" {
                        top.mode = GroupMode::ScanForShptxt;
                        return Ok(None);
                    }
                    if is_skip_destination(word) {
                        top.mode = GroupMode::Skip;
                        return Ok(None);
                    }
                    if matches!(
                        word,
                        "header"
                            | "headerr"
                            | "headerl"
                            | "headerf"
                            | "footer"
                            | "footerr"
                            | "footerl"
                            | "footerf"
                            | "shptxt"
                    ) {
                        top.flush_on_exit = true;
                        if !self.in_tbl {
                            self.flush_paragraph();
                        }
                        return Ok(None);
                    }
                    if word == "footnote" {
                        if !self.current_para.is_empty() && !self.current_para.ends_with(' ') {
                            self.current_para.push(' ');
                        }
                        return Ok(None);
                    }
                }
            }
            GroupMode::Skip => return Ok(None),
        }

        // Control words valid during `Emit` and `CaptureTitle`.
        match word {
            "ansi" => {
                self.flush_hex_run()?;
                self.default_encoding = WINDOWS_1252;
                if let Some(f) = self.stack.last_mut() {
                    f.encoding = WINDOWS_1252;
                }
            }
            "mac" => {
                self.flush_hex_run()?;
                self.default_encoding = MACINTOSH;
                if let Some(f) = self.stack.last_mut() {
                    f.encoding = MACINTOSH;
                }
            }
            "pc" | "pca" => {
                self.flush_hex_run()?;
                self.default_encoding = WINDOWS_1252;
                if let Some(f) = self.stack.last_mut() {
                    f.encoding = WINDOWS_1252;
                }
            }
            "ansicpg" => {
                self.flush_hex_run()?;
                if let Some(cpg) = param.and_then(|v| i32::try_from(v).ok())
                    && let Some(enc) = encoding_for_codepage(cpg)
                {
                    self.default_encoding = enc;
                    if let Some(f) = self.stack.last_mut() {
                        f.encoding = enc;
                    }
                }
            }
            "f" | "af" => {
                self.flush_hex_run()?;
                if let Some(id) = param.and_then(|v| i32::try_from(v).ok()) {
                    let enc = self
                        .font_table
                        .get(&id)
                        .copied()
                        .unwrap_or(self.default_encoding);
                    if let Some(f) = self.stack.last_mut() {
                        f.encoding = enc;
                    }
                }
            }
            "uc" => {
                let uc = param.unwrap_or(1).clamp(0, MAX_UC_SKIP as i64) as usize;
                if let Some(f) = self.stack.last_mut() {
                    f.uc = uc;
                }
            }
            "u" => {
                let raw = param.unwrap_or(0);
                if !(-32768..=65535).contains(&raw) {
                    return Err(ExtractError::Invalid(format!(
                        "RTF \\u parameter {raw} out of 16-bit range"
                    )));
                }
                let unit = if raw < 0 {
                    (raw + 65536) as u16
                } else {
                    raw as u16
                };
                self.emit_unicode_unit(unit)?;
                self.skip_fallback_tokens = self.stack.last().map_or(1, |f| f.uc);
            }
            "pard" => {
                self.flush_hex_run()?;
                self.flush_unpaired_surrogate()?;
                self.in_tbl = false;
            }
            "intbl" | "trowd" => {
                self.flush_hex_run()?;
                self.flush_unpaired_surrogate()?;
                self.in_tbl = true;
            }
            "cell" if self.current_mode() == GroupMode::Emit => {
                self.flush_hex_run()?;
                self.flush_unpaired_surrogate()?;
                self.in_tbl = true;
                self.finish_cell();
            }
            "row" if self.current_mode() == GroupMode::Emit => {
                self.flush_hex_run()?;
                self.flush_unpaired_surrogate()?;
                self.finish_row();
                self.in_tbl = false;
            }
            "nestcell" if self.current_mode() == GroupMode::Emit => {
                self.flush_hex_run()?;
                self.flush_unpaired_surrogate()?;
                if !self.current_para.trim().is_empty() {
                    self.pending_nest_sep = Some(" | ");
                }
            }
            "nestrow" if self.current_mode() == GroupMode::Emit => {
                self.flush_hex_run()?;
                self.flush_unpaired_surrogate()?;
                if !self.current_para.trim().is_empty() {
                    self.pending_nest_sep = Some("; ");
                }
            }
            "par" | "sect" | "page" if self.current_mode() == GroupMode::Emit => {
                self.flush_hex_run()?;
                self.flush_unpaired_surrogate()?;
                if self.in_tbl {
                    self.push_decoded_char(' ')?;
                } else {
                    self.flush_paragraph();
                }
            }
            "line" if self.current_mode() == GroupMode::Emit => {
                self.flush_hex_run()?;
                self.flush_unpaired_surrogate()?;
                if self.in_tbl {
                    self.push_decoded_char(' ')?;
                } else {
                    self.push_decoded_char('\n')?;
                }
            }
            "tab" => {
                self.flush_hex_run()?;
                self.flush_unpaired_surrogate()?;
                self.push_decoded_char('\t')?;
            }
            "emdash" => {
                self.flush_hex_run()?;
                self.flush_unpaired_surrogate()?;
                self.push_decoded_char('—')?;
            }
            "endash" => {
                self.flush_hex_run()?;
                self.flush_unpaired_surrogate()?;
                self.push_decoded_char('–')?;
            }
            "lquote" => {
                self.flush_hex_run()?;
                self.flush_unpaired_surrogate()?;
                self.push_decoded_char('‘')?;
            }
            "rquote" => {
                self.flush_hex_run()?;
                self.flush_unpaired_surrogate()?;
                self.push_decoded_char('’')?;
            }
            "ldblquote" => {
                self.flush_hex_run()?;
                self.flush_unpaired_surrogate()?;
                self.push_decoded_char('“')?;
            }
            "rdblquote" => {
                self.flush_hex_run()?;
                self.flush_unpaired_surrogate()?;
                self.push_decoded_char('”')?;
            }
            "bullet" => {
                self.flush_hex_run()?;
                self.flush_unpaired_surrogate()?;
                self.push_decoded_char('•')?;
            }
            "enspace" | "emspace" | "qmspace" => {
                self.flush_hex_run()?;
                self.flush_unpaired_surrogate()?;
                self.push_decoded_char(' ')?;
            }
            _ => {}
        }

        Ok(None)
    }

    fn finish(mut self) -> RawExtraction {
        let _ = self.flush_hex_run();
        let _ = self.flush_unpaired_surrogate();
        if !self.current_row_cells.is_empty() || !self.pending_table_rows.is_empty() {
            if !self.current_para.trim().is_empty() && self.in_tbl {
                self.finish_cell();
            }
            self.flush_pending_table();
        }
        self.flush_paragraph();
        if self.truncated_cols {
            self.warnings
                .push(format!("RTF table columns truncated to {MAX_RTF_COLS}"));
        }
        RawExtraction {
            format: DocumentFormat::Rtf,
            title: self.captured_title,
            markdown: self.md_blocks.join("\n\n"),
            plain_text: self.plain_blocks.join("\n\n"),
            truncated: self.truncated_cols || self.stopped_early,
            warnings: self.warnings,
        }
    }
}

pub(crate) fn extract_rtf(
    bytes: &[u8],
    limits: &ExtractLimits,
) -> Result<BackendOutcome, ExtractError> {
    let mut i = 0usize;
    while i < bytes.len() && i < 64 && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    if bytes.len().saturating_sub(i) < 5 || !bytes[i..i + 5].eq_ignore_ascii_case(b"{\\rtf") {
        return Err(ExtractError::Invalid(
            "input does not start with RTF header".to_owned(),
        ));
    }

    let mut parser = RtfParser::new(limits);
    let len = bytes.len();

    while i < len && !parser.stopped_early {
        let b = bytes[i];
        match b {
            // Bare CR/LF/NUL are ignored everywhere without consuming \ucN
            // fallback skip slots or splitting UTF-16 surrogate pairs.
            b'\r' | b'\n' | b'\0' => {
                i += 1;
            }
            b'{' => {
                parser.open_group()?;
                i += 1;
            }
            b'}' => {
                parser.close_group()?;
                i += 1;
                if parser.stack.is_empty() {
                    break;
                }
            }
            b'\\' => {
                i += 1;
                if i >= len {
                    break;
                }
                let next = bytes[i];
                if next.is_ascii_alphabetic() {
                    let word_start = i;
                    while i < len && bytes[i].is_ascii_alphabetic() {
                        i += 1;
                    }
                    let word = std::str::from_utf8(&bytes[word_start..i]).unwrap_or_default();
                    let param = if i < len && (bytes[i] == b'-' || bytes[i].is_ascii_digit()) {
                        let sign_neg = if bytes[i] == b'-' {
                            i += 1;
                            true
                        } else {
                            false
                        };
                        let digits_start = i;
                        while i < len && bytes[i].is_ascii_digit() {
                            i += 1;
                        }
                        if i > digits_start {
                            let raw = std::str::from_utf8(&bytes[digits_start..i])
                                .ok()
                                .and_then(|s| s.parse::<i64>().ok())
                                .unwrap_or(i64::MAX);
                            Some(if sign_neg { raw.saturating_neg() } else { raw })
                        } else {
                            None
                        }
                    } else {
                        None
                    };
                    // Optional single space delimiter after a control word.
                    if i < len && bytes[i] == b' ' {
                        i += 1;
                    }
                    if let Some(bin_skip) = parser.handle_control_word(word, param)? {
                        if bin_skip > len.saturating_sub(i) {
                            return Err(ExtractError::Invalid(format!(
                                "RTF \\bin{bin_skip} extends past end of document"
                            )));
                        }
                        i += bin_skip;
                    }
                } else {
                    i += 1;
                    match next {
                        b'*' => {
                            if let Some(top) = parser.stack.last_mut() {
                                if top.at_group_start && top.mode != GroupMode::Skip {
                                    top.saw_ignorable_star = true;
                                } else {
                                    top.at_group_start = false;
                                }
                            }
                        }
                        b'\'' => {
                            if i + 1 < len
                                && let (Some(hi), Some(lo)) =
                                    (hex_val(bytes[i]), hex_val(bytes[i + 1]))
                            {
                                let byte = (hi << 4) | lo;
                                i += 2;
                                if let Some(top) = parser.stack.last_mut() {
                                    top.at_group_start = false;
                                    if top.saw_ignorable_star {
                                        top.saw_ignorable_star = false;
                                        top.mode = GroupMode::Skip;
                                    }
                                }
                                if parser.skip_fallback_tokens > 0 {
                                    parser.skip_fallback_tokens -= 1;
                                } else if parser.is_emitting() {
                                    parser.hex_buf.push(byte);
                                }
                            }
                        }
                        b'\\' | b'{' | b'}' => {
                            if let Some(top) = parser.stack.last_mut() {
                                top.at_group_start = false;
                                if top.saw_ignorable_star {
                                    top.saw_ignorable_star = false;
                                    top.mode = GroupMode::Skip;
                                }
                            }
                            if parser.skip_fallback_tokens > 0 {
                                parser.skip_fallback_tokens -= 1;
                            } else if parser.is_emitting() {
                                parser.flush_hex_run()?;
                                parser.flush_unpaired_surrogate()?;
                                parser.push_decoded_char(char::from(next))?;
                            }
                        }
                        b'~' => {
                            if let Some(top) = parser.stack.last_mut() {
                                top.at_group_start = false;
                            }
                            if parser.skip_fallback_tokens > 0 {
                                parser.skip_fallback_tokens -= 1;
                            } else if parser.is_emitting() {
                                parser.flush_hex_run()?;
                                parser.flush_unpaired_surrogate()?;
                                parser.push_decoded_char(' ')?;
                            }
                        }
                        b'_' => {
                            if let Some(top) = parser.stack.last_mut() {
                                top.at_group_start = false;
                            }
                            if parser.skip_fallback_tokens > 0 {
                                parser.skip_fallback_tokens -= 1;
                            } else if parser.is_emitting() {
                                parser.flush_hex_run()?;
                                parser.flush_unpaired_surrogate()?;
                                parser.push_decoded_char('-')?;
                            }
                        }
                        b'-' => {
                            // Optional hyphen: drop cleanly.
                            if let Some(top) = parser.stack.last_mut() {
                                top.at_group_start = false;
                            }
                        }
                        b'\r' | b'\n' => {
                            // Escaped CR/LF is equivalent to \par.
                            if next == b'\r' && i < len && bytes[i] == b'\n' {
                                i += 1;
                            }
                            if parser.current_mode() == GroupMode::Emit {
                                parser.flush_hex_run()?;
                                parser.flush_unpaired_surrogate()?;
                                if parser.in_tbl {
                                    parser.push_decoded_char(' ')?;
                                } else {
                                    parser.flush_paragraph();
                                }
                            }
                        }
                        _ => {
                            if let Some(top) = parser.stack.last_mut() {
                                top.at_group_start = false;
                            }
                        }
                    }
                }
            }
            other => {
                i += 1;
                if let Some(top) = parser.stack.last_mut() {
                    top.at_group_start = false;
                    if top.saw_ignorable_star {
                        top.saw_ignorable_star = false;
                        top.mode = GroupMode::Skip;
                    }
                    // In flat fonttbl entries, `;` ends the current font entry.
                    if top.mode == GroupMode::FontTable && other == b';' {
                        top.font_id = None;
                        continue;
                    }
                }
                if parser.skip_fallback_tokens > 0 {
                    parser.skip_fallback_tokens -= 1;
                    continue;
                }
                if !parser.is_emitting() {
                    continue;
                }
                if other >= 0x80 {
                    // Raw high byte in RTF stream: decode through active codepage.
                    parser.hex_buf.push(other);
                } else {
                    parser.flush_hex_run()?;
                    parser.flush_unpaired_surrogate()?;
                    parser.push_decoded_char(char::from(other))?;
                }
            }
        }
    }

    Ok(BackendOutcome::Extracted(parser.finish()))
}

fn is_skip_destination(word: &str) -> bool {
    matches!(
        word,
        "colortbl"
            | "stylesheet"
            | "listtable"
            | "listoverridetable"
            | "rsidtbl"
            | "mmathPr"
            | "pict"
            | "object"
            | "objdata"
            | "datafield"
            | "themedata"
            | "colorschememapping"
            | "datastore"
            | "latentstyles"
            | "fldinst"
            | "shprslt"
            | "xmlnstbl"
            | "bkmkstart"
            | "bkmkend"
            | "xe"
            | "tc"
            | "rxe"
            | "ftnsep"
            | "ftnsepc"
            | "aftnsep"
            | "aftnsepc"
            | "pnseclvl1"
            | "pnseclvl2"
            | "pnseclvl3"
            | "pnseclvl4"
            | "pnseclvl5"
            | "pnseclvl6"
            | "pnseclvl7"
            | "pnseclvl8"
            | "pnseclvl9"
            | "generator"
            | "revtbl"
            | "pgdsctbl"
            | "defchp"
            | "defpap"
            | "sp"
            | "nonesttables"
            | "nesttableprops"
            | "panose"
            | "falt"
    )
}

fn encoding_for_codepage(cpg: i32) -> Option<&'static Encoding> {
    match cpg {
        874 => Some(WINDOWS_874),
        932 => Some(SHIFT_JIS),
        936 => Some(GBK),
        949 => Some(EUC_KR),
        950 => Some(BIG5),
        1250 => Some(WINDOWS_1250),
        1251 => Some(WINDOWS_1251),
        1252 | 28591 => Some(WINDOWS_1252),
        1253 => Some(WINDOWS_1253),
        1254 => Some(WINDOWS_1254),
        1255 => Some(WINDOWS_1255),
        1256 => Some(WINDOWS_1256),
        1257 => Some(WINDOWS_1257),
        1258 => Some(WINDOWS_1258),
        10000 => Some(MACINTOSH),
        65001 => Some(UTF_8),
        _ => None,
    }
}

fn encoding_for_fcharset(charset: i32) -> Option<&'static Encoding> {
    match charset {
        0..=2 => Some(WINDOWS_1252),
        77 => Some(MACINTOSH),
        128 => Some(SHIFT_JIS),
        129 => Some(EUC_KR),
        134 => Some(GBK),
        136 => Some(BIG5),
        161 => Some(WINDOWS_1253),
        162 => Some(WINDOWS_1254),
        163 => Some(WINDOWS_1258),
        177 => Some(WINDOWS_1255),
        178 => Some(WINDOWS_1256),
        186 => Some(WINDOWS_1257),
        204 => Some(WINDOWS_1251),
        222 => Some(WINDOWS_874),
        238 => Some(WINDOWS_1250),
        _ => None,
    }
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn normalize_rtf_paragraph(raw: &str) -> String {
    raw.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn sanitize_table_cell(raw: &str) -> String {
    raw.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn render_gfm_table(rows: &[Vec<String>]) -> (String, String) {
    let col_count = rows
        .iter()
        .map(Vec::len)
        .max()
        .unwrap_or(0)
        .min(MAX_RTF_COLS);
    if col_count == 0 {
        return (String::new(), String::new());
    }

    let mut md = String::new();
    let mut plain = String::new();

    for (idx, row) in rows.iter().enumerate() {
        md.push('|');
        for col in 0..col_count {
            let cell = row.get(col).map(String::as_str).unwrap_or("");
            let escaped = cell.replace('|', "\\|");
            md.push(' ');
            md.push_str(&escaped);
            md.push_str(" |");

            if col > 0 {
                plain.push('\t');
            }
            plain.push_str(cell);
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
