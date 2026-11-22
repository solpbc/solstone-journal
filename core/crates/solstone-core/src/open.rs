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
use crate::{EXIT_TEMPFAIL, resolve_process_journal_path};

/// The path to open: what the owner typed, rooted at `/`.
fn rooted(path: &str) -> String {
    if path.starts_with('/') {
        path.to_owned()
    } else {
        format!("/{path}")
    }
}

pub(crate) fn run(path: String, print_url: bool) -> ExitCode {
    let locale = if print_url {
        solstone_core_cli_copy::ENGLISH
    } else {
        crate::host_locale()
    };
    let journal = match resolve_process_journal_path() {
        Ok(journal) => journal,
        Err(error) => {
            crate::eprint_journal_path_error_in(locale, error);
            return ExitCode::from(EXIT_TEMPFAIL);
        }
    };
    let path = rooted(&path);
    #[cfg(any(target_os = "linux", target_os = "macos", windows))]
    {
        let url = match mint(locale, &journal.path, &path) {
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
            "{}",
            solstone_core_cli_copy::render_for(
                crate::host_locale(),
                "journal.cli.host.open.run.browser.could.not.be.opened",
                &[]
            )
        );
        if let Some(port) = link_port(&url) {
            eprintln!(
                "{}",
                solstone_core_cli_copy::render_for(
                    crate::host_locale(),
                    "journal.cli.host.open.run.open.it.from.another.computer",
                    &[]
                )
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
            "{}",
            solstone_core_cli_copy::display_for(
                crate::host_locale(),
                "journal.cli.host.open.run.open.url.path",
                [
                    ("url", &crate::settings::journal_url(&journal.path)),
                    ("path", &path)
                ]
            )
        );
        ExitCode::SUCCESS
    }
}

// A mac journal runs inside the journal app, so the app is what starts it.
#[cfg(target_os = "macos")]
const NOT_RUNNING: &str = "journal.cli.host.open.socket.mac.down.isn.t.running.for.this";
#[cfg(target_os = "linux")]
const NOT_RUNNING: &str = "journal.cli.host.open.socket.linux.down.isn.t.running.for.this";
#[cfg(target_os = "macos")]
const OLDER_JOURNAL: &str = "journal.cli.host.open.socket.mac.old.running.older.than.this.command";
#[cfg(windows)]
const NOT_RUNNING: &str = "journal.cli.host.open.socket.windows.down.isn.t.running.for.this";
#[cfg(target_os = "linux")]
const OLDER_JOURNAL: &str =
    "journal.cli.host.open.socket.linux.old.running.older.than.this.command";
#[cfg(windows)]
const OLDER_JOURNAL: &str =
    "journal.cli.host.open.socket.windows.old.running.older.than.this.command";

/// The port of a minted link, for the SSH forwarding line.
#[cfg(any(target_os = "linux", target_os = "macos", windows))]
fn link_port(url: &str) -> Option<&str> {
    let port = url.strip_prefix("http://localhost:")?.split('/').next()?;
    (!port.is_empty() && port.bytes().all(|byte| byte.is_ascii_digit())).then_some(port)
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn mint(locale: &str, journal: &std::path::Path, path: &str) -> Result<String, String> {
    use solstone_core_local_endpoint as endpoint;
    let root = std::fs::canonicalize(journal).unwrap_or_else(|_| journal.to_owned());
    let socket = endpoint::socket_for_journal(&root)
        .ok()
        .flatten()
        .filter(|socket| {
            endpoint::owned_socket(socket) && endpoint::socket_belongs_to_journal(socket, &root)
        })
        .ok_or_else(|| solstone_core_cli_copy::render_for(locale, NOT_RUNNING, &[]))?;
    let agent = endpoint::socket_agent(&socket, std::time::Duration::from_secs(5));
    let mut response = agent
        .post("http://localhost/_solstone/browser-link")
        .header(
            "User-Agent",
            concat!("solstone/", env!("CARGO_PKG_VERSION")),
        )
        .header("Content-Type", "application/json")
        .send(serde_json::json!({ "path": path }).to_string())
        .map_err(|_| solstone_core_cli_copy::render_for(locale, NOT_RUNNING, &[]))?;
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
            solstone_core_cli_copy::render_for(
                locale,
                "journal.cli.host.open.mint.response.from.could.not.be",
                &[],
            )
        }),
        404 => Err(solstone_core_cli_copy::render_for(
            locale,
            OLDER_JOURNAL,
            &[],
        )),
        _ => Err(body["error"]
            .as_str()
            .map(|error| {
                solstone_core_cli_copy::display_for(
                    locale,
                    "journal.cli.host.open.mint.could.not.make.link",
                    [("error", &error)],
                )
            })
            .unwrap_or_else(|| {
                solstone_core_cli_copy::render_for(
                    locale,
                    "journal.cli.host.open.mint.could.not.make.link.try",
                    &[],
                )
            })),
    }
}

#[cfg(windows)]
fn mint(locale: &str, journal: &std::path::Path, path: &str) -> Result<String, String> {
    use solstone_core_local_endpoint as endpoint;
    let root = std::fs::canonicalize(journal).unwrap_or_else(|_| journal.to_owned());
    let pipe = endpoint::socket_for_journal(&root)
        .ok()
        .flatten()
        .ok_or_else(|| solstone_core_cli_copy::render_for(locale, NOT_RUNNING, &[]))?;
    let timeout = std::time::Duration::from_secs(5);
    let request = serde_json::json!({ "path": path }).to_string();
    let response = endpoint::pipe_request(
        &pipe,
        "POST",
        "/_solstone/browser-link",
        Some(request.as_bytes()),
        timeout,
    )
    .map_err(|_| solstone_core_cli_copy::render_for(locale, NOT_RUNNING, &[]))?;
    let body: serde_json::Value = serde_json::from_slice(&response.body).unwrap_or_default();
    match response.status {
        200 => body["url"].as_str().map(str::to_owned).ok_or_else(|| {
            solstone_core_cli_copy::render_for(
                locale,
                "journal.cli.host.open.mint.response.from.could.not.be",
                &[],
            )
        }),
        404 => Err(solstone_core_cli_copy::render_for(
            locale,
            OLDER_JOURNAL,
            &[],
        )),
        _ => Err(body["error"]
            .as_str()
            .map(|error| {
                solstone_core_cli_copy::display_for(
                    locale,
                    "journal.cli.host.open.mint.could.not.make.link",
                    [("error", &error)],
                )
            })
            .unwrap_or_else(|| {
                solstone_core_cli_copy::render_for(
                    locale,
                    "journal.cli.host.open.mint.could.not.make.link.try",
                    &[],
                )
            })),
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
