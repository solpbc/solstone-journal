// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use super::*;
use crate::bounded_child::{self, Output, Request};
use solstone_core_journal_io::{
    activity::ActivityLease,
    media::{Cleanup, MediaDirectory, MediaStage, digest_reader},
};
use std::{ffi::OsStr, fs, path::Path, time::Duration};

fn child(
    program: &Path,
    argv: Vec<OsString>,
    env: BTreeMap<OsString, OsString>,
    sources: &[ArchiveSource<'_>],
) -> Output {
    let request = Request {
        program: program.as_os_str().to_owned(),
        argv,
        env,
        timeout: Some(Duration::from_secs(30)),
        pass_fds: sources
            .iter()
            .filter_map(ArchiveSource::descriptor)
            .collect(),
    };
    bounded_child::run_owned_archive_creator(&request, Some(1024 * 1024), None)
        .unwrap_or_else(|_| panic!("actual bounded archive child did not settle"))
        .into_output()
}

#[test]
#[ignore = "requires pinned restic0.19.0 via SOLSTONE_NATIVE_RESTIC and a disposable local repository"]
fn android_temp_packs_use_inherited_stage_without_archiving_or_sweeping_residue() {
    let restic = PathBuf::from(std::env::var_os("SOLSTONE_NATIVE_RESTIC").expect("pinned restic"));
    let version = child(&restic, vec!["version".into()], BTreeMap::new(), &[]);
    assert_eq!(version.returncode, 0);
    assert!(version.stdout.starts_with(b"restic 0.19.0 "));

    let temp = tempfile::tempdir().unwrap();
    let media = temp.path().join("media");
    fs::create_dir(&media).unwrap();
    let payload = b"private synthetic archive fixture\n";
    fs::write(media.join("original.wav"), payload).unwrap();
    let digest = digest_reader(&mut payload.as_slice()).unwrap();
    let root = Arc::new(JournalRoot::open(temp.path()).unwrap());
    let activity = ActivityLease::try_acquire(Arc::clone(&root)).unwrap();
    let parent = MediaDirectory::open(root, Path::new("media")).unwrap();
    let stage = MediaStage::create(Arc::clone(&parent), activity).unwrap();
    let stage_path = stage.diagnostic_path();
    let mut prepared = stage
        .create_named_target(OsStr::new("audio.wav"))
        .unwrap()
        .copy_from(&parent, OsStr::new("original.wav"), &digest)
        .unwrap();
    let sources = vec![prepared.archive_source().unwrap()];

    let mut env = BTreeMap::from([
        (
            "RESTIC_REPOSITORY".into(),
            temp.path().join("repository").into_os_string(),
        ),
        ("RESTIC_PASSWORD".into(), "synthetic-password".into()),
        ("TMPDIR".into(), temp.path().as_os_str().to_owned()),
    ]);
    let init = child(
        &restic,
        vec!["init".into(), "--no-cache".into()],
        env.clone(),
        &[],
    );
    assert_eq!(
        init.returncode,
        0,
        "{}",
        String::from_utf8_lossy(&init.stderr)
    );
    env.insert(
        "TMPDIR".into(),
        temp.path().join("unavailable").into_os_string(),
    );
    let failed = child(&restic, creator_argv(&sources), env.clone(), &sources);
    assert_eq!(failed.returncode, 1);
    assert!(String::from_utf8_lossy(&failed.stderr).contains("restic-temp-pack-"));

    let ambient_temp = std::env::var_os("TMPDIR");
    let fixed_env = android_creator_environment(env, &sources);
    assert_eq!(std::env::var_os("TMPDIR"), ambient_temp);
    // An unselected pack-shaped sibling must not enter the snapshot either.
    fs::write(
        stage_path.join("restic-temp-pack-unselected"),
        b"unselected",
    )
    .unwrap();
    let argv = creator_argv(&sources);
    assert!(argv.iter().any(|arg| arg == "--no-cache"));
    let archived = child(&restic, argv, fixed_env.clone(), &sources);
    assert_eq!(
        archived.returncode,
        0,
        "{}",
        String::from_utf8_lossy(&archived.stderr)
    );
    let snapshot = parse_summary(&archived.stdout).unwrap();
    let listed = child(
        &restic,
        vec![
            "ls".into(),
            "--json".into(),
            "--no-cache".into(),
            snapshot.into(),
        ],
        fixed_env.clone(),
        &sources,
    );
    assert_eq!(listed.returncode, 0);
    let files: Vec<_> = listed
        .stdout
        .split(|b| *b == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_slice::<Value>(line).unwrap())
        .filter(|node| node.get("type").and_then(Value::as_str) == Some("file"))
        .collect();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0]["name"], "audio.wav");
    let mut entries: Vec<_> = fs::read_dir(&stage_path)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    entries.sort();
    assert_eq!(
        entries,
        [
            OsString::from("audio.wav"),
            OsString::from("restic-temp-pack-unselected")
        ]
    );

    let interrupted = child(
        Path::new("/bin/sh"),
        vec![
            "-c".into(),
            "printf 'held pack residue' > \"$TMPDIR/restic-temp-pack-interrupted\"; exit 17".into(),
        ],
        fixed_env,
        &sources,
    );
    assert_eq!(interrupted.returncode, 17);
    assert_eq!(
        fs::read(stage_path.join("restic-temp-pack-interrupted")).unwrap(),
        b"held pack residue"
    );
    sources[0].revalidate().unwrap();
    drop(sources);
    assert_ne!(prepared.cleanup(), Cleanup::Complete);
    assert_eq!(
        fs::read(stage_path.join("restic-temp-pack-interrupted")).unwrap(),
        b"held pack residue"
    );
    assert_eq!(
        fs::read(stage_path.join("restic-temp-pack-unselected")).unwrap(),
        b"unselected"
    );
    assert_eq!(fs::read(media.join("original.wav")).unwrap(), payload);
    assert_eq!(std::env::var_os("TMPDIR"), ambient_temp);
}
