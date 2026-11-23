// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use fluent_langneg::{NegotiationStrategy, negotiate_languages};
use std::sync::OnceLock;
use unic_langid::LanguageIdentifier;

#[allow(dead_code, clippy::all)]
mod compiled {
    include!("../../../../i18n/generated/windows-app/rust/journal-windows-app/api.rs");
}

pub(crate) const ENGLISH: &str = "en-US";
const AVAILABLE: &[&str] = include!(concat!(env!("OUT_DIR"), "/languages.rs"));

pub(crate) fn non_english_locale_for(tag: &str) -> Option<&str> {
    (tag != ENGLISH).then_some(tag)
}

pub fn locale() -> &'static str {
    static TAG: OnceLock<String> = OnceLock::new();
    TAG.get_or_init(|| {
        let requested: Vec<LanguageIdentifier> = preferred_languages()
            .iter()
            .filter_map(|s| s.parse().ok())
            .collect();
        let available: Vec<LanguageIdentifier> = AVAILABLE
            .iter()
            .map(|s| s.parse().expect("catalog language"))
            .collect();
        let fallback = ENGLISH.parse().expect("English language tag");
        negotiate_languages(
            &requested,
            &available,
            Some(&fallback),
            NegotiationStrategy::Lookup,
        )[0]
        .to_string()
    })
}

pub fn text(key: &str) -> String {
    strings(key, &[])
}

pub fn strings(key: &str, args: &[(&str, &str)]) -> String {
    let values: Vec<_> = args
        .iter()
        .map(|&(name, value)| (name, compiled::Value::String(value)))
        .collect();
    render(key, &values)
}

pub fn byte_count(count: u16) -> String {
    render(
        "size.bytes",
        &[("quantity", compiled::Value::Cardinal(i64::from(count)))],
    )
}

fn render(key: &str, values: &[(&str, compiled::Value<'_>)]) -> String {
    let tag = locale();
    compiled::render_dotted_with_options(
        tag,
        &format!("journal.windows.{key}"),
        values,
        tag != ENGLISH,
    )
    .unwrap_or_else(|error| panic!("Windows journal app catalog {key}: {error:?}"))
}

#[cfg(windows)]
fn preferred_languages() -> Vec<String> {
    use windows::System::UserProfile::GlobalizationPreferences;
    if let Ok(languages) = GlobalizationPreferences::Languages()
        && let Ok(size) = languages.Size()
    {
        let result: Vec<String> = (0..size)
            .filter_map(|i| languages.GetAt(i).ok().map(|s| s.to_string_lossy()))
            .collect();
        if !result.is_empty() {
            return result;
        }
    }
    use windows::Win32::Globalization::{GetUserPreferredUILanguages, MUI_LANGUAGE_NAME};
    // Settings can change between the size and fill calls. Retry with a newly
    // sized buffer; never treat a partial MULTI_SZ as the user's complete list.
    for _ in 0..3 {
        let mut count = 0;
        let mut length = 0;
        // SAFETY: the first call measures the buffer, and the second supplies that many UTF-16 units.
        unsafe {
            if GetUserPreferredUILanguages(MUI_LANGUAGE_NAME, &mut count, None, &mut length)
                .is_err()
            {
                break;
            }
            let mut buffer = vec![0u16; length as usize];
            if GetUserPreferredUILanguages(
                MUI_LANGUAGE_NAME,
                &mut count,
                Some(windows::core::PWSTR(buffer.as_mut_ptr())),
                &mut length,
            )
            .is_ok()
            {
                return buffer
                    .split(|c| *c == 0)
                    .filter(|s| !s.is_empty())
                    .filter_map(|s| String::from_utf16(s).ok())
                    .collect();
            }
        }
    }
    Vec::new()
}

#[cfg(not(windows))]
fn preferred_languages() -> Vec<String> {
    Vec::new()
}
