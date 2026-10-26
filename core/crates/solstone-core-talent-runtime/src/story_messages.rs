// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Optional literal screen-message details. No model calls or journal reads.
//! The source labels remain fallible extraction, never actor or event authority.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::LazyLock;

use chrono::{NaiveDate, NaiveTime};
use serde::Serialize;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use solstone_core_transcripts::EntrySource;
use unicode_general_category::{GeneralCategory, get_general_category};

const MAX_VALUES: usize = 5;
const MAX_BODY_CHARS: usize = 4_000;
const HEADING: &str = "screen notes (unverified)";
const RECORDED: &str = "recorded:";
const NO_MESSAGE_DATE: &str = "message date unavailable";
const ROOTS: &[&str] = &[
    "request", "review", "accept", "approv", "confirm", "cancel", "pend", "declin", "await",
];
const EXACT_CUES: &[&str] = &["if", "unless", "until", "only", "not"];
const STOP_WORDS: &str = "a an the and or but of to in on at by as from with for into over under about this that these those it its is are was were be been being do does did have has had having your you yours we our ours i me my mine they their them he him his she her hers us not if then than so can could may might will would should must only also all any some more most other same such no yes email mail gmail message messages inbox notification notifications update updates updated updating review reviews reviewed reviewing activity activities";
static STOPS: LazyLock<BTreeSet<String>> = LazyLock::new(|| words(STOP_WORDS));

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
struct Message {
    sender: Option<String>,
    timestamp: Option<String>,
    subject: Option<String>,
    text: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
struct Parent {
    app: Option<String>,
    thread: Option<String>,
    view: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Key {
    message: Message,
    parent: Parent,
    day: String,
    monitor: String,
}

#[derive(Clone, Debug, Serialize)]
struct Alias {
    ordinal: usize,
    source_index: usize,
    source_path: String,
    source_sha256: String,
    record_index: usize,
    chunk_index: usize,
    message_index: usize,
    rendered_chunk_start: usize,
    rendered_chunk_end: usize,
    frame_value_sha256: String,
    observation_clock: String,
}

#[derive(Clone, Debug)]
struct Group {
    key: Key,
    aliases: Vec<Alias>,
    terms: BTreeSet<String>,
    cue_count: usize,
}

#[derive(Clone, Debug)]
pub(crate) struct Candidate {
    groups: Vec<Group>,
    occurrences: usize,
}

pub(crate) struct Supplement {
    pub(crate) body: String,
    pub(crate) provenance: Value,
}

impl Candidate {
    /// Build only from the exact typed snapshot already loaded for this Work
    /// activity. An unavailable source could hide a competing message subject,
    /// so incomplete snapshots abstain rather than performing another read.
    pub(crate) fn prepare(
        sources: &[Option<EntrySource>],
        config: &Map<String, Value>,
    ) -> Option<Self> {
        let hook = config.get("hook").and_then(Value::as_object);
        if !hook.is_some_and(|hook| {
            hook.len() == 1 && hook.get("post").and_then(Value::as_str) == Some("story")
        }) || config
            .get("name")
            .is_some_and(|name| name.as_str() != Some("work"))
        {
            log::debug!("story message details omitted: not the Work Story hook");
            return None;
        }
        if config.contains_key("messages") {
            log::debug!("story message details omitted: custom message input");
            return None;
        }
        match Self::collect(sources) {
            Ok(candidate) => candidate,
            Err(reason) => {
                log::debug!("story message details omitted: {reason}");
                None
            }
        }
    }

