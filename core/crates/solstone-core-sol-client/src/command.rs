// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::decode::decode_response;
use crate::error::ClientError;
use crate::seam::{
    BuildIdentityProvider, ClientItemIdProvider, Clock, FileProvider, HttpTransport,
    LinkJoinPairingSeam, LinkServeRunner, LinkStatusProbe, NotificationSink,
};
use crate::transport::{ApiRequest, HttpMethod, TimeoutPolicy};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Clone, Copy)]
pub struct CommandContext<'a> {
    pub args: &'a [String],
    pub env: &'a BTreeMap<String, String>,
    pub stdin: &'a str,
    pub transport: &'a dyn HttpTransport,
    pub clock: Option<&'a dyn Clock>,
    pub files: Option<&'a dyn FileProvider>,
    pub build_identity: Option<&'a dyn BuildIdentityProvider>,
    pub client_item_ids: Option<&'a dyn ClientItemIdProvider>,
    pub notification_sink: Option<&'a dyn NotificationSink>,
    pub link_pairing: Option<&'a dyn LinkJoinPairingSeam>,
    pub link_serve: Option<&'a dyn LinkServeRunner>,
    pub link_status_probe: Option<&'a dyn LinkStatusProbe>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit: i32,
}

impl CommandOutput {
    #[must_use]
    pub fn success(stdout: impl Into<String>) -> Self {
        Self {
            stdout: stdout.into(),
            stderr: String::new(),
            exit: 0,
        }
    }

    #[must_use]
    pub fn failure(stderr: impl Into<String>, exit: i32) -> Self {
        Self {
            stdout: String::new(),
            stderr: stderr.into(),
            exit,
        }
    }
}

/// The journal's today as a `YYYYMMDD` key. The journal files days on its
/// own zone, which this computer's clock may not share, so a command that
/// defaults to "today" asks the journal rather than reading the local date.
pub fn journal_today(ctx: CommandContext<'_>) -> Result<String, ClientError> {
    let response = ctx.transport.request(ApiRequest {
        method: HttpMethod::Get,
        path: "/api/shell".to_owned(),
        params: vec![],
        json: None,
        headers: vec![],
        policy: TimeoutPolicy::Api,
    })?;
    decode_response(&response)?
        .pointer("/clock/today")
        .and_then(Value::as_str)
        .filter(|day| day.len() == 8 && day.bytes().all(|byte| byte.is_ascii_digit()))
        .map(str::to_owned)
        .ok_or(ClientError::MalformedSuccess {
            status: Some(response.status),
        })
}

/// A scripted `/api/shell` exchange answering with the journal's `today`.
#[cfg(test)]
pub(crate) fn shell_today_call(today: &str) -> crate::seam::ExpectedHttpCall {
    crate::seam::ExpectedHttpCall::Request {
        expected: ApiRequest {
            method: HttpMethod::Get,
            path: "/api/shell".to_owned(),
            params: vec![],
            json: None,
            headers: vec![],
            policy: TimeoutPolicy::Api,
        },
        result: Ok(crate::transport::HttpResponse {
            status: 200,
            headers: vec![],
            body: serde_json::json!({"clock": {"tz": "Pacific/Kiritimati", "today": today}})
                .to_string()
                .into_bytes(),
            policy: TimeoutPolicy::Api,
        }),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::seam::{ExpectedHttpCall, ScriptedHttpTransport};

    fn context<'a>(
        env: &'a BTreeMap<String, String>,
        transport: &'a ScriptedHttpTransport,
    ) -> CommandContext<'a> {
        CommandContext {
            args: &[],
            env,
            stdin: "",
            transport,
            clock: None,
            files: None,
            build_identity: None,
            client_item_ids: None,
            notification_sink: None,
            link_pairing: None,
            link_serve: None,
            link_status_probe: None,
        }
    }

    #[test]
    fn journal_today_is_the_day_the_journal_reports() {
        let env = BTreeMap::new();
        let transport = ScriptedHttpTransport::new(vec![shell_today_call("20261001")]);
        assert_eq!(
            journal_today(context(&env, &transport)),
            Ok("20261001".to_owned())
        );
        transport.assert_done();
    }

    #[test]
    fn a_journal_that_reports_no_today_is_a_malformed_answer() {
        let env = BTreeMap::new();
        let ExpectedHttpCall::Request { expected, .. } = shell_today_call("20261001") else {
            unreachable!()
        };
        let transport = ScriptedHttpTransport::new(vec![ExpectedHttpCall::Request {
            expected,
            result: Ok(crate::transport::HttpResponse {
                status: 200,
                headers: vec![],
                body: br#"{"clock":{"tz":"UTC"}}"#.to_vec(),
                policy: TimeoutPolicy::Api,
            }),
        }]);
        assert_eq!(
            journal_today(context(&env, &transport)),
            Err(ClientError::MalformedSuccess { status: Some(200) })
        );
    }
}
