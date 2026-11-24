#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
"""Verify, apply, and concurrency-safely land draft catalog translations."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import random
import re
import shutil
import subprocess
import sys
import tempfile
import time

EXCLUDED_REASONS = frozenset(
    {"legal", "statutory", "refused", "uncertain", "protocol", "tmux"}
)
RESTRICTED_RISKS = frozenset({"legal", "statutory"})
DEFAULT_FIXED_TERMS = ("sol pbc", "solstone", "vit", "solstone.me")


def resolve_fixed_terms(args: argparse.Namespace, manifest: dict) -> list[str]:
    if args.no_fixed_terms:
        return []
    if args.fixed_terms:
        return [t.strip() for t in args.fixed_terms.split(",") if t.strip()]
    if isinstance(manifest.get("fixed_terms"), list):
        return [str(t).strip() for t in manifest["fixed_terms"] if str(t).strip()]
    return list(DEFAULT_FIXED_TERMS)

FTL_ID_RE = re.compile(r"^([A-Za-z][A-Za-z0-9._-]*)\s*=(.*)$")
PLACEABLE_RE = re.compile(r"\{\s*\$[A-Za-z0-9_-]+(?:\s*->)?|\}")


def parse_ftl_entries(text: str) -> dict[str, str]:
    """Parse top-level message ID -> raw block text from a catalog FTL file."""
    entries: dict[str, str] = {}
    current_id: str | None = None
    current_lines: list[str] = []

    def flush() -> None:
        nonlocal current_id, current_lines
        if current_id is not None:
            entries[current_id] = "\n".join(current_lines).rstrip()
            current_id = None
            current_lines = []

    for line in text.splitlines():
        if not line.strip() or line.startswith("#"):
            if current_id is not None and not line.strip():
                current_lines.append("")
            continue
        if not line[0].isspace():
            match = FTL_ID_RE.match(line)
            if match:
                flush()
                msg_id = match.group(1)
                if msg_id in entries:
                    raise ValueError(f"Duplicate message ID in FTL: {msg_id}")
                current_id = msg_id
                current_lines = [line]
                continue
        if current_id is not None:
            current_lines.append(line)
    flush()
    return entries


def strip_ftl_header_and_placeables(raw_block: str) -> str:
    """Return prose body of an FTL message block for fixed-term checking."""
    lines = raw_block.splitlines()
    if not lines:
        return ""
    first = lines[0]
    if "=" in first:
        lines[0] = first.split("=", 1)[1]
    body = "\n".join(lines)
    # Remove selector keys like [one] *[other] and variable references { $arg }
    body = re.sub(r"\*?\s*\[[^\]]+\]", " ", body)
    body = re.sub(r"\{\s*\$[A-Za-z0-9_-]+\s*(?:->)?\s*\}", " ", body)
    body = re.sub(r"\{\s*\$[A-Za-z0-9_-]+\s*->", " ", body)
    return body


def check_fixed_terms(
    source_entries: dict[str, str],
    draft_entries: dict[str, str],
    fixed_terms: list[str],
) -> list[str]:
    """Check that fixed brand marks present in English prose survive verbatim in drafts."""
    violations: list[str] = []
    # Check longer terms first so solstone.me is distinct from solstone
    ordered = sorted(fixed_terms, key=len, reverse=True)
    for msg_id in sorted(draft_entries):
        src_raw = source_entries.get(msg_id, "")
        dst_raw = draft_entries[msg_id]
        src_body = strip_ftl_header_and_placeables(src_raw)
        dst_body = strip_ftl_header_and_placeables(dst_raw)

        for term in ordered:
            # Word-boundary match around alphanumeric ends
            pattern = re.compile(
                rf"(?<![A-Za-z0-9_.-]){re.escape(term)}(?![A-Za-z0-9_.-])",
                re.IGNORECASE,
            )
            src_matches = pattern.findall(src_body)
            if not src_matches:
                continue
            exact_in_src = any(m == term for m in src_matches)
            dst_matches = pattern.findall(dst_body)
            if not dst_matches:
                violations.append(
                    f"{msg_id}: fixed term '{term}' present in source but missing in draft"
                )
            elif exact_in_src and not all(m == term for m in dst_matches):
                bad = sorted({m for m in dst_matches if m != term})
                violations.append(
                    f"{msg_id}: fixed term '{term}' changed casing to {bad}"
                )
    return violations


def load_manifest(path: Path) -> dict:
    data = json.loads(path.read_text(encoding="utf-8"))
    if "messages" not in data or not isinstance(data["messages"], list):
        raise ValueError(f"Manifest {path} missing 'messages' list")
    return data


def load_drafts_jsonl(path: Path) -> list[dict]:
    rows: list[dict] = []
    for lineno, raw in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        line = raw.strip()
        if not line:
            continue
        try:
            row = json.loads(line)
        except json.JSONDecodeError as err:
            raise ValueError(f"{path}:{lineno}: invalid JSON: {err}") from err
        if not isinstance(row, dict) or "id" not in row or "status" not in row:
            raise ValueError(f"{path}:{lineno}: row must have 'id' and 'status'")
        rows.append(row)
    return rows


def validate_slice_records(
    manifest: dict,
    drafts_rows: list[dict],
    ftl_entries: dict[str, str],
) -> tuple[list[str], dict[str, int], list[str]]:
    """Validate exact manifest coverage, exclusion rules, and FTL membership.

    Returns (drafted_ids_sorted, excluded_counts_by_reason, errors).
    """
    errors: list[str] = []
    manifest_by_id: dict[str, dict] = {}
    for entry in manifest["messages"]:
        mid = entry["id"]
        if mid in manifest_by_id:
            errors.append(f"Duplicate ID in manifest: {mid}")
        manifest_by_id[mid] = entry

    rows_by_id: dict[str, dict] = {}
    for row in drafts_rows:
        mid = row["id"]
        if mid in rows_by_id:
            errors.append(f"Duplicate ID in drafts.jsonl: {mid}")
        rows_by_id[mid] = row

    manifest_ids = set(manifest_by_id)
    jsonl_ids = set(rows_by_id)
    ftl_ids = set(ftl_entries)

    missing_from_jsonl = sorted(manifest_ids - jsonl_ids)
    extra_in_jsonl = sorted(jsonl_ids - manifest_ids)
    if missing_from_jsonl:
        errors.append(
            f"Missing {len(missing_from_jsonl)} manifest IDs in drafts.jsonl: "
            + ", ".join(missing_from_jsonl[:10])
        )
    if extra_in_jsonl:
        errors.append(
            f"Found {len(extra_in_jsonl)} out-of-slice IDs in drafts.jsonl: "
            + ", ".join(extra_in_jsonl[:10])
        )

    extra_in_ftl = sorted(ftl_ids - manifest_ids)
    if extra_in_ftl:
        errors.append(
            f"Found {len(extra_in_ftl)} out-of-slice IDs in candidate FTL: "
            + ", ".join(extra_in_ftl[:10])
        )

    drafted_ids: list[str] = []
    excluded_by_reason: dict[str, int] = {}

    for mid in sorted(manifest_ids & jsonl_ids):
        m_entry = manifest_by_id[mid]
        row = rows_by_id[mid]
        status = row.get("status")
        reason = row.get("reason")
        note = row.get("note") or ""
        risk = m_entry.get("risk", "routine")

        if status == "drafted":
            if risk in RESTRICTED_RISKS:
                errors.append(
                    f"{mid}: risk='{risk}' must be excluded, never drafted"
                )
            if reason:
                errors.append(
                    f"{mid}: drafted row must not set reason (got {reason!r})"
                )
            if mid not in ftl_ids:
                errors.append(f"{mid}: marked drafted but missing from candidate FTL")
            drafted_ids.append(mid)
        elif status == "excluded":
            if reason not in EXCLUDED_REASONS:
                errors.append(
                    f"{mid}: invalid exclusion reason {reason!r} "
                    f"(expected one of {sorted(EXCLUDED_REASONS)})"
                )
            if risk in RESTRICTED_RISKS and reason != risk:
                errors.append(
                    f"{mid}: risk='{risk}' must use reason='{risk}' (got {reason!r})"
                )
            if reason == "uncertain" and not str(note).strip():
                errors.append(f"{mid}: uncertain exclusion requires a non-empty note")
            if mid in ftl_ids:
                errors.append(
                    f"{mid}: marked excluded ({reason}) but present in candidate FTL "
                    "(never commit English fills disguised as translations)"
                )
            key = str(reason or "unknown")
            excluded_by_reason[key] = excluded_by_reason.get(key, 0) + 1
        else:
            errors.append(
                f"{mid}: invalid status {status!r} (expected 'drafted' or 'excluded')"
            )

    return sorted(drafted_ids), excluded_by_reason, errors


def translation_dest_path(catalog_dir: Path, locale: str, shard: str | None) -> Path:
    trans_dir = catalog_dir / "translations"
    if shard:
        return trans_dir / locale / f"{shard}.ftl"
    return trans_dir / f"{locale}.ftl"


def format_canonical_ftl(ftl_entries: dict[str, str], ordered_ids: list[str]) -> str:
    blocks = [ftl_entries[mid].rstrip() for mid in ordered_ids if mid in ftl_entries]
    if not blocks:
        return ""
    return "\n\n".join(blocks) + "\n"


def dir_hashes(root: Path) -> dict[str, str]:
    out: dict[str, str] = {}
    for p in sorted(root.rglob("*")):
        if p.is_file():
            rel = p.relative_to(root).as_posix()
            out[rel] = hashlib.sha256(p.read_bytes()).hexdigest()
    return out


def run_cmd(
    cmd: list[str] | str,
    *,
    cwd: Path | None = None,
    shell: bool = False,
    check: bool = True,
) -> subprocess.CompletedProcess[str]:
    proc = subprocess.run(
        cmd,
        cwd=cwd,
        shell=shell,
        text=True,
        capture_output=True,
    )
    if check and proc.returncode != 0:
        raise RuntimeError(
            f"Command failed (exit {proc.returncode}): {cmd}\n"
            f"STDOUT:\n{proc.stdout}\nSTDERR:\n{proc.stderr}"
        )
    return proc


def check_source_revisions(
    catalog_dir: Path, manifest: dict, drafted_ids: list[str]
) -> list[str]:
    """Return drafted IDs whose on-disk meta source_revision differs from manifest."""
    expected = {
        m["id"]: m.get("source_revision")
        for m in manifest["messages"]
        if m.get("source_revision")
    }
    drifted: list[str] = []
    meta_dir = catalog_dir / "meta"
    for mid in drafted_ids:
        exp_rev = expected.get(mid)
        if not exp_rev:
            continue
        meta_path = meta_dir / f"{mid}.json"
        if not meta_path.is_file():
            drifted.append(f"{mid} (missing meta)")
            continue
        cur_rev = json.loads(meta_path.read_text(encoding="utf-8")).get(
            "source_revision"
        )
        if cur_rev != exp_rev:
            drifted.append(mid)
    return drifted


def apply_draft_to_catalog(
    *,
    compiler: Path,
    catalog_dir: Path,
    manifest: dict,
    ftl_path: Path,
    drafts_jsonl_path: Path,
    locale: str,
    shard: str | None,
    allow_drifted_source: bool = False,
    fixed_terms: list[str] | None = None,
) -> dict:
    """Validate and apply a unit's draft FTL + scoped source-lock acks."""
    ftl_text = ftl_path.read_text(encoding="utf-8")
    ftl_entries = parse_ftl_entries(ftl_text)
    drafts_rows = load_drafts_jsonl(drafts_jsonl_path)

    drafted_ids, excluded_by_reason, errors = validate_slice_records(
        manifest, drafts_rows, ftl_entries
    )
    if errors:
        raise RuntimeError("Slice validation failed:\n  " + "\n  ".join(errors))

    source_ftl_path = catalog_dir / "source" / "en-US.ftl"
    source_entries = parse_ftl_entries(source_ftl_path.read_text(encoding="utf-8"))
    if fixed_terms:
        term_errors = check_fixed_terms(source_entries, ftl_entries, fixed_terms)
        if term_errors:
            raise RuntimeError(
                "Fixed-terms check failed:\n  " + "\n  ".join(term_errors)
            )

    drifted = check_source_revisions(catalog_dir, manifest, drafted_ids)
    if drifted and not allow_drifted_source:
        raise RuntimeError(
            "English source_revision changed on disk since manifest freeze for "
            f"{len(drifted)} drafted IDs (re-translate them against HEAD or pass "
            "--allow-drifted-source to leave them stale): "
            + ", ".join(drifted[:10])
        )
    drifted_set = set(drifted)
    ack_ids = [mid for mid in drafted_ids if mid not in drifted_set]

    dest = translation_dest_path(catalog_dir, locale, shard)
    dest.parent.mkdir(parents=True, exist_ok=True)
    # Preserve manifest order for clean, deterministic diffs
    manifest_order = [m["id"] for m in manifest["messages"]]
    ordered_drafted = [mid for mid in manifest_order if mid in ftl_entries]
    canonical_ftl = format_canonical_ftl(ftl_entries, ordered_drafted)
    if canonical_ftl:
        dest.write_text(canonical_ftl, encoding="utf-8")
    elif dest.exists():
        dest.unlink()

    if ack_ids:
        with tempfile.NamedTemporaryFile(
            mode="w", encoding="utf-8", suffix=".ids", delete=False
        ) as tf:
            tf.write("\n".join(ack_ids) + "\n")
            ids_file = Path(tf.name)
        try:
            run_cmd(
                [
                    str(compiler),
                    "lock",
                    str(catalog_dir),
                    "--acknowledge",
                    locale,
                    "--ids-file",
                    str(ids_file),
                ]
            )
            status_proc = run_cmd(
                [
                    str(compiler),
                    "status",
                    str(catalog_dir),
                    "--locale",
                    locale,
                    "--ids-file",
                    str(ids_file),
                    "--json",
                ]
            )
            status_data = json.loads(status_proc.stdout)
        finally:
            ids_file.unlink(missing_ok=True)
    else:
        status_data = {
            "locale": locale,
            "fresh": [],
            "stale": [],
            "invalid": [],
            "excluded_restricted": [],
            "missing": [],
        }

    return {
        "catalog": str(catalog_dir),
        "locale": locale,
        "shard": shard,
        "dest": str(dest),
        "manifest_total": len(manifest["messages"]),
        "drafted": len(drafted_ids),
        "acknowledged": len(ack_ids),
        "drifted_unacked": drifted,
        "excluded_by_reason": excluded_by_reason,
        "fresh_verified": len(status_data.get("fresh", [])),
    }


