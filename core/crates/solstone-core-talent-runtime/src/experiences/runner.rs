// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Direct, backend-agnostic runners for two-stage daily Experience detection
//! (`run_detect_day`) and Experience composition with 1-shot corrective
//! retry (`run_compose_experience`). Shared by `solstone journal experiences`,
//! `journal think --experience`, and tests.

use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use solstone_core_facets::{ExperienceRecord, apply_composed_experience, get_experience};
use solstone_core_generate::{ContentPart, GenerateExecutor, GenerateRequest, GenerateResponse};
use solstone_core_journal_io::{AtomicWriteOptions, atomic_replace};
use solstone_core_talent_config::TalentConfig;

use super::compose::{
    ComposeAudit, apply_deterministic_repair, build_composer_packet, build_corrective_retry_suffix,
    postprocess_compose_output,
};
use super::detect::{
    DetectedCandidate, Stage1AAudit, Stage1ACandidateFile, Stage1BAudit,
    apply_detected_experiences, candidates_to_upsert_inputs, postprocess_stage1a_facet,
    postprocess_stage1b_verdicts, render_stage1a_facet_digest, render_stage1b_arbitration_packet,
    stage1a_candidate_path,
};
use super::prefilter::load_day_multi_facet_pool;

/// Outcome of `run_detect_day`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DetectDayResult {
    pub day: String,
    pub facets_scanned: Vec<String>,
    pub stage1a_candidates: usize,
    pub stage1a_audits: Vec<(String, Stage1AAudit)>,
    pub stage1b_audit: Option<Stage1BAudit>,
    pub committed: Vec<CommittedExperienceRef>,
    pub composed: Vec<ComposeRunResult>,
}

/// Summary of one detected Experience persisted by `run_detect_day`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CommittedExperienceRef {
    pub primary_facet: String,
    pub anchor_day: String,
    pub id: String,
    pub path: String,
}

/// Outcome of `run_compose_experience`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ComposeRunResult {
    pub primary_facet: String,
    pub anchor_day: String,
    pub id: String,
    pub retried: bool,
    pub repaired: bool,
    pub audit: ComposeAudit,
    pub experience: ExperienceRecord,
}

fn load_named_talent(
    talent_root: &Path,
    apps_root: &Path,
    name: &str,
) -> Result<(TalentConfig, Value), String> {
    let configs = solstone_core_talent_config::discover(talent_root, apps_root)
        .map_err(|e| format!("discover talents: {e}"))?;
    let cfg = configs.into_iter().find(|c| c.key == name).ok_or_else(|| {
        format!(
            "shipped talent `{name}` not found under {}",
            talent_root.display()
        )
    })?;
    let schema_name = cfg
        .metadata
        .get("schema")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("talent `{name}` missing schema declaration"))?;
    let schema_path = talent_root.join(schema_name);
    let schema_text = fs::read_to_string(&schema_path)
        .map_err(|e| format!("read schema {}: {e}", schema_path.display()))?;
    let schema: Value = serde_json::from_str(&schema_text)
        .map_err(|e| format!("parse schema {}: {e}", schema_path.display()))?;
    Ok((cfg, schema))
}

fn call_json_generate(
    generate: &dyn GenerateExecutor,
    prompt_body: &str,
    schema: &Value,
    temperature: f64,
    max_output_tokens: u32,
) -> Result<Value, String> {
    let req = GenerateRequest {
        id: None,
        context: "experiences".to_owned(),
        contents: vec![ContentPart::Text {
            text: prompt_body.to_owned(),
        }],
        overflow_text: None,
        system_instruction: None,
        temperature,
        max_output_tokens: u64::from(max_output_tokens),
        timeout_s: None,
        json_output: true,
        json_schema: Some(schema.clone()),
        attempt_index: 0,
        exclusive_admission: false,
        transport_retries: None,
    };
    let resp = generate
        .execute(&req)
        .map_err(|e| format!("generate failed: {e}"))?;
    let text = match resp {
        GenerateResponse::Generated(ok) => ok.text,
        GenerateResponse::Refused(refused) => {
            let code = refused
                .reason_code
                .as_ref()
                .map(|c| c.as_wire())
                .unwrap_or("unknown");
            return Err(format!("generate refused ({code}): {}", refused.detail));
        }
        GenerateResponse::Cancelled => return Err("generate cancelled".to_owned()),
    };
    parse_json_lenient(&text)
}

