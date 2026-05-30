# Changelog

All notable changes to solstone (the Python package) will be documented in this file.

Format adapted from [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), aligned with `cmo/brand/changelog-voice.md`.

## [Unreleased]

### Changed
- cogitate is now baseline — the openhands-sdk runtime that powers sol's tool-calling agents ships in the wheel, so a fresh install with a hosted provider key runs cogitate immediately with no extra install step. wheel size grows by about 337 MB on install to carry openhands-sdk, litellm, and their transitive dependencies.
- minimum python is now 3.12 (was 3.11) — required by the openhands-sdk runtime that ships baseline. if you installed solstone with a 3.11 interpreter, reinstall under 3.12+ before updating.
- on linux, the default on-device transcription now works out of the box — its runtime ships with the install and `journal setup` downloads the model, so there's no separate extra to add. NVIDIA GPU owners can still opt into `solstone[parakeet-onnx-cuda]` for GPU acceleration, and `sol doctor` now reports whether the default transcription backend's runtime and model are ready.

### Removed
- the built-in `sol observer install` command is gone. linux and tmux observers now install from their own published packages: `pipx install solstone-linux` (or `solstone-tmux`), `solstone-linux install-service` (or `solstone-tmux install-service`), then `sol observer create <name>` mints a key you give the observer. the macOS observer continues to come from the signed app bundle at solstone.app/observers.
- the bundled per-provider install commands are gone — `sol call settings providers install` now accepts `local` only (cogitate runs out of the box for hosted providers with a key set), and `uninstall`/`disable`/`enable`/`validate-key` are removed entirely. local install continues to work via `sol call settings providers install local`.

## [0.4.5] - 2026-05-30

### Added
- you can now reach your journal from your phone or laptop even when they aren't on the same network as your home machine. setup lives at the connections page, which is now the single home for how you connect, your network access, and your paired devices. pairing shows a fresh code, lets you name each device, and lets you see and remove any device with one tap.
- the local model that runs entirely on your machine can now take in images as well as text, so the on-device option works on more of what's in your journal. nothing new leaves your machine.

### Changed
- the local model is now kept running for you in the background instead of starting up on demand, so it's ready the moment sol needs it. fresh installs launch it reliably the first time, and a model download now shows real progress instead of sitting at 0 percent through several gigabytes.
- diagnostics are now two clearer commands. `sol doctor` checks that the `sol` command itself is working, from anywhere. the new `journal doctor` checks the health of your journal and its background service. each asks only the question that fits where you run it, so neither raises a false alarm.
- the entities and devices views read more clearly: plain empty states when there's nothing yet, a retry when something fails to load, and detaching a facet now spells out what will happen and offers a one-tap way to undo it.

### Fixed
- your journal now shows when a moment has been transcribed but not yet thought through, instead of looking finished, and catches those moments up on its own. day-by-day status and the transcripts view reflect this honestly, so nothing sits half-processed without you knowing.

## [0.4.4] - 2026-05-27

### Changed
- when sol is catching up on a backlog, today's thinking no longer waits in line behind it. on a busy journal, or right after an install, the day's catch-up work and sol's thinking on fresh observations now run alongside each other, so new moments get attended to in seconds instead of waiting hours.

### Fixed
- transcripts come through on every audio format again. if you run a transcription backend other than whisper, some audio was making it into your journal but quietly producing no transcript. this resolves it, so the moments you spoke are written down the way you'd expect.
- upgrading from an older install no longer trips a setup check. if you first installed solstone a different way and then moved to the current method, `sol doctor` now adjusts the older `sol` and `journal` shortcuts for you instead of stopping. if you hit this, this resolves it.

## [0.4.3] - 2026-05-27

### Added
- a dedicated reader for facet newsletters at `/app/news/`. reverse-chrono index across all your facets, per-day detail with a copy button and a pdf download, and a sample newsletter so you can see the shape before your first one lands. newsletters sit next to reflections in the sidebar.

### Changed
- the participation tab on an activity now shows a structured list of people, grouped into attendees and mentioned, with a short note next to each name about how that person showed up in the activity. low-confidence entries appear muted with a "less certain" tag, and empty or unavailable states read in plain language instead of raw json.

### Fixed
- weekly reflection writes a full reflection to your journal again. on busy journals it was running out of room mid-gather and either saving nothing or saving only a short summary; both paths are resolved, and the reflections page renders again.
- attendee lists are stricter about who counts as an attendee. someone whose name only appeared in a transcript, without other corroboration, is now demoted to mentioned rather than surfaced as an attendee. reported by Ryan during a walkthrough.
- background work sol runs through google (morning briefings and other scheduled talents) no longer fails silently on a size limit. a request-budget calculation was landing one over the supported maximum, rejecting every call on the default settings; the calculation is corrected.
- sidebar labels in the expanded menu no longer truncate. entities, transcripts, and other longer labels show in full at narrower window widths. reported by Ryan.

