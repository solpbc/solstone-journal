#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
"""Tests for land_drafts.py: concurrent landing, staleness, exclusions, and byte identity."""

from __future__ import annotations

import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

TOOLS_I18N = Path(__file__).resolve().parent
LAND_DRAFTS = TOOLS_I18N / "land_drafts.py"


def find_compiler() -> Path:
    env_path = os.environ.get("I18N_COMPILER")
    if env_path:
        return Path(env_path).resolve()
    candidates = [
        TOOLS_I18N / "target/debug/i18n-catalog",
        TOOLS_I18N.parent.parent / "core/target/i18n-catalog/debug/i18n-catalog",
    ]
    for c in candidates:
        if c.is_file():
            return c
    raise RuntimeError(
        "Set I18N_COMPILER to a built i18n-catalog binary before running test_land_drafts.py"
    )


def run(cmd: list[str], cwd: Path | None = None, check: bool = True) -> subprocess.CompletedProcess[str]:
    proc = subprocess.run(cmd, cwd=cwd, text=True, capture_output=True)
    if check and proc.returncode != 0:
        raise AssertionError(
            f"Command failed (exit {proc.returncode}): {cmd}\nSTDOUT:\n{proc.stdout}\nSTDERR:\n{proc.stderr}"
        )
    return proc


def init_sample_repo(compiler: Path, base: Path) -> tuple[Path, Path, dict[str, str]]:
    """Create a bare remote + working clone with a valid link-host catalog."""
    remote = base / "remote.git"
    run(["git", "init", "--bare", "-b", "main", str(remote)])

    work = base / "seed"
    run(["git", "clone", str(remote), str(work)])
    run(["git", "config", "user.name", "Test"], cwd=work)
    run(["git", "config", "user.email", "test@example.invalid"], cwd=work)

    cat = work / "i18n/catalog"
    (cat / "source").mkdir(parents=True)
    (cat / "meta").mkdir(parents=True)
    (cat / "translations").mkdir(parents=True)

    (cat / "catalog.json").write_text(
        json.dumps(
            {
                "catalog_version": 1,
                "label": "Concurrent landing test catalog",
                "history": False,
                "source_locale": "en-US",
            },
            indent=2,
        )
        + "\n",
        encoding="utf-8",
    )

    source_ftl = (
        "link.test.a1 = open solstone on your phone\n"
        "link.test.a2 = keep your journal private\n"
        "link.test.b1 = { $count ->\n"
        "    [one] { $count } device paired\n"
        "   *[other] { $count } devices paired\n"
        "}\n"
        "link.test.b2 = visit solstone.me for details\n"
        "link.test.terms = terms of service stay english\n"
        "link.test.statute = cancel contracts here\n"
    )
    (cat / "source/en-US.ftl").write_text(source_ftl, encoding="utf-8")

    # Write placeholder source_revision then let `lock` compute exact hashes
    dummy_rev = "0" * 64
    specs = {
        "link.test.a1": ("routine", True, False),
        "link.test.a2": ("routine", True, False),
        "link.test.b1": ("routine", True, True),
        "link.test.b2": ("consent", False, False),
        "link.test.terms": ("legal", False, False),
        "link.test.statute": ("statutory", False, False),
    }
    for mid, (risk, auto_merge, cardinal) in specs.items():
        meta: dict = {
            "description": "test",
            "context": "test",
            "surfaces": ["link-host"],
            "bundles": ["link-host"],
            "arguments": [],
            "format_intent": "text",
            "max_length": 120,
            "source_revision": dummy_rev,
            "risk": risk,
            "auto_merge": auto_merge,
        }
        if cardinal:
            meta["format_intent"] = "cardinal"
            meta["arguments"] = [
                {
                    "name": "count",
                    "type": "cardinal",
                    "numeric_style": "ungrouped",
                    "min": 0,
                    "max": 1000,
                }
            ]
            meta["selector"] = {"name": "count", "type": "cardinal"}
        (cat / "meta" / f"{mid}.json").write_text(
            json.dumps(meta, indent=2) + "\n", encoding="utf-8"
        )

    run([str(compiler), "lock", str(cat)])
    lock_data = json.loads((cat / "source-lock.json").read_text(encoding="utf-8"))
    revs = {m["id"]: m["source"] for m in lock_data["messages"]}
    for mid, rev in revs.items():
        p = cat / "meta" / f"{mid}.json"
        m = json.loads(p.read_text(encoding="utf-8"))
        m["source_revision"] = rev
        p.write_text(json.dumps(m, indent=2) + "\n", encoding="utf-8")

    run([str(compiler), "history", str(cat)])
    cat_cfg = json.loads((cat / "catalog.json").read_text(encoding="utf-8"))
    cat_cfg["history"] = True
    (cat / "catalog.json").write_text(json.dumps(cat_cfg, indent=2) + "\n", encoding="utf-8")
    run([str(compiler), "compile", str(cat), "--channel", "production"])

    run(["git", "add", "."], cwd=work)
    run(["git", "commit", "-m", "Seed catalog"], cwd=work)
    run(["git", "push", "origin", "main"], cwd=work)
    return remote, work, revs


class LandDraftsTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.compiler = find_compiler()

    def test_concurrent_units_race_retry_and_staleness_after_english_edit(self) -> None:
        with tempfile.TemporaryDirectory(prefix="test-land-drafts-") as tmp_str:
            tmp = Path(tmp_str)
            remote, seed, revs = init_sample_repo(self.compiler, tmp)

            # Two independent clones at the same base commit (simulating concurrent translators)
            clone_a = tmp / "clone-a"
            clone_b = tmp / "clone-b"
            for c in (clone_a, clone_b):
                run(["git", "clone", str(remote), str(c)])
                run(["git", "config", "user.name", "Unit"], cwd=c)
                run(["git", "config", "user.email", "unit@example.invalid"], cwd=c)

            # Unit A owns a1, a2, terms (legal excluded)
            manifest_a = tmp / "manifest-a.json"
            manifest_a.write_text(
                json.dumps(
                    {
                        "unit": "unit-a",
                        "messages": [
                            {"id": "link.test.a1", "risk": "routine", "source_revision": revs["link.test.a1"]},
                            {"id": "link.test.a2", "risk": "routine", "source_revision": revs["link.test.a2"]},
                            {"id": "link.test.terms", "risk": "legal", "source_revision": revs["link.test.terms"]},
                        ],
                    }
                ),
                encoding="utf-8",
            )
            ftl_a = tmp / "unit-a.ftl"
            ftl_a.write_text(
                "link.test.a1 = abre solstone en tu teléfono\n"
                "link.test.a2 = mantén tu diario privado\n",
                encoding="utf-8",
            )
            jsonl_a = tmp / "unit-a.jsonl"
            jsonl_a.write_text(
                "\n".join(
                    [
                        json.dumps({"id": "link.test.a1", "status": "drafted", "reason": None, "flags": [], "note": ""}),
                        json.dumps({"id": "link.test.a2", "status": "drafted", "reason": None, "flags": [], "note": ""}),
                        json.dumps({"id": "link.test.terms", "status": "excluded", "reason": "legal", "flags": [], "note": ""}),
                    ]
                )
                + "\n",
                encoding="utf-8",
            )

            # Unit B owns b1 (cardinal), b2 (left uncertain -> excluded), statute (statutory excluded)
            manifest_b = tmp / "manifest-b.json"
            manifest_b.write_text(
                json.dumps(
                    {
                        "unit": "unit-b",
                        "messages": [
                            {"id": "link.test.b1", "risk": "routine", "source_revision": revs["link.test.b1"]},
                            {"id": "link.test.b2", "risk": "consent", "source_revision": revs["link.test.b2"]},
                            {"id": "link.test.statute", "risk": "statutory", "source_revision": revs["link.test.statute"]},
                        ],
                    }
                ),
                encoding="utf-8",
            )
            ftl_b = tmp / "unit-b.ftl"
            ftl_b.write_text(
                "link.test.b1 = { $count ->\n"
                "    [one] { $count } dispositivo vinculado\n"
                "    [many] { $count } dispositivos vinculados\n"
                "   *[other] { $count } dispositivos vinculados\n"
                "}\n",
                encoding="utf-8",
            )
            jsonl_b = tmp / "unit-b.jsonl"
            jsonl_b.write_text(
                "\n".join(
                    [
                        json.dumps({"id": "link.test.b1", "status": "drafted", "reason": None, "flags": [], "note": ""}),
                        json.dumps({"id": "link.test.b2", "status": "excluded", "reason": "uncertain", "flags": ["uncertain"], "note": "confirm details wording"}),
                        json.dumps({"id": "link.test.statute", "status": "excluded", "reason": "statutory", "flags": [], "note": ""}),
                    ]
                )
                + "\n",
                encoding="utf-8",
            )

            # Verify Unit A with targets + production byte-identity check
            v_res = run(
                [
                    "python3",
                    str(LAND_DRAFTS),
                    "verify",
                    "--compiler",
                    str(self.compiler),
                    "--catalog",
                    str(clone_a / "i18n/catalog"),
                    "--manifest",
                    str(manifest_a),
                    "--ftl",
                    str(ftl_a),
                    "--drafts-jsonl",
                    str(jsonl_a),
                    "--shard",
                    "unit-a",
                    "--check-targets",
                    "esm,rust",
                ]
            )
            v_json = json.loads(v_res.stdout)
            self.assertTrue(v_json["production_byte_identical"])
            self.assertEqual(v_json["drafted"], 2)
            self.assertEqual(v_json["excluded_by_reason"], {"legal": 1})

            # Unit A lands first onto main
            run(
                [
                    "python3",
                    str(LAND_DRAFTS),
                    "land",
                    "--repo",
                    str(clone_a),
                    "--compiler",
                    str(self.compiler),
                    "--entry",
                    f"i18n/catalog:{manifest_a}:{ftl_a}:{jsonl_a}:unit-a",
                    "--message",
                    "i18n: land es-419 draft unit-a",
                ]
            )

            # Unit B (whose clone is still at the old commit) lands on top without conflict
            run(
                [
                    "python3",
                    str(LAND_DRAFTS),
                    "land",
                    "--repo",
                    str(clone_b),
                    "--compiler",
                    str(self.compiler),
                    "--entry",
                    f"i18n/catalog:{manifest_b}:{ftl_b}:{jsonl_b}:unit-b",
                    "--message",
                    "i18n: land es-419 draft unit-b",
                ]
            )

            # Inspect merged main: BOTH shards present, BOTH acked, legal/statutory/uncertain absent
            run(["git", "pull", "--ff-only", "origin", "main"], cwd=seed)
            self.assertTrue((seed / "i18n/catalog/translations/es-419/unit-a.ftl").is_file())
            self.assertTrue((seed / "i18n/catalog/translations/es-419/unit-b.ftl").is_file())
            status_proc = run(
                [
                    str(self.compiler),
                    "status",
                    str(seed / "i18n/catalog"),
                    "--locale",
                    "es-419",
                    "--json",
                ]
            )
            st = json.loads(status_proc.stdout)
            self.assertEqual(st["fresh"], ["link.test.a1", "link.test.a2", "link.test.b1"])
            self.assertEqual(st["stale"], [])
            self.assertEqual(st["excluded_restricted"], ["link.test.statute", "link.test.terms"])
            self.assertEqual(st["missing"], ["link.test.b2"])

            # Another session edits English for link.test.a1 and runs lock WITHOUT --acknowledge es-419
            src_path = seed / "i18n/catalog/source/en-US.ftl"
            src_path.write_text(
                src_path.read_text(encoding="utf-8").replace(
                    "link.test.a1 = open solstone on your phone",
                    "link.test.a1 = open solstone on your mobile device",
                ),
                encoding="utf-8",
            )
            run([str(self.compiler), "lock", str(seed / "i18n/catalog")])
            new_lock = json.loads((seed / "i18n/catalog/source-lock.json").read_text(encoding="utf-8"))
            new_a1_rev = next(m["source"] for m in new_lock["messages"] if m["id"] == "link.test.a1")
            meta_a1 = seed / "i18n/catalog/meta/link.test.a1.json"
            m_obj = json.loads(meta_a1.read_text(encoding="utf-8"))
            m_obj["source_revision"] = new_a1_rev
            meta_a1.write_text(json.dumps(m_obj, indent=2) + "\n", encoding="utf-8")

            # Production compile STILL succeeds
            run([str(self.compiler), "compile", str(seed / "i18n/catalog"), "--channel", "production"])

            # Strict development compile FAILS with stale on link.test.a1
            stale_proc = run(
                [str(self.compiler), "compile", str(seed / "i18n/catalog"), "--channel", "development", "--locale", "es-419"],
                check=False,
            )
            self.assertNotEqual(stale_proc.returncode, 0)
            self.assertIn("link.test.a1:es-419:stale", stale_proc.stderr)

            # status --json reports link.test.a1 stale and the other two fresh
            st2_proc = run(
                [str(self.compiler), "status", str(seed / "i18n/catalog"), "--locale", "es-419", "--json"],
                check=False,
            )
            self.assertEqual(st2_proc.returncode, 1)
            st2 = json.loads(st2_proc.stdout)
            self.assertEqual(st2["stale"], ["link.test.a1"])
            self.assertEqual(st2["fresh"], ["link.test.a2", "link.test.b1"])

    def test_verify_rejects_english_fill_on_excluded_and_broken_fixed_term(self) -> None:
        with tempfile.TemporaryDirectory(prefix="test-land-reject-") as tmp_str:
            tmp = Path(tmp_str)
            _, seed, revs = init_sample_repo(self.compiler, tmp)

            manifest = tmp / "manifest.json"
            manifest.write_text(
                json.dumps(
                    {
                        "unit": "bad-unit",
                        "messages": [
                            {"id": "link.test.a1", "risk": "routine", "source_revision": revs["link.test.a1"]},
                            {"id": "link.test.terms", "risk": "legal", "source_revision": revs["link.test.terms"]},
                        ],
                    }
                ),
                encoding="utf-8",
            )
            # 1. Rejects excluded legal message stuffed into FTL
            ftl_bad = tmp / "bad.ftl"
            ftl_bad.write_text(
                "link.test.a1 = abre solstone en tu teléfono\n"
                "link.test.terms = terms of service stay english\n",
                encoding="utf-8",
            )
            jsonl = tmp / "bad.jsonl"
            jsonl.write_text(
                json.dumps({"id": "link.test.a1", "status": "drafted", "reason": None, "flags": [], "note": ""})
                + "\n"
                + json.dumps({"id": "link.test.terms", "status": "excluded", "reason": "legal", "flags": [], "note": ""})
                + "\n",
                encoding="utf-8",
            )
            proc1 = run(
                [
                    "python3",
                    str(LAND_DRAFTS),
                    "verify",
                    "--compiler",
                    str(self.compiler),
                    "--catalog",
                    str(seed / "i18n/catalog"),
                    "--manifest",
                    str(manifest),
                    "--ftl",
                    str(ftl_bad),
                    "--drafts-jsonl",
                    str(jsonl),
                ],
                check=False,
            )
            self.assertNotEqual(proc1.returncode, 0)
            self.assertIn("present in candidate FTL", proc1.stderr)

            # 2. Rejects fixed-term casing break (Solstone instead of solstone)
            ftl_case = tmp / "case.ftl"
            ftl_case.write_text("link.test.a1 = abre Solstone en tu teléfono\n", encoding="utf-8")
            proc2 = run(
                [
                    "python3",
                    str(LAND_DRAFTS),
                    "verify",
                    "--compiler",
                    str(self.compiler),
                    "--catalog",
                    str(seed / "i18n/catalog"),
                    "--manifest",
                    str(manifest),
                    "--ftl",
                    str(ftl_case),
                    "--drafts-jsonl",
                    str(jsonl),
                ],
                check=False,
            )
            self.assertNotEqual(proc2.returncode, 0)
            self.assertIn("changed casing", proc2.stderr)

    def test_parallel_push_race_retries_and_preserves_all_shards(self) -> None:
        from concurrent.futures import ThreadPoolExecutor

        with tempfile.TemporaryDirectory(prefix="test-land-race-") as tmp_str:
            tmp = Path(tmp_str)
            remote, seed, revs = init_sample_repo(self.compiler, tmp)

            units = [
                ("s1", "link.test.a1", "routine", "link.test.a1 = abre solstone en tu teléfono\n"),
                ("s2", "link.test.a2", "routine", "link.test.a2 = mantén tu diario privado\n"),
                (
                    "s3",
                    "link.test.b1",
                    "routine",
                    "link.test.b1 = { $count ->\n    [one] { $count } dispositivo vinculado\n    [many] { $count } dispositivos vinculados\n   *[other] { $count } dispositivos vinculados\n}\n",
                ),
                ("s4", "link.test.b2", "consent", "link.test.b2 = visita solstone.me para detalles\n"),
            ]

            def launch(spec: tuple[str, str, str, str]) -> dict:
                shard, mid, risk, ftl_body = spec
                clone = tmp / f"clone-{shard}"
                run(["git", "clone", str(remote), str(clone)])
                run(["git", "config", "user.name", shard], cwd=clone)
                run(["git", "config", "user.email", f"{shard}@example.invalid"], cwd=clone)
                manifest = tmp / f"m-{shard}.json"
                manifest.write_text(
                    json.dumps(
                        {"unit": shard, "messages": [{"id": mid, "risk": risk, "source_revision": revs[mid]}]}
                    ),
                    encoding="utf-8",
                )
                ftl = tmp / f"{shard}.ftl"
                ftl.write_text(ftl_body, encoding="utf-8")
                jsonl = tmp / f"{shard}.jsonl"
                jsonl.write_text(
                    json.dumps({"id": mid, "status": "drafted", "reason": None, "flags": [], "note": ""}) + "\n",
                    encoding="utf-8",
                )
                proc = run(
                    [
                        "python3",
                        str(LAND_DRAFTS),
                        "land",
                        "--repo",
                        str(clone),
                        "--compiler",
                        str(self.compiler),
                        "--entry",
                        f"i18n/catalog:{manifest}:{ftl}:{jsonl}:{shard}",
                        "--message",
                        f"i18n: land es-419 {shard}",
                    ]
                )
                return json.loads(proc.stdout)

            with ThreadPoolExecutor(max_workers=4) as pool:
                outcomes = list(pool.map(launch, units))

            self.assertEqual(len(outcomes), 4)
            for out in outcomes:
                self.assertEqual(out["status"], "pushed")

            run(["git", "pull", "--ff-only", "origin", "main"], cwd=seed)
            for shard, _, _, _ in units:
                self.assertTrue((seed / f"i18n/catalog/translations/es-419/{shard}.ftl").is_file())
            st = json.loads(
                run(
                    [str(self.compiler), "status", str(seed / "i18n/catalog"), "--locale", "es-419", "--json"]
                ).stdout
            )
            self.assertEqual(
                st["fresh"],
                ["link.test.a1", "link.test.a2", "link.test.b1", "link.test.b2"],
            )
            self.assertEqual(st["stale"], [])
            self.assertEqual(st["excluded_restricted"], ["link.test.statute", "link.test.terms"])

    def test_fixed_terms_allow_sentence_final_period_and_catch_casing(self):
        import importlib.util

        spec = importlib.util.spec_from_file_location("land_drafts", LAND_DRAFTS)
        mod = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(mod)
        terms = ["sol pbc", "solstone", "vit", "solstone.me"]
        self.assertEqual(
            mod.check_fixed_terms(
                {
                    "a": "a = open with the solstone app.",
                    "b": "b = open in sol pbc's apps.",
                    "c": "c = visit solstone.app/install",
                },
                {
                    "a": "a = abre con la app de solstone.",
                    "b": "b = se abre en las apps de sol pbc.",
                    "c": "c = visita solstone.app/install",
                },
                terms,
            ),
            [],
        )
        violations = mod.check_fixed_terms(
            {"a": "a = open with the solstone app."},
            {"a": "a = abre con la app de Solstone."},
            terms,
        )
        self.assertEqual(len(violations), 1)
        self.assertIn("changed casing", violations[0])


if __name__ == "__main__":
    unittest.main()
