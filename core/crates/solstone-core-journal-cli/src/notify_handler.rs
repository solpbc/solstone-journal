// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fmt::Write as _;

use solstone_core_sol_client::command::{CommandContext, CommandOutput};

const HELP: &str = "usage: journal notify [-h] [--title TITLE] [--icon ICON] [--event EVENT]\n                  [--action ACTION] [--app APP] [--badge BADGE]\n                  [--auto-dismiss AUTO_DISMISS] [--no-dismiss] [-v] [-d]\n                  message [message ...]\n\nSend a notification via callosum\n\npositional arguments:\n  message               notification message text\n\noptions:\n  -h, --help            show this help message and exit\n  --title TITLE         notification title\n  --icon ICON           Lucide icon name (default: mailbox)\n  --event EVENT         event name (default: show)\n  --action ACTION       URL path to open on click\n  --app APP             source app name\n  --badge BADGE         badge text or number\n  --auto-dismiss AUTO_DISMISS\n                        auto-dismiss after N milliseconds\n  --no-dismiss          make notification non-dismissible\n  -v, --verbose         Enable verbose output\n  -d, --debug           Enable debug logging\n";
const FAILURE: &str = "Failed to send notification (is callosum running?)\n";

#[must_use]
pub(crate) fn notify(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(ctx.args) {
        Ok(parsed) => parsed,
        Err(error) => return argparse_error(error),
    };
    if parsed.help {
        return CommandOutput::success(HELP);
    }
    if !parsed.message_present {
        return argparse_error("the following arguments are required: message".to_string());
    }
    let Some(sink) = ctx.notification_sink else {
        return send_failed();
    };
    let line = notification_line(&parsed);
    if sink.send_line(&line).is_err() {
        return send_failed();
    }
    CommandOutput {
        stdout: String::new(),
        stderr: "Notification sent\n".to_string(),
        exit: 0,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedArgs {
    message: String,
    message_present: bool,
    title: Option<String>,
    icon: Option<String>,
    event: String,
    action: Option<String>,
    app: Option<String>,
    badge: Option<String>,
    auto_dismiss: Option<i64>,
    no_dismiss: bool,
    help: bool,
}

impl Default for ParsedArgs {
    fn default() -> Self {
        Self {
            message: String::new(),
            message_present: false,
            title: None,
            icon: None,
            event: "show".to_string(),
            action: None,
            app: None,
            badge: None,
            auto_dismiss: None,
            no_dismiss: false,
            help: false,
        }
    }
}

fn parse_args(args: &[String]) -> Result<ParsedArgs, String> {
    let mut parsed = ParsedArgs::default();
    let mut message = Vec::new();
    let mut index = 0;
    while index < args.len() {
        let token = &args[index];
        if token == "-h" || token == "--help" {
            parsed.help = true;
        } else if token == "-v" || token == "--verbose" || token == "-d" || token == "--debug" {
        } else if token == "--no-dismiss" {
            parsed.no_dismiss = true;
        } else if token == "--" {
            message.extend(args[index + 1..].iter().cloned());
            break;
        } else if let Some(value) = token.strip_prefix("--title=") {
            parsed.title = Some(value.to_string());
        } else if token == "--title" {
            index += 1;
            parsed.title = Some(take_value(args, index, "--title")?.to_string());
        } else if let Some(value) = token.strip_prefix("--icon=") {
            parsed.icon = Some(value.to_string());
        } else if token == "--icon" {
            index += 1;
            parsed.icon = Some(take_value(args, index, "--icon")?.to_string());
        } else if let Some(value) = token.strip_prefix("--event=") {
            parsed.event = value.to_string();
        } else if token == "--event" {
            index += 1;
            parsed.event = take_value(args, index, "--event")?.to_string();
        } else if let Some(value) = token.strip_prefix("--action=") {
            parsed.action = Some(value.to_string());
        } else if token == "--action" {
            index += 1;
            parsed.action = Some(take_value(args, index, "--action")?.to_string());
        } else if let Some(option) = retired_facet_option(token) {
            return Err(format!(
                "{option} is no longer supported; facet selection is workspace-local — use the app's own facet URL/query parameter"
            ));
        } else if let Some(value) = token.strip_prefix("--app=") {
            parsed.app = Some(value.to_string());
        } else if token == "--app" {
            index += 1;
            parsed.app = Some(take_value(args, index, "--app")?.to_string());
        } else if let Some(value) = token.strip_prefix("--badge=") {
            parsed.badge = Some(value.to_string());
        } else if token == "--badge" {
            index += 1;
            parsed.badge = Some(take_value(args, index, "--badge")?.to_string());
        } else if let Some(value) = token.strip_prefix("--auto-dismiss=") {
            parsed.auto_dismiss = Some(parse_auto_dismiss(value)?);
        } else if token == "--auto-dismiss" {
            index += 1;
            let value = take_value(args, index, "--auto-dismiss")?;
            parsed.auto_dismiss = Some(parse_auto_dismiss(value)?);
        } else if token.starts_with('-') {
            return Err(format!("unrecognized arguments: {token}"));
        } else {
            message.push(token.clone());
        }
        index += 1;
    }
    parsed.message_present = !message.is_empty();
    parsed.message = message.join(" ");
    Ok(parsed)
}

fn take_value<'a>(args: &'a [String], index: usize, option: &str) -> Result<&'a str, String> {
    args.get(index)
        .map(String::as_str)
        .ok_or_else(|| format!("argument {option}: expected one argument"))
}

