// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Detect generated outputs that decline the requested work.

use serde_json::Value;

pub const NON_RESPONSIVE_RAW_OUTPUT_CAP_CHARS: usize = 512;

const LEAD_INS: &[&str] = &[
    "unfortunately",
    "my apologies",
    "i am sorry",
    "i'm sorry",
    "apologies",
    "sorry",
];
const NEGATION_HEADS: &[(&str, ResponsivenessSignal)] = &[
    ("i cannot", ResponsivenessSignal::ICannot),
    ("i can't", ResponsivenessSignal::ICant),
    ("i am not able to", ResponsivenessSignal::IAmNotAbleTo),
    ("i'm not able to", ResponsivenessSignal::ImNotAbleTo),
    ("i am unable to", ResponsivenessSignal::IAmUnableTo),
    ("i'm unable to", ResponsivenessSignal::ImUnableTo),
    (
        "i do not have access",
        ResponsivenessSignal::IDoNotHaveAccess,
    ),
    ("i don't have access", ResponsivenessSignal::IDontHaveAccess),
    (
        "i do not have the ability",
        ResponsivenessSignal::IDoNotHaveTheAbility,
    ),
    (
        "i don't have the ability",
        ResponsivenessSignal::IDontHaveTheAbility,
    ),
    ("as an ai", ResponsivenessSignal::AsAnAi),
];
const CONTINUATION_MARKERS: &[&str] = &[", so ", ", but ", ", though "];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResponsivenessSignal {
    ICannot,
    ICant,
    IAmNotAbleTo,
    ImNotAbleTo,
    IAmUnableTo,
    ImUnableTo,
    IDoNotHaveAccess,
    IDontHaveAccess,
    IDoNotHaveTheAbility,
    IDontHaveTheAbility,
    AsAnAi,
}

