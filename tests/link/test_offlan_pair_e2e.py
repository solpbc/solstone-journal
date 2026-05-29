# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""End-to-end off-LAN (spl-posture) pairing test.

Sibling of ``test_integration.py``. Where that test pairs over a direct HTTP
connection to convey and then exercises the relay-brokered *dial*, this test
drives the full off-LAN *pair* ceremony through the spl ``mobile/`` reference
client (the L10 phone surface) so every component is exercised together:

    spl/mobile (parse 0x03 relay QR)
      -> relay POST /session/pair-ticket   (TOTP-gated; nonce never sent)
      -> relay GET  /session/pair-dial      (one-use jti; brokers blind tunnel)
      -> solstone secure_listener           (loopback => pl-via-spl, cert-less
                                              admitted only while window open)
      -> POST /app/link/pair                (confined dispatch; CA-signs CSR,
                                              mints home attestation)
      -> relay POST /enroll/device          (no client_cert; device_fp from
                                              the attestation claim)

Then it proves the off-LAN-paired device reaches the journal (dial ->
/app/link/api/status) and that revocation ends reach.

Gated like the other live tests (``SPL_RELAY_LIVE_TESTS`` + relay reachability)
and additionally skipped unless:
  * ``bun`` is on PATH and the spl ``mobile/`` checkout is present
    (override its location with ``SPL_MOBILE_DIR``), and
  * TCP 7657 is free on this host.

