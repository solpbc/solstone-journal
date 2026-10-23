// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native history regression. Only Generate replies are synthetic; import,
//! Cortex use files, activity publication, provenance and sealing use owners.
use super::*;
use crate::context::{CortexBoundary, DispatchFailure};
use solstone_core_cortex::{CallableCortex, CallableRoots};
use solstone_core_cortex_client::{
    CortexRequest, CortexRequestClient, CortexRequestPolicy, WaitForUsesReport,
};
use solstone_core_generate::{GenerateRequest, GenerateResponse, GeneratedResponse};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

const DAY: &str = "20261016";

struct LocalIndex;
impl crate::context::IndexBoundary for LocalIndex {
    fn enqueue_path(&self, journal: &Path, path: &Path) -> Result<(), String> {
        solstone_core_indexer_store::todo::enqueue_path(journal, path).map_err(|e| e.to_string())
    }
    fn drain_pending(
        &self,
        journal: &Path,
        force: bool,
        budget: Duration,
        checkpoint: &dyn Fn() -> Result<(), String>,
    ) -> Result<(), String> {
        solstone_core_indexer_store::todo::drain_with_checkpoint(
            journal,
            solstone_core_indexer_store::todo::DrainOptions { force, budget },
            checkpoint,
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }
}

struct HistoryCortex {
    root: PathBuf,
    payload: PathBuf,
    owner: CallableCortex,
    reading: AtomicBool,
    edit_during_generate: AtomicUsize,
    calls: Mutex<Vec<(String, String)>>,
}

fn generated(request: &GenerateRequest, text: Value) -> GenerateResponse {
    GenerateResponse::Generated(Box::new(GeneratedResponse {
        id: request.id.clone(),
        text: text.to_string(),
        model: "declared-synthetic".into(),
        usage: serde_json::json!({}),
        finish_reason: "stop".into(),
        thinking: None,
        schema_validation: None,
        input_budget: None,
        request_budget: None,
        inference: None,
        hints_applied: vec![],
    }))
}

impl CortexBoundary for HistoryCortex {
    fn dispatch(
        &self,
        rt: &tokio::runtime::Runtime,
        request: &CortexRequest,
    ) -> Result<String, DispatchFailure> {
        self.dispatch_prepared(rt, request, None, &mut |_| Ok(()))
    }

