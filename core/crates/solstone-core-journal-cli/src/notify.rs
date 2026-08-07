// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;

use solstone_core_sol_client::aggregate::handler_for;
use solstone_core_sol_client::command::{CommandContext, CommandOutput};
use solstone_core_sol_client::error::ClientError;
use solstone_core_sol_client::seam::{HttpTransport, NotificationSink, NotificationSinkError};
use solstone_core_sol_client::transport::{
    ApiRequest, HttpResponse, SseRequest, SseStream, UploadRequest,
};

use crate::Outcome;
use crate::host::resolution_failure;
use crate::layout::resolve_current_journal;

const EXIT_TEMPFAIL: u8 = 75;
const SOCKET_TIMEOUT: Duration = Duration::from_secs(2);

pub(crate) fn notify(owner_argv: &[OsString]) -> Outcome {
    let args = match owner_argv
        .iter()
        .map(|argument| argument.to_str().map(str::to_owned))
        .collect::<Option<Vec<_>>>()
    {
        Some(args) => args,
        None => {
            return Outcome::LocalFailure {
                stdout: String::new(),
                stderr: "native journal notify failed: owner arguments are not valid UTF-8\n"
                    .to_string(),
                exit: EXIT_TEMPFAIL,
            };
        }
    };
    let journal = match resolve_current_journal() {
        Ok(journal) => journal,
        Err(error) => return resolution_failure(error),
    };
    let sink = UnixNotificationSink::new(journal.path.join("health/callosum.sock"));
    let env = BTreeMap::new();
    let transport = NotifyHttpTransport;
    let (_, handler) = handler_for(&["notify"])
        .expect("solstone-core-sol-client must provide the native notify handler");
    command_output(handler(CommandContext {
        args: &args,
        env: &env,
        stdin: "",
        today: "",
        transport: &transport,
        clock: None,
        chat_events: None,
        files: None,
        build_identity: None,
        client_item_ids: None,
        notification_sink: Some(&sink),
        link_pairing: None,
        link_serve: None,
        journal_root: None,
    }))
}

fn command_output(output: CommandOutput) -> Outcome {
    if output.exit == 0 {
        return Outcome::LocalSuccess {
            stdout: output.stdout,
            stderr: output.stderr,
        };
    }
    Outcome::LocalFailure {
        stdout: output.stdout,
        stderr: output.stderr,
        exit: u8::try_from(output.exit)
            .expect("native notify handler must return an exit code representable by ExitCode"),
    }
}

struct UnixNotificationSink {
    socket_path: PathBuf,
}

impl UnixNotificationSink {
    fn new(socket_path: PathBuf) -> Self {
        Self { socket_path }
    }
}

impl NotificationSink for UnixNotificationSink {
    fn send_line(&self, line: &str) -> Result<(), NotificationSinkError> {
        #[cfg(unix)]
        {
            let mut stream = std::os::unix::net::UnixStream::connect(&self.socket_path)
                .map_err(|_| NotificationSinkError::Unavailable)?;
            stream
                .set_write_timeout(Some(SOCKET_TIMEOUT))
                .map_err(|_| NotificationSinkError::Unavailable)?;
            stream
                .set_read_timeout(Some(SOCKET_TIMEOUT))
                .map_err(|_| NotificationSinkError::Unavailable)?;
            stream
                .write_all(line.as_bytes())
                .map_err(|_| NotificationSinkError::Unavailable)
        }
        #[cfg(not(unix))]
        {
            let _ = line;
            Err(NotificationSinkError::Unavailable)
        }
    }
}

struct NotifyHttpTransport;

impl HttpTransport for NotifyHttpTransport {
    fn request(&self, _request: ApiRequest) -> Result<HttpResponse, ClientError> {
        unreachable!("journal notify never invokes HttpTransport")
    }

    fn upload(&self, _request: UploadRequest) -> Result<HttpResponse, ClientError> {
        unreachable!("journal notify never invokes HttpTransport")
    }

    fn open_sse(&self, _request: SseRequest) -> Result<SseStream, ClientError> {
        unreachable!("journal notify never invokes HttpTransport")
    }
}
