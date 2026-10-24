// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Possible names: the per-segment talent that labels speech which may name a
//! registry entity.
//!
//! After Sense, the pre-step reads the segment's speech transcript
//! (`solstone_core_possible_names::speech_transcript`), finds facet-attached
//! entities whose name, alias, first name, sound or spelling matches a span
//! ([`retrieval`]), and renders the evaluated packet ([`packet`]). The owner's
//! active brain answers `yes` or `no` for each candidate at temperature 0,
//! with the talent's prompt as the system instruction and the packet as the
//! only text. Accepted candidates are written to
//! `talents/possible_names.json` through `solstone-core-possible-names`, and
//! the file is queued for search. The transcript, the registry and every
//! other talent's inputs are left as they are.
//!
//! - A sidecar whose transcript digest, registry revision and talent version
//!   all match is fresh: Generate is skipped (`fresh`).
//! - No candidates skips Generate (`no_candidates`). If an older, no longer
//!   fresh sidecar exists it is replaced by an empty one, so entries chosen
//!   from another transcript, registry or policy stop showing; otherwise
//!   nothing is written.
//! - The model abstaining is an answer: an empty sidecar is written.
//! - A malformed answer fails the talent and writes nothing. The talent never
//!   gates a segment's publication.

mod packet;
mod phon;
mod policy;
mod retrieval;
#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use solstone_core_possible_names::{
    CandidateEntity, Sidecar, SidecarEntry, SidecarSource, load_candidate_entities,
    speech_transcript,
};
use solstone_core_system_health::find_segment_dir;
use solstone_core_transcripts::{next_statement_ordinal, statement_text};

use crate::contract::{CommitPlan, ParsedOutput, PrePostState};
use crate::writers::WriteIntent;
use crate::{
    ExecutionContext, FramedRequest, PreparedTalent, RuntimeOutcome, StageError, stage_error,
};

use retrieval::{Action, Statement};

/// The talent's key and its hook.
pub const TALENT: &str = "entities:possible_names";
const HOOK: &str = "entities:possible_names";
/// The policy generation; the prompt and every [`policy`] constant are
/// hashed after it.
const TALENT_VERSION_PREFIX: &str = "possible-names-v1";
/// Derived text whose words already make a canonical name findable.
const DERIVED_TEXT: [&str; 3] = [
    "talents/activity.md",
    "talents/sense.md",
    "talents/topics.md",
];

/// What the pre-step found, carried to the prompt and the commit.
#[derive(Clone, Debug, PartialEq)]
pub struct PossibleNamesState {
    segment_dir: PathBuf,
    system_instruction: String,
    plan: Plan,
}

/// One segment's retrieval, read-only.
#[derive(Clone, Debug, PartialEq)]
struct Plan {
    source: SidecarSource,
    registry_revision: String,
    talent_version: String,
    facets: Vec<String>,
    /// Every matched entity, before the gate.
    pool: Vec<Action>,
    /// The candidates offered, in id order.
    candidates: Vec<Action>,
    /// Canonical names at retrieval, by entity id.
    names: BTreeMap<String, String>,
    packet: String,
    schema: Value,
    /// A sidecar exists (readable or not).
    existing: bool,
    /// The existing sidecar already answers this exact input.
    fresh: bool,
}

/// `<policy generation>-<12 hex of the prompt and policy>`: a prompt or rule
/// change regenerates every segment's possible names on its next think.
pub(crate) fn talent_version(system_instruction: &str) -> String {
    let digest = Sha256::digest(format!("{system_instruction}\n{}", policy::fingerprint()));
    format!("{TALENT_VERSION_PREFIX}-{}", &hex(&digest)[..12])
}

