#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""A publication guard for captured conformance corpora.

🔴 **These fixtures ship in a public repository. Whatever a generator captures,
we publish.** A corpus is produced by driving a reference application on a
developer's machine, and any value the reference reads from the host rather than
from the seeded journal travels into the committed file: a LAN address, a disk
geometry, a home path, a hostname, a username.

⚠ **This is a publication guard, not a hermeticity fix, and the two are not
interchangeable.** The fix for an observed value is to *author* it — establish
the condition at the call site so the generator hands the reference a fixed
value — and then prove it with two runs that produce byte-identical output.
Normalizing after the fact hides the value; authoring it means there is nothing
to hide. This guard exists to catch the case where a generator's author did
neither and did not notice.

⚠ **A hit here is a shape match, never a provenance claim.** A matched literal
may be perfectly legitimate — an authored fixture address, a documentation
example, a loopback constant. That is what `allowed` is for: naming it converts
"this looks like host data" into "this value was chosen deliberately", and the
naming is the review record. ⛔ Do not widen a pattern to silence a hit; add the
literal, or author the value away.
"""

from __future__ import annotations

import getpass
import ipaddress
import os
import platform
import re
import socket
from collections.abc import Iterable

# Any dotted quad. Loopback and the unspecified address are always fine: they are
# constants of the protocol, not facts about a machine.
_IPV4_RE = re.compile(r"\b\d{1,3}(?:\.\d{1,3}){3}\b")
_ALWAYS_ALLOWED_IPV4 = frozenset({"127.0.0.1", "0.0.0.0", "255.255.255.255"})

# A POSIX home directory of a real account.
_HOME_RE = re.compile(r"/(?:home|Users)/[A-Za-z0-9._-]+")


def _host_identifiers() -> set[str]:
    """Names that identify the generating machine or the account running it."""
    identifiers: set[str] = set()
    try:
        identifiers.add(socket.gethostname())
    except OSError:
        pass
    try:
        identifiers.add(platform.node())
    except Exception:  # noqa: BLE001 - platform.node is best-effort by contract
        pass
    try:
        identifiers.add(getpass.getuser())
    except Exception:  # noqa: BLE001 - getuser raises broadly when no account maps
        pass
    for name in ("USER", "LOGNAME", "USERNAME", "HOSTNAME"):
        value = os.environ.get(name)
        if value:
            identifiers.add(value)
    home = os.environ.get("HOME")
    if home:
        identifiers.add(home)
    # An empty or one-character identifier would match nearly any document.
    return {value for value in identifiers if value and len(value) > 2}


def assert_publishable(
    rendered: str,
    *,
    label: str,
    allowed_ipv4: Iterable[str] = (),
    allowed_paths: Iterable[str] = (),
) -> None:
    """Raise if a rendered corpus carries anything host-identifying.

    Args:
        rendered: the exact text about to be written to the fixture.
        label: the corpus name, for the error message.
        allowed_ipv4: dotted quads this corpus authors deliberately.
        allowed_paths: home-shaped paths this corpus authors deliberately.

    Raises:
        RuntimeError: naming every disallowed literal found.
    """
    allowed_addresses = _ALWAYS_ALLOWED_IPV4 | set(allowed_ipv4)
    allowed_home_paths = set(allowed_paths)
    findings: list[str] = []

    addresses = sorted(
        {
            match.group(0)
            for match in _IPV4_RE.finditer(rendered)
            if match.group(0) not in allowed_addresses
        }
    )
    if addresses:
        findings.append("IPv4 literals: " + ", ".join(addresses))

    paths = sorted(
        {
            match.group(0)
            for match in _HOME_RE.finditer(rendered)
            if match.group(0) not in allowed_home_paths
        }
    )
    if paths:
        findings.append("home-shaped paths: " + ", ".join(paths))

    identifiers = sorted(
        identifier for identifier in _host_identifiers() if identifier in rendered
    )
    if identifiers:
        findings.append("host/account identifiers: " + ", ".join(identifiers))

    if findings:
        raise RuntimeError(
            f"{label} is not publishable — "
            + "; ".join(findings)
            + ". Author the value at its call site rather than normalizing it away, "
            "or add it to the corpus's explicit allowlist."
        )


class EgressAttempted(RuntimeError):
    """Raised when a probed reference route tried to leave the machine."""


# Every blocked destination, in order. 🔴 This exists because "the capture came
# out byte-identical with the guard armed" is NOT proof that nothing tried: a
# route could attempt an outbound call, swallow the exception, and answer exactly
# as before — which would mean the *unguarded* run egressed. Only an empty
# attempt log rules that out.
_EGRESS_ATTEMPTS: list[str] = []


def egress_attempts() -> list[str]:
    """Return every non-loopback destination the guard blocked this run."""
    return list(_EGRESS_ATTEMPTS)


def assert_no_egress_attempted(label: str, *, ignore: Iterable[str] = ()) -> None:
    """Raise unless the guard blocked nothing, so a swallowed attempt cannot hide.

    Args:
        label: the corpus name, for the error message.
        ignore: destinations recorded by the guard's own positive control.
    """
    ignored = set(ignore)
    attempts = sorted({entry for entry in _EGRESS_ATTEMPTS if entry not in ignored})
    if attempts:
        raise RuntimeError(
            f"{label}: a probed reference route ATTEMPTED to reach "
            + ", ".join(attempts)
            + ". The guard blocked it, but the route is not read-only — treat "
            "probing it as a mutation and stub the service at its configured seam."
        )


def forbid_non_loopback_egress() -> None:
    """Make any non-loopback network call raise instead of leaving the host.

    🔴 **"Capture is read-only" is an assumption, not a property.** A reference
    route can look like a read and still register with a live service: driving
    one app's list endpoint on a throwaway journal generated a keypair, signed a
    live terms document and POSTed a signup to a production host. A route
    expected to refuse is not a route that stayed local — a blueprint-level
    `before_request` or `after_request` hook runs on refusals too.

    ✅ So the harness *establishes* that egress is impossible rather than
    reasoning about which routes reach out. Loopback stays open: the reference
    talks to local sockets (callosum, a loopback listener) as ordinary operation.

    ⚠ This binds the *process*, so it must be called before the reference is
    imported and it stays in force for the run. Pair it with
    `assert_egress_guard_can_see`, or a clean run proves nothing.
    """
    real_connect = socket.socket.connect
    real_connect_ex = socket.socket.connect_ex
    real_getaddrinfo = socket.getaddrinfo

    def _is_loopback(host: object) -> bool:
        if not isinstance(host, str):
            return False
        if host in ("localhost", ""):
            return True
        try:
            return ipaddress.ip_address(host).is_loopback
        except ValueError:
            return False

    def _check(address: object) -> None:
        # AF_UNIX addresses are plain paths, and they never leave the machine.
        if isinstance(address, (bytes, str)):
            return
        if isinstance(address, tuple) and address and not _is_loopback(address[0]):
            _EGRESS_ATTEMPTS.append(str(address[0]))
            raise EgressAttempted(
                f"a probed reference route attempted a non-loopback connection to {address[0]!r}; "
                "treat that route as a MUTATION until proven otherwise"
            )

    def guarded_connect(self: socket.socket, address: object):  # type: ignore[no-untyped-def]
        _check(address)
        return real_connect(self, address)  # type: ignore[arg-type]

    def guarded_connect_ex(self: socket.socket, address: object):  # type: ignore[no-untyped-def]
        _check(address)
        return real_connect_ex(self, address)  # type: ignore[arg-type]

    def guarded_getaddrinfo(host: object, *args: object, **kwargs: object):  # type: ignore[no-untyped-def]
        if not _is_loopback(host):
            _EGRESS_ATTEMPTS.append(str(host))
            raise EgressAttempted(
                f"a probed reference route attempted to resolve {host!r}; "
                "treat that route as a MUTATION until proven otherwise"
            )
        return real_getaddrinfo(host, *args, **kwargs)  # type: ignore[arg-type]

    socket.socket.connect = guarded_connect  # type: ignore[method-assign]
    socket.socket.connect_ex = guarded_connect_ex  # type: ignore[method-assign]
    socket.getaddrinfo = guarded_getaddrinfo  # type: ignore[assignment]


def assert_egress_guard_can_see(label: str) -> None:
    """Positive control: prove the egress guard refuses a real outbound call.

    🔴 A guard that is not actually installed produces a clean run for exactly
    the same reason a guarded one does.
    """
    try:
        socket.getaddrinfo("example.invalid", 443)
    except EgressAttempted:
        pass
    else:
        raise RuntimeError(
            f"{label} egress guard did NOT fire on an outbound resolution; "
            "a clean capture proves nothing about egress"
        )
    probe = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    try:
        probe.connect(("198.51.100.7", 443))
    except EgressAttempted:
        return
    except OSError as error:  # pragma: no cover - only on a host that refuses first
        raise RuntimeError(
            f"{label} egress guard was bypassed: the connection reached the network stack ({error})"
        ) from error
    finally:
        probe.close()
    raise RuntimeError(
        f"{label} egress guard did NOT fire on an outbound connection; "
        "a clean capture proves nothing about egress"
    )


def assert_guard_can_see(label: str) -> None:
    """Positive control: prove the guard fires on a value it must catch.

    🔴 A publication guard that cannot detect a planted leak reports a clean
    corpus for exactly the same reason as a clean one, and the two are
    indistinguishable from the exit status. This runs before every real check.
    """
    planted = '{"address": "203.0.113.9", "home": "/home/planted-control"}'
    try:
        assert_publishable(planted, label=f"{label} positive control")
    except RuntimeError as error:
        message = str(error)
        if "203.0.113.9" in message and "/home/planted-control" in message:
            return
        raise RuntimeError(
            f"{label} publication guard fired but did not name the planted values: {message}"
        ) from error
    raise RuntimeError(
        f"{label} publication guard did NOT fire on a planted leak; "
        "a clean result from it means nothing"
    )
