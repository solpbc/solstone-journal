// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Entity slug generation mirrors `python-slugify` 8.0.4 only for the
//! `entity_slug(name)` call path: `slugify(name, separator="_")`, plus
//! solstone's 200-character MD5 suffix truncation.
//! It deliberately does not implement general slugify features outside that
//! call path: `allow_unicode`, custom `replacements`, `stopwords`, or
//! word-boundary truncation.

use unicode_normalization::UnicodeNormalization;
use unidecode::unidecode;

pub const MAX_ENTITY_SLUG_LENGTH: usize = 200;

pub fn entity_slug(name: &str) -> String {
    if name.trim().is_empty() {
        return String::new();
    }

    let mut text = replace_quote_runs_with_dash(name);
    text = nfkd(&text);
    text = unidecode(&text);
    text = replace_named_entities(&text);
    text = replace_decimal_entities(&text);
    text = replace_hex_entities(&text);
    text = nfkd(&text);
    text = text.to_lowercase();
    text = remove_quotes(&text);
    text = remove_number_commas(&text);
    text = replace_disallowed_with_dash(&text);
    text = collapse_and_trim_dashes(&text);
    text = text.replace('-', "_");

    if text.len() > MAX_ENTITY_SLUG_LENGTH {
        let digest = md5::compute(name.as_bytes());
        let hash = format!("{digest:x}");
        text = format!("{}_{}", &text[..MAX_ENTITY_SLUG_LENGTH - 9], &hash[..8]);
    }

    text
}

fn nfkd(text: &str) -> String {
    text.nfkd().collect()
}

fn replace_quote_runs_with_dash(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let mut in_quote_run = false;
    for ch in text.chars() {
        if ch == '\'' {
            if !in_quote_run {
                output.push('-');
                in_quote_run = true;
            }
        } else {
            output.push(ch);
            in_quote_run = false;
        }
    }
    output
}

fn remove_quotes(text: &str) -> String {
    text.chars().filter(|ch| *ch != '\'').collect()
}

fn replace_named_entities(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('&') {
        output.push_str(&rest[..start]);
        let after_amp = &rest[start + 1..];
        if let Some(end) = after_amp.find(';') {
            let name = &after_amp[..end];
            if let Some(ch) = html_entity(name) {
                output.push(ch);
                rest = &after_amp[end + 1..];
                continue;
            }
        }
        output.push('&');
        rest = after_amp;
    }
    output.push_str(rest);
    output
}

fn replace_decimal_entities(text: &str) -> String {
    replace_numeric_entities(text, "&#", 10, |ch| ch.is_ascii_digit())
}

fn replace_hex_entities(text: &str) -> String {
    replace_numeric_entities(text, "&#x", 16, |ch| ch.is_ascii_hexdigit())
}

fn replace_numeric_entities(
    text: &str,
    prefix: &str,
    radix: u32,
    valid_digit: impl Fn(char) -> bool,
) -> String {
    let mut output = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find(prefix) {
        output.push_str(&rest[..start]);
        let after_prefix = &rest[start + prefix.len()..];
        let digit_len = after_prefix
            .chars()
            .take_while(|ch| valid_digit(*ch))
            .map(char::len_utf8)
            .sum::<usize>();
        if digit_len > 0 && after_prefix[digit_len..].starts_with(';') {
            let digits = &after_prefix[..digit_len];
            let Ok(codepoint) = u32::from_str_radix(digits, radix) else {
                return text.to_string();
            };
            let Some(ch) = char::from_u32(codepoint) else {
                return text.to_string();
            };
            output.push(ch);
            rest = &after_prefix[digit_len + 1..];
        } else {
            output.push_str(prefix);
            rest = after_prefix;
        }
    }
    output.push_str(rest);
    output
}

fn remove_number_commas(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut output = String::with_capacity(text.len());
    for (idx, ch) in chars.iter().enumerate() {
        if *ch == ','
            && idx > 0
            && idx + 1 < chars.len()
            && chars[idx - 1].is_ascii_digit()
            && chars[idx + 1].is_ascii_digit()
        {
            continue;
        }
        output.push(*ch);
    }
    output
}

fn replace_disallowed_with_dash(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let mut in_disallowed_run = false;
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' {
            output.push(ch);
            in_disallowed_run = false;
        } else if !in_disallowed_run {
            output.push('-');
            in_disallowed_run = true;
        }
    }
    output
}

fn collapse_and_trim_dashes(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let mut previous_dash = false;
    for ch in text.chars() {
        if ch == '-' {
            if !previous_dash {
                output.push(ch);
                previous_dash = true;
            }
        } else {
            output.push(ch);
            previous_dash = false;
        }
    }
    output.trim_matches('-').to_string()
}

