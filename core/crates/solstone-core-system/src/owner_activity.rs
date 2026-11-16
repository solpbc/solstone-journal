// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;

use serde_json::{Map, Value};
use solstone_core_facets::ActivityRecordStoreError;
use solstone_core_facets::stretch::{StretchCorrection, correct_over_stretch};

use crate::owner_ask::{AskedActivity, ask_corrected};

const RETHOUGHT_FIELDS: [&str; 3] = ["details", "playback", "description"];

#[derive(Debug)]
pub struct AppliedOwnerActivityCorrection {
    pub correction: StretchCorrection,
    pub asked: bool,
    pub ask_reason: Option<String>,
    pub action_log_failed: bool,
}

#[allow(clippy::too_many_arguments)]
pub fn apply_owner_activity_correction(
    root: &Path,
    facet: &str,
    day: &str,
    record_id: &str,
    patch: &Map<String, Value>,
    note: &str,
    timestamp: &str,
    log_source: &str,
    log_actor: &str,
    checkpoint: impl FnOnce(&solstone_core_facets::ActivityRecord) -> Result<(), String>,
    extra: &Map<String, Value>,
    require_complete_stretch: bool,
    ask_hook: Option<&crate::owner_ask::CorrectionAsk<'_>>,
) -> Result<AppliedOwnerActivityCorrection, ActivityRecordStoreError> {
    let correction = correct_over_stretch(
        root, facet, day, record_id, patch, note, timestamp, checkpoint, extra,
    )?;

    let mut asked = false;
    let mut ask_reason = None;
    let mut action_log_failed = false;

    if let Some(record) = &correction.record {
        let should_ask_and_log = !require_complete_stretch || correction.failed.is_empty();
        if should_ask_and_log {
            if !solstone_core_facets::is_body_record(record) {
                let rethought = patch
                    .keys()
                    .any(|key| RETHOUGHT_FIELDS.contains(&key.as_str()));
                let activities: Vec<AskedActivity> = rethought
                    .then(|| {
                        std::iter::once(AskedActivity {
                            facet: facet.to_owned(),
                            activity: record_id.to_owned(),
                        })
                        .chain(correction.copies.iter().map(|copy| {
                            AskedActivity {
                                facet: copy.facet.clone(),
                                activity: copy.id.clone(),
                            }
                        }))
                    })
                    .into_iter()
                    .flatten()
                    .collect();

                let ask_result = match ask_hook {
                    Some(hook) => hook(root, day, &activities),
                    None => ask_corrected(root, day, &activities),
                };

                match ask_result {
                    Ok(()) => {
                        asked = true;
                    }
                    Err(err) => {
                        asked = false;
                        ask_reason = Some(err.reason_code().to_owned());
                    }
                }
            }

            if let Err(error) = solstone_core_facets::append_action_log_for_day(
                root,
                Some(facet),
                log_source,
                log_actor,
                "activity_update",
                serde_json::json!({
                    "id": record_id,
                    "fields": patch.keys().collect::<Vec<_>>(),
                }),
                day,
            ) {
                log::warn!("owner activity: append_action_log_for_day failed: {error}");
                action_log_failed = true;
            }
        }
    }

    Ok(AppliedOwnerActivityCorrection {
        correction,
        asked,
        ask_reason,
        action_log_failed,
    })
}
