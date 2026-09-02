// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Deterministic journal-mark derivation from a journal ID.

use std::fmt;

use argon2::{Algorithm, Argon2, Error as Argon2Error, Params, Version};
use indexmap::IndexMap;
use serde::Serialize;

const ICON_COUNT: usize = 60;
const COLOR_COUNT: usize = 16;
const WORD_COUNT: usize = 7776;
const MARK_ARGON2_SALT: &[u8] = b"solstone-journal-mark-v1";

const GLYPHS_JSON: &str = include_str!("../assets/mark_assets/glyphs.json");
const COLORS_JSON: &str = include_str!("../assets/mark_assets/colors.json");
const WORDS_JSON: &str = include_str!("../assets/mark_assets/words.json");

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct MarkColor {
    pub name: String,
    pub hex: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct MarkIconSpec {
    pub name: String,
    pub svg: String,
    pub color: MarkColor,
    pub rot: u16,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct MarkRenderSpec {
    pub icon1: MarkIconSpec,
    pub icon2: MarkIconSpec,
    pub words: [String; 2],
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Mark {
    icon1: MarkIconSpec,
    icon2: MarkIconSpec,
    words: [String; 2],
}

#[derive(Debug)]
pub enum MarkError {
    Asset(String),
    Argon2(Argon2Error),
    InvalidJid(String),
}

impl fmt::Display for MarkError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Asset(message) => write!(formatter, "invalid mark asset: {message}"),
            Self::Argon2(error) => error.fmt(formatter),
            Self::InvalidJid(message) => write!(formatter, "invalid journal ID: {message}"),
        }
    }
}

impl std::error::Error for MarkError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Asset(_) | Self::Argon2(_) | Self::InvalidJid(_) => None,
        }
    }
}

impl From<Argon2Error> for MarkError {
    fn from(error: Argon2Error) -> Self {
        Self::Argon2(error)
    }
}

impl Mark {
    pub fn to_render_spec(&self) -> MarkRenderSpec {
        MarkRenderSpec {
            icon1: self.icon1.clone(),
            icon2: self.icon2.clone(),
            words: self.words.clone(),
        }
    }
}

/// Derive the frozen journal mark from a canonical textual UUID journal ID.
pub fn mark_from_jid(jid: &str) -> Result<Mark, MarkError> {
    let assets = MarkAssets::load()?;
    derive_mark(parse_jid_bytes(jid)?, &assets)
}

fn derive_mark(jid: [u8; 16], assets: &MarkAssets) -> Result<Mark, MarkError> {
    let params = Params::new(65_536, 3, 1, Some(32))?;
    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut digest = [0_u8; 32];
    argon2.hash_password_into(&jid, MARK_ARGON2_SALT, &mut digest)?;
    let words = digest_words(&digest);

    let icon1_index = pick(words[0], assets.glyphs.len());
    let icon2_index = pick_distinct(words[1], assets.glyphs.len(), icon1_index);
    let color1_index = pick(words[2], assets.colors.len());
    let color2_index = pick_distinct(words[3], assets.colors.len(), color1_index);
    let word1_index = pick(words[4], assets.words.len());
    let word2_index = pick_distinct(words[5], assets.words.len(), word1_index);

    let (icon1_name, icon1_svg) = &assets.glyphs[icon1_index];
    let (icon2_name, icon2_svg) = &assets.glyphs[icon2_index];
    let color1 = &assets.colors[color1_index];
    let color2 = &assets.colors[color2_index];

    Ok(Mark {
        icon1: MarkIconSpec {
            name: icon1_name.clone(),
            svg: icon1_svg.clone(),
            color: color1.clone(),
            rot: if words[6] & 1 == 0 { 0 } else { 45 },
        },
        icon2: MarkIconSpec {
            name: icon2_name.clone(),
            svg: icon2_svg.clone(),
            color: color2.clone(),
            rot: if words[6] >> 1 & 1 == 0 { 0 } else { 45 },
        },
        words: [
            assets.words[word1_index].clone(),
            assets.words[word2_index].clone(),
        ],
    })
}

