// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use solstone_core_format::content::conversation::{
    ConversationMedium, MAX_CONVERSATION_ROW_BYTES, file_medium_with_header,
};
use solstone_core_journal_io::{
    JournalRoot,
    media::{FileReadVersion, MediaDirectory},
};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;

use super::SegmentTranscriptReadError as Error;

// Only the first read parses the complete historical row. Later reads seek
// into the validated string and decode one small window directly from disk.
const PASSAGE_BYTES: usize = 8 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct MessageBounds {
    version: FileReadVersion,
    record_start: u64,
    decoded_offset: u64,
    text_end: u64,
    record_end: u64,
}

pub(super) struct Passage {
    pub text: String,
    pub part: Option<TurnPart>,
    pub next_offset: u64,
    pub bounds: Option<MessageBounds>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct TurnPart {
    pub turn_id: String,
    pub byte_offset: u64,
    pub complete: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_notice: Option<String>,
}

pub(super) fn bound_read<T>(
    path: &Path,
    relative: &str,
    medium: ConversationMedium,
    read: impl FnOnce(&mut File, FileReadVersion) -> Result<T, Error>,
) -> Result<T, Error> {
    read_if_medium(path, relative, medium, read)?.ok_or(Error::Changed)
}

pub(super) fn read_if_medium<T>(
    path: &Path,
    relative: &str,
    medium: ConversationMedium,
    read: impl FnOnce(&mut File, FileReadVersion) -> Result<T, Error>,
) -> Result<Option<T>, Error> {
    read_classified(path, relative, Some(medium), |file, version, _| {
        read(file, version)
    })
}

/// Read exactly the version whose medium was resolved. Owner-local readers
/// accept every medium and carry it beside the text they render.
pub(super) fn read_classified<T>(
    path: &Path,
    relative: &str,
    required: Option<ConversationMedium>,
    read: impl FnOnce(&mut File, FileReadVersion, ConversationMedium) -> Result<T, Error>,
) -> Result<Option<T>, Error> {
    let parent = path.parent().ok_or(Error::Io)?;
    let root_path = parent
        .ancestors()
        .find(|p| p.file_name().is_some_and(|n| n == "chronicle"))
        .and_then(Path::parent)
        .ok_or(Error::Io)?;
    let root = JournalRoot::open(root_path).map_err(|_| Error::Io)?;
    let directory = MediaDirectory::open(
        Arc::new(root),
        parent.strip_prefix(root_path).map_err(|_| Error::Io)?,
    )
    .map_err(|_| Error::Io)?;
    let name = path.file_name().and_then(|s| s.to_str()).ok_or(Error::Io)?;
    directory
        .read_versioned(name.as_ref(), |file, version| {
            let check = |file: &mut File| -> Result<Option<ConversationMedium>, Error> {
                file.seek(SeekFrom::Start(0)).map_err(|_| Error::Io)?;
                let header = solstone_core_processing_record::read_jsonl_header_from(file);
                if header.is_none() && required == Some(ConversationMedium::Spoken) {
                    return Err(Error::Io);
                }
                // A native chat stream may contain an explicitly transcribed
                // recording. A typed speech header on this handle takes priority.
                use solstone_core_processing_record::{header_is_early_transcribe, header_names_transcribe, try_jsonl_has_row_with_key_from};
                let mut spoken = false;
                if let Some(map) = header.as_ref().and_then(serde_json::Value::as_object) {
                    spoken = header_names_transcribe(map);
                    if !spoken && header_is_early_transcribe(map) {
                        for extension in solstone_core_processing_record::media::audio_extensions() {
                            let sibling = Path::new(name).with_extension(extension);
                            match directory.read_versioned(sibling.as_os_str(), |_,_| ()) {
                                Ok(()) => {
                                    file.seek(SeekFrom::Start(0)).map_err(|_| Error::Io)?;
                                    spoken = try_jsonl_has_row_with_key_from(&mut *file, solstone_core_processing_record::vocab::AUDIO_TRANSCRIPT_ROW_KEY).map_err(|_| Error::Io)?;
                                    break;
                                }
                                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                                Err(_) => return Err(Error::Changed),
                            }
                        }
                    }
                }
                let current = file_medium_with_header(path, relative, || spoken, || header);
                // Only positive written evidence can settle a voice candidate
                // as non-acoustic. Missing or unreadable historical evidence
                // goes through catch-up's bounded retry path.
                if required == Some(ConversationMedium::Spoken) && matches!(current, None | Some(ConversationMedium::Unresolved)) {
                    return Err(Error::Changed);
                }
                Ok(current)
            };
            let Some(medium) = check(file)? else { return Ok(None); };
            if required.is_some_and(|wanted| wanted != medium) { return Ok(None); }
            let result = read(file, version, medium)?;
            if check(file)? != Some(medium) {
                return Err(Error::Changed);
            }
            Ok(Some(result))
        })
        .map_err(|_| Error::Changed)?
}

pub(super) fn passage(
    file: &mut File,
    version: FileReadVersion,
    offset: u64,
    bounds: Option<MessageBounds>,
    name: &str,
    origin: &str,
    untimed: bool,
) -> Result<Passage, Error> {
    match bounds {
        Some(bounds) if bounds.version != version => Err(Error::Changed),
        Some(bounds) => read(file, offset, bounds, name),
        None => first(file, version, offset, name, origin, untimed),
    }
}

fn first(
    file: &mut File,
    version: FileReadVersion,
    offset: u64,
    name: &str,
    segment_origin: &str,
    untimed: bool,
) -> Result<Passage, Error> {
    file.seek(SeekFrom::Start(offset)).map_err(|_| Error::Io)?;
    let mut line = Vec::new();
    BufReader::new(file.take((MAX_CONVERSATION_ROW_BYTES + 1) as u64))
        .read_until(b'\n', &mut line)
        .map_err(|_| Error::Io)?;
    if line.len() > MAX_CONVERSATION_ROW_BYTES {
        return Err(Error::RecordTooLarge);
    }
    let fields: BTreeMap<String, &RawValue> =
        serde_json::from_slice(&line).map_err(|_| Error::Io)?;
    let end = offset + line.len() as u64;
    let skip = || Passage {
        text: String::new(),
        part: None,
        next_offset: end,
        bounds: None,
    };
    if !fields.contains_key("start") {
        return Ok(skip());
    }
    let Some(raw) = fields
        .get("text")
        .map(|r| r.get())
        .filter(|s| s.starts_with('"'))
    else {
        return Ok(skip());
    };
    if raw.len() == 2 {
        return Ok(skip());
    }
    let start = offset + (raw.as_ptr() as usize - line.as_ptr() as usize) as u64 + 1;
    let bounds = MessageBounds {
        version,
        record_start: offset,
        decoded_offset: 0,
        text_end: start + raw.len() as u64 - 2,
        record_end: end,
    };
    let mut passage = read(file, start, bounds, name)?;
    // Use the ordinary turn formatter for time, attribution and source limits.
    // A marker stands in for the body while assembling its display prefix.
    let mut row = serde_json::Map::new();
    for key in [
        "start",
        "sent_at",
        "speaker",
        "partial_input",
        "participation_partial_input",
        "story",
    ] {
        if let Some(value) = fields.get(key) {
            if value.get().len() > 1024 && matches!(key, "speaker" | "start" | "sent_at") {
                if key != "speaker" {
                    row.insert(key.into(), serde_json::Value::Null);
                }
                continue;
            }
            row.insert(
                key.into(),
                serde_json::from_str(value.get()).map_err(|_| Error::Io)?,
            );
        }
    }
    const MARKER: &str = "\u{0}";
    row.insert("text".into(), serde_json::Value::from(MARKER));
    let projected = super::project_line(
        segment_origin,
        name,
        false,
        untimed,
        &serde_json::Value::Object(row).to_string(),
    );
    if let Some((prefix, suffix)) = projected.rsplit_once(MARKER) {
        passage.text.insert_str(0, prefix);
        if !suffix.is_empty()
            && let Some(part) = passage.part.as_mut()
        {
            part.source_notice = Some(suffix.trim_start().to_owned());
        }
    }
    Ok(passage)
}

fn read(file: &mut File, offset: u64, bounds: MessageBounds, name: &str) -> Result<Passage, Error> {
    file.seek(SeekFrom::Start(offset)).map_err(|_| Error::Io)?;
    let len = file.metadata().map_err(|_| Error::Io)?.len();
    if offset > bounds.text_end || bounds.text_end >= bounds.record_end || bounds.record_end > len {
        return Err(Error::Changed);
    }
    let count = (bounds.text_end - offset).min(PASSAGE_BYTES as u64) as usize;
    let mut bytes = vec![0; count];
    file.read_exact(&mut bytes).map_err(|_| Error::Io)?;
    let text = match std::str::from_utf8(&bytes) {
        Ok(text) => text,
        Err(error) if error.error_len().is_none() => {
            std::str::from_utf8(&bytes[..error.valid_up_to()]).map_err(|_| Error::Io)?
        }
        Err(_) => return Err(Error::Io),
    };
    let raw = text.as_bytes();
    let mut pos = 0;
    while pos < raw.len() {
        if raw[pos] != b'\\' {
            pos += 1;
            continue;
        }
        let Some(escape) = raw.get(pos + 1) else {
            break;
        };
        let len = if *escape == b'u' {
            let Some(digits) = text.get(pos + 2..pos + 6) else {
                break;
            };
            let value = u16::from_str_radix(digits, 16).map_err(|_| Error::Io)?;
            if (0xd800..=0xdbff).contains(&value) {
                12
            } else {
                6
            }
        } else {
            2
        };
        if pos + len > raw.len() {
            break;
        }
        pos += len;
    }
    if pos == 0 && count != 0 {
        return Err(Error::Io);
    }
    let decoded: String =
        serde_json::from_str(&format!("\"{}\"", &text[..pos])).map_err(|_| Error::Io)?;
    let next = offset + pos as u64;
    let complete = next == bounds.text_end;
    let part = TurnPart {
        turn_id: format!("{name}#{}", bounds.record_start),
        byte_offset: bounds.decoded_offset,
        complete,
        source_notice: None,
    };
    let bounds = MessageBounds {
        decoded_offset: bounds.decoded_offset + decoded.len() as u64,
        ..bounds
    };
    Ok(Passage {
        text: decoded,
        part: Some(part),
        next_offset: if complete { bounds.record_end } else { next },
        bounds: (!complete).then_some(bounds),
    })
}
