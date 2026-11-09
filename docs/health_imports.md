# body imports

This document is the current engineering boundary for Apple Health, Oura and Strava
body data entering the journal. The filename is retained for links; `health` in
owner-facing language means the journal system's health, while physiological
records are body data.

Apple Health and Oura land as ordinary journal segments. [`BODY_SEGMENTS.md`](BODY_SEGMENTS.md) is the contract: what a window holds, how an import is identified, and how readers use it.

## Strava workouts

Strava is the first body source an owner brings in from the import page (the **Strava** card), or with `solstone journal importer --source strava <download.zip | activities.csv>`.

- **What is read:** only `activities.csv` at the root of the download. Photos, messages, the people the owner follows and every other file in the download are never opened. Every filled-in column of each workout's row is kept: its type, name, start, elapsed and moving time, distance, elevation gain, heart rate, power, calories, the commute flag, and every other field Strava exported for it (description, private note, gear, weather, Strava's own scores and the rest), attributed to Strava. The name of the workout's GPS file is kept only as the entered-by-hand flag. English and German downloads are read; any other language is refused.
- **Where it lands:** each workout becomes pieces of up to five minutes under `chronicle/<day>/import.strava/`, from its start to its end, continuing past midnight. Each piece holds one `workout.json` with the workout's values.
- **Search, agents and thinking:** only a workout's first piece renders (as the workout's values, attributed to Strava), so a workout is one search result, on the day it started. Agents the owner lets read the journal find a workout in search like any other journal content. Its pieces aren't described or summarized by the journal's thinking and add no work to the owner's days, however old they are. Each workout filed under a facet is listed with the owner's activities there (see `docs/BODY_SEGMENTS.md` § How Strava relates). The people and gear named in the owner's own words on it are read once, by the model the journal thinks with, during an import while the journal may process (in the owner's processing window, if one is set), and otherwise on a later import.
- **Time zone:** Strava writes each workout's start in UTC, with no zone marker, so a workout is placed at that instant. The journal's zone, fixed at the first Strava import (kept in `imports/.strava-zone.json`, even if the journal's zone changes later), only decides the day and clock time its pieces are filed under. Journal 2.0.33 read the start as wall time in the journal's zone, which placed workouts off by the journal's UTC offset; importing the same download again sets those workouts aside and files them at the right time. A workout the owner deleted before that stays deleted; the deletion is matched under the key the earlier version used.
- **The download is not kept:** an upload is removed when its import ends; one never started is removed once it is an hour old, the next time the journal starts.
- **Deleting:** the body app deletes one workout (every piece, on every day it covers); a Strava import's own page deletes the whole import. A workout deleted on its own stays deleted when a download is imported again, unless its start time or length changed on Strava. A deleted import is released: importing the same download again brings its workouts back. Deletes don't reach copies made before them (encrypted backup, or wherever the journal folder was copied).

## Apple Health and Oura

Both are command-line imports for now, with no import card. The import page's generic start can't save an Apple export, because it can't pass the go-ahead flag.

```text
solstone journal importer <export> --confirm-body-save        # save an Apple export
solstone journal importer <export> --dry-run [--json]         # say what a save would read; no scan
solstone journal importer --connect oura                      # authorize Oura
solstone journal importer --sync oura                         # say what a save would ask for; writes nothing
solstone journal importer --sync oura --save                  # bring Oura in
```

`--confirm-health-save` is a hidden alias of `--confirm-body-save`.

**Apple Health.** A folder or zip holding `export.xml`. The whole export is read each time, and importing it again changes nothing, so `--date-from`, `--date-to` and `--force` are refused. Nothing of the export is kept.

**Oura.** With no range, each endpoint asks for every stretch from 2015-01-01 to today that `imports/.oura-coverage.json` doesn't hold, and its trailing seven days. An endpoint that answers with nothing for a stretch has covered it. A stretch whose only trouble is a `Deferred` hole isn't asked, because asking can't fill it. A first run that stops part way carries on from what it holds. `--date-from` (with `--date-to`, which defaults to today) or `--window-days N` ask only what they name. `--scheduled` means nobody is there. A move across the owner's midnight is kept at the time first saved, and the summary says so.

**Facets.** `--facet F` and `--no-facet` mean what they mean for Strava. With neither, a saving run with a terminal on stdin and stdout asks the question the card asks and remembers the answer; with nobody there it uses the remembered facet, if it's enabled, and never makes one. Flags are checked first, then the question, then everything else, so a refused flag writes nothing and a terminal left waiting holds no lock. Only an owner's answer for this import moves a record already listed under another facet. At the terminal the question says first how many could move, and the count is taken again once the run holds its lock: if more would move, none do and the summary says so. `--facet` on the command line says afterward how many moved.

**An unfinished conversion.** Each import converts earlier Strava records first. A workout it can't get to can sit beside an Apple or Oura record of the same workout until it converts, and the summary counts it.

**Tokens.** `refresh` holds Oura's answer in `config/oura-renewal.json` (mode 0600, backed up as the token's own config file is) until it reaches config. A run applies a held answer under `imports/oura.json`, only while config still holds the token it spent; a reconnect clears it.

**The phone.** The embedded importer refuses Apple Health and every Oura connect and sync.

## Earlier body imports

An earlier release saved Apple Health and Oura data, and before it Python saved those and Dexcom Clarity, as bundles under `imports/`: a directory with `normalized/` and neither `body-touched.jsonl` nor `body-progress.json`. They stay on disk, untouched, and nothing reads their rows. So do `imports/health-dedupe.sqlite`, `imports/oura.json` as a cursor, and `imports/_approvals/`.

- The Body app lists each one with its source and span, and how to import again. Dexcom Clarity has no importer now, and the list says so.
- A day page, and MCP's `list_transcripts` for a connection with Transcripts and every facet, say when an earlier import holds a source's data for a day the new path hasn't reached.
- A restore doesn't read them, so a damaged one can't stop it.
- Importing again retires the line for the days it read. An import's manifest records those days as `span`.

## What stays out of scope

- Oura webhooks.
- Health Auto Export or a custom HealthKit ingest service.
- Any LAN, public, or phone-to-Mac body ingest service.
- Medical advice, recommendations, or anomaly interpretation.
