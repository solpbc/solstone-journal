journal.cli.about.os.release.unavailable = about: { $release_file } nicht verfügbar: { $error }

journal.cli.about.resource.unavailable = die about-Informationen des Journals konnten nicht gelesen werden. prüfe, ob dein Journal läuft, dann versuch es noch einmal.

journal.cli.about.usage.journal = Verwendung: { $syntax }

journal.cli.about.usage.remote = Verwendung: { $syntax }

journal.cli.check.disk.error = freier Speicherplatz bei { $path } konnte nicht überprüft werden: { $message }

journal.cli.check.disk.free = { $gb } GB frei bei { $path } (20 GB nötig)

journal.cli.check.disk.low = das Journal und lokale Modelle brauchen mindestens 20 GB frei — { $path } hat { $gb } GB

journal.cli.check.disk.overflow = Überlauf des freien Speicherplatzes

journal.cli.check.feedback = Denkst du, dass diese Anzeige für deinen Computer falsch ist? Wir hätten gern einen Patch — { $url }

journal.cli.check.gpu.absent = keine nutzbare GPU gefunden — die mitgelieferten lokalen Modelle brauchen eine Hardware-GPU

journal.cli.check.gpu.absent.linux = keine nutzbare GPU gefunden — die mitgelieferten lokalen Modelle brauchen eine Hardware-GPU mit mindestens 6 GB

journal.cli.check.gpu.cuda.integrity = Integritätsfehler der CUDA-Laufzeitumgebung

journal.cli.check.gpu.low.linux = GPU { $gpu } hat { $gb } GB — die mitgelieferten lokalen Modelle brauchen mindestens 6 GB

journal.cli.check.gpu.low.windows = GPU { $gpu } hat { $gb } GB; die mitgelieferten lokalen Modelle brauchen mindestens 6 GB

journal.cli.check.gpu.nvidia = Nvidia-GPU mit { $gb } GB

journal.cli.check.gpu.nvidia.cpu = Nvidia-GPU mit { $gb } GB; ein Modell läuft auf deiner GPU; die Transkription läuft auf diesem Computer auf deiner CPU

journal.cli.check.gpu.nvidia.low = die Nvidia-GPU hat { $gb } GB — die mitgelieferten lokalen Modelle brauchen mindestens 6 GB

journal.cli.check.gpu.nvidia.unknown = Nvidia-GPU erkannt, aber ihr Speicher konnte nicht gelesen werden — die GPU-Bereitschaft ist unbekannt

journal.cli.check.gpu.readiness.unknown = die GPU-Bereitschaft konnte nicht bestimmt werden: { $error }

journal.cli.check.gpu.render.permission = ein GPU-Render-Knoten liegt unter { $path }, aber dieses Konto kann ihn nicht öffnen — füge dich mit `{ $repair }` zur Gruppe render hinzu, melde dich dann ab und wieder an und führe `{ $command }` erneut aus

journal.cli.check.gpu.unified = Nvidia-GPU mit { $gb } GB (gemeinsamer Speicher)

journal.cli.check.gpu.unified.cpu = Nvidia-GPU mit { $gb } GB (gemeinsamer Speicher); ein Modell läuft auf deiner GPU; die Transkription läuft auf diesem Computer auf deiner CPU

journal.cli.check.gpu.unknown = GPU-Speicher konnte nicht gelesen werden. lokales Denken braucht mindestens 6 GB

journal.cli.check.gpu.vulkan = Vulkan-GPU { $gpu } mit { $gb } GB

journal.cli.check.gpu.vulkan.cpu = Vulkan-GPU { $gpu } mit { $gb } GB; ein Modell läuft auf deiner GPU; die Transkription läuft auf diesem Computer auf deiner CPU

journal.cli.check.gpu.vulkan.incomplete = keine Nvidia-GPU gefunden, und die Vulkan-Sonde wurde nicht abgeschlossen — die GPU-Bereitschaft ist unbekannt

journal.cli.check.label.ced = ced

journal.cli.check.label.disk = Datenträger

journal.cli.check.label.gpu = GPU

journal.cli.check.label.memory = Arbeitsspeicher

journal.cli.check.label.platform = Plattform

journal.cli.check.label.ram = RAM

journal.cli.check.label.rfdetr = rfdetr

journal.cli.check.marker.blocked = [blockiert]

journal.cli.check.marker.ok = [ok]

journal.cli.check.marker.unknown = [unbekannt]

journal.cli.check.marker.warn = [Warnung]

journal.cli.check.memory.available = genug Arbeitsspeicher insgesamt, aber gerade sind nur { $gb } GB frei — schließe einige Apps, bevor die lokalen Modelle laden

journal.cli.check.memory.eligible = { $gb } GB Arbeitsspeicher erfüllt das Minimum von 16 GB

journal.cli.check.memory.minimum = Apple Silicon braucht mindestens 16 GB Arbeitsspeicher für die mitgelieferten lokalen Modelle (dieser Mac hat { $gb } GB)

journal.cli.check.memory.unknown = verfügbarer Arbeitsspeicher konnte nicht überprüft werden

journal.cli.check.overall.blocked = Nicht bereit — dieser Computer kann die mitgelieferten lokalen Modelle noch nicht ausführen.

journal.cli.check.overall.ready = Bereit — installiere als Nächstes das Journal:  siehe { $file }

journal.cli.check.overall.warning = Weitgehend bereit (siehe die Warnungen oben) — du kannst das Journal installieren:  siehe { $file }

journal.cli.check.platform.apple = Apple Silicon macOS (arm64)

journal.cli.check.platform.intel = Intel-Macs werden nicht unterstützt — nimm einen Apple-Silicon-Mac oder führe das Journal auf unterstütztem Linux aus.

journal.cli.check.platform.linux = Linux ({ $arch })

journal.cli.check.platform.unsupported = { $os }/{ $arch } kann die mitgelieferten lokalen Modelle noch nicht ausführen. unterstützte Plattformen sind Apple Silicon macOS, x86_64/aarch64 Linux und x86_64/aarch64 Windows.

journal.cli.check.platform.windows = Windows ({ $arch })

journal.cli.check.ram.low = { $gb } GB Arbeitsspeicher ist eher knapp — 8 GB oder mehr werden empfohlen

journal.cli.check.ram.total = { $gb } GB Arbeitsspeicher

journal.cli.check.ram.unknown = Arbeitsspeicher konnte nicht überprüft werden

journal.cli.check.title = { $command } — kann dieser Computer das Journal mit den mitgelieferten lokalen Modellen ausführen?

journal.cli.doctor.backlog.band = { $recording_count ->
    [one] Band { $band }: { $recordings } Aufnahme bereit / { $recordings_left } übrig, { $outputs } tägliche { $output_count ->
        [one] Ausgabe
        *[other] Ausgaben
    } bereit / { $outputs_left } übrig, { $activities } { $activity_count ->
        [one] Aktivität
        *[other] Aktivitäten
    } übrig, { $requests } { $request_count ->
        [one] Anfrage von dir
        *[other] Anfragen von dir
    }, { $permission } wartet auf Berechtigung, { $prerequisites } wartet auf Voraussetzungen
    *[other] Band { $band }: { $recordings } Aufnahmen bereit / { $recordings_left } übrig, { $outputs } tägliche { $output_count ->
        [one] Ausgabe
        *[other] Ausgaben
    } bereit / { $outputs_left } übrig, { $activities } { $activity_count ->
        [one] Aktivität
        *[other] Aktivitäten
    } übrig, { $requests } { $request_count ->
        [one] Anfrage von dir
        *[other] Anfragen von dir
    }, { $permission } wartet auf Berechtigung, { $prerequisites } wartet auf Voraussetzungen
}

journal.cli.doctor.backlog.caught = auf dem neuesten Stand

journal.cli.doctor.backlog.caught.capped = { $day_count ->
    [one] auf dem neuesten Stand; { $days } Tag abgeschlossen mit { $unit_count ->
        [one] einer gedeckelten Tageseinheit
        *[other] gedeckelten Tageseinheiten
    }
    *[other] auf dem neuesten Stand; { $days } Tage abgeschlossen mit { $unit_count ->
        [one] einer gedeckelten Tageseinheit
        *[other] gedeckelten Tageseinheiten
    }
}

journal.cli.doctor.backlog.change.recorded = { $name } hat auf { $location } eine Änderung aufgezeichnet; löse die aufgezeichnete Änderung auf, bevor dieser Tag neu gemacht wird

journal.cli.doctor.backlog.change.started = { $name } hat auf { $location } vielleicht eine Änderung begonnen, den Abschluss aber nicht bestätigt; löse die laufende Änderung auf, bevor neu verarbeitet wird

journal.cli.doctor.backlog.checked = jüngste Arbeit geprüft

journal.cli.doctor.backlog.checked.capped = { $day_count ->
    [one] jüngste Arbeit geprüft; { $days } Tag abgeschlossen mit { $unit_count ->
        [one] einer gedeckelten Tageseinheit
        *[other] gedeckelten Tageseinheiten
    }
    *[other] jüngste Arbeit geprüft; { $days } Tage abgeschlossen mit { $unit_count ->
        [one] einer gedeckelten Tageseinheit
        *[other] gedeckelten Tageseinheiten
    }
}

journal.cli.doctor.backlog.deferred = { $name } hat bei { $location } gestoppt; es kann es bei einem späteren Lauf erneut versuchen, nachdem sich der lokale Tag ändert

journal.cli.doctor.backlog.deferred.reason = { $name } hat bei { $location } mit dem Grund { $reason } gestoppt; es kann es bei einem späteren Lauf erneut versuchen, nachdem sich der lokale Tag ändert

journal.cli.doctor.backlog.eligible = { $name } hat bei { $location } gestoppt; es kann es beim nächsten infrage kommenden Lauf erneut versuchen

journal.cli.doctor.backlog.eligible.reason = { $name } hat bei { $location } mit dem Grund { $reason } gestoppt; es kann es beim nächsten infrage kommenden Lauf erneut versuchen

journal.cli.doctor.backlog.empty.date = nichts

journal.cli.doctor.backlog.exhausted = { $name } hat bei { $location } nach dem automatischen erneuten Versuch gestoppt; führe { $command } aus

journal.cli.doctor.backlog.exhausted.reason = { $name } hat bei { $location } mit dem Grund { $reason } nach dem automatischen erneuten Versuch gestoppt; führe { $command } aus

journal.cli.doctor.backlog.hold.retry = { $day }: { $reason } (erneuter Versuch { $unit })

journal.cli.doctor.backlog.inventory.partial.current = { $quantity ->
    [one] untersucht { $inspected } von { $total } Tag, { $oldest } bis { $newest }, Stand { $as_of }; { $bands }; unvollständige Prüfung; beiseitegelegt: { $holds }
    *[other] untersucht { $inspected } von { $total } Tagen, { $oldest } bis { $newest }, Stand { $as_of }; { $bands }; unvollständige Prüfung; beiseitegelegt: { $holds }
}

journal.cli.doctor.backlog.inventory.partial.current.clear = { $quantity ->
    [one] untersucht { $inspected } von { $total } Tag, { $oldest } bis { $newest }, Stand { $as_of }; { $bands }; unvollständige Prüfung
    *[other] untersucht { $inspected } von { $total } Tagen, { $oldest } bis { $newest }, Stand { $as_of }; { $bands }; unvollständige Prüfung
}

journal.cli.doctor.backlog.inventory.partial.last = { $quantity ->
    [one] untersucht { $inspected } von { $total } Tag, { $oldest } bis { $newest }, Stand { $as_of }; { $bands }; unvollständige Prüfung; letzter bekannter Fortschritt; beiseitegelegt: { $holds }
    *[other] untersucht { $inspected } von { $total } Tagen, { $oldest } bis { $newest }, Stand { $as_of }; { $bands }; unvollständige Prüfung; letzter bekannter Fortschritt; beiseitegelegt: { $holds }
}

journal.cli.doctor.backlog.inventory.partial.last.clear = { $quantity ->
    [one] untersucht { $inspected } von { $total } Tag, { $oldest } bis { $newest }, Stand { $as_of }; { $bands }; unvollständige Prüfung; letzter bekannter Fortschritt
    *[other] untersucht { $inspected } von { $total } Tagen, { $oldest } bis { $newest }, Stand { $as_of }; { $bands }; unvollständige Prüfung; letzter bekannter Fortschritt
}

journal.cli.doctor.backlog.inventory.whole.current = { $quantity ->
    [one] untersucht { $inspected } von { $total } Tag, { $oldest } bis { $newest }, Stand { $as_of }; { $bands }; Prüfung des ganzen Journals; beiseitegelegt: { $holds }
    *[other] untersucht { $inspected } von { $total } Tagen, { $oldest } bis { $newest }, Stand { $as_of }; { $bands }; Prüfung des ganzen Journals; beiseitegelegt: { $holds }
}

journal.cli.doctor.backlog.inventory.whole.current.clear = { $quantity ->
    [one] untersucht { $inspected } von { $total } Tag, { $oldest } bis { $newest }, Stand { $as_of }; { $bands }; Prüfung des ganzen Journals
    *[other] untersucht { $inspected } von { $total } Tagen, { $oldest } bis { $newest }, Stand { $as_of }; { $bands }; Prüfung des ganzen Journals
}

journal.cli.doctor.backlog.inventory.whole.last = { $quantity ->
    [one] untersucht { $inspected } von { $total } Tag, { $oldest } bis { $newest }, Stand { $as_of }; { $bands }; Prüfung des ganzen Journals; letzter bekannter Fortschritt; beiseitegelegt: { $holds }
    *[other] untersucht { $inspected } von { $total } Tagen, { $oldest } bis { $newest }, Stand { $as_of }; { $bands }; Prüfung des ganzen Journals; letzter bekannter Fortschritt; beiseitegelegt: { $holds }
}

journal.cli.doctor.backlog.inventory.whole.last.clear = { $quantity ->
    [one] untersucht { $inspected } von { $total } Tag, { $oldest } bis { $newest }, Stand { $as_of }; { $bands }; Prüfung des ganzen Journals; letzter bekannter Fortschritt
    *[other] untersucht { $inspected } von { $total } Tagen, { $oldest } bis { $newest }, Stand { $as_of }; { $bands }; Prüfung des ganzen Journals; letzter bekannter Fortschritt
}

journal.cli.doctor.backlog.pending = ausstehend: { $days }

journal.cli.doctor.backlog.pending.counts = { $pending_count ->
    [one] { $pending } Tag ausstehend, { $stuck } { $stuck_count ->
        [one] Tag hängt fest
        *[other] Tage hängen fest
    }
    *[other] { $pending } Tage ausstehend, { $stuck } { $stuck_count ->
        [one] Tag hängt fest
        *[other] Tage hängen fest
    }
}

journal.cli.doctor.backlog.pending.oldest = { $pending_count ->
    [one] { $pending } Tag ausstehend, { $stuck } { $stuck_count ->
        [one] Tag hängt fest
        *[other] Tage hängen fest
    }; ältester offener Tag { $day }
    *[other] { $pending } Tage ausstehend, { $stuck } { $stuck_count ->
        [one] Tag hängt fest
        *[other] Tage hängen fest
    }; ältester offener Tag { $day }
}

journal.cli.doctor.backlog.resolve = { $name } hat bei { $location } gestoppt; löse den Konflikt auf, bevor ein vergangener Tag neu verarbeitet wird

journal.cli.doctor.backlog.resolve.reason = { $name } hat bei { $location } mit dem Grund { $reason } gestoppt; löse den Konflikt auf, bevor ein vergangener Tag neu verarbeitet wird

journal.cli.doctor.backlog.retry = { $name } hat bei { $location } gestoppt; führe { $command } aus

journal.cli.doctor.backlog.retry.reason = { $name } hat bei { $location } mit dem Grund { $reason } gestoppt; führe { $command } aus

journal.cli.doctor.backlog.review.started = { $name } hat auf { $location } vielleicht eine Änderung der Entitätsprüfung begonnen, den Abschluss aber nicht bestätigt; löse die laufende Änderung auf, bevor neu verarbeitet wird

journal.cli.doctor.backlog.stuck = hängt fest: { $days }

journal.cli.doctor.backlog.unfinished = { $activity_count ->
    [one] { $activities } Aktivität an { $days } { $day_count ->
        [one] abgeschlossenem Tag
        *[other] abgeschlossenen Tagen
    } konnte die Verarbeitung nicht abschließen
    *[other] { $activities } Aktivitäten an { $days } { $day_count ->
        [one] abgeschlossenem Tag
        *[other] abgeschlossenen Tagen
    } konnten die Verarbeitung nicht abschließen
}

journal.cli.doctor.backlog.unknown = unbekannt: { $days }

journal.cli.doctor.backlog.unknown.count = { $quantity ->
    [one] das ließ sich nicht vollständig bestimmen; { $days } Tag unbekannt
    *[other] das ließ sich nicht vollständig bestimmen; { $days } Tage unbekannt
}

journal.cli.doctor.backup.association = { $association ->
    [unbound] Zuordnung der Ersteinrichtung unbekannt; nur Verlauf
    [unknown] aktuelle Zuordnung der Ersteinrichtung nicht verfügbar
    [superseded] überholte Ersteinrichtung; nur Verlauf
    [current] stimmt mit der ausgewählten Ersteinrichtung überein
    *[invalid] aufgezeichnete Zuordnung der Ersteinrichtung nicht verfügbar
}

journal.cli.doctor.backup.attempt.incomplete = { $outcome ->
    [success] Sicherung an; letzter aufgezeichneter Versuch: erfolgreiche Kopie am { $date }; einige Dateien wurden nicht kopiert; { $history }; { $association }
    [incomplete] Sicherung an; letzter aufgezeichneter Versuch: unvollständige Kopie am { $date }; einige Dateien wurden nicht kopiert; { $history }; { $association }
    *[failed] Sicherung an; letzter aufgezeichneter Versuch: fehlgeschlagen am { $date }; einige Dateien wurden nicht kopiert; { $history }; { $association }
}

journal.cli.doctor.backup.attempt.locked = { $outcome ->
    [success] Sicherung an; letzter aufgezeichneter Versuch: erfolgreiche Kopie am { $date }; das Repository war gesperrt; { $history }; { $association }
    [incomplete] Sicherung an; letzter aufgezeichneter Versuch: unvollständige Kopie am { $date }; das Repository war gesperrt; { $history }; { $association }
    *[failed] Sicherung an; letzter aufgezeichneter Versuch: fehlgeschlagen am { $date }; das Repository war gesperrt; { $history }; { $association }
}

journal.cli.doctor.backup.attempt.ordinary = { $outcome ->
    [success] Sicherung an; letzter aufgezeichneter Versuch: erfolgreiche Kopie am { $date }; { $history }; { $association }
    [incomplete] Sicherung an; letzter aufgezeichneter Versuch: unvollständige Kopie am { $date }; { $history }; { $association }
    *[failed] Sicherung an; letzter aufgezeichneter Versuch: fehlgeschlagen am { $date }; { $history }; { $association }
}

journal.cli.doctor.backup.attempt.refused = { $outcome ->
    [success] Sicherung an; letzter aufgezeichneter Versuch: erfolgreiche Kopie am { $date }; Sicherungsschlüssel abgelehnt; { $history }; { $association }
    [incomplete] Sicherung an; letzter aufgezeichneter Versuch: unvollständige Kopie am { $date }; Sicherungsschlüssel abgelehnt; { $history }; { $association }
    *[failed] Sicherung an; letzter aufgezeichneter Versuch: fehlgeschlagen am { $date }; Sicherungsschlüssel abgelehnt; { $history }; { $association }
}

journal.cli.doctor.backup.attempt.timeout = { $outcome ->
    [success] Sicherung an; letzter aufgezeichneter Versuch: erfolgreiche Kopie am { $date }; Zeitüberschreitung; { $history }; { $association }
    [incomplete] Sicherung an; letzter aufgezeichneter Versuch: unvollständige Kopie am { $date }; Zeitüberschreitung; { $history }; { $association }
    *[failed] Sicherung an; letzter aufgezeichneter Versuch: fehlgeschlagen am { $date }; Zeitüberschreitung; { $history }; { $association }
}

journal.cli.doctor.backup.attempt.unknown = { $outcome ->
    [success] Sicherung an; letzter aufgezeichneter Versuch: erfolgreiche Kopie am { $date }; Ursache unbekannt; { $history }; { $association }
    [incomplete] Sicherung an; letzter aufgezeichneter Versuch: unvollständige Kopie am { $date }; Ursache unbekannt; { $history }; { $association }
    *[failed] Sicherung an; letzter aufgezeichneter Versuch: fehlgeschlagen am { $date }; Ursache unbekannt; { $history }; { $association }
}

journal.cli.doctor.backup.date.unknown = unbekanntes Datum

journal.cli.doctor.backup.details = öffne Sicherung oder Zustand in deinem Journal für Details

journal.cli.doctor.backup.history.incomplete.dated = { $quantity ->
    [one] letzte erfolgreiche Kopie (historisch): Abbild { $snapshot } am { $date }, Alter { $seconds } Sekunde; unvollständige Kopie, einige Dateien wurden nicht kopiert; aktuelle Abdeckung und Überprüfung unbekannt
    *[other] letzte erfolgreiche Kopie (historisch): Abbild { $snapshot } am { $date }, Alter { $seconds } Sekunden; unvollständige Kopie, einige Dateien wurden nicht kopiert; aktuelle Abdeckung und Überprüfung unbekannt
}

journal.cli.doctor.backup.history.incomplete.undated = letzte erfolgreiche Kopie (historisch): Abbild { $snapshot } am { $date }, Alter unbekannt; unvollständige Kopie, einige Dateien wurden nicht kopiert; aktuelle Abdeckung und Überprüfung unbekannt

journal.cli.doctor.backup.history.success.dated = { $quantity ->
    [one] letzte erfolgreiche Kopie (historisch): Abbild { $snapshot } am { $date }, Alter { $seconds } Sekunde; aktuelle Abdeckung und Überprüfung unbekannt
    *[other] letzte erfolgreiche Kopie (historisch): Abbild { $snapshot } am { $date }, Alter { $seconds } Sekunden; aktuelle Abdeckung und Überprüfung unbekannt
}

journal.cli.doctor.backup.history.success.undated = letzte erfolgreiche Kopie (historisch): Abbild { $snapshot } am { $date }, Alter unbekannt; aktuelle Abdeckung und Überprüfung unbekannt

journal.cli.doctor.backup.history.unknown = letzte erfolgreiche Kopie unbekannt

journal.cli.doctor.backup.invalid.history = Sicherung an; aufgezeichneter Status nicht verfügbar; { $history }

journal.cli.doctor.backup.paused.history = Sicherung von dir pausiert seit { $date }; { $history }

journal.cli.doctor.backup.snapshot.unknown = unbekannt

journal.cli.doctor.backup.status.invalid.pause = Sicherungsstatus nicht verfügbar: ungültige Pause-Einstellung

journal.cli.doctor.backup.status.invalid.settings = Sicherungsstatus nicht verfügbar: ungültige Sicherungseinstellungen

journal.cli.doctor.backup.status.invalid.switch = Sicherungsstatus nicht verfügbar: ungültige an/aus-Einstellung

journal.cli.doctor.backup.status.no.attempt = Sicherung an; kein aufgezeichneter Versuch

journal.cli.doctor.backup.status.off = Sicherung aus

journal.cli.doctor.backup.status.unavailable.journal = Sicherungsstatus nicht verfügbar: Journal nicht lesbar

journal.cli.doctor.backup.status.unavailable.settings = Sicherungsstatus nicht verfügbar: Einstellungen nicht lesbar

journal.cli.doctor.backup.status.unconfigured = Sicherung nicht konfiguriert

journal.cli.doctor.brain.age.unknown = unbekannt

journal.cli.doctor.brain.no.component = nichts

journal.cli.doctor.brain.report = { $headline }; { $state_key }={ $state }; { $reason_key }={ $reason }; { $component_key }={ $component }; { $age_key }={ $age }

journal.cli.doctor.brain.unknown = unbekannt: brain-Zustand nicht verfügbar

journal.cli.doctor.brain.unknown.reason = unbekannt: { $reason }

journal.cli.doctor.capture.health.active = { $rollup }; die { $brand }-App auf jedem Gerät, das zu deinem Journal hinzugefügt hat, ist aktuell

journal.cli.doctor.capture.health.data.unavailable = { $rollup }; Geräteaktivität nicht verfügbar

journal.cli.doctor.capture.health.inspect = öffne { $health_path }, um jedes Gerät zu untersuchen

journal.cli.doctor.capture.health.no.senders = { $rollup }; die { $brand }-App hat deinem Journal noch nichts hinzugefügt

journal.cli.doctor.capture.health.quiet = { $rollup }; keine abgelehnten Uploads aufgezeichnet; siehe Geräte für die letzten Zustellungszeiten

journal.cli.doctor.capture.health.registry.unavailable = { $rollup }; Geräteliste nicht verfügbar

journal.cli.doctor.capture.recent = die { $brand }-App auf { $client } hat zuletzt vor { $hours } h hinzugefügt

journal.cli.doctor.capture.recent.sources = { $quantity ->
    [one] die { $brand }-App auf { $client } hat zuletzt vor { $hours } h hinzugefügt ({ $sources } braucht Aufmerksamkeit)
    *[other] die { $brand }-App auf { $client } hat zuletzt vor { $hours } h hinzugefügt ({ $sources } brauchen Aufmerksamkeit)
}

journal.cli.doctor.capture.rollup = { $rollup }; { $devices }

journal.cli.doctor.capture.sources = die { $brand }-App auf { $client } hat Schwierigkeiten, { $sources } hinzuzufügen

journal.cli.doctor.capture.trouble = die { $brand }-App auf { $client } hat Schwierigkeiten beim Hinzufügen

journal.cli.doctor.capture.trouble.sources = { $quantity ->
    [one] die { $brand }-App auf { $client } hat Schwierigkeiten beim Hinzufügen ({ $sources } braucht Aufmerksamkeit)
    *[other] die { $brand }-App auf { $client } hat Schwierigkeiten beim Hinzufügen ({ $sources } brauchen Aufmerksamkeit)
}

journal.cli.doctor.check.deferred = zurückgestellter Prüfsatz: { $set }

journal.cli.doctor.client.binding.active = aktive Client-Datensätze={ $records }; ungebunden=0; { $certificate_term }-Bindung ist maßgeblich

journal.cli.doctor.client.delivery.stall.activity.unavailable = Geräteaktivität nicht verfügbar

journal.cli.doctor.client.delivery.stall.inspect = öffne { $health_path }, um den abgelehnten Upload zu untersuchen

journal.cli.doctor.client.delivery.stall.list.unavailable = Geräteliste nicht verfügbar

journal.cli.doctor.client.delivery.stall.no.senders = die { $brand }-App hat deinem Journal noch nichts hinzugefügt

journal.cli.doctor.client.delivery.stall.quiet = keine abgelehnten Uploads aufgezeichnet; siehe Geräte für die letzten Zustellungszeiten

journal.cli.doctor.client.ingest.health.healthy = keine Geräte, bei denen die Annahme fehlschlägt

journal.cli.doctor.client.ingest.health.rejection = Gerät { $device }, Annahme schlägt fehl: { $reason }, { $attempts }x seit { $since }

journal.cli.doctor.client.ingest.health.repair = aktualisiere das Gerät oder starte es neu, und bestätige dann, dass ein gültiger Upload die Ablehnung aufhebt

journal.cli.doctor.client.transport.refusal.healthy = bei keinem Gerät wurden in den letzten 7 Tagen Anfragen abgewiesen

journal.cli.doctor.client.transport.refusal.repair = das ist eine ausgelastete Verbindung, keine verlorene; melde es, wenn es weiter vorkommt

journal.cli.doctor.clients.ledger.unavailable = autorisierte Client-Liste nicht verfügbar: { $reason }

journal.cli.doctor.config.dir.readable.config.unwritable = das Konfigurationsverzeichnis des Dienstes ist nicht beschreibbar: { $path }

journal.cli.doctor.config.dir.readable.config.will.create = Home ist beschreibbar; die Installation legt { $path } an

journal.cli.doctor.config.dir.readable.config.writable = Home und das Konfigurationsverzeichnis des Dienstes sind beschreibbar ({ $path })

journal.cli.doctor.config.dir.readable.home.missing = Home-Verzeichnis existiert nicht: { $path }

journal.cli.doctor.config.dir.readable.home.unusable = Home-Verzeichnis ist nicht les- und beschreibbar: { $path }

journal.cli.doctor.config.dir.readable.owner.unavailable = die Owner-Basis konnte nicht gefunden werden: { $error }

journal.cli.doctor.config.dir.readable.owner.windows = die Windows-Owner-Basis ist nur unter Windows lesbar

journal.cli.doctor.config.restore = repariere { $config_file } oder stelle sie aus einer Sicherung wieder her

journal.cli.doctor.context.failed = doctor konnte den Kontext nicht auflösen: { $error }

journal.cli.doctor.default.stt.ready.backend = konfiguriertes backend ist { $backend }; parakeet-Bereitschaft nicht anwendbar

journal.cli.doctor.default.stt.ready.download = das { $coreml }-parakeet-Modell ist nicht heruntergeladen — hol es mit: { $models_command }

journal.cli.doctor.default.stt.ready.other.model.ready = parakeet-Modell bereit bei { $path }

journal.cli.doctor.default.stt.ready.unsupported = parakeet wird auf dieser Plattform nicht unterstützt

journal.cli.doctor.delivery.rejected = das Journal hat einen Upload von { $client } abgelehnt

journal.cli.doctor.delivery.rejected.sources = { $quantity ->
    [one] das Journal hat einen Upload von { $client } abgelehnt ({ $sources } braucht Aufmerksamkeit)
    *[other] das Journal hat einen Upload von { $client } abgelehnt ({ $sources } brauchen Aufmerksamkeit)
}

journal.cli.doctor.device.day.listing.chronicle.error = Gerätedatenströme nicht verfügbar: Chronik nicht lesbar: { $error }

journal.cli.doctor.device.day.listing.continuity.error = Gerätedatenströme nicht verfügbar: Datenstrom-Kontinuität nicht lesbar: { $error }

journal.cli.doctor.device.day.listing.continuity.malformed = Gerätedatenströme nicht verfügbar: Datenstrom-Kontinuität nicht lesbar: der Kontinuitätsdatensatz ist fehlerhaft oder wird nicht unterstützt

journal.cli.doctor.device.day.listing.fault = Tag { $day } Datenstrom { $stream } Gerät { $device }: { $reason }

journal.cli.doctor.device.day.listing.readable = das Journal kann die letzten { $days } Tage für jedes gebundene Gerät lesen

journal.cli.doctor.device.day.listing.record.line = Gerätedatenströme nicht verfügbar: Datenstrom-Registry nicht lesbar: fehlerhafter Datenstrom-Datensatz { $record }: fehlerhafte Daten in { $path } in Zeile { $line }

journal.cli.doctor.device.day.listing.record.malformed = Gerätedatenströme nicht verfügbar: Datenstrom-Registry nicht lesbar: fehlerhafter Datenstrom-Datensatz { $record }: fehlerhafte Daten in { $path }

journal.cli.doctor.device.day.listing.registry.error = Gerätedatenströme nicht verfügbar: Datenstrom-Registry nicht lesbar: { $error }

journal.cli.doctor.device.day.listing.repair = einem Gerät, das diesen Tag abgleicht, wird gesagt, dass das Journal ihn nicht lesen kann; eine { $brand }-App, die älter ist als die Fehlerbehebung von 2026-09, zeigt das als offline; untersuche die Segmente des genannten Datenstrom-Verzeichnisses und ihre { $events_file }

journal.cli.doctor.device.day.listing.streams.unbound = keine Gerätedatenströme gebunden

journal.cli.doctor.device.refusal.count = { $quantity ->
    [one] bei Gerät { $device } wurde eine Anfrage abgewiesen: { $reason }, { $attempts }x bis { $last_seen }
    *[other] bei Gerät { $device } wurden Anfragen abgewiesen: { $reason }, { $attempts }x bis { $last_seen }
}

journal.cli.doctor.devices.absent = keine registrierten Geräte

journal.cli.doctor.devices.more = { $devices }{ $separator }+{ $extra } weitere

journal.cli.doctor.devices.unavailable = Gerätedatensätze nicht verfügbar: { $error }

journal.cli.doctor.disk.space.layout.unsupported = die aufgelöste Installationswurzel ist kein Layout-Installationsbaum

journal.cli.doctor.disk.space.package.install = eine Paketinstallation wird bei der Aktualisierung an Ort und Stelle ersetzt, daher ist keine zweite Kopie nötig

journal.cli.doctor.disk.space.root.unresolved = die Installationswurzel konnte nicht aus dem bin-Verzeichnis der Installation bestimmt werden: { $path }

journal.cli.doctor.disk.space.size.overflow = Größenüberlauf des Installationsbaums bei { $path }

journal.cli.doctor.disk.space.size.unreadable = der Installationsbaum bei { $path } konnte nicht gemessen werden: { $error }

journal.cli.doctor.disk.space.space.available = { $free_gib } { $gib } frei auf dem Dateisystem, auf dem { $path } liegt ({ $required_gib } { $gib } ist eine zweite Kopie dieser Installation plus Puffer)

journal.cli.doctor.disk.space.space.inspect = freier Speicherplatz bei { $path } konnte nicht untersucht werden: { $error }

journal.cli.doctor.disk.space.space.low = nur { $free_gib } { $gib } frei auf dem Dateisystem, auf dem { $path } liegt; { $required_gib } { $gib } ist eine zweite Kopie dieser Installation plus Puffer

journal.cli.doctor.disk.space.space.unreadable = freier Speicherplatz ist bei { $path } nicht lesbar

journal.cli.doctor.durability.action.recovered = keine Aktion ist nötig: wiederhergestellte Datensätze bleiben neben den Originalen und sind hier aufgeführt

journal.cli.doctor.durability.action.refused = das Journal wird nicht starten, bis sich das klärt; jede Zeile oben sagt, ob es sich nach einem Neustart des Computers klärt oder Hilfe braucht

journal.cli.doctor.durability.action.unreadable = prüfe die Pfade unter 'konnte nicht untersuchen'; etwaige wiederhergestellte Datensätze bleiben neben den Originalen und sind hier aufgeführt

journal.cli.doctor.durability.admission.helper = { $finding ->
    [exited] Generation { $generation } Zulassung { $admission }: Helfer { $identity }, beendet
    [escalated] Generation { $generation } Zulassung { $admission }: Helfer { $identity }, stillgelegt (eskaliert: true)
    [retired] Generation { $generation } Zulassung { $admission }: Helfer { $identity }, stillgelegt
    [unacknowledged] Generation { $generation } Zulassung { $admission }: Helfer { $identity }, nie bestätigt
    [spawn] Generation { $generation } Zulassung { $admission }: Helfer { $identity }, Start fehlgeschlagen
    [unidentified] Generation { $generation } Zulassung { $admission }: Helfer { $identity }, nicht erkannt
    [rejected] Generation { $generation } Zulassung { $admission }: Helfer { $identity }, abgelehnt und eingesammelt
    *[reused] Generation { $generation } Zulassung { $admission }: Helfer { $identity }, PID von einem anderen Konto wiederverwendet
}

journal.cli.doctor.durability.admission.service = { $finding ->
    [exited] Generation { $generation } Zulassung { $admission }: Dienst { $identity }, beendet
    [escalated] Generation { $generation } Zulassung { $admission }: Dienst { $identity }, stillgelegt (eskaliert: true)
    [retired] Generation { $generation } Zulassung { $admission }: Dienst { $identity }, stillgelegt
    [unacknowledged] Generation { $generation } Zulassung { $admission }: Dienst { $identity }, nie bestätigt
    [spawn] Generation { $generation } Zulassung { $admission }: Dienst { $identity }, Start fehlgeschlagen
    [unidentified] Generation { $generation } Zulassung { $admission }: Dienst { $identity }, nicht erkannt
    [rejected] Generation { $generation } Zulassung { $admission }: Dienst { $identity }, abgelehnt und eingesammelt
    *[reused] Generation { $generation } Zulassung { $admission }: Dienst { $identity }, PID von einem anderen Konto wiederverwendet
}

journal.cli.doctor.durability.clean = keine aufgezeichnete Wiederherstellung zu melden

journal.cli.doctor.durability.clean.outstanding = { $quantity ->
    [one] keine aufgezeichnete Wiederherstellung zu melden; der neueste Lauf (Generation { $generation }) schuldet noch { $launches } Start
    *[other] keine aufgezeichnete Wiederherstellung zu melden; der neueste Lauf (Generation { $generation }) schuldet noch { $launches } Starts
}

journal.cli.doctor.durability.closed = { $quantity ->
    [one] Generation { $generation } wurde von dem Start geschlossen, der zu Generation { $successor } wurde (unix { $time }): { $total } Zulassung, { $exited } bereits beendet, { $retired } vom Journal gestoppt, { $never } nie zugelassen
    *[other] Generation { $generation } wurde von dem Start geschlossen, der zu Generation { $successor } wurde (unix { $time }): { $total } Zulassungen, { $exited } bereits beendet, { $retired } vom Journal gestoppt, { $never } nie zugelassen
}

journal.cli.doctor.durability.closed.restarted = { $quantity ->
    [one] Generation { $generation } wurde von dem Start geschlossen, der zu Generation { $successor } wurde (unix { $time }): { $total } Zulassung, { $exited } bereits beendet, { $retired } vom Journal gestoppt, { $never } nie zugelassen; der Computer war seitdem neu gestartet
    *[other] Generation { $generation } wurde von dem Start geschlossen, der zu Generation { $successor } wurde (unix { $time }): { $total } Zulassungen, { $exited } bereits beendet, { $retired } vom Journal gestoppt, { $never } nie zugelassen; der Computer war seitdem neu gestartet
}

