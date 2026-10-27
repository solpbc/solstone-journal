// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Sense's prompt shows two example activity sentences, and the model copies
//! them into its own output: usually as a leading clause in front of a real
//! summary ("Debugged the retry handling in the ingest worker, then reviewed
//! the dashboard."), sometimes as the whole sentence. Those strings are shown
//! verbatim in the owner's activity list and become journal text, so they are
//! removed here, after generation and before anything reads the record.
//!
//! Only the example clauses go. The rest of a sentence is kept as written, with
//! its first letter capitalised when the example opened it. A field that was
//! nothing but an example becomes empty, and a topic naming one is dropped.

use crate::PreparedTalent;
use regex::Regex;
use serde_json::Value;
use std::sync::LazyLock;

/// The `activity_summary` examples in `sense.md`, as the model rewords them:
/// it keeps the object and swaps the verb or adds a word in front of it.
static EXAMPLE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(?:debugged|debugging|fixed|fixing)\s+(?:the\s+)?(?:[\w-]+\s+)?retry handling in (?:the )?ingest worker\b|\bsent (?:the )?invoices? to sam\b(?:,?\s+(?:then\s+|and\s+)?opened the launch checklist\b)?",
    )
    .expect("example pattern compiles")
});

/// A topic made from an example clause, in any word order.
static EXAMPLE_TOPIC: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\bretry\b.*\bingest worker\b|\bingest worker\b.*\bretry\b|\binvoices? to sam\b",
    )
    .expect("topic pattern compiles")
});

const CONNECTORS: [&str; 4] = ["and", "then", "also", "while"];

/// The Sense response with example clauses removed, or `None` when nothing changed.
pub(crate) fn scrub(prepared: &PreparedTalent, response: &str) -> Option<String> {
    if prepared.name != "sense" {
        return None;
    }
    let mut output = serde_json::from_str::<Value>(response).ok()?;
    scrub_record(&mut output).then(|| output.to_string())
}

fn scrub_record(output: &mut Value) -> bool {
    let mut changed = scrub_field(output.get_mut("activity_summary"));
    if let Some(facets) = output.get_mut("facets").and_then(Value::as_array_mut) {
        for facet in facets {
            changed |= scrub_field(facet.get_mut("activity"));
        }
    }
    if let Some(topics) = output.get_mut("topics").and_then(Value::as_array_mut) {
        let before = topics.len();
        topics.retain(|topic| !topic.as_str().is_some_and(|t| EXAMPLE_TOPIC.is_match(t)));
        changed |= topics.len() != before;
    }
    changed
}

fn scrub_field(field: Option<&mut Value>) -> bool {
    let Some(field) = field else {
        return false;
    };
    let Some(text) = field.as_str() else {
        return false;
    };
    let cleaned = without_examples(text);
    if cleaned == text {
        return false;
    }
    *field = Value::String(cleaned);
    true
}

fn without_examples(text: &str) -> String {
    let mut text = text.to_owned();
    while let Some(found) = EXAMPLE.find(&text) {
        let before = trim_trailing_connectors(&text[..found.start()]);
        let after = &text[found.end()..];
        text = if before.is_empty() {
            capitalise(trim_leading_connectors(after))
        } else if before.ends_with(['.', '!', '?']) {
            // The example was a sentence of its own after another one.
            let rest = capitalise(trim_leading_connectors(after));
            if rest.is_empty() {
                before.to_owned()
            } else {
                format!("{before} {rest}")
            }
        } else {
            format!("{before}{after}")
        };
    }
    if text.chars().any(char::is_alphanumeric) {
        text.trim().to_owned()
    } else {
        String::new()
    }
}

fn trim_leading_connectors(mut text: &str) -> &str {
    loop {
        let trimmed = text.trim_start_matches(|c: char| c.is_whitespace() || ",;:.!?".contains(c));
        let word_end = trimmed.find(char::is_whitespace).unwrap_or(trimmed.len());
        if CONNECTORS
            .iter()
            .any(|w| trimmed[..word_end].eq_ignore_ascii_case(w))
        {
            text = &trimmed[word_end..];
        } else {
            return trimmed;
        }
    }
}

