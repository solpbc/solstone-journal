# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import hashlib
import json
import os
import subprocess
import tempfile
import textwrap
import unittest
from io import StringIO
from pathlib import Path
from unittest.mock import patch

from tools.journal_device_sim.process import LinkBridge, LinkProcessError

_FIXED_CERTIFICATE_PEM = """-----BEGIN CERTIFICATE-----
MIIBqTCCAU+gAwIBAgIUKZ4GlQ+jaITZjYye0LTx71Oqx/kwCgYIKoZIzj0EAwIw
KjEoMCYGA1UEAwwfc29sc3RvbmUgZml4ZWQgZG9vciBsb29rdXAgdGVzdDAeFw0y
NjA4MDQyMjMyNDFaFw0zNjA4MDEyMjMyNDFaMCoxKDAmBgNVBAMMH3NvbHN0b25l
IGZpeGVkIGRvb3IgbG9va3VwIHRlc3QwWTATBgcqhkjOPQIBBggqhkjOPQMBBwNC
AAQLWc/O7vh+eaolXyLl4UttktPMSL8L53AtLdpZnRxmQC0eA73pSSSHXyUricim
cdS9bsJS5CKw4vsk+W8Oh8rGo1MwUTAdBgNVHQ4EFgQUrMksIzdtNTRky8Sk8RLe
M0kYEQMwHwYDVR0jBBgwFoAUrMksIzdtNTRky8Sk8RLeM0kYEQMwDwYDVR0TAQH/
BAUwAwEB/zAKBggqhkjOPQQDAgNIADBFAiAVugzqjG4CX0sUgtnU3Xuo4gh9XK1P
KJnZhZwLOZPNdgIhAMNXOb63RcTM0DDHjfwiz6hLCvQ10aPUkW8izj8nv36W
-----END CERTIFICATE-----
"""
_FIXED_CERTIFICATE_CID = (
    "sha256:fbce31e7e99dbb0361851f0a27fe1909df27dc85ec268a9326c719dc8351d83e"
)
_VALID_NATIVE_SCRIPT = """#!/bin/sh
case "$1" in
    --help) printf '%s\\n' 'solstone - journal access CLI' ;;
    --version) printf '%s\\n' 'solstone-test 3.1.4' ;;
    *) exit 8 ;;
esac
"""


