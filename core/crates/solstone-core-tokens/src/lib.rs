// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Token counts in the Qwen3.5-4B tokenizer: the journal's one counter, used to
//! size every request the same way whatever thinks.
//!
//! It matches the bundled engine's tokenizer token for token (llama.cpp does no
//! Unicode normalization, so neither does this). Other models tokenize
//! differently, so for them it is a close proxy; a provider that still refuses
//! an input takes the ordinary refusal path. The merges file is rebuilt by
//! `scripts/build_qwen_token_merges.py`.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};
use std::io::Read;
use std::sync::OnceLock;

/// Ranked byte-level BPE merges from `Qwen/Qwen3.5-4B` `tokenizer.json`.
const MERGES: &[u8] = include_bytes!("../assets/qwen3.5-4b-merges.txt.gz");

/// The tokenizer's added tokens. Each counts as one token wherever it appears.
const ADDED_TOKENS: [&str; 26] = [
    "<|endoftext|>",
    "<|im_start|>",
    "<|im_end|>",
    "<|object_ref_start|>",
    "<|object_ref_end|>",
    "<|box_start|>",
    "<|box_end|>",
    "<|quad_start|>",
    "<|quad_end|>",
    "<|vision_start|>",
    "<|vision_end|>",
    "<|vision_pad|>",
    "<|image_pad|>",
    "<|video_pad|>",
    "<tool_call>",
    "</tool_call>",
    "<|fim_prefix|>",
    "<|fim_middle|>",
    "<|fim_suffix|>",
    "<|fim_pad|>",
    "<|repo_name|>",
    "<|file_sep|>",
    "<tool_response>",
    "</tool_response>",
    "<think>",
    "</think>",
];

/// The tokenizer's pre-tokenizer split, applied with isolated matches.
const PRE_TOKENIZER: &str = r"(?i:'s|'t|'re|'ve|'m|'ll|'d)|[^\r\n\p{L}\p{N}]?[\p{L}\p{M}]+|\p{N}| ?[^\s\p{L}\p{M}\p{N}]+[\r\n]*|\s*[\r\n]+|\s+(?!\S)|\s+";

struct Tokenizer {
    /// `(left, right)` symbol ids to `(rank, merged id)`.
    merges: HashMap<(u32, u32), (u32, u32)>,
    split: fancy_regex::Regex,
}

fn tokenizer() -> &'static Tokenizer {
    static TOKENIZER: OnceLock<Tokenizer> = OnceLock::new();
    TOKENIZER.get_or_init(|| {
        // The first line names the source; every later line is one merge, in rank order.
        let mut text = String::new();
        flate2::read::GzDecoder::new(MERGES)
            .read_to_string(&mut text)
            .expect("bundled merges decompress");
        let byte_of: HashMap<char, u8> = (0..=255u8).map(|byte| (byte_char(byte), byte)).collect();
        let decode = |symbol: &str| -> Vec<u8> { symbol.chars().map(|c| byte_of[&c]).collect() };
        let mut ids: HashMap<Vec<u8>, u32> = (0..=255u8)
            .map(|byte| (vec![byte], u32::from(byte)))
            .collect();
        // A merge may name a symbol that only a later merge produces.
        let intern = |ids: &mut HashMap<Vec<u8>, u32>, symbol: Vec<u8>| {
            let next = u32::try_from(ids.len()).expect("vocabulary fits u32");
            *ids.entry(symbol).or_insert(next)
        };
        let mut merges = HashMap::new();
        for (rank, line) in text.lines().skip(1).enumerate() {
            let (left, right) = line.split_once(' ').expect("merge line has two symbols");
            let (left, right) = (decode(left), decode(right));
            let merged = [left.as_slice(), right.as_slice()].concat();
            let pair = (intern(&mut ids, left), intern(&mut ids, right));
            let merged = intern(&mut ids, merged);
            let rank = u32::try_from(rank).expect("rank fits u32");
            merges.entry(pair).or_insert((rank, merged));
        }
        Tokenizer {
            merges,
            split: fancy_regex::Regex::new(PRE_TOKENIZER).expect("pre-tokenizer compiles"),
        }
    })
}

/// GPT-2's printable stand-in for each byte, the alphabet the merges are written in.
fn byte_char(byte: u8) -> char {
    let printable = |b: u8| (b'!'..=b'~').contains(&b) || (0xA1..=0xAC).contains(&b) || b >= 0xAE;
    if printable(byte) {
        return char::from(byte);
    }
    let offset = (0..byte).filter(|&b| !printable(b)).count();
    char::from_u32(256 + u32::try_from(offset).expect("offset fits")).expect("valid stand-in")
}

