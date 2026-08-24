// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use serde_json::Value;

use super::candidates::EdgeResolver;
use super::{
    EdgeContext, EdgeError, EdgeRow, EdgeValue, JsonObject, json_truthy, segment_start_ts_ms,
};
use solstone_core_journal::python_strip;

#[derive(Clone)]
struct MentionCandidate {
    entity_id: String,
    entity_name: String,
    variant: String,
}

struct MentionVariant {
    chars: Vec<char>,
}

pub(super) struct MentionCandidateIndex {
    candidates_by_key: BTreeMap<String, MentionCandidate>,
    variants: Vec<MentionVariant>,
    buckets: BTreeMap<char, Vec<usize>>,
}

pub(super) struct SpeakerExtraction {
    pub rows: Vec<EdgeRow>,
    pub warnings: Vec<String>,
}

#[derive(Clone)]
struct LabelRecord {
    speaker: String,
    sentence_id: i64,
}

pub(crate) fn extract_speaker_edges(
    payload: &JsonObject,
    context: &EdgeContext,
    journal: &Path,
    resolver: &mut EdgeResolver,
) -> Result<SpeakerExtraction, EdgeError> {
    let (composite_id, segment) = parse_speaker_label_path(&context.path)?;
    let Some(Value::Array(labels)) = payload.get("labels") else {
        return Ok(SpeakerExtraction {
            rows: Vec::new(),
            warnings: Vec::new(),
        });
    };

    let raw_speaker_ids = distinct_speaker_ids(labels);
    if raw_speaker_ids.is_empty() {
        return Ok(SpeakerExtraction {
            rows: Vec::new(),
            warnings: Vec::new(),
        });
    }

    let entities = load_journal_entities(journal)?;
    let speaker_ids = raw_speaker_ids
        .into_iter()
        .filter(|entity_id| {
            entities
                .iter()
                .find(|(id, _)| id == entity_id)
                .is_some_and(|(_, entity)| is_admissible_speaker_entity(entity))
        })
        .collect::<BTreeSet<_>>();
    let ts = segment_start_ts_ms(&context.day, &segment, resolver.owner_timezone()?)?;
    let mut rows = spoke_with_rows(&speaker_ids, context, &composite_id, ts);
    let mention_labels = valid_label_records(labels);
    if mention_labels.is_empty() {
        return Ok(SpeakerExtraction {
            rows,
            warnings: Vec::new(),
        });
    }

    let segment_dir = journal.join("chronicle").join(&composite_id);
    let mut warnings = Vec::new();
    let Some(transcript_stem) = select_transcript_stem(&segment_dir, &composite_id, &mut warnings)?
    else {
        return Ok(SpeakerExtraction { rows, warnings });
    };
    let Some(transcript_texts) =
        load_transcript_texts(&segment_dir, &transcript_stem, &composite_id, &mut warnings)?
    else {
        warnings.push(format!(
            "speaker edge transcript missing for {composite_id}"
        ));
        return Ok(SpeakerExtraction { rows, warnings });
    };
    if transcript_texts.is_empty() {
        return Ok(SpeakerExtraction { rows, warnings });
    }

    let candidates = resolver.mention_candidates()?;
    rows.extend(mentioned_rows(
        &mention_labels,
        &transcript_texts,
        candidates,
        context,
        &composite_id,
        ts,
    ));
    Ok(SpeakerExtraction { rows, warnings })
}

