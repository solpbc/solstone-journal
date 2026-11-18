// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Expected readings of calendars and of dates and date-times.

use std::fmt::Write as _;

use super::*;

fn reading(text: &str) -> String {
    let Some(calendar) = Calendar::parse(text) else {
        return "REFUSED\n".to_owned();
    };
    let mut out = String::from("CALENDAR\n");
    for property in calendar.properties() {
        property_line(&mut out, "cal", property);
    }
    let _ = writeln!(
        out,
        "cal X-WR-CALNAME={:?} name={:?}",
        calendar.property_value("X-WR-CALNAME"),
        calendar.get_name()
    );
    for event in calendar.events() {
        out.push_str("EVENT\n");
        for property in event.properties().values() {
            property_line(&mut out, "single", property);
            time_line(&mut out, property);
        }
        for property in event.multi_properties().values().flatten() {
            property_line(&mut out, "multi", property);
            time_line(&mut out, property);
        }
        let _ = writeln!(
            out,
            "start={} end={}",
            perhaps(event.get_start().as_ref()),
            perhaps(event.get_end().as_ref())
        );
    }
    out
}

fn property_line(out: &mut String, kind: &str, property: &Property) {
    let _ = write!(out, "{kind} {:?}={:?}", property.key(), property.value());
    for param in property.params().values() {
        let _ = write!(out, " ;{:?}={:?}", param.key(), param.value());
    }
    out.push('\n');
}

fn time_line(out: &mut String, property: &Property) {
    let value = DatePerhapsTime::from_property(property);
    let _ = writeln!(out, "  time {}", perhaps(value.as_ref()));
}

fn perhaps(value: Option<&DatePerhapsTime>) -> String {
    use solstone_core_time::{Precision, utc_rfc3339, zoned_rfc3339};
    match value {
        None => "none".to_owned(),
        Some(DatePerhapsTime::Date(date)) => format!("date {}", date.strftime("%Y-%m-%d")),
        Some(DatePerhapsTime::DateTime(CalendarDateTime::Floating(wall))) => {
            format!("floating {}", wall.strftime("%Y-%m-%dT%H:%M:%S"))
        }
        Some(DatePerhapsTime::DateTime(CalendarDateTime::Utc(time))) => {
            format!("utc {}", utc_rfc3339(*time))
        }
        Some(DatePerhapsTime::DateTime(
            time @ CalendarDateTime::WithTimezone { date_time, tzid },
        )) => {
            format!(
                "zoned {} {:?} utc={} zoned={}",
                date_time.strftime("%Y-%m-%dT%H:%M:%S"),
                tzid,
                time.try_into_utc().map_or("none".to_owned(), utc_rfc3339),
                time.with_zone()
                    .map_or("none".to_owned(), |zoned| zoned_rfc3339(
                        &zoned,
                        Precision::AutoSi,
                        false
                    )),
            )
        }
    }
}