fn digest_words(digest: &[u8; 32]) -> [u32; 7] {
    std::array::from_fn(|index| {
        u32::from_be_bytes(digest[index * 4..index * 4 + 4].try_into().unwrap())
    })
}

fn pick(word: u32, count: usize) -> usize {
    word as usize % count
}

fn pick_distinct(word: u32, count: usize, excluded: usize) -> usize {
    let mut index = word as usize % (count - 1);
    if index >= excluded {
        index += 1;
    }
    index
}

fn parse_jid_bytes(jid: &str) -> Result<[u8; 16], MarkError> {
    if jid.len() != 36
        || !matches!(jid.as_bytes().get(8), Some(b'-'))
        || !matches!(jid.as_bytes().get(13), Some(b'-'))
        || !matches!(jid.as_bytes().get(18), Some(b'-'))
        || !matches!(jid.as_bytes().get(23), Some(b'-'))
    {
        return Err(MarkError::InvalidJid(
            "expected a hyphenated 36-character UUID".to_owned(),
        ));
    }

    let mut bytes = [0_u8; 16];
    let mut output = 0;
    let mut input = jid.bytes();
    while let Some(high) = input.next() {
        if high == b'-' {
            continue;
        }
        let low = input.next().ok_or_else(|| {
            MarkError::InvalidJid("expected an even number of hexadecimal digits".to_owned())
        })?;
        bytes[output] = hex_byte(high, low)?;
        output += 1;
    }
    if output != bytes.len() {
        return Err(MarkError::InvalidJid(
            "expected exactly 16 UUID bytes".to_owned(),
        ));
    }
    Ok(bytes)
}

fn hex_byte(high: u8, low: u8) -> Result<u8, MarkError> {
    let nibble = |byte| match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err(MarkError::InvalidJid(
            "UUID contains a non-hexadecimal digit".to_owned(),
        )),
    };
    Ok(nibble(high)? << 4 | nibble(low)?)
}

struct MarkAssets {
    glyphs: Vec<(String, String)>,
    colors: Vec<MarkColor>,
    words: Vec<String>,
}

impl MarkAssets {
    fn load() -> Result<Self, MarkError> {
        let glyphs: IndexMap<String, String> = serde_json::from_str(GLYPHS_JSON)
            .map_err(|error| MarkError::Asset(format!("glyphs.json: {error}")))?;
        let glyphs = glyphs.into_iter().collect::<Vec<_>>();
        if glyphs.len() != ICON_COUNT {
            return Err(MarkError::Asset(format!(
                "glyphs.json must contain {ICON_COUNT} icons; found {}",
                glyphs.len()
            )));
        }

        let colors: Vec<(String, String)> = serde_json::from_str(COLORS_JSON)
            .map_err(|error| MarkError::Asset(format!("colors.json: {error}")))?;
        if colors.len() != COLOR_COUNT {
            return Err(MarkError::Asset(format!(
                "colors.json must contain {COLOR_COUNT} colors; found {}",
                colors.len()
            )));
        }

        let words: Vec<String> = serde_json::from_str(WORDS_JSON)
            .map_err(|error| MarkError::Asset(format!("words.json: {error}")))?;
        if words.len() != WORD_COUNT {
            return Err(MarkError::Asset(format!(
                "words.json must contain {WORD_COUNT} words; found {}",
                words.len()
            )));
        }

        Ok(Self {
            glyphs,
            colors: colors
                .into_iter()
                .map(|(name, hex)| MarkColor { name, hex })
                .collect(),
            words,
        })
    }
}

#[cfg(all(test, not(feature = "full-tests")))]
mod tests {
    use crate::ca::jid_from_spki;

    use super::*;