fn html_entity(name: &str) -> Option<char> {
    HTML_ENTITIES.iter().find_map(|(candidate, codepoint)| {
        if *candidate == name {
            char::from_u32(*codepoint)
        } else {
            None
        }
    })
}

const HTML_ENTITIES: &[(&str, u32)] = &[
    ("AElig", 198),
    ("Aacute", 193),
    ("Acirc", 194),
    ("Agrave", 192),
    ("Alpha", 913),
    ("Aring", 197),
    ("Atilde", 195),
    ("Auml", 196),
    ("Beta", 914),
    ("Ccedil", 199),
    ("Chi", 935),
    ("Dagger", 8225),
    ("Delta", 916),
    ("ETH", 208),
    ("Eacute", 201),
    ("Ecirc", 202),
    ("Egrave", 200),
    ("Epsilon", 917),
    ("Eta", 919),
    ("Euml", 203),
    ("Gamma", 915),
    ("Iacute", 205),
    ("Icirc", 206),
    ("Igrave", 204),
    ("Iota", 921),
    ("Iuml", 207),
    ("Kappa", 922),
    ("Lambda", 923),
    ("Mu", 924),
    ("Ntilde", 209),
    ("Nu", 925),
    ("OElig", 338),
    ("Oacute", 211),
    ("Ocirc", 212),
    ("Ograve", 210),
    ("Omega", 937),
    ("Omicron", 927),
    ("Oslash", 216),
    ("Otilde", 213),
    ("Ouml", 214),
    ("Phi", 934),
    ("Pi", 928),
    ("Prime", 8243),
    ("Psi", 936),
    ("Rho", 929),
    ("Scaron", 352),
    ("Sigma", 931),
    ("THORN", 222),
    ("Tau", 932),
    ("Theta", 920),
    ("Uacute", 218),
    ("Ucirc", 219),
    ("Ugrave", 217),
    ("Upsilon", 933),
    ("Uuml", 220),
    ("Xi", 926),
    ("Yacute", 221),
    ("Yuml", 376),
    ("Zeta", 918),
    ("aacute", 225),
    ("acirc", 226),
    ("acute", 180),
    ("aelig", 230),
    ("agrave", 224),
    ("alefsym", 8501),
    ("alpha", 945),
    ("amp", 38),
    ("and", 8743),
    ("ang", 8736),
    ("aring", 229),
    ("asymp", 8776),
    ("atilde", 227),
    ("auml", 228),
    ("bdquo", 8222),
    ("beta", 946),
    ("brvbar", 166),
    ("bull", 8226),
    ("cap", 8745),
    ("ccedil", 231),
    ("cedil", 184),
    ("cent", 162),
    ("chi", 967),
    ("circ", 710),
    ("clubs", 9827),
    ("cong", 8773),
    ("copy", 169),
    ("crarr", 8629),
    ("cup", 8746),
    ("curren", 164),
    ("dArr", 8659),
    ("dagger", 8224),
    ("darr", 8595),
    ("deg", 176),
    ("delta", 948),
    ("diams", 9830),
    ("divide", 247),
    ("eacute", 233),
    ("ecirc", 234),
    ("egrave", 232),
    ("empty", 8709),
    ("emsp", 8195),
    ("ensp", 8194),
    ("epsilon", 949),
    ("equiv", 8801),
    ("eta", 951),
    ("eth", 240),
    ("euml", 235),
    ("euro", 8364),
    ("exist", 8707),
    ("fnof", 402),
    ("forall", 8704),
    ("frac12", 189),
    ("frac14", 188),
    ("frac34", 190),
    ("frasl", 8260),
    ("gamma", 947),
    ("ge", 8805),
    ("gt", 62),
    ("hArr", 8660),
    ("harr", 8596),
    ("hearts", 9829),
    ("hellip", 8230),
    ("iacute", 237),
    ("icirc", 238),
    ("iexcl", 161),
    ("igrave", 236),
    ("image", 8465),
    ("infin", 8734),
    ("int", 8747),
    ("iota", 953),
    ("iquest", 191),
    ("isin", 8712),
    ("iuml", 239),
    ("kappa", 954),
    ("lArr", 8656),
    ("lambda", 955),
    ("lang", 9001),
    ("laquo", 171),
    ("larr", 8592),
    ("lceil", 8968),
    ("ldquo", 8220),
    ("le", 8804),
    ("lfloor", 8970),
    ("lowast", 8727),
    ("loz", 9674),
    ("lrm", 8206),
    ("lsaquo", 8249),
    ("lsquo", 8216),
    ("lt", 60),
    ("macr", 175),
    ("mdash", 8212),
    ("micro", 181),
    ("middot", 183),
    ("minus", 8722),
    ("mu", 956),
    ("nabla", 8711),
    ("nbsp", 160),
    ("ndash", 8211),
    ("ne", 8800),
    ("ni", 8715),
    ("not", 172),
    ("notin", 8713),
    ("nsub", 8836),
    ("ntilde", 241),
    ("nu", 957),
    ("oacute", 243),
    ("ocirc", 244),
    ("oelig", 339),
    ("ograve", 242),
    ("oline", 8254),
    ("omega", 969),
    ("omicron", 959),
    ("oplus", 8853),
    ("or", 8744),
    ("ordf", 170),
    ("ordm", 186),
    ("oslash", 248),
    ("otilde", 245),
    ("otimes", 8855),
    ("ouml", 246),
    ("para", 182),
    ("part", 8706),
    ("permil", 8240),
    ("perp", 8869),
    ("phi", 966),
    ("pi", 960),
    ("piv", 982),
    ("plusmn", 177),
    ("pound", 163),
    ("prime", 8242),
    ("prod", 8719),
    ("prop", 8733),
    ("psi", 968),
    ("quot", 34),
    ("rArr", 8658),
    ("radic", 8730),
    ("rang", 9002),
    ("raquo", 187),
    ("rarr", 8594),
    ("rceil", 8969),
    ("rdquo", 8221),
    ("real", 8476),
    ("reg", 174),
    ("rfloor", 8971),
    ("rho", 961),
    ("rlm", 8207),
    ("rsaquo", 8250),
    ("rsquo", 8217),
    ("sbquo", 8218),
    ("scaron", 353),
    ("sdot", 8901),
    ("sect", 167),
    ("shy", 173),
    ("sigma", 963),
    ("sigmaf", 962),
    ("sim", 8764),
    ("spades", 9824),
    ("sub", 8834),
    ("sube", 8838),
    ("sum", 8721),
    ("sup", 8835),
    ("sup1", 185),
    ("sup2", 178),
    ("sup3", 179),
    ("supe", 8839),
    ("szlig", 223),
    ("tau", 964),
    ("there4", 8756),
    ("theta", 952),
    ("thetasym", 977),
    ("thinsp", 8201),
    ("thorn", 254),
    ("tilde", 732),
    ("times", 215),
    ("trade", 8482),
    ("uArr", 8657),
    ("uacute", 250),
    ("uarr", 8593),
    ("ucirc", 251),
    ("ugrave", 249),
    ("uml", 168),
    ("upsih", 978),
    ("upsilon", 965),
    ("uuml", 252),
    ("weierp", 8472),
    ("xi", 958),
    ("yacute", 253),
    ("yen", 165),
    ("yuml", 255),
    ("zeta", 950),
    ("zwj", 8205),
    ("zwnj", 8204),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugifies_python_entity_slug_examples() {
        assert_eq!(entity_slug("Alice Johnson"), "alice_johnson");
        assert_eq!(entity_slug("O'Brien"), "o_brien");
        assert_eq!(entity_slug("AT&T"), "at_t");
        assert_eq!(entity_slug("José García"), "jose_garcia");
        assert_eq!(entity_slug(""), "");
        assert_eq!(entity_slug("   "), "");
    }

    #[test]
    fn slugifies_transliterated_vectors_from_python() {
        let cases = [
            ("日本語", "ri_ben_yu"),
            ("Ünïcödé Ñame", "unicode_name"),
            ("中文", "zhong_wen"),
            ("Москва", "moskva"),
            ("e\u{0301}mile", "emile"),
            ("👩\u{200d}💻 Developer", "developer"),
            ("影師嗎", "ying_shi_ma"),
        ];
        for (input, expected) in cases {
            assert_eq!(entity_slug(input), expected, "{input:?}");
        }
    }

    #[test]
    fn decodes_named_decimal_and_hex_html_entities_like_python_slugify() {
        let cases = [
            ("AT&amp;T", "at_t"),
            ("&eacute;mile", "e_mile"),
            ("&#233;mile", "e_mile"),
            ("&#xE9;mile", "e_mile"),
            ("&#9312; Project", "1_project"),
        ];
        for (input, expected) in cases {
            assert_eq!(entity_slug(input), expected, "{input:?}");
        }
    }

    #[test]
    fn nfkd_before_unidecode_matches_python_slugify() {
        assert_eq!(entity_slug("① Project"), "1_project");
        assert_eq!(entity_slug("Ångström"), "angstrom");
    }

    #[test]
    fn truncates_long_slug_with_python_md5_suffix() {
        let slug = entity_slug(&"A".repeat(250));
        assert_eq!(slug.len(), MAX_ENTITY_SLUG_LENGTH);
        assert_eq!(slug, format!("{}_e993f498", "a".repeat(191)));
    }
}
