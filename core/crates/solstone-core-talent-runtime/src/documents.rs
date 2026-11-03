// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::{Map, Value};

use crate::contract::GateDecision;
use crate::floor_reading::{FloorReading, floor_budget};
use crate::{ExecutionContext, PreparedTalent, StageError};

/// A page's opening lines, at the least, worth showing a reader.
const MIN_EXCERPT_TOKENS: u32 = 60;
const PAGE_HEADING: &str = "\n## Page ";

pub fn gate(
    prepared: &PreparedTalent,
    _context: &ExecutionContext,
) -> Result<GateDecision, StageError> {
    // Callable hosts do not project request.env into the host environment.
    // The prepared request carries the same bound stream in every host.
    Ok(gate_stream(
        prepared
            .config
            .get("stream")
            .and_then(serde_json::Value::as_str),
    ))
}

fn gate_stream(stream: Option<&str>) -> GateDecision {
    if stream == Some("import.document") {
        GateDecision::Proceed
    } else {
        GateDecision::Skip("not a document import segment".to_owned())
    }
}

/// A long document as a window too small for all of it reads it: its first
/// pages, where its parties, date and definitions are, then its last pages,
/// where its signatures and schedules are, and the opening lines of pages
/// spread evenly between them. A window that holds the whole document reads
/// the whole document (docs/THINK.md § Documents).
///
/// `None` when it fits with its whole reply or holds no pages to choose between.
pub(crate) fn floor_reading(config: &Map<String, Value>) -> Option<FloorReading> {
    let budget = floor_budget(config)?;
    let transcript = config.get("transcript").and_then(Value::as_str)?;
    (solstone_core_tokens::count(transcript) > budget)
        .then(|| read_to_budget(transcript, budget))
        .flatten()
}

fn read_to_budget(transcript: &str, budget: u32) -> Option<FloorReading> {
    let (head, pages) = split_pages(transcript);
    if pages.len() < 2 {
        return None;
    }
    let count = solstone_core_tokens::count;
    // The note is counted at its longest form: every page range it can name.
    let room = budget.checked_sub(count(head) + 96)?;
    let tokens: Vec<u32> = pages.iter().map(|page| count(page)).collect();
    let (mut first, mut front_room) = (0, room / 2);
    while first < pages.len() && tokens[first] <= front_room {
        front_room -= tokens[first];
        first += 1;
    }
    let mut last = pages.len();
    let mut back_room = room / 8;
    while last > first && tokens[last - 1] <= back_room {
        back_room -= tokens[last - 1];
        last -= 1;
    }
    let between_room = room.saturating_sub(room / 2 + room / 8) + front_room + back_room;
    let between = last - first;
    // Evenly spaced from the first of them to the last, so a first or last page
    // too large to read whole still opens the reading or closes it.
    let shown = between
        .min((between_room / MIN_EXCERPT_TOKENS) as usize)
        .max(between.min(2));
    let chosen: Vec<usize> = (0..shown)
        .map(|index| first + index * (between - 1) / (shown - 1).max(1))
        .collect();
    // Shrinks to no excerpts at all, so the bound holds whatever the pages hold.
    let mut excerpt_chars = if shown == 0 {
        0
    } else {
        (between_room / shown as u32) as usize * 3
    };
    loop {
        let reading = assemble(head, &pages, first, last, &chosen, excerpt_chars);
        if count(&reading.text) <= budget || excerpt_chars == 0 {
            return (count(&reading.text) <= budget).then_some(reading);
        }
        excerpt_chars = excerpt_chars * 9 / 10;
    }
}

fn split_pages(transcript: &str) -> (&str, Vec<&str>) {
    let mut starts: Vec<usize> = transcript
        .match_indices(PAGE_HEADING)
        .map(|(at, _)| at + 1)
        .collect();
    let Some(&start) = starts.first() else {
        return (transcript, Vec::new());
    };
    starts.push(transcript.len());
    let pages = starts
        .windows(2)
        .map(|pair| &transcript[pair[0]..pair[1]])
        .collect();
    (&transcript[..start], pages)
}

fn assemble(
    head: &str,
    pages: &[&str],
    first: usize,
    last: usize,
    chosen: &[usize],
    excerpt_chars: usize,
) -> FloorReading {
    let mut body = String::new();
    let mut chars_left_out = 0;
    for (index, page) in pages.iter().enumerate() {
        let whole = index < first || index >= last;
        if whole {
            body.push_str(page);
            continue;
        }
        let excerpt = (excerpt_chars > 0 && chosen.contains(&index))
            .then(|| page_opening(page, excerpt_chars));
        let kept = excerpt.as_deref().unwrap_or_default();
        chars_left_out += page.chars().count().saturating_sub(kept.chars().count());
        body.push_str(kept);
    }
    let mut complete = Vec::new();
    if first > 0 {
        complete.push(format!("1-{first}"));
    }
    if last < pages.len() {
        complete.push(format!("{}-{}", last + 1, pages.len()));
    }
    let complete = if complete.is_empty() {
        "No page is complete".to_owned()
    } else {
        format!("Pages {} are complete", complete.join(" and "))
    };
    let note = format!(
        "[Long document, {} pages. {complete}. Other pages show only their opening lines, and some are not shown.]\n\n",
        pages.len(),
    );
    FloorReading {
        text: format!("{head}{note}{body}"),
        pages_not_whole: last - first,
        chars_left_out,
    }
}