    fn dispatch_prepared(
        &self,
        _: &tokio::runtime::Runtime,
        request: &CortexRequest,
        reserved: Option<&str>,
        prepare: &mut (dyn FnMut(&str) -> std::io::Result<()> + Send),
    ) -> Result<String, DispatchFailure> {
        let now = Utc::now().timestamp_millis();
        let id = reserved.map(str::to_owned).unwrap_or_else(|| {
            solstone_core_journal_io::cortex_use::allocate_cortex_use_id(&self.root, now)
                .unwrap()
                .to_string()
        });
        prepare(&id).map_err(|_| DispatchFailure::Unavailable)?;
        self.calls
            .lock()
            .unwrap()
            .push((request.name.clone(), id.clone()));
        let mut config = request.config.clone();
        config.insert("name".into(), Value::from(request.name.clone()));
        config.insert("use_id".into(), Value::from(id));
        config.insert("prompt".into(), Value::from(request.prompt.clone()));
        let generate = |req: &GenerateRequest| {
            let target = match request.name.as_str() {
                "work" => 1,
                "participation" => 2,
                _ => 0,
            };
            if target != 0
                && self
                    .edit_during_generate
                    .compare_exchange(target, 0, Ordering::SeqCst, Ordering::SeqCst)
                    .is_ok()
            {
                let patch = if target == 1 {
                    serde_json::json!({"story":{"talent":"work","body":"Owner story entered during Generate.","topics":[],"confidence":1.0}})
                } else {
                    serde_json::json!({"participation":[{"name":"Manual Test Person","role":"mentioned","source":"other","confidence":1.0,"context":"Declared owner fixture edit","entity_id":null}]})
                };
                solstone_core_facets::update_activity_record(
                    &self.root,
                    "home",
                    DAY,
                    request.config["activity"]["id"].as_str().unwrap(),
                    patch.as_object().unwrap(),
                    "owner:regression",
                    "Edit during Generate",
                    "2026-10-16T12:01:00Z",
                )
                .unwrap()
                .unwrap();
            }
            let reply = match request.name.as_str() {
                "sense" => serde_json::json!({"density":"active",
                    "content_type": if self.reading.load(Ordering::SeqCst) {"reading"} else {"writing"},
                    "activity_summary":"A declared synthetic garden note.",
                    "entities":[],"facets":[{"facet":"home","activity":"A synthetic garden note.","level":"high"}],
                    "speculative_facet":null,"meeting_detected":false,"speakers":[],
                    "recommend":{"screen_record":false,"speaker_attribution":false},
                    "emotional_register":"neutral","topics":["garden"],"quotes":[]}),
                "participation" => {
                    serde_json::json!({"participation":[],"participation_confidence":null})
                }
                "work" => serde_json::json!({"body":"You considered a synthetic garden note.",
                    "topics":["garden"],"confidence":0.8,"relations":[]}),
                "entities:detection" => serde_json::json!({"detections":[]}),
                "daily_schedule" => serde_json::json!({"primary":"03:00","fallback":"04:00"}),
                "morning_briefing" => {
                    let identity = solstone_core_journal_io::DailyUnitIdentity::new(
                        DAY,
                        "morning_briefing",
                        None,
                    );
                    let owner =
                        solstone_core_journal_io::observe_daily_unit_record(&self.root, &identity)
                            .unwrap()
                            .unwrap();
                    let metadata:Value = serde_json::from_str(owner.frozen_packet.as_ref().unwrap()["prepared"]["config"]["briefing_metadata"].as_str().unwrap()).unwrap();
                    serde_json::json!({"metadata":metadata,"yesterday":[],"needs_attention":[]})
                }
                "entities:entities_review" => serde_json::json!({"promotions":[],"merges":[]}),
                "entities:entity_suggest" | "entities:entity_observer" => {
                    serde_json::json!({"entities":[]})
                }
                other => panic!("unexpected synthetic Generate task: {other}"),
            };
            Ok(generated(req, reply))
        };
        let outcome = self
            .owner
            .execute(
                CallableRoots {
                    talent_root: &self.payload.join("talent"),
                    apps_root: &self.payload.join("apps"),
                    templates_dir: &self.payload.join("think/templates"),
                },
                config,
                &generate,
                now,
                &|| false,
            )
            .map_err(|error| {
                eprintln!("native history Cortex: {error}");
                DispatchFailure::Unavailable
            })?;
        assert!(outcome.completed);
        Ok(outcome.use_id)
    }