journal.cli.doctor.durability.heading.aside = Buchführung beiseitegelegt ({ $count }):

journal.cli.doctor.durability.heading.closed = Läufe, die ein späterer Start geschlossen hat ({ $count }):

journal.cli.doctor.durability.heading.refused = abgelehnte Starts ({ $count }):

journal.cli.doctor.durability.heading.unreadable = konnte nicht untersuchen ({ $count }):

journal.cli.doctor.durability.heading.unresolved = Läufe, die nicht sauber gestoppt haben ({ $count }):

journal.cli.doctor.durability.more = ... und { $count } weitere

journal.cli.doctor.durability.outstanding = { $quantity ->
    [one] der neueste Lauf (Generation { $generation }) schuldet noch { $launches } Start
    *[other] der neueste Lauf (Generation { $generation }) schuldet noch { $launches } Starts
}

journal.cli.doctor.durability.refusal.restart = Generation { $generation }: der Datensatz der Prozesse, die eine Stilllegung gestartet hat, ist verloren, daher wird das Journal nicht starten; es klärt sich, nachdem der Computer neu startet

journal.cli.doctor.durability.refusal.support = Generation { $generation }: der Datensatz der Prozesse, die eine Stilllegung gestartet hat, ist verloren, daher wird das Journal nicht starten; es klärt sich nicht von selbst und braucht Hilfe vom Support

journal.cli.doctor.durability.unresolved = Generation { $generation }: { $reason }

journal.cli.doctor.executable.directory = das Verzeichnis der Programmdatei konnte nicht bestimmt werden: { $path }

journal.cli.doctor.execution.error = Ausführung der Prüfung fehlgeschlagen: { $kind }

journal.cli.doctor.execution.error.detail = Ausführung der Prüfung fehlgeschlagen: { $kind }: { $error }

journal.cli.doctor.facet.fix = { $unrouted_quantity ->
    [one] dieses Segment fehlt in den Aktivitätslisten; prüfe die Facetten-Deklarationen mit `{ $doctor_command }` und die jüngsten sense-Läufe mit `{ $logs_command }`
    *[other] diese Segmente fehlen in den Aktivitätslisten; prüfe die Facetten-Deklarationen mit `{ $doctor_command }` und die jüngsten sense-Läufe mit `{ $logs_command }`
}

journal.cli.doctor.facet.no.active = keine aktiven Segmente in den neuesten Tagen

journal.cli.doctor.facet.routed = { $active_quantity ->
    [one] { $routed } von { $active } aktivem Segment seit { $since } unter einer Facette abgelegt
    *[other] { $routed } von { $active } aktiven Segmenten seit { $since } unter einer Facette abgelegt
}

journal.cli.doctor.facet.unrouted = { $active_quantity ->
    [one] { $unrouted } von { $active } aktivem Segment seit { $since } { $unrouted_quantity ->
        [one] wurde
        *[other] wurden
    } unter keiner Facette abgelegt
    *[other] { $unrouted } von { $active } aktiven Segmenten seit { $since } { $unrouted_quantity ->
        [one] wurde
        *[other] wurden
    } unter keiner Facette abgelegt
}

journal.cli.doctor.help = Verwendung: { $syntax }{ $line_break }{ $line_break }Führe die { $brand }-Diagnose aus.{ $line_break }{ $line_break }Optionen:{ $line_break }{ $indent }{ $help_option }{ $help_gap }diese Hilfenachricht anzeigen und beenden{ $line_break }{ $indent }{ $verbose_option }{ $verbose_gap }jedes Prüfergebnis ausgeben{ $line_break }{ $indent }{ $json_option }{ $json_gap }JSON statt Text ausgeben{ $line_break }{ $indent }{ $jsonl_option }{ $jsonl_gap }Ereignisse als ein JSON pro Zeile statt Text ausgeben{ $line_break }{ $indent }{ $port_option }{ $port_gap }Port zum Prüfen (Standard: { $port }){ $line_break }{ $indent }{ $readiness_option }{ $readiness_gap }die Bereitschaftsreihe der Ersteinrichtung ausführen{ $line_break }{ $line_break }Wenn '{ $command }' nicht verfügbar ist, baue den nativen doctor aus diesem Checkout:{ $line_break }{ $build_command }

journal.cli.doctor.holds.partition = zurückgehaltene Partition { $partition }

journal.cli.doctor.holds.partition.stopped = Partition { $partition } lief, als das Journal gestoppt hat; das Journal hält sie bei seinem nächsten Start zurück, bis ihre Prozesse weg sind

journal.cli.doctor.holds.record.empty = Datensatzdatei ist leer

journal.cli.doctor.holds.record.filename = ungültiger Partitionsdateiname im Bereichsverzeichnis; das Journal liest ihn trotzdem als Aufgabendatensatz

journal.cli.doctor.holds.record.invalid = Datensatz kann nicht geparst werden: { $error }

journal.cli.doctor.holds.record.stray = Eintrag, der kein Datensatz ist, im Bereichsverzeichnis

journal.cli.doctor.holds.record.unreadable = Datensatz kann nicht gelesen werden: { $error }

journal.cli.doctor.holds.remove = entferne { $path }: er hält nichts

journal.cli.doctor.holds.scope.unknown = unbekanntes Format des Bereichsverzeichnisses; das Journal hält trotzdem jede darin aufgezeichnete Aufgabe

journal.cli.doctor.holds.scope.unreadable = Bereichsverzeichnis kann nicht aufgelistet werden: { $error }

journal.cli.doctor.holds.stray = verirrtes Nicht-Verzeichnis im laufenden Pfad der Aufgabenwarteschlange

journal.cli.doctor.holds.unreadable = Verzeichnis der laufenden Zurückhaltungen kann nicht aufgelistet werden: { $error }

journal.cli.doctor.index.fix.checking = die Suche prüft das selbst und baut neu auf, wenn der Schaden bestätigt ist; führe { $command } aus für Details

journal.cli.doctor.index.fix.deletions = die Suche versucht diese Entfernungen immer wieder; führe { $command } aus für Details

journal.cli.doctor.index.fix.errors = führe { $command } aus und prüfe seine Fehler

journal.cli.doctor.index.fix.failed.files = die Suche versucht diese Aktualisierungen bei ihrer täglichen Prüfung erneut; führe { $command } aus für Details

journal.cli.doctor.index.fix.failed.work = die Suche versucht diese Arbeit jede Stunde erneut; führe { $command } aus für Details

journal.cli.doctor.index.fix.full = führe { $command } aus

journal.cli.doctor.index.fix.rebuild = die Suche baut ihren Index aus deinem Journal neu auf, sobald der Schaden bestätigt ist; wenn er bleibt, führe { $command } aus

journal.cli.doctor.index.fix.reset = führe { $command } aus

journal.cli.doctor.index.fix.upgrade = aktualisiere { $brand } auf die neueste Version

journal.cli.doctor.index.fix.warnings = führe { $command } aus und prüfe seine Warnungen

journal.cli.doctor.journal.absent = kein lokales Journal

journal.cli.doctor.journal.caught.up.backlog.unavailable = das ließ sich nicht vollständig bestimmen; das Lesen des Rückstands ist fehlgeschlagen: { $error }

journal.cli.doctor.journal.caught.up.chronicle.entry.unavailable = das ließ sich nicht vollständig bestimmen; der Chronikeintrag konnte nicht gelesen werden: { $error }

journal.cli.doctor.journal.caught.up.chronicle.unavailable = das ließ sich nicht vollständig bestimmen; die Chronik konnte nicht gelesen werden: { $error }

journal.cli.doctor.journal.caught.up.configs.unavailable = das ließ sich nicht vollständig bestimmen; die täglichen Konfigurationen konnten nicht gelesen werden: { $error }

journal.cli.doctor.journal.caught.up.coverage.unavailable = die Abdeckung der Tageseinheit am { $day } konnte nicht bestimmt werden: { $error }

journal.cli.doctor.journal.caught.up.definitions.unavailable = das ließ sich nicht vollständig bestimmen; die Definitionen der täglichen Verarbeitung konnten nicht gefunden werden: { $error }

journal.cli.doctor.journal.caught.up.facets.day.unavailable = das ließ sich nicht vollständig bestimmen; die aktiven Facetten für { $day } konnten nicht geprüft werden: { $error }

journal.cli.doctor.journal.caught.up.facets.unavailable = das ließ sich nicht vollständig bestimmen; die Facetten konnten nicht gelesen werden: { $error }

journal.cli.doctor.journal.caught.up.inspect = öffne den Zustand, um wartende Arbeit und eine Aufwandsbestätigung zu sehen; verarbeite einen vergangenen Tag neu, um ihm nach heute Vorrang zu geben

journal.cli.doctor.journal.caught.up.inventory.fix = öffne den Zustand, um wartende Arbeit und ihren nächsten Schritt zu sehen

journal.cli.doctor.journal.caught.up.inventory.unavailable = Prüfung des ganzen Journals nicht verfügbar: { $error }

journal.cli.doctor.journal.caught.up.inventory.waiting = Prüfung des ganzen Journals hat noch nicht gemeldet

journal.cli.doctor.journal.caught.up.outside.backlog = Tageseinheit außerhalb des jüngsten Rückstands am { $day }

journal.cli.doctor.journal.caught.up.repair = führe { $doctor_command } erneut aus; prüfe die Zustandsprotokolle, wenn es bleibt

journal.cli.doctor.journal.caught.up.unit.identity = Tageseinheitendatei { $file_name } am Tag { $day } konnte nicht gelesen werden: die Identität des Datensatzes stimmt nicht mit seinem Pfad überein

journal.cli.doctor.journal.caught.up.unit.invalid = das ließ sich nicht vollständig bestimmen; fehlerhafte Tageseinheitendatei { $file_name } am Tag { $day }: { $source }

journal.cli.doctor.journal.caught.up.unit.unreadable = das ließ sich nicht vollständig bestimmen; nicht lesbare Tageseinheitendatei { $file_name } am Tag { $day }: { $source }

journal.cli.doctor.journal.caught.up.unit.version = Tageseinheitendatei { $file_name } am Tag { $day } konnte nicht gelesen werden: nicht unterstützte Datensatzversion { $version }

journal.cli.doctor.journal.caught.up.units.entry.unavailable = das ließ sich nicht vollständig bestimmen; der Tageseinheiten-Eintrag für { $day } konnte nicht gelesen werden: { $error }

journal.cli.doctor.journal.caught.up.units.unavailable = das ließ sich nicht vollständig bestimmen; das daily-units-Verzeichnis für { $day } konnte nicht gelesen werden: { $error }

journal.cli.doctor.journal.dir.writable.move = verschiebe oder entferne { $path }, und führe dann erneut aus

journal.cli.doctor.journal.dir.writable.not.directory = Journal-Pfad existiert, ist aber kein Verzeichnis: { $path }

journal.cli.doctor.journal.dir.writable.parent.unwritable = Journal-Verzeichnis fehlt; der nächste vorhandene Vorfahre ist nicht beschreibbar: { $path }

journal.cli.doctor.journal.dir.writable.parent.writable = Journal-Verzeichnis fehlt; übergeordnetes Verzeichnis { $path } ist beschreibbar

journal.cli.doctor.journal.dir.writable.unwritable = Journal-Verzeichnis nicht beschreibbar: { $path }

journal.cli.doctor.journal.dir.writable.writable = Journal-Verzeichnis beschreibbar: { $path }

journal.cli.doctor.journal.sources.readable.healthy = der letzte Scan hat keine unlesbaren Tage gefunden

journal.cli.doctor.journal.sources.readable.inspect.folders = prüfe die oben genannten Tagesordner — ein Name, der kein Kalenderdatum ist, ist kein Tag, den das Journal verarbeiten kann

journal.cli.doctor.journal.sources.readable.repair.files = repariere die oben genannte Datei, und führe dann { $reprocess_command } für jeden aufgeführten Tag, der bereits vorbei ist, aus

journal.cli.doctor.journal.sources.readable.reprocess = führe { $reprocess_command } für jeden aufgeführten Tag, der bereits vorbei ist, aus

journal.cli.doctor.journal.sources.readable.stats.invalid = das ließ sich nicht prüfen — die Statistiken sind nicht lesbar

journal.cli.doctor.journal.sources.readable.stats.missing = das ließ sich nicht prüfen — keine Statistiken zum Lesen

journal.cli.doctor.journal.sources.readable.stats.predates = das ließ sich nicht prüfen — die Statistiken stammen von vor dieser Version

journal.cli.doctor.launchd.stale.plist.absent = launchd-plist fehlt

journal.cli.doctor.launchd.stale.plist.arguments.missing = plist enthält { $plist_argument } nicht

journal.cli.doctor.launchd.stale.plist.invalid = plist konnte nicht ausgewertet werden: { $plist_error }: { $error }

journal.cli.doctor.launchd.stale.plist.repair = führe { $uninstall_command } aus, und führe dann { $install_command } einzeln aus, um einen kopflosen Hintergrunddienst neu zu installieren

journal.cli.doctor.launchd.stale.plist.target.exists = Ziel der launchd-plist vorhanden ({ $path })

journal.cli.doctor.launchd.stale.plist.target.missing = plist verweist auf eine fehlende Programmdatei: { $path }

journal.cli.doctor.local.bin.solstone.reachable.file.invalid = { $path } ist keine Datei

journal.cli.doctor.local.bin.solstone.reachable.file.missing = { $path } fehlt

journal.cli.doctor.local.bin.solstone.reachable.path.absent = { $brand } ist nicht auf { $path_variable }

journal.cli.doctor.local.bin.solstone.reachable.path.mismatch = { $path_variable } { $brand } wird zu { $resolved } aufgelöst, erwartet { $expected }

journal.cli.doctor.local.bin.solstone.reachable.path.ready = { $wrapper_path } liegt auf { $path_variable } unter { $path }

journal.cli.doctor.local.bin.solstone.reachable.path.repair = setze { $bin_dir } weiter vorn auf { $path_variable }, oder führe { $setup_command } aus, um den verwalteten Wrapper neu auszurichten

journal.cli.doctor.local.bin.solstone.reachable.symlink.ready = { $wrapper_path } ist ein symbolischer Link auf { $path_variable } { $brand } unter { $path }

journal.cli.doctor.local.bin.solstone.reachable.windows.path.priority = setze { $install_dir } vor { $other_dir } auf deinen { $path_variable }

journal.cli.doctor.local.bin.solstone.reachable.windows.ready = { $brand } liegt auf { $path_variable } unter { $path }

journal.cli.doctor.local.bin.solstone.reachable.windows.repair = installiere das Journal neu, um { $executable } wiederherzustellen

journal.cli.doctor.local.bin.solstone.reachable.windows.terminal = öffne ein neues Terminalfenster und versuch es noch einmal; wenn { $brand } immer noch nicht gefunden wird, installiere das Journal neu

journal.cli.doctor.local.bin.solstone.reachable.wrapper.install = führe { $setup_command } aus, um den verwalteten { $brand }-Wrapper zu installieren

journal.cli.doctor.orphan.pdf.found = { $quantity ->
    [one] { $count } unbearbeitetes PDF-Original ohne lesbares Dokumenttranskript: { $paths }
   *[other] { $count } unbearbeitete PDF-Originale ohne lesbares Dokumenttranskript: { $paths }
}

journal.cli.doctor.orphan.segment.pdf.chronicle.unavailable = Chronik-Verzeichnis nicht verfügbar

journal.cli.doctor.orphan.segment.pdf.clear = alle unbearbeiteten PDF-Originale haben lesbare Dokumenttranskripte

journal.cli.doctor.orphan.segment.pdf.repair = füge { $transcript_file } lesbar neben das PDF-Original hinzu, und führe dann { $doctor_command } erneut aus

journal.cli.doctor.package.executable.parent = Programmdatei hat kein übergeordnetes Verzeichnis

journal.cli.doctor.package.layout = gepackte Talent-Wurzeln konnten nicht aus dem Verzeichnis der Programmdatei { $directory } gefunden werden{ $newline }durchlaufener Vorfahre { $ancestor }{ $newline }site-packages-Kandidat: { $site }{ $newline }Checkout-Kandidat ({ $pyproject } + { $git_dir }): { $checkout }{ $newline }Layout-share-Anker { $bundle }, { $layout }, { $template }: { $install }{ $newline }Apple-Bundle { $journal_payload }-Kandidat: { $embedded }

journal.cli.doctor.package.missing = in der Installationswurzel { $root } fehlt das Verzeichnis { $path }

journal.cli.doctor.package.none = nichts

journal.cli.doctor.package.target = { $newline }Cargo-Zielverzeichnis { $target } liegt außerhalb jedes Checkouts: eine dort gebaute Binärdatei kann die Nutzdaten nicht finden, also halte { $target_env } im Checkout

journal.cli.doctor.package.unavailable = gepackte Talent-Wurzeln konnten nicht aus dem Verzeichnis der Programmdatei { $directory } gefunden werden

journal.cli.doctor.parakeet.coreml.configured = parakeet-Prüfung fehlgeschlagen: installierter Zwischenspeicher { $installed } stimmt nicht mit der Konfiguration { $configured } überein

journal.cli.doctor.parakeet.coreml.digest = parakeet-Prüfung fehlgeschlagen: Asset-Digest weicht ab für { $file }: erwartet { $expected }, gefunden { $found }

journal.cli.doctor.parakeet.coreml.failed = parakeet-Prüfung fehlgeschlagen: { $error }

journal.cli.doctor.parakeet.coreml.missing = parakeet-Prüfung fehlgeschlagen: Asset fehlt: { $file }

journal.cli.doctor.parakeet.coreml.missing.error = parakeet-Prüfung fehlgeschlagen: Asset fehlt: { $file }: { $error }

journal.cli.doctor.parakeet.coreml.sentinel = parakeet-Prüfung fehlgeschlagen: Sentinel nicht bereit unter { $path }

journal.cli.doctor.parakeet.coreml.size = parakeet-Prüfung fehlgeschlagen: Abweichung der Asset-Größe für { $file }: erwartet { $expected }, gefunden { $found }

journal.cli.doctor.parakeet.coreml.unreadable = parakeet-Prüfung fehlgeschlagen: { $file } konnte nicht gelesen werden: { $error }

journal.cli.doctor.parakeet.cpp.executable = { $parakeet_cpp }-Prüfung fehlgeschlagen: { $name } nicht ausführbar unter { $path }

journal.cli.doctor.parakeet.cpp.missing = { $parakeet_cpp }-Prüfung fehlgeschlagen: { $name } fehlt unter { $path }

journal.cli.doctor.parakeet.cpp.model = { $parakeet_cpp }-Prüfung fehlgeschlagen: Modell fehlt unter { $path }

journal.cli.doctor.parakeet.cpp.pin = { $parakeet_cpp }-Pin enthält { $key } nicht

journal.cli.doctor.parakeet.cpp.platform = { $parakeet_cpp } wird auf { $os }/{ $arch } nicht unterstützt

journal.cli.doctor.parakeet.cpp.stt.ready.backend.not.selected = konfiguriertes backend ist nicht { $backend_name }; Prüfung nicht anwendbar

journal.cli.doctor.parakeet.cpp.stt.ready.binary.unavailable = { $backend_name }-Binärdatei kann nicht starten

journal.cli.doctor.parakeet.cpp.stt.ready.install = { $backend_name }-Artefakte sind nicht installiert — hol sie mit: { $provider_command }

journal.cli.doctor.parakeet.cpp.stt.ready.offline = { $server } nicht erreichbar: dein Journal läuft nicht

journal.cli.doctor.parakeet.cpp.stt.ready.online = { $backend_name } bereit (Server und Modell stimmen mit dem signierten Paket überein, Server erreichbar)

journal.cli.doctor.parakeet.cpp.stt.ready.ready = { $backend_name } bereit (Binärdateien und Modell installiert, Server erreichbar)

journal.cli.doctor.parakeet.cpp.stt.ready.runtime.install = installiere die { $openmp }-Laufzeitumgebung des Systems, die { $runtime_file } bereitstellt, und führe dann { $doctor_command } erneut aus

journal.cli.doctor.parakeet.cpp.stt.ready.runtime.unavailable = { $backend_name } kann nicht starten: { $openmp }-Laufzeitumgebung nicht verfügbar ({ $runtime_file })

journal.cli.doctor.parakeet.cpp.stt.ready.server.missing = { $server } nicht erreichbar: dein Journal läuft, aber nicht { $server }

journal.cli.doctor.parakeet.cpp.stt.ready.start = { $server } ist nicht erreichbar — starte den Journal-Dienst: { $start_command }

journal.cli.doctor.parakeet.cpp.stt.ready.unreachable = { $server } nicht erreichbar: { $error }

journal.cli.doctor.parakeet.cpp.stt.ready.unsupported = { $backend_name } wird nur auf Linux und Windows unterstützt

journal.cli.doctor.parakeet.phase = { $server } nicht erreichbar: dein Journal meldet seinen Zustand als { $phase }

journal.cli.doctor.parakeet.phase.checking = { $server } nicht erreichbar: dein Journal meldet seinen Zustand als wird geprüft

journal.cli.doctor.parakeet.reinstall = installiere das Journal neu

journal.cli.doctor.parakeet.unreachable = { $cause ->
    [absent] { $server } nicht erreichbar: nichts lauscht
    [unknown] { $server } nicht erreichbar: es lässt sich nicht feststellen, ob etwas lauscht
    [runtime] { $server } nicht erreichbar: nichts hat geantwortet
    [timeout] { $server } nicht erreichbar: die Antwort hat zu lange gedauert
   *[transport] { $server } nicht erreichbar: die Verbindung ist abgebrochen
}

journal.cli.doctor.path.repair = korrigiere Eigentümer und Berechtigungen von { $path }

journal.cli.doctor.platform.unsupported = wird auf { $platform } nicht unterstützt

journal.cli.doctor.progress.check = { $seconds } s prüft { $name }

journal.cli.doctor.progress.hash = { $seconds } s überprüft { $member }: { $hashed } von { $total } { $megabytes }

journal.cli.doctor.search.log.finished = letzte abgeschlossene Suchprüfung konnte nicht gelesen werden: { $error }

journal.cli.doctor.search.log.unavailable = Suchindex-Status nicht verfügbar: { $error }

journal.cli.doctor.search.log.work = Arbeitsliste der Suche konnte nicht gelesen werden: { $error }

journal.cli.doctor.sense.dispatch.capped.reason = mindestens 99 Sense-Dispatch-Fehler seit dem letzten erfolgreichen Handler-Abgleich; letzter Grund: { $reason }

journal.cli.doctor.sense.dispatch.capped.unknown = mindestens 99 Sense-Dispatch-Fehler seit dem letzten erfolgreichen Handler-Abgleich; letzter Grund nicht verfügbar

journal.cli.doctor.sense.dispatch.clear = keine Sense-Dispatch-Fehler seit dem letzten erfolgreichen Handler-Abgleich

journal.cli.doctor.sense.dispatch.count.invalid = Sense-Status ist unvollständig: keine gültige nichtnegative Anzahl von Dispatch-Fehlern

journal.cli.doctor.sense.dispatch.count.out.of.range = Sense-Status ist unvollständig: die Anzahl der Dispatch-Fehler liegt außerhalb des unterstützten Bereichs

journal.cli.doctor.sense.dispatch.errors.reason = { $quantity ->
    [one] { $count } Sense-Dispatch-Fehler seit dem letzten erfolgreichen Handler-Abgleich; letzter Grund: { $reason }
   *[other] { $count } Sense-Dispatch-Fehler seit dem letzten erfolgreichen Handler-Abgleich; letzter Grund: { $reason }
}

journal.cli.doctor.sense.dispatch.errors.unknown = { $quantity ->
    [one] { $count } Sense-Dispatch-Fehler seit dem letzten erfolgreichen Handler-Abgleich; letzter Grund nicht verfügbar
   *[other] { $count } Sense-Dispatch-Fehler seit dem letzten erfolgreichen Handler-Abgleich; letzter Grund nicht verfügbar
}

journal.cli.doctor.sense.dispatch.unreachable = { $cause ->
    [absent] Sense-Dispatch-Zustand konnte nicht bestätigt werden: nichts lauscht
    [unknown] Sense-Dispatch-Zustand konnte nicht bestätigt werden: es lässt sich nicht feststellen, ob etwas lauscht
    [runtime] Sense-Dispatch-Zustand konnte nicht bestätigt werden: nichts hat geantwortet
    [timeout] Sense-Dispatch-Zustand konnte nicht bestätigt werden: die Antwort hat zu lange gedauert
   *[transport] Sense-Dispatch-Zustand konnte nicht bestätigt werden: die Verbindung ist abgebrochen
}

journal.cli.doctor.service.absent = kein lokaler Journal-Dienst

journal.cli.doctor.service.conflict.warning = behebe die Warnung zur macOS-Supervisor-Topologie, bevor du den Journal-Dienst änderst

journal.cli.doctor.service.crash.list = Absturzschleife: { $services }

journal.cli.doctor.service.identity.repair = führe { $setup_command } aus, um den Dienst neu zu installieren

journal.cli.doctor.service.identity.repair.force = führe von dieser Installation aus { $setup_command } aus, um den Dienst zu aktualisieren

journal.cli.doctor.service.identity.target.absent = Dienst-Unit ist fehlerhaft: kein Programmdatei-Ziel in { $path }

journal.cli.doctor.service.identity.target.current = Dienstziel entspricht der aktuellen Installation: { $raw } -> { $path }

journal.cli.doctor.service.identity.target.mismatch = Abweichung beim Dienstziel: { $raw } wird zu { $resolved } aufgelöst, erwartet { $expected }

journal.cli.doctor.service.identity.windows.current = Dienstziel entspricht der aktuellen Installation: { $command }

journal.cli.doctor.service.identity.windows.mismatch = Abweichung beim Dienstziel: { $mismatch }

journal.cli.doctor.service.registration.unreadable = die Dienstregistrierung des Journals konnte nicht gelesen werden — { $reason }

journal.cli.doctor.service.restart.attempts = { $quantity ->
    [one] { $name } ({ $attempts } Neustartversuch)
   *[other] { $name } ({ $attempts } Neustartversuche)
}

journal.cli.doctor.service.running.logs = führe { $logs_command } aus

journal.cli.doctor.service.running.pid.invalid = Kind-PID der Probe passt nicht in eine Prozessgruppen-ID

journal.cli.doctor.service.running.registration.no.reader = kein Leser für die Dienstregistrierung

journal.cli.doctor.service.running.restart = führe { $restart_command } aus; wenn es anhält, führe { $logs_command } aus

journal.cli.doctor.service.running.running = Journal-Dienst läuft

journal.cli.doctor.service.running.start = führe { $start_command } aus

journal.cli.doctor.service.running.stopped = Dienst installiert, läuft aber nicht

journal.cli.doctor.service.running.unit.failed = Journal-Dienst-Unit ist fehlgeschlagen

journal.cli.doctor.service.running.unreachable = { $cause ->
    [absent] ein Status konnte nicht abgerufen werden — nichts lauscht
    [unknown] ein Status konnte nicht abgerufen werden — es lässt sich nicht feststellen, ob etwas lauscht
    [runtime] ein Status konnte nicht abgerufen werden — nichts hat geantwortet
    [timeout] ein Status konnte nicht abgerufen werden — die Antwort hat zu lange gedauert
   *[transport] ein Status konnte nicht abgerufen werden — die Verbindung ist abgebrochen
}

journal.cli.doctor.skill.state.checkout.required = Router-skill-Zustand ist nur aus einem Quell-Checkout verfügbar

journal.cli.doctor.skill.state.directories.missing = Router-skill-Verzeichnisse sind nicht verfügbar

journal.cli.doctor.skill.state.install = führe { $skills_command } aus

journal.cli.doctor.skill.state.link.foreign = { $skill } zeigt woandershin auf { $path }

journal.cli.doctor.skill.state.link.missing = { $skill } fehlt unter { $path }

journal.cli.doctor.skill.state.link.stale = veralteter Router-skill-Link unter { $path }

journal.cli.doctor.skill.state.links.ready = Router-skills { $brand }, journal sind installiert und aktuell

journal.cli.doctor.source.default = Standard

journal.cli.doctor.sources.days.more = { $days }, und { $more } weitere

journal.cli.doctor.sources.fold.failed = { $quantity ->
    [one] { $count } Tag hat die Verarbeitung nicht abgeschlossen ({ $days })
   *[other] { $count } Tage haben die Verarbeitung nicht abgeschlossen ({ $days })
}

journal.cli.doctor.sources.statistics.refresh = führe { $command } aus; prüfe die Zustandsprotokolle, wenn es anhält

journal.cli.doctor.sources.statistics.stale = das ließ sich nicht prüfen — die Statistiken sind { $hours } Stunden alt

journal.cli.doctor.sources.unreadable = { $quantity ->
    [one] { $count } Tag konnte nicht gescannt werden ({ $days })
   *[other] { $count } Tage konnten nicht gescannt werden ({ $days })
}

journal.cli.doctor.sources.unreadable.cause = { $quantity ->
    [one] { $count } Tag konnte nicht gescannt werden ({ $days }): { $day }: { $cause }
   *[other] { $count } Tage konnten nicht gescannt werden ({ $days }): { $day }: { $cause }
}

journal.cli.doctor.sources.unreadable.undated = { $quantity ->
    [one] { $count } Tag konnte nicht gescannt werden ({ $days }): dieser Tag: { $cause }
   *[other] { $count } Tage konnten nicht gescannt werden ({ $days }): dieser Tag: { $cause }
}

journal.cli.doctor.speakers.installation.app = { $speakers_name }-Installation ist unvollständig ({ $detail }). Reparatur: installiere die Journal-App neu.

journal.cli.doctor.speakers.installation.journal = { $speakers_name }-Installation ist unvollständig ({ $detail }). Reparatur: installiere das Journal neu.

journal.cli.doctor.speakers.ready = { $command }-Installation bereit

journal.cli.doctor.speakers.reinstall.app = installiere die Journal-App neu

journal.cli.doctor.speakers.unavailable = { $command }-Installation nicht verfügbar

journal.cli.doctor.status.error = ERROR

journal.cli.doctor.status.fail = FAIL

journal.cli.doctor.status.ok = OK

journal.cli.doctor.status.skip = SKIP

journal.cli.doctor.status.warn = WARN

journal.cli.doctor.storage.inspection.log = Speicher-Datenschutz: { $error }

journal.cli.doctor.storage.privacy.choice = andere Konten auf diesem Computer können dein Journal möglicherweise lesen. sein Ordner konnte auf diesem Laufwerk nicht privat gemacht werden. du kannst es auf diesem Laufwerk behalten oder es auf ein Laufwerk verschieben, das Ordner privat behält. um es auf diesem Laufwerk zu behalten, wähle das auf der Seite Zustand deines Journals.

journal.cli.doctor.summary = { $total_quantity ->
    [one] doctor: { $total } Prüfung, { $failed } fehlgeschlagen, { $warnings } { $warning_quantity ->
        [one] Warnung
       *[other] Warnungen
    }, { $skipped } übersprungen, { $errors } { $error_quantity ->
        [one] Fehler
       *[other] Fehler
    }
   *[other] doctor: { $total } Prüfungen, { $failed } fehlgeschlagen, { $warnings } { $warning_quantity ->
        [one] Warnung
       *[other] Warnungen
    }, { $skipped } übersprungen, { $errors } { $error_quantity ->
        [one] Fehler
       *[other] Fehler
    }
}

journal.cli.doctor.supervisor.conflict.clear = kein macOS-Supervisor-Konflikt (native Prüfung: nur Scan fremder Launcher; kein konkurrierender KeepAlive-Launcher gefunden)

journal.cli.doctor.supervisor.conflict.repair = { $quantity ->
    [one] entferne den fremden Launcher, der auf { $app_path } zielt: { $commands }; führe dann { $doctor_command } erneut aus
   *[other] entferne fremde Launcher, die auf { $app_path } zielen: { $commands }; führe dann { $doctor_command } erneut aus
}

journal.cli.doctor.supervisor.conflict.report = { $quantity ->
    [one] macOS-Supervisor-Konflikt: { $count } fremder { $keep_alive }-Launcher zielt auf { $app_path }
   *[other] macOS-Supervisor-Konflikt: { $count } fremde { $keep_alive }-Launcher zielen auf { $app_path }
}

journal.cli.doctor.supervisor.conflict.resolve = { $quantity ->
    [one] behebe zuerst den macOS-Supervisor-Konflikt: entferne den fremden Launcher, der auf { $app_path } zielt: { $commands }; führe dann { $doctor_command } erneut aus
   *[other] behebe zuerst den macOS-Supervisor-Konflikt: entferne fremde Launcher, die auf { $app_path } zielen: { $commands }; führe dann { $doctor_command } erneut aus
}

journal.cli.doctor.supervisor.conflict.unsupported = wird auf Windows nicht unterstützt

journal.cli.doctor.sync.directory.required = Nachfahre im flachen Verzeichnis ist kein Verzeichnis: { $path }

journal.cli.doctor.sync.entry.changed = Eintrag hat sich während der Beobachtung geändert: { $reason }

journal.cli.doctor.sync.entry.invalid = ungültiger Eintrag im flachen Verzeichnis { $entry }: { $reason }

journal.cli.doctor.sync.entry.replaced = geändert oder verschwunden während der Prüfung

journal.cli.doctor.sync.entry.unsafe = Installation: braucht Aufmerksamkeit{ $newline }dein Journal enthält ein Element, das nicht sicher geprüft werden kann.{ $newline }verschiebe das unter Details genannte Element aus dem Ordner `{ $sync_folder }` deines Journals, und versuch es dann noch einmal.{ $newline }{ $newline }Details:{ $newline }Pfad: { $path }{ $newline }Typ: { $entry_type }{ $newline }Grund: { $reason }

journal.cli.doctor.sync.entry.unsafe.bytes = Installation: braucht Aufmerksamkeit{ $newline }dein Journal enthält ein Element, das nicht sicher geprüft werden kann.{ $newline }verschiebe das unter Details genannte Element aus dem Ordner `{ $sync_folder }` deines Journals, und versuch es dann noch einmal.{ $newline }{ $newline }Details:{ $newline }Pfad: { $path }{ $newline }Typ: { $entry_type }{ $newline }Grund: { $reason }{ $newline }Namensbytes: { $bytes }

journal.cli.doctor.sync.entry.vanished = Eintrag im flachen Verzeichnis ist beim Auflisten verschwunden: { $path }

journal.cli.doctor.sync.file.required = Eintrag im flachen Verzeichnis ist keine reguläre Datei: { $path }

journal.cli.doctor.sync.folder.full = Installation: braucht Aufmerksamkeit{ $newline }dein Journal hat zu viele Elemente, um sie sicher zu prüfen.{ $newline }prüfe die Elemente im Ordner `{ $sync_folder }` deines Journals. verschiebe nur Elemente, von denen du weißt, dass sie dort nicht hingehören, bis 256 oder weniger übrig sind, und versuch es dann noch einmal.{ $newline }{ $newline }Details:{ $newline }Ordner: { $folder }{ $newline }gefundene Elemente: mehr als 256{ $newline }maximal geprüft: 256

journal.cli.doctor.sync.folder.unsafe = Installation: braucht Aufmerksamkeit{ $newline }ein Teil deines Journals kann nicht sicher geprüft werden.{ $newline }prüfe den unter Details genannten Pfad. er muss ein gewöhnlicher Ordner in deinem Journal sein, den du öffnen kannst, bevor du es noch einmal versuchst.{ $newline }{ $newline }Details:{ $newline }Pfad: { $path }{ $newline }Grund: { $reason }

journal.cli.doctor.sync.heartbeat = Installation: braucht Aufmerksamkeit{ $newline }ein heartbeat von vor Kurzem aus einem anderen Lauf ist vorhanden.{ $newline }warte einen Moment, und versuch es dann noch einmal.

journal.cli.doctor.sync.identity.changed = Identität im flachen Verzeichnis hat sich geändert: { $path }

journal.cli.doctor.sync.io = { $operation } ist für { $path } fehlgeschlagen: { $reason }

journal.cli.doctor.sync.kind.blockdevice = Blockgerät

journal.cli.doctor.sync.kind.characterdevice = Zeichengerät

journal.cli.doctor.sync.kind.directory = Verzeichnis

journal.cli.doctor.sync.kind.fifo = benannte Pipe

journal.cli.doctor.sync.kind.other = anderes Dateisystemobjekt

journal.cli.doctor.sync.kind.regularfile = reguläre Datei

journal.cli.doctor.sync.kind.socket = Unix-Socket

journal.cli.doctor.sync.kind.symlink = symbolischer Link

journal.cli.doctor.sync.local = nur dieser Computer ({ $host })

journal.cli.doctor.sync.path.invalid = ungültiger Pfad im flachen Verzeichnis { $path }: { $reason }

journal.cli.doctor.sync.peer.legacy = nur dieser Computer ({ $host }){ $newline }  letzter anderer Schreiber: { $writer } (Legacy-Rechner { $prefix }...)

journal.cli.doctor.sync.peer.unknown = nur dieser Computer ({ $host }){ $newline }  letzter anderer Schreiber: { $writer } (nicht erkannter heartbeat)

journal.cli.doctor.sync.peer.writer = nur dieser Computer ({ $host }){ $newline }  letzter anderer Schreiber: { $writer } (Schreiber { $prefix }... Lauf { $run })

journal.cli.doctor.sync.reason.disappeared = Eintrag ist während der Beobachtung verschwunden

journal.cli.doctor.sync.reason.nonregular = keine reguläre Datei ({ $kind })

journal.cli.doctor.sync.reason.oversized = reguläre Datei ist { $size } Bytes, Grenze ist { $limit }

journal.cli.doctor.sync.reason.read = beobachtete Datei konnte nicht gelesen werden

