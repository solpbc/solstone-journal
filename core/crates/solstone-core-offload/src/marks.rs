// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use solstone_core_retention::{Register, RemovalClass};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OffloadMarkIndex {
    pub entries: Vec<OffloadMark>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OffloadMark {
    pub id: String,
    pub day: String,
    pub stream: String,
    pub dir: String,
    pub names: Vec<String>,
    pub bytes: u64,
}
impl OffloadMarkIndex {
    pub fn from_register(register: &Register) -> Self {
        Self {
            entries: register
                .marks
                .values()
                .filter(|mark| {
                    mark.class == RemovalClass::OffloadRawRelease
                        && matches!(mark.state, solstone_core_retention::MarkState::Marked)
                })
                .map(|mark| OffloadMark {
                    id: mark.id.as_str().into(),
                    day: mark.target.day.clone(),
                    stream: mark.target.stream.clone(),
                    dir: mark.target.dir.clone(),
                    names: mark.proposal.names.clone(),
                    bytes: mark.proposal.bytes,
                })
                .collect(),
        }
    }
    pub fn matches(
        &self,
        day: &str,
        stream: &str,
        dir: &str,
        names: &[String],
    ) -> Option<&OffloadMark> {
        self.entries.iter().find(|entry| {
            entry.day == day && entry.stream == stream && entry.dir == dir && entry.names == names
        })
    }
}
