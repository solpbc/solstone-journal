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
                .map(|selected| finalize_selection(selected, frames, overrides))
                .unwrap_or_else(|| fallback_with_category_caps(frames, max_extractions, overrides))
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
            fallback_with_category_caps(frames, max_extractions, overrides)
        }
    };
    Ok(selected)
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
        max_output_tokens: 512,
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
    let mut seen = HashSet::new();
    Some(
        ids.iter()
            .filter_map(Value::as_u64)
            .filter(|id| valid.contains(id))
            .filter(|id| seen.insert(*id))
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

fn fallback_with_category_caps(
    frames: &[CategorizedFrame],
    max_extractions: u32,
    overrides: &BTreeMap<String, CategoryOverride>,
) -> Vec<u64> {
    let baseline = finalize_selection(
        fallback_select_frames(frames, max_extractions),
        frames,
        overrides,
    );
    let mut selected = frames
        .iter()
        .filter(|frame| baseline.contains(&frame.frame_id))
        .collect::<Vec<_>>();
    // Keep the existing fallback's choices and first-frame anchor. Refill only
    // slots removed by category caps, using its same farthest-in-time rule.
    while selected.len() < usize::try_from(max_extractions).unwrap_or(usize::MAX) {
        let candidate = frames
            .iter()
            .filter(|frame| {
                !selected
                    .iter()
                    .any(|chosen| chosen.frame_id == frame.frame_id)
            })
            .filter(|frame| {
                let category = frame.analysis.get("primary").and_then(Value::as_str);
                match resolved_importance(category, overrides) {
                    Importance::Ignore => false,
                    Importance::Low => {
                        selected
                            .iter()
                            .filter(|chosen| {
                                chosen.analysis.get("primary").and_then(Value::as_str) == category
                            })
                            .count()
                            < 2
                    }
                    Importance::Normal | Importance::High => true,
                }
            })
            .map(|frame| {
                let min_distance = selected
                    .iter()
                    .map(|chosen| (frame.timestamp - chosen.timestamp).abs())
                    .fold(f64::INFINITY, f64::min);
                (frame, (min_distance, std::cmp::Reverse(frame.frame_id)))
            })
            .max_by(|(_, left), (_, right)| {
                left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal)
            });
        let Some((frame, _)) = candidate else {
            break;
        };
        selected.push(frame);
    }
    let mut ids = selected
        .into_iter()
        .map(|frame| frame.frame_id)
        .collect::<Vec<_>>();
    ids.sort_unstable();
    ids
}

