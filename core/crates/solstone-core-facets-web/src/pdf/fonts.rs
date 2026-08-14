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

/// AFM advance widths in 1/1000 em for every WinAnsi byte emitted by
/// `encode_winansi`. Courier's single 600-unit width is its actual fixed-pitch
/// AFM metric, not a fallback class.
pub(crate) fn advance_width(face: Face, byte: u8) -> u16 {
    match byte {
        _ if face == Face::Courier => 600,
        _ => match face {
            Face::TimesRoman => TIMES_ROMAN_WIDTHS[byte as usize],
            Face::TimesBold => TIMES_BOLD_WIDTHS[byte as usize],
            Face::TimesItalic => TIMES_ITALIC_WIDTHS[byte as usize],
            Face::TimesBoldItalic => TIMES_BOLD_ITALIC_WIDTHS[byte as usize],
            Face::Courier => unreachable!("Courier handled above"),
        },
    }
}

const TIMES_ROMAN_WIDTHS: [u16; 256] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    250, 333, 408, 500, 500, 833, 778, 333, 333, 333, 500, 564, 250, 333, 250, 278, 500, 500, 500,
    500, 500, 500, 500, 500, 500, 500, 278, 278, 564, 564, 564, 444, 921, 722, 667, 667, 722, 611,
    556, 722, 722, 333, 389, 722, 611, 889, 722, 722, 556, 722, 667, 556, 611, 722, 722, 944, 722,
    722, 611, 333, 278, 333, 469, 500, 333, 444, 500, 444, 500, 444, 333, 500, 500, 278, 278, 500,
    278, 778, 500, 500, 500, 500, 333, 389, 278, 500, 500, 722, 500, 500, 444, 480, 200, 480, 541,
    0, 500, 0, 333, 500, 444, 1000, 500, 500, 333, 1000, 556, 333, 889, 0, 611, 0, 0, 333, 333,
    444, 444, 350, 500, 1000, 333, 980, 389, 333, 722, 0, 444, 722, 250, 333, 500, 500, 500, 500,
    200, 500, 333, 760, 276, 500, 564, 333, 760, 333, 400, 564, 300, 300, 333, 500, 453, 250, 333,
    300, 310, 500, 750, 750, 750, 444, 722, 722, 722, 722, 722, 722, 889, 667, 611, 611, 611, 611,
    333, 333, 333, 333, 722, 722, 722, 722, 722, 722, 722, 564, 722, 722, 722, 722, 722, 722, 556,
    500, 444, 444, 444, 444, 444, 444, 667, 444, 444, 444, 444, 444, 278, 278, 278, 278, 500, 500,
    500, 500, 500, 500, 500, 564, 500, 500, 500, 500, 500, 500, 500, 500,
];

const TIMES_BOLD_WIDTHS: [u16; 256] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    250, 333, 555, 500, 500, 1000, 833, 333, 333, 333, 500, 570, 250, 333, 250, 278, 500, 500, 500,
    500, 500, 500, 500, 500, 500, 500, 333, 333, 570, 570, 570, 500, 930, 722, 667, 722, 722, 667,
    611, 778, 778, 389, 500, 778, 667, 944, 722, 778, 611, 778, 722, 556, 667, 722, 722, 1000, 722,
    722, 667, 333, 278, 333, 581, 500, 333, 500, 556, 444, 556, 444, 333, 500, 556, 278, 333, 556,
    278, 833, 556, 500, 556, 556, 444, 389, 333, 556, 500, 722, 500, 500, 444, 394, 220, 394, 520,
    0, 500, 0, 333, 500, 500, 1000, 500, 500, 333, 1000, 556, 333, 1000, 0, 667, 0, 0, 333, 333,
    500, 500, 350, 500, 1000, 333, 1000, 389, 333, 722, 0, 444, 722, 250, 333, 500, 500, 500, 500,
    220, 500, 333, 747, 300, 500, 570, 333, 747, 333, 400, 570, 300, 300, 333, 556, 540, 250, 333,
    300, 330, 500, 750, 750, 750, 500, 722, 722, 722, 722, 722, 722, 1000, 722, 667, 667, 667, 667,
    389, 389, 389, 389, 722, 722, 778, 778, 778, 778, 778, 570, 778, 722, 722, 722, 722, 722, 611,
    556, 500, 500, 500, 500, 500, 500, 722, 444, 444, 444, 444, 444, 278, 278, 278, 278, 500, 556,
    500, 500, 500, 500, 500, 570, 500, 556, 556, 556, 556, 500, 556, 500,
];

