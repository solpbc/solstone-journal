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

It handles two further cases. The first is a **git-source pin move**, where
the set of external packages is unchanged by `(name, version)` and the rows
that differ are all `git+` sources. That is the shape of advancing a first-party
library tag. It cannot be member-preserved -- the vendored bytes genuinely
change -- so this script performs the `cargo vendor` acquisition itself and
substitutes only the members that actually moved, after proving that the
vendored member *set* is identical and that every other member is byte-for-byte
unchanged. Supply `--vendor-dir` to reuse a tree you already produced.

The second is a **workspace path-dependency move**, where a workspace member
gains or loses a path dependency on another workspace member. The archive holds
vendored EXTERNAL sources plus `Cargo.lock`, so that edge cannot change one
vendored byte when no external package moved -- and it cannot change a notice
when it does not move anything in or out of the Windows notice closure, which is
what the attestation is actually derived from. Both are required, and both are
checked rather than assumed.

It refuses -- loudly, with the reason -- rather than proceed, when:
  * the external package population changed by `(name, version)` (an add,
    remove, or upgrade). That is a real dependency change and needs licence
    review, not a mechanical refresh.
  * an external row changed source and either side is not a `git+` source.
  * the **Windows notice closure** changed -- the non-dev reach of the Windows
    inventory binary roots. That is the set the notices are derived from, so a
    graph difference that moves it needs a fresh acquisition. A graph difference
    that leaves it untouched does not, and is admitted.
  * a workspace member changed by more than its own version or its path
    dependencies on other workspace members.
  * a re-vendored package's licence text changed. The notices file is an input
    here, not an output; a changed licence needs the notices regenerated, which
    this script deliberately does not do.

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


def normalize_source(source: str) -> str:
    """Drop a git source's locator so two revisions of one dependency compare
    equal.

    The graph and population controls exist to prove that no dependency or
    feature MOVED. A git pin advancing is exactly the change this script is
    allowed to carry, so the revision is the one component those controls must
    not key on -- everything else about the row still has to match.
    """
    if not source.startswith("git+"):
        return source
    return source.split("#", 1)[0].split("?", 1)[0]


def normalize_identity(identity: str) -> str:
    """`name@version (source)` with a git source's locator dropped."""
    head, _, source = identity.partition(" (")
    if not source.endswith(")"):
        return identity
    return f"{head} ({normalize_source(source[:-1])})"


def git_source_revision(source: str) -> str:
    """The 40-character commit a git source resolves to."""
    _, _, revision = source.partition("#")
    if not re.fullmatch(r"[0-9a-f]{40}", revision):
        raise RefreshError(f"git source has no resolved revision: {source!r}")
    return revision


def classify_external_delta(
    old_external: list[dict[str, Any]], new_external: list[dict[str, Any]]
) -> list[tuple[dict[str, Any], dict[str, Any]]]:
    """Return the `(old_row, new_row)` pairs a git pin move explains.

    Refuses anything else. An empty list means the population is identical and
    the cheap member-preserving path applies unchanged.
    """
    old_by_id = {(p["name"], p["version"]): p for p in old_external}
    new_by_id = {(p["name"], p["version"]): p for p in new_external}
    if len(old_by_id) != len(old_external) or len(new_by_id) != len(new_external):
        raise RefreshError(
            "the lock has two external rows sharing one name and version; "
            "this script cannot tell them apart -- scope this as engineering work"
        )
    if set(old_by_id) != set(new_by_id):
        added = sorted(set(new_by_id) - set(old_by_id))
        removed = sorted(set(old_by_id) - set(new_by_id))
        raise RefreshError(
            "the external (non-workspace) package population changed -- an "
            f"added, removed, or upgraded dependency (added={added} "
            f"removed={removed}). That is a real dependency change and needs "
            "licence review, not a mechanical refresh."
        )
    moved = []
    for identity, old_row in old_by_id.items():
        new_row = new_by_id[identity]
        if old_row == new_row:
            continue
        old_source = old_row.get("source", "")
        new_source = new_row.get("source", "")
        if not (old_source.startswith("git+") and new_source.startswith("git+")):
            raise RefreshError(
                f"external package {identity[0]} {identity[1]} changed "
                f"{sorted(field for field in set(old_row) | set(new_row) if old_row.get(field) != new_row.get(field))} "
                "and is not a git dependency on both sides. A member-preserving refresh "
                "cannot attest to vendor bytes nobody produced, and this script only "
                "re-vendors git pin moves."
            )
        if normalize_source(old_source) != normalize_source(new_source):
            raise RefreshError(
                f"git dependency {identity[0]} moved repository "
                f"({normalize_source(old_source)} -> {normalize_source(new_source)}); "
                "that is a re-source, not a pin move"
            )
        differing = {
            field
            for field in set(old_row) | set(new_row)
            if old_row.get(field) != new_row.get(field)
        }
        if differing != {"source"}:
            raise RefreshError(
                f"git dependency {identity[0]} changed more than its source row "
                f"({sorted(differing)}); refusing rather than guess what moved"
            )
        moved.append((old_row, new_row))
    return moved


