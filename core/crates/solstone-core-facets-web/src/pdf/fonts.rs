// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

// Adobe Core 14 AFM provenance: Adobe's Times and Courier AFM metrics, with
// the PDF WinAnsiEncoding glyph names. The following APAFML notice is
// reproduced verbatim as required by its distribution terms.
//
// Copyright (c) 1985, 1987, 1989, 1990, 1991, 1992, 1993, 1997 Adobe Systems Incorporated. All Rights Reserved.
//
// This file and the 14 PostScript(R) AFM files it accompanies may be used, copied, and distributed for any purpose and without charge, with or without modification, provided that all copyright notices are retained; that the AFM files are not distributed without this file; that all modifications to this file or any of the AFM files are prominently noted in the modified file(s); and that this paragraph is not modified. Adobe Systems has no responsibility or obligation to support the use of the AFM files.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Face {
    TimesRoman,
    TimesBold,
    TimesItalic,
    TimesBoldItalic,
    Courier,
}

impl Face {
    pub(crate) const fn resource_name(self) -> &'static str {
        match self {
            Self::TimesRoman => "F1",
            Self::TimesBold => "F2",
            Self::TimesItalic => "F3",
            Self::TimesBoldItalic => "F4",
            Self::Courier => "F5",
        }
    }
}

/// Encodes one Unicode scalar as PDF WinAnsi bytes. The fallback is visible
/// ASCII so it can be measured and emitted by this same single encoding step.
pub(crate) fn encode_winansi(value: &str) -> Vec<u8> {
    let mut encoded = Vec::with_capacity(value.len());
    for character in value.chars() {
        if let Some(byte) = winansi_byte(character) {
            encoded.push(byte);
        } else {
            encoded.extend_from_slice(b"[?]");
        }
    }
    encoded
}

/// The code-point -> WinAnsi byte/glyph map used by the writer. In particular,
/// U+00B7 is periodcentered at 0xB7; 0x95 is the distinct bullet glyph.
pub(crate) fn winansi_byte(character: char) -> Option<u8> {
    let codepoint = character as u32;
    match codepoint {
        0x20..=0x7e | 0xa0..=0xff => Some(codepoint as u8),
        0x20ac => Some(0x80),
        0x201a => Some(0x82),
        0x0192 => Some(0x83),
        0x201e => Some(0x84),
        0x2026 => Some(0x85),
        0x2020 => Some(0x86),
        0x2021 => Some(0x87),
        0x02c6 => Some(0x88),
        0x2030 => Some(0x89),
        0x0160 => Some(0x8a),
        0x2039 => Some(0x8b),
        0x0152 => Some(0x8c),
        0x017d => Some(0x8e),
        0x2018 => Some(0x91),
        0x2019 => Some(0x92),
        0x201c => Some(0x93),
        0x201d => Some(0x94),
        0x2022 => Some(0x95),
        0x2013 => Some(0x96),
        0x2014 => Some(0x97),
        0x02dc => Some(0x98),
        0x2122 => Some(0x99),
        0x0161 => Some(0x9a),
        0x203a => Some(0x9b),
        0x0153 => Some(0x9c),
        0x017e => Some(0x9e),
        0x0178 => Some(0x9f),
        _ => None,
    }
}

/// WinAnsi glyph names for the non-identity part of the mapping. ASCII and
/// Latin-1 retain their Adobe glyph names from the AFM tables; these entries
/// cover every code point whose WinAnsi byte differs from its Unicode value.
pub(crate) fn winansi_glyph_name(byte: u8) -> Option<&'static str> {
    match byte {
        0x80 => Some("Euro"),
        0x82 => Some("quotesinglbase"),
        0x83 => Some("florin"),
        0x84 => Some("quotedblbase"),
        0x85 => Some("ellipsis"),
        0x86 => Some("dagger"),
        0x87 => Some("daggerdbl"),
        0x88 => Some("circumflex"),
        0x89 => Some("perthousand"),
        0x8a => Some("Scaron"),
        0x8b => Some("guilsinglleft"),
        0x8c => Some("OE"),
        0x8e => Some("Zcaron"),
        0x91 => Some("quoteleft"),
        0x92 => Some("quoteright"),
        0x93 => Some("quotedblleft"),
        0x94 => Some("quotedblright"),
        0x95 => Some("bullet"),
        0x96 => Some("endash"),
        0x97 => Some("emdash"),
        0x98 => Some("tilde"),
        0x99 => Some("trademark"),
        0x9a => Some("scaron"),
        0x9b => Some("guilsinglright"),
        0x9c => Some("oe"),
        0x9e => Some("zcaron"),
        0x9f => Some("Ydieresis"),
        0xb7 => Some("periodcentered"),
        _ => None,
    }
}

/// AFM advance widths in 1/1000 em. Courier is fixed pitch. For the serif
/// faces, the common ASCII classes and WinAnsi punctuation retain their Core
/// 14 AFM widths; the remaining Latin glyphs use the AFM letter-width class.
pub(crate) fn advance_width(face: Face, byte: u8) -> u16 {
    if face == Face::Courier {
        return 600;
    }
    let bold = matches!(face, Face::TimesBold | Face::TimesBoldItalic);
    let italic = matches!(face, Face::TimesItalic | Face::TimesBoldItalic);
    match byte {
        b' ' => 250,
        b'!' | b'\'' | b',' | b'.' | b':' | b';' => {
            if bold {
                333
            } else {
                250
            }
        }
        b'"' => {
            if bold {
                555
            } else if italic {
                420
            } else {
                408
            }
        }
        b'-' | b'(' | b')' | b'[' | b']' => 333,
        b'0'..=b'9' => 500,
        b'I' => {
            if bold {
                389
            } else {
                333
            }
        }
        b'J' => {
            if bold {
                500
            } else {
                389
            }
        }
        b'M' => {
            if bold {
                944
            } else if italic {
                833
            } else {
                889
            }
        }
        b'W' => {
            if bold {
                1000
            } else if italic {
                833
            } else {
                944
            }
        }
        b'i' | b'l' => 278,
        b'm' => {
            if bold {
                833
            } else if italic {
                722
            } else {
                778
            }
        }
        b'w' => {
            if italic && !bold {
                667
            } else {
                722
            }
        }
        b'r' => {
            if bold {
                444
            } else if italic {
                389
            } else {
                333
            }
        }
        0x91 | 0x92 => 333,
        0x93 | 0x94 => {
            if bold {
                500
            } else {
                444
            }
        }
        0x95 => 350,
        0x96 => 500,
        0x97 => 1000,
        0xb7 => 250,
        _ if byte.is_ascii_uppercase() => 667,
        _ if byte.is_ascii_lowercase() => 500,
        _ => 500,
    }
}

#[cfg(test)]
mod tests {
    use super::{Face, advance_width, encode_winansi, winansi_byte, winansi_glyph_name};

    #[test]
    fn periodcentered_is_not_bullet() {
        assert_eq!(winansi_byte('·'), Some(0xb7));
        assert_eq!(winansi_glyph_name(0xb7), Some("periodcentered"));
        assert_eq!(winansi_glyph_name(0x95), Some("bullet"));
        assert_eq!(advance_width(Face::TimesRoman, 0xb7), 250);
        assert_eq!(encode_winansi("A😀B"), b"A[?]B");
    }
}
