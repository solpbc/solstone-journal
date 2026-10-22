#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Refresh the Windows Rust dependency notices after `core/Cargo.lock` moves.

`produce::windows_cli::tests::committed_rust_notices_match_workspace_lock`
(`core/crates/solstone-core-distribution/src/produce/windows_cli.rs`) binds
`core/distribution/windows-rust-sources.json` to `population.source_sha256` --
a digest of the current lock's external (non-workspace) package set, not the
whole-lock digest, so a workspace-internal-only dependency edge cannot red it.
Any change to that external population -- an add, remove, or upgrade -- does
red it, until the index, and the ~99 MB `dependency_source_companion`
vendor-source archive it points at, are regenerated for the new lock. This
script does that regeneration for the one case where it is cheap and safe:
the external (non-workspace) dependency population is unchanged, so the
archive can be rebuilt by copying every vendored member across untouched and
substituting only `Cargo.lock` itself.

It handles further cases. The first is a **git-source pin move**, where
the set of external packages is unchanged by `(name, version)` and the rows
that differ are all `git+` sources. That is the shape of advancing a first-party
library tag. The moved rows may also change their resolved dependency edges,
but only when the independently recovered Windows notice closure remains
identical. It cannot be member-preserved -- the vendored bytes genuinely
change -- so this script performs the `cargo vendor` acquisition itself and
substitutes the members that actually moved. Added files are admitted only
when they sit under a moved package prefix (`vendor/<name>-<version>/`);
every other member must stay byte-for-byte unchanged, and removals still
refuse. Supply `--vendor-dir` to reuse a tree you already produced.

The second is a **workspace dependency-edge move**, where a workspace member
gains or loses a dependency on an already present, unchanged package. The archive holds
vendored EXTERNAL sources plus `Cargo.lock`, so that edge cannot change one
vendored byte when no external package moved -- and it cannot change a notice
when it does not move anything in or out of the Windows notice closure, which is
what the attestation is actually derived from. Both are required, and both are
checked rather than assumed.

The third is a **registry addition outside the Windows notice closure**: the
new lock adds checksummed registry packages, removes nothing, and leaves the
Windows notice closure (below) exactly as it was, so no Windows binary reaches
them and the NOTICES file cannot change. The additions are acquired by `cargo
vendor`, appended to the archive, and recorded as index rows outside the notice
population, with their licence texts referenced by crate archive member. Each
addition must declare a licence expression made only of permissive identifiers.

The fourth is a **reviewed registry addition inside the Windows notice
closure**. A package a Windows binary reaches changes what the payload must
reproduce, so it is never admitted on measurement alone: the operator names
each one with `--admit-closure-addition NAME@VERSION` after reviewing its
licence, and the closure must grow by exactly the named packages and nothing
else. A named package still has to be a checksummed registry addition with a
wholly permissive licence expression. It is vendored into the archive like an
addition outside the closure, recorded as a notice population row, and its
licence texts are added to the NOTICES file and indexed by byte range. The texts
come from the crate archive; a crate that publishes none gets them from its
upstream repository at the revision its `.cargo_vcs_info.json` records, each
text checked against the git blob id that revision lists. A crate with neither
refuses. Licence terms the crate's own texts do not cover -- a vendored
prebuilt binary under someone else's licence, for one -- are outside this
script's view, and the named review has to account for them separately.

An unchanged registry package newly reached by a shipped feature can instead
be named with `--admit-closure-promotion NAME@VERSION`. The prior acquired
source bytes stay unchanged; its licence texts are verified against that
companion and added to NOTICES. Unnamed gains still refuse.
Cargo metadata always includes the canonical shipped-core-features list.
A reviewed promotion that declares an Apache-2.0 option but publishes no text
can name `--admit-standard-apache-text NAME@VERSION`. This acquires the
canonical Apache text at its reviewed digest; it cannot replace an existing
crate notice or select a licence the package does not declare.

The fifth is a **checksummed registry removal**: the new lock drops registry
packages and adds or re-pins nothing. Every surviving vendored member is
copied across byte for byte, the removed packages' `vendor/<name>-<version>/`
members and index rows are dropped, and `Cargo.lock` is substituted. A removed
package outside the Windows notice closure needs no review. One inside it
leaves the closure only by name, with `--admit-closure-removal NAME@VERSION`,
and only while every notice text stays referenced by a surviving package or
path dependency, so the NOTICES file stays byte-identical. A surviving package that leaves the
closure still refuses.

The sixth is a **licence-identical crates.io upgrade**. Each removed version
must pair with one newer version of the same crate, with lock checksums on
both sides. Only the upgraded .crate archives are acquired; every other
vendored member is copied unchanged. The declared permissive expression,
licence/notice files and indexed notice texts must all remain identical,
and rendering the NOTICES must reproduce its bytes. No closure promotion or
removal, git pin move, or other external addition/removal may accompany it.
Changed texts or expressions still require acquisition and licence review.

The same shapes may come with a **new workspace crate**, such as a new Windows
inventory binary root. It is admitted only when `core/Cargo.toml` declares it
as a workspace member or exclude, and when every dependency edge it records
names a workspace crate, an unchanged external package, or a package this lock
adds. What it brings into Windows reach is the notice-closure check's to decide.

It refuses -- loudly, with the reason -- rather than proceed, when:
  * the external package population upgraded a package outside the
    licence-identical crates.io shape above; lost one that is not a checksummed registry package, or lost
    one alongside an addition, git pin move or promotion; or added one that
    is not a checksummed registry package, that reaches the Windows notice
    closure without being named for admission, or whose licence expression is
    not wholly permissive. That is a real
    dependency change and needs licence review, not a mechanical refresh.
  * a package named for addition is not an addition in this lock, or does not
    reach the Windows notice closure. A stale review is not a review.
  * an external row changed source and either side is not a `git+` source.
  * the **Windows notice closure** changed -- the non-dev reach of the Windows
    inventory binary roots -- other than by gaining named additions or
    losing named removals. That is
    the set the notices are derived from, so a graph difference that moves it
    needs a fresh acquisition. A graph difference that leaves it untouched does
    not, and is admitted.
  * a workspace member changed by more than its own version or its
    dependency edges to existing, unchanged packages, or a workspace crate was
    added that `core/Cargo.toml` does not declare.
  * a re-vendored package's licence text changed. Outside the named additions
    above, the notices file is an input here, not an output; a changed licence
    needs the notices regenerated, which this script deliberately does not do.
  * a git pin move removed vendored files, or added files outside the moved
    package prefix. Additions inside that prefix are the shape of a first-party
    crate growing source; anything else is not a pin move this script will
    attest.

On success it writes the new archive plus a small report to `--out`, and
rewrites `core/distribution/windows-rust-sources.json` in place (and
`core/distribution/windows-rust-NOTICES.txt`, when a named addition brings new
licence texts). It also
embeds the resolved dependency graph it verified into the new archive as
`resolved-dependency-graph.json`, so the *next* refresh can read it straight
back out instead of depending on a `cargo metadata` capture kept somewhere
outside this repo. `--prior-metadata` exists only to bootstrap an archive
whose embedded graph is missing or not in the shape this script writes.

Usage:
    python3 scripts/refresh_windows_rust_notices.py \\
        --prior-archive /path/to/windows-rust-dependencies-<oldhash>.tar.gz \\
        [--prior-metadata /path/to/old-cargo-metadata.json] \\
        [--admit-closure-addition NAME@VERSION ...] \\
        [--admit-closure-removal NAME@VERSION ...] \\
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
import urllib.error
import urllib.request
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parent.parent
FILTER_PLATFORM = "x86_64-pc-windows-msvc"
GRAPH_MEMBER_NAME = "resolved-dependency-graph.json"
INDEX_RELATIVE_PATH = "core/distribution/windows-rust-sources.json"
NOTICES_RELATIVE_PATH = "core/distribution/windows-rust-NOTICES.txt"
LOCK_RELATIVE_PATH = "core/Cargo.lock"
INVENTORY_RELATIVE_PATH = "core/distribution/inventory.toml"
# This binary has its own Cargo lock and per-bundle notice profile.
SCREEN_OCR_PACKAGE = "solstone-screen-ocr-helper"

# Operator-internal fragments that must never end up in a file this repo
# carries: no internal office/session paths on public surface. Structural
# patterns, not a specific incident's names. Plain substrings only for
# path/host fragments that cannot collide with a real crate name; an internal
# work-item id (e.g. `req_` followed by an 8-character alphanumeric token)
# needs a word-boundaried regex instead, because a bare "req-" substring match
# is a false positive against real crates (e.g. `ureq`, `ureq-proto`).
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


