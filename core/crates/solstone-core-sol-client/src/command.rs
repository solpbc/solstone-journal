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
    pub stdin: Stdin<'a>,
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

/// Where a command's piped body comes from.
pub trait StdinSource {
    /// Reads the whole body. Called at most once, and only by a command that
    /// takes one.
    fn read_body(&self) -> std::io::Result<String>;
}

/// A command's piped body. Nothing reads it until a command asks with
/// [`Stdin::read`], so a command that takes no body never waits on a pipe
/// its caller holds open.
#[derive(Clone, Copy)]
pub enum Stdin<'a> {
    Text(&'a str),
    Source(&'a dyn StdinSource),
}

impl<'a> From<&'a str> for Stdin<'a> {
    fn from(text: &'a str) -> Self {
        Self::Text(text)
    }
}

impl Stdin<'_> {
    /// The body, read now. A read failure is the command's output.
    pub fn read(self) -> Result<String, CommandOutput> {
        match self {
            Self::Text(text) => Ok(text.to_owned()),
            Self::Source(source) => source.read_body().map_err(|error| {
                CommandOutput::failure(format!("native solstone stdin read failed: {error}\n"), 75)
            }),
        }
    }
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

/// The day a thing in the trash is erased, from its `YYYYMMDD` key, as the
/// trash page writes it: lowercase weekday, month and day, with the year when
/// it isn't the year of `today` (also a `YYYYMMDD` key). Never "today" or
/// "tomorrow". `None` for a key that is not a date.
#[must_use]
pub fn format_erase_day(day: &str, today: Option<&str>) -> Option<String> {
    if day.len() != 8 || !day.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let date = chrono::NaiveDate::parse_from_str(day, "%Y%m%d").ok()?;
    let mut text = date.format("%a %b %-d").to_string().to_lowercase();
    if today.and_then(|today| today.get(..4)) != day.get(..4) {
        text.push_str(&date.format(" '%y").to_string());
    }
    Some(text)
}

/// The day a delete's answer says its thing is erased on (`trash.erase_day`),
/// written for the owner with [`format_erase_day`]. Asks the journal for its
/// today to know the year; without it the year is always written.
pub fn erase_day_of(ctx: CommandContext<'_>, answer: &Value) -> Option<String> {
    let day = answer.pointer("/trash/erase_day").and_then(Value::as_str)?;
    let today = journal_today(ctx).ok();
    format_erase_day(day, today.as_deref())
}

/// What the command line says of a thing it moved to the trash: its name,
/// `failed` when an erase asked for now was refused, and the day it's erased
/// on when the answer gave one.
#[must_use]
pub fn moved_to_trash_line(name: &str, failed: &str, day: Option<&str>) -> String {
    match day {
        Some(day) => {
            format!("Moved '{name}' to the trash.{failed} It's erased {day} unless it's recovered.")
        }
        None => format!("Moved '{name}' to the trash.{failed}"),
    }
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
            stdin: "".into(),
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
    fn an_erase_day_reads_as_the_trash_page_writes_it() {
        assert_eq!(
            format_erase_day("20261117", Some("20261111")).as_deref(),
            Some("tue nov 17")
        );
        // Another year's day carries its year; an unknown today always does.
        assert_eq!(
            format_erase_day("20270105", Some("20261231")).as_deref(),
            Some("tue jan 5 '27")
        );
        assert_eq!(
            format_erase_day("20261117", None).as_deref(),
            Some("tue nov 17 '26")
        );
        for day in ["", "2026111", "2026-11-17", "20261341"] {
            assert_eq!(format_erase_day(day, Some("20261111")), None, "{day}");
        }
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
