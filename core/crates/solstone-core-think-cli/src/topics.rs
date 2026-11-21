// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MatchedQuote {
    pub speaker: Option<String>,
    pub span: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TopicsEvaluation {
    pub markdown: Option<String>,
    pub kept_quotes: u64,
    pub dropped_quotes: u64,
    pub unreadable: Vec<(PathBuf, String)>,
}

#[derive(Clone, Debug)]
struct TranscriptToken {
    _file_idx: usize,
    record_idx: usize,
    _token_idx: usize,
    word: String,
    normalized: String,
    /// The normalized word as a small id, so a passage compares as a sequence.
    id: u32,
    speaker: Option<String>,
}

#[derive(Clone, Debug)]
struct SpeechRecord {
    words: Vec<String>,
    speaker: Option<String>,
}

fn is_drop_punct(c: char) -> bool {
    c.is_ascii_punctuation()
        || matches!(
            c,
            '\u{2018}'..='\u{201F}' | '\u{2032}' | '\u{2035}' | '\u{00AB}' | '\u{00BB}'
        )
}

fn normalize_token(token: &str) -> String {
    token
        .chars()
        .filter(|&c| !is_drop_punct(c))
        .collect::<String>()
        .to_lowercase()
}

fn word_id(vocabulary: &mut HashMap<String, u32>, word: &str) -> u32 {
    if let Some(id) = vocabulary.get(word) {
        return *id;
    }
    let id = u32::try_from(vocabulary.len()).unwrap_or(u32::MAX);
    vocabulary.insert(word.to_owned(), id);
    id
}

fn collapse_crlf(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut in_break = false;
    for c in s.chars() {
        if c == '\r' || c == '\n' {
            if !in_break {
                result.push(' ');
                in_break = true;
            }
        } else {
            in_break = false;
            result.push(c);
        }
    }
    result
}

fn is_truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|num| num != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

fn display_json_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        _ => v.to_string(),
    }
}

fn parse_speaker_label(record: &Map<String, Value>) -> Option<String> {
    match record.get("speaker") {
        Some(Value::Number(n)) => Some(format!("Speaker {n}")),
        Some(Value::String(s)) if !s.trim().is_empty() => Some(s.clone()),
        _ => None,
    }
}

fn parse_file_speech(content: &str) -> Vec<SpeechRecord> {
    let mut records = Vec::new();
    let mut seen_first_object = false;

    for line in content.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(Value::Object(obj)) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let has_start = obj.contains_key("start");
        if !seen_first_object {
            seen_first_object = true;
            if !has_start {
                // First object without start is a header, never speech.
                continue;
            }
        }
        if !has_start {
            // Objects without start are not speech and not in sequence.
            continue;
        }

        let spoken_text = if let Some(corrected) = obj.get("corrected").filter(|v| is_truthy(v)) {
            display_json_text(corrected)
        } else if let Some(text) = obj.get("text").filter(|v| is_truthy(v)) {
            display_json_text(text)
        } else {
            String::new()
        };

        let speaker = parse_speaker_label(&obj);
        let words = spoken_text
            .split_whitespace()
            .map(str::to_string)
            .collect::<Vec<_>>();
        records.push(SpeechRecord { words, speaker });
    }

    records
}

/// The lowest word-sequence similarity at which a quote that is not word for
/// word still finds its passage. The passage is shown in the transcript's own
/// words, never the quote's.
const FUZZY_QUOTE_FLOOR: f64 = 0.85;

/// How many words longer or shorter than the quote a passage may be.
const FUZZY_QUOTE_SLACK: usize = 4;

