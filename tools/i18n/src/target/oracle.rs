// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

pub fn format_integer_ungrouped(val: i64) -> String {
    format!("{}", val)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_integer_ungrouped() {
        assert_eq!(format_integer_ungrouped(10000), "10000");
        assert_eq!(format_integer_ungrouped(-1), "-1");
        assert_eq!(format_integer_ungrouped(i32::MIN as i64), "-2147483648");
        assert_eq!(
            format_integer_ungrouped(9007199254740991_i64),
            "9007199254740991"
        );
    }
}
