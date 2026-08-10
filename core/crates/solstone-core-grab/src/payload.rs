// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;

use serde_json::{Map, Value, json};

use crate::encode::{refuse_conflicts, save_frame};
use crate::error::GrabFailure;
use crate::extract::decode_frames;
use crate::reader::{
    ScreenBundle, available_days, available_screen_tokens, available_segments, frame_id,
    load_bundle, normalize_screen_token, require_day, require_stream, screen_stem,
    streams_except_health,
};
use crate::render::{frame_notes, render};
use crate::request::{GrabDiagnostics, GrabOutput, GrabRequest};
use crate::selection::{parse_frame_id_token, resolve_output_paths};
use crate::time::{frame_abs_time, segment_window};

pub(crate) fn run(
    journal: &Path,
    request: GrabRequest,
    diagnostics: &mut dyn GrabDiagnostics,
) -> Result<GrabOutput, GrabFailure> {
    if request.tokens.len() > 5 {
        return Err(GrabFailure::Usage(
            "grab accepts at most 5 positional tokens: day stream segment screen frame-id"
                .to_owned(),
        ));
    }
    if request.force && request.out.is_none() {
        return Err(GrabFailure::Usage("--force requires --out".to_owned()));
    }
    if request.out.is_some() && request.tokens.len() != 5 {
        return Err(GrabFailure::Usage(
            "--out requires day stream segment screen and frame-id".to_owned(),
        ));
    }
    if let Some(out) = &request.out {
        let _ = resolve_output_paths(out, &[1])?;
    }
    let payload = match request.tokens.as_slice() {
        [] => list_available_days(journal, diagnostics)?,
        [day] => list_day_streams(journal, day, diagnostics)?,
        [day, stream] => list_stream_segments(journal, day, stream, diagnostics)?,
        [day, stream, segment] => list_segment_screens(journal, day, stream, segment, diagnostics)?,
        [day, stream, segment, screen] => {
            list_screen_frames(journal, day, stream, segment, screen, diagnostics)?
        }
        [day, stream, segment, screen, token] => {
            let ids = parse_frame_id_token(token)?;
            if ids.len() > 1 && request.out.is_none() {
                return Err(GrabFailure::Usage(
                    "multiple frame ids require --out".to_owned(),
                ));
            }
            if let Some(out) = request.out {
                save_frame_images(
                    journal,
                    day,
                    stream,
                    segment,
                    screen,
                    &ids,
                    &out,
                    request.force,
                    diagnostics,
                )?
            } else {
                show_frame_metadata(journal, day, stream, segment, screen, ids[0], diagnostics)?
            }
        }
        _ => unreachable!("length checked above"),
    };
    let human = render(&payload)?;
    Ok(GrabOutput { payload, human })
}

