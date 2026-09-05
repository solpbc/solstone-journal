// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::Path;

use chrono::NaiveDateTime;
use tempfile::TempDir;

use crate::Clock;

pub fn corpus() -> serde_json::Value {
    serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/convey_facets_corpus.json"
    )))
    .expect("facets corpus")
}

pub fn fixed_clock() -> Clock {
    Clock::new(|| {
        NaiveDateTime::parse_from_str("2026-05-15T12:00:00", "%Y-%m-%dT%H:%M:%S")
            .expect("fixed clock")
    })
}

pub fn later_clock() -> Clock {
    Clock::new(|| {
        NaiveDateTime::parse_from_str("2026-06-15T12:00:00", "%Y-%m-%dT%H:%M:%S")
            .expect("fixed clock")
    })
}

pub fn phase_root(phase: &str) -> TempDir {
    let root = TempDir::new().expect("temporary journal");
    match phase {
        "unestablished" => {}
        "corrupt" => write(
            &root.path().join("config/journal.json"),
            "{\"setup\": {\"completed_at\": 17672256",
        ),
        "established_empty" => config(root.path()),
        "populated" => {
            config(root.path());
            chronicle(root.path());
            news(root.path());
            curation(root.path());
            awareness(root.path());
            activities(root.path());
        }
        _ => panic!("known phase: {phase}"),
    }
    root
}

fn curation(root: &Path) {
    write(
        &root.join("facets/review-candidates.jsonl"),
        "{\"name\":\"Atlas\",\"name_key\":\"atlas\",\"status\":\"open\",\"count\":4,\"window_days\":7,\"evidence\":{\"samples\":[{\"day\":\"20260510\",\"quote\":\"we should look at Atlas again\",\"segment\":\"100000_300\"}]},\"first_surfaced\":\"20260510\",\"last_surfaced\":\"20260510\",\"created_at\":\"2026-05-15T12:00:00Z\",\"updated_at\":\"2026-05-15T12:00:00Z\"}\n{\"name\":\"Ledger\",\"name_key\":\"ledger\",\"status\":\"open\",\"count\":2,\"window_days\":7,\"evidence\":{\"samples\":[{\"day\":\"20260510\",\"quote\":\"we should look at Ledger again\",\"segment\":\"100000_300\"}]},\"first_surfaced\":\"20260510\",\"last_surfaced\":\"20260510\",\"created_at\":\"2026-05-15T12:00:00Z\",\"updated_at\":\"2026-05-15T12:00:00Z\"}\n",
    );
    write(
        &root.join("entities/review-candidates.jsonl"),
        "{\"facet\":\"work\",\"source\":\"Jordan Vancey\",\"source_slug\":\"jordan-vancey\",\"target\":\"Jordan Vance\",\"target_slug\":\"jordan-vance\",\"status\":\"open\",\"evidence\":{\"basis\":\"name-variant\",\"summary\":\"the two names appear in the same segment\",\"detection_count\":3,\"needs\":2}}\n",
    );
    write(
        &root.join("entities/ambiguities.jsonl"),
        "{\"schema_version\":1,\"ambiguity_id\":\"amb_ed9e7f6db452dc02e3f6f752\",\"scope\":{\"kind\":\"facet\",\"facet\":\"work\"},\"normalized_query\":\"jordan\",\"status\":\"open\",\"original_query\":\"Jordan\",\"latest_query\":\"Jordan\",\"first_seen\":\"2026-05-15T12:00:00Z\",\"last_seen\":\"2026-05-15T12:00:00Z\",\"observed_tier\":5,\"occurrence_count\":1,\"origins\":[{\"day\":\"20260510\",\"facet\":\"work\",\"field\":\"participation.name\",\"lane\":\"corpus.seed\",\"record_id\":\"100000_300\"}],\"origin_keys\":[\"{\\\"day\\\":\\\"20260510\\\",\\\"facet\\\":\\\"work\\\",\\\"field\\\":\\\"participation.name\\\",\\\"lane\\\":\\\"corpus.seed\\\",\\\"record_id\\\":\\\"100000_300\\\"}\"],\"resolved_entity_id\":null,\"resolved_at\":null,\"audit\":{\"prior_choices\":[]},\"ranked_candidates\":[{\"id\":\"jordan-vance\",\"name\":\"Jordan Vance\",\"score\":0.61,\"tier\":5},{\"id\":\"jordan-vancey\",\"name\":\"Jordan Vancey\",\"score\":0.58,\"tier\":5}]}\n",
    );
    write(
        &root.join("speakers/review-candidates.jsonl"),
        "{\"source_id\":\"spk_jordan_v\",\"source_label\":\"Jordan V\",\"target_id\":\"spk_jordan_vance\",\"target_label\":\"Jordan Vance\",\"status\":\"open\",\"similarity\":0.86,\"readiness\":\"ready\",\"evidence\":{\"basis\":\"speaker-name-variant\",\"summary\":\"Jordan V and Jordan Vance have matching speaker voiceprints (similarity 0.8600).\",\"detection_count\":1,\"readiness\":\"ready\"}}\n",
    );
    write(
        &root.join("speakers/candidate-pair-review-candidates.jsonl"),
        "{\"key\":\"[\\\"anchor_a\\\",\\\"anchor_b\\\"]\",\"anchor_a\":\"anchor_a\",\"anchor_b\":\"anchor_b\",\"status\":\"open\",\"similarity\":0.74,\"evidence\":{\"basis\":\"speaker-candidate-pair\",\"similarity\":0.74,\"source_intervals\":4,\"source_samples\":[{\"day\":\"20260510\",\"segment\":\"100000_300\"}],\"target_intervals\":3,\"target_samples\":[{\"day\":\"20260510\",\"segment\":\"103000_300\"}]}}\n",
    );
}

