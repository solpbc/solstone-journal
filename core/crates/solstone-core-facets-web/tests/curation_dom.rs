// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::io::ErrorKind;
use std::process::Command;

use axum::body::{Body, to_bytes};
use axum::http::Request;
use serde_json::{Value, json};
use solstone_core_facets_web::Clock;
use tower::ServiceExt;

#[test]
fn curation_dom_contract() {
    match Command::new("node").arg("--version").output() {
        Err(error) if error.kind() == ErrorKind::NotFound => {
            panic!("curation DOM harness requires node")
        }
        Err(error) => panic!("node availability probe failed: {error}"),
        Ok(output) if !output.status.success() => panic!(
            "node availability probe failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        ),
        Ok(_) => {}
    }

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");

    let temp_journal = tempfile::tempdir().expect("temp journal");
    let journal_path = temp_journal.path();

    // 1. config/journal.json
    let config_dir = journal_path.join("config");
    fs::create_dir_all(&config_dir).expect("config dir");
    fs::write(
        config_dir.join("journal.json"),
        br#"{"setup":{"completed_at":1767225600}}"#,
    )
    .expect("journal.json");

    // 2. facets/review-candidates.jsonl
    let facets_dir = journal_path.join("facets");
    fs::create_dir_all(&facets_dir).expect("facets dir");
    let facet_names = [
        ("low_light_capture", "low_light_capture"),
        ("Low light capture", "low_light_capture_title"),
        ("日本", "nihon"),
        ("東京", "tokyo"),
        (" 日本 ", "nihon_spaced"),
    ];
    let facet_rows: Vec<String> = facet_names
        .iter()
        .map(|(name, key)| {
            json!({
                "name": name,
                "name_key": key,
                "status": "open",
                "count": 4,
                "window_days": 7,
                "evidence": {
                    "samples": [{
                        "day": "20260510",
                        "quote": format!("we should look at {name} again"),
                        "segment": "100000_300"
                    }]
                },
                "first_surfaced": "20260510",
                "last_surfaced": "20260510",
                "created_at": "2026-05-15T12:00:00Z",
                "updated_at": "2026-05-15T12:00:00Z"
            })
            .to_string()
        })
        .collect();
    fs::write(
        facets_dir.join("review-candidates.jsonl"),
        facet_rows.join("\n") + "\n",
    )
    .expect("facet candidates");

    // 3. entities/review-candidates.jsonl
    let entities_dir = journal_path.join("entities");
    fs::create_dir_all(&entities_dir).expect("entities dir");
    let entity_merge_rows = vec![
        json!({
            "facet": "work",
            "source": "Jordan Vancey",
            "source_slug": "jordan-vancey",
            "target": "Jordan Vance",
            "target_slug": "jordan-vance",
            "status": "open",
            "evidence": {
                "basis": "name-variant",
                "summary": "the two names appear in the same segment",
                "detection_count": 3,
                "needs": 2
            }
        })
        .to_string(),
        json!({
            "facet": "work",
            "source": "Jordan V",
            "source_slug": "jordan-v",
            "target": "Jordan Vance",
            "target_slug": "jordan-vance",
            "status": "open",
            "evidence": {
                "basis": "name-variant",
                "summary": "the two names appear in the same segment",
                "detection_count": 3,
                "needs": 2
            },
            "review": {
                "suppression": {
                    "reason": "stale",
                    "at": "2026-01-01T00:00:00Z",
                    "evidence_key": "k"
                }
            }
        })
        .to_string(),
    ];
    fs::write(
        entities_dir.join("review-candidates.jsonl"),
        entity_merge_rows.join("\n") + "\n",
    )
    .expect("entity review candidates");

    // 4. entities/ambiguities.jsonl
    let make_ambiguity = |normalized_query: &str,
                          candidates: &[(&str, &str)],
                          suppression: Option<Value>|
     -> String {
        let ambiguity_id =
            solstone_core_entity::ambiguity_id(&format!("facet:work|{normalized_query}"));
        let mut row = json!({
            "schema_version": 1,
            "ambiguity_id": ambiguity_id,
            "scope": { "kind": "facet", "facet": "work" },
            "normalized_query": normalized_query,
            "status": "open",
            "original_query": normalized_query,
            "latest_query": normalized_query,
            "first_seen": "2026-05-15T12:00:00Z",
            "last_seen": "2026-05-15T12:00:00Z",
            "observed_tier": 5,
            "occurrence_count": 1,
            "origins": [{
                "day": "20260510",
                "facet": "work",
                "field": "participation.name",
                "lane": "corpus.seed",
                "record_id": "100000_300"
            }],
            "origin_keys": ["{\"day\":\"20260510\",\"facet\":\"work\",\"field\":\"participation.name\",\"lane\":\"corpus.seed\",\"record_id\":\"100000_300\"}"],
            "resolved_entity_id": Value::Null,
            "resolved_at": Value::Null,
            "audit": { "prior_choices": [] },
            "ranked_candidates": candidates.iter().map(|(id, name)| json!({
                "id": id,
                "name": name,
                "score": 0.60,
                "tier": 5
            })).collect::<Vec<_>>()
        });
        if let Some(supp) = suppression {
            row["review"] = json!({ "suppression": supp });
        }
        row.to_string()
    };

    let ambiguity_rows = vec![
        make_ambiguity(
            "jordan",
            &[
                ("jordan-vance", "Jordan Vance"),
                ("jordan-vancey", "Jordan Vancey"),
            ],
            None,
        ),
        make_ambiguity(
            "jordy",
            &[
                ("jordan-vance", "Jordan Vance"),
                ("jordan-vancey", "Jordan Vancey"),
            ],
            None,
        ),
        make_ambiguity("taylor", &[("taylor-swift", "Taylor Swift")], None),
        make_ambiguity("alex", &[("alex-smith", "Alex Smith")], None),
        make_ambiguity(
            "casey",
            &[("casey-jones", "Casey Jones")],
            Some(json!({
                "reason": "stale",
                "at": "2026-01-01T00:00:00Z",
                "evidence_key": "k"
            })),
        ),
    ];
    fs::write(
        entities_dir.join("ambiguities.jsonl"),
        ambiguity_rows.join("\n") + "\n",
    )
    .expect("ambiguities");

    // 5. speakers/review-candidates.jsonl
    let speakers_dir = journal_path.join("speakers");
    fs::create_dir_all(&speakers_dir).expect("speakers dir");
    let speaker_rows = vec![
        json!({
            "source_id": "spk_jordan_v",
            "source_label": "Jordan V",
            "target_id": "spk_jordan_vance",
            "target_label": "Jordan Vance",
            "status": "open",
            "similarity": 0.86,
            "readiness": "ready",
            "evidence": {
                "basis": "speaker-name-variant",
                "summary": "Jordan V and Jordan Vance have matching speaker voiceprints.",
                "detection_count": 1,
                "readiness": "ready"
            }
        })
        .to_string(),
    ];
    fs::write(
        speakers_dir.join("review-candidates.jsonl"),
        speaker_rows.join("\n") + "\n",
    )
    .expect("speaker review candidates");

    // 6. speakers/candidate-pair-review-candidates.jsonl
    let pair_rows = vec![
        json!({
            "key": "[\"anchor_a\",\"anchor_b\"]",
            "anchor_a": "anchor_a",
            "anchor_b": "anchor_b",
            "status": "open",
            "similarity": 0.74,
            "evidence": {
                "basis": "speaker-candidate-pair",
                "similarity": 0.74,
                "source_intervals": 4,
                "source_samples": [{"day": "20260510", "segment": "100000_300"}],
                "target_intervals": 3,
                "target_samples": [{"day": "20260510", "segment": "103000_300"}]
            }
        })
        .to_string(),
        json!({
            "key": "[\"anchor_c\",\"anchor_d\"]",
            "anchor_a": "anchor_c",
            "anchor_b": "anchor_d",
            "status": "open",
            "similarity": 0.75,
            "evidence": {
                "basis": "speaker-candidate-pair",
                "similarity": 0.75,
                "source_intervals": 4,
                "source_samples": [{"day": "20260510", "segment": "100000_300"}],
                "target_intervals": 3,
                "target_samples": [{"day": "20260510", "segment": "103000_300"}]
            }
        })
        .to_string(),
        json!({
            "key": "[\"anchor_e\",\"anchor_f\"]",
            "anchor_a": "anchor_e",
            "anchor_b": "anchor_f",
            "status": "open",
            "similarity": 0.76,
            "evidence": {
                "basis": "speaker-candidate-pair",
                "similarity": 0.76,
                "source_intervals": 4,
                "source_samples": [{"day": "20260510", "segment": "100000_300"}],
                "target_intervals": 3,
                "target_samples": [{"day": "20260510", "segment": "103000_300"}]
            }
        })
        .to_string(),
    ];
    fs::write(
        speakers_dir.join("candidate-pair-review-candidates.jsonl"),
        pair_rows.join("\n") + "\n",
    )
    .expect("pair candidates");

    let (state_body, summary_body) = rt.block_on(async {
        let app = solstone_core_facets_web::routes(
            journal_path.to_path_buf(),
            Clock::owner(journal_path.to_path_buf()),
        );

        let state_res = app
            .clone()
            .oneshot(
                Request::get("/app/curation/api/state?limit=2")
                    .body(Body::empty())
                    .expect("state request"),
            )
            .await
            .expect("state response");
        assert_eq!(state_res.status(), axum::http::StatusCode::OK);
        let state_bytes = to_bytes(state_res.into_body(), usize::MAX)
            .await
            .expect("state bytes");

        let summary_res = app
            .oneshot(
                Request::get("/app/curation/api/summary")
                    .body(Body::empty())
                    .expect("summary request"),
            )
            .await
            .expect("summary response");
        assert_eq!(summary_res.status(), axum::http::StatusCode::OK);
        let summary_bytes = to_bytes(summary_res.into_body(), usize::MAX)
            .await
            .expect("summary bytes");

        (state_bytes, summary_bytes)
    });

    let temp_files = tempfile::tempdir().expect("temp files");
    let state_file = temp_files.path().join("state.json");
    let summary_file = temp_files.path().join("summary.json");
    fs::write(&state_file, &state_body).expect("write state file");
    fs::write(&summary_file, &summary_body).expect("write summary file");

    let output = Command::new("node")
        .arg(format!(
            "{}/tests/curation_dom.js",
            env!("CARGO_MANIFEST_DIR")
        ))
        .arg(env!("CARGO_MANIFEST_DIR"))
        .arg(&state_file)
        .arg(&summary_file)
        .output()
        .expect("curation DOM harness starts");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "curation DOM harness failed:\nstdout:\n{}\nstderr:\n{}",
        stdout,
        String::from_utf8_lossy(&output.stderr),
    );
    assert!(
        stdout.starts_with("DOM CASES: ") && stdout.contains(" passed"),
        "curation DOM harness did not report its internal case count:\n{stdout}"
    );
    println!("{}", stdout.trim());
}
