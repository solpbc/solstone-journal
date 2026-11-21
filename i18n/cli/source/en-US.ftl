journal.cli.streams.arguments = usage: { $syntax }{ $break }{ $command }: error: unrecognized arguments: { $arguments }

journal.cli.streams.empty = No streams found

journal.cli.streams.header.day = Last Day

journal.cli.streams.header.name = Name

journal.cli.streams.header.segment = Last Segment

journal.cli.streams.header.sequence = Seq

journal.cli.streams.header.type = Type

journal.cli.streams.help = usage: { $syntax }{ $paragraph }Inspect and manage stream identity{ $paragraph }positional arguments:{ $break }{ $indent }{ $name_input }{ $name_space }Stream name to inspect (omit to list all streams){ $paragraph }options:{ $break }{ $indent }{ $help_input }{ $help_space }show this help message and exit{ $break }{ $indent }{ $rebuild_input }{ $rebuild_space }Reconstruct stream state from per-segment markers{ $break }{ $indent }{ $verbose_input }{ $verbose_space }Enable verbose output{ $break }{ $indent }{ $debug_input }{ $debug_space }Enable debug logging

journal.cli.streams.inspect.malformed = Could not read stream { $path }: malformed record

journal.cli.streams.inspect.not.found = Stream not found: { $name }

journal.cli.streams.list.error = Could not list streams: { $error }

journal.cli.streams.marker.sequence = unreadable marker { $path }: invalid sequence

journal.cli.streams.marker.unreadable = unreadable marker { $path }: could not read marker

journal.cli.streams.rebuild.empty = { $segment_quantity ->
    [one] No streams found ({ $segments } segment scanned)
   *[other] No streams found ({ $segments } segments scanned)
}

journal.cli.streams.rebuild.result = { $stream_quantity ->
    [one] Rebuilt { $streams } stream from { $segment_quantity ->
        [one] { $segments } segment:
       *[other] { $segments } segments:
    }
   *[other] Rebuilt { $streams } stream(s) from { $segment_quantity ->
        [one] { $segments } segment:
       *[other] { $segments } segments:
    }
}

journal.cli.streams.record.malformed = Could not read stream record { $path }: malformed record

journal.cli.streams.repair.ahead = { $stream }: stream state sequence is ahead of markers; unchanged

journal.cli.streams.repair.locked = { $stream }: could not lock { $path }; unchanged

journal.cli.streams.repair.malformed = { $stream }: malformed stream record at { $path }; unchanged

journal.cli.streams.repair.missing = { $stream }: no stream record at { $path }; unchanged

journal.cli.streams.repair.write.failed = { $stream }: write failed for { $path }: atomic publication failed

journal.cli.streams.segments.error = Could not list segments: { $error }

journal.cli.streams.supervisor = journal isn't running. start it with '{ $command }' and retry.

journal.cli.doctor.context.failed = doctor failed to resolve context: { $error }

journal.cli.doctor.help = usage: { $syntax }{ $line_break }{ $line_break }Run { $brand } diagnostics.{ $line_break }{ $line_break }options:{ $line_break }{ $indent }{ $help_option }{ $help_gap }show this help message and exit{ $line_break }{ $indent }{ $verbose_option }{ $verbose_gap }print every check result{ $line_break }{ $indent }{ $json_option }{ $json_gap }emit JSON instead of text{ $line_break }{ $indent }{ $jsonl_option }{ $jsonl_gap }emit one-JSON-per-line events instead of text{ $line_break }{ $indent }{ $port_option }{ $port_gap }port to probe (default: { $port }){ $line_break }{ $indent }{ $readiness_option }{ $readiness_gap }run the setup readiness battery{ $line_break }{ $line_break }If '{ $command }' is unavailable, build the native doctor from this checkout:{ $line_break }{ $build_command }

journal.cli.doctor.usage.argument = usage: { $syntax }{ $line_break }{ $command }: error: unexpected argument

journal.cli.doctor.usage.formats = usage: { $syntax }{ $line_break }{ $command }: error: { $json_flag } and { $jsonl_flag } are mutually exclusive

journal.cli.doctor.usage.port = usage: { $syntax }{ $line_break }{ $command }: error: { $port_flag } requires an integer

journal.cli.doctor.backup.status.unconfigured = backup not configured

journal.cli.doctor.backup.status.off = backup off

journal.cli.doctor.backup.status.no.attempt = backup on; no recorded attempt

journal.cli.doctor.backup.status.invalid.settings = backup status unavailable: invalid backup settings

journal.cli.doctor.backup.status.invalid.switch = backup status unavailable: invalid on/off setting

journal.cli.doctor.backup.status.invalid.pause = backup status unavailable: invalid pause setting

journal.cli.doctor.backup.status.unavailable.journal = backup status unavailable: journal unreadable

journal.cli.doctor.backup.status.unavailable.settings = backup status unavailable: settings unreadable

journal.cli.doctor.capture.health.inspect = open { $health_path } to inspect each device

journal.cli.doctor.capture.health.active = { $rollup }; the { $brand } app on every device that has added to your journal is current

journal.cli.doctor.capture.health.no.senders = { $rollup }; the { $brand } app hasn't added anything to your journal yet

journal.cli.doctor.capture.health.quiet = { $rollup }; no rejected uploads recorded; see devices for last delivery times

journal.cli.doctor.capture.health.data.unavailable = { $rollup }; device activity unavailable

journal.cli.doctor.capture.health.registry.unavailable = { $rollup }; device list unavailable

journal.cli.doctor.client.binding.active = active client records={ $records }; unbound=0; { $certificate_term } binding is authoritative

journal.cli.doctor.client.delivery.stall.activity.unavailable = device activity unavailable

journal.cli.doctor.client.delivery.stall.list.unavailable = device list unavailable

journal.cli.doctor.client.delivery.stall.quiet = no rejected uploads recorded; see devices for last delivery times

journal.cli.doctor.client.delivery.stall.inspect = open { $health_path } to inspect the rejected upload

journal.cli.doctor.client.delivery.stall.no.senders = the { $brand } app hasn't added anything to your journal yet

journal.cli.doctor.client.ingest.health.rejection = device { $device } failing ingest: { $reason }, { $attempts }x since { $since }

journal.cli.doctor.client.ingest.health.healthy = no devices failing ingest

journal.cli.doctor.client.ingest.health.repair = update or restart the device, then confirm a valid upload clears the rejection


journal.cli.doctor.client.transport.refusal.healthy = no devices had requests turned away in the last 7 days

journal.cli.doctor.client.transport.refusal.repair = this is a busy connection, not a lost one; report it if it keeps happening

journal.cli.doctor.config.dir.readable.owner.unavailable = could not locate the owner base: { $error }

journal.cli.doctor.config.dir.readable.config.writable = home and service config dir are writable ({ $path })

journal.cli.doctor.config.dir.readable.home.missing = home directory does not exist: { $path }

journal.cli.doctor.config.dir.readable.home.unusable = home directory is not readable and writable: { $path }

journal.cli.doctor.config.dir.readable.config.will.create = home is writable; install will create { $path }

journal.cli.doctor.config.dir.readable.config.unwritable = service config directory is not writable: { $path }

journal.cli.doctor.config.dir.readable.owner.windows = the Windows owner base is only readable on Windows

journal.cli.doctor.config.restore = repair or restore { $config_file } from a backup

journal.cli.doctor.default.stt.ready.backend = configured backend is { $backend }; parakeet readiness not applicable

journal.cli.doctor.default.stt.ready.download = { $coreml } parakeet model is not downloaded — fetch it with: { $models_command }

journal.cli.doctor.default.stt.ready.other.model.ready = parakeet model ready at { $path }

journal.cli.doctor.default.stt.ready.unsupported = parakeet not supported on this platform

journal.cli.doctor.device.day.listing.repair = a device syncing that day is told the journal cannot read it; a { $brand } app older than the 2026-09 fix shows that as offline; inspect the named stream directory's segments and their { $events_file }