def run_cargo_vendor(repo: Path, destination: Path) -> None:
    """Acquire the vendor tree for the current lock.

    This is the acquisition the member-preserving path cannot do. `--locked`
    makes it refuse rather than quietly resolve something else.
    """
    result = subprocess.run(
        [
            "cargo",
            "vendor",
            "--locked",
            "--versioned-dirs",
            "--manifest-path",
            str(repo / "core/Cargo.toml"),
            str(destination),
        ],
        capture_output=True,
        check=False,
    )
    if result.returncode != 0:
        raise RefreshError(
            "cargo vendor failed against the current lock (exit "
            f"{result.returncode}): {result.stderr.decode(errors='replace').strip()}\n"
            "This step needs network egress to fetch the moved dependency's "
            "sources. If this session has no venue for that, stop here and say "
            "so rather than improvising one."
        )


def normalize_graph(meta: dict[str, Any]) -> dict[str, Any]:
    """The resolved dependency/feature graph, keyed by identity rather than
    by cargo metadata's checkout-path-bearing package id -- two checkouts of
    the same lock normalize identically even though their ids differ."""
    packages = {p["id"]: p for p in meta["packages"]}

    def key(pkg_id: str) -> str:
        p = packages[pkg_id]
        if p["source"]:
            return f"{p['name']}@{p['version']} ({normalize_source(p['source'])})"
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


def renormalize_graph_keys(graph: dict[str, Any]) -> dict[str, Any]:
    """Re-key a graph produced before git locators were stripped.

    An archive written by an earlier run carries the full `?tag=...#sha` in every
    key, so comparing it against a freshly normalized graph would report a change
    on every git pin move -- which is exactly the case this path exists to carry.
    Applied to both sides, the comparison still proves what it is for: that no
    dependency edge or feature moved.
    """
    return {
        normalize_identity(key): {
            "features": node["features"],
            "deps": sorted(
                (
                    {
                        "name": dep["name"],
                        "pkg": normalize_identity(dep["pkg"]),
                        "dep_kinds": dep["dep_kinds"],
                    }
                    for dep in node["deps"]
                ),
                key=lambda dep: (dep["name"], dep["pkg"]),
            ),
        }
        for key, node in graph.items()
    }


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
            return renormalize_graph_keys(payload["graph"])
    if prior_metadata is None:
        raise RefreshError(
            f"--prior-archive has no embedded '{GRAPH_MEMBER_NAME}' member (it "
            "predates this tool) and no --prior-metadata was supplied. Without "
            "one of the two, the resolved dependency/feature graph cannot be "
            "compared, and a refresh cannot prove itself version-only. Supply "
            "--prior-metadata pointing at the 'cargo metadata' JSON captured "
            "when --prior-archive was produced."
        )
    return renormalize_graph_keys(
        normalize_graph(json.loads(prior_metadata.read_bytes()))
    )


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


