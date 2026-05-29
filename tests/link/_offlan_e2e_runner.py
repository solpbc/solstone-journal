# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Manual off-LAN pairing e2e runner (VPE live validation; not pytest-collected).

Drives the full cross-component off-LAN pair chain:

  spl/mobile (L10 reference client)
    -> relay /session/pair-ticket   (TOTP-gated, L5)
    -> relay /session/pair-dial     (one-use jti, brokers blind tunnel, L5)
    -> solstone secure_listener      (loopback => pl-via-spl, cert-less if window open, L6)
    -> /app/link/pair                (confined dispatch, CA-signs CSR, mints attestation)
    -> relay /enroll/device          (no client_cert; device_fp from attestation, L3/Change 1)

Then proves the off-LAN-paired device reaches the journal (dial -> /app/link/api/status)
and that revocation ends reach.

Run with the solstone venv python:
  SOLSTONE_JOURNAL=/tmp/spl-offlan-home \
  SOL_LINK_RELAY_URL=https://spl-relay-staging.jer-3f2.workers.dev \
  /home/jer/projects/solstone/.venv/bin/python tests/link/_offlan_e2e_runner.py

Assumes `journal services enable spl` has already run against the same journal + relay.
"""

from __future__ import annotations

import asyncio
import contextlib
import json
import os
import socket
import subprocess
import sys
import threading
import time
from pathlib import Path

import requests

REPO = Path(__file__).resolve().parents[2]
SPL_MOBILE = Path(os.environ.get("SPL_MOBILE_DIR", "/home/jer/projects/spl/mobile"))
RELAY = os.environ["SOL_LINK_RELAY_URL"].rstrip("/")
JOURNAL = Path(os.environ["SOLSTONE_JOURNAL"])


def _wait_tcp(host: str, port: int, timeout: float = 5.0) -> None:
    deadline = time.monotonic() + timeout
    last = None
    while time.monotonic() < deadline:
        try:
            with socket.create_connection((host, port), timeout=0.2):
                return
        except OSError as exc:
            last = exc
            time.sleep(0.1)
    raise RuntimeError(f"{host}:{port} not ready: {last}")


@contextlib.contextmanager
def convey_server():
    from solstone.convey import create_app
    from solstone.convey.secure_listener import (
        start_secure_listener,
        stop_secure_listener,
    )
    from solstone.think.utils import write_service_port

    app = create_app(str(JOURNAL))
    app.config["SECURE_LISTENER_ENABLED"] = True
    start_secure_listener(app)
    from werkzeug.serving import make_server

    server = make_server("127.0.0.1", 0, app, threaded=True)
    write_service_port("convey", server.server_port)
    t = threading.Thread(target=server.serve_forever, daemon=True)
    t.start()
    _wait_tcp("127.0.0.1", server.server_port)
    _wait_tcp("127.0.0.1", 7657)
    try:
        yield f"http://127.0.0.1:{server.server_port}"
    finally:
        stop_secure_listener(app)
        server.shutdown()
        t.join(timeout=5)


@contextlib.contextmanager
def link_service():
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

    def drain():
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


def pair_start(base_url: str) -> dict:
    r = requests.post(
        f"{base_url}/app/link/pair-start",
        json={"device_label": "offlan-e2e", "role": "phone"},
        timeout=10,
    )
    r.raise_for_status()
    return r.json()


def run_mobile_pair(pair_link: str, state_path: Path) -> subprocess.CompletedProcess:
    return subprocess.run(
        [
            "bun", "run", "src/index.ts", "pair",
            pair_link, "offlan-e2e-device",
            "--relay", RELAY,
            "--state", str(state_path),
        ],
        cwd=SPL_MOBILE,
        capture_output=True,
        text=True,
        timeout=60,
    )


def list_devices(base_url: str) -> list[dict]:
    r = requests.get(f"{base_url}/app/link/api/devices", timeout=10)
    r.raise_for_status()
    return r.json()["devices"]


def unpair(base_url: str, fp: str) -> dict:
    r = requests.post(f"{base_url}/app/link/unpair", json={"fingerprint": fp}, timeout=10)
    r.raise_for_status()
    return r.json()


async def dial_reach(state: dict) -> tuple[int, str]:
    """Dial via the production Python client built from the mobile's state."""
    from solstone.think.link.client import Client, ClientIdentity, EnrolledDevice

    identity = ClientIdentity(
        private_key_pem=state["client_key_pem"],
        client_cert_pem=state["client_cert"],
        ca_chain_pem="".join(state["ca_chain"]),
        fingerprint=state["fingerprint"],
        home_instance_id=state["instance_id"],
        home_label=state.get("home_label", ""),
        home_attestation="",
    )
    enrolled = EnrolledDevice(device_token=state["device_token"], identity=identity)
    session = await Client.dial(RELAY, enrolled)
    async with session:
        status, _headers, body = await session.request("GET", "/app/link/api/status")
    return status, body.decode("utf-8", "replace")


