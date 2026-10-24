// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! A source-bound model view, separate from the original transcript.
mod spelling;
use crate::PreparedTalent;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use solstone_core_transcripts::EntrySource;

pub(crate) const VIEW_KEY: &str = "_product_spelling_view";
const TALENTS: &[&str] = &["sense", "work", "conversation", "screen", "participation"];

#[derive(Clone, Debug, Deserialize, Serialize)]
struct Line {
    source_path: String,
    source_sha256: String,
    record: usize,
    field: String,
    speaker: String,
    original_start: usize,
    view_start: usize,
    original: String,
    text: String,
    edits: Vec<spelling::Edit>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
struct View {
    text: String,
    original_sha256: String,
    view_sha256: String,
    lines: Vec<Line>,
    gaps: Vec<Value>,
}
fn hash(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}
fn view(config: &Map<String, Value>) -> Option<View> {
    serde_json::from_value(config.get(VIEW_KEY)?.clone()).ok()
}

pub(crate) fn prepare(
    name: &str,
    config: &mut Map<String, Value>,
    sources: &[Option<EntrySource>],
) {
    config.remove(VIEW_KEY);
    if !TALENTS.contains(&name) || config.contains_key("messages") {
        return;
    }
    let Some(original) = config.get("transcript").and_then(Value::as_str) else {
        return;
    };
    if !spelling::has_variant(original) {
        return;
    }
    let shift = if original.starts_with(crate::prepare::SPARSE_INPUT_NOTE) {
        crate::prepare::SPARSE_INPUT_NOTE.len()
    } else {
        0
    };
    let mut lines = Vec::new();
    let mut gaps = Vec::new();
    for source in sources
        .iter()
        .flatten()
        .filter(|s| s.file.ends_with("audio.jsonl"))
    {
        let mut record_cursor = 0;
        for ((chunk, record), at) in source.chunks.iter().zip(&source.chunk_offsets) {
            if !record.contains_key("start") {
                continue;
            }
            let (field, raw) = match record
                .get("corrected")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
            {
                Some(text) => ("corrected", text),
                None => match record.get("text").and_then(Value::as_str) {
                    Some(text) => ("text", text),
                    None => continue,
                },
            };
            let (text, edits) = spelling::normalize(raw);
            let record_index = source.records[record_cursor..]
                .iter()
                .position(|r| r == record)
                .map(|i| i + record_cursor);
            if let Some(i) = record_index {
                record_cursor = i + 1;
            }
            let positions: Vec<_> = if raw.is_empty() {
                Vec::new()
            } else {
                chunk.match_indices(raw).map(|(i, _)| i).collect()
            };
            let chunk_start = at.checked_add(shift);
            let bound = chunk_start
                .and_then(|start| start.checked_add(chunk.len()).map(|end| (start, end)))
                .filter(|(start, end)| {
                    *start >= source.rendered_range.start.saturating_add(shift)
                        && *end <= source.rendered_range.end.saturating_add(shift)
                        && original.get(*start..*end) == Some(chunk.as_str())
                });
            let start = bound.and_then(|(start, _)| {
                positions
                    .first()
                    .filter(|_| positions.len() == 1)
                    .and_then(|i| start.checked_add(*i))
            });
            let Some((start, record_index)) = start.zip(record_index) else {
                if !edits.is_empty() {
                    gaps.push(json!({"source_path":source.path,"source_sha256":source.sha256,"record":record_index,"reason":"unbound_statement"}));
                }
                continue;
            };
            let speaker = match record.get("speaker") {
                Some(Value::Number(n)) => format!("Speaker {n}"),
                Some(Value::String(s)) => s.clone(),
                _ => "unknown".to_owned(),
            };
            lines.push(Line {
                source_path: source.path.display().to_string(),
                source_sha256: source.sha256.clone(),
                record: record_index,
                field: field.to_owned(),
                speaker,
                original_start: start,
                view_start: 0,
                original: raw.to_owned(),
                text,
                edits,
            });
        }
    }
    let mut all = lines
        .iter()
        .flat_map(|line| {
            line.edits.iter().map(move |e| {
                (
                    line.original_start + e.start,
                    line.original_start + e.end,
                    e.replacement.clone(),
                )
            })
        })
        .collect::<Vec<_>>();
    all.sort_by_key(|e| e.0);
    if all.is_empty() && gaps.is_empty() {
        return;
    }
    let mut text = String::with_capacity(original.len());
    let mut cursor = 0;
    for (start, end, replacement) in &all {
        if *start < cursor || original.get(*start..*end).is_none() {
            return;
        }
        text.push_str(&original[cursor..*start]);
        text.push_str(replacement);
        cursor = *end;
    }
    text.push_str(&original[cursor..]);
    for line in &mut lines {
        let delta: isize = all
            .iter()
            .filter(|(_, end, _)| *end <= line.original_start)
            .map(|(start, end, replacement)| replacement.len() as isize - (*end - *start) as isize)
            .sum();
        line.view_start = line
            .original_start
            .checked_add_signed(delta)
            .expect("non-overlapping source view");
    }
    let result = View {
        original_sha256: hash(original),
        view_sha256: hash(&text),
        text,
        lines,
        gaps,
    };
    config.insert(
        VIEW_KEY.to_owned(),
        serde_json::to_value(result).expect("source spelling view"),
    );
}

pub(crate) fn model_text(config: &Map<String, Value>) -> Option<&str> {
    config.get(VIEW_KEY)?.get("text")?.as_str()
}

/// Plan screen batches in view coordinates, without reusing the full view in
/// each batch. The caller's original transcript and source coordinates stay put.
pub(crate) fn screen_batch_view(
    prepared: &PreparedTalent,
) -> Result<Option<PreparedTalent>, String> {
    let Some(v) = view(&prepared.config) else {
        return Ok(None);
    };
    let mut candidate = prepared.clone();
    let Some(cuts) = candidate
        .config
        .get_mut("_screen_batch_cuts")
        .and_then(Value::as_array_mut)
    else {
        return Ok(None);
    };
    for cut in cuts {
        for field in ["byte_offset", "observation_byte_offset"] {
            let at = cut
                .get(field)
                .and_then(Value::as_u64)
                .and_then(|at| usize::try_from(at).ok())
                .ok_or("screen_source_unbatchable: invalid spelling-view cut")?;
            let mut delta = 0isize;
            for line in &v.lines {
                for edit in &line.edits {
                    let start = line.original_start + edit.start;
                    let end = line.original_start + edit.end;
                    if at > start && at < end {
                        return Err("screen_source_unbatchable: cut splits a spelling edit".into());
                    }
                    if at >= end {
                        delta += edit.replacement.len() as isize - (end - start) as isize;
                    }
                }
            }
            cut[field] = json!(
                at.checked_add_signed(delta)
                    .ok_or("screen_source_unbatchable: invalid spelling-view offset")?
            );
        }
    }
    candidate
        .config
        .insert("transcript".into(), Value::String(v.text));
    candidate.config.remove(VIEW_KEY);
    Ok(Some(candidate))
}

pub(crate) fn event(config: &Map<String, Value>) -> Option<Value> {
    let v = view(config)?;
    Some(
        json!({"event":"product_spelling_view","coordinate_units":"UTF-8 bytes in decoded source field and rendered input","original_sha256":v.original_sha256,"view_sha256":v.view_sha256,"gaps":v.gaps,
        "sources":v.lines.iter().filter(|l|!l.edits.is_empty()).map(|l|json!({"source_path":l.source_path,"source_sha256":l.source_sha256,"record":l.record,"field":l.field,"speaker":l.speaker,"original_start":l.original_start,"view_start":l.view_start,"edits":l.edits})).collect::<Vec<_>>() }),
    )
}
fn actor_key(s: &str) -> String {
    let compact = s
        .to_lowercase()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>();
    if let Some(n) = compact
        .strip_prefix("speaker")
        .filter(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
    {
        n.to_owned()
    } else {
        compact
    }
}
fn canonical(text: &str) -> bool {
    let lower = text.to_lowercase();
    lower.contains("solstone") || lower.contains("sol pbc")
}
fn bindings(v: &View, text: &str, speaker: Option<&str>, field: &str) -> Vec<Value> {
    v.lines.iter().filter(|l|speaker.is_none_or(|s|matches!(actor_key(s).as_str(),"unknown"|"unidentified")||actor_key(s)==actor_key(&l.speaker)))
        .flat_map(|l|l.text.match_indices(text).filter_map(move|(start,_)|{
            let span=spelling::original_range(start,start+text.len(),&l.edits)?;
            l.original.get(span.clone())?;
            Some(json!({"field":field,"source_path":l.source_path,"source_sha256":l.source_sha256,"record":l.record,"source_field":l.field,"speaker":l.speaker,"raw_start":span.start,"raw_end":span.end,"view_start":l.view_start+start,"view_end":l.view_start+start+text.len()}))
        })).collect()
}

pub(crate) struct CheckedResponse {
    pub text: String,
    pub bindings: Vec<Value>,
    pub gaps: Vec<Value>,
}

/// Drop unsupported structured quotes, retaining the rest of the completion.
pub(crate) fn verify(prepared: &PreparedTalent, response: &str) -> Result<CheckedResponse, String> {
    let mut result = CheckedResponse {
        text: response.to_owned(),
        bindings: Vec::new(),
        gaps: Vec::new(),
    };
    let Some(v) = view(&prepared.config) else {
        return Ok(result);
    };
    let Ok(mut value) = serde_json::from_str::<Value>(response) else {
        return Ok(result);
    };
    let mut check = |text: &str, speaker: Option<&str>, field: &str, action: &str| {
        if !canonical(text) {
            return true;
        }
        let found = bindings(&v, text, speaker, field);
        if found.is_empty() {
            result.gaps.push(json!({
                "event": "product_quote_gap",
                "field": field,
                "detail": format!("canonical quote has no source span: {field}"),
                "action": action,
            }));
            return false;
        }
        result.bindings.extend(found);
        true
    };
    if let Some(quotes) = value.get_mut("quotes").and_then(Value::as_array_mut) {
        let mut index = 0;
        quotes.retain(|q| {
            let field = format!("quotes.{index}.text");
            index += 1;
            q.get("text").and_then(Value::as_str).is_none_or(|text| {
                check(
                    text,
                    q.get("speaker").and_then(Value::as_str),
                    &field,
                    "drop_quote",
                )
            })
        });
    }
    if let Some(relations) = value.get_mut("relations").and_then(Value::as_array_mut) {
        let mut index = 0;
        relations.retain(|r| {
            let field = format!("relations.{index}.quote");
            index += 1;
            r.get("quote")
                .and_then(Value::as_str)
                .is_none_or(|text| check(text, None, &field, "skip_relation"))
        });
    }
    if let Some(body) = value.get("body").and_then(Value::as_str) {
        let quoted = regex::Regex::new(r#""([^"\n]+)"|“([^”\n]+)”"#).expect("quoted prose");
        for (i, c) in quoted.captures_iter(body).enumerate() {
            let field = format!("body.quote.{i}");
            if !check(
                c.get(1).or_else(|| c.get(2)).unwrap().as_str(),
                None,
                &field,
                "refuse_prose",
            ) {
                return Err(format!("canonical quote has no source span: {field}"));
            }
        }
    }
    if !result.gaps.is_empty() {
        result.text = value.to_string();
    }
    Ok(result)
}

pub(crate) fn canonical_owner_quote(quote: &str, owner_lines: &[String]) -> bool {
    if quote.split_whitespace().count() < 3 {
        return false;
    }
    owner_lines.iter().any(|raw| {
        !spelling::normalize(raw).1.is_empty() && !spelling::quote_ranges(raw, quote).is_empty()
    })
}

/// Only quotes bound to voice-identified owner lines in this exact input view
/// may use spelling equivalence in the later owner-evidence check.
pub(crate) fn owner_quotes(config: &Map<String, Value>, output: &Value) -> Vec<String> {
    let Some(v) = view(config) else {
        return Vec::new();
    };
    output
        .get("relations")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|r| r.get("quote").and_then(Value::as_str))
        .filter(|quote| {
            canonical(quote) && !bindings(&v, quote, Some("You"), "relations.quote").is_empty()
        })
        .map(str::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use solstone_core_generate::{
        ContentPart, GenerateRequest, GenerateResponse, GeneratedResponse,
    };
    use solstone_core_transcripts::{Sources, TalentSource};
    use std::{fs, sync::Mutex};
    fn load(
        root: &std::path::Path,
        voices: Option<solstone_core_transcripts::VoiceNames>,
    ) -> solstone_core_transcripts::ScreenTranscript {
        solstone_core_transcripts::cluster_period_with_memory(
            root,
            "20260101",
            "090000_300",
            &Sources {
                transcripts: true,
                percepts: true,
                talents: TalentSource::Disabled,
                voices,
            },
            Some("watch"),
        )
        .0
    }
    fn speech(root: &std::path::Path, rows: &[Value]) -> std::path::PathBuf {
        let p = root.join("chronicle/20260101/watch/090000_300/audio.jsonl");
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(
            &p,
            rows.iter()
                .map(|v| v.to_string() + "\n")
                .collect::<String>(),
        )
        .unwrap();
        p
    }
    fn config(text: &str) -> Map<String, Value> {
        json!({"transcript":text,"user_instruction":"Keep Soulstone in this instruction.","max_output_tokens":1024}).as_object().unwrap().clone()
    }
    #[test]
    fn typed_source_view_preserves_headers_percepts_instructions_and_raw_bytes() {
        let root = tempfile::tempdir().unwrap();
        let path = speech(
            root.path(),
            &[
                json!({"title":"Soulstone header"}),
                json!({"start":"09:00:01","speaker":1,"text":"é Today I use Soulstone journal."}),
                json!({"start":"09:00:02","speaker":2,"text":"é Today I use solstone journal."}),
            ],
        );
        let raw = fs::read(&path).unwrap();
        let loaded = load(root.path(), None);
        let original =
            loaded.text.clone() + "\n### Screen Activity\nSoulstone remains screen text.";
        let mut c = config(&original);
        let instruction = c["user_instruction"].clone();
        prepare("sense", &mut c, &loaded.sources);
        assert_eq!(c["transcript"], original);
        assert_eq!(c["user_instruction"], instruction);
        assert_eq!(fs::read(&path).unwrap(), raw);
        let v = view(&c).unwrap();
        assert!(v.gaps.is_empty(), "{:?}", v.gaps);
        assert!(v.text.contains("Soulstone header"));
        assert!(v.text.contains("Soulstone remains screen text."));
        assert!(v.text.contains("é Today I use solstone journal."));
        assert_eq!(
            v.lines[0].source_sha256,
            format!("{:x}", Sha256::digest(&raw))
        );
        let prepared = PreparedTalent {
            name: "sense".into(),
            config: c,
        };
        let b = verify(
            &prepared,
            &json!({"quotes":[{"speaker":"Speaker 1","text":"é Today I use solstone journal."}]})
                .to_string(),
        )
        .unwrap();
        let b = b.bindings;
        assert_eq!(b.len(), 1);
        assert_eq!(b[0]["record"], 1);
        assert_eq!(b[0]["speaker"], "Speaker 1");
        assert_eq!(
            &v.lines[0].original[b[0]["raw_start"].as_u64().unwrap() as usize
                ..b[0]["raw_end"].as_u64().unwrap() as usize],
            "é Today I use Soulstone journal."
        );
        assert!(
            verify(
                &prepared,
                &json!({"quotes":[{"speaker":"Speaker 3","text":"é Today I use solstone journal."}]})
                    .to_string()
            )
            .unwrap().gaps.len() == 1
        );
        for name in ["pulse", "documents"] {
            let mut c = config(&original);
            prepare(name, &mut c, &loaded.sources);
            assert!(event(&c).is_none());
        }
        let mut c = config(&original);
        c.insert("messages".into(), json!([]));
        prepare("sense", &mut c, &loaded.sources);
        assert!(event(&c).is_none());
    }
    #[test]
    fn variant_free_input_is_a_noop_and_unbound_source_has_a_gap() {
        let root = tempfile::tempdir().unwrap();
        speech(
            root.path(),
            &[
                json!({"start":"09:00:01","speaker":1,"text":"Mina repaired the garden gate and still needs paint."}),
            ],
        );
        let loaded = load(root.path(), None);
        let mut c = config(&loaded.text);
        let before = c.clone();
        prepare("sense", &mut c, &loaded.sources);
        assert_eq!(c, before);
        assert!(event(&c).is_none());
        speech(
            root.path(),
            &[
                json!({"start":"09:00:01","speaker":1,"text":"I tested the Soulstone journal on my phone today."}),
            ],
        );
        let mut loaded = load(root.path(), None);
        loaded.sources[0].as_mut().unwrap().chunk_offsets[0] += 1;
        let mut c = config(&loaded.text);
        prepare("sense", &mut c, &loaded.sources);
        let e = event(&c).unwrap();
        assert_eq!(e["gaps"].as_array().unwrap().len(), 1);
        assert_eq!(model_text(&c), Some(loaded.text.as_str()));
    }
    #[test]
    fn preview_equals_execution_and_unbound_quotes_are_dropped_before_publication() {
        let root = tempfile::tempdir().unwrap();
        let journal = root.path().join("journal");
        fs::create_dir_all(journal.join("config")).unwrap();
        fs::write(
            journal.join("config/journal.json"),
            r#"{"providers":{"active":{"provider":"test","model":"test-model"}}}"#,
        )
        .unwrap();
        let source = speech(
            &journal,
            &[
                json!({"start":"09:00:01","speaker":1,"text":"I tested the Soulstone personal-memory journal on my phone today."}),
            ],
        );
        let raw = fs::read(&source).unwrap();
        let paths = crate::prepare::RuntimePaths {
            talent_root: root.path().join("talent"),
            apps_root: root.path().join("apps"),
            templates_dir: root.path().join("templates"),
        };
        for p in [&paths.talent_root, &paths.apps_root, &paths.templates_dir] {
            fs::create_dir_all(p).unwrap();
        }
        fs::write(paths.talent_root.join("sense.md"),"{\n\"type\":\"generate\",\"max_output_tokens\":1024,\"load\":{\"transcripts\":true,\"percepts\":true},\"output\":\"json\"\n}\nExtract only source evidence.\n").unwrap();
        let context = crate::ExecutionContext {
            journal: journal.clone(),
        };
        let preview = crate::assemble::assemble_prompt_preview(
            &solstone_core_talent_cli::preview::PreviewRequest {
                name: "sense".into(),
                day: Some("20260101".into()),
                segment: Some("090000_300".into()),
                facet: None,
                activity: None,
            },
            &paths,
            &context,
        );
        let solstone_core_talent_cli::preview::PromptPreview::Assembled { parts, .. } = preview
        else {
            panic!("{preview:?}")
        };
        assert!(parts[0].contains("solstone personal-memory"));
        let captured = Mutex::new(Vec::new());
        let supported = json!({"speaker":"Speaker 1","text":"I tested the solstone personal-memory journal on my phone today."});
        let expected = json!({"title":"Phone test", "quotes":[supported.clone()]});
        let output=json!({"title":"Phone test","quotes":[
            {"speaker":"Speaker 1","text":"I invented a solstone quantum phone."},
            supported,
            {"speaker":"Speaker 2","text":"I tested the solstone personal-memory journal on my phone today."}
        ]}).to_string();
        let executor = |request: &GenerateRequest| {
            *captured.lock().unwrap() = request.contents.clone();
            Ok(GenerateResponse::Generated(Box::new(GeneratedResponse {
                id: None,
                text: output.clone(),
                model: "test-model".into(),
                usage: json!({}),
                finish_reason: "stop".into(),
                thinking: None,
                schema_validation: None,
                input_budget: None,
                request_budget: None,
                inference: None,
                hints_applied: Vec::new(),
            })))
        };
        let dest = journal.join("result.json");
        let mut events = Vec::new();
        let result=crate::execute_request(json!({"name":"sense","day":"20260101","segment":"090000_300","stream":"watch","prompt":"","output":"json","output_path":dest}).as_object().unwrap().clone(),&paths,&context,&executor,&mut events,&solstone_core_generate::Cancellation::never());
        assert!(
            matches!(result, crate::RuntimeOutcome::Finished { .. }),
            "{result:?}"
        );
        assert_eq!(
            *captured.lock().unwrap(),
            parts
                .into_iter()
                .map(|text| ContentPart::Text { text })
                .collect::<Vec<_>>()
        );
        assert_eq!(
            serde_json::from_str::<Value>(&fs::read_to_string(dest).unwrap()).unwrap(),
            expected
        );
        assert_eq!(fs::read(source).unwrap(), raw);
        let events = String::from_utf8(events).unwrap();
        assert!(events.contains("product_spelling_view"));
        assert!(events.contains("product_quote_bindings"));
        assert_eq!(gap_fields(&events), ["quotes.0.text", "quotes.2.text"]);
    }
    #[test]
    fn unbound_relations_are_skipped_and_supported_owner_evidence_persists() {
        for is_owner in [true, false] {
            let root = tempfile::tempdir().unwrap();
            let write = |rel: &str, value: Value| {
                let p = root.path().join(rel);
                fs::create_dir_all(p.parent().unwrap()).unwrap();
                fs::write(p, value.to_string()).unwrap();
            };
            solstone_core_facets::create_facet(root.path(), "work", "Work", "", "", "", None)
                .unwrap();
            write(
                "config/journal.json",
                json!({"identity":{"name":"Jordan Rivers"}}),
            );
            write(
                "entities/jordan/entity.json",
                json!({"id":"jordan","name":"Jordan Rivers","type":"Person","is_principal":true}),
            );
            write(
                "facets/work/activities/20260101.jsonl",
                json!({"id":"meeting-1","activity":"meeting","stream":"watch","segments":["090000_300"]}),
            );
            let source = speech(
                root.path(),
                &[
                    json!({"start":"09:00:01","speaker":1,"sentence_id":1,"text":"I use Soulstone journal every morning."}),
                ],
            );
            let raw = fs::read(&source).unwrap();
            write(
                "chronicle/20260101/watch/090000_300/talents/speaker_labels.json",
                json!({"labels":[{"sentence_id":1,"speaker":if is_owner{"jordan"}else{"priya"},"method":"user_confirmed"}]}),
            );
            let voices = solstone_core_transcripts::VoiceNames {
                principal: "jordan".into(),
                owner_voice_confirmed: false,
                people: std::collections::BTreeMap::from([("priya".into(), "Priya".into())]),
            };
            let loaded = load(root.path(), Some(voices));
            let mut c = config(&loaded.text);
            c.insert("hook".into(), json!({"post":"story"}));
            let body = crate::story::source_body("conversation", &c, &loaded.sources).unwrap();
            assert!(body.contains("Soulstone"));
            prepare("conversation", &mut c, &loaded.sources);
            assert_eq!(
                crate::story::source_body("conversation", &c, &loaded.sources),
                Some(body)
            );
            c.extend(json!({"facet":"work","destination_id":solstone_core_facets::observe_facet_write_identity(root.path(),"work").unwrap(),"day":"20260101","activity":{"id":"meeting-1"}}).as_object().unwrap().clone());
            let prepared = PreparedTalent {
                name: "conversation".into(),
                config: c,
            };
            let supported = json!({"from":"you","to":"solstone","kind":"uses","note":"","quote":"I use solstone journal every morning."});
            let output=json!({"body":"A discussion of the journal.","topics":[],"confidence":0.9,"relations":[
                {"from":"you","to":"solstone","kind":"uses","note":"","quote":"I invented a solstone quantum phone."},
                supported,
                {"from":"you","to":"solstone","kind":"uses","note":"","quote":"I fly to the solstone moon."}
            ]}).to_string();
            let checked = verify(&prepared, &output).unwrap();
            assert!(!checked.bindings.is_empty());
            assert_eq!(
                checked
                    .gaps
                    .iter()
                    .map(|g| g["field"].as_str().unwrap())
                    .collect::<Vec<_>>(),
                ["relations.0.quote", "relations.2.quote"]
            );
            let plan = crate::story::commit(
                crate::story::parse(
                    &checked.text,
                    &prepared,
                    &crate::contract::PrePostState::None,
                )
                .unwrap(),
                &prepared,
                &crate::contract::PrePostState::None,
            )
            .unwrap();
            let crate::contract::CommitPlan::Write(crate::writers::WriteIntent::Story {
                value,
                ..
            }) = &plan
            else {
                panic!("expected Story plan")
            };
            if is_owner {
                assert_eq!(
                    value["_product_owner_quotes"],
                    json!(["I use solstone journal every morning."])
                );
            } else {
                assert!(value.get("_product_owner_quotes").is_none());
            }
            let mut prepared = prepared;
            let mut events = Vec::new();
            let outcome = crate::generate_and_write(
                &mut prepared,
                &crate::ExecutionContext {
                    journal: root.path().into(),
                },
                &|_: &GenerateRequest| Ok(generated(&output)),
                &mut events,
                Some((&crate::contract::STORY, crate::contract::PrePostState::None)),
                &solstone_core_generate::Cancellation::never(),
            );
            assert!(
                matches!(outcome, crate::RuntimeOutcome::Finished { .. }),
                "{outcome:?}"
            );
            assert_eq!(
                gap_fields(&String::from_utf8(events).unwrap()),
                ["relations.0.quote", "relations.2.quote"]
            );
            let record = solstone_core_facets::get_activity_record(
                root.path(),
                "work",
                "20260101",
                "meeting-1",
            )
            .unwrap()
            .unwrap();
            assert_eq!(record["relations"].as_array().unwrap().len(), 1);
            assert_eq!(record["story"]["body"], "A discussion of the journal.");
            let relation = &record["relations"][0];
            if is_owner {
                assert_eq!(relation["from"], "you");
                assert_eq!(relation["from_entity_id"], "jordan");
                assert_eq!(relation["owner_evidence"], "voice");
            } else {
                assert_eq!(relation["from"], "unknown");
                assert!(relation["from_entity_id"].is_null());
                assert!(relation.get("owner_evidence").is_none());
            }
            assert_eq!(relation["quote"], "I use solstone journal every morning.");
            assert_eq!(fs::read(source).unwrap(), raw);
            assert!(record["story"].get("_product_owner_quotes").is_none());
        }
    }
    fn generated(text: &str) -> GenerateResponse {
        GenerateResponse::Generated(Box::new(GeneratedResponse {
            id: None,
            text: text.to_owned(),
            model: "test-model".into(),
            usage: json!({}),
            finish_reason: "stop".into(),
            thinking: None,
            schema_validation: None,
            input_budget: None,
            request_budget: None,
            inference: None,
            hints_applied: Vec::new(),
        }))
    }

    fn gap_fields(events: &str) -> Vec<String> {
        events
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .filter(|event| event["event"] == "product_quote_gap")
            .map(|event| event["field"].as_str().unwrap().to_owned())
            .collect()
    }

    #[test]
    fn screen_and_participation_requests_change_only_bound_speech() {
        let root = tempfile::tempdir().unwrap();
        let path = speech(
            root.path(),
            &[json!({
                "start":"09:00:01", "text":"I use Soulstone journal, made by Soul PBC."
            })],
        );
        let raw = fs::read(&path).unwrap();
        let loaded = load(root.path(), None);
        let original = loaded.text.clone() + "\n### Screen Activity\nSoulstone.cpp /work/Soulstone";
        for name in ["screen", "participation"] {
            let mut c = config(&original);
            prepare(name, &mut c, &loaded.sources);
            let prepared = PreparedTalent {
                name: name.into(),
                config: c,
            };
            let parts = crate::generate_contents(&prepared);
            let ContentPart::Text { text } = &parts[0] else {
                panic!("text input")
            };
            assert!(text.contains("I use solstone journal, made by sol pbc."));
            assert!(text.contains("Soulstone.cpp /work/Soulstone"));
            assert_eq!(prepared.config["transcript"], original);
            assert!(event(&prepared.config).is_some());
            assert_eq!(fs::read(&path).unwrap(), raw);
        }
    }

    #[test]
    fn unsupported_prose_quote_still_refuses_and_preserves_existing_output() {
        let root = tempfile::tempdir().unwrap();
        let source = speech(
            root.path(),
            &[
                json!({"start":"09:00:01","speaker":1,"text":"I use Soulstone journal every morning."}),
            ],
        );
        let raw = fs::read(&source).unwrap();
        let loaded = load(root.path(), None);
        let mut c = config(&loaded.text);
        c.insert("output_path".into(), json!("result.json"));
        prepare("conversation", &mut c, &loaded.sources);
        let destination = root.path().join("result.json");
        fs::write(&destination, "existing output").unwrap();
        let mut prepared = PreparedTalent {
            name: "conversation".into(),
            config: c,
        };
        let output = json!({"body":"They said \"I invented a solstone quantum phone.\"", "relations":[],"topics":[],"confidence":0.9}).to_string();
        let mut events = Vec::new();
        let result = crate::generate_and_write(
            &mut prepared,
            &crate::ExecutionContext {
                journal: root.path().into(),
            },
            &|_: &GenerateRequest| Ok(generated(&output)),
            &mut events,
            None,
            &solstone_core_generate::Cancellation::never(),
        );
        assert!(
            matches!(result, crate::RuntimeOutcome::StageFailed(ref error) if error.stage == "product_quote"),
            "{result:?}"
        );
        assert_eq!(fs::read_to_string(destination).unwrap(), "existing output");
        assert_eq!(fs::read(source).unwrap(), raw);
        assert!(String::from_utf8(events).unwrap().contains("body.quote.0"));
    }
}
