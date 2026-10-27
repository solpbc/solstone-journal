// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Audio archive command execution and dispatch.

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::json;
use sha2::{Digest, Sha256};
use solstone_core_import::audio_archive::{
    ArchiveFault, ArchiveProgressPhase, ArchiveStore, AudioArchiveError, AudioArchiveMaterializer,
    AudioArchiveMemberInput, AudioArchiveProgress, AudioArchivePublishOptions,
    AudioArchivePublishSeams, AudioArchiveSettings, MemberDisposition, prepare_audio_archive_plan,
    publish_audio_archive, read_matching_archive_run,
};
use solstone_core_import::cli_render::CliRun;

use super::folder::scan_folder_members;
use super::manifest::read_manifest_members;
use super::slicer::HostAudioArchiveMaterializer;

#[derive(Clone, Debug, Default)]
pub struct AudioArchiveDispatchOptions {
    pub path: PathBuf,
    pub source_zone: Option<String>,
    pub resume: Option<String>,
    /// A caller's reviewed member/time/zone identity, checked before admission.
    pub expected_identity: Option<String>,
    pub expected_owner_zone: Option<String>,
    pub dry_run: bool,
    pub json: bool,
}

pub fn compute_archive_identity(
    is_manifest: bool,
    source_zone: Option<&str>,
    members: &[AudioArchiveMemberInput],
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"solstone-audio-archive-v1\0");
    if is_manifest {
        hasher.update(b"manifest\0");
    } else {
        hasher.update(b"folder\0");
    }
    if let Some(zone) = source_zone {
        hasher.update(zone.as_bytes());
    }
    hasher.update(b"\0");

    let mut sorted_members: Vec<_> = members.iter().collect();
    sorted_members.sort_by(|a, b| a.original_relative_name.cmp(&b.original_relative_name));

    for member in &sorted_members {
        hasher.update(member.original_relative_name.as_bytes());
        hasher.update(b"\0");
        hasher.update(member.sha256.as_bytes());
        hasher.update(b"\0");
        hasher.update(member.byte_len.to_string().as_bytes());
        hasher.update(b"\0");
    }

    format!("{:x}", hasher.finalize())
}

pub fn run_audio_archive(options: AudioArchiveDispatchOptions, journal: &Path) -> CliRun {
    run_audio_archive_with(options, journal, None, None)
}

pub fn run_audio_archive_with(
    options: AudioArchiveDispatchOptions,
    journal: &Path,
    custom_materializer: Option<Box<dyn AudioArchiveMaterializer>>,
    custom_fault: Option<ArchiveFault>,
) -> CliRun {
    match save(options, journal, custom_materializer, custom_fault) {
        Ok(run) => run,
        Err(error) => failure("", &format!("audio archive failed: {error}\n"), 1),
    }
}

