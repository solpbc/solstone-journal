// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::PathBuf;
use std::time::Duration;

use solstone_core_callosum::CallosumOneShotSender;
use solstone_core_sol_client::command::{CommandContext, CommandOutput};
use solstone_core_sol_client::error::ClientError;
use solstone_core_sol_client::seam::{HttpTransport, NotificationSink, NotificationSinkError};
use solstone_core_sol_client::transport::{
    ApiRequest, HttpResponse, SseRequest, SseStream, UploadRequest,
};

use crate::Outcome;
use crate::host::resolution_failure;
use crate::layout::resolve_current_journal;
use crate::notify_handler;

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
    command_output(notify_handler::notify(CommandContext {
        args: &args,
        env: &env,
        stdin: "",
        today: "",
        transport: &transport,
        clock: None,
        files: None,
        build_identity: None,
        client_item_ids: None,
        notification_sink: Some(&sink),
        link_pairing: None,
        link_serve: None,
        link_status_probe: None,
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
        CallosumOneShotSender::new(&self.socket_path, SOCKET_TIMEOUT)
            .send_line(line)
            .map_err(|_| NotificationSinkError::Unavailable)
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
