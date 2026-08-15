#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
"""Reject release channel adapter host/reach leaks in tracked files."""

from __future__ import annotations

import ipaddress
import re
import subprocess
import sys
from collections.abc import Iterable, Sequence
from dataclasses import dataclass
from pathlib import Path
from typing import TypeAlias

ROOT = Path(__file__).resolve().parents[1]


def _parts(*pieces: str) -> str:
    return "".join(pieces)


@dataclass(frozen=True)
class Finding:
    path: str
    line: int
    tier: str
    component: str
    value: str
    detail: str

    def format(self) -> str:
        return (
            f"{self.path}:{self.line}: {self.tier} {self.component}: "
            f"{self.value} ({self.detail})"
        )


@dataclass(frozen=True)
class ScanStats:
    skipped_nul_binary: int = 0
    skipped_decode: int = 0


@dataclass(frozen=True)
class ScanResult:
    findings: tuple[Finding, ...]
    stats: ScanStats


@dataclass(frozen=True)
class TrackedEntry:
    mode: str
    object_id: str
    path: str


IPAddress: TypeAlias = ipaddress.IPv4Address | ipaddress.IPv6Address
IPNetwork: TypeAlias = ipaddress.IPv4Network | ipaddress.IPv6Network
IPExclusion: TypeAlias = IPAddress | IPNetwork


TIER1_VALUES = (
    _parts("pr", "o5", "e"),
    _parts("spark", "-", "a8", "a6"),
    _parts("tm", "ux", "-", "run"),
    _parts("automation", ":", "build", "-"),
)

TIER3_TERMS = (
    _parts("fed", "ora"),
    _parts("d", "gx"),
    _parts("j", "er"),
    _parts("nvidia", "-", "smi"),
    _parts("solstone", "-", "mac", "os"),
    _parts("sol", "-", "signing"),
    _parts("sol", "-", "pbc", "-", "notary"),
)

OCTET_RE = r"(?:25[0-5]|2[0-4][0-9]|1[0-9]{2}|[1-9][0-9]?|0)"
HOST_LABEL_RE = r"[A-Za-z0-9](?:[A-Za-z0-9-]{0,61}[A-Za-z0-9])?"
HOSTNAME_ALPHA_FINAL_RE = (
    rf"(?:{HOST_LABEL_RE}\.)+"
    rf"[A-Za-z](?:[A-Za-z0-9-]{{0,61}}[A-Za-z0-9])?"
)
IPV4_RE = re.compile(
    rf"(?<![A-Za-z0-9_.]){OCTET_RE}(?:\.{OCTET_RE}){{3}}(?![A-Za-z0-9_.])"
)
IPV6_RE = re.compile(
    r"(?<![A-Za-z0-9_.:-])"
    r"(?=[0-9A-Fa-f:.]*[0-9A-Fa-f])"
    r"(?:[0-9A-Fa-f]{0,4}:){2,}[0-9A-Fa-f:.]*"
    r"(?![A-Za-z0-9_.:-])"
)
USER_HOST_RE = re.compile(
    rf"(?<![A-Za-z0-9._%+-])"
    rf"[A-Za-z0-9._-]+@"
    rf"(?:{OCTET_RE}(?:\.{OCTET_RE}){{3}}|localhost|{HOSTNAME_ALPHA_FINAL_RE})"
    r"(?::[^\s\"'`,)\]}]+)?"
    rf"(?![A-Za-z0-9_.-])"
)

