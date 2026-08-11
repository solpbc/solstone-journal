// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
use crate::{
    context::CheckContext,
    vocabulary::{Check, RunnerResult, Status, make_result},
};
pub fn run(context: &CheckContext, check: Check) -> RunnerResult {
    let dir = context.home_dir.join("Library/LaunchAgents");
    let mut foreign = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|ext| ext != "plist") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let label = value(&text, "Label");
            let persistent = text.contains("<key>KeepAlive</key>")
                && (text.contains("<true/>") || text.contains("<dict>"));
            if let Some(label) = label.filter(|label| {
                label != "org.solpbc.solstone" && !label.starts_with("org.solpbc.solstone.")
            }) && persistent
                && text.contains("/Applications/solstone.app")
            {
                foreign.push((label, path));
            }
        }
    }
    if foreign.is_empty() {
        return Ok(make_result(
            check,
            Status::Ok,
            "no macOS supervisor conflict (native check: foreign-launcher scan only; no competing KeepAlive launcher found)",
            None::<String>,
        ));
    }
    let commands = foreign
        .iter()
        .map(|(label, path)| format!("launchctl bootout 'gui/0/{label}'; rm '{}'", path.display()))
        .collect::<Vec<_>>()
        .join("; ");
    let fix = format!(
        "remove foreign launchers targeting /Applications/solstone.app: {commands}; then rerun journal doctor"
    );
    let count = foreign.len();
    let noun = if count == 1 { "launcher" } else { "launchers" };
    let verb = if count == 1 { "targets" } else { "target" };
    Ok(make_result(
        check,
        Status::Fail,
        format!(
            "macOS supervisor conflict: {count} foreign KeepAlive {noun} {verb} /Applications/solstone.app"
        ),
        Some(fix),
    ))
}
fn value(text: &str, key: &str) -> Option<String> {
    let (_, rest) = text.split_once(&format!("<key>{key}</key>"))?;
    let start = rest.find("<string>")? + 8;
    let end = rest[start..].find("</string>")? + start;
    Some(rest[start..end].to_owned())
}