    #[test]
    fn glyph_asset_order_is_load_bearing() {
        let assets = MarkAssets::load().unwrap();
        let names = assets
            .glyphs
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            &names[..6],
            ["anchor", "banana", "bike", "bug", "cake", "cat"]
        );
        assert_eq!(
            &names[names.len() - 6..],
            ["train", "trees", "truck", "turtle", "tv", "watch"]
        );
    }

    #[test]
    fn color_asset_order_is_load_bearing() {
        let assets = MarkAssets::load().unwrap();
        let names = assets
            .colors
            .iter()
            .map(|color| color.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(&names[..4], ["crimson", "orange", "amber", "gold"]);
        assert_eq!(
            &names[names.len() - 4..],
            ["purple", "magenta", "pink", "slate"]
        );
    }

    #[test]
    fn reordered_colors_change_the_reference_derivation() {
        let jid = parse_jid_bytes("f30ed159-ef46-8e9c-913f-e49f0fe7d201").unwrap();
        let assets = MarkAssets::load().unwrap();
        let expected = derive_mark(jid, &assets).unwrap();
        let mut reordered = MarkAssets::load().unwrap();
        reordered.colors.swap(9, 10);

        assert_ne!(derive_mark(jid, &reordered).unwrap(), expected);
    }

    #[test]
    fn mark_vectors_match_python() {
        let cases = [
            (
                "3059301306072a8648ce3d020106082a8648ce3d03010703420004471c3e758c4904285bba7e53118ed0f524adeb0757d25bd2f8e7b0d76dfa714cdd520f7aca8a8b917acc37f51de8f0c9bbe3ad858382e702dc25a12d09f7a858",
                "piano",
                "key",
                "blue",
                "#3b82f6",
                "purple",
                "#a855f7",
                45,
                0,
                "liquefy",
                "smock",
            ),
            (
                "3059301306072a8648ce3d020106082a8648ce3d030107034200047cf27b188d034f7e8a52380304b51ac3c08969e277f21b35a60b48fc4766997807775510db8ed040293d9ac69f7430dbba7dade63ce982299e04b79d227873d1",
                "turtle",
                "pizza",
                "pink",
                "#ec4899",
                "cyan",
                "#06b6d4",
                0,
                0,
                "distrust",
                "chokehold",
            ),
            (
                "3059301306072a8648ce3d020106082a8648ce3d030107034200048e533b6fa0bf7b4625bb30667c01fb607ef9f8b8a80fef5b300628703187b2a373eb1dbde03318366d069f83a6f5900053c73633cb041b21c55e1a86c1f400b4",
                "dice-5",
                "snail",
                "magenta",
                "#d946ef",
                "sky",
                "#38bdf8",
                0,
                45,
                "delay",
                "safari",
            ),
            (
                "3059301306072a8648ce3d020106082a8648ce3d03010703420004ea68d7b6fedf0b71878938d51d71f8729e0acb8c2c6df8b3d79e8a4b90949ee02a2744c972c9fce787014a964a8ea0c84d714feaa4de823fe85a224a4dd048fa",
                "truck",
                "snail",
                "orange",
                "#f97316",
                "teal",
                "#14b8a6",
                45,
                45,
                "duvet",
                "capital",
            ),
        ];

        for (
            spki,
            icon1,
            icon2,
            color1,
            color1_hex,
            color2,
            color2_hex,
            rot1,
            rot2,
            word1,
            word2,
        ) in cases
        {
            let jid = jid_from_spki(&decode_hex(spki)).unwrap();
            let mark = mark_from_jid(&jid).unwrap().to_render_spec();
            assert_eq!(mark.icon1.name, icon1);
            assert_eq!(mark.icon2.name, icon2);
            assert_eq!(mark.icon1.color.name, color1);
            assert_eq!(mark.icon1.color.hex, color1_hex);
            assert_eq!(mark.icon2.color.name, color2);
            assert_eq!(mark.icon2.color.hex, color2_hex);
            assert_eq!(mark.icon1.rot, rot1);
            assert_eq!(mark.icon2.rot, rot2);
            assert_eq!(mark.words, [word1.to_owned(), word2.to_owned()]);
        }
    }

    fn decode_hex(value: &str) -> Vec<u8> {
        value
            .as_bytes()
            .chunks_exact(2)
            .map(|pair| hex_byte(pair[0], pair[1]).unwrap())
            .collect()
    }
}