/// SHA-256 over the canonical serialization of the registry candidates were
/// drawn from: ids in order, with name, type, aka and facet descriptions.
fn registry_revision(entities: &[CandidateEntity]) -> String {
    hex(&Sha256::digest(
        serde_json::to_vec(entities).expect("registry serializes"),
    ))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Every statement of a transcript, numbered as speaker labels number them,
/// with the words readers show (`corrected` over `text`); a statement without
/// words is kept, empty, so line numbers stay ordinals.
fn read_statements(transcript: &str) -> Vec<Statement> {
    let mut ordinal = 0;
    transcript
        .lines()
        .filter_map(|line| {
            let sentence = u64::try_from(next_statement_ordinal(line, &mut ordinal)?).ok()?;
            let text = match serde_json::from_str::<Value>(line.trim()) {
                Ok(Value::Object(record)) => statement_text(&record)
                    .map(|(_, text)| text.to_owned())
                    .unwrap_or_default(),
                _ => String::new(),
            };
            Some(Statement { sentence, text })
        })
        .collect()
}

/// The segment's own facets, in `talents/facets.json` order.
fn read_facets(segment_dir: &Path) -> Vec<String> {
    fs::read(segment_dir.join("talents/facets.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|value| value.as_array().cloned())
        .into_iter()
        .flatten()
        .filter_map(|row| {
            row.get("facet")
                .and_then(Value::as_str)
                .filter(|facet| !facet.is_empty())
                .map(str::to_owned)
        })
        .collect()
}

fn read_derived_text(segment_dir: &Path) -> String {
    let text = DERIVED_TEXT
        .iter()
        .map(|name| fs::read_to_string(segment_dir.join(name)).unwrap_or_default())
        .collect::<Vec<_>>()
        .join("\n");
    retrieval::casefold(&text)
}

/// The retrieval for one segment. `None` when it has no speech transcript.
fn plan_segment(
    journal: &Path,
    segment_dir: &Path,
    system_instruction: &str,
) -> Result<Option<Plan>, String> {
    let Some(file) = speech_transcript(segment_dir) else {
        return Ok(None);
    };
    let bytes = fs::read(segment_dir.join(&file))
        .map_err(|error| format!("transcript {file} could not be read: {error}"))?;
    let transcript = std::str::from_utf8(&bytes)
        .map_err(|error| format!("transcript {file} is not UTF-8: {error}"))?;
    let source = SidecarSource {
        file,
        sha256: hex(&Sha256::digest(&bytes)),
    };
    let statements = read_statements(transcript);
    let entities = load_candidate_entities(journal)?;
    let registry_revision = registry_revision(&entities);
    let talent_version = talent_version(system_instruction);
    let facets = read_facets(segment_dir);
    let pool = retrieval::find_actions(&statements, &entities, &read_derived_text(segment_dir));
    let candidates = retrieval::select(&pool);
    let registry = entities
        .iter()
        .map(|entity| (entity.entity_id.as_str(), entity))
        .collect::<BTreeMap<_, _>>();
    let packet = packet::render(&statements, &facets, &candidates, &registry);
    let names = candidates
        .iter()
        .map(|candidate| {
            (
                candidate.entity_id.clone(),
                registry[candidate.entity_id.as_str()].name.clone(),
            )
        })
        .collect();
    let (existing, fresh) = match solstone_core_possible_names::read(segment_dir) {
        Ok(None) => (false, false),
        Ok(Some(sidecar)) => (
            true,
            sidecar.source == source
                && sidecar.registry_revision == registry_revision
                && sidecar.talent_version == talent_version,
        ),
        Err(_) => (true, false),
    };
    Ok(Some(Plan {
        schema: packet::schema(candidates.len()),
        source,
        registry_revision,
        talent_version,
        facets,
        pool,
        candidates,
        names,
        packet,
        existing,
        fresh,
    }))
}

fn sidecar(plan: &Plan, accepted: &[usize]) -> Sidecar {
    Sidecar::new(
        plan.talent_version.clone(),
        plan.source.clone(),
        plan.registry_revision.clone(),
        chrono::Utc::now().timestamp_millis(),
        accepted
            .iter()
            .map(|index| {
                let candidate = &plan.candidates[*index];
                SidecarEntry {
                    sentence: candidate.sentence,
                    start_byte: candidate.start_byte,
                    end_byte: candidate.end_byte,
                    heard: candidate.heard.clone(),
                    entity_id: candidate.entity_id.clone(),
                    name: plan.names[&candidate.entity_id].clone(),
                }
            })
            .collect(),
    )
}

fn segment(prepared: &PreparedTalent, journal: &Path) -> Option<PathBuf> {
    find_segment_dir(
        journal,
        prepared.config.get("day")?.as_str()?,
        prepared.config.get("segment")?.as_str()?,
        prepared.config.get("stream").and_then(Value::as_str),
    )
}

/// The talent's prompt, which is sent as the system instruction.
fn system_instruction(prepared: &PreparedTalent) -> Result<String, StageError> {
    prepared
        .config
        .get("user_instruction")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|prompt| !prompt.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| stage_error("build", HOOK, prepared, "the talent has no prompt"))
}

fn is_set(prepared: &PreparedTalent, key: &str) -> bool {
    prepared
        .config
        .get(key)
        .is_some_and(solstone_core_talent_config::is_truthy)
}

pub fn build(
    prepared: &mut PreparedTalent,
    context: &ExecutionContext,
    cancellation: &solstone_core_generate::Cancellation,
) -> Result<PrePostState, RuntimeOutcome> {
    let skipped = |prepared: &PreparedTalent, reason: &str| RuntimeOutcome::Skipped {
        stage: HOOK.to_owned(),
        talent: prepared.name.clone(),
        reason: reason.to_owned(),
    };
    let Some(segment_dir) = segment(prepared, &context.journal) else {
        return Err(skipped(prepared, "no_segment"));
    };
    let system_instruction = system_instruction(prepared).map_err(RuntimeOutcome::StageFailed)?;
    let plan = match plan_segment(&context.journal, &segment_dir, &system_instruction) {
        Ok(Some(plan)) => plan,
        Ok(None) => return Err(skipped(prepared, "no_transcript")),
        Err(detail) => {
            return Err(RuntimeOutcome::StageFailed(stage_error(
                "build", HOOK, prepared, detail,
            )));
        }
    };
    if plan.fresh && !is_set(prepared, "refresh") {
        return Err(skipped(prepared, "fresh"));
    }
    if plan.candidates.is_empty() {
        // Entries chosen from an older transcript, registry or policy would
        // otherwise keep showing; nothing new needs a file.
        if plan.existing && !is_set(prepared, crate::DRY_RUN_KEY) {
            crate::writers::apply_prepared(
                CommitPlan::Write(WriteIntent::PossibleNames {
                    segment_dir,
                    sidecar: sidecar(&plan, &[]),
                }),
                context,
                prepared,
                cancellation,
            )
            .map_err(RuntimeOutcome::StageFailed)?;
        }
        return Err(skipped(prepared, "no_candidates"));
    }
    Ok(PrePostState::PossibleNames(Box::new(PossibleNamesState {
        segment_dir,
        system_instruction,
        plan,
    })))
}

fn state<'a>(
    phase: &'static str,
    prepared: &PreparedTalent,
    state: &'a PrePostState,
) -> Result<&'a PossibleNamesState, StageError> {
    match state {
        PrePostState::PossibleNames(state) => Ok(state),
        _ => Err(stage_error(
            phase,
            HOOK,
            prepared,
            "missing possible names state",
        )),
    }
}

