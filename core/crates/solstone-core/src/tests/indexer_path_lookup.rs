// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#[cfg(all(test, feature = "full-tests"))]
#[test]
fn indexer_path_lookup() {
    const TEST_ENV: &str = "SOLSTONE_TEST_INDEXER_PATH_LOOKUP_CHILD";
    if let Ok(raw) = std::env::var(TEST_ENV) {
        let parts: Vec<&str> = raw.split('\x1f').collect();
        let journal_path = parts[0];
        let mode = parts[1];
        if mode.starts_with("journal-") {
            let mut args = vec![
                std::ffi::OsString::from("indexer"),
                std::ffi::OsString::from("path-lookup"),
                std::ffi::OsString::from("--json"),
            ];
            if mode == "journal-apply" {
                args.push(std::ffi::OsString::from("--apply"));
            } else if mode == "journal-invalid" {
                args.push(std::ffi::OsString::from("--apply"));
                args.push(std::ffi::OsString::from("--apply"));
            }
            let exit = solstone_core_journal_cli::run(args);
            use std::io::Write;
            std::io::stdout().flush().unwrap();
            std::io::stderr().flush().unwrap();
            std::process::exit(if exit == ExitCode::SUCCESS { 0 } else { 1 });
        }
        let mut args = vec![
            std::ffi::OsString::from("indexer"),
            std::ffi::OsString::from("path-lookup"),
            std::ffi::OsString::from("--journal"),
            std::ffi::OsString::from(journal_path),
        ];
        if mode == "apply" {
            args.push(std::ffi::OsString::from("--apply"));
        }
        let command = evaluate_args(&args).expect("evaluate args in child");
        let exit_code = match command {
            Command::Indexer(cmd) => run_indexer(*cmd),
            _ => panic!("unexpected command parsed"),
        };
        use std::io::Write;
        let _ = std::io::stdout().flush();
        let _ = std::io::stderr().flush();
        let code = match exit_code {
            code if code == ExitCode::SUCCESS => 0,
            _ => EXIT_TEMPFAIL as i32,
        };
        std::process::exit(code);
    }

    let run_cmd = |journal: &Path, mode: &str| -> std::process::Output {
        if let Ok(bin) = std::env::var("CARGO_BIN_EXE_solstone-core") {
            let mut cmd = std::process::Command::new(bin);
            cmd.arg("indexer")
                .arg("path-lookup")
                .arg("--journal")
                .arg(journal);
            if mode == "apply" {
                cmd.arg("--apply");
            }
            cmd.output().expect("spawn CARGO_BIN_EXE_solstone-core")
        } else {
            let current = std::env::current_exe().expect("current_exe");
            let mut cmd = std::process::Command::new(current);
            cmd.env(TEST_ENV, format!("{}\x1f{}", journal.display(), mode));
            cmd.arg("--exact")
                .arg("tests::indexer_path_lookup")
                .arg("--nocapture");
            cmd.output().expect("spawn current_exe re-entry")
        }
    };

    let temp = tempfile::tempdir().unwrap();
    let absent_journal = temp.path().join("absent_journal");
    let output1 = run_cmd(&absent_journal, "inspect");
    assert!(output1.status.success());
    let stdout1 = String::from_utf8_lossy(&output1.stdout);
    assert!(
        stdout1.contains("path lookup: unready"),
        "stdout: {stdout1}"
    );
    assert!(!absent_journal.join("indexer").exists());

    let run_journal = |journal: &Path, mode: &str| -> std::process::Output {
        std::process::Command::new(std::env::current_exe().expect("current_exe"))
            .env(TEST_ENV, format!("{}\x1f{}", journal.display(), mode))
            .env("SOLSTONE_JOURNAL", journal)
            .arg("--exact")
            .arg("tests::indexer_path_lookup")
            .arg("--nocapture")
            .arg("--quiet")
            .output()
            .expect("spawn public journal dispatcher")
    };
    // The re-entered test harness prints its start banner before dispatch.
    let journal_status = |output: &std::process::Output| -> Value {
        serde_json::from_str(
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .last()
                .expect("public dispatcher JSON result"),
        )
        .unwrap()
    };
    fs::create_dir_all(&absent_journal).unwrap();
    let empty_inspection = run_journal(&absent_journal, "journal-inspect");
    assert!(empty_inspection.status.success(), "{empty_inspection:?}");
    let empty_status = journal_status(&empty_inspection);
    assert_eq!(empty_status["ready"], false);
    assert!(!absent_journal.join("indexer").exists());
    let empty_apply = run_journal(&absent_journal, "journal-apply");
    assert!(!empty_apply.status.success());
    assert!(!absent_journal.join("indexer/journal.sqlite").exists());
    assert!(absent_journal.join("indexer/journal.sqlite.lock").is_file());
    assert_eq!(fs::read_dir(absent_journal.join("indexer")).unwrap().count(), 1);

    let existing_journal = temp.path().join("existing_journal");
    fs::create_dir_all(existing_journal.join("indexer")).unwrap();
    let db_file = existing_journal.join("indexer/journal.sqlite");
    {
        let conn = rusqlite::Connection::open(&db_file).unwrap();
        conn.execute_batch(
                "CREATE VIRTUAL TABLE chunks USING fts5(content, path UNINDEXED, day UNINDEXED, facet UNINDEXED, agent UNINDEXED, stream UNINDEXED, idx UNINDEXED, time_bucket UNINDEXED);
                 INSERT INTO chunks(content, path, day, facet, agent, stream, idx, time_bucket) VALUES ('hello', 'test.md', '20260101', '', 'test', '', 0, '');",
            )
            .unwrap();
    }
    let len_before = fs::metadata(&db_file).unwrap().len();
    let master_before: String = {
        let conn = rusqlite::Connection::open(&db_file).unwrap();
        conn.query_row(
            "SELECT sql FROM sqlite_master WHERE type='table' AND name='chunks'",
            [],
            |r| r.get(0),
        )
        .unwrap()
    };

    let output2 = run_cmd(&existing_journal, "inspect");
    assert!(output2.status.success());
    let stdout2 = String::from_utf8_lossy(&output2.stdout);
    assert!(
        stdout2.contains("path lookup: unready"),
        "stdout: {stdout2}"
    );
    assert_eq!(fs::metadata(&db_file).unwrap().len(), len_before);
    let master_after: String = {
        let conn = rusqlite::Connection::open(&db_file).unwrap();
        conn.query_row(
            "SELECT sql FROM sqlite_master WHERE type='table' AND name='chunks'",
            [],
            |r| r.get(0),
        )
        .unwrap()
    };
    assert_eq!(master_after, master_before);

    let public_inspection = run_journal(&existing_journal, "journal-inspect");
    assert!(public_inspection.status.success(), "{public_inspection:?}");
    let public_status = journal_status(&public_inspection);
    assert_eq!(public_status["ready"], false);
    assert_eq!(fs::metadata(&db_file).unwrap().len(), len_before);
    let invalid_apply = run_journal(&existing_journal, "journal-invalid");
    assert!(!invalid_apply.status.success());
    assert_eq!(fs::metadata(&db_file).unwrap().len(), len_before);
    let public_apply = run_journal(&existing_journal, "journal-apply");
    assert!(public_apply.status.success(), "{public_apply:?}");
    let public_ready = journal_status(&public_apply);
    assert_eq!(public_ready["ready"], true);

    let output3 = run_cmd(&existing_journal, "apply");
    assert!(output3.status.success());
    let stdout3 = String::from_utf8_lossy(&output3.stdout);
    assert!(stdout3.contains("path lookup: ready"), "stdout: {stdout3}");
}