const TIMES_ITALIC_WIDTHS: [u16; 256] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    250, 333, 420, 500, 500, 833, 778, 333, 333, 333, 500, 675, 250, 333, 250, 278, 500, 500, 500,
    500, 500, 500, 500, 500, 500, 500, 333, 333, 675, 675, 675, 500, 920, 611, 611, 667, 722, 611,
    611, 722, 722, 333, 444, 667, 556, 833, 667, 722, 611, 722, 611, 500, 556, 722, 611, 833, 611,
    556, 556, 389, 278, 389, 422, 500, 333, 500, 500, 444, 500, 444, 278, 500, 500, 278, 278, 444,
    278, 722, 500, 500, 500, 500, 389, 389, 278, 500, 444, 667, 444, 444, 389, 400, 275, 400, 541,
    0, 500, 0, 333, 500, 556, 889, 500, 500, 333, 1000, 500, 333, 944, 0, 556, 0, 0, 333, 333, 556,
    556, 350, 500, 889, 333, 980, 389, 333, 667, 0, 389, 556, 250, 389, 500, 500, 500, 500, 275,
    500, 333, 760, 276, 500, 675, 333, 760, 333, 400, 675, 300, 300, 333, 500, 523, 250, 333, 300,
    310, 500, 750, 750, 750, 500, 611, 611, 611, 611, 611, 611, 889, 667, 611, 611, 611, 611, 333,
    333, 333, 333, 722, 667, 722, 722, 722, 722, 722, 675, 722, 722, 722, 722, 722, 556, 611, 500,
    500, 500, 500, 500, 500, 500, 667, 444, 444, 444, 444, 444, 278, 278, 278, 278, 500, 500, 500,
    500, 500, 500, 500, 675, 500, 500, 500, 500, 500, 444, 500, 444,
];

const TIMES_BOLD_ITALIC_WIDTHS: [u16; 256] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    250, 389, 555, 500, 500, 833, 778, 333, 333, 333, 500, 570, 250, 333, 250, 278, 500, 500, 500,
    500, 500, 500, 500, 500, 500, 500, 333, 333, 570, 570, 570, 500, 832, 667, 667, 667, 722, 667,
    667, 722, 778, 389, 500, 667, 611, 889, 722, 722, 611, 722, 667, 556, 611, 722, 667, 889, 667,
    611, 611, 333, 278, 333, 570, 500, 333, 500, 500, 444, 500, 444, 333, 500, 556, 278, 278, 500,
    278, 778, 556, 500, 500, 500, 389, 389, 278, 556, 444, 667, 500, 444, 389, 348, 220, 348, 570,
    0, 500, 0, 333, 500, 500, 1000, 500, 500, 333, 1000, 556, 333, 944, 0, 611, 0, 0, 333, 333,
    500, 500, 350, 500, 1000, 333, 1000, 389, 333, 722, 0, 389, 611, 250, 389, 500, 500, 500, 500,
    220, 500, 333, 747, 266, 500, 606, 333, 747, 333, 400, 570, 300, 300, 333, 576, 500, 250, 333,
    300, 300, 500, 750, 750, 750, 500, 667, 667, 667, 667, 667, 667, 944, 667, 667, 667, 667, 667,
    389, 389, 389, 389, 722, 722, 722, 722, 722, 722, 722, 570, 722, 722, 722, 722, 722, 611, 611,
    500, 500, 500, 500, 500, 500, 500, 722, 444, 444, 444, 444, 444, 278, 278, 278, 278, 500, 556,
    500, 500, 500, 500, 500, 570, 500, 556, 556, 556, 556, 444, 500, 444,
];

#[cfg(test)]
mod tests {
    use super::{Face, advance_width, encode_winansi, winansi_byte, winansi_glyph_name};

    #[test]
    fn periodcentered_is_not_bullet() {
        assert_eq!(winansi_byte('·'), Some(0xb7));
        assert_eq!(winansi_glyph_name(0xb7), Some("periodcentered"));
        assert_eq!(winansi_glyph_name(0x95), Some("bullet"));
        assert_eq!(advance_width(Face::TimesRoman, 0xb7), 250);
        assert_eq!(advance_width(Face::TimesRoman, b'a'), 444);
        assert_eq!(advance_width(Face::TimesBold, b'a'), 500);
        assert_eq!(advance_width(Face::TimesItalic, b'a'), 500);
        assert_eq!(advance_width(Face::TimesBoldItalic, b'a'), 500);
        assert_eq!(advance_width(Face::Courier, 0x97), 600);
        assert_eq!(encode_winansi("A😀B"), b"A[?]B");
    }
}
