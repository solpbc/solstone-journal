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
pub use solstone_core_window_catalog::ENGLISH;

pub fn packaged_locale(requested: Option<&str>) -> &'static str {
    const LOCALES: &[&str] = include!(concat!(env!("OUT_DIR"), "/locales.rs"));
    requested
        .and_then(|tag| {
            LOCALES
                .iter()
                .copied()
                .find(|locale| locale.eq_ignore_ascii_case(tag))
        })
        .unwrap_or(ENGLISH)
}

pub fn cli_locale() -> &'static str {
    packaged_locale(std::env::var("SOLSTONE_LOCALE").ok().as_deref())
}

pub fn terminal_locale() -> Option<&'static str> {
    if std::env::var("SOL_SUPERVISOR_SPAWNED").as_deref() == Ok("1") {
        None
    } else {
        Some(cli_locale())
    }
}

static TERMINAL_LOCALE: std::sync::OnceLock<&'static str> = std::sync::OnceLock::new();

/// Select the language for log messages in a command and its worker threads.
pub fn begin_terminal_output() {
    TERMINAL_LOCALE.get_or_init(|| terminal_locale().unwrap_or(ENGLISH));
}

pub fn manual_log_locale() -> &'static str {
    TERMINAL_LOCALE.get().copied().unwrap_or(ENGLISH)
}