journal.cli.doctor.sync.size = Eintrag im flachen Verzeichnis überschreitet die Grenze für das beobachtete Lesen: { $path } ist { $kind }, { $size } Bytes überschreiten { $limit }

journal.cli.doctor.sync.symlink = Nachfahre im flachen Verzeichnis ist ein symbolischer Link: { $path }

journal.cli.doctor.sync.unsupported = Installation: braucht Aufmerksamkeit{ $newline }die Fähigkeit zum erneuten Scannen des Abgleichs ist auf dieser Plattform nicht verfügbar.

journal.cli.doctor.sync.unverifiable = Installation: braucht Aufmerksamkeit{ $newline }Startstatus konnte nicht überprüft werden.{ $newline }warte einen Moment, und versuch es dann noch einmal.

journal.cli.doctor.sync.waiting = Installation: wartet{ $newline }ein heartbeat von vor Kurzem aus einem anderen Lauf wurde gefunden.{ $newline }die { $brand }-App wartet, während sie prüft, ob diese Aktivität weitergeht. sie prüft in Kürze erneut.

journal.cli.doctor.task.pace.about.cap = ein Auftrag läuft lange; er wird automatisch gestoppt, wenn er seine Obergrenze überschreitet — kein Handeln nötig, außer wenn er anhält

journal.cli.doctor.task.pace.healthy = Aufgaben im Takt

journal.cli.doctor.task.pace.status.unavailable = Supervisor-Status nicht verfügbar

journal.cli.doctor.task.queue.holds.absent = keine Datensätze laufender oder zurückgehaltener Aufgabenpartitionen vorhanden

journal.cli.doctor.task.queue.holds.clear = alle Datensätze der Aufgabenpartitionen sind sauber

journal.cli.doctor.tasks.duration = { $task } ({ $seconds } s von { $limit } s Obergrenze)

journal.cli.doctor.tasks.long = läuft lange: { $tasks }

journal.cli.doctor.transcription.unretryable = { $quantity ->
    [one] { $count } Aufnahme, die das Journal nicht von selbst erneut versucht ({ $handler_key }: { $handler }, { $reason_key }: { $reason_code }): { $identities }
   *[other] { $count } Aufnahmen, die das Journal nicht von selbst erneut versucht ({ $handler_key }: { $handler }, { $reason_key }: { $reason_code }): { $identities }
}

journal.cli.doctor.unretryable.transcribe.input.clear = 0 Aufnahmen, die das Journal nicht von selbst erneut versucht (handler: { $handler }, { $reason_key }: { $reason_code })

journal.cli.doctor.unretryable.transcribe.input.days.unreadable = Chronik-Tage konnten nicht gelesen werden: { $error }

journal.cli.doctor.unretryable.transcribe.input.directory.unreadable = Segmentverzeichnis { $path } kann nicht gelesen werden: { $error }

journal.cli.doctor.unretryable.transcribe.input.entry.unreadable = Segmenteintrag in { $path } kann nicht gelesen werden: { $error }

journal.cli.doctor.unretryable.transcribe.input.handler.missing = Verarbeitungsdatensatz in { $path }: Feld handler fehlt

journal.cli.doctor.unretryable.transcribe.input.identity.invalid = ungültige Segmentidentität in { $path }: { $error }

journal.cli.doctor.unretryable.transcribe.input.open = { $path } kann nicht geöffnet werden: { $error }

journal.cli.doctor.unretryable.transcribe.input.read = { $path } kann nicht gelesen werden: { $error }

journal.cli.doctor.unretryable.transcribe.input.row.encoding = erste Zeile in { $path } ist kein gültiges utf-8: { $error }

journal.cli.doctor.unretryable.transcribe.input.row.json = erste Zeile in { $path } ist kein gültiges JSON: { $error }

journal.cli.doctor.unretryable.transcribe.input.row.large = erste Zeile in { $path } überschreitet die Byte-Grenze

journal.cli.doctor.unretryable.transcribe.input.row.unavailable = erste Zeile in { $path } kann nicht ermittelt werden oder überschreitet die Byte-Grenze

journal.cli.doctor.unretryable.transcribe.input.scan.failed = das ließ sich nicht bestimmen, wie viele Aufnahmen das Journal nicht von selbst erneut versucht ({ $reason })

journal.cli.doctor.unretryable.transcribe.input.segments.unreadable = Segmente für Tag { $day } konnten nicht aufgelistet werden: { $error }

journal.cli.doctor.usage.argument = Verwendung: { $syntax }{ $line_break }{ $command }: Fehler: unerwartetes Argument

journal.cli.doctor.usage.formats = Verwendung: { $syntax }{ $line_break }{ $command }: Fehler: { $json_flag } und { $jsonl_flag } schließen sich gegenseitig aus

journal.cli.doctor.usage.port = Verwendung: { $syntax }{ $line_break }{ $command }: Fehler: { $port_flag } erfordert eine ganze Zahl

journal.cli.doctor.vad.contract = { $vad }-Helfer lief, hat aber den Verwendungsvertrag bei geschlossenem stdin nicht gemeldet (Exit { $exit_code }): { $stderr }

journal.cli.doctor.vad.executable = die aktuelle Programmdatei für den { $vad }-Helfer konnte nicht bestimmt werden: { $error }

journal.cli.doctor.vad.layout = die Suche nach dem { $vad }-Helfer liegt außerhalb der laufenden signierten App-Nutzdaten

journal.cli.doctor.vad.loader = { $vad }-Helfer konnte { $library } nicht laden

journal.cli.doctor.vad.missing = { $vad }-Helfer-Binärdatei fehlt: { $path }

journal.cli.doctor.vad.parent = aktuelle Programmdatei hat kein übergeordnetes Verzeichnis für die Suche nach dem { $vad }-Helfer

journal.cli.doctor.vad.probe.log = { $vad }-Helfer-Probe fehlgeschlagen: { $error }

journal.cli.doctor.vad.ready = { $vad }-Helfer startbar (Verwendungsvertrag bei geschlossenem stdin)

journal.cli.doctor.vad.repair.install = installiere das Journal neu, und führe dann { $doctor_command } erneut aus

journal.cli.doctor.vad.repair.install.app = installiere die Journal-App neu, und führe dann { $doctor_command } erneut aus

journal.cli.doctor.vad.repair.restart = starte das Journal neu, und führe dann { $doctor_command } erneut aus; wenn es immer noch zu einer Zeitüberschreitung kommt, installiere das Journal neu

journal.cli.doctor.vad.repair.restart.app = starte die Journal-App neu, und führe dann { $doctor_command } erneut aus; wenn es immer noch zu einer Zeitüberschreitung kommt, installiere die Journal-App neu

journal.cli.doctor.vad.spawn = { $vad }-Helfer konnte nicht ausgeführt werden: { $cause }

journal.cli.doctor.vad.timeout = { $vad }-Helfer wurde nicht innerhalb von { $timeout } beendet

journal.cli.doctor.vad.unresolved = der Pfad des { $vad }-Helfers konnte nicht aufgelöst werden: { $cause }

journal.cli.doctor.windows.onnx.helper = { $onnx }-Helfer ist nicht der deklarierte Bestandteil der signierten App-Nutzdaten

journal.cli.doctor.windows.onnx.member = signierte { $onnx }-App-Nutzdaten weisen { $member } zurück: { $error }

journal.cli.doctor.windows.onnx.platform = { $windows }-{ $onnx }-Paketprüfung erfordert eine { $windows }-Laufzeitumgebung

journal.cli.doctor.windows.onnx.verification = die signierten { $onnx }-App-Nutzdaten konnten nicht überprüft werden: { $error }

journal.cli.doctor.windows.parakeet.executable = die laufende Programmdatei des Journals konnte nicht bestimmt werden: { $error }

journal.cli.doctor.windows.parakeet.layout = die laufende Programmdatei des Journals liegt nicht im bin-Verzeichnis des Pakets: { $path }

journal.cli.doctor.windows.parakeet.member = signierte { $parakeet_name }-App-Nutzdaten weisen { $member } zurück: { $error }

journal.cli.doctor.windows.parakeet.parent = die laufende Programmdatei des Journals hat kein übergeordnetes Verzeichnis: { $path }

journal.cli.doctor.windows.parakeet.platform = Windows-{ $parakeet_name }-Paketprüfung erfordert eine Windows-Laufzeitumgebung

journal.cli.doctor.windows.parakeet.root = das bin-Verzeichnis des Pakets hat keine Paketwurzel: { $path }

journal.cli.doctor.windows.parakeet.verification = die signierten { $parakeet_name }-App-Nutzdaten konnten nicht überprüft werden: { $error }

journal.cli.host.arguments.invalid = { $command }: Fehler: ungültige Argumente

journal.cli.host.backfill.facet.ids.help = Verwendung: { $syntax }{ $line_break }{ $line_break }Fehlende { $term }-Kennungen über alle Facetten-Deklarationen hinweg nachtragen.{ $line_break }{ $line_break }Optionen:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_2 }diese Hilfenachricht anzeigen und beenden{ $line_break }{ $spaces_2 }{ $option_6 }{ $spaces_4 }Nachgetragene Facetten-IDs auf den Datenträger schreiben (Standard ist ein Probelauf){ $line_break }

journal.cli.host.backfill.facet.ids.solstone.journal.backfill.facet.ids.error = { $command_solstone_journal_backfill_facet_ids }: Fehler: { $error }

journal.cli.host.backfill.facet.ids.usage = Verwendung: { $syntax }{ $line_break }

journal.cli.host.backfill.facets.applied = { $scanned_quantity ->
    [one] { $backfilled_quantity ->
        [one] Untersucht { $scanned } Facette; eine ID für { $backfilled } Facette nachgetragen.
        *[other] Untersucht { $scanned } Facette; IDs für { $backfilled } Facetten nachgetragen.
        }
    *[other] { $backfilled_quantity ->
        [one] Untersucht { $scanned } Facetten; eine ID für { $backfilled } Facette nachgetragen.
        *[other] Untersucht { $scanned } Facetten; IDs für { $backfilled } Facetten nachgetragen.
        }
    }

journal.cli.host.backfill.facets.dry = { $scanned_quantity ->
    [one] { $backfilled_quantity ->
        [one] Probelauf: untersucht { $scanned } Facette; { $backfilled } Facette erfordert eine ID (gib { $literal_commit } an, um es anzuwenden).
        *[other] Probelauf: untersucht { $scanned } Facette; { $backfilled } Facetten erfordern IDs (gib { $literal_commit } an, um es anzuwenden).
        }
    *[other] { $backfilled_quantity ->
        [one] Probelauf: untersucht { $scanned } Facetten; { $backfilled } Facette erfordert eine ID (gib { $literal_commit } an, um es anzuwenden).
        *[other] Probelauf: untersucht { $scanned } Facetten; { $backfilled } Facetten erfordern IDs (gib { $literal_commit } an, um es anzuwenden).
        }
    }

journal.cli.host.check.help = Verwendung: { $syntax }{ $line_break }{ $line_break }Bereitschaftsurteil für mitgelieferte lokale Journal-Modelle.{ $line_break }{ $line_break }Optionen:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_2 }diese Hilfenachricht anzeigen und beenden{ $line_break }{ $spaces_2 }{ $option_5 }{ $spaces_6 }das Bereitschaftsurteil als { $term } für Agenten ausgeben{ $line_break }

journal.cli.host.check.usage = Verwendung: { $syntax }{ $line_break }

journal.cli.host.config.admit.rewrite.identity.admission = solstone journal config: Identitätszulassung abgelehnt: { $error }

journal.cli.host.config.admit.rewrite.identity.owner = solstone journal config: Identität owner abgelehnt: { $error }

journal.cli.host.config.admit.rewrite.identity.root = solstone journal config: Identitätswurzel abgelehnt: { $error }

journal.cli.host.config.admit.rewrite.path = solstone journal config: Journal-Pfad abgelehnt: { $error }

journal.cli.host.config.corrupt.settings.file.at.path.couldn = deine Einstellungsdatei unter { $path } konnte nicht gelesen werden. deine Einstellungen wurden nicht geändert. repariere die Datei oder stelle { $literal_1 } aus einer Sicherung wieder her, und versuch es dann noch einmal.

journal.cli.host.config.cross.filesystem.cannot.move.across.filesystems.filesystem = solstone journal config: abgelehnt: Verschieben über Dateisysteme hinweg nicht möglich (Dateisystem-IDs { $source_device } und { $target_device }); stoppe dein Journal mit '{ $literal_1 }', kopiere den Journal-Ordner an den neuen Pfad, verwende { $literal_3 }, starte es dann erneut mit '{ $literal_2 }'

journal.cli.host.config.decide.move.requires.not.active.target = solstone journal config: abgelehnt: { $literal_3 } erfordert ein nicht aktives Ziel; aktuell ist { $current_state } und Ziel ist { $target_state }; gültige Flags: { $literal_1 }, { $literal_2 }

journal.cli.host.config.execute.already.set.path = solstone journal config: Journal bereits auf { $path } gesetzt

journal.cli.host.config.execute.before.mutation = solstone journal config: vor der Änderung abgelehnt: { $error }

journal.cli.host.config.execute.warning.force.bypasses.confirmation.and = solstone journal config: Warnung: { $literal_1 } umgeht die Bestätigung und die Prüfungen der Zielaktivität

journal.cli.host.config.existing.target.move.target.already.exists.path = solstone journal config: abgelehnt: Ziel der Verschiebung existiert bereits: { $path }

journal.cli.host.config.help = Verwendung: { $syntax }{ $line_break }{ $spaces_7 }{ $syntax_4 }{ $line_break }{ $line_break }Positionsargumente:{ $line_break }{ $spaces_2 }{ $choices }{ $line_break }{ $spaces_4 }{ $subcommand }{ $spaces_10 }den konfigurierten Journal-Pfad und die Quelle anzeigen{ $line_break }{ $spaces_4 }{ $subcommand_10 }{ $spaces_2 }den Journal-Pfad ändern, den diese Installation verwendet{ $line_break }

journal.cli.host.config.identity.root.from.current.executable = solstone journal config: { $error }

journal.cli.host.config.journal.path.contains.shell.active.character = solstone journal config: abgelehnt: Journal-Pfad enthält ein shell-aktives Zeichen: { $path }

journal.cli.host.config.journal.path.source.tree.fallback.path = solstone journal config: abgelehnt: { $path } ist der Rückfallpfad des Quellbaums, aber dies ist kein Quell-Checkout

journal.cli.host.config.maybe.restart.current.service.could.not.restart.service.at = Dienst am ursprünglichen Journal-Ordner konnte nicht neu gestartet werden

journal.cli.host.config.missing.current.move.source.does.not.exist = solstone journal config: abgelehnt: Ursprung der Verschiebung existiert nicht: { $path }

journal.cli.host.config.missing.parent.move.target.parent.does.not = solstone journal config: abgelehnt: übergeordnetes Verzeichnis des Verschiebeziels existiert nicht: { $directory }

journal.cli.host.config.plan.action.action = Aktion:  { $action }

journal.cli.host.config.plan.current.left.intact.re.adopt = aktuelles Journal bleibt unverändert. später wieder übernehmen: { $literal_1 } { $path } { $literal_2 } { $literal_3 }

journal.cli.host.config.plan.current.path.state = aktuell: { $path } ({ $state })

journal.cli.host.config.plan.dry.run.yes.nothing.will = Probelauf: ja; es wird nichts geändert

journal.cli.host.config.plan.filesystem.different.devices = Dateisystem: verschiedene Geräte

journal.cli.host.config.plan.filesystem.same.device = Dateisystem: dasselbe Gerät

journal.cli.host.config.plan.move = verschieben

journal.cli.host.config.plan.plan.summary = { $literal_1 } - Planübersicht

journal.cli.host.config.plan.proceed = fortfahren

journal.cli.host.config.plan.re.run.with.yes.proceed = mit { $literal_1 } erneut ausführen, um fortzufahren

journal.cli.host.config.plan.switch = wechseln

journal.cli.host.config.plan.target.path.state = Ziel:  { $path } ({ $state })

journal.cli.host.config.preflight.coordinates.saved.location.not.recognized.or = der gespeicherte Journal-Ort wird nicht erkannt oder konnte nicht gelesen werden; die Einstellungen wurden nicht geändert

journal.cli.host.config.preflight.coordinates.settings.could.not.be.read = deine Einstellungen konnten nicht gelesen werden; die Einstellungen wurden nicht geändert: { $error }

journal.cli.host.config.prepare.folder.relocation.requires.managed.posix = die Verschiebung des Journal-Ordners erfordert eine verwaltete { $literal_1 }-Installation

journal.cli.host.config.read.wrapper.targets.cannot.read.path.e = solstone journal config: abgelehnt: { $path } kann nicht gelesen werden: { $e }

journal.cli.host.config.read.wrapper.targets.location.changed.while.this.command = der Journal-Ort hat sich geändert, während dieser Befehl gewartet hat; versuch die Ordneränderung noch einmal

journal.cli.host.config.refusal.current.current.state.and.target = solstone journal config: abgelehnt: aktuell ist { $current_state } und das Ziel ist { $target_state }; gültige Flags: { $flags }

journal.cli.host.config.rewrite.preflighted.with.path.not.utf8 = Journal-Pfad ist nicht { $literal_1 }

journal.cli.host.config.run.move.could.not.stop.service.before = solstone journal config: der Dienst konnte vor dem Verschieben nicht beendet werden ({ $error })

journal.cli.host.config.run.move.could.not.stop.service.before.stop.service.before.move = solstone journal config: der Dienst konnte vor dem Verschieben nicht beendet werden

journal.cli.host.config.run.move.move.failed.e = solstone journal config: Verschieben fehlgeschlagen: { $e }

journal.cli.host.config.run.move.moved.wrapper.updated.service.restarted = Journal verschoben; Wrapper aktualisiert; Dienst neu gestartet.

journal.cli.host.config.run.move.remains.at.target.could.not = Journal bleibt unter { $target }; der neue Ort konnte nicht fertig gespeichert werden: { $e }; erneut versuchen: { $literal_1 } { $path } { $literal_2 } { $literal_3 }

journal.cli.host.config.run.move.remains.at.target.retry.path = Journal bleibt unter { $target }; erneut versuchen: { $literal_1 } { $path } { $literal_2 } { $literal_3 }

journal.cli.host.config.run.move.service.installed.but.not.running = Dienst installiert, läuft aber nicht; Journal verschoben; Wrapper aktualisiert.

journal.cli.host.config.run.move.service.not.installed.moved.wrapper = Dienst nicht installiert; Journal verschoben; Wrapper aktualisiert.

journal.cli.host.config.run.move.wrapper.updated.target.but.service = Wrapper auf { $target } aktualisiert, aber der Dienststart ist fehlgeschlagen; starte manuell neu

journal.cli.host.config.run.switch.cannot.create.path.e = solstone journal config: abgelehnt: { $path } kann nicht erstellt werden: { $e }

journal.cli.host.config.run.switch.cannot.inspect.managed.wrappers = solstone journal config: abgelehnt: verwaltete Wrapper können nicht untersucht werden: { $error }

journal.cli.host.config.run.switch.cannot.lock.managed.wrappers = solstone journal config: abgelehnt: verwaltete Wrapper können nicht gesperrt werden: { $error }

journal.cli.host.config.run.switch.could.not.finish.saving.new = der neue Journal-Ort konnte nicht fertig gespeichert werden: { $message }; erneut versuchen: { $literal_1 } { $path } { $literal_2 } { $literal_3 }

journal.cli.host.config.run.switch.could.not.finish.saving.new.journal.path.switch.yes = der neue Journal-Ort konnte nicht fertig gespeichert werden: { $e }; erneut versuchen: { $literal_1 } { $path } { $literal_2 } { $literal_3 }

journal.cli.host.config.run.switch.could.not.stop.service.before = der Dienst konnte vor der Änderung des Journal-Ordners nicht beendet werden

journal.cli.host.config.run.switch.now.at.target.could.not = dein Journal ist jetzt unter { $target }; sein Dienst konnte nicht aktualisiert werden: { $error }; erneut versuchen: { $literal_1 } { $path } { $literal_2 } { $literal_3 }

journal.cli.host.config.run.switch.service.installed.but.not.running = Dienst installiert, läuft aber nicht; Wrapper aktualisiert.

journal.cli.host.config.run.switch.service.not.installed.wrapper.updated = Dienst nicht installiert; Wrapper aktualisiert.

journal.cli.host.config.run.switch.wrapper.rewritten.target.but.service = solstone journal config: Wrapper auf { $target } umgeschrieben, aber der Neustart des Journal-Dienstes konnte nicht ausgeführt werden ({ $error }); starte manuell neu

journal.cli.host.config.run.switch.wrapper.rewritten.target.but.service.investigate.and.restart.manually = solstone journal config: Wrapper auf { $target } umgeschrieben, aber '{ $literal_1 }' wurde mit { $code } beendet; untersuche das und starte manuell neu

journal.cli.host.config.run.switch.wrapper.updated.service.restarted = Wrapper aktualisiert; Dienst neu gestartet.

journal.cli.host.config.run.unsupported.folder.service.action = nicht unterstützte Dienstaktion für den Journal-Ordner

journal.cli.host.config.service.summary.service.installed.and.running.will = Dienst: installiert und läuft; wird beenden, verschieben, den Wrapper umschreiben und neu starten

journal.cli.host.config.service.summary.service.installed.and.running.will.will.rewrite.wrapper.restart = Dienst: installiert und läuft; wird den Wrapper umschreiben und neu starten

journal.cli.host.config.service.summary.service.installed.but.not.running = Dienst: installiert, läuft aber nicht; wird verschieben und den Wrapper umschreiben

journal.cli.host.config.service.summary.service.installed.but.not.running.running.will.rewrite.wrapper = Dienst: installiert, läuft aber nicht; wird den Wrapper umschreiben

journal.cli.host.config.service.summary.service.not.installed.will.move = Dienst: nicht installiert; wird verschieben und den Wrapper umschreiben

journal.cli.host.config.service.summary.service.not.installed.will.rewrite = Dienst: nicht installiert; wird den Wrapper umschreiben

journal.cli.host.config.show.next.next = weiter: { $next }

journal.cli.host.config.show.source.built.in.default = eingebauter Standard ({ $literal_1 })

journal.cli.host.config.show.source.source.tree.fallback = Quellbaum-Rückfall

journal.cli.host.config.show.source.user.toml = user-Konfiguration ({ $literal_1 })

journal.cli.host.config.state.active = aktiv

journal.cli.host.config.state.not.active = nicht aktiv

journal.cli.host.config.usage = Verwendung: { $syntax }{ $line_break }{ $spaces_7 }{ $syntax_4 }{ $line_break }

journal.cli.host.config.wrapper.next.move.path.aside.then.run = lege { $path } beiseite und führe dann '{ $literal_1 }' aus dem solstone-Checkout aus

journal.cli.host.config.wrapper.refusal.wrapper.not.managed.wrapper.run = solstone journal config: abgelehnt: { $wrapper } ist kein verwalteter Wrapper (führe '{ $literal_1 }' aus dem solstone-Checkout aus, um den Wrapper zuerst zu installieren)

journal.cli.host.contract.build.bundle.browser.schema.bytes.diverged.from = Vertrag: Browser-Schema-Bytes weichen von { $literal_1 } ab

journal.cli.host.contract.build.bundle.cannot.read.browser.schema.path = Vertrag: Browser-Schema { $path } kann nicht gelesen werden: { $error }

journal.cli.host.contract.build.bundle.invalid.json.schema.path = Vertrag: ungültiges { $literal_1 }-Schema { $path }: { $error }

journal.cli.host.contract.build.bundle.missing.required.schema.sources.sources = Vertrag: erforderliche Schemaquellen fehlen: { $sources }

journal.cli.host.contract.build.bundle.path.duplicate.format.id.format = Vertrag: { $path }: doppelte Format-ID '{ $format_id }' des Journal-Vertrags

journal.cli.host.contract.build.bundle.path.format.id.must.be = Vertrag: { $path }: { $literal_1 } muss eine Zeichenkette sein

journal.cli.host.contract.build.bundle.schema.must.be.json.object = Vertrag: Schema muss ein { $literal_1 }-Objekt sein: { $path }

journal.cli.host.contract.build.help = Verwendung: { $syntax }{ $line_break }{ $line_break }Baue das Journal-Vertragsbündel.{ $line_break }{ $line_break }Optionen:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_12 }diese Hilfenachricht anzeigen und beenden{ $line_break }{ $spaces_2 }{ $option_6 }{ $spaces_15 }Prüfe, ob das Bündel aktuell ist, ohne zu schreiben.{ $line_break }{ $spaces_2 }{ $option_8 }{ $spaces_11 }Vertrags-Checkout oder Wurzel des installierten Pakets.{ $line_break }

journal.cli.host.contract.build.usage = Verwendung: { $syntax }{ $line_break }

journal.cli.host.contract.check.help = Verwendung: { $syntax }{ $line_break }{ $line_break }Prüfe das Bündel und die Journal-Dateien gegen ihre Schemas.{ $line_break }{ $line_break }Optionen:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_12 }diese Hilfenachricht anzeigen und beenden{ $line_break }{ $spaces_2 }{ $option_6 }{ $spaces_8 }Zusätzliche Journal-Wurzel zum Validieren (wiederholbar).{ $line_break }{ $spaces_2 }{ $option_8 }{ $spaces_11 }Vertrags-Checkout oder Wurzel des installierten Pakets.{ $line_break }

journal.cli.host.contract.check.usage = Verwendung: { $syntax }{ $line_break }

journal.cli.host.contract.classify.breaking.changes.bundle.malformed = solstone-Journal-Vertragsbündel ist fehlerhaft

journal.cli.host.contract.classify.breaking.changes.format.removed.format = { $format }: entferntes Format

journal.cli.host.contract.classify.breaking.changes.format.removed.key.field.field = { $format }: entferntes Schlüsselfeld '{ $field }'

journal.cli.host.contract.classify.breaking.changes.format.removed.producer.path.path = { $format }: entfernter Erzeugerpfad '{ $path }'

journal.cli.host.contract.discover.schema.sources.path.missing.meta = Vertrag: { $path }: { $contract_meta } fehlt

journal.cli.host.contract.fixture.journal.fixture.missing.path = Vertrag: Fixture-Journal fehlt: { $path }

journal.cli.host.contract.format.errors.invalid.json.schema = ungültiges { $literal_1 }-Schema: { $error }

journal.cli.host.contract.from.root.package.tree.not.found.path = Vertrag: solstone-Paketbaum nicht gefunden: { $path }

journal.cli.host.contract.help = Verwendung: { $syntax }{ $line_break }{ $line_break }Baue das Journal-Vertragsbündel und validiere es.{ $line_break }{ $line_break }Positionsargumente:{ $line_break }{ $spaces_2 }{ $choices }{ $line_break }{ $spaces_4 }{ $subcommand }{ $spaces_15 }Baue das Vertragsbündel.{ $line_break }{ $spaces_4 }{ $subcommand_8 }{ $spaces_15 }Prüfe das Bündel und die Journal-Dateien.{ $line_break }{ $line_break }Optionen:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_12 }diese Hilfenachricht anzeigen und beenden{ $line_break }

journal.cli.host.contract.load.json.invalid.json.in.path = Vertrag: ungültiges { $literal_1 } in { $path }: { $error }

journal.cli.host.contract.metadata.path.allowed.producers.must.be = Vertrag: { $path }: { $literal_1 } muss eine Liste sein

journal.cli.host.contract.metadata.path.key.fields.must.be = Vertrag: { $path }: { $literal_1 } muss eine Liste sein

journal.cli.host.contract.metadata.path.missing.metadata.key = Vertrag: { $path }: Vertragsmetadaten fehlen: { $key }

journal.cli.host.contract.resolve.could.not.inspect.current.executable = Vertrag: die aktuelle Programmdatei konnte nicht untersucht werden: { $error }

journal.cli.host.contract.resolve.from.executable.could.not.locate.installed.package = Vertrag: installiertes solstone-Paket oder Quell-Checkout konnten von { $path } aus nicht gefunden werden

journal.cli.host.contract.run.build.cannot.create.bundle.directory = Vertrag: Bündelverzeichnis kann nicht erstellt werden: { $error }

journal.cli.host.contract.run.build.cannot.write.path = Vertrag: { $path } kann nicht geschrieben werden: { $error }

journal.cli.host.contract.run.build.path.stale.run.build = { $path } ist veraltet; führe `{ $literal_1 }` aus

journal.cli.host.contract.run.build.wrote.path = { $path } geschrieben

journal.cli.host.contract.run.check.bundle.artifact.missing.path = Vertrag: Bündelartefakt fehlt: { $path }

journal.cli.host.contract.run.check.no.covered.files.found.under = Vertrag: keine vom Vertrag erfassten Dateien unter { $path } gefunden

journal.cli.host.contract.run.check.tree.not.found.path.expected = Vertrag: Journal-Baum nicht gefunden: { $path } (erwartet { $expected })

journal.cli.host.contract.usage = Verwendung: { $syntax }{ $line_break }

journal.cli.host.contract.validate.contract.file.unsupported.file.kind.kind = nicht unterstützte Vertragsdateiart: { $kind }

journal.cli.host.contract.validate.headered.jsonl.headered.jsonl.requires.header.line = { $literal_1 } erfordert eine Kopfzeile

journal.cli.host.contract.validate.headered.jsonl.invalid.utf8 = ungültig ({ $literal_1 }): { $error }

journal.cli.host.contract.validate.headered.jsonl.line.must.be.json.object = Zeile muss ein { $literal_1 }-Objekt sein

journal.cli.host.contract.validate.headered.jsonl.missing.schema.definition = Schemadefinition fehlt

journal.cli.host.contract.validate.json.invalid.json = ungültig ({ $literal_1 }): { $error }

journal.cli.host.contract.validated = { $quantity ->
    [one] { $display_count } vom Vertrag erfasste Datei unter { $root } validiert
   *[other] { $display_count } vom Vertrag erfasste Dateien unter { $root } validiert
    }

journal.cli.host.contract.walk.four.levels.cannot.read.entry = Vertrag: Journal-Eintrag kann nicht gelesen werden: { $error }

journal.cli.host.contract.walk.schema.files.cannot.read.path = Vertrag: { $path } kann nicht gelesen werden: { $error }

journal.cli.host.contract.walk.schema.files.cannot.read.schema.entry = Vertrag: Schemaeintrag kann nicht gelesen werden: { $error }

journal.cli.host.convey.help = Verwendung: { $syntax }{ $line_break }{ $line_break }{ $term }-Weboberfläche{ $line_break }{ $line_break }Optionen:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_5 }diese Hilfenachricht anzeigen und beenden{ $line_break }{ $spaces_2 }{ $option_7 }{ $spaces_4 }Port, auf dem bereitgestellt wird{ $line_break }{ $spaces_2 }{ $option_9 }{ $spaces_2 }Ausführliche Ausgabe aktivieren{ $line_break }{ $spaces_2 }{ $option_10 }{ $spaces_4 }Debug-Protokollierung aktivieren{ $line_break }

journal.cli.host.convey.usage = Verwendung: { $syntax }{ $line_break }

journal.cli.host.cortex.help = Verwendung: { $syntax }{ $line_break }{ $line_break }{ $term } { $term_4 } Talent-Manager{ $line_break }{ $line_break }Optionen:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_5 }diese Hilfenachricht anzeigen und beenden{ $line_break }{ $spaces_2 }{ $option_8 }{ $spaces_2 }Ausführliche Ausgabe aktivieren{ $line_break }{ $spaces_2 }{ $option_9 }{ $spaces_4 }Debug-Protokollierung aktivieren{ $line_break }

journal.cli.host.cortex.service.cortex.service.failed = cortex-Dienst fehlgeschlagen: { $error }

journal.cli.host.cortex.usage = Verwendung: { $syntax }{ $line_break }

journal.cli.host.down.help = Verwendung: { $syntax }{ $line_break }{ $line_break }Beende den Journal-Dienst.{ $line_break }{ $line_break }Optionen:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_2 }diese Hilfenachricht anzeigen und beenden{ $line_break }

journal.cli.host.facet.candidates.help = Verwendung: { $syntax }{ $line_break }{ $line_break }Erfasse wiederkehrende Kandidaten der Facettenprüfung.{ $line_break }{ $line_break }Optionen:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_5 }diese Hilfenachricht anzeigen und beenden{ $line_break }{ $spaces_2 }{ $option_6 }{ $spaces_2 }Ausführliche Ausgabe aktivieren{ $line_break }{ $spaces_2 }{ $option_7 }{ $spaces_4 }Debug-Protokollierung aktivieren{ $line_break }

journal.cli.host.facet.candidates.recorded = { $quantity ->
    [one] { $display_count } Facettenkandidat erfasst/aktualisiert.
   *[other] { $display_count } Facettenkandidaten erfasst/aktualisiert.
    }

journal.cli.host.facet.candidates.run.facet.candidates = solstone journal facet-candidates: Fehler: { $error }

journal.cli.host.facet.candidates.run.facet.candidates.root.path.not = solstone journal facet-candidates: Fehler: Journal-Wurzel { $path } ist kein Verzeichnis

journal.cli.host.facet.candidates.usage = Verwendung: { $syntax }{ $line_break }

journal.cli.host.grab.help = Verwendung: { $syntax }{ $line_break }{ $line_break }Gehe beobachtete Bildschirmbilder durch und schreibe optional Einzelbilder.{ $line_break }{ $line_break }Positionsargumente:{ $line_break }{ $spaces_2 }{ $subcommand }{ $spaces_11 }Pfad-Token: { $path_tokens }{ $line_break }{ $line_break }Optionen:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_5 }diese Hilfenachricht anzeigen und beenden{ $line_break }{ $spaces_2 }{ $option_9 }{ $spaces_6 }Schreibe das ausgewählte Einzelbild hierhin ({ $extension }, { $extension_12 }, { $extension_13 } oder{ $line_break }{ $spaces_17 }{ $extension_15 }).{ $line_break }{ $spaces_2 }{ $option_16 }{ $spaces_8 }Ersetze einen vorhandenen Ausgabepfad.{ $line_break }{ $spaces_2 }{ $option_18 }{ $spaces_9 }Gib { $term } aus statt einer Tabelle oder einer einfachen Ausgabe.{ $line_break }{ $spaces_2 }{ $option_21 }{ $spaces_2 }Ausführliche Ausgabe aktivieren{ $line_break }{ $spaces_2 }{ $option_22 }{ $spaces_4 }Debug-Protokollierung aktivieren{ $line_break }

journal.cli.host.grab.usage = Verwendung: { $syntax }{ $line_break }

journal.cli.host.health.callosum.clients = { $quantity ->
    [one] Callosum: { $clients } Client
   *[other] Callosum: { $clients } Clients
    }

journal.cli.host.health.ced.ready = ced.cpp-Sound-Tag-Engine und Modell sind bereit

journal.cli.host.health.ced.unsupported = ced-Installation: Plattform { $os }/{ $arch } wird nicht unterstützt; ced-Sound-Tag-Assets werden übersprungen

journal.cli.host.health.crash.attempts = { $quantity ->
    [one] { $space_2 }{ $name } { $attempts } Neustartversuch
   *[other] { $space_2 }{ $name } { $attempts } Neustartversuche
    }

journal.cli.host.health.crash.reason = { $quantity ->
    [one] { $space_2 }{ $name } { $attempts } Neustartversuch{ $space_2 }{ $reason }
   *[other] { $space_2 }{ $name } { $attempts } Neustartversuche{ $space_2 }{ $reason }
    }

journal.cli.host.health.crashed = { $line_break }Abgestürzt:{ $line_break }

journal.cli.host.health.heartbeat.ok = Herzschlag: ok{ $line_break }

journal.cli.host.health.heartbeat.stale = { $line_break }Herzschlag: VERALTET ({ $stale })

journal.cli.host.health.held = { $space_2 }zurückgehalten { $partition } { $reason }

journal.cli.host.health.help = Verwendung: { $syntax }{ $line_break }{ $line_break }Zeige den behaltenen Dienstzustand des Supervisors.{ $line_break }{ $line_break }Optionen:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_5 }diese Hilfenachricht anzeigen und beenden{ $line_break }{ $spaces_2 }{ $option_6 }{ $spaces_2 }ausführliche Ausgabe aktivieren{ $line_break }{ $spaces_2 }{ $option_7 }{ $spaces_4 }detaillierte Debug-Ausgabe aktivieren{ $line_break }

journal.cli.host.health.logs.help = Verwendung: { $syntax }{ $line_break }{ $line_break }Zeige betriebliche Dienstprotokolle.{ $line_break }{ $line_break }Optionen:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_12 }diese Hilfenachricht anzeigen und beenden{ $line_break }{ $spaces_2 }{ $option_6 }{ $spaces_18 }Anzahl der anzuzeigenden Zeilen (Standard: 5){ $line_break }{ $spaces_2 }{ $option_8 }{ $spaces_20 }Protokollausgabe folgen{ $line_break }{ $spaces_2 }{ $option_10 }{ $spaces_10 }Zeilen ab { $argument } anzeigen{ $line_break }{ $spaces_2 }{ $option_13 }{ $spaces_8 }Zeilen für einen Dienst anzeigen{ $line_break }{ $spaces_2 }{ $option_15 }{ $spaces_8 }Zeilen anzeigen, die zu { $argument_16 } passen{ $line_break }{ $spaces_2 }{ $option_17 }{ $spaces_9 }ausführliche Ausgabe aktivieren{ $line_break }{ $spaces_2 }{ $option_19 }{ $spaces_11 }detaillierte Debug-Ausgabe aktivieren{ $line_break }

journal.cli.host.health.logs.usage = Verwendung: { $syntax }{ $line_break }

journal.cli.host.health.media.absent = Medienprozessor: meldet nicht

