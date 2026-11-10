// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The order this journal's own Apple Health runs were admitted in.
//!
//! Each run takes the next number from `imports/.apple_health-runs.json` under
//! the source lock and leaves it in its import as `body-run.json`. An import is
//! ordered only if it carries this journal's mark: a journal merge copies import
//! folders but not that file, so a merged import is never ordered against this
//! journal's runs, and the numbers never follow the clock. Imports already here
//! when the file is first made (runs from before it existed) are adopted at 0.
//! Only a missing file counts as first made: one that can't be parsed starts a
//! new order that adopts nothing, and one that can't be read is left alone.

use std::collections::BTreeSet;
use std::path::Path;

use serde_json::{Value, json};

/// The journal's run counter, beside the import folders. A file, so a journal
/// merge, which copies import folders, leaves it behind.
pub const RUNS_FILE: &str = ".apple_health-runs.json";

/// An import's own mark: the journal it ran in and its place in that order.
pub const STAMP_FILE: &str = "body-run.json";

fn write(path: &Path, value: &Value) -> bool {
    solstone_core_journal_io::atomic_replace(
        path,
        value.to_string().as_bytes(),
        solstone_core_journal_io::AtomicWriteOptions { mode: Some(0o600) },
    )
    .is_ok()
}

fn read(path: &Path) -> Option<Value> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

/// What the counter file holds, kept apart because each case means something
/// different: only a missing file means the counter was never made.
enum Counter {
    Missing,
    Unreadable,
    Failed,
    Read(Value),
}

fn read_counter(path: &Path) -> Counter {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_or(Counter::Unreadable, Counter::Read),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Counter::Missing,
        Err(_) => Counter::Failed,
    }
}

/// The highest place an import in `epoch` already holds, so a counter restored
/// from an older copy never hands out a number a run already has.
fn highest_seq(imports: &Path, epoch: &str) -> Option<u64> {
    std::fs::read_dir(imports)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| read(&entry.path().join(STAMP_FILE)))
        .filter(|mark| mark.get("epoch").and_then(Value::as_str) == Some(epoch))
        .filter_map(|mark| mark.get("seq").and_then(Value::as_u64))
        .max()
}

fn new_epoch() -> String {
    let mut bytes = [0u8; 16];
    if getrandom::fill(&mut bytes).is_err() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        bytes = (nanos ^ u128::from(std::process::id())).to_le_bytes();
    }
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Mark `import_id` as this journal's next Apple Health run. Call it holding the
/// Apple Health source lock, once the run's attempt is admitted. Best effort:
/// an import that can't be marked is never ordered, which only ever leaves a
/// row reading what its own record says.
pub fn stamp(journal: &Path, import_id: &str) {
    let imports = journal.join("imports");
    let runs = imports.join(RUNS_FILE);
    let (epoch, next, adopted) = match read_counter(&runs) {
        Counter::Read(value) => match (
            value.get("epoch").and_then(Value::as_str),
            value.get("next").and_then(Value::as_u64),
        ) {
            (Some(epoch), Some(next)) => {
                let next = highest_seq(&imports, epoch)
                    .map_or(next, |seq| next.max(seq.saturating_add(1)));
                (
                    epoch.to_owned(),
                    next,
                    value.get("adopted").cloned().unwrap_or_else(|| json!([])),
                )
            }
            // A counter it can't read starts a new order: nothing marked
            // before it is ordered against what comes after.
            _ => (new_epoch(), 1, json!([])),
        },
        Counter::Unreadable => (new_epoch(), 1, json!([])),
        // A read that failed says nothing about the order: leave it as it is,
        // and this run unordered.
        Counter::Failed => return,
        Counter::Missing => {
            let adopted: Vec<String> = std::fs::read_dir(&imports)
                .into_iter()
                .flatten()
                .flatten()
                .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .filter(|id| !id.starts_with('.') && id != import_id)
                .collect();
            (new_epoch(), 1, json!(adopted))
        }
    };
    let counter = json!({"epoch": epoch, "next": next.saturating_add(1), "adopted": adopted});
    if write(&runs, &counter) {
        write(
            &imports.join(import_id).join(STAMP_FILE),
            &json!({"epoch": epoch, "seq": next}),
        );
    }
}

/// Where each import sits in this journal's order of Apple Health runs.
pub struct Order {
    epoch: Option<String>,
    adopted: BTreeSet<String>,
}

impl Order {
    /// Read the journal's counter once.
    #[must_use]
    pub fn read(journal: &Path) -> Self {
        let counter = read(&journal.join("imports").join(RUNS_FILE));
        let epoch = counter
            .as_ref()
            .and_then(|value| value.get("epoch")?.as_str().map(str::to_owned));
        let adopted = counter
            .as_ref()
            .and_then(|value| value.get("adopted")?.as_array().cloned())
            .unwrap_or_default()
            .into_iter()
            .filter_map(|id| id.as_str().map(str::to_owned))
            .collect();
        Self { epoch, adopted }
    }

