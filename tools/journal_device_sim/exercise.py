# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Finite checks of the ordinary native device client, beyond fixture upload."""

from __future__ import annotations

import hashlib
import time
from collections.abc import Callable
from typing import Any

from .http_client import BridgeHttpClient, HttpRequestError, HttpResponseError
from .process import LinkBridge


class ExerciseFailure(RuntimeError):
    """An observed client result violates the exercised contract."""

    def __init__(self, message: str, details: dict[str, Any] | None = None) -> None:
        self.details = details or {}
        super().__init__(message)


class ExerciseInconclusive(HttpRequestError):
    def __init__(self, message: str, details: dict[str, Any]) -> None:
        self.details = details
        super().__init__(message)


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ExerciseFailure(message)


class DeviceExercise:
    def __init__(self, bridge: LinkBridge, client: BridgeHttpClient, *,
                 instance_id: str, cid: str, timeout: float, poll_interval: float,
                 receipts: list[dict[str, Any]], progress: Callable[..., None]) -> None:
        self.bridge, self.client = bridge, client
        self.instance_id, self.cid = instance_id, cid
        self.timeout, self.poll_interval = timeout, poll_interval
        self.receipts, self.progress = receipts, progress

    def step(self, name: str, operation: Callable[[], dict[str, Any]]) -> None:
        receipt: dict[str, Any] = {"name": name, "result": "RUNNING"}
        self.receipts.append(receipt)
        started = time.monotonic()
        self.progress("check.started", check=name)
        try:
            receipt.update(operation())
            receipt["result"] = "PASS"
        except BaseException as error:
            if isinstance(error, (ExerciseFailure, ExerciseInconclusive)):
                receipt.update(error.details)
            receipt["result"] = "INCOMPLETE"
            raise
        finally:
            receipt["elapsed_seconds"] = round(time.monotonic() - started, 3)
            self.progress("check.finished", check=name, result=receipt["result"])

    def identity(self) -> dict[str, Any]:
        identity = self.client.get_json("/app/link/api/identity")
        require(identity.status == 200 and identity.body.get("committed") is True
                and identity.body.get("instance_id") == self.instance_id,
                "device reached a different or uncommitted journal")
        clients = self.client.get_json("/app/network/api/clients")
        entries = clients.body.get("clients")
        require(clients.status == 200 and isinstance(entries, list)
                and any(isinstance(row, dict) and row.get("cid") == self.cid for row in entries),
                "authenticated client CID is absent from the journal's client listing")
        require(self.bridge.provenance["credentials"]["client_cid"] == self.cid,
                "native bridge restarted with another certificate")
        return {"instance_id": self.instance_id, "client_cid": self.cid}

    def metadata(self) -> dict[str, Any]:
        deadline = time.monotonic() + self.timeout
        while True:
            reply = self.client.get_json("/app/network/api/clients/self")
            require(reply.status == 200, "authenticated device description was refused")
            body = reply.body
            reported = body.get("reported")
            if isinstance(reported, dict):
                require(body.get("protocol_version") == 1 and isinstance(body.get("revision"), int)
                        and body["revision"] > 0 and isinstance(body.get("journal"), dict)
                        and bool(body["journal"].get("version"))
                        and all(isinstance(reported.get(key), str) and bool(reported[key])
                                for key in ("platform", "app_id", "app_version")),
                        "device metadata contract is incomplete")
                status = self.client.get_json("/_solstone/link/status")
                if (status.status == 200 and status.body.get("journal_version_fresh") is True
                        and status.body.get("journal_version") == body["journal"]["version"]):
                    return {"revision": body["revision"], "journal_version": body["journal"]["version"],
                            "journal_version_fresh": True}
            if time.monotonic() >= deadline:
                raise HttpRequestError("native device description publication did not complete")
            time.sleep(min(self.poll_interval, max(0, deadline - time.monotonic())))

    def cli(self, args: list[str], *, identity: bool = False) -> dict[str, Any]:
        code, stdout, stderr, failure = self.bridge.run_cli(args)
        receipt = {"command": args, "exit_code": code, "failure": failure,
                   "stdout_bytes": len(stdout), "stdout_sha256": hashlib.sha256(stdout).hexdigest(),
                   "stderr_bytes": len(stderr), "stderr_sha256": hashlib.sha256(stderr).hexdigest()}
        if failure is not None or code != 0:
            raise ExerciseFailure("native CLI operation failed through the device bridge", receipt)
        if identity:
            require(self.instance_id.encode() in stdout, "native CLI returned another journal identity")
        return receipt

    def shell(self) -> dict[str, Any]:
        receipt = self.client.check_dashboard()
        shell = self.client.get_json("/api/shell")
        require(shell.status == 200 and isinstance(shell.body.get("clock"), dict)
                and bool(shell.body["clock"].get("today")), "dashboard shell API is incomplete")
        return receipt

    def reconnect(self) -> dict[str, Any]:
        self.bridge.stop()
        address = self.bridge.start()
        self.client = BridgeHttpClient(address, timeout=self.timeout)
        return self.identity()

    def revoke(self) -> dict[str, Any]:
        self.identity()  # Establish a working authenticated path immediately before removal.
        code, stdout, stderr, failure = self.bridge.run_cli(["call", "link", "unpair", self.cid])
        receipt: dict[str, Any] = {"client_cid": self.cid, "cli_exit_code": code,
                                   "cli_failure": failure, "cli_stdout_bytes": len(stdout),
                                   "cli_stderr_bytes": len(stderr)}
        # Removal can close the carrier before its CLI response arrives. Only
        # the native TLS access-denied latch reconciles that uncertain result.
        deadline = time.monotonic() + self.timeout
        while True:
            try:
                refused_status = self.client.get_json("/app/link/api/identity").status
            except HttpResponseError as error:
                # The native bridge's local 502 body is deliberately plain text.
                refused_status = error.status
                receipt["refusal"] = error.receipt
            if refused_status == 200:
                raise ExerciseFailure("unpaired client can still read the journal")
            state = self.client.get_json("/_solstone/link/status")
            body = state.body
            reason = body.get("terminal_reason")
            receipt.update({"upstream_status": refused_status, "state": body.get("state"),
                            "manager_alive": body.get("manager_alive"), "terminal_reason": reason,
                            "instance_id": body.get("instance_id")})
            if (state.status == 200 and body.get("manager_alive") is True
                    and body.get("instance_id") == self.instance_id
                    and body.get("state") == "unpaired" and reason == "tls-access-denied"):
                return {**receipt, "upstream_status": refused_status,
                        "state": body["state"], "terminal_reason": reason, "manager_alive": True}
            if time.monotonic() >= deadline:
                raise ExerciseInconclusive("revocation lacks the native TLS access-denied proof", receipt)
            time.sleep(min(self.poll_interval, max(0, deadline - time.monotonic())))

    def run(self, reconcile: Callable[[BridgeHttpClient], None]) -> None:
        self.step("device_identity", self.identity)
        self.step("device_metadata", self.metadata)
        self.step("dashboard", self.shell)
        self.step("dashboard_events", self.client.check_events)
        self.step("native_cli", lambda: self.cli(["call", "link", "status"], identity=True))
        self.step("reconnect", self.reconnect)
        self.step("reconnected_metadata", self.metadata)
        self.step("reconnected_cli", lambda: self.cli(["call", "link", "status"], identity=True))
        def listing() -> dict[str, Any]:
            reconcile(self.client)
            return {"reconciled": True}
        self.step("reconnected_segments", listing)
        self.step("self_revocation", self.revoke)