SHELL_PORT_RE = re.compile(
    rf"(?<![A-Za-z0-9_.-])(?:{_parts('s', 'sh')}|{_parts('s', 'cp')})"
    rf"(?![A-Za-z0-9_.-]).*(?:^|[\s\"'])-[pP][\s\"']+[0-9]+"
)
ARGV_PORT_RE = re.compile(
    rf"['\"](?:{_parts('s', 'sh')}|{_parts('s', 'cp')})['\"].*"
    rf"['\"]-[pP]['\"]\s*,\s*['\"][0-9]+['\"]"
)
SHELL_OPTION_PORT_RE = re.compile(
    rf"(?<![A-Za-z0-9_.-])(?:{_parts('s', 'sh')}|{_parts('s', 'cp')})"
    rf"(?![A-Za-z0-9_.-]).*(?:^|[\s\"'])-o\s*Port\s*=\s*[0-9]+",
    re.IGNORECASE,
)
ARGV_OPTION_PORT_RE = re.compile(
    rf"['\"](?:{_parts('s', 'sh')}|{_parts('s', 'cp')})['\"].*"
    rf"(?:['\"]-oPort=[0-9]+['\"]|"
    rf"['\"]-o['\"]\s*,\s*['\"]Port=[0-9]+['\"])",
    re.IGNORECASE,
)
PORT_PATTERNS = (
    ("-p/-P", SHELL_PORT_RE),
    ("argv -p/-P", ARGV_PORT_RE),
    ("-oPort=/-o Port=", SHELL_OPTION_PORT_RE),
    ("argv -oPort=/argv -o Port=", ARGV_OPTION_PORT_RE),
)

SSH_ARGV_RE = re.compile(
    rf"['\"](?:{_parts('s', 'sh')}|{_parts('s', 'cp')})['\"].*__TERM__"
)
SSH_SHELL_RE = re.compile(
    rf"(?<![A-Za-z0-9_.-])(?:{_parts('s', 'sh')}|{_parts('s', 'cp')})"
    rf"(?![A-Za-z0-9_.-]).*__TERM__"
)
USER_AT_TERM_RE = re.compile(
    rf"(?:^|[^A-Za-z0-9._-])__TERM__@"
    rf"(?:{OCTET_RE}(?:\.{OCTET_RE}){{3}}|[A-Za-z0-9-]+(?:\.local)?)"
    rf"(?![A-Za-z0-9_.:-])"
)
TERM_AT_HOST_RE = re.compile(
    r"@(?:[A-Za-z0-9-]+\.)*__TERM__(?:\.local)?(?![A-Za-z0-9_.:-])"
)
CONFIG_KEY_RE = re.compile(
    r"(?i)\b[A-Za-z0-9_]*(?:host|hostname|remote|reach|target|ssh|scp|"
    r"tmux_window|unlock_workdir|remote_work_prefix|build_window|macos_dir|workdir)"
    r"[A-Za-z0-9_]*\s*[:=]\s*['\"][^'\"]*__TERM__"
)
REMOTE_CALL_RE = re.compile(r"\b(?:ssh_run|scp_to|scp_from)\b.*__TERM__")

TIER3_COMPONENTS = (
    ("ssh-argv", SSH_ARGV_RE),
    ("ssh-shell", SSH_SHELL_RE),
    ("user-host", USER_AT_TERM_RE),
    ("host-user", TERM_AT_HOST_RE),
    ("config-value", CONFIG_KEY_RE),
    ("remote-call", REMOTE_CALL_RE),
)


