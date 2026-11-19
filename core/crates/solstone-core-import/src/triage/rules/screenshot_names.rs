// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use super::Evidence;
use file_format::FileFormat;
use jiff::civil::{Date, Time};
use regex::Regex;
use std::borrow::Cow;
use std::sync::LazyLock;

macro_rules! dated_name {
    ($format:ident, $pattern:literal, $date:literal, $time:literal) => {
        |e| {
            static PATTERN: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
                regex::Regex::new(concat!(r"\A", $pattern, r"\z")).expect("valid screenshot name")
            });
            screenshot_names::dated(screenshot_names::$format(e), &PATTERN, $date, $time)
        }
    };
}
pub(super) use dated_name;

pub(super) fn aosp_png_name(evidence: &Evidence<'_>) -> bool {
    static PATTERN: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"\A[Ss]creenshot[ _](?<date>[0-9]{8})-(?<time>[0-9]{6})(?:-display-[0-9]+)?\z")
            .expect("valid AOSP screenshot name")
    });
    dated(png(evidence), &PATTERN, "%Y%m%d", "%H%M%S")
}

pub(super) fn macos_name(evidence: &Evidence<'_>) -> bool {
    static ENGLISH: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"\A[Ss]creenshot[ _](?<date>[0-9]{4}-[0-9]{2}-[0-9]{2})[ _]at[ _](?<time>[0-9]{1,2}\.[0-9]{2}\.[0-9]{2})(?:[ _\x{202f}](?<meridiem>[AP]M))?\z")
            .expect("valid English macOS screenshot name")
    });
    static LOCALIZED: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(concat!(
            r"\A(?:(?<de>[Bb]ildschirmfoto)[ _]|(?<fr>[Cc]apture[ _]d’écran)[ _]|(?<ru>[Сс]нимок[ _]экрана)[ _]|截屏)",
            r"(?<date>[0-9]{4}-[0-9]{2}-[0-9]{2})(?:[ _](?<connector>um|à|в))?[ _]",
            r"(?<time>[0-9]{2}\.[0-9]{2}\.[0-9]{2})\z"
        ))
        .expect("valid localized macOS screenshot name")
    });

    let Some(stem) = png(evidence) else {
        return false;
    };
    if let Some(stamp) = ENGLISH.captures(stem) {
        let hour = stamp["time"].split_once('.').expect("dot-separated time").0;
        return if stamp.name("meridiem").is_some() {
            hour.parse::<u8>()
                .is_ok_and(|hour| (1..=12).contains(&hour))
        } else {
            hour.len() == 2
        } && valid_stamp(&stamp, "%Y-%m-%d", "%H.%M.%S");
    }
    LOCALIZED.captures(stem).is_some_and(|stamp| {
        let connector = stamp.name("connector").map(|m| m.as_str());
        let localized = match connector {
            Some("um") => stamp.name("de").is_some(),
            Some("à") => stamp.name("fr").is_some(),
            Some("в") => stamp.name("ru").is_some(),
            None => stem.starts_with("截屏"),
            _ => false,
        };
        localized && valid_stamp(&stamp, "%Y-%m-%d", "%H.%M.%S")
    })
}

pub(super) fn png<'a>(evidence: &Evidence<'a>) -> Option<&'a str> {
    stem(evidence, FileFormat::PortableNetworkGraphics, "png")
}

pub(super) fn jpeg<'a>(evidence: &Evidence<'a>) -> Option<&'a str> {
    stem(evidence, FileFormat::JointPhotographicExpertsGroup, "jpg")
}

fn stem<'a>(evidence: &Evidence<'a>, format: FileFormat, extension: &str) -> Option<&'a str> {
    let (stem, suffix) = evidence.name.rsplit_once('.')?;
    (evidence.format == format && suffix.eq_ignore_ascii_case(extension)).then_some(stem)
}

pub(super) fn dated(stem: Option<&str>, pattern: &Regex, date: &str, time: &str) -> bool {
    let Some(stamp) = stem.and_then(|stem| pattern.captures(stem)) else {
        return false;
    };
    stamp
        .name("prefix")
        .is_none_or(|m| !m.as_str().trim_matches([' ', '_']).is_empty())
        && valid_stamp(&stamp, date, time)
}

fn valid_stamp(stamp: &regex::Captures<'_>, date_format: &str, time_format: &str) -> bool {
    let time = &stamp["time"];
    let time = if time_format == "%H %M %S" && time.contains('_') {
        Cow::Owned(time.replace('_', " "))
    } else {
        Cow::Borrowed(time)
    };
    // Local system filenames only carry wall-clock seconds from 00 through 59.
    !time.ends_with("60")
        && Date::strptime(date_format, &stamp["date"]).is_ok()
        && Time::strptime(time_format, time.as_ref()).is_ok()
}