def cmd_verify(args: argparse.Namespace) -> int:
    compiler = Path(args.compiler).resolve()
    catalog_dir = Path(args.catalog).resolve()
    manifest = load_manifest(Path(args.manifest).resolve())
    ftl_path = Path(args.ftl).resolve()
    drafts_path = Path(args.drafts_jsonl).resolve()
    fixed_terms = resolve_fixed_terms(args, manifest)

    with tempfile.TemporaryDirectory(prefix="i18n-verify-") as tmp_str:
        tmp = Path(tmp_str)
        scratch_cat = tmp / "catalog"
        shutil.copytree(catalog_dir, scratch_cat)

        prod_before_hashes: dict[str, str] | None = None
        targets = (
            [t.strip() for t in args.check_targets.split(",") if t.strip()]
            if args.check_targets
            else []
        )
        if targets:
            prod_before = tmp / "prod-before"
            cmd = [
                str(compiler),
                "targets",
                str(scratch_cat),
                "--channel",
                "production",
                "--out",
                str(prod_before),
            ]
            for tgt in targets:
                cmd.extend(["--target", tgt])
            run_cmd(cmd)
            prod_before_hashes = dir_hashes(prod_before)

        result = apply_draft_to_catalog(
            compiler=compiler,
            catalog_dir=scratch_cat,
            manifest=manifest,
            ftl_path=ftl_path,
            drafts_jsonl_path=drafts_path,
            locale=args.locale,
            shard=args.shard,
            allow_drifted_source=args.allow_drifted_source,
            fixed_terms=fixed_terms,
        )

        # Gate: compile production
        run_cmd([str(compiler), "compile", str(scratch_cat), "--channel", "production"])

        # Gate: compile development (--fallback-stale handles unrelated stale sibling slices)
        compile_dev_cmd = [
            str(compiler),
            "compile",
            str(scratch_cat),
            "--channel",
            "development",
            "--locale",
            args.locale,
        ]
        if args.fallback_stale:
            compile_dev_cmd.append("--fallback-stale")
        run_cmd(compile_dev_cmd)

        # Gate: coverage development
        cov_cmd = [
            str(compiler),
            "coverage",
            str(scratch_cat),
            "--channel",
            "development",
            "--locale",
            args.locale,
        ]
        if args.fallback_stale:
            cov_cmd.append("--fallback-stale")
        cov_proc = run_cmd(cov_cmd)
        result["coverage"] = cov_proc.stdout.strip()

        if targets:
            dev_out = tmp / "dev-out"
            dev_cmd = [
                str(compiler),
                "targets",
                str(scratch_cat),
                "--channel",
                "development",
                "--out",
                str(dev_out),
            ]
            if args.fallback_stale:
                dev_cmd.append("--fallback-stale")
            for tgt in targets:
                dev_cmd.extend(["--target", tgt])
            run_cmd(dev_cmd)

            prod_after = tmp / "prod-after"
            prod_cmd = [
                str(compiler),
                "targets",
                str(scratch_cat),
                "--channel",
                "production",
                "--out",
                str(prod_after),
            ]
            for tgt in targets:
                prod_cmd.extend(["--target", tgt])
            run_cmd(prod_cmd)
            prod_after_hashes = dir_hashes(prod_after)
            if prod_before_hashes != prod_after_hashes:
                raise RuntimeError(
                    "Production target output changed after applying draft translation!"
                )
            result["production_byte_identical"] = True

    print(json.dumps(result, indent=2, ensure_ascii=False))
    return 0


