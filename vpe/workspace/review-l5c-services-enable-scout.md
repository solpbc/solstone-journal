# L5C Review Gate: `sol services enable scout` + storage module

Parent plan: vpe/workspace/plan-arc-a-back-channel-rederivation.md (Wave 4 — L5C)
Worker contract: services.solstone.app (shipped 2026-05-24, source not vendored)

## §1 — Atomicity of `write_journal_config`

Decision: choose option (A). Upgrade `solstone/think/journal_config.py:28-36` so `write_journal_config()` writes to a temporary sibling file, chmods it private, then commits with `os.replace`. This mirrors `solstone/think/utils.py:703-709` while keeping the public journal-config helper as the storage call site. Replace only the `init_finalize` duplicate write in `solstone/convey/root.py:370-375` with `write_journal_config(config)`. Do not touch `_save_config_section` at `solstone/convey/root.py:86-96`.

Trade-off accepted — the small helper change benefits every existing caller of `write_journal_config`, including the new storage module, and makes AC #11 meetable. The `init_finalize` site becomes a one-line helper call with no intentional behavior change.

## §2 — Lock file path & lifecycle

Decision: use `<journal>/config/.journal.json.lock`. The lock sits next to `journal.json`, is dot-prefixed, and is created on first lock acquisition. Opening with `mode="w"` is acceptable because content is irrelevant; the lock is the file descriptor plus `fcntl.flock(..., LOCK_EX)`, matching `solstone/think/skills.py:152-185`. The storage module should chmod it to `0o600`.

Trade-off accepted — leaving the lock file behind is conventional `fcntl` practice. The storage module never removes it; removing a lock path during active or future coordination is more surprising than retaining an empty private sentinel.

## §3 — COMMANDS group placement

Decision: add `"services": "solstone.think.services"` to `COMMANDS` in `solstone/think/sol_cli.py:41-82`, and add a new peer group named `"Services"` in `GROUPS` at `solstone/think/sol_cli.py:101-146`. Keep the existing singular `"Service"` group for lifecycle commands around `solstone.think.service`; preserve its relationship to `start`, `up`, and `down` aliases. The new `"Services"` group contains only `services` for L5C. Later L7 verbs such as `disable` are argparse subcommands under the one `services` command.

Trade-off accepted — adjacent singular/plural groups are mildly ambiguous in `sol -h`, but renaming the existing service lifecycle surface is out of scope. The new command help text must make clear that `services` manages optional hosted services, while `service` manages the local background supervisor.

## §4 — Wall-clock budget & rationale

Decision: expose `--wait <seconds>` on `sol services enable scout`, default `900`, clamped to `[60, 3600]` by the argparse `type=` converter. Rationale: owner-patience budget for the in-browser consent flow. Do not describe it as matching `HANDOFF_TTL_MS`; the worker constant is five minutes and governs the post-consent handoff row, not the whole human flow.

Per-poll request timeout: `35s`, giving five seconds of local slack over the worker’s documented `HANDOFF_POLL_BUDGET_MS = 30s`. The CLI should keep polling until the wall-clock budget expires, a terminal worker result arrives, or a local boundary failure occurs.

## §5 — Headless detection predicate

Decision: place `_is_headless() -> bool` in `solstone/think/services/cli.py`, and call it through the function reference so tests can monkeypatch it. The predicate returns true if `SSH_TTY` is non-empty, or if Linux lacks both `DISPLAY` and `WAYLAND_DISPLAY`. Only after those cheap checks pass should the CLI call `webbrowser.open()`; if it returns `False`, treat the run as headless and print the manual URL path.

Trade-off accepted — the worker flow is still browser-first for normal desktops, but SSH and display-less Linux avoid a noisy launch attempt. Including `WAYLAND_DISPLAY` avoids false positives for Hyprland, Sway, and other Wayland sessions.

## §6 — Output wording

Decision: terse, product-safe CLI copy. Stdout strings: `Opening services.solstone.app to enable scout...`; `Waiting for you to finish in the browser (up to 15 minutes)...`; `Scout enabled.`

Error tokens:

- `consent_link_expired`: Browser approval expired. Rerun the command to start a fresh enable flow.
- `consent_timeout`: The browser flow exceeded the wait budget. Rerun with a longer `--wait` if needed.
- `portal_unreachable`: services.solstone.app could not be reached. Check network and try again.
- `tls_verification_failed`: TLS verification failed while contacting services.solstone.app. Check system time, certificates, or network interception.
- `nonce_invalid`: The enable request token was rejected. Rerun the command to create a fresh token.
- `unexpected_payload`: The services response shape was unexpected. Update solstone and try again.
- `write_failed`: Scout was approved, but journal config was not saved. Check `<journal>/config` permissions and retry.
- `already_enabled`: Scout is already enabled. No change needed.
- `manual_key_present`: A manual Gemini key is already present in journal config. Use --force to overwrite with a portal-provisioned key.
- `headless_no_browser`: No browser is available from this shell. Rerun from a desktop session.
- `journal_not_initialized`: Journal config file is missing. Run `sol setup`, then retry.
- `unknown_service`: Unknown service name. Use `sol services enable scout`.

| Token | Trigger |
|-------|---------|
| `consent_link_expired` | worker returned 410 gone |
| `consent_timeout` | wall-clock budget elapsed with only 204s |
| `portal_unreachable` | urllib `URLError` (network failure) |
| `tls_verification_failed` | urllib `ssl.SSLError` / cert validation failure |
| `nonce_invalid` | worker returned 400 (malformed nonce) |
| `unexpected_payload` | 200 but JSON missing required fields or wrong shape, or non-2xx other than 204/410/400 |
| `write_failed` | `provision_scout_handoff` raised after a valid 200 (filesystem / permission error) |
| `already_enabled` | `is_scout_enabled()` returns True before browser opens |
| `manual_key_present` | `is_manual_key_present()` returns True before browser opens |
| `headless_no_browser` | `_is_headless()` returns True before browser opens |
| `journal_not_initialized` | journal config file does not exist at flow start |
| `unknown_service` | argparse-level: `sol services enable <something-other-than-scout>` |

Exit codes: `already_enabled` and `manual_key_present` exit 0; `headless_no_browser` and `unknown_service` exit 2; all other eight tokens exit 1. Grep-tested the 15 CLI strings against `r"sign(?:ed)?\s+in|signing\s+in|log(?:ged)?\s+in|your\s+account|account\s+settings|linked|authenticate"`: zero matches.

## §7 — File boundaries within `solstone/think/services/`

Decision: split the package by runtime concern. `solstone/think/services/__init__.py` re-exports `main` from `cli.py`; `__main__.py` supports `python -m solstone.think.services`; `cli.py` owns argparse shape, `main()`, `_is_headless`, error-token copy, URL opening, and HTTP polling; `scout.py` owns `provision_scout_handoff`, `is_scout_enabled`, `is_manual_key_present`, `scout_provenance`, and `JournalNotInitializedError`; `constants.py` owns `NONCE_ALPHABET`, `NONCE_LENGTH_CHARS`, and `NONCE_REGEX`, with a header docstring citing `enable-constants.js`.

Trade-off accepted — `scout.py` must stay importable without argparse, urllib, or browser side effects because L10 imports it as a storage module. All network and CLI behavior remains isolated in `cli.py`.

## §8 — Open questions for Jer

None — proceeding with recommendations above on Jer's approval.