journal.cli.doctor.device.day.listing.fault = day { $day } stream { $stream } device { $device }: { $reason }

journal.cli.doctor.device.day.listing.streams.unavailable = device streams unavailable: { $error }

journal.cli.doctor.device.day.listing.streams.unbound = no device streams bound

journal.cli.doctor.device.day.listing.readable = the journal can read the last { $days } days for every bound device

journal.cli.doctor.devices.absent = no registered devices

journal.cli.doctor.devices.unavailable = device records unavailable: { $error }

journal.cli.doctor.disk.space.package.install = a package install is replaced in place when it updates, so no second copy is needed

journal.cli.doctor.disk.space.space.inspect = could not inspect free space at { $path }: { $error }

journal.cli.doctor.disk.space.size.unreadable = could not measure installation tree at { $path }: { $error }

journal.cli.doctor.disk.space.root.unresolved = could not resolve installation root from install bin directory: { $path }

journal.cli.doctor.disk.space.space.unreadable = free space is not readable at { $path }

journal.cli.doctor.disk.space.space.available = { $free_gib } { $gib } free on the filesystem holding { $path } ({ $required_gib } { $gib } is a second copy of this install plus headroom)

journal.cli.doctor.disk.space.size.overflow = installation tree size overflow at { $path }

journal.cli.doctor.disk.space.space.low = only { $free_gib } { $gib } free on the filesystem holding { $path }; { $required_gib } { $gib } is a second copy of this install plus headroom

journal.cli.doctor.disk.space.layout.unsupported = resolved installation root is not a layout install tree

journal.cli.doctor.journal.absent = no local journal

journal.cli.doctor.journal.caught.up.coverage.unavailable = couldn't determine daily unit coverage on { $day }: { $error }

journal.cli.doctor.journal.caught.up.backlog.unavailable = couldn't fully determine; backlog read failed: { $error }

journal.cli.doctor.journal.caught.up.definitions.unavailable = couldn't fully determine; couldn't locate the daily processing definitions: { $error }

journal.cli.doctor.journal.caught.up.facets.day.unavailable = couldn't fully determine; failed to check active facets for { $day }: { $error }

journal.cli.doctor.journal.caught.up.chronicle.unavailable = couldn't fully determine; failed to read chronicle: { $error }

journal.cli.doctor.journal.caught.up.chronicle.entry.unavailable = couldn't fully determine; failed to read chronicle entry: { $error }

journal.cli.doctor.journal.caught.up.configs.unavailable = couldn't fully determine; failed to read daily configs: { $error }

journal.cli.doctor.journal.caught.up.units.entry.unavailable = couldn't fully determine; failed to read daily-unit entry for { $day }: { $error }

journal.cli.doctor.journal.caught.up.units.unavailable = couldn't fully determine; failed to read daily-units directory for { $day }: { $error }

journal.cli.doctor.journal.caught.up.facets.unavailable = couldn't fully determine; failed to read facets: { $error }

journal.cli.doctor.journal.caught.up.unit.invalid = couldn't fully determine; malformed daily unit file { $file_name } on day { $day }: { $source }

journal.cli.doctor.journal.caught.up.unit.unreadable = couldn't fully determine; unreadable daily unit file { $file_name } on day { $day }: { $source }

journal.cli.doctor.journal.caught.up.unit.identity = couldn't read daily unit file { $file_name } on day { $day }: record identity does not match its path

journal.cli.doctor.journal.caught.up.unit.version = couldn't read daily unit file { $file_name } on day { $day }: unsupported record version { $version }

journal.cli.doctor.journal.caught.up.outside.backlog = daily unit outside the recent backlog on { $day }

journal.cli.doctor.journal.caught.up.inspect = open health to see waiting work and any spending confirmation; reprocess a past day to give it priority after today

journal.cli.doctor.journal.caught.up.inventory.fix = open health to see waiting work and its next step

journal.cli.doctor.journal.caught.up.repair = re-run { $doctor_command }; check the health logs if it persists

journal.cli.doctor.journal.caught.up.inventory.waiting = whole-journal inspection has not reported yet

journal.cli.doctor.journal.caught.up.inventory.unavailable = whole-journal inspection unavailable: { $error }

journal.cli.doctor.journal.dir.writable.parent.unwritable = journal dir absent; nearest existing ancestor is not writable: { $path }

journal.cli.doctor.journal.dir.writable.parent.writable = journal dir absent; parent { $path } is writable

journal.cli.doctor.journal.dir.writable.unwritable = journal dir not writable: { $path }

journal.cli.doctor.journal.dir.writable.writable = journal dir writable: { $path }

journal.cli.doctor.journal.dir.writable.not.directory = journal path exists but is not a directory: { $path }

journal.cli.doctor.journal.dir.writable.move = move or remove { $path }, then re-run

journal.cli.doctor.journal.sources.readable.inspect.folders = check the day folders named above — a name that is not a calendar date is not a day the journal can process

journal.cli.doctor.journal.sources.readable.stats.missing = couldn't check — no statistics to read

journal.cli.doctor.journal.sources.readable.stats.invalid = couldn't check — the statistics aren't readable

journal.cli.doctor.journal.sources.readable.stats.predates = couldn't check — the statistics predate this version

journal.cli.doctor.journal.sources.readable.repair.files = repair the file named above, then run { $reprocess_command } for each listed day that has already ended

journal.cli.doctor.journal.sources.readable.reprocess = run { $reprocess_command } for each listed day that has already ended

journal.cli.doctor.journal.sources.readable.healthy = the last scan found no unreadable days

journal.cli.doctor.launchd.stale.plist.invalid = could not parse plist: { $plist_error }: { $error }

journal.cli.doctor.launchd.stale.plist.absent = launchd plist absent

journal.cli.doctor.launchd.stale.plist.target.exists = launchd plist target exists ({ $path })

journal.cli.doctor.launchd.stale.plist.arguments.missing = plist is missing { $plist_argument }

journal.cli.doctor.launchd.stale.plist.target.missing = plist points to missing executable: { $path }

journal.cli.doctor.launchd.stale.plist.repair = run { $uninstall_command }, then run { $install_command } separately to reinstall a headless background service

journal.cli.doctor.local.bin.solstone.reachable.file.missing = { $path } is missing

journal.cli.doctor.local.bin.solstone.reachable.file.invalid = { $path } is not a file

journal.cli.doctor.local.bin.solstone.reachable.path.ready = { $wrapper_path } is on { $path_variable } at { $path }

journal.cli.doctor.local.bin.solstone.reachable.symlink.ready = { $wrapper_path } symlinks to { $path_variable } { $brand } at { $path }

journal.cli.doctor.local.bin.solstone.reachable.windows.terminal = open a new terminal window and try again; if { $brand } is still not found, reinstall the journal

journal.cli.doctor.local.bin.solstone.reachable.path.mismatch = { $path_variable } { $brand } resolves to { $resolved }, expected { $expected }

journal.cli.doctor.local.bin.solstone.reachable.windows.path.priority = put { $install_dir } ahead of { $other_dir } on your { $path_variable }

journal.cli.doctor.local.bin.solstone.reachable.path.repair = put { $bin_dir } earlier on { $path_variable }, or run { $setup_command } to repoint the managed wrapper

journal.cli.doctor.local.bin.solstone.reachable.windows.repair = reinstall the journal to restore { $executable }

journal.cli.doctor.local.bin.solstone.reachable.wrapper.install = run { $setup_command } to install the managed { $brand } wrapper

journal.cli.doctor.local.bin.solstone.reachable.path.absent = { $brand } is not on { $path_variable }

journal.cli.doctor.local.bin.solstone.reachable.windows.ready = { $brand } is on { $path_variable } at { $path }

journal.cli.doctor.orphan.segment.pdf.repair = add a readable { $transcript_file } beside the PDF original, then re-run { $doctor_command }

