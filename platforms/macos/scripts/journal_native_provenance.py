#!/usr/bin/env python3
"""Fail-closed source binding and receipt for the native journal payload."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import pathlib
import plistlib
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile
from datetime import datetime, timezone


SHA40_RE = re.compile(r"^[0-9a-f]{40}$")
SHA256_RE = re.compile(r"^[0-9a-f]{64}$")
VERSION_RE = re.compile(r"^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$")
TARGET_RE = re.compile(r"^[a-z0-9]+(-[a-z0-9]+)*$")
MINISIGN_PUBLIC_KEY = "RWRE2eBJv3NAtN0mF5+kqygYyP/ocYNw1Ng9yJhAKgyTflNV9NabMMjq"
PLACEHOLDER_DIGESTS = {character * 64 for character in "0123456789abcdef"}


def die(message: str) -> None:
    print(f"error: {message}", file=sys.stderr)
    raise SystemExit(1)


def git(source_dir: pathlib.Path, *args: str) -> str:
    try:
        result = subprocess.run(
            ["git", "-C", str(source_dir), *args],
            check=True,
            capture_output=True,
            text=True,
        )
    except (OSError, subprocess.CalledProcessError) as exc:
        die(f"cannot inspect journal source {source_dir}: {exc}")
    return result.stdout.strip()


def verify_source(source_dir: pathlib.Path, expected_commit: str) -> str:
    if not SHA40_RE.fullmatch(expected_commit):
        die("JOURNAL_NATIVE_EXPECTED_COMMIT must be an exact 40-character lowercase commit")
    source_dir = source_dir.resolve()
    if git(source_dir, "rev-parse", "--is-inside-work-tree") != "true":
        die(f"journal source is not a git worktree: {source_dir}")
    actual_commit = git(source_dir, "rev-parse", "HEAD")
    if actual_commit != expected_commit:
        die(
            f"journal source HEAD is {actual_commit}, expected {expected_commit}; "
            "refusing to package a different revision"
        )
    dirty = git(source_dir, "status", "--porcelain=v1", "--untracked-files=all")
    if dirty:
        die(f"journal source is dirty at {source_dir}; refusing to package\n{dirty}")
    return actual_commit


def sha256(path: pathlib.Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def regular_file(value: str, label: str) -> pathlib.Path:
    path = pathlib.Path(value)
    if path.is_symlink():
        die(f"{label} must not be a symlink: {path}")
    try:
        resolved = path.resolve(strict=True)
    except OSError as exc:
        die(f"{label} cannot be resolved: {path}: {exc}")
    if not resolved.is_file():
        die(f"{label} is not a regular file: {resolved}")
    return resolved


def json_object(path: pathlib.Path, label: str) -> dict:
    def unique_object(pairs: list[tuple[str, object]]) -> dict:
        result = {}
        for key, value in pairs:
            if key in result:
                raise ValueError(f"duplicate JSON key: {key}")
            result[key] = value
        return result

    try:
        value = json.loads(
            path.read_text(encoding="utf-8"), object_pairs_hook=unique_object
        )
    except (OSError, UnicodeError, json.JSONDecodeError, ValueError) as exc:
        die(f"cannot read {label} {path}: {exc}")
    if not isinstance(value, dict):
        die(f"{label} must contain a JSON object: {path}")
    return value


def release_fields(path: pathlib.Path) -> dict[str, str]:
    fields: dict[str, str] = {}
    try:
        lines = path.read_text(encoding="utf-8").splitlines()
    except (OSError, UnicodeError) as exc:
        die(f"cannot read native release receipt {path}: {exc}")
    for line in lines:
        if not line or "=" not in line:
            die(f"native release receipt contains a malformed line: {line!r}")
        key, value = line.split("=", 1)
        if not key or not value or key in fields:
            die(f"native release receipt contains an invalid field: {key!r}")
        fields[key] = value
    return fields


def runtime_tree_map(runtime_dir: pathlib.Path) -> tuple[str, str]:
    """Return the exact 59739f7 path/content map and its SHA-256."""
    if runtime_dir.is_symlink() or not runtime_dir.is_dir():
        die(f"native journal runtime must be a real directory: {runtime_dir}")

    lines: list[str] = []

    def display(parts: tuple[str, ...]) -> str:
        return "." if not parts else "./" + "/".join(parts)

    def visit(directory: pathlib.Path, parts: tuple[str, ...]) -> None:
        lines.append(display(parts))
        try:
            entries = list(os.scandir(directory))
        except OSError as exc:
            die(f"cannot enumerate native journal runtime {directory}: {exc}")
        for entry in entries:
            child_parts = (*parts, entry.name)
            shown = display(child_parts)
            try:
                if entry.is_symlink():
                    lines.append(f"{shown} -> {os.readlink(entry.path)}")
                elif entry.is_dir(follow_symlinks=False):
                    visit(pathlib.Path(entry.path), child_parts)
                elif entry.is_file(follow_symlinks=False):
                    lines.append(f"{sha256(pathlib.Path(entry.path))}  {shown}")
                else:
                    die(f"native journal runtime contains a special entry: {shown}")
            except OSError as exc:
                die(f"cannot inspect native journal runtime entry {shown}: {exc}")

    visit(runtime_dir, ())
    lines.sort(key=lambda line: line.encode("utf-8"))
    tree_map = "\n".join(lines) + "\n"
    return tree_map, hashlib.sha256(tree_map.encode("utf-8")).hexdigest()


def verify_runtime_matches_archive(
    archive: pathlib.Path, runtime_dir: pathlib.Path, native_receipt_name: str
) -> None:
    expected_files: set[str] = set()
    expected_directories = {"."}
    seen: set[str] = set()
    try:
        with tarfile.open(archive, mode="r:gz") as bundle:
            for member in bundle.getmembers():
                name = member.name.rstrip("/")
                parts = name.split("/")
                if (
                    not name
                    or member.name.startswith("/")
                    or any(part in ("", ".", "..") for part in parts)
                    or name in seen
                ):
                    die(f"accepted journal archive has an unsafe or duplicate member: {member.name}")
                seen.add(name)
                for length in range(1, len(parts)):
                    expected_directories.add("/".join(parts[:length]))
                target = runtime_dir.joinpath(*parts)
                if member.isdir():
                    expected_directories.add(name)
                    if target.is_symlink() or not target.is_dir():
                        die(f"composed runtime directory mismatch: {name}")
                    continue
                if not member.isfile():
                    die(
                        "accepted journal archive member is not a regular file or directory: "
                        f"{member.name}"
                    )
                expected_files.add(name)
                if target.is_symlink() or not target.is_file():
                    die(f"composed runtime file is missing: {name}")
                source = bundle.extractfile(member)
                if source is None:
                    die(f"accepted journal archive member cannot be read: {member.name}")
                digest = hashlib.sha256()
                with source:
                    for chunk in iter(lambda: source.read(1024 * 1024), b""):
                        digest.update(chunk)
                if sha256(target) != digest.hexdigest():
                    die(f"composed runtime differs from accepted archive: {name}")
    except (OSError, tarfile.TarError) as exc:
        die(f"cannot verify accepted journal archive {archive}: {exc}")

    if native_receipt_name in expected_files or native_receipt_name in expected_directories:
        die("accepted journal archive collides with the app-owned native receipt")
    expected_files.add(native_receipt_name)

    actual_files: set[str] = set()
    actual_directories = {"."}
    for root, directory_names, file_names in os.walk(runtime_dir, followlinks=False):
        root_path = pathlib.Path(root)
        relative_root = root_path.relative_to(runtime_dir)
        for name in directory_names:
            path = root_path / name
            relative = (relative_root / name).as_posix()
            if path.is_symlink():
                die(f"composed runtime contains an unexpected symlink: {relative}")
            actual_directories.add(relative)
        for name in file_names:
            path = root_path / name
            relative = (relative_root / name).as_posix()
            if path.is_symlink() or not path.is_file():
                die(f"composed runtime contains a special entry: {relative}")
            actual_files.add(relative)
    if actual_files != expected_files:
        added = sorted(actual_files - expected_files)
        missing = sorted(expected_files - actual_files)
        die(f"composed runtime file inventory mismatch: added={added} missing={missing}")
    if actual_directories != expected_directories:
        added = sorted(actual_directories - expected_directories)
        missing = sorted(expected_directories - actual_directories)
        die(f"composed runtime directory inventory mismatch: added={added} missing={missing}")


def verify_minisign(
    executable: pathlib.Path, manifest: pathlib.Path, signature: pathlib.Path
) -> None:
    if not executable.is_file() or not os.access(executable, os.X_OK):
        die(f"minisign verifier is not executable: {executable}")
    try:
        result = subprocess.run(
            [
                str(executable),
                "-Vm",
                str(manifest),
                "-x",
                str(signature),
                "-P",
                MINISIGN_PUBLIC_KEY,
            ],
            capture_output=True,
            text=True,
            check=False,
        )
    except OSError as exc:
        die(f"cannot run minisign verifier {executable}: {exc}")
    if result.returncode != 0:
        detail = (result.stderr or result.stdout).strip()
        die(f"native manifest signature verification failed: {detail}")


def candidate_provenance(args: argparse.Namespace) -> tuple[dict, str]:
    if not SHA40_RE.fullmatch(args.expected_commit):
        die("JOURNAL_NATIVE_EXPECTED_COMMIT must be an exact 40-character lowercase commit")
    if not SHA256_RE.fullmatch(args.expected_sha256):
        die("JOURNAL_NATIVE_ACCEPTED_SHA256 must be an exact lowercase SHA-256")

    archive = regular_file(args.archive, "accepted journal archive")
    manifest_path = regular_file(args.manifest, "accepted journal manifest")
    signature_path = regular_file(
        args.manifest_signature, "accepted journal manifest signature"
    )
    release_path = regular_file(args.release_receipt, "accepted journal release receipt")
    signing_path = regular_file(args.signing_receipt, "accepted journal signing receipt")
    minisign = pathlib.Path(args.minisign).resolve()

    archive_sha256 = sha256(archive)
    if archive_sha256 != args.expected_sha256:
        die(
            f"accepted journal archive SHA-256 is {archive_sha256}, expected "
            f"{args.expected_sha256}"
        )
    verify_minisign(minisign, manifest_path, signature_path)

    manifest = json_object(manifest_path, "accepted journal manifest")
    if set(manifest) != {"product", "version", "target", "files"}:
        die("accepted journal manifest has an unexpected schema")
    if manifest.get("product") != "solstone-journal" or manifest.get("target") != args.target:
        die("accepted journal manifest product/target mismatch")
    version = manifest.get("version")
    files = manifest.get("files")
    if not isinstance(version, str) or not version or not isinstance(files, dict):
        die("accepted journal manifest version/files are malformed")

    bound_files = {
        archive.name: archive_sha256,
        release_path.name: sha256(release_path),
        signing_path.name: sha256(signing_path),
    }
    for name, digest in bound_files.items():
        if files.get(name) != digest:
            die(f"accepted journal manifest digest mismatch for {name}")

    release = release_fields(release_path)
    expected_release = {
        "product": "solstone-journal",
        "version": version,
        "target": args.target,
        "commit": args.expected_commit,
    }
    for key, expected in expected_release.items():
        if release.get(key) != expected:
            die(f"accepted journal release receipt {key} mismatch")

    runtime_input = pathlib.Path(args.runtime_dir)
    if runtime_input.is_symlink():
        die(f"native journal runtime must not be a symlink: {runtime_input}")
    runtime_dir = runtime_input.resolve(strict=True)
    native_receipt_path = regular_file(args.native_receipt, "embedded native receipt")
    if native_receipt_path != runtime_dir / native_receipt_path.name:
        die("embedded native receipt must be directly inside the native runtime")
    native_receipt = json_object(native_receipt_path, "embedded native receipt")
    native_source = native_receipt.get("source")
    native_archive = native_receipt.get("archive")
    if (
        native_receipt.get("schema") != 1
        or native_receipt.get("mode") != "accepted-archive"
        or native_receipt.get("target") != args.target
        or not isinstance(native_source, dict)
        or native_source.get("commit") != args.expected_commit
        or not isinstance(native_archive, dict)
        or native_archive.get("sha256") != archive_sha256
        or native_archive.get("name") != archive.name
    ):
        die("embedded native receipt does not bind the accepted archive")
    verify_runtime_matches_archive(archive, runtime_dir, native_receipt_path.name)

    signing = json_object(signing_path, "accepted journal signing receipt")
    members = signing.get("members")
    if not isinstance(members, list) or not members:
        die("accepted journal signing receipt has no member inventory")
    seen: set[str] = set()
    for member in members:
        if not isinstance(member, dict):
            die("accepted journal signing receipt has a malformed member")
        relative = member.get("path")
        expected_digest = member.get("sha256")
        if (
            not isinstance(relative, str)
            or not relative
            or relative.startswith("/")
            or ".." in pathlib.PurePosixPath(relative).parts
            or relative in seen
            or not isinstance(expected_digest, str)
            or not SHA256_RE.fullmatch(expected_digest)
        ):
            die("accepted journal signing receipt has an invalid member")
        seen.add(relative)
        member_path = runtime_dir.joinpath(*pathlib.PurePosixPath(relative).parts)
        if member_path.is_symlink() or not member_path.is_file():
            die(f"accepted signed runtime member is missing: {relative}")
        if sha256(member_path) != expected_digest:
            die(f"accepted signed runtime member digest mismatch: {relative}")

    info_path = regular_file(args.info_plist, "journal app Info.plist")
    try:
        with info_path.open("rb") as handle:
            info = plistlib.load(handle)
    except (OSError, plistlib.InvalidFileException) as exc:
        die(f"cannot read journal app Info.plist {info_path}: {exc}")
    target = {
        "bundle_identifier": info.get("CFBundleIdentifier"),
        "bundle_short_version": info.get("CFBundleShortVersionString"),
        "bundle_version": info.get("CFBundleVersion"),
    }
    if not all(isinstance(value, str) and value for value in target.values()):
        die("journal app Info.plist identity is incomplete")
    # The wrapper can ship independently of its accepted native payload. Bind
    # this app's identity in the generated provenance; native identity remains
    # authenticated above by the signed manifest and matching release receipt.
    if (
        target["bundle_identifier"] != "app.solstone.journal"
        or not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", target["bundle_short_version"])
        or not target["bundle_version"].isdigit()
        or int(target["bundle_version"]) < 1
    ):
        die("journal app identity is invalid")

    tree_map, tree_sha256 = runtime_tree_map(runtime_dir)
    provenance = {
        "schema": "journal-runtime-entry-candidate-provenance",
        "schema_version": 1,
        "source": "J",
        "target": target,
        "runtime_archive_sha256": archive_sha256,
        "manifest_sha256": sha256(manifest_path),
        "release_receipt_sha256": bound_files[release_path.name],
        "signing_receipt_sha256": bound_files[signing_path.name],
        "runtime_tree_sha256": tree_sha256,
    }
    for key in (
        "runtime_archive_sha256",
        "manifest_sha256",
        "release_receipt_sha256",
        "signing_receipt_sha256",
        "runtime_tree_sha256",
    ):
        digest = provenance[key]
        if digest in PLACEHOLDER_DIGESTS:
            die(f"candidate provenance {key} is a placeholder digest")
    return provenance, tree_map


def write_text_atomic(path: pathlib.Path, text: str) -> None:
    path = path.resolve()
    path.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(
        "w",
        encoding="utf-8",
        dir=path.parent,
        prefix=f".{path.name}.",
        delete=False,
    ) as handle:
        handle.write(text)
        temporary_path = pathlib.Path(handle.name)
    temporary_path.chmod(0o644)
    temporary_path.replace(path)


def write_candidate_provenance(args: argparse.Namespace) -> None:
    provenance, tree_map = candidate_provenance(args)
    output = pathlib.Path(args.output)
    if output.is_symlink() or output.name != "runtime-entry-candidate-provenance.json":
        die(f"candidate provenance output is invalid: {output}")
    store_receipt(provenance, output)
    write_text_atomic(pathlib.Path(args.tree_map), tree_map)
    print(
        "journal runtime-entry candidate provenance written: "
        f"archive={provenance['runtime_archive_sha256']} "
        f"tree={provenance['runtime_tree_sha256']}"
    )


def verify_candidate_provenance(args: argparse.Namespace) -> None:
    expected, tree_map = candidate_provenance(args)
    output = regular_file(args.output, "candidate provenance resource")
    actual = json_object(output, "candidate provenance resource")
    if actual != expected:
        die("candidate provenance resource does not match the accepted native set")
    tree_map_path = regular_file(args.tree_map, "candidate runtime tree map")
    try:
        actual_tree_map = tree_map_path.read_text(encoding="utf-8")
    except (OSError, UnicodeError) as exc:
        die(f"cannot read candidate runtime tree map {tree_map_path}: {exc}")
    if actual_tree_map != tree_map:
        die("candidate runtime tree map does not match the composed runtime")
    print(
        "journal runtime-entry candidate provenance verified: "
        f"archive={expected['runtime_archive_sha256']} "
        f"tree={expected['runtime_tree_sha256']}"
    )


def runtime_entries(runtime_dir: pathlib.Path) -> dict[str, dict[str, str]]:
    entries = {}
    for name in ("solstone",):
        entry = runtime_dir / "bin" / name
        if not entry.is_file() or not os.access(entry, os.X_OK):
            die(f"native journal runtime is missing executable bin/{name}")
        entries[name] = {"path": f"bin/{name}", "sha256": sha256(entry)}
    return entries


def store_receipt(receipt: dict, receipt_path: pathlib.Path) -> None:
    receipt_path = receipt_path.resolve()
    receipt_path.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(
        "w",
        encoding="utf-8",
        dir=receipt_path.parent,
        prefix=f".{receipt_path.name}.",
        delete=False,
    ) as handle:
        json.dump(receipt, handle, indent=2, sort_keys=True)
        handle.write("\n")
        temporary_path = pathlib.Path(handle.name)
    # Temporary files are owner-only; Journal.app's resources must be readable
    # by every account on the Mac.
    temporary_path.chmod(0o644)
    temporary_path.replace(receipt_path)


def extract_regular_tree(bundle: tarfile.TarFile, destination: pathlib.Path) -> None:
    """Extract a link-free archive without relying on Python 3.12 filters."""
    seen: set[str] = set()
    for member in bundle.getmembers():
        name = member.name.rstrip("/")
        parts = name.split("/")
        if (
            not name
            or member.name.startswith("/")
            or any(part in ("", ".", "..") for part in parts)
        ):
            die(f"accepted journal archive has an unsafe member path: {member.name}")
        if name in seen:
            die(f"accepted journal archive has a duplicate member path: {name}")
        seen.add(name)

        target = destination.joinpath(*parts)
        if member.isdir():
            target.mkdir(parents=True, exist_ok=True)
            # Keep staging traversable even when the archive declares a
            # restrictive directory; no archive mode may block later members.
            target.chmod(0o755)
            continue
        if not member.isfile():
            die(
                "accepted journal archive member is not a regular file or directory: "
                f"{member.name}"
            )

        target.parent.mkdir(parents=True, exist_ok=True)
        source = bundle.extractfile(member)
        if source is None:
            die(f"accepted journal archive member cannot be read: {member.name}")
        with source, target.open("xb") as output:
            shutil.copyfileobj(source, output)
        mode = member.mode & 0o755
        if not mode & 0o100:
            mode &= ~0o111
        target.chmod(mode | 0o600)


def write_receipt(args: argparse.Namespace) -> None:
    source_dir = pathlib.Path(args.source_dir).resolve()
    actual_commit = verify_source(source_dir, args.expected_commit)
    output_dir = pathlib.Path(args.output_dir).resolve()
    archives = sorted(output_dir.glob("*.tar.gz"))
    if len(archives) != 1 or not archives[0].is_file():
        die(f"expected exactly one native journal archive in {output_dir}")

    runtime_dir = pathlib.Path(args.runtime_dir).resolve()
    entries = runtime_entries(runtime_dir)

    receipt = {
        "schema": 1,
        "generated_at": datetime.now(timezone.utc).isoformat(),
        "source": {
            "checkout_path": str(source_dir),
            "commit": actual_commit,
            "clean": True,
        },
        "target": args.target,
        "archive": {
            "name": archives[0].name,
            "sha256": sha256(archives[0]),
            "length": archives[0].stat().st_size,
        },
        "entries": entries,
    }

    receipt_path = pathlib.Path(args.receipt).resolve()
    store_receipt(receipt, receipt_path)
    print(
        f"journal native provenance: {actual_commit} -> "
        f"{archives[0].name} ({receipt_path})"
    )


def stage_accepted(args: argparse.Namespace) -> None:
    if not SHA40_RE.fullmatch(args.expected_commit):
        die("JOURNAL_NATIVE_EXPECTED_COMMIT must be an exact 40-character lowercase commit")
    if not SHA256_RE.fullmatch(args.expected_sha256):
        die("JOURNAL_NATIVE_ACCEPTED_SHA256 must be an exact lowercase SHA-256")
    if not args.acceptance_evidence.strip():
        die("JOURNAL_NATIVE_ACCEPTANCE_EVIDENCE must name the journal lane's evidence")

    archive_input = pathlib.Path(args.archive)
    if archive_input.is_symlink():
        die(f"accepted journal archive must not be a symlink: {archive_input}")
    try:
        archive = archive_input.resolve(strict=True)
    except OSError as exc:
        die(f"accepted journal archive cannot be resolved: {archive_input}: {exc}")
    if not archive.is_file():
        die(f"accepted journal archive is not a regular file: {archive}")
    actual_sha256 = sha256(archive)
    if actual_sha256 != args.expected_sha256:
        die(
            f"accepted journal archive SHA-256 is {actual_sha256}, expected "
            f"{args.expected_sha256}"
        )

    runtime_input = pathlib.Path(args.runtime_dir)
    if runtime_input.is_symlink():
        die(f"native journal runtime destination must not be a symlink: {runtime_input}")
    runtime_dir = runtime_input.resolve()
    workspace_root = pathlib.Path(args.workspace_root).resolve(strict=True)
    if runtime_dir == workspace_root or not runtime_dir.is_relative_to(workspace_root):
        die(
            f"native journal runtime destination must be inside the workspace: {runtime_dir}"
        )
    if runtime_dir.exists() and not runtime_dir.is_dir():
        die(f"native journal runtime destination is not a directory: {runtime_dir}")

    receipt_path = pathlib.Path(args.receipt).resolve()
    if receipt_path == runtime_dir or not receipt_path.is_relative_to(runtime_dir):
        die(f"accepted journal receipt must be inside the runtime directory: {receipt_path}")
    receipt_relative = receipt_path.relative_to(runtime_dir)

    runtime_dir.parent.mkdir(parents=True, exist_ok=True)
    staged_dir = pathlib.Path(
        tempfile.mkdtemp(prefix=f".{runtime_dir.name}.", dir=runtime_dir.parent)
    )
    try:
        # mkdtemp is owner-only, and this directory becomes Journal.app's
        # solstone-runtime: every account on the Mac must be able to enter it.
        staged_dir.chmod(0o755)
        with tarfile.open(archive, mode="r:gz") as bundle:
            extract_regular_tree(bundle, staged_dir)
        entries = runtime_entries(staged_dir)

        receipt = {
            "schema": 1,
            "generated_at": datetime.now(timezone.utc).isoformat(),
            "mode": "accepted-archive",
            "source": {
                "commit": args.expected_commit,
                "acceptance_evidence": args.acceptance_evidence,
            },
            "target": args.target,
            "archive": {
                "name": archive.name,
                "sha256": actual_sha256,
                "length": archive.stat().st_size,
            },
            "entries": entries,
        }
        store_receipt(receipt, staged_dir / receipt_relative)

        if runtime_dir.exists():
            shutil.rmtree(runtime_dir)
        os.replace(staged_dir, runtime_dir)
    except (OSError, tarfile.TarError) as exc:
        die(f"cannot extract accepted journal archive {archive}: {exc}")
    finally:
        if staged_dir.exists():
            shutil.rmtree(staged_dir)
    print(
        f"accepted journal archive verified: {args.expected_commit} "
        f"{actual_sha256} receipt={receipt_path}"
    )


def accepted_input_names(version: str, target: str) -> dict[str, str]:
    """The published file names of one release target's signed inputs."""
    stem = f"solstone-journal-{version}-{target}"
    return {
        "manifest": f"{stem}.manifest.json",
        "manifest_signature": f"{stem}.manifest.json.minisig",
        "archive": f"{stem}.tar.gz",
        "release_receipt": f"{stem}.release",
        "signing_receipt": f"{stem}.signing.json",
    }


