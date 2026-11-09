// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::env;
use std::fs;

use solstone_core_auth_flow::{BrowserInvocation, BrowserStdio, execute_browser_invocation};

const BROWSER_FIFO_ENV: &str = "SOLSTONE_BROWSER_FIFO";
const BROWSER_STDIO_LEAK: &[u8] = b"browser-stdio-leak";

#[cfg(unix)]
#[test]
#[ignore = "subprocess fixture for browser_opener_does_not_hold_or_contaminate_stdio"]
fn browser_stdio_standin() {
    use std::io::{Write, stderr, stdout};

    let Some(fifo) = env::var_os(BROWSER_FIFO_ENV) else {
        return;
    };
    {
        let mut out = stdout();
        out.write_all(BROWSER_STDIO_LEAK).unwrap();
        out.flush().unwrap();
    }
    {
        let mut err = stderr();
        err.write_all(BROWSER_STDIO_LEAK).unwrap();
        err.flush().unwrap();
    }
    let mut file = fs::OpenOptions::new().write(true).open(fifo).unwrap();
    file.write_all(&[1]).unwrap();
}

#[cfg(unix)]
#[test]
#[ignore = "subprocess fixture for browser_opener_does_not_hold_or_contaminate_stdio"]
fn browser_stdio_opener() {
    let Some(fifo) = env::var_os(BROWSER_FIFO_ENV) else {
        return;
    };
    let exe = env::current_exe().expect("current test executable");
    let invocation = BrowserInvocation {
        program: exe,
        args: vec![
            "--ignored".to_owned(),
            "--exact".to_owned(),
            "oura_browser_stdio::browser_stdio_standin".to_owned(),
            "--nocapture".to_owned(),
        ],
        cwd: None,
        env: BTreeMap::from([(
            BROWSER_FIFO_ENV.to_owned(),
            fifo.to_string_lossy().into_owned(),
        )]),
        stdin: BrowserStdio::Null,
        stdout: BrowserStdio::Null,
        stderr: BrowserStdio::Null,
    };
    execute_browser_invocation(&invocation).expect("spawn stand-in");
}

#[cfg(unix)]
#[test]
fn browser_opener_does_not_hold_or_contaminate_stdio() {
    use std::io::Read;
    use std::process::{Command, Stdio};

    use nix::sys::stat::Mode;
    use nix::unistd::mkfifo;

    let temporary = tempfile::tempdir().unwrap();
    let fifo = temporary.path().join("browser.fifo");
    mkfifo(&fifo, Mode::S_IRUSR | Mode::S_IWUSR).unwrap();

    let opener = Command::new(env::current_exe().expect("current test executable"))
        .args([
            "--ignored",
            "--exact",
            "oura_browser_stdio::browser_stdio_opener",
            "--nocapture",
        ])
        .env(BROWSER_FIFO_ENV, &fifo)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn opener");

    let mut reader = fs::File::open(&fifo).expect("open fifo for read");
    let mut byte = [0_u8; 1];
    reader.read_exact(&mut byte).expect("stand-in wrote a byte");

    let output = opener.wait_with_output().expect("wait for opener");
    assert!(output.status.success());
    for captured in [&output.stdout, &output.stderr] {
        assert!(
            !captured
                .windows(BROWSER_STDIO_LEAK.len())
                .any(|part| part == BROWSER_STDIO_LEAK)
        );
    }
}
