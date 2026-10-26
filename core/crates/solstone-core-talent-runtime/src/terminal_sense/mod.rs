// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Complete terminal state input and source-bound Sense activity descriptions.
mod cursor;
mod diff_matcher;
mod execution;
#[cfg(test)]
mod execution_tests;
mod instructions;
mod passages;
#[cfg(all(test, feature = "full-tests"))]
mod preparation_tests;
mod progressive_frames;
mod python_json;
mod raw_changes;
mod readable_changes;
mod readable_terminal;
mod state_codec;
pub(crate) use execution::execute;

use crate::{ExecutionContext, PreparedTalent};
use cursor::Bound;
use passages::Passage;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use solstone_core_generate::{ContentPart, GenerateRequest};

#[derive(Clone, Debug)]
pub(crate) struct Candidate {
    pub request: GenerateRequest,
    original: GenerateRequest,
    /// The owner's input as an agent UI shows it. Every terminal segment reads
    /// the timeline; only an anchored one also binds its account to that input.
    anchor: Option<Anchor>,
}

#[derive(Clone, Debug)]
struct Anchor {
    bound: Bound,
    passages: Vec<Passage>,
}

fn digest(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

/// Only preparation's typed native source records can create this binding.
pub(crate) fn prepare(prepared: &PreparedTalent, context: &ExecutionContext) -> Option<Candidate> {
    if prepared.name != "sense"
        || prepared.stock_task.is_none()
        || prepared.config.contains_key("messages")
        || prepared.config.contains_key("hook")
        || prepared.config.contains_key("span")
        || prepared.config.contains_key("activity")
    {
        return None;
    }
    let counts = prepared.config.get("source_counts")?;
    if counts["transcripts"] != 0
        || counts["talents"] != 0
        || counts["percepts"].as_u64()? == 0
        || prepared.config.get("_memory_sources")? != &json!([])
    {
        return None;
    }
    let sources = prepared
        .entry_sources
        .iter()
        .map(Option::as_ref)
        .collect::<Option<Vec<_>>>()?;
    if sources.len() != counts["percepts"].as_u64()? as usize {
        return None;
    }
    let original = crate::native_generate_request(prepared).ok()?;
    if original.system_instruction.is_some() || original.contents.len() != 2 {
        return None;
    }
    let ContentPart::Text {
        text: original_text,
    } = &original.contents[0]
    else {
        return None;
    };
    let mut text = original_text.clone();
    let source_shift = if original_text.starts_with(crate::prepare::SPARSE_INPUT_NOTE) {
        crate::prepare::SPARSE_INPUT_NOTE.len()
    } else {
        0
    };
    let mut all_frames = Vec::new();
    let mut guides: Vec<String> = Vec::new();
    let base_ms = base_ms(prepared)?;
    let day = prepared.config.get("day")?.as_str()?;
    let segment = prepared.config.get("segment")?.as_str()?;
    for source in sources {
        let mut expected = context.journal.join("chronicle").join(day);
        if let Some(stream) = &source.stream {
            expected = expected.join(stream);
        }
        expected = expected.join(segment).join(&source.file);
        if source.path != expected
            || source.path.file_name()?.to_str()? != source.file
            || source.unparsed_lines != 0
        {
            return None;
        }
        if let Some(stream) = prepared.config.get("stream").and_then(Value::as_str)
            && source.stream.as_deref() != Some(stream)
        {
            return None;
        }
        if source.file.ends_with("_desktop_activity.jsonl")
            && !source.records.is_empty()
            && source.records.iter().all(|record| {
                record.get("v").and_then(Value::as_u64) == Some(1)
                    && record.get("t").and_then(Value::as_str) == Some("desktop_activity")
                    && record.get("method").and_then(Value::as_str) == Some("tmux")
                    && matches!(
                        record.get("kind").and_then(Value::as_str),
                        Some("terminal" | "availability")
                    )
            })
        {
            let start = source.rendered_range.start.checked_add(source_shift)?;
            let end = source.rendered_range.end.checked_add(source_shift)?;
            let activity = source
                .chunks
                .iter()
                .map(|(chunk, _)| chunk.as_str())
                .collect::<Vec<_>>()
                .join("\n\n");
            if original_text.get(start..end)? != activity.trim() {
                return None;
            }
            // Keep existing executable/context text verbatim; only screen sources
            // are replaced by the timeline. No unknown source is skipped.
            continue;
        }
        let mut frames: Vec<Value> = source
            .records
            .iter()
            .filter(|r| r.contains_key("timestamp"))
            .cloned()
            .map(Value::Object)
            .collect();
        if frames.is_empty()
            || frames.len() != source.chunks.len()
            || frames.iter().any(|f| !valid_frame(f))
        {
            return None;
        }
        // Chunk authority is from the very bytes the native loader rendered.
        if frames
            .iter()
            .zip(&source.chunks)
            .any(|(f, (_, r))| f.as_object() != Some(r))
        {
            return None;
        }
        frames.sort_by(|a, b| {
            a["timestamp"]
                .as_f64()
                .unwrap()
                .total_cmp(&b["timestamp"].as_f64().unwrap())
        });
        let old = source
            .chunks
            .iter()
            .map(|(s, _)| s.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        text.find(&old)?;
        if old.is_empty() || text.matches(&old).count() != 1 {
            return None;
        }
        let (visible, records) = raw_changes::prepare(&frames, true).ok()?;
        let encoded = raw_changes::encode(&records, base_ms);
        if raw_changes::restore(&raw_changes::decode(&encoded, base_ms).ok()?).ok()? != visible {
            return None;
        }
        let (guide, replacement) = raw_changes::split_guide(&encoded)?;
        if !guides.iter().any(|known| known == guide) {
            guides.push(guide.to_owned());
        }
        // Offsets are checked against the original body, before any replacement.
        let original_at = original_text.find(&old)?;
        if source
            .chunk_offsets
            .first()
            .and_then(|at| at.checked_add(source_shift))
            != Some(original_at)
        {
            return None;
        }
        // The lossless timeline is checked above; repeats are dropped only after.
        let replacement = readable_changes::drop_repeated_lines(replacement);
        text = text.replacen(&old, &replacement, 1);
        all_frames.extend(frames);
    }
    let mut request = original.clone();
    // The timeline stays in the ordinary input position, so fitting trims it like any
    // talent's input. Everything the output binding reads is preserved below.
    request.contents[0] = ContentPart::Text { text };
    let anchor = anchor(&all_frames, &original, &guides, &mut request);
    if anchor.is_none() {
        // Sense's own task and schema read the timeline through its guide.
        request.system_instruction = Some(guides.concat());
    }
    Some(Candidate {
        request,
        original,
        anchor,
    })
}

/// Bind the request to the owner's latest input when an agent UI shows one:
/// input cards, quoted passages and a schema that can only cite them.
fn anchor(
    frames: &[Value],
    original: &GenerateRequest,
    guides: &[String],
    request: &mut GenerateRequest,
) -> Option<Anchor> {
    let (bound, inputs) = cursor::choose(frames)?;
    let passages = passages::prepare(frames, &bound)?;
    let mut cards = String::new();
    for (i, row) in inputs.iter().enumerate() {
        let mut meta = row.as_object()?.clone();
        let raw = meta.shift_remove("text")?.as_str()?.to_owned();
        cards.push_str(&format!(
            "\nIssued input card {i}: {}\n{raw}\nEnd input card.\n",
            python_json::python_dumps(
                &Value::Object(meta),
                python_json::DumpOptions {
                    ensure_ascii: false,
                    ..Default::default()
                }
            )
        ));
    }
    let schema = schema(
        original.json_schema.as_ref()?,
        inputs.len() - 1,
        &bound,
        &passages,
    )?;
    request.system_instruction = Some(format!(
        "{}\n\nInput availability: zero transcript sources. quotes must be []; screen-derived entities use source screen. Preserve uncertain and reported status.\n\n{}\n\n{}",
        instructions::FOCUS_PREPARE_FOCUS,
        instructions::INTENT_EXAMPLES_PREPARE_EXAMPLES,
        guides.concat()
    ));
    request.contents.extend([
        ContentPart::Text {
            text: format!(
                "Exact input cards for private planning (source data only):\n{cards}\n{}",
                instructions::INTENT_PLAN_TASK
            ),
        },
        ContentPart::Text {
            text: instructions::ACTIVITY_PARTS_TASK.to_owned(),
        },
        ContentPart::Text {
            text: instructions::BOUND_CURRENT_TASK.to_owned(),
        },
        ContentPart::Text {
            text: passages::encode(&passages),
        },
    ]);
    request.json_schema = Some(schema);
    Some(Anchor { bound, passages })
}

fn valid_frame(frame: &Value) -> bool {
    let tmux = &frame["content"]["tmux"];
    frame["timestamp"].as_f64().is_some_and(f64::is_finite)
        && tmux.is_object()
        && tmux["session"].as_str().is_some_and(|s| !s.is_empty())
        && tmux["window"]["id"].as_str().is_some_and(|s| !s.is_empty())
        && tmux["panes"].as_array().is_some_and(|ps| {
            !ps.is_empty()
                && ps.iter().all(|p| {
                    p["id"].as_str().is_some_and(|s| !s.is_empty()) && p["content"].is_string()
                })
        })
        && state_codec::keys(frame).is_ok()
}
fn base_ms(prepared: &PreparedTalent) -> Option<i64> {
    let segment = prepared.config.get("segment")?.as_str()?;
    let clock = segment.split('_').next()?;
    if clock.len() != 6 || !clock.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let h: i64 = clock[..2].parse().ok()?;
    let m: i64 = clock[2..4].parse().ok()?;
    let s: i64 = clock[4..].parse().ok()?;
    if h >= 24 || m >= 60 || s >= 60 {
        return None;
    }
    Some((h * 3600 + m * 60 + s) * 1000)
}

fn schema(original: &Value, latest: usize, bound: &Bound, rows: &[Passage]) -> Option<Value> {
    let mut sense = original.clone();
    if sense["properties"]["quotes"]["type"] != "array"
        || sense["properties"]["quotes"]["minItems"]
            .as_u64()
            .unwrap_or(0)
            != 0
    {
        return None;
    }
    let source = &sense["properties"]["entities"]["items"]["properties"]["source"];
    if source["type"] != "string" {
        return None;
    }
    let mut enums = source["enum"]
        .as_array()?
        .iter()
        .map(|x| x.as_str())
        .collect::<Option<Vec<_>>>()?;
    enums.sort_unstable();
    if enums != ["other", "screen", "speaker_label", "transcript", "voice"] {
        return None;
    }
    sense["properties"]["quotes"]["maxItems"] = json!(0);
    sense["properties"]["entities"]["items"]["properties"]["source"]["enum"] = json!(["screen"]);
    if sense["properties"]["activity_summary"]["type"] != "string"
        || sense["properties"]["facets"]["items"]["properties"]["activity"]["type"] != "string"
    {
        return None;
    }
    let parts = json!({"type":"object","additionalProperties":false,"required":["background","current"],"properties":{"background":{"type":["string","null"]},"current":{"type":["string","null"]}}});
    sense["properties"]["activity_summary"] = parts.clone();
    sense["properties"]["facets"]["items"]["properties"]["activity"] = parts;
    sense["properties"]["activity_summary"]["properties"]["current"] =
        json!({"type":"string","const":bound.current});
    sense["properties"]["facets"]["items"]["properties"]["activity"]["properties"]["current"] =
        json!({"type":["string","null"],"enum":[null,bound.current]});
    let item =
        json!({"type":"string","enum":rows.iter().map(|r|r.id.as_str()).collect::<Vec<_>>()});
    sense["properties"]["activity_summary"]["properties"]["background"] =
        json!({"type":"array","minItems":1,"maxItems":2,"items":item});
    sense["properties"]["facets"]["items"]["properties"]["activity"]["properties"]["background"] =
        json!({"type":["array","null"],"minItems":1,"maxItems":2,"items":item});
    let plan = json!({"type":"object","additionalProperties":false,"required":["anchor_input_id","current_act","earlier_work_state","pending_decision","requested_follow_through"],"properties":{"anchor_input_id":{"type":"integer","enum":[latest]},"current_act":{"type":"string","enum":["question","request","approval","report","unclear"]},"earlier_work_state":{"type":"string","enum":["reported_complete_or_validated","proposed_or_unverified","not_established"]},"pending_decision":{"type":"boolean"},"requested_follow_through":{"type":["string","null"]}}});
    Some(
        json!({"type":"object","additionalProperties":false,"required":["model_plan","sense"],"properties":{"model_plan":plan,"sense":sense}}),
    )
}

impl Candidate {
    pub(super) fn anchored(&self) -> bool {
        self.anchor.is_some()
    }
    fn binding_fingerprint(&self) -> Option<String> {
        self.anchor.as_ref().map(Anchor::fingerprint)
    }
    /// The Sense record a response publishes, checked against what it may cite.
    fn render(&self, text: &str) -> Result<String, String> {
        let Some(anchor) = &self.anchor else {
            let sense: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
            jsonschema::validator_for(self.original.json_schema.as_ref().ok_or("no schema")?)
                .map_err(|e| e.to_string())?
                .validate(&sense)
                .map_err(|e| e.to_string())?;
            return Ok(text.to_owned());
        };
        anchor.render(text, &self.request, &self.original)
    }
}

impl Anchor {
    fn fingerprint(&self) -> String {
        digest(&json!({"screen":self.bound.screen_sha,"current":self.bound.current_sha,"passages":self.passages.iter().map(|r|json!([r.id,r.timestamp,r.scope,r.screen_sha,r.raw_sha,r.start,r.end])).collect::<Vec<_>>()} ).to_string())
    }
    fn render(
        &self,
        text: &str,
        request: &GenerateRequest,
        original: &GenerateRequest,
    ) -> Result<String, String> {
        let wrapper: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
        jsonschema::validator_for(request.json_schema.as_ref().unwrap())
            .map_err(|e| e.to_string())?
            .validate(&wrapper)
            .map_err(|e| e.to_string())?;
        let mut sense = wrapper["sense"].clone();
        if sense.get("model_plan").is_some()
            || sense.get("model_notes").is_some()
            || sense["quotes"] != json!([])
            || !sense["entities"]
                .as_array()
                .is_some_and(|es| es.iter().all(|e| e["source"] == "screen"))
        {
            return Err("invalid source evidence".into());
        }
        let summary = self.clause(&sense["activity_summary"])?;
        sense["activity_summary"] = json!(summary);
        for facet in sense["facets"].as_array_mut().ok_or("invalid facets")? {
            facet["activity"] = json!(self.clause(&facet["activity"])?);
        }
        jsonschema::validator_for(original.json_schema.as_ref().unwrap())
            .map_err(|e| e.to_string())?
            .validate(&sense)
            .map_err(|e| e.to_string())?;
        serde_json::to_string(&sense).map_err(|e| e.to_string())
    }
    fn clause(&self, value: &Value) -> Result<String, String> {
        let mut parts = Vec::new();
        if !value["background"].is_null() {
            let keys = value["background"].as_array().ok_or("invalid background")?;
            if !(1..=2).contains(&keys.len()) || (keys.len() == 2 && keys[0] == keys[1]) {
                return Err("invalid context handles".into());
            }
            let mut chosen = keys
                .iter()
                .map(|k| {
                    self.passages
                        .iter()
                        .find(|r| Some(r.id.as_str()) == k.as_str())
                        .ok_or("unknown context handle")
                })
                .collect::<Result<Vec<_>, _>>()?;
            chosen.sort_by(|a, b| passages::compare(a, b));
            parts.push(format!(
                "had this text on screen: {}",
                chosen
                    .iter()
                    .map(|r| format!("\"{}\"", r.text))
                    .collect::<Vec<_>>()
                    .join(" ")
            ));
        }
        if !value["current"].is_null() {
            let current = value["current"].as_str().ok_or("invalid current")?;
            if digest(current) != self.bound.current_sha {
                return Err("altered current input".into());
            }
            let current = if self.bound.kind == "selection_menu_with_highlight_visible" {
                "had menu options on screen.".to_owned()
            } else {
                let input = current
                    .strip_prefix("Composed this input: “")
                    .and_then(|s| s.strip_suffix('”'))
                    .ok_or("unknown input framing")?;
                format!("had this input on screen: \"{input}\"")
            };
            parts.push(current);
        }
        if parts.is_empty() {
            return Err("empty activity".into());
        }
        Ok(parts.iter().map(|p| p.trim()).collect::<Vec<_>>().join(" "))
    }
}

#[cfg(any(feature = "test-hooks", all(test, feature = "full-tests")))]
#[doc(hidden)]
pub fn inspect(prepared: &PreparedTalent, text: Option<&str>) -> Result<Value, String> {
    let request = crate::generate_request(prepared)?;
    let encoded = solstone_core_generate::encode_one_shot_request(&request)?;
    let mut result = json!({"request": serde_json::from_str::<Value>(&encoded).map_err(|e| e.to_string())?, "candidate": prepared.terminal_sense.is_some()});
    let native = crate::native_generate_request(prepared)?;
    let native_text = match native.contents.first() {
        Some(ContentPart::Text { text }) => text.as_str(),
        _ => "",
    };
    let mut frames = Vec::new();
    let sources=prepared.entry_sources.iter().map(|source|match source {
        None=>json!({"missing":true}),
        Some(source)=>{
            let observed=source.records.iter().filter(|r|r.contains_key("timestamp")).cloned().map(Value::Object).collect::<Vec<_>>();
            let old=source.chunks.iter().map(|(s,_)|s.as_str()).collect::<Vec<_>>().join("\n");
            let value=json!({"frames":observed.len(),"chunks":source.chunks.len(),"frames_equal_chunks":observed.iter().zip(&source.chunks).all(|(f,(_,r))| f.as_object()==Some(r)),"old_matches":native_text.matches(&old).count(),"actual_offset":native_text.find(&old),"expected_offset":source.chunk_offsets.first(),"valid_frames":observed.iter().all(valid_frame)});
            frames.extend(observed);value
        }
    }).collect::<Vec<_>>();
    let bound = cursor::choose(&frames).map(|(bound, _)| bound);
    result["diagnostic"] = json!({"stock":prepared.stock_task.is_some(),"counts":prepared.config.get("source_counts"),"sources":sources,"bound":bound.as_ref().map(|b|b.kind),"passages":bound.as_ref().and_then(|b|passages::prepare(&frames,b)).map(|p|p.len())});
    if let (Some(candidate), Some(text)) = (&prepared.terminal_sense, text) {
        result["publication"] =
            serde_json::from_str(&candidate.render(text)?).map_err(|e| e.to_string())?;
    }
    Ok(result)
}

#[cfg(feature = "test-hooks")]
#[doc(hidden)]
pub fn inspect_codec(frames: &[Value], base_ms: i64) -> Result<Value, String> {
    let (visible, records) = raw_changes::prepare(frames, true).map_err(str::to_owned)?;
    let encoded = raw_changes::encode(&records, base_ms);
    let restored =
        raw_changes::restore(&raw_changes::decode(&encoded, base_ms).map_err(str::to_owned)?)
            .map_err(str::to_owned)?;
    Ok(json!({"visible":visible,"restored":restored,"encoded":encoded}))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn frame(time: f64, text: &str) -> Value {
        json!({"timestamp":time,"analysis":{"primary":"tmux","visual_description":"test"},"content":{"tmux":{"session":"session","window":{"id":"window","name":"work"},"windows":[{"id":"window","active":true}],"panes":[{"id":"pane","active":true,"content":text}]}}})
    }
    #[test]
    fn capture_metadata_is_inert_for_old_new_and_mixed_timeline_frames() {
        let legacy = vec![
            frame(0.0, "prompt> first"),
            frame(5.0, "prompt> first more"),
        ];
        let (old_visible, old_records) = raw_changes::prepare(&legacy, true).unwrap();
        assert!(
            !old_records[0]["prefix_history"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        let expected = raw_changes::encode(&old_records, 36000000);
        let mut extended = legacy.clone();
        for (index, f) in extended.iter_mut().enumerate() {
            let p = &mut f["content"]["tmux"]["panes"][0];
            p["cursor_x"] = json!(index + 2);
            p["cursor_y"] = json!(index);
            p["cursor_visible"] = json!(index == 0);
            p["alternate_on"] = json!(index != 0);
            p["pane_in_mode"] = json!(index != 0);
        }
        for frames in [&extended, &vec![legacy[0].clone(), extended[1].clone()]] {
            let (visible, records) = raw_changes::prepare(frames, true).unwrap();
            assert_eq!(visible, old_visible);
            assert_eq!(raw_changes::encode(&records, 36000000), expected);
            assert_eq!(raw_changes::restore(&records).unwrap(), old_visible);
        }
        extended[1]["content"]["tmux"]["panes"][0]["active"] = json!(false);
        let (_, meaningful) = raw_changes::prepare(&extended, true).unwrap();
        assert_ne!(raw_changes::encode(&meaningful, 36000000), expected);
        assert_eq!(meaningful.len(), 2);
    }

    #[test]
    fn timeline_restores_recurrence_prefixes_styles_links_and_roster_changes() {
        let a = "hello\r\n❯ question\u{2028}line\n";
        let b = "\x1b[48;5;7m\x1b[7;9mchanged\x1b[0m \x1b]8;;https://example.test/path\x1b\\target\x1b]8;;\x1b\\\n";
        let mut frames = vec![
            frame(0.0, a),
            frame(1.0, b),
            frame(2.0, a),
            frame(3.0, &format!("{a}more")),
            frame(63.0, "final value 🧭\n"),
        ];
        frames[3]["content"]["tmux"]["windows"] =
            json!([{"id":"window","active":true},{"id":"other","active":false}]);
        let (visible, records) = raw_changes::prepare(&frames, true).unwrap();
        let encoded = raw_changes::encode(&records, 36000000);
        assert_eq!(
            raw_changes::restore(&raw_changes::decode(&encoded, 36000000).unwrap()).unwrap(),
            visible
        );
        assert_eq!(visible[2]["content"]["tmux"]["panes"][0]["content"], a);
        assert_eq!(
            visible.last().unwrap()["content"]["tmux"]["panes"][0]["content"],
            "final value 🧭\n"
        );
        let empty = vec![frame(0.0, "")];
        let (visible, records) = raw_changes::prepare(&empty, true).unwrap();
        assert_eq!(
            raw_changes::restore(
                &raw_changes::decode(&raw_changes::encode(&records, 0), 0).unwrap()
            )
            .unwrap(),
            visible
        );
    }
}
