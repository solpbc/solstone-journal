// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

const EXTENSIONS: [&str; 25] = [
    "md", "txt", "json", "jsonl", "csv", "log", "yaml", "yml", "toml", "sql", "py", "ts", "tsx",
    "html", "css", "pdf", "png", "jpg", "jpeg", "gif", "zip", "doc", "docx", "xls", "xlsx",
];

/// Return whether a name string matches one of five synthetic generated-ID shapes.
///
/// Whitespace never matches (so `Ada Lovelace` and `iPhone 15` do not).
/// An underscore-rule letter is ASCII `A-Z`/`a-z`.
/// Extensions match case-sensitively against the lowercase list.
///
/// Matching synthetic examples include:
/// - `tkt_x7k2p9qa` (underscore prefix-token)
/// - `ops:reimbur` (lowercase colon-token)
/// - `docs/plan` (lowercase path)
/// - `a3f9c2e1` (lowercase hex identifier)
/// - `notes_v2.md` (known file extension)
///
/// Synthetic rejection examples include:
/// - `x86_64` (no ASCII letter after underscore)
/// - `re:Invent` (contains uppercase letter)
/// - `10:30` (no lowercase letter)
/// - `16:9` (no lowercase letter)
/// - `AC/DC` (contains uppercase letters)
/// - `20261022` (no hex letter)
/// - `Face2Face` (uppercase, not all hex)
pub fn looks_like_generated_id(name: &str) -> bool {
    if name.chars().any(char::is_whitespace) {
        return false;
    }

    // 1. Underscore rule: at least one '_', with at least one ASCII letter on both sides.
    let underscore_match = name.match_indices('_').any(|(idx, _)| {
        let (left, right) = name.split_at(idx);
        let right = &right[1..];
        left.chars().any(|c| c.is_ascii_alphabetic())
            && right.chars().any(|c| c.is_ascii_alphabetic())
    });
    if underscore_match {
        return true;
    }

    // 2. Colon rule: a colon between two non-empty parts, each part contains a lowercase letter,
    // and no uppercase anywhere.
    if !name.chars().any(char::is_uppercase) {
        let parts: Vec<&str> = name.split(':').collect();
        if parts.len() == 2
            && !parts[0].is_empty()
            && !parts[1].is_empty()
            && parts[0].chars().any(char::is_lowercase)
            && parts[1].chars().any(char::is_lowercase)
        {
            return true;
        }
    }

    // 3. Lowercase path: contains '/', no uppercase anywhere, and at least one '/'-separated
    // part has two or more lowercase letters.
    if !name.chars().any(char::is_uppercase)
        && name.contains('/')
        && name
            .split('/')
            .any(|part| part.chars().filter(|c| c.is_lowercase()).count() >= 2)
    {
        return true;
    }

    // 4. File extension: the suffix after the last '.' is one of the 25 allowed extensions.
    if matches!(name.rsplit_once('.'), Some((stem, ext)) if !stem.is_empty() && EXTENSIONS.contains(&ext))
    {
        return true;
    }

    // 5. Lowercase hex: length >= 8, all chars in 0-9a-f, at least one digit and at least one letter.
    if name.len() >= 8
        && name.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f'))
        && name.chars().any(|c| c.is_ascii_digit())
        && name.chars().any(|c| matches!(c, 'a'..='f'))
    {
        return true;
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn looks_like_generated_id_accepts_and_rejects_the_shape_list() {
        let accept = [
            "tkt_x7k2p9qa",
            "ops:reimbur",
            "notes_v2.md",
            "plan.pdf",
            "a3f9c2e1",
            "docs/plan",
        ];
        for item in accept {
            assert!(looks_like_generated_id(item), "expected accept for {item}");
        }

        let reject = [
            "Face2Face",
            "Mac/PC",
            "Oct/Nov",
            "Sales/Marketing",
            "10:30",
            "16:9",
            "bun.sh",
            "docs.rs",
            "Ada Lovelace",
            "GPT-4",
            "iPhone 15",
            "3M",
            "Acme Corp",
            "O'Brien",
            "Zoë",
            "Node.js",
            "node.js",
            "next.js",
            "Vue.js",
            "ASP.NET",
            "x86_64",
            "EC2",
            "Web3",
            "AC/DC",
            "TCP/IP",
            "24/7",
            "I/O",
            "N/A",
            "Amazon.com",
            "github.com",
            "Booking.com",
            "Fly.io",
            "Character.ai",
            "re:Invent",
            "20261022",
        ];
        for item in reject {
            assert!(!looks_like_generated_id(item), "expected reject for {item}");
        }
    }
}
