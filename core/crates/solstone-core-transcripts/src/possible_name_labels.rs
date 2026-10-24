// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Possible names shown beside the transcript words they may refer to.
//!
//! The words a statement holds are never changed. A reader that may show a
//! possible name inserts a tentative label right after the words it labels:
//! `… Fryer (possible name: Friar) …`. Which labels a reader may show is
//! decided by `solstone-core-possible-names`; this module only knows where a
//! statement's words are and how a label reads.

use std::collections::BTreeMap;

use serde_json::{Map, Value};
use solstone_core_format::content::possible_names::possible_name_label;

/// Labels for one transcript file, keyed by the statement ordinals speaker
/// labels use ([`crate::next_statement_ordinal`]). Each statement's labels are
/// `(end_byte, name)` pairs: insert `name` after the statement text's first
/// `end_byte` bytes.
#[derive(Clone, Debug, Default, Hash, PartialEq, Eq)]
pub struct PossibleNameLabels {
    /// Transcript file name inside the segment directory.
    pub file: String,
    pub statements: BTreeMap<i64, Vec<(usize, String)>>,
}

/// The words a transcript reader renders for one statement: `corrected` when
/// it holds a value, otherwise `text`, as the audio renderer chooses them.
/// Returns the chosen field and its words. A statement whose chosen value is
/// not text has no words a label can follow.
pub fn statement_text(record: &Map<String, Value>) -> Option<(&'static str, &str)> {
    for key in ["corrected", "text"] {
        match record.get(key) {
            None | Some(Value::Null) | Some(Value::Bool(false)) => {}
            Some(Value::String(text)) if text.is_empty() => {}
            Some(Value::Array(items)) if items.is_empty() => {}
            Some(Value::Object(fields)) if fields.is_empty() => {}
            Some(Value::Number(number)) if number.as_f64() == Some(0.0) => {}
            Some(Value::String(text)) => return Some((key, text)),
            Some(_) => return None,
        }
    }
    None
}

/// Insert each label right after the words it follows, in byte order. A label
/// whose position is not a character boundary of `text` is left out.
#[must_use]
pub fn label_statement(text: &str, labels: &[(usize, String)]) -> String {
    let mut ordered = labels
        .iter()
        .filter(|(end, _)| text.is_char_boundary(*end))
        .collect::<Vec<_>>();
    ordered.sort_by_key(|(end, _)| *end);
    let mut labelled = String::with_capacity(text.len() + 32 * ordered.len());
    let mut cursor = 0;
    for (end, name) in ordered {
        labelled.push_str(&text[cursor..*end]);
        labelled.push_str(" (");
        labelled.push_str(&possible_name_label(name));
        labelled.push(')');
        cursor = *end;
    }
    labelled.push_str(&text[cursor..]);
    labelled
}

/// Apply this statement's labels to one transcript line, or `None` when it
/// has none to show.
pub(crate) fn label_line(line: &str, labels: &[(usize, String)]) -> Option<String> {
    let Value::Object(mut record) = serde_json::from_str::<Value>(line).ok()? else {
        return None;
    };
    let (key, text) = statement_text(&record)?;
    let labelled = label_statement(text, labels);
    record.insert(key.to_owned(), Value::String(labelled));
    Some(Value::Object(record).to_string())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{label_line, label_statement, statement_text};

    #[test]
    fn statement_words_follow_the_renderer_choice() {
        let record = |value: serde_json::Value| value.as_object().unwrap().clone();
        assert_eq!(
            statement_text(&record(json!({"text": "plain", "corrected": "fixed"}))),
            Some(("corrected", "fixed"))
        );
        assert_eq!(
            statement_text(&record(json!({"text": "plain", "corrected": ""}))),
            Some(("text", "plain"))
        );
        assert_eq!(
            statement_text(&record(json!({"text": "plain", "corrected": null}))),
            Some(("text", "plain"))
        );
        assert_eq!(statement_text(&record(json!({"text": 7}))), None);
        assert_eq!(statement_text(&record(json!({"start": "00:00:01"}))), None);
    }

    #[test]
    fn labels_follow_their_words_in_byte_order() {
        let labels = vec![(22, "Ana Lima".to_owned()), (9, "Friar".to_owned())];
        assert_eq!(
            label_statement("ask Fryer to call anna today", &labels),
            "ask Fryer (possible name: Friar) to call anna (possible name: Ana Lima) today"
        );
        // A position inside a character is never split.
        assert_eq!(label_statement("é", &[(1, "X".to_owned())]), "é");
        assert_eq!(
            label_line(
                &json!({"start": "00:00:01", "text": "hi Fryer"}).to_string(),
                &[(8, "Friar".to_owned())]
            )
            .unwrap(),
            json!({"start": "00:00:01", "text": "hi Fryer (possible name: Friar)"}).to_string()
        );
    }
}
