// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Explicit native-restic proof of the production backup selection.

use std::fs;
use std::path::Path;
use std::process::Command;

use super::prepare_backup_args;

#[test]
#[ignore = "requires a native restic executable; set SOLSTONE_NATIVE_RESTIC and run explicitly"]
fn real_restic_preserves_owner_names_and_excludes_operational_secrets() {
    assert!(
        std::env::var_os("PATH").is_some(),
        "populated expansion control"
    );
    assert!(
        std::env::var_os("SOLSTONE_UNSET_COPY_TEST").is_none(),
        "unset expansion control"
    );
    let temp = tempfile::Builder::new()
        .prefix("journal-copy-")
        .tempdir()
        .unwrap();
    #[cfg(unix)]
    let root = temp
        .path()
        .join("journal [literal] café $PATH ${PATH} $SOLSTONE_UNSET_COPY_TEST * ? ");
    #[cfg(windows)]
    let root = temp
        .path()
        .join("journal [literal] café $PATH ${PATH} $SOLSTONE_UNSET_COPY_TEST");
    fs::create_dir(&root).unwrap();
    let root = fs::canonicalize(&root).unwrap();
    let kept = [
        "entities/cache/entity.json",
        "entities/indexer/entity.json",
        "facets/cache/facet.json",
        "facets/indexer/facet.json",
        "imports/item/original.key",
        "imports/health-dedupe.sqlite-notes/original.key",
        "imports/health-dedupe.sqlite-wal/original.key",
        "chronicle/20260101/indexer/120001_60.sqlite/original.key",
        "imports/item/.removing",
        "imports/item/notes.removing_original",
        "imports/item/cache/owner.sqlite",
        "chronicle/20260101/indexer/120000_60/slides.key",
        "chronicle/20260101/source/120000_60/audio.jsonl.lock",
        "chronicle/20260101/source/120000_60/.tmp_123_4.tmp",
        "chronicle/20260101/source/120000_60/brain.json",
        "imports/item/mcp-endpoint/pop.ed25519.pk8",
        "health/retention.log",
        "health/pruning-runs/run.jsonl",
        "chronicle/20260101/health/supervisor.process_instance",
        "chronicle/20260101/health/talent-provenance/run.json",
        "config/journal.json",
        "chronicle/20260102/indexer/indexer.sqlite/original.key",
    ];
    let omitted = [
        "indexer/journal.sqlite",
        ".removing_root/owner.json",
        "chronicle/20260101/.removing_flat/owner.json",
        "chronicle/20260101/source/.removing_qualified/owner.json",
        "imports/item/deep/.removing_deep/owner.json",
        "chronicle/20260101/120000_60/.removing_original",
        "imports/.health-dedupe.sqlite.rebuild-journal",
        "cache/providers/model",
        ".cache/derived",
        "chronicle/20260101/indexer/indexer.sqlite-wal",
        "imports/health-dedupe.sqlite",
        "health/brain.json",
        "health/brain-fingerprint.key",
        "health/brain-refresh.lease",
        "health/callosum.sock",
        "health/supervisor.pid",
        "health/supervisor.process_instance",
        "health/callosum.pipe-secret",
        "health/local.port",
        "health/locks/backup-activity.lock",
        "health/sync/state",
        "health/scheduler.json",
        "mcp-endpoint/pop.ed25519.pk8",
        "mcp-endpoint/tokens.json",
        "config/chatgpt-sign-in.json",
        "config/chatgpt-sign-in.json.unreadable-1",
        "config/.tmp_123_4.tmp",
        "config/_stage_123_4.tmp",
        "link/ca/.tmp_123_4.tmp",
        "backup/hosted/.tmp_123_4.tmp",
        "chronicle/20260101/source/.removing_120000_60/tombstone.json",
    ];
    for name in kept.iter().chain(&omitted) {
        let path = root.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, name.as_bytes()).unwrap();
    }
    let restic = std::env::var_os("SOLSTONE_NATIVE_RESTIC");
    #[cfg(windows)]
    let restic = restic.unwrap_or_else(|| pinned_windows_restic(temp.path()).into_os_string());
    #[cfg(not(windows))]
    let restic = restic.expect("native restic path required");
    let repository = temp.path().join("repository");
    let run = |args: &[String]| {
        let output = Command::new(&restic)
            .arg("--repo")
            .arg(&repository)
            .arg("--no-cache")
            .args(args)
            .env("RESTIC_PASSWORD", "synthetic-copy-test")
            .output()
            .expect("restic starts");
        assert!(
            output.status.success(),
            "restic {args:?}: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        output.stdout
    };
    run(&["init".into()]);
    // Seed an older snapshot with the previous basename policy's omissions.
    let previous_prepared = prepare_backup_args(&root).unwrap();
    let mut previous = previous_prepared.args.clone();
    previous.extend([
        "--exclude".into(),
        "cache".into(),
        "--exclude".into(),
        "indexer".into(),
        "--exclude".into(),
        "*.lock".into(),
        "--exclude".into(),
        ".tmp*".into(),
        "--exclude".into(),
        "*.sqlite*".into(),
    ]);
    previous.push("--json".into());
    let snapshot_id = |output: Vec<u8>| {
        String::from_utf8(output)
            .unwrap()
            .lines()
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
            .find_map(|row| {
                row.get("snapshot_id")
                    .and_then(|id| id.as_str())
                    .map(str::to_owned)
            })
            .expect("backup produced a snapshot")
    };
    let old_snapshot = snapshot_id(run(&previous));
    // No source file changes, force flag, migration or rescan before the next run.
    let current_prepared = prepare_backup_args(&root).unwrap();
    let mut current = current_prepared.args.clone();
    current.push("--json".into());
    let new_snapshot = snapshot_id(run(&current));
    assert_ne!(old_snapshot, new_snapshot);
    run(&["check".into()]);
    let previously_missing = [
        "entities/cache/entity.json",
        "entities/indexer/entity.json",
        "facets/cache/facet.json",
        "facets/indexer/facet.json",
        "imports/item/cache/owner.sqlite",
        "chronicle/20260101/indexer/120000_60/slides.key",
        "chronicle/20260101/source/120000_60/audio.jsonl.lock",
        "chronicle/20260101/source/120000_60/.tmp_123_4.tmp",
        "chronicle/20260102/indexer/indexer.sqlite/original.key",
        "imports/health-dedupe.sqlite-notes/original.key",
        "imports/health-dedupe.sqlite-wal/original.key",
        "chronicle/20260101/indexer/120001_60.sqlite/original.key",
    ];
    for (snapshot, previous) in [(old_snapshot, true), (new_snapshot, false)] {
        let restore = temp.path().join(format!("restored-{snapshot}"));
        run(&[
            "restore".into(),
            snapshot,
            "--target".into(),
            crate::restic_filesystem_path(&restore),
        ]);
        let tree_root = crate::restic_tree_path(&root);
        let restored_root = restore.join(Path::new(tree_root.trim_start_matches('/')));
        for name in kept {
            if previous && previously_missing.contains(&name) {
                assert!(
                    !restored_root.join(name).exists(),
                    "previous omission: {name}"
                );
                continue;
            }
            assert_eq!(
                fs::read(restored_root.join(name)).expect(name),
                name.as_bytes(),
                "{name}"
            );
        }
        for name in omitted {
            assert!(!restored_root.join(name).exists(), "{name}");
        }
    }
    // A snapshot taken before the resident identity was omitted carries
    // another machine's copies; restoring it over a live journal keeps this one's.
    let unfiltered = snapshot_id(run(&[
        "backup".into(),
        crate::restic_filesystem_path(&root),
        "--json".into(),
    ]));
    let live = temp.path().join("restored-over-live");
    fs::create_dir_all(live.join("health")).unwrap();
    for name in [
        "health/supervisor.process_instance",
        "health/callosum.pipe-secret",
    ] {
        fs::write(live.join(name), b"live resident").unwrap();
    }
    run(&crate::restore::restore_args(
        &unfiltered,
        &crate::restic_tree_path(&root),
        &crate::restic_filesystem_path(&live),
    ));
    for name in [
        "health/supervisor.process_instance",
        "health/callosum.pipe-secret",
    ] {
        assert_eq!(
            fs::read(live.join(name)).unwrap(),
            b"live resident",
            "{name}"
        );
    }
    for name in [
        "config/journal.json",
        "chronicle/20260101/health/supervisor.process_instance",
    ] {
        assert_eq!(
            fs::read(live.join(name)).expect(name),
            name.as_bytes(),
            "{name}"
        );
    }
    let owner_directory = "imports/health-dedupe.sqlite/original.key";
    fs::remove_file(root.join("imports/health-dedupe.sqlite")).unwrap();
    fs::create_dir(root.join("imports/health-dedupe.sqlite")).unwrap();
    fs::write(root.join(owner_directory), b"owner directory bytes").unwrap();
    let prepared = prepare_backup_args(&root).unwrap();
    let mut args = prepared.args.clone();
    args.push("--json".into());
    let snapshot = snapshot_id(run(&args));
    let restore = temp.path().join("restored-owner-directory");
    run(&[
        "restore".into(),
        snapshot,
        "--target".into(),
        crate::restic_filesystem_path(&restore),
    ]);
    let tree_root = crate::restic_tree_path(&root);
    let restored_root = restore.join(Path::new(tree_root.trim_start_matches('/')));
    assert_eq!(
        fs::read(restored_root.join(owner_directory)).unwrap(),
        b"owner directory bytes"
    );
}

#[cfg(windows)]
fn pinned_windows_restic(directory: &Path) -> std::path::PathBuf {
    // Explicit native proof only. Admission uses the same archive/member pins as
    // the Windows producer; the URL supplies bytes, never trust metadata.
    let mut response = ureq::get("https://github.com/restic/restic/releases/download/v0.19.0/restic_0.19.0_windows_amd64.zip")
        .call().expect("download native restic fixture");
    let bytes = response
        .body_mut()
        .with_config()
        .limit(16 * 1024 * 1024)
        .read_to_vec()
        .expect("read restic fixture");
    let archive = directory.join("restic.zip");
    fs::write(&archive, bytes).unwrap();
    let license = fs::canonicalize(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../distribution/licenses/restic/LICENSE"),
    )
    .expect("canonical restic license path");
    let admitted = solstone_core_distribution::produce::windows_archives::admit_restic(
        &archive,
        &license,
        solstone_core_installed_payload::WindowsArchitecture::X86_64,
    )
    .expect("producer pins admit restic fixture");
    let executable = directory.join("restic.exe");
    fs::write(&executable, &admitted.outputs()["restic.exe"]).unwrap();
    executable
}
