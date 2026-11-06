// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! One `Server-Timing` line per response.
//!
//! Every layer and handler adds its entry through [`append`], so all entries
//! survive as one comma-separated list (RFC 9110 list syntax). Never send them
//! as separate lines: solstone macOS through 2.0.59 reads the journal's local
//! socket with a parser that refuses any repeated header name.

use axum::http::header::{HeaderMap, HeaderName, HeaderValue};

pub const SERVER_TIMING: HeaderName = HeaderName::from_static("server-timing");

pub fn append(headers: &mut HeaderMap, entry: HeaderValue) {
    let mut line = Vec::new();
    for existing in headers.get_all(SERVER_TIMING) {
        line.extend_from_slice(existing.as_bytes());
        line.extend_from_slice(b", ");
    }
    line.extend_from_slice(entry.as_bytes());
    let line = HeaderValue::from_bytes(&line).expect("valid values joined by a comma stay valid");
    headers.insert(SERVER_TIMING, line);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_keep_their_order_on_one_line() {
        let mut headers = HeaderMap::new();
        append(&mut headers, HeaderValue::from_static("gate;dur=1.5"));
        append(&mut headers, HeaderValue::from_static("app;dur=2.5"));
        let lines: Vec<_> = headers.get_all(SERVER_TIMING).iter().collect();
        assert_eq!(lines, ["gate;dur=1.5, app;dur=2.5"]);
    }

    #[test]
    fn lines_already_present_fold_into_one() {
        let mut headers = HeaderMap::new();
        headers.append(SERVER_TIMING, HeaderValue::from_static("first"));
        headers.append(SERVER_TIMING, HeaderValue::from_static("second"));
        append(&mut headers, HeaderValue::from_static("third"));
        let lines: Vec<_> = headers.get_all(SERVER_TIMING).iter().collect();
        assert_eq!(lines, ["first, second, third"]);
    }
}
