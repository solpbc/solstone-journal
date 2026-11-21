// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use fluent_bundle::{FluentArgs, FluentResource, FluentValue};
use fluent_bundle::concurrent::FluentBundle;
use std::{collections::HashSet, sync::OnceLock};

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
    m6a6f75726e616c2e636c692e73747265616d732e617267756d656e7473,
    m6a6f75726e616c2e636c692e73747265616d732e656d707479,
    m6a6f75726e616c2e636c692e73747265616d732e6865616465722e646179,
    m6a6f75726e616c2e636c692e73747265616d732e6865616465722e6e616d65,
    m6a6f75726e616c2e636c692e73747265616d732e6865616465722e7365676d656e74,
    m6a6f75726e616c2e636c692e73747265616d732e6865616465722e73657175656e6365,
    m6a6f75726e616c2e636c692e73747265616d732e6865616465722e74797065,
    m6a6f75726e616c2e636c692e73747265616d732e68656c70,
    m6a6f75726e616c2e636c692e73747265616d732e696e73706563742e6d616c666f726d6564,
    m6a6f75726e616c2e636c692e73747265616d732e696e73706563742e6e6f742e666f756e64,
    m6a6f75726e616c2e636c692e73747265616d732e6c6973742e6572726f72,
    m6a6f75726e616c2e636c692e73747265616d732e6d61726b65722e73657175656e6365,
    m6a6f75726e616c2e636c692e73747265616d732e6d61726b65722e756e7265616461626c65,
    m6a6f75726e616c2e636c692e73747265616d732e72656275696c642e656d707479,
    m6a6f75726e616c2e636c692e73747265616d732e72656275696c642e726573756c74,
    m6a6f75726e616c2e636c692e73747265616d732e7265636f72642e6d616c666f726d6564,
    m6a6f75726e616c2e636c692e73747265616d732e7265706169722e6168656164,
    m6a6f75726e616c2e636c692e73747265616d732e7265706169722e6c6f636b6564,
    m6a6f75726e616c2e636c692e73747265616d732e7265706169722e6d616c666f726d6564,
    m6a6f75726e616c2e636c692e73747265616d732e7265706169722e6d697373696e67,
    m6a6f75726e616c2e636c692e73747265616d732e7265706169722e77726974652e6661696c6564,
    m6a6f75726e616c2e636c692e73747265616d732e7365676d656e74732e6572726f72,
    m6a6f75726e616c2e636c692e73747265616d732e73757065727669736f72,
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
    static IDS: &[(&str, MessageId)] = &[
        ("journal.cli.streams.arguments", MessageId::m6a6f75726e616c2e636c692e73747265616d732e617267756d656e7473),
        ("journal.cli.streams.empty", MessageId::m6a6f75726e616c2e636c692e73747265616d732e656d707479),
        ("journal.cli.streams.header.day", MessageId::m6a6f75726e616c2e636c692e73747265616d732e6865616465722e646179),
        ("journal.cli.streams.header.name", MessageId::m6a6f75726e616c2e636c692e73747265616d732e6865616465722e6e616d65),
        ("journal.cli.streams.header.segment", MessageId::m6a6f75726e616c2e636c692e73747265616d732e6865616465722e7365676d656e74),
        ("journal.cli.streams.header.sequence", MessageId::m6a6f75726e616c2e636c692e73747265616d732e6865616465722e73657175656e6365),
        ("journal.cli.streams.header.type", MessageId::m6a6f75726e616c2e636c692e73747265616d732e6865616465722e74797065),
        ("journal.cli.streams.help", MessageId::m6a6f75726e616c2e636c692e73747265616d732e68656c70),
        ("journal.cli.streams.inspect.malformed", MessageId::m6a6f75726e616c2e636c692e73747265616d732e696e73706563742e6d616c666f726d6564),
        ("journal.cli.streams.inspect.not.found", MessageId::m6a6f75726e616c2e636c692e73747265616d732e696e73706563742e6e6f742e666f756e64),
        ("journal.cli.streams.list.error", MessageId::m6a6f75726e616c2e636c692e73747265616d732e6c6973742e6572726f72),
        ("journal.cli.streams.marker.sequence", MessageId::m6a6f75726e616c2e636c692e73747265616d732e6d61726b65722e73657175656e6365),
        ("journal.cli.streams.marker.unreadable", MessageId::m6a6f75726e616c2e636c692e73747265616d732e6d61726b65722e756e7265616461626c65),
        ("journal.cli.streams.rebuild.empty", MessageId::m6a6f75726e616c2e636c692e73747265616d732e72656275696c642e656d707479),
        ("journal.cli.streams.rebuild.result", MessageId::m6a6f75726e616c2e636c692e73747265616d732e72656275696c642e726573756c74),
        ("journal.cli.streams.record.malformed", MessageId::m6a6f75726e616c2e636c692e73747265616d732e7265636f72642e6d616c666f726d6564),
        ("journal.cli.streams.repair.ahead", MessageId::m6a6f75726e616c2e636c692e73747265616d732e7265706169722e6168656164),
        ("journal.cli.streams.repair.locked", MessageId::m6a6f75726e616c2e636c692e73747265616d732e7265706169722e6c6f636b6564),
        ("journal.cli.streams.repair.malformed", MessageId::m6a6f75726e616c2e636c692e73747265616d732e7265706169722e6d616c666f726d6564),
        ("journal.cli.streams.repair.missing", MessageId::m6a6f75726e616c2e636c692e73747265616d732e7265706169722e6d697373696e67),
        ("journal.cli.streams.repair.write.failed", MessageId::m6a6f75726e616c2e636c692e73747265616d732e7265706169722e77726974652e6661696c6564),
        ("journal.cli.streams.segments.error", MessageId::m6a6f75726e616c2e636c692e73747265616d732e7365676d656e74732e6572726f72),
        ("journal.cli.streams.supervisor", MessageId::m6a6f75726e616c2e636c692e73747265616d732e73757065727669736f72),
    ];
    let index = IDS.binary_search_by_key(&dotted_id, |(name, _)| *name).map_err(|_| RenderError::UnknownMessage)?;
    let id = IDS[index].1;
    render_with_options(locale, id, args, use_isolating)
}