journal.cli.doctor.orphan.segment.pdf.clear = all raw PDF originals have readable document transcripts

journal.cli.doctor.orphan.segment.pdf.chronicle.unavailable = chronicle directory unavailable

journal.cli.doctor.parakeet.cpp.stt.ready.backend.not.selected = configured backend is not { $backend_name }; check not applicable

journal.cli.doctor.parakeet.cpp.stt.ready.runtime.install = install the system { $openmp } runtime that provides { $runtime_file }, then rerun { $doctor_command }

journal.cli.doctor.parakeet.cpp.stt.ready.install = { $backend_name } artifacts are not installed — fetch them with: { $provider_command }

journal.cli.doctor.parakeet.cpp.stt.ready.binary.unavailable = { $backend_name } binary cannot start

journal.cli.doctor.parakeet.cpp.stt.ready.runtime.unavailable = { $backend_name } cannot start: { $openmp } runtime unavailable ({ $runtime_file })

journal.cli.doctor.parakeet.cpp.stt.ready.unsupported = { $backend_name } is only supported on linux and windows

journal.cli.doctor.parakeet.cpp.stt.ready.ready = { $backend_name } ready (binaries + model installed, server reachable)

journal.cli.doctor.parakeet.cpp.stt.ready.online = { $backend_name } ready (server + model match the signed package, server reachable)

journal.cli.doctor.parakeet.cpp.stt.ready.start = { $server } is not reachable — start the journal service: { $start_command }

journal.cli.doctor.parakeet.cpp.stt.ready.unreachable = { $server } not reachable: { $error }

journal.cli.doctor.parakeet.cpp.stt.ready.server.missing = { $server } not reachable: your journal is running, but not { $server }

journal.cli.doctor.parakeet.cpp.stt.ready.offline = { $server } not reachable: your journal isn't running

journal.cli.doctor.path.repair = fix ownership/permissions of { $path }

journal.cli.doctor.sense.dispatch.clear = no Sense dispatch errors since the last successful handler sync

journal.cli.doctor.sense.dispatch.count.out.of.range = Sense status is incomplete: dispatch error count is outside its supported range

journal.cli.doctor.sense.dispatch.count.invalid = Sense status is incomplete: no valid nonnegative dispatch error count

journal.cli.doctor.service.absent = no local journal service

journal.cli.doctor.service.identity.repair.force = run { $setup_command } from this install to refresh the service

journal.cli.doctor.service.identity.repair = run { $setup_command } to reinstall the service

journal.cli.doctor.service.identity.target.current = service target matches current install: { $raw } -> { $path }

journal.cli.doctor.service.identity.windows.current = service target matches current install: { $command }

journal.cli.doctor.service.identity.windows.mismatch = service target mismatch: { $mismatch }

journal.cli.doctor.service.identity.target.mismatch = service target mismatch: { $raw } resolves to { $resolved }, expected { $expected }

journal.cli.doctor.service.identity.target.absent = service unit is malformed: no executable target in { $path }

journal.cli.doctor.service.registration.unreadable = couldn't read the journal's service registration — { $reason }

journal.cli.doctor.service.running.running = journal service is running

journal.cli.doctor.service.running.unit.failed = journal service unit is failed

journal.cli.doctor.service.running.registration.no.reader = no reader for the service registration

journal.cli.doctor.service.running.pid.invalid = probe child PID does not fit a process-group ID

journal.cli.doctor.service.running.logs = run { $logs_command }

journal.cli.doctor.service.running.restart = run { $restart_command }; if it persists, run { $logs_command }

journal.cli.doctor.service.running.start = run { $start_command }

journal.cli.doctor.service.running.stopped = service installed but not running

journal.cli.doctor.skill.state.link.missing = { $skill } missing at { $path }

journal.cli.doctor.skill.state.link.foreign = { $skill } points elsewhere at { $path }

journal.cli.doctor.skill.state.directories.missing = router skill directories are unavailable

journal.cli.doctor.skill.state.checkout.required = router skill state is only available from a source checkout

journal.cli.doctor.skill.state.links.ready = router skills { $brand }, journal are installed and current

journal.cli.doctor.skill.state.install = run { $skills_command }

journal.cli.doctor.skill.state.link.stale = stale router skill link at { $path }

journal.cli.doctor.supervisor.conflict.clear = no macOS supervisor conflict (native check: foreign-launcher scan only; no competing KeepAlive launcher found)

journal.cli.doctor.supervisor.conflict.unsupported = not supported on windows


journal.cli.doctor.task.pace.about.cap = a job is running long; it will be stopped automatically if it passes its cap — no action needed unless it persists

journal.cli.doctor.task.pace.status.unavailable = supervisor status unavailable

journal.cli.doctor.task.pace.healthy = tasks on pace

journal.cli.doctor.task.queue.holds.clear = all task partition records are clean

journal.cli.doctor.task.queue.holds.absent = no in-flight or held task partition records present

journal.cli.doctor.unretryable.transcribe.input.clear = 0 recordings the journal will not retry on its own (handler: { $handler }, { $reason_key }: { $reason_code })

journal.cli.doctor.unretryable.transcribe.input.open = cannot open { $path }: { $error }

journal.cli.doctor.unretryable.transcribe.input.read = cannot read { $path }: { $error }

journal.cli.doctor.unretryable.transcribe.input.directory.unreadable = cannot read segment dir { $path }: { $error }

journal.cli.doctor.unretryable.transcribe.input.entry.unreadable = cannot read segment entry in { $path }: { $error }

journal.cli.doctor.unretryable.transcribe.input.scan.failed = could not determine how many recordings the journal will not retry on its own ({ $reason })

journal.cli.doctor.unretryable.transcribe.input.segments.unreadable = failed to list segments for day { $day }: { $error }

journal.cli.doctor.unretryable.transcribe.input.days.unreadable = failed to read chronicle days: { $error }

journal.cli.doctor.unretryable.transcribe.input.row.unavailable = first row in { $path } cannot be established or exceeds byte limit

journal.cli.doctor.unretryable.transcribe.input.row.large = first row in { $path } exceeds byte limit

journal.cli.doctor.unretryable.transcribe.input.row.json = first row in { $path } is not valid json: { $error }

journal.cli.doctor.unretryable.transcribe.input.row.encoding = first row in { $path } is not valid utf-8: { $error }

journal.cli.doctor.unretryable.transcribe.input.identity.invalid = invalid segment identity in { $path }: { $error }

journal.cli.doctor.unretryable.transcribe.input.handler.missing = processing record in { $path } missing handler field

journal.cli.doctor.brain.age.unknown = unknown

journal.cli.doctor.brain.no.component = none

journal.cli.doctor.brain.report = { $headline }; { $state_key }={ $state }; { $reason_key }={ $reason }; { $component_key }={ $component }; { $age_key }={ $age }

journal.cli.doctor.brain.unknown = unknown: brain state unavailable

journal.cli.doctor.brain.unknown.reason = unknown: { $reason }

journal.cli.doctor.index.fix.checking = search is checking this itself and rebuilds if the damage is confirmed; run { $command } for details

journal.cli.doctor.index.fix.deletions = search keeps retrying these removals; run { $command } for details

journal.cli.doctor.index.fix.errors = run { $command } and check its errors

journal.cli.doctor.index.fix.failed.files = search retries these updates at its daily check; run { $command } for details

journal.cli.doctor.index.fix.failed.work = search retries this work every hour; run { $command } for details

journal.cli.doctor.index.fix.full = run { $command }

journal.cli.doctor.index.fix.rebuild = search rebuilds its index from your journal once the damage is confirmed; if it persists, run { $command }

journal.cli.doctor.index.fix.reset = run { $command }

journal.cli.doctor.index.fix.upgrade = update { $brand } to the newest version

journal.cli.doctor.index.fix.warnings = run { $command } and check its warnings

