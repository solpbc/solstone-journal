// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Render the journal's command-line catalog for the invoking process.

#[allow(clippy::all, dead_code)]
mod compiled {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../i18n/generated/cli/rust/journal-cli/api.rs"
    ));
}

pub use compiled::Value;
use solstone_core_window_catalog::{ENGLISH, cli_locale};

pub fn render_for(locale: &str, id: &str, args: &[(&str, Value<'_>)]) -> String {
    let (locale, id) = id
        .strip_prefix('!')
        .map_or((locale, id), |id| (ENGLISH, id));
    // Terminal output includes copyable commands and aligned option columns.
    let mut result = compiled::render_dotted_with_options(locale, id, args, false);
    if matches!(result, Err(compiled::RenderError::UnknownLocale)) {
        result = compiled::render_dotted_with_options(ENGLISH, id, args, false);
    }
    result.unwrap_or_else(|error| panic!("CLI catalog message {id} for {locale}: {error:?}"))
}

pub fn render(id: &str, args: &[(&str, Value<'_>)]) -> String {
    render_for(cli_locale(), id, args)
}

pub fn text(id: &str) -> String {
    render(id, &[])
}

pub fn strings<const N: usize>(id: &str, args: [(&str, &str); N]) -> String {
    render(id, &args.map(|(name, value)| (name, Value::String(value))))
}
