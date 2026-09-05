// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::ffi::OsStr;
use std::process::Command;

// Cargo supplies package metadata to `run` and `test` processes. Reusing that
// metadata as a nested Cargo invocation's input invalidates build scripts that
// track it, including ring. Preserve caller-owned Cargo/build/runtime settings.
pub fn clear_package_environment(command: &mut Command) {
    let keys: Vec<_> = std::env::vars_os()
        .map(|(key, _)| key)
        .chain(command.get_envs().map(|(key, _)| key.to_os_string()))
        .filter(|key| {
            key.to_str().is_some_and(|key| {
                key.starts_with("CARGO_PKG_")
                    || matches!(
                        key,
                        "CARGO_MANIFEST_DIR" | "CARGO_MANIFEST_PATH" | "CARGO_MANIFEST_LINKS"
                    )
            })
        })
        .collect();
    for key in keys {
        command.env_remove(key);
    }
}

pub fn cargo_command(program: impl AsRef<OsStr>) -> Command {
    let mut command = Command::new(program);
    clear_package_environment(&mut command);
    command
}