def external_population_sha256(external: list[dict[str, Any]]) -> str:
    """The digest `population.source_sha256` binds to: every external
    (non-workspace) `[[package]]` row, identified the same way the committed
    index identifies its own rows (`name@version (source)`), sorted and
    hashed. Must match `windows_cli.rs`'s `external_population_sha256`
    exactly -- same identity string, same sort, same join. Path-patched
    Velopack is absent from this digest on purpose; the no-source check
    accounts for it.
    """
    identities = sorted(f"{p['name']}@{p['version']} ({p['source']})" for p in external)
    return sha256_bytes("\n".join(identities).encode())


def load_index(repo: Path) -> dict[str, Any]:
    return json.loads((repo / INDEX_RELATIVE_PATH).read_bytes())


def shipped_feature_arguments(repo: Path) -> list[str]:
    features = (repo / "core/distribution/shipped-core-features.txt").read_text()
    return [
        argument
        for line in features.splitlines()
        if (feature := line.split("#", 1)[0].strip())
        for argument in ("--features", f"solstone-core/{feature}")
    ]


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
            *shipped_feature_arguments(repo),
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


def split_registry_removals(
    old_external: list[dict[str, Any]],
    new_external: list[dict[str, Any]],
    new_workspace_names: frozenset[str] = frozenset(),
) -> tuple[list[dict[str, Any]], list[dict[str, Any]]]:
    """Return `(surviving_old_rows, removed_rows)` for a removal-only lock move.

    Removals are split off only when the new lock adds no `(name, version)`
    and every removed row is a checksummed registry package whose name does not
    reappear as a no-source package (a re-source, not a removal). Anything else
    returns no removals, so `classify_external_delta` still sees -- and
    refuses -- the whole delta: an upgrade reads as a removal plus an addition,
    and a mixed shape is not one this script can prove.
    """
    new_ids = {(p["name"], p["version"]) for p in new_external}
    old_ids = {(p["name"], p["version"]) for p in old_external}
    removed = [p for p in old_external if (p["name"], p["version"]) not in new_ids]
    if not removed or new_ids - old_ids or any(
        not p.get("source", "").startswith("registry+")
        or not p.get("checksum")
        or p["name"] in new_workspace_names
        for p in removed
    ):
        return old_external, []
    surviving = [p for p in old_external if (p["name"], p["version"]) in new_ids]
    return surviving, sorted(removed, key=lambda p: (p["name"], p["version"]))


def split_registry_upgrades(
    old: list[dict[str, Any]], new: list[dict[str, Any]],
) -> list[tuple[dict[str, Any], dict[str, Any]]]:
    """Pair an upgrade-only crates.io delta; leave other shapes to the old checks."""
    key = lambda row: (row["name"], row["version"])
    before = {key(row): row for row in old}
    after = {key(row): row for row in new}
    if len(before) != len(old) or len(after) != len(new):
        raise RefreshError("duplicate external name/version; cannot pair registry upgrades")
    removed = [before[k] for k in before.keys() - after.keys()]
    added = [after[k] for k in after.keys() - before.keys()]
    if not removed or not added:
        return []
    names = {row["name"] for row in removed}
    if (len(names) != len(removed) or len(added) != len(removed)
            or {row["name"] for row in added} != names):
        raise RefreshError("registry upgrades cannot mix additions, removals or ambiguous version pairs")
    pairs = []
    for left in sorted(removed, key=key):
        right = next(row for row in added if row["name"] == left["name"])
        for row in (left, right):
            if (row.get("source") != "registry+https://github.com/rust-lang/crates.io-index"
                    or not re.fullmatch(r"[0-9a-f]{64}", row.get("checksum", ""))):
                raise RefreshError("registry upgrades require checksummed crates.io sources on both sides")
            if not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", row["version"]):
                raise RefreshError("registry upgrades require stable numeric versions")
        if tuple(map(int, right["version"].split("."))) <= tuple(map(int, left["version"].split("."))):
            raise RefreshError("registry upgrade must increase the version")
        pairs.append((left, right))
    return pairs


def replace_graph_identities(graph: dict[str, Any], replacements: dict[str, str]) -> dict[str, Any]:
    """Compare reach with paired versions treated as the same package."""
    result = {}
    for key, node in graph.items():
        result[replacements.get(key, key)] = {
            **node,
            "deps": [{**dep, "pkg": replacements.get(dep["pkg"], dep["pkg"])}
                     for dep in node["deps"]],
        }
    return result


def classify_external_delta(
    old_external: list[dict[str, Any]], new_external: list[dict[str, Any]]
) -> tuple[list[tuple[dict[str, Any], dict[str, Any]]], list[dict[str, Any]]]:
    """Return the `(old_row, new_row)` pairs a git pin move explains, and the
    registry packages the new lock adds.

    An added package is admitted here only as a candidate: the caller refuses
    it unless it stays outside the Windows notice closure (so no notice text
    can change) and carries a permissive licence expression. A removal, an
    upgrade (which reads as a removal plus an add) or an added git dependency
    still refuses; a removal-only delta is split off by the caller first
    (`split_registry_removals`). Empty results mean the population is
    identical and the cheap member-preserving path applies unchanged.

    This script still refuses a zip 2.4.2/8.6.0 → 9.0.3 upgrade and Velopack
    leaving the registry source. That regeneration is caller work. No rewrite
    mode.
    """
    old_by_id = {(p["name"], p["version"]): p for p in old_external}
    new_by_id = {(p["name"], p["version"]): p for p in new_external}
    if len(old_by_id) != len(old_external) or len(new_by_id) != len(new_external):
        raise RefreshError(
            "the lock has two external rows sharing one name and version; "
            "this script cannot tell them apart -- scope this as engineering work"
        )
    added_ids = sorted(set(new_by_id) - set(old_by_id))
    removed = sorted(set(old_by_id) - set(new_by_id))
    if removed:
        raise RefreshError(
            "the external (non-workspace) package population changed -- a "
            f"removed or upgraded dependency (added={added_ids} "
            f"removed={removed}). That is a real dependency change and needs "
            "licence review, not a mechanical refresh."
        )
    added = [new_by_id[identity] for identity in added_ids]
    for row in added:
        if not row.get("source", "").startswith("registry+") or not row.get("checksum"):
            raise RefreshError(
                f"added package {row['name']} {row['version']} is not a checksummed "
                "registry package; this script only admits registry additions"
            )
    moved = []
    for identity, old_row in old_by_id.items():
        new_row = new_by_id[identity]
        if old_row == new_row:
            continue
        old_source = old_row.get("source", "")
        new_source = new_row.get("source", "")
        edges_only = {
            field
            for field in set(old_row) | set(new_row)
            if old_row.get(field) != new_row.get(field)
        } == {"dependencies"}
        if edges_only and old_source.startswith(("registry+", "git+")):
            # Same source, pin and checksum, so the vendored bytes cannot differ; a
            # feature elsewhere in the workspace only re-resolved this package's
            # edges. Whether that moves anything into or out of Windows reach
            # is the notice-closure check's to decide, after this.
            continue
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
        if "source" not in differing or not differing <= {"source", "dependencies"}:
            raise RefreshError(
                f"git dependency {identity[0]} changed beyond its source and resolved dependency edges "
                f"({sorted(differing)}); refusing rather than guess what moved"
            )
        moved.append((old_row, new_row))
    return moved, added


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
            if "graph" in payload:
                return renormalize_graph_keys(payload["graph"])
            if prior_metadata is None:
                raise RefreshError(
                    f"--prior-archive carries a '{GRAPH_MEMBER_NAME}' member that "
                    "is not in the shape this script writes (no 'graph' key), and "
                    "no --prior-metadata was supplied. Supply --prior-metadata "
                    "pointing at the 'cargo metadata' JSON for the prior lock."
                )
            recovered = renormalize_graph_keys(
                normalize_graph(json.loads(prior_metadata.read_bytes()))
            )
            if recovered != renormalize_graph_keys(payload):
                raise RefreshError(
                    "--prior-metadata does not reproduce the graph embedded in "
                    "--prior-archive; refusing to compare against either"
                )
            return recovered
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
            and e["package"] != SCREEN_OCR_PACKAGE
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


def _resolve_lock_dependency(token: str, packages: list[dict[str, Any]]) -> tuple[str, str, str] | None:
    """Resolve Cargo.lock's short or qualified edge without guessing between versions."""
    matches = set()
    for package in packages:
        name, version, source = package["name"], package["version"], package.get("source", "")
        forms = {name, f"{name} {version}"}
        if source:
            forms.add(f"{name} {version} ({source})")
        if token in forms:
            matches.add((name, version, source))
    return next(iter(matches)) if len(matches) == 1 else None


