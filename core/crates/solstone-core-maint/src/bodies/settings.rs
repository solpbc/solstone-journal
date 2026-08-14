// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
use crate::registry::{MaintBodyContext, MaintBodyResult};
fn result(value: Result<bool, String>) -> MaintBodyResult {
    match value {
        Ok(changed) => MaintBodyResult {
            stdout: vec![if changed {
                "Migration completed.".into()
            } else {
                "No changes needed.".into()
            }],
            exit_code: 0,
        },
        Err(error) => MaintBodyResult {
            stdout: vec![error],
            exit_code: 1,
        },
    }
}
pub fn seed_default_app_navigation(c: &MaintBodyContext<'_>) -> MaintBodyResult {
    result(
        solstone_core_convey_config::seed_default_app_navigation(c.journal)
            .map(|r| r.changed)
            .map_err(|e| e.to_string()),
    )
}
pub fn pin_curation_navigation(c: &MaintBodyContext<'_>) -> MaintBodyResult {
    result(
        solstone_core_convey_config::pin_curation_navigation(c.journal)
            .map(|r| r.changed)
            .map_err(|e| e.to_string()),
    )
}
pub fn drop_services_navigation(c: &MaintBodyContext<'_>) -> MaintBodyResult {
    result(
        solstone_core_convey_config::drop_services_navigation(c.journal)
            .map(|r| r.changed)
            .map_err(|e| e.to_string()),
    )
}
pub fn backfill_import_manifests(c: &MaintBodyContext<'_>) -> MaintBodyResult {
    match solstone_core_import::backfill_retained_import_manifests(c.journal) {
        Ok(r) => MaintBodyResult {
            stdout: vec![
                format!("Scanned {} import(s)", r.scanned),
                format!("Backfilled {} manifest(s)", r.backfilled),
            ],
            exit_code: 0,
        },
        Err(e) => MaintBodyResult {
            stdout: vec![e.to_string()],
            exit_code: 1,
        },
    }
}
pub fn backfill_streams(c: &MaintBodyContext<'_>) -> MaintBodyResult {
    match solstone_core_segment::backfill_stream_records(c.journal, None, c.verbose) {
        Ok(r) => {
            let mut stdout = vec![
                format!("Classified {} segment(s)", r.classified),
                format!("Wrote {} stream marker(s)", r.written),
                format!("Repaired {} marker linkage(s)", r.linkage_fixed),
                format!("Rebuilt {} stream record(s)", r.rebuilt_streams),
            ];
            if r.nothing_to_do() {
                stdout.push("No changes needed.".into());
            }
            MaintBodyResult {
                stdout,
                exit_code: 0,
            }
        }
        Err(e) => MaintBodyResult {
            stdout: vec![e.to_string()],
            exit_code: 1,
        },
    }
}
pub fn restructure_stream_dirs(c: &MaintBodyContext<'_>) -> MaintBodyResult {
    match solstone_core_segment::restructure_segments_by_stream(c.journal, c.dry_run) {
        // A partially tagged journal refuses before any move, exactly as the
        // retired migration's `SystemExit(1)` did.
        Ok(r) if r.missing_markers > 0 => MaintBodyResult {
            stdout: vec![
                format!(
                    "ERROR: {} segments are missing stream.json markers.",
                    r.missing_markers
                ),
                "Run 'journal maint settings:001_backfill_streams' first to tag all segments."
                    .into(),
            ],
            exit_code: 1,
        },
        Ok(r) if r.already_restructured => MaintBodyResult {
            stdout: vec!["Journal already uses stream directory layout. Nothing to do.".into()],
            exit_code: 0,
        },
        Ok(r) => MaintBodyResult {
            stdout: vec![
                format!("Moved {} segment(s) into {} stream(s)", r.moved, r.streams),
                format!("Removed {} empty segment directories", r.removed),
            ],
            exit_code: 0,
        },
        Err(e) => MaintBodyResult {
            stdout: vec![e.to_string()],
            exit_code: 1,
        },
    }
}
pub fn migrate_pdf_extractions(c: &MaintBodyContext<'_>) -> MaintBodyResult {
    match solstone_core_segment::migrate_pdf_extractions(c.journal) {
        Ok(r) => MaintBodyResult {
            stdout: r.report_lines(),
            exit_code: 0,
        },
        Err(e) => MaintBodyResult {
            stdout: vec![e.to_string()],
            exit_code: 1,
        },
    }
}
pub fn migrate_pairing_home_address(c: &MaintBodyContext<'_>) -> MaintBodyResult {
    result(
        solstone_core_journal_config_write::migrate_pairing_home_address(c.journal)
            .map(|r| r.changed)
            .map_err(|e| e.to_string()),
    )
}
