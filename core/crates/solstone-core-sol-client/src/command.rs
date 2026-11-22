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

/// Select a plural form while preserving the exact count in its separate display argument.
/// Above the portable catalog range, the final six digits cover the supported CLDR rules.
pub(crate) fn count_selector(count: u64) -> solstone_core_cli_copy::Value<'static> {
    let quantity =
        i32::try_from(count).map_or_else(|_| 1_000_000 + (count % 1_000_000) as i64, i64::from);
    solstone_core_cli_copy::Value::Cardinal(quantity)
}

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

impl CommandContext<'_> {
    /// Select display copy for this invocation. Machine switches keep their source English.
    #[must_use]
    pub fn human_locale(self) -> &'static str {
        self.human_locale_with_options(&[])
    }

    pub(crate) fn human_locale_with_options(self, options: &[&str]) -> &'static str {
        self.human_locale_matching(|token| options.contains(&token))
    }

    pub(crate) fn human_locale_matching(self, takes_value: impl Fn(&str) -> bool) -> &'static str {
        self.locale_for_args(takes_value, false)
    }

    pub(crate) fn human_locale_before_separator(self, options: &[&str]) -> &'static str {
        self.locale_for_args(|token| options.contains(&token), true)
    }

    fn locale_for_args(
        self,
        takes_value: impl Fn(&str) -> bool,
        stop_at_separator: bool,
    ) -> &'static str {
        let locale = solstone_core_cli_copy::packaged_locale(
            self.env.get("SOLSTONE_LOCALE").map(String::as_str),
        );
        if locale == solstone_core_cli_copy::ENGLISH {
            return locale;
        }
        let mut args = self.args.iter().peekable();
        while let Some(arg) = args.next() {
            if stop_at_separator && arg == "--" {
                break;
            }
            let structured = matches!(
                arg.as_str(),
                "--json" | "--jsonl" | "--format=json" | "--format=jsonl"
            ) || (arg == "--format"
                && args
                    .peek()
                    .is_some_and(|next| matches!(next.as_str(), "json" | "jsonl")));
            if structured {
                return solstone_core_cli_copy::ENGLISH;
            }
            if takes_value(arg) {
                args.next();
            }
        }
        locale
    }
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
        self.read_for(solstone_core_cli_copy::ENGLISH)
    }

    pub fn read_for(self, locale: &str) -> Result<String, CommandOutput> {
        match self {
            Self::Text(text) => Ok(text.to_owned()),
            Self::Source(source) => source.read_body().map_err(|error| {
                CommandOutput::failure(
                    format!(
                        "{}\n",
                        solstone_core_cli_copy::display_for(
                            locale,
                            "journal.cli.client.stdin.read",
                            [("error", &error)]
                        )
                    ),
                    75,
                )
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
    format_erase_day_for(solstone_core_cli_copy::ENGLISH, day, today)
}

fn format_erase_day_for(locale: &str, day: &str, today: Option<&str>) -> Option<String> {
    use solstone_core_cli_copy::{MessageDetail, display_for};
    if day.len() != 8 || !day.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let date = solstone_core_time::parse_day(day)?;
    let weekday_id = [
        "journal.window.home.week.day.short.mon",
        "journal.window.home.week.day.short.tue",
        "journal.window.home.week.day.short.wed",
        "journal.window.home.week.day.short.thu",
        "journal.window.home.week.day.short.fri",
        "journal.window.home.week.day.short.sat",
        "journal.window.home.week.day.short.sun",
    ][date.weekday().to_monday_zero_offset() as usize];
    let weekday = MessageDetail::window(weekday_id, []).render(locale);
    let month_id = [
        "journal.window.shared.date.format.months.since.jan",
        "journal.window.shared.date.format.months.since.feb",
        "journal.window.shared.date.format.months.since.mar",
        "journal.window.shared.date.format.months.since.apr",
        "journal.window.shared.date.format.months.since.may",
        "journal.window.shared.date.format.months.since.jun",
        "journal.window.shared.date.format.months.since.jul",
        "journal.window.shared.date.format.months.since.aug",
        "journal.window.shared.date.format.months.since.sep",
        "journal.window.shared.date.format.months.since.oct",
        "journal.window.shared.date.format.months.since.nov",
        "journal.window.shared.date.format.months.since.dec",
    ][(date.month() - 1) as usize];
    let month = MessageDetail::window(month_id, []).render(locale);
    let day_number = date.day();
    Some(if today.and_then(|today| today.get(..4)) == day.get(..4) {
        display_for(
            locale,
            "journal.cli.client.date.short",
            [
                ("weekday", &weekday),
                ("month", &month),
                ("day", &day_number),
            ],
        )
    } else {
        display_for(
            locale,
            "journal.cli.client.date.short.year",
            [
                ("weekday", &weekday),
                ("month", &month),
                ("day", &day_number),
                ("year", &date.strftime("%y")),
            ],
        )
    })
}

/// The day a delete's answer says its thing is erased on (`trash.erase_day`),
/// written for the owner with [`format_erase_day`]. Asks the journal for its
/// today to know the year; without it the year is always written.
pub fn erase_day_of(ctx: CommandContext<'_>, answer: &Value) -> Option<String> {
    erase_day_of_for(ctx.human_locale(), ctx, answer)
}

pub(crate) fn erase_day_of_for(
    locale: &str,
    ctx: CommandContext<'_>,
    answer: &Value,
) -> Option<String> {
    let day = answer.pointer("/trash/erase_day").and_then(Value::as_str)?;
    let today = journal_today(ctx).ok();
    format_erase_day_for(locale, day, today.as_deref())
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

/// The owner's choice, on a delete that erases now, to also remove it from
/// their backups: `--from-backups` or `--not-from-backups`, else the trash
/// setting. Shown only while backup is set up.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct BackupsChoice {
    pub chosen: bool,
    pub paused: bool,
}

pub const FROM_BACKUPS_HELP: &str = "Also remove it from your backups. Without this or --not-from-backups, the trash setting decides.";
pub const NOT_FROM_BACKUPS_HELP: &str = "Leave it in your backups.";
pub const NO_BACKUP_SET_UP: &str =
    "Error: backup isn't set up, so there are no backups to remove it from.";
/// Backup's state couldn't be read: an erase never goes ahead on a guess.
pub const BACKUPS_UNCHECKED: &str = "Error: your journal couldn't check your backups just now. Try again, or pass --not-from-backups to erase without removing it from your backups.";
pub const NOT_AVAILABLE_HERE: &str =
    "Error: this journal can't remove things from your backups yet.";

/// Work out the choice for an erase now. `from` and `not_from` are the flags.
pub fn backups_choice(
    ctx: CommandContext<'_>,
    from: bool,
    not_from: bool,
) -> Result<BackupsChoice, String> {
    backups_choice_for(ctx.human_locale(), ctx, from, not_from)
}

pub(crate) fn backups_choice_for(
    locale: &str,
    ctx: CommandContext<'_>,
    from: bool,
    not_from: bool,
) -> Result<BackupsChoice, String> {
    if from && not_from {
        return Err(solstone_core_cli_copy::render_for(
            locale,
            "journal.cli.client.erase.backups.conflict",
            &[],
        ));
    }
    if not_from {
        return Ok(BackupsChoice::default());
    }
    let state = ctx
        .transport
        .request(ApiRequest {
            method: HttpMethod::Get,
            path: "/app/settings/api/trash/backups".to_owned(),
            params: vec![],
            json: None,
            headers: vec![],
            policy: TimeoutPolicy::Api,
        })
        .ok()
        .and_then(|response| decode_response(&response).ok());
    // Not known is not "not set up": a setting that ticks it is never dropped
    // without a word.
    if state.is_none() {
        return Err(solstone_core_cli_copy::render_for(
            locale,
            "journal.cli.client.erase.backups.unchecked",
            &[],
        ));
    }
    let set_up = state
        .as_ref()
        .and_then(|state| state.get("set_up"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if from && !set_up {
        return Err(solstone_core_cli_copy::render_for(
            locale,
            "journal.cli.client.erase.backups.missing",
            &[],
        ));
    }
    let available = state
        .as_ref()
        .and_then(|state| state.get("available"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if from && !available {
        return Err(solstone_core_cli_copy::render_for(
            locale,
            "journal.cli.client.erase.backups.unavailable",
            &[],
        ));
    }
    let set_up = set_up && available;
    let setting = state
        .as_ref()
        .and_then(|state| state.get("setting"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let paused = state
        .as_ref()
        .and_then(|state| state.get("paused"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    Ok(BackupsChoice {
        chosen: set_up && (from || setting),
        paused,
    })
}

/// The line after `Erased '...'.` when the choice was made.
#[must_use]
pub fn backups_next_line(choice: BackupsChoice) -> Option<&'static str> {
    match (choice.chosen, choice.paused) {
        (false, _) => None,
        (true, false) => Some("Removing it from your backups next."),
        (true, true) => Some("Removing it from your backups once you resume backup."),
    }
}

pub fn backups_next_line_for(locale: &str, choice: BackupsChoice) -> Option<String> {
    match (choice.chosen, choice.paused) {
        (false, _) => None,
        (true, false) => Some(solstone_core_cli_copy::render_for(
            locale,
            "journal.cli.client.erase.backups.next",
            &[],
        )),
        (true, true) => Some(solstone_core_cli_copy::render_for(
            locale,
            "journal.cli.client.erase.backups.resume",
            &[],
        )),
    }
}

pub fn moved_to_trash_line_for(
    locale: &str,
    name: &str,
    erase_failed: bool,
    day: Option<&str>,
) -> String {
    match (erase_failed, day) {
        (false, None) => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.client.trash.moved",
            [("name", &name)],
        ),
        (false, Some(day)) => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.client.trash.moved.day",
            [("name", &name), ("day", &day)],
        ),
        (true, None) => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.client.trash.moved.failed",
            [("name", &name)],
        ),
        (true, Some(day)) => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.client.trash.moved.failed.day",
            [("name", &name), ("day", &day)],
        ),
    }
}

/// A scripted backup-state exchange, as an erase now asks it.
#[cfg(test)]
pub(crate) fn backups_state_call(state: Value) -> crate::seam::ExpectedHttpCall {
    crate::seam::ExpectedHttpCall::Request {
        expected: ApiRequest {
            method: HttpMethod::Get,
            path: "/app/settings/api/trash/backups".to_owned(),
            params: vec![],
            json: None,
            headers: vec![],
            policy: TimeoutPolicy::Api,
        },
        result: Ok(crate::transport::HttpResponse {
            status: 200,
            headers: vec![],
            body: state.to_string().into_bytes(),
            policy: TimeoutPolicy::Api,
        }),
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
    use serde_json::json;

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
    fn an_erase_never_guesses_the_backup_choice() {
        let env = BTreeMap::new();
        let ExpectedHttpCall::Request { expected, .. } = backups_state_call(json!({})) else {
            unreachable!()
        };
        let failed = || ExpectedHttpCall::Request {
            expected: expected.clone(),
            result: Ok(crate::transport::HttpResponse {
                status: 500,
                headers: vec![],
                body: br#"{"error":"trash_unreadable"}"#.to_vec(),
                policy: TimeoutPolicy::Api,
            }),
        };
        for from in [false, true] {
            let transport = ScriptedHttpTransport::new(vec![failed()]);
            assert_eq!(
                backups_choice(context(&env, &transport), from, false).map(|_| ()),
                Err(BACKUPS_UNCHECKED.to_owned()),
                "from {from}"
            );
        }
        // Not from backups asks nothing and goes ahead.
        let transport = ScriptedHttpTransport::new(vec![]);
        assert!(backups_choice(context(&env, &transport), false, true).is_ok());
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
