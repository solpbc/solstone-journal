// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::io::{self, Write};

use solstone_core_system::operational_log_parse::ParsedHealthLogRow;
use solstone_core_system_health::sanitize_for_terminal;

const DIM: &str = "\x1b[2m";
const RESET: &str = "\x1b[0m";

/// Render collected rows with Python-compatible TTY grouping.
pub fn render_collected(
    output: &mut dyn Write,
    rows: &[ParsedHealthLogRow],
    is_tty: bool,
) -> io::Result<()> {
    if !is_tty {
        for row in rows {
            writeln!(output, "{}", row.raw)?;
        }
        return Ok(());
    }

    let mut last_service = None;
    for row in rows {
        render_stream_row(
            output,
            &row.raw,
            Some(&row.service),
            true,
            &mut last_service,
        )?;
    }
    Ok(())
}

pub(crate) fn render_stream_row(
    output: &mut dyn Write,
    raw: &str,
    service: Option<&str>,
    is_tty: bool,
    last_service: &mut Option<String>,
) -> io::Result<()> {
    if !is_tty {
        return writeln!(output, "{raw}");
    }
    if let Some(service) = service
        && last_service.as_deref() != Some(service)
    {
        if last_service.is_some() {
            writeln!(output)?;
        }
        writeln!(
            output,
            "{DIM}── {} ──{RESET}",
            sanitize_for_terminal(service)
        )?;
        *last_service = Some(service.to_owned());
    }
    writeln!(output, "{}", sanitize_for_terminal(raw))
}

#[cfg(test)]
mod tests {
    use chrono::NaiveDateTime;
    use solstone_core_system::operational_log_parse::ParsedHealthLogRow;

    use super::*;

    fn row(service: &str, raw: &str) -> ParsedHealthLogRow {
        ParsedHealthLogRow {
            timestamp: NaiveDateTime::parse_from_str("2026-01-01 12:00:00", "%Y-%m-%d %H:%M:%S")
                .unwrap(),
            service: service.to_owned(),
            stream: "stdout".to_owned(),
            message: "message".to_owned(),
            raw: raw.to_owned(),
        }
    }

    #[test]
    fn non_tty_keeps_raw_terminal_unsafe_text() {
        let rows = [row("svc", "raw\x1b\ntext")];
        let mut output = Vec::new();
        render_collected(&mut output, &rows, false).unwrap();
        assert_eq!(output, b"raw\x1b\ntext\n");
    }

    #[test]
    fn tty_groups_services_and_sanitizes_headers_and_rows() {
        let rows = [
            row("one\x1b", "first\x1b"),
            row("one\x1b", "second"),
            row("two\n", "third\n"),
        ];
        let mut output = Vec::new();
        render_collected(&mut output, &rows, true).unwrap();
        assert_eq!(
            String::from_utf8(output).unwrap(),
            "\x1b[2m── one\\x1b ──\x1b[0m\nfirst\\x1b\nsecond\n\n\x1b[2m── two\\n ──\x1b[0m\nthird\\n\n"
        );
    }

    #[test]
    fn empty_rows_write_nothing_in_either_mode() {
        for is_tty in [false, true] {
            let mut output = Vec::new();
            render_collected(&mut output, &[], is_tty).unwrap();
            assert!(output.is_empty());
        }
    }
}