def _workspace_existing_deps_only(
    left: dict[str, Any],
    right: dict[str, Any],
    old_lock: dict[str, Any],
    new_lock: dict[str, Any],
    external_unchanged: bool,
    added: frozenset[tuple[str, str, str]] = frozenset(),
    removed_workspace: frozenset[str] = frozenset(),
    added_workspace: frozenset[str] = frozenset(),
    removed_external: frozenset[tuple[str, str, str]] = frozenset(),
) -> bool:
    """Admit edges to unchanged packages, independent of unrelated pin moves.

    The complete external delta and Windows notice closure are checked by the
    caller; each moved edge still has to name the same source in both locks.
    """
    del external_unchanged
    left_rest, right_rest = dict(left), dict(right)
    left_deps = left_rest.pop("dependencies", None)
    right_deps = right_rest.pop("dependencies", None)
    if left_rest != right_rest or left_deps == right_deps:
        return False
    # Cargo qualifies an existing edge when another version of the same name
    # enters the lock. Resolve each token in the lock that actually contains it,
    # then compare identities; a newly ambiguous bare name must not be guessed.
    old_edges = {
        _resolve_lock_dependency(token, old_lock["package"])
        for token in left_deps or []
    }
    new_edges = {
        _resolve_lock_dependency(token, new_lock["package"])
        for token in right_deps or []
    }
    if None in old_edges or None in new_edges:
        return False
    for identity in old_edges ^ new_edges:
        old_row = next(
            (p for p in old_lock["package"] if
             (p["name"], p["version"], p.get("source", "")) == identity), None
        )
        new_row = next(
            (p for p in new_lock["package"] if
             (p["name"], p["version"], p.get("source", "")) == identity), None
        )
        if old_row is None and new_row is not None and identity in added:
            continue
        if new_row is None and old_row is not None and not identity[2] and identity[0] in removed_workspace:
            continue
        if new_row is None and old_row is not None and identity in removed_external:
            continue
        if old_row is None and new_row is not None and not identity[2] and identity[0] in added_workspace:
            continue
        # Workspace version moves remain permitted even when an unrelated git
        # pin moved elsewhere in the lock. External identities require the
        # referenced package itself to retain its source and notice identity.
        if identity[2]:
            if (old_row is None or new_row is None
                    or old_row.get("checksum") != new_row.get("checksum")):
                return False
        elif not all(
            any(p["name"] == identity[0] and not p.get("source") for p in lock["package"])
            for lock in (old_lock, new_lock)
        ):
            return False
    return True


def declared_workspace_crates(repo: Path) -> frozenset[str]:
    """Workspace members and explicitly owned excluded evidence code.

    An arbitrary workspace exclusion does not establish first-party ownership.
    Keep this classification consistent with the Windows producer and the
    target-notice selector; unknown no-source packages remain unqualified.
    """
    workspace = tomllib.loads((repo / "core/Cargo.toml").read_text())["workspace"]
    names = set()
    for path in [*workspace.get("members", []), *[
        item for item in workspace.get("exclude", [])
        if item == "crates/solstone-core-service-legacy-evidence"
    ]]:
        manifest = repo / "core" / path / "Cargo.toml"
        names.add(tomllib.loads(manifest.read_text())["package"]["name"])
    return frozenset(names)


def _added_workspace_crate_edges_resolve(
    row: dict[str, Any],
    old_lock: dict[str, Any],
    new_lock: dict[str, Any],
    added: frozenset[tuple[str, str, str]],
) -> bool:
    """A new workspace crate records only its name, version and edges, and
    every edge names a workspace crate, an unchanged external package, or a
    package this lock adds."""
    if set(row) - {"name", "version", "dependencies"}:
        return False
    for token in row.get("dependencies", []):
        new = _resolve_lock_dependency(token, new_lock["package"])
        if new is None:
            return False
        if not new[2] or new in added:
            continue
        old_row = next(
            (p for p in old_lock["package"]
             if (p["name"], p["version"], p.get("source", "")) == new), None
        )
        new_row = next(
            p for p in new_lock["package"]
            if (p["name"], p["version"], p.get("source", "")) == new
        )
        if old_row is None or old_row.get("checksum") != new_row.get("checksum"):
            return False
    return True


def workspace_version_or_existing_dependency_edge_delta(
    old_lock: dict[str, Any],
    new_lock: dict[str, Any],
    external_unchanged: bool,
    added: frozenset[tuple[str, str, str]] = frozenset(),
    declared_workspace: frozenset[str] = frozenset(),
    removed: frozenset[tuple[str, str, str]] = frozenset(),
) -> list[str]:
    """Allow workspace versions or edges to existing packages only, plus new
    workspace crates `core/Cargo.toml` declares.

    External edge changes require the referenced package to retain its source
    and checksum; workspace edges require unambiguous workspace resolution in
    both locks. The resulting
    Windows notice closure is checked separately before any archive or index is
    published.
    """
    old_by_name = {p["name"]: p for p in old_lock["package"] if not p.get("source")}
    new_by_name = {p["name"]: p for p in new_lock["package"] if not p.get("source")}
    added_workspace = sorted(set(new_by_name) - set(old_by_name))
    undeclared = [name for name in added_workspace if name not in declared_workspace]
    if undeclared:
        raise RefreshError(
            f"the workspace package set changed (a crate was added: {undeclared}) "
            "and core/Cargo.toml does not declare it as a workspace member or "
            "an explicitly owned exclusion; scope this as engineering work instead"
        )
    for name in added_workspace:
        if not _added_workspace_crate_edges_resolve(
            new_by_name[name], old_lock, new_lock, added
        ):
            raise RefreshError(
                f"added workspace crate '{name}' records more than its version and "
                "dependency edges, or an edge that does not name a workspace "
                "crate, an unchanged external package or a package this lock "
                "adds; scope this as engineering work instead"
            )
    # A removed workspace crate can only take edges away; whether that moves
    # anything out of Windows reach is the notice-closure check's to decide.
    removed_workspace = frozenset(set(old_by_name) - set(new_by_name))
    delta = [f"removed:{name}" for name in sorted(removed_workspace)]
    delta += [f"added:{name}" for name in added_workspace]
    for name, old_pkg in old_by_name.items():
        if name in removed_workspace:
            continue
        new_pkg = new_by_name[name]
        if old_pkg == new_pkg:
            continue
        left, right = dict(old_pkg), dict(new_pkg)
        left.pop("version", None)
        right.pop("version", None)
        if left != right and not _workspace_existing_deps_only(
            left,
            right,
            old_lock,
            new_lock,
            external_unchanged,
            added,
            removed_workspace,
            frozenset(added_workspace),
            removed,
        ):
            raise RefreshError(
                f"workspace package '{name}' changed beyond its version or "
                "resolved dependency edges to existing unchanged packages; "
                "scope this as engineering work instead"
            )
        delta.append(name)
    return delta


def require_explained_notice_closure(
    old_graph: dict[str, Any],
    new_graph: dict[str, Any],
    roots: list[str],
    added: frozenset[str] = frozenset(),
    admitted: frozenset[str] = frozenset(),
    added_workspace: frozenset[str] = frozenset(),
    promoted: frozenset[str] = frozenset(),
    removed: frozenset[str] = frozenset(),
    admitted_removals: frozenset[str] = frozenset(),
) -> frozenset[str]:
    """Return what the Windows notice closure gained, once every gain is a
    registry addition or unchanged registry package named for admission.

    `added` and `admitted` are normalized identities: every registry package
    the lock adds, and the ones the operator named after licence review. A
    Windows inventory root the prior graph lacks is admitted only when it is a
    workspace crate this lock adds; the prior closure is then walked from the
    roots the prior graph has, and the new one from all of them.
    `promoted` names unchanged registry packages entering the closure, whose
    acquired licence texts will be verified against the prior companion.
    `removed` names every registry package the lock drops; the closure may
    lose only those, and only the ones named in `admitted_removals`.
    """
    missing = [
        root
        for root in roots
        if f"workspace:{root}" not in old_graph and root not in added_workspace
    ]
    if missing:
        raise RefreshError(
            f"Windows inventory roots absent from the recovered graph: {missing}"
        )
    before = selected_from_graph(
        old_graph, [root for root in roots if f"workspace:{root}" in old_graph]
    )
    after = selected_from_graph(new_graph, roots)
    gained = after - before
    lost = before - after
    unexplained = sorted((lost - removed) | (gained - added - promoted))
    if unexplained:
        raise RefreshError(
            "the Windows notice closure changed beyond the packages this lock "
            f"adds ({unexplained}): a feature flag or dependency edge moved an "
            "existing package in or out of the Windows binary reach. Fresh "
            "acquisition required; refusing rather than publish an "
            "attestation for an unverified closure. Review unchanged registry gains "
            "and name each with --admit-closure-promotion NAME@VERSION."
        )
    unnamed = sorted(gained - admitted - promoted)
    if unnamed:
        raise RefreshError(
            f"added package(s) {unnamed} reach the Windows notice closure. That "
            "needs licence review, not a mechanical refresh; name each reviewed "
            "package with --admit-closure-addition NAME@VERSION."
        )
    stale = sorted((admitted | promoted) - gained)
    if stale:
        raise RefreshError(
            f"package(s) named for admission {stale} do not reach the Windows "
            "notice closure; refusing a review that does not describe this lock"
        )
    unnamed_removals = sorted(lost - admitted_removals)
    if unnamed_removals:
        raise RefreshError(
            f"removed package(s) {unnamed_removals} were in the Windows notice "
            "closure. Review that nothing shipped still needs them and name each "
            "with --admit-closure-removal NAME@VERSION."
        )
    stale_removals = sorted(admitted_removals - lost)
    if stale_removals:
        raise RefreshError(
            f"package(s) named for removal {stale_removals} did not leave the "
            "Windows notice closure; refusing a review that does not describe this lock"
        )
    return frozenset(gained)


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


