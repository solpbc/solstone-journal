// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Admission for request-supplied speaker transcript stems.

/// Return whether `source` can name one speaker transcript/embedding stem.
///
/// Speaker artifacts retain legacy colons such as `mic:audio`, so this is
/// deliberately narrower than the portable writer-name policy. It still
/// refuses every spelling that could change the joined path on Unix or
/// Windows.
pub(crate) fn is_safe_source_component(source: &str) -> bool {
    if source.is_empty()
        || matches!(source, "." | "..")
        || source.contains(['/', '\\', '\0'])
        || contains_encoded_path_control(source)
    {
        return false;
    }
    let bytes = source.as_bytes();
    !matches!(bytes, [drive, b':', ..] if drive.is_ascii_alphabetic())
}

fn contains_encoded_path_control(source: &str) -> bool {
    source.as_bytes().windows(3).any(|escape| {
        escape[0] == b'%'
            && matches!(
                (
                    escape[1].to_ascii_lowercase(),
                    escape[2].to_ascii_lowercase()
                ),
                (b'2', b'f') | (b'5', b'c') | (b'0', b'0')
            )
    })
}

#[cfg(test)]
mod tests {
    use super::is_safe_source_component;

    #[test]
    fn source_component_keeps_legacy_colons_but_refuses_path_spellings() {
        assert!(is_safe_source_component("mic:audio"));
        assert!(is_safe_source_component("audio"));
        for source in [
            "",
            ".",
            "..",
            "../outside",
            "..%2Foutside",
            "..%5Coutside",
            "/outside",
            r"..\outside",
            "C:outside",
            "a\0b",
            "a%00b",
        ] {
            assert!(!is_safe_source_component(source), "{source:?}");
        }
    }
}
