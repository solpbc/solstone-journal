// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native owner-facing `journal open`.
//!
//! The journal mints a one-time link over its owner socket or pipe, which only
//! this OS account can reach, and the default browser opens it. The
//! link is spent on first use, so a copy left in a process listing opens
//! nothing. There is no fallback to a bare address: without a live socket of
//! this account's journal there is nothing to authorize.

use std::process::ExitCode;

#[cfg(any(target_os = "linux", target_os = "macos", windows))]
use crate::EXIT_UNAVAILABLE;
use crate::{EXIT_TEMPFAIL, eprint_journal_path_error, resolve_process_journal_path};

/// The path to open: what the owner typed, rooted at `/`.
fn rooted(path: &str) -> String {
    if path.starts_with('/') {
        path.to_owned()
    } else {
        format!("/{path}")
    }
}

pub(crate) fn run(path: String, print_url: bool) -> ExitCode {
    let journal = match resolve_process_journal_path() {
        Ok(journal) => journal,
        Err(error) => {
            eprint_journal_path_error(error);
            return ExitCode::from(EXIT_TEMPFAIL);
        }
    };
    let path = rooted(&path);
    #[cfg(any(target_os = "linux", target_os = "macos", windows))]
    {
        let url = match mint(&journal.path, &path) {
            Ok(url) => url,
            Err(message) => {
                eprintln!("{message}");
                return ExitCode::from(EXIT_UNAVAILABLE);
            }
        };
        if print_url {
            println!("{url}");
            return ExitCode::SUCCESS;
        }
        if launch(&url) {
            return ExitCode::SUCCESS;
        }
        eprintln!(
            "a browser could not be opened here. run this command again with --print-url, and open the link it prints within a minute."
        );
        if let Some(port) = link_port(&url) {
            eprintln!(
                "to open it from another computer, first forward the journal's ports over SSH, and keep that terminal open:"
            );
            eprintln!(
                "  ssh -N -L {port}:127.0.0.1:{port} -L 7659:127.0.0.1:7659 YOUR_LOGIN@JOURNAL_COMPUTER"
            );
        }
        ExitCode::FAILURE
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        // No owner socket here, so the browser port is open to this computer.
        let _ = print_url;
        println!(
            "open your journal: {}{path}",
            crate::settings::journal_url(&journal.path)
        );
        ExitCode::SUCCESS
    }
}

// A mac journal runs inside the journal app, so the app is what starts it.
#[cfg(target_os = "macos")]
const NOT_RUNNING: &str = "your journal isn't running for this account on this computer, so there is nothing to open. open the journal app to start it.";
#[cfg(target_os = "linux")]
const NOT_RUNNING: &str = "your journal isn't running for this account on this computer, so there is nothing to open. start it with solstone journal up.";
#[cfg(target_os = "macos")]
const OLDER_JOURNAL: &str = "your running journal is older than this command. quit the journal app, open it again, and try again.";
#[cfg(windows)]
const NOT_RUNNING: &str = "your journal isn't running for this account on this computer, so there is nothing to open. start it from the journal app, and try again.";
#[cfg(target_os = "linux")]
const OLDER_JOURNAL: &str = "your running journal is older than this command. restart it with solstone journal down, then solstone journal up, and try again.";
#[cfg(windows)]
const OLDER_JOURNAL: &str = "your running journal is older than this command. restart it from the journal app, and try again.";

/// The port of a minted link, for the SSH forwarding line.
#[cfg(any(target_os = "linux", target_os = "macos", windows))]
fn link_port(url: &str) -> Option<&str> {
    let port = url.strip_prefix("http://localhost:")?.split('/').next()?;
    (!port.is_empty() && port.bytes().all(|byte| byte.is_ascii_digit())).then_some(port)
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn mint(journal: &std::path::Path, path: &str) -> Result<String, String> {
    use solstone_core_local_endpoint as endpoint;
    let root = std::fs::canonicalize(journal).unwrap_or_else(|_| journal.to_owned());
    let socket = endpoint::socket_for_journal(&root)
        .ok()
        .flatten()
        .filter(|socket| {
            endpoint::owned_socket(socket) && endpoint::socket_belongs_to_journal(socket, &root)
        })
        .ok_or_else(|| NOT_RUNNING.to_owned())?;
    let agent = endpoint::socket_agent(&socket, std::time::Duration::from_secs(5));
    let mut response = agent
        .post("http://localhost/_solstone/browser-link")
        .header(
            "User-Agent",
            concat!("solstone/", env!("CARGO_PKG_VERSION")),
        )
        .header("Content-Type", "application/json")
        .send(serde_json::json!({ "path": path }).to_string())
        .map_err(|_| NOT_RUNNING.to_owned())?;
    let status = response.status().as_u16();
    let body: serde_json::Value = response
        .body_mut()
        .with_config()
        .limit(16384)
        .read_to_vec()
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    match status {
        200 => body["url"].as_str().map(str::to_owned).ok_or_else(|| {
            "the response from your journal could not be read. try again.".to_owned()
        }),
        404 => Err(OLDER_JOURNAL.to_owned()),
        _ => Err(body["error"]
            .as_str()
            .map(|error| format!("your journal could not make a link: {error}"))
            .unwrap_or_else(|| "your journal could not make a link. try again.".to_owned())),
    }
}

#[cfg(windows)]
fn mint(journal: &std::path::Path, path: &str) -> Result<String, String> {
    use solstone_core_local_endpoint as endpoint;
    let root = std::fs::canonicalize(journal).unwrap_or_else(|_| journal.to_owned());
    let pipe = endpoint::socket_for_journal(&root)
        .ok()
        .flatten()
        .ok_or_else(|| NOT_RUNNING.to_owned())?;
    let timeout = std::time::Duration::from_secs(5);
    let request = serde_json::json!({ "path": path }).to_string();
    let response = endpoint::pipe_request(
        &pipe,
        "POST",
        "/_solstone/browser-link",
        Some(request.as_bytes()),
        timeout,
    )
    .map_err(|_| NOT_RUNNING.to_owned())?;
    let body: serde_json::Value = serde_json::from_slice(&response.body).unwrap_or_default();
    match response.status {
        200 => body["url"].as_str().map(str::to_owned).ok_or_else(|| {
            "the response from your journal could not be read. try again.".to_owned()
        }),
        404 => Err(OLDER_JOURNAL.to_owned()),
        _ => Err(body["error"]
            .as_str()
            .map(|error| format!("your journal could not make a link: {error}"))
            .unwrap_or_else(|| "your journal could not make a link. try again.".to_owned())),
    }
}

/// Hand the link to the default browser.
#[cfg(windows)]
fn launch(url: &str) -> bool {
    solstone_core_local_endpoint::open_in_browser(url)
}

/// Hand the link to the default browser. True once a launcher accepted it.
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn launch(url: &str) -> bool {
    use std::process::{Command, Stdio};
    let launchers: &[(&str, &[&str])] = if cfg!(target_os = "macos") {
        &[("open", &[])]
    } else {
        &[("xdg-open", &[]), ("gio", &["open"])]
    };
    launchers.iter().any(|(program, args)| {
        Command::new(program)
            .args(*args)
            .arg(url)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    })
}

#[cfg(test)]
mod tests {
    use super::rooted;

    #[test]
    fn paths_are_rooted() {
        assert_eq!(rooted(""), "/");
        assert_eq!(rooted("init"), "/init");
        assert_eq!(rooted("/app/backup"), "/app/backup");
    }
}
