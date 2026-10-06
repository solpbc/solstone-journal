// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#[cfg(all(test, feature = "full-tests"))]
#[test]
fn indexer_classifications() {
    use std::io::Write;
    use std::process::{Child, Output, Stdio};
    use std::time::{Duration, Instant};

    const TEST_ENV: &str = "SOLSTONE_TEST_INDEXER_CLASSIFICATIONS_CHILD";
    if let Ok(raw) = std::env::var(TEST_ENV) {
        install_logger();
        let (route, mode) = raw.split_once(':').unwrap();
        let mut args = vec![
            std::ffi::OsString::from("indexer"),
            std::ffi::OsString::from("classifications"),
            std::ffi::OsString::from("--json"),
        ];
        if mode == "apply" || mode == "drain" {
            args.push(std::ffi::OsString::from("--apply"));
        }
        if mode == "drain" || mode == "invalid" {
            args.push(std::ffi::OsString::from("--drain"));
        }
        let exit = if route == "journal" {
            solstone_core_journal_cli::run(args)
        } else {
            args.push(std::ffi::OsString::from("--journal"));
            args.push(std::env::var_os("SOLSTONE_JOURNAL").unwrap());
            match evaluate_args(&args) {
                Ok(Command::Indexer(command)) => run_indexer(*command),
                Err(_) => ExitCode::from(2),
                _ => panic!("unexpected indexer command"),
            }
        };
        std::io::stdout().flush().unwrap();
        std::io::stderr().flush().unwrap();
        std::process::exit(if exit == ExitCode::SUCCESS { 0 } else { 1 });
    }

    struct Running {
        child: Child,
        logs: tempfile::TempDir,
    }
    impl Drop for Running {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
    impl Running {
        fn finish(mut self) -> Output {
            let deadline = Instant::now() + Duration::from_secs(15);
            let status = loop {
                if let Some(status) = self.child.try_wait().unwrap() {
                    break status;
                }
                assert!(
                    Instant::now() < deadline,
                    "classification child timed out: {}",
                    fs::read_to_string(self.logs.path().join("stderr")).unwrap_or_default()
                );
                std::thread::sleep(Duration::from_millis(10));
            };
            Output {
                status,
                stdout: fs::read(self.logs.path().join("stdout")).unwrap(),
                stderr: fs::read(self.logs.path().join("stderr")).unwrap(),
            }
        }
    }
    fn spawn(journal: &Path, route: &str, mode: &str, release: Option<&Path>) -> Running {
        let logs = tempfile::tempdir().unwrap();
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command
            .env(TEST_ENV, format!("{route}:{mode}"))
            .env("SOLSTONE_JOURNAL", journal)
            .env("RUST_LOG", "info")
            .arg("--exact")
            .arg("tests::indexer_classifications")
            .arg("--nocapture")
            .arg("--quiet")
            .stdin(Stdio::null())
            .stdout(fs::File::create(logs.path().join("stdout")).unwrap())
            .stderr(fs::File::create(logs.path().join("stderr")).unwrap());
        if let Some(release) = release {
            command.env("SOLSTONE_TEST_CLASSIFICATION_BATCH_RELEASE", release);
        }
        Running {
            child: command.spawn().unwrap(),
            logs,
        }
    }
    fn json(output: &Output) -> Value {
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        serde_json::from_str(stdout.lines().last().unwrap()).unwrap()
    }
    fn seed(journal: &Path, count: usize) {
        let conn = solstone_core_indexer_store::db::open_index(journal).unwrap();
        for n in 0..count {
            conn.execute(
                "INSERT INTO chunks(content,path) VALUES('fixture',?1)",
                [format!("item-{n:02}.md")],
            )
            .unwrap();
        }
        drop(conn);
        solstone_core_indexer_store::apply_path_lookup(journal).unwrap();
    }

    let temp = tempfile::tempdir().unwrap();
    for route in ["native", "journal"] {
        let absent = temp.path().join(format!("{route}-absent"));
        fs::create_dir_all(&absent).unwrap();
        assert_eq!(
            json(&spawn(&absent, route, "inspect", None).finish())["initialization"],
            "absent"
        );
        assert!(!absent.join("indexer").exists());
        assert!(!spawn(&absent, route, "invalid", None).finish().status.success());
        assert!(!absent.join("indexer").exists());
        assert!(!spawn(&absent, route, "apply", None).finish().status.success());
        assert!(!absent.join("indexer/journal.sqlite").exists());
        assert!(absent.join("indexer/journal.sqlite.lock").is_file());
        assert_eq!(fs::read_dir(absent.join("indexer")).unwrap().count(), 1);

        let journal = temp.path().join(format!("{route}-ready"));
        seed(&journal, 40);
        let db = solstone_core_indexer_store::db::db_path(&journal);
        let before = fs::read(&db).unwrap();
        let inspected = json(&spawn(&journal, route, "inspect", None).finish());
        assert_eq!(inspected["remaining"], 40);
        assert_eq!(inspected["missing"], 40);
        assert_eq!(fs::read(&db).unwrap(), before);
        let first = json(&spawn(&journal, route, "apply", None).finish());
        assert_eq!(first["processed"], 32);
        assert_eq!(first["cursor"], "item-31.md");
        assert_eq!(first["remaining"], 8);
        let last = json(&spawn(&journal, route, "drain", None).finish());
        assert_eq!(last["processed"], 8);
        assert_eq!(last["completed"], true);
        assert_eq!(last["missing"], 0);

        // Keep a real writer open so these frames stay in WAL while inspection runs.
        let writer = rusqlite::Connection::open(&db).unwrap();
        writer.execute("INSERT INTO chunks(content,path) VALUES('active WAL','new.md')", []).unwrap();
        let wal = journal.join("indexer/journal.sqlite-wal");
        let wal_before = fs::read(&wal).unwrap();
        assert!(!wal_before.is_empty());
        json(&spawn(&journal, route, "inspect", None).finish());
        assert_eq!(fs::read(&wal).unwrap(), wal_before);
        drop(writer);

        let progress = temp.path().join(format!("{route}-progress"));
        seed(&progress, 40);
        let release = temp.path().join(format!("{route}.release"));
        let mut child = spawn(&progress, route, "drain", Some(&release));
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let stderr = fs::read_to_string(child.logs.path().join("stderr")).unwrap();
            if release.with_extension("ready").is_file()
                && stderr.contains("classification batch processed=32")
            {
                break;
            }
            assert!(
                child.child.try_wait().unwrap().is_none(),
                "child exited before committed progress: {stderr}"
            );
            assert!(Instant::now() < deadline, "no live progress: {stderr}");
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(child.child.try_wait().unwrap().is_none());
        let conn = rusqlite::Connection::open_with_flags(
            solstone_core_indexer_store::db::db_path(&progress),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        ).unwrap();
        let (cursor, completed): (String, i64) = conn.query_row(
            "SELECT cursor,completed FROM chunk_classification_backfill WHERE id=1",
            [], |row| Ok((row.get(0)?,row.get(1)?)),
        ).unwrap();
        assert_eq!(cursor, "item-31.md");
        assert_eq!(completed, 0);
        drop(conn);
        fs::write(&release, b"continue").unwrap();
        let final_status = json(&child.finish());
        assert_eq!(final_status["processed"], 40);
        assert_eq!(final_status["completed"], true);
        assert_eq!(final_status["missing"], 0);
    }
}
