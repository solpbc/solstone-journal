// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;

use serde_json::{Map, Value, json};

use crate::encode::{refuse_conflicts_in, save_frame_in};
use crate::error::GrabFailure;
use crate::extract::decode_frames_in;
use crate::reader::{
    ScreenBundle, ScreenStatus, available_days, available_screen_tokens, available_segments,
    frame_id, load_bundle, normalize_screen_token, require_day, require_stream, screen_stem,
    streams_except_health,
};
use crate::render::{HumanHints, frame_notes, raw_frame_error_text, render_in};
use crate::request::{GrabDiagnostics, GrabOutput, GrabRequest};
use crate::selection::{parse_frame_id_token_in, resolve_output_paths_in};
use crate::time::{frame_abs_time_in, segment_window_in};

pub(crate) fn run_in(
    locale: &str,
    journal: &Path,
    request: GrabRequest,
    diagnostics: &mut dyn GrabDiagnostics,
) -> Result<GrabOutput, GrabFailure> {
    if request.tokens.len() > 5 {
        return Err(GrabFailure::Usage(solstone_core_cli_copy::render_for(
            locale,
            "journal.cli.grab.error.too.many.tokens",
            &[],
        )));
    }
    if request.force && request.out.is_none() {
        return Err(GrabFailure::Usage(solstone_core_cli_copy::render_for(
            locale,
            "journal.cli.grab.error.force.requires.out",
            &[
                (
                    "force_flag",
                    solstone_core_cli_copy::Value::String("--force"),
                ),
                ("out_flag", solstone_core_cli_copy::Value::String("--out")),
            ],
        )));
    }
    if request.out.is_some() && request.tokens.len() != 5 {
        return Err(GrabFailure::Usage(solstone_core_cli_copy::render_for(
            locale,
            "journal.cli.grab.error.out.requires.five.tokens",
            &[("out_flag", solstone_core_cli_copy::Value::String("--out"))],
        )));
    }
    if let Some(out) = &request.out {
        let _ = resolve_output_paths_in(locale, out, &[1])?;
    }
    let (payload, hints) = match request.tokens.as_slice() {
        [] => (
            list_available_days(locale, journal, diagnostics)?,
            HumanHints::default(),
        ),
        [day] => (
            list_day_streams(locale, journal, day, diagnostics)?,
            HumanHints::default(),
        ),
        [day, stream] => (
            list_stream_segments(locale, journal, day, stream, diagnostics)?,
            HumanHints::default(),
        ),
        [day, stream, segment] => {
            list_segment_screens(locale, journal, day, stream, segment, diagnostics)?
        }
        [day, stream, segment, screen] => {
            list_screen_frames(locale, journal, day, stream, segment, screen, diagnostics)?
        }
        [day, stream, segment, screen, token] => {
            let ids = parse_frame_id_token_in(locale, token)?;
            if ids.len() > 1 && request.out.is_none() {
                return Err(GrabFailure::Usage(solstone_core_cli_copy::render_for(
                    locale,
                    "journal.cli.grab.error.multiple.ids.require.out",
                    &[("out_flag", solstone_core_cli_copy::Value::String("--out"))],
                )));
            }
            if let Some(out) = request.out {
                (
                    save_frame_images(
                        locale,
                        journal,
                        day,
                        stream,
                        segment,
                        screen,
                        &ids,
                        &out,
                        request.force,
                        diagnostics,
                    )?,
                    HumanHints::default(),
                )
            } else {
                show_frame_metadata(
                    locale,
                    journal,
                    day,
                    stream,
                    segment,
                    screen,
                    ids[0],
                    diagnostics,
                )?
            }
        }
        _ => unreachable!("length checked above"),
    };
    let human = render_in(locale, &payload, &hints)?;
    Ok(GrabOutput { payload, human })
}