    fn collect(sources: &[Option<EntrySource>]) -> Result<Option<Self>, &'static str> {
        let mut groups: Vec<Group> = Vec::new();
        let mut by_key: BTreeMap<Key, usize> = BTreeMap::new();
        let mut ordinal = 0;
        for (source_index, source) in sources.iter().enumerate() {
            let source = source
                .as_ref()
                .ok_or("original source metadata unavailable")?;
            if !source.file.ends_with("screen.jsonl") {
                continue;
            }
            if source.unparsed_lines != 0 || source.chunks.len() != source.chunk_offsets.len() {
                return Err("original screen source metadata incomplete");
            }
            let day = source_day(source).ok_or("screen observation day unavailable")?;
            let path = source.path.to_str().ok_or("source path is not UTF-8")?;
            let monitor = monitor_key(&source.file);
            let mut used_records = BTreeSet::new();
            for (chunk_index, ((chunk, frame), offset)) in
                source.chunks.iter().zip(&source.chunk_offsets).enumerate()
            {
                if *offset == usize::MAX {
                    return Err("original screen chunk was trimmed");
                }
                let record_index = source
                    .records
                    .iter()
                    .enumerate()
                    .position(|(index, record)| !used_records.contains(&index) && record == frame)
                    .ok_or("screen chunk has no original record")?;
                used_records.insert(record_index);
                let Some(payload) = messaging(frame) else {
                    continue;
                };
                if payload.is_null() {
                    continue;
                }
                let payload = payload.as_object().ok_or("invalid messaging object")?;
                if payload.len() != 4 {
                    return Err("unsupported messaging fields");
                }
                let parent = Parent {
                    app: field(payload, "app")?,
                    thread: field(payload, "thread")?,
                    view: field(payload, "view")?,
                };
                let messages = payload
                    .get("messages")
                    .and_then(Value::as_array)
                    .ok_or("invalid message list")?;
                let clock = chunk
                    .lines()
                    .next()
                    .and_then(|line| line.strip_prefix("### "))
                    .filter(|clock| NaiveTime::parse_from_str(clock, "%H:%M:%S%.f").is_ok())
                    .ok_or("screen observation clock unavailable")?;
                let chunk_end = offset
                    .checked_add(chunk.len())
                    .ok_or("source range overflow")?;
                let frame_sha = value_hash(&Value::Object(frame.clone()));
                for (message_index, message) in messages.iter().enumerate() {
                    let message = message.as_object().ok_or("invalid message record")?;
                    if message.len() != 4 {
                        return Err("unsupported message fields");
                    }
                    let message = Message {
                        sender: field(message, "sender")?,
                        timestamp: field(message, "timestamp")?,
                        subject: field(message, "subject")?,
                        text: field(message, "text")?,
                    };
                    let content = [
                        message.sender.as_deref(),
                        message.subject.as_deref(),
                        message.text.as_deref(),
                    ]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join(" ");
                    let key = Key {
                        message,
                        parent: parent.clone(),
                        day: day.clone(),
                        monitor: monitor.clone(),
                    };
                    let alias = Alias {
                        ordinal,
                        source_index,
                        source_path: path.to_owned(),
                        source_sha256: source.sha256.clone(),
                        record_index,
                        chunk_index,
                        message_index,
                        rendered_chunk_start: *offset,
                        rendered_chunk_end: chunk_end,
                        frame_value_sha256: frame_sha.clone(),
                        observation_clock: clock.to_owned(),
                    };
                    ordinal += 1;
                    if let Some(index) = by_key.get(&key).copied() {
                        groups[index].aliases.push(alias);
                    } else {
                        by_key.insert(key.clone(), groups.len());
                        groups.push(Group {
                            key,
                            aliases: vec![alias],
                            terms: terms(&content),
                            cue_count: cue_count(&content),
                        });
                    }
                }
            }
            if source.records.iter().enumerate().any(|(index, record)| {
                messaging(record).is_some_and(|value| !value.is_null())
                    && !used_records.contains(&index)
            }) {
                return Err("unrendered original messaging frame");
            }
        }
        Ok((!groups.is_empty()).then_some(Self {
            groups,
            occurrences: ordinal,
        }))
    }