fn save(
    options: AudioArchiveDispatchOptions,
    journal: &Path,
    custom_materializer: Option<Box<dyn AudioArchiveMaterializer>>,
    fault: Option<ArchiveFault>,
) -> Result<CliRun, AudioArchiveError> {
    let path = &options.path;
    let metadata = std::fs::metadata(path)?;
    let is_manifest = metadata.is_file();
    let source_root = if is_manifest {
        path.parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf()
    } else {
        path.clone()
    };
    let owner_zone = solstone_core_journal_config::owner_zone(journal);
    if options
        .expected_owner_zone
        .as_deref()
        .is_some_and(|expected| expected != owner_zone.name())
    {
        return Err(AudioArchiveError::Other(
            "the time zone changed after preview; preview it again".into(),
        ));
    }
    // An explicit resume uses the admitted plan, even when the source folder
    // changed. Only unsaved parts reopen their frozen source and verify bytes.
    let (members, settings) = if let Some(id) = &options.resume {
        {
            let frozen = ArchiveStore::open(journal, id, false)?.settings()?;
            (
                Vec::new(),
                AudioArchiveSettings {
                    qualifier: frozen.qualifier,
                    archive_identity: frozen.archive_identity,
                },
            )
        }
    } else {
        let members = if is_manifest {
            read_manifest_members(path, &source_root, options.source_zone.as_deref())
        } else {
            let zone = options.source_zone.as_deref().ok_or_else(|| {
                AudioArchiveError::Other("a folder requires --source-zone".into())
            })?;
            scan_folder_members(path, zone)
        }
        .map_err(|error| AudioArchiveError::Other(error.to_string()))?;
        let settings = AudioArchiveSettings {
            qualifier: "archive".into(),
            archive_identity: compute_archive_identity(
                is_manifest,
                options.source_zone.as_deref(),
                &members,
            ),
        };
        (members, settings)
    };
    if options
        .expected_identity
        .as_ref()
        .is_some_and(|identity| identity != &settings.archive_identity)
    {
        return Err(AudioArchiveError::Other(
            "the archive changed after preview; preview it again".into(),
        ));
    }
    if options.dry_run {
        if options.resume.is_some() {
            return Err(AudioArchiveError::Other(
                "use --audio-archive-status to inspect an admitted run".into(),
            ));
        }
        let archive_identity = settings.archive_identity.clone();
        let plan = prepare_audio_archive_plan(journal, &members, owner_zone, settings)?;
        return Ok(success(if options.json {
            format!(
                "{}\n",
                json!({"operation":"dry_run", "archive_identity":archive_identity, "size":plan.size,
                "members":plan.size.members, "source_bytes":plan.size.source_bytes,
                "bytes":plan.size.bytes, "days":plan.size.days, "tiles":plan.size.tiles,
                "span":plan.size.span, "left_out":plan.size.left_out})
            )
        } else {
            format!(
                "audio archive preview: {} members, {} source bytes, {} days, {} tiles\nspan: {}\nleft out: {:?}\n",
                plan.size.members,
                plan.size.source_bytes,
                plan.size.days,
                plan.size.tiles,
                span_text(plan.size.span.as_ref()),
                plan.size.left_out
            )
        }));
    }
    let matching = match &options.resume {
        Some(id) => Some(id.clone()),
        None => read_matching_archive_run(journal, &settings.archive_identity)?,
    };
    let before = matching
        .as_deref()
        .map(|id| ArchiveStore::open(journal, id, false)?.status())
        .transpose()?;
    let materializer = custom_materializer
        .unwrap_or_else(|| Box::new(HostAudioArchiveMaterializer::new(Some(source_root))));
    let json_output = options.json;
    let seams = || AudioArchivePublishSeams {
        fault,
        // Sensing discovers the indexed pending frontier. A saved raw is not
        // an observed/processed segment, and a bus burst is not a work queue.
        emit_observed: None,
        heartbeat_spawner: None,
        progress: Some(Arc::new(move |event| write_progress(event, json_output))),
    };
    let mut existing = matching;
    // A corrected implicit manifest may admit a new plan. Explicit resume
    // always retains the old plan; saved identities remain held either way.
    let result = publish_audio_archive(
        journal,
        AudioArchivePublishOptions {
            import_id: existing.as_deref(),
            members: members.clone(),
            owner_zone,
            settings: settings.clone(),
            materializer: &*materializer,
            source_timestamp: None,
        },
        Some(seams()),
    );
    let result = match result {
        Err(AudioArchiveError::SettingsMismatch(_)) if options.resume.is_none() => {
            existing = None;
            publish_audio_archive(
                journal,
                AudioArchivePublishOptions {
                    import_id: None,
                    members,
                    owner_zone,
                    settings,
                    materializer: &*materializer,
                    source_timestamp: None,
                },
                Some(seams()),
            )?
        }
        other => other?,
    };
    let status = ArchiveStore::open(journal, &result.import_id, false)?.status()?;
    let operation = if existing.is_none() {
        "new"
    } else if before.as_ref().is_some_and(|s| s.save_complete)
        && result.tiles_saved == 0
        && result.parts_saved == 0
    {
        "unchanged"
    } else {
        "resume"
    };
    Ok(success(if options.json {
        format!(
            "{}\n",
            json!({"operation":operation, "run_id":result.import_id,
            "tiles_saved":result.tiles_saved, "parts_saved":result.parts_saved,
            "pending_processing":status.pending_processing, "processed_parts":status.processed_parts,
            "unavailable_parts":status.unavailable_parts, "rejected_parts":status.rejected_parts,
            "save_complete":status.save_complete, "left_out":status.size.left_out.values().sum::<usize>(),
            "left_out_counts":status.size.left_out, "status":status})
        )
    } else {
        format!(
            "audio archive {operation}: {}\nsaved {} parts in {} tiles; {} pending processing, {} processed, {} unavailable, {} rejected\nleft out: {:?}\n",
            result.import_id,
            status.saved_parts,
            status.saved_tiles,
            status.pending_processing,
            status.processed_parts,
            status.unavailable_parts,
            status.rejected_parts,
            status.size.left_out
        )
    }))
}

fn span_text(span: Option<&(String, String)>) -> String {
    span.map_or_else(
        || "none".into(),
        |(first, last)| format!("{first} to {last}"),
    )
}

