// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;

use chrono::{Duration, NaiveDate};
use serde_json::Value;

use crate::weekly_reflection::{
    Candidate, DayClassification, DayReport, Said, WeeklyReflectionState, assemble_reflection,
};
use solstone_core_home::weekly::{WeekJudgment, judge, page_model};

fn classification_places() -> Vec<(DayClassification, &'static str)> {
    [
        DayClassification::NothingShared,
        DayClassification::NotReady,
        DayClassification::NotOnPage,
        DayClassification::Memory,
        DayClassification::Unreadable,
    ]
    .into_iter()
    .map(|classification| {
        let place = match classification {
            DayClassification::NothingShared => "nothing_shared",
            DayClassification::NotReady => "not_ready",
            DayClassification::NotOnPage => "not_on_page",
            DayClassification::Memory => "memory",
            DayClassification::Unreadable => "unreadable",
        };
        (classification, place)
    })
    .collect()
}

#[test]
fn producer_week_opens_in_the_reader() {
    let places = classification_places();
    let mut assigned = places.clone();
    let filler = places[0];
    while !assigned.len().is_multiple_of(7) {
        assigned.push(filler);
    }

    let root = tempfile::tempdir().unwrap();
    let first = NaiveDate::from_ymd_opt(2026, 3, 8).unwrap();
    for (week_index, chunk) in assigned.chunks(7).enumerate() {
        let start = first + Duration::days((week_index * 7) as i64);
        let mut reports = Vec::with_capacity(7);
        let mut candidates = Vec::new();
        for (offset, (classification, _)) in chunk.iter().enumerate() {
            let day = (start + Duration::days(offset as i64))
                .format("%Y%m%d")
                .to_string();
            if matches!(classification, DayClassification::Memory) {
                candidates.push(Candidate {
                    day: day.clone(),
                    position: 0,
                    text: "A memory from this day.".to_owned(),
                    placeholder: false,
                    word_count: 5,
                });
            }
            reports.push(DayReport {
                day,
                classification: *classification,
            });
        }

        let said = vec![
            Said {
                day: reports[0].day.clone(),
                facet: "work".to_owned(),
                record_id: "meeting_1".to_owned(),
                group: "commitments",
                index: 0,
                quote: "I'll get you the deck by friday".to_owned(),
            },
            Said {
                day: reports[1].day.clone(),
                facet: "work".to_owned(),
                record_id: "meeting_1".to_owned(),
                group: "decisions",
                index: 1,
                quote: "let's go with blue then".to_owned(),
            },
        ];
        let start_day = reports[0].day.clone();
        let state = WeeklyReflectionState {
            start_day: start_day.clone(),
            end_day: reports[6].day.clone(),
            today: start,
            generated_at: "2026-03-15T18:00:00Z".to_owned(),
            day_reports: reports,
            slots: Vec::new(),
            all_candidates: Vec::new(),
            said,
        };
        let (_markdown, document) =
            assemble_reflection(&state, "model", None, &candidates).unwrap();
        let path = root
            .path()
            .join(format!("reflections/weekly/{start_day}.json"));
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, &document).unwrap();

        let doc: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        let WeekJudgment::Page(trusted) = judge(root.path(), &start_day) else {
            panic!("week {start_day} did not open");
        };
        let solstone_core_home::weekly::Said::Entries(entries) = &trusted.said else {
            panic!("said list was unreadable");
        };
        let raw_said = doc["said"].as_array().unwrap();
        assert_eq!(entries.len(), raw_said.len());
        for (entry, raw) in entries.iter().zip(raw_said) {
            assert_eq!(entry.id, raw["id"].as_str().unwrap());
            assert_eq!(entry.key, raw["key"].as_str().unwrap());
            assert_eq!(entry.day, raw["day"].as_str().unwrap());
            assert_eq!(entry.quote, raw["quote"].as_str().unwrap());
            assert_eq!(entry.uri, raw["source"]["uri"].as_str().unwrap());
        }

        let model = page_model(root.path(), &start_day, 2026, &|_| false).unwrap();
        let cells = model["cells"].as_array().unwrap();
        let legend = model["legend"].as_array().unwrap();
        let doc_days = doc["days"].as_array().unwrap();
        assert_eq!(cells.len(), 7);
        assert_eq!(doc_days.len(), 7);
        for (offset, cell) in cells.iter().enumerate() {
            assert_eq!(cell["state"], doc_days[offset]["state"]);
            assert_eq!(cell["state"].as_str().unwrap(), chunk[offset].1);
            assert!(legend.iter().any(|item| item["state"] == cell["state"]));
            let name = cell["accessible_name"].as_str().unwrap();
            let phrase = name.split_once(", ").map(|(_, phrase)| phrase).unwrap();
            assert!(!phrase.is_empty());
        }
    }
}