    fn wait(
        &self,
        _: &tokio::runtime::Runtime,
        ids: &[String],
        _: Option<Duration>,
    ) -> Result<WaitForUsesReport, String> {
        CortexRequestClient::new(&self.root, CortexRequestPolicy::think())
            .wait_for_uses_from_disk(ids)
            .map_err(|e| format!("{e:?}"))
    }
}

struct ImportWire;
impl solstone_core_import::WireClient for ImportWire {
    fn execute(
        &self,
        request: &GenerateRequest,
    ) -> Result<GenerateResponse, solstone_core_generate::ClientError> {
        Ok(generated(
            request,
            serde_json::json!({"topics":"garden","setting":"synthetic local regression"}),
        ))
    }
}

#[test]
fn retained_writing_to_reading_history_seals_only_after_owning_work() {
    history_qualification(None);
}

#[test]
fn owner_edit_during_generate_blocks_publication_and_due_retry() {
    for field in ["story", "participation"] {
        history_qualification(Some(field));
    }
}

fn history_qualification(held_output: Option<&'static str>) {
    let disposable = tempfile::tempdir().unwrap();
    // A caller may retain the final root and every owner artifact for review.
    let mut root = std::env::var_os("RETAINED_ACTIVITY_PROOF_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| disposable.path().join("journal"));
    if let Some(field) = held_output {
        root = root.join(field);
    }
    assert!(!root.exists(), "proof root must be unused");
    std::fs::create_dir_all(&root).unwrap();
    let payload = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../payload/solstone");
    solstone_core_journal_config_write::mutate_journal_config(
        &root,
        solstone_core_journal_io::LockOptions::default(),
        |config| {
            config.insert(
                "providers".into(),
                serde_json::json!({"active":{"provider":"test","model":"test-model"}}),
            );
            solstone_core_journal_config_write::JournalConfigMutation {
                changed: true,
                value: (),
            }
        },
    )
    .unwrap();
    solstone_core_facets::create_facet(
        &root,
        "home",
        "Home",
        "Synthetic local regression",
        "#123456",
        "",
        None,
    )
    .unwrap();
    let source = root.parent().unwrap().join("synthetic-garden-note.txt");
    std::fs::write(&source,"This declared synthetic garden note discusses basil seeds and a blue notebook. It is local test input for native activity history reconciliation, with no assertion about a real person's activity.\n").unwrap();
    let imported = solstone_core_import::process_transcript_with_wire(
        &source,
        &root.join("chronicle").join(DAY),
        "09:00:00",
        "history-regression",
        "import.text",
        Some("home"),
        None,
        &ImportWire,
        None,
    );
    assert!(
        matches!(
            imported,
            solstone_core_import::TextImportOutcome::Success(_)
        ),
        "{imported:?}"
    );
    assert_eq!(imported.created().len(), 1);
    let input = &imported.created()[0];
    let raw_transcript = root
        .join("chronicle")
        .join(DAY)
        .join(&input.stream)
        .join(&input.segment)
        .join("conversation_transcript.jsonl");
    let raw_bytes = std::fs::read(&raw_transcript).unwrap();
    let cortex = Arc::new(HistoryCortex {
        root: root.clone(),
        payload: payload.clone(),
        owner: CallableCortex::open(&root).unwrap(),
        reading: AtomicBool::new(false),
        edit_during_generate: AtomicUsize::new(0),
        calls: Mutex::new(vec![]),
    });
    let mut context = ThinkContext::hosted(
        &root,
        DAY.into(),
        root.join("chronicle").join(DAY),
        Utc::now().timestamp_millis(),
        payload.join("talent"),
        payload.join("apps"),
        cortex.clone(),
        Arc::new(LocalIndex),
        Arc::new(|| Utc::now().timestamp_millis()),
        None,
        Arc::new(|| false),
    );
    let mut log = RunLogWriter::open(&root, DAY, "history-regression");
    let first = segment::run(
        &context,
        &mut log,
        &input.segment,
        false,
        Some(&input.stream),
        1,
        Some(Duration::from_secs(90)),
        false,
        &[],
    )
    .unwrap();
    assert_eq!(first.failed, 0, "{first:?}");
    segment::replay_activity_state(
        &context,
        &mut log,
        &[(input.segment.clone(), Some(input.stream.clone()))],
        false,
        1,
        false,
        false,
    )
    .unwrap();
    let before = solstone_core_facets::read_activity_records_strict(&root, "home", DAY).unwrap();
    let writing = before
        .iter()
        .find(|r| r["activity"] == "writing")
        .expect("native writing row");
    let writing_id = writing["id"].as_str().unwrap().to_owned();
    solstone_core_facets::update_activity_record(
        &root,
        "home",
        DAY,
        &writing_id,
        &Map::from_iter([("title".into(), Value::from("Owner garden title"))]),
        "owner:regression",
        "Keep this title",
        "2026-10-16T12:00:00Z",
    )
    .unwrap()
    .unwrap();
    let before = solstone_core_facets::read_activity_records_strict(&root, "home", DAY).unwrap();
    let provenance = segment::activity_provenance_path(&context, DAY, "home", &writing_id);
    let old_receipt = std::fs::read(&provenance).unwrap();
    assert!(crate::activity_work::rebuild_receipts(&context, DAY).is_ok());
    let source_bytes = std::fs::read(&source).unwrap();
    if let Some(field) = held_output {
        cortex.reading.store(true, Ordering::SeqCst);
        assert_eq!(
            segment::run(
                &context,
                &mut log,
                &input.segment,
                true,
                Some(&input.stream),
                1,
                Some(Duration::from_secs(90)),
                false,
                &[]
            )
            .unwrap()
            .failed,
            0
        );
        cortex
            .edit_during_generate
            .store(if field == "story" { 1 } else { 2 }, Ordering::SeqCst);
        let failed = crate::activity_work::reconcile_retained(&context, &mut log, 1);
        std::fs::write(root.parent().unwrap().join(format!("owner-{field}-publication-observed.json")),
            serde_json::to_vec_pretty(&serde_json::json!({"outcome":format!("{failed:?}"),
                "rows":solstone_core_facets::read_activity_records_strict(&root,"home",DAY).unwrap(),
                "calls":*cortex.calls.lock().unwrap()})).unwrap()).unwrap();
        assert!(failed.is_err(), "native owner publication must refuse");
        assert_eq!(
            cortex.edit_during_generate.load(Ordering::SeqCst),
            0,
            "must reach real Generate callback"
        );
        let row = solstone_core_facets::get_activity_record(&root, "home", DAY, &writing_id)
            .unwrap()
            .unwrap();
        assert!(solstone_core_facets::activity_field_held(&row, field));
        let retained = row[field].clone();
        let history = row["edits"].clone();
        assert_eq!(std::fs::read(&provenance).unwrap(), old_receipt);
        assert!(crate::activity_work::rebuild_receipts(&context, DAY).is_err());
        let calls = cortex.calls.lock().unwrap().clone();
        let resumed = context
            .clone()
            .with_event_clock(Arc::new(|| Utc::now().timestamp_millis() + 3_600_000));
        let blocked =
            crate::activity::run(&resumed, &mut log, &writing_id, "home", false, false, 1).unwrap();
        assert!(blocked.failed > 0);
        assert_eq!(
            *cortex.calls.lock().unwrap(),
            calls,
            "due retry must not Generate while owner output is held"
        );
        let work = pending_record(&root, &writing_id).unwrap();
        assert_eq!(work["blocked"]["reason_code"], "owner_field_held");
        let after = solstone_core_facets::get_activity_record(&root, "home", DAY, &writing_id)
            .unwrap()
            .unwrap();
        assert_eq!(after[field], retained);
        assert_eq!(after["edits"], history);
        assert_eq!(std::fs::read(provenance).unwrap(), old_receipt);
        assert_eq!(std::fs::read(raw_transcript).unwrap(), raw_bytes);
        assert_eq!(std::fs::read(source).unwrap(), source_bytes);
        assert!(
            solstone_core_system::work_intent::read(&root)
                .unwrap()
                .is_empty()
        );
        log.finish().unwrap();
        std::fs::write(root.parent().unwrap().join(format!("owner-{field}-hold-result.json")),serde_json::to_vec_pretty(&serde_json::json!({"failed":format!("{failed:?}"),"blocked":format!("{blocked:?}"),"work":work,"retained":retained,"calls":calls,"after":after,"no_second_dispatch":true,"provenance_unchanged":true})).unwrap()).unwrap();
        return;
    }
    let request = solstone_core_system::work_intent::register(&root, DAY, true).unwrap();
    let binding = solstone_core_journal_io::JournalRoot::open(&root)
        .unwrap()
        .identity()
        .observation_id();
    context.rebuild = Some(Arc::new(
        solstone_core_system::work_intent::begin_rebuild(&root, DAY, &request.id, &binding)
            .unwrap(),
    ));
    cortex.reading.store(true, Ordering::SeqCst);
    let calls_before = cortex.calls.lock().unwrap().len();
    let args = ThinkArgs {
        day: Some(DAY.into()),
        refresh: true,
        ..ThinkArgs::default()
    };
    let outcome = run(
        &context,
        &mut log,
        &args,
        1,
        Some(Duration::from_secs(90)),
        &solstone_core_system::process::ChildLaunchContext::default(),
    )
    .unwrap();
    log.finish().unwrap();
    let intent = solstone_core_system::work_intent::read(&root)
        .unwrap()
        .remove(DAY)
        .unwrap();
    let after = solstone_core_facets::read_activity_records_strict(&root, "home", DAY).unwrap();
    let proof = serde_json::json!({"synthetic_generate":true,"root":root,"before":before,"after":after,
        "old_receipt":serde_json::from_slice::<Value>(&old_receipt).unwrap(),
        "current_receipt":serde_json::from_slice::<Value>(&std::fs::read(&provenance).unwrap()).unwrap(),
        "calls":*cortex.calls.lock().unwrap(),"calls_before":calls_before,"intent":intent,"outcome":format!("{outcome:?}")});
    std::fs::write(
        root.parent().unwrap().join("history-result.json"),
        serde_json::to_vec_pretty(&proof).unwrap(),
    )
    .unwrap();
    assert_eq!(std::fs::read(&source).unwrap(), source_bytes);
    assert_eq!(std::fs::read(&raw_transcript).unwrap(), raw_bytes);
    assert!(after.iter().any(|r| r["id"] == writing_id));
    let old_writing = before.iter().find(|r| r["id"] == writing_id).unwrap();
    let new_writing = after.iter().find(|r| r["id"] == writing_id).unwrap();
    assert_eq!(new_writing["title"], old_writing["title"]);
    assert_eq!(new_writing["segments"], old_writing["segments"]);
    assert!(
        new_writing["edits"]
            .as_array()
            .unwrap()
            .starts_with(old_writing["edits"].as_array().unwrap())
    );
    assert!(after.iter().any(|r| r["activity"] == "reading"));
    assert!(
        intent.rebuild.as_ref().unwrap().segments.is_some(),
        "native transition must seal: {outcome:?}"
    );
    assert!(crate::activity_work::rebuild_receipts(&context, DAY).is_ok());
    assert_ne!(
        std::fs::read(provenance).unwrap(),
        old_receipt,
        "owning work must replace stale input provenance"
    );
    let calls = cortex.calls.lock().unwrap().clone();
    let path = segment::activity_provenance_path(&context, DAY, "home", &writing_id);
    let current_receipt = std::fs::read(&path).unwrap();
    crate::activity_work::reconcile_retained(&context, &mut log, 1).unwrap();
    assert_eq!(*cortex.calls.lock().unwrap(), calls);
    assert_eq!(std::fs::read(path).unwrap(), current_receipt);
    context.rebuild = None;
    for (frontier, reading) in [("accepted", false), ("empty", true), ("published", false)] {
        exercise_owner_interruption(
            &context,
            &cortex,
            &writing_id,
            &input.segment,
            &input.stream,
            frontier,
            reading,
        );
    }
    exercise_source_drift(
        &context,
        &cortex,
        &writing_id,
        &input.segment,
        &input.stream,
    );
    // Cancel after real activity work has drained and published, but before
    // its enclosing request can store a segment seal.
    let prior_sense = std::fs::read(
        context
            .day_dir
            .join(&input.stream)
            .join(&input.segment)
            .join("talents/sense.json"),
    )
    .unwrap();
    cortex.reading.store(false, Ordering::SeqCst);
    begin_request(&mut context);
    let probe = context.clone();
    let flag = Arc::new(AtomicBool::new(false));
    let reached = flag.clone();
    let sense = context
        .day_dir
        .join(&input.stream)
        .join(&input.segment)
        .join("talents/sense.json");
    context.cancel_checkpoint = Arc::new(move || {
        if reached.load(Ordering::SeqCst) {
            return true;
        }
        let changed = std::fs::read(&sense).is_ok_and(|bytes| bytes != prior_sense);
        if changed && crate::activity_work::rebuild_receipts(&probe, DAY).is_ok() {
            reached.store(true, Ordering::SeqCst);
            return true;
        }
        false
    });
    let stopped = run(
        &context,
        &mut log,
        &args,
        1,
        Some(Duration::from_secs(90)),
        &solstone_core_system::process::ChildLaunchContext::default(),
    )
    .unwrap();
    assert!(flag.load(Ordering::SeqCst));
    assert!(stopped.failed > 0);
    let held = solstone_core_system::work_intent::read(&root)
        .unwrap()
        .remove(DAY)
        .unwrap();
    assert!(held.rebuild.as_ref().unwrap().segments.is_none());
    std::fs::write(
        root.parent().unwrap().join("cancel-before-seal.json"),
        serde_json::to_vec_pretty(
            &serde_json::json!({"outcome":format!("{stopped:?}"),"intent":held}),
        )
        .unwrap(),
    )
    .unwrap();
    context.cancel_checkpoint = Arc::new(|| false);
    context.rebuild = None;
    // A new operation can qualify the retained history after the ordinary
    // owner retries, using the full pipeline rather than a fabricated seal.
    begin_request(&mut context);
    let final_outcome = run(
        &context,
        &mut log,
        &args,
        1,
        Some(Duration::from_secs(90)),
        &solstone_core_system::process::ChildLaunchContext::default(),
    )
    .unwrap();
    assert_eq!(final_outcome.failed, 0, "{final_outcome:?}");
    context.rebuild_admission_checkpoint().unwrap();
    log.finish().unwrap();
    let final_intent = solstone_core_system::work_intent::read(&root)
        .unwrap()
        .remove(DAY)
        .unwrap();
    assert!(final_intent.rebuild.as_ref().unwrap().segments.is_some());
    assert_eq!(std::fs::read(&source).unwrap(), source_bytes);
    assert_eq!(std::fs::read(&raw_transcript).unwrap(), raw_bytes);
    let final_rows =
        solstone_core_facets::read_activity_records_strict(&root, "home", DAY).unwrap();
    assert_eq!(
        final_rows.iter().find(|r| r["id"] == writing_id).unwrap()["title"],
        old_writing["title"]
    );
    std::fs::write(root.parent().unwrap().join("final-sealed-result.json"),serde_json::to_vec_pretty(&serde_json::json!({"outcome":format!("{final_outcome:?}"),"intent":final_intent,"rows":final_rows,"calls":*cortex.calls.lock().unwrap(),"strict_admission_valid":true})).unwrap()).unwrap();
}

fn begin_request(context: &mut ThinkContext) {
    let request = solstone_core_system::work_intent::register(&context.journal, DAY, true).unwrap();
    let binding = solstone_core_journal_io::JournalRoot::open(&context.journal)
        .unwrap()
        .identity()
        .observation_id();
    context.rebuild = Some(Arc::new(
        solstone_core_system::work_intent::begin_rebuild(
            &context.journal,
            DAY,
            &request.id,
            &binding,
        )
        .unwrap(),
    ));
}

fn exercise_source_drift(
    base: &ThinkContext,
    cortex: &Arc<HistoryCortex>,
    activity: &str,
    segment_key: &str,
    stream: &str,
) {
    let mut context = base.clone();
    let mut log = RunLogWriter::open(&base.journal, DAY, "source-drift");
    cortex.reading.store(true, Ordering::SeqCst);
    assert_eq!(
        segment::run(
            &context,
            &mut log,
            segment_key,
            true,
            Some(stream),
            1,
            Some(Duration::from_secs(90)),
            false,
            &[]
        )
        .unwrap()
        .failed,
        0
    );
    let root = context.journal.clone();
    let name = activity.to_owned();
    let sense = context
        .day_dir
        .join(stream)
        .join(segment_key)
        .join("talents/sense.json");
    let changed = Arc::new(AtomicBool::new(false));
    let flag = changed.clone();
    context.cancel_checkpoint = Arc::new(move || {
        if !flag.load(Ordering::SeqCst)
            && pending_record(&root, &name)
                .is_some_and(|r| r["remaining"].as_array().is_some_and(Vec::is_empty))
        {
            // Explicit negative fixture mutation: source bytes change after
            // accepted work, while remaining valid Sense JSON.
            let mut bytes = std::fs::read(&sense).unwrap();
            bytes.push(b'\n');
            solstone_core_journal_io::atomic_replace(
                &sense,
                &bytes,
                solstone_core_journal_io::AtomicWriteOptions::default(),
            )
            .unwrap();
            flag.store(true, Ordering::SeqCst);
        }
        false
    });
    let result = crate::activity_work::reconcile_retained(&context, &mut log, 1);
    assert!(changed.load(Ordering::SeqCst));
    assert_eq!(
        result.unwrap_err(),
        "retained activity input changed during work"
    );
    assert!(crate::activity_work::rebuild_receipts(base, DAY).is_err());
    std::fs::write(base.journal.parent().unwrap().join("source-drift-result.json"),serde_json::to_vec_pretty(&serde_json::json!({"fixture_source_mutation":"one LF after owning work before provenance cleanup","strict_receipts_refused":true})).unwrap()).unwrap();
}

fn pending_record(root: &Path, activity: &str) -> Option<Value> {
    std::fs::read_dir(root.join("health/activity-work"))
        .ok()?
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().and_then(|e| e.to_str()) == Some("json"))
        .filter_map(|e| serde_json::from_slice::<Value>(&std::fs::read(e.path()).ok()?).ok())
        .find(|r| r["identity"]["activity"] == activity)
}

fn exercise_owner_interruption(
    base: &ThinkContext,
    cortex: &Arc<HistoryCortex>,
    activity: &str,
    segment_key: &str,
    stream: &str,
    frontier: &'static str,
    reading: bool,
) {
    let mut context = base.clone();
    let mut log = RunLogWriter::open(&context.journal, DAY, "history-interruption");
    cortex.reading.store(reading, Ordering::SeqCst);
    let output = segment::run(
        &context,
        &mut log,
        segment_key,
        true,
        Some(stream),
        1,
        Some(Duration::from_secs(90)),
        false,
        &[],
    )
    .unwrap();
    assert_eq!(output.failed, 0);
    let provenance = segment::activity_provenance_path(&context, DAY, "home", activity);
    let old_provenance = std::fs::read(&provenance).unwrap();
    let record = solstone_core_facets::get_activity_record(&context.journal, "home", DAY, activity)
        .unwrap()
        .unwrap();
    let input_hash = segment::compute_activity_input_hash(&context, DAY, &record).unwrap();
    assert_ne!(
        serde_json::from_slice::<Value>(&old_provenance).unwrap()["input_hash"],
        input_hash
    );
    let fired = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&fired);
    let root = context.journal.clone();
    let name = activity.to_owned();
    let prior = old_provenance.clone();
    let receipt = provenance.clone();
    context.cancel_checkpoint = Arc::new(move || {
        if flag.load(Ordering::SeqCst) {
            return true;
        }
        let Some(work) = pending_record(&root, &name) else {
            return false;
        };
        let empty = work["remaining"].as_array().is_some_and(Vec::is_empty);
        let published = std::fs::read(&receipt).is_ok_and(|bytes| bytes != prior);
        let completed = work["uses"].as_object().is_some_and(|uses| {
            uses.values().any(|id| {
                id.as_str().is_some_and(|id| {
                    solstone_core_cortex_client::use_file_status(&root.join("talents"), id)
                        .is_ok_and(|status| {
                            status == solstone_core_cortex_client::UseFileStatus::Completed
                        })
                })
            })
        });
        let reached = match frontier {
            "accepted" => !empty && completed,
            "empty" => empty && !published,
            "published" => empty && published,
            _ => unreachable!(),
        };
        if reached {
            flag.store(true, Ordering::SeqCst);
        }
        reached
    });
    let start = cortex.calls.lock().unwrap().len();
    let held = crate::activity_work::claim_drained(&context, "home", activity).unwrap();
    let stopped = crate::activity::reconcile_retained(
        &context,
        &mut log,
        activity,
        "home",
        &input_hash,
        1,
        held,
    );
    assert!(
        fired.load(Ordering::SeqCst),
        "must reach actual {frontier} frontier: {stopped:?}"
    );
    assert!(
        !stopped.as_ref().is_ok_and(|r| r.failed == 0),
        "{frontier}: {stopped:?}"
    );
    let work = pending_record(&context.journal, activity).expect("real interrupted owner record");
    assert!(crate::activity_work::rebuild_receipts(&context, DAY).is_err());
    let accepted = cortex.calls.lock().unwrap()[start..].to_vec();
    // Reconciliation refuses the retained partial work; the ordinary owner
    // resumes its durable uses and owes only units not accepted before Stop.
    let resumed = base
        .clone()
        .with_event_clock(Arc::new(|| Utc::now().timestamp_millis() + 3_600_000));
    assert!(crate::activity_work::reconcile_retained(&resumed, &mut log, 1).is_err());
    assert_eq!(cortex.calls.lock().unwrap()[start..], accepted);
    let result =
        crate::activity::run(&resumed, &mut log, activity, "home", false, false, 1).unwrap();
    assert_eq!(result.failed, 0, "{frontier}: {result:?}");
    assert!(pending_record(&context.journal, activity).is_none());
    let after = cortex.calls.lock().unwrap()[start..].to_vec();
    for (_, id) in &accepted {
        assert_eq!(
            after.iter().filter(|(_, new)| new == id).count(),
            1,
            "accepted use must not replay: {frontier}"
        );
    }
    if frontier != "accepted" {
        assert_eq!(after, accepted, "cleanup cannot Generate: {frontier}");
    }
    crate::activity_work::reconcile_retained(&resumed, &mut log, 1).unwrap();
    assert!(rebuild_segment_receipts(&resumed, None).is_ok());
    std::fs::write(base.journal.parent().unwrap().join(format!("frontier-{frontier}.json")),
        serde_json::to_vec_pretty(&serde_json::json!({"frontier":frontier,"stopped":format!("{stopped:?}"),
        "interrupted_work":work,"accepted_calls":accepted,"after_retry_calls":after,
        "after_receipt":serde_json::from_slice::<Value>(&std::fs::read(provenance).unwrap()).unwrap(),
        "strict_receipts_valid":true})).unwrap()).unwrap();
}