def selected_from_graph(graph: dict[str, Any], roots: list[str]) -> set[str]:
    """The Windows notice closure, computed over a NORMALIZED graph.

    `selected_external_identities` answers the same question from live cargo
    metadata. This answers it from a graph recovered out of a prior archive, so
    the two eras can be compared at all. Same walk, same non-dev filter, same
    external-only result -- expressed in normalized keys, which is what a
    normalized graph has.
    """
    missing = [root for root in roots if f"workspace:{root}" not in graph]
    if missing:
        raise RefreshError(
            f"Windows inventory roots absent from the recovered graph: {missing}"
        )
    pending = [f"workspace:{root}" for root in roots]
    seen: set[str] = set()
    while pending:
        key = pending.pop()
        if key in seen:
            continue
        seen.add(key)
        for dep in graph[key]["deps"]:
            if any(kind["kind"] != "dev" for kind in dep["dep_kinds"]):
                pending.append(dep["pkg"])
    return {key for key in seen if not key.startswith("workspace:")}


def _workspace_path_deps_only(
    left: dict[str, Any],
    right: dict[str, Any],
    workspace_names: set[str],
    external_unchanged: bool,
) -> bool:
    """Whether two workspace package rows differ ONLY by path dependencies on
    other workspace members, with the external population proven unchanged."""
    if not external_unchanged:
        return False
    left_rest, right_rest = dict(left), dict(right)
    left_deps = left_rest.pop("dependencies", None)
    right_deps = right_rest.pop("dependencies", None)
    if left_rest != right_rest or left_deps == right_deps:
        return False
    moved = set(left_deps or []) ^ set(right_deps or [])
    return bool(moved) and moved <= workspace_names


def workspace_only_version_delta(
    old_lock: dict[str, Any], new_lock: dict[str, Any], external_unchanged: bool
) -> list[str]:
    """Assert every workspace package is unchanged except (optionally) its own
    version, or -- when the external population is provably identical -- its own
    path dependencies on other workspace members. No hardcoded version strings,
    unlike the prior-art scripts this generalizes. Any other difference refuses.

    The second case is admitted on a strictly stronger proof than the first. The
    archive holds vendored EXTERNAL sources plus `Cargo.lock` and nothing else,
    so a workspace member gaining or losing a path dependency on another
    workspace member cannot change one vendored byte -- provided no external
    package moved, and provided every dependency that actually moved is itself a
    workspace member. Both are required; neither is inferred. Whether that edge
    moved anything in or out of the Windows notice closure is a separate
    question, checked separately."""
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
        if left != right and _workspace_path_deps_only(
            left, right, set(old_by_name) | set(new_by_name), external_unchanged
        ):
            delta.append(name)
            continue
        if left != right:
            raise RefreshError(
                f"workspace package '{name}' changed by more than its version; "
                "this tool only refreshes a version-only lock move -- scope this "
                "as engineering work instead"
            )
        delta.append(name)
    return delta


def advance_git_index_rows(
    index: dict[str, Any], moved_git: list[tuple[dict[str, Any], dict[str, Any]]]
) -> None:
    """Point the committed index's git rows at the new revision.

    Purely mechanical: identity, source, the recorded vcs sha1, and every notice
    reference's revision and URL. The notice DIGESTS are deliberately untouched
    -- `git_source_substitutions` has already proved they still hold at the new
    revision, and rewriting one here would turn a proof into an assertion.
    """
    for old_row, new_row in moved_git:
        old_identity = f"{old_row['name']}@{old_row['version']} ({old_row['source']})"
        new_identity = f"{new_row['name']}@{new_row['version']} ({new_row['source']})"
        revision = git_source_revision(new_row["source"])
        old_revision = git_source_revision(old_row["source"])
        matched = 0
        for package in index["packages"]:
            if package["identity"] != old_identity:
                continue
            matched += 1
            package["identity"] = new_identity
            package["source"] = new_row["source"]
            if package.get("vcs", {}).get("git", {}).get("sha1") == old_revision:
                package["vcs"]["git"]["sha1"] = revision
            for reference in package["notice_references"]:
                source = reference["source"]
                if source.get("revision") == old_revision:
                    source["revision"] = revision
                if isinstance(source.get("source_url"), str):
                    source["source_url"] = source["source_url"].replace(
                        old_revision, revision
                    )
        if matched != 1:
            raise RefreshError(
                f"expected exactly one committed index row for {old_identity}, "
                f"found {matched}"
            )
    stale = [
        package["identity"]
        for package in index["packages"]
        if any(
            git_source_revision(old_row["source"]) in json.dumps(package)
            for old_row, _ in moved_git
        )
    ]
    if stale:
        raise RefreshError(
            f"index rows still reference a superseded git revision: {stale}"
        )