/// A page's heading and opening lines, ending at a sentence when one is near.
fn page_opening(page: &str, chars: usize) -> String {
    if page.chars().count() <= chars {
        return page.to_owned();
    }
    let cut: String = page.chars().take(chars).collect();
    let end = cut
        .rfind(". ")
        .filter(|at| *at >= cut.len() / 2)
        .map_or(cut.len(), |at| at + 1);
    format!("{} …\n\n", cut[..end].trim_end())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const BUDGET: u32 = 20_000;

    fn document(pages: usize, sentences: usize) -> String {
        let mut text = format!(
            "## 2026-11-03 12:00:00 - 12:00:00\n\n### Transcript\n# trust\n\n**Pages:** {pages}\n"
        );
        for page in 1..=pages {
            text.push_str(&format!("\n## Page {page}\n\nOpening of page {page}. "));
            for sentence in 0..sentences {
                text.push_str(&format!(
                    "The trustee shall hold share {sentence} of page {page} under this article. "
                ));
            }
            text.push_str(&format!("Closing of page {page}.\n"));
        }
        text
    }

    #[test]
    fn a_long_document_keeps_its_ends_and_samples_between_them_within_the_floor() {
        for (pages, sentences) in [(30, 60), (100, 60), (500, 60), (5000, 60), (3, 40_000)] {
            let reading = read_to_budget(&document(pages, sentences), BUDGET).expect("sampled");
            assert!(
                solstone_core_tokens::count(&reading.text) <= BUDGET,
                "{pages}"
            );
            assert!(reading.text.contains("**Pages:**"), "{pages}");
            assert!(reading.chars_left_out > 0, "{pages}");
            assert!(reading.text.contains("Opening of page 1."), "{pages}");
            assert!(
                reading.text.contains(&format!("## Page {pages}")),
                "{pages}"
            );
        }
    }

    #[test]
    fn a_document_of_many_pages_reads_its_first_page_whole_and_says_what_it_shows() {
        let reading = read_to_budget(&document(100, 60), BUDGET).expect("sampled");
        assert!(reading.text.contains("Closing of page 1.\n"));
        assert!(reading.text.contains("Closing of page 100.\n"));
        assert!(reading.text.contains("Opening of page 50."));
        assert!(!reading.text.contains("Closing of page 50.\n"));
        assert!(reading.text.contains("[Long document, 100 pages. Pages 1-"));
        assert!(reading.pages_not_whole > 0);
    }

    #[test]
    fn a_first_or_last_page_too_large_to_read_whole_still_opens_or_closes_the_reading() {
        let huge = "The trustee shall hold this share under this article. ".repeat(20_000);
        let normal = document(400, 60);
        let (head, pages) = split_pages(&normal);
        let last = pages.len() - 1;
        for oversized in [0, last] {
            let mut text = head.to_owned();
            for (index, page) in pages.iter().enumerate() {
                text.push_str(page);
                if index == oversized {
                    text.push_str(&huge);
                    text.push('\n');
                }
            }
            let reading = read_to_budget(&text, BUDGET).expect("sampled");
            assert!(solstone_core_tokens::count(&reading.text) <= BUDGET);
            assert!(reading.text.contains("## Page 1\n"), "{oversized}");
            assert!(reading.text.contains("## Page 400\n"), "{oversized}");
            assert!(!reading.text.contains("Pages 1-0"), "{oversized}");
            assert!(!reading.text.contains("401-400"), "{oversized}");
        }
    }

    #[test]
    fn a_result_read_from_the_reading_says_what_it_left_out_and_a_whole_read_does_not() {
        let mut config = Map::new();
        config.insert("max_output_tokens".into(), json!(9984));
        config.insert("prompt".into(), json!("Read it."));
        config.insert("transcript".into(), json!(document(100, 60)));
        let reading = floor_reading(&config).expect("sampled");
        let mut prepared = PreparedTalent::new("documents".into(), config);
        prepared.floor_reading = Some(reading.clone());
        let request = crate::generate_request(&prepared).expect("request");
        assert_eq!(
            request.overflow_text.as_deref(),
            Some(reading.text.as_str())
        );

        crate::record_input_budget(
            &mut prepared,
            Some(&json!({"clipped": false, "used_overflow_text": true})),
        );
        let marker = crate::partial_input::marker(&prepared).expect("left out");
        assert_eq!(marker["dropped_entries"], json!(reading.pages_not_whole));
        assert_eq!(marker["dropped_chars"], json!(reading.chars_left_out));

        prepared.config.remove(crate::INPUT_BUDGET_KEY);
        crate::record_input_budget(&mut prepared, Some(&json!({"clipped": false})));
        assert!(crate::partial_input::marker(&prepared).is_none());
    }

    #[test]
    fn a_document_that_fits_or_has_no_pages_stays_with_the_ordinary_path() {
        let mut config = Map::new();
        config.insert("max_output_tokens".into(), json!(9984));
        config.insert("prompt".into(), json!("Read it."));
        config.insert("transcript".into(), json!(document(5, 20)));
        assert!(floor_reading(&config).is_none());
        config.insert("transcript".into(), json!("word ".repeat(60_000)));
        assert!(floor_reading(&config).is_none());
        config.insert("transcript".into(), json!(document(100, 60)));
        let reading = floor_reading(&config).expect("sampled");
        assert_eq!(reading.input_budget().unwrap()["clipped"], true);
        assert_eq!(
            reading.input_budget().unwrap()["dropped_chars"],
            json!(reading.chars_left_out)
        );
    }
}
