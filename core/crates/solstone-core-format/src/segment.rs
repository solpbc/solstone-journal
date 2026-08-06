// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SegmentTimes {
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
}

pub fn is_date_key(value: &str) -> bool {
    value.len() == 8 && value.bytes().all(|byte| byte.is_ascii_digit())
}

pub fn segment_key(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut index = 0;
    while index + 8 <= bytes.len() {
        if !has_word_boundary_before(bytes, index) || !bytes[index].is_ascii_digit() {
            index += 1;
            continue;
        }
        if !bytes[index..index + 6]
            .iter()
            .all(|byte| byte.is_ascii_digit())
            || bytes[index + 6] != b'_'
        {
            index += 1;
            continue;
        }
        let mut end = index + 7;
        while end < bytes.len() && bytes[end].is_ascii_digit() {
            end += 1;
        }
        if end == index + 7 {
            index += 1;
            continue;
        }
        if end == bytes.len() || bytes[end] == b'_' || !is_word_byte(bytes[end]) {
            return Some(value[index..end].to_string());
        }
        index += 1;
    }
    None
}

pub fn segment_parse(value: &str) -> Option<SegmentTimes> {
    let name = if value.contains('/') || value.contains('\\') {
        let normalized = value.replace('\\', "/");
        let parts: Vec<&str> = normalized.split('/').collect();
        let mut found = None;
        for (index, part) in parts.iter().enumerate() {
            if is_date_key(part) {
                for candidate in parts.iter().skip(index + 1) {
                    if segment_key(candidate).is_some() {
                        found = Some((*candidate).to_string());
                        break;
                    }
                }
            }
            if found.is_some() {
                break;
            }
        }
        found?
    } else {
        value.to_string()
    };

    let (time_part, length_part) = name.split_once('_')?;
    if time_part.len() != 6
        || !time_part.bytes().all(|byte| byte.is_ascii_digit())
        || length_part.is_empty()
        || !length_part.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let hour = time_part[0..2].parse::<u8>().ok()?;
    let minute = time_part[2..4].parse::<u8>().ok()?;
    let second = time_part[4..6].parse::<u8>().ok()?;
    if hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    Some(SegmentTimes {
        hour,
        minute,
        second,
    })
}

pub fn time_bucket(rel: &str) -> String {
    match segment_parse(rel).map(|times| times.hour) {
        Some(6..=11) => "morning".to_string(),
        Some(12..=16) => "afternoon".to_string(),
        Some(17..=20) => "evening".to_string(),
        Some(_) => "night".to_string(),
        None => String::new(),
    }
}

pub fn is_historical_day(rel: &str, today: &str) -> bool {
    if !rel.contains('/') {
        return false;
    }
    let first = rel.split('/').next().unwrap_or("");
    is_date_key(first) && first < today
}

fn has_word_boundary_before(bytes: &[u8], index: usize) -> bool {
    if index == 0 {
        return true;
    }
    !is_word_byte(bytes[index - 1]) && is_word_byte(bytes[index])
}

fn is_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segment_key_matches_python_boundary_semantics() {
        assert_eq!(segment_key("143022_300"), Some("143022_300".to_string()));
        assert_eq!(segment_key("123456_1"), Some("123456_1".to_string()));
        assert_eq!(segment_key("123456_12"), Some("123456_12".to_string()));
        assert_eq!(segment_key("143022_60"), Some("143022_60".to_string()));
        assert_eq!(segment_key("123456_3000"), Some("123456_3000".to_string()));
        assert_eq!(
            segment_key("143022_300_summary.txt"),
            Some("143022_300".to_string())
        );
        assert_eq!(
            segment_key("/journal/20250109/143022_300/audio.jsonl"),
            Some("143022_300".to_string())
        );
        assert_eq!(
            segment_key("prefix...123456_300..."),
            Some("123456_300".to_string())
        );
        assert_eq!(segment_key("coding_093000_300"), None);
        assert_eq!(segment_key("abc123456_300"), None);
        assert_eq!(segment_key("invalid"), None);
    }

    #[test]
    fn segment_parse_validates_clock_ranges() {
        assert_eq!(
            segment_parse("20240101/default/123456_300/talents/audio.md"),
            Some(SegmentTimes {
                hour: 12,
                minute: 34,
                second: 56,
            })
        );
        assert_eq!(
            segment_parse("20240102/default/234567_300/talents/audio.md"),
            None
        );
        assert_eq!(
            segment_parse("facets/work/activities/20260214/coding_093000_300/x.md"),
            None
        );
        assert_eq!(
            segment_parse("20240101/default/143022_300_summary.txt/talents/audio.md"),
            None
        );
    }

    #[test]
    fn time_bucket_matches_python_ranges() {
        assert_eq!(
            time_bucket("20260304/default/090000_300/talents/audio.md"),
            "morning"
        );
        assert_eq!(
            time_bucket("20260304/default/140000_300/talents/audio.md"),
            "afternoon"
        );
        assert_eq!(
            time_bucket("20240101/default/143022_60/talents/audio.md"),
            "afternoon"
        );
        assert_eq!(
            time_bucket("20260304/default/180000_300/talents/audio.md"),
            "evening"
        );
        assert_eq!(
            time_bucket("20260305/default/220000_300/talents/audio.md"),
            "night"
        );
        assert_eq!(
            time_bucket("20240102/default/234567_300/talents/audio.md"),
            ""
        );
        assert_eq!(
            time_bucket("facets/work/activities/20260214/coding_093000_300/x.md"),
            ""
        );
        assert_eq!(
            time_bucket("20240101/default/143022_300_summary.txt/talents/audio.md"),
            ""
        );
    }

    #[test]
    fn historical_day_uses_injected_today() {
        assert!(is_historical_day("20240101/talents/flow.md", "20240102"));
        assert!(!is_historical_day("20240102/talents/flow.md", "20240102"));
        assert!(!is_historical_day(
            "facets/work/news/20240101.md",
            "20240102"
        ));
        assert!(!is_historical_day("20240101", "20240102"));
    }
}