fn checked_render(
    locale: &str,
    id: &str,
    args: &[(&str, Value<'_>)],
) -> Result<String, compiled::RenderError> {
    let (locale, id) = id
        .strip_prefix('!')
        .map_or((locale, id), |id| (ENGLISH, id));
    // Terminal output includes copyable commands and aligned option columns.
    let mut result = compiled::render_dotted_with_options(locale, id, args, false);
    if matches!(result, Err(compiled::RenderError::UnknownLocale)) {
        result = compiled::render_dotted_with_options(ENGLISH, id, args, false);
    }
    result
}

pub fn render_for(locale: &str, id: &str, args: &[(&str, Value<'_>)]) -> String {
    checked_render(locale, id, args)
        .unwrap_or_else(|error| panic!("CLI catalog message {id} for {locale}: {error:?}"))
}

pub fn try_render_for(locale: &str, id: &str, args: &[(&str, Value<'_>)]) -> Option<String> {
    checked_render(locale, id, args).ok()
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

    pub fn fields(&self) -> &[(&'static str, String)] {
        &self.values
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

/// A diagnostic detail that can cross a record boundary before it reaches a terminal.
/// External messages retain their original text. Journal messages keep their arguments.
#[derive(Clone, PartialEq, Eq)]
pub struct MessageDetail(Box<DetailValue>);

#[derive(Clone, PartialEq, Eq)]
enum DetailValue {
    External(String),
    Catalog(Diagnostic),
    Window(Diagnostic),
    Count {
        message: Diagnostic,
        counts: Vec<(&'static str, i64)>,
        source: String,
    },
    Multiple {
        id: &'static str,
        fields: Vec<(&'static str, MessageDetail)>,
    },
    Joined {
        items: Vec<MessageDetail>,
        separator: &'static str,
    },
    Context {
        window: bool,
        slot: &'static str,
        message: Diagnostic,
        detail: MessageDetail,
    },
}

impl MessageDetail {
    pub fn message<const N: usize>(
        id: &'static str,
        args: [(&'static str, &dyn std::fmt::Display); N],
    ) -> Self {
        Self(Box::new(DetailValue::Catalog(Diagnostic::new(id, args))))
    }

    pub fn catalog_fields(&self) -> Option<(&'static str, &[(&'static str, String)])> {
        match self.0.as_ref() {
            DetailValue::Catalog(message) | DetailValue::Window(message) => {
                Some((message.id, &message.values))
            }
            _ => None,
        }
    }

    /// Return the nested diagnostic in a context message without rendering its cause.
    pub fn context_fields(&self) -> Option<(&Diagnostic, &'static str, &MessageDetail)> {
        if let DetailValue::Context {
            slot,
            message,
            detail,
            ..
        } = self.0.as_ref()
        {
            Some((message, *slot, detail))
        } else {
            None
        }
    }

    /// Numeric arguments retain their type across a stored diagnostic boundary.
    pub fn cardinal_fields(&self) -> Option<(&Diagnostic, &[(&'static str, i64)])> {
        if let DetailValue::Count {
            message, counts, ..
        } = self.0.as_ref()
        {
            Some((message, counts))
        } else {
            None
        }
    }

    pub fn window<const N: usize>(
        id: &'static str,
        args: [(&'static str, &dyn std::fmt::Display); N],
    ) -> Self {
        Self(Box::new(DetailValue::Window(Diagnostic::new(id, args))))
    }

    pub fn cardinal<const C: usize, const N: usize>(
        id: &'static str,
        counts: [(&'static str, i64); C],
        source: String,
        args: [(&'static str, &dyn std::fmt::Display); N],
    ) -> Self {
        Self(Box::new(DetailValue::Count {
            message: Diagnostic::new(id, args),
            counts: counts.into(),
            source,
        }))
    }

    /// Preserve each diagnostic's origin when several failures form one message.
    pub fn with_details<const N: usize>(
        id: &'static str,
        fields: [(&'static str, Self); N],
    ) -> Self {
        Self(Box::new(DetailValue::Multiple {
            id,
            fields: fields.into(),
        }))
    }

    pub fn joined(items: Vec<Self>, separator: &'static str) -> Self {
        Self(Box::new(DetailValue::Joined { items, separator }))
    }

    pub fn context(self, id: &'static str) -> Self {
        self.context_with(id, [])
    }

    pub fn context_with<const N: usize>(
        self,
        id: &'static str,
        args: [(&'static str, &dyn std::fmt::Display); N],
    ) -> Self {
        self.context_slot(id, "detail", args)
    }

    pub fn context_slot<const N: usize>(
        self,
        id: &'static str,
        slot: &'static str,
        args: [(&'static str, &dyn std::fmt::Display); N],
    ) -> Self {
        Self(Box::new(DetailValue::Context {
            window: false,
            slot,
            message: Diagnostic::new(id, args),
            detail: self,
        }))
    }

    pub fn window_context_slot<const N: usize>(
        self,
        id: &'static str,
        slot: &'static str,
        args: [(&'static str, &dyn std::fmt::Display); N],
    ) -> Self {
        Self(Box::new(DetailValue::Context {
            window: true,
            slot,
            message: Diagnostic::new(id, args),
            detail: self,
        }))
    }

    pub fn render(&self, locale: &str) -> String {
        self.render_for_output(Some(locale))
    }

    /// Keep native English response fields byte-compatible with older clients.
    pub fn render_for_response(&self, requested: &str) -> String {
        let locale = packaged_locale(Some(requested));
        self.render_for_output((locale != ENGLISH).then_some(locale))
    }

    /// A machine reader gets the stable source spelling even after a terminal grammar fix.
    pub fn render_for_output(&self, requested: Option<&str>) -> String {
        let locale = requested.unwrap_or(ENGLISH);
        match self.0.as_ref() {
            DetailValue::External(text) => text.clone(),
            DetailValue::Catalog(diagnostic) => diagnostic.render(locale),
            DetailValue::Window(diagnostic) => {
                let args = diagnostic
                    .values
                    .iter()
                    .map(|(name, value)| {
                        (*name, solstone_core_window_catalog::Value::String(value))
                    })
                    .collect::<Vec<_>>();
                solstone_core_window_catalog::render(locale, diagnostic.id, &args)
            }
            DetailValue::Count { source, .. } if requested.is_none() => source.clone(),
            DetailValue::Count {
                message, counts, ..
            } => {
                let mut values: Vec<_> = message
                    .values
                    .iter()
                    .map(|(name, value)| (*name, Value::String(value)))
                    .collect();
                values.extend(
                    counts
                        .iter()
                        .map(|(name, count)| (*name, Value::Cardinal(*count))),
                );
                render_for(locale, message.id, &values)
            }
            DetailValue::Multiple { id, fields } => {
                let rendered: Vec<_> = fields
                    .iter()
                    .map(|(name, detail)| (*name, detail.render_for_output(requested)))
                    .collect();
                let values: Vec<_> = rendered
                    .iter()
                    .map(|(name, text)| (*name, Value::String(text)))
                    .collect();
                render_for(locale, id, &values)
            }
            DetailValue::Joined { items, separator } => items
                .iter()
                .map(|item| item.render_for_output(requested))
                .collect::<Vec<_>>()
                .join(separator),
            DetailValue::Context {
                window,
                slot,
                message,
                detail,
            } => {
                let cause = detail.render_for_output(requested);
                if *window {
                    let mut values: Vec<_> = message
                        .values
                        .iter()
                        .map(|(name, value)| {
                            (*name, solstone_core_window_catalog::Value::String(value))
                        })
                        .collect();
                    values.push((*slot, solstone_core_window_catalog::Value::String(&cause)));
                    return solstone_core_window_catalog::render(
                        solstone_core_window_catalog::packaged_locale(Some(locale)),
                        message.id,
                        &values,
                    );
                }
                let mut values: Vec<_> = message
                    .values
                    .iter()
                    .map(|(name, value)| (*name, Value::String(value)))
                    .collect();
                values.push((*slot, Value::String(&cause)));
                render_for(locale, message.id, &values)
            }
        }
    }
}

impl From<String> for MessageDetail {
    fn from(value: String) -> Self {
        Self(Box::new(DetailValue::External(value)))
    }
}

impl From<&str> for MessageDetail {
    fn from(value: &str) -> Self {
        value.to_owned().into()
    }
}

impl std::fmt::Display for MessageDetail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.render_for_output(None))
    }
}

// Some diagnostic logs display enclosing errors with Debug. Keep their original
// quoted source string there as well as in Display's unquoted machine output.
impl std::fmt::Debug for MessageDetail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(&self.render_for_output(None), f)
    }
}

impl std::error::Error for MessageDetail {}

#[cfg(test)]
mod tests {
    use super::{Diagnostic, ENGLISH};

    #[test]
    fn saved_diagnostic_renders_its_detail_when_the_locale_is_unknown() {
        let diagnostic = {
            let detail = String::from("catalog fixture detail");
            Diagnostic::new("journal.cli.doctor.context.failed", [("error", &detail)])
        };
        let english = diagnostic.render(ENGLISH);
        assert!(english.contains("catalog fixture detail"));
        assert_eq!(diagnostic.render("x-catalog-fixture"), english);
    }
}
