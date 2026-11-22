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
pub use solstone_core_window_catalog::{ENGLISH, cli_locale};

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

/// Render terminal messages whose arguments use Rust's display formatting.
pub fn display<const N: usize>(id: &str, args: [(&str, &dyn std::fmt::Display); N]) -> String {
    display_for(cli_locale(), id, args)
}

pub fn display_for<const N: usize>(
    locale: &str,
    id: &str,
    args: [(&str, &dyn std::fmt::Display); N],
) -> String {
    let values = args.map(|(name, value)| (name, value.to_string()));
    render_for(
        locale,
        id,
        &values
            .each_ref()
            .map(|(name, value)| (*name, Value::String(value))),
    )
}

/// Keep diagnostic facts available when a probe result outlives its caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    id: &'static str,
    values: Vec<(&'static str, String)>,
}

impl Diagnostic {
    pub fn id(&self) -> &'static str {
        self.id
    }

    pub fn new<const N: usize>(
        id: &'static str,
        values: [(&'static str, &dyn std::fmt::Display); N],
    ) -> Self {
        Self {
            id,
            values: values
                .into_iter()
                .map(|(name, value)| (name, value.to_string()))
                .collect(),
        }
    }

    pub fn render(&self, locale: &str) -> String {
        let args: Vec<_> = self
            .values
            .iter()
            .map(|(name, value)| (*name, Value::String(value)))
            .collect();
        render_for(locale, self.id, &args)
    }
}