def cmd_apply(args: argparse.Namespace) -> int:
    compiler = Path(args.compiler).resolve()
    catalog_dir = Path(args.catalog).resolve()
    manifest = load_manifest(Path(args.manifest).resolve())
    ftl_path = Path(args.ftl).resolve()
    drafts_path = Path(args.drafts_jsonl).resolve()
    fixed_terms = resolve_fixed_terms(args, manifest)

    result = apply_draft_to_catalog(
        compiler=compiler,
        catalog_dir=catalog_dir,
        manifest=manifest,
        ftl_path=ftl_path,
        drafts_jsonl_path=drafts_path,
        locale=args.locale,
        shard=args.shard,
        allow_drifted_source=args.allow_drifted_source,
        fixed_terms=fixed_terms,
    )
    print(json.dumps(result, indent=2, ensure_ascii=False))
    return 0


def parse_entry_spec(raw: str) -> tuple[str, Path, Path, Path, str | None]:
    parts = raw.split(":")
    if len(parts) not in (4, 5):
        raise ValueError(
            f"Invalid --entry {raw!r}; expected catalog_rel:manifest:ftl:drafts_jsonl[:shard]"
        )
    cat_rel = parts[0].strip("/")
    manifest = Path(parts[1]).resolve()
    ftl = Path(parts[2]).resolve()
    drafts = Path(parts[3]).resolve()
    shard = parts[4] if len(parts) == 5 and parts[4] else None
    return cat_rel, manifest, ftl, drafts, shard