pub(super) fn build_candidate_index(journal: &Path) -> Result<MentionCandidateIndex, EdgeError> {
    let mut candidates_by_key = BTreeMap::new();
    let mut ambiguous = BTreeSet::new();
    for (entity_id, entity) in load_journal_entities(journal)? {
        if json_truthy(entity.get("blocked")) {
            continue;
        }
        let Some(Value::String(entity_name)) = entity.get("name") else {
            continue;
        };
        if python_strip(entity_name).is_empty() {
            continue;
        }

        for variant in entity_variants(&entity) {
            let key = python_casefold_key(&variant);
            if ambiguous.contains(&key) {
                continue;
            }
            let candidate = MentionCandidate {
                entity_id: entity_id.clone(),
                entity_name: entity_name.clone(),
                variant,
            };
            match candidates_by_key.get(&key) {
                None => {
                    candidates_by_key.insert(key, candidate);
                }
                Some(existing) if existing.entity_id != entity_id => {
                    candidates_by_key.remove(&key);
                    ambiguous.insert(key);
                }
                Some(_existing) => {}
            }
        }
    }

    let mut sorted: Vec<MentionCandidate> = candidates_by_key.values().cloned().collect();
    sorted.sort_by(|left, right| {
        right
            .variant
            .chars()
            .count()
            .cmp(&left.variant.chars().count())
            .then_with(|| {
                python_casefold_key(&left.variant).cmp(&python_casefold_key(&right.variant))
            })
            .then_with(|| left.variant.cmp(&right.variant))
    });
    let variants: Vec<MentionVariant> = sorted
        .into_iter()
        .map(|candidate| MentionVariant {
            chars: candidate.variant.chars().collect(),
        })
        .collect();
    let mut buckets: BTreeMap<char, Vec<usize>> = BTreeMap::new();
    for (index, variant) in variants.iter().enumerate() {
        let Some(first) = variant.chars.first().copied() else {
            continue;
        };
        buckets
            .entry(first_char_bucket_for_pattern(first))
            .or_default()
            .push(index);
    }
    Ok(MentionCandidateIndex {
        candidates_by_key,
        variants,
        buckets,
    })
}

fn parse_speaker_label_path(path: &str) -> Result<(String, String), EdgeError> {
    let normalized = path.replace('\\', "/");
    let parts: Vec<&str> = normalized.split('/').collect();
    if parts.len() != 5 || parts[3] != "talents" || parts[4] != "speaker_labels.json" {
        return Err(EdgeError::InvalidJsonPayload {
            source: "speaker labels",
            value_type: "invalid path",
        });
    }
    Ok((parts[..3].join("/"), parts[2].to_string()))
}

fn distinct_speaker_ids(labels: &[Value]) -> BTreeSet<String> {
    let mut speaker_ids = BTreeSet::new();
    for label in labels {
        let Value::Object(label) = label else {
            continue;
        };
        let Some(Value::String(speaker)) = label.get("speaker") else {
            continue;
        };
        if !speaker.is_empty() {
            speaker_ids.insert(speaker.clone());
        }
    }
    speaker_ids
}

fn valid_label_records(labels: &[Value]) -> Vec<LabelRecord> {
    let mut records = Vec::new();
    for label in labels {
        let Value::Object(label) = label else {
            continue;
        };
        let Some(Value::String(speaker)) = label.get("speaker") else {
            continue;
        };
        if speaker.is_empty() {
            continue;
        }
        let Some(Value::Number(sentence_id)) = label.get("sentence_id") else {
            continue;
        };
        let Some(sentence_id) = sentence_id.as_i64() else {
            continue;
        };
        if sentence_id <= 0 {
            continue;
        }
        records.push(LabelRecord {
            speaker: speaker.clone(),
            sentence_id,
        });
    }
    records
}

fn spoke_with_rows(
    speaker_ids: &BTreeSet<String>,
    context: &EdgeContext,
    composite_id: &str,
    ts: i64,
) -> Vec<EdgeRow> {
    let speakers: Vec<String> = speaker_ids.iter().cloned().collect();
    let mut rows = Vec::new();
    for left_index in 0..speakers.len() {
        for right in speakers.iter().skip(left_index + 1) {
            rows.push(EdgeRow {
                src: speakers[left_index].clone(),
                dst: right.clone(),
                kind: "spoke-with".to_string(),
                src_name: EdgeValue::Null,
                dst_name: EdgeValue::Null,
                day: Some(context.day.clone()),
                facet: Some(context.facet.clone()),
                source: "speaker".to_string(),
                path: context.path.clone(),
                anchor: Some(composite_id.to_string()),
                label: EdgeValue::Text(String::new()),
                ts: EdgeValue::Int(ts),
                weight: 1,
            });
        }
    }
    rows
}