/// (calendar text, expected reading)
const CASES: &[(&str, &str)] = &[
    (
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nX-WR-CALNAME:Work\r\nBEGIN:VEVENT\r\nUID:standup-1\r\nSUMMARY:standup\r\nDTSTART;TZID=America/Denver:20261020T090000\r\nDTEND;TZID=America/Denver:20261020T091500\r\nRRULE:FREQ=WEEKLY;BYDAY=TU;COUNT=4\r\nEXDATE;TZID=America/Denver:20261103T090000\r\nRDATE;TZID=America/Denver:20261022T120000\r\nLAST-MODIFIED:20261019T154200Z\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
        "CALENDAR\ncal \"VERSION\"=\"2.0\"\ncal \"X-WR-CALNAME\"=\"Work\"\ncal X-WR-CALNAME=Some(\"Work\") name=Some(\"Work\")\nEVENT\nsingle \"DTEND\"=\"20261020T091500\" ;\"TZID\"=\"America/Denver\"\n  time zoned 2026-10-20T09:15:00 \"America/Denver\" utc=2026-10-20T15:15:00+00:00 zoned=2026-10-20T09:15:00-06:00\nsingle \"DTSTART\"=\"20261020T090000\" ;\"TZID\"=\"America/Denver\"\n  time zoned 2026-10-20T09:00:00 \"America/Denver\" utc=2026-10-20T15:00:00+00:00 zoned=2026-10-20T09:00:00-06:00\nsingle \"LAST-MODIFIED\"=\"20261019T154200Z\"\n  time utc 2026-10-19T15:42:00+00:00\nsingle \"RRULE\"=\"FREQ=WEEKLY;BYDAY=TU;COUNT=4\"\n  time none\nsingle \"SUMMARY\"=\"standup\"\n  time none\nsingle \"UID\"=\"standup-1\"\n  time none\nmulti \"EXDATE\"=\"20261103T090000\" ;\"TZID\"=\"America/Denver\"\n  time zoned 2026-11-03T09:00:00 \"America/Denver\" utc=2026-11-03T16:00:00+00:00 zoned=2026-11-03T09:00:00-07:00\nmulti \"RDATE\"=\"20261022T120000\" ;\"TZID\"=\"America/Denver\"\n  time zoned 2026-10-22T12:00:00 \"America/Denver\" utc=2026-10-22T18:00:00+00:00 zoned=2026-10-22T12:00:00-06:00\nstart=zoned 2026-10-20T09:00:00 \"America/Denver\" utc=2026-10-20T15:00:00+00:00 zoned=2026-10-20T09:00:00-06:00 end=zoned 2026-10-20T09:15:00 \"America/Denver\" utc=2026-10-20T15:15:00+00:00 zoned=2026-10-20T09:15:00-06:00\n",
    ),
    (
        "BEGIN:VCALENDAR\nBEGIN:VEVENT\nSUMMARY:a long\r\n  title\nDESCRIPTION:line\\none\\, two\\; three\\\\ four\\: five\\t six\\\nLOCATION:Room\n\t 1\nATTENDEE;CN=\"Doe, Jane\";PARTSTAT=ACCEPTED:mailto:Jane@Example.com\nATTENDEE;CN=Plain;CN=Again:MAILTO:plain@example.com\nATTENDEE;CN=\"\":mailto:empty@example.com\nORGANIZER; CN=Spaced;X=:mailto:org@example.com\nURL:https\\://example.com\nX-FOO;VALUE=URI:a\\nb\nCOMMENT;VALUE=TEXT:x\\Ny\nEND:VEVENT\nEND:VCALENDAR\n",
        "CALENDAR\ncal X-WR-CALNAME=None name=None\nEVENT\nsingle \"DESCRIPTION\"=\"line\\none, two; three\\\\ four: five\\\\t six\\\\\"\n  time none\nsingle \"LOCATION\"=\"Room 1\"\n  time none\nsingle \"ORGANIZER\"=\"mailto:org@example.com\" ;\"CN\"=\"Spaced\" ;\"X\"=\"\"\n  time none\nsingle \"SUMMARY\"=\"a long title\"\n  time none\nsingle \"URL\"=\"https\\\\://example.com\"\n  time none\nsingle \"X-FOO\"=\"a\\\\nb\" ;\"VALUE\"=\"URI\"\n  time none\nmulti \"ATTENDEE\"=\"mailto:Jane@Example.com\" ;\"CN\"=\"Doe, Jane\" ;\"PARTSTAT\"=\"ACCEPTED\"\n  time none\nmulti \"ATTENDEE\"=\"MAILTO:plain@example.com\" ;\"CN\"=\"Again\"\n  time none\nmulti \"ATTENDEE\"=\"mailto:empty@example.com\" ;\"CN\"=\"\\\"\\\"\"\n  time none\nmulti \"COMMENT\"=\"x\\ny\" ;\"VALUE\"=\"TEXT\"\n  time none\nstart=none end=none\n",
    ),
    (
        "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nDTSTART;VALUE=DATE:20261031\r\nDTEND;VALUE=DATE:20261101\r\nCREATED:20261001T120000\r\nLAST-MODIFIED:2026101T9000\r\nDTSTAMP: 20261019T154200Z\r\nRECURRENCE-ID;TZID=Pacific Standard Time:20261022T090000\r\nX-A;TZID=America/Denver:20261020\r\nX-B;VALUE=DATE:20261020T090000\r\nX-C;TZID=Asia/Calcutta:20261020T090000\r\nX-D;TZID=America/Denver:20260308T023000\r\nX-E;TZID=America/Denver:20261101T013000\r\nX-F;TZID=\"Europe/London\":20261020T090000\r\nX-G:20261320T090000\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
        "CALENDAR\ncal X-WR-CALNAME=None name=None\nEVENT\nsingle \"CREATED\"=\"20261001T120000\"\n  time floating 2026-10-01T12:00:00\nsingle \"DTEND\"=\"20261101\" ;\"VALUE\"=\"DATE\"\n  time date 2026-11-01\nsingle \"DTSTAMP\"=\" 20261019T154200Z\"\n  time utc 2026-10-19T15:42:00+00:00\nsingle \"DTSTART\"=\"20261031\" ;\"VALUE\"=\"DATE\"\n  time date 2026-10-31\nsingle \"LAST-MODIFIED\"=\"2026101T9000\"\n  time none\nsingle \"RECURRENCE-ID\"=\"20261022T090000\" ;\"TZID\"=\"Pacific Standard Time\"\n  time zoned 2026-10-22T09:00:00 \"Pacific Standard Time\" utc=none zoned=none\nsingle \"X-A\"=\"20261020\" ;\"TZID\"=\"America/Denver\"\n  time date 2026-10-20\nsingle \"X-B\"=\"20261020T090000\" ;\"VALUE\"=\"DATE\"\n  time none\nsingle \"X-C\"=\"20261020T090000\" ;\"TZID\"=\"Asia/Calcutta\"\n  time zoned 2026-10-20T09:00:00 \"Asia/Calcutta\" utc=2026-10-20T03:30:00+00:00 zoned=2026-10-20T09:00:00+05:30\nsingle \"X-D\"=\"20260308T023000\" ;\"TZID\"=\"America/Denver\"\n  time zoned 2026-03-08T02:30:00 \"America/Denver\" utc=none zoned=none\nsingle \"X-E\"=\"20261101T013000\" ;\"TZID\"=\"America/Denver\"\n  time zoned 2026-11-01T01:30:00 \"America/Denver\" utc=none zoned=none\nsingle \"X-F\"=\"20261020T090000\" ;\"TZID\"=\"Europe/London\"\n  time zoned 2026-10-20T09:00:00 \"Europe/London\" utc=2026-10-20T08:00:00+00:00 zoned=2026-10-20T09:00:00+01:00\nsingle \"X-G\"=\"20261320T090000\"\n  time none\nstart=date 2026-10-31 end=date 2026-11-01\n",
    ),
    (
        "BEGIN:VCALENDAR\r\nNAME:Calendar\r\nX-WR-CALNAME:Other\r\nX-WR-CALNAME:Third\r\nBEGIN:VTIMEZONE\r\nTZID:America/Denver\r\nBEGIN:STANDARD\r\nDTSTART:19701101T020000\r\nEND:STANDARD\r\nEND:VTIMEZONE\r\nBEGIN:VEVENT\r\nsummary:lower\r\nSummary:Mixed\\nCase\r\nSUMMARY:first\r\nSUMMARY:second\r\nBEGIN:VALARM\r\nTRIGGER:-PT15M\r\nSUMMARY:alarm\r\nEND:VALARM\r\nEND:VEVENT\r\nbegin:VEVENT\r\nUID:2\r\nEnd:VEVENT\r\nBEGIN:VTODO\r\nSUMMARY:todo\r\nEND:VTODO\r\nEND:VCALENDAR\r\n\r\n\n",
        "CALENDAR\ncal \"NAME\"=\"Calendar\"\ncal \"X-WR-CALNAME\"=\"Other\"\ncal \"X-WR-CALNAME\"=\"Third\"\ncal X-WR-CALNAME=Some(\"Other\") name=Some(\"Calendar\")\nEVENT\nsingle \"SUMMARY\"=\"second\"\n  time none\nsingle \"Summary\"=\"Mixed\\\\nCase\"\n  time none\nsingle \"summary\"=\"lower\"\n  time none\nstart=none end=none\nEVENT\nsingle \"UID\"=\"2\"\n  time none\nstart=none end=none\n",
    ),
    (
        "BEGIN:VEVENT\nSUMMARY\nJUNK\n:novalue\nX-EMPTY;\nBEGINNING:yes\nEND:VEVENT",
        "REFUSED\n",
    ),
    ("BEGIN:VCALENDAR\r\nEND:VCALENDAR\r\njunk", "REFUSED\n"),
    (
        "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nEND:VTODO\r\nEND:VCALENDAR\r\n",
        "REFUSED\n",
    ),
    (
        "BEGIN:VCALENDAR\r\nBEGIN:\r\nEND:VCALENDAR\r\n",
        "REFUSED\n",
    ),
    (
        "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\njunk line\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
        "REFUSED\n",
    ),
    (
        "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nSUMMARY:x\r\nEND:VCALENDAR\r\n",
        "REFUSED\n",
    ),
    ("this is not a calendar", "REFUSED\n"),
    ("", "REFUSED\n"),
];

