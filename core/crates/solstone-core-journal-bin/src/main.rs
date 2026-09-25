// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::env;
use std::process::ExitCode;

fn install_logger() {
    let _ = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn"))
        .try_init();
}

fn main() -> ExitCode {
    #[cfg(windows)]
    if env::args_os().all(|arg| arg.to_str().is_some()) {
        // The SDK reads Unicode args during construction. Leave non-Unicode
        // arguments to the journal's existing OsString boundary below.
        velopack::VelopackApp::build()
            .set_auto_apply_on_startup(false)
            .on_after_install_fast_callback(|_| {
                solstone_core_journal_cli::add_commands_to_owner_path();
                solstone_core_journal_cli::resume_service_after_update();
            })
            .on_after_update_fast_callback(|_| {
                solstone_core_journal_cli::resume_service_after_update();
            })
            .on_before_uninstall_fast_callback(|_| {
                solstone_core_journal_cli::remove_commands_from_owner_path();
                remove_service_before_uninstall();
            })
            .run();
    }
    install_logger();
    solstone_core_journal_cli::run(env::args_os().skip(1).collect())
}

#[cfg(windows)]
fn remove_service_before_uninstall() {
    use std::os::windows::process::CommandExt;

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let result = std::env::current_exe().and_then(|journal| {
        let core = journal.with_file_name("solstone-core.exe");
        std::process::Command::new(core)
            .args(["service", "__before-uninstall"])
            .creation_flags(CREATE_NO_WINDOW)
            .output()
    });
    match result {
        Ok(output) if output.status.success() => {}
        Ok(output) => {
            eprintln!(
                "journal service cleanup failed during uninstall: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            std::process::exit(1);
        }
        Err(error) => {
            eprintln!("journal service cleanup could not start during uninstall: {error}");
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn logger_install_is_idempotent_and_defaults_to_warn() {
        super::install_logger();
        super::install_logger();
        if std::env::var("RUST_LOG").is_err() {
            assert!(log::max_level() >= log::LevelFilter::Warn);
        }
    }
}