def build_git_substitutions(
    repo: Path,
    prior_archive_path: Path,
    moved_git: list[tuple[dict[str, Any], dict[str, Any]]],
    old_index: dict[str, Any],
    vendor_dir: Path | None,
    revendored: list[dict[str, Any]],
) -> dict[str, bytes]:
    """Produce the replacement bytes for a git pin move, and prove the scope.

    The whole safety of this path rests on one control: a fresh `cargo vendor`
    must produce a member set IDENTICAL to the archive's, so the only thing that
    can differ is the content of members that already exist. An added or removed
    member means something moved that a pin bump cannot explain, and it refuses.
    """
    with tempfile.TemporaryDirectory(prefix="windows-rust-notice-vendor-") as scratch:
        if vendor_dir is None:
            vendor_root = Path(scratch) / "vendor"
            run_cargo_vendor(repo, vendor_root)
        else:
            vendor_root = vendor_dir
            if not vendor_root.is_dir():
                raise RefreshError(f"--vendor-dir is not a directory: {vendor_root}")

        prior_vendor: dict[str, str] = {}
        with tarfile.open(prior_archive_path, "r:gz") as tar:
            for member in tar:
                if member.isfile() and member.name.startswith("vendor/"):
                    prior_vendor[member.name] = sha256_bytes(
                        tar.extractfile(member).read()
                    )
        fresh_vendor = {
            "vendor/" + str(path.relative_to(vendor_root)): path
            for path in vendor_root.rglob("*")
            if path.is_file()
        }
        added = sorted(set(fresh_vendor) - set(prior_vendor))
        removed = sorted(set(prior_vendor) - set(fresh_vendor))
        if added or removed:
            raise RefreshError(
                "the freshly vendored tree does not have the same member set as "
                f"the committed archive (added={added[:10]} removed={removed[:10]}). "
                "A pin move cannot add or remove vendored files; refusing rather "
                "than publish an archive whose shape nobody verified."
            )

        substitutions: dict[str, bytes] = {}
        for name, digest in prior_vendor.items():
            data = fresh_vendor[name].read_bytes()
            if sha256_bytes(data) != digest:
                substitutions[name] = data

        moved_prefixes = tuple(
            f"vendor/{new_row['name']}-{new_row['version']}/"
            for _, new_row in moved_git
        )
        outside = sorted(
            name for name in substitutions if not name.startswith(moved_prefixes)
        )
        if outside:
            raise RefreshError(
                "re-vendoring changed files outside the packages whose pin moved "
                f"{sorted(moved_prefixes)}: {outside[:10]}. Refusing rather than "
                "carry a change nobody accounted for."
            )
        if not substitutions:
            raise RefreshError(
                "the lock moved a git pin but no vendored byte changed. Either the "
                "vendor tree is stale or the new revision is identical content; "
                "refusing rather than publish an archive that attests to the wrong "
                "revision."
            )

        verify_vendored_against_checkout(moved_git, old_index, vendor_root)
        substitutions |= git_source_substitutions(moved_git, old_index, revendored)
        for _, new_row in moved_git:
            revendored.append(
                {
                    "name": new_row["name"],
                    "version": new_row["version"],
                    "source": new_row["source"],
                    "substituted_members": sorted(
                        name
                        for name in substitutions
                        if name.startswith(
                            f"vendor/{new_row['name']}-{new_row['version']}/"
                        )
                    ),
                }
            )
        return substitutions