fn select_transcript_stem(
    segment_dir: &Path,
    composite_id: &str,
    warnings: &mut Vec<String>,
) -> Result<Option<String>, EdgeError> {
    let mut npz_stems = Vec::new();
    let mut jsonl_stems = Vec::new();
    if segment_dir.is_dir() {
        for entry in fs::read_dir(segment_dir).map_err(|error| {
            EdgeError::Io(format!(
                "speaker transcript directory read failed for {}: {error}",
                segment_dir.display()
            ))
        })? {
            let entry = entry.map_err(|error| {
                EdgeError::Io(format!(
                    "speaker transcript directory entry read failed for {}: {error}",
                    segment_dir.display()
                ))
            })?;
            if !entry
                .file_type()
                .map_err(|error| {
                    EdgeError::Io(format!(
                        "speaker transcript file type read failed for {}: {error}",
                        entry.path().display()
                    ))
                })?
                .is_file()
            {
                continue;
            }
            let path = entry.path();
            let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
                continue;
            };
            if !is_qualifying_audio_stem(stem) {
                continue;
            }
            match path.extension().and_then(|extension| extension.to_str()) {
                Some("npz") => npz_stems.push(stem.to_string()),
                Some("jsonl") => jsonl_stems.push(stem.to_string()),
                _ => {}
            }
        }
    }
    npz_stems.sort();
    if let Some(stem) = npz_stems.first() {
        return Ok(Some(stem.clone()));
    }
    jsonl_stems.sort();
    if jsonl_stems.len() == 1 {
        return Ok(jsonl_stems.first().cloned());
    }
    warnings.push(format!(
        "speaker edge transcript unresolved for {composite_id}"
    ));
    Ok(None)
}

fn load_transcript_texts(
    segment_dir: &Path,
    stem: &str,
    composite_id: &str,
    warnings: &mut Vec<String>,
) -> Result<Option<BTreeMap<i64, String>>, EdgeError> {
    let transcript_path = segment_dir.join(format!("{stem}.jsonl"));
    if !transcript_path.exists() {
        return Ok(None);
    }
    let bytes = fs::read(&transcript_path).map_err(|error| {
        EdgeError::Io(format!(
            "speaker transcript read failed for {}: {error}",
            transcript_path.display()
        ))
    })?;
    let read =
        solstone_core_speaker_id::transcript::read_transcript_rows(&bytes).map_err(|error| {
            EdgeError::Io(format!(
                "speaker transcript decode failed for {}: {error}",
                transcript_path.display()
            ))
        })?;
    let mut texts = BTreeMap::new();
    for row in read.rows {
        let Some(Value::String(text)) = row.value.get("text") else {
            continue;
        };
        if !text.is_empty() {
            // Persisted sentence IDs can collide; preserve document order by keeping the last.
            if texts.contains_key(&row.sentence_id) {
                warnings.push(format!(
                    "speaker edge transcript duplicate sentence_id {} for {composite_id}",
                    row.sentence_id
                ));
            }
            texts.insert(row.sentence_id, text.clone());
        }
    }
    Ok(Some(texts))
}

fn mentioned_rows(
    labels: &[LabelRecord],
    transcript_texts: &BTreeMap<i64, String>,
    candidates: &MentionCandidateIndex,
    context: &EdgeContext,
    composite_id: &str,
    ts: i64,
) -> Vec<EdgeRow> {
    let mut sorted_labels = labels.to_vec();
    sorted_labels.sort_by_key(|label| label.sentence_id);
    let mut pair_sentence_ids: BTreeMap<(String, String), BTreeSet<i64>> = BTreeMap::new();
    let mut pair_labels = BTreeMap::new();
    let mut pair_names = BTreeMap::new();

    for label in sorted_labels {
        let Some(text) = transcript_texts.get(&label.sentence_id) else {
            continue;
        };
        for candidate in candidates.candidates_in_text(text) {
            if candidate.entity_id == label.speaker {
                continue;
            }
            let pair = (label.speaker.clone(), candidate.entity_id.clone());
            pair_labels
                .entry(pair.clone())
                .or_insert_with(|| candidate.variant.clone());
            pair_names
                .entry(pair.clone())
                .or_insert_with(|| candidate.entity_name.clone());
            pair_sentence_ids
                .entry(pair)
                .or_default()
                .insert(label.sentence_id);
        }
    }

    let mut rows = Vec::new();
    for ((speaker, target), sentence_ids) in pair_sentence_ids {
        rows.push(EdgeRow {
            src: speaker.clone(),
            dst: target.clone(),
            kind: "mentioned".to_string(),
            src_name: EdgeValue::Null,
            dst_name: EdgeValue::Text(pair_names[&(speaker.clone(), target.clone())].clone()),
            day: Some(context.day.clone()),
            facet: Some(context.facet.clone()),
            source: "mention".to_string(),
            path: context.path.clone(),
            anchor: Some(composite_id.to_string()),
            label: EdgeValue::Text(pair_labels[&(speaker.clone(), target.clone())].clone()),
            ts: EdgeValue::Int(ts),
            weight: sentence_ids.len() as i64,
        });
    }
    rows
}