## [0.4.2] - 2026-05-26

### fixed
- on a fresh install, `journal setup` could stop on a doctor check that flagged the `sol` command on your machine as out of place — even when it was the one journal had just put there. if you hit this setting up 0.4.1, this resolves it.

## [0.4.1] - 2026-05-26

### fixed
- some 0.4.0 installs didn't come back up after upgrading — sol wouldn't start, and journal commands timed out. this resolves it.

## [0.4.0] — 2026-05-26

### changed
- **service commands moved fully to `journal`.** Service commands (supervisor, cortex, heartbeat, setup, transcribe, services, etc.) are no longer surfaced under `sol` — they live exclusively under `journal`. Your existing solstone service migrates itself automatically on the next service restart; no action needed.
- `journal start` is now the canonical run command (replaces `journal supervisor` as the service-unit entry point — old units self-migrate).
- the `sol` CLI continues to be your day-to-day surface (chat, call, top, import, search across the journal).

### removed
- `sol <service-cmd>` paths typed by a human now redirect to `journal <cmd>` with a clear error and exit non-zero. Service units still pointing at the old paths self-migrate; nothing on disk breaks.

## [0.3.10] — 2026-05-26

### Added
- **journal CLI** — `solstone` now installs two CLI binaries: `sol` (the day-to-day surface for talking to your journal — chat, call, top, import, etc.) and `journal` (host operations — supervisor, setup, install-models, the daemons that tend your journal). `sol --help` shows both surfaces; `journal --help` shows just the host commands. Existing `sol <cmd>` invocations all keep working. Internal docs and scripts use `journal <cmd>` for host operations going forward.

## [0.3.9] - 2026-05-25

### Added
- solstone now has a "services" layer for the optional cloud-backed extras sol pbc offers alongside your local solstone. today that means solstone scout, the alpha-tester program that provisions a Google Gemini key for you and unlocks scout-only features. services are off by default; you turn them on from `services.solstone.app` or `sol services enable scout`, and solstone itself still runs entirely on your machine.
- you can now move days or whole journals between your own machines, and connect an observer on one machine to a journal on another, over a direct private link between your devices. `sol link join` pairs them; `sol transfer send --to <peer>` and `sol export --to <peer>` push from one to the other. revoking a paired device at the `/link` dashboard, with `sol observer revoke`, or with `sol call link unpair` cuts the connection at TLS the moment you revoke.
- a new "Local (on-device)" provider runs sol from a bundled `llama-server` on your own machine with a pinned Qwen model. zero-egress: when sol is set to local, it never falls through to a cloud provider.
- a new daily `journal/identity/health.md` surface tells you whether solstone is OK at a glance. sol reads its own signals, auto-recovers from things like stuck transcripts, and the home page and morning briefing now read its summary.

### Changed
- a few of the surfaces you touch most are now more direct. creating a facet lands you on a real detail page that confirms what you just made and offers next steps. clicking a "needs you" item on the home page opens a fresh chat with editable starter text already in the box (not as ghost placeholder), and sol knows which item you came from. each modality on segment-detail pages has its own "analyze now" button so you can re-run analysis on one part of a day without dropping to a terminal. the health, tokens, and service-log pages were rebuilt around a glance row that answers the first question (is solstone OK, is this costing too much, where did the pipeline fail) with the detail kept under progressive disclosure; service log lines now carry severity colors with screen-reader announcements on errors.

### Fixed
- segments that were already analyzed sometimes painted as still-pending on the day timeline; they now render correctly. audio playback on segment pages now shows the real duration and the right format, transcript lines no longer carry a doubled timestamp, the day view scrolls naturally on short windows, and a cold-load race on transcripts pages is resolved. internal stability improvements across providers install, background tasks, and the convey wizard.

## [0.3.8] - 2026-05-22

### Added
- you can now run sol's on-screen analysis fully on your own Mac. on Apple Silicon with at least 16 GB of memory, "MLX (Local, Apple Silicon)" appears in Settings under Providers; choose it once, sol downloads a local model in the background, and from then on the part of sol that makes sense of your screen runs on your machine, with nothing sent to a cloud provider. it's opt-in and covers vision today; the rest of sol stays on whichever provider you've chosen.
- you can now power sol with Anthropic or OpenAI without installing anything extra. choose the provider in Settings and solstone sets it up for you, with no separate command-line tool to install first. running on a hosted Google key needs no extra setup either.
- `sol setup --clean-uninstall` removes the pieces setup added to your machine, behind a confirmation that lists exactly what it will remove. your journal is never touched.