def verify_vendored_against_checkout(
    moved_git: list[tuple[dict[str, Any], dict[str, Any]]],
    old_index: dict[str, Any],
    vendor_root: Path,
) -> None:
    """Prove the vendored bytes really are the revision the lock now names.

    Everything else here compares the vendor tree against the *prior* archive,
    which cannot tell a correct tree from a stale one at the same name and
    version -- and `--vendor-dir` makes a stale tree an ordinary operator slip.
    A git dependency has no registry checksum to fall back on, so the check is
    against cargo's own checkout at the new revision.

    `Cargo.toml` and `.cargo-checksum.json` are excluded: cargo rewrites the
    first when vendoring and generates the second.
    """
    generated = {"Cargo.toml", ".cargo-checksum.json"}
    for _, new_row in moved_git:
        revision = git_source_revision(new_row["source"])
        checkout = cargo_git_checkout(new_row["source"], revision)
        path_in_vcs = index_path_in_vcs(old_index, new_row)
        crate_root = checkout / path_in_vcs if path_in_vcs else checkout
        vendored = vendor_root / f"{new_row['name']}-{new_row['version']}"
        compared = 0
        for path in sorted(vendored.rglob("*")):
            if not path.is_file():
                continue
            relative = path.relative_to(vendored)
            if str(relative) in generated:
                continue
            origin = crate_root / relative
            if not origin.is_file():
                raise RefreshError(
                    f"vendored file '{relative}' for {new_row['name']} is absent from "
                    f"the {revision} checkout; the vendor tree does not match the lock"
                )
            if sha256_file(origin) != sha256_file(path):
                raise RefreshError(
                    f"vendored file '{relative}' for {new_row['name']} does not match "
                    f"the {revision} checkout. The vendor tree is for a different "
                    "revision -- re-run without --vendor-dir."
                )
            compared += 1
        if compared == 0:
            raise RefreshError(
                f"nothing was comparable for {new_row['name']} at {revision}; a check "
                "that verifies zero files is not a check"
            )


def index_path_in_vcs(old_index: dict[str, Any], new_row: dict[str, Any]) -> str:
    """Where in its repository a git dependency's crate lives."""
    identity = f"{new_row['name']}@{new_row['version']}"
    rows = [
        package
        for package in old_index["packages"]
        if f"{package['name']}@{package['version']}" == identity
    ]
    if len(rows) != 1:
        raise RefreshError(
            f"expected exactly one committed index row for {identity}, found {len(rows)}"
        )
    return rows[0].get("vcs", {}).get("path_in_vcs", "")


