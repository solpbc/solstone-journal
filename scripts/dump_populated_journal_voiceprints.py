#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Extract oracle-sensitive populated-journal voiceprint vectors."""

from __future__ import annotations

import json
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REPO_ROOT / "scripts"))
OUTPUT_PATH = REPO_ROOT / "core" / "fixtures" / "populated_journal_voiceprints.json"


def main() -> int:
    import entity_corpus

    from solstone.think.entities.voiceprints import (
        compute_intra_cosine_p25,
        load_entity_voiceprints_file,
    )

    voiceprints: dict[str, list[list[float]]] = {}
    with entity_corpus._temp_journal() as root:
        entity_corpus.seed_populated_speakers_journal(root)
        for entity_id in ("grace_hopper", "alan_turing"):
            loaded = load_entity_voiceprints_file(entity_id)
            if loaded is None:
                raise RuntimeError(f"missing seeded voiceprints for {entity_id}")
            embeddings, _metadata = loaded
            p25 = compute_intra_cosine_p25(embeddings)
            print(f"{entity_id} intra_cosine_p25={p25!r}")
            voiceprints[entity_id] = embeddings.astype(float).tolist()

    OUTPUT_PATH.write_text(json.dumps(voiceprints, indent=2) + "\n", encoding="utf-8")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