fn awareness(root: &Path) {
    write(
        &root.join("awareness/current.json"),
        "{\"capture\":{\"first_segment_day\":\"20260510\",\"streams_seen\":[\"_default\",\"workstation.browser\"]},\"imports\":{\"has_imported\":true,\"import_count\":1,\"last_completed\":\"20260515T12:00:00\",\"last_nudge\":null,\"last_result_summary\":\"12 notes\",\"offer_declined\":null,\"sources_used\":[\"obsidian\"]}}\n",
    );
    write(
        &root.join("awareness/20260515.jsonl"),
        "{\"ts\":1778846400000,\"kind\":\"state\",\"key\":\"imports.completed\",\"data\":{\"source_type\":\"obsidian\"}}\n{\"ts\":1778846400000,\"kind\":\"observation\",\"message\":\"seeded on the injected clock's today\"}\n",
    );
    write(
        &root.join("awareness/20260510.jsonl"),
        "{\"ts\":1778846400000,\"kind\":\"state\",\"key\":\"capture.first_segment\",\"message\":\"first segment seen\"}\n{\"ts\":1778846400000,\"kind\":\"observation\",\"message\":\"the owner works mornings\"}\n{\"ts\":1778846400000,\"kind\":\"nudge\",\"key\":\"imports.nudge_sent\"}\n",
    );
}