journal.cli.doctor.storage.privacy.choice = other accounts on this computer may be able to read your journal. its folder could not be made private on this drive. you can keep it on this drive or move it to a drive that keeps folders private. to keep it on this drive, choose that on your journal's health page.

journal.cli.doctor.backlog.pending = pending: { $days }

journal.cli.doctor.backlog.stuck = stuck: { $days }

journal.cli.doctor.backlog.unknown = unknown: { $days }

journal.cli.doctor.device.refusal.count = { $quantity ->
    [one] device { $device } had a request turned away: { $reason }, { $attempts }x through { $last_seen }
   *[other] device { $device } had requests turned away: { $reason }, { $attempts }x through { $last_seen }
}

journal.cli.doctor.facet.fix = { $unrouted_quantity ->
    [one] that segment is missing from the activity lists; check the facet declarations with `{ $doctor_command }` and recent sense runs with `{ $logs_command }`
   *[other] those segments are missing from the activity lists; check the facet declarations with `{ $doctor_command }` and recent sense runs with `{ $logs_command }`
}

journal.cli.doctor.facet.no.active = no active segments in the newest days

journal.cli.doctor.facet.routed = { $active_quantity ->
    [one] { $routed } of { $active } active segment since { $since } filed under a facet
   *[other] { $routed } of { $active } active segments since { $since } filed under a facet
}

journal.cli.doctor.facet.unrouted = { $active_quantity ->
    [one] { $unrouted } of { $active } active segment since { $since } { $unrouted_quantity ->
        [one] was
       *[other] were
    } filed under no facet
   *[other] { $unrouted } of { $active } active segments since { $since } { $unrouted_quantity ->
        [one] was
       *[other] were
    } filed under no facet
}

journal.cli.doctor.orphan.pdf.found = { $quantity ->
    [one] { $count } raw PDF original without a readable document transcript: { $paths }
   *[other] { $count } raw PDF original(s) without a readable document transcript: { $paths }
}

journal.cli.doctor.service.crash.list = crash-loop: { $services }

journal.cli.doctor.service.restart.attempts = { $quantity ->
    [one] { $name } ({ $attempts } restart attempt)
   *[other] { $name } ({ $attempts } restart attempts)
}

journal.cli.doctor.status.error = ERROR

journal.cli.doctor.status.fail = FAIL

journal.cli.doctor.status.ok = OK

journal.cli.doctor.status.skip = SKIP

journal.cli.doctor.status.warn = WARN

journal.cli.doctor.summary = { $total_quantity ->
    [one] doctor: { $total } check, { $failed } failed, { $warnings } { $warning_quantity ->
        [one] warning
       *[other] warnings
    }, { $skipped } skipped, { $errors } { $error_quantity ->
        [one] error
       *[other] errors
    }
   *[other] doctor: { $total } checks, { $failed } failed, { $warnings } { $warning_quantity ->
        [one] warning
       *[other] warnings
    }, { $skipped } skipped, { $errors } { $error_quantity ->
        [one] error
       *[other] errors
    }
}

journal.cli.doctor.transcription.unretryable = { $quantity ->
    [one] { $count } recording the journal will not retry on its own ({ $handler_key }: { $handler }, { $reason_key }: { $reason_code }): { $identities }
   *[other] { $count } recordings the journal will not retry on its own ({ $handler_key }: { $handler }, { $reason_key }: { $reason_code }): { $identities }
}

journal.cli.doctor.check.deferred = deferred check set: { $set }

journal.cli.doctor.execution.error = check execution failed: { $kind }

journal.cli.doctor.execution.error.detail = check execution failed: { $kind }: { $error }

journal.cli.doctor.platform.unsupported = not supported on { $platform }

journal.cli.doctor.progress.check = { $seconds }s checking { $name }

journal.cli.doctor.progress.hash = { $seconds }s verifying { $member }: { $hashed } of { $total } { $megabytes }

journal.cli.doctor.service.conflict.warning = resolve the macOS supervisor topology warning before changing the journal service

journal.cli.doctor.supervisor.conflict.repair = { $quantity ->
    [one] remove the foreign launcher targeting { $app_path }: { $commands }; then rerun { $doctor_command }
   *[other] remove foreign launchers targeting { $app_path }: { $commands }; then rerun { $doctor_command }
}

journal.cli.doctor.supervisor.conflict.report = { $quantity ->
    [one] macOS supervisor conflict: { $count } foreign { $keep_alive } launcher targets { $app_path }
   *[other] macOS supervisor conflict: { $count } foreign { $keep_alive } launchers target { $app_path }
}

journal.cli.doctor.supervisor.conflict.resolve = { $quantity ->
    [one] resolve the macOS supervisor conflict first: remove the foreign launcher targeting { $app_path }: { $commands }; then rerun { $doctor_command }
   *[other] resolve the macOS supervisor conflict first: remove foreign launchers targeting { $app_path }: { $commands }; then rerun { $doctor_command }
}

journal.cli.doctor.parakeet.unreachable = { $cause ->
    [absent] { $server } not reachable: nothing is listening
    [unknown] { $server } not reachable: can't tell whether anything is listening
    [runtime] { $server } not reachable: nothing answered
    [timeout] { $server } not reachable: took too long to answer
   *[transport] { $server } not reachable: the connection dropped
}

journal.cli.doctor.sense.dispatch.capped.reason = at least 99 Sense dispatch errors since the last successful handler sync; latest reason: { $reason }

journal.cli.doctor.sense.dispatch.capped.unknown = at least 99 Sense dispatch errors since the last successful handler sync; latest reason unavailable

journal.cli.doctor.sense.dispatch.errors.reason = { $quantity ->
    [one] { $count } Sense dispatch error since the last successful handler sync; latest reason: { $reason }
   *[other] { $count } Sense dispatch errors since the last successful handler sync; latest reason: { $reason }
}

journal.cli.doctor.sense.dispatch.errors.unknown = { $quantity ->
    [one] { $count } Sense dispatch error since the last successful handler sync; latest reason unavailable
   *[other] { $count } Sense dispatch errors since the last successful handler sync; latest reason unavailable
}

journal.cli.doctor.sense.dispatch.unreachable = { $cause ->
    [absent] could not confirm Sense dispatch health: nothing is listening
    [unknown] could not confirm Sense dispatch health: can't tell whether anything is listening
    [runtime] could not confirm Sense dispatch health: nothing answered
    [timeout] could not confirm Sense dispatch health: took too long to answer
   *[transport] could not confirm Sense dispatch health: the connection dropped
}

journal.cli.doctor.service.running.unreachable = { $cause ->
    [absent] couldn't get a status — nothing is listening
    [unknown] couldn't get a status — can't tell whether anything is listening
    [runtime] couldn't get a status — nothing answered
    [timeout] couldn't get a status — took too long to answer
   *[transport] couldn't get a status — the connection dropped
}

journal.cli.doctor.sources.days.more = { $days }, and { $more } more

journal.cli.doctor.sources.fold.failed = { $quantity ->
    [one] { $count } day didn't finish processing ({ $days })
   *[other] { $count } day(s) didn't finish processing ({ $days })
}

journal.cli.doctor.sources.statistics.refresh = run { $command }; check the health logs if it persists

journal.cli.doctor.sources.statistics.stale = couldn't check — the statistics are { $hours } hours old

journal.cli.doctor.sources.unreadable = { $quantity ->
    [one] { $count } day couldn't be scanned ({ $days })
   *[other] { $count } day(s) couldn't be scanned ({ $days })
}

journal.cli.doctor.sources.unreadable.cause = { $quantity ->
    [one] { $count } day couldn't be scanned ({ $days }): { $day }: { $cause }
   *[other] { $count } day(s) couldn't be scanned ({ $days }): { $day }: { $cause }
}

journal.cli.doctor.sources.unreadable.undated = { $quantity ->
    [one] { $count } day couldn't be scanned ({ $days }): that day: { $cause }
   *[other] { $count } day(s) couldn't be scanned ({ $days }): that day: { $cause }
}