def git_source_substitutions(
    moved_git: list[tuple[dict[str, Any], dict[str, Any]]],
    old_index: dict[str, Any],
    revendored: list[dict[str, Any]],
) -> dict[str, bytes]:
    """Rebuild `spl-source.tar` at the new revision, and prove the licence held.

    The archive carries the first-party git dependency's full repository source
    because the vendored crate directory does not contain the repository's
    LICENSE -- the notices reference it as a git blob. That makes the licence a
    thing to re-verify, not assume: if it moved, the notices file is stale and
    regenerating it is out of this script's scope.
    """
    revisions = {git_source_revision(new_row["source"]) for _, new_row in moved_git}
    if len(revisions) != 1:
        raise RefreshError(
            f"git pins moved to more than one revision {sorted(revisions)}; this "
            "script assumes one first-party repository per refresh"
        )
    revision = revisions.pop()
    checkout = cargo_git_checkout(moved_git[0][1]["source"], revision)

    for _, new_row in moved_git:
        identity = f"{new_row['name']}@{new_row['version']}"
        rows = [
            package
            for package in old_index["packages"]
            if f"{package['name']}@{package['version']}" == identity
        ]
        if len(rows) != 1:
            raise RefreshError(
                f"expected exactly one committed index row for {identity}, found {len(rows)}"
            )
        for reference in rows[0]["notice_references"]:
            source = reference["source"]
            if source.get("kind") != "git-blob":
                continue
            member = checkout / source["member"]
            if not member.is_file():
                raise RefreshError(
                    f"{identity}'s notice source '{source['member']}' is absent at "
                    f"{revision}; the notices file is stale and regenerating it is "
                    "out of this script's scope"
                )
            data = member.read_bytes()
            if (
                sha256_bytes(data) != reference["sha256"]
                or len(data) != reference["bytes"]
            ):
                raise RefreshError(
                    f"{identity}'s licence text changed at {revision}. The notices "
                    "file is an input here, not an output -- regenerate it and the "
                    "index's notice spans before refreshing the archive."
                )

    buffer = io.BytesIO()
    with tarfile.open(fileobj=buffer, mode="w", format=tarfile.PAX_FORMAT) as tar:
        for path in sorted(checkout.rglob("*")):
            if path.name == ".cargo-ok":
                continue
            info = tar.gettarinfo(str(path), arcname=str(path.relative_to(checkout)))
            # The archive is a source offer, not a filesystem image. Ownership
            # from whichever machine produced it is noise at best and a leak at
            # worst (VPE principle 8), and a checkout mtime is whenever cargo
            # happened to fetch -- normalizing both is what lets a second
            # operator rebuild this member and get the same bytes.
            info.uid = 0
            info.gid = 0
            info.uname = "root"
            info.gname = "root"
            info.mtime = 0
            if info.isfile():
                with path.open("rb") as handle:
                    tar.addfile(info, handle)
            else:
                tar.addfile(info)
    return {"spl-source.tar": buffer.getvalue()}


def cargo_git_checkout(source: str, revision: str) -> Path:
    """Locate cargo's own checkout of a git dependency at `revision`.

    Deliberately reads what cargo resolved rather than cloning independently: a
    second clone is a second answer to "what is at this revision", and the point
    is to attest to the bytes that were actually built.
    """
    root = Path.home() / ".cargo" / "git" / "checkouts"
    candidates = sorted(root.glob(f"*/{revision[:7]}"))
    if len(candidates) != 1:
        raise RefreshError(
            f"expected exactly one cargo git checkout for {source} at {revision}, "
            f"found {len(candidates)}. Run `cargo fetch --locked --manifest-path "
            "core/Cargo.toml` in this checkout first."
        )
    return candidates[0]


