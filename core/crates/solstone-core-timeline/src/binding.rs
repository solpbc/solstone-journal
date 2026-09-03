// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::{Path, PathBuf};

use solstone_core_journal_io::{DEFAULT_STREAM, PathOrDay, contained_path, iter_segments};

use crate::{
    SegmentBindingV1, SegmentSelectorV1, TimelineError, artifact_sha256, validate_segment_binding,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivitySourceSnapshot {
    pub relative_path: String,
    pub text: String,
    pub sha256: String,
}

pub fn resolve_segment_binding(
    journal: &Path,
    selector: &SegmentSelectorV1,
) -> Result<SegmentBindingV1, TimelineError> {
    if selector.day.is_empty() || selector.segment.is_empty() {
        return Err(TimelineError::MalformedBinding {
            day: selector.day.clone(),
            stream: selector.stream.clone().unwrap_or_default(),
            segment: selector.segment.clone(),
        });
    }
    if selector.stream.as_deref() == Some("") {
        return Err(TimelineError::MalformedBinding {
            day: selector.day.clone(),
            stream: String::new(),
            segment: selector.segment.clone(),
        });
    }

    let candidates = discover_day_segment_bindings(journal, &selector.day)?
        .into_iter()
        .filter(|binding| binding.segment == selector.segment)
        .collect::<Vec<_>>();
    if let Some(stream) = selector.stream.as_deref() {
        return candidates
            .into_iter()
            .find(|binding| binding.stream == stream)
            .ok_or_else(|| {
                TimelineError::segment_not_found(&selector.day, &selector.segment, Some(stream))
            });
    }
    match candidates.as_slice() {
        [] => Err(TimelineError::segment_not_found(
            &selector.day,
            &selector.segment,
            None,
        )),
        [binding] => Ok(binding.clone()),
        _ => Err(TimelineError::AmbiguousSegment {
            day: selector.day.clone(),
            segment: selector.segment.clone(),
            streams: candidates
                .into_iter()
                .map(|binding| binding.stream)
                .collect(),
        }),
    }
}

pub fn discover_day_segment_bindings(
    journal: &Path,
    day: &str,
) -> Result<Vec<SegmentBindingV1>, TimelineError> {
    let mut bindings = iter_segments(journal, PathOrDay::Day(day))?
        .into_iter()
        .map(|segment| {
            let identity = segment.record_identity().map_err(|error| {
                TimelineError::InvalidSegmentIdentity {
                    detail: error.to_string(),
                }
            })?;
            Ok(SegmentBindingV1 {
                day: day.to_owned(),
                stream: identity.stream.to_owned(),
                segment: identity.name.to_owned(),
            })
        })
        .collect::<Result<Vec<_>, TimelineError>>()?;
    bindings.sort_by(|left, right| {
        left.segment
            .cmp(&right.segment)
            .then_with(|| left.stream.cmp(&right.stream))
    });
    Ok(bindings)
}

pub fn segment_directory(
    journal: &Path,
    binding: &SegmentBindingV1,
) -> Result<PathBuf, TimelineError> {
    validate_segment_binding(binding)?;
    let day = journal.join("chronicle").join(&binding.day);
    if binding.stream == DEFAULT_STREAM {
        Ok(day.join(&binding.segment))
    } else {
        Ok(day.join(&binding.stream).join(&binding.segment))
    }
}

pub fn origin_for_binding(binding: &SegmentBindingV1) -> Result<String, TimelineError> {
    validate_segment_binding(binding)?;
    if binding.stream == DEFAULT_STREAM {
        Ok(format!("{}/{}", binding.day, binding.segment))
    } else {
        Ok(format!(
            "{}/{}/{}",
            binding.day, binding.stream, binding.segment
        ))
    }
}

pub fn activity_source_relative_paths(
    binding: &SegmentBindingV1,
) -> Result<[String; 2], TimelineError> {
    let origin = origin_for_binding(binding)?;
    Ok([
        format!("chronicle/{origin}/talents/activity.md"),
        format!("chronicle/{origin}/activity.md"),
    ])
}

pub fn resolve_activity_source(
    journal: &Path,
    binding: &SegmentBindingV1,
) -> Result<Option<ActivitySourceSnapshot>, TimelineError> {
    for relative_path in activity_source_relative_paths(binding)? {
        let path = contained_path(journal, &relative_path)?;
        if !path.is_file() {
            continue;
        }
        let bytes = std::fs::read(&path).map_err(|error| TimelineError::InvalidSourceEvidence {
            detail: format!("cannot read {relative_path}: {error}"),
        })?;
        let text =
            String::from_utf8(bytes).map_err(|error| TimelineError::InvalidSourceEvidence {
                detail: format!("source {relative_path} is not UTF-8: {error}"),
            })?;
        return Ok(Some(ActivitySourceSnapshot {
            sha256: artifact_sha256(&text),
            relative_path,
            text,
        }));
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn segment(root: &Path, stream: Option<&str>, name: &str) {
        let day = root.join("chronicle/20260401");
        let path = match stream {
            Some(stream) => day.join(stream).join(name),
            None => day.join(name),
        };
        fs::create_dir_all(path).unwrap();
    }

    fn selector(stream: Option<&str>) -> SegmentSelectorV1 {
        SegmentSelectorV1 {
            day: "20260401".to_owned(),
            segment: "080000_300".to_owned(),
            stream: stream.map(ToOwned::to_owned),
        }
    }

    #[test]
    fn explicit_stream_never_falls_back_to_direct_layout() {
        let journal = tempfile::tempdir().unwrap();
        segment(journal.path(), None, "080000_300");

        assert!(matches!(
            resolve_segment_binding(journal.path(), &selector(Some("audio"))),
            Err(TimelineError::SegmentNotFound { .. })
        ));
    }

    #[test]
    fn no_stream_rejects_duplicate_basenames() {
        let journal = tempfile::tempdir().unwrap();
        segment(journal.path(), None, "080000_300");
        segment(journal.path(), Some("audio"), "080000_300");

        assert!(matches!(
            resolve_segment_binding(journal.path(), &selector(None)),
            Err(TimelineError::AmbiguousSegment { streams, .. })
                if streams == vec!["_default", "audio"]
        ));
    }

    #[test]
    fn no_stream_binds_the_only_matching_segment() {
        let journal = tempfile::tempdir().unwrap();
        segment(journal.path(), Some("audio"), "080000_300");
        segment(journal.path(), Some("audio"), "090000_300");

        assert_eq!(
            resolve_segment_binding(journal.path(), &selector(None)).unwrap(),
            SegmentBindingV1 {
                day: "20260401".to_owned(),
                stream: "audio".to_owned(),
                segment: "080000_300".to_owned(),
            }
        );
    }

    #[test]
    fn missing_segment_is_named() {
        let journal = tempfile::tempdir().unwrap();

        assert!(matches!(
            resolve_segment_binding(journal.path(), &selector(None)),
            Err(TimelineError::SegmentNotFound { .. })
        ));
    }

    #[test]
    fn activity_resolution_binds_the_preferred_exact_path_and_bytes() {
        let journal = tempfile::tempdir().unwrap();
        segment(journal.path(), None, "080000_300");
        let segment = journal.path().join("chronicle/20260401/080000_300");
        fs::create_dir_all(segment.join("talents")).unwrap();
        fs::write(segment.join("activity.md"), "legacy").unwrap();
        fs::write(segment.join("talents/activity.md"), "canonical").unwrap();

        let snapshot = resolve_activity_source(
            journal.path(),
            &resolve_segment_binding(journal.path(), &selector(None)).unwrap(),
        )
        .unwrap()
        .unwrap();

        assert_eq!(
            snapshot.relative_path,
            "chronicle/20260401/080000_300/talents/activity.md"
        );
        assert_eq!(snapshot.text, "canonical");
        assert_eq!(snapshot.sha256, artifact_sha256("canonical"));
    }

    #[test]
    fn invalid_utf8_activity_is_a_named_source_error() {
        let journal = tempfile::tempdir().unwrap();
        segment(journal.path(), None, "080000_300");
        let segment = journal.path().join("chronicle/20260401/080000_300");
        fs::create_dir_all(segment.join("talents")).unwrap();
        fs::write(segment.join("talents/activity.md"), [0xff]).unwrap();

        assert!(matches!(
            resolve_activity_source(
                journal.path(),
                &resolve_segment_binding(journal.path(), &selector(None)).unwrap(),
            ),
            Err(TimelineError::InvalidSourceEvidence { detail }) if detail.contains("not UTF-8")
        ));
    }
}
