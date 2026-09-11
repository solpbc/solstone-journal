#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
"""Finite model-boundary double; installed only in a disposable test payload."""

import json
import os
import sys
from pathlib import Path


def main():
    root = Path(os.environ["TALENT_FAULT_CASE"])
    script = json.loads((root / "script.json").read_text())
    if sys.argv[1:] != [script["engine"], "--one-shot"]:
        raise ValueError("unexpected model command")
    raw = sys.stdin.buffer.read(1_048_577)
    if len(raw) > 1_048_576:
        raise ValueError("model request exceeds byte limit")
    request = json.loads(raw)
    calls = root / "calls.jsonl"
    prior = calls.read_text().splitlines() if calls.exists() else []
    index = len(prior)
    with calls.open("a") as out:
        out.write(json.dumps(request) + "\n")
    if index >= len(script["responses"]):
        raise ValueError("unexpected extra model call")
    response = script["responses"][index]
    for event in response if isinstance(response, list) else [response]:
        print(json.dumps(event), flush=True)


if __name__ == "__main__":
    main()