def refresh(
    repo: Path,
    prior_archive_path: Path,
    prior_metadata_path: Path | None,
    out_dir: Path,
    vendor_dir: Path | None = None,
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
    moved_git = classify_external_delta(old_external, new_external)

    workspace_delta = workspace_only_version_delta(
        old_lock, new_lock, external_unchanged=old_external == new_external
    )

    old_graph = load_prior_graph(prior_archive_path, prior_metadata_path)
    new_meta = query_cargo_metadata(repo)
    new_graph = normalize_graph(new_meta)
    roots = windows_roots(repo)
    if old_graph != new_graph:
        # A graph difference is only tolerable when the thing this attestation
        # is actually derived from is unchanged: the Windows notice closure.
        # Comparing the whole graph is a proxy for that, and it is a coarse one
        # -- a workspace member gaining a path dependency moves the graph while
        # leaving the closure, and therefore every notice, untouched. Check the
        # property rather than the proxy, and keep refusing when the property
        # itself moves.
        if selected_from_graph(old_graph, roots) != selected_from_graph(
            new_graph, roots
        ):
            raise RefreshError(
                "the Windows notice closure changed even though the external "
                "package population did not (a feature flag or dependency edge "
                "moved something in or out of the Windows binary reach). Fresh "
                "acquisition required; refusing rather than publish an "
                "attestation for an unverified closure."
            )

    selected = {
        normalize_identity(identity)
        for identity in selected_external_identities(new_meta, roots)
    }
    wanted = {
        normalize_identity(p["identity"])
        for p in old["packages"]
        if p["windows_notice_population"]
    }
    if selected != wanted:
        raise RefreshError(
            "the Windows notice population computed from the current lock "
            f"does not match the committed index: missing={sorted(wanted - selected)} "
            f"added={sorted(selected - wanted)}"
        )
    all_identities = {normalize_identity(p["identity"]) for p in old["packages"]}
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

    substitutions: dict[str, bytes] = {"Cargo.lock": lock_bytes}
    with tarfile.open(prior_archive_path, "r:gz") as tar:
        if GRAPH_MEMBER_NAME in tar.getnames():
            substitutions[GRAPH_MEMBER_NAME] = graph_member_payload
    revendored: list[dict[str, Any]] = []
    if moved_git:
        substitutions |= build_git_substitutions(
            repo, prior_archive_path, moved_git, old, vendor_dir, revendored
        )

    # A substitution whose bytes already match is not a change, and the
    # controls below are stated in terms of changes. Running this tool against
    # an unmoved lock is a legitimate smoke test and must stay a clean no-op.
    with tarfile.open(prior_archive_path, "r:gz") as tar:
        for name in list(substitutions):
            try:
                existing = tar.extractfile(name)
            except KeyError:
                continue
            if existing is not None and sha256_bytes(existing.read()) == sha256_bytes(
                substitutions[name]
            ):
                del substitutions[name]

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
            replacement = substitutions.get(member.name)
            if replacement is not None:
                if data is None:
                    raise RefreshError(
                        f"cannot substitute the non-file member {member.name}"
                    )
                data = replacement
                member.size = len(replacement)
            dest.addfile(member, io.BytesIO(data) if data is not None else None)

        # Substituted when the prior archive already carries it, appended only
        # the first time. ⛔ Appending unconditionally writes a second member
        # under the same name on every run after the first, which reads back as
        # whichever copy the reader happens to keep.
        if GRAPH_MEMBER_NAME not in prior_members:
            graph_info = tarfile.TarInfo(GRAPH_MEMBER_NAME)
            graph_info.size = len(graph_member_payload)
            graph_info.mtime = 0
            dest.addfile(graph_info, io.BytesIO(graph_member_payload))

    unplaced = sorted(set(substitutions) - set(prior_members))
    if unplaced:
        raise RefreshError(
            "substitutions were computed for members the prior archive does not "
            f"carry, so they would have been silently dropped: {unplaced}"
        )

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
    changed = sorted(
        name
        for name in observed
        if name in prior_members and observed[name] != prior_members[name]
    )
    unexpected = sorted(set(changed) - set(substitutions))
    if unexpected:
        raise RefreshError(
            f"the refresh changed members it was not substituting: {unexpected}"
        )
    missed = sorted(set(substitutions) - set(changed))
    if missed:
        raise RefreshError(
            "members were substituted but came out byte-identical, so the "
            f"substitution did not take: {missed}"
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
            # A git dependency is not a registry archive: cargo writes a null
            # `package` and the lock carries no checksum, so the comparison is
            # null-to-absent and still meaningful.
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
    advance_git_index_rows(new_index, moved_git)
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
        "revendored_git_packages": revendored,
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
    parser.add_argument(
        "--vendor-dir",
        type=Path,
        default=None,
        help=(
            "an already-produced `cargo vendor --locked --versioned-dirs` tree for "
            "the current lock, reused instead of running the acquisition again; "
            "only consulted when a git pin moved"
        ),
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
        result = refresh(
            repo,
            args.prior_archive.resolve(),
            prior_metadata,
            out_dir,
            args.vendor_dir.resolve() if args.vendor_dir else None,
        )
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