pub fn render(locale: &str, id: MessageId, args: &[(&str, Value)]) -> Result<String, RenderError> { render_with_options(locale, id, args, true) }

pub fn render_with_options(locale: &str, id: MessageId, args: &[(&str, Value)], use_isolating: bool) -> Result<String, RenderError> {
    let mut seen = HashSet::new();
    for (name, _) in args {
        if !seen.insert(*name) {
            return Err(RenderError::DuplicateArgument((*name).to_string()));
        }
    }

    let (bundle, cardinal_rule) = match (locale, use_isolating) {
        ("en-US", false) => {
            static BUNDLE: OnceLock<Result<FluentBundle<FluentResource>, RenderError>> = OnceLock::new();
            (BUNDLE.get_or_init(|| load_bundle(include_str!("en-US.ftl"), false)), Some("en"))
        }
        ("en-US", true) => {
            static BUNDLE: OnceLock<Result<FluentBundle<FluentResource>, RenderError>> = OnceLock::new();
            (BUNDLE.get_or_init(|| load_bundle(include_str!("en-US.ftl"), true)), Some("en"))
        }
        _ => return Err(RenderError::UnknownLocale),
    };

    let (key, prepare) = MESSAGES[id as usize];
    let (res_key, fluent_args) = prepare(key, args, cardinal_rule)?;
    let bundle = bundle.as_ref().map_err(Clone::clone)?;
    let msg = bundle.get_message(res_key).ok_or(RenderError::FormatFailed)?;
    let pattern = msg.value().ok_or(RenderError::FormatFailed)?;
    let mut errs = vec![];
    let formatted = bundle.format_pattern(pattern, Some(&fluent_args), &mut errs);
    if !errs.is_empty() {
        return Err(RenderError::FormatFailed);
    }
    Ok(formatted.into_owned())
}