    /// V2 winner ordering, then the V3 overlap floor. Ambiguity is checked
    /// against every original same-parent value, not just the selected subset.
    pub(crate) fn supplement(&self, body: &str, topics: &[String]) -> Option<Supplement> {
        let mut selected: Vec<(usize, Vec<usize>)> = Vec::new();
        for (topic_index, topic) in topics.iter().enumerate() {
            let query = terms(topic);
            let mut best: Option<(usize, usize, usize)> = None;
            for (index, group) in self.groups.iter().enumerate() {
                let overlap = query.intersection(&group.terms).count();
                if overlap == 0 || group.cue_count == 0 {
                    continue;
                }
                if best.is_none_or(|(_, cues, count)| (group.cue_count, overlap) > (cues, count)) {
                    best = Some((index, group.cue_count, overlap));
                }
            }
            let Some((index, _, overlap)) = best else {
                continue;
            };
            // Do not re-rank or choose a runner-up when the fixed winner fails.
            if overlap < 2 {
                continue;
            }
            if let Some((_, indices)) = selected.iter_mut().find(|(chosen, _)| *chosen == index) {
                indices.push(topic_index);
            } else if selected.len() < MAX_VALUES {
                selected.push((index, vec![topic_index]));
            }
        }
        let mut included = Vec::new();
        let mut withheld = Vec::new();
        let mut footer = String::new();
        for (index, topic_indices) in selected {
            let group = &self.groups[index];
            let witnesses = self
                .groups
                .iter()
                .filter(|other| ambiguous(group, other))
                .collect::<Vec<_>>();
            if !witnesses.is_empty() {
                withheld.push(json!({
                    "message_value_sha256":value_hash(&json!(group.key.message)),
                    "parent_value_sha256":value_hash(&json!(group.key.parent)),
                    "aliases":group.aliases,"topic_indices":topic_indices,
                    "reason":"subject_prefix_ambiguity",
                    "witnesses":witnesses.iter().map(|w| json!({"subject_sha256":value_hash(&json!(w.key.message.subject)),"first_alias":w.aliases.first(),"occurrences":w.aliases.len()})).collect::<Vec<_>>()
                }));
                continue;
            }
            if !group
                .key
                .message
                .text
                .as_deref()
                .is_some_and(|text| !text.trim().is_empty())
            {
                continue;
            }
            if !footer.is_empty() {
                footer.push_str("\n\n");
            }
            footer.push_str(&render(group));
            included.push(
                json!({"message":group.key.message,"parent":group.key.parent,"day":group.key.day,
                "monitor":group.key.monitor,"aliases":group.aliases,"topic_indices":topic_indices}),
            );
        }
        if included.is_empty() {
            log::debug!("story message details omitted: no usable unambiguous nomination");
            return None;
        }
        let enriched = format!("{body}\n\n{HEADING}\n\n{footer}");
        if enriched.chars().count() > MAX_BODY_CHARS {
            log::debug!("story message details omitted: complete footer exceeds body limit");
            return None;
        }
        Some(Supplement {
            body: enriched,
            provenance: json!({
                "version":1,"authority":"unverified_screen_extraction","source_complete":false,
                "selection":"cue_first_then_overlap_v3_floor","original_occurrences":self.occurrences,
                "included":included,"withheld":withheld,
                "observation_time_is_not_message_time":true
            }),
        })
    }
}

fn messaging(frame: &Map<String, Value>) -> Option<&Value> {
    frame.get("content")?.as_object()?.get("messaging")
}

fn field(object: &Map<String, Value>, name: &str) -> Result<Option<String>, &'static str> {
    match object.get(name) {
        Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        _ => Err("missing or invalid literal message field"),
    }
}

fn source_day(source: &EntrySource) -> Option<String> {
    let parts = source
        .path
        .iter()
        .map(|part| part.to_str())
        .collect::<Option<Vec<_>>>()?;
    parts.windows(2).rev().find_map(|pair| {
        (pair[0] == "chronicle" && solstone_core_format::segment::is_date_key(pair[1]))
            .then(|| NaiveDate::parse_from_str(pair[1], "%Y%m%d").ok())
            .flatten()
            .map(|day| day.format("%Y-%m-%d").to_string())
    })
}

// Same closed screen-name projection as the native raw-screen header. It is
// used only for the frozen value/alias key; it is not a monitor identity claim.
fn monitor_key(file: &str) -> String {
    let parsed = file
        .strip_suffix("_screen.jsonl")
        .and_then(|stem| stem.rsplit_once('_'));
    if let Some((position, connector)) = parsed
        && !position.is_empty()
        && position
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b == b'-')
        && !connector.is_empty()
        && connector
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
    {
        return format!("# Frame Analyses ({position} - {connector})");
    }
    "# Frame Analyses".to_owned()
}