fn list_available_days(
    locale: &str,
    journal: &Path,
    diagnostics: &mut dyn GrabDiagnostics,
) -> Result<Value, GrabFailure> {
    let mut days = Vec::new();
    for day in available_days(journal)? {
        let streams = list_day_streams(locale, journal, &day, diagnostics)?
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
    locale: &str,
    journal: &Path,
    day: &str,
    diagnostics: &mut dyn GrabDiagnostics,
) -> Result<Value, GrabFailure> {
    let day_path = require_day(locale, journal, day)?;
    let mut streams = Vec::new();
    for stream in streams_except_health(&day_path)? {
        let segments = list_stream_segments(locale, journal, day, &stream, diagnostics)?
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
    locale: &str,
    journal: &Path,
    day: &str,
    stream: &str,
    diagnostics: &mut dyn GrabDiagnostics,
) -> Result<Value, GrabFailure> {
    let stream_path = require_stream(locale, journal, day, stream)?;
    let mut rows = Vec::new();
    for segment in available_segments(&stream_path)? {
        let window = match segment_window_in(locale, day, &segment) {
            Ok(window) => window,
            Err(_) => continue,
        };
        let (segment_payload, _) =
            list_segment_screens(locale, journal, day, stream, &segment, diagnostics)?;
        let screens = segment_payload
            .get("data")
            .and_then(|data| data.get("screens"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if screens.is_empty() {
            continue;
        }
        rows.push(json!({"segment":segment, "start":window.start.time().strftime("%H:%M:%S").to_string(), "end":window.end.time().strftime("%H:%M:%S").to_string(), "screens":screens.len(), "frames_analyzed":sum(&screens,"frames_analyzed")}));
    }
    Ok(json!({"level":"2", "scope":{"day":day,"stream":stream}, "data":{"segments":rows}}))
}

fn list_segment_screens(
    locale: &str,
    journal: &Path,
    day: &str,
    stream: &str,
    segment: &str,
    diagnostics: &mut dyn GrabDiagnostics,
) -> Result<(Value, HumanHints), GrabFailure> {
    let segment_path = crate::reader::require_segment(locale, journal, day, stream, segment)?;
    let mut stems: Vec<_> = available_screen_tokens(&segment_path)?
        .into_iter()
        .map(|token| screen_stem(&token))
        .collect();
    stems.sort();
    let mut screens = Vec::new();
    let mut statuses = Vec::new();
    for stem in stems {
        let bundle = load_bundle(
            locale,
            journal,
            day,
            stream,
            segment,
            &stem,
            true,
            diagnostics,
        )?;
        let (position, connector) = parse_screen_filename(&stem);
        statuses.push(bundle.status);
        screens.push(json!({"screen":normalize_screen_token(&stem), "position":position, "connector":connector, "frames_analyzed":bundle.frame_records.len(), "jsonl":bundle.jsonl_rel, "video":bundle.video_rel, "status":bundle.status_json()}));
    }
    Ok((
        json!({"level":"3", "scope":{"day":day,"stream":stream,"segment":segment}, "data":{"screens":screens}}),
        HumanHints {
            screen_statuses: statuses,
            ..HumanHints::default()
        },
    ))
}

fn list_screen_frames(
    locale: &str,
    journal: &Path,
    day: &str,
    stream: &str,
    segment: &str,
    screen: &str,
    diagnostics: &mut dyn GrabDiagnostics,
) -> Result<(Value, HumanHints), GrabFailure> {
    let bundle = load_bundle(
        locale,
        journal,
        day,
        stream,
        segment,
        screen,
        true,
        diagnostics,
    )?;
    if bundle.status == ScreenStatus::CapturedNotAnalyzed {
        return Err(GrabFailure::runtime(solstone_core_cli_copy::render_for(
            locale,
            "journal.cli.grab.error.captured.not.analyzed",
            &[
                ("screen", solstone_core_cli_copy::Value::String(screen)),
                ("segment", solstone_core_cli_copy::Value::String(segment)),
            ],
        )));
    }
    let (frames, frame_error_texts) = if bundle.legacy_schema || bundle.header_only {
        (Vec::new(), Vec::new())
    } else {
        let mut frames = Vec::with_capacity(bundle.frame_records.len());
        let mut errors_text = Vec::with_capacity(bundle.frame_records.len());
        for frame in &bundle.frame_records {
            frames.push(frame_view(locale, &bundle, frame)?);
            errors_text.push(raw_frame_error_text(frame));
        }
        (frames, errors_text)
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
    let mut summary = json!({"frames_analyzed":bundle.frame_records.len(),"error_frames":errors,"legacy_schema":bundle.legacy_schema,"video_present":bundle.video_path.is_some()});
    // The key appears only when there is a recorded cause to name, so a present
    // video keeps the payload it has always had.
    if let (Some(reason), Some(map)) = (bundle.missing_video_reason_json(), summary.as_object_mut())
    {
        map.insert("video_missing_reason".into(), Value::from(reason));
    }
    Ok((
        json!({"level":"4", "scope":scope(day,stream,segment,screen), "data":{"summary":summary,"frames":frames}}),
        HumanHints {
            missing_video_cause: bundle.missing_video_cause,
            frame_error_texts,
            ..HumanHints::default()
        },
    ))
}

#[allow(clippy::too_many_arguments)]
fn show_frame_metadata(
    locale: &str,
    journal: &Path,
    day: &str,
    stream: &str,
    segment: &str,
    screen: &str,
    id: i64,
    diagnostics: &mut dyn GrabDiagnostics,
) -> Result<(Value, HumanHints), GrabFailure> {
    let bundle = load_analyzed_bundle(locale, journal, day, stream, segment, screen, diagnostics)?;
    let frame = bundle.frame_index.get(&id).ok_or_else(|| {
        GrabFailure::runtime(solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.grab.error.frame.not.found",
            [
                ("frame_id", &id),
                ("screen", &screen),
                ("segment", &segment),
            ],
        ))
    })?;
    let single_frame_error_text = raw_frame_error_text(frame);
    Ok((
        json!({"level":"5a", "scope":scope_with_frame(day,stream,segment,screen,id), "data":{"source":source(&bundle),"frame":frame,"computed":{"abs_time":frame_abs_time_in(locale, bundle.window.start, frame.get("timestamp").unwrap_or(&Value::from(0.0)))?,"notes":frame_notes(frame)}}}),
        HumanHints {
            single_frame_error_text,
            ..HumanHints::default()
        },
    ))
}

// This directly mirrors the five-token grab selection plus save options; a
// one-use carrier type would obscure rather than simplify the payload boundary.
#[allow(clippy::too_many_arguments)]
fn save_frame_images(
    locale: &str,
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
    let bundle = load_analyzed_bundle(locale, journal, day, stream, segment, screen, diagnostics)?;
    let video = bundle.video_path.as_ref().ok_or_else(|| {
        GrabFailure::runtime(match bundle.missing_video_cause {
            Some(cause) => {
                let reason = cause.render_reason_in(locale);
                let id_list = ids
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(",");
                let command =
                    format!("solstone journal grab {day} {stream} {segment} {screen} {id_list}");
                solstone_core_cli_copy::render_for(
                    locale,
                    "journal.cli.grab.error.video.missing.with.reason",
                    &[
                        ("reason", solstone_core_cli_copy::Value::String(&reason)),
                        ("command", solstone_core_cli_copy::Value::String(&command)),
                    ],
                )
            }
            None => solstone_core_cli_copy::render_for(
                locale,
                "journal.cli.grab.error.video.missing",
                &[
                    ("screen", solstone_core_cli_copy::Value::String(screen)),
                    ("segment", solstone_core_cli_copy::Value::String(segment)),
                ],
            ),
        })
    })?;
    let selected: Vec<_> = ids
        .iter()
        .map(|id| {
            bundle.frame_index.get(id).cloned().ok_or_else(|| {
                GrabFailure::runtime(solstone_core_cli_copy::display_for(
                    locale,
                    "journal.cli.grab.error.frame.not.found",
                    [("frame_id", id), ("screen", &screen), ("segment", &segment)],
                ))
            })
        })
        .collect::<Result<_, _>>()?;
    let targets = resolve_output_paths_in(locale, out, ids)?;
    refuse_conflicts_in(locale, &targets, force)?;
    let decoded = decode_frames_in(locale, video, ids)?;
    let missing: Vec<_> = ids
        .iter()
        .zip(&decoded)
        .filter_map(|(id, image)| image.is_none().then_some(*id))
        .collect();
    if !missing.is_empty() {
        let missing_ids = missing
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        return Err(GrabFailure::runtime(solstone_core_cli_copy::render_for(
            locale,
            "journal.cli.grab.error.decode.failed.ids",
            &[("ids", solstone_core_cli_copy::Value::String(&missing_ids))],
        )));
    }
    let mut saved = Vec::new();
    for ((frame, image), target) in selected.iter().zip(decoded.iter()).zip(&targets) {
        save_frame_in(
            locale,
            image.as_ref().expect("checked missing frames"),
            target,
        )?;
        let mut view = frame_view(locale, &bundle, frame)?;
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
    locale: &str,
    journal: &Path,
    day: &str,
    stream: &str,
    segment: &str,
    screen: &str,
    diagnostics: &mut dyn GrabDiagnostics,
) -> Result<ScreenBundle, GrabFailure> {
    let bundle = load_bundle(
        locale,
        journal,
        day,
        stream,
        segment,
        screen,
        true,
        diagnostics,
    )?;
    if bundle.status == ScreenStatus::CapturedNotAnalyzed {
        return Err(GrabFailure::runtime(solstone_core_cli_copy::render_for(
            locale,
            "journal.cli.grab.error.captured.not.analyzed",
            &[
                ("screen", solstone_core_cli_copy::Value::String(screen)),
                ("segment", solstone_core_cli_copy::Value::String(segment)),
            ],
        )));
    }
    if bundle.legacy_schema {
        return Err(GrabFailure::runtime(solstone_core_cli_copy::render_for(
            locale,
            "journal.cli.grab.error.legacy.schema",
            &[],
        )));
    }
    Ok(bundle)
}

fn frame_view(locale: &str, bundle: &ScreenBundle, frame: &Value) -> Result<Value, GrabFailure> {
    Ok(
        json!({"frame_id":frame_id(frame).ok_or_else(|| GrabFailure::runtime(solstone_core_cli_copy::render_for(locale, "journal.cli.grab.error.invalid.frame.id", &[])))?,"timestamp":frame.get("timestamp").cloned().unwrap_or_else(|| json!(0.0)),"abs_time":frame_abs_time_in(locale, bundle.window.start, frame.get("timestamp").unwrap_or(&json!(0.0)))?,"primary":frame.get("analysis").and_then(|value| value.get("primary")).and_then(Value::as_str).unwrap_or_default(),"notes":frame_notes(frame)}),
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

    #[test]
    fn the_missing_reason_key_appears_only_when_a_video_is_missing() {
        let temp = tempdir().unwrap();
        let segment = temp.path().join("chronicle/20260809/work/120000_300");
        fs::create_dir_all(&segment).unwrap();
        for screen in ["kept", "gone"] {
            fs::write(
                segment.join(format!("{screen}_screen.jsonl")),
                format!(
                    "{{\"raw\": \"{screen}_screen.mp4\"}}\n{{\"frame_id\": 1, \"timestamp\": 0, \"analysis\": {{\"primary\": \"editor\"}}}}\n"
                ),
            )
            .unwrap();
        }
        fs::write(segment.join("kept_screen.mp4"), b"stand-in for a video").unwrap();

        let summary = |screen: &str| {
            let mut diagnostics = RecordingDiagnostics::default();
            run(
                temp.path(),
                GrabRequest {
                    tokens: vec![
                        "20260809".into(),
                        "work".into(),
                        "120000_300".into(),
                        screen.into(),
                    ],
                    ..GrabRequest::default()
                },
                &mut diagnostics,
            )
            .unwrap()
            .payload["data"]["summary"]
                .clone()
        };

        let kept = summary("kept");
        assert_eq!(kept["video_present"], true);
        assert!(
            kept.get("video_missing_reason").is_none(),
            "a present video adds no key to the payload: {kept}"
        );

        let gone = summary("gone");
        assert_eq!(gone["video_present"], false);
        assert_eq!(
            gone["video_missing_reason"],
            "the original video is no longer in your journal"
        );
    }
}
