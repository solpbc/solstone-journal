// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use fluent_bundle::{FluentArgs, FluentBundle, FluentResource, FluentValue};
use std::collections::HashSet;

#[derive(Clone, Debug, PartialEq)]
pub enum Value<'a> {
    String(&'a str),
    Boolean(bool),
    Integer(i64),
    Cardinal(i64),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RenderError {
    UnknownLocale,
    UnknownMessage,
    MissingArgument(String),
    UnknownArgument(String),
    DuplicateArgument(String),
    InvalidArgumentType(String),
    OutOfRange(String),
    FormatFailed,
}

#[allow(non_camel_case_types)]
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum MessageId {
    m6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e616374696f6e2e636c656172,
    m6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e616374696f6e2e636c6f7365,
    m6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e616374696f6e2e73656e64,
    m6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e616374696f6e2e73656e642e616c6c,
    m6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e616374696f6e2e73686f772e64657461696c73,
    m6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e64657461696c2e6c6f636174696f6e2e6c6162656c,
    m6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e64657461696c2e6d6573736167652e6c6162656c,
    m6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e64657461696c2e737461636b2e6c6162656c,
    m6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e656d707479,
    m6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e68656164696e67,
    m6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e6c696e6b2e6c6162656c,
    m6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e7265706f7274696e672e6f6666,
    m6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e736e617073686f742e68656164696e67,
    m6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e73756d6d6172792e6c6f61642e6661696c6564,
    m6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e73756d6d6172792e726571756573742e6661696c6564,
    m6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e73756d6d6172792e756e6578706563746564,
    m6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e7461622e616c6c,
    m6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e7461622e6572726f7273,
    m6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e7461622e696e666f,
    m6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e7461622e7761726e696e6773,
    m6a6f75726e616c2e77696e646f772e6368726f6d652e6461792e7a6f6e65732e616c6c,
    m6a6f75726e616c2e77696e646f772e6368726f6d652e6461792e7a6f6e65732e70617274,
    m6a6f75726e616c2e77696e646f772e6368726f6d652e6c6f672e726561642e6661696c6564,
    m6a6f75726e616c2e77696e646f772e6368726f6d652e72656c6f61642e68696e74,
    m6a6f75726e616c2e77696e646f772e6368726f6d652e7265706f72742e616374696f6e2e63616e63656c,
    m6a6f75726e616c2e77696e646f772e6368726f6d652e7265706f72742e616374696f6e2e73656e64,
    m6a6f75726e616c2e77696e646f772e6368726f6d652e7265706f72742e6170702e6c6162656c,
    m6a6f75726e616c2e77696e646f772e6368726f6d652e7265706f72742e627574746f6e2e6c6162656c,
    m6a6f75726e616c2e77696e646f772e6368726f6d652e7265706f72742e64656661756c742e7375626a656374,
    m6a6f75726e616c2e77696e646f772e6368726f6d652e7265706f72742e6572726f722e636f64652e6c6162656c,
    m6a6f75726e616c2e77696e646f772e6368726f6d652e7265706f72742e707269766163792e6e6f7465,
    m6a6f75726e616c2e77696e646f772e6368726f6d652e7265706f72742e726563656e742e6c6162656c,
    m6a6f75726e616c2e77696e646f772e6368726f6d652e7265706f72742e726f7574652e6c6162656c,
    m6a6f75726e616c2e77696e646f772e6368726f6d652e7265706f72742e7469746c65,
    m6a6f75726e616c2e77696e646f772e6368726f6d652e7265747279,
    m6a6f75726e616c2e77696e646f772e6368726f6d652e73656374696f6e2e6572726f72,
    m6a6f75726e616c2e77696e646f772e6368726f6d652e7374696c6c2e776f726b696e67,
    m6a6f75726e616c2e77696e646f772e6368726f6d652e756e6b6e6f776e2e6572726f72,
    m6a6f75726e616c2e77696e646f772e6368726f6d652e7a6f6e652e74696d65,
    m6a6f75726e616c2e77696e646f772e6e6f742e666f756e642e64657461696c,
    m6a6f75726e616c2e77696e646f772e6e6f742e666f756e642e68656164696e67,
    m6a6f75726e616c2e77696e646f772e6e6f742e666f756e642e7469746c65,
    m6a6f75726e616c2e77696e646f772e7368656c6c2e636f6e6e656374696e67,
    m6a6f75726e616c2e77696e646f772e7368656c6c2e6c6173742e6d657373616765,
    m6a6f75726e616c2e77696e646f772e7368656c6c2e6c6f6164696e67,
    m6a6f75726e616c2e77696e646f772e7368656c6c2e6e6f74696669636174696f6e732e656d707479,
    m6a6f75726e616c2e77696e646f772e7368656c6c2e6e6f74696669636174696f6e732e656e61626c65,
    m6a6f75726e616c2e77696e646f772e7368656c6c2e6e6f74696669636174696f6e732e6c6162656c,
    m6a6f75726e616c2e77696e646f772e7368656c6c2e6e6f74696669636174696f6e732e7175696574,
    m6a6f75726e616c2e77696e646f772e7368656c6c2e6e6f74696669636174696f6e732e71756965742e6578706c616e6174696f6e,
    m6a6f75726e616c2e77696e646f772e7368656c6c2e6e6f74696669636174696f6e732e726563656e74,
    m6a6f75726e616c2e77696e646f772e7368656c6c2e736b69702e746f2e776f726b7370616365,
    m6a6f75726e616c2e77696e646f772e7368656c6c2e7374617465,
    m6a6f75726e616c2e77696e646f772e7368656c6c2e737461747573,
    m6a6f75726e616c2e77696e646f772e7368656c6c2e7374617475732e64657461696c73,
    m6a6f75726e616c2e77696e646f772e7368656c6c2e746563686e6963616c2e64657461696c73,
    m6a6f75726e616c2e77696e646f772e7368656c6c2e7469746c65,
    m6a6f75726e616c2e77696e646f772e7368656c6c2e7570646174652e6c696e6b,
    m6a6f75726e616c2e77696e646f772e7368656c6c2e757074696d65,
    m6a6f75726e616c2e77696e646f772e7368656c6c2e76657273696f6e,
    m6a6f75726e616c2e77696e646f772e7368656c6c2e7669657773,
}

#[allow(dead_code, unused_parens)]
fn plural_category(family: Option<&str>, count: i64) -> &'static str {
    let n = count.unsigned_abs();
    let _ = n;
    match family {
        Some("en") => if n == 1 { "one" } else { "other" },
        _ => "other",
    }
}

