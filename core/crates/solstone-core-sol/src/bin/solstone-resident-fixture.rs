// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::net::TcpListener;
use std::process::ExitCode;

use solstone_core_sol_client::command::{CommandContext, CommandOutput};
use solstone_core_sol_client::error::ClientError;
use solstone_core_sol_client::resident::ResidentCommand;
use solstone_core_sol_client::seam::HttpTransport;
use solstone_core_sol_client::transport::{
    ApiRequest, HttpResponse, SseRequest, SseStream, UploadRequest,
};

const EXIT_TEMPFAIL: i32 = 75;

struct FixtureTransport;

impl HttpTransport for FixtureTransport {
    fn request(&self, _request: ApiRequest) -> Result<HttpResponse, ClientError> {
        panic!("resident fixture must not issue HTTP requests")
    }

    fn upload(&self, _request: UploadRequest) -> Result<HttpResponse, ClientError> {
        panic!("resident fixture must not issue HTTP uploads")
    }

    fn open_sse(&self, _request: SseRequest) -> Result<SseStream, ClientError> {
        panic!("resident fixture must not open SSE streams")
    }
}

fn resident_fixture<'a>(
    _context: CommandContext<'a>,
) -> Result<ResidentCommand<'a>, CommandOutput> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).map_err(|error| {
        CommandOutput::failure(
            format!("resident fixture bind failed: {error}\n"),
            EXIT_TEMPFAIL,
        )
    })?;
    let local = listener.local_addr().map_err(|error| {
        CommandOutput::failure(
            format!("resident fixture local address failed: {error}\n"),
            EXIT_TEMPFAIL,
        )
    })?;
    let startup = format!("resident-fixture\t{}\n", local.port());

    Ok(ResidentCommand::new(startup, move |shutdown| {
        shutdown.wait();
        drop(listener);
        CommandOutput::success("")
    }))
}

fn main() -> ExitCode {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let env = BTreeMap::new();
    let stdin = "";
    let today = "";
    let transport = FixtureTransport;

    solstone_core_sol::run_resident_command(
        resident_fixture,
        CommandContext {
            args: &args,
            env: &env,
            stdin,
            today,
            transport: &transport,
            clock: None,
            files: None,
            build_identity: None,
            client_item_ids: None,
            notification_sink: None,
            link_pairing: None,
            link_serve: None,
            link_status_probe: None,
        },
    )
}
