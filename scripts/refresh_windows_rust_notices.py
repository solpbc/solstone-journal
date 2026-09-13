#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Refresh the Windows Rust dependency notices after `core/Cargo.lock` moves.

`produce::windows_cli::tests::committed_rust_notices_match_workspace_lock`
(`core/crates/solstone-core-distribution/src/produce/windows_cli.rs`) binds
`core/distribution/windows-rust-sources.json` to the exact `Cargo.lock` it
describes. Any lock change reds that test until the index, and the ~99 MB
`dependency_source_companion` vendor-source archive it points at, are
regenerated for the new lock. This script does that regeneration for the
one case where it is cheap and safe: the external (non-workspace) dependency
population is unchanged, so the archive can be rebuilt by copying every
vendored member across untouched and substituting only `Cargo.lock` itself.

It refuses -- loudly, with the reason -- rather than proceed, when:
  * the external package population changed (an add/remove/upgrade/re-source).
    That needs a fresh `cargo vendor` acquisition against the new lock, which
    this script does not perform: it needs network egress to fetch the
    changed crates' sources, which is a venue question for the caller, not
    something to improvise here.
  * the resolved dependency/feature graph changed even though the population
    did not (a feature flag moved) -- same refusal, same reason.
  * a workspace member changed by more than its own version number.

On success it writes the new archive plus a small report to `--out`, and
rewrites `core/distribution/windows-rust-sources.json` in place. It also
embeds the resolved dependency graph it verified into the new archive as
`resolved-dependency-graph.json`, so the *next* refresh can read it straight
back out instead of depending on a `cargo metadata` capture kept somewhere
outside this repo. `--prior-metadata` exists only to bootstrap the one
archive produced before this script existed.

Usage:
    python3 scripts/refresh_windows_rust_notices.py \\
        --prior-archive /path/to/windows-rust-dependencies-<oldhash>.tar.gz \\
        [--prior-metadata /path/to/old-cargo-metadata.json] \\
        [--out /path/to/output/dir]