/// The passage closest to a quote that is not word for word, if any reaches
/// [`FUZZY_QUOTE_FLOOR`]. Like an exact match it stays in one file, crosses
/// only consecutive records, and runs 3 to 30 words. The first best wins.
fn fuzzy_match_quote(
    model_ids: &[u32],
    files_tokens: &[Vec<TranscriptToken>],
) -> Option<MatchedQuote> {
    if model_ids.is_empty() {
        return None;
    }
    let shortest = model_ids.len().saturating_sub(FUZZY_QUOTE_SLACK).max(3);
    let longest = (model_ids.len() + FUZZY_QUOTE_SLACK).min(30);
    let mut best: Option<(f64, usize, usize, usize)> = None;
    for (file_idx, tokens) in files_tokens.iter().enumerate() {
        for start in 0..tokens.len() {
            for len in shortest..=longest {
                let end = start + len;
                if end > tokens.len() {
                    break;
                }
                let window = &tokens[start..end];
                if !window
                    .windows(2)
                    .all(|pair| pair[1].record_idx <= pair[0].record_idx + 1)
                {
                    break;
                }
                let score = rapidfuzz::fuzz::ratio(
                    model_ids.iter().copied(),
                    window.iter().map(|token| token.id),
                );
                if score >= FUZZY_QUOTE_FLOOR && best.is_none_or(|(top, ..)| score > top) {
                    best = Some((score, file_idx, start, end));
                }
            }
        }
    }
    let (_, file_idx, start, end) = best?;
    let tokens = &files_tokens[file_idx][start..end];
    let first_speaker = tokens[0].speaker.as_ref();
    let speaker = first_speaker
        .filter(|label| {
            tokens
                .iter()
                .all(|token| token.speaker.as_ref() == Some(*label))
        })
        .map(|label| collapse_crlf(label).trim().to_string());
    let span = tokens
        .iter()
        .map(|token| token.word.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    Some(MatchedQuote {
        speaker,
        span: collapse_crlf(&span).trim().to_string(),
    })
}

// Whole-word matching assumes spaces between words, so a quote from a transcript with no spaces between words does not match and is dropped.
fn match_quote_in_files(
    model_norm: &[String],
    files_tokens: &[Vec<TranscriptToken>],
) -> Option<MatchedQuote> {
    if model_norm.is_empty() {
        return None;
    }
    let quote_len = model_norm.len();
    // The span shown comes from the first file that holds the words; its speaker
    // is shown only when every occurrence, in every file, carries the same label.
    let mut chosen: Option<(usize, usize, usize)> = None;
    let mut all_spans: Vec<(usize, usize, usize)> = Vec::new();

    for (file_idx, file_tokens) in files_tokens.iter().enumerate() {
        if file_tokens.len() < quote_len {
            continue;
        }

        // Find all matching spans in this file
        let mut match_spans: Vec<(usize, usize)> = Vec::new(); // (start_idx, end_idx) in file_tokens
        let token_count = file_tokens.len();

        for start in 0..=token_count - quote_len {
            let mut matches = true;
            let mut prev_record: Option<usize> = None;

            for offset in 0..quote_len {
                let curr_tok = &file_tokens[start + offset];
                if curr_tok.normalized != model_norm[offset] {
                    matches = false;
                    break;
                }
                if let Some(prev) = prev_record {
                    // Check if run across records is consecutive without barrier
                    if curr_tok.record_idx != prev && curr_tok.record_idx != prev + 1 {
                        matches = false;
                        break;
                    }
                }
                prev_record = Some(curr_tok.record_idx);
            }

            if matches {
                match_spans.push((start, start + quote_len - 1));
            }
        }

        if match_spans.is_empty() {
            continue;
        }
        if chosen.is_none() {
            chosen = Some((file_idx, match_spans[0].0, match_spans[0].1));
        }
        all_spans.extend(
            match_spans
                .into_iter()
                .map(|(start, end)| (file_idx, start, end)),
        );
    }

    let (chosen_file, chosen_start, chosen_end) = chosen?;
    let span_label = |file_idx: usize, start: usize, end: usize| -> Option<String> {
        let tokens = &files_tokens[file_idx][start..=end];
        let first_speaker = tokens[0].speaker.as_ref()?;
        tokens
            .iter()
            .all(|tok| tok.speaker.as_ref() == Some(first_speaker))
            .then(|| first_speaker.clone())
    };
    let final_speaker = span_label(chosen_file, chosen_start, chosen_end).and_then(|label| {
        all_spans
            .iter()
            .all(|&(file_idx, start, end)| {
                span_label(file_idx, start, end).as_ref() == Some(&label)
            })
            .then(|| collapse_crlf(&label).trim().to_string())
    });
    let span_words = files_tokens[chosen_file][chosen_start..=chosen_end]
        .iter()
        .map(|t| t.word.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    Some(MatchedQuote {
        speaker: final_speaker,
        span: collapse_crlf(&span_words).trim().to_string(),
    })
}

pub(crate) fn evaluate_topics(segment_dir: &Path, sense: &Map<String, Value>) -> TopicsEvaluation {
    // 1. Collect speech files
    let mut speech_paths = Vec::new();
    let mut unreadable = Vec::new();
    // Without quotes to check, no transcript is read and none can be reported unreadable.
    let has_quotes = sense
        .get("quotes")
        .and_then(Value::as_array)
        .is_some_and(|quotes| !quotes.is_empty());

    if has_quotes {
        match fs::read_dir(segment_dir) {
            Ok(entries) => {
                for entry in entries {
                    let entry = match entry {
                        Ok(e) => e,
                        Err(err) => {
                            unreadable.push((segment_dir.to_path_buf(), err.to_string()));
                            break;
                        }
                    };
                    let path = entry.path();
                    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                        continue;
                    };
                    if solstone_core_format::content::segment_file_offers(
                        name,
                        solstone_core_format::content::SegmentReader::Talents,
                        Default::default(),
                    )
                    .iter()
                    .any(|offer| {
                        offer.format
                            == solstone_core_format::content::FileFormat::Raw(
                                solstone_core_format::content::RawPerceptFamily::Audio,
                            )
                    }) {
                        speech_paths.push(path);
                    }
                }
                speech_paths.sort();
            }
            Err(err) => {
                unreadable.push((segment_dir.to_path_buf(), err.to_string()));
            }
        }
    }

    // 2. Read and parse speech files
    let mut vocabulary = HashMap::new();
    let mut files_tokens: Vec<Vec<TranscriptToken>> = Vec::new();
    if unreadable.is_empty() {
        for (file_idx, path) in speech_paths.iter().enumerate() {
            let content = if let Some(relative) =
                solstone_core_transcripts::conversation_relative(path)
            {
                match solstone_core_transcripts::read_talent_conversation(
                    path,
                    &relative,
                    |file, medium| {
                        if medium != solstone_core_format::content::conversation::ConversationMedium::Spoken { return Ok(None); }
                        let mut body = String::new();
                        file.read_to_string(&mut body)?;
                        Ok(Some(body))
                    },
                ) {
                    Ok(Some(body)) => body,
                    Ok(None) => continue,
                    Err(err) => {
                        unreadable.push((path.clone(), format!("{err:?}")));
                        break;
                    }
                }
            } else {
                match fs::read_to_string(path) {
                    Ok(body) => body,
                    Err(err) => {
                        unreadable.push((path.clone(), err.to_string()));
                        break;
                    }
                }
            };

            let records = parse_file_speech(&content);
            let mut file_tokens = Vec::new();
            for (record_idx, record) in records.into_iter().enumerate() {
                for (token_idx, word) in record.words.into_iter().enumerate() {
                    let normalized = normalize_token(&word);
                    let id = word_id(&mut vocabulary, &normalized);
                    file_tokens.push(TranscriptToken {
                        _file_idx: file_idx,
                        record_idx,
                        _token_idx: token_idx,
                        word,
                        normalized,
                        id,
                        speaker: record.speaker.clone(),
                    });
                }
            }
            files_tokens.push(file_tokens);
        }
    }

    let speech_incomplete = !unreadable.is_empty();

    // 3. Process topics from sense
    let mut topics = Vec::new();
    if let Some(raw_topics) = sense.get("topics").and_then(Value::as_array) {
        for val in raw_topics {
            if let Some(s) = val.as_str() {
                let trimmed = s.trim();
                let collapsed = collapse_crlf(trimmed);
                let cleaned = collapsed.trim();
                if !cleaned.is_empty() && cleaned.chars().count() <= 80 {
                    topics.push(cleaned.to_string());
                    if topics.len() == 12 {
                        break;
                    }
                }
            }
        }
    }

    // 4. Process quotes from sense
    let mut kept_quotes: u64 = 0;
    let mut dropped_quotes: u64 = 0;
    let mut surviving_quotes = Vec::new();

    if let Some(raw_quotes) = sense.get("quotes").and_then(Value::as_array) {
        for quote_val in raw_quotes {
            let Some(quote_obj) = quote_val.as_object() else {
                dropped_quotes += 1;
                continue;
            };
            let Some(quote_text) = quote_obj.get("text").and_then(Value::as_str) else {
                dropped_quotes += 1;
                continue;
            };

            let trimmed_quote = quote_text.trim();
            let word_count = trimmed_quote.split_whitespace().count();
            if !(3..=30).contains(&word_count) {
                dropped_quotes += 1;
                continue;
            }

            if speech_incomplete || surviving_quotes.len() == 8 {
                dropped_quotes += 1;
                continue;
            }

            let model_norm = trimmed_quote
                .split_whitespace()
                .map(normalize_token)
                .collect::<Vec<_>>();

            let model_ids = model_norm
                .iter()
                .map(|word| word_id(&mut vocabulary, word))
                .collect::<Vec<_>>();
            if let Some(matched) = match_quote_in_files(&model_norm, &files_tokens)
                .or_else(|| fuzzy_match_quote(&model_ids, &files_tokens))
            {
                if surviving_quotes.len() < 8 {
                    kept_quotes += 1;
                    surviving_quotes.push(matched);
                } else {
                    dropped_quotes += 1;
                }
            } else {
                dropped_quotes += 1;
            }
        }
    }

    // 5. Build Markdown
    let markdown = if !topics.is_empty() || !surviving_quotes.is_empty() {
        let mut md = String::new();
        md.push_str("## Topics\n");
        for topic in &topics {
            md.push_str(&format!("- {topic}\n"));
        }
        md.push_str("## Quotes\n");
        for quote in &surviving_quotes {
            if let Some(ref speaker) = quote.speaker {
                md.push_str(&format!("- {speaker}: {}\n", quote.span));
            } else {
                md.push_str(&format!("- {}\n", quote.span));
            }
        }
        Some(md)
    } else {
        None
    };

    TopicsEvaluation {
        markdown,
        kept_quotes,
        dropped_quotes,
        unreadable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::tempdir;

    #[test]
    fn only_spoken_files_ground_a_quote_in_a_mixed_import_segment() {
        let root = tempdir().unwrap();
        for stream in ["import.chatgpt", "import.text"] {
            let dir = root
                .path()
                .join("chronicle/20261120")
                .join(stream)
                .join("090000_300");
            fs::create_dir_all(&dir).unwrap();
            let row =
                json!({"start":"00:00:01","speaker":"Sam","text":"the chart uses green triangles"});
            fs::write(
                dir.join("conversation_transcript.jsonl"),
                format!("{{\"imported\":{{\"id\":\"fixture\"}}}}\n{row}"),
            )
            .unwrap();
            fs::write(
                dir.join("mic_audio.jsonl"),
                json!({"start":0,"speaker":"Jo","text":"the recording captures faint birdsong"})
                    .to_string(),
            )
            .unwrap();
            let sense = json!({"quotes":[{"speaker":"Sam","text":"the chart uses green triangles"},{"speaker":"Jo","text":"the recording captures faint birdsong"}]});
            let result = evaluate_topics(&dir, sense.as_object().unwrap());
            assert!(result.unreadable.is_empty(), "{result:?}");
            assert_eq!(
                (result.kept_quotes, result.dropped_quotes),
                (1, 1),
                "{result:?}"
            );
        }
    }

    #[test]
    fn test_apostrophe_styles_and_surrounding_quotes() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("audio.jsonl");
        fs::write(
            &path,
            r#"{"start": 1.0, "text": "We don’t want this failure here."}"#,
        )
        .unwrap();

        // Model quotes with ASCII apostrophe and extra quotes
        let sense = json!({
            "quotes": [
                {"speaker": "unknown", "text": "“We don't want this failure”"}
            ]
        });

        let eval = evaluate_topics(dir.path(), sense.as_object().unwrap());
        assert_eq!(eval.kept_quotes, 1);
        assert_eq!(eval.dropped_quotes, 0);
        let md = eval.markdown.unwrap();
        // Transcript's typographic apostrophe and casing are preserved
        assert!(md.contains("- We don’t want this failure\n"), "got {md}");
    }

    #[test]
    fn test_speaker_label_formatting() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("audio.jsonl");
        fs::write(
            &path,
            [
                r#"{"start": 1.0, "speaker": 2, "text": "Plan the launch event."}"#,
                r#"{"start": 2.0, "speaker": "2", "text": "Review the pricing sheet."}"#,
                r#"{"start": 3.0, "speaker": "Human", "text": "Explain the architecture here."}"#,
                r#"{"start": 4.0, "speaker": "Assistant", "text": "Here is the diagram."}"#,
                r#"{"start": 5.0, "text": "No speaker was given."}"#,
            ]
            .join("\n"),
        )
        .unwrap();

        let sense = json!({
            "quotes": [
                {"speaker": "model_guess", "text": "Plan the launch event."},
                {"speaker": "model_guess", "text": "Review the pricing sheet."},
                {"speaker": "model_guess", "text": "Explain the architecture here."},
                {"speaker": "model_guess", "text": "Here is the diagram."},
                {"speaker": "model_guess", "text": "No speaker was given."}
            ]
        });

        let eval = evaluate_topics(dir.path(), sense.as_object().unwrap());
        assert_eq!(eval.kept_quotes, 5);
        let md = eval.markdown.unwrap();
        assert!(md.contains("- Speaker 2: Plan the launch event.\n"));
        assert!(md.contains("- 2: Review the pricing sheet.\n"));
        assert!(md.contains("- Human: Explain the architecture here.\n"));
        assert!(md.contains("- Assistant: Here is the diagram.\n"));
        assert!(md.contains("- No speaker was given.\n"));
        assert!(!md.contains("- Speaker: No speaker was given."));
    }

    #[test]
    fn test_corrected_field_takes_precedence() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("audio.jsonl");
        fs::write(
            &path,
            r#"{"start": 1.0, "text": "raw inaccurate wording", "corrected": "fixed accurate wording"}"#,
        )
        .unwrap();

        let sense = json!({
            "quotes": [
                {"speaker": "unknown", "text": "raw inaccurate wording"},
                {"speaker": "unknown", "text": "fixed accurate wording"}
            ]
        });

        let eval = evaluate_topics(dir.path(), sense.as_object().unwrap());
        assert_eq!(eval.kept_quotes, 1);
        assert_eq!(eval.dropped_quotes, 1);
        let md = eval.markdown.unwrap();
        assert!(md.contains("- fixed accurate wording\n"));
    }

    #[test]
    fn test_headerless_first_record_with_start_is_kept() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("audio.jsonl");
        fs::write(
            &path,
            r#"{"start": 0.0, "speaker": 1, "text": "First line of speech."}"#,
        )
        .unwrap();

        let sense = json!({
            "quotes": [
                {"speaker": "unknown", "text": "First line of speech."}
            ]
        });

        let eval = evaluate_topics(dir.path(), sense.as_object().unwrap());
        assert_eq!(eval.kept_quotes, 1);
        assert!(
            eval.markdown
                .unwrap()
                .contains("- Speaker 1: First line of speech.\n")
        );
    }

    #[test]
    fn test_header_without_start_carrying_topics_setting_not_speech() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("imported_audio.jsonl");
        fs::write(
            &path,
            [
                r#"{"topics": "Planning meeting", "setting": "Conference room"}"#,
                r#"{"start": 1.0, "speaker": "Alex", "text": "Welcome to the meeting."}"#,
            ]
            .join("\n"),
        )
        .unwrap();

        let sense = json!({
            "quotes": [
                {"speaker": "unknown", "text": "Planning meeting Conference room"},
                {"speaker": "unknown", "text": "Welcome to the meeting."}
            ]
        });

        let eval = evaluate_topics(dir.path(), sense.as_object().unwrap());
        assert_eq!(eval.kept_quotes, 1);
        assert_eq!(eval.dropped_quotes, 1);
        let md = eval.markdown.unwrap();
        assert!(md.contains("- Alex: Welcome to the meeting.\n"));
    }

    #[test]
    fn test_decorations_not_treated_as_speech() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("audio.jsonl");
        fs::write(
            &path,
            r#"{"start": "00:01", "speaker": 1, "text": "Deploying the staging cluster now."}"#,
        )
        .unwrap();

        let sense = json!({
            "quotes": [
                {"speaker": "unknown", "text": "[00:01] Speaker 1: Deploying the staging cluster now."},
                {"speaker": "unknown", "text": "Deploying the staging cluster now."}
            ]
        });

        let eval = evaluate_topics(dir.path(), sense.as_object().unwrap());
        assert_eq!(eval.kept_quotes, 1);
        assert_eq!(eval.dropped_quotes, 1);
        assert!(
            eval.markdown
                .unwrap()
                .contains("- Speaker 1: Deploying the staging cluster now.\n")
        );
    }

    #[test]
    fn test_rapist_does_not_match_therapist() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("audio.jsonl");
        fs::write(
            &path,
            r#"{"start": 1.0, "text": "The licensed therapist arrived."}"#,
        )
        .unwrap();

        let sense = json!({
            "quotes": [
                {"speaker": "unknown", "text": "licensed rapist arrived"}
            ]
        });

        let eval = evaluate_topics(dir.path(), sense.as_object().unwrap());
        assert_eq!(eval.kept_quotes, 0);
        assert_eq!(eval.dropped_quotes, 1);
    }

    #[test]
    fn a_near_quote_is_kept_in_the_transcripts_words_and_label() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("audio.jsonl"),
            r#"{"start": 1.0, "speaker": 1, "text": "So um we will ship it on Friday."}"#,
        )
        .unwrap();
        let sense =
            json!({"quotes": [{"speaker": "unknown", "text": "So we will ship it on Friday"}]});

        let eval = evaluate_topics(dir.path(), sense.as_object().unwrap());
        assert_eq!((eval.kept_quotes, eval.dropped_quotes), (1, 0));
        let md = eval.markdown.unwrap();
        assert!(
            md.contains("- Speaker 1: So um we will ship it on Friday.\n"),
            "got {md}"
        );
    }

    #[test]
    fn a_near_quote_never_shows_words_the_transcript_does_not_have() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("audio.jsonl"),
            r#"{"start": 1.0, "text": "We are not going to ship on Friday."}"#,
        )
        .unwrap();
        let sense =
            json!({"quotes": [{"speaker": "unknown", "text": "We are going to ship on Friday"}]});

        let eval = evaluate_topics(dir.path(), sense.as_object().unwrap());
        assert_eq!(eval.kept_quotes, 1);
        let md = eval.markdown.unwrap();
        assert!(
            md.contains("- We are not going to ship on Friday.\n"),
            "got {md}"
        );
    }

    #[test]
    fn a_quote_far_from_every_passage_is_dropped() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("audio.jsonl"),
            r#"{"start": 1.0, "text": "We will ship it on Friday."}"#,
        )
        .unwrap();
        let sense =
            json!({"quotes": [{"speaker": "unknown", "text": "We will ship the beta on Monday"}]});

        let eval = evaluate_topics(dir.path(), sense.as_object().unwrap());
        assert_eq!((eval.kept_quotes, eval.dropped_quotes), (0, 1));
        assert!(eval.markdown.is_none());
    }

    #[test]
    fn test_span_consecutive_records_different_labels_has_no_speaker() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("audio.jsonl");
        fs::write(
            &path,
            [
                r#"{"start": 1.0, "speaker": "Alice", "text": "We will ship"}"#,
                r#"{"start": 2.0, "speaker": "Bob", "text": "the release tomorrow."}"#,
            ]
            .join("\n"),
        )
        .unwrap();

        let sense = json!({
            "quotes": [
                {"speaker": "Alice", "text": "We will ship the release tomorrow."}
            ]
        });

        let eval = evaluate_topics(dir.path(), sense.as_object().unwrap());
        assert_eq!(eval.kept_quotes, 1);
        let md = eval.markdown.unwrap();
        assert!(md.contains("- We will ship the release tomorrow.\n"));
        assert!(!md.contains("- Alice:"));
        assert!(!md.contains("- Bob:"));
    }

    #[test]
    fn test_same_words_under_two_labels_in_one_file_shows_no_speaker_one_span() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("audio.jsonl");
        fs::write(
            &path,
            [
                r#"{"start": 1.0, "speaker": "Alice", "text": "Let us check the dashboard."}"#,
                r#"{"start": 2.0, "speaker": "Bob", "text": "Let us check the dashboard."}"#,
            ]
            .join("\n"),
        )
        .unwrap();

        let sense = json!({
            "quotes": [
                {"speaker": "unknown", "text": "Let us check the dashboard."}
            ]
        });

        let eval = evaluate_topics(dir.path(), sense.as_object().unwrap());
        assert_eq!(eval.kept_quotes, 1);
        let md = eval.markdown.unwrap();
        assert_eq!(md, "## Topics\n## Quotes\n- Let us check the dashboard.\n");
    }

    #[test]
    fn test_span_across_two_files_dropped() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("1_audio.jsonl"),
            r#"{"start": 1.0, "text": "Beginning of the discussion"}"#,
        )
        .unwrap();
        fs::write(
            dir.path().join("2_audio.jsonl"),
            r#"{"start": 1.0, "text": "and continuation here"}"#,
        )
        .unwrap();

        let sense = json!({
            "quotes": [
                {"speaker": "unknown", "text": "discussion and continuation"}
            ]
        });

        let eval = evaluate_topics(dir.path(), sense.as_object().unwrap());
        assert_eq!(eval.kept_quotes, 0);
        assert_eq!(eval.dropped_quotes, 1);
    }

    #[test]
    fn test_empty_start_bearing_record_is_barrier() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("audio.jsonl"),
            [
                r#"{"start": 1.0, "text": "We start here"}"#,
                r#"{"start": 2.0, "text": ""}"#,
                r#"{"start": 3.0, "text": "and finish there"}"#,
            ]
            .join("\n"),
        )
        .unwrap();

        let sense = json!({
            "quotes": [
                {"speaker": "unknown", "text": "start here and finish"}
            ]
        });

        let eval = evaluate_topics(dir.path(), sense.as_object().unwrap());
        assert_eq!(eval.kept_quotes, 0);
        assert_eq!(eval.dropped_quotes, 1);
    }

    #[test]
    fn test_first_8_fail_matching_last_2_kept() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("audio.jsonl"),
            [
                r#"{"start": 1.0, "text": "Real speech one is here."}"#,
                r#"{"start": 2.0, "text": "Real speech two is there."}"#,
            ]
            .join("\n"),
        )
        .unwrap();

        let mut quotes = Vec::new();
        for i in 0..8 {
            quotes.push(
                json!({"speaker": "unknown", "text": format!("Fake unmatching words number {i}")}),
            );
        }
        quotes.push(json!({"speaker": "unknown", "text": "Real speech one is here."}));
        quotes.push(json!({"speaker": "unknown", "text": "Real speech two is there."}));

        let sense = json!({ "quotes": quotes });
        let eval = evaluate_topics(dir.path(), sense.as_object().unwrap());
        assert_eq!(eval.kept_quotes, 2);
        assert_eq!(eval.dropped_quotes, 8);
    }

    #[test]
    fn test_word_count_bounds_and_caps() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("audio.jsonl"),
            r#"{"start": 1.0, "text": "one two three four five six seven eight nine ten eleven twelve thirteen fourteen fifteen sixteen seventeen eighteen nineteen twenty twenty-one twenty-two twenty-three twenty-four twenty-five twenty-six twenty-seven twenty-eight twenty-nine thirty thirty-one"}"#,
        )
        .unwrap();

        // 2-word: dropped
        let sense_2 = json!({"quotes": [{"speaker": "x", "text": "one two"}]});
        let eval_2 = evaluate_topics(dir.path(), sense_2.as_object().unwrap());
        assert_eq!(eval_2.kept_quotes, 0);
        assert_eq!(eval_2.dropped_quotes, 1);

        // 3-word: kept
        let sense_3 = json!({"quotes": [{"speaker": "x", "text": "one two three"}]});
        let eval_3 = evaluate_topics(dir.path(), sense_3.as_object().unwrap());
        assert_eq!(eval_3.kept_quotes, 1);
        assert_eq!(eval_3.dropped_quotes, 0);

        // 30-word: kept
        let words_30 = (0..30)
            .map(|i| format!("w{i}"))
            .collect::<Vec<_>>()
            .join(" ");
        fs::write(
            dir.path().join("30_audio.jsonl"),
            format!(r#"{{"start": 1.0, "text": "{words_30}"}}"#),
        )
        .unwrap();
        let sense_30 = json!({"quotes": [{"speaker": "x", "text": words_30}]});
        let eval_30 = evaluate_topics(dir.path(), sense_30.as_object().unwrap());
        assert_eq!(eval_30.kept_quotes, 1);

        // 31-word: dropped
        let words_31 = (0..31)
            .map(|i| format!("w{i}"))
            .collect::<Vec<_>>()
            .join(" ");
        // The 31-word run is in a transcript, so only the length bound can drop it.
        fs::write(
            dir.path().join("31_audio.jsonl"),
            format!(r#"{{"start": 1.0, "text": "{words_31}"}}"#),
        )
        .unwrap();
        let sense_31 = json!({"quotes": [{"speaker": "x", "text": words_31}]});
        let eval_31 = evaluate_topics(dir.path(), sense_31.as_object().unwrap());
        assert_eq!(eval_31.kept_quotes, 0);
        assert_eq!(eval_31.dropped_quotes, 1);

        // Nine 31-word matches then one 3-word match keeps the 3-word quote
        let mut nine_plus_one = Vec::new();
        for _ in 0..9 {
            nine_plus_one.push(json!({"speaker": "x", "text": words_31.clone()}));
        }
        nine_plus_one.push(json!({"speaker": "x", "text": "one two three"}));
        let eval_mix = evaluate_topics(
            dir.path(),
            json!({"quotes": nine_plus_one}).as_object().unwrap(),
        );
        assert_eq!(eval_mix.kept_quotes, 1);
        assert_eq!(eval_mix.dropped_quotes, 9);
    }

    #[test]
    fn test_first_12_of_13_topics_and_first_8_of_9_quotes() {
        let dir = tempdir().unwrap();
        let mut lines = Vec::new();
        for i in 0..10 {
            lines.push(format!(
                r#"{{"start": {i}.0, "text": "Valid match phrase for quote number {i}."}}"#
            ));
        }
        fs::write(dir.path().join("audio.jsonl"), lines.join("\n")).unwrap();

        let mut topics = Vec::new();
        for i in 0..13 {
            topics.push(format!("Topic number {i}"));
        }
        let mut quotes = Vec::new();
        for i in 0..9 {
            quotes.push(json!({
                "speaker": "unknown",
                "text": format!("Valid match phrase for quote number {i}.")
            }));
        }

        let sense = json!({
            "topics": topics,
            "quotes": quotes
        });

        let eval = evaluate_topics(dir.path(), sense.as_object().unwrap());
        assert_eq!(eval.kept_quotes, 8);
        assert_eq!(eval.dropped_quotes, 1);
        let md = eval.markdown.unwrap();
        assert!(md.contains("- Topic number 11\n"));
        assert!(!md.contains("- Topic number 12\n"));
        assert!(md.contains("quote number 7."));
        assert!(!md.contains("quote number 8."));
    }

    #[test]
    fn invalid_topics_are_dropped_before_the_first_twelve_are_taken() {
        let dir = tempdir().unwrap();
        let mut topics = vec![json!("   "), json!("x".repeat(81))];
        for i in 0..13 {
            topics.push(json!(format!("Topic number {i}")));
        }
        let eval = evaluate_topics(dir.path(), json!({"topics": topics}).as_object().unwrap());
        let md = eval.markdown.unwrap();
        assert!(md.contains("- Topic number 0\n"));
        assert!(md.contains("- Topic number 11\n"));
        assert!(!md.contains("- Topic number 12\n"));
        assert!(!md.contains(&"x".repeat(81)));
    }

    #[test]
    fn same_words_under_different_labels_in_two_files_show_no_speaker() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("audio.jsonl"),
            r#"{"start": "00:00:01", "speaker": 1, "text": "let us check the dashboard"}"#,
        )
        .unwrap();
        fs::write(
            dir.path().join("imported_audio.jsonl"),
            r#"{"start": "00:00:02", "speaker": 2, "text": "let us check the dashboard"}"#,
        )
        .unwrap();
        let sense =
            json!({"quotes": [{"speaker": "Speaker 1", "text": "let us check the dashboard"}]});
        let eval = evaluate_topics(dir.path(), sense.as_object().unwrap());
        assert_eq!(eval.kept_quotes, 1);
        let md = eval.markdown.unwrap();
        assert!(!md.contains("Speaker 1"), "{md}");
        assert!(!md.contains("Speaker 2"), "{md}");
        assert!(md.contains("let us check the dashboard"), "{md}");
    }

    #[test]
    fn an_empty_speaker_label_shows_no_speaker() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("audio.jsonl"),
            r#"{"start": "00:00:01", "speaker": "  ", "text": "we ship on monday"}"#,
        )
        .unwrap();
        let sense = json!({"quotes": [{"speaker": "x", "text": "we ship on monday"}]});
        let md = evaluate_topics(dir.path(), sense.as_object().unwrap())
            .markdown
            .unwrap();
        assert!(md.contains("- we ship on monday"), "{md}");
        assert!(!md.contains(": we ship"), "{md}");
    }

    #[test]
    fn test_crlf_collapse_for_topics_and_quotes() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("audio.jsonl"),
            r#"{"start": 1.0, "speaker": "Sam", "text": "Speech with\r\nnewlines inside\nthe sentence."}"#,
        )
        .unwrap();

        let sense = json!({
            "topics": ["A topic with\r\n## Quotes\n- Speaker 1: forged bullet"],
            "quotes": [
                {"speaker": "Sam", "text": "Speech with newlines inside the sentence."}
            ]
        });

        let eval = evaluate_topics(dir.path(), sense.as_object().unwrap());
        let md = eval.markdown.unwrap();
        // Check that forged bullet is collapsed into one physical line
        assert_eq!(
            md,
            "## Topics\n- A topic with ## Quotes - Speaker 1: forged bullet\n## Quotes\n- Sam: Speech with newlines inside the sentence.\n"
        );
    }

    #[test]
    fn test_malformed_json_lines_skipped_without_invalidating_speech() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("audio.jsonl"),
            [
                r#"{"start": 1.0, "speaker": "Alex", "text": "Valid first sentence."}"#,
                "not json at all",
                "{\"no_start_and_not_speech\": true}",
                "42",
                r#"{"start": 2.0, "speaker": "Alex", "text": "Valid second sentence."}"#,
            ]
            .join("\n"),
        )
        .unwrap();

        let sense = json!({
            "quotes": [
                {"speaker": "Alex", "text": "Valid first sentence. Valid second sentence."}
            ]
        });

        let eval = evaluate_topics(dir.path(), sense.as_object().unwrap());
        assert_eq!(eval.kept_quotes, 1);
        let md = eval.markdown.unwrap();
        assert!(md.contains("- Alex: Valid first sentence. Valid second sentence.\n"));
    }

    #[test]
    fn test_unreadable_selected_file_drops_all_quotes_keeps_topics() {
        let dir = tempdir().unwrap();
        // A directory named audio.jsonl fails read_to_string
        fs::create_dir(dir.path().join("audio.jsonl")).unwrap();
        fs::write(
            dir.path().join("readable_audio.jsonl"),
            r#"{"start": 1.0, "text": "This would have matched if readable."}"#,
        )
        .unwrap();

        let sense = json!({
            "topics": ["Staging migration"],
            "quotes": [
                {"speaker": "unknown", "text": "This would have matched if readable."}
            ]
        });

        let eval = evaluate_topics(dir.path(), sense.as_object().unwrap());
        assert_eq!(eval.kept_quotes, 0);
        assert_eq!(eval.dropped_quotes, 1);
        assert!(!eval.unreadable.is_empty());
        let md = eval.markdown.unwrap();
        assert_eq!(md, "## Topics\n- Staging migration\n## Quotes\n");
    }

    #[test]
    fn test_no_selected_speech_file_keeps_topics_drops_quotes() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("screen.jsonl"), "{}\n").unwrap();
        fs::write(dir.path().join("imported.md"), "# Import\n").unwrap();

        let sense = json!({
            "topics": ["Database migration"],
            "quotes": [
                {"speaker": "unknown", "text": "Some spoken quote that cannot match."}
            ]
        });

        let eval = evaluate_topics(dir.path(), sense.as_object().unwrap());
        assert_eq!(eval.kept_quotes, 0);
        assert_eq!(eval.dropped_quotes, 1);
        let md = eval.markdown.unwrap();
        assert_eq!(md, "## Topics\n- Database migration\n## Quotes\n");
    }
    #[cfg(feature = "full-tests")]
    #[test]
    fn archive_quotes_are_checked_against_the_actual_part_transcript() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("p0000_0123456789abcdef.jsonl"),
            "{}\n{\"start\":\"12:00:10\",\"speaker\":\"Alex\",\"text\":\"The archived recording keeps these exact spoken words\"}\n").unwrap();
        let evaluation = evaluate_topics(
            root.path(),
            json!({"quotes":[{
                "speaker":"Alex", "text":"The archived recording keeps these exact spoken words"
            }]})
            .as_object()
            .unwrap(),
        );
        assert_eq!((evaluation.kept_quotes, evaluation.dropped_quotes), (1, 0));
    }
}