fn activities(root: &Path) {
    write(
        &root.join("facets/work/activities/activities.jsonl"),
        "{\"id\": \"meeting\", \"instructions\": \"Record meetings held during this span.\"}\n{\"id\": \"focus\", \"custom\": true, \"name\": \"Focus\", \"instructions\": \"Record focused work during this span.\", \"emoji\": \"🧠\"}\n",
    );
    write(
        &root.join("facets/work/activities/20260510.jsonl"),
        "{\"id\": \"meeting_100000_300\", \"activity\": \"meeting\", \"title\": \"Seeded meeting\", \"description\": \"A meeting the corpus seeded.\", \"details\": \"\", \"segments\": [\"100000_300\"], \"active_entities\": [], \"created_at\": 1770000400000, \"source\": \"user\", \"hidden\": false, \"edits\": [{\"timestamp\": \"2026-05-15T12:00:00Z\", \"actor\": \"corpus:seed\", \"fields\": [\"activity\", \"title\"], \"note\": \"created\"}]}\n{\"id\": \"focus_103000_300\", \"activity\": \"focus\", \"title\": \"Seeded muted focus block\", \"description\": \"A muted record, so include_hidden has something to reveal.\", \"details\": \"\", \"segments\": [\"103000_300\"], \"active_entities\": [], \"created_at\": 1770000500000, \"source\": \"user\", \"hidden\": true, \"edits\": [{\"timestamp\": \"2026-05-15T12:00:00Z\", \"actor\": \"corpus:seed\", \"fields\": [\"activity\", \"title\"], \"note\": \"created\"}]}\n",
    );
}

fn news(root: &Path) {
    write(
        &root.join("facets/work/news/20260510.md"),
        "---\ntitle: Work, week of May 10\nfacet: work\ngenerated_at: 1770000200\n---\n\n# What happened\n\nA **short** newsletter body with a list:\n\n- one item\n- two item\n\n> and a blockquote, because the PDF stylesheet has a rule for it.\n",
    );
    write(
        &root.join("facets/work/news/20260503.md"),
        "---\ntitle: Work, week of May 3\nfacet: work\ngenerated_at: 1770000100\n---\n\nAn earlier work newsletter so the feed has a second page.\n",
    );
    write(
        &root.join("facets/personal/news/20260510.md"),
        "---\ntitle: Personal, week of May 10\nfacet: personal\ngenerated_at: 1770000201\n---\n\nThe personal facet newsletter, one paragraph, no headings.\n",
    );
    write(
        &root.join("facets/work/facet.json"),
        "{\"title\": \"Work\", \"description\": \"The work facet.\"}\n",
    );
    write(
        &root.join("facets/personal/facet.json"),
        "{\"title\": \"Personal\", \"description\": \"The personal facet.\"}\n",
    );
}

pub fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().expect("parent")).expect("directories");
    fs::write(path, text).expect("file");
}

fn config(root: &Path) {
    write(
        &root.join("config/journal.json"),
        "{\n  \"setup\": {\n    \"completed_at\": 1767225600\n  }\n}\n",
    );
}

fn chronicle(root: &Path) {
    let day = root.join("chronicle/20260510");
    write(
        &day.join("100000_300/audio.jsonl"),
        "{\"t\": \"header\", \"stream\": \"_default\", \"start\": \"10:00:00\"}\n{\"t\": \"line\", \"ts\": 1, \"speaker\": \"S1\", \"text\": \"first line\"}\n{\"t\": \"line\", \"ts\": 2, \"speaker\": \"S2\", \"text\": \"second line\"}\n",
    );
    write(
        &day.join("100000_300/desktop.screen.jsonl"),
        "{\"t\": \"header\", \"device\": \"desktop\"}\n{\"t\": \"frame\", \"ts\": 1, \"summary\": \"an editor\"}\n",
    );
    write(
        &day.join("103000_300/audio.jsonl"),
        "{\"t\": \"header\", \"stream\": \"_default\", \"start\": \"10:30:00\"}\n{\"t\": \"line\", \"ts\": 1, \"speaker\": \"S1\", \"text\": \"audio only\"}\n",
    );
    write(
        &day.join("workstation.browser/140000_300/stream.json"),
        "{\"stream\": \"workstation.browser\"}\n",
    );
    write(
        &day.join("workstation.browser/140000_300/browser_docs-example-com.jsonl"),
        "{\"t\": \"segment_start\", \"ts\": 1770000300, \"site\": \"docs.example.com\", \"title\": \"Example docs\", \"adapter\": \"generic\", \"text\": \"The opening snapshot of the page.\"}\n{\"t\": \"change\", \"ts\": 1770000360, \"text\": \"A second paragraph appeared.\"}\n",
    );
    write_timeline_artifacts(root, &day);
}

