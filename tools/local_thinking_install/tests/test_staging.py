# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Candidate staging regressions for the local thinking install harness."""

import tempfile
import unittest
from pathlib import Path

from tools.local_thinking_install.harness import (
    required_candidate_binaries,
    setup_disposable_source_root,
)


class TestCandidateStaging(unittest.TestCase):
    def test_journal_command_resolves_to_staged_dispatcher(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            candidate_dir = root / "candidate" / "debug"
            candidate_dir.mkdir(parents=True)
            for name in required_candidate_binaries():
                binary = candidate_dir / name
                binary.write_text("fixture", encoding="utf-8")
                binary.chmod(0o755)

            candidate_lib = candidate_dir.parent / "lib" / "solstone-core-speakers-analyze"
            candidate_lib.mkdir(parents=True)
            (candidate_lib / "libonnxruntime.fixture").write_text("fixture", encoding="utf-8")

            repo_root = root / "repo"
            (repo_root / "core" / "payload").mkdir(parents=True)
            (repo_root / "core" / "models" / "assets").mkdir(parents=True)

            source_root = setup_disposable_source_root(
                root / "case", candidate_dir, repo_root
            )
            debug_dir = source_root / "core" / "target" / "debug"
            journal_entry = debug_dir / "journal"

            self.assertTrue(journal_entry.exists())
            self.assertEqual(
                journal_entry.read_bytes(),
                (debug_dir / "solstone-core-journal").read_bytes(),
            )


if __name__ == "__main__":
    unittest.main()
