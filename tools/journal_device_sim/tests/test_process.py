# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import tempfile
import textwrap
import unittest
import subprocess
from io import StringIO
from pathlib import Path
from unittest.mock import patch

from tools.journal_device_sim.process import LinkBridge, LinkProcessError


class LinkBridgeTests(unittest.TestCase):
    bundle_files = (
        "private.pem",
        "cert.pem",
        "chain.pem",
        "home_attestation.jwt",
        "peer.json",
    )

    @classmethod
    def write_bundle(cls, bundle: Path) -> None:
        bundle.mkdir(parents=True)
        for name in cls.bundle_files:
            (bundle / name).write_text(f"{name}\n", encoding="utf-8")

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
                        if os.environ.get("SOLSTONE_CONVEY_PORT") != "6200":
                            raise SystemExit(10)
                        label = sys.argv[sys.argv.index("--label") + 1]
                        bundle = pathlib.Path(os.environ["XDG_CONFIG_HOME"]) / "solstone-observer" / "spl" / label
                        bundle.mkdir(parents=True)
                        for name in ("private.pem", "cert.pem", "chain.pem", "home_attestation.jwt", "peer.json"):
                            (bundle / name).write_text(name, encoding="utf-8")
                        raise SystemExit(0)
                    if sys.argv[1:3] == ["link", "serve"]:
                        if os.environ.get("SOLSTONE_CONVEY_PORT") != "6200":
                            raise SystemExit(10)
                        if sys.argv[sys.argv.index("--port") + 1] != "0":
                            raise SystemExit(9)
                        if "--direct" not in sys.argv:
                            raise SystemExit(12)
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
                convey_port=6200,
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

    def test_prepaired_bundle_skips_join(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            executable = root / "fake-solstone"
            executable.write_text(
                textwrap.dedent(
                    """\
                    #!/usr/bin/env python3
                    import os
                    import sys
                    import time

                    if sys.argv[1:3] == ["link", "join"]:
                        raise SystemExit(11)
                    if sys.argv[1:3] == ["link", "serve"]:
                        if "--direct" in sys.argv:
                            raise SystemExit(12)
                        if os.environ.get("SOLSTONE_CONVEY_PORT") != "6201":
                            raise SystemExit(13)
                        relay_url = sys.argv[sys.argv.index("--relay-url") + 1]
                        if relay_url != "wss://relay.test/v1":
                            raise SystemExit(14)
                        print("forwarding 127.0.0.1:43128 -> home test over pl", flush=True)
                        while True:
                            time.sleep(60)
                    raise SystemExit(8)
                    """
                ),
                encoding="utf-8",
            )
            executable.chmod(0o700)
            state_dir = root / "state"
            bundle = (
                state_dir
                / "credentials"
                / "solstone-observer"
                / "spl"
                / "journal-device-sim"
            )
            self.write_bundle(bundle)
            bridge = LinkBridge(
                solstone_bin=str(executable),
                pair_code=None,
                state_dir=state_dir,
                carrier="relay",
                relay_url="wss://relay.test/v1",
                convey_port=6201,
                startup_timeout=2,
            )
            try:
                self.assertEqual(bridge.start(), "http://127.0.0.1:43128")
            finally:
                bridge.stop()

    def test_prepaired_bundle_must_exist(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            bridge = LinkBridge(
                solstone_bin="unused",
                pair_code=None,
                state_dir=Path(temporary) / "state",
                carrier="relay",
                relay_url=None,
                convey_port=None,
                startup_timeout=2,
            )
            with self.assertRaisesRegex(
                LinkProcessError, "pre-paired credential bundle is missing"
            ):
                bridge.ensure_paired()

    def test_existing_bundle_refuses_a_new_pair_code(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            state_dir = Path(temporary) / "state"
            bundle = (
                state_dir
                / "credentials"
                / "solstone-observer"
                / "spl"
                / "journal-device-sim"
            )
            self.write_bundle(bundle)
            bridge = LinkBridge(
                solstone_bin="unused",
                pair_code="new-pair-code",
                state_dir=state_dir,
                carrier="direct",
                relay_url=None,
                convey_port=None,
                startup_timeout=2,
            )
            with self.assertRaisesRegex(
                LinkProcessError, "credential bundle already exists"
            ):
                bridge.ensure_paired()

    def test_prepaired_bundle_rejects_incomplete_and_symlinked_state(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            for shape in ("empty", "partial", "symlink"):
                with self.subTest(shape=shape):
                    state_dir = root / shape / "state"
                    bundle = (
                        state_dir
                        / "credentials"
                        / "solstone-observer"
                        / "spl"
                        / "journal-device-sim"
                    )
                    if shape == "empty":
                        bundle.mkdir(parents=True)
                    elif shape == "partial":
                        bundle.mkdir(parents=True)
                        (bundle / "cert.pem").write_text("cert\n", encoding="utf-8")
                    else:
                        outside = root / "outside"
                        self.write_bundle(outside)
                        bundle.parent.mkdir(parents=True)
                        bundle.symlink_to(outside, target_is_directory=True)
                    bridge = LinkBridge(
                        solstone_bin="unused",
                        pair_code=None,
                        state_dir=state_dir,
                        carrier="relay",
                        relay_url=None,
                        convey_port=None,
                        startup_timeout=2,
                    )
                    with self.assertRaises(LinkProcessError):
                        bridge.ensure_paired()

    def test_pairing_refuses_a_linked_credential_root(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            state_dir = root / "state"
            state_dir.mkdir()
            outside = root / "outside-credentials"
            outside.mkdir()
            (state_dir / "credentials").symlink_to(
                outside, target_is_directory=True
            )
            bridge = LinkBridge(
                solstone_bin="unused",
                pair_code="new-pair-code",
                state_dir=state_dir,
                carrier="direct",
                relay_url=None,
                convey_port=None,
                startup_timeout=2,
            )
            with self.assertRaisesRegex(
                LinkProcessError, "credential state must contain only plain directories"
            ):
                bridge.ensure_paired()

    def test_stop_tolerates_child_exit_between_poll_and_terminate(self) -> None:
        class ExitingProcess:
            stdout = StringIO()

            @staticmethod
            def poll() -> None:
                return None

            @staticmethod
            def terminate() -> None:
                raise ProcessLookupError()

            @staticmethod
            def wait(*, timeout: float) -> int:
                self.assertEqual(timeout, 5)
                return 0

        with tempfile.TemporaryDirectory() as temporary:
            bridge = LinkBridge(
                solstone_bin="unused",
                pair_code=None,
                state_dir=Path(temporary) / "state",
                carrier="relay",
                relay_url=None,
                convey_port=None,
                startup_timeout=2,
            )
            bridge._process = ExitingProcess()  # type: ignore[assignment]
            bridge.stop()
            self.assertIsNone(bridge._process)

    def test_finish_attempts_credential_cleanup_after_stop_failure(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            bridge = LinkBridge(
                solstone_bin="unused",
                pair_code=None,
                state_dir=Path(temporary) / "state",
                carrier="relay",
                relay_url=None,
                convey_port=None,
                startup_timeout=2,
            )
            with patch.object(
                bridge,
                "stop",
                side_effect=LinkProcessError("stop failed"),
            ), patch.object(bridge, "remove_credentials") as remove:
                with self.assertRaisesRegex(
                    LinkProcessError, "native bridge finalization failed"
                ):
                    bridge.finish(remove_credentials=True)
            remove.assert_called_once_with()

    def test_stop_retains_live_child_handle_after_failed_termination(self) -> None:
        class StuckProcess:
            stdout = StringIO()

            @staticmethod
            def poll() -> None:
                return None

            @staticmethod
            def terminate() -> None:
                raise PermissionError()

            @staticmethod
            def kill() -> None:
                raise PermissionError()

            @staticmethod
            def wait(*, timeout: float) -> int:
                raise subprocess.TimeoutExpired("fake-solstone", timeout)

        with tempfile.TemporaryDirectory() as temporary:
            bridge = LinkBridge(
                solstone_bin="unused",
                pair_code=None,
                state_dir=Path(temporary) / "state",
                carrier="relay",
                relay_url=None,
                convey_port=None,
                startup_timeout=2,
            )
            process = StuckProcess()
            bridge._process = process  # type: ignore[assignment]
            with self.assertRaisesRegex(LinkProcessError, "cleanup failed"):
                bridge.stop()
            self.assertIs(bridge._process, process)

    def test_finish_preserves_credentials_while_child_may_be_live(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            bridge = LinkBridge(
                solstone_bin="unused",
                pair_code=None,
                state_dir=Path(temporary) / "state",
                carrier="relay",
                relay_url=None,
                convey_port=None,
                startup_timeout=2,
            )
            bridge._process = object()  # type: ignore[assignment]
            with patch.object(
                bridge,
                "stop",
                side_effect=LinkProcessError("child remains live"),
            ), patch.object(bridge, "remove_credentials") as remove:
                with self.assertRaisesRegex(
                    LinkProcessError, "native bridge finalization failed"
                ):
                    bridge.finish(remove_credentials=True)
            remove.assert_not_called()


if __name__ == "__main__":
    unittest.main()