def download(url: str, destination: pathlib.Path) -> None:
    # curl, not urllib: the origin refuses urllib's default User-Agent. Transport
    # is not the trust anchor (the minisign signature and digests below are), so
    # file:// origins are accepted for local mirrors and tests.
    try:
        result = subprocess.run(
            [
                "curl",
                "--fail",
                "--silent",
                "--show-error",
                "--location",
                "--proto",
                "=https,file",
                "--proto-redir",
                "=https",
                "--retry",
                "2",
                "--output",
                str(destination),
                url,
            ],
            capture_output=True,
            text=True,
            check=False,
        )
    except OSError as exc:
        die(f"cannot run curl to fetch {url}: {exc}")
    if result.returncode != 0:
        die(f"cannot fetch {url}: {result.stderr.strip()}")


def fetch_accepted(args: argparse.Namespace) -> None:
    """Fetch one published release target's signed inputs by digest.

    The manifest is trusted only once its minisign signature verifies against
    the pinned release key; every other input is trusted only if its SHA-256
    equals the signed manifest's entry, and the archive's must also equal the
    digest the journal lane accepted. Nothing reaches the output directory
    until all five verify.
    """
    if not VERSION_RE.fullmatch(args.version):
        die("JOURNAL_NATIVE_ACCEPTED_VERSION must be an exact released version such as 2.0.16")
    if not TARGET_RE.fullmatch(args.target):
        die(f"native journal target is malformed: {args.target!r}")
    if not SHA256_RE.fullmatch(args.expected_sha256):
        die("JOURNAL_NATIVE_ACCEPTED_SHA256 must be an exact lowercase SHA-256")
    minisign = pathlib.Path(args.minisign).resolve()
    names = accepted_input_names(args.version, args.target)
    base = f"{args.origin.rstrip('/')}/release/{args.version}"

    output_input = pathlib.Path(args.output_dir)
    if output_input.is_symlink():
        die(f"accepted journal input directory must not be a symlink: {output_input}")
    output_dir = output_input.resolve()
    output_dir.mkdir(parents=True, exist_ok=True)
    staged_dir = pathlib.Path(tempfile.mkdtemp(prefix=".fetch.", dir=output_dir))
    try:
        for key in ("manifest", "manifest_signature"):
            download(f"{base}/{names[key]}", staged_dir / names[key])
        manifest_path = staged_dir / names["manifest"]
        verify_minisign(minisign, manifest_path, staged_dir / names["manifest_signature"])

        manifest = json_object(manifest_path, "published journal manifest")
        if set(manifest) != {"product", "version", "target", "files"}:
            die("published journal manifest has an unexpected schema")
        if (
            manifest.get("product") != "solstone-journal"
            or manifest.get("version") != args.version
            or manifest.get("target") != args.target
        ):
            die("published journal manifest product/version/target mismatch")
        files = manifest.get("files")
        if not isinstance(files, dict):
            die("published journal manifest files are malformed")
        if files.get(names["archive"]) != args.expected_sha256:
            die(
                f"the signed {args.version} manifest does not list {names['archive']} "
                f"with the accepted SHA-256 {args.expected_sha256}"
            )

        for key in ("archive", "release_receipt", "signing_receipt"):
            name = names[key]
            expected = files.get(name)
            if not isinstance(expected, str) or not SHA256_RE.fullmatch(expected):
                die(f"the signed manifest carries no SHA-256 for {name}")
            download(f"{base}/{name}", staged_dir / name)
            actual = sha256(staged_dir / name)
            if actual != expected:
                die(f"fetched {name} has SHA-256 {actual}, the signed manifest says {expected}")

        release = release_fields(staged_dir / names["release_receipt"])
        for name in names.values():
            os.replace(staged_dir / name, output_dir / name)
    finally:
        if staged_dir.exists():
            shutil.rmtree(staged_dir)
    print(
        f"accepted journal inputs fetched: {args.version} {args.target} "
        f"archive={args.expected_sha256} manifest={sha256(output_dir / names['manifest'])} "
        f"release-commit={release.get('commit', '')} from {base}/ into {output_dir}"
    )


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)

    check = subparsers.add_parser("check-source")
    check.add_argument("--source-dir", required=True)
    check.add_argument("--expected-commit", required=True)

    write = subparsers.add_parser("write-receipt")
    write.add_argument("--source-dir", required=True)
    write.add_argument("--expected-commit", required=True)
    write.add_argument("--target", required=True)
    write.add_argument("--output-dir", required=True)
    write.add_argument("--runtime-dir", required=True)
    write.add_argument("--receipt", required=True)

    accepted = subparsers.add_parser("stage-accepted")
    accepted.add_argument("--archive", required=True)
    accepted.add_argument("--expected-sha256", required=True)
    accepted.add_argument("--expected-commit", required=True)
    accepted.add_argument("--acceptance-evidence", required=True)
    accepted.add_argument("--target", required=True)
    accepted.add_argument("--workspace-root", required=True)
    accepted.add_argument("--runtime-dir", required=True)
    accepted.add_argument("--receipt", required=True)

    fetch = subparsers.add_parser("fetch-accepted")
    fetch.add_argument("--version", required=True)
    fetch.add_argument("--target", required=True)
    fetch.add_argument("--expected-sha256", required=True)
    fetch.add_argument("--origin", required=True)
    fetch.add_argument("--minisign", required=True)
    fetch.add_argument("--output-dir", required=True)

    def add_candidate_arguments(candidate: argparse.ArgumentParser) -> None:
        candidate.add_argument("--archive", required=True)
        candidate.add_argument("--expected-sha256", required=True)
        candidate.add_argument("--expected-commit", required=True)
        candidate.add_argument("--target", required=True)
        candidate.add_argument("--manifest", required=True)
        candidate.add_argument("--manifest-signature", required=True)
        candidate.add_argument("--release-receipt", required=True)
        candidate.add_argument("--signing-receipt", required=True)
        candidate.add_argument("--minisign", required=True)
        candidate.add_argument("--runtime-dir", required=True)
        candidate.add_argument("--native-receipt", required=True)
        candidate.add_argument("--info-plist", required=True)
        candidate.add_argument("--output", required=True)
        candidate.add_argument("--tree-map", required=True)

    write_candidate = subparsers.add_parser("write-candidate")
    add_candidate_arguments(write_candidate)
    verify_candidate = subparsers.add_parser("verify-candidate")
    add_candidate_arguments(verify_candidate)

    args = parser.parse_args()
    if args.command == "check-source":
        commit = verify_source(pathlib.Path(args.source_dir), args.expected_commit)
        print(f"journal native source verified: {commit}")
    elif args.command == "write-receipt":
        write_receipt(args)
    elif args.command == "stage-accepted":
        stage_accepted(args)
    elif args.command == "fetch-accepted":
        fetch_accepted(args)
    elif args.command == "write-candidate":
        write_candidate_provenance(args)
    else:
        verify_candidate_provenance(args)


if __name__ == "__main__":
    main()