def admit_git_pin_vendor_delta(
    prior_vendor: set[str],
    fresh_vendor: set[str],
    moved_prefixes: tuple[str, ...],
) -> list[str]:
    """Return added members a git pin move may carry, or refuse.

    A first-party pin can grow files inside `vendor/<name>-<version>/`. Removals,
    and additions outside those prefixes, are not a pin move this script attests.
    """
    added = sorted(fresh_vendor - prior_vendor)
    removed = sorted(prior_vendor - fresh_vendor)
    added_outside = [name for name in added if not name.startswith(moved_prefixes)]
    if removed or added_outside:
        raise RefreshError(
            "the freshly vendored tree does not have the same member set as "
            f"the committed archive (added={added[:10]} removed={removed[:10]}). "
            "A pin move may add files only under the moved package prefix; "
            "removed files or additions outside that prefix are refused."
        )
    return [name for name in added if name.startswith(moved_prefixes)]


def build_git_substitutions(
    repo: Path,
    prior_archive_path: Path,
    moved_git: list[tuple[dict[str, Any], dict[str, Any]]],
    old_index: dict[str, Any],
    vendor_dir: Path | None,
    revendored: list[dict[str, Any]],
) -> dict[str, bytes]:
    """Produce the replacement bytes for a git pin move, and prove the scope.

    A fresh `cargo vendor` may change bytes of the moved packages and add files
    under those packages' prefixes. Every other vendored member must stay
    byte-for-byte unchanged. Removals, and additions outside the moved prefixes,
    still refuse.
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
        moved_prefixes = tuple(
            f"vendor/{new_row['name']}-{new_row['version']}/"
            for _, new_row in moved_git
        )
        added_inside = admit_git_pin_vendor_delta(
            set(prior_vendor), set(fresh_vendor), moved_prefixes
        )

        substitutions: dict[str, bytes] = {}
        for name, digest in prior_vendor.items():
            data = fresh_vendor[name].read_bytes()
            if sha256_bytes(data) != digest:
                substitutions[name] = data
        for name in added_inside:
            substitutions[name] = fresh_vendor[name].read_bytes()

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

    return {"spl-source.tar": source_tar(checkout)}


def source_tar(checkout: Path) -> bytes:
    """Archive a first-party checkout as a source offer.

    Cargo's git checkout is itself a git repository. Its `.git` directory holds
    the fetching machine's absolute paths and reflog identity, which are not
    source and must never be published.
    """
    buffer = io.BytesIO()
    with tarfile.open(fileobj=buffer, mode="w", format=tarfile.PAX_FORMAT) as tar:
        for path in sorted(checkout.rglob("*")):
            relative = path.relative_to(checkout)
            if path.name == ".cargo-ok" or ".git" in relative.parts:
                continue
            info = tar.gettarinfo(str(path), arcname=str(relative))
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
    return buffer.getvalue()


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


# Licence identifiers an added package outside the Windows notice closure may
# carry without review. Anything else, including a copyleft or unknown
# identifier anywhere in the expression, refuses.
PERMISSIVE_LICENSE_IDS = frozenset(
    {
        "0BSD",
        "Apache-2.0",
        "BSD-1-Clause",
        "BSD-2-Clause",
        "BSD-3-Clause",
        "BSL-1.0",
        "CC0-1.0",
        "ISC",
        "MIT",
        "MIT-0",
        "Unicode-3.0",
        "Unicode-DFS-2016",
        "Unlicense",
        "Zlib",
    }
)
LICENSE_FILE_PREFIXES = ("LICENSE", "LICENCE", "COPYING", "COPYRIGHT", "NOTICE")


def vendored_package_metadata(
    vendor_root: Path, added: list[dict[str, Any]]
) -> dict[str, Any]:
    """The added packages' own `[package]` tables, read from the vendored
    manifests. Windows-filtered cargo metadata omits packages no Windows
    target reaches, which is exactly what an admitted addition is."""
    packages = []
    for row in added:
        manifest = vendor_root / f"{row['name']}-{row['version']}" / "Cargo.toml"
        if not manifest.is_file():
            raise RefreshError(f"cargo vendor produced no manifest for {row['name']} {row['version']}")
        table = tomllib.loads(manifest.read_text())["package"]
        packages.append({**table, "source": row["source"]})
    return {"packages": packages}


def require_permissive_additions(
    meta: dict[str, Any], added: list[dict[str, Any]]
) -> dict[tuple[str, str], dict[str, Any]]:
    """Return the manifest table for each added package after checking its licence."""
    by_id = {(p["name"], p["version"]): p for p in meta["packages"] if p.get("source")}
    found = {}
    for row in added:
        package = by_id.get((row["name"], row["version"]))
        if package is None:
            raise RefreshError(
                f"no manifest describes added package {row['name']} {row['version']}"
            )
        expression = package.get("license") or ""
        ids = {
            token
            for token in expression.replace("(", " ").replace(")", " ").replace("/", " OR ").split()
            if token not in {"OR", "AND", "WITH"}
        }
        if not ids or not ids <= PERMISSIVE_LICENSE_IDS:
            raise RefreshError(
                f"added package {row['name']} {row['version']} declares licence "
                f"{expression!r}, outside the permissive set this script admits "
                "without review"
            )
        found[(row["name"], row["version"])] = package
    return found


def http_fetch(url: str) -> tuple[int, bytes]:
    """`(status, body)` for one HTTPS GET; the only network read this script
    makes itself, outside `cargo vendor`."""
    request = urllib.request.Request(
        url, headers={"User-Agent": "solstone-windows-rust-notices-refresh"}
    )
    try:
        with urllib.request.urlopen(request, timeout=60) as response:
            return response.status, response.read()
    except urllib.error.HTTPError as error:
        return error.code, b""
    except urllib.error.URLError as error:
        raise RefreshError(
            f"could not fetch {url}: {error.reason}. This step needs network "
            "egress to the package's upstream repository; if this session has no "
            "venue for that, stop here and say so rather than improvising one."
        ) from error


def git_blob_sha1(data: bytes) -> str:
    """The id git gives a blob with these bytes."""
    return hashlib.sha1(b"blob %d\0" % len(data) + data).hexdigest()


def github_repository(repository: str | None) -> tuple[str, str]:
    """`(owner, name)` of a GitHub repository URL, or a refusal."""
    match = re.fullmatch(
        r"https://github\.com/([A-Za-z0-9_.-]+)/([A-Za-z0-9_.-]+?)(?:\.git)?/?",
        repository or "",
    )
    if match is None:
        raise RefreshError(
            f"repository {repository!r} is not a GitHub repository URL; this "
            "script can only pin an upstream licence text to a GitHub revision"
        )
    return match.group(1), match.group(2)


def pinned_upstream_licence_references(
    name: str,
    repository: str | None,
    vcs: dict[str, Any],
    fetch: Any,
) -> list[tuple[dict[str, Any], bytes]]:
    """Licence texts for a crate whose archive publishes none, read from its
    upstream repository at the revision `.cargo_vcs_info.json` records.

    The crate's own directory in that repository is searched first, then each
    parent up to the repository root; the first directory holding licence
    files supplies them all. Each text must hash to the blob id the revision's
    tree lists for it, so the bytes are the ones the revision holds and not
    merely whatever the URL served.
    """
    owner, repo_name = github_repository(repository)
    revision = (vcs.get("git") or {}).get("sha1", "")
    if not re.fullmatch(r"[0-9a-f]{40}", revision):
        raise RefreshError(
            f"{name} records no upstream revision in .cargo_vcs_info.json, so its "
            "licence text cannot be pinned"
        )
    tree_url = (
        f"https://api.github.com/repos/{owner}/{repo_name}/git/trees/{revision}?recursive=1"
    )
    status, body = fetch(tree_url)
    if status != 200:
        raise RefreshError(f"{tree_url} answered HTTP {status}")
    tree = json.loads(body)
    if tree.get("truncated"):
        raise RefreshError(f"{tree_url} returned a truncated tree")
    blobs = {item["path"]: item["sha"] for item in tree["tree"] if item["type"] == "blob"}
    directory = vcs.get("path_in_vcs", "").strip("/")
    while True:
        prefix = f"{directory}/" if directory else ""
        members = sorted(
            path
            for path in blobs
            if path.startswith(prefix)
            and "/" not in path[len(prefix):]
            and path[len(prefix):].upper().startswith(LICENSE_FILE_PREFIXES)
        )
        if members or not directory:
            break
        directory = directory.rpartition("/")[0]
    if not members:
        raise RefreshError(
            f"{name} publishes no licence text in its crate archive or in "
            f"{owner}/{repo_name} at {revision}; it needs a reviewed text of its own"
        )
    references = []
    for member in members:
        url = f"https://raw.githubusercontent.com/{owner}/{repo_name}/{revision}/{member}"
        status, data = fetch(url)
        if status != 200:
            raise RefreshError(f"{url} answered HTTP {status}")
        if git_blob_sha1(data) != blobs[member]:
            raise RefreshError(
                f"{url} does not hash to the blob {blobs[member]} its revision lists"
            )
        references.append(
            (
                {
                    "bytes": len(data),
                    "sha256": sha256_bytes(data),
                    "source": {
                        "git_blob_match": True,
                        "git_blob_sha1": blobs[member],
                        "http_status": status,
                        "kind": "pinned-upstream-git-blob",
                        "member": member,
                        "repository": f"https://github.com/{owner}/{repo_name}",
                        "revision": revision,
                        "source_url": url,
                    },
                },
                data,
            )
        )
    return references


def added_package_rows(
    added: list[dict[str, Any]],
    metadata: dict[tuple[str, str], dict[str, Any]],
    vendor_root: Path,
    in_closure: frozenset[tuple[str, str]] = frozenset(),
    fetch: Any = http_fetch,
) -> tuple[list[dict[str, Any]], dict[str, bytes], dict[str, bytes]]:
    """Index rows, vendored archive members and new notice texts for admitted
    additions.

    A row outside the Windows notice closure matches how the index records
    such a package: its licence texts are referenced by crate archive member,
    and none of them enters the NOTICES file. A row inside it -- one named for
    admission -- is a notice population row, which must have texts: the crate
    archive's own, or failing those its pinned upstream repository's. Those
    texts are returned by digest for the NOTICES file.
    """
    rows, members, texts = [], {}, {}
    for lock_row in added:
        name, version = lock_row["name"], lock_row["version"]
        directory = vendor_root / f"{name}-{version}"
        if not directory.is_dir():
            raise RefreshError(f"cargo vendor produced no directory for {name} {version}")
        for path in sorted(directory.rglob("*")):
            if path.is_symlink() or not (path.is_file() or path.is_dir()):
                raise RefreshError(f"non-regular vendored member: {path}")
            if path.is_file():
                members[f"vendor/{name}-{version}/{path.relative_to(directory).as_posix()}"] = (
                    path.read_bytes()
                )
        references = []
        for path in sorted(directory.iterdir()):
            if path.is_file() and path.name.upper().startswith(LICENSE_FILE_PREFIXES):
                data = path.read_bytes()
                references.append(
                    (
                        {
                            "bytes": len(data),
                            "sha256": sha256_bytes(data),
                            "source": {
                                "archive_sha256": lock_row["checksum"],
                                "kind": "cargo-registry-archive",
                                "member": path.name,
                                "source_url": f"https://static.crates.io/crates/{name}/{name}-{version}.crate",
                            },
                        },
                        data,
                    )
                )
        vcs_path = directory / ".cargo_vcs_info.json"
        vcs = {}
        if vcs_path.is_file():
            recorded = json.loads(vcs_path.read_text())
            vcs = {key: recorded[key] for key in ("git", "path_in_vcs") if key in recorded}
        package = metadata[(name, version)]
        population = (name, version) in in_closure
        if population:
            if not references:
                references = pinned_upstream_licence_references(
                    name, package.get("repository"), vcs, fetch
                )
            for reference, data in references:
                texts[reference["sha256"]] = data
        rows.append(
            {
                "archive_sha256": lock_row["checksum"],
                "identity": f"{name}@{version} ({lock_row['source']})",
                "license_expression": package.get("license"),
                "name": name,
                "notice_references": [reference for reference, _ in references],
                "notice_status": "texts-acquired" if references else "upstream-named-text-absent",
                "repository": package.get("repository"),
                "source": lock_row["source"],
                "vcs": vcs,
                "version": version,
                "windows_notice_population": population,
            }
        )
    return rows, members, texts


def acquire_registry_upgrade(row: dict[str, Any], vendor: Path, fetch: Any = http_fetch) -> None:
    """Acquire one lock-verified .crate, without resolving or vendoring the workspace."""
    name, version = row["name"], row["version"]
    if not re.fullmatch(r"[A-Za-z0-9_-]+", name):
        raise RefreshError(f"unsafe registry package name: {name!r}")
    url = f"https://static.crates.io/crates/{name}/{name}-{version}.crate"
    status, data = fetch(url)
    if status != 200 or sha256_bytes(data) != row["checksum"]:
        raise RefreshError(f"registry archive failed HTTP/checksum verification: {name} {version}")
    prefix = f"{name}-{version}/"
    files = {}
    with tarfile.open(fileobj=io.BytesIO(data), mode="r:gz") as archive:
        seen = set()
        for member in archive:
            relative = member.name.removeprefix(prefix)
            path = Path(relative)
            if (not member.name.startswith(prefix) or not relative
                    or path.drive or path.is_absolute() or ".." in path.parts
                    or relative != path.as_posix() or relative in seen
                    or not (member.isfile() or member.isdir())):
                raise RefreshError(f"unsafe or duplicate registry archive member: {member.name}")
            seen.add(relative)
            if member.isfile():
                files[relative] = archive.extractfile(member).read()
    if ".cargo-checksum.json" in files or "Cargo.toml" not in files:
        raise RefreshError(f"registry archive has invalid Cargo manifests: {name} {version}")
    directory = vendor / f"{name}-{version}"
    directory.mkdir(parents=True)
    for relative, content in files.items():
        destination = directory / relative
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_bytes(content)
    checks = {"package": row["checksum"],
              "files": {path: sha256_bytes(content) for path, content in files.items()}}
    (directory / ".cargo-checksum.json").write_text(json.dumps(checks, sort_keys=True))


def upgraded_package_rows(
    prior: Path, index: dict[str, Any],
    upgrades: list[tuple[dict[str, Any], dict[str, Any]]],
    fetch: Any = http_fetch,
) -> tuple[list[dict[str, Any]], dict[str, bytes]]:
    """Replace sources only after proving the expression and all licence texts identical."""
    rows, members = [], {}
    with tempfile.TemporaryDirectory(prefix="windows-rust-upgrade-") as scratch, tarfile.open(prior) as archive:
        vendor = Path(scratch)
        for left, right in upgrades:
            identity = f"{left['name']}@{left['version']} ({left['source']})"
            matches = [row for row in index["packages"] if row["identity"] == identity]
            if len(matches) != 1 or matches[0]["archive_sha256"] != left["checksum"]:
                raise RefreshError(f"prior index does not attest upgraded package {identity}")
            old_row = matches[0]
            prefix = f"vendor/{left['name']}-{left['version']}/"
            old_files = {m.name[len(prefix):]: archive.extractfile(m).read()
                         for m in archive.getmembers() if m.isfile() and m.name.startswith(prefix)}
            checks = json.loads(old_files.pop(".cargo-checksum.json"))
            if (checks["package"] != left["checksum"]
                    or checks["files"] != {name: sha256_bytes(data) for name, data in old_files.items()}):
                raise RefreshError(f"prior upgraded package fails vendor checksums: {identity}")
            old_manifest = tomllib.loads(old_files["Cargo.toml"].decode())["package"]
            if ((old_manifest["name"], old_manifest["version"]) != (left["name"], left["version"])
                    or old_manifest.get("license") != old_row["license_expression"]):
                raise RefreshError(f"prior upgraded package manifest differs from its index: {identity}")
            acquire_registry_upgrade(right, vendor, fetch)
            directory = vendor / f"{right['name']}-{right['version']}"
            metadata = vendored_package_metadata(vendor, [right])
            manifest = metadata["packages"][0]
            # The roots crate's data licence is allowed only on this identity-preserving path.
            expression = manifest.get("license")
            if expression != old_row["license_expression"]:
                raise RefreshError(f"upgraded package licence expression changed: {identity}")
            if expression != "CDLA-Permissive-2.0":
                require_permissive_additions(metadata, [right])
            new_files = {p.relative_to(directory).as_posix(): p.read_bytes()
                         for p in directory.rglob("*") if p.is_file()}

            def licence_files(files: dict[str, bytes], package: dict[str, Any]) -> dict[str, bytes]:
                selected = {name: data for name, data in files.items()
                            if Path(name).name.upper().startswith(LICENSE_FILE_PREFIXES)}
                declared = package.get("license-file")
                if declared:
                    if declared not in files:
                        raise RefreshError(f"declared licence file absent: {identity}: {declared}")
                    selected[declared] = files[declared]
                return selected

            if licence_files(old_files, old_manifest) != licence_files(new_files, manifest):
                raise RefreshError(f"upgraded package licence/notice texts changed: {identity}")
            generated, acquired, _texts = added_package_rows(
                [right], {(right["name"], right["version"]): manifest}, vendor,
                frozenset({(right["name"], right["version"])}) if old_row["windows_notice_population"] else frozenset(),
                fetch,
            )
            row = generated[0]
            def references(package: dict[str, Any]) -> list[tuple[str, str, int]]:
                return sorted((ref["source"]["member"], ref["sha256"], ref["bytes"])
                              for ref in package["notice_references"])
            if not references(old_row) or references(old_row) != references(row):
                raise RefreshError(f"upgraded package indexed notice texts changed or absent: {identity}")
            for reference in old_row["notice_references"]:
                if reference["source"]["kind"] == "cargo-registry-archive":
                    raw = old_files.get(reference["source"]["member"])
                    if raw is None or len(raw) != reference["bytes"] or sha256_bytes(raw) != reference["sha256"]:
                        raise RefreshError(f"prior indexed notice does not match acquired source: {identity}")
            rows.append(row)
            members.update(acquired)
    return rows, members


def promote_notice_rows(
    prior_archive: Path, rows: list[dict[str, Any]],
    standard_apache: frozenset[tuple[str, str]] = frozenset(),
    fetch: Any = http_fetch,
) -> tuple[list[dict[str, Any]], dict[str, bytes]]:
    promoted, texts = [], {}
    with tarfile.open(prior_archive, "r:gz") as archive:
        members = archive.getnames()
        for row in rows:
            prefix = f"vendor/{row['name']}-{row['version']}/"
            package = tomllib.loads(archive.extractfile(prefix + "Cargo.toml").read().decode())["package"]
            package["source"] = row["source"]
            if (package["name"], package["version"], package.get("license")) != (
                row["name"], row["version"], row["license_expression"]
            ):
                raise RefreshError("promotion manifest does not match the acquired index row")
            require_permissive_additions({"packages": [package]}, [row])
            references = []
            for reference in row["notice_references"]:
                source = reference["source"]
                member = source["member"]
                if source["kind"] != "cargo-registry-archive" or Path(member).name != member:
                    raise RefreshError("promotion notice must name a top-level crate archive member")
                data = archive.extractfile(prefix + member).read()
                if sha256_bytes(data) != reference["sha256"] or len(data) != reference["bytes"]:
                    raise RefreshError("promotion notice does not match the acquired source text")
                references.append((reference, data))
            if (row["name"], row["version"]) in standard_apache:
                has_notice = any(
                    member.startswith(prefix)
                    and Path(member).name.upper().startswith(LICENSE_FILE_PREFIXES)
                    for member in members
                )
                if references or has_notice or package.get("license") not in (
                    "Apache-2.0", "Apache-2.0 OR MIT", "MIT OR Apache-2.0", "MIT/Apache-2.0"
                ):
                    raise RefreshError("standard Apache text requires an absent text and an explicit Apache-2.0 licence option")
                url = "https://www.apache.org/licenses/LICENSE-2.0.txt"
                status, data = fetch(url)
                if status != 200 or sha256_bytes(data) != "cfc7749b96f63bd31c3c42b5c471bf756814053e847c10f3eb003417bc523d30":
                    raise RefreshError("canonical Apache-2.0 licence text could not be acquired")
                references = [({"bytes": len(data), "sha256": sha256_bytes(data), "source": {
                    "kind": "standard-license-text", "source_url": url,
                    "license_identifier": "Apache-2.0", "declaration": package["license"],
                }}, data)]
            elif not references:
                references = pinned_upstream_licence_references(
                    row["name"], package.get("repository"), row.get("vcs", {}), fetch
                )
            if not references:
                raise RefreshError("promotion has no acquired licence texts")
            new_row = copy.deepcopy(row)
            new_row["notice_references"] = [reference for reference, _ in references]
            new_row["notice_status"] = "texts-acquired"
            new_row["windows_notice_population"] = True
            promoted.append(new_row)
            for reference, data in references:
                texts[reference["sha256"]] = data
    return promoted, texts


def removed_index_identities(
    index: dict[str, Any],
    removed: list[dict[str, Any]],
    lost: frozenset[str],
    committed_texts: set[str],
) -> set[str]:
    """The committed index rows a registry removal drops, after proving them.

    Each removed package must have exactly one row, flagged in the notice
    population exactly when it left the Windows notice closure, and dropping
    the rows must leave every committed notice text referenced by a surviving
    population row or path dependency -- so the NOTICES file stays as it is.
    """
    identities = {f"{row['name']}@{row['version']} ({row['source']})" for row in removed}
    for identity in sorted(identities):
        rows = [p for p in index["packages"] if p["identity"] == identity]
        if len(rows) != 1:
            raise RefreshError(
                f"expected exactly one committed index row for {identity}, found {len(rows)}"
            )
        if rows[0]["windows_notice_population"] != (normalize_identity(identity) in lost):
            raise RefreshError(
                f"the committed index's notice population flag for {identity} "
                "does not match the recovered Windows notice closure"
            )

    def referenced(rows: list[dict[str, Any]]) -> set[str]:
        return {
            reference["sha256"]
            for row in rows
            if row["windows_notice_population"]
            for reference in row["notice_references"]
        }

    surviving = [p for p in index["packages"] if p["identity"] not in identities]
    orphaned = sorted(
        (
            referenced(index["packages"])
            - referenced(surviving)
            - referenced(index.get("path_dependencies", []))
        )
        & committed_texts
    )
    if orphaned:
        raise RefreshError(
            f"a removal leaves notice text(s) {orphaned} referenced by no "
            "surviving package. The notices file is an input here, not an "
            "output; regenerating it is out of this script's scope."
        )
    return identities


NOTICES_HEADER = b"Rust dependency notices\n\n"


def render_notices(texts: dict[str, bytes]) -> tuple[bytes, list[dict[str, Any]]]:
    """The NOTICES file for a set of texts, and its `notice_texts` index.

    One block per distinct text, ordered by digest: a `SHA-256:` line, a blank
    line, the text's bytes exactly, and a blank line.
    """
    rendered = bytearray(NOTICES_HEADER)
    index = []
    for digest, data in sorted(texts.items()):
        if sha256_bytes(data) != digest:
            raise RefreshError(f"notice text does not hash to {digest}")
        rendered += f"SHA-256: {digest}\n\n".encode()
        start = len(rendered)
        rendered += data
        index.append(
            {
                "byte_end_exclusive": len(rendered),
                "byte_start_inclusive": start,
                "bytes": len(data),
                "sha256": digest,
            }
        )
        rendered += b"\n\n"
    return bytes(rendered), index


def parse_admissions(values: list[str]) -> frozenset[tuple[str, str]]:
    """`NAME@VERSION` operands of `--admit-closure-addition`."""
    admitted = set()
    for value in values:
        name, separator, version = value.partition("@")
        if not separator or not name or not version or "@" in version:
            raise RefreshError(
                f"admission flags take NAME@VERSION, not {value!r}"
            )
        admitted.add((name, version))
    return frozenset(admitted)


def refresh(
    repo: Path,
    prior_archive_path: Path,
    prior_metadata_path: Path | None,
    out_dir: Path,
    vendor_dir: Path | None = None,
    admissions: frozenset[tuple[str, str]] = frozenset(),
    promotions: frozenset[tuple[str, str]] = frozenset(),
    standard_apache: frozenset[tuple[str, str]] = frozenset(),
    removals: frozenset[tuple[str, str]] = frozenset(),
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

    if external_population_sha256(old_external) != old["population"]["source_sha256"]:
        raise RefreshError(
            "the committed index's population.source_sha256 does not match the "
            "external package population recovered from --prior-archive's own "
            "Cargo.lock -- it was edited out of band"
        )

    upgrades = split_registry_upgrades(old_external, new_external)
    upgrade_old = {(row["name"], row["version"]) for row, _ in upgrades}
    upgrade_new = {(row["name"], row["version"]) for _, row in upgrades}
    surviving_external, removed_external = split_registry_removals(
        [row for row in old_external if (row["name"], row["version"]) not in upgrade_old],
        [row for row in new_external if (row["name"], row["version"]) not in upgrade_new],
        frozenset(p["name"] for p in new_lock["package"] if not p.get("source")),
    )
    moved_git, added_external = classify_external_delta(
        surviving_external,
        [row for row in new_external if (row["name"], row["version"]) not in upgrade_new],
    )
    if upgrades and (moved_git or added_external or removed_external or admissions
                     or promotions or standard_apache or removals):
        raise RefreshError("registry upgrades cannot mix git moves, additions, removals or closure reviews")
    if removed_external and (moved_git or promotions):
        raise RefreshError(
            "the lock removes registry packages and also moves a git pin or "
            "promotes a package; this script attests a removal only on its own"
        )
    removed_ids = frozenset(
        (p["name"], p["version"], p["source"]) for p in removed_external
    )
    not_removed = sorted(
        f"{name}@{version}"
        for name, version in removals
        if not any((p["name"], p["version"]) == (name, version) for p in removed_external)
    )
    if not_removed:
        raise RefreshError(
            f"package(s) named for removal {not_removed} are not registry "
            "removals in this lock; refusing a review that does not describe it"
        )
    added_ids = frozenset(
        (p["name"], p["version"], p["source"]) for p in added_external
    )
    existing_new_external = [
        p for p in new_external if (p["name"], p["version"], p["source"]) not in added_ids
    ]
    not_added = sorted(
        f"{name}@{version}"
        for name, version in admissions
        if not any((p["name"], p["version"]) == (name, version) for p in added_external)
    )
    if not_added:
        raise RefreshError(
            f"package(s) named for admission {not_added} are not registry "
            "additions in this lock; refusing a review that does not describe it"
        )

    if not standard_apache <= promotions:
        raise RefreshError("standard licence review must name an admitted closure promotion")
    promotion_rows = [
        row for row in old["packages"]
        if (row["name"], row["version"]) in promotions
    ]
    if {(row["name"], row["version"]) for row in promotion_rows} != promotions or any(
        row["windows_notice_population"]
        or not row["source"].startswith("registry+")
        or not any(
            (p["name"], p["version"], p["source"], p.get("checksum"))
            == (row["name"], row["version"], row["source"], row["archive_sha256"])
            for p in existing_new_external
        )
        for row in promotion_rows
    ):
        raise RefreshError("promotions must name unchanged registry packages outside the prior notice closure")

    workspace_delta = workspace_version_or_existing_dependency_edge_delta(
        old_lock,
        new_lock,
        external_unchanged=old_external == existing_new_external,
        added=added_ids | frozenset((p["name"], p["version"], p["source"]) for _, p in upgrades),
        declared_workspace=declared_workspace_crates(repo),
        removed=removed_ids | frozenset((p["name"], p["version"], p["source"]) for p, _ in upgrades),
    )
    added_workspace = frozenset(
        change.removeprefix("added:")
        for change in workspace_delta
        if change.startswith("added:")
    )

    old_graph = load_prior_graph(prior_archive_path, prior_metadata_path)
    new_meta = query_cargo_metadata(repo)
    new_graph = normalize_graph(new_meta)
    roots = windows_roots(repo)

    def added_identity(row: dict[str, Any]) -> str:
        return normalize_identity(f"{row['name']}@{row['version']} ({row['source']})")

    upgrade_forward = {added_identity(left): added_identity(right) for left, right in upgrades}
    upgrade_backward = {right: left for left, right in upgrade_forward.items()}
    gained = require_explained_notice_closure(
        old_graph,
        replace_graph_identities(new_graph, upgrade_backward),
        roots,
        added=frozenset(added_identity(row) for row in added_external),
        admitted=frozenset(
            added_identity(row)
            for row in added_external
            if (row["name"], row["version"]) in admissions
        ),
        added_workspace=added_workspace,
        promoted=frozenset(added_identity(row) for row in promotion_rows),
        removed=frozenset(added_identity(row) for row in removed_external),
        admitted_removals=frozenset(
            added_identity(row)
            for row in removed_external
            if (row["name"], row["version"]) in removals
        ),
    )
    # The closure check above proved the closure lost exactly the named
    # removals, so they are the population rows that go.
    lost = frozenset(
        added_identity(row)
        for row in removed_external
        if (row["name"], row["version"]) in removals
    )

    selected = {
        normalize_identity(identity)
        for identity in selected_external_identities(new_meta, roots)
    }
    wanted = {
        upgrade_forward.get(normalize_identity(p["identity"]), normalize_identity(p["identity"]))
        for p in old["packages"]
        if p["windows_notice_population"]
    } - lost | gained
    if selected != wanted:
        raise RefreshError(
            "the Windows notice population computed from the current lock "
            f"does not match the committed index: missing={sorted(wanted - selected)} "
            f"added={sorted(selected - wanted)}"
        )
    all_identities = {normalize_identity(p["identity"]) for p in old["packages"]} - {
        added_identity(row) for row in removed_external + [left for left, _ in upgrades]
    } | {added_identity(row) for row in added_external + [right for _, right in upgrades]}
    if not selected < all_identities:
        raise RefreshError(
            "negative control failed: the selected population is not a proper "
            "subset of the full lock -- the root/edge walk may be selecting everything"
        )

    committed_texts: dict[str, bytes] = {}
    for row in old["notice_texts"]:
        raw = notices[row["byte_start_inclusive"] : row["byte_end_exclusive"]]
        if len(raw) != row["bytes"] or sha256_bytes(raw) != row["sha256"]:
            raise RefreshError(
                "a notice text span starting at byte "
                f"{row['byte_start_inclusive']} no longer matches the committed NOTICES file"
            )
        committed_texts[row["sha256"]] = raw

    removed_identities = removed_index_identities(
        old, removed_external, lost, set(committed_texts)
    )
    upgraded_rows, upgraded_members = upgraded_package_rows(prior_archive_path, old, upgrades)
    if upgrades:
        regenerated, _spans = render_notices(committed_texts)
        if regenerated != notices:
            raise RefreshError("registry upgrade must regenerate byte-identical NOTICES")

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

    substitutions: dict[str, bytes] = {"Cargo.lock": lock_bytes, **upgraded_members}
    with tarfile.open(prior_archive_path, "r:gz") as tar:
        if GRAPH_MEMBER_NAME in tar.getnames():
            substitutions[GRAPH_MEMBER_NAME] = graph_member_payload
    revendored: list[dict[str, Any]] = []
    if moved_git:
        substitutions |= build_git_substitutions(
            repo, prior_archive_path, moved_git, old, vendor_dir, revendored
        )
    added_rows: list[dict[str, Any]] = []
    added_texts: dict[str, bytes] = {}
    if added_external:
        with tempfile.TemporaryDirectory(prefix="windows-rust-add-vendor-") as scratch:
            vendor_root = vendor_dir
            if vendor_root is None:
                vendor_root = Path(scratch) / "vendor"
                run_cargo_vendor(repo, vendor_root)
            added_metadata = require_permissive_additions(
                vendored_package_metadata(vendor_root, added_external), added_external
            )
            added_rows, added_members, added_texts = added_package_rows(
                added_external,
                added_metadata,
                vendor_root,
                in_closure=frozenset(
                    (row["name"], row["version"])
                    for row in added_external
                    if added_identity(row) in gained
                ),
            )
        collisions = sorted(set(added_members) & set(substitutions))
        if collisions:
            raise RefreshError(f"added members collide with substitutions: {collisions}")
        substitutions |= added_members

    promoted_rows, promoted_texts = promote_notice_rows(prior_archive_path, promotion_rows, standard_apache)
    added_texts |= promoted_texts

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

    dropped_prefixes = tuple(
        f"vendor/{row['name']}-{row['version']}/"
        for row in removed_external + [left for left, _ in upgrades]
    )

    def dropped(name: str) -> bool:
        # tarfile strips a directory member's trailing slash.
        return bool(dropped_prefixes) and (
            name.startswith(dropped_prefixes) or f"{name}/" in dropped_prefixes
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
            if dropped(member.name):
                if member.name in substitutions:
                    raise RefreshError(
                        f"a substitution targets a removed package's member: {member.name}"
                    )
                continue
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

        for name in sorted(set(substitutions) - set(prior_members)):
            if name == GRAPH_MEMBER_NAME:
                continue
            if name.startswith("/") or ".." in Path(name).parts:
                raise RefreshError(f"unsafe tar member path: {name}")
            info = tarfile.TarInfo(name)
            info.size = len(substitutions[name])
            info.mtime = 0
            info.mode = 0o644
            dest.addfile(info, io.BytesIO(substitutions[name]))

    observed: dict[str, dict[str, Any]] = {}
    vendor_manifests: dict[str, bytes] = {}
    with tarfile.open(new_archive_path, "r:gz") as tar:
        for member in tar:
            data = tar.extractfile(member).read() if member.isfile() else None
            observed[member.name] = {
                "mode": member.mode,
                "size": member.size,
                "sha256": sha256_bytes(data) if data is not None else None,
            }
            if (data is not None and member.name.startswith("vendor/")
                    and Path(member.name).name in {".cargo-checksum.json", "Cargo.toml"}):
                vendor_manifests[member.name] = data

    expected_dropped = {
        name
        for name in prior_members
        if dropped(name)
    }
    absent = sorted(
        prefix for prefix in dropped_prefixes
        if prefix + ".cargo-checksum.json" not in prior_members
    )
    if absent:
        raise RefreshError(
            f"the prior archive does not vendor the removed package(s) {absent}"
        )
    removed = set(prior_members) - set(observed)
    if removed != expected_dropped:
        raise RefreshError(
            "the refresh dropped members that must be preserved, or kept a "
            "removed package's members: "
            f"{sorted(removed ^ expected_dropped)[:10]}"
        )
    added = set(observed) - set(prior_members)
    expected_added = set(substitutions) - set(prior_members)
    if GRAPH_MEMBER_NAME not in prior_members:
        expected_added.add(GRAPH_MEMBER_NAME)
    unexpected_added = added - expected_added
    if unexpected_added:
        raise RefreshError(
            f"the refresh added unexpected members: {sorted(unexpected_added)}"
        )
    missing_added = expected_added - added
    if missing_added:
        raise RefreshError(
            "substitutions were computed for members the prior archive does not "
            f"carry, so they were dropped: {sorted(missing_added)}"
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
    missed = sorted((set(substitutions) & set(prior_members)) - set(changed))
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
    for package in new_external:
        prefix = f"vendor/{package['name']}-{package['version']}/"
        checks = json.loads(vendor_manifests[prefix + ".cargo-checksum.json"])
        # A git dependency is not a registry archive: cargo writes a null
        # `package` and the lock carries no checksum, so the comparison is
        # null-to-absent and still meaningful.
        if checks.get("package") != package.get("checksum"):
            raise RefreshError(
                f"vendor checksum manifest for {package['name']} {package['version']} "
                "does not match the lock"
            )
        manifest = tomllib.loads(
            vendor_manifests[prefix + "Cargo.toml"].decode()
        )
        if (
            manifest["package"]["name"] != package["name"]
            or manifest["package"]["version"] != package["version"]
        ):
            raise RefreshError(
                f"vendored manifest identity mismatch for {package['name']} {package['version']}"
            )
        for name, digest in checks["files"].items():
            actual = observed.get(prefix + name)
            if actual is None or actual["sha256"] is None or actual["sha256"] != digest:
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
    if upgraded_rows:
        replaced = set(upgrade_forward)
        new_index["packages"] = sorted(
            [row for row in new_index["packages"] if normalize_identity(row["identity"]) not in replaced]
            + upgraded_rows, key=lambda row: row["name"].lower(),
        )
    if removed_external:
        new_index["packages"] = [
            p for p in new_index["packages"] if p["identity"] not in removed_identities
        ]
        new_index["population"]["source_package_count"] = len(new_external)
    if added_rows:
        new_index["packages"] = sorted(
            new_index["packages"] + added_rows,
            key=lambda row: row["name"].lower(),
        )
        new_index["population"]["source_package_count"] = len(new_external)
    if promoted_rows:
        by_identity = {row["identity"]: row for row in promoted_rows}
        new_index["packages"] = [by_identity.get(row["identity"], row) for row in new_index["packages"]]
    if gained or lost:
        count = (
            old["population"]["windows_notice_package_count"] + len(gained) - len(lost)
        )
        if count != len(selected):
            raise RefreshError(
                f"the committed windows_notice_package_count plus {len(gained)} "
                f"admitted and minus {len(lost)} removed package(s) is {count}, "
                f"not the {len(selected)} the current lock selects"
            )
        new_index["population"]["windows_notice_package_count"] = count
    new_notices = None
    if set(added_texts) - set(committed_texts):
        # The file is rewritten whole, so first prove the layout this script
        # renders is the one the committed file already has. Otherwise the
        # rewrite would silently reformat every existing text.
        rendered, rendered_index = render_notices(committed_texts)
        if rendered != notices or rendered_index != sorted(
            old["notice_texts"], key=lambda row: row["byte_start_inclusive"]
        ):
            raise RefreshError(
                "rendering the committed notice texts does not reproduce "
                f"{NOTICES_RELATIVE_PATH} byte for byte; refusing to rewrite it"
            )
        new_notices, new_index["notice_texts"] = render_notices(
            committed_texts | added_texts
        )
        new_index["notices_sha256"] = sha256_bytes(new_notices)
    new_index["cargo_lock_sha256"] = lock_hash
    new_index["population"]["query"] = (
        "cargo metadata --manifest-path core/Cargo.toml --locked --offline --format-version 1 "
        f"--filter-platform {FILTER_PLATFORM} {' '.join(shipped_feature_arguments(repo))}; "
        "inventory roots; follow non-dev dependency edges"
    )
    new_index["population"]["shipped_feature_arguments"] = shipped_feature_arguments(repo)
    new_index["population"]["query_utc"] = datetime.datetime.now(
        datetime.timezone.utc
    ).isoformat()
    new_index["population"]["source_sha256"] = external_population_sha256(new_external)
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
            "the regenerated index contains what looks like an internal work-item id: "
            f"{id_match.group().decode(errors='replace')!r}"
        )

    report = {
        "workspace_version_or_existing_dependency_edge_changes": workspace_delta,
        "revendored_git_packages": revendored,
        "licence_identical_registry_upgrades": [
            {"from": added_identity(left), "to": added_identity(right),
             "archive_sha256": right["checksum"]} for left, right in upgrades
        ],
        "added_packages_outside_windows_notice_closure": [
            {
                "identity": row["identity"],
                "license_expression": row["license_expression"],
                "notice_references": len(row["notice_references"]),
            }
            for row in added_rows
            if not row["windows_notice_population"]
        ],
        "added_packages_inside_windows_notice_closure": [
            {
                "identity": row["identity"],
                "license_expression": row["license_expression"],
                "notice_references": [
                    {
                        "kind": reference["source"]["kind"],
                        "member": reference["source"]["member"],
                        "sha256": reference["sha256"],
                    }
                    for reference in row["notice_references"]
                ],
            }
            for row in added_rows
            if row["windows_notice_population"]
        ],
        "promoted_existing_packages": [row["identity"] for row in promoted_rows],
        "removed_packages": [
            {
                "identity": f"{row['name']}@{row['version']} ({row['source']})",
                "left_windows_notice_closure": added_identity(row) in lost,
            }
            for row in removed_external
        ],
        "removed_vendor_members": len(expected_dropped),
        "added_notice_texts": sorted(set(added_texts) - set(committed_texts)),
        "windows_notice_package_count": len(selected),
        "external_package_count": len(new_external),
        "external_population_sha256": new_index["population"]["source_sha256"],
        "selected_external_identities": sorted(selected),
        "new_archive": new_index["dependency_source_companion"],
        "vendor_verification": vendor_rows,
    }
    (out_dir / "refresh-report.json").write_text(
        json.dumps(report, indent=2, sort_keys=True) + "\n"
    )

    return {
        "new_index": new_index,
        "new_index_encoded": encoded,
        "new_notices": new_notices,
        "report": report,
    }


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
            f"only needed when that archive's embedded '{GRAPH_MEMBER_NAME}' member is "
            "missing or not in the shape this script writes"
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
            "only consulted when a git pin moved or a package was added"
        ),
    )
    parser.add_argument(
        "--admit-closure-addition",
        action="append",
        default=[],
        metavar="NAME@VERSION",
        help=(
            "a registry package this lock adds inside the Windows notice closure, "
            "whose licence has been reviewed; repeat once per package. Any "
            "in-closure addition not named here refuses."
        ),
    )
    parser.add_argument(
        "--admit-closure-promotion", action="append", default=[], metavar="NAME@VERSION",
        help="reviewed unchanged registry package entering the shipped-feature notice closure",
    )
    parser.add_argument(
        "--admit-standard-apache-text", action="append", default=[], metavar="NAME@VERSION",
        help="reviewed promotion publishing an Apache-2.0 option but no text; acquire the canonical licence",
    )
    parser.add_argument(
        "--admit-closure-removal", action="append", default=[], metavar="NAME@VERSION",
        help=(
            "a registry package this lock removes from inside the Windows notice "
            "closure, reviewed as no longer needed by anything shipped; repeat once "
            "per package. Any in-closure removal not named here refuses."
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
            parse_admissions(args.admit_closure_addition),
            parse_admissions(args.admit_closure_promotion),
            parse_admissions(args.admit_standard_apache_text),
            parse_admissions(args.admit_closure_removal),
        )
    except RefreshError as error:
        print(f"refresh refused: {error}", file=sys.stderr)
        return 1

    index_path = repo / INDEX_RELATIVE_PATH
    index_path.write_bytes(result["new_index_encoded"])
    if result["new_notices"] is not None:
        (repo / NOTICES_RELATIVE_PATH).write_bytes(result["new_notices"])

    print(
        json.dumps(
            {
                "index_written": str(index_path),
                "notices_written": result["new_notices"] is not None,
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