fn write_timeline_artifacts(root: &Path, day_path: &Path) {
    let default = entry(
        "Both streams",
        "A default-stream segment with audio and screen.",
        "_default",
        "100000_300",
    );
    let browsing = entry(
        "Browsing",
        "A named browser stream.",
        "workstation.browser",
        "140000_300",
    );
    let mut day = serde_json::json!({
        "schema_version": 1,
        "kind": "day",
        "day": "20260510",
        "source_digest": "day-input",
        "generated_at_ms": 1770000100000_i64,
        "top_n": 4,
        "segment_count": 3,
        "hour_count": 2,
        "hours": {
            "10": hour("hour-10-input", 2, vec![default.clone()], "The only populated hour with both."),
            "14": hour("hour-14-input", 1, vec![browsing.clone()], "The browser hour."),
        },
        "day_curation": timeline_curation("day-input", 3, vec![default.clone()], "One seeded day.", "corpus-day-model"),
    });
    let day_text = write_json(&day_path.join("timeline.json"), &day);
    let mut segment_artifacts = std::collections::BTreeMap::new();
    for (path, label, title, description, stream, segment) in [
        (
            day_path.join("100000_300/timeline.json"),
            "default",
            "Both streams",
            "A default-stream segment with audio and screen.",
            "_default",
            "100000_300",
        ),
        (
            day_path.join("103000_300/timeline.json"),
            "second",
            "Audio follow-up",
            "A default-stream audio follow-up.",
            "_default",
            "103000_300",
        ),
        (
            day_path.join("workstation.browser/140000_300/timeline.json"),
            "browser",
            "Browsing",
            "A named browser stream.",
            "workstation.browser",
            "140000_300",
        ),
    ] {
        let summary = entry(title, description, stream, segment);
        let binding = solstone_core_timeline::SegmentBindingV1 {
            day: "20260510".to_owned(),
            stream: stream.to_owned(),
            segment: segment.to_owned(),
        };
        let relative_path = if stream == "_default" {
            format!("chronicle/20260510/{segment}/talents/activity.md")
        } else {
            format!("chronicle/20260510/{stream}/{segment}/talents/activity.md")
        };
        let activity = format!("Corpus activity for {label}.\n");
        write(&root.join(&relative_path), &activity);
        let source = solstone_core_timeline::SegmentSourceV1::GeneratedActivity {
            schema_version: solstone_core_timeline::SEGMENT_SOURCE_SCHEMA_VERSION,
            relative_path,
            sha256: solstone_core_timeline::artifact_sha256(&activity),
        };
        let digest = solstone_core_timeline::segment_input_digest(&binding, &source)
            .expect("segment input digest");
        let timeline = serde_json::json!({
            "schema_version": 1,
            "kind": "segment",
            "binding": summary["binding"],
            "input_digest": digest,
            "source": source,
            "generated_at_ms": 1770000050000_i64,
            "summary": {
                "title": summary["title"],
                "description": summary["description"],
                "origin": summary["origin"],
                "continuation_of": null,
            },
            "provenance": provenance("corpus-segment-model"),
        });
        let timeline_text = write_json(&path, &timeline);
        segment_artifacts.insert(label, (digest, timeline_text));
    }
    let (default_digest, default_text) = &segment_artifacts["default"];
    let (second_digest, second_text) = &segment_artifacts["second"];
    let (browser_digest, browser_text) = &segment_artifacts["browser"];
    let master = serde_json::json!({
        "schema_version": 1,
        "kind": "master",
        "source_digest": "master-input",
        "generated_at_ms": 1770000000000_i64,
        "top_n": 4,
        "months": {
            "202604": {
                "day_count": 0,
                "days": {},
                "month_curation": timeline_curation("month-202604-input", 0, Vec::<serde_json::Value>::new(), "", "corpus-master-model"),
            },
            "202605": {
                "day_count": 1,
                "days": {"20260510": day},
                "month_curation": timeline_curation("month-202605-input", 1, vec![default.clone()], "One seeded day with two streams.", "corpus-master-model"),
            },
        },
        "year_top": [{"month": "202605", "entry": default.clone()}],
        "year_curation": timeline_curation("master-input", 1, vec![default], "One seeded month.", "corpus-master-model"),
    });
    let master_text = write_json(&root.join("timeline.json"), &master);
    for (subject, mut published) in [
        ("master", artifact("master-input", &master_text)),
        ("day:20260510", artifact("day-input", &day_text)),
        (
            "segment:20260510/_default/100000_300",
            artifact(default_digest, default_text),
        ),
        (
            "segment:20260510/_default/103000_300",
            artifact(second_digest, second_text),
        ),
        (
            "segment:20260510/workstation.browser/140000_300",
            artifact(browser_digest, browser_text),
        ),
    ] {
        published.as_object_mut().unwrap().remove("generation");
        write_json(
            &solstone_core_timeline::timeline_record_path(root, subject).unwrap(),
            &serde_json::json!({"schema_version":1,"subject":subject,"published":published,"attempts":[]}),
        );
    }
    // Keep the presentation fixture's seeded picks/provenance, but bind its day
    // publication to the real segment inputs instead of the old placeholder.
    let digest =
        solstone_core_maintenance::bodies::timeline::current_day_source_digest(root, "20260510", 4)
            .expect("verified corpus day inputs");
    day["source_digest"] = serde_json::json!(digest);
    let day_text = write_json(&day_path.join("timeline.json"), &day);
    let mut published = artifact(&digest, &day_text);
    published.as_object_mut().unwrap().remove("generation");
    write_json(
        &solstone_core_timeline::timeline_record_path(root, "day:20260510").unwrap(),
        &serde_json::json!({"schema_version":1,"subject":"day:20260510","published":published,"attempts":[]}),
    );
}

