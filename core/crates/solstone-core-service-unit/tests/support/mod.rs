// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![allow(dead_code)] // Shared support is intentionally used by different integration binaries.

use std::collections::BTreeMap;

use plist::Value;

#[derive(Debug, PartialEq, Eq)]
pub struct ParsedUnit {
    pub exec_start: Vec<String>,
    pub environment: BTreeMap<String, String>,
    pub has_legacy_log_directives: bool,
}

pub fn parse_plist(bytes: &[u8]) -> Value {
    Value::from_reader_xml(bytes).expect("rendered plist parses")
}

pub fn parse_unit(unit: &str) -> ParsedUnit {
    let exec_start = unit
        .lines()
        .find_map(|line| line.strip_prefix("ExecStart="))
        .map(parse_exec_start)
        .expect("unit has ExecStart");
    let environment = unit
        .lines()
        .filter_map(|line| line.strip_prefix("Environment="))
        .map(parse_environment)
        .collect();
    ParsedUnit {
        exec_start,
        environment,
        has_legacy_log_directives: unit
            .lines()
            .any(|line| line.starts_with("StandardOutput=") || line.starts_with("StandardError=")),
    }
}

fn parse_exec_start(value: &str) -> Vec<String> {
    lex(value)
        .expect("ExecStart lexes")
        .into_iter()
        .map(|token| collapse_exec_expansions(&token))
        .collect()
}

fn parse_environment(value: &str) -> (String, String) {
    let assignments = lex(value).expect("Environment lexes");
    assert_eq!(assignments.len(), 1, "one assignment per Environment line");
    let (key, value) = assignments[0]
        .split_once('=')
        .expect("Environment assignment has equals");
    (key.to_owned(), collapse_percent_specifiers(value))
}

fn lex(value: &str) -> Result<Vec<String>, String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    let mut characters = value.chars().peekable();
    while let Some(character) = characters.next() {
        match quote {
            None if character.is_whitespace() => {
                if !current.is_empty() {
                    words.push(std::mem::take(&mut current));
                }
            }
            None if matches!(character, '\'' | '"') => quote = Some(character),
            Some(delimiter) if character == delimiter => quote = None,
            _ if character == '\\' => current.push(decode_escape(&mut characters)?),
            _ => current.push(character),
        }
    }
    if quote.is_some() {
        return Err("unterminated quote".to_owned());
    }
    if !current.is_empty() {
        words.push(current);
    }
    Ok(words)
}

fn decode_escape(
    characters: &mut std::iter::Peekable<std::str::Chars<'_>>,
) -> Result<char, String> {
    let escape = characters
        .next()
        .ok_or_else(|| "trailing escape".to_owned())?;
    match escape {
        '\\' => Ok('\\'),
        '"' => Ok('"'),
        'n' => Ok('\n'),
        'r' => Ok('\r'),
        't' => Ok('\t'),
        'x' => decode_hex(characters, 2),
        'u' => decode_hex(characters, 4),
        _ => Err(format!("unsupported escape \\{escape}")),
    }
}

fn decode_hex(
    characters: &mut std::iter::Peekable<std::str::Chars<'_>>,
    width: usize,
) -> Result<char, String> {
    let mut value = 0_u32;
    for _ in 0..width {
        let digit = characters
            .next()
            .and_then(|character| character.to_digit(16))
            .ok_or_else(|| "invalid hexadecimal escape".to_owned())?;
        value = (value << 4) | digit;
    }
    char::from_u32(value).ok_or_else(|| "invalid Unicode escape".to_owned())
}

fn collapse_exec_expansions(value: &str) -> String {
    collapse_pairs(&collapse_pairs(value, '$'), '%')
}

fn collapse_percent_specifiers(value: &str) -> String {
    collapse_pairs(value, '%')
}

fn collapse_pairs(value: &str, marker: char) -> String {
    let mut output = String::with_capacity(value.len());
    let mut characters = value.chars().peekable();
    while let Some(character) = characters.next() {
        if character == marker && characters.peek() == Some(&marker) {
            characters.next();
        }
        output.push(character);
    }
    output
}
