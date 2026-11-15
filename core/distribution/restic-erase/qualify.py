#!/usr/bin/env python3
"""Qualify the erase-paths helper against stock restic, on synthetic data only.

Every case builds a fresh repository in a temporary directory under /var/tmp,
backs up a synthetic journal with STOCK restic, runs the helper, and then
checks with STOCK restic alone: `check --read-data` passes, every snapshot that
remains restores byte for byte except what was erased, and no snapshot lists
an erased path. The pieces erased are listed independently, with stock
`restic cat tree`, before the helper runs; a physical scan of every pack header
must find all of them before (the positive control) and none after.

Run it for every restic upgrade:

    qualify.py --stock /path/to/restic --helper /path/to/restic-erase
"""
import argparse
import hashlib
import json
import os
import shutil
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import restserver  # noqa: E402

PASSWORD = "synthetic-only"
TOMB = "tombstone.json"


class Repo:
    def __init__(self, root, stock, helper):
        self.root = Path(root)
        self.stock, self.helper = stock, helper
        self.repo = self.root / "repo"
        self.env = dict(os.environ, RESTIC_REPOSITORY=str(self.repo), RESTIC_PASSWORD=PASSWORD,
                        RESTIC_CACHE_DIR=str(self.root / "cache"))
        self.state = self.root / "state.json"
        self.run(self.stock, "init", "-q")

    def run(self, exe, *args, check=True, env=None):
        r = subprocess.run([exe, *args], env=env or self.env, capture_output=True, text=True, timeout=900)
        if check and r.returncode != 0:
            raise AssertionError(f"{' '.join(args)}: rc {r.returncode}\n{r.stderr}")
        return r

    def backup(self, path, *tags, when=None):
        args = ["backup", str(path), "-q", "--host", "synthetic", "--json"]
        for tag in tags:
            args += ["--tag", tag]
        if when:
            args += ["--time", when]
        out = self.run(self.stock, *args).stdout
        return [json.loads(l) for l in out.splitlines() if '"summary"' in l][-1]["snapshot_id"]

    def snapshots(self):
        return json.loads(self.run(self.stock, "snapshots", "--json").stdout)

    def erase(self, request, stop_after=None, check=True):
        req = self.root / f"request-{time.monotonic_ns()}.json"
        req.write_text(json.dumps(request))
        args = ["--retry-lock", "2m", "erase-paths", "--request", str(req), "--state", str(self.state)]
        if stop_after:
            args += ["--stop-after", stop_after]
        r = self.run(self.helper, *args, check=False)
        result = json.loads(r.stdout.strip().splitlines()[-1])
        if check and r.returncode != 0:
            raise AssertionError(f"erase-paths rc {r.returncode}: {result.get('reason')} {result.get('error')}\n{r.stderr}")
        return r.returncode, result

    def scan(self, state):
        r = self.run(self.helper, "--no-cache", "scan-blobs", "--state", str(state))
        return json.loads(r.stdout)

    def ls_paths(self, snap):
        out = self.run(self.stock, "ls", "--json", snap).stdout
        return {json.loads(l)["path"] for l in out.splitlines()[1:] if l.strip()}

    def tree(self, snap, path):
        r = self.run(self.stock, "cat", "tree", f"{snap}:{path}", check=False)
        return json.loads(r.stdout)["nodes"] if r.returncode == 0 else None

    def pieces_under(self, snap, path):
        """Data blob ids under a snapshot path, read with stock restic only."""
        ids = set()
        parent, name = str(path).rsplit("/", 1)
        stack = [(f"{parent}/{n['name']}", n) for n in self.tree(snap, parent) or [] if n["name"] == name]
        while stack:
            p, node = stack.pop()
            if node["type"] == "file":
                ids.update(node.get("content") or [])
            elif node["type"] == "dir":
                stack += [(f"{p}/{c['name']}", c) for c in self.tree(snap, p) or []]
        return ids

    def check(self):
        self.run(self.stock, "check", "--read-data", "-q")

    def restore(self, snap, target):
        self.run(self.stock, "restore", snap, "--target", str(target), "-q")


def write_state(path, data_ids):
    path.write_text(json.dumps({"schema": 1, "targets": sorted(f"data:{i}" for i in data_ids)}))
    return path


