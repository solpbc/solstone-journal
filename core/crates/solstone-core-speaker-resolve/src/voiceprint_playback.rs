// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Playback admission shared by every path that learns a person's voice.

use serde_json::Value;
use solstone_core_journal_io::SegmentLayout;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

/// Tags are maxima across the recording, not sentence-level classifications.
/// Require strong playback evidence that exceeds speech: light background
/// music must not prevent learning from a conversation. Missing tags predate
/// classification and do not themselves establish playback.
fn playback(header: &Value) -> bool {
    let tags = &header["sound_tags"]["tags"];
    let speech = tags["Speech"].as_f64().unwrap_or(0.0);
    let music = tags["Music"].as_f64().unwrap_or(0.0);
    let television = tags["Television"].as_f64().unwrap_or(0.0);
    (music >= 0.8 && music > speech) || (television >= 0.5 && television > speech)
}

pub(crate) fn allows_transcript(path: &Path) -> bool {
    let read = || -> Result<bool, Box<dyn std::error::Error>> {
        let file = match File::open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(true),
            Err(error) => return Err(error.into()),
        };
        let mut line = String::new();
        if BufReader::new(file).read_line(&mut line)? == 0 {
            return Ok(true);
        }
        Ok(!playback(&serde_json::from_str::<Value>(&line)?))
    };
    match read() {
        Ok(allowed) => allowed,
        Err(error) => {
            log::warn!(
                "voice sample admission could not read {}: {error}",
                path.display()
            );
            false
        }
    }
}

pub(crate) fn allows_metadata(journal: &Path, metadata: &Value) -> bool {
    let Some(day) = metadata["day"].as_str() else {
        return false;
    };
    let Some(key) = metadata["segment_key"].as_str() else {
        return false;
    };
    let Some(source) = metadata["source"].as_str() else {
        return false;
    };
    if source.is_empty() || source.contains(['/', '\\']) || source == "." || source == ".." {
        return false;
    }
    let stream = metadata["stream"]
        .as_str()
        .filter(|s| !s.is_empty())
        .unwrap_or("_default");
    let layout = match metadata["stream_layout"].as_str() {
        Some("direct") => SegmentLayout::Direct,
        Some("named") => SegmentLayout::Named,
        None if stream == "_default" => SegmentLayout::Direct,
        None => SegmentLayout::Named,
        Some(_) => return false,
    };
    match crate::segment_catalog::resolve_exact(journal, day, stream, key, layout) {
        Ok(Some(dir)) => allows_transcript(&dir.join(format!("{source}.jsonl"))),
        Ok(None) => true,
        Err(error) => {
            log::warn!("voice sample admission could not resolve recording: {error}");
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn dominant_playback_is_excluded_but_background_music_is_not() {
        assert!(playback(
            &json!({"sound_tags":{"tags":{"Music":0.93,"Speech":0.79,"Television":0.38,"Music of Asia":0.25}}})
        ));
        assert!(playback(
            &json!({"sound_tags":{"tags":{"Television":0.85,"Speech":0.6}}})
        ));
        assert!(!playback(
            &json!({"sound_tags":{"tags":{"Music":0.25,"Speech":0.95}}})
        ));
        assert!(!playback(
            &json!({"sound_tags":{"tags":{"Music":0.85,"Speech":0.97}}})
        ));
        assert!(!playback(&json!({})));
    }
}
