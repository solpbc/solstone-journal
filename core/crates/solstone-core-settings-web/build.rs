// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

fn assignment_end(source: &str, start: usize) -> usize {
    let bytes = source.as_bytes();
    let mut depth = 0_usize;
    let mut quote = None;
    let mut escaped = false;
    for (offset, byte) in bytes[start..].iter().copied().enumerate() {
        if let Some(delimiter) = quote {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == delimiter {
                quote = None;
            }
            continue;
        }
        match byte {
            b'\'' | b'"' => quote = Some(byte),
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth = depth.saturating_sub(1),
            b'\n' if depth == 0 => return start + offset,
            _ => {}
        }
    }
    source.len()
}

fn assignments(source: &str) -> BTreeMap<String, String> {
    let mut values = BTreeMap::new();
    let mut offset = 0;
    while offset < source.len() {
        let line_end = source[offset..]
            .find('\n')
            .map_or(source.len(), |index| offset + index);
        let line = &source[offset..line_end];
        if !line.chars().next().is_some_and(char::is_whitespace)
            && let Some((name, expression)) = line.split_once('=')
        {
            let name = name.split(':').next().expect("assignment name").trim();
            if name
                .chars()
                .all(|character| character == '_' || character.is_ascii_alphanumeric())
                && !name.is_empty()
            {
                let expression_start = offset + line.find('=').expect("assignment delimiter") + 1;
                let end = assignment_end(source, expression_start);
                values.insert(name.to_owned(), source[expression_start..end].to_owned());
                offset = end.saturating_add(1);
                continue;
            }
            let _ = expression;
        }
        offset = line_end.saturating_add(1);
    }
    values
}

struct Literal<'a> {
    source: &'a [u8],
    index: usize,
}

impl<'a> Literal<'a> {
    fn parse(expression: &'a str) -> Value {
        let mut parser = Self {
            source: expression.as_bytes(),
            index: 0,
        };
        let value = parser.value();
        parser.whitespace();
        assert_eq!(
            parser.index,
            parser.source.len(),
            "unsupported Python literal tail"
        );
        value
    }

    fn whitespace(&mut self) {
        while self
            .source
            .get(self.index)
            .is_some_and(|byte| byte.is_ascii_whitespace())
        {
            self.index += 1;
        }
    }

    fn value(&mut self) -> Value {
        self.whitespace();
        match self.source.get(self.index).copied() {
            Some(b'\'') | Some(b'"') => Value::String(self.string()),
            Some(b'[') => self.sequence(b'[', b']'),
            Some(b'(') => self.sequence(b'(', b')'),
            Some(b'{') => self.dictionary(),
            Some(b'T') if self.consume_word(b"True") => Value::Bool(true),
            Some(b'F') if self.consume_word(b"False") => Value::Bool(false),
            Some(b'N') if self.consume_word(b"None") => Value::Null,
            Some(byte) if byte.is_ascii_digit() || byte == b'-' => self.number(),
            _ => panic!("unsupported Python literal"),
        }
    }

    fn consume_word(&mut self, word: &[u8]) -> bool {
        if self.source.get(self.index..self.index + word.len()) == Some(word) {
            self.index += word.len();
            true
        } else {
            false
        }
    }

    fn string(&mut self) -> String {
        let quote = self.source[self.index];
        self.index += 1;
        let mut output = Vec::new();
        while let Some(byte) = self.source.get(self.index).copied() {
            self.index += 1;
            if byte == quote {
                self.whitespace();
                if self
                    .source
                    .get(self.index)
                    .is_some_and(|next| *next == quote)
                {
                    output.extend(self.string().bytes());
                }
                return String::from_utf8(output).expect("Python string UTF-8");
            }
            if byte == b'\\' {
                let escaped = self
                    .source
                    .get(self.index)
                    .copied()
                    .expect("terminated escape");
                self.index += 1;
                output.push(match escaped {
                    b'\\' => b'\\',
                    b'\'' => b'\'',
                    b'"' => b'"',
                    b'n' => b'\n',
                    b't' => b'\t',
                    other => panic!("unsupported Python string escape: {}", other as char),
                });
            } else {
                output.push(byte);
            }
        }
        panic!("unterminated Python string")
    }

    fn sequence(&mut self, open: u8, close: u8) -> Value {
        assert_eq!(self.source[self.index], open, "sequence opener");
        self.index += 1;
        let mut values = Vec::new();
        let mut comma = false;
        loop {
            self.whitespace();
            if self.source.get(self.index) == Some(&close) {
                self.index += 1;
                if open == b'(' && !comma && values.len() == 1 {
                    return values.pop().expect("grouped literal");
                }
                return Value::Array(values);
            }
            values.push(self.value());
            self.whitespace();
            if self.source.get(self.index) == Some(&b',') {
                self.index += 1;
                comma = true;
            } else {
                assert_eq!(
                    self.source.get(self.index),
                    Some(&close),
                    "sequence delimiter"
                );
            }
        }
    }

