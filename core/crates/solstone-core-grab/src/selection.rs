// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::{Path, PathBuf};

use crate::error::GrabFailure;

pub(crate) const SUPPORTED_OUTPUT_SUFFIXES: [&str; 4] = [".png", ".jpg", ".jpeg", ".webp"];

pub(crate) fn parse_frame_id_token(token: &str) -> Result<Vec<i64>, GrabFailure> {
    let invalid = || {
        GrabFailure::runtime(format!(
            "frame ids must be positive integers: got '{token}'"
        ))
    };
    let mut ids = Vec::new();
    for part in token.split(',').map(str::trim) {
        if part.is_empty() {
            return Err(invalid());
        }
        let id = part.parse::<i64>().map_err(|_| invalid())?;
        if id < 1 {
            return Err(invalid());
        }
        if ids.contains(&id) {
            return Err(GrabFailure::runtime(format!(
                "frame ids must be unique: {id}"
            )));
        }
        ids.push(id);
    }
    ids.sort_unstable();
    Ok(ids)
}

pub(crate) fn resolve_output_paths(
    target: &Path,
    ids: &[i64],
) -> Result<Vec<PathBuf>, GrabFailure> {
    let suffix = target
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| format!(".{}", extension.to_ascii_lowercase()))
        .unwrap_or_default();
    if !SUPPORTED_OUTPUT_SUFFIXES.contains(&suffix.as_str()) {
        return Err(GrabFailure::Usage(
            "--out must end in .png, .jpg, .jpeg, or .webp".to_owned(),
        ));
    }
    if ids.len() == 1 {
        return Ok(vec![target.to_path_buf()]);
    }
    let stem = target
        .file_stem()
        .and_then(|value| value.to_str())
        .ok_or_else(|| GrabFailure::runtime("output path has no valid filename"))?;
    let extension = target
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    let parent = target.parent().unwrap_or_else(|| Path::new(""));
    Ok(ids
        .iter()
        .map(|id| parent.join(format!("{stem}_{id}.{extension}")))
        .collect())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{parse_frame_id_token, resolve_output_paths};

    #[test]
    fn frame_ids_are_validated_and_sorted() {
        assert_eq!(parse_frame_id_token("7, 2,10").unwrap(), vec![2, 7, 10]);
        for token in ["", "1,,2", "x", "0", "-1"] {
            assert!(parse_frame_id_token(token).is_err(), "{token}");
        }
        assert_eq!(
            parse_frame_id_token("2,2").unwrap_err().to_string(),
            "frame ids must be unique: 2"
        );
    }

    #[test]
    fn output_paths_match_single_and_batch_rules() {
        assert_eq!(
            resolve_output_paths(Path::new("out.png"), &[2]).unwrap(),
            vec![Path::new("out.png")]
        );
        assert_eq!(
            resolve_output_paths(Path::new("dir/out.JPEG"), &[2, 7]).unwrap(),
            vec![Path::new("dir/out_2.JPEG"), Path::new("dir/out_7.JPEG")]
        );
        assert!(resolve_output_paths(Path::new("out.gif"), &[1]).is_err());
    }
}