fn list_available_days(
    journal: &Path,
    diagnostics: &mut dyn GrabDiagnostics,
) -> Result<Value, GrabFailure> {
    let mut days = Vec::new();
    for day in available_days(journal)? {
        let streams = list_day_streams(journal, &day, diagnostics)?
            .get("data")
            .and_then(|data| data.get("streams"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if streams.is_empty() {
            continue;
        }
        days.push(json!({"day":day, "streams":streams.len(), "segments":sum(&streams,"segments"), "screens":sum(&streams,"screens"), "frames_analyzed":sum(&streams,"frames_analyzed")}));
    }
    Ok(json!({"level":"0", "scope":{}, "data":{"days":days}}))
}

fn list_day_streams(
    journal: &Path,
    day: &str,
    diagnostics: &mut dyn GrabDiagnostics,
) -> Result<Value, GrabFailure> {
    let day_path = require_day(journal, day)?;
    let mut streams = Vec::new();
    for stream in streams_except_health(&day_path)? {
        let segments = list_stream_segments(journal, day, &stream, diagnostics)?
            .get("data")
            .and_then(|data| data.get("segments"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if segments.is_empty() {
            continue;
        }
        streams.push(json!({"stream":stream, "segments":segments.len(), "screens":sum(&segments,"screens"), "frames_analyzed":sum(&segments,"frames_analyzed")}));
    }
    Ok(json!({"level":"1", "scope":{"day":day}, "data":{"streams":streams}}))
}

fn list_stream_segments(
    journal: &Path,
    day: &str,
    stream: &str,
    diagnostics: &mut dyn GrabDiagnostics,
) -> Result<Value, GrabFailure> {
    let stream_path = require_stream(journal, day, stream)?;
    let mut rows = Vec::new();
    for segment in available_segments(&stream_path)? {
        let window = match segment_window(day, &segment) {
            Ok(window) => window,
            Err(_) => continue,
        };
        let screens = list_segment_screens(journal, day, stream, &segment, diagnostics)?
            .get("data")
            .and_then(|data| data.get("screens"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if screens.is_empty() {
            continue;
        }
        rows.push(json!({"segment":segment, "start":window.start.time().format("%H:%M:%S").to_string(), "end":window.end.time().format("%H:%M:%S").to_string(), "screens":screens.len(), "frames_analyzed":sum(&screens,"frames_analyzed")}));
    }
    Ok(json!({"level":"2", "scope":{"day":day,"stream":stream}, "data":{"segments":rows}}))
}

fn list_segment_screens(
    journal: &Path,
    day: &str,
    stream: &str,
    segment: &str,
    diagnostics: &mut dyn GrabDiagnostics,
) -> Result<Value, GrabFailure> {
    let segment_path = crate::reader::require_segment(journal, day, stream, segment)?;
    let mut stems: Vec<_> = available_screen_tokens(&segment_path)?
        .into_iter()
        .map(|token| screen_stem(&token))
        .collect();
    stems.sort();
    let mut screens = Vec::new();
    for stem in stems {
        let bundle = load_bundle(journal, day, stream, segment, &stem, true, diagnostics)?;
        let (position, connector) = parse_screen_filename(&stem);
        screens.push(json!({"screen":normalize_screen_token(&stem), "position":position, "connector":connector, "frames_analyzed":bundle.frame_records.len(), "jsonl":bundle.jsonl_rel, "video":bundle.video_rel, "status":bundle.status}));
    }
    Ok(
        json!({"level":"3", "scope":{"day":day,"stream":stream,"segment":segment}, "data":{"screens":screens}}),
    )
}

fn list_screen_frames(
    journal: &Path,
    day: &str,
    stream: &str,
    segment: &str,
    screen: &str,
    diagnostics: &mut dyn GrabDiagnostics,
) -> Result<Value, GrabFailure> {
    let bundle = load_bundle(journal, day, stream, segment, screen, true, diagnostics)?;
    if bundle.status == "captured but not analyzed" {
        return Err(GrabFailure::runtime(format!(
            "screen {screen} in {segment} is captured but not analyzed"
        )));
    }
    let frames = if bundle.legacy_schema || bundle.header_only {
        Vec::new()
    } else {
        bundle
            .frame_records
            .iter()
            .map(|frame| frame_view(&bundle, frame))
            .collect::<Result<Vec<_>, _>>()?
    };
    let errors = if bundle.legacy_schema || bundle.header_only {
        0
    } else {
        bundle
            .frame_records
            .iter()
            .filter(|frame| frame.get("error").is_some())
            .count()
    };
    Ok(
        json!({"level":"4", "scope":scope(day,stream,segment,screen), "data":{"summary":{"frames_analyzed":bundle.frame_records.len(),"error_frames":errors,"legacy_schema":bundle.legacy_schema,"video_present":bundle.video_path.is_some()},"frames":frames}}),
    )
}

fn show_frame_metadata(
    journal: &Path,
    day: &str,
    stream: &str,
    segment: &str,
    screen: &str,
    id: i64,
    diagnostics: &mut dyn GrabDiagnostics,
) -> Result<Value, GrabFailure> {
    let bundle = load_analyzed_bundle(journal, day, stream, segment, screen, diagnostics)?;
    let frame = bundle.frame_index.get(&id).ok_or_else(|| {
        GrabFailure::runtime(format!("frame id {id} not found in {screen} for {segment}"))
    })?;
    Ok(
        json!({"level":"5a", "scope":scope_with_frame(day,stream,segment,screen,id), "data":{"source":source(&bundle),"frame":frame,"computed":{"abs_time":frame_abs_time(bundle.window.start, frame.get("timestamp").unwrap_or(&Value::from(0.0)))?,"notes":frame_notes(frame)}}}),
    )
}

// This directly mirrors the five-token grab selection plus save options; a
// one-use carrier type would obscure rather than simplify the payload boundary.
#[allow(clippy::too_many_arguments)]
fn save_frame_images(
    journal: &Path,
    day: &str,
    stream: &str,
    segment: &str,
    screen: &str,
    ids: &[i64],
    out: &Path,
    force: bool,
    diagnostics: &mut dyn GrabDiagnostics,
) -> Result<Value, GrabFailure> {
    let bundle = load_analyzed_bundle(journal, day, stream, segment, screen, diagnostics)?;
    let video = bundle.video_path.as_ref().ok_or_else(|| GrabFailure::runtime(if bundle.jsonl_rel.is_some() { format!("raw video has been purged by retention; metadata-only access remains via: journal grab {day} {stream} {segment} {screen} {}", ids.iter().map(ToString::to_string).collect::<Vec<_>>().join(",")) } else { format!("raw video not found for screen {screen} in {segment}") }))?;
    let selected: Vec<_> = ids
        .iter()
        .map(|id| {
            bundle.frame_index.get(id).cloned().ok_or_else(|| {
                GrabFailure::runtime(format!("frame id {id} not found in {screen} for {segment}"))
            })
        })
        .collect::<Result<_, _>>()?;
    let targets = resolve_output_paths(out, ids)?;
    refuse_conflicts(&targets, force)?;
    let decoded = decode_frames(video, ids)?;
    let missing: Vec<_> = ids
        .iter()
        .zip(&decoded)
        .filter_map(|(id, image)| image.is_none().then_some(*id))
        .collect();
    if !missing.is_empty() {
        return Err(GrabFailure::runtime(format!(
            "failed to decode frame ids: {}",
            missing
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }
    let mut saved = Vec::new();
    for ((frame, image), target) in selected.iter().zip(decoded.iter()).zip(&targets) {
        save_frame(image.as_ref().expect("checked missing frames"), target)?;
        let mut view = frame_view(&bundle, frame)?;
        view.as_object_mut().expect("frame view object").insert(
            "path".to_owned(),
            Value::String(target.display().to_string()),
        );
        let mut object = view.as_object().expect("frame view object").clone();
        let path = object.remove("path").expect("path inserted");
        let mut saved_item = Map::new();
        saved_item.insert("path".to_owned(), path);
        saved_item.extend(object);
        saved.push(Value::Object(saved_item));
    }
    Ok(
        json!({"level":if ids.len()==1 {"5b"} else {"5c"}, "scope":scope_with_frames(day,stream,segment,screen,ids), "data":{"source":source(&bundle),"saved":saved}}),
    )
}

fn load_analyzed_bundle(
    journal: &Path,
    day: &str,
    stream: &str,
    segment: &str,
    screen: &str,
    diagnostics: &mut dyn GrabDiagnostics,
) -> Result<ScreenBundle, GrabFailure> {
    let bundle = load_bundle(journal, day, stream, segment, screen, true, diagnostics)?;
    if bundle.status == "captured but not analyzed" {
        return Err(GrabFailure::runtime(format!(
            "screen {screen} in {segment} is captured but not analyzed"
        )));
    }
    if bundle.legacy_schema {
        return Err(GrabFailure::runtime(
            "screen file uses pre-frame_id schema; frame selection is unavailable",
        ));
    }
    Ok(bundle)
}

fn frame_view(bundle: &ScreenBundle, frame: &Value) -> Result<Value, GrabFailure> {
    Ok(
        json!({"frame_id":frame_id(frame).ok_or_else(|| GrabFailure::runtime("invalid frame id"))?,"timestamp":frame.get("timestamp").cloned().unwrap_or_else(|| json!(0.0)),"abs_time":frame_abs_time(bundle.window.start, frame.get("timestamp").unwrap_or(&json!(0.0)))?,"primary":frame.get("analysis").and_then(|value| value.get("primary")).and_then(Value::as_str).unwrap_or_default(),"notes":frame_notes(frame)}),
    )
}

fn scope(day: &str, stream: &str, segment: &str, screen: &str) -> Value {
    json!({"day":day,"stream":stream,"segment":segment,"screen":screen})
}
fn scope_with_frame(day: &str, stream: &str, segment: &str, screen: &str, frame_id: i64) -> Value {
    json!({"day":day,"stream":stream,"segment":segment,"screen":screen,"frame_id":frame_id})
}
fn scope_with_frames(
    day: &str,
    stream: &str,
    segment: &str,
    screen: &str,
    frame_ids: &[i64],
) -> Value {
    json!({"day":day,"stream":stream,"segment":segment,"screen":screen,"frame_ids":frame_ids})
}
fn source(bundle: &ScreenBundle) -> Value {
    json!({"jsonl":bundle.jsonl_rel,"video":bundle.video_rel})
}
fn sum(rows: &[Value], key: &str) -> usize {
    rows.iter()
        .filter_map(|row| row.get(key).and_then(Value::as_u64))
        .sum::<u64>() as usize
}
fn parse_screen_filename(stem: &str) -> (&str, &str) {
    let parts: Vec<_> = stem.split('_').collect();
    if parts.len() == 3
        && parts[2] == "screen"
        && !parts[0].is_empty()
        && !parts[1].is_empty()
        && parts[0]
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte == b'-')
        && parts[1]
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        (parts[0], parts[1])
    } else {
        ("unknown", "unknown")
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use crate::{GrabRequest, RecordingDiagnostics, run};

    use super::parse_screen_filename;

    #[test]
    fn screen_filename_requires_nonempty_position_and_connector() {
        assert_eq!(
            parse_screen_filename("_DP-3_screen"),
            ("unknown", "unknown")
        );
        assert_eq!(
            parse_screen_filename("center__screen"),
            ("unknown", "unknown")
        );
        assert_eq!(
            parse_screen_filename("center_DP-3_screen"),
            ("center", "DP-3")
        );
    }

    #[test]
    fn browse_and_metadata_levels_share_one_reader() {
        let temp = tempdir().unwrap();
        let segment = temp.path().join("chronicle/20260809/work/120000_300");
        fs::create_dir_all(&segment).unwrap();
        fs::write(
            segment.join("screen.jsonl"),
            "{\"raw\": \"screen.webm\"}\n{\"frame_id\": 7, \"timestamp\": 1.5, \"analysis\": {\"primary\": \"editor\"}}\n",
        ).unwrap();
        let mut diagnostics = RecordingDiagnostics::default();
        for tokens in [
            vec![],
            vec!["20260809".into()],
            vec!["20260809".into(), "work".into()],
            vec!["20260809".into(), "work".into(), "120000_300".into()],
            vec![
                "20260809".into(),
                "work".into(),
                "120000_300".into(),
                "screen".into(),
            ],
        ] {
            let output = run(
                temp.path(),
                GrabRequest {
                    tokens,
                    ..GrabRequest::default()
                },
                &mut diagnostics,
            )
            .unwrap();
            assert!(output.payload.get("level").is_some());
        }
        let output = run(
            temp.path(),
            GrabRequest {
                tokens: vec![
                    "20260809".into(),
                    "work".into(),
                    "120000_300".into(),
                    "screen".into(),
                    "7".into(),
                ],
                ..GrabRequest::default()
            },
            &mut diagnostics,
        )
        .unwrap();
        assert_eq!(output.payload["level"], "5a");
        assert_eq!(
            output.payload["data"]["computed"]["abs_time"],
            "2026-08-09T12:00:01.500000"
        );
    }
}