    fn dictionary(&mut self) -> Value {
        assert_eq!(self.source[self.index], b'{', "dictionary opener");
        self.index += 1;
        let mut values = serde_json::Map::new();
        loop {
            self.whitespace();
            if self.source.get(self.index) == Some(&b'}') {
                self.index += 1;
                return Value::Object(values);
            }
            let key = match self.value() {
                Value::String(value) => value,
                _ => panic!("Python dict key is a string"),
            };
            self.whitespace();
            assert_eq!(self.source.get(self.index), Some(&b':'), "dict colon");
            self.index += 1;
            values.insert(key, self.value());
            self.whitespace();
            if self.source.get(self.index) == Some(&b',') {
                self.index += 1;
            } else {
                assert_eq!(self.source.get(self.index), Some(&b'}'), "dict delimiter");
            }
        }
    }

    fn number(&mut self) -> Value {
        let start = self.index;
        while self.source.get(self.index).is_some_and(|byte| {
            byte.is_ascii_digit() || matches!(*byte, b'-' | b'+' | b'.' | b'e' | b'E')
        }) {
            self.index += 1;
        }
        serde_json::from_str(
            std::str::from_utf8(&self.source[start..self.index]).expect("number UTF-8"),
        )
        .expect("Python numeric literal")
    }
}

fn exported_constants(source: &str, use_all: bool) -> serde_json::Map<String, Value> {
    let assignments = assignments(source);
    let names: Vec<String> = if use_all {
        match Literal::parse(
            assignments
                .get("__all__")
                .expect("copy module defines __all__"),
        ) {
            Value::Array(values) => values
                .into_iter()
                .map(|value| value.as_str().expect("__all__ item is a string").to_owned())
                .collect(),
            _ => panic!("__all__ is a list or tuple of strings"),
        }
    } else {
        assignments
            .keys()
            .filter(|name| {
                name.chars()
                    .all(|character| !character.is_ascii_lowercase())
                    && !name.starts_with('_')
            })
            .cloned()
            .collect()
    };
    names
        .into_iter()
        .map(|name| {
            let expression = assignments
                .get(&name)
                .expect("exported copy constant exists");
            (name, Literal::parse(expression))
        })
        .collect()
}

fn write_constants(path: &Path, constants: &serde_json::Map<String, Value>) {
    let json = serde_json::to_string(constants).expect("copy constants serialize");
    fs::write(
        path,
        format!("pub const COPY_JSON: &str = r##\"{json}\"##;\n"),
    )
    .expect("generated copy constants write");
}

fn write_value(path: &Path, value: &Value) {
    let json = serde_json::to_string(value).expect("literal serializes");
    fs::write(path, format!("pub const JSON: &str = r##\"{json}\"##;\n"))
        .expect("generated literal write");
}

fn main() {
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest directory"));
    let modules = [
        (manifest.join("assets/copy.py"), true, "settings_copy.rs"),
        (
            manifest.join("assets/install_copy.py"),
            true,
            "install_copy.rs",
        ),
        (manifest.join("assets/chat_copy.py"), false, "chat_copy.rs"),
        (
            manifest.join("assets/sol_initiated_copy.py"),
            false,
            "sol_voice_copy.rs",
        ),
    ];
    let output = PathBuf::from(env::var_os("OUT_DIR").expect("output directory"));
    let mut source_names = fs::read_dir(manifest.join("src"))
        .expect("settings source directory")
        .filter_map(Result::ok)
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| name.ends_with(".rs"))
        .collect::<Vec<_>>();
    source_names.sort();
    fs::write(
        output.join("settings_sources.rs"),
        format!("pub const SOURCES: &[&str] = &{:?};\n", source_names),
    )
    .expect("settings source manifest");
    println!("cargo:rerun-if-changed={}", manifest.join("src").display());
    for (path, use_all, generated) in modules {
        println!("cargo:rerun-if-changed={}", path.display());
        let source = fs::read_to_string(&path).expect("copy module is readable");
        write_constants(
            &output.join(generated),
            &exported_constants(&source, use_all),
        );
    }
    let backup_copy = manifest.join("assets/backup_copy.py");
    println!("cargo:rerun-if-changed={}", backup_copy.display());
    let backup_assignments =
        assignments(&fs::read_to_string(&backup_copy).expect("backup copy module is readable"));
    let backup_constants = ["OFFLOAD_STALLED_LEAD", "OFFLOAD_STALL_REASON_LABELS"]
        .into_iter()
        .map(|name| {
            (
                name.to_owned(),
                Literal::parse(
                    backup_assignments
                        .get(name)
                        .expect("backup copy constant exists"),
                ),
            )
        })
        .collect();
    write_constants(&output.join("backup_copy.rs"), &backup_constants);
    let activities = manifest.join("assets/activities.py");
    println!("cargo:rerun-if-changed={}", activities.display());
    let activities_source = fs::read_to_string(&activities).expect("activities module is readable");
    write_value(
        &output.join("default_activities.rs"),
        &Literal::parse(
            assignments(&activities_source)
                .get("DEFAULT_ACTIVITIES")
                .expect("activities module defines DEFAULT_ACTIVITIES"),
        ),
    );
    for path in [
        manifest.join("assets/workspace.html"),
        manifest.join("assets/settings.js"),
        manifest.join("assets/copy.py"),
    ] {
        println!("cargo:rerun-if-changed={}", path.display());
    }
}
