// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Audio sample URLs shared by speaker maintenance and review.

use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};
use solstone_core_journal_io::SegmentLayout;
use std::path::Path;

const PATH_COMPONENT: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'!')
    .add(b'"')
    .add(b'#')
    .add(b'$')
    .add(b'%')
    .add(b'&')
    .add(b'\'')
    .add(b'(')
    .add(b')')
    .add(b'*')
    .add(b'+')
    .add(b',')
    .add(b'/')
    .add(b':')
    .add(b';')
    .add(b'<')
    .add(b'=')
    .add(b'>')
    .add(b'?')
    .add(b'@')
    .add(b'[')
    .add(b'\\')
    .add(b']')
    .add(b'^')
    .add(b'`')
    .add(b'{')
    .add(b'|')
    .add(b'}');

const AUDIO_FORMATS: [(&str, &str); 6] = [
    (".flac", "audio/flac"),
    (".opus", "audio/opus"),
    (".ogg", "audio/ogg"),
    (".m4a", "audio/mp4"),
    (".mp3", "audio/mpeg"),
    (".wav", "audio/wav"),
];

/// Resolve the existing audio sample and its MIME type for an exact segment layout.
pub fn audio_info(
    segment_dir: &Path,
    day: &str,
    stream: &str,
    segment_key: &str,
    source: &str,
    layout: SegmentLayout,
) -> (Option<String>, Option<String>) {
    for (extension, mimetype) in AUDIO_FORMATS {
        let filename = format!("{source}{extension}");
        if segment_dir.join(&filename).is_file() {
            let day = path_component(day);
            let stream = path_component(stream);
            let segment_key = path_component(segment_key);
            let filename = path_component(&filename);
            let url = match layout {
                SegmentLayout::Direct => {
                    format!("/app/speakers/api/serve_audio/{day}/{segment_key}/{filename}")
                }
                SegmentLayout::Named => {
                    format!("/app/speakers/api/serve_audio/{day}/{stream}/{segment_key}/{filename}")
                }
            };
            return (Some(url), Some(mimetype.to_owned()));
        }
    }
    (None, None)
}

fn path_component(value: &str) -> String {
    utf8_percent_encode(value, PATH_COMPONENT).to_string()
}