    /// The import's place in this journal's order, or `None` when it didn't
    /// run here as far as this journal can tell.
    #[must_use]
    pub fn of(&self, journal: &Path, import_id: &str) -> Option<u64> {
        let epoch = self.epoch.as_deref()?;
        match read(&journal.join("imports").join(import_id).join(STAMP_FILE)) {
            Some(mark) => (mark.get("epoch").and_then(Value::as_str) == Some(epoch))
                .then(|| mark.get("seq").and_then(Value::as_u64))
                .flatten(),
            None => self.adopted.contains(import_id).then_some(0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn import(root: &Path, id: &str) {
        std::fs::create_dir_all(root.join("imports").join(id)).unwrap();
    }

    #[test]
    fn runs_are_ordered_by_admission_here_and_nothing_from_elsewhere_is() {
        let journal = tempfile::tempdir().unwrap();
        let root = journal.path();
        import(root, "20261001_120000");
        import(root, "20261011_120000");
        import(root, "20261009_120000");
        stamp(root, "20261011_120000");
        stamp(root, "20261009_120000");
        let order = Order::read(root);
        assert_eq!(
            order.of(root, "20261001_120000"),
            Some(0),
            "already here: adopted"
        );
        assert_eq!(order.of(root, "20261011_120000"), Some(1));
        assert_eq!(
            order.of(root, "20261009_120000"),
            Some(2),
            "admitted later, whatever its id or clock says"
        );
        // A merged import, with or without another journal's mark.
        import(root, "20261005_120000");
        let other = tempfile::tempdir().unwrap();
        import(other.path(), "20261006_120000");
        stamp(other.path(), "20261006_120000");
        import(root, "20261006_120000");
        std::fs::copy(
            other
                .path()
                .join("imports/20261006_120000")
                .join(STAMP_FILE),
            root.join("imports/20261006_120000").join(STAMP_FILE),
        )
        .unwrap();
        let order = Order::read(root);
        assert_eq!(
            order.of(root, "20261005_120000"),
            None,
            "arrived after the counter"
        );
        assert_eq!(
            order.of(root, "20261006_120000"),
            None,
            "another journal's mark"
        );
        // A counter that can't be read starts over, and orders nothing before it.
        std::fs::write(root.join("imports").join(RUNS_FILE), b"{").unwrap();
        import(root, "20261012_120000");
        stamp(root, "20261012_120000");
        let order = Order::read(root);
        assert_eq!(order.of(root, "20261012_120000"), Some(1));
        assert_eq!(order.of(root, "20261011_120000"), None);
    }

    #[test]
    fn a_counter_that_cant_be_parsed_adopts_nothing() {
        let journal = tempfile::tempdir().unwrap();
        let root = journal.path();
        import(root, "20261001_120000");
        stamp(root, "20261001_120000");
        // Merged in later: no mark of this journal's.
        import(root, "20261003_120000");
        std::fs::write(root.join("imports").join(RUNS_FILE), b"{").unwrap();
        import(root, "20261004_120000");
        stamp(root, "20261004_120000");
        let order = Order::read(root);
        assert_eq!(order.of(root, "20261004_120000"), Some(1));
        assert_eq!(
            order.of(root, "20261003_120000"),
            None,
            "a merged import is not adopted when the counter is only unparsable"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_counter_that_cant_be_read_is_left_alone() {
        use std::os::unix::fs::PermissionsExt;
        let journal = tempfile::tempdir().unwrap();
        let root = journal.path();
        let runs = root.join("imports").join(RUNS_FILE);
        import(root, "20261001_120000");
        stamp(root, "20261001_120000");
        let before = std::fs::read(&runs).unwrap();
        // Merged in later, so a counter made afresh would adopt it.
        import(root, "20261002_120000");
        // A read that fails with something other than "not found", while the
        // folder can still be written.
        std::fs::set_permissions(&runs, std::fs::Permissions::from_mode(0o000)).unwrap();
        if std::fs::read(&runs).is_ok() {
            return; // running as root: permissions don't stop the read
        }
        import(root, "20261003_120000");
        stamp(root, "20261003_120000");
        std::fs::set_permissions(&runs, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(std::fs::read(&runs).unwrap(), before, "counter untouched");
        assert!(
            !root
                .join("imports/20261003_120000")
                .join(STAMP_FILE)
                .exists(),
            "the run stays unordered"
        );
    }

    #[test]
    fn an_older_counter_never_hands_out_a_number_already_held() {
        let journal = tempfile::tempdir().unwrap();
        let root = journal.path();
        let runs = root.join("imports").join(RUNS_FILE);
        import(root, "20261001_120000");
        stamp(root, "20261001_120000");
        let older = std::fs::read(&runs).unwrap();
        for id in ["20261002_120000", "20261003_120000"] {
            import(root, id);
            stamp(root, id);
        }
        // An older copy of the counter, restored beside newer stamps.
        std::fs::write(&runs, older).unwrap();
        import(root, "20261004_120000");
        stamp(root, "20261004_120000");
        let order = Order::read(root);
        assert_eq!(order.of(root, "20261003_120000"), Some(3));
        assert_eq!(order.of(root, "20261004_120000"), Some(4));
    }
}