fn parse_json_lenient(raw: &str) -> Result<Value, String> {
    let trimmed = raw.trim();
    if let Ok(v) = serde_json::from_str::<Value>(trimmed) {
        return Ok(v);
    }
    // Strip accidental markdown fence if a backend ignored strict JSON mode.
    if let Some(rest) = trimmed.strip_prefix("```") {
        let after_lang = rest.split_once('\n').map(|(_, body)| body).unwrap_or(rest);
        let unwrapped = after_lang
            .trim_end()
            .strip_suffix("```")
            .unwrap_or(after_lang)
            .trim();
        if let Ok(v) = serde_json::from_str::<Value>(unwrapped) {
            return Ok(v);
        }
    }
    if let (Some(start), Some(end)) = (trimmed.find('{'), trimmed.rfind('}'))
        && start < end
        && let Ok(v) = serde_json::from_str::<Value>(&trimmed[start..=end])
    {
        return Ok(v);
    }
    Err(format!(
        "model did not return valid JSON (len={}): {}",
        trimmed.len(),
        super::prefilter::truncate_chars(trimmed, 160)
    ))
}

/// Run the full two-stage daily Experience detector (`experiences_detect_facet`
/// Stage 1A per active facet -> solo pre-gate & gap/shift splitting ->
/// `experiences_detect` Stage 1B -> commit + `OwnerAsk`), and optionally compose immediately.
pub fn run_detect_day(
    journal: &Path,
    day: &str,
    facets_filter: Option<&[String]>,
    compose_after: bool,
    generate: &dyn GenerateExecutor,
    talent_root: &Path,
    apps_root: &Path,
) -> Result<DetectDayResult, String> {
    let (cfg_1a, schema_1a) =
        load_named_talent(talent_root, apps_root, "experiences_detect_facet")?;
    let (cfg_1b, schema_1b) = load_named_talent(talent_root, apps_root, "experiences_detect")?;

    let temp_1a = cfg_1a
        .metadata
        .get("temperature")
        .and_then(Value::as_f64)
        .unwrap_or(0.2);
    let temp_1b = cfg_1b
        .metadata
        .get("temperature")
        .and_then(Value::as_f64)
        .unwrap_or(0.1);
    let max_out_1a = cfg_1a
        .metadata
        .get("max_output_tokens")
        .and_then(Value::as_u64)
        .and_then(|v| u32::try_from(v).ok())
        .unwrap_or(2048);
    let max_out_1b = cfg_1b
        .metadata
        .get("max_output_tokens")
        .and_then(Value::as_u64)
        .and_then(|v| u32::try_from(v).ok())
        .unwrap_or(1536);

    let overhead_1a = solstone_core_tokens::count(&cfg_1a.body)
        .saturating_add(solstone_core_tokens::count(&schema_1a.to_string()))
        .saturating_add(512);
    let overhead_1b = solstone_core_tokens::count(&cfg_1b.body)
        .saturating_add(solstone_core_tokens::count(&schema_1b.to_string()))
        .saturating_add(512);

    let pool = load_day_multi_facet_pool(journal, day, facets_filter)?;
    let facets_scanned: Vec<String> = pool.facets.keys().cloned().collect();
    let mut all_candidates: Vec<DetectedCandidate> = Vec::new();
    let mut stage1a_audits = Vec::new();

    for facet in &facets_scanned {
        let Some((digest, _)) = render_stage1a_facet_digest(
            &pool,
            facet,
            &pool.open_prev_experiences,
            max_out_1a,
            overhead_1a,
        ) else {
            continue;
        };
        let prompt = cfg_1a.body.replace("$facet_digest", &digest);
        let raw = call_json_generate(generate, &prompt, &schema_1a, temp_1a, max_out_1a)?;
        let (cands, audit) = postprocess_stage1a_facet(&raw, &pool, facet);

        // Persist Stage 1A intermediate artifact under chronicle/<day>/health/experience-candidates/<facet>.json.
        let cand_path = stage1a_candidate_path(journal, day, facet)?;
        if let Some(parent) = cand_path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let envelope = Stage1ACandidateFile {
            version: 1,
            day: day.to_owned(),
            facet: facet.clone(),
            candidates: cands.clone(),
        };
        if let Ok(bytes) = serde_json::to_vec_pretty(&envelope) {
            let mut out = bytes;
            out.push(b'\n');
            let _ = atomic_replace(&cand_path, &out, AtomicWriteOptions { mode: Some(0o600) });
        }

        stage1a_audits.push((facet.clone(), audit));
        all_candidates.extend(cands);
    }

    let stage1a_candidates = all_candidates.len();
    if all_candidates.is_empty() {
        return Ok(DetectDayResult {
            day: day.to_owned(),
            facets_scanned,
            stage1a_candidates: 0,
            stage1a_audits,
            stage1b_audit: None,
            committed: Vec::new(),
            composed: Vec::new(),
        });
    }

    all_candidates.sort_by(|a, b| {
        a.started_at
            .cmp(&b.started_at)
            .then_with(|| a.primary_facet.cmp(&b.primary_facet))
            .then_with(|| a.title_hint.cmp(&b.title_hint))
    });
    let packet_1b =
        render_stage1b_arbitration_packet(&pool, &mut all_candidates, max_out_1b, overhead_1b);
    let prompt_1b = cfg_1b.body.replace("$candidates_packet", &packet_1b);
    let raw_1b = call_json_generate(generate, &prompt_1b, &schema_1b, temp_1b, max_out_1b)?;
    let (accepted, stage1b_audit) = postprocess_stage1b_verdicts(&raw_1b, &all_candidates);

    let now_iso =
        solstone_core_time::utc_z(jiff::Timestamp::now(), solstone_core_time::Precision::Secs);
    let upsert_inputs = candidates_to_upsert_inputs(&accepted);
    let committed_tuples = apply_detected_experiences(journal, &upsert_inputs, &now_iso)?;
    let committed: Vec<CommittedExperienceRef> = committed_tuples
        .into_iter()
        .map(
            |(primary_facet, anchor_day, id, path)| CommittedExperienceRef {
                primary_facet,
                anchor_day,
                id,
                path,
            },
        )
        .collect();

    let mut composed = Vec::new();
    if compose_after {
        for item in &committed {
            let res = run_compose_experience(
                journal,
                &item.primary_facet,
                &item.anchor_day,
                &item.id,
                generate,
                talent_root,
                apps_root,
            )?;
            composed.push(res);
        }
    }

    Ok(DetectDayResult {
        day: day.to_owned(),
        facets_scanned,
        stage1a_candidates,
        stage1a_audits,
        stage1b_audit: Some(stage1b_audit),
        committed,
        composed,
    })
}

