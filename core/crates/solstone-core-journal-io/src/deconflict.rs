// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Read-only segment-key deconfliction.

use std::collections::HashSet;
use std::error::Error;
use std::fmt;
use std::path::Path;

use getrandom::fill as fill_random;

use crate::{PathError, path_lexists};

/// Failure while choosing an unused segment key.
#[derive(Debug)]
pub enum SegmentDeconflictError {
    /// The source key is not a `HHMMSS_LEN` key.
    InvalidCandidate(String),
    /// Inspecting an existing destination failed.
    Path(PathError),
    /// The operating system could not provide entropy for the random walk.
    Entropy(getrandom::Error),
}

impl fmt::Display for SegmentDeconflictError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidCandidate(candidate) => {
                write!(
                    formatter,
                    "invalid segment key {candidate:?}; expected HHMMSS_LEN"
                )
            }
            Self::Path(error) => error.fmt(formatter),
            Self::Entropy(error) => write!(formatter, "could not randomize segment key: {error}"),
        }
    }
}

impl Error for SegmentDeconflictError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Path(error) => Some(error),
            Self::Entropy(_) => None,
            Self::InvalidCandidate(_) => None,
        }
    }
}

/// Return an unused key under `parent`, without creating or claiming it.
pub fn find_available_segment(
    parent: &Path,
    candidate: &str,
    max_attempts: usize,
) -> Result<Option<String>, SegmentDeconflictError> {
    find_available_segment_with_occupied(parent, candidate, max_attempts, &HashSet::new())
}

/// Return an unused key while also avoiding keys reserved by this caller.
///
/// `occupied` is deliberately in-memory only: this helper never writes a
/// placeholder to the journal merely to reserve a candidate during planning.
pub fn find_available_segment_with_occupied(
    parent: &Path,
    candidate: &str,
    max_attempts: usize,
    occupied: &HashSet<String>,
) -> Result<Option<String>, SegmentDeconflictError> {
    let mut current = SegmentKey::parse(candidate)?;
    let mut tried = HashSet::from([candidate.to_owned()]);
    if !is_taken(parent, candidate, occupied)? {
        return Ok(Some(candidate.to_owned()));
    }

    for _ in 0..max_attempts {
        current = current.random_step()?;
        let key = current.to_string();
        if !tried.insert(key.clone()) || is_taken(parent, &key, occupied)? {
            continue;
        }
        return Ok(Some(key));
    }
    Ok(None)
}

fn is_taken(
    parent: &Path,
    key: &str,
    occupied: &HashSet<String>,
) -> Result<bool, SegmentDeconflictError> {
    if occupied.contains(key) {
        return Ok(true);
    }
    path_lexists(&parent.join(key)).map_err(SegmentDeconflictError::Path)
}

#[derive(Clone, Copy)]
struct SegmentKey {
    start_seconds: i32,
    length: i32,
}

impl SegmentKey {
    fn parse(candidate: &str) -> Result<Self, SegmentDeconflictError> {
        let Some((time, length)) = candidate.split_once('_') else {
            return Err(SegmentDeconflictError::InvalidCandidate(
                candidate.to_owned(),
            ));
        };
        if time.len() != 6
            || !time.bytes().all(|byte| byte.is_ascii_digit())
            || length.is_empty()
            || !length.bytes().all(|byte| byte.is_ascii_digit())
        {
            return Err(SegmentDeconflictError::InvalidCandidate(
                candidate.to_owned(),
            ));
        }
        let hour: i32 = time[0..2]
            .parse()
            .map_err(|_| SegmentDeconflictError::InvalidCandidate(candidate.to_owned()))?;
        let minute: i32 = time[2..4]
            .parse()
            .map_err(|_| SegmentDeconflictError::InvalidCandidate(candidate.to_owned()))?;
        let second: i32 = time[4..6]
            .parse()
            .map_err(|_| SegmentDeconflictError::InvalidCandidate(candidate.to_owned()))?;
        let length: i32 = length
            .parse()
            .map_err(|_| SegmentDeconflictError::InvalidCandidate(candidate.to_owned()))?;
        if hour > 23 || minute > 59 || second > 59 || length <= 0 {
            return Err(SegmentDeconflictError::InvalidCandidate(
                candidate.to_owned(),
            ));
        }
        Ok(Self {
            start_seconds: hour * 3600 + minute * 60 + second,
            length,
        })
    }

    fn random_step(self) -> Result<Self, SegmentDeconflictError> {
        let mut entropy = [0_u8; 2];
        fill_random(&mut entropy).map_err(SegmentDeconflictError::Entropy)?;
        let delta = if entropy[1] & 1 == 0 { -1 } else { 1 };
        if entropy[0] & 1 == 0 {
            let start_seconds = self.start_seconds + delta;
            if !(0..86_400).contains(&start_seconds) {
                return Ok(self);
            }
            Ok(Self {
                start_seconds,
                ..self
            })
        } else {
            let length = self.length + delta;
            if length <= 0 {
                return Ok(self);
            }
            Ok(Self { length, ..self })
        }
    }
}

impl fmt::Display for SegmentKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let hour = self.start_seconds / 3600;
        let minute = (self.start_seconds % 3600) / 60;
        let second = self.start_seconds % 60;
        write!(formatter, "{hour:02}{minute:02}{second:02}_{}", self.length)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::fs;

    use super::{find_available_segment, find_available_segment_with_occupied};

    #[test]
    fn free_candidate_is_returned_without_creating_it() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let key = find_available_segment(temporary.path(), "120000_30", 1)
            .expect("availability check")
            .expect("free candidate");
        assert_eq!(key, "120000_30");
        assert!(!temporary.path().join(key).exists());
    }

    #[test]
    fn returned_conflict_key_is_valid_and_not_occupied() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        fs::create_dir(temporary.path().join("120000_30")).expect("occupied key");
        let occupied = HashSet::from(["120000_30".to_owned()]);
        let key =
            find_available_segment_with_occupied(temporary.path(), "120000_30", 100, &occupied)
                .expect("availability check")
                .expect("available key");
        let (time, length) = key.split_once('_').expect("key shape");
        assert_eq!(time.len(), 6);
        assert!(time.bytes().all(|byte| byte.is_ascii_digit()));
        assert!(length.parse::<u32>().expect("length") > 0);
        assert!(!occupied.contains(&key));
        assert!(!temporary.path().join(key).exists());
    }

    #[test]
    fn zero_attempts_exhausts_without_creating_a_candidate() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        fs::create_dir(temporary.path().join("120000_30")).expect("occupied key");
        assert_eq!(
            find_available_segment(temporary.path(), "120000_30", 0).expect("availability"),
            None
        );
        assert_eq!(
            fs::read_dir(temporary.path()).expect("directory").count(),
            1
        );
    }
}
