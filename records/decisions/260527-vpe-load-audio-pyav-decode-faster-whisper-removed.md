# 260527 load_audio PyAV decode and faster-whisper removal

## Summary

`load_audio()` now decodes all supported audio formats through PyAV. Non-M4A files decode the first audio stream into mono float32 at the requested sample rate, while the existing M4A branch remains the multi-stream mixer for sck-cli files. This removes the runtime dependency on `faster_whisper.audio.decode_audio`; the optional Whisper extra now describes only the Whisper transcription backend.

## Files Touched

| Path | Change |
|---|---|
| `solstone/observe/utils.py` | Replace non-M4A `faster_whisper.audio.decode_audio` path with PyAV decode/resample, direct no-stream/no-data failures, and wrapped PyAV decode failures. |
| `solstone/think/features.py` | Narrow `feature:whisper` summary to the optional Whisper backend. |
| `tests/test_transcribe.py` | Extend `TestLoadAudio` with import-probe, per-format decode, resampling sanity, failure wrapping, and short-clip coverage. |
| `tests/test_silero_vad_vendored.py` | Add `pytest.importorskip("faster_whisper.vad")` only to the upstream parity test. |
| `tests/test_features.py` | Make Whisper availability assertion reflect whether the optional extra is installed. |
| `records/decisions/260527-vpe-load-audio-pyav-decode-faster-whisper-removed.md` | Record decisions, acceptance results, and bug reproduction. |

## Decisions

### PyAV Decode

Decision: use PyAV for non-M4A decode instead of promoting `faster-whisper` to a base dependency or adding a `soundfile` fallback.

Why: PyAV is already a base dependency, the M4A branch already proves the local resampler pattern, and this is the smallest diff that removes the optional-extra failure mode without adding another decode stack.

### `feature:whisper` Severity

Decision: keep `feature:whisper` advisory.

Why: missing `faster-whisper` no longer breaks `load_audio()` or VAD. It only disables the Whisper transcription backend, so the warning should remain advisory and read narrowly: `Whisper transcription backend (optional) not installed`.

### Decode Failure Type

Decision: wrap PyAV decode failures in `RuntimeError`, not `ValueError`.

Why: a corrupt or undecodable media file is a runtime decode failure, not invalid caller input. Actual PyAV failures keep the PyAV `FFmpegError` subclass as the cause for traceback fidelity; internal no-stream/no-data validations raise the same user-facing `RuntimeError` shape directly.

### Test Placement

Decision: keep new `load_audio()` tests in `tests/test_transcribe.py::TestLoadAudio`.

Why: that class already owns load-audio behavior for FLAC, M4A, and multi-track M4A. The subprocess import probe follows the `tests/test_silero_vad_vendored.py` idiom but lives with the rest of the load-audio coverage.

## AC Results

| AC | Result |
|---|---|
| AC1 | Non-M4A `load_audio()` no longer imports `faster_whisper`; see `solstone/observe/utils.py`. |
| AC2 | Non-M4A decode uses `av.open`, first audio stream, and `AudioResampler(format="flt", layout="mono", rate=sample_rate)`. |
| AC3 | M4A multi-stream branch remains unchanged and covered by existing M4A tests. |
| AC4 | `feature:whisper` summary is now `Whisper transcription backend (optional)`. |
| AC5 | No stale `"Whisper speech-to-text transcription"` hits remain in `tests/` or `solstone/`. |
| AC6 | Import probe in `tests/test_transcribe.py::TestLoadAudio::test_load_audio_does_not_pull_faster_whisper` asserts no `faster_whisper` module load. |
| AC7 | Per-format non-M4A decode tests cover supported libsndfile encodes, with explicit skips for host encoder gaps. |
| AC8 | Source grep confirms `solstone/observe/utils.py` has zero `faster_whisper` hits; full `solstone/` text grep has the expected 4 remaining references outside cache. `make ci` passed with `faster-whisper` installed, and the no-extra gate passed after uninstalling `faster-whisper`; the package still raised `ModuleNotFoundError` after that no-extra run. |
| AC9 | Decode failures raise `RuntimeError` with file path and suffix in the message; zero-frame `.flac` decode is covered as an internal no-data failure. |
| AC9a | Actual PyAV failures keep a PyAV `av.error.FFmpegError` subclass as `__cause__`; PyAV 16.1.0 does not expose `av.AVError`. |
| AC10 | This decision record captures the dependency decision, tests, doctor output, and follow-up. |

## Verification

Focused tests:

- `make test-only TEST=tests/test_transcribe.py::TestLoadAudio` -> 12 passed, 1 skipped.
- `make test-only TEST=tests/test_silero_vad_vendored.py` -> 3 passed.
- `make test-only TEST=tests/test_doctor_features.py` -> 8 passed.
- `make test-only TEST=tests/test_features.py` -> 15 passed.
- `.venv/bin/ruff check solstone/observe/utils.py solstone/think/features.py tests/test_transcribe.py tests/test_silero_vad_vendored.py` -> all checks passed.

CI gates:

- With `faster-whisper` installed: `make ci` -> 6697 passed, 15 skipped; link QR test -> 1 passed.
- Without `faster-whisper`: `make ci` -> 6696 passed, 16 skipped; link QR test -> 1 passed. The no-extra run used Make's install sentinel as current and a temporary no-op `uv sync` shim so the pre-existing `tests/test_doctor.py::TestMakefileIntegration::test_dry_run_install_does_not_run_doctor` dry-run install probe could not mutate the venv while exercising the CI target.
- Affected no-extra skip details: `tests/test_silero_vad_vendored.py::test_vendored_get_speech_timestamps_matches_upstream` skipped because `faster_whisper.vad` was absent; `tests/test_transcribe.py::TestLoadAudio::test_load_audio_decodes_ext[.opus]` skipped because libsndfile could not encode `.opus` on this host.

Required command outputs:

```text
$ grep -rn "faster_whisper" solstone/observe/utils.py
<no output>
```

```text
$ .venv/bin/sol doctor --feature whisper 2>&1 | head -20
doctor: 1 checks, 0 failed, 0 warnings, 0 skipped
```

The exact command emits no warn line in this environment because `faster-whisper` is still installed. The missing-extra check path now produces:

```text
warn feature:whisper: Whisper transcription backend (optional) not installed
fix: pip install 'solstone[whisper]'
```

## Bug Reproduction Artifact

Before this commit, `git show HEAD:solstone/observe/utils.py | sed -n '70,80p'` showed the non-M4A branch hard-importing `from faster_whisper.audio import decode_audio`. Calling `load_audio()` on any non-M4A file without `[whisper]` installed would raise `ModuleNotFoundError: No module named 'faster_whisper'` from inside that conditional, and the broad `except` in `solstone/observe/transcribe/main.py:700` turned the backend-missing failure into a failed transcription path instead of an actionable optional-extra boundary.

The failure shape captured before this change was:

```text
Traceback (most recent call last):
  File "<stdin>", line 16, in <module>
  File "/home/jer/.local/share/hopper/lodes/k6rga6kh/worktree/solstone/observe/utils.py", line 77, in load_audio
    from faster_whisper.audio import decode_audio
  File "<stdin>", line 11, in blocked_import
ModuleNotFoundError: No module named 'faster_whisper'
```

## Open Follow-Up

The `output-rate-zero` health metric should be revisited after this ships. With decode no longer coupled to the optional Whisper backend, zero-output diagnosis can distinguish media decode failures from backend availability more cleanly.
