// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The display identity of a terminal error recorded by a daily worker.

use crate::daily_unit::{DailyUnitRecord, DailyUnitStatus};
use serde::{Deserialize, Serialize};
use solstone_core_cli_copy::{ENGLISH, MessageDetail, Value, display_for, try_render_for};
use std::collections::BTreeMap;

#[derive(Debug, Serialize, Deserialize)]
struct Cause {
    id: String,
    args: BTreeMap<String, String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct StageCopy {
    version: u8,
    use_id: Option<String>,
    phase: String,
    stage: String,
    talent: String,
    detail_bytes: usize,
    cause: Option<Cause>,
}

pub fn stage_copy(
    use_id: Option<&str>,
    phase: &str,
    stage: &str,
    talent: &str,
    detail: &str,
    cause: Option<&MessageDetail>,
) -> serde_json::Value {
    let cause = cause
        .and_then(MessageDetail::catalog_fields)
        .map(|(id, values)| Cause {
            id: id.to_owned(),
            args: values
                .iter()
                .map(|(k, v)| (k.to_string(), v.clone()))
                .collect(),
        });
    serde_json::json!(StageCopy {
        version: 1,
        use_id: use_id.map(str::to_owned),
        phase: phase.into(),
        stage: stage.into(),
        talent: talent.into(),
        detail_bytes: detail.len(),
        cause
    })
}

impl StageCopy {
    fn render(&self, locale: &str, original: &str) -> Option<String> {
        if self.version != 1 {
            return None;
        }
        // An upstream diagnostic can contain any words. Its own bytes remain
        // the last argument of the canonical stage message.
        let tail = original.get(original.len().checked_sub(self.detail_bytes)?..)?;
        let detail = if let Some(cause) = &self.cause {
            if !(cause.id.starts_with("journal.cli.talent.daily.")
                || matches!(
                    cause.id.as_str(),
                    "journal.cli.talent.hook.day.missing"
                        | "journal.cli.talent.hook.facet.missing"
                        | "journal.cli.think.cancelled"
                ))
            {
                return None;
            }
            let args = cause
                .args
                .iter()
                .map(|(k, v)| (k.as_str(), Value::String(v)))
                .collect::<Vec<_>>();
            if try_render_for(ENGLISH, &cause.id, &args)? != tail {
                return None;
            }
            try_render_for(locale, &cause.id, &args)?
        } else {
            tail.to_owned()
        };
        let outer = |locale, detail: &str| {
            display_for(
                locale,
                "journal.cli.talent.hook.failure",
                [
                    ("phase", &self.phase),
                    ("stage", &self.stage),
                    ("talent", &self.talent),
                    ("detail", &detail),
                ],
            )
        };
        (outer(ENGLISH, tail) == original).then(|| outer(locale, &detail))
    }
}

/// Render only the terminal event actually recorded for this attempt. Logs
/// that outlive the current unit keep their original English.
pub fn recorded_stage_error(
    record: &DailyUnitRecord,
    use_id: &str,
    event: &serde_json::Value,
    locale: &str,
) -> Option<String> {
    if !matches!(
        record.status,
        DailyUnitStatus::Failed | DailyUnitStatus::Conflicting
    ) || record.use_id.as_deref() != Some(use_id)
        || event.get("event")?.as_str()? != "error"
        || !event.get("terminal")?.as_bool()?
        || event.get("reason_code")?.as_str()? != record.reason_code.as_deref()?
    {
        return None;
    }
    let original = event.get("error")?.as_str()?;
    if record.error_detail.as_deref()? != original {
        return None;
    }
    let receipt: StageCopy = serde_json::from_value(record.error_copy.clone()?).ok()?;
    if receipt.use_id.as_deref() != Some(use_id) || event.get("name")?.as_str()? != receipt.talent {
        return None;
    }
    receipt.render(locale, original)
}