pub fn render_dotted(locale: &str, dotted_id: &str, args: &[(&str, Value)]) -> Result<String, RenderError> { render_dotted_with_options(locale, dotted_id, args, true) }

pub fn render_dotted_with_options(locale: &str, dotted_id: &str, args: &[(&str, Value)], use_isolating: bool) -> Result<String, RenderError> {
    let id = match dotted_id {
        "journal.window.chrome.console.action.clear" => MessageId::m6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e616374696f6e2e636c656172,
        "journal.window.chrome.console.action.close" => MessageId::m6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e616374696f6e2e636c6f7365,
        "journal.window.chrome.console.action.send" => MessageId::m6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e616374696f6e2e73656e64,
        "journal.window.chrome.console.action.send.all" => MessageId::m6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e616374696f6e2e73656e642e616c6c,
        "journal.window.chrome.console.action.show.details" => MessageId::m6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e616374696f6e2e73686f772e64657461696c73,
        "journal.window.chrome.console.detail.location.label" => MessageId::m6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e64657461696c2e6c6f636174696f6e2e6c6162656c,
        "journal.window.chrome.console.detail.message.label" => MessageId::m6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e64657461696c2e6d6573736167652e6c6162656c,
        "journal.window.chrome.console.detail.stack.label" => MessageId::m6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e64657461696c2e737461636b2e6c6162656c,
        "journal.window.chrome.console.empty" => MessageId::m6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e656d707479,
        "journal.window.chrome.console.heading" => MessageId::m6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e68656164696e67,
        "journal.window.chrome.console.link.label" => MessageId::m6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e6c696e6b2e6c6162656c,
        "journal.window.chrome.console.reporting.off" => MessageId::m6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e7265706f7274696e672e6f6666,
        "journal.window.chrome.console.snapshot.heading" => MessageId::m6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e736e617073686f742e68656164696e67,
        "journal.window.chrome.console.summary.load.failed" => MessageId::m6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e73756d6d6172792e6c6f61642e6661696c6564,
        "journal.window.chrome.console.summary.request.failed" => MessageId::m6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e73756d6d6172792e726571756573742e6661696c6564,
        "journal.window.chrome.console.summary.unexpected" => MessageId::m6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e73756d6d6172792e756e6578706563746564,
        "journal.window.chrome.console.tab.all" => MessageId::m6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e7461622e616c6c,
        "journal.window.chrome.console.tab.errors" => MessageId::m6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e7461622e6572726f7273,
        "journal.window.chrome.console.tab.info" => MessageId::m6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e7461622e696e666f,
        "journal.window.chrome.console.tab.warnings" => MessageId::m6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e7461622e7761726e696e6773,
        "journal.window.chrome.day.zones.all" => MessageId::m6a6f75726e616c2e77696e646f772e6368726f6d652e6461792e7a6f6e65732e616c6c,
        "journal.window.chrome.day.zones.part" => MessageId::m6a6f75726e616c2e77696e646f772e6368726f6d652e6461792e7a6f6e65732e70617274,
        "journal.window.chrome.log.read.failed" => MessageId::m6a6f75726e616c2e77696e646f772e6368726f6d652e6c6f672e726561642e6661696c6564,
        "journal.window.chrome.reload.hint" => MessageId::m6a6f75726e616c2e77696e646f772e6368726f6d652e72656c6f61642e68696e74,
        "journal.window.chrome.report.action.cancel" => MessageId::m6a6f75726e616c2e77696e646f772e6368726f6d652e7265706f72742e616374696f6e2e63616e63656c,
        "journal.window.chrome.report.action.send" => MessageId::m6a6f75726e616c2e77696e646f772e6368726f6d652e7265706f72742e616374696f6e2e73656e64,
        "journal.window.chrome.report.app.label" => MessageId::m6a6f75726e616c2e77696e646f772e6368726f6d652e7265706f72742e6170702e6c6162656c,
        "journal.window.chrome.report.button.label" => MessageId::m6a6f75726e616c2e77696e646f772e6368726f6d652e7265706f72742e627574746f6e2e6c6162656c,
        "journal.window.chrome.report.default.subject" => MessageId::m6a6f75726e616c2e77696e646f772e6368726f6d652e7265706f72742e64656661756c742e7375626a656374,
        "journal.window.chrome.report.error.code.label" => MessageId::m6a6f75726e616c2e77696e646f772e6368726f6d652e7265706f72742e6572726f722e636f64652e6c6162656c,
        "journal.window.chrome.report.privacy.note" => MessageId::m6a6f75726e616c2e77696e646f772e6368726f6d652e7265706f72742e707269766163792e6e6f7465,
        "journal.window.chrome.report.recent.label" => MessageId::m6a6f75726e616c2e77696e646f772e6368726f6d652e7265706f72742e726563656e742e6c6162656c,
        "journal.window.chrome.report.route.label" => MessageId::m6a6f75726e616c2e77696e646f772e6368726f6d652e7265706f72742e726f7574652e6c6162656c,
        "journal.window.chrome.report.title" => MessageId::m6a6f75726e616c2e77696e646f772e6368726f6d652e7265706f72742e7469746c65,
        "journal.window.chrome.retry" => MessageId::m6a6f75726e616c2e77696e646f772e6368726f6d652e7265747279,
        "journal.window.chrome.section.error" => MessageId::m6a6f75726e616c2e77696e646f772e6368726f6d652e73656374696f6e2e6572726f72,
        "journal.window.chrome.still.working" => MessageId::m6a6f75726e616c2e77696e646f772e6368726f6d652e7374696c6c2e776f726b696e67,
        "journal.window.chrome.unknown.error" => MessageId::m6a6f75726e616c2e77696e646f772e6368726f6d652e756e6b6e6f776e2e6572726f72,
        "journal.window.chrome.zone.time" => MessageId::m6a6f75726e616c2e77696e646f772e6368726f6d652e7a6f6e652e74696d65,
        "journal.window.not.found.detail" => MessageId::m6a6f75726e616c2e77696e646f772e6e6f742e666f756e642e64657461696c,
        "journal.window.not.found.heading" => MessageId::m6a6f75726e616c2e77696e646f772e6e6f742e666f756e642e68656164696e67,
        "journal.window.not.found.title" => MessageId::m6a6f75726e616c2e77696e646f772e6e6f742e666f756e642e7469746c65,
        "journal.window.shell.connecting" => MessageId::m6a6f75726e616c2e77696e646f772e7368656c6c2e636f6e6e656374696e67,
        "journal.window.shell.last.message" => MessageId::m6a6f75726e616c2e77696e646f772e7368656c6c2e6c6173742e6d657373616765,
        "journal.window.shell.loading" => MessageId::m6a6f75726e616c2e77696e646f772e7368656c6c2e6c6f6164696e67,
        "journal.window.shell.notifications.empty" => MessageId::m6a6f75726e616c2e77696e646f772e7368656c6c2e6e6f74696669636174696f6e732e656d707479,
        "journal.window.shell.notifications.enable" => MessageId::m6a6f75726e616c2e77696e646f772e7368656c6c2e6e6f74696669636174696f6e732e656e61626c65,
        "journal.window.shell.notifications.label" => MessageId::m6a6f75726e616c2e77696e646f772e7368656c6c2e6e6f74696669636174696f6e732e6c6162656c,
        "journal.window.shell.notifications.quiet" => MessageId::m6a6f75726e616c2e77696e646f772e7368656c6c2e6e6f74696669636174696f6e732e7175696574,
        "journal.window.shell.notifications.quiet.explanation" => MessageId::m6a6f75726e616c2e77696e646f772e7368656c6c2e6e6f74696669636174696f6e732e71756965742e6578706c616e6174696f6e,
        "journal.window.shell.notifications.recent" => MessageId::m6a6f75726e616c2e77696e646f772e7368656c6c2e6e6f74696669636174696f6e732e726563656e74,
        "journal.window.shell.skip.to.workspace" => MessageId::m6a6f75726e616c2e77696e646f772e7368656c6c2e736b69702e746f2e776f726b7370616365,
        "journal.window.shell.state" => MessageId::m6a6f75726e616c2e77696e646f772e7368656c6c2e7374617465,
        "journal.window.shell.status" => MessageId::m6a6f75726e616c2e77696e646f772e7368656c6c2e737461747573,
        "journal.window.shell.status.details" => MessageId::m6a6f75726e616c2e77696e646f772e7368656c6c2e7374617475732e64657461696c73,
        "journal.window.shell.technical.details" => MessageId::m6a6f75726e616c2e77696e646f772e7368656c6c2e746563686e6963616c2e64657461696c73,
        "journal.window.shell.title" => MessageId::m6a6f75726e616c2e77696e646f772e7368656c6c2e7469746c65,
        "journal.window.shell.update.link" => MessageId::m6a6f75726e616c2e77696e646f772e7368656c6c2e7570646174652e6c696e6b,
        "journal.window.shell.uptime" => MessageId::m6a6f75726e616c2e77696e646f772e7368656c6c2e757074696d65,
        "journal.window.shell.version" => MessageId::m6a6f75726e616c2e77696e646f772e7368656c6c2e76657273696f6e,
        "journal.window.shell.views" => MessageId::m6a6f75726e616c2e77696e646f772e7368656c6c2e7669657773,
        _ => return Err(RenderError::UnknownMessage),
    };
    render_with_options(locale, id, args, use_isolating)
}