async def dial_expect_revoked(state: dict) -> str:
    from solstone.think.link.client import (
        Client,
        ClientIdentity,
        EnrolledDevice,
        StreamResetError,
    )

    identity = ClientIdentity(
        private_key_pem=state["client_key_pem"],
        client_cert_pem=state["client_cert"],
        ca_chain_pem="".join(state["ca_chain"]),
        fingerprint=state["fingerprint"],
        home_instance_id=state["instance_id"],
        home_label=state.get("home_label", ""),
        home_attestation="",
    )
    enrolled = EnrolledDevice(device_token=state["device_token"], identity=identity)
    session = await Client.dial(RELAY, enrolled)
    async with session:
        try:
            await session.request("GET", "/app/link/api/status")
            return "REACHED (revocation FAILED)"
        except StreamResetError:
            return "rejected at app layer (revocation holds)"


def main() -> int:
    state_path = JOURNAL.parent / "mobile-state.json"
    state_path.unlink(missing_ok=True)
    print(f"relay   = {RELAY}")
    print(f"journal = {JOURNAL}")
    print(f"mobile  = {SPL_MOBILE}")

    with convey_server() as base_url, link_service():
        print(f"convey  = {base_url}  (secure_listener on :7657)")

        # 1. open a pairing window + get the relay (0x03) QR
        ps = pair_start(base_url)
        pair_link = ps["pair_link"]
        nonce = ps["nonce"]
        print(f"\n[pair-start] expires_in={ps['expires_in']}s nonce={nonce[:8]}...")
        print(f"[pair-start] pair_link={pair_link}")
        assert pair_link.startswith("https://link.solpbc.org/p#"), "expected relay 0x03 QR"

        # 2. run the real L10 mobile client through the off-LAN ceremony
        t0 = time.monotonic()
        res = run_mobile_pair(pair_link, state_path)
        dt = time.monotonic() - t0
        print(f"\n[mobile pair] exit={res.returncode} ({dt:.1f}s)")
        print("[mobile stdout]\n" + res.stdout)
        if res.returncode != 0:
            print("[mobile stderr]\n" + res.stderr)
            return 1

        state = json.loads(state_path.read_text())
        assert state.get("device_token"), "no device_token in mobile state"
        fp = state["fingerprint"]
        print(f"[mobile] device_token len={len(state['device_token'])} fp={fp[:24]}...")

        # 3. home registered the device (CSR signed over the cert-less tunnel)
        devs = list_devices(base_url)
        assert any(d["fingerprint"] == fp for d in devs), f"device {fp} not registered at home"
        print(f"[home] /api/devices lists paired fp ({len(devs)} device(s))")

        # 4. reach proof: off-LAN-paired device dials + hits the journal
        status, body = asyncio.run(dial_reach(state))
        print(f"[reach] GET /app/link/api/status -> {status}")
        payload = json.loads(body)
        assert status == 200 and payload.get("instance_id") == state["instance_id"]
        print(f"[reach] journal reachable; instance_id matches")

        # 5. revocation (AC5)
        unpair(base_url, fp)
        time.sleep(1.0)  # mtime poll for authorized_clients.json
        verdict = asyncio.run(dial_expect_revoked(state))
        print(f"[revoke] {verdict}")
        assert "holds" in verdict

    print("\nOFF-LAN PAIR E2E: PASS")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