def tree_hashes(root):
    out = {}
    for p in sorted(Path(root).rglob("*")):
        if p.is_file():
            out[p.relative_to(root).as_posix()] = hashlib.sha256(p.read_bytes()).hexdigest()
    return out


def write(path, data):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(data)


SEG = "chronicle/20251125/default/093000_300"
KEEP = "chronicle/20251125/default/100000_300"


def journal_case(tmp):
    j = Path(tmp) / "home" / "owner" / "journal"
    write(j / SEG / "audio.m4a", os.urandom(3_000_000))
    write(j / SEG / "transcript.jsonl", b"erased words " * 2000)
    write(j / KEEP / "audio.m4a", os.urandom(2_000_000))
    write(j / "config" / "journal.json", b"{}")
    return j


def contents(repo, tmp, root):
    """Restore every snapshot: {original id: file hashes under root}."""
    out = {}
    for sn in repo.snapshots():
        target = Path(tmp) / "restore" / sn["id"]
        repo.restore(sn["id"], target)
        out[sn.get("original") or sn["id"]] = tree_hashes(target / str(root).lstrip("/"))
        shutil.rmtree(target)
    return out


def minus(files, gone):
    return {p: h for p, h in files.items() if not any(p == g or p.startswith(g + "/") for g in gone)}


def trash_move(j, item, rel):
    dest = j / ".trash" / item / "files" / rel
    dest.parent.mkdir(parents=True, exist_ok=True)
    os.rename(j / rel, dest)
    write(j / ".trash" / item / "item.json", json.dumps({"id": item, "paths": [rel]}).encode())


def tombstone(j, rel, item):
    write(j / rel / TOMB, json.dumps({"cid": item}).encode())


def erase_request(j, item, settled, live=(), seg=SEG):
    return {"roots": [str(j)],
            "targets": [{"entry": item, "path": seg, "marker": {"name": TOMB, "key": "cid", "value": item},
                         "settled": settled, "trash_copies": True},
                        {"entry": item, "path": f".trash/{item}"}],
            "trash": {"dir": ".trash", "files": "files", "live": list(live), "item": "item.json", "paths_key": "paths"}}


def expect_erased(before, after, gone):
    """Everything but the gone paths stays byte for byte; under a gone segment
    path only its tombstone may stay."""
    for key, files in before.items():
        want = {p: h for p, h in files.items()
                if not any(p == g or p.startswith(g + "/") for g in gone) or p.endswith("/" + TOMB)}
        assert after[key] == want, key


def case_basic(stock, helper):
    """Original path, trash path and a kept-before-restore backup; the positive
    control is built with stock restic."""
    with tempfile.TemporaryDirectory(dir="/var/tmp") as tmp:
        repo = Repo(tmp, stock, helper)
        j = journal_case(tmp)
        s1 = repo.backup(j, when="2025-11-25 10:00:00")
        trash_move(j, "t1", SEG)
        tombstone(j, SEG, "t1")
        s2 = repo.backup(j, when="2025-11-26 10:00:00")
        kept = repo.backup(j, "solstone-kept-before-restore", when="2025-11-27 10:00:00")
        shutil.rmtree(j / ".trash/t1")
        repo.backup(j, when="2025-11-28 10:00:00")
        expected = repo.pieces_under(s1, f"{j}/{SEG}") | repo.pieces_under(s2, f"{j}/.trash/t1")
        control = write_state(Path(tmp) / "control.json", expected)
        positive = repo.scan(control)
        assert positive["targets_found"] == len(expected) > 0, positive
        before = contents(repo, tmp, j)
        rc, res = repo.erase(erase_request(j, "t1", "2025-11-26T00:00:00Z"))
        assert res["snapshots_holding"] == 3 and len(res["snapshots_rewritten"]) == 3, res
        assert res["entries"]["t1"]["snapshots"] == 3 and res["entries"]["t1"]["in_use_kept"] == 0, res
        assert res["remaining"] == 0, res
        assert repo.scan(control)["occurrences"] == 0
        repo.check()
        for sn in repo.snapshots():
            paths = repo.ls_paths(sn["id"])
            assert f"{j}/.trash/t1" not in paths, sn["id"]
            assert not any(p.startswith(f"{j}/{SEG}/") and not p.endswith(TOMB) for p in paths), sn["id"]
        tags = {sn.get("original") or sn["id"]: sn.get("tags") for sn in repo.snapshots()}
        assert tags[kept] == ["solstone-kept-before-restore"], tags
        expect_erased(before, contents(repo, tmp, j), [SEG, ".trash/t1"])
        rc, again = repo.erase(erase_request(j, "t1", "2025-11-26T00:00:00Z"))
        assert again["snapshots_holding"] == 0 and again["removed"]["packs_deleted"] == 0, again
        return {"positive_control": positive["targets_found"], "rewritten": 3}


