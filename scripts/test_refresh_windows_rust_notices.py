#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Dependency-edge admission regressions for the mechanical notice refresh."""
import copy
import unittest

import refresh_windows_rust_notices as refresh

SOURCE = "registry+https://github.com/rust-lang/crates.io-index"


def package(name, version="1.0.0", source=None, dependencies=None):
    row = {"name": name, "version": version}
    if source:
        row.update(source=source, checksum="unchanged")
    if dependencies:
        row["dependencies"] = dependencies
    return row


def graph(extra_edge=False, digest_reachable=True):
    edge = lambda key: {"pkg": key, "dep_kinds": [{"kind": None}]}
    return {
        "workspace:app": {"deps": [edge("workspace:indexer")] + (
            [edge("digest")] if digest_reachable else []
        )},
        "workspace:indexer": {"deps": [edge("digest")] if extra_edge else []},
        "digest": {"deps": []},
    }


class ExistingDependencyEdges(unittest.TestCase):
    def setUp(self):
        self.old = {"package": [
            package("app", dependencies=["indexer", "digest"]),
            package("indexer"),
            package("digest", source=SOURCE),
        ]}
        self.new = copy.deepcopy(self.old)
        self.new["package"][1]["dependencies"] = ["digest"]

    def admit(self, old_graph=None, new_graph=None):
        before = [p for p in self.old["package"] if p.get("source")]
        after = [p for p in self.new["package"] if p.get("source")]
        self.assertEqual(refresh.classify_external_delta(before, after), [])
        changes = refresh.workspace_only_version_delta(
            self.old, self.new, external_unchanged=before == after
        )
        refresh.require_unchanged_notice_closure(
            old_graph or graph(), new_graph or graph(extra_edge=True), ["app"]
        )
        return changes

    def test_existing_external_edge_preserves_notice_closure(self):
        self.assertEqual(self.admit(), ["indexer"])

    def test_removed_existing_edge_preserves_notice_closure(self):
        self.old, self.new = self.new, self.old
        self.assertEqual(self.admit(), ["indexer"])

    def test_newly_reachable_external_package_refuses(self):
        with self.assertRaisesRegex(refresh.RefreshError, "notice closure changed"):
            self.admit(graph(digest_reachable=False), graph(extra_edge=True))

    def test_changed_external_population_refuses(self):
        self.new["package"].append(package("unknown", source=SOURCE))
        with self.assertRaisesRegex(refresh.RefreshError, "population changed"):
            self.admit()

    def test_changed_external_row_cannot_be_admitted_by_claiming_equality(self):
        self.new["package"][2]["checksum"] = "changed"
        with self.assertRaises(refresh.RefreshError):
            self.admit()
        with self.assertRaises(refresh.RefreshError):
            refresh.workspace_only_version_delta(self.old, self.new, external_unchanged=True)

    def test_unknown_dependency_refuses(self):
        self.new["package"][1]["dependencies"] = ["unknown"]
        with self.assertRaises(refresh.RefreshError):
            self.admit()

    def test_ambiguous_bare_name_refuses_but_exact_version_resolves(self):
        for lock in (self.old, self.new):
            lock["package"].append(package("digest", version="2.0.0", source=SOURCE))
        with self.assertRaises(refresh.RefreshError):
            self.admit()
        self.new["package"][1]["dependencies"] = ["digest 1.0.0"]
        self.assertEqual(self.admit(), ["indexer"])

    def test_source_qualification_is_exact(self):
        packages = [package("digest", source=SOURCE), package("digest", source="registry+https://other.example/index")]
        self.assertIsNone(refresh._resolve_lock_dependency("digest 1.0.0", packages))
        self.assertEqual(refresh._resolve_lock_dependency(f"digest 1.0.0 ({SOURCE})", packages), ("digest", "1.0.0", SOURCE))
        self.assertIsNone(refresh._resolve_lock_dependency("digest 1.0.0 (unknown)", packages))

    def test_existing_workspace_edge_and_version_move_remain_supported(self):
        self.new["package"][1]["dependencies"] = ["app"]
        self.new["package"][0]["version"] = "2.0.0"
        self.assertEqual(self.admit(), ["app", "indexer"])


if __name__ == "__main__":
    unittest.main()
