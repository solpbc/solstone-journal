// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::env;
use std::fs;
use std::path::PathBuf;

use serde_json::Value;
use std::collections::BTreeMap;

// Copied from settings-web's build-time Python-literal reader. Keeping the
// same parser makes multi-line strings, collections, and escapes fail or parse
// consistently across native web crates.
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
        let end = source[offset..]
            .find('\n')
            .map_or(source.len(), |index| offset + index);
        let line = &source[offset..end];
        if !line.chars().next().is_some_and(char::is_whitespace)
            && let Some((name, _)) = line.split_once('=')
        {
            let name = name.split(':').next().expect("assignment name").trim();
            if !name.is_empty()
                && name
                    .chars()
                    .all(|value| value == '_' || value.is_ascii_alphanumeric())
            {
                let start = offset + line.find('=').expect("assignment delimiter") + 1;
                let assignment_end = assignment_end(source, start);
                values.insert(name.to_owned(), source[start..assignment_end].to_owned());
                offset = assignment_end.saturating_add(1);
                continue;
            }
        }
        offset = end.saturating_add(1);
    }
    values
}

struct Literal<'a> {
    source: &'a [u8],
    index: usize,
}
impl<'a> Literal<'a> {
    fn parse(source: &'a str) -> Value {
        let mut parser = Self {
            source: source.as_bytes(),
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
            Some(b'T') if self.word(b"True") => Value::Bool(true),
            Some(b'F') if self.word(b"False") => Value::Bool(false),
            Some(b'N') if self.word(b"None") => Value::Null,
            Some(byte) if byte.is_ascii_digit() || byte == b'-' => self.number(),
            _ => panic!("unsupported Python literal"),
        }
    }
    fn word(&mut self, word: &[u8]) -> bool {
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
        assert_eq!(self.source[self.index], open);
        self.index += 1;
        let mut values = Vec::new();
        let mut comma = false;
        loop {
            self.whitespace();
            if self.source.get(self.index) == Some(&close) {
                self.index += 1;
                return if open == b'(' && !comma && values.len() == 1 {
                    values.pop().unwrap()
                } else {
                    Value::Array(values)
                };
            }
            values.push(self.value());
            self.whitespace();
            if self.source.get(self.index) == Some(&b',') {
                self.index += 1;
                comma = true;
            } else {
                assert_eq!(self.source.get(self.index), Some(&close));
            }
        }
    }
    fn dictionary(&mut self) -> Value {
        assert_eq!(self.source[self.index], b'{');
        self.index += 1;
        let mut values = serde_json::Map::new();
        loop {
            self.whitespace();
            if self.source.get(self.index) == Some(&b'}') {
                self.index += 1;
                return Value::Object(values);
            }
            let key = self
                .value()
                .as_str()
                .expect("Python dict key is a string")
                .to_owned();
            self.whitespace();
            assert_eq!(self.source.get(self.index), Some(&b':'));
            self.index += 1;
            values.insert(key, self.value());
            self.whitespace();
            if self.source.get(self.index) == Some(&b',') {
                self.index += 1;
            } else {
                assert_eq!(self.source.get(self.index), Some(&b'}'));
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
        serde_json::from_str(std::str::from_utf8(&self.source[start..self.index]).unwrap())
            .expect("Python numeric literal")
    }
}

fn main() {
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let root = manifest
        .join("../../..")
        .canonicalize()
        .expect("repository root");
    let workspace = manifest.join("assets/transcripts/workspace.html");
    let copy = root.join("solstone/apps/transcripts/copy.py");
    println!("cargo:rerun-if-changed={}", workspace.display());
    println!("cargo:rerun-if-changed={}", copy.display());

    let source = fs::read_to_string(&copy).expect("transcripts copy is readable");
    let values = assignments(&source)
        .into_iter()
        .filter(|(name, _)| name.starts_with("TR_"))
        .map(|(name, expression)| (name, Literal::parse(&expression)))
        .collect::<serde_json::Map<_, _>>();
    let copy_json = serde_json::to_string(&values).expect("copy JSON serializes");
    let generated = format!(
        "const WORKSPACE: EmbeddedAsset = EmbeddedAsset {{ content_type: \"text/html; charset=utf-8\", bytes: include_bytes!({:?}) }};\nconst TRANSCRIPTS_COPY_JSON: &str = {copy_json:?};\n",
        workspace,
    );
    fs::write(
        PathBuf::from(env::var_os("OUT_DIR").expect("output dir")).join("transcripts_assets.rs"),
        generated,
    )
    .expect("generated assets write");
}