### Changed
- the timeline view is rebuilt. it opens straight into your real journal, fits any window from a phone-width pane to a wide desktop, and every entry shows which AI produced it with a link to that day. when sol finishes summarizing a new day, the view updates on its own.
- long todo lists now load fast and stay readable: solstone shows the most recent items first with a "show more" control for the rest, instead of rendering everything at once.
- api keys in setup and Settings are now masked as you type, and the validate button tells you plainly whether the key connected or failed.
- on Linux, bringing an observer online no longer needs git or a build step on the host; observers now install straight from their published packages.

### Fixed
- video and audio in your journal that showed "format not supported" now play. some entries with video or audio hit this; it's resolved.
- on installs from PyPI, sol's meeting-screen analysis was coming back as freeform notes instead of the structured entries it was built to produce. the missing piece now ships with the package, so meeting frames return to their intended shape.
- transcription that gave up on a long, dense stretch of audio now retries and recovers, so days that previously failed to transcribe complete. this also recovered a backlog of past days that had errored.
- pages that occasionally didn't finish loading now load cleanly.
- on some machines the background service could stop overnight and not restart; it now restarts as intended.
- pairing a phone by QR code now works in Safari on iPhone and Mac, where the code could previously render too small to scan.
- internal stability improvements, plus quieter local logs.

## [0.3.6] - 2026-05-18

### Changed
- solstone now uses each provider's current models, and the structured results sol asks providers for are validated the same way across every provider, including the backup one. this makes the AI features more reliable, with no change to how you use solstone.

### Fixed
- in some cases what sol wrote to your journal from a screen could be off. a frame with little on it could pick up names from your own contacts as if they'd been on screen, and an occasional runaway from the model could write a long block of repeated text into an entry. both are now caught before anything is written, so your journal reflects what was actually there.
- when sol fell back to a backup AI provider for a task that involved an image, the image could be left out of the request, so the result was a confident guess instead of something grounded in what was on screen. images are now sent correctly on every provider, and structured results from the backup provider are read correctly.
- upgrading solstone over an existing install now works cleanly. before, an upgrade could stop partway: setup could wrongly report that port 5015 was in use when it was solstone's own running service, and re-registering this machine's observer could fail as "already exists." if an upgrade left you stuck, this resolves it.

## [0.3.5] - 2026-05-17

### Added
- a new data-flow page explains, in plain language, what solstone sends to your chosen AI provider and what never leaves your machine. it covers local-first processing, that each task is scoped (not your whole journal), that the keys and the account are yours, and the things sol pbc is bound never to do with your data. it's linked from setup, the install guide, and the readme so you can read it before you connect a provider.
- the install guide now has a section on how to power sol: starting with a hosted provider key is recommended, running fully local is a real supported goal but not yet the default daily experience, with a heads-up on the hardware that takes. setup and the api-key settings now also tell you, per provider, to use a developer api key from the provider's console rather than your consumer chat login, with the right console link for each.

### Changed
- in-app support and feedback now point to support.solstone.app, and that's the default in support settings for new journals. if your settings still point at the old support address, nothing breaks and you can leave it as is. setup, the install guide, and the readme now also lead with following and tagging @solstone.app on Bluesky for feedback, then GitHub issues, then the support site.

## [0.3.4] - 2026-05-16

### Added
- a fresh journal now opens with a useful set of starred apps in the nav rail instead of a blank one. if you've already arranged your own starred apps, your choices are left exactly as they are.

### Changed
- the deprecated `precision` setting for parakeet transcription has been removed. `quantization` (auto, fp32, or int8) is the setting to use. if your journal config still carries the old `precision` line it's now simply ignored, with no change to how transcription runs.

### Fixed
- browsing back from the all-facets entity edit view now returns you to the entity you were looking at, in the same facet. before, back could land you on a different view.
- the bundled `transcripts read` documentation now shows the correct options. the previous example listed the wrong units for `--start` and `--length`, so following it as written would have failed.

## [0.3.3] - 2026-05-16

### Added
- a validate button now sits next to the gemini api key on the setup page, so you can confirm the key works before finalizing.

### Changed
- the setup page is reworked: cleaner typography, retention preferences as three explicit choices (always keep, keep for a set number of days, don't retain), enter-to-submit from any field, and your journal version and path surfaced up top.
- a fresh `sol setup` now installs the solstone bundle into all three coding-agent configs (claude, codex, gemini) at once, and lands the per-talent skill files in your journal so sol's sub-agents can find them.

### Fixed
- the setup page works end-to-end on a fresh install. earlier builds had a silent javascript bug that left the validate button, retention radios, and finalize submit unresponsive.
- on macos, your local timezone now resolves correctly on first setup. earlier installs could land in utc because the resolver missed where macos stores zone data.