fn parse_auto_dismiss(value: &str) -> Result<i64, String> {
    value
        .parse::<i64>()
        .map_err(|_| format!("argument --auto-dismiss: invalid int value: '{value}'"))
}

fn argparse_error(error: String) -> CommandOutput {
    CommandOutput::failure(format!("{HELP}journal notify: error: {error}\n"), 2)
}

fn send_failed() -> CommandOutput {
    CommandOutput::failure(FAILURE, 1)
}

fn notification_line(parsed: &ParsedArgs) -> String {
    let mut fields = vec![
        json_field("tract", JsonValue::String("notification")),
        json_field("event", JsonValue::String(&parsed.event)),
        json_field("message", JsonValue::String(&parsed.message)),
    ];
    if let Some(value) = parsed.title.as_deref() {
        fields.push(json_field("title", JsonValue::String(value)));
    }
    if let Some(value) = parsed.icon.as_deref() {
        fields.push(json_field("icon", JsonValue::String(value)));
    }
    if let Some(value) = parsed.action.as_deref() {
        fields.push(json_field("action", JsonValue::String(value)));
    }
    if let Some(value) = parsed.app.as_deref() {
        fields.push(json_field("app", JsonValue::String(value)));
    }
    if let Some(value) = parsed.badge.as_deref() {
        fields.push(json_field("badge", JsonValue::String(value)));
    }
    if let Some(value) = parsed.auto_dismiss {
        fields.push(json_field("autoDismiss", JsonValue::Integer(value)));
    }
    if parsed.no_dismiss {
        fields.push(json_field("dismissible", JsonValue::Bool(false)));
    }
    format!("{{{}}}\n", fields.join(", "))
}