def case_outputs_beside_and_mid_detach(stock, helper):
    """A backup that read the original files after the delete began (before it
    settled) loses them; outputs a late run wrote beside the item's tombstone go
    from every backup; the tombstone stays; a recording that arrived again since
    (the tombstone no longer the item's) is left alone."""
    with tempfile.TemporaryDirectory(dir="/var/tmp") as tmp:
        repo = Repo(tmp, stock, helper)
        j = journal_case(tmp)
        repo.backup(j, when="2025-11-25 10:00:00")
        mid = repo.backup(j, when="2025-11-26 09:30:00")  # after trashed_at, before settled
        trash_move(j, "t1", SEG)
        tombstone(j, SEG, "t1")
        write(j / SEG / "late-output.md", b"made from the deleted recording " * 300)
        beside = repo.backup(j, when="2025-11-27 10:00:00")
        shutil.rmtree(j / ".trash/t1")
        (j / SEG / "late-output.md").unlink()
        (j / SEG / TOMB).unlink()
        write(j / SEG / "audio.m4a", b"the same recording sent again " * 2000)
        again = repo.backup(j, when="2025-11-29 10:00:00")
        before = contents(repo, tmp, j)
        rc, res = repo.erase(erase_request(j, "t1", "2025-11-26T18:00:00Z"))
        assert res["remaining"] == 0, res
        repo.check()
        after = contents(repo, tmp, j)
        assert not any(p.startswith(SEG + "/") for p in after[mid]), "mid-detach copy stayed"
        assert after[beside].get(f"{SEG}/{TOMB}") == before[beside][f"{SEG}/{TOMB}"], "tombstone went"
        assert f"{SEG}/late-output.md" not in after[beside], "outputs beside the tombstone stayed"
        assert after[again] == before[again], "the recording sent again was touched"
        return {"mid_detach": "removed", "beside_tombstone": "removed", "tombstone": "kept", "sent_again": "kept"}


def case_trash_recover_trash(stock, helper):
    """Trashed as t0, recovered, trashed again as t1, erased: the t0 copy goes
    too; another item still in the trash at the same path stays."""
    with tempfile.TemporaryDirectory(dir="/var/tmp") as tmp:
        repo = Repo(tmp, stock, helper)
        j = journal_case(tmp)
        repo.backup(j, when="2025-11-20 10:00:00")
        trash_move(j, "t0", SEG)
        repo.backup(j, when="2025-11-21 10:00:00")
        os.rename(j / ".trash/t0/files" / SEG, j / SEG)
        shutil.rmtree(j / ".trash/t0")
        repo.backup(j, when="2025-11-22 10:00:00")
        write(j / ".trash/t9/files" / SEG / "other.m4a", os.urandom(500_000))
        trash_move(j, "t1", SEG)
        tombstone(j, SEG, "t1")
        repo.backup(j, when="2025-11-26 10:00:00")
        shutil.rmtree(j / ".trash/t1")
        repo.backup(j, when="2025-11-28 10:00:00")
        rc, res = repo.erase(erase_request(j, "t1", "2025-11-26T00:00:00Z", live=["t9"]))
        assert res["remaining"] == 0 and res["entries"]["t1"]["in_use_kept"] == 0, res
        repo.check()
        for sn in repo.snapshots():
            paths = repo.ls_paths(sn["id"])
            assert not any(p.startswith(f"{j}/.trash/t0") for p in paths), sn["id"]
            if sn["time"] >= "2025-11-26":
                assert f"{j}/.trash/t9/files/{SEG}/other.m4a" in paths, "an item still in the trash lost its copy"
        return {"earlier_trash_copy": "removed", "live_trash_item": "kept"}


