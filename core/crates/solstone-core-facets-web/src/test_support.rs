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
        }
        _ => panic!("known phase: {phase}"),
    }
    root
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
    write(&root.join("timeline.json"), MASTER);
}

const MASTER: &str = r#"{
  "generated_at": 1770000000,
  "model": "corpus-model",
  "top_n": 4,
  "year_top": [
    {
      "month": "202605",
      "title": "Timeline port",
      "description": "The month the corpus describes.",
      "origin": "20260510/100000_300"
    }
  ],
  "months": {
    "202604": {
      "month_top": [],
      "month_rationale": "",
      "day_count": 0,
      "days_with_data": [],
      "days": {}
    },
    "202605": {
      "month_top": [
        {
          "title": "Timeline port",
          "description": "The month the corpus describes.",
          "origin": "20260510/100000_300"
        }
      ],
      "month_rationale": "One seeded day with two streams.",
      "day_count": 1,
      "days_with_data": [
        "20260510"
      ],
      "days": {
        "20260510": {
          "generated_at": 1770000100,
          "model": "corpus-day-model",
          "day_top": [
            {
              "title": "Both streams",
              "description": "A default-stream segment with audio and screen.",
              "origin": "20260510/100000_300"
            }
          ],
          "day_rationale": "One seeded day.",
          "hours": {
            "10": {
              "picks": [
                {
                  "title": "Both streams",
                  "description": "Audio and screen together.",
                  "origin": "20260510/100000_300"
                }
              ],
              "rationale": "The only populated hour with both."
            },
            "14": {
              "picks": [
                {
                  "title": "Browsing",
                  "description": "A named browser stream.",
                  "origin": "20260510/workstation.browser/140000_300"
                }
              ],
              "rationale": "The browser hour."
            }
          }
        }
      }
    }
  }
}
"#;
