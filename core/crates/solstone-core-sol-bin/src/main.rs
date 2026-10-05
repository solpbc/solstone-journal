// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::process::ExitCode;

fn main() -> ExitCode {
    solstone_core_sol::process_main()
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use std::fs;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::path::Path;
    use std::process::ExitCode;

    const TEST_ENV: &str = "SOLSTONE_TEST_JOURNAL_CLASSIFICATIONS_CHILD";

    #[test]
    fn journal_indexer_classifications_progress_and_drain() {
        if let Ok(raw) = std::env::var(TEST_ENV) {
            let parts: Vec<&str> = raw.split('\x1f').collect();
            let journal_path = parts[0];
            let mode = parts[1];
            let mut args = vec![
                std::ffi::OsString::from("indexer"),
                std::ffi::OsString::from("classifications"),
            ];
            if mode == "apply" {
                args.push(std::ffi::OsString::from("--apply"));
            } else if mode == "drain" {
                args.push(std::ffi::OsString::from("--apply"));
                args.push(std::ffi::OsString::from("--drain"));
            }
            if parts.get(2) == Some(&"json") {
                args.push(std::ffi::OsString::from("--json"));
            }
            // Set SOLSTONE_JOURNAL so journal resolution finds the temp journal
            unsafe {
                std::env::set_var("SOLSTONE_JOURNAL", journal_path);
            }
            let exit_code = solstone_core_sol::run_journal(args);
            let _ = std::io::stdout().flush();
            let _ = std::io::stderr().flush();
            let code = match exit_code {
                code if code == ExitCode::SUCCESS => 0,
                _ => 75,
            };
            std::process::exit(code);
        }

        let temp = tempfile::tempdir_in("/var/tmp").unwrap();
        let journal_dir = temp.path().join("journal");
        fs::create_dir_all(&journal_dir).unwrap();

        // Populate database with 40 chunks
        {
            let conn = solstone_core_indexer_store::open_index(&journal_dir).unwrap();
            for i in 0..40 {
                let path = format!("item-{i:02}.md");
                conn.execute(
                    "INSERT INTO chunks(content, path) VALUES ('text', ?1)",
                    [&path],
                )
                .unwrap();
            }
            drop(conn);
            solstone_core_indexer_store::apply_path_lookup(&journal_dir).unwrap();
        }

        #[cfg(unix)]
        {
            let fifo_dir = tempfile::tempdir_in("/var/tmp").unwrap();
            let fifo_path = fifo_dir.path().join("sol_batch_hold.fifo");
            let c_path = std::ffi::CString::new(fifo_path.to_str().unwrap()).unwrap();
            unsafe {
                libc::mkfifo(c_path.as_ptr(), 0o600);
            }

            let fifo_path_clone = fifo_path.clone();
            let journal_path_clone = journal_dir.clone();

            let child_handle = std::thread::spawn(move || {
                let bin = std::env::var("CARGO_BIN_EXE_solstone-core-sol")
                    .unwrap_or_else(|_| std::env::current_exe().unwrap().display().to_string());
                let mut cmd = std::process::Command::new(bin);
                if std::env::var("CARGO_BIN_EXE_solstone-core-sol").is_ok() {
                    cmd.arg("journal")
                        .arg("indexer")
                        .arg("classifications")
                        .arg("--apply")
                        .arg("--drain")
                        .arg("--json");
                } else {
                    cmd.env(
                        TEST_ENV,
                        format!("{}\x1fdrain\x1fjson", journal_path_clone.display()),
                    );
                    cmd.arg("--exact")
                        .arg("tests::journal_indexer_classifications_progress_and_drain")
                        .arg("--nocapture");
                }
                cmd.env("SOLSTONE_JOURNAL", &journal_path_clone)
                    .env("RUST_LOG", "info")
                    .env(
                        "SOLSTONE_INDEXER_CLASSIFICATION_BATCH_HOLD",
                        &fifo_path_clone,
                    )
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::piped());
                cmd.spawn().expect("spawn child")
            });

            let mut child = child_handle.join().unwrap();
            let stderr = child.stderr.take().unwrap();
            let stdout = child.stdout.take().unwrap();

            // Read stderr until progress line
            let mut reader = BufReader::new(stderr);
            let mut found_progress = false;
            let mut line = String::new();
            while reader.read_line(&mut line).unwrap() > 0 {
                if line.contains("classification batch processed=") {
                    found_progress = true;
                    break;
                }
                line.clear();
            }
            assert!(
                found_progress,
                "must see classification batch progress line"
            );

            // Release FIFO
            let mut fifo_writer = fs::OpenOptions::new()
                .write(true)
                .open(&fifo_path)
                .expect("open fifo writer");
            fifo_writer.write_all(b"x").expect("write release byte");
            drop(fifo_writer);

            let status = child.wait().expect("child wait");
            assert!(status.success());

            let mut out_reader = BufReader::new(stdout);
            let mut out_str = String::new();
            out_reader.read_to_string(&mut out_str).unwrap();
            let final_json: serde_json::Value =
                serde_json::from_str(out_str.trim()).expect("stdout is valid single json");
            assert_eq!(final_json["completed"], true);
            assert_eq!(final_json["processed"], 40);
        }
    }
}