pub fn render(locale: &str, id: MessageId, args: &[(&str, Value)]) -> Result<String, RenderError> { render_with_options(locale, id, args, true) }

pub fn render_with_options(locale: &str, id: MessageId, args: &[(&str, Value)], use_isolating: bool) -> Result<String, RenderError> {
    let message_id = id;

    let mut seen = HashSet::new();
    for (name, _) in args {
        if !seen.insert(*name) {
            return Err(RenderError::DuplicateArgument((*name).to_string()));
        }
    }

    let (ftl_content, cardinal_rule) = match locale {
        "en-US" => (include_str!("en-US.ftl"), Some("en")),
        _ => return Err(RenderError::UnknownLocale),
    };

    let (res_key, fluent_args) = match message_id {
        MessageId::m6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e616374696f6e2e636c656172 => {
            const DECLARED: &[&str] = &[];
            for (name, _) in args {
                if !DECLARED.contains(name) {
                    return Err(RenderError::UnknownArgument((*name).to_string()));
                }
            }
            #[allow(unused_mut)]
            let mut f_args = FluentArgs::new();
            ("m6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e616374696f6e2e636c656172".to_string(), f_args)
        }
        MessageId::m6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e616374696f6e2e636c6f7365 => {
            const DECLARED: &[&str] = &[];
            for (name, _) in args {
                if !DECLARED.contains(name) {
                    return Err(RenderError::UnknownArgument((*name).to_string()));
                }
            }
            #[allow(unused_mut)]
            let mut f_args = FluentArgs::new();
            ("m6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e616374696f6e2e636c6f7365".to_string(), f_args)
        }
        MessageId::m6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e616374696f6e2e73656e64 => {
            const DECLARED: &[&str] = &[];
            for (name, _) in args {
                if !DECLARED.contains(name) {
                    return Err(RenderError::UnknownArgument((*name).to_string()));
                }
            }
            #[allow(unused_mut)]
            let mut f_args = FluentArgs::new();
            ("m6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e616374696f6e2e73656e64".to_string(), f_args)
        }
        MessageId::m6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e616374696f6e2e73656e642e616c6c => {
            const DECLARED: &[&str] = &[];
            for (name, _) in args {
                if !DECLARED.contains(name) {
                    return Err(RenderError::UnknownArgument((*name).to_string()));
                }
            }
            #[allow(unused_mut)]
            let mut f_args = FluentArgs::new();
            ("m6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e616374696f6e2e73656e642e616c6c".to_string(), f_args)
        }
        MessageId::m6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e616374696f6e2e73686f772e64657461696c73 => {
            const DECLARED: &[&str] = &[];
            for (name, _) in args {
                if !DECLARED.contains(name) {
                    return Err(RenderError::UnknownArgument((*name).to_string()));
                }
            }
            #[allow(unused_mut)]
            let mut f_args = FluentArgs::new();
            ("m6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e616374696f6e2e73686f772e64657461696c73".to_string(), f_args)
        }
        MessageId::m6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e64657461696c2e6c6f636174696f6e2e6c6162656c => {
            const DECLARED: &[&str] = &[];
            for (name, _) in args {
                if !DECLARED.contains(name) {
                    return Err(RenderError::UnknownArgument((*name).to_string()));
                }
            }
            #[allow(unused_mut)]
            let mut f_args = FluentArgs::new();
            ("m6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e64657461696c2e6c6f636174696f6e2e6c6162656c".to_string(), f_args)
        }
        MessageId::m6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e64657461696c2e6d6573736167652e6c6162656c => {
            const DECLARED: &[&str] = &[];
            for (name, _) in args {
                if !DECLARED.contains(name) {
                    return Err(RenderError::UnknownArgument((*name).to_string()));
                }
            }
            #[allow(unused_mut)]
            let mut f_args = FluentArgs::new();
            ("m6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e64657461696c2e6d6573736167652e6c6162656c".to_string(), f_args)
        }
        MessageId::m6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e64657461696c2e737461636b2e6c6162656c => {
            const DECLARED: &[&str] = &[];
            for (name, _) in args {
                if !DECLARED.contains(name) {
                    return Err(RenderError::UnknownArgument((*name).to_string()));
                }
            }
            #[allow(unused_mut)]
            let mut f_args = FluentArgs::new();
            ("m6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e64657461696c2e737461636b2e6c6162656c".to_string(), f_args)
        }
        MessageId::m6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e656d707479 => {
            const DECLARED: &[&str] = &[];
            for (name, _) in args {
                if !DECLARED.contains(name) {
                    return Err(RenderError::UnknownArgument((*name).to_string()));
                }
            }
            #[allow(unused_mut)]
            let mut f_args = FluentArgs::new();
            ("m6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e656d707479".to_string(), f_args)
        }
        MessageId::m6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e68656164696e67 => {
            const DECLARED: &[&str] = &[];
            for (name, _) in args {
                if !DECLARED.contains(name) {
                    return Err(RenderError::UnknownArgument((*name).to_string()));
                }
            }
            #[allow(unused_mut)]
            let mut f_args = FluentArgs::new();
            ("m6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e68656164696e67".to_string(), f_args)
        }
        MessageId::m6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e6c696e6b2e6c6162656c => {
            const DECLARED: &[&str] = &[];
            for (name, _) in args {
                if !DECLARED.contains(name) {
                    return Err(RenderError::UnknownArgument((*name).to_string()));
                }
            }
            #[allow(unused_mut)]
            let mut f_args = FluentArgs::new();
            ("m6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e6c696e6b2e6c6162656c".to_string(), f_args)
        }
        MessageId::m6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e7265706f7274696e672e6f6666 => {
            const DECLARED: &[&str] = &[];
            for (name, _) in args {
                if !DECLARED.contains(name) {
                    return Err(RenderError::UnknownArgument((*name).to_string()));
                }
            }
            #[allow(unused_mut)]
            let mut f_args = FluentArgs::new();
            ("m6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e7265706f7274696e672e6f6666".to_string(), f_args)
        }
        MessageId::m6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e736e617073686f742e68656164696e67 => {
            const DECLARED: &[&str] = &["count"];
            for (name, _) in args {
                if !DECLARED.contains(name) {
                    return Err(RenderError::UnknownArgument((*name).to_string()));
                }
            }
            #[allow(unused_mut)]
            let mut f_args = FluentArgs::new();
            let val_v0 = args.iter().find(|(n, _)| *n == "count").ok_or_else(|| RenderError::MissingArgument("count".to_string()))?.1.clone();
            let v0 = match val_v0 {
                Value::String(s) => s.to_string(),
                _ => return Err(RenderError::InvalidArgumentType("count".to_string())),
            };
            f_args.set("count", FluentValue::from(v0.clone()));
            ("m6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e736e617073686f742e68656164696e67".to_string(), f_args)
        }
        MessageId::m6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e73756d6d6172792e6c6f61642e6661696c6564 => {
            const DECLARED: &[&str] = &[];
            for (name, _) in args {
                if !DECLARED.contains(name) {
                    return Err(RenderError::UnknownArgument((*name).to_string()));
                }
            }
            #[allow(unused_mut)]
            let mut f_args = FluentArgs::new();
            ("m6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e73756d6d6172792e6c6f61642e6661696c6564".to_string(), f_args)
        }
        MessageId::m6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e73756d6d6172792e726571756573742e6661696c6564 => {
            const DECLARED: &[&str] = &[];
            for (name, _) in args {
                if !DECLARED.contains(name) {
                    return Err(RenderError::UnknownArgument((*name).to_string()));
                }
            }
            #[allow(unused_mut)]
            let mut f_args = FluentArgs::new();
            ("m6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e73756d6d6172792e726571756573742e6661696c6564".to_string(), f_args)
        }
        MessageId::m6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e73756d6d6172792e756e6578706563746564 => {
            const DECLARED: &[&str] = &[];
            for (name, _) in args {
                if !DECLARED.contains(name) {
                    return Err(RenderError::UnknownArgument((*name).to_string()));
                }
            }
            #[allow(unused_mut)]
            let mut f_args = FluentArgs::new();
            ("m6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e73756d6d6172792e756e6578706563746564".to_string(), f_args)
        }
        MessageId::m6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e7461622e616c6c => {
            const DECLARED: &[&str] = &[];
            for (name, _) in args {
                if !DECLARED.contains(name) {
                    return Err(RenderError::UnknownArgument((*name).to_string()));
                }
            }
            #[allow(unused_mut)]
            let mut f_args = FluentArgs::new();
            ("m6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e7461622e616c6c".to_string(), f_args)
        }
        MessageId::m6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e7461622e6572726f7273 => {
            const DECLARED: &[&str] = &[];
            for (name, _) in args {
                if !DECLARED.contains(name) {
                    return Err(RenderError::UnknownArgument((*name).to_string()));
                }
            }
            #[allow(unused_mut)]
            let mut f_args = FluentArgs::new();
            ("m6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e7461622e6572726f7273".to_string(), f_args)
        }
        MessageId::m6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e7461622e696e666f => {
            const DECLARED: &[&str] = &[];
            for (name, _) in args {
                if !DECLARED.contains(name) {
                    return Err(RenderError::UnknownArgument((*name).to_string()));
                }
            }
            #[allow(unused_mut)]
            let mut f_args = FluentArgs::new();
            ("m6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e7461622e696e666f".to_string(), f_args)
        }
        MessageId::m6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e7461622e7761726e696e6773 => {
            const DECLARED: &[&str] = &[];
            for (name, _) in args {
                if !DECLARED.contains(name) {
                    return Err(RenderError::UnknownArgument((*name).to_string()));
                }
            }
            #[allow(unused_mut)]
            let mut f_args = FluentArgs::new();
            ("m6a6f75726e616c2e77696e646f772e6368726f6d652e636f6e736f6c652e7461622e7761726e696e6773".to_string(), f_args)
        }
        MessageId::m6a6f75726e616c2e77696e646f772e6368726f6d652e6461792e7a6f6e65732e616c6c => {
            const DECLARED: &[&str] = &["zones"];
            for (name, _) in args {
                if !DECLARED.contains(name) {
                    return Err(RenderError::UnknownArgument((*name).to_string()));
                }
            }
            #[allow(unused_mut)]
            let mut f_args = FluentArgs::new();
            let val_v0 = args.iter().find(|(n, _)| *n == "zones").ok_or_else(|| RenderError::MissingArgument("zones".to_string()))?.1.clone();
            let v0 = match val_v0 {
                Value::String(s) => s.to_string(),
                _ => return Err(RenderError::InvalidArgumentType("zones".to_string())),
            };
            f_args.set("zones", FluentValue::from(v0.clone()));
            ("m6a6f75726e616c2e77696e646f772e6368726f6d652e6461792e7a6f6e65732e616c6c".to_string(), f_args)
        }
        MessageId::m6a6f75726e616c2e77696e646f772e6368726f6d652e6461792e7a6f6e65732e70617274 => {
            const DECLARED: &[&str] = &["zones"];
            for (name, _) in args {
                if !DECLARED.contains(name) {
                    return Err(RenderError::UnknownArgument((*name).to_string()));
                }
            }
            #[allow(unused_mut)]
            let mut f_args = FluentArgs::new();
            let val_v0 = args.iter().find(|(n, _)| *n == "zones").ok_or_else(|| RenderError::MissingArgument("zones".to_string()))?.1.clone();
            let v0 = match val_v0 {
                Value::String(s) => s.to_string(),
                _ => return Err(RenderError::InvalidArgumentType("zones".to_string())),
            };
            f_args.set("zones", FluentValue::from(v0.clone()));
            ("m6a6f75726e616c2e77696e646f772e6368726f6d652e6461792e7a6f6e65732e70617274".to_string(), f_args)
        }
        MessageId::m6a6f75726e616c2e77696e646f772e6368726f6d652e6c6f672e726561642e6661696c6564 => {
            const DECLARED: &[&str] = &[];
            for (name, _) in args {
                if !DECLARED.contains(name) {
                    return Err(RenderError::UnknownArgument((*name).to_string()));
                }
            }
            #[allow(unused_mut)]
            let mut f_args = FluentArgs::new();
            ("m6a6f75726e616c2e77696e646f772e6368726f6d652e6c6f672e726561642e6661696c6564".to_string(), f_args)
        }
        MessageId::m6a6f75726e616c2e77696e646f772e6368726f6d652e72656c6f61642e68696e74 => {
            const DECLARED: &[&str] = &[];
            for (name, _) in args {
                if !DECLARED.contains(name) {
                    return Err(RenderError::UnknownArgument((*name).to_string()));
                }
            }
            #[allow(unused_mut)]
            let mut f_args = FluentArgs::new();
            ("m6a6f75726e616c2e77696e646f772e6368726f6d652e72656c6f61642e68696e74".to_string(), f_args)
        }
        MessageId::m6a6f75726e616c2e77696e646f772e6368726f6d652e7265706f72742e616374696f6e2e63616e63656c => {
            const DECLARED: &[&str] = &[];
            for (name, _) in args {
                if !DECLARED.contains(name) {
                    return Err(RenderError::UnknownArgument((*name).to_string()));
                }
            }
            #[allow(unused_mut)]
            let mut f_args = FluentArgs::new();
            ("m6a6f75726e616c2e77696e646f772e6368726f6d652e7265706f72742e616374696f6e2e63616e63656c".to_string(), f_args)
        }
        MessageId::m6a6f75726e616c2e77696e646f772e6368726f6d652e7265706f72742e616374696f6e2e73656e64 => {
            const DECLARED: &[&str] = &[];
            for (name, _) in args {
                if !DECLARED.contains(name) {
                    return Err(RenderError::UnknownArgument((*name).to_string()));
                }
            }
            #[allow(unused_mut)]
            let mut f_args = FluentArgs::new();
            ("m6a6f75726e616c2e77696e646f772e6368726f6d652e7265706f72742e616374696f6e2e73656e64".to_string(), f_args)
        }
        MessageId::m6a6f75726e616c2e77696e646f772e6368726f6d652e7265706f72742e6170702e6c6162656c => {
            const DECLARED: &[&str] = &[];
            for (name, _) in args {
                if !DECLARED.contains(name) {
                    return Err(RenderError::UnknownArgument((*name).to_string()));
                }
            }
            #[allow(unused_mut)]
            let mut f_args = FluentArgs::new();
            ("m6a6f75726e616c2e77696e646f772e6368726f6d652e7265706f72742e6170702e6c6162656c".to_string(), f_args)
        }
        MessageId::m6a6f75726e616c2e77696e646f772e6368726f6d652e7265706f72742e627574746f6e2e6c6162656c => {
            const DECLARED: &[&str] = &[];
            for (name, _) in args {
                if !DECLARED.contains(name) {
                    return Err(RenderError::UnknownArgument((*name).to_string()));
                }
            }
            #[allow(unused_mut)]
            let mut f_args = FluentArgs::new();
            ("m6a6f75726e616c2e77696e646f772e6368726f6d652e7265706f72742e627574746f6e2e6c6162656c".to_string(), f_args)
        }
        MessageId::m6a6f75726e616c2e77696e646f772e6368726f6d652e7265706f72742e64656661756c742e7375626a656374 => {
            const DECLARED: &[&str] = &[];
            for (name, _) in args {
                if !DECLARED.contains(name) {
                    return Err(RenderError::UnknownArgument((*name).to_string()));
                }
            }
            #[allow(unused_mut)]
            let mut f_args = FluentArgs::new();
            ("m6a6f75726e616c2e77696e646f772e6368726f6d652e7265706f72742e64656661756c742e7375626a656374".to_string(), f_args)
        }
        MessageId::m6a6f75726e616c2e77696e646f772e6368726f6d652e7265706f72742e6572726f722e636f64652e6c6162656c => {
            const DECLARED: &[&str] = &[];
            for (name, _) in args {
                if !DECLARED.contains(name) {
                    return Err(RenderError::UnknownArgument((*name).to_string()));
                }
            }
            #[allow(unused_mut)]
            let mut f_args = FluentArgs::new();
            ("m6a6f75726e616c2e77696e646f772e6368726f6d652e7265706f72742e6572726f722e636f64652e6c6162656c".to_string(), f_args)
        }
        MessageId::m6a6f75726e616c2e77696e646f772e6368726f6d652e7265706f72742e707269766163792e6e6f7465 => {
            const DECLARED: &[&str] = &[];
            for (name, _) in args {
                if !DECLARED.contains(name) {
                    return Err(RenderError::UnknownArgument((*name).to_string()));
                }
            }
            #[allow(unused_mut)]
            let mut f_args = FluentArgs::new();
            ("m6a6f75726e616c2e77696e646f772e6368726f6d652e7265706f72742e707269766163792e6e6f7465".to_string(), f_args)
        }
        MessageId::m6a6f75726e616c2e77696e646f772e6368726f6d652e7265706f72742e726563656e742e6c6162656c => {
            const DECLARED: &[&str] = &[];
            for (name, _) in args {
                if !DECLARED.contains(name) {
                    return Err(RenderError::UnknownArgument((*name).to_string()));
                }
            }
            #[allow(unused_mut)]
            let mut f_args = FluentArgs::new();
            ("m6a6f75726e616c2e77696e646f772e6368726f6d652e7265706f72742e726563656e742e6c6162656c".to_string(), f_args)
        }
        MessageId::m6a6f75726e616c2e77696e646f772e6368726f6d652e7265706f72742e726f7574652e6c6162656c => {
            const DECLARED: &[&str] = &[];
            for (name, _) in args {
                if !DECLARED.contains(name) {
                    return Err(RenderError::UnknownArgument((*name).to_string()));
                }
            }
            #[allow(unused_mut)]
            let mut f_args = FluentArgs::new();
            ("m6a6f75726e616c2e77696e646f772e6368726f6d652e7265706f72742e726f7574652e6c6162656c".to_string(), f_args)
        }
        MessageId::m6a6f75726e616c2e77696e646f772e6368726f6d652e7265706f72742e7469746c65 => {
            const DECLARED: &[&str] = &[];
            for (name, _) in args {
                if !DECLARED.contains(name) {
                    return Err(RenderError::UnknownArgument((*name).to_string()));
                }
            }
            #[allow(unused_mut)]
            let mut f_args = FluentArgs::new();
            ("m6a6f75726e616c2e77696e646f772e6368726f6d652e7265706f72742e7469746c65".to_string(), f_args)
        }
        MessageId::m6a6f75726e616c2e77696e646f772e6368726f6d652e7265747279 => {
            const DECLARED: &[&str] = &[];
            for (name, _) in args {
                if !DECLARED.contains(name) {
                    return Err(RenderError::UnknownArgument((*name).to_string()));
                }
            }
            #[allow(unused_mut)]
            let mut f_args = FluentArgs::new();
            ("m6a6f75726e616c2e77696e646f772e6368726f6d652e7265747279".to_string(), f_args)
        }
        MessageId::m6a6f75726e616c2e77696e646f772e6368726f6d652e73656374696f6e2e6572726f72 => {
            const DECLARED: &[&str] = &[];
            for (name, _) in args {
                if !DECLARED.contains(name) {
                    return Err(RenderError::UnknownArgument((*name).to_string()));
                }
            }
            #[allow(unused_mut)]
            let mut f_args = FluentArgs::new();
            ("m6a6f75726e616c2e77696e646f772e6368726f6d652e73656374696f6e2e6572726f72".to_string(), f_args)
        }
        MessageId::m6a6f75726e616c2e77696e646f772e6368726f6d652e7374696c6c2e776f726b696e67 => {
            const DECLARED: &[&str] = &[];
            for (name, _) in args {
                if !DECLARED.contains(name) {
                    return Err(RenderError::UnknownArgument((*name).to_string()));
                }
            }
            #[allow(unused_mut)]
            let mut f_args = FluentArgs::new();
            ("m6a6f75726e616c2e77696e646f772e6368726f6d652e7374696c6c2e776f726b696e67".to_string(), f_args)
        }
        MessageId::m6a6f75726e616c2e77696e646f772e6368726f6d652e756e6b6e6f776e2e6572726f72 => {
            const DECLARED: &[&str] = &[];
            for (name, _) in args {
                if !DECLARED.contains(name) {
                    return Err(RenderError::UnknownArgument((*name).to_string()));
                }
            }
            #[allow(unused_mut)]
            let mut f_args = FluentArgs::new();
            ("m6a6f75726e616c2e77696e646f772e6368726f6d652e756e6b6e6f776e2e6572726f72".to_string(), f_args)
        }
        MessageId::m6a6f75726e616c2e77696e646f772e6368726f6d652e7a6f6e652e74696d65 => {
            const DECLARED: &[&str] = &["label"];
            for (name, _) in args {
                if !DECLARED.contains(name) {
                    return Err(RenderError::UnknownArgument((*name).to_string()));
                }
            }
            #[allow(unused_mut)]
            let mut f_args = FluentArgs::new();
            let val_v0 = args.iter().find(|(n, _)| *n == "label").ok_or_else(|| RenderError::MissingArgument("label".to_string()))?.1.clone();
            let v0 = match val_v0 {
                Value::String(s) => s.to_string(),
                _ => return Err(RenderError::InvalidArgumentType("label".to_string())),
            };
            f_args.set("label", FluentValue::from(v0.clone()));
            ("m6a6f75726e616c2e77696e646f772e6368726f6d652e7a6f6e652e74696d65".to_string(), f_args)
        }
        MessageId::m6a6f75726e616c2e77696e646f772e6e6f742e666f756e642e64657461696c => {
            const DECLARED: &[&str] = &[];
            for (name, _) in args {
                if !DECLARED.contains(name) {
                    return Err(RenderError::UnknownArgument((*name).to_string()));
                }
            }
            #[allow(unused_mut)]
            let mut f_args = FluentArgs::new();
            ("m6a6f75726e616c2e77696e646f772e6e6f742e666f756e642e64657461696c".to_string(), f_args)
        }
        MessageId::m6a6f75726e616c2e77696e646f772e6e6f742e666f756e642e68656164696e67 => {
            const DECLARED: &[&str] = &[];
            for (name, _) in args {
                if !DECLARED.contains(name) {
                    return Err(RenderError::UnknownArgument((*name).to_string()));
                }
            }
            #[allow(unused_mut)]
            let mut f_args = FluentArgs::new();
            ("m6a6f75726e616c2e77696e646f772e6e6f742e666f756e642e68656164696e67".to_string(), f_args)
        }
        MessageId::m6a6f75726e616c2e77696e646f772e6e6f742e666f756e642e7469746c65 => {
            const DECLARED: &[&str] = &[];
            for (name, _) in args {
                if !DECLARED.contains(name) {
                    return Err(RenderError::UnknownArgument((*name).to_string()));
                }
            }
            #[allow(unused_mut)]
            let mut f_args = FluentArgs::new();
            ("m6a6f75726e616c2e77696e646f772e6e6f742e666f756e642e7469746c65".to_string(), f_args)
        }
        MessageId::m6a6f75726e616c2e77696e646f772e7368656c6c2e636f6e6e656374696e67 => {
            const DECLARED: &[&str] = &[];
            for (name, _) in args {
                if !DECLARED.contains(name) {
                    return Err(RenderError::UnknownArgument((*name).to_string()));
                }
            }
            #[allow(unused_mut)]
            let mut f_args = FluentArgs::new();
            ("m6a6f75726e616c2e77696e646f772e7368656c6c2e636f6e6e656374696e67".to_string(), f_args)
        }
        MessageId::m6a6f75726e616c2e77696e646f772e7368656c6c2e6c6173742e6d657373616765 => {
            const DECLARED: &[&str] = &[];
            for (name, _) in args {
                if !DECLARED.contains(name) {
                    return Err(RenderError::UnknownArgument((*name).to_string()));
                }
            }
            #[allow(unused_mut)]
            let mut f_args = FluentArgs::new();
            ("m6a6f75726e616c2e77696e646f772e7368656c6c2e6c6173742e6d657373616765".to_string(), f_args)
        }
        MessageId::m6a6f75726e616c2e77696e646f772e7368656c6c2e6c6f6164696e67 => {
            const DECLARED: &[&str] = &[];
            for (name, _) in args {
                if !DECLARED.contains(name) {
                    return Err(RenderError::UnknownArgument((*name).to_string()));
                }
            }
            #[allow(unused_mut)]
            let mut f_args = FluentArgs::new();
            ("m6a6f75726e616c2e77696e646f772e7368656c6c2e6c6f6164696e67".to_string(), f_args)
        }
        MessageId::m6a6f75726e616c2e77696e646f772e7368656c6c2e6e6f74696669636174696f6e732e656d707479 => {
            const DECLARED: &[&str] = &[];
            for (name, _) in args {
                if !DECLARED.contains(name) {
                    return Err(RenderError::UnknownArgument((*name).to_string()));
                }
            }
            #[allow(unused_mut)]
            let mut f_args = FluentArgs::new();
            ("m6a6f75726e616c2e77696e646f772e7368656c6c2e6e6f74696669636174696f6e732e656d707479".to_string(), f_args)
        }
        MessageId::m6a6f75726e616c2e77696e646f772e7368656c6c2e6e6f74696669636174696f6e732e656e61626c65 => {
            const DECLARED: &[&str] = &[];
            for (name, _) in args {
                if !DECLARED.contains(name) {
                    return Err(RenderError::UnknownArgument((*name).to_string()));
                }
            }
            #[allow(unused_mut)]
            let mut f_args = FluentArgs::new();
            ("m6a6f75726e616c2e77696e646f772e7368656c6c2e6e6f74696669636174696f6e732e656e61626c65".to_string(), f_args)
        }
        MessageId::m6a6f75726e616c2e77696e646f772e7368656c6c2e6e6f74696669636174696f6e732e6c6162656c => {
            const DECLARED: &[&str] = &[];
            for (name, _) in args {
                if !DECLARED.contains(name) {
                    return Err(RenderError::UnknownArgument((*name).to_string()));
                }
            }
            #[allow(unused_mut)]
            let mut f_args = FluentArgs::new();
            ("m6a6f75726e616c2e77696e646f772e7368656c6c2e6e6f74696669636174696f6e732e6c6162656c".to_string(), f_args)
        }
        MessageId::m6a6f75726e616c2e77696e646f772e7368656c6c2e6e6f74696669636174696f6e732e7175696574 => {
            const DECLARED: &[&str] = &[];
            for (name, _) in args {
                if !DECLARED.contains(name) {
                    return Err(RenderError::UnknownArgument((*name).to_string()));
                }
            }
            #[allow(unused_mut)]
            let mut f_args = FluentArgs::new();
            ("m6a6f75726e616c2e77696e646f772e7368656c6c2e6e6f74696669636174696f6e732e7175696574".to_string(), f_args)
        }
        MessageId::m6a6f75726e616c2e77696e646f772e7368656c6c2e6e6f74696669636174696f6e732e71756965742e6578706c616e6174696f6e => {
            const DECLARED: &[&str] = &[];
            for (name, _) in args {
                if !DECLARED.contains(name) {
                    return Err(RenderError::UnknownArgument((*name).to_string()));
                }
            }
            #[allow(unused_mut)]
            let mut f_args = FluentArgs::new();
            ("m6a6f75726e616c2e77696e646f772e7368656c6c2e6e6f74696669636174696f6e732e71756965742e6578706c616e6174696f6e".to_string(), f_args)
        }
        MessageId::m6a6f75726e616c2e77696e646f772e7368656c6c2e6e6f74696669636174696f6e732e726563656e74 => {
            const DECLARED: &[&str] = &[];
            for (name, _) in args {
                if !DECLARED.contains(name) {
                    return Err(RenderError::UnknownArgument((*name).to_string()));
                }
            }
            #[allow(unused_mut)]
            let mut f_args = FluentArgs::new();
            ("m6a6f75726e616c2e77696e646f772e7368656c6c2e6e6f74696669636174696f6e732e726563656e74".to_string(), f_args)
        }
        MessageId::m6a6f75726e616c2e77696e646f772e7368656c6c2e736b69702e746f2e776f726b7370616365 => {
            const DECLARED: &[&str] = &[];
            for (name, _) in args {
                if !DECLARED.contains(name) {
                    return Err(RenderError::UnknownArgument((*name).to_string()));
                }
            }
            #[allow(unused_mut)]
            let mut f_args = FluentArgs::new();
            ("m6a6f75726e616c2e77696e646f772e7368656c6c2e736b69702e746f2e776f726b7370616365".to_string(), f_args)
        }
        MessageId::m6a6f75726e616c2e77696e646f772e7368656c6c2e7374617465 => {
            const DECLARED: &[&str] = &[];
            for (name, _) in args {
                if !DECLARED.contains(name) {
                    return Err(RenderError::UnknownArgument((*name).to_string()));
                }
            }
            #[allow(unused_mut)]
            let mut f_args = FluentArgs::new();
            ("m6a6f75726e616c2e77696e646f772e7368656c6c2e7374617465".to_string(), f_args)
        }
        MessageId::m6a6f75726e616c2e77696e646f772e7368656c6c2e737461747573 => {
            const DECLARED: &[&str] = &[];
            for (name, _) in args {
                if !DECLARED.contains(name) {
                    return Err(RenderError::UnknownArgument((*name).to_string()));
                }
            }
            #[allow(unused_mut)]
            let mut f_args = FluentArgs::new();
            ("m6a6f75726e616c2e77696e646f772e7368656c6c2e737461747573".to_string(), f_args)
        }
        MessageId::m6a6f75726e616c2e77696e646f772e7368656c6c2e7374617475732e64657461696c73 => {
            const DECLARED: &[&str] = &[];
            for (name, _) in args {
                if !DECLARED.contains(name) {
                    return Err(RenderError::UnknownArgument((*name).to_string()));
                }
            }
            #[allow(unused_mut)]
            let mut f_args = FluentArgs::new();
            ("m6a6f75726e616c2e77696e646f772e7368656c6c2e7374617475732e64657461696c73".to_string(), f_args)
        }
        MessageId::m6a6f75726e616c2e77696e646f772e7368656c6c2e746563686e6963616c2e64657461696c73 => {
            const DECLARED: &[&str] = &[];
            for (name, _) in args {
                if !DECLARED.contains(name) {
                    return Err(RenderError::UnknownArgument((*name).to_string()));
                }
            }
            #[allow(unused_mut)]
            let mut f_args = FluentArgs::new();
            ("m6a6f75726e616c2e77696e646f772e7368656c6c2e746563686e6963616c2e64657461696c73".to_string(), f_args)
        }
        MessageId::m6a6f75726e616c2e77696e646f772e7368656c6c2e7469746c65 => {
            const DECLARED: &[&str] = &[];
            for (name, _) in args {
                if !DECLARED.contains(name) {
                    return Err(RenderError::UnknownArgument((*name).to_string()));
                }
            }
            #[allow(unused_mut)]
            let mut f_args = FluentArgs::new();
            ("m6a6f75726e616c2e77696e646f772e7368656c6c2e7469746c65".to_string(), f_args)
        }
        MessageId::m6a6f75726e616c2e77696e646f772e7368656c6c2e7570646174652e6c696e6b => {
            const DECLARED: &[&str] = &[];
            for (name, _) in args {
                if !DECLARED.contains(name) {
                    return Err(RenderError::UnknownArgument((*name).to_string()));
                }
            }
            #[allow(unused_mut)]
            let mut f_args = FluentArgs::new();
            ("m6a6f75726e616c2e77696e646f772e7368656c6c2e7570646174652e6c696e6b".to_string(), f_args)
        }
        MessageId::m6a6f75726e616c2e77696e646f772e7368656c6c2e757074696d65 => {
            const DECLARED: &[&str] = &[];
            for (name, _) in args {
                if !DECLARED.contains(name) {
                    return Err(RenderError::UnknownArgument((*name).to_string()));
                }
            }
            #[allow(unused_mut)]
            let mut f_args = FluentArgs::new();
            ("m6a6f75726e616c2e77696e646f772e7368656c6c2e757074696d65".to_string(), f_args)
        }
        MessageId::m6a6f75726e616c2e77696e646f772e7368656c6c2e76657273696f6e => {
            const DECLARED: &[&str] = &[];
            for (name, _) in args {
                if !DECLARED.contains(name) {
                    return Err(RenderError::UnknownArgument((*name).to_string()));
                }
            }
            #[allow(unused_mut)]
            let mut f_args = FluentArgs::new();
            ("m6a6f75726e616c2e77696e646f772e7368656c6c2e76657273696f6e".to_string(), f_args)
        }
        MessageId::m6a6f75726e616c2e77696e646f772e7368656c6c2e7669657773 => {
            const DECLARED: &[&str] = &[];
            for (name, _) in args {
                if !DECLARED.contains(name) {
                    return Err(RenderError::UnknownArgument((*name).to_string()));
                }
            }
            #[allow(unused_mut)]
            let mut f_args = FluentArgs::new();
            ("m6a6f75726e616c2e77696e646f772e7368656c6c2e7669657773".to_string(), f_args)
        }
    };

    let _ = cardinal_rule;
    let lang = "und".parse().map_err(|_| RenderError::FormatFailed)?;
    let mut bundle = FluentBundle::new(vec![lang]);
    bundle.set_use_isolating(use_isolating);
    let _ = bundle.add_builtins();
    let res = FluentResource::try_new(ftl_content.to_string()).map_err(|_| RenderError::FormatFailed)?;
    bundle.add_resource(res).map_err(|_| RenderError::FormatFailed)?;

    let msg = bundle.get_message(&res_key).ok_or(RenderError::FormatFailed)?;
    let pattern = msg.value().ok_or(RenderError::FormatFailed)?;
    let mut errs = vec![];
    let formatted = bundle.format_pattern(pattern, Some(&fluent_args), &mut errs);
    if !errs.is_empty() {
        return Err(RenderError::FormatFailed);
    }
    Ok(formatted.into_owned())
}