journal.cli.host.health.media.depth.stale = Medienprozessor: { $depth } ausstehend (veraltet, { $seconds } s seit dem Lebenszeichen)

journal.cli.host.health.media.idle = Medienprozessor: im Leerlauf

journal.cli.host.health.media.pending = Medienprozessor: { $depth } ausstehend

journal.cli.host.health.media.stale = Medienprozessor: veraltet ({ $seconds } s seit dem Lebenszeichen)

journal.cli.host.health.not.running = Verbindung kann nicht hergestellt werden: dein Journal läuft nicht{ $line_break }

journal.cli.host.health.queue = { $space_2 }in der Warteschlange { $name } { $count }

journal.cli.host.health.queue.hold = { $space_2 }Warteschlangen-Halt { $reason } { $details }

journal.cli.host.health.rfdetr.assets = Die Objekterkennung ist beeinträchtigt, weil ihre RF-DETR-Assets nicht verfügbar sind. Bildschirmbeschreibungen laufen weiter. Führe `{ $command }` aus, um die RF-DETR-Assets zu prüfen oder zu reparieren.

journal.cli.host.health.rfdetr.package = Objekterkennung ist nicht verfügbar. repariere dein Journal oder installiere es neu.

journal.cli.host.health.rfdetr.ready = rf-detr.cpp-Objekterkennungs-Engine und Modell sind bereit

journal.cli.host.health.rfdetr.unsupported = rf-detr-Installation: Plattform { $os }/{ $arch } wird nicht unterstützt; rf-detr-Objekterkennungs-Assets werden übersprungen

journal.cli.host.health.runtime = Verbindung kann nicht hergestellt werden: Laufzeitumgebung für den Zustand nicht verfügbar: { $message }{ $line_break }

journal.cli.host.health.service.days = { $space_2 }{ $name } pid { $pid }{ $space_2 }Laufzeit { $days } d

journal.cli.host.health.service.days.hours = { $space_2 }{ $name } pid { $pid }{ $space_2 }Laufzeit { $days } d { $hours } h

journal.cli.host.health.service.days.hours.minutes = { $space_2 }{ $name } pid { $pid }{ $space_2 }Laufzeit { $days } d { $hours } h { $minutes } min

journal.cli.host.health.service.days.minutes = { $space_2 }{ $name } pid { $pid }{ $space_2 }Laufzeit { $days } d { $minutes } min

journal.cli.host.health.service.hours = { $space_2 }{ $name } pid { $pid }{ $space_2 }Laufzeit { $hours } h

journal.cli.host.health.service.hours.minutes = { $space_2 }{ $name } pid { $pid }{ $space_2 }Laufzeit { $hours } h { $minutes } min

journal.cli.host.health.service.minutes = { $space_2 }{ $name } pid { $pid }{ $space_2 }Laufzeit { $minutes } min

journal.cli.host.health.service.seconds = { $space_2 }{ $name } pid { $pid }{ $space_2 }Laufzeit { $seconds } s

journal.cli.host.health.services = Dienste:{ $line_break }

journal.cli.host.health.socket.encoding = Verbindung kann nicht hergestellt werden: der callosum-Socket-Pfad ist kein gültiges UTF-8{ $line_break }

journal.cli.host.health.socket.inspect = Verbindung kann nicht hergestellt werden: der callosum-Socket kann nicht untersucht werden: { $reason }{ $line_break }

journal.cli.host.health.socket.missing = Verbindung kann nicht hergestellt werden: callosum-Socket unter { $path } nicht gefunden{ $line_break }

journal.cli.host.health.socket.type = Pfad ist kein Unix-Socket

journal.cli.host.health.status.error = Verbindung kann nicht hergestellt werden: Fehler im Supervisor-Status: { $service }: { $reason }{ $line_break }

journal.cli.host.health.status.invalid = Verbindung kann nicht hergestellt werden: der Supervisor hat unter { $path } ungültige Status-Nutzdaten gesendet

journal.cli.host.health.task.running = { $space_2 }{ $name } { $duration } s

journal.cli.host.health.task.slow = { $space_2 }{ $name } { $duration } s{ $space_2 }LANGSAM (Obergrenze { $limit } s)

journal.cli.host.health.task.stuck = { $space_2 }{ $name } { $duration } s{ $space_2 }HÄNGT FEST (Obergrenze { $limit } s)

journal.cli.host.health.tasks = Aufgaben:{ $line_break }

journal.cli.host.health.tasks.empty = Aufgaben: nichts{ $line_break }

journal.cli.host.health.timeout = Zeitüberschreitung beim Warten auf den Supervisor-Status (10 s){ $line_break }

journal.cli.host.health.timeout.malformed.frames = { $quantity ->
    [one] Zeitüberschreitung beim Warten auf den Supervisor-Status (10 s; { $count } fehlerhafter Frame fallengelassen){ $line_break }
   *[other] Zeitüberschreitung beim Warten auf den Supervisor-Status (10 s; { $count } fehlerhafte Frames fallengelassen){ $line_break }
}

journal.cli.host.health.usage = Verwendung: { $syntax }{ $line_break }

journal.cli.host.heartbeat.error = { $command }: { $detail }

journal.cli.host.heartbeat.help = Verwendung: { $syntax }{ $line_break }{ $line_break }Führe einen deterministischen Reparaturdurchlauf für den Zustand aus{ $line_break }{ $line_break }Optionen:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_2 }diese Hilfenachricht anzeigen und beenden{ $line_break }{ $spaces_2 }{ $option_5 }{ $spaces_5 }Führe eine vollständige Prüfung unabhängig vom Alter der letzten Prüfung aus{ $line_break }

journal.cli.host.heartbeat.oplog = heartbeat-oplog konnte nicht erstellt werden: { $detail }

journal.cli.host.heartbeat.usage = Verwendung: { $syntax }{ $line_break }

journal.cli.host.hosted.service.hosted.service.admission.refused = Zulassung des gehosteten { $service }-Dienstes abgelehnt: { $error }

journal.cli.host.importer.desktop.run.resolved.could.not.show.import.day = der Hinweis zum Importtag konnte nicht angezeigt werden: { $error }{ $line_break }

journal.cli.host.importer.desktop.run.resolved.could.not.show.import.size = die Importgröße konnte nicht angezeigt werden: { $error }{ $line_break }

journal.cli.host.install.models.coreml.disclosure.parakeet.assets.downloading.parakeet.tdt = parakeet-Assets: das { $model }-{ $literal_4 }-Modell ({ $literal_3 }) wird von { $literal_2 } heruntergeladen. siehe { $literal_1 }.

journal.cli.host.install.models.help = Verwendung: { $syntax }{ $line_break }{ $spaces_30 }{ $syntax_4 }{ $line_break }{ $spaces_30 }{ $syntax_5 }{ $line_break }{ $line_break }mitgelieferte { $argument }-Modelle von { $term } installieren und prüfen (lokale { $argument_8 } plus mitgelieferte{ $line_break }{ $term_9 }{ $path }-Assets). die Standardaktion prüft die lokalen { $argument_8 }-Artefakte und{ $line_break }holt sie, wenn sie fehlen; { $flag } holt erneut; { $flag_12 } prüft nur und beendet{ $line_break }bei jedem Problem mit einem Wert ungleich null.{ $line_break }{ $line_break }Optionen:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_12 }diese Hilfenachricht anzeigen und beenden{ $line_break }{ $spaces_2 }{ $option_16 }{ $spaces_15 }mitgelieferte Assets und lokale { $argument_8 }-Artefakte prüfen, ohne{ $line_break }{ $spaces_24 }etwas zu holen.{ $line_break }{ $spaces_2 }{ $option_19 }{ $spaces_15 }Bereitschaft ignorieren und erneut holen{ $path_20 } lokale { $argument_8 }{ $line_break }{ $spaces_24 }Artefakte.{ $line_break }{ $spaces_2 }{ $option_21 }{ $spaces_7 }nur mitgelieferte Modelle installieren oder prüfen, die für die{ $line_break }{ $spaces_24 }Journal-Bereitschaft nötig sind; optionale Anbieter nicht holen.{ $line_break }{ $spaces_2 }{ $flag_23 } { $choices }{ $line_break }{ $spaces_24 }Journal-Variante zum Installieren oder Prüfen. { $literal } berücksichtigt{ $line_break }{ $spaces_24 }{ $argument_26 } auf { $literal_27 }{ $path_28 }, danach erkennt { $literal } automatisch.{ $line_break }

journal.cli.host.install.models.install.required.rfdetr.rf.detr.install.unsupported.platform = { $literal_1 }-Installation: Plattform { $os }/{ $arch } wird nicht unterstützt; { $literal_1 }-Objekterkennungs-Assets werden übersprungen

journal.cli.host.install.models.install.with.live.progress.parakeet.model.model.name.percent = parakeet-Modell { $model_name }: { $percent }% ({ $received } / { $total } MB)

journal.cli.host.install.models.install.with.live.progress.parakeet.model.model.name.verifying = parakeet-Modell { $model_name }: Download wird geprüft

journal.cli.host.install.models.install.with.live.progress.parakeet.setup.download.starting = parakeet-Einrichtung: Download startet

journal.cli.host.install.models.install.with.live.progress.parakeet.setup.downloading.required.files = parakeet-Einrichtung: erforderliche Dateien werden heruntergeladen { $percent }% ({ $received } / { $total } MB)

journal.cli.host.install.models.install.with.live.progress.preparing.parakeet.speech.model.model = parakeet-Sprachmodell wird vorbereitet: { $model_name }

journal.cli.host.install.models.parakeet.ready.parakeet.cpp.check.failed.model = { $literal_1 }-Prüfung fehlgeschlagen: Modell fehlt unter { $path }

journal.cli.host.install.models.parakeet.ready.parakeet.cpp.check.failed.name = { $literal_1 }-Prüfung fehlgeschlagen: { $name } fehlt unter { $path }

journal.cli.host.install.models.parakeet.ready.parakeet.cpp.check.failed.name.not.executable.at.path = { $literal_1 }-Prüfung fehlgeschlagen: { $name } ist unter { $path } nicht ausführbar

journal.cli.host.install.models.parakeet.ready.parakeet.model.path.missing = parakeet-Modellpfad fehlt

journal.cli.host.install.models.parakeet.ready.parakeet.path.missing = parakeet-Pfad fehlt

journal.cli.host.install.models.parakeet.without.download.parakeet.install.unsupported.platform.os = parakeet-Installation: Plattform { $os }/{ $arch } wird nicht unterstützt; unterstützt: { $literal_1 }, { $literal_2 }

journal.cli.host.install.models.parakeet.without.download.parakeet.speech.recognition.ships.inside = parakeet: die Spracherkennung ist in der Journal-App unter Windows enthalten, es gibt also nichts herunterzuladen

journal.cli.host.install.models.ready.line.model.ready.path = Modell bereit: { $path }

journal.cli.host.install.models.resolve.variant.invalid.variant.cuda.use.cpu = ungültig ({ $literal_1 }={ $literal_2 }); verwende { $literal_3 } auf { $os }/{ $arch }

journal.cli.host.install.models.resolve.variant.invalid.variant.value.use.cpu = ungültig ({ $literal_1 }='{ $value }'); verwende { $literal_3 } oder { $literal_2 }

journal.cli.host.install.models.resolve.variant.variant.coreml.not.supported.on = Variante { $literal_1 } wird auf { $os } nicht unterstützt

journal.cli.host.install.models.resolve.variant.variant.coreml.not.supported.on.supported.on.os.arch = Variante { $literal_1 } wird auf { $os }/{ $arch } nicht unterstützt

journal.cli.host.install.models.resolve.variant.variant.cpu.not.supported.on = Variante { $literal_1 } wird auf { $os } nicht unterstützt

journal.cli.host.install.models.resolve.variant.variant.cpu.not.supported.on.supported.on.os.arch = Variante { $literal_1 } wird auf { $os }/{ $arch } nicht unterstützt

journal.cli.host.install.models.resolve.variant.variant.cuda.not.supported.on = Variante { $literal_1 } wird auf { $os } nicht unterstützt

journal.cli.host.install.models.resolve.variant.variant.cuda.not.supported.on.supported.on.os.arch = Variante { $literal_1 } wird auf { $os }/{ $arch } nicht unterstützt

journal.cli.host.install.models.rfdetr.bundled.asset.disclosure.rf.detr.assets.verifying.bundled = rf-detr-Assets: die mitgelieferte { $literal_2 } { $version }-Engine ({ $literal_3 }) und die { $literal_4 } nano { $literal_5 }-Gewichte ({ $literal_3 }) werden geprüft. siehe { $literal_1 }.

journal.cli.host.install.models.rfdetr.repair.object.detection.unavailable.repair.or = Objekterkennung ist nicht verfügbar. repariere dein Journal oder installiere es neu. { $error }

journal.cli.host.install.models.run.inner.speech.install.already.running.wait = die Sprachinstallation läuft bereits; warte, bis sie fertig ist

journal.cli.host.install.models.run.inner.with.bundled.asset.verification.failed = Prüfung der mitgelieferten Assets fehlgeschlagen: { $error }

journal.cli.host.install.models.run.inner.with.observing.parakeet.install.state.suffix = parakeet-Installation wird beobachtet: { $state }{ $suffix }

journal.cli.host.install.models.run.inner.with.parakeet.install.already.running.for = parakeet-Installation läuft bereits für ein anderes Ziel

journal.cli.host.install.models.run.inner.with.parakeet.install.failed = parakeet-Installation fehlgeschlagen

journal.cli.host.install.models.run.inner.with.timed.out.observing.parakeet.install = Zeitüberschreitung beim Beobachten der parakeet-Installation

journal.cli.host.install.models.usage = Verwendung: { $syntax }{ $line_break }{ $spaces_30 }{ $syntax_4 }{ $line_break }{ $spaces_30 }{ $syntax_5 }{ $line_break }

journal.cli.host.install.provider.dispatch.message.provider.install.failed = { $provider }-Installation fehlgeschlagen

journal.cli.host.install.provider.global.local.model.assets.downloading.llama = lokale Modell-Assets: die { $literal_4 }-Laufzeitumgebung ({ $literal_7 }; der { $literal_6 }-Build enthält außerdem { $literal_5 }-lizenzierte Laufzeitkomponenten) und das Modell ({ $literal_3 }) werden von { $literal_2 } heruntergeladen. siehe { $literal_1 }.

journal.cli.host.install.provider.global.local.thinking.downloads.model.and = lokales Denken lädt das Modell und den Vision-Projektor ({ $literal_3 }) bei Bedarf von { $literal_2 } herunter. die Laufzeitumgebung ist im Journal enthalten. siehe { $literal_1 }.

journal.cli.host.install.provider.global.local.thinking.downloads.model.apache = lokales Denken lädt das Modell ({ $literal_3 }) von { $literal_2 } herunter. die { $literal_4 }-Laufzeitumgebung ist im Journal enthalten. siehe { $literal_1 }.

journal.cli.host.install.provider.global.parakeet.cpp.and.its.speech = { $literal_2 } und das zugehörige Sprachmodell sind unter Windows im Journal enthalten, es gibt also nichts zu installieren. siehe { $literal_1 }.

journal.cli.host.install.provider.global.parakeet.cpp.can.t.run = { $literal_1 } kann auf einem Mac nicht laufen. ein Mac transkribiert stattdessen mit seinem { $literal_2 }-Modell.

journal.cli.host.install.provider.global.parakeet.cpp.fetches.two.artifacts = { $literal_4 } holt zwei Artefakte in den Anbieter-Zwischenspeicher dieses Journals, bevor der Lauf möglich ist, beide von { $literal_2 }: die { $literal_3 }-Server-Binärdatei ({ $literal_6 }) und das Sprachmodell ({ $literal_5 }). siehe { $literal_1 }.

journal.cli.host.install.provider.help = Verwendung: { $syntax }{ $line_break }{ $line_break }Installiere eine Anbieter-Laufzeitumgebung oder versuche es erneut.{ $line_break }{ $line_break }Positionsargumente:{ $line_break }{ $spaces_2 }{ $subcommand }{ $spaces_8 }Zu installierender Anbieter: '{ $literal }' oder '{ $literal_7 }'.{ $line_break }{ $line_break }Optionen:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_2 }diese Hilfenachricht anzeigen und beenden{ $line_break }

journal.cli.host.install.provider.local.target.sha.local.fingerprint.dispatch.omitted.target = lokaler Fingerabdruck-Dispatch hat target_fingerprint_sha256 ausgelassen

journal.cli.host.install.provider.observe.existing.with.provider.install.already.running.for = { $provider }-Installation läuft bereits für ein anderes Ziel

journal.cli.host.install.provider.observe.existing.with.timed.out.observing.provider.install = Zeitüberschreitung beim Beobachten der { $provider }-Installation

journal.cli.host.install.provider.progress.line.observing.provider.install.state.suffix = { $provider }-Installation wird beobachtet: { $state }{ $suffix }

journal.cli.host.install.provider.run.inner.with.parakeet.already.installed = parakeet bereits installiert

journal.cli.host.install.provider.run.inner.with.unsupported.provider.provider.supported.local = nicht unterstützter Anbieter { $provider }; unterstützt: { $literal_1 }

journal.cli.host.install.provider.run.local.inner.with.platform.and.target.sha.local.already.installed = local bereits installiert

journal.cli.host.install.provider.unavailable.status.error.could.not.read.persisted.provider = der gespeicherte { $provider }-Installationsstatus konnte nicht gelesen werden: { $error }

journal.cli.host.install.provider.usage = Verwendung: { $syntax }{ $line_break }

journal.cli.host.installation.executable = aktuelle Programmdatei konnte nicht bestimmt werden: { $error }

journal.cli.host.installation.executable.parent = aktuelle Programmdatei hat kein übergeordnetes Verzeichnis

journal.cli.host.installation.recovery = diese Installation konnte nicht überprüft werden.{ $line_break }führe `{ $setup }` aus, um sie zu prüfen. wenn die Ersteinrichtung erfolgreich abschließt, versuch es noch einmal.{ $line_break }Details: { $detail }

journal.cli.host.installation.root = Wurzel der Installationsidentität konnte von { $directory } aus nicht bestimmt werden

journal.cli.host.logs.count = ungültige Anzahl: { $detail }

journal.cli.host.logs.follow.descriptor = Katalog für { $path } fehlgeschlagen: der oplog-Deskriptor hat sich beim Folgen geändert

journal.cli.host.logs.follow.empty = Keine Protokolldateien gefunden.

journal.cli.host.logs.follow.failed = { $operation } für { $path } fehlgeschlagen

journal.cli.host.logs.follow.root = Katalog für { $path } fehlgeschlagen: Journal-Wurzel nicht verfügbar

journal.cli.host.logs.follow.source = { $operation } für { $path } fehlgeschlagen: { $source }

journal.cli.host.logs.grep.compile = Regex-Muster konnte nicht kompiliert werden

journal.cli.host.logs.grep.invalid = ungültiges Regex-Muster bei Byte-Offset { $offset }

journal.cli.host.logs.grep.unsupported = nicht unterstütztes Regex-Merkmal '{ $family }' bei Byte-Offset { $offset }

journal.cli.host.logs.health.error = Zustandsprotokolle: { $detail }

journal.cli.host.logs.health.usage = { $command }: Fehler: { $detail }

journal.cli.host.logs.service.empty = === Dienstprotokolle === (keine oplog-Blätter)

journal.cli.host.logs.service.error = Dienstprotokolle: { $detail }

journal.cli.host.logs.service.heading = === Dienstprotokolle ===

journal.cli.host.logs.service.operation = { $operation } fehlgeschlagen: { $source }

journal.cli.host.logs.service.unsupported = Fehler: nicht unterstützte Plattform '{ $platform }'

journal.cli.host.logs.since.invalid = Ungültige Zeit: '{ $input }'. Verwende z. B. { $examples }

journal.cli.host.logs.since.overflow = Python-int zu groß, um in ein C-int umgewandelt zu werden

journal.cli.host.main.windows.service.admission.failed = Windows-Dienst-Zulassung fehlgeschlagen: { $error }

journal.cli.host.malformed.jsonl.warning.solstone.observe.utils.invalid.json.at.line.in = { $literal_warning_solstone_observe_utils }:Ungültiges { $literal_json } in Zeile { $line } in { $path }: { $error }

journal.cli.host.mcp.activity.activity = { $command_solstone_journal_mcp_activity }: { $error }

journal.cli.host.mcp.activity.activity.is.not.compiled.into.this.build = { $command_solstone_journal_mcp_activity } ist in diesem Build nicht einkompiliert

journal.cli.host.mcp.activity.activity.unknown.outcome = { $command_solstone_journal_mcp_activity }: unbekanntes Ergebnis { $value }

journal.cli.host.mcp.activity.activity.unknown.tool = { $command_solstone_journal_mcp_activity }: unbekanntes Tool { $value }

journal.cli.host.mcp.activity.bytes = { $space_4 }Bytes:{ $space_3 }{ $byte_count }

journal.cli.host.mcp.activity.created = { $space_4 }erstellt: { $created }

journal.cli.host.mcp.activity.help = Verwendung: { $syntax }{ $line_break }{ $spaces_37 }{ $syntax_4 }{ $line_break }{ $spaces_37 }{ $syntax_5 }{ $line_break }{ $line_break }Anzeigen, was Verbindungen angefragt haben und wie jede Anfrage endete.{ $line_break }{ $line_break }Optionen:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_11 }diese Hilfenachricht anzeigen und beenden{ $line_break }{ $spaces_2 }{ $option_9 }{ $spaces_8 }nur eine Bearer-Token-Verbindung, nach ihrer Beschriftung{ $line_break }{ $spaces_2 }{ $option_11 }{ $spaces_4 }nur eine { $term }-Verbindung, nach ihrer Client-ID{ $line_break }{ $spaces_2 }{ $option_14 }{ $spaces_2 }mit { $flag }, welche Gewährung, wenn eine Client-ID mehrere hat{ $line_break }{ $spaces_2 }{ $option_16 }{ $spaces_10 }nur Anfragen an dieses Tool{ $line_break }{ $spaces_2 }{ $option_18 }{ $spaces_4 }nur Anfragen, die so geendet sind{ $line_break }{ $spaces_2 }{ $option_19 }{ $spaces_2 }ab diesem Tag{ $line_break }{ $spaces_2 }{ $option_20 }{ $spaces_4 }bis einschließlich dieses Tages{ $line_break }{ $spaces_2 }{ $option_21 }{ $spaces_12 }höchstens N Anfragen, 1 bis 1000 (Standard: 20){ $line_break }{ $spaces_2 }{ $option_23 }{ $spaces_15 }{ $term_25 } ausgeben{ $line_break }

journal.cli.host.mcp.activity.no.match = { $examined_quantity ->
    [one] Kein Treffer in { $examined } untersuchtem Datensatz; weiter zurück kann es mehr geben.
    *[other] Kein Treffer in { $examined } untersuchten Datensätzen; weiter zurück kann es mehr geben.
    }

journal.cli.host.mcp.activity.no.recorded.mcp.activity.matches = Keine aufgezeichnete { $literal_mcp }-Aktivität passt.

journal.cli.host.mcp.activity.origin = { $space_4 }Herkunft:{ $space_2 }{ $origin }

journal.cli.host.mcp.activity.raise.limit.or.narrow.day.from.day.to.to.see = Erhöhe { $literal_limit } oder grenze { $literal_day_from }/{ $literal_day_to } ein, um mehr zu sehen.

journal.cli.host.mcp.activity.reason = { $space_4 }Grund:{ $space_2 }{ $reason }

journal.cli.host.mcp.activity.record = { $space_4 }Datensatz:{ $space_2 }{ $day }/{ $segment }

journal.cli.host.mcp.activity.request = { $space_4 }Anfrage: { $rendered }

journal.cli.host.mcp.activity.request.arguments.exceeded.the.record.bound.and.were.not.stored = { $space_4 }Anfrage: (Argumente haben die Datensatzgrenze überschritten und wurden nicht gespeichert)

journal.cli.host.mcp.activity.request.legacy = { $space_4 }Anfrage: (Schema 1: nicht aufgezeichnet)

journal.cli.host.mcp.activity.result = { $count_quantity ->
    [one] { $space_4 }Ergebnis:{ $space_2 }{ $count } Ergebnis, Digest { $digest }
    *[other] { $space_4 }Ergebnis:{ $space_2 }{ $count } Ergebnisse, Digest { $digest }
    }

journal.cli.host.mcp.activity.result.targets = { $count_quantity ->
    [one] { $space_4 }Ergebnis:{ $space_2 }{ $count } Ergebnis, Digest { $digest }, Ziele { $targets }
    *[other] { $space_4 }Ergebnis:{ $space_2 }{ $count } Ergebnisse, Digest { $digest }, Ziele { $targets }
    }

journal.cli.host.mcp.activity.result.truncated = { $count_quantity ->
    [one] { $space_4 }Ergebnis:{ $space_2 }{ $count } Ergebnis, Digest { $digest }, Ziele { $targets }, ...
    *[other] { $space_4 }Ergebnis:{ $space_2 }{ $count } Ergebnisse, Digest { $digest }, Ziele { $targets }, ...
    }

journal.cli.host.mcp.activity.schema.connection = (Schema 1: keine Verbindung aufgezeichnet)

journal.cli.host.mcp.activity.shown.complete = { $shown_quantity ->
    [one] { $shown } Datensatz angezeigt von { $examined } untersucht. { $counts }
    *[other] { $shown } Datensätze angezeigt von { $examined } untersucht. { $counts }
    }

journal.cli.host.mcp.activity.shown.partial = { $shown_quantity ->
    [one] { $shown } Datensatz angezeigt von { $examined } untersucht (weiter zurück kann es mehr geben). { $counts }
    *[other] { $shown } Datensätze angezeigt von { $examined } untersucht (weiter zurück kann es mehr geben). { $counts }
    }

journal.cli.host.mcp.activity.unreadable = { $count_quantity ->
    [one] { $count } Datensatz im Bereich konnte nicht eingelesen werden und wird oben nicht mitgezählt.
    *[other] { $count } Datensätze im Bereich konnten nicht eingelesen werden und werden oben nicht mitgezählt.
    }

journal.cli.host.mcp.help = Verwendung: { $syntax }{ $line_break }{ $line_break }Optionen:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_12 }diese Hilfenachricht anzeigen und beenden{ $line_break }{ $line_break }Unterbefehle:{ $line_break }{ $spaces_2 }{ $subcommand }{ $spaces_15 }Den { $term }-endpoint ausführen{ $line_break }{ $spaces_2 }{ $subcommand_9 }{ $spaces_12 }Die Verbindung für Agenten auf diesem Computer ausführen{ $line_break }{ $spaces_2 }{ $subcommand_10 }{ $spaces_16 }Status des { $term }-endpoints anzeigen{ $line_break }{ $spaces_2 }{ $subcommand_12 }{ $spaces_17 }{ $term }-Bearer-Tokens verwalten{ $line_break }{ $spaces_2 }{ $subcommand_14 }{ $spaces_15 }Den Kopplungscode des Agenten verwalten{ $line_break }{ $spaces_2 }{ $subcommand_15 }{ $spaces_17 }Registrierte { $term_16 }-Clients verwalten{ $line_break }{ $spaces_2 }{ $subcommand_17 }{ $spaces_12 }Leseberechtigungen von Verbindungen verwalten{ $line_break }{ $spaces_2 }{ $subcommand_18 }{ $spaces_14 }Anzeigen, was Verbindungen angefragt haben und wie jede Anfrage endete{ $line_break }{ $spaces_2 }{ $subcommand_20 }{ $spaces_17 }Einen { $term }-Lesevorgang aufrufen, ohne einen Port zu öffnen{ $line_break }

journal.cli.host.mcp.local.door.help = Verwendung: { $syntax }{ $line_break }{ $line_break }Die Verbindung für Agenten auf diesem Computer ausführen.{ $line_break }{ $line_break }Optionen:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_2 }diese Hilfenachricht anzeigen und beenden{ $line_break }

journal.cli.host.mcp.local.door.local.door.is.not.compiled.into.this.build = { $command_solstone_journal_mcp_local_door } ist in diesen Build nicht einkompiliert

journal.cli.host.mcp.local.door.mcp.local.door.failed = mcp local-door fehlgeschlagen: { $class }

journal.cli.host.mcp.oauth.error.detail = { $command_solstone_journal_mcp } { $command } { $operation }: { $error }

journal.cli.host.mcp.oauth.help = Verwendung: { $syntax }{ $line_break }{ $line_break }Registrierte { $term }-Clients verwalten.{ $line_break }{ $line_break }Optionen:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_2 }diese Hilfenachricht anzeigen und beenden{ $line_break }{ $line_break }Unterbefehle:{ $line_break }{ $spaces_2 }{ $subcommand }{ $spaces_8 }Registrierte { $term }-Clients auflisten{ $line_break }{ $spaces_2 }{ $subcommand_8 }{ $spaces_6 }Einen registrierten { $term }-Client widerrufen{ $line_break }

journal.cli.host.mcp.oauth.list.help = Verwendung: { $syntax }{ $line_break }{ $line_break }Registrierte { $term }-Clients auflisten.{ $line_break }{ $line_break }Optionen:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_2 }diese Hilfenachricht anzeigen und beenden{ $line_break }

journal.cli.host.mcp.oauth.oauth.is.not.compiled.into.this.build = { $command_solstone_journal_mcp_oauth } ist in diesen Build nicht einkompiliert

journal.cli.host.mcp.oauth.oauth.revoke.no.such.oauth.client = { $command_solstone_journal_mcp_oauth_revoke }: kein solcher { $literal_oauth }-Client

journal.cli.host.mcp.oauth.revoke.help = Verwendung: { $syntax }{ $line_break }{ $line_break }Einen registrierten { $term }-Client widerrufen.{ $line_break }{ $line_break }Optionen:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_13 }diese Hilfenachricht anzeigen und beenden{ $line_break }{ $spaces_2 }{ $option_7 }{ $spaces_2 }der zu widerrufende Client{ $line_break }

journal.cli.host.mcp.oauth.revoked.oauth.client = { $literal_oauth }-Client { $client_id } widerrufen.

journal.cli.host.mcp.pairing.at.expires.at = Läuft ab um { $expires_at }.

journal.cli.host.mcp.pairing.at.it.works.only.at.your.own.domain = Es gilt nur für deine eigene Domäne.

journal.cli.host.mcp.pairing.at.it.works.only.for.an.agent.on.this.computer = Es gilt nur für einen Agenten auf diesem Computer.

journal.cli.host.mcp.pairing.at.it.works.only.for.an.agent.on.this.computer.to = Es gilt nur für einen Agenten auf diesem Computer. Um einen Agenten anders zu verbinden, verwende { $literal_door } lan, { $literal_solstone_me } oder hostname.

journal.cli.host.mcp.pairing.at.it.works.only.for.an.agent.on.your.network = Es gilt nur für einen Agenten in deinem Netzwerk.

journal.cli.host.mcp.pairing.at.it.works.only.through.solstone.me = Es gilt nur über { $literal_solstone_me }.

journal.cli.host.mcp.pairing.at.pairing.generate.no.hostname.is.set.up.for.this.journal = { $command_solstone_journal_mcp_pairing_generate }: für dieses Journal ist kein Hostname eingerichtet. richte zuerst einen in der Agenten-App ein.

journal.cli.host.mcp.pairing.at.pairing.generate.this.journal.s.config.could.not.be.read = { $command_solstone_journal_mcp_pairing_generate }: die Konfiguration dieses Journals konnte nicht gelesen werden.

journal.cli.host.mcp.pairing.at.pairing.revoke.no.active.pairing.code.to.revoke = { $command_solstone_journal_mcp_pairing_revoke }: kein aktiver Kopplungscode zum Widerrufen

journal.cli.host.mcp.pairing.at.revoked.the.active.pairing.code = Den aktiven Kopplungscode widerrufen.

journal.cli.host.mcp.pairing.at.save.this.now.it.will.not.be.shown.again.and = Speichere das jetzt. Es wird nicht wieder angezeigt und kann nicht wiederhergestellt werden:

journal.cli.host.mcp.pairing.generate.help = Verwendung: { $syntax }{ $line_break }{ $line_break }Einen Kopplungscode für einen Agenten erzeugen, gültig für 10 Minuten und eine Nutzung. Er wird einmal angezeigt.{ $line_break }{ $line_break }Optionen:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_12 }diese Hilfenachricht anzeigen und beenden{ $line_break }{ $spaces_2 }{ $flag } { $choices }{ $line_break }{ $spaces_24 }wo der Code gilt (Standard: { $literal }, ein Agent auf diesem Computer){ $line_break }

journal.cli.host.mcp.pairing.generated = Kopplungscode erzeugt, gültig für 10 Minuten und eine Nutzung.

journal.cli.host.mcp.pairing.help = Verwendung: { $syntax }{ $line_break }{ $line_break }Den Kopplungscode des Agenten verwalten.{ $line_break }{ $line_break }Optionen:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_2 }diese Hilfenachricht anzeigen und beenden{ $line_break }{ $line_break }Unterbefehle:{ $line_break }{ $spaces_2 }{ $subcommand }{ $spaces_4 }Einen Kopplungscode erzeugen{ $line_break }{ $spaces_2 }{ $subcommand_7 }{ $spaces_6 }Den aktuellen Kopplungscode widerrufen{ $line_break }

journal.cli.host.mcp.pairing.pairing.is.not.compiled.into.this.build = { $command_solstone_journal_mcp_pairing } ist in diesen Build nicht einkompiliert

journal.cli.host.mcp.pairing.revoke.help = Verwendung: { $syntax }{ $line_break }{ $line_break }Den aktuellen Kopplungscode widerrufen.{ $line_break }{ $line_break }Optionen:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_2 }diese Hilfenachricht anzeigen und beenden{ $line_break }

journal.cli.host.mcp.permission.categories = Kategorien: { $categories }

journal.cli.host.mcp.permission.clear.help = Verwendung: { $syntax }{ $line_break }{ $line_break }Die Leseberechtigung einer Verbindung aufheben.{ $line_break }{ $line_break }Optionen:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_11 }diese Hilfenachricht anzeigen und beenden{ $line_break }{ $spaces_2 }{ $option_6 }{ $spaces_8 }eine Bearer-Token-Verbindung, nach ihrer Beschriftung{ $line_break }{ $spaces_2 }{ $option_8 }{ $spaces_4 }eine { $term }-Verbindung, nach ihrer Client-ID{ $line_break }{ $spaces_2 }{ $option_11 }{ $spaces_2 }mit { $flag }, welche Gewährung, wenn eine Client-ID mehrere hat{ $line_break }

journal.cli.host.mcp.permission.cleared.permission.for.connection = Berechtigung für Verbindung { $connection } aufgehoben.

journal.cli.host.mcp.permission.connection = Verbindung: { $connection }

journal.cli.host.mcp.permission.evaluation = Auswertung: { $evaluation }

journal.cli.host.mcp.permission.generation = Generation: { $generation }

journal.cli.host.mcp.permission.help = Verwendung: { $syntax }{ $line_break }{ $line_break }Leseberechtigungen von Verbindungen verwalten.{ $line_break }{ $line_break }Optionen:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_2 }diese Hilfenachricht anzeigen und beenden{ $line_break }{ $line_break }Unterbefehle:{ $line_break }{ $spaces_2 }{ $subcommand }{ $spaces_8 }Leseberechtigungen anzeigen, für jede Verbindung oder eine{ $line_break }{ $spaces_2 }{ $subcommand_7 }{ $spaces_9 }Festlegen, was eine Verbindung lesen darf{ $line_break }{ $spaces_2 }{ $subcommand_9 }{ $spaces_7 }Die Leseberechtigung einer Verbindung aufheben{ $line_break }

journal.cli.host.mcp.permission.no.mcp.connection.permissions.configured = Keine { $literal_mcp }-Verbindungsberechtigungen konfiguriert.

journal.cli.host.mcp.permission.no.permission.was.configured.for.connection = Für Verbindung { $connection } war keine Berechtigung konfiguriert.

journal.cli.host.mcp.permission.permission.clear = { $command_solstone_journal_mcp_permission_clear }: { $error }

journal.cli.host.mcp.permission.permission.management.is.not.compiled.into.this.build = { $command_solstone_journal_mcp_permission }-Verwaltung ist in diesen Build nicht einkompiliert

journal.cli.host.mcp.permission.permission.no.permission.configured = Berechtigung: keine Berechtigung konfiguriert

journal.cli.host.mcp.permission.permission.set = { $command_solstone_journal_mcp_permission_set }: { $error }

journal.cli.host.mcp.permission.permission.set.choose.a.category.from = { $command_solstone_journal_mcp_permission_set }: wähle eine Kategorie aus { $categories }

journal.cli.host.mcp.permission.permission.set.choose.places.precision = { $command_solstone_journal_mcp_permission_set }: wähle einen Detailgrad für Orte aus { $levels }

journal.cli.host.mcp.permission.permission.set.places.precision.requires.places = { $command_solstone_journal_mcp_permission_set }: { $flag } requires { $category_flag }

journal.cli.host.mcp.permission.permission.show = { $command_solstone_journal_mcp_permission_show }: { $error }

journal.cli.host.mcp.permission.places.precision = Ortsdetail: { $precision }

journal.cli.host.mcp.permission.set.help.places.precision = { $spaces_2 }{ $option }{ $spaces_2 }Ortsdetail, wenn { $flag } gesetzt ist: { $values } (Standard: { $default }){ $line_break }