journal.cli.doctor.backlog.band = { $recording_count ->
    [one] band { $band }: { $recordings } recording ready / { $recordings_left } left, { $outputs } daily { $output_count ->
        [one] output
       *[other] outputs
    } ready / { $outputs_left } left, { $activities } { $activity_count ->
        [one] activity
       *[other] activities
    } left, { $requests } owner { $request_count ->
        [one] request
       *[other] requests
    }, { $permission } waiting for permission, { $prerequisites } waiting for prerequisites
   *[other] band { $band }: { $recordings } recordings ready / { $recordings_left } left, { $outputs } daily { $output_count ->
        [one] output
       *[other] outputs
    } ready / { $outputs_left } left, { $activities } { $activity_count ->
        [one] activity
       *[other] activities
    } left, { $requests } owner { $request_count ->
        [one] request
       *[other] requests
    }, { $permission } waiting for permission, { $prerequisites } waiting for prerequisites
}

journal.cli.doctor.backlog.caught = caught up

journal.cli.doctor.backlog.caught.capped = { $day_count ->
    [one] caught up; { $days } day completed with { $unit_count ->
        [one] a capped daily unit
       *[other] capped daily unit(s)
    }
   *[other] caught up; { $days } day(s) completed with { $unit_count ->
        [one] a capped daily unit
       *[other] capped daily unit(s)
    }
}

journal.cli.doctor.backlog.change.recorded = { $name } recorded a change on { $location }; resolve the recorded change before that day is redone

journal.cli.doctor.backlog.change.started = { $name } may have started a change but did not confirm completion on { $location }; resolve the in-progress change before reprocessing

journal.cli.doctor.backlog.checked = recent work checked

journal.cli.doctor.backlog.checked.capped = { $day_count ->
    [one] recent work checked; { $days } day completed with { $unit_count ->
        [one] a capped daily unit
       *[other] capped daily unit(s)
    }
   *[other] recent work checked; { $days } day(s) completed with { $unit_count ->
        [one] a capped daily unit
       *[other] capped daily unit(s)
    }
}

journal.cli.doctor.backlog.deferred = { $name } stopped on { $location }; it can retry on a later run after the local day changes

journal.cli.doctor.backlog.deferred.reason = { $name } stopped on { $location } with reason { $reason }; it can retry on a later run after the local day changes

journal.cli.doctor.backlog.eligible = { $name } stopped on { $location }; it can retry on the next eligible run

journal.cli.doctor.backlog.eligible.reason = { $name } stopped on { $location } with reason { $reason }; it can retry on the next eligible run

journal.cli.doctor.backlog.empty.date = none

journal.cli.doctor.backlog.exhausted = { $name } stopped on { $location } after its automatic retry; run { $command }

journal.cli.doctor.backlog.exhausted.reason = { $name } stopped on { $location } with reason { $reason } after its automatic retry; run { $command }

journal.cli.doctor.backlog.hold.retry = { $day }: { $reason } (retry { $unit })

journal.cli.doctor.backlog.inventory.partial.current = { $quantity ->
    [one] inspected { $inspected } of { $total } day, { $oldest } through { $newest }, as of { $as_of }; { $bands }; incomplete inspection; set aside: { $holds }
   *[other] inspected { $inspected } of { $total } days, { $oldest } through { $newest }, as of { $as_of }; { $bands }; incomplete inspection; set aside: { $holds }
}

journal.cli.doctor.backlog.inventory.partial.last = { $quantity ->
    [one] inspected { $inspected } of { $total } day, { $oldest } through { $newest }, as of { $as_of }; { $bands }; incomplete inspection; last known progress; set aside: { $holds }
   *[other] inspected { $inspected } of { $total } days, { $oldest } through { $newest }, as of { $as_of }; { $bands }; incomplete inspection; last known progress; set aside: { $holds }
}

journal.cli.doctor.backlog.inventory.whole.current = { $quantity ->
    [one] inspected { $inspected } of { $total } day, { $oldest } through { $newest }, as of { $as_of }; { $bands }; whole-journal inspection; set aside: { $holds }
   *[other] inspected { $inspected } of { $total } days, { $oldest } through { $newest }, as of { $as_of }; { $bands }; whole-journal inspection; set aside: { $holds }
}

journal.cli.doctor.backlog.inventory.whole.last = { $quantity ->
    [one] inspected { $inspected } of { $total } day, { $oldest } through { $newest }, as of { $as_of }; { $bands }; whole-journal inspection; last known progress; set aside: { $holds }
   *[other] inspected { $inspected } of { $total } days, { $oldest } through { $newest }, as of { $as_of }; { $bands }; whole-journal inspection; last known progress; set aside: { $holds }
}

journal.cli.doctor.backlog.pending.counts = { $pending_count ->
    [one] { $pending } day pending, { $stuck } { $stuck_count ->
        [one] day
       *[other] day(s)
    } stuck
   *[other] { $pending } day(s) pending, { $stuck } { $stuck_count ->
        [one] day
       *[other] day(s)
    } stuck
}

journal.cli.doctor.backlog.pending.oldest = { $pending_count ->
    [one] { $pending } day pending, { $stuck } { $stuck_count ->
        [one] day
       *[other] day(s)
    } stuck; oldest outstanding { $day }
   *[other] { $pending } day(s) pending, { $stuck } { $stuck_count ->
        [one] day
       *[other] day(s)
    } stuck; oldest outstanding { $day }
}

journal.cli.doctor.backlog.resolve = { $name } stopped on { $location }; resolve the conflict before reprocessing a past day

journal.cli.doctor.backlog.resolve.reason = { $name } stopped on { $location } with reason { $reason }; resolve the conflict before reprocessing a past day

journal.cli.doctor.backlog.retry = { $name } stopped on { $location }; run { $command }

journal.cli.doctor.backlog.retry.reason = { $name } stopped on { $location } with reason { $reason }; run { $command }

journal.cli.doctor.backlog.review.started = { $name } may have started an entity review change but did not confirm completion on { $location }; resolve the in-progress change before reprocessing

journal.cli.doctor.backlog.unfinished = { $activity_count ->
    [one] { $activities } activity on { $days } completed { $day_count ->
        [one] day
       *[other] days
    } couldn't finish processing
   *[other] { $activities } activities on { $days } completed { $day_count ->
        [one] day
       *[other] days
    } couldn't finish processing
}

journal.cli.doctor.backlog.unknown.count = { $quantity ->
    [one] couldn't fully determine; { $days } day unknown
   *[other] couldn't fully determine; { $days } day(s) unknown
}

journal.cli.doctor.backup.association = { $association ->
    [unbound] setup association unknown; history only
    [unknown] current setup association unavailable
    [superseded] superseded setup; history only
    [current] matches the selected setup
   *[invalid] recorded setup association unavailable
}

journal.cli.doctor.backup.attempt.incomplete = { $outcome ->
    [success] backup on; latest recorded attempt: successful copy at { $date }; some files were not copied; { $history }; { $association }
    [incomplete] backup on; latest recorded attempt: incomplete copy at { $date }; some files were not copied; { $history }; { $association }
   *[failed] backup on; latest recorded attempt: failed at { $date }; some files were not copied; { $history }; { $association }
}

journal.cli.doctor.backup.attempt.locked = { $outcome ->
    [success] backup on; latest recorded attempt: successful copy at { $date }; repository was locked; { $history }; { $association }
    [incomplete] backup on; latest recorded attempt: incomplete copy at { $date }; repository was locked; { $history }; { $association }
   *[failed] backup on; latest recorded attempt: failed at { $date }; repository was locked; { $history }; { $association }
}

journal.cli.doctor.backup.attempt.ordinary = { $outcome ->
    [success] backup on; latest recorded attempt: successful copy at { $date }; { $history }; { $association }
    [incomplete] backup on; latest recorded attempt: incomplete copy at { $date }; { $history }; { $association }
   *[failed] backup on; latest recorded attempt: failed at { $date }; { $history }; { $association }
}

