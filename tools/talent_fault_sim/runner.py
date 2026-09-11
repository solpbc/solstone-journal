# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
"""Run finite, replayable faults through the native talent worker."""

import argparse
import hashlib
import json
import os
import selectors
import shutil
import signal
import subprocess
import sys
import time
from pathlib import Path

FIELD_PATH = "reference/ami/ES2002a/transcript.txt"
DAY = "20260201"
SEGMENT = "090000_60"
STREAM = "field.audio"
GOOD = '{"body":"The team discussed a remote control."}'
OLD = "previous complete artifact\n"
COGITATE_USAGE = {
    "input_tokens": 11,
    "output_tokens": 7,
    "model_version": "fault-fixture",
}


def digest(data):
    return hashlib.sha256(data).hexdigest()


def write_json(path, value):
    path.write_text(json.dumps(value, indent=2) + "\n")


def generated(valid=True):
    return {
        "schema": "solstone-generate-response-v2",
        "id": None,
        "outcome": "generated",
        "text": GOOD if valid else '{"body":""}',
        "model": "fault-fixture",
        "usage": {},
        "finish_reason": "stop",
        "thinking": None,
        "schema_validation": {
            "valid": valid,
            "errors": [] if valid else ["empty body"],
        },
        "input_budget": None,
        "request_budget": None,
        "inference": None,
    }


def refused(code):
    return {
        "schema": "solstone-generate-response-v2",
        "id": None,
        "outcome": "refused",
        "reason": "unknown",
        "reason_code": code,
        "retryable": False,
        "blocking": True,
        "reset_at_ms": None,
        "provider": "fixture",
        "detail": "injected " + code,
    }


def scenarios():
    finish = [
        {
            "event": "finish",
            "terminal": True,
            "result": GOOD,
            "usage": COGITATE_USAGE,
            "model": "fault-fixture",
        }
    ]
    refusal = [
        {
            "event": "error",
            "terminal": True,
            "error": "injected provider refusal",
            "usage": COGITATE_USAGE,
            "provider_failure": {
                "reason_code": "provider_request_rejected",
                "retryable": False,
                "blocking": True,
            },
        }
    ]
    return [
        ("generate_clean", "generate", [generated()], None, False),
        ("schema_recovers", "generate", [generated(False), generated()], None, False),
        (
            "length_recovers",
            "generate",
            [refused("incomplete_json_length"), generated()],
            None,
            False,
        ),
        (
            "schema_exhausted",
            "generate",
            [generated(False), generated(False)],
            "schema_validation_failed",
            False,
        ),
        ("cogitate_refused", "cogitate", [refusal], "provider_request_rejected", False),
        ("cogitate_clean", "cogitate", [finish], None, False),
        (
            "generate_write_failure",
            "generate",
            [generated()],
            "talent_stage_failed",
            True,
        ),
        ("cogitate_write_failure", "cogitate", [finish], "talent_stage_failed", True),
    ]