fn resolved_importance(
    category: Option<&str>,
    overrides: &BTreeMap<String, CategoryOverride>,
) -> Importance {
    let Some(category) = category else {
        return Importance::Normal;
    };
    if let Some(importance) = overrides.get(category).and_then(|value| value.importance) {
        return importance;
    }
    let definition = CATEGORIES_META.iter().find(|meta| meta.name == category);
    definition
        .and_then(|meta| meta.importance.as_deref())
        .map(|importance| {
            Importance::parse(importance).unwrap_or_else(|| {
                panic!("invalid embedded category importance for {category}: {importance:?}")
            })
        })
        .unwrap_or(Importance::Normal)
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
    selected_ids.dedup();
    let mut counts = BTreeMap::<Option<String>, u32>::new();
    selected_ids
        .into_iter()
        .filter(|frame_id| {
            let category = categories.get(frame_id).cloned().flatten();
            match resolved_importance(category.as_deref(), overrides) {
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
        let importance = resolved_importance(Some(category.name), overrides);
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
    use std::time::Duration;

    use crate::session::DescribeSession;
    use serde::Deserialize;
    use serde_json::json;
    use solstone_core_generate::{
        GenerateRequest, GenerateResponse, GeneratedResponse, RefusalReason, RefusedResponse,
        SessionCloseError, SessionCompletion, SessionReceiveError, SessionSubmitError,
    };

    use super::{
        CategorizedFrame, CategoryOverride, Importance, SelectionError, apply_category_caps,
        extraction_guidance, fallback_select_frames, finalize_selection, parse_selected_ids,
        select,
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

    struct Reply(GenerateResponse);

    impl DescribeSession for Reply {
        fn submit(&self, _: GenerateRequest) -> Result<(), SessionSubmitError> {
            Ok(())
        }

        fn recv_timeout(&self, _: Duration) -> Result<SessionCompletion, SessionReceiveError> {
            Ok(SessionCompletion::Response(self.0.clone()))
        }

        fn close(&self) -> Result<(), SessionCloseError> {
            Ok(())
        }
    }

    fn generated(text: &str) -> Reply {
        Reply(GenerateResponse::Generated(Box::new(GeneratedResponse {
            id: Some(super::REQUEST_ID.to_owned()),
            text: text.to_owned(),
            model: "test".to_owned(),
            usage: json!({}),
            finish_reason: "stop".to_owned(),
            thinking: None,
            schema_validation: None,
            input_budget: None,
            request_budget: None,
            inference: None,
            hints_applied: Vec::new(),
        })))
    }

    fn refused(reason: RefusalReason, blocking: bool) -> Reply {
        Reply(GenerateResponse::Refused(RefusedResponse {
            id: Some(super::REQUEST_ID.to_owned()),
            reason,
            reason_code: None,
            retryable: false,
            blocking,
            reset_at_ms: None,
            provider: None,
            detail: String::new(),
        }))
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
    fn duplicate_ids_do_not_consume_response_or_category_limits() {
        let categorized = frames(&[
            (1, 0.0, "terminal"),
            (2, 1.0, "terminal"),
            (3, 2.0, "terminal"),
        ]);
        for response in ["[1,1,2,3]", r#"{"frame_ids":[1,1,2,3]}"#] {
            let selected = parse_selected_ids(response, &categorized, 1).unwrap();
            assert_eq!(selected, vec![1, 2]);
            assert_eq!(
                finalize_selection(selected, &categorized, &BTreeMap::new()),
                vec![1, 2]
            );
        }
        assert_eq!(
            apply_category_caps(vec![1, 1, 2, 3], &categorized, &BTreeMap::new()),
            vec![1, 2],
        );
    }

    #[test]
    fn fallback_refills_capped_slots_without_replacing_existing_choices() {
        let categorized = frames(&[
            (1, 0.0, "gaming"),
            (2, 1.0, "messaging"),
            (3, 2.0, "gaming"),
            (4, 3.0, "reading"),
            (5, 4.0, "gaming"),
            (6, 5.0, "messaging"),
            (7, 6.0, "reading"),
            (8, 7.0, "messaging"),
            (9, 8.0, "gaming"),
        ]);
        let overrides = BTreeMap::new();
        let baseline = finalize_selection(
            fallback_select_frames(&categorized, 5),
            &categorized,
            &overrides,
        );
        assert_eq!(baseline, vec![1, 7]);
        for reply in [
            generated("invalid JSON"),
            refused(RefusalReason::IncompleteJson, false),
        ] {
            let chosen = select(&reply, &categorized, 5, &overrides).unwrap();
            assert_eq!(chosen, vec![1, 2, 4, 6, 7]);
            assert!(baseline.iter().all(|id| chosen.contains(id)));
        }

        let overrides = BTreeMap::from([(
            "gaming".to_owned(),
            CategoryOverride {
                importance: Some(Importance::Low),
                extraction: None,
            },
        )]);
        assert_eq!(
            select(&generated("invalid JSON"), &categorized, 5, &overrides).unwrap(),
            vec![1, 2, 3, 4, 7],
            "refill counts the Low anchor toward the two-frame category quota"
        );
    }

    #[test]
    fn valid_model_selection_is_not_refilled_and_preserves_the_anchor_exception() {
        let categorized = frames(&[
            (1, 0.0, "code"),
            (2, 1.0, "reading"),
            (3, 2.0, "code"),
            (4, 3.0, "reading"),
            (5, 4.0, "code"),
        ]);
        for (response, expected) in [
            ("[4]", vec![1, 4]),
            ("[]", vec![1]),
            (r#"{"frame_ids":[]}"#, vec![1]),
            ("{}", vec![1]),
            ("[3,5]", vec![1, 3, 5]),
        ] {
            assert_eq!(
                select(&generated(response), &categorized, 5, &BTreeMap::new()).unwrap(),
                expected,
                "{response}"
            );
        }
    }

    #[test]
    fn fallback_returns_fewer_when_preferences_exhaust_eligible_frames() {
        let categorized = frames(&[
            (1, 0.0, "code"),
            (2, 1.0, "code"),
            (3, 2.0, "code"),
            (4, 3.0, "code"),
        ]);
        assert_eq!(
            select(
                &generated("invalid JSON"),
                &categorized,
                5,
                &BTreeMap::new()
            )
            .unwrap(),
            vec![1, 2]
        );
        let overrides = BTreeMap::from([(
            "code".to_owned(),
            CategoryOverride {
                importance: Some(Importance::Ignore),
                extraction: None,
            },
        )]);
        assert_eq!(
            select(&generated("invalid JSON"), &categorized, 5, &overrides).unwrap(),
            vec![1]
        );
        assert_eq!(
            select(&generated("invalid JSON"), &[], 5, &overrides).unwrap(),
            Vec::<u64>::new()
        );
        assert_eq!(
            select(&generated("invalid JSON"), &categorized, 0, &overrides).unwrap(),
            vec![1]
        );
    }

    #[test]
    fn blocked_refusals_do_not_fall_back() {
        let categorized = frames(&[(1, 0.0, "reading"), (2, 1.0, "reading")]);
        for reply in [
            refused(RefusalReason::NoEngineConfigured, false),
            refused(RefusalReason::AttestationStale, true),
        ] {
            assert!(matches!(
                select(&reply, &categorized, 5, &BTreeMap::new()),
                Err(SelectionError::Blocked { .. })
            ));
        }
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
        let mut overrides = BTreeMap::new();
        overrides.insert(
            "code".to_owned(),
            CategoryOverride {
                importance: Some(Importance::Normal),
                extraction: None,
            },
        );
        assert_eq!(
            finalize_selection(fallback, &frames, &overrides),
            fixture.expected_final_order
        );
    }

    #[test]
    fn category_caps_bind_definition_defaults_and_allow_overrides() {
        let categorized_frames = frames(&[
            (1, 0.0, "gaming"),
            (2, 1.0, "gaming"),
            (3, 2.0, "gaming"),
            (4, 3.0, "code"),
        ]);
        assert_eq!(
            apply_category_caps(vec![4, 3, 2, 1], &categorized_frames, &BTreeMap::new()),
            vec![4],
            "gaming definition importance is ignore so caps drop frames 1, 2, 3"
        );
        assert_eq!(
            finalize_selection(vec![4, 3, 2, 1], &categorized_frames, &BTreeMap::new()),
            vec![1, 4],
            "the first frame is restored after its ignored category is dropped"
        );

        let mut overrides = BTreeMap::new();
        overrides.insert(
            "gaming".to_owned(),
            CategoryOverride {
                importance: None,
                extraction: Some("custom extraction".to_owned()),
            },
        );
        assert_eq!(
            apply_category_caps(vec![4, 3, 2, 1], &categorized_frames, &overrides),
            vec![4],
            "extraction-only override still binds definition ignore"
        );

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

        let browsing_frames = frames(&[
            (1, 0.0, "browsing"),
            (2, 1.0, "browsing"),
            (3, 2.0, "browsing"),
            (4, 3.0, "browsing"),
        ]);
        assert_eq!(
            apply_category_caps(vec![4, 3, 2, 1], &browsing_frames, &BTreeMap::new()),
            vec![1, 2, 3, 4],
            "browsing third-rung default is normal (uncapped)"
        );
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

    #[test]
    fn extraction_guidance_binds_definition_defaults_and_allows_overrides() {
        let guidance = extraction_guidance(&BTreeMap::new());
        assert!(
            guidance.contains("**Skip unless notable:**\n- gaming"),
            "gaming defaults to skip/ignore"
        );
        assert!(
            guidance.contains("**Normal:**") && guidance.contains("- browsing:"),
            "browsing defaults to normal"
        );

        let mut overrides = BTreeMap::new();
        overrides.insert(
            "gaming".to_owned(),
            CategoryOverride {
                importance: Some(Importance::High),
                extraction: Some("Extract gaming scoreboard.".to_owned()),
            },
        );
        let overridden_guidance = extraction_guidance(&overrides);
        assert!(
            overridden_guidance.contains("**Prioritize:**")
                && overridden_guidance.contains("- gaming: Extract gaming scoreboard."),
            "gaming override to high moves it to prioritize heading"
        );
        assert!(
            !overridden_guidance.contains("- gaming\n")
                && !overridden_guidance.ends_with("- gaming"),
            "gaming is no longer under skip heading"
        );
    }

    #[test]
    fn resolved_importance_resolves_three_rungs() {
        let mut overrides = BTreeMap::new();
        overrides.insert(
            "gaming".to_owned(),
            CategoryOverride {
                importance: Some(Importance::High),
                extraction: None,
            },
        );
        overrides.insert(
            "code".to_owned(),
            CategoryOverride {
                importance: None,
                extraction: Some("custom extraction".to_owned()),
            },
        );

        // 1. Override wins
        assert_eq!(
            super::resolved_importance(Some("gaming"), &overrides),
            Importance::High
        );
        // 2. Importance None falls through to definition
        assert_eq!(
            super::resolved_importance(Some("code"), &overrides),
            Importance::Low
        );
        // 3. Live definition with no importance (third rung) -> Normal
        assert_eq!(
            super::resolved_importance(Some("browsing"), &overrides),
            Importance::Normal
        );
        assert_eq!(
            super::resolved_importance(Some("social"), &overrides),
            Importance::Normal
        );
        // 4. Unknown or None category -> Normal
        assert_eq!(
            super::resolved_importance(Some("unknown"), &overrides),
            Importance::Normal
        );
        assert_eq!(
            super::resolved_importance(None, &overrides),
            Importance::Normal
        );
    }
}
