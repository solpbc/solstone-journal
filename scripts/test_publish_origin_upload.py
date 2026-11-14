#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
"""Offline publisher integration checks; run directly, outside the routine unit gate."""

import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
CAP = 300 * 1024 * 1024

FAKE = r"""import json, os, sys
from pathlib import Path
args = sys.argv[1:]
def value(flag):
    return args[args.index(flag) + 1]
root = Path(os.environ["FAKE_ORIGIN"])
state_path = root / "state.json"
state = json.loads(state_path.read_text())
s3 = Path(sys.argv[0]).name == "aws"
if s3:
    assert args[:2] == ["s3api", "put-object"]
    assert value("--endpoint-url") == "https://test.r2.cloudflarestorage.com"
    key = value("--key")
    action = "put"
else:
    assert args[:2] == ["r2", "object"]
    action = args[2]
    key = args[3].split("/", 1)[1]
with (root / "calls.jsonl").open("a") as f:
    f.write(json.dumps({"s3": s3, "action": action, "key": key, "args": args}) + "\n")
if action == "get":
    if key not in state:
        print("The specified key does not exist", file=sys.stderr)
        sys.exit(1)
    row = state[key]
    with Path(value("--file")).open("wb") as f:
        f.write(bytes.fromhex(row["head"]))
        f.truncate(row["size"])
else:
    if s3 and os.environ.get("FAKE_FAIL"):
        if os.environ["FAKE_FAIL"] == "race":
            state[key] = {"head": b"competitor".hex(), "size": CAP + 1}
            state_path.write_text(json.dumps(state))
        sys.exit(1)
    if s3 and "--if-none-match" in args and key in state:
        sys.exit(1)
    path = Path(value("--body" if s3 else "--file"))
    size = path.stat().st_size
    if not s3 and size > CAP:
        print("Wrangler upload limit exceeded", file=sys.stderr)
        sys.exit(1)
    with path.open("rb") as f:
        head = f.read(100000)
    state[key] = {"head": head.hex(), "size": size}
    state_path.write_text(json.dumps(state))
""".replace("CAP", str(CAP))