impl MentionCandidateIndex {
    fn candidates_in_text(&self, text: &str) -> Vec<&MentionCandidate> {
        let mut matches = Vec::new();
        let mut index = 0;
        while index < text.len() {
            if !text.is_char_boundary(index) {
                index += 1;
                continue;
            }
            let Some(ch) = text[index..].chars().next() else {
                break;
            };
            if previous_char(text, index).is_some_and(is_word_char) {
                index += ch.len_utf8();
                continue;
            }
            let bucket = first_char_bucket_for_text(ch);
            let Some(variant_indexes) = self.buckets.get(&bucket) else {
                index += ch.len_utf8();
                continue;
            };
            let mut matched_end = None;
            for variant_index in variant_indexes {
                let variant = &self.variants[*variant_index];
                if let Some(end) = match_variant_at(text, index, variant)
                    && next_char(text, end).is_none_or(|next| !is_word_char(next))
                {
                    matched_end = Some(end);
                    break;
                }
            }
            if let Some(end) = matched_end {
                let key = python_casefold_key(&text[index..end]);
                if let Some(candidate) = self.candidates_by_key.get(&key) {
                    matches.push(candidate);
                }
                index = end;
            } else {
                index += ch.len_utf8();
            }
        }
        matches
    }
}

fn match_variant_at(text: &str, start: usize, variant: &MentionVariant) -> Option<usize> {
    let mut index = start;
    for pattern_ch in &variant.chars {
        let text_ch = text.get(index..)?.chars().next()?;
        if !literal_char_matches(*pattern_ch, text_ch) {
            return None;
        }
        index += text_ch.len_utf8();
    }
    Some(index)
}

fn previous_char(text: &str, index: usize) -> Option<char> {
    text.get(..index)?.chars().next_back()
}

fn next_char(text: &str, index: usize) -> Option<char> {
    text.get(index..)?.chars().next()
}