/// The prompt goes as the system instruction, the packet as the only text,
/// with this request's own answer schema.
pub fn apply_prompt_override(
    prepared: &mut PreparedTalent,
    pre: &PrePostState,
) -> Result<(), StageError> {
    let state = state("prompt_override", prepared, pre)?;
    prepared.framed_request = Some(FramedRequest {
        system_instruction: state.system_instruction.clone(),
        text: state.plan.packet.clone(),
    });
    prepared
        .config
        .insert("json_schema".to_owned(), state.plan.schema.clone());
    Ok(())
}

pub fn parse(
    output: &str,
    prepared: &PreparedTalent,
    pre: &PrePostState,
) -> Result<ParsedOutput, StageError> {
    let state = state("parse", prepared, pre)?;
    let accepted = packet::parse(output, state.plan.candidates.len())
        .map_err(|detail| stage_error("parse", HOOK, prepared, detail))?;
    Ok(ParsedOutput::Json(json!(accepted)))
}

pub fn commit(
    parsed: ParsedOutput,
    prepared: &PreparedTalent,
    pre: &PrePostState,
) -> Result<CommitPlan, StageError> {
    let state = state("commit", prepared, pre)?;
    let ParsedOutput::Json(Value::Array(accepted)) = parsed else {
        return Err(stage_error(
            "commit",
            HOOK,
            prepared,
            "expected the accepted candidates",
        ));
    };
    let accepted = accepted
        .iter()
        .map(|index| {
            index
                .as_u64()
                .and_then(|index| usize::try_from(index).ok())
                .filter(|index| *index < state.plan.candidates.len())
                .ok_or_else(|| stage_error("commit", HOOK, prepared, "unknown candidate"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(CommitPlan::Write(WriteIntent::PossibleNames {
        segment_dir: state.segment_dir.clone(),
        sidecar: sidecar(&state.plan, &accepted),
    }))
}

/// Whether a segment has the speech transcript possible names address, so
/// thinking dispatches the talent for it.
#[must_use]
pub fn has_speech_transcript(segment_dir: &Path) -> bool {
    speech_transcript(segment_dir).is_some()
}

/// Save a segment's possible names and queue them for search.
pub(crate) fn apply_result(
    journal: &Path,
    segment_dir: &Path,
    sidecar: &Sidecar,
) -> Result<(), String> {
    solstone_core_possible_names::write(segment_dir, sidecar).map_err(|error| error.to_string())?;
    if let Err(error) = solstone_core_indexer_store::todo::enqueue_path(
        journal,
        &solstone_core_possible_names::sidecar_path(segment_dir),
    ) {
        crate::writers::index_warning(&format!(
            "possible names search update could not be queued: {error}"
        ));
    }
    Ok(())
}

/// For evaluation only: what the talent would send for one segment, without
/// calling Generate or writing anything. The prompt is the installed talent's,
/// prepared as a run prepares it.
pub fn inspect_packet(
    segment_dir: &Path,
    paths: &crate::prepare::RuntimePaths,
) -> Result<Value, String> {
    let segment_dir = segment_dir
        .canonicalize()
        .map_err(|error| format!("{}: {error}", segment_dir.display()))?;
    let journal = segment_dir
        .ancestors()
        .find(|path| path.file_name().is_some_and(|name| name == "chronicle"))
        .and_then(Path::parent)
        .ok_or("the segment is not inside a journal's chronicle")?
        .to_path_buf();
    let origin = solstone_core_journal_io::segment_origin(&segment_dir);
    let parts = origin.split('/').collect::<Vec<_>>();
    let mut request = serde_json::Map::from_iter([("name".to_owned(), json!(TALENT))]);
    match parts.as_slice() {
        [day, segment] => {
            request.insert("day".to_owned(), json!(day));
            request.insert("segment".to_owned(), json!(segment));
        }
        [day, stream, segment] => {
            request.insert("day".to_owned(), json!(day));
            request.insert("stream".to_owned(), json!(stream));
            request.insert("segment".to_owned(), json!(segment));
        }
        _ => return Err(format!("{origin} is not a segment")),
    }
    let context = ExecutionContext {
        journal: journal.clone(),
    };
    let prepared = crate::prepare::prepare(
        request,
        paths,
        &context,
        crate::prepare::PrepareMode::Preview,
    )
    .map_err(|error| error.to_string())?;
    let system_instruction = system_instruction(&prepared).map_err(|error| error.detail)?;
    let Some(plan) = plan_segment(&journal, &segment_dir, &system_instruction)? else {
        return Ok(json!({"talent": TALENT, "decision": "no_transcript"}));
    };
    let decision = if plan.candidates.is_empty() {
        "no_candidates"
    } else {
        "generate"
    };
    let candidates = plan
        .candidates
        .iter()
        .enumerate()
        .map(|(index, candidate)| {
            json!({
                "id": packet::candidate_id(index),
                "name": plan.names[&candidate.entity_id],
                "basis": candidate.route.basis(),
                "action": candidate,
            })
        })
        .collect::<Vec<_>>();
    let pool = plan
        .pool
        .iter()
        .map(|action| json!({"dropped": action.drop_reason(), "action": action}))
        .collect::<Vec<_>>();
    Ok(json!({
        "talent": TALENT,
        "decision": decision,
        "fresh": plan.fresh,
        "segment_facets": plan.facets,
        "source": plan.source,
        "registry_revision": plan.registry_revision,
        "talent_version": plan.talent_version,
        "temperature": prepared.config.get("temperature"),
        "max_output_tokens": prepared.config.get("max_output_tokens"),
        "system_instruction": system_instruction,
        "packet": plan.packet,
        "json_schema": plan.schema,
        "candidates": candidates,
        "pool": pool,
    }))
}
