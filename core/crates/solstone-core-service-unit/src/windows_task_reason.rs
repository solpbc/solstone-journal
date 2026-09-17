// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Reading the real refusal out of a Windows task-operation worker.
//!
//! The worker is `powershell.exe`, and PowerShell serializes its error,
//! warning and *progress* streams as a CLIXML document whenever stderr is a
//! pipe rather than a console. A resumed `journal setup` therefore told its
//! owner to re-inspect scheduler state and handed them `#< CLIXML` carrying
//! only a "Preparing modules for first use" progress record -- the refusal
//! itself (`task-not-idle-before-update`) was nowhere in what they were shown.
//!
//! The worker now writes its reason as one line of JSON on **stdout**, which
//! PowerShell never wraps. This module reads that first, falls back to the
//! plain line the worker also writes to stderr, and only then to whatever
//! stderr holds with the CLIXML envelope removed.

/// The schema of the worker's one-line failure record.
pub const WINDOWS_TASK_FAILURE_SCHEMA: &str = "solstone-windows-task-operation-failure-v1";

/// The prefix the worker writes on its plain stderr line.
const PLAIN_PREFIX: &str = "Windows task operation failed: ";

/// What an owner is told when the worker failed without saying anything.
/// Names who was silent: "the task operation gave no reason" restated the
/// subject of the clause it sits inside and answered nothing.
const NO_REASON: &str = "windows gave no reason";

/// Resolve the refusal to report for a failed task operation.
///
/// `stdout` and `stderr` are the worker's raw byte streams.
#[must_use]
pub fn windows_task_failure_reason(stdout: &[u8], stderr: &[u8]) -> String {
    let stdout = String::from_utf8_lossy(stdout);
    if let Some(reason) = structured_reason(&stdout) {
        return reason;
    }
    let stderr = String::from_utf8_lossy(stderr);
    if let Some(reason) = stderr
        .lines()
        .filter_map(|line| line.trim().strip_prefix(PLAIN_PREFIX))
        .map(str::trim)
        .find(|reason| !reason.is_empty())
    {
        return reason.to_owned();
    }
    let remainder = strip_clixml(&stderr);
    if remainder.is_empty() {
        NO_REASON.to_owned()
    } else {
        remainder
    }
}

/// Read the worker's JSON failure record without pulling a deserializer into
/// this crate: the record has exactly two string fields and a fixed schema.
fn structured_reason(stdout: &str) -> Option<String> {
    let line = stdout
        .lines()
        .map(str::trim)
        .find(|line| line.contains(WINDOWS_TASK_FAILURE_SCHEMA))?;
    let reason = json_string_field(line, "reason")?;
    let schema = json_string_field(line, "schema")?;
    if schema != WINDOWS_TASK_FAILURE_SCHEMA || reason.is_empty() {
        return None;
    }
    Some(reason)
}

/// Extract `"<name>":"<value>"` from a flat one-line JSON object, honouring
/// the escapes `ConvertTo-Json` can emit for a thrown message.
fn json_string_field(line: &str, name: &str) -> Option<String> {
    let needle = format!("\"{name}\"");
    let start = line.find(&needle)? + needle.len();
    let rest = line[start..].trim_start();
    let rest = rest.strip_prefix(':')?.trim_start();
    let mut characters = rest.strip_prefix('"')?.chars();
    let mut value = String::new();
    while let Some(character) = characters.next() {
        match character {
            '"' => return Some(value),
            '\\' => match characters.next()? {
                'n' => value.push('\n'),
                'r' => value.push('\r'),
                't' => value.push('\t'),
                'u' => {
                    let hex: String = characters.by_ref().take(4).collect();
                    let code = u32::from_str_radix(&hex, 16).ok()?;
                    value.push(char::from_u32(code)?);
                }
                escaped => value.push(escaped),
            },
            _ => value.push(character),
        }
    }
    None
}

/// Drop PowerShell's CLIXML envelope, keeping any plain text around it.
///
/// The envelope is a `#< CLIXML` marker line followed by one XML document, so
/// removing the marker and every XML-looking line leaves exactly the bytes a
/// script wrote straight to the stderr handle.
fn strip_clixml(stderr: &str) -> String {
    stderr
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with("#< CLIXML") && !line.starts_with('<'))
        .collect::<Vec<_>>()
        .join("; ")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The exact shape measured on a Windows 11 test host: a resumed
    /// `journal setup` refused with the progress record as its whole reason.
    const MEASURED_CLIXML: &str = concat!(
        "#< CLIXML\r\n",
        "<Objs Version=\"1.1.0.1\" xmlns=\"http://schemas.microsoft.com/powershell/2004/04\">",
        "<PR><S S=\"progress\">Preparing modules for first use.</S></PR></Objs>\r\n",
    );

    #[test]
    fn the_structured_stdout_reason_wins_over_progress_noise() {
        let stdout = format!(
            "{{\"schema\":\"{WINDOWS_TASK_FAILURE_SCHEMA}\",\"reason\":\"task-not-idle-before-update\"}}\r\n"
        );
        assert_eq!(
            windows_task_failure_reason(stdout.as_bytes(), MEASURED_CLIXML.as_bytes()),
            "task-not-idle-before-update"
        );
    }

    #[test]
    fn a_plain_stderr_line_is_read_when_stdout_is_empty() {
        let stderr = format!(
            "{MEASURED_CLIXML}Windows task operation failed: task-changed-before-mutation\r\n"
        );
        assert_eq!(
            windows_task_failure_reason(b"", stderr.as_bytes()),
            "task-changed-before-mutation"
        );
    }

    #[test]
    fn progress_only_clixml_never_becomes_the_reason() {
        assert_eq!(
            windows_task_failure_reason(b"", MEASURED_CLIXML.as_bytes()),
            NO_REASON
        );
    }

    #[test]
    fn unrecognized_stderr_survives_with_its_envelope_removed() {
        let stderr = format!("{MEASURED_CLIXML}powershell.exe: access is denied\r\n");
        assert_eq!(
            windows_task_failure_reason(b"", stderr.as_bytes()),
            "powershell.exe: access is denied"
        );
    }

    #[test]
    fn a_foreign_schema_or_empty_reason_is_not_trusted() {
        let foreign = "{\"schema\":\"something-else\",\"reason\":\"nope\"}";
        assert_eq!(
            windows_task_failure_reason(foreign.as_bytes(), MEASURED_CLIXML.as_bytes()),
            NO_REASON
        );
        let empty = format!("{{\"schema\":\"{WINDOWS_TASK_FAILURE_SCHEMA}\",\"reason\":\"\"}}");
        assert_eq!(
            windows_task_failure_reason(empty.as_bytes(), MEASURED_CLIXML.as_bytes()),
            NO_REASON
        );
    }

    #[test]
    fn escaped_text_in_the_reason_is_decoded() {
        let stdout = format!(
            "{{\"schema\":\"{WINDOWS_TASK_FAILURE_SCHEMA}\",\"reason\":\"one \\u0022two\\u0022\\nthree\"}}"
        );
        assert_eq!(
            windows_task_failure_reason(stdout.as_bytes(), b""),
            "one \"two\"\nthree"
        );
    }
}