fn is_word_char(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

fn first_char_bucket_for_pattern(ch: char) -> char {
    if ch.is_ascii_alphabetic() {
        ch.to_ascii_lowercase()
    } else {
        ch
    }
}

fn first_char_bucket_for_text(ch: char) -> char {
    if ch.is_ascii_alphabetic() {
        return ch.to_ascii_lowercase();
    }
    match ch {
        'İ' | 'ı' => 'i',
        'ſ' => 's',
        'K' => 'k',
        _ => ch,
    }
}

fn literal_char_matches(pattern: char, text: char) -> bool {
    if pattern == text {
        return true;
    }
    if pattern.is_ascii_alphabetic() {
        let pattern = pattern.to_ascii_lowercase();
        if text.is_ascii_alphabetic() {
            return pattern == text.to_ascii_lowercase();
        }
        return matches!(
            (pattern, text),
            ('i', 'İ') | ('i', 'ı') | ('s', 'ſ') | ('k', 'K')
        );
    }
    false
}

fn load_journal_entities(journal: &Path) -> Result<Vec<(String, JsonObject)>, EdgeError> {
    let entities_dir = journal.join("entities");
    if !entities_dir.exists() {
        return Ok(Vec::new());
    }
    if !entities_dir.is_dir() {
        return Err(EdgeError::Io(format!(
            "{} is not a directory",
            entities_dir.display()
        )));
    }
    let mut entity_files = Vec::new();
    for entry in fs::read_dir(&entities_dir).map_err(|error| {
        EdgeError::Io(format!(
            "entity directory read failed for {}: {error}",
            entities_dir.display()
        ))
    })? {
        let entry = entry.map_err(|error| {
            EdgeError::Io(format!(
                "entity directory entry read failed for {}: {error}",
                entities_dir.display()
            ))
        })?;
        if !entry
            .file_type()
            .map_err(|error| {
                EdgeError::Io(format!(
                    "entity file type read failed for {}: {error}",
                    entry.path().display()
                ))
            })?
            .is_dir()
        {
            continue;
        }
        let entity_id = entry.file_name().to_string_lossy().into_owned();
        let entity_file = entry.path().join("entity.json");
        if entity_file.is_file() {
            entity_files.push((entity_id, entity_file));
        }
    }
    entity_files.sort_by(|left, right| left.0.cmp(&right.0));

    let mut entities = Vec::new();
    for (entity_id, entity_file) in entity_files {
        let text = fs::read_to_string(&entity_file).map_err(|error| {
            EdgeError::Io(format!(
                "entity read failed for {}: {error}",
                entity_file.display()
            ))
        })?;
        let Ok(Value::Object(entity)) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        entities.push((entity_id, entity));
    }
    Ok(entities)
}

// This duplicates `is_admissible_person` locally because indexer entities are raw JSON and one boolean does not warrant a new crate dependency.
fn is_admissible_speaker_entity(entity: &JsonObject) -> bool {
    entity.get("type").and_then(Value::as_str) == Some("Person")
        && !json_truthy(entity.get("blocked"))
}

fn entity_variants(entity: &JsonObject) -> Vec<String> {
    let mut sources = Vec::new();
    if let Some(Value::String(name)) = entity.get("name") {
        sources.push(name.clone());
    }
    if let Some(Value::Array(aka)) = entity.get("aka") {
        for alias in aka {
            if let Value::String(alias) = alias {
                sources.push(alias.clone());
            }
        }
    }

    let mut variants = Vec::new();
    let mut seen = BTreeSet::new();
    for source in sources {
        for variant in variants_from_name(&source) {
            if variant.chars().count() < 3 || !is_speakable(&variant) {
                continue;
            }
            let key = python_casefold_key(&variant);
            if seen.insert(key) {
                variants.push(variant);
            }
        }
    }
    variants
}

fn variants_from_name(name: &str) -> Vec<String> {
    let chars: Vec<char> = name.chars().collect();
    let mut base = String::new();
    let mut groups = Vec::new();
    let mut index = 0;
    while index < chars.len() {
        if chars[index] == '('
            && let Some(end) = chars.iter().skip(index + 1).position(|ch| *ch == ')')
        {
            while base.chars().last().is_some_and(char::is_whitespace) {
                base.pop();
            }
            groups.push(chars[index + 1..index + 1 + end].iter().collect::<String>());
            index += end + 2;
            continue;
        }
        base.push(chars[index]);
        index += 1;
    }

    let mut variants = Vec::new();
    let base = python_strip(&base);
    if !base.is_empty() {
        variants.push(base.to_string());
    }
    for group in groups {
        for item in group.split(',') {
            let item = python_strip(item);
            if !item.is_empty() {
                variants.push(item.to_string());
            }
        }
    }
    variants
}

fn is_speakable(name: &str) -> bool {
    name.chars().all(|ch| {
        ch.is_ascii_alphanumeric() || ch.is_whitespace() || matches!(ch, '.' | '-' | '\'')
    }) && name.chars().any(|ch| ch.is_ascii_alphabetic())
}

fn is_qualifying_audio_stem(stem: &str) -> bool {
    stem == "audio" || stem.ends_with("_audio")
}

fn python_casefold_key(value: &str) -> String {
    let mut folded = String::new();
    for ch in value.chars() {
        match ch {
            'ß' | 'ẞ' => folded.push_str("ss"),
            'ſ' => folded.push('s'),
            'K' => folded.push('k'),
            _ => folded.extend(ch.to_lowercase()),
        }
    }
    folded
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::reserve_temp_path;
    use serde_json::json;

    fn temp_root(name: &str) -> std::path::PathBuf {
        reserve_temp_path(&format!("solstone-core-speaker-{name}"))
    }

    fn write_entity(root: &Path, entity_id: &str, value: Value) {
        let path = root.join("entities").join(entity_id).join("entity.json");
        fs::create_dir_all(path.parent().expect("entity path should have parent"))
            .expect("create entity directory");
        fs::write(
            path,
            serde_json::to_string(&value).expect("encode entity json"),
        )
        .expect("write entity");
    }

    #[test]
    fn variants_split_parentheticals_and_filter_unspeakable_names() {
        let mut entity = JsonObject::new();
        entity.insert("name".to_string(), json!("Chris One (Ray, C. One)"));
        entity.insert("aka".to_string(), json!(["Chris Ray", "短名", "Z"]));
        assert_eq!(
            entity_variants(&entity),
            vec!["Chris One", "Ray", "C. One", "Chris Ray"]
        );
    }

    #[test]
    fn mention_index_drops_ambiguous_casefold_and_blocks_entities() {
        let root = temp_root("candidate-index");
        write_entity(
            &root,
            "edge_zephyr",
            json!({"name":"Zephyr Person","aka":["Project Zephyr"]}),
        );
        write_entity(&root, "edge_left", json!({"name":"Case Twin"}));
        write_entity(&root, "edge_right", json!({"name":"case twin"}));
        write_entity(
            &root,
            "edge_blocked",
            json!({"name":"Blocked Target","blocked":true}),
        );

        let index = build_candidate_index(&root).expect("build speaker candidate index");
        let matches = index
            .candidates_in_text("Project Zephyr met Case Twin and Blocked Target.")
            .iter()
            .map(|candidate| candidate.entity_id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(matches, vec!["edge_zephyr"]);
        fs::remove_dir_all(root).expect("cleanup candidate index root");
    }

    #[test]
    fn matcher_enforces_unicode_boundaries_longest_wins_and_casefold_drop() {
        assert_eq!(
            python_casefold_key("ſam Kilo İris Straße ẞ"),
            "sam kilo i\u{307}ris strasse ss"
        );

        let mut candidates = BTreeMap::new();
        candidates.insert(
            "ann".to_string(),
            MentionCandidate {
                entity_id: "ann".to_string(),
                entity_name: "Ann".to_string(),
                variant: "Ann".to_string(),
            },
        );
        candidates.insert(
            "ann lee".to_string(),
            MentionCandidate {
                entity_id: "ann_lee".to_string(),
                entity_name: "Ann Lee".to_string(),
                variant: "Ann Lee".to_string(),
            },
        );
        candidates.insert(
            "iris example".to_string(),
            MentionCandidate {
                entity_id: "iris".to_string(),
                entity_name: "Iris Example".to_string(),
                variant: "Iris Example".to_string(),
            },
        );
        candidates.insert(
            "project zephyr".to_string(),
            MentionCandidate {
                entity_id: "zephyr".to_string(),
                entity_name: "Zephyr".to_string(),
                variant: "Project Zephyr".to_string(),
            },
        );
        let variants = vec![
            MentionVariant {
                chars: "Ann Lee".chars().collect(),
            },
            MentionVariant {
                chars: "Project Zephyr".chars().collect(),
            },
            MentionVariant {
                chars: "Ann".chars().collect(),
            },
            MentionVariant {
                chars: "Iris Example".chars().collect(),
            },
        ];
        let mut buckets = BTreeMap::new();
        buckets.insert('a', vec![0, 2]);
        buckets.insert('p', vec![1]);
        buckets.insert('i', vec![3]);
        let index = MentionCandidateIndex {
            candidates_by_key: candidates,
            variants,
            buckets,
        };

        assert_eq!(
            index
                .candidates_in_text("éProject Zephyr and Project Zephyrñ")
                .len(),
            0
        );
        assert_eq!(
            index
                .candidates_in_text("Project Zephyr met İris Example.")
                .iter()
                .map(|candidate| candidate.entity_id.as_str())
                .collect::<Vec<_>>(),
            vec!["zephyr"]
        );
        assert_eq!(
            index
                .candidates_in_text("Ann Lee and Ann.")
                .iter()
                .map(|candidate| candidate.entity_id.as_str())
                .collect::<Vec<_>>(),
            vec!["ann_lee", "ann"]
        );
    }

    // AC12 oracle for comparing the pre-switch line-number behavior on safe fixtures.
    fn legacy_load_transcript_texts_for_oracle(path: &Path) -> BTreeMap<i64, String> {
        let text = fs::read_to_string(path).unwrap();
        let mut texts = BTreeMap::new();
        for (line_no, line) in text.lines().enumerate() {
            if line_no == 0 || line.trim().is_empty() {
                continue;
            }
            let Ok(Value::Object(entry)) = serde_json::from_str::<Value>(line) else {
                continue;
            };
            let Some(Value::String(text)) = entry.get("text") else {
                continue;
            };
            if !text.is_empty() {
                texts.insert(line_no as i64, text.clone());
            }
        }
        texts
    }

    fn transcript_result(body: &[u8]) -> (BTreeMap<i64, String>, Vec<String>) {
        let root = temp_root("transcript-reader");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("audio.jsonl"), body).unwrap();
        let mut warnings = Vec::new();
        let texts =
            load_transcript_texts(&root, "audio", "20260808/default/120000_300", &mut warnings)
                .unwrap()
                .unwrap();
        fs::remove_dir_all(root).unwrap();
        (texts, warnings)
    }

    #[test]
    fn transcript_reader_matches_legacy_without_persisted_ids() {
        let root = temp_root("legacy-oracle");
        fs::create_dir_all(&root).unwrap();
        let path = root.join("audio.jsonl");
        fs::write(&path, "header\n{\"text\":\"one\"}\n{\"text\":\"two\"}\n").unwrap();
        let mut warnings = Vec::new();
        let actual = load_transcript_texts(&root, "audio", "fixture", &mut warnings)
            .unwrap()
            .unwrap();
        assert_eq!(actual, legacy_load_transcript_texts_for_oracle(&path));
        assert_eq!(
            actual,
            BTreeMap::from([(1, "one".to_owned()), (2, "two".to_owned())])
        );
        assert!(warnings.is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn transcript_reader_preserves_ordinals_after_blank_and_malformed_lines() {
        let (texts, warnings) = transcript_result(
            b"header\n{\"text\":\"one\"}\n\n{bad}\n{\"text\":\"four\"}\n{\"text\":\"five\"}",
        );
        assert_eq!(
            texts,
            BTreeMap::from([
                (1, "one".to_owned()),
                (4, "four".to_owned()),
                (5, "five".to_owned()),
            ])
        );
        assert!(warnings.is_empty());
    }

    #[test]
    fn transcript_reader_filters_absent_empty_and_non_string_text() {
        let (texts, warnings) =
            transcript_result(b"header\n{}\n{\"text\":\"\"}\n{\"text\":null}\n{\"text\":\"four\"}");
        assert_eq!(texts, BTreeMap::from([(4, "four".to_owned())]));
        assert!(warnings.is_empty());
    }

    #[test]
    fn transcript_reader_handles_empty_header_cr_and_persisted_ids() {
        for body in [b"".as_slice(), b"header".as_slice()] {
            let (texts, warnings) = transcript_result(body);
            assert!(texts.is_empty());
            assert!(warnings.is_empty());
        }
        let (texts, _) = transcript_result(b"header\r{\"text\":\"one\"}\r{\"text\":\"two\"}");
        assert_eq!(
            texts,
            BTreeMap::from([(1, "one".to_owned()), (2, "two".to_owned())])
        );
        let (texts, _) = transcript_result(b"header\n{\"sentence_id\":9,\"text\":\"persisted\"}");
        assert_eq!(texts, BTreeMap::from([(9, "persisted".to_owned())]));
    }

    #[test]
    fn transcript_reader_warns_and_keeps_last_duplicate() {
        let (texts, warnings) = transcript_result(
            b"header\n{\"sentence_id\":3,\"text\":\"first\"}\n{\"sentence_id\":3,\"text\":\"last\"}\n",
        );
        assert_eq!(texts, BTreeMap::from([(3, "last".to_owned())]));
        assert_eq!(
            warnings,
            vec!["speaker edge transcript duplicate sentence_id 3 for 20260808/default/120000_300"]
        );
    }

    #[test]
    fn duplicate_transcript_warning_reaches_speaker_extraction() {
        let root = temp_root("duplicate-transcript-warning");
        let composite_id = "20260808/default/120000_300";
        let segment_dir = root.join("chronicle").join(composite_id);
        fs::create_dir_all(&segment_dir).unwrap();
        fs::write(
            segment_dir.join("audio.jsonl"),
            "header\n{\"sentence_id\":3,\"text\":\"first\"}\n{\"sentence_id\":3,\"text\":\"last\"}\n",
        )
        .unwrap();
        let payload = json!({
            "labels": [{"speaker": "speaker_one", "sentence_id": 3}]
        });
        let context = EdgeContext {
            path: format!("{composite_id}/talents/speaker_labels.json"),
            day: "20260808".to_owned(),
            facet: String::new(),
        };
        let mut resolver = EdgeResolver::new(&root);

        let extracted =
            extract_speaker_edges(payload.as_object().unwrap(), &context, &root, &mut resolver)
                .unwrap();

        assert_eq!(
            extracted.warnings,
            vec!["speaker edge transcript duplicate sentence_id 3 for 20260808/default/120000_300"]
        );
        fs::remove_dir_all(root).unwrap();
    }
}