#[test]
fn each_calendar_reads_as_recorded() {
    for (text, expected) in CASES {
        assert_eq!(reading(text), *expected, "{text:?}");
    }
}

/// (text, as a date, as a date-time, as a UTC date-time)
#[rustfmt::skip]
#[allow(clippy::type_complexity)]
const DATE_TEXTS: &[(&str, Option<&str>, Option<&str>, Option<&str>)] = &[
    ("20261020", Some("2026-10-20"), None, None),
    ("2026102", Some("2026-10-02"), None, None),
    ("202611", None, None, None),
    ("2026 10 20", Some("2026-10-20"), None, None),
    (" 20261020", Some("2026-10-20"), None, None),
    ("20261020 ", None, None, None),
    ("+2026 1020", Some("2026-10-20"), None, None),
    ("02026", None, None, None),
    ("20261320", None, None, None),
    ("20260229", None, None, None),
    ("20280229", Some("2028-02-29"), None, None),
    ("00010101", Some("0001-01-01"), None, None),
    ("20261020T090000", None, Some("2026-10-20T09:00:00"), None),
    ("20261020T9000", None, None, None),
    ("20261020T 90000", None, None, None),
    ("20261020t090000", None, None, None),
    ("20261020T240000", None, None, None),
    ("20261020T235959Z", None, None, Some("2026-10-20T23:59:59+00:00")),
    ("20261020T090000z", None, None, None),
    ("2026-10-20", None, None, None),
    ("", None, None, None),
    ("T", None, None, None),
    ("２０２６1020", None, None, None),
];