/// Run the Experience Composer (`experience_compose`) for one Experience with
/// grounded locations, clamped moment timestamps, humanized channels, hard
/// deterministic post-hooks, 1-shot corrective retry, and durable commit respecting `owner_holds`.
pub fn run_compose_experience(
    journal: &Path,
    facet: &str,
    anchor_day: &str,
    exp_id: &str,
    generate: &dyn GenerateExecutor,
    talent_root: &Path,
    apps_root: &Path,
) -> Result<ComposeRunResult, String> {
    let experience = get_experience(journal, facet, anchor_day, exp_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("experience not found: {facet}/{anchor_day}/{exp_id}"))?;
    if experience.hidden {
        return Err(format!("experience is muted: {exp_id}"));
    }

    let (cfg, schema) = load_named_talent(talent_root, apps_root, "experience_compose")?;
    let temp = cfg
        .metadata
        .get("temperature")
        .and_then(Value::as_f64)
        .unwrap_or(0.2);
    let max_out = cfg
        .metadata
        .get("max_output_tokens")
        .and_then(Value::as_u64)
        .and_then(|v| u32::try_from(v).ok())
        .unwrap_or(3072);
    let overhead = solstone_core_tokens::count(&cfg.body)
        .saturating_add(solstone_core_tokens::count(&schema.to_string()))
        .saturating_add(768);

    let packet = build_composer_packet(journal, &experience, max_out, overhead)?;
    if packet.act_entries.is_empty() {
        return Err(format!(
            "experience {exp_id} has no live constituent activities to compose"
        ));
    }

    let base_prompt = cfg.body.replace("$experience_packet", &packet.prompt_text);
    let raw_1 = call_json_generate(generate, &base_prompt, &schema, temp, max_out)?;
    let (mut composed, mut audit) = postprocess_compose_output(&raw_1, &packet);

    let mut retried = false;
    if audit.needs_retry {
        retried = true;
        let retry_prompt = format!(
            "{}{}",
            base_prompt,
            build_corrective_retry_suffix(&audit, &packet.owner.preferred)
        );
        if let Ok(raw_2) = call_json_generate(generate, &retry_prompt, &schema, 0.1, max_out) {
            let (composed_2, audit_2) = postprocess_compose_output(&raw_2, &packet);
            // Prefer pass 2 if it reduced or eliminated violations.
            if audit_2.retry_reasons.len() <= audit.retry_reasons.len() {
                composed = composed_2;
                audit = audit_2;
            }
        }
    }

    let mut repaired = false;
    if audit.needs_retry {
        repaired = true;
        apply_deterministic_repair(&mut composed);
    }

    let now_iso =
        solstone_core_time::utc_z(jiff::Timestamp::now(), solstone_core_time::Precision::Secs);
    let updated = apply_composed_experience(journal, facet, anchor_day, exp_id, composed, &now_iso)
        .map_err(|e| e.to_string())?;

    // Emit JSON summary metadata cleanly when callers serialize.
    let _ = json!({
        "id": updated.id,
        "retried": retried,
        "repaired": repaired,
    });

    Ok(ComposeRunResult {
        primary_facet: updated.primary_facet.clone(),
        anchor_day: updated.anchor_day.clone(),
        id: updated.id.clone(),
        retried,
        repaired,
        audit,
        experience: updated,
    })
}