DOCUMENTED_IP_LITERAL_EXCLUSIONS = {
    # RFC 6890: unspecified address used only for local bind/listener examples.
    ipaddress.IPv4Address("0.0.0.0"),
    # RFC 6890: loopback addresses.
    ipaddress.IPv4Network("127.0.0.0/8"),
    # RFC 1918: private-use network fixtures.
    ipaddress.IPv4Network("10.0.0.0/8"),
    # RFC 1918: private-use network fixtures.
    ipaddress.IPv4Network("172.16.0.0/12"),
    # RFC 1918: private-use network fixtures.
    ipaddress.IPv4Network("192.168.0.0/16"),
    # RFC 6598: shared address space fixtures.
    ipaddress.IPv4Network("100.64.0.0/10"),
    # RFC 3927: link-local address fixtures.
    ipaddress.IPv4Network("169.254.0.0/16"),
    # RFC 5771: multicast negative fixtures.
    ipaddress.IPv4Network("224.0.0.0/4"),
    # RFC 919/RFC 922: limited broadcast.
    ipaddress.IPv4Address("255.255.255.255"),
    # RFC 5737: documentation range.
    ipaddress.IPv4Network("192.0.2.0/24"),
    # RFC 5737: documentation range.
    ipaddress.IPv4Network("198.51.100.0/24"),
    # RFC 5737: documentation range.
    ipaddress.IPv4Network("203.0.113.0/24"),
    # Direct-pair admission boundary fixture values retained from legacy tests.
    # Adjacent-refused boundary below RFC 1918 10/8.
    ipaddress.IPv4Address("9.255.255.255"),
    # Adjacent-refused boundary above RFC 1918 10/8.
    ipaddress.IPv4Address("11.0.0.0"),
    # Adjacent-refused boundary below RFC 1918 172.16/12.
    ipaddress.IPv4Address("172.15.255.255"),
    # Adjacent-refused boundary above RFC 1918 172.16/12.
    ipaddress.IPv4Address("172.32.0.0"),
    # Adjacent-refused boundary below RFC 1918 192.168/16.
    ipaddress.IPv4Address("192.167.255.255"),
    # Adjacent-refused boundary above RFC 1918 192.168/16.
    ipaddress.IPv4Address("192.169.0.0"),
    # Adjacent-refused boundary below RFC 3927 link-local 169.254/16.
    ipaddress.IPv4Address("169.253.255.255"),
    # Adjacent-refused boundary above RFC 3927 link-local 169.254/16.
    ipaddress.IPv4Address("169.255.0.0"),
    # Adjacent-refused boundary below RFC 6598 shared address space 100.64/10.
    ipaddress.IPv4Address("100.63.255.255"),
    # Adjacent-refused boundary above RFC 6598 shared address space 100.64/10.
    ipaddress.IPv4Address("100.128.0.0"),
    # Adjacent-refused boundary below RFC 6890 loopback 127/8.
    ipaddress.IPv4Address("126.255.255.255"),
    # Adjacent-refused boundary above RFC 6890 loopback 127/8.
    ipaddress.IPv4Address("128.0.0.0"),
    # RFC 2544 benchmark refusal fixture.
    ipaddress.IPv4Address("198.18.0.1"),
    # Representative public-address refusal fixture.
    ipaddress.IPv4Address("1.1.1.1"),
}

DOCUMENTED_IP_VALUE_EXCLUSIONS = {
    # Python package version in the checked-in lockfile.
    ipaddress.IPv4Address("1.2.0.1"),
    # HTTP forwarding fixture.
    ipaddress.IPv4Address("1.2.3.4"),
    # Minified SVG path coordinate text.
    ipaddress.IPv4Address("2.95.6.6"),
    # Minified SVG path coordinate text.
    ipaddress.IPv4Address("3.5.7.7"),
    # Minified SVG path coordinate text.
    ipaddress.IPv4Address("4.5.8.8"),
    # Python package version in the checked-in lockfile.
    ipaddress.IPv4Address("4.13.0.92"),
    # Public DNS address used in routing diagnostics.
    ipaddress.IPv4Address("8.8.8.8"),
    # Python package version in the checked-in lockfile.
    ipaddress.IPv4Address("9.21.1.3"),
    # Python package version in the checked-in lockfile.
    ipaddress.IPv4Address("11.4.1.4"),
    # Python package version in the checked-in lockfile.
    ipaddress.IPv4Address("12.9.2.10"),
    # Package version fixture for CUDA runtime repacking.
    ipaddress.IPv4Address("13.5.1.27"),
    # Public-side boundary fixture for private-network classification.
    ipaddress.IPv4Address("172.32.0.1"),
}

DOCUMENTED_IPV6_LITERAL_EXCLUSIONS = {
    # RFC 4291: loopback address.
    ipaddress.IPv6Address("::1"),
    # RFC 3849: documentation range.
    ipaddress.IPv6Network("2001:db8::/32"),
    # RFC 4291: link-local range fixtures.
    ipaddress.IPv6Network("fe80::/10"),
    # RFC 4193: unique local address fixtures.
    ipaddress.IPv6Network("fc00::/7"),
    # RFC 4291: IPv4-mapped IPv6 range fixtures.
    ipaddress.IPv6Network("::ffff:0:0/96"),
    # RFC 4291: multicast negative fixtures.
    ipaddress.IPv6Network("ff00::/8"),
}

