//! Reaping of decoded-audio scratch that a killed analysis left behind.
//!
//! VAD, sound tagging, speaker analysis and speaker discovery each write a
//! segment's decoded audio (or embeddings derived from it) into a private
//! temporary directory and remove it when the pass ends, errors included. A
//! process killed mid-pass never reaches that removal, so the journal sweeps
//! for leftovers instead.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::audio::VAD_TEMP_PREFIX;
use crate::speakers::{TEMP_PREFIX as SPEAKERS_TEMP_PREFIX, speakers_temp_root};

/// Age past which a scratch directory has no live writer. Every pass is bounded
/// well inside it: speaker analysis, the longest, at forty minutes plus its
/// signal graces.
pub const ANALYSIS_SCRATCH_MAX_AGE: Duration = Duration::from_secs(60 * 60);

/// Remove analysis scratch directories older than [`ANALYSIS_SCRATCH_MAX_AGE`]
/// and return how many were removed. Best effort: an unreadable root or a
/// directory another account owns is left alone.
pub fn sweep_stale_analysis_scratch() -> usize {
    sweep_stale_analysis_scratch_in(
        &speakers_temp_root(),
        &std::env::temp_dir(),
        ANALYSIS_SCRATCH_MAX_AGE,
        SystemTime::now(),
    )
}

fn sweep_stale_analysis_scratch_in(
    speakers_root: &Path,
    temp_root: &Path,
    max_age: Duration,
    now: SystemTime,
) -> usize {
    let mut roots: Vec<(&Path, Vec<&str>)> = vec![(speakers_root, vec![SPEAKERS_TEMP_PREFIX])];
    let temp_prefixes = [
        VAD_TEMP_PREFIX,
        solstone_core_sound_tags::CED_ANALYZE_TEMP_PREFIX,
    ];
    if temp_root == speakers_root {
        roots[0].1.extend(temp_prefixes);
    } else {
        roots.push((temp_root, temp_prefixes.to_vec()));
    }
    roots
        .iter()
        .map(|(root, prefixes)| sweep_stale_dirs_at(root, prefixes, max_age, now))
        .sum()
}

fn sweep_stale_dirs_at(
    root: &Path,
    prefixes: &[&str],
    max_age: Duration,
    now: SystemTime,
) -> usize {
    let Ok(entries) = fs::read_dir(root) else {
        return 0;
    };
    entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            // `file_type` does not follow a symlink, so a link planted in a
            // shared temporary root is never treated as one of ours.
            let stale = entry.file_type().is_ok_and(|kind| kind.is_dir())
                && prefixes
                    .iter()
                    .any(|prefix| entry.file_name().to_string_lossy().starts_with(prefix))
                && entry
                    .metadata()
                    .and_then(|metadata| metadata.modified())
                    .is_ok_and(|modified| {
                        now.duration_since(modified).is_ok_and(|age| age > max_age)
                    });
            stale.then(|| entry.path())
        })
        .filter(|path: &PathBuf| fs::remove_dir_all(path).is_ok())
        .count()
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use super::sweep_stale_analysis_scratch_in;

    fn dir_aged(root: &Path, name: &str, modified: SystemTime) -> std::path::PathBuf {
        let path = root.join(name);
        fs::create_dir(&path).unwrap();
        fs::write(path.join("audio.f32le"), [0_u8; 8]).unwrap();
        fs::File::open(&path)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(modified))
            .unwrap();
        path
    }

    #[test]
    fn stale_scratch_of_every_kind_is_removed_and_fresh_or_foreign_entries_stay() {
        let speakers = tempfile::tempdir().unwrap();
        let temp = tempfile::tempdir().unwrap();
        let now = UNIX_EPOCH + Duration::from_secs(100_000);
        let old = UNIX_EPOCH;
        let fresh = now - Duration::from_secs(60);

        let removed = [
            dir_aged(
                speakers.path(),
                "solstone-speakers-analyze-day-seg-src-1-a",
                old,
            ),
            dir_aged(
                speakers.path(),
                "solstone-speakers-analyze-discovery-cluster-1-0",
                old,
            ),
            dir_aged(temp.path(), "solstone-transcribe-vad-a", old),
            dir_aged(temp.path(), "solstone-ced-analyze-a", old),
        ];
        let kept = [
            dir_aged(speakers.path(), "solstone-speakers-analyze-live", fresh),
            dir_aged(temp.path(), "solstone-transcribe-vad-live", fresh),
            dir_aged(temp.path(), "unrelated-old", old),
            // Only the speakers prefix belongs to the speakers root.
            dir_aged(speakers.path(), "solstone-transcribe-vad-elsewhere", old),
        ];
        let file = temp.path().join("solstone-ced-analyze-file");
        fs::write(&file, b"keep").unwrap();

        assert_eq!(
            sweep_stale_analysis_scratch_in(
                speakers.path(),
                temp.path(),
                Duration::from_secs(3600),
                now
            ),
            removed.len()
        );
        for path in removed {
            assert!(!path.exists(), "{} should be reaped", path.display());
        }
        for path in kept {
            assert!(path.exists(), "{} should stay", path.display());
        }
        assert!(file.exists());
    }

    #[test]
    fn one_shared_root_is_swept_for_every_prefix() {
        let root = tempfile::tempdir().unwrap();
        let now = UNIX_EPOCH + Duration::from_secs(100_000);
        let speakers = dir_aged(root.path(), "solstone-speakers-analyze-x", UNIX_EPOCH);
        let vad = dir_aged(root.path(), "solstone-transcribe-vad-x", UNIX_EPOCH);
        let ced = dir_aged(root.path(), "solstone-ced-analyze-x", UNIX_EPOCH);

        assert_eq!(
            sweep_stale_analysis_scratch_in(
                root.path(),
                root.path(),
                Duration::from_secs(3600),
                now
            ),
            3
        );
        assert!(!speakers.exists() && !vad.exists() && !ced.exists());
    }

    #[cfg(unix)]
    #[test]
    fn a_planted_symlink_is_not_followed() {
        let root = tempfile::tempdir().unwrap();
        let victim = tempfile::tempdir().unwrap();
        fs::write(victim.path().join("keep"), b"keep").unwrap();
        let link = root.path().join("solstone-speakers-analyze-link");
        std::os::unix::fs::symlink(victim.path(), &link).unwrap();

        assert_eq!(
            sweep_stale_analysis_scratch_in(
                root.path(),
                root.path(),
                Duration::ZERO,
                SystemTime::now() + Duration::from_secs(3600)
            ),
            0
        );
        assert!(victim.path().join("keep").exists());
    }
}