fn write_progress(
    event: &AudioArchiveProgress,
    json_output: bool,
) -> Result<(), AudioArchiveError> {
    let mut stdout = io::stdout().lock();
    if json_output {
        // One compact JSON object per line; saved counters come only from the
        // durable run record after tile publication, never from staging.
        writeln!(
            stdout,
            "{}",
            json!({"event":"archive_progress", "run_id":event.import_id,
            "phase":event.phase, "status":event.status})
        )?;
    } else if event.phase == ArchiveProgressPhase::Admitted {
        writeln!(
            stdout,
            "archive {}: {} members, {} source bytes, {} days, {} tiles\nspan: {}\nleft out: {:?}",
            event.import_id,
            event.status.size.members,
            event.status.size.source_bytes,
            event.status.size.days,
            event.status.size.tiles,
            span_text(event.status.size.span.as_ref()),
            event.status.size.left_out
        )?;
    } else {
        writeln!(
            stdout,
            "saved {} of {} parts ({} tiles)",
            event.status.saved_parts, event.status.total_parts, event.status.saved_tiles
        )?;
    }
    stdout.flush()?;
    Ok(())
}

pub fn run_audio_archive_status(run_id: &str, json: bool, journal: &Path) -> CliRun {
    let store = match ArchiveStore::open(journal, run_id, false) {
        Ok(s) => s,
        Err(e) => return failure("", &format!("audio archive status failed: {e}\n"), 1),
    };
    let status = match store.status() {
        Ok(st) => st,
        Err(e) => return failure("", &format!("audio archive status failed: {e}\n"), 1),
    };

    if json {
        let settings = match store.settings() {
            Ok(settings) => settings,
            Err(error) => {
                return failure("", &format!("audio archive status failed: {error}\n"), 1);
            }
        };
        let mut value = serde_json::to_value(&status).unwrap();
        value["operation"] = json!("status");
        value["run_id"] = json!(run_id);
        value["archive_identity"] = json!(settings.archive_identity);
        let stdout = format!("{}\n", serde_json::to_string_pretty(&value).unwrap());
        success(stdout)
    } else {
        let mut out = format!(
            "audio archive status: run_id={run_id} total_parts={} saved_parts={} saved_tiles={} pending_processing={} processed_parts={} unavailable_parts={} rejected_parts={} save_complete={}\n",
            status.total_parts,
            status.saved_parts,
            status.saved_tiles,
            status.pending_processing,
            status.processed_parts,
            status.unavailable_parts,
            status.rejected_parts,
            status.save_complete,
        );
        if !status.size.left_out.is_empty() {
            out.push_str("left out:\n");
            for (reason, count) in &status.size.left_out {
                out.push_str(&format!("  {reason}: {count}\n"));
            }
        }
        success(out)
    }
}

pub fn run_audio_archive_members(
    run_id: &str,
    offset: u64,
    limit: Option<u32>,
    json: bool,
    journal: &Path,
) -> CliRun {
    let store = match ArchiveStore::open(journal, run_id, false) {
        Ok(s) => s,
        Err(e) => return failure("", &format!("audio archive members failed: {e}\n"), 1),
    };
    let capped_limit = limit.unwrap_or(100).min(1000);
    let members = match store.members(offset, capped_limit) {
        Ok(m) => m,
        Err(e) => return failure("", &format!("audio archive members failed: {e}\n"), 1),
    };

    if json {
        let stdout = format!("{}\n", serde_json::to_string_pretty(&members).unwrap());
        success(stdout)
    } else {
        let mut out = format!(
            "audio archive members: run_id={run_id} offset={offset} count={}\n",
            members.len()
        );
        for member in &members {
            let disp_str = match &member.disposition {
                MemberDisposition::Included => "included",
                MemberDisposition::LeftOut(reason) => reason.kind_key(),
            };
            out.push_str(&format!(
                "  {} ({} bytes, {} ms): disposition={disp_str} rejection={:?}\n",
                member.original_relative_name,
                member.byte_len,
                member.duration_ms,
                member.save_rejection,
            ));
        }
        success(out)
    }
}

fn success(stdout: String) -> CliRun {
    CliRun {
        stdout,
        stderr: String::new(),
        exit_code: 0,
    }
}

fn failure(stdout: &str, stderr: &str, exit_code: i32) -> CliRun {
    CliRun {
        stdout: stdout.to_owned(),
        stderr: stderr.to_owned(),
        exit_code,
    }
}