DOCUMENTED_USER_HOST_EXCLUSIONS = {
    # Negative home-address validation fixture; not a reachable adapter host.
    _parts("user", "@", "192.168.", "1.44", ":", "7657"),
    # Vendored license contact addresses retained verbatim as third-party text.
    _parts("ahmad.ahmad", "@", "kaust.edu", ".sa"),
    _parts("david.keyes", "@", "kaust.edu", ".sa"),
    _parts("hatem.ltaief", "@", "kaust.edu", ".sa"),
    # Upstream author attribution retained verbatim in bundled license text.
    _parts("alexander", "@", "bumpern", ".de"),
    # Public support and owner identity fixtures used by package/app metadata.
    _parts("j", "er", "@", "solpbc", ".org"),
    # Owner-facing and importer examples that must stay non-reserved placeholders.
    _parts("user", "@", "domain", ".com"),
    _parts("work", "@", "company", ".com"),
    # Fixture journal personas used by importer and chronicle contract tests.
    _parts("carlos", "@", "meridian", ".io"),
    _parts("david", "@", "betaworks", ".com"),
    _parts("erik", "@", "solpbc", ".org"),
    _parts("lin", "@", "solpbc", ".org"),
    _parts("maya", "@", "solpbc", ".org"),
    _parts("sarah.chen", "@", "whitfield-law", ".com"),
    # Unit-test personas for matching, merge, importer, and support workflows.
    _parts("a", "@", "b", ".com"),
    _parts("aj", "@", "work", ".com"),
    _parts("alice", "@", "acme", ".com"),
    _parts("alice", "@", "co", ".com"),
    _parts("alice", "@", "new", ".com"),
    _parts("alice", "@", "old", ".com"),
    _parts("bob", "@", "co", ".com"),
    _parts("bob", "@", "jones", ".io"),
    _parts("eve", "@", "megacorp", ".com"),
    _parts("jane", "@", "startup", ".co"),
    _parts("smug", "@", "x", ".com"),
}


def _address_is_excluded(
    address: IPAddress,
    literal_exclusions: Iterable[IPExclusion],
    value_exclusions: Iterable[IPAddress] = (),
) -> bool:
    for excluded in literal_exclusions:
        if excluded.version != address.version:
            continue
        if isinstance(excluded, (ipaddress.IPv4Network, ipaddress.IPv6Network)):
            if address in excluded:
                return True
        elif excluded == address:
            return True
    return address in value_exclusions


def _ipv4_is_excluded(value: str) -> bool:
    return _address_is_excluded(
        ipaddress.IPv4Address(value),
        DOCUMENTED_IP_LITERAL_EXCLUSIONS,
        DOCUMENTED_IP_VALUE_EXCLUSIONS,
    )


def _ipv6_is_excluded(address: ipaddress.IPv6Address) -> bool:
    return _address_is_excluded(address, DOCUMENTED_IPV6_LITERAL_EXCLUSIONS)


def _user_host_host(value: str) -> str:
    _user, target = value.split("@", 1)
    host, _sep, _suffix = target.partition(":")
    return host.lower().rstrip(".")


def _user_host_is_reserved(value: str) -> bool:
    host = _user_host_host(value)
    return (
        host == "localhost"
        or host.endswith((".example", ".test", ".invalid"))
        or host in {"example.com", "example.net", "example.org"}
    )


def _pattern_for_term(pattern: re.Pattern[str], term: str) -> re.Pattern[str]:
    return re.compile(
        pattern.pattern.replace("__TERM__", re.escape(term)), pattern.flags
    )