/// How many Qwen3.5-4B tokens `text` is.
pub fn count(text: &str) -> u32 {
    let tokenizer = tokenizer();
    let mut total = 0usize;
    let mut rest = text;
    while !rest.is_empty() {
        let (before, added, after) = next_added_token(rest);
        total += count_ordinary(tokenizer, before);
        if added {
            total += 1;
        }
        rest = after;
    }
    u32::try_from(total).unwrap_or(u32::MAX)
}

/// Split at the leftmost, longest added token.
fn next_added_token(text: &str) -> (&str, bool, &str) {
    for (index, _) in text.match_indices('<') {
        let found = ADDED_TOKENS
            .iter()
            .filter(|token| text[index..].starts_with(*token))
            .map(|token| token.len())
            .max();
        if let Some(len) = found {
            return (&text[..index], true, &text[index + len..]);
        }
    }
    (text, false, "")
}

fn count_ordinary(tokenizer: &Tokenizer, text: &str) -> usize {
    let mut total = 0;
    let mut end = 0;
    for found in tokenizer.split.find_iter(text) {
        // The pattern's lookahead can exhaust the backtracking limit on
        // pathological text. No token spans less than a byte, so counting the
        // rest by bytes never lets an input look smaller than it is.
        let Ok(found) = found else {
            return total + text.len() - end;
        };
        if found.start() > end {
            total += count_piece(tokenizer, &text.as_bytes()[end..found.start()]);
        }
        total += count_piece(tokenizer, found.as_str().as_bytes());
        end = found.end();
    }
    if end < text.len() {
        total += count_piece(tokenizer, &text.as_bytes()[end..]);
    }
    total
}

/// Apply ranked merges to one pre-token, lowest rank and then leftmost first.
fn count_piece(tokenizer: &Tokenizer, bytes: &[u8]) -> usize {
    if bytes.len() < 2 {
        return bytes.len();
    }
    let mut ids: Vec<u32> = bytes.iter().map(|&byte| u32::from(byte)).collect();
    let mut next: Vec<usize> = (1..=bytes.len()).collect();
    let mut prev: Vec<usize> = (0..bytes.len()).map(|i| i.wrapping_sub(1)).collect();
    let mut alive = bytes.len();
    let mut queue = BinaryHeap::new();
    let push = |queue: &mut BinaryHeap<_>, ids: &[u32], left: usize, right: usize| {
        if let Some(&(rank, merged)) = tokenizer.merges.get(&(ids[left], ids[right])) {
            queue.push(Reverse((rank, left, right, ids[left], ids[right], merged)));
        }
    };
    for left in 0..bytes.len() - 1 {
        push(&mut queue, &ids, left, left + 1);
    }
    while let Some(Reverse((_, left, right, left_id, right_id, merged))) = queue.pop() {
        if ids[left] != left_id || next[left] != right || ids[right] != right_id {
            continue;
        }
        ids[left] = merged;
        ids[right] = u32::MAX;
        next[left] = next[right];
        if next[left] < bytes.len() {
            prev[next[left]] = left;
        }
        alive -= 1;
        if prev[left] != usize::MAX {
            push(&mut queue, &ids, prev[left], left);
        }
        if next[left] < bytes.len() {
            push(&mut queue, &ids, left, next[left]);
        }
    }
    alive
}

#[cfg(test)]
mod tests {
    use super::count;

    /// Counts produced by the bundled engine (llama.cpp b11669 with
    /// `Qwen3.5-4B-Q4_K_M.gguf`, `llama-tokenize --no-bos --no-escape`).
    #[test]
    fn counts_match_the_bundled_engine() {
        for (text, tokens) in [
            ("", 0),
            ("#[derive(Debug)]\nfn main() { let x = 42; }\n", 18),
            (
                "The journal sizes every request the same way, whatever thinks.",
                12,
            ),
            (
                r#"{"panes":[{"id":"%0","active":true,"text":"$ ls -la\n-rw-r--r-- 1 user"}]}"#,
                31,
            ),
            (
                "<|im_start|>user\n<think>plan</think><|im_end|><|endoftext|>",
                8,
            ),
            (
                "日本語のテキスト、中文简体、한국어, العربية, हिन्दी, e\u{301} 👩\u{200d}💻",
                24,
            ),
            ("a   \n\n\t  b      \r\n   ", 6),
            ("It's we'LL they'd I'M you're", 11),
            ("\x1b[1;32muser@host\x1b[0m:~$ cargo test\r\n┌──┬──┐\n", 27),
        ] {
            assert_eq!(count(text), tokens, "{text:?}");
        }
    }

    #[test]
    fn text_past_the_pattern_backtracking_limit_is_overcounted_not_a_panic() {
        let text = format!("{}x", " ".repeat(1_000_000));
        assert!(count(&text) >= 2);
        assert!(count(&text) as usize <= text.len());
    }

    #[test]
    fn a_long_unbroken_run_counts_without_slowing_down() {
        let run = "x".repeat(200_000);
        let started = std::time::Instant::now();
        assert!(count(&run) > 0);
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
    }
}