fn retired_facet_option(token: &str) -> Option<&'static str> {
    if token == "--facet" || token.starts_with("--facet=") {
        Some("--facet")
    } else {
        None
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JsonValue<'a> {
    String(&'a str),
    Integer(i64),
    Bool(bool),
}

fn json_field(key: &str, value: JsonValue<'_>) -> String {
    format!("{}: {}", python_json_string(key), python_json_value(value))
}

fn python_json_value(value: JsonValue<'_>) -> String {
    match value {
        JsonValue::String(value) => python_json_string(value),
        JsonValue::Integer(value) => value.to_string(),
        JsonValue::Bool(value) => {
            if value {
                "true".to_string()
            } else {
                "false".to_string()
            }
        }
    }
}

fn python_json_string(value: &str) -> String {
    let mut output = String::with_capacity(value.len() + 2);
    output.push('"');
    for ch in value.chars() {
        match ch {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\u{08}' => output.push_str("\\b"),
            '\u{0c}' => output.push_str("\\f"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            ch if (ch as u32) < 0x20 => push_unicode_escape(&mut output, ch as u32),
            ch if (ch as u32) < 0x80 => output.push(ch),
            ch if (ch as u32) <= 0xffff => push_unicode_escape(&mut output, ch as u32),
            ch => {
                let value = ch as u32 - 0x1_0000;
                push_unicode_escape(&mut output, 0xd800 + (value >> 10));
                push_unicode_escape(&mut output, 0xdc00 + (value & 0x03ff));
            }
        }
    }
    output.push('"');
    output
}

fn push_unicode_escape(output: &mut String, value: u32) {
    write!(output, "\\u{value:04x}").expect("write to string");
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use solstone_core_sol_client::command::{CommandContext, CommandOutput};
    use solstone_core_sol_client::seam::{
        NotificationSink, RecordingNotificationSink, ScriptedHttpTransport,
    };

    use super::*;

    fn string_args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    fn run_notify_case(args: &[&str], sink: Option<&RecordingNotificationSink>) -> CommandOutput {
        let args = string_args(args);
        let env = BTreeMap::new();
        let transport = ScriptedHttpTransport::new(vec![]);
        notify(CommandContext {
            args: &args,
            env: &env,
            stdin: "",
            today: "20260723",
            transport: &transport,
            clock: None,
            files: None,
            build_identity: None,
            client_item_ids: None,
            notification_sink: sink.map(|sink| sink as &dyn NotificationSink),
            link_pairing: None,
            link_serve: None,
        })
    }

    #[test]
    fn help_matches_argparse_bytes() {
        let output = run_notify_case(&["--help"], None);

        assert_eq!(output, CommandOutput::success(HELP));
        assert!(HELP.ends_with('\n'));
        assert!(!HELP.contains("facet"));
    }

    #[test]
    fn message_only_emits_minimal_notification_line() {
        let sink = RecordingNotificationSink::new();
        let output = run_notify_case(&["hello", "there"], Some(&sink));

        assert_eq!(
            output,
            CommandOutput {
                stdout: String::new(),
                stderr: "Notification sent\n".to_string(),
                exit: 0,
            }
        );
        assert_eq!(
            sink.recorded(),
            vec![
                "{\"tract\": \"notification\", \"event\": \"show\", \"message\": \"hello there\"}\n"
                    .to_string()
            ]
        );
        let line = &sink.recorded()[0];
        for absent in [
            "\"title\"",
            "\"icon\"",
            "\"action\"",
            "\"facet\"",
            "\"app\"",
            "\"badge\"",
            "\"autoDismiss\"",
            "\"dismissible\"",
        ] {
            assert!(!line.contains(absent), "{absent} should be absent");
        }
    }

    #[test]
    fn empty_message_token_emits_empty_message() {
        let sink = RecordingNotificationSink::new();
        let output = run_notify_case(&[""], Some(&sink));

        assert_eq!(output.exit, 0);
        assert_eq!(
            sink.recorded(),
            vec![
                "{\"tract\": \"notification\", \"event\": \"show\", \"message\": \"\"}\n"
                    .to_string()
            ]
        );
    }

    #[test]
    fn empty_message_tokens_join_with_single_space() {
        let sink = RecordingNotificationSink::new();
        let output = run_notify_case(&["", ""], Some(&sink));

        assert_eq!(output.exit, 0);
        assert_eq!(
            sink.recorded(),
            vec![
                "{\"tract\": \"notification\", \"event\": \"show\", \"message\": \" \"}\n"
                    .to_string()
            ]
        );
    }

    #[test]
    fn all_options_emit_in_python_json_order() {
        let sink = RecordingNotificationSink::new();
        let output = run_notify_case(
            &[
                "--title",
                "Test",
                "--icon",
                "triangle-alert",
                "--event",
                "custom",
                "--action",
                "/open",
                "--app",
                "alerts",
                "--badge",
                "7",
                "--auto-dismiss",
                "3000",
                "--no-dismiss",
                "-v",
                "-d",
                "hello",
                "world",
            ],
            Some(&sink),
        );

        assert_eq!(output.exit, 0);
        assert_eq!(
            sink.recorded(),
            vec!["{\"tract\": \"notification\", \"event\": \"custom\", \"message\": \"hello world\", \"title\": \"Test\", \"icon\": \"triangle-alert\", \"action\": \"/open\", \"app\": \"alerts\", \"badge\": \"7\", \"autoDismiss\": 3000, \"dismissible\": false}\n".to_string()]
        );
        let line = &sink.recorded()[0];
        assert!(line.contains("\"autoDismiss\": 3000"));
        assert!(!line.contains("\"autoDismiss\": \"3000\""));
        assert!(line.contains("\"dismissible\": false"));
        assert!(!line.contains("\"dismissible\": true"));
        assert!(!line.contains("\"facet\""));
    }

    #[test]
    fn facet_options_are_rejected_before_a_notification_is_sent() {
        for values in [
            &["--facet", "work", "hello"][..],
            &["--facet=work", "hello"][..],
        ] {
            let sink = RecordingNotificationSink::new();
            let output = run_notify_case(values, Some(&sink));

            assert_eq!(output.exit, 2, "{values:?}");
            assert!(
                output.stderr.contains("facet selection is workspace-local"),
                "{values:?}: {}",
                output.stderr
            );
            assert!(sink.recorded().is_empty(), "{values:?}");
        }
    }

    #[test]
    fn non_ascii_matches_python_json_dumps_ensure_ascii() {
        let sink = RecordingNotificationSink::new();
        let output = run_notify_case(&["--icon", "triangle-alert", "h\u{e9}llo"], Some(&sink));

        assert_eq!(output.exit, 0);
        assert_eq!(
            sink.recorded(),
            vec![
                "{\"tract\": \"notification\", \"event\": \"show\", \"message\": \"h\\u00e9llo\", \"icon\": \"triangle-alert\"}\n"
                    .to_string()
            ]
        );
    }

    #[test]
    fn icon_option_remains_transport_only() {
        let sink = RecordingNotificationSink::new();
        let output = run_notify_case(&["--icon", "not-a-lucide-name", "hello"], Some(&sink));

        assert_eq!(output.exit, 0);
        assert_eq!(
            sink.recorded(),
            vec![
                "{\"tract\": \"notification\", \"event\": \"show\", \"message\": \"hello\", \"icon\": \"not-a-lucide-name\"}\n"
                    .to_string()
            ]
        );
    }

    #[test]
    fn no_sink_collapses_to_send_failure() {
        let output = run_notify_case(&["hello"], None);

        assert_eq!(
            output,
            CommandOutput {
                stdout: String::new(),
                stderr: FAILURE.to_string(),
                exit: 1,
            }
        );
    }

    #[test]
    fn failing_sink_collapses_to_send_failure() {
        let sink = RecordingNotificationSink::failing();
        let output = run_notify_case(&["hello"], Some(&sink));

        assert_eq!(
            output,
            CommandOutput {
                stdout: String::new(),
                stderr: FAILURE.to_string(),
                exit: 1,
            }
        );
        assert_eq!(
            sink.recorded(),
            vec![
                "{\"tract\": \"notification\", \"event\": \"show\", \"message\": \"hello\"}\n"
                    .to_string()
            ]
        );
    }

    #[test]
    fn malformed_args_follow_native_full_help_error_shape() {
        for (args, message) in [
            (vec!["--bogus", "hello"], "unrecognized arguments: --bogus"),
            (vec!["-f", "hello"], "unrecognized arguments: -f"),
            (vec!["-fwork", "hello"], "unrecognized arguments: -fwork"),
            (
                vec!["--auto-dismiss", "nope", "hello"],
                "argument --auto-dismiss: invalid int value: 'nope'",
            ),
        ] {
            let output = run_notify_case(&args, None);
            assert_eq!(output.stdout, "");
            assert_eq!(
                output.stderr,
                format!("{HELP}journal notify: error: {message}\n")
            );
            assert_eq!(output.exit, 2);
        }
    }

    #[test]
    fn bare_notify_requires_a_positional_token() {
        let output = run_notify_case(&[], None);

        assert_eq!(output.stdout, "");
        assert_eq!(
            output.stderr,
            format!("{HELP}journal notify: error: the following arguments are required: message\n")
        );
        assert_eq!(output.exit, 2);
    }
}
