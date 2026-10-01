// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! `journal-app.exe`: the Windows journal app. It is what the Start menu
//! opens, what sits in the taskbar, and what the installer runs: it keeps the
//! journal running on this PC, shows its name, mark and run state, takes a new
//! owner through the first run, and updates itself and the journal together.
//! The journal itself still opens in the owner's browser.

#![cfg_attr(windows, windows_subsystem = "windows")]
// The portable modules serve the Windows app; elsewhere only their tests use them.
#![cfg_attr(not(windows), allow(dead_code))]

mod ico;
mod setup_events;
mod status;

#[cfg(windows)]
mod app;
#[cfg(windows)]
mod convey;
#[cfg(windows)]
mod journal;
#[cfg(windows)]
mod prefs;
#[cfg(windows)]
mod shell;
#[cfg(windows)]
mod update;
#[cfg(windows)]
mod webview;
#[cfg(windows)]
mod window;

#[cfg(windows)]
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args
        .first()
        .is_some_and(|first| first.starts_with("--veloapp-"))
    {
        std::process::exit(run_install_hook(&args));
    }
    velopack::VelopackApp::build()
        .set_auto_apply_on_startup(false)
        .run();
    let launch = if args.iter().any(|arg| arg == "--sign-in") {
        app::Launch::SignIn
    } else if args.iter().any(|arg| arg == "--after-update") {
        app::Launch::AfterUpdate
    } else {
        app::Launch::Plain
    };
    let update_feed = args
        .windows(2)
        .find(|pair| pair[0] == "--update-feed")
        .map(|pair| pair[1].clone());
    app::run(launch, update_feed);
}

/// The installer runs its install, update and uninstall steps in this
/// program, because it is the one the Start menu opens. Those steps belong to
/// the journal, so they run in `journal.exe` exactly as before; the app only
/// adds removing its own sign-in entry on uninstall.
#[cfg(windows)]
fn run_install_hook(args: &[String]) -> i32 {
    use std::os::windows::process::CommandExt;

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    if args[0].eq_ignore_ascii_case("--veloapp-uninstall") {
        let _ = shell::set_sign_in_launch(false);
    }
    std::process::Command::new(journal::journal_exe())
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .status()
        .ok()
        .and_then(|status| status.code())
        .unwrap_or(1)
}

#[cfg(not(windows))]
fn main() {
    eprintln!("journal-app is the Windows journal app; on this system, use `journal`.");
    std::process::exit(64);
}