def case_shared(stock, helper):
    """A kept file byte-identical to an erased one keeps its pieces, counted as kept."""
    with tempfile.TemporaryDirectory(dir="/var/tmp") as tmp:
        repo = Repo(tmp, stock, helper)
        j = journal_case(tmp)
        same = os.urandom(1_000_000)
        write(j / SEG / "copy.bin", same)
        write(j / KEEP / "copy.bin", same)
        repo.backup(j, when="2025-11-25 10:00:00")
        trash_move(j, "t1", SEG)
        tombstone(j, SEG, "t1")
        repo.backup(j, when="2025-11-26 10:00:00")
        shutil.rmtree(j / ".trash/t1")
        before = contents(repo, tmp, j)
        rc, res = repo.erase(erase_request(j, "t1", "2025-11-26T00:00:00Z"))
        e = res["entries"]["t1"]
        assert e["in_use_kept"] >= 1 and e["in_use_trash"] == 0, res
        assert all(KEEP in p for p in e["in_use_at"]), e
        assert res["remaining"] == 0, res
        repo.check()
        expect_erased(before, contents(repo, tmp, j), [SEG, ".trash/t1"])
        return {"in_use_kept": e["in_use_kept"], "in_use_at": e["in_use_at"]}


OTHER = "chronicle/20251125/default/110000_300"


def stop_fixture(tmp, stock, helper):
    repo = Repo(tmp, stock, helper)
    j = journal_case(tmp)
    write(j / OTHER / "audio.m4a", os.urandom(1_500_000))
    repo.backup(j, when="2025-11-25 10:00:00")
    repo.backup(j, when="2025-11-25 11:00:00")
    trash_move(j, "t1", SEG)
    tombstone(j, SEG, "t1")
    trash_move(j, "t2", OTHER)
    tombstone(j, OTHER, "t2")
    repo.backup(j, when="2025-11-26 10:00:00")
    shutil.rmtree(j / ".trash/t1")
    shutil.rmtree(j / ".trash/t2")
    return repo, j


def both(j):
    req = erase_request(j, "t1", "2025-11-26T00:00:00Z")
    req["targets"] += erase_request(j, "t2", "2025-11-26T00:00:00Z", seg=OTHER)["targets"]
    return req


def case_stops(stock, helper):
    """A stop after each phase leaves a healthy repository; a retry with the
    same request, or with one more entry, finishes with no duplicate snapshot."""
    out = {}
    for phase in ["save", "rewrite-save", "rewrite"]:
        for grown in [False, True]:
            with tempfile.TemporaryDirectory(dir="/var/tmp") as tmp:
                repo, j = stop_fixture(tmp, stock, helper)
                before = contents(repo, tmp, j)
                rc, res = repo.erase(erase_request(j, "t1", "2025-11-26T00:00:00Z"), stop_after=phase, check=False)
                assert rc != 0 and res["reason"] == "stopped", res
                assert json.loads(repo.state.read_text())["targets"], "list not saved"
                repo.check()
                rc, res = repo.erase(both(j) if grown else erase_request(j, "t1", "2025-11-26T00:00:00Z"))
                assert res["remaining"] == 0, res
                repo.check()
                snaps = repo.snapshots()
                originals = [sn.get("original") or sn["id"] for sn in snaps]
                assert len(snaps) == 3 and len(set(originals)) == 3, "a retry left a duplicate snapshot"
                gone = [SEG, ".trash/t1"] + ([OTHER, ".trash/t2"] if grown else [])
                expect_erased(before, contents(repo, tmp, j), gone)
                out[f"{phase}{'+grown' if grown else ''}"] = "resumed"
    return out


