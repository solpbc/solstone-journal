// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Legacy active-profile pagination rules.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Pagination {
    pub(crate) limit: usize,
    pub(crate) offset: usize,
}

pub(crate) fn parse_pagination(limit: Option<&str>, offset: Option<&str>) -> Pagination {
    let limit = limit
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or(20)
        .clamp(1, 100) as usize;
    let offset = offset
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or(0)
        .max(0) as usize;
    Pagination { limit, offset }
}

#[cfg(test)]
mod tests {
    use super::{Pagination, parse_pagination};

    #[test]
    fn clamps_defaults_malformed_and_negative_values() {
        assert_eq!(
            parse_pagination(None, None),
            Pagination {
                limit: 20,
                offset: 0
            }
        );
        assert_eq!(
            parse_pagination(Some("nope"), Some("bad")),
            Pagination {
                limit: 20,
                offset: 0
            }
        );
        assert_eq!(
            parse_pagination(Some("0"), Some("-2")),
            Pagination {
                limit: 1,
                offset: 0
            }
        );
        assert_eq!(
            parse_pagination(Some("999"), Some("7")),
            Pagination {
                limit: 100,
                offset: 7
            }
        );
    }
}