fn trim_trailing_connectors(mut text: &str) -> &str {
    loop {
        let trimmed = text.trim_end_matches(|c: char| c.is_whitespace() || ",;:".contains(c));
        let word_start = trimmed
            .rfind(char::is_whitespace)
            .map_or(0, |index| index + 1);
        if CONNECTORS
            .iter()
            .any(|w| trimmed[word_start..].eq_ignore_ascii_case(w))
        {
            text = &trimmed[..word_start];
        } else {
            return trimmed;
        }
    }
}

fn capitalise(text: &str) -> String {
    let mut characters = text.chars();
    match characters.next() {
        Some(first) => first.to_uppercase().collect::<String>() + characters.as_str(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_leading_example_clause_is_removed_and_the_real_summary_kept() {
        // The shapes Sense writes, with made-up content around the example.
        for (written, kept) in [
            (
                "Debugged the retry handling in the ingest worker, then reviewed the garden planner.",
                "Reviewed the garden planner.",
            ),
            (
                "Debugged retry handling in the ingest worker, fixed the oven times in the bread recipe.",
                "Fixed the oven times in the bread recipe.",
            ),
            (
                "Debugged the retry handling in the ingest worker; saved changes to the reading list.",
                "Saved changes to the reading list.",
            ),
            (
                "Debugged the retry handling in the ingest worker and drafted a reply to Avery.",
                "Drafted a reply to Avery.",
            ),
            (
                "Debugged the retry handling in the ingest worker; reported that Priya repaired the fence.",
                "Reported that Priya repaired the fence.",
            ),
            (
                "Fixed the retry handling in the ingest worker and corrected the calendar reminder.",
                "Corrected the calendar reminder.",
            ),
            (
                "Debugged upload retry handling in the ingest worker, fixed the bike lights.",
                "Fixed the bike lights.",
            ),
            (
                "Debugged the retry handling in the ingest worker. Opened the photo album.",
                "Opened the photo album.",
            ),
        ] {
            assert_eq!(without_examples(written), kept, "{written}");
        }
    }

    #[test]
    fn a_trailing_or_whole_example_leaves_only_what_else_was_written() {
        for (written, kept) in [
            (
                "Watered the tomatoes, then sent the invoice to Sam.",
                "Watered the tomatoes.",
            ),
            (
                "Sent the invoice to Sam, then opened the launch checklist.",
                "",
            ),
            (
                "Monitored system load while debugging the retry handling in the ingest worker.",
                "Monitored system load.",
            ),
            (
                "Reviewed the release notes. Debugged the retry handling in the ingest worker.",
                "Reviewed the release notes.",
            ),
            (
                "Reviewed the release notes. Debugged the retry handling in the ingest worker. Sent the patch.",
                "Reviewed the release notes. Sent the patch.",
            ),
            ("Debugged the retry handling in the ingest worker.", ""),
            (
                "Debugged the retry handling in the ingest worker, then sent the invoice to Sam and opened the launch checklist.",
                "",
            ),
            (
                "Debugged the retry handling in the ingest worker, sent invoices to Sam and opened the launch checklist, sorted the weekend shopping list.",
                "Sorted the weekend shopping list.",
            ),
        ] {
            assert_eq!(without_examples(written), kept, "{written}");
        }
    }

    #[test]
    fn real_work_near_the_example_words_is_left_alone() {
        for written in [
            "Debugged read failures in the camera's ingest worker by adding logs.",
            "Added retry handling to the upload queue.",
            "Opened the launch checklist with Sam.",
            "Sent the invoice to Samantha.",
        ] {
            assert_eq!(without_examples(written), written);
        }
    }

    #[test]
    fn every_owner_visible_field_is_scrubbed_and_a_clean_record_is_untouched() {
        let mut record = json!({
            "activity_summary": "Debugged the retry handling in the ingest worker, then reviewed the budget memo.",
            "facets": [{"facet": "work", "activity": "Debugged the retry handling in the ingest worker and reviewed the budget memo.", "level": "high"}],
            "topics": ["ingest worker retry handling", "budget memo", "retry handling in ingest worker"],
        });
        assert!(scrub_record(&mut record));
        assert_eq!(record["activity_summary"], "Reviewed the budget memo.");
        assert_eq!(record["facets"][0]["activity"], "Reviewed the budget memo.");
        assert_eq!(record["topics"], json!(["budget memo"]));

        let mut clean = json!({
            "activity_summary": "Reviewed the budget memo.",
            "facets": [{"facet": "work", "activity": "Reviewed the budget memo.", "level": "high"}],
            "topics": ["budget memo"],
        });
        assert!(!scrub_record(&mut clean));
    }
}