def case_two_runners(stock, helper):
    """Two runners at once, the first stopping part way: the second reads the
    list only once it holds the lock, so nothing the first found is lost."""
    with tempfile.TemporaryDirectory(dir="/var/tmp") as tmp:
        repo, j = stop_fixture(tmp, stock, helper)
        result = {}

        def first():
            result["a"] = repo.erase(erase_request(j, "t1", "2025-11-26T00:00:00Z"), stop_after="rewrite", check=False)

        def second():
            result["b"] = repo.erase(both(j), check=False)

        locks = repo.repo / "locks"
        ta = threading.Thread(target=first)
        ta.start()
        deadline = time.monotonic() + 60
        while not any(locks.iterdir()):
            assert time.monotonic() < deadline
            time.sleep(0.01)
        tb = threading.Thread(target=second)
        tb.start()
        ta.join()
        tb.join()
        rc, res = result["b"]
        assert rc == 0 and res["remaining"] == 0, res
        repo.check()
        listed = json.loads(repo.state.read_text())["targets"]
        state = Path(tmp) / "listed.json"
        state.write_text(json.dumps({"schema": 1, "targets": listed}))
        assert repo.scan(state)["occurrences"] == 0
        return {"first": result["a"][1]["reason"], "second": "finished", "listed": len(listed)}


def case_racing_backup(stock, helper):
    """Whichever starts second waits on the lock, and both succeed."""
    out = {}
    for first in ["erase", "backup"]:
        with tempfile.TemporaryDirectory(dir="/var/tmp") as tmp:
            repo = Repo(tmp, stock, helper)
            j = journal_case(tmp)
            for n in range(400):
                write(j / f"chronicle/20251124/default/{n:06d}_300/audio.m4a", os.urandom(200_000))
            repo.backup(j, when="2025-11-25 10:00:00")
            trash_move(j, "t1", SEG)
            tombstone(j, SEG, "t1")
            repo.backup(j, when="2025-11-26 10:00:00")
            shutil.rmtree(j / ".trash/t1")
            for n in range(400):
                write(j / f"chronicle/20251123/default/{n:06d}_300/audio.m4a", os.urandom(200_000))
            locks = repo.repo / "locks"
            result = {}

            def backup():
                r = repo.run(stock, "backup", str(j), "--retry-lock", "2m", check=False)
                result["backup"] = r.returncode

            def erase():
                result["erase"] = repo.erase(erase_request(j, "t1", "2025-11-26T00:00:00Z"), check=False)

            a, b = (erase, backup) if first == "erase" else (backup, erase)
            ta = threading.Thread(target=a)
            ta.start()
            deadline = time.monotonic() + 60
            while not any(locks.iterdir()):
                assert time.monotonic() < deadline, "no lock appeared"
                time.sleep(0.01)
            tb = threading.Thread(target=b)
            tb.start()
            ta.join()
            tb.join()
            assert result["backup"] == 0, result
            rc, res = result["erase"]
            assert rc == 0 and res["remaining"] == 0, res
            repo.check()
            assert len(repo.snapshots()) == 3
            out[f"{first}_first"] = "both succeeded"
    return out


def case_delete_faults(stock, helper):
    """A refused delete and a delete acknowledged but not done both fail with
    stable reasons, and the retry finishes once deletes work."""
    out = {}
    for fault, reason in [("refuse-delete", "delete_refused"), ("fake-delete", "residue")]:
        with tempfile.TemporaryDirectory(dir="/var/tmp") as tmp:
            repo = Repo(tmp, stock, helper)
            j = journal_case(tmp)
            repo.backup(j, when="2025-11-25 10:00:00")
            trash_move(j, "t1", SEG)
            tombstone(j, SEG, "t1")
            repo.backup(j, when="2025-11-26 10:00:00")
            shutil.rmtree(j / ".trash/t1")
            server, url = restserver.start(repo.repo, str(Path(tmp) / "rest.log"))
            local_env = repo.env
            repo.env = dict(local_env, RESTIC_REPOSITORY=url)
            server.fault = fault
            rc, res = repo.erase(erase_request(j, "t1", "2025-11-26T00:00:00Z"), check=False)
            assert rc != 0 and res["reason"] == reason, res
            repo.check()
            assert repo.scan(repo.state)["occurrences"] > 0, "the fault left nothing to find"
            server.fault = None
            rc, res = repo.erase(erase_request(j, "t1", "2025-11-26T00:00:00Z"))
            assert res["remaining"] == 0, res
            assert repo.scan(repo.state)["occurrences"] == 0
            server.shutdown()
            repo.env = local_env
            repo.check()
            out[fault] = reason
    return out