journal.cli.doctor.backup.attempt.refused = { $outcome ->
    [success] backup on; latest recorded attempt: successful copy at { $date }; backup key refused; { $history }; { $association }
    [incomplete] backup on; latest recorded attempt: incomplete copy at { $date }; backup key refused; { $history }; { $association }
   *[failed] backup on; latest recorded attempt: failed at { $date }; backup key refused; { $history }; { $association }
}

journal.cli.doctor.backup.attempt.timeout = { $outcome ->
    [success] backup on; latest recorded attempt: successful copy at { $date }; timed out; { $history }; { $association }
    [incomplete] backup on; latest recorded attempt: incomplete copy at { $date }; timed out; { $history }; { $association }
   *[failed] backup on; latest recorded attempt: failed at { $date }; timed out; { $history }; { $association }
}

journal.cli.doctor.backup.attempt.unknown = { $outcome ->
    [success] backup on; latest recorded attempt: successful copy at { $date }; cause unknown; { $history }; { $association }
    [incomplete] backup on; latest recorded attempt: incomplete copy at { $date }; cause unknown; { $history }; { $association }
   *[failed] backup on; latest recorded attempt: failed at { $date }; cause unknown; { $history }; { $association }
}

journal.cli.doctor.backup.date.unknown = unknown date

journal.cli.doctor.backup.details = open backup or health in your journal for details

journal.cli.doctor.backup.history.incomplete.dated = { $quantity ->
    [one] last successful copy (historical): snapshot { $snapshot } at { $date }, age { $seconds } second; incomplete copy, some files were not copied; present coverage and verification unknown
   *[other] last successful copy (historical): snapshot { $snapshot } at { $date }, age { $seconds } seconds; incomplete copy, some files were not copied; present coverage and verification unknown
}

journal.cli.doctor.backup.history.incomplete.undated = last successful copy (historical): snapshot { $snapshot } at { $date }, age unknown; incomplete copy, some files were not copied; present coverage and verification unknown

journal.cli.doctor.backup.history.success.dated = { $quantity ->
    [one] last successful copy (historical): snapshot { $snapshot } at { $date }, age { $seconds } second; present coverage and verification unknown
   *[other] last successful copy (historical): snapshot { $snapshot } at { $date }, age { $seconds } seconds; present coverage and verification unknown
}

journal.cli.doctor.backup.history.success.undated = last successful copy (historical): snapshot { $snapshot } at { $date }, age unknown; present coverage and verification unknown

journal.cli.doctor.backup.history.unknown = last successful copy unknown

journal.cli.doctor.backup.invalid.history = backup on; recorded status unavailable; { $history }

journal.cli.doctor.backup.paused.history = backup paused by you since { $date }; { $history }

journal.cli.doctor.backup.snapshot.unknown = unknown

journal.cli.doctor.durability.action.recovered = no action is needed: recovered records stay beside the originals and are listed here

journal.cli.doctor.durability.action.refused = the journal will not start until this clears; each line above says whether it clears after the computer restarts or needs help

journal.cli.doctor.durability.action.unreadable = check the paths under 'could not inspect'; any recovered records stay beside the originals and are listed here

journal.cli.doctor.durability.admission.helper = { $finding ->
    [exited] generation { $generation } admission { $admission }: helper { $identity }, exited
    [escalated] generation { $generation } admission { $admission }: helper { $identity }, retired (escalated: true)
    [retired] generation { $generation } admission { $admission }: helper { $identity }, retired
    [unacknowledged] generation { $generation } admission { $admission }: helper { $identity }, never-acknowledged
    [spawn] generation { $generation } admission { $admission }: helper { $identity }, spawn-failed
    [unidentified] generation { $generation } admission { $admission }: helper { $identity }, unidentified
    [rejected] generation { $generation } admission { $admission }: helper { $identity }, rejected-and-reaped
   *[reused] generation { $generation } admission { $admission }: helper { $identity }, pid-reused-by-another-user
}

journal.cli.doctor.durability.admission.service = { $finding ->
    [exited] generation { $generation } admission { $admission }: service { $identity }, exited
    [escalated] generation { $generation } admission { $admission }: service { $identity }, retired (escalated: true)
    [retired] generation { $generation } admission { $admission }: service { $identity }, retired
    [unacknowledged] generation { $generation } admission { $admission }: service { $identity }, never-acknowledged
    [spawn] generation { $generation } admission { $admission }: service { $identity }, spawn-failed
    [unidentified] generation { $generation } admission { $admission }: service { $identity }, unidentified
    [rejected] generation { $generation } admission { $admission }: service { $identity }, rejected-and-reaped
   *[reused] generation { $generation } admission { $admission }: service { $identity }, pid-reused-by-another-user
}

journal.cli.doctor.durability.clean = no recorded recovery to report

journal.cli.doctor.durability.clean.outstanding = { $quantity ->
    [one] no recorded recovery to report; the newest run (generation { $generation }) still owes { $launches } launch
   *[other] no recorded recovery to report; the newest run (generation { $generation }) still owes { $launches } launches
}

journal.cli.doctor.durability.closed = { $quantity ->
    [one] generation { $generation } was closed by the start that became generation { $successor } (unix { $time }): { $total } admission, { $exited } already exited, { $retired } stopped by the journal, { $never } never admitted
   *[other] generation { $generation } was closed by the start that became generation { $successor } (unix { $time }): { $total } admissions, { $exited } already exited, { $retired } stopped by the journal, { $never } never admitted
}

journal.cli.doctor.durability.closed.restarted = { $quantity ->
    [one] generation { $generation } was closed by the start that became generation { $successor } (unix { $time }): { $total } admission, { $exited } already exited, { $retired } stopped by the journal, { $never } never admitted; the computer had restarted since
   *[other] generation { $generation } was closed by the start that became generation { $successor } (unix { $time }): { $total } admissions, { $exited } already exited, { $retired } stopped by the journal, { $never } never admitted; the computer had restarted since
}

journal.cli.doctor.durability.heading.aside = bookkeeping set aside ({ $count }):

journal.cli.doctor.durability.heading.closed = runs closed by a later start ({ $count }):

journal.cli.doctor.durability.heading.refused = starts refused ({ $count }):

journal.cli.doctor.durability.heading.unreadable = could not inspect ({ $count }):

journal.cli.doctor.durability.heading.unresolved = runs that did not stop cleanly ({ $count }):

journal.cli.doctor.durability.more = ... and { $count } more

journal.cli.doctor.durability.outstanding = { $quantity ->
    [one] the newest run (generation { $generation }) still owes { $launches } launch
   *[other] the newest run (generation { $generation }) still owes { $launches } launches
}

journal.cli.doctor.durability.refusal.restart = generation { $generation }: the record of processes a retirement started is lost, so the journal will not start; it clears after the computer restarts

journal.cli.doctor.durability.refusal.support = generation { $generation }: the record of processes a retirement started is lost, so the journal will not start; it will not clear on its own and needs help from support

journal.cli.doctor.durability.unresolved = generation { $generation }: { $reason }

journal.cli.doctor.capture.recent = the { $brand } app on { $client } last added { $hours }h ago

journal.cli.doctor.capture.recent.sources = { $quantity ->
    [one] the { $brand } app on { $client } last added { $hours }h ago ({ $sources } needs attention)
   *[other] the { $brand } app on { $client } last added { $hours }h ago ({ $sources } need attention)
}

journal.cli.doctor.capture.rollup = { $rollup }; { $devices }

journal.cli.doctor.capture.sources = the { $brand } app on { $client } is having trouble adding { $sources }

journal.cli.doctor.capture.trouble = the { $brand } app on { $client } is having trouble adding

journal.cli.doctor.capture.trouble.sources = { $quantity ->
    [one] the { $brand } app on { $client } is having trouble adding ({ $sources } needs attention)
   *[other] the { $brand } app on { $client } is having trouble adding ({ $sources } need attention)
}