type Prepared = Result<(&'static str, FluentArgs<'static>), RenderError>;
type Prepare = fn(&'static str, &[(&str, Value)], Option<&str>) -> Prepared;
static MESSAGES: &[(&str, Prepare)] = &[
    ("m6a6f75726e616c2e636c692e73747265616d732e617267756d656e7473", prepare_m6a6f75726e616c2e636c692e73747265616d732e617267756d656e7473),
    ("m6a6f75726e616c2e636c692e73747265616d732e656d707479", no_arguments),
    ("m6a6f75726e616c2e636c692e73747265616d732e6865616465722e646179", no_arguments),
    ("m6a6f75726e616c2e636c692e73747265616d732e6865616465722e6e616d65", no_arguments),
    ("m6a6f75726e616c2e636c692e73747265616d732e6865616465722e7365676d656e74", no_arguments),
    ("m6a6f75726e616c2e636c692e73747265616d732e6865616465722e73657175656e6365", no_arguments),
    ("m6a6f75726e616c2e636c692e73747265616d732e6865616465722e74797065", no_arguments),
    ("m6a6f75726e616c2e636c692e73747265616d732e68656c70", no_arguments),
    ("m6a6f75726e616c2e636c692e73747265616d732e696e73706563742e6d616c666f726d6564", prepare_m6a6f75726e616c2e636c692e73747265616d732e696e73706563742e6d616c666f726d6564),
    ("m6a6f75726e616c2e636c692e73747265616d732e696e73706563742e6e6f742e666f756e64", prepare_m6a6f75726e616c2e636c692e73747265616d732e696e73706563742e6e6f742e666f756e64),
    ("m6a6f75726e616c2e636c692e73747265616d732e6c6973742e6572726f72", prepare_m6a6f75726e616c2e636c692e73747265616d732e6c6973742e6572726f72),
    ("m6a6f75726e616c2e636c692e73747265616d732e6d61726b65722e73657175656e6365", prepare_m6a6f75726e616c2e636c692e73747265616d732e6d61726b65722e73657175656e6365),
    ("m6a6f75726e616c2e636c692e73747265616d732e6d61726b65722e756e7265616461626c65", prepare_m6a6f75726e616c2e636c692e73747265616d732e6d61726b65722e756e7265616461626c65),
    ("m6a6f75726e616c2e636c692e73747265616d732e72656275696c642e656d707479", prepare_m6a6f75726e616c2e636c692e73747265616d732e72656275696c642e656d707479),
    ("m6a6f75726e616c2e636c692e73747265616d732e72656275696c642e726573756c74", prepare_m6a6f75726e616c2e636c692e73747265616d732e72656275696c642e726573756c74),
    ("m6a6f75726e616c2e636c692e73747265616d732e7265636f72642e6d616c666f726d6564", prepare_m6a6f75726e616c2e636c692e73747265616d732e7265636f72642e6d616c666f726d6564),
    ("m6a6f75726e616c2e636c692e73747265616d732e7265706169722e6168656164", prepare_m6a6f75726e616c2e636c692e73747265616d732e7265706169722e6168656164),
    ("m6a6f75726e616c2e636c692e73747265616d732e7265706169722e6c6f636b6564", prepare_m6a6f75726e616c2e636c692e73747265616d732e7265706169722e6c6f636b6564),
    ("m6a6f75726e616c2e636c692e73747265616d732e7265706169722e6d616c666f726d6564", prepare_m6a6f75726e616c2e636c692e73747265616d732e7265706169722e6d616c666f726d6564),
    ("m6a6f75726e616c2e636c692e73747265616d732e7265706169722e6d697373696e67", prepare_m6a6f75726e616c2e636c692e73747265616d732e7265706169722e6d697373696e67),
    ("m6a6f75726e616c2e636c692e73747265616d732e7265706169722e77726974652e6661696c6564", prepare_m6a6f75726e616c2e636c692e73747265616d732e7265706169722e77726974652e6661696c6564),
    ("m6a6f75726e616c2e636c692e73747265616d732e7365676d656e74732e6572726f72", prepare_m6a6f75726e616c2e636c692e73747265616d732e7365676d656e74732e6572726f72),
    ("m6a6f75726e616c2e636c692e73747265616d732e73757065727669736f72", no_arguments),
];

fn no_arguments(key: &'static str, args: &[(&str, Value)], _: Option<&str>) -> Prepared {
    if let Some((name, _)) = args.first() { return Err(RenderError::UnknownArgument((*name).to_owned())); }
    Ok((key, FluentArgs::new()))
}

fn prepare_m6a6f75726e616c2e636c692e73747265616d732e617267756d656e7473(_resource_key: &'static str, args: &[(&str, Value)], _cardinal_rule: Option<&str>) -> Prepared {
    const DECLARED: &[&str] = &["arguments"];
    for (name, _) in args {
        if !DECLARED.contains(name) {
            return Err(RenderError::UnknownArgument((*name).to_string()));
        }
    }
    #[allow(unused_mut)]
    let mut f_args = FluentArgs::new();
    let val_v0 = args.iter().find(|(n, _)| *n == "arguments").ok_or_else(|| RenderError::MissingArgument("arguments".to_string()))?.1.clone();
    let v0 = match val_v0 {
        Value::String(s) => s.to_string(),
        _ => return Err(RenderError::InvalidArgumentType("arguments".to_string())),
    };
    f_args.set("arguments", FluentValue::from(v0.clone()));
    Ok((_resource_key, f_args))
}

fn prepare_m6a6f75726e616c2e636c692e73747265616d732e696e73706563742e6d616c666f726d6564(_resource_key: &'static str, args: &[(&str, Value)], _cardinal_rule: Option<&str>) -> Prepared {
    const DECLARED: &[&str] = &["path"];
    for (name, _) in args {
        if !DECLARED.contains(name) {
            return Err(RenderError::UnknownArgument((*name).to_string()));
        }
    }
    #[allow(unused_mut)]
    let mut f_args = FluentArgs::new();
    let val_v0 = args.iter().find(|(n, _)| *n == "path").ok_or_else(|| RenderError::MissingArgument("path".to_string()))?.1.clone();
    let v0 = match val_v0 {
        Value::String(s) => s.to_string(),
        _ => return Err(RenderError::InvalidArgumentType("path".to_string())),
    };
    f_args.set("path", FluentValue::from(v0.clone()));
    Ok((_resource_key, f_args))
}

fn prepare_m6a6f75726e616c2e636c692e73747265616d732e696e73706563742e6e6f742e666f756e64(_resource_key: &'static str, args: &[(&str, Value)], _cardinal_rule: Option<&str>) -> Prepared {
    const DECLARED: &[&str] = &["name"];
    for (name, _) in args {
        if !DECLARED.contains(name) {
            return Err(RenderError::UnknownArgument((*name).to_string()));
        }
    }
    #[allow(unused_mut)]
    let mut f_args = FluentArgs::new();
    let val_v0 = args.iter().find(|(n, _)| *n == "name").ok_or_else(|| RenderError::MissingArgument("name".to_string()))?.1.clone();
    let v0 = match val_v0 {
        Value::String(s) => s.to_string(),
        _ => return Err(RenderError::InvalidArgumentType("name".to_string())),
    };
    f_args.set("name", FluentValue::from(v0.clone()));
    Ok((_resource_key, f_args))
}

fn prepare_m6a6f75726e616c2e636c692e73747265616d732e6c6973742e6572726f72(_resource_key: &'static str, args: &[(&str, Value)], _cardinal_rule: Option<&str>) -> Prepared {
    const DECLARED: &[&str] = &["error"];
    for (name, _) in args {
        if !DECLARED.contains(name) {
            return Err(RenderError::UnknownArgument((*name).to_string()));
        }
    }
    #[allow(unused_mut)]
    let mut f_args = FluentArgs::new();
    let val_v0 = args.iter().find(|(n, _)| *n == "error").ok_or_else(|| RenderError::MissingArgument("error".to_string()))?.1.clone();
    let v0 = match val_v0 {
        Value::String(s) => s.to_string(),
        _ => return Err(RenderError::InvalidArgumentType("error".to_string())),
    };
    f_args.set("error", FluentValue::from(v0.clone()));
    Ok((_resource_key, f_args))
}

fn prepare_m6a6f75726e616c2e636c692e73747265616d732e6d61726b65722e73657175656e6365(_resource_key: &'static str, args: &[(&str, Value)], _cardinal_rule: Option<&str>) -> Prepared {
    const DECLARED: &[&str] = &["path"];
    for (name, _) in args {
        if !DECLARED.contains(name) {
            return Err(RenderError::UnknownArgument((*name).to_string()));
        }
    }
    #[allow(unused_mut)]
    let mut f_args = FluentArgs::new();
    let val_v0 = args.iter().find(|(n, _)| *n == "path").ok_or_else(|| RenderError::MissingArgument("path".to_string()))?.1.clone();
    let v0 = match val_v0 {
        Value::String(s) => s.to_string(),
        _ => return Err(RenderError::InvalidArgumentType("path".to_string())),
    };
    f_args.set("path", FluentValue::from(v0.clone()));
    Ok((_resource_key, f_args))
}

fn prepare_m6a6f75726e616c2e636c692e73747265616d732e6d61726b65722e756e7265616461626c65(_resource_key: &'static str, args: &[(&str, Value)], _cardinal_rule: Option<&str>) -> Prepared {
    const DECLARED: &[&str] = &["path"];
    for (name, _) in args {
        if !DECLARED.contains(name) {
            return Err(RenderError::UnknownArgument((*name).to_string()));
        }
    }
    #[allow(unused_mut)]
    let mut f_args = FluentArgs::new();
    let val_v0 = args.iter().find(|(n, _)| *n == "path").ok_or_else(|| RenderError::MissingArgument("path".to_string()))?.1.clone();
    let v0 = match val_v0 {
        Value::String(s) => s.to_string(),
        _ => return Err(RenderError::InvalidArgumentType("path".to_string())),
    };
    f_args.set("path", FluentValue::from(v0.clone()));
    Ok((_resource_key, f_args))
}

fn prepare_m6a6f75726e616c2e636c692e73747265616d732e72656275696c642e656d707479(_resource_key: &'static str, args: &[(&str, Value)], _cardinal_rule: Option<&str>) -> Prepared {
    const DECLARED: &[&str] = &["segment_quantity", "segments"];
    for (name, _) in args {
        if !DECLARED.contains(name) {
            return Err(RenderError::UnknownArgument((*name).to_string()));
        }
    }
    #[allow(unused_mut)]
    let mut f_args = FluentArgs::new();
    let val_v0 = args.iter().find(|(n, _)| *n == "segment_quantity").ok_or_else(|| RenderError::MissingArgument("segment_quantity".to_string()))?.1.clone();
    let v0 = match val_v0 {
        Value::Cardinal(c) => c,
        _ => return Err(RenderError::InvalidArgumentType("segment_quantity".to_string())),
    };
    if v0 < -2147483648_i64 || v0 > 2147483647_i64 { return Err(RenderError::OutOfRange("segment_quantity".to_string())); }
    f_args.set("segment_quantity", FluentValue::from(v0));
    let val_v1 = args.iter().find(|(n, _)| *n == "segments").ok_or_else(|| RenderError::MissingArgument("segments".to_string()))?.1.clone();
    let v1 = match val_v1 {
        Value::String(s) => s.to_string(),
        _ => return Err(RenderError::InvalidArgumentType("segments".to_string())),
    };
    f_args.set("segments", FluentValue::from(v1.clone()));
    f_args.set("i18nCategory", plural_category(_cardinal_rule, v0));
    Ok((_resource_key, f_args))
}

fn prepare_m6a6f75726e616c2e636c692e73747265616d732e72656275696c642e726573756c74(_resource_key: &'static str, args: &[(&str, Value)], _cardinal_rule: Option<&str>) -> Prepared {
    const DECLARED: &[&str] = &["stream_quantity", "segment_quantity", "streams", "segments"];
    for (name, _) in args {
        if !DECLARED.contains(name) {
            return Err(RenderError::UnknownArgument((*name).to_string()));
        }
    }
    #[allow(unused_mut)]
    let mut f_args = FluentArgs::new();
    let val_v0 = args.iter().find(|(n, _)| *n == "stream_quantity").ok_or_else(|| RenderError::MissingArgument("stream_quantity".to_string()))?.1.clone();
    let v0 = match val_v0 {
        Value::Cardinal(c) => c,
        _ => return Err(RenderError::InvalidArgumentType("stream_quantity".to_string())),
    };
    if v0 < -2147483648_i64 || v0 > 2147483647_i64 { return Err(RenderError::OutOfRange("stream_quantity".to_string())); }
    f_args.set("stream_quantity", FluentValue::from(v0));
    let val_v1 = args.iter().find(|(n, _)| *n == "segment_quantity").ok_or_else(|| RenderError::MissingArgument("segment_quantity".to_string()))?.1.clone();
    let v1 = match val_v1 {
        Value::Cardinal(c) => c,
        _ => return Err(RenderError::InvalidArgumentType("segment_quantity".to_string())),
    };
    if v1 < -2147483648_i64 || v1 > 2147483647_i64 { return Err(RenderError::OutOfRange("segment_quantity".to_string())); }
    f_args.set("segment_quantity", FluentValue::from(v1));
    let val_v2 = args.iter().find(|(n, _)| *n == "streams").ok_or_else(|| RenderError::MissingArgument("streams".to_string()))?.1.clone();
    let v2 = match val_v2 {
        Value::String(s) => s.to_string(),
        _ => return Err(RenderError::InvalidArgumentType("streams".to_string())),
    };
    f_args.set("streams", FluentValue::from(v2.clone()));
    let val_v3 = args.iter().find(|(n, _)| *n == "segments").ok_or_else(|| RenderError::MissingArgument("segments".to_string()))?.1.clone();
    let v3 = match val_v3 {
        Value::String(s) => s.to_string(),
        _ => return Err(RenderError::InvalidArgumentType("segments".to_string())),
    };
    f_args.set("segments", FluentValue::from(v3.clone()));
    f_args.set("i18nCategory", plural_category(_cardinal_rule, v0));
    f_args.set("i18nCategory-segment_quantity", plural_category(_cardinal_rule, v1));
    Ok((_resource_key, f_args))
}

fn prepare_m6a6f75726e616c2e636c692e73747265616d732e7265636f72642e6d616c666f726d6564(_resource_key: &'static str, args: &[(&str, Value)], _cardinal_rule: Option<&str>) -> Prepared {
    const DECLARED: &[&str] = &["path"];
    for (name, _) in args {
        if !DECLARED.contains(name) {
            return Err(RenderError::UnknownArgument((*name).to_string()));
        }
    }
    #[allow(unused_mut)]
    let mut f_args = FluentArgs::new();
    let val_v0 = args.iter().find(|(n, _)| *n == "path").ok_or_else(|| RenderError::MissingArgument("path".to_string()))?.1.clone();
    let v0 = match val_v0 {
        Value::String(s) => s.to_string(),
        _ => return Err(RenderError::InvalidArgumentType("path".to_string())),
    };
    f_args.set("path", FluentValue::from(v0.clone()));
    Ok((_resource_key, f_args))
}

fn prepare_m6a6f75726e616c2e636c692e73747265616d732e7265706169722e6168656164(_resource_key: &'static str, args: &[(&str, Value)], _cardinal_rule: Option<&str>) -> Prepared {
    const DECLARED: &[&str] = &["stream"];
    for (name, _) in args {
        if !DECLARED.contains(name) {
            return Err(RenderError::UnknownArgument((*name).to_string()));
        }
    }
    #[allow(unused_mut)]
    let mut f_args = FluentArgs::new();
    let val_v0 = args.iter().find(|(n, _)| *n == "stream").ok_or_else(|| RenderError::MissingArgument("stream".to_string()))?.1.clone();
    let v0 = match val_v0 {
        Value::String(s) => s.to_string(),
        _ => return Err(RenderError::InvalidArgumentType("stream".to_string())),
    };
    f_args.set("stream", FluentValue::from(v0.clone()));
    Ok((_resource_key, f_args))
}

fn prepare_m6a6f75726e616c2e636c692e73747265616d732e7265706169722e6c6f636b6564(_resource_key: &'static str, args: &[(&str, Value)], _cardinal_rule: Option<&str>) -> Prepared {
    const DECLARED: &[&str] = &["stream", "path"];
    for (name, _) in args {
        if !DECLARED.contains(name) {
            return Err(RenderError::UnknownArgument((*name).to_string()));
        }
    }
    #[allow(unused_mut)]
    let mut f_args = FluentArgs::new();
    let val_v0 = args.iter().find(|(n, _)| *n == "stream").ok_or_else(|| RenderError::MissingArgument("stream".to_string()))?.1.clone();
    let v0 = match val_v0 {
        Value::String(s) => s.to_string(),
        _ => return Err(RenderError::InvalidArgumentType("stream".to_string())),
    };
    f_args.set("stream", FluentValue::from(v0.clone()));
    let val_v1 = args.iter().find(|(n, _)| *n == "path").ok_or_else(|| RenderError::MissingArgument("path".to_string()))?.1.clone();
    let v1 = match val_v1 {
        Value::String(s) => s.to_string(),
        _ => return Err(RenderError::InvalidArgumentType("path".to_string())),
    };
    f_args.set("path", FluentValue::from(v1.clone()));
    Ok((_resource_key, f_args))
}

fn prepare_m6a6f75726e616c2e636c692e73747265616d732e7265706169722e6d616c666f726d6564(_resource_key: &'static str, args: &[(&str, Value)], _cardinal_rule: Option<&str>) -> Prepared {
    const DECLARED: &[&str] = &["stream", "path"];
    for (name, _) in args {
        if !DECLARED.contains(name) {
            return Err(RenderError::UnknownArgument((*name).to_string()));
        }
    }
    #[allow(unused_mut)]
    let mut f_args = FluentArgs::new();
    let val_v0 = args.iter().find(|(n, _)| *n == "stream").ok_or_else(|| RenderError::MissingArgument("stream".to_string()))?.1.clone();
    let v0 = match val_v0 {
        Value::String(s) => s.to_string(),
        _ => return Err(RenderError::InvalidArgumentType("stream".to_string())),
    };
    f_args.set("stream", FluentValue::from(v0.clone()));
    let val_v1 = args.iter().find(|(n, _)| *n == "path").ok_or_else(|| RenderError::MissingArgument("path".to_string()))?.1.clone();
    let v1 = match val_v1 {
        Value::String(s) => s.to_string(),
        _ => return Err(RenderError::InvalidArgumentType("path".to_string())),
    };
    f_args.set("path", FluentValue::from(v1.clone()));
    Ok((_resource_key, f_args))
}

fn prepare_m6a6f75726e616c2e636c692e73747265616d732e7265706169722e6d697373696e67(_resource_key: &'static str, args: &[(&str, Value)], _cardinal_rule: Option<&str>) -> Prepared {
    const DECLARED: &[&str] = &["stream", "path"];
    for (name, _) in args {
        if !DECLARED.contains(name) {
            return Err(RenderError::UnknownArgument((*name).to_string()));
        }
    }
    #[allow(unused_mut)]
    let mut f_args = FluentArgs::new();
    let val_v0 = args.iter().find(|(n, _)| *n == "stream").ok_or_else(|| RenderError::MissingArgument("stream".to_string()))?.1.clone();
    let v0 = match val_v0 {
        Value::String(s) => s.to_string(),
        _ => return Err(RenderError::InvalidArgumentType("stream".to_string())),
    };
    f_args.set("stream", FluentValue::from(v0.clone()));
    let val_v1 = args.iter().find(|(n, _)| *n == "path").ok_or_else(|| RenderError::MissingArgument("path".to_string()))?.1.clone();
    let v1 = match val_v1 {
        Value::String(s) => s.to_string(),
        _ => return Err(RenderError::InvalidArgumentType("path".to_string())),
    };
    f_args.set("path", FluentValue::from(v1.clone()));
    Ok((_resource_key, f_args))
}

fn prepare_m6a6f75726e616c2e636c692e73747265616d732e7265706169722e77726974652e6661696c6564(_resource_key: &'static str, args: &[(&str, Value)], _cardinal_rule: Option<&str>) -> Prepared {
    const DECLARED: &[&str] = &["stream", "path"];
    for (name, _) in args {
        if !DECLARED.contains(name) {
            return Err(RenderError::UnknownArgument((*name).to_string()));
        }
    }
    #[allow(unused_mut)]
    let mut f_args = FluentArgs::new();
    let val_v0 = args.iter().find(|(n, _)| *n == "stream").ok_or_else(|| RenderError::MissingArgument("stream".to_string()))?.1.clone();
    let v0 = match val_v0 {
        Value::String(s) => s.to_string(),
        _ => return Err(RenderError::InvalidArgumentType("stream".to_string())),
    };
    f_args.set("stream", FluentValue::from(v0.clone()));
    let val_v1 = args.iter().find(|(n, _)| *n == "path").ok_or_else(|| RenderError::MissingArgument("path".to_string()))?.1.clone();
    let v1 = match val_v1 {
        Value::String(s) => s.to_string(),
        _ => return Err(RenderError::InvalidArgumentType("path".to_string())),
    };
    f_args.set("path", FluentValue::from(v1.clone()));
    Ok((_resource_key, f_args))
}

fn prepare_m6a6f75726e616c2e636c692e73747265616d732e7365676d656e74732e6572726f72(_resource_key: &'static str, args: &[(&str, Value)], _cardinal_rule: Option<&str>) -> Prepared {
    const DECLARED: &[&str] = &["error"];
    for (name, _) in args {
        if !DECLARED.contains(name) {
            return Err(RenderError::UnknownArgument((*name).to_string()));
        }
    }
    #[allow(unused_mut)]
    let mut f_args = FluentArgs::new();
    let val_v0 = args.iter().find(|(n, _)| *n == "error").ok_or_else(|| RenderError::MissingArgument("error".to_string()))?.1.clone();
    let v0 = match val_v0 {
        Value::String(s) => s.to_string(),
        _ => return Err(RenderError::InvalidArgumentType("error".to_string())),
    };
    f_args.set("error", FluentValue::from(v0.clone()));
    Ok((_resource_key, f_args))
}

fn load_bundle(ftl: &str, use_isolating: bool) -> Result<FluentBundle<FluentResource>, RenderError> {
    let lang = "und".parse().map_err(|_| RenderError::FormatFailed)?;
    let mut bundle = FluentBundle::new_concurrent(vec![lang]);
    bundle.set_use_isolating(use_isolating);
    let _ = bundle.add_builtins();
    let resource = FluentResource::try_new(ftl.to_owned()).map_err(|_| RenderError::FormatFailed)?;
    bundle.add_resource(resource).map_err(|_| RenderError::FormatFailed)?;
    Ok(bundle)
}
