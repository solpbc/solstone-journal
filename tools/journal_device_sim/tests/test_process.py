# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import tempfile
import textwrap
import unittest
from pathlib import Path

from tools.journal_device_sim.process import LinkBridge


class LinkBridgeTests(unittest.TestCase):
    def test_native_child_owns_pairing_ephemeral_port_and_cleanup(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            executable = root / "fake-solstone"
            executable.write_text(
                textwrap.dedent(
                    """\
                    #!/usr/bin/env python3
                    import os
                    import pathlib
                    import sys
                    import time

                    if sys.argv[1:3] == ["link", "join"]:
                        label = sys.argv[sys.argv.index("--label") + 1]
                        bundle = pathlib.Path(os.environ["XDG_CONFIG_HOME"]) / "solstone-observer" / "spl" / label
                        bundle.mkdir(parents=True)
                        raise SystemExit(0)
                    if sys.argv[1:3] == ["link", "serve"]:
                        if sys.argv[sys.argv.index("--port") + 1] != "0":
                            raise SystemExit(9)
                        print("forwarding 127.0.0.1:43127 -> home test over pl", flush=True)
                        while True:
                            time.sleep(60)
                    raise SystemExit(8)
                    """
                ),
                encoding="utf-8",
            )
            executable.chmod(0o700)
            state_dir = root / "state"
            bridge = LinkBridge(
                solstone_bin=str(executable),
                pair_code="splink-secret-test-value",
                state_dir=state_dir,
                carrier="direct",
                relay_url=None,
                startup_timeout=2,
            )
            try:
                self.assertEqual(bridge.start(), "http://127.0.0.1:43127")
                self.assertTrue(bridge.credential_dir.is_dir())
            finally:
                bridge.stop()
            bridge.remove_credentials()
            self.assertFalse(bridge.credential_dir.exists())
            self.assertNotIn("splink-secret-test-value", "\n".join(bridge._log))


if __name__ == "__main__":
    unittest.main()