journal.cli.doctor.clients.ledger.unavailable = authorized client ledger unavailable: { $reason }

journal.cli.doctor.delivery.rejected = the journal rejected an upload from { $client }

journal.cli.doctor.delivery.rejected.sources = { $quantity ->
    [one] the journal rejected an upload from { $client } ({ $sources } needs attention)
   *[other] the journal rejected an upload from { $client } ({ $sources } need attention)
}

journal.cli.doctor.devices.more = { $devices }{ $separator }+{ $extra } more

journal.cli.doctor.executable.directory = could not determine executable directory: { $path }

journal.cli.doctor.parakeet.phase = { $server } not reachable: your journal reports its state as { $phase }

journal.cli.doctor.parakeet.phase.checking = { $server } not reachable: your journal reports its state as checking

journal.cli.doctor.parakeet.reinstall = reinstall the journal

journal.cli.doctor.source.default = default

journal.cli.doctor.speakers.ready = { $command } installation ready

journal.cli.doctor.speakers.unavailable = { $command } installation unavailable

journal.cli.doctor.sync.local = this computer only ({ $host })

journal.cli.doctor.sync.peer.legacy = this computer only ({ $host }){ $newline }  last other writer: { $writer } (legacy machine { $prefix }...)

journal.cli.doctor.sync.peer.unknown = this computer only ({ $host }){ $newline }  last other writer: { $writer } (unidentified heartbeat)

journal.cli.doctor.sync.peer.writer = this computer only ({ $host }){ $newline }  last other writer: { $writer } (writer { $prefix }... run { $run })

journal.cli.doctor.tasks.duration = { $task } ({ $seconds }s of { $limit }s cap)

journal.cli.doctor.tasks.long = running long: { $tasks }

journal.cli.doctor.holds.partition = held partition { $partition }

journal.cli.doctor.holds.partition.stopped = partition { $partition } was running when the journal stopped; the journal holds it at its next start until its processes are gone

journal.cli.doctor.holds.record.empty = record file is empty

journal.cli.doctor.holds.record.filename = invalid partition file name in scope directory; the journal still reads it as a task record

journal.cli.doctor.holds.record.invalid = cannot parse record: { $error }

journal.cli.doctor.holds.record.stray = non-record entry in scope directory

journal.cli.doctor.holds.record.unreadable = cannot read record: { $error }

journal.cli.doctor.holds.remove = remove { $path }: it holds nothing

journal.cli.doctor.holds.scope.unknown = unrecognized scope directory format; the journal still holds any task recorded in it

journal.cli.doctor.holds.scope.unreadable = cannot list scope directory: { $error }

journal.cli.doctor.holds.stray = stray non-directory in task-queue in-flight path

journal.cli.doctor.holds.unreadable = cannot list in-flight holds directory: { $error }

journal.cli.doctor.parakeet.coreml.configured = parakeet check failed: installed cache { $installed } does not match configured { $configured }

journal.cli.doctor.parakeet.coreml.digest = parakeet check failed: asset digest mismatch for { $file }: expected { $expected }, found { $found }

journal.cli.doctor.parakeet.coreml.failed = parakeet check failed: { $error }

journal.cli.doctor.parakeet.coreml.missing = parakeet check failed: asset missing: { $file }

journal.cli.doctor.parakeet.coreml.missing.error = parakeet check failed: asset missing: { $file }: { $error }

journal.cli.doctor.parakeet.coreml.sentinel = parakeet check failed: sentinel not ready at { $path }

journal.cli.doctor.parakeet.coreml.size = parakeet check failed: asset size mismatch for { $file }: expected { $expected }, found { $found }

journal.cli.doctor.parakeet.coreml.unreadable = parakeet check failed: failed to read { $file }: { $error }

journal.cli.doctor.parakeet.cpp.executable = { $parakeet_cpp } check failed: { $name } not executable at { $path }

journal.cli.doctor.parakeet.cpp.missing = { $parakeet_cpp } check failed: { $name } missing at { $path }

journal.cli.doctor.parakeet.cpp.model = { $parakeet_cpp } check failed: model missing at { $path }

journal.cli.doctor.parakeet.cpp.pin = { $parakeet_cpp } pin lacks { $key }

journal.cli.doctor.parakeet.cpp.platform = { $parakeet_cpp } is unsupported on { $os }/{ $arch }

journal.cli.doctor.vad.contract = { $vad } helper ran but did not report closed-stdin usage contract (exit { $exit_code }): { $stderr }

journal.cli.doctor.vad.loader = { $vad } helper could not load { $library }

journal.cli.doctor.vad.missing = { $vad } helper binary is missing: { $path }

journal.cli.doctor.vad.ready = { $vad } helper launchable (closed-stdin usage contract)

journal.cli.doctor.vad.repair.install = reinstall the journal, then rerun { $doctor_command }

journal.cli.doctor.vad.repair.install.app = reinstall the journal app, then rerun { $doctor_command }

journal.cli.doctor.vad.repair.restart = restart the journal, then rerun { $doctor_command }; if it still times out, reinstall the journal

journal.cli.doctor.vad.repair.restart.app = restart the journal app, then rerun { $doctor_command }; if it still times out, reinstall the journal app

journal.cli.doctor.vad.spawn = { $vad } helper could not be executed: { $cause }

journal.cli.doctor.vad.timeout = { $vad } helper did not exit within { $timeout }

journal.cli.doctor.vad.unresolved = could not resolve the { $vad } helper path: { $cause }

journal.cli.doctor.sync.directory.required = flat-directory descendant is not a directory: { $path }

journal.cli.doctor.sync.entry.changed = entry changed during observation: { $reason }

journal.cli.doctor.sync.entry.invalid = invalid flat-directory entry { $entry }: { $reason }

journal.cli.doctor.sync.entry.replaced = changed or disappeared during inspection

journal.cli.doctor.sync.entry.unsafe = Installation: needs attention{ $newline }your journal contains an item that can't be checked safely.{ $newline }move the item named under details out of your journal's `{ $sync_folder }` folder, then try again.{ $newline }{ $newline }details:{ $newline }path: { $path }{ $newline }type: { $entry_type }{ $newline }reason: { $reason }

journal.cli.doctor.sync.entry.unsafe.bytes = Installation: needs attention{ $newline }your journal contains an item that can't be checked safely.{ $newline }move the item named under details out of your journal's `{ $sync_folder }` folder, then try again.{ $newline }{ $newline }details:{ $newline }path: { $path }{ $newline }type: { $entry_type }{ $newline }reason: { $reason }{ $newline }name bytes: { $bytes }

journal.cli.doctor.sync.entry.vanished = flat-directory entry vanished while listing: { $path }

journal.cli.doctor.sync.file.required = flat-directory entry is not a regular file: { $path }

journal.cli.doctor.sync.folder.full = Installation: needs attention{ $newline }your journal has too many items to check safely.{ $newline }review the items in your journal's `{ $sync_folder }` folder. move only items you know do not belong there until 256 or fewer remain, then try again.{ $newline }{ $newline }details:{ $newline }folder: { $folder }{ $newline }items found: more than 256{ $newline }maximum checked: 256

journal.cli.doctor.sync.folder.unsafe = Installation: needs attention{ $newline }part of your journal can't be checked safely.{ $newline }check the path named under details. it needs to be an ordinary folder inside your journal that you can open before you try again.{ $newline }{ $newline }details:{ $newline }path: { $path }{ $newline }reason: { $reason }

journal.cli.doctor.sync.heartbeat = Installation: needs attention{ $newline }a recent heartbeat from another run is present.{ $newline }wait a moment, then try again.

journal.cli.doctor.sync.identity.changed = flat-directory identity changed: { $path }

journal.cli.doctor.sync.io = { $operation } failed for { $path }: { $reason }

journal.cli.doctor.sync.kind.blockdevice = block device

journal.cli.doctor.sync.kind.characterdevice = character device

