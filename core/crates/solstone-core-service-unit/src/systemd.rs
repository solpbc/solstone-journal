// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;

const SERVICE_START_TIMEOUT_SECONDS: u32 = 120;
const SERVICE_FILE_DESCRIPTOR_LIMIT: u32 = 4096;

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
        "[Unit]\nDescription=Solstone Supervisor\nAfter=default.target\nStartLimitIntervalSec=120\nStartLimitBurst=10\n\n[Service]\nType=notify\nTimeoutStartSec={SERVICE_START_TIMEOUT_SECONDS}\nExecStart={} start {}\nRestart=on-failure\nRestartSec=5\nKillMode=control-group\nTimeoutStopSec=30\nLimitNOFILE={SERVICE_FILE_DESCRIPTOR_LIMIT}\n{environment_lines}\n\n[Install]\nWantedBy=default.target\n",
        render_exec_token(launcher_path),
        render_exec_token(port),
    )
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
    use super::{escape_quoted, render_environment, render_exec_token};

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