class LinkBridgeTests(unittest.TestCase):
    bundle_files = (
        "private.pem",
        "cert.pem",
        "chain.pem",
        "home_attestation.jwt",
        "peer.json",
    )

    def setUp(self) -> None:
        self._saved_convey_port = os.environ.pop("SOLSTONE_CONVEY_PORT", None)

    def tearDown(self) -> None:
        if self._saved_convey_port is None:
            os.environ.pop("SOLSTONE_CONVEY_PORT", None)
        else:
            os.environ["SOLSTONE_CONVEY_PORT"] = self._saved_convey_port

    @classmethod
    def write_bundle(
        cls,
        bundle: Path,
        *,
        cert: str = _FIXED_CERTIFICATE_PEM,
        peer: dict[str, object] | None = None,
    ) -> None:
        bundle.mkdir(parents=True)
        for name in cls.bundle_files:
            (bundle / name).write_text(f"{name}\n", encoding="utf-8")
        (bundle / "cert.pem").write_text(cert, encoding="utf-8")
        (bundle / "peer.json").write_text(
            json.dumps(
                peer
                or {
                    "instance_id": "receiver-instance",
                    "home_label": "Test Home",
                }
            ),
            encoding="utf-8",
        )

    def test_source_built_default_ignores_a_poisoned_path(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary) / "checkout"
            module_path = root / "tools" / "journal_device_sim" / "process.py"
            launcher = root / "core" / "target" / "debug" / "solstone"
            launcher.parent.mkdir(parents=True)
            launcher.write_text(_VALID_NATIVE_SCRIPT, encoding="utf-8")
            launcher.chmod(0o700)
            poison = Path(temporary) / "poison"
            poison.mkdir()
            (poison / "solstone").write_text("#!/bin/sh\nexit 97\n", encoding="utf-8")
            (poison / "solstone").chmod(0o700)
            elsewhere = Path(temporary) / "elsewhere"
            elsewhere.mkdir()
            original_cwd = Path.cwd()
            try:
                os.chdir(elsewhere)
                with patch(
                    "tools.journal_device_sim.process.__file__", str(module_path)
                ), patch.dict(os.environ, {"PATH": str(poison)}):
                    bridge = LinkBridge(
                        solstone_bin=None,
                        pair_code=None,
                        state_dir=Path(temporary) / "state",
                        carrier="direct",
                        relay_url=None,
                        convey_port=None,
                        startup_timeout=2,
                    )
                    self.assertEqual(bridge._native_executable(), str(launcher))
            finally:
                os.chdir(original_cwd)
            self.assertEqual(
                bridge.provenance["native_executable"],
                {
                    "path": str(launcher),
                    "sha256": hashlib.sha256(launcher.read_bytes()).hexdigest(),
                    "version": "solstone-test 3.1.4",
                    "selection_mode": "source-build-default",
                },
            )

    def test_absolute_override_wins_over_a_poisoned_path(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            launcher = root / "override-solstone"
            launcher.write_text(_VALID_NATIVE_SCRIPT, encoding="utf-8")
            launcher.chmod(0o700)
            poison = root / "poison"
            poison.mkdir()
            (poison / "solstone").write_text("#!/bin/sh\nexit 97\n", encoding="utf-8")
            (poison / "solstone").chmod(0o700)
            with patch.dict(os.environ, {"PATH": str(poison)}):
                bridge = LinkBridge(
                    solstone_bin=str(launcher),
                    pair_code=None,
                    state_dir=root / "state",
                    carrier="direct",
                    relay_url=None,
                    convey_port=None,
                    startup_timeout=2,
                )
                self.assertEqual(bridge._native_executable(), str(launcher))
            native = bridge.provenance["native_executable"]
            assert isinstance(native, dict)
            self.assertEqual(native["selection_mode"], "override")
            self.assertEqual(native["version"], "solstone-test 3.1.4")

    def test_relative_or_bare_native_override_is_rejected_at_construction(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            for value in ("solstone", "./solstone", "../solstone", "~/solstone"):
                with self.subTest(value=value):
                    with self.assertRaisesRegex(LinkProcessError, "absolute path"):
                        LinkBridge(
                            solstone_bin=value,
                            pair_code=None,
                            state_dir=Path(temporary) / value.replace("/", "_"),
                            carrier="direct",
                            relay_url=None,
                            convey_port=None,
                            startup_timeout=2,
                        )

    def test_native_header_preflight_preserves_version_provenance(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            launcher = Path(temporary) / "native-solstone"
            launcher.write_text(_VALID_NATIVE_SCRIPT, encoding="utf-8")
            launcher.chmod(0o700)
            bridge = LinkBridge(
                solstone_bin=str(launcher),
                pair_code=None,
                state_dir=Path(temporary) / "state",
                carrier="direct",
                relay_url=None,
                convey_port=None,
                startup_timeout=2,
            )
            self.assertEqual(bridge._native_executable(), str(launcher))
            native = bridge.provenance["native_executable"]
            assert isinstance(native, dict)
            self.assertEqual(native["version"], "solstone-test 3.1.4")

    def test_wrong_header_refuses_fresh_and_prepaired_before_bundle_validation(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            launcher = root / "impostor-solstone"
            launcher.write_text(
                "#!/bin/sh\nif [ \"$1\" = --help ]; then printf '%s\\n' impostor; fi\n",
                encoding="utf-8",
            )
            launcher.chmod(0o700)
            fresh = LinkBridge(
                solstone_bin=str(launcher),
                pair_code="pair-code",
                state_dir=root / "fresh",
                carrier="direct",
                relay_url=None,
                convey_port=None,
                startup_timeout=2,
            )
            with self.assertRaisesRegex(LinkProcessError, "condition=wrong-header"):
                fresh.ensure_paired()
            self.assertFalse((root / "fresh").exists())

            prepaired = LinkBridge(
                solstone_bin=str(launcher),
                pair_code=None,
                state_dir=root / "prepaired",
                carrier="direct",
                relay_url=None,
                convey_port=None,
                startup_timeout=2,
            )
            self.write_bundle(prepaired._bundle_dir())
            with patch.object(prepaired, "_validate_bundle") as validate:
                with self.assertRaisesRegex(LinkProcessError, "condition=wrong-header"):
                    prepaired.ensure_paired()
            validate.assert_not_called()

    def test_header_timeout_refuses_fresh_and_prepaired_and_reaps_children(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            launcher = root / "hanging-solstone"
            pid_path = root / "pid"
            launcher.write_text(
                textwrap.dedent(
                    """\
                    #!/usr/bin/env python3
                    import os
                    import pathlib
                    import sys
                    import time

                    if sys.argv[1:] == ["--help"]:
                        pathlib.Path(os.environ["PROBE_PID"]).write_text(str(os.getpid()))
                        time.sleep(60)
                    """
                ),
                encoding="utf-8",
            )
            launcher.chmod(0o700)
            with patch("tools.journal_device_sim.process._HELP_PREFLIGHT_TIMEOUT_S", 0.5), patch.dict(
                os.environ, {"PROBE_PID": str(pid_path)}
            ):
                for name, pair_code in (("fresh", "pair-code"), ("prepaired", None)):
                    with self.subTest(name=name):
                        bridge = LinkBridge(
                            solstone_bin=str(launcher),
                            pair_code=pair_code,
                            state_dir=root / name,
                            carrier="direct",
                            relay_url=None,
                            convey_port=None,
                            startup_timeout=2,
                        )
                        if pair_code is None:
                            self.write_bundle(bridge._bundle_dir())
                        with patch.object(bridge, "_validate_bundle") as validate:
                            with self.assertRaisesRegex(
                                LinkProcessError, "condition=probe-timeout"
                            ):
                                bridge.ensure_paired()
                        validate.assert_not_called()
                        pid = int(pid_path.read_text(encoding="utf-8"))
                        with self.assertRaises(ProcessLookupError):
                            os.kill(pid, 0)

    def test_header_preflight_rejects_stdout_and_stderr_overflow(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            for stream in ("stdout", "stderr"):
                with self.subTest(stream=stream):
                    launcher = Path(temporary) / f"overflow-{stream}"
                    launcher.write_text(
                        textwrap.dedent(
                            f"""\
                            #!/usr/bin/env python3
                            import sys

                            if sys.argv[1:] == ["--help"]:
                                sys.{stream}.buffer.write(b"x" * 65537)
                            """
                        ),
                        encoding="utf-8",
                    )
                    launcher.chmod(0o700)
                    bridge = LinkBridge(
                        solstone_bin=str(launcher),
                        pair_code=None,
                        state_dir=Path(temporary) / stream,
                        carrier="direct",
                        relay_url=None,
                        convey_port=None,
                        startup_timeout=2,
                    )
                    with self.assertRaisesRegex(
                        LinkProcessError, f"condition={stream}-overflow"
                    ):
                        bridge._native_executable()

    def test_nonzero_help_exit_with_wrong_header_is_refused(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            launcher = Path(temporary) / "nonzero-impostor"
            launcher.write_text(
                "#!/bin/sh\nprintf '%s\\n' impostor\nexit 17\n", encoding="utf-8"
            )
            launcher.chmod(0o700)
            bridge = LinkBridge(
                solstone_bin=str(launcher),
                pair_code=None,
                state_dir=Path(temporary) / "state",
                carrier="direct",
                relay_url=None,
                convey_port=None,
                startup_timeout=2,
            )
            with self.assertRaisesRegex(LinkProcessError, "nonzero-help-exit:17"):
                bridge._native_executable()

    def test_changed_candidate_between_join_and_serve_is_refused(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            launcher = root / "native-solstone"
            serve_marker = root / "serve-marker"
            launcher.write_text(
                textwrap.dedent(
                    f"""\
                    #!/usr/bin/env python3
                    import pathlib
                    import sys

                    if sys.argv[1:] == ["--help"]:
                        print("solstone - journal access CLI")
                        raise SystemExit(0)
                    if sys.argv[1:] == ["--version"]:
                        print("solstone-test 3.1.4")
                        raise SystemExit(0)
                    if sys.argv[1:3] == ["link", "join"]:
                        raise SystemExit(0)
                    if sys.argv[1:3] == ["link", "serve"]:
                        pathlib.Path({str(serve_marker)!r}).write_text("served")
                    """
                ),
                encoding="utf-8",
            )
            launcher.chmod(0o700)
            bridge = LinkBridge(
                solstone_bin=str(launcher),
                pair_code="pair-code",
                state_dir=root / "state",
                carrier="direct",
                relay_url=None,
                convey_port=None,
                startup_timeout=2,
            )
            with patch.object(bridge, "_validate_bundle"):
                bridge.ensure_paired()
            launcher.write_text("#!/bin/sh\nexit 8\n", encoding="utf-8")
            launcher.chmod(0o700)
            with self.assertRaisesRegex(LinkProcessError, "changed-candidate"):
                bridge.start()
            self.assertFalse(serve_marker.exists())

    def test_source_built_root_launcher_fixture_pairs_and_serves(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary) / "checkout"
            module_path = root / "tools" / "journal_device_sim" / "process.py"
            launcher = root / "core" / "target" / "debug" / "solstone"
            launcher.parent.mkdir(parents=True)
            source_launcher = (
                Path(__file__).resolve().parents[3]
                / "scripts"
                / "root-launchers"
                / "solstone"
            )
            launcher.write_bytes(source_launcher.read_bytes())
            launcher.chmod(0o755)
            sibling = launcher.parent / "solstone-core-sol"
            sibling.write_text(
                textwrap.dedent(
                    """\
                    #!/usr/bin/env python3
                    import json
                    import os
                    import pathlib
                    import sys
                    import time

                    if sys.argv[1:] == ["--help"]:
                        print("solstone - journal access CLI")
                        raise SystemExit(0)
                    if sys.argv[1:] == ["--version"]:
                        print("solstone-test 3.1.4")
                        raise SystemExit(0)
                    if sys.argv[1:3] == ["link", "join"]:
                        label = sys.argv[sys.argv.index("--label") + 1]
                        bundle = pathlib.Path(os.environ["XDG_CONFIG_HOME"]) / "solstone-observer" / "spl" / label
                        bundle.mkdir(parents=True)
                        for name in ("private.pem", "cert.pem", "chain.pem", "home_attestation.jwt", "peer.json"):
                            (bundle / name).write_text(name, encoding="utf-8")
                        (bundle / "cert.pem").write_text(__FIXED_CERTIFICATE__, encoding="utf-8")
                        (bundle / "peer.json").write_text(json.dumps({"instance_id": "home-test", "home_label": "Test Home"}), encoding="utf-8")
                        raise SystemExit(0)
                    if sys.argv[1:3] == ["link", "serve"]:
                        print("forwarding 127.0.0.1:43129 -> home test via direct connection", flush=True)
                        while True:
                            time.sleep(60)
                    raise SystemExit(8)
                    """
                ).replace("__FIXED_CERTIFICATE__", repr(_FIXED_CERTIFICATE_PEM)),
                encoding="utf-8",
            )
            sibling.chmod(0o700)
            with patch("tools.journal_device_sim.process.__file__", str(module_path)):
                bridge = LinkBridge(
                    solstone_bin=None,
                    pair_code="pair-code",
                    state_dir=root / "state",
                    carrier="direct",
                    relay_url=None,
                    convey_port=None,
                    startup_timeout=2,
                )
                try:
                    self.assertEqual(bridge.start(), "http://127.0.0.1:43129")
                finally:
                    bridge.stop()
            native = bridge.provenance["native_executable"]
            assert isinstance(native, dict)
            self.assertEqual(native["path"], str(launcher))
            self.assertEqual(native["selection_mode"], "source-build-default")

    def test_impostor_cli_is_refused_before_join_or_serve(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            launcher = root / "impostor-solstone"
            marker = root / "native-side-effect"
            launcher.write_text(
                textwrap.dedent(
                    f"""\
                    #!/usr/bin/env python3
                    import pathlib
                    import sys

                    if sys.argv[1:] == ["--help"]:
                        print("another CLI")
                        raise SystemExit(0)
                    if sys.argv[1:] == ["--version"]:
                        print("solstone-test 3.1.4")
                        raise SystemExit(0)
                    if sys.argv[1:3] in (["link", "join"], ["link", "serve"]):
                        pathlib.Path({str(marker)!r}).write_text("called")
                    """
                ),
                encoding="utf-8",
            )
            launcher.chmod(0o700)
            bridge = LinkBridge(
                solstone_bin=str(launcher),
                pair_code="pair-code",
                state_dir=root / "state",
                carrier="direct",
                relay_url=None,
                convey_port=None,
                startup_timeout=2,
            )
            with self.assertRaisesRegex(LinkProcessError, "condition=wrong-header"):
                bridge.start()
            self.assertFalse(marker.exists())
            self.assertFalse((root / "state").exists())

    def test_native_child_owns_pairing_ephemeral_port_and_cleanup(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            executable = root / "fake-solstone"
            executable.write_text(
                textwrap.dedent(
                    """\
                    #!/usr/bin/env python3
                    import json
                    import os
                    import pathlib
                    import sys
                    import time

                    if sys.argv[1:] == ["--help"]:
                        print("solstone - journal access CLI")
                        raise SystemExit(0)
                    if sys.argv[1:] == ["--version"]:
                        print("solstone-test 3.1.4")
                        raise SystemExit(0)
                    if sys.argv[1:3] == ["link", "join"]:
                        if os.environ.get("SOLSTONE_CONVEY_PORT") != "6200":
                            raise SystemExit(10)
                        label = sys.argv[sys.argv.index("--label") + 1]
                        bundle = pathlib.Path(os.environ["XDG_CONFIG_HOME"]) / "solstone-observer" / "spl" / label
                        bundle.mkdir(parents=True)
                        for name in ("private.pem", "cert.pem", "chain.pem", "home_attestation.jwt", "peer.json"):
                            (bundle / name).write_text(name, encoding="utf-8")
                        (bundle / "cert.pem").write_text(__FIXED_CERTIFICATE__, encoding="utf-8")
                        (bundle / "peer.json").write_text(json.dumps({"instance_id": "home-test", "home_label": "Test Home"}), encoding="utf-8")
                        raise SystemExit(0)
                    if sys.argv[1:3] == ["link", "serve"]:
                        if os.environ.get("SOLSTONE_CONVEY_PORT") != "6200":
                            raise SystemExit(10)
                        if sys.argv[sys.argv.index("--port") + 1] != "0":
                            raise SystemExit(9)
                        if "--direct" not in sys.argv:
                            raise SystemExit(12)
                        if "--relay-only" in sys.argv:
                            raise SystemExit(13)
                        print("forwarding 127.0.0.1:43127 -> home test via direct connection", flush=True)
                        while True:
                            time.sleep(60)
                    raise SystemExit(8)
                    """
                ).replace("__FIXED_CERTIFICATE__", repr(_FIXED_CERTIFICATE_PEM)),
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
                provenance = bridge.provenance
            finally:
                bridge.stop()
            self.assertEqual(
                provenance["native_executable"],
                {
                    "path": str(executable.resolve()),
                    "sha256": hashlib.sha256(executable.read_bytes()).hexdigest(),
                    "version": "solstone-test 3.1.4",
                    "selection_mode": "override",
                },
            )
            self.assertEqual(
                provenance["convey"], {"port": 6200, "source": "explicit"}
            )
            self.assertEqual(
                provenance["credentials"],
                {
                    "cert_pem_sha256": hashlib.sha256(
                        _FIXED_CERTIFICATE_PEM.encode("ascii")
                    ).hexdigest(),
                    "client_cid": _FIXED_CERTIFICATE_CID,
                    "peer": {
                        "instance_id": "home-test",
                        "home_label": "Test Home",
                    },
                },
            )
            bridge.remove_credentials()
            self.assertFalse(bridge.credential_dir.exists())
            self.assertNotIn("splink-secret-test-value", "\n".join(bridge._log))
            self.assertNotIn(
                "splink-secret-test-value", json.dumps(provenance, sort_keys=True)
            )

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

                    if sys.argv[1:] == ["--help"]:
                        print("solstone - journal access CLI")
                        raise SystemExit(0)
                    if sys.argv[1:3] == ["link", "join"]:
                        raise SystemExit(11)
                    if sys.argv[1:3] == ["link", "serve"]:
                        if "--direct" in sys.argv:
                            raise SystemExit(12)
                        if "--relay-only" not in sys.argv:
                            raise SystemExit(15)
                        if os.environ.get("SOLSTONE_CONVEY_PORT") != "6201":
                            raise SystemExit(13)
                        if "--relay-url" in sys.argv:
                            relay_url = sys.argv[sys.argv.index("--relay-url") + 1]
                            if relay_url != "wss://relay.test/v1":
                                raise SystemExit(14)
                        print("forwarding 127.0.0.1:43128 -> home test via relay only", flush=True)
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

            default_state = root / "default-relay-state"
            default_bundle = (
                default_state
                / "credentials"
                / "solstone-observer"
                / "spl"
                / "journal-device-sim"
            )
            self.write_bundle(default_bundle)
            default_bridge = LinkBridge(
                solstone_bin=str(executable),
                pair_code=None,
                state_dir=default_state,
                carrier="relay",
                relay_url=None,
                convey_port=6201,
                startup_timeout=2,
            )
            try:
                self.assertEqual(
                    default_bridge.start(), "http://127.0.0.1:43128"
                )
            finally:
                default_bridge.stop()

    def test_convey_port_provenance_covers_explicit_ambient_and_default(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            with patch.dict(
                os.environ, {"SOLSTONE_CONVEY_PORT": "not-a-port"}
            ):
                explicit = LinkBridge(
                    solstone_bin=str(Path(temporary) / "unused"),
                    pair_code=None,
                    state_dir=root / "explicit",
                    carrier="direct",
                    relay_url=None,
                    convey_port=6202,
                    startup_timeout=2,
                )
            self.assertEqual(
                explicit.provenance["convey"],
                {"port": 6202, "source": "explicit"},
            )

            with patch.dict(os.environ, {"SOLSTONE_CONVEY_PORT": "6203"}):
                ambient = LinkBridge(
                    solstone_bin=str(Path(temporary) / "unused"),
                    pair_code=None,
                    state_dir=root / "ambient",
                    carrier="direct",
                    relay_url=None,
                    convey_port=None,
                    startup_timeout=2,
                )
                self.assertEqual(ambient._env()["SOLSTONE_CONVEY_PORT"], "6203")
            self.assertEqual(
                ambient.provenance["convey"],
                {"port": 6203, "source": "ambient"},
            )

            default = LinkBridge(
                solstone_bin=str(Path(temporary) / "unused"),
                pair_code=None,
                state_dir=root / "default",
                carrier="direct",
                relay_url=None,
                convey_port=None,
                startup_timeout=2,
            )
            self.assertEqual(default._env()["SOLSTONE_CONVEY_PORT"], "5015")
            self.assertEqual(
                default.provenance["convey"],
                {"port": 5015, "source": "default"},
            )
            self.assertIsNone(default.provenance["native_executable"])

    def test_invalid_ambient_convey_port_fails_before_any_child(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            for value in (
                "",
                "0",
                "65536",
                "12.5",
                " 6200",
                "9" * 5000,
            ):
                with self.subTest(value=value), patch.dict(
                    os.environ, {"SOLSTONE_CONVEY_PORT": value}
                ), patch("tools.journal_device_sim.process.subprocess.run") as run, patch(
                    "tools.journal_device_sim.process.subprocess.Popen"
                ) as popen:
                    with self.assertRaisesRegex(
                        LinkProcessError,
                        "SOLSTONE_CONVEY_PORT must be an integer",
                    ):
                        LinkBridge(
                            solstone_bin=str(Path(temporary) / "unused"),
                            pair_code=None,
                            state_dir=Path(temporary) / "state",
                            carrier="direct",
                            relay_url=None,
                            convey_port=None,
                            startup_timeout=2,
                        )
                run.assert_not_called()
                popen.assert_not_called()

    def test_prepaired_provenance_exposes_only_non_secret_credential_fields(
        self,
    ) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            state_dir = root / "state"
            executable = root / "valid-solstone"
            executable.write_text(_VALID_NATIVE_SCRIPT, encoding="utf-8")
            executable.chmod(0o700)
            bundle = (
                state_dir
                / "credentials"
                / "solstone-observer"
                / "spl"
                / "journal-device-sim"
            )
            self.write_bundle(
                bundle,
                peer={
                    "instance_id": "receiver-safe-id",
                    "home_label": "Safe Home",
                    "relay_device_token": "TOKEN-SECRET-VALUE",
                    "local_endpoints": [{"host": "private-host"}],
                    "pair_link": "https://go.solstone.app/p#LINK-SECRET-VALUE",
                },
            )
            (bundle / "private.pem").write_text(
                "PRIVATE-KEY-SECRET", encoding="utf-8"
            )
            (bundle / "home_attestation.jwt").write_text(
                "ATTESTATION-SECRET", encoding="utf-8"
            )
            bridge = LinkBridge(
                solstone_bin=str(executable),
                pair_code=None,
                state_dir=state_dir,
                carrier="direct",
                relay_url=None,
                convey_port=None,
                startup_timeout=2,
            )
            bridge.ensure_paired()

            provenance = bridge.provenance
            self.assertEqual(
                provenance["native_executable"],
                {
                    "path": str(executable.resolve()),
                    "sha256": hashlib.sha256(executable.read_bytes()).hexdigest(),
                    "version": "solstone-test 3.1.4",
                    "selection_mode": "override",
                },
            )
            self.assertEqual(
                provenance["credentials"],
                {
                    "cert_pem_sha256": hashlib.sha256(
                        _FIXED_CERTIFICATE_PEM.encode("ascii")
                    ).hexdigest(),
                    "client_cid": _FIXED_CERTIFICATE_CID,
                    "peer": {
                        "instance_id": "receiver-safe-id",
                        "home_label": "Safe Home",
                    },
                },
            )
            encoded = json.dumps(provenance)
            for secret in (
                "TOKEN-SECRET-VALUE",
                "PRIVATE-KEY-SECRET",
                "ATTESTATION-SECRET",
                "private-host",
                "LINK-SECRET-VALUE",
            ):
                self.assertNotIn(secret, encoded)

    def test_client_cid_rejects_non_certificate_pem(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            executable = Path(temporary) / "valid-solstone"
            executable.write_text(_VALID_NATIVE_SCRIPT, encoding="utf-8")
            executable.chmod(0o700)
            for index, certificate in enumerate((
                "not a certificate\n",
                "-----BEGIN CERTIFICATE-----\n!!!!\n-----END CERTIFICATE-----\n",
                "-----BEGIN CERTIFICATE-----\nYWJj\n-----END CERTIFICATE-----\n",
                _FIXED_CERTIFICATE_PEM + "trailing data\n",
            )):
                with self.subTest(certificate=certificate[:32]):
                    state_dir = Path(temporary) / f"state-{index}"
                    bundle = (
                        state_dir
                        / "credentials"
                        / "solstone-observer"
                        / "spl"
                        / "journal-device-sim"
                    )
                    self.write_bundle(bundle, cert=certificate)
                    bridge = LinkBridge(
                        solstone_bin=str(executable),
                        pair_code=None,
                        state_dir=state_dir,
                        carrier="direct",
                        relay_url=None,
                        convey_port=None,
                        startup_timeout=2,
                    )
                    with self.assertRaisesRegex(
                        LinkProcessError, "credential cert.pem is invalid"
                    ):
                        bridge.ensure_paired()

    def test_prepaired_bundle_must_exist(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            executable = Path(temporary) / "valid-solstone"
            executable.write_text(_VALID_NATIVE_SCRIPT, encoding="utf-8")
            executable.chmod(0o700)
            bridge = LinkBridge(
                solstone_bin=str(executable),
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
            executable = Path(temporary) / "valid-solstone"
            executable.write_text(_VALID_NATIVE_SCRIPT, encoding="utf-8")
            executable.chmod(0o700)
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
            executable = root / "valid-solstone"
            executable.write_text(_VALID_NATIVE_SCRIPT, encoding="utf-8")
            executable.chmod(0o700)
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
                        solstone_bin=str(executable),
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
            executable = root / "valid-solstone"
            executable.write_text(_VALID_NATIVE_SCRIPT, encoding="utf-8")
            executable.chmod(0o700)
            outside = root / "outside-credentials"
            outside.mkdir()
            (state_dir / "credentials").symlink_to(
                outside, target_is_directory=True
            )
            bridge = LinkBridge(
                solstone_bin=str(executable),
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
                solstone_bin=str(Path(temporary) / "unused"),
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
                solstone_bin=str(Path(temporary) / "unused"),
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
                solstone_bin=str(Path(temporary) / "unused"),
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
                solstone_bin=str(Path(temporary) / "unused"),
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
