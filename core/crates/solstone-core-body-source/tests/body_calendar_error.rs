// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::error::Error;

use solstone_core_body_source::{BodyCalendarError, BodyCalendarField};

#[test]
fn body_calendar_errors_are_bounded_redacting_and_source_free() {
    for (field, expected) in [
        (BodyCalendarField::Day, "body-calendar invalid_format: day"),
        (
            BodyCalendarField::Month,
            "body-calendar invalid_format: month",
        ),
    ] {
        let error = BodyCalendarError::InvalidFormat(field);
        let display = error.to_string();
        let debug = format!("{error:?}");
        assert_eq!(display, expected);
        assert_eq!(debug, expected);
        assert!(display.len() <= 48 && debug.len() <= 48);
        assert!(Error::source(&error).is_none());
    }
}