def git_changed_paths(repo: Path) -> list[str]:
    proc = run_cmd(["git", "status", "--porcelain", "-uall"], cwd=repo)
    paths: list[str] = []
    for line in proc.stdout.splitlines():
        if not line:
            continue
        # Format: XY <path> or XY <old> -> <new>
        rel = line[3:]
        if " -> " in rel:
            old, new = rel.split(" -> ", 1)
            paths.append(old)
            paths.append(new)
        else:
            paths.append(rel)
    return sorted(set(paths))


def cmd_land(args: argparse.Namespace) -> int:
    repo = Path(args.repo).resolve()
    compiler = Path(args.compiler).resolve()
    entries = [parse_entry_spec(raw) for raw in args.entry]
    if not entries:
        raise ValueError("At least one --entry is required")

    allowed_paths: set[str] = set(args.allowed_extra_path or [])
    for cat_rel, _, _, _, shard in entries:
        allowed_paths.add(f"{cat_rel}/source-lock.json")
        if shard:
            allowed_paths.add(f"{cat_rel}/translations/{args.locale}/{shard}.ftl")
        else:
            allowed_paths.add(f"{cat_rel}/translations/{args.locale}.ftl")

    for attempt in range(1, args.max_retries + 1):
        run_cmd(["git", "fetch", "origin", args.branch], cwd=repo)
        run_cmd(["git", "checkout", "--force", "--detach", f"origin/{args.branch}"], cwd=repo)
        run_cmd(["git", "clean", "-fd"], cwd=repo)

        base_sha = run_cmd(["git", "rev-parse", "HEAD"], cwd=repo).stdout.strip()
        results = []
        for cat_rel, manifest_path, ftl_path, drafts_path, shard in entries:
            cat_dir = repo / cat_rel
            manifest = load_manifest(manifest_path)
            fixed_terms = resolve_fixed_terms(args, manifest)
            res = apply_draft_to_catalog(
                compiler=compiler,
                catalog_dir=cat_dir,
                manifest=manifest,
                ftl_path=ftl_path,
                drafts_jsonl_path=drafts_path,
                locale=args.locale,
                shard=shard,
                allow_drifted_source=args.allow_drifted_source,
                fixed_terms=fixed_terms,
            )
            run_cmd([str(compiler), "compile", str(cat_dir), "--channel", "production"])
            dev_cmd = [
                str(compiler),
                "compile",
                str(cat_dir),
                "--channel",
                "development",
                "--locale",
                args.locale,
            ]
            if args.fallback_stale:
                dev_cmd.append("--fallback-stale")
            run_cmd(dev_cmd)
            results.append(res)

        if args.regen_cmd:
            env = os.environ.copy()
            env["I18N_COMPILER"] = str(compiler)
            env["I18N_CATALOG"] = str(compiler)
            proc = subprocess.run(
                args.regen_cmd,
                cwd=repo,
                shell=True,
                env=env,
                text=True,
                capture_output=True,
            )
            if proc.returncode != 0:
                raise RuntimeError(
                    f"--regen-cmd failed (exit {proc.returncode}): {args.regen_cmd}\n"
                    f"STDOUT:\n{proc.stdout}\nSTDERR:\n{proc.stderr}"
                )

        changed = git_changed_paths(repo)
        unexpected = [p for p in changed if p not in allowed_paths]
        if unexpected:
            raise RuntimeError(
                "Refusing to commit unexpected file changes (production outputs or "
                f"English files must stay byte-identical): {unexpected}"
            )

        if not changed:
            print(
                json.dumps(
                    {
                        "status": "no-op",
                        "base": base_sha,
                        "entries": results,
                    },
                    indent=2,
                )
            )
            return 0

        if args.gate_cmd:
            env = os.environ.copy()
            env["I18N_COMPILER"] = str(compiler)
            env["I18N_CATALOG"] = str(compiler)
            proc = subprocess.run(
                args.gate_cmd,
                cwd=repo,
                shell=True,
                env=env,
                text=True,
                capture_output=True,
            )
            if proc.returncode != 0:
                raise RuntimeError(
                    f"--gate-cmd failed (exit {proc.returncode}): {args.gate_cmd}\n"
                    f"STDOUT:\n{proc.stdout}\nSTDERR:\n{proc.stderr}"
                )

        run_cmd(["git", "add", "--", *changed], cwd=repo)
        run_cmd(["git", "commit", "-m", args.message], cwd=repo)
        commit_sha = run_cmd(["git", "rev-parse", "HEAD"], cwd=repo).stdout.strip()

        if args.no_push:
            print(
                json.dumps(
                    {
                        "status": "committed-no-push",
                        "base": base_sha,
                        "commit": commit_sha,
                        "changed_files": changed,
                        "attempt": attempt,
                        "entries": results,
                    },
                    indent=2,
                )
            )
            return 0

        push_proc = run_cmd(
            ["git", "push", "origin", f"HEAD:refs/heads/{args.branch}"],
            cwd=repo,
            check=False,
        )
        if push_proc.returncode == 0:
            print(
                json.dumps(
                    {
                        "status": "pushed",
                        "base": base_sha,
                        "commit": commit_sha,
                        "changed_files": changed,
                        "attempt": attempt,
                        "entries": results,
                    },
                    indent=2,
                )
            )
            return 0

        stderr_lower = push_proc.stderr.lower()
        is_race = any(
            tok in stderr_lower
            for tok in ("non-fast-forward", "fetch first", "[rejected]", "cannot lock ref")
        )
        if not is_race or attempt == args.max_retries:
            raise RuntimeError(
                f"git push failed on attempt {attempt}/{args.max_retries}:\n"
                f"{push_proc.stderr}"
            )
        delay = min(1.5 * attempt, 8.0) + random.uniform(0.1, 1.0)
        print(
            f"Push race on attempt {attempt}/{args.max_retries}; retrying in {delay:.1f}s...",
            file=sys.stderr,
        )
        time.sleep(delay)

    return 1


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)

    def add_common(p: argparse.ArgumentParser) -> None:
        p.add_argument("--compiler", required=True, help="Path to i18n-catalog binary")
        p.add_argument("--locale", default="es-419", help="Draft locale tag")
        p.add_argument(
            "--fixed-terms",
            default=",".join(DEFAULT_FIXED_TERMS),
            help="Comma-separated fixed brand terms checked for verbatim preservation",
        )
        p.add_argument(
            "--no-fixed-terms",
            action="store_true",
            help="Disable fixed-terms check",
        )
        p.add_argument(
            "--allow-drifted-source",
            action="store_true",
            help="Leave IDs whose English changed since freeze unacknowledged (stale)",
        )

    p_verify = sub.add_parser("verify", help="Verify draft FTL and drafts.jsonl in scratch")
    add_common(p_verify)
    p_verify.add_argument("--catalog", required=True)
    p_verify.add_argument("--manifest", required=True)
    p_verify.add_argument("--ftl", required=True)
    p_verify.add_argument("--drafts-jsonl", required=True)
    p_verify.add_argument("--shard", default=None)
    p_verify.add_argument("--check-targets", default="")
    p_verify.add_argument("--fallback-stale", action="store_true")
    p_verify.set_defaults(func=cmd_verify)

    p_apply = sub.add_parser("apply", help="Apply draft FTL and scoped lock acks to a catalog")
    add_common(p_apply)
    p_apply.add_argument("--catalog", required=True)
    p_apply.add_argument("--manifest", required=True)
    p_apply.add_argument("--ftl", required=True)
    p_apply.add_argument("--drafts-jsonl", required=True)
    p_apply.add_argument("--shard", default=None)
    p_apply.set_defaults(func=cmd_apply)

    p_land = sub.add_parser(
        "land",
        help="Fetch, cleanly re-apply, gate, commit, and push with automatic retry on race",
    )
    add_common(p_land)
    p_land.add_argument("--repo", required=True, help="Isolated product-repo clone")
    p_land.add_argument("--branch", default="main")
    p_land.add_argument(
        "--entry",
        action="append",
        required=True,
        help="catalog_rel:manifest:ftl:drafts_jsonl[:shard] (repeatable)",
    )
    p_land.add_argument("--regen-cmd", default=None)
    p_land.add_argument("--gate-cmd", default=None)
    p_land.add_argument("--allowed-extra-path", action="append", default=[])
    p_land.add_argument("--fallback-stale", action="store_true")
    p_land.add_argument("--message", required=True)
    p_land.add_argument("--max-retries", type=int, default=10)
    p_land.add_argument("--no-push", action="store_true")
    p_land.set_defaults(func=cmd_land)

    return parser


def main(argv: list[str] | None = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)
    try:
        return args.func(args)
    except Exception as err:
        print(f"ERROR: {err}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