def run_bounded(argv, request, env, timeout=30, limits=(1_048_576, 65_536)):
    """Own the process group and bound each output stream independently."""
    if len(request) > 4096:
        raise ValueError("worker probe exceeds the 4096-byte request limit")
    start = time.monotonic()
    data = [bytearray(), bytearray()]
    failure = None
    with subprocess.Popen(
        argv,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        env=env,
        start_new_session=True,
    ) as child:
        try:
            # Requests are a small, fixed probe; model input travels over child pipes.
            child.stdin.write(request)
            child.stdin.close()
            with selectors.DefaultSelector() as selector:
                selector.register(child.stdout, selectors.EVENT_READ, 0)
                selector.register(child.stderr, selectors.EVENT_READ, 1)
                while selector.get_map():
                    remaining = timeout - (time.monotonic() - start)
                    if remaining <= 0:
                        failure = "worker_timeout"
                        break
                    for key, _ in selector.select(remaining):
                        chunk = os.read(key.fileobj.fileno(), 65536)
                        if not chunk:
                            selector.unregister(key.fileobj)
                            continue
                        index = key.data
                        room = limits[index] - len(data[index])
                        data[index].extend(chunk[:room])
                        if len(chunk) > room:
                            failure = "worker_output_limit"
                            break
                    if failure:
                        break
                if not failure:
                    try:
                        child.wait(
                            timeout=max(0.001, timeout - (time.monotonic() - start))
                        )
                    except subprocess.TimeoutExpired:
                        failure = "worker_timeout"
        except KeyboardInterrupt:
            failure = "worker_interrupted"
        except OSError as error:
            failure = "worker_io_error: " + str(error)
        finally:
            # Also removes descendants that closed their pipes before their parent exited.
            try:
                os.killpg(child.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            child.wait()
    return {
        "exit_code": child.returncode,
        "failure": failure,
        "stdout": bytes(data[0]),
        "stderr": bytes(data[1]),
    }


def verify(
    events,
    calls,
    engine,
    expected_calls,
    reason,
    output,
    expected_output,
    excerpt,
    journal,
    first_cause="schema_validation_failed",
):
    errors = []
    if len(calls) != expected_calls:
        errors.append("model_call_count")
    for call in calls:
        if engine == "generate":
            texts = [
                part.get("text", "")
                for part in call.get("contents", [])
                if isinstance(part, dict) and part.get("type") == "text"
            ]
            if not any(excerpt in text for text in texts):
                errors.append("assembled_fixture_missing")
        if engine == "cogitate" and call.get("journal_root") != str(journal):
            errors.append("wrong_child_journal")
    terminals = [
        e
        for e in events
        if e.get("event") in ("finish", "error")
        and (e.get("event") == "finish" or e.get("terminal", True))
    ]
    if len(terminals) != 1:
        errors.append("terminal_count")
    elif reason:
        if (
            terminals[0].get("event") != "error"
            or terminals[0].get("reason_code") != reason
        ):
            errors.append("terminal_cause")
    elif terminals[0].get("event") != "finish" or terminals[0].get("output") != GOOD:
        errors.append("terminal_result")
    if output != expected_output:
        errors.append("artifact_bytes")
    if engine == "cogitate" and len(terminals) == 1:
        if terminals[0].get("usage") != COGITATE_USAGE:
            errors.append("terminal_usage")
    if engine == "generate":
        attempts = [e for e in events if e.get("event") == "generate_attempt"]
        if len(attempts) != expected_calls:
            errors.append("attempt_evidence_count")
        for ordinal, attempt in enumerate(attempts):
            retry = ordinal < expected_calls - 1
            cause = (
                first_cause
                if retry
                else (
                    reason
                    if reason in ("schema_validation_failed", "incomplete_json_length")
                    else None
                )
            )
            if attempt.get("ordinal") != ordinal or attempt.get("batch") is not None:
                errors.append("attempt_identity")
            if (
                attempt.get("terminal") is not False
                or attempt.get("retry") is not retry
            ):
                errors.append("attempt_retry_decision")
            if attempt.get("cause") != cause:
                errors.append(
                    "intermediate_failure_missing" if retry else "attempt_final_cause"
                )
            status = (
                "retry_eligible" if retry else ("exhausted" if cause else "success")
            )
            if attempt.get("status") != status:
                errors.append("attempt_status")
    return errors


def verify_publication(before, after, failed):
    if failed and before != after:
        return ["failed_publication_replaced_artifact"]
    if not failed and before == after:
        return ["publication_not_replaced"]
    return []


def absolute(path):
    path = Path(path)
    if not path.is_absolute():
        raise ValueError("paths must be absolute")
    return path.resolve(strict=True)


def validate_output(output, inputs):
    if not output.is_absolute():
        raise ValueError("output must be absolute")
    for source in inputs:
        directory = source if source.is_dir() else source.parent
        result = subprocess.run(
            ["git", "rev-parse", "--show-toplevel"],
            cwd=directory,
            capture_output=True,
            text=True,
            check=False,
        )
        root = Path(result.stdout.strip()) if result.returncode == 0 else directory
        if output.resolve().is_relative_to(root.resolve()):
            raise ValueError(
                "output must be outside all input repositories and payload directories"
            )


def run(args):
    if os.name != "posix":
        raise ValueError("the process-group harness currently requires Unix")
    binary, payload, field = map(absolute, (args.binary, args.payload, args.field_root))
    output = Path(args.output)
    validate_output(output, (binary, payload, field))
    revision = subprocess.check_output(
        [
            "git",
            "rev-parse",
            "--verify",
            "--end-of-options",
            args.field_ref + "^{commit}",
        ],
        cwd=field,
        text=True,
    ).strip()
    fixture = subprocess.check_output(
        ["git", "show", revision + ":" + FIELD_PATH], cwd=field
    )
    excerpt = fixture.decode()[:1800]
    # Keep a single line so the assembled-input assertion does not depend on JSON escaping.
    excerpt = " ".join(excerpt.split())
    output.mkdir(mode=0o700, parents=False, exist_ok=False)
    prefix = output / "payload"
    (prefix / "bin").mkdir(parents=True)
    shutil.copytree(payload, prefix / "share")
    worker = prefix / "bin/talent-worker"
    shutil.copy2(binary, worker)
    stub_source = Path(__file__).with_name("stub.py")
    stub = prefix / "bin/solstone-core"
    stub.write_text(
        "#!" + sys.executable + "\n" + stub_source.read_text().split("\n", 1)[1]
    )
    stub.chmod(0o700)
    (output / "field-source.txt").write_bytes(fixture)
    attribution = subprocess.check_output(
        ["git", "show", revision + ":ATTRIBUTION.md"], cwd=field
    )
    (output / "ATTRIBUTION.md").write_bytes(attribution)
    copied_payload = prefix / "share"
    files = sorted(p for p in copied_payload.rglob("*") if p.is_file())
    provenance = {
        "field_revision": revision,
        "field_path": FIELD_PATH,
        "field_sha256": digest(fixture),
        "binary_sha256": digest(worker.read_bytes()),
        "binary_source": str(binary),
        "runner_sha256": digest(Path(__file__).read_bytes()),
        "stub_sha256": digest(stub.read_bytes()),
        "payload_files": {
            str(p.relative_to(copied_payload)): digest(p.read_bytes()) for p in files
        },
        "boundary": "native talent runtime; scripted model subprocess; no scheduler or model-quality score",
        "fixture_conversion": "first 1800 transcript characters, whitespace normalized; media transcription skipped",
    }
    write_json(output / "provenance.json", provenance)
    results = []
    cases = scenarios()
    if not cases:
        raise ValueError("no scenarios selected")
    for name, engine, responses, reason, write_failure in cases:
        case = output / name
        case.mkdir()
        journal = case / "journal"
        source = journal / "chronicle" / DAY / STREAM / SEGMENT
        source.mkdir(parents=True)
        (source / "audio.jsonl").write_text(
            json.dumps({"start": "00:00:00", "text": excerpt}) + "\n"
        )
        (journal / "config").mkdir()
        write_json(
            journal / "config/journal.json",
            {"providers": {"active": {"provider": "test", "model": "fault-fixture"}}},
        )
        talent = "fault-probe-" + engine
        metadata = {
            "type": engine,
            "output": "json",
            "load": {"transcripts": True, "percepts": False, "talents": False},
        }
        if engine == "generate":
            metadata["schema"] = talent + ".schema.json"
            write_json(
                prefix / "share/solstone/talent" / (talent + ".schema.json"),
                {
                    "type": "object",
                    "required": ["body"],
                    "properties": {"body": {"type": "string", "minLength": 1}},
                },
            )
        (prefix / "share/solstone/talent" / (talent + ".md")).write_text(
            json.dumps(metadata, indent=2) + "\n\nSummarize the meeting.\n"
        )
        destination = source / "talents" / (talent + ".json")
        destination.parent.mkdir()
        read_failure = name == "generate_write_failure"
        if write_failure and not read_failure:
            destination.mkdir()
            (destination / "sentinel").write_text(OLD)
        else:
            destination.write_text(OLD)
        if read_failure:
            destination.chmod(0)
            try:
                destination.read_bytes()
            except PermissionError:
                pass
            else:
                destination.chmod(0o600)
                raise ValueError(
                    "the unreadable-artifact fixture requires an account without read-permission bypass"
                )
        before_stat = destination.stat()
        before_identity = (before_stat.st_dev, before_stat.st_ino)
        request = {
            "name": talent,
            "day": DAY,
            "segment": SEGMENT,
            "stream": STREAM,
            "use_id": "1789000000000",
            "output_path": str(destination),
            "prompt": "Summarize the meeting.",
        }
        write_json(case / "request.json", request)
        write_json(case / "script.json", {"engine": engine, "responses": responses})
        env = {
            "PATH": "/usr/bin:/bin",
            "HOME": str(case),
            "SOLSTONE_JOURNAL": str(journal),
            "TALENT_FAULT_CASE": str(case),
        }
        execution = run_bounded(
            [str(worker), "__talent-worker"],
            (json.dumps(request) + "\n").encode(),
            env,
            args.timeout,
        )
        (case / "events.jsonl").write_bytes(execution.pop("stdout"))
        (case / "stderr.log").write_bytes(execution.pop("stderr"))
        observed = None
        after_identity = None
        try:
            events = [
                json.loads(line)
                for line in (case / "events.jsonl").read_text().splitlines()
            ]
            calls = (
                [
                    json.loads(line)
                    for line in (case / "calls.jsonl").read_text().splitlines()
                ]
                if (case / "calls.jsonl").exists()
                else []
            )
            if read_failure:
                destination.chmod(0o600)
            observed = (
                destination / "sentinel"
                if write_failure and not read_failure
                else destination
            ).read_bytes()
            errors = verify(
                events,
                calls,
                engine,
                len(responses),
                reason,
                observed,
                (OLD if reason else GOOD).encode(),
                excerpt,
                journal,
                first_cause="incomplete_json_length"
                if name == "length_recovers"
                else "schema_validation_failed",
            )
            after_stat = destination.stat()
            after_identity = (after_stat.st_dev, after_stat.st_ino)
            errors.extend(
                verify_publication(
                    before_identity,
                    after_identity,
                    reason is not None,
                )
            )
        except (ValueError, OSError) as error:
            errors = ["unreadable_evidence: " + str(error)]
        if execution["exit_code"] != 0 or execution["failure"]:
            errors.append("worker_execution")
        result = {
            "scenario": name,
            "outcome": "FAIL" if errors else "PASS",
            "errors": errors,
            "artifact_sha256": digest(observed) if observed is not None else None,
            "artifact_identity_before": before_identity,
            "artifact_identity_after": after_identity,
            **execution,
        }
        write_json(case / "result.json", result)
        results.append(result)
        if execution["failure"] == "worker_interrupted":
            break
    write_json(
        output / "probe-overlay.json",
        {
            str(p.relative_to(prefix / "share")): digest(p.read_bytes())
            for p in sorted((prefix / "share/solstone/talent").glob("fault-probe-*"))
        },
    )
    write_json(
        output / "summary.json",
        {
            "outcome": "PASS"
            if len(results) == len(cases)
            and all(r["outcome"] == "PASS" for r in results)
            else "FAIL",
            "expected_scenarios": [case[0] for case in cases],
            "scenarios": results,
        },
    )
    print(json.dumps({"output": str(output), "scenarios": results}, indent=2))
    if any(r["failure"] == "worker_interrupted" for r in results):
        return 130
    return int(any(r["outcome"] != "PASS" for r in results))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("binary", "payload", "field-root", "output"):
        parser.add_argument("--" + name, required=True)
    parser.add_argument("--field-ref", default="HEAD")
    parser.add_argument("--timeout", type=float, default=30)
    args = parser.parse_args()
    if not 0 < args.timeout <= 120:
        parser.error("timeout must be positive and at most 120 seconds")

    def interrupt(_signal, _frame):
        raise KeyboardInterrupt

    previous = signal.signal(signal.SIGTERM, interrupt)
    try:
        return run(args)
    except KeyboardInterrupt:
        return 130
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        print("talent fault runner: " + str(error), file=sys.stderr)
        return 2
    finally:
        signal.signal(signal.SIGTERM, previous)
