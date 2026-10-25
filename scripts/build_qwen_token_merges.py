# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Build the token counter's merges file from Qwen3.5-4B's published tokenizer.

The journal counts every model input in the Qwen3.5-4B tokenizer
(``core/crates/solstone-core-tokens``). Counting needs only the tokenizer's ranked
byte-level BPE merges; this keeps them, one per line in rank order, after a line
naming the source revision. The 26 added tokens and the pre-tokenizer pattern are
constants in the crate and are checked against this file's source below.

Input: ``tokenizer.json`` from https://huggingface.co/Qwen/Qwen3.5-4B at the
revision passed as the second argument (Apache-2.0, see
``core/distribution/model-notices.md``). The output is deterministic for given
input bytes.

    python3 scripts/build_qwen_token_merges.py tokenizer.json 851bf6e806efd8d0a36b00ddf55e13ccb7b8cd0a
"""

from __future__ import annotations

import gzip
import json
import sys
from pathlib import Path

OUTPUT = Path(__file__).resolve().parents[1] / "core/crates/solstone-core-tokens/assets/qwen3.5-4b-merges.txt.gz"
PRE_TOKENIZER = (
    r"(?i:'s|'t|'re|'ve|'m|'ll|'d)|[^\r\n\p{L}\p{N}]?[\p{L}\p{M}]+|\p{N}| ?[^\s\p{L}\p{M}\p{N}]+[\r\n]*"
    r"|\s*[\r\n]+|\s+(?!\S)|\s+"
)


def main(source: str, revision: str) -> None:
    tokenizer = json.loads(Path(source).read_text(encoding="utf-8"))
    model = tokenizer["model"]
    assert model["type"] == "BPE" and not model["ignore_merges"] and not model["byte_fallback"]
    split, byte_level = tokenizer["pre_tokenizer"]["pretokenizers"]
    assert split["pattern"]["Regex"] == PRE_TOKENIZER and split["behavior"] == "Isolated"
    assert byte_level["type"] == "ByteLevel" and not byte_level["add_prefix_space"]
    added = tokenizer["added_tokens"]
    assert len(added) == 26 and not any(t["lstrip"] or t["rstrip"] or t["single_word"] for t in added)
    merges = model["merges"]
    assert all(isinstance(merge, str) and merge.count(" ") == 1 for merge in merges)
    body = f"#solstone-qwen-bpe-merges v1 Qwen/Qwen3.5-4B@{revision}\n" + "\n".join(merges) + "\n"
    with gzip.GzipFile(OUTPUT, "wb", compresslevel=9, mtime=0) as out:
        out.write(body.encode("utf-8"))
    print(f"{len(merges)} merges -> {OUTPUT}")
    print("added tokens:", [t["content"] for t in added])


if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2])