"""

from __future__ import annotations

import argparse
import copy
import datetime
import gzip
import hashlib
import io
import json
import re
import subprocess
import sys
import tarfile
import tempfile
import tomllib
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parent.parent
FILTER_PLATFORM = "x86_64-pc-windows-msvc"
GRAPH_MEMBER_NAME = "resolved-dependency-graph.json"
INDEX_RELATIVE_PATH = "core/distribution/windows-rust-sources.json"
NOTICES_RELATIVE_PATH = "core/distribution/windows-rust-NOTICES.txt"
LOCK_RELATIVE_PATH = "core/Cargo.lock"
INVENTORY_RELATIVE_PATH = "core/distribution/inventory.toml"

# Operator-internal fragments that must never end up in a file this repo
# carries (VPE index § operating principle 8: no extro/office/session paths
# on public surface). Structural patterns, not a specific incident's names.
# Plain substrings only for path/host fragments that cannot collide with a
# real crate name; a hopper/session id (`req_igym6lkc`, `req-ymiemtwm`) needs
# a word-boundaried regex instead, because a bare "req-" substring match is a
# false positive against real crates (e.g. `ureq`, `ureq-proto`).
FORBIDDEN_PATH_FRAGMENTS = (
    "/home/",
    "/var/tmp/",
    "/data/vartmp/",
    "sol-winbuild",
    "fedora.local",
    "suze.local",
)
FORBIDDEN_ID_PATTERN = re.compile(rb"(?<![a-z0-9])req[_-][a-z0-9]{4,}")


class RefreshError(Exception):
    """A clear, actionable refusal -- never a bare assert with no message."""


def sha256_file(path: Path) -> str:
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def load_index(repo: Path) -> dict[str, Any]:
    return json.loads((repo / INDEX_RELATIVE_PATH).read_bytes())


def query_cargo_metadata(repo: Path) -> dict[str, Any]:
    result = subprocess.run(
        [
            "cargo",
            "metadata",
            "--manifest-path",
            str(repo / "core/Cargo.toml"),
            "--locked",
            "--offline",
            "--format-version",
            "1",
            "--filter-platform",
            FILTER_PLATFORM,
        ],
        capture_output=True,
        check=False,
    )
    if result.returncode != 0:
        raise RefreshError(
            "cargo metadata failed against the current lock (exit "
            f"{result.returncode}): {result.stderr.decode(errors='replace').strip()}\n"
            "repair: run 'cargo fetch --locked --manifest-path core/Cargo.toml' "
            "once in this checkout (the offline resolve needs every crate in "
            "the lockfile cached, including ones that never build here), then retry."
        )
    return json.loads(result.stdout)


def normalize_graph(meta: dict[str, Any]) -> dict[str, Any]:
    """The resolved dependency/feature graph, keyed by identity rather than
    by cargo metadata's checkout-path-bearing package id -- two checkouts of
    the same lock normalize identically even though their ids differ."""
    packages = {p["id"]: p for p in meta["packages"]}

    def key(pkg_id: str) -> str:
        p = packages[pkg_id]
        if p["source"]:
            return f"{p['name']}@{p['version']} ({p['source']})"
        return f"workspace:{p['name']}"

    normalized: dict[str, Any] = {}
    for node in meta["resolve"]["nodes"]:
        deps = [
            {"name": d["name"], "pkg": key(d["pkg"]), "dep_kinds": d["dep_kinds"]}
            for d in node["deps"]
        ]
        normalized[key(node["id"])] = {
            "features": sorted(node["features"]),
            "deps": sorted(deps, key=lambda d: (d["name"], d["pkg"])),
        }
    return normalized


def load_prior_graph(
    prior_archive: Path, prior_metadata: Path | None
) -> dict[str, Any]:
    with tarfile.open(prior_archive, "r:gz") as tar:
        try:
            member = tar.getmember(GRAPH_MEMBER_NAME)
        except KeyError:
            member = None
        if member is not None:
            payload = json.loads(tar.extractfile(member).read())
            return payload["graph"]
    if prior_metadata is None:
        raise RefreshError(
            f"--prior-archive has no embedded '{GRAPH_MEMBER_NAME}' member (it "
            "predates this tool) and no --prior-metadata was supplied. Without "
            "one of the two, the resolved dependency/feature graph cannot be "
            "compared, and a refresh cannot prove itself version-only. Supply "
            "--prior-metadata pointing at the 'cargo metadata' JSON captured "
            "when --prior-archive was produced."
        )
    return normalize_graph(json.loads(prior_metadata.read_bytes()))


def windows_roots(repo: Path) -> list[str]:
    inventory = tomllib.loads((repo / INVENTORY_RELATIVE_PATH).read_text())
    return sorted(
        {
            e["package"]
            for e in inventory["entry"]
            if e["kind"] == "bin" and "windows-x86_64" in e.get("targets", [])
        }
    )


def selected_external_identities(meta: dict[str, Any], roots: list[str]) -> set[str]:
    packages = {p["id"]: p for p in meta["packages"]}
    nodes = {n["id"]: n for n in meta["resolve"]["nodes"]}
    pending = [
        p["id"] for p in packages.values() if not p["source"] and p["name"] in roots
    ]
    if len(pending) != len(roots):
        raise RefreshError(
            f"expected one workspace package per Windows inventory root {roots}, "
            f"found {len(pending)} in cargo metadata"
        )
    seen: set[str] = set()
    while pending:
        pkg_id = pending.pop()
        if pkg_id in seen:
            continue
        seen.add(pkg_id)
        for dep in nodes[pkg_id]["deps"]:
            if any(kind["kind"] != "dev" for kind in dep["dep_kinds"]):
                pending.append(dep["pkg"])
    return {
        f"{packages[pkg_id]['name']}@{packages[pkg_id]['version']} ({packages[pkg_id]['source']})"
        for pkg_id in seen
        if packages[pkg_id]["source"]
    }


def workspace_only_version_delta(
    old_lock: dict[str, Any], new_lock: dict[str, Any]
) -> list[str]:
    """Assert every workspace package is unchanged except (optionally) its own
    version -- no hardcoded version strings, unlike the prior-art scripts this
    generalizes. Any other difference refuses."""
    old_by_name = {p["name"]: p for p in old_lock["package"] if not p.get("source")}
    new_by_name = {p["name"]: p for p in new_lock["package"] if not p.get("source")}
    if set(old_by_name) != set(new_by_name):
        raise RefreshError(
            "the workspace package set changed (a crate was added or removed); "
            "this tool only refreshes a version-only lock move -- scope this as "
            "engineering work instead"
        )
    delta = []
    for name, old_pkg in old_by_name.items():
        new_pkg = new_by_name[name]
        if old_pkg == new_pkg:
            continue
        left, right = dict(old_pkg), dict(new_pkg)
        left.pop("version", None)
        right.pop("version", None)
        if left != right:
            raise RefreshError(
                f"workspace package '{name}' changed by more than its version; "
                "this tool only refreshes a version-only lock move -- scope this "
                "as engineering work instead"
            )
        delta.append(name)
    return delta


def refresh(
    repo: Path,
    prior_archive_path: Path,
    prior_metadata_path: Path | None,
    out_dir: Path,
) -> dict[str, Any]:
    old = load_index(repo)

    notices = (repo / NOTICES_RELATIVE_PATH).read_bytes()
    if sha256_bytes(notices) != old["notices_sha256"]:
        raise RefreshError(
            f"{NOTICES_RELATIVE_PATH} does not match the committed index's "
            "notices_sha256 -- it was edited out of band"
        )

    companion = old["dependency_source_companion"]
    if (
        sha256_file(prior_archive_path) != companion["sha256"]
        or prior_archive_path.stat().st_size != companion["bytes"]
    ):
        raise RefreshError(
            "--prior-archive does not match the committed dependency_source_companion "
            f"({companion['filename']}, {companion['bytes']} bytes, sha256 "
            f"{companion['sha256']})"
        )

    lock_bytes = (repo / LOCK_RELATIVE_PATH).read_bytes()
    lock_hash = sha256_bytes(lock_bytes)

    with tarfile.open(prior_archive_path, "r:gz") as tar:
        prior_lock_bytes = tar.extractfile("Cargo.lock").read()
    if sha256_bytes(prior_lock_bytes) != old["cargo_lock_sha256"]:
        raise RefreshError(
            "the prior archive's own Cargo.lock does not match the committed "
            "index's cargo_lock_sha256"
        )

    old_lock = tomllib.loads(prior_lock_bytes.decode())
    new_lock = tomllib.loads(lock_bytes.decode())

    old_external = [p for p in old_lock["package"] if p.get("source")]
    new_external = [p for p in new_lock["package"] if p.get("source")]
    if old_external != new_external:
        raise RefreshError(
            "the external (non-workspace) package population changed -- an "
            "added, removed, upgraded, or re-sourced dependency. A "
            "member-preserving refresh cannot attest to vendor bytes nobody "
            "produced; this needs a fresh `cargo vendor` acquisition against "
            "the new lock, not this tool. If this session has no venue for "
            "that (network egress to fetch the changed crates' sources, plus "
            "disk for the vendor tree), stop here and say so rather than "
            "improvising one."
        )

    workspace_delta = workspace_only_version_delta(old_lock, new_lock)

    old_graph = load_prior_graph(prior_archive_path, prior_metadata_path)
    new_meta = query_cargo_metadata(repo)
    new_graph = normalize_graph(new_meta)
    if old_graph != new_graph:
        raise RefreshError(
            "the resolved dependency/feature graph changed even though the "
            "external package population did not (a feature flag or "
            "dependency edge moved). Fresh acquisition required; refusing "
            "rather than publish an attestation for an unverified graph."
        )

    roots = windows_roots(repo)
    selected = selected_external_identities(new_meta, roots)
    wanted = {p["identity"] for p in old["packages"] if p["windows_notice_population"]}
    if selected != wanted:
        raise RefreshError(
            "the Windows notice population computed from the current lock "
            f"does not match the committed index: missing={sorted(wanted - selected)} "
            f"added={sorted(selected - wanted)}"
        )
    all_identities = {p["identity"] for p in old["packages"]}
    if not selected < all_identities:
        raise RefreshError(
            "negative control failed: the selected population is not a proper "
            "subset of the full lock -- the root/edge walk may be selecting everything"
        )

    for row in old["notice_texts"]:
        raw = notices[row["byte_start_inclusive"] : row["byte_end_exclusive"]]
        if len(raw) != row["bytes"] or sha256_bytes(raw) != row["sha256"]:
            raise RefreshError(
                "a notice text span starting at byte "
                f"{row['byte_start_inclusive']} no longer matches the committed NOTICES file"
            )

    out_dir.mkdir(parents=True, exist_ok=True)
    new_archive_path = out_dir / f"windows-rust-dependencies-{lock_hash[:16]}.tar.gz"
    if new_archive_path.exists():
        raise RefreshError(f"refusing to overwrite existing {new_archive_path}")

    graph_member_payload = (
        json.dumps(
            {
                "cargo_lock_sha256": lock_hash,
                "filter_platform": FILTER_PLATFORM,
                "graph": new_graph,
            },
            indent=2,
            sort_keys=True,
        ).encode()
        + b"\n"
    )

    prior_members: dict[str, dict[str, Any]] = {}
    with (
        tarfile.open(prior_archive_path, "r:gz") as source,
        new_archive_path.open("xb") as raw,
        gzip.GzipFile(
            filename="", mode="wb", fileobj=raw, mtime=0, compresslevel=6
        ) as gz,
        tarfile.open(fileobj=gz, mode="w|", format=tarfile.PAX_FORMAT) as dest,
    ):
        for member in source:
            if not (member.isfile() or member.isdir()):
                raise RefreshError(f"unexpected non-regular tar member: {member.name}")
            if member.name.startswith("/") or ".." in Path(member.name).parts:
                raise RefreshError(f"unsafe tar member path: {member.name}")
            if member.name in prior_members:
                raise RefreshError(f"duplicate tar member: {member.name}")
            data = source.extractfile(member).read() if member.isfile() else None
            prior_members[member.name] = {
                "mode": member.mode,
                "size": member.size,
                "sha256": sha256_bytes(data) if data is not None else None,
            }
            if member.name == "Cargo.lock":
                data = lock_bytes
                member.size = len(lock_bytes)
            dest.addfile(member, io.BytesIO(data) if data is not None else None)

        graph_info = tarfile.TarInfo(GRAPH_MEMBER_NAME)
        graph_info.size = len(graph_member_payload)
        graph_info.mtime = 0
        dest.addfile(graph_info, io.BytesIO(graph_member_payload))

    observed: dict[str, dict[str, Any]] = {}
    with tarfile.open(new_archive_path, "r:gz") as tar:
        for member in tar:
            data = tar.extractfile(member).read() if member.isfile() else None
            observed[member.name] = {
                "mode": member.mode,
                "size": member.size,
                "sha256": sha256_bytes(data) if data is not None else None,
            }

    removed = set(prior_members) - set(observed)
    if removed:
        raise RefreshError(
            f"the refresh dropped members that must be preserved: {sorted(removed)}"
        )
    added = set(observed) - set(prior_members)
    if added - {GRAPH_MEMBER_NAME}:
        raise RefreshError(
            f"the refresh added unexpected members: {sorted(added - {GRAPH_MEMBER_NAME})}"
        )
    changed = [
        name
        for name in observed
        if name in prior_members and observed[name] != prior_members[name]
    ]
    if changed and changed != ["Cargo.lock"]:
        raise RefreshError(
            f"the refresh changed members other than Cargo.lock: {changed}"
        )
    if observed["Cargo.lock"]["sha256"] != lock_hash:
        raise RefreshError(
            "the new archive's Cargo.lock does not match the current lock"
        )

    vendor_rows = []
    with tarfile.open(new_archive_path, "r:gz") as tar:
        for package in new_external:
            prefix = f"vendor/{package['name']}-{package['version']}/"
            checks = json.loads(tar.extractfile(prefix + ".cargo-checksum.json").read())
            if checks.get("package") != package.get("checksum"):
                raise RefreshError(
                    f"vendor checksum manifest for {package['name']} {package['version']} "
                    "does not match the lock"
                )
            manifest = tomllib.loads(
                tar.extractfile(prefix + "Cargo.toml").read().decode()
            )
            if (
                manifest["package"]["name"] != package["name"]
                or manifest["package"]["version"] != package["version"]
            ):
                raise RefreshError(
                    f"vendored manifest identity mismatch for {package['name']} {package['version']}"
                )
            for name, digest in checks["files"].items():
                if sha256_bytes(tar.extractfile(prefix + name).read()) != digest:
                    raise RefreshError(
                        f"vendored file '{name}' for {package['name']} {package['version']} "
                        "failed its checksum"
                    )
            vendor_rows.append(
                {
                    "name": package["name"],
                    "version": package["version"],
                    "source": package["source"],
                    "verified_files": len(checks["files"]),
                }
            )

    new_index = copy.deepcopy(old)
    new_index["cargo_lock_sha256"] = lock_hash
    new_index["population"]["query_utc"] = datetime.datetime.now(
        datetime.timezone.utc
    ).isoformat()
    new_index["dependency_source_companion"] = {
        "filename": new_archive_path.name,
        "bytes": new_archive_path.stat().st_size,
        "sha256": sha256_file(new_archive_path),
    }

    encoded = (json.dumps(new_index, indent=2, sort_keys=True) + "\n").encode()
    for fragment in FORBIDDEN_PATH_FRAGMENTS:
        if fragment.encode() in encoded:
            raise RefreshError(
                f"the regenerated index contains an operator-internal path fragment: {fragment!r}"
            )
    id_match = FORBIDDEN_ID_PATTERN.search(encoded)
    if id_match:
        raise RefreshError(
            "the regenerated index contains what looks like a hopper/session id: "
            f"{id_match.group().decode(errors='replace')!r}"
        )

    report = {
        "workspace_version_only_changes": workspace_delta,
        "external_package_count": len(new_external),
        "selected_external_identities": sorted(selected),
        "new_archive": new_index["dependency_source_companion"],
        "vendor_verification": vendor_rows,
    }
    (out_dir / "refresh-report.json").write_text(
        json.dumps(report, indent=2, sort_keys=True) + "\n"
    )

    return {"new_index": new_index, "new_index_encoded": encoded, "report": report}


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument(
        "--repo",
        type=Path,
        default=ROOT,
        help="solstone-journal checkout root (default: this script's own checkout)",
    )
    parser.add_argument(
        "--prior-archive",
        type=Path,
        required=True,
        help="path to the currently-committed dependency_source_companion .tar.gz",
    )
    parser.add_argument(
        "--prior-metadata",
        type=Path,
        default=None,
        help=(
            "'cargo metadata' JSON captured when --prior-archive was produced; "
            f"only needed when that archive predates the embedded '{GRAPH_MEMBER_NAME}' member"
        ),
    )
    parser.add_argument(
        "--out",
        type=Path,
        default=None,
        help="output directory for the new archive and report (default: a fresh temp dir)",
    )
    args = parser.parse_args(argv)

    repo = args.repo.resolve()
    out_dir = (
        args.out.resolve()
        if args.out
        else Path(tempfile.mkdtemp(prefix="windows-rust-notice-refresh-"))
    )
    prior_metadata = args.prior_metadata.resolve() if args.prior_metadata else None

    try:
        result = refresh(repo, args.prior_archive.resolve(), prior_metadata, out_dir)
    except RefreshError as error:
        print(f"refresh refused: {error}", file=sys.stderr)
        return 1

    index_path = repo / INDEX_RELATIVE_PATH
    index_path.write_bytes(result["new_index_encoded"])

    print(
        json.dumps(
            {
                "index_written": str(index_path),
                "new_archive_path": str(
                    out_dir
                    / result["new_index"]["dependency_source_companion"]["filename"]
                ),
                **result["report"],
            },
            indent=2,
            sort_keys=True,
        )
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