fn words(text: &str) -> BTreeSet<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_ascii_lowercase())
        .filter(|word| !word.is_empty())
        .map(|word| {
            if word.len() > 4 && word.ends_with('s') && !word.ends_with("ss") {
                word[..word.len() - 1].to_owned()
            } else {
                word.to_owned()
            }
        })
        .collect()
}
fn terms(text: &str) -> BTreeSet<String> {
    words(text).difference(&STOPS).cloned().collect()
}
fn cue_count(text: &str) -> usize {
    let tokens = words(text);
    ROOTS
        .iter()
        .filter(|root| tokens.iter().any(|token| token.starts_with(**root)))
        .count()
        + EXACT_CUES
            .iter()
            .filter(|word| tokens.contains(**word))
            .count()
}
fn alphanumeric(c: char) -> bool {
    matches!(
        get_general_category(c),
        GeneralCategory::UppercaseLetter
            | GeneralCategory::LowercaseLetter
            | GeneralCategory::TitlecaseLetter
            | GeneralCategory::ModifierLetter
            | GeneralCategory::OtherLetter
            | GeneralCategory::DecimalNumber
            | GeneralCategory::LetterNumber
            | GeneralCategory::OtherNumber
    )
}
fn ambiguous(target: &Group, witness: &Group) -> bool {
    if target.key.parent != witness.key.parent
        || target.key.message.subject == witness.key.message.subject
    {
        return false;
    }
    let Some(subject) = witness
        .key
        .message
        .subject
        .as_deref()
        .filter(|s| !s.is_empty() && !s.contains("...") && !s.contains('…'))
    else {
        return false;
    };
    let Some(text) = target.key.message.text.as_deref() else {
        return false;
    };
    text.strip_prefix(subject)
        .is_some_and(|rest| rest.chars().next().is_none_or(|c| !alphanumeric(c)))
}
fn value_hash(value: &Value) -> String {
    format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(value).expect("JSON value serializes"))
    )
}
fn visible(text: &str) -> String {
    let mut output = String::new();
    for c in text.chars() {
        if c != '\n'
            && matches!(
                get_general_category(c),
                GeneralCategory::Control
                    | GeneralCategory::Format
                    | GeneralCategory::LineSeparator
                    | GeneralCategory::ParagraphSeparator
            )
        {
            output.push_str(&format!("\\u{{{:04X}}}", u32::from(c)));
        } else {
            output.push(c);
        }
    }
    output
}
fn caption(text: &str) -> String {
    let mut output = String::new();
    for c in visible(text).chars() {
        if c == '\n' {
            output.push_str("  \n");
        } else if c.is_ascii() && !c.is_ascii_alphanumeric() {
            output.push_str(&format!("&#{};", u32::from(c)));
        } else {
            output.push(c);
        }
    }
    output
}
fn quotation(text: &str) -> String {
    let shown = visible(text);
    let mut longest = 0;
    let mut current = 0;
    for c in shown.chars() {
        if c == '`' {
            current += 1;
            longest = longest.max(current);
        } else {
            current = 0;
        }
    }
    let fence = "`".repeat(3.max(longest + 1));
    format!(
        "> {fence}\n{}\n> {fence}",
        shown
            .split('\n')
            .map(|line| format!("> {line}"))
            .collect::<Vec<_>>()
            .join("\n")
    )
}
fn render(group: &Group) -> String {
    let message = &group.key.message;
    let mut out = format!(
        "{RECORDED} {}",
        caption(&format!(
            "{} {}",
            group.key.day, group.aliases[0].observation_clock
        ))
    );
    if let Some(app) = &group.key.parent.app {
        out.push_str(&format!(" · {}", caption(app)));
    }
    let mut captions = Vec::new();
    if let Some(sender) = &message.sender {
        captions.push(caption(sender));
    }
    match &message.timestamp {
        Some(time) => captions.push(caption(time)),
        None => captions.push(NO_MESSAGE_DATE.to_owned()),
    }
    if !captions.is_empty() {
        out.push_str(&format!("\n\n{}", captions.join(" · ")));
    }
    for value in [&message.subject, &message.text].into_iter().flatten() {
        out.push_str(&format!("\n\n{}", quotation(value)));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(subject: Value, text: Value) -> Value {
        json!({"sender":"Sender","timestamp":null,"subject":subject,"text":text})
    }
    fn source(frames: Vec<Value>) -> Option<EntrySource> {
        let records = frames
            .into_iter()
            .map(|value| value.as_object().unwrap().clone())
            .collect::<Vec<_>>();
        let chunks = records
            .iter()
            .enumerate()
            .map(|(index, row)| (format!("### 09:00:{index:02}\n\nframe"), row.clone()))
            .collect::<Vec<_>>();
        Some(EntrySource {
            path: "/journal/chronicle/20400102/device_1/090000_300/left_DP-1_screen.jsonl".into(),
            sha256: "invented-source-hash".into(),
            rendered_range: 0..1000,
            stream: Some("device_1".into()),
            file: "left_DP-1_screen.jsonl".into(),
            records,
            unparsed_lines: 0,
            chunk_offsets: (0..chunks.len()).map(|index| index * 100).collect(),
            chunks,
            voice_named: BTreeSet::new(),
        })
    }
    fn frame(messages: Vec<Value>) -> Value {
        json!({"timestamp":0,"content":{"messaging":{"app":"Mail","thread":"Inbox","view":"inbox","messages":messages}}})
    }
    fn candidate(messages: Vec<Value>) -> Candidate {
        Candidate::prepare(&[source(vec![frame(messages)])], &config()).unwrap()
    }
    fn config() -> Map<String, Value> {
        json!({"hook":{"post":"story"}})
            .as_object()
            .unwrap()
            .clone()
    }
    fn topics(values: &[&str]) -> Vec<String> {
        values.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn mixed_ambiguity_recomposes_only_whole_valid_records() {
        let c = candidate(vec![
            message(
                json!("Alpha island request"),
                json!("Other title: accept alpha now"),
            ),
            message(json!("Other title"), json!("unrelated")),
            message(
                json!("Beta ocean request"),
                json!("Beta ocean is ready only if approved."),
            ),
        ]);
        let output = c
            .supplement("Narrative.", &topics(&["alpha island", "beta ocean"]))
            .unwrap();
        assert!(output.body.starts_with("Narrative."));
        assert!(!output.body.contains("accept alpha now"));
        assert!(
            output
                .body
                .contains("Beta ocean is ready only if approved.")
        );
        assert_eq!(output.provenance["included"].as_array().unwrap().len(), 1);
        assert_eq!(output.provenance["withheld"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn aliases_are_preserved_without_repeating_the_quote() {
        let m = message(json!("Gamma request"), json!("Gamma is ready for review."));
        let c = Candidate::prepare(
            &[source(vec![frame(vec![m.clone()]), frame(vec![m.clone()])])],
            &config(),
        )
        .unwrap();
        let out = c
            .supplement("Base", &topics(&["gamma request", "gamma review"]))
            .unwrap();
        assert_eq!(out.provenance["included"].as_array().unwrap().len(), 1);
        assert_eq!(
            out.provenance["included"][0]["aliases"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert_eq!(out.body.matches("Gamma is ready for review.").count(), 1);
    }

    #[test]
    fn null_sender_does_not_invent_an_actor_and_empty_text_abstains() {
        let mut m = message(json!("Delta request"), json!("Delta ready for review."));
        m["sender"] = Value::Null;
        let out = candidate(vec![m.clone()])
            .supplement("Base", &topics(&["delta request"]))
            .unwrap();
        assert!(out.provenance["included"][0]["message"]["sender"].is_null());
        assert!(out.body.contains("Delta ready for review."));
        for text in [Value::Null, json!(""), json!(" \n ")] {
            let m = message(json!("Delta request"), text);
            assert!(
                candidate(vec![m])
                    .supplement("Base", &topics(&["delta request"]))
                    .is_none()
            );
        }
        assert!(
            candidate(vec![
                json!({"sender":null,"timestamp":null,"subject":null,"text":null})
            ])
            .supplement("Base", &topics(&["delta request"]))
            .is_none()
        );
    }

    #[test]
    fn dangerous_markup_controls_and_fences_stay_data() {
        let mut m = message(
            json!("Echo request"),
            json!("Echo ready for review.\n`````\n<script>x</script>\u{202e}"),
        );
        m["sender"] = json!("[sender](https://invalid) <img>\t");
        let out = candidate(vec![m.clone()])
            .supplement("Base", &topics(&["echo request"]))
            .unwrap();
        assert!(out.body.contains("&#91;sender&#93;"));
        assert!(out.body.contains("> ``````\n"));
        assert!(!out.body.contains('\u{202e}'));
        assert_eq!(out.provenance["included"][0]["message"], m);
    }

    #[test]
    fn body_budget_counts_unicode_and_never_clips_a_record() {
        let c = candidate(vec![message(
            json!("Foxtrot request"),
            json!("Foxtrot ready for review."),
        )]);
        let q = topics(&["foxtrot request"]);
        let baseline = c.supplement("", &q).unwrap();
        let count = MAX_BODY_CHARS - baseline.body.chars().count();
        let exact = c.supplement(&"é".repeat(count), &q).unwrap();
        assert_eq!(exact.body.chars().count(), MAX_BODY_CHARS);
        assert!(c.supplement(&"é".repeat(count + 1), &q).is_none());
        assert!(exact.body.contains("Foxtrot ready for review."));
    }

    #[test]
    fn fixed_winner_floor_and_parent_scope_do_not_rerank_or_reassign() {
        let c = candidate(vec![
            message(
                json!("Hotel"),
                json!("Hotel request review accept approve confirm"),
            ),
            message(json!("Hotel journey"), json!("Hotel journey review")),
        ]);
        assert!(c.supplement("Base", &topics(&["hotel journey"])).is_none());
        let mut other = frame(vec![message(json!("Juliet title"), json!("unused"))]);
        other["content"]["messaging"]["view"] = json!("other");
        let c = Candidate::prepare(
            &[source(vec![
                frame(vec![message(
                    json!("India request"),
                    json!("Juliet title: India ready for review."),
                )]),
                other,
            ])],
            &config(),
        )
        .unwrap();
        assert!(c.supplement("Base", &topics(&["india request"])).is_some());
        assert!(Candidate::prepare(&[None], &config()).is_none());
    }

    #[test]
    fn at_most_five_distinct_topic_order_nominees_are_appended() {
        let labels = [
            "lima amber",
            "mike birch",
            "november cedar",
            "oscar dune",
            "papa elm",
            "quebec fern",
        ];
        let messages = labels
            .iter()
            .map(|label| {
                message(
                    json!(format!("{label} request")),
                    json!(format!("{label} ready for review.")),
                )
            })
            .collect();
        let out = candidate(messages)
            .supplement("Base", &topics(&labels))
            .unwrap();
        assert_eq!(
            out.provenance["included"].as_array().unwrap().len(),
            MAX_VALUES
        );
        assert!(!out.body.contains(labels[5]));
    }

    #[test]
    fn story_parse_keeps_model_schema_and_internal_provenance_separate() {
        use crate::contract::{ParsedOutput, PrePostState};
        let mut prepared = crate::PreparedTalent::new("work".into(), config());
        prepared.story_messages = Some(candidate(vec![message(
            json!("Kilo request"),
            json!("Kilo is ready for review."),
        )]));
        let raw = json!({"body":"A compact narrative.","topics":["kilo request"],"confidence":0.7,
            "relations":[{"from":"you","to":"tool","kind":"uses","note":"","quote":"example"}]});
        let schema: Value = serde_json::from_str(
            &std::fs::read_to_string(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../payload/solstone/talent/story.schema.json"),
            )
            .unwrap(),
        )
        .unwrap();
        let validator = jsonschema::validator_for(&schema).unwrap();
        prepared.config.insert("json_schema".into(), schema.clone());
        assert!(validator.is_valid(&raw));
        let ParsedOutput::Json(parsed) =
            crate::story::parse(&raw.to_string(), &prepared, &PrePostState::None).unwrap()
        else {
            panic!("JSON story")
        };
        assert!(
            parsed["body"]
                .as_str()
                .unwrap()
                .starts_with(raw["body"].as_str().unwrap())
        );
        for key in ["topics", "confidence", "relations"] {
            assert_eq!(parsed[key], raw[key]);
        }
        assert!(parsed.get("_source_notes").is_some());
        let mut forged = raw.clone();
        forged["_source_notes"] = json!({"included":"model cannot set this"});
        assert!(!validator.is_valid(&forged));
        assert!(crate::story::parse(&forged.to_string(), &prepared, &PrePostState::None).is_err());
        let mut smaller_schema = schema.clone();
        smaller_schema["properties"]["body"]["maxLength"] =
            json!(raw["body"].as_str().unwrap().chars().count());
        assert!(
            jsonschema::validator_for(&smaller_schema)
                .unwrap()
                .is_valid(&raw)
        );
        prepared.config.insert("json_schema".into(), smaller_schema);
        let ParsedOutput::Json(smaller) =
            crate::story::parse(&raw.to_string(), &prepared, &PrePostState::None).unwrap()
        else {
            panic!("JSON story")
        };
        assert_eq!(smaller["body"], raw["body"]);
        assert!(smaller.get("_source_notes").is_none());
        prepared.config.insert("json_schema".into(), schema);
        let mut full = raw;
        full["body"] = json!("é".repeat(MAX_BODY_CHARS));
        assert!(validator.is_valid(&full));
        let ParsedOutput::Json(capped) =
            crate::story::parse(&full.to_string(), &prepared, &PrePostState::None).unwrap()
        else {
            panic!("JSON story")
        };
        assert_eq!(capped["body"], full["body"]);
        assert!(capped.get("_source_notes").is_none());
    }
}