journal.cli.host.mcp.permission.set.help.v2 = Verwendung: { $syntax }{ $line_break }{ $spaces_43 }{ $syntax_4 }{ $line_break }{ $spaces_43 }{ $syntax_5 }{ $line_break }{ $line_break }Festlegen, was eine Verbindung lesen darf.{ $line_break }{ $line_break }Optionen:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_11 }diese Hilfenachricht anzeigen und beenden{ $line_break }{ $spaces_2 }{ $option_9 }{ $spaces_8 }eine Bearer-Token-Verbindung, nach ihrer Beschriftung{ $line_break }{ $spaces_2 }{ $option_11 }{ $spaces_4 }eine { $term }-Verbindung, nach ihrer Client-ID{ $line_break }{ $spaces_2 }{ $option_14 }{ $spaces_2 }mit { $flag }, welche Gewährung, wenn eine Client-ID mehrere hat{ $line_break }{ $spaces_2 }{ $option_16 }{ $spaces_2 }eine Art, die diese Verbindung lesen darf (für jede Art wiederholen){ $line_break }{ $spaces_2 }{ $option_17 }{ $spaces_8 }nur diese Facette, nach Name; für weitere wiederholen (Standard: das ganze Journal){ $line_break }

journal.cli.host.mcp.permission.set.mcp.read.permission.for.connection.generation = { $literal_mcp }-Leseberechtigung für Verbindung { $connection } festgelegt (Generation { $generation }).

journal.cli.host.mcp.permission.show.help = Verwendung: { $syntax }{ $line_break }{ $line_break }Leseberechtigungen anzeigen, für jede Verbindung oder für eine.{ $line_break }{ $line_break }Optionen:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_11 }diese Hilfenachricht anzeigen und beenden{ $line_break }{ $spaces_2 }{ $option_6 }{ $spaces_8 }eine Bearer-Token-Verbindung, nach ihrer Beschriftung{ $line_break }{ $spaces_2 }{ $option_8 }{ $spaces_4 }eine { $term }-Verbindung, nach ihrer Client-ID{ $line_break }{ $spaces_2 }{ $option_11 }{ $spaces_2 }mit { $flag }, welche Gewährung, wenn eine Client-ID mehrere hat{ $line_break }

journal.cli.host.mcp.probe.help = Verwendung: { $syntax }{ $line_break }{ $spaces_34 }{ $flag } { $argument } { $flag_6 } { $term }{ $line_break }{ $line_break }Einen { $term_8 }-Lesevorgang als Verbindung aufrufen, ohne einen Port zu öffnen.{ $line_break }{ $line_break }Optionen:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_11 }diese Hilfenachricht anzeigen und beenden{ $line_break }{ $spaces_2 }{ $option_12 }{ $spaces_8 }eine Bearer-Token-Verbindung, nach ihrer Beschriftung{ $line_break }{ $spaces_2 }{ $option_14 }{ $spaces_4 }eine { $term_16 }-Verbindung, nach ihrer Client-ID{ $line_break }{ $spaces_2 }{ $option_17 }{ $spaces_2 }mit { $flag_18 }, welche Gewährung, wenn eine Client-ID mehrere hat{ $line_break }{ $spaces_2 }{ $option_19 }{ $spaces_10 }das aufzurufende Tool{ $line_break }{ $spaces_2 }{ $option_21 }{ $spaces_5 }die Argumente des Tools, als { $term }-Objekt{ $line_break }

journal.cli.host.mcp.service.help = Verwendung: { $syntax }{ $line_break }{ $line_break }Den { $term }-endpoint ausführen.{ $line_break }{ $line_break }Optionen:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_2 }diese Hilfenachricht anzeigen und beenden{ $line_break }

journal.cli.host.mcp.service.mcp.service.failed = mcp service fehlgeschlagen: { $class }

journal.cli.host.mcp.service.service.is.not.compiled.into.this.build = { $command_solstone_journal_mcp_service } ist in diesen Build nicht einkompiliert

journal.cli.host.mcp.status.capability.compiled.in.true = Fähigkeit einkompiliert: true

journal.cli.host.mcp.status.capability.disabled = Fähigkeit: deaktiviert

journal.cli.host.mcp.status.capability.enabled = Fähigkeit: aktiviert

journal.cli.host.mcp.status.capability.error = Fähigkeit: Fehler

journal.cli.host.mcp.status.help = Verwendung: { $syntax }{ $line_break }{ $line_break }Status des { $term }-endpoints anzeigen.{ $line_break }{ $line_break }Optionen:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_2 }diese Hilfenachricht anzeigen und beenden{ $line_break }

journal.cli.host.mcp.status.lan.disabled = LAN-Tür: aus an Port { $port }

journal.cli.host.mcp.status.lan.enabled = LAN-Tür: aktiviert an Port { $port }

journal.cli.host.mcp.status.local.disabled = lokale Tür: nicht aktiviert bei { $resource }

journal.cli.host.mcp.status.local.enabled = lokale Tür: aktiviert bei { $resource }

journal.cli.host.mcp.status.mcp.capability.config.status.only.this.does.not.report.service = nur { $literal_mcp }-Status zu Fähigkeit und Konfiguration; das meldet nicht, ob der Dienst läuft.

journal.cli.host.mcp.status.status.could.not.read.bearer.token.ledger = { $command_solstone_journal_mcp_status }: der Bearer-Token-Bestand konnte nicht gelesen werden: { $error }

journal.cli.host.mcp.status.status.could.not.read.mcp.capability.configuration = { $command_solstone_journal_mcp_status }: die { $literal_mcp }-Fähigkeitskonfiguration konnte nicht gelesen werden: { $error }

journal.cli.host.mcp.status.status.is.not.compiled.into.this.build = { $command_solstone_journal_mcp_status } ist in diesen Build nicht einkompiliert

journal.cli.host.mcp.status.status.mcp.capability.configuration.is.invalid = { $command_solstone_journal_mcp_status }: die { $literal_mcp }-Fähigkeitskonfiguration ist ungültig: { $error }

journal.cli.host.mcp.status.token.count = Token-Anzahl: { $count }

journal.cli.host.mcp.status.token.unavailable = Token-Anzahl: nicht verfügbar

journal.cli.host.mcp.token.create.help = Verwendung: { $syntax }{ $line_break }{ $line_break }Ein Bearer-Token für eine Verbindung erstellen. Das Token wird einmal angezeigt.{ $line_break }{ $line_break }Optionen:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_5 }diese Hilfenachricht anzeigen und beenden{ $line_break }{ $spaces_2 }{ $option_6 }{ $spaces_2 }ein Name für die Verbindung, für die das Token ist{ $line_break }

journal.cli.host.mcp.token.created.bearer.token.for.label = Bearer-Token für Beschriftung { $label } erstellt.

journal.cli.host.mcp.token.grant.before.the.agent.connects.an.agent.that.connected.first = Gewähre, bevor der Agent sich verbindet. Ein Agent, der sich zuerst verbunden hat, behält seine alte Tool-Liste, bis er sich neu verbindet.

journal.cli.host.mcp.token.help = Verwendung: { $syntax }{ $line_break }{ $line_break }{ $term }-Bearer-Tokens verwalten.{ $line_break }{ $line_break }Optionen:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_2 }diese Hilfenachricht anzeigen und beenden{ $line_break }{ $line_break }Unterbefehle:{ $line_break }{ $spaces_2 }{ $subcommand }{ $spaces_6 }Ein Bearer-Token für eine Verbindung erstellen{ $line_break }{ $spaces_2 }{ $subcommand_8 }{ $spaces_8 }Tokens auflisten{ $line_break }{ $spaces_2 }{ $subcommand_10 }{ $spaces_6 }Ein Token widerrufen{ $line_break }

journal.cli.host.mcp.token.list.help = Verwendung: { $syntax }{ $line_break }{ $line_break }Beschriftung und Erstellungszeitpunkt jedes Tokens auflisten.{ $line_break }{ $line_break }Optionen:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_2 }diese Hilfenachricht anzeigen und beenden{ $line_break }

journal.cli.host.mcp.token.revoke.help = Verwendung: { $syntax }{ $line_break }{ $line_break }Das Bearer-Token für eine Beschriftung widerrufen.{ $line_break }{ $line_break }Optionen:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_5 }diese Hilfenachricht anzeigen und beenden{ $line_break }{ $spaces_2 }{ $option_6 }{ $spaces_2 }die Beschriftung des zu widerrufenden Tokens{ $line_break }

journal.cli.host.mcp.token.revoked.bearer.token.for.label = Bearer-Token für Beschriftung { $label } widerrufen.

journal.cli.host.mcp.token.save.this.token.now.it.will.not.be.shown.again = Speichere dieses Token jetzt. Es wird nicht wieder angezeigt und kann nicht wiederhergestellt werden:

journal.cli.host.mcp.token.token.management.is.not.compiled.into.this.build = { $command_solstone_journal_mcp_token }-Verwaltung ist in diesen Build nicht einkompiliert

journal.cli.host.mcp.token.until.you.grant.it.read.access.an.agent.using.this = Bis du ihm Lesezugriff gewährst, sieht ein Agent mit diesem Token nur { $literal_save_memory } und { $literal_recall_memory }. Damit er dein ganzes Journal lesen kann (füge { $literal_facet } NAME hinzu, um nur diese Facette zu erlauben):

journal.cli.host.mcp.usage = Verwendung: { $syntax }{ $line_break }

journal.cli.host.navigate.error.is.no.longer.supported.put.facet.selection.in.the.destination.url.for = { $command }: Fehler: { $option } wird nicht mehr unterstützt. Leg die Facettenauswahl in die Ziel-{ $term }; zum Beispiel, { $path }

journal.cli.host.navigate.help = Verwendung: { $syntax }{ $line_break }{ $line_break }Den Browser zu einem Pfad führen.{ $line_break }{ $line_break }Positionsargumente:{ $line_break }{ $spaces_2 }{ $subcommand }{ $spaces_18 }{ $term }-Pfad, zu dem geführt wird.{ $line_break }{ $line_break }Optionen:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_12 }diese Hilfenachricht anzeigen und beenden{ $line_break }

journal.cli.host.navigate.print.navigation.navigate.path = Navigation: { $path }

journal.cli.host.navigate.run.navigate.callosum.socket.unavailable.path = solstone journal navigate: Fehler: { $literal_1 } Socket nicht verfügbar: { $path }

journal.cli.host.navigate.run.navigate.failed.encode.callosum.request = solstone journal navigate: Fehler: die { $literal_1 }-Anfrage konnte nicht kodiert werden: { $error }

journal.cli.host.navigate.usage = Verwendung: { $syntax }{ $line_break }

journal.cli.host.open.help = Verwendung: { $syntax }{ $line_break }{ $line_break }öffne dein Journal im Browser mit einem Einmallink.{ $line_break }{ $line_break }Positionsargumente:{ $line_break }{ $spaces_2 }{ $subcommand }{ $spaces_10 }Seite zum Öffnen (Standard: /){ $line_break }{ $line_break }Optionen:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_4 }diese Hilfenachricht anzeigen und beenden{ $line_break }{ $spaces_2 }{ $option_8 }{ $spaces_3 }den Einmallink ausgeben, statt einen Browser zu öffnen{ $line_break }

journal.cli.host.open.mint.could.not.make.link = dein Journal konnte keinen Link erstellen: { $error }

journal.cli.host.open.mint.could.not.make.link.try = dein Journal konnte keinen Link erstellen. versuch es noch einmal.

journal.cli.host.open.mint.response.from.could.not.be = die Antwort von deinem Journal konnte nicht gelesen werden. versuch es noch einmal.

journal.cli.host.open.run.browser.could.not.be.opened = hier konnte kein Browser geöffnet werden. führe diesen Befehl erneut mit { $literal_1 } aus und öffne den Link, den er ausgibt, innerhalb einer Minute.

journal.cli.host.open.run.open.it.from.another.computer = um es von einem anderen Computer zu öffnen, leite zuerst die Ports des Journals über { $literal_1 } weiter und lass dieses Terminal offen:

journal.cli.host.open.run.open.url.path = öffne dein Journal: { $url }{ $path }

journal.cli.host.open.socket.linux.down.isn.t.running.for.this = dein Journal läuft für dieses Konto auf diesem Computer nicht, also gibt es nichts zu öffnen. starte es mit { $literal_1 }.

journal.cli.host.open.socket.linux.old.running.older.than.this.command = dein laufendes Journal ist älter als dieser Befehl. starte es mit { $literal_1 } neu, dann { $literal_2 }, und versuch es noch einmal.

journal.cli.host.open.socket.mac.down.isn.t.running.for.this = dein Journal läuft für dieses Konto auf diesem Computer nicht, also gibt es nichts zu öffnen. öffne die Journal-App, um es zu starten.

journal.cli.host.open.socket.mac.old.running.older.than.this.command = dein laufendes Journal ist älter als dieser Befehl. beende die Journal-App, öffne sie erneut und versuch es noch einmal.

journal.cli.host.open.socket.windows.down.isn.t.running.for.this = dein Journal läuft für dieses Konto auf diesem Computer nicht, also gibt es nichts zu öffnen. starte es über die Journal-App und versuch es noch einmal.

journal.cli.host.open.socket.windows.old.running.older.than.this.command = dein laufendes Journal ist älter als dieser Befehl. starte es über die Journal-App neu und versuch es noch einmal.

journal.cli.host.open.usage = Verwendung: { $syntax }{ $line_break }

journal.cli.host.parser.convey.argument.journal.expected.one.argument = { $command }: Fehler: Argument { $flag }: ein Argument erwartet

journal.cli.host.parser.convey.argument.port.expected.one.argument = { $command }: Fehler: Argument { $flag }: ein Argument erwartet

journal.cli.host.parser.convey.argument.port.invalid.int.value = { $command }: Fehler: Argument { $flag }: ungültiger int-Wert

journal.cli.host.parser.convey.argument.port.invalid.int.value.value = { $command }: Fehler: Argument { $flag }: ungültiger int-Wert: '{ $value }'

journal.cli.host.parser.convey.argument.port.must.be.between.n1.and.n65535 = { $command }: Fehler: Argument { $flag }: muss zwischen 1 und 65535 liegen

journal.cli.host.parser.convey.the.following.arguments.are.required.port = { $command }: Fehler: die folgenden Argumente sind erforderlich: { $flag }

journal.cli.host.parser.convey.unrecognized.arguments.arguments = { $command }: Fehler: unbekannte Argumente: { $arguments }

journal.cli.host.parser.cortex.unrecognized.arguments.arguments = { $command }: Fehler: unbekannte Argumente: { $arguments }

journal.cli.host.parser.direct.port.value.argument.direct.port.invalid.int.value.value = { $command }: Fehler: Argument { $flag }: ungültiger int-Wert: '{ $value }'

journal.cli.host.parser.direct.port.value.argument.direct.port.must.be.between.n1.and.n65535 = { $command }: Fehler: Argument { $flag }: muss zwischen 1 und 65535 liegen

journal.cli.host.parser.grab.argument.out.expected.one.argument = { $command }: Fehler: Argument { $flag }: ein Argument erwartet

journal.cli.host.parser.grab.path.tokens.must.be.valid.unicode = { $command }: Fehler: Pfad-Tokens müssen gültiges Unicode sein

journal.cli.host.parser.grab.unrecognized.arguments.arguments = { $command }: Fehler: unbekannte Argumente: { $arguments }

journal.cli.host.parser.mcp.activity.expected.argument.after.name = { $command }: Fehler: Argument nach { $name } erwartet

journal.cli.host.parser.mcp.activity.limit.must.be.between.n1.and.n1000 = { $command }: Fehler: { $flag } muss zwischen 1 und 1000 liegen

journal.cli.host.parser.mcp.activity.must.be.nonempty.name = { $command }: Fehler: { $name } darf nicht leer sein

journal.cli.host.parser.mcp.activity.unknown.option.accepted.token.oauth.created.tool.outcome.day.from.day.to.limit.json.other = { $command }: Fehler: unbekannte Option { $other }; zulässig: { $flag }, { $flag_2 }, { $flag_3 }, { $flag_4 }, { $flag_5 }, { $flag_6 }, { $flag_7 }, { $flag_8 }, { $flag_9 }

journal.cli.host.parser.mcp.invalid.mcp.command.value = { $command }: Fehler: ungültiger { $term }-Befehl: { $value }

journal.cli.host.parser.mcp.oauth.client.id.client.id.must.be.valid.utf.n8.and.nonempty = { $command }: Fehler: Client-ID muss gültiges { $term } sein und darf nicht leer sein

journal.cli.host.parser.mcp.oauth.client.id.expected.exactly.one.client.id.client.id.argument = { $command }: Fehler: genau ein { $flag } { $argument }-Argument erwartet

journal.cli.host.parser.mcp.oauth.client.id.the.following.arguments.are.required.client.id.client.id = { $command }: Fehler: die folgenden Argumente sind erforderlich: { $flag } { $argument }

journal.cli.host.parser.mcp.oauth.invalid.mcp.oauth.command.value = { $command }: Fehler: ungültiger { $term }-oauth-Befehl: { $value }

journal.cli.host.parser.mcp.oauth.the.following.arguments.are.required.list.revoke = { $command }: Fehler: die folgenden Argumente sind erforderlich: { $literal }, { $literal_2 }

journal.cli.host.parser.mcp.pairing.expected.optional.door.local.lan.solstone.me.hostname.argument = { $command }: Fehler: optionales { $syntax }-Argument erwartet

journal.cli.host.parser.mcp.pairing.invalid.mcp.pairing.command.value = { $command }: Fehler: ungültiger { $term }-pairing-Befehl: { $value }

journal.cli.host.parser.mcp.pairing.the.following.arguments.are.required.generate.revoke = { $command }: Fehler: die folgenden Argumente sind erforderlich: { $literal }, { $literal_2 }

journal.cli.host.parser.mcp.pairing.unknown.door.choose.one.of.local.lan.solstone.me.hostname = { $command }: Fehler: unbekannte Tür, wähle eines von { $literal }, { $literal_2 }, { $literal_3 }, { $literal_4 }

journal.cli.host.parser.mcp.permission.invalid.mcp.permission.command.value = { $command }: Fehler: ungültiger { $term }-permission-Befehl: { $value }

journal.cli.host.parser.mcp.permission.set.at.least.one.category.is.required = { $command }: Fehler: mindestens ein { $flag } ist erforderlich

journal.cli.host.parser.mcp.permission.set.category.must.be.nonempty = { $command }: Fehler: Kategorie darf nicht leer sein

journal.cli.host.parser.mcp.permission.set.category.must.be.valid.utf.n8.and.nonempty = { $command }: Fehler: Kategorie muss gültiges { $term } sein und darf nicht leer sein

journal.cli.host.parser.mcp.permission.set.expected.argument.after.category = { $command }: Fehler: Argument nach { $flag } erwartet

journal.cli.host.parser.mcp.permission.set.expected.argument.after.facet = { $command }: Fehler: Argument nach { $flag } erwartet

journal.cli.host.parser.mcp.permission.set.expected.argument.after.places.precision = { $command }: Fehler: Argument nach { $flag } erwartet

journal.cli.host.parser.mcp.permission.set.facet.name.must.be.nonempty = { $command }: Fehler: Facettenname darf nicht leer sein

journal.cli.host.parser.mcp.permission.set.facet.name.must.be.valid.utf.n8.and.nonempty = { $command }: Fehler: Facettenname muss gültiges { $term } sein und darf nicht leer sein

journal.cli.host.parser.mcp.permission.set.places.precision.must.be.nonempty = { $command }: Fehler: Ortsgenauigkeit darf nicht leer sein

journal.cli.host.parser.mcp.permission.set.places.precision.must.be.valid.utf.n8.and.nonempty = { $command }: Fehler: Ortsgenauigkeit muss gültiges { $term } sein und darf nicht leer sein

journal.cli.host.parser.mcp.permission.set.places.precision.repeated = { $command }: Fehler: { $flag } darf nur einmal angegeben werden

journal.cli.host.parser.mcp.permission.the.following.arguments.are.required.show.set.clear = { $command }: Fehler: die folgenden Argumente sind erforderlich: { $literal }, { $literal_2 }, { $literal_3 }

journal.cli.host.parser.mcp.probe.arguments.is.required = { $command }: Fehler: { $flag } ist erforderlich

journal.cli.host.parser.mcp.probe.expected.argument.after.arguments = { $command }: Fehler: Argument nach { $flag } erwartet

journal.cli.host.parser.mcp.probe.expected.argument.after.tool = { $command }: Fehler: Argument nach { $flag } erwartet

journal.cli.host.parser.mcp.probe.tool.is.required = { $command }: Fehler: { $flag } ist erforderlich

journal.cli.host.parser.mcp.target.created.cannot.be.used.with.token = { $command }: Fehler: { $flag } kann nicht mit { $flag_2 } verwendet werden

journal.cli.host.parser.mcp.target.created.timestamp.must.be.nonempty = { $command }: Fehler: Erstellungszeitstempel darf nicht leer sein

journal.cli.host.parser.mcp.target.created.timestamp.must.be.valid.utf.n8.and.nonempty = { $command }: Fehler: Erstellungszeitstempel muss gültiges { $term } sein und darf nicht leer sein

journal.cli.host.parser.mcp.target.expected.argument.after.created = { $command }: Fehler: Argument nach { $flag } erwartet

journal.cli.host.parser.mcp.target.expected.argument.after.oauth = { $command }: Fehler: Argument nach { $flag } erwartet

journal.cli.host.parser.mcp.target.expected.argument.after.token = { $command }: Fehler: Argument nach { $flag } erwartet

journal.cli.host.parser.mcp.target.oauth.client.id.must.be.nonempty = { $command }: Fehler: oauth-Client-ID darf nicht leer sein

journal.cli.host.parser.mcp.target.oauth.client.id.must.be.valid.utf.n8.and.nonempty = { $command }: Fehler: oauth-Client-ID muss gültiges { $term } sein und darf nicht leer sein

journal.cli.host.parser.mcp.target.one.of.token.or.oauth.is.required = { $command }: Fehler: one of { $flag } or { $flag_2 } ist erforderlich

journal.cli.host.parser.mcp.target.specify.either.token.or.oauth.not.both = { $command }: Fehler: gib entweder { $flag } oder { $flag_2 } an, nicht beides

journal.cli.host.parser.mcp.target.token.label.must.be.nonempty = { $command }: Fehler: Token-Beschriftung darf nicht leer sein

journal.cli.host.parser.mcp.target.token.label.must.be.valid.utf.n8.and.nonempty = { $command }: Fehler: Token-Beschriftung muss gültiges { $term } sein und darf nicht leer sein

journal.cli.host.parser.mcp.target.unexpected.argument.option = { $command }: Fehler: unerwartetes Argument: { $option }

journal.cli.host.parser.mcp.the.following.arguments.are.required.service.local.door.status.token.pairing.oauth.permission.activity.probe = { $command }: Fehler: die folgenden Argumente sind erforderlich: { $literal }, { $literal_2 }, { $literal_3 }, { $literal_4 }, { $literal_5 }, { $literal_6 }, { $literal_7 }, { $literal_8 }, { $literal_9 }

journal.cli.host.parser.mcp.token.invalid.mcp.token.command.value = { $command }: Fehler: ungültiger { $term }-token-Befehl: { $value }

journal.cli.host.parser.mcp.token.label.expected.exactly.one.label.label.argument = { $command }: Fehler: genau ein { $flag } { $argument }-Argument erwartet

journal.cli.host.parser.mcp.token.label.label.must.be.valid.utf.n8.and.nonempty = { $command }: Fehler: Beschriftung muss gültiges { $term } sein und darf nicht leer sein

journal.cli.host.parser.mcp.token.label.the.following.arguments.are.required.label.label = { $command }: Fehler: die folgenden Argumente sind erforderlich: { $flag } { $argument }

journal.cli.host.parser.mcp.token.the.following.arguments.are.required.create.list.revoke = { $command }: Fehler: die folgenden Argumente sind erforderlich: { $literal }, { $literal_2 }, { $literal_3 }

journal.cli.host.parser.schedule.unrecognized.arguments.arguments = { $command }: Fehler: unbekannte Argumente: { $arguments }

journal.cli.host.parser.spl.invalid.choice.choose.from.service.value = { $command }: Fehler: ungültige Auswahl: { $value } (wähle aus { $literal })

journal.cli.host.parser.spl.the.following.arguments.are.required.service = { $command }: Fehler: die folgenden Argumente sind erforderlich: { $literal }

journal.cli.host.parser.spl.unrecognized.arguments.arguments = { $command }: Fehler: unbekannte Argumente: { $arguments }

journal.cli.host.parser.supervisor.argument.direct.port.cannot.be.repeated = { $command }: Fehler: Argument { $flag }: kann nicht wiederholt werden

journal.cli.host.parser.supervisor.argument.direct.port.expected.one.argument = { $command }: Fehler: Argument { $flag }: ein Argument erwartet

journal.cli.host.parser.supervisor.argument.direct.port.invalid.int.value = { $command }: Fehler: Argument { $flag }: ungültiger int-Wert

journal.cli.host.parser.validate.day.day.must.be.eight.digits.yyyymmdd.got.value = { $command }: Fehler: der Tag muss acht Ziffern haben ({ $argument }), erhalten { $value }

journal.cli.host.process.admission = Zulassung des journal-Prozesses fehlgeschlagen: { $error }

journal.cli.host.read.error.error.solstone.observe.utils.error.reading = { $literal_error_solstone_observe_utils }:Fehler beim Lesen von { $path }: { $error }

journal.cli.host.render.oauth.store.error.no.active.pairing.code = { $command_solstone_journal_mcp } { $command } { $operation }: kein aktiver Kopplungscode

journal.cli.host.render.oauth.store.error.no.such.oauth.client = { $command_solstone_journal_mcp } { $command } { $operation }: kein solcher { $literal_oauth }-Client

journal.cli.host.render.supervisor.host.outcome.supervisor.failed.to.boot = supervisor konnte nicht starten: { $reason }

journal.cli.host.render.supervisor.host.outcome.supervisor.lifecycle.shutdown.failed.cause.readiness.self.heartbeat.identity = Herunterfahren im supervisor-Lebenszyklus fehlgeschlagen: Ursache={ $cause }, Bereitschaft={ $readiness }, self_heartbeat={ $self_heartbeat }, Identität={ $identity }

journal.cli.host.render.token.store.error.token = { $command_solstone_journal_mcp_token } { $operation }: { $error }

journal.cli.host.render.token.store.error.token.a.bearer.token.already.exists.for.label = { $command_solstone_journal_mcp_token } { $operation }: für die Beschriftung { $label } gibt es bereits ein Bearer-Token

journal.cli.host.render.token.store.error.token.invalid.label = { $command_solstone_journal_mcp_token } { $operation }: ungültige Beschriftung: { $error }

journal.cli.host.render.token.store.error.token.no.bearer.token.exists.for.label = { $command_solstone_journal_mcp_token } { $operation }: für die Beschriftung { $label } gibt es kein Bearer-Token

journal.cli.host.resolve.connection.key.solstone.journal.mcp.failed.to.query.oauth.store = { $command_solstone_journal_mcp }: die oauth-Ablage konnte nicht abgefragt werden: { $e }

journal.cli.host.resolve.connection.key.solstone.journal.mcp.failed.to.query.token.store = { $command_solstone_journal_mcp }: die Token-Ablage konnte nicht abgefragt werden: { $e }

journal.cli.host.resolve.connection.key.solstone.journal.mcp.multiple.oauth.grants.matched.client.id.specify = { $command_solstone_journal_mcp }: mehrere { $literal_oauth }-Gewährungen passen zu { $literal_client_id } { $client_id }; gib { $literal_created } { $timestamp_arg } an

journal.cli.host.resolve.connection.key.solstone.journal.mcp.no.bearer.token.exists.for.label = { $command_solstone_journal_mcp }: für die Beschriftung { $label } gibt es kein Bearer-Token

journal.cli.host.resolve.connection.key.solstone.journal.mcp.no.oauth.grant.found.matching.client.id = { $command_solstone_journal_mcp }: keine { $literal_oauth }-Gewährung gefunden, die zu { $literal_client_id } { $client_id } passt

journal.cli.host.schedule.help = Verwendung: { $syntax }{ $line_break }{ $line_break }Geplante Aufgaben anzeigen{ $line_break }{ $line_break }Optionen:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_5 }diese Hilfenachricht anzeigen und beenden{ $line_break }{ $spaces_2 }{ $option_6 }{ $spaces_2 }Ausführliche Ausgabe aktivieren{ $line_break }{ $spaces_2 }{ $option_7 }{ $spaces_4 }Debug-Protokollierung aktivieren{ $line_break }

journal.cli.host.schedule.usage = Verwendung: { $syntax }{ $line_break }

journal.cli.host.sense.help = Verwendung: { $syntax }{ $line_break }{ $spaces_20 }{ $syntax_4 }{ $line_break }{ $spaces_20 }{ $syntax_5 }{ $line_break }{ $line_break }Einheitlicher Dateiprozessor für observe{ $line_break }{ $line_break }Optionen:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_12 }diese Hilfenachricht anzeigen und beenden{ $line_break }{ $spaces_2 }{ $option_9 }{ $spaces_13 }Dateien eines bestimmten Tags verarbeiten ({ $argument }-Format), statt zu beobachten{ $line_break }{ $spaces_2 }{ $option_12 }{ $spaces_2 }Maximal gleichzeitige screen-describe-Aufträge bei { $flag } (Standard: 1).{ $line_break }{ $spaces_2 }{ $flag_14 } { $choices }{ $line_break }{ $spaces_24 }Vorhandene Bildschirm- oder Audio-Ausgaben löschen und neu verarbeiten (erfordert { $flag }){ $line_break }{ $spaces_2 }{ $option_17 }{ $spaces_5 }Auf ein bestimmtes Segment filtern ({ $argument_19 }-Format, erfordert { $flag }){ $line_break }{ $spaces_2 }{ $option_20 }{ $spaces_7 }Auf einen bestimmten Datenstrom filtern (erfordert { $flag }){ $line_break }{ $spaces_2 }{ $option_22 }{ $spaces_13 }Anzeigen, was verarbeitet (oder mit { $flag_14 } gelöscht) würde, ohne etwas zu ändern{ $line_break }{ $spaces_2 }{ $option_23 }{ $spaces_9 }Ausführliche Ausgabe aktivieren{ $line_break }{ $spaces_2 }{ $option_25 }{ $spaces_11 }Debug-Protokollierung aktivieren{ $line_break }

journal.cli.host.sense.sense.batch.failed = sense batch fehlgeschlagen: { $error }

journal.cli.host.sense.sense.parent.loss.admission.failed = Zulassung für sense parent-loss fehlgeschlagen: { $error }

journal.cli.host.sense.service.sense.service.failed = sense service fehlgeschlagen: { $class }

journal.cli.host.sense.usage = Verwendung: { $syntax }{ $line_break }{ $spaces_20 }{ $syntax_4 }{ $line_break }{ $spaces_20 }{ $syntax_5 }{ $line_break }

journal.cli.host.service.argument.port = Fehler: unerwartetes Argument '{ $text }'; gib den Port als { $flag } { $port } an

journal.cli.host.service.argument.range = Fehler: unerwartetes Argument '{ $text }'; gib einen Port zwischen 1 und 65535 als { $flag } { $argument } an

journal.cli.host.service.argument.unexpected = Fehler: unerwartetes Argument '{ $text }'; gib den Port als { $flag } { $argument } an

journal.cli.host.service.conflicting.absent.systemd.state = widersprüchlicher fehlender { $systemd }-Zustand

journal.cli.host.service.could.not.clear.the.service.s.ready.state = der Bereitschaftszustand des Dienstes konnte nicht aufgehoben werden: { $error }

journal.cli.host.service.could.not.find.the.solstone.installation.from = die { $solstone }-Installation konnte von { $path } aus nicht gefunden werden

journal.cli.host.service.could.not.resolve.journal = journal konnte nicht aufgelöst werden

journal.cli.host.service.could.not.resolve.the.solstone.executable = die { $solstone }-Programmdatei konnte nicht aufgelöst werden: { $error }

journal.cli.host.service.could.not.verify.the.saved.installation = die gespeicherte Installation konnte nicht überprüft werden: { $error }

journal.cli.host.service.could.not.verify.the.solstone.installation = die { $solstone }-Installation konnte nicht überprüft werden: { $error }

journal.cli.host.service.could.not.verify.the.solstone.setup.storage = der Speicher der { $solstone }-Ersteinrichtung konnte nicht überprüft werden: { $error }

journal.cli.host.service.couldn.t.confirm.that.the.service.uses.this.journal = das ließ sich nicht bestätigen, dass der Dienst dieses Journal nutzt. führe '{ $setup }' aus, um den gespeicherten Ort zu aktualisieren.

journal.cli.host.service.create.service.directory = Dienstverzeichnis erstellen: { $source }

journal.cli.host.service.create.service.health.directory = Zustandsverzeichnis des Dienstes erstellen: { $source }

journal.cli.host.service.duplicate.systemd.property = doppelte { $systemd }-Eigenschaft: { $key }

journal.cli.host.service.entry.is.not.a.regular.file = Eintrag ist keine reguläre Datei

journal.cli.host.service.error.details = Fehler: { $detail }

journal.cli.host.service.executable.parent = die Programmdatei des Dienstes hat kein übergeordnetes Verzeichnis

journal.cli.host.service.file.absent = { $operation ->
    [change-journal-folder] Wechsel des Journal-Ordners durch den Dienst abgelehnt { $path }: nicht vorhanden
    [install] Installation des Dienstes abgelehnt { $path }: nicht vorhanden
    [operate] Betrieb des Dienstes abgelehnt { $path }: nicht vorhanden
    [restart] Neustart des Dienstes abgelehnt { $path }: nicht vorhanden
    [route-repair] Routenreparatur des Dienstes abgelehnt { $path }: nicht vorhanden
    [status] Status des Dienstes abgelehnt { $path }: nicht vorhanden
    [stop] Stopp des Dienstes abgelehnt { $path }: nicht vorhanden
   *[uninstall] Deinstallation des Dienstes abgelehnt { $path }: nicht vorhanden
    }

journal.cli.host.service.file.foreign = { $operation ->
    [change-journal-folder] Wechsel des Journal-Ordners durch den Dienst abgelehnt { $path }: nicht als Dienstdatei erkannt, die journal erstellt hat
    [install] Installation des Dienstes abgelehnt { $path }: nicht als Dienstdatei erkannt, die journal erstellt hat
    [operate] Betrieb des Dienstes abgelehnt { $path }: nicht als Dienstdatei erkannt, die journal erstellt hat
    [restart] Neustart des Dienstes abgelehnt { $path }: nicht als Dienstdatei erkannt, die journal erstellt hat
    [route-repair] Routenreparatur des Dienstes abgelehnt { $path }: nicht als Dienstdatei erkannt, die journal erstellt hat
    [status] Status des Dienstes abgelehnt { $path }: nicht als Dienstdatei erkannt, die journal erstellt hat
    [stop] Stopp des Dienstes abgelehnt { $path }: nicht als Dienstdatei erkannt, die journal erstellt hat
   *[uninstall] Deinstallation des Dienstes abgelehnt { $path }: nicht als Dienstdatei erkannt, die journal erstellt hat
    }

journal.cli.host.service.file.managed = { $operation ->
    [change-journal-folder] Wechsel des Journal-Ordners durch den Dienst abgelehnt { $path }: verwaltet
    [install] Installation des Dienstes abgelehnt { $path }: verwaltet
    [operate] Betrieb des Dienstes abgelehnt { $path }: verwaltet
    [restart] Neustart des Dienstes abgelehnt { $path }: verwaltet
    [route-repair] Routenreparatur des Dienstes abgelehnt { $path }: verwaltet
    [status] Status des Dienstes abgelehnt { $path }: verwaltet
    [stop] Stopp des Dienstes abgelehnt { $path }: verwaltet
   *[uninstall] Deinstallation des Dienstes abgelehnt { $path }: verwaltet
    }

journal.cli.host.service.file.unknown = { $operation ->
    [change-journal-folder] Wechsel des Journal-Ordners durch den Dienst abgelehnt { $path }: die Dienstdatei konnte nicht überprüft werden: { $cause }
    [install] Installation des Dienstes abgelehnt { $path }: die Dienstdatei konnte nicht überprüft werden: { $cause }
    [operate] Betrieb des Dienstes abgelehnt { $path }: die Dienstdatei konnte nicht überprüft werden: { $cause }
    [restart] Neustart des Dienstes abgelehnt { $path }: die Dienstdatei konnte nicht überprüft werden: { $cause }
    [route-repair] Routenreparatur des Dienstes abgelehnt { $path }: die Dienstdatei konnte nicht überprüft werden: { $cause }
    [status] Status des Dienstes abgelehnt { $path }: die Dienstdatei konnte nicht überprüft werden: { $cause }
    [stop] Stopp des Dienstes abgelehnt { $path }: die Dienstdatei konnte nicht überprüft werden: { $cause }
   *[uninstall] Deinstallation des Dienstes abgelehnt { $path }: die Dienstdatei konnte nicht überprüft werden: { $cause }
    }

journal.cli.host.service.incomplete.systemd.property.set = unvollständiger { $systemd }-Eigenschaftssatz

journal.cli.host.service.inspect.launchd.directory = { $launchd }-Verzeichnis untersuchen: { $error }

journal.cli.host.service.inspect.service.directory = Dienstverzeichnis untersuchen: { $error }

journal.cli.host.service.inspect.service.health.directory = Zustandsverzeichnis des Dienstes untersuchen: { $error }

journal.cli.host.service.inspect.service.journal.root = Journal-Wurzel des Dienstes untersuchen: { $error }

journal.cli.host.service.inspect.stale.launchd.unit = veraltete { $launchd }-Unit untersuchen: { $error }

journal.cli.host.service.installation.changed.while.waiting.for.the.new.journal.folder = die Installation hat sich geändert, während auf den neuen Journal-Ordner gewartet wurde

journal.cli.host.service.launchd.accepted.the.unload.request.but.the.service.is = { $launchd } hat die Entladeanfrage angenommen, aber der Dienst ist noch vorhanden