#[test]
fn dates_read_as_recorded() {
    for (text, date, date_time, utc) in DATE_TEXTS {
        let read_date = parse_ical_date(text).map(|d| d.strftime("%Y-%m-%d").to_string());
        let read_date_time =
            parse_ical_datetime(text).map(|d| d.strftime("%Y-%m-%dT%H:%M:%S").to_string());
        let read_utc = parse_ical_utc(text).map(solstone_core_time::utc_rfc3339);
        assert_eq!(read_date.as_deref(), *date, "{text:?} as a date");
        assert_eq!(
            read_date_time.as_deref(),
            *date_time,
            "{text:?} as a date-time"
        );
        assert_eq!(read_utc.as_deref(), *utc, "{text:?} as UTC");
    }
}

#[test]
fn a_leading_byte_order_mark_is_skipped() {
    let text =
        "\u{feff}BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nSUMMARY:x\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    let calendar = Calendar::parse(text).expect("a calendar after its mark");
    let events: Vec<_> = calendar.events().collect();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].property_value("SUMMARY"), Some("x"));
    // Only a leading one: a mark inside the text is a line that is no property.
    assert!(Calendar::parse("BEGIN:VCALENDAR\r\n\u{feff}X:1\r\nEND:VCALENDAR\r\n").is_none());
}

#[test]
fn component_names_are_read_in_any_case() {
    let text = "begin:vcalendar\r\nX-WR-CALNAME:Home\r\nbegin:vevent\r\nSUMMARY:lower\r\nend:vevent\r\nBEGIN:VEvent\r\nSUMMARY:mixed\r\nEnd:VEVENT\r\nend:VCALENDAR\r\n";
    let calendar = Calendar::parse(text).expect("a calendar in lower case");
    assert_eq!(calendar.property_value("X-WR-CALNAME"), Some("Home"));
    let summaries: Vec<_> = calendar
        .events()
        .map(|event| event.property_value("SUMMARY"))
        .collect();
    assert_eq!(summaries, [Some("lower"), Some("mixed")]);
    // A mismatched name still refuses the file, in any case.
    assert!(
        Calendar::parse("BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nEND:vtodo\r\nEND:VCALENDAR\r\n")
            .is_none()
    );
}

#[test]
fn a_value_ends_at_its_own_line_break_in_a_file_of_mixed_endings() {
    let text = "BEGIN:VCALENDAR\nBEGIN:VEVENT\nSUMMARY:one\nDESCRIPTION:two\r\nLOCATION:three\nEND:VEVENT\r\nEND:VCALENDAR\n";
    let calendar = Calendar::parse(text).expect("a calendar of mixed endings");
    let event = calendar.events().next().expect("one event");
    assert_eq!(event.property_value("SUMMARY"), Some("one"));
    assert_eq!(event.property_value("DESCRIPTION"), Some("two"));
    assert_eq!(event.property_value("LOCATION"), Some("three"));
    // A carriage return not before a line feed stays in the value.
    let lone = "BEGIN:VCALENDAR\nBEGIN:VEVENT\nSUMMARY:a\rb\nEND:VEVENT\nEND:VCALENDAR\n";
    let calendar = Calendar::parse(lone).expect("a calendar");
    assert_eq!(
        calendar.events().next().unwrap().property_value("SUMMARY"),
        Some("a\rb")
    );
}

#[test]
fn an_end_names_its_component_exactly() {
    // A longer name is another component's end, so the file is refused.
    for text in [
        "BEGIN:VCALENDAR\nBEGIN:VEVENT\nEND:VEVENTX\nEND:VCALENDAR\n",
        "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nSUMMARY:x\r\nEND:VEVENT-OLD\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
        "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nEND:VEVENT\r\nEND:VCALENDARS\r\n",
    ] {
        assert!(Calendar::parse(text).is_none(), "{text:?}");
    }
    // The exact name, in any case, closes it.
    let calendar = Calendar::parse(
        "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nSUMMARY:x\r\nend:vevent\r\nEND:VCALENDAR\r\n",
    )
    .expect("a calendar");
    assert_eq!(
        calendar.events().next().unwrap().property_value("SUMMARY"),
        Some("x")
    );
}
