// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

pub fn hostname() -> String {
    let value = std::env::var("HOSTNAME")
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| {
            std::fs::read_to_string("/etc/hostname").unwrap_or_else(|_| "unknown".to_owned())
        });
    stream_name(&value).unwrap_or_else(|| "unknown".to_owned())
}

fn stream_name(host: &str) -> Option<String> {
    let parts = host.trim().split('.').collect::<Vec<_>>();
    let base = if parts
        .iter()
        .filter(|part| !part.is_empty())
        .all(|part| part.chars().all(char::is_numeric))
    {
        parts
            .iter()
            .filter(|part| !part.is_empty())
            .copied()
            .collect::<Vec<_>>()
            .join("-")
    } else {
        parts.first().copied().unwrap_or_default().to_owned()
    };
    let mut output = String::new();
    let mut separator = false;
    for character in base.trim().to_ascii_lowercase().chars() {
        if character.is_whitespace() || matches!(character, '/' | '\\') {
            separator = true;
        } else {
            if separator && !output.is_empty() {
                output.push('-');
            }
            output.push(character);
            separator = false;
        }
    }
    let valid = !output.is_empty()
        && !output.contains("..")
        && output.chars().enumerate().all(|(index, character)| {
            character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || (index != 0 && matches!(character, '.' | '_' | '-'))
        });
    valid.then_some(output)
}

#[cfg(test)]
mod tests {
    use super::stream_name;

    #[test]
    fn hostname_matches_python_stream_name_normalization() {
        assert_eq!(stream_name("192.168.1.1").as_deref(), Some("192-168-1-1"));
        assert_eq!(stream_name("My Host").as_deref(), Some("my-host"));
        assert_eq!(stream_name("bad@host"), None);
    }
}