journal.cli.host.service.launchd.directory.is.not.a.real.directory = { $launchd }-Verzeichnis ist kein echtes Verzeichnis

journal.cli.host.service.launchd.observation.was.not.utf = { $launchd }-Beobachtung war nicht UTF-8

journal.cli.host.service.launchd.omitted.or.malformed.arguments = { $launchd } hat Argumente ausgelassen oder sie waren fehlerhaft

journal.cli.host.service.launchd.omitted.the.runtime.state = { $launchd } hat den Laufzeitzustand ausgelassen

journal.cli.host.service.malformed.systemd.property = fehlerhafte { $systemd }-Eigenschaft

journal.cli.host.service.manager.disable.service = Deaktivieren des Dienstes fehlgeschlagen: { $result }

journal.cli.host.service.manager.disable.stale.service = Deaktivieren des veralteten Dienstes fehlgeschlagen: { $result }

journal.cli.host.service.manager.enable.service = Aktivieren des Dienstes fehlgeschlagen: { $result }

journal.cli.host.service.manager.load.installed.service = Laden des installierten Dienstes fehlgeschlagen: { $result }

journal.cli.host.service.manager.load.service = Laden des Dienstes fehlgeschlagen: { $result }

journal.cli.host.service.manager.load.service.after.changing.journal.folder = Laden des Dienstes nach dem Ändern des Journal-Ordners fehlgeschlagen: { $result }

journal.cli.host.service.manager.reload.service.after.changing.journal.folder = Neuladen des Dienstes nach dem Ändern des Journal-Ordners fehlgeschlagen: { $result }

journal.cli.host.service.manager.reload.systemd = Neuladen von systemd fehlgeschlagen: { $result }

journal.cli.host.service.manager.reload.systemd.for.route.repair = Neuladen von systemd für die Routenreparatur fehlgeschlagen: { $result }

journal.cli.host.service.manager.request.launchd.unload = Anfordern des Entladens von launchd fehlgeschlagen: { $result }

journal.cli.host.service.manager.restart.service = Neustart des Dienstes fehlgeschlagen: { $result }

journal.cli.host.service.manager.restart.service.for.route.repair = Neustart des Dienstes für die Routenreparatur fehlgeschlagen: { $result }

journal.cli.host.service.manager.start.after.changing.journal.folder = Start nach dem Ändern des Journal-Ordners fehlgeschlagen: { $result }

journal.cli.host.service.manager.start.service = Start des Dienstes fehlgeschlagen: { $result }

journal.cli.host.service.manager.stop.service = Stoppen des Dienstes fehlgeschlagen: { $result }

journal.cli.host.service.manager.stop.service.before.changing.journal.folder = Stoppen des Dienstes vor dem Ändern des Journal-Ordners fehlgeschlagen: { $result }

journal.cli.host.service.manager.stop.service.before.restart = Stoppen des Dienstes vor dem Neustart fehlgeschlagen: { $result }

journal.cli.host.service.manager.stop.stale.service = Stoppen des veralteten Dienstes fehlgeschlagen: { $result }

journal.cli.host.service.manager.unload.stale.launchd.service = Entladen des veralteten launchd-Dienstes fehlgeschlagen: { $result }

journal.cli.host.service.manager.unload.stale.service = Entladen des veralteten Dienstes fehlgeschlagen: { $result }

journal.cli.host.service.metadata = Metadaten: { $error }

journal.cli.host.service.open.your.journal = öffne dein Journal: { $url }

journal.cli.host.service.open.your.journal.solstone.journal.open = öffne dein Journal: { $open }

journal.cli.host.service.parse.stale.launchd.unit = veraltete { $launchd }-Unit einlesen: { $error }

journal.cli.host.service.port.invalid = Fehler: ungültiger Port '{ $value }'

journal.cli.host.service.preserved.service.unit = bewahrte Dienst-Unit { $path }

journal.cli.host.service.read = lesen: { $error }

journal.cli.host.service.ready.timeout = Dienst wurde nicht innerhalb von 120 Sekunden bereit

journal.cli.host.service.recheck.stale.launchd.unit = veraltete { $launchd }-Unit erneut prüfen: { $error }

journal.cli.host.service.registration.a.different.unit.answers.to.this.name = { $operation ->
    [change-journal-folder] Ändern des Journal-Ordners für den Dienst abgelehnt: eine andere Unit antwortet auf diesen Namen. journal überschreibt sie nicht. Wenn das beabsichtigt ist, schließe das Ändern des Journal-Ordners selbst ab -- unter Linux: `{ $restart_command }` -- und bestätige dann den laufenden Build mit `{ $inspect_command }`
    [clean-stale-launchd-service] Bereinigen des veralteten launchd-Dienstes abgelehnt: eine andere Unit antwortet auf diesen Namen. journal überschreibt sie nicht. Wenn das beabsichtigt ist, schließe das Bereinigen des veralteten launchd-Dienstes selbst ab -- unter Linux: `{ $restart_command }` -- und bestätige dann den laufenden Build mit `{ $inspect_command }`
    [install] Installation des Dienstes abgelehnt: eine andere Unit antwortet auf diesen Namen. journal überschreibt sie nicht. Wenn das beabsichtigt ist, schließe die Installation selbst ab -- unter Linux: `{ $restart_command }` -- und bestätige dann den laufenden Build mit `{ $inspect_command }`
    [operate] Betrieb des Dienstes abgelehnt: eine andere Unit antwortet auf diesen Namen. journal überschreibt sie nicht. Wenn das beabsichtigt ist, schließe den Betrieb selbst ab -- unter Linux: `{ $restart_command }` -- und bestätige dann den laufenden Build mit `{ $inspect_command }`
    [start-after-changing-journal-folder] Start nach dem Ändern des Journal-Ordners abgelehnt: eine andere Unit antwortet auf diesen Namen. journal überschreibt sie nicht. Wenn das beabsichtigt ist, schließe den Start nach dem Ändern des Journal-Ordners selbst ab -- unter Linux: `{ $restart_command }` -- und bestätige dann den laufenden Build mit `{ $inspect_command }`
    [status] Status des Dienstes abgelehnt: eine andere Unit antwortet auf diesen Namen. journal überschreibt sie nicht. Wenn das beabsichtigt ist, schließe den Status selbst ab -- unter Linux: `{ $restart_command }` -- und bestätige dann den laufenden Build mit `{ $inspect_command }`
    [stop] Stoppen des Dienstes abgelehnt: eine andere Unit antwortet auf diesen Namen. journal überschreibt sie nicht. Wenn das beabsichtigt ist, schließe das Stoppen selbst ab -- unter Linux: `{ $restart_command }` -- und bestätige dann den laufenden Build mit `{ $inspect_command }`
    [uninstall] Deinstallation des Dienstes abgelehnt: eine andere Unit antwortet auf diesen Namen. journal überschreibt sie nicht. Wenn das beabsichtigt ist, schließe die Deinstallation selbst ab -- unter Linux: `{ $restart_command }` -- und bestätige dann den laufenden Build mit `{ $inspect_command }`
    [up] up des Dienstes abgelehnt: eine andere Unit antwortet auf diesen Namen. journal überschreibt sie nicht. Wenn das beabsichtigt ist, schließe das up selbst ab -- unter Linux: `{ $restart_command }` -- und bestätige dann den laufenden Build mit `{ $inspect_command }`
    [verify-service-stop] Überprüfen des Dienststopps abgelehnt: eine andere Unit antwortet auf diesen Namen. journal überschreibt sie nicht. Wenn das beabsichtigt ist, schließe das Überprüfen des Dienststopps selbst ab -- unter Linux: `{ $restart_command }` -- und bestätige dann den laufenden Build mit `{ $inspect_command }`
   *[verify-unload] Überprüfen des Entladens abgelehnt: eine andere Unit antwortet auf diesen Namen. journal überschreibt sie nicht. Wenn das beabsichtigt ist, schließe das Überprüfen des Entladens selbst ab -- unter Linux: `{ $restart_command }` -- und bestätige dann den laufenden Build mit `{ $inspect_command }`
    }

journal.cli.host.service.registration.the.launch.agent.is.not.the.one.journal.manages = { $operation ->
    [change-journal-folder] Ändern des Journal-Ordners für den Dienst abgelehnt: der Launch Agent ist nicht der, den journal verwaltet. journal überschreibt ihn nicht. Wenn das beabsichtigt ist, schließe das Ändern des Journal-Ordners selbst ab -- unter Linux: `{ $restart_command }` -- und bestätige dann den laufenden Build mit `{ $inspect_command }`
    [clean-stale-launchd-service] Bereinigen des veralteten launchd-Dienstes abgelehnt: der Launch Agent ist nicht der, den journal verwaltet. journal überschreibt ihn nicht. Wenn das beabsichtigt ist, schließe das Bereinigen des veralteten launchd-Dienstes selbst ab -- unter Linux: `{ $restart_command }` -- und bestätige dann den laufenden Build mit `{ $inspect_command }`
    [install] Installation des Dienstes abgelehnt: der Launch Agent ist nicht der, den journal verwaltet. journal überschreibt ihn nicht. Wenn das beabsichtigt ist, schließe die Installation selbst ab -- unter Linux: `{ $restart_command }` -- und bestätige dann den laufenden Build mit `{ $inspect_command }`
    [operate] Betrieb des Dienstes abgelehnt: der Launch Agent ist nicht der, den journal verwaltet. journal überschreibt ihn nicht. Wenn das beabsichtigt ist, schließe den Betrieb selbst ab -- unter Linux: `{ $restart_command }` -- und bestätige dann den laufenden Build mit `{ $inspect_command }`
    [start-after-changing-journal-folder] Start nach dem Ändern des Journal-Ordners abgelehnt: der Launch Agent ist nicht der, den journal verwaltet. journal überschreibt ihn nicht. Wenn das beabsichtigt ist, schließe den Start nach dem Ändern des Journal-Ordners selbst ab -- unter Linux: `{ $restart_command }` -- und bestätige dann den laufenden Build mit `{ $inspect_command }`
    [status] Status des Dienstes abgelehnt: der Launch Agent ist nicht der, den journal verwaltet. journal überschreibt ihn nicht. Wenn das beabsichtigt ist, schließe den Status selbst ab -- unter Linux: `{ $restart_command }` -- und bestätige dann den laufenden Build mit `{ $inspect_command }`
    [stop] Stoppen des Dienstes abgelehnt: der Launch Agent ist nicht der, den journal verwaltet. journal überschreibt ihn nicht. Wenn das beabsichtigt ist, schließe das Stoppen selbst ab -- unter Linux: `{ $restart_command }` -- und bestätige dann den laufenden Build mit `{ $inspect_command }`
    [uninstall] Deinstallation des Dienstes abgelehnt: der Launch Agent ist nicht der, den journal verwaltet. journal überschreibt ihn nicht. Wenn das beabsichtigt ist, schließe die Deinstallation selbst ab -- unter Linux: `{ $restart_command }` -- und bestätige dann den laufenden Build mit `{ $inspect_command }`
    [up] up des Dienstes abgelehnt: der Launch Agent ist nicht der, den journal verwaltet. journal überschreibt ihn nicht. Wenn das beabsichtigt ist, schließe das up selbst ab -- unter Linux: `{ $restart_command }` -- und bestätige dann den laufenden Build mit `{ $inspect_command }`
    [verify-service-stop] Überprüfen des Dienststopps abgelehnt: der Launch Agent ist nicht der, den journal verwaltet. journal überschreibt ihn nicht. Wenn das beabsichtigt ist, schließe das Überprüfen des Dienststopps selbst ab -- unter Linux: `{ $restart_command }` -- und bestätige dann den laufenden Build mit `{ $inspect_command }`
   *[verify-unload] Überprüfen des Entladens abgelehnt: der Launch Agent ist nicht der, den journal verwaltet. journal überschreibt ihn nicht. Wenn das beabsichtigt ist, schließe das Überprüfen des Entladens selbst ab -- unter Linux: `{ $restart_command }` -- und bestätige dann den laufenden Build mit `{ $inspect_command }`
    }

journal.cli.host.service.registration.the.launch.agent.s.program.is.not.a.journal = { $operation ->
    [change-journal-folder] Ändern des Journal-Ordners für den Dienst abgelehnt: das Programm des Launch Agents ist kein journal-Launcher. journal überschreibt es nicht. Wenn das beabsichtigt ist, schließe das Ändern des Journal-Ordners selbst ab -- unter Linux: `{ $restart_command }` -- und bestätige dann den laufenden Build mit `{ $inspect_command }`
    [clean-stale-launchd-service] Bereinigen des veralteten launchd-Dienstes abgelehnt: das Programm des Launch Agents ist kein journal-Launcher. journal überschreibt es nicht. Wenn das beabsichtigt ist, schließe das Bereinigen des veralteten launchd-Dienstes selbst ab -- unter Linux: `{ $restart_command }` -- und bestätige dann den laufenden Build mit `{ $inspect_command }`
    [install] Installation des Dienstes abgelehnt: das Programm des Launch Agents ist kein journal-Launcher. journal überschreibt es nicht. Wenn das beabsichtigt ist, schließe die Installation selbst ab -- unter Linux: `{ $restart_command }` -- und bestätige dann den laufenden Build mit `{ $inspect_command }`
    [operate] Betrieb des Dienstes abgelehnt: das Programm des Launch Agents ist kein journal-Launcher. journal überschreibt es nicht. Wenn das beabsichtigt ist, schließe den Betrieb selbst ab -- unter Linux: `{ $restart_command }` -- und bestätige dann den laufenden Build mit `{ $inspect_command }`
    [start-after-changing-journal-folder] Start nach dem Ändern des Journal-Ordners abgelehnt: das Programm des Launch Agents ist kein journal-Launcher. journal überschreibt es nicht. Wenn das beabsichtigt ist, schließe den Start nach dem Ändern des Journal-Ordners selbst ab -- unter Linux: `{ $restart_command }` -- und bestätige dann den laufenden Build mit `{ $inspect_command }`
    [status] Status des Dienstes abgelehnt: das Programm des Launch Agents ist kein journal-Launcher. journal überschreibt es nicht. Wenn das beabsichtigt ist, schließe den Status selbst ab -- unter Linux: `{ $restart_command }` -- und bestätige dann den laufenden Build mit `{ $inspect_command }`
    [stop] Stoppen des Dienstes abgelehnt: das Programm des Launch Agents ist kein journal-Launcher. journal überschreibt es nicht. Wenn das beabsichtigt ist, schließe das Stoppen selbst ab -- unter Linux: `{ $restart_command }` -- und bestätige dann den laufenden Build mit `{ $inspect_command }`
    [uninstall] Deinstallation des Dienstes abgelehnt: das Programm des Launch Agents ist kein journal-Launcher. journal überschreibt es nicht. Wenn das beabsichtigt ist, schließe die Deinstallation selbst ab -- unter Linux: `{ $restart_command }` -- und bestätige dann den laufenden Build mit `{ $inspect_command }`
    [up] up des Dienstes abgelehnt: das Programm des Launch Agents ist kein journal-Launcher. journal überschreibt es nicht. Wenn das beabsichtigt ist, schließe das up selbst ab -- unter Linux: `{ $restart_command }` -- und bestätige dann den laufenden Build mit `{ $inspect_command }`
    [verify-service-stop] Überprüfen des Dienststopps abgelehnt: das Programm des Launch Agents ist kein journal-Launcher. journal überschreibt es nicht. Wenn das beabsichtigt ist, schließe das Überprüfen des Dienststopps selbst ab -- unter Linux: `{ $restart_command }` -- und bestätige dann den laufenden Build mit `{ $inspect_command }`
   *[verify-unload] Überprüfen des Entladens abgelehnt: das Programm des Launch Agents ist kein journal-Launcher. journal überschreibt es nicht. Wenn das beabsichtigt ist, schließe das Überprüfen des Entladens selbst ab -- unter Linux: `{ $restart_command }` -- und bestätige dann den laufenden Build mit `{ $inspect_command }`
    }

journal.cli.host.service.registration.the.unit.carries.a.drop.in.journal.did.not = { $operation ->
    [change-journal-folder] Ändern des Journal-Ordners für den Dienst abgelehnt: die Unit trägt ein Drop-in, das journal nicht geschrieben hat. journal überschreibt sie nicht. Wenn das beabsichtigt ist, schließe das Ändern des Journal-Ordners selbst ab -- unter Linux: `{ $restart_command }` -- und bestätige dann den laufenden Build mit `{ $inspect_command }`
    [clean-stale-launchd-service] Bereinigen des veralteten launchd-Dienstes abgelehnt: die Unit trägt ein Drop-in, das journal nicht geschrieben hat. journal überschreibt sie nicht. Wenn das beabsichtigt ist, schließe das Bereinigen des veralteten launchd-Dienstes selbst ab -- unter Linux: `{ $restart_command }` -- und bestätige dann den laufenden Build mit `{ $inspect_command }`
    [install] Installation des Dienstes abgelehnt: die Unit trägt ein Drop-in, das journal nicht geschrieben hat. journal überschreibt sie nicht. Wenn das beabsichtigt ist, schließe die Installation selbst ab -- unter Linux: `{ $restart_command }` -- und bestätige dann den laufenden Build mit `{ $inspect_command }`
    [operate] Betrieb des Dienstes abgelehnt: die Unit trägt ein Drop-in, das journal nicht geschrieben hat. journal überschreibt sie nicht. Wenn das beabsichtigt ist, schließe den Betrieb selbst ab -- unter Linux: `{ $restart_command }` -- und bestätige dann den laufenden Build mit `{ $inspect_command }`
    [start-after-changing-journal-folder] Start nach dem Ändern des Journal-Ordners abgelehnt: die Unit trägt ein Drop-in, das journal nicht geschrieben hat. journal überschreibt sie nicht. Wenn das beabsichtigt ist, schließe den Start nach dem Ändern des Journal-Ordners selbst ab -- unter Linux: `{ $restart_command }` -- und bestätige dann den laufenden Build mit `{ $inspect_command }`
    [status] Status des Dienstes abgelehnt: die Unit trägt ein Drop-in, das journal nicht geschrieben hat. journal überschreibt sie nicht. Wenn das beabsichtigt ist, schließe den Status selbst ab -- unter Linux: `{ $restart_command }` -- und bestätige dann den laufenden Build mit `{ $inspect_command }`
    [stop] Stoppen des Dienstes abgelehnt: die Unit trägt ein Drop-in, das journal nicht geschrieben hat. journal überschreibt sie nicht. Wenn das beabsichtigt ist, schließe das Stoppen selbst ab -- unter Linux: `{ $restart_command }` -- und bestätige dann den laufenden Build mit `{ $inspect_command }`
    [uninstall] Deinstallation des Dienstes abgelehnt: die Unit trägt ein Drop-in, das journal nicht geschrieben hat. journal überschreibt sie nicht. Wenn das beabsichtigt ist, schließe die Deinstallation selbst ab -- unter Linux: `{ $restart_command }` -- und bestätige dann den laufenden Build mit `{ $inspect_command }`
    [up] up des Dienstes abgelehnt: die Unit trägt ein Drop-in, das journal nicht geschrieben hat. journal überschreibt sie nicht. Wenn das beabsichtigt ist, schließe das up selbst ab -- unter Linux: `{ $restart_command }` -- und bestätige dann den laufenden Build mit `{ $inspect_command }`
    [verify-service-stop] Überprüfen des Dienststopps abgelehnt: die Unit trägt ein Drop-in, das journal nicht geschrieben hat. journal überschreibt sie nicht. Wenn das beabsichtigt ist, schließe das Überprüfen des Dienststopps selbst ab -- unter Linux: `{ $restart_command }` -- und bestätige dann den laufenden Build mit `{ $inspect_command }`
   *[verify-unload] Überprüfen des Entladens abgelehnt: die Unit trägt ein Drop-in, das journal nicht geschrieben hat. journal überschreibt sie nicht. Wenn das beabsichtigt ist, schließe das Überprüfen des Entladens selbst ab -- unter Linux: `{ $restart_command }` -- und bestätige dann den laufenden Build mit `{ $inspect_command }`
    }

journal.cli.host.service.registration.the.unit.file.is.not.the.one.journal.manages = { $operation ->
    [change-journal-folder] Ändern des Journal-Ordners für den Dienst abgelehnt: die Unit-Datei ist nicht die, die journal verwaltet. journal überschreibt sie nicht. Wenn das beabsichtigt ist, schließe das Ändern des Journal-Ordners selbst ab -- unter Linux: `{ $restart_command }` -- und bestätige dann den laufenden Build mit `{ $inspect_command }`
    [clean-stale-launchd-service] Bereinigen des veralteten launchd-Dienstes abgelehnt: die Unit-Datei ist nicht die, die journal verwaltet. journal überschreibt sie nicht. Wenn das beabsichtigt ist, schließe das Bereinigen des veralteten launchd-Dienstes selbst ab -- unter Linux: `{ $restart_command }` -- und bestätige dann den laufenden Build mit `{ $inspect_command }`
    [install] Installation des Dienstes abgelehnt: die Unit-Datei ist nicht die, die journal verwaltet. journal überschreibt sie nicht. Wenn das beabsichtigt ist, schließe die Installation selbst ab -- unter Linux: `{ $restart_command }` -- und bestätige dann den laufenden Build mit `{ $inspect_command }`
    [operate] Betrieb des Dienstes abgelehnt: die Unit-Datei ist nicht die, die journal verwaltet. journal überschreibt sie nicht. Wenn das beabsichtigt ist, schließe den Betrieb selbst ab -- unter Linux: `{ $restart_command }` -- und bestätige dann den laufenden Build mit `{ $inspect_command }`
    [start-after-changing-journal-folder] Start nach dem Ändern des Journal-Ordners abgelehnt: die Unit-Datei ist nicht die, die journal verwaltet. journal überschreibt sie nicht. Wenn das beabsichtigt ist, schließe den Start nach dem Ändern des Journal-Ordners selbst ab -- unter Linux: `{ $restart_command }` -- und bestätige dann den laufenden Build mit `{ $inspect_command }`
    [status] Status des Dienstes abgelehnt: die Unit-Datei ist nicht die, die journal verwaltet. journal überschreibt sie nicht. Wenn das beabsichtigt ist, schließe den Status selbst ab -- unter Linux: `{ $restart_command }` -- und bestätige dann den laufenden Build mit `{ $inspect_command }`
    [stop] Stoppen des Dienstes abgelehnt: die Unit-Datei ist nicht die, die journal verwaltet. journal überschreibt sie nicht. Wenn das beabsichtigt ist, schließe das Stoppen selbst ab -- unter Linux: `{ $restart_command }` -- und bestätige dann den laufenden Build mit `{ $inspect_command }`
    [uninstall] Deinstallation des Dienstes abgelehnt: die Unit-Datei ist nicht die, die journal verwaltet. journal überschreibt sie nicht. Wenn das beabsichtigt ist, schließe die Deinstallation selbst ab -- unter Linux: `{ $restart_command }` -- und bestätige dann den laufenden Build mit `{ $inspect_command }`
    [up] up des Dienstes abgelehnt: die Unit-Datei ist nicht die, die journal verwaltet. journal überschreibt sie nicht. Wenn das beabsichtigt ist, schließe das up selbst ab -- unter Linux: `{ $restart_command }` -- und bestätige dann den laufenden Build mit `{ $inspect_command }`
    [verify-service-stop] Überprüfen des Dienststopps abgelehnt: die Unit-Datei ist nicht die, die journal verwaltet. journal überschreibt sie nicht. Wenn das beabsichtigt ist, schließe das Überprüfen des Dienststopps selbst ab -- unter Linux: `{ $restart_command }` -- und bestätige dann den laufenden Build mit `{ $inspect_command }`
   *[verify-unload] Überprüfen des Entladens abgelehnt: die Unit-Datei ist nicht die, die journal verwaltet. journal überschreibt sie nicht. Wenn das beabsichtigt ist, schließe das Überprüfen des Entladens selbst ab -- unter Linux: `{ $restart_command }` -- und bestätige dann den laufenden Build mit `{ $inspect_command }`
    }

journal.cli.host.service.registration.the.unit.is.in.an.unexpected.state = { $operation ->
    [change-journal-folder] Ändern des Journal-Ordners für den Dienst abgelehnt: die Unit ist in einem unerwarteten Zustand. journal überschreibt sie nicht. Wenn das beabsichtigt ist, schließe das Ändern des Journal-Ordners selbst ab -- unter Linux: `{ $restart_command }` -- und bestätige dann den laufenden Build mit `{ $inspect_command }`
    [clean-stale-launchd-service] Bereinigen des veralteten launchd-Dienstes abgelehnt: die Unit ist in einem unerwarteten Zustand. journal überschreibt sie nicht. Wenn das beabsichtigt ist, schließe das Bereinigen des veralteten launchd-Dienstes selbst ab -- unter Linux: `{ $restart_command }` -- und bestätige dann den laufenden Build mit `{ $inspect_command }`
    [install] Installation des Dienstes abgelehnt: die Unit ist in einem unerwarteten Zustand. journal überschreibt sie nicht. Wenn das beabsichtigt ist, schließe die Installation selbst ab -- unter Linux: `{ $restart_command }` -- und bestätige dann den laufenden Build mit `{ $inspect_command }`
    [operate] Betrieb des Dienstes abgelehnt: die Unit ist in einem unerwarteten Zustand. journal überschreibt sie nicht. Wenn das beabsichtigt ist, schließe den Betrieb selbst ab -- unter Linux: `{ $restart_command }` -- und bestätige dann den laufenden Build mit `{ $inspect_command }`
    [start-after-changing-journal-folder] Start nach dem Ändern des Journal-Ordners abgelehnt: die Unit ist in einem unerwarteten Zustand. journal überschreibt sie nicht. Wenn das beabsichtigt ist, schließe den Start nach dem Ändern des Journal-Ordners selbst ab -- unter Linux: `{ $restart_command }` -- und bestätige dann den laufenden Build mit `{ $inspect_command }`
    [status] Status des Dienstes abgelehnt: die Unit ist in einem unerwarteten Zustand. journal überschreibt sie nicht. Wenn das beabsichtigt ist, schließe den Status selbst ab -- unter Linux: `{ $restart_command }` -- und bestätige dann den laufenden Build mit `{ $inspect_command }`
    [stop] Stoppen des Dienstes abgelehnt: die Unit ist in einem unerwarteten Zustand. journal überschreibt sie nicht. Wenn das beabsichtigt ist, schließe das Stoppen selbst ab -- unter Linux: `{ $restart_command }` -- und bestätige dann den laufenden Build mit `{ $inspect_command }`
    [uninstall] Deinstallation des Dienstes abgelehnt: die Unit ist in einem unerwarteten Zustand. journal überschreibt sie nicht. Wenn das beabsichtigt ist, schließe die Deinstallation selbst ab -- unter Linux: `{ $restart_command }` -- und bestätige dann den laufenden Build mit `{ $inspect_command }`
    [up] up des Dienstes abgelehnt: die Unit ist in einem unerwarteten Zustand. journal überschreibt sie nicht. Wenn das beabsichtigt ist, schließe das up selbst ab -- unter Linux: `{ $restart_command }` -- und bestätige dann den laufenden Build mit `{ $inspect_command }`
    [verify-service-stop] Überprüfen des Dienststopps abgelehnt: die Unit ist in einem unerwarteten Zustand. journal überschreibt sie nicht. Wenn das beabsichtigt ist, schließe das Überprüfen des Dienststopps selbst ab -- unter Linux: `{ $restart_command }` -- und bestätige dann den laufenden Build mit `{ $inspect_command }`
   *[verify-unload] Überprüfen des Entladens abgelehnt: die Unit ist in einem unerwarteten Zustand. journal überschreibt sie nicht. Wenn das beabsichtigt ist, schließe das Überprüfen des Entladens selbst ab -- unter Linux: `{ $restart_command }` -- und bestätige dann den laufenden Build mit `{ $inspect_command }`
    }

journal.cli.host.service.registration.the.unit.s.execstart.is.not.a.journal.launcher = { $operation ->
    [change-journal-folder] Ändern des Journal-Ordners für den Dienst abgelehnt: das ExecStart der Unit ist kein journal-Launcher. journal überschreibt es nicht. Wenn das beabsichtigt ist, schließe das Ändern des Journal-Ordners selbst ab -- unter Linux: `{ $restart_command }` -- und bestätige dann den laufenden Build mit `{ $inspect_command }`
    [clean-stale-launchd-service] Bereinigen des veralteten launchd-Dienstes abgelehnt: das ExecStart der Unit ist kein journal-Launcher. journal überschreibt es nicht. Wenn das beabsichtigt ist, schließe das Bereinigen des veralteten launchd-Dienstes selbst ab -- unter Linux: `{ $restart_command }` -- und bestätige dann den laufenden Build mit `{ $inspect_command }`
    [install] Installation des Dienstes abgelehnt: das ExecStart der Unit ist kein journal-Launcher. journal überschreibt es nicht. Wenn das beabsichtigt ist, schließe die Installation selbst ab -- unter Linux: `{ $restart_command }` -- und bestätige dann den laufenden Build mit `{ $inspect_command }`
    [operate] Betrieb des Dienstes abgelehnt: das ExecStart der Unit ist kein journal-Launcher. journal überschreibt es nicht. Wenn das beabsichtigt ist, schließe den Betrieb selbst ab -- unter Linux: `{ $restart_command }` -- und bestätige dann den laufenden Build mit `{ $inspect_command }`
    [start-after-changing-journal-folder] Start nach dem Ändern des Journal-Ordners abgelehnt: das ExecStart der Unit ist kein journal-Launcher. journal überschreibt es nicht. Wenn das beabsichtigt ist, schließe den Start nach dem Ändern des Journal-Ordners selbst ab -- unter Linux: `{ $restart_command }` -- und bestätige dann den laufenden Build mit `{ $inspect_command }`
    [status] Status des Dienstes abgelehnt: das ExecStart der Unit ist kein journal-Launcher. journal überschreibt es nicht. Wenn das beabsichtigt ist, schließe den Status selbst ab -- unter Linux: `{ $restart_command }` -- und bestätige dann den laufenden Build mit `{ $inspect_command }`
    [stop] Stoppen des Dienstes abgelehnt: das ExecStart der Unit ist kein journal-Launcher. journal überschreibt es nicht. Wenn das beabsichtigt ist, schließe das Stoppen selbst ab -- unter Linux: `{ $restart_command }` -- und bestätige dann den laufenden Build mit `{ $inspect_command }`
    [uninstall] Deinstallation des Dienstes abgelehnt: das ExecStart der Unit ist kein journal-Launcher. journal überschreibt es nicht. Wenn das beabsichtigt ist, schließe die Deinstallation selbst ab -- unter Linux: `{ $restart_command }` -- und bestätige dann den laufenden Build mit `{ $inspect_command }`
    [up] up des Dienstes abgelehnt: das ExecStart der Unit ist kein journal-Launcher. journal überschreibt es nicht. Wenn das beabsichtigt ist, schließe das up selbst ab -- unter Linux: `{ $restart_command }` -- und bestätige dann den laufenden Build mit `{ $inspect_command }`
    [verify-service-stop] Überprüfen des Dienststopps abgelehnt: das ExecStart der Unit ist kein journal-Launcher. journal überschreibt es nicht. Wenn das beabsichtigt ist, schließe das Überprüfen des Dienststopps selbst ab -- unter Linux: `{ $restart_command }` -- und bestätige dann den laufenden Build mit `{ $inspect_command }`
   *[verify-unload] Überprüfen des Entladens abgelehnt: das ExecStart der Unit ist kein journal-Launcher. journal überschreibt es nicht. Wenn das beabsichtigt ist, schließe das Überprüfen des Entladens selbst ab -- unter Linux: `{ $restart_command }` -- und bestätige dann den laufenden Build mit `{ $inspect_command }`
    }

journal.cli.host.service.registration.unknown = { $operation ->
    [change-journal-folder] Ändern des Journal-Ordners für den Dienst: die Dienstregistrierung konnte nicht geprüft werden: { $cause }
    [clean-stale-launchd-service] Bereinigen des veralteten launchd-Dienstes: die Dienstregistrierung konnte nicht geprüft werden: { $cause }
    [install] Installation des Dienstes: die Dienstregistrierung konnte nicht geprüft werden: { $cause }
    [operate] Betrieb des Dienstes: die Dienstregistrierung konnte nicht geprüft werden: { $cause }
    [start-after-changing-journal-folder] Start nach dem Ändern des Journal-Ordners: die Dienstregistrierung konnte nicht geprüft werden: { $cause }
    [status] Status des Dienstes: die Dienstregistrierung konnte nicht geprüft werden: { $cause }
    [stop] Stoppen des Dienstes: die Dienstregistrierung konnte nicht geprüft werden: { $cause }
    [uninstall] Deinstallation des Dienstes: die Dienstregistrierung konnte nicht geprüft werden: { $cause }
    [up] up des Dienstes: die Dienstregistrierung konnte nicht geprüft werden: { $cause }
    [verify-service-stop] Überprüfen des Dienststopps: die Dienstregistrierung konnte nicht geprüft werden: { $cause }
   *[verify-unload] Überprüfen des Entladens: die Dienstregistrierung konnte nicht geprüft werden: { $cause }
    }

journal.cli.host.service.remove.service.unit = Dienst-Unit entfernen: { $error }

journal.cli.host.service.remove.stale.launchd.unit = veraltete { $launchd }-Unit entfernen: { $error }

journal.cli.host.service.removed = { $path } entfernt

journal.cli.host.service.removed.stale.launchd.plist = veraltete { $launchd }-plist { $path } entfernt

journal.cli.host.service.removed.stale.service.registration = veraltete Dienstregistrierung entfernt

journal.cli.host.service.route.repair.service.launcher.is.not.an.executable.regular = Launcher für die Routenreparatur des Dienstes ist keine ausführbare reguläre Datei

journal.cli.host.service.route.repair.service.launcher.is.unavailable = Launcher für die Routenreparatur des Dienstes ist nicht verfügbar: { $error }

journal.cli.host.service.route.repair.service.unit.has.an.invalid.command.tail = Dienst-Unit für die Routenreparatur hat ein ungültiges Befehlsende

journal.cli.host.service.route.repair.service.unit.has.an.invalid.execstart = Dienst-Unit für die Routenreparatur hat ein ungültiges { $exec_start }

journal.cli.host.service.route.repair.service.unit.has.no.execstart = Dienst-Unit für die Routenreparatur hat kein { $exec_start }

journal.cli.host.service.route.repair.service.unit.is.not.utf = Dienst-Unit für die Routenreparatur ist nicht UTF-8

journal.cli.host.service.run.solstone.journal.setup.or.solstone.journal.service.install = führe '{ $setup }' oder '{ $install }' aus, um ihn zu installieren.

journal.cli.host.service.scan.launchd.directory = { $launchd }-Verzeichnis durchsuchen: { $error }

journal.cli.host.service.scan.launchd.entry = { $launchd }-Eintrag durchsuchen: { $error }

journal.cli.host.service.service.became.active.during.the.journal.folder.change = Dienst wurde während der Änderung des Journal-Ordners aktiv

journal.cli.host.service.service.command.output.exceeded.kib = Ausgabe des Dienstbefehls hat 256 KiB überschritten

journal.cli.host.service.service.command.output.failed = Ausgabe des Dienstbefehls fehlgeschlagen: { $error }

journal.cli.host.service.service.command.spawn.failed = Start des Dienstbefehls fehlgeschlagen: { $error }

journal.cli.host.service.service.command.timed.out = Zeitüberschreitung beim Dienstbefehl

journal.cli.host.service.service.command.wait.failed = Warten auf den Dienstbefehl fehlgeschlagen: { $error }

journal.cli.host.service.service.directory.contains.unsafe.component = Dienstverzeichnis enthält eine unsichere Komponente

journal.cli.host.service.service.directory.escaped.home = Dienstverzeichnis hat das Home-Verzeichnis verlassen

journal.cli.host.service.service.enabled = Dienst aktiviert

journal.cli.host.service.service.health.path.is.not.a.real.directory = Zustandspfad des Dienstes ist kein echtes Verzeichnis

journal.cli.host.service.service.home = Dienst-Home: { $error }

journal.cli.host.service.service.install.could.not.resolve.journal = Dienstinstallation: journal konnte nicht aufgelöst werden

journal.cli.host.service.service.install.current.executable.has.no.directory = Dienstinstallation: aktuelle Programmdatei hat kein Verzeichnis

journal.cli.host.service.service.install.invalid.installation.identity.arguments = Dienstinstallation: ungültige Argumente der Installationsidentität: { $error }

journal.cli.host.service.service.install.requires.installation.identity.arguments = Dienstinstallation erfordert Argumente der Installationsidentität

journal.cli.host.service.service.installed = Dienst: installiert

journal.cli.host.service.service.journal.root.is.not.a.real.directory = Journal-Wurzel des Dienstes ist kein echtes Verzeichnis

journal.cli.host.service.service.launcher.is.not.an.executable.regular.file = Dienst-Launcher ist keine ausführbare reguläre Datei

journal.cli.host.service.service.launcher.is.unavailable = Dienst-Launcher ist nicht verfügbar: { $error }

journal.cli.host.service.service.loaded.into.launchd = Dienst in { $launchd } geladen

journal.cli.host.service.service.lock = Dienstsperre: { $error }

journal.cli.host.service.service.management.is.supported.only.on.linux.and.mac = Dienstverwaltung wird nur unter Linux und auf dem Mac unterstützt

journal.cli.host.service.service.not.installed = Dienst: nicht installiert

journal.cli.host.service.service.not.installed.run.solstone.journal.service.install.first = Dienst nicht installiert. führe zuerst '{ $install }' aus.

journal.cli.host.service.service.path.is.not.utf = Dienstpfad ist nicht UTF-8

