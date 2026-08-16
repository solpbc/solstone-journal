// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Test-only metadata-helper process fixture.

use std::env;
use std::fs;
use std::io::Write;
use std::process::ExitCode;
use std::thread;

fn main() -> ExitCode {
    let mut args = env::args().skip(1);
    match args.next().as_deref() {
        Some("success") => {
            print!(r#"[{{"CreateDate":"2026:08:01 12:34:56"}}]"#);
            ExitCode::SUCCESS
        }
        Some("malformed") => {
            print!("not-json");
            ExitCode::SUCCESS
        }
        Some("unavailable") => ExitCode::from(1),
        Some("stall") => {
            let Some(marker) = args.next() else {
                return ExitCode::from(2);
            };
            let mut file = match fs::File::create(&marker) {
                Ok(file) => file,
                Err(_) => return ExitCode::from(2),
            };
            if file.write_all(b"started").is_err() || file.flush().is_err() {
                return ExitCode::from(2);
            }
            let _ = file.sync_all();
            drop(file);
            loop {
                thread::park();
            }
        }
        _ => ExitCode::from(2),
    }
}