def case_archive_paths(stock, helper):
    """Media archives store files at staging paths: exact per-archive paths,
    keyed by a short id, remove only the erased media; an archive left with
    nothing is forgotten; a rewritten archive is found again by its original."""
    with tempfile.TemporaryDirectory(dir="/var/tmp") as tmp:
        repo = Repo(tmp, stock, helper)
        stage = Path(tmp) / "stage"
        write(stage / "a" / "erased.flac", os.urandom(800_000))
        write(stage / "a" / "kept.flac", os.urandom(800_000))
        mixed = repo.backup(stage / "a", "spb-offload-archive")
        write(stage / "b" / "erased2.flac", os.urandom(800_000))
        only = repo.backup(stage / "b", "spb-offload-archive")
        j = journal_case(tmp)
        repo.backup(j)
        kept_before = tree_hashes(stage / "a")
        request = {"roots": [], "targets": [], "drop_empty_tags": ["spb-offload-archive"],
                   "snapshot_paths": {mixed[:8]: [{"entry": "t1", "path": f"{stage}/a/erased.flac"}],
                                      only: [{"entry": "t1", "path": f"{stage}/b/erased2.flac"}]}}
        rc, res = repo.erase(request)
        assert res["snapshots_dropped"] == [only], res
        assert res["remaining"] == 0, res
        repo.check()
        snaps = {sn.get("original") or sn["id"]: sn for sn in repo.snapshots()}
        assert only not in snaps and mixed in snaps and snaps[mixed]["tags"] == ["spb-offload-archive"], snaps
        target = Path(tmp) / "restore-mixed"
        repo.restore(snaps[mixed]["id"], target)
        got = tree_hashes(target / str(stage / "a").lstrip("/"))
        assert got == {"kept.flac": kept_before["kept.flac"]}, got
        rc, res = repo.erase({"roots": [], "targets": [], "drop_empty_tags": ["spb-offload-archive"],
                              "snapshot_paths": {mixed: [{"entry": "t2", "path": f"{stage}/a/kept.flac"}]}})
        assert res["snapshots_dropped"] and res["remaining"] == 0, res
        return {"mixed": "kept media restores", "only_erased": "forgotten", "second_erase": "found by original"}


def case_nothing_held(stock, helper):
    """An entry no backup held reports zero snapshots, so the caller can tell it
    apart from one that was removed."""
    with tempfile.TemporaryDirectory(dir="/var/tmp") as tmp:
        repo = Repo(tmp, stock, helper)
        j = journal_case(tmp)
        repo.backup(j)
        req = erase_request(j, "t5", "2025-01-01T00:00:00Z", seg="chronicle/20250101/default/000000_300")
        rc, res = repo.erase(req)
        assert res["entries"].get("t5", {"snapshots": 0})["snapshots"] == 0 and res["snapshots_holding"] == 0, res
        return {"snapshots": 0}


CASES = [case_basic, case_outputs_beside_and_mid_detach, case_trash_recover_trash, case_shared, case_stops,
         case_two_runners, case_racing_backup, case_delete_faults, case_archive_paths, case_nothing_held]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--stock", required=True)
    ap.add_argument("--helper", required=True)
    ap.add_argument("--out")
    ap.add_argument("--only")
    a = ap.parse_args()
    report = {"stock": subprocess.run([a.stock, "version"], capture_output=True, text=True).stdout.strip(),
              "helper": subprocess.run([a.helper, "version"], capture_output=True, text=True).stdout.strip(),
              "cases": {}}
    ok = True
    for case in CASES:
        if a.only and case.__name__ != a.only:
            continue
        started = time.monotonic()
        try:
            report["cases"][case.__name__] = {"ok": True, "detail": case(a.stock, a.helper)}
        except Exception as e:  # noqa: BLE001 - every case reports
            ok = False
            report["cases"][case.__name__] = {"ok": False, "error": str(e)[:2000]}
        report["cases"][case.__name__]["seconds"] = round(time.monotonic() - started, 1)
        print(case.__name__, json.dumps(report["cases"][case.__name__]), flush=True)
    if a.out:
        Path(a.out).write_text(json.dumps(report, indent=2) + "\n")
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
