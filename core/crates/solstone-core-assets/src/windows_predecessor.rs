// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde::{Deserialize, Serialize};
use std::sync::LazyLock;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PredecessorAsset {
    pub origin_key: String,
    pub sha256: String,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PredecessorGroup {
    pub unit: String,
    pub assets: Vec<PredecessorAsset>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PredecessorRelease {
    pub windows_release: String,
    pub source_commit: String,
    pub groups: Vec<PredecessorGroup>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WindowsPredecessor {
    pub schema_version: u32,
    pub candidate_release: String,
    pub predecessor: PredecessorRelease,
}

fn version(value: &str) -> Option<Vec<u64>> {
    let parts = value
        .split('.')
        .map(str::parse)
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    (parts.len() == 3).then_some(parts)
}

impl WindowsPredecessor {
    /// Validate the explicit prior supported Windows release, independent of
    /// releases published only for other platforms. Unknown input forbids GC.
    pub fn validate(&self, candidate: &str) -> Result<(), String> {
        let invalid =
            || "invalid Windows predecessor input; automatic cache cleanup is disabled".to_owned();
        if self.schema_version != 1 || self.candidate_release != candidate {
            return Err(
                "Windows predecessor input must be updated for this candidate release".into(),
            );
        }
        if version(candidate)
            .zip(version(&self.predecessor.windows_release))
            .is_none_or(|(c, p)| p >= c)
            || self.predecessor.source_commit.len() != 40
            || !self
                .predecessor
                .source_commit
                .bytes()
                .all(|b| b.is_ascii_hexdigit())
        {
            return Err(invalid());
        }
        // These are the complete optional Windows units. A model and its
        // projector are retained together; sharing just one digest is not a match.
        for (unit, names) in [
            (
                "local-model",
                &["Qwen3.5-4B-Q4_K_M.gguf", "mmproj-F16.gguf"][..],
            ),
            (
                "location-accuracy",
                &["containment.slpk", "venues.slpk"][..],
            ),
        ] {
            let groups: Vec<_> = self
                .predecessor
                .groups
                .iter()
                .filter(|g| g.unit == unit)
                .collect();
            if groups.len() != 1 || groups[0].assets.len() != names.len() {
                return Err(invalid());
            }
            for name in names {
                let entries: Vec<_> = groups[0]
                    .assets
                    .iter()
                    .filter(|a| {
                        a.origin_key.starts_with(&format!("assets/{unit}/"))
                            && a.origin_key.ends_with(&format!("/{name}"))
                    })
                    .collect();
                if entries.len() != 1
                    || entries[0].size_bytes == 0
                    || entries[0].sha256.len() != 64
                    || !entries[0].sha256.bytes().all(|b| b.is_ascii_hexdigit())
                {
                    return Err(invalid());
                }
            }
        }
        if self.predecessor.groups.len() != 2 {
            return Err(invalid());
        }
        Ok(())
    }

    pub fn protects(&self, unit: &str, files: &[(&str, u64)]) -> bool {
        let mut actual: Vec<_> = files
            .iter()
            .map(|(sha, size)| (sha.to_ascii_lowercase(), *size))
            .collect();
        actual.sort();
        self.predecessor.groups.iter().any(|group| {
            let mut expected: Vec<_> = group
                .assets
                .iter()
                .map(|a| (a.sha256.to_ascii_lowercase(), a.size_bytes))
                .collect();
            expected.sort();
            group.unit == unit && expected == actual
        })
    }
}

pub fn windows_predecessor() -> Result<&'static WindowsPredecessor, &'static str> {
    static RECORD: LazyLock<Result<WindowsPredecessor, String>> = LazyLock::new(|| {
        let record: WindowsPredecessor =
            serde_json::from_str(include_str!("../windows-predecessor.json"))
                .map_err(|e| e.to_string())?;
        record.validate(env!("CARGO_PKG_VERSION"))?;
        Ok(record)
    });
    RECORD.as_ref().map_err(String::as_str)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn explicit_predecessor_is_complete_and_bound_to_the_candidate() {
        let record = windows_predecessor().unwrap();
        assert!(record.validate(&record.candidate_release).is_ok());
        let mut stale = record.clone();
        stale.candidate_release = "0.0.0".into();
        assert!(stale.validate(&record.candidate_release).is_err());
        let mut incomplete = record.clone();
        incomplete.predecessor.groups[0].assets.pop();
        assert!(incomplete.validate(&record.candidate_release).is_err());
        let group = &record.predecessor.groups[0];
        let mut members: Vec<_> = group
            .assets
            .iter()
            .map(|a| (a.sha256.as_str(), a.size_bytes))
            .collect();
        assert!(record.protects(&group.unit, &members));
        members[1].0 = "changed-projector";
        assert!(!record.protects(&group.unit, &members));
        let mut skipped = record.clone();
        skipped.candidate_release = "2.0.53".into();
        skipped.predecessor.windows_release = "2.0.51".into();
        assert!(skipped.validate("2.0.53").is_ok());
    }
}

#[cfg(feature = "runtime-fetch-test")]
thread_local! {
    static RETENTION_FIXTURE: std::cell::Cell<Option<Result<&'static WindowsPredecessor,&'static str>>> = const { std::cell::Cell::new(None) };
}

/// Runtime retention uses immutable packaging input on Windows. Other hosts
/// need no Windows predecessor; portable tests can exercise the same policy.
pub fn retention_predecessor() -> Option<Result<&'static WindowsPredecessor, &'static str>> {
    #[cfg(feature = "runtime-fetch-test")]
    if let Some(record) = RETENTION_FIXTURE.with(std::cell::Cell::get) {
        return Some(record);
    }
    #[cfg(windows)]
    {
        Some(windows_predecessor())
    }
    #[cfg(not(windows))]
    {
        None
    }
}

#[cfg(feature = "runtime-fetch-test")]
#[doc(hidden)]
pub fn with_windows_predecessor_fixture<T>(
    record: WindowsPredecessor,
    work: impl FnOnce() -> T,
) -> T {
    struct Restore(Option<Result<&'static WindowsPredecessor, &'static str>>);
    impl Drop for Restore {
        fn drop(&mut self) {
            RETENTION_FIXTURE.with(|slot| slot.set(self.0));
        }
    }
    let fixture = match record.validate(env!("CARGO_PKG_VERSION")) {
        Ok(()) => Ok(&*Box::leak(Box::new(record))),
        Err(error) => Err(&*Box::leak(error.into_boxed_str())),
    };
    let _restore = Restore(RETENTION_FIXTURE.with(|slot| slot.replace(Some(fixture))));
    work()
}
