// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

pub(crate) fn host_locale() -> &'static str {
    static LOCALE: std::sync::OnceLock<&'static str> = std::sync::OnceLock::new();
    LOCALE.get_or_init(|| {
        solstone_core_cli::parser_locale(&std::env::args_os().skip(1).collect::<Vec<_>>())
    })
}

pub(crate) fn host_text(id: &str) -> String {
    solstone_core_cli_copy::render_for(host_locale(), id, &[])
}

pub(crate) fn host_display<const N: usize>(
    id: &str,
    arguments: [(&str, &dyn std::fmt::Display); N],
) -> String {
    solstone_core_cli_copy::display_for(host_locale(), id, arguments)
}
