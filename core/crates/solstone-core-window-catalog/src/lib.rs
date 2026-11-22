// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Render packaged journal-window messages.

#[allow(clippy::all, dead_code)]
mod compiled {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../i18n/generated/window/rust/journal-window/api.rs"
    ));
}

pub const ENGLISH: &str = "en-US";

pub use compiled::Value;

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

pub fn text(locale: &str, id: &str) -> String {
    render(locale, id, &[])
}

pub fn screen_category_label(locale: &str, name: &str) -> Option<String> {
    let id = match name {
        "browsing" => "journal.window.settings.vision.category.label.browsing",
        "calendar" => "journal.window.settings.vision.category.label.calendar",
        "code" => "journal.window.settings.vision.category.label.code",
        "gaming" => "journal.window.settings.vision.category.label.gaming",
        "media" => "journal.window.settings.vision.category.label.media",
        "meeting" => "journal.window.settings.vision.category.label.meeting",
        "messaging" => "journal.window.settings.vision.category.label.messaging",
        "productivity" => "journal.window.settings.vision.category.label.productivity",
        "reading" => "journal.window.settings.vision.category.label.reading",
        "social" => "journal.window.settings.vision.category.label.social",
        "terminal" => "journal.window.settings.vision.category.label.terminal",
        _ => return None,
    };
    Some(text(locale, id))
}

pub fn strings<const N: usize>(locale: &str, id: &str, args: [(&str, &str); N]) -> String {
    render(
        locale,
        id,
        &args.map(|(name, value)| (name, Value::String(value))),
    )
}

/// Resolve message IDs in a trusted copy table while preserving its keys.
pub fn resolve_table(locale: &str, table: &mut serde_json::Value) {
    match table {
        serde_json::Value::String(value)
            if value.starts_with("journal.window.") || value.starts_with("!journal.window.") =>
        {
            *value = text(locale, value);
        }
        serde_json::Value::Array(values) => {
            for value in values {
                resolve_table(locale, value);
            }
        }
        serde_json::Value::Object(values) => {
            for value in values.values_mut() {
                resolve_table(locale, value);
            }
        }
        _ => {}
    }
}

pub fn render(locale: &str, id: &str, arguments: &[(&str, Value<'_>)]) -> String {
    let (locale, id) = id
        .strip_prefix('!')
        .map_or((locale, id), |id| (ENGLISH, id));
    compiled::render_dotted_with_options(locale, id, arguments, locale != ENGLISH)
        .unwrap_or_else(|error| panic!("window catalog message {id} for {locale}: {error:?}"))
}

pub fn language_tag(locale: &str) -> &str {
    if locale == ENGLISH { "en" } else { locale }
}

pub fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

pub fn html(locale: &str, id: &str) -> String {
    escape_html(&text(locale, id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_packaged_catalog_renders_on_a_small_stack() {
        // Windows debug executables have a 1 MiB main stack. Leave room for
        // the shell build script or service that calls the renderer.
        std::thread::Builder::new()
            .stack_size(512 * 1024)
            .spawn(|| {
                assert_eq!(
                    text(
                        ENGLISH,
                        "journal.window.agents.permission.kind.screen.text.label"
                    ),
                    "screen text"
                );
                assert_eq!(
                    strings(
                        ENGLISH,
                        "journal.window.shared.api.request.failed.http",
                        [("http_status", "502")]
                    ),
                    "Request failed (HTTP 502)"
                );
            })
            .unwrap()
            .join()
            .unwrap();
    }
}