impl ResponsivenessSignal {
    pub const fn as_log_value(self) -> &'static str {
        match self {
            Self::ICannot => "i cannot",
            Self::ICant => "i can't",
            Self::IAmNotAbleTo => "i am not able to",
            Self::ImNotAbleTo => "i'm not able to",
            Self::IAmUnableTo => "i am unable to",
            Self::ImUnableTo => "i'm unable to",
            Self::IDoNotHaveAccess => "i do not have access",
            Self::IDontHaveAccess => "i don't have access",
            Self::IDoNotHaveTheAbility => "i do not have the ability",
            Self::IDontHaveTheAbility => "i don't have the ability",
            Self::AsAnAi => "as an ai",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResponsivenessVerdict {
    pub non_responsive: bool,
    pub matched_signal: Option<ResponsivenessSignal>,
    pub empty_corpus: bool,
}

pub fn classify_output_responsiveness(output: &str) -> ResponsivenessVerdict {
    let prose_leaves = string_leaves(output)
        .into_iter()
        .map(normalize_text)
        .filter(|leaf| !leaf.is_empty() && is_prose_like(leaf))
        .collect::<Vec<_>>();

    let mut evaluated_any = false;
    for leaf in prose_leaves {
        let Some(opening) = first_substantive_opening(&leaf) else {
            continue;
        };
        evaluated_any = true;
        if let Some((head, signal)) = matched_negation_head(&opening)
            && !continues_past_negation(&opening, head)
        {
            return ResponsivenessVerdict {
                non_responsive: true,
                matched_signal: Some(signal),
                empty_corpus: false,
            };
        }
    }

    ResponsivenessVerdict {
        non_responsive: false,
        matched_signal: None,
        empty_corpus: !evaluated_any,
    }
}

fn string_leaves(output: &str) -> Vec<String> {
    match serde_json::from_str::<Value>(output) {
        Ok(value) => walk_string_leaves(&value),
        Err(_) => vec![output.to_owned()],
    }
}

fn walk_string_leaves(value: &Value) -> Vec<String> {
    match value {
        Value::String(value) => vec![value.clone()],
        Value::Array(values) => values.iter().flat_map(walk_string_leaves).collect(),
        Value::Object(values) => values.values().flat_map(walk_string_leaves).collect(),
        Value::Null | Value::Bool(_) | Value::Number(_) => Vec::new(),
    }
}

fn normalize_text(text: String) -> String {
    text.replace('\u{2019}', "'")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn is_prose_like(text: &str) -> bool {
    text.chars().any(char::is_alphabetic)
        && (text.chars().any(char::is_whitespace) || text.contains(['.', '!', '?']))
}

fn first_substantive_opening(text: &str) -> Option<String> {
    text.split(['.', '!', '?']).find_map(|sentence| {
        let sentence = sentence.trim();
        if sentence.is_empty() || !is_prose_like(sentence) {
            return None;
        }
        let opening = strip_lead_in(sentence);
        (!opening.is_empty()).then_some(opening)
    })
}

fn strip_lead_in(opening: &str) -> String {
    let mut current = opening.to_owned();
    loop {
        let lowered = current.to_lowercase();
        let Some(lead_in) = LEAD_INS
            .iter()
            .find(|lead_in| lowered.starts_with(**lead_in))
        else {
            return current;
        };
        if lowered == *lead_in {
            return String::new();
        }
        let rest = &current[lead_in.len()..];
        if rest
            .chars()
            .next()
            .is_some_and(|character| character.is_alphanumeric() || character == '\'')
        {
            return current;
        }
        let stripped = rest.trim_start_matches(|character: char| {
            matches!(character, ' ' | '\t' | '\r' | '\n' | ',' | ':' | ';' | '-')
        });
        if stripped == current {
            return current;
        }
        current = stripped.trim().to_owned();
    }
}

fn matched_negation_head(opening: &str) -> Option<(&'static str, ResponsivenessSignal)> {
    let lowered = opening.to_lowercase();
    NEGATION_HEADS.iter().find_map(|(head, signal)| {
        if lowered == *head {
            return Some((*head, *signal));
        }
        if let Some(remainder) = lowered.strip_prefix(head)
            && remainder
                .chars()
                .next()
                .is_none_or(|character| !(character.is_alphabetic() || character == '\''))
        {
            return Some((*head, *signal));
        }
        None
    })
}

fn continues_past_negation(opening: &str, head: &str) -> bool {
    let tail = opening[head.len()..].to_lowercase();
    CONTINUATION_MARKERS.iter().any(|marker| {
        tail.find(marker)
            .is_some_and(|index| is_prose_like(tail[index + marker.len()..].trim()))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_negations_after_apology_lead_ins() {
        let verdict = classify_output_responsiveness("Sorry, I cannot complete that request.");
        assert!(verdict.non_responsive);
        assert_eq!(verdict.matched_signal, Some(ResponsivenessSignal::ICannot));
        assert!(!verdict.empty_corpus);
    }

    #[test]
    fn continuation_markers_keep_real_content_responsive() {
        for text in [
            "I cannot inspect the original file, so here is a description of the visible image.",
            "I can't access the source, but the screenshot shows a blue menu.",
            "As an AI, though I cannot browse, I can summarize the supplied text.",
        ] {
            assert!(
                !classify_output_responsiveness(text).non_responsive,
                "{text}"
            );
        }
    }

    #[test]
    fn walks_json_string_leaves() {
        let verdict = classify_output_responsiveness(
            r#"{"answer":"A useful answer.","note":"I do not have access to that."}"#,
        );
        assert!(verdict.non_responsive);
        assert_eq!(
            verdict.matched_signal,
            Some(ResponsivenessSignal::IDoNotHaveAccess)
        );
    }

    #[test]
    fn reports_empty_corpus_when_no_prose_opening_exists() {
        assert!(classify_output_responsiveness(r#"{"at":"12:30"}"#).empty_corpus);
    }
}
