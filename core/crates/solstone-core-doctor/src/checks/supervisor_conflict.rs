// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
use crate::{
    context::CheckContext,
    vocabulary::{Check, RunnerResult, Status, make_result},
};

#[cfg(unix)]
pub fn run(context: &CheckContext, check: Check) -> RunnerResult {
    let dir = context.home_dir.join("Library/LaunchAgents");
    let mut foreign = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|ext| ext != "plist") {
                continue;
            }
            let Ok(value) = plist::Value::from_file(&path) else {
                continue;
            };
            let Some(data) = value.as_dictionary() else {
                continue;
            };
            let label = data
                .get("Label")
                .and_then(plist::Value::as_string)
                .filter(|label| {
                    *label != "org.solpbc.solstone" && !label.starts_with("org.solpbc.solstone.")
                });
            let persistent = data.get("KeepAlive").is_some_and(|value| {
                value.as_boolean() == Some(true)
                    || value.as_dictionary().is_some_and(|dict| !dict.is_empty())
            });
            let mentions_solstone_app = command_strings(data)
                .iter()
                .any(|command| command.contains("/Applications/solstone.app"));
            if let Some(label) = label.filter(|_| persistent && mentions_solstone_app) {
                foreign.push((label.to_owned(), path));
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
        .map(|(label, path)| {
            let target = format!("gui/{}/{label}", nix::unistd::Uid::effective());
            format!(
                "launchctl bootout {}; rm {}",
                shell_quote(&target),
                shell_quote(&path.display().to_string())
            )
        })
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

#[cfg(not(unix))]
pub fn run(_context: &CheckContext, check: Check) -> RunnerResult {
    Ok(make_result(
        check,
        Status::Skip,
        "not supported on windows",
        None::<String>,
    ))
}

#[cfg(unix)]
fn command_strings(data: &plist::Dictionary) -> Vec<&str> {
    let mut strings = data
        .get("Program")
        .and_then(plist::Value::as_string)
        .into_iter()
        .collect::<Vec<_>>();
    if let Some(arguments) = data
        .get("ProgramArguments")
        .and_then(plist::Value::as_array)
    {
        strings.extend(arguments.iter().filter_map(plist::Value::as_string));
    }
    strings
}

#[cfg(unix)]
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}
