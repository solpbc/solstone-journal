// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

/// The original terminator following a row's content.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Terminator {
    Lf,
    Crlf,
    Cr,
    None,
}

impl Terminator {
    pub(crate) const fn bytes(self) -> &'static [u8] {
        match self {
            Self::Lf => b"\n",
            Self::Crlf => b"\r\n",
            Self::Cr => b"\r",
            Self::None => b"",
        }
    }
}

/// A borrowed physical row. Content deliberately excludes its terminator.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Row<'a> {
    pub content: &'a [u8],
    pub terminator: Terminator,
}

/// Splits raw bytes without assuming UTF-8 or normalizing line terminators.
pub struct RowSplitter<'a> {
    input: &'a [u8],
    offset: usize,
}

impl<'a> RowSplitter<'a> {
    pub const fn new(input: &'a [u8]) -> Self {
        Self { input, offset: 0 }
    }
}

impl<'a> Iterator for RowSplitter<'a> {
    type Item = Row<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.offset >= self.input.len() {
            return None;
        }
        let start = self.offset;
        let mut cursor = start;
        while cursor < self.input.len() {
            match self.input[cursor] {
                b'\n' => {
                    self.offset = cursor + 1;
                    return Some(Row {
                        content: &self.input[start..cursor],
                        terminator: Terminator::Lf,
                    });
                }
                b'\r' if self.input.get(cursor + 1) == Some(&b'\n') => {
                    self.offset = cursor + 2;
                    return Some(Row {
                        content: &self.input[start..cursor],
                        terminator: Terminator::Crlf,
                    });
                }
                b'\r' => {
                    self.offset = cursor + 1;
                    return Some(Row {
                        content: &self.input[start..cursor],
                        terminator: Terminator::Cr,
                    });
                }
                _ => cursor += 1,
            }
        }
        self.offset = self.input.len();
        Some(Row {
            content: &self.input[start..],
            terminator: Terminator::None,
        })
    }
}
