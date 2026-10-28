// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

/// Render untrusted text literally when it is displayed as Markdown.
///
/// Escape each value independently, after slicing or pagination. A code fence
/// can lose its opening delimiter when a value is split into separate entries.
pub fn literal_text(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            _ if ch.is_ascii_punctuation() => {
                escaped.push('\\');
                escaped.push(ch);
            }
            _ => escaped.push(ch),
        }
    }
    escaped
}

/// Render one untrusted value literally, leaving it as written when it cannot
/// carry a link, image or HTML.
///
/// Markdown links and images and HTML need at least one of these delimiters,
/// so ordinary text, ids, paths and URLs keep their bytes.
pub fn literal_markup(text: &str) -> String {
    if text.contains(['[', '<', '&']) {
        literal_text(text)
    } else {
        text.to_owned()
    }
}

/// Keep JSON text inert when it is itself displayed as Markdown.
///
/// JSON doubles the backslashes [`literal_text`] adds, so two escaped values
/// could still meet as a link in the raw text. Escape the delimiters inside
/// every JSON string as `\u` sequences; parsed values are unchanged.
pub fn literal_json_syntax(json: &str) -> String {
    let mut output = String::with_capacity(json.len());
    let mut in_string = false;
    let mut escaped = false;
    for ch in json.chars() {
        if ch == '"' && !escaped {
            in_string = !in_string;
        }
        if in_string && matches!(ch, '[' | ']' | '(' | ')' | '!' | '<' | '>' | '&' | '`') {
            use std::fmt::Write;
            write!(output, "\\u{:04x}", u32::from(ch)).expect("string write");
        } else {
            output.push(ch);
        }
        escaped = ch == '\\' && !escaped;
    }
    output
}

#[cfg(test)]
mod tests {
    use pulldown_cmark::{Event, Options, Parser, Tag};

    use super::literal_text;

    #[test]
    fn literal_text_preserves_visible_content_without_active_markup() {
        for text in [
            "ordinary words 日本語",
            "![cover](https://example.test/cover.png)",
            "![cover][image]\n\n[image]: https://example.test/cover.png",
            "[open](https://example.test/) <https://example.test/>",
            "<img src='https://example.test/cover.png'> &lt;img&gt; &#33;",
            "```markdown\n![cover](https://example.test/cover.png)\n```",
            "~~~\n# heading\n- item\n> quote\n~~~",
            "\\![image](https://example.test/) **bold** _text_ ~strike~",
        ] {
            let escaped = literal_text(text);
            let mut visible = String::new();
            for event in Parser::new_ext(&escaped, Options::all()) {
                match event {
                    Event::Start(Tag::Image { .. } | Tag::Link { .. } | Tag::CodeBlock(_))
                    | Event::Html(_)
                    | Event::InlineHtml(_) => panic!("active markup in {escaped:?}"),
                    Event::Text(value) => visible.push_str(&value),
                    Event::SoftBreak | Event::HardBreak | Event::End(_) => visible.push(' '),
                    _ => {}
                }
            }
            assert_eq!(
                visible.split_whitespace().collect::<Vec<_>>(),
                text.split_whitespace().collect::<Vec<_>>(),
                "{escaped:?}"
            );
        }
        assert_eq!(literal_text(""), "");
    }
}