fn entry(title: &str, description: &str, stream: &str, segment: &str) -> serde_json::Value {
    let origin = if stream == "_default" {
        format!("20260510/{segment}")
    } else {
        format!("20260510/{stream}/{segment}")
    };
    serde_json::json!({
        "title": title,
        "description": description,
        "origin": origin,
        "binding": {"day": "20260510", "stream": stream, "segment": segment},
    })
}

fn hour(
    input_digest: &str,
    segment_count: usize,
    picks: Vec<serde_json::Value>,
    rationale: &str,
) -> serde_json::Value {
    serde_json::json!({
        "source_digest": input_digest,
        "segment_count": segment_count,
        "curation": timeline_curation(input_digest, segment_count, picks, rationale, "corpus-day-model"),
    })
}

fn timeline_curation(
    input_digest: &str,
    candidate_count: usize,
    picks: Vec<serde_json::Value>,
    rationale: &str,
    model: &str,
) -> serde_json::Value {
    serde_json::json!({
        "input_digest": input_digest,
        "candidate_count": candidate_count,
        "picks": picks,
        "rationale": rationale,
        "error": null,
        "provenance": provenance(model),
    })
}

fn provenance(model: &str) -> serde_json::Value {
    serde_json::json!({
        "model": model,
        "finish_reason": "stop",
        "schema_validation": {"valid": true},
        "inference": {},
        "usage": {},
    })
}

fn artifact(input_digest: &str, artifact_text: &str) -> serde_json::Value {
    serde_json::json!({
        "input_digest": input_digest,
        "artifact_sha256": solstone_core_timeline::artifact_sha256(artifact_text),
        "published_at_ms": 1770000000000_i64,
        "generation": 1,
    })
}

fn write_json(path: &Path, value: &serde_json::Value) -> String {
    let text = serde_json::to_string(value).expect("timeline JSON");
    write(path, &text);
    text
}
