// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Phase-two frame selection shared by the native describe pipeline.

use std::collections::{BTreeMap, HashSet};
use std::time::Duration;

use serde_json::{Value, json};
use solstone_core_generate::{
    ContentPart, GenerateRequest, GenerateResponse, ReasonCodeValue, RefusalReason,
    SessionCompletion,
};

use crate::categories::CATEGORIES_META;
use crate::session::DescribeSession;

const PROMPT: &str = include_str!("../assets/extract.md");
const SCHEMA: &str = include_str!("../assets/extract.schema.json");
const REQUEST_ID: &str = "selection:attempt:0";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Importance {
    Ignore,
    Low,
    Normal,
    High,
}

impl Importance {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "ignore" => Some(Self::Ignore),
            "low" => Some(Self::Low),
            "normal" => Some(Self::Normal),
            "high" => Some(Self::High),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct CategoryOverride {
    pub importance: Option<Importance>,
    pub extraction: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CategorizedFrame {
    pub frame_id: u64,
    pub timestamp: f64,
    pub analysis: Value,
}

#[derive(Debug)]
pub enum SelectionError {
    Blocked {
        reason_code: Option<String>,
        provider: Option<String>,
    },
    Session,
}

pub fn select(
    session: &dyn DescribeSession,
    frames: &[CategorizedFrame],
    max_extractions: u32,
    overrides: &BTreeMap<String, CategoryOverride>,
) -> Result<Vec<u64>, SelectionError> {
    let request = request(frames, max_extractions, overrides);
    session
        .submit(request)
        .map_err(|_| SelectionError::Session)?;
    let completion = session
        .recv_timeout(Duration::from_secs(120))
        .map_err(|_| SelectionError::Session)?;
    let SessionCompletion::Response(response) = completion else {
        return Err(SelectionError::Session);
    };
    let id = match &response {
        GenerateResponse::Generated(generated) => generated.id.as_deref(),
        GenerateResponse::Refused(refusal) => refusal.id.as_deref(),
    };
    if id != Some(REQUEST_ID) {
        return Err(SelectionError::Session);
    }

    let selected = match response {
        GenerateResponse::Generated(generated) => {
            parse_selected_ids(&generated.text, frames, max_extractions)
                .unwrap_or_else(|| fallback_select_frames(frames, max_extractions))
        }
        GenerateResponse::Refused(refusal) => {
            if refusal.reason == RefusalReason::NoEngineConfigured
                || refusal.blocking
                || matches!(refusal.reason_code, Some(ReasonCodeValue::Unknown(_)))
            {
                return Err(SelectionError::Blocked {
                    reason_code: refusal.reason_code.map(|value| value.as_wire().to_owned()),
                    provider: refusal.provider,
                });
            }
            fallback_select_frames(frames, max_extractions)
        }
    };
    Ok(finalize_selection(selected, frames, overrides))
}

pub fn request(
    frames: &[CategorizedFrame],
    max_extractions: u32,
    overrides: &BTreeMap<String, CategoryOverride>,
) -> GenerateRequest {
    let summaries = frames
        .iter()
        .map(|frame| {
            let analysis = frame.analysis.as_object();
            json!({
                "frame_id": frame.frame_id,
                "timestamp": frame.timestamp,
                "primary": analysis.and_then(|value| value.get("primary")).and_then(Value::as_str).unwrap_or("?"),
                "secondary": analysis.and_then(|value| value.get("secondary")).and_then(Value::as_str).unwrap_or("none"),
                "overlap": analysis.and_then(|value| value.get("overlap")).and_then(Value::as_bool).unwrap_or(true),
                "visual_description": analysis.and_then(|value| value.get("visual_description")).and_then(Value::as_str).unwrap_or(""),
            })
        })
        .collect::<Vec<_>>();
    GenerateRequest {
        id: Some(REQUEST_ID.to_owned()),
        context: "observe.extract.selection".to_owned(),
        contents: vec![ContentPart::Text {
            text: serde_json::to_string(&summaries).expect("selection summaries are JSON"),
        }],
        system_instruction: Some(selection_instruction(max_extractions, overrides)),
        temperature: 0.3,
        max_output_tokens: 1024,
        thinking_budget: Some(4096),
        timeout_s: None,
        json_output: true,
        json_schema: Some(serde_json::from_str(SCHEMA).expect("selection schema is valid JSON")),
        enforce_responsiveness: true,
        attempt_index: 0,
        exclusive_admission: false,
        transport_retries: None,
    }
}

pub fn parse_selected_ids(
    response: &str,
    frames: &[CategorizedFrame],
    max_extractions: u32,
) -> Option<Vec<u64>> {
    let response = serde_json::from_str::<Value>(response).ok()?;
    let ids = match response {
        Value::Object(mut object) => match object.remove("frame_ids") {
            None => return Some(Vec::new()),
            Some(Value::Array(ids)) => ids,
            Some(_) => return None,
        },
        Value::Array(ids) => ids,
        _ => return None,
    };
    let valid: HashSet<u64> = frames.iter().map(|frame| frame.frame_id).collect();
    Some(
        ids.iter()
            .filter_map(Value::as_u64)
            .filter(|id| valid.contains(id))
            .take(usize::try_from(max_extractions.saturating_mul(2)).unwrap_or(usize::MAX))
            .collect(),
    )
}

pub fn fallback_select_frames(frames: &[CategorizedFrame], max_extractions: u32) -> Vec<u64> {
    if frames.is_empty() {
        return Vec::new();
    }
    if frames.len() <= usize::try_from(max_extractions).unwrap_or(usize::MAX) {
        return frames.iter().map(|frame| frame.frame_id).collect();
    }

    let mut remaining = frames
        .iter()
        .map(|frame| (frame.frame_id, frame.timestamp))
        .collect::<Vec<_>>();
    let seed_index = remaining
        .iter()
        .enumerate()
        .min_by_key(|(_, (frame_id, _))| *frame_id)
        .expect("nonempty frames")
        .0;
    let seed = remaining.remove(seed_index);
    let mut selected = vec![seed];
    while selected.len() < usize::try_from(max_extractions).unwrap_or(usize::MAX)
        && !remaining.is_empty()
    {
        let (best_index, _) = remaining
            .iter()
            .enumerate()
            .map(|(index, (frame_id, timestamp))| {
                let min_distance = selected
                    .iter()
                    .map(|(_, selected_timestamp)| (timestamp - selected_timestamp).abs())
                    .fold(f64::INFINITY, f64::min);
                (index, (min_distance, std::cmp::Reverse(*frame_id)))
            })
            .max_by(|(_, left), (_, right)| {
                left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal)
            })
            .expect("remaining frames");
        selected.push(remaining.remove(best_index));
    }
    selected.into_iter().map(|(frame_id, _)| frame_id).collect()
}

pub fn apply_category_caps(
    selected_ids: Vec<u64>,
    frames: &[CategorizedFrame],
    overrides: &BTreeMap<String, CategoryOverride>,
) -> Vec<u64> {
    let categories = frames
        .iter()
        .map(|frame| {
            (
                frame.frame_id,
                frame
                    .analysis
                    .get("primary")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let mut selected_ids = selected_ids;
    selected_ids.sort_unstable();
    let mut counts = BTreeMap::<Option<String>, u32>::new();
    selected_ids
        .into_iter()
        .filter(|frame_id| {
            let category = categories.get(frame_id).cloned().flatten();
            match category
                .as_deref()
                .and_then(|name| overrides.get(name))
                .and_then(|override_value| override_value.importance)
                .unwrap_or(Importance::Normal)
            {
                Importance::Ignore => false,
                Importance::Low => {
                    let count = counts.entry(category).or_default();
                    if *count >= 2 {
                        false
                    } else {
                        *count += 1;
                        true
                    }
                }
                Importance::Normal | Importance::High => true,
            }
        })
        .collect()
}

pub fn finalize_selection(
    selected_ids: Vec<u64>,
    frames: &[CategorizedFrame],
    overrides: &BTreeMap<String, CategoryOverride>,
) -> Vec<u64> {
    if frames.is_empty() {
        return Vec::new();
    }
    let mut selected_ids = apply_category_caps(selected_ids, frames, overrides);
    let first = frames
        .iter()
        .map(|frame| frame.frame_id)
        .min()
        .expect("nonempty frames");
    if !selected_ids.contains(&first) {
        selected_ids.insert(0, first);
    }
    selected_ids.sort_unstable();
    selected_ids
}

fn selection_instruction(
    max_extractions: u32,
    overrides: &BTreeMap<String, CategoryOverride>,
) -> String {
    prompt_body(PROMPT)
        .replace("$extraction_guidance", &extraction_guidance(overrides))
        .replace("$max_extractions", &max_extractions.to_string())
}

fn prompt_body(prompt: &str) -> &str {
    prompt
        .strip_prefix("---\n")
        .and_then(|prompt| prompt.split_once("\n---\n").map(|(_, body)| body.trim()))
        .expect("embedded selection prompt has frontmatter")
}

fn extraction_guidance(overrides: &BTreeMap<String, CategoryOverride>) -> String {
    let mut high = Vec::new();
    let mut normal = Vec::new();
    let mut low = Vec::new();
    let mut ignore = Vec::new();
    let mut categories = CATEGORIES_META.iter().collect::<Vec<_>>();
    categories.sort_unstable_by_key(|category| category.name);
    for category in categories {
        let override_value = overrides.get(category.name);
        let importance = override_value
            .and_then(|value| value.importance)
            .unwrap_or(Importance::Normal);
        let extraction = override_value
            .and_then(|value| value.extraction.as_deref())
            .filter(|value| !value.is_empty())
            .or(category.extraction.as_deref());
        let entry = match importance {
            Importance::Ignore => format!("- {}", category.name),
            _ => match extraction {
                Some(extraction) => format!("- {}: {extraction}", category.name),
                None => continue,
            },
        };
        match importance {
            Importance::High => high.push(entry),
            Importance::Normal => normal.push(entry),
            Importance::Low => low.push(entry),
            Importance::Ignore => ignore.push(entry),
        }
    }
    if high.is_empty() && low.is_empty() && ignore.is_empty() {
        return if !normal.is_empty() {
            normal.join("\n")
        } else {
            "No category-specific rules.".to_owned()
        };
    }
    let mut sections = Vec::new();
    if !high.is_empty() {
        sections.push(format!("**Prioritize:**\n{}", high.join("\n")));
    }
    if !normal.is_empty() {
        sections.push(format!("**Normal:**\n{}", normal.join("\n")));
    }
    if !low.is_empty() {
        sections.push(format!("**Low priority:**\n{}", low.join("\n")));
    }
    if !ignore.is_empty() {
        sections.push(format!("**Skip unless notable:**\n{}", ignore.join("\n")));
    }
    if !sections.is_empty() {
        sections.join("\n\n")
    } else {
        "No category-specific rules.".to_owned()
    }
}

#[cfg(all(test, not(feature = "full-tests")))]
mod tests {
    use std::collections::BTreeMap;

    use serde::Deserialize;
    use serde_json::json;

    use super::{
        CategorizedFrame, CategoryOverride, Importance, apply_category_caps, extraction_guidance,
        fallback_select_frames, finalize_selection, parse_selected_ids,
    };

    #[derive(Deserialize)]
    struct Fixture {
        frames: Vec<Frame>,
        max_extractions: u32,
        expected_fallback_order: Vec<u64>,
        expected_final_order: Vec<u64>,
    }
    #[derive(Deserialize)]
    struct Frame {
        frame_id: u64,
        timestamp: f64,
    }

    fn frames(ids: &[(u64, f64, &str)]) -> Vec<CategorizedFrame> {
        ids.iter()
            .map(|(frame_id, timestamp, primary)| CategorizedFrame {
                frame_id: *frame_id,
                timestamp: *timestamp,
                analysis: json!({"primary": primary}),
            })
            .collect()
    }

    #[test]
    fn accepts_wrapped_and_bare_selection_responses() {
        let frames = frames(&[(1, 0.0, "code"), (2, 1.0, "code")]);
        assert_eq!(
            parse_selected_ids(r#"{"frame_ids":[2,1]}"#, &frames, 20),
            Some(vec![2, 1])
        );
        assert_eq!(parse_selected_ids("[2,1]", &frames, 20), Some(vec![2, 1]));
    }

    #[test]
    fn filters_invalid_ids_before_preserving_response_order_at_hard_cap() {
        let frames = frames(&[(1, 0.0, "code"), (2, 1.0, "code"), (3, 2.0, "code")]);
        assert_eq!(
            parse_selected_ids(r#"{"frame_ids":[999,3,1,2]}"#, &frames, 1),
            Some(vec![3, 1])
        );
    }

    #[test]
    fn fallback_matches_evenly_spaced_fixture_with_lowest_id_ties() {
        let fixture: Fixture =
            serde_json::from_str(include_str!("../../../fixtures/describe_selection.json"))
                .expect("selection fixture");
        let frames = fixture
            .frames
            .iter()
            .map(|frame| CategorizedFrame {
                frame_id: frame.frame_id,
                timestamp: frame.timestamp,
                analysis: json!({"primary":"code"}),
            })
            .collect::<Vec<_>>();
        let fallback = fallback_select_frames(&frames, fixture.max_extractions);
        assert_eq!(fallback, fixture.expected_fallback_order);
        assert_eq!(
            finalize_selection(fallback, &frames, &BTreeMap::new()),
            fixture.expected_final_order
        );
    }

    #[test]
    fn category_caps_only_consult_config_overrides_and_restore_first_frame() {
        let categorized_frames = frames(&[
            (1, 0.0, "gaming"),
            (2, 1.0, "gaming"),
            (3, 2.0, "gaming"),
            (4, 3.0, "code"),
        ]);
        assert_eq!(
            apply_category_caps(vec![4, 3, 2, 1], &categorized_frames, &BTreeMap::new()),
            vec![1, 2, 3, 4],
            "gaming frontmatter importance is not a cap"
        );
        let mut overrides = BTreeMap::new();
        overrides.insert(
            "gaming".to_owned(),
            CategoryOverride {
                importance: Some(Importance::Ignore),
                extraction: None,
            },
        );
        assert_eq!(
            finalize_selection(vec![4, 3, 2, 1], &categorized_frames, &overrides),
            vec![1, 4],
            "the first frame is restored after its ignored category is dropped"
        );

        overrides.insert(
            "gaming".to_owned(),
            CategoryOverride {
                importance: Some(Importance::Low),
                extraction: None,
            },
        );
        assert_eq!(
            apply_category_caps(vec![4, 3, 2, 1], &categorized_frames, &overrides),
            vec![1, 2, 4],
            "low keeps the two lowest frame ids in a category"
        );

        let many_code = frames(&[
            (1, 0.0, "code"),
            (2, 1.0, "code"),
            (3, 2.0, "code"),
            (4, 3.0, "code"),
        ]);
        for importance in [Importance::Normal, Importance::High] {
            let mut overrides = BTreeMap::new();
            overrides.insert(
                "code".to_owned(),
                CategoryOverride {
                    importance: Some(importance),
                    extraction: None,
                },
            );
            assert_eq!(
                apply_category_caps(vec![4, 3, 2, 1], &many_code, &overrides),
                vec![1, 2, 3, 4],
                "{importance:?} is uncapped"
            );
        }
    }

    #[test]
    fn extraction_guidance_prefers_nonempty_config_override() {
        let mut overrides = BTreeMap::new();
        overrides.insert(
            "browsing".to_owned(),
            CategoryOverride {
                importance: None,
                extraction: Some("Use the configured browsing guidance.".to_owned()),
            },
        );
        let guidance = extraction_guidance(&overrides);
        assert!(guidance.contains("- browsing: Use the configured browsing guidance."));
        assert!(!guidance.contains("Extract when visiting distinctly different websites"));
    }
}