journal.cli.doctor.sync.kind.directory = directory

journal.cli.doctor.sync.kind.fifo = named pipe

journal.cli.doctor.sync.kind.other = other filesystem object

journal.cli.doctor.sync.kind.regularfile = regular file

journal.cli.doctor.sync.kind.socket = Unix socket

journal.cli.doctor.sync.kind.symlink = symbolic link

journal.cli.doctor.sync.path.invalid = invalid flat-directory path { $path }: { $reason }

journal.cli.doctor.sync.reason.disappeared = entry disappeared during observation

journal.cli.doctor.sync.reason.nonregular = not a regular file ({ $kind })

journal.cli.doctor.sync.reason.oversized = regular file is { $size } bytes, limit is { $limit }

journal.cli.doctor.sync.reason.read = could not read observed file

journal.cli.doctor.sync.size = flat-directory entry exceeds observed-read limit: { $path } is { $kind }, { $size } bytes exceeds { $limit }

journal.cli.doctor.sync.symlink = flat-directory descendant is a symlink: { $path }

journal.cli.doctor.sync.unsupported = Installation: needs attention{ $newline }sync rescan capability is unavailable on this platform.

journal.cli.doctor.sync.unverifiable = Installation: needs attention{ $newline }startup status couldn't be verified.{ $newline }wait a moment, then try again.

journal.cli.doctor.sync.waiting = Installation: waiting{ $newline }a recent heartbeat from another run was found.{ $newline }the { $brand } app is waiting while it checks whether that activity continues. it will check again shortly.

journal.cli.doctor.package.executable.parent = executable has no parent

journal.cli.doctor.package.layout = could not locate packaged talent roots from executable directory { $directory }{ $newline }walked ancestor { $ancestor }{ $newline }site-packages candidate: { $site }{ $newline }checkout candidate ({ $pyproject } + { $git_dir }): { $checkout }{ $newline }layout share anchors { $bundle }, { $layout }, { $template }: { $install }{ $newline }Apple bundle { $journal_payload } candidate: { $embedded }

journal.cli.doctor.package.missing = installation root { $root } is missing directory { $path }

journal.cli.doctor.package.none = none

journal.cli.doctor.package.target = { $newline }Cargo target directory { $target } is outside any checkout: a binary built there cannot find the payload, so keep { $target_env } inside the checkout

journal.cli.doctor.package.unavailable = could not locate packaged talent roots from executable directory { $directory }

journal.cli.doctor.speakers.installation.app = { $speakers_name } installation is incomplete ({ $detail }). Repair: reinstall the journal app.

journal.cli.doctor.speakers.installation.journal = { $speakers_name } installation is incomplete ({ $detail }). Repair: reinstall the journal.

journal.cli.doctor.speakers.reinstall.app = reinstall the journal app

journal.cli.doctor.windows.parakeet.executable = could not determine the running journal executable: { $error }

journal.cli.doctor.windows.parakeet.layout = running journal executable is not in the package bin directory: { $path }

journal.cli.doctor.windows.parakeet.member = signed { $parakeet_name } app payload refuses { $member }: { $error }

journal.cli.doctor.windows.parakeet.parent = running journal executable has no containing directory: { $path }

journal.cli.doctor.windows.parakeet.platform = Windows { $parakeet_name } package verification requires a Windows runtime

journal.cli.doctor.windows.parakeet.root = package bin directory has no package root: { $path }

journal.cli.doctor.windows.parakeet.verification = could not verify the signed { $parakeet_name } app payload: { $error }

journal.cli.doctor.storage.inspection.log = storage privacy: { $error }

journal.cli.doctor.windows.onnx.member = signed { $onnx } app payload refuses { $member }: { $error }

journal.cli.doctor.windows.onnx.platform = { $windows } { $onnx } package verification requires a { $windows } runtime

journal.cli.doctor.windows.onnx.verification = could not verify the signed { $onnx } app payload: { $error }

journal.cli.doctor.vad.executable = could not determine current executable for { $vad } helper: { $error }

journal.cli.doctor.vad.layout = { $vad } helper lookup is outside the running signed app payload

journal.cli.doctor.vad.parent = current executable has no parent directory for { $vad } helper lookup

journal.cli.doctor.vad.probe.log = { $vad } helper probe failed: { $error }

journal.cli.doctor.windows.onnx.helper = { $onnx } helper is not the declared member of the signed app payload

journal.cli.doctor.search.log.finished = last finished search check could not be read: { $error }

journal.cli.doctor.search.log.unavailable = search index status unavailable: { $error }

journal.cli.doctor.search.log.work = search work list could not be read: { $error }

journal.cli.about.os.release.unavailable = about: { $release_file } unavailable: { $error }

journal.cli.about.resource.unavailable = the journal's about information couldn't be read. check that your journal is running, then try again.

journal.cli.about.usage.journal = usage: { $syntax }

journal.cli.about.usage.remote = usage: { $syntax }

journal.cli.client.error.malformed = the journal response couldn't be read.

journal.cli.client.error.timeout = The journal didn't answer in time.

journal.cli.client.error.unreachable = your journal couldn't be reached over { $http }.

journal.cli.client.error.unreadable = The journal returned an unreadable error.

journal.cli.client.port.invalid = { $variable } must be an integer from 1 to 65535.

journal.cli.describe.usage.argument.unknown = usage: { $syntax }{ $line_break }{ $command }: error: unknown argument: { $argument }

journal.cli.describe.usage.backfill.arguments = usage: { $syntax }{ $line_break }{ $command }: error: face backfill requires { $arguments }

journal.cli.describe.usage.backfill.budget = usage: { $syntax }{ $line_break }{ $command }: error: face backfill budget is outside the allowed range

journal.cli.describe.usage.backfill.desktop = usage: { $syntax }{ $line_break }{ $command }: error: face backfill is available on desktop journals only

journal.cli.describe.usage.describe.repeated = usage: { $syntax }{ $line_break }{ $command }: error: { $describe_flag } was provided more than once

journal.cli.describe.usage.file.extra = usage: { $syntax }{ $line_break }{ $command }: error: only one video path may be provided

journal.cli.describe.usage.file.required = usage: { $syntax }{ $line_break }{ $command }: error: the following arguments are required: { $file }

journal.cli.describe.usage.frames.repeated = usage: { $syntax }{ $line_break }{ $command }: error: { $frames_only_flag } was provided more than once

journal.cli.describe.usage.help = usage: { $syntax }

journal.cli.describe.usage.help.extra = usage: { $syntax }{ $line_break }{ $command }: error: { $help_flag } does not accept other arguments

journal.cli.describe.usage.jobs.positive = usage: { $syntax }{ $line_break }{ $command }: error: { $jobs_flag } requires a positive integer

journal.cli.describe.usage.jobs.repeated = usage: { $syntax }{ $line_break }{ $command }: error: { $jobs_flag } was provided more than once

journal.cli.describe.usage.journal.path = usage: { $syntax }{ $line_break }{ $command }: error: { $journal_flag } requires a path

journal.cli.describe.usage.journal.repeated = usage: { $syntax }{ $line_break }{ $command }: error: { $journal_flag } was provided more than once

journal.cli.describe.usage.mode.required = usage: { $syntax }{ $line_break }{ $command }: error: exactly one of { $frames_only_flag } or { $describe_flag } is required

journal.cli.describe.usage.redo.mode = usage: { $syntax }{ $line_break }{ $command }: error: { $redo_flag } requires { $describe_flag }

journal.cli.describe.usage.redo.repeated = usage: { $syntax }{ $line_break }{ $command }: error: { $redo_flag } was provided more than once

journal.cli.describe.usage.registry.extra = usage: { $syntax }{ $line_break }{ $command }: error: { $category_registry_flag } does not accept other arguments

journal.cli.describe.usage.version.extra = usage: { $syntax }{ $line_break }{ $command }: error: { $version_flag } does not accept other arguments