class Upload(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.bin = self.root / "bin"
        self.bin.mkdir()
        for name in ("aws", "wrangler"):
            path = self.bin / name
            path.write_text(f"#!{sys.executable}\n" + FAKE)
            path.chmod(0o755)
        self.stage = self.root / "stage"
        self.lane = self.stage / "solstone-journal/release"
        self.object = self.lane / "9.9.9/payload.tar.gz"
        self.object.parent.mkdir(parents=True)
        (self.lane / "latest").write_text("version=9.9.9\n")
        self.changelog = self.root / "CHANGELOG.md"
        self.changelog.write_text("release notes\n")
        self.state = self.root / "state.json"
        self.state.write_text("{}")
        self.env = dict(
            os.environ,
            PATH=f"{self.bin}:{os.environ['PATH']}",
            FAKE_ORIGIN=str(self.root),
            SOLSTONE_ORIGIN_BUCKET="test-bucket",
            SOLSTONE_R2_ENDPOINT="https://test.r2.cloudflarestorage.com",
            AWS_ACCESS_KEY_ID="test",
            AWS_SECRET_ACCESS_KEY="test",
        )
        self.env.pop("SOLSTONE_ORIGIN_FAIL_AFTER", None)
        self.env.pop("FAKE_FAIL", None)

    def resize(self, size, head=b"payload"):
        with self.object.open("wb") as f:
            f.write(head)
            f.truncate(size)

    def run_upload(self, *args):
        return subprocess.run(
            [
                "bash",
                str(ROOT / "scripts/publish-origin-upload.sh"),
                "--stage",
                str(self.stage),
                "--changelog",
                str(self.changelog),
                *args,
            ],
            cwd=self.root,
            env=self.env,
            capture_output=True,
            text=True,
            timeout=30,
        )

    def calls(self):
        p = self.root / "calls.jsonl"
        return (
            [json.loads(line) for line in p.read_text().splitlines()]
            if p.exists()
            else []
        )

    def test_threshold_and_metadata_and_latest_order(self):
        for size, s3 in ((CAP, False), (CAP + 1, True)):
            with self.subTest(size=size):
                self.resize(size)
                self.state.write_text("{}")
                (self.root / "calls.jsonl").unlink(missing_ok=True)
                r = self.run_upload()
                self.assertEqual(r.returncode, 0, r.stderr)
                puts = [c for c in self.calls() if c["action"] == "put"]
                payload = puts[0]
                self.assertEqual(payload["s3"], s3)
                args = payload["args"]
                self.assertEqual(
                    args[args.index("--content-type") + 1], "application/gzip"
                )
                self.assertEqual(
                    args[args.index("--cache-control") + 1],
                    "public, max-age=31536000, immutable",
                )
                if s3:
                    self.assertEqual(args[args.index("--if-none-match") + 1], "*")
                    self.assertEqual(args[args.index("--bucket") + 1], "test-bucket")
                self.assertEqual(puts[-1]["key"], "solstone-journal/release/latest")

    def test_identical_large_object_is_reused_and_mismatch_refuses(self):
        self.resize(CAP + 1)
        self.assertEqual(self.run_upload().returncode, 0)
        count = len([c for c in self.calls() if c["action"] == "put"])
        r = self.run_upload()
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertEqual(
            len([c for c in self.calls() if c["action"] == "put"]), count + 1
        )  # latest
        self.resize(CAP + 1, b"different")
        r = self.run_upload()
        self.assertNotEqual(r.returncode, 0)
        self.assertIn("object-immutable", r.stderr)
        self.assertEqual(
            len([c for c in self.calls() if c["action"] == "put"]), count + 1
        )

    def test_failed_or_racing_s3_write_never_updates_mutable_objects(self):
        for failure in ("error", "race"):
            with self.subTest(failure=failure):
                self.resize(CAP + 1)
                self.state.write_text("{}")
                self.env["FAKE_FAIL"] = failure
                r = self.run_upload()
                self.assertNotEqual(r.returncode, 0)
                self.assertIn("via S3", r.stderr)
                state = json.loads(self.state.read_text())
                self.assertFalse(
                    any(
                        k.endswith(("latest", "install.sh", "CHANGELOG.md"))
                        for k in state
                    )
                )
                if failure == "race":
                    self.assertEqual(
                        bytes.fromhex(next(iter(state.values()))["head"]), b"competitor"
                    )

    def test_missing_credentials_and_oversize_refuse_without_put(self):
        for size, missing, error in (
            (CAP + 1, True, "origin-auth-missing"),
            (5363466241, False, "object-too-large"),
        ):
            with self.subTest(size=size):
                self.resize(size)
                if missing:
                    self.env.pop("SOLSTONE_R2_ENDPOINT")
                else:
                    self.env["SOLSTONE_R2_ENDPOINT"] = (
                        "https://test.r2.cloudflarestorage.com"
                    )
                r = self.run_upload()
                self.assertNotEqual(r.returncode, 0)
                self.assertIn(error, r.stderr)
                self.assertFalse(any(c["action"] == "put" for c in self.calls()))

    def test_dry_run_needs_no_s3_credentials(self):
        self.resize(CAP + 1)
        self.env.pop("SOLSTONE_R2_ENDPOINT")
        r = self.run_upload("--dry-run")
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertEqual(self.calls(), [])

    def test_dev_large_object_can_overwrite(self):
        dev = self.stage / "solstone-journal/dev"
        self.lane.rename(dev)
        self.object = dev / "9.9.9/payload.tar.gz"
        self.resize(CAP + 1)
        self.assertEqual(self.run_upload().returncode, 0)
        self.resize(CAP + 1, b"changed")
        r = self.run_upload()
        self.assertEqual(r.returncode, 0, r.stderr)
        s3 = [c for c in self.calls() if c["s3"]]
        self.assertEqual(len(s3), 2)
        self.assertTrue(all("--if-none-match" not in c["args"] for c in s3))
        self.assertTrue(
            all(
                c["args"][c["args"].index("--cache-control") + 1] == "no-cache"
                for c in s3
            )
        )


if __name__ == "__main__":
    unittest.main()
