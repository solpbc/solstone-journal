// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! A browser segment over the floor window, as Sense and Screen read it there
//! (docs/THINK.md § Browser segments). A window that holds the segment still
//! reads it whole: this text reaches the model through `overflow_text`.

use std::fs;
use std::path::Path;

use serde_json::{Map, Value};

use crate::floor_reading::{FloorReading, floor_budget};

/// Room that leaves a file's reading with only its repeats removed.
const NO_LIMIT: u32 = 1 << 30;

/// The reading for a segment over the floor, or `None` when the segment fits,
/// holds no browser pages, or its rendering cannot be located in the request.
pub(crate) fn floor_reading(config: &Map<String, Value>, journal: &Path) -> Option<FloorReading> {
    if ["messages", "hook", "span", "activity", "_screen_batch_cuts"]
        .iter()
        .any(|key| config.contains_key(*key))
        || crate::product_names::model_text(config).is_some()
    {
        return None;
    }
    let budget = floor_budget(config)?;
    let transcript = config.get("transcript").and_then(Value::as_str)?;
    if solstone_core_tokens::count(transcript) <= budget {
        return None;
    }
    let stream = config
        .get("stream")
        .and_then(Value::as_str)
        .unwrap_or(solstone_core_journal_io::DEFAULT_STREAM);
    let segment = solstone_core_journal_io::paths::Segment::resolve_exact(
        journal,
        config.get("day")?.as_str()?,
        stream,
        config.get("segment")?.as_str()?,
    )
    .ok()??;
    let mut files: Vec<_> = fs::read_dir(segment.path())
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("browser_") && name.ends_with(".jsonl"))
        })
        .collect();
    files.sort();
    read_files(transcript, &files, budget)
}

fn read_files(transcript: &str, files: &[std::path::PathBuf], budget: u32) -> Option<FloorReading> {
    let count = solstone_core_tokens::count;
    let mut sources = Vec::new();
    for path in files {
        let text = fs::read_to_string(path).ok()?;
        let rendered = solstone_core_format::content::produce_browser_attended_chunks(&text)
            .chunks
            .into_iter()
            .map(|chunk| chunk.content)
            .collect::<Vec<_>>()
            .join("\n\n");
        let rendered = rendered.trim().to_owned();
        if rendered.is_empty() {
            continue;
        }
        if transcript.matches(&rendered).count() != 1 {
            return None;
        }
        sources.push((text, rendered));
    }
    if sources.is_empty() {
        return None;
    }
    let mut rest = transcript.to_owned();
    let mut sizes = Vec::new();
    for (_, rendered) in &sources {
        rest = rest.replacen(rendered.as_str(), "", 1);
        sizes.push(count(rendered));
    }
    let mut room = budget.checked_sub(count(&rest))?;
    // What each file needs once its repeats are gone. A file that needs less than its share stays whole.
    let needs: Vec<u32> = sources
        .iter()
        .map(|(text, _)| {
            solstone_core_format::content::browser_attended_reading(text, NO_LIMIT, &count)
                .map_or(NO_LIMIT, |reading| count(&reading.text))
        })
        .collect();
    for _ in 0..3 {
        let shares = shares_of(&needs, room);
        let mut result = transcript.to_owned();
        let mut lines_left_out = 0;
        let mut chars_left_out = 0;
        for ((text, rendered), share) in sources.iter().zip(shares) {
            let reading =
                solstone_core_format::content::browser_attended_reading(text, share, &count)?;
            result = result.replacen(rendered.as_str(), &reading.text, 1);
            lines_left_out += reading.lines_left_out;
            chars_left_out += reading.chars_left_out;
        }
        let tokens = count(&result);
        if tokens <= budget {
            return Some(FloorReading {
                text: result,
                pages_not_whole: lines_left_out,
                chars_left_out,
            });
        }
        // Tokenizing the joined text can cost a little more than its parts.
        room = room.saturating_sub(tokens - budget);
    }
    None
}

