# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Regression test for the helper's --locked Cargo build."""

import unittest

from tools.local_thinking_install.harness import find_or_build_helper


class TestHelperLockedBuild(unittest.TestCase):
    def test_locked_build_succeeds(self) -> None:
        # A helper Cargo.lock left behind by a workspace version bump breaks
        # find_or_build_helper() before any fixture can start the portal.
        helper_bin = find_or_build_helper()
        self.assertTrue(helper_bin.exists())


if __name__ == "__main__":
    unittest.main()
