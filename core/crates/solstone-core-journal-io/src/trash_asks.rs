// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Which days the journal is making again because recordings on them went to
//! the trash or came back, read straight from the owner-ask store.
//!
//! The store's owner is `solstone-core-system::owner_ask`; this is the one
//! read a crate below it needs (the index writer, which leaves those days'
//! briefing and week page out of search meanwhile). It reads only what an ask
//! the trash filed says about itself.

use std::collections::BTreeSet;
use std::path::Path;

use chrono::Datelike;
use serde_json::Value;

use crate::durability::{ArtifactId, artifact};

/// The cause prefix an ask the trash files carries.
pub const TRASH_CAUSE: &str = "trash:";

/// What the trash's pending asks keep out of search.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TrashPending {
    /// Days (`YYYYMMDD`) whose briefing is being made again.
    pub briefings: BTreeSet<String>,
    /// Week starts (the Sunday, `YYYYMMDD`) whose page is being made again.
    pub weeks: BTreeSet<String>,
}

/// The Sunday starting the week that holds `day`.
pub fn week_start(day: &str) -> Option<String> {
    let date = chrono::NaiveDate::parse_from_str(day, "%Y%m%d").ok()?;
    let sunday = date - chrono::Duration::days(i64::from(date.weekday().num_days_from_sunday()));
    Some(sunday.format("%Y%m%d").to_string())
}

/// Read the trash's pending asks. No store is none pending; a store that can't
/// be read or understood is an error, so a caller keeping things out of search
/// keeps them out and tries again.
pub fn trash_pending(journal: &Path) -> Result<TrashPending, String> {
    let path = journal.join(artifact(ArtifactId::OwnerAsks).path);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(TrashPending::default());
        }
        Err(error) => return Err(error.to_string()),
    };
    let store: Value = serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
    let asks = store
        .get("asks")
        .and_then(Value::as_object)
        .ok_or("owner asks hold no asks")?;
    let mut pending = TrashPending::default();
    for ask in asks.values() {
        let from_trash = ask
            .get("cause")
            .and_then(Value::as_str)
            .is_some_and(|cause| cause.starts_with(TRASH_CAUSE));
        if !from_trash {
            continue;
        }
        let day = ask
            .get("day")
            .and_then(Value::as_str)
            .ok_or("an owner ask names no day")?;
        let flag = |name: &str| ask.get(name).and_then(Value::as_bool).unwrap_or(false);
        if flag("thaw") && !flag("thaw_done") {
            pending.briefings.insert(day.to_owned());
        }
        let week_ended = ask
            .get("week")
            .and_then(|week| week.get("ended"))
            .is_some_and(|ended| !ended.is_null());
        if !week_ended && let Some(sunday) = week_start(day) {
            pending.weeks.insert(sunday);
        }
    }
    Ok(pending)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_trashs_pending_steps_are_read() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(trash_pending(root.path()).unwrap(), TrashPending::default());
        let path = root.path().join(artifact(ArtifactId::OwnerAsks).path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            serde_json::json!({"version": 1, "asks": {
                "a": {"day": "20261111", "cause": "trash:op", "thaw": true, "thaw_done": false, "week": {}},
                "b": {"day": "20261020", "cause": "trash:op", "thaw": true, "thaw_done": true, "week": {"ended": "built"}},
                "c": {"day": "20261021", "thaw": true, "thaw_done": false, "week": {}},
                "d": {"day": "20261003", "cause": "trash:x", "thaw": true, "thaw_done": true, "week": {}}
            }})
            .to_string(),
        )
        .unwrap();
        let pending = trash_pending(root.path()).unwrap();
        assert_eq!(pending.briefings, BTreeSet::from(["20261111".to_owned()]));
        assert_eq!(
            pending.weeks,
            BTreeSet::from(["20261108".to_owned(), "20260927".to_owned()])
        );
        std::fs::write(&path, b"{").unwrap();
        assert!(trash_pending(root.path()).is_err());
    }
}