journal.cli.host.service.service.registration.has.no.matching.managed.file = Dienstregistrierung hat keine passende verwaltete Datei

journal.cli.host.service.service.registration.has.no.valid.port = Dienstregistrierung hat keinen gültigen Port

journal.cli.host.service.service.restarted = Dienst neu gestartet

journal.cli.host.service.service.runtime.state.was.unexpected = { $operation ->
    [change-journal-folder] Ändern des Journal-Ordners für den Dienst: Zustand der Laufzeitumgebung war unerwartet
    [clean-stale-launchd-service] Bereinigen des veralteten launchd-Dienstes: Zustand der Laufzeitumgebung war unerwartet
    [install] Installation des Dienstes: Zustand der Laufzeitumgebung war unerwartet
    [operate] Betrieb des Dienstes: Zustand der Laufzeitumgebung war unerwartet
    [start-after-changing-journal-folder] Start nach dem Ändern des Journal-Ordners: Zustand der Laufzeitumgebung war unerwartet
    [status] Status des Dienstes: Zustand der Laufzeitumgebung war unerwartet
    [stop] Stoppen des Dienstes: Zustand der Laufzeitumgebung war unerwartet
    [uninstall] Deinstallation des Dienstes: Zustand der Laufzeitumgebung war unerwartet
    [up] up des Dienstes: Zustand der Laufzeitumgebung war unerwartet
    [verify-service-stop] Überprüfen des Dienststopps: Zustand der Laufzeitumgebung war unerwartet
   *[verify-unload] Überprüfen des Entladens: Zustand der Laufzeitumgebung war unerwartet
    }

journal.cli.host.service.service.started = Dienst gestartet

journal.cli.host.service.service.stderr.reader.panicked = stderr-Leser des Dienstes ist mit Panic abgebrochen

journal.cli.host.service.service.stderr.unavailable = stderr des Dienstes nicht verfügbar

journal.cli.host.service.service.stdout.reader.panicked = stdout-Leser des Dienstes ist mit Panic abgebrochen

journal.cli.host.service.service.stdout.unavailable = stdout des Dienstes nicht verfügbar

journal.cli.host.service.service.stopped = Dienst gestoppt

journal.cli.host.service.service.stopped.but.its.ready.state.could.not.be = Dienst gestoppt, aber sein Bereitschaftszustand konnte nicht entfernt werden: { $error }

journal.cli.host.service.service.unit.changed.before.publication.nothing.was.written = Dienst-Unit vor der Veröffentlichung geändert; nichts wurde geschrieben

journal.cli.host.service.service.unit.changed.during.operation.preserved.replacement = Dienst-Unit während des Vorgangs geändert; Ersatz bewahrt

journal.cli.host.service.service.unit.has.no.parent = Dienst-Unit hat kein übergeordnetes Verzeichnis

journal.cli.host.service.service.unit.has.no.trusted.home.ancestor = Dienst-Unit hat keinen vertrauenswürdigen Home-Vorfahren

journal.cli.host.service.service.unit.publication.failed = Veröffentlichung der Dienst-Unit fehlgeschlagen: { $error }

journal.cli.host.service.service.unit.published.with.uncertain.durability = Dienst-Unit mit unsicherer Dauerhaftigkeit veröffentlicht: { $other }

journal.cli.host.service.service.was.not.installed = Dienst war nicht installiert

journal.cli.host.service.sign.in.invalid = Fehler: { $flag } erwartet { $literal } oder { $literal_3 }

journal.cli.host.service.stale.launchd.plist.changed.during.cleanup.preserved = veraltete { $launchd }-plist während der Bereinigung geändert; { $path } bewahrt

journal.cli.host.service.stale.launchd.unit.has.a.foreign.label = veraltete { $launchd }-Unit hat eine fremde Beschriftung

journal.cli.host.service.stale.launchd.unit.has.conflicting.program.fields = veraltete { $launchd }-Unit hat widersprüchliche Programmfelder

journal.cli.host.service.stale.launchd.unit.has.no.label = veraltete { $launchd }-Unit hat keine Beschriftung

journal.cli.host.service.stale.launchd.unit.has.no.launcher = veraltete { $launchd }-Unit hat keinen Launcher

journal.cli.host.service.stale.launchd.unit.is.foreign.or.unrecognized = veraltete { $launchd }-Unit ist fremd oder unerkannt

journal.cli.host.service.stale.launchd.unit.is.not.a.dictionary = veraltete { $launchd }-Unit ist kein Dictionary

journal.cli.host.service.state.running = Zustand: läuft ({ $manager })

journal.cli.host.service.state.stopped = Zustand: gestoppt

journal.cli.host.service.subcommand.unknown = unbekannter Unterbefehl: { $value }; verfügbar: { $literal }

journal.cli.host.service.systemd.observation.was.not.utf = { $systemd }-Beobachtung war nicht UTF-8

journal.cli.host.service.the.previous.service.did.not.stop.before.replacement = der vorherige Dienst wurde vor dem Ersatz nicht gestoppt

journal.cli.host.service.the.solstone.executable.has.no.containing.directory = die { $solstone }-Programmdatei hat kein übergeordnetes Verzeichnis

journal.cli.host.service.unit.exceeds.mib = Unit überschreitet 1 MiB

journal.cli.host.service.unit.path.has.no.trusted.home.ancestor = Unit-Pfad hat keinen vertrauenswürdigen Home-Vorfahren

journal.cli.host.service.unrecognized.systemd.active.substate.pair = unerkanntes active/substate-Paar von { $systemd }

journal.cli.host.service.unsafe.service.directory = unsicheres Dienstverzeichnis: { $path }

journal.cli.host.service.usage = Verwendung: { $syntax }{ $line_break }{ $spaces_7 }{ $syntax_4 }{ $spaces_2 }(Standard: 5015; nur { $term }){ $line_break }{ $spaces_7 }{ $syntax_7 }{ $spaces_2 }(neu starten; { $flag } tut nichts, wenn nicht installiert){ $line_break }{ $spaces_7 }{ $syntax_9 }{ $spaces_29 }(starten + Status; Dienst muss installiert sein){ $line_break }{ $spaces_7 }{ $syntax_11 }{ $spaces_27 }(stoppen){ $line_break }

journal.cli.host.service.windows.only = dieser Dienstbefehl ist nur unter Windows verfügbar

journal.cli.host.service.wrote = { $path } geschrieben

journal.cli.host.settings.bind = convey{ $line_break }{ $space_2 }Bindung:{ $space_14 }{ $address }

journal.cli.host.settings.convey.error.argument.convey.command.invalid.choice.choose.from.status = { $command }: Fehler: Argument { $literal }: ungültige Auswahl: '{ $value }' (wähle aus { $literal_3 })

journal.cli.host.settings.convey.help = Verwendung: { $syntax }{ $line_break }{ $line_break }Positionsargumente:{ $line_break }{ $spaces_2 }{ $choices }{ $line_break }{ $spaces_4 }{ $subcommand }{ $spaces_4 }Status der convey-Bindung und der Dashboard-{ $term } anzeigen{ $line_break }{ $line_break }Optionen:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_2 }diese Hilfenachricht anzeigen und beenden{ $line_break }

journal.cli.host.settings.convey.usage = Verwendung: { $syntax }{ $line_break }

journal.cli.host.settings.error.argument.section.invalid.choice.choose.from.convey = { $command }: Fehler: Argument { $literal }: ungültige Auswahl: '{ $value }' (wähle aus { $literal_3 })

journal.cli.host.settings.error.unrecognized.arguments = { $command }: Fehler: unbekannte Argumente: { $value }

journal.cli.host.settings.help = Verwendung: { $syntax }{ $line_break }{ $line_break }Lokale Journal-Einstellungen verwalten{ $line_break }{ $line_break }Positionsargumente:{ $line_break }{ $spaces_2 }{ $choices }{ $line_break }{ $spaces_4 }{ $subcommand }{ $spaces_7 }convey-Einstellungen verwalten{ $line_break }{ $line_break }Optionen:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_5 }diese Hilfenachricht anzeigen und beenden{ $line_break }{ $spaces_2 }{ $option_10 }{ $spaces_2 }Ausführliche Ausgabe aktivieren{ $line_break }{ $spaces_2 }{ $option_11 }{ $spaces_4 }Debug-Protokollierung aktivieren{ $line_break }

journal.cli.host.settings.open = öffne es mit:{ $space_6 }{ $command }

journal.cli.host.settings.port.read = Fehler: { $path }: { $detail }

journal.cli.host.settings.status.help = Verwendung: { $syntax }{ $line_break }{ $line_break }Optionen:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_2 }diese Hilfenachricht anzeigen und beenden{ $line_break }{ $spaces_2 }{ $option_5 }{ $spaces_6 }Maschinenlesbaren Status ausgeben.{ $line_break }

journal.cli.host.settings.url = Dashboard-URL:{ $space_5 }{ $url }

journal.cli.host.settings.usage = Verwendung: { $syntax }{ $line_break }

journal.cli.host.setup.error = { $literal }: Fehler: { $detail }

journal.cli.host.setup.usage = Verwendung: { $syntax }{ $line_break }{ $spaces_21 }{ $syntax_4 }{ $line_break }{ $spaces_21 }{ $syntax_5 }{ $line_break }{ $spaces_21 }{ $syntax_6 }{ $line_break }{ $spaces_21 }{ $syntax_7 }{ $line_break }{ $spaces_21 }{ $syntax_8 }{ $line_break }{ $spaces_21 }{ $syntax_9 } (nur { $term }){ $line_break }

journal.cli.host.spl.help = Verwendung: { $syntax }{ $line_break }{ $line_break }Optionen:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_5 }diese Hilfenachricht anzeigen und beenden{ $line_break }{ $spaces_2 }{ $option_6 }{ $spaces_2 }Ausführliche Ausgabe aktivieren{ $line_break }{ $spaces_2 }{ $option_7 }{ $spaces_4 }Debug-Protokollierung aktivieren{ $line_break }

journal.cli.host.spl.service.spl.service.failed = spl-Dienst fehlgeschlagen: { $class }

journal.cli.host.spl.usage = Verwendung: { $syntax }{ $line_break }

journal.cli.host.start.help = Verwendung: { $syntax }{ $line_break }{ $spaces_21 }{ $syntax_4 }{ $line_break }{ $spaces_21 }{ $syntax_5 }{ $line_break }{ $spaces_21 }{ $syntax_6 }{ $line_break }{ $spaces_21 }{ $syntax_7 }{ $line_break }{ $line_break }Zustand des Journal-Systems überwachen{ $line_break }{ $line_break }Positionsargumente:{ $line_break }{ $spaces_2 }{ $subcommand }{ $spaces_15 }{ $term }-Port (0 = verfügbaren Port automatisch wählen){ $line_break }{ $line_break }Optionen:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_9 }diese Hilfenachricht anzeigen und beenden{ $line_break }{ $spaces_2 }{ $option_14 }{ $spaces_9 }Täglichen Verarbeitungslauf um Mitternacht deaktivieren{ $line_break }{ $spaces_2 }{ $option_15 }{ $spaces_8 }Den { $term_17 }-Server nicht starten (manuell ausführen zum{ $line_break }{ $spaces_21 }Debuggen){ $line_break }{ $spaces_2 }{ $option_18 }{ $spaces_11 }Das Relay des privaten Netzwerks nicht starten{ $line_break }{ $spaces_2 }{ $option_20 }{ $spaces_8 }Die Webanwendung von { $term } nicht starten{ $line_break }{ $spaces_2 }{ $option_21 }{ $spaces_6 }Die Zeitplan-Engine nicht initialisieren oder ausführen{ $line_break }{ $spaces_2 }{ $option_23 }{ $spaces_4 }Den direkten Elternprozess am Leben halten{ $line_break }{ $spaces_2 }{ $option_25 }{ $spaces_2 }Diesen Pfad als Journal-Wurzel verwenden statt der normalen{ $line_break }{ $spaces_21 }Auflösung des Journals.{ $line_break }{ $spaces_2 }{ $flag } { $argument }{ $line_break }{ $spaces_21 }Tür-Port des gekoppelten Geräts (Standard: 7657){ $line_break }{ $spaces_2 }{ $option_28 }{ $spaces_6 }Ausführliche Ausgabe aktivieren{ $line_break }{ $spaces_2 }{ $option_29 }{ $spaces_8 }Debug-Protokollierung aktivieren{ $line_break }

journal.cli.host.start.usage = Verwendung: { $syntax }{ $line_break }{ $spaces_21 }{ $syntax_4 }{ $line_break }{ $spaces_21 }{ $syntax_5 }{ $line_break }{ $spaces_21 }{ $syntax_6 }{ $line_break }{ $spaces_21 }{ $syntax_7 }{ $line_break }

journal.cli.host.supervisor.admission.wait.active = ein weiterer { $brand }-Start wartet, nachdem er aktuelle Aktivität im Journal gefunden hat.{ $break_line }dieser Start wurde nicht fortgesetzt, solange dieses Warten aktiv war.{ $break_line }warte einen Moment, dann versuch es noch einmal.

journal.cli.host.supervisor.admission.wait.terminal = ein aktueller heartbeat eines anderen Laufs liegt vor.{ $break_line }die solstone-App wurde nicht gestartet, solange dieser heartbeat noch vorlag.{ $break_line }warte einen Moment, dann versuch es noch einmal.

journal.cli.host.supervisor.admission.wait.unverifiable = der Startstatus konnte nicht überprüft werden.{ $break_line }warte einen Moment, dann versuch es noch einmal.

journal.cli.host.supervisor.already.running = dein Journal läuft schon.{ $break_line }{ $break_line }dieser Start ist zur Seite gegangen, damit der laufende nicht gestört wird. es ist nichts zu tun.{ $break_line }

journal.cli.host.supervisor.binding.home = Home: { $error }

journal.cli.host.supervisor.binding.installation.root = Installationswurzel: { $error }

journal.cli.host.supervisor.binding.journal.token = Journal-Token: { $error }

journal.cli.host.supervisor.binding.owner.storage = dein Speicher: { $error }

journal.cli.host.supervisor.binding.root.token = Wurzel-Token: { $error }

journal.cli.host.supervisor.binding.saved.binding = gespeicherte Bindung: { $error }

journal.cli.host.supervisor.binding.unreadable = die gespeicherte Installationsbindung konnte nicht geladen werden

journal.cli.host.supervisor.help = Verwendung: { $syntax }{ $line_break }{ $spaces_26 }{ $syntax_4 }{ $line_break }{ $spaces_26 }{ $syntax_5 }{ $line_break }{ $spaces_26 }{ $syntax_6 }{ $line_break }{ $spaces_26 }{ $syntax_7 }{ $line_break }{ $line_break }Zustand des Journal-Systems überwachen{ $line_break }{ $line_break }Positionsargumente:{ $line_break }{ $spaces_2 }{ $subcommand }{ $spaces_15 }{ $term }-Port (0 = verfügbaren Port automatisch wählen){ $line_break }{ $line_break }Optionen:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_9 }diese Hilfenachricht anzeigen und beenden{ $line_break }{ $spaces_2 }{ $option_14 }{ $spaces_9 }Täglichen Verarbeitungslauf um Mitternacht deaktivieren{ $line_break }{ $spaces_2 }{ $option_15 }{ $spaces_8 }Den { $term_17 }-Server nicht starten (manuell ausführen zum{ $line_break }{ $spaces_21 }Debuggen){ $line_break }{ $spaces_2 }{ $option_19 }{ $spaces_11 }Das Relay des privaten Netzwerks nicht starten{ $line_break }{ $spaces_2 }{ $option_21 }{ $spaces_8 }Die Webanwendung von { $term } nicht starten{ $line_break }{ $spaces_2 }{ $option_22 }{ $spaces_6 }Die Zeitplan-Engine nicht initialisieren oder ausführen{ $line_break }{ $spaces_2 }{ $option_24 }{ $spaces_2 }Diesen Pfad als Journal-Wurzel verwenden statt der normalen{ $line_break }{ $spaces_21 }Auflösung des Journals.{ $line_break }{ $spaces_2 }{ $flag } { $argument }{ $line_break }{ $spaces_21 }Tür-Port des gekoppelten Geräts (Standard: 7657){ $line_break }{ $spaces_2 }{ $option_27 }{ $spaces_6 }Ausführliche Ausgabe aktivieren{ $line_break }{ $spaces_2 }{ $option_28 }{ $spaces_8 }Debug-Protokollierung aktivieren{ $line_break }

journal.cli.host.supervisor.journal.program.folder = Neuinstallieren oder Deinstallieren entfernt diesen Programmordner. Wähle einen Ort für das Journal außerhalb davon. Wenn dein Journal schon dort liegt, beende die Journal-App. Wenn die Ersteinrichtung abgeschlossen ist, führe `{ $stop }` aus, danach `{ $uninstall }`, und warte, bis es gelingt. Verschiebe den ganzen Journal-Ordner vor dem Neuinstallieren oder Deinstallieren aus dem Programmordner und wähle dann seinen neuen Ort mit `{ $setup }`, wobei du { $path } durch seinen neuen Ort ersetzt.

journal.cli.host.supervisor.journal.token.mismatch = diese Installation konnte nicht überprüft werden.{ $line_break }führe `{ $setup }` aus, um sie zu prüfen. wenn die Ersteinrichtung erfolgreich endet, versuch es noch einmal.{ $line_break }Details: die gespeicherte Installationsbindung gehört zu einem anderen Journal

journal.cli.host.supervisor.lifecycle.redirect = { $supervisor } ist der Befehl zum Starten des Servers (nimmt einen Port). Für den Lebenszyklus verwende: { $service_verb }. Meintest du: { $requested }?

journal.cli.host.supervisor.recovery.closing = dieser Start konnte nicht fortgesetzt werden.{ $line_break }{ $line_break }der letzte Lauf fährt noch herunter. warte einen Moment, dann versuch es noch einmal.{ $line_break }{ $line_break }deine Erinnerungen sind unberührt.{ $line_break }

journal.cli.host.supervisor.recovery.coordinator.retirement.unverified = Details: ein Helferprozess konnte nicht als gestoppt bestätigt werden

journal.cli.host.supervisor.recovery.coordinator.stopped.before.ready = Details: ein Helferprozess wurde gestoppt, bevor er bereit war

journal.cli.host.supervisor.recovery.identity.establishment = Details: ein Helferprozess konnte nicht erkannt werden

journal.cli.host.supervisor.recovery.initial.admission.handshake = Details: ein Helferprozess hat nicht rechtzeitig geantwortet

journal.cli.host.supervisor.recovery.launch = Details: ein Helferprozess konnte nicht gestartet werden

journal.cli.host.supervisor.recovery.retry = dieser Start konnte nicht fortgesetzt werden.{ $line_break }{ $line_break }warte einen Moment, dann versuch es noch einmal.{ $line_break }{ $line_break }deine Erinnerungen sind unberührt.{ $line_break }

journal.cli.host.supervisor.supervisor.runtime.unavailable = Laufzeitumgebung von supervisor nicht verfügbar: { $error }

journal.cli.host.supervisor.supervisor.service.capture.unavailable = Erfassung des supervisor-Dienstes nicht verfügbar: { $error }

journal.cli.host.supervisor.usage = Verwendung: { $syntax }{ $line_break }{ $spaces_26 }{ $syntax_4 }{ $line_break }{ $spaces_26 }{ $syntax_5 }{ $line_break }{ $spaces_26 }{ $syntax_6 }{ $line_break }{ $spaces_26 }{ $syntax_7 }{ $line_break }

journal.cli.host.supervisor.your.journal.can.t.start.because.another.account.on.this = dein Journal kann nicht starten, weil ein anderes Konto auf diesem Computer sein Journal ausführt. auf einem Computer läuft immer nur ein Journal, daher kann deins starten, sobald dieses Konto sein Journal stoppt oder sich abmeldet.

journal.cli.host.supervisor.your.journal.can.t.start.because.port.on.this.computer = dein Journal kann nicht starten, weil Port { $port } auf diesem Computer schon belegt ist, meist von einem Journal unter einem anderen Konto. auf diesem Computer kann immer nur ein Journal laufen.

journal.cli.host.thinking.chatgpt.help = Verwendung: { $syntax }{ $line_break }{ $line_break }Anmeldung, Token und Modellkatalog von { $term } verwalten.{ $line_break }{ $line_break }Positionsargumente:{ $line_break }{ $spaces_2 }{ $choices }{ $line_break }{ $spaces_4 }{ $subcommand }{ $spaces_14 }Bei { $term } über den Browser per { $term_9 } anmelden{ $line_break }{ $spaces_4 }{ $subcommand_10 }{ $spaces_13 }Abmelden und aktive Zugangsdaten von { $term } widerrufen{ $line_break }{ $spaces_4 }{ $subcommand_12 }{ $spaces_15 }Aktuellen Authentifizierungsstatus von { $term } anzeigen{ $line_break }{ $spaces_4 }{ $subcommand_14 }{ $spaces_15 }Verfügbare Modelle von { $term } auflisten{ $line_break }{ $line_break }Optionen:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_12 }diese Hilfenachricht anzeigen und beenden{ $line_break }

journal.cli.host.thinking.chatgpt.models.help = Verwendung: { $syntax }{ $line_break }{ $line_break }Verfügbare Modelle von { $term } auflisten.{ $line_break }{ $line_break }Optionen:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_12 }diese Hilfenachricht anzeigen und beenden{ $line_break }{ $spaces_2 }{ $option_7 }{ $spaces_8 }Journal-Wurzel{ $line_break }{ $spaces_2 }{ $option_9 }{ $spaces_16 }Maschinenlesbares { $term_11 } ausgeben{ $line_break }

journal.cli.host.thinking.chatgpt.models.usage = Verwendung: { $syntax }{ $line_break }

journal.cli.host.thinking.chatgpt.sign.in.help = Verwendung: { $syntax }{ $line_break }{ $line_break }Bei { $term } über den Browser per { $term_4 } anmelden.{ $line_break }{ $line_break }Optionen:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_12 }diese Hilfenachricht anzeigen und beenden{ $line_break }{ $spaces_2 }{ $option_8 }{ $spaces_8 }Journal-Wurzel{ $line_break }{ $spaces_2 }{ $option_10 }{ $spaces_10 }Standardbrowser nicht automatisch öffnen{ $line_break }

journal.cli.host.thinking.chatgpt.sign.in.usage = Verwendung: { $syntax }{ $line_break }

journal.cli.host.thinking.chatgpt.sign.out.help = Verwendung: { $syntax }{ $line_break }{ $line_break }Abmelden und aktive Zugangsdaten von { $term } widerrufen.{ $line_break }{ $line_break }Optionen:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_12 }diese Hilfenachricht anzeigen und beenden{ $line_break }{ $spaces_2 }{ $option_7 }{ $spaces_8 }Journal-Wurzel{ $line_break }{ $spaces_2 }{ $option_9 }{ $spaces_14 }Gespeicherte Datei der Zugangsdaten und Registrierung entfernen{ $line_break }

journal.cli.host.thinking.chatgpt.sign.out.usage = Verwendung: { $syntax }{ $line_break }

journal.cli.host.thinking.chatgpt.status.help = Verwendung: { $syntax }{ $line_break }{ $line_break }Aktuellen Authentifizierungsstatus von { $term } anzeigen.{ $line_break }{ $line_break }Optionen:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_12 }diese Hilfenachricht anzeigen und beenden{ $line_break }{ $spaces_2 }{ $option_7 }{ $spaces_8 }Journal-Wurzel{ $line_break }{ $spaces_2 }{ $option_9 }{ $spaces_16 }Maschinenlesbares { $term_11 } ausgeben{ $line_break }

journal.cli.host.thinking.chatgpt.status.usage = Verwendung: { $syntax }{ $line_break }

journal.cli.host.thinking.chatgpt.usage = Verwendung: { $syntax }{ $line_break }

journal.cli.host.thinking.credential.chatgpt.auth.server.temporarily.unavailable = { $literal_1 }-Auth-Server ist vorübergehend nicht verfügbar (429 oder 5xx)

journal.cli.host.thinking.credential.chatgpt.credential.store.busy = { $literal_1 }-Zugangsdatenspeicher ist belegt

journal.cli.host.thinking.credential.chatgpt.request.was.by.auth = { $literal_1 }-Anfrage wurde vom Auth-Server abgelehnt

journal.cli.host.thinking.credential.chatgpt.request.was.code = { $literal_1 }-Anfrage wurde abgelehnt: { $code }

journal.cli.host.thinking.credential.chatgpt.server.returned.malformed.payload = { $literal_1 }-Server hat fehlerhafte Nutzdaten zurückgegeben ({ $field } fehlt oder ist ungültig)

journal.cli.host.thinking.credential.chatgpt.server.returned.malformed.payload.server.returned.malformed.payload = { $literal_1 }-Server hat fehlerhafte Nutzdaten zurückgegeben

journal.cli.host.thinking.credential.chatgpt.token.grant.was.received = Token-Gewährung von { $literal_1 } wurde vom Server empfangen, konnte aber nicht auf den Datenträger gespeichert werden

journal.cli.host.thinking.credential.i.o.during.credential.operation = I/O-Fehler während des Zugangsdatenvorgangs: { $msg }

journal.cli.host.thinking.credential.network.communicating.with.chatgpt.auth = Netzwerkfehler bei der Kommunikation mit den Auth-Servern von { $literal_1 }

journal.cli.host.thinking.credential.path.credential.storage.unreadable.or = { $path }: Zugangsdatenspeicher ist unlesbar oder beschädigt; führe '{ $literal_1 }' aus

journal.cli.host.thinking.credential.this.chatgpt.account.isn.t = dieses { $literal_1 }-Konto ist nicht berechtigt

journal.cli.host.thinking.help = Verwendung: { $syntax }{ $line_break }{ $line_break }Spur fürs Denken des Journals wählen oder die Authentifizierung von { $term } verwalten.{ $line_break }{ $line_break }Positionsargumente:{ $line_break }{ $spaces_2 }{ $choices }{ $line_break }{ $spaces_4 }{ $subcommand }{ $spaces_13 }Spur fürs Denken setzen ({ $literal }, { $literal_10 } oder { $literal_11 }){ $line_break }{ $spaces_4 }{ $subcommand_12 }{ $spaces_14 }Anmeldung und Token von { $term } verwalten{ $line_break }{ $line_break }Optionen:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_12 }diese Hilfenachricht anzeigen und beenden{ $line_break }

journal.cli.host.thinking.run.chatgpt.sign.in.account.mismatch.run.thinking.chatgpt = Konto stimmt nicht überein: führe '{ $literal_1 }' aus

journal.cli.host.thinking.run.chatgpt.sign.in.chatgpt.sign.in.failed.outcome = Anmeldung bei { $literal_1 } fehlgeschlagen: { $outcome }

journal.cli.host.thinking.run.chatgpt.sign.in.chatgpt.sign.in.required.run = Anmeldung bei { $literal_2 } erforderlich: führe '{ $literal_1 }' aus

journal.cli.host.thinking.run.chatgpt.sign.in.signed.in.chatgpt.successfully = erfolgreich bei { $literal_1 } angemeldet.

journal.cli.host.thinking.run.chatgpt.sign.out.signed.out.chatgpt = bei { $literal_1 } abgemeldet.

journal.cli.host.thinking.run.chatgpt.sign.out.signed.out.chatgpt.complete.sign = bei { $literal_1 } abgemeldet.{ $line_break }Um die Abmeldung abzuschließen, trenne solstone in den Einstellungen deines { $literal_1 }-Kontos.

journal.cli.host.thinking.run.chatgpt.status.plan.usage.declined = Tarifnutzung: abgelehnt

journal.cli.host.thinking.run.chatgpt.status.status.not.signed.in = Status: nicht angemeldet

journal.cli.host.thinking.run.chatgpt.status.status.signed.in = Status: angemeldet

journal.cli.host.thinking.set.lane.help = Verwendung: { $syntax }{ $line_break }{ $line_break }Spur fürs Denken setzen, die { $literal } verwendet.{ $line_break }{ $line_break }Positionsargumente:{ $line_break }{ $spaces_2 }{ $choices }{ $line_break }{ $spaces_24 }Zu aktivierende Spur fürs Denken.{ $line_break }{ $line_break }Optionen:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_12 }diese Hilfenachricht anzeigen und beenden{ $line_break }{ $spaces_2 }{ $option_9 }{ $spaces_3 }{ $argument }-Anbieter: { $literal_12 }, { $literal_13 }, { $literal_14 }, { $literal_15 } oder { $literal_16 }{ $line_break }{ $spaces_2 }{ $option_17 }{ $spaces_9 }Name des Cloud-{ $argument }-Modells{ $line_break }{ $spaces_2 }{ $option_19 }{ $spaces_8 }Journal-Wurzel{ $line_break }

journal.cli.host.thinking.set.lane.usage = Verwendung: { $syntax }{ $line_break }

journal.cli.host.thinking.usage = Verwendung: { $syntax }{ $line_break }

journal.cli.host.top.failure = { $command_solstone_journal } top: { $error }

journal.cli.host.top.help = Verwendung: { $syntax }{ $line_break }{ $line_break }Interaktive Aktivität von Dienst, Beobachtung, Aufgabe und brain anzeigen.{ $line_break }{ $line_break }Optionen:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_5 }diese Hilfenachricht anzeigen und beenden{ $line_break }{ $spaces_2 }{ $option_6 }{ $spaces_2 }ausführliche Ausgabe aktivieren{ $line_break }{ $spaces_2 }{ $option_7 }{ $spaces_4 }Debug-Ausgabe aktivieren{ $line_break }

journal.cli.host.top.usage = Verwendung: { $syntax }{ $line_break }

journal.cli.host.up.help = Verwendung: { $syntax }{ $line_break }{ $line_break }Den installierten Journal-Dienst starten, wenn er nicht läuft, und dann warten, bis er bereit ist.{ $line_break }{ $line_break }Optionen:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_2 }diese Hilfenachricht anzeigen und beenden{ $line_break }

journal.cli.host.usage.commands = Verwendung:{ $line_break }{ $commands }

journal.cli.host.warm.development = Ein GAP ist ein Geschwisterelement im Entwicklungs-Layout, das nicht gebaut und daher nicht ausgeführt wurde; es wird namentlich gemeldet, lässt warm aber nicht fehlschlagen.

journal.cli.host.warm.inventory = warm prüft die { $count } nativen Binärdateien, die die Paketierungsblätter von maturin { $bindings } des Hosts deklarieren.

journal.cli.host.warm.linux.dynamic = Unter Linux laden { $solstone_core_speakers_analyze } und { $solstone_core_vad_analyze } das mitgelieferte { $onnx } dynamisch, und { $solstone_core_vulkan_probe } lädt den { $vulkan }-Loader des Hosts dynamisch. warm weist ihre Loader-Auflösung ebenso wie ihren Start nach. { $solstone_core_describe } wird hier weder als statisch noch als dynamisch behauptet; die Wheel-Prüfung verbietet nur, dass { $ffmpeg } dynamisch gelinkt ist.

journal.cli.host.warm.linux.static = Unter Linux sind { $solstone_core }, { $solstone_core_journal }, { $solstone_retention }, { $solstone_core_sol } und { $solstone_core_depict } statisch mit musl gelinkt. warm weist nach, dass sie starten und ihren eigenen Code erreichen; es gibt keine dynamische Loader-Auflösung, die nachzuweisen wäre.

journal.cli.host.warm.mac = Unter macOS ist jedes Mach-O mit { $library } gelinkt. Die Eigenschaft ist nicht leer, unterscheidet sich aber von Linux: warm legt dort Fehler bei Signierung, Quarantäne und { $dyld } offen.

journal.cli.host.warm.platforms = Plattformbeschränkte warm-Zeilen: { $rows }. Diese melden NOT APPLICABLE, wenn ihre Anwendbarkeit diesen Host nicht umfasst, und lassen warm nicht fehlschlagen.

journal.cli.host.warm.repair.development = { $space_2 }Reparatur: Baue dieses Crate in einer konfigurierten Entwicklungsumgebung.

journal.cli.host.warm.repair.execution = { $space_2 }Reparatur: Behebe den gemeldeten Ausführungsfehler des Hosts und installiere dann das zugehörige Host-Werkzeug neu, falls nötig.

journal.cli.host.warm.repair.missing = { $space_2 }Reparatur: Installiere das zugehörige Host-Werkzeug neu oder aktualisiere es mit der Familie von Installationsprogrammen, die diese Umgebung erzeugt hat.

journal.cli.host.warm.repair.platform = { $space_2 }Reparatur: Keine Reparatur auf dieser Plattform nötig.

journal.cli.host.warm.row.development = GAP { $binary }: { $path } fehlt neben der Entwicklungs-Programmdatei

journal.cli.host.warm.row.executable = CANNOT LOAD { $binary }: aktuelle Programmdatei kann nicht aufgelöst werden: { $error }

journal.cli.host.warm.row.exit = RAN { $binary }: Exit { $code } zurückgegeben

journal.cli.host.warm.row.library = CANNOT LOAD { $binary }: gemeinsame Bibliothek { $library } konnte nicht geladen werden

journal.cli.host.warm.row.missing = MISSING { $binary }: { $path } fehlt neben der laufenden Programmdatei

journal.cli.host.warm.row.no.code = CANNOT LOAD { $binary }: ohne Exit-Code beendet

journal.cli.host.warm.row.platform = NOT APPLICABLE { $binary }: Nur-Linux-Binärdatei ist auf dieser Plattform nicht anwendbar

journal.cli.host.warm.row.signal = CANNOT LOAD { $binary }: durch Signal { $signal } beendet

journal.cli.host.warm.row.spawn = CANNOT LOAD { $binary }: Start von { $path } fehlgeschlagen: { $error }

journal.cli.host.warm.summary = { $quantity ->
    [one] Zusammenfassung: { $ran } gelaufen, { $missing } fehlend, { $cannot_load } nicht ladbar, { $gaps } Lücke, { $not_applicable } nicht anwendbar.
   *[other] Zusammenfassung: { $ran } gelaufen, { $missing } fehlend, { $cannot_load } nicht ladbar, { $gaps } Lücken, { $not_applicable } nicht anwendbar.
    }

journal.cli.host.warm.unresolved = { $space_2 }ungelöste Bibliothek: { $library }

journal.cli.host.windows.service.adopted.installation.has.no.journal.path = übernommene Installation hat keinen Journal-Pfad

journal.cli.host.windows.service.background.service.is.not.installed = Hintergrunddienst ist nicht installiert

journal.cli.host.windows.service.background.service.stopped = Hintergrunddienst gestoppt

journal.cli.host.windows.service.background.support.for.your.journal.couldn.t.be.changed.run = Hintergrundunterstützung für dein Journal konnte nicht geändert werden.{ $break_line }führe `{ $command_solstone_journal_service_status }` aus, um sie zu prüfen.{ $break_line }Details: { $reason }

journal.cli.host.windows.service.background.support.for.your.journal.couldn.t.be.checked.try = Hintergrundunterstützung für dein Journal konnte nicht geprüft werden.{ $break_line }versuch es noch einmal in einem Moment. wenn es weiter fehlschlägt, nimm die Details unten in eine Support-Anfrage auf.{ $break_line }Details: { $reason }

journal.cli.host.windows.service.before.uninstall.deadline.is.out.of.range = Frist vor der Deinstallation liegt außerhalb des Bereichs

journal.cli.host.windows.service.cannot.identify.one.running.service.task = kann nicht genau eine laufende Dienstaufgabe erkennen

journal.cli.host.windows.service.cannot.resolve.task.artifact.directory = Verzeichnis der Aufgabenartefakte kann nicht aufgelöst werden

journal.cli.host.windows.service.clean.uninstall.requires.an.adopted.installation.identity = saubere Deinstallation erfordert eine übernommene Installationsidentität

journal.cli.host.windows.service.could.not.clear.prior.update.intent = vorherige Aktualisierungsabsicht konnte nicht entfernt werden: { $error }

journal.cli.host.windows.service.could.not.clear.windows.sign.in.resume = Wiederaufnahme bei der { $literal_windows }-Anmeldung konnte nicht entfernt werden: { $error }

journal.cli.host.windows.service.could.not.consume.update.intent = Aktualisierungsabsicht konnte nicht verbraucht werden: { $error }

journal.cli.host.windows.service.could.not.inspect.current.executable = aktuelle Programmdatei konnte nicht untersucht werden: { $error }

journal.cli.host.windows.service.could.not.load.installation.binding = Installationsbindung konnte nicht geladen werden: { $error }

journal.cli.host.windows.service.could.not.locate.owner.base = Basis von dir konnte nicht gefunden werden: { $error }

journal.cli.host.windows.service.could.not.prepare.windows.sign.in.resume = Wiederaufnahme bei der { $literal_windows }-Anmeldung konnte nicht vorbereitet werden: { $error }

journal.cli.host.windows.service.could.not.read.update.intent.record = Datensatz der Aktualisierungsabsicht konnte nicht gelesen werden: { $error }

journal.cli.host.windows.service.could.not.resolve.identity.root.from.executable.directory = Identitätswurzel konnte nicht aus dem Verzeichnis der Programmdatei aufgelöst werden

journal.cli.host.windows.service.could.not.resolve.root.token = Wurzel-Token konnte nicht aufgelöst werden: { $error }

journal.cli.host.windows.service.could.not.resolve.uninstall.root = Deinstallationswurzel konnte nicht aufgelöst werden

journal.cli.host.windows.service.could.not.retrieve.user.sid = User { $literal_sid } konnte nicht abgerufen werden: { $error }

journal.cli.host.windows.service.could.not.write.update.intent.record = Datensatz der Aktualisierungsabsicht konnte nicht geschrieben werden: { $error }

journal.cli.host.windows.service.direct.worker.stop.request.direct.worker.observation = Stoppanfrage von direct-worker: { $termination }; Beobachtung von direct-worker: { $observed }