def scan_line(path: str, line_number: int, line: str) -> list[Finding]:
    findings: list[Finding] = []
    for value in TIER1_VALUES:
        if value in line:
            findings.append(
                Finding(path, line_number, "Tier-1", "literal", value, "banned value")
            )

    for match in IPV4_RE.finditer(line):
        value = match.group(0)
        if _ipv4_is_excluded(value):
            continue
        findings.append(
            Finding(
                path,
                line_number,
                "Tier-2",
                "ip-literal",
                value,
                "if this is a version string or coordinate rather than a host "
                "address, add it to the documented IP literal exclusion list "
                "with a one-line justification",
            )
        )

    for match in IPV6_RE.finditer(line):
        value = match.group(0)
        try:
            address = ipaddress.IPv6Address(value)
        except ValueError:
            continue
        if _ipv6_is_excluded(address):
            continue
        findings.append(
            Finding(
                path,
                line_number,
                "Tier-2",
                "ipv6-literal",
                value,
                "if this is a fixture rather than a host address, add it to "
                "the documented IPv6 literal exclusion list with a one-line "
                "justification",
            )
        )

    for match in USER_HOST_RE.finditer(line):
        value = match.group(0)
        if value in DOCUMENTED_USER_HOST_EXCLUSIONS or _user_host_is_reserved(value):
            continue
        findings.append(
            Finding(
                path,
                line_number,
                "Tier-2",
                "user-host",
                value,
                "replace reachable user/host literals with operator config",
            )
        )

    for label, pattern in PORT_PATTERNS:
        if pattern.search(line):
            findings.append(
                Finding(
                    path,
                    line_number,
                    "Tier-2",
                    "ssh-scp-port",
                    label,
                    "move SSH/SCP port values into operator config",
                )
            )
            break

    for term in TIER3_TERMS:
        if term not in line:
            continue
        for component, pattern in TIER3_COMPONENTS:
            if _pattern_for_term(pattern, term).search(line):
                findings.append(
                    Finding(
                        path,
                        line_number,
                        "Tier-3",
                        component,
                        term,
                        "move reach or host-specific construction into operator config",
                    )
                )
                break

    return findings


def scan_text(path: str, text: str) -> list[Finding]:
    findings: list[Finding] = []
    for line_number, line in enumerate(text.splitlines(), 1):
        findings.extend(scan_line(path, line_number, line))
    return findings


def tracked_entries(root: Path = ROOT) -> list[TrackedEntry]:
    result = subprocess.run(
        ["git", "ls-files", "-s"],
        cwd=root,
        check=True,
        capture_output=True,
        text=True,
    )
    entries: list[TrackedEntry] = []
    for line in result.stdout.splitlines():
        if not line:
            continue
        metadata, relative_path = line.split("\t", 1)
        mode, object_id, _stage = metadata.split()
        entries.append(TrackedEntry(mode, object_id, relative_path))
    return entries


def _read_symlink_target(root: Path, entry: TrackedEntry) -> str:
    result = subprocess.run(
        ["git", "cat-file", "-p", entry.object_id],
        cwd=root,
        check=True,
        capture_output=True,
        text=True,
    )
    return result.stdout


def _unreadable_finding(path: str, exc: OSError) -> Finding:
    errno = exc.errno if exc.errno is not None else "unknown"
    return Finding(
        path,
        1,
        "Tier-2",
        "tracked-io",
        path,
        f"tracked file could not be read (errno {errno})",
    )


def scan_paths(root: Path, entries: Iterable[TrackedEntry]) -> ScanResult:
    findings: list[Finding] = []
    skipped_nul_binary = 0
    skipped_decode = 0
    for entry in entries:
        relative_path = entry.path
        if entry.mode == "120000":
            findings.extend(
                scan_line(relative_path, 1, _read_symlink_target(root, entry))
            )
            continue
        path = root / relative_path
        if not path.exists():
            continue
        try:
            data = path.read_bytes()
        except OSError as exc:
            findings.append(_unreadable_finding(relative_path, exc))
            continue
        if b"\0" in data:
            skipped_nul_binary += 1
            continue
        try:
            text = data.decode("utf-8")
        except UnicodeDecodeError:
            skipped_decode += 1
            continue
        findings.extend(scan_text(relative_path, text))
    return ScanResult(
        tuple(findings),
        ScanStats(
            skipped_nul_binary=skipped_nul_binary,
            skipped_decode=skipped_decode,
        ),
    )


def main(argv: Sequence[str] | None = None) -> int:
    _ = argv
    result = scan_paths(ROOT, tracked_entries(ROOT))
    if result.findings:
        sys.stderr.write("Channel adapter scrub found host/reach literals:\n")
        for finding in result.findings:
            sys.stderr.write(f"  {finding.format()}\n")
        stats = result.stats
        sys.stderr.write(
            "Skipped tracked files: "
            f"NUL-binary={stats.skipped_nul_binary}, "
            f"decode={stats.skipped_decode}\n"
        )
        return 1
    stats = result.stats
    print(
        "Channel adapter scrub passed "
        f"(skipped NUL-binary={stats.skipped_nul_binary}, "
        f"decode={stats.skipped_decode})."
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