The secure_listener binds 127.0.0.1:7657 with ``SO_REUSEPORT`` and the link
service pipes relay tunnels to that fixed loopback port, so a second journal
already bound to 7657 would make every tunnel non-deterministic (the kernel
load-balances across both listeners). This test therefore requires exclusive
ownership of 7657 — it skips loudly rather than flake.
"""

from __future__ import annotations

import asyncio
import contextlib
import json
import os
import shutil
import socket
import subprocess
import sys
import threading
import time
from pathlib import Path

import pytest
import requests

from tests.link.live_helpers import (
    RELAY_URL,
    _prepare_journal,
    list_devices,
    skip_unless_live_relay,
    unpair_device,
)

pytestmark = pytest.mark.integration
skip_unless_live_relay()

REPO = Path(__file__).resolve().parents[2]
SPL_MOBILE = Path(os.environ.get("SPL_MOBILE_DIR", REPO.parent / "spl" / "mobile"))
SECURE_LISTENER_PORT = 7657


def _port_in_use(host: str, port: int) -> bool:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
        s.settimeout(0.2)
        return s.connect_ex((host, port)) == 0


def _wait_tcp(host: str, port: int, timeout: float = 5.0) -> None:
    deadline = time.monotonic() + timeout
    last: OSError | None = None
    while time.monotonic() < deadline:
        try:
            with socket.create_connection((host, port), timeout=0.2):
                return
        except OSError as exc:
            last = exc
            time.sleep(0.1)
    raise RuntimeError(f"{host}:{port} not ready: {last}")


@contextlib.contextmanager
def _convey_with_secure_listener(journal: Path):
    """Start convey + secure_listener in-process WITHOUT re-preparing the journal
    (so the spl posture written by enable_spl survives)."""
    from werkzeug.serving import make_server

    from solstone.convey import create_app
    from solstone.convey.secure_listener import (
        start_secure_listener,
        stop_secure_listener,
    )
    from solstone.think.utils import write_service_port

    app = create_app(str(journal))
    app.config["SECURE_LISTENER_ENABLED"] = True
    start_secure_listener(app)
    server = make_server("127.0.0.1", 0, app, threaded=True)
    write_service_port("convey", server.server_port)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    _wait_tcp("127.0.0.1", server.server_port)
    _wait_tcp("127.0.0.1", SECURE_LISTENER_PORT)
    try:
        yield f"http://127.0.0.1:{server.server_port}"
    finally:
        stop_secure_listener(app)
        server.shutdown()
        thread.join(timeout=5)


@contextlib.contextmanager
def _link_service(journal: Path):
    """Start `sol link` parked on the relay; wait for the listen WS."""
    sol_bin = Path(sys.executable).with_name("sol")
    env = os.environ.copy()
    env["SOL_SKIP_SUPERVISOR_CHECK"] = "1"
    env["PYTHONUNBUFFERED"] = "1"
    env["PATH"] = f"{REPO / '.venv' / 'bin'}:{env.get('PATH', '')}"
    proc = subprocess.Popen(
        [str(sol_bin), "link", "-v"],
        cwd=REPO,
        env=env,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        bufsize=1,
    )
    lines: list[str] = []
    ready = threading.Event()

    def drain() -> None:
        assert proc.stderr is not None
        for line in proc.stderr:
            lines.append(line)
            if "listen WS open" in line:
                ready.set()

    threading.Thread(target=drain, daemon=True).start()
    if not ready.wait(timeout=20):
        proc.terminate()
        raise RuntimeError("link service never opened listen WS:\n" + "".join(lines[-40:]))
    try:
        yield proc
    finally:
        proc.terminate()
        with contextlib.suppress(subprocess.TimeoutExpired):
            proc.wait(timeout=5)


def _identity_and_enrolled(state: dict):
    from solstone.think.link.client import ClientIdentity, EnrolledDevice

    identity = ClientIdentity(
        private_key_pem=state["client_key_pem"],
        client_cert_pem=state["client_cert"],
        ca_chain_pem="".join(state["ca_chain"]),
        fingerprint=state["fingerprint"],
        home_instance_id=state["instance_id"],
        home_label=state.get("home_label", ""),
        home_attestation="",
    )
    return EnrolledDevice(device_token=state["device_token"], identity=identity)


async def _dial_status(state: dict) -> tuple[int, str]:
    from solstone.think.link.client import Client

    session = await Client.dial(RELAY_URL, _identity_and_enrolled(state))
    async with session:
        status, _headers, body = await session.request("GET", "/app/link/api/status")
    return status, body.decode("utf-8", "replace")


async def _dial_expect_revoked(state: dict) -> None:
    from solstone.think.link.client import Client, StreamResetError

    session = await Client.dial(RELAY_URL, _identity_and_enrolled(state))
    async with session:
        with pytest.raises(StreamResetError):
            await session.request("GET", "/app/link/api/status")


@pytest.mark.timeout(120)
def test_offlan_pair_reach_revoke(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    if shutil.which("bun") is None:
        pytest.skip("bun not on PATH")
    if not (SPL_MOBILE / "src" / "index.ts").exists():
        pytest.skip(f"spl mobile checkout not found at {SPL_MOBILE}")
    if _port_in_use("127.0.0.1", SECURE_LISTENER_PORT):
        pytest.skip(
            f"127.0.0.1:{SECURE_LISTENER_PORT} already in use; off-LAN e2e needs "
            "exclusive ownership of the secure-listener port (another journal is running)"
        )

    journal = tmp_path / "journal"
    journal.mkdir()
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))
    monkeypatch.setenv("SOL_LINK_RELAY_URL", RELAY_URL)
    monkeypatch.setenv("SOL_SKIP_SUPERVISOR_CHECK", "1")

    _prepare_journal(journal)

    # Enable spl: generate + upload the TOTP secret, register the CA, set posture.
    from solstone.think.services import spl as spl_service

    spl_service.enable_spl()
    assert spl_service.is_spl_enabled()

    state_path = tmp_path / "mobile-state.json"

    with _convey_with_secure_listener(journal) as base_url, _link_service(journal):
        # 1. open a pairing window + mint the relay (0x03) QR
        resp = requests.post(
            f"{base_url}/app/link/pair-start",
            json={"device_label": "offlan-e2e", "role": "phone"},
            timeout=10,
        )
        resp.raise_for_status()
        ps = resp.json()
        pair_link = ps["pair_link"]
        assert pair_link.startswith("https://link.solpbc.org/p#"), pair_link
        assert ps["expires_in"] == 30  # nonce TTL == one TOTP step (rotation)

        # 2. run the real L10 mobile client through the off-LAN ceremony
        result = subprocess.run(
            [
                "bun", "run", "src/index.ts", "pair",
                pair_link, "offlan-e2e-device",
                "--relay", RELAY_URL,
                "--state", str(state_path),
            ],
            cwd=SPL_MOBILE,
            capture_output=True,
            text=True,
            timeout=60,
        )
        assert result.returncode == 0, f"mobile pair failed:\n{result.stdout}\n{result.stderr}"

        state = json.loads(state_path.read_text())
        assert state.get("device_token"), "no device_token issued"
        fingerprint = state["fingerprint"]

        # 3. home registered the device (CSR signed over the cert-less tunnel)
        devices = list_devices(base_url)
        assert any(d["fingerprint"] == fingerprint for d in devices)

        # 4. the off-LAN-paired device reaches the journal
        status, body = asyncio.run(_dial_status(state))
        assert status == 200, body
        assert json.loads(body)["instance_id"] == state["instance_id"]

        # 5. revocation holds (per-request authorized_clients.json check)
        unpair_device(base_url, fingerprint)
        time.sleep(1.0)  # authorized_clients.json mtime poll
        asyncio.run(_dial_expect_revoked(state))