journal.cli.host.windows.service.error.invalid.port = Fehler: ungültiger Port '{ $port }'

journal.cli.host.windows.service.installation.identity.census.is.unreadable = Zensus der Installationsidentität ist unlesbar

journal.cli.host.windows.service.installation.provider.location.is.unavailable = Ort des Installationsanbieters ist nicht verfügbar

journal.cli.host.windows.service.installed.command.contains.invalid.text = installierter Befehl enthält ungültigen Text

journal.cli.host.windows.service.installed.task.action.did.not.match.its.readback = Aktion der installierten Aufgabe stimmte nicht mit ihrer Rücklesung überein

journal.cli.host.windows.service.is.registered.expected = { $registered } ist registriert, erwartet { $expected }

journal.cli.host.windows.service.it.belongs.to.a.different.installation.of.the.journal = es gehört zu einer anderen Installation des Journals

journal.cli.host.windows.service.it.is.registered.to.another.account = es ist für ein anderes Konto registriert

journal.cli.host.windows.service.it.runs.the.journal.at.expected = es führt das Journal unter { $registered } aus, erwartet { $journal }

journal.cli.host.windows.service.journal = journal: { $error }

journal.cli.host.windows.service.journal.path.contains.invalid.text = der Journalpfad enthält ungültigen Text

journal.cli.host.windows.service.native.process.birth.changed = die native Prozessgeburt hat sich geändert

journal.cli.host.windows.service.native.process.birth.is.missing = die native Prozessgeburt fehlt

journal.cli.host.windows.service.native.process.pid.is.invalid = die PID des nativen Prozesses ist ungültig

journal.cli.host.windows.service.native.windows.directory.is.unavailable = das native { $literal_windows }-Verzeichnis ist nicht verfügbar

journal.cli.host.windows.service.no.reason = Windows hat keinen Grund genannt.

journal.cli.host.windows.service.scheduler.returned.no.task.run.instance = die Aufgabenplanung hat keine Laufinstanz der Aufgabe zurückgegeben

journal.cli.host.windows.service.scheduler.rpc.may.have.changed.state.re.inspection.required = { $reason }; { $observation }; die RPC der Aufgabenplanung hat den Zustand möglicherweise geändert; erneute Prüfung erforderlich

journal.cli.host.windows.service.selected.journal.differs.from.the.saved.installation.binding = das ausgewählte Journal weicht von der gespeicherten Installationsbindung ab

journal.cli.host.windows.service.service.exited.abnormally.cleanup.remains.unverified = der Dienst wurde nicht normal beendet; die Bereinigung bleibt ungeprüft

journal.cli.host.windows.service.service.forwarder.admission.is.invalid = die Zulassung des Dienstweiterleiters ist ungültig

journal.cli.host.windows.service.service.installation.guard.differs.from.the.saved.binding = der Installationswächter des Dienstes weicht von der gespeicherten Bindung ab

journal.cli.host.windows.service.service.readiness.has.no.exact.native.identity = die Dienstbereitschaft hat keine exakte native Identität

journal.cli.host.windows.service.service.readiness.has.no.installed.forwarder.admission = die Dienstbereitschaft hat keine installierte Zulassung des Weiterleiters

journal.cli.host.windows.service.service.readiness.identity.changed = die Identität der Dienstbereitschaft hat sich geändert

journal.cli.host.windows.service.service.run.exited.during.admission = der Dienstlauf wurde während der Zulassung beendet

journal.cli.host.windows.service.service.scheduler.instance.does.not.match.its.admitted.forwarder = die Aufgabenplanungsinstanz des Dienstes stimmt nicht mit ihrem zugelassenen Weiterleiter überein

journal.cli.host.windows.service.service.stop.deadline.elapsed.cleanup.remains.unverified = die Stoppfrist des Dienstes ist abgelaufen; die Bereinigung bleibt ungeprüft

journal.cli.host.windows.service.service.task.changed.during.run.admission = die Dienstaufgabe hat sich während der Laufzulassung geändert

journal.cli.host.windows.service.service.task.changed.during.start = die Dienstaufgabe hat sich beim Start geändert

journal.cli.host.windows.service.service.task.changed.during.stop = die Dienstaufgabe hat sich beim Stopp geändert

journal.cli.host.windows.service.service.task.changed.while.recording.its.state = die Dienstaufgabe hat sich geändert, während ihr Zustand festgehalten wurde

journal.cli.host.windows.service.service.task.deletion.was.not.observed = das Löschen der Dienstaufgabe wurde nicht beobachtet

journal.cli.host.windows.service.service.task.differs.from.the.current.installation.binding = die Dienstaufgabe weicht von der aktuellen Installationsbindung ab

journal.cli.host.windows.service.service.task.instance.changed.during.stop = die Instanz der Dienstaufgabe hat sich beim Stopp geändert

journal.cli.host.windows.service.service.task.instance.changed.while.retaining.native.handles = die Instanz der Dienstaufgabe hat sich geändert, während native Handles gehalten wurden

journal.cli.host.windows.service.service.task.is.absent.and.its.forwarder.cleanup.cannot.be = die Dienstaufgabe ist abwesend, und die Bereinigung ihres Weiterleiters kann nicht geprüft werden

journal.cli.host.windows.service.service.task.is.not.installed = die Dienstaufgabe ist nicht installiert

journal.cli.host.windows.service.stopped.background.support.for.your.journal.starts.again.when.you = Gestoppt: der Hintergrundsupport für dein Journal startet wieder, wenn du dich das nächste Mal anmeldest.

journal.cli.host.windows.service.stopped.background.support.for.your.journal.stays.off.until.you = Gestoppt: der Hintergrundsupport für dein Journal bleibt aus, bis du ihn startest oder die Journal-App öffnest.

journal.cli.host.windows.service.supervisor.readiness.not.ready = Supervisor-Bereitschaft: nicht bereit

journal.cli.host.windows.service.supervisor.readiness.ready = Supervisor-Bereitschaft: bereit

journal.cli.host.windows.service.systemroot.is.unavailable = SystemRoot ist nicht verfügbar

journal.cli.host.windows.service.task = Aufgabe: { $task }

journal.cli.host.windows.service.task.control.deadline.elapsed.before.spawn = die Frist der Aufgabensteuerung ist vor dem Start abgelaufen

journal.cli.host.windows.service.task.control.exchange.failed = der Austausch der Aufgabensteuerung ist fehlgeschlagen: { $error }

journal.cli.host.windows.service.task.control.exchange.failed.worker.channel.closed = der Austausch der Aufgabensteuerung ist fehlgeschlagen: der Worker-Kanal ist geschlossen

journal.cli.host.windows.service.task.control.exchange.timed.out = Zeitüberschreitung beim Austausch der Aufgabensteuerung

journal.cli.host.windows.service.task.control.input.exceeds.its.byte.limit = die Eingabe der Aufgabensteuerung überschreitet ihre Bytegrenze

journal.cli.host.windows.service.task.control.output.exceeds.its.byte.limit = die Ausgabe der Aufgabensteuerung überschreitet ihre Bytegrenze

journal.cli.host.windows.service.task.control.spawn.failed = der Start der Aufgabensteuerung ist fehlgeschlagen: { $error }

journal.cli.host.windows.service.task.control.stdio.missing = stdio der Aufgabensteuerung fehlt

journal.cli.host.windows.service.task.control.worker.exited = der Worker der Aufgabensteuerung wurde beendet

journal.cli.host.windows.service.task.operation.returned.inconsistent.evidence = die Aufgabenoperation hat einen widersprüchlichen Nachweis zurückgegeben

journal.cli.host.windows.service.task.operation.returned.invalid.json = die Aufgabenoperation hat ungültiges { $literal_json } zurückgegeben

journal.cli.host.windows.service.task.request.is.not.a.json.object = die Aufgabenanfrage ist kein { $literal_json }-Objekt

journal.cli.host.windows.service.task.xml.readback.missing = die XML-Rücklesung der Aufgabe fehlt

journal.cli.host.windows.service.the.installed.solstone.exe.facade.is.unavailable = die installierte solstone.exe-Fassade ist nicht verfügbar

journal.cli.host.windows.service.the.registered.task.has.no.readable.definition = die registrierte Aufgabe hat keine lesbare Definition

journal.cli.host.windows.service.update.intent.record.is.invalid = der Datensatz der Aktualisierungsabsicht ist ungültig

journal.cli.host.windows.service.windows.run.key.was.not.opened = { $literal_windows }-Run-Schlüssel wurde nicht geöffnet

journal.cli.host.windows.service.windows.service.action.contains.non.unicode.text = die Aktion des Windows-Dienstes enthält Nicht-Unicode-Text

journal.cli.host.windows.service.windows.service.action.differs.from.the.complete.saved.binding = die Aktion des Windows-Dienstes weicht von der vollständigen gespeicherten Bindung ab

journal.cli.host.windows.service.windows.service.binding.unavailable = die Bindung des Windows-Dienstes ist nicht verfügbar

journal.cli.host.windows.service.windows.sign.in.resume.command.exceeds.the.run.key.limit = der Befehl zur { $literal_windows }-Anmeldungsfortsetzung überschreitet die Grenze des Run-Schlüssels oder hat einen ungültigen Pfad

journal.cli.host.windows.service.windows.sign.in.resume.failed.windows.error = { $literal_windows }-Anmeldungsfortsetzung { $action } fehlgeschlagen ({ $literal_windows }-Fehler { $code })

journal.cli.host.windows.service.windows.sign.in.resume.registry.readback.differed = die Registrierungs-Rücklesung der { $literal_windows }-Anmeldungsfortsetzung wich ab

journal.cli.host.windows.service.your.journal.didn.t.report.ready.so.the.start.can = dein Journal hat sich nicht als bereit gemeldet, daher lässt sich der Start nicht bestätigen. führe `{ $command_solstone_journal_service } logs` aus, um den Grund zu sehen

journal.cli.host.windows.service.your.journal.is.ready = dein Journal ist bereit

journal.cli.install.archive.binary = { $binary_name } fehlt im Archiv

journal.cli.install.archive.digest = { $algorithm }-Abweichung: erwartet { $expected }, erhalten { $actual }

journal.cli.install.archive.download = Download fehlgeschlagen: { $message }

journal.cli.install.archive.host = Downloadhost abgelehnt: { $host }

journal.cli.install.archive.origin = Downloadursprung nicht verfügbar unter { $host }: { $message }

journal.cli.install.archive.path = Archivbestandteil verlässt das Ziel: { $path }

journal.cli.install.archive.redirect = Sprunggrenze der Downloadweiterleitung überschritten: { $limit }

journal.cli.install.archive.scheme = Downloadschema für { $host } abgelehnt: { $scheme }

journal.cli.install.archive.size = { $quantity ->
    [one] Abweichung der Downloadgröße: erwartet { $expected } Byte, erhalten { $actual }
   *[other] Abweichung der Downloadgröße: erwartet { $expected } Bytes, erhalten { $actual }
}

journal.cli.install.archive.userinfo = die Authority der Download-URL darf userinfo nicht enthalten: { $authority }

journal.cli.install.backend.metal = backend muss "metal" sein, wenn es angegeben ist

journal.cli.install.backend.unavailable = das angeforderte lokale backend ist auf dieser Plattform nicht verfügbar

journal.cli.install.component.packaged = Build-Defekt: diese Komponente wird im Paket mitgeliefert, und ein erneuter Versuch hilft nicht

journal.cli.install.component.unfetched = Build-Defekt: dieser Build lädt diese Komponente nicht herunter, und ein erneuter Versuch hilft nicht

journal.cli.install.coreml.asset.digest = Digest-Abweichung beim parakeet-{ $coreml }-Asset für { $file }: erwartet { $expected }, gefunden { $found }

journal.cli.install.coreml.asset.incomplete = das Asset des Sprachmodells ist unvollständig

journal.cli.install.coreml.asset.missing = parakeet-{ $coreml }-Asset fehlt: { $file }; installiere es mit: { $install_command }

journal.cli.install.coreml.asset.read = { $file } konnte nicht gelesen werden: { $error }

journal.cli.install.coreml.asset.size = Größenabweichung beim parakeet-{ $coreml }-Asset für { $file }: erwartet { $expected }, gefunden { $found }

journal.cli.install.coreml.attempt.changed = der Installationsversuch für die Spracherkennung hat sich geändert.

journal.cli.install.coreml.cache.directory = das Zwischenspeicherverzeichnis des parakeet-{ $coreml }-Sentinels ({ $installed }) stimmt nicht mit dem konfigurierten ({ $configured }) überein

journal.cli.install.coreml.catalog.conflict = die Katalogzeilen von { $artifact } haben widersprüchliche Versionen

journal.cli.install.coreml.catalog.empty = keine Katalogzeilen für { $artifact } gefunden

journal.cli.install.coreml.catalog.mismatch = Abweichung der Katalogversion von parakeet { $coreml }: gefunden { $found }, erwartet { $expected }

journal.cli.install.coreml.platform = parakeet { $coreml } wird nur auf { $host } unterstützt, nicht auf { $os_name }/{ $arch }

journal.cli.install.coreml.publication.lock = die Veröffentlichungssperre konnte nicht erworben werden: { $error }

journal.cli.install.coreml.publish.restore = { $error }; Wiederherstellung fehlgeschlagen: { $restore }

journal.cli.install.coreml.sentinel.not.ready = parakeet-{ $coreml }-Sentinel nicht bereit: { $path }; installiere es mit: { $install_command }

journal.cli.install.coreml.sentinel.removed = der bisherige Sentinel wurde vor der Veröffentlichung entfernt

journal.cli.install.cuda.integrity = Integritätsfehler der CUDA-Laufzeitumgebung

journal.cli.install.disk.insufficient = nicht genug Speicherplatz, um das Modell fertig herunterzuladen

journal.cli.install.fingerprint.artifact = { $field } fehlt

journal.cli.install.fingerprint.backend = backend fehlt

journal.cli.install.fingerprint.missing = der Fingerabdruck des Versuchs fehlt

journal.cli.install.fingerprint.runtime = der Artefaktschlüssel der Laufzeitumgebung fehlt

journal.cli.install.fit.artifact.local = Artefakte des lokalen Anbieters

journal.cli.install.fit.artifact.parakeet = parakeet.cpp-Artefakte

journal.cli.install.fit.artifact.rfdetr = rf-detr.cpp-Artefakte

journal.cli.install.fit.check.disk = Datenträger

journal.cli.install.fit.check.gpu = GPU

journal.cli.install.fit.check.platform = Plattform

journal.cli.install.fit.check.ram = RAM

journal.cli.install.fit.disk.insufficient = nicht genug Speicherplatz für { $requirement } ({ $required } GB nötig, { $available } GB frei)

journal.cli.install.fit.disk.insufficient.named = nicht genug Speicherplatz für { $requirement } ({ $required } GB nötig, { $available } GB frei): { $known }

journal.cli.install.fit.disk.insufficient.named.unknown = nicht genug Speicherplatz für { $requirement } ({ $required } GB nötig, { $available } GB frei): { $known }; unbekannte Downloadgröße für { $unknown }

journal.cli.install.fit.disk.insufficient.unknown = nicht genug Speicherplatz für { $requirement } ({ $required } GB nötig, { $available } GB frei); unbekannte Downloadgröße für { $unknown }

journal.cli.install.fit.disk.item.cuda.tarball = CUDA-llama-server-Tarball

journal.cli.install.fit.disk.item.gguf.model = GGUF-Modell

journal.cli.install.fit.disk.item.llama.tarball = llama-server-Tarball

journal.cli.install.fit.disk.item.mlx.model = MLX-Modell

journal.cli.install.fit.disk.item.mmproj = mmproj

journal.cli.install.fit.disk.item.parakeet.cpu = parakeet-CPU-Server-Tarball

journal.cli.install.fit.disk.item.parakeet.model = parakeet-GGUF-Modell

journal.cli.install.fit.disk.item.parakeet.vulkan = parakeet-Vulkan-Server-Tarball

journal.cli.install.fit.disk.item.rfdetr.cli = rf-detr-CLI-Programmdatei

journal.cli.install.fit.disk.item.rfdetr.model = rf-detr-GGUF-Modell

journal.cli.install.fit.disk.ok = { $available } GB frei für { $required } GB { $requirement }

journal.cli.install.fit.disk.ok.unknown = { $available } GB frei; { $requirement } brauchen { $required } GB; unbekannte Downloadgröße für { $unknown }

journal.cli.install.fit.disk.overflow = Überlauf des verfügbaren Speicherplatzes

journal.cli.install.fit.disk.requirement.bundled = Installation mitgelieferter Assets

journal.cli.install.fit.disk.requirement.downloads = bekannte Downloads

journal.cli.install.fit.disk.unavailable = der verfügbare Speicherplatz unter { $path } konnte nicht geprüft werden: { $error }

journal.cli.install.fit.disk.unavailable.unknown = der verfügbare Speicherplatz unter { $path } konnte nicht geprüft werden: { $error }; unbekannte Downloadgröße für { $unknown }

journal.cli.install.fit.disk.unsupported = die Speicherplatzprüfung wird auf dieser Plattform nicht unterstützt

journal.cli.install.fit.gpu.cuda.memory.unknown = backend ist { $backend } (ermittelt): { $reason }; der GPU-Arbeitsspeicher ist unbekannt

journal.cli.install.fit.gpu.cuda.ok = CUDA-backend ausgewählt: { $reason }

journal.cli.install.fit.gpu.cuda.ok.cpu = CUDA-backend ausgewählt: { $reason }; ein Modell läuft auf deiner GPU; die Transkription läuft auf deiner CPU auf diesem Rechner

journal.cli.install.fit.gpu.cuda.ok.unified = CUDA-backend ausgewählt: { $reason }; der Arbeitsspeicher der GPU-Stufung nutzt das MemAvailable des Systems

journal.cli.install.fit.gpu.cuda.ok.unified.cpu = CUDA-backend ausgewählt: { $reason }; der Arbeitsspeicher der GPU-Stufung nutzt das MemAvailable des Systems; ein Modell läuft auf deiner GPU; die Transkription läuft auf deiner CPU auf diesem Rechner

journal.cli.install.fit.gpu.cuda.probe.unavailable = Nvidia-GPU-Sonde nicht verfügbar; backend ist { $backend } (ermittelt): { $reason }

journal.cli.install.fit.gpu.vulkan.below.bar = GPU { $name } hat { $available } GB; die mitgelieferten lokalen Modelle brauchen mindestens 6 GB; backend ist { $backend } (ermittelt): { $reason }

journal.cli.install.fit.gpu.vulkan.below.bar.cpu = GPU { $name } hat { $available } GB; die mitgelieferten lokalen Modelle brauchen mindestens 6 GB; backend ist { $backend } (ermittelt): { $reason }; ein Modell läuft auf deiner GPU; die Transkription läuft auf deiner CPU auf diesem Rechner

journal.cli.install.fit.gpu.vulkan.eligible = Vulkan-GPU ausgewählt: { $name }; backend ist { $backend } (ermittelt): { $reason }

journal.cli.install.fit.gpu.vulkan.eligible.cpu = Vulkan-GPU ausgewählt: { $name }; backend ist { $backend } (ermittelt): { $reason }; ein Modell läuft auf deiner GPU; die Transkription läuft auf deiner CPU auf diesem Rechner

journal.cli.install.fit.gpu.vulkan.memory.unknown = der GPU-Arbeitsspeicher konnte nicht gelesen werden; backend ist { $backend } (ermittelt): { $reason }

journal.cli.install.fit.gpu.vulkan.no.hardware = keine Hardware-Vulkan-GPU ausgewählt; backend ist { $backend } (ermittelt): { $reason }

journal.cli.install.fit.gpu.vulkan.probe.incomplete = die Vulkan-GPU-Sonde wurde nicht abgeschlossen; backend ist { $backend } (ermittelt): { $reason }

journal.cli.install.fit.header = Eignungsprüfung für { $artifact }: { $overall }

journal.cli.install.fit.platform.embedded.blocked = die Journal-App stellt diese lokale Laufzeitumgebung nicht bereit

journal.cli.install.fit.platform.embedded.ok = die lokale Laufzeitumgebung ist in der Journal-App enthalten

journal.cli.install.fit.platform.llama.blocked = Kein per Pin festgelegtes llama-server-Artefakt für die Plattform { $key }

journal.cli.install.fit.platform.llama.ok = das per Pin festgelegte llama-server-Artefakt ist für { $key } verfügbar

journal.cli.install.fit.platform.mlx.blocked = MLX braucht macOS auf Apple Silicon; erhalten { $os }/{ $arch }

journal.cli.install.fit.platform.mlx.ok = MLX-Laufzeitumgebung wird mit dem Journal mitgeliefert

journal.cli.install.fit.platform.parakeet.ok = die per Pin festgelegten parakeet.cpp-Artefakte sind für { $key } verfügbar

journal.cli.install.fit.platform.rfdetr.blocked = rf-detr.cpp braucht darwin/arm64, linux/x86_64 oder linux/arm64; erhalten { $os }/{ $arch }

journal.cli.install.fit.platform.rfdetr.ok = die per Pin festgelegten rf-detr.cpp-Artefakte sind für { $key } verfügbar

journal.cli.install.fit.platform.rfdetr.windows.ok = die Nutzdaten des signierten rf-detr.cpp-Pakets werden für { $os }/{ $arch } unterstützt

journal.cli.install.fit.platform.windows.llama.blocked = llama-server braucht eine unterstützte Windows-Architektur; erhalten { $os }/{ $arch }

journal.cli.install.fit.platform.windows.llama.ok = die Nutzdaten des signierten llama-server-Pakets werden für Windows/{ $arch } unterstützt

journal.cli.install.fit.ram.insufficient = nicht genug Arbeitsspeicher für { $model } (nötig { $required } GB verfügbar, { $available } GB verfügbar)

journal.cli.install.fit.ram.ok = { $available } GB verfügbarer Arbeitsspeicher erfüllt die Anforderung von { $required } GB für { $model }

journal.cli.install.fit.ram.unknown = der verfügbare Arbeitsspeicher für { $model } konnte nicht geprüft werden

journal.cli.install.fit.row = [{ $severity }] { $name }: { $detail }

journal.cli.install.fit.severity.blocked = blockiert

journal.cli.install.fit.severity.ok = ok

journal.cli.install.fit.severity.unknown = unbekannt

journal.cli.install.fit.severity.warning = Warnung

journal.cli.install.hardware.apple.memory = lokales Denken braucht mindestens { $minimum } Arbeitsspeicher

journal.cli.install.hardware.memory.minimum = { $available_bytes ->
    [one] { $vram } auf dem Gerät unter dem Minimum von { $minimum }: { $available_bytes } Byte
   *[other] { $vram } auf dem Gerät unter dem Minimum von { $minimum }: { $available_bytes } Bytes
}

journal.cli.install.hardware.memory.unknown = die { $vulkan }-Sonde konnte keinen Arbeitsspeicher melden

journal.cli.install.hardware.no.hardware = keine Hardware-{ $vulkan }-GPU gefunden

journal.cli.install.hardware.probe.required = { $vulkan }-GPU-Beobachtung auf Windows erforderlich

journal.cli.install.lease.held = die Installationslease von { $provider } wird gehalten

journal.cli.install.lease.regular = die Installationslease ist keine reguläre Datei

journal.cli.install.member.cuda.arch = CUDA-Arch { $arch } wird nicht unterstützt

journal.cli.install.member.cuda.arch.key = CUDA-Arch-Schlüssel { $artifact_key } wird nicht unterstützt

journal.cli.install.member.cuda.missing = erforderlicher CUDA-Bestandteil fehlt: { $name }

journal.cli.install.member.cuda.regular = erforderlicher CUDA-Bestandteil ist keine reguläre Datei: { $name }

journal.cli.install.member.link.broken = defekter Link in der Kette für { $name }

journal.cli.install.member.link.cycle = Linkzyklus erkannt für { $name }

journal.cli.install.member.link.missing = erforderlicher Link-Bestandteil fehlt: { $name }

journal.cli.install.member.link.read = Link { $name } kann nicht gelesen werden: { $error }

journal.cli.install.member.link.regular = Link { $name } wird nicht zu einer regulären Datei aufgelöst

journal.cli.install.member.link.target = Zielabweichung bei Link { $name }: erwartet { $target }, erhalten { $found }

journal.cli.install.member.link.type = erforderlicher Link-Bestandteil ist kein symbolischer Link: { $name }

journal.cli.install.member.link.unreadable = Link in der Kette für { $name } kann nicht gelesen werden

journal.cli.install.member.regular.missing = erforderlicher regulärer Bestandteil fehlt: { $name }

journal.cli.install.member.regular.type = erforderlicher regulärer Bestandteil ist keine reguläre Datei: { $name }

journal.cli.install.metal.platform = lokale { $metal }-Inferenz wird auf { $key } nicht unterstützt

journal.cli.install.mlx.digest.read = { $function } für { $file } fehlgeschlagen: { $error }

journal.cli.install.mlx.directory.explicit = explizites { $mlx }-Modellverzeichnis existiert nicht: { $path }

journal.cli.install.mlx.directory.invalid = das Modellverzeichnis ist kein Verzeichnis: { $path }

journal.cli.install.mlx.directory.override = { $env } existiert nicht: { $path }

journal.cli.install.mlx.directory.read = das Modellverzeichnis kann nicht gelesen werden: { $error }

journal.cli.install.mlx.directory.unset = { $env } ist nicht gesetzt oder kein Verzeichnis

journal.cli.install.mlx.entry.read = der Eintrag konnte nicht gelesen werden: { $error }

journal.cli.install.mlx.file.digest = sha256-Abweichung für { $file }: erwartet { $expected } erhalten { $actual }

journal.cli.install.mlx.file.missing = erforderliche Datei fehlt: { $file }

journal.cli.install.mlx.file.regular = keine reguläre Datei: { $file }

journal.cli.install.mlx.file.size = Größenabweichung für { $file }: erwartet { $expected } erhalten { $actual }

journal.cli.install.mlx.helper.explicit = der explizite { $mlx }-Helfer ist nicht gültig oder { $library } fehlt: { $path }

journal.cli.install.mlx.helper.invalid = die Helferprogrammdatei ist ungültig: { $path }

journal.cli.install.mlx.helper.missing = { $helper }-Programmdatei mit Ressourcenbündel { $library } nicht gefunden

journal.cli.install.mlx.helper.override = { $env } ist nicht gültig oder { $library } fehlt: { $path }

journal.cli.install.mlx.input.extra = zusätzliche ungeprüfte Modelleingabe: { $file }

journal.cli.install.mlx.metadata.read = die Metadaten für { $file } können nicht gelesen werden: { $error }

journal.cli.install.mlx.override.failed = die Prüfung der { $mlx }-Überschreibung ist fehlgeschlagen

journal.cli.install.mlx.pins.empty = leere Pin-Liste

journal.cli.install.mlx.pins.missing = keine Pins angegeben

journal.cli.install.mlx.platform = { $mlx } braucht { $os }

journal.cli.install.mlx.type.read = Dateityp fehlgeschlagen: { $error }

journal.cli.install.pdfium.windows.executable.location = die laufende Programmdatei des Journals liegt nicht im bin-Verzeichnis des Pakets: { $path }

journal.cli.install.pdfium.windows.executable.parent = die laufende Programmdatei des Journals hat kein übergeordnetes Verzeichnis: { $path }

journal.cli.install.pdfium.windows.executable.read = die laufende Programmdatei des Journals konnte nicht bestimmt werden: { $error }

journal.cli.install.pdfium.windows.package.root = das bin-Verzeichnis des Pakets hat keine Paketwurzel: { $path }

journal.cli.install.pdfium.windows.payload.refuses = die signierten Nutzdaten der PDF-App lehnen { $member } ab: { $error }

journal.cli.install.pdfium.windows.payload.verify = die signierten Nutzdaten der PDF-App konnten nicht geprüft werden: { $error }

journal.cli.install.pdfium.windows.runtime = Windows-PDF-Paketprüfung braucht eine Windows-Laufzeitumgebung

journal.cli.install.pin.missing = kein Pin für { $key }

journal.cli.install.registry.artifact.ambiguous = der Katalog hat mehrere passende { $unit }-Artefakte

journal.cli.install.registry.artifact.missing = der Katalog hat kein passendes { $unit }-Artefakt

journal.cli.install.registry.platform = keine Plattformzuordnung im Katalog für { $key }

journal.cli.install.rfdetr.asset.missing = rf-detr-Asset fehlt: { $label }

journal.cli.install.rfdetr.binary.missing = Entpacktes Archiv enthielt { $binary } nicht

journal.cli.install.rfdetr.bundled.missing = mitgeliefertes rf-detr-Asset fehlt: { $filename }; installiere das Journal-Paket neu

journal.cli.install.rfdetr.contract.missing = kompilierter rf-detr-Zustellungsvertrag fehlt

journal.cli.install.rfdetr.contract.slot = der Slot { $actual } des kompilierten rf-detr-Zustellungsvertrags stimmt nicht mit { $expected } überein

journal.cli.install.rfdetr.member.mismatch = der rf-detr-Programmdatei-Bestandteil { $actual } stimmt nicht mit { $expected } überein

journal.cli.install.rfdetr.member.outside = die rf-detr-Programmdatei lag außerhalb der Entpackungswurzel: { $error }

journal.cli.install.rfdetr.probe.failed = die rf-detr-Engine-Sonde ist fehlgeschlagen: { $result }

journal.cli.install.rfdetr.sha256.mismatch = sha256-Abweichung für { $label }

journal.cli.install.rfdetr.sidecar.invalid = rf-detr-Sidecar ungültig: { $path }: { $error }

journal.cli.install.rfdetr.sidecar.mismatch = das rf-detr-Sidecar stimmt nicht mit den per Pin festgelegten Artefakten überein

journal.cli.install.rfdetr.sidecar.missing = rf-detr-Sidecar fehlt: { $path }

journal.cli.install.rfdetr.size.mismatch = Größenabweichung für { $label }: erwartet { $expected }, erhalten { $actual }

journal.cli.install.rfdetr.windows.detect.failed = rfdetr-cli detect fehlgeschlagen

journal.cli.install.rfdetr.windows.executable.location = die laufende Programmdatei des Journals liegt nicht im bin-Verzeichnis des Pakets: { $path }

journal.cli.install.rfdetr.windows.executable.parent = die laufende Programmdatei des Journals hat kein übergeordnetes Verzeichnis

journal.cli.install.rfdetr.windows.executable.read = die laufende Programmdatei des Journals konnte nicht bestimmt werden: { $error }

journal.cli.install.rfdetr.windows.package.root = das bin-Verzeichnis des Pakets hat keine Paketwurzel

journal.cli.install.rfdetr.windows.result.limit = RF-DETR-Ergebnis hat seine Bytegrenze überschritten

journal.cli.install.rfdetr.windows.runtime = Windows-RF-DETR-Paketprüfung braucht eine Windows-Laufzeitumgebung

journal.cli.install.rfdetr.windows.verify.failed = Windows-RF-DETR-Paketprüfung fehlgeschlagen: { $error }

journal.cli.install.runtime.unavailable = die Journal-App stellt diese lokale Laufzeitumgebung nicht bereit

journal.cli.install.speech.platform = { $program } wird auf { $os_name }/{ $arch } nicht unterstützt

journal.cli.install.staging.binary.parent = die Programmdatei hat kein übergeordnetes Verzeichnis

journal.cli.install.staging.bundle.occupied = das Bündel-Ziel existiert bereits: { $path }

journal.cli.install.staging.target.name = das Ziel hat keinen Dateinamen

journal.cli.install.staging.target.parent = das Ziel hat kein übergeordnetes Verzeichnis

journal.cli.install.status.conflict.different.attempt.while.install.in.flight = Konflikt beim Installationsstatus: anderer Versuch während einer laufenden Installation

journal.cli.install.status.conflict.illegal.install.status.transition = Konflikt beim Installationsstatus: unzulässiger Übergang des Installationsstatus

journal.cli.install.status.conflict.install.attempt.id.changed = Konflikt beim Installationsstatus: die ID des Installationsversuchs hat sich geändert

journal.cli.install.status.conflict.install.attempt.is.no.longer.in.flight = Konflikt beim Installationsstatus: der Installationsversuch ist nicht mehr laufend

journal.cli.install.status.conflict.install.progress.can.only.be.bumped.for = Konflikt beim Installationsstatus: der Installationsfortschritt lässt sich nur für laufende Zustände erhöhen

journal.cli.install.status.conflict.install.target.fingerprint.changed = Konflikt beim Installationsstatus: der Fingerabdruck des Installationsziels hat sich geändert

journal.cli.install.status.conflict.interrupted.attempt.id.does.not.match = Konflikt im Installationsstatus: die ID des unterbrochenen Versuchs stimmt nicht überein

journal.cli.install.status.conflict.interrupted.target.fingerprint.does.not.match = Konflikt im Installationsstatus: der Fingerabdruck des unterbrochenen Ziels stimmt nicht überein

journal.cli.install.status.conflict.new.in.flight.attempt.reused.attempt.id = Konflikt im Installationsstatus: ein neuer laufender Versuch hat die Versuchs-ID wiederverwendet

journal.cli.install.status.conflict.non.idle.install.status.requires.attempt.id = Konflikt im Installationsstatus: ein Installationsstatus, der nicht im Leerlauf ist, erfordert eine Versuchs-ID

journal.cli.install.status.conflict.only.in.flight.installs.can.be.interrupted = Konflikt im Installationsstatus: nur laufende Installationen können unterbrochen werden

journal.cli.install.status.conflict.stale.install.status.revision = Konflikt im Installationsstatus: veraltete Revision des Installationsstatus

journal.cli.install.status.interrupted = der Download wurde unterbrochen

journal.cli.install.status.lock = Fehler der Statussperre: { $error }

journal.cli.install.status.malformed.file = fehlerhafter Installationsstatus: { $path }

journal.cli.install.status.malformed.initial.install.attempt.state.must.be.in = fehlerhafter Installationsstatus: der Anfangszustand des Installationsversuchs muss laufend sein

journal.cli.install.status.malformed.invalid.provider = fehlerhafter Installationsstatus: ungültiger Status für { $provider }

journal.cli.install.status.malformed.owner.must.be.an.object.or.null = fehlerhafter Installationsstatus: owner muss ein Objekt oder null sein

journal.cli.install.status.malformed.providers = fehlerhafter Installationsstatus: der Installationsstatus des Anbieters muss einer der folgenden sein: [{ $listed }]

journal.cli.install.status.malformed.state = fehlerhafter Installationsstatus: unbekannter Installationszustand: { $state }

journal.cli.install.terminal.write = die native Installation ist fehlgeschlagen und der Status konnte nicht geschrieben werden

journal.cli.install.windows.digest.absent = das Manifest der signierten Nutzdaten deklariert keinen Digest für { $path }

journal.cli.install.windows.executable.location = die laufende Programmdatei des Journals liegt nicht im bin-Verzeichnis des Pakets: { $path }

journal.cli.install.windows.executable.parent = die laufende Programmdatei des Journals hat kein übergeordnetes Verzeichnis

journal.cli.install.windows.executable.read = die laufende Programmdatei des Journals konnte nicht bestimmt werden: { $error }

journal.cli.install.windows.package.root = das bin-Verzeichnis des Pakets hat keine Paketwurzel

journal.cli.install.windows.package.runtime = Windows-Paketprüfung für lokales Denken erfordert eine Windows-Laufzeitumgebung

journal.cli.install.windows.payload.bytes = Bytes{ $line_break }  { $detail }

journal.cli.install.windows.payload.case.collision = Kollision der Groß-/Kleinschreibung{ $line_break }  { $detail }

journal.cli.install.windows.payload.digest = Digest{ $line_break }  { $detail }

journal.cli.install.windows.payload.empty = leer{ $line_break }  { $detail }

journal.cli.install.windows.payload.lock.digest = cargo-lock-sha256{ $line_break }  { $detail }

journal.cli.install.windows.payload.manifest = Manifest{ $line_break }  { $detail }

journal.cli.install.windows.payload.missing = fehlend{ $line_break }  { $detail }

journal.cli.install.windows.payload.missing.member = fehlender Bestandteil{ $line_break }  { $detail }

journal.cli.install.windows.payload.not.regular = keine reguläre Datei{ $line_break }  { $detail }

journal.cli.install.windows.payload.reparse.or.symlink = Reparsepunkt oder symbolischer Link{ $line_break }  { $detail }

journal.cli.install.windows.payload.schema = Schema{ $line_break }  { $detail }

journal.cli.install.windows.payload.signature = Signatur{ $line_break }  { $detail }

journal.cli.install.windows.payload.source.commit = Quell-Commit{ $line_break }  { $detail }

journal.cli.install.windows.payload.target = Ziel{ $line_break }  { $detail }

journal.cli.install.windows.payload.unexpected.member = unerwarteter Bestandteil{ $line_break }  { $detail }

journal.cli.install.windows.payload.unsafe.path = unsicherer Pfad{ $line_break }  { $detail }

journal.cli.install.windows.payload.unsorted.or.duplicate = unsortiert oder doppelt{ $line_break }  { $detail }

journal.cli.install.windows.platform = nicht unterstützte Windows-Plattform: { $key }

journal.cli.install.windows.target.different = das Ziel der signierten Nutzdaten unterscheidet sich von { $key }

journal.cli.install.windows.target.unsupported = das Ziel der signierten Nutzdaten wird nicht unterstützt

journal.cli.path.create = das Journal-Verzeichnis konnte nicht erstellt werden ({ $source }): { $path }: { $error }

journal.cli.path.home = journal-path fehlgeschlagen: das Home-Verzeichnis konnte nicht bestimmt werden

journal.cli.path.utf8 = journal-path fehlgeschlagen: die Konfiguration ist kein gültiges { $encoding }
