// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;

const SERVICE_START_TIMEOUT_SECONDS: u32 = 120;
const SERVICE_FILE_DESCRIPTOR_LIMIT: u32 = 4096;
/// How long systemd lets a stop run before it SIGKILLs the whole control
/// group. The supervisor's own standard shutdown is budgeted well under it
/// (`solstone_core_system::lifecycle::STANDARD_SHUTDOWN_BUDGET`; a test in
/// `solstone-core` pins the margin), so this is the manager's backstop, not
/// the shape of a normal stop. Left at its installed value on purpose: an
/// installed unit only changes at the next `journal setup`, and the fix that
/// reaches every install at once is the supervisor's budget.
pub const SERVICE_STOP_TIMEOUT_SECONDS: u32 = 30;
/// launchd's documented default `ExitTimeOut`; the generated plist does not
/// set one, so the supervisor's budget has to clear this too.
pub const LAUNCHD_DEFAULT_EXIT_TIMEOUT_SECONDS: u32 = 20;

/// Render the systemd user unit for the Solstone supervisor.
pub fn render_systemd_unit(
    env: &BTreeMap<String, String>,
    launcher_path: &str,
    port: &str,
) -> String {
    let environment_lines = env
        .iter()
        .map(|(key, value)| render_environment(key, value))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "[Unit]\nDescription=Solstone Supervisor\nAfter=default.target\nStartLimitIntervalSec=120\nStartLimitBurst=10\n\n[Service]\nType=notify\nTimeoutStartSec={SERVICE_START_TIMEOUT_SECONDS}\nExecStart={} start {}\nRestart=on-failure\nRestartSec=5\nKillMode=control-group\nTimeoutStopSec={SERVICE_STOP_TIMEOUT_SECONDS}\nLimitNOFILE={SERVICE_FILE_DESCRIPTOR_LIMIT}\n{environment_lines}\n\n[Install]\nWantedBy=default.target\n",
        render_exec_token(launcher_path),
        render_exec_token(port),
    )
}

/// Read the port out of an installed systemd unit.
///
/// The exec line this crate renders ends in the port, quoted only when the
/// token needs it. Returning `None` means "this unit does not say", which the
/// caller treats as "fall back to the default" rather than as a failure.
#[must_use]
pub fn systemd_unit_port(unit: &str) -> Option<String> {
    let exec = unit
        .lines()
        .map(str::trim)
        .find_map(|line| line.strip_prefix("ExecStart="))?;
    unquote_exec_token(exec.split_whitespace().next_back()?)
}

/// Undo `render_exec_token` for one token.
fn unquote_exec_token(token: &str) -> Option<String> {
    let Some(quoted) = token.strip_prefix('"').and_then(|rest| rest.strip_suffix('"')) else {
        return (!token.is_empty()).then(|| token.to_owned());
    };
    let mut value = String::with_capacity(quoted.len());
    let mut characters = quoted.chars();
    while let Some(character) = characters.next() {
        if character == '\\' {
            value.push(characters.next()?);
        } else {
            value.push(character);
        }
    }
    (!value.is_empty()).then_some(value)
}

fn render_exec_token(token: &str) -> String {
    if is_safe(token) {
        return token.to_owned();
    }
    format!("\"{}\"", escape_quoted(token, true))
}

fn render_environment(key: &str, value: &str) -> String {
    if is_safe(value) {
        return format!("Environment={key}={value}");
    }
    format!("Environment=\"{key}={}\"", escape_quoted(value, false))
}

fn is_safe(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'.' | b'_' | b'-' | b':')
        })
}

fn escape_quoted(value: &str, escape_dollar: bool) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            '\0'..='\u{8}' | '\u{b}'..='\u{c}' | '\u{e}'..='\u{1f}' | '\u{7f}' => {
                escaped.push_str(&format!("\\x{:02X}", character as u32));
            }
            '\u{80}'..='\u{9f}' => escaped.push_str(&format!("\\u{:04X}", character as u32)),
            '$' if escape_dollar => escaped.push_str("$$"),
            '%' => escaped.push_str("%%"),
            _ => escaped.push(character),
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{
        escape_quoted, render_environment, render_exec_token, render_systemd_unit,
        systemd_unit_port,
    };

    #[test]
    fn the_installed_port_round_trips_out_of_the_rendered_unit() {
        // `journal service install` with no `--port` adopts this reading, so a
        // re-register never silently moves an established journal onto 5015.
        for port in ["5015", "6123", "65535"] {
            let unit = render_systemd_unit(&BTreeMap::new(), "/home/sol/.local/bin/journal", port);
            assert_eq!(systemd_unit_port(&unit).as_deref(), Some(port));
        }
    }

    #[test]
    fn a_unit_without_an_exec_line_says_nothing_rather_than_guessing() {
        assert_eq!(systemd_unit_port("[Service]\nType=notify\n"), None);
        assert_eq!(systemd_unit_port(""), None);
    }

    #[test]
    fn a_quoted_launcher_does_not_swallow_the_port() {
        let unit = render_systemd_unit(
            &BTreeMap::new(),
            "/home/sol journal/.local/bin/journal",
            "6124",
        );
        assert!(unit.contains("ExecStart=\""));
        assert_eq!(systemd_unit_port(&unit).as_deref(), Some("6124"));
    }

    #[test]
    fn keeps_safe_values_unquoted() {
        assert_eq!(render_exec_token("/usr/bin/journal"), "/usr/bin/journal");
        assert_eq!(
            render_environment("PATH", "/usr/bin:/bin"),
            "Environment=PATH=/usr/bin:/bin"
        );
    }

    #[test]
    fn distinguishes_exec_and_environment_dollars() {
        assert_eq!(escape_quoted("${name}%", true), "$${name}%%");
        assert_eq!(escape_quoted("${name}%", false), "${name}%%");
    }
}