/// One cap across the files' needs: each gets what it needs, up to the largest cap the room allows.
fn shares_of(needs: &[u32], room: u32) -> Vec<u32> {
    let total = |cap: u32| -> u64 {
        needs
            .iter()
            .map(|need| u64::from((*need).min(cap)))
            .sum::<u64>()
    };
    let (mut low, mut high) = (0, needs.iter().copied().max().unwrap_or(0));
    while low < high {
        let middle = low + (high - low).div_ceil(2);
        if total(middle) <= u64::from(room) {
            low = middle;
        } else {
            high = middle - 1;
        }
    }
    needs.iter().map(|need| (*need).min(low)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const BUDGET: u32 = 12_000;

    fn long_page(title: &str, lines: usize) -> String {
        let blocks = (0..lines)
            .map(|line| {
                json!({"type":"text","text":format!("Line {line} of {title} says something plain enough to read.")})
            })
            .collect::<Vec<_>>();
        json!({"t":"segment_start","ts":1,"inst":"i","ctx":title,"url":format!("https://{title}.example/"),"title":title,"tab":{"front":true,"focused":true},"blocks":blocks}).to_string()
    }

    fn written(text: &str) -> (tempfile::TempDir, std::path::PathBuf, String) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("browser_pages.jsonl");
        fs::write(&path, text).unwrap();
        let rendered = solstone_core_format::content::produce_browser_attended_chunks(text)
            .chunks
            .into_iter()
            .map(|chunk| chunk.content)
            .collect::<Vec<_>>()
            .join("\n\n");
        let transcript = format!(
            "## 2026-11-03 12:00:00 - 12:04:59\n\n### Browser Content\n{}\n\n### desktop activity\nFront app noted.\n",
            rendered.trim()
        );
        (directory, path, transcript)
    }

    #[test]
    fn a_reading_that_fitting_then_cut_still_says_so() {
        let cut = json!({"clipped": true, "used_overflow_text": true, "dropped_entries": 1, "dropped_chars": 9000});
        let mut prepared = crate::PreparedTalent::new("sense".into(), Map::new());
        prepared.floor_reading = Some(FloorReading {
            text: "repeats gone".into(),
            pages_not_whole: 0,
            chars_left_out: 0,
        });
        crate::record_input_budget(&mut prepared, Some(&cut));
        let marker = crate::partial_input::marker(&prepared).expect("left out");
        assert_eq!(marker["dropped_chars"], json!(9000));

        prepared.config.remove(crate::INPUT_BUDGET_KEY);
        crate::record_input_budget(
            &mut prepared,
            Some(&json!({"clipped": false, "used_overflow_text": true})),
        );
        assert!(crate::partial_input::marker(&prepared).is_none());

        prepared.floor_reading = Some(FloorReading {
            text: "lines left out".into(),
            pages_not_whole: 3,
            chars_left_out: 100,
        });
        crate::record_input_budget(&mut prepared, Some(&cut));
        let marker = crate::partial_input::marker(&prepared).expect("left out");
        assert_eq!(marker["dropped_entries"], json!(4));
        assert_eq!(marker["dropped_chars"], json!(9100));
    }

    #[test]
    fn shares_give_a_small_file_what_it_needs_and_the_rest_to_the_large_one() {
        assert_eq!(shares_of(&[100_000, 40], 20_000), vec![19_960, 40]);
        assert_eq!(shares_of(&[5_000, 5_000], 25_000), vec![5_000, 5_000]);
        assert_eq!(shares_of(&[30_000, 30_000], 20_000), vec![10_000, 10_000]);
    }

    #[test]
    fn the_browser_view_is_replaced_in_place_and_the_rest_of_the_request_stays() {
        let (_directory, path, transcript) = written(&format!(
            "{}\n{}\n",
            long_page("oldest", 800),
            long_page("newest", 800)
        ));
        let reading = read_files(&transcript, &[path], BUDGET).expect("read");
        assert!(solstone_core_tokens::count(&reading.text) <= BUDGET);
        assert!(reading.text.starts_with("## 2026-11-03 12:00:00"));
        assert!(
            reading
                .text
                .contains("### Browser Content\n[Long browsing segment")
        );
        assert!(reading.text.contains("## oldest"));
        assert!(reading.text.contains("## newest"));
        assert!(
            reading
                .text
                .ends_with("### desktop activity\nFront app noted.\n")
        );
        assert!(reading.pages_not_whole > 0);
        assert!(reading.input_budget().is_some());
    }

    #[test]
    fn a_reading_that_removed_only_repeats_is_whole() {
        let page = long_page("again", 400);
        let (_directory, path, transcript) = written(&format!("{page}\n{page}\n{page}\n{page}\n"));
        assert!(solstone_core_tokens::count(&transcript) > BUDGET);
        let reading = read_files(&transcript, &[path], BUDGET).expect("read");
        assert_eq!(reading.pages_not_whole, 0);
        assert!(reading.input_budget().is_none());
    }

    #[test]
    fn a_rendering_it_cannot_locate_leaves_the_ordinary_path() {
        let (_directory, path, _) = written(&long_page("elsewhere", 800));
        assert!(read_files("no such rendering in here", &[path], BUDGET).is_none());
    }

    #[test]
    fn a_default_stream_segment_is_read_where_it_sits_under_its_day() {
        let journal = tempfile::tempdir().unwrap();
        let segment = journal.path().join("chronicle/20261103/120000_300");
        fs::create_dir_all(&segment).unwrap();
        let (_directory, path, transcript) = written(&format!(
            "{}\n{}\n",
            long_page("oldest", 3000),
            long_page("newest", 3000)
        ));
        fs::copy(&path, segment.join("browser_pages.jsonl")).unwrap();
        let mut config = Map::new();
        for (key, value) in [
            ("day", "20261103"),
            ("stream", "_default"),
            ("segment", "120000_300"),
            ("prompt", "Read."),
        ] {
            config.insert(key.into(), json!(value));
        }
        config.insert("max_output_tokens".into(), json!(6912));
        config.insert("transcript".into(), json!(transcript));
        assert!(floor_reading(&config, journal.path()).is_some());
    }

    #[test]
    fn a_segment_that_fits_or_holds_no_browser_file_has_no_reading() {
        let journal = tempfile::tempdir().unwrap();
        let segment = journal.path().join("chronicle/20261103/web/120000_300");
        fs::create_dir_all(&segment).unwrap();
        let mut config = Map::new();
        for (key, value) in [
            ("day", "20261103"),
            ("stream", "web"),
            ("segment", "120000_300"),
            ("prompt", "Read."),
        ] {
            config.insert(key.into(), json!(value));
        }
        config.insert("max_output_tokens".into(), json!(6912));
        config.insert("transcript".into(), json!("short"));
        assert!(floor_reading(&config, journal.path()).is_none());
        config.insert("transcript".into(), json!("word ".repeat(40_000)));
        assert!(floor_reading(&config, journal.path()).is_none());
    }
}
